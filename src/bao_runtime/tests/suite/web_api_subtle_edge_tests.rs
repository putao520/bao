// @trace TEST-ENG-6-SUBTLE-EDGE [req:REQ-ENG-006] [level:integration]
//
// Coverage-shortfall tests for web_api.rs (REQ-ENG-006): the crypto.subtle
// error matrix and the asymmetric success paths the existing subtle suites
// never reach, plus the TextDecoder label-family dispatch (latin1 / utf-16le
// / utf-16be / streaming carry / fatal / BOM) and the web_streams install
// face (install_web_streams_on_target has no product call site — the only
// way its lines execute is a direct install, which is also the honest probe
// that the installed constructors actually work).
//
// Every rejection expectation is checked in JS against the REAL rejection
// message (substring contract from the implementation); the Rust side
// asserts the verdict register contains no WRONG-ERR / UNEXPECTED verdict
// and that every case terminated.

use std::time::{Duration, Instant};

use bao_engine::context::JsContext;
use mozjs::rooted;
#[path = "common/mod.rs"]
mod common;

use common::eval_string;

fn drive_until(
    ctx: &mut JsContext,
    probe: &str,
    check: impl Fn(&str) -> bool,
    timeout: Duration,
) -> String {
    let deadline = Instant::now() + timeout;
    let mut last = String::new();
    while Instant::now() < deadline {
        let raw = ctx.raw_cx();
        unsafe {
            mozjs_sys::jsapi::js::RunJobs(raw);
        }
        bun_runtime::timers::with_event_loop(|loop_| {
            loop_.tick_without_idle(std::ptr::null_mut());
        });
        std::thread::sleep(Duration::from_millis(2));
        last = eval_string(ctx, probe);
        if check(&last) {
            return last;
        }
    }
    last
}

/// The crypto.subtle error matrix + asymmetric success paths. Each case is a
/// sequential chain link; verdicts land in `__R` as
/// `<name>:OK|REJECT-OK|WRONG-ERR:<msg>|UNEXPECTED-REJECT:<msg>`.
#[test]
fn subtle_error_matrix_and_asymmetric_success_paths() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let script = r#"
        (function () {
            var S = crypto.subtle;
            var nodeCrypto = require('crypto');
            var enc = new TextEncoder();
            var payload = enc.encode('subtle-edge-matrix-payload');
            function hex(u8) { var s = ''; var b = new Uint8Array(u8); for (var i = 0; i < b.length; i++) s += ('0' + b[i].toString(16)).slice(-2); return s; }

            globalThis.__R = [];
            globalThis.__done = 0;
            globalThis.__total = 0;

            function step(name, expectRej, body) {
                globalThis.__total++;
                var p = Promise.resolve().then(body);
                return p.then(function (v) {
                    if (expectRej) throw new Error('UNEXPECTED-RESOLVE (wanted: ' + expectRej + ')');
                    __R.push(name + ':OK');
                }, function (e) {
                    var msg = String(e && e.message || e);
                    if (!expectRej) __R.push(name + ':UNEXPECTED-REJECT:' + msg.substring(0, 70));
                    else if (msg.indexOf(expectRej) === -1) __R.push(name + ':WRONG-ERR:' + msg.substring(0, 90));
                    else __R.push(name + ':REJECT-OK');
                }).then(function () { __done++; });
            }

            var chain = Promise.resolve();
            function link(name, expectRej, body) {
                chain = chain.then(function () { return step(name, expectRej, body); });
            }

            // ── shared keys (imported first; later links consume them) ──
            var k192 = new Uint8Array(24); for (var i = 0; i < 24; i++) k192[i] = (i * 11 + 1) & 0xff;
            var k256 = new Uint8Array(32); for (var i = 0; i < 32; i++) k256[i] = (i * 7 + 3) & 0xff;
            var aes192 = null, aes256 = null, jwkAes = null, jwkHmac = null, rsaPair = null, ec256 = null, ec384 = null;

            // ════════ digest ════════
            link('digest-sha512-oracle', '', function () {
                return S.digest('SHA-512', payload).then(function (d) {
                    var expected = nodeCrypto.createHash('sha512').update('subtle-edge-matrix-payload').digest('hex');
                    if (hex(d) !== expected) throw new Error('sha512 mismatch: ' + hex(d));
                });
            });
            link('digest-unsupported', 'unsupported algorithm', function () { return S.digest('SHA3-512', payload); });
            link('digest-arity', 'requires 2 arguments', function () { return S.digest('SHA-256'); });
            link('digest-bad-alg-type', 'must be a string or {name}', function () { return S.digest(42, payload); });
            link('digest-no-name', 'unsupported algorithm', function () { return S.digest({}, payload); });

            // ════════ importKey (keys for later links) ════════
            link('imp-aes192', '', function () {
                return S.importKey('raw', k192, { name: 'AES-GCM' }, true, ['encrypt', 'decrypt'])
                    .then(function (k) { aes192 = k; });
            });
            link('imp-aes256', '', function () {
                return S.importKey('raw', k256, { name: 'AES-CBC' }, true, ['encrypt', 'decrypt'])
                    .then(function (k) { aes256 = k; });
            });
            link('imp-jwk-aes128', '', function () {
                // "000102...0f" → base64 of 16 bytes 0x00..0x0f
                var jwk = { kty: 'oct', k: 'AAECAwQFBgcICQoLDA0ODw' };
                return S.importKey('jwk', jwk, { name: 'AES-GCM' }, true, ['encrypt', 'decrypt'])
                    .then(function (k) { jwkAes = k; });
            });
            link('imp-jwk-hmac', '', function () {
                var jwk = { kty: 'oct', k: 'a2V5LW1hdGVyaWFs' }; // "key-material"
                return S.importKey('jwk', jwk, { name: 'HMAC', hash: 'SHA-256' }, false, ['sign', 'verify'])
                    .then(function (k) {
                        jwkHmac = k;
                        return S.sign('HMAC', k, payload);
                    }).then(function (sig) {
                        var expected = nodeCrypto.createHmac('sha256', Buffer.from('key-material'))
                            .update('subtle-edge-matrix-payload').digest('hex');
                        if (hex(sig) !== expected) throw new Error('jwk-hmac mismatch');
                    });
            });
            link('imp-rsa', '', function () {
                return S.generateKey(
                    { name: 'RSA-RSASSA-PKCS1-v1_5', modulusLength: 2048, hash: 'SHA-256' },
                    true, ['sign', 'verify']
                ).then(function (pair) { rsaPair = pair; });
            });
            link('imp-ec256', '', function () {
                return S.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, true, ['sign', 'verify'])
                    .then(function (pair) { ec256 = pair; });
            });
            link('imp-ec384', '', function () {
                return S.generateKey({ name: 'ECDSA', namedCurve: 'P-384' }, true, ['sign', 'verify'])
                    .then(function (pair) { ec384 = pair; });
            });

            link('imp-arity', 'requires 5 arguments', function () {
                return S.importKey('raw', k192, { name: 'AES-GCM' }, true);
            });
            // NOTE: the zero-length keyData case (importKey('raw', new
            // Uint8Array(0), …) → "raw import requires key bytes") stays OUT
            // of this matrix: it is pinned by the dedicated
            // `subtle_bytes_empty_buffersource_crash_defect` pin below (SUBTLE-BYTES
            // defect, registered + fixed 2026-10-16).
            link('imp-raw-aes-bad-len', 'invalid AES key length 64 bits', function () {
                return S.importKey('raw', new Uint8Array(8), { name: 'AES-GCM' }, true, ['encrypt']);
            });
            link('imp-raw-unsupported', 'subtle.importKey raw: unsupported algorithm DES', function () {
                return S.importKey('raw', k192, { name: 'DES' }, true, ['encrypt']);
            });
            // imp-pkcs8-empty / imp-spki-empty (empty `new Uint8Array(0)`
            // DER) are likewise OUT of this matrix — same registered+fixed
            // defect as the raw case above.
            link('imp-pkcs8-unsupported', 'subtle.importKey pkcs8: unsupported algorithm DES', function () {
                return S.importKey('pkcs8', new Uint8Array(4), { name: 'DES' }, true, ['sign']);
            });
            link('imp-bad-format', 'subtle.importKey: unsupported format pem', function () {
                return S.importKey('pem', k192, { name: 'AES-GCM' }, true, ['encrypt']);
            });
            link('imp-jwk-non-object', 'jwk import requires a JWK object', function () {
                return S.importKey('jwk', 'not-an-object', { name: 'AES-GCM' }, true, ['encrypt']);
            });
            link('imp-jwk-kty-rsa', 'is not supported (oct only)', function () {
                return S.importKey('jwk', { kty: 'RSA', n: 'x', e: 'y' }, { name: 'RSA-RSASSA-PKCS1-v1_5' }, true, ['sign']);
            });
            link('imp-jwk-missing-k', 'oct JWK requires the k field', function () {
                return S.importKey('jwk', { kty: 'oct' }, { name: 'AES-GCM' }, true, ['encrypt']);
            });
            link('imp-jwk-k-empty', 'oct JWK k field decoded to zero bytes', function () {
                return S.importKey('jwk', { kty: 'oct', k: '****' }, { name: 'AES-GCM' }, true, ['encrypt']);
            });

            // ════════ encrypt / decrypt ════════
            link('enc-arity', 'requires 3 arguments', function () { return S.encrypt({ name: 'AES-GCM' }, aes192); });
            link('enc-key-primitive', 'key must be a CryptoKey', function () {
                return S.encrypt({ name: 'AES-GCM', iv: new Uint8Array(12) }, 42, payload);
            });
            link('enc-key-no-material', 'not a symmetric CryptoKey', function () {
                return S.encrypt({ name: 'AES-GCM', iv: new Uint8Array(12) }, {}, payload);
            });
            link('enc-gcm-missing-iv', 'AES-GCM requires an iv', function () {
                return S.encrypt({ name: 'AES-GCM' }, aes192, payload);
            });
            link('enc-gcm-bad-taglen', 'tagLength 96 is not supported', function () {
                return S.encrypt({ name: 'AES-GCM', iv: new Uint8Array(12), tagLength: 96 }, aes192, payload);
            });
            link('enc-gcm-forged-key-len', 'invalid AES-GCM key length: 56 bits', function () {
                var forged = { _raw: new Uint8Array(7) };
                return S.encrypt({ name: 'AES-GCM', iv: new Uint8Array(12) }, forged, payload);
            });
            link('enc-unsupported', 'unsupported algorithm DES', function () { // outer match arm wins over aes_cipher_algo
                return S.encrypt({ name: 'DES', iv: new Uint8Array(8) }, aes192, payload);
            });
            link('enc-cbc-missing-iv', 'AES-CBC requires an iv', function () {
                return S.encrypt({ name: 'AES-CBC' }, aes256, payload);
            });
            link('dec-gcm-short-ct', 'AES-GCM ciphertext shorter than the auth tag', function () {
                return S.decrypt({ name: 'AES-GCM', iv: new Uint8Array(12) }, aes192, new Uint8Array(3));
            });
            link('dec-gcm-tampered', 'decryption failed', function () {
                return S.encrypt({ name: 'AES-GCM', iv: new Uint8Array(12) }, aes256, payload)
                    .then(function (ct) {
                        var bad = new Uint8Array(ct); bad[1] ^= 0xa5;
                        return S.decrypt({ name: 'AES-GCM', iv: new Uint8Array(12) }, aes256, bad);
                    });
            });
            link('dec-asym-key', 'not a symmetric CryptoKey', function () {
                return S.decrypt({ name: 'AES-GCM', iv: new Uint8Array(12) }, rsaPair.publicKey, payload);
            });
            // NOTE: an AES-192-GCM roundtrip is NOT here — it is pinned by the
            // dedicated `subtle_aes192gcm_aead_missing_defect` pin below (SUBTLE-
            // AES192GCM-NO-AEAD defect, registered + fixed 2026-10-16 by wiring
            // BoringSSL's EVP_aead_aes_192_gcm; the subtle layer always
            // advertised 192-bit GCM keys).
            link('aes256-cbc-roundtrip', '', function () {
                var iv = new Uint8Array(16); iv[15] = 7;
                return S.encrypt({ name: 'AES-CBC', iv: iv }, aes256, payload)
                    .then(function (ct) { return S.decrypt({ name: 'AES-CBC', iv: iv }, aes256, ct); })
                    .then(function (pt) {
                        if (hex(pt) !== hex(payload)) throw new Error('aes256-cbc roundtrip mismatch');
                    });
            });
            link('gcm-aad-roundtrip', '', function () {
                var iv = new Uint8Array(12); iv[0] = 9;
                var aad = enc.encode('associated-header');
                var alg = { name: 'AES-GCM', iv: iv, additionalData: aad };
                return S.encrypt(alg, aes256, payload)
                    .then(function (ct) { return S.decrypt(alg, aes256, ct); })
                    .then(function (pt) {
                        if (hex(pt) !== hex(payload)) throw new Error('gcm-aad roundtrip mismatch');
                    });
            });

            // ════════ generateKey ════════
            link('gen-usages-not-array', 'usages must be an array', function () {
                return S.generateKey({ name: 'AES-GCM', length: 256 }, true, 42);
            });
            link('gen-aes-no-length', 'AES generateKey requires algorithm.length', function () {
                return S.generateKey({ name: 'AES-GCM' }, true, ['encrypt']);
            });
            link('gen-aes-bad-length', 'invalid AES key length 100 (128/192/256)', function () {
                return S.generateKey({ name: 'AES-GCM', length: 100 }, true, ['encrypt']);
            });
            link('gen-unsupported', 'subtle.generateKey: unsupported algorithm DES', function () {
                return S.generateKey({ name: 'DES' }, true, ['encrypt']);
            });
            link('gen-rsa-no-modulus', 'RSA generateKey requires modulusLength', function () {
                return S.generateKey({ name: 'RSA-RSASSA-PKCS1-v1_5', hash: 'SHA-256' }, true, ['sign']);
            });
            link('gen-ecdsa-no-curve', 'ECDSA generateKey requires namedCurve', function () {
                return S.generateKey({ name: 'ECDSA' }, true, ['sign']);
            });
            link('gen-ecdsa-bad-curve', 'unsupported ECDSA curve P-521', function () {
                return S.generateKey({ name: 'ECDSA', namedCurve: 'P-521' }, true, ['sign']);
            });
            link('gen-rsa-shape', '', function () {
                if (!rsaPair || rsaPair.privateKey.type !== 'private' || rsaPair.publicKey.type !== 'public')
                    throw new Error('RSA key pair shape wrong');
                if (rsaPair.privateKey.algorithm.name !== 'RSA-RSASSA-PKCS1-v1_5') throw new Error('RSA alg name wrong');
            });
            link('gen-ecdsa-shape', '', function () {
                if (!ec256 || ec256.privateKey.type !== 'private') throw new Error('EC key pair shape wrong');
                if (ec256.privateKey.algorithm.namedCurve !== 'P-256') throw new Error('EC curve wrong');
            });

            // ════════ sign ════════
            link('sign-arity', 'requires 3 arguments', function () { return S.sign('HMAC', jwkHmac); });
            link('sign-key-primitive', 'sign: key must be a CryptoKey', function () {
                return S.sign('HMAC', 42, payload);
            });
            link('sign-key-no-alg', 'sign: not a CryptoKey', function () {
                return S.sign('HMAC', { _raw: new Uint8Array(4) }, payload);
            });
            link('sign-hmac-asym-key', 'sign: not a symmetric CryptoKey', function () {
                return S.sign('HMAC', rsaPair.publicKey, payload);
            });
            link('sign-hmac-bad-hash', 'unsupported HMAC hash MD5', function () {
                var forged = { type: 'secret', algorithm: { name: 'HMAC', hash: 'MD5' }, _raw: new Uint8Array(4) };
                return S.sign('HMAC', forged, payload);
            });
            link('sign-rsa-with-public', 'sign: not a private CryptoKey', function () {
                return S.sign('RSA-RSASSA-PKCS1-v1_5', rsaPair.publicKey, payload);
            });
            link('sign-ecdsa-bad-curve', 'unsupported ECDSA curve P-999', function () {
                var forged = { type: 'private', algorithm: { name: 'ECDSA', namedCurve: 'P-999' }, _der: new Uint8Array(4) };
                return S.sign('ECDSA', forged, payload);
            });

            // ════════ sign/verify success + tamper ════════
            link('rsa-sign-verify-true', '', function () {
                return S.sign('RSA-RSASSA-PKCS1-v1_5', rsaPair.privateKey, payload)
                    .then(function (sig) {
                        globalThis.__rsaSig = sig;
                        return S.verify('RSA-RSASSA-PKCS1-v1_5', rsaPair.publicKey, sig, payload);
                    }).then(function (ok) {
                        if (ok !== true) throw new Error('RSA verify returned ' + ok);
                    });
            });
            link('rsa-verify-tampered-false', '', function () {
                var bad = new Uint8Array(globalThis.__rsaSig); bad[5] ^= 0x11;
                return S.verify('RSA-RSASSA-PKCS1-v1_5', rsaPair.publicKey, bad, payload)
                    .then(function (ok) {
                        if (ok !== false) throw new Error('tampered RSA signature verified');
                    });
            });
            // NOTE: verify() with the PRIVATE-key CryptoKey is not here —
            // observed to return false for a valid signature (the
            // from_pkcs8_der verifier path); registered 2026-10-16 as a minor
            // defect observation (RSA-VERIFY-PRIVATE-KEY-FALSE), contract
            // direction unclear (browsers expose public-key verify only), so
            // no behavior is pinned.
            link('ecdsa-p256-sign-verify', '', function () {
                return S.sign('ECDSA', ec256.privateKey, payload)
                    .then(function (sig) {
                        if (sig.byteLength !== 64) throw new Error('P-256 raw sig length ' + sig.byteLength);
                        return S.verify('ECDSA', ec256.publicKey, sig, payload);
                    }).then(function (ok) {
                        if (ok !== true) throw new Error('ECDSA P-256 verify failed');
                    });
            });
            link('ecdsa-p384-sign-len', '', function () {
                return S.sign('ECDSA', ec384.privateKey, payload)
                    .then(function (sig) {
                        if (sig.byteLength !== 96) throw new Error('P-384 raw sig length ' + sig.byteLength);
                    });
            });

            // ════════ verify ════════
            link('verify-arity', 'requires 4 arguments', function () {
                return S.verify('HMAC', jwkHmac, new Uint8Array(4));
            });
            link('verify-key-primitive', 'verify: key must be a CryptoKey', function () {
                return S.verify('HMAC', null, new Uint8Array(4), payload);
            });
            link('verify-key-no-alg', 'verify: not a CryptoKey', function () {
                return S.verify('HMAC', { _raw: new Uint8Array(4) }, new Uint8Array(4), payload);
            });

            chain.then(function () { globalThis.__finished = true; });
            return 'queued:' + __total;
        })();
        "#;

    let queued = eval_string(&mut ctx, script);
    assert!(queued.starts_with("queued:"), "matrix did not queue: {queued}");

    let final_state = drive_until(
        &mut ctx,
        "__done + '/' + __total + '|' + __R.join(';;')",
        |s| s.starts_with("57/57|"),
        Duration::from_secs(30),
    );
    assert!(
        final_state.starts_with("57/57|"),
        "matrix did not terminate: {}",
        final_state
    );
    assert!(
        !final_state.contains(":WRONG-ERR:") && !final_state.contains(":UNEXPECTED"),
        "subtle matrix diverged from the message contract: {}",
        final_state
    );
    for verdict in final_state.split('|').nth(1).unwrap().split(";;") {
        assert!(
            verdict.ends_with(":OK") || verdict.ends_with(":REJECT-OK"),
            "non-terminal verdict: {}",
            verdict
        );
    }
}

/// TextDecoder label families, streaming carry, fatal mode, BOM handling and
/// the BufferSource input contract — the decode-dispatch branches behind
/// td_label_family / td_decode_utf8 / td_decode_utf16.
#[test]
fn text_decoder_label_families_and_streaming_edges() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let report = eval_string(
        &mut ctx,
        r#"
        (function () {
            var out = [];
            function check(label, fn) {
                try { out.push(label + '=' + fn()); }
                catch (e) { out.push(label + '=THREW:' + (e && e.name || e)); }
            }

            // ── latin1 family: byte→U+00XX one-to-one (NOT utf-8) ──
            check('latin1', function () {
                var d = new TextDecoder('latin1');
                return d.decode(new Uint8Array([0xc3, 0xa4])) === 'Ã¤' && d.encoding === 'latin1';
            });
            check('latin1-alias-windows1252', function () {
                return new TextDecoder('Windows-1252').decode(new Uint8Array([0xe9])) === 'é';
            });
            check('latin1-alias-iso', function () {
                // The constructor returns a plain object (no prototype link),
                // so the label face is probed via the .encoding echo.
                var codes = ['iso-8859-1', 'iso8859-1', 'l1', 'cp819', 'ascii', 'us-ascii', 'ansi_x3.4-1968'];
                return codes.every(function (c) { return new TextDecoder(c).encoding === c; });
            });

            // ── utf-16le: code units, surrogate pairs, lone-surrogate FFFD ──
            check('utf16le-basic', function () {
                return new TextDecoder('utf-16le').decode(new Uint8Array([0x34, 0x00, 0x41, 0x00])) === '4A';
            });
            check('utf16le-surrogate-pair', function () {
                // U+1F600 = D83D DE00 (LE: 3D D8 00 DE)
                return new TextDecoder('utf-16le').decode(new Uint8Array([0x3d, 0xd8, 0x00, 0xde])) === '😀';
            });
            check('utf16le-lone-high-surrogate', function () {
                // lone high surrogate mid-stream → U+FFFD
                return new TextDecoder('utf-16le').decode(new Uint8Array([0x3d, 0xd8, 0x41, 0x00])) === '�A';
            });
            check('utf16le-lone-low-surrogate', function () {
                return new TextDecoder('utf-16le').decode(new Uint8Array([0x00, 0xde])) === '�';
            });
            check('utf16le-odd-tail-eos', function () {
                // odd trailing byte at end of stream → U+FFFD
                return new TextDecoder('utf-16le').decode(new Uint8Array([0x41, 0x00, 0x42])) === 'A�';
            });
            check('utf16le-odd-tail-streaming-held', function () {
                var d = new TextDecoder('utf-16le');
                var first = d.decode(new Uint8Array([0x41, 0x00, 0x42]), { stream: true });
                var second = d.decode(new Uint8Array([0x00]));
                return first === 'A' && second === 'B';
            });
            check('utf16be-basic', function () {
                return new TextDecoder('utf-16be').decode(new Uint8Array([0x00, 0x34])) === '4';
            });
            check('utf16be-alias-unicodefffe', function () {
                return new TextDecoder('UnicodeFFFE').decode(new Uint8Array([0x00, 0x7a])) === 'z';
            });

            // ── utf-8 streaming: truncated sequence is HELD, not replaced ──
            check('utf8-streaming-carry', function () {
                var d = new TextDecoder();
                var first = d.decode(new Uint8Array([0xe4, 0xb8]), { stream: true });
                var second = d.decode(new Uint8Array([0xad]));
                return first === '' && second === '中';
            });
            check('utf8-truncated-at-eos-lossy', function () {
                // stream:false with a truncated tail → U+FFFD (no throw when not fatal)
                return new TextDecoder().decode(new Uint8Array([0xe4, 0xb8])) === '�';
            });
            check('utf8-fatal-invalid-throws', function () {
                var d = new TextDecoder('utf-8', { fatal: true });
                try { d.decode(new Uint8Array([0xff, 0xfe, 0xfd])); return 'no-throw'; }
                catch (e) { return e instanceof TypeError ? 'typeerror' : 'wrong:' + e; }
            });
            check('utf8-fatal-truncated-throws', function () {
                var d = new TextDecoder('utf-8', { fatal: true });
                try { d.decode(new Uint8Array([0xe4, 0xb8])); return 'no-throw'; }
                catch (e) { return e instanceof TypeError ? 'typeerror' : 'wrong:' + e; }
            });
            check('utf8-fatal-valid-ok', function () {
                return new TextDecoder('utf-8', { fatal: true }).decode(new Uint8Array([0xe4, 0xb8, 0xad])) === '中';
            });

            // ── BOM handling ──
            check('bom-stripped-by-default', function () {
                return new TextDecoder().decode(new Uint8Array([0xef, 0xbb, 0xbf, 0x41])) === 'A';
            });
            check('bom-ignore-keeps-feff', function () {
                return new TextDecoder('utf-8', { ignoreBOM: true }).decode(new Uint8Array([0xef, 0xbb, 0xbf, 0x41])) === '﻿A';
            });
            check('bom-utf16le-stripped', function () {
                return new TextDecoder('utf-16le').decode(new Uint8Array([0xff, 0xfe, 0x41, 0x00])) === 'A';
            });
            check('bom-prefix-streaming-held', function () {
                var d = new TextDecoder();
                var first = d.decode(new Uint8Array([0xef]), { stream: true });
                var second = d.decode(new Uint8Array([0xbb, 0xbf, 0x42]), { stream: true });
                return first === '' && second === 'B';
            });
            check('bom-latin1-no-bom-domain', function () {
                // latin1 family has no BOM — FF FE decodes as two latin1 chars
                return new TextDecoder('latin1').decode(new Uint8Array([0xff, 0xfe])) === 'ÿþ';
            });

            // ── input contract: BufferSource or TypeError ──
            check('decode-non-buffersource-throws', function () {
                try { new TextDecoder().decode(5); return 'no-throw'; }
                catch (e) { return e instanceof TypeError ? 'typeerror' : 'wrong'; }
            });
            check('decode-undefined-input-empty', function () {
                return new TextDecoder().decode(undefined) === '';
            });
            check('decode-arraybuffer-input', function () {
                var ab = new Uint8Array([0x41]).buffer;
                return new TextDecoder().decode(ab) === 'A';
            });

            // ── unknown labels keep the legacy utf-8 echo face ──
            check('unknown-label-echoes', function () {
                var d = new TextDecoder('klingon');
                return d.encoding === 'klingon' && d.decode(new Uint8Array([0x41])) === 'A';
            });

            // ── queueMicrotask callable contract ──
            check('queueMicrotask-non-callable', function () {
                try { queueMicrotask(42); return 'no-throw'; }
                catch (e) { return e instanceof TypeError ? 'typeerror' : 'wrong'; }
            });

            // ── encodeInto boundary: complete code points only ──
            check('encodeInto-exact-fit', function () {
                var te = new TextEncoder();
                var dest = new Uint8Array(3);
                var r = te.encodeInto('abc', dest);
                return r.read === 3 && r.written === 3;
            });
            check('encodeInto-partial-multibyte-stops-complete', function () {
                var te = new TextEncoder();
                var dest = new Uint8Array(4); // '中' is 3 bytes; next '文' would not fit
                var r = te.encodeInto('中文', dest);
                return r.read === 1 && r.written === 3 && dest[0] === 0xe4;
            });
            check('encodeInto-non-uint8-throws', function () {
                var te = new TextEncoder();
                try { te.encodeInto('x', {}); return 'no-throw'; }
                catch (e) { return 'threw'; }
            });

            return out.join(';;');
        })();
        "#,
    );

    let failures: Vec<&str> = report
        .split(";;")
        .filter(|v| !v.ends_with("=true") && !v.ends_with("=typeerror") && !v.ends_with("=threw"))
        .collect();
    assert!(
        failures.is_empty(),
        "TextDecoder label-family dispatch diverged: {report}"
    );
    assert_eq!(report.split(";;").count(), 29, "case count drifted: {report}");
}

/// web_streams.rs install face: `install_web_streams_on_target` has no
/// product call site, so its lines only execute via a direct install. The
/// honest probe is behavioral: after the install the global carries all 17
/// constructors and the streams actually transport data (read roundtrip,
/// identity transform, writer error propagation).
#[test]
fn web_streams_install_and_on_target_face() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    // Warm the realm: thread_realm_global resolves only after JS has run.
    assert_eq!(eval_string(&mut ctx, "'warm'"), "warm");

    let global = bao_engine::context::thread_realm_global()
        .filter(|g| !g.is_null())
        .expect("thread realm global must resolve after eval");
    {
        let mut cxm = ctx.cx();
        rooted!(&in(cxm) let g_root = global);
        let mut realm = mozjs::realm::AutoRealm::new_from_handle(&mut cxm, g_root.handle());
        let realm_cx: &mut mozjs::context::JSContext = &mut realm;
        // Target = the global itself: the copy loop (17 names global→target)
        // runs, and the result is directly observable from JS.
        bun_runtime::web_streams::install_web_streams_on_target(realm_cx, g_root.handle());
    }

    let report = eval_string(
        &mut ctx,
        r#"
        (function () {
            var names = ['ReadableStream', 'ReadableStreamDefaultReader', 'ReadableStreamBYOBReader',
                'ReadableStreamDefaultController', 'ReadableByteStreamController', 'ReadableStreamBYOBRequest',
                'WritableStream', 'WritableStreamDefaultWriter', 'WritableStreamDefaultController',
                'TransformStream', 'TransformStreamDefaultController',
                'ByteLengthQueuingStrategy', 'CountQueuingStrategy',
                'TextEncoderStream', 'TextDecoderStream', 'CompressionStream', 'DecompressionStream'];
            var missing = names.filter(function (n) { return typeof globalThis[n] !== 'function'; });
            if (missing.length) return 'MISSING:' + missing.join(',');

            globalThis.__sr = [];
            globalThis.__sdone = 0;
            globalThis.__stotal = 3;

            function step(name, body) {
                return Promise.resolve()
                    .then(body)
                    .then(function (v) {
                        __sr.push(name + ':' + (v === true ? 'OK' : 'FAIL:' + v));
                    }, function (e) {
                        __sr.push(name + ':ERR:' + String(e && e.message || e).substring(0, 50));
                    })
                    .then(function () { __sdone++; });
            }

            // Read roundtrip: enqueue → close → read yields value then done.
            step('read-roundtrip', function () {
                var rs = new ReadableStream({
                    start: function (c) { c.enqueue('chunk-a'); c.close(); }
                });
                var reader = rs.getReader();
                return reader.read().then(function (first) {
                    if (first.done || first.value !== 'chunk-a') return 'bad-first:' + JSON.stringify(first);
                    return reader.read().then(function (second) {
                        return second.done === true ? true : 'not-closed';
                    });
                });
            });

            // Identity transform: write('tf-x') → readable side yields it.
            step('transform-identity', function () {
                var ts = new TransformStream();
                var writer = ts.writable.getWriter();
                var reader = ts.readable.getReader();
                var writeP = writer.write('tf-x').then(function () { return writer.close(); });
                return reader.read().then(function (first) {
                    return writeP.then(function () {
                        return (!first.done && first.value === 'tf-x') ? true : 'bad-transform-value';
                    });
                });
            });

            // Writer error propagation: a throwing sink rejects write() with
            // the sink's error (error branch of the polyfill).
            step('writer-error-propagation', function () {
                var ws = new WritableStream({
                    write: function () { throw new Error('sink-boom'); }
                });
                var writer = ws.getWriter();
                return writer.write('x').then(
                    function () { return 'write-should-reject'; },
                    function (e) { return String(e && e.message) === 'sink-boom'; }
                );
            });

            return 'installed';
        })();
        "#,
    );
    assert_eq!(report, "installed", "constructor set incomplete");

    let final_state = drive_until(
        &mut ctx,
        "__sdone + '/' + __stotal + '|' + __sr.join(';;')",
        |s| s.starts_with("3/3|"),
        Duration::from_secs(15),
    );
    // The three cases settle within the first drain pass in a
    // nondeterministic order — compare sorted verdicts.
    let mut verdicts: Vec<String> = final_state
        .split('|')
        .nth(1)
        .unwrap()
        .split(";;")
        .map(str::to_owned)
        .collect();
    verdicts.sort();
    assert_eq!(
        verdicts.join(";;"),
        "read-roundtrip:OK;;transform-identity:OK;;writer-error-propagation:OK",
        "streams behavior diverged: {final_state}"
    );
}

/// REGRESSION PIN (SUBTLE-BYTES-EMPTY-BUFFERSOURCE-CRASH, registered +
/// fixed 2026-10-16) — post-fix contract probe; was an #[ignore]d crash
/// repro while the defect was open.
///
/// Pre-fix mechanism: `subtle_bytes` (bao_runtime/src/web_api.rs) gated its
/// TypedArray and ArrayBufferView fast paths on `length > 0`, so a
/// ZERO-length BufferSource fell through to the bare-ArrayBuffer fallback,
/// which called `JS::GetArrayBufferByteLength` UNCONDITIONALLY on the
/// object. That SM API requires a real ArrayBuffer
/// (`maybeUnwrapAs<ArrayBufferObject>`); on a zero-length Uint8Array (or
/// any non-ArrayBuffer object — e.g. a plain object passed as
/// `subtle.digest` data) the unwrap assertion fired and the runtime died
/// with SIGSEGV (MOZ_CrashSequence; opt profile included — the trap is
/// unconditional). Reachable from every subtle entry point that consumes
/// BufferSource data with attacker-controlled input: hostile-page crash
/// vector.
///
/// Fix (landed): a successful view unwrap owns the answer — zero-length /
/// detached views read as an empty Vec and never fall through; the bare leg
/// is guarded by IsArrayBufferObjectMaybeShared. importKey then rejects
/// with "raw import requires key bytes" (as the error-path links in the
/// matrix above expect).
#[test]
fn subtle_bytes_empty_buffersource_crash_defect() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    // The verdict lands in a global once the promise chain settles; drive the
    // job queue + event loop until it does (a bare eval_string on the chain
    // IIFE would just stringify the pending Promise).
    eval_string(
        &mut ctx,
        r#"
        (function () {
            crypto.subtle
                .importKey('raw', new Uint8Array(0), { name: 'AES-GCM' }, true, ['encrypt'])
                .then(
                    function () { globalThis.__verdict = 'RESOLVED (unexpected)'; },
                    function (e) { globalThis.__verdict = 'REJECTED:' + String(e && e.message || e); }
                );
            return 'queued';
        })();
        "#,
    );
    drive_until(
        &mut ctx,
        "globalThis.__verdict || ''",
        |s| !s.is_empty(),
        Duration::from_secs(10),
    );
    let verdict = eval_string(&mut ctx, "globalThis.__verdict || ''");
    // Post-fix contract: a clean promise rejection, never a crash.
    assert_eq!(
        verdict,
        "REJECTED:subtle.importKey: raw import requires key bytes",
        "empty keyData must reject cleanly, got: {verdict}"
    );
}

/// REGRESSION PIN (SUBTLE-AES192GCM-NO-AEAD, registered + fixed
/// 2026-10-16) — post-fix contract probe; was an #[ignore]d repro while the
/// defect was open.
///
/// Pre-fix mechanism: the subtle layer ADVERTISED AES-192-GCM —
/// `aes_cipher_algo` (bao_runtime/src/web_api.rs) accepts 192-bit GCM keys,
/// importKey validates 192 as legal, generateKey offers length:192 — but
/// `bao_crypto::cipher::CipherAlgorithm::Aes192Gcm::aead_cipher()` returned
/// None, so every actual AES-192-GCM encrypt/decrypt rejected with
/// "encrypt() requires an AEAD cipher". The None arm cited a stale
/// "BoringSSL ships no EVP_aead_aes_192_gcm" belief: the vendored BoringSSL
/// DOES export it (include/openssl/aead.h, key_len 24, shared GCM
/// sealv/openv_detached). Fix (landed): wire the real getter via a local
/// extern binding (the TASK-1-CRYPTO file-domain rule — never edit
/// bun_boringssl_sys). Rejection was never an option: the W3C WebCrypto
/// spec mandates 128/192/256-bit keys for AES-GCM (importKey throws
/// DataError otherwise).
#[test]
fn subtle_aes192gcm_aead_missing_defect() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    // The verdict lands in a global once the promise chain settles; drive the
    // job queue + event loop until it does (a bare eval_string on the chain
    // IIFE would just stringify the pending Promise). The imported key is
    // captured in `key` — the chain's later links run in their own handler
    // scopes where the first handler's `k` parameter is invisible.
    eval_string(
        &mut ctx,
        r#"
        (function () {
            var S = crypto.subtle;
            var k192 = new Uint8Array(24); for (var i = 0; i < 24; i++) k192[i] = i;
            var iv = new Uint8Array(12);
            var key = null;
            S.importKey('raw', k192, { name: 'AES-GCM' }, true, ['encrypt', 'decrypt'])
                .then(function (k) {
                    key = k;
                    return S.encrypt({ name: 'AES-GCM', iv: iv }, key, new TextEncoder().encode('x'));
                })
                .then(function (ct) { return S.decrypt({ name: 'AES-GCM', iv: iv }, key, ct); })
                .then(
                    function () { globalThis.__verdict = 'ROUNDTRIP-OK'; },
                    function (e) { globalThis.__verdict = 'REJECTED:' + String(e && e.message || e); }
                );
            return 'queued';
        })();
        "#,
    );
    drive_until(
        &mut ctx,
        "globalThis.__verdict || ''",
        |s| !s.is_empty(),
        Duration::from_secs(10),
    );
    let verdict = eval_string(&mut ctx, "globalThis.__verdict || ''");
    // Post-fix contract: a full AES-192-GCM roundtrip.
    assert_eq!(verdict, "ROUNDTRIP-OK", "AES-192-GCM still broken: {verdict}");
}
