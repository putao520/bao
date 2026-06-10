// @trace TEST-ENG-007-PROC [req:REQ-ENG-007] [level:integration]
// Process API deep tests: arch, platform, version, env, cwd, hrtime, uptime, memoryUsage, etc.

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
fn test_process_deep_all() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    // === process existence ===
    assert!(eval_bool(&mut ctx, "typeof process === 'object'"), "process should be object");
    assert!(eval_bool(&mut ctx, "process !== null"), "process should not be null");

    // === process.arch ===
    let arch = eval_string(&mut ctx, "process.arch");
    assert!(!arch.is_empty(), "process.arch should be non-empty, got: {}", arch);
    assert!(
        ["x86_64", "aarch64", "x86", "arm", "riscv64"].iter().any(|&a| a == arch),
        "process.arch should be a known arch, got: {}", arch
    );

    // === process.platform ===
    let platform = eval_string(&mut ctx, "process.platform");
    assert!(!platform.is_empty(), "process.platform should be non-empty");
    assert!(
        ["linux", "macos", "darwin", "win32", "freebsd", "openbsd"].iter().any(|&p| p == platform),
        "process.platform should be a known platform, got: {}", platform
    );

    // === process.version ===
    let version = eval_string(&mut ctx, "process.version");
    assert!(version.starts_with('v'), "process.version should start with 'v', got: {}", version);

    // === process.versions ===
    assert!(eval_bool(&mut ctx, "typeof process.versions === 'object'"), "process.versions should be object");
    let node_ver = eval_string(&mut ctx, "process.versions.node");
    assert!(!node_ver.is_empty(), "process.versions.node should be non-empty");
    let bao_ver = eval_string(&mut ctx, "process.versions.bao");
    assert!(!bao_ver.is_empty(), "process.versions.bao should exist");
    let sm_ver = eval_string(&mut ctx, "process.versions.spidermonkey");
    assert!(!sm_ver.is_empty(), "process.versions.spidermonkey should exist");
    let rust_ver = eval_string(&mut ctx, "process.versions.rust");
    assert!(!rust_ver.is_empty(), "process.versions.rust should exist");
    let bun_ver = eval_string(&mut ctx, "process.versions.bun");
    assert!(!bun_ver.is_empty(), "process.versions.bun should exist");

    // === process.argv ===
    assert!(eval_bool(&mut ctx, "Array.isArray(process.argv)"), "process.argv should be array");
    assert!(eval_bool(&mut ctx, "process.argv.length > 0"), "process.argv should have at least one element");

    // === process.argv0 ===
    let argv0 = eval_string(&mut ctx, "process.argv0");
    assert!(!argv0.is_empty(), "process.argv0 should be non-empty");

    // === process.env ===
    assert!(eval_bool(&mut ctx, "typeof process.env === 'object'"), "process.env should be object");
    let path_val = eval_string(&mut ctx, "process.env.PATH || ''");
    assert!(!path_val.is_empty() || !eval_bool(&mut ctx, "'PATH' in process.env"),
        "process.env.PATH should exist if PATH is set");

    // === process.env write (Proxy-backed) ===
    let set_result = eval_string(&mut ctx, r#"
        process.env.__BAO_TEST_VAR = "hello";
        process.env.__BAO_TEST_VAR
    "#);
    assert_eq!(set_result, "hello", "process.env write should work");

    // === process.env delete ===
    let del_result = eval_string(&mut ctx, r#"
        delete process.env.__BAO_TEST_VAR;
        process.env.__BAO_TEST_VAR
    "#);
    assert_eq!(del_result, "", "process.env delete should remove key");

    // === process.cwd() ===
    let cwd = eval_string(&mut ctx, "process.cwd()");
    assert!(!cwd.is_empty(), "process.cwd() should return non-empty string");

    // === process.pid ===
    let pid = eval_number(&mut ctx, "process.pid");
    assert!(pid > 0.0, "process.pid should be positive, got: {}", pid);

    // === process.ppid ===
    let ppid = eval_number(&mut ctx, "process.ppid");
    assert!(ppid >= 0.0, "process.ppid should be non-negative, got: {}", ppid);

    // === process.title ===
    let title = eval_string(&mut ctx, "process.title");
    assert!(!title.is_empty(), "process.title should be non-empty");

    // === process.stdout ===
    assert!(eval_bool(&mut ctx, "typeof process.stdout === 'object'"), "process.stdout should be object");
    assert!(eval_bool(&mut ctx, "typeof process.stdout.write === 'function'"), "stdout.write should be function");
    let stdout_fd = eval_number(&mut ctx, "process.stdout.fd");
    assert_eq!(stdout_fd, 1.0, "stdout.fd should be 1");
    assert!(eval_bool(&mut ctx, "typeof process.stdout.isTTY === 'boolean'"), "stdout.isTTY should be boolean");

    // === process.stderr ===
    assert!(eval_bool(&mut ctx, "typeof process.stderr === 'object'"), "process.stderr should be object");
    assert!(eval_bool(&mut ctx, "typeof process.stderr.write === 'function'"), "stderr.write should be function");
    let stderr_fd = eval_number(&mut ctx, "process.stderr.fd");
    assert_eq!(stderr_fd, 2.0, "stderr.fd should be 2");

    // === process.stdin ===
    assert!(eval_bool(&mut ctx, "typeof process.stdin === 'object'"), "process.stdin should be object");
    assert!(eval_bool(&mut ctx, "typeof process.stdin.read === 'function'"), "stdin.read should be function");
    assert!(eval_bool(&mut ctx, "typeof process.stdin.on === 'function'"), "stdin.on should be function");
    assert!(eval_bool(&mut ctx, "typeof process.stdin.pipe === 'function'"), "stdin.pipe should be function");
    assert!(eval_bool(&mut ctx, "typeof process.stdin.resume === 'function'"), "stdin.resume should be function");
    assert!(eval_bool(&mut ctx, "typeof process.stdin.pause === 'function'"), "stdin.pause should be function");
    assert!(eval_bool(&mut ctx, "typeof process.stdin.destroy === 'function'"), "stdin.destroy should be function");
    let stdin_fd = eval_number(&mut ctx, "process.stdin.fd");
    assert_eq!(stdin_fd, 0.0, "stdin.fd should be 0");
    assert!(eval_bool(&mut ctx, "process.stdin.readable === true"), "stdin.readable should be true");

    // === process.on ===
    assert!(eval_bool(&mut ctx, "typeof process.on === 'function'"), "process.on should be function");

    // === process.nextTick ===
    assert!(eval_bool(&mut ctx, "typeof process.nextTick === 'function'"), "process.nextTick should be function");

    // === process.hrtime() ===
    assert!(eval_bool(&mut ctx, "typeof process.hrtime === 'function'"), "process.hrtime should be function");
    let hrtime = eval_string(&mut ctx, "JSON.stringify(process.hrtime())");
    assert!(hrtime.contains(","), "hrtime() should return array [seconds, nanos], got: {}", hrtime);
    let hrtime_secs = eval_number(&mut ctx, "process.hrtime()[0]");
    assert!(hrtime_secs >= 0.0, "hrtime seconds should be non-negative");

    // === process.hrtime.bigint() ===
    assert!(eval_bool(&mut ctx, "typeof process.hrtime.bigint === 'function'"), "hrtime.bigint should be function");

    // === process.uptime() ===
    assert!(eval_bool(&mut ctx, "typeof process.uptime === 'function'"), "process.uptime should be function");
    let uptime = eval_number(&mut ctx, "process.uptime()");
    assert!(uptime > 0.0, "process.uptime() should be positive, got: {}", uptime);

    // === process.memoryUsage() ===
    assert!(eval_bool(&mut ctx, "typeof process.memoryUsage === 'function'"), "process.memoryUsage should be function");
    let mem = eval_string(&mut ctx, "JSON.stringify(process.memoryUsage())");
    assert!(mem.contains("rss"), "memoryUsage should have rss, got: {}", mem);
    assert!(mem.contains("heapTotal"), "memoryUsage should have heapTotal");
    assert!(mem.contains("heapUsed"), "memoryUsage should have heapUsed");
    let rss = eval_number(&mut ctx, "process.memoryUsage().rss");
    assert!(rss > 0.0, "memoryUsage.rss should be positive, got: {}", rss);

    // === process.kill ===
    assert!(eval_bool(&mut ctx, "typeof process.kill === 'function'"), "process.kill should be function");

    // === process.umask ===
    assert!(eval_bool(&mut ctx, "typeof process.umask === 'function'"), "process.umask should be function");
    let umask = eval_number(&mut ctx, "process.umask()");
    assert!(umask >= 0.0, "umask should be non-negative, got: {}", umask);

    // === process.config ===
    assert!(eval_bool(&mut ctx, "typeof process.config === 'object'"), "process.config should be object");
    assert!(eval_bool(&mut ctx, "typeof process.config.variables === 'object'"), "process.config.variables should be object");

    // === process.release ===
    assert!(eval_bool(&mut ctx, "typeof process.release === 'object'"), "process.release should be object");
    let release_name = eval_string(&mut ctx, "process.release.name");
    assert!(!release_name.is_empty(), "process.release.name should be non-empty");
    let source_url = eval_string(&mut ctx, "process.release.sourceUrl");
    assert!(!source_url.is_empty(), "process.release.sourceUrl should be non-empty");

    // === process.execPath ===
    let _exec_path = eval_string(&mut ctx, "typeof process.execPath === 'string' ? process.execPath : ''");
    assert!(eval_bool(&mut ctx, "typeof process.execPath === 'string'"), "process.execPath should be string");

    // === process.exit ===
    assert!(eval_bool(&mut ctx, "typeof process.exit === 'function'"), "process.exit should be function");

    // === process.chdir ===
    assert!(eval_bool(&mut ctx, "typeof process.chdir === 'function'"), "process.chdir should be function");

    bun_runtime::shutdown_thread_sm();
}
