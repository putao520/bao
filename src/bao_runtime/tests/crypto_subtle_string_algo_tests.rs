// @trace TEST-ENG-007-SUBTLE [req:REQ-ENG-006] [level:e2e]
// crypto.subtle string-form algorithm identifiers (#e-gap):
//
// WebCrypto AlgorithmIdentifier is `(Algorithm or DOMString)` — a bare
// 'HMAC' string is the exact equivalent of {name: 'HMAC'} (browsers and
// Node webcrypto both coerce). Pre-fix, every subtle entry point except
// digest() rejected strings with "algorithm identifier must be an object".
//
// Matrix below (all via bare STRING identifiers, object-form cross-checked):
//   S1 sign('HMAC', key, data) — HMAC-SHA-256, byte-compared against
//      node:crypto createHmac (ground truth), not just self-consistent
//   S2 verify('HMAC', key, sig, data) — true + tamper-false
//   S3 importKey('raw', keyData, 'HMAC'-with-hash-object …) — the string
//      algorithm at import (hash still required, like Chrome)
//   S4 digest('SHA-256', …) string form (pre-existing, pinned here)
//   S5 SHA-384 HMAC via string sign + verify round trip
//   S6 string identifier with missing params errors the same as the object
//      form ('AES-GCM' string encrypt → "requires an iv", never silent OK)

use std::time::{Duration, Instant};

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
use bun_runtime::timers;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(JsValue::Null) => "null".to_string(),
        Ok(JsValue::Undefined) => "undefined".to_string(),
        Ok(JsValue::Object(_)) => "[object]".to_string(),
        Err(e) => format!("ERROR:{}", e.message),
    }
}

fn drive_until(
    ctx: &mut JsContext,
    probe: &str,
    check: impl Fn(&str) -> bool,
    timeout: Duration,
) -> String {
    let cx_raw = ctx.raw_cx();
    let deadline = Instant::now() + timeout;
    let mut last = String::new();
    while Instant::now() < deadline {
        unsafe {
            mozjs_sys::jsapi::js::RunJobs(cx_raw);
        }
        timers::with_event_loop(|loop_| {
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

#[test]
fn test_subtle_string_algorithm_identifiers() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let setup = eval_string(
        &mut ctx,
        r#"
        (function() {
            globalThis.__errs = [];
            globalThis.__done = 0;
            globalThis.__total = 6;
            function step(name, p) {
                return p.then(function() { globalThis.__done++; })
                        .catch(function(e) { globalThis.__errs.push(name + ': ' + (e && e.message || String(e))); globalThis.__done++; });
            }
            function hex(u8) { var s = ''; for (var i = 0; i < u8.length; i++) s += ('0' + u8[i].toString(16)).slice(-2); return s; }
            var enc = new TextEncoder();
            var S = crypto.subtle;
            var nodeCrypto = require('crypto');

            var keyBytes = new Uint8Array(32);
            for (var i = 0; i < 32; i++) keyBytes[i] = (i * 7 + 3) & 0xff;
            var payloadStr = 'string-algorithm-identifier-payload';
            var payload = enc.encode(payloadStr);

            // S1/S2: string-form HMAC sign + verify, byte-compared against
            // node:crypto createHmac (ground truth) and the object form.
            var s1 = step('S1-hmac-string-sign', S.importKey('raw', keyBytes, {name:'HMAC', hash:'SHA-256'}, false, ['sign','verify'])
            .then(function(k) {
                globalThis.__hmacKey = k;
                return S.sign('HMAC', k, payload);                     // STRING identifier
            }).then(function(sig) {
                var expected = nodeCrypto.createHmac('sha256', Buffer.from(keyBytes)).update(payloadStr).digest('hex');
                var got = hex(new Uint8Array(sig));
                if (got !== expected) throw new Error('string-form HMAC mismatch vs createHmac: ' + got + ' != ' + expected);
                globalThis.__sig = sig;
                return S.sign({name:'HMAC'}, globalThis.__hmacKey, payload);  // object form must match
            }).then(function(sigObj) {
                if (hex(new Uint8Array(sigObj)) !== hex(new Uint8Array(globalThis.__sig)))
                    throw new Error('string vs object form diverged');
            }));

            // S2 chains on S1 (needs the signature S1 produced).
            step('S2-hmac-string-verify', s1
            .then(function() {
                return S.verify('HMAC', globalThis.__hmacKey, globalThis.__sig, payload);   // STRING
            }).then(function(ok) {
                if (ok !== true) throw new Error('string-form verify failed');
                var bad = new Uint8Array(globalThis.__sig); bad[3] ^= 0x40;
                return S.verify('HMAC', globalThis.__hmacKey, bad, payload);
            }).then(function(okT) {
                if (okT) throw new Error('tampered HMAC verified (must be false)');
                return S.verify('HMAC', globalThis.__hmacKey, globalThis.__sig, enc.encode('other-data'));
            }).then(function(okW) {
                if (okW) throw new Error('wrong-payload HMAC verified (must be false)');
            }));

            // S3: string algorithm at importKey — 'HMAC' alone lacks hash and
            // must reject exactly like the object form without hash (Chrome
            // behavior); with hash via object it imports fine and string-sign
            // picks the hash from the KEY.
            step('S3-import-string-alg', Promise.resolve()
            .then(function() {
                return S.importKey('raw', keyBytes, 'HMAC', false, ['sign']);   // STRING, no hash anywhere
            }).then(function() {
                throw new Error('bare-HMAC import without hash must reject');
            }, function(e) {
                if (String(e.message).indexOf('algorithm.hash') === -1) throw e;
            }).then(function() {
                // import with OBJECT alg (hash present), then SIGN with string:
                // the hash must come from the key's algorithm.
                return S.importKey('raw', keyBytes, {name:'HMAC', hash:'SHA-384'}, false, ['sign','verify']);
            }).then(function(k384) {
                return S.sign('HMAC', k384, payload);
            }).then(function(sig384) {
                if (sig384.byteLength !== 48) throw new Error('SHA-384 HMAC length: ' + sig384.byteLength);
                var expected = nodeCrypto.createHmac('sha384', Buffer.from(keyBytes)).update(payloadStr).digest('hex');
                if (hex(new Uint8Array(sig384)) !== expected) throw new Error('string-form HMAC-SHA384 mismatch vs createHmac');
            }));

            // S5: string identifier round trip verify with SHA-384 key.
            step('S5-verify-384', Promise.resolve()
            .then(function() {
                return S.importKey('raw', keyBytes, {name:'HMAC', hash:'SHA-384'}, false, ['sign','verify']);
            }).then(function(k) {
                return S.sign('HMAC', k, enc.encode('384-rt')).then(function(sig) {
                    return S.verify('HMAC', k, sig, enc.encode('384-rt')).then(function(ok) {
                        if (!ok) throw new Error('384 string verify failed');
                        return S.verify('HMAC', k, sig, enc.encode('384-rT'));
                    }).then(function(okT) {
                        if (okT) throw new Error('384 tampered verified');
                    });
                });
            }));

            // S4: digest string form pinned (pre-existing surface).
            step('S4-digest-string', S.digest('SHA-256', enc.encode('abc')).then(function(d) {
                if (d.byteLength !== 32) throw new Error('digest length');
                var known = nodeCrypto.createHash('sha256').update('abc').digest('hex');
                if (hex(new Uint8Array(d)) !== known) throw new Error('digest value mismatch: ' + hex(new Uint8Array(d)) + ' != ' + known);
            }));

            // S6: string identifier carrying no params behaves EXACTLY like
            // the param-less object form — AES-GCM without iv rejects.
            step('S6-string-no-params', Promise.resolve()
            .then(function() {
                return S.importKey('raw', new Uint8Array(32), {name:'AES-GCM'}, false, ['encrypt']);
            }).then(function(k) {
                return S.encrypt('AES-GCM', k, enc.encode('x'));      // STRING, no iv available
            }).then(function() {
                throw new Error('string AES-GCM without iv must reject');
            }, function(e) {
                if (String(e.message).indexOf('iv') === -1) throw e;
            }));

            return 'scheduled';
        })()
        "#,
    );
    assert!(setup.contains("scheduled"), "subtle string-algo setup failed: {}", setup);
    let status = drive_until(
        &mut ctx,
        r#"globalThis.__done + ':' + globalThis.__errs.length"#,
        |s| s.starts_with(&format!("{}:", 6)),
        Duration::from_secs(30),
    );
    let errs = eval_string(&mut ctx, r#"JSON.stringify(globalThis.__errs)"#);
    assert!(
        status.starts_with("6:0"),
        "subtle string-identifier steps failed: status={} errs={}",
        status,
        errs
    );
}
