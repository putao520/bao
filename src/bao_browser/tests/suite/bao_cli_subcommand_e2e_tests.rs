// @trace REQ-CLI-001 [level:e2e]
// @trace REQ-ENG-006 [level:e2e]
//
// # CLI 子命令 E2E — doctor / test / build / install / external(std::process::Command)
//
// `bao run` / `bao browser` 的二进制面覆盖在 bao_cli_e2e_tests.rs /
// bao_cli_timeout_e2e_tests.rs / cli_browser_tests.rs;本文件只补其余五个
// 子命令面的零覆盖缺口。断言全部对照 cli.rs / doctor.rs / install.rs 的
// **真实行为**(实测退出码与输出位置),不猜契约:
//
//   1. **doctor**: 信息性巡检,文档注释明示 "never exits non-zero" —
//      恒 rc=0,stdout 恒含表头 + 逐项检查行(与探针结果无关)。
//   2. **test**: run_test_file 发现式跑 test|tests|__tests__ 目录(cli.rs:547);
//      `.ts` 文件走模块路(bun:test import 可解析),纯 `.js` 无裸 `test`
//      全局(实测 "test is not defined")。空目录 → "no test files found" rc=1;
//      缺文件 → "FAIL [...]" rc=1;`test -e` 走 run_registered_tests。
//   3. **build**: 产物落在 `<outdir|dist>/<entrypoint 原名>`(cli.rs:444-457);
//      缺入口 → bundler 读文件错 rc=1。
//   4. **install**: bao 层 clap 先拦截 — `--help` rc=0 印用法;未知 flag
//      rc=2 印 "unexpected argument"。两者都不触达 bun_install 委托,
//      网络路径(真装包)按合同排除。
//   5. **external**: `External(Vec<String>)` 目前对所有裸参数统一拒绝
//      ("bao: unknown command ..." rc=1,cli.rs:182-185)— 不做脚本直跑;
//      断言该真实行为(SKIPPED 语义见测试内注释)。
//
// **运行约束**: 需要预 build 的 bao 二进制(bao_path() 探测);缺失时
// graceful skip,与 bao_cli_e2e_tests.rs 同约定。

use std::path::{Path, PathBuf};
use std::process::Command;

const BAO_BIN: &str = "target/debug/bao";

// ─── 辅助 — 定位 bao 二进制(镜像 bao_cli_e2e_tests.rs 的探测序) ─────────────

fn bao_path() -> Option<PathBuf> {
    if let Ok(override_path) = std::env::var("BAO_BIN") {
        let candidate = PathBuf::from(override_path);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    if let Ok(p) = std::env::var("BAO_TEST_BAO_BIN") {
        let candidate = PathBuf::from(p);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    // Profile-agnostic probe: this suite exe sits at $TARGET/<profile>/deps/ —
    // the bao binary sits at $TARGET/<profile>/bao for EVERY cargo profile.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(profile_dir) = exe.parent().and_then(|d| d.parent()) {
            for name in ["bao", "bao.exe"] {
                let candidate = profile_dir.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    if let Ok(target_dir) = std::env::var("CARGO_TARGET_DIR") {
        for profile in ["test-ci", "debug", "release"] {
            for name in ["bao", "bao.exe"] {
                let candidate = PathBuf::from(&target_dir).join(profile).join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    let mut here = std::env::current_dir().ok()?;
    for _ in 0..5 {
        for profile in ["test-ci", "debug", "release"] {
            for name in ["bao", "bao.exe"] {
                let candidate = here.join("target").join(profile).join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
        if !here.pop() {
            break;
        }
    }
    None
}

/// Spawn bao with an explicit cwd (install/test discovery/build outdir 都是
/// cwd 相对行为,必须钉在一次性临时目录里,避免污染仓库工作树)。
fn run_bao_in(cwd: &Path, args: &[&str]) -> std::io::Result<std::process::Output> {
    let bao = bao_path().expect("bao binary not found — run `cargo build` first");
    Command::new(bao)
        .args(args)
        .current_dir(cwd)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
}

/// 每测试一次性临时目录(进程 id + 原子计数保证并发 nextest 进程不撞车)。
fn fresh_temp_dir(label: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "bao_cli_sub_e2e_{}_{}_{}",
        label,
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

// ─── 1. bao doctor — 信息性巡检,恒 rc=0 ─────────────────────────────────────

#[test]
// @trace REQ-CLI-001 [level:e2e]
fn bao_cli_subcommand_doctor_never_exits_nonzero() {
    let bao = match bao_path() {
        Some(p) => p,
        None => {
            eprintln!(
                "SKIP: bao binary not found at ./{} — run `cargo build` first",
                BAO_BIN
            );
            return;
        }
    };
    eprintln!("using bao binary: {}", bao.display());

    let temp = fresh_temp_dir("doctor");
    // doctor 的 target/ 产物探针与 CDP 探测都无 cwd 依赖,钉临时目录仅为卫生。
    let output = run_bao_in(&temp, &["doctor"]).expect("spawn bao doctor");
    let _ = std::fs::remove_dir_all(&temp);

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // 文档注释契约: "Informational only — never exits non-zero"(doctor.rs:82-84
    // 即使有 ✗ 项也返回 Ok)。断言恒成立,与机器装了什么无关。
    assert_eq!(
        output.status.code(),
        Some(0),
        "bao doctor must always exit 0 (stdout={:?}, stderr={:?})",
        stdout.trim(),
        stderr.trim()
    );

    // 恒打印的稳定标记:版本表头(doctor.rs:29)+ 恒在检查列表里的行标。
    // 每一项检查无论 ✓/✗ 都打印 label 行(doctor.rs:61-69),✓/✗ 本身依赖环境,
    // 不作为断言对象。
    assert!(
        stdout.contains("Bao doctor v"),
        "doctor must print its version header, got: {:?}",
        stdout.trim()
    );
    for label in ["Rust (rustc)", "Cargo", "C/C++ compiler", "CDP :9222"] {
        assert!(
            stdout.contains(label),
            "doctor output must contain the always-printed check label {:?}, got: {:?}",
            label,
            stdout.trim()
        );
    }
    assert!(
        stdout.contains("✓") || stdout.contains("✗"),
        "doctor must print the check table (✓/✗ marks), got: {:?}",
        stdout.trim()
    );
}

// ─── 2. bao test — 发现式 runner 的退出码契约 ────────────────────────────────

#[test]
// @trace REQ-ENG-006 [level:e2e]
fn bao_cli_subcommand_test_runner_exit_codes() {
    if bao_path().is_none() {
        eprintln!(
            "SKIP: bao binary not found at ./{} — run `cargo build` first",
            BAO_BIN
        );
        return;
    }

    let mut passed = 0u32;
    let mut failed = 0u32;

    // ── §a 发现式:通过文件 → rc=0 + ✓ 行 + "Summary: 1 passed, 0 failed" ──
    //
    // 实测:纯 `.js` 脚本没有裸 `test` 全局("test is not defined"),
    // runner 文档注释明示 test 文件应使用 ESM/TS(runtime.rs run_test_file
    // doc:"test files should use ESM/TS syntax"),`.ts` 走模块路且 bun:test
    // import 可解析 — 这就是实现支持的规范形态。
    let temp = fresh_temp_dir("test_pass");
    std::fs::create_dir_all(temp.join("test")).unwrap();
    std::fs::write(
        temp.join("test/ok.test.ts"),
        "import { test, expect } from \"bun:test\";\n\
         test(\"bao-sub-e2e-pass\", () => { expect(2 + 2).toBe(4); });\n",
    )
    .unwrap();
    match run_bao_in(&temp, &["test"]) {
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if output.status.code() == Some(0)
                && stderr.contains("✓ bao-sub-e2e-pass")
                && stderr.contains("Summary: 1 passed, 0 failed")
            {
                eprintln!("PASS  §a::discovery_passing_file_exits_zero");
                passed += 1;
            } else {
                eprintln!(
                    "FAIL  §a::discovery_passing_file_exits_zero  (rc={:?}, stderr={})",
                    output.status.code(),
                    stderr.trim()
                );
                failed += 1;
            }
        }
        Err(e) => {
            eprintln!("FAIL  §a::discovery_passing_file_exits_zero  (spawn failed: {})", e);
            failed += 1;
        }
    }
    let _ = std::fs::remove_dir_all(&temp);

    // ── §b 发现式:失败断言 → rc=1 + ✗ 行 + 计数进 Summary ────────────────
    let temp = fresh_temp_dir("test_fail");
    std::fs::create_dir_all(temp.join("test")).unwrap();
    std::fs::write(
        temp.join("test/bad.test.ts"),
        "import { test, expect } from \"bun:test\";\n\
         test(\"bao-sub-e2e-fail\", () => { expect(1).toBe(2); });\n",
    )
    .unwrap();
    match run_bao_in(&temp, &["test"]) {
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if output.status.code() == Some(1)
                && stderr.contains("✗ bao-sub-e2e-fail")
                && stderr.contains("Summary: 0 passed, 1 failed")
            {
                eprintln!("PASS  §b::discovery_failing_file_exits_one");
                passed += 1;
            } else {
                eprintln!(
                    "FAIL  §b::discovery_failing_file_exits_one  (rc={:?}, stderr={})",
                    output.status.code(),
                    stderr.trim()
                );
                failed += 1;
            }
        }
        Err(e) => {
            eprintln!("FAIL  §b::discovery_failing_file_exits_one  (spawn failed: {})", e);
            failed += 1;
        }
    }
    let _ = std::fs::remove_dir_all(&temp);

    // ── §c 空目录(无 test|tests|__tests__)→ rc=1 + 固定提示 ─────────────
    // cli.rs:584-587: eprintln "bao test: no test files found (looked in ...)"
    // + return Err(1)。
    let temp = fresh_temp_dir("test_empty");
    match run_bao_in(&temp, &["test"]) {
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if output.status.code() == Some(1)
                && stderr
                    .contains("bao test: no test files found (looked in test/, tests/, __tests__/)")
            {
                eprintln!("PASS  §c::no_test_files_found_exits_one");
                passed += 1;
            } else {
                eprintln!(
                    "FAIL  §c::no_test_files_found_exits_one  (rc={:?}, stderr={})",
                    output.status.code(),
                    stderr.trim()
                );
                failed += 1;
            }
        }
        Err(e) => {
            eprintln!("FAIL  §c::no_test_files_found_exits_one  (spawn failed: {})", e);
            failed += 1;
        }
    }
    let _ = std::fs::remove_dir_all(&temp);

    // ── §d 显式文件不存在 → rc=1 + "FAIL [<path>]" + Summary 计失败 ───────
    // cli.rs:596-609: run_test_file 的读文件错误 → "FAIL [file]: ..." 行,
    // total_failed+=1 → 汇总 Err(1)。
    let temp = fresh_temp_dir("test_missing");
    let missing = temp.join("nope_missing.test.ts");
    let missing_str = missing.to_string_lossy().into_owned();
    let missing_ref = missing_str.clone();
    match run_bao_in(&temp, &["test", missing_ref.as_str()]) {
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if output.status.code() == Some(1)
                && stderr.contains(&format!("FAIL [{}]", missing_str))
                && stderr.contains("Error reading")
                && stderr.contains("Summary: 0 passed, 1 failed")
            {
                eprintln!("PASS  §d::missing_file_reports_fail_and_exits_one");
                passed += 1;
            } else {
                eprintln!(
                    "FAIL  §d::missing_file_reports_fail_and_exits_one  (rc={:?}, stderr={})",
                    output.status.code(),
                    stderr.trim()
                );
                failed += 1;
            }
        }
        Err(e) => {
            eprintln!(
                "FAIL  §d::missing_file_reports_fail_and_exits_one  (spawn failed: {})",
                e
            );
            failed += 1;
        }
    }
    let _ = std::fs::remove_dir_all(&temp);

    // ── §e `bao test -e '<inline>'` — eval + run_registered_tests ────────
    // cli.rs:528-545:`-e` 形态受支持,eval 后驱动已注册 suite;注册了
    // node:test 用例并全部通过 → rc=0(实测 `require("node:test")` 的 CJS
    // 形态与 collector 打通;裸全局 `test()` 不可用,见 §a 注)。
    // 注意:-e 分支只走 render_report(✓ 行 + "-> N passed" 行),不打印
    // 文件/发现式分支才有的 "# Summary:" 汇总行(cli.rs:539 vs 588/613)。
    let temp = fresh_temp_dir("test_eval");
    match run_bao_in(
        &temp,
        &[
            "test",
            "-e",
            "const { test } = require(\"node:test\"); test(\"bao-sub-e2e-inline\", () => {});",
        ],
    ) {
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if output.status.code() == Some(0)
                && stderr.contains("✓ bao-sub-e2e-inline")
                && stderr.contains("-> 1 passed, 0 failed")
                && !stderr.contains("# Summary:")
            {
                eprintln!("PASS  §e::inline_eval_registered_tests_exits_zero");
                passed += 1;
            } else {
                eprintln!(
                    "FAIL  §e::inline_eval_registered_tests_exits_zero  (rc={:?}, stderr={})",
                    output.status.code(),
                    stderr.trim()
                );
                failed += 1;
            }
        }
        Err(e) => {
            eprintln!(
                "FAIL  §e::inline_eval_registered_tests_exits_zero  (spawn failed: {})",
                e
            );
            failed += 1;
        }
    }
    let _ = std::fs::remove_dir_all(&temp);

    eprintln!(
        "=== bao test runner E2E ===\n--- {} passed, {} failed ---",
        passed, failed
    );
    assert_eq!(failed, 0, "{} test-runner sub-assertions failed — see stderr above", failed);
}

// ─── 3. bao build — 产物落点与缺入口错误 ─────────────────────────────────────

#[test]
// @trace REQ-CLI-001 [level:e2e]
fn bao_cli_subcommand_build_artifact_and_missing_entry() {
    if bao_path().is_none() {
        eprintln!(
            "SKIP: bao binary not found at ./{} — run `cargo build` first",
            BAO_BIN
        );
        return;
    }

    let mut passed = 0u32;
    let mut failed = 0u32;

    // ── §a 显式 --outdir → rc=0 + 产物在 <outdir>/<entrypoint 原名> ──────
    // cli.rs:453-457:输出名取 entrypoint 的 file_name 原样,目录缺省 "dist"
    // 或 --outdir 给定;成功行打到 stderr(477-484)。
    let temp = fresh_temp_dir("build_ok");
    std::fs::write(temp.join("entry.js"), "export const baoAnswer = 42;\n").unwrap();
    let outdir = temp.join("bundle-out");
    let outdir_str = outdir.to_string_lossy().into_owned();
    match run_bao_in(&temp, &["build", "entry.js", "--outdir", outdir_str.as_str()]) {
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let artifact = outdir.join("entry.js");
            let artifact_body = std::fs::read_to_string(&artifact).unwrap_or_default();
            if output.status.code() == Some(0)
                && stderr.contains("bundled")
                && stderr.contains("entry.js")
                && artifact.is_file()
                && artifact_body.contains("baoAnswer")
            {
                eprintln!("PASS  §a::outdir_artifact_written");
                passed += 1;
            } else {
                eprintln!(
                    "FAIL  §a::outdir_artifact_written  (rc={:?}, stderr={}, artifact_exists={})",
                    output.status.code(),
                    stderr.trim(),
                    artifact.is_file()
                );
                failed += 1;
            }
        }
        Err(e) => {
            eprintln!("FAIL  §a::outdir_artifact_written  (spawn failed: {})", e);
            failed += 1;
        }
    }
    let _ = std::fs::remove_dir_all(&temp);

    // ── §b 缺省 outdir = "dist"(cwd 相对)→ dist/<原名> 落盘 ─────────────
    let temp = fresh_temp_dir("build_default");
    std::fs::write(temp.join("entry.js"), "const x = 1;\nexport default x;\n").unwrap();
    match run_bao_in(&temp, &["build", "entry.js"]) {
        Ok(output) => {
            let default_artifact = temp.join("dist").join("entry.js");
            if output.status.code() == Some(0) && default_artifact.is_file() {
                eprintln!("PASS  §b::default_dist_outdir");
                passed += 1;
            } else {
                eprintln!(
                    "FAIL  §b::default_dist_outdir  (rc={:?}, dist/entry.js exists={})",
                    output.status.code(),
                    default_artifact.is_file()
                );
                failed += 1;
            }
        }
        Err(e) => {
            eprintln!("FAIL  §b::default_dist_outdir  (spawn failed: {})", e);
            failed += 1;
        }
    }
    let _ = std::fs::remove_dir_all(&temp);

    // ── §c 入口不存在 → bundler 读文件失败 → rc=1 + "Error reading" ──────
    // cli.rs:459-462:bao_bundler::build Err → eprintln "Error: {}" + Err(1)
    // (Rust 侧错误,不是 JS 异常面)。
    let temp = fresh_temp_dir("build_missing");
    let missing = temp.join("no_such_entry.js");
    let missing_str = missing.to_string_lossy().into_owned();
    let missing_ref = missing_str.clone();
    match run_bao_in(&temp, &["build", missing_ref.as_str()]) {
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if output.status.code() == Some(1)
                && stderr.contains("Error reading")
                && stderr.contains(&missing_str)
            {
                eprintln!("PASS  §c::missing_entry_exits_one");
                passed += 1;
            } else {
                eprintln!(
                    "FAIL  §c::missing_entry_exits_one  (rc={:?}, stderr={})",
                    output.status.code(),
                    stderr.trim()
                );
                failed += 1;
            }
        }
        Err(e) => {
            eprintln!("FAIL  §c::missing_entry_exits_one  (spawn failed: {})", e);
            failed += 1;
        }
    }
    let _ = std::fs::remove_dir_all(&temp);

    eprintln!(
        "=== bao build E2E ===\n--- {} passed, {} failed ---",
        passed, failed
    );
    assert_eq!(failed, 0, "{} build sub-assertions failed — see stderr above", failed);
}

// ─── 4. bao install — 参数面(clap 先拦截,零网络) ───────────────────────────
//
// 网络排除说明:真装包路径(bun_install 的 resolve/fetch)需要远端 registry,
// 且 bao 层 clap 会先于 run_install() 校验全部参数 — 无法经由 CLI 到达
// bun_install::CommandLineArguments::parse 的委派面(它读进程 argv,而 clap
// 已把未知 flag 拒在门外)。因此本面只覆盖参数路由的真实行为:
// `--help` 与未知 flag,两者均零网络、零写盘(实测临时目录无产物)。

#[test]
// @trace REQ-CLI-001 [level:e2e]
fn bao_cli_subcommand_install_argument_surface() {
    if bao_path().is_none() {
        eprintln!(
            "SKIP: bao binary not found at ./{} — run `cargo build` first",
            BAO_BIN
        );
        return;
    }

    let mut passed = 0u32;
    let mut failed = 0u32;

    // ── §a `bao install --help` → rc=0 + clap 子命令用法 ─────────────────
    let temp = fresh_temp_dir("install_help");
    match run_bao_in(&temp, &["install", "--help"]) {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if output.status.code() == Some(0)
                && stdout.contains("Usage: bao") // argv[0] basename: "bao" posix / "bao.exe" windows
                && stdout.contains("install")
                && stdout.contains("Install dependencies")
            {
                eprintln!("PASS  §a::install_help_exits_zero");
                passed += 1;
            } else {
                eprintln!(
                    "FAIL  §a::install_help_exits_zero  (rc={:?}, stdout={})",
                    output.status.code(),
                    stdout.trim()
                );
                failed += 1;
            }
        }
        Err(e) => {
            eprintln!("FAIL  §a::install_help_exits_zero  (spawn failed: {})", e);
            failed += 1;
        }
    }
    let _ = std::fs::remove_dir_all(&temp);

    // ── §b 未知 flag → clap usage error rc=2 + "unexpected argument" ─────
    let temp = fresh_temp_dir("install_badflag");
    match run_bao_in(&temp, &["install", "--bao-sub-e2e-not-a-real-flag"]) {
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if output.status.code() == Some(2) && stderr.contains("unexpected argument") {
                eprintln!("PASS  §b::install_unknown_flag_usage_error");
                passed += 1;
            } else {
                eprintln!(
                    "FAIL  §b::install_unknown_flag_usage_error  (rc={:?}, stderr={})",
                    output.status.code(),
                    stderr.trim()
                );
                failed += 1;
            }
        }
        Err(e) => {
            eprintln!("FAIL  §b::install_unknown_flag_usage_error  (spawn failed: {})", e);
            failed += 1;
        }
    }
    let _ = std::fs::remove_dir_all(&temp);

    eprintln!(
        "=== bao install E2E ===\n--- {} passed, {} failed ---",
        passed, failed
    );
    assert_eq!(failed, 0, "{} install sub-assertions failed — see stderr above", failed);
}

// ─── 5. bao <file.js> — External 子命令当前的真实行为 ────────────────────────
//
// SKIPPED(behavior-mismatch): 任务期望 `bao /path/to/script.js` 直跑脚本
// (exit 0 + marker)。实测与 cli.rs:182-185 一致:External 变体对**所有**
// 裸参数统一打印 "bao: unknown command '<first arg>'" 并返回 Err(1) —
// 不存在脚本直跑通路(脚本入口是 `bao run <file>` 或顶层 `-e`)。
// 本测试钉住当前真实契约:脚本路径参数被拒绝且 rc=1,防止未来引入直跑时
// 无声漂移。

#[test]
// @trace REQ-CLI-001 [level:e2e]
fn bao_cli_subcommand_external_script_path_is_rejected() {
    if bao_path().is_none() {
        eprintln!(
            "SKIP: bao binary not found at ./{} — run `cargo build` first",
            BAO_BIN
        );
        return;
    }

    let mut passed = 0u32;
    let mut failed = 0u32;

    // ── §a 存在的脚本文件 → rc=1 + "bao: unknown command '<path>'" ───────
    let temp = fresh_temp_dir("external_script");
    std::fs::write(temp.join("ext.js"), "console.log('bao-external-marker');\n").unwrap();
    let script = temp.join("ext.js");
    let script_str = script.to_string_lossy().into_owned();
    let script_ref = script_str.clone();
    match run_bao_in(&temp, &[script_ref.as_str()]) {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            if output.status.code() == Some(1)
                && stderr.contains(&format!("bao: unknown command '{}'", script_str))
                && !stdout.contains("bao-external-marker")
            {
                eprintln!("PASS  §a::existing_script_rejected_rc1");
                passed += 1;
            } else {
                eprintln!(
                    "FAIL  §a::existing_script_rejected_rc1  (rc={:?}, stdout={}, stderr={})",
                    output.status.code(),
                    stdout.trim(),
                    stderr.trim()
                );
                failed += 1;
            }
        }
        Err(e) => {
            eprintln!("FAIL  §a::existing_script_rejected_rc1  (spawn failed: {})", e);
            failed += 1;
        }
    }
    let _ = std::fs::remove_dir_all(&temp);

    // ── §b 不存在的路径 → 同一拒绝通路 rc=1(与文件存在与否无关) ─────────
    let temp = fresh_temp_dir("external_missing");
    match run_bao_in(&temp, &["bao_sub_e2e_missing.js"]) {
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if output.status.code() == Some(1)
                && stderr.contains("bao: unknown command 'bao_sub_e2e_missing.js'")
            {
                eprintln!("PASS  §b::missing_path_same_rejection");
                passed += 1;
            } else {
                eprintln!(
                    "FAIL  §b::missing_path_same_rejection  (rc={:?}, stderr={})",
                    output.status.code(),
                    stderr.trim()
                );
                failed += 1;
            }
        }
        Err(e) => {
            eprintln!("FAIL  §b::missing_path_same_rejection  (spawn failed: {})", e);
            failed += 1;
        }
    }
    let _ = std::fs::remove_dir_all(&temp);

    eprintln!(
        "=== bao external E2E ===\n--- {} passed, {} failed ---",
        passed, failed
    );
    assert_eq!(failed, 0, "{} external sub-assertions failed — see stderr above", failed);
}
