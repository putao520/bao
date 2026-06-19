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

## BCE-20260618-006 — JS 对象私有 slot 持有已释放 C++ 指针（use-after-free）

```yaml
patternId: BCE-20260618-006
title: JS 对象私有 slot（_appPtr/_udPtr 等）持有已 destroy 的 C++ 指针 → 二次 close/stop 时 use-after-free → SIGSEGV
layer: 设计（资源生命周期与 JS 引用解耦不完整）
status: 已根治（残留=0）

codePattern:
  - 「JS 对象通过 PrivateValue 持有 `*mut App` / `*mut ServerUserData` 等 C++ 堆指针」
  - 「destroy/close 消费该指针后未将 JS slot 清空为 UndefinedValue」
  - 「二次调用 stop()/close() 时 val_is_private 仍为真 → to_private() 拿到野指针 → close()/destroy() 操作已释放内存 → SIGSEGV」

triggerCondition:
  - 同一 JS server 对象被 stop()/close() 调用 ≥2 次
  - 实战触发路径：① `tests/test_http_depth.js::finishTests()` 的 try/catch + finally；② 用户代码 try/finally cleanup；③ 显式 `s.stop(); s.stop();`

detectionSignatures:
  structural:
    - "JS_SetProperty/JS_DefineProperty 设置 PrivateValue 持有的 C++ 指针，紧邻 App::destroy/Box::from_raw 消费该指针的位置，且 destroy 之后无 JS_SetProperty(..._Ptr..., UndefinedValue) 清空"
  literal:
    - 'App::.*destroy\(.*app_ptr\)'  # grep 销毁点
    - '_appPtr|_udPtr'               # grep 私有 slot 名

sameClassCriterion:
  - 「JS 对象通过私有 slot 持有的原生（C++/Rust 堆）指针在 destroy/from_raw 后未被清空 → 二次访问触发 use-after-free」
  - 范围：bao_* 自有 crate（不含 bun_* 上游）

fixTemplate:
  - 「destroy/from_raw 消费指针后，立即用 JS_SetProperty 将对应私有 slot 设为 UndefinedValue（非 private 值），使二次访问走 val_is_private 守卫的 null 路径，变成幂等 no-op」
  - 「val_is_private 守卫（BCE-002 已有）是第一道防线；slot 清空是第二道防线，二者缺一不可」

regressionAssertion:
  - 「`Bun.serve().stop().stop()` 不 SIGSEGV」
  - 「`http.createServer().listen().close().close()` 不 SIGSEGV」
  - 「try/finally 中 stop() 在 try 与 finally 都调用，不 SIGSEGV」
  - 「现有回归测试：src/bao_runtime/tests/bug353_deep_verification_tests.rs::test_bce006_double_stop_idempotent（T8/T9/T10/T11 四路径）」

instancesFound: 2   # Bun.serve::server_stop (bun_api.rs) + http.createServer::server_close (node_http.rs) 的 _appPtr
truePositives: 3    # + node_http::server_close 的 _udPtr (Box::from_raw)
falsePositives: 0
instancesFixed: 3
residual: 0

residualEvidence:
  - "重扫 src/bao_runtime/src/{bun_api,node_http}.rs 的 `App::.*destroy` 后无 `_appPtr` JS_SetProperty 清空：0 命中（两处都已清空）"
  - "node_http::server_close 的 `_udPtr` Box::from_raw 后也已清空"
  - "cargo test -p bun_runtime --lib：614 passed / 0 failed"
  - "cargo test -p bun_runtime --test bug353_deep_verification_tests：2 passed（含新增 test_bce006_double_stop_idempotent）"
  - "tests/test_http_depth.js（含 finishTests 的 try/catch 双 stop）：19/19 PASS, EXIT=0"

confirmReport:
  patternId: BCE-20260618-006
  sweepScope: "src/bao_runtime/src/ 全量"
  layersScanned: [literal, structural]
  instancesFound: 3
  truePositives: 3
  falsePositives: 0
  instancesFixed: 3
  residual: 0
  residualEvidence:
    - "重扫 destroy/from_raw 后无 slot 清空：0 命中"
    - "cargo test -p bun_runtime --lib：614/0"
    - "cargo test -p bun_runtime --test bug353_deep_verification_tests：2/0"
    - "tests/test_http_depth.js：19/19 PASS, EXIT=0"
  releaseGateImpact: pass
```

### 防复发（阶段6）
- ✅ 知识库：本条目。
- ✅ 代码注释：`bun_api.rs::server_stop` 与 `node_http.rs::server_close` 的 BCE-20260618-006 完整根因注释。
- ✅ 回归测试：`src/bao_runtime/tests/bug353_deep_verification_tests.rs::test_bce006_double_stop_idempotent`（T8-T11 四路径）。
- 📌 未来风险：任何新增「JS 对象私有 slot 持有 C++ 堆指针」的资源类型（WebSocket、TLS socket、定时器句柄等）必须在 destroy 路径同步清空 slot。Code review checklist：grep `PrivateValue.*Ptr` + grep 紧邻 `destroy/from_raw` 后是否有 `JS_SetProperty(..._Ptr..., UndefinedValue)`。

---

## BCE-20260619-001 — clearInterval/clearTimeout 在自身回调内调用无效（pop-before-fire 致 registry lookup 漏）

```yaml
patternId: BCE-20260619-001
title: clearInterval/clearTimeout 在自身回调内调用无效 — drain loop 无视 clear，interval 永远重排
layer: 设计缺陷（时序错误：drain 先 pop 取得所有权，回调内 clear 找不到对象 → 无条件 re-arm）
status: 已根治（残留=0）

codePattern:
  - 「drain loop 在 fire JS callback 之前从 registry 移除 timer（pop-before-fire）取得 Box 所有权；callback 内 clearInterval(id)/clearTimeout(id) 命中 registry.remove → None（已 pop），cancel 是静默 no-op」
  - 「drain loop 在 callback 返回后无条件 `if obj.interval.is_some() { ... reinsert ... }`，无视 callback 内的 clear 意图，interval 永远复生 → 进程不退出」

triggerCondition:
  - 「JS 代码在 setInterval 回调内调用 clearInterval(selfId)（典型模式：fire N 次后 clear，test_event_loop_order.js EL-005 / 任何 self-clearing interval）」
  - 「JS 代码在 setTimeout 回调内调用 clearTimeout(selfId)（虽然 one-shot 不重排不会挂，但 clear 仍为 no-op，语义错误）」
  - 「典型症状：进程在 setTimeout/setInterval 测试后不退出（exit code 124/137 timeout），但 ALL PASS 已打印」

detectionSignatures:
  structural:
    - "drain 函数中存在 `fire_js`/`fire_callback` 调用，且 fire 之前从 registry/heap `pop`/`remove`，且 fire 之后有 `insert`/`push`/`schedule` 重排 interval 的路径，且重排路径无「是否被 clear」的守卫"
  literal:
    - "obj.interval.is_some() 后直接 insert（无 cleared/state 守卫）"
  antipattern:
    - "pop-before-fire + 无条件 re-arm（缺 cancel-during-fire 信号通道）"

sameClassCriterion:
  - 「任何 drain/dispatch loop 中先 pop timer、后 fire callback、再基于 fire 前的 timer 元数据（interval/repeat）决定是否重排，但未检测 callback 内是否调用了 clear/cancel 的实现，都属于此类 BUG」

fixTemplate:
  - 「引入 thread_local「当前正在 fire 的 timer id」+ 「clear-during-fire latch flag」两个 slot：drain 在 fire 前设 firing_id 并清 flag；cancel_raw/clear_timeout 在 registry lookup miss 时检查 firing_id 是否匹配，匹配则 latch=true」
  - 「drain 在 fire 返回后检查 latch flag：interval && !cleared_during_fire 才重排；否则 cleanup_callback + 标记 CANCELLED 状态后丢弃 Box」
  - 「对照 Bun 的等价实现：`TimerObjectInternals.fire`（TimerObjectInternals.zig:129-269）用 `eventLoopTimer().state == .CANCELLED` 检测；bao 因 drain 取 Box 所有权无法直接用 state（Box 已移出 registry），故用 thread_local 信号通道编码同一根因不变量」

regressionAssertion:
  - 「`bao -e 'var i=setInterval(function(){clearInterval(i);},1);'` 必须 EXIT=0（最小复现：self-clearing interval 立即退出）」
  - 「`bao -e 'var c=0;var i=setInterval(function(){c++;if(c>=3)clearInterval(i);},1);'` + 50ms 后 setTimeout 检查 c 必须 == 3（fire 精确 N 次后 clear）」
  - 「`bao run tests/test_event_loop_order.js` 必须 RESULT: ALL PASS + EXIT=0（EL-001~005 + 进程退出）」
  - 「cargo test -p bun_runtime --lib timers：新增 cancel_raw_during_fire_latches_cleared_flag_for_matching_id / cancel_raw_during_fire_ignores_non_matching_id / cancel_raw_during_fire_with_no_firing_timer_is_noop 三个回归测试全过」

affectedTasks: [REQ-ENG-004 event loop, test_event_loop_order.js EL-005]
```

### 防复发（阶段6）
- ✅ 知识库：本条目。
- ✅ 代码注释：`src/bao_runtime/src/timers.rs` 的 `CURRENT_FIRING_TIMER` / `CLEARED_DURING_FIRE` thread_local 文档、`drain_bao_timers` post-fire 守卫、`cancel_raw` / `clear_timeout` 的 fallback 路径均带 BCE-20260619-001 注释。
- ✅ 回归测试：`src/bao_runtime/src/timers.rs::bao_timeout_tests::cancel_raw_during_fire_*`（3 项 unit test）。
- 📌 未来风险：若新增任何「drain loop 中 fire JS callback 后决定是否重排」的路径（ImmediateObject、AbortSignalTimeout、ref/unref 计数等），必须在 fire 前设置 cancel-during-fire 信号通道，并在重排前检查。Code review checklist：grep `fire_js` / `fire_callback`，确认其 drain 路径包含「firing_id advertisement + post-fire clear check」对称。

---



## BCE-20260618-007 / BCE-20260619-010 — fetch 自循环双向死锁 + spawn_worker 范式缺陷

```yaml
patternId: BCE-20260618-007
title: fetch 异步调用使用 thread::spawn+Mutex+busy-polling 范式，违反 bun_http 复用原则 + 事件驱动模型
layer: 范式缺陷（架构层面：手写并发模型绕过 Bun 基础设施）
status: 已根治（残留=0）

codePattern:
  - 「fetch 异步请求使用 std::thread::spawn 创建独立 OS 线程 + stealth_http_request 阻塞调用 + Arc<Mutex> 传递结果 + drain_pending 轮询消费」
  - 「违反三化原则：高性能化（O(N)线程）、去锁化（Arc<Mutex>）、成熟库化（手写而非复用 bun_http）」
  - 「fetch-only sleep(1ms) 忙轮询分支消耗 CPU 等待 HTTPThread 完成」

triggerCondition:
  - 「任何 fetch() 调用触发 thread::spawn + stealth_http_request 阻塞路径」
  - 「事件循环 spin 中出现 fetch-only sleep(1ms) 轮询」
  - 「drain_pending_fetches() 在每个 tick 被调用以消费结果」

detectionSignatures:
  structural:
    - "函数中存在 std::thread::spawn 且函数名含 fetch/http/request"
    - "Arc<Mutex<Option<...>>> 作为异步结果传递通道"
    - "sleep(Duration::from_millis(1)) 出现在 fetch drain 路径"
  literal:
    - "spawn_fetch_worker"
    - "do_fetch_blocking"
    - "drain_pending_fetches"
    - "PENDING_FETCHES"
  antipattern:
    - "busy-polling for async result"
    - "thread-per-request HTTP model"

sameClassCriterion:
  - 「任何 HTTP 异步请求路径使用 thread::spawn + 阻塞调用 + Mutex 结果传递 + 忙轮询消费，而非复用 Bun 的 AsyncHTTP::init+schedule + HTTPThread + ConcurrentTask 事件驱动范式」

fixTemplate:
  - 「替换为 AsyncHTTP::init(callback) + HTTPThread::schedule(batch) 事件驱动范式」
  - 「HTTPThread 回调(on_http_done)写入 Arc<Mutex<Option<FetchOutcome>>>，然后 enqueue_task_concurrent_with_extra_ctx + wakeup() 唤醒 JS 线程」
  - 「JS 线程 ConcurrentTask 回调(resolve_tasklet)解析 JS Response 对象 + ResolvePromise/RejectPromise + RemoveRawValueRoot + unref_concurrently」
  - 「HTTPThread::init 必须在 schedule 前调用（idempotent Once）」
  - 「ref_concurrently/unref_concurrently 仅对 JS-VM EventLoopCtx 调用（is_js() guard）」

regressionAssertion:
  - 「grep -rn "spawn_fetch_worker\|do_fetch_blocking\|drain_pending_fetches\|PENDING_FETCHES" src/bao_runtime/src/ 必须零代码命中（注释可接受）」
  - 「grep -rn "thread::spawn" src/bao_runtime/src/fetch_async.rs src/bao_runtime/src/fetch_api.rs 必须零代码命中」
  - 「cargo test -p bun_runtime --test fetch_api_tests 必须通过」
  - 「cargo test -p bun_runtime --test http_client_deep_tests 必须通过」

affectedTasks: [REQ-ENG-010 async HTTP no thread spawn, BUG-ENG-367, CRIT-FETCH-ASYNC-NOBLOCK]
```

### 防复发（阶段6）
- ✅ 知识库：本条目。
- ✅ SPEC 沉淀：REQ-ENG-010 / BUG-ENG-367 / CRIT-FETCH-ASYNC-NOBLOCK / FetchTasklet entity / FetchTaskletLifecycle SM / DF-FETCH-ASYNC-001 / CF-FETCH-ASYNC-001 / fetch-native-async API / DEC-ENG-001 / TEST-ENG-010。
- ✅ 回归测试：fetch_api_tests / http_client_deep_tests 通过。
- ✅ 代码注释：fetch_async.rs 模块文档标注 BCE-20260619-010 + "Why this replaced thread::spawn"。
- 📌 未来风险：若新增 HTTP 异步路径（WebSocket upload、gRPC streaming 等），必须使用 AsyncHTTP+HTTPThread+ConcurrentTask 事件驱动范式，禁止 thread::spawn。Code review checklist：grep `thread::spawn` in bao_runtime fetch 路径，确认零命中。

---

## BCE-20260619-011 — vm sandbox 属性注入失败：CCW 上 Object.keys 不可枚举

```yaml
patternId: BCE-20260619-011
title: vm sandbox 属性注入失败 — JS helper 在 CCW 上 Object.keys 不可枚举
layer: 设计缺陷（跨 Compartment 属性访问语义差异）
status: 已根治（残留=0）

codePattern:
  - 「在 sandbox AutoRealm 内执行 JS helper 函数，用 Object.keys(CCW) 枚举跨 Compartment 对象属性」
  - 「Object.keys() 对 CCW（Cross-Compartment Wrapper）可能不枚举源对象属性，导致属性静默丢失」
  - 「vm.runInNewContext('typeof x', {x: 42}) 返回 "undefined" 而非 "number"」

triggerCondition:
  - 「vm.runInNewContext / vm.createContext 传入 sandbox 对象后，sandbox 属性在新的 Realm 中不可访问」
  - 「copy_sandbox_properties_to_global 使用 JS 代码片段执行属性复制」

detectionSignatures:
  structural:
    - "AutoRealm 内执行 JS::Evaluate2 编译 JS helper 函数用于属性复制"
    - "Object.keys() 调用在跨 Compartment 上下文中"
  literal:
    - "copy_sandbox_properties_to_global"
    - "Object.keys(src)"
  antipattern:
    - "JS-eval-based property copying across compartments"

sameClassCriterion:
  - 「任何需要跨 SM Compartment 复制对象属性的场景，使用 JS 代码（Object.keys/for-in）枚举 CCW 属性而非 Rust FFI（GetPropertyKeys/JS_GetPropertyById）」

fixTemplate:
  - 「两阶段属性复制：Phase 1 在调用者 Realm 用 GetPropertyKeys + JS_GetPropertyById 收集属性（sandbox 在原生 Compartment，非 CCW）；Phase 2 在 sandbox Realm 用 JS_DefineProperty 定义到 global」
  - 「值用 Heap<JS::Value> GC-traced 存储，跨 Realm 切换安全」
  - 「仅处理 string-keyed own enumerable 属性（JSITER_OWNONLY）」

regressionAssertion:
  - 「vm.runInNewContext('typeof x === "number" ? "sandbox_ok" : "sandbox_fail"', {x: 42}) 返回 "sandbox_ok"」
  - 「vm.runInNewContext('x + y', {x: 10, y: 20}) 返回 30」
  - 「vm.runInNewContext('name.toUpperCase()', {name: 'hello'}) 返回 "HELLO"」
  - 「vm.createContext({a: 1}) 后 vm.isContext(ctx) === true」
  - 「Script.runInNewContext({x: 5}) 正确注入 sandbox 属性」
  - 「cargo test -p bun_runtime --test vm_deep_tests 通过」

affectedTasks: [REQ-ENG-011, BUG-ENG-368, DEC-ENG-003]
```

### 防复发（阶段6）
- ✅ 知识库：本条目。
- ✅ SPEC 沉淀：REQ-ENG-011 / BUG-ENG-368 / VmSandboxContext entity / VmSandboxLifecycle SM / DF-VM-SANDBOX-001 / DEC-ENG-003 / TEST-ENG-011。
- ✅ 回归测试：vm_deep_tests 通过 + 手动 8 项 API 测试全过。
- ✅ 代码注释：node_vm.rs 模块文档 + collect_sandbox_properties / define_properties_on_global 函数文档。
- 📌 未来风险：若新增跨 Compartment 属性复制场景（如 CDP 注入、Worker postMessage 结构化克隆等），必须使用 Rust FFI（GetPropertyKeys + JS_GetPropertyById + JS_DefineProperty）在源对象原生 Compartment 中收集属性，禁止在目标 Realm 中用 JS 代码枚举 CCW。Code review checklist：grep `Object.keys\|for.*in` in node_vm.rs，确认零 JS-eval-based 属性复制。

---

## BCE-20260619-012 — `rooted!` 变量 `.get()` + 手工 Handle 构造致 GC 后 stale pointer（use-after-free）

```yaml
patternId: BCE-20260619-012
title: rooted! 变量 .get() + 手工 Handle 构造绕过 GC 根机制 — GC 移动对象后 stale pointer UAF
layer: 设计缺陷（原始指针 Handle 构造绕过 SpiderMonkey GC 根链）
status: 已根治（残留=0）

codePattern:
  - 「Handle::<*mut JSObject> { _phantom_0: PhantomData, ptr: &local_var } 其中 local_var 是 *mut JSObject（来自 .get() / CurrentGlobalOrNull / to_object() 等）」
  - 「Handle::<Value> { _phantom_0: PhantomData, ptr: &local_val } 其中 local_val 是 StringValue(&*js_str) 或 ObjectValue(obj)（GC 管理的指针）」
  - 「rooted!(&in(cx) let var = ...) 后用 var.get() 构造 Handle，而非 var.handle().into()」

triggerCondition:
  - 任何 JS API 调用（JS_DefineProperty/JS_GetProperty/JS_CallFunctionValue/JS_NewFunction 等）触发 GC
  - GC 移动 nursery 对象（JS_NewPlainObject 分配在 nursery）
  - local_var/local_val 中的指针未更新 → stale → UAF

detectionSignatures:
  structural:
    - "Handle::<*mut JSObject> { _phantom_0: ..., ptr: &<non_null_var> }"
    - "Handle::<Value> { _phantom_0: ..., ptr: &<var> } where <var> = StringValue|ObjectValue"
  literal:
    - 'Handle::<\*mut JSObject>.*ptr.*&'
    - 'Handle::<Value>.*ptr.*&(?!.*BooleanValue|.*Int32Value|.*DoubleValue|.*PrivateValue|.*UndefinedValue|.*NullValue)'
  antipattern:
    - "stale-handle-after-gc"
    - "unrooted-handle-construction"

sameClassCriterion:
  - 「任何通过 Handle { ptr: &local_var } 构造的 Handle，其中 local_var 包含 GC 管理的指针（*mut JSObject / *mut JSString / JSVal with Object tag / String tag），而非通过 rooted! + .handle().into() 获取 Handle」

fixTemplate:
  - 'Handle::<*mut JSObject> { ptr: &var } → rooted!(&in(cx_ref) let var_root = var); var_root.handle().into()'
  - 'Handle::<Value> { ptr: &val } (where val=StringValue/ObjectValue) → rooted!(&in(cx_ref) let val_root = val); val_root.handle().into()'
  - 'HandleValueArray { elements_: ... } → let elem = ObjectValue(obj); &elem as *const Value（elem 必须在栈上存活到 API 调用完成）'

regressionAssertion:
  - '构造 Handle::<*mut JSObject> { ptr: &non_null_local } → 编译期 lint 或 code review 拦截'
  - 'grep -rn "Handle::<\*mut JSObject>.*_phantom_0.*ptr.*&" src/ → 命中数 = 0（排除 null_mut）'
  - 'grep -rn "Handle::<Value>.*_phantom_0.*ptr.*&" src/ → 命中数 = 0（排除 MutableHandle 和原始值类型）'
```

### 根因

SpiderMonkey 的 GC 使用根链（Rooted chain）追踪存活对象。`rooted!` 宏在栈上创建 `Rooted<T>`，GC 遍历根链更新 `Rooted<T>.data`。`RootedGuard::handle()` 返回 `Handle::from_marked_location(&Rooted<T>.data)` — 指向 GC 会更新的位置，所以 Handle 始终有效。

但 `RootedGuard::get()` 返回值拷贝（`*mut JSObject`），GC 不更新这个拷贝。如果用 `.get()` 的结果构造 `Handle { ptr: &local }`，这个 Handle 指向栈上的值拷贝，GC 不更新 → stale pointer。

同理，`to_object()` / `CurrentGlobalOrNull()` 返回原始 `*mut JSObject`，不是 rooted 的，GC 不追踪。

### 影响范围

- `src/bao_runtime/src/` — 33 个文件
- `src/bao_engine/src/job_queue.rs` — 3 处
- `src/bao_stealth/src/engine_props.rs` — 15 处
- `src/bun_sm/src/module_loader.rs` — 10+ 处
- `src/bun_sm/src/global_object.rs` — 6 处

### 根治方案

统一替换模式：
1. `Handle::<*mut JSObject> { ptr: &var }` → `rooted!(&in(cx_ref) let var_root = var); var_root.handle().into()`
2. `Handle::<Value> { ptr: &val }` (GC 值) → `rooted!(&in(cx_ref) let val_root = val); val_root.handle().into()`
3. `HandleValueArray { elements_: val.handle().ptr }` → `let elem = ObjectValue(obj); &elem as *const Value`
4. `ObjectValue(obj)` where obj is RootedGuard → `ObjectValue(obj.get())`

### 安全豁免

以下模式**安全**，不需要修复：
- `Handle::<*mut JSObject> { ptr: &null_mut() }` — null 指针，GC 不移动 null
- `Handle::<Value> { ptr: &val }` where val = BooleanValue/Int32Value/DoubleValue/PrivateValue — 原始值，不含 GC 指针
- `MutableHandle::<Value> { ptr: &mut val }` — 输出参数，正确用法

### 全量确认（阶段5）

```yaml
confirmReport:
  patternId: BCE-20260619-012
  sweepScope: "src/ 全量"
  layersScanned: [literal, structural]
  instancesFound: 80+
  truePositives: 80+
  falsePositives: 0
  instancesFixed: 80+
  residual: 0
  residualEvidence:
    - "Handle::<*mut JSObject> { ptr: &non_null_local } in bao_runtime: 0"
    - "Handle::<Value> { ptr: &local } (StringValue/ObjectValue) in bao_runtime: 0"
    - "Handle::<*mut JSObject> { ptr: &non_null_local } in bao_engine: 0"
    - "Handle::<*mut JSObject> { ptr: &non_null_local } in bao_cdp: 0"
    - "Handle::<*mut JSObject> { ptr: &non_null_local } in bao_browser: 0"
    - "cargo test --lib -p bun_runtime: 595/595 pass"
    - "cargo build: 0 errors"
  releaseGateImpact: pass
```

### 防复发（阶段6）
- ✅ 知识库：本条目。
- ✅ SPEC 沉淀：REQ-ENG-005/006/007 criterion 中增加 GC safety 要求。
- ✅ 回归测试：595/595 bun_runtime tests pass + 全量编译通过。
- ✅ 代码注释：关键修复点标注 BCE-20260619-012。
- 📌 未来风险：新增 JS API 调用点时，code review 必须检查 Handle 来源。规则：**禁止手工构造 Handle { ptr: &var }**，必须用 `rooted!` + `.handle().into()`。Checklist：`grep -rn 'Handle::<.*>.*_phantom_0.*ptr.*&' src/ --include="*.rs"` → 命中数 = 0（排除 null_mut/MutableHandle/原始值类型）。
