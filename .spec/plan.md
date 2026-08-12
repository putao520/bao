# 开发计划: 删 bao_engine::WebWorker bypass + 重接线 servo 原生 Worker 路径 (BCE-20260627-008 根治) | epoch: 6 | status: active

## epoch
6

## status
active

## 背景与根因

**BCE-20260627-008**: DEC-WK-001 架构未落地 — bypass WebWorker 与 servo 原生路径双轨并存。

SPEC DEC-WK-001 要求三类 Worker 全经 servo 原生路径(vendor/servo/components/script/dom/workers/),复用 servo Worker::Constructor + DedicatedWorkerGlobalScope::run_worker_scope。但实际:
- bao_browser::create_worker (lib.rs:224) 仍调 bao_engine::WebWorker::new_with_structured_clone(bypass)
- bypass 是 bao 层自建 mozjs 线程 + SIMPLE_GLOBAL_CLASS 全局,完全旁路 servo
- WorkerLocation/WorkerNavigator/onerror dispatch 全加在 bypass 上(技术债)
- bypass terminate 对 while(true) JS 无 interrupt callback → concurrent_terminate 测试只能 #[ignore]

servo vendor patch 已就绪(register_worker_scope_callback + dedicatedworkerglobalscope.rs:525 drain_worker_scope_callbacks),但 bao_browser 调用入口未重接线。

## reqLedger
| REQ ID | 交付范围 | 覆盖 TASK |
|--------|---------|----------|
| REQ-BRW-004 | C1-C19 全经 servo 原生(删 bypass) | TASK-1,2,3,4 |
| DEC-WK-001 | servo 原生 Worker 路径落地 | TASK-1,2 |
| DEC-WK-002 | Worker 全局 = servo DedicatedWorkerGlobalScope | TASK-2 |
| DEC-WK-003 | 删 bypass(Node worker_threads 独立,不受影响) | TASK-1 |
| DEC-WK-005 | 跨线程只传 servo StructuredSerializedData | TASK-2 |
| DEC-WK-007 | Arc<StealthProfile> 经 vendor callback 注入 | TASK-2 |
| DF-WK-1 | Worker 构造经 servo Worker::Constructor | TASK-1,2 |
| DF-WK-6 | terminate 经 servo DedicatedWorkerControlMsg + interrupt callback | TASK-3 |
| BCE-20260627-008 | 删 bypass + 重接线 | TASK-1,2,3,4 |

## 范围

### REQ
- REQ-BRW-004 C1-C19: 全部经 servo 原生 Worker 路径(删 bypass)

### Decision
- DEC-WK-001/002/003/005/007

### Dataflow
- DF-WK-1(构造经 servo)、DF-WK-6(terminate 经 servo interrupt)

### Bug-Knowledge
- BCE-20260627-008(本 epoch 根治目标)

## 影响矩阵
| SPEC ID | 关联 TASK | 文件 |
|---------|----------|------|
| DEC-WK-001, BCE-20260627-008 | TASK-1 | src/bun_sm/src/lib.rs, src/bun_sm/Cargo.toml, src/bao_engine/src/lib.rs |
| REQ-BRW-004, DEC-WK-001/002/005/007, DF-WK-1 | TASK-2 | src/bao_browser/src/lib.rs, src/bao_browser/src/runtime_bridge.rs |
| REQ-BRW-004 C4/C18, DF-WK-6, DEC-WK-001 | TASK-3 | src/bao_browser/src/delegate.rs |
| BCE-20260627-008 | TASK-4 | src/bun_sm/src/web_worker.rs(删整文件) |
| 全量确认 | TASK-5 | oracle_gate + verify + grep bypass=0 |

## 任务树(扁平,串行依赖,禁止并行 — 文件域耦合)

### TASK-1: 删 bypass 源头 re-export
- SPEC: DEC-WK-003 [删 bypass,Node worker_threads 独立] | 文件: src/bun_sm/src/lib.rs(删 pub use web_worker::WebWorker), src/bun_sm/Cargo.toml(删 url dep), src/bao_engine/src/lib.rs:74(删 WebWorker re-export) | 实现: 删 3 处 re-export/依赖。注意 bun_sm/src/web_worker.rs 文件本身暂不删(TASK-4 删),先断 re-export 链
- 依赖: 无
- 状态: pending

### TASK-2: bao_browser create_worker 重接线 servo 原生(依赖 TASK-1)
- SPEC: REQ-BRW-004 [C1-C19 全经 servo 原生] DEC-WK-001/002/005/007 DF-WK-1 | 文件: src/bao_browser/src/lib.rs(create_worker/create_worker_with_url 改 servo 路径), src/bao_browser/src/runtime_bridge.rs(删 create_worker_with_script_loader/inline_script 对 WebWorker 的调用) | 实现: create_worker 改为经 servo Worker::Constructor(页面 JS new Worker(url) 触发,bao 层只注册 register_worker_scope_callback_native 注入 stealth profile + CDP 观测)。bao 层不再 spawn worker 线程
- 依赖: TASK-1
- 状态: pending

### TASK-3: delegate.rs WorkerHandle 状态管理替换(依赖 TASK-2)
- SPEC: REQ-BRW-004 C4/C18 DF-WK-6 DEC-WK-001 | 文件: src/bao_browser/src/delegate.rs | 实现: 删 web_worker: Option<&bao_engine::WebWorker> 字段 + web_workers HashMap,改为 WorkerHandle(servo Worker DOM 对象句柄代理)。terminate 经 servo DedicatedWorkerControlMsg::Exit + interrupt callback(DF-WK-6)。取消 concurrent_terminate 测试的 #[ignore](servo 原生 interrupt 可工作)
- 依赖: TASK-2
- 状态: pending

### TASK-4: 删 bypass 整文件(依赖 TASK-3)
- SPEC: DEC-WK-003 [删 bypass] BCE-20260627-008 | 文件: src/bun_sm/src/web_worker.rs(删整文件 1861 行) | 实现: 确认无任何代码引用后删整文件。保留 bun_sm/src/lib.rs 已断的 re-export
- 依赖: TASK-3
- 状态: pending

### TASK-5: 全量确认(依赖 TASK-4)
- SPEC: BCE-20260627-008 残留=0 | 实现: (1) grep bao_engine::WebWorker 全项目 = 0;(2) cargo build --all;(3) cargo test -p bao_browser --test worker_tests 全 pass;(4) cargo test -p bao_browser --test bce004_stress_tests 全 pass(含 concurrent_terminate,无 #[ignore]);(5) oracle_gate(REQ-BRW-004)
- 依赖: TASK-4
- 状态: pending
