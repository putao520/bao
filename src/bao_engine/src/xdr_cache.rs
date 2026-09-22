// @trace REQ-ENG-012 [entity:BaoStencilXdrCache]
//! Persistent Stencil XDR cache — the disk front layer of
//! [`crate::stencil_cache`] (REQ-ENG-012, SM-EVOLUTION #26). Compiled
//! stencils are `JS::EncodeStencil`-encoded to a content-addressed file and
//! reloaded with `JS::DecodeStencil` after a process restart, removing the
//! repeated parse+compile of unchanged payloads (the #26 bench: 79.6% of warm
//! injection cost is compile; instantiate-vs-recompile 4.9×).
//!
//! ## Addressing (SPEC REQ-ENG-012: "按源 hash + 编译选项寻址")
//!
//! Key = wyhash(source bytes, filename bytes, line NE) — the exact key of the
//! in-memory layer, which in this surface is also the *complete* compile-option
//! surface (`stencil_cache` documents that `CompileOptionsWrapper::new`
//! defaults are the only option degrees of freedom; filename/line are baked
//! into the stencil's ScriptSource). Same admission policy as the memory
//! layer: sources below its floor bypass this layer entirely. A hash collision
//! cannot serve a wrong script: the entry stores the full key record
//! (filename/line/source bytes) and [`load`] verifies them exactly before any
//! decode — a mismatch is a miss, never a wrong hit.
//!
//! ## Entry format (v1, little-endian)
//!
//! ```text
//! "BAOXDR1\n"                  magic + format version
//! u32 len + bytes              process transcoding build id at write time
//! u32 len + bytes              filename (exact key record)
//! u32                          line
//! u64 len + bytes              source (exact key record)
//! u64 len + bytes              XDR payload (JS::EncodeStencil output)
//! ```
//!
//! ## Validation chain (C3 — fail-closed, observable)
//!
//! 1. Process build id (via `JS::GetScriptTranscodingBuildId`) must exist —
//!    otherwise the whole layer is disabled (SM's XDR version check would
//!    dereference a NULL BuildIdOp; see stage1 findings).
//! 2. Magic / header bounds / key-record equality — mismatch = miss.
//! 3. `JS::DecodeStencil` itself re-validates the XDR-embedded build id and
//!    buffer integrity (`Failure_BadBuildId` / `Failure_BadDecode`) — SM is
//!    the decode authority; a truncated or tampered payload decodes to a
//!    miss, never to a wrong script. Every miss falls back to the plain
//!    compile path and the fresh compile re-stores the entry (self-healing).
//!
//! ## Atomicity (C4)
//!
//! Entries are written to `<key>.tmp.<pid>` in the cache directory and
//! `rename`d into place — POSIX rename is atomic, so an interrupted write can
//! only ever leave an unreferenced temp file, never a half-written entry that
//! `load` would consider (load opens the exact `.xdr` path only).
//!
//! ## Process build id op
//!
//! XDR encode AND decode both invoke the process `BuildIdOp` through SM's
//! version check, which NULL-derefs when no op is installed. This module
//! installs a fixed-tag op lazily on first use. Servo installs its own
//! (`servo_build_id`, wasm identity) in browser processes and there is no
//! public getter to probe it — if both run in one process the op flips at
//! most once, invalidating at most one write-generation of entries, which
//! the C3 path then re-stores under the current id. Correctness is never
//! affected; the id is a staleness tag, not an identity.
//!
//! Internal experimental surface (SM-EVOLUTION), not a stable API commitment
//! — hence `#[doc(hidden)]` on the observability/seam helpers.

use std::ffi::CStr;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};
use std::time::Instant;

use mozjs::jsapi;
use mozjs::rust::wrappers2;

use crate::stencil_cache::hash_key;

/// Directory seam: production always uses the internal default (no user
/// visible configuration surface); tests isolate via
/// [`set_cache_dir_for_tests`].
static CACHE_DIR: RwLock<Option<PathBuf>> = RwLock::new(None);

fn cache_dir() -> PathBuf {
    CACHE_DIR
        .read()
        .expect("cache dir lock poisoned")
        .clone()
        .unwrap_or_else(|| std::env::temp_dir().join("bao-stencil-xdr-v1"))
}

/// Override the cache directory (test seam — production uses the internal
/// default).
#[doc(hidden)]
pub fn set_cache_dir_for_tests(dir: PathBuf) {
    *CACHE_DIR.write().expect("cache dir lock poisoned") = Some(dir);
}

/// Content-addressed entry path for (source, filename, line).
fn cache_file(source: &str, filename: &CStr, line: u32) -> PathBuf {
    let key = hash_key(source, filename, line);
    cache_dir().join(format!("{key:016x}.xdr"))
}

// ─── Process build id (XDR version tag) ─────────────────────────────────────

static BUILD_ID: OnceLock<Option<Vec<u8>>> = OnceLock::new();

/// Fixed process tag — mixed by SM with endianness/pointer-size into the XDR
/// version check, so it only needs to be stable per Bao build.
const BUILD_ID_TAG: &[u8] = b"bao-stencil-xdr-1";

/// # Safety
/// SM callback: `build_id` must be a live `JS::BuildIdCharVector`.
unsafe extern "C" fn bao_process_build_id(build_id: *mut jsapi::BuildIdCharVector) -> bool {
    unsafe {
        wrappers2::SetBuildId(
            build_id,
            BUILD_ID_TAG.as_ptr() as *const _,
            BUILD_ID_TAG.len(),
        )
    }
}

/// Process transcoding build id; installs the process BuildIdOp on first use
/// (required before ANY XDR encode/decode — without it SM NULL-derefs).
/// `None` = SM declined: the persistent layer stays disabled (fail-closed).
fn process_build_id() -> Option<&'static [u8]> {
    BUILD_ID
        .get_or_init(|| unsafe {
            unsafe {
                jsapi::SetProcessBuildIdOp(Some(bao_process_build_id));
            }
            let vector = unsafe { wrappers2::CreateBuildIdCharVector() };
            if vector.is_null() {
                return None;
            }
            let ok = unsafe { wrappers2::GetScriptTranscodingBuildId(vector) };
            let begin = unsafe { wrappers2::BuildIdCharVectorBegin(vector) };
            let len = unsafe { wrappers2::BuildIdCharVectorLength(vector) };
            let id = if ok && !begin.is_null() {
                Some(unsafe { std::slice::from_raw_parts(begin, len) }.to_vec())
            } else {
                None
            };
            unsafe { wrappers2::DestroyBuildIdCharVector(vector) };
            id
        })
        .as_deref()
}

// ─── Counters (observability — C2/C3 evidence surface) ──────────────────────

/// Cache outcome counters (process-lifetime). Test/observability surface.
#[doc(hidden)]
#[derive(Default)]
pub struct XdrCounters {
    pub hits: u64,
    pub miss_absent: u64,
    pub miss_corrupt: u64,
    pub miss_stale: u64,
    pub miss_decode_failed: u64,
    pub disabled: u64,
    pub stores: u64,
    pub store_failed: u64,
    /// Cumulative time spent inside `JS::DecodeStencil` (µs).
    pub decode_us: u64,
}

macro_rules! counters {
    ($($field:ident => $static_name:ident),+ $(,)?) => {
        $(static $static_name: AtomicU64 = AtomicU64::new(0);)+
        impl XdrCounters {
            fn snapshot() -> Self {
                Self { $($field: $static_name.load(Ordering::Relaxed),)+ }
            }
        }
    };
}

counters!(
    hits => HITS,
    miss_absent => MISS_ABSENT,
    miss_corrupt => MISS_CORRUPT,
    miss_stale => MISS_STALE,
    miss_decode_failed => MISS_DECODE_FAILED,
    disabled => DISABLED,
    stores => STORES,
    store_failed => STORE_FAILED,
    decode_us => DECODE_US,
);

/// (hits, miss_absent, miss_corrupt, miss_stale, miss_decode_failed, disabled,
/// stores, store_failed, decode_us) since process start. Test/observability.
#[doc(hidden)]
pub fn xdr_counters() -> XdrCounters {
    XdrCounters::snapshot()
}

// ─── Entry format ───────────────────────────────────────────────────────────

const MAGIC: &[u8; 8] = b"BAOXDR1\n";

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Reader { bytes, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        if end > self.bytes.len() {
            return None;
        }
        let out = &self.bytes[self.pos..end];
        self.pos = end;
        Some(out)
    }
    fn take_u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn take_u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put_bytes(out: &mut Vec<u8>, v: &[u8]) {
    put_u64(out, v.len() as u64);
    out.extend_from_slice(v);
}

fn put_bytes32(out: &mut Vec<u8>, v: &[u8]) {
    put_u32(out, v.len() as u32);
    out.extend_from_slice(v);
}

/// Parse and fully verify an entry against the exact key record + current
/// build id. Returns the XDR payload, or the miss reason (C3 observability).
fn parse_entry<'a>(
    bytes: &'a [u8],
    build_id: &[u8],
    source: &str,
    filename: &CStr,
    line: u32,
) -> Result<&'a [u8], &'static str> {
    let mut r = Reader::new(bytes);
    if r.take(MAGIC.len()) != Some(&MAGIC[..]) {
        return Err("corrupt");
    }
    let id_len = r.take_u32().ok_or("corrupt")? as usize;
    let stored_id = r.take(id_len).ok_or("corrupt")?;
    if stored_id != build_id {
        return Err("stale");
    }
    let file_len = r.take_u32().ok_or("corrupt")? as usize;
    let stored_file = r.take(file_len).ok_or("corrupt")?;
    if stored_file != filename.to_bytes() {
        return Err("corrupt");
    }
    if r.take_u32().ok_or("corrupt")? != line {
        return Err("corrupt");
    }
    let src_len = r.take_u64().ok_or("corrupt")? as usize;
    let stored_src = r.take(src_len).ok_or("corrupt")?;
    if stored_src != source.as_bytes() {
        return Err("corrupt");
    }
    let xdr_len = r.take_u64().ok_or("corrupt")? as usize;
    let xdr = r.take(xdr_len).ok_or("corrupt")?;
    if r.pos != bytes.len() {
        return Err("corrupt");
    }
    Ok(xdr)
}

/// `TranscodeRange` view over raw XDR bytes (release-layout `mozilla::Range`:
/// two raw pointers; decode only reads them).
fn transcode_range(bytes: &[u8]) -> jsapi::TranscodeRange {
    let base = bytes.as_ptr() as *mut u8;
    jsapi::TranscodeRange {
        _phantom_0: PhantomData,
        mStart: jsapi::mozilla::RangedPtr {
            _phantom_0: PhantomData,
            mPtr: base,
        },
        mEnd: jsapi::mozilla::RangedPtr {
            _phantom_0: PhantomData,
            mPtr: base.wrapping_add(bytes.len()),
        },
    }
}

/// C++-default `ReadOnlyDecodeOptions` (null introducer, no borrow).
fn decode_options() -> jsapi::ReadOnlyDecodeOptions {
    jsapi::ReadOnlyDecodeOptions {
        borrowBuffer: false,
        usePinnedBytecode: false,
        introducerFilename_: jsapi::ConstUTF8CharsZ { data_: ptr::null() },
        introductionType: ptr::null(),
        introductionLineno: 0,
        introductionOffset: 0,
    }
}

// ─── Public API ─────────────────────────────────────────────────────────────

/// Load the persisted stencil for (source, filename, line), if a valid entry
/// exists for the current process build id. Any absence/corruption/staleness
/// is a counted miss — the caller falls back to the plain compile path
/// (REQ-ENG-012 C3). The returned stencil carries one reference (the caller
/// owns it).
///
/// # Safety
/// `cx` must be the raw JSContext of the calling thread's live runtime (same
/// discipline as the in-memory cache: stencils are runtime-scoped, never
/// `Send`).
pub unsafe fn load(
    cx: *mut jsapi::JSContext,
    source: &str,
    filename: &CStr,
    line: u32,
) -> Option<*mut jsapi::Stencil> {
    let Some(build_id) = process_build_id() else {
        DISABLED.fetch_add(1, Ordering::Relaxed);
        return None;
    };
    let Ok(bytes) = std::fs::read(cache_file(source, filename, line)) else {
        MISS_ABSENT.fetch_add(1, Ordering::Relaxed);
        return None;
    };
    let xdr = match parse_entry(&bytes, build_id, source, filename, line) {
        Ok(xdr) => xdr,
        Err("stale") => {
            MISS_STALE.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        Err(_) => {
            MISS_CORRUPT.fetch_add(1, Ordering::Relaxed);
            return None;
        }
    };

    let opts = decode_options();
    let range = transcode_range(xdr);
    let mut decoded: *mut jsapi::Stencil = ptr::null_mut();
    let t0 = Instant::now();
    let result = unsafe { jsapi::DecodeStencil(cx, &opts, &range, &mut decoded) };
    DECODE_US.fetch_add(t0.elapsed().as_micros() as u64, Ordering::Relaxed);
    if result != jsapi::TranscodeResult::Ok || decoded.is_null() {
        MISS_DECODE_FAILED.fetch_add(1, Ordering::Relaxed);
        return None;
    }
    HITS.fetch_add(1, Ordering::Relaxed);
    Some(decoded)
}

/// Persist the compiled stencil for (source, filename, line) — best effort:
/// every failure (not cacheable, encode error, I/O) is counted and skipped,
/// never fatal to evaluation. Atomic rename (REQ-ENG-012 C4).
///
/// # Safety
/// `stencil` must be a live stencil on `cx`'s runtime (caller holds a
/// reference); `cx` the calling thread's raw JSContext.
pub unsafe fn store(
    cx: *mut jsapi::JSContext,
    stencil: *mut jsapi::Stencil,
    source: &str,
    filename: &CStr,
    line: u32,
) {
    let Some(build_id) = process_build_id() else {
        DISABLED.fetch_add(1, Ordering::Relaxed);
        return;
    };
    if !unsafe { jsapi::IsStencilCacheable(stencil) } {
        // asm.js and other non-cacheable stencils: encode would fail anyway.
        return;
    }

    let buffer = unsafe { wrappers2::CreateTranscodeBuffer() };
    if buffer.is_null() {
        STORE_FAILED.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let result = unsafe { jsapi::EncodeStencil(cx, stencil, buffer) };
    let (begin, len) = unsafe {
        (
            wrappers2::TranscodeBufferBegin(buffer),
            wrappers2::TranscodeBufferLength(buffer),
        )
    };
    let xdr = if result == jsapi::TranscodeResult::Ok && !begin.is_null() && len > 0 {
        Some(unsafe { std::slice::from_raw_parts(begin, len) }.to_vec())
    } else {
        None
    };
    unsafe { wrappers2::DestroyTranscodeBuffer(buffer) };
    let Some(xdr) = xdr else {
        STORE_FAILED.fetch_add(1, Ordering::Relaxed);
        return;
    };

    let mut entry = Vec::with_capacity(MAGIC.len() + 8 + build_id.len() + filename.to_bytes().len() + source.len() + xdr.len());
    entry.extend_from_slice(MAGIC);
    put_bytes32(&mut entry, build_id);
    put_bytes32(&mut entry, filename.to_bytes());
    put_u32(&mut entry, line);
    put_bytes(&mut entry, source.as_bytes());
    put_bytes(&mut entry, &xdr);

    let path = cache_file(source, filename, line);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // C4: tmp + rename — an interrupted write leaves only an unreferenced
    // temp file that load can never open (it reads the exact `.xdr` path).
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    let written = std::fs::write(&tmp, &entry).and_then(|()| std::fs::rename(&tmp, &path));
    match written {
        Ok(()) => STORES.fetch_add(1, Ordering::Relaxed),
        Err(_) => {
            let _ = std::fs::remove_file(&tmp);
            STORE_FAILED.fetch_add(1, Ordering::Relaxed)
        }
    };
}

// ─── Tests (REQ-ENG-012 C1/C2/C3/C4) ────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::JsContext;
    use crate::stencil_cache::{clear_thread_cache, evaluate_script_cached, thread_cache_len};
    use crate::value::{jsval_to_jsvalue, JsValue};
    use mozjs::jsval::UndefinedValue;
    use mozjs::rooted;
    use std::ffi::CString;
    use std::sync::atomic::AtomicU32;

    static TEST_DIR_SEQ: AtomicU32 = AtomicU32::new(0);

    /// Isolate each test in its own cache directory (no cross-test or
    /// cross-process contamination of the production default).
    fn fresh_test_dir(tag: &str) -> PathBuf {
        let n = TEST_DIR_SEQ.fetch_add(1, Ordering::Relaxed);
        // Windows path hygiene: libtest thread names carry `::` separators
        // (code 123 InvalidFilename) — keep [A-Za-z0-9_-] only.
        let thread: String = std::thread::current()
            .name()
            .unwrap_or("t")
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
            .collect();
        let dir = std::env::temp_dir().join(format!(
            "bao-xdr-test-{}-{}-{tag}-{n}",
            std::process::id(),
            thread
        ));
        let _ = std::fs::remove_dir_all(&dir);
        set_cache_dir_for_tests(dir.clone());
        dir
    }

    fn make_jsvalue_fresh_realm(
        src: &str,
        filename: &str,
    ) -> Result<JsValue, String> {
        let mut ctx = JsContext::for_test().map_err(|e| e.message)?;
        let mut cx = ctx.cx();
        let global_ptr = ctx
            .ensure_realm_global(&mut cx, None)
            .map_err(|e| e.message)?;
        rooted!(&in(cx) let global = global_ptr);
        let c_filename = CString::new(filename).unwrap();
        rooted!(&in(cx) let mut rval = UndefinedValue());
        evaluate_script_cached(&mut cx, global.handle(), src, &c_filename, 1, rval.handle_mut())
            .map_err(|_| "eval failed".to_string())?;
        unsafe { Ok(jsval_to_jsvalue(cx.raw_cx_no_gc(), rval.get())) }
    }

    /// Deterministic ≥-admission-floor payload whose completion value is a
    /// plain number (compile-heavy for the C2 comparison).
    fn heavy_payload(tag: u64, functions: usize) -> String {
        let mut src = String::new();
        for i in 0..functions {
            src.push_str(&format!(
                "function f{tag}_{i}(x) {{ var s = 0; for (var j = 0; j < 8; j++) {{ s += x * {i} + j; }} return s; }}\n"
            ));
        }
        src.push_str(&format!("globalThis.__xdr_tag = {tag};\n"));
        src.push_str(&format!("f{tag}_0(2) + f{tag}_1(3)\n"));
        src
    }

    fn entry_files(dir: &PathBuf) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().map_or(false, |e| e == "xdr"))
            .collect();
        out.sort();
        out
    }

    fn tmp_files(dir: &PathBuf) -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map_or(false, |n| n.contains(".tmp."))
            })
            .collect()
    }

    fn number_value(v: &JsValue) -> f64 {
        match v {
            JsValue::Number(n) => *n,
            other => panic!("expected number, got {other:?}"),
        }
    }

    /// REQ-ENG-012 C1 (+ disk-hit plumbing): first eval compiles + stores;
    /// a memory-cold second eval must be served by the DISK layer and stay
    /// execution-equal to the direct compile.
    #[test]
    fn c1_disk_hit_is_execution_equal_to_direct_compile() {
        let dir = fresh_test_dir("c1");
        let src = heavy_payload(1, 24);

        let first = make_jsvalue_fresh_realm(&src, "<xdr-c1>").unwrap();
        let dbg = xdr_counters();
        eprintln!(
            "[c1dbg] dir={:?} exists={} stores={} failed={} disabled={} misses_absent={}",
            dir, dir.exists(), dbg.stores, dbg.store_failed, dbg.disabled, dbg.miss_absent
        );
        assert_eq!(entry_files(&dir).len(), 1, "store must persist one entry");
        let c = xdr_counters();
        assert_eq!(c.stores, 1, "exactly one store");

        // Cold memory: next eval must take the disk path.
        clear_thread_cache();
        let hits_before = xdr_counters().hits;
        let second = make_jsvalue_fresh_realm(&src, "<xdr-c1>").unwrap();
        let c = xdr_counters();
        assert_eq!(c.hits, hits_before + 1, "second eval must be a disk hit");
        assert_eq!(
            number_value(&first),
            number_value(&second),
            "C1: decoded execution must equal direct compile"
        );
        // The decoded stencil entered the in-memory layer.
        assert_eq!(thread_cache_len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// REQ-ENG-012 C3: truncated entry → counted corrupt miss, transparent
    /// fallback recompile, entry self-heals on the re-store.
    #[test]
    fn c3_truncated_entry_falls_back_and_self_heals() {
        let dir = fresh_test_dir("c3trunc");
        let src = heavy_payload(3, 24);

        make_jsvalue_fresh_realm(&src, "<xdr-c3>").unwrap();
        let path = &entry_files(&dir)[0];
        let full = std::fs::read(path).unwrap();
        std::fs::write(path, &full[..full.len() / 2]).unwrap();

        clear_thread_cache();
        let c0 = xdr_counters();
        let value = make_jsvalue_fresh_realm(&src, "<xdr-c3>").unwrap();
        let c = xdr_counters();
        assert_eq!(c.miss_corrupt, c0.miss_corrupt + 1, "truncation = corrupt miss");
        assert_eq!(c.hits, c0.hits, "corrupt entry must not hit");
        // Transparent fallback: still the right answer.
        assert!(number_value(&value) >= 0.0);

        // Self-healing: the fallback compile re-stored a good entry.
        assert_eq!(entry_files(&dir).len(), 1);
        clear_thread_cache();
        let hits_before = xdr_counters().hits;
        let again = make_jsvalue_fresh_realm(&src, "<xdr-c3>").unwrap();
        assert_eq!(xdr_counters().hits, hits_before + 1, "healed entry hits");
        assert!(number_value(&again) >= 0.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// REQ-ENG-012 C3: entry written under a different build id (byte-flip in
    /// the id field) = stale miss, never decoded.
    #[test]
    fn c3_stale_build_id_is_a_miss() {
        let dir = fresh_test_dir("c3stale");
        let src = heavy_payload(4, 24);

        make_jsvalue_fresh_realm(&src, "<xdr-c3s>").unwrap();
        let path = &entry_files(&dir)[0];
        // Entry layout: 8 magic + 4 id-len + id bytes — flip the id's first
        // byte without changing any length.
        let mut bytes = std::fs::read(path).unwrap();
        let id_start = 8 + 4;
        bytes[id_start] ^= 0xff;
        std::fs::write(path, &bytes).unwrap();

        clear_thread_cache();
        let c0 = xdr_counters();
        let value = make_jsvalue_fresh_realm(&src, "<xdr-c3s>").unwrap();
        let c = xdr_counters();
        assert_eq!(c.miss_stale, c0.miss_stale + 1, "id flip = stale miss");
        assert_eq!(c.hits, c0.hits, "stale entry must not hit");
        assert!(number_value(&value) >= 0.0, "fallback still evaluates");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// REQ-ENG-012 C4: writes leave exactly one entry and no temp files;
    /// a crash leftover temp file is never consumed.
    #[test]
    fn c4_atomic_write_leaves_no_temp_and_ignores_leftovers() {
        let dir = fresh_test_dir("c4");
        let src = heavy_payload(5, 24);

        make_jsvalue_fresh_realm(&src, "<xdr-c4>").unwrap();
        assert_eq!(entry_files(&dir).len(), 1);
        assert!(
            tmp_files(&dir).is_empty(),
            "completed store must not leave temp files"
        );

        // Simulate a crash leftover (never renamed): a partial temp file.
        let leftover = dir.join(".deadbeef.tmp999");
        std::fs::write(&leftover, b"BAOXDR1\npartial").unwrap();
        clear_thread_cache();
        let value = make_jsvalue_fresh_realm(&src, "<xdr-c4>").unwrap();
        assert!(number_value(&value) >= 0.0);
        assert!(leftover.exists(), "leftover temp is not consumed");
        let c = xdr_counters();
        assert!(c.stores >= 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// REQ-ENG-012 C2 evidence surface: cold start takes the decode path and
    /// the decode-served eval is measurably faster than the compile-served
    /// one (same payload, fresh realm each, memory layer cleared between).
    #[test]
    fn c2_cold_start_decode_beats_recompile_on_heavy_payload() {
        let dir = fresh_test_dir("c2");
        let src = heavy_payload(6, 240); // compile-heavy: 240 functions

        let t0 = Instant::now();
        make_jsvalue_fresh_realm(&src, "<xdr-c2>").unwrap();
        let compile_us = t0.elapsed().as_micros() as u64;

        clear_thread_cache();
        let hits_before = xdr_counters().hits;
        let t1 = Instant::now();
        make_jsvalue_fresh_realm(&src, "<xdr-c2>").unwrap();
        let decode_us = t1.elapsed().as_micros() as u64;

        assert_eq!(xdr_counters().hits, hits_before + 1, "cold eval = disk hit");
        let c = xdr_counters();
        assert!(c.decode_us > 0, "decode time must be recorded");
        assert!(
            decode_us < compile_us,
            "C2: decode-served cold start ({decode_us}µs) must beat compile-served ({compile_us}µs)"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Store failures are non-fatal by construction: an entry file replaced
    /// by a directory makes the rename fail — eval must still succeed.
    #[test]
    fn store_io_failure_is_non_fatal() {
        let dir = fresh_test_dir("c4io");
        let src = heavy_payload(7, 24);

        make_jsvalue_fresh_realm(&src, "<xdr-io>").unwrap();
        let path = &entry_files(&dir)[0];
        // Remove the entry and put a DIRECTORY at its path: rename fails.
        std::fs::remove_file(path).unwrap();
        std::fs::create_dir(path).unwrap();

        clear_thread_cache();
        let c0 = xdr_counters();
        // Memory miss → disk hit path reads a directory → read fails → absent
        // miss → recompile → store fails on rename (dir at target) — all
        // transparent.
        let value = make_jsvalue_fresh_realm(&src, "<xdr-io>").unwrap();
        let c = xdr_counters();
        assert!(number_value(&value) >= 0.0, "eval succeeds despite store failure");
        let failed = c.store_failed - c0.store_failed;
        let absent = c.miss_absent - c0.miss_absent;
        assert!(
            failed + absent >= 1,
            "the failure path must be observable (store_failed={failed}, absent={absent})"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
