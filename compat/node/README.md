# Node.js API Compatibility

> **Honesty-first.** 通过率表全部 `TBD` 或 "tests exist, not aggregated",**不造数据**。

## 范围

Bao 在 SpiderMonkey 之上实现 Node.js / Bun 兼容 API。本目录对照 Node.js 上游测试规范,
公开每个模块的**真实通过率**。

## 已覆盖模块(从 `src/bao_runtime/tests/` 与 `tests/` 盘点)

下列模块**已有测试存在**(在 `src/bao_runtime/tests/` 与 `tests/`),但**尚未聚合**为统一通过率报告。

| Module | 测试文件存在 | Pass Rate | Notes |
|--------|:---:|:---:|-------|
| `assert` | ✓ `assert_deep_tests.rs`, `node_assert_deep_tests.rs`, `node_assert_util_tests.rs` | TBD | tests exist, not aggregated |
| `buffer` | ✓ `buffer_deep_tests.rs`, `buffer_module_tests.rs`, `node_buffer_tests.rs`, `test_upstream_buffer.js`, `stream_buffer_assert_tests.rs` | TBD | tests exist, not aggregated |
| `child_process` | ✓ `child_process_deep_tests.rs`, `child_process_vm_module_tests.rs` | TBD | tests exist, not aggregated |
| `crypto` | ✓ `crypto_deep_tests.rs`, `node_crypto_tests.rs`, `realworld_crypto_workflow_tests.rs`, `test_crypto_cipher.js` | TBD | tests exist, not aggregated |
| `dgram` | ✓ `node_dgram_inspector_deep_tests.rs` | TBD | tests exist, not aggregated |
| `dns` | ✓ `dns_net_deep_tests.rs`, `node_dns_net_tests.rs` | TBD | tests exist, not aggregated |
| `events` | ✓ `events_deep_tests.rs`, `events_path_deep_tests.rs`, `node_events_tests.rs`, `test_upstream_events.js` | TBD | tests exist, not aggregated |
| `fs` | ✓ `fs_deep_tests.rs`, `fs_buffer_write_tests.rs`, `node_fs_tests.rs` | TBD | tests exist, not aggregated |
| `http` / `https` | ✓ `http_https_deep_tests.rs`, `http_client_deep_tests.rs`, `node_http_tests.rs`, `test_http_depth.js` | TBD | tests exist, not aggregated |
| `net` | ✓ `net_deep_tests.rs`, `node_dns_net_tests.rs` | TBD | tests exist, not aggregated |
| `os` | ✓ `os_deep_tests.rs`, `node_os_util_tests.rs` | TBD | tests exist, not aggregated |
| `path` | ✓ `path_deep_tests.rs`, `node_path_tests.rs`, `test_upstream_path.js` | TBD | tests exist, not aggregated |
| `process` / `env` | ✓ `process_deep_tests.rs`, `node_process_env_deep_tests.rs` | TBD | tests exist, not aggregated |
| `querystring` | ✓ `querystring_deep_tests.rs`, `node_querystring_deep_tests.rs` | TBD | tests exist, not aggregated |
| `readline` | ✓ `readline_deep_tests.rs`, `node_readline_deep_tests.rs` | TBD | tests exist, not aggregated |
| `stream` | ✓ `stream_deep_tests.rs`, `node_stream_qs_tests.rs` | TBD | tests exist, not aggregated |
| `string_decoder` | ✓ `node_string_decoder_deep_tests.rs`, `strdec_module_deep_tests.rs` | TBD | tests exist, not aggregated |
| `timers` | ✓ `timers_deep_tests.rs`, `node_timers_tests.rs`, `node_timers_module_deep_tests.rs`, `require_timers_tests.rs`, `timers_https_tls_tests.rs` | TBD | tests exist, not aggregated |
| `tls` | ✓ `tls_deep_tests.rs` | TBD | tests exist, not aggregated |
| `tty` | ✓ `node_tty_deep_tests.rs` | TBD | tests exist, not aggregated |
| `url` | ✓ `url_deep_tests.rs`, `node_url_tests.rs`, `url_util_os_deep_tests.rs`, `test_upstream_url.js` | TBD | tests exist, not aggregated |
| `util` | ✓ `util_deep_tests.rs`, `test_upstream_util.js` | TBD | tests exist, not aggregated |
| `vm` | ✓ `vm_deep_tests.rs`, `vm_codegen_tests.rs` | TBD | tests exist, not aggregated |
| `worker_threads` | ✓ `node_worker_threads_deep_tests.rs` | TBD | tests exist, not aggregated |
| `zlib` | ✓ `zlib_deep_tests.rs` | TBD | tests exist, not aggregated |
| `async_hooks` | ✓ `node_async_hooks_deep_tests.rs` | TBD | tests exist, not aggregated |
| `diagnostics_channel` | ✓ `node_diagnostics_channel_deep_tests.rs` | TBD | tests exist, not aggregated |
| `perf_hooks` | ✓ `node_perf_hooks_deep_tests.rs` | TBD | tests exist, not aggregated |
| `module` | ✓ `node_module_deep_tests.rs`, `esm_import_deep_tests.rs`, `require_deep_tests.rs`, `require_system_deep_tests.rs`, `test_module_resolution.js`, `test_node_modules.js`, `npm_project_e2e_tests.rs`, `test_dynamic_import.js` | TBD | tests exist, not aggregated |

## `node_conformance/` 子目录(已有)

`src/bao_runtime/tests/node_conformance/` 已建立 conformance 骨架:

- `assert_conformance.rs`
- `buffer_conformance.rs`
- `crypto_conformance.rs`
- `events_conformance.rs`
- `fs_conformance.rs`
- `http_conformance.rs`
- `path_conformance.rs`
- `stream_conformance.rs`
- `url_conformance.rs`
- `util_conformance.rs`
- `conformance_common.rs`(共享辅助)
- `GAP_REPORT.md`(已有 gap 分析)

## TODO(尚未覆盖或覆盖不全)

- `cluster` — TBD
- `inspector` — TBD
- `repl` — TBD
- `trace_events` — TBD
- `v8` — TBD(SpiderMonkey 无 v8 API,仅能 stub 名字)
- `wasi` — TBD
- `worker_threads` 完整 NMI 行为 — TBD
- 各模块的**通过率聚合脚本** — TBD

## 跑法

```bash
# Rust 侧 Node 兼容全量
cargo test -p bao_runtime

# 仅 conformance 子集
cargo test -p bao_runtime --test '*conformance*'

# 仅单个模块
cargo test -p bao_runtime --test fs_deep_tests
```

JS 侧(tests/*.js)通过 `bao test tests/test_upstream_*.js` 跑(需要 bao 二进制)。

## 统一聚合命令

`bao compat node`(聚合所有 Node 模块测试,输出通过率报告):**TBD**,尚未实现。
