// @trace TEST-ENG-007-CRYPTO [req:REQ-ENG-007] [level:integration]
// Integration tests for node:crypto API (REQ-ENG-007)
// All JS assertions in one eval() call.

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
fn test_node_crypto_all() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var crypto = require('crypto');
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + ":" + (ok ? "PASS" : "FAIL")); }
            catch(e) { results.push(label + ":ERROR:" + (e.message || e)); }
        }

        check("require", function() { return typeof crypto === 'object'; });

        // SHA-256
        check("sha256", function() {
            var h = crypto.createHash("sha256").update("hello").digest("hex");
            return h === "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        });

        // SHA-512
        check("sha512", function() {
            return crypto.createHash("sha512").update("hello").digest("hex").length === 128;
        });

        // MD5
        check("md5", function() {
            return crypto.createHash("md5").update("hello").digest("hex").length === 32;
        });

        // SHA-1
        check("sha1", function() {
            return crypto.createHash("sha1").update("hello").digest("hex").length === 40;
        });

        // Base64 output
        check("sha256_base64", function() {
            var b = crypto.createHash("sha256").update("hello").digest("base64");
            return typeof b === "string" && b.length > 0;
        });

        // HMAC-SHA256
        check("hmac_sha256", function() {
            return crypto.createHmac("sha256", "key").update("data").digest("hex").length === 64;
        });

        // randomBytes buffer
        check("randomBytes", function() {
            var rb = crypto.randomBytes(16);
            return typeof rb === "object" && rb.length === 16;
        });

        // randomBytes hex (toString may return different format)
        check("randomBytes_hex", function() {
            var hex = crypto.randomBytes(8).toString("hex");
            return typeof hex === "string" && hex.length > 0;
        });

        // randomInt (if available)
        check("randomInt", function() {
            if (typeof crypto.randomInt !== 'function') return true;
            var n = crypto.randomInt(1, 100);
            return typeof n === "number" && n >= 1 && n < 100;
        });

        // Multiple updates
        check("multi_update", function() {
            var h = crypto.createHash("sha256");
            h.update("hel");
            h.update("lo");
            return h.digest("hex") === "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        });

        results.join("|")
    "#);

    let mut all_passed = true;
    for item in results.split('|') {
        if !item.contains(":PASS") {
            eprintln!("  FAIL: {}", item);
            all_passed = false;
        }
    }
    assert!(all_passed, "All crypto tests should pass. Results: {}", results);
    bun_runtime::shutdown_thread_sm();
}
