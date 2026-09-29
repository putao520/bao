# CDP Compatibility

> **Honesty-first.** 无实测的维度一律立「分母/分子」或事实矩阵,**不造通过率数字**。
> 全量盘点 SSOT:**[INVENTORY.md](./INVENTORY.md)**(协议版本锁定 **cdp-protocol 0.3.1**,54 域 / 662 methods + 事件面;协议锚=crate 类型面)。

## 范围

Bao 内置 CDP Server,支持 12 个域。本目录对照 CDP spec 与 Playwright/Puppeteer 调用面,
公开每个域的方法覆盖率与通过率。

## 12 域(README.md Compatibility Matrix)

| Domain | Bao 状态 | methods | INVENTORY 判定分布 | Notes |
|--------|:---:|:---:|---|-------|
| `Page` | ✓ | 61 | S 8+ack3 / P 0 / EU 2 / U 48 | navigate/lifecycle/screenshot 基础覆盖 |
| `Runtime` | ✓ | 23 | S 6+ack4 / P 0 / EU 0 / U 13 | evaluate / callFunctionOn / object protocol |
| `DOM` | ✓ | 53 | S 11 / P 0 / EU 1 / U 41 | getDocument/flatten/selector/attribute(W8 深化后) |
| `Network` | ✓ | 40 | S 14+ack2 / P 0+ackonly3 / EU 18 / U 3 | request/response 观察;EU=无基础设施面(逐条 -32000 裁决) |
| `Debugger` | ✓(SM 原生面 12 method Supported) | 33 | S 12 / P 2(ack-only) / EU 1 / U 18 | 逐 method 缺口清单见 INVENTORY #11-B 节 |
| `Input` | ✓ | 13 | S 3+ack2 / P 0 / EU 1 / U 7 | dispatchKey/Mouse;touch 无投递面(EU) |
| `Emulation` | ✓ | 46 | S 2+ack1 / P 0+ackonly6 / U 37 | viewport / UA override;上游深面未接 |
| `CSS` | ✓ | 37 | S 6 / P 0 / EU 0 / U 31 | getComputedStyle/getInlineStyles/getMatchedStyles/setStyleTexts(W8 真数据) |
| `Overlay` | ✓ | 30 | S 0+ack2 / P 0+ackonly1 / U 27 | highlight ack 面;渲染 overlay 无 |
| `Log` | ✓ | 5 | S 0+ack2 / P 0+ackonly2 / U 3 | entryAdded 经 console 桥;clear ack |
| `Fetch` | ✓(显式拒绝) | 9 | S 0 / EU 1 / U 8 | 无请求拦截设施(显式 -32000 裁决面) |
| `Target` | ✓ | 19 | S 4+ack2 / P 0 / EU 3 / U 10 | createTarget/attach(会话表)/close |


## 已有 CDP 测试(从 `src/bao_cdp/tests/` 盘点)

下列测试**存在**,但**尚未聚合**为按域维度的通过率报告。

- `protocol_all_domains_internal_backend_tests.rs`
- `protocol_domain_handler_deep_tests.rs`
- `protocol_message_deep_tests.rs`
- `protocol_edge_case_tests.rs`
- `protocol_serialize_boundary_tests.rs`
- `protocol_subcommand_full_coverage_tests.rs`
- `router_backend_deep_tests.rs`
- `router_external_detach_edge_tests.rs`
- `router_lifecycle_tests.rs`
- `router_session_internal_backend_deep_tests.rs`
- `router_session_lifecycle_deep_tests.rs`
- `session_lifecycle_deep_tests.rs`
- `cdp_types_deep_tests.rs`
- `domain_handler_response_field_boundary_tests.rs`
- `domain_stress_tests.rs`
- `bridge_channel_*_tests.rs`(bridge 通道,多文件)
- `backend_bridge_channel_*_tests.rs`
- `perf_refactor_integration_tests.rs`

## 客户端兼容(Playwright / Puppeteer)

| Client | 状态 | Pass Rate | Notes |
|--------|:---:|:---:|-------|
| Playwright (`chromium.connectOverCDP`) | **Experimental** | 分母/分子见 INVENTORY | connect / newPage / goto / evaluate / screenshot 已验证;完整生命周期=后续波 |
| Puppeteer | **Experimental** | — | README.md 标 Experimental |
| `bao_cdp_client::Browser` (Rust native) | ✓ | 事实矩阵见 INVENTORY | memory:// 与 ws:// 双 scheme |

## TODO(高优)

- `Debugger` 域补全(README.md 已标 Partial)
- `Network` 域 headers/cookies 完整性
- Playwright/Puppeteer 完整 connect→navigate→eval→screenshot→close lifecycle 回归
- CDP method 覆盖率聚合脚本(对照 CDP spec JSON)
- Bridge channel 压测 / 超时 / detach 边界(已有测试,需聚合)

## 上游对照

- CDP spec (Chrome DevTools Protocol): <https://chromedevtools.github.io/devtools-protocol/>
- Playwright CDP 调用面: <https://playwright.dev/docs/api/class-browserserver>
- Puppeteer: <https://pptr.dev/api/>

## 跑法

```bash
cargo test -p bao_cdp
cargo test -p bao_cdp_client
```

## 统一聚合命令

`bao compat cdp`(按 12 域输出 method 覆盖矩阵):数据源已立——INVENTORY 662-method
事实矩阵即机器可读合同;实现归 CLI 波(镜像 `bao compat node` 设计)。
