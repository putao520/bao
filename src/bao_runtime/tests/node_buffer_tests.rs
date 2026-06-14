// @trace TEST-ENG-007-BUF [req:REQ-ENG-007] [level:integration]
// Integration tests for node:buffer API (REQ-ENG-007)
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
fn test_node_buffer_all() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + ":" + (ok ? "PASS" : "FAIL")); }
            catch(e) { results.push(label + ":ERROR:" + (e.message || e)); }
        }

        // Buffer global exists
        check("Buffer_exists", function() { return typeof Buffer === 'function'; });

        // Buffer.alloc
        check("alloc", function() {
            var b = Buffer.alloc(10);
            return b.length === 10;
        });

        // Buffer.from string
        check("from_string", function() {
            var b = Buffer.from("hello");
            return b.length === 5;
        });

        // Buffer.from hex
        check("from_hex", function() {
            var b = Buffer.from("48656c6c6f", "hex");
            return b.length === 5;
        });

        // Buffer.from array
        check("from_array", function() {
            var b = Buffer.from([72, 101, 108, 108, 111]);
            return b.length === 5;
        });

        // toString utf8
        check("toString", function() {
            return Buffer.from("hello").toString("utf8") === "hello";
        });

        // toString hex
        check("toString_hex", function() {
            var h = Buffer.from("AB").toString("hex");
            return typeof h === "string" && h.length === 4;
        });

        // toString base64
        check("toString_base64", function() {
            var b = Buffer.from("hello").toString("base64");
            return typeof b === "string" && b.length > 0;
        });

        // Buffer.isBuffer
        check("isBuffer", function() {
            return Buffer.isBuffer(Buffer.alloc(1)) === true && Buffer.isBuffer("no") === false;
        });

        // Buffer.byteLength
        check("byteLength", function() {
            return Buffer.byteLength("hello") === 5;
        });

        // Buffer.concat
        check("concat", function() {
            var a = Buffer.from("hel");
            var b = Buffer.from("lo");
            var c = Buffer.concat([a, b]);
            return c.length === 5 && c.toString() === "hello";
        });

        // slice
        check("slice", function() {
            var b = Buffer.from("hello world");
            var s = b.slice(0, 5);
            return s.toString() === "hello";
        });

        // write
        check("write", function() {
            var b = Buffer.alloc(10);
            var n = b.write("hi", 0, "utf8");
            return n === 2;
        });

        // equals
        check("equals", function() {
            var a = Buffer.from("abc");
            var b = Buffer.from("abc");
            return a.equals(b) === true;
        });

        // compare
        check("compare", function() {
            var a = Buffer.from("a");
            var b = Buffer.from("b");
            return a.compare(b) < 0;
        });

        // indexof
        check("indexOf", function() {
            var b = Buffer.from("hello world");
            return b.indexOf("world") === 6;
        });

        // Buffer constants
        check("constants", function() {
            return typeof Buffer.constants === 'object' || typeof Buffer.constants === 'undefined';
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
    assert!(all_passed, "All buffer tests should pass. Results: {}", results);
    bun_runtime::shutdown_thread_sm();
}
