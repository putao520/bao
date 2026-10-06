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

    let results = eval_string(
        &mut ctx,
        r#"
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
    "#,
    );

    let mut all_passed = true;
    for item in results.split('|') {
        if !item.contains(":PASS") {
            eprintln!("  FAIL: {}", item);
            all_passed = false;
        }
    }
    assert!(
        all_passed,
        "All buffer tests should pass. Results: {}",
        results
    );
    bun_runtime::shutdown_thread_sm();
}

// @trace TEST-ENG-007-BUF [req:REQ-ENG-007] [level:integration]
// Test lock for upstream oven-sh/bun aa8307619d: the per-encoding Buffer
// *Write methods reject a non-string value (ERR_INVALID_ARG_TYPE, "argument
// must be a string") and never coerce it. Order follows Node:
// utf8/latin1/ascii check bounds first (ERR_BUFFER_OUT_OF_BOUNDS wins,
// offset/length valueOf runs before the rejection); the others reject the
// value before reading offset/length.
#[test]
fn test_buffer_write_rejects_non_string_value() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let results = eval_string(
        &mut ctx,
        r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + ":" + (ok ? "PASS" : "FAIL")); }
            catch(e) { results.push(label + ":ERROR:" + (e.message || e)); }
        }
        // Node's THROW_AND_RETURN_IF_NOT_STRING: fixed message + code.
        function isInvalidArgType(e) {
            return e instanceof TypeError &&
                e.message === 'argument must be a string' &&
                e.code === 'ERR_INVALID_ARG_TYPE';
        }
        function throwsInvalidArgType(fn) {
            try { fn(); return false; }
            catch (e) { return isInvalidArgType(e); }
        }
        var METHODS = ['utf8Write', 'latin1Write', 'asciiWrite', 'hexWrite',
                       'base64Write', 'base64urlWrite', 'ucs2Write',
                       'utf16leWrite', 'utf16beWrite'];
        var nonStrings = [123, null, undefined, true, 1n, Symbol('s'), {},
                          [], new String('ab'), Buffer.from('ab'), function() {}];

        check("reject_non_string_all_methods", function() {
            var buf = Buffer.alloc(8);
            for (var m = 0; m < METHODS.length; m++) {
                var method = METHODS[m];
                for (var v = 0; v < nonStrings.length; v++) {
                    if (!throwsInvalidArgType(function() { buf[method](nonStrings[v]); })) return false;
                    if (!throwsInvalidArgType(function() { buf[method](nonStrings[v], 0, 1); })) return false;
                }
                if (!throwsInvalidArgType(function() { buf[method](); })) return false;
            }
            return true;
        });

        check("buffer_untouched_after_rejections", function() {
            var buf = Buffer.alloc(8);
            try { buf.utf8Write(123); } catch (e) {}
            try { buf.hexWrite({}); } catch (e) {}
            return buf.toString('hex') === '0'.repeat(16);
        });

        // No user code runs on the value: toString/valueOf/Symbol.toPrimitive
        // must not be called.
        check("no_coercion_of_object_value", function() {
            var calls = [];
            var value = {
                toString: function() { calls.push('toString'); return 'ab'; },
                valueOf: function() { calls.push('valueOf'); return 'ab'; }
            };
            value[Symbol.toPrimitive] = function() { calls.push('toPrimitive'); return 'ab'; };
            var buf = Buffer.alloc(8);
            if (!throwsInvalidArgType(function() { buf.utf8Write(value); })) return false;
            if (!throwsInvalidArgType(function() { buf.hexWrite(value); })) return false;
            return calls.length === 0;
        });

        // utf8/latin1/ascii: bounds checked first — B wins over A, and the
        // offset/length coercion runs before the rejection.
        check("utf8_bounds_error_wins", function() {
            var buf = Buffer.alloc(8);
            try { buf.utf8Write(123, -1); return false; }
            catch (e) { if (!(e.code === 'ERR_BUFFER_OUT_OF_BOUNDS')) return false; }
            try { buf.utf8Write(123, 0, 9); return false; }
            catch (e) { if (!(e.code === 'ERR_BUFFER_OUT_OF_BOUNDS')) return false; }
            return true;
        });
        check("utf8_valueof_runs_before_rejection", function() {
            var calls = [];
            var offObj = { valueOf: function() { calls.push('off'); return 0; } };
            var lenObj = { valueOf: function() { calls.push('len'); return 4; } };
            var buf = Buffer.alloc(8);
            if (!throwsInvalidArgType(function() { buf.utf8Write(123, offObj, lenObj); })) return false;
            return calls.length === 2 && calls[0] === 'off' && calls[1] === 'len';
        });

        // hex/base64/base64url/ucs2/utf16le: value rejected BEFORE offset/
        // length are read — no valueOf calls.
        check("hex_rejects_before_reading_offset", function() {
            var calls = [];
            var offObj = { valueOf: function() { calls.push('off'); return 0; } };
            var lenObj = { valueOf: function() { calls.push('len'); return 4; } };
            var buf = Buffer.alloc(8);
            if (!throwsInvalidArgType(function() { buf.hexWrite(123, offObj, lenObj); })) return false;
            if (!throwsInvalidArgType(function() { buf.hexWrite(123, 9); })) return false;
            return calls.length === 0;
        });

        check("empty_buffer_still_rejects", function() {
            return throwsInvalidArgType(function() { Buffer.alloc(0).utf8Write(123); });
        });

        check("string_path_unchanged", function() {
            var buf = Buffer.alloc(8);
            return buf.utf8Write('ab') === 2 && buf.hexWrite('abcd', 2) === 2 &&
                   buf.toString('hex') === '6162abcd' + '0'.repeat(8);
        });

        results.join("|")
    "#,
    );

    let mut all_passed = true;
    for item in results.split('|') {
        if !item.contains(":PASS") {
            eprintln!("  FAIL: {}", item);
            all_passed = false;
        }
    }
    assert!(
        all_passed,
        "buffer non-string write rejection tests should pass. Results: {}",
        results
    );
    bun_runtime::shutdown_thread_sm();
}
