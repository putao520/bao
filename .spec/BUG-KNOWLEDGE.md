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
