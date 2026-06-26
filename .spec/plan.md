# 开发计划: Web Worker 接入 servo 原生路径,废弃 bao_engine::WebWorker 旁路,三类 Worker 全交付 | epoch: 4 | status: active

## epoch
4

## status
active

## reqLedger
| REQ ID | 交付范围 | 覆盖 TASK |
|--------|---------|----------|
| REQ-BRW-004 | C1~C19 全量 | TASK-1,2,3,4,5,6,7,8,9 |
| REQ-BRW-4 | C1~C19 全量 | TASK-1,2,3,5,6,7,8,9 |
| DEC-WK-001 | servo原生Worker路径 | TASK-1 |
| DEC-WK-002 | servo原生WorkerGlobalScope | TASK-1,3 |
| DEC-WK-003 | 双轨隔离不合并 | TASK-1,5 |
| DEC-WK-004 | Node Realm cross-compartment暴露 | TASK-5 |
| DEC-WK-005 | StructuredSerializedData跨线程 | TASK-2,7 |
| DEC-WK-006 | 三类Worker平权全交付 | TASK-8,9 |
| DEC-WK-007 | Arc<StealthProfile>共享+getter注入 | TASK-4,6 |
| DEC-WK-008 | ServiceWorker fetch偏离处理 | TASK-9 |
| DF-WK-1 | Worker构造 | TASK-1 |
| DF-WK-2 | Worker脚本加载 | TASK-1 |
| DF-WK-3 | Dedicated stealth注入 | TASK-4 |
| DF-WK-4 | page→worker postMessage | TASK-2 |
| DF-WK-5 | worker→page onmessage | TASK-2 |
| DF-WK-6 | terminate | TASK-3,7 |
| DF-WK-7 | SharedWorker跨页路由 | TASK-8 |
| DF-WK-8 | ServiceWorker注册与拦截 | TASK-9 |
| DF-WK-9 | Shared stealth注入 | TASK-4 |
| DF-WK-10 | Service stealth注入 | TASK-4 |
| DF-WK-11 | Node Realm构造函数暴露 | TASK-5 |
| NFR-MEMSAF-001 | JSContext单线程串行化 | TASK-1,7 |
| NFR-THREAD-SAFETY | 禁止跨线程JSObject裸指针 | TASK-2,7 |
| WorkerScopeFingerprintConsistency | worker指纹与主线程一致 | TASK-4,6 |

## 范围

### REQ
- REQ-BRW-004: 浏览器 Web Worker API (Dedicated Worker) — C1~C19 全量
- REQ-BRW-4: Web Worker (DedicatedWorker/SharedWorker/ServiceWorker) — C1~C19 全量

### Entity
- Worker, DedicatedWorkerGlobalScope, WorkerGlobalScope, WorkerLocation, WorkerNavigator, WorkerMessage
- SharedWorker, SharedWorkerGlobalScope, ServiceWorker, ServiceWorkerGlobalScope

### API
- API-WK-001~010: /worker/new, /worker/postMessage, /worker/terminate, /worker/self.postMessage, /worker/self.close, /worker/addEventListener, /sharedworker/new, /serviceworker/register, /serviceworker/unregister, /serviceworker/getRegistration

### Decision
- DEC-WK-001~008

### Dataflow
- DF-WK-1~11

### NFR
- NFR-MEMSAF-001, NFR-THREAD-SAFETY, WorkerScopeFingerprintConsistency

### Test
- TEST-BRW-004, TEST-BRW-4

## 架构偏离说明（epoch 2→3 回跳根因）

**epoch 2 实现偏离**：TASK-1~6 实现了独立的 `bao_engine::WebWorker` 旁路（独立 spawn 线程、独立 Runtime/JSContext、独立 postMessage channel），而非 S1 设计决策要求的 servo 原生路径。

**S1 设计决策（必须遵守）**：
- DEC-WK-001: 三类 Worker 均经 servo 原生路径（vendor/servo/components/script/dom/workers/）
- REQ-BRW-004 C11: `bun_sm::WebWorker` stub 替换为 servo 真实实现（删除空壳，非充实旁路）

**修正方向**：
1. 废弃 `bao_engine::WebWorker` 旁路，保留其结构化克隆接口（`StructuredCloneReceiver`/`StructuredCloneSender` trait）供 servo 桥接复用
2. servo vendor patch 暴露 Worker scope embedder callback hook（复用 `register_script_thread_callback` 模式）
3. bao_browser 注册 callback → 在 servo Worker scope 创建后注入 stealth profile + 建立 bao 侧追踪结构

## 影响矩阵
| SPEC ID | 关联 TASK | 文件 |
|---------|----------|------|
| REQ-BRW-004 C1-C3,C7,C11 | TASK-1 | bao_browser/src/lib.rs, bao_browser/src/delegate.rs, bao_browser/src/runtime_bridge.rs, bun_sm/src/web_worker.rs, bun_sm/src/lib.rs |
| REQ-BRW-004 C6 | TASK-2 | bao_browser/src/delegate.rs |
| REQ-BRW-004 C4,C5,C8,C9 | TASK-3 | bao_browser/src/delegate.rs, bao_browser/src/runtime_bridge.rs |
| REQ-BRW-004 C12-C17 | TASK-4 | bao_stealth/src/engine_props.rs, bao_browser/src/runtime_bridge.rs |
| DEC-WK-004, DF-WK-11 | TASK-5 | bao_browser/src/runtime_bridge.rs |
| REQ-BRW-004 C10 | TASK-6 | bao_browser/src/delegate.rs, bao_browser/src/page.rs |
| REQ-BRW-004 C18 | TASK-7 | bao_browser/src/delegate.rs, bao_browser/src/page.rs, bao_stealth/src/engine_props.rs |
| REQ-BRW-4 C5, DF-WK-7 | TASK-8 | bao_browser/src/delegate.rs |
| REQ-BRW-4 C6,C19, DF-WK-8 | TASK-9 | bao_browser/src/delegate.rs |
| TEST-BRW-004, TEST-BRW-4 | TASK-10 | bao_browser/tests/worker_tests.rs, bao_stealth/tests/worker_*.rs |

## 任务树（扁平列表，禁止分 Phase/阶段/分期）

### TASK-1: servo 原生 Worker 路径接入 — Worker::Constructor + embedder callback
- SPEC: REQ-BRW-004 [C1: typeof Worker==='function', C2: new Worker()不抛异常, C3: Worker线程正常启动, C7: 独立Runtime/JSContext, C11: 删除bao_engine旁路接入servo原生] | TDD: TEST-BRW-004 | 文件: bao_browser/src/lib.rs, bao_browser/src/delegate.rs, bun_sm/src/web_worker.rs, bun_sm/src/lib.rs | 实现: (1) 验证 servo vendor 编译时 Worker DOM binding 可用;(2) servo vendor patch: 在 script_thread.rs 或 lib.rs 添加 `register_worker_scope_callback` (复用 `register_script_thread_callback` 模式);(3) bao_browser 初始化时注册 callback → worker scope 创建后回调 bao 层;(4) callback 内: 创建 WorkerHandle + WorkerChannelBridge + 注册 stealth profile;(5) 废弃 bao_engine::WebWorker::new* 独立 spawn 路径,保留 trait 接口供桥接
- 复用锚点: spec:REQ-BRW-004/REQ-BRW-4/DEC-WK-001/DEC-WK-002/DF-WK-1 | code:register_script_thread_callback(servo lib.rs:73 已有模式)/install_lazy_dom_getters(runtime_bridge.rs:600)/WorkerHandle(delegate.rs:55 已有)/WorkerChannelBridge(delegate.rs:341 已有) | pattern:servo_vendor_patch/embedder_callback
- 依赖: 无
- 状态: pending

### TASK-2: postMessage 结构化克隆通道 — servo StructuredClone + bao 桥接
- SPEC: REQ-BRW-004 [C6: structured clone 消息序列化支持] | TDD: TEST-BRW-004 | 文件: bao_browser/src/delegate.rs | 实现: (1) 验证 servo WorkerScriptMsg::DOMMessage 与 bao WorkerChannelBridge 的衔接;(2) WorkerChannelBridge 持有 crossbeam channel 端点,跨线程传递 servo StructuredSerializedData;(3) postMessage 路径: 主 worker.postMessage(v) → servo structuredclone::write → channel → worker 线程 structuredclone::read;(4) onmessage 路径: worker self.postMessage(v) → structuredclone::write → channel → 主 ScriptThread drain → structuredclone::read → 触发 onmessage
- 复用锚点: spec:DF-WK-4/DF-WK-5/DEC-WK-005 | code:WorkerChannelBridge(delegate.rs:341 已有)/StructuredClonePayload(delegate.rs:200 已有)/structuredclone(servo vendor bindings) | pattern:structured_clone_channel
- 依赖: TASK-1
- 状态: pending

### TASK-3: Worker 生命周期管理 + DedicatedWorkerGlobalScope API
- SPEC: REQ-BRW-004 [C4: worker.terminate(), C5: self.close(), C8: DedicatedWorkerGlobalScope API, C9: onerror 传播] | TDD: TEST-BRW-004 | 文件: bao_browser/src/delegate.rs, bao_browser/src/runtime_bridge.rs | 实现: (1) WorkerHandle::terminate() 设 closing flag + 向 servo 发 Terminate 消息;(2) self.close() 经 servo DedicatedWorkerGlobalScopeMethods 实现;(3) DedicatedWorkerGlobalScope API (self/close/importScripts/setTimeout/fetch/crypto/performance/location/navigator) 由 servo 原生提供,bao 仅补充 stealth getter;(4) onerror 经 servo WorkerScriptMsg::DispatchError 传播
- 复用锚点: spec:DF-WK-6/REQ-BRW-004 C4/C5/C8/C9 | code:WorkerHandle(delegate.rs:55 已有)/dedicatedworkerglobalscope.rs(servo vendor 868行) | pattern:worker_lifecycle
- 依赖: TASK-1
- 状态: pending

### TASK-4: Stealth Profile 继承 — Worker 全局指纹与主线程一致
- SPEC: REQ-BRW-004 [C12-C17: navigator/Canvas/WebGL/Audio 指纹一致] | TDD: TEST-BRW-004 | 文件: bao_stealth/src/engine_props.rs, bao_browser/src/runtime_bridge.rs | 实现: (1) worker scope callback 内调用 set_profile_for_global 注册;(2) 三类 Worker (Dedicated/Shared/Service) 继承父页/注册页 Arc<StealthProfile>;(3) WorkerGlobalScope getter 解析命中 REALM_PROFILES;(4) terminate 时 remove_profile_for_global 注销
- 复用锚点: spec:DEC-WK-007/DF-WK-3/DF-WK-9/DF-WK-10 | code:REALM_PROFILES(engine_props.rs:310 已有)/set_profile_for_global(engine_props.rs:325 已有)/remove_profile_for_global(engine_props.rs:354 已有) | pattern:stealth_profile_inherit
- 依赖: TASK-1, TASK-3
- 状态: pending

### TASK-5: Node Realm cross-compartment 代理暴露 Worker 构造函数
- SPEC: DEC-WK-004 [Node Realm 经 cross-compartment proxy 暴露 Worker/SharedWorker/ServiceWorker] | TDD: TEST-BRW-4 | 文件: bao_browser/src/runtime_bridge.rs | 实现: (1) install_lazy_dom_getters 注册 Worker/SharedWorker/ServiceWorker lazy getter;(2) lazy_dom_getter_worker 经 JS_WrapObject 创建 cross-compartment proxy;(3) Node Realm evaluate_js 自动化脚本可访问 Worker 构造函数
- 复用锚点: spec:DEC-WK-004/DF-WK-11 | code:install_lazy_dom_getters(runtime_bridge.rs:600 已有)/lazy_dom_getter_worker(runtime_bridge.rs:698 已有)/JS_WrapObject(runtime_bridge.rs:563 已有) | pattern:cross_compartment_proxy
- 依赖: TASK-1
- 状态: pending

### TASK-6: 页面卸载自动终止 Worker
- SPEC: REQ-BRW-004 [C10: GlobalScope::track_worker + AutoCloseWorker] | TDD: TEST-BRW-004 | 文件: bao_browser/src/delegate.rs, bao_browser/src/page.rs | 实现: (1) BaoWebViewState::track_worker 在 Worker 创建时被调用;(2) AutoCloseWorker Drop impl 调用 WorkerHandle::terminate;(3) 页面导航回调中 terminate_all_workers
- 复用锚点: spec:REQ-BRW-004 C10 | code:AutoCloseWorker(delegate.rs:1844 已有)/BaoWebViewState::track_worker(已有)/terminate_all_workers(已有) | pattern:raii_auto_close
- 依赖: TASK-1, TASK-3
- 状态: pending

### TASK-7: Crash-Safe Teardown — 三路径销毁零崩溃零泄漏
- SPEC: REQ-BRW-004 [C18: terminate/close/page-unload 三路径 crash-safe] | TDD: TEST-BRW-004 | 文件: bao_browser/src/delegate.rs, bao_browser/src/page.rs, bao_stealth/src/engine_props.rs | 实现: (1) closing flag 原子传播;(2) JS interrupt callback 中断 worker 事件循环;(3) 线程 join 无悬挂;(4) REALM_PROFILES 条目注销;(5) EBUSY patch 防止 mutex destroy SIGSEGV
- 复用锚点: spec:REQ-BRW-004 C18/DF-WK-6/DEC-WK-007/NFR-MEMSAF-001 | code:remove_profile_for_global(engine_props.rs:354 已有)/WorkerHandle::terminate(已有)/AutoCloseWorker Drop(已有)/EBUSY patch(mozjs 已应用) | pattern:crash_safe_teardown
- 依赖: TASK-3, TASK-6
- 状态: pending

### TASK-8: SharedWorker 跨页路由
- SPEC: REQ-BRW-4 [C5: SharedWorker 构造函数存在且可创建] | TDD: TEST-BRW-4 | 文件: bao_browser/src/delegate.rs | 实现: (1) servo SharedWorker::Constructor 经 constellation 路由到同一 worker 线程;(2) connect 事件派发 MessagePort;(3) SharedWorkerChannelBridge 管理 per-page port;(4) 页面卸载时断开 port 而非终止 worker;(5) stealth 继承同 TASK-4
- 复用锚点: spec:REQ-BRW-4 C5/DF-WK-7/DEC-WK-001/DEC-WK-006 | code:SharedWorkerChannelBridge(delegate.rs:758 已有)/sharedworker.rs(servo vendor 274行) | pattern:constellation_routing
- 依赖: TASK-1, TASK-2, TASK-3
- 状态: pending

### TASK-9: ServiceWorker 注册 + fetch 拦截（DEC-WK-008 偏离处理）
- SPEC: REQ-BRW-4 [C6: ServiceWorker 构造函数存在且可注册, C19: SW fetch×stealth/CDP边界一致] | TDD: TEST-BRW-4 | 文件: bao_browser/src/delegate.rs | 实现: (1) navigator.serviceWorker.register 注册;(2) ServiceWorkerHandle 跟踪注册/激活/冗余状态;(3) **C19 硬现实**: servo 上游 SW fetch 拦截是 TODO 占位 → 按 DEC-WK-008 选择 vendor patch 或 bao 层桥接;(4) stealth 继承注册页 Arc<StealthProfile>;(5) terminate 后注销 profile
- 复用锚点: spec:REQ-BRW-4 C6/C19/DF-WK-8/DEC-WK-008 | code:ServiceWorkerHandle(delegate.rs:2729 已有)/ServiceWorkerFetchInterceptMode(delegate.rs:2711 已有)/serviceworker.rs(servo vendor 175行) | pattern:serviceworker_registration
- 依赖: TASK-1, TASK-2, TASK-3
- 状态: pending

### TASK-10: 集成与验收测试
- SPEC: TEST-BRW-004, TEST-BRW-4 | TDD: TEST-BRW-004, TEST-BRW-4 | 文件: bao_browser/tests/worker_tests.rs | 实现: (1) DedicatedWorker: 构造/postMessage 双向/terminate/close/onerror/importScripts;(2) SharedWorker: 跨页共享/connect 事件/端口断开;(3) ServiceWorker: 注册/fetch 拦截(若 vendor patch 完成)/注销;(4) Stealth: worker 内 navigator/canvas/webgl/audio 指纹与主线程一致;(5) Crash-safe: 并发创建-销毁 stress test
- 复用锚点: spec:TEST-BRW-004/TEST-BRW-4/NFR-STL-WORKER-1/NFR-MEMSAF-001 | code:worker_tests.rs(已有 1195 行基础)/pagepool_chaos_memory_safety_tests.rs(stress test 模式) | pattern:worker_integration_test
- 依赖: TASK-1, TASK-2, TASK-3, TASK-4, TASK-5, TASK-6, TASK-7, TASK-8, TASK-9
- 状态: pending
