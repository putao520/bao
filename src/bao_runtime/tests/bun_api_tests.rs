// @trace TEST-ENG-006 [req:REQ-ENG-006] [level:integration]
// Integration tests for Bun.* / Bao.* API (REQ-ENG-006)
//
// All tests run in a single #[test] function to avoid mozjs Runtime
// per-thread singleton issues — creating/destroying JsContext across
// multiple test functions causes segfaults.

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn eval_bool(ctx: &mut JsContext, source: &str) -> bool {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::Bool(b)) => b,
        Ok(JsValue::String(s)) => s == "true",
        _ => false,
    }
}

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        _ => String::new(),
    }
}

fn eval_number(ctx: &mut JsContext, source: &str) -> f64 {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::Number(n)) => n,
        _ => f64::NAN,
    }
}

fn eval_ok(ctx: &mut JsContext, source: &str) -> bool {
    ctx.eval(source, "<test>").is_ok()
}

fn escape_path(p: &str) -> String {
    p.replace('\\', "\\\\").replace('"', "\\\"")
}

#[test]
fn test_bun_api_all() {
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::new().expect("Failed to create JSContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    // --- C6: Bao.* is alias of Bun.* (same object) ---
    assert!(eval_bool(&mut ctx, "Bun === Bao"), "Bao should be same object as Bun");
    assert!(eval_bool(&mut ctx, "Bun.env === Bao.env"), "Bao.env should be same as Bun.env");
    assert!(eval_bool(&mut ctx, "Bun.version === Bao.version"), "Bao.version should equal Bun.version");
    assert!(eval_bool(&mut ctx, r#"
        typeof Bao.file === "function" &&
        typeof Bao.write === "function" &&
        typeof Bao.readFile === "function" &&
        typeof Bao.serve === "function" &&
        typeof Bao.spawn === "function" &&
        typeof Bao.cwd === "function" &&
        typeof Bao.version === "string"
    "#), "Bao.* should have same methods as Bun.*");

    // --- Bun basic properties ---
    assert!(!eval_string(&mut ctx, "Bun.version").is_empty(), "Bun.version not empty");
    assert!(eval_bool(&mut ctx, r#"typeof Bun.env.PATH === "string" && Bun.env.PATH.length > 0"#), "Bun.env.PATH");
    assert!(eval_bool(&mut ctx, "Array.isArray(Bun.argv) && Bun.argv.length > 0"), "Bun.argv");
    assert!(!eval_string(&mut ctx, "Bun.cwd()").is_empty(), "Bun.cwd()");
    assert!(eval_ok(&mut ctx, "Bun.gc()"), "Bun.gc()");
    assert!(!eval_string(&mut ctx, "Bun.revision").is_empty(), "Bun.revision");
    assert_eq!(eval_string(&mut ctx, "typeof Bun.main"), "string", "Bun.main");

    // --- Bun functions exist ---
    assert!(eval_bool(&mut ctx, "typeof Bun.exit === 'function'"), "Bun.exit");
    assert!(eval_bool(&mut ctx, "typeof Bun.sleep === 'function'"), "Bun.sleep");
    assert!(eval_bool(&mut ctx, "typeof Bun.sleepSync === 'function'"), "Bun.sleepSync");
    assert!(eval_bool(&mut ctx, "typeof Bun.spawn === 'function'"), "Bun.spawn");
    assert!(eval_bool(&mut ctx, "typeof Bun.build === 'function'"), "Bun.build");
    assert!(eval_bool(&mut ctx, "typeof Bun.resolve === 'function'"), "Bun.resolve");
    assert!(eval_bool(&mut ctx, "typeof Bun.test === 'function'"), "Bun.test");
    assert!(eval_bool(&mut ctx, "typeof Bun.testRun === 'function'"), "Bun.testRun");
    assert!(eval_bool(&mut ctx, "Bun.read === Bun.readFile"), "Bun.read alias");

    // --- Bun.hash ---
    let sha256 = eval_string(&mut ctx, r#"Bun.hash("hello", "sha256")"#);
    assert_eq!(sha256.len(), 64, "SHA-256 hash should be 64 hex chars");
    let sha256_default = eval_string(&mut ctx, r#"Bun.hash("hello")"#);
    assert!(!sha256_default.is_empty(), "Bun.hash() default algo");
    let sha512 = eval_string(&mut ctx, r#"Bun.hash("hello", "sha512")"#);
    assert_eq!(sha512.len(), 128, "SHA-512 hash should be 128 hex chars");

    // --- Bun.inspect ---
    let inspected = eval_string(&mut ctx, r#"Bun.inspect("hello")"#);
    assert!(inspected.contains("hello"), "Bun.inspect");

    // --- Bun.which ---
    assert!(eval_bool(&mut ctx, r#"typeof Bun.which("ls") === "string" || Bun.which("ls") === null"#), "Bun.which");

    // --- Bun.serve ---
    assert!(eval_bool(&mut ctx, r#"
        var server = Bun.serve({ port: 0 });
        typeof server === "object" &&
        typeof server.port === "number" &&
        typeof server.stop === "function" &&
        typeof server.ref === "function" &&
        typeof server.unref === "function"
    "#), "Bun.serve() returns object with stop/ref/unref");

    // --- Bun.file ---
    let tmp = ::std::env::temp_dir().join("bao_test_file.txt");
    ::std::fs::write(&tmp, b"hello bao").unwrap();
    let path = escape_path(&tmp.to_string_lossy());
    assert!(eval_bool(&mut ctx, &format!(r#"typeof Bun.file("{}").path === "string""#, path)), "Bun.file path");
    assert!(eval_bool(&mut ctx, &format!(r#"Bun.file("{}").size === 9"#, path)), "Bun.file size");
    assert!(eval_bool(&mut ctx, &format!(r#"Bun.file("{}").exists === true"#, path)), "Bun.file exists");
    let _ = ::std::fs::remove_file(&tmp);

    // --- Bun.write ---
    let tmp2 = ::std::env::temp_dir().join("bao_test_write.txt");
    let _ = ::std::fs::remove_file(&tmp2);
    let path2 = escape_path(&tmp2.to_string_lossy());
    assert!(eval_ok(&mut ctx, &format!(r#"Bun.write("{}", "hello world")"#, path2)), "Bun.write ok");
    assert_eq!(::std::fs::read_to_string(&tmp2).unwrap(), "hello world", "Bun.write content");
    let written = eval_number(&mut ctx, &format!(r#"Bun.write("{}", "abc")"#, path2));
    assert_eq!(written as i32, 3, "Bun.write returns bytes");
    let _ = ::std::fs::remove_file(&tmp2);

    // --- Bun.readFile ---
    let tmp3 = ::std::env::temp_dir().join("bao_test_read.txt");
    ::std::fs::write(&tmp3, b"test content").unwrap();
    let path3 = escape_path(&tmp3.to_string_lossy());
    assert_eq!(eval_string(&mut ctx, &format!(r#"Bun.readFile("{}")"#, path3)), "test content", "Bun.readFile");
    let _ = ::std::fs::remove_file(&tmp3);

    // --- fetch ---
    assert!(eval_bool(&mut ctx, "typeof fetch === 'function'"), "fetch global");

    // --- WebSocket ---
    assert!(eval_bool(&mut ctx, "typeof WebSocket === 'function'"), "WebSocket constructor");

    // --- process global ---
    assert!(!eval_string(&mut ctx, "process.arch").is_empty(), "process.arch");
    assert!(!eval_string(&mut ctx, "process.platform").is_empty(), "process.platform");
    assert!(eval_string(&mut ctx, "process.version").starts_with('v'), "process.version");
    assert!(eval_bool(&mut ctx, "Array.isArray(process.argv) && process.argv.length > 0"), "process.argv");
    assert!(eval_bool(&mut ctx, r#"typeof process.env.PATH === "string""#), "process.env.PATH");
    assert!(eval_bool(&mut ctx, "typeof process.cwd() === 'string'"), "process.cwd()");
    assert!(eval_bool(&mut ctx, "typeof process.pid === 'number' && process.pid > 0"), "process.pid");
    assert!(eval_bool(&mut ctx, "typeof process.ppid === 'number' && process.ppid > 0"), "process.ppid");
    assert_eq!(eval_string(&mut ctx, "process.title"), "bao", "process.title");
    assert!(eval_bool(&mut ctx, r#"
        typeof process.versions === "object" &&
        typeof process.versions.node === "string" &&
        typeof process.versions.bao === "string"
    "#), "process.versions");
    assert!(eval_bool(&mut ctx, r#"
        typeof process.stdout === "object" &&
        typeof process.stdout.write === "function" &&
        process.stdout.fd === 1
    "#), "process.stdout");
    assert!(eval_bool(&mut ctx, r#"
        typeof process.stderr === "object" &&
        typeof process.stderr.write === "function" &&
        process.stderr.fd === 2
    "#), "process.stderr");
    assert!(eval_bool(&mut ctx, r#"
        typeof process.stdin === "object" &&
        process.stdin.fd === 0 &&
        process.stdin.readable === true
    "#), "process.stdin");
    assert!(eval_bool(&mut ctx, "typeof process.on === 'function'"), "process.on");
    assert!(eval_bool(&mut ctx, "typeof process.nextTick === 'function'"), "process.nextTick");
    assert!(eval_bool(&mut ctx, r#"
        var t = process.hrtime();
        Array.isArray(t) && t.length === 2 && typeof t[0] === "number"
    "#), "process.hrtime");
    assert!(eval_bool(&mut ctx, "typeof process.hrtime.bigint === 'function'"), "process.hrtime.bigint");
    assert!(eval_bool(&mut ctx, "typeof process.uptime() === 'number' && process.uptime() >= 0"), "process.uptime");
    assert!(eval_bool(&mut ctx, r#"
        var m = process.memoryUsage();
        typeof m.rss === "number" && typeof m.heapTotal === "number"
    "#), "process.memoryUsage");
    assert!(eval_bool(&mut ctx, r#"
        typeof process.release === "object" &&
        process.release.name === "bao"
    "#), "process.release");

    // Leak the context to prevent mozjs drop-order crashes on thread exit
    std::mem::forget(ctx);
}
