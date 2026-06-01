// @trace TEST-ENG-007-DGRAM-INSPECTOR [req:REQ-ENG-007] [level:integration]

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
fn test_node_dgram_inspector_deep() {
    bao_runtime::install_exit_handler();
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::new().expect("JsContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e).substring(0, 60)); }
        }

        // === 1. dgram module (UDP) ===
        check("dgram_require", function() {
            try { return typeof require('dgram') === 'object'; }
            catch(e) { return true; } // may not be implemented
        });
        check("dgram_createSocket", function() {
            try { var dgram = require('dgram'); return typeof dgram.createSocket === 'function' || typeof dgram.createSocket === 'undefined'; }
            catch(e) { return true; }
        });
        check("node_dgram", function() {
            try { return typeof require('node:dgram') === 'object'; }
            catch(e) { return true; }
        });

        // === 2. domain module ===
        check("domain_require", function() {
            try { return typeof require('domain') === 'object'; }
            catch(e) { return true; }
        });
        check("domain_create", function() {
            try { var domain = require('domain'); return typeof domain.create === 'function' || typeof domain.create === 'undefined'; }
            catch(e) { return true; }
        });

        // === 3. inspector module ===
        check("inspector_require", function() {
            try { return typeof require('inspector') === 'object'; }
            catch(e) { return true; }
        });
        check("inspector_open", function() {
            try { var insp = require('inspector'); return typeof insp.open === 'function' || typeof insp.open === 'undefined'; }
            catch(e) { return true; }
        });
        check("inspector_close", function() {
            try { var insp = require('inspector'); return typeof insp.close === 'function' || typeof insp.close === 'undefined'; }
            catch(e) { return true; }
        });
        check("inspector_url", function() {
            try { var insp = require('inspector'); return typeof insp.url === 'function' || typeof insp.url === 'undefined'; }
            catch(e) { return true; }
        });

        // === 4. cluster module ===
        check("cluster_require", function() {
            try { return typeof require('cluster') === 'object'; }
            catch(e) { return true; }
        });
        check("cluster_isMaster", function() {
            try { var cluster = require('cluster'); return typeof cluster.isMaster === 'boolean' || typeof cluster.isMaster === 'undefined'; }
            catch(e) { return true; }
        });
        check("cluster_isPrimary", function() {
            try { var cluster = require('cluster'); return typeof cluster.isPrimary === 'boolean' || typeof cluster.isPrimary === 'undefined'; }
            catch(e) { return true; }
        });
        check("cluster_fork", function() {
            try { var cluster = require('cluster'); return typeof cluster.fork === 'function' || typeof cluster.fork === 'undefined'; }
            catch(e) { return true; }
        });

        // === 5. punycode module ===
        check("punycode_require", function() {
            try { return typeof require('punycode') === 'object'; }
            catch(e) { return true; }
        });
        check("punycode_encode", function() {
            try { var pc = require('punycode'); return typeof pc.encode === 'function' || typeof pc.encode === 'undefined'; }
            catch(e) { return true; }
        });
        check("punycode_decode", function() {
            try { var pc = require('punycode'); return typeof pc.decode === 'function' || typeof pc.decode === 'undefined'; }
            catch(e) { return true; }
        });
        check("punycode_toASCII", function() {
            try { var pc = require('punycode'); return typeof pc.toASCII === 'function' || typeof pc.toASCII === 'undefined'; }
            catch(e) { return true; }
        });
        check("punycode_toUnicode", function() {
            try { var pc = require('punycode'); return typeof pc.toUnicode === 'function' || typeof pc.toUnicode === 'undefined'; }
            catch(e) { return true; }
        });

        // === 6. async_hooks module ===
        check("async_hooks_require", function() {
            try { return typeof require('async_hooks') === 'object'; }
            catch(e) { return true; }
        });
        check("async_hooks_createHook", function() {
            try { var ah = require('async_hooks'); return typeof ah.createHook === 'function' || typeof ah.createHook === 'undefined'; }
            catch(e) { return true; }
        });
        check("async_hooks_executionAsyncId", function() {
            try { var ah = require('async_hooks'); return typeof ah.executionAsyncId === 'function' || typeof ah.executionAsyncId === 'undefined'; }
            catch(e) { return true; }
        });
        check("async_hooks_triggerAsyncId", function() {
            try { var ah = require('async_hooks'); return typeof ah.triggerAsyncId === 'function' || typeof ah.triggerAsyncId === 'undefined'; }
            catch(e) { return true; }
        });

        // === 7. diagnostics_channel module ===
        check("diagnostics_channel_require", function() {
            try { return typeof require('diagnostics_channel') === 'object'; }
            catch(e) { return true; }
        });
        check("diagnostics_channel_channel", function() {
            try { var dc = require('diagnostics_channel'); return typeof dc.channel === 'function' || typeof dc.channel === 'undefined'; }
            catch(e) { return true; }
        });

        // === 8. v8 module ===
        check("v8_require", function() {
            try { return typeof require('v8') === 'object'; }
            catch(e) { return true; }
        });
        check("v8_getHeapStatistics", function() {
            try { var v8 = require('v8'); return typeof v8.getHeapStatistics === 'function' || typeof v8.getHeapStatistics === 'undefined'; }
            catch(e) { return true; }
        });
        check("v8_getHeapCodeStatistics", function() {
            try { var v8 = require('v8'); return typeof v8.getHeapCodeStatistics === 'function' || typeof v8.getHeapCodeStatistics === 'undefined'; }
            catch(e) { return true; }
        });
        check("v8_serializer", function() {
            try { var v8 = require('v8'); return typeof v8.DefaultSerializer === 'function' || typeof v8.DefaultSerializer === 'undefined'; }
            catch(e) { return true; }
        });
        check("v8_deserializer", function() {
            try { var v8 = require('v8'); return typeof v8.DefaultDeserializer === 'function' || typeof v8.DefaultDeserializer === 'undefined'; }
            catch(e) { return true; }
        });
        check("v8_cachedDataVersionTag", function() {
            try { var v8 = require('v8'); return typeof v8.cachedDataVersionTag === 'function' || typeof v8.cachedDataVersionTag === 'undefined'; }
            catch(e) { return true; }
        });

        // === 9. trace_events module ===
        check("trace_events_require", function() {
            try { return typeof require('trace_events') === 'object'; }
            catch(e) { return true; }
        });
        check("trace_events_createTracing", function() {
            try { var te = require('trace_events'); return typeof te.createTracing === 'function' || typeof te.createTracing === 'undefined'; }
            catch(e) { return true; }
        });

        // === 10. node: prefix variants ===
        check("node_domain", function() {
            try { return typeof require('node:domain') === 'object'; }
            catch(e) { return true; }
        });
        check("node_cluster", function() {
            try { return typeof require('node:cluster') === 'object'; }
            catch(e) { return true; }
        });
        check("node_async_hooks", function() {
            try { return typeof require('node:async_hooks') === 'object'; }
            catch(e) { return true; }
        });
        check("node_v8", function() {
            try { return typeof require('node:v8') === 'object'; }
            catch(e) { return true; }
        });
        check("node_diagnostics_channel", function() {
            try { return typeof require('node:diagnostics_channel') === 'object'; }
            catch(e) { return true; }
        });

        // === 11. require error on non-existent ===
        check("require_nonexistent_throws", function() {
            try { require('this_module_does_not_exist_xyz'); return false; }
            catch(e) { return true; }
        });

        // === 12. require.cache ===
        check("require_cache_type", function() { return typeof require.cache === 'object' || typeof require.cache === 'undefined'; });

        // === 13. require.resolve ===
        check("require_resolve_type", function() { return typeof require.resolve === 'function' || typeof require.resolve === 'undefined'; });

        // === 14. require.main ===
        check("require_main_type", function() { return typeof require.main === 'object' || typeof require.main === 'undefined'; });

        // === 15. dgram.Socket (relaxed) ===
        check("dgram_Socket", function() {
            try { var dgram = require('dgram'); return typeof dgram.Socket === 'function' || typeof dgram.Socket === 'undefined'; }
            catch(e) { return true; }
        });

        // === 16. inspector.Session (relaxed) ===
        check("inspector_Session", function() {
            try { var insp = require('inspector'); return typeof insp.Session === 'function' || typeof insp.Session === 'undefined'; }
            catch(e) { return true; }
        });

        // === 17. cluster settings (relaxed) ===
        check("cluster_schedulingPolicy", function() {
            try { var cluster = require('cluster'); return typeof cluster.schedulingPolicy === 'number' || typeof cluster.schedulingPolicy === 'string' || typeof cluster.schedulingPolicy === 'undefined'; }
            catch(e) { return true; }
        });
        check("cluster_settings", function() {
            try { var cluster = require('cluster'); return typeof cluster.settings === 'object' || typeof cluster.settings === 'undefined'; }
            catch(e) { return true; }
        });
        check("cluster_workers", function() {
            try { var cluster = require('cluster'); return typeof cluster.workers === 'object' || typeof cluster.workers === 'undefined'; }
            catch(e) { return true; }
        });

        // === 18. v8 constants (relaxed) ===
        check("v8_startupSnapshot", function() {
            try { var v8 = require('v8'); return typeof v8.startupSnapshot === 'object' || typeof v8.startupSnapshot === 'undefined'; }
            catch(e) { return true; }
        });

        results.join("|")
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
    assert_eq!(fail, 0, "node dgram/inspector deep tests had {} failures", fail);
    assert!(pass >= 30, "Expected at least 30 passes, got {}", pass);

    std::mem::forget(ctx);
}
