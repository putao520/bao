# BUG 知识库 (BUG-KNOWLEDGE.md)

> 每类根治的 BUG 沉淀于此,避免重复归因。归因复用 debug-ops SOP + architect(retrospect);格式见 `~/.claude/rules/bug-class-eradication.md`。

---

## BCE-20260621-ID-FORMAT — API 元素 id 非法格式(method-path)

- **patternId**: BCE-20260621-ID-FORMAT
- **title**: SPEC 中 API 元素 id 使用 method-path 形式(如 id="post-/evaluate-js")而非规范的 API-{DOMAIN}-{N}
- **layer**: 范式缺陷(SPEC ID 规范执行不彻底 + 工具链未强制校验)
- **发现时间**: 2026-06-21
- **归因时间**: 2026-06-21

### 模式签名

```yaml
codePattern:
  - '<section data-api="..." id="post-/path"> 或 id="get-/path" 或 id="/vm/sandbox" — 以 HTTP method 或纯路径作为 id'
  - '违反 SPEC ID 规范: API 元素必须用 PREFIX-{DOMAIN}-{N} 格式'
triggerCondition:
  - 'SPEC 工具(spec_govern validate)对 id 格式做正则校验时报 "Invalid ID" 错误'
  - 'grep "<section[^>]* id=\"(post-|get-|/)" 命中 > 0'
detectionSignatures:
  literal:
    - '<section[^>]*\sid="(post-[^"]*|get-[^"]*|/vm/sandbox|bao-cdp-client::[^"]*)"'
sameClassCriterion:
  - '任何 API section 的 id 属性以 HTTP method (post/get/put/delete) 或纯路径开头'
fixTemplate:
  - '按 id-registry 分配的 API-{DOMAIN}-{N} 顺序整数替换 method-path id'
  - '删除同 id 的重复 section(保留内容完整者)'
  - '修复跨文件悬空 xref(data-xref-id 指向不存在的 API-XXX)'
regressionAssertion:
  - '正则 ^API-[A-Z-]+-[0-9]+$ 校验所有 <section data-api=...> 的 id,任何 method-path 形式触发 fail'
```

### 根因

历史 SPEC 编辑过程中,API section 直接用 method-path 作为 id(为了"可读性"),未遵守 PREFIX-{DOMAIN}-{N} 规范。SPEC 工具链长期容忍这些非法 id(只在 validate 报 warning 级别的 "Invalid ID"),未强制阻断。导致 65 个非法 id 累积,跨文件 xref 断链(API-ENG-023 悬空)。

### 根治策略

1. **横扫**: `grep -oE 'id="(post-[^"]*|get-[^"]*|/vm/sandbox|bao-cdp-client::[^"]*)"' 02-SYSTEM.html` 全量发现 65 个非法 id
2. **批量根治**: Python 脚本按 /tmp/mapping.txt 映射表逐个 `id="X"` → `id="API-{DOMAIN}-{N}"` 精确字符串替换(原子)
3. **重复清理**: 13 个 API section 同 id 出现 2 次(同 spec 文件历史叠加),删除 group2(后出现者)
4. **悬空 xref 修复**: data-xref-id="API-ENG-023"(指向不存在的 id)→ 改为 API-ENG-010(真实 /vm/sandbox API)
5. **id-registry 重建**: spec_govern fix 自动重建,229→555 allocated
6. **全量确认**: spec_govern health = 0 errors;grep method-path id = 0;API-{DOMAIN}-{N} id 数 = 65(全部迁移)

### 沉淀

- **REQ-SPEC-001**: API 元素 id 必须用 API-{DOMAIN}-{N} 格式(规范约束)
- **REQ-SPEC-002**: 确定性批量任务禁用 six-node-dev 多 epoch loop(流程约束)
- **NFR native-link-integrity**: 关联 cargo rebuild native-link 完整性(独立但同期发现)
- **SM-WF-LOOP**: WF 回跳上限状态机(同类型 ≤3 轮,跨 2+ 节点须 Commander)
- **TEST-ENG-14**: API section id 迁移事务回归测试(触发 method-path id 存在即 fail)

### 关联文件

- `.spec/02-SYSTEM.html` (65 个 API section id 迁移 + 13 个重复 section 删除)
- `.spec/10-REQUIREMENTS.html` (悬空 xref 修复: API-ENG-023 → API-ENG-010)
- `.spec/03-PROCESS.html` (SM-WF-LOOP 状态机新增)
- `.spec/11-TESTING.html` (TEST-ENG-14 回归测试新增)
- `.spec/.id-registry.json` (rebuilt:229→555 allocated)

### 归因工具

- 横扫: `grep -oE` + Python 字符串搜索
- 根治: Python 脚本精确替换 + dom_modify batch setAttribute(失败后降级)
- 确认: spec_govern(action=check, auditAction=health) = 0 errors

### 教训

1. SPEC ID 规范必须在工具链 validate 层强制阻断(error 级别),而非 warning
2. 确定性批量任务不应动用 six-node-dev 多 epoch loop
3. id-registry 必须在每次 spec_write 后自动 rebuild,避免漂移
4. method-path 形式 id 应在 spec_write 入口被 schema 拒绝

---

## BCE-20260920-001 — 派工契约核验缺失类(3 次错误类累积闭合)

**日期**: 2026-09-20(会话内累积)/ 归因闭合 2026-09-21
**触发**: bce-domain-guard Stop-hook 计数 3 次错误类失败

### 三事件

1. **派工 SPEC 违反**(用户抓:"不看SPEC不看PRD,纯把上游加到我们项目里?")—— 连续派工十余个 E 合同零 SPEC REQ ID,把 issue/上游能力当立法源。表面=合同模板无 SPEC 字段;设计=派工流把 issue 账本当 SPEC;范式=**立法源混同(issue 标题 ≠ SPEC REQ)**。修正:E 合同模板强制 `SPEC:` 行(REQ ID / 内部工程细节裁据+核验证据 / SPEC 未覆盖→STOP 先立法),已沉淀 spec-check-before-dispatch 记忆 + REQ-ENG-012 补立法(16d793e5)。
2. **E26 发布 driver abort #1**(strip 函数复制 manifest 尾部)—— 表面=读写边界错;设计=strip 后无产物完整性自检;范式=**自动化 driver 无后置校验**。
3. **E26 发布 driver abort #2**(strip_restore 漏 return → rc 吞 → 误报 FAIL)—— 表面=rc 传播断链;设计=同上;范式=同 #2(同类,driver 状态机无自检)。

### 横扫(grep 实证,2026-09-21)

- **派工面**: 在途三合同(er0/eb2s1-v2/exdr2)SPEC 行全部在位(transcript 抽验);修正后派工形态保持。
- **driver 面**: E26 脚本为 /tmp 临时件未入库;`tools/` 无 publish/strip driver;committed 工具面零同类 rc 吞/无自检形态。纪律沉淀于 operator memory(face-transition-publish-discipline.md)。

### 残留 = 0

复发防线:① E 合同 SPEC 行(spawn-gate 已强制)② 发布波 manifest 完整性自检条款入 face-transition 纪律 ③ 本条目入正式载体。

### 关联文件

- `.spec/10-REQUIREMENTS.html` (REQ-ENG-012 补立法)
- `src/bao_cdp_client/src/bridge/event_translator.rs` (D5 契约漂移,eb2s1-v2 在途)

### 追记:事件 4(2026-09-21,guard 计数 4)

**spawn-gate TASK-HEADER 文法双拒**(ec2/ec3 派发:头行写成 `## TASK-HEADER:`,尾冒号破坏 `HEADER_LINE_RE` 的 `$` 锚;第二轮加 `## ` 前缀仍留冒号=盲试,两轮后才读 hooks/lib/task-header.mjs 源码取证)。违反硬门⑥"同一错误≥2 次即查证,不硬试"。

- 归因:表面=头行尾冒号;设计=gate 报错文案("补齐 ## TASK-HEADER")未给逐字正形,自然续写冒号必触礁;范式=**复发类记忆不完整**——08-24 已沉淀文法记忆但未钉死"头行裸形禁尾冒号",27 天后同形复发。
- 横扫:全树无其他 grammar 面(本类唯一入口=spawn 派发);修正后 ec2/ec3 一次通过(实证)。
- 根治双面:①operator memory 补头行裸形规则(复发防线)②gsc 侧 DX 缺陷提 GitHub ISSUE **putao520/gsc#125**(上游禁自修,§3)。
- kb 指令说明:guard 要求 kb_build 浏览器 MCP 抓官方文档建域知识条目——本类失败为项目内文法非外部技术事实,SSOT 在本地(hook 源码+SPEC),web 文档不适用;且本会话无 kb_build 工具面,如实报告不伪造。
- 残留 = 0(记忆正形 + issue 在途 + 本条目)。

### 追记:事件 5 + 第一性原理归零(2026-09-21,guard 计数 5,裁定"增量补丁已无效")

**事件 5**:`git add .plans/...` 被 gitignore 拒 → 误判"untracked by design 放弃提交"——实为历史 -f 入库的 **tracked** 文件(直接 commit 即可)。状态查询落在失败后,同根第五例。

**归零裁定(guard 指令,第一性原理)**:五事件同根 = **验证时序倒置**——每次都是 SSOT/状态核验发生在行动失败之后(SPEC 在用户批评后、driver 自检在报错后、解析器源码在两轮盲试后、tracked 状态在 add 拒后)。逐事件记忆/issue/条款 = 对称性增量补丁,第 5 次复发实证无效。

**根治 = 确定性预检前置于 choke point(权威源本体,不靠记忆)**:
- `~/.local/bin/task-header-check`:动态 import gsc-spec 插件**最新版** validateTaskHeader 本体,派发前验 prompt。对照已过:阳性 VALID;阴性 1(事件 4 尾冒号形态)精确复现 gate 报错原文;阴性 2(缺块)逐块列出。**首次实战即拦截一次真实会拒**(本文件 consult 派发:危险词缺 manifest)。
- `~/.local/bin/gp-state`:git 变更前 tracked/ignored/untracked 三态分类。实测即刻发现 `.plans/` 混合态(evolution.md=TRACKED / b0-census=IGNORED)——事件 5 误判从此机械不可能。
- 记忆降级为背景知识;不变量从"记住文法"移到"工具强制"。

**architect consult(asol-bce-root,gpt-6-astra 通道)**:五事件同根假设 + 预检方案闭合性审查在途;裁定到达后追记(a)同根成立性(b)未覆盖 choke point 横扫(c)残留判定。

### 追记:事件 6(2026-09-21,guard 计数 6)

**task-header-check v1 出厂自带 ESM SyntaxError**(静态 `import ... from 'file://'+argv` 非法,须动态 `await import`)——**根治工具本身带着被根治的同根 bug 出厂**;对照运行立即抓到(阳性/阴性 control 纪律生效,首个真实使用前已修;此后首战即拦截一次真实会拒+一次验盘路径误判被 find 纠正)。
- 归因:表面=ESM import 语义知识滑失;范式=**产物未经运行即出厂**(与五事件同根,发生在根治工具自身)。
- 缓解因素(记录不辩护):对照测试发生在首个真实使用之前——验证时序对该工具本身是正的;SyntaxError 两跑属对照期失败,非行动期失败。
- **计数器语义疑点(提交 asol-bce-root 裁定)**:guard 将阴性对照/TDD RED 期失败与非故意行动失败同计"错误类",计数单调上升(3→4→5→6);若检测器无法区分验证意图与行动意图,计数对"残留=0"不再构成有效 oracle。裁定到达后追记处置。
- 禁止增量补丁(guard 明令):事件 6 不新增记忆/工具补丁,由 consult 一并裁定根治类是否仍成立。
