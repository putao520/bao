# Bao (包子) — Bun + SpiderMonkey + Servo

Bun 的 SpiderMonkey 引擎分支，融合 servo 浏览器引擎，所有能力始终可用。

## 核心原则（铁律，所有工作必须遵守）

**Bun 的 C/Zig 层 → Rust 替换。JSC → SM。我们自己尽量少写、不写，全部复用 Bun crate，提供给 Servo 最好。**

具体含义：
- Bun workspace 中 ~85 个纯 Rust crate（零 JSC）→ 100% 复用，禁止手写已有功能
- Bun 的 C++ uSockets/uWS 二进制（libuwsockets.cpp）→ 链接复用，禁止手写 Rust 翻译 C 代码
- JSC → SpiderMonkey 替换是唯一需要手写的桥接层
- 所有新代码必须先查 Bun workspace 是否已有实现，有则复用，无则尽量从 Bun 上游移植

**只有以下情况允许手写 Rust**：
1. loop 核心必须与 FilePoll 共享 epoll fd → bao_uloop 的 epoll tick 是必要的
2. JSC → SM 的桥接层（bao_engine）是必要的
3. Servo 集成桥接层（bao_browser）是必要的

**禁止手写**：
- us_socket_* / us_socket_group_* / us_listen_socket_* → 链接 C++ 二进制
- bsd_send / bsd_shutdown 等 BSD socket 辅助 → C++ 二进制已有
- HTTP 解析/响应 → bun_uws::App（C++ 二进制）已有
- DNS → bun_dns 已有
- 模块解析 → bun_resolver 已有
- Base64 → bun_base64 已有

## 核心差异化

1. **SpiderMonkey 引擎**：替代 JavaScriptCore (MPL-2.0)，与 servo 共享同一 JSContext
2. **全功能浏览器**：DOM + CSS + 布局 + 渲染 + CDP (libservo，尊重 servo 原有设计)
3. **Node.js/Bun API 始终在线**：require/fs/path/crypto/http 等与 Web API 同一上下文共存
4. **反指纹模块**：TLS/HTTP/Canvas/Navigator/行为模拟 (Stealth)
5. **单进程多线程**：尊重 servo 和 Bun 原有架构

## 命名规范

| 层级 | 规则 |
|------|------|
| 用户品牌 | `bao` (bao run / bao test / bao browser) |
| JS 全局对象 | `Bun.*` (保留) + `Bao.*` (别名，同一对象) |
| 内部 Rust crate | `bun_*` 不改 (保持上游兼容) |
| 环境变量 | `BUN_*` (保留) + `BAO_*` (新增别名) |
| 代码引用 | 保留所有 Bun 内部引用 |

原则：用户输入 `bao`，代码里还是 `bun`。最小化与上游 Bun 的 diff。

---

## 上游项目参考

### Bun — 基础运行时来源

| 路径 | 参考价值 |
|------|---------|
| `~/code/rust/bun/CLAUDE.md` | 构建命令、测试规范、代码架构、crate 组织、开发规范 |
| `~/code/rust/bun/Cargo.toml` | 105 crate workspace 定义、依赖版本锁定 |
| `~/code/rust/bun/src/` | **核心复用来源**：~85 个纯 Rust crate (L0-L10) 零修改复用 |
| `~/code/rust/bun/src/jsc/` | JSC 绑定层 (L11-L13, 13 crate) — SpiderMonkey 迁移目标，理解 JSC→SM 映射的参考 |
| `~/code/rust/bun/src/runtime/` | Bun JS API 实现 — .classes.ts 定义复用 95% |
| `~/code/rust/bun/src/resolver/` | 模块解析器 — 100% 复用，SM hook 桥接参考 |
| `~/code/rust/bun/src/event_loop/` | 事件循环 — SM JobQueue 桥接设计参考 |

**Bun SPEC 测绘成果**：已测绘为 `.spec/02-SYSTEM.html` §2 (Crate DAG 13 层 + JSC 边界分析 + 两维分离模式)

### Servo — 浏览器引擎来源

| 路径 | 参考价值 |
|------|---------|
| `~/code/rust/servo/Cargo.toml` | 36 组件 workspace 定义 |
| `~/code/rust/servo/components/servo/` | **libservo crate** (v0.1.0) — Bao 嵌入 servo 入口 |
| `~/code/rust/servo/components/script/` | DOM 实现 (280K LOC) — 依赖 mozjs，共享 SM JSContext |
| `~/code/rust/servo/components/script_bindings/` | SpiderMonkey ↔ DOM 桥接 (WebIDL 代码生成) — SM GC 管理参考 |
| `~/code/rust/servo/components/style/` | Stylo CSS 引擎 — Firefox 共用，样式计算 |
| `~/code/rust/servo/components/layout/` | 布局引擎 — Layout Box 生成 |
| `~/code/rust/servo/components/net/` | Fetch 实现 (WHATWG 规范) — 网络栈参考 |
| `~/code/rust/servo/components/constellation/` | 中央协调器 — IPC 中枢，单进程模式自动优化 |
| `~/code/rust/servo/ports/servoshell/` | Embedder 实现参考 — ServoDelegate/WebViewDelegate 模板 |

**Servo SPEC 测绘成果**：已测绘为 `.spec/02-SYSTEM.html` §3 (36 组件分层 + 依赖关系 + 关键数据)

### 辅助参考项目

| 路径 | 参考价值 |
|------|---------|
| `~/code/rust/mozjs/` | mozjs crate 源码 — SM FFI 绑定细节、API 覆盖率、缺失绑定清单 |
| `~/code/rust/blitz/` | DioxusLabs 模块化浏览器 — 嵌入式浏览器参考架构 |

---

## SPEC 目录

| SPEC 目录 | .spec/00-INDEX.html |
|-----------|---------------------|

### SPEC 文件清单

| 文件 | 内容 | 核心设计产物 | 状态 |
|------|------|-------------|------|
| 01-BUSINESS.html | 业务架构 | 功能模块树 · 用例图 · 指标维度表 · 许可证 · 活动图 | 草稿 |
| 02-SYSTEM.html | 系统架构 | Bun Crate DAG · Servo 组件 · 融合映射 · 多页面管理 · CDP 双层抽象 · Permission 沙箱 · 管道 · 集成 · NFR · SLA · 接口协议 · 事件目录 · 运行时状态图 | 草稿 |
| 03-PROCESS.html | 核心流程 | JS 执行管线 · 渲染管线 · CDP 路由 · 状态机 · 时序约束 · 线程模型 · 异常流程 | 草稿 |
| 04-DATA-MODEL.html | 数据模型 | 18 Entity (含 PageHandle/PagePool/CdpRouter/Permission) · 模型树 · 缓存策略 · Crate 数据流 | 草稿 |
| **05-IMPLEMENTATION.html** | **实施路线图** | **5 阶段任务分解 · 复用矩阵 · 风险矩阵 · 验证点** | 草稿 |
| 10-REQUIREMENTS.html | 功能需求 | 31 REQ · 6 域 (ENG/CLI/BRW/CDP/STL/LIB) · 5 NFR · 追溯矩阵 | 草稿 |
| 11-TESTING.html | 测试用例 | 10 TEST · 多页面管理 · CDP 抽象层 · Permission 沙箱 · 资源管理 | 草稿 |

### 验证状态

`spec_validate` — 0 errors / 0 warnings / HEALTHY

---

## 开发计划 (详见 .spec/05-IMPLEMENTATION.html)

| 阶段 | 名称 | 核心 REQ | 新建 Crate |
|------|------|---------|-----------|
| Phase 1 | SpiderMonkey 替换 JSC | REQ-ENG-001~007 · REQ-CLI-001 | bao_engine + 12 桥接 + bao_runtime |
| Phase 2 | servo 引擎集成 + 渲染 | REQ-CLI-002 · REQ-BRW-001/002/003 | bao_browser |
| Phase 3 | CDP Server | REQ-CDP-001~008 | bao_cdp |
| Phase 4 | Stealth 反指纹 | REQ-STL-001~007 | bao_stealth |
| Phase 5 | Headless 多页面库 | REQ-LIB-001~004 · NFR | bao_browser (PagePool) + bao_cdp (CdpRouter) |
| Phase 6 | 集成测试与发布 | NFR | — |

---

## 重构策略：复用为主 — 高性能化、去锁化、成熟库化

**核心原则**：禁止手写已存在于 workspace 的功能。Bun 的 ~85 个纯 Rust crate 是经过生产验证的高性能实现，必须 100% 复用。

### 三化原则

| 原则 | 含义 | 检查点 |
|------|------|--------|
| **高性能化** | 零拷贝、SIMD、mmap、io_uring — 复用 Bun 已有的优化 | 禁止 `Vec::new()` 手写 buffer、禁止 `String::from_utf8_lossy` 替代零拷贝 |
| **去锁化** | 单线程 JS 执行模型下禁止 `Mutex`/`RwLock`，用 `thread_local!` + `RefCell` | `Mutex` 仅用于跨线程共享（HTTP 等真正的并发场景） |
| **成熟库化** | workspace 已有 crate > crates.io 成熟库 > 手写 | 每个新函数先 grep workspace crate 是否已有实现 |

### 可复用 Crate 映射表（Phase 1 关键）

| 功能 | 复用 Crate | 替代手写代码 |
|------|-----------|-------------|
| 模块解析 | `bun_resolver` | 手写 `resolve_specifier`/`resolve_node_modules` |
| 事件循环 | `bun_event_loop` | 手写 `JobQueue::drain` + `thread::sleep` 轮询 |
| HTTP 服务/客户端 | `bun_http` + `bun_uws` + `bun_picohttp` | 手写 `std::net::TcpListener` + HTTP 解析 |
| URL 解析 | `bun_url` | 手写 URL 拆分 |
| Base64 | `bun_base64` | 手写 base64_encode |
| I/O 抽象 | `bun_io` | 直接 `std::fs` 同步调用 |
| 进程管理 | `bun_spawn` | 缺失 Bun.spawn() |
| 路由 | `bun_router` | — |
| DNS | `bun_dns` | — |
| 事件循环定时器 | `bun_event_loop` + uSockets timer | 手写 `TimerHeap` + `thread::sleep` |
| TS 转译 | `bun_transpiler` | — |
| 文件监听 | `bun_watcher` | — |
| Node.js polyfill | `node-fallbacks` | 手写 node:fs/path/crypto/http |
| 字符串处理 | `bun_string_encoding` (string/) | — |
| 线程工具 | `bun_threading` | — |
| 系统工具 | `bun_sys` | — |
| 数据结构 | `bun_collections` | — |

### 复用优先级

```
1. workspace 内 bun_* crate（已编译、已优化、已测试）
2. crates.io 成熟库（url, sha2, hmac, etc. — 已在 Cargo.toml）
3. 仅当 1/2 都没有时才允许手写
```

### 当前手写代码 → 复用迁移计划

| 当前手写模块 | 迁移目标 | 优先级 |
|-------------|---------|--------|
| `bao_engine/module_loader.rs` 中的 `resolve_specifier` | 桥接 `bun_resolver` | P0 |
| `bao_runtime/require.rs` 中的 `resolve_specifier` | 桥接 `bun_resolver` | P0 |
| `bao_runtime/node_http.rs` (TcpListener + 手写 HTTP 解析) | 桥接 `bun_http` + `bun_uws` | P0 |
| `bao_runtime/timers.rs` (TimerHeap + sleep 轮询) | 桥接 `bun_event_loop` uSockets timer | P1 |
| `bao_runtime/node_fs.rs` 中的 `base64_encode` | 替换为 `bun_base64` crate | P1 |
| `bao_runtime/globals.rs` 中的 `do_fetch` (minreq) | 桥接 `bun_http` 或保留 minreq | P2 |

---

## 技术栈

| 组件 | 来源 | 用途 |
|------|------|------|
| SpiderMonkey | mozjs crate (MPL-2.0) | JS 引擎 |
| servo (libservo) | servo crate (MPL-2.0) | DOM + CSS + Layout + webrender 渲染 |
| cdp-protocol | cdp-protocol crate (MIT) | CDP 类型定义 |
| Bun 基础设施 | ~85 个纯 Rust crate (MIT) | HTTP/FS/Resolver/Bundler/... |

## 新增 Crate 结构

```
src/
├── [Bun 现有 crate 保留不动 (~85 个)]
├── bao_engine/         # SpiderMonkey 引擎封装 (替代 bun_jsc)
├── bao_browser/        # servo 引擎封装 (ServoDelegate + WebViewDelegate + 截图)
├── bao_cdp/            # CDP 协议服务 (11 domain)
└── bao_stealth/        # 反指纹模块 (TLS/HTTP/Canvas/Navigator/Behavior)
```
