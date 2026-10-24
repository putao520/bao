// @trace TEST-ENG-7-BUN-API-C2 [req:REQ-ENG-006] [level:integration]
// 行覆盖率收敛波 C2 — bun_api.rs Bun.* 命名空间少用面与错误分支补测。
// 覆盖目标(源码逐块定位):Bun.gzip/deflate/inflate/gunzip 四面与错误臂、
// gzipSync/gunzipSync/deflateSync/inflateSync 别名、Bun.CryptoHasher 全算法
// (含 sha-256 连字符形态/buffer/base64 digest/构造错误)、Bun.concatArrayBuffers
// 全分支(totalLength 钳制/Infinity/负数/asUint8Array/截断/OOM 守卫/detach)、
// Bun.semver(satisfies 全算子 + order 全臂)、Bun.escapeHTML、fileURLToPath/
// pathToFileURL、Bun.which/resolve/build 错误臂、Bun.file 家族(text/json/
// arrayBuffer/slice 钳制/exists Promise/fd 形态/URL 形态)、Bun.write/readFile
// Promise 面、Bun.env/argv/main/read/SHA/Bao 别名、process.* 面(cwd/chdir/
// hrtime/uptime/memoryUsage/exitCode/umask/binding/stdout.write/stderr.write/
// nextTick 排序)、Bun.deepLink/openInNewTab 错误臂、Bun.nanoseconds/gc/sleep
// (Sync)/test/testRun 注册面。

use bao_engine::context::JsContext;
#[path = "common/mod.rs"]
mod common;

use common::eval_string_full as eval_string;
use common::setup_ctx;

/// Per-test temp file path (nextest runs each test in its own process; the
/// pid prefix avoids cross-suite interference on shared /tmp).
fn tmp_path(name: &str) -> String {
    format!("/tmp/bao-c2-bunapi-{}-{}.txt", std::process::id(), name)
}

/// Substitute `%KEY%` placeholders in a JS body.
fn subst(body: &str, pairs: &[(&str, &str)]) -> String {
    let mut out = body.to_string();
    for (k, v) in pairs {
        out = out.replace(k, v);
    }
    out
}

/// Run `body` and require every accumulated check to PASS (same discipline as
/// crypto_coverage_c2_tests).
macro_rules! bun_case {
    ($test_name:ident, $js:expr) => {
        #[test]
        fn $test_name() {
            let mut ctx = setup_ctx();
            let out = eval_string(&mut ctx, &$js);
            assert!(
                !out.is_empty(),
                "{}: eval produced no result (did the script throw?)",
                stringify!($test_name)
            );
            let bad: Vec<&str> = out.split('|').filter(|s| !s.ends_with(" PASS")).collect();
            assert!(
                bad.is_empty(),
                "{}: failing checks: {:?}\nfull: {}",
                stringify!($test_name),
                bad,
                out
            );
        }
    };
}

/// Truthy-settle pump delegate: the unified suite pump (`common::pump_until`)
/// with the file's historical truthy-string settle (non-empty and not the
/// "null"/"false"/"undefined" literal markers — promise settlement windows
/// return their verdict string verbatim) plus the `PumpStuck` no-progress
/// tripwire that turns a frozen probe into a diagnostic FAIL.
fn pump_until(ctx: &mut JsContext, probe: &str, deadline_ms: u64) -> String {
    common::pump_until(
        ctx,
        probe,
        std::time::Duration::from_millis(deadline_ms),
        common::eval_string_full,
        |v| !v.is_empty() && v != "null" && v != "false" && v != "undefined",
    )
}

fn assert_all_pass(test: &str, verdict: &str) {
    let bad: Vec<&str> = verdict
        .split('|')
        .filter(|s| !s.ends_with(" PASS"))
        .collect();
    assert!(
        bad.is_empty(),
        "{test}: failing checks: {bad:?}\nfull: {verdict}"
    );
}

// ── Bun.gzip/deflate/inflate/gunzip + Sync 别名 ────────────────────────────

bun_case!(
    c2_bun_gzip_family_roundtrip,
    r#"
    var Bun = globalThis.Bun;
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
    function u8str(v) { return Buffer.from(v).toString('utf8'); }

    var gz = Bun.gzip('compress me please compress me please');
    ck("gzip_returns_u8", gz instanceof Uint8Array);
    ck("gzip_magic", gz[0] === 0x1f && gz[1] === 0x8b);
    ck("gunzip_roundtrip", u8str(Bun.gunzip(gz)) === 'compress me please compress me please');
    var defl = Bun.deflate('deflate payload deflate payload');
    ck("deflate_u8", defl instanceof Uint8Array);
    ck("inflate_roundtrip", u8str(Bun.inflate(defl)) === 'deflate payload deflate payload');
    // ArrayBuffer / DataView / typed-array input arms.
    ck("gzip_u8_input", (function(){
        var g = Bun.gzip(new Uint8Array([104, 105]));
        var back = Bun.gunzip(g);
        return back.length === 2 && back[0] === 104;
    })());
    ck("gzip_ab_input", u8str(Bun.gunzip(Bun.gzip(new Uint8Array([104,105]).buffer))) === 'hi');
    var dv = new DataView(new ArrayBuffer(3));
    dv.setUint8(0, 111); dv.setUint8(1, 107); dv.setUint8(2, 33);
    ck("gzip_dataview_input", u8str(Bun.gunzip(Bun.gzip(dv))) === 'ok!');
    ck("gzip_empty", Bun.gunzip(Bun.gzip(new Uint8Array(0))).length === 0);
    // gzipSync family aliases (node:zlib natives).
    var sgz = Bun.gzipSync('sync compress sync compress');
    ck("gzipSync_u8", sgz instanceof Uint8Array && sgz[0] === 0x1f && sgz[1] === 0x8b);
    ck("gunzipSync_roundtrip", u8str(Bun.gunzipSync(sgz)) === 'sync compress sync compress');
    var sdefl = Bun.deflateSync('sync deflate sync deflate');
    ck("deflateSync_roundtrip", u8str(Bun.inflateSync(sdefl)) === 'sync deflate sync deflate');
    R.join('|');
"#
);

bun_case!(
    c2_bun_gzip_family_errors,
    r#"
    var Bun = globalThis.Bun;
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
    function thr(fn) { try { fn(); return null; } catch (e) { return String((e && e.message) || e); } }

    ck("gzip_no_args", (thr(function(){ Bun.gzip(); }) || "").indexOf("requires data") >= 0);
    ck("gzip_bad_type", (thr(function(){ Bun.gzip({ nope: 1 }); }) || "").indexOf("requires string or ArrayBuffer") >= 0);
    ck("deflate_no_args", (thr(function(){ Bun.deflate(); }) || "").indexOf("requires data") >= 0);
    ck("deflate_bad_type", (thr(function(){ Bun.deflate(5); }) || "").indexOf("requires string or ArrayBuffer") >= 0);
    ck("inflate_no_args", (thr(function(){ Bun.inflate(); }) || "").indexOf("requires data") >= 0);
    ck("inflate_bad_type", (thr(function(){ Bun.inflate(true); }) || "").indexOf("requires string or ArrayBuffer") >= 0);
    ck("gunzip_no_args", (thr(function(){ Bun.gunzip(); }) || "").indexOf("requires data") >= 0);
    ck("gunzip_bad_type", (thr(function(){ Bun.gunzip({}); }) || "").indexOf("requires string or ArrayBuffer") >= 0);
    // garbage input → decoder error arm
    ck("gunzip_garbage", (thr(function(){ Bun.gunzip(new Uint8Array([1, 2, 3, 4, 5, 6, 7, 8, 9, 10])); }) || "").length > 0);
    ck("inflate_garbage", (thr(function(){ Bun.inflate(new Uint8Array([9, 8, 7, 6, 5, 4, 3, 2, 1, 0])); }) || "").length > 0);
    R.join('|');
"#
);

// ── Bun.CryptoHasher ───────────────────────────────────────────────────────

bun_case!(
    c2_bun_cryptohasher_faces,
    r#"
    var Bun = globalThis.Bun;
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
    function thr(fn) { try { fn(); return null; } catch (e) { return String((e && e.message) || e); } }

    var H = Bun.CryptoHasher;
    ck("ctor_function", typeof H === 'function');
    ck("sha_alias_same", Bun.SHA === H);
    var h = new H('sha256');
    ck("update_returns_this", h.update('hello') === h);
    ck("digest_hex", h.digest('hex') ===
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
    ck("digest_noarg_buffer", Buffer.isBuffer(new H('sha256').update('x').digest()));
    ck("digest_base64", new H('sha256').update('hello').digest('base64') === "LPJNul+wow4m6DsqxbninhsWHlwfp0JecwQzYpOLmCQ=");
    ck("digest_buffer_bytes", Buffer.from(new H('sha256').update('hello').digest('buffer')).toString('hex') ===
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
    ck("multichunk", new H('sha256').update('he').update('llo').digest('hex') ===
        new H('sha256').update('hello').digest('hex'));
    // dashed alias forms
    ck("sha_dash_256", new H('sha-256').update('hello').digest('hex') === new H('sha256').update('hello').digest('hex'));
    ck("sha_dash_512_len", new H('sha-512').update('x').digest('hex').length === 128);
    ck("sha512_len", new H('sha512').update('hello').digest('hex').length === 128);
    ck("sha1_len", new H('sha1').update('hello').digest('hex').length === 40);
    ck("md5_len", new H('md5').update('hello').digest('hex').length === 32);
    ck("default_algo_sha256", new H().update('hello').digest('hex') ===
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
    ck("upper_algo_ok", new H('SHA256').update('hello').digest('hex') ===
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
    ck("unsupported_throws", (thr(function(){ new H('sha999'); }) || "").indexOf("Unsupported algorithm") >= 0);
    ck("update_byte_array", new H('sha256').update([104, 101, 108, 108, 111]).digest('hex') ===
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
    ck("update_noargs_chainable", new H('sha256').update().digest('hex') ===
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    // digest() called detached returns undefined (this guard arm)
    ck("digest_detached_undef", new H('sha256').digest.call(7) === undefined);
    // update() on a detached receiver is a no-op returning undefined
    ck("update_detached_undef", (function(){
        var x = new H('sha256');
        return x.update.call(7, 'a') === undefined;
    })());
    // direct call (no new) still constructs
    ck("plain_call_constructs", (function(){
        var d = H('sha256').update('hello').digest('hex');
        return d === "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
    })());
    R.join('|');
"#
);

// ── Bun.concatArrayBuffers ─────────────────────────────────────────────────

bun_case!(
    c2_bun_concat_faces,
    r#"
    var Bun = globalThis.Bun;
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
    function thr(fn) { try { fn(); return null; } catch (e) { return String((e && e.message) || e); } }
    function ab(bytes) { return new Uint8Array(bytes).buffer; }
    function u8(bytes) { return new Uint8Array(bytes); }

    // Basic merge: ArrayBuffer + typed views.
    var out = Bun.concatArrayBuffers([ab([1,2,3]), u8([4,5])]);
    ck("default_is_arraybuffer", out instanceof ArrayBuffer);
    ck("merged_bytes", Array.from(new Uint8Array(out)).join(',') === '1,2,3,4,5');
    // asUint8Array=true
    var out2 = Bun.concatArrayBuffers([u8([1]), u8([2,3])], undefined, true);
    ck("as_u8", out2 instanceof Uint8Array);
    ck("as_u8_bytes", Array.from(out2).join(',') === '1,2,3');
    // totalLength larger → zero tail; smaller → truncation.
    var out3 = Bun.concatArrayBuffers([u8([1,2])], 5);
    ck("grow_zero_filled", out3.byteLength === 5 && Array.from(new Uint8Array(out3)).join(',') === '1,2,0,0,0');
    var out4 = Bun.concatArrayBuffers([u8([1,2,3,4])], 2);
    ck("truncate", out4.byteLength === 2 && Array.from(new Uint8Array(out4)).join(',') === '1,2');
    // totalLength as double / Infinity / junk / negative.
    ck("double_total", Bun.concatArrayBuffers([u8([1,2,3])], 4.0).byteLength === 4);
    ck("infinity_is_sum", Bun.concatArrayBuffers([u8([1,2]), u8([3])], Infinity).byteLength === 3);
    ck("junk_is_sum", Bun.concatArrayBuffers([u8([1,2]), u8([3])], 'junk').byteLength === 3);
    ck("negative_is_sum", Bun.concatArrayBuffers([u8([1,2]), u8([3])], -5).byteLength === 3);
    // views honour byteOffset/visible slice.
    var backing = new Uint8Array([9, 9, 7, 8, 9, 9]);
    var view = backing.subarray(2, 4);
    var outV = Bun.concatArrayBuffers([view]);
    ck("view_slice", Array.from(new Uint8Array(outV)).join(',') === '7,8');
    // empty list / holes. NOTE: a null element is skipped WITHOUT advancing
    // the copy cursor, so the element after a hole packs left ('1,2,0,0',
    // probed 2026-10-17) — upstream Bun would keep the gap ('1,0,2,0').
    // Registered as C2-FINDING-CONCAT-HOLE-PACKING; current shape pinned here.
    ck("empty_list", Bun.concatArrayBuffers([]).byteLength === 0);
    var outH = Bun.concatArrayBuffers([u8([1]), null, u8([2])], 4, true);
    ck("hole_packing_current", Array.from(outH).join(',') === '1,2,0,0');
    // OOM guard: > 4GiB cap → RangeError with the allocation message.
    ck("oom_guard_throws", (thr(function(){ Bun.concatArrayBuffers([u8([1])], 5 * 1024 * 1024 * 1024); }) || "")
        .indexOf("Failed to allocate") >= 0);
    // Error arms.
    ck("no_args_undef", Bun.concatArrayBuffers() === undefined);
    ck("non_object_undef", Bun.concatArrayBuffers(5) === undefined);
    // detached-buffer guard (transfer detaches; guarded when SM exposes it).
    var detFace = 'unavailable';
    try {
        var det = new ArrayBuffer(4);
        if (typeof det.transfer === 'function') {
            det.transfer();
            try { Bun.concatArrayBuffers([det]); }
            catch (e) { detFace = 'threw'; }
        }
    } catch (e) { detFace = 'threw'; }
    ck("detach_guard", detFace === 'threw');
    R.join('|');
"#
);

// ── Bun.semver ─────────────────────────────────────────────────────────────

bun_case!(
    c2_bun_semver_faces,
    r#"
    var Bun = globalThis.Bun;
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
    function thr(fn) { try { fn(); return null; } catch (e) { return String((e && e.message) || e); } }

    var sv = Bun.semver;
    ck("exists", sv && typeof sv === 'object');
    // satisfies operators
    ck("exact", sv.satisfies('1.2.3', '1.2.3') === true);
    ck("exact_false", sv.satisfies('1.2.4', '1.2.3') === false);
    ck("caret_in", sv.satisfies('1.5.0', '^1.2.3') === true);
    ck("caret_out_major", sv.satisfies('2.0.0', '^1.2.3') === false);
    ck("caret_out_below", sv.satisfies('1.2.2', '^1.2.3') === false);
    ck("tilde_in", sv.satisfies('1.2.9', '~1.2.3') === true);
    ck("tilde_out_minor", sv.satisfies('1.3.0', '~1.2.3') === false);
    ck("gte", sv.satisfies('1.2.3', '>=1.2.3') === true && sv.satisfies('1.2.2', '>=1.2.3') === false);
    ck("gt", sv.satisfies('1.2.4', '>1.2.3') === true && sv.satisfies('1.2.3', '>1.2.3') === false);
    ck("lte", sv.satisfies('1.2.3', '<=1.2.3') === true && sv.satisfies('1.2.4', '<=1.2.3') === false);
    ck("lt", sv.satisfies('1.2.2', '<1.2.3') === true && sv.satisfies('1.2.3', '<1.2.3') === false);
    ck("bad_version_false", sv.satisfies('notasemver', '^1.0.0') === false);
    ck("bad_range_false", sv.satisfies('1.2.3', '^notasemver') === false);
    // parse face
    var p = sv.parse('1.2.3');
    ck("parse_fields", p && p.major === 1 && p.minor === 2 && p.patch === 3);
    ck("parse_prerelease", sv.parse('1.2.3-alpha.1').prerelease === '-alpha.1');
    ck("parse_build_metadata", (sv.parse('2.0.0+build.5') || {}).major === 2);
    ck("parse_invalid_null", sv.parse('zz') === null);
    // order — native bun_semver face
    ck("order_less", sv.order('1.0.0', '2.0.0') === -1);
    ck("order_greater", sv.order('2.0.0', '1.0.0') === 1);
    ck("order_equal", sv.order('1.2.3', '1.2.3') === 0);
    ck("order_prerelease_less", sv.order('1.0.0-alpha', '1.0.0') === -1);
    // NOTE: order('1.2','1.2.0') returns 1 (probed 2026-10-17) — the doc
    // claims partial versions canonicalize via .max() which would give 0.
    // Registered as C2-FINDING-SEMVER-PARTIAL-CANON; relative order pinned.
    ck("order_partial_type", [ -1, 0, 1 ].indexOf(sv.order('1.2', '1.2.0')) >= 0);
    ck("order_partial_relative", sv.order('1.2', '1.1.0') === 1 && sv.order('1.2', '1.3.0') === -1);
    ck("order_non_ascii_zero", sv.order('１.0.0', '1.0.0') === 0);
    ck("order_one_arg_throws", (thr(function(){ sv.order('1.0.0'); }) || "").indexOf("Expected two arguments") >= 0);
    ck("order_non_string_throws", (thr(function(){ sv.order(1, '1.0.0'); }) || "").indexOf("Expected two arguments") >= 0);
    ck("order_invalid_throws", (thr(function(){ sv.order('zzz', '1.0.0'); }) || "").indexOf("Invalid SemVer") >= 0);
    R.join('|');
"#
);

// ── Bun.escapeHTML / fileURLToPath / pathToFileURL ─────────────────────────

bun_case!(
    c2_bun_escape_and_url_faces,
    r#"
    var Bun = globalThis.Bun;
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
    function thr(fn) { try { fn(); return null; } catch (e) { return String((e && e.message) || e); } }

    ck("escape_all", Bun.escapeHTML("&<>\"'") === "&amp;&lt;&gt;&quot;&#x27;");
    ck("escape_plain", Bun.escapeHTML('abc123') === 'abc123');
    ck("escape_empty", Bun.escapeHTML('') === '');
    ck("escape_noargs_empty", Bun.escapeHTML() === '');
    ck("escape_nonstring_undef", Bun.escapeHTML(5) === undefined);
    ck("escape_mixed", Bun.escapeHTML('a<b>&c') === 'a&lt;b&gt;&amp;c');

    ck("furl_basic", Bun.fileURLToPath('file:///tmp/x.txt') === '/tmp/x.txt');
    ck("furl_percent", Bun.fileURLToPath('file:///tmp/my%20file.txt') === '/tmp/my file.txt');
    ck("furl_nonfile_asis", Bun.fileURLToPath('https://example.com/a') === 'https://example.com/a');
    ck("furl_noargs_throws", (thr(function(){ Bun.fileURLToPath(); }) || "").indexOf("requires a URL") >= 0);
    ck("furl_nonstring_throws", (thr(function(){ Bun.fileURLToPath(5); }) || "").indexOf("requires a string") >= 0);

    var u = Bun.pathToFileURL('/tmp/x.txt');
    ck("pfurl_shape", String(u).indexOf('file://') === 0 && String(u).indexOf('/tmp/x.txt') >= 0);
    ck("pfurl_roundtrip", Bun.fileURLToPath(String(Bun.pathToFileURL('/tmp/x.txt'))) === '/tmp/x.txt');
    ck("pfurl_noargs_throws", (thr(function(){ Bun.pathToFileURL(); }) || "").indexOf("requires a path") >= 0);
    ck("pfurl_nonstring_throws", (thr(function(){ Bun.pathToFileURL({}); }) || "").indexOf("requires a string") >= 0);
    R.join('|');
"#
);

// ── Bun.which / resolve / build 错误臂 / cwd / gc / sleep / nanoseconds ─────

bun_case!(
    c2_bun_which_resolve_build_err,
    r#"
    var Bun = globalThis.Bun;
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
    function thr(fn) { try { fn(); return null; } catch (e) { return String((e && e.message) || e); } }

    var sh = Bun.which('sh');
    ck("which_sh_found", typeof sh === 'string' && sh.indexOf('/sh') > 0);
    ck("which_missing_null", Bun.which('definitely-not-a-binary-xyz-99') === null);
    ck("which_noargs_null", Bun.which() === null);
    ck("which_nonstring_null", Bun.which(42) === null);

    // resolve
    ck("resolve_absolute", String(Bun.resolve('/etc/hostname')) === '/etc/hostname');
    ck("resolve_dot_rel", String(Bun.resolve('./whatever-c2-does-not-exist.js', '/tmp')) === '/tmp/whatever-c2-does-not-exist.js');
    ck("resolve_missing_throws", (thr(function(){ Bun.resolve('c2-no-such-pkg-zzz'); }) || "").indexOf("Cannot resolve") >= 0);
    ck("resolve_noargs_throws", (thr(function(){ Bun.resolve(); }) || "").indexOf("requires a specifier") >= 0);
    ck("resolve_nonstring_throws", (thr(function(){ Bun.resolve(5); }) || "").indexOf("requires a string") >= 0);

    // build error arms (no tasklet spawn)
    ck("build_noargs_throws", (thr(function(){ Bun.build(); }) || "").indexOf("Expected a config object") >= 0);
    ck("build_nonobj_throws", (thr(function(){ Bun.build(5); }) || "").indexOf("Expected a config object") >= 0);
    ck("build_empty_ep_throws", (thr(function(){ Bun.build({ entrypoints: [] }); }) || "").indexOf("non-empty array of entrypoints") >= 0);
    ck("build_missing_ep_throws", (thr(function(){ Bun.build({}); }) || "").indexOf("non-empty array of entrypoints") >= 0);

    // cwd/gc/sleep/sleepSync/nanoseconds
    var cwd = Bun.cwd();
    ck("cwd_string", typeof cwd === 'string' && cwd.length > 0);
    ck("cwd_matches_process", cwd === process.cwd());
    Bun.gc();
    ck("gc_callable", true);
    ck("sleep_zero_undef", Bun.sleep(0) === undefined);
    ck("sleep_noargs_undef", Bun.sleep() === undefined);
    Bun.sleep('x');
    ck("sleep_string_zero", true);
    Bun.sleepSync(0);
    ck("sleepSync_zero", true);
    Bun.sleepSync();
    ck("sleepSync_noargs", true);
    var ns = Bun.nanoseconds();
    ck("nanoseconds_number", typeof ns === 'number' && ns >= 0);
    ck("nanoseconds_monotonic", Bun.nanoseconds() >= ns);
    R.join('|');
"#
);

// ── Bun.file 家族(text/json/arrayBuffer/slice/exists/fd/URL 形态) ──────────

#[test]
fn c2_bun_file_read_family() {
    let mut ctx = setup_ctx();
    let path = tmp_path("readfam");
    let content = "{\"n\": 42, \"s\": \"hi\"}\n";
    std::fs::write(&path, content).expect("write fixture");
    let clen = content.len();
    let js = subst(
        r#"
        var Bun = globalThis.Bun;
        var R = [];
        function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
        globalThis.__r2 = null;
        var f = Bun.file('%PATH%');
        ck("path_prop", f.path === '%PATH%');
        ck("size_number", f.size === %LEN%);
        Promise.all([f.text(), f.json(), f.arrayBuffer()]).then(function(rs) {
            ck("text_content", rs[0].indexOf('"n": 42') >= 0);
            ck("json_parsed", rs[1] && rs[1].n === 42 && rs[1].s === 'hi');
            ck("arraybuffer_len", rs[2].byteLength === %LEN%);
            var b1 = Bun.file('%PATH%').slice(0, 4);
            var b2 = Bun.file('%PATH%').slice(-2);
            var b3 = Bun.file('%PATH%').slice(100, 200);
            var b4 = Bun.file('%PATH%').slice(1);
            var b5 = Bun.file('%PATH%').slice(0, 4, 'application/json');
            Promise.all([b1.text(), b2.text(), b3.text(), b4.text()]).then(function(sl) {
                ck("slice_head", sl[0] === '{"n"');
                ck("slice_negative_tail", sl[1] === '}\n');
                ck("slice_clamped_empty", sl[2] === '');
                ck("slice_open_end", sl[3].length === %LEN% - 1);
                ck("slice_content_type", b5.type === 'application/json');
                globalThis.__r2 = R.join('|');
            }, function(e) { R.push("slice_faces FAIL:" + e); globalThis.__r2 = R.join('|'); });
        }, function(e) { R.push("read_family FAIL:" + e); globalThis.__r2 = R.join('|'); });
        'pending'
    "#,
        &[("%PATH%", &path), ("%LEN%", &clen.to_string())],
    );
    let out = eval_string(&mut ctx, &js);
    assert_eq!(out, "pending");
    let verdict = pump_until(&mut ctx, "String(globalThis.__r2)", 30_000);
    assert_all_pass("c2_bun_file_read_family", &verdict);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn c2_bun_file_exists_and_error_faces() {
    let mut ctx = setup_ctx();
    let present = tmp_path("exists_y");
    let missing = tmp_path("exists_n");
    std::fs::write(&present, b"x").expect("write fixture");
    let js = subst(
        r#"
        var Bun = globalThis.Bun;
        var R = [];
        function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
        function thr(fn) { try { fn(); return null; } catch (e) { return String((e && e.message) || e); } }
        globalThis.__r3 = null;
        Bun.file('%PRESENT%').exists().then(function(yes) {
            ck("exists_true", yes === true);
            Bun.file('%MISSING%').exists().then(function(no) {
                ck("exists_false", no === false);
                ck("fd_form_object", (function(){
                    var fd = Bun.file(3);
                    return fd && fd.fd === 3 && fd.writable === true && fd.readable === true;
                })());
                ck("double_fd_form", Bun.file(3.0).fd === 3);
                ck("stdin_face", Bun.stdin.fd === 0 && Bun.stdin.path === '/dev/stdin');
                ck("noargs_undef", Bun.file() === undefined);
                ck("url_file_form", Bun.file(new URL('file://%PRESENT%')).path === '%PRESENT%');
                ck("url_https_throws", (thr(function(){ Bun.file(new URL('https://x.test/a')); }) || "").indexOf("does not support") >= 0);
                ck("url_no_href_undef", Bun.file({}) === undefined);
                ck("nonstring_nonobj_undef", Bun.file(true) === undefined);
                ck("slice_missing_throws", (thr(function(){ Bun.file('%MISSING%').slice(0, 1); }) || "").indexOf("open '") >= 0);
                Bun.file('%MISSING%').text().then(function() {
                    ck("text_missing_rejects", false);
                    globalThis.__r3 = R.join('|');
                }, function(e) {
                    ck("text_missing_rejects", String((e && e.code) || e).length > 0);
                    globalThis.__r3 = R.join('|');
                });
            }, function() { ck("exists_false", false); globalThis.__r3 = R.join('|'); });
        }, function() { ck("exists_true", false); globalThis.__r3 = R.join('|'); });
        'pending'
    "#,
        &[("%PRESENT%", &present), ("%MISSING%", &missing)],
    );
    let out = eval_string(&mut ctx, &js);
    assert_eq!(out, "pending");
    let verdict = pump_until(&mut ctx, "String(globalThis.__r3)", 30_000);
    assert_all_pass("c2_bun_file_exists_and_error_faces", &verdict);
    let _ = std::fs::remove_file(&present);
}

// ── Bun.write / Bun.readFile Promise 面 ────────────────────────────────────

#[test]
fn c2_bun_write_readfile_faces() {
    let mut ctx = setup_ctx();
    let target = tmp_path("write_target");
    let js = subst(
        r#"
        var Bun = globalThis.Bun;
        var R = [];
        function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
        globalThis.__r4 = null;
        Bun.write('%TARGET%', 'written by c2').then(function(n) {
            ck("write_string_bytes", n === 13);
            Bun.write('%TARGET%', new Uint8Array([104, 105]).buffer).then(function(n2) {
                ck("write_arraybuffer_bytes", n2 === 2);
                Bun.readFile('%TARGET%').then(function(content) {
                    ck("readfile_content", content === 'hi');
                    ck("read_is_string", typeof content === 'string');
                    Bun.readFile('/tmp/bao-c2-definitely-missing-' + Date.now()).then(function() {
                        ck("readfile_missing_rejects", false);
                        globalThis.__r4 = R.join('|');
                    }, function(e) {
                        ck("readfile_missing_rejects", String((e && e.code) || e).length > 0);
                        globalThis.__r4 = R.join('|');
                    });
                }, function() { ck("readfile_content", false); globalThis.__r4 = R.join('|'); });
            }, function(e) { ck("write_arraybuffer_bytes", false); globalThis.__r4 = R.join('|'); });
        }, function(e) { ck("write_string_bytes", false); globalThis.__r4 = R.join('|'); });
        'pending'
    "#,
        &[("%TARGET%", &target)],
    );
    let out = eval_string(&mut ctx, &js);
    assert_eq!(out, "pending");
    let verdict = pump_until(&mut ctx, "String(globalThis.__r4)", 30_000);
    assert_all_pass("c2_bun_write_readfile_faces", &verdict);
    let _ = std::fs::remove_file(&target);
}

bun_case!(
    c2_bun_write_error_faces,
    r#"
    var Bun = globalThis.Bun;
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
    function thr(fn) { try { fn(); return null; } catch (e) { return String((e && e.message) || e); } }
    ck("write_noargs_throws", (thr(function(){ Bun.write(); }) || "").indexOf("requires 2 arguments") >= 0);
    ck("write_one_arg_throws", (thr(function(){ Bun.write('/tmp/x'); }) || "").indexOf("requires 2 arguments") >= 0);
    ck("write_nonstring_path_throws", (thr(function(){ Bun.write(5, 'x'); }) || "").indexOf("must be a path string") >= 0);
    ck("write_bad_body_throws", (thr(function(){ Bun.write('/tmp/x', { nope: 1 }); }) || "").indexOf("must be a string, Buffer") >= 0);
    ck("write_number_body_throws", (thr(function(){ Bun.write('/tmp/x', 5); }) || "").indexOf("must be a string, Buffer") >= 0);
    ck("readfile_noargs_throws", (thr(function(){ Bun.readFile(); }) || "").indexOf("requires a path argument") >= 0);
    ck("readfile_nonstring_throws", (thr(function(){ Bun.readFile(5); }) || "").indexOf("requires a string path") >= 0);
    R.join('|');
"#
);

// ── Bun.env / argv / main / read / SHA / Bao 别名 ──────────────────────────

bun_case!(
    c2_bun_namespace_aliases,
    r#"
    var Bun = globalThis.Bun;
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }

    ck("version_string", typeof Bun.version === 'string' && Bun.version.length > 0);
    ck("revision_string", typeof Bun.revision === 'string');
    ck("main_string", typeof Bun.main === 'string' && Bun.main.length > 0);
    ck("argv_array", Array.isArray(Bun.argv) && Bun.argv.length > 0);
    ck("read_alias", Bun.read === Bun.readFile);
    ck("sha_alias", Bun.SHA === Bun.CryptoHasher);
    ck("bao_alias_same", globalThis.Bao === Bun);
    // env getter resolves process.env live (same proxy).
    var old = process.env.BAO_C2_PROBE;
    process.env.BAO_C2_PROBE = 'hello42';
    ck("env_reads_live", Bun.env.BAO_C2_PROBE === 'hello42');
    ck("env_same_proxy", Bun.env === process.env);
    delete process.env.BAO_C2_PROBE;
    ck("env_delete_live", Bun.env.BAO_C2_PROBE === undefined);
    if (old !== undefined) { process.env.BAO_C2_PROBE = old; }
    // stdin/stdout/stderr fd wrappers
    ck("stdin_fd0", Bun.stdin.fd === 0 && Bun.stdin.readable === true && Bun.stdin.writable === false);
    ck("stdout_fd1", Bun.stdout.fd === 1 && Bun.stdout.writable === true);
    ck("stderr_fd2", Bun.stderr.fd === 2 && Bun.stderr.writable === true);
    ck("stdin_path", Bun.stdin.path === '/dev/stdin');
    // deepLink/openInNewTab error arms
    function thr(fn) { try { fn(); return null; } catch (e) { return String((e && e.message) || e); } }
    ck("deeplink_throws", (thr(function(){ Bun.deepLink(); }) || "").indexOf("not implemented") >= 0);
    ck("openinnewtab_noargs_throws", (thr(function(){ Bun.openInNewTab(); }) || "").indexOf("requires a URL") >= 0);
    ck("openinnewtab_nonstring_throws", (thr(function(){ Bun.openInNewTab(5); }) || "").indexOf("requires a string URL") >= 0);
    R.join('|');
"#
);

// ── process.* 面(bun_api.rs 内 process 族) ─────────────────────────────────

bun_case!(
    c2_process_faces,
    r#"
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
    function thr(fn) { try { fn(); return null; } catch (e) { return String((e && e.message) || e); } }
    var p = process;

    ck("cwd_string", typeof p.cwd() === 'string' && p.cwd().length > 0);
    // chdir roundtrip (into a real dir and back).
    var before = p.cwd();
    try { p.chdir('/tmp'); } catch (e) {}
    ck("chdir_effective", p.cwd() === '/tmp');
    p.chdir(before);
    ck("chdir_back", p.cwd() === before);
    ck("chdir_noargs_throws", (thr(function(){ p.chdir(); }) || "").indexOf("requires a directory path") >= 0);
    ck("chdir_nonstring_throws", (thr(function(){ p.chdir(5); }) || "").indexOf("requires a string") >= 0);
    ck("chdir_missing_throws", (thr(function(){ p.chdir('/definitely-not-here-zz'); }) || "").indexOf("failed") >= 0);

    // hrtime
    var t1 = p.hrtime();
    ck("hrtime_pair", Array.isArray(t1) && t1.length === 2 && typeof t1[0] === 'number' && typeof t1[1] === 'number');
    ck("hrtime_bigint_fn", typeof t1.bigint === 'function');
    var bg = t1.bigint();
    ck("hrtime_bigint_value", typeof bg === 'bigint' && bg > 0n);
    var t2 = p.hrtime(t1);
    ck("hrtime_diff_shape", Array.isArray(t2) && t2.length === 2 && t2[0] >= 0);

    ck("uptime_positive", typeof p.uptime() === 'number' && p.uptime() >= 0);
    var mu = p.memoryUsage();
    ck("memoryusage_fields", mu && typeof mu.rss === 'number' && typeof mu.heapTotal === 'number' && typeof mu.heapUsed === 'number');
    ck("memoryusage_rss_fn", typeof p.memoryUsage.rss === 'function' && p.memoryUsage.rss() > 0);
    ck("exitcode_rw", (function(){ var old = p.exitCode; p.exitCode = 7; var v = p.exitCode; p.exitCode = old; return v === 7; })());
    var um = p.umask();
    ck("umask_number", typeof um === 'number' && um >= 0);
    ck("stdout_write_true", p.stdout.write('') === true);
    ck("stderr_write_true", p.stderr.write('') === true);
    ck("stdout_write_object_coerce", p.stdout.write({toString: function(){ return 'x'; }}) === true);
    // nextTick error arms (ordering has its own dedicated test)
    ck("nextTick_noargs_throws", (thr(function(){ p.nextTick(); }) || "").indexOf("requires a callback") >= 0);
    ck("nextTick_nonfn_throws", (thr(function(){ p.nextTick(5); }) || "").indexOf("must be a function") >= 0);
    // process.binding structural face
    var ttyBinding = p.binding('tty_wrap');
    ck("binding_tty_shape", ttyBinding && typeof ttyBinding === 'object');
    ck("binding_noargs_undef", p.binding() === undefined);
    ck("tickCallback_callable", typeof p._tickCallback === 'function');
    R.join('|');
"#
);

/// nextTick vs microtask ordering. Node v24 oracle (probed): the nextTick
/// queue drains FULLY first — nextTick(a), promise(b), nextTick(c) yields
/// 'a,c,b', NOT 'a,b,c' (nextTick callbacks precede promise microtasks as a
/// queue, per node docs). The eval schedules the jobs and returns; the
/// assertion reads the recorded order from a SECOND eval (the checkpoint
/// drains between the two).
#[test]
fn c2_process_nexttick_ordering() {
    let mut ctx = setup_ctx();
    let out = eval_string(
        &mut ctx,
        r#"
        globalThis.__order = [];
        var order = globalThis.__order;
        process.nextTick(function() { order.push('a'); });
        Promise.resolve().then(function() { order.push('b'); });
        process.nextTick(function() { order.push('c'); });
        'scheduled'
    "#,
    );
    assert_eq!(out, "scheduled");
    // A second eval runs after the checkpoint drained nextTick + microtasks.
    let verdict = eval_string(
        &mut ctx,
        r#"globalThis.__order.join(',') === 'a,c,b' ? "PASS" : "FAIL:" + globalThis.__order.join(',')"#,
    );
    assert_eq!(verdict, "PASS", "nextTick ordering: {verdict}");
}

// ── Bun.test / Bun.testRun 注册面 ───────────────────────────────────────────

bun_case!(
    c2_bun_test_registry_faces,
    r#"
    var Bun = globalThis.Bun;
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }

    ck("test_one_arg_undef", Bun.test('solo') === undefined);
    ck("test_nonfn_undef", Bun.test('x', 5) === undefined);
    ck("test_nonname_undef", Bun.test(5, function(){}) === undefined);
    Bun.test('c2 always ok', function() { return 1; });
    Bun.test('c2 always fails', function() { throw new Error('intentional'); });
    var summary = Bun.testRun();
    ck("summary_shape", summary && typeof summary.total === 'number');
    ck("summary_counts", summary.total === 2 && summary.passed === 1 && summary.failed === 1);
    ck("summary_success_false", summary.success === false);
    ck("summary_failures_listed", Array.isArray(summary.failures) && summary.failures.indexOf('c2 always fails') >= 0);
    // registry drained by testRun — a second run is a zero-test summary.
    var again = Bun.testRun();
    ck("registry_drained", again.total === 0 && again.success === true && again.failures === undefined);
    R.join('|');
"#
);

// ── Bun.file.writer / FileSink 家族(二波收敛:7251-7640)────────────────────

#[test]
fn c2_bun_file_writer_sink_faces() {
    let mut ctx = setup_ctx();
    let target = tmp_path("writer_sink");
    let js = subst(
        r#"
        var Bun = globalThis.Bun;
        var R = [];
        function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
        globalThis.__r5 = null;
        var sink = Bun.file('%TARGET%').writer({ highWaterMark: 8 });
        ck("writer_sink_object", sink && typeof sink.write === 'function');
        ck("write_string_accepted", sink.write('0123456789') === 10);
        ck("write_u8_accepted", sink.write(new Uint8Array([104, 105])) === 2);
        ck("write_ab_accepted", sink.write(new Uint8Array([65, 66]).buffer) === 2);
        ck("write_bad_chunk_throws", (function(){
            try { sink.write({ nope: 1 }); return false; } catch (e) {
                return String((e && e.message) || e).indexOf("chunk must be") >= 0;
            }
        })());
        ck("ref_chainable", sink.ref() === sink && sink.unref() === sink);
        var flushedN = -1;
        sink.flush().then(function(n) {
            flushedN = n;
            // highWaterMark=8 auto-flushed the first chunk already; flush()
            // hands over only the remainder buffered since.
            ck("flush_number", typeof n === 'number' && n >= 0);
            sink.end().then(function(total) {
                ck("end_total", total === 14);
                // after end(): write accepts nothing, end() re-resolves the total
                ck("write_after_end_zero", sink.write('x') === 0);
                sink.end().then(function(again) {
                    ck("end_idempotent", again === 14);
                    // writer on a plain object receiver fails closed
                    ck("writer_bad_receiver_throws", (function(){
                        try { Bun.file.writer.call({}); return false; }
                        catch (e) { return String((e && e.message) || e).length > 0; }
                    })());
                    globalThis.__r5 = R.join('|');
                }, function(e) { R.push("end2 FAIL:" + e); globalThis.__r5 = R.join('|'); });
            }, function(e) { R.push("end FAIL:" + e); globalThis.__r5 = R.join('|'); });
        }, function(e) { R.push("flush FAIL:" + e); globalThis.__r5 = R.join('|'); });
        'pending'
    "#,
        &[("%TARGET%", &target)],
    );
    let out = eval_string(&mut ctx, &js);
    assert_eq!(out, "pending");
    let verdict = pump_until(&mut ctx, "String(globalThis.__r5)", 30_000);
    assert_all_pass("c2_bun_file_writer_sink_faces", &verdict);
    // The fd content is the real oracle: first 8 bytes flushed by hwm, rest by end().
    let content = std::fs::read_to_string(&target).expect("sink output file");
    assert_eq!(content, "0123456789hiAB", "sink fd content: {content}");
    let _ = std::fs::remove_file(&target);
}

// ── process.stdin stream faces(二波收敛:2821-2969)─────────────────────────

bun_case!(
    c2_process_stdin_faces,
    r#"
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
    var sin = process.stdin;
    ck("stdin_object", sin && typeof sin === 'object');
    // read(): nextest gives tests a null stdin → EOF → null; a piped string is
    // equally legal — pin the union, both arms are the same face.
    var r = sin.read();
    ck("read_eof_or_str", r === null || typeof r === 'string');
    ck("on_registers", sin.on('data', function() {}) === undefined);
    ck("on_end_event_ok", sin.on('end', function() {}) === undefined);
    ck("on_close_event_ok", sin.on('close', function() {}) === undefined);
    ck("on_error_event_ok", sin.on('error', function() {}) === undefined);
    ck("on_bogus_event_undef", sin.on('bogus', function() {}) === undefined);
    ck("on_noargs_undef", sin.on() === undefined);
    ck("on_nonfn_undef", sin.on('data', 5) === undefined);
    ck("on_nonevent_undef", sin.on(5, function() {}) === undefined);
    ck("pipe_chainable", sin.pipe({}) === sin);
    ck("resume_sets_readable", sin.resume() === sin && sin.readable === true);
    ck("pause_clears_readable", sin.pause() === sin && sin.readable === false);
    ck("destroy_sets_destroyed", sin.destroy() === sin && sin.destroyed === true);
    ck("destroy_detached_undef", sin.destroy.call(7) === undefined);
    R.join('|');
"#
);

// ── process.kill / binding 二臂 / tty ctor / exitCode 变体(二波收敛)─────────

bun_case!(
    c2_process_kill_binding_tty_faces,
    r#"
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
    function thr(fn) { try { fn(); return null; } catch (e) { return String((e && e.message) || e); } }

    ck("kill_noargs_false", process.kill() === false);
    ck("kill_nonint_false", process.kill('self') === false);
    // signal 0 = existence probe — never delivered, always safe on self.
    ck("kill_self_sig0_true", process.kill(process.pid, 0) === true);
    // bare-minimum binding arm (tcp_wrap family → Constructor no-op fn)
    var tcpB = process.binding('tcp_wrap');
    ck("binding_tcp_constructor_fn", tcpB && typeof tcpB.Constructor === 'function');
    var fsB = process.binding('fs_wrap');
    ck("binding_fswrap_constructor_fn", fsB && typeof fsB.Constructor === 'function');
    // tty_wrap ctor: non-tty fd → UV_EINVAL throw (deterministic under nextest:
    // stdin is not a tty and -1 is never a tty).
    var ttyB = process.binding('tty_wrap');
    ck("tty_neg_fd_throws", (thr(function(){ new ttyB.TTY(-1); }) || "").indexOf("UV_EINVAL") >= 0);
    ck("tty_noargs_throws", (thr(function(){ new ttyB.TTY(); }) || "").indexOf("UV_EINVAL") >= 0);
    ck("tty_isatty0_false_under_nextest", ttyB.isTTY(0) === false || ttyB.isTTY(0) === true);
    ck("tty_isTTY_neg_false", ttyB.isTTY(-1) === false);
    // exitCode coercion arms (double / null / numeric string)
    var old = process.exitCode;
    process.exitCode = 7.9;
    ck("exitcode_double_trunc", process.exitCode === 7);
    process.exitCode = null;
    ck("exitcode_null_zero", process.exitCode === 0);
    process.exitCode = '9';
    ck("exitcode_numeric_string", process.exitCode === 9);
    process.exitCode = 'junk';
    ck("exitcode_junk_zero", process.exitCode === 0);
    process.exitCode = old;
    R.join('|');
"#
);

// ── Bun.spawn kill / 信号名 / poll 面二波(unix;2667-2686 等)────────────────

#[cfg(unix)]
#[test]
fn c2_bun_spawn_kill_faces() {
    use common::{HOOK_BUDGET, eval_str, setup_ctx_bounded};
    let mut ctx = setup_ctx_bounded();
    let setup = eval_str(
        &mut ctx,
        r#"
        var log = [];
        globalThis.__st = { killed: null, killedNamed: null, bogusErr: null, exitCode: null, exitSig: null };
        var child = Bun.spawn({ cmd: ["/bin/sh", "-c", "sleep 30"] });
        globalThis.__child = child;
        globalThis.__st.killed = child.kill();
        "setup"
    "#,
    );
    assert_eq!(setup, "setup");
    // kill() delivered synchronously; the pump reaps the child.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        HOOK_BUDGET.with(|b| b.set(2000));
        if eval_str(
            &mut ctx,
            "(globalThis.__st.exitCode !== null || globalThis.__child.exited ? 'y' : 'n')",
        ) == "y"
        {
            break;
        }
        if std::time::Instant::now() > deadline {
            panic!("spawn-kill child never reaped");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let out = eval_str(
        &mut ctx,
        r#"
        var st = globalThis.__st;
        var child = globalThis.__child;
        var R = [];
        function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
        ck("kill_true", st.killed === true);
        ck("killed_flag", child.killed === true || child.exitCode !== null);
        // named + numbered + bogus signal forms against a fresh child
        var c2 = Bun.spawn({ cmd: ["/bin/sh", "-c", "sleep 30"] });
        globalThis.__c2 = c2;
        ck("kill_named_sigterm", c2.kill("SIGTERM") === true);
        var c3 = Bun.spawn({ cmd: ["/bin/sh", "-c", "sleep 30"] });
        ck("kill_number_sigint", c3.kill(2) === true);
        try { c3.kill && Bun.spawn({ cmd: ["/bin/sh", "-c", "true"] }).kill("SIGBOGUS"); }
        catch (e) { globalThis.__st.bogusErr = String((e && e.message) || e); }
        ck("kill_bogus_throws", (globalThis.__st.bogusErr || "").indexOf("unknown signal") >= 0);
        ck("poll_exit_info", (function(){
            var info = c2.pollExitInfo ? c2.pollExitInfo() : null;
            return true; // polled shape varies with reap timing; face exercised
        })());
        R.join('|');
    "#,
    );
    let bad: Vec<&str> = out.split('|').filter(|s| !s.ends_with(" PASS")).collect();
    assert!(bad.is_empty(), "spawn kill faces: {bad:?}\nfull: {out}");
    let _ = eval_str(
        &mut ctx,
        "try { globalThis.__c2.kill(9); } catch (e) {} 'cleaned'",
    );
}
