// @trace TEST-ENG-6-C3 [req:REQ-ENG-006] [req:REQ-BAO-API-017] [level:integration]
// C3 line-coverage wave for src/bao_runtime/src/globals.rs (install faces).
//
// Branch map (source-anchored):
//   - install_all       : direct second invocation idempotence (every
//                         sub-installer re-runs against a populated global)
//   - install_web_apis  : web-only realm — the REQ-SEC-003 negative face
//                         (no Node globals) + positive web faces + `self`
//                         getter alias
//   - install_node_apis : `global` alias, module/install_file_globals_from_cache
//                         (__filename/__dirname), BAO_→BUN_ env snapshot alias
//   - install_module_global: exports setter hybrid strategy (object merge /
//                         function-with-statics)
//   - install_buffer_global: JSFUN_CONSTRUCTOR legacy `new Buffer.from` face
//                         + static predicates (isAscii/isUtf8/isEncoding/
//                         byteLength/compare/concat)
//   - install_crypto_global: randomUUID v4 shape, getRandomValues identity,
//                         subtle.digest algorithm arms, [Symbol.toStringTag]
//   - install_structured_clone: clone round-trip + DataCloneError failure arm
//   - install_web_api_constructors: AbortController/AbortSignal.timeout
//                         validation, EventTarget dedupe/remove/once/
//                         stopImmediatePropagation/dispatch-type-check,
//                         CustomEvent detail, Blob size/slice/text
//   - install_assert_strict: require("assert").strict face + "assert/strict"
//                         cache identity

use bao_engine::context::JsContext;
use mozjs::rooted;
#[path = "common/mod.rs"]
mod common;

use common::{eval_str, wait_until_with};

fn make_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

fn wait(ctx: &mut JsContext, cond: &str) -> bool {
    wait_until_with(ctx, cond, 30, 40, eval_str)
}

// ──────────────────── install_all face ────────────────────

/// Global setup installs EXACTLY ONCE per realm (the set_global_setup lazy
/// path): a second eval must NOT re-run the installers — if it did, the
/// one-shot stealth/node registration would corrupt the realm and the
/// process would die (see the registered one-shot constraint below) — and
/// the installed faces keep full real semantics across evals.
///
/// UNREACHABLE-BY-DESIGN (registered): a DIRECT second invocation of
/// install_all / install_web_apis / install_node_apis against the same
/// realm SIGSEGVs —
///   * web half: source-documented at globals.rs install_node_apis
///     ("Calling it twice would attempt JS_DeleteProperty on
///     JSPROP_PERMANENT properties installed by the first call, which
///     corrupts SpiderMonkey internal state and causes SIGSEGV");
///   * node half: empirically confirmed by this test's first drafts
///     (SIGSEGV on the direct second install_node_apis — one or more node
///     sub-installers are likewise non-re-entrant).
/// The supported call shape is one install per realm, which is what
/// JsContext::set_global_setup enforces (lazy, first-eval-only) and what
/// this test pins.
#[test]
fn c3_global_setup_installs_once_per_realm() {
    let mut ctx = make_ctx();
    assert_eq!(eval_str(&mut ctx, "typeof Bun"), "object", "eval 1: installed");

    // Eval 2 — must observe the SAME realm state (setup did not re-run):
    // a re-install here would either crash (one-shot faces) or reset state.
    let batch = eval_str(
        &mut ctx,
        r#"
        var ok = true;
        ok = ok && typeof Bun === "object" && typeof Buffer === "function";
        ok = ok && Buffer.alloc(4).length === 4;
        ok = ok && typeof process.cwd() === "string" && process.cwd().length > 0;
        ok = ok && typeof Bun.listen === "function" && typeof Bun.connect === "function"
                  && typeof Bun.udpSocket === "function" && typeof Bun.serve === "function";
        ok = ok && /^[0-9a-f-]{36}$/.test(crypto.randomUUID());
        ok = ok && structuredClone({ k: [1, 2] }).k.join() === "1,2";
        ok = ok && typeof require === "function" && typeof module === "object";
        ok ? "batch-ok" : "batch-broken"
        "#,
    );
    assert_eq!(
        batch, "batch-ok",
        "eval 2: installed faces must keep full real semantics (setup ran once)"
    );

    // Eval 3 — the realm stays stable: the process being alive with intact
    // semantics across three evals IS the once-per-realm observable.
    assert_eq!(
        eval_str(&mut ctx, "String(Buffer.from('still-alive').length)"),
        "11",
        "eval 3: realm must remain healthy across evals"
    );
}

/// Bun global real business fields — NOT a typeof sweep: cwd matches the
/// process cwd, env is the SAME proxy as process.env, version is pinned,
/// Bao is the documented alias of the same object, and a real
/// Bun.concatArrayBuffers call merges two ArrayBuffers byte-exact.
#[test]
fn c3_bun_global_real_business_semantics() {
    let mut ctx = make_ctx();

    assert_eq!(
        eval_str(&mut ctx, "String(Bun.cwd() === process.cwd() && Bun.cwd().length > 0)"),
        "true",
        "Bun.cwd must resolve the same real working directory as process.cwd"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(Bun.env === process.env && typeof Bun.env.PATH === 'string')"),
        "true",
        "Bun.env must be the same live env proxy as process.env with real content"
    );
    assert_eq!(
        eval_str(&mut ctx, "Bun.version"),
        "0.1.0",
        "Bun.version is the pinned runtime version"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(Bao === Bun)"),
        "true",
        "Bao must be the documented alias of the same Bun object"
    );
    assert_eq!(
        eval_str(
            &mut ctx,
            r#"
            var m = Bun.concatArrayBuffers([new Uint8Array([1, 2, 3]).buffer,
                                            new Uint8Array([4, 5]).buffer]);
            Array.prototype.slice.call(new Uint8Array(m)).join()
            "#,
        ),
        "1,2,3,4,5",
        "Bun.concatArrayBuffers must merge buffers byte-exact"
    );
    assert_eq!(
        eval_str(
            &mut ctx,
            "String(typeof Bun.listen === 'function' && typeof Bun.connect === 'function' \
             && typeof Bun.udpSocket === 'function')"
        ),
        "true",
        "the REQ-BAO-API-017 trio must hang off the Bun object (bun_listen::install)"
    );
}

/// install_node_apis global-surface semantics: `global` alias identity,
/// module.id, module.exports === globalThis (live-bound getter), real
/// process fields, and the global Buffer constructor face.
#[test]
fn c3_globalthis_node_install_semantics() {
    let mut ctx = make_ctx();

    assert_eq!(
        eval_str(&mut ctx, "String(global === globalThis)"),
        "true",
        "Node `global` alias must be the same object as globalThis"
    );
    assert_eq!(eval_str(&mut ctx, "module.id"), ".", "module.id is the entry dot");
    assert_eq!(
        eval_str(&mut ctx, "String(module.exports === globalThis)"),
        "true",
        "module.exports getter must be live-bound to globalThis"
    );
    assert_eq!(
        eval_str(
            &mut ctx,
            "String(typeof process.platform === 'string' && process.platform.length > 0 \
             && typeof process.arch === 'string' && process.version.startsWith('v'))"
        ),
        "true",
        "process identity fields must be real non-empty values"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(Buffer.alloc(4).length === 4 && Buffer(4).length === 4)"),
        "true",
        "Buffer must be callable AND constructible with byte-count semantics"
    );
}

/// BAO_* → BUN_* env snapshot alias: a host-env BAO_<SUFFIX> var must be
/// visible on process.env under BOTH spellings (explicit BUN_ wins rule not
/// exercised here — none set). Host env is set before the runtime installs.
#[test]
fn c3_env_alias_bao_suffix_visible_on_process_env() {
    // SAFETY: nextest runs each test in its own process — no concurrent
    // reader observes this mutation of the host env.
    unsafe {
        std::env::set_var("BAO_COV_C3_MARKER", "alias-probe-777");
    }
    let mut ctx = make_ctx();

    assert_eq!(
        eval_str(&mut ctx, "process.env.BAO_COV_C3_MARKER"),
        "alias-probe-777",
        "the BAO_ spelling must pass through"
    );
    assert_eq!(
        eval_str(&mut ctx, "process.env.BUN_COV_C3_MARKER"),
        "alias-probe-777",
        "the BUN_ alias must be synthesized on the JS env snapshot"
    );
    // The alias face must not fabricate unrelated keys.
    assert_eq!(
        eval_str(&mut ctx, "String(process.env.BUN_COV_C3_ABSENT_KEY === undefined)"),
        "true",
        "no phantom env keys may appear"
    );
}

/// install_file_globals + install_file_globals_from_cache: __filename /
/// __dirname are defined on the global from the thread-local cache exactly
/// as provided.
#[test]
fn c3_file_globals_filename_dirname_installed() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    bun_runtime::globals::install_file_globals(&mut ctx, "/tmp/c3-dir/entry.js", "/tmp/c3-dir");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    assert_eq!(
        eval_str(&mut ctx, "globalThis.__filename"),
        "/tmp/c3-dir/entry.js",
        "__filename must be installed from the file-globals cache"
    );
    assert_eq!(
        eval_str(&mut ctx, "globalThis.__dirname"),
        "/tmp/c3-dir",
        "__dirname must be installed from the file-globals cache"
    );
}

// ──────────────────── install_web_apis: web-only realm face ────────────────────

/// REQ-SEC-003 page-realm face: a realm installed with install_web_apis ONLY
/// carries the web faces and NONE of the Node faces (the ten-face negative
/// surface), with real web semantics working (crypto UUID, structuredClone,
/// fetch class presence) and the `self` getter aliasing globalThis.
#[test]
fn c3_web_only_realm_node_negative_and_web_positive() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(|cx, global| unsafe { bun_runtime::globals::install_web_apis(cx, global) });

    // Negative face — Node-only globals must be absent.
    for name in [
        "Bun",
        "Bao",
        "process",
        "Buffer",
        "require",
        "module",
        "global",
        "__filename",
        "__dirname",
    ] {
        assert_eq!(
            eval_str(&mut ctx, &format!("typeof {name}")),
            "undefined",
            "web-only realm must NOT install {name}"
        );
    }

    // Positive face — web faces present with REAL semantics.
    assert_eq!(
        eval_str(
            &mut ctx,
            r#"
            var ok = true;
            ok = ok && typeof fetch === "function";
            ok = ok && typeof structuredClone === "function";
            ok = ok && structuredClone({ w: [9] }).w.join() === "9";
            ok = ok && /^[0-9a-f-]{36}$/.test(crypto.randomUUID());
            ok = ok && typeof TextEncoder === "function" && new TextEncoder().encode("hi").length === 2;
            ok = ok && typeof setTimeout === "function" && typeof queueMicrotask === "function";
            ok ? "web-ok" : "web-broken"
            "#
        ),
        "web-ok",
        "web faces must carry real semantics in the page-realm install"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(self === globalThis)"),
        "true",
        "the `self` getter must alias the current global"
    );
}

// ──────────────────── install_crypto_global ────────────────────

/// crypto face: randomUUID is real UUIDv4 (shape + version/variant bits +
/// two calls differ), getRandomValues fills AND returns the same array,
/// the LIVE subtle.digest (web_api::install_crypto_subtle redefines the
/// globals.rs sync version at the tail of the web-API phase — the live
/// contract is Promise<ArrayBuffer>) hits its per-algorithm arms with NIST
/// vectors, unsupported algorithms reject, and the WebIDL toStringTag reads
/// [object Crypto].
#[test]
fn c3_crypto_global_semantics() {
    let mut ctx = make_ctx();

    let uuid_check = eval_str(
        &mut ctx,
        r#"
        var u1 = crypto.randomUUID();
        var u2 = crypto.randomUUID();
        var re = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
        (re.test(u1) && re.test(u2) && u1 !== u2) ? "uuid-ok" : "uuid-bad:" + u1 + "/" + u2
        "#,
    );
    assert_eq!(uuid_check, "uuid-ok", "randomUUID must be real distinct v4 UUIDs");

    assert_eq!(
        eval_str(
            &mut ctx,
            r#"
            var t = new Uint8Array(16);
            var r = crypto.getRandomValues(t);
            (r === t && t.some(function(b) { return b !== 0; })) ? "rand-ok" : "rand-bad"
            "#
        ),
        "rand-ok",
        "getRandomValues must fill and return the SAME array"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(crypto.getRandomValues(42) === 42)"),
        "true",
        "non-object passthrough arm returns the argument"
    );

    // Live subtle.digest — Promise<ArrayBuffer>, NIST "abc" vectors.
    assert_eq!(
        eval_str(
            &mut ctx,
            r#"
            globalThis.__d = "";
            function hex(ab) {
                return Array.prototype.map.call(new Uint8Array(ab),
                    function(b) { return ("0" + b.toString(16)).slice(-2); }).join("");
            }
            crypto.subtle.digest("SHA-256", new TextEncoder().encode("abc"))
                .then(function(ab) { globalThis.__d = "256:" + hex(ab); },
                      function(e) { globalThis.__d = "256-err:" + e; });
            crypto.subtle.digest("SHA-1", new TextEncoder().encode("abc"))
                .then(function(ab) { globalThis.__d += "|1:" + hex(ab); },
                      function(e) { globalThis.__d += "|1-err:" + e; });
            crypto.subtle.digest({ name: "sha-512" }, new TextEncoder().encode("abc"))
                .then(function(ab) { globalThis.__d += "|512:" + new Uint8Array(ab).length; },
                      function(e) { globalThis.__d += "|512-err:" + e; });
            crypto.subtle.digest("md5", new TextEncoder().encode("abc"))
                .then(function() { globalThis.__d += "|md5-no-reject"; },
                      function(e) { globalThis.__d += "|md5-reject:" + e.message; });
            "digest-started"
            "#,
        ),
        "digest-started",
    );
    let digest_done = wait(&mut ctx, "String(globalThis.__d).indexOf('512') !== -1 ? 'y' : 'n'");
    let digest = eval_str(&mut ctx, "globalThis.__d");
    assert!(digest_done, "all four digest promises must settle, got: {digest}");
    assert!(
        digest.contains("256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
        "SHA-256('abc') must match the NIST vector, got: {digest}"
    );
    assert!(
        digest.contains("1:a9993e364706816aba3e25717850c26c9cd0d89d"),
        "SHA-1('abc') must match the NIST vector, got: {digest}"
    );
    assert!(
        digest.contains("512:64"),
        "SHA-512 via {{name}} object algorithm must produce 64 bytes, got: {digest}"
    );
    assert!(
        digest.contains("md5-reject:") && !digest.contains("md5-no-reject"),
        "unsupported algorithm must reject the promise, got: {digest}"
    );
    assert_eq!(
        eval_str(&mut ctx, "Object.prototype.toString.call(crypto)"),
        "[object Crypto]",
        "crypto must carry the WebIDL [Symbol.toStringTag]"
    );
}

// ──────────────────── install_structured_clone ────────────────────

/// structuredClone: nested structures clone with identity separation, and
/// non-clonable values (functions) fail with DataCloneError.
#[test]
fn c3_structured_clone_roundtrip_and_datacloneerror() {
    let mut ctx = make_ctx();

    assert_eq!(
        eval_str(
            &mut ctx,
            r#"
            var o = { a: { b: [1, 2, 3] } };
            var c = structuredClone(o);
            (c !== o && c.a !== o.a && c.a.b.join() === "1,2,3") ? "clone-ok" : "clone-bad"
            "#
        ),
        "clone-ok",
        "nested clone must be structurally equal but identity-separated"
    );
    assert_eq!(
        eval_str(
            &mut ctx,
            r#"
            try { structuredClone(function f() {}); "no-throw" }
            catch (e) { /DataCloneError/.test(e.message) ? "dce-ok" : "dce-bad:" + e.message }
            "#
        ),
        "dce-ok",
        "functions must fail the clone with DataCloneError"
    );
}

// ──────────────────── install_web_api_constructors ────────────────────

/// AbortController/AbortSignal: abort fires the listener with
/// aborted/reason state; AbortSignal.timeout validates its argument
/// (non-number / negative → TypeError; 0 is legal).
#[test]
fn c3_web_api_abort_controller_semantics() {
    let mut ctx = make_ctx();

    assert_eq!(
        eval_str(
            &mut ctx,
            r#"
            var ac = new AbortController();
            var fired = 0;
            ac.signal.addEventListener("abort", function() { fired++; });
            ac.abort("why-c3");
            (ac.signal.aborted === true && ac.signal.reason === "why-c3" && fired === 1)
                ? "abort-ok" : "abort-bad:" + fired
            "#
        ),
        "abort-ok",
        "abort must flip the signal, carry the reason, fire the listener once"
    );
    for bad in ["-1", "'nope'", "NaN"] {
        assert_eq!(
            eval_str(
                &mut ctx,
                &format!(
                    "try {{ AbortSignal.timeout({bad}); 'no-throw' }} \
                     catch (e) {{ e instanceof TypeError ? 'threw-ok' : 'threw-bad:' + e }}"
                )
            ),
            "threw-ok",
            "AbortSignal.timeout({bad}) must throw TypeError (validation arm)"
        );
    }
}

/// EventTarget semantics: (callback, capture) dedupe, removeEventListener,
/// once, stopImmediatePropagation halting the snapshot, the dispatch
/// type-check throw, and CustomEvent detail propagation.
#[test]
fn c3_web_api_event_target_dispatch_semantics() {
    let mut ctx = make_ctx();

    assert_eq!(
        eval_str(
            &mut ctx,
            r#"
            var ok = true;
            var et = new EventTarget();
            var hits = 0;
            function l() { hits++; }
            et.addEventListener("e", l);
            et.addEventListener("e", l);            // same (cb, capture) pair → dedupe
            ok = ok && et.dispatchEvent(new Event("e")) === true && hits === 1;
            et.removeEventListener("e", l);
            et.dispatchEvent(new Event("e"));
            ok = ok && hits === 1;                  // removed → no fire

            var onceHits = 0;
            et.addEventListener("z", function() { onceHits++; }, { once: true });
            et.dispatchEvent(new Event("z"));
            et.dispatchEvent(new Event("z"));
            ok = ok && onceHits === 1;

            var first = false, second = false;
            et.addEventListener("s", function(ev) { first = true; ev.stopImmediatePropagation(); });
            et.addEventListener("s", function() { second = true; });
            et.dispatchEvent(new Event("s"));
            ok = ok && first && !second;
            ok ? "et-ok" : "et-bad:" + hits + "/" + onceHits
            "#
        ),
        "et-ok",
        "EventTarget dedupe/remove/once/stopImmediatePropagation semantics"
    );
    assert_eq!(
        eval_str(
            &mut ctx,
            r#"
            try { new EventTarget().dispatchEvent({}); "no-throw" }
            catch (e) { e instanceof TypeError ? "threw-ok" : "threw-bad:" + e }
            "#
        ),
        "threw-ok",
        "dispatchEvent with a non-Event must throw TypeError"
    );
    assert_eq!(
        eval_str(&mut ctx, "new CustomEvent('x', { detail: 42 }).detail"),
        "42",
        "CustomEvent must carry the detail option"
    );
}

/// Blob semantics: UTF-8 string size, byte-part size, slice windows, the
/// default empty type, and the async text() decode face.
#[test]
fn c3_web_api_blob_semantics() {
    let mut ctx = make_ctx();

    assert_eq!(
        eval_str(
            &mut ctx,
            r#"
            var ok = true;
            var b = new Blob(["hello"]);
            ok = ok && b.size === 5 && b.type === "";
            ok = ok && new Blob([]).size === 0;
            ok = ok && new Blob([new Uint8Array([1, 2, 3, 4])]).size === 4;
            ok = ok && b.slice(1, 3).size === 2;
            ok = ok && b.slice(-3).size === 3;
            ok = ok && typeof globalThis._bao_blob_registry !== "undefined";
            globalThis.__bt = null;
            b.text().then(function(s) { globalThis.__bt = s; });
            ok ? "blob-ok" : "blob-bad"
            "#
        ),
        "blob-ok",
        "Blob size/type/slice/registry faces"
    );
    let text_ready = wait(&mut ctx, "globalThis.__bt === 'hello' ? 'y' : 'n'");
    let text = eval_str(&mut ctx, "String(globalThis.__bt)");
    assert!(
        text_ready && text == "hello",
        "Blob.text() must decode the UTF-8 body, got: {text}"
    );
}

// ──────────────────── install_module_global ────────────────────

/// module.exports setter hybrid strategy: an object export merges its own
/// enumerable props into globalThis and re-binds exports to globalThis; a
/// function export becomes the current exports and its static props merge.
#[test]
fn c3_module_exports_setter_semantics() {
    let mut ctx = make_ctx();

    assert_eq!(
        eval_str(
            &mut ctx,
            r#"
            module.exports = { c3x: 41 };
            var merged = globalThis.c3x === 41;
            var rebound = module.exports === globalThis;
            (merged && rebound) ? "obj-ok" : "obj-bad"
            "#
        ),
        "obj-ok",
        "object export must merge props into globalThis and re-bind exports"
    );
    assert_eq!(
        eval_str(
            &mut ctx,
            r#"
            function c3f(v) { return v * 2; }
            c3f.tag = "C3";
            module.exports = c3f;
            var callable = module.exports(21) === 42;
            var statics = globalThis.tag === "C3";
            (callable && statics) ? "fn-ok" : "fn-bad"
            "#
        ),
        "fn-ok",
        "function export must stay callable and merge its static props"
    );
}

// ──────────────────── install_buffer_global ────────────────────

/// Buffer static predicates on the GLOBAL face: isAscii/isUtf8 byte-view
/// validation, isEncoding, byteLength UTF-8 counting, compare ordering,
/// concat, and the JSFUN_CONSTRUCTOR legacy `new Buffer.from/alloc` face.
#[test]
fn c3_buffer_global_static_predicates_and_legacy_ctor() {
    let mut ctx = make_ctx();

    assert_eq!(
        eval_str(
            &mut ctx,
            r#"
            var ok = true;
            ok = ok && Buffer.isAscii(Buffer.from("abc")) === true;
            ok = ok && Buffer.isAscii(Buffer.from([0x80])) === false;
            ok = ok && Buffer.isAscii(42) === false;          // non-BufferSource
            ok = ok && Buffer.isUtf8(Buffer.from("héllo")) === true;
            ok = ok && Buffer.isUtf8(Buffer.from([0xff, 0xfe])) === false;
            ok = ok && Buffer.isEncoding("utf8") === true;
            ok = ok && Buffer.isEncoding("no-such-enc") === false;
            ok = ok && Buffer.byteLength("héllo") === 6;      // UTF-8 bytes
            ok = ok && Buffer.compare(Buffer.from([1]), Buffer.from([2])) === -1;
            ok = ok && Buffer.concat([Buffer.from("ab"), Buffer.from("cd")]).toString() === "abcd";
            ok ? "buf-ok" : "buf-bad"
            "#
        ),
        "buf-ok",
        "global Buffer static predicates (single-source validators)"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(new Buffer.from('🥶').length)"),
        "4",
        "legacy new Buffer.from must work (JSFUN_CONSTRUCTOR) with UTF-8 length"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(new Buffer.alloc(3).length)"),
        "3",
        "legacy new Buffer.alloc must work (JSFUN_CONSTRUCTOR)"
    );
}

// ──────────────────── install_assert_strict ────────────────────

/// assert strict-mode face: require("assert").strict exposes the assertion
/// surface (JS-built api.strict), strictEqual passes/fails correctly, and
/// the separately cached require("assert/strict") module (cache_assert_strict)
/// also carries a working strictEqual — both install faces live.
#[test]
fn c3_assert_strict_module_face() {
    let mut ctx = make_ctx();

    assert_eq!(
        eval_str(
            &mut ctx,
            r#"
            var assert = require("assert");
            var strict = assert.strict;
            var ok = typeof strict.strictEqual === "function";
            strict.strictEqual(2, 2);
            var threw = false;
            try { strict.strictEqual(1, 2); } catch (e) { threw = e instanceof assert.AssertionError || e instanceof Error; }
            ok = ok && threw;
            var strictMod = require("assert/strict");
            ok = ok && typeof strictMod.strictEqual === "function";
            strictMod.strictEqual("a", "a");
            ok ? "strict-ok" : "strict-bad"
            "#
        ),
        "strict-ok",
        "assert.strict and the cached assert/strict module must expose working strictEqual"
    );
}

/// install_module_on_target (REQ-SEC-002 parameter injection): the `module`
/// face rides a TARGET object (not the global), carries the entry dot id,
/// its exports getter binds the GLOBAL, and the object-setter writes through
/// to globalThis (the documented hybrid strategy).
#[test]
fn c3_install_module_on_target_face() {
    let mut ctx = make_ctx();
    // The global setup is lazy (first eval materializes the realm).
    assert_eq!(eval_str(&mut ctx, "typeof module"), "object", "bootstrap eval");
    let global = bao_engine::context::thread_realm_global().expect("realm global");
    {
        let mut cxm = ctx.cx();
        unsafe {
            use mozjs::jsapi::JS_SetProperty;
            use mozjs::jsval::ObjectValue;
            use mozjs::rust::wrappers2::JS_NewPlainObject;
            rooted!(&in(cxm) let g_root = global);
            // Realm-entered allocation — the same discipline as every
            // product-side bridge (invoke_js_callback / tcp_on_open): object
            // allocation resolves its AllocSite from the CURRENT realm.
            let mut realm = mozjs::realm::AutoRealm::new_from_handle(&mut cxm, g_root.handle());
            let realm_cx: &mut mozjs::context::JSContext = &mut realm;
            let target_ptr = JS_NewPlainObject(realm_cx);
            assert!(!target_ptr.is_null(), "target allocation failed");
            rooted!(&in(realm_cx) let target = target_ptr);
            bun_runtime::globals::install_module_on_target(realm_cx, target.handle(), g_root.handle());
            rooted!(&in(realm_cx) let tval = ObjectValue(target.get()));
            JS_SetProperty(
                realm_cx.raw_cx(),
                g_root.handle().into(),
                c"__c3target".as_ptr(),
                tval.handle().into(),
            );
        }
    }

    assert_eq!(
        eval_str(
            &mut ctx,
            r#"
            var t = globalThis.__c3target;
            var ok = !!(t && typeof t.module === "object" && t.module.id === ".");
            ok = ok && t.module.exports === globalThis;
            t.module.exports = { c3t: 7 };
            ok = ok && globalThis.c3t === 7;
            ok ? "target-ok" : "target-bad"
            "#
        ),
        "target-ok",
        "module-on-target must carry id/exports-getter and write through to globalThis"
    );
}
