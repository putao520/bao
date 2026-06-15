# TASK-12 — 全链路 E2E 测试索引

bao 系统全链路端到端测试(JS → SpiderMonkey → servo → CDP)。

## 为什么测试分散在多个 crate 的 tests/

Rust workspace 的集成测试**必须**位于某个 crate 的 `tests/` 目录下(每个
`.rs` 文件编译为独立的 test binary)。没有"workspace 级 tests/"原生概念。

bao 的 E2E 测试需要访问跨 crate 的 API,所以每个测试放在能最自然 import
所需 API 的 crate:

| 测试文件 | 所在 crate | 验证维度 |
|---------|-----------|---------|
| `js_context_fusion_tests.rs` | `bao_engine` | JsContext 融合(Node API + Web API 共存) |
| `dom_node_interop_tests.rs` | `bao_browser` | DOM ↔ Node.js 对象互操作 |
| `servo_render_pipeline_tests.rs` | `bao_browser` | servo 真渲染链路(navigate → DOM → screenshot) |
| `bao_cli_e2e_tests.rs` | `bao_browser` | bao CLI 端到端(std::process::Command 子进程) |
| `cdp_full_chain_tests.rs` | `bao_cdp_client` | CDP memory://bao 端到端往返 |

## 运行约束

- **mozjs Runtime + servo Opts 是 per-process 单例** — 每个测试文件的断言
  合并到单个 `#[test]` 函数,避免多次 init/destroy 造成 segfault
- **真·servo 链路默认运行** — 用 `data:` URL 或本地 HTTP server,不依赖外网
- **网络测试** — `#[ignore]` + `BAO_TEST_NETWORK=1` 启用
  - `servo_render_pipeline_network_example_com` — navigate https://example.com
  - `bao_cli_browser_subcommand_starts` — bao browser 子命令 + CDP server

## 运行命令

```bash
# 所有 E2E 测试(默认 + ignored)
cargo test -p bao_engine --test js_context_fusion_tests
cargo test -p bao_browser --test dom_node_interop_tests
cargo test -p bao_browser --test servo_render_pipeline_tests
cargo test -p bao_browser --test bao_cli_e2e_tests
cargo test -p bao_cdp_client --test cdp_full_chain_tests

# 包含网络测试(需要联网)
BAO_TEST_NETWORK=1 cargo test -- --ignored

# 全 workspace
cargo test --workspace
```

## 验收维度

| 维度 | 测试 | 状态 |
|------|------|------|
| JSContext 融合(Node API + Web API 共存) | `js_context_fusion` | ✓ |
| DOM ↔ Node 互操作 ≥3 场景 | `dom_node_interop` §2/§3/§4/§5/§6 | ✓ |
| servo 真渲染链路(navigate → DOM → screenshot) | `servo_render_pipeline` | ✓ |
| bao CLI 端到端(bao run script) | `bao_cli_e2e` §1-§7 | ✓ |
| CDP memory://bao 往返 | `cdp_full_chain` §1-§10 | ✓ |

## @trace 标注

所有测试函数含 `// @trace REQ-XXX [level:e2e]` 标注,关联 SPEC REQ:

- `REQ-ENG-001` (SpiderMonkey JsContext)
- `REQ-ENG-006` (CLI runtime)
- `REQ-CLI-001/002` (bao CLI)
- `REQ-BRW-001/002/003` (servo 集成)
- `REQ-CDP-001~008` (CDP)
- `REQ-BAO-API-001/002/003` (CDP client API)
- `REQ-SEC-002` (Realm 隔离)

AAA 模式:每个子断言都有 Arrange / Act / Assert 注释分隔。
