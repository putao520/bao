# 发布收尾协议(daily-ops 阶段⑧)

daily-ops §1 阶段 8(发布收尾)的操作细则。**前置四条件(缺一即 `SKIP_PUBLISH` 并记原因)**:① MODE=live ② 本日有 wave commit ③ 三重判据 PASS ④ push 成功。教训全量来自发布工程实录(memory:crates-io-publish-ready-state / crates-io-published-final,170/170 上库 + 八类教训)。

## 版本策略

- **变更闭包**:本波被触 crate 的 workspace 依赖闭包(`cargo metadata` 计算),闭包内逐 crate patch bump(fix/absorb 语义)
- **CLI 永不发布**:`bao_bin` / `bao_cli` 为既定裁决不发布,不在闭包内
- **版本号唯一性**:已用号禁复用;失败重试靠新 patch 号,或同号幂等(先 curl registry 查验已上库则跳过)

## cargo 发布序

1. **预检**:`CARGO_REGISTRY_TOKEN` 存在(env 缺失 → `SKIP_PUBLISH_TOKEN`;脚本已注入 `$DAILY_OPS_PUBLISH=failed` 标志,会话不重查)
2. **变更闭包计算**:`cargo metadata` 拓扑,产出发包清单
3. **逐 crate 发布**:
   - 元数据自查:description/license 缺失 = 上传 API **硬拒**而 dry-run 只警告——发布前必查
   - `cargo publish --dry-run`(隔离验证:feature 并集幻觉类教训;凡 build.rs/codegen 生成物缺失,先查 webidl skip-unless 门)
   - 正式 `cargo publish`
   - **上库查验:curl registry 返回 200**(发布验证以 curl 200 为准,非命令退出码独断)
4. **dev-dep 环**:剥离 → 发布 → 恢复模式(workspace 内 dev-dependency 循环时)
5. **限流**:报文带精确解除时刻 → **锚定等待,勿盲退避**;新 crate 速率限制同

## GitHub release

- 前置:全部 crate 发布成功(curl 200 验讫)
- `git tag daily-<YYYYMMDD>`(当日已有则 `-2` 后缀递增)
- `gh release create`,notes 取当日 wave commit message 全文
- tag 仅在有变更日打,无变更日不打

## 失败语义

- 限流等待内完成 = 正常路径
- 其他失败 → `state.json` 登记 `publish_pending`(crate + 版本 + 失败原因);次日 daily-ops **优先重试**(幂等:先 curl 查验,已上库则跳过)
- 发布失败**不影响**已完成的主链收口报告(阶段 1-7 结论保持)

## 波内纪律

- 发布操作由 daily-ops 会话**派 E 执行**(合同四要素 scope/completion/retry/stop + 本协议为 scope 附件)
- **禁自评「发布成功」**:以 curl registry 200 为准
