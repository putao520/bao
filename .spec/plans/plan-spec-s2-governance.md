# 开发计划: SPEC S2 治理修复 (API ID + REQ 关联 + 维度连通) | epoch: 2 | status: active

## status: active

## 范围：REQ-ENG-010, REQ-ENG-011, API 元素完整性, DF/CF 维度连通性, S2 门控重验

关联 SPEC 元素：
- REQ: REQ-ENG-010 (异步 Fetch/HTTP 事件循环集成), REQ-ENG-011 (node:vm Sandbox 语义)
- Entity: FetchTasklet, FetchResolveTask, FetchOutcome, VmSandboxContext
- API: 02-SYSTEM.html 中 data-api 标记的 ~30 个端点（含 GET /sandbox-status, GET /api/* 等）
- Dataflow/Controlflow: DF-FETCH-ASYNC-001, CF-FETCH-ASYNC-001（已引用但定义缺失）, business/technical 维度骨干
- BUG: BUG-ENG-369 (REQ-ENG-010 关联), BUG-ENG-368 (REQ-ENG-011 关联)
- SM: FetchTaskletLifecycle (REQ-ENG-010 关联状态机)

复用探测结论：
- spec 域: full_match — REQ-ENG-001~011/TEST-ENG-*/API 路径在 10-REQUIREMENTS 与 02-SYSTEM 已定义
- code/ui/asset/pattern 域: no_match — 本计划纯 SPEC 治理，无源码改动（scope = SPEC 元素增删改 + HTML 结构修复）

## 影响矩阵

| SPEC ID | 关联 TASK | 文件 |
|---------|-----------|------|
| 65 个 data-api 端点 ID | TASK-1 | .spec/02-SYSTEM.html |
| REQ-ENG-010 | TASK-2 | .spec/10-REQUIREMENTS.html |
| REQ-ENG-011 | TASK-2 | .spec/10-REQUIREMENTS.html |
| DF-FETCH-ASYNC-001, CF-FETCH-ASYNC-001 | TASK-3 | .spec/02-SYSTEM.html |
| business 维度 DF/CF | TASK-3 | .spec/02-SYSTEM.html, .spec/04-DATA-MODEL.html |
| technical 维度 DF/CF | TASK-3 | .spec/02-SYSTEM.html, .spec/04-DATA-MODEL.html |
| 5 个 data 组件桥接 | TASK-3 | .spec/04-DATA-MODEL.html |
| S2 门控 (0 errors) | TASK-4 | (验证无文件写入) |
| .spec/.id-registry.json | TASK-1, TASK-2, TASK-3 | .spec/.id-registry.json |

## 任务树（扁平列表）

### TASK-1: 修复 02-SYSTEM.html 中 65 个无效 API ID
- SPEC: REQ-ENG-001~011 (API 引用完整性) [验收标准: spec_govern health → 0 API ID error / 0 断链] | TDD: 无（SPEC 治理无测试用例）| 文件: .spec/02-SYSTEM.html | 实现:
  - 前置: 确认 gsc MCP 已重启（Architect 已 patch api.mjs:136-164，但运行中 MCP 持有旧缓存）。若仍不可用，降级为 dom_modify batch 原子修复
  - 方案 A (首选): spec_write(elementType="api", crudAction="update") 对 65 个 API 端点统一规范化 ID（data-api 属性 + 端点 id）
  - 方案 B (降级): dom_modify(action="batch") 原子批量修复 data-api 属性格式，保证 65 个端点 id 唯一且符合 `API-{DOMAIN}-{N}` 规范或路径式 id
  - 每个修复后 spec_govern(action="check", auditAction="health") 复查 error 计数递减
  - 同步更新 .spec/.id-registry.json（若 API 域缺则补建 API domain nextN + allocated 数组）
- 复用锚点: spec:[REQ-ENG-001~011, data-api 端点列表] / code:[] / ui:[] / asset:[] / pattern:[dom_modify batch 原子操作模式]
- 依赖: 无
- 状态: pending

### TASK-2: 补全 REQ-ENG-010/011 的 Entity + API 关联
- SPEC: REQ-ENG-010 (FetchTasklet/FetchResolveTask/FetchOutcome Entity + POST /fetch/async API), REQ-ENG-011 (VmSandboxContext Entity + POST /vm/create-context API) [验收标准: spec_govern audit req_coverage → 100%, 两个 REQ 的 section 标签同时含 data-req-entity + data-req-api] | TDD: 无 | 文件: .spec/10-REQUIREMENTS.html | 实现:
  - REQ-ENG-010: section[data-req="REQ-ENG-010"] 当前缺 data-req-entity + data-req-api → dom_modify(action="setAttribute", selector='[id="req-eng-010"]', attributeName="data-req-entity", attributeValue="FetchTasklet, FetchResolveTask, FetchOutcome") + attributeName="data-req-api" attributeValue="POST /fetch/async"
  - REQ-ENG-011: section[data-req="REQ-ENG-011"] 已有 data-req-entity="VmSandboxContext" 但 data-req-api 为空 → dom_modify(action="setAttribute", selector='[id="req-eng-011"]', attributeName="data-req-api", attributeValue="POST /vm/create-context, POST /vm/run-in-context")
  - 若新增 POST /fetch/async, POST /vm/create-context, POST /vm/run-in-context 为新端点 → spec_write(elementType="api", crudAction="create") 在 02-SYSTEM.html 落地对应 data-api 段（与 TASK-1 协调，避免 id 冲突）
  - 验证双向 xref: Entity 段的「关联需求」反向引用 REQ-ENG-010/011 已存在（FetchTasklet/VmSandboxContext 段已含），仅需正向补全
- 复用锚点: spec:[REQ-ENG-010, REQ-ENG-011, FetchTasklet, VmSandboxContext] / code:[] / ui:[] / asset:[] / pattern:[dom_modify setAttribute 模式]
- 依赖: TASK-1（API ID 规范化完成后再补新端点，避免 id 冲突）
- 状态: pending

### TASK-3: 填充 business+technical 维度数据流并桥接 data 组件
- SPEC: DF-FETCH-ASYNC-001, CF-FETCH-ASYNC-001, business 维度骨干, technical 维度骨干 [验收标准: spec_govern audit dfs_connectivity → 连通, dimension_connectivity business+technical 通过] | TDD: 无 | 文件: .spec/02-SYSTEM.html, .spec/04-DATA-MODEL.html | 实现:
  - 当前状态: 02-SYSTEM.html 中 data-dimension / DF- / CF- id 元素计数 = 0（维度骨干完全缺失），但 FetchTasklet Entity 段已引用 DF-FETCH-ASYNC-001/CF-FETCH-ASYNC-001（悬空引用）
  - 步骤 1: spec_write(elementType="dataflow", crudAction="create", data={id:"DF-FETCH-ASYNC-001", dimension:"technical", ...}) 落地已引用但未定义的 DF-FETCH-ASYNC-001（HTTPThread→ConcurrentTask→JS线程 Promise 解析链路）
  - 步骤 2: spec_write(elementType="controlflow", crudAction="create", data={id:"CF-FETCH-ASYNC-001", dimension:"technical", ...}) 落地 CF-FETCH-ASYNC-001（FetchTasklet 状态机转换控制流）
  - 步骤 3: business 维度骨干 — spec_write batch 创建 business 维度的 DF（用户 fetch 请求→响应业务流）+ CF（sandbox 创建/执行业务控制流）
  - 步骤 4: technical 维度骨干 — 补全 JS 执行管线/渲染管线/CDP 路由的技术维度 DF/CF
  - 步骤 5: 桥接 04-DATA-MODEL.html 中 5 个孤立 data 组件（FetchTasklet, FetchResolveTask, FetchOutcome, VmSandboxContext, H3FetchClient 等）通过 DF/CF 关联成单一连通图（双向 xref: Entity→DF/CF, DF/CF→Entity）
- 复用锚点: spec:[FetchTasklet, FetchResolveTask, FetchOutcome, VmSandboxContext, FetchTaskletLifecycle SM] / code:[] / ui:[] / asset:[] / pattern:[spec_write dataflow/controlflow batch 模式]
- 依赖: TASK-2（REQ-ENG-010/011 Entity 关联完成，DF/CF 才能正确反向引用 REQ）
- 状态: pending

### TASK-4: 验证 S2 门控通过 (0 errors / 100% coverage / connected)
- SPEC: 全局 SPEC 健康约束 [验收标准: workflow_guard gate stage=2 → pass, spec_govern health → 0 errors 0 warnings] | TDD: 无 | 文件: 无（纯验证）| 实现:
  - workflow_guard(action="gate", stage="2", dir=".spec", sourceDir="./src") → pass
  - spec_govern(action="check", auditAction="health", dir=".spec") → 0 errors / 0 warnings
  - spec_govern(action="check", auditAction="audit", auditMode="req_coverage", dir=".spec") → 100%
  - spec_govern(action="check", auditAction="audit", auditMode="dfs_connectivity", dir=".spec") → connected
  - spec_govern(action="check", auditAction="audit", auditMode="dimension_connectivity", dir=".spec") → business+technical 通过
  - spec_govern(action="check", auditAction="check", dir=".spec", checkLinks=true) → 0 断链
  - 任一失败 → 记录失败项，回溯对应 TASK 修复（不计入 BCE，纯治理回归）
- 复用锚点: spec:[] / code:[] / ui:[] / asset:[] / pattern:[workflow_guard gate + spec_govern health 组合验证模式]
- 依赖: TASK-1, TASK-2, TASK-3（全部完成后才验）
- 状态: pending

## 铁律
1. 文件域: TASK-1/TASK-3 共改 02-SYSTEM.html → 必须串行（TASK-1→TASK-3），禁止并行；TASK-2 独占 10-REQUIREMENTS.html 可与 TASK-1 并行但须 TASK-1 先完成 API id 规范化
2. 纯 SPEC 治理: 本计划零源码改动（scope 严格限定 .spec/ 目录），不触发 BCE（无 BUG 模式）
3. 复用 > 手写: dom_modify batch 原子操作优先于逐个 Edit；spec_write 优先于手动 HTML 编辑
4. 无 TODO/FIXME/stub: SPEC 元素必须完整定义（DF/CF 含完整 steps/nodes，REQ 含完整 criteria）
5. 范围守恒: 仅修复 S2 门控报告的 3 类失败（API ID / REQ 覆盖率 / 维度连通），不扩张到其他 SPEC 元素
6. 验证铁律: TASK-4 必须出 0 errors 的客观证据（spec_govern health 输出），非主观声明
7. MCP 缓存: 若 spec_write API 操作报错，先 `claude mcp restart gsc-spec` 再重试（Architect patch 后旧缓存问题）

---

## Epoch 2 DIFF (2026-06-21) — 仅针对 S4 失败的 TASK-1 重新计划

### 归因 (BCE 阶段 0-1)

| 项 | 内容 |
|----|------|
| 失败现象 | oracle_gate TASK-1 步骤1 (@trace 检查) + 步骤5 (覆盖率) fail：11 个 REQ (REQ-ENG-001~011) 全部 untraced，percent=0% |
| 触发条件 | batch-execute WF 内部调用 oracle_gate 时传入 `sourceInclude=home/**/*.rs`（缺前导斜杠 / 缺项目根前缀），glob 不匹配绝对路径 `/home/putao/code/rust/bao/src/bao_engine/src/lib.rs` |
| 影响范围 | 仅 TASK-1，TASK-2/3/4 未在本次 batch 中执行（依赖 TASK-1） |
| 根因定位 | **oracle_gate sourceInclude glob 误报（假阴性 false-negative）**，非源码缺陷。源码 `/home/putao/code/rust/bao/src/bao_engine/src/{lib,context,job_queue}.rs` 已存在全部 11 条 @trace 注解，覆盖 REQ-ENG-001~011（grep 实证见下） |
| 缺陷分层 | **工具调用层误用**（非表层缺陷/设计缺陷/范式缺陷）：batch-execute WF 调用 oracle_gate 时构造的 sourceInclude glob 与项目绝对路径不匹配 |

### 源码实证（grep 输出）

```text
$ grep -rn "@trace" /home/putao/code/rust/bao/src/bao_engine/ | grep -oE "REQ-ENG-[0-9]+" | sort -u
REQ-ENG-001  (lib.rs:1, 44; context.rs:1)
REQ-ENG-002  (lib.rs:78)
REQ-ENG-003  (lib.rs:73)
REQ-ENG-004  (lib.rs:74; job_queue.rs:1)
REQ-ENG-005  (lib.rs:67)
REQ-ENG-006  (lib.rs:68)
REQ-ENG-007  (lib.rs:45)
REQ-ENG-008  (lib.rs:46)
REQ-ENG-009  (lib.rs:47)
REQ-ENG-010  (lib.rs:48)
REQ-ENG-011  (lib.rs:49)
```

11/11 REQ 全部已 @trace，源码无需改动。

### 泛化 (BCE 阶段 2)

```yaml
patternId: BCE-20260621-001
title: oracle_gate sourceInclude glob 不匹配项目绝对路径 → @trace 检查假阴性
layer: 工具调用层（非代码缺陷）
codePattern:
  - 「oracle_gate 调用时 sourceInclude 传 'home/**/*.rs'，但项目文件在 '/home/putao/code/rust/bao/...'，glob 缺前导 '/' 或项目根前缀导致零匹配」
triggerCondition:
  - 项目根为绝对路径 (/home/...) 时，sourceInclude glob 未包含完整路径前缀或未使用 src/ 相对前缀
sameClassCriterion:
  - 任何 oracle_gate 调用 sourceInclude glob 无法匹配到实际源文件路径的情况
fixTemplate:
  - sourceInclude 用相对项目根的 glob：`{bao_engine,bao_runtime}/**/*.rs` 或直接省略 sourceInclude（让 oracle_gate 默认扫 sourceDir）
regressionAssertion:
  - oracle_gate(sourceInclude=相对 glob) 后 tracedCount=11 totalCount=11 untracedCount=0 percent=100%
```

### 横扫 (BCE 阶段 3) — 真阳性甄别

| 命中 | 类型 | 处置 |
|------|------|------|
| TASK-1 (本计划) | 真阳性 — glob `home/**/*.rs` 不匹配 `/home/putao/...` | 修正 glob 重跑 oracle_gate |

范围内仅 1 处 oracle_gate 误用（本 batch TASK-1），无其他同类实例。

### Epoch 2 任务树

> TASK-2/3/4 状态不变（未执行，依赖 TASK-1）。本 epoch 仅新增 TASK-1E2 替代 TASK-1 的 oracle_gate 验收。

#### TASK-1E2: 用正确 sourceInclude 重跑 oracle_gate 验收 TASK-1 (0 源码改动)

- SPEC: REQ-ENG-001~011 (验收门控) [验收标准: oracle_gate TASK-1 steps[1].status=pass, steps[5].status=pass, tracedCount=11, percent=100%, canCommit=true] | TDD: 无（纯验收门控回归，源码已含全部 @trace） | 文件: 无（0 源码改动） | 实现:
  - **禁止源码改动**：源码 `/home/putao/code/rust/bao/src/bao_engine/src/{lib,context,job_queue}.rs` 已含全部 11 条 @trace（grep 实证见上），任何 Edit 都属于画蛇添足
  - 步骤 1: 调用 oracle_gate(taskId="TASK-1", dir=".spec", sourceDir="src/bao_engine", reqIds=["REQ-ENG-001".."REQ-ENG-011"], sourceInclude="{bao_engine,bao_runtime}/**/*.rs") — 使用相对项目根的 glob，确保匹配 `/home/putao/code/rust/bao/src/bao_engine/**/*.rs`
  - 步骤 2: 若 step 1 oracle_gate 仍报 untraced → 改用 sourceInclude="**/*.rs"（宽 glob 兜底）或省略 sourceInclude 让 oracle_gate 默认扫 sourceDir=src/bao_engine
  - 步骤 3: 复核 oracle_gate raw output → steps[1].detail.traced 必须含全部 11 个 REQ-ENG-001~011，steps[5].detail.percent 必须为 100
  - 步骤 4: canCommit=true 即通过；仍 fail 则升级到 BCE（说明 oracle_gate 实现层有更深的 glob bug，需去 ~/code/claude/gsc 修源码后 `claude mcp restart gsc-spec`）
- 复用锚点: spec:[] / code:[/home/putao/code/rust/bao/src/bao_engine/src/{lib,context,job_queue}.rs 已有的 11 条 @trace] / ui:[] / asset:[] / pattern:[oracle_gate 相对 glob 调用模式]
- 依赖: 无（TASK-1 源码部分已在前序 epoch 完成且 git status 显示已修改 .spec/ 文件；本任务纯验收回归）
- 状态: pending
- 防复发：TASK-1E2 通过后，在 BUG-KNOWLEDGE.md 追加 BCE-20260621-001 条目（oracle_gate sourceInclude glob 误用模式），后续 batch-execute WF 须用相对项目根的 glob

### Epoch 2 铁律补充

8. **0 源码改动铁律**：TASK-1E2 禁止 Edit 任何 `.rs` 文件。源码已含全部 @trace，任何修改都违反「范围守恒」与「SPEC 治理零源码改动」原则
9. **glob 校验铁律**：oracle_gate sourceInclude 必须用相对项目根的 glob（如 `{bao_engine,bao_runtime}/**/*.rs`），禁止 `home/**` 这类缺项目根前缀的写法
10. **回归证据铁律**：TASK-1E2 完成后必须输出 oracle_gate raw output（steps[1].detail.traced 含 11 项 / steps[5].detail.percent=100 / canCommit=true）作为残留=0 的客观证据

## REQ台账 (reqLedger)

| REQ ID | 验收标准 | 关联 TASK | 闭合状态 |
|--------|---------|----------|---------|
| REQ-ENG-001 | SpiderMonkey engine core @trace | TASK-1 / TASK-1E2 | 源码已 @trace（lib.rs:1,44 / context.rs:1），待 TASK-1E2 oracle_gate 复验 |
| REQ-ENG-002 | codegen backend @trace | TASK-1 / TASK-1E2 | 源码已 @trace（lib.rs:78），待 TASK-1E2 复验 |
| REQ-ENG-003 | host_fn safe FFI @trace | TASK-1 / TASK-1E2 | 源码已 @trace（lib.rs:73），待 TASK-1E2 复验 |
| REQ-ENG-004 | Event Loop bridge @trace | TASK-1 / TASK-1E2 | 源码已 @trace（lib.rs:74 / job_queue.rs:1），待 TASK-1E2 复验 |
| REQ-ENG-005 | Module Loader bridge @trace | TASK-1 / TASK-1E2 | 源码已 @trace（lib.rs:67），待 TASK-1E2 复验 |
| REQ-ENG-006 | Bun.*/Bao.* API @trace | TASK-1 / TASK-1E2 | 源码已 @trace（lib.rs:68），待 TASK-1E2 复验 |
| REQ-ENG-007 | Node.js compat @trace | TASK-1 / TASK-1E2 | 源码已 @trace（lib.rs:45），待 TASK-1E2 复验 |
| REQ-ENG-008 | bun:sqlite bridge @trace | TASK-1 / TASK-1E2 | 源码已 @trace（lib.rs:46），待 TASK-1E2 复验 |
| REQ-ENG-009 | bun:ffi bridge @trace | TASK-1 / TASK-1E2 | 源码已 @trace（lib.rs:47），待 TASK-1E2 复验 |
| REQ-ENG-010 | async Fetch/HTTP @trace | TASK-1 / TASK-1E2 | 源码已 @trace（lib.rs:48），待 TASK-1E2 复验 |
| REQ-ENG-011 | node:vm Sandbox @trace | TASK-1 / TASK-1E2 | 源码已 @trace（lib.rs:49），待 TASK-1E2 复验 |
