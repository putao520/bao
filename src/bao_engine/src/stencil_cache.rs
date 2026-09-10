// @trace REQ-ENG-001 [entity:JsContext]
//! Per-JSContext in-memory Stencil cache (SM-EVOLUTION #26, verdict 2026-09-10).
//!
//! Judgment basis (ledger `.plans/spidermonkey-evolution.md` §8 #26, bench
//! `stencil-cost` @ 03396a13): the production stealth blob re-pays
//! parse+bytecode-compile on EVERY realm — `JS::Evaluate` in this SM snapshot
//! has no eval cache (`EvaluateSourceBuffer` calls `CompileGlobalScript`
//! per invocation) — and for the 28,221 B stealth blob that compile share is
//! 79.6% of the warm-realm injection cost (4.9× instantiate-vs-recompile
//! speedup, breakeven 0.9 realm). This module caches the compiled
//! `JS::Stencil` per source and re-instantiates it into each target realm.
//!
//! ## Ownership / lifecycle contract (why per-JSContext)
//!
//! - Stencils are NOT `Send`/`Sync` and are runtime-scoped
//!   (`js/public/experimental/JSStencil.h`: "may be instantiated into any
//!   Realm on the current runtime and may be used multiple times") — the
//!   mozjs `unsafe impl Send/Sync for Stencil` is commented out upstream.
//!   The cache therefore lives in a **thread-local** keyed by the owning
//!   `JSContext` pointer: one cache per thread's context (S0-3 topology —
//!   each ScriptThread/CLI/worker thread owns its own cx).
//! - Stencil memory is process-heap refcounted
//!   (`InitialStencilAndDelazifications::Release() → js_delete(this)`; the
//!   owned `CompilationStencil` carries its own `LifoAlloc` + a plain
//!   `RefPtr<ScriptSource>`), so releasing entries after the context died
//!   is a pure heap free — no cx access. Eviction/reset release eagerly;
//!   a thread that dies with entries still cached (servo worker threads)
//!   releases them from the TLS destructor, which is safe for the same
//!   reason.
//! - Accessing with a different `cx` pointer rebinds ownership and drops
//!   all entries: a stencil from one runtime must never be instantiated
//!   into another, and `shutdown_thread_sm()` clears explicitly so a
//!   recycled context address can never be served stale stencils.
//!
//! ## Cache key (collision-proof by construction)
//!
//! Fast path hashes the source; the hit then verifies the full source
//! bytes + filename + line exactly (both stored in the entry). A hash
//! collision therefore degrades to a miss, never to executing the wrong
//! stencil. Filename and line are part of the key because they are baked
//! into the stencil's `ScriptSource` (error messages / `Error.stack`).
//! Compile options otherwise come from the same fixed
//! `CompileOptionsWrapper::new(cx, filename, line)` defaults that every
//! current caller of `mozjs::rust::evaluate_script` uses — there are no
//! other option degrees of freedom to fingerprint.
//!
//! ## Admission & capacity policy (ledger #26 verdict ③)
//!
//! - Sources below [`MIN_CACHED_SOURCE_BYTES`] bypass the cache entirely
//!   (tiny sources compile in ~2 µs; the ledger bench shows lookup overhead
//!   would dominate — `tiny_1p1` compile share is only 13.8%).
//! - Capacity is [`MAX_ENTRIES`] with LRU eviction (stealth blobs are
//!   per-profile, a handful in any session; 16 covers distinct
//!   profile-blob variants with margin). Evicted stencils are released.
//!
//! ## Semantics parity with `mozjs::rust::evaluate_script`
//!
//! Same utf8 source transform, same `CompileOptionsWrapper` defaults, same
//! AutoRealm-into-global, same error contract (`Err(())` with the pending
//! exception left for the caller, `maybe_resume_unwind()` on failure).
//! The only engine-visible deltas: the instantiated top-level script does
//! not carry the `isRunOnce` hint (each call still executes a FRESH
//! `JSScript` instantiated from the stencil, so the interpreter's
//! run-once-multiple-times trap can never fire — and `TreatAsRunOnce`
//! only *permits* internal reuse, its absence is strictly conservative),
//! and per-realm lazy functions delazify from the stencil-retained source
//! exactly as the header's multi-use contract specifies.
//!
//! Internal experimental surface (SM-EVOLUTION), not a stable API
//! commitment — hence `#[doc(hidden)]` on the observability helpers.

use std::cell::RefCell;
use std::ffi::CStr;
use std::ptr;

use mozjs::jsapi;
use mozjs::panic::maybe_resume_unwind;
use mozjs::realm::AutoRealm;
use mozjs::rooted;
use mozjs::rust::{
    CompileOptionsWrapper, HandleObject, MutableHandleValue, transform_str_to_source_text,
    wrappers2,
};

/// Maximum cached stencils per JSContext (LRU eviction beyond this).
/// Stealth blobs are per-profile; 16 covers all realistic profile variants.
const MAX_ENTRIES: usize = 16;

/// Sources below this size bypass the cache (tiny compiles are ~2 µs; the
/// hash+lookup fixed cost would dominate — ledger #26 bench `tiny_1p1`).
const MIN_CACHED_SOURCE_BYTES: usize = 1024;

struct CacheEntry {
    /// Fast bucket locator; hits are verified by exact bytes below, so a
    /// collision degrades to a miss.
    key: u64,
    /// Verbatim source — exact-match verification makes the key sound.
    source: Box<str>,
    filename: Box<[u8]>,
    line: u32,
    /// Addref'd stencil owned by the cache (one reference; released on
    /// eviction / reset / TLS drop).
    stencil: *mut jsapi::Stencil,
}

impl Drop for CacheEntry {
    fn drop(&mut self) {
        if !self.stencil.is_null() {
            unsafe { jsapi::StencilRelease(self.stencil) };
        }
    }
}

struct StencilCache {
    /// The JSContext whose runtime produced the cached stencils. Any access
    /// with a different cx resets the cache (cross-runtime instantiation is
    /// out of the Stencil contract).
    owner_cx: *mut jsapi::JSContext,
    /// Front = most recently used; the Vec order IS the LRU order.
    entries: Vec<CacheEntry>,
    hits: u64,
    misses: u64,
    bypasses: u64,
}

impl StencilCache {
    const fn new() -> Self {
        StencilCache {
            owner_cx: ptr::null_mut(),
            entries: Vec::new(),
            hits: 0,
            misses: 0,
            bypasses: 0,
        }
    }

    fn reset(&mut self, owner_cx: *mut jsapi::JSContext) {
        self.entries.clear(); // drops entries → StencilRelease
        self.owner_cx = owner_cx;
    }

    /// Exact-match lookup that bumps LRU order. No JS runs under the caller's
    /// borrow — this only touches owned memory.
    fn lookup(&mut self, key: u64, source: &str, filename: &CStr, line: u32) -> Option<*mut jsapi::Stencil> {
        let idx = self.entries.iter().position(|e| {
            e.key == key
                && e.line == line
                && e.filename.as_ref() == filename.to_bytes()
                && e.source.as_bytes() == source.as_bytes()
        })?;
        // Move to front (MRU).
        let entry = self.entries.remove(idx);
        let stencil = entry.stencil;
        self.entries.insert(0, entry);
        Some(stencil)
    }

    /// Insert a freshly compiled (already-addrefed) stencil, evicting the
    /// LRU victim past capacity. A same-key-different-bytes slot is replaced
    /// (hash collision resolution — exact key semantics preserved).
    fn insert(&mut self, key: u64, source: &str, filename: &CStr, line: u32, stencil: *mut jsapi::Stencil) {
        if let Some(idx) = self.entries.iter().position(|e| e.key == key) {
            // Same hash slot: exact same content would have been a lookup
            // hit, so reaching here means different bytes — replace.
            self.entries.remove(idx); // drop → StencilRelease of the old stencil
        }
        while self.entries.len() >= MAX_ENTRIES {
            self.entries.pop(); // LRU tail → drop → StencilRelease
        }
        self.entries.insert(
            0,
            CacheEntry {
                key,
                source: source.into(),
                filename: filename.to_bytes().to_vec().into_boxed_slice(),
                line,
                stencil,
            },
        );
    }
}

thread_local! {
    static CACHE: RefCell<StencilCache> = RefCell::new(StencilCache::new());
}

fn hash_key(source: &str, filename: &CStr, line: u32) -> u64 {
    // Wyhash (workspace `bun_wyhash`, Bun's port): ~0.1 ns/byte, so hashing
    // the 28 KB stealth blob costs ~3 µs vs ~15-19 µs for std's SipHash —
    // the residual overhead of a cache hit is dominated by this hash. The
    // hash is only a bucket locator: the exact-bytes verification in
    // `StencilCache::lookup` makes any collision degrade to a miss, never
    // to executing the wrong stencil.
    let mut h = bun_wyhash::Wyhash::init(0);
    h.update(source.as_bytes());
    h.update(filename.to_bytes());
    h.update(&line.to_ne_bytes());
    h.final_()
}

/// Evaluate `script` as a global script in `glob`'s realm, caching the
/// compiled stencil per (source, filename, line) on this thread's JSContext.
///
/// Drop-in for `mozjs::rust::evaluate_script` with identical error contract:
/// `Err(())` leaves the pending exception on the context for the caller.
/// Sources below [`MIN_CACHED_SOURCE_BYTES`] take the plain evaluate path
/// unchanged (no compile-stencil round trip).
///
/// # Safety
/// Same as `evaluate_script`: `glob` must be a live global object of a realm
/// on `cx`'s runtime, on the current thread.
pub fn evaluate_script_cached(
    cx: &mut mozjs::context::JSContext,
    glob: HandleObject,
    script: &str,
    filename: &CStr,
    line: u32,
    rval: MutableHandleValue,
) -> Result<(), ()> {
    // Below the admission floor the plain path is cheaper than the lookup:
    // identical semantics (it is the exact production path), no store.
    if script.len() < MIN_CACHED_SOURCE_BYTES {
        CACHE.with(|c| c.borrow_mut().bypasses += 1);
        let options = CompileOptionsWrapper::new(cx, filename.to_owned(), line);
        return mozjs::rust::evaluate_script(cx, glob, script, rval, options);
    }

    let key = hash_key(script, filename, line);
    let raw_cx = unsafe { cx.raw_cx() };

    // 1) Lookup — never holds the borrow across JS execution.
    let hit = CACHE.with(|c| {
        let mut cache = c.borrow_mut();
        if cache.owner_cx != raw_cx {
            cache.reset(raw_cx);
        }
        match cache.lookup(key, script, filename, line) {
            Some(stencil) => {
                cache.hits += 1;
                Some(stencil)
            }
            None => {
                cache.misses += 1;
                None
            }
        }
    });

    // Frame reference: protects the raw pointer against reentrant eviction
    // (payload JS re-entering this API can evict entries mid-frame). The
    // non-atomic refcount is fine — the cache is thread-local.
    let stencil: *mut jsapi::Stencil = match hit {
        Some(s) => {
            unsafe { jsapi::StencilAddRef(s) };
            s
        }
        None => {
            let mut realm = AutoRealm::new_from_handle(cx, glob);
            let realm_cx: &mut mozjs::context::JSContext = &mut realm;

            let options = CompileOptionsWrapper::new(realm_cx, filename.to_owned(), line);
            let mut source = transform_str_to_source_text(script);
            let addrefed = unsafe {
                wrappers2::CompileGlobalScriptToStencil(realm_cx, options.ptr, &mut source)
            };
            let raw = addrefed.mRawPtr;
            if raw.is_null() {
                // Compile error: pending exception set — same contract as
                // Evaluate2 failing. Do not pollute the cache.
                maybe_resume_unwind();
                return Err(());
            }
            unsafe { jsapi::StencilAddRef(raw) }; // frame ref (cache keeps the addrefed one)
            CACHE.with(|c| {
                let mut cache = c.borrow_mut();
                if cache.owner_cx != raw_cx {
                    cache.reset(raw_cx);
                }
                cache.insert(key, script, filename, line, raw);
            });
            raw
        }
    };

    let result = unsafe { instantiate_and_execute(cx, glob, stencil, rval) };
    // Release the frame reference; the cache keeps its own.
    unsafe { jsapi::StencilRelease(stencil) };
    result
}

/// Instantiate `stencil` into `glob`'s realm and execute it — the per-realm
/// steady-state cost identified by the #26 bench (phase C).
///
/// # Safety
/// `stencil` must be a live stencil (caller holds a reference) compiled for
/// global-scope execution on this runtime; `glob` a live realm global on
/// this runtime's current thread.
unsafe fn instantiate_and_execute(
    cx: &mut mozjs::context::JSContext,
    glob: HandleObject,
    stencil: *mut jsapi::Stencil,
    rval: MutableHandleValue,
) -> Result<(), ()> {
    // C++ defaults (js/public/CompileOptions.h `InstantiateOptions`):
    // all-false + OnDemandOnly — mirrors what a plain `JS::Evaluate`
    // compile would use (same shape as the #26 judgment bench).
    let inst_opts = jsapi::InstantiateOptions {
        skipFilenameValidation: false,
        hideScriptFromDebugger: false,
        deferDebugMetadata: false,
        eagerDelazificationStrategy_: jsapi::DelazificationOption::OnDemandOnly,
    };

    let mut realm = AutoRealm::new_from_handle(cx, glob);
    let realm_cx: &mut mozjs::context::JSContext = &mut realm;

    rooted!(&in(realm_cx) let script = unsafe {
        wrappers2::InstantiateGlobalStencil(
            realm_cx,
            &inst_opts as *const jsapi::InstantiateOptions,
            stencil,
            ptr::null_mut(), // InstantiationStorage is an optional param (JSStencil.h)
        )
    });
    if script.get().is_null() {
        maybe_resume_unwind();
        return Err(());
    }
    if !unsafe { wrappers2::JS_ExecuteScript(realm_cx, script.handle(), rval) } {
        maybe_resume_unwind();
        return Err(());
    }
    Ok(())
}

// ─── Lifecycle hooks & observability (internal surface) ───────────────────

/// Release every cached stencil on the current thread and detach ownership.
///
/// Called by [`crate::context::JsContext::shutdown_thread_sm`] BEFORE the
/// runtime is destroyed (release while the runtime is still alive is the
/// cleanest ordering), and usable by any embedder that knows a context is
/// going away. Returns the number of stencils released.
#[doc(hidden)]
pub fn clear_thread_cache() -> usize {
    CACHE.with(|c| {
        let mut cache = c.borrow_mut();
        let n = cache.entries.len();
        cache.reset(ptr::null_mut());
        n
    })
}

/// Number of stencils currently cached on this thread (0 when no context
/// owns the cache). Test/bench observability.
#[doc(hidden)]
pub fn thread_cache_len() -> usize {
    CACHE.with(|c| c.borrow().entries.len())
}

/// (hits, misses, bypasses) counters since thread start. Test/bench
/// observability — proving the cache is actually engaged.
#[doc(hidden)]
pub fn thread_cache_counters() -> (u64, u64, u64) {
    CACHE.with(|c| {
        let cache = c.borrow();
        (cache.hits, cache.misses, cache.bypasses)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::JsContext;
    use crate::value::JsValue;
    use mozjs::jsval::UndefinedValue;
    use mozjs::rooted;
    use std::ffi::CString;

    /// A deterministic ≥-floor payload with rich observable state: patched
    /// builtin (Date.now), closure-carrying global, JSON fingerprint of
    /// everything the script installed, and a last-expression value.
    const PAYLOAD: &str = r#"
(function() {
  function roundToPrecision(t) { return Math.round(t / 5) * 5; }
  var origDateNow = Date.now;
  Date.now = function() { return roundToPrecision(origDateNow()); };
  globalThis.__probe = {
    dateNowSrc: Date.now.toString(),
    hasClosure: (function() { var c = 41; return function() { return c + 1; }; })()(),
    names: Object.keys(globalThis).filter(function(n) { return n.indexOf('__probe') === 0 || n.indexOf('__extra') === 0; }).sort()
  };
})();
globalThis.__extra = 7;
__probe.names = Object.keys(globalThis).filter(function(n) { return n.indexOf('__probe') === 0 || n.indexOf('__extra') === 0; }).sort();
JSON.stringify(__probe)
"#;

    fn pad_to_floor(base: &str) -> String {
        // Padding PREPENDS filler statements so `base`'s final expression
        // stays the script's completion value (the last statement executed).
        let mut pad = String::new();
        while pad.len() + base.len() < MIN_CACHED_SOURCE_BYTES {
            pad.push_str(&format!(
                "globalThis.__pad_{} = {};\n",
                pad.len(),
                pad.len() % 97
            ));
        }
        format!("{pad}{base}")
    }

    /// Evaluate `src` in a FRESH realm of the (single, TLS) test runtime via
    /// the cached API and return the completion value as a JsValue snapshot.
    fn eval_cached_fresh_realm(src: &str, filename: &str) -> Result<JsValue, String> {
        let mut ctx = JsContext::for_test().map_err(|e| e.message)?;
        let mut cx = ctx.cx();
        let global_ptr = ctx
            .ensure_realm_global(&mut cx, None)
            .map_err(|e| e.message)?;
        rooted!(&in(cx) let global = global_ptr);
        let c_filename = CString::new(filename).unwrap();
        rooted!(&in(cx) let mut rval = UndefinedValue());
        evaluate_script_cached(&mut cx, global.handle(), src, &c_filename, 1, rval.handle_mut())
            .map_err(|_| "cached eval failed".to_string())?;
        unsafe { Ok(crate::value::jsval_to_jsvalue(cx.raw_cx_no_gc(), rval.get())) }
    }

    /// Evaluate `src` via the PLAIN path in a fresh realm (the no-cache
    /// reference for equivalence).
    fn eval_plain_fresh_realm(src: &str, filename: &str) -> Result<JsValue, String> {
        let mut ctx = JsContext::for_test().map_err(|e| e.message)?;
        // ctx.eval routes through mozjs::rust::evaluate_script (uncached).
        ctx.eval(src, filename).map_err(|e| e.message)
    }

    #[test]
    fn cached_hit_state_is_byte_equal_to_plain_eval() -> Result<(), String> {
        clear_thread_cache();
        let payload = pad_to_floor(PAYLOAD);

        let plain = match eval_plain_fresh_realm(&payload, "<stencil-eq>")? {
            JsValue::String(s) => s,
            other => panic!("plain path returned {other:?}, expected JSON string"),
        };
        // First cached call = miss (compile + store); second = hit.
        let miss = match eval_cached_fresh_realm(&payload, "<stencil-eq>")? {
            JsValue::String(s) => s,
            other => panic!("cached miss path returned {other:?}"),
        };
        let hit = match eval_cached_fresh_realm(&payload, "<stencil-eq>")? {
            JsValue::String(s) => s,
            other => panic!("cached hit path returned {other:?}"),
        };
        assert_eq!(plain, miss, "miss path must be byte-equal to plain eval");
        assert_eq!(plain, hit, "hit path must be byte-equal to plain eval");

        let (hits, misses, _) = thread_cache_counters();
        assert!(hits >= 1, "second cached call must be a hit");
        assert!(misses >= 1, "first cached call must be a miss");
        assert_eq!(thread_cache_len(), 1);
        Ok(())
    }

    #[test]
    fn cache_admission_floor_bypasses_tiny_sources() {
        clear_thread_cache();
        let v = eval_cached_fresh_realm("1+1", "<tiny>").unwrap();
        assert!(matches!(v, JsValue::Number(n) if n == 2.0));
        assert_eq!(thread_cache_len(), 0, "tiny source must not be cached");
        let (_, _, bypasses) = thread_cache_counters();
        assert!(bypasses >= 1);
    }

    #[test]
    fn capacity_lru_evicts_oldest() {
        clear_thread_cache();
        for i in 0..(MAX_ENTRIES + 2) {
            let src = pad_to_floor(&format!(
                "globalThis.__lru_marker = {i};\n{PAYLOAD}"
            ));
            let v = eval_cached_fresh_realm(&src, "<lru>").unwrap();
            assert!(matches!(v, JsValue::String(_)), "iter {i} must evaluate");
        }
        assert_eq!(
            thread_cache_len(),
            MAX_ENTRIES,
            "capacity bound must hold with LRU eviction"
        );
        // The most recent source must still hit.
        let (hits_before, _, _) = thread_cache_counters();
        let src = pad_to_floor(&format!(
            "globalThis.__lru_marker = {};\n{PAYLOAD}",
            MAX_ENTRIES + 1
        ));
        eval_cached_fresh_realm(&src, "<lru>").unwrap();
        let (hits_after, _, _) = thread_cache_counters();
        assert!(hits_after > hits_before, "newest entry must hit");
    }

    #[test]
    fn distinct_sources_never_cross_contaminate() {
        clear_thread_cache();
        let a = pad_to_floor("globalThis.__x = 'A';\nglobalThis.__ret = globalThis.__x;");
        let b = pad_to_floor("globalThis.__x = 'B';\nglobalThis.__ret = globalThis.__x;");
        for src in [&a, &b, &a, &b] {
            // same filename+line, different bytes — exact-match key must separate
            let _ = eval_cached_fresh_realm(src, "<same-file>").unwrap();
        }
        let _ = eval_cached_fresh_realm(&a, "<same-file>").unwrap();
        // Probe the last realm? Realms are distinct per call — verify via the
        // completion of a fresh eval of A with a return expression instead.
        let a_ret = pad_to_floor("globalThis.__x = 'A';\nglobalThis.__ret = globalThis.__x; globalThis.__ret");
        match eval_cached_fresh_realm(&a_ret, "<same-file>").unwrap() {
            JsValue::String(s) => assert_eq!(s, "A"),
            other => panic!("expected 'A', got {other:?}"),
        }
        assert_eq!(thread_cache_len(), 3, "a, b, a_ret are 3 distinct sources");
    }

    #[test]
    fn runtime_error_and_syntax_error_paths_match_plain_contract() {
        clear_thread_cache();
        // Error paths leave the pending exception on the cx by contract —
        // consume it between cases (same as production inject_js_hooks' Err
        // arm) so the shared test cx stays clean.
        fn clear_pending() {
            let ctx = JsContext::for_test().unwrap();
            unsafe { mozjs::jsapi::JS_ClearPendingException(ctx.cx().raw_cx()) };
        }
        // Syntax error at compile: Err, nothing cached.
        let bad = pad_to_floor("this is not valid javascript !!!");
        assert!(eval_cached_fresh_realm(&bad, "<syntax-err>").is_err());
        clear_pending();
        assert_eq!(thread_cache_len(), 0, "failed compile must not be cached");
        // Runtime throw: Err on both paths, cache still usable afterwards.
        let throwing = pad_to_floor("throw new Error('boom');");
        assert!(eval_cached_fresh_realm(&throwing, "<throw>").is_err());
        clear_pending();
        assert_eq!(thread_cache_len(), 1, "throwing source is compiled fine (cached)");
        // Second evaluation of the same throwing source hits the cache and
        // still throws.
        assert!(eval_cached_fresh_realm(&throwing, "<throw>").is_err());
        clear_pending();
        let (hits, _, _) = thread_cache_counters();
        assert!(hits >= 1, "throwing source second eval must be a hit");
        // Context still healthy.
        let v = eval_plain_fresh_realm("2+3", "<health>").unwrap();
        assert!(matches!(v, JsValue::Number(n) if n == 5.0));
    }

    #[test]
    fn filename_and_line_are_part_of_the_key() {
        clear_thread_cache();
        let src = pad_to_floor(PAYLOAD);
        let _ = eval_cached_fresh_realm(&src, "<file-a>").unwrap();
        let _ = eval_cached_fresh_realm(&src, "<file-b>").unwrap();
        assert_eq!(thread_cache_len(), 2, "same source, different filename → 2 entries");
    }

    #[test]
    fn clear_thread_cache_releases_everything() {
        clear_thread_cache();
        let src = pad_to_floor(PAYLOAD);
        let _ = eval_cached_fresh_realm(&src, "<clear>").unwrap();
        assert_eq!(thread_cache_len(), 1);
        assert_eq!(clear_thread_cache(), 1);
        assert_eq!(thread_cache_len(), 0);
        // Re-evaluate after clear: fresh miss, still correct.
        let v = eval_cached_fresh_realm(&src, "<clear>").unwrap();
        assert!(matches!(v, JsValue::String(_)));
    }
}

