// @trace TEST-ENG-006 [req:REQ-ENG-006] [level:integration]
// Rust-level probe for the Bun.build native driver (`build_api::run_bundle`
// via `install()`): drives the full bun_bundler BundleV2 pipeline on a plain
// test thread — no JSContext — so pipeline breakage is diagnosable without
// the JS face in the loop.

use std::path::PathBuf;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bao_build_api_tests_{}_{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &std::path::Path, name: &str, contents: &str) -> String {
    let p = dir.join(name);
    std::fs::write(&p, contents).unwrap();
    p.to_string_lossy().into_owned()
}

use bun_runtime::bun_build::{NativeBuildConfig, NativeMinify};

fn base_config(entrypoints: Vec<String>) -> NativeBuildConfig {
    NativeBuildConfig {
        entrypoints,
        outdir: None,
        root: None,
        target: "browser".into(),
        format: "esm".into(),
        naming: None,
        naming_entry: None,
        naming_chunk: None,
        naming_asset: None,
        minify: NativeMinify::default(),
        sourcemap: "none".into(),
        external: Vec::new(),
        define: Vec::new(),
        splitting: false,
        banner: None,
        footer: None,
        public_path: None,
        jsx_runtime: None,
        jsx_factory: None,
        jsx_fragment: None,
        jsx_import_source: None,
        jsx_development: None,
    }
}

/// The driver is not public (registered through the install hook); exercise
/// it through the registry exactly as the JS face does.
fn run(config: &NativeBuildConfig) -> bun_runtime::bun_build::NativeBuildResult {
    bao_bundler::build_api::install();
    assert!(
        bun_runtime::bun_build::native_build_installed(),
        "native driver must register via install()"
    );
    // Re-register is a no-op (OnceLock first-wins) — same driver anyway.
    bao_bundler::build_api::install();
    bao_bundler::build_api::run_bundle_for_test(config)
}

/// Process-owner output bring-up: pool workers assert STDOUT_STREAM_SET at
/// startup (`Source::configure_thread`); without this the first spawned
/// worker panics and `wait_for_parse` never wakes (same contract as the
/// fetch e2e tests' `bun_core::output::init_test()`).
fn init_output() {
    bun_core::output::init_test();
}

#[test]
fn test_bundle_single_ts_entry() {
    init_output();
    std::env::set_current_dir(std::env::temp_dir()).ok();
    let dir = temp_dir("single_ts");
    let entry = write(&dir, "entry.ts", "const x: number = 41;\nexport const answer: number = x + 1;\n");

    let cfg = base_config(vec![entry]);
    let result = run(&cfg);

    assert!(
        result.success,
        "build should succeed, logs: {:?}",
        result.logs
    );
    assert_eq!(result.outputs.len(), 1, "single entry → one output");
    let out = &result.outputs[0];
    assert!(out.path.ends_with(".js"), "output path should be .js, got {}", out.path);
    assert_eq!(out.kind, "entry-point");
    let text = String::from_utf8_lossy(&out.bytes).into_owned();
    assert!(
        text.contains("41"),
        "bundled output should contain the TS body, got: {}",
        text
    );
    assert!(
        !text.contains(": number"),
        "TS type annotations must be stripped, got: {}",
        text
    );
}

#[test]
fn test_bundle_missing_entry_resolves_unsuccessful_with_logs() {
    init_output();
    std::env::set_current_dir(std::env::temp_dir()).ok();
    let cfg = base_config(vec!["/nonexistent/does-not-exist-xyz.ts".into()]);
    let result = run(&cfg);
    assert!(!result.success, "missing entry must not report success");
    assert!(
        !result.logs.is_empty(),
        "failure face must carry logs, got: {:?}",
        result.logs
    );
    assert!(
        result.logs.iter().any(|l| l.level == "error"),
        "at least one error-level log expected"
    );
}

#[test]
fn test_bundle_external_passthrough() {
    init_output();
    std::env::set_current_dir(std::env::temp_dir()).ok();
    let dir = temp_dir("external");
    let entry = write(&dir, "entry.ts", "import leftpad from \"left-pad\";\nexport const v = leftpad(\"a\", 3);\n");

    let mut cfg = base_config(vec![entry]);
    cfg.external = vec!["left-pad".into()];
    let result = run(&cfg);
    assert!(result.success, "logs: {:?}", result.logs);
    let text = String::from_utf8_lossy(&result.outputs[0].bytes).into_owned();
    assert!(
        text.contains("left-pad"),
        "external module specifier must pass through, got: {}",
        text
    );
}

#[test]
fn test_bundle_minify_differs_and_shrinks() {
    init_output();
    std::env::set_current_dir(std::env::temp_dir()).ok();
    let dir = temp_dir("minify");
    let entry = write(
        &dir,
        "entry.ts",
        "function add(a: number, b: number): number {\n  return a + b;\n}\nexport const r = add(1, 2);\n",
    );

    let plain = run(&base_config(vec![entry.clone()]));
    assert!(plain.success, "logs: {:?}", plain.logs);
    let minified_cfg = {
        let mut c = base_config(vec![entry]);
        c.minify = NativeMinify::all();
        c
    };
    let minified = run(&minified_cfg);
    assert!(minified.success, "logs: {:?}", minified.logs);

    let plain_text = String::from_utf8_lossy(&plain.outputs[0].bytes).into_owned();
    let min_text = String::from_utf8_lossy(&minified.outputs[0].bytes).into_owned();
    assert_ne!(plain_text, min_text, "minify must change the output bytes");
    assert!(
        min_text.len() < plain_text.len(),
        "minified output should be smaller: {} vs {}",
        min_text.len(),
        plain_text.len()
    );
}

#[test]
fn test_bundle_multi_entry() {
    init_output();
    std::env::set_current_dir(std::env::temp_dir()).ok();
    let dir = temp_dir("multi");
    let a = write(&dir, "alpha.ts", "export const a = 1;\n");
    let b = write(&dir, "beta.ts", "export const b = 2;\n");

    let result = run(&base_config(vec![a, b]));
    assert!(result.success, "logs: {:?}", result.logs);
    assert_eq!(result.outputs.len(), 2, "two entries → two outputs");
}

#[test]
fn test_bundle_tsx_classic_jsx() {
    init_output();
    std::env::set_current_dir(std::env::temp_dir()).ok();
    let dir = temp_dir("tsx_classic");
    let entry = write(
        &dir,
        "app.tsx",
        "export const el = <div className=\"greet\">hi</div>;\n",
    );

    let mut cfg = base_config(vec![entry]);
    cfg.jsx_runtime = Some("classic".into());
    let result = run(&cfg);
    assert!(result.success, "logs: {:?}", result.logs);
    let text = String::from_utf8_lossy(&result.outputs[0].bytes).into_owned();
    assert!(
        text.contains("greet") && text.contains("React.createElement"),
        "JSX must lower to React.createElement, got: {}",
        text
    );
}

#[test]
fn test_bundle_outdir_writes_disk() {
    init_output();
    std::env::set_current_dir(std::env::temp_dir()).ok();
    let dir = temp_dir("outdir_src");
    let outdir = dir.join("dist");
    let entry = write(&dir, "entry.ts", "export const marker = \"outdir-bytes\";\n");

    let mut cfg = base_config(vec![entry]);
    cfg.outdir = Some(outdir.to_string_lossy().into_owned());
    let result = run(&cfg);
    assert!(result.success, "logs: {:?}", result.logs);

    let out = &result.outputs[0];
    let disk = outdir.join(&out.path);
    let disk_bytes = std::fs::read(&disk).unwrap_or_else(|e| {
        panic!("outdir artifact {} must exist on disk: {}", disk.display(), e)
    });
    assert_eq!(
        disk_bytes, out.bytes,
        "disk bytes must equal the in-memory artifact bytes"
    );
}
