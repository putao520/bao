# CDP Compatibility

> **Honesty-first.** 通过率表全部 `TBD`,**不造数据**。

## 范围

Bao 内置 CDP Server,支持 12 个域。本目录对照 CDP spec 与 Playwright/Puppeteer 调用面,
公开每个域的方法覆盖率与通过率。

## 12 域(README.md Compatibility Matrix)

| Domain | Bao 状态 | Pass Rate | Notes |
|--------|:---:|:---:|-------|
| `Page` | ✓ | TBD | navigate / lifecycle / screenshot 基础覆盖 |
| `Runtime` | ✓ | TBD | evaluate / callFunctionOn / console |
| `DOM` | ✓ | TBD | getDocument / querySelector / attribute |
| `Network` | ✓ | TBD | request/responseObserver;headers/cookies completeness TBD |
| `Debugger` | **Partial** | TBD | 已知 Partial(见 README.md) |
| `Input` | ✓ | TBD | dispatchKey/Mouse;合成事件 |
| `Emulation` | ✓ | TBD | viewport / userAgent override |
| `CSS` | ✓ | TBD | getInlineStyles / getMatchedStyles |
| `Overlay` | ✓ | TBD | highlight node |
| `Log` | ✓ | TBD | entryAdded / clear |
| `Fetch` | ✓ | TBD | requestPaused / continueResponse |
| `Target` | ✓ | TBD | createTarget / attachToTarget |

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
| Playwright (`chromium.connectOverCDP`) | **Experimental** | TBD | connect / newPage / goto / evaluate / screenshot 已验证(README.md);完整生命周期 TBD |
| Puppeteer | **Experimental** | TBD | README.md 标 Experimental |
| `bao_cdp_client::Browser` (Rust native) | ✓ | TBD | memory:// 与 ws:// 双 scheme |

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

`bao compat cdp`(按 12 域输出 method 覆盖矩阵):**TBD**,尚未实现。
