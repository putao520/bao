// @trace TEST-ENG-007-REQ [req:REQ-ENG-007] [level:integration]

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
fn test_require_timers_all() {
    bao_runtime::install_exit_handler();
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e)); }
        }

        // === require() exists ===
        check("require_exists", function() { return typeof require === 'function'; });

        // === require('path') ===
        var path = require('path');
        check("path_require", function() { return typeof path === 'object'; });
        check("path_join", function() { return typeof path.join === 'function'; });
        check("path_resolve", function() { return typeof path.resolve === 'function'; });
        check("path_basename", function() { return typeof path.basename === 'function'; });

        // === require('fs') ===
        var fs = require('fs');
        check("fs_require", function() { return typeof fs === 'object'; });
        check("fs_readFileSync", function() { return typeof fs.readFileSync === 'function'; });
        check("fs_writeFileSync", function() { return typeof fs.writeFileSync === 'function'; });

        // === require('crypto') ===
        var crypto = require('crypto');
        check("crypto_require", function() { return typeof crypto === 'object'; });
        check("crypto_createHash", function() { return typeof crypto.createHash === 'function'; });

        // === require('events') ===
        var events = require('events');
        check("events_require", function() { return typeof events === 'object'; });
        check("events_EventEmitter", function() { return typeof events.EventEmitter === 'function'; });

        // === require('url') ===
        var url = require('url');
        check("url_require", function() { return typeof url === 'object'; });
        check("url_URL", function() { return typeof url.URL === 'function'; });

        // === require('util') ===
        var util = require('util');
        check("util_require", function() { return typeof util === 'object'; });
        check("util_inspect", function() { return typeof util.inspect === 'function'; });

        // === require('buffer') ===
        var buffer = require('buffer');
        check("buffer_require", function() { return typeof buffer === 'object'; });
        check("buffer_Buffer", function() { return typeof buffer.Buffer === 'function'; });

        // === require('os') ===
        var os = require('os');
        check("os_require", function() { return typeof os === 'object'; });
        check("os_hostname", function() { return typeof os.hostname === 'function'; });
        check("os_platform", function() { return typeof os.platform === 'function'; });
        check("os_type", function() { return typeof os.type === 'function'; });

        // === require('stream') ===
        var stream = require('stream');
        check("stream_require", function() { return typeof stream === 'object'; });
        check("stream_Readable", function() { return typeof stream.Readable === 'function'; });
        check("stream_Writable", function() { return typeof stream.Writable === 'function'; });

        // === require('querystring') ===
        var qs = require('querystring');
        check("qs_require", function() { return typeof qs === 'object'; });
        check("qs_parse", function() { return typeof qs.parse === 'function'; });
        check("qs_stringify", function() { return typeof qs.stringify === 'function'; });

        // === require('timers') ===
        var timers = require('timers');
        check("timers_require", function() { return typeof timers === 'object'; });
        check("timers_setTimeout", function() { return typeof timers.setTimeout === 'function'; });
        check("timers_setInterval", function() { return typeof timers.setInterval === 'function'; });
        check("timers_setImmediate", function() { return typeof timers.setImmediate === 'function'; });

        // === require('child_process') ===
        var cp = require('child_process');
        check("cp_require", function() { return typeof cp === 'object'; });
        check("cp_exec", function() { return typeof cp.exec === 'function'; });
        check("cp_spawn", function() { return typeof cp.spawn === 'function'; });

        // === require('dns') ===
        var dns = require('dns');
        check("dns_require", function() { return typeof dns === 'object'; });
        check("dns_lookup", function() { return typeof dns.lookup === 'function'; });
        check("dns_resolve", function() { return typeof dns.resolve === 'function'; });

        // === require('net') ===
        var net = require('net');
        check("net_require", function() { return typeof net === 'object'; });
        check("net_createServer", function() { return typeof net.createServer === 'function'; });
        check("net_isIP", function() { return typeof net.isIP === 'function'; });
        check("net_isIPv4", function() { return typeof net.isIPv4 === 'function'; });
        check("net_isIPv6", function() { return typeof net.isIPv6 === 'function'; });

        // === require('assert') ===
        var assert = require('assert');
        check("assert_require", function() { return typeof assert === 'object'; });
        check("assert_ok", function() { return typeof assert.ok === 'function'; });
        check("assert_equal", function() { return typeof assert.equal === 'function'; });
        check("assert_deepEqual", function() { return typeof assert.deepEqual === 'function'; });

        // === require('assert/strict') ===
        var strict = require('assert/strict');
        check("assert_strict_require", function() { return typeof strict === 'object'; });

        // === require('module') ===
        var mod = require('module');
        check("module_require", function() { return typeof mod === 'object'; });
        check("module_createRequire", function() { return typeof mod.createRequire === 'function'; });

        // === Timer globals ===
        check("global_setTimeout", function() { return typeof setTimeout === 'function'; });
        check("global_setInterval", function() { return typeof setInterval === 'function'; });
        check("global_setImmediate", function() { return typeof setImmediate === 'function'; });
        check("global_clearTimeout", function() { return typeof clearTimeout === 'function'; });
        check("global_clearInterval", function() { return typeof clearInterval === 'function'; });
        check("global_clearImmediate", function() { return typeof clearImmediate === 'function'; });

        results.join("|")
    "#);

    let mut all_passed = true;
    for item in results.split('|') {
        if !item.contains(" PASS") {
            eprintln!("  FAIL: {}", item);
            all_passed = false;
        }
    }
    assert!(all_passed, "All require/timers tests should pass. Results: {}", results);
    bao_runtime::shutdown_thread_sm();
}
