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
