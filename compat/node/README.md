# Node.js API Compatibility

> **Honesty-first.** 通过率表全部 `TBD` 或 "tests exist, not aggregated",**不造数据**。

## 范围

Bao 在 SpiderMonkey 之上实现 Node.js / Bun 兼容 API。本目录对照 Node.js 上游测试规范,
公开每个模块的**真实通过率**。

## 已覆盖模块(从 `src/bao_runtime/tests/` 与 `tests/` 盘点)

下表分两类:
- **Conformance 测量值**(来自 [`node_conformance/GAP_REPORT.md`](../../src/bao_runtime/tests/node_conformance/GAP_REPORT.md),对照 Node.js/Bun 参考行为逐 check 实测)——这些是真实通过率。
- **仅有 deep_tests、未做 conformance 聚合**的模块——标 `TBD`,诚实保留。

> 数据来源:`src/bao_runtime/tests/node_conformance/GAP_REPORT.md`(TASK-16d 收口)。Conformance % = 通过的 implemented checks / (implemented checks);gap 数 = 已知未实现的 Node-API(TASK-16d 已清零 9 个模块的 API-shape gap,仅 crypto 留 5 个高级原语)。

### Conformance 测量值(10 模块,实测)

| Module | Implemented checks | Node-API gaps | Conformance % | Notes |
|--------|-------------------:|--------------:|--------------:|-------|
| `buffer` | 39 | 0 | **100%** | — |
| `path` | 32 | 0 | **100%** | incl. win32, matchesGlob |
| `fs` | 23 | 0 | **100%** | incl. watch/cp |
| `url` | 17 | 0 | **100%** | incl. pathToFileURL/domainTo* |
| `events` | 26 | 0 | **100%** | incl. defaultMaxListeners/errorMonitor |
| `assert` | 25 | 0 | **100%** | incl. strict |
| `util` | 26 | 0 | **100%** | incl. styleText/isDeepStrictEqual/promisify |
| `stream` | 12 | 0 | **100%** | — |
| `http` | 15 | 0 | **100%** | — |
| `crypto` | 29 | 5 | **~85%** | gaps: X509/ECDH/hkdf/DH/HMAC-MD5(高级原语,非 TASK-16d 范围) |

**10 模块 conformance 合计:254 checks / 5 gaps = 98.0%**(按 implemented check 计;5 gap 是 crypto 的高级原语未实现)。

### 仅有 deep_tests、未做 conformance 聚合(TBD)

下列模块**已有 deep_tests 存在**,但未跑 conformance 逐 check 对照,通过率待聚合:

| Module | 测试文件存在 | Pass Rate | Notes |
|--------|:---:|:---:|-------|
| `child_process` | ✓ `child_process_deep_tests.rs`, `child_process_vm_module_tests.rs` | TBD | tests exist, not aggregated |
| `dgram` | ✓ `node_dgram_inspector_deep_tests.rs` | TBD | tests exist, not aggregated |
| `dns` | ✓ `dns_net_deep_tests.rs`, `node_dns_net_tests.rs` | TBD | tests exist, not aggregated |
| `net` | ✓ `net_deep_tests.rs`, `node_dns_net_tests.rs` | TBD | tests exist, not aggregated |
| `os` | ✓ `os_deep_tests.rs`, `node_os_util_tests.rs` | TBD | tests exist, not aggregated |
| `process` / `env` | ✓ `process_deep_tests.rs`, `node_process_env_deep_tests.rs` | TBD | tests exist, not aggregated |
| `querystring` | ✓ `querystring_deep_tests.rs`, `node_querystring_deep_tests.rs` | TBD | tests exist, not aggregated |
| `readline` | ✓ `readline_deep_tests.rs`, `node_readline_deep_tests.rs` | TBD | tests exist, not aggregated |
| `string_decoder` | ✓ `node_string_decoder_deep_tests.rs`, `strdec_module_deep_tests.rs` | TBD | tests exist, not aggregated |
| `timers` | ✓ `timers_deep_tests.rs`, `node_timers_tests.rs`, `node_timers_module_deep_tests.rs`, `require_timers_tests.rs`, `timers_https_tls_tests.rs` | TBD | tests exist, not aggregated |
| `tls` | ✓ `tls_deep_tests.rs` | TBD | tests exist, not aggregated |
| `tty` | ✓ `node_tty_deep_tests.rs` | TBD | tests exist, not aggregated |
| `vm` | ✓ `vm_deep_tests.rs`, `vm_codegen_tests.rs` | TBD | tests exist, not aggregated |
| `worker_threads` | ✓ `node_worker_threads_deep_tests.rs` | TBD | tests exist, not aggregated |
| `zlib` | ✓ `zlib_deep_tests.rs` | TBD | tests exist, not aggregated |
| `async_hooks` | ✓ `node_async_hooks_deep_tests.rs` | TBD | stub module(API shape only) |
| `diagnostics_channel` | ✓ `node_diagnostics_channel_deep_tests.rs` | TBD | stub module |
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
