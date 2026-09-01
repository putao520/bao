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

- Bao 源码未发现 `JS_RequestInterruptCallback` 接入；
- 未形成 Bao Stencil/XDR script cache；
- 未发现 Realm-native locale/timezone override 的 Bao 侧使用；
- CDP Debugger 仍未证明由 SM 原生 debugger/script/frame/object facts 驱动；
- GC/rooting 中仍有 intentional leak / `mem::forget` / foreign-thread fail-safe 路径，需量化；
- mozjs upgrade 已有 patch replay，但没有 JSAPI capability drift/adoption ledger。

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

## 5. SpiderMonkey capability inventory（初始种子，待 #30 变成机器生成）

| Capability | 当前判断 | Bao 当前状态 | 目标 | Issue |
|---|---|---|---|---|
| JSEngine / JSContext ownership | Public embedding core | native | 收口生命周期 | #23/#29 |
| Realm | Public | native/partial mapping | 显式 topology | #23 |
| Compartment | Public | 待审计 | 裁决隔离边界 | #23 |
| Zone | Public/internal embedding surface | 待审计 | 裁决 GC topology | #23/#29 |
| Realm locale override | Public/current ref 待绑定 | missing | engine-native Stealth | #28 |
| Realm timezone override | Public/current ref 待绑定 | missing | engine-native Stealth | #28 |
| debugger visibility | Public options/patch-related | partial | unified policy | #27/#28 |
| JIT preserve/options | Public options/current ref 待绑定 | unknown | benchmark-based policy | #28 |
| SharedMemory/Atomics/SAB policy | Realm creation options | unknown | explicit capability policy | #28 |
| Embedder JobQueue | Public | native | scheduler contract | #25 |
| Interrupt callback/request | Public | missing | timeout/cancel | #24 |
| Stencil | Experimental | missing | internal cache first | #26 |
| Stencil/XDR | Experimental/internal mix | missing | persistent cache if proven | #26 |
| Off-thread compile | Public/experimental mix | unknown | threshold-based use | #26 |
| Debugger/script/frame/object facts | mixed | partial/unknown | typed adapter | #27 |
| Memory/GC observability | mixed | partial/unknown | soak metrics | #27/#29 |
| Rooting APIs | Public | native | complete ledger | #29 |
| Structured Clone | Public | unknown/Servo-used | audit later | #30 |
| WebAssembly engine controls | Public | unknown | audit relevance | #30 |
| Principals/security hooks | Public | unknown | audit relevance | #30/#20 |

注意：本表不是完成证明。#30 建立机器 ledger 后，本表只保留 Domain summary。

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
