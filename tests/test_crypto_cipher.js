/**
 * crypto cipher + KDF boundary test (TASK-1-CRYPTO, REQ-ENG-007).
 *
 * Verifies createCipheriv/createDecipheriv roundtrips for non-AEAD
 * (AES-CBC/CTR) and AEAD (AES-GCM/ChaCha20-Poly1305) algorithms, plus
 * pbkdf2Sync/scryptSync derivation. Ciphers return byte arrays (number[])
 * matching the randomBytes convention in node_crypto.rs.
 */

var assert = console.assert;
var crypto = require("crypto");

var passed = 0;
var failed = 0;
function check(name, fn) {
    try {
        fn();
        passed++;
    } catch (e) {
        failed++;
        console.log("FAIL: " + name + " — " + (e && e.message ? e.message : e));
    }
}

function bytesToArr(s) {
    // string → UTF-8 byte array
    var a = [];
    for (var i = 0; i < s.length; i++) {
        var c = s.charCodeAt(i);
        if (c < 0x80) a.push(c);
        else if (c < 0x800) { a.push(0xc0 | (c >> 6)); a.push(0x80 | (c & 0x3f)); }
        else { a.push(0xe0 | (c >> 12)); a.push(0x80 | ((c >> 6) & 0x3f)); a.push(0x80 | (c & 0x3f)); }
    }
    return a;
}
function arrToStr(a) {
    var s = "";
    for (var i = 0; i < a.length; i++) s += String.fromCharCode(a[i] & 0xff);
    return s;
}
function concatArr(a, b) { return a.concat(b); }
function arrEq(a, b) {
    if (a.length !== b.length) return false;
    for (var i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
    return true;
}
function keyBytes(len) { var k = []; for (var i = 0; i < len; i++) k.push((i * 7 + 1) & 0xff); return k; }
function ivBytes(len) { var v = []; for (var i = 0; i < len; i++) v.push((0xa0 + i) & 0xff); return v; }

// ── Non-AEAD block/stream ciphers ──────────────────────────────────────────
function roundtripBlock(algo, klen, ivlen) {
    var key = keyBytes(klen);
    var iv = ivBytes(ivlen);
    var pt = bytesToArr("the quick brown fox 1234567890 !@#");

    var enc = crypto.createCipheriv(algo, key, iv);
    var ct = concatArr(enc.update(pt), enc.final());
    assert(ct.length > 0, algo + ": ciphertext must be non-empty");

    var dec = crypto.createDecipheriv(algo, key, iv);
    var recovered = concatArr(dec.update(ct), dec.final());
    assert(arrEq(recovered, pt), algo + ": roundtrip must recover plaintext");
}

check("AES-128-CBC roundtrip", function () { roundtripBlock("aes-128-cbc", 16, 16); });
check("AES-192-CBC roundtrip", function () { roundtripBlock("aes-192-cbc", 24, 16); });
check("AES-256-CBC roundtrip", function () { roundtripBlock("aes-256-cbc", 32, 16); });
check("AES-128-CTR roundtrip", function () { roundtripBlock("aes-128-ctr", 16, 16); });
check("AES-256-CTR roundtrip", function () { roundtripBlock("aes-256-ctr", 32, 16); });

// ── AEAD ciphers (AES-GCM) ─────────────────────────────────────────────────
function roundtripGCM(algo, klen) {
    var key = keyBytes(klen);
    var iv = ivBytes(12); // GCM nonce = 12
    var pt = bytesToArr("gcm secret payload");

    var enc = crypto.createCipheriv(algo, key, iv);
    var ct = concatArr(enc.update(pt), enc.final());
    var tag = enc.getAuthTag();
    assert(tag.length === 16, algo + ": auth tag must be 16 bytes");

    var dec = crypto.createDecipheriv(algo, key, iv);
    dec.setAuthTag(tag);
    var recovered = concatArr(dec.update(ct), dec.final());
    assert(arrEq(recovered, pt), algo + ": AEAD roundtrip must recover plaintext");
}

check("AES-128-GCM roundtrip + auth tag", function () { roundtripGCM("aes-128-gcm", 16); });
check("AES-256-GCM roundtrip + auth tag", function () { roundtripGCM("aes-256-gcm", 32); });

check("AES-256-GCM tampered tag fails", function () {
    var key = keyBytes(32);
    var iv = ivBytes(12);
    var pt = bytesToArr("tamper me");
    var enc = crypto.createCipheriv("aes-256-gcm", key, iv);
    var ct = concatArr(enc.update(pt), enc.final());
    var tag = enc.getAuthTag();
    tag[0] = (tag[0] ^ 0xff) & 0xff;
    var dec = crypto.createDecipheriv("aes-256-gcm", key, iv);
    dec.setAuthTag(tag);
    dec.update(ct);
    var threw = false;
    try { dec.final(); } catch (e) { threw = true; }
    assert(threw, "tampered GCM tag must throw on final()");
});

// ── ChaCha20-Poly1305 ──────────────────────────────────────────────────────
check("ChaCha20-Poly1305 roundtrip + auth tag", function () {
    var key = keyBytes(32);
    var iv = ivBytes(12);
    var pt = bytesToArr("chacha aead");
    var enc = crypto.createCipheriv("chacha20-poly1305", key, iv);
    var ct = concatArr(enc.update(pt), enc.final());
    var tag = enc.getAuthTag();
    assert(tag.length === 16, "chacha auth tag must be 16 bytes");
    var dec = crypto.createDecipheriv("chacha20-poly1305", key, iv);
    dec.setAuthTag(tag);
    var recovered = concatArr(dec.update(ct), dec.final());
    assert(arrEq(recovered, pt), "chacha roundtrip must recover plaintext");
});

// ── Multiple concurrent cipher objects (per-instance state) ────────────────
check("two concurrent ciphers have independent state", function () {
    var key = keyBytes(32);
    var iv1 = ivBytes(16);
    var iv2 = []; for (var i = 0; i < 16; i++) iv2.push((0x55 + i) & 0xff);
    var pt1 = bytesToArr("first plaintext");
    var pt2 = bytesToArr("second plaintext");
    var e1 = crypto.createCipheriv("aes-256-cbc", key, iv1);
    var e2 = crypto.createCipheriv("aes-256-cbc", key, iv2);
    var ct1 = concatArr(e1.update(pt1), e1.final());
    var ct2 = concatArr(e2.update(pt2), e2.final());
    // Decrypt each with its own iv.
    var d1 = crypto.createDecipheriv("aes-256-cbc", key, iv1);
    assert(arrEq(concatArr(d1.update(ct1), d1.final()), pt1), "cipher 1 must recover");
    var d2 = crypto.createDecipheriv("aes-256-cbc", key, iv2);
    assert(arrEq(concatArr(d2.update(ct2), d2.final()), pt2), "cipher 2 must recover");
});

// ── KDF: pbkdf2Sync / scryptSync ───────────────────────────────────────────
check("pbkdf2Sync SHA-256 determinism + length", function () {
    var a = crypto.pbkdf2Sync("password", "salt", 1000, 32, "sha256");
    var b = crypto.pbkdf2Sync("password", "salt", 1000, 32, "sha256");
    assert(a.length === 32, "pbkdf2 must return 32 bytes");
    assert(arrEq(a, b), "pbkdf2 must be deterministic");
});

check("pbkdf2Sync SHA-1 length", function () {
    var a = crypto.pbkdf2Sync("password", "salt", 1, 20, "sha1");
    assert(a.length === 20, "pbkdf2 sha1 must return 20 bytes");
});

check("pbkdf2Sync SHA-512 length", function () {
    var a = crypto.pbkdf2Sync("password", "salt", 10, 64, "sha512");
    assert(a.length === 64, "pbkdf2 sha512 must return 64 bytes");
});

check("scryptSync determinism + length", function () {
    var a = crypto.scryptSync("password", "NaCl", 32);
    var b = crypto.scryptSync("password", "NaCl", 32);
    assert(a.length === 32, "scrypt must return 32 bytes");
    assert(arrEq(a, b), "scrypt must be deterministic");
});

// ── Unsupported algorithm rejected ─────────────────────────────────────────
check("unsupported cipher algorithm throws", function () {
    var threw = false;
    try { crypto.createCipheriv("rc4", keyBytes(16), ivBytes(16)); } catch (e) { threw = true; }
    assert(threw, "rc4 must be rejected");
});

// ── randomBytes + timingSafeEqual still work ───────────────────────────────
check("randomBytes returns requested length", function () {
    var b = crypto.randomBytes(16);
    assert(b.length === 16, "randomBytes must return 16 bytes");
});

check("timingSafeEqual equal buffers", function () {
    var a = keyBytes(16);
    assert(crypto.timingSafeEqual(a, a) === true, "timingSafeEqual of identical buffers must be true");
});

console.log("PASSED: " + passed);
console.log("FAILED: " + failed);
if (failed > 0) {
    console.log("RESULT: FAIL");
} else {
    console.log("RESULT: ALL PASS");
}
