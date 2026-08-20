// @trace TEST-ENG-006 [req:REQ-ENG-006] [level:e2e]
// Bun.build JS-face end-to-end (REQ-ENG-006): the JS call → BuildTasklet
// (worker thread, full bun_bundler pipeline) → ConcurrentTask resolve on the
// JS thread. Asserts the upstream BuildOutput contract at the byte level:
//   * Promise<BuildOutput> semantics (build failures RESOLVE with
//     success:false + logs; only invalid config throws),
//   * TS/TSX transpiled artifact bytes via the BuildArtifact Blob face
//     (`await artifact.text()`),
//   * outdir disk writes byte-identical to the artifact bytes,
//   * minify changes/shrinks the output bytes,
//   * multi-entry → one output per entry, external passthrough.
//
// The native driver is installed like the product does (bao_cli::run →
// bao_bundler::build_api::install).

use std::path::PathBuf;
use std::time::Duration;

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
use bun_runtime::timers;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "bao_bun_build_e2e_{}_{}",
        name,
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &std::path::Path, name: &str, contents: &str) -> String {
    let p = dir.join(name);
    std::fs::write(&p, contents).unwrap();
    p.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"")
}

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(JsValue::Null) => "null".to_string(),
        Ok(JsValue::Undefined) => "undefined".to_string(),
        Ok(JsValue::Object(_)) => "[object]".to_string(),
        Err(e) => format!("ERROR:{}", e.message),
    }
}

/// Drive the JS thread's MiniEventLoop: RunJobs + tick, with a deadline.
/// Bun.build resolves via a ConcurrentTask, so ticks must run for both the
/// worker completion AND the promise reaction jobs.
fn drive_event_loop(ctx: &mut JsContext, max_iters: usize) {
    // BCE (bun_build_e2e flake, 2026-08-19): this loop used to be a BLIND
    // fixed window (1500 passes x 2ms = 3s) racing the BuildTasklet
    // worker's completion time — under CPU contention the worker overshoots
    // the window, `settled` stays false and the assertion fires at a
    // drifting scenario (observed at lines 278 and 202 across runs).
    // Event-driven wait instead: poll BOTH settle flags (build scenarios
    // flip `__r.settled`; the artifact.text() scenario fills `__r.text`)
    // and exit the moment either holds; the iteration cap stays as the
    // deadline only (raised to 30s), never as a blind duration.
    let cx_raw = ctx.raw_cx();
    for _ in 0..max_iters {
        unsafe {
            mozjs_sys::jsapi::js::RunJobs(cx_raw);
        }
        timers::with_event_loop(|loop_| {
            loop_.tick_without_idle(std::ptr::null_mut());
        });
        std::thread::sleep(Duration::from_millis(2));
        if eval_string(
            ctx,
            "(globalThis.__r && globalThis.__r.settled === true) || \
             (globalThis.__r && globalThis.__r.text !== null && globalThis.__r.text !== undefined)",
        ) == "true"
        {
            return;
        }
    }
}

/// Run one Bun.build scenario: eval a script that awaits the build and
/// stashes the settled result fields on `globalThis.__r`, then read them.
/// `body` is the config literal.
fn run_build_scenario(ctx: &mut JsContext, config: &str) {
    let script = format!(
        r#"
        globalThis.__r = {{ settled: false, rejected: false, value: null, error: null }};
        Bun.build({}).then(
            function (out) {{ globalThis.__r.settled = true; globalThis.__r.value = out; }},
            function (err) {{ globalThis.__r.settled = true; globalThis.__r.rejected = true; globalThis.__r.error = String(err); }}
        );
        "#,
        config
    );
    match ctx.eval(&script, "<test>") {
        Ok(_) => {}
        Err(e) => panic!("Bun.build eval failed: {}", e.message),
    }
    drive_event_loop(ctx, 15000);
}

fn settled(ctx: &mut JsContext) -> bool {
    eval_string(ctx, "globalThis.__r && globalThis.__r.settled") == "true"
}

#[test]
fn test_bun_build_e2e_all() {
    // Pool workers assert STDOUT_STREAM_SET at startup (fetch e2e parity).
    bun_core::output::init_test();
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    bao_bundler::build_api::install();
    assert!(
        bun_runtime::bun_build::native_build_installed(),
        "native bundle driver must be installed for the e2e"
    );

    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    // ── shape: Bun.build exists and returns a Promise ────────────────────
    assert_eq!(eval_string(&mut ctx, "typeof Bun.build"), "function");
    let dir0 = temp_dir("promise_shape");
    let entry0 = write(&dir0, "entry.ts", "export const ok = 1;\n");
    assert_eq!(
        eval_string(
            &mut ctx,
            &format!("typeof Bun.build({{ entrypoints: [\"{}\"] }}).then", entry0)
        ),
        "function",
        "Bun.build() must return a Promise-shaped value"
    );

    // ── invalid config throws (upstream throwInvalidArguments parity) ────
    assert!(ctx.eval("Bun.build()", "<t>").is_err(), "no-arg call must throw");
    assert!(
        ctx.eval("Bun.build({})", "<t>").is_err(),
        "missing entrypoints must throw"
    );
    assert!(
        ctx.eval("Bun.build({ entrypoints: [] })", "<t>").is_err(),
        "empty entrypoints must throw"
    );
    drive_event_loop(&mut ctx, 5); // flush the promise-shape probe's reactions

    // ── single TS entry: transpiled bytes via the Blob face ─────────────
    let dir = temp_dir("single_ts");
    let entry = write(&dir, "entry.ts", "const x: number = 41;\nexport const answer: number = x + 1;\n");
    run_build_scenario(
        &mut ctx,
        &format!(r#"{{ entrypoints: ['{}'] }}"#, entry),
    );
    assert!(settled(&mut ctx), "promise must settle");
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__r.rejected"),
        "false",
        "successful build must not reject"
    );
    assert_eq!(eval_string(&mut ctx, "globalThis.__r.value.success"), "true");
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__r.value.outputs.length"),
        "1"
    );
    let out_path = eval_string(&mut ctx, "globalThis.__r.value.outputs[0].path");
    assert!(out_path.ends_with(".js"), "output path {} should be .js", out_path);
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__r.value.outputs[0].kind"),
        "entry-point"
    );
    // Byte-level: TS annotations stripped, body present, size consistent.
    let text = eval_string(
        &mut ctx,
        "globalThis.__r.value.outputs[0].text()",
    );
    // text() is async — drive then read the stashed result.
    let _ = text; // (the reaction below re-reads after settling)
    run_await_text_scenario(&mut ctx);
    let body = eval_string(&mut ctx, "globalThis.__r.text");
    assert!(
        body.contains("41"),
        "artifact bytes must contain the TS body, got: {:?}",
        body
    );
    assert!(
        !body.contains(": number"),
        "TS type annotations must be stripped, got: {}",
        body
    );
    let size = eval_string(&mut ctx, "globalThis.__r.value.outputs[0].size");
    let size_num: f64 = size.parse().unwrap_or(0.0);
    assert!(
        size_num > 0.0,
        "artifact size must be positive, got: {}",
        size
    );
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__r.value.logs.length"),
        "0",
        "successful build should carry no error logs"
    );

    // ── TSX: JSX lowered through the jsx runtime ──────────────────────────
    let dir = temp_dir("tsx");
    let entry = write(
        &dir,
        "app.tsx",
        "export const el = <div className=\"greet\">hi</div>;\n",
    );
    // classic runtime: React.createElement default factory, no react
    // package resolution needed (upstream classic semantics)
    run_build_scenario(
        &mut ctx,
        &format!(r#"{{ entrypoints: ['{}'], jsx: {{ runtime: 'classic' }} }}"#, entry),
    );
    assert!(settled(&mut ctx));
    let tsx_success = eval_string(&mut ctx, "globalThis.__r.value.success");
    if tsx_success != "true" {
        let tsx_logs = eval_string(
            &mut ctx,
            "globalThis.__r.value.logs.map(function(l){return l.level+':'+l.message;}).join(' || ')",
        );
        panic!("TSX build failed, logs: {}", tsx_logs);
    }
    run_await_text_scenario(&mut ctx);
    let body = eval_string(&mut ctx, "globalThis.__r.text");
    assert!(
        body.contains("greet") && body.contains("React.createElement"),
        "JSX must be lowered to React.createElement calls, got: {}",
        body
    );

    // ── minify: output bytes differ and shrink ────────────────────────────
    let dir = temp_dir("minify");
    let entry = write(
        &dir,
        "entry.ts",
        "function add(a: number, b: number): number {\n  return a + b;\n}\nexport const r = add(1, 2);\n",
    );
    run_build_scenario(
        &mut ctx,
        &format!(r#"{{ entrypoints: ['{}'] }}"#, entry),
    );
    assert!(settled(&mut ctx));
    run_await_text_scenario(&mut ctx);
    let plain = eval_string(&mut ctx, "globalThis.__r.text");
    run_build_scenario(
        &mut ctx,
        &format!(r#"{{ entrypoints: ['{}'], minify: true }}"#, entry),
    );
    assert!(settled(&mut ctx));
    run_await_text_scenario(&mut ctx);
    let minified = eval_string(&mut ctx, "globalThis.__r.text");
    assert_ne!(plain, minified, "minify must change the output bytes");
    assert!(
        minified.len() < plain.len(),
        "minified output should be smaller: {} vs {}",
        minified.len(),
        plain.len()
    );

    // ── multi entry ───────────────────────────────────────────────────────
    let dir = temp_dir("multi");
    let a = write(&dir, "alpha.ts", "export const a = 1;\n");
    let b = write(&dir, "beta.ts", "export const b = 2;\n");
    run_build_scenario(
        &mut ctx,
        &format!(r#"{{ entrypoints: ['{}', '{}'] }}"#, a, b),
    );
    assert!(settled(&mut ctx));
    assert_eq!(eval_string(&mut ctx, "globalThis.__r.value.success"), "true");
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__r.value.outputs.length"),
        "2",
        "two entries → two outputs"
    );

    // ── external passthrough ──────────────────────────────────────────────
    let dir = temp_dir("external");
    let entry = write(
        &dir,
        "entry.ts",
        "import leftpad from \"left-pad\";\nexport const v = leftpad(\"a\", 3);\n",
    );
    run_build_scenario(
        &mut ctx,
        &format!(
            r#"{{ entrypoints: ['{}'], external: ['left-pad'] }}"#,
            entry
        ),
    );
    assert!(settled(&mut ctx));
    assert_eq!(eval_string(&mut ctx, "globalThis.__r.value.success"), "true");
    run_await_text_scenario(&mut ctx);
    let body = eval_string(&mut ctx, "globalThis.__r.text");
    assert!(
        body.contains("left-pad"),
        "external specifier must pass through, got: {}",
        body
    );

    // ── outdir: disk bytes identical to artifact bytes ───────────────────
    let dir = temp_dir("outdir");
    let outdir = dir.join("dist");
    let entry = write(&dir, "entry.ts", "export const marker = \"outdir-bytes\";\n");
    run_build_scenario(
        &mut ctx,
        &format!(
            r#"{{ entrypoints: ['{}'], outdir: '{}' }}"#,
            entry,
            outdir.to_string_lossy()
        ),
    );
    assert!(settled(&mut ctx));
    assert_eq!(eval_string(&mut ctx, "globalThis.__r.value.success"), "true");
    let rel_path = eval_string(&mut ctx, "globalThis.__r.value.outputs[0].path");
    let disk = outdir.join(&rel_path);
    let disk_bytes = std::fs::read(&disk).unwrap_or_else(|e| {
        panic!("outdir artifact {} must exist on disk: {}", disk.display(), e)
    });
    run_await_text_scenario(&mut ctx);
    let artifact_text = eval_string(&mut ctx, "globalThis.__r.text");
    assert_eq!(
        String::from_utf8_lossy(&disk_bytes),
        artifact_text,
        "disk bytes must equal the artifact bytes"
    );

    // ── failure face: missing entry resolves with success:false + logs ───
    run_build_scenario(
        &mut ctx,
        r#"{ entrypoints: ["/nonexistent/no-such-entry-xyz.ts"] }"#,
    );
    assert!(settled(&mut ctx), "failure promise must settle");
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__r.rejected"),
        "false",
        "build failure must RESOLVE (upstream semantics), not reject"
    );
    assert_eq!(eval_string(&mut ctx, "globalThis.__r.value.success"), "false");
    let logs_len: f64 = eval_string(&mut ctx, "globalThis.__r.value.logs.length")
        .parse()
        .unwrap_or(0.0);
    assert!(logs_len > 0.0, "failure face must carry logs");
    let log_text = eval_string(&mut ctx, "globalThis.__r.value.logs[0].message");
    assert!(
        !log_text.is_empty() && !log_text.starts_with("ERROR:"),
        "log message must be populated, got: {}",
        log_text
    );

    bun_runtime::shutdown_thread_sm();
}

/// Await `outputs[0].text()` from the LAST settled build and stash it on
/// `globalThis.__r.text` (re-uses the same drive loop).
fn run_await_text_scenario(ctx: &mut JsContext) {
    let script = r#"
        globalThis.__r.text = null;
        globalThis.__r.value.outputs[0].text().then(function (t) {
            globalThis.__r.text = t;
        });
    "#;
    ctx.eval(script, "<test>")
        .expect("artifact.text() eval must succeed");
    drive_event_loop(ctx, 15000);
    assert_eq!(
        eval_string(ctx, "globalThis.__r.text === null"),
        "false",
        "artifact.text() must settle (Blob face)"
    );
}
