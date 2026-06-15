// @trace TEST-E2E-CLI [req:REQ-CLI-001,REQ-CLI-002,REQ-ENG-006] [level:system]
// @trace REQ-CLI-001 [level:system]
// @trace REQ-CLI-002 [level:system]
// @trace REQ-ENG-006 [level:system]
//
// # TASK-12 E2E — bao CLI 端到端(std::process::Command 子进程)
//
// **核心断言**: `bao` 二进制可被 `std::process::Command` 驱动,完整执行
// 用户脚本。`bao run script.js` 启动 JsContext + 注入 Node API + 执行脚本 +
// 退出码反映执行结果。
//
// 测试维度:
//   1. **bao --help**: CLI 可执行,clap 注册成功
//   2. **bao run --eval "console.log"**: 一行脚本走完 JsContext 生命周期
//   3. **bao run script.js**: 文件脚本端到端(读文件 → 执行 → 退出)
//   4. **Bun API 可用**: 脚本内 typeof Bun === 'object'
//   5. **Node API 可用**: 脚本内 typeof process === 'object'
//   6. **退出码传播**: 脚本 process.exit(N) → bao 进程退出码 N
//   7. **stdout 捕获**: console.log → bao stdout
//
// **运行约束**: 测试需要预先 `cargo build` 产出 ./target/debug/bao 二进制。
// 缺失时 skip 而非 fail(避免 CI 在未 build 时直接红)。

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

const BAO_BIN: &str = "target/debug/bao";

// ─── 辅助 — 定位 bao 二进制 ──────────────────────────────────────────────────

fn bao_path() -> Option<PathBuf> {
    // 测试 cwd 通常是 crate 根目录(bao_browser/),向上一级到 workspace 根
    let mut here = std::env::current_dir().ok()?;
    for _ in 0..5 {
        let candidate = here.join(BAO_BIN);
        if candidate.is_file() {
            return Some(candidate);
        }
        if !here.pop() {
            break;
        }
    }
    // 兜底:直接信相对路径
    let direct = PathBuf::from(BAO_BIN);
    if direct.is_file() {
        Some(direct)
    } else {
        None
    }
}

fn run_bao(args: &[&str], stdin: Option<&str>) -> std::io::Result<std::process::Output> {
    let bao = bao_path().expect("bao binary not found — run `cargo build` first");
    let mut cmd = Command::new(bao);
    cmd.args(args);
    if stdin.is_some() {
        cmd.stdin(std::process::Stdio::piped());
    }
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn()?;
    if let Some(input) = stdin {
        use std::io::Write;
        let mut child_stdin = child.stdin.take().expect("stdin pipe");
        child_stdin.write_all(input.as_bytes())?;
        drop(child_stdin); // 关闭 stdin,触发 EOF
    }
    child.wait_with_output()
}

// ─── 主测试 ────────────────────────────────────────────────────────────────

#[test]
// @trace REQ-CLI-001 [level:e2e]
fn bao_cli_e2e_full_lifecycle() {
    let bao = match bao_path() {
        Some(p) => p,
        None => {
            eprintln!("SKIP: bao binary not found at ./{} — run `cargo build` first", BAO_BIN);
            return;
        }
    };
    eprintln!("using bao binary: {}", bao.display());

    let mut passed = 0u32;
    let mut failed = 0u32;

    // ── §1 bao --help — CLI 可执行 ──────────────────────────────────────
    //
    // clap 在 --help 时退出码 0,stdout 含 "bao" 或 "Bao"
    match run_bao(&["--help"], None) {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let combined = format!("{}\n{}", stdout, stderr);
            // clap --help 通常退出码 0(some versions exit 2 but still print help)
            if combined.to_lowercase().contains("usage")
                || combined.to_lowercase().contains("bao")
                || combined.contains("run")
                || combined.contains("browser")
            {
                eprintln!("PASS  §1::cli_help_responds");
                passed += 1;
            } else {
                eprintln!("FAIL  §1::cli_help_responds  (combined output empty or unexpected)");
                failed += 1;
            }
        }
        Err(e) => {
            eprintln!("FAIL  §1::cli_help_responds  (spawn failed: {})", e);
            failed += 1;
        }
    }

    // ── §2 bao run --eval "console.log('hello')" — 基础 eval ──────────────
    //
    // 这验证 JsContext 完整生命周期:创建 → eval console.log → drain stdout → 退出
    match run_bao(&["run", "--eval", "console.log('bao-e2e-marker')"], None) {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.contains("bao-e2e-marker") {
                eprintln!("PASS  §2::eval_console_log");
                passed += 1;
            } else {
                eprintln!(
                    "FAIL  §2::eval_console_log  (stdout='{}', exit={:?})",
                    stdout.trim(),
                    output.status.code()
                );
                failed += 1;
            }
        }
        Err(e) => {
            eprintln!("FAIL  §2::eval_console_log  (spawn failed: {})", e);
            failed += 1;
        }
    }

    // ── §3 bao run --eval "Bun.version" — Bun API 真可用 ──────────────────
    //
    // typeof Bun === 'object' 验证 JsContext 内 Bun 全局对象已注入
    match run_bao(&["run", "--eval", "console.log(typeof Bun)"], None) {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.trim().contains("object") {
                eprintln!("PASS  §3::bun_api_available");
                passed += 1;
            } else {
                eprintln!(
                    "FAIL  §3::bun_api_available  (typeof Bun = '{}', exit={:?})",
                    stdout.trim(),
                    output.status.code()
                );
                failed += 1;
            }
        }
        Err(e) => {
            eprintln!("FAIL  §3::bun_api_available  (spawn failed: {})", e);
            failed += 1;
        }
    }

    // ── §4 bao run --eval "typeof process" — Node API 真可用 ──────────────
    match run_bao(&["run", "--eval", "console.log(typeof process)"], None) {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.trim().contains("object") {
                eprintln!("PASS  §4::node_api_available");
                passed += 1;
            } else {
                eprintln!(
                    "FAIL  §4::node_api_available  (typeof process = '{}', exit={:?})",
                    stdout.trim(),
                    output.status.code()
                );
                failed += 1;
            }
        }
        Err(e) => {
            eprintln!("FAIL  §4::node_api_available  (spawn failed: {})", e);
            failed += 1;
        }
    }

    // ── §5 bao run script.js — 文件脚本端到端 ─────────────────────────────
    //
    // 临时写一个 .js 文件,bao run 它,验证文件读取 + 执行 + stdout。
    let temp_dir = std::env::temp_dir();
    let script_path = temp_dir.join("bao_e2e_test_script.js");
    std::fs::write(
        &script_path,
        r#"
            // 文件脚本 — 用 Node API (Buffer) + Bun API (Bun.version)
            const buf = Buffer.from('hello-from-file');
            console.log('file-script-runs');
            console.log(buf.toString());
            console.log(typeof Bun === 'object' ? 'bun-ok' : 'bun-missing');
        "#,
    )
    .expect("write temp script");
    let script_str = script_path.to_string_lossy().to_string();
    match run_bao(&["run", script_str.as_str()], None) {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.contains("file-script-runs")
                && stdout.contains("hello-from-file")
                && stdout.contains("bun-ok")
            {
                eprintln!("PASS  §5::run_file_script");
                passed += 1;
            } else {
                eprintln!(
                    "FAIL  §5::run_file_script  (stdout='{}', exit={:?})",
                    stdout.trim(),
                    output.status.code()
                );
                failed += 1;
            }
        }
        Err(e) => {
            eprintln!("FAIL  §5::run_file_script  (spawn failed: {})", e);
            failed += 1;
        }
    }

    // ── §6 退出码传播 — process.exit(N) ──────────────────────────────────
    //
    // process.exit(0) → bao 退出码 0;process.exit(42) → 退出码 42
    match run_bao(&["run", "--eval", "process.exit(0)"], None) {
        Ok(output) => {
            let code = output.status.code();
            if code == Some(0) {
                eprintln!("PASS  §6a::exit_code_zero");
                passed += 1;
            } else {
                eprintln!("FAIL  §6a::exit_code_zero  (got {:?})", code);
                failed += 1;
            }
        }
        Err(e) => {
            eprintln!("FAIL  §6a::exit_code_zero  (spawn failed: {})", e);
            failed += 1;
        }
    }
    match run_bao(&["run", "--eval", "process.exit(42)"], None) {
        Ok(output) => {
            let code = output.status.code();
            if code == Some(42) {
                eprintln!("PASS  §6b::exit_code_42");
                passed += 1;
            } else {
                eprintln!("FAIL  §6b::exit_code_42  (got {:?})", code);
                failed += 1;
            }
        }
        Err(e) => {
            eprintln!("FAIL  §6b::exit_code_42  (spawn failed: {})", e);
            failed += 1;
        }
    }

    // ── §7 stdout 捕获 — 多行 console.log ────────────────────────────────
    match run_bao(&["run", "--eval", "console.log('line1'); console.log('line2');"], None) {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.contains("line1") && stdout.contains("line2") {
                eprintln!("PASS  §7::multi_line_stdout");
                passed += 1;
            } else {
                eprintln!("FAIL  §7::multi_line_stdout  (stdout='{}')", stdout.trim());
                failed += 1;
            }
        }
        Err(e) => {
            eprintln!("FAIL  §7::multi_line_stdout  (spawn failed: {})", e);
            failed += 1;
        }
    }

    // ── 清理 ────────────────────────────────────────────────────────────
    let _ = std::fs::remove_file(&script_path);

    eprintln!(
        "=== bao CLI E2E ===\n--- {} passed, {} failed ---",
        passed, failed
    );

    // 至少 5/8 通过(允许 §1 help 格式差异等少数容忍)
    assert!(
        passed >= 5,
        "too few CLI E2E sub-assertions passed: {}/8",
        passed
    );
    assert_eq!(
        failed, 0,
        "{} CLI E2E sub-assertions failed — see stderr above",
        failed
    );
}

// ─── 网络 E2E — bao browser 子命令(需要 servo Opts 单例 + 网络) ──────────────
//
// bao browser --cdp-port 启动 servo + CDP server,长时间运行。
// 此测试默认 #[ignore],因为它会启动一个长时间运行的进程。
// 启用方式:BAO_TEST_NETWORK=1 cargo test bao_cli_browser_subcommand -- --ignored

#[test]
#[ignore = "long-running server — set BAO_TEST_NETWORK=1 to enable"]
// @trace REQ-CLI-002 [level:e2e]
fn bao_cli_browser_subcommand_starts() {
    if std::env::var("BAO_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!("skipping bao browser subcommand E2E — set BAO_TEST_NETWORK=1 to enable");
        return;
    }
    let bao = bao_path().expect("bao binary not found");

    // Act: 启动 bao browser,绑一个空闲端口
    let port = pick_free_port();
    let mut cmd = Command::new(&bao);
    cmd.args(["browser", "--headless", "--cdp-port", &port.to_string()])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn().expect("spawn bao browser");

    // 给 servo + CDP server 最多 5 秒初始化
    std::thread::sleep(Duration::from_secs(5));

    // Assert: 端口可连(CDP server 已起)
    let connected = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).is_ok();

    // 清理:杀子进程
    let _ = child.kill();
    let _ = child.wait();

    assert!(connected, "bao browser --cdp-port {} must listen", port);
}

fn pick_free_port() -> u16 {
    // OS-assigned free port
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(9922)
}
