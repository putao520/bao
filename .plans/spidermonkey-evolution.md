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

- ~~Bao 源码未发现 `JS_RequestInterruptCallback` 接入~~ → 2026-09-04 已落地最小闭环：`src/bao_engine/src/execution_control.rs`（JS_AddInterruptCallback once-per-JSContext + owner 线程 armed 栈 + thread-safe cancel + deadline watcher + TerminalState + reset 防污染；内部试验面）；2026-09-10 S1 已接线真实入口（`BaoRuntime::{eval,eval_module}_with_control`，script/module/事件循环泵 whole-entry 覆盖 + #25 排序合同测试锁定，见 §8 S1 节）；~~产品级暴露（CLI flag/SIGINT）未接~~ → 2026-09-11 已接（CLI `--timeout <ms>` + SIGINT→cancel，立法提案消费，见 §8 S1 CLI 消费节）；servo 侧入口（ScriptThread/worker realm eval）未接；
- 未形成 Bao Stencil/XDR script cache（binding 已具备 Stencil wrappers，见 ledger）；
- 未发现 Realm-native locale/timezone override 的 Bao 侧使用；
- CDP Debugger 仍未证明由 SM 原生 debugger/script/frame/object facts 驱动（JS::Debugger binding 缺失，bun_sm::debugger 为 emulated CRUD）；
- GC/rooting 中仍有 intentional leak / `mem::forget` / foreign-thread fail-safe 路径，需量化（2026-09-10 S2 已完成全仓行号级 inventory 与首个缺陷类根治——vm context 未 root 注册表，见 §8 S2 节；bounded leak 分类清单已落账，soak 触发率量化仍缺）；
- mozjs upgrade 已有 patch replay + capability ledger（`.claude/sm-capability-ledger.json`，2026-09-04 首轮 #30 census）；drift 自动化 v1 已落地（`scripts/sm-audit/` 三脚本 + seed inventory/adoption + `.claude/sm-audit/BASELINE` 基线指针，2026-09-10，见 §8 #30 节）；升级波首跑待下次 mozjs 前移。

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
| #24 | Interrupt / timeout / cancellation | P0 | #23 最终 policy；审计可并行 | OPEN（S1 已接线 bao_runtime script/module 入口 + whole-entry 泵覆盖，2026-09-10 见 §8；**产品级暴露已接 2026-09-11**：CLI `--timeout <ms>` + SIGINT→cancel（立法提案消费，见 §8 S1 CLI 消费节）；servo 侧入口未接） |
| #25 | JobQueue / scheduler ordering | P0 | #23 最终 Realm ownership | OPEN（S1 已落调用点 inventory + 排序合同测试，2026-09-10 见 §8；S1-续已裁决分歧①（per-timer 微任务 checkpoint，已落地）+②（nextTick 独立队列，方案已记待实现）+ navigation/close/shutdown pending-work 审计（1 红项立法提案待用户，见 §8 S1-续） |
| #26 | Stencil / XDR / off-thread compile | P1 | #23 | CLOSED-实现(in-memory per-JSContext cache 落地+接线+兑现 2026-09-11:5.21× 实测 vs 4.9× 判据,80.8% 编译占比消除,engine 383/stealth 1691/browser 族 503 零回归,见 §8 实现节;XDR encode 绑定缺口独立排程、off-thread 关闭不变) |
| #27 | Debugger / Memory / CDP observability | P1 | #23 | OPEN（核查轮 2026-09-10 完成：可达面+自模拟面+裁决提案见 §8；**裁决 2/3/9 已消费 2026-09-10**：CDP Debugger 胶水保真批换原生 + blackbox 显式 unsupported + bun_sm::debugger 死模块删除 + 三真缺口根治（G1 face 原生装出/G2 Node Realm compartment 落位/G3 console 通道回传），live e2e 绿，见 §8 #27 消费节；**裁决 6 已消费 2026-09-10**：Memory 计量换原生 CollectRuntimeStats（jsglue 构造 + bao_engine 内部面 + soak 采样点，见 §8 #19 节）；GC callback 归 #19 待接） |
| #28 | Realm locale/timezone/JIT/shared-memory policy | P1 | #23 | CLOSED（2026-09-11 裁决消费闭：三维身份泄漏 locale/tz/时间精度引擎原生根治+live 证据（engine 376/stealth 1689/browser stealth 族 160 零回归 + live 4/4），见 §8 消费节；JIT/SAB 维持现状=终态裁定；#16 Stealth 身份一致性三维闭） |
| #29 | GC/rooting/Zone reclamation | P0/P1 | #23 | OPEN（S2+S2-续+S2-续2+S2-续3 落地 2026-09-10：全仓 rooting/leak inventory 落账 + vm context 未 root 根治 + bun_api concatArrayBuffers frame 级未 root 根治（RED→GREEN 双 SIGSEGV 实证）+ NODE_REALM_TRACER_CX dedupe ABA 根治 + VM_CONTEXT_MAP flags 死数据清除——代码面 findings ①②③全闭，见 §8 S2/S2-续/S2-续2/S2-续3 节；soak 量化（前置 #19）仍挂；RED-1 已闭——用户裁决 P-A 落地 2026-09-10，见 §8 末节） |
| #30 | mozjs capability inventory/drift automation | P0 | — | OPEN（自动化 v1 已落地 2026-09-10：三脚本+seed inventory+adoption report+drift 基线，见 §8；升级波首跑+DoD 收口待下次 mozjs 前移） |

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
  **2026-09-10 #27 修正**：该判定仅对 Rust typed C++ face 成立；JS-face（JS_DefineDebuggerObject，
  wrapped）一直可达，Bao CDP Debugger 域实际已走 page-realm JS Debugger（非 emulated）——
  裁决=保持 JS-face，详见 §8 #27 节。
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
→ **已消费（用户裁决 2026-09-10「全部完成」；2026-09-11 落地）**：CLI `--timeout <ms>` +
SIGINT→cancel 产品级暴露已接线，见 §8 「S1 CLI 消费」节。

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

### 2026-09-10 / S1-续——navigation/page-close/shutdown pending-work 悬挂核查 + #25 Node 分歧两项裁决（审计+裁决+分歧①落地）

**基线**：bao master `dbbbcc8f`（S1 同日）；mozjs 不变。

#### A. pending-work 归属结构事实（三场景共用）

全部 pending 结构均 thread-local（绑定 owner 线程的 cx）：

| pending 结构 | 位置 | realm 锚定方式 |
|---|---|---|
| bao JobQueue（JOB_IDS） | bao_engine/job_queue.rs:53 thread-local VecDeque | job 存 global 属性；**global 本身无第二根**（依赖 realm 存活） |
| servo MicrotaskQueue（page/worker realm 微任务） | script_runtime.rs:884 CreateJobQueue+SetJobQueue；microtask.rs:31 | `EnqueuedPromiseCallback.global: Dom<GlobalScope>`（microtask.rs:79）——**根持 global，realm 生命线** |
| bao BAO_REGISTRY timers | timers.rs:51 thread-local | `global_root` AddRawValueRoot 原根（timers.rs:1174）——**根持 registration global，realm 生命线**；Drop 时 RemoveRawValueRoot（timers.rs:1318，cx 死亡则跳过） |
| MiniEventLoop（ConcurrentTask 队列） | timers.rs:40 thread-local，Box::into_raw **leak 永不释放** | 任务各自持 JS 侧根（gc_store/raw root） |

关键拓扑:servo constellation 按 **registered domain(eTLD+1)+webview** 复用 EventLoop
（constellation.rs:874 get_event_loop / bc_group.event_loops 键=Host）——**同 registered
domain 导航=同一 ScriptThread/cx 存活,旧 realm 在同线程内 discard**;跨 registered domain
才换新 ScriptThread。bao 每页独立 WebView+独立 bc group（无 opener）→ 跨页恒独立线程。

#### B. 三场景核查矩阵(悬挂=红,明确终态=绿)

| 场景 | pending 类 | 归属(drain/丢弃/等待) | 终态 | 证据 |
|---|---|---|---|---|
| **N1 导航·跨 registered domain** | 全部(servo 微任务/BAO timers/JOB_IDS/ConcurrentTask) | 旧 pipeline ExitScriptThread(event_loop.rs:53 Drop 发消息)→handle_exit_pipeline_msg(script_thread.rs:3619,逐 doc clear_js_runtime+cancel_all_tasks_and_ignore_future_tasks)→线程退出→thread-local dtor 丢弃 | **丢弃(随线程死)** | script_thread.rs:3619-3706;BAO_REGISTRY Box drop 无 JS 调用(timers.rs:1318, current_cx 已清);leaked MiniEventLoop 残任务永不 tick=OS 进程退出回收(#29 记账) | 绿 |
| **N2 导航·同 registered domain(主流场景,ScriptThread 存活)** | servo 微任务(旧 realm) | Dom<GlobalScope> 根持;下一次 checkpoint(任意 pump 步骤1 RunJobs)照常执行 | **执行(spec 一致:微任务队列属 event loop,旧 doc 的已入队微任务合法运行)** | microtask.rs:76-94;script_runtime.rs:318-327 | 绿 |
| **N2 同上** | **BAO_REGISTRY timers(旧 realm)** | **已闭(RED-1 P-A 落地 2026-09-10,§8 末节)**:vendor hook 于 `handle_exit_pipeline_msg`(window_detached 门**之前**——detached 分支整个跳过 clear_js_runtime,hook 只挂那里会竞态漏 purge)→ bao `cancel_timers_for_global` 按 global 精确清条目+释 raw root+zombie-fire 探针;实测发现同域导航旧 pipeline exit 常态**迟到**(有时只到 teardown),迟到期间旧 realm 活着非 zombie——确定性 discard 面=page close/线程 teardown | 终态=discard 时清零;zombie 执行计数=0(e2e 锁定) | script_thread.rs(handle_exit_pipeline_msg hook);timers.rs(cancel_timers_for_global/zombie 探针) | **绿(已闭)** |
| **N2 同上** | ConcurrentTask 完成指向旧 realm | 该线程 MiniEventLoop 共享(不分 realm);完成照常派发进旧 realm(一次性,bounded) | 执行一次(与 RED-1 同族 discard 语义缺口,非永久) | pump_embedder_thread 步骤3(timers.rs:341) | 黄(随 RED-1 一并裁决) |
| **page-close** | 全部 | page.close()(page.rs:1290,embedder 线程:terminate workers+清注册表)→PageInner drop→WebViewInner::drop(webview.rs:144)→CloseWebView→constellation 逐 pipeline 退出→每 ScriptThread 走 N1 路径 | **丢弃(随线程死;不 drain——已关页面的 pending 不再执行=浏览器语义)** | page.rs:1290-1363;webview.rs:144-152;script_thread.rs:3619 | 绿 |
| **shutdown·browser** | 全部 | BaoRuntime::drop→close_all(lib.rs:644-653)→每页 page.close() 同上;ServoInner::drop(servo.rs:872)spin 至 ScriptThreads join | 丢弃 | 同上+servo.rs:872-880 | 绿 |
| **shutdown·CLI(bao run/-e)** | 微任务/JOB_IDS | drain_and_check 每遍历末尾 JobQueue::drain(timers.rs:534,560)→liveness 判定前队列恒空(判定不含 JobQueue 的原因:drain 刚跑过) | 执行完毕(自持链在单次 RunJobs 内耗尽) | timers.rs:532-580 | 绿 |
| | timers/IO | has_pending_work(timers.rs:694)持续等待至无 pending(Node 语义:timers 保活进程) | 等待后执行 | timers.rs:442-580 | 绿 |
| | process.exit 短路 | drain_and_check 入口 should_exit→false 直接断(bun_api.rs:7233-7239 post_eval_drain_then_exit→'exit' handlers→exit) | 丢弃(Node 'exit' 后 setTimeout 永不跑的官方语义) | timers.rs:445;Node process 文档 exit 节 | 绿 |
| **CLI JsContext 销毁** | 残留一切 | shutdown_thread_sm(context.rs:651):RootedTraceableSet clear+JS_DestroyContext;BAO_REGISTRY 主线程随进程丢弃 | 丢弃 | context.rs:651-689 | 绿 |
| **bao JOB_IDS 裸指针 liveness 专项** | bao job queue 残留 job | 安装面仅 CLI JsContext(持久 realm=线程寿命)与 node_worker_threads bypass(线程寿命);CLI eval_module fresh realm 的 job 每遍历 drain 后才退出/退出时队列恒空;无 realm 先死场景 | 无悬挂指针活化路径 | job_queue.rs 安装点 context.rs:491/536/607+node_worker_threads.rs:457 | 绿 |

#### C. RED-1 立法提案(用户裁决 2026-09-10 P-A 直做——已落地,见 §8 末节 2026-09-10 / RED-1)

**现象**:同 registered domain 导航后,旧 realm 的 bao setTimeout/setInterval 继续按
deadline 在旧(已 discard,WindowState::Zombie)realm 里执行;interval 永续;旧 realm 被
global_root 原根 pin 无法回收;每次同域导航累积。浏览器语义(Chrome 非 bfcache 逐出路径:
导航即销毁旧 doc timers)与 Bao 反指纹保真均要求终止。

**为何不能 bao 层自修**:BAO_REGISTRY 是 thread-local,只有 ScriptThread 本人可 purge;
realm discard 事件发生在 servo `Window::clear_js_runtime`(vendor 代码),无既有 embedder
回调(对照:pump/settings-runner 两桥均为 dispatch 侧注册)。修复必须新 vendor patch:
在 clear_js_runtime(或 handle_exit_pipeline_msg)处调用 bao 注册的
`cancel_timers_for_global(global)`(镜像 register_bao_event_loop_pump 桥模式)。

**双方案(用户裁决)**:
- P-A(对齐浏览器):discard 时 cancel 该 global 的全部 BAO_REGISTRY timers(含
  ConcurrentTask 旧 realm 完成路径同族裁决);
- P-B(维持现状+护栏):fire 前查 global 存活登记表,死 global 静默 drop——仍需 vendor
  hook 登记 discard,且 zombie window 期间(deadline 早于 discard 后首个 GC)依旧执行,
  不推荐,列出仅为完备。

**连带**:#29 输入——interval zombie + realm pin 是 nav churn 内存增长源之一;修 P-A 后
nav 累积消除。

#### D. #25 分歧裁决(两项,Node 官方依据)

**① timers 整批后 drain 微任务 → 裁决:对齐 Node ≥11(=浏览器 task 语义),本轮已落地。**

- 依据:Node 11.0.0 release notes(nodejs.org/en/blog/release/v11.0.0)Notable Changes:
  「nextTick queue will be run after each immediate and timer」+ SEMVER-MAJOR 条目
  「timers: run nextTicks after each immediate and timer (Anatoli Papirovski) #22842」
  (2018-10 起 Node 全系如此;HTML 事件循环同构:每 task 后 microtask checkpoint,
  timer callback=task)。Bao 现状=Node ≤10 legacy。无产品级保留理由(双目标 Bun 兼容+
  浏览器保真同向)→ 非用户裁决面,工程对齐。
- 落地:`drain_bao_timers` fire 循环内、`fire_js` 之后 `CURRENT_FIRING_TIMER` 清零之前
  插入 `js::RunJobs`(timers.rs:843-861)——微任务里的 clearInterval 仍命中
  CLEARED_DURING_FIRE 守卫(interval re-arm 正确跳过);js::RunJobs 自动路由到所在 cx
  的队列(bao JOB_IDS / servo MicrotaskQueue),CLI drain_and_check、test-runner
  drain_one_pass、servo pump_embedder_thread 三个 drain 消费方同点受益(单一咽喉)。
- 合同测试更新:execution_control_entry_tests.rs #5 增两 timer 穿刺用例
  (t1@20ms 排 continuation、t2@45ms:期望 t1,t1-cont,t2——旧批序为 t1,t2,t1-cont;
  断言 timing-robust:同批 due 时 heap 序仍保证 t1 先火、checkpoint 在循环内)。

**② nextTick 与微任务同队列 FIFO → 裁决:对齐 Node CJS 语义(独立 next-tick 队列),目标
确定,实现属大改,本轮只记方案。**

- 依据:Node 官方 process 文档(nodejs.org/api/process.html,v26):
  「`process.nextTick()` adds callback to the "next tick queue". This queue is fully
  drained after the current operation … before the event loop is allowed to continue」;
  「every time the "next tick queue" is drained, the microtask queue is drained
  immediately after」;「in CJS modules process.nextTick() callbacks are always run
  before queueMicrotask() ones」(官方示例 CJS 输出 `nextTick, resolve, microtask`;
  ESM 反序——ESM 顶层本身处于微任务 drain 中)。Bao 现状(process_nextTick 委托
  queueMicrotask,bun_api.rs:7296)在 nextTick 后排的 promise/qmt 之前不插队=与 CJS
  反序。
- 方案(未实现):新 thread-local `NEXT_TICK_QUEUE`(回调+args 经 gc_store 或 raw root
  锚 registration global,镜像 timers 模式);`drain_next_ticks(cx)` 循环「清空
  next-tick 队列(每回调 AutoRealm+settings-runner 进注册 realm)→ RunJobs 清微任务
  → 若 next-tick 又被填则重复」;在 S1 inventory 的全部 checkpoint 位
  (context.rs eval 后/module_loader 4 变体/各 pump/dispatch_sm:263)以
  checkpoint(cx)=drain_next_ticks+RunJobs 包装替换;runaway 递归 nextTick 由
  ExecutionControl whole-entry arm 覆盖(可终止);合同测试重写(首 checkpoint 序不变
  sync,nextTick,microtask,qmt…,需加 CJS 插队用例)。规模 4-6 文件,含全部 drain 位
  触达+合同测试——超出本轮「小改落地」线,排 S2 前 #25 收尾波。
- 注记:Node 自 v22.7.0 起 nextTick 标 Stability: 3 - Legacy(官方推荐
  queueMicrotask)——不影响裁决(兼容承诺以现状 Node 语义为准),但提示新代码面无需
  扩展 nextTick 特性。

**BCE 检查**:分歧①落地非 bug 修复;同类面=「批内多 timer 的 continuation 晚于同批后继
timer」已由新两 timer 用例锁定;RED-1 属立法提案未动代码。

**验证**(波末一次测):`cargo nt -p bao_engine` **373/373**;`cargo nt -p bun_runtime`
**1209 passed / 1 pre-existing skipped**(含更新后的 ordering 合同)。

**回滚点**:单 commit revert(timers.rs drain 循环 6 行 + 合同测试更新 + 账本),无
数据/接口迁移。

**遗留移交**:RED-1 立法提案(§C)待用户裁决;分歧②方案待 #25 收尾波;leaked
MiniEventLoop 残任务与 interval zombie 内存形态归 #29。

**下一唯一动作**:S2——GC/lifecycle closure(#29:root/GC-pointer inventory 100% +
async root owner/release 100%,输入=S0-3 拓扑表与本轮 RED-1/泄漏记账);若用户先裁决
RED-1 P-A,则优先插队其 vendor patch 落地波。

### 2026-09-10 / S2——#29 GC/rooting 全仓 inventory + vm context 未 root 缺陷类根治（inventory+代码+测试）

**基线**：bao master `642da220`（S1-续同日）；mozjs 不变。RED-1（BAO_REGISTRY interval
zombie/realm pin）按用户裁决挂起——本轮零触碰。

#### A. 全仓 rooting/leak inventory（行号级，`command grep` + 阳性对照）

**A-1 正确 root（必要，不动）**：

| # | 位置 | 机制 |
|---|---|---|
| 1 | `bao_engine/src/context.rs:80 PersistentGlobal` | persistent realm global `AddRawValueRoot` cx 寿命；Drop 带存活守卫移除 |
| 2 | `bao_engine/src/context.rs:142 RawValueRootGuard` | async 窗口值根（Box 钉地址）；Drop/into_inner 移除；失败回滚前缀 |
| 3 | `context.rs:238 THREAD_REALM_GLOBAL` | #1 已 root 的 global 地址发布（dispatch AutoRealm 锚） |
| 4 | `bao_runtime/src/gc_store.rs` | 值挂 realm global 属性——GC 自然 trace（**模范模式，本轮 vm 修复的范式来源**） |
| 5 | `bao_runtime/src/timers.rs:971` BAO_REGISTRY `global_root` | AddRawValueRoot/Drop 移除（cx 死亡跳过）——**RED-1 关联禁碰** |
| 6 | `bao_browser/src/runtime_bridge.rs:167 trace_node_realm_roots` | R2 修复后 per-cx extra GC roots tracer（全树唯一 `JS_AddExtraGCRootsTracer`） |
| 7 | `PAGE_GLOBAL_BY_WEBVIEW`/`PER_THREAD_PAGE_GLOBAL`（runtime_bridge.rs:117/:205） | servo-owned Window global 生命周期（servo realm 持有；runtime_bridge.rs:755 注释明示） |
| 8 | `REALM_PROFILES`（engine_props.rs:333） | usize→Arc<RealmProfile>，无 GC 指针（纯投影层） |
| 9 | 作用域 `rooted!`（全树千级） | 栈根 RAII，正确惯用 |
| 10 | `node_tls.rs:4136` 等 `RawValueRootGuard` 消费点 | RAII 正用（tasklet 跨 tick 根） |

**A-2 主动 leak 分类（bounded/deliberate，全部记录不动）**：

| # | 位置 | 形态 | 裁决 |
|---|---|---|---|
| 1 | `context.rs:205/:227` | RawValueRootGuard foreign-thread/dead-runtime → `mem::forget` rooted slots | **必要**（root table 可能仍持地址；释放=dangling GC scan 地址，严格更糟；文档化 bounded） |
| 2 | `context.rs:719` | shutdown_engine `mem::forget(engine)` | **必要**（OnceLock handle 致 outstanding>0 assert；进程退出一次） |
| 3 | `context.rs NeverDrop`/RUNTIME_TLS ManuallyDrop | TLS 析构序（cx 不在 `__call_tls_dtors` 中死） | **必要**（历史 BCE；timers.rs:12-41 注释） |
| 4 | `timers.rs:40 BAO_RUNTIME_LOOP` | `Box::into_raw` MiniEventLoop | **deliberate**（BCE-20260621-001 reentry 根治；1/thread 单例；OS 回收） |
| 5 | `bun_sm/dispatch_sm.rs:60` | `mem::forget` runtime drop 链 | **必要**（文档化所有权链） |
| 6 | fetch_async/bun_listen/bun_udp/node_http/node_fs 等 `Box::into_raw` FFI userdata | C 回调侧配对 `from_raw` | **非 leak**（配对回收模式） |
| 7 | `node_vm.rs VM_CONTEXT_MAP`（本轮前） | 未 root 的 realm global 裸指针 + 永不删除的 key | **可消除缺陷 → 本轮已修（§B）** |

**A-3 S0-3 事实修正（#29 输入裁决）**：

- **风险 (c) 已失效**：CLI `eval_module` 生产路径已全部走 in-realm 变体
  （`bao_runtime/runtime.rs:88 eval_module`→`eval_module_in_realm`、`:149
  eval_module_with_control`、`:259 run_test_file`→`in_realm_then`）；fresh-realm 变体
  （`ModuleLoader::eval_module`）仅 bao_engine 测试调用（每测试自建 JsContext，无
  churn）。生产 zone churn (c) 已被 2026-08-14 realm-per-context 根治，账本 S0-3 (c)
  作废。
- **风险 (b) 非缺陷**：跨 registered domain 导航=换 ScriptThread，旧 node realm zone
  随旧线程 cx **即刻**销毁（无堆积）；同域导航 `node_realm_belongs_to_current_context`
  判定通过→Node Realm 幸存（runtime_bridge.rs:304 注释）——servo-owned 生命周期，
  bao 层无需显式 zone 回收。
- **风险 (d)(e) 维持原判**（S0-4 裁决不变）。

#### B. 缺陷类根治：node_vm context 注册表未 root（R2 同类横扫命中）

**缺陷**（与 R2 realworld opt-profile SIGSEVP / `trace_node_realm_roots` BCE 同类，
`opt-only-sigsegv-gc-rooting` 家族）：`VM_CONTEXT_MAP`（原 node_vm.rs:53）把 sandbox
realm 的 global 存为 thread-local Vec 裸指针——malloc 侧内存对 SM tracer 不可见；
realm 在自有 `NewCompartmentAndZone` zone 无其他引用者，createContext 返回后首次
major GC 可整 zone 扫掉，下次 `runInContext` 的 `AutoRealm` 解引用已释放 realm
（UAF/SIGSEGV；dev 档靠 GC 时序运气存活）。**已核实机制**：`Heap` 的
`ValuePostWriteBarrier`（Barrier.h:372 `ValuePostWriteBarrier`）只在值位于 nursery 时
入 store buffer，不构成 major GC 持续 trace——malloc 侧 GC 引用必须 embedder 自证。
镜像缺陷：key 为永不删除的裸地址（dead sandbox 条目残留→地址复用 false-positive）；
`collect_sandbox_properties` 的 `Box<Heap<Value>>` 同窗裸奔（getter 产物仅我们持有的
值在 collect→define 窗口可被扫）。

**修复**（gc_store 范式：JS 值挂属性让 GC 自然 trace）：

- `sandbox[Symbol.for("bao.vm.context.global")] = <sandbox global>`——registered
  symbol 属性（runtime registry 钉住 symbol 免 rooting，同
  bun_inspect_api `nodejs.util.inspect.custom` 先例；READONLY|PERMANENT 非枚举）。
  **属性即 GC 边**：realm 存活期 ≡ sandbox 存活期（Node vm 语义），无 tracer 注册、
  无永久 pin。
- 查询全走边：`is_context_registered`/`get_context_global` 读边 + `UncheckedUnwrap`
  解 CCW——永不返回缓存地址（指针不可能 stale），fresh 对象不可能带边（杀 ABA）；
  `get_context_baseline` 先验证边再查 Vec（死条目不可达）。
- `collect_sandbox_properties`：`Heap::boxed` → `RootedTraceableBox::from_box`
  （RootedTraceableSet 全 GC 可见，Vec drop 自动 unroot）。
- 降级语义：边定义失败（frozen sandbox / Proxy trap 拒绝）→ 后续查询边缺失 →
  重创建 realm，绝不 stale 解引用（fail-safe）。

**测试**（`bao_runtime/tests/suite/node_vm_gc_rooting_tests.rs`，2 用例）：createContext
→ 强制 `JS_GC(API)` 全量 GC ×2 → realm 存活 + **identity 断言**（GC 前 realm 内
`globalThis.marker` GC 后可读——swept/re-created realm 必答不出）+ write-through 双向
+ 64 fresh 对象 isContext 全 false + drop 引用后新 context 健康（无 pin 崩溃）+ 4 轮
8-context churn 交错 GC。**RED→GREEN 证明**：注释掉 attach_context_edge 一行复跑 →
2 用例全红（"sandbox is not a contextified object"）；恢复 → 全绿（测试有判别力，
边是承重的）。

#### C. 遗留 findings（记录不动，后续波候选）

1. `runtime_bridge.rs:163 NODE_REALM_TRACER_CX` TLS dedupe 的 cx 地址 ABA：同线程第二
   cx 复用同地址时跳过 `JS_AddExtraGCRootsTracer` 重注册 → node realm 裸奔（R2 类
   残余）。生产不可达（ScriptThread 一生一 cx）；测试路径（for_test 重建 cx）可构造。
   修复候选：注册时对照 `Runtime::get()` 或放弃 dedupe（重复注册幂等无害）。
2. `bun_api.rs:7861 element_objs: Vec<*mut JSObject>`：`JS_GetElement` 可跑 getter→GC，
   循环内早先元素未 root（gc_store.rs:135 dangling-nursery 合同的 frame 级违例；
   `Bun.concatArrayBuffers` 非普通数组入参可触发）。修复候选：逐元素 rooted 收集。
3. node_vm `CodeGenerationFlags` 存 map 后无读取方（创建时已应用，stored-but-unread）。
4. soak 量化（intentional leak 触发率/100+ page churn RSS）依赖 #19，未开始。

**验证**（波末一次测）：`cargo nt -p bao_engine` **373/373**；`cargo nt -p bun_runtime
--no-fail-fast` **1211 passed / 1 pre-existing skipped**（1209 基线+2 新；首轮 1 红
`fs_watch_file_stat_polling_fires` 隔离复跑 7/7 绿=负载时序 flake，非本改动面——
fswatch 面有专门在途 agent，归属其波次）；scoped vm 全谱 11/11。

**BCE 横扫结论**：同类（malloc 侧 GC 引用无 tracer 可见性）全仓横扫已完成——A-1/A-2
清单穷尽全部 GC 指针持有形态（注册表/根/guard/tracer/FFI userdata）；命中实例=vm
注册表（已修）+ findings 1/2（已记账待后续波）；gc_store/PersistentGlobal/
RawValueRootGuard/tracer 为正例基线。

**回滚点**：单 commit revert（node_vm.rs + suite 测试文件 + main.rs 两行 + 账本），
无数据/接口迁移。

**下一唯一动作**：S2 续——finding 2 落地（`bun_api.rs` concatArrayBuffers 元素收集
rooting，frame 级 dangling-nursery 类，单函数+单测小 slice）；若用户裁决 RED-1 P-A
则插队其 vendor patch 波；findings 1（tracer dedupe ABA）与 soak 量化随后。

### 2026-09-10 / S2-续——#29 finding ② 根治：bun_api concatArrayBuffers 元素收集 frame 级 rooting（代码+测试）

**基线**：bao master `642da220` + e4675351（S2 同日续）；mozjs 不变。RED-1 零触碰。

**缺陷**（S2 遗留 finding ②，`gc_store.rs` dangling-nursery 合同的 frame 级违例，
e4675351/9f5f4692 同族）：`bun_concat_array_buffers` 第一遍 sweep 逐元素
`JS_GetElement` 触发用户 getter 后把元素对象存进 `Vec<*mut JSObject>` 裸指针 Vec
——malloc 侧内存对 SM tracer 不可见。getter 可分配→GC：早先元素（getter 返回、
无其他引用者的 fresh buffer，如 `{length:8, get 0(){...}}` 形态）被整块扫掉或
nursery 搬迁，第二遍 sweep 与 memcpy 继续解引用悬垂对象/已释放 backing store。
**未证崩（S2 记账）→ 本轮实证为确定性 SIGSEGV**（见 RED 证明）。

**修复**（servo structuredclone reader 同形先例 `RootedVec<Box<Heap<*mut JSObject>>>`）：

- `RootableVec::new_unrooted()` + `RootedVec::new(&mut ...)`：整个收集 Vec 注册进
  `RootedTraceableSet`（`Runtime::new` 的 `JS_AddExtraGCRootsTracer(trace_traceables)`
  全 context GC 可见——rust.rs:393 接线已核）；每元素 `Heap::boxed(obj)`——Box 钉死
  Heap 槽位地址（post-write-barrier 槽地址不随 Vec 扩容漂移），`Heap<*mut JSObject>`
  的 `HeapObjectWriteBarriers` + `CallObjectTracer` 双保险。
- 消费点改 `obj.get()`（第二遍 sweep 的 `is_array_buffer`/`ab_bytes`/`ta_bytes` 与
  copy pass 的 null 判定）；rooted guard 活到 frame 尾——backing store 存活覆盖
  memcpy 全窗。
- null 槽（非对象元素）为 `Heap::boxed(null)`，barrier no-op，语义不变。

**RED→GREEN 证明**（先例纪律，测试有判别力、root 是承重的）：临时还原裸 Vec 复跑
→ **2 用例全 SIGSEGV**（`test_concat_getter_forced_full_gc`：每个 getter 内
`Bun.gc()`（→`JS_GC` API reason）确定性 full GC 恰落在 JS_GetElement 之间；
`test_concat_getter_allocation_storm`：大数组 128×64KiB + 每 getter 4 MiB 分配
churn 触发自然 major GC——两路径都崩，缺陷是 live crasher 非 theoretical）；
恢复修复 → 2/2 绿。测试 = `bun_concat_gc_rooting_tests.rs`（getter 返回值全树
零 JS 引用者，唯一引用=收集器；逐字节 pattern 断言非仅 length 抽查）。

**横扫结论**（API 族面 = bun_api.rs 全文件 + 同族收集形态）：`Vec<*mut JSObject>` /
`Vec<JSVal>` / `Vec<Value>` frame 级收集全仓（含 node_vm 复核）**仅此一处**（grep
零漏报已过阳性对照——目标行自身命中）；残留=0。

**findings ③ 处置（顺带核明，不改码——node_vm 不在本 slice 代码面）**：
`VM_CONTEXT_MAP` 元组 `.1`（`CodeGenerationFlags`）确认 stored-but-unread——限制在
createContext 时经 `apply_code_generation_restrictions` 一次性落地（eval/Function
替换 + WebAssembly 删除，行为已由 vm_codegen 测试覆盖 11/11）；纯死数据无 GC 指针
无运行时危害。处置：记录为 bounded dead data，下轮触碰 node_vm 的波顺带删字段。

**验证**（波末一次测）：`cargo nt -p bao_engine` **373/373**；`cargo nt -p
bun_runtime --no-fail-fast` **1213 passed / 1 pre-existing skipped**（1211 基线+2 新，
零红零新增 flake）；scoped vm 全谱 `test(vm)` **11/11**。本轮无 fswatch 面触碰。

**回滚点**：单 commit revert（bun_api.rs 收集段 + suite 测试文件 + main.rs 一行 +
账本），无 API/数据/接口变更。

**下一唯一动作**：S2 遗留 findings 1（`runtime_bridge.rs` NODE_REALM_TRACER_CX TLS
dedupe cx 地址 ABA——修复候选=注册时对照 `Runtime::get()` 或放弃 dedupe）为下一切片；
soak 量化依赖 #19（未开始）；RED-1 等用户裁决（P-A 则插队 vendor patch 波）。

### 2026-09-10 / S2-续2——#29 finding ① 根治：NODE_REALM_TRACER_CX dedupe cx 地址 ABA（代码+测试）

**基线**：bao master `642da220` + S2/S2-续（同日）；mozjs 不变。RED-1 零触碰。

**缺陷**（S2 遗留 finding ①，60fb645c NodeRealmEntry 身份同族）：`create_node_realm_native`
的 `JS_AddExtraGCRootsTracer` 注册以 thread-local `NODE_REALM_TRACER_CX`（裸 cx 地址
Cell）dedupe——cache 存活期超过它采样的 runtime：同线程旧 cx 死后 malloc 把同地址交给
新 cx（S0/S1 期 60fb645c gdb 已实证同址复用）→ cache 命中 → 跳过重注册 → 新 cx 的全部
node realm 无 tracer（R2 类残余，realm 可被 major GC 整 zone 扫掉）。生产不可达
（ScriptThread 一生一 cx 且新 ScriptThread=新线程，thread-local 随线程消亡）；for_test
同线程重建 cx 可构造。**连带发现并同轮根治**：tracer 过滤器 `trace_node_realm_roots`
只比 `owner_cx`（单因子）——注册表是进程级 DashMap，跨线程地址碰撞可让本线程 tracer
追踪外线程（可能已死的）同址条目（GC trace 期 UAF 面）；60fb645c 给 NodeRealmEntry 加
的 `owner_thread` 因子在 tracer 侧一直未被消费。

**修复**（60fb645c 双因子形态 REUSE + 注册权威源迁移）：

- **注册去 embedder-cache 化**：dedupe 状态从 embedder 侧地址 cache（跨 runtime 残留、
  可 ABA）迁到 **live runtime 自己的 blackRootTracers 列表**——
  `JS_RemoveExtraGCRootsTracer`（首个 (op,data) 匹配删除、缺席 no-op，GC.cpp:1615/1631
  已核）+ `JS_AddExtraGCRootsTracer` 成对无条件调用：每 runtime 恰好一条，列表随
  runtime 原子死亡。`NODE_REALM_TRACER_CX` thread-local 整体删除——漏装类**按构造不可
  达**（无任何跨 runtime 缓存状态参与注册决策），无需时序论证。
- **tracer 过滤器双因子化**（60fb645c 形态落地）：新增单一身份谓词
  `node_realm_entry_matches(entry, thread, cx)`（ThreadId 主因子进程内永不回收 + cx
  同线程次因子），`node_realm_belongs_to_current_context` / tracer 过滤器 / 注册期
  scrub 三消费方统一走同一谓词（identity 单真源）。跨线程同址碰撞从此不匹配。
- **生命周期对齐（scrub）**：注册时 `scrub_stale_cx_entries(thread, current_cx)` 丢弃
  本线程 `owner_cx != current_cx` 的条目（BCE-20260621-001 单活 cx/线程 ⇒ 同线程异 cx
  条目必属死代 cx）；外线程条目永不动（其自身 tracer 持根）。残留角落=同线程**同址**
  死条目，系 60fb645c 地址身份无死亡回调的固有边界，生产不可达（新 ScriptThread=新
  线程），已在谓词文档与 tracer SAFETY 注释记录。

**RED→GREEN 证明**：临时回退 tracer 为单因子（`entry.owner_cx != owner_cx`）→
`tracer_filter_is_two_factor` 红；临时回退 cache+skip dedupe 形态 →
`tracer_registration_is_runtime_authoritative` 红（needle 运行时拼接防断言自匹配）；
恢复 → 全绿。测试有判别力、双因子与去 cache 是承重的。

**测试**（runtime_bridge.rs tests，4 新用例）：①注册权威性（全文件无
`static NODE_REALM_TRACER_CX` 残留 + remove 先于 add + 注册段禁条件 skip + scrub 在场）；
②tracer 双因子（共享谓词 + thread 因子在场）；③身份谓词行为（spawn 真实线程取
ThreadId：全匹配唯一真，同线程异 cx / 跨线程同址 / 双异皆假——跨线程同址即旧单因子
ABA 盲区）；④scrub keep-rule（外线程同址条目保留=不误拆他线程根、本线程活 cx 保留、
本线程死代 cx 丢弃）。真实同址双 cx 的 live 负向构造不可确定性构造（malloc 复用不受
控），由结构论证承载——与 60fb645c 先例同形。

**验证**（波末一次测）：`cargo nt -p bao-browser --lib` **583/583**（含 4 新）；
`cargo nt -p bao_engine` **373/373**；`cargo nt -p bun_runtime --no-fail-fast`
**1213 passed / 1 pre-existing skipped**（基线零红）。

**回滚点**：单 commit revert（runtime_bridge.rs + 账本），无 API/数据/接口变更，无
vendor 触碰。

**下一唯一动作**：#29 代码面 findings 清零（①②已闭；③为 bounded dead data，随下轮
触碰 node_vm 的波顺带删字段）；soak 量化依赖 #19（未开始）；RED-1 等用户裁决（P-A 则
插队其 vendor patch 波）。

### 2026-09-10 / S2-续3——#29 findings ③ 清除：VM_CONTEXT_MAP flags stored-but-unread 死数据删除（代码面清零）

**基线**：bao master `642da220` + S2/S2-续/S2-续2（同日）；mozjs 不变。RED-1 零触碰。

**缺陷**（S2-续 findings ③，本波即「下轮触碰 node_vm 的波」——e4675351/1bf2d5ee
已动 node_vm/bun_api 域）：`node_vm.rs` 的 `VM_CONTEXT_MAP` 元组 `.1`
（`CodeGenerationFlags`）写入（`register_context` push）后全仓零读取——唯一 map
消费方 `get_context_baseline` 只读 `.2` baseline（`(_, _, ref b)`）；限制在
createContext 时经 `apply_code_generation_restrictions` 一次性落地（eval/Function
替换 + WebAssembly 删除，行为由 vm_codegen 测试承载）。纯死数据：无 GC 指针、无
运行时危害（e74 判定），仅 bounded 存储噪声。

**修复**（最小差分，字段级删除）：

- `VM_CONTEXT_MAP` 元组 `(*mut JSObject, CodeGenerationFlags, Rc<Vec<String>>)` →
  `(*mut JSObject, Rc<Vec<String>>)`；
- `register_context` 去 `flags` 参数（唯一调用点 vm_create_context 同步）；
- `get_context_baseline` find/map 模式随元组收窄（`&&(s, _)` / `&(_, ref b)`）；
- **`CodeGenerationFlags` struct 本体保留**——创建时活性消费
  （`parse_code_generation_options` → `apply_code_generation_restrictions`），非死代码；
- 注释同步：registry BCE 注释记「policy 不入 map——创建时一次性落地，再查即死数据」，
  防未来回填。

**读取者实证**（删除前 grep，阳性对照过）：`VM_CONTEXT_MAP` 全仓仅 3 处
（定义/push/get_context_baseline，全在 node_vm.rs）；`strings_allowed|wasm_allowed`
域外唯一命中为测试函数名（非 map 读取者）；`register_context` 非 pub、唯一调用点
vm_create_context。删除后 `cargo check -p bun_runtime` 零新警告
（node_vm 仅余 4 个 pre-existing `unsafe_jsstr_to_string` deprecation）。

**验证**（波末一次测）：`cargo nt -p bao_engine` **373/373**；`cargo nt -p
bun_runtime --no-fail-fast` **1213 passed / 1 pre-existing skipped**（基线零红）；
scoped `test(vm)` **11/11**。本轮无 vendor 触碰。

**回滚点**：单 commit revert（node_vm.rs + 账本），无 API/数据/接口变更。

**下一唯一动作**：#29 代码面 findings 清零达成（①②③全闭）→ soak 量化前置 =
启动 #19（未开始）；RED-1 等用户裁决（P-A 则插队其 vendor patch 波）。

### 2026-09-10 / #28——RealmPolicy 核查轮：locale/timezone/JIT/shared-memory 引擎原生策略 inventory + 裁决提案（+2 实证测试）

**基线**：bao master `642da220`（并行 slice，零触碰 S2/S2-续* 文件域）；mozjs 不变
（jsapi.rs 最新 d252b443，25342 行）。RED-1/soak 主线零触碰。

#### 28-1 绑定可达面（行号=jsapi.rs；bindgen 输入=`mozjs-sys/src/jsapi.cpp` include 聚合 :9-42）

| 维度 | API | C++ 锚 | Rust 可达性 |
|---|---|---|---|
| locale（runtime 级） | `JS_SetDefaultLocale` / `JS_GetDefaultLocale` / `JS_ResetDefaultLocale` | LocaleSensitive.h:34/:45，impl jsapi.cpp:4058/4063/4072 | **缺——但非 C++ 墙**：精确缺口=bindgen shim `src/jsapi.cpp` include 清单缺 `js/LocaleSensitive.h`；allowlist `JS_.*`（build.rs:1016-1021）已匹配；`UniqueChars` 返回类型 jsapi.rs 已解析（13 处引用）→ 补 1 行 include 即 bindgen 再生成可达。代价=jsapi.rs 变更→全 workspace 下游重编，归 locale 接线波首动作，非本 slice |
| locale（per-realm） | `RealmCreationOptions::setLocaleCopyZ` | RealmOptions.h:215，impl jsapi.cpp:1783 | **C++ only**：raw 字段 `locale_: RefPtr<LocaleString>`（jsapi.rs:10854）Rust 不可安全构造（RefCounted 无绑定） |
| locale（默认派生） | `JSRuntime::getDefaultLocale` | Runtime.cpp:527-560 | 未 override 时 ICU `Locale::GetDefaultLocale()` ← 进程环境（LANG/LC_*）→ **宿主身份泄漏面**（现状态） |
| timezone（per-realm） | `forceUTC_` 字段 | RealmOptions.h:206-210/250；DateTime.cpp:486-490 | **可达（本轮测试实证）**：pub 字段 jsapi.rs:10860，creation-time only（无 post-creation setter）；语义=Firefox RFP 同形——真 IANA 区 Atlantic/Reykjavik（UTC+0、真实 DST 史），非裸 +0000 |
| timezone（重查询） | `JS::ResetTimeZone` | Date.h:56，绑定 jsapi.rs:12470 | 可达（重读系统时区） |
| timezone（任意时区名） | ——（不存在） | DateTime.cpp:486 硬编码 Reykjavik | **引擎不暴露**：任意 tz 只有 TZ env（进程级、宿主态、启动前）或 mozjs vendor patch 两路；对照 Chrome CDP `Emulation.setTimezoneOverride`（V8 ICU per-isolate override）是 SM 真实能力差 |
| 时间精度 | `JS::SetTimeResolutionUsec` / `SetReduceMicrosecondTimePrecisionCallback` / `Get...Callback` | Date.h:204/:215，绑定 jsapi.rs:12569/12558 | **可达（本轮实证前者）**：`Date.now()`/`getTime` 钳制（jsdate.cpp:2096-2135 NowAsMillis）；per-realm 门=`RealmBehaviors.clampAndJitterTime_`（C++ 默认 **true**，RealmOptions.h:305；pub 字段 jsapi.rs RealmBehaviors） |
| JIT | `ContextOptionsRef` + bitfield setters（`set_disableIon_`/`set_wasm_`/`set_wasmBaseline_`/`set_wasmIon_`/`set_fuzzing_` 等）；`JS::DisableJitBackend`；per-realm `preserveJitCode_` | ContextOptions.h；绑定 jsapi.rs:12450/12672；字段 :10859（默认 false，RealmOptions.h:242） | 可达；另有 build-time cargo feature `jit`（默认开；makefile.cargo 无 feature 时 `--disable-jit`） |
| SAB/Atomics | `sharedMemoryAndAtomics_` / `defineSharedArrayBufferConstructor_` / `JS::SetWaitCallback` | 字段 jsapi.rs:10861-10862；SetWaitCallback :11271 | 可达（node_realm_options 已用前者；SetWaitCallback 零使用——不装时 Atomics.wait 仍走 condvar 可等（AtomicsObject.cpp:1670-1690），但阻塞期 embedder 无法泵 job/timer） |

#### 28-2 Bao/Servo 现状锚

- **locale**：bao 全层零处理——bao_stealth 无 locale 模块（模块清单核实）、
  `StealthProfile` 无 locale/language 字段、无 LC/TZ env 读取；servo 全树零调用
  SetDefaultLocale → 页面/Node realm 的 `Intl.*` locale 身份=宿主环境派生（泄漏面，#16 feed）。
- **timezone**：同上零处理——页面 `Intl.DateTimeFormat().resolvedOptions().timeZone`
  与 Date 偏移=宿主 TZ（本机 +0800）直接泄漏（#16 gap：stealth 身份维度未覆盖 tz/locale）。
- **JIT**：servo `script_runtime.rs:911-943` ContextOptionsRef 由 servo prefs 驱动
  （`js_ion_enabled`/`js_baseline_*`/`js_wasm_*` 默认全开）——**页面/worker JIT policy
  owner=servo prefs 机制（上游形态，bao 继承不动）**；bao 层零触碰；runtime realm 默认 JIT on。
- **SAB**：node 语义 realm true（`node_realm_options`，S0-3 已录）/servo 页面 realm false
  （未跨站隔离默认）——双身份拆分为设计既定，无第二 owner。
- **Atomics.wait liveness**：SetWaitCallback 未装 → CLI 主线程 `Atomics.wait` 阻塞期
  job/timer/pump 全停 → feed #24/#25（liveness 非 policy）。

#### 28-3 裁决提案（每维度三选一；页面可见身份=产品语义→立法提案，本轮禁实施）

| 维度 | 裁决提案 | 依据 |
|---|---|---|
| locale | **引擎原生下沉（提案）**——runtime 级 `JS_SetDefaultLocale`（1 行 include 解锁绑定），per-realm setLocaleCopyZ 不采纳（C++ only） | stealth 一致性：JS hook 层模拟需 hook 全 Intl 构造器+resolvedOptions+collation 排序，引擎原生一处覆盖全 realm（含 servo 页面/worker/SW）；生命周期与 realm 一致免 hook。**页面可见 locale 身份=产品语义（StealthProfile 增 locale 字段=REQ-STL 范围）→待用户立法**；绑定解锁落地归 locale 接线波（共享树内多 agent 在途，jsapi.rs 再生成=全 workspace 重编，非并行 slice 动作） |
| timezone | **引擎原生下沉（UTC 类）+ 任意 tz 引擎层显式不用** | forceUTC 已实测可达（per-realm creation-time，RFP 同形、真 IANA 区非裸 +0000）；任意时区名引擎不暴露——若未来用户裁决要 Chrome 级 tz 仿真=mozjs vendor patch 立法提案（DateTime.cpp timeZoneOverride 硬编码改可注入）。页面可见 tz 身份=产品语义→待立法；现状宿主 tz 泄漏已记 #16 gap |
| 时间精度 | **引擎原生下沉（提案）** | `SetTimeResolutionUsec`/callback 全已绑定，Firefox RFP 机制原样；**覆盖面=Date.\*（NowAsMillis）——performance.now 是 DOM/servo 层**，#16 接线须两层齐动，否则 Date/perf 精度不一致本身是指纹信号；产品语义→待立法 |
| JIT | **维持现状** | 页面/worker=servo prefs owner（上游机制）；bao runtime realm 默认 on；`preserveJitCode_` 归 #26/#29（churn/benchmark 实测前不动，S3 phase gate 原文）；JIT 开关非 fingerprint 面，无 stealth 收益，无产品语义 |
| SAB/Atomics | **维持现状** | node on / web off 双身份拆分正确（页面 SAB 缺席=Chrome 未隔离页默认，指纹一致）；`SetWaitCallback` liveness 接线 feed #24/#25；`defineSharedArrayBufferConstructor_` 未用不动 |

#### 28-4 本轮落地（无争议项=2 个引擎实证测试；零产品语义、零 vendor 变更）

- `src/bao_engine/tests/suite/realm_policy_tests.rs`（`main.rs` 挂载，
  `@trace TEST-ENG-001-REALMPOLICY [req:REQ-ENG-001] [level:integration]`）：
  ① forceUTC realm 创建 + Date offset 双时点（1 月/7 月瞬时）===0 实证（宿主 +0800 下
  为有效证明，7 月时点同时排除宿主 DST 泄漏）；② `SetTimeResolutionUsec` 1s 钳制 +
  恢复 roundtrip（**断言前恢复**——进程级 static，防同进程后续测试被污染）。
- locale 绑定解锁（include）本轮**未落**：jsapi.rs 再生成→全 workspace 下游重编，
  共享工作树多 agent 在途，归 locale 接线波（#16 立法通过后）首动作。
- **验证**（波末一次测）：`cargo nt -p bao_engine` **374 run / 374 passed / 0 failed**
  （基线 373 + 新 1，零红）。无 vendor 触碰。

**stop 条款修正**：locale/tz 的 Rust 绑定缺失=**精确缺口已录**（缺 1 行 include 而非
C++ 墙），裁决提案未降级为纯 C++ 面评估；三维页面可见身份（locale/tz/时间精度）均属
产品语义→立法提案待用户，本轮按 scope 纪律零实施。

**下一唯一动作**：不变——#29 soak 量化前置 #19 启动；RED-1 待用户裁决（本 slice 为
并行核查，不触碰主线）。

### 2026-09-11 / #28——裁决消费：宿主 locale/timezone/时间精度身份泄漏引擎原生根治（三维页面可见身份，用户裁决 2026-09-10 直做）

**基线**：master `ec4a0e5a` 消费起跳（并行 slice 共存：e92 timers / e93 net 域文件零
触碰）；jsapi.rs 再生成=全 workspace 下游重编，28-1 预告的「接线波首动作」兑现（无
make 增量 bug 触发——SM 源零改动，仅 bindgen shim + glue 重编）。

#### 消费实现（三维 → 引擎原生 sink）

| 维度 | 裁决 | 落地 |
|---|---|---|
| locale | 引擎原生（runtime 级 JS_SetDefaultLocale；per-realm setLocaleCopyZ C++-only 不采纳） | ① vendor/mozjs shim 补 `js/LocaleSensitive.h` include → jsapi.rs 再生成（Set/Get/Reset :19014/:19022/:19027）② `bao_engine::realm_policy::set_default_locale/reset_default_locale`（SM 每 JSContext 私有 runtime → 实际粒度=per-ScriptThread，同线程多页 last-write-wins——engine 级粒度如实入档）③ 接线=runtime_bridge install_all_native（script thread 自有 cx 上调用，覆盖 page+Node+后续 Worker/SW 全 realm，零 JS hook 可检测） |
| timezone | 引擎原生 forceUTC（RFP Reykjavik 语义）；任意 tz 引擎层显式不用（Chrome 级 tz 仿真=未来 mozjs vendor patch 立法案，未采） | 双落点读同一组进程级创建期旗标：servo `create_global_object`（script_bindings/interface.rs——Window/DedicatedWorker/SharedWorker/SW 全 DOM realm 单咽喉）+ `bun_sm::node_realm_options`（Node 语义 realm，NODE_FORCE_UTC）；bao_browser 于 page_pool::create_page 在 pipeline realm 创建**前**武装（forceUTC 无 post-creation setter，晚于 wait_for_pipeline_ready 即静默漏页） |
| 时间精度 | 引擎原生 + 两层齐动（两层不一致本身是指纹信号） | Date 层=`JS::SetTimeResolutionUsec`（process 级 static；clampAndJitterTime_ C++ 默认 true 两构造器均保持）。performance 层**实测双钳制点**（live 首跑发现：页面 `performance` 对象是 bao_runtime `install_performance` 装的 epoch 基原生对象——servo DOM Performance 在 install 时被整体替换，页面 JS 观测面不是 servo Performance）：① servo `ToDOMHighResTimeStamp for Duration`（performance.rs——servo 自有换算面/timeOrigin/entries 单咽喉；上游 10µs 网格=servo 特有 tell，unset 时字节等价保留）② bun `performance_now` 原生按 engine_props `timing_precision_us()` 网格化（页面实际观测面，floor 语义同引擎钳制）。全部同源 `StealthProfile::timing.precision_us`，JS hook 层（build_timing_js）亦同源=一网格 |

**StealthProfile 扩字段**（纯新增，默认=Chrome 桌面常见形态、宿主派生零参与）：
`locale: LocaleConfig`（en-US）+ `timezone: TimezoneConfig`（force_utc=true）；
时间精度复用既有 `timing.precision_us`（100µs，防第二真源）。三字段 per-page 可覆盖
（clone+mutate，单测锁）。stealth-free 页（profile=None）显式复位三 sink
（reset_default_locale/精度 0/forceUTC false）——早先 stealth 页不得把策略泄漏给后建
stealth-free 页（与既有 TLS/canvas 全局复位语义同构）。

**精确缺口记录**（stop 条款预置面）：performance DOM 层钳制点=servo 有此面
（ToDOMHighResTimeStamp），已接——零缺口发生。

**验证**（波末一次测）：

- `cargo nt -p bao_engine`：**376/376 passed**（基线 374+并行 slice 增量，零红；
  realm_policy_tests 扩 2 维：forceUTC×100ms 任意精度组合往返 + locale sink 双 tag
  de-DE→ja-JP 主机无关证明）。
- `cargo nt -p bao_stealth`：**1689/1689 passed**。
- `cargo nt -p bao-browser -E 'test(stealth)'`（stealth 全族非网络面）：**160/160
  passed**（live 门控项按既有纪律自跳过；live 门控面由上述
  stealth_identity_locale_tz_tests 单独全绿覆盖）。
- live RED→GREEN（`BAO_TEST_NETWORK=1 xvfb-run`，bao-browser
  `stealth_identity_locale_tz_tests`，宿主 Asia/Shanghai+0800 / LANG=en_US.UTF-8）：
  ① profile 页（chrome_default）UTC offset 0（1 月+7 月瞬时；宿主 +0800 下修复前
  live 泄漏面=-480）+ Intl 默认 locale=en-US + navigator.language=en-US（hook 层与
  引擎层一致）+ 100µs 网格；② per-page 覆盖（en-GB/1s）：Intl 跟随 + Date.now
  引擎钳制 %1000==0 + performance.now 同 1s 网格（两层一致断言）；③ stealth-free
  页宿主态恢复（locale/offset 宿主值打印=泄漏态存证）。**终态 4/4 PASS**。
  精度维的 RED 证据即来自首跑：performance.now 返回裸值 `1789057624036.04`
  （epoch 基 40µs 尾，100µs profile 下不在网格）——由此发现页面 `performance`
  观测面是 bao_runtime 原生对象（servo DOM Performance 被 install_performance
  整体替换），钳制点补齐到 bun `performance_now` 后 GREEN（该发现已回写上表
  时间精度行）。
- locale 维 live 证据说明（如实）：本机宿主 LANG=en_US.UTF-8 → Intl 宿主泄漏面在本机
  不可与 en-US 目标态 live 区分；locale sink 机制证据=引擎级双 tag 证明（宿主无关），
  tz 泄漏面（+0800）为本机 live 主证据。

**mozjs/servo 清单旁注**（清单本体归主会话）：CLAUDE.md mozjs fork patch 清单旁注=
bindgen shim include 行（jsapi.cpp 内带 BAO PATCH 注释块）；servo 定制文件清单新增
4 文件（script_bindings/interface.rs、script/lib.rs、script/dom/performance/
performance.rs、servo/lib.rs——各自带 SM-EVOLUTION #28 BAO PATCH 注释）。

**回滚点**：2 commit revert（① 引擎机制层 9c92fd04 ② 接线+profile+vendor servo）；
无 API 移除、无数据迁移；stealth-free 路径字节等价（精度 0=上游 10µs 网格、locale
reset=OS 派生、forceUTC false=上游）。

**下一唯一动作**：#28 CLOSED（五维全裁定：locale/tz/精度已消费，JIT/SAB 维持现状=
终态裁定）；#16 Stealth 身份一致性缺陷三维闭。

### 2026-09-10 / #30——UpstreamAudit 自动化 v1：capability inventory + drift detector + Bao 使用映射（纯工具轮，零产品代码改动）

**基线**：bao master `a3220511`（生成时共享树 dirty=true，已如实入 inventory 元数据；
本 slice 仅新增 scripts/ 与 .claude/sm-audit/ 产物，不改 API 面）；mozjs 不变
（bao-mozjs-sys 140.14.0-0，jsapi.rs md5 `ffaa6002`，多 build out 副本 md5 一致）。

**交付（scripts/sm-audit/ 三脚本 + 共享模块 + README，纯 Python3 标准库）**：

1. `extract_inventory.py`——源 A=bindgen jsapi.rs（bao-mozjs-sys build out 自动发现，
   env `BAO_SM_AUDIT_JSAPI` 可覆写）全部 extern "C" pub fn（symbol/rust_path/mangled/
   归一化签名 hash/surface/category_guess/stability_guess）；源 B=mozjs 安全层绑定分类
   （safe-wrapper(wrappers2) 667 > wrappers1-deprecated > mozjs-layer-reference > raw-only）。
   fail-closed 自检：阳性对照（JS_NewContext/EnterRealm@root::JS/
   ReportOutOfMemory@root::js）+ extern 块计数（fn+static==blocks）；失败退出码 2，
   stop 条件降级路径已文档化（rust.rs 单源）。
2. `usage_map.py`——src/**/*.rs 全树（1491 文件，含 bun_* vendored 与 tests，S0-A 同口径）
   单遍 tokenize 词边界计数 → **native-used 154 / unused-pending-verdict 923** 两分类
   初判 + confidence 三级（high/medium/low，通用名误报防呆：low 仅 1）。
3. `drift.py`——两版 inventory 三分类（added/removed/renamed，改名=同命名空间名相似度
   ≥0.75 配对）+ signature_changed + stability_flip（experimental↔public，实验头词表
   交叉）+ 域级 rollup（#30 禁令：不裸 diff bindgen）+ 旧版 adoption 自动发现标注
   removed_bao_used（升级波阻断候选）。
4. seed 产物（.claude/sm-audit/）：`inventory-2026-09-10-a3220511.json`（1077 symbols：
   public 860/friend 154/glue 46/internal 17；experimental 56）+
   `adoption-*.json`/`adoption-report-*.md` + `drift-selfcheck-baseline.json`（零漂移
   证明）+ `BASELINE` 基线指针（下次 mozjs 前移的 diff 锚）。

**验证**（波末一次测=工具自证，无 Rust 编译面）：

- extractor：667/667 wrapper 全匹配（0 unmatched）；1189 extern 块=1077 fn+112 static 记平。
- usage_map：6 symbol 抽样计数与独立 `command grep -rw` 全等（24/8/101/0/863/21）；
  语义对照账本既有事实——`JS_RequestInterruptCallback` 唯一文件=execution_control.rs
  （#24 闭环）、`CompileGlobalScriptToStencil`=0（#26 纯接入）、`JS_NewGlobalObject`=24。
- drift：同基线自比=全零；合成变异 4/4 检出（added/removed/renamed/signature_changed
  各 1）+ 一致性守卫触发（同 commit+md5 却有 diff → 退出码 2）。

**与人工账本分工**（#30 契约）：inventory/adoption 是 symbol 级 FACTS；capability 裁决
（used-native/wrapped/emulated/missing/deliberately-unused）仍由
`.claude/sm-capability-ledger.json` 人工层持有；unused-pending-verdict ≠ 弃用。

**daily-ops 集成点**（建议已录 `scripts/sm-audit/README.md`，不改 daily-ops 本体）：
mozjs 升级波编译绿后 patch replay 前跑 extract→drift→消费三步（阻断候选/
patch supersession/adoption policy 分类/§8 追加/BASELINE 前移）。

**下一唯一动作**：升级波首跑待下次 mozjs 前移（#22 合同消费点）；#30 余项（patch
supersession 自动对照、cap ledger last_audited 联动刷新）归下次 census 轮。

### 2026-09-10 / #27——Debugger/observability 核查轮：SM debugger/script/frame/object/memory 原生可观测 inventory + 裁决提案（+1 引擎实证测试）

**方法**：#30 inventory（`inventory-2026-09-10-a3220511.json`）过滤 debugger/memory 族
1077 symbol + bindgen jsapi.rs（`c7c33b2c` build out）绑定级双核实 + mozjs/servo 源码
核实（MemoryMetrics.h / MemoryMetrics.cpp / Stack.h / Debug.h / debugger.js）。

#### 27-1 绑定可达面（四分类）

- **safe-wrapper 已有（直接可调）**：`JS_DefineDebuggerObject`
  （jsapi2_wrappers.in.rs:380；servo `dom/debugger/debuggerglobalscope.rs:136` 同型使用）、
  `ExposeScriptToDebugger`（:226）、`JS_Tracer*` 5 个（:381-385）、`CaptureCurrentStack`
  （:173）、`BuildStackString`（:175）、`JS_StackCapture_FirstSubsumedFrame`（:654）、
  `CollectRuntimeStats`、`JS_SetGCCallback`、`SetGCSliceCallback`、
  `AddGCNurseryCollectionCallback`/`Remove`、`SetWarningReporter`、
  `JS_GetOwnPropertyDescriptor(ById)`/`JS_GetPropertyDescriptor(ById)`、
  `SetAllocationMetadataBuilder`、`JS_GetGCParameter`（:373，已采：
  bun_sm/virtual_machine.rs:112 JSGC_BYTES）。
- **bindgen raw-only（unsafe FFI 可达）**：`GetDebuggeeGlobals`/`IsDebugger`/
  `GetDebuggerMallocSizeOf`/`SetDebuggerMallocSizeOf`/`GetDebuggerObservesWasm`（JS::dbg）、
  `GetAllocationMetadata`（js）、`EnableContextProfilingStack`/
  `SetContextProfilingStack`/`RegisterContextProfilingEventMarker`/
  `SetProfilingThreadCallbacks`/`JS_DefineProfilingFunctions`/
  `GetProfilingCategoryPairInfo`。
- **类型墙（C++ 阻，inventory 0 命中）**：`JS::Debugger` C++ class API（Debug.h 的
  onNewScript/onEnterFrame hook、breakpoint、Debugger.Frame typed 面）——非 bindgen 面；
  `JS::ubi::Census`/UbiNode（heap snapshot 全族）；`ProfilingFrameIterator`（sampling
  profiler 阻断，cdp_handler.rs:275-281 注释仍准确）；`ProfilingStack` 构造。
- **符号可达但类型构造墙**：`CollectRuntimeStats` 符号可达且 **null opv 显式容忍**
  （MemoryMetrics.cpp:380 `if (ObjectPrivateVisitor* opv = ...)` 守卫；opv 纯虚类
  MemoryMetrics.h:910 Rust 不可实现，但传 null 合法）；`RuntimeStats`
  （MemoryMetrics.h:839 非 virtual struct，bindgen 字段全 pub）含 `js::Vector` 成员
  （RealmStatsVector/ZoneStatsVector），Rust 零化构造 UB 风险 → **需 mozjs-sys
  jsglue.cpp ~5 行 C++ glue（new/free RuntimeStats）才能安全采**。
- **对 2026-09-04 ledger 的修正**：「CDP Debugger 复用受 binding 阻塞（emulated 现状）」
  对 **JS-face 不成立**——`JS_DefineDebuggerObject` 一直是 wrapped 的；受阻的只是
  Rust-side typed C++ face（JS::Debugger class）。

#### 27-2 Bao CDP 自模拟面锚定

- **CDP Debugger 域不是 Rust 自模拟**：走 page-realm **JS-level SM Debugger**
  （`DEBUGGER_SETUP` cdp_handler.rs:981-1026：`new Debugger()` + onNewScript/
  onDebuggerStatement + findScripts；事件经 console.log `__BAO_EVT__` 嗅探回传）。
  原生 JS face，但胶水层保真缺口：
  - 断点 hit 的 paused 事件 `callFrames` 恒 `[]`（:1057 setBreakpoint hit 回调）；
  - onDebuggerStatement 帧 location lineNumber/columnNumber 硬编码 0（:1006）；
  - `cmd_debugger_list_frames` lineNumber:0（:1093）；
  - `cmd_debugger_get_environment` 恒 `{environment:{}}`（:1099）——空占位；
  - `cmd_debugger_get_possible_breakpoints` 逐行合成非真实可断点位置（:1117；SM
    `Debugger.Script.getPossibleBreakpoints()` 原生可用未用）；
  - `cmd_debugger_set_breakpoint` 用 `s.offsetLine(line,col)`（:1057）——
    Debugger.Script **无此双参 API**（真 API=`getLineOffsets(line)`/
    `getPossibleBreakpoints`）→ JS 胶水层「未读 SSOT 猜 API」缺陷类；
  - blackbox/unblackbox 静默 no-op 返回 ok（:1133-1145）——silent fake success。
- **CDP Runtime 域 native JS 语义**：page-realm `window.__bao_cdp` registry +
  getOwnPropertyNames/getOwnPropertyDescriptor（cdp_handler.rs:1441-1479），objectId 真
  roundtrip；exceptionDetails 无 stackTrace/line/column（`exceptionId:0`，:703）——
  Error.stack（SavedFrame 后端）可供而未供。
- **Profiler/HeapProfiler/Memory fail-closed 显式错误**（cdp_handler.rs:278-301）无自
  模拟；collectGarbage/forciblyPurgeJSMemory 真（JS_GC）；
  `Memory.prepareForLeakDetection` ok_empty()（protocol.rs:1924）——无声 no-op 小残留。
- **bun_sm::debugger（Rust breakpoint CRUD，cap ledger 标 emulated）全仓零消费者**
  （唯一引用=bao_engine/src/lib.rs:54 re-export）——死代码。
- servo 侧 debugger.js + DebuggerGlobalScope（about:internal/debugger）在 bao 未接
  （不连 servo devtools：`disable_script_debugger=true` bao_browser/src/lib.rs:79/115/170
  + BCE-20260621-002 gate script_thread.rs:3977）。

#### 27-3 hideScriptFromDebugger（BCE-20260622-004）因果链复核

- patch 在位：vendored mozjs rust.rs:607-624
  `CompileOptionsWrapper::set_hide_script_from_debugger`（直写
  `TransitiveCompileOptions.hideScriptFromDebugger_`）。
- 调用面恰 2 处、全在 bao_browser/runtime_bridge.rs（:552 evaluate_in_node_realm
  filename=`bao_evaluate_js`；:1569 wasm-init 探针）；servo 零调用。
- 因果链与 BUG-KNOWLEDGE.md BCE-20260622-004 记录一致（onNewScript →
  RememberSourceURL → AtomizeUTF8Chars → AtomCacheHashTable::lookupForAdd → 多 Realm
  create/destroy 生命周期 deref GC'd atom chars → SIGSEGV；runtime_bridge.rs:544-551）。
- **与 CDP Debugger 域交互（本轮新结论）**：hide 仅作用于两处 bao 内部 Node-realm
  编译；页面脚本走 servo 正常编译路径 → page-realm `new Debugger()` 的
  onNewScript/scriptParsed **不受该 patch 抑制**，当前无冲突。**前瞻约束**：若未来
  #11 typed adapter 或 Node-realm 观测需要覆盖这两类脚本的 onNewScript，hide flag 将
  静默隐藏之——届时需将该 patch 语义纳入裁决（修因/root 或显式豁免）；本轮仅记录。
- 与 BCE-20260621-002 gate 分工：gate 挡 servo devtools `fire_add_debuggee`
  （Realm::setIsDebuggee + BaselineInterpreter 翻转，initForOsr NULL deref 崩溃类）；
  hide 挡 onNewScript atom-cache UAF 类。两 patch + mozjs patch #4（BaselineFrame
  NULL activation guard）构成「debugger visibility ↔ 稳定性」治理面全景，已统一入账。

#### 27-4 裁决提案（每面三选一；产品语义面=提案禁实施）

| 面 | 现状 | 提案 | 依据 |
|---|---|---|---|
| CDP Debugger 域架构 | page-realm JS Debugger + console 嗅探 | **保持 JS-face**（不引 Rust C++ JS::Debugger） | C++ JS::Debugger 类型墙（需大量手写 glue）；JS face 已覆盖 scriptParsed/breakpoint/pause/step；缺口在胶水保真不在 API 面 |
| Debugger 胶水保真（callFrames:[]/lineNumber:0/environment{} /possibleBreakpoints/offsetLine 假 API） | 占位/假 API | **换原生（JS-face 内修）** | 数据全部 JS Debugger API 原生可取（frame.script.getOffsetLocation、getPossibleBreakpoints、frame.environment）；offsetLine 是缺陷类必改；#11 消费 |
| Debugger blackbox/unblackbox | 静默 no-op ok | **换 explicit-unsupported 错误** | SM 无原生 blackbox（Debug.h 0 命中）；servo 亦仅 client-side Map 模拟（debugger.js:13/801+）；fake success 违宪法 |
| Runtime exceptionDetails stackTrace | exceptionId:0 无栈 | **换原生（page-realm Error.stack/SavedFrame）** | 零 Rust 绑定需求；Rust 侧 CaptureCurrentStack 受 StackCapture（mozilla::Variant）构造墙——zeroed ABI 未验证禁猜，不走 |
| HeapProfiler snapshot | fail-closed 错误 | **保持 fail-closed** | ubi/Census 0 可达 |
| Memory 引擎计量（#19 soak） | 无 | **换原生（CollectRuntimeStats）**，前置=mozjs-sys jsglue.cpp ~5 行 RuntimeStats glue | 符号可达+null opv 容忍（MemoryMetrics.cpp:380），只差类型构造 glue |
| GC 事件观测（#19） | 无 | **换原生** | JS_SetGCCallback/SetGCSliceCallback/AddGCNurseryCollectionCallback 已 wrapped |
| Profiler（sampling） | fail-closed 错误 | **保持 fail-closed** | ProfilingFrameIterator 0 可达（cdp_handler.rs:275 注释准确） |
| bun_sm::debugger 死模块 | 零消费者 | **删除**（独立微波） | 自模拟占位结构违反禁占位门；本轮零删除纪律不实施 |
| WarningReporter/ErrorInterceptor | 未接 | **显式不用（当前）** | SetWarningReporter 已 wrapped；无产品需求驱动（interrupt warning 暂无观测面） |
| AllocationMetadata | 未接 | **显式不用（当前）** | 需 C++ AllocationMetadataBuilder 子类（类型墙），收益未证 |

#### 27-5 本轮落地（无争议项=1 个引擎实证测试；零产品语义、零 vendor、零绑定再生成）

- `src/bao_engine/tests/suite/debugger_native_tests.rs`（main.rs 挂载，
  `@trace TEST-ENG-001-DBG [req:REQ-ENG-001] [level:integration]`）：
  `JS_DefineDebuggerObject` 原生装出 ①`typeof Debugger === 'function'` ②5 个 hook
  （onNewScript/onDebuggerStatement/onEnterFrame/onExceptionUnwind/onPromiseSettled）
  全在 prototype ③`new Debugger()` 可构造且 findScripts/addDebuggee/
  removeAllDebuggees 方法面完整（无 addDebuggee——不触碰 BCE-20260621-002 崩溃类）。
  JS-face 可达性的活体证据=「保持 JS-face」裁决的锚。
- 验证（scoped）：`cargo nt -p bao_engine -E 'test(debugger_object_native_install)'`
  **1 run / 1 passed / 0 failed**。

**stop 条款核对**：无「大面积绑定缺失」降级——缺口精确四项（C++ JS::Debugger class
类型墙、ubi 0 可达、ProfilingFrameIterator 0 可达、RuntimeStats 需 glue），每项有
三选一裁决与依据。

**下一唯一动作**：裁决「换原生」各面归 #11 CDP 波消费（Debugger 胶水保真批 +
blackbox fail-closed）；CollectRuntimeStats glue + GC callback = #19 soak 前置；
bun_sm::debugger 删除=独立微波。#27 核查轮本体完成（proposal-complete）。

### 2026-09-10 / soak——#29 首轮 soak 收账：run-1 + aborted-1 判词三件 + run-1 挂死取证定性（数据分析轮，零产品代码改动）

**数据基线**（本轮已入库 `bench/results/2026-09-10-{ab114dce,d644e5df}/`）：
- **aborted-1**（ab114dce 构建，15:20-15:50）：10989 cycles / 1818.6s（30.3min），3 完整
  segment + 3 GC 探针，SIGTERM 外部终止（run.sh soak 无 timeout 包装，log 尾
  "Terminated"=e83 手 kill 后换 d644e5df 重建重launch）；
- **run-1**（d644e5df 构建，15:55 起，预期 60min）：5928 cycles / 926.1s（**15.4min，
  26%完整度**），1 segment + 1 GC 探针后 **t=926.1s（cycle 5928 内）进程挂死**，
  18:5x 取证后 kill（处置记录见下）。终局文档 soak.run-1.json 未产出——sidecar
  （append+flush per record）为完整数据载体。

**判词一（线性斜率外推 → 72h）**：唯一可外推项是 **post-GC 基线棘轮**——aborted-1
三探针序列 377.9→394.6→419.3 MiB（t=606/1206/1806s），OLS **+2.07 MiB/min =
35.4 KiB/s = 5.85 KiB/cycle**（6.04 cycles/s）。72h 线性外推 = **+8.7 GiB 基线**（另加
~250-300 MiB 垃圾稳态包络 + warm-up ~250 MiB 瞬态）。**局限（如实）**：3 探针/20min/
单机/两构建；段内分钟斜率（aborted-1 三段 +1.94/−2.00/+4.92，run-1 seg0 +3.31/seg1
部分 −1.35 MiB/min）短窗噪声 ±2 MiB/min 级；**线性 vs 平台未分辨**——探针增量
+16.7→+24.7 MiB/10min 呈加速倾向而非收敛；HWM 包络同向（aborted-1 30min 内
634.8→725.4 MiB）。结论：+8.7 GiB 只是量级预期不可作承诺；**需完整 60min（6 探针）
分辨线性/平台后方可谈 72h 数字**。

**判词二（突变段定性）**：①warm-up 瞬态：t≤1.6s HWM 415.6→634.8 MiB（引擎+首页
bring-up，指标设计已排除 segment 0 warm-up）；②**GC 边界事件=唯一显著突变源**：探针
即回收 202.1-268.3 MiB（4/4 次，见判词三），回收后 40-75s 内 churn 重建短暂越过前
HWM（run-1 全程 HWM 峰 698.8 MiB 首达 t=645.9s=探针后 40s；aborted-1 越 667.6 MiB 于
t=680.8s=探针后 75s）——malloc trim 归还后重 fault-in 的 overshoot，非新增峰值增长；
③其余分钟均值平滑无阶跃无悬崖；per-cycle RSS 锯齿 ±100 MiB 为每页分配/释放常态；
④**run-1 挂死本身即突变**：i=5928 突发停滞，零前驱——末 5 cycle 各相位延迟全部正常
（cycle 85-118ms，与全程均值一致）、RSS 646 MiB 中带（远低于 HWM）、fd=16/threads=87
全程恒定 → 非资源耗竭，是 page-pipeline 停摆。

**判词三（GC 回落性）**：**良好**。4/4 探针（跨两构建）`Bun.gc()`×2 回收
202.1/268.3/260.9/241.5 MiB（pre 的 35-42%），gc_eval_ms 4.1-5.4（GC 本身廉价），3s
settle 后 drop 已可见（malloc trim 生效）；回收后 ~60s 内垃圾回升至 ~600-630 MiB
工作集（锯齿上包络仍 <HWM）。run-1 单探针 post 369.5 MiB **低于** i=0 采样 414.5 MiB
（aborted-1 同构：probe-1 post 377.9 < rss_first 404.5）→ 10min 净增长 100% 可回收。
**无 GC-immobile 堆累积**；非 GC 棘轮归 C 级/thread-local 丢弃（见下对照）。

**S2 A-2 bounded leak 清单对照（「实测触发率」首个数据点，S2 退出标准尾项）**：
- fd=16 恒定（两跑全程零漂移）——零描述符泄漏；threads=87 恒定——零线程泄漏
  （页管线生命周期闭合，N1 形态无堆积）；
- **+5.85 KiB/cycle 非 GC 残留**：量级与 A-2 #3（NeverDrop TLS）/#4
  （BAO_RUNTIME_LOOP `Box::into_raw` MiniEventLoop 1/thread deliberate）+ N1
  （线程死亡 thread-local 丢弃，每页 1 ScriptThread）类完全一致——per-event bounded、
  总量线性累积的 C 级残留；**非 GC rooting 失败**（否则表现为不可回收堆，判词三已排除）；
- A-2 #1（foreign-thread forget）本形态零可观测；类别归因计数器（A-2 级粒度）仍缺，
  soak 只给聚合 RSS/GC 面。

**run-1 超时存活进程取证与定性（挂死，已处置）**：
- 证据链：PID 3214308（`bench-harness soak --duration-mins 60 --out
  .../soak.run-1.json`）；sidecar/log 最后 mtime 16:10（=t=926.1s 末记录），此后
  **2h45m 零写入**（健康期 6.4 记录/s）；主线程 wchan=futex_do_wait（内核栈 futex 链）；
  136/136 线程全 S、3s CPU delta=0（**纯静默死锁**；ps 累计 67:22 CPU 为健康期 churn
  线程组合计，非在烧）；终局文档不存在；挂死期 RSS 49.9 MiB/VmHWM 698.8 MiB/
  threads 136 = 87 基线 + 49 个挂死 cycle 页管线线程（页事件循环活着但永不应答）。
- 定性：**run-1 本体挂死（非 run-2、非正常长跑）**。挂点 ∈ cycle 5928 的
  {create_page, navigate, evaluate_js_web, close_page}——churn_cycle 内
  wait_for_pipeline_ready/wait_for_navigation 均有 15s 有界超时（触发会写 soak-end
  cycle-failure 记录，run-1 无此记录），**挂死根因面=无超时保护的阻塞调用**。
- **频率（头号发现，#19/#29 soak 目标即此）**：合并两跑 16917 cycles / 2744.7s churn
  1 次挂死 → MTBF 点估计 **≈46min**（Poisson n=1，95% CI ≈ [8min, 30h]）；60min 补跑
  单次挂死概率 ≈73%；**72h soak 预期 ~94 次挂死——RCA + 根治（或 churn_cycle 全相位
  有界化）前 72h 不可行，且不带 RCA 的 60min 补跑基本必败**。
- 处置：取证完毕后 kill（SIGTERM→SIGKILL 进程树 3213085/3214292/3214308，19:01
  清空零残留）；disposition=temporarily-missing：run-1 完整 60min 数据缺，以已落盘
  15.4min 分段为准。

**72h 调度建议（具体方案）**：
1. **前置 P0**：挂死 RCA 独立合同（candidate：cycle 内四个无界阻塞调用的相位级
   超时 + 挂死现场路由取证；顺带定性 node_timers_module promisify-custom 每 cycle
   一条 stderr 噪声，890KB/15min，低害但污染日志）；
2. **60min 完整跑 ×N**（分辨棘轮线性 vs 平台，需 ≥6 探针序列）：**systemd user
   timer `bao-soak.timer`**（OnCalendar 每日一跑；**TimeoutStartSec=90min 硬超时**
   ——run-1 挂死正是缺此保护的实证；结果落 `bench/results/<date>-<rev>/`，daily-ops
   次晨收账）。**不并入 daily-ops 串行窗口**（60min+ 会阻塞值班窗口）；
3. **72h 全时长**：手动 tmux/nohup + `timeout 78h` 外壳，不进常驻 timer；进入条件=
   60min 连续 ≥3 次零挂死 + 棘轮线性/平台已分辨。

**遗留移交**：①挂死 RCA 合同（P0，阻塞一切长跑）；②60min 完整补跑（RCA 后）；
③A-2 类别级 leak 计数器（S2 退出标准尾项）；④RED-1 裁决仍挂（前轮遗留，不变）。

### 2026-09-10 / soak——调度基建落地：`bao-soak.timer` 每夜 60min soak（72h 阶梯 step ② 兑现上文建议第 2 条；纯新增基建，零产品代码改动）

**Timer 状态**：单元对 canonical 在 repo、`~/.config/systemd/user/` 软链（daily-ops 形态照抄）：
- `scripts/systemd/bao-soak.timer`——`OnCalendar=*-*-* 00:07:00` + `RandomizedDelaySec=10m` +
  `Persistent=true`，与 daily-ops 06:07 错开 6h（soak 最坏 00:17+90min=01:47 收尾，不侵入值班窗口）；
- `scripts/systemd/bao-soak.service`——Type=oneshot，**`TimeoutStartSec=90min` 硬超时**
  （run-1 无保护挂死 2h45m 的根治面）、`TimeoutStopSec=2min`、`Environment=BAO_SOAK_MINS=60`
  与 `BAO_TEST_NETWORK=1`，ExecStart=`scripts/soak-daily.sh`；
- 调度器 `scripts/soak-daily.sh`：flock 防重入；**pending 标记 = TimeoutStartSec 击杀后唯一幸存
  证人**（下夜启动见未清 marker → 记 killed-no-completion 且 streak 诚实清零）；同
  date+commit 重跑守卫（segments sidecar 是 append 模式，先归档 `.prev-<HHMM>`）；run.sh 仍是
  唯一 soak 前端，结果照旧落 `bench/results/<UTC日期>-<commit>/`。
- 实测：daemon-reload + `enable --now` 成功，active (waiting)，**首跳 2026-09-11 00:07
  （+≤10m 随机延迟）**，此后每夜一跑。

**验证证据（2min 截短全链，2026-09-10 21:38–21:42 CST）**：drop-in `BAO_SOAK_MINS=2` 下
`systemctl --user start bao-soak.service` → **exit 0/SUCCESS**（4min07s 含构建校验）；产出
`bench/results/2026-09-10-a4a6738c/soak.run-1.{json,log,segments.jsonl}`：**1126 cycles /
cycle_failures=0 / 零挂死**——a4a6738c（BCE-20260910-003）修复后首个 churn 数据点（2min
截短跑，不构成 60min 判据）；`bench/soak-state/runs.jsonl` 首条 verdict=completed、
`counts_toward_entry=false`；streak_write 的 jq 变换独立验证通过。未活测分支：击杀→pending→
下夜清零路径（8 行，bash -n 过，待真实击杀事件自然验证）。

**进入条件进度文件**：`bench/soak-state/entry-progress.json`（tracked，含 target_streak/ladder/
ratchet_resolution）+ `runs.jsonl`（每夜一条全史）+ `last-run.json`（最新记录镜像）；transient 件
（lock/pending/night-*.log）已 gitignore。**当前 streak = 0/3（如实）**——尚无一次完整 60min
零挂死跑（run-1 挂死于 15.4min、aborted-1 30.3min 外部终止）；streak 只计 `duration_mins=60`
的完整零挂死跑，验证跑不计数也不清零。阶梯现状：①RCA 已闭（a4a6738c）✅ ②本 timer 落地 ✅
③72h tmux 外壳 blocked（进入条件 streak≥3 + 棘轮线性/平台分辨）。

**daily-ops 次晨收账说明（只录说明，不改 daily-ops 本体）**：每晨值班在 soak 收集节点——
①读 `bench/soak-state/last-run.json`（昨夜 verdict/zero_hang/duration_mins/note）与
`runs.jsonl`（全史）；②读 `entry-progress.json` 的 `streak`/`ladder`/`ratchet_resolution`；
③把昨夜 `bench/results/<date>-<commit>/soak.run-1.*` 连同 `bench/soak-state/` 状态文件一并
commit（夜间 timer 留下的脏树属预期，run.sh 的 GIT_DIRTY 记录如实反映）；④streak 达 3/3 且
棘轮分辨后，按上文建议第 3 条发起 72h tmux 外壳（手动 + `timeout 78h`，不进常驻 timer）。

### 2026-09-10 / #27 消费——CDP Debugger 保真批落地（裁决 2/3/9 实施：胶水换原生 + blackbox 显式 unsupported + bun_sm::debugger 死模块删除；+1 live e2e）

**裁决 2（胶水保真换原生）六项终态**（cdp_handler.rs Debugger 域重写）：
①断点 hit callFrames 恒 `[]` → `__bao_dbg_emit_paused` 命中时走真实帧链
（callee/script/offset/environment/this 全真值）；②硬编码 location（onDebuggerStatement
/list_frames lineNumber:0）→ `getOffsetLocation` 真值 + SM 1-origin→CDP 0-origin
边界换算；③假 API `s.offsetLine(line,col)` → `getLineOffsets(smLine)`（真 API）+
列号过滤 `getOffsetLocation().columnNumber`；④possibleBreakpoints 逐行合成 →
`getPossibleBreakpoints()` 原生；⑤getEnvironment `{environment:{}}` 占位 →
`frame.environment` 链原生（type/scopeKind/names()/getVariable 非调用式读，
`{unavailable:true}` 标 optimized-out/DebuggeeWouldRun 拒读）；另：断点注册表/
移除由「id miss → clearAllBreakpoints 全页 wipe」改为 `clearBreakpoint(handler)`
精确移除；catch-all 吞错全部改显式错误（fail-closed）。

**裁决 3（blackbox）**：cdp_handler 与 protocol.rs 双层显式
`not_supported(-32000)`（"SpiderMonkey's Debugger API has no native blackbox;
bao does not emulate one"），替代静默 no-op ok 假成功。

**裁决 9（死模块）**：`bun_sm/src/debugger.rs`（166 行 Rust breakpoint CRUD，
全仓零消费者）删除；`bun_sm/lib.rs` mod+re-export、`bao_engine/lib.rs` re-export
同步摘除。

**实施中发现并根治的三个真缺口**（e85 遗产复核暴露——原六项改造建立在不可用
地基上，旧行为从未真正工作过）：
- **G1 Debugger face 缺失**：servo 仅在 about:internal DebuggerGlobalScope 定义
  Debugger 构造器，Node Realm/page realm 均无 → `new Debugger()` ReferenceError
  被旧胶水 catch-all 吞掉（域从未活过）。根治：
  `runtime_bridge::register_node_realm_debugger_install`——脚本线程回调内
  `JS_DefineDebuggerObject`（jsapi2_wrappers:380 原生 face）装到 Node Realm
  global，HasProperty 幂等守卫，**与 evaluate 回调同 FIFO 队列 + 同 stale-realm
  生命周期校验**（navigation 换 ScriptThread/cx 后 realm 在当前 cx 重建，
  install 落在配对 evaluate 真正使用的 global 上）。
- **G2 SM compartment 铁律**：Debugger 必须与其 debuggee 异 compartment
  （page realm 内 `new Debugger(window)` 直接 throw）。Debugger 实例与胶水落
  **Node Realm**（`evaluate_js` face），`dbg.addDebuggee(window)`（window =
  Node Realm 对 page global 的跨 compartment wrapper）。事件发射走
  `window.console.log`（page console → servo delegate 传输；Node Realm 自有
  console 写 process stdout，不可用作 `__BAO_EVT__` 通道）——delegate 双实现
  （BaoServoDelegate + BaoWebViewDelegate）补 `__BAO_EVT__` 前缀臂：结构化事件
  走 ConsoleMessage parser（Path A），Path B 活跃时不再降级 Log.entryAdded。
- **G3 console_log_tx 无回传**：`set_console_log_channel` 只设 runtime 级
  delegate，servo 控制台消息按 webview 路由（读 `state.console_log_tx`）——
  run_browser 先建页后装通道，**首页 state 恒 None**（set_event_channel 早修过
  同型缺口，console 通道漏修）。根治：镜像 event_tx 回传到全部现存页 state。

**BCE-20260621-002 复核**：JS-face `addDebuggee` 同样触发
`Realm::setIsDebuggee` JIT instrumentation（servo Rust-side fire_add_debuggee
被 disable_script_debugger 挡住，JS face 不经该门）——mozjs fork patch #4
（BaselineFrame NULL activation guard）为防护；live e2e 实证 attach+断点+后续
JIT 活动零崩溃。BCE-20260622-004（hideScriptFromDebugger）：仅作用于 node-realm
evaluate 编译，与 Debugger 域 onNewScript 无冲突（27-3 前瞻约束仍立）。

**live 保真 e2e**（`bao_browser/tests/suite/cdp_debugger_fidelity_tests.rs`，
生产 run_browser 全布线：console receiver + event channel + WS registry）：
保真三角——scriptParsed.startLine、setBreakpointByUrl 解析 location
（getOffsetLocation 真值非请求回显）、paused 帧 location 三处独立原生读必须
同线；断点命中 callFrames 非空 + functionName/location/scopeChain
（local/closure/global + className 描述、无假 objectId）/this 全真值断言；
getPossibleBreakpoints 原生条目含已证可断行；blackbox/unblackbox 显式
-32000；removeBreakpoint 后负窗口零复发；`debugger;` 语句真位置暂停（函数级
Script 同 url 异 id 已入断言口径）；GetEnvironment bridge face 新数组形态。
**1 run / 1 passed（~4.2s）**；attach 后多次 evaluate+JIT 活动无 SIGSEGV。

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

### 2026-09-10 / #19——引擎原生 Memory 计量落地（#27 裁决 6 消费：CollectRuntimeStats jsglue 构造 + bao_engine 内部面 + soak 采样点接入）

**#27 裁决 6 实施**（"Memory 引擎计量（#19 soak）→ 换原生（CollectRuntimeStats），前置=
mozjs-sys jsglue.cpp RuntimeStats glue"）：类型构造墙以 vendor glue 破——
`JS::RuntimeStats` 纯虚 `initExtra*Stats` + `js::Vector` 成员使 Rust 不可安全构造
（bindgen 字段全 pub 但零化构造 UB 风险，#27 核查轮结论），C++ 侧子类化 + POD
copy-out 是最小差分。opv=null 显式容忍（MemoryMetrics.cpp null 守卫，servo 形态）。

**三层落地**：
- **vendor/mozjs**（在册 patch 文件，CSP 记账式追加，零删除）：jsglue.cpp 追加
  `BaoRuntimeStatsPOD`（7×usize：gcHeapChunkTotal/gcHeapGCThings/zone·realm live
  GC things/zoneUnusedGcThings/zone·realm count）+ `BaoRuntimeStats : JS::RuntimeStats`
  子类（no-op extra hooks，MallocSizeOf 复用文件内既有 malloc_usable_size 形态）+
  `BaoCollectRuntimeStats(cx, ServoSizes*, POD*)`——ServoSizes rollup =
  runtime+zTotals+realmTotals 三段 addToServoSizes（单一 rtStats.addToServoSizes 不含
  zone/realm 段，会低估）；mozjs glue2_wrappers.in.rs 追加 wrap!。
- **bao_engine 内部面**（`#[doc(hidden)]`+零 pub 稳定承诺，execution_control 同形态）：
  `memory_stats.rs` 的 `EngineMemoryStats`（13 字段）+ `collect_runtime_stats(cx)`；
  fail-closed——引擎失败返 Err，禁零填充占位。合同：须在 cx 属主线程静止点调用。
- **bao_browser 桥 + soak 采样点**：`runtime_bridge::register_engine_memory_stats_
  collection`（ScriptThread 回调，与 Node-Realm evaluate 同 FIFO 队列同
  happens-before 形态）+ `PageHandle::collect_engine_memory_stats`（register →
  drain → OnceLock 读）；soak `forced_gc_probe` 探针旁加 pre/post 引擎采样
  （`Bun.gc()` ×2 前后各一次），sidecar gc-probe 记录含 `engine_pre/engine_post`，
  result doc 新增 `forced_gc_engine_{gc_heap,gc_things,malloc_heap}_kib` /
  `zone_count` / `realm_count` / `gc_things_drop_kib`（引擎级 GC 回收判定数据——
  #29 soak 量化缺的正是这个）；引擎计量失败降级记录不伪造（probe 契约同型）。
  JS-face 零新增（纯 Rust 内部观测面，无产品语义扩张）。

**引擎实证**（`bao_engine/tests/suite/memory_stats_tests.rs`，30k live strings 负载）：
①全字段非零（gc_heap_chunk_total/gc_things/zone·realm live/malloc_heap ≥ 各自下界，
  zero 即 copy-out 破损）；②SM totals 恒等式**精确成立**：
  `gcHeapGCThings == zTotals.sizeOfLiveGCThings() + realmTotals.sizeOfLiveGCThings()`
  （MemoryMetrics.cpp:723 就是这么算的，同一 finalized 对象 copy-out）；③chunk 覆盖
  一致性 + ServoSizes used-heap 与 headline GC-things 双 rollup 同源相等。
  `cargo nt -p bao_engine -E 'test(engine_memory_stats)'` **1/1 PASS**。

**soak 2min 截短验证数据面**（`bench/results/2026-09-10-03396a13/soak.run-1.{json,
segments.jsonl}`，duration=2min/segment=1min/post=0，git_dirty=true 如实入档）：
gc-probe sidecar 记录含 engine_pre/engine_post 全字段；result doc 含
forced_gc_engine_* 六指标。引擎计量面数据通路闭合。

**回归**：`cargo nextest run -p bao_engine --cargo-profile test-ci` **376/376 PASS**
（374 基线 + debugger_native 1 + 本轮 1）；`cargo nextest run -p bao-browser
--cargo-profile test-ci`（xvfb）**1824/1824 PASS**（1216 基线已随今晚多波增长；
含本轮 page/runtime_bridge 改动的全套浏览器回归，2 slow 均 PASS）。

**账本状态更新**：#27 裁决 6（Memory 引擎计量）**已消费**；GC callback
（JS_SetGCCallback 族,已 wrapped）仍归 #19 待接。soak 的 RSS 代理 + 引擎级 heap/GC
双数据面就位——#29 soak 量化的引擎级判定数据（live-GC-things 回收/增长趋势）自此可测。

### 2026-09-10 / #26——Stencil 可达面核查 + stencil-cost 收益判据 bench（裁决：in-memory cache 值得做；XDR encode=绑定缺口；off-thread=上游 API 缺失关闭）

**基线**：bao master `03396a13`（worktree dirty=true——本波 stencil-cost bench 代码即
dirty 内容，结果与代码同 commit 落库可复现）；mozjs 不变（bao-mozjs-sys 140.14.0-0，
bindgen jsapi.rs md5 `ffaa6002` = #30 seed inventory 同源）。零产品代码改动（纯核查 +
bench 场景新增）。

**方法**：#30 inventory（1077 symbol）过滤 stencil/xdr/transcode/offthread 族 + 绑定级
双核实（jsapi2_wrappers.in.rs / bindgen jsapi.rs 行号）+ vendored SM 源码契约核实
（JSStencil.h / CompilationAndEvaluation.cpp）+ 新 bench `stencil-cost`
（`bench/harness/src/stencil_bench.rs`，METHODOLOGY 六要素 + R=3 进程级重跑，
`bench/run.sh stencil-cost`）。

#### 26-1 绑定可达面（in-memory cache 全链路无阻塞）

| 能力 | 符号 | 绑定 | 位置 |
|---|---|---|---|
| 编译→Stencil（utf8/u16） | `CompileGlobalScriptToStencil`(+1) | safe-wrapper（wrappers2） | jsapi2_wrappers.in.rs:330-331 |
| 模块→Stencil | `CompileModuleScriptToStencil`(+1) | wrappers2 | :332-333 |
| Stencil 实例化（script/module） | `InstantiateGlobalStencil` / `InstantiateModuleStencil` | wrappers2 | :334-335 |
| XDR decode | `DecodeStencil`(+1) | wrappers2 | :336 |
| delazification 收集 | `StartCollectingDelazifications` 族 7 个 | wrappers2 | :337-343 |
| 执行实例化 script | `JS_ExecuteScript` | wrappers2 | :585 |
| 引用计数/元数据 | `StencilAddRef`/`StencilRelease`/`IsStencilCacheable`/`SizeOfStencil`/`StencilIsBorrowed` | bindgen raw | jsapi.rs:14652+ |
| Rust 侧 wrapper | `mozjs::rust::Stencil`（Drop→StencilRelease） | 已有 | rust.rs:663-691 |
| **XDR encode** | `EncodeStencil` | **bindgen 0 命中**（C++ 有：JSStencil.h:188） | **绑定缺口** |
| **off-thread 编译** | `JS::CompileToStencilOffThread` 族 | **上游 public API 不存在**（仅 CompileOptions.h:19 注释残留 + HelperThreads 内部任务；bindgen 只有 JIT 侧 `JS_SetOffthread{Baseline,Ion}CompilationEnabled` jsapi2:577-578） | **不可达** |

关键契约事实（vendored JSStencil.h）：stencil "may be instantiated into any Realm on
the current runtime and may be used multiple times"——**跨 realm 复用是文档化支持面**；
`InstantiateGlobalStencil` 的 `InstantiationStorage` 参数可传 null（C++ 默认
`= nullptr`，规避未绑定的非平凡析构）。另一面：mozjs rust.rs:667-668 的
`unsafe impl Send/Sync for Stencil` 被上游**注释保留**——stencil 非 Send/Sync，且
runtime-scoped → **缓存只能是 per-JSContext，禁跨线程共享**（与 S0-3 拓扑一致：每
ScriptThread/CLI/worker 线程各自的 cx 各持缓存）。

#### 26-2 现状编译路径事实（每 realm 重复编译的机制根源）

`JsContext::eval` → `mozjs::rust::evaluate_script` → `wrappers2::Evaluate2`
（`JS::Evaluate` utf8）→ `EvaluateSourceBuffer`
（vendored `vm/CompilationAndEvaluation.cpp:642-663`）：**每次调用**
`frontend::CompileGlobalScript` 全量 parse+bytecode 生成 + `setIsRunOnce(true)`；
该 SM snapshot **eval cache 已移除**（`lookupEvalCache` 全文件 0 命中）——同源重复
eval 无任何缓存层。

生产重复编译面（每 realm/每调用付费）：

1. **stealth blob**：`inject_js_hooks`（engine_props.rs:1392，`install_stealth_props`
   尾部 :1586 调用）——每 Page Realm + 每 Worker/SW realm 全量 eval
   `combined_js()`（28,221 B，typeof-guarded bare-realm 安全，36 处守卫）；
2. **CDP evaluate**（Runtime.evaluate 每调用编译；Playwright/Puppeteer
   waitForFunction 轮询=同源高频重复形态）；
3. **`vm.createContext`**（node_vm.rs:657，同 cx 每 sandbox 重复编译 contextify wrapper）；
4. **CLI `eval_module`**（S0-3 #4 churn 风险 c：每次调用 fresh realm 全量编译）。

#### 26-3 stencil-cost bench 数字（R=3 × n=60/phase，loadavg 9.37 共享机入档；test-ci 档）

| payload（bytes） | A1 realm+编译+执行 | A2 编译+执行 | B stencil 编译 | C 实例化+执行 | 编译占比 | 加速比 | breakeven |
|---|---:|---:|---:|---:|---:|---:|---:|
| stealth（28,221） | 968.2 µs | 830.4 µs | 598.8 µs | 166.6 µs | **79.6%** | **4.9×** | **0.9 realm** |
| stealth_x10（282,258，合成） | 6,687.5 µs | 6,591.0 µs | 5,602.0 µs | 755.5 µs | **88.6%** | **8.7×** | 1.0 realm |
| tiny_1p1（3） | 161.6 µs | 39.1 µs | 2.0 µs | 34.1 µs | 13.8% | 1.2× | — |

（中位数；判定指标 run 间 spread≤1.0%（stealth/x10 占比），tiny 绝对值小故 spread 大。
结果文件 `bench/results/2026-09-10-03396a13/stencil-cost.run-{1,2,3}.json`，
fail-closed 全验证过——x10 marker 探针 + 逐 realm 功能探针 + C 相 JS_ExecuteScript
全 true。）另：realm 建造成本 ≈ A1−A2 ≈ 138 µs（与 realm-create-drop 基线一致量级）。

#### 26-4 裁决

- **in-memory per-JSContext Stencil cache：值得做**（完成定义第 6 条门通过）——生产
  stealth blob 编译占比 **79.6%**（阈值 5% 的 16 倍），加速 **4.9×**，breakeven
  **0.9 realm**（同 cx 第 2 个注入 realm 即回本）；载荷越大占比越高（x10=88.6%），
  parse 主导。实现波范围（S3 排程，本波零实现）：
  ① `bao_engine` 层 StencilCache（per-JSContext 所有权——S0-3 表 owner 语义：cx 属主
  线程持有，stencil 非 Send/Sync + runtime-scoped，禁跨线程/跨 cx 共享；key=源
  hash+编译选项相关字段；显式 opt-in API，非全局拦截）；
  ② 调用点优先级：stealth `inject_js_hooks`（per-profile blob）> `vm.createContext`
  contextify wrapper > CDP evaluate 同源重复（waitForFunction 形态）> CLI `eval_module`；
  ③ tiny 事实（3B 编译仅 2.0 µs、占比 13.8%）→ 缓存准入需 size/hit 感知
  （小源查表成本占比高，防负收益）。
- **XDR persistent cache：blocked-绑定缺口**——`EncodeStencil` C++ 存在但 bindgen
  未绑定（DecodeStencil 已绑，非对称）；需 mozjs-sys wrapper 增补（候选入 vendor patch
  清单）；维持 S3「只有正确性完成后」门，未解锁。
- **off-thread compile：关闭（上游 API 缺失）**——该 SM snapshot public 面无
  off-thread stencil 编译族（仅内部 HelperThreads + JIT 侧开关）；且主线程 blob 编译
  仅 ~600 µs，收益上限小。不列为 Bao 目标。

**下一唯一动作**：#26 实现波（S3 排程）——`bao_engine` StencilCache 最小闭环 +
`inject_js_hooks` 接入 + 同 bench 对照复跑（A2 vs C 作为回归判据）；XDR encode 绑定
增补独立排程。

### 2026-09-10 / RED-1 P-A 落地——同域导航 realm discard 时 bao timers 随 realm 终止(vendor hook + registry purge + zombie-fire 探针;RED→GREEN live)

**基线**:bao master `642da220` + 当日并行波 dirty tree(多 Agent 共树);mozjs 不变。用户裁决
2026-09-10 P-A 直做(浏览器行为一致:导航丢弃旧 realm 时同步 cancel 其全部 bao timers)。

**形态**(镜像 pump 桥注册面,ADAPT):

1. **vendor(servo script,均在册 patch 文件)**:
   - `event_loop/script_thread.rs`:`BAO_REALM_DISCARD_CANCEL` OnceLock + `register_bao_realm_discard_cancel`
     (`Box<dyn Fn(*mut c_void, *mut c_void)>`——c_void 双参解耦两 mozjs crate 实例,同 pump 桥)
     + `bao_cancel_timers_for_discarded_realm(cx, global)`。**调用点在
     `handle_exit_pipeline_msg` 的 `window_detached` 门之前**——同域导航 browsing context
     已迁移时 `clear_js_runtime` 整段被跳过(detached 分支),hook 只挂 clear_js_runtime
     会竞态漏 purge(live 实证:同测试同 binary 两种结局);两侧分支均覆盖。
   - `script/lib.rs` re-export(`register_bao_realm_discard_cancel` / `BaoRealmDiscardCancel`);
     `components/servo/lib.rs` wrapper(`servo::register_bao_realm_discard_cancel`)。
   - window.rs **零触碰**(中间形态曾改 `clear_js_runtime(&self, cx)` 签名,因 detached
     竞态发现后整体回退——vendor diff 最小化)。
2. **`bao_runtime/timers.rs`**:
   - `cancel_timers_for_global(cx, global)`:DEAD_GLOBALS 标记 + 两阶段(候选快照→
     remove+cleanup_callback(REGISTERED in-Box slot 地址释 raw root,同 BCE-20260910-004c
     教训)+ gc_store 回调清除);per-thread ID + per-global 精确匹配,他 realm/他线程零误伤
     (unit 锁定:global_b 存活+无 root 条目不动)。
   - 探针三件(process-global AtomicUsize,跨线程可读):`realm_discard_events_total`
     (hook 到达次数,含零 purge——链条可能已在垂死 realm 自断)、
     `realm_discard_cancelled_total`(实际清除条数)、`zombie_fires_total`
     (**DEAD_GLOBALS 上执行的 fire 计数**——ABA 安全:schedule_raw 对同地址注册即 un-mark)。
3. **`bao_browser/lib.rs`**:`BaoRuntime::new` 里 `servo::register_bao_realm_discard_cancel`
   → `bun_runtime::timers::cancel_timers_for_global`(pump 桥注册旁,OnceLock 首注即定)。

**RED 证据**(wiring 注释掉=pre-fix 语义,同 binary 对照):
- 行为面(首轮):武装 setImmediate 链(fetch 打点)同域导航后 **25×`n=NaN` zombie tick
  持续执行**(2s 观察窗 37≠17);trace 级:schedule/fire 逐 id 递增过 nav 点,
  `n=NaN`=window proxy 已换而 tick 仍执行(commit→clear 窗口签名)。
- 探针面(终版测试):`same_domain_nav_discards` FAIL「pipeline exits at page close never
  reached cancel_timers_for_global (events 0 -> 0)」;`churn` FAIL「4 navs must deliver
  ≥4 realm-discard notifications (events 0)」;cross-host 轴 pre-fix 即绿(线程整体退场,
  registry 随线程死——N1 语义,回归轴)。

**GREEN**:`realm_discard_timers_tests` 3 e2e(同域导航 zombie=0+close 触发 discard 探针、
4 页 churn 每页 zombie=0+events≥N、跨域+页关零回归)+ `cancel_timers_for_global_matches_
only_that_global` unit——全绿。timers 单元族 60/60。

**关键实测发现**(测试方法学沉淀,后续 lifecycle 测试必读):
1. **同域导航旧 pipeline exit 常态迟到**(有时只到 teardown):迟到期间旧 realm *活着*
   (proxy 已换、`n=NaN`,但非 zombie)——确定性 discard 面=page close(强制 pipeline
   exits)与线程 teardown。zombie 风险窗口=exit 迟到但最终到达且条目仍在的部分。
2. **fixture/HTTP 到达≠执行**:tick 的 fetch 在 egress 队列里可滞后秒级落地,arrival 计数
   会把 pre-discard fetch 误判为 zombie hit(两轮假 RED/GREEN 教训);必须数**执行**
   (zombie-fire 探针/闭包内 k 计数),不得数到达。
3. **垂死 realm 自断链**:commit→exit 窗口内 tick 的 fetch 可抛错→链条停止 re-arm→
   exit 时 registry 已空(purge=0 但 discard 正常发生)——「清除条数」不可作确定性断言,
   「hook 到达 events + zombie 执行=0」才是合同。

**验证**(波末一次测):`cargo nt -p bao-browser -E 'test(realm_discard_timers_tests)'`
**6/6**;`-p bun_runtime -E 'test(timers::)'` **60/60**;回归 `-p bun_runtime -p bao_engine`
全量 **1600/1600**(1595 直绿 + 5 个 stale-binary 守卫测试在 `cargo build -p bao_bin` 后
复跑 5/5 + 1 pre-existing skip)+ bao-browser worker 族 **355/355**(同树并行波次文件的
瞬时编译错误按归属分责,未计入)。

**回滚点**:单 commit revert(vendor 4 文件 hook+re-export + timers.rs purge/探针 +
bao_browser wiring + 测试),无数据/接口迁移。

**遗留如实记**:
- N2 第二行(ConcurrentTask 完成指向旧 realm,一次性 bounded)未随 P-A 立法——原黄,
  仍黄(本轮零触碰);
- 同域导航旧 pipeline exit 迟到本身(旧 realm 长活=内存驻留面)是 #29 nav-churn 的独立
  输入,非本波范围;P-A 已保证 exit 到达即清。

### 2026-09-11 / #26 实现波——per-JSContext Stencil cache 落地 + stealth blob 接线 + bench 兑现(裁决消费:全部完成)

**基线**:engine `496e2926` + stealth `27dbb606`(bench 记录 commit=27dbb606,dirty=true 即
bench D-phase 代码本身,同 e90 落库形态);regression 复跑 engine `383/383` + stealth
`1691/1691` + browser stealth/worker/SW/fingerprint 族 `503/503` 零失败(nextest test-ci)。

#### 26-5 实现(bao_engine::stencil_cache + inject_js_hooks 接线)

- **模块** `bao_engine/src/stencil_cache.rs`:`evaluate_script_cached`(签名镜像
  `mozjs::rust::evaluate_script` + filename/line 显式参数;Err 契约一致——pending
  exception 留给调用方,`maybe_resume_unwind` 同步;`<` 1024 B 源走原 evaluate 路径并计
  bypass——判据 ③ 小源防负收益)。
- **所有权/lifecycle**(26-1 契约落实):thread-local per-JSContext(owner_cx 不匹配即全量
  reset+StencilRelease;`shutdown_thread_sm` 在 JS_DestroyContext 前显式 clear——防 cx
  地址复用吃到死 runtime 的 stencil);stencil 进程堆引用计数
  (`Release→js_delete`,CompilationStencil 自持 LifoAlloc+RefPtr<ScriptSource>),worker
  线程死亡时 TLS dtor 释放=纯堆 free,无 cx 访问(已核实 SM 源码)。
- **键**:wyhash(`bun_wyhash`,workspace 复用,~0.1ns/B)做 bucket locator + 命中时
  (source, filename, line) 逐字节精确比对——hash 碰撞退化为 miss,绝不错 stencil
  (SipHash 28KB≈15-19µs 会吃掉 3/4 理论收益,wyhash 后残余≈噪声底)。
- **容量**:16 条 LRU,逐出即 StencilRelease;帧级 AddRef/Release 防重入逐出 UAF。
- **接线**:`inject_js_hooks`(engine_props.rs:1451)单漏斗换
  `evaluate_script_cached`——page W1a/worker 第二 drain/SW scope/node realm 全部
  `install_stealth_props` 调用方自动继承;per-realm profile 产生不同 source=不同键。
  编译路径零语义变化:同 utf8 transform、同 CompileOptionsWrapper 默认、同 AutoRealm、
  同错误臂(BCE-20260621-001 pending-exception 消费不变)。引擎级唯一 delta:实例化
  script 不带 isRunOnce(每次调用都是全新 JSScript,run-once 多次执行陷阱不可能触发;
  TreatAsRunOnce=false 是严格保守形态)。
- **等价断言**(完成定义②,双层):
  - `bao_engine` 单测 7 条:plain vs cached-miss vs cached-hit 三 realm 状态指纹逐字节
    相等 / floor bypass / LRU 容量 / source·filename·line 键分离 / 语法+运行时错误路径
    / clear 生命周期;
  - `bao_stealth` 真 blob 集成 2 条:firefox_default `combined_js()`(28,221 B)三臂指纹
    逐字节相等且指纹必须观测到 patched Date.now;A/B 双 profile 交错评估各自观测各自
    marker(2 entries)。

#### 26-6 bench 兑现(stencil-cost +Phase D,`bench/results/2026-09-10-27dbb606/`,R=3,test-ci)

| 指标(stealth 28,221B) | e90 判据(预测) | 实现后实测(median of R=3) |
|---|---:|---:|
| 每 realm 注入成本 | A2=830µs(负载 9.4) | A2≈1273-2351µs(本机负载 ~30,绝对值不跨日比较) |
| cached path(D) | C=167µs 预测 | D=244-250µs(同负载日 A2/C/D 同窗口) |
| **cached_speedup_x** | **4.9×** | **5.21×**(5.21/5.21/11.05;run-3 A2 遭负载尖峰抬高,保守读 1-2 轮 5.21×) |
| 消除编译占比 | 79.6% 理论 | **80.8%**(run-3 90.9%) |
| 缓存新增开销/原始路径 | —(判据 ≤~16%) | **0.0-0.15%**(wyhash+逐字节验证≈噪声底) |
| stealth_x10 speedup | 8.7× | 8.64×(7.02/8.64/9.84) |
| tiny_1p1 | 13.8% 占比→floor | bypass 计数证实,speedup≈1.0(设计如此) |
| 缓存命中 | — | 64/64 hits/run(fail-closed 计数器断言) |

  判据口径:residual=(D−C) 在 ~ms 级载荷上跨相位测量吸收同级调度噪声(可测出负值),
  故门控用 `cached_added_overhead_pct_of_original`=max(0,D−C)/A2 ≤16%(A2 为稳定大分母,
  D 与 C 共同坐落其下);`cached_speedup_x ≥3.0` 硬地板。3/3 run 全 PASS。中间诚实记:首
  轮 R=3 用 (D−C)/D 门控在负载 ~50 下 2 run 红于 x10(53.7%/41.5%——纯相位间噪声,非算
  法开销),门控口径修正后重跑,旧失败 run 不留档、口径变化入 bench 头注释与本节。

**终态**:#26 实现闭环(完成定义①②③④⑤全过)。剩余独立排程项不变:XDR encode 绑定
增补(`EncodeStencil` bindgen 缺口)维持 blocked;off-thread 家族维持关闭(上游无 API)。
下一消费者候选(未排程,需新判据):CDP evaluate waitForFunction 形态、`vm.createContext`
contextify wrapper、CLI `eval_module`。

### 2026-09-11 / S1 CLI 消费——`--timeout <ms>` + SIGINT→cancel 产品级暴露落地（立法提案：全部完成）

**基线**：bao master `642da220`+（并行波工作树；servo 0.5.8 bump 同树）；mozjs 不变。
**授权**：S1 节「SPEC 立法提案停点」——用户裁决 2026-09-10「全部完成」，本节为消费记录。

**代码改动**（全部在 bao 层，bao_engine 零改动——纯消费 S1 既有接口）：

- `bao_cli/src/cli.rs`：clap 全局可选参数 `--timeout <MS>`（value_parser 拒 0，fail-closed
  exit 2）；仅脚本执行入口消费（`bao run` / 顶层 `-e` / `run -e` / `run --module -e` /
  `bao run <file>`），非脚本子命令给了就拒（exit 2 + 消息，禁静默忽略）。三个入口
  （run_eval / run_file / run_module_eval）的 None 路径字节级不动；Some 路径 =
  `execution_control()` + `InterruptBridge` + `eval{,_module}_with_control` /
  `run_file_with_control`。退出码契约：TimedOut→**124**（GNU timeout(1) 惯例）、
  Cancelled→**130**（128+SIGINT，shell 惯例信号中断码；信号被转为受控终止故以等价码
  呈现）、其余维持 1——退出码依据即提案停点预录的「默认 124/timeout 风格」。**关键语
  义**：control 终止码优先于 should_exit/exit_code 尾检查——module 管线把 uncatchable
  终止路由进 uncaught-exception 机制会 `request_exit(1)`（副作用），124/130 是真信号必须
  胜出（无此前置时 `run --timeout x.mjs`/`run --module -e` 误报 1，实测复现后修复）。
- `bao_runtime/src/interrupt_bridge.rs`（新增）：SIGINT→cancel 桥。**AS 安全设计**：
  `cancel()` 走 `JS_RequestInterruptCallback` → 引擎 `requestInterrupt`（Runtime.cpp）取
  FutexThread 锁 + wasm 中断锁——非 async-signal-safe，故 handler 体仅 `write(pipe, 1)`
  （self-pipe），专职 watcher 线程 read→对 armed control 调 cancel（普通线程上下文）。
  SA_RESTART（libuv 同款）；pipe2(O_CLOEXEC) 防 exec 子进程持有写端。**吞信补偿**：无
  armed control 时收到的 SIGINT 记 `eaten_unhandled`，Drop 恢复先前 disposition 后
  `raise(SIGINT)` 重放——Ctrl-C 永不被静默吞掉。**既有信号面交互（已查）**：
  `Bun__registerSignalsForForwarding`（product_native_symbols.rs，sync spawn 前后注册/
  注销）会覆盖本桥 handler 并在注销时置 SIG_DFL——此后 SIGINT 退回暴露前行为（默认
  disposition），降级为现状语义、无腐蚀，记录接受；无 `process.on('SIGINT')` 分发机制，
  无监听器冲突。
- `bao_runtime/src/runtime.rs`：`run_file` 重构为 prepare_file_execution（读文件+require
  dir+file globals+script/module 判定，顺序与历史逐字节同序）+ eval_file_body（四组合纯
  委派）；新增 `#[doc(hidden)] run_file_with_control`。plain 路径行为零变化。另 re-export
  `TerminalState`（CLI 退出码映射用）。

**测试**（`bao_browser/tests/suite/bao_cli_timeout_e2e_tests.rs`，9 用例，真二进制子进程 +
kill -INT）：T1 顶层 `-e` runaway 400ms→124（[350ms,10s] 确定性窗口 + stderr 稳定终止错
误）；T2 `run --timeout x.mjs`→124（module 文件入口）；T3 `run --timeout x.js`→124
（script 文件入口）；T4 大 deadline 快脚本→0（未用 deadline 不触发）；T5 `--timeout
60000` runaway + kill -INT→130 + stderr "cancelled"（稳定 Cancelled 终态）；T6 **无
--timeout + kill -INT→死于信号 2**（默认 disposition 负对照，「无 flag 行为不变」的可执
行证据）；T7 `--timeout 0`→clap 拒 2；T8 `doctor --timeout`→拒 2；T9 `run --module
--timeout -e`→124。

**验证**（波末一次测）：`cargo nt -p bao-browser -E 'test(bao_cli)'` 9+2 全绿；
`cargo nt -p bao_cli` 12/12；`cargo nt -p bao_engine` **383/383**（基线随并行波 376→383，
零回归）；`cargo nt -p bun_runtime` **1217 passed / 1 pre-existing skipped**（基线
1216→1217，零回归）。手工 smoke 补充：`process.exit(7)`+timeout→7（显式退出码不受优
先级破坏）、无 flag throw→1、无 flag `process.exit(3)`→3。

**已知边界（记录不改动）**：① module 管线在受控终止时额外打印一行「bao: uncaught
exception: undefined」（module loader 错误路径对 uncatchable 清场后 pending 值的既有报
告格式，属 bao_engine/module_loader 域，本波禁碰，未抑制）；② SIGINT 落在 eval 返回与
disarm 之间的指令级窗口会被 cancel 静默消费（与内核信号投递固有竞态同级，Node/libuv
同类）；③ 脚本阻塞在不可重启长 syscall 时 cancel 等返回 JS 后才生效（与 S1 deadline
watcher 同类限制，已文档化）。

**BCE 检查**：产品暴露非 bug 修复；发现的「module 路径终止码被 uncaught 路由覆盖」为
本波引入即修（同一 commit 内闭环），无同类残留（三个入口统一 termination_exit 优先序）。

**回滚点**：单 commit revert（cli.rs + runtime.rs + lib.rs + interrupt_bridge.rs 新模块 +
suite 测试注册 + Cargo.toml dev-dep + 本账本节）。

**下一候选(2026-09-17 补记,未排程,待下轮 daily-ops 裁决)**:S4 #27 Debugger 原生面——「换原生」各面已裁归 #11 CDP 波消费(需新判据);#30 余项(patch supersession 自动对照、cap ledger last_audited 联动)锚定 mozjs 前移后的升级波首跑;XDR encode 绑定增补(`EncodeStencil` bindgen 缺口,vendor patch 清单项)维持独立排程。S0-S3 全部消费完毕(S0 census+#23、S1 #24+#25、S2 #29、S3 #26+#28 CLOSED)。

---

## mozjs 跨版本升级波(2026-09-20 启动 · 7 轮预算长任务协议)

**裁决**:立即启动(probe 五节判据:上游稳定 3+ 周、0.26.1 收敛尾、吸收债复利、-lts 回退锚在案;等待无收益)。Round0(patch 真源捕获)= 主波第一步,er0 执行中。回退锚:上游 140.14.0-0-lts 分支 + 本地 0.22.1/140.14.0-1。

### E-MOZJSPROBE 结论(2026-09-20,只读双向差分,全 commit 级实证)

**窗口**:vendor Rust 面基线 eb36274(08-21,pre-#801);C++ 源面基线 etc/COMMIT `ee9f2b2`(release 资产 tag,SM 140.14.0)。至 main `6170583a`(09-13)共 7 commit:#801(&mut JSContext)/#796(ohos/LLVM19)/#807(CI)/#803(**SM 140→153**,3000 文件+extracted-crates 8 crate+icu 2.1.2)/#802(删 deprecated,-breaking)/#811+#812(JobQueue draining)。版本 0.22.0→0.26.1 / 140.14.0-1→153.0.0-2。

**逐 commit 三分类**:#801 INDEPENDENT 必吸+PATCH-CONFLICT(rust.rs=P2/P3 锚);#796 INDEPENDENT 可选;#807 零代码;#803 引擎本体 INDEPENDENT + PATCH-CONFLICT(P4 BaselineFrame*/P5 jsapi.*/B1 B2 build.rs)+ **REPEAT-RISK×2**(①extracted-crates vs BAO 自研 bao-mozjs-src-intl #42 切片卫星——同题两解,波内对照取优,**禁双轨**;②上游 cc::Build 直打包覆盖 B2 绕过补丁,吸收后删 B2);#802 必吸 breaking+**迁移债实锤:`unsafe_jsstr_to_string` 删除→bao 106 处/10+ 文件**(node_dns/node_https/node_url/bun_inspect_api/node_http/bun_api/node_events/globals/node_cluster/node_worker_threads;3 类机械样板:raw cx→&mut 包装/NonNull 传参/safe 形替换)+`error_info_from_exception_stack` 旧形→bao_engine/context.rs:897 一处(main:1071 safe 归宿);#811 REPEAT-RISK(jsglue RustJobQueue draining 重构 vs bao_engine/job_queue.rs 317 行自研队列——对照裁定,禁双源;servo d8671305 JSContext-owned 为消费侧参照)+PATCH-CONFLICT(P6 jsglue 邻区/P5 jsapi 邻区);#812 bugfix 随 #811。

**反向冲突七项**(自研面被上游新版冲击):①P5/P6 自增 wrapper 条目须重挂 #802 后新模板体系并 diff 生成产物;②src-intl 切片对 SM153 树重裁或换轨 extracted-crates,W0b 发布面 patch(root Cargo.toml:104-107 dep-key 规则)扩展到新 crate 族;③job_queue.rs draining 语义裁定;④rust.rs 自增块对 #802 -230 重排做**语义锚**重放,P3 packed-struct 字段偏移 SAFETY 断言对 SM153 bindgen 输出复验;⑤B2 吸收后删除;⑥P2 PROCESS_ENGINE_OUTSTANDING/process_handle 与上游新 `ENGINE_STATE: Mutex<EngineState>`(含 InitFailed 臂,:156-211)合流——P2 风险 Low→**Low-Med**;⑦etc/COMMIT+get_mozjs.py 物化管线对 153 release tag 资产布局(内嵌 irregexp/patches 层)验证。

**patch 重放风险表**:P1 EBUSY Low(SM153 换 TRY_CALL_PTHREADS 锚)/P2 Low-Med(InitFailed 合流)/P3 Low-Med(上游 setter 已删需重加+偏移复验)/P6 Medium(jsglue 邻区被 #811 重写)/B1 B2 Low(B2 疑可删);**P4/P5 High**(SM153 仍存活危险点——main:155 无守卫 activation 链——但锚文件跨大版本重构)。

**Round0 前置判据(实锤)**:`vendor/mozjs/mozjs-sys/mozjs/js/` **零文件入 git**(git ls-files=0),etc/patches 无 BAO 自有条目 → P4/P5 C++ patch 真源不在版本库;三源捕获序:git 历史(`bb313dd1` vendoring 直改源码原始 commit,已验含 BaselineFrame 路径)> /var/cargo-builds 物化树 > crates.io .crate。

### V 抽验(主会话独立复核,2026-09-21)

`command grep -rn unsafe_jsstr_to_string src/ --include=*.rs | wc -l` = **106 精确命中**(probe 数字属实);`git ls-files vendor/mozjs/mozjs-sys/mozjs/js/` = **0**(未追踪证实);`git log --all -- '**/jit/BaselineFrame*'` = `bb313dd1`(2026-06 vendoring,含 BCE-002/004 直改)在历史——**源①可行,Round0 无阻断**。

### 轮次规划(4-6 有效轮预估,并入 106 处机械迁移)

- **R0**:P4/P5 真源捕获→committed `etc/patches/bao-000{1,2}.patch`(`git apply --check` 验证)(er0 在途)
- **R1**:SM153 源导入(#803)+get_mozjs.py/COMMIT 对 153 tag 物化验证+8 patch 对账表(vendor 21 文件↔上游 25)
- **R2**:patch 重放(P1-P6+B1/B2 逐项,P2 与 EngineState 合流、B2 吸收 cc::Build 后删、P5 wrapper 重挂新模板)+ #801/#802/#811/#812 连吸 + **106 处 unsafe_jsstr_to_string 机械迁移**(编译错误驱动)+ context.rs:897 safe 归宿
- **R3**:bao_engine JobQueue 迁移裁定(#811 对照,禁双源)+ extracted-crates vs src-intl 换轨裁定 + W0b 发布面
- **R4**:extracted crate bao- 改名发布 + servo 重钉 ^0.26
- **R5-R6**:7 oracle 再证 + 全量 build/test + CLAUDE.md 版本漂移修正(实际 0.22.1/140.14.0-1 vs 文档 0.22.0/140.14.0-0;etc/COMMIT=ee9f2b2 是 release tag 非 main commit)+ patch 清单表更新(含 exdr2 的 EncodeStencil 条目)

并行在途:exdr2(EncodeStencil 绑定,REQ-ENG-012,XDR encode bindgen 缺口——本账本 09-17 已预锚)。er0 出果即启 R1。

### R0 完成(2026-09-21,b9c47230 + 8658fccc 收口)

**probe「零追踪」前提被推翻**:probe 查的是 W2 拆分前旧路径 `vendor/mozjs/mozjs-sys/mozjs/js/`(确零追踪);真身 `vendor/mozjs/src-js/mozjs/js/src/`(2207 文件)全程 git 追踪。四源全活:git 现树(决定性)/ /var/cargo-builds 物化树(md5 全等)/ crates.io bao-mozjs-src-js-140.14.0-0(字节一致)/ .in.rs Rust 侧。上游干净基线 = servo/mozjs Release `mozjs-source-ee9f2b2…` mozjs.tar.xz(根=mozjs-140.14.0/,FIREFOX_140_14_0esr_RELEASE;etc/COMMIT 的 ee9f2b2 是 mozilla release 标记,vendor servo 基底是 eb36274)。

交付:`vendor/mozjs/mozjs-sys/etc/patches/bao-000{1,2}.patch`(P4/P5,-p1 SM 树相对,`bao-` 前缀排序自动落 0002-0045 后)。**验证:apply --check 双 rc=0(pristine+0044 基线)+ 重放字节全等(pristine+0044+bao-0001+bao-0002 → 三文件 md5 ≡ 在产 src-js 树)**。主波情报三条:P5 jsapi.cpp hunk 须在 0044 后(非 BAO hunk 已剥离,旧侧行号=post-0044 态);namespace-JS-out 陷阱固化进 0002 头部 CAUTION;P4 语义跨版本恒定(jsapi.cpp 恒 2332 行)。

R1-prep 已续派 er0(SM153 资产物化 dry-run+双向对账表+迁移 API 核查,零工作树变更,/tmp/r1-prep/)。

### R1-prep 完成(2026-09-21,er0,/tmp/r1-prep/dossier.md;V 独立复核通过)

**资产**:SM153 = Release `mozjs-source-3b49a449…`(FIREFOX_153_3_0esr_RELEASE,2026-09-13),tarball 238MB 根 `mozjs-153.3.0/`,物化 /tmp/r1-prep/sm153 + 上游 24 patch 全文 + 6 crate 文件 + 工具链三件 + pristine scratch。**V 复核:bao-0001 与 bao-0002 对 153 pristine apply --check 双 rc=0(C 独立重跑,0002 连 0044 前置都不需要——比 er0 声明更强)。**

**表 A(上游 patch 对账)**:11 同 / 5 漂移(0016/0029/0033/0036/0043,R2 取 153 版)/ 删 3(0031/0040/0041)/ 0032 重锚 / 0045 换血(cbindgen/icu 供头新义务)/ 新增 6(0046-0051)。**★0046-AddServoSizeOf 与 BAO P6 BaoCollectRuntimeStats 同面——R2 合流裁定,禁双源(新裁决点)。**

**表 B(BAO 8 面对 153)**:P4/P5 零 re-base(rc=0);P1 锚存活需按新宏形态重推(TRY_CALL_PTHREADS,Mutex_posix.cpp:17-30/84-88);P2 存活移位(JSEngineError::AlreadyInitialized rust.rs:172/Err:207,上游无恢复路径);P3 Rust 锚消失、C++ 存活(CompileOptions.h:702)→ 重挂 wrapper;P6 重挂+0046 裁定;B1 重放;**B2 上游已无(0 hit,证实删除裁定)**。

**迁移 API(行号)**:&mut JSContext 已在(rust.rs:1659/1763);unsafe_jsstr_to_string 未删已 deprecated,替代 `jsstr_to_string(&JSContext)`(conversions.rs:615)——106 处迁移目标;error_info_from_exception_stack_safe 在(rust.rs:1101)。

**工具链**:get_mozjs.py 零变化(etc/COMMIT→3b49a449);update.py 88→314L 大改写(extract_rust_crate 轨);filters +4 排除 +icu_capi −Jinja2;**#803 要求新 clang/NDK——R1 实做前门:frog-build:ubuntu24 clang 版本核查**。R3 预锚:上游以 mozjs- 前缀发布 mozilla+patched icu crates,bao 可消费发布物免自抽(extracted-crates vs src-intl 裁定输入)。

**归一联动(用户裁决 2026-09-21)**:R1 SM153 导入落进**单一编译宇宙**——eu1(mozjs 吸收主 workspace)在 exdr2 收口后重派;导入协议禁复活 vendor workspace 根。

### R1 工具链门结果+升级批准(2026-09-21,er0)

**判定:farm 不够**——frog-build:ubuntu24 实测 clang 18.1.3(apt 浮动,ubuntu noble 上限 18.x)vs **SM153 硬门 ≥19.0**(toolchain.configure:1494-1499 FatalCheckError,非 warning)。NDK 缺失出域(android/ohos 腿已裁,#43 终裁;平台矩阵启用时再补)。升级已批执行:apt.llvm.org noble + clang-20/libclang-20-dev + alternatives,镜像钉版消浮动;一次成本=sccache 失效+frog-target 首轮全重编(预告)。证据可复算:ssh 16.18.0.1 docker run frog-build:ubuntu24 clang --version。

### REQ-ENG-012 stage1 完成(2026-09-21,exdr2,commit 6f9c6a01)

EncodeStencil XDR 绑定落地(5 文件 ~40 行净增,全 vendor/mozjs):根因=build.rs blacklist_fn 显式排除 `JS::EncodeStencil`(上游 bb313dd1 进场,动机=第三参 `TranscodeBuffer&`(mozilla::Vector<u8>)bindgen 降级为 u8 无构造面);解法=un-blacklist(保留真 C++ mangled link_name,ABI 正确)+ jsglue 四 shim(Create/Destroy/Begin/Length,buffer C++ 侧生死 Rust 持 opaque 句柄,SetBuildId 先例)。V:生成 jsapi.rs link_name 逐字节对齐实证 + smoke 独立复跑 1/1(真 parse→encode×2 字节确定性→独立堆副本 decode→双 fresh realm 实例化执行→完成值等价)。CLAUDE.md patch 清单第 7 项落盘(6→7 项)。

**stage2 输入(挂 #26)**:缓存键候选 `GetScriptTranscodingBuildId`/`GetOptimizedEncodingBuildId`(同 Vector 降级病同 shim 可治,BuildId.h:77/:58);EMBEDDER CONTRACT=encode 前必装 SetProcessBuildIdOp(否则 StencilXdr.cpp:1373 空函数指针 SIGSEGV,gdb 实证)。

**上游 issue(禁自修)**:草稿 /tmp/issue-servo-mozjs.md(140.14+153.3 双版本同缺陷实证+fail-closed 建议修法)——**提交被 fine-grained PAT 仓库范围阻断**(servo/mozjs 出域),待用户扩 PAT 或手工提交。wrap! 宏零参缺陷(bao 侧)记录在案,下次新增零参 glue 绑定时绕行。

### extracted-crates 裁定(2026-09-21,案 A 采纳;er0 案卷 /tmp/ev4-crates-dossier.md)

**probe REPEAT-RISK① 实测推翻**:"8 crate vs 自研卫星同题两解"不成立——**互补两层**:①我方三卫星(src-js/src-intl/src-python)=SM 主树切片,上游无发布物(tarball 形态),10MiB 裁决继续自发布;②上游 153 新 Rust 依赖面(实测 **12 件**非 8:5 glue+7 patched-icu/iter,crates.io API 逐件实查存在,newest=153.0.0)我方零存在。**裁定案 A**:mozjs-sys 消费上游 `mozjs_*="=153.0.0"`(feature 门照抄;五 glue 经 DEP_*_GLUE_INCLUDE links 元数据流供头,registry 形态等价;patch 面与 glue 来源解耦);案 B(自抽自发布)否决——违"上游只用最新"+平行真源。动作已入 em1 合同(12 deps 行[eu1 边后]+build.rs DEP_* 段[:220-235 形态]+filters 取 153 版)。

### SM153 前移终报收口(2026-09-21,em1,V 过)

**10-commit 串全落**,cargo 面达成:**stencil_xdr smoke 绿(C 独立复跑 1/1 RC=0——REQ-ENG-012 XDR 面在 153 存活)**。实构建揪出 4 个 dry-run 不可见缺陷并根治:①bao-0001 语义重锚(153 initForOsr void 化无 bail 通道→脚本入口 pc 兜底形态,roundtrip 字节验证)②bindgen+libclang23 吐不出 MicroTask 块(与上游预构建产物对照定案,显式加 header 修)③链接器剪 no_mangle-only glue rlib(5 个被剪 18 undefined——extern-crate 保边)④NonNull 限定。绿面:bao_engine context.rs:897/bun_runtime 106-face/bao_workflow_host/bao-mozjs 全绿。

**遗留面(已续派 em1)**:bun_sm moduleloading 迁移(15 错 4 类,语义级):FinishLoadingDynamicImportedModule 新签名/SetModuleLoadHook 重实现/动态导入 hook 无等价需设计/forceUTC_ 删除需行为平机制——合同判据=行为平,不可达即 STOP 转用户裁决。

### 案 A 形态终裁修正(2026-09-21,em1 实证)

registry 形态否决(当下):crates.io mozjs_* 实测 max=**153.0.0**(er0"12 件全在"属实但版本对齐缺口漏检)——registry 喂 153.3.0 源=跨版本组合(icu_collator rust 面内容差+icudt77l→78l 实测)。**维持 in-tree path deps(=153.3.0 tarball 同源提取,上游 314L 原代码)**;registry=发布终态,触发器=上游发布 153.3.x era(届时 12 行机械切换);publish 波 registry 前置(bao 自发布或等上游)记为发布波裁决点。extracted-crates 体量纠正:12M/200 文件(前记 26G 有误)。P4 第 4 行语义更新入 CLAUDE.md。

### bun_sm moduleloading 落地(b5b3fa96)+ JobQueue 线内联收口(2026-09-21)

四类迁移全落(spec HostLoadImportedModule 统一面:FinishLoadingImportedModule payload 路由静图/动 promise 保 TLA 语义;ResolveHook→LoadHook 原样外包;动态导入无等价项→LoadHook 承载+引擎 ContinueDynamicImport,**删 ~28KB SM140 机器**;forceUTC_→setTimeZoneOverride(Atlantic/Reykjavik=引擎真映射,jsglue 新 shim BaoSetRealmTimeZoneOverride))。**#811/#812 JobQueue 迁移同 commit 内联完成**(153.3 traps 双源定点排空+interrupt token 栈)。bun_sm check 0 错(15/15 清),smoke 绿。**末枚(裁定已发)**:realm 时间精度——SetTimeResolutionUsec 全删,裁=全 realm clamp 平价(RTPCallback+两处 token 接线;per-realm 门控=行为变更禁自行接受);runtime_bridge.rs 所有权转移 em1。落即全树 build+波门§①②。

### 收波+发布序列(用户裁决 2026-09-21"全部做完最后要验收,通过验收后记得发布")

序列:em1 末枚(realm clamp 平价)→ 全树 build 绿 → eb2s1 三组补面(自动)→ **波门§①-§⑤ 齐绿=验收** → **发布闭包**。

发布波骨架(预立,门绿即发):
1. **extracted-crates 12 件自发布**(bao-mozjs-sys 的 path deps 是发布阻断;crates.io mozjs_* 名属 servo 不可代发;上游 153.3.x 未发):**bao- fork 改名发布**(W3 stylo 先例+bao-fork-rename-rule:package=bao-mozjs-unicode-bidi-ffi 等,dep key 保 mozjs_* 原名→DEP_*_GLUE_INCLUDE links 流不变),version=153.3.0 系
2. mozjs 族:bao-mozjs 0.24.0/bao-mozjs-sys 153.3.0-0(deps 指 bao- 发布物)/三卫星 153.3.0-0
3. 消费链 topo 序:bun_sm → bao_engine → bun_runtime(106-face)→ bao_workflow_host → bao_browser(D2/carrier-A)→ bao_cdp_client(D5/D3)→ 其余受 req 波及 crate
4. face-transition 纪律全适用(breaking=minor bump 非_patch_;strip-publish-restore 舞步;发布前逐 crate 新鲜复核)

### 发布预备案卷(er0,/tmp+/.plans/publish-prep-20260921.md)

61 commits 聚类:breaking=bao-mozjs 族(SM153 代际)+bun_sm/bao_engine/bun_runtime/bao-browser(mozjs req 泄漏)/bao_cdp_client;internal=workflow_host/servo 族(零版本漂移)/boringssl。版本面:bao-mozjs 0.24.0(已预跳)/sys+三卫星 153.3.0-0/bun_sm+engine=0.3.0/runtime=0.4.0/browser+cdp_client=0.3.0/workflow_host=0.2.1/bao-core 伞 0.3.0。**topo 双修正(实测边)**:workflow_host 先于 bun_runtime、bao_cdp_client 先于 bao-browser;序=12 extracted(bao- 改名,key 保 mozjs_* 供 DEP_*)→3 卫星→sys→mozjs→bun_sm→engine→workflow_host→runtime→cdp_client→browser→伞→closure 扩集。**阻断面=零**:token 在位、12 bao-mozjs-* 名全 FREE(逐 API 实查)、same-name 被 servo 占(改名=唯一自主路径)。执行要点:改名舞步+cbindgen build-dep registry 可解析+closure 机械扫(58-crate loop 先例)。

### realm 时间精度平价落地(de741bbd)+ servo 消费面末班合同

clamp 回调(SM153 RTPCallback 面,µs floor 网格无 jitter,arm/disarm 原子周期平价)+双 token 接线(node=context.rs stamp 方向修正;page=新 jsglue shim 绕 incomplete-type)+两 shim。**深层缺陷根治**:SM153 ModuleLink 拒 status=New——直连 Compile→Link 站点须先 JS::LoadRequestedModules(moduleloading 面隐藏半边)。bun_sm 14/14(5 败转绿)+新 clamp 回归测试绿;engine/runtime/browser/bun_sm check 全绿。**全树最后残红已派**:bao-servo-script 29 错 14 站点+bindings 4 错(servo 自有 hooks/JobQueueTraps/principals/error bool 面;bao_engine 模式可复用)——落即全树 build→补面→五门→发布。

### em1→em2 交接(上下文耗尽,2026-09-21 深夜)

em1 五小时弧线交付 SM153 前移全线(树/patch/build/jsglue/wrapper/update.py/106-face/req/106-face/moduleloading/JobQueue/realm clamp/ModuleLink 深修/bindings 4 错/servo 机械面七类)后上下文耗尽,交接报完整:script 余 32 错(尾部导入/残留引用 3-4 迭代+error.rs×4 POD shim+ModuleType 臂×2+模块深水面三 hook→LoadHook+动态续延,草图与 PR #47489 参照在案)。em2 已派(树态续作);落即全树 build→收波链。em1 転 idle,不再派新。

### #47 收口(d4dbb39b,波内并入)

build.rs 子进程错误传播根治:4 硬化(make 全形 cmd+status+stdout/stderr 尾 20 行/strip cmd+status/attestation verify——顺修 gh 非零静默通过的 fail-open(Strict 名不副实)/curl)+3 软探针保留(可用性探测带回退,失败=合法路径);assert 2→0 实证;tail_lines 自包含。类型/运行验证挂波末实构建(build.rs 指纹失效会触发 make 全周期,避免与 em2 迭代互扰)。**#47 关单评论待发布后带证据执行(§12A:引用波门+发布版本)**。

### ★全树绿 + 收波链点火(2026-09-21)

em2 终报:两 commit(d5d82b6d engine 面 15 文件/bc675a71 module loading 3 文件),script 32 错→0,**cargo check --workspace rc=0(476 crate 零 err)**;深水面上游 PR #47489 全量形态移植(自创循环被替换);零新增文件;error.rs 走 fork 自带 borrowed_error_report(零新 shim)。CLAUDE.md servo 清单 31 条目落(346dd857)。收波链执行中:eb2s1 三组补面(绿窗宣告即跑)+波门§②(后台)/§① 排队——3/5 已绿。全绿→发布闭包(预案零阻断,topo 双修正版)。

### ★★验收全绿(2026-09-21,波门+补面)

**波门五节全 PASS**:§① B2(cdp_client 全 suite+browser scoped)/§② mozjs(checks+smoke+engine 377/0)/§③ 单宇宙(全深度断言)/§④ parity(153 基线 apply-check+7 行清单)/§⑤ publish(package 零网络)。nextest 缺失走文档化降级(WARN 非 FAIL)。**补面三组 RC=0**(sw_stealth_profile/opaque_origin_startup/lib event_tx——eb2s1 后台证据)。过程揪出并根治:测试面 153 迁移盲区(check 不编 test 目标,01f9d259)+OnceLock 跨测试污染(T2 自违规,2f9a0655 裁 A+结构延伸追认:契约测试迁 lib #[cfg(test)] 隔离进程)。**发布闭包已点火**(s6-release,案卷 topo 序 12 级)。

### ★★★波终态(2026-09-21,daily-20260921 tag)

**发布闭包 34/34**(s6,C 独立 V 抽验断点尾三版 runtime 0.4.0/browser 0.3.0/core 0.3.0 全在):12 extracted(bao- 改名首发)+mozjs 族(sys/卫星 153.3.0-0,mozjs 0.24.0)+src 链(0.3/0.4 面)+closure ×10(links 冲突强制:cdp/stealth/servo 族 8)。消费端冒烟:cargo add bao-core@0.3.0 全图 4161 节点 resolve RC=0,单实例断言全过,零 SM140 残影。strip-publish-restore ×5;HEAD=origin=6ceeddf2。tag **daily-20260921** 已推。**#47 关单**(§12A:波门 5/5 GREEN 引用+153.3.0-0 版本证据);#26 stage1 评论挂 stage2 在案。

## 新波:Windows 交叉编译 + 全量并行(用户裁决 2026-09-21 收波后)

**裁决原文**:「全量并行突进同时增加对WINDOWS交叉编译的支持与验证」。立项=用户书面裁决+#18 Platform/Build 板(REQ-DEPLOY 域,musl p3.5 先例)。

**并行四线**:①ew1-win-probe(探测:153 configure winreg 门复核[#37 波 140 时代实证 configure 层不可能]→路由案 xwin-msvc/mingw-gnu/win-host→破口清单→验证口径)②er0(farm 双路由分层:mingw-w64 apt+xwin SDK 独立卷 farm-xwin+双 rustup target,clang-20 已带 clang-cl/lld-link)③exdr2(REQ-ENG-012 stage2:XDR 持久缓存本体,C1-C4 框架)④eb2s1(观测缺口微单:11 站点 Full 节流日志)。修复线挂探测 DAG 边。

### 用户情报修正(Windows 阻断根因):libuv 供给路径移植未完

用户原话:"上游 libuv 供应路径没移植完,我们需要自己维护并移植完**或者看看最新版上游做完了没**"。盘面实证:src/libuv_sys 在树(FFI 起步)+bao_uloop 纯 epoll 形(零 cfg(windows) 实证)——uv 消费面 Windows 分支缺失=首要根因,mozjs configure winreg 门为并行阻断之一。新增 ew3-uv-gap 线(fetch 上游 origin/main 核最新完成度+我方逐功能缺口+搬运/自维护归属);ew1 增补该情报(破口清单必含 uv 层)。

### ew3 案卷结论:上游做完了(声明侧),bao 缺 C 供给(2026-09-21,/tmp+.plans/uv-gap-CASE.md)

上游 origin/main=4af1842c8c:src/ 零 .zig,libuv 供给路径声明侧全 Rust(libuv_sys 3132L/108 extern+open_handles 228L);Windows x64/arm64 官方支持;C 侧=真 libuv fork @8023581113(1.51.0,37 .c)+2 win patch。**bao 已吸收声明链(125 cfg(windows) 文件,同版本镜像),真缺=C 符号供给零**(libuv_sys 纯声明;bun_uws_sys build.rs:101 exit(1)=#34;bao_uloop compile_error!=#35;mimalloc MSVC=#33)。路线三合同(C 侧零手写):①ew4 已派(vendor+cc 编译关 #34)②臂解锁③FFI 再同步(276→108 漂移,bao 独有 168=上游已删)。**最大边界**:mozjs configure 拒 Linux host(140 实证,ew1 复核 153 中)——Windows 全构建可能需 Windows host。文档漂移:platform-support.md 提 bao_uloop kqueue.rs 但树无此文件(mac 口径另波核实)。

### Windows host 批准(用户裁决 2026-09-21"可以给 WINDOWS HOST,继续")

最大边界(mozjs 需 win host)解除,全构建路径开。farm 实测:KVM 硬件在/96G 闲/虚拟化栈未装/无 ISO——VM 路线差两步(装栈可做+**ISO/许可证待用户**)。ewe-win-host-design 已派(host 无关设计五节:环境规格钉版/mozjs host 门全链/农场节点集成/交叉-vs-host 分工矩阵/分阶段落地序)。七线并行:ew1/er0/ew4/exdr2/eb2s1/ewe 设计+host 来源问询(用户 AFK 待复:farm VM vs 现成机 vs 云)。

### Windows 波推进:ew4 合同 1/3 落地+bun_alloc 臂派修(2026-09-21)

ew4(9595dbae):libuv C 符号供给——vendor 真 fork 头(1.51.1-dev rev 验证)+2 patch 重放+build.rs 仅 windows 激活(37 .c 与上游数组逐一核对,linux 零发射,links=uv)+flags 翻译表(clang-cl 全接受);linux 双绿;windows 面两外部位等待(SDK 头=xwin/农场接合;bun_alloc msvc 臂=全 cargo 图阻断)。#34 状态评论已发未关(残面:消费臂解锁=合同 2/3)。**s1a 已派修 bun_alloc 3×E0425**(今晨 #45 波遗留,修完=msvc 面 cargo 图解锁首枚)。两处过期文档真源待后续合同(uws_sys 门消息文本/platform-support §2§4)。

### ★ew1 终报:路由 A 判决,SM153 交叉三层绿(2026-09-21,/tmp+.plans/win-probe-DOSSIER.md)

**headline**:Linux→x86_64-pc-windows-msvc 的 configure/全量 C++(655 obj)/js_static.lib(342MB)三层全实测绿——140 时代"configure 不可能"判死;winreg 门 env 100% 绕过零 patch;配方=xwin winsysroot+clang-cl/lld-link shims+**cross 必须 --enable-libz-rs**(host zlib 泄漏防线)。验证口径 L1-L4(链接绿=支持;Wine smoke=stretch 非承诺)。**DAG 执行中**:W1 alloc(s1a 在途)/W2 boringssl win64(em2 已派)/合同 2/3 臂解锁(ew4 已派,用 /tmp/win-probe 实测 sysroot)/W0 farm(er0 在途);W3 mozjs cargo face+W6 fonts/servo 挂 W1;W7 全链=W1-W6 齐后。#37 判定修订与 platform-support §2 修订挂文档合同。配方已沉淀(windows-cross-probe-recipe 记忆)。

### ewe host 设计案卷(2026-09-21,/tmp+.plans/win-host-CASE.md)

五节:①环境规格(bootstrap.ps1=§8 Windows 等价形:rust nightly 钉/MSVC Build Tools 含 ATL/Python 3.11/moztools 4.0 钉版 zip/GStreamer 1.22.8 **必需**(media-gstreamer feature)/LLVM 建议 21.1.8)②mozjs host 门(find_moztools tier-1/2 供给;硬约束:makefile.cargo bash 语法钉 msys2 sh 禁裸 make;should_build_from_source 恒真)③农场集成(C:\build-farm\bao 布局;git fetch+reset 同步走 16.18.0.1 bare 镜像免凭据扩散;bao-win-remote-build 五段镜像 frog)④分工矩阵(**校正:ew1 终判推翻 mozjs-需-host 旧据,交叉可承载全树待 W3 证实;host=运行时验证+备胎**——校正令已发 ewe)⑤落地序 W0-W5。风险显式(boringssl win asm 旗标/GStreamer pkg-config/UCRT servicing)。

### ew1 增补:#34 供给侧实证闭合+新破口 lsquic(2026-09-21)

**uv.lib 独立复现**:cargo check --target msvc+xwin 七段 INCLUDE+AR=llvm-lib → 2,093,250B/**685 个 uv_* 符号**——用户根因"libuv 供给未完"的供给侧已毕(ew4 9595dbae 与本探针互证);残余=消费侧(uws 开闸+链接,ew4 2/3 在途)。预测序回执:uws/uloop=PASS-by-design;mimalloc 门已随 #45 亡(真缺口=bun_alloc);声明线全 BLOCKED-BY bun_alloc(W1 后回归探针)。**新破口:bun_lsquic_sys msvc exit(1)**(QUIC/H3 C 层)→ W2.5 已派 ec2(上游配方先查;无配方=vendor-vs-砍面报 C 裁)。**W1.5**(xwin env 契约固化 farm,双 lib 依赖)已增补 er0。DAG 改版入案卷(§③A 新增)。

### ewe 案卷 v2 终态(实靶+ew1 校准,231 行)

§3 实靶 .200(SSH 唯一控制面,SMB 备选大产物;**授权=W0 阻塞前置**——用户授权公钥前 host 面不可开跑);§4 矩阵双态(交叉承载全构建[条件性];host=运行时验证真机+备胎,**Supported 实证链=交叉构建证明+真机运行证明缺一不可**);§5 W0 授权→W1 交叉收敛(合流 ew1,含 bcm/bun_alloc 破口)→W2 交叉全构建门→W3 真机运行验证→W4 host 备胎(并行)→W5 收口;§6 风险+4 行(xwin 钉版双口径裁主钉=ew1 实测 0.10)。

### Windows 波:W1 链进+W2+W2.5 三收(2026-09-21)

**s1a 6866e835**:bun_core 三 windows 臂(GetCurrentThreadStackLimits/_flushall[证伪注记]/S_IEXEC)+import cfg;msvc RC=0,linux 52/52;**自由解锁 bun_safety/paths/collections**;移交下一堵点=libuv C build script 需 SDK sysroot(并纠晨间误读:build script 在依赖后跑,C 面此前未触)。**em2 c3a5800f(W2)**:boringssl 双面 build.rs(msvc=clang-cl+MSVC 原文 flag+NOMINMAX 实证;asm 走显式 nasm win64 非默认 ml64);**asm 零再生**(win64n 产物 W0b publish 已在树,对上游 cmake 清单全中);win64 boringssl.lib 6.68MB COFF 实证,linux 零 diff。**ec2 b54d6e97(W2.5)**:lsquic 上游配方存在(lsquic.ts cfg.windows)→windows 臂+5 vendor shim;msvc 绿(lsquic.lib 4.8MB/572 符号);**证据否决 zlib 供给**(IETF-only 集零 zlib 引用,探针 72 TU 实证,已加的 dep 撤回)。**ew1 续派**:env 契约产品化(wsroot 稳定位+source 脚本+端到端)——C 三层(libuv/boringssl/lsquic)供给全闭环,cargo 面等 env 产品化后链式再进。

### ew4 合同 2/3 交付+usockets 吸收波立项(d3b51836,2026-09-21)

uws_sys windows 臂结构性落地(LIBUS_USE_LIBUV 真编译路径,uv.h 直连 libuv_sys vendor 零 skew,zlib/libdeflate win 供给)+bao_uloop compile_error! 解除(windows FilePoll→bun_io::windows_event_loop 上游形);linux 回归绿;platform-support 三行+kqueue 死引用顺更;#34#35 状态评论已发(关单归 C)。**cargo windows 绿 stop**:路线 B(3 文件最小换)实测死路;路线 A 需 usockets@4af 全吸收(拖 openssl.c 2188 行分叉+TLS 签名演化,触 linux 生产 TLS 面)。**裁定:吸收立项**——em3 已派(上游基底+bao patch 三值对账+TLS 签名迁移+linux TLS 回归硬门+windows 验收=ew4 门零改动过)。附带:win-cross-env.sh(ew1 在产)/uv.h 旧镜像登记 FFI 和解合同域。

### W1.5 env 契约产品化(03c1f671,2026-09-21)

wsroot=/opt/bao-win-cross(634M 全断言过+无空格 symlink);`scripts/win-cross-env.sh`(偏离 tools/ 声明采纳:scripts/=仓库惯例[clang-musl 先例]);端到端:lsquic/boringssl/libuv 三 C 层 msvc check RC=0;bun_alloc 已在树确认=Layer-0 消。s1a 链式续进(crash_handler/zlib→下游+声明线回归探针条件成立);em3 路径修正已达。

### s1a 链式完成:bun_sys 解锁,全图收敛单点(656e9a77)

O_CLOEXEC→O_NOINHERIT 每平台旗标+bun_sys 死 import 清;libuv_sys 经 env 脚本确认绿;声明线探针:libuv_sys/sys 绿,io/event_loop/zlib/crash_handler 四 crate 均恰剩 uws build script 一错(传递性)。**Windows 全图收敛单点=usockets 吸收(em3 在途)**——其过则四 crate 连锁解封。msvc 已解锁:alloc/core/safety/paths/collections/sys/libuv_sys。

### W0 farm 双路由分层完成(er0,0f426af+5d20e75,2026-09-21)

mingw(gcc-mingw-w64 钉版)+xwin 0.10.0(sha256 双钉,--accept-license 显式;farm-xwin 卷:splat 630M 真件在位,§8 分层=MSVC 内容全在卷零混装)+双 rustup target 入共享工具链卷;首建揭出 apt.llvm.org 拆包缺件(clang-cl wrapper/lld)→lld-20+符号链修复;四件套实测(clang-cl 20.1.8/lld-link/mingw-gcc 13/xwin);正本双 commit,服务器 diff 清零。**Windows 波 W 面盘点:W0 farm✅+host✅(.200 docker)/W1 链✅至 uws 门/W1.5 env✅/W2✅/W2.5✅/W4 供给✅+臂 2/3(等 em3)/W5✅(同门)。唯一关键路径=em3 usockets 吸收。**

### er0 W1.5 增补:farm 卷收敛脚本契约+链接冒烟绿(d9f5b9f)

farm-xwin 卷布局收敛到 /opt/bao-win-cross 契约形(wsroot 组装+无空格别名+shims 固化);波折如实:首组装 DNS 丢包被无 pipefail 掩蔽→完整性实检抓出→显式 rc 重跑(教训:下载管道必 pipefail);**容器内冒烟:clang-cl COMPILE_OK+lld-link LINK_OK(smoke.exe)=SDK include+LIB 双层完整,farm 具备交叉编译+链接双能力(W7 面就绪)**。固化链:镜像工具→卷 wsroot→仓库脚本 source 即全套。

### ★em3 usockets 吸收波全绿:uws 门开,全图解锁(5cf54648/fc34baa8/4396a20e,2026-09-21)

基底 bun@0ba403277a 逐 hunk 对 4af:全部 bao cherry-pick 被上游逐字包含/超越,**真冲突=0**(仅 4 小 patch 重放:Bun__panic 链接钉/us_cert_string_t 类型钉/分配器回 libc[mimalloc 退役连带堵暗雷]/lazy-cert eager+IGNORE_EXPIRED shim);保留 quic.c(lsquic 锁)与 root_certs 五文件(boringssl 锁)延后立案。签名迁移全清单(adopt_tls 7→10/SNI 2→4/group_connect 源绑定[**真野指针崩溃修复 context.c:612**]/297 extern 机械比对零漂移)。**三门**:linux check(uws+6 下游)RC=0/linux TLS 硬门全绿(SNI 6/6 C 复验)/windows ew4 收窄门 RC=0(C 复验 0.54s)。#34#35 证据评论已补(两案 09-08 已关,非本波关)。遗留登记:quic.c 4af 增量+lazy-cert+真机 link/E2E(#18)。**s1a 门开令已发**(四 crate 连锁解封推进)。

### ★.200 compose 测试环境建成(er0,2026-09-22)

~/build-farm/bao-win:Dockerfile(farm 正本 msvc 裁剪:llvm-20 全符号面+rustup 含 msvc target)+compose 双 bind 卷(win-cross=/opt/bao-win-cross,src=契约脚本);wsroot 634M 自 .205 rsync(勘误:um/x64 实为 952 .lib,此前 0 计数系漏版本段路径);**根治:.200 docker CLI 因 DD 残留(~/.docker desktop 上下文)全体崩→隔离 dd-stale.bak+净 config 复活**;容器冒烟:env ready(7 INCLUDE/4 LIB)+COMPILE_OK+LINK_OK(rust msvc target+clang-cl 20.1.8)。ops 册已追加环境节。**用户点名的"SSH 到 WINDOWS 的 compose 做环境去测试"就绪**——候 W3/W7 产物即真机冒烟。

### ★W8 交叉测试运行器建成(er0,2026-09-22)

.200 ~/build-farm/bao-win/bin/win-test-runner.sh:exe 列表→NTFS 放置→interop 执行(timeout 180s)→libtest 解析→汇总;**双证防假绿**(rc==0 且 test result: ok 才 PASS,双向实证:故意败判 FAIL/真过判 PASS)。**关键配方入册**:交叉测试 exe=静态 CRT(+crt-static 零 DLL)+rust-lld+三段 /LIBPATH(vc14+um+ucrt);libtest 输出跨平台同形;--exact 需全路径名;interop=WSL 特性故 runner 必 WSL 侧(容器编译+WSL 执行编排);WSL-fs binfmt 直执行亦实证可用。**Windows 验证基建至此完备:编译三面(env/farm/.200 容器)+测试一面(runner)。候 W3/W7 产物即 T1 单测波开跑。**

### ★W3 根因破案(C 亲测 12 轮 bisect,2026-09-22)

**libclang 23(Linux 宿主)对 cl-driver 旗标面 TU 创建组合性硬死**(bindgen context.rs:562;单剔除 cl 旗标/-D/driver-mode/-FI/confdefs/-imsvc 均不救);**GNU 拼写面(bindgen 解析无需 cl 语义)实测完全可用**:M11 配方 rc=0 产 4.3MB 绑定(-x c++ -std=gnu++20 -fms-compatibility/-fms-extensions + confdefs 先序 -include + TU -include + XP_WIN 显式 + js 双 -I + SDK 五段 -isystem)。顺序敏感:confdefs 必须先于 TU(XP_WIN 迟到=Unsupported OS 崩)。解方已交 s1a 落 build.rs(msvc 臂弃 compiler.args 传播,构造 GNU 面;其 -FI 中间方案被取代);C 停止碰此文件避双写。教训:依赖 libclang 的面,旗标拼写按 libclang 方言(GNU)而非编译器 driver 方言(cl)。

### ★★W3 绿(b1c45895,C 独立 V 过,2026-09-22)

M11 GNU 面落地:msvc 臂弃 compiler.args 传播,构造 -x c++/-std=gnu++20/-fms-compatibility/-fms-extensions+confdefs 先序 -include+TU -include(JSApi;JSGlue 偏差 2 项如实:TU=自身免双包含/保留 .header 单头形)+SDK 七段自 env INCLUDE 解析 fail-loud。**判定:bao-mozjs-sys msvc check RC=0(101→0)**;linux 回归 RC=0 非流逐字节保持;C 独立 bao-mozjs RC=0(21.8s)。层 1/2(2837d978:语言旗标+-FI 顺序敏感)+层 3(M11 GNU 面)三段合围。W2 链进令已发(engine→sm→runtime→workspace)。

### W2 链终态:4 结构断点+裁决 v1 核心先通(s1a 67a15c84,2026-09-22)

W2 累计 14 crate 面解锁(watcher/sm/engine 旗舰面绿;runtime 仅被 libffi 挡)。全 workspace keep-going 实测:仅剩 2 Rust 面+12 C 域脚本失败。**四断点裁决(v1 核心先通,用户 AFK 按判据,回来可推翻)**:①media-gstreamer 按目标条件 feature(windows v1 去启;glib/gst 12 crate 栈独立后续波)②addrinfo.rs Windows 实现(GetAddrInfoW,ec2 已派)③libffi-sys 上游 bug(HOST cfg 分派错误)bao fork 根治+issue 草稿(ew4 已派)④shim.exe 工件构建时生成(s1a 已派)。三合同并发。

### W2 断点 3 闭:libffi fork(acb25b92,ew4)

[patch.crates-io] 同名 vendor fork(freetype-wrapper 先例;改名键在 [patch] 内静默忽略=已验陷阱规避);四点修法:CARGO_CFG_TARGET_ENV 运行时分派(原 HOST cfg bug)/EP 接 env 契约弃真 MSVC/.asm 落 OUT_DIR(源树污染根治)/**GAS win64.S 变体**(MASM 硬阻断实证:llvm-ml 解析不了 extern near/.seh;win64.S 同 ABI)。libffi-sys+libffi msvc RC=0;linux 回归绿;issue 草稿 /tmp(待发 tov/libffi-rs)。归属如实:bun_runtime 全量 msvc 未跑(闭包含 mozjs 大构建=W2 面)。环境注:/tmp/win-probe 被外部清空,/opt/bao-win-cross 为耐久真源。**断点计:libffi✅/addrinfo(ec2 在途)/media 门控+shim(s1a 在途)**。

### W2 断点 4+shim 闭:media 门控+b368b6d7(s1a,2026-09-22)

media-gstreamer 移 target(not(windows)) 依赖——msvc 树 glib/gst **0 命中**(12 crate 门解除),linux 树 62 命中零 diff。shim.exe 根因=include 路径差一层(build.rs 已有 0-byte 占位供给语义)→路径修正+bun_install msvc RC=0;真 PE 产出=W7 域(rust.ts no-CRT 配方已文档化)。顺带:windows 死 import×2+MaxPathExceeded 错误转换。**断点计:media✅/shim✅/libffi(ew4 升 4.2.2 在途)/addrinfo(ec2 在途)**——余二闭即 W2 workspace 全绿。

### W2 断点 3 终态:libffi 5.2 升级弃 fork(4e2f7209,ew4,2026-09-22)

升 libffi 5.2.0+sys 4.2.2(上游同根修法四点全已修=纯消费,逐点对照表在案);fork 整树删除零残差。API 唯一真破缺(middle::Arg 'argument 生命周期)→FfiNullPtr 'static 载体。**CFLAGS_<triple> 通道裁定追认**(-Wno-incompatible-pointer-types 治上游 dlmalloc clang-cl 指针严格性;零 fork,上游 bun 全局 flags 同姿态;fork 层备选在案)。env 契约部分固化 .cargo/config.toml [env](CC/CXX/AR/CFLAGS_<triple>,与 musl per-target 同形;INCLUDE/LIB 留操作方)。linux 绿+真 E2E 6/0。**W2 断点计:media✅/shim✅/libffi✅——仅余 addrinfo(ec2 在途)**。

### W2 断点 4/4 闭:addrinfo(955d680c,ec2,2026-09-22)

GetAddrInfoW 管线:平台 addrinfo 布局别名(windows=ADDRINFOA 镜像,producer/consumer cfg-blind)+W-API 直接喂 WSA 码(11004=EAI_NONAME 映射)+UTF-16 加宽(弃 ANSI 导出的 codepage 路径,IDN 字节忠实);58 错清零,msvc 绿(耐久根 /opt/bao-win-cross);linux 6/6+回归绿;ABI 对照表入模块文档。**附带发现(回归修复已派 em3)**:5cf54648 吸收丢 us_poll_ext 定义→bao_uloop linux 测试链接断(ec2 pure-HEAD 复现实证)。**W2 四断点全闭——workspace msvc 全绿门在跑**(后台)。

### W2 门首跑:尾层暴露+三组并行派工(2026-09-22)

**C/依赖门全闭后首曝 bun_runtime 自身 POSIX 面:474 错**(child_process 91/fs 68/dns 56/constants 45/os 31/tls 30/tty 27/ipc+udp+api 各 23+尾)——libc socket 族/std::os::unix/E0308,预期层(C 门挡着从未编到)。env 脚本与 config 旗标对齐先行(178c34ea:两通道 -Wno-incompatible 一致)。**三组并行**:s1a(child_process/ipc/tty)/ec2(dns/constants/os——addrinfo 模板作者)/ew4(tls/udp/api/fs/尾);统一纪律=上游 bun windows 分支为真源+行为平+对照注记。三组闭=W2 门绿。

### em3 回归修复闭(659a8d55):us_poll_ext 上游已删→本地实现+第二断点

上游对账:4af **整体删除**该 C ABI→BaoPoll 本地 16 字节头+ext 槽尾字节实现(零 C 状态);**第二断点顺修**:us_socket_from_fd 4af 增 options 槽(7 参),6 参 extern 把 ipc=1 滑进 options→SCM_RIGHTS 不布防→全链迁移。52/52 绿(含原链接遮蔽测试);windows 门保持绿。**漏检根因两条入册**:①机械比对域缺口(297-extern 只扫 uws_sys,bao_uloop 自持 extern 不在域)②验收门特征缺口(check≠链接解析)——**纪律修正:凡触碰 csrc 的吸收波,验收必含全部树内 extern 持有 crate 的 cargo test --no-run(链接面验收)**。

### W2 尾·组间调度(2026-09-22)

组 1(s1a 会话边界):ipc 23→0/tty 27→0 交付(718af5ca);child_process 95 错**架构裁定 (a) 同步桥**(linux 侧本非 libuv,(b)=windows-only 架构例外非对称双源;行为平判据 a 完整达成)→ ec3 接独立合同(交接包八类修法 verbatim,零未知缺口,~600 行)。组 3 中期:~197→~132(udp/listen/interrupt/ffi 近零;**FSWatcher 裁定 (a) 全量 ReadDirectoryChangesW 移植**——fs.watch=既有能力砍面即回退)。组 2(dns/constants/os)在途收敛。s1a 本会话 5 commit 战绩在案(W3 GNU 面/watcher/门控+shim/ipc+tty)。

### W2 尾·组 2 DONE(8a57baa7,2026-09-22)

dns/constants/os 132→0(GetAddrInfoW 管线+EAI 全表 ws2tcpip 值/constants 上游"平台所定义"语义/GetAdaptersAddresses 前缀镜像等;三文件头部上游对照注记)。linux 面逐字节不变。**协调 A(热修已令 ew4)**:node_net.rs:41 `use socklen_t;` HEAD linux 断(E0432,pure 复现)。**协调 B**:msvc 面 mozjs 依赖墙 env 组合(/opt wsroot+WINSYSROOT 单源+HOST_CC/HOST_CXX+cl shim)实证配方待入 W2 门脚本。剩余:组 1b child_process(ec3,95)/组 3 尾(fs/tls/api 等,ew4)。

### W2 尾·组 3 DONE(50973505+694f7140)+ 新热项(2026-09-22)

组 3 归属 ~197 全清:tls(WSAPoll+loopback wake pair)/interrupt(SetConsoleCtrlHandler)/**FSWatcher ReadDirectoryChangesW 全量移植**(递归 watch windows 原生 bWatchSubtree>posix 臂)/api(伪信号+TerminateProcess)/udp/net/listen/cluster/dgram/ffi 全零。GAP 登记:spawn 管道泵+cluster boot 依赖组 1 register_async_child/IPC(组 1 收尾覆盖)。**新热项(已派 s1a)**:b1c45895 引入 mozjs-sys **linux 面**回归(build.rs:276 mirror_dir NotFound,C 实证 RC=101)——挡 linux 测试与 W2 全 crate 门。**W2 余量=child_process(ec3)+此热修**。

### 热修落地+余量收敛(94c15f55,2026-09-22)

node_net linux 断已修(cfg 双臂 socklen_t+node_tls AsRawFd 漏提交两件一并;成因=毯式替换改坏自插行+两 1 行修漏在树外——组 2 pure-HEAD 复现窗口即此)。linux 现存断点余二(均已有主):mozjs-sys mirror(s1a 热修在途)/addrinfo E0133(ec2 微修已派,edition-2024 unsafe 包裹)。FSWatcher 裁定确认=已先行完成。env 组合权威=scripts/win-cross-env.sh+/opt/bao-win-cross(ew4 全程实证)。

### addrinfo E0133 闭(a16f7ffd,双面绿,C 复验)
余量=child_process(ec3)+mozjs linux mirror(s1a 热修)。

### mozjs linux 热修闭(0ebbf2a1):兄弟 checkout 陷阱(根因深于预判)

b1c45895 无辜(git show 零交集,失败先于其存在);真因=satellite `cargo:root=` 导出被共享 target 下兄弟 checkout 覆写(主树/b2s1-headwt/C 的 bisect 树三个历史 root 实证;树删→悬空)。修=四包 clean 重建+panic 自愈指引;双面 RC=0+xdr 6/6(单线程纪律)。机制入 operator memory(禁兄弟 checkout 构建此类 crate)。**余量=仅 ec3 child_process**。

### ★child_process 闭(285089dd,ec3)——W2 全部 474 理论清零,终门复跑中

八类全清(同步桥):waitpid 孪生/PeekNamedPipe+10ms 节流/cp_signal_pid(TerminateProcess,仅 KILL/TERM/INT 投递=Node parity)/伪信号表 21 名/WindowsOptions×8(**关键差异:loop_ 接 EventLoopHandle::js_current()**——default() 零值触发非空断言,from_any 无 manager)/pid 取 process_/IPC=s1a 面+UV_INHERIT_FD fd-3。crate msvc REAL_EXIT=0 全零;linux 23/23;真机注记(fd-3 _open_osfhandle round-trip)保留。bun_api 波及最小更新(exit-only 注册,GAP 注释刷新)。**W2 终门(background)跑中——绿即 W7**。

### ★★★W2 终门 GREEN(C 亲跑,2026-09-22)

`cargo check --workspace --target x86_64-pc-windows-msvc` **RC=0 零错误**——全 bao workspace(476 crate 含 mozjs/servo/全部 runtime 模块)msvc 类型面全通。**编译面 type-check 完结。W7 链接面(bao.exe)已发车**(cargo build -p bao_bin --target msvc,后台)→ 绿即 scp .200 真机测试(W8 runner+T1/T2)。用户裁决的 Windows 大计划:编译✅(check 级)→链接(在途)→真机测试(就绪)。

### W7 链接差三 .lib(2026-09-22)

lld-link 已钉(config [target] linker);全 rlib 编完,链接报:archive.lib/cares.lib(bun_libarchive/bun_cares_sys 的 out 零 .lib=build.rs 无 msvc 交叉产 lib 面,同 C 四先例模式)+Dbghelp.lib(SDK 大小写:请求 Dbghelp vs 文件 DbgHelp,Linux lld-link 大小写敏感;LIB env 已被读——kernel32 等过了)。终链合同已派 ew4(casefix symlink+两 C 臂+复链产 bao.exe)。

### W7 第二层(ew4 5ce86a79+f82f620e):三 .lib 清+20 符号分类

casefix 目录(Linux lld-link 大小写面,可持续吸收)+**libarchive 3.8.7/cares 全新 vendored DirectBuild**(两 C 库一次编译绿,unix 面不变)+bao_loop_tick windows 臂(us_loop_pump 改道)。链接推进到 20 未定义符号:A 类 17(BunString__* 等上游 jsc/bindings C++ 未 msvc 供给)/B 类 3(stdio/init)/C 类 1(shim _pread dllimport,s1a 已派)/D 类 1(stdio windows)。**A/B/D 续派 ew4(linux 侧定义定位+msvc 臂供给,bao.exe=硬判据)**。

### W7 第二层 17/20+第三层分类派发(f8bcaa40,2026-09-22)

BunString__×12(bun_core windows 臂全族)+getRSS(GetProcessMemoryInfo)+bun_is_stdio_null+clock_gettime_monotonic(Instant 锚)闭。第三层 20:**bun_core 8(ew4 续:visibleWidth 族[unicode_width 复用]/ANSI 迭代/ttySetMode/ensureHash/createExternal)**+quic 6(em3:quic.c 回归 windows 构建)+spawn_sync 2(ec3)+blob/vm 2(s1a,**纠偏:linux 能链必有供给者,先定位**)+_pread(s1a 已闭 22ae9f33)。四线并发,bao.exe=终判。

### W7 第三层·spawn_sync 2 闭(ec3 8a42fd50)

前提修正实证:两符号=孤儿 extern(bun_jsc 时代家族,linux 链得过只因零消费者)→**双侧无条件供给**(灭 linux 潜在断链):destroy=js_current 单例文档化生命周期(泄漏即模型,非消音)+suppress=真 thread_local swap 状态(+pub 读面供 W7 微任务 checkpoint)。双面 build 绿+msvc 链接面双 T。归因注记:HEAD 级自破(windows_enable_stdio_inheritance)系 ew4 在途 WIP 已修未提交。**余量=ew4 10+em3 quic 6**。

### W7 第三层·quic 6 闭(em3 a43947fd)

quic.c 回归 windows C 面(撤 lsquic 锁时代跳过;bao 保留树门系本全)+libuwsockets_h3.cpp 全平台入 C++ 面(**h3 四符号全平台从未编过=又一孤儿类**)。六符号逐 TU 实测发射(ew1 cross env 手动 clang-cl);linux 零回归+52/52+TLS spot 绿。共树阻断上报:bun_core 在途(ew4 10 符号工作态)——**终局唯一余量=ew4 的 bun_core 8+blob/vm 2**,其落即全图重链。

### W7 第二层终收+W8 立派(f8bcaa40+86db92c2,2026-09-22)

17+1 闭(BunString__×12 Rust 实现/getRSS/stdio_null/clock/stdio_inheritance[SetStdHandle×3])。**关键实证:linux bao 二进制产出(RC=0)且同 20 符号 U=跨平台 deferred 面**(ELF 容忍 vs COFF 严,linux 面同样未吸收——非 windows 特有)。W8 终链已派 ew4:**Rust 优先实现裁定**(BunString__/ec3/s1a 三先例;禁整吞上游 C++;单点报批制)——spawn_sync 家族 5/SourceProvider 5/NamedPipe 3/rescle 2/blob/vm 2/系统 2。bao.exe=硬判据,顺带治 linux deferred。

### W7 L3·bun_core 8 闭(317d350e+b7b7e892,2026-09-22)

visibleWidth 族(unicode_width 复用)/ANSI 迭代/ttySetMode(树内 tty.rs windows 臂,SetConsoleMode)/ensureHash/createExternal + Bun__ramSize(GlobalMemoryStatusEx)/isThreadSafe 落地。webcore_faces 2(8baf6739):`__bun_stdio_blob_store_new`(StdioBlobStore #[repr(C)] intrusive refcount ctor=2 真实现)+`__bun_js_vm_get`(mozjs Runtime TLS accessor)。

### W7 L3·rescle 真身(5ae38a51)

src/sys/windows/mod.rs ~890 行 Rust ResourceEditor(字节级镜像上游 quirk),bun build --compile 元数据面;windows_enable_stdio_inheritance(86db92c2 已在前)。真机 VersionInfo smoke 记 W3' 注记。

### W8 终链尾单双闭(s1a 2ce20635 + ew4 ab201b29,2026-09-22)

NamedPipe 15(s1a:bao_runtime socket.rs,状态/流控/写/lifecycle/ssl-never-TLS 行为 parity)+四散点(ew4):ZigSourceProvider__getSourceSlice(src/sourcemap 空面,SM 下 JSC 路径恒不存在)/WTFStringImpl__isThreadSafe(bun_alloc,**顺带根治 windows_ensure_hash 位布局 bug:hash<<2 污染 kind 位 4/5 → hash<<8|flags&0xFF**)/PE-graph Data/Length(exe_format/pe.rs standalone_pe 模块:GetModuleHandleExW UNCHANGED_REFCOUNT→DOS/NT header walk→.bun section,原子缓存,直接移植上游 _WIN32 臂)。

### W8 终链 E0432 修(C 直修,2026-09-22)

`bun_standalone_graph` 按 Rust path import `bun_exe_format::pe::{Bun__...}` 而 windows 实现在 `pe::standalone_pe::` 子模块(not(windows) 才在 pe 根 extern)→ import 改指 standalone_pe::。净终链重跑(bao.exe 判定)。脏树合流注记:ew4 spawn_sync 家族续(bun_sm+bun_uws dep)/tty windows 臂/CDP clone-face 观测(REQ-CDP-006,独立工作流)/vendor/libffi-sys 目录删除(4e2f7209 残留,patch/exclude 已删但目录未 git rm)——链后按功能分笔提交。

### ★W8 真机首跑三连崩→ABI 根治(509761cc,2026-09-22)

**bao.exe 产出后真机(.200) `-e 'console.log(42)'` RC=5 零输出**。逐层探针(E/R/W/S/X 七层)+ 进程内 VEH 崩溃取证(ExceptionRecord 原始 words + RtlCaptureStackBackTrace + force-frame-pointers + PDB 离线符号化)钉死真栈:`inject_js_hooks → evaluate_script_cached → CompileGlobalScriptToStencil → FrontendContext::setCurrentJSContext → JSContext::stackLimitForCurrentPrincipal` 内 READ 近空。**根因=C++ 按值返回类 ABI 分歧**:MSVC 对「有用户默认构造子或 deleted copy ctor 的类」按隐藏 sret 槽返回,Itanium/Rust 按寄存器返回→参数整体右移一格+返回结构写穿第一参(JSContext)→堆腐蚀→全下游随机崩(ucrtbase strlen/CreateErrorNotesArray 皆是余震)。**类成员横扫**:`already_AddRefed<Stencil>`×4(Compile{Global,Module}ScriptToStencil[1],上游 mozjs crate 原生携带=上游 bug,draft /tmp/issue-mozjs-msvc-sret.md)+ `JS::PropertyKey`(GetWellKnownSymbolKey,用户 constexpr 默认构造子)。**修法=jsglue 裸指针/裸 bits trampoline**(bao_Compile×4 返回 `JS::Stencil*`、bao_GetWellKnownSymbolKeyRaw 返回 uintptr_t,双 ABI 恒等);7 个 Rust 调用点改换 raw 面;jsapi2_wrappers 的 5 个 ABI 陷阱声明删除。**验收(真机)**:`-e console.log(42)` RC=0 出 42;run smoke.js(map/JSON/process.platform=windows/Date/setTimeout)RC=0;run mod.mjs(ESM)RC=0;missing-file 错误路径 RC=1 带消息。附带排除假说:PE 1MB 栈 reserve(patch 16MB 无效,但确证 debug 构建应加 /STACK)、DEBUG 布局不一致(confdefs 双侧无 DEBUG)、栈溢出。探测期 `/STACK` 教训:lld-link 默认 1MB,debug O0 建议显式放大——后续 build 配置项。

### ★W8 T1 真机测试面落地(5d3c1f8a,2026-09-22)

交叉测试二进制首次在 .200 真机全量跑通(win-test-runner,--test-threads=1,rc+result 双证)。**绿 1779**:bao_stealth suite 1379/1379 + lib 314/314 + bun_core lib 52(+1 ignored)+ bao_engine lib 20/21 + bun_sm module 14/15。测试面 windows 臂全落:test-sink libc 面/链接 seam(uv_get_osfhandle=UCRT wrap 同 libuv 本体)/return_address 走 RtlCaptureStackBackTrace(MSVC 帧无 SysV [rbp+8] 链,实证 saved_pc=0)/xdr 测试目录线程名净化/console 重定向 pipe+dup2 返回值面/高层软链 provider dev-dep+force_link 锚(GNU ld 容忍未定义 vs COFF 拒绝;cargo 不链未 use 的 dev-dep)+ /OPT:NOREF 保 seam。**残余 4 项(如实开放)**:①bun_runtime suite 首测族挂起(fetch 形)②console 重定向 flush 在 output-sink vtable flush 内挂(bun_sys windows 写路径)③TLA 模块 TDZ(module_sm)④c2 性能比断言环境敏感(非正确性)。上游 issue 草稿:/tmp/issue-mozjs-msvc-sret.md(servo/mozjs ABI 类,交互会话待用户过目后提)。

### W8 深修②:console/stdio 重定向类根治(0d2840f4,2026-09-22)

上游核查:servo/mozjs 主线仍带 4 个 already_AddRefed wrapper(未修)→ 按 2026-09-22 用户裁决保留我们 fork 的 trampoline 修复,非本账号项目不提 issue(draft 作废)。sret 类消费面复扫=零残留。

**三层根因+修**(.200 分段取证):
1. 启动 stdio 缓存冻结裸 GetStdHandle 句柄→dup2 失明;改 CRT-fd(uv-kind)形态(native() 写入期解析,上游 bun 语义=经 CRT fd 写)。
2. QuietWriter 槽位往返存 fd.native()(构造期解析句柄,冻结绑定)→ 改存 Fd tag 位型(to_bits/from_bits 新增),写入期解析,两对 qw(sys+core test 孪生)同改。
3. UCRT 默认 invalid-parameter= Watson 静默 fastfail(exit 9,_get_osfhandle 垃圾 fd 实证)→ windows_stdio::init 装返回式 handler(errno=EINVAL,CRT 落回文档化错误返回);init_test 在 windows 先拉缓存 stdio。
验收:bao.exe console.log/error/warn+smoke 绿;console_routing 2/2、console_output 3/3 真机绿。

**残余(下一聚焦,已精确定位)**:触 console/fetch natives 的测试在 JsContext teardown 崩——`destroyRuntime → GC → JS::RootingContext::traceStackRoots` AV(悬垂栈根;suiteA abort_signal 与 suiteB host_fn 同栈同判;es_advanced 等不触 natives 的 JsContext 测试 teardown 无恙=natives 路径留死根)。TLA TDZ(module_sm)与 c2 性能比断言(环境敏感)仍开放。

### ★W8 深修③:sret 第三实例=teardown AV 与 TLA TDZ 同源(0f1bc667,2026-09-22)

双 E 并行独立取证收敛同一根因:`JS::DequeueNextRegularMicroTask` **按值返回 `JS::Value`**(用户构造子→MSVC sret,bindgen 寄存器声明)→ 每次 dequeue 双腐蚀:①写出队值穿 `*RCX==*cx==RootingContext::stackRoots_[0]`(精确栈根链头);②死栈地址当 Value 入 task root。下一次 GC 走污染链→traceStackRoots AV(teardown 崩);reaction job 被静默丢弃→TLA 模块 async 体永不 resume(V TDZ)。**console natives 全无罪**(E1 bisect 电池 tdf_00~16 实证);该家族随 SM153 前移(em1)落地,晚于 W8 首轮横扫故漏网。修=同 mangled symbol sret 形态重声明(msvc 臂)/Itanium 保持原声明;同族 `DequeueNextDebuggerMicroTask`/`PeekNextMicroTask` 零调用点未动(未来接线须同修)。连带修 0d2840f4 引入的 `Fd::to_bits/from_bits` posix 编译回归(FdBacking 跟随 u64/i32)。**真机终验**:bao_engine suite **391/391**、module_sm **15/15**(TLA 愈)、tdf 电池 17/17、stealth 1379+314、core 52、bao.exe 冒烟绿——**真机累计 2151 测绿**;bun_runtime suite 崩已愈,剩余阻断=该 suite 自带 exit(0)-in-test 设计(杀 harness,独立治理项);另有 test_bun_api_all Bun.env.PATH 断言(既有,env 读取域)与 bun_sm/bun_runtime lib-test duplicate-symbol(既有,lib-test 目标从未在任何平台绿)开放。

### ★W8 收官:全测试面解锁(8d965656,2026-09-23)

并发双 E(E3 suite 解截流/E4 lib-test 去重)+ 主会话 c2 取证:
- **E3**:exit(0)-in-test 截流根治(self-re-exec 隔离 harness,13 个 force-exit 测试语义等价改造);36 崩溃/挂起测试同入 deadline 隔离(单崩不再拖死整跑,如实 FAIL);find_bao_binary/dlopen 平台化/宿主门控/zz_probe 清除。bun_runtime suite 首次完整收尾:**471 passed/114 failed**,114 份失败全档分责 16 项产品缺陷(P1-P16:.200 fail_*.log+矩阵)。
- **E4**:lib-test duplicate 根治=删制造环的 dev-dep 引用+本地 #[cfg(test)] seam 1:1 镜像生产 owner Phase-1 语义(bun_sm 40+面/bun_runtime 38面,ABI 全经接口别名零手猜);**bun_sm lib 208/208 首绿、bun_runtime lib 629+1(产品缺陷刻意留红)+7 spawn skip(spawn 状态依赖硬崩分责)**,两目标双平台历史首次可构建可运行。
- **主会话**:c2 XDR decode-vs-compile 倒挂三次实测(10.5 vs 7.1ms)=真实性能 finding 非噪声(load 路径干净,倒挂在 DecodeStencil 侧),入账不假绿。
- **真机终态:3459 绿**(engine 391+stealth 1379+314+core 52+module 15+sm lib 208+rt lib 629+rt suite 471);linux 全链 check 绿。
- **产品缺陷 backlog(W8 交付的核心资产)**:P1 env 大小写/P2 FFI 外呼 AV/P3 X509 load 顺序/P4 uv_loop_delete EBUSY/P5 tick wedge/P6 platform=win32/P7 spawn/worker/ipc/cluster(含状态依赖硬崩)/P8-16 fs/watch/dns/os/net/prometheus 级杂项/get_username GetUserNameW/c2 性能——下一波 Windows 产品化主清单。

### ★W8 产品化波:七域并发根治(b04cee87,2026-09-23)

7 线域互斥并发(E5 env/E6 ffi/E7 X509/E8 loop/E9 spawn/E10 fs·dns·os)+E11 daily-ops worktree 隔离。真机已验(b04cee87 前逐项):
- **E5**:env CI 全套/win32/nanoseconds 真值。**E6**:FFI 外呼 AV=SysV trampoline 误编 windows(rdi/rsi vs rcx/rdx/r8),新 win64 位置化 trampoline,9 探针全绿。**E7**:P3 证伪归档(load() 五 init 皆空函数;InvalidCertKey=旧链接组成伪影),9-step 自诊断落盘。**E8**:uws App 种入 native loop 不变量恢复(uv_loop_delete EBUSY 根治)+Mini tick 非阻塞 pump(wedge 根治)。**E9**:spawn 硬崩=environ_ptr 悬垂非空指针;535 常量笔误;IPC fd 转换/PeekNamedPipe 泵/cluster boot 落地,五项修复。**E10**:fs EINVAL=libc::O_* 位碰撞(CREAT 丢+EXCL 误加);watch 父目录监视+offsetof(12) 截断;WSAStartup Once(dns 10093);require verbatim 剥离;netif ERROR_MORE_DATA(111);GetUserNameW;真机 27/1。
- **判别纪律**(node v24.12.0 oracle 同机对照):ENAMETOOLONG/recursive-throw/'lo' 键/qsort 序=测试期待未平台化(产品与上游逐字节一致),tests 已平台化收口(双平台断言/门控)。
- **主会话追加**:os.tmpdir windows=GetTempPathW 语义(WSL 泄漏 TMPDIR 劫持根因)。
- **E11(daily-ops)**:worktree dops-20260923 4 absorb commits(streams ERR_INVALID_STATE/fonts 16.16/Sec-Fetch/innerText alloc)+#47 验收过+需判 5 全收口;待验证毕合并。
- **事故**:.200 WSL sshd 于波末拒连(ICMP 通/kex reset),真机终验与全量 battery 挂起待机器恢复;断连前全部修复已验。
- **残余 backlog**:src/sys/fs.rs O_* 横扫(其他调用方同病)/openSync 假 fd 0/bun_glob_api/fs_rmdir_recursive/cluster primary 泵饿死(E8 drain 域)/bun_build parse-worker 堆腐蚀/h2 语义 ×2/c2 XDR decode 性能。
