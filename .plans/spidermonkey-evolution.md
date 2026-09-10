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

- ~~Bao 源码未发现 `JS_RequestInterruptCallback` 接入~~ → 2026-09-04 已落地最小闭环：`src/bao_engine/src/execution_control.rs`（JS_AddInterruptCallback once-per-JSContext + owner 线程 armed 栈 + thread-safe cancel + deadline watcher + TerminalState + reset 防污染；内部试验面）；2026-09-10 S1 已接线真实入口（`BaoRuntime::{eval,eval_module}_with_control`，script/module/事件循环泵 whole-entry 覆盖 + #25 排序合同测试锁定，见 §8 S1 节）；servo 侧入口（ScriptThread/worker realm eval）与产品级暴露（CLI flag/SIGINT）未接；
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
| #23 | Realm / Compartment / Zone topology | P0 | — | OPEN（S0 topology census 已完成 2026-09-10，见 §8；余 capability/stale-object 测试与 Zone 实测数据） |
| #24 | Interrupt / timeout / cancellation | P0 | #23 最终 policy；审计可并行 | OPEN（S1 已接线 bao_runtime script/module 入口 + whole-entry 泵覆盖，2026-09-10 见 §8；servo 侧入口与产品级暴露未接） |
| #25 | JobQueue / scheduler ordering | P0 | #23 最终 Realm ownership | OPEN（S1 已落调用点 inventory + 排序合同测试，2026-09-10 见 §8；Node 分歧 2 项待裁决 + navigation/close lifecycle 待收口） |
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

### 2026-09-10 / S0 收口——#23 Realm/Compartment/Zone topology census（纯审计轮，零代码改动）

**基线**：bao master `642da220`；mozjs bao-mozjs-sys 140.14.0-0（bindgen jsapi.rs 25342 行，
`/var/cargo-builds/.../bao-mozjs-sys-*/out/build/jsapi.rs`，下称 `jsapi.rs`）。
扫描方法：`command grep` + 阳性对照（记忆教训：ugrep 桥字面 pattern 假阴性）。

#### S0-1 vendored mozjs Realm/Compartment/Zone API 面（jsapi 可达，行号=jsapi.rs）

创建/所有权：

| API | 位置 | 说明 |
|---|---|---|
| `JS::CompartmentSpecifier` | jsapi.rs:10824-10830 | `NewCompartmentInSystemZone=0` / `NewCompartmentInExistingZone=1` / `NewCompartmentAndZone=2` / `ExistingCompartment=3` |
| `JS::RealmCreationOptions` | jsapi.rs:10846-10865 | `traceGlobal_` / `compSpec_` + union{`comp_`,`zone_`} / `profilerRealmID_` / `locale_` / `invisibleToDebugger_` / `preserveJitCode_` / `sharedMemoryAndAtomics_` / `defineSharedArrayBufferConstructor_` / `coopAndCoep_` / `toSource_` / `secureContext_` / `freezeBuiltins_` / `forceUTC_` / `alwaysUseFdlibm_` |
| C++ 默认 compSpec | `src-js/mozjs/js/public/RealmOptions.h:234` | **默认 `NewCompartmentAndZone`**（非 system zone）。Rust 无 setter 绑定（`setNewCompartmentIn*` 未 bind），只能裸字段改 `compSpec_` + union——servo 与 bao 均如此用 |
| `JS::RealmBehaviors` | jsapi.rs:10932 区域 | `rtpCallerType` / `discardSource_` / `clampAndJitterTime_` / `isNonLive_` |
| `JS::RealmOptions` | jsapi.rs:10955 | creationOptions_ + behaviors_；Rust 侧 `mozjs::rust::RealmOptions`（rust.rs:133-157，glue `JS_NewRealmOptions`/`DeleteRealmOptions`） |
| `JS_NewGlobalObject` | jsapi.rs:16682；wrapper `jsapi2_wrappers.in.rs:395` | 唯一 global 创建入口 |
| `JS_FireOnNewGlobalObject` | jsapi.rs:16697；wrapper :396 | DontFireOnNewGlobalHook 路径手动补射 |
| `JS::EnterRealm`/`LeaveRealm` | jsapi.rs:7906/7913；wrapper jsapi2_wrappers.in.rs:92-93；safe 封装 `mozjs::realm::AutoRealm`（realm.rs，LIFO 强制） | realm 进入/退出 |
| `GetRealmPrincipals`/`SetRealmPrincipals` | jsapi.rs:14966/14970 | principal 挂 realm |
| `JS_SetTrustedPrincipals` | wrapper（servo 用） | system realm 判定锚 |

拓扑查询/判定：

| API | 位置 | 说明 |
|---|---|---|
| `JS::GetCurrentRealmOrNull` / `GetObjectRealmOrNull` / `CurrentGlobalOrNull` | jsapi.rs:7800/7804/10019 | realm/global 查询 |
| `js::IsSystemCompartment` / `IsSystemZone` | jsapi.rs:5829/5833 | system 判定 |
| `js::IsSharableCompartment` | jsapi.rs:5902；C++ impl `jsfriendapi.cpp:663` | **语义=live global 存在 且 未 nuked outgoing wrappers**（与 origin 无关） |
| `JS_IterateCompartments` / `JS_IterateCompartmentsInZone` / `IterateRealmsInCompartment` | wrapper jsapi2_wrappers.in.rs:488/489/170 | 拓扑枚举（S2/S4 可作自省测试载体） |
| `JS::Zone` | jsapi.rs:6435（opaque） | **无 realm→zone 反查绑定**（RealmZoneIter/GetZoneOf 不存在）——zone 归属只能靠创建时的 compSpec 记账 |

**stop 条件未触发**：Realm API 面完全可辨，无 binding 缺口阻塞。

#### S0-2 servo 侧创建点 inventory（单一咽喉）

**唯一创建咽喉**：`components/script_bindings/interface.rs:134 create_global_object`（codegen
`CGWrapGlobalMethod` 对每个 global interface 生成调用，codegen.py:3441）：

- `RealmOptions::default()` + `traceGlobal_` + `sharedMemoryAndAtomics_=false`；
- `use_system_compartment=true` → `NewCompartmentAndZone` + `JS_SetTrustedPrincipals`（**仅
  DebuggerGlobalScope**，Bindings.conf:239；bao 运行期不出现）；
- 否则 `select_compartment`（interface.rs:196-230）：`JS_IterateCompartments` 找第一个
  sharable 非 system compartment → `ExistingCompartment`（**复用**）；找不到 →
  `NewCompartmentAndZone`。**callback 不查 origin**——同 origin 分离由 constellation
  event-loop 层保证（跨 origin = 独立 pipeline/ScriptThread），非 principal 保证；
- principals：`ServoJSPrincipals::new(origin)`（principals.rs:26，MutableOrigin 装箱进
  JSPrincipals private）——servo content realm 全部携带 origin principals。

| Realm 创建点 | 线程 / JSContext | Compartment | Zone |
|---|---|---|---|
| Window（页面/同 event-loop iframe） | ScriptThread 线程（`Script#{id}`，script_thread.rs:869-888 spawn，`Runtime::new` thread-local cx） | 该 cx 第一个 content compartment（首个=NewCompartmentAndZone，后续同 event-loop 文档 `ExistingCompartment` 共享） | 该 compartment 自带 zone（1 content zone/cx） |
| DedicatedWorker / SharedWorker / ServiceWorker / Worklet global | 各自 worker 线程自己的 thread-local cx（同一 codegen 咽喉） | 新 cx 首个 global → `NewCompartmentAndZone` | 1 zone/worker cx |
| DebuggerGlobalScope | （理论）ScriptThread cx | 新 compartment + trusted principals | 新 zone（bao 不触发） |

#### S0-3 bao 层创建点 + 与 servo 拓扑的映射（重叠/独占/冲突三分类）

| # | Bao 概念 | 创建点 | 线程/cx | SM 拓扑（compSpec） | principals | 分类 |
|---|---|---|---|---|---|---|
| 1 | Page Realm（servo Window global） | servo `create_global_object` | ScriptThread | servo select_compartment | origin principals | **重叠-一致**（bao 零重复创建；`PAGE_GLOBAL_BY_WEBVIEW` 只收 servo 回调给的指针） |
| 2 | Node Realm（per WebViewId） | runtime_bridge.rs:654 `create_node_realm_native`（ScriptThread 回调内；**显式** NewCompartmentAndZone :649-650；SAB=true node 语义；测试 4322-4325 锁死） | ScriptThread（与页面同 cx） | 独立 compartment+zone | null | **Bao-独占**（servo 不知道；与 page compartment 物理隔离=REQ-BRW-003 C10 契约） |
| 3 | JsContext persistent realm（CLI eval / bao_engine WebWorker bypass / BaoRuntime） | context.rs:831 `ensure_realm_global`（lazy 一次性；`node_realm_options()` 默认 compSpec=NewCompartmentAndZone；AddRawValueRoot；发布 THREAD_REALM_GLOBAL） | CLI 线程 / 每 worker 线程自己的 cx | 独立 compartment+zone | null | **Bao-独占**（无 servo 参与的场景） |
| 4 | CLI module 入口 realm | module_loader.rs:360/:503 `eval_module(_then)`——**每次调用 fresh global**（fresh compartment+zone） | CLI 线程 | 独立 compartment+zone（per 调用） | null | **Bao-独占 + 风险(c)**（churn，见下） |
| 5 | `vm.createContext` sandbox | node_vm.rs:657（每 contextify 一次；node_realm_options 默认） | 调用者 cx（CLI persistent realm 或 Node Realm 所在 ScriptThread cx） | 独立 compartment+zone | null | **Bao-独占**（Node 语义正确） |
| 6 | servo-native Worker/SW realm | servo codegen 咽喉 | worker 线程 | NewCompartmentAndZone | origin principals | **重叠-一致**（bao 只经 vendor-patch 回调注入 hooks，DEC-WK-001/003） |
| 7 | `JSGlobalObject::create` | global_object.rs:111 | — | — | — | 可用-未用（零 bao 调用） |
| 8 | REALM_PROFILES + PAGE_GLOBAL_BY_WEBVIEW + NODE_REALM_BY_WEBVIEW | engine_props.rs:333（`DashMap<global_addr, Arc<RealmProfile>>` + alias 链 :363）、runtime_bridge.rs:80-115 | 跨线程（仅存 usize 地址，无 JSObject 跨线程） | — | — | **Bao 自管 identity 投影层**（非第二 owner：从不创建/销毁 realm，只做 global_addr→profile/identity 映射；nav 时 alias old→new + node realm context-swap 重建，runtime_bridge.rs:340-361） |

**冲突/风险清单**（#23「禁止与 Servo 重复 ownership」核查结论）：

- (a) 同 event-loop 同 origin iframe 与父页面共享 compartment——对象级隔离非 compartment 级
  （浏览器常态；stealth profile 按 global_addr 键控不受影响）；
- (b) navigation context-swap 重建 Node Realm → 旧 node realm zone 只随旧 cx 整体销毁回收，
  无显式 zone 回收（#29 输入）；
- (c) CLI `eval_module` 每次调用 fresh compartment+zone——CLI 长进程多次跑模块=zone churn
  （#26/#29 输入；`eval_module_in_realm` 变体已存在，browser worker 路径 node_worker_threads.rs:589
  用的是 in-realm 版，无此问题）;
- (d) servo `select_compartment` 不查 origin——依赖 constellation event-loop 分离；bao PagePool
  每页独立 pipeline → 实际无跨页共享；若未来出现同 cx 多 origin global 会静默共 compartment
  （已记录为设计约束，非当前缺陷）；
- (e) 全部 bao realm principals=null（servo content realm 有 origin principals）——node realm ↔
  page realm 跨 compartment 桥接必须且已经走 `JS_WrapObject`/AutoRealm（runtime_bridge.rs:635），
  无裸跨 compartment 借用。

**Page/Host/Worker topology 图（现状）**：

```
进程（唯一 JSEngine）
├─ CLI 线程 cx（bao run / BaoRuntime）
│   ├─ [3] persistent realm（compartment+zone，SAB=true）
│   ├─ [4] 每 eval_module 调用 fresh compartment+zone（churn 风险 c）
│   └─ [5] 每 vm.createContext fresh compartment+zone
├─ ScriptThread#N cx（每页面 pipeline 一个；同 event-loop iframe 共 cx）
│   ├─ [1] content compartment+zone：Window global(s) 共享（servo select_compartment）
│   └─ [2] Node Realm compartment+zone（per WebViewId；nav context-swap 重建）
├─ Worker 线程 cx（servo-native DedicatedWorker/SharedWorker）
│   └─ [6] Worker global realm = 新 compartment+zone（servo-owned）
├─ SW 线程 cx
│   └─ [6] ServiceWorkerGlobalScope realm = 新 compartment+zone（servo-owned）
├─ bao_engine WebWorker 线程 cx（bypass 轨，DEC-WK-003）
│   └─ [3] persistent realm = 新 compartment+zone
└─ 进程级注册表（非 realm owner，纯投影）：REALM_PROFILES(addr→profile+alias 链) /
    PAGE_GLOBAL_BY_WEBVIEW / NODE_REALM_BY_WEBVIEW（跨线程只存 usize）
```

#### S0-4 裁决清单（哪些绑 SM 原生 / 哪些 Bao-Servo 自管）

| 隔离边界 | 裁决 | 依据 |
|---|---|---|
| Page/Web realm | **绑 SM 原生（现状即终态）** | servo 拥有 Window realm 全生命周期；bao 零重复创建（S0-3 #1）；#23 禁止绕过 Servo 新建 Window Realm 已满足 |
| Worker/SW realm | **绑 SM 原生（servo-owned）** | 同 codegen 咽喉；bao 只注入（DEC-WK-001）；bypass 轨（bao_engine WebWorker）保持 Bao 自管，仅存在于无 servo 场景（DEC-WK-003 双轨） |
| Node Realm（browser 模式） | **保持 Bao 自管** | Node 语义需 SAB=true（servo `create_global_object` 硬编码 false）+ Bun globals；独立 compartment+zone 是 REQ-BRW-003 C10 物理隔离契约（测试锁死）；必须在 ScriptThread cx 内创建（同线程铁律） |
| vm sandbox | **保持 Bao 自管** | Node `vm` 语义每 context 独立 compartment 是 Node 正确行为 |
| identity/profile 注册表 | **保持 Bao 自管投影层，非 owner** | 从不创建/销毁 realm；nav invalidation 已闭环（alias+重建）；addr-keyed ABA 残留风险归 #29 |
| Zone strategy | **维持现状（每 bao realm 独立 zone + servo 默认共享 content compartment/zone）**——实测数据前不动 | #23 要求以 memory/churn 实测裁决；当前 zone churn 源=(b)(c) 归 #29/#26 量化；若未来合并 node realm 进共享 zone（`NewCompartmentInExistingZone`，绑定已可达）需先有 churn/RSS 数据 + CCW 成本评估 |

#### S0-5 #23 DoD 对照

- 创建点 inventory 100%：✅（单一咽喉 interface.rs:134 + bao 5 处，S0-2/S0-3）；
- topology + 裁决证据落账本：✅（本节）；
- 无第二套隐式 Realm owner：✅（S0-3 #8 为投影非 owner）；
- 其余 DoD（capability isolation 测试、stale-object 负测、Zone strategy 实测数据、#15/#20 接收）
  为 #23 后续编码波，非本审计轮范围。

**下一唯一动作**：S1——把 ExecutionControl 接到 bao_runtime 脚本入口与 scheduler
（#24/#25 合流；realm topology 前置输入已就绪：所有入口的 owner 线程/cx/realm 锚点已在
S0-3 表中，`AutoRealm`/`eval_with_control` 无 binding 缺口）。

### 2026-09-10 / S1——ExecutionControl 接线 bao_runtime 入口 + #25 scheduler 排序合同（单 slice：接线+测试+合同落账）

**基线**：bao master `642da220`（S0 收口同日）；mozjs 不变（bao-mozjs-sys 140.14.0-0）。

**目标与边界**：把 S0-A 的引擎级 ExecutionControl 接到真实生产入口——`BaoRuntime::eval` /
`eval_module`（即 `bao -e` / `bao run` 所用 body），#25 调用点 inventory + 排序合同落账本并
测试锁定。**零新增用户可见行为**（无 CLI flag / 信号接线——产品级暴露属稳定公开行为，
触发契约第 5 条停点，见文末提案记录）。

**代码改动**：

- `bao_engine/src/execution_control.rs`：新增 generic `JsContext::run_with_control(control,
  timeout, run)`——把 S0-A 版 `eval_with_control` 的 arm/latch/终止错误映射六步从 eval body
  解耦，任意 owner 线程执行路径可控；`eval_with_control` 改为薄委托（行为零变化，373 基线
  证明）。deadline 语义文档化：终止的是**正在执行的 JS**（loop back-edge / JIT 栈检查观察
  interrupt），事件循环自然空闲后按自身 liveness 收尾不受罚——deadline 是 runaway 控制不是
  总时长 bound。
- `bao_runtime/src/runtime.rs`：三个 `#[doc(hidden)]` 内部试验面入口——`execution_control()`
  （绑定本 runtime owner 线程 cx，外部线程仅可 cancel()/poll）、`eval_with_control`（script
  入口）、`eval_module_with_control`（module 入口；control 挂在 **WHOLE entry** 周围：
  ModuleLink/ModuleEvaluate + post-eval 事件循环泵——runaway 在 module 顶层或 timer/job/pump
  回调内 equally 被终止；终止以稳定 termination error 流出入口，中途 kill 不伪装成功）。

**测试**（`bao_runtime/tests/suite/execution_control_entry_tests.rs`，5 用例）：

1. module 入口 `while(true){try/catch}` + 500ms deadline → **TimedOut** 稳定终态（<5s prompt、
   ≥400ms 证来自 deadline、`<execution-control>` 稳定错误、终止后 runtime 可复用）；
2. script 入口 runaway + 外部线程 `cancel()` → **Cancelled** 稳定终态（<5s）；**退出码边界
   合同**：`should_exit()==false` + `exit_code()==0`——证明终止以真实 `Err` 流出入口、走 CLI
   既有 `Err(_) => Err(1)` 臂（bao_cli/src/cli.rs run_eval/run_file，零 CLI 改动），不伪装
   exit 0；
3. timer 回调内 runaway（`setTimeout(25ms){ while(true){} }`）→ deadline 终止 mid-pump
   （`__fired='in-callback'` 证明真的进入回调后才被杀）——whole-entry arm 覆盖证据；
4. 正常 module 快速完成（不阻塞到未用 deadline）+ 同 control 复用零污染（arm 时 reset 清
   既有 latch）；
5. #25 排序合同确定序锁定（见下）。

**#25 scheduler inventory（审计落账：谁 drain / 何时 / 谁唤醒）**：

SM JobQueue（微任务/promise job）结构性 drain 点：

| 调用点 | 何时 drain | 唤醒者 |
|---|---|---|
| `bao_engine/context.rs:787` `JsContext::eval` | evaluate_script 返回后立即（script 入口 microtask checkpoint） | 同步 |
| `context.rs:790` eval post_eval_hook loop 头 | 每个事件循环 pass 前 | eval 循环（1ms sleep 节拍） |
| `bun_sm/module_loader.rs` `drain_job_queue`（4 个 `eval_module*` 变体；ModuleEvaluate 后 + hook loop 头） | module 入口 | 同上 |
| `bao_runtime/timers.rs:534,560` `drain_and_check`（= `post_eval_drain_then_exit`，`bao run` 的 hook） | due timers 批之后 + pumps 之后 | eval 循环 |
| `timers.rs:672,687` `drain_one_pass` | bun:test runner 每 pass | test runner |
| `node_worker_threads.rs:607,621` | worker message pump 每 pass | worker 消息 |
| 直接 `RunJobs`：bun_listen / bun_udp / fetch_async / node_vm / bun_build / require / bun_test / `dispatch_sm.rs:263`（ConcurrentTask dispatch） | native completion 后立即 checkpoint | 各 native 完成源（ConcurrentTask eventfd 唤醒 MiniEventLoop） |

Timer/唤醒源：`BAO_REGISTRY` wall-clock deadline（per-pass 检查）；MiniEventLoop（uSockets
epoll；跨线程 ConcurrentTask enqueue 经 eventfd 唤醒）；eval 循环 1ms sleep 节拍。

**排序合同（测试锁定）**：sync body → microtask checkpoint（RunJobs：nextTick[经
queueMicrotask 入队] / promise continuation / queueMicrotask，入队 FIFO）→ event-loop pass
（drain_and_check：due bao timers **整批**先火 → JobQueue::drain → pumps[ws/fswatch/cluster/
spawn] → JobQueue::drain → liveness verdict）→ 'exit' listeners 仅 liveness=false 后
（post_eval_drain_then_exit）。

**已知 Node 分歧（记录不改动，#25 后续裁决项）**：① due timers 整批先火、批后才 drain
微任务（Node 在每个 timer 回调之间 drain）；② nextTick 与 promise 微任务同队列 FIFO（Node
nextTick 队列严格先行）。

**SPEC 立法提案停点（未实施）**：CLI `--timeout` / SIGINT→cancel 等产品级暴露属稳定公开
行为，需 SPEC 立法后接线（本 slice 全部 `#[doc(hidden)]` 内部试验面）。

**BCE 检查**：能力接线非 bug 修复；同类面=「control 终止伪装成功/污染下一 entry」已由
run_with_control 的 latch-wins 映射 + arm 时 reset + 测试 2/4 锁定，无同类残留实例。

**验证**（波末一次测）：`cargo nt -p bao_engine` **373/373 passed**（delegation 重构零行为
变化）；`cargo nt -p bun_runtime` **1209 passed / 1 pre-existing skipped**（含 5 新用例，
72.4s）。

**回滚点**：单 commit revert（4 文件：execution_control.rs / runtime.rs / suite 新测试文件 +
suite main.rs），无数据/接口迁移。

**下一唯一动作**：S1 续——navigation/page-close/runtime-shutdown pending-work 永久悬挂
审计收口（S1 目标第 4 条；输入=本节 inventory），同轮裁决 #25 已记录 Node 分歧（nextTick
独立队列 / timer 间微任务 drain 是否对齐）。

---

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
