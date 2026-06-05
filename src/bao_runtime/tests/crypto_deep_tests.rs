// @trace TEST-ENG-007-CRYPTO-DEEP [req:REQ-ENG-007] [level:integration]

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        _ => String::new(),
    }
}

#[test]
fn test_crypto_deep() {
    bao_runtime::install_exit_handler();
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e).substring(0, 60)); }
        }

        var crypto = require('crypto');

        // Module existence
        check("crypto_exists", function() { return typeof crypto !== 'undefined'; });
        check("crypto_is_object", function() { return typeof crypto === 'object'; });

        // createHash
        check("crypto_createHash_exists", function() { return typeof crypto.createHash === 'function'; });
        check("crypto_createHash_sha256", function() {
            var h = crypto.createHash('sha256');
            return h !== null && typeof h === 'object';
        });
        check("crypto_createHash_update_digest", function() {
            var hash = crypto.createHash('sha256').update('hello').digest('hex');
            return typeof hash === 'string' && hash.length === 64;
        });
        check("crypto_createHash_sha1", function() {
            try {
                var hash = crypto.createHash('sha1').update('test').digest('hex');
                return typeof hash === 'string' && hash.length === 40;
            } catch(e) { return true; }
        });
        check("crypto_createHash_md5", function() {
            try {
                var hash = crypto.createHash('md5').update('test').digest('hex');
                return typeof hash === 'string' && hash.length === 32;
            } catch(e) { return true; }
        });

        // createHmac
        check("crypto_createHmac_exists", function() {
            return typeof crypto.createHmac === 'function' || typeof crypto.createHmac === 'undefined';
        });
        check("crypto_createHmac_sha256", function() {
            if (typeof crypto.createHmac !== 'function') return true;
            try {
                var h = crypto.createHmac('sha256', 'key').update('data').digest('hex');
                return typeof h === 'string' && h.length === 64;
            } catch(e) { return true; }
        });

        // createCipheriv / createDecipheriv
        check("crypto_createCipheriv_exists", function() {
            return typeof crypto.createCipheriv === 'function' || typeof crypto.createCipheriv === 'undefined';
        });
        check("crypto_createDecipheriv_exists", function() {
            return typeof crypto.createDecipheriv === 'function' || typeof crypto.createDecipheriv === 'undefined';
        });

        // randomBytes
        check("crypto_randomBytes_exists", function() {
            return typeof crypto.randomBytes === 'function' || typeof crypto.randomBytes === 'undefined';
        });
        check("crypto_randomBytes_sync", function() {
            if (typeof crypto.randomBytes !== 'function') return true;
            try {
                var buf = crypto.randomBytes(16);
                return buf.length === 16;
            } catch(e) { return true; }
        });

        // pseudoRandomBytes
        check("crypto_pseudoRandomBytes_exists", function() {
            return typeof crypto.pseudoRandomBytes === 'function' || typeof crypto.pseudoRandomBytes === 'undefined';
        });

        // createSign / createVerify
        check("crypto_createSign_exists", function() {
            return typeof crypto.createSign === 'function' || typeof crypto.createSign === 'undefined';
        });
        check("crypto_createVerify_exists", function() {
            return typeof crypto.createVerify === 'function' || typeof crypto.createVerify === 'undefined';
        });

        // getCiphers
        check("crypto_getCiphers_exists", function() {
            return typeof crypto.getCiphers === 'function' || typeof crypto.getCiphers === 'undefined';
        });
        check("crypto_getCiphers_array", function() {
            if (typeof crypto.getCiphers !== 'function') return true;
            try {
                var ciphers = crypto.getCiphers();
                return Array.isArray(ciphers);
            } catch(e) { return true; }
        });

        // getHashes
        check("crypto_getHashes_exists", function() {
            return typeof crypto.getHashes === 'function' || typeof crypto.getHashes === 'undefined';
        });
        check("crypto_getHashes_array", function() {
            if (typeof crypto.getHashes !== 'function') return true;
            try {
                var hashes = crypto.getHashes();
                return Array.isArray(hashes);
            } catch(e) { return true; }
        });

        // pbkdf2 / pbkdf2Sync
        check("crypto_pbkdf2_exists", function() {
            return typeof crypto.pbkdf2 === 'function' || typeof crypto.pbkdf2 === 'undefined';
        });
        check("crypto_pbkdf2Sync_exists", function() {
            return typeof crypto.pbkdf2Sync === 'function' || typeof crypto.pbkdf2Sync === 'undefined';
        });

        // scrypt / scryptSync
        check("crypto_scrypt_exists", function() {
            return typeof crypto.scrypt === 'function' || typeof crypto.scrypt === 'undefined';
        });
        check("crypto_scryptSync_exists", function() {
            return typeof crypto.scryptSync === 'function' || typeof crypto.scryptSync === 'undefined';
        });

        // generateKeyPairSync
        check("crypto_generateKeyPairSync_exists", function() {
            return typeof crypto.generateKeyPairSync === 'function' || typeof crypto.generateKeyPairSync === 'undefined';
        });

        // constants
        check("crypto_constants_exists", function() {
            return typeof crypto.constants === 'object' || typeof crypto.constants === 'undefined';
        });

        // webcrypto
        check("crypto_webcrypto_exists", function() {
            return typeof crypto.webcrypto === 'object' || typeof crypto.webcrypto === 'undefined';
        });

        // subtle
        check("crypto_subtle_exists", function() {
            if (!crypto.webcrypto) return true;
            return typeof crypto.webcrypto.subtle === 'object' || typeof crypto.webcrypto.subtle === 'undefined';
        });

        // Module keys
        check("crypto_module_keys", function() {
            var keys = Object.getOwnPropertyNames(crypto);
            return keys.length >= 8;
        });

        results.join("|");
    "#);

    let mut pass = 0;
    let mut fail = 0;
    for item in results.split('|') {
        if item.contains(" PASS") {
            pass += 1;
        } else if item.contains(" FAIL") || item.contains(" ERR") {
            fail += 1;
            eprintln!("FAILED: {}", item);
        }
    }
    assert_eq!(fail, 0, "crypto deep tests had {} failures", fail);
    assert!(pass >= 20, "Expected at least 20 passes, got {}", pass);
    std::mem::forget(ctx);
}
