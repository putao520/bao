# Bao 架构

> 一句话:**Bao 把 Servo(浏览器引擎)+ SpiderMonkey(JS 运行时)+ Node/Bun API + CDP 自动化统一到一个 Rust 二进制。**

这份文档给想快速理解 Bao 是什么、怎么拼起来的 OSS 用户。需要深 dive 内部契约 / REQ / 状态机的贡献者请走 [.spec/02-SYSTEM.html](../.spec/02-SYSTEM.html)。

## 总览

```
                        ┌─────────┐
                        │   Bao   │
                        └────┬────┘
                             │
              ┌──────────────┴───────────────┐
              │                              │
        ┌─────┴──────┐                 ┌──────┴──────┐
        │  Rust API  │                 │     CDP     │
        │ BaoRuntime │                 │  WebSocket  │
        └─────┬──────┘                 └──────┬──────┘
              │                               │
        ┌─────┴──────┐               ┌────────┴────────┐
        │  PagePool  │               │   Playwright    │
        │ PageHandle │               │   Puppeteer     │
        └─────┬──────┘               │  bao_cdp_client │
              │                      └────────┬────────┘
              │                               │
              └───────────────┬───────────────┘
                              │
           ┌──────────────────┴──────────────────┐
           │              JS Realm               │
           │                                      │
           │    ┌──────────┐      ┌──────────┐    │
           │    │  Node    │      │   Page   │    │
           │    │  Realm   │      │  Realm   │    │
           │    │ (trusted)│      │(untrusted)│   │
           │    └─────┬────┘      └────┬─────┘    │
           │          │                │          │
           │   Bun/Node API    Web APIs + DOM    │
           │          │                │          │
           │          └────────┬───────┘         │
           │                   │                 │
           │            SpiderMonkey             │
           │       (per-thread JSContext)        │
           │                   │                 │
           │                 Servo               │
           │       DOM/CSS/Layout/WebRender      │
           └──────────────────┬──────────────────┘
                              │
           ┌──────────────────┴──────────────────┐
           │       Bao 原创层 (bao_*)            │
           │                                      │
           │  bao_engine   bao_browser   bao_cdp  │
           │  bao_runtime  bao_stealth   bao_uloop│
           │  bao_cdp_client                       │
           ├──────────────────────────────────────┤
           │   Bun ~85 个纯 Rust crate(零修改复用) │
           ├──────────────────────────────────────┤
           │   mozjs · libservo · boringssl       │
           │   cdp-protocol                       │
           └──────────────────────────────────────┘
```

横向看是**两个入口**(Rust API / CDP),纵向看是**四层栈**(Realm 抽象 → Bao 原创层 → Bun crates → 上游 big-three)。同一个 SpiderMonkey JSContext 同时承载 Node API 和 Web API,这是 Bao 与其他 headless 浏览器 / Node runtime 的关键差异点。

## 核心概念

### BaoRuntime —— 对外入口

`BaoRuntime::new(BaoConfig)` 是用户接触的第一类型,内部管理全局唯一 servo 实例 + 一个 `PagePool`。多页面场景下每页独立 servo `ScriptThread`(独立事件循环 + 独立线程局部 JSContext),由 PagePool 统一调度、idle 回收。宿主代码只 depend package `bao`,不需要分别拉内部 crate。

### 双 Realm 安全模型

同一个 JSContext 下并存两个 JS Realm:

| Realm | global | 可见 API | 谁的脚本跑在这 |
|-------|--------|---------|----------------|
| **Node Realm** | Node global | Node.js + Bun API + DOM + Web API | 宿主可信脚本(`PageHandle::evaluate_js`) |
| **Page Realm** | `Window` | 仅 Web API + DOM | 页面 JS / untrusted 脚本(`evaluate_js_web`) |

这是 Bao 的关键差异点:**DOM 和 Node API 在同一可编程运行时里共存但隔离**。页面脚本 `typeof require === 'undefined'`,而宿主脚本可以一行里同时 `document.querySelector(...)` 和 `require('fs')`。Node API 不写入 `Window` global,untrusted 页面拿不到。

### JSContext 模型

全局唯一 `JSEngine` + 每个 servo `ScriptThread` 持有线程局部 `JSContext`(对齐 servo 上游 `script_runtime.rs` 的 thread-local slot 语义)。规则:

- 各模式(CLI / browser / CDP)各自在其所属线程内使用该线程的 JSContext
- DOM ↔ Node.js 互操作必须发生在**同一线程内**——跨线程会破坏 activation 栈导致 SIGSEGV
- **禁止跨线程传递 `JSObject` 裸指针**:`JSObject` 归属创建它的线程的 JSContext。跨线程只能传 `PageId` / 句柄 / 序列化数据

### PagePool / PageHandle

`PagePool` 管理多页面生命周期:`create_page(PageConfig)` 返回 `PageHandle`,idle 超时(默认 60s)自动回收,`close_all()` 一键清理。`PageHandle` 是高层 API:`navigate` / `evaluate_js` / `evaluate_js_web` / `take_screenshot` / `page_title` / `current_url` / `permission`。每页可挂独立 `StealthProfile`。

### CDP

内置 CDP Server,支持 12 个域(Page / Runtime / DOM / Network / Debugger / Input / Emulation / CSS / Overlay / Log / Fetch / Target),Playwright / Puppeteer 可直连 `ws://127.0.0.1:9222`。

Rust 侧用 `bao::Browser::connect(url)` 按 scheme 路由:`memory://bao` 走同进程 `InMemoryTransport`(零网络往返,直连 servo),`ws://...` / `http://...` 走外部 WebSocket。同一套 Playwright 风格 API,两种 transport 互换。

### Browser Identity & Privacy(原 Stealth)

可配置的浏览器身份与隐私 profile,运行时数据驱动(不是 Cargo feature):

- **TLS**:JA3 / JA4 hash、cipher suites、extensions(匹配 Firefox / Chrome)
- **HTTP/2**:AKAMAI fingerprint(SETTINGS + PRIORITY frame 模式)
- **Canvas / WebGL / Audio**:per-pixel / per-profile 噪声注入
- **Navigator / Screen**:userAgent、vendor、platform、hardwareConcurrency、deviceMemory、width/height/colorDepth
- **Behavior**:贝塞尔曲线鼠标路径 + 拟人点击 / 打字延迟

`StealthProfile::firefox_default()` / `chrome_default()` 是预置 profile;`StealthProfile` 是 `Clone + Debug` 纯数据结构,可直接构造自定义 profile。

## crate 分层

对外只暴露 **一个** Cargo package:`bao`(整栈始终链接)。`bao_*` 是 monorepo 内部分层,**不要**在宿主项目里分别 path 依赖。

| crate | 职责 |
|-------|------|
| `bao` | **唯一对外 lib**,整栈 re-export |
| `bao_engine` | SpiderMonkey 引擎封装 + JSC→SM 桥接 + context/job_queue |
| `bao_browser` | servo 集成桥;`BaoRuntime`(浏览器运行时)+ `PagePool` + `PageHandle` + `BaoServoDelegate` |
| `bao_cdp` | CDP Server(`cdp-server` crate)+ servo 桥(`ServoTargetProvider` / `CDPRdpBridge`) |
| `bao_cdp_client` | Playwright 风格高层 API(`Browser::connect("memory://bao" \| "ws://...")`) |
| `bao_runtime` | Node.js / Bun API 兼容层(fs / http / crypto / sqlite / ffi) |
| `bao_stealth` | 反指纹引擎;`StealthProfile` + `StealthEngine`(TLS / HTTP2 / Canvas / Navigator / WebGL / Audio / Behavior) |
| `bao_uloop` | 事件循环;epoll tick 与 `FilePoll` 共享 fd |
| `bao_cli` / `bao_bin` | `bao` CLI binary(clap subcommands) |
| `bao_bundler` | 打包器(基于 `bun_bundler`) |
| `bao_crypto` | crypto 桥(boringssl) |
| `bao_boringssl_bridge` | boringssl Rust 桥 |
| `bao_engine_macros` | `codegen_cached_accessors` 宏 |
| `bao_lints` | BCE 门禁 AST 检测器 |
| `bao_native_stubs` | dispatch no-op stubs + C 库桥锚点 |

底层依赖:`mozjs`(SpiderMonkey FFI,MPL-2.0)、`libservo`(DOM+CSS+Layout+webrender,MPL-2.0)、`boringssl`(TLS)、`cdp-protocol`(CDP 类型)、Bun workspace ~85 个纯 Rust crate(零修改复用)。

## 数据流

**页面 JS 执行**:`<script>` → servo `ScriptThread` 的 thread-local JSContext → SpiderMonkey 解释执行 → DOM/CSS/Layout(servo)→ webrender 渲染。

**Node.js API 调用**(Node Realm):同一 JSContext 的 Node global → `bao_runtime` → 对应 Bun crate(`bun_fs` / `bun_http` / `bun_crypto` / ...)或 `bun:sqlite` / `bun:ffi` 实现。

**互操作**:DOM 句柄和 Node API 在同一线程同一 JSContext 内可直接互调——这是双 Realm 模型得以成立的前提。跨线程 / 跨 JSContext 传递 JS 对象裸指针被铁律禁止,只能走 `PageId` 或序列化数据。

**CDP**:外部 WebSocket client → `bao_cdp` Server → 路由到对应 `ScriptThread` 的 CDP 域 handler → servo / SpiderMonkey。`memory://` scheme 跳过网络,走同进程 `InMemoryTransport` 直连。

## 进一步阅读

- [README](../README.md) —— 安装、CLI、示例代码、Cargo 依赖方式
- [CLAUDE.md](../CLAUDE.md) —— 项目指令(架构原则、复用映射、铁律)
- [.spec/02-SYSTEM.html](../.spec/02-SYSTEM.html) —— 系统架构深 dive(Bun Crate DAG + Servo 组件 + 融合映射)
- [.spec/03-PROCESS.html](../.spec/03-PROCESS.html) —— 核心流程(JS 执行管线 / 渲染管线 / CDP 路由 / 状态机)
- [.spec/10-REQUIREMENTS.html](../.spec/10-REQUIREMENTS.html) —— 31 REQ(ENG / CLI / BRW / CDP / STL / LIB 六域)
- [docs/unified-library-integration.md](./unified-library-integration.md) —— 嵌入到自己项目的集成方式

## 许可证

MPL-2.0(SpiderMonkey + Servo) + MIT(Bun crates)。
