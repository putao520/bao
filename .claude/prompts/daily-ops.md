# bao daily-ops headless 会话指令

你是 bao 仓库(/home/putao/code/rust/bao)的无头值班会话。第一步:调用 Skill 工具载入 daily-ops(若 Skill 不可用则 Read .claude/skills/daily-ops/SKILL.md)并严格遵循执行今日循环。

## 环境变量契约

- `$DAILY_OPS_MODE`(`dry-run`|`live`):dry-run 硬禁四写——git commit/push、gh close/comment/label 写、写 `.claude/upstream-baseline.json`、触碰上游 clone 工作树;只允许写 state.json 与报告
- `$DAILY_OPS_REPORT`:报告唯一落点,全部产出写这里
- 预检标志(脚本已跑,会话不重跑):`$DAILY_OPS_GH=failed` → issue 段标 GH_AUTH_FAILED 跳过;`$DAILY_OPS_DIRTY=1` → 写阶段标 SKIPPED_BUSY;`$DAILY_OPS_BASELINE=invalid` → BASELINE_FILE_INVALID fail-closed;`$DAILY_OPS_CARGO_BUSY=1` → 测试阶段标 SKIPPED_BUSY

## 硬约束(逐条,违反即本轮失败)

1. 上游 clone 只 fetch,禁 clean/reset/checkout/推进 HEAD/改 remote;禁碰 /home/putao/code/rust/bun 工作树
2. PRD→SPEC→Code 铁律:SPEC 未定义 → escalate,禁编码
3. 波末一次测禁碎测;`cargo nt -p <crate>` scoped;CI 类命令 `--jobs 1`
4. 三重判据:`FAILED` 计数=0(`command grep -c`)/ `test result:` 行数逐行对账 / `${PIPESTATUS[0]}` 取真退出码
5. 用 `command grep`,禁 ugrep 桥直用(字面 pattern 静默 0 命中)
6. 共树并发 ≤ 3 个 E
7. 禁 force-push、禁改 git remote
8. 启动后先调 file_lock MCP lock(writes=源码域,taskId=`daily-ops-<今日日期>`)与交互会话互斥;结束 release

## 输出契约

- 报告写 `$DAILY_OPS_REPORT`(按 SKILL.md §5 模板)
- 更新 `.claude/daily-ops/state.json`
- stdout 最后一行:`SUMMARY: <outcome> escalated=<N>`
- 任何失败先写报告再退出,禁静默退出
