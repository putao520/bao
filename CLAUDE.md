# Bao (包子) — Bun + SpiderMonkey + Servo

**高性能反指纹浏览器运行时。** SpiderMonkey 引擎 + servo 全功能浏览器 + Node.js/Bun API 始终在线 + 内置 Stealth 反指纹,一个 Rust 运行时全搞定。

## 核心愿景

把浏览器引擎、JS 运行时、反指纹能力统一到一个 Rust 二进制里:

- **反指纹浏览器** — 默认对抗 TLS/HTTP2/Canvas/Navigator/WebGL/Audio/行为指纹检测
- **Bun 兼容运行时** — `require` / `fs` / `http` / `crypto` / `bun:sqlite` 等 Node.js + Bun API 始终在线,与 Web API 同一 JSContext 共存
- **Headless 多页面库** — `PagePool` 多页面管理,`PageHandle` 高层 API(navigate / evaluate / screenshot)
- **CDP 自动化** — 内置 CDP Server,Playwright/Puppeteer 可直连 `ws://127.0.0.1:9222`

## 核心原则(铁律,所有工作必须遵守)

### 1. Bun 适配 Servo(Servo 是上游真源)

**遇到冲突改 Bun 不改 Servo。Servo 是上游,Bun 是下游。**

- Servo 代码禁止修改(BCE-002 / BCE-004 用户破例授权的 `script_thread.rs` / `lib.rs` patch 除外,已沉淀)
- Bao 层(`bao_engine` / `bao_browser` / `bao_cdp` / `bao_cdp_client` / `bao_stealth` / `bao_runtime` / `bao_uloop`)只适配 servo 的接口与数据模型
- Bun 的 C/Zig 层 → Rust 替换;JSC → SpiderMonkey 桥接是唯一需要手写的桥接层

### 2. JSContext 模型(BCE-20260621-001 修订版,废止旧"唯一 JSContext 共享"铁律)

**全局唯一 JSEngine + 每个 ScriptThread 持有自己的线程局部 JSContext**(servo 上游 `script_runtime.rs` 的 `RustRuntime::get()` 是 thread-local slot;SAFETY 注释:"only one JSContext can exist on the thread")。

- 各模式(CLI / browser / CDP)各自在其所属线程内使用该线程的 JSContext
- DOM ↔ Node.js 互操作必须发生在**同一线程内**(跨线程会破坏 activation 栈导致 SIGSEGV,此即 PagePool 混沌 SIGSEGV 根因)
- **禁止跨线程传递 `JSObject` 裸指针(铁律)**:`JSObject` 归属于创建它的线程的 JSContext。bao 层不得在跨线程结构(`DashMap` / `Mutex` / 全局 `static`)中持有 `JSObject` 裸指针,跨线程只能传 `PageId` / 句柄 / 序列化数据
- `bao_engine` / `bao_browser` 通过 `RustRuntime::get()` 获取当前线程的 JSContext(thread-local),不创建独立 JSEngine

### 3. 复用优先(Bun crate > 社区库 > 翻译他语言库 > 手写)

Bun workspace 中 ~85 个纯 Rust crate(零 JSC)是经过生产验证的高性能实现,**100% 复用,禁止手写已有功能**。

```
1. workspace 内 bun_* crate(已编译、已优化、已测试)
2. crates.io 成熟库(url, sha2, hmac, kurbo, svgtypes, etc. — 优先查依赖树内已有版本)
3. 无 Rust 库时:翻译同功能他语言库(如 mozilla C++ 参考实现)
4. 仅当 1/2/3 都没有时才允许手写(用户裁决 2026-09-27:实在没办法才自己裸写)
```

**只有以下情况允许手写 Rust**:

1. loop 核心必须与 `FilePoll` 共享 epoll fd → `bao_uloop` 的 epoll tick 是必要的
2. JSC → SM 桥接层(`bao_engine`)是必要的
3. Servo 集成桥接层(`bao_browser`)是必要的
4. CDP / Stealth / Node.js 兼容层(`bao_cdp` / `bao_stealth` / `bao_runtime`)是必要的

**禁止手写**(链接 / 复用 C++ 二进制或 Bun crate):

- `us_socket_*` / `us_socket_group_*` / `us_listen_socket_*` → 链接 C++ `libuwsockets.cpp` 二进制
- `bsd_send` / `bsd_shutdown` 等 BSD socket 辅助 → C++ 二进制已有
- HTTP 解析/响应 → `bun_uws::App`(C++ 二进制)已有
- DNS → `bun_dns` 已有
- 模块解析 → `bun_resolver` 已有
- Base64 → `bun_base64` 已有

### 4. 三化原则

| 原则 | 含义 | 检查点 |
|------|------|--------|
| **高性能化** | 零拷贝、SIMD、mmap、io_uring — 复用 Bun 已有的优化 | 禁止 `Vec::new()` 手写 buffer、禁止 `String::from_utf8_lossy` 替代零拷贝 |
| **去锁化** | 单线程 JS 执行模型下禁止 `Mutex`/`RwLock`,用 `thread_local!` + `RefCell` | `Mutex` 仅用于跨线程共享(HTTP 等真正的并发场景) |
| **成熟库化** | workspace 已有 crate > crates.io 成熟库 > 手写 | 每个新函数先 grep workspace crate 是否已有实现 |

### 5. SPEC SSOT + 范围守恒 + BCE(C-1 / C-5 / C-7)

- **SPEC SSOT** — `.spec/` 是唯一真相来源。SPEC 有定义按 SPEC 执行;SPEC 未定义停止报告用户,禁止自行补充
- **范围守恒** — 交付范围 ≡ SPEC 定义范围,双向零差集
- **BUG 类根除(BCE)** — 任何错误修复后强制走 BCE 闭环(归因 → 泛化 → 全项目横扫 → 批量根治 → 全量确认残留=0 → 防复发沉淀)。完整定义见 `~/.claude/rules/bug-class-eradication.md`

## 命名规范

| 层级 | 规则 |
|------|------|
| 用户品牌 | `bao`(`bao run` / `bao test` / `bao browser`) |
| JS 全局对象 | `Bun.*`(保留) + `Bao.*`(别名,同一对象) |
| 内部 Rust crate | `bun_*` 不改(保持上游兼容);`bao_*` 是新建层 |
| 环境变量 | `BUN_*`(保留) + `BAO_*`(新增别名,`NodeRuntime::new()` 调用 `init_env_aliases()` 把 `BAO_<SUFFIX>` 复制到 `BUN_<SUFFIX>`) |
| 代码引用 | 保留所有 Bun 内部引用 |

原则:用户输入 `bao`,代码里还是 `bun`。最小化与上游 Bun 的 diff。

## 架构分层

```
┌──────────────────────────────────────────────────────────┐
│                     bao (CLI binary)                      │
│            bao_bin → bao_cli (clap subcommands)           │
├────────────┬────────────┬──────────┬─────────────────────┤
│ bao_engine │ bao_browser│ bao_cdp  │ bao_stealth         │
│ SpiderMonkey│  Servo 桥  │ CDP WS   │ 反指纹              │
│ JSC→SM 桥  │ PagePool   │ Router   │ TLS JA3/JA4         │
│ context/   │ PageHandle │ Session  │ HTTP2 AKAMAI        │
│ job_queue  │ evaluate   │ 12 域    │ Canvas/WebGL/Audio  │
├────────────┴────────────┴──────────┴─────────────────────┤
│ bao_cdp_client  Playwright 风格高层 API(Browser/Page/...) │
├──────────────────────────────────────────────────────────┤
│ bao_runtime  Node.js/Bun 兼容(fs/http/crypto/sqlite/ffi) │
├──────────────────────────────────────────────────────────┤
│ bao_uloop  事件循环(epoll tick,共享 FilePoll fd)         │
├──────────────────────────────────────────────────────────┤
│          Bun ~85 个纯 Rust crate(零修改复用)             │
├──────────────────────────────────────────────────────────┤
│ mozjs(SpiderMonkey FFI) · libservo · boringssl · cdp-protocol │
└──────────────────────────────────────────────────────────┘
```

### Bao 层 crate

| crate | 路径 | 职责 |
|-------|------|------|
| `bao` | `src/bao` | **对外唯一公共 lib**：整栈 re-export（引擎+浏览器+runtime+CDP+Stealth 始终链接，无产品 feature 拆分） |
| `bao_engine` | `src/bao_engine` | SpiderMonkey 引擎封装,re-export `bun_sm` 核心类型;`context` + `job_queue` 自有模块 |
| `bao_runtime` | `src/bao_runtime`(crate 名 `bun_runtime`) | Node.js/Bun API 兼容层;`NodeRuntime`(Node.js 运行时入口;旧名 `BaoRuntime` deprecated alias) |
| `bao_browser` | `src/bao_browser` | servo 集成桥;`BrowserRuntime`(浏览器运行时;旧名 `BaoRuntime` deprecated alias) + `PagePool` + `PageHandle` + `BaoServoDelegate` |
| `bao_cdp` | `src/bao_cdp` | CDP Server(`cdp-server` crate)+ servo 桥(`ServoTargetProvider` / `CDPRdpBridge`) |
| `bao_cdp_client` | `src/bao_cdp_client` | Playwright 风格高层 API(`Browser::connect("memory://bao" | "ws://...")`) |
| `bao_stealth` | `src/bao_stealth` | 反指纹引擎;`StealthProfile` + `StealthEngine`(TLS/HTTP2/Canvas/Navigator/WebGL/Audio/Behavior) |
| `bao_uloop` | `src/bao_uloop` | 事件循环;epoll tick 与 `FilePoll` 共享 fd |
| `bao_cli` / `bao_bin` | `src/bao_cli` / `src/bao_bin` | CLI(`bao` binary,clap subcommands) |
| `bao_bundler` | `src/bao_bundler` | 打包器(基于 `bun_bundler`) |
| `bao_crypto` | `src/bao_crypto` | crypto 桥(boringssl) |
| `bao_boringssl_bridge` | `src/bao_boringssl_bridge` | boringssl Rust 桥 |
| `bao_native_stubs` | `src/bao_native_stubs` | dispatch no-op stubs + C 库桥锚点 |
| `bao_engine_macros` | `src/bao_engine_macros` | `codegen_cached_accessors` 宏 |
| `bao_lints` | `src/bao_lints` | BCE 门禁 AST 检测器(GC-unsafe / SPEC id) |

## 技术栈

| 组件 | 来源 | 用途 |
|------|------|------|
| SpiderMonkey | `mozjs` crate(MPL-2.0,内置从源码编译) | JS 引擎(替代 JSC) |
| servo | `libservo`(MPL-2.0) | DOM + CSS + Layout + webrender 渲染 |
| boringssl | `bao_boringssl_bridge` + `boringssl_sys` | TLS(Stealth JA3/JA4) |
| cdp-protocol | crates.io(MIT) | CDP 类型定义 |
| Bun 基础设施 | ~85 个纯 Rust crate(MIT) | HTTP/FS/Resolver/Bundler/DNS/Base64/... |

## SPEC 体系

| SPEC 目录 | `.spec/` |
|-----------|----------|

### SPEC 文件清单

| 文件 | 内容 | 状态 |
|------|------|------|
| `00-INDEX.html` | 索引 | — |
| `01-BUSINESS.html` | 业务架构(功能模块树 · 用例图 · 指标维度表) | 草稿 |
| `02-SYSTEM.html` | 系统架构(Bun Crate DAG · Servo 组件 · 融合映射 · 多页面管理 · CDP 双层抽象 · Permission 沙箱) | 草稿 |
| `03-PROCESS.html` | 核心流程(JS 执行管线 · 渲染管线 · CDP 路由 · 状态机 · 时序约束 · 线程模型) | 草稿 |
| `04-DATA-MODEL.html` | 数据模型(18 Entity · 模型树 · 缓存策略 · Crate 数据流) | 草稿 |
| `05-IMPLEMENTATION.html` | 实施路线图(5 阶段任务分解 · 复用矩阵 · 风险矩阵 · 验证点) | 草稿 |
| `06-CDP-SERVER.html` | CDP Server 设计 | 草稿 |
| `10-REQUIREMENTS.html` | 功能需求(31 REQ · 6 域 ENG/CLI/BRW/CDP/STL/LIB · 5 NFR · 追溯矩阵) | 草稿 |
| `11-TESTING.html` | 测试用例 | 草稿 |

### REQ 域分布

| 域 | REQ | 范围 |
|----|-----|------|
| ENG | REQ-ENG-001~011 | SpiderMonkey 引擎 + Node.js 兼容 + bun:sqlite/ffi/fetch/vm |
| CLI | REQ-CLI-001~002 | `bao run` / `bao browser` 子命令 |
| BRW | REQ-BRW-001~003 | servo 浏览器集成 + 渲染 + 多页面 |
| CDP | REQ-CDP-001~008 | CDP Server + 12 域 + Router + Session |
| STL | REQ-STL-001~007 | Stealth 反指纹(TLS/HTTP2/Canvas/Navigator/WebGL/Audio/Behavior) |
| LIB | REQ-LIB-001~004 | Headless 多页面库(PagePool/PageHandle) |

## 工作归属立法(用户裁决 2026-09-24)

**禁止把任何工作推到 daily-ops**。daily-ops 定时器与日常开发无关:交互会话中发现/立项的一切缺陷修复、上游吸收、测试面工作都由当前会话即时完成;`SKIPPED_BUSY`/backlog 让位不是推迟理由。daily-ops 仅在其自主定时窗口运行,不作为任何交互工作的下游承接方。

## 每日自动化(daily-ops)

systemd timer 每日 06:07±10min 运行 `.claude/skills/daily-ops/`(全自主:上游任意窗口吸收含 BCE patch 重放 + issue 根治 + 波末验收 + 发布闭包,含 mozjs 跨版本升级(单轮 7 天预算长任务协议);2026-08-24 用户裁决扩权,详见该 skill)。交互会话在此窗口派工前先查 `systemctl --user status bao-daily-ops`。上游同步基线 SSOT = `.claude/upstream-baseline.json`。

## 构建与测试

### 远程农场(bao 自有槽,用户裁决 2026-10-02 项目自有化)

| 项 | 值 |
|---|---|
| 主机 | 16.18.0.1(putao@) |
| 远程项目根 | `/home/putao/build-farm/bao`(src/ + outbox/ + Dockerfile + docker-compose.yml) |
| 本地脚本入口 | `~/.local/bin/bao-remote-build <check\|test\|build\|run\|raw> [args...]`(JOBS 默认 8) |
| 同步 | rsync -a --delete 工作树(含 .git,**未提交改动可达**——farm 增量 mtime 保留);排除 target/.claude/bench/results/.venv/node_modules/__pycache__ |
| 镜像 | 自有 `bao-build:ubuntu24`(compose build 现场);依赖含 uv/llvm-18-tools/xvfb 等 mozjs+servo 构建面 |
| 卷 | 私有 `bao-target`;共享 farm-rustup/farm-cargo/farm-sccache(farm 标准语言级卷,external) |
| 回退 | farm 不可达时本机直跑同命令(小 jobs) |

历史教训(2026-10-01):借用 frog 槽(git fetch+reset --hard origin/main 同步)导致未提交树结构性不可达,E 的 farm 证据全体无效——bao 槽 rsync 同步根治。容器内 root 会在 /src 生成 .venv(wrapper 已排除,勿手动同步该面)。

```bash
# 构建(首次构建 mozjs 从源码编译,耗时较长)
cargo build

# 构建二进制
cargo build -p bao_bin        # 产物:target/debug/bao

# 运行测试(见下方「测试运行纪律」:plain cargo test 必须 --test-threads=1)
cargo test --test-threads=1

# BCE 门禁(参见 Makefile)
make bce-check
```

StyleThread 栈 8MiB(上游 parity):bao 不走上游 mach,`.cargo/config.toml` [env] 持久化 `SERVO_STYLE_THREAD_STACK_SIZE_KB=8192`(上游 PR #43888 于 command_base.py:441 编译期注入 8MiB,"match the recursion depth of Chromium");缺省 512KB(vendor/stylo/parallel.rs:40)使 layout 2020 box-tree 深度成比例递归(dom_traversal.rs↔flow/construct.rs,每 DOM 层 8-13 帧)在 StyleThread worker 溢出——WPT scope-deep/inline-crash/outline-scrollIntoView 三条即此。option_env! 无 rebuild 追踪:改值后必须 `cargo clean -p bao-stylo`。

### cargo 宇宙拓扑(2026-09-21 归一后,用户裁决「全部立即归一」)

单一宇宙:全仓唯一 `[workspace]` 根 / `Cargo.lock` / `[patch.crates-io]`(freetype 1 条,主根)/ `rust-toolchain.toml`(nightly pin)。vendor 四仓形态:

- `vendor/servo` 组件 = 主 workspace **非成员 path dep**(70 manifest 已内联全部 workspace 继承,5a2d85bc;`[workspace]` 虚拟根/Cargo.lock/rust-toolchain.toml 已删,a4a3b942;主根 exclude 保留 `vendor/servo` 作防再成员化护栏)。`vendor/mozjs` 同构(eu1 8d4c8260);`vendor/stylo` / `vendor/ipc-channel` / `vendor/freetype-wrapper` 本就是自洽单 crate。
- **验证 remap**:一律主根 `cargo check|cargo nt -p bao-servo-*`(`-p` 匹配全图包,含非成员)。禁 `cd vendor/servo`(根已删)。从主根首次对某 servo crate 跑 test 会按需解析其 dev-deps 并一次性增长主锁,跑后 `git diff Cargo.lock` 审计。
- **发布 remap**:servo lockstep 线走组件目录 `cargo publish --manifest-path vendor/servo/components/<c>/Cargo.toml`(manifest 自含;publish-verify 解析 registry freetype 0.8.0——2026-09-21 解析级实证,编译 parity 依 E17 符号对照)。
- 历史记录:本文档 2026-09-21 前的「双 workspace patch 链 / 双侧 lock」叙述为当时机制描述,现行为单侧主根 patch。
- `vendor/boringssl/rust/` 第五宇宙已灭(`375abb6b`):根 manifest 仅 members+resolver 3 零继承面,无需内联直接删根;考古实证 **bssl-\* rust crate 全仓零消费**(主锁无任何 bssl-\* 包;源码引用全为注释性出处标注;C 构建走 `src/boringssl_sys/csrc` 字节级镜像 + cc crate,CMakeLists.txt 系上游脚手架 bao 从不调用),删根零解析影响。六个 bssl-\* manifest 留作上游 rust bindings 源记录。

### 测试运行纪律(集成测试已收敛为单 harness suite)

7 个重引擎 crate(`bao_runtime` / `bao_browser` / `bao_stealth` / `cdp-server` / `bao_engine` / `bao_cdp` / `bao_cdp_client`)的集成测试已结构性收敛:原 `tests/` 顶层每个 `.rs` 都是独立 auto-discovered target(每个全引擎链接,332 个测试二进制、267 个 ≥500M、合计 225G),现全部并入各自 `tests/suite/` 单 harness target(`tests/suite/main.rs` 为聚合根,子目录不被 auto-discover)。**运行时隔离由 cargo-nextest 保证**——每个 `#[test]` 独立进程运行,合并二进制不改变测试隔离语义(这是本结构的成立前提)。

| 场景 | 命令 | 说明 |
|------|------|------|
| 日常迭代(**必须 scoped**) | `cargo nt -p <crate>` 或 `-E '<filterset>'` 过滤 | dev profile;禁无过滤全量。例:`cargo nt -p bun_runtime -E 'test(buffer_conformance)'` |
| 批量 / 回归 | `cargo nextest run --cargo-profile test-ci`(`-p <crate>` 可选) | stripped + opt-level 2 二进制(workspace `[profile.test-ci]`),磁盘占用最小 |
| dev profile 全量构建 | 仅限需要 backtrace 符号调试时 | dev(debug=1)测试二进制极大;批量跑测试不要用 dev 全量 |
| plain `cargo test` | `cargo test --test-threads=1` | suite 单二进制内 libtest 默认多线程与引擎进程内单例(mozjs per-process singleton)冲突;nextest 每 test 独立进程,无此问题 |

- **`--cargo-profile` ≠ `-P`**:nextest 的 `--cargo-profile test-ci` 选 cargo 构建 profile;`-P/--profile` 选 nextest 自身配置(`.config/nextest.toml`),两者不同
- **suite 结构约定**:新增集成测试一律放 `tests/suite/<name>_tests.rs` 并在 `tests/suite/main.rs` 加 `mod <name>_tests;`。**禁止在 `tests/` 顶层新建 `.rs` 文件**(每个都会重新变成独立全引擎 target),也禁止在 `tests/` 下新建含 `main.rs` 的子目录**(cargo 会 auto-discover 为新 target;共享 helper 用 `mod.rs` + `#[path]` 引入,参照 `tests/suite/node_conformance/mod.rs`、`tests/suite/common/`)
- **WPT 波基线二进制 = opt 构建(2026-10-04,e45/e46 波裁决)**:WPT campaign 一律用 opt profile 二进制(`cargo build --profile test-ci -p bao_bin`,私有 CARGO_TARGET_DIR 防锁竞争);dev/debug_info 族二进制 DOM getter 每-op ~500ns 通胀 × 重 op 数测试 → 假 TIMEOUT/假 CRASH 族(B1 六格实录:debug 下 3 CRASH+3 TIMEOUT,opt 下 6/6 全绿)。CRASH+零 stderr 的静默死亡格先按「wptrunner SIGTERM 杀挂死浏览器」判别律分流(memory `wpt-crash-sigterm-harness-class.md`)再立缺陷假说

### mozjs 构建经验

1. **已内置从源码编译**:`mozjs-sys/build.rs` 的 `should_build_from_source()` 硬编码返回 `true`,无需 `MOZJS_FROM_SOURCE=1`。EBUSY patch + 其他本地修复始终生效
2. **rlib 包含 native 代码**:`libmozjs_sys-*.rlib` 打包了 `libjs_static.a` 的全部 C++ 符号。改 `.a` 不够——必须删 rlib 重新编译
3. **mozjs make 增量构建 bug**:make 会编译新 `.o` 但不重新打包 `libjs_static.a`。需手动 `ar -d` + `ar -q` 替换,或删整个 build output 目录
4. **清理顺序**:删 `.fingerprint/mozjs_sys-*` + `deps/libmozjs*` + `build/mozjs_sys-*` + `incremental/mozjs*`,然后 `cargo build`
5. **真实构建目录**:CARGO_TARGET_DIR 由环境注入为 /var/cargo-builds/3c/6184ceb77072ba(repo 内 target/ 是残迹)——清理序(.fingerprint/deps/build/incremental)应对真实目录执行

#### EBUSY Patch(已应用)

`mozjs/mozjs-sys/mozjs/mozglue/misc/Mutex_posix.cpp` 的 `MutexImpl::~MutexImpl` 已 patch:

- 原始:`pthread_mutex_destroy` 返回非零时 `MOZ_CRASH`(SIGSEGV)
- Patch:忽略 `EBUSY`(libtest 线程池线程在 TLS teardown 时仍持有 mutex)
- 仅在 `result != 0 && result != EBUSY` 时才 `MOZ_CRASH`

如果 SIGSEGV 复现,第一步 `nm libmozjs_sys-*.rlib | grep MutexImplD1` 查 rlib 是否包含旧代码。

#### mozjs fork BAO patch 清单(7 项,SM153 形态全部在位——2026-09-21 前移波 em1 系列重锚+实构建/smoke 实证;第 6 项 2026-09-10 增;第 7 项 2026-09-21 增;第 4 项同日 SM153 重锚)

上游同步 mozjs 时必须逐项重放(参照 git 历史 `git show <old>:vendor/mozjs/...`):

| # | Patch | 位置 | 语义 |
|---|-------|------|------|
| 1 | EBUSY 激进版 | `mozjs-sys/mozjs/mozglue/misc/Mutex_posix.cpp` | `MutexImpl` 析构整体 `return;`(进程退出期 TLS 可能已 unmap,EBUSY 时原版 MOZ_CRASH) |
| 2 | JSEngine init race | `mozjs/src/rust.rs` | `PROCESS_ENGINE_OUTSTANDING` OnceLock + `process_handle()`:多 BaoRuntime 二次 init 从 `Err(AlreadyInitialized)` 恢复而非 panic |
| 3 | set_hide_script_from_debugger | `mozjs/src/rust.rs`(BCE-20260622-004) | CompileOptions 的 `hideScriptFromDebugger_` setter:抑制 `onNewScript` → AtomCacheHashTable SIGSEGV 路径 |
| 4 | BaselineFrame NULL activation guard | `mozjs-sys/mozjs/js/src/jit/BaselineFrame.cpp`(BCE-20260621-002) | OSR 入口 `cx->activation()`/`prev()` NULL 检查。**SM153 重锚(449a92a6,2026-09-21)**:153 的 initForOsr 改 void 签名(OSR trampoline 无 bail 通道),守卫改**脚本入口 pc 兜底**形态——帧初始化不变量保全(RUNNING_IN_INTERPRETER+setInterpreterFields 照常)、NULL 解引用不可能、真激活链在时行为不变;bao-0001 patch 本体重生成+roundtrip 字节验证 |
| 5 | JS_NewEmulatesUndefinedFunction | `mozjs-sys/mozjs/js/src/jsapi.cpp` + `js/src/jsapi.h` + `mozjs/src/jsapi2_wrappers.in.rs` | callable NativeObject 且 `typeof` 为 "undefined"(镜像 Bun `Buffer.transcode` stub)。**注意:jsapi.h 声明必须在 `namespace JS` 外(全局作用域),否则 bindgen 生成 `JS::` 前缀 mangled link_name 与 cpp 全局定义不匹配 → 链接失败** |
| 6 | BaoCollectRuntimeStats | `mozjs-sys/mozjs/jsglue.cpp` + `mozjs/src/glue2_wrappers.in.rs`(SM-EVOLUTION #27 裁决 6,a51a81ef) | `BAORuntimeStatsPOD`(7×usize)+ `JS::RuntimeStats` C++ 子类构造(no-op extra hooks)+三段 ServoSizes rollup(runtime+zone+realm,单段会低估);漏重放=loud 链接断,引擎 Memory 计量面(soak 探针)依赖 |
| 7 | EncodeStencil XDR 绑定 + TranscodeBuffer shim(REQ-ENG-012,#26 stage1,2026-09-21) | `mozjs-sys/build.rs`(blacklist 移除 `JS::EncodeStencil`)+ `mozjs-sys/src/jsglue.cpp`(Create/Destroy/Begin/Length 四 shim)+ `mozjs/src/jsapi2_wrappers.in.rs`(`wrappers2::EncodeStencil`,镜像 DecodeStencil)+ `mozjs/src/glue2_wrappers.in.rs`(4 wrap;零参 CreateTranscodeBuffer 手写——wrap! 宏零参不可用)+ smoke `mozjs/tests/stencil_xdr.rs` | XDR persistent cache encode 半边:上游 blacklist 动机=`TranscodeBuffer&`(mozilla::Vector<uint8_t>)bindgen 降级为 `u8` 无构造面;bindgen 保留真 C++ link_name 故 ABI 正确,buffer 经 jsglue shim 以 opaque 句柄持有(同 SetBuildId 先例)。**EMBEDDER CONTRACT:encode 前必须 `JS::SetProcessBuildIdOp` 装 op,否则 VersionCheck→GetScriptTranscodingBuildId 空函数指针 SIGSEGV(StencilXdr.cpp:1373;上游 issue 候选)**;上游同步 5 文件逐个重放 |
| 8 | BAO_SM_CHUNK_POOL_MAX 空 chunk 池上限(env 门控,默认关)(B2/W5 归因,2026-10-03) | `src-js/mozjs/js/src/gc/BaoChunkPoolCap.h`(新,env 读一次)+ `Allocator.cpp`(recycleChunk 挂点)+ `Heap.cpp`(clearCurrentChunk 挂点) | soak W5/W31 chunk 驻留归因结论:驻留字节 98%=空 chunk 池(202 MiB=199 池+3 附着);chunk unmap 只存在于 decommit pass,Normal GC 在 high-frequency 模式(距上次 GC<1s)恒跳过(`shouldDecommit` GC.cpp:2280)→ churn 下 freed chunk 全滞池。**默认 0/未设=上游语义零行为变化(env off 字节级验证:N=100/300/600 shrink 三档与 patch 前一致)**;N>0=池钉 N chunk,超限空 chunk 立即 COUNT_DESTROY_CHUNK+UnmapPages(镜像 `prepareToFreeChunk`——该函数 GC.cpp inline 无出线符号,跨 TU 不可链,两侧同步注释)。Phase C 实测:normal env-off 202 MiB → cap=8 11 MiB → cap=1 4 MiB。启用与否(A/B/C)用户裁决中;启用后 soak 入场门=RSS 平台=live+N MiB 池上限 |

**编译宇宙归一(2026-09-21,用户裁决"不允许一直存在 2 个")**:vendor/mozjs workspace 根三件套(`Cargo.toml`/`Cargo.lock`/`rust-toolchain.toml`)已删除,5 crate(mozjs-sys/mozjs/src-js/src-intl/src-python)以成员身份并入主 workspace(主根 members 显式列出 + exclude 移除 `vendor/mozjs`;mozjs-sys/mozjs 两 manifest 的 `*.workspace = true` 已内联为与原 vendor 根同值字面量)。单一 lockfile、单一工具链钉(主根 nightly-2026-07-20)、单一 SM 构建面;**SM153 等后续版本导入禁止复活 vendor workspace 根**——新版本 crate 直接以成员路径进主根 members。26G 双宇宙事故废料 vendor/mozjs/target/ 一并清除。

另:`mozjs-sys/build.rs` 有 2 个 BAO patch(`should_build_from_source() -> true` 硬编码、`fix_stale_archive_objects()` make 增量 stale .o 修复)。

#### servo 定制文件清单(31 个条目,上游同步时逐个重放)

上游同步 servo 时,先 `grep -rln "BCE-\|BAO " vendor/servo/components/` 重建清单,再按"upstream 基底 + patch 精确重放"迁移(patch 锚点与完整记录见 git log 各 stage commit message):

| 文件 | Patch 概要 |
|------|-----------|
| `net/http_loader.rs` | **http_fetch step 3 "handle fetch" 落地**(上游裸 TODO):`invoke_handle_fetch`——按 origin 查 `SwManagers` 注册表 → 构造 `CustomResponseMediator` 发给 SW manager → `tokio::task::spawn_blocking` + `IpcReceiver::try_recv_timeout(30s)` 有界等待回注(阻塞 recv 不占 async worker,同 websocket_loader DNS 形态)→ `Some(CustomResponse)` 转 net `Response`(status/headers/body 一次性 Done);任何失败(无 manager/通道失败/超时)→ None → 走原网络路径;`Destination::ServiceWorker` 排除(SW 脚本自抓防环,防 update job 自拦)(REQ-BRW-004 C19 S2b,用户裁决 2026-09-09 vendor patch)<br>**R53-A net 面身份贯通**:`request.target_webview_id` 传入 `obtain_response_bun`(bun bridge 按 per-WebViewId 注册表解析 stealth wire config)与 WS 路径 `create_tls_config`(2026-09-10 用户裁决 R53 方案 A) |
| `net/resource_thread.rs` + `net/fetch/methods.rs` | `CoreResourceManager.sw_managers` 上游只写不读的 `HashMap` → `SwManagers`(`Arc<Mutex<FxHashMap>>` 共享注册表,resource 线程写 NetworkMediator / tokio fetch 任务读);`FetchContext` 新增 `sw_managers` 字段贯通(net/tests 两处 FetchContext literal 同步补字段)(REQ-BRW-004 C19 S2b) |
| `net/request_interceptor.rs` | **webview-less WebResourceRequested 本地裁决**(BCE-20260910-002):`target_webview_id == None` 的 fetch(SW/worker realm)在 `intercept_request` 里先查进程级 `BAO_WEBVIEWLESS_RESOURCE_HANDLER`(parking_lot RwLock 全局 setter,同 http_loader C19-② tap 模式;bao_browser 于 `BaoRuntime::new` 经 `servo::set_webviewless_resource_handler` 安装,现裁决恒 `PassThrough` = 无 embedder 往返直接 DoNotIntercept——与 bao 继承的 no-op `ServoDelegate::load_web_resource` → `WebResourceLoad` drop → 默认 DoNotIntercept 字节等价);无 handler 时保留上游 embedder 往返不变,webview 持有请求恒走完整往返。根因:上游假定 embedder 常驻主循环持续 drain net→embedder 通道,bao 是惰性泵且 `Servo(Rc<ServoInner>)` !Send 无法常驻线程泵,页面空闲期 SW/worker fetch 在 interceptor 永挂(fetchevent 25s 卡死)。若未来 bao 为 webview-less 请求覆写 `load_web_resource`,逻辑归属此 handler |
| `script/dom/serviceworker/serviceworker_manager.rs` | **install() 补 "Try Activate" 步**(spec activation-algorithm:无 active worker 时 waiting→active 并清 waiting;上游从不传 `RegistrationUpdateTarget::Active`,`active_worker` 恒 None → SW 拦截永不生效)(REQ-BRW-004 C19 S2b,用户裁决 2026-09-09 vendor patch)<br>**install() 的 Resolve Job Promise 后移到 waiting→active 迁移之后**(上游 Step 7 在激活前发,resolved info 恒 pre-activation;后移使注册 resolve 携带 `active_worker`,成为 container controller 赋值的数据源)(REQ-BRW-004 C19 controller 波,用户裁决 2026-09-09 vendor patch) |
| `script/dom/serviceworker/serviceworkercontainer.rs` | **navigator.serviceWorker.controller 赋值链**(上游 getter 硬编码 `None`、`controller` 字段全树无人写,页面 JS 无从观察受控状态):`GetController` 返回字段;`refresh_controller`——注册 resolve(与 register promise 同 task)或 getRegistration 匹配携带 active worker 且 scope 前缀匹配本 global URL 时,存页面侧 ServiceWorker 对象为 controller;SW 换代(unregister+重注册)后 controller 跟随新 active worker。边界(最小子集):仅注册/查询过的 container 刷新(manager 每 registration 单 client,多 client 通知=claim()/clients 全家,不做);属性不清理(unregister 后残留旧对象,拦截本身在 manager 侧随 registration 删除已停);navigation SW 化上游未实现(未触碰 SW API 的同 scope 页面保持 null)(REQ-BRW-004 C19 controller 波,用户裁决 2026-09-09 vendor patch) |
| `script/dom/globalscope/globalscope.rs` | `fetch_with_network_listener` 补 spec main-fetch 级 SW-realm 降级:`is::<ServiceWorkerGlobalScope>()` → `service_workers_mode=None`(上游只在 fetch() DOM 入口降级,XHR 等 SW-realm 请求自拦截 → SW 线程阻塞在 sync 事件泵无法应答自己的 mediator → 30s 死锁)(REQ-BRW-004 C19 S2b)<br>**`egress_webview_id()`**(R53-A,2026-09-10):出站请求的 webview 身份——与 `webview_id()` 同,唯 SW global 解析为**注册页**的 WebViewId(`owning_webview_id`,ScopeThings 继承;上游对 SW 刻意 None 的存储分区语义不动),SW realm 的 fetch/XHR egress 由此归属宿主页 per-WebViewId stealth wire profile |
| `script/event_loop/script_thread.rs` | embedder 脚本/Worker-scope 回调注册(drain 于 handle_evaluate_javascript / run_worker_scope)、router_proxy 安装(BCE-20260627-009)、disable_script_debugger 门控(BCE-20260621-002)<br>**第二 Worker drain 点** `EMBEDDER_WORKER_INTERFACES_READY_CALLBACKS`(同 WebViewId 键控/FnOnce consume-once 同构,drain 于 workerglobalscope.rs `run_worker_script` 的 `define_all_exposed_interfaces` 之后):第一 drain 早于 worker 接口构造器定义,bao_stealth W1a typeof 守卫 JS hooks 全静默跳过(engine getters 不依赖接口故生效)——第二点重跑幂等 install 落全类 JS hooks(audio/webgl)(REQ-BRW-004 C15,用户裁决 2026-09-09 vendor patch)<br>**per-Worker injector 层**(REQ-BRW-004,用户裁决 2026-09-09 vendor patch,e43 多 Worker 缺口):`EMBEDDER_WORKER_SCOPE_INJECTORS` / `EMBEDDER_WORKER_INTERFACES_READY_INJECTORS`(`Arc<dyn Fn + Send + Sync>`,upsert per WebView,**非消费**——同页第 2+ `new Worker()` 各自交付,consume-once 队列只够第 1 个 Worker;此前第 2+ Worker 零注入=裸 Worker 可被指纹识别);`register_worker_{scope,interfaces_ready}_injector` + `unregister_worker_injectors`(页关闭清理);SW 的 S-family one-shot drain 不扩展到本层<br>**`bao_run_in_script_settings(cx, global, f)`**(BCE-20260910-004):对给定 global 的 realm 跑 `run_a_script`(servo 一切 JS 入口的 "prepare to run script" 封装——push settings-stack Entry + 空栈时 microtask checkpoint);bun_runtime `fire_js` 的 page-realm timer 派发经 `register_bao_settings_runner` 注册表借由此封装,缺它则回调里 `location.*`/`document.open()` 走 `entry_global().unwrap()` 空栈 panic(Script#3 meituan settings_stack.rs:36)<br>**RED-1 P-A realm-discard cancel 桥**(2026-09-10 用户裁决 P-A 直做):`BAO_REALM_DISCARD_CANCEL` OnceLock + `register_bao_realm_discard_cancel` + `bao_cancel_timers_for_discarded_realm(cx, global)`(镜像 pump 桥注册面,`*mut c_void` 双参解耦两 mozjs 实例);调用点在 `handle_exit_pipeline_msg`——`window_detached` 门 **之前**(同域导航 browsing context 已迁移时 `clear_js_runtime` 整段被跳过,hook 只挂那里会竞态漏 purge),两侧分支均覆盖——同域导航复用 ScriptThread 丢弃旧 realm 时,bao BAO_REGISTRY 里该 global 名下的 timers(setImmediate 链/node 段 timers)随 realm 终止(浏览器导航语义),否则 zombie 回调进被丢弃 realm 永续 + raw-root pin 旧 realm 无法 GC(每次同域导航累积,#29 nav churn 源之一)<br>**W15 shrink 钩子**(2026-09-29,soak 泄漏链收口/W10 联动,设计 /tmp/w15-servo-shrink-design.md §4 候选 A):`handle_exit_pipeline_msg` 尾部(三调用点共用收口,cx 在场线程正确)30s 时间窗节流的 `NonIncrementalGC(cx, GCOptions::Shrink, GCReason::API)`——死 realm 不可达后(RED-1 后置状态)其 chunk 内存仍驻留(API/Normal options 的 sweep tail 跳过 decommit,SM `shouldDecommit`);W10 已在 CLI/bench runtime 面落 Shrink,本钩子补 per-ScriptThread runtime 半边(W5/W6/W7 泄漏链)。节流=进程级 `static LAST_SHRINK_MS: AtomicU64` + `SHRINK_WINDOW_MS=30_000`(churn ~7.5 close/s,不节流=事件循环被背靠背全量收集串行化;进程级=每窗口全进程一次 shrink,谁先退出 pipeline 谁领取,CAS 单胜者)。回滚=删 hunk;调参只动常量<br>**new-document 注入层**(REQ-CDP-004,用户裁决 2026-10-01 W55 vendor realm entry injection):`EMBEDDER_NEW_DOCUMENT_SCRIPTS`(`Mutex<Vec<(WebViewId, u64, String)>>`,镜像 per-Worker injector 层——非消费、WebViewId 键控、同源同文重注册去重保序;**identifier 单源**:vendor 注册表自铸进程级单调 `EMBEDDER_NEW_DOCUMENT_SCRIPT_NEXT_ID`,register 返回该 id(同文同页重注册幂等返回同一 id),是 WS 注册面与 memory bridge 双 CDP 面共同返回给客户端的唯一 id 来源,`Page.removeScriptToEvaluateOnNewDocument` 按它注销)+ `register_embedder_new_document_script` / `unregister_embedder_new_document_script`(按 id 单删,页内作用域——他页持 id 不可删)+ `unregister_embedder_new_document_scripts`(整页清理;lib.rs 同 re-export);drain 点在 `load()` 的 ServoParser 启动块之前(两 window 创建臂合流点,`CurrentRealm::assert` 借已入场的 window auto realm 逐条 `evaluate_js_on_global`,失败 warn 续跑)——CDP `Page.addScriptToEvaluateOnNewDocument` 规范时点(新 document 建成后、任何页面脚本写入前),取代 bao 侧 FrameStartedLoading pump 分派(结构性迟到,0/40 NO-HARVEST);registry 级单测在 script_thread.rs 尾部 `embedder_new_document_script_registry_tests` |
| `script/engine/handle.rs`(2026-08-23 上游 b54baa327 移动后新家)| **Bao 补丁版 JSEngineSetup**:`JSEngineSetup(Option<JSEngine>)` 幂等 init(Ok→存 handle;AlreadyInitialized→`JSEngine::process_handle()` 优先 + JS_ENGINE spin 回退 50×1ms;AlreadyShutDown→None;其他 Err→panic)+ Drop engine-leak(`mem::forget`,不清 JS_ENGINE,多 BaoRuntime 生命周期)。上游版 handle.rs 是裸 `JSEngineSetup(JSEngine)`,重放禁用上游版 |
| `script/runtime/script_runtime.rs` | **`servo_build_id` 指针 bug 修复 + tag 统一**(2026-09-24,XDR 双缺陷):①上游 `servo_id[0] as *const c_char` 把字节值 `'S'`(0x53)当指针传给 `SetBuildId`——首个 `GetScriptTranscodingBuildId` 调用即 SIGSEGV 于 0x53(`_siginfo.si_addr` 实证);潜伏至 bao stencil XDR decode(`xdr_cache::load`→VersionCheck)成为首个调用方,崩掉全部到达 ScriptThread stealth 注入的浏览器 e2e。修复=`as_ptr()`。②`GetBuildId` 是进程级函数指针,浏览器进程双写者(本处 per-ScriptThread + bao_engine `bao_process_build_id` 懒装)——tag 必须字节一致否则 XDR 条目跨 id VersionCheck 失败=cache 抖动;本处镜像 `BUILD_ID_TAG = b"bao-stencil-xdr-1"`(canonical 在 `bao_engine/src/xdr_cache.rs`,双向注释锚定,改任一侧必须同步) |
| `components/shared/paint/rendering_context.rs`(2026-09-27,fork 自维护裁决后首条) | **WGL make-current 缺序修复**:上游 `SurfmanRenderingContext::new` 在 `create_context` 后立即加载 gleam/glow——surfman EGL/ANGLE 后端在 create_context 内保持 context current,但 WGL 后端以 `CurrentContextGuard` 恢复先前(null)context → `glGetString(GL_VERSION)` 无当前 context → glow panic(native.rs:69),每次页面创建必崩 + 泄漏 HGLRC/隐藏窗口累积至 nvoglv64 驱动 teardown AV。修复 = `device.make_context_current(&context)` 插在 GL 加载前(EGL/ANGLE 上安全 no-op)。bao Windows 面同时以 `no-wgl` feature 走 ANGLE/D3D11(嵌入者义务,镜像上游 servoshell),本补丁使 fork 的 WGL 路径亦正确(纵深防御) |
| `script/lib.rs` | 26-28 行:`pub use event_loop::script_thread::{register_embedder_callback, register_worker_scope_callback};`(Bao embedder 回调 re-export,上游同步合并时必保;C15 起追加 `register_worker_interfaces_ready_callback` 同 re-export;per-Worker 层起追加 `register_worker_{scope,interfaces_ready}_injector` / `unregister_worker_injectors` / `EmbedderWorkerInjector` 同 re-export;BCE-20260910-004 起追加 `bao_run_in_script_settings` 同 re-export;RED-1 P-A 起追加 `register_bao_realm_discard_cancel` / `BaoRealmDiscardCancel` 同 re-export;2026-10-01 起追加 `register_embedder_new_document_script` / `unregister_embedder_new_document_script` / `unregister_embedder_new_document_scripts` 同 re-export) |
| `script/dom/workers/dedicatedworkerglobalscope.rs` | worker-scope 回调 drain(2026-09-09 起按 `webview_id` per-worker 键控,6b3caa34 跨页串扰根治)+ clear_js_runtime 前 realm flush(UAF 防护)<br>one-shot drain 之后追加 **per-Worker injector 交付**(非消费层,同页第 2+ Worker 注入点,REQ-BRW-004,用户裁决 2026-09-09 vendor patch) |
| `script/dom/serviceworker/serviceworkerglobalscope.rs` | ServiceWorkerGlobalScope 接 WebViewId-keyed `drain_worker_scope_callbacks`(SW scope 的 stealth 注入点,REQ-BRW-004 C19 S1,f77faf8b)<br>**SW scope 两点 per-Worker injector 交付**(非消费层,SW 注入饥饿根治:同页先建 DedicatedWorker 耗尽 consume-once 队列后,后注册 SW 靠 injector 层仍获完整注入——① one-shot drain 后 `worker_scope_injectors`(engine getters)② SW 自己的 `define_all_exposed_interfaces` 之后 `worker_interfaces_ready_injectors`(W1a JS hooks;SW 不走 `on_complete`,无此点则 JS hooks 在 SW realm 永不落地),REQ-BRW-004,用户裁决 2026-09-09 vendor patch)<br>Response(mediator) 分支重写为 `FetchEvent::handle_mediator` 真 FetchEvent 管线(替代上游裸 Event TODO)+ `onfetch` event_handler(REQ-BRW-004 C19 S2a,用户裁决 2026-09-09 vendor patch)<br>`new_script_pair()`(镜像 SharedWorker 形态,`unbounded()` 新通道)——SW realm 同步 DOM API 通道载体(REQ-BRW-004 C19,用户裁决 2026-09-09 vendor patch)<br>**`owning_webview_id` 字段 + accessor**(R53-A,2026-09-10):从 `ScopeThings.webview_id` 捕获注册页身份存于 SW scope(#[no_trace] Option<WebViewId>),`GlobalScope::egress_webview_id` 消费——SW egress 归属宿主页 stealth wire profile 的身份载体 |
| `script/dom/serviceworker/fetchevent.rs`(新增,上游无此文件)+ `script_bindings/webidls/FetchEvent.webidl`(新增)| FetchEvent DOM 类型(request/respondWith/waitUntil 全实现;respondWith 单次 InvalidStateError 门);`handle_mediator`:构造 Request → dispatch 受信 fetch 事件 → PromiseNativeHandler 在 SW 事件循环异步 settle → 读 status/headers/body → `CustomResponse::new` → `response_chan.send(Some)`,未调用/rejected/非 Response → pass-through `None`(dom/serviceworker/mod.rs 注册 + ServiceWorkerGlobalScope.webidl `onfetch` 解注释,REQ-BRW-004 C19 S2a,用户裁决 2026-09-09 vendor patch) |
| `script/dom/workers/workerglobalscope.rs` | 存 `init.webgl_chan`(原 new_inherited 丢弃)+ accessor——worker 继承父 Window 的 WebGL 通道,OffscreenCanvas WebGL1 worker 通路载体(REQ-BRW-004 C14,用户裁决 2026-09-09 vendor patch)<br>`new_script_pair` 补第三臂 `downcast::<ServiceWorkerGlobalScope>()`(上游 `panic!` TODO,SW 线程同步 XHR 即死),尾 else 改 `unreachable!`(REQ-BRW-004 C19,用户裁决 2026-09-09 vendor patch)<br>`run_worker_script` 在 `define_all_exposed_interfaces` 之后 drain 第二 embedder 回调点(worker 全部 WebIDL 接口已定义、worker 脚本未跑——bao_stealth JS hooks 落点,REQ-BRW-004 C15,用户裁决 2026-09-09 vendor patch)<br>第二 one-shot drain 之后追加 **per-Worker injector 交付**(非消费层;`on_complete` 不在 SW 路径上——SW 有自己的 define+drain,S-family 语义不动,REQ-BRW-004,用户裁决 2026-09-09 vendor patch) |
| `script/messaging.rs` | `ScriptEventLoopReceiver` 补 `ServiceWorker(Receiver<ServiceWorkerScriptMsg>)` 变体 + `recv()` 臂(镜像 SharedWorker 臂;sender 侧 `ScriptEventLoopSender::ServiceWorker` 上游已有),使 SW realm 同步 DOM API(sync XHR)的 new_script_pair 通道闭合(REQ-BRW-004 C19,用户裁决 2026-09-09 vendor patch) |
| `script/dom/xmlhttprequest/xmlhttprequest.rs` | sync XHR 嵌套泵 fail-closed:`script_port.recv()` 返回 Err(sync task-source sender 被无终态 drop)时不再 `unwrap()` panic 整个 SW/worker 线程,改走 XHR spec 网络错误臂(`Error::Network(None)`,status=0 可观测)(R1, 2026-09-10)<br>**R53-A**:Request 构造改用 `global.egress_webview_id()`(SW realm XHR egress 归属注册页,同 fetch 路径) |
| `script/dom/fetch/request.rs` | **R53-A**(2026-09-10 用户裁决,net 面):`net_request_from_global` 的 RequestBuilder 身份改用 `global.egress_webview_id()`——SW realm `fetch()`/`new Request()` 的 net Request 带 `target_webview_id=注册页`(此前恒 None→落进程全局 fallback,多页不同 profile 时 SW egress 被最后安装页污染);Window/DedicatedWorker/SharedWorker 身份不变 |
| `script/dom/webgl/webglrenderingcontext.rs` | `new_inherited` 解 Window 锚定(收 `&GlobalScope`,`webgl_chan_from_global` helper 按 Window/WorkerGlobalScope 分派 + `new_in_worker` 入口);`mark_as_dirty` 的 XR 检查 Window 降级(原 `as_window()` 对 worker 首次 draw 即 panic);`GetShaderPrecisionFormat` 反射锚 `as_window()` → 所在 global(原 worker 侧 panic)(REQ-BRW-004 C14,用户裁决 2026-09-09 vendor patch) |
| `script/dom/webgl/webgl2renderingcontext.rs` | `new_inherited` 解 Window 锚定(收 `&GlobalScope`,base 创建按 Window/Worker 分派)+ `new_in_worker` worker-realm 入口(W3a 同形态,REQ-BRW-004 C14 W3b,用户裁决 2026-09-09 vendor patch) |
| `script/dom/webgl/webglshaderprecisionformat.rs` | `new` 收 `&GlobalScope`(原 `&Window`)——worker 侧 getShaderPrecisionFormat 反射载体(W3b 连带) |
| `script/dom/canvas/offscreencanvas.rs` | `get_or_init_webgl_context` / `get_or_init_webgl2_context` 均按 global 类型分派(Window 旧路 fire `webglcontextcreationerror` / Worker 新路 `new_in_worker`;原 Window downcast 对 worker 恒 null)(REQ-BRW-004 C14,用户裁决 2026-09-09 vendor patch;WebGL2 分派为 W3b) |
| `script_bindings/webidls/WebGL2RenderingContext.webidl` | `Exposed=Window` → `(Window,Worker)`:worker 侧 WebGL2 原型方法组(readPixels 等 mixin 成员随 includer 暴露)原在 worker realm 不定义(REQ-BRW-004 C14 W3b,用户裁决 2026-09-09 vendor patch) |
| `script_bindings/webidls/WebGL{Query,Sampler,Sync,TransformFeedback,VertexArrayObject}.webidl`(5 个) | `Exposed=Window` → `(Window,Worker)`(保 `Pref="dom_webgl2_enabled"`):WebIDL parser 强制方法返回类型须在方法暴露处可见——WebGL2 方法返回这些 Window-only 辅助接口,不改则 codegen 拒绝(同 W3b 连带) |
| `script_bindings/webidls/{OfflineAudioContext,BaseAudioContext,AudioBuffer,AudioNode,AudioParam,AudioDestinationNode,AudioScheduledSourceNode,OscillatorNode,GainNode,AudioBufferSourceNode,OfflineAudioCompletionEvent,AudioContext}.webidl`(12 个) | **worker 侧 Audio 栈暴露**(REQ-BRW-004 C15,用户裁决 2026-09-09 vendor patch——接受 Bao 独有暴露特征风险):`Exposed=Window` → `(Window,Worker)`;BaseAudioContext 上 9 个返回 Window-only 节点类型的成员加成员级 `[Exposed=Window, Throws]` 门(listener + createConstantSource/createAnalyser/createBiquadFilter/createIIRFilter/createPanner/createStereoPanner/createChannelSplitter/createChannelMerger);**实时 AudioContext 构造器亦 worker 暴露**(criterion 字面「worker 内 AudioContext」补全:渲染在 servo-media 自有 AudioRenderThread,sink 在 render 线程构造,调用线程仅通道控制+有界 init 握手,零 Window 依赖;4 个 createMedia* 成员加成员级 `[Exposed=Window, Throws]` 门——HTMLMediaElement/MediaStream 族 Window-only 类型引用);AnalyserNode/AudioListener 族保持 Window-only;AudioDestinationNode/AudioScheduledSourceNode/OfflineAudioCompletionEvent 为类型可见性/继承/complete 事件连带 |
| `script/dom/audio/*.rs`(14 文件) | C15 签名解 Window 锚(REQ-BRW-004 C15,用户裁决 2026-09-09 vendor patch):9 文件(offlineaudiocontext/baseaudiocontext/audiobuffer/audioparam/oscillatornode/gainnode/audiobuffersourcenode/offlineaudiocompletionevent/audiocontext)构造器/工厂 `&Window` → `&GlobalScope`(`pipeline_id()` 取自 global、`as_window()` 锚移除,audiodestinationnode 树内本就 `&GlobalScope`;audiocontext 的 createMedia* 保持 `as_window()` 下传——成员级门内 Window realm 专用);5 个 Window-only 节点文件(audiolistener/panner/biquadfilter/stereopannernode/constantsourcenode,21 处)`AudioParam::new` 实参改 `window.upcast::<GlobalScope>()` |
| `canvas/canvas_paint_thread.rs` | `CanvasCommand::GetImageData` handler 接 canvas 噪声(seed=0 字节零 diff 硬保证;单咽喉覆盖 convertToBlob/transferToImageBitmap/createImageBitmap/texImage2D/createPattern,REQ-BRW-004 C13 W2,6bcf30af)。**R53-A 二阶段(BUN-EVOLUTION,2026-09-11)**:`canvas_webviews` map(CanvasId→WebViewId,创建链 stamp、Destroy 同步清除),GetImageData 噪声按 canvas 归属 webview 查 keyed 注册表(命中权威,显式 disabled=零噪声;miss/无身份→进程全局 fallback,pre-R53 语义)——同 runtime 双页不同 profile 的 canvas 噪声不再互相覆写,stealth-free 页读回字节精确 |
| `canvas/canvas_noise.rs`(Bao 新增,上游无此文件) | `CanvasNoiseConfig` 确定性噪声算法 + 全局 seed set/get(`set_canvas_noise_seed` 的消费载体;W2 起被 paint 线程读取)。**R53-A 二阶段**:per-WebViewId 注册表 `CANVAS_NOISE_BY_WEBVIEW`(`set/clear_canvas_noise_for_webview` + `canvas_noise_for_webview`:keyed 命中权威含显式 None=stealth-free 零噪声,miss→进程全局 fallback;三原子保留为 fallback bucket) |
| canvas 身份链 4 文件:`shared/canvas/lib.rs` + `shared/constellation/from_script_message.rs` + `constellation/constellation.rs`(Create relay) + `script/dom/canvas/2d/canvas_state.rs` | **CanvasId 创建链携带 webview 身份**(R53-A 二阶段,2026-09-11):`ConstellationCanvasMsg::Create` 与 `ScriptToConstellationMessage::CreateCanvasPaintThread` 增 `Option<WebViewId>`;`CanvasState::new` stamp `global.egress_webview_id()`(worker/SW realm→宿主页);constellation 纯 relay 不解释 |
| `script_bindings/lock.rs` | ThreadUnsafeOnceLock 等(Bao 扩展) |
| `shared/base/id.rs` | Bao ID 类型(+ AtomicOptionScrollTreeNodeId 从上游增补) |
| `shared/base/lib.rs` + `ipc_router.rs` | per-instance RouterProxy(BCE-20260628-002) |
| `shared/net/lib.rs` | ipc_router 路由 + per-instance FetchThread + ProcessContentLength 等上游增补合并 |
| `shared/script/lib.rs` | ScriptThreadInit.router_proxy 字段(BCE-20260627-009) |
| `constellation/constellation.rs` + `event_loop.rs` | per-Constellation RouterProxy 全生命周期 |
| `net/connector.rs` + `websocket_loader.rs` 等 | **boringssl stealth TLS connector**(JA3/JA4 全面:cipher/curves/sigalgs 重排 + ALPN + H2 SETTINGS,REQ-STL-001;上游 rustls 迁移被回滚)<br>**R53-A net 面 per-WebViewId stealth wire 注册表**(2026-09-10 用户裁决 R53 方案 A 一阶段):`STEALTH_TLS_BY_WEBVIEW`/`STEALTH_H2_BY_WEBVIEW`(LazyLock RwLock<HashMap<WebViewId, Option<…>>>,显式 None=stealth-free 页不落 fallback)、`set/clear_stealth_wire_config_for_webview`(upsert/页关闭清理)、`resolve_stealth_{tls_config,http2_fingerprint}(Option<WebViewId>)`(keyed 命中权威,miss/无身份→进程全局 fallback——SW script update 等无身份 infra fetch 保持旧行为);`create_tls_config` 增 `webview_id` 参(WS 路径);进程级 `set_stealth_tls_config`/`GLOBAL_HTTP2_FINGERPRINT` 保留为 fallback 真源(既有 getter 语义零 break) |
| `net/fetch/bun_bridge.rs` | Bao 新增文件(上游无;U2 page-network 统一):`obtain_response_bun`——页面网络唯一 HTTP 交换路径(bun HTTPThread + stealth SSLConfig,redirect/CORS/cache/HSTS/cookies 留 servo 侧;方法/头/流式 body/devtools 消息全 parity)。**R53-A**:`obtain_response_bun` 增 `target_webview_id` 参,wire config 与 h2 fingerprint 均按 per-WebViewId 解析(`crate::connector::resolve_*`;SSLConfig intern 注册表内容不同→指针不同→连接池自动按 profile 分桶) |
| `script/dom/indexeddb/idbtransaction.rs` | **IDB 事务双终局门**(BCE-20260910-004b,上游缺陷):`send_abort_notification` / `send_complete_notification` 两个终局任务体顶部补 `finished` 复查——上游只在入队时(`finalize_abort`/`finalize_commit`)检查 `finished`,backend commit-Ok 与 abort ack 竞态可双双入队;先跑的 body 清 upgrade transaction 并置 finished,后跑的 body 再清 `db.upgrade_transaction`(已 None)→ `clear_upgrade_transaction` 的 `expect` panic(idbdatabase.rs,Script#3 meituan WAF IDB 探测每次触发)。按 IndexedDB §transaction-lifetime:finished 事务保持 finished,迟到终局=幂等 no-op(上游 issue 候选) |
| **W28 三文件**(2026-09-29):`script/dom/document/document.rs` + `script/dom/css/fontfaceset.rs` + `script/dom/promise/promise.rs` | **font-ready 站点 discard 守卫 + RootedPromise root-slot 读**(W28,与 W15 shrink 钩子交互回归):①document.rs `maybe_fulfill_font_ready_promise` 入口 `bao_is_realm_discarded`(B-family 第 10 站,ISSUE #25 形态镜像)——documents map 保留 pump-deferred 的已 discard 文档,其 FontFaceSet deref 在 W15 的 `NonIncrementalGC(GCOptions::Shrink)` 压缩重定位后读到搬走前的旧地址(`waiting_to_fullfill_promise → promise_obj → IsPromiseObject` 于 freed cell,Shape::getObjectClass SIGSEGV);②fontfaceset.rs `waiting_to_fullfill_promise` 改走 `ready_promise_pin.is_fulfilled_from_root()`(ae61880a pin 只保 collection-liveness;注册 root 槽(`AddRawValueRoot`)才是唯一会被 GC 重定位更新的地址——reflector 副本在 JS 堆不可达的 wrapper 里既不 mark 也不搬移更新;纵深防御,兼护 window.rs 的无守卫 caller);③promise.rs 新增 `RootedPromise::is_fulfilled_from_root`(从注册槽读 PromiseState,`GetPromiseState` 纯读无 GC)。A/B 实证:守卫单独即绿(load-bearing),root-slot 读为防御层。同族教训:凡 pump 面对 stored promise 的 deref,地址必须来自 GC 注册槽或先过 discard 探针 |
| `script_bindings/reflector.rs`(2026-09-29,W30) | **Reflector::trace 补自指槽追踪**(W30,压缩搬移盲区类根修):上游终态的 `Traceable for Reflector` 是**空 trace**——`object: Heap<*mut JSObject>` 回指针从不参与搬移更新(上游 libservo 嵌入从不对 script-thread runtime 跑压缩收集,缺陷潜伏)。W15 shrink 是首个压缩收集:被搬移的 DOM wrapper 回指针全部 stale,首个炸点=finalize 记账面 `finalize_common → drop_memory → RemoveAssociatedMemory(reflector 槽) → zoneFromAnyThread` 于已 decommit 的源 chunk(media_e2e XHR SIGSEGV,gdb 实证:被 sweep 的 cell 本体可读、槽值指向零页 chunk)。修复=trace 该槽(`trace_object`/`CallObjectTracer`,MovingTracer 随搬移更新;自指边 mark 幂等安全;null 槽跳过)。**类纪律:任何持有 GC 指针槽的 native 结构,trace 不得为空,否则首个压缩 GC 即盲区**。与 W28 的关系:W28=不可达 wrapper 变体(trace 根本不跑,靠注册槽读+discard 守卫);W30=可达 wrapper 变体(trace 可跑,补上即根修)。横扫:DomRoot 空 trace=设计使然(already traced 无自身槽),`unsafe_no_jsmanaged_fields` 类型无 GC 持有,drop_memory 3 调用点(finalize×2+windowproxy)全部经此槽 → 单点根修全覆盖 |
| `script_bindings/codegen/codegen.py`(2026-10-03) | **回调接口属性查找 Get-抛掷 Chromium 形对齐**(wptserve 载具波分歧格终裁,用户裁决 2026-10-03):`CallbackOperationBase.getCallableDecl` 单操作臂的操作属性查找(`get_callable_property`,即 "handleEvent" 系)由裸 `?` 传播改三臂 match——`Err(JSFailed)`(查找本身抛 JS 异常,如 revoked proxy 的 Get)→ **清 SM pending exception(`JS_ClearPendingException`)+ `return Ok(Default::default())` 静默成功 no-op**(=Chromium/Blink 形:Call 前全吞、零 error 事件零 report、CDP exceptionThrown/console 全零);`Err(e)` 照旧传播(not-callable 的 `Error::Type` 系 Rust 侧错误、无 SM pending 异常、报告面天然静默——与 Chromium 双侧同形,cells 1/2 零变化)。**依据**:WPT `dom/events/EventListener-handleEvent-cross-realm.html`「non-callable revoked Proxy」格双引擎三角实测——bao 抛+报 iframe realm TypeError(=DOM dispatch report-the-exception 的 spec 字面),Chromium 真静默;反指纹宪法裁决 A(2026-10-02)同构延伸终裁(2026-10-03,分止单格=探测向量:页面构造 revoked-proxy listener 观察 error 事件即可区分引擎;spec 正确性让位不可区分性)。**零触碰面**:Call 抛掷路径(cells 3/5 照报面,`ExceptionHandling::Report` + `call_setup` 报告到 callback object 的 global 原样)、`get_callable_property` 本体(全体回调接口共享)、多操作臂、TreatNonObjectAsNull 守卫臂。**范围**:全部单操作回调接口(在树三者 EventListener/NodeFilter/XPathNSResolver——同经 Blink CallbackInterfaceBase invoke 层,Chromium 形态一致;EventListener 之外为结构同形推定);`Ok(Default::default())` 要求操作成功型 impl Default(在树三者 ()/u16/Option<DOMString> 全满足;未来非 Default 返回型在此编译期响亮失败,故意设计)。**反向跟进条款(吸收波必读):若未来 Blink 开始 report 该 Get 抛掷(spec report-the-exception 方向修正),撤此吞臂重新对齐**;上游 issue 候选已登记(Blink callback-interface Get-throw 被 TryCatch 吞不 report;该 WPT 文件 Chromium 自身 2/5 红)。证据:`/tmp/incv/revoked_proxy_cell4_probe.py` 双引擎逐格表 + `compat/web/WPT-FIRST-RUN.md` 2026-10-03 专节 |
| `components/servo/servo.rs`(2026-09-29,W27) | **ServoInner::drop join spin 有界化**(W27,liveness 硬化):上游 `Drop for ServoInner` 的 `while spin_event_loop() { sleep(500µs) }` **无超时**——死掉/悬挂的 servo 线程(无法处理 Exit 消息完成 join)使 teardown 无限挂死(W16 >13min 实测;**考古修正:该次挂死的根因是 W28 font-promise panic 杀死 Script#2——死线程永不 join;W26/W28 根治两类 panic 后 hanging-fetch 场景 0.17s 完成 teardown,「in-flight fetch 悬挂 teardown」假设被证伪**)。修复=15s(仓库有界等待惯例)deadline 后放弃 spin、**泄漏该线程**(log::error 如实记录,禁 panic)+继续 teardown(liveness 优先于完美回收)。机制验证:timeout=0 A/B 下全部 22 测试绿(每次 drop 第 1 轮即走 break 路径,无下游不稳定)。配套 liveness pin 测试(`shutdown_drop_runtime_with_hanging_fetch_is_bounded`,HangingFixture 永不应答载体)把未来任何 wedge 类缺陷从「无限挂死 suite」转化为「45s 内有界红灯」。横扫:servo.rs 其余 join(`run_content_process` 1395-1406)属沙箱子进程嵌入路径(bao 不走),未触碰 |
| `Cargo.toml` + `components/fonts/Cargo.toml` | **freetype-sys req 范围化 + harfbuzz freetype 死激活移除**(issue #43,GPUI 同二进制解锁):workspace `freetype-sys = "0.23"` → `">=0.20.1, <0.24"`;fonts Linux/Android/freebsd 腿 `harfbuzz-sys` 去 `"freetype"` feature(servo shaper 零 `hb_ft_*` 消费——死激活;harfbuzz-sys 0.8 该 feature 强制 freetype-sys ^0.23)。遗留已闭环(E17):`vendor/freetype-wrapper/`=registry freetype 0.8.0 vendored(src 零改动,symbol 对照:wrapper 自带 bindgen dump、Rust 层零 `freetype_sys::` 消费、77 native fn 在 0.20.1/0.23.0 同版 bundled freetype2 2.13.2 全导出),freetype-sys req `">=0.20.1, <0.24"`;根 Cargo.toml 与 vendor/servo/Cargo.toml 双 workspace `[patch.crates-io] freetype`(双侧 lock 均解析该链,patch 按 workspace 生效)。fonts android/ohos 腿终态=**整段删除(用户裁决 2026-09-20 终裁,推翻「保留无 bundled 引用」代裁;删除态已经 E18 全链独立验证:pin RC=0+check 8m31s RC=0,恢复态另由 E19 对照)**。build.rs 一手事实(registry 双版全文核对):0.20.1 android/ohos 无条件 fall-through 源码编译(零 feature);0.23.0 非 bundled 移动端 build.rs 跳过 pkg-config 后**空 return——零源码编译零链接配置**(依赖 NDK 系统 freetype,默认无→慢性链接失败),bundled 是 0.23 移动端源码编译唯一通路(初版"两轨均无条件源码编译/行为冗余"系误传 C 的概括,已更正)。**边界(机制级,worktree 实证):fonts 的 `bundled_freetype = ["freetype-sys/bundled"]` 映射仅凭存在即把 workspace lock 钉死 0.23**——cargo lock 版本选择按全图特征边并集验证、与激活无关(清空映射后 `cargo update --precise 0.20.1` 原生通过;保留则恒拒,default-members/fonts 单包宇宙同样拒绝),故 **0.20 轨落地必须清空该映射**(fonts 单行;`components/servo` 的 bundled/bundled_freetype def 可原样保留,激活链最后一跳为空=两轨均 no-op,bao 实际构建从不激活,零行为影响)。当前主树按裁决字母态保留 def、lock 恒 0.23,清空+翻 0.20.1 待批准;上游 freetype-sys 0.20.x 补 bundled 为 upstream-issue 候选。gpui probe 终验:RED(无 patch)精确复现 issue #43 links="freetype" 双版本冲突 → GREEN(带 patch)freetype-sys 全图唯一 0.20.1,wrapper/wr_glyph_rasterizer/zed-font-kit 三件 0.20.1 编译全绿。**0.20 轨全链终验(E18,2026-09-20,worktree /tmp/bao-ft020c 套全树未 commit diff)**:根图(servo default-features=false、fonts 的 bundled_freetype 映射在位不清空)`cargo update -p freetype-sys --precise 0.20.1` 原生通过(Downgrading 0.23.0→0.20.1,零拒绝)→ `cargo check -p bao-servo --jobs 4` Finished dev 8m31s **RC=0**(freetype-sys 0.20.1 C 编译,wrapper/wr_glyph_rasterizer/webrender/bao-servo-fonts/bao-servo-script/bao-servo 全链 0.20.1 下编译绿;唯一新 warning=fonts freetype_face.rs:196 unnecessary `unsafe`,0.20.1 宏形态所致,非错误)→ lock 断言全过(全图唯一 freetype-sys 0.20.1、freetype 0.8.0 零 source/checksum=path wrapper、零 patch.unused)。宿主 pkg-config freetype2 26.1.20 在位:0.20.1 与 0.23 根图同走 pkg-config 系统链接,轨间桌面 provenance 零漂移。**「保留则恒拒」适用域修正(E18 实证)**:根图映射在位即 pin 成功(本终验两次复现:主树 dry-run+worktree 真 pin),「恒拒」仅在 vendor/servo 自身 default-feature 解析成立(bundled 激活链真激活 freetype-sys/bundled)。本图回归:主树 `cargo check -p bao-servo --jobs 4` Finished 6m49s **RC=0**、lock freetype-sys 恒 0.23.0(行为零变化)、`cargo test -p bao-servo-fonts --jobs 4 -- --test-threads=1` **6/6**(font_context 4+font 1+font_template 1;lib 内 2 test 为 ohos-gated 不计)。腿删除语义:ohos manifest 期零 freetype-sys 依赖(freetype 模块 cfg 本不含 ohos);android 经共享 linux/android/freebsd 腿仍持非 bundled freetype-sys——0.20 轨恒源码编译可用、0.23 轨链接期响亮失败。恢复条件=平台矩阵承诺 android 时恢复 target 段(0.23 轨需 bundled,或上游 0.20.x 补 feature 后按 0.20 轨)。**MW Desktop 接线**:mw-ai-platform apps/desktop(gpui-pre 0.3.5/freetype-sys ^0.20)与 bao 链全图唯一 freetype-sys 共存已解锁,`src/tools/bao_webview_real.rs` 可由 mock(`bao = []` 空 feature)切真实链接(T14 集成) |

| 渲染面(2026-10-02,headless 帧生产契约) | **embedder 泵循环帧契约**:GL composite(`WebView::paint`)是渲染管线唯一心跳(rAF tick 与截图 capture 都只在 `Painter::render` 内推进),三泵循环(run/pump_cdp/run_with_bridge)每迭代经 `PagePool::paint_pages_needing_repaint()` 排空 `repaint_pending` latch;**禁以"零 GL 循环"名义全禁 composite**(BCE-20260910-003 前提已被 2026-10-02 推翻——全禁=boot-once 帧+截图死通道 #40);phase 面包屑 `paint:composite` 供 #40 watchdog 归因 |
| `script/dom/console.rs` | **describe_scripted_caller_safe 恢复**(ISSUE #29/W12-B,2026-09-29):7045fa57 协调大波把 build_message 的 caller 探测换成裸 `describe_scripted_caller`,opt 档 FrameIter::settleOnActivation 对 CDP 包装器 eval 帧态 SIGSEGV(FrameIter.cpp:326)——恢复 safe 变体(wrappers2 路径,失败回退默认值) |
| `script/dom/security/csp.rs`(2026-10-04) | **compute_scripted_caller_source_position 切 describe_scripted_caller_safe**(W37,https 导航进程 SIGSEGV 根治):bao vendor 滞留 deprecated 裸调用 `describe_scripted_caller(&*cx as *const _ as *mut _)`——cast 链传 Rust `js::context::JSContext` wrapper 地址而非 raw 指针,SM 读 wrapper+0xb0 的 realm_ 得邻接栈垃圾(实证 -1)→ jsapi.cpp:4952 SIGSEGV;触发面=CSP 违规上报任务(report_csp_violations source_position=None 分支),与 IDN/https 无关(plain http+CSP header 即复现);上游 9fc8f7389 已是 safe 形——absorb 对账缺口回填,console.rs W12-B 同形;deprecated 裸调用横扫零残留 |
| **SW 生命周期波**(2026-10-04,REQ-BRW-004 lifecycle,上游同步逐文件重放):`shared/constellation/from_script_message.rs` + `script/dom/serviceworker/{serviceworkerglobalscope,serviceworker_manager,serviceworkercontainer,serviceworker,serviceworkerregistration}.rs` + `script/dom/globalscope/globalscope.rs` + `vendor/stylo_atoms/static_atoms.txt`(sorted 位 +updatefound;发布时 registry stylo_atoms 无此 atom,同 WebVTT 先例 published 脚本需动态替换) | **Update Worker State / Update Registration State 落地**(SW 注册挂起根治,WPT https 面 TIMEOUT 主桶):DOM ServiceWorker.state 构造后永钉 Installing、零 statechange/updatefound 事件源(上游 TODO 2.7/2.8/2.2 + Update Worker State 全缺,origin/main 实查未修→自写必要,全部文件已是 BAO patch 区)。修复=三跳最小中继:①SW 线程 dispatch_activate 后发 `ServiceWorkerMsg::WorkerEvaluated{worker_id}`(脚本加载失败早退不上报=诚实不激活);②manager 回发 `UpdateFound{registration_id}`(**必须在 ResolvePromise 之后**,FIFO 保序=wait_for_update 次序前提)+ `WorkerActivated{worker_id}`;③container 两新臂**不消费 pending_algorithm_results 队列**,按 id 查 DOM 对象 fire updatefound / update_state(Activated)+statechange;④TODO 2.7/2.8 物化 waiting/active(DOM 对象)。诚实线:membership 来自 manager 槽位,state 只来自 worker 线程证据,物化时不推断。20 条样本 6/20 转 OK+10/10 setup 子测试 PASS;残留 14 条挂点全越过生命周期,落在 DEC-WK-008 拦截排除域,未触碰 |
| **SM153 servo 消费面(11 文件,d5d82b6d+bc675a71,2026-09-21 前移波)** | engine 面:`script/dom/workers/script_runtime.rs`(JobQueue traps→引擎自有微任务队列:EnqueueMicroTask 私有值+checkpoint 全量 drain+traceNonGCThingMicroTask 真 trace;enqueuePromiseJob/empty/HOST_DEFINED_DATA 包装类全删——上游 PR #47489 形态移植)+`microtask.rs`+`script/script_module.rs`+`script/module_loading.rs`(三 hook 归一 SetModuleLoadHook;GraphLoadingState/Payload 走访器删除——GetRequestedModulesCount/Specifier API 被 SM 移除;ImportRequest 四 RootedTraceableBox 跨 fetch 续延;ModuleType CSS/Bytes/Text 按 Unknown 拒绝=140 平价)+`script/bindings/error.rs`(borrowed_error_report fork 自带形,零 jsglue shim)。机械站:`script/dom/structuredclone.rs`(WriteUint32Pair→Unchecked×3)+`script/dom/utils.rs`(extractExceptionInfo=None)+`script/bindings/principals.rs`(isSystem/Addon 拆分,全 false 平价)+`script/bindings/codegen/`/`buffer_source.rs`/`console.rs`/`keyframeeffect.rs`(&mut 包装形/for_of 双参/ToNumber) |
| **Web 平台波(2026-09-27,REQ-BRW-046/047,上游同步时逐文件重放)** | **SVG 几何(REQ-BRW-046)**:`script_bindings/webidls/SVGGraphicsElement.webidl`+`SVGGeometryElement.webidl`(解注 getBBox/getCTM/getScreenCTM/getTotalLength/getPointAtLength+SVGBoundingBoxOptions,optional dict 补 `= {}` fork codegen 形态)、`script/dom/svg/svg_geometry.rs`(新;kurbo 0.13.1 BezPath::from_svg/ParamCurveArclen/PathSeg::inv_arclen/Affine + svgtypes 0.16.1 三解析面;script/Cargo.toml 增 kurbo/svgtypes);fork codegen 画像=typeNeedsCx stub→方法 trait 无 cx 参、`Finite<f32>` 包装、CSSFloat=f32,DOM 构造经 `svg_geometry::current_cx()`(JSContext::get_from_thread)。**WebVTT(REQ-BRW-047,打包吸收+渲染自写)**:`components/webvtt/`(lib.rs+collectors.rs 重构形态)、`script/dom/webvtt/` 五件(GetActiveCues 真身/cue-order Ord/GetCueAsHTML 全量 construction rules)、`htmltrackelement.rs`(URL 变更全语义+FetchCanceller)、`htmlmediaelement.rs`(time_marches_on 完整算法+**queue_throttled_timeupdate() 保活锚**——上游无 track 早退会压掉 fork 无条件节流 timeupdate,media_e2e 回归根因)、`htmlvideoelement.rs`;渲染:`shared/layout/lib.rs`(WebVttCueBoxData 纯数据快照,零指纹面)+`layout/webvtt_cue_overlay.rs`(新;CSS WebVTT 几何)+layout 四文件挂点(ImageFragment.cue_overlays/display_list push_text);crown 纪元形态适配点=TextTrackList 平面 trait 签名+Vec<u8> payload+microtask import(上游 no_gc/Bytes/job_queue 形态禁直搬)。**拓扑件**:`vendor/stylo_atoms/`(0.20.0 代码零改动+static_atoms.txt append 3 atom enter/exit/cuechange,主根 [patch.crates-io])+`vendor/servo/tests/wpt/.../track-element/resources/`(15 个 .vtt fixture 64K byte-identical,crate tests include_str! 依赖);ADAPT 边界:play promises/WeakRef/Vec<u8> 保持 fork 形态 |
`components/servo/lib.rs` 另有 Bao embedder API 面(register_script_thread_callback / register_worker_scope_callback / register_worker_interfaces_ready_callback(C15 第二 drain 点) / set_canvas_noise_seed / set/clear_canvas_noise_for_webview(R53-A 二阶段 keyed) / set_stealth_tls_config / bao_run_in_script_settings(BCE-20260910-004 settings-stack 借用面) / set_webviewless_resource_handler(BCE-20260910-002) / register_bao_realm_discard_cancel(RED-1 P-A realm-discard timer cancel 桥,2026-09-10) / register/unregister_embedder_new_document_script(s)(REQ-CDP-004 new-document 注入层,2026-10-01;register 返回 vendor 自铸 identifier,单删按 id))。`config/prefs.rs`、`config/opts.rs`、`allocator/`、`net/async_runtime.rs` 有小 patch。

## 复用映射(Phase 1 关键)

| 功能 | 复用 crate | 替代手写代码 |
|------|-----------|-------------|
| 模块解析 | `bun_resolver` | 手写 `resolve_specifier` / `resolve_node_modules` |
| 事件循环 | `bun_event_loop` + `bao_uloop` | 手写 `JobQueue::drain` + `thread::sleep` 轮询 |
| HTTP 服务/客户端 | `bun_http` + `bun_uws` + `bun_picohttp` | 手写 `std::net::TcpListener` + HTTP 解析 |
| URL 解析 | `bun_url` | 手写 URL 拆分 |
| Base64 | `bun_base64` | 手写 `base64_encode` |
| I/O 抽象 | `bun_io` | 直接 `std::fs` 同步调用 |
| 进程管理 | `bun_spawn` | 缺失 `Bun.spawn()` |
| 路由 | `bun_router` | — |
| DNS | `bun_dns` | — |
| 事件循环定时器 | `bun_event_loop` + uSockets timer | 手写 `TimerHeap` + `thread::sleep` |
| TS 转译 | `bun_transpiler` | — |
| 文件监听 | `bun_watcher` | — |
| Node.js polyfill | `node-fallbacks` | 手写 `node:fs/path/crypto/http` |
| 字符串处理 | `bun_string_encoding` | — |
| 线程工具 | `bun_threading` | — |
| 系统工具 | `bun_sys` | — |
| 数据结构 | `bun_collections` | — |

## 上游项目参考

| 项目 | 路径 | 参考价值 |
|------|------|---------|
| Bun | `~/code/rust/bun/src/` | ~85 个纯 Rust crate(零修改复用);`jsc/` 是 JSC→SM 迁移目标;`runtime/` 是 Bun API 实现来源 |
| Bun SPEC | `~/code/rust/bun/CLAUDE.md` | 构建命令、测试规范、crate 组织 |
| Servo | `~/code/tools/servo/`(vendor 快照见 `vendor/servo/`,2026-08-13 上游 HEAD,10 个 Bao 定制文件见上文清单) | `libservo` 嵌入入口;`script/` DOM(每 ScriptThread 一个 thread-local SM JSContext);`script_bindings/` SM↔DOM 桥接 |
| mozjs | `vendor/mozjs/`(SM 153.3.0esr / bao-mozjs 0.24.0 / bao-mozjs-sys 153.3.0-0,2026-09-21 前移波 em1 系列;extracted-crates 12 件 in-tree 同源提取;7 项 BAO patch 见上文清单;单宇宙吸收后无独立 workspace——8d4c8260) | SM FFI 绑定源码 |
| blitz | `~/code/rust/blitz/` | DioxusLabs 模块化浏览器参考架构 |

Bun / Servo SPEC 测绘成果:`.spec/02-SYSTEM.html` §2(Bun Crate DAG)+ §3(Servo 36 组件分层)。

## 编程规范

### P-1 红线

`TODO` / `FIXME` / `stub` / 空实现 / `console.log` — commit 前清除。`oracle_gate` 强制执行。

### P-2 复杂度限制

嵌套 ≤5 层 | 圈复杂度 ≤10 | 参数 ≤5 个

### P-3 架构风格

Clean Code + Rust 惯例 | DRY/KISS | ECS + Microkernel

### P-4 Executor 编码协议

reuse probe → Scan-Before-Code → TDD → `@trace` → Oracle Gate → commit

## 许可证

MPL-2.0(SpiderMonkey + Servo) + MIT(Bun crates)
