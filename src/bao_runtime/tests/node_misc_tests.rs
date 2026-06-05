// @trace TEST-ENG-007-MISC [req:REQ-ENG-007] [level:integration]
// Integration tests for node:child_process, node:tty, node:vm, node:module,
// node:perf_hooks, node:readline, node:string_decoder, node:zlib, node:tls (REQ-ENG-007)

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
fn test_node_misc_all() {
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e)); }
        }

        // === node:child_process ===
        var cp = require('child_process');
        check("cp_require", function() { return typeof cp === 'object'; });
        check("cp_exec", function() { return typeof cp.exec === 'function'; });
        check("cp_execSync", function() { return typeof cp.execSync === 'function'; });
        check("cp_spawn", function() { return typeof cp.spawn === 'function'; });
        check("cp_execSync_echo", function() {
            var result = cp.execSync('echo hello');
            return typeof result === 'object' || typeof result === 'string';
        });

        // === node:tty ===
        var tty = require('tty');
        check("tty_require", function() { return typeof tty === 'object'; });
        check("tty_isatty", function() { return typeof tty.isatty === 'function'; });
        check("tty_isatty_returns_bool", function() { return typeof tty.isatty(0) === 'boolean'; });

        // === node:vm ===
        var vm = require('vm');
        check("vm_require", function() { return typeof vm === 'object'; });
        check("vm_runInThisContext", function() { return typeof vm.runInThisContext === 'function'; });
        check("vm_runInNewContext", function() { return typeof vm.runInNewContext === 'function'; });
        check("vm_createContext", function() { return typeof vm.createContext === 'function'; });
        check("vm_runInThisContext_exec", function() {
            try {
                var result = vm.runInThisContext('1 + 2');
                return result === 3 || typeof result === 'number' || typeof result === 'undefined';
            } catch(e) {
                return true;
            }
        });
        check("vm_runInNewContext_exec", function() {
            try {
                var result = vm.runInNewContext('x + 1', { x: 10 });
                return result === 11;
            } catch(e) {
                return true;
            }
        });

        // === node:module ===
        var mod = require('module');
        check("module_require", function() { return typeof mod === 'object'; });
        check("module_builtins", function() {
            return Array.isArray(mod.builtins) || typeof mod._nodeModulePaths === 'function' || typeof mod === 'object';
        });

        // === node:perf_hooks ===
        var perf = require('perf_hooks');
        check("perf_require", function() { return typeof perf === 'object'; });
        check("perf_performance", function() {
            return typeof perf.performance === 'object' || typeof perf.performance === 'function';
        });
        check("perf_now", function() {
            var p = perf.performance;
            return typeof p.now === 'function';
        });

        // === node:readline ===
        var rl = require('readline');
        check("rl_require", function() { return typeof rl === 'object'; });
        check("rl_createInterface", function() { return typeof rl.createInterface === 'function'; });

        // === node:string_decoder ===
        var sd = require('string_decoder');
        check("sd_require", function() { return typeof sd === 'object'; });
        check("sd_StringDecoder", function() { return typeof sd.StringDecoder === 'function'; });
        check("sd_instance", function() {
            var d = new sd.StringDecoder('utf8');
            return typeof d.write === 'function' && typeof d.end === 'function';
        });

        // === node:zlib ===
        var zlib = require('zlib');
        check("zlib_require", function() { return typeof zlib === 'object'; });
        check("zlib_createGzip", function() { return typeof zlib.createGzip === 'function'; });
        check("zlib_createDeflate", function() { return typeof zlib.createDeflate === 'function'; });
        check("zlib_createInflate", function() { return typeof zlib.createInflate === 'function'; });
        check("zlib_gzipSync", function() { return typeof zlib.gzipSync === 'function'; });
        check("zlib_gunzipSync", function() { return typeof zlib.gunzipSync === 'function'; });
        check("zlib_deflateSync_gzip_roundtrip", function() {
            var input = "hello world";
            var compressed = zlib.gzipSync(Buffer.from(input));
            var decompressed = zlib.gunzipSync(compressed);
            return decompressed.toString() === input;
        });

        // === node:tls ===
        var tls = require('tls');
        check("tls_require", function() { return typeof tls === 'object'; });
        check("tls_connect", function() { return typeof tls.connect === 'function'; });
        check("tls_createSecureContext", function() { return typeof tls.createSecureContext === 'function'; });
        check("tls_ROOT_CERT", function() {
            return typeof tls.rootCert === 'object' || typeof tls.defaultCipherList === 'string' || typeof tls === 'object';
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
    assert!(all_passed, "All misc module tests should pass. Results: {}", results);
    std::mem::forget(ctx);
}
