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

### 3. 复用优先(Bun crate > 社区库 > 手写)

Bun workspace 中 ~85 个纯 Rust crate(零 JSC)是经过生产验证的高性能实现,**100% 复用,禁止手写已有功能**。

```
1. workspace 内 bun_* crate(已编译、已优化、已测试)
2. crates.io 成熟库(url, sha2, hmac, etc. — 已在 Cargo.toml)
3. 仅当 1/2 都没有时才允许手写
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
| 环境变量 | `BUN_*`(保留) + `BAO_*`(新增别名,`BaoRuntime::new()` 调用 `init_env_aliases()` 把 `BAO_<SUFFIX>` 复制到 `BUN_<SUFFIX>`) |
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
| `bao_runtime` | `src/bao_runtime`(crate 名 `bun_runtime`) | Node.js/Bun API 兼容层;`BaoRuntime`(Node.js 运行时入口) |
| `bao_browser` | `src/bao_browser` | servo 集成桥;`BaoRuntime`(浏览器运行时) + `PagePool` + `PageHandle` + `BaoServoDelegate` |
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

## 每日自动化(daily-ops)

systemd timer 每日 09:37±10min 运行 `.claude/skills/daily-ops/`(上游轻量同步+issue 分诊,详见该 skill)。交互会话在此窗口派工前先查 `systemctl --user status bao-daily-ops`。上游同步基线 SSOT = `.claude/upstream-baseline.json`。

## 构建与测试

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

### mozjs 构建经验

1. **已内置从源码编译**:`mozjs-sys/build.rs` 的 `should_build_from_source()` 硬编码返回 `true`,无需 `MOZJS_FROM_SOURCE=1`。EBUSY patch + 其他本地修复始终生效
2. **rlib 包含 native 代码**:`libmozjs_sys-*.rlib` 打包了 `libjs_static.a` 的全部 C++ 符号。改 `.a` 不够——必须删 rlib 重新编译
3. **mozjs make 增量构建 bug**:make 会编译新 `.o` 但不重新打包 `libjs_static.a`。需手动 `ar -d` + `ar -q` 替换,或删整个 build output 目录
4. **清理顺序**:删 `.fingerprint/mozjs_sys-*` + `deps/libmozjs*` + `build/mozjs_sys-*` + `incremental/mozjs*`,然后 `cargo build`

#### EBUSY Patch(已应用)

`mozjs/mozjs-sys/mozjs/mozglue/misc/Mutex_posix.cpp` 的 `MutexImpl::~MutexImpl` 已 patch:

- 原始:`pthread_mutex_destroy` 返回非零时 `MOZ_CRASH`(SIGSEGV)
- Patch:忽略 `EBUSY`(libtest 线程池线程在 TLS teardown 时仍持有 mutex)
- 仅在 `result != 0 && result != EBUSY` 时才 `MOZ_CRASH`

如果 SIGSEGV 复现,第一步 `nm libmozjs_sys-*.rlib | grep MutexImplD1` 查 rlib 是否包含旧代码。

#### mozjs fork BAO patch 清单(5 项,0.21.4 全部在位)

上游同步 mozjs 时必须逐项重放(参照 git 历史 `git show <old>:vendor/mozjs/...`):

| # | Patch | 位置 | 语义 |
|---|-------|------|------|
| 1 | EBUSY 激进版 | `mozjs-sys/mozjs/mozglue/misc/Mutex_posix.cpp` | `MutexImpl` 析构整体 `return;`(进程退出期 TLS 可能已 unmap,EBUSY 时原版 MOZ_CRASH) |
| 2 | JSEngine init race | `mozjs/src/rust.rs` | `PROCESS_ENGINE_OUTSTANDING` OnceLock + `process_handle()`:多 BaoRuntime 二次 init 从 `Err(AlreadyInitialized)` 恢复而非 panic |
| 3 | set_hide_script_from_debugger | `mozjs/src/rust.rs`(BCE-20260622-004) | CompileOptions 的 `hideScriptFromDebugger_` setter:抑制 `onNewScript` → AtomCacheHashTable SIGSEGV 路径 |
| 4 | BaselineFrame NULL activation guard | `mozjs-sys/mozjs/js/src/jit/BaselineFrame.cpp`(BCE-20260621-002) | OSR 入口 `cx->activation()`/`prev()` NULL 检查,bail 回 interpreter |
| 5 | JS_NewEmulatesUndefinedFunction | `mozjs-sys/mozjs/js/src/jsapi.cpp` + `js/src/jsapi.h` + `mozjs/src/jsapi2_wrappers.in.rs` | callable NativeObject 且 `typeof` 为 "undefined"(镜像 Bun `Buffer.transcode` stub)。**注意:jsapi.h 声明必须在 `namespace JS` 外(全局作用域),否则 bindgen 生成 `JS::` 前缀 mangled link_name 与 cpp 全局定义不匹配 → 链接失败** |

另:`mozjs-sys/build.rs` 有 2 个 BAO patch(`should_build_from_source() -> true` 硬编码、`fix_stale_archive_objects()` make 增量 stale .o 修复)。

#### servo 定制文件清单(10 个,上游同步时逐个重放)

上游同步 servo 时,先 `grep -rln "BCE-\|BAO " vendor/servo/components/` 重建清单,再按"upstream 基底 + patch 精确重放"迁移(patch 锚点与完整记录见 git log 各 stage commit message):

| 文件 | Patch 概要 |
|------|-----------|
| `script/event_loop/script_thread.rs` | embedder 脚本/Worker-scope 回调注册(drain 于 handle_evaluate_javascript / run_worker_scope)、router_proxy 安装(BCE-20260627-009)、disable_script_debugger 门控(BCE-20260621-002) |
| `script/script_runtime.rs` | JSEngineSetup 幂等 init + engine leak(多 BaoRuntime 生命周期) |
| `script/dom/workers/dedicatedworkerglobalscope.rs` | worker-scope 回调 drain + clear_js_runtime 前 realm flush(UAF 防护) |
| `script_bindings/lock.rs` | ThreadUnsafeOnceLock 等(Bao 扩展) |
| `shared/base/id.rs` | Bao ID 类型(+ AtomicOptionScrollTreeNodeId 从上游增补) |
| `shared/base/lib.rs` + `ipc_router.rs` | per-instance RouterProxy(BCE-20260628-002) |
| `shared/net/lib.rs` | ipc_router 路由 + per-instance FetchThread + ProcessContentLength 等上游增补合并 |
| `shared/script/lib.rs` | ScriptThreadInit.router_proxy 字段(BCE-20260627-009) |
| `constellation/constellation.rs` + `event_loop.rs` | per-Constellation RouterProxy 全生命周期 |
| `net/connector.rs` + `websocket_loader.rs` 等 | **boringssl stealth TLS connector**(JA3/JA4 全面:cipher/curves/sigalgs 重排 + ALPN + H2 SETTINGS,REQ-STL-001;上游 rustls 迁移被回滚) |

`components/servo/lib.rs` 另有 Bao embedder API 面(register_script_thread_callback / register_worker_scope_callback / set_canvas_noise_seed / set_stealth_tls_config)。`config/prefs.rs`、`config/opts.rs`、`allocator/`、`net/async_runtime.rs` 有小 patch。

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
| mozjs | `vendor/mozjs/`(0.21.4,vendor 进本仓库;5 项 BAO patch 见上文清单) | SM FFI 绑定源码 |
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
