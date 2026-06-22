// @trace REQ-ENG-007 [level:integration]
// Conformance tests for node:crypto against Node.js / Bun reference behavior.
// Reference: ~/code/rust/bun/test/js/node/crypto/crypto.test.ts (MIT, Bun project)
//
// All checks in one #[test] — SpiderMonkey is single-init.

#[path = "../conformance_common.rs"]
mod common;

use common::{make_ctx, run_checks, CHECK_SCAFFOLD};

#[test]
fn test_crypto_conformance_suite() {
    let mut ctx = make_ctx();

    // ===== createHash with known vectors =====
    let src = format!(
        r##"
        {scaffold}
        var crypto = require('crypto');
        check("createHash_is_function", function() {{
            return typeof crypto.createHash === "function";
        }});
        check("md5_known_vector", function() {{
            return crypto.createHash("md5").update("hello").digest("hex")
                === "5d41402abc4b2a76b9719d911017c592";
        }});
        check("sha256_known_vector", function() {{
            return crypto.createHash("sha256").update("hello").digest("hex")
                === "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        }});
        check("sha1_known_vector", function() {{
            return crypto.createHash("sha1").update("hello").digest("hex")
                === "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d";
        }});
        check("sha512_length", function() {{
            return crypto.createHash("sha512").update("hello").digest("hex").length === 128;
        }});
        check("digest_base64_nonempty", function() {{
            return crypto.createHash("sha256").update("hello").digest("base64").length > 0;
        }});
        check("chained_update_equivalence", function() {{
            return crypto.createHash("sha256").update("hel").update("lo").digest("hex")
                === crypto.createHash("sha256").update("hello").digest("hex");
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== createHmac =====
    let src = format!(
        r##"
        {scaffold}
        var crypto = require('crypto');
        check("createHmac_is_function", function() {{
            return typeof crypto.createHmac === "function";
        }});
        check("hmac_sha256_length", function() {{
            var h = crypto.createHmac("sha256", "key").update("hello").digest("hex");
            return typeof h === "string" && h.length === 64;
        }});
        check("hmac_sha1_length", function() {{
            var h = crypto.createHmac("sha1", "key").update("hello").digest("hex");
            return typeof h === "string" && h.length === 40;
        }});
        check("hmac_md5_deviation_now_supported", function() {{
            // HMAC-MD5 now supported via BoringSSL EVP_md5 (REQ-ENG-007).
            // MD5 digest is 16 bytes → 32 hex chars.
            var h = crypto.createHmac("md5", "key").update("hello").digest("hex");
            return typeof h === "string" && h.length === 32;
        }});
        check("hmac_deterministic", function() {{
            var a = crypto.createHmac("sha256", "key").update("hello").digest("hex");
            var b = crypto.createHmac("sha256", "key").update("hello").digest("hex");
            return a === b;
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== randomBytes / randomUUID =====
    // NOTE: bao_runtime's randomBytes returns a generic object (Uint8Array-like),
    // not a Buffer instance (Node.js returns Buffer). Documented in GAP_REPORT.
    let src = format!(
        r##"
        {scaffold}
        var crypto = require('crypto');
        check("randomBytes_size", function() {{
            var b = crypto.randomBytes(16);
            return b.length === 16;
        }});
        check("randomBytes_unequal", function() {{
            var a = crypto.randomBytes(32).toString("hex");
            var b = crypto.randomBytes(32).toString("hex");
            return a !== b;
        }});
        check("randomBytes_zero_size", function() {{
            return crypto.randomBytes(0).length === 0;
        }});
        check("randomUUID_format", function() {{
            var u = crypto.randomUUID();
            return typeof u === "string" && u.length === 36 && u.charAt(8) === "-";
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== pbkdf2Sync =====
    let src = format!(
        r##"
        {scaffold}
        var crypto = require('crypto');
        check("pbkdf2Sync_size", function() {{
            var k = crypto.pbkdf2Sync("password", "salt", 1000, 32, "sha256");
            return k.length === 32;
        }});
        check("pbkdf2Sync_deterministic", function() {{
            var a = crypto.pbkdf2Sync("password", "salt", 1000, 32, "sha256").toString("hex");
            var b = crypto.pbkdf2Sync("password", "salt", 1000, 32, "sha256").toString("hex");
            return a === b;
        }});
        check("pbkdf2Sync_diff_salt", function() {{
            var a = crypto.pbkdf2Sync("password", "salt1", 1000, 32, "sha256").toString("hex");
            var b = crypto.pbkdf2Sync("password", "salt2", 1000, 32, "sha256").toString("hex");
            return a !== b;
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== timingSafeEqual =====
    let src = format!(
        r##"
        {scaffold}
        var crypto = require('crypto');
        check("timingSafeEqual_equal", function() {{
            return crypto.timingSafeEqual(Buffer.from("abc"), Buffer.from("abc")) === true;
        }});
        check("timingSafeEqual_diff", function() {{
            return crypto.timingSafeEqual(Buffer.from("abc"), Buffer.from("abd")) === false;
        }});
        check("timingSafeEqual_diff_lengths_throws", function() {{
            try {{ crypto.timingSafeEqual(Buffer.from("a"), Buffer.from("ab")); return false; }}
            catch(e) {{ return true; }}
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== Cipher / Decipher round-trip =====
    let src = format!(
        r##"
        {scaffold}
        var crypto = require('crypto');
        check("createCipheriv_is_function", function() {{
            return typeof crypto.createCipheriv === "function";
        }});
        check("aes256_roundtrip", function() {{
            var key = Buffer.from("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef", "hex");
            var iv = Buffer.from("0123456789abcdef0123456789abcdef", "hex");
            var cipher = crypto.createCipheriv("aes-256-cbc", key, iv);
            var enc = cipher.update("hello", "utf8", "hex") + cipher.final("hex");
            var decipher = crypto.createDecipheriv("aes-256-cbc", key, iv);
            var dec = decipher.update(enc, "hex", "utf8") + decipher.final("utf8");
            return dec === "hello";
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== enumerate / subtle =====
    let src = format!(
        r##"
        {scaffold}
        var crypto = require('crypto');
        check("getHashes_returns_array", function() {{
            return Array.isArray(crypto.getHashes());
        }});
        check("getHashes_includes_sha256", function() {{
            return crypto.getHashes().indexOf("sha256") >= 0;
        }});
        check("getCiphers_returns_array", function() {{
            return Array.isArray(crypto.getCiphers());
        }});
        check("subtle_exists", function() {{
            return typeof crypto.subtle === "object";
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    bun_runtime::shutdown_thread_sm();
}

#[test]
fn test_crypto_conformance_ecdh() {
    // @trace REQ-ENG-007 [api:node:crypto createECDH]
    // Real ECDH via bao_crypto::key_exchange (BoringSSL EC_KEY/ECDH_compute_key
    // for P-256/P-384, EVP_PKEY for X25519). Verifies: constructor is callable,
    // getPublicKey returns non-empty bytes, and two instances on the same curve
    // derive identical shared secrets via computeSecret (the actual ECDH
    // contract — not just a typeof check).
    let mut ctx = make_ctx();
    let src = format!(
        r##"
        {scaffold}
        var crypto = require('crypto');
        check("createECDH_is_function", function() {{
            return typeof crypto.createECDH === "function";
        }});
        check("createECDH_prime256v1_constructs", function() {{
            var alice = crypto.createECDH("prime256v1");
            return typeof alice === "object" && alice !== null;
        }});
        check("ecdh_getPublicKey_returns_bytes", function() {{
            var alice = crypto.createECDH("prime256v1");
            var pub = alice.getPublicKey();
            return !!pub && typeof pub.length === "number" && pub.length > 0;
        }});
        check("ecdh_computeSecret_roundtrip_matches", function() {{
            var alice = crypto.createECDH("prime256v1");
            var bob = crypto.createECDH("prime256v1");
            var alicePub = alice.getPublicKey();
            var bobPub = bob.getPublicKey();
            var s1 = alice.computeSecret(bobPub);
            var s2 = bob.computeSecret(alicePub);
            // Shared secrets must be equal-length byte buffers and bit-identical.
            if (!s1 || !s2 || s1.length !== s2.length || s1.length === 0) return false;
            for (var i = 0; i < s1.length; i++) {{ if (s1[i] !== s2[i]) return false; }}
            return true;
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);
    bun_runtime::shutdown_thread_sm();
}

#[test]
fn test_crypto_conformance_x509() {
    let mut ctx = make_ctx();
    let src = format!(
        r##"
        {scaffold}
        var crypto = require('crypto');
        check("X509_constructor", function() {{
            return typeof crypto.X509 === "function";
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);
    bun_runtime::shutdown_thread_sm();
}

#[test]
fn test_crypto_conformance_hkdf() {
    let mut ctx = make_ctx();
    let src = format!(
        r##"
        {scaffold}
        var crypto = require('crypto');
        check("hkdfSync_exists", function() {{
            return typeof crypto.hkdfSync === "function";
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);
    bun_runtime::shutdown_thread_sm();
}

#[test]
fn test_crypto_conformance_dh() {
    let mut ctx = make_ctx();
    let src = format!(
        r##"
        {scaffold}
        var crypto = require('crypto');
        check("createDiffieHellman_exists", function() {{
            return typeof crypto.createDiffieHellman === "function";
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);
    bun_runtime::shutdown_thread_sm();
}

#[test]
fn test_crypto_conformance_randombytes_is_buffer() {
    // Node.js: crypto.randomBytes(16) → Buffer.isBuffer === true
    // bao_runtime: returns object where Buffer.isBuffer === false
    let mut ctx = make_ctx();
    use common::eval_string;
    let r = eval_string(
        &mut ctx,
        r#"Buffer.isBuffer(require('crypto').randomBytes(8)) ? "PASS" : "FAIL""#,
    );
    assert_eq!(r, "PASS");
    bun_runtime::shutdown_thread_sm();
}

#[test]
fn test_crypto_conformance_hmac_md5_deviation() {
    let mut ctx = make_ctx();
    use common::eval_string;
    let r = eval_string(
        &mut ctx,
        r#"require('crypto').createHmac("md5", "key").update("x").digest("hex").length === 32 ? "PASS" : "FAIL""#,
    );
    assert_eq!(r, "PASS");
    bun_runtime::shutdown_thread_sm();
}
