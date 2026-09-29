# GC intentional-leak ledger(#29-Eliminate 正式化 · 2026-09-29 W5)

> 上游清单:S2 A-2(`.plans/spidermonkey-evolution.md` §S2,b61a9b9b 期落账)。
> 本表为 #29-Eliminate 的正式载体:每项上界/回收证明/计数器状态。
> 计数器读取面:`bao_engine::memory_stats::{LEAK_RAW_VALUE_ROOT_GUARD, LEAK_SHUTDOWN_ENGINE, leak_counter}`。
> soak 聚合数据引用:S2 尾项首个数据点(fd=16 恒定/threads=87 恒定/+5.85 KiB/cycle 非 GC 残留,见 evolution §S2 尾)。

## 清单(7 项)

| # | 位置 | 形态 | 上界量化 | 回收证明 | 计数器 | 判定 |
|---|---|---|---|---|---|---|
| 1 | `bao_engine/context.rs` RawValueRootGuard(into_inner + Drop 双站位) | foreign-thread/dead-runtime → `mem::forget` rooted slots | 每次触发 = 一个 rooted-slots 块(`Box<[JSVal]>`,~16 B/slot×slot 数);触发频率=guard 析构遇死 runtime(罕见路径) | 无 OS 回收(进程内 deliberate);释放即 dangling GC scan(更糟)——必要性证明见源内注释 | ✅ `LEAK_RAW_VALUE_ROOT_GUARD`(本波加) | **必要·bounded** |
| 2 | `bao_engine/context.rs` shutdown_engine | `mem::forget(engine)`(OnceLock outstanding assert 规避) | 1 次/进程(受控关机路径独占) | OS 进程退出回收 | ✅ `LEAK_SHUTDOWN_ENGINE`(本波加) | **必要·once-per-process** |
| 3 | `context.rs` NeverDrop/RUNTIME_TLS ManuallyDrop | TLS 析构序(cx 不在 `__call_tls_dtors` 中死) | 1×线程(cx+engine 块,常量级/线程) | OS 线程退出回收 | ✗(线程死亡后计数器自身随 TLS 消亡,不可观测) | **必要**(历史 BCE;timers.rs:12-41) |
| 4 | `timers.rs:40 BAO_RUNTIME_LOOP` | `Box::into_raw` MiniEventLoop | 1×线程(单例,`~sizeof(MiniEventLoop)` 常量级) | OS 线程退出回收 | ✗(跨 crate bun_sm,延后) | **deliberate**(BCE-20260621-001) |
| 5 | `bun_sm/dispatch_sm.rs:60` | runtime drop 链 `mem::forget` | 1×runtime teardown 链 | 所有权链文档化(roots→runtime→engine 受控序) | ✗(bun_sm 跨 crate,本波域外延后) | **必要** |
| 6 | fetch_async/bun_listen/bun_udp/node_http/node_fs 等 `Box::into_raw` FFI userdata | C 回调侧配对 `from_raw` | —(非泄漏:配对回收模式) | 配对回收即证明 | ✗(非 leak,不设) | **非 leak** |
| 7 | `node_vm.rs VM_CONTEXT_MAP` | 未 root realm global 裸指针+永不删 key | —(已消除) | §B 修复即证明 | ✗(已消除) | **已根治** |

## soak 数据引用(S2 尾项)
- fd=16 恒定、threads=87 恒定:零描述符/零线程泄漏。
- +5.85 KiB/cycle 非 GC 残留:与 #3/#4(1/thread 常量级)+ N1(线程死亡 TLS 丢弃)量级一致——per-event bounded、线性累积的 C 级残留;非 GC rooting 失败。

## W5 新发现(待裁决,未入 A-2)
- **zone-chunk 内存驻留**:realm create/drop N 次后 zone_count 随 GC 全量回收(102→2 / 302→2,两次 JS_GC 后),但 `gc_heap_chunk_total` 线性驻留(~2.1 MB/realm:100→203 MiB、300→603 MiB,双 GC 后不回落,servo_gc_heap_decommitted=0)。zone 结构体=回收 ✓;chunk 字节驻留=SM chunk 池语义或真驻留,**裁决权在用户/主会话**(W5 段1 STOP 条款触发)。数据:`bench-harness zone-eval --iterations N`(本波新增模式)。

## 处置裁定(主会话 2026-09-29)
chunk 池线性驻留(~2.1 MB/realm,双 GC 不回落,decommit=0)= **known-limitation**:上游 SM chunk-pool 语义;生产调用面零 fresh-realm churn(e1 核实);影响面=测试/bench 长 context churn 场景。上游排查(arena/chunk decommit 触发条件)记 backlog 非阻断。与 soak 22KiB/cycle malloc 泄漏**互斥已证**(单上下文实验 vs 每页 runtime 销毁路径,两现象不同源)。
