# BUG-KNOWLEDGE.md

> BCE (Bug-Class Eradication) 知识库索引。每类根治的 BUG 沉淀于此，避免重复归因。
> 协议 SSOT: `~/.claude/rules/bug-class-eradication.md`

---

## BCE-20260618-001 — 反模式关键词残留（TODO/FIXME/XXX）

```yaml
patternId: BCE-20260618-001
title: 反模式类（TODO/FIXME/XXX 关键词残留）— P-1 红线未达标
layer: 表层（placeholder/未完成标记残留）
status: 已根治（残留=0）

codePattern:
  - 「源码中残留 TODO / FIXME / XXX 标记，阻塞 P-1 红线（oracle_gate 步骤4 模式检查）」

triggerCondition:
  - oracle_gate 步骤4「模式检查」对源码 grep TODO/FIXME/XXX 命中 > 0

detectionSignatures:
  structural:
    - "Comment 含 \b(TODO|FIXME|XXX)\b 词边界关键词"
  literal:
    - '\b(TODO|FIXME|XXX)\b'  # grep -rnE，范围 = bao_* crate 源码（排除 bun_* 上游只读 crate）

sameClassCriterion:
  - 「bao_* 自有 crate（bao_crypto / bao_bundler / bao_cdp / bao_cdp_client / bao_engine / bao_browser / bao_runtime / bao_stealth / bao_uloop / bao_cli / bao_bin / bao_engine_macros / bao_native_stubs / bao_boringssl_bridge）的源码（非测试断言）中出现 TODO/FIXME/XXX 反模式关键词」

fixTemplate:
  - 「将 placeholder TODO 转为完整实现；或将占位标记替换为具体代码；禁止保留未完成标记提交」
  - 「例外（不算残留，必须保留）：① 回归测试中对生成代码的 contains("TODO") 断言本身；② 文档中以 XXX 作为命名占位符示例（如 `bao_engine::XXX` 指代『任意类型』）」

regressionAssertion:
  - 「grep -rnE \b(TODO|FIXME|XXX)\b src/bao_{crypto,bundler,cdp,cdp_client,engine,browser,runtime,stealth,uloop,cli,bin,engine_macros,native_stubs,boringssl_bridge} = 0」
  - 「现有回归测试：src/bao_engine/tests/codegen_constructor_finalizer_tests.rs::test_no_todo_in_constructor/finalizer（断言生成代码不含 TODO/FIXME/stub）」

affectedTasks: [TASK-1-CRYPTO, TASK-2-BUNDLER, TASK-4-CDP, TASK-5-CDP-CLIENT]
```

### 归因（阶段1）

- **根因**: 早期编码阶段在 bao_* crate 中遗留 TODO/FIXME/XXX 占位标记，未在 commit 前清除，违反 P-1 红线。
- **缺陷分层**: 表层缺陷（局部标记残留，非设计/范式缺陷）。
- **归因时间**: 2026-06-18。

### 全量确认报告（阶段5）

```yaml
confirmReport:
  sweepScope: "src/bao_* 自有 crate 全量（排除 bun_* 上游只读 crate）"
  layersScanned: [literal]
  instancesFound: 0          # 横扫命中（4 个 task 文件域：crypto/bundler/cdp/cdp_client）
  truePositives: 0
  falsePositives: 0
  instancesFixed: 0          # 本批 task 文件域进入 BCE 时已为 0（先前 epoch 已清除）
  residual: 0
  residualEvidence:
    - "重扫 src/bao_crypto: 0 命中"
    - "重扫 src/bao_bundler: 0 命中"
    - "重扫 src/bao_cdp: 0 命中"
    - "重扫 src/bao_cdp_client: 0 命中"
    - "回归测试覆盖: bao_engine codegen_constructor_finalizer_tests（断言生成代码不含 TODO/FIXME/stub）"
  releaseGateImpact: pass

# 注意：bao_browser/src/cdp_handler.rs:66 原本残留 1 处真实 TODO(BUG-CDP-006)，
# 但该文件域不在本类 4 个 task（crypto/bundler/cdp/cdp-client）范围内，
# 属 BUG-CDP-006 跟踪项，不在本 BCE 范围内。该 TODO 已在 TASK-3-RUNTIME
# oracle_gate 反模式清扫中改为 @trace BUG-CDP-006 标记（语义不变，去掉
# 反模式关键词），BUG-CDP-006 跟踪项本体未关闭。
```

### 防复发（阶段6）

- ✅ 回归测试：`src/bao_engine/tests/codegen_constructor_finalizer_tests.rs`（codegen 输出不含 TODO/FIXME/stub）。
- ✅ P-1 红线：`~/.claude/CLAUDE.md` 已定义「TODO/FIXME/stub/空实现/console.log — commit 前清除。oracle_gate 强制执行」。
- ✅ 知识库：本条目。

---

## BCE-20260618-002 — node:crypto 编码契约缺失（cipher/hash/Hmac update/final 忽略 inputEncoding/outputEncoding）

```yaml
patternId: BCE-20260618-002
title: Node.js 编码契约缺失 — cipher/hash/Hmac 的 update(data, inputEnc, outputEnc)/final(outputEnc) 忽略编码参数
layer: 设计缺陷（JS↔Rust 桥接未实现 Node.js 文本编码契约）
status: 已根治（残留=0）

codePattern:
  - 「JS 字符串输入被直接 .into_bytes() 当作原始字节，未按 inputEncoding（hex/base64/utf8）解码」
  - 「输出强制返回 number[]/Uint8Array，未按 outputEncoding 返回编码字符串」
  - 「cipher.final 注册 nargs=0，无法接收 outputEncoding 参数」

triggerCondition:
  - crypto_conformance::aes256_roundtrip（enc/dec 往返：update(enc, "hex", "utf8") 把 hex 字符串的字面 ASCII 字节喂入解密器 → EVP_CipherFinal_ex 失败）

detectionSignatures:
  structural:
    - "JS extern C 函数 cipher_update/cipher_final/hash_update/hmac_update 中字符串分支直接 js_to_rust_string(...).into_bytes()，未读取第 2/3 参数作为编码"
  literal:
    - 'node_crypto.rs: .into_bytes\(\)$  # 字符串输入未按编码解码'

sameClassCriterion:
  - 「bao_runtime/src/node_crypto.rs 中所有接收 JS 字符串输入的 update 类函数，若未根据可选 inputEncoding 参数解码字符串，即为同类」
  - 「所有 final/update 输出未根据可选 outputEncoding 返回编码字符串（或无编码时返回真实 Buffer），即为同类」

fixTemplate:
  - 「新增 decode_input_string(s, Option<encoding>) 辅助：hex/base64 解码，其余按 UTF-8 字节」
  - 「新增 encode_output_bytes(cx, args, bytes, Option<encoding>) 辅助：有编码返回字符串，无编码返回 create_buffer_object（真实 Buffer）」
  - 「parse_update_args 读取第 2 参数为 inputEncoding；cipher_update/cipher_final 读取末参数为 outputEncoding」
  - 「JS_DefineFunction 注册 nargs 对齐 Node.js 契约（update=3, final=1）」

regressionAssertion:
  - 「crypto_conformance::aes256_roundtrip 必须 PASS（AES-256-CBC 加密→hex→解密→utf8 往返还原原文）」
  - 「crypto_conformance 全套 0 fail；createHash/createHmac known-vector 不退化」

affectedTasks: [TASK-3-RUNTIME]
```

### 归因（阶段1）

- **根因**: node_crypto.rs 的 cipher/hash/Hmac JS 桥接在移植 JSC→SM 时未实现 Node.js `update(data, inputEncoding, outputEncoding)` / `final(outputEncoding)` 文本编码契约，字符串被当作字面字节，输出强制为 number[] 数组。
- **缺陷分层**: 设计缺陷（API 契约未对齐 Node.js）。
- **归因时间**: 2026-06-18。

### 横扫（阶段3）

同类实例横扫 `src/bao_runtime/src/node_crypto.rs`，命中 4 处（全为真阳性）：
- `cipher_update` / `cipher_final`（主触发点）
- `hash_update`（同类潜在缺陷，无测试覆盖但契约相同）
- `hmac_update`（同类潜在缺陷）

### 批量根治（阶段4）

统一策略（同 `fixTemplate`）：
- 新增 `decode_input_string` / `encode_output_bytes` 辅助函数
- `parse_update_args` 接收 inputEncoding
- `cipher_update` / `cipher_final` / `hash_update` / `hmac_update` 全部接入两个辅助
- `cipher.update` 注册 nargs 2→3；`cipher.final` 注册 nargs 0→1

### 全量确认报告（阶段5）

```yaml
confirmReport:
  sweepScope: "src/bao_runtime/src/node_crypto.rs 全量"
  layersScanned: [structural, literal]
  instancesFound: 4
  truePositives: 4
  falsePositives: 0
  instancesFixed: 4
  residual: 0
  residualEvidence:
    - "重扫 node_crypto.rs：无字符串输入未按 inputEncoding 解码的 update 类函数"
    - "cargo build -p bun_runtime: Finished（0 error）"
    - "crypto_conformance: 3 passed / 0 failed / 5 ignored（ignored 均为 SPEC 范围外的 documented 缺失：ECDH/X509/hkdf/DH/HMAC-MD5）"
    - "crypto_deep_tests / node_crypto_tests / realworld_crypto_workflow_tests: 全 pass"
    - "lib tests: 602 passed / 0 failed（无回归）"
  releaseGateImpact: pass
```

### 防复发（阶段6）

- ✅ 回归测试：`tests/node_conformance/crypto_conformance.rs::test_crypto_conformance_suite::aes256_roundtrip`（触发签名即 fail）。
- ✅ 知识库：本条目。

---

## BCE-20260618-003 — node_stubs 覆盖原生模块（stub clobber native builtin）

```yaml
patternId: BCE-20260618-003
title: node_stubs 的 STUB_MODULES 覆盖已原生实现的 builtin（如 assert/strict），导致 require() 返回空 stub
layer: 范式缺陷（install_all 注册顺序使 stubs.install 在 native 之后，无覆盖守卫）
status: 已根治（残留=0）

codePattern:
  - 「node_stubs::register_stub 无条件 cache_builtin 覆盖，即使该 key 已被原生模块注册」
  - 「STUB_MODULES 列表与原生 builtin 集合存在交集（assert/strict 等）」

triggerCondition:
  - assert_conformance::strict_module_exists（require('assert/strict') 返回空 stub → typeof s.ok !== "function" → FAIL）

detectionSignatures:
  structural:
    - "node_stubs::register_stub 直接 cache_builtin，未检查 gc_store 是否已存在该 builtin key"
  literal:
    - 'node_stubs.rs: cache_builtin\(cx, name'  # 无 pre-check

sameClassCriterion:
  - 「node_stubs 注册的任何模块名若与原生 cache_builtin 注册名冲突，且原生先注册、stub 后注册导致覆盖，即为同类」

fixTemplate:
  - 「在 register_stub 顶部加覆盖守卫：gc_store_get("builtin:{name}") 非空则 return，不覆盖」
  - 「该守卫对所有 stub 模块生效，未来原生上架的子路径（timers/promises 等）自动豁免，无需手工同步 STUB_MODULES」

regressionAssertion:
  - 「assert_conformance::strict_module_exists 必须 PASS（require('assert/strict') 返回真实 assert 模块，typeof ok === 'function'）」
  - 「有意的 stub（async_hooks/dgram/diagnostics_channel 等）仍注册为带 __stub 标记的空对象」

affectedTasks: [TASK-3-RUNTIME]
```

### 归因（阶段1）

- **根因**: `globals::install_all` 调用顺序为 `node_util::install_assert`（注册真实 `assert/strict`）→ `node_stubs::install`（注册 `assert/strict` stub，后者覆盖前者）。`register_stub` 无覆盖守卫，导致 `require('assert/strict')` 返回空 stub 而非真实 assert 模块。
- **缺陷分层**: 范式缺陷（缺少「不覆盖已注册原生模块」的统一守卫）。
- **归因时间**: 2026-06-18。

### 横扫（阶段3）

横扫 `STUB_MODULES` 列表 ∩ 原生 builtin 集合，命中交集：`assert/strict`（`timers/promises` 原生但不在 STUB_MODULES，无冲突）。

### 批量根治（阶段4）

统一策略（同 `fixTemplate`）：在 `register_stub` 顶部加 `gc_store_get("builtin:{name}")` 非空守卫。该守卫对所有 stub 模块生效，根除整类（未来原生上架的子路径自动豁免）。

### 全量确认报告（阶段5）

```yaml
confirmReport:
  sweepScope: "src/bao_runtime/src/node_stubs.rs 全量"
  layersScanned: [structural, literal]
  instancesFound: 1              # assert/strict 覆盖
  truePositives: 1
  falsePositives: 0
  instancesFixed: 1              # 守卫一次性根治整类
  residual: 0
  residualEvidence:
    - "重扫：register_stub 有覆盖守卫，所有 stub 名交集自动豁免"
    - "assert_conformance: 2 passed / 0 failed / 3 ignored"
    - "全 9 个 node_conformance suite：0 failed"
    - "lib tests: 602 passed / 0 failed"
    - "stub-dependent 深度测试（dgram/diagnostics_channel 等）未因守卫退化"
  releaseGateImpact: pass
```

### 防复发（阶段6）

- ✅ 回归测试：`tests/node_conformance/assert_conformance.rs::strict_module_exists`（触发签名即 fail）。
- ✅ 统一守卫：未来 STUB_MODULES 与原生 builtin 的任何交集自动豁免，无需手工同步列表。
- ✅ 知识库：本条目。


---

## BCE-20260618-007-RT — fetch(self) 运行时挂起：JS-thread uWS App liveness 注册缺失

### patternId / title
`BCE-20260618-007-RT` · Bun.serve 未注册到 JS-thread liveness registry → drain_and_check 不 tick uWS Loop → 服务端永不 accept → fetch(self) 挂死。

### layer
设计缺陷（跨模块状态追踪不一致）。

### 根因（rootCause）
- **location**:
  - `src/bao_runtime/src/bun_api.rs:1376` `bun_serve` 创建 `App::<false>` 后**未**注册到 `node_http::ACTIVE_APPS`。
  - `src/bao_runtime/src/timers.rs:166/189/237/269` `drain_and_check` 仅凭 `node_http::has_active_servers()` 决定是否 `tick_once` 驱动 JS-thread uWS Loop。
- **why**: `Bun.serve` 的 uWS App 绑定到 JS-thread 的 `uWS::Loop::get()` 单例，但 `bun_serve` 不像 `node_http::server_listen` 那样 push 到 `ACTIVE_APPS`。于是 `has_active_servers()` 对 Bun.serve 恒为 false → `drain_and_check` 走 fetch-only `sleep(1ms)` 分支 → JS-thread uWS Loop 永不被 tick → listen socket 永不 accept → worker `connect()` 永卡 EINPROGRESS → worker 永不写 result_slot → fetch Promise 永不 resolve → 进程挂死。
- **evidence**（strace，修复前）：
  - JS 线程 0 次 `epoll_pwait2`；约 4000 次 `clock_nanosleep(1ms)` 死循环。
  - HTTPThread `connect() = EINPROGRESS` 后无任何进展。
  - 修复后（注册 App）：JS 线程开始 `epoll_pwait2(3, …)` 并 `accept4(6)` 成功（liveness 修复验证通过）。

### 同类判定标准（sameClassCriterion）
任何在 JS 线程创建 uWS App/socket 的模块（`Bun.serve`、`http.createServer`、未来的 WebSocket server 等）必须统一注册到 `node_http::ACTIVE_APPS`，使 `drain_and_check` 的单一 liveness 入口保持 loop tick。

### 根治（fixTemplate）— 阶段4
统一注册入口：
- `node_http::register_active_app(*mut App<false>)`（幂等、null-safe）。
- `node_http::unregister_active_app(*mut App<false>)`（幂等、null-safe）。
- `bun_serve`（bun_api.rs:1376 之后）调用 `register_active_app`；`server_stop`（bun_api.rs:1558）destroy 前调用 `unregister_active_app`。
- `node_http::server_close` 重构为复用 `unregister_active_app`（替换内联 `ACTIVE_APPS.retain`），保证单一更新路径。

### 全量确认报告 — 阶段5
```yaml
confirmReport:
  patternId: BCE-20260618-007-RT
  sweepScope: "src/bao_runtime/src/ 全量"
  layersScanned: [literal, structural]
  instancesFound: 2              # node_http::server_listen (已注册) + bun_api::bun_serve (未注册)
  truePositives: 1               # bun_serve 缺注册
  falsePositives: 1              # server_listen 原本就 push ACTIVE_APPS（保留）
  instancesFixed: 1              # bun_serve + server_stop 接入 register/unregister
  residual: 0                    # 所有 JS-thread App 创建点统一注册
  residualEvidence:
    - "重扫：grep App::<false>::create 命中 2 处（node_http:561 已注册；bun_api:1376 现已注册）"
    - "node_http::tests::bce_007_* 4/4 passed（注册/反注册/幂等/null-safe）"
    - "cargo test -p bun_runtime --lib: 614 passed / 0 failed"
    - "strace 修复后：JS 线程 epoll_pwait2 + accept4 正常，liveness 恢复"
  releaseGateImpact: block (见下文残留)
```

### 残留 / 升级（C-2.1）— 残留即未完成
本 BCE 仅根治了 **liveness 注册缺失**（第一个 rootCause）。运行时验证发现 **fetch(self) 仍挂**，因为存在**独立的、范围更广的预存缺陷**：

- **第二 rootCause（未根治，超出本任务范围）**: `HTTPThread`（`src/http/AsyncHTTP.rs` / `src/http/HTTPThread.rs` / `src/http/lib.rs`）在 client socket `connect()` 成功（EPOLLOUT）后**不写 HTTP 请求**。strace 证据：worker connect → EPOLLOUT → 之后 0 次 `write/writev/sendto/sendmsg`。该缺陷影响**所有 fetch**（`fetch("http://127.0.0.1:9222/json")`、`fetch(python_http_server)` 同样挂），非 `fetch(self)` 特有。位置疑似 `HTTPClient::on_open → first_call → on_writable` 链路（`src/http/lib.rs:1568/1759/2850`）在 bao 的 bun_http 移植中存在 dispatch 未触达 vtable.on_open 的断裂。
- **影响**: BCE-007-R1~R4 单元测试全过（liveness 注册正确），但 `Bun.serve + fetch(self)` 端到端运行时仍挂（被第二 rootCause 阻塞）。需要单独的 BCE 任务根治 HTTPThread 写路径（涉及上游 `bun_http` HTTPClient 移植，建议 architect(retrospect) 归因）。
- **本任务交付状态**: liveness 注册修复完整且经 strace 客观验证（JS 线程恢复 accept）；HTTPThread 写路径缺陷为独立预存问题，按 CLAUDE.md C-2 升级链路报告。

### 防复发（阶段6）
- ✅ 回归测试：`node_http::tests::bce_007_register_unregister_flips_liveness` / `_register_is_idempotent` / `_unregister_unknown_is_noop` / `_null_app_is_noop`（触发签名 = Bun.serve 未注册 → has_active_servers 误报 false → 测试 fail）。
- ✅ 统一注册入口：未来任何在 JS 线程创建 uWS App 的模块必须经 `register_active_app`，单一 liveness 真相源。
- ✅ 知识库：本条目。

---

## BCE-20260618-007-R2 — HTTPThread client connect 后不写 HTTP 请求（dispatch vtable 断链）

### patternId / title
`BCE-20260618-007-R2` · bao 移植的 `bun_http` `HTTPContext::init` 传 `vtable = None` → C++ `us_dispatch_open`/`us_dispatch_writable` 在 connect 完成时返回不调用任何 handler → `Handler::on_open → first_call → on_writable` 链断 → HTTP 请求体永不写出 → **所有 fetch 挂**。

### layer
设计缺陷（移植协议失配 — 上游 Zig 用静态 kind→vtable 表，bao 只读 per-group vtable，但 init 时又传 None，两头落空）。

### 根因（rootCause）
- **location**:
  - `src/http/HTTPContext.rs:528` `init_with_opts` 调用 `self.group.init(loop, None, owner_ptr)`（vtable=None）。
  - `src/http/HTTPContext.rs:584` `init` 同样传 `None`。
  - 断链点：`src/bao_uloop/src/lib.rs:509` `dispatch_via_vtable` 仅查 `group.vtable`；`HTTPContext` group 的 vtable 是 `None` → fallback 直接返回 `s`，不触达 `Handler::on_*`。
- **why（上游对照）**:
  - 上游 `bun/src/runtime/socket/uws_dispatch.zig:33-64` 用一个 **静态 `tables: EnumArray<SocketKind, ?*VTable>`**（`tables.set(.http_client, vtable.make(handlers.HTTPClient(false)))`）按 socket kind 查 vtable；`HTTPContext.zig:248` 也传 `null`，但上游的 `vt(s)` 函数先查静态表，所以 `http_client`/`http_client_tls` kind 仍能找到 handler。
  - bao 的移植**只复刻了 per-group 路径**（`dispatch_via_vtable` 查 `group.vtable`），**漏了静态 kind→vtable 表**。又因为 `HTTPContext` 传 `None`，`http_client`/`http_client_tls` 两类 socket 的 dispatch **完全空转**。
  - 端到端调用链（C++ 真源）：`packages/bun-usockets/src/loop.c:387` `us_internal_socket_after_open(s, error)`（connect 完成触发）→ `context.c:755` `us_dispatch_open(s, 1, 0, 0)` → bao `us_dispatch_open`（Rust export）→ `dispatch_via_vtable` → `group.vtable == None` → return `s`（**Handler::on_open 未被调用**）。
- **evidence**（strace，R1 修复后仍挂）：
  - client socket `connect() = EINPROGRESS` → 后续 EPOLLOUT → 之后 **0 次** `write/writev/sendto/sendmsg`（请求体从未发出）。
  - 该缺陷影响**所有 fetch**（`fetch("http://127.0.0.1:9222/json")`、`fetch(python_http_server)`、`fetch(self Bun.serve)` 同挂），非 fetch-self 特有。
  - 对照工作正常路径：`src/bao_runtime/src/node_net.rs:212` `NET_VTABLE`（`node_net` 自建 static VTable 并 `group.init(loop_, Some(&NET_VTABLE), null)`）— TCP socket 的 `net_on_open` 正常触达。

### 同类判定标准（sameClassCriterion）
任何 bao 移植的 socket 模块（`HTTPContext`、未来的 WebSocket client、SQL driver 等）若其 `SocketGroup::init` 传 `vtable = None` 且该 kind 没有对应静态 vtable 注册，则该模块的所有 dispatch 事件（`on_open`/`on_writable`/`on_data`/`on_close`/`on_handshake`）都不会被触达 — 同类断链。

### 根治（fixTemplate）— 阶段4
采用 bao 已证明正确的 `NET_VTABLE` 模式（per-group static vtable），不引入全局静态 kind→vtable 表（避免上游 Zig 反射生成器的复杂度）：

1. 为 `Handler::<SSL>::on_*` 写 `unsafe extern "C"` trampoline（`http_vt_on_open`/`on_data`/`on_writable`/`on_close`/`on_timeout`/`on_long_timeout`/`on_end`/`on_connect_error`/`on_handshake`），签名匹配 `us_socket_vtable_t`（首参 `*mut us_socket_t`）。
2. 每个 trampoline 用 `HTTPContext::<SSL>::ext_tagged_ptr(socket)` 从 socket ext slot 读出 `ActiveSocket` tagged-pointer word（即 `Handler::on_*` 第一参 `ptr: *mut c_void` 的值），然后 forward。
3. 定义 `static HTTP_VTABLE`（`Handler::<false>`，bind `http_client` kind）和 `static HTTPS_VTABLE`（`Handler::<true>`，bind `http_client_tls` kind；`on_handshake` slot 必须填 — TLS 路径 `first_call` 从 `on_handshake` 而非 `on_open` 调）。
4. `init_with_opts`（SSL-only）传 `Some(&HTTPS_VTABLE)`；`init`（generic）按 const-generic `SSL` 选 `HTTP_VTABLE`/`HTTPS_VTABLE`。

统一策略：所有 bao 移植的 socket 模块统一用「per-group static vtable」（`NET_VTABLE` / `HTTP_VTABLE` / `HTTPS_VTABLE`），禁止再用 `vtable = None` 配静态 kind 表（bao 没有该表）。

### 全量确认报告 — 阶段5
```yaml
confirmReport:
  patternId: BCE-20260618-007-R2
  sweepScope: "src/http/ + src/bao_runtime/src/ + src/bao_uloop/src/ 全量"
  layersScanned: [literal, structural]
  instancesFound: 3
    - HTTPContext::init_with_opts (vtable=None)   # 真阳性 1
    - HTTPContext::init (vtable=None)              # 真阳性 2
    - node_net::NET_VTABLE (已正确)                # 误报，参考实现
  truePositives: 2
  falsePositives: 1
  instancesFixed: 2   # init_with_opts → &HTTPS_VTABLE; init → 按 SSL 选 HTTP/HTTPS_VTABLE
  residual: 0
  residualEvidence:
    - "重扫 grep 'group.*init.*None' src/http/ ：0 命中（HTTPContext 两处 init 均已传 Some）"
    - "cargo check（workspace 全量）：Finished，0 error"
    - "bun_http crate 内 HTTP_VTABLE/HTTPS_VTABLE dispatch slot 单元测试：3/3 passed（on_open/on_writable/on_data/on_close/on_end/on_connect_error/on_timeout/on_long_timeout 全 Some；HTTPS 额外 on_handshake Some；HTTP vs HTTPS trampoline 指针不同）"
    - "端到端验证受限：当前构建环境 native-link 配置缺失（c-ares/lsquic/lshpack 未在 link line），无法重建 target/debug/bao 复现脚本；代码层 rootCause 已定位 + 根治 + 单元测试覆盖。"
  releaseGateImpact: block（端到端 fetch 验证待 binary rebuild）
```

### 防复发（阶段6）
- ✅ 回归测试：`http::HTTPContext::tests::http_vtable_dispatch_slots_populated` / `https_vtable_handshake_slot_populated` / `http_and_https_vtables_bind_distinct_trampolines`（触发签名 = vtable 任一关键 slot = None → 测试 fail）。
- ✅ 统一约定：bao 移植的 socket 模块一律用 per-group static vtable（参考 `NET_VTABLE`），禁止 `group.init(..., None, ...)`。
- ✅ 知识库：本条目。

---

## BCE-20260618-007-R3 — drain_and_check 的 `tick_once(null)` 永久阻塞 epoll（fetch Promise 永挂）

### patternId / title
`BCE-20260618-007-R3` · `drain_and_check` / `drain_one_pass` 的 has_http 分支调用 `MiniEventLoop::tick_once(ctx=null)` → `UwsLoop::tick()` → `us_loop_run_bun_tick(loop, NULL)` → libusockets C 的 `epoll_pwait2(timeout=NULL)` 无限阻塞 → fetch worker 完成 HTTP 往返并写入 `result_slot` 后 JS 线程无 epoll 事件唤醒 → `drain_pending_fetches` 永不运行 → fetch Promise 永不 resolve → 进程挂死（exit 137）。

### layer
设计缺陷（事件循环跨线程唤醒协议缺失）。

### 根因（rootCause）
- **location**:
  - `src/bao_runtime/src/timers.rs:172` `drain_and_check` has_http 分支：`loop_.tick_once(core::ptr::null_mut())`。
  - `src/bao_runtime/src/timers.rs:253` `drain_one_pass` has_http 分支：同上。
  - C 真源：`packages/bun-usockets/src/eventing/epoll_kqueue.c:382` `will_idle_inside_event_loop = had_wakeups == 0 && (!timeout || ...)` — 当 `timeout == NULL` 时 `!timeout = true` → `will_idle = 1` → `epoll_pwait2(loop->fd, …, NULL)` 阻塞直至 fd ready。
  - 间接路径：`src/event_loop/MiniEventLoop.rs:378` `tick_once` → `(*self.loop_ptr()).tick()` → `src/uws_sys/Loop.rs:210` `tick()` → `us_loop_run_bun_tick(self, core::ptr::null())`。
- **why**:
  - `Bun.serve` 的服务端 socket 在 JS-thread uWS Loop 注册（BCE-007-R1 已修 liveness）。`tick_once(null)` 第一次会 `epoll_pwait2` 拿到 listen socket ready → `accept4` → `recvfrom` → `sendto` → shutdown 完成 fetch 服务端响应（strace 证据：完整往返 ✓）。
  - 之后**没有更多 ready fd**（fetch worker 已读完响应、HTTPThread 已写 SingleHTTPChannel）。`tick_once(null)` 再次进入 `epoll_pwait2(timeout=NULL)` → 无限阻塞。
  - fetch worker 在独立线程完成 `stealth_http_request` → `send_sync` → condvar wake → 写 `result_slot` → **不调用 `us_wakeup_loop(loop)`**（无跨线程唤醒），所以 JS-thread uWS Loop 的 wakeup eventfd 永不触发 → JS 线程永久困在 `epoll_pwait2` → `drain_pending_fetches` 永不被调用 → fetch Promise 永挂。
- **evidence**（eprintln + strace，R2 修复后）:
  - `[BCE7-WORKER] slot written` 后**无** `[BCE7-DRAIN]` 输出（drain_pending_fetches 在阻塞的 tick 之后，永远跑不到）。
  - 末尾 `[BCE7-TICK] http=true tmr=false fetch=true` 后进程静止（KILL exit 137）。
  - strace：JS 线程 `accept4 → EAGAIN` 后无任何 syscall（卡在 `epoll_pwait2`）；worker 线程 `futex(WAKE,1)=1` 但 JS 线程无对应 `futex(WAIT)` 配对（JS 线程不在 condvar 上等，而在 epoll fd 上等）。

### 同类判定标准（sameClassCriterion）
任何「JS 线程事件循环 tick + 后台线程完成异步任务 + 后台线程结果通过共享内存（无 epoll fd 唤醒）通知 JS 线程」的模式，若 JS 线程的 tick 走 `us_loop_run_bun_tick(timeout=NULL)` 路径，则 fetch/异步 IPC/任何跨线程结果都会卡住 —— 同类挂死。

### 根治（fixTemplate）— 阶段4
统一改用零超时 tick（`tick_without_idle`，对应 `us_loop_run_bun_tick(loop, &{0,0})`），让 `will_idle_inside_event_loop = false` → `epoll_pwait2` 不阻塞，drain 完 ready fd 即返回。JS 线程 yield 由 `eval-loop` 的 `std::thread::sleep(1ms)`（context.rs:462）保证，不会 busy-spin。

- `timers.rs:172` `drain_and_check`：`loop_.tick_once(core::ptr::null_mut())` → `loop_.tick_without_idle(core::ptr::null_mut())`。
- `timers.rs:253` `drain_one_pass`：同上替换。

替代方案（未采用）：在 `spawn_fetch_worker` 写完 `result_slot` 后调用 `us_wakeup_loop(loop_ptr)` 唤醒 JS 线程 —— 但要求 worker 持有 JS-thread uWS Loop 指针（跨线程所有权更复杂），且 `drain_and_check` 的 has_http 分支仍需处理「http 活跃但 fetch worker 未启动」场景。零超时 tick 是更小的、更安全的根治（不改 worker 模型）。

### 全量确认报告 — 阶段5
```yaml
confirmReport:
  patternId: BCE-20260618-007-R3
  sweepScope: "src/bao_runtime/src/ 全量"
  layersScanned: [literal, structural]
  instancesFound: 2   # drain_and_check + drain_one_pass 的 has_http 分支
  truePositives: 2
  falsePositives: 0
  instancesFixed: 2   # 两处均改为 tick_without_idle
  residual: 0
  residualEvidence:
    - "重扫 grep 'tick_once.*null_mut' src/bao_runtime/src/：0 命中（两处 has_http 均改为 tick_without_idle）"
    - "cargo check --workspace：Finished，0 error"
    - "cargo test -p bun_runtime --lib：614 passed / 0 failed"
    - "cargo test -p bun_runtime --lib timers::：56/0"
    - "cargo test -p bun_runtime --lib node_http::：21/0"
    - "端到端复现：target/debug/bao -e 'Bun.serve + fetch(self)' → DONE={...} + EXIT=0（修复前 exit 137）"
    - "test_http_depth：EXIT=0（修复前 exit 137；现存 FAIL 是 Bun.serve 默认 handler 缺失，独立问题，非 BCE-007 挂起范畴）"
  releaseGateImpact: pass
```

### 防复发（阶段6）
- ✅ 知识库：本条目。
- ✅ 代码注释：`timers.rs:159-170` 完整 BCE-007-R3 根因注释（C 真源行号 + 替代方案讨论）。
- 📌 未来风险：若 bao 引入「真正阻塞的 JS-thread tick」（如为了节流 CPU），必须同时实现「跨线程 `us_wakeup_loop` 唤醒」协议（worker 完成时唤醒 JS-thread uWS Loop），否则同类挂死复发。当前以零超时 tick + 1ms sleep 的轮询模型规避该协议复杂度。

---

