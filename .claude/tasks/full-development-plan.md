# Bao 全量开发计划 v3 — 终极版

## 一、真实 REQ 状态审计

基于源码逐条验证，非 SPEC 标注。

### VERIFIED（真实实现）
| REQ | 标题 | 验证依据 |
|-----|------|---------|
| REQ-ENG-001 | SpiderMonkey 引擎集成 | bao_engine 用 mozjs，JSContext/Runtime 完整 |
| REQ-CLI-001 | bao 品牌替换 | bao_bin CLI 完整 |
| REQ-CLI-002 | bao browser 子命令 | run_browser 入口完整 |
| REQ-BRW-001 | libservo 集成 | BaoRuntime 用 ServoBuilder + delegate |
| REQ-CDP-001 | CDP WebSocket Server | cdp-server crate 完整（HTTP + WS） |
| REQ-CDP-002 | Runtime Domain | RuntimeHandler 实现 |
| REQ-CDP-003 | Debugger Domain | DebuggerHandler 实现 |
| REQ-CDP-004 | Page Domain | PageHandler 实现 |
| REQ-CDP-005 | DOM Domain | DOMHandler + evaluate_js DOM 查询 |
| REQ-CDP-006 | Network Domain | NetworkHandler 实现 |
| REQ-CDP-008 | Target Domain | ServoTargetProvider 实现 |
| REQ-STL-003 | Canvas 指纹防护 | CanvasNoise 完整 + 测试覆盖 |
| REQ-STL-004 | Navigator/Screen 构造 | NavigatorProfile + ScreenProfile 完整 |
| REQ-STL-005 | WebGL/Audio 指纹防护 | WebGLProfile + AudioProfile 完整 |
| REQ-STL-006 | 行为模拟 | BehaviorSimulator 完整 |
| REQ-LIB-001 | 多页面管理 | PagePool 实现 |
| REQ-LIB-002 | PagePool 资源管理 | check_idle_pages + close_all |
| REQ-LIB-003 | CDP 双层抽象 API | InternalBackend + ExternalBackend |
| REQ-LIB-004 | Permission 可选沙箱 | PermissionGuard 实现 |

### SPEC-OUTDATED（已完成但 SPEC 标 draft）
| REQ | 标题 | 说明 |
|-----|------|------|
| REQ-CDS-001 | CDP 传输层 | cdp-server transport.rs 完整 |
| REQ-CDS-002 | Target 管理 | TargetProvider trait 实现 |
| REQ-CDS-003 | 会话生命周期 | CdpSession 完整 |
| REQ-CDS-004 | 消息路由 | DomainRegistry 路由完整 |
| REQ-CDS-005 | 事件系统 | EventBroadcaster 实现 |
| REQ-CDS-006 | DomainHandler trait 系统 | trait + 11 个 handler 实现 |
| REQ-CDS-007 | 并发安全 | Arc + Mutex 正确使用 |
| REQ-CDS-008 | 可配置性 | ServerConfig builder 完整 |

### WRONG-IMPLEMENTED（手写轮子，必须删除替换）
| REQ | 标题 | 手写垃圾 | 应替换为 |
|-----|------|---------|---------|
| REQ-ENG-004 | Event Loop 桥接 | 手写 TimerHeap 419行 | bun_event_loop |
| REQ-ENG-005 | Module Loader 桥接 | 手写 resolve_specifier ~240行 | bun_resolver |
| REQ-ENG-007 | Node.js 兼容层适配 | 手写 HTTP/HTTPS/TCP/DNS/Timer 等 ~2000行 | bun_http/bun_uws/bun_dns/bun_event_loop |

### STUB（空壳）
| REQ | 标题 | 缺什么 |
|-----|------|--------|
| REQ-ENG-002 | 代码生成后端重写 | 无实际代码生成实现 |
| REQ-ENG-003 | host_fn 抽象层 | 只实现了 console，缺少通用 host_fn 包装 |
| REQ-ENG-006 | Bun API 适配 | Bao.* 别名未实现，Bun.* API 大部分手写 |
| REQ-CDP-007 | CSS/Input/Emulation | cmd_mouse_event/cmd_key_event 空壳 |

### PARTIAL（部分实现）
| REQ | 标题 | 缺什么 |
|-----|------|--------|
| REQ-BRW-002 | 内存渲染 | SoftwareRenderingContext 存在，完整渲染管线未验证 |
| REQ-BRW-003 | SpiderMonkey JSContext 融合 | runtime_bridge.rs 明确说不共享 JSContext，用 JS polyfill 桥接 |
| REQ-STL-001 | TLS 指纹模拟 | TlsFingerprint 数据模型完整，但未注入 HTTP 客户端 |
| REQ-STL-002 | HTTP/2 指纹匹配 | Http2Fingerprint 数据模型完整，但未注入 HTTP 客户端 |
| REQ-STL-007 | CDP 隐蔽性 | StealthProfile 未贯穿到 BaoConfig（stealth: bool 非 Profile） |

---

## 二、手写垃圾清单（全部删除）

### 文件级删除
| 文件 | 行数 | 手写内容 | 替换为 |
|------|------|---------|--------|
| node_http.rs | 693 | tiny_http + TcpListener + HTTP 解析 | bun_http + bun_uws + bun_picohttp |
| node_https.rs | 300 | minreq HTTPS 客户端 | bun_http |
| node_net.rs | 338 | TcpListener/TcpStream + Socket 管理 | bun_uws |
| timers.rs | 419 | TimerHeap 二叉堆 + sleep 轮询 | bun_event_loop |
| node_dns.rs | 112 | 手写 DNS 解析 | bun_dns |
| require.rs 中 resolve | ~120 | resolve_specifier + resolve_node_modules | bun_resolver |
| module_loader.rs 中 resolve | ~120 | resolve_specifier | bun_resolver |
| globals.rs 中 fetch | ~50 | minreq do_fetch | bun_http |
| node_child_process.rs | ~80 | Command::new 手写 | bun_spawn |

### Cargo.toml 删除垃圾依赖
- `minreq = { version = "2", features = ["https"] }`
- `tiny_http = "0.12"`

### Cargo.toml 添加 Bun 依赖
- `bun_http = { path = "../http" }`
- `bun_uws = { path = "../uws" }`
- `bun_picohttp = { path = "../picohttp" }`
- `bun_resolver = { path = "../resolver" }`
- `bun_event_loop = { path = "../event_loop" }`
- `bun_dns = { path = "../dns" }`
- `bun_spawn = { path = "../spawn" }`
- `bun_io = { path = "../io" }`
- `bun_base64 = { path = "../base64" }`

---

## 三、未实现需求清单

### 完全未实现
| 任务 | 关联 REQ | 描述 |
|------|---------|------|
| E1: 代码生成后端 | REQ-ENG-002 | bao_engine 需要实现 JS → Rust binding 代码生成 |
| ~~E2: 通用 host_fn 包装~~ | ~~REQ-ENG-003~~ | ~~已完成: ArgReader + define_host_fn! 宏~~ |
| E3: Bao.* 别名 + Bun.* API | REQ-ENG-006 | 实现 Bun.serve/Bun.file/Bun.hash 等真正 API |
| ~~E4: servo 输入事件分发~~ | ~~REQ-CDP-007~~ | ~~已完成: InputHandler 通过 bridge 连接 servo~~ |

### 部分未实现
| 任务 | 关联 REQ | 描述 |
|------|---------|------|
| E5: JSContext 融合 | REQ-BRW-003 | 当前用 JS polyfill 桥接，需评估 servo 共享 JSContext 可行性 |
| E6: TLS 指纹注入网络栈 | REQ-STL-001 | TlsFingerprint → bun_http TLS 配置 |
| E7: HTTP/2 指纹注入 | REQ-STL-002 | Http2Fingerprint → bun_http HTTP/2 配置 |
| E8: StealthProfile 贯穿 | REQ-STL-007 | stealth: bool → Option<StealthProfile> 贯穿全链路 |

### SPEC 状态更新
| 任务 | 描述 |
|------|------|
| E9: REQ-CDS-001~008 | draft → implemented |
| E10: REQ-IMPL-06 | 新增 Phase 6 描述 |

---

## 四、执行计划

### 执行顺序

```
Phase 1: 删除手写垃圾（F/G/H/D/S）     ← 全部并行
    ↓
Phase 2: 未实现需求（E1-E8）             ← 部分串行
    ↓
Phase 3: SPEC 状态更新（E9-E10）         ← 快速
    ↓
Phase 4: 质量收敛                         ← 最后
```

### Phase 1：删除手写垃圾（并行）

所有任务互相独立，可用 worker_dispatch 并行。

| ID | 任务 | 文件 | 行数 | 替换为 |
|----|------|------|------|--------|
| F1 | 删除 node_http.rs | node_http.rs | 693 | bun_http + bun_uws |
| F2 | 删除 node_https.rs | node_https.rs | 300 | bun_http |
| F3 | 删除 node_net.rs | node_net.rs | 338 | bun_uws |
| F4 | 删除 globals.rs fetch | globals.rs | ~50 | bun_http |
| H | 删除 timers.rs | timers.rs | 419 | bun_event_loop |
| G1 | 删除 require.rs resolver | require.rs | ~120 | bun_resolver |
| G2 | 删除 module_loader.rs resolver | module_loader.rs | ~120 | bun_resolver |
| D | 删除 node_dns.rs | node_dns.rs | 112 | bun_dns |
| S | 删除 node_child_process.rs | node_child_process.rs | ~80 | bun_spawn |

**总计删除：~2232 行手写垃圾**

### Phase 2：未实现需求

| ID | 任务 | 依赖 | 复杂度 | 状态 |
|----|------|------|--------|------|
| E8 | StealthProfile 贯穿 | 无 | 低 | ✅ 已完成 |
| E4 | servo 输入事件分发 | 无 | 中 | ✅ 已完成 |
| E6 | TLS 指纹注入 | 被上游阻塞 | 中 | 🔶 stealth_http.rs 已创建 |
| E7 | HTTP/2 指纹注入 | 被上游阻塞 | 中 | 🔶 stealth_http.rs 已创建 |
| E2 | 通用 host_fn 包装 | 无 | 高 | ✅ ArgReader + define_host_fn! 宏 |
| E3 | Bun.* / Bao.* API | 无 | 高 | ✅ 已完成 (60+ 断言测试通过) |
| E1 | 代码生成后端 | 无 | 高 | 待实现 |
| E5 | JSContext 融合评估 | 无 | 高 | 待架构决策 |

### Phase 3：SPEC 状态更新

| ID | 任务 |
|----|------|
| E9 | REQ-CDS-001~008 draft → implemented |
| E10 | REQ-IMPL-06 Phase 6 描述 |

### Phase 4：质量收敛

| ID | 任务 |
|----|------|
| Q1 | cargo clippy 零 warning |
| Q2 | cargo test 全通过（> 150 测试） |
| Q3 | spec_audit 成熟度 > 70% |

---

## 五、依赖图

```
Phase 1 (全部并行):
  F1 ─┐
  F2 ─┤
  F3 ─┤
  F4 ─┤
  H ──┤──→ Phase 2
  G1 ─┤
  G2 ─┤
  D ──┤
  S ──┘

Phase 2:
  E8 (独立) ──→ E6 + E7 (依赖 F2 + E8)
  E4 (独立)
  E2, E3, E1 (独立，高复杂度)
  E5 (独立，需架构决策)

Phase 3: E9, E10 (Phase 2 完成后)
Phase 4: Q1, Q2, Q3 (全部完成后)
```

---

## 六、已完成的 Wave（历史记录）

- [x] Wave 1: SPEC 修复 + @trace 注入
- [x] Wave 2: bao_cdp 重构接入 cdp-server (11 DomainHandler)
- [x] Wave 3: bao_browser CDP 桥接升级
- [x] Wave 4: 测试实现 (102 测试通过)
- [x] Wave 5: 质量收敛
- [x] Wave 6: Phase 4 质量收敛 — 成熟度 57.3% → 89.3%
  - cdp-server SessionError 类型修复 clippy
  - SPEC 测试 GAP 补齐 (REQ-CDS-006/008, REQ-IMPL-06)
  - 323 测试全通过
- [x] Wave 7: 深度测试 + E2 host_fn 扩展
  - 90 个 domain 深度命令覆盖测试
  - ArgReader 类型化参数提取 + define_host_fn! 宏
  - bao_cdp dead_code 清理 (endpoint 移除)
  - 323 测试全通过
- [x] Wave 8: Bun.* API 测试 + Node.js 模块集成测试
  - bun_api_tests: 60+ 断言 (Bun.*/Bao.*/process.*) 全通过
  - node_fs_tests: 20 个 fs API 测试 (readFile/writeFile/stat/mkdir/rename/copy/unlink 等)
  - node_path_tests: 18 个 path API 测试 (join/resolve/basename/dirname/extname/parse 等)
  - node_crypto_tests: 11 个 crypto API 测试 (SHA-256/512/MD5/HMAC/randomBytes)
  - node_events_tests: 14 个 EventEmitter 测试 (on/emit/off/once/prepend/instanceof)
  - Bun.read 别名修复 (JS_DefineProperty 指向 readFile 同一 JSObject)
  - 151 测试通过 (3 个 flaky bridge channel 竞态，单独运行全通过)
