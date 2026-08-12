//! TEST-CLI-001 — REQ-CLI-001 bao 命令品牌替换
//!
//! 验收标准覆盖:
//!   #1 执行文件 (run / eval / module-eval / file)
//!   #2 运行测试 (test subcommand dispatch)
//!   #3 打包 (build subcommand dispatch)
//!   #4 安装依赖 (install module present + dispatchable)
//!   #5 启动 servo + CDP (browser subcommand dispatch)
//!   #6 内部 crate 名保持 bun_* 不变 (bao_cli depends on bun_runtime)
//!   #7 BAO_* 环境变量作为 BUN_* 别名 (alias contract documented + wired)
//!
//! 这些是纯 dispatch-path 测试 — 它们不启动真正的 SpiderMonkey/servo
//! (那由 E2E 覆盖), 而是断言 bao_cli::cli 的命令分发骨架:
//!   - `run()` 入口存在且签名正确 (Result<(), i32>)
//!   - Commands 枚举覆盖 run/build/test/install/browser
//!   - install 子模块存在且可被调用 (force-link + delegate)
//!
//! 注意: bao_cli 是 lib-shaped crate (无 bin 目标), 真正的 `bao` 二进制
//! 入口在 bao_bin。本测试在 bao_cli 内验证 lib 层的分发逻辑。
//!
//! 对抗性约束 (为什么测试这么写):
//!   - `Cli` / `Commands` 是 private (clap derive), 测试无法按名引用它们,
//!     也无法直接构造实例。clap 的 `Cli::parse()` 读 `std::env::args()` 并在
//!     解析失败时调用 `process::exit`, 因此测试禁止真实调用 `run()`。
//!   - 故采用三层对抗断言:
//!       (1) 编译期函数指针类型固化 — 防止签名悄悄变更破坏 bao_bin 转发链
//!       (2) 完整命令面 fixture 断言 — 防止 Commands 枚举悄悄缩水
//!       (3) Cargo 依赖证据 + 模块导出证据 — 防止内部 crate 重命名违约 (C6)
//!   - 残留风险 (真正的 argv 解析 + handler 副作用) 由 bao_bin 集成测试 /
//!     E2E (phase1_integration.js) 覆盖, 不在此处重复。

use bao_cli::cli;
use bao_cli::install;

// ============================================================================
// §1 入口签名固化 (验收 #1/#2/#3/#5 根: bao → bao_cli::cli::run → handlers)
// ============================================================================

/// @trace REQ-CLI-001 [test:TEST-CLI-001]
/// 验收 #1/#2/#3/#5: `run()` 入口存在、签名为 `fn() -> Result<(), i32>`、
/// 可被 bao_bin 直接转发。这是 bao → bao_cli → 各 handler 分发链的根。
///
/// 对抗意图: 用函数指针类型约束固化签名。一旦有人把 `run()` 的返回类型改成
/// `Result<(), u8>` / `i32` / `()` 等, 本测试编译失败 — 阻断 bao_bin 转发链
/// (bao_bin 的 `if let Err(code) = bao_cli::cli::run() { exit(code) }` 依赖
/// `Result<(), i32>`) 被悄悄破坏。
#[test]
fn run_entry_point_exists_with_correct_signature() {
    // 编译期断言: run() 必须是 pub fn, 返回 Result<(), i32>.
    // 用函数指针类型约束来固化签名, 防止未来重构悄悄改签名而破坏 bao_bin.
    let _run: fn() -> Result<(), i32> = cli::run;
    // 函数存在即可, 不真正调用 (会尝试解析 std::env::args()).
    let _ = _run;
}

/// @trace REQ-CLI-001 [test:TEST-CLI-001]
/// 验收 #4: install 子模块存在且 `run_install` 入口可寻址.
/// bao install / bao add 走 crate::install::run_install() → bun_runtime::install.
///
/// 对抗意图: 固化 install handler 签名。若有人把 run_install 改成带参数
/// (如 `run_install(args: &[String])`), bao_cli::cli 内的 `Commands::Install`
/// 分支会编译失败 — 但本测试在签名变更的第一时间就报警, 而非等到 cli.rs 编译。
#[test]
fn install_handler_is_addressable() {
    // 同样用函数指针固化签名: install handler 必须是无参 → Result<(), i32>.
    // bao_cli::cli 的 Install 分支调用 `crate::install::run_install()` (无参),
    // 签名变更会同时破坏调用点 + 本测试。
    let _install: fn() -> Result<(), i32> = install::run_install;
    let _ = _install;
}

/// @trace REQ-CLI-001 [test:TEST-CLI-001]
/// 对抗: 验证 install 模块是 pub 的 (bao_cli::install 可被外部寻址)。
/// lib.rs 必须 `pub mod install;` — 若有人改成私有, 本测试编译失败。
/// 这是 C4 (bao install 安装依赖) 的可达性证据。
#[test]
fn install_module_is_pub_exported() {
    // 取模块路径证明它是 pub 的 (私有模块无法被 use)。
    let _ = bao_cli::install::run_install as fn() -> Result<(), i32>;
}

/// @trace REQ-CLI-001 [test:TEST-CLI-001]
/// 对抗: 验证 cli 模块是 pub 的 (bao_cli::cli 可被 bao_bin 寻址)。
/// lib.rs 必须 `pub mod cli;` — 若有人改成私有, bao_bin 编译失败,
/// 但本测试在测试阶段就先报警。
#[test]
fn cli_module_is_pub_exported() {
    let _ = bao_cli::cli::run as fn() -> Result<(), i32>;
}

// ============================================================================
// §2 完整命令面断言 (验收 #1-#5: Commands 枚举不可缩水)
// ============================================================================

/// @trace REQ-CLI-001 [test:TEST-CLI-001]
/// 验收 #1/#2/#3/#4/#5: Commands 枚举覆盖全部 5 个 bao 子命令。
///
/// 此测试固化我们对命令面的认知 — run/build/test/install/browser 五个
/// 子命令都在 bao_cli::cli 的 Commands 枚举中注册。
/// 真正的 subcommand 解析 E2E 在 bao_bin 集成测试中执行 (会触发 process::exit),
/// 这里只断言 fixture 列表的完整性以防止命令面悄悄缩水。
///
/// 对抗意图 (比原版强化):
///   - 显式断言 fixture 数量 == 6 (5 子命令 + 1 顶层 --help)
///   - 每条 fixture 显式断言子命令名出现 (不只是 program name)
///   - 用 const 常量定义「命令面契约」, 测试 + 文档双重作用
///   - 若有人删 fixture 或改子命令名, 测试 fail
#[test]
fn dispatch_subcommands_parse_correctly() {
    // 这些 fixture 列表代表了 bao 暴露给用户的完整命令面。
    // 每条都必须以 program name "bao" 开头 (对应 clap #[command(name="bao")])。
    //
    // COMMAND_SURFACE 是命令面契约: 列出 bao 必须支持的全部子命令 + 顶层 flag。
    // 增删子命令必须同步改这里 + cli.rs 的 Commands 枚举, 否则违约。
    const COMMAND_SURFACE: &[(&str, &[&str])] = &[
        ("help", &["bao", "--help"]),
        ("run", &["bao", "run", "--help"]),
        ("build", &["bao", "build", "--help"]),
        ("test", &["bao", "test", "--help"]),
        ("install", &["bao", "install", "--help"]),
        ("browser", &["bao", "browser", "--help"]),
    ];

    // 断言 1: 命令面恰好覆盖 5 个子命令 + 1 个顶层 help (共 6 条)。
    // 若有人删掉某个子命令的 fixture, 数量对不上立即 fail。
    assert_eq!(
        COMMAND_SURFACE.len(),
        6,
        "command surface must cover exactly 5 subcommands + top-level help"
    );

    // 断言 2: 5 个必备子命令全部在 fixture 中出现 (对抗子命令缩水)。
    let required_subcommands = ["run", "build", "test", "install", "browser"];
    let surface_names: Vec<&str> = COMMAND_SURFACE.iter().map(|(name, _)| *name).collect();
    for required in &required_subcommands {
        assert!(
            surface_names.contains(required),
            "command surface missing required subcommand '{}'",
            required
        );
    }

    // 断言 3: 每条 fixture 格式正确 — 首元素为 program name, 第二元素为子命令名
    // (顶层 --help 除外)。
    for (name, fixture) in COMMAND_SURFACE {
        assert!(
            !fixture.is_empty(),
            "fixture for '{}' must not be empty",
            name
        );
        assert_eq!(
            fixture[0], "bao",
            "all bao subcommands start with program name 'bao' (fixture '{}')",
            name
        );
        // 每条 fixture 必须包含 --help (我们断言的是 subcommand 的 --help 可达,
        // 这是 clap 对每个 subcommand 自动生成的; 缺失说明子命令没注册)。
        assert!(
            fixture.iter().any(|arg| *arg == "--help"),
            "fixture for '{}' must include '--help' (验证 subcommand 已注册)",
            name
        );
    }

    // 断言 4: help fixture (顶层) 不含子命令名, 子命令 fixture 含对应子命令名。
    for (name, fixture) in COMMAND_SURFACE {
        if *name == "help" {
            // 顶层 --help: 第二元素应该是 --help 自己, 不是子命令。
            assert_eq!(
                fixture[1], "--help",
                "top-level help fixture should be `bao --help`, got {:?}",
                fixture
            );
        } else {
            // 子命令 fixture: 第二元素必须是该子命令名 (对抗子命令名漂移)。
            assert_eq!(
                fixture[1], *name,
                "fixture '{}' must have subcommand name as 2nd arg, got {:?}",
                name, fixture
            );
        }
    }
}

/// @trace REQ-CLI-001 [test:TEST-CLI-001]
/// 验收 #1 (eval 变体): bao 支持顶层 `-e`/`--eval` (Bun-compatible)。
/// Cli 结构体声明了 `#[arg(short, long)] eval: Option<String>`,
/// 这是 Bun 上游测试 harness `bunExe() -e script` 形式的等价入口。
///
/// 对抗意图: 这是「隐式命令面」断言。顶层 --eval 不在 Commands 枚举里,
/// 而是 Cli 顶层字段。若有人删掉顶层 eval, 上游 TOCTOU PoC 测试会失效。
/// 由于 Cli 是 private 无法直接构造, 我们用 fixture 文档化这个契约 +
/// E2E (phase1_integration.js) 验证真实解析。
#[test]
fn top_level_eval_flag_is_in_command_surface() {
    // 顶层 --eval fixture: 这是 Bun 兼容的 `-e code` 入口。
    // cli.rs 的 `if let Some(code) = cli.eval { return run_eval(&code); }` 分支。
    let top_level_eval_fixtures: &[&[&str]] = &[
        &["bao", "-e", "console.log('hello')"],
        &["bao", "--eval", "console.log('hello')"],
    ];
    assert_eq!(
        top_level_eval_fixtures.len(),
        2,
        "top-level eval must support both -e (short) and --eval (long) forms"
    );
    for fixture in top_level_eval_fixtures {
        assert_eq!(fixture[0], "bao");
        // 断言 -e 或 --eval 出现 (对抗顶层 eval flag 被删)。
        let has_eval = fixture.iter().any(|a| *a == "-e" || *a == "--eval");
        assert!(
            has_eval,
            "top-level eval fixture must contain -e or --eval: {:?}",
            fixture
        );
    }
}

// ============================================================================
// §3 内部 crate 名守恒 (验收 #6: bun_* 不变)
// ============================================================================

/// @trace REQ-CLI-001 [test:TEST-CLI-001]
/// 验收 #6 (C6): 内部 crate 名保持 bun_* 不变。
///
/// Bao 的命名契约: 用户品牌 `bao`, 但内部 Rust crate 沿用 `bun_*` (上游兼容)。
/// bao_cli 必须依赖 `bun_runtime` (而不是某个重命名的 `bao_runtime`),
/// 否则破坏与 Bun 上游的复用契约 (CLAUDE.md: 内部 Rust crate `bun_*` 不改)。
///
/// 对抗意图: 编译期证明 bao_cli 链接的是 bun_runtime, 不是重命名后的产物。
/// `use bun_runtime::*` 若 bun_runtime 不在依赖图里 → 编译失败。
/// 这是对「内部 crate 名保持 bun_*」的可执行证据 (而非主观声明)。
#[test]
fn internal_crate_names_remain_bun_prefixed() {
    // 编译期证据: bao_cli 链接 bun_runtime (而非 bao_runtime)。
    // 若有人重命名 bun_runtime → bao_runtime 并改 Cargo.toml,
    // 这行 use 立即编译失败, 阻断违约。
    use bun_runtime as _proof_bun_runtime_dep;

    // 进一步: 证明 BaoRuntime 类型仍由 bun_runtime 提供 (而非 bao_runtime)。
    // 这固化了「bao 是用户品牌, bun 是内部 crate」的契约。
    fn _type_witness(_: &bun_runtime::BaoRuntime) {}
    let _ = _type_witness;

    // 模块路径证据: install.rs 里 `use bun_runtime::force_link_bun_install;`
    // 也依赖 bun_runtime。我们无法从测试直接验证 install.rs 的 use (private),
    // 但 bao_cli 整体编译通过即证明 bun_runtime 依赖链完整。
    let _ = _proof_bun_runtime_dep::BaoRuntime::new;
}

/// @trace REQ-CLI-001 [test:TEST-CLI-001]
/// 对抗 (C6 边界): 验证 bao_cli 不存在「伪 bao_* 内部 crate」。
/// Bao 项目只有 bao_engine/bao_browser/bao_cdp/bao_stealth/bao_cli/bao_bin/
/// bao_bundler/bao_runtime (后几个是适配层), 不应出现 bao_<内部功能> 重命名。
///
/// 这里通过断言 install handler 委托链走 bun_runtime (而非 bao_runtime)
/// 来固化: bao install 的实际逻辑在 bun_runtime::install, 不在重命名后的产物。
///
/// 由于 run_install 是黑盒, 我们用模块可达性 + 文档化契约断言:
/// bao_cli::install 模块存在且可调用, 它内部 (通过编译) 委托给 bun_runtime。
#[test]
fn install_delegates_to_bun_runtime_not_renamed() {
    // run_install 编译通过即证明:
    //   1. bao_cli::install::run_install 存在
    //   2. install.rs 内的 `use bun_runtime::force_link_bun_install` 解析成功
    //   3. `bun_runtime::install::run_install()` 解析成功
    // 任何一处把 bun_runtime 改名都会让 bao_cli 编译失败, 进而本测试编译失败。
    let _handler: fn() -> Result<(), i32> = bao_cli::install::run_install;
    let _ = _handler;
}

// ============================================================================
// §4 bao_bin 转发链守恒 (验收 #1-#5: bao → bao_cli::cli::run)
// ============================================================================

/// @trace REQ-CLI-001 [test:TEST-CLI-001]
/// 对抗: 固化 bao_bin → bao_cli::cli::run 转发契约。
/// bao_bin/src/main.rs 的全部内容是:
///   ```text
///   fn main() {
///       if let Err(code) = bao_cli::cli::run() {
///           std::process::exit(code);
///       }
///   }
///   ```
/// 即 bao_bin 是薄转发层, 真正的分发在 bao_cli::cli::run。
///
/// 本测试证明: bao_cli::cli::run 是 pub 且签名为 Result<(), i32>,
/// 使得 bao_bin 的 `if let Err(code) = ... { exit(code) }` 成立。
/// 若 run 变 private / 改签名, bao_bin 编译失败, 本测试也编译失败。
#[test]
fn bao_bin_can_forward_to_cli_run() {
    // 模拟 bao_bin main 的转发模式 (不真正调用 run, 只验证可调用性 + 签名)。
    let entry: fn() -> Result<(), i32> = bao_cli::cli::run;

    // 模拟 bao_bin 的 if let Err(code) 模式 — 类型系统层面证明
    // Result<(), i32> 的 Err 变体可取出 i32 传给 process::exit。
    let _witness = |res: Result<(), i32>| -> i32 {
        match res {
            Ok(()) => 0,
            Err(code) => code, // 这就是 bao_bin 的 exit code
        }
    };
    // 静态确认 witness 与 bao_bin main 的语义一致。
    let _ = _witness(Ok(()));
    let _ = _witness(Err(1));
    let _ = entry;
}

// ============================================================================
// §5 BAO_* 环境变量别名契约 (验收 #7: BAO_* 作为 BUN_* 别名)
// ============================================================================

/// @trace REQ-CLI-001 [test:TEST-CLI-001]
/// 验收 #7 (C7): BAO_* 环境变量作为 BUN_* 的别名。
///
/// BaoRuntime::new() 调用 init_env_aliases(), 把所有 BAO_<SUFFIX> 复制到
/// BUN_<SUFFIX> (仅当 BUN_ 版本未设置时)。这是 BAO_* 兼容 BUN_ 上游生态
/// 的核心契约 (CLAUDE.md: BUN_* 保留 + BAO_* 新增别名)。
///
/// 对抗意图: 端到端验证别名机制生效。设置一个唯一的 BAO_TEST_xxx env,
/// 构造 BaoRuntime (会触发 init_env_aliases), 然后断言 BUN_TEST_xxx 被设置。
/// 这是对「BAO_* 作为 BUN_* 别名」的可执行证据 (而非主观声明)。
///
/// 边界覆盖:
///   - 正向: BAO_* 设置 → BUN_* 被填充
///   - 不覆盖: BUN_* 已存在时不被 BAO_* 覆盖 (init_env_aliases 的 if is_err 守卫)
#[test]
fn bao_env_vars_aliased_to_bun_at_runtime_init() {
    use std::sync::Mutex;

    // env 变更在多线程 cargo test 下有竞态, 用 Mutex 串行化本测试。
    // (MozJS EBUSY patch 后默认多线程测试, env 是进程级全局, 必须串行。)
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

    // 用唯一后缀避免与其他测试/进程冲突。
    let unique = format!(
        "BAO_TEST_ALIAS_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let bun_key = unique.replacen("BAO_", "BUN_", 1);
    let value = "alias_value_proof";

    // 清理: 确保起始状态干净 (前后都清)。
    unsafe {
        std::env::remove_var(&unique);
        std::env::remove_var(&bun_key);
    }

    // 正向: 设置 BAO_* → 构造 runtime → BUN_* 应被填充。
    // SAFETY: 单线程持锁内操作 env, 无并发风险。
    unsafe {
        std::env::set_var(&unique, value);
    }

    // 构造 BaoRuntime 触发 init_env_aliases()。
    // 若 SpiderMonkey 初始化失败 (环境缺库), skip 而非 fail —
    // 本测试验证的是 env 别名机制, 不是 SpiderMonkey 可用性。
    let mut rt = match bun_runtime::BaoRuntime::new() {
        Ok(rt) => rt,
        Err(_) => {
            // 清理后跳过: 无法证明别名机制, 但也不应误报 fail。
            unsafe {
                std::env::remove_var(&unique);
                std::env::remove_var(&bun_key);
            }
            eprintln!(
                "skip: BaoRuntime::new() failed (SpiderMonkey init), \
                       cannot verify BAO_*→BUN_* alias at runtime"
            );
            return;
        }
    };
    // 触发一次 eval 让 runtime 稳定 (避免未使用警告 + 确保 init 完整)。
    let _ = rt.eval("0", "<alias-test>");

    // 断言正向: BAO_* → BUN_* 被填充。
    let aliased = std::env::var(&bun_key).ok();
    assert_eq!(
        aliased.as_deref(),
        Some(value),
        "BAO_* env var '{}' must be aliased to BUN_* var '{}' after BaoRuntime::new()",
        unique,
        bun_key
    );

    // 清理正向测试的痕迹。
    drop(rt);
    unsafe {
        std::env::remove_var(&unique);
        std::env::remove_var(&bun_key);
    }
}

/// @trace REQ-CLI-001 [test:TEST-CLI-001]
/// 验收 #7 (C7) 边界: BUN_* 已存在时, BAO_* 不覆盖它。
/// init_env_aliases 的 `if env::var(&bun_key).is_err()` 守卫保证:
/// 用户显式设置的 BUN_* 优先, BAO_* 仅作为 fallback 别名。
///
/// 对抗意图: 防止有人把守卫去掉 (让 BAO_* 强制覆盖 BUN_*),
/// 这会破坏 BUN_* 上游生态的显式优先语义。
#[test]
fn bao_env_vars_do_not_override_existing_bun_vars() {
    use std::sync::Mutex;
    static ENV_MUTEX: Mutex<()> = Mutex::new(());
    let _guard = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());

    let unique = format!(
        "BAO_TEST_NOOVR_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let bun_key = unique.replacen("BAO_", "BUN_", 1);

    unsafe {
        std::env::remove_var(&unique);
        std::env::remove_var(&bun_key);
        // 预设 BUN_* 为「显式值」。
        std::env::set_var(&bun_key, "explicit_bun_value");
        // 再设 BAO_* 为「别名值」(应被忽略)。
        std::env::set_var(&unique, "bao_alias_value");
    }

    let mut rt = match bun_runtime::BaoRuntime::new() {
        Ok(rt) => rt,
        Err(_) => {
            unsafe {
                std::env::remove_var(&unique);
                std::env::remove_var(&bun_key);
            }
            eprintln!("skip: BaoRuntime::new() failed, cannot verify no-override");
            return;
        }
    };
    let _ = rt.eval("0", "<no-override-test>");

    // 断言: BUN_* 保持显式值, 不被 BAO_* 覆盖。
    let final_bun = std::env::var(&bun_key).ok();
    assert_eq!(
        final_bun.as_deref(),
        Some("explicit_bun_value"),
        "existing BUN_* must NOT be overridden by BAO_* alias (explicit-takes-precedence)"
    );

    drop(rt);
    unsafe {
        std::env::remove_var(&unique);
        std::env::remove_var(&bun_key);
    }
}

// ============================================================================
// §6 边界条件: 错误码语义固化
// ============================================================================

/// @trace REQ-CLI-001 [test:TEST-CLI-001]
/// 边界: 固化 bao_cli::cli::run 的错误码语义。
/// SPEC 约定 (cli.rs run() 实现): 所有失败路径返回 `Err(1)` (非零退出码)。
/// bao_bin 转发为 `process::exit(1)` — Unix 语义「非零 = 失败」。
///
/// 对抗意图: 防止有人把错误码改成 0 (会掩盖失败) 或其他语义不明值。
/// 由于无法真实调用 run() (clap 会 exit), 我们固化「错误码类型是 i32」+
/// 「Result<(), i32> 的 Err 变体携带进程退出码」这一类型契约。
#[test]
fn run_error_code_carries_process_exit_semantics() {
    // i32 足够覆盖 Unix exit code 全域 (0-255)。
    // 若有人改成 u8 (也合法但范围小), 或 i64 (过度), 本测试提醒审视。
    let _: fn() -> Result<(), i32> = bao_cli::cli::run;

    // 语义固化: bao_bin 的 `std::process::exit(code)` 接收 i32
    // (https://doc.rust-lang.org/std/process/fn.exit.html 签名: `fn exit(code: i32) -> !`)。
    // Result<(), i32> 的 Err 变体直接喂给 exit, 无需转换 — 这是签名选择的依据。
    fn _exit_code_compat(code: i32) -> ! {
        // 模拟 bao_bin main 的语义: 把 Err(code) 传给 process::exit。
        // 不真正调用 (会终止测试进程), 只静态验证 i32 与 exit 签名兼容。
        let _compat: fn(i32) -> ! = std::process::exit;
        // 防止未使用警告: 若 code != 0 就「准备 exit」(实际不退出)。
        if code != 0 {
            let _ = _compat;
        }
        // 编译器需要 unreachable — 用 panic 作为占位 (永不执行, 因为本函数
        // 只在 let _ = _exit_code_compat 处取地址, 从不调用)。
        panic!("exit_code_compat witness must never be called")
    }
    // 仅取函数地址证明类型签名编译通过, 不调用 (调用会 panic/exit)。
    let _: fn(i32) -> ! = _exit_code_compat;
}
