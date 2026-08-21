# issue 分诊规则(putao520/bao)

daily-ops §1 阶段 3(issue 分诊)与阶段 6(收尾回复/关闭)的操作细则。目标仓库:`putao520/bao`。

## §1 入口与游标

```bash
gh issue list --repo putao520/bao --state open --json number,title,body,labels
```

- `state.json` 的 `issue_cursor.triaged[]` 记已分诊 issue 号;**已 triaged 且结论为 needs-adjudication 的不重判**(留人工裁定,除非用户明示重审)
- `issue_cursor.pending[]`:上次执行中途失败遗留;**最多自动重试 1 次**,再失败转 escalate
- 仅新出现且不在 triaged 里的 issue 进入本轮分诊

## §2 PRD 对齐判定矩阵(label/标题 → SPEC 域锚点)

| 信号 | SPEC 锚点 |
|---|---|
| label `cdp` / title 含 CDP | REQ-CDP-001~008(`.spec/10-REQUIREMENTS.html` CDP 段) |
| label `node-compat` / `compatibility` / `runtime` | REQ-ENG-001~011 |
| label `servo` | REQ-BRW-001~003 |
| label `tests` / title 含 bench | `.spec/11-TESTING.html` 测试补齐类 |
| title `[cli]` | REQ-CLI-001~002 |
| title `[docs]` / label `documentation` | 无 REQ 域;docs 轻量 accept |
| stealth 类(TLS / 指纹 / canvas / fingerprint) | REQ-STL-001~007 |
| 无映射 / 新能力 | **escalate(SPEC 未定义禁编码)** |

**判据必须读 SPEC 文本核实锚点真实存在且语义匹配,禁凭 label 直接定论。**

## §3 三分判定

| 判定 | 判据 |
|---|---|
| **accept** | SPEC 有对应 REQ,且 issue 是其子集 / 澄清 / bug 报告;scope 可估:≤ 3 文件、非 vendor、非 BCE 10 文件清单 |
| **reject** | 已实现(给 `file:line` 证据)/ 重复(给 `#号`)/ 不适用(Bao 无对应结构,给理由) |
| **escalate** | SPEC 未定义 / 触 BCE 10 文件 / scope > 3 文件 / 语义不明 |

**禁假绿**:每个判定必须落到证据锚点(SPEC 段落或 `file:line`),禁「大概」。

## §4 英文回复模板(占位符 `<...>`)

### accept-progress(accept 且本轮未完成)

```
Thanks for the report. We've triaged this against <SPEC-REQ-ID> and confirmed it is in scope.

Plan: <one-line plan>.
This will land in an upcoming fix wave — the closing comment will reference the root-cure commit and test evidence.
```

### accept-close(root-cure 完成;风格对齐 wave-6 commit 2755f74f:技术性、证据数字、简洁)

```
Fixed by root cure in <commit-hash>.

Root cause: <technical root cause, one paragraph>.
Change: <what changed and where>.

Verification: `cargo nextest -p <crate>` — <N> passed, 0 failed; FAILED count = 0 across the full wave run (<M> test targets).

Closing as fixed. If you can still reproduce on <commit/version>, please reopen with your repro.
```

### reject-evidence

```
Thanks for the report. This does not appear to apply to Bao: <verdict>.

Evidence: <file:line / #duplicate-number / structural reason>.

If we've misread the report, please clarify <specific point> and we'll re-triage.
```

### needs-adjudication(不 close)

```
Thanks for the report. Initial triage assessment: <preliminary verdict>.

Reasoning: <reasoning anchored to SPEC text or missing coverage>.

Questions for the maintainer:
1. <question>
2. <question>

Leaving this open for adjudication — not closing.
```

## §5 关闭协议

- **仅 live 可 close / comment / label**;dry-run 只在报告写「将会做什么」,零 gh 写
- close 前置(全部满足):daily-ops §3 三重判据 PASS + commit 已 push
- commit message 列**全部**处理的 issue 号与各自根治摘要
- 重复 issue:同类已关 → reject 回复中引用已关 issue 链接(`#n`,closed by `<hash>`)
