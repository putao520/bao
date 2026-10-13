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

## e207 spin 形嵌入者事件通道积压(2026-10-13 发现→当日根治;REQ-LIB-002)

**形态**:e199(3aad40a57)起 `BrowserRuntime::new` 恒装 runtime 自有 servo 事件通道(无界可靠 mpsc,REQ-CDP-004 立法:永不丢弃、无高水位自弃),接收端留在 runtime 上且仅 run/run_until/pump_cdp/run_with_bridge 四个内置泵面显式 drain;`spin_event_loop` 包装与 PageHandle 13 个等待循环 spin 点——churn/soak 嵌入方的唯一泵形态——不排空,逐 cycle 永久积压(suite 负对照实测:console 事件 3 轮零 drain 即 emitted>pumped,积压真实非 vacuous)。

**根治**:1423e97f3(基于 dad7fd576)——接收端+tap 移入 BaoServoDelegate(`install_runtime_event_drain`),一切 spin 面 spin 后即排空(`drain_runtime_events`);PageInner 增 servo_delegate 字段,`spin_and_drain` 覆盖 13 个等待循环点;`pump_servo_events_to_taps` 保留 pub 契约面(4c30afd5b)但 drain 体走 delegate 单真源。锁:`spin_face_drains_runtime_event_channel_no_backlog`(RED pre-fix → GREEN post-fix)。

**判定**:必要性=可靠队列立法(REQ-CDP-004)排除有界/丢弃方案,排空责任必须由库侧全泵面承担;修复后缺省无漏,手动泵契约降为 belt-and-suspenders。

**二分边界账(终版,P1-P4 控制矩阵后改判)**:soak 判别载具全矩阵(10 run)证伪「泄漏进窗」叙事——形态与 commit 零相关、与机器负载强相关(收缩仅在 loadavg≳14 出现:A60/C60/P1 与 10-11 nightly(15min 均值 14.7);单调全部在 load≲10:FIX60/P2/P3 与 10-12/13 nightly)。**P4 对照:健康锚 7343d4069 全速低载同样单调(+52..+77/6min,21.3k cycles 零收缩)**,与 92d90290e/ff5786571/HEAD+fix 逐段同值——10-11「健康」与 10-12/13「泄漏」的形态差=负载 regime 差,非代码差;夜跑 verdict 的「线性泄漏进窗」判定不成立(混淆变量=负载)。仍真实的数据:低载下所有构建(含锚)60min/36k cycles 持续 ~+15-20 KiB/cycle 无收缩——或为 ≤7343d4069 已存在的慢性驻留(账面 W5/W6 已知类),或为 >60min 周期回收机制;P3 门斜率判定须速率归一+负载门控+emitted/pumped 对账(§U 立法候选强化)。本修封闭的通道积压结构独立成立(锁 RED→GREEN+计数器探针)。
