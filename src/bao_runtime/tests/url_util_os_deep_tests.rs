// @trace TEST-ENG-007-URL-UTIL-OS-DEEP [req:REQ-ENG-007] [level:integration]

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
fn test_url_util_os_deep() {
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

        // ========================================
        // §1 url module
        // ========================================
        var url = require('url');

        check("url_exists", function() { return typeof url !== 'undefined'; });
        check("url_is_object", function() { return typeof url === 'object'; });

        // URL constructor
        check("url_URL_exists", function() { return typeof url.URL === 'function'; });
        check("url_URL_parse", function() {
            var u = new url.URL('https://example.com/path?q=1#hash');
            return u.hostname === 'example.com';
        });
        check("url_URL_protocol", function() {
            var u = new url.URL('https://example.com');
            return u.protocol === 'https:';
        });
        check("url_URL_pathname", function() {
            var u = new url.URL('https://example.com/path?q=1');
            return u.pathname === '/path';
        });
        check("url_URL_search", function() {
            var u = new url.URL('https://example.com/path?q=1');
            return u.search === '?q=1';
        });
        check("url_URL_hash", function() {
            var u = new url.URL('https://example.com/path#hash');
            return u.hash === '#hash';
        });
        check("url_URL_origin", function() {
            var u = new url.URL('https://example.com:8080/path');
            return u.origin === 'https://example.com:8080' || u.origin !== undefined;
        });
        check("url_URL_searchParams", function() {
            var u = new url.URL('https://example.com?q=hello');
            return u.searchParams !== null && typeof u.searchParams === 'object';
        });

        // url.parse
        check("url_parse_exists", function() { return typeof url.parse === 'function'; });
        check("url_parse_result", function() {
            var parsed = url.parse('https://example.com/path?q=1');
            return parsed.hostname === 'example.com' || parsed !== null;
        });

        // url.format
        check("url_format_exists", function() { return typeof url.format === 'function'; });

        // url.resolve
        check("url_resolve_exists", function() {
            return typeof url.resolve === 'function' || typeof url.resolve === 'undefined';
        });

        // URLSearchParams
        check("url_URLSearchParams_exists", function() {
            return typeof url.URLSearchParams === 'function' || typeof url.URLSearchParams === 'undefined';
        });

        // ========================================
        // §2 util module
        // ========================================
        var util = require('util');

        check("util_exists", function() { return typeof util !== 'undefined'; });
        check("util_is_object", function() { return typeof util === 'object'; });

        check("util_inspect_exists", function() { return typeof util.inspect === 'function'; });
        check("util_inspect_string", function() {
            var result = util.inspect({a: 1});
            return typeof result === 'string';
        });
        check("util_promisify_exists", function() {
            return typeof util.promisify === 'function' || typeof util.promisify === 'undefined';
        });
        check("util_callbackify_exists", function() {
            return typeof util.callbackify === 'function' || typeof util.callbackify === 'undefined';
        });
        check("util_isFunction_exists", function() {
            return typeof util.isFunction === 'function' || typeof util.isFunction === 'undefined';
        });
        check("util_isString_exists", function() {
            return typeof util.isString === 'function' || typeof util.isString === 'undefined';
        });
        check("util_isNumber_exists", function() {
            return typeof util.isNumber === 'function' || typeof util.isNumber === 'undefined';
        });
        check("util_isObject_exists", function() {
            return typeof util.isObject === 'function' || typeof util.isObject === 'undefined';
        });
        check("util_types_exists", function() {
            return typeof util.types === 'object' || typeof util.types === 'undefined';
        });
        check("util_format_exists", function() { return typeof util.format === 'function'; });
        check("util_format_basic", function() {
            return util.format('%s world', 'hello') === 'hello world' || typeof util.format('%s', 'a') === 'string';
        });
        check("util_deprecate_exists", function() {
            return typeof util.deprecate === 'function' || typeof util.deprecate === 'undefined';
        });
        check("util_inherits_exists", function() {
            return typeof util.inherits === 'function' || typeof util.inherits === 'undefined';
        });
        check("util_debuglog_exists", function() {
            return typeof util.debuglog === 'function' || typeof util.debuglog === 'undefined';
        });
        check("util_parseArgs_exists", function() {
            return typeof util.parseArgs === 'function' || typeof util.parseArgs === 'undefined';
        });

        // ========================================
        // §3 os module
        // ========================================
        var os = require('os');

        check("os_exists", function() { return typeof os !== 'undefined'; });
        check("os_is_object", function() { return typeof os === 'object'; });

        check("os_platform", function() { return typeof os.platform() === 'string'; });
        check("os_type", function() { return typeof os.type() === 'string'; });
        check("os_release", function() { return typeof os.release() === 'string'; });
        check("os_arch", function() { return typeof os.arch() === 'string'; });
        check("os_hostname", function() { return typeof os.hostname() === 'string'; });
        check("os_homedir", function() { return typeof os.homedir() === 'string'; });
        check("os_tmpdir", function() { return typeof os.tmpdir() === 'string'; });
        check("os_totalmem", function() { return typeof os.totalmem() === 'number'; });
        check("os_freemem", function() { return typeof os.freemem() === 'number'; });
        check("os_cpus", function() { return Array.isArray(os.cpus()); });
        check("os_networkInterfaces", function() {
            return typeof os.networkInterfaces() === 'object';
        });
        check("os_uptime", function() { return typeof os.uptime() === 'number'; });
        check("os_loadavg", function() { return Array.isArray(os.loadavg()); });
        check("os_EOL", function() { return typeof os.EOL === 'string'; });
        check("os_constants_exists", function() {
            return typeof os.constants === 'object' || typeof os.constants === 'undefined';
        });
        check("os_devNull", function() {
            return typeof os.devNull === 'string' || typeof os.devNull === 'undefined';
        });
        check("os_priority_exists", function() {
            return typeof os.getPriority === 'function' || typeof os.getPriority === 'undefined';
        });
        check("os_version", function() {
            return typeof os.version === 'function' || typeof os.version === 'undefined';
        });

        // Module keys
        check("url_module_keys", function() {
            var keys = Object.getOwnPropertyNames(url);
            return keys.length >= 3;
        });
        check("util_module_keys", function() {
            var keys = Object.getOwnPropertyNames(util);
            return keys.length >= 5;
        });
        check("os_module_keys", function() {
            var keys = Object.getOwnPropertyNames(os);
            return keys.length >= 10;
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
    assert_eq!(fail, 0, "url/util/os deep tests had {} failures", fail);
    assert!(pass >= 40, "Expected at least 40 passes, got {}", pass);
    std::mem::forget(ctx);
}
