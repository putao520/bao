// @trace TEST-ENG-007-CRYPTO-C2 [req:REQ-ENG-006] [level:integration]
// 行覆盖率收敛波 C2 — node_crypto.rs 算法变体与错误分支补测。
// 覆盖目标(源码逐块定位的非 subtle 面):createHash/Hmac 全算法与错误臂、
// cipher/decipher 全算法族(update 编码形态/GCM AAD+tag/错误臂)、Sign/Verify
// (HMAC + RSA + EC + Ed25519 真路径)、KeyObject 全 export 矩阵、ECDH/DH、
// randomBytes/randomInt/randomFill(Sync)/getRandomValues、pbkdf2/scrypt/hkdf
// (sync+async)、argon2 校验矩阵、checkPrime/generatePrime、Certificate(SPKAC)、
// X509Certificate、crypto.hash 一次性面。
//
// 真 oracle:标准算法向量(RFC 4231/5869/7914/FIPS-197)+ node v24 语义对照
// (本机 node 实测:fingerprint256 列分隔 95 字符、subject "CN=c2test" 形态)。

use bao_engine::context::JsContext;
#[path = "common/mod.rs"]
mod common;

use common::eval_string_full as eval_string;
use common::setup_ctx;

/// Fixed PEM fixtures (openssl-generated, RSA-1024 self-signed "CN=c2test").
const RSA_CERT_PEM: &str = "-----BEGIN CERTIFICATE-----\nMIIB/jCCAWegAwIBAgIUTXnbMr3oirDdVAe+VkCo+g5kLQ8wDQYJKoZIhvcNAQEL\nBQAwETEPMA0GA1UEAwwGYzJ0ZXN0MB4XDTI2MTAxNjE4NTMxN1oXDTM2MTAxMzE4\nNTMxN1owETEPMA0GA1UEAwwGYzJ0ZXN0MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCB\niQKBgQCZhm9D4y7+1Mcqij6S8gHxx0oKFxh6zyZR7T4AWc+nxtkSr4N0/PDYiMvb\n4qQT0Wal6Qo4Y8yTdpO7apPJIHdiOY039/IVsaqoHRtzHTRFYGHjFEHhtJAqOva7\n5VL4Guxt1P9zAZfxtQ1tesO6KVlt1evm1fhDl07YBhpWSm/qlQIDAQABo1MwUTAd\nBgNVHQ4EFgQUopiOg0mu61MxuzjW/kLIXKXrBjAwHwYDVR0jBBgwFoAUopiOg0mu\n61MxuzjW/kLIXKXrBjAwDwYDVR0TAQH/BAUwAwEB/zANBgkqhkiG9w0BAQsFAAOB\ngQBe4rnbueXhzKEskeMk6BU6HCKbnHCpW+HGwkBPKWnODArihtkQo7c6sgAvHmJy\nCSWt0hVrS9sYEM/ZPJkPKu7zUBc+UzmE+TfHfxYjRXd7DkeXh6oKPQgOTFSccP8o\n2G2gVYhkMXmY8HKKHk1feKnet6DZ+kO08uzsRJ5ad1WrBQ==\n-----END CERTIFICATE-----";
const RSA_KEY_PEM: &str = "-----BEGIN PRIVATE KEY-----\nMIICdwIBADANBgkqhkiG9w0BAQEFAASCAmEwggJdAgEAAoGBAJmGb0PjLv7UxyqK\nPpLyAfHHSgoXGHrPJlHtPgBZz6fG2RKvg3T88NiIy9vipBPRZqXpCjhjzJN2k7tq\nk8kgd2I5jTf38hWxqqgdG3MdNEVgYeMUQeG0kCo69rvlUvga7G3U/3MBl/G1DW16\nw7opWW3V6+bV+EOXTtgGGlZKb+qVAgMBAAECgYB69itjii/3tmAWeEcJ1n8sMNvi\nHQFkS0l70LIaAdTJof/2cVCugHA+XcI5SNr1bH/0XiPrajNN4GkOcJwROMmkSpyy\nEbErtCVTCXPiuyROLinOXHDqwMoh68HMeVVTL54qK7k2A3tnDFof+he2etv+r1nl\n2ZX3iGU1vOCO5DhM+QJBAMtTqq4TIzM3JOUL6Es+WRWVsRF6bFO4mZUYxR6QXj1d\nUJIKWJCWiYGTwSvKJmnDeQvJLCrWqHRD0pghlRZ3NHMCQQDBS/5tE+Bus+79YmxS\nyiE55MODfWYETJzIYQBUfX/dpmYRVzP0g9aPzZ3QCOX0ThvUSXeXUkEzdy9wzwl1\naCrXAkEAgIXoc5RhQbRvu+Ck+zrz8LLodFXgN6OOYTKBF2rGRx+/GDE1/l9JmACf\nsGBdm6kV0J9fLcz2OlfMvKEtrj+3hQJATr2vSi3jqhUAURXr/Jm8iUHoZ0MI8M1o\nvLPPF5v8H4WuZHf6gSvHltLy65EEY84BxiubXPgz+Uwe9jxQpRQnBwJBAJ+z6QDr\nY5qandnzB8o47qfL3fN24NkgVllYMOtGYYJO4DnPOGjusTk/RbBWOaottCL471j2\n2fORrcpyYGYcd3E=\n-----END PRIVATE KEY-----";
const EC_KEY_PEM: &str = "-----BEGIN EC PRIVATE KEY-----\nMHcCAQEEIJMueQ/urXLx+F1/31U6poeJgd3JXg84i1tKCt7S/N/soAoGCCqGSM49\nAwEHoUQDQgAExCvszcdwXG4lwc48FVhJ0Y6vznJJUm9Tw+DirJBxGPtnhboq0loZ\ntDWSNd0+PLQA+5eissG3MPiK6YgUmn7aYw==\n-----END EC PRIVATE KEY-----";
/// SEC1 private scalar of EC_KEY_PEM (32 bytes) — convertKey reconstruct-oracle input.
const EC_PRIV_HEX: &str = "932e790feead72f1f85d7fdf553aa6878981ddc95e0f388b5b4a0aded2fcdfec";
/// Uncompressed public point derived from EC_PRIV_HEX (65 bytes, leading 0x04).
const EC_PUB_HEX: &str = "04c42beccdc7705c6e25c1ce3c155849d18eafce7249526f53c3e0e2ac907118fb6785ba2ad25a19b4359235dd3e3cb400fb97a2b2c1b730f88ae988149a7eda63";
/// SPKI PEM of the same P-256 key pair.
const EC_PUB_PEM: &str = "-----BEGIN PUBLIC KEY-----\nMFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAExCvszcdwXG4lwc48FVhJ0Y6vznJJ\nUm9Tw+DirJBxGPtnhboq0loZtDWSNd0+PLQA+5eissG3MPiK6YgUmn7aYw==\n-----END PUBLIC KEY-----";

/// Production-shaped pump for async crypto callbacks (spawn_crypto_async
/// tasklet delivery — same discipline as timer_crypto_pump_starvation_tests).
fn pump_until_quiescent(ctx: &mut JsContext, deadline_ms: u64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(deadline_ms);
    while std::time::Instant::now() < deadline {
        let mut cxm = ctx.cx();
        if !bun_runtime::timers::drain_and_check(&mut cxm) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// Run `body` in a fresh realm and require every accumulated check to PASS.
/// The body must push `label + (ok ? " PASS" : " FAIL:" + detail)` strings.
macro_rules! crypto_case {
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

/// Substitute `%KEY%` placeholders in a JS body (PEM fixture injection — raw
/// strings cannot interpolate, and newlines must stay literal for JS).
fn subst(body: &str, pairs: &[(&str, &str)]) -> String {
    let mut out = body.to_string();
    for (k, v) in pairs {
        out = out.replace(k, v);
    }
    out
}

/// Render a multi-line PEM fixture as a complete single-quoted JS string
/// literal (newlines escaped; the JS engine unescapes them at parse time).
fn pem_js(pem: &str) -> String {
    let mut s = String::with_capacity(pem.len() + 16);
    s.push('\'');
    for ch in pem.chars() {
        match ch {
            '\\' => s.push_str("\\\\"),
            '\'' => s.push_str("\\'"),
            '\n' => s.push_str("\\n"),
            _ => s.push(ch),
        }
    }
    s.push('\'');
    s
}

// ── createHash: 全算法 + 错误臂 + 编码形态 ─────────────────────────────────

crypto_case!(c2_hash_algorithms_and_vectors, r#"
    var crypto = require('crypto');
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }

    // Canonical digest vectors — the oracle is the algorithm itself.
    ck("sha256_hello", crypto.createHash('sha256').update('hello').digest('hex') ===
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
    ck("sha1_hello", crypto.createHash('sha1').update('hello').digest('hex') ===
        "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d");
    ck("md5_hello", crypto.createHash('md5').update('hello').digest('hex') ===
        "5d41402abc4b2a76b9719d911017c592");
    ck("sha512_hello_len", crypto.createHash('sha512').update('hello').digest('hex').length === 128);
    ck("sha384_hello_len", crypto.createHash('sha384').update('hello').digest('hex').length === 96);
    ck("sha224_hello_len", crypto.createHash('sha224').update('hello').digest('hex').length === 56);
    ck("upper_algo_lowered", crypto.createHash('SHA256').update('hello').digest('hex') ===
        crypto.createHash('sha256').update('hello').digest('hex'));
    // digest encodings
    ck("digest_base64", crypto.createHash('sha256').update('hello').digest('base64') ===
        "LPJNul+wow4m6DsqxbninhsWHlwfp0JecwQzYpOLmCQ=");
    ck("digest_noarg_is_buffer", Buffer.isBuffer(crypto.createHash('sha256').update('x').digest()));
    ck("digest_buffer_len", crypto.createHash('sha256').update('x').digest().length === 32);
    ck("digest_unknown_enc_falls_hex", crypto.createHash('sha256').update('hello').digest('utf16') ===
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
    // multi-chunk == one-shot
    ck("multichunk", crypto.createHash('sha256').update('he').update('llo').digest('hex') ===
        crypto.createHash('sha256').update('hello').digest('hex'));
    // update input encodings (BUG-ENG-CIPHER-ENC face)
    var h1 = crypto.createHash('sha256'); h1.update('68656c6c6f', 'hex');
    ck("update_hex_enc", h1.digest('hex') === "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
    var h2 = crypto.createHash('sha256'); h2.update('aGVsbG8=', 'base64');
    ck("update_base64_enc", h2.digest('hex') === "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
    var h3 = crypto.createHash('sha256'); h3.update('hello', 'latin1');
    ck("update_latin1_enc", h3.digest('hex') === "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
    // typed-array / DataView / ArrayBuffer update paths
    ck("update_uint8array", crypto.createHash('sha256').update(new Uint8Array([104,101,108,108,111])).digest('hex') ===
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
    ck("update_int32array_view", typeof crypto.createHash('sha256').update(new Int32Array([104,101,108,108,111])).digest('hex') === 'string');
    var dv = new DataView(new ArrayBuffer(5));
    for (var i = 0; i < 5; i++) dv.setUint8(i, 'hello'.charCodeAt(i));
    ck("update_dataview", crypto.createHash('sha256').update(dv).digest('hex') ===
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
    ck("update_arraybuffer", crypto.createHash('sha256').update(new Uint8Array([104,101,108,108,111]).buffer).digest('hex') ===
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
    ck("update_empty_uint8array", crypto.createHash('sha256').update(new Uint8Array(0)).digest('hex') ===
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    R.join('|');
"#);

crypto_case!(c2_hash_error_branches, r#"
    var crypto = require('crypto');
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
    function thr(fn) { try { fn(); return null; } catch (e) { return String((e && e.message) || e); } }

    ck("no_args_throws", (thr(function(){ crypto.createHash(); }) || "").indexOf("algorithm name") >= 0);
    ck("non_string_throws", (thr(function(){ crypto.createHash(null); }) || "").indexOf("must be a string") >= 0);
    ck("unknown_algo_throws", (thr(function(){ crypto.createHash('sha999'); }) || "").indexOf("Digest method not supported") >= 0);
    ck("update_no_args_throws", (thr(function(){ crypto.createHash('sha256').update(); }) || "").indexOf("requires data") >= 0);
    ck("update_number_throws", (thr(function(){ crypto.createHash('sha256').update(5); }) || "").indexOf("must be a string, Buffer") >= 0);
    ck("update_plain_object_throws", (thr(function(){ crypto.createHash('sha256').update({}); }) || "").indexOf("must be a string, Buffer") >= 0);
    ck("update_after_digest_throws", (thr(function(){
        var h = crypto.createHash('sha256'); h.update('a').digest('hex'); h.update('b');
    }) || "").indexOf("Digest already called") >= 0);
    ck("digest_twice_throws", (thr(function(){
        var h = crypto.createHash('sha256'); h.update('a'); h.digest('hex'); h.digest('hex');
    }) || "").indexOf("Digest already called") >= 0);
    ck("copy_after_digest_throws", (thr(function(){
        var h = crypto.createHash('sha256'); h.update('a'); h.digest('hex'); h.copy();
    }) || "").indexOf("Digest already called") >= 0);
    ck("digest_detached_throws", (thr(function(){
        crypto.createHash('sha256').digest.call(5);
    }) || "").indexOf("receiver") >= 0);
    ck("update_detached_throws", (thr(function(){
        crypto.createHash('sha256').update.call(5, 'x');
    }) || "").indexOf("receiver") >= 0);
    // copy() independence + state snapshot
    var base = crypto.createHash('sha256').update('he');
    var cp = base.copy();
    cp.update('llo');
    ck("copy_digest_same", cp.digest('hex') === crypto.createHash('sha256').update('hello').digest('hex'));
    base.update('x');
    ck("copy_independent", base.digest('hex') === crypto.createHash('sha256').update('hex').digest('hex'));
    // class form
    var H = crypto.Hash;
    ck("hash_class_exists", typeof H === 'function');
    var inst; try { inst = new H('sha256'); } catch (e) { inst = null; }
    ck("hash_new_works", inst !== null);
    ck("hash_instanceof", inst !== null && inst instanceof crypto.Hash);
    try { ck("hash_class_digest", new H('sha256').update('hello').digest('hex') ===
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"); }
    catch (e) { ck("hash_class_digest", false); }
    R.join('|');
"#);

// ── createHmac: 全算法 + KeyObject key + 错误臂 ────────────────────────────

crypto_case!(c2_hmac_algorithms, r#"
    var crypto = require('crypto');
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
    // RFC 4231 test case 2: key "Jefe", data "what do ya want for nothing?".
    var EXPECT = "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843";
    function hmac_hex(algo, key, data) { return crypto.createHmac(algo, key).update(data).digest('hex'); }
    ck("hmac_sha256", hmac_hex('sha256', 'Jefe', 'what do ya want for nothing?') === EXPECT);
    ck("hmac_sha256_upper", hmac_hex('SHA256', 'Jefe', 'what do ya want for nothing?') === EXPECT);
    ck("hmac_sha384_len", hmac_hex('sha384', 'Jefe', 'what do ya want for nothing?').length === 96);
    ck("hmac_sha512_len", hmac_hex('sha512', 'Jefe', 'what do ya want for nothing?').length === 128);
    ck("hmac_sha224_len", hmac_hex('sha224', 'Jefe', 'what do ya want for nothing?').length === 56);
    ck("hmac_sha1_len", hmac_hex('sha1', 'Jefe', 'what do ya want for nothing?').length === 40);
    ck("hmac_md5_len", hmac_hex('md5', 'Jefe', 'what do ya want for nothing?').length === 32);
    ck("hmac_buffer_key", crypto.createHmac('sha256', Buffer.from('Jefe')).update('what do ya want for nothing?').digest('hex') === EXPECT);
    var sk = crypto.createSecretKey(Buffer.from('Jefe'));
    ck("hmac_keyobject_key", crypto.createHmac('sha256', sk).update('what do ya want for nothing?').digest('hex') === EXPECT);
    ck("hmac_multichunk", (function(){
        var h = crypto.createHmac('sha256', 'Jefe');
        h.update('what do ya '); h.update('want for nothing?');
        return h.digest('hex');
    })() === EXPECT);
    ck("hmac_digest_noarg_buffer", Buffer.isBuffer((function(){
        var h = crypto.createHmac('sha256', 'k'); h.update('d'); return h.digest();
    })()));
    ck("hmac_digest_base64", crypto.createHmac('sha256', 'Jefe').update('what do ya want for nothing?').digest('base64') ===
        "W9zBRr9gdU5qBCQmCJV1x1oAPwidJzmDnexYuWTsOEM=");
    ck("hmac_digest_unknown_enc_hex", crypto.createHmac('sha256', 'Jefe').update('what do ya want for nothing?').digest('bogus') === EXPECT);
    R.join('|');
"#);

crypto_case!(c2_hmac_error_branches, r#"
    var crypto = require('crypto');
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
    function thr(fn) { try { fn(); return null; } catch (e) { return String((e && e.message) || e); } }

    ck("no_args_throws", (thr(function(){ crypto.createHmac(); }) || "").indexOf("algorithm and key") >= 0);
    ck("one_arg_throws", (thr(function(){ crypto.createHmac('sha256'); }) || "").indexOf("algorithm and key") >= 0);
    ck("algo_non_string_throws", (thr(function(){ crypto.createHmac(null, 'k'); }) || "").indexOf("must be a string") >= 0);
    ck("unknown_algo_throws", (thr(function(){ crypto.createHmac('sha999', 'k'); }) || "").indexOf("Digest method not supported") >= 0);
    ck("number_key_throws", (thr(function(){ crypto.createHmac('sha256', 7); }) || "").indexOf("key must be a string") >= 0);
    ck("empty_buffer_key_throws", (thr(function(){ crypto.createHmac('sha256', Buffer.alloc(0)); }) || "").indexOf("key must be a string") >= 0);
    ck("public_keyobject_throws", (function(){
        try {
            var kp = crypto.generateKeyPairSync('ed25519');
            crypto.createHmac('sha256', kp.publicKey);
            return false;
        } catch (e) { return String((e && e.message) || e).indexOf("secret key") >= 0; }
    })());
    ck("update_no_args_throws", (thr(function(){ crypto.createHmac('sha256', 'k').update(); }) || "").indexOf("requires data") >= 0);
    ck("update_number_throws", (thr(function(){ crypto.createHmac('sha256', 'k').update(3); }) || "").indexOf("must be a string") >= 0);
    ck("update_after_digest_throws", (thr(function(){
        var h = crypto.createHmac('sha256', 'k'); h.update('a'); h.digest('hex'); h.update('b');
    }) || "").indexOf("Digest already called") >= 0);
    ck("digest_twice_throws", (thr(function(){
        var h = crypto.createHmac('sha256', 'k'); h.update('a'); h.digest('hex'); h.digest();
    }) || "").indexOf("Digest already called") >= 0);
    ck("digest_detached_throws", (thr(function(){
        crypto.createHmac('sha256', 'k').digest.call(3);
    }) || "").indexOf("receiver") >= 0);
    R.join('|');
"#);

// ── randomBytes / randomUUID / getRandomValues / randomInt / randomFill ────

crypto_case!(c2_random_bytes_faces, r#"
    var crypto = require('crypto');
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
    function thr(fn) { try { fn(); return null; } catch (e) { return String((e && e.message) || e); } }

    var b = crypto.randomBytes(16);
    ck("sync_is_buffer", Buffer.isBuffer(b));
    ck("sync_len", b.length === 16);
    var b0 = crypto.randomBytes(0);
    ck("zero_size_ok", Buffer.isBuffer(b0) && b0.length === 0);
    ck("double_size_ok", crypto.randomBytes(8.0).length === 8);
    ck("two_runs_differ", (function(){
        var x = crypto.randomBytes(32), y = crypto.randomBytes(32);
        return x.toString('hex') !== y.toString('hex');
    })());
    ck("no_args_throws", (thr(function(){ crypto.randomBytes(); }) || "").indexOf("requires a size") >= 0);
    ck("string_size_throws", (thr(function(){ crypto.randomBytes('4'); }) || "").indexOf("must be a number") >= 0);
    ck("nan_size_throws", (thr(function(){ crypto.randomBytes(NaN); }) || "").indexOf("must be a number") >= 0);
    ck("negative_throws", (thr(function(){ crypto.randomBytes(-1); }) || "").indexOf("between 0 and") >= 0);
    ck("oversize_throws", (thr(function(){ crypto.randomBytes(2147483648); }) || "").indexOf("between 0 and") >= 0);
    ck("cb_non_function_throws", (thr(function(){ crypto.randomBytes(4, 5); }) || "").indexOf("callback must be a function") >= 0);
    // randomUUID
    var u = crypto.randomUUID();
    var re = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
    ck("uuid_v4_shape", re.test(u));
    ck("uuid_distinct", crypto.randomUUID() !== crypto.randomUUID());
    // getRandomValues
    var arr = new Uint8Array(8);
    var ret = crypto.getRandomValues(arr);
    ck("grv_returns_same", ret === arr);
    var allzero = true; for (var i = 0; i < 8; i++) if (arr[i] !== 0) allzero = false;
    ck("grv_filled", !allzero);
    var plain = new Array(4); plain[0]=0; plain[1]=0; plain[2]=0; plain[3]=0;
    crypto.getRandomValues(plain);
    ck("grv_plain_array", Array.isArray(plain) && plain.length === 4);
    ck("grv_no_args_throws", (thr(function(){ crypto.getRandomValues(); }) || "").indexOf("typed array") >= 0);
    ck("grv_bad_array_throws", (thr(function(){ crypto.getRandomValues({}); }) || "").indexOf("invalid array") >= 0);
    // randomInt
    var ri = crypto.randomInt(10);
    ck("ri_one_arg_range", ri >= 0 && ri < 10 && (ri | 0) === ri);
    var ri2 = crypto.randomInt(5, 10);
    ck("ri_two_arg_range", ri2 >= 5 && ri2 < 10);
    ck("ri_non_num_throws", (thr(function(){ crypto.randomInt('x'); }) || "").indexOf("must be a number") >= 0);
    ck("ri_min_max_equal_throws", (thr(function(){ crypto.randomInt(5, 5); }) || "").indexOf("less than max") >= 0);
    ck("ri_no_args_throws", (thr(function(){ crypto.randomInt(); }) || "").indexOf("at least one argument") >= 0);
    ck("ri_min_non_num_throws", (thr(function(){ crypto.randomInt({}, 5); }) || "").indexOf("min must be a number") >= 0);
    ck("ri_max_non_num_throws", (thr(function(){ crypto.randomInt(0, {}); }) || "").indexOf("max must be a number") >= 0);
    // randomFillSync
    var fill = Buffer.alloc(10);
    var fret = crypto.randomFillSync(fill);
    ck("rfs_returns_buffer", fret === fill);
    var anyset = false; for (var i = 0; i < 10; i++) if (fill[i] !== 0) anyset = true;
    ck("rfs_filled", anyset);
    var fill2 = Buffer.alloc(10);
    crypto.randomFillSync(fill2, 2, 4);
    var head_zero = true; for (var i = 0; i < 2; i++) if (fill2[i] !== 0) head_zero = false;
    ck("rfs_offset_size", head_zero);
    var fill3 = Buffer.alloc(6);
    ck("rfs_overflow_throws", (thr(function(){ crypto.randomFillSync(fill3, 4, 8); }) || "").indexOf("exceeds buffer length") >= 0);
    ck("rfs_no_args_throws", (thr(function(){ crypto.randomFillSync(); }) || "").indexOf("requires a buffer") >= 0);
    ck("rfs_non_ta_throws", (thr(function(){ crypto.randomFillSync({length: 4}); }) || "").indexOf("TypedArray") >= 0);
    R.join('|');
"#);

#[test]
fn c2_random_bytes_callback_form() {
    let mut ctx = setup_ctx();
    let out = eval_string(
        &mut ctx,
        r#"
        var crypto = require('crypto');
        globalThis.__rb = { done: false, hex: null, err: null };
        crypto.randomBytes(24, function(err, buf) {
            globalThis.__rb.done = true;
            if (err) { globalThis.__rb.err = String(err.message || err); }
            else { globalThis.__rb.hex = buf.toString('hex'); }
        });
        'scheduled'
    "#,
    );
    assert_eq!(out, "scheduled");
    pump_until_quiescent(&mut ctx, 10_000);
    let verdict = eval_string(
        &mut ctx,
        r#"
        var v = globalThis.__rb;
        (v.done && v.err === null && typeof v.hex === 'string' && v.hex.length === 48) ? "PASS" : "FAIL:" + JSON.stringify(v)
    "#,
    );
    assert_eq!(verdict, "PASS", "randomBytes callback: {verdict}");
}

#[test]
fn c2_random_fill_callback_form() {
    let mut ctx = setup_ctx();
    let out = eval_string(
        &mut ctx,
        r#"
        var crypto = require('crypto');
        globalThis.__rf = { done: false, len: -1, err: null };
        crypto.randomFill(Buffer.alloc(12), function(err, buf) {
            globalThis.__rf.done = true;
            if (err) { globalThis.__rf.err = String(err.message || err); }
            else { globalThis.__rf.len = buf.length; }
        });
        'scheduled'
    "#,
    );
    assert_eq!(out, "scheduled");
    pump_until_quiescent(&mut ctx, 10_000);
    let verdict = eval_string(
        &mut ctx,
        r#"
        var v = globalThis.__rf;
        (v.done && v.err === null && v.len === 12) ? "PASS" : "FAIL:" + JSON.stringify(v)
    "#,
    );
    assert_eq!(verdict, "PASS", "randomFill callback: {verdict}");
}

// ── pbkdf2 / scrypt / hkdf / argon2 ────────────────────────────────────────

crypto_case!(c2_pbkdf2_sync_faces, r#"
    var crypto = require('crypto');
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
    function thr(fn) { try { fn(); return null; } catch (e) { return String((e && e.message) || e); } }

    // RFC 7914 §11 PBKDF2-HMAC-SHA256 vectors.
    ck("pbkdf2_c1", crypto.pbkdf2Sync('password', 'salt', 1, 32, 'sha256').toString('hex') ===
        "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b");
    ck("pbkdf2_c2", crypto.pbkdf2Sync('password', 'salt', 2, 32, 'sha256').toString('hex') ===
        "ae4d0c95af6b46d32d0adff928f06dd02a303f8ef3c251dfd6e2d85a95474c43");
    ck("pbkdf2_is_buffer", Buffer.isBuffer(crypto.pbkdf2Sync('p', 's', 1, 16, 'sha256')));
    ck("pbkdf2_sha1_ok", crypto.pbkdf2Sync('password', 'salt', 1, 20, 'sha1').length === 20);
    ck("pbkdf2_sha512_ok", crypto.pbkdf2Sync('password', 'salt', 1, 64, 'sha512').length === 64);
    ck("pbkdf2_upper_digest", crypto.pbkdf2Sync('password', 'salt', 1, 32, 'SHA256').toString('hex') ===
        "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b");
    ck("bad_digest_throws", (thr(function(){ crypto.pbkdf2Sync('p','s',1,32,'md6'); }) || "").indexOf("Unsupported PBKDF2 digest") >= 0);
    ck("missing_args_throws", (thr(function(){ crypto.pbkdf2Sync('p','s',1,32); }) || "").indexOf("requires") >= 0);
    ck("pw_non_string_throws", (thr(function(){ crypto.pbkdf2Sync(null,'s',1,32,'sha256'); }) || "").indexOf("password must be a string") >= 0);
    ck("salt_non_string_throws", (thr(function(){ crypto.pbkdf2Sync('p',null,1,32,'sha256'); }) || "").indexOf("salt must be a string") >= 0);
    ck("iter_non_num_throws", (thr(function(){ crypto.pbkdf2Sync('p','s','x',32,'sha256'); }) || "").indexOf("iterations must be a number") >= 0);
    ck("keylen_non_num_throws", (thr(function(){ crypto.pbkdf2Sync('p','s',1,null,'sha256'); }) || "").indexOf("keylen must be a number") >= 0);
    ck("digest_non_string_throws", (thr(function(){ crypto.pbkdf2Sync('p','s',1,32,null); }) || "").indexOf("digest must be a string") >= 0);
    R.join('|');
"#);

#[test]
fn c2_pbkdf2_async_faces() {
    let mut ctx = setup_ctx();
    let out = eval_string(
        &mut ctx,
        r#"
        var crypto = require('crypto');
        globalThis.__pk = { done: false, hex: null, err: null, err2: null };
        crypto.pbkdf2('password', 'salt', 1, 32, 'sha256', function(err, key) {
            globalThis.__pk.done = true;
            if (err) { globalThis.__pk.err = String(err.message || err); }
            else { globalThis.__pk.hex = key.toString('hex'); }
        });
        crypto.pbkdf2('password', 'salt', 1, 32, 'md6', function(err) {
            globalThis.__pk.err2 = err ? String(err.message || err) : null;
        });
        'scheduled'
    "#,
    );
    assert_eq!(out, "scheduled");
    pump_until_quiescent(&mut ctx, 15_000);
    let verdict = eval_string(
        &mut ctx,
        r#"
        var v = globalThis.__pk;
        (v.done && v.err === null &&
         v.hex === "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b" &&
         v.err2 !== null && v.err2.indexOf("md6") >= 0) ? "PASS" : "FAIL:" + JSON.stringify(v)
    "#,
    );
    assert_eq!(verdict, "PASS", "pbkdf2 async: {verdict}");
}

crypto_case!(c2_scrypt_faces, r#"
    var crypto = require('crypto');
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
    function thr(fn) { try { fn(); return null; } catch (e) { return String((e && e.message) || e); } }

    // RFC 7914 §12 scrypt test vector (N=1024, r=8, p=16).
    var v = crypto.scryptSync('password', 'NaCl', 64, { N: 1024, r: 8, p: 16 });
    ck("rfc7914_vector", v.toString('hex') ===
        "fdbabe1c9d3472007856e7190d01e9fe7c6ad7cbc8237830e77376634b3731622eaf30d92e22a3886ff109279d9830dac727afb94a83ee6d8360cbdfa2cc0640");
    ck("is_buffer", Buffer.isBuffer(v));
    // Alias option names hit the cost/blocksize/parallelization fallback arms.
    var v2 = crypto.scryptSync('password', 'NaCl', 64, { cost: 1024, blocksize: 8, parallelization: 16 });
    ck("alias_options", v2.toString('hex') === v.toString('hex'));
    ck("buffer_password", Buffer.isBuffer(crypto.scryptSync(Buffer.from('password'), Buffer.from('NaCl'), 32, { N: 16, r: 8, p: 1 })));
    ck("missing_args_throws", (thr(function(){ crypto.scryptSync('p','s'); }) || "").indexOf("requires") >= 0);
    ck("keylen_zero_throws", (thr(function(){ crypto.scryptSync('p','s',0,{N:16,r:2,p:1}); }) || "").indexOf("keylen must be > 0") >= 0);
    ck("keylen_neg_double_throws", (thr(function(){ crypto.scryptSync('p','s',-4,{N:16,r:2,p:1}); }) || "").indexOf("keylen must be > 0") >= 0);
    ck("keylen_non_num_throws", (thr(function(){ crypto.scryptSync('p','s','x'); }) || "").indexOf("keylen must be a number") >= 0);
    ck("pw_bad_type_throws", (thr(function(){ crypto.scryptSync(5,'s',8,{N:16,r:2,p:1}); }) || "").indexOf("password must be") >= 0);
    ck("salt_bad_type_throws", (thr(function(){ crypto.scryptSync('p',5,8,{N:16,r:2,p:1}); }) || "").indexOf("salt must be") >= 0);
    R.join('|');
"#);

#[test]
fn c2_scrypt_async_faces() {
    let mut ctx = setup_ctx();
    let out = eval_string(
        &mut ctx,
        r#"
        var crypto = require('crypto');
        globalThis.__sc = { done: false, hex: null };
        crypto.scrypt('password', 'NaCl', 64, { N: 16, r: 8, p: 1 }, function(err, key) {
            globalThis.__sc.done = true;
            if (!err) { globalThis.__sc.hex = key.toString('hex'); }
        });
        'scheduled'
    "#,
    );
    assert_eq!(out, "scheduled");
    pump_until_quiescent(&mut ctx, 20_000);
    let verdict = eval_string(
        &mut ctx,
        r#"
        var v = globalThis.__sc;
        (v.done && typeof v.hex === 'string' && v.hex.length === 128) ? "PASS" : "FAIL:" + JSON.stringify(v)
    "#,
    );
    assert_eq!(verdict, "PASS", "scrypt async: {verdict}");
}

crypto_case!(c2_hkdf_sync_faces, r#"
    var crypto = require('crypto');
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
    function thr(fn) { try { fn(); return null; } catch (e) { return String((e && e.message) || e); } }

    function u8(hex) { var a = []; for (var i = 0; i < hex.length; i += 2) a.push(parseInt(hex.substr(i, 2), 16)); return new Uint8Array(a); }
    // REGISTERED DEFECT C2-FINDING-HKDF-SALT-IKM-SWAP (not pinned green here):
    // bao_crypto::kdf::hkdf feeds its (salt, ikm) params into BoringSSL
    // HKDF's (secret=IKM, salt) positions — BoringSSL orders inputs
    // differently from RFC 5869 and its header says so — so RFC 5869 TC1
    // derives 4fef808a… instead of 3cb25f25… (node v24 + python reference
    // agree on 3cb25f25…). Vector assertion therefore lives in the report,
    // not here; the happy path is pinned for shape/determinism/length only.
    var okm = crypto.hkdfSync('sha256', u8('0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b'),
        u8('000102030405060708090a0b0c'), u8('f0f1f2f3f4f5f6f7f8f9'), 42);
    ck("hkdf_len42", Buffer.from(okm).length === 42);
    ck("hkdf_deterministic", (function(){
        var again = crypto.hkdfSync('sha256', u8('0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b'),
            u8('000102030405060708090a0b0c'), u8('f0f1f2f3f4f5f6f7f8f9'), 42);
        return Buffer.from(again).toString('hex') === Buffer.from(okm).toString('hex');
    })());
    ck("hkdf_info_sensitive", (function(){
        var other = crypto.hkdfSync('sha256', u8('0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b'),
            u8('000102030405060708090a0b0c'), u8('f0f1f2f3f4f5f6f7f8f8'), 42);
        return Buffer.from(other).toString('hex') !== Buffer.from(okm).toString('hex');
    })());
    ck("hkdf_is_u8", Buffer.from(okm) instanceof Buffer);
    // SHA-1 arm derivable (length parity with node for L=20).
    var okm1 = crypto.hkdfSync('sha1', Buffer.alloc(22, 0x0b), Buffer.alloc(0), Buffer.alloc(0), 20);
    ck("hkdf_sha1_len20", Buffer.from(okm1).length === 20);
    ck("bad_digest_throws", (thr(function(){ crypto.hkdfSync('md6', Buffer.alloc(4), Buffer.alloc(0), Buffer.alloc(0), 8); }) || "").indexOf("Unsupported HKDF digest") >= 0);
    ck("missing_args_throws", (thr(function(){ crypto.hkdfSync('sha256', Buffer.alloc(4), Buffer.alloc(0), Buffer.alloc(0)); }) || "").indexOf("requires") >= 0);
    ck("len_non_num_throws", (thr(function(){ crypto.hkdfSync('sha256', Buffer.alloc(4), Buffer.alloc(0), Buffer.alloc(0), 'x'); }) || "").indexOf("length must be a number") >= 0);
    R.join('|');
"#);

#[test]
fn c2_hkdf_async_faces() {
    let mut ctx = setup_ctx();
    let out = eval_string(
        &mut ctx,
        r#"
        var crypto = require('crypto');
        globalThis.__hk = { done: false, hex: null };
        crypto.hkdf('sha256', Buffer.from('secret'), Buffer.from('salt'), Buffer.from('info'), 16, function(err, key) {
            globalThis.__hk.done = true;
            if (!err) { globalThis.__hk.hex = Buffer.from(key).toString('hex'); }
        });
        'scheduled'
    "#,
    );
    assert_eq!(out, "scheduled");
    pump_until_quiescent(&mut ctx, 15_000);
    let verdict = eval_string(
        &mut ctx,
        r#"
        var v = globalThis.__hk;
        (v.done && typeof v.hex === 'string' && v.hex.length === 32) ? "PASS" : "FAIL:" + JSON.stringify(v)
    "#,
    );
    assert_eq!(verdict, "PASS", "hkdf async: {verdict}");
}

crypto_case!(c2_argon2_validation_matrix, r#"
    var crypto = require('crypto');
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
    function thr(fn) { try { fn(); return null; } catch (e) { return String((e && e.message) || e); } }
    function code(fn) { try { fn(); return null; } catch (e) { return (e && e.code) || null; } }

    // Happy path: minimal in-range parameters, sync.
    var tag = crypto.argon2Sync('argon2id', {
        message: 'msg', nonce: Buffer.alloc(16, 1),
        parallelism: 1, tagLength: 32, memory: 32, passes: 1
    });
    ck("sync_happy_buffer", Buffer.isBuffer(tag));
    ck("sync_happy_len", tag.length === 32);
    ck("sync_deterministic", (function(){
        var t2 = crypto.argon2Sync('argon2id', {
            message: 'msg', nonce: Buffer.alloc(16, 1),
            parallelism: 1, tagLength: 32, memory: 32, passes: 1
        });
        return t2.toString('hex') === tag.toString('hex');
    })());
    ck("argon2i_ok", Buffer.isBuffer(crypto.argon2Sync('argon2i', {
        message: 'm', nonce: Buffer.alloc(16, 2), parallelism: 1, tagLength: 16, memory: 32, passes: 1 })));
    ck("argon2d_ok", Buffer.isBuffer(crypto.argon2Sync('argon2d', {
        message: 'm', nonce: Buffer.alloc(16, 3), parallelism: 2, tagLength: 16, memory: 64, passes: 2 })));
    ck("secret_and_ad_ok", Buffer.isBuffer(crypto.argon2Sync('argon2id', {
        message: 'm', nonce: Buffer.alloc(16, 4), parallelism: 1, tagLength: 16, memory: 32, passes: 1,
        secret: 's3cret', associatedData: 'ad' })));

    // Error matrix (node lib/internal/crypto/argon2.js shapes).
    ck("bad_algo_code", code(function(){ crypto.argon2Sync('argon2x', {}); }) === "ERR_INVALID_ARG_VALUE");
    ck("algo_non_string_code", code(function(){ crypto.argon2Sync(5, {}); }) === "ERR_INVALID_ARG_TYPE");
    ck("params_non_object_code", code(function(){ crypto.argon2Sync('argon2id', 'x'); }) === "ERR_INVALID_ARG_TYPE");
    ck("params_array_code", code(function(){ crypto.argon2Sync('argon2id', []); }) === "ERR_INVALID_ARG_TYPE");
    ck("message_bad_type_code", code(function(){ crypto.argon2Sync('argon2id', { message: 5, nonce: Buffer.alloc(16,1), parallelism: 1, tagLength: 32, memory: 32, passes: 1 }); }) === "ERR_INVALID_ARG_TYPE");
    ck("nonce_short_code", code(function(){ crypto.argon2Sync('argon2id', { message: 'm', nonce: Buffer.alloc(4), parallelism: 1, tagLength: 32, memory: 32, passes: 1 }); }) === "ERR_OUT_OF_RANGE");
    ck("parallelism_zero_code", code(function(){ crypto.argon2Sync('argon2id', { message: 'm', nonce: Buffer.alloc(16), parallelism: 0, tagLength: 32, memory: 32, passes: 1 }); }) === "ERR_OUT_OF_RANGE");
    ck("taglen_small_code", code(function(){ crypto.argon2Sync('argon2id', { message: 'm', nonce: Buffer.alloc(16), parallelism: 1, tagLength: 3, memory: 32, passes: 1 }); }) === "ERR_OUT_OF_RANGE");
    ck("memory_lt_8p_code", code(function(){ crypto.argon2Sync('argon2id', { message: 'm', nonce: Buffer.alloc(16), parallelism: 4, tagLength: 32, memory: 8, passes: 1 }); }) === "ERR_OUT_OF_RANGE");
    ck("passes_zero_code", code(function(){ crypto.argon2Sync('argon2id', { message: 'm', nonce: Buffer.alloc(16), parallelism: 1, tagLength: 32, memory: 32, passes: 0 }); }) === "ERR_OUT_OF_RANGE");
    ck("scalar_fractional_code", code(function(){ crypto.argon2Sync('argon2id', { message: 'm', nonce: Buffer.alloc(16), parallelism: 1.5, tagLength: 32, memory: 32, passes: 1 }); }) === "ERR_OUT_OF_RANGE");
    R.join('|');
"#);

#[test]
fn c2_argon2_async_faces() {
    let mut ctx = setup_ctx();
    let out = eval_string(
        &mut ctx,
        r#"
        var crypto = require('crypto');
        globalThis.__ag = { done: false, len: -1, cberr: null };
        crypto.argon2('argon2id', {
            message: 'msg', nonce: Buffer.alloc(16, 9),
            parallelism: 1, tagLength: 32, memory: 32, passes: 1
        }, function(err, tag) {
            globalThis.__ag.done = true;
            if (err) { globalThis.__ag.cberr = String(err.message || err); }
            else { globalThis.__ag.len = tag.length; }
        });
        'scheduled'
    "#,
    );
    assert_eq!(out, "scheduled");
    pump_until_quiescent(&mut ctx, 20_000);
    let verdict = eval_string(
        &mut ctx,
        r#"
        var v = globalThis.__ag;
        (v.done && v.cberr === null && v.len === 32) ? "PASS" : "FAIL:" + JSON.stringify(v)
    "#,
    );
    assert_eq!(verdict, "PASS", "argon2 async: {verdict}");
}

crypto_case!(c2_argon2_callback_invalid, r#"
    var crypto = require('crypto');
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
    function code(fn) { try { fn(); return null; } catch (e) { return (e && e.code) || null; } }
    // callback validated AFTER parameters (node order)
    ck("cb_non_function_code", code(function(){
        crypto.argon2('argon2id', { message: 'm', nonce: Buffer.alloc(16), parallelism: 1, tagLength: 32, memory: 32, passes: 1 }, 5);
    }) === "ERR_INVALID_ARG_TYPE");
    ck("cb_missing_code", code(function(){
        crypto.argon2('argon2id', { message: 'm', nonce: Buffer.alloc(16), parallelism: 1, tagLength: 32, memory: 32, passes: 1 });
    }) === "ERR_INVALID_ARG_TYPE");
    // parameters validated BEFORE the callback (this must be a params error, not a callback error)
    ck("params_checked_first", code(function(){
        crypto.argon2('argon2x', {}, function() {});
    }) === "ERR_INVALID_ARG_VALUE");
    R.join('|');
"#);

// ── cipher / decipher ──────────────────────────────────────────────────────

crypto_case!(c2_cipher_block_roundtrips, r#"
    var crypto = require('crypto');
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
    function thr(fn) { try { fn(); return null; } catch (e) { return String((e && e.message) || e); } }

    // NOTE: AES-ECB is registered in getCiphers() but rejected by
    // parse_algorithm ("Unsupported cipher: aes-128-ecb") — node supports it.
    // Deviation registered as C2-FINDING-CIPHER-ECB (test-only wave, no fix).
    // Block-mode coverage here: CBC/CTR/3DES.

    // CBC multi-block with padding, string update + encodings.
    var k16 = Buffer.alloc(16, 3); var iv = Buffer.alloc(16, 4);
    var ce = crypto.createCipheriv('aes-128-cbc', k16, iv);
    var ct = Buffer.concat([ce.update('attack at dawn!!'), ce.final()]);
    ck("cbc_ct_len_mod16", ct.length > 0 && ct.length % 16 === 0);
    var cd = crypto.createDecipheriv('aes-128-cbc', k16, iv);
    var pt2 = Buffer.concat([cd.update(ct), cd.final()]).toString('utf8');
    ck("cbc_roundtrip", pt2 === 'attack at dawn!!');

    // CTR mode: arbitrary-length ciphertext.
    var ce3 = crypto.createCipheriv('aes-256-ctr', Buffer.alloc(32, 7), Buffer.alloc(16, 8));
    var ct3 = Buffer.concat([ce3.update('ctr mode no padding'), ce3.final()]);
    ck("ctr_len_preserved", ct3.length === 'ctr mode no padding'.length);
    var cd3 = crypto.createDecipheriv('aes-256-ctr', Buffer.alloc(32, 7), Buffer.alloc(16, 8));
    ck("ctr_roundtrip", Buffer.concat([cd3.update(ct3), cd3.final()]).toString('utf8') === 'ctr mode no padding');

    // des-ede3-cbc (key 24, iv 8).
    var ce4 = crypto.createCipheriv('des-ede3-cbc', Buffer.alloc(24, 9), Buffer.alloc(8, 1));
    var ct4 = Buffer.concat([ce4.update('3des!'), ce4.final()]);
    var cd4 = crypto.createDecipheriv('des-ede3-cbc', Buffer.alloc(24, 9), Buffer.alloc(8, 1));
    ck("des3_roundtrip", Buffer.concat([cd4.update(ct4), cd4.final()]).toString('utf8') === '3des!');

    // update(data, inputEnc, outputEnc) and final(outputEnc) string forms.
    var ce5 = crypto.createCipheriv('aes-128-cbc', k16, iv);
    var hexOut = ce5.update('68656c6c6f', 'hex', 'hex') + ce5.final('hex');
    var cd5 = crypto.createDecipheriv('aes-128-cbc', k16, iv);
    var txt = cd5.update(hexOut, 'hex', 'utf8') + cd5.final('utf8');
    ck("update_enc_forms", txt === 'hello');
    // update(Buffer, outputEncoding) two-arg form.
    var ce6 = crypto.createCipheriv('aes-128-cbc', k16, iv);
    var s6 = ce6.update(Buffer.from('two-arg form'), 'base64') + ce6.final('base64');
    ck("update_buf_outenc", typeof s6 === 'string' && s6.length > 0);
    R.join('|');
"#);

crypto_case!(c2_cipher_aead_faces, r#"
    var crypto = require('crypto');
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
    function thr(fn) { try { fn(); return null; } catch (e) { return String((e && e.message) || e); } }

    // GCM with AAD: tag roundtrip.
    var key = Buffer.alloc(32, 5); var iv = Buffer.alloc(12, 6);
    var ce = crypto.createCipheriv('aes-256-gcm', key, iv);
    ce.setAAD(Buffer.from('header'));
    var ct = Buffer.concat([ce.update('gcm payload'), ce.final()]);
    var tag = ce.getAuthTag();
    // NOTE: getAuthTag returns a plain Array (bytes_to_js_array), not a
    // Buffer — node returns Buffer. Shape deviation registered as
    // C2-FINDING-AUTHTAG-ARRAY; content length is pinned here.
    ck("tag_len16", tag && tag.length === 16 && typeof tag[0] === 'number');
    var cd = crypto.createDecipheriv('aes-256-gcm', key, iv);
    cd.setAuthTag(tag);
    cd.setAAD(Buffer.from('header'));
    ck("gcm_roundtrip", Buffer.concat([cd.update(ct), cd.final()]).toString('utf8') === 'gcm payload');

    // Tampered ciphertext → final() throws.
    var cd2 = crypto.createDecipheriv('aes-256-gcm', key, iv);
    cd2.setAuthTag(tag);
    var tampered = Buffer.from(ct); tampered[0] ^= 0xff;
    ck("gcm_tamper_throws", (thr(function(){ Buffer.concat([cd2.update(tampered), cd2.final()]); }) || "").length > 0);

    // Wrong tag length → setAuthTag error.
    var cd3 = crypto.createDecipheriv('aes-256-gcm', key, iv);
    ck("settag_bad_len_throws", (thr(function(){ cd3.setAuthTag(Buffer.alloc(4)); }) || "").indexOf("setAuthTag") >= 0);

    // ChaCha20-Poly1305 roundtrip.
    var cc = crypto.createCipheriv('chacha20-poly1305', Buffer.alloc(32, 1), Buffer.alloc(12, 2));
    var ct2 = Buffer.concat([cc.update('chacha secret'), cc.final()]);
    var tag2 = cc.getAuthTag();
    var cd4 = crypto.createDecipheriv('chacha20-poly1305', Buffer.alloc(32, 1), Buffer.alloc(12, 2));
    cd4.setAuthTag(tag2);
    ck("chacha_roundtrip", Buffer.concat([cd4.update(ct2), cd4.final()]).toString('utf8') === 'chacha secret');

    // aes-192-gcm variant arm.
    var ce5 = crypto.createCipheriv('aes-192-gcm', Buffer.alloc(24, 8), Buffer.alloc(12, 9));
    var ct5 = Buffer.concat([ce5.update('a192'), ce5.final()]);
    var cd5 = crypto.createDecipheriv('aes-192-gcm', Buffer.alloc(24, 8), Buffer.alloc(12, 9));
    cd5.setAuthTag(ce5.getAuthTag());
    ck("aes192gcm_roundtrip", Buffer.concat([cd5.update(ct5), cd5.final()]).toString('utf8') === 'a192');

    // Error arms.
    ck("too_few_args_throws", (thr(function(){ crypto.createCipheriv('aes-128-cbc'); }) || "").indexOf("requires") >= 0);
    ck("algo_non_string_throws", (thr(function(){ crypto.createCipheriv(null, Buffer.alloc(16), Buffer.alloc(16)); }) || "").indexOf("algorithm must be a string") >= 0);
    ck("unsupported_throws", (thr(function(){ crypto.createCipheriv('aes-999-cbc', Buffer.alloc(16), Buffer.alloc(16)); }) || "").indexOf("Unsupported cipher") >= 0);
    ck("bad_key_len_throws", (thr(function(){ crypto.createCipheriv('aes-128-cbc', Buffer.alloc(15), Buffer.alloc(16)); }) || "").indexOf("init failed") >= 0);
    ck("bad_iv_len_throws", (thr(function(){ crypto.createCipheriv('aes-128-cbc', Buffer.alloc(16), Buffer.alloc(5)); }) || "").indexOf("init failed") >= 0);
    ck("decipher_unsupported_throws", (thr(function(){ crypto.createDecipheriv('nope', Buffer.alloc(16), Buffer.alloc(16)); }) || "").indexOf("Unsupported cipher") >= 0);
    ck("double_final_throws", (thr(function(){
        var c7 = crypto.createCipheriv('aes-128-cbc', Buffer.alloc(16), Buffer.alloc(16));
        c7.update('x'); c7.final(); c7.final();
    }) || "").indexOf("final() failed") >= 0);
    ck("plain_object_receiver_throws", (thr(function(){
        crypto.createCipheriv('aes-128-cbc', Buffer.alloc(16), Buffer.alloc(16)).update.call({}, 'x');
    }) || "").indexOf("invalid receiver") >= 0);
    ck("gettag_on_plain_throws", (thr(function(){
        crypto.createCipheriv('aes-128-cbc', Buffer.alloc(16), Buffer.alloc(16)).getAuthTag.call({});
    }) || "").indexOf("invalid receiver") >= 0);
    ck("settag_no_args_throws", (thr(function(){
        var c8 = crypto.createCipheriv('aes-128-gcm', Buffer.alloc(16), Buffer.alloc(12));
        c8.setAuthTag();
    }) || "").indexOf("requires a tag") >= 0);
    ck("setaad_no_args_throws", (thr(function(){
        var c9 = crypto.createCipheriv('aes-128-gcm', Buffer.alloc(16), Buffer.alloc(12));
        c9.setAAD();
    }) || "").indexOf("requires data") >= 0);
    R.join('|');
"#);

// ── timingSafeEqual / getHashes / getCiphers / getCurves / getCipherInfo ───

crypto_case!(c2_info_and_compare_faces, r#"
    var crypto = require('crypto');
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
    function thr(fn) { try { fn(); return null; } catch (e) { return String((e && e.message) || e); } }

    ck("tse_true", crypto.timingSafeEqual(Buffer.from('abc'), Buffer.from('abc')) === true);
    ck("tse_false", crypto.timingSafeEqual(Buffer.from('abc'), Buffer.from('abd')) === false);
    ck("tse_len_mismatch_throws", (thr(function(){ crypto.timingSafeEqual(Buffer.from('a'), Buffer.from('bb')); }) || "").indexOf("same length") >= 0);
    ck("tse_one_arg_throws", (thr(function(){ crypto.timingSafeEqual(Buffer.from('a')); }) || "").indexOf("two buffer arguments") >= 0);

    var hashes = crypto.getHashes();
    ck("hashes_array", Array.isArray(hashes));
    ck("hashes_has_sha256", hashes.indexOf('sha256') >= 0);
    var ciphers = crypto.getCiphers();
    ck("ciphers_array", Array.isArray(ciphers));
    ck("ciphers_has_aes128cbc", ciphers.indexOf('aes-128-cbc') >= 0);
    var curves = crypto.getCurves();
    ck("curves_array", Array.isArray(curves) && curves.length > 0);
    ck("curves_has_p256", curves.indexOf('P-256') >= 0 || curves.indexOf('prime256v1') >= 0);

    var info = crypto.getCipherInfo('aes-128-cbc');
    ck("info_aes128cbc", info && info.keyLength === 16 && info.ivLength === 16 && info.mode === 'cbc' && info.blockSize === 16 && info.name === 'aes-128-cbc');
    var info3des = crypto.getCipherInfo('des-ede3-cbc');
    ck("info_3des", info3des && info3des.keyLength === 24 && info3des.ivLength === 8 && info3des.blockSize === 8);
    var infoCha = crypto.getCipherInfo('chacha20-poly1305');
    ck("info_chacha", infoCha && infoCha.keyLength === 32 && infoCha.ivLength === 12);
    // Remaining CipherAlgorithm match arms (key/iv lengths per node table).
    var i192c = crypto.getCipherInfo('aes-192-cbc');
    ck("info_aes192cbc", i192c && i192c.keyLength === 24 && i192c.ivLength === 16 && i192c.mode === 'cbc');
    var i256c = crypto.getCipherInfo('aes-256-cbc');
    ck("info_aes256cbc", i256c && i256c.keyLength === 32 && i256c.mode === 'cbc');
    var i128ctr = crypto.getCipherInfo('aes-128-ctr');
    ck("info_aes128ctr", i128ctr && i128ctr.keyLength === 16 && i128ctr.mode === 'ctr');
    var i192ctr = crypto.getCipherInfo('aes-192-ctr');
    ck("info_aes192ctr", i192ctr && i192ctr.keyLength === 24 && i192ctr.mode === 'ctr');
    var i256ctr = crypto.getCipherInfo('aes-256-ctr');
    ck("info_aes256ctr", i256ctr && i256ctr.keyLength === 32 && i256ctr.mode === 'ctr');
    var i128g = crypto.getCipherInfo('aes-128-gcm');
    ck("info_aes128gcm", i128g && i128g.keyLength === 16 && i128g.ivLength === 12 && i128g.mode === 'gcm');
    var i192g = crypto.getCipherInfo('aes-192-gcm');
    ck("info_aes192gcm", i192g && i192g.keyLength === 24 && i192g.mode === 'gcm');
    var i256g = crypto.getCipherInfo('aes-256-gcm');
    ck("info_aes256gcm", i256g && i256g.keyLength === 32 && i256g.mode === 'gcm');
    ck("info_unknown_undef", crypto.getCipherInfo('no-such-cipher') === undefined);
    ck("info_no_args_throws", (thr(function(){ crypto.getCipherInfo(); }) || "").indexOf("requires a cipher name") >= 0);
    ck("info_non_string_throws", (thr(function(){ crypto.getCipherInfo(null); }) || "").indexOf("must be a string") >= 0);

    // crypto.constants spot checks (installation surface).
    ck("constants_rsa_pkcs1", crypto.constants.RSA_PKCS1_PADDING === 1);
    ck("constants_oaep", crypto.constants.RSA_PKCS1_OAEP_PADDING === 4);
    ck("constants_point_conv", crypto.constants.POINT_CONVERSION_UNCOMPRESSED === 4);
    ck("constants_ssl_op", typeof crypto.constants.SSL_OP_NO_TLSv1_3 === 'number');
    R.join('|');
"#);

// ── crypto.hash one-shot ───────────────────────────────────────────────────

crypto_case!(c2_crypto_hash_one_shot, r#"
    var crypto = require('crypto');
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
    function thr(fn) { try { fn(); return null; } catch (e) { return String((e && e.message) || e); } }

    ck("oneshot_sha256", crypto.hash('sha256', 'hello', 'hex') ===
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
    ck("oneshot_sha1", crypto.hash('sha1', 'hello', 'hex') === "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d");
    ck("oneshot_md5", crypto.hash('md5', 'hello', 'hex') === "5d41402abc4b2a76b9719d911017c592");
    ck("oneshot_sha384_len", crypto.hash('sha384', 'hello', 'hex').length === 96);
    ck("oneshot_sha224_len", crypto.hash('sha224', 'hello', 'hex').length === 56);
    ck("oneshot_sha512_len", crypto.hash('sha512', 'hello', 'hex').length === 128);
    ck("oneshot_base64", crypto.hash('sha256', 'hello', 'base64') === "LPJNul+wow4m6DsqxbninhsWHlwfp0JecwQzYpOLmCQ=");
    ck("oneshot_buffer", Buffer.isBuffer(crypto.hash('sha256', 'hello', 'buffer')));
    ck("oneshot_default_hex", crypto.hash('sha256', 'hello') === crypto.hash('sha256', 'hello', 'hex'));
    ck("oneshot_buffer_input", crypto.hash('sha256', Buffer.from('hello'), 'hex') === crypto.hash('sha256', 'hello', 'hex'));
    ck("oneshot_missing_args_throws", (thr(function(){ crypto.hash('sha256'); }) || "").indexOf("requires algorithm and input") >= 0);
    ck("oneshot_algo_non_string_throws", (thr(function(){ crypto.hash(null, 'x'); }) || "").indexOf("algorithm must be a string") >= 0);
    ck("oneshot_input_bad_throws", (thr(function(){ crypto.hash('sha256', 5); }) || "").indexOf("input must be a string or Buffer") >= 0);
    ck("oneshot_unsupported_throws", (thr(function(){ crypto.hash('sha999', 'x'); }) || "").indexOf("unsupported algorithm") >= 0);
    ck("oneshot_unknown_enc_hex", crypto.hash('sha256', 'hello', 'what') === crypto.hash('sha256', 'hello', 'hex'));
    R.join('|');
"#);

// ── Sign / Verify (HMAC + asymmetric true paths) ───────────────────────────

crypto_case!(c2_sign_verify_hmac_faces, r#"
    var crypto = require('crypto');
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
    function thr(fn) { try { fn(); return null; } catch (e) { return String((e && e.message) || e); } }

    // HMAC path via createSign/createVerify with a raw secret.
    var s = crypto.createSign('sha256');
    s.update('sign this');
    var sigBuf = s.sign('hmac-secret-key');
    ck("sign_buffer_default", Buffer.isBuffer(sigBuf));
    ck("sign_hex_len", (function(){
        var s2 = crypto.createSign('sha256'); s2.update('sign this');
        return s2.sign('hmac-secret-key', 'hex').length === 64;
    })());
    ck("sign_base64", (function(){
        var s3 = crypto.createSign('sha256'); s3.update('sign this');
        var b64 = s3.sign('hmac-secret-key', 'base64');
        var s4 = crypto.createSign('sha256'); s4.update('sign this');
        return s4.sign('hmac-secret-key', 'hex') === Buffer.from(b64, 'base64').toString('hex');
    })());
    ck("sign_sha512_len", (function(){
        var s5 = crypto.createSign('sha512'); s5.update('x');
        return s5.sign('k', 'hex').length === 128;
    })());
    var v = crypto.createVerify('sha256');
    v.update('sign this');
    ck("verify_true", v.verify('hmac-secret-key', sigBuf.toString('hex'), 'hex') === true);
    var v2 = crypto.createVerify('sha256');
    v2.update('sign this');
    ck("verify_wrong_data_false", v2.verify('hmac-secret-key', Buffer.from('other').toString('hex'), 'hex') === false);
    ck("verify_wrong_key_false", (function(){
        var v3 = crypto.createVerify('sha256'); v3.update('sign this');
        return v3.verify('another-key', sigBuf.toString('hex'), 'hex') === false;
    })());
    // verify signature as raw byte array (object arm)
    var v4 = crypto.createVerify('sha256');
    v4.update('sign this');
    var arr = []; for (var i = 0; i < sigBuf.length; i++) arr.push(sigBuf[i]);
    ck("verify_byte_array_sig", v4.verify('hmac-secret-key', arr, 'hex') === true);
    ck("verify_few_args_throws", (thr(function(){ crypto.createVerify('sha256').verify.call({}, 'k'); }) || "").indexOf("requires (key, signature)") >= 0);
    // sign.update number input → coerced empty-data chunk, must not throw
    ck("sign_number_update_ok", (function(){
        var s6 = crypto.createSign('sha256'); s6.update(5); return typeof s6.sign('k', 'hex') === 'string';
    })());
    R.join('|');
"#);

crypto_case!(c2_sign_verify_ed25519, r#"
    var crypto = require('crypto');
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
    var kp = crypto.generateKeyPairSync('ed25519');
    var privPem = kp.privateKey.export({ type: 'pkcs8', format: 'pem' });
    var pubPem = kp.publicKey.export({ type: 'spki', format: 'pem' });
    var s = crypto.createSign('sha256');
    s.update('ed message');
    var sigHex = s.sign(privPem, 'hex');
    ck("ed_sign_hex", typeof sigHex === 'string' && sigHex.length === 128);
    var v = crypto.createVerify('sha256');
    v.update('ed message');
    ck("ed_verify_true", v.verify(pubPem, sigHex, 'hex') === true);
    var v2 = crypto.createVerify('sha256');
    v2.update('other');
    ck("ed_verify_false", v2.verify(pubPem, sigHex, 'hex') === false);
    // one-shot crypto.sign / crypto.verify with the same PEMs
    var sig2 = crypto.sign('sha256', Buffer.from('ed message'), privPem);
    ck("oneshot_sign", Buffer.isBuffer(sig2) && sig2.length === 64);
    ck("oneshot_verify_true", crypto.verify('sha256', Buffer.from('ed message'), pubPem, sig2) === true);
    ck("oneshot_verify_false", crypto.verify('sha256', Buffer.from('nope'), pubPem, sig2) === false);
    // KeyObject slot path (from_pkcs8_der) — sign via KeyObject, verify via PEM.
    var sig3 = crypto.sign('sha256', Buffer.from('slot'), kp.privateKey);
    ck("keyobject_slot_sign", crypto.verify('sha256', Buffer.from('slot'), kp.publicKey, sig3) === true);
    // sign/verify error arms
    ck("sign_no_key_throws", (function(){
        try { crypto.sign('sha256', Buffer.from('x')); return false; }
        catch (e) { return String((e && e.message) || e).indexOf("key") >= 0; }
    })());
    ck("verify_no_key_throws", (function(){
        try { crypto.verify('sha256', Buffer.from('x'), null, Buffer.alloc(4)); return false; }
        catch (e) { return String((e && e.message) || e).indexOf("key") >= 0; }
    })());
    R.join('|');
"#);

fn js_sign_verify_ec_rsa() -> String {
    subst(
        r#"
    var crypto = require('crypto');
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }

    var ecPriv = %EC_PRIV%;
    var ecPub = %EC_PUB%;
    var s = crypto.createSign('sha256');
    s.update('ec message');
    var sig = s.sign(ecPriv, 'hex');
    ck("ec_sign_der", sig.length > 100);
    var v = crypto.createVerify('sha256');
    v.update('ec message');
    ck("ec_verify_true", v.verify(ecPub, sig, 'hex') === true);
    // one-shot crypto.sign/verify with EC + KeyObject slot path
    var privKey = crypto.createPrivateKey(ecPriv);
    var pubKey = crypto.createPublicKey(ecPub);
    var msg = Buffer.from('slot path');
    var sig2 = crypto.sign('sha256', msg, privKey);
    ck("ec_keyobject_sign", Buffer.isBuffer(sig2));
    ck("ec_keyobject_verify", crypto.verify('sha256', msg, pubKey, sig2) === true);

    var rsaPriv = %RSA_PRIV%;
    var s2 = crypto.createSign('sha256');
    s2.update('rsa message');
    var rsig = s2.sign(rsaPriv, 'hex');
    ck("rsa_sign_hex", typeof rsig === 'string' && rsig.length === 256);
    var pubPem = crypto.createPublicKey(rsaPriv).export({ type: 'spki', format: 'pem' });
    var v2 = crypto.createVerify('sha256');
    v2.update('rsa message');
    ck("rsa_verify_true", v2.verify(pubPem, rsig, 'hex') === true);
    // PSS family name resolves (RsaPss arm of resolve_sign_algorithm).
    var s3 = crypto.createSign('rsa-pss-sha256');
    s3.update('pss');
    ck("pss_resolves", typeof s3.sign(rsaPriv, 'hex') === 'string');
    R.join('|');
"#,
        &[
            ("%EC_PRIV%", &pem_js(EC_KEY_PEM)),
            ("%EC_PUB%", &pem_js(EC_PUB_PEM)),
            ("%RSA_PRIV%", &pem_js(RSA_KEY_PEM)),
        ],
    )
}

crypto_case!(c2_sign_verify_ec_and_rsa, js_sign_verify_ec_rsa());

// ── KeyObject 全 export 矩阵 ───────────────────────────────────────────────

crypto_case!(c2_secret_key_faces, r#"
    var crypto = require('crypto');
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
    function thr(fn) { try { fn(); return null; } catch (e) { return String((e && e.message) || e); } }

    var k = crypto.createSecretKey(Buffer.from('0102030405', 'hex'));
    ck("type_secret", k.type === 'secret');
    ck("symmetric_true", k.symmetric === true);
    ck("export_buffer", Buffer.isBuffer(k.export()));
    ck("export_roundtrip", k.export().toString('hex') === '0102030405');
    ck("export_format_buffer_ok", k.export({ format: 'buffer' }).toString('hex') === '0102030405');
    ck("export_pem_secret_throws", (thr(function(){ k.export({ format: 'pem' }); }) || "").indexOf("invalid format") >= 0);
    ck("string_hex_form", crypto.createSecretKey('68656c6c6f', 'hex').export().toString('utf8') === 'hello');
    ck("string_base64_form", crypto.createSecretKey('aGVsbG8=', 'base64').export().toString('utf8') === 'hello');
    ck("string_base64url_form", crypto.createSecretKey('aGVsbG8', 'base64url').export().toString('utf8') === 'hello');
    ck("string_default_utf8", crypto.createSecretKey('hello').export().toString('utf8') === 'hello');
    ck("string_bad_enc_throws", (thr(function(){ crypto.createSecretKey('xx', 'uu32'); }) || "").indexOf("unsupported encoding") >= 0);
    ck("bad_hex_throws", (thr(function(){ crypto.createSecretKey('zz', 'hex'); }) || "").indexOf("hex decode") >= 0);
    ck("empty_throws", (thr(function(){ crypto.createSecretKey(Buffer.alloc(0)); }) || "").indexOf("non-empty") >= 0);
    ck("no_args_throws", (thr(function(){ crypto.createSecretKey(); }) || "").indexOf("requires key material") >= 0);
    ck("number_throws", (thr(function(){ crypto.createSecretKey(5); }) || "").indexOf("Buffer or string") >= 0);
    // generateKeySync — secret KeyObject of the requested length.
    var gk = crypto.generateKeySync('hmac', { length: 24 });
    ck("generateKeySync_len", gk.type === 'secret' && gk.export().length === 24);
    ck("generateKeySync_double_len", crypto.generateKeySync('hmac', { length: 16.0 }).export().length === 16);
    // KeyObject internal constructor face.
    var ko = crypto.KeyObject(Buffer.from('abcd'), 'secret');
    ck("keyobject_ctor", ko.type === 'secret' && ko.export().toString() === 'abcd');
    R.join('|');
"#);

fn js_asym_key_export_matrix() -> String {
    subst(
        r#"
    var crypto = require('crypto');
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
    function thr(fn) { try { fn(); return null; } catch (e) { return String((e && e.message) || e); } }

    var rsaPrivPem = %RSA_PRIV%;
    var pk = crypto.createPrivateKey(rsaPrivPem);
    ck("priv_type", pk.type === 'private');
    ck("priv_asym_rsa", pk.asymmetricKeyType === 'rsa');
    ck("priv_symmetric_false", pk.symmetric === false);
    var pkcs8 = pk.export({ type: 'pkcs8', format: 'der' });
    ck("priv_pkcs8_der", Buffer.isBuffer(pkcs8) && pkcs8.length > 100);
    var pkcs8pem = pk.export({ type: 'pkcs8', format: 'pem' });
    ck("priv_pkcs8_pem", String(pkcs8pem).indexOf('BEGIN PRIVATE KEY') >= 0);
    var pkcs1pem = pk.export({ type: 'pkcs1', format: 'pem' });
    ck("priv_pkcs1_pem", String(pkcs1pem).indexOf('BEGIN RSA PRIVATE KEY') >= 0);
    var pkcs1der = pk.export({ type: 'pkcs1', format: 'der' });
    ck("priv_pkcs1_der", Buffer.isBuffer(pkcs1der) && pkcs1der.length > 100);
    ck("priv_bad_type_throws", (thr(function(){ pk.export({ type: 'spki', format: 'pem' }); }) || "").indexOf("invalid export type") >= 0);
    ck("export_bad_format_throws", (thr(function(){ pk.export({ type: 'pkcs8', format: 'jwt' }); }) || "").indexOf("invalid export format") >= 0);

    var pub = crypto.createPublicKey(rsaPrivPem);
    ck("pub_type", pub.type === 'public');
    ck("pub_asym_rsa", pub.asymmetricKeyType === 'rsa');
    var spkiPem = pub.export({ type: 'spki', format: 'pem' });
    ck("pub_spki_pem", String(spkiPem).indexOf('BEGIN PUBLIC KEY') >= 0);
    var spkiDer = pub.export({ type: 'spki', format: 'der' });
    ck("pub_spki_der", Buffer.isBuffer(spkiDer) && spkiDer.length > 100);
    var pubPkcs1 = pub.export({ type: 'pkcs1', format: 'pem' });
    ck("pub_pkcs1_rsa", String(pubPkcs1).indexOf('BEGIN RSA PUBLIC KEY') >= 0);
    ck("pub_default_is_spki_pem", String(pub.export()) === String(spkiPem));

    // EC: pkcs8 + sec1 export arms + asymmetricKeyType 'ec'.
    var ecPrivPem = %EC_PRIV%;
    var ec = crypto.createPrivateKey(ecPrivPem);
    ck("ec_asym_kind", ec.asymmetricKeyType === 'ec');
    var sec1 = ec.export({ type: 'sec1', format: 'pem' });
    ck("ec_sec1_pem", String(sec1).indexOf('BEGIN EC PRIVATE KEY') >= 0);
    var ecPkcs8 = ec.export({ type: 'pkcs8', format: 'pem' });
    ck("ec_pkcs8_pem", String(ecPkcs8).indexOf('BEGIN PRIVATE KEY') >= 0);
    var ecPub = crypto.createPublicKey(ecPrivPem);
    ck("pub_pkcs1_non_rsa_throws", (thr(function(){ ecPub.export({ type: 'pkcs1', format: 'pem' }); }) || "").indexOf("RSA only") >= 0);
    ck("priv_sec1_non_ec_throws", (thr(function(){ pk.export({ type: 'sec1', format: 'pem' }); }) || "").indexOf("EC only") >= 0);
    // Private key can export a public-form SPKI face (parse_key_to_pkey public arm on private storage
    // is NOT the export face — private storage exports only private forms; assert spki on private throws).
    ck("priv_spki_export_throws", (thr(function(){ ec.export({ type: 'spki', format: 'pem' }); }) || "").indexOf("invalid export type") >= 0);

    // Error arms of createPublicKey/createPrivateKey.
    ck("createPub_noargs_throws", (thr(function(){ crypto.createPublicKey(); }) || "").indexOf("requires a key") >= 0);
    ck("createPriv_noargs_throws", (thr(function(){ crypto.createPrivateKey(); }) || "").indexOf("requires a key") >= 0);
    ck("createPub_empty_string_throws", (thr(function(){ crypto.createPublicKey(''); }) || "").indexOf("key is empty") >= 0);
    ck("createPub_garbage_throws", (thr(function(){ crypto.createPublicKey('not a key'); }) || "").indexOf("Failed to parse") >= 0);
    ck("createPub_passphrase_throws", (thr(function(){
        crypto.createPublicKey({ key: rsaPrivPem, format: 'pem', passphrase: 'x' });
    }) || "").indexOf("passphrase") >= 0);
    ck("createPub_number_throws", (thr(function(){ crypto.createPublicKey(5); }) || "").indexOf("must be a string") >= 0);
    // options-object import form.
    var viaOpts = crypto.createPublicKey({ key: rsaPrivPem, format: 'pem' });
    ck("pub_via_options", viaOpts.type === 'public' && viaOpts.asymmetricKeyType === 'rsa');
    // KeyObject → KeyObject clone.
    var clone = crypto.createPublicKey(viaOpts);
    ck("pub_from_keyobject", clone.export({ type: 'spki', format: 'der' }).toString('hex') ===
        viaOpts.export({ type: 'spki', format: 'der' }).toString('hex'));
    // createPublicKey(privateKey) derives the public half.
    var derived = crypto.createPublicKey(pk);
    ck("pub_from_priv", derived.type === 'public' && derived.asymmetricKeyType === 'rsa');
    R.join('|');
"#,
        &[("%RSA_PRIV%", &pem_js(RSA_KEY_PEM)), ("%EC_PRIV%", &pem_js(EC_KEY_PEM))],
    )
}

crypto_case!(c2_asym_key_export_matrix, js_asym_key_export_matrix());

crypto_case!(c2_generate_keypair_sync_faces, r#"
    var crypto = require('crypto');
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
    function thr(fn) { try { fn(); return null; } catch (e) { return String((e && e.message) || e); } }

    var ed = crypto.generateKeyPairSync('ed25519');
    ck("ed_keys_present", ed.publicKey && ed.privateKey);
    ck("ed_types", ed.publicKey.type === 'public' && ed.privateKey.type === 'private');
    ck("ed_asym", ed.publicKey.asymmetricKeyType === 'ed25519');
    ck("ed_export", String(ed.publicKey.export({ type: 'spki', format: 'pem' })).indexOf('BEGIN PUBLIC KEY') >= 0);

    var ec = crypto.generateKeyPairSync('ec', { namedCurve: 'P-256' });
    ck("ec_asym", ec.publicKey.asymmetricKeyType === 'ec');
    var ec384 = crypto.generateKeyPairSync('ec', { namedCurve: 'P-384' });
    ck("ec384_asym", ec384.publicKey.asymmetricKeyType === 'ec');
    var x = crypto.generateKeyPairSync('x25519');
    // NOTE: x25519 kind resolves to "unknown" in pkey_kind_name (vendored
    // BoringSSL NID not matched) — node reports "x25519". Registered as
    // C2-FINDING-X25519-KIND; key generation + export surface pinned here.
    ck("x25519_keys", x.publicKey && x.privateKey && typeof x.publicKey.asymmetricKeyType === 'string');
    var rsa = crypto.generateKeyPairSync('rsa', { modulusLength: 1024 });
    ck("rsa_asym", rsa.publicKey.asymmetricKeyType === 'rsa');
    ck("rsa_sign_roundtrip", (function(){
        var s = crypto.createSign('sha256'); s.update('kp');
        var sig = s.sign(rsa.privateKey.export({ type: 'pkcs8', format: 'pem' }), 'hex');
        var v = crypto.createVerify('sha256'); v.update('kp');
        return v.verify(rsa.publicKey.export({ type: 'spki', format: 'pem' }), sig, 'hex');
    })());
    ck("missing_type_throws", (thr(function(){ crypto.generateKeyPairSync(); }) || "").indexOf("requires a key type") >= 0);
    ck("type_non_string_throws", (thr(function(){ crypto.generateKeyPairSync(null); }) || "").indexOf("type must be a string") >= 0);
    ck("unsupported_type_throws", (thr(function(){ crypto.generateKeyPairSync('dsa'); }) || "").indexOf("unsupported key type") >= 0);
    ck("bad_curve_throws", (thr(function(){ crypto.generateKeyPairSync('ec', { namedCurve: 'P-777' }); }) || "").indexOf("unsupported EC curve") >= 0);
    R.join('|');
"#);

// ── ECDH / DH ──────────────────────────────────────────────────────────────

fn js_ecdh_faces() -> String {
    subst(
        r#"
    var crypto = require('crypto');
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
    function thr(fn) { try { fn(); return null; } catch (e) { return String((e && e.message) || e); } }

    var a = crypto.createECDH('prime256v1');
    var b = crypto.createECDH('prime256v1');
    var pubA = Buffer.from(a.getPublicKey());
    var pubB = Buffer.from(b.getPublicKey());
    ck("pub_len65", pubA.length === 65 && pubB.length === 65);
    var s1 = Buffer.from(a.computeSecret(pubB));
    var s2 = Buffer.from(b.computeSecret(pubA));
    ck("shared_equal", s1.length > 0 && s1.toString('hex') === s2.toString('hex'));
    // x25519 arm
    var xa = crypto.createECDH('x25519');
    var xb = crypto.createECDH('x25519');
    ck("x25519_shared", Buffer.from(xa.computeSecret(xb.getPublicKey())).toString('hex') ===
        Buffer.from(xb.computeSecret(xa.getPublicKey())).toString('hex'));
    // P-384 arm (parse_curve spelling: p384 | secp384r1)
    var pa = crypto.createECDH('secp384r1');
    var pb = crypto.createECDH('secp384r1');
    ck("p384_shared", Buffer.from(pa.computeSecret(pb.getPublicKey())).length === 48);
    // convertKey: reconstruct the public point from the known SEC1 private scalar.
    var convPriv = Buffer.from('%EC_PRIV_HEX%', 'hex');
    ck("convert_key_from_priv", Buffer.from(a.convertKey(convPriv)).toString('hex') === '%EC_PUB_HEX%');
    ck("convert_no_args_throws", (thr(function(){ a.convertKey(); }) || "").indexOf("requires key data") >= 0);
    ck("convert_bad_data_throws", (thr(function(){ a.convertKey(Buffer.alloc(3, 1)); }) || "").indexOf("failed") >= 0);
    ck("no_args_throws", (thr(function(){ crypto.createECDH(); }) || "").indexOf("requires a curve") >= 0);
    ck("curve_non_string_throws", (thr(function(){ crypto.createECDH(null); }) || "").indexOf("curve must be a string") >= 0);
    ck("bad_curve_throws", (thr(function(){ crypto.createECDH('p-999'); }) || "").indexOf("failed") >= 0);
    ck("compute_no_args_throws", (thr(function(){ a.computeSecret(); }) || "").indexOf("requires peer public key") >= 0);
    ck("compute_garbage_throws", (thr(function(){ a.computeSecret(Buffer.alloc(3, 1)); }) || "").indexOf("failed") >= 0);
    ck("plain_receiver_throws", (thr(function(){ a.getPublicKey.call({}); }) || "").indexOf("invalid receiver") >= 0);
    R.join('|');
"#,
        &[
            ("%EC_PRIV_HEX%", EC_PRIV_HEX),
            ("%EC_PUB_HEX%", EC_PUB_HEX),
        ],
    )
}

crypto_case!(c2_ecdh_faces, js_ecdh_faces());

crypto_case!(c2_dh_faces, r#"
    var crypto = require('crypto');
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
    function thr(fn) { try { fn(); return null; } catch (e) { return String((e && e.message) || e); } }

    var d1 = crypto.createDiffieHellman(512);
    d1.generateKeys();
    var d2 = crypto.createDiffieHellman(d1.getPrime(), d1.getGenerator());
    d2.generateKeys();
    var s1 = Buffer.from(d1.computeSecret(d2.getPublicKey()));
    var s2 = Buffer.from(d2.computeSecret(d1.getPublicKey()));
    ck("shared_equal", s1.length > 0 && s1.toString('hex') === s2.toString('hex'));
    ck("prime_len", Buffer.from(d1.getPrime()).length === 64);
    ck("generator_nonempty", Buffer.from(d1.getGenerator()).length > 0);
    ck("pub_key_nonempty", Buffer.from(d1.getPublicKey()).length > 0);
    ck("priv_key_nonempty", Buffer.from(d1.getPrivateKey()).length > 0);
    // group form (modp14 well-known prime; both sides agree).
    var g1 = crypto.createDiffieHellmanGroup('modp14');
    g1.generateKeys();
    var g2 = crypto.getDiffieHellman('modp14');
    g2.generateKeys();
    var gs1 = Buffer.from(g1.computeSecret(g2.getPublicKey()));
    var gs2 = Buffer.from(g2.computeSecret(g1.getPublicKey()));
    ck("modp14_shared", gs1.toString('hex') === gs2.toString('hex'));
    ck("modp1_ok", (function(){
        var m = crypto.createDiffieHellmanGroup('modp1'); m.generateKeys();
        return Buffer.from(m.getPublicKey()).length > 0;
    })());
    ck("no_args_throws", (thr(function(){ crypto.createDiffieHellman(); }) || "").indexOf("requires a prime") >= 0);
    ck("bad_group_throws", (thr(function(){ crypto.createDiffieHellmanGroup('modp999'); }) || "").indexOf("Unsupported DH group") >= 0);
    ck("group_no_args_throws", (thr(function(){ crypto.createDiffieHellmanGroup(); }) || "").indexOf("requires a group") >= 0);
    ck("compute_no_args_throws", (thr(function(){ d1.computeSecret(); }) || "").indexOf("requires peer public key") >= 0);
    // BoringSSL DH accepts any in-range peer value (0x0707 derived a 64-byte
    // secret in the probe) — only an EMPTY peer key errors. Node is equally
    // permissive for in-range integers; pin the error arm only.
    ck("compute_empty_throws", (thr(function(){ d1.computeSecret(Buffer.alloc(0)); }) || "").indexOf("failed") >= 0);
    ck("plain_receiver_throws", (thr(function(){ d1.getPrime.call({}); }) || "").indexOf("invalid diffiehellman receiver") >= 0);
    ck("diffiehellman_garbage_throws", (thr(function(){ crypto.diffieHellman(Buffer.alloc(4), Buffer.alloc(4)); }) || "").length > 0);
    R.join('|');
"#);

// ── checkPrime / generatePrime ─────────────────────────────────────────────

crypto_case!(c2_prime_faces, r#"
    var crypto = require('crypto');
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
    function thr(fn) { try { fn(); return null; } catch (e) { return String((e && e.message) || e); } }

    function be(n) { var a = []; while (n > 0) { a.unshift(n & 0xff); n = Math.floor(n / 256); } return Buffer.from(a); }
    ck("prime_11", crypto.checkPrimeSync(be(11)) === true);
    ck("prime_7919", crypto.checkPrimeSync(be(7919)) === true);
    ck("composite_9", crypto.checkPrimeSync(be(9)) === false);
    ck("composite_1", crypto.checkPrimeSync(be(1)) === false);
    ck("large_mersenne", crypto.checkPrimeSync(Buffer.from('1fffffffffffffff', 'hex')) === true);
    // generatePrimeSync: produced value round-trips as prime (node v24 oracle: output is a Buffer).
    var p = crypto.generatePrimeSync(64);
    ck("gen_len", Buffer.from(p).length === 8);
    ck("gen_is_prime", crypto.checkPrimeSync(Buffer.from(p)) === true);
    var p2 = crypto.generatePrimeSync(64, { safe: false });
    ck("gen_unsafe_ok", crypto.checkPrimeSync(Buffer.from(p2)) === true);
    var p3 = crypto.generatePrimeSync(3);
    ck("gen_prime3", crypto.checkPrimeSync(Buffer.from(p3)) === true);
    ck("gen_no_args_throws", (thr(function(){ crypto.generatePrimeSync(); }) || "").indexOf("requires a bit size") >= 0);
    ck("gen_non_num_throws", (thr(function(){ crypto.generatePrimeSync('x'); }) || "").indexOf("size must be a number") >= 0);
    ck("gen_too_small_throws", (thr(function(){ crypto.generatePrimeSync(1); }) || "").indexOf("at least 2") >= 0);
    R.join('|');
"#);

#[test]
fn c2_prime_and_checkprime_async() {
    let mut ctx = setup_ctx();
    let out = eval_string(
        &mut ctx,
        r#"
        var crypto = require('crypto');
        globalThis.__pp = { primeDone: false, primeLen: -1, checkDone: false, checkVal: null };
        crypto.generatePrime(64, { safe: false }, function(err, prime) {
            globalThis.__pp.primeDone = true;
            if (!err) {
                globalThis.__pp.primeLen = prime.length;
                crypto.checkPrime(prime, function(err2, result) {
                    globalThis.__pp.checkDone = true;
                    globalThis.__pp.checkVal = result;
                });
            }
        });
        'scheduled'
    "#,
    );
    assert_eq!(out, "scheduled");
    pump_until_quiescent(&mut ctx, 20_000);
    let verdict = eval_string(
        &mut ctx,
        r#"
        var v = globalThis.__pp;
        // NOTE: the async checkPrime callback delivers a one-byte Buffer([1]),
        // not node's boolean — shape deviation registered as
        // C2-FINDING-CHECKPRIME-CB-BUFFER; the payload is pinned here.
        (v.primeDone && v.primeLen === 8 && v.checkDone &&
         v.checkVal && v.checkVal.length === 1 && v.checkVal[0] === 1) ? "PASS" : "FAIL:" + JSON.stringify(v)
    "#,
    );
    assert_eq!(verdict, "PASS", "prime async: {verdict}");
}

crypto_case!(c2_checkprime_no_cb_throws, r#"
    var crypto = require('crypto');
    var R = [];
    function thr(fn) { try { fn(); return null; } catch (e) { return String((e && e.message) || e); } }
    R.push("no_cb_throws" + ((thr(function(){ crypto.checkPrime(Buffer.from([3])); }) || "").indexOf("requires a callback") >= 0 ? " PASS" : " FAIL"));
    R.push("gp_no_cb_sync_fallback" + ((function(){
        try { var p = crypto.generatePrime(32, { safe: false }); return Buffer.from(p).length >= 4 ? " PASS" : " FAIL"; }
        catch (e) { return " FAIL:" + e; }
    })()));
    R.join('|');
"#);

// ── X509 / Certificate(SPKAC) ──────────────────────────────────────────────

fn js_x509_faces() -> String {
    subst(
        r#"
    var crypto = require('crypto');
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
    function thr(fn) { try { fn(); return null; } catch (e) { return String((e && e.message) || e); } }

    var pem = %CERT%;
    // NOTE: X509Certificate/X509/Certificate are plain JS_DefineFunction
    // installs (no JSFUN_CONSTRUCTOR) — `new` throws "is not a constructor"
    // while node requires new. Plain-call shape registered as
    // C2-FINDING-X509-NOT-CONSTRUCTOR; the parse face is pinned here.
    var c = crypto.X509Certificate(pem);
    ck("subject_cn", String(c.subject).indexOf('c2test') >= 0);
    ck("issuer_cn", String(c.issuer).indexOf('c2test') >= 0);
    ck("serial_nonempty", typeof c.serialNumber === 'string' && c.serialNumber.length > 0);
    ck("validity_present", typeof c.validFrom === 'string' && c.validTo.length > 0);
    // node v24 oracle: fingerprint256 is colon-separated upper hex (95 chars).
    ck("fp256_colon95", typeof c.fingerprint256 === 'string' && c.fingerprint256.length === 95);
    ck("fp1_colon59", typeof c.fingerprint === 'string' && c.fingerprint.length === 59);
    // X509 alias face
    var c2 = crypto.X509(pem);
    ck("x509_alias_subject", String(c2.subject) === String(c.subject));
    ck("x509_serial_equal", c2.serialNumber === c.serialNumber);
    // DER Buffer form of the X509 alias.
    ck("x509_der_form", (function(){
        var b64 = pem.split('\n').filter(function(l){ return l && l.indexOf('-----') !== 0; }).join('');
        var der = Buffer.from(b64, 'base64');
        var c3 = crypto.X509(der);
        return c3.serialNumber === c.serialNumber;
    })());
    ck("cert_no_args_throws", (thr(function(){ crypto.X509Certificate(); }) || "").indexOf("requires a PEM") >= 0);
    ck("cert_non_string_throws", (thr(function(){ crypto.X509Certificate(null); }) || "").indexOf("must be a PEM string") >= 0);
    ck("cert_garbage_throws", (thr(function(){ crypto.X509Certificate('garbage'); }) || "").indexOf("parse failed") >= 0);
    ck("x509_no_args_throws", (thr(function(){ crypto.X509(); }) || "").indexOf("requires a PEM") >= 0);
    ck("x509_bad_type_throws", (thr(function(){ crypto.X509(null); }) || "").indexOf("must be a PEM string or DER") >= 0);
    R.join('|');
"#,
        &[("%CERT%", &pem_js(RSA_CERT_PEM))],
    )
}

crypto_case!(c2_x509_faces, js_x509_faces());

crypto_case!(c2_certificate_spkac_faces, r#"
    var crypto = require('crypto');
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
    function thr(fn) { try { fn(); return null; } catch (e) { return String((e && e.message) || e); } }

    // NOTE: crypto.Certificate is defined as a plain JS_DefineFunction, not
    // JSFUN_CONSTRUCTOR — `new crypto.Certificate()` throws "is not a
    // constructor" while node requires new. Plain-call shape registered as
    // C2-FINDING-CERTIFICATE-NOT-CONSTRUCTOR; the method face is pinned here.
    var cert = crypto.Certificate();
    ck("verify_spkac_garbage_false", cert.verifySpkac(Buffer.from('junk')) === false);
    ck("verify_pubkey_garbage_false", cert.verifyPublicKey(Buffer.from('junk')) === false);
    ck("export_challenge_empty", cert.exportChallenge(Buffer.from([0x30, 0x03, 0x02, 0x01, 0x00])) === "");
    ck("export_pub_returns_buffer", Buffer.isBuffer(cert.exportPublicKey(Buffer.from('x'))));
    ck("export_spkac_roundtrip", cert.exportSpkac(Buffer.from('data')).toString() === 'data');
    ck("verify_spkac_no_args_throws", (thr(function(){ cert.verifySpkac(); }) || "").indexOf("requires a buffer") >= 0);
    ck("export_pub_no_args_throws", (thr(function(){ cert.exportPublicKey(); }) || "").indexOf("requires a buffer") >= 0);
    ck("export_challenge_no_args_throws", (thr(function(){ cert.exportChallenge(); }) || "").indexOf("requires a buffer") >= 0);
    ck("export_spkac_no_args_throws", (thr(function(){ cert.exportSpkac(); }) || "").indexOf("requires a buffer") >= 0);
    ck("verify_pub_no_args_throws", (thr(function(){ cert.verifyPublicKey(); }) || "").indexOf("requires a key buffer") >= 0);
    ck("challenge_short_buf_empty", cert.exportChallenge(Buffer.from([0x30])) === "");
    R.join('|');
"#);

// ── publicEncrypt / privateDecrypt(RSA 四面) ───────────────────────────────

fn js_rsa_crypt_faces() -> String {
    subst(
        r#"
    var crypto = require('crypto');
    var R = [];
    function ck(label, ok) { R.push(label + (ok ? " PASS" : " FAIL")); }
    function thr(fn) { try { fn(); return null; } catch (e) { return String((e && e.message) || e); } }

    var priv = %RSA_PRIV%;
    var pub = crypto.createPublicKey(priv).export({ type: 'spki', format: 'pem' });
    var msg = 'rsa-pkcs1 payload';
    var enc = crypto.publicEncrypt(pub, Buffer.from(msg));
    ck("pe_is_buffer", Buffer.isBuffer(enc) && enc.length === 128);
    // node v24 oracle: publicDecrypt of an ENCRYPT block fails PKCS#1 type-1
    // padding ("invalid padding"); privateDecrypt recovers it.
    ck("pubdec_enc_block_throws", (thr(function(){ crypto.publicDecrypt(pub, enc); }) || "").length > 0);
    ck("privdec_roundtrip", crypto.privateDecrypt(priv, enc).toString('utf8') === msg);
    // privateEncrypt (sign-without-digest) → publicDecrypt verifies.
    var signed = crypto.privateEncrypt(priv, Buffer.from('raw sig'));
    ck("pe_priv_len", Buffer.isBuffer(signed) && signed.length === 128);
    ck("pubdec_raw_sig", crypto.publicDecrypt(pub, signed).toString('utf8') === 'raw sig');
    // Error arms.
    ck("pe_no_key_throws", (thr(function(){ crypto.publicEncrypt(); }) || "").indexOf("key argument required") >= 0);
    ck("pd_no_key_throws", (thr(function(){ crypto.publicDecrypt(); }) || "").indexOf("key argument required") >= 0);
    ck("priven_no_key_throws", (thr(function(){ crypto.privateEncrypt(); }) || "").indexOf("key argument required") >= 0);
    ck("privdec_no_key_throws", (thr(function(){ crypto.privateDecrypt(); }) || "").indexOf("key argument required") >= 0);
    ck("pe_bad_key_throws", (thr(function(){ crypto.publicEncrypt('not a key', Buffer.from('x')); }) || "").length > 0);
    ck("privdec_bad_key_throws", (thr(function(){ crypto.privateDecrypt('not a key', Buffer.from('x')); }) || "").length > 0);
    ck("priven_pub_key_throws", (thr(function(){ crypto.privateEncrypt(pub, Buffer.from('x')); }) || "").length > 0);
    ck("pubdec_garbage_throws", (thr(function(){ crypto.publicDecrypt(pub, Buffer.from('garbage data!!')); }) || "").length > 0);
    R.join('|');
"#,
        &[("%RSA_PRIV%", &pem_js(RSA_KEY_PEM))],
    )
}

crypto_case!(c2_rsa_crypt_faces, js_rsa_crypt_faces());
