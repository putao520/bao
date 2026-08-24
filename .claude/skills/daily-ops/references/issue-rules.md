# issue 分诊规则(putao520/bao)

daily-ops §1 阶段 3(issue 分诊)与阶段 6(收尾回复/关闭)的操作细则。目标仓库:`putao520/bao`。

## §1 入口与游标

```bash
gh issue list --repo putao520/bao --state open --json number,title,body,labels
```

- `state.json` 的 `issue_cursor.triaged[]` 记已分诊 issue 号;**已 triaged 且结论为 needs-adjudication(仅语义不明通道)的不重判**(留人工裁定,除非用户明示重审)
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
| 无映射 / 新能力(判明超范围) | **reject+close(安全门禁主路径)** |

**判据必须读 SPEC 文本核实锚点真实存在且语义匹配,禁凭 label 直接定论。**

**安全门禁(用户裁决 2026-08-21)**:凡判明超出主任务(PRD 六域 ENG/CLI/BRW/CDP/STL/LIB +「反指纹浏览器运行时」愿景)的 issue——新功能诉求、无关集成、与主任务无关的一切——一律 **reject+close(not planned)+ 英文理由回复**,不留人工、不放进升级通道;仅语义不明无法判定时才 escalate。**禁用 escalate 兜底明显的范围外诉求**(防门禁被绕过)。

## §3 三分判定

| 判定 | 判据 |
|---|---|
| **accept** | SPEC 有对应 REQ,且 issue 是其子集 / 澄清 / bug 报告;scope 任意(2026-08-24 扩权)——大 scope 分批 C-E-W-V,触 vendor 按 upstream-daily.md §3 安全协议 |
| **reject-and-close(门禁主路径)** | 四类:① 已实现(给 `file:line` 证据);② 重复(给 `#号`);③ 不适用(Bao 无对应结构,给理由);④ **超主任务范围(门禁,用户裁决 2026-08-21)**——超出 PRD 六域 / 主任务愿景、与主任务或 PRD/SPEC 直接冲突、无映射且判明是新能力诉求 |
| **escalate(收窄)** | 仅三类:语义不明(读不出是否在范围内 / 证据不足无法判定)+ 工程类 mozjs 跨版本升级(用户裁决 2026-08-24)+ pending 重试耗尽 |

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

### reject-evidence(已实现 / 重复 / 不适用)

```
Thanks for the report. This does not appear to apply to Bao: <verdict>.

Evidence: <file:line / #duplicate-number / structural reason>.

If we've misread the report, please clarify <specific point> and we'll re-triage.
```

### reject-out-of-scope(超主任务范围,门禁主路径)

```
Thanks for taking the time to file this. After triage, this falls outside the scope of Bao's current mission: <one-line scope basis, e.g. "Bao is an anti-fingerprinting browser runtime (PRD domains ENG/CLI/BRW/CDP/STL/LIB), and this request is <why it does not map>">.

We are not planning to extend Bao in this direction, so closing as not planned.

If you can reframe this within the current scope — e.g. <adjacent in-scope angle, if any> — we'd welcome a new issue.
```

### needs-adjudication(仅语义不明;**禁用于明显范围外**)

```
Thanks for the report. We were unable to determine whether this falls within Bao's current mission scope.

What we see: <what the issue appears to request>.
Why it is ambiguous: <specific ambiguity>.

Questions for the maintainer:
1. <question>
2. <question>

Leaving this open for adjudication — not closing.
```

## §5 关闭协议

- **仅 live 可 close / comment / label**;dry-run 只在报告写「将会做什么」,零 gh 写
- 代码类 close 前置(全部满足):daily-ops §3 三重判据 PASS + commit 已 push
- **门禁类 reject+close 前置**:无代码改动,**不需三重判据**;仍仅 live;close 理由必须引用范围依据(主任务愿景 / PRD 域)
- commit message 列**全部**处理的 issue 号与各自根治摘要
- 重复 issue:同类已关 → reject 回复中引用已关 issue 链接(`#n`,closed by `<hash>`)
