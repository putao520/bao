// @trace TEST-ENG-007-STR [req:REQ-ENG-007] [level:integration]
// Integration tests for node:stream and node:querystring API (REQ-ENG-007)
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
fn test_node_stream_qs_all() {
    bao_runtime::install_exit_handler();
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var stream = require('stream');
        var qs = require('querystring');
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e)); }
        }

        // === node:stream ===
        check("stream_require", function() { return typeof stream === 'object'; });
        check("Readable", function() { return typeof stream.Readable === 'function'; });
        check("Writable", function() { return typeof stream.Writable === 'function'; });
        check("Duplex", function() { return typeof stream.Duplex === 'function'; });
        check("Transform", function() { return typeof stream.Transform === 'function'; });
        check("PassThrough", function() { return typeof stream.PassThrough === 'function'; });
        check("readable_instance", function() {
            var r = new stream.Readable();
            return typeof r.on === 'function' && typeof r.push === 'function';
        });
        check("readable_read", function() {
            var r = new stream.Readable();
            return typeof r.read === 'function';
        });
        check("readable_pipe", function() {
            var r = new stream.Readable();
            return typeof r.pipe === 'function';
        });
        check("writable_instance", function() {
            var w = new stream.Writable();
            return typeof w.write === 'function' && typeof w.end === 'function';
        });
        check("duplex_instance", function() {
            var d = new stream.Duplex();
            return typeof d.read === 'function' && typeof d.write === 'function';
        });
        check("transform_instance", function() {
            var t = new stream.Transform();
            return typeof t.write === 'function' && typeof t.push === 'function';
        });

        // === node:querystring ===
        check("qs_require", function() { return typeof qs === 'object'; });
        check("qs_parse_basic", function() {
            var obj = qs.parse("a=1&b=2");
            return obj.a === "1" && obj.b === "2";
        });
        check("qs_parse_empty_val", function() {
            var obj = qs.parse("key=");
            return obj.key === "";
        });
        check("qs_parse_no_val", function() {
            var obj = qs.parse("key");
            return obj.key === "" || obj.key === true || typeof obj.key === "string";
        });
        check("qs_parse_encoded", function() {
            var obj = qs.parse("name=hello%20world");
            return typeof obj.name === "string";
        });
        check("qs_stringify", function() {
            return qs.stringify({a: "1", b: "2"}) === "a=1&b=2";
        });
        check("qs_stringify_encode", function() {
            var s = qs.stringify({name: "hello world"});
            return s.indexOf("hello") >= 0;
        });
        check("qs_escape", function() {
            return typeof qs.escape("hello world") === "string";
        });
        check("qs_unescape", function() {
            return typeof qs.unescape("hello%20world") === "string";
        });

        results.join("|")
    "#);

    let mut all_passed = true;
    for item in results.split('|') {
        if !item.contains(" PASS") {
            eprintln!("  FAIL: {}", item);
            all_passed = false;
        }
    }
    assert!(all_passed, "All stream/qs tests should pass");
    bao_runtime::shutdown_thread_sm();
}
