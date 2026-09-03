# SpiderMonkey 原生能力进化计划

> 创建日期：2026-09-01
> 状态：ACTIVE
> 主控 Issue：#22
> 执行者：Bao 现有 daily-ops / 定时 Agent
> 规则：本文件是 SM-EVOLUTION 唯一长期状态账本；Issue 定义边界，本文件记录真实审计、执行、验证与下一动作。

---

## 0. 目标

Bao 不再只把 SpiderMonkey 当作 JSC/V8 的替代执行引擎，而要系统利用其 embedding-native 能力增强 Runtime：

1. Realm / Compartment / Zone → 生命周期、隔离与 GC topology；
2. Interrupt → timeout / cancellation / runaway-script control；
3. JobQueue → Promise/microtask 与 Servo/Node/Bun scheduler 统一；
4. Stencil / XDR / off-thread compile → 脚本预编译与多 Realm 复用；
5. Debugger / Memory → CDP 与 Runtime observability 的真实引擎事实；
6. Realm policy → locale/timezone/JIT/shared-memory 的原生配置；
7. GC/rooting/Zone reclamation → page/worker churn 与长期内存治理；
8. Upstream capability audit → mozjs 升级后自动发现新能力和 API 漂移。

本计划不追求“把所有 SpiderMonkey API 都暴露给用户”。每项能力必须裁决为：

- **Use internally**：Bao 内部使用；
- **Expose as Bao capability**：形成稳定产品能力；
- **Deliberately unused**：明确不采用；
- **Blocked / Experimental**：当前 API 稳定性或 Servo/mozjs 约束不足。

---

## 1. 当前基线

### Bao Runtime

- 进程级唯一 JSEngine；
- 每个 Servo ScriptThread / owner thread 使用自己的 thread-local JSContext；
- JSObject / GC cell 禁止跨线程；
- `bao_engine::context::JsContext` 已有 persistent realm global；
- 已有 GC extra-root / `RawValueRootGuard`；
- JSEngine / Runtime teardown 有历史 BCE 与 mozjs fork patch；
- Page Realm / Node Realm 已形成产品隔离语义，但尚未系统映射到 SM Realm/Compartment/Zone topology。

### JobQueue

Bao 已使用 mozjs `CreateJobQueue` / `SetJobQueue` / `RunJobs`：

- Promise job GC-safe rooting；
- job 保存 source global 并在 drain 时进入正确 Realm；
- job throw → uncaught exception router；
- drain tail → unhandled rejection flush。

因此 JobQueue 工作不是重写，而是收敛 scheduler ordering、navigation/close lifecycle 和 starvation。

### 当前明确缺口

- ~~Bao 源码未发现 `JS_RequestInterruptCallback` 接入~~ → 2026-09-04 已落地最小闭环：`src/bao_engine/src/execution_control.rs`（JS_AddInterruptCallback once-per-JSContext + owner 线程 armed 栈 + thread-safe cancel + deadline watcher + TerminalState + reset 防污染；内部试验面，S1 继续统一到全部 eval 入口与 scheduler）；
- 未形成 Bao Stencil/XDR script cache（binding 已具备 Stencil wrappers，见 ledger）；
- 未发现 Realm-native locale/timezone override 的 Bao 侧使用；
- CDP Debugger 仍未证明由 SM 原生 debugger/script/frame/object facts 驱动（JS::Debugger binding 缺失，bun_sm::debugger 为 emulated CRUD）；
- GC/rooting 中仍有 intentional leak / `mem::forget` / foreign-thread fail-safe 路径，需量化；
- mozjs upgrade 已有 patch replay + capability ledger（`.claude/sm-capability-ledger.json`，2026-09-04 首轮 #30 census）；drift 自动化（升级波 diff 报告）待下次 mozjs 前移时首跑。

### Upstream

- Servo/mozjs 当前公开 `RealmOptions.h` 中有 Realm policy/locale/timezone 等入口；
- `Interrupt.h` 提供 interrupt callback/request API；
- `experimental/JSStencil.h`、`experimental/CompileScript.h` 和 Stencil XDR implementation 存在；
- experimental API 第一阶段默认仅内部试验，不自动进入稳定 public API。

---

## 2. Issue 集合

| Issue | Domain | 优先级 | 依赖 | 状态 |
|---|---|---:|---|---|
| #22 | META / scheduled-agent contract | P0 | — | OPEN |
| #23 | Realm / Compartment / Zone topology | P0 | — | OPEN |
| #24 | Interrupt / timeout / cancellation | P0 | #23 最终 policy；审计可并行 | OPEN |
| #25 | JobQueue / scheduler ordering | P0 | #23 最终 Realm ownership | OPEN |
| #26 | Stencil / XDR / off-thread compile | P1 | #23 | OPEN |
| #27 | Debugger / Memory / CDP observability | P1 | #23 | OPEN |
| #28 | Realm locale/timezone/JIT/shared-memory policy | P1 | #23 | OPEN |
| #29 | GC/rooting/Zone reclamation | P0/P1 | #23 | OPEN |
| #30 | mozjs capability inventory/drift automation | P0 | — | OPEN |

与 Bao 1.0 Domain 的消费关系：

- #15 Runtime ← #23 #24 #25 #29
- #16 Stealth ← #23 #28
- #11 CDP ← #24 #27
- #19 Performance/Soak ← #25 #26 #27 #28 #29
- #20 Security/Capability ← #23 #24 #28 #29
- #18 Platform/Build ← #30（mozjs upgrade/release reproducibility）

---

## 3. 执行 Phase

### Phase S0 — Capability census + topology（第一优先）

执行：#30 + #23

目标：先知道“当前 SM 到底有什么、Bao 到底已经用了什么”，再改代码。

交付：

1. 当前 mozjs ref / version / BuildId 记录；
2. `js/public/**` + Rust binding capability inventory；
3. Bao code adoption mapping；
4. Realm/Compartment/Zone 创建点 inventory；
5. Page/Host/Worker 当前 topology 图；
6. machine-readable SM capability ledger；
7. 本文件更新成真实审计数据。

完成门槛：不是“文档写完”，而是从审计中至少选出一个 P0 可执行 slice 继续编码。

### Phase S1 — Execution correctness/control

执行：#24 + #25

目标：

- SM Interrupt 接入统一 ExecutionControl；
- `while(true){}` 可 deterministic timeout/cancel；
- microtask / nextTick / timer / Servo task / native completion ordering 固化；
- navigation/page-close/runtime-shutdown pending work 无永久悬挂。

这是 Stencil/JIT 优化前的硬前置。

### Phase S2 — GC/lifecycle closure

执行：#29，消费 #23 topology。

目标：

- root/GC-pointer inventory 100%；
- async root owner/release path 100%；
- page/worker/navigation churn 无 stale root/UAF；
- intentional leak 有计数、上界、实测触发率；
- 100+ page churn 可被 #19 soak 量化。

### Phase S3 — Engine leverage/performance

执行：#26 + #28

目标：

- Stencil in-memory cache；
- XDR persistent cache（只有正确性完成后）；
- off-thread compile（只有 benchmark 证明收益后）；
- Realm-native locale/timezone；
- JIT preserve policy 基于生命周期和 benchmark；
- SharedMemory/Atomics policy 与 Security/Web/Node/Bun 对齐。

### Phase S4 — Debugger/observability

执行：#27

目标：

- 建立单一 SM typed debug adapter；
- CDP Runtime/Debugger 使用真实 script/frame/object/exception facts；
- engine memory/GC metrics 可供 #19 soak 使用；
- Chrome/V8-specific 不可表示项 Explicitly Unsupported。

### Phase S5 — Continuous evolution

执行：#30 常驻 daily-ops。

以后每次 mozjs baseline 前移：

1. capability diff；
2. Bao patch supersession check；
3. adoption classification；
4. relevant capability → 新/现有 Issue；
5. ledger + 本 MD 更新；
6. scoped/full validation；
7. wave closure。

---

## 4. Scheduled Agent 强制循环

每轮定时开发必须执行：

```text
READ
  #22 + open children
  .plans/spidermonkey-evolution.md
  CLAUDE.md
  .claude/upstream-baseline.json
    ↓
REBASE FACTS
  当前 Bao / mozjs / Servo ref
    ↓
AUDIT CURRENT CODE
  native / emulated / missing / unused
    ↓
SELECT ONE UNBLOCKED SLICE
  correctness > safety > lifecycle > leverage > perf > observability
    ↓
UPDATE THIS MD BEFORE CODE
  scope + tests + rollback point
    ↓
IMPLEMENT
    ↓
SCOPED NEXTTEST / BCE / BENCHMARK
    ↓
UPDATE THIS MD WITH REAL EVIDENCE
    ↓
COMMIT / WAVE CLOSURE
    ↓
SET EXACT NEXT ACTION
```

### 禁止“只计划不执行”

若存在未阻塞的可执行 slice：

- 不能以“完成审计/完成计划/建议下一步”为本轮终点；
- 必须继续写代码和测试；
- 一个 slice 未闭环前不要开多个半成品 slice。

只有真实 blocker（上游 API 缺失、SPEC 立法阻塞、环境工具不可用等）才能停写代码；必须记录 blocker 证据并自动切换下一个未阻塞 Issue。

---

## 5. SpiderMonkey capability inventory（2026-09-04 #30 S0-A 真实 census 数据）

机器 ledger SSOT：`.claude/sm-capability-ledger.json`（55 capabilities / 13 domains，逐项
symbol/header/stability/bao_status/code_refs/issue/last_audited=2026-09-04）。本表只保留
Domain summary（数字来自 ledger，禁手工漂移）。

| Domain | capabilities | used-native | wrapped | emulated | engine-internal | missing | deliberately-unused |
|---|---:|---:|---:|---:|---:|---:|---:|
| runtime/context/realm/compartment/zone | 8 | 4 | — | — | — | 3 | 1 |
| compile/stencil/module/xdr/cache | 7 | 3 | — | — | — | 4 | — |
| jobs/promise/event-loop | 2 | 2 | — | — | — | — | — |
| interrupt/cancellation | 4 | — | 2 | — | 1 | — | 1 |
| debugger/profiling/memory | 8 | 1 | — | 1 | — | 5 | 1 |
| GC/rooting/heap/weak-refs | 6 | 3 | — | — | 1 | 2 | — |
| structured-clone/serialization | 1 | 1 | — | — | — | — | — |
| wasm | 2 | — | — | — | 1 | — | 1 |
| intl/locale/timezone | 3 | — | — | — | 1 | 2 | — |
| shared-memory/atomics | 3 | 1 | — | — | 1 | 1 | — |
| principals/security/options | 3 | — | — | — | 1 | 2 | — |
| embedding-hooks/callbacks | 7 | 3 | — | — | — | 4 | — |
| core-value/object-surface（基础面汇总条目） | 1 | 1 | — | — | — | — | — |
| **合计** | **55** | **19** | **2** | **1** | **6** | **23** | **4** |

关键事实修正（相对 2026-09-01 种子表）：

- Interrupt：种子表 `missing` → **wrapped**（本轮 #24 最小闭环，`src/bao_engine/src/execution_control.rs`）。
- Stencil：种子表「待绑定」→ mozjs binding **已具备** 10 个 Stencil wrappers
  （`jsapi2_wrappers.in.rs:330-341`，CompileGlobalScriptToStencil/Instantiate/DecodeStencil 等），
  bao 侧零调用——#26 是纯接入工作，无 binding 阻塞。
- Debugger：JS::Debugger C++ API 在 mozjs rust binding **不可达**（bun_sm/src/debugger.rs 自述），
  CDP Debugger 复用受 binding 阻塞（emulated 现状）→ #27 需先裁决 binding 扩展 vs adapter 自持。
- Structured Clone / RealmOptions / SetPromiseRejectionTracker / SAB realm flag：种子表
  unknown → **used-native**（node_worker_threads / node_realm_options / uncaught.rs / global_object.rs）。
- Principals/security：zero 使用——Page/Node 隔离目前靠 object-level（分 global），非 principal-level。
- mozjs baseline：bao-mozjs 0.22.0 / bao-mozjs-sys 140.14.0-0 / servo-mozjs main `eb36274`。

注意：本表不是完成证明；drift 自动化（升级波 capability diff 报告）在下次 mozjs 前移时首跑。

---

## 6. 第一轮 Agent 建议动作

### Slice S0-A：mozjs capability census + existing adoption scan

优先执行 #30，但**不能只生成报告结束**。

步骤：

1. 固定当前 vendored mozjs commit/version；
2. 扫描 `js/public/**`、`experimental/**` 与 Rust wrappers；
3. 生成 capability-level ledger，而不是 bindgen symbol dump；
4. grep Bao 对 RealmOptions / Interrupt / Stencil / Debugger / GC API 的当前使用；
5. 更新本 MD 的初始 inventory；
6. 紧接着从 #24 选择一个最小可执行 slice：
   - 安装 interrupt callback；
   - 为一个内部 eval path 增加 cancellation state；
   - 加 `while(true){}` timeout regression；
7. scoped nextest；
8. 若暴露 bug class，执行 BCE；
9. 写真实结果和 commit 到本文件。

这能保证第一轮不是“又写了一份计划”。

---

## 7. 验证纪律

日常：

```bash
cargo nt -p <crate>
# 或
cargo nextest run --cargo-profile test-ci -p <crate> -E '<filterset>'
```

涉及 SpiderMonkey process-singleton / plain cargo test 时遵守仓库 `CLAUDE.md` 的隔离纪律。

涉及 crash/hang/UAF/exception/lifecycle：必须 BCE。

性能：必须 before/after，记录：

- command
- hardware/environment
- Bao commit
- mozjs ref
- sample count
- median/p95（按 benchmark methodology）
- RSS/CPU where relevant

---

## 8. 每轮执行记录

### 2026-09-01 / bootstrap

**来源**：#22–#30 初始规划。

**已确认事实**：

- Bao 已有 native SM JobQueue；
- Bao 已有 persistent Realm global 与 GC extra-root guard；
- Bao 未发现 `JS_RequestInterruptCallback` 使用；
- Bao 未形成 Stencil/XDR script cache；
- Bao 未发现 Realm locale/timezone override 接入；
- daily-ops 已负责每日 issue 根治与 mozjs 跨版本升级，不建立第二 scheduler。

**代码改动**：仅建立长期计划账本；实际功能实现交由下一次 daily-ops 从 S0-A 立即开始。

**下一唯一动作**：

> 执行 #30 S0-A capability census，并在同一轮继续实现 #24 的第一个 interrupt/cancellation 最小闭环；不得在 census/plan 阶段结束。

### 2026-09-02 / daily-ops live 轮

**代码改动**：无 SM slice——本轮工程预算用于上游吸收批次 1（bun 5 项 correctness，commit `c0a09301`）；file_lock MCP 已恢复（09-01 回归清除），本程序启动阻塞解除。

**下一唯一动作**：沿袭 2026-09-01 bootstrap 裁决——S0-A capability census + 同轮 #24 interrupt/cancellation 最小闭环，不得以 census 结束。

### 2026-09-04 / S0-A census + #24 最小闭环（单 slice：census + 代码 + 测试）

**基线**：bao master `4976f330`；mozjs bao-mozjs 0.22.0 / bao-mozjs-sys 140.14.0-0 / servo-mozjs main `eb36274`。

**Census（#30 首轮，真实扫描）**：

- 扫描面：`vendor/mozjs/src-js/mozjs/js/public/**`（105 headers）+ `experimental/`（11）+ mozjs rust binding（`rust.rs` / `jsapi2_wrappers.in.rs` / bindgen out）。
- Bao 采用面：`command grep` 全 `src/` 树（每个"零命中"结论均带阳性对照——`JS_NewGlobalObject` 对照组命中正常）。
- 产出：`.claude/sm-capability-ledger.json`（55 capabilities / 13 domains；
  used-native 19 / wrapped 2 / emulated 1 / engine-internal 6 / missing 23 / deliberately-unused 4；
  schema=symbol/header/stability/bao_status/code_refs/note/issue/last_audited=2026-09-04）。
- 本文件 §1 缺口与 §5 inventory 表已更新为真实数据。
- 关键新事实：Stencil binding 已具备（10 wrappers，非 binding 阻塞）；JS::Debugger binding 不可达
  （#27 受阻需裁决）；`JS_ResetInterruptCallback(cx, enable)` 在 vendored 版语义为
  `interruptCallbackDisabled = enable`（参数命名与直觉相反，已记入 ledger note）。

**代码改动（#24 最小闭环）**：

- 新增 `src/bao_engine/src/execution_control.rs`：
  - `ExecutionControl`（Arc 共享 handle；外部线程仅 `cancel()`=atomic flag + 文档化线程安全
    `JS_RequestInterruptCallback`，零 JSObject/GC 指针跨线程）；
  - `TerminalState`（Running/Completed/Errored/Cancelled/TimedOut，AtomicU8 latch，首写者赢）；
  - owner 线程 armed 栈（thread-local；callback 只看栈顶；**空栈必返回 true**——引擎内部 GC
    interrupt 不得误杀共享 context 上的无关脚本，这是安全不变量）；
  - `JS_AddInterruptCallback` once-per-JSContext 安装（TLS 地址追踪；Runtime 重建/销毁路径重置）；
  - deadline watcher（condvar 可取消，eval 帧内 join——request 只发生在 context 可证存活窗口；
    快速 eval 不阻塞到 deadline）;
  - `JsContext::eval_with_control`（复用 `eval` 持久 realm 路径；terminated 时引擎自清 pending
    exception——`HandleInterrupt`→`reportUncatchableException` 已核实——返回稳定终止错误）。
- `src/bao_engine/src/context.rs`：`init_runtime`/`for_test` Runtime 创建后 + `shutdown_thread_sm`
  销毁前重置 callback 安装追踪（防地址复用 stale-skip）。
- `src/bao_engine/src/lib.rs`：`pub mod execution_control`。
- 内部试验面承诺边界：全部 `#[doc(hidden)]`，无 pub 稳定 API 承诺（S1 统一时再定合同）。

**测试**（`src/bao_engine/tests/suite/execution_control_tests.rs`，suite 单 harness 约定）：

1. `while(true){try/catch}` + 500ms deadline → TimedOut 稳定终态（不可 catch、<5s、≥400ms）；
2. 正常 eval 不受影响（Completed=42、快速返回不阻塞到 5s deadline、JS 错误 latch Errored、
   无 control 的 plain eval 正常）；
3. timeout 后 `reset()` 二次 eval 零污染（同 control 复用 + plain eval）；
4. 外部线程 `cancel()` → Cancelled 稳定终态（<5s）。

**验证**：`cargo nt -p bao_engine`（波末一次测）：**373 run / 373 passed / 0 failed / 0 skipped**，
含 `execution_control_tests::test_execution_control_all`（1.129s，四子项时延与各自 deadline 相符，
证实终止来自 deadline/cancel 而非早退）。

**BCE 检查**：本轮为能力新增非 bug 修复；同类横扫面=「engine-interrupt 回调误杀空栈脚本」类
不变量已以 armed-栈空栈-continue 设计 + 测试 2 锁定，无同类残留实例。

**下一唯一动作**：S0 收口——#23 Realm/Compartment/Zone topology census（Realm/Zone 创建点
inventory + Page/Host/Worker topology 图落账本），随后 S1 把 ExecutionControl 接到
bao_runtime 脚本入口与 scheduler（#24/#25 合流）。

---

## 9. 最终完成定义

本计划完成时必须满足：

1. SM embedding capability inventory 100% classified；
2. #23–#30 均完成或有明确 deliberately-unused/blocked 裁决；
3. #15/#16/#11/#19/#20 已实际消费相应成果；
4. `while(true){}` 等 CPU runaway 有 deterministic control；
5. scheduler ordering/lifecycle 有测试合同；
6. Stencil/cache 只有在 benchmark 证明收益后保留；
7. locale/timezone 等身份事实优先走 engine-native mechanism；
8. GC/root/leak 可观测且进入 72h soak；
9. mozjs upgrade 自动产生 capability drift/adoption report；
10. 不存在长期“只写计划不执行”的 open slice。
