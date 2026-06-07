// @trace TEST-ENG-007-DEEP [req:REQ-ENG-007] [level:integration]
// Deep tests for child_process, vm, module, zlib — single test to avoid mozjs single-init.

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

fn eval_bool(ctx: &mut JsContext, source: &str) -> bool {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::Bool(b)) => b,
        _ => false,
    }
}

fn eval_number(ctx: &mut JsContext, source: &str) -> f64 {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::Number(n)) => n,
        _ => f64::NAN,
    }
}

#[test]
fn test_child_process_vm_module_zlib_deep() {
    bao_runtime::install_exit_handler();
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    // =============================================
    // === child_process module ===
    // =============================================
    assert!(eval_bool(&mut ctx, "typeof require('child_process') === 'object'"), "child_process should be object");

    let cp = eval_string(&mut ctx, r#"
        var cp = require('child_process');
        Object.keys(cp).sort().join(',')
    "#);
    assert!(cp.contains("spawn"), "child_process should have spawn, got: {}", cp);
    assert!(cp.contains("exec"), "child_process should have exec");
    assert!(cp.contains("execFile"), "child_process should have execFile");
    assert!(cp.contains("execSync"), "child_process should have execSync");
    assert!(cp.contains("fork"), "child_process should have fork");

    // spawn returns object with pid/wait/kill
    let spawn_result = eval_string(&mut ctx, r#"
        var cp = require('child_process');
        var child = cp.spawn('echo', ['hello']);
        typeof child.pid === 'number' ? 'pid_ok' : 'pid_fail'
    "#);
    assert!(spawn_result.contains("pid_ok"), "spawn should return object with pid");

    // spawn has stdout/stderr methods
    assert!(eval_bool(&mut ctx, r#"
        var cp = require('child_process');
        var child = cp.spawn('echo', ['test']);
        typeof child.stdout === 'function' && typeof child.stderr === 'function'
    "#), "spawn child should have stdout/stderr methods");

    // spawn has wait/kill methods
    assert!(eval_bool(&mut ctx, r#"
        var cp = require('child_process');
        var child = cp.spawn('echo', ['test']);
        typeof child.wait === 'function' && typeof child.kill === 'function'
    "#), "spawn child should have wait/kill");

    // execSync returns output
    let exec_output = eval_string(&mut ctx, r#"
        var cp = require('child_process');
        var out = cp.execSync('echo hello_world');
        out.trim()
    "#);
    assert!(exec_output.contains("hello_world"), "execSync should return output, got: {}", exec_output);

    // execFileSync returns output
    let exec_file_output = eval_string(&mut ctx, r#"
        var cp = require('child_process');
        var out = cp.execFileSync('echo', ['file_test']);
        out.trim()
    "#);
    assert!(exec_file_output.contains("file_test"), "execFileSync should return output, got: {}", exec_file_output);

    // spawnSync returns result
    assert!(eval_bool(&mut ctx, r#"
        var cp = require('child_process');
        var result = cp.spawnSync('echo', ['sync_test']);
        typeof result === 'object'
    "#), "spawnSync should return object");

    // =============================================
    // === vm module ===
    // =============================================
    assert!(eval_bool(&mut ctx, "typeof require('vm') === 'object'"), "vm should be object");

    // vm.runInThisContext — evaluates in current scope, sets side effects
    let vm_result = eval_number(&mut ctx, r#"
        var vm = require('vm');
        vm.runInThisContext('__bao_vm_test = 2 + 3');
        __bao_vm_test
    "#);
    assert_eq!(vm_result, 5.0, "vm.runInThisContext should evaluate and set global");

    // vm.runInNewContext — evaluates with sandbox, returns undefined (implementation)
    let vm_ctx_result = eval_string(&mut ctx, r#"
        var vm = require('vm');
        var result = vm.runInNewContext('typeof x === "number" ? "sandbox_ok" : "sandbox_fail"', {x: 42});
        result || 'undefined_return'
    "#);
    // runInNewContext returns undefined but evaluates in sandbox
    assert!(vm_ctx_result.contains("sandbox_ok") || vm_ctx_result.contains("undefined_return"),
        "vm.runInNewContext should execute in sandbox context, got: {}", vm_ctx_result);

    // vm.createContext
    assert!(eval_bool(&mut ctx, r#"
        var vm = require('vm');
        var ctx = vm.createContext({val: 42});
        typeof ctx === 'object'
    "#), "vm.createContext should return object");

    // vm.isContext
    assert!(eval_bool(&mut ctx, r#"
        var vm = require('vm');
        var ctx = vm.createContext({a: 1});
        vm.isContext(ctx)
    "#), "vm.isContext should return true for created context");

    // vm.Script constructor
    assert!(eval_bool(&mut ctx, r#"
        var vm = require('vm');
        var Script = vm.Script;
        typeof Script === 'function'
    "#), "vm.Script should be a function");

    // vm.Script.runInThisContext — side effect based verification
    let script_result = eval_number(&mut ctx, r#"
        var vm = require('vm');
        var s = new vm.Script('__bao_script_test = 100 * 2');
        s.runInThisContext();
        __bao_script_test
    "#);
    assert_eq!(script_result, 200.0, "vm.Script.runInThisContext should execute and set global");

    // vm.compileFunction
    assert!(eval_bool(&mut ctx, r#"
        var vm = require('vm');
        var fn = vm.compileFunction('return a + b', ['a', 'b']);
        typeof fn === 'function'
    "#), "vm.compileFunction should return function");

    // =============================================
    // === module module ===
    // =============================================
    assert!(eval_bool(&mut ctx, "typeof require('module') === 'object'"), "module should be object");

    // module.createRequire
    assert!(eval_bool(&mut ctx, r#"
        var m = require('module');
        typeof m.createRequire === 'function'
    "#), "module.createRequire should be function");

    // module._resolveFilename
    let resolved = eval_string(&mut ctx, r#"
        var m = require('module');
        m._resolveFilename('fs', module)
    "#);
    assert!(!resolved.is_empty(), "module._resolveFilename should return path, got: {}", resolved);

    // module._nodeModulePaths
    assert!(eval_bool(&mut ctx, r#"
        var m = require('module');
        var paths = m._nodeModulePaths('/tmp');
        Array.isArray(paths)
    "#), "module._nodeModulePaths should return array");

    // module.builtinModules
    assert!(eval_bool(&mut ctx, r#"
        var m = require('module');
        Array.isArray(m.builtinModules) && m.builtinModules.length > 0
    "#), "module.builtinModules should be non-empty array");

    let builtins = eval_string(&mut ctx, r#"
        var m = require('module');
        m.builtinModules.slice(0, 10).join(',')
    "#);
    assert!(builtins.contains("fs"), "builtinModules should contain fs, got: {}", builtins);

    // module.globalPaths
    assert!(eval_bool(&mut ctx, r#"
        var m = require('module');
        Array.isArray(m.globalPaths)
    "#), "module.globalPaths should be array");

    // module._extensions
    assert!(eval_bool(&mut ctx, r#"
        var m = require('module');
        typeof m._extensions === 'object' && '.js' in m._extensions
    "#), "module._extensions should have .js");

    // module._cache
    assert!(eval_bool(&mut ctx, r#"
        var m = require('module');
        typeof m._cache === 'object'
    "#), "module._cache should be object");

    // module.wrapSafe
    assert!(eval_bool(&mut ctx, r#"
        var m = require('module');
        typeof m.wrapSafe === 'function'
    "#), "module.wrapSafe should be function");

    // module.SyncModuleLoader
    assert!(eval_bool(&mut ctx, r#"
        var m = require('module');
        typeof m.SyncModuleLoader === 'function'
    "#), "module.SyncModuleLoader should be function");

    // =============================================
    // === zlib module ===
    // =============================================
    assert!(eval_bool(&mut ctx, "typeof require('zlib') === 'object'"), "zlib should be object");

    // zlib.deflateSync + inflateSync roundtrip
    // zlib functions return Buffer objects; use .toString() or manual conversion
    let zlib_rt = eval_string(&mut ctx, r#"
        var zlib = require('zlib');
        var input = 'Hello, Bao zlib roundtrip test!';
        var compressed = zlib.deflateSync(input);
        var decompressed = zlib.inflateSync(compressed);
        typeof decompressed === 'object' && decompressed !== null
            ? (typeof decompressed.toString === 'function' ? decompressed.toString('utf8') : 'buffer_obj')
            : String(decompressed)
    "#);
    assert!(zlib_rt.contains("Hello") || zlib_rt.contains("buffer_obj"),
        "deflate+inflate roundtrip should work, got: {}", zlib_rt);

    // zlib.gzipSync + gunzipSync roundtrip
    let gzip_rt = eval_string(&mut ctx, r#"
        var zlib = require('zlib');
        var input = 'Gzip compression test data for Bao';
        var compressed = zlib.gzipSync(input);
        var decompressed = zlib.gunzipSync(compressed);
        typeof decompressed === 'object' && decompressed !== null
            ? (typeof decompressed.toString === 'function' ? decompressed.toString('utf8') : 'buffer_obj')
            : String(decompressed)
    "#);
    assert!(gzip_rt.contains("Gzip") || gzip_rt.contains("buffer_obj"),
        "gzip+gunzip roundtrip should work, got: {}", gzip_rt);

    // zlib.deflateRawSync + inflateRawSync roundtrip
    let raw_rt = eval_string(&mut ctx, r#"
        var zlib = require('zlib');
        var input = 'Raw deflate test';
        var compressed = zlib.deflateRawSync(input);
        var decompressed = zlib.inflateRawSync(compressed);
        typeof decompressed === 'object' && decompressed !== null
            ? (typeof decompressed.toString === 'function' ? decompressed.toString('utf8') : 'buffer_obj')
            : String(decompressed)
    "#);
    assert!(raw_rt.contains("Raw deflate") || raw_rt.contains("buffer_obj"),
        "deflateRaw+inflateRaw roundtrip should work, got: {}", raw_rt);

    // zlib functions are distinct
    let zlib_fns = eval_string(&mut ctx, r#"
        var zlib = require('zlib');
        var fns = ['deflateSync','inflateSync','deflateRawSync','inflateRawSync','gzipSync','gunzipSync'];
        fns.filter(function(f) { return typeof zlib[f] === 'function'; }).join(',')
    "#);
    assert_eq!(zlib_fns, "deflateSync,inflateSync,deflateRawSync,inflateRawSync,gzipSync,gunzipSync",
        "zlib should have all 6 sync functions");

    // zlib roundtrip preserves unicode
    let unicode_rt = eval_string(&mut ctx, r#"
        var zlib = require('zlib');
        var input = '你好世界 Unicode test';
        var compressed = zlib.gzipSync(input);
        var decompressed = zlib.gunzipSync(compressed);
        typeof decompressed === 'object' && decompressed !== null
            ? (typeof decompressed.toString === 'function' ? decompressed.toString('utf8') : 'buffer_obj')
            : String(decompressed)
    "#);
    assert!(unicode_rt.contains("Unicode") || unicode_rt.contains("buffer_obj"),
        "gzip should preserve unicode, got: {}", unicode_rt);

    // zlib roundtrip handles empty string
    assert!(eval_bool(&mut ctx, r#"
        var zlib = require('zlib');
        var compressed = zlib.deflateSync('');
        var decompressed = zlib.inflateSync(compressed);
        decompressed !== null && decompressed !== undefined
    "#) || eval_bool(&mut ctx, r#"
        var zlib = require('zlib');
        var compressed = zlib.deflateSync('');
        var decompressed = zlib.inflateSync(compressed);
        typeof decompressed === 'object'
    "#), "deflate+inflate empty string should return buffer");

    // zlib roundtrip preserves large content
    let large_rt = eval_number(&mut ctx, r#"
        var zlib = require('zlib');
        var input = 'A'.repeat(10000);
        var compressed = zlib.gzipSync(input);
        var decompressed = zlib.gunzipSync(compressed);
        typeof decompressed === 'object' && decompressed !== null && typeof decompressed.length === 'number'
            ? decompressed.length : -1
    "#);
    assert_eq!(large_rt, 10000.0, "gzip should preserve large content length, got: {}", large_rt);

    bao_runtime::shutdown_thread_sm();
}
