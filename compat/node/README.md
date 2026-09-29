# Node.js API Compatibility

> **Honesty-first.** 无实测的模块一律输出「分母/分子」,**不造通过率数字**。
> 全量 72 模块盘点 SSOT:**[INVENTORY.md](./INVENTORY.md)**(上游锚 Node **v24.19.0**,72 builtin,总 exports **1402**;bao 名字级代理实现 **955 = 68.1%**)。

## 范围

Bao 在 SpiderMonkey 之上实现 Node.js / Bun 兼容 API。本目录对照 Node.js 上游测试规范,
公开每个模块的**真实通过率**。

## 已覆盖模块(从 `src/bao_runtime/tests/` 与 `tests/` 盘点)

下表分两类:
- **Conformance 测量值**(来自 [`node_conformance/GAP_REPORT.md`](../../src/bao_runtime/tests/node_conformance/GAP_REPORT.md),对照 Node.js/Bun 参考行为逐 check 实测)——这些是真实通过率。
- **仅有 deep_tests、未做 conformance 聚合**的模块——W9 已立分母/分子(上游 exports vs bao 实现),状态落定为 Supported/Partial(见 [INVENTORY.md](./INVENTORY.md));通过率聚合=后续波。

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

### deep_tests 模块(19 行已立分母;通过率聚合=后续波)

19 个「tests exist, not aggregated」模块已全部立起分母(上游 exports 数)与分子
(bao 侧名字级实现数)——逐行见 [INVENTORY.md](./INVENTORY.md) 72 模块表。
测试文件佐证保留如下:

| Module | 测试文件存在 | 状态(按 INVENTORY) |
|--------|:---:|-------|
| `child_process` | ✓ `child_process_deep_tests.rs`, `child_process_vm_module_tests.rs` | Supported |
| `dgram` | ✓ `node_dgram_inspector_deep_tests.rs` | Supported |
| `dns` | ✓ `dns_net_deep_tests.rs`, `node_dns_net_tests.rs` | Supported |
| `net` | ✓ `net_deep_tests.rs`, `node_dns_net_tests.rs` | Supported |
| `os` | ✓ `os_deep_tests.rs`, `node_os_util_tests.rs` | Supported |
| `process` / `env` | ✓ `process_deep_tests.rs`, `node_process_env_deep_tests.rs` | Supported |
| `querystring` | ✓ `querystring_deep_tests.rs`, `node_querystring_deep_tests.rs` | Supported |
| `readline` | ✓ `readline_deep_tests.rs`, `node_readline_deep_tests.rs` | Supported |
| `string_decoder` | ✓ `node_string_decoder_deep_tests.rs`, `strdec_module_deep_tests.rs` | Supported |
| `timers` | ✓ `timers_deep_tests.rs`, `node_timers_tests.rs`, `node_timers_module_deep_tests.rs`, `require_timers_tests.rs`, `timers_https_tls_tests.rs` | Supported |
| `tls` | ✓ `tls_deep_tests.rs` | Supported |
| `tty` | ✓ `node_tty_deep_tests.rs` | Supported |
| `vm` | ✓ `vm_deep_tests.rs`, `vm_codegen_tests.rs` | Supported |
| `worker_threads` | ✓ `node_worker_threads_deep_tests.rs` | Supported |
| `zlib` | ✓ `zlib_deep_tests.rs` | Supported |
| `async_hooks` | ✓ `node_async_hooks_deep_tests.rs` | Partial(stub module,API shape only) |
| `diagnostics_channel` | ✓ `node_diagnostics_channel_deep_tests.rs` | Partial(stub module) |
| `perf_hooks` | ✓ `node_perf_hooks_deep_tests.rs` | Supported |
| `module` | ✓ `node_module_deep_tests.rs`, `esm_import_deep_tests.rs`, `require_deep_tests.rs`, `require_system_deep_tests.rs`, `test_module_resolution.js`, `test_node_modules.js`, `npm_project_e2e_tests.rs`, `test_dynamic_import.js` | Supported |

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

## 覆盖缺口(W9 盘点后)

全量 72 模块实现文件齐(模块级 Unsupported=0);语义面窄的模块逐行见
[INVENTORY.md](./INVENTORY.md)(如 repl 3/9、module 12/34、sys 1/34、v8 8/23、util 15/34、process 37/83)。
剩余缺口属「语义面深化」(行为测试深化=后续波),非模块缺位。

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

`bao compat node`(聚合所有 Node 模块测试,输出通过率报告):命令面设计已立
(见 [INVENTORY.md](./INVENTORY.md)「`bao compat node` 命令面设计」)——实现归 CLI 波;
数据源=INVENTORY 72 行分母/分子 + conformance 254 checks 实测。
