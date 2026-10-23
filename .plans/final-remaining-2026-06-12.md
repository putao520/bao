# 剩余任务完整开发计划

> 创建日期: 2026-06-12
> 前置: arch-perf Phase 1-3 ✅ | audit-remediation A1-A3 ✅ | B1 ✅ | C1-C4 ✅ | D1 ✅ | D3 ✅
> 目标: 关闭两个计划的所有剩余 TODO

---

## 0. 剩余项汇总

| # | 任务 | 来源 | 复杂度 | 依赖 |
|---|------|------|--------|------|
| T1 | bao_tls 链接集成（替代 native_stubs 的 UpgradedDuplex） | A4-4 | 中 | bao_tls 已有实现 |
| T2 | node_tls.rs SecureContext 真实实现 | A4-3 | 中 | T1 |
| T3 | node_tls.rs tls.createServer 真实实现 | A4-2 | 高 | T2 |
| T4 | node_tls.rs 事件方法实现 | A4-5 | 低 | T1 |
| T5 | node_tls.rs SSL_set_ciphersuites | A4-6 | 低 | T1 |
| T6 | bao_runtime 依赖 bao_tls | A4 | 低 | 无 |
| T7 | eprintln!/println! → log/bun_sys | D2 | 低 | 无 |
| T8 | CDP enum dispatch (Box<dyn> → enum) | arch-perf 2b | 中 | 无 |
| T9 | 端到端集成测试 | A5 | 高 | T1-T6 |

---

## 1. Phase 1: TLS 管线打通 (T6 → T1 → T5 → T4 → T2 → T3)

### T6: bao_runtime 添加 bao_tls 依赖

**文件**: `src/bao_runtime/Cargo.toml`
```
bao_tls.workspace = true
```

**验证**: `cargo check -p bao_runtime`

### T1: UpgradedDuplex 链接集成

**问题**: `bao_native_stubs` 和 `bao_tls/socket.rs` 都导出同名 `#[no_mangle]` 符号。当前链接器用 native_stubs 的 stub。

**方案**: 从 `bao_native_stubs/src/lib.rs` 删除 12 个 UpgradedDuplex stub 函数（lines 122-146），让链接器绑定 `bao_tls` 的真实实现。同时更新 force_link 函数中的引用。

**文件变更**:
- `src/bao_native_stubs/src/lib.rs`:
  - 删除 lines 122-146 的 12 个 `UpgradedDuplex__*` 函数
  - 更新 force_link() 中对 UpgradedDuplex 的引用（lines 76-82）

**风险**: 如果最终二进制不依赖 bao_tls，链接会失败。需要确认 bao_runtime → bao_tls 依赖链。

**验证**: `cargo check -p bao_runtime`，确认无链接错误

### T5: SSL_set_ciphersuites 真实实现

**文件**: `src/bao_native_stubs/src/lib.rs` (line 277)
- 当前: `return 0;`（no-op）
- 替换: 调用 bao_tls provider 配置 cipher suite 列表
- 或者: 移至 bao_tls 导出，native_stubs 删除

### T4: node_tls.rs 事件方法

**文件**: `src/bao_runtime/src/node_tls.rs`
- 当前: 9 个方法绑定到 `tls_socket_noop`
- 替换: 实现 `once/emit/on/removeListener/removeAllListeners/off` 为真实事件注册
- 底层: 使用 `bao_runtime::node_events` 的 EventEmitter 机制

### T2: SecureContext 真实实现

**文件**: `src/bao_runtime/src/node_tls.rs` (lines 247-250)
- 当前: setKey/setCert/addCACert/setCA → tls_socket_noop
- 替换:
  - setKey: 解析 PEM 私钥 → 存入 `rustls::PrivateKey`
  - setCert: 解析 PEM 证书链 → 存入 `Vec<rustls::Certificate>`
  - addCACert: 解析 PEM CA → 添加到 root store
  - setCA: 同 addCACert 批量
- 内部存储: 在 SecureContext JS 对象的 hidden slot 中存 `Box<SecureContextState>`
- State 包含: `Option<PrivateKey>`, `Vec<Certificate>`, `RootCertStore`

### T3: tls.createServer 真实实现

**文件**: `src/bao_runtime/src/node_tls.rs` (lines 213-233)
- 当前: listen/close/on → noop
- 替换:
  - listen: 创建 `bao_tls::TlsServer::listen()` + `bun_uws::App`
  - close: 关闭 uWS listen socket
  - on: 注册事件回调 (tlsClientConnection / secureConnection)
- 依赖: SecureContext 配置 (T2) + UpgradedDuplex 集成 (T1)

---

## 2. Phase 2: 诊断输出清理 (T7)

### T7: eprintln!/println! 替换

**25 处**分布在 5 个文件:

**bao_engine/src/host_fn.rs (9处)**:
- console.log/error/warn/time/trace/assert 的实现
- 方案: 替换为 `eprintln!` → `[bao]` 前缀的 log crate 输出
- 或者: 使用 `std::io::stderr().write_all()` 直接写 fd（避免格式化开销）

**bao_browser/src/lib.rs (5处)**:
- Runtime 状态输出 (4行) + 导航日志 (1行)
- 方案: 删除运行时状态输出（调试遗留），导航日志用 `log::debug!`

**bao_browser/src/delegate.rs (3处)**:
- servo/webview console 转发 + crash 日志
- 方案: crash 日志保留（安全关键），console 转发用 `log::trace!`

**bao_cdp/src/lib.rs (5处)**:
- CDP server 启动/监听日志
- 方案: 替换为 `log::info!`

**bao_runtime/src/bun_api.rs (3处)**:
- Bun.serve 监听输出
- 方案: 替换为 `log::info!`

**统一方案**: 引入 `log` crate，所有 `eprintln!/println!` → `log::info!/log::debug!/log::trace!/log::error!`。
最终二进制通过 `env_logger` 或自定义 logger 控制输出。

---

## 3. Phase 3: CDP enum dispatch (T8)

### T8: Box<dyn DomainHandler> → enum DomainDispatch

**当前**: 12 个 handler 通过 `Box::new(handler)` 注册，运行时虚表分派
**目标**: 枚举直接分派，消除虚表开销和堆分配

**文件变更**:
- `src/cdp-server/src/lib.rs`: 定义 `DomainDispatch` enum（12 变体）
- `src/cdp-server/src/registry.rs`: `HashMap<&str, Box<dyn>>` → `HashMap<&str, DomainDispatch>`
- `src/cdp-server/src/registry.rs`: dispatch_command → match 分派
- `src/bao_cdp/src/domains/mod.rs`: register 函数改为传入 enum 变体

**enum 定义**:
```rust
pub enum DomainDispatch {
    Page(PageHandler),
    Runtime(RuntimeHandler),
    Dom(DomHandler),
    Network(NetworkHandler),
    Debugger(DebuggerHandler),
    Input(InputHandler),
    Emulation(EmulationHandler),
    Css(CssHandler),
    Overlay(OverlayHandler),
    Log(LogHandler),
    Fetch(FetchHandler),
    Target(TargetHandler),
}
```

**收益**: 消除 12 次堆分配 + 虚表间接跳转。代码量增加约 60 行（match 分派）。

---

## 4. Phase 4: 端到端集成测试 (T9)

### T9: 集成测试

**前提**: Phase 1-3 全部完成，`cargo check` 零错误

**测试用例**:

| # | 测试 | 验证点 |
|---|------|--------|
| E1 | HTTPS fetch | TLS 握手成功，响应正常 |
| E2 | tls.connect | 返回 authorized+encrypted 对象 |
| E3 | tls.createServer | 加载 cert+key，客户端连接成功 |
| E4 | CDP Debugger | 断点命中，step 暂停，evaluate 返回值 |
| E5 | CDP Target | createTarget 新建 WebView，attachToTarget 路由 |
| E6 | Stealth | Canvas/Audio/Navigator 指纹修改 |

**方法**: 启动 bao browser + CDP server → Playwright 连接 → 执行测试脚本

---

## 5. 执行顺序与依赖 DAG

```
T6 (add dep) ──→ T1 (link swap) ──→ T5 (ciphersuites)
                   │                      │
                   ├─→ T4 (events)        │
                   │                      │
                   ├─→ T2 (SecureContext) ─┼─→ T3 (createServer)
                   │                      │
                   └──────────────────────┴─→ T9 (e2e tests)

T7 (log cleanup) — 独立，可与 Phase 1 并行
T8 (enum dispatch) — 独立，可与 Phase 1 并行
```

**建议并行度**: 3 条线
- **Line A**: T6 → T1 → T5 → T4 → T2 → T3 → T9（TLS 主线）
- **Line B**: T7（log 清理）
- **Line C**: T8（enum dispatch）

---

## 6. 预估工作量

| 任务 | 改动文件数 | 新增/修改行 | 预估时间 |
|------|-----------|------------|---------|
| T6 | 1 | 1 | 1 min |
| T1 | 1 | -40 | 10 min |
| T5 | 1 | 20 | 15 min |
| T4 | 1 | 60 | 20 min |
| T2 | 1 | 80 | 30 min |
| T3 | 1 | 100 | 45 min |
| T7 | 6 | 50 | 20 min |
| T8 | 3 | 120 | 30 min |
| T9 | 2+ | 200 | 60 min |
| **合计** | **~10** | **~590** | **~3.5h** |

---

## 7. 验证标准

- [ ] T1: `grep UpgradedDuplex bao_native_stubs` 零匹配
- [ ] T2: SecureContext setKey/setCert 不再 noop
- [ ] T3: tls.createServer().listen() 真实监听
- [ ] T4: 零 `tls_socket_noop` 在 node_tls.rs（除兼容 fallback）
- [ ] T5: SSL_set_ciphersuites 配置实际 cipher 列表
- [ ] T6: bao_runtime/Cargo.toml 包含 bao_tls
- [ ] T7: 零 `eprintln!`/`println!` 在生产代码（测试允许）
- [ ] T8: 零 `Box<dyn DomainHandler>` 在 cdp-server
- [ ] T9: 6 个集成测试全部 PASS
- [ ] `cargo check` 全 crate 零错误
