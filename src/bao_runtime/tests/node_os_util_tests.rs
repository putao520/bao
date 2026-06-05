// @trace TEST-ENG-007-OS [req:REQ-ENG-007] [level:integration]
// Integration tests for node:os and node:util API (REQ-ENG-007)
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
fn test_node_os_util_all() {
    bao_runtime::install_exit_handler();
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var os = require('os');
        var util = require('util');
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e)); }
        }

        // === node:os ===
        check("os_require", function() { return typeof os === 'object'; });
        check("os_platform", function() { return typeof os.platform() === "string" && os.platform().length > 0; });
        check("os_arch", function() { return typeof os.arch() === "string" && os.arch().length > 0; });
        check("os_type", function() { return typeof os.type() === "string" && os.type().length > 0; });
        check("os_release", function() { return typeof os.release() === "string"; });
        check("os_hostname", function() { return typeof os.hostname() === "string" && os.hostname().length > 0; });
        check("os_uptime", function() { return typeof os.uptime() === "number" && os.uptime() > 0; });
        check("os_totalmem", function() { return typeof os.totalmem() === "number" && os.totalmem() > 0; });
        check("os_freemem", function() { return typeof os.freemem() === "number" && os.freemem() >= 0; });
        check("os_cpus", function() { return Array.isArray(os.cpus()) && os.cpus().length > 0; });
        check("os_cpus_model", function() { return typeof os.cpus()[0].model === "string"; });
        check("os_homedir", function() { return typeof os.homedir() === "string" && os.homedir().length > 0; });
        check("os_tmpdir", function() { return typeof os.tmpdir() === "string" && os.tmpdir().length > 0; });
        check("os_endianness", function() { var e = os.endianness(); return e === "LE" || e === "BE"; });
        check("os_loadavg", function() { return Array.isArray(os.loadavg()) && os.loadavg().length === 3; });
        check("os_userInfo", function() {
            var ui = os.userInfo();
            return typeof ui.username === "string" && typeof ui.homedir === "string";
        });
        check("os_networkInterfaces", function() {
            var ni = os.networkInterfaces();
            return typeof ni === "object" && ni !== null;
        });
        check("os_EOL", function() { return typeof os.EOL === "string"; });
        check("os_constants", function() { return typeof os.constants === "object" || typeof os.constants === "undefined"; });

        // === node:util ===
        check("util_require", function() { return typeof util === 'object'; });
        check("util_inspect", function() { return typeof util.inspect("hello") === "string"; });
        check("util_inspect_obj", function() { return typeof util.inspect({a: 1}) === "string"; });
        check("util_isBoolean", function() { return util.isBoolean(true) === true && util.isBoolean(1) === false; });
        check("util_isNumber", function() { return util.isNumber(42) === true && util.isNumber("42") === false; });
        check("util_isString", function() { return util.isString("hi") === true && util.isString(42) === false; });
        check("util_isUndefined", function() { return util.isUndefined(undefined) === true && util.isUndefined(null) === false; });
        check("util_isNull", function() { return util.isNull(null) === true && util.isNull(undefined) === false; });
        check("util_isObject", function() { return util.isObject({}) === true && util.isObject(null) === false; });
        check("util_isFunction", function() { return util.isFunction(function(){}) === true && util.isFunction(1) === false; });
        check("util_isArray", function() { return util.isArray([1]) === true && util.isArray("1") === false; });
        check("util_isDate", function() { return util.isDate(new Date()) === true && util.isDate({}) === false; });
        check("util_isRegExp", function() { return util.isRegExp(/a/) === true && util.isRegExp("a") === false; });
        check("util_isError", function() { return util.isError(new Error("x")) === true && util.isError({}) === false; });
        check("util_format", function() { return util.format("hello %s", "world") === "hello world"; });
        check("util_format_no_args", function() { return util.format("hello") === "hello"; });
        check("util_format_numbers", function() { return typeof util.format("%d", 42) === "string"; });
        check("util_promisify", function() { return typeof util.promisify === "function"; });

        results.join("|")
    "#);

    let mut all_passed = true;
    for item in results.split('|') {
        if !item.contains(" PASS") {
            eprintln!("  FAIL: {}", item);
            all_passed = false;
        }
    }
    assert!(all_passed, "All os/util tests should pass");
    std::mem::forget(ctx);
}
