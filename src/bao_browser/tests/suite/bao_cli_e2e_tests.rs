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
//   8. **双向流归属**: console.log → stdout 且 console.error → stderr(不串流)
//
// **运行约束**: 测试需要预先 `cargo build` 产出 ./target/debug/bao 二进制。
// 缺失时 skip 而非 fail(避免 CI 在未 build 时直接红)。

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

#[path = "common/mod.rs"]
mod common;

// ─── 辅助 — 定位 bao 二进制(共享 BCE-20261018-STALEBIN canary) ─────────────

/// Resolve `bao` through the shared three-arm behavior canary
/// (`common::bao_bin_canary`, single source shared with the bao_cdp_client
/// suite). Missing binary or a stale/poisoned candidate is a fail-closed
/// panic: these tests must never run against an unverified binary.
fn bao_path() -> PathBuf {
    common::bao_bin_canary::find_bao_binary()
}

fn run_bao(args: &[&str], stdin: Option<&str>) -> std::io::Result<std::process::Output> {
    let bao = bao_path();
    let mut cmd = Command::new(bao);
    cmd.args(args);
    if stdin.is_some() {
        cmd.stdin(std::process::Stdio::piped());
    }
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    // e204 reaping discipline: process-group spawn + PDEATHSIG + reap-on-drop
    // (see common::GroupChild) — a panic mid-test must not orphan the child.
    let mut child = common::GroupChild::spawn(&mut cmd)?;
    if let Some(input) = stdin {
        use std::io::Write;
        let mut child_stdin = child.stdin().take().expect("stdin pipe");
        child_stdin.write_all(input.as_bytes())?;
        drop(child_stdin); // 关闭 stdin,触发 EOF
    }
    child.wait_with_output()
}

// ─── 主测试 ────────────────────────────────────────────────────────────────

#[test]
// @trace REQ-CLI-001 [level:e2e]
fn bao_cli_e2e_full_lifecycle() {
    let bao = bao_path();
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
    match run_bao(
        &[
            "run",
            "--eval",
            "console.log('line1'); console.log('line2');",
        ],
        None,
    ) {
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

    // ── §8 双向流归属 — console.log→stdout 且 console.error→stderr ────────
    //
    // Node 流语义:log 走 stdout,error 走 stderr。子进程两路管道分别捕获,
    // 断言 marker 各归各的流、不串流、不丢失(缓冲未 flush 时整段丢失)。
    match run_bao(
        &[
            "run",
            "--eval",
            "console.log('stdout-marker'); console.error('stderr-marker');",
        ],
        None,
    ) {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stdout.contains("stdout-marker")
                && stderr.contains("stderr-marker")
                && !stdout.contains("stderr-marker")
                && !stderr.contains("stdout-marker")
            {
                eprintln!("PASS  §8::console_stream_routing");
                passed += 1;
            } else {
                eprintln!(
                    "FAIL  §8::console_stream_routing  (stdout='{}', stderr='{}', exit={:?})",
                    stdout.trim(),
                    stderr.trim(),
                    output.status.code()
                );
                failed += 1;
            }
        }
        Err(e) => {
            eprintln!("FAIL  §8::console_stream_routing  (spawn failed: {})", e);
            failed += 1;
        }
    }

    // ── 清理 ────────────────────────────────────────────────────────────
    let _ = std::fs::remove_file(&script_path);

    eprintln!(
        "=== bao CLI E2E ===\n--- {} passed, {} failed ---",
        passed, failed
    );

    // 至少 5/9 通过(允许 §1 help 格式差异等少数容忍)
    assert!(
        passed >= 5,
        "too few CLI E2E sub-assertions passed: {}/9",
        passed
    );
    assert_eq!(
        failed, 0,
        "{} CLI E2E sub-assertions failed — see stderr above",
        failed
    );
}

// ─── E2E — bao browser 子命令(需要 servo Opts 单例;零外部网络:只连 127.0.0.1)──
//
// bao browser --cdp-port 启动 servo + CDP server。servo WebRender GL-based,
// 无 DISPLAY 环境静态 #[ignore](原因串携带 Xvfb 配方);
// `--run-ignored=all` 在已供环境执行。

#[ignore = "需要 DISPLAY: servo WebRender GL-based(headless 亦需),scripts/local-smoke.sh:97 Xvfb :99 配方; 真跑: pgrep -x Xvfb || Xvfb :99 -screen 0 1280x720x24 -ac -nolisten tcp & 后 DISPLAY=:99 cargo nextest run --run-ignored=all -p bao-browser"]
#[test]
// @trace REQ-CLI-002 [level:e2e]
fn bao_cli_browser_subcommand_starts() {
    let bao = bao_path();

    // Act: 启动 bao browser,绑一个空闲端口。E26 ledger class A(probe-release
    // rebind 窗口):probe drop 与子进程 CDP bind 之间,并行套件进程可抢占该
    // 端口——每个 attempt 用新鲜 probe 端口重试;子进程已退出(bind 失败)或
    // 端口连不通都判输该 attempt。probe 失败本身 loud panic(原
    // `.unwrap_or(9922)` 固定端口静默兜底已删)。
    let mut bound = None;
    for _ in 0..common::port_probe::PROBE_REBIND_RETRIES {
        let port = common::port_probe::probe_free_tcp_port();
        let mut cmd = Command::new(&bao);
        cmd.args(["browser", "--headless", "--cdp-port", &port.to_string()])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        // e204 reaping discipline: process-group spawn + PDEATHSIG + reap-on-drop
        // (see common::GroupChild) — 重试环同样不得泄漏引擎进程。
        let mut child = common::GroupChild::spawn(&mut cmd).expect("spawn bao browser");

        // 给 servo + CDP server 最多 5 秒初始化
        std::thread::sleep(Duration::from_secs(5));

        // Assert(候选): 端口可连且连的是本子进程——子进程若已退出,端口属于
        // 抢占者,连上也不算赢(赢了 rebind 竞争的判据 = connected + child 存活)。
        let connected = std::net::TcpStream::connect(format!("127.0.0.1:{}", port)).is_ok();
        let child_alive = child.try_wait().map_or(true, |status| status.is_none());

        // 清理:e204 组级收割——每个 attempt 都收割,重试不得泄漏引擎进程
        child.kill_group();

        if connected && child_alive {
            bound = Some(port);
            break;
        }
    }
    // Assert: 赢得 rebind 竞争的 attempt 才算通过(budget 耗尽 = fail-closed)
    bound
        .expect("bao browser --cdp-port must listen on a freshly probed port within the retry budget");
}
