---
name: daily-ops
description: 每日运维值班:上游同步 + GitHub issue 分诊处理(putao520/bao)。当用户说"每日同步/daily ops/处理 issue/issue 值班/issue 分诊/日常巡检/定时同步",或 headless 定时任务(systemd timer)执行时使用。自动边界:全自主:任意窗口吸收(含 >20 与 BCE patch 重放)+ mozjs 跨版本升级(§9 长任务协议)+ issue 根治(任意 scope)+ 波末验收 + commit/push/关 issue + 发布闭包;超范围 issue 直接 reject+close(安全门禁);语义不明/PRD-SPEC 冲突/重试耗尽/长任务单轮 7 天预算耗尽 → 升级人工。
---

# 每日运维值班(daily-ops)

无头定时执行(systemd timer)或手动触发的三条值班线:**上游同步**(细则:`references/upstream-daily.md`)+ **GitHub issue 分诊处理**(细则:`references/issue-rules.md`)+ **发布收尾**(细则:`references/publish.md`)。本文件是编排契约与硬边界;三份 reference 是各自域的操作细则,执行对应阶段前必读。

## §0 运行契约

### MODE 语义(唯一真源 `$DAILY_OPS_MODE`)

| MODE | 语义 |
|---|---|
| `dry-run`(缺省) | 只读演练。**硬禁四写**:① `git commit`/`git push`;② 一切 gh 写(close/comment/label);③ 写 `.claude/upstream-baseline.json`;④ 触碰上游 clone 工作树。仅允许写 `state.json` 与 reports |
| `live` | 真执行:吸收 wave 单 commit + push + gh 英文回复/关 issue + 基线 bump 同 commit |

- MODE 由 `scripts/daily-ops.sh` 注入;未设置按 dry-run 处理(fail-safe)
- 脚本侧护栏:dry-run 结束后核对 bao HEAD,动了即报告 `VIOLATION`

### 预检环境变量(脚本注入,会话**不重跑预检**)

| 变量 | 语义 | 会话动作 |
|---|---|---|
| `$DAILY_OPS_REPORT` | 报告唯一落点 | §5 报告只写这里 |
| `$DAILY_OPS_GH=failed` | gh auth 预检失败 | issue 段标 `GH_AUTH_FAILED` 跳过 |
| `$DAILY_OPS_DIRTY=1` | bao 工作树脏 | 只读阶段照常;写阶段标 `SKIPPED_BUSY` |
| `$DAILY_OPS_BASELINE=invalid` | 基线文件缺失/字段缺失 | 全程 `BASELINE_FILE_INVALID` fail-closed 升级 |
| `$DAILY_OPS_CARGO_BUSY=1` | 有 cargo 进程运行(锁可能被持有;脚本侧已有限等待 ≤30min,注入标志时已等待耗尽) | 测试阶段标 `SKIPPED_BUSY`,禁等锁 |
| `$DAILY_OPS_PUBLISH=failed` | `CARGO_REGISTRY_TOKEN` 缺失 | 发布收尾标 `SKIP_PUBLISH_TOKEN` |

### 上游 clone 铁律

- `~/code/rust/bun` 与 `~/code/tools/servo` **只允许 `git fetch origin`**;禁 clean/reset/checkout/推进 HEAD/改 remote/碰工作树
- bun clone 工作树脏是**已知正常态**,不是异常信号,禁"修复"它

## §1 每日循环(8 阶段)

1. **预检消费**:读 §0 环境变量确定各阶段可用性;`BASELINE_FILE_INVALID` 直接走升级报告
2. **上游窗口扫描**:两 clone 各自 `git fetch origin`(单侧失败独立重试 1 次);baseline 从 `.claude/upstream-baseline.json` 读;`git rev-list --count <baseline>..origin/main` 得窗口数
3. **issue 分诊**:`gh issue list --repo putao520/bao --state open --json number,title,body,labels`,按 `references/issue-rules.md` 判定
4. **执行波**:吸收 wave(§2 边界内,含 BCE 重放)/ issue 修复 wave;零工作量(窗口=0 且无可做 issue)→ no-op clean
5. **波末验收**:§3 三重判据,任一不过 → `VERIFICATION_FAILED`
6. **收尾**:**live 才** commit + push + 关 issue + 英文回复;dry-run 只在报告写「将会做什么」
7. **报告+state**:报告写 `$DAILY_OPS_REPORT`(§5 模板);更新 `state.json`(`last_run` + `issue_cursor`)
8. **发布收尾**:前置四条件(缺一即 `SKIP_PUBLISH` 并记原因:① MODE=live ② 本日有 wave commit ③ §3 三重判据 PASS ④ push 成功);满足 → 按 `references/publish.md` 发 cargo 变更闭包 + GitHub daily release;结果回写报告「发布」段

## §2 auto / escalate 边界表(硬门,逐行)

| 条件 | 动作 |
|---|---|
| 上游窗口 = 0 | no-op clean |
| 窗口任意规模(含 >20) | 自主吸收:rev-list 实数规划派工(禁估算),细测 references/upstream-daily.md |
| 触 11 个 servo BCE patch-replay 文件之一 | **自主 patch 重放**(references/upstream-daily.md §5;upstream 基底 + patch 锚点精确重放;差异过大 stop 报告不猜) |
| 吸收判定项 ≤ 5 | 派 1-2 个 E(共树并发 ≤ 3) |
| 吸收判定项 > 5 | 自主分批:每批 ≤ 5,上一批波内验收过再派下一批 |
| issue:SPEC 有 REQ 映射 + 范围内 | 修复 wave(C-E-W-V;scope 任意,触 vendor 按 upstream-daily.md §3 安全协议) |
| issue/吸收:mozjs 跨版本升级 | **自主(长任务协议 SKILL.md §9)**:单轮 7 天预算,耗尽 escalate |
| issue:无映射(判明超范围) | **reject+close(not planned)+ 英文理由(安全门禁,仅 live)** |
| SPEC 未定义 / 与主任务・PRD・SPEC 冲突 | **reject+close(not planned)+ 英文理由回复(安全门禁,仅 live)**;仅语义不明才 escalate |
| issue 已实现 / 重复 / 不适用 / 超主任务范围(门禁) | reject + 证据/理由回复 + close(仅 live) |
| 验收 PASS | 收尾(§1 阶段 6) |
| 验收 FAIL | 不关不 push,评论进展 |
| bao 工作树脏 / cargo 锁(脚本等待 30min 后仍 busy) | 只读阶段照常,写/测阶段标 `SKIPPED_BUSY` |
| pending 重试 1 次仍失败 | escalate 留人工(§4 不变) |
| 发布收尾 | 四条件满足 → 按 `references/publish.md` 发 cargo 变更闭包 + GitHub release | 任一不满足 / 限流外失败 → `SKIP_PUBLISH` 登记 pending 次日重试 |

## §3 波末验收三重判据(硬编码,任一不过 = `VERIFICATION_FAILED`)

1. **FAILED=0**:测试输出中 `FAILED` 出现次数 = 0,用 `command grep -c`(本机 `grep` 是 ugrep 桥,**禁直用**,引号逗号字面 pattern 会静默 0 命中)
2. **test result 对账**:输出中 `test result:` 行数 = 实际跑的 test target 数,逐行对账;禁 head 截断、禁只数绿行
3. **真退出码**:管道取 `${PIPESTATUS[0]}`,禁取管道末位命令退出码

测试纪律:`cargo nt -p <crate>` scoped;CI 类命令 `--jobs 1`;**波末一次测,禁碎测**。

## §4 失败处理

- 各阶段**独立 fail-closed**:上游失败不拖垮 issue 段,反之亦然
- 单侧 fetch 重试 1 次仍失败 → 该侧标 `SKIPPED_<UPSTREAM>_FETCH_FAILED`,另一侧照常
- gh 失败 → `GH_AUTH_FAILED`,跳过 issue 段
- 修复波中途失败 → 不 close issue;live 下评论进展;**代码中途态保留,禁自动 reset**;`state.json` 记 pending,下次最多自动重试 1 次,再失败转 escalate
- **报告必产出**:任何失败路径都要先写报告再退出,禁静默退出

## §5 报告模板(写 `$DAILY_OPS_REPORT`)

```
# daily-ops report <YYYY-MM-DD>
MODE: <dry-run|live>

## 8 阶段 outcome
预检/扫描/分诊/执行/验收/收尾/报告/发布:各一行结论(OK|SKIPPED_*|FAILED_*)

## 上游窗口
- bun: <N> commits(吸收 <a> / 已含 <b> / 不适用 <c> / 需进一步判断 <d>)
- servo: <N> commits(同上四类计数)

## issue 分诊表
| # | 判定 | 依据(证据锚点) |

## 验收证据
- 命令 + 退出码
- (a) FAILED 计数 (b) test result 行数对账 (c) PIPESTATUS

## commit
<hash + 一句话>(如有)

## 发布
- crate 清单 + 版本 + tag + release URL;或 SKIP_PUBLISH + 原因

## 升级项
- <逐项:原因 + 建议去向>

SUMMARY: clean|acted|failed|timeout escalated=<N>
```

`SUMMARY` 行必须是 stdout 与报告的最后一行。

## §6 cron 契约

- 执行链:`bao-daily-ops.timer`(每日 06:07 ± 10m,Persistent 补跑)→ `bao-daily-ops.service` → `scripts/daily-ops.sh`(flock 单飞 / 预检注标志 / `timeout 604800s(7 天,用户裁决 2026-08-24)` / dry-run 违规护栏)→ `claude -p "$(cat .claude/prompts/daily-ops.md)"` → 载入本 skill
- MODE 切换:`.claude/daily-ops/mode.conf` 写 `MODE=live`(gitignore 本地文件,不进 git)
- 同日重跑:报告文件名加 `.HHMM` 后缀,不覆盖已有报告
- 切 live 门槛:**连续 ≥ 5 天 dry-run 零违规 + 用户逐份审阅报告**

## §7 与 upstream-absorb 分工

本协议自动执行任意窗口、BCE 重放与 mozjs 跨版本升级(按 `upstream-absorb` skill 固化规则;mozjs 走 §9 长任务协议);仅两类留交互:

| 场景 | 路线 |
|---|---|
| 吸收「需进一步判断」项 | 交互会话(不猜原则) |
| issue 语义不明 / PRD-SPEC 冲突 | 交互会话(宪法裁定权在用户) |

## §8 基线契约

- **SSOT = `.claude/upstream-baseline.json`**(bun/servo baseline hash + updated_at)
- 更新时机 = 仅吸收 wave commit 内 bump(**同 commit**,不单独提交)
- 读不到 / 字段缺失 → `BASELINE_FILE_INVALID` fail-closed 升级,禁继续
- **禁从 memory 读基线**;auto-memory 只保留方法论(陷阱/教训),不存 hash

## §9 长任务协议(用户裁决 2026-08-24)

- **适用**:mozjs 跨版本升级等单轮 7 天预算内持续推进的长任务
- **state.json 新增 `long_running` 字段**(schema:`{task, started, deadline, phase, notes}`);会话起手检查:有 long_running → **优先继续**(单轮 7 天预算内连续推进,不分日分片),无才走常规 8 阶段
- **单轮 7 天预算(MAX_SECONDS=604800)耗尽 → timeout 中止,中途态保留,escalate 留人工**,报告登记当前 phase 与已完成证据;轮间中断(超时/失败)→ long_running 字段记录 phase,下轮优先续跑
- **完成** = 基线 bump 同 commit + §3 三重判据 + 发布闭包,清空 long_running 字段
- **失败语义**:完全继承 §4 / pending 重试契约(用户裁决 2026-08-24 确认)
