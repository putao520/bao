# Node.js API INVENTORY(G1-Node 全量盘点)

> 机械可重建;分类三态:**Supported**(实现+测试佐证)/ **Partial**(实现无专属测试或面窄)/ **Unsupported**(无实现)。
> 本表只立「分母/分子」,不造通过率数字(通过率聚合是后续波;conformance 10 模块已有真实测量,见 README)。

## 上游锚(ref 锁定)

- 上游真源:**本机 Node.js 二进制 v24.19.0**(LTS 线;`require('module').builtinModules` = **72** builtin)
- 枚举方法:`node -e` 逐模块 `Object.getOwnPropertyNames(require(m))`(try/catch 全通过,0 失败)
- 上游总 exports:**1402**(72 模块合计;含 node: 前缀特化模块 sea/sqlite/test/test-reporters 与 _http_/_stream_/_tls_ 内部子模块)
- bao 面来源:`src/bao_runtime/src/node_*.rs`(52 个实现文件,72 模块全覆盖——**模块级 Unsupported=0**)+ bun_api.rs process 面
- 分子口径:上游 export 名在对应实现文件文本中的出现(名字级代理计数;同名异义风险保留,聚合时以 conformance 实测校准)

## 72 模块表

| module | upstream_exports | bao_implemented(名字级代理) | tests | status | note |
|---|---:|---:|---|---|---|
| _http_agent | 2 | 2 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| _http_client | 1 | 1 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| _http_common | 14 | 2 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| _http_incoming | 3 | 1 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| _http_outgoing | 6 | 3 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| _http_server | 9 | 3 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| _stream_duplex | 3 | 1 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| _stream_passthrough | 0 | 0 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| _stream_readable | 6 | 2 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| _stream_transform | 0 | 0 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| _stream_wrap | 1 | 0 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| _stream_writable | 3 | 0 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| _tls_common | 3 | 1 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| _tls_wrap | 4 | 0 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| assert | 22 | 4 | node_conformance(254 checks 面) | Supported | node_conformance 实测 |
| assert/strict | 22 | 4 | 镜像:node_subpath_aliases.rs | Supported | 镜像实现(assert conformance 覆盖) |
| async_hooks | 7 | 7 | node_async_hooks_deep_tests.rs(shape only) | Partial | stub module(API shape only,无行为实现) |
| buffer | 14 | 11 | node_conformance(254 checks 面) | Supported | node_conformance 实测 |
| child_process | 9 | 8 | child_process_deep_tests.rs, child_process_vm_module_tests.rs | Supported | deep_tests 存在(通过率待聚合,分母已立) |
| cluster | 16 | 14 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| console | 27 | 22 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| constants | 235 | 144 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| crypto | 74 | 63 | node_conformance(254 checks 面) | Supported | node_conformance 实测 |
| dgram | 3 | 2 | node_dgram_inspector_deep_tests.rs | Supported | deep_tests 存在(通过率待聚合,分母已立) |
| diagnostics_channel | 6 | 6 | node_diagnostics_channel_deep_tests.rs(shape only) | Partial | stub module(API shape only,无行为实现) |
| dns | 50 | 48 | dns_net_deep_tests.rs, node_dns_net_tests.rs | Supported | deep_tests 存在(通过率待聚合,分母已立) |
| dns/promises | 46 | 44 | 镜像:node_dns.rs | Supported | 镜像实现(dns conformance 覆盖) |
| domain | 5 | 3 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| events | 15 | 11 | node_conformance(254 checks 面) | Supported | node_conformance 实测 |
| fs | 108 | 100 | node_conformance(254 checks 面) | Supported | node_conformance 实测 |
| fs/promises | 33 | 32 | 镜像:node_fs.rs | Supported | 镜像实现(fs conformance 覆盖) |
| http | 21 | 15 | node_conformance(254 checks 面) | Supported | node_conformance 实测 |
| http2 | 11 | 10 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| https | 6 | 6 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| inspector | 9 | 6 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| inspector/promises | 9 | 6 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| module | 34 | 12 | node_module_deep_tests.rs, esm_import_deep_tests.rs, require_deep_tests.rs, require_system_deep_tests.rs | Supported | deep_tests 佐证(通过率待聚合) |
| net | 18 | 9 | net_deep_tests.rs, node_dns_net_tests.rs | Supported | deep_tests 存在(通过率待聚合,分母已立) |
| node:sea | 5 | 0 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| node:sqlite | 5 | 1 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| node:test | 17 | 14 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| node:test/reporters | 5 | 1 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| os | 23 | 22 | os_deep_tests.rs, node_os_util_tests.rs | Supported | deep_tests 存在(通过率待聚合,分母已立) |
| path | 17 | 16 | node_conformance(254 checks 面) | Supported | node_conformance 实测 |
| path/posix | 17 | 16 | 镜像:node_path.rs | Supported | 镜像实现(path conformance 覆盖) |
| path/win32 | 17 | 16 | 镜像:node_path.rs | Supported | 镜像实现(path conformance 覆盖) |
| perf_hooks | 13 | 6 | node_perf_hooks_deep_tests.rs | Supported | deep_tests 佐证(通过率待聚合) |
| process | 83 | 37 | process_deep_tests.rs, node_process_env_deep_tests.rs | Supported | deep_tests 存在(通过率待聚合,分母已立) |
| punycode | 6 | 6 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| querystring | 7 | 6 | querystring_deep_tests.rs, node_querystring_deep_tests.rs | Supported | deep_tests 存在(通过率待聚合,分母已立) |
| readline | 8 | 8 | readline_deep_tests.rs, node_readline_deep_tests.rs | Supported | deep_tests 存在(通过率待聚合,分母已立) |
| readline/promises | 3 | 2 | 镜像:node_readline.rs | Supported | 镜像实现(readline conformance 覆盖) |
| repl | 9 | 3 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| stream | 23 | 12 | node_conformance(254 checks 面) | Supported | node_conformance 实测 |
| stream/consumers | 6 | 6 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| stream/promises | 2 | 2 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| stream/web | 18 | 17 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| string_decoder | 1 | 1 | node_string_decoder_deep_tests.rs, strdec_module_deep_tests.rs | Supported | deep_tests 存在(通过率待聚合,分母已立) |
| sys | 34 | 1 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| timers | 7 | 7 | timers_deep_tests.rs, node_timers_tests.rs, node_timers_module_deep_tests.rs | Supported | deep_tests 存在(通过率待聚合,分母已立) |
| timers/promises | 4 | 4 | 镜像:node_timers_module.rs | Supported | deep_tests 存在(通过率待聚合,分母已立) |
| tls | 19 | 11 | tls_deep_tests.rs | Supported | deep_tests 佐证(通过率待聚合) |
| trace_events | 2 | 2 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| tty | 3 | 3 | node_tty_deep_tests.rs | Supported | deep_tests 佐证(通过率待聚合) |
| url | 14 | 10 | node_conformance(254 checks 面) | Supported | node_conformance 实测 |
| util | 34 | 15 | node_conformance(254 checks 面) | Supported | node_conformance 实测 |
| util/types | 43 | 40 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| v8 | 23 | 8 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| vm | 10 | 7 | vm_deep_tests.rs, vm_codegen_tests.rs | Supported | deep_tests 佐证(通过率待聚合) |
| wasi | 1 | 1 | — | Partial | 实现存在,无专属 conformance/deep 测试 |
| worker_threads | 21 | 15 | node_worker_threads_deep_tests.rs | Supported | deep_tests 佐证(通过率待聚合) |
| zlib | 47 | 36 | zlib_deep_tests.rs | Supported | deep_tests 佐证(通过率待聚合) |

## 汇总

- **Supported 34 / Partial 38 / Unsupported 0**(合计 72)
- 分母/分子已立:上游总 exports **1402**,bao 名字级代理实现 **955(68.1%)**——「pass rate 聚合」的数据基础(分母=上游,分子=bao 侧名字级;真实通过率以 conformance 逐 check 实测为准,不以此代理数冒充)
- 19 个「tests exist, not aggregated」模块:上表 Supported 中标 deep_tests 的 10 行 + 镜像/子路径行;分母(上游 exports)已逐模块立起

## 254 checks 的上游语义锚

compat/node README 的 10 模块 conformance(254 checks / 5 gaps = 98.0%)语义锚 =
**Node v24.19.0 官方行为/文档**(本机二进制实测对照;buffer/path/fs/url/events/assert/util/stream/http/crypto)。

## crypto 5 gap 如实标注(P3 证伪记忆)

- X509:**P3 已证伪为 link 组成伪影**(e3.exe link 面产物,非真实实现缺口;8d965656 后已灭)——gap 记账保留,语义=「无 Bun.x509 等价面」,非编译性缺陷
- ECDH / hkdf / DH / HMAC-MD5:高级原语未实现(真实 gap,非义务硬缺口)
- 处置:按需逐项立卡,不在本 inventory 展开

## `bao compat node` 命令面设计(本合同只设计,不入码)

- 形态:`bao compat node [--module <name>] [--json]`
- 数据源:本表(行契约 `module | upstream_exports | bao_implemented | tests | status`)+ conformance 实测(GAP_REPORT/254 checks)
- 输出:逐模块 `module: implemented/total (status)` + 总骨架(数字仅来自实测;无实测模块输出 `denominator-only`)
- CLI 波消费:本表格式即机器可读性合同
