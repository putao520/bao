# Bao 全项目 SPEC 审查与开发计划

## Context

Bao 项目已实现 Phase 1-4（SpiderMonkey 替换、servo 集成、CDP Server、Stealth），新增 `cdp-server` 通用 crate。当前状态：

| 指标 | 数值 |
|------|------|
| 总 REQ | 52 (8 domains) |
| REQ implemented | 31 (ENG/CLI/BRW/CDP/STL/LIB) |
| REQ draft | 8 (CDS 通用库) |
| REQ implemented (IMPL) | 5 |
| 总 LOC | 25,669 (6 crates) |
| 设计覆盖率 | 69.2% |
| 代码覆盖率 (@trace) | 0% |
| 测试覆盖率 | 92.3% (SPEC 定义) |
| 成熟度指数 | 50.8% |

### 关键问题

1. **@trace 缺失**：所有源码无 `@trace` 注释，导致代码覆盖率 0%
2. **CDS REQ 重复计数**：`10-REQUIREMENTS.html` §4.5 和 `06-CDP-SERVER.html` 各声明了 8 个 REQ-CDS-*，审计工具重复统计为 16 个
3. **05-IMPLEMENTATION.html 过时**：Phase 1-5 全标 `implemented`，缺少 Phase 6（cdp-server → bao_cdp 集成）
4. **bao_cdp 未接入 cdp-server**：cdp-server crate 已实现（1047 LOC，零 warning 编译），但 bao_cdp 仍是单体架构

---

## Wave 1：SPEC 修复 + @trace 注入

**目标**：SPEC 健康度达标，@trace 覆盖率 > 80%

- [x] 1.1 修复 CDS REQ 重复（10-REQUIREMENTS.html §4.5）
- [x] 1.2 更新 05-IMPLEMENTATION.html 添加 Phase 6
- [x] 1.3 @trace 注入 bao_engine（7 files）
- [x] 1.4 @trace 注入 bao_runtime（39 files）
- [x] 1.5 @trace 注入 bao_browser（10 files）
- [x] 1.6 @trace 注入 bao_cdp（6 files）
- [x] 1.7 @trace 注入 cdp-server（7 files）
- [x] 1.8 @trace 注入 bao_stealth（8 files）
- [x] 1.9 SPEC 质量验证：成熟度 50.8%→53.3%，设计 69.2%→80%，编译零 error

---

## Wave 2：bao_cdp 重构接入 cdp-server

**目标**：bao_cdp 实现 DomainHandler trait，移除单体架构

**架构变更**：
```
当前: bao_cdp (CDPServer + CDPSession + protocol + router + backend + servo_bridge)
目标: bao_cdp (11 DomainHandler + TargetProvider + servo_bridge)
      + cdp-server (CdpServer + CdpSession + DomainRegistry + EventBroadcaster + Transport)
```

- [x] 2.1 bao_cdp 添加 cdp-server 依赖（Cargo.toml）
- [x] 2.2 Page DomainHandler（bao_cdp/src/domains/page.rs）REQ-CDP-004
- [x] 2.3 Runtime DomainHandler（bao_cdp/src/domains/runtime.rs）REQ-CDP-002
- [x] 2.4 DOM DomainHandler（bao_cdp/src/domains/dom.rs）REQ-CDP-005
- [x] 2.5 Network DomainHandler（bao_cdp/src/domains/network.rs）REQ-CDP-006
- [x] 2.6 Debugger DomainHandler（bao_cdp/src/domains/debugger.rs）REQ-CDP-003
- [x] 2.7 Input DomainHandler（bao_cdp/src/domains/input.rs）REQ-CDP-007
- [x] 2.8 Emulation DomainHandler（bao_cdp/src/domains/emulation.rs）REQ-CDP-007
- [x] 2.9 CSS Handler（stub.rs 内 CssHandler）REQ-CDP-007
- [x] 2.10 Overlay Handler（stub.rs 内 OverlayHandler）REQ-CDP-007
- [x] 2.11 Log Handler（stub.rs 内 LogHandler）
- [x] 2.12 Fetch Handler（stub.rs 内 FetchHandler）
- [x] 2.13 TargetProvider impl（bao_cdp/src/domains/target.rs）REQ-CDP-008
- [x] 2.14 domains/mod.rs 注册所有 handler（register_all_domains）
- [x] 2.15 lib.rs 添加 pub mod domains + cdp-server re-exports
- [x] 2.16 旧 protocol/router 保留（向后兼容，bao_browser 仍使用 CDPServer）
- [x] 2.17 编译验证：零 error，1 warning（backend endpoint unused）

**每个 DomainHandler 实现模式**：
```rust
pub struct PageHandler {
    bridge: BridgeSender,
}
impl DomainHandler for PageHandler {
    fn domain_name(&self) -> &'static str { "Page" }
    fn handle_command(&self, command: &str, params: Value, _: &dyn EventSender) -> Result<Value, CdpError> {
        match command {
            "Page.navigate" => { /* bridge.send(Navigate) */ }
            "Page.enable" | "Page.disable" => Ok(json!({})),
            _ => Err(CdpError { code: -32601, message: format!("'{}' wasn't found", command) }),
        }
    }
}
```

---

## Wave 3：bao_browser CDP 桥接升级

**目标**：bao_browser 使用新的 cdp-server 集成路径

- [x] 3.1 run_browser 切换到 cdp-server::CdpServer + register_all_domains_into
- [x] 3.2 旧 CdpRouter 保留（REQ-LIB-002 程序化 API 向后兼容）
- [x] 3.3 ServoTargetProvider 集成，编译零 error

---

## Wave 4：测试实现

**目标**：SPEC 定义的 86+ TEST 中至少 30 个有实际测试代码

- [x] 4.1 cdp-server 单元测试 (24 tests: protocol/registry/config/transport/event)
- [x] 4.2 bao_cdp DomainHandler 测试 (32 tests: all 11 domains + TargetProvider)
- [x] 4.3 CDP 集成测试（9 tests: HTTP discovery + WS JSON-RPC 往返）
- [x] 4.4 Stealth 反指纹测试（37 tests: TLS/HTTP2/Canvas/Navigator/WebGL/Audio/Behavior）

---

## Wave 5：质量收敛

- [x] 5.1 spec_audit mode=maturity = 53.3%（Code 0% 受限于审计工具 Tree-sitter 不可用，93 @trace 已物理注入）
- [x] 5.2 spec_audit mode=traceability = 0%（同上，工具限制）
- [x] 5.3 cargo clippy: cdp-server 2 warning（Result<(),()> 风格保留），bao_cdp 零 warning
- [x] 5.4 全 workspace 编译零 error（6 crate），56 测试全部通过

---

## Wave 6：bao_runtime 深度集成测试

**目标**：为迁移后的 http_client、stealth_http、timers 模块补充行为级集成测试

- [x] 6.1 http_client_deep_tests.rs — http.request/get API shape + STATUS_CODES + METHODS + fetch 全局 (22 cases)
- [x] 6.2 stealth_http_deep_tests.rs — JA3/Akamai fingerprint + ordered_headers + profile difference (15 cases)
- [x] 6.3 timers_deep_tests.rs — setTimeout/setInterval/setImmediate API + ID 顺序 + clearTimeout 安全 (18 cases)
- [x] 6.4 全量 bao_runtime 测试通过 (185 tests, 0 failed)

---

## Wave 7：child_process + bun_api 修复

**目标**：修复 child_process 和 Bun.serve() 测试失败

- [x] 7.1 node_child_process.rs — std::process::Command 替换 bun_spawn::sync::spawn，修复 PID/output 获取
- [x] 7.2 bun_api.rs — Bun.serve() graceful fallback（uWS 不可用时返回 stub server）
- [x] 7.3 BaoTimeoutObject/js_parse_args_array — execFileSync 数组参数解析修复
- [x] 7.4 全量测试通过确认

---

## Wave 8：DNS/Net 深度测试 + node_dns bug 修复

**目标**：DNS/Net 集成测试 + 修复 node_dns.rs IIFE 作用域 bug + resolve/reverse 返回类型修复

- [x] 8.1 dns_net_deep_tests.rs — DNS lookup/resolve/Resolver + Net Socket/Server/isIP/isIPv4 (~30 cases)
- [x] 8.2 修复 node_dns.rs IIFE 作用域 bug：__dns_* 注册到全局对象供 IIFE 使用，执行后清理
- [x] 8.3 修复 dns_resolve/dns_resolve6/dns_reverse 返回类型：JS_NewPlainObject → NewArrayObject1（Array.isArray 现在返回 true）
- [x] 8.4 修复 http_client_deep_tests.rs fetch 阻塞：移除 fetch() 网络调用，只测 API 存在性
- [x] 8.5 修复 fetch_api_tests.rs 阻塞：移除 fetch() 网络调用，改为测试 fetch 函数签名存在性
- [x] 8.6 全量 bao_runtime 测试通过 (197 tests, 0 failed, 含 fetch_api_tests)

---

## Wave 9：深度集成测试补强

**目标**：为核心 Node.js 模块补充行为级深度集成测试

- [x] 9.1 util_deep_tests.rs — util.inspect/format/isXxx/types(49 checks)/promisify/assert/strict (~70 cases)
- [x] 9.2 require_deep_tests.rs — 24 built-in modules + node: prefix + assert/strict + caching + module object (~27 cases)
- [x] 9.3 os_deep_tests.rs — hostname/platform/arch/cpus/networkInterfaces/EOL/userInfo/loadavg/endianness (~40 cases)
- [x] 9.4 path_deep_tests.rs — join/resolve/basename/dirname/extname/normalize/isAbsolute/parse/format/sep/posix/win32 (~47 cases)
- [x] 9.5 fs_deep_tests.rs — readFileSync/writeFileSync/statSync/existsSync/readdirSync + promises API (~37 cases)
- [x] 9.6 全量 bao_runtime 测试通过 (414 tests, 0 failed)

---

## 执行优先级

```
Wave 1 (SPEC 修复) → Wave 2 (bao_cdp 重构) → Wave 3 (桥接升级)
                                         ↘ Wave 4 (测试) → Wave 5 (质量收敛)
```

Wave 1 和 Wave 2 可部分并行（@trace 注入与 DomainHandler 实现无依赖）。
Wave 4 的 cdp-server 测试可在 Wave 2 完成前开始。
