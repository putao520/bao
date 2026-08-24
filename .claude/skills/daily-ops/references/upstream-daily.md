# 上游轻量日同步(daily-ops 自动线)

daily-ops §1 阶段 2(窗口扫描)与阶段 4(吸收波)的上游操作细则。任意窗口与 BCE patch 重放均在本协议内自动执行(2026-08-24 用户裁决扩权);仅 mozjs 跨版本升级归交互会话 `upstream-absorb` skill。

## §1 分工表

| 条件 | 路线 |
|---|---|
| 任意窗口、含触 BCE patch-replay 文件 | 本协议(daily-ops 内自动,2026-08-24 用户裁决扩权) |
| mozjs 跨版本升级(vendor 重建 + rlib 重编 + 5 项 BAO patch 重放) | escalate → 交互会话(用户裁决 2026-08-24) |

BCE 清单锚点 = 项目 CLAUDE.md「servo 定制文件清单(11 个)」。**触其一 → 按 §5 重放协议自主执行。**

## §2 轻量流程

1. **fetch**(只 fetch,SKILL.md §0 铁律):两 clone `git fetch origin`,单侧失败独立重试 1 次
2. **读基线**:`.claude/upstream-baseline.json`(读不到 / 字段缺失 → `BASELINE_FILE_INVALID` fail-closed)
3. **窗口计量 + 分类**:`git rev-list --count <baseline>..origin/main` 取实数;按目录 `git log --stat` 分类,install / CI / windows 类整批跳过
4. **三招自动过滤**(第一道过滤器,先跑再人工):
   - **hash 前置 grep**:Bao 有在注释里引用上游 hash 的惯例,`command grep -rl "<hash>" src packages vendor` 命中即「已含」秒判
   - **pre-fix 指纹比对**:取 diff **删除行**形状(标识符 / 常量 / 条件),grep Bao 对应目录,命中即「同 bug 确认(吸收)」;不命中再看形状差异
   - **窗口规模校正**:以 rev-list 实数规划派工,**禁「约 N 个」估算**
5. **四类判定**(每项落其一):吸收 / 已含 / 不适用 / 需进一步判断(证据不足**不猜**)
6. **派工**:吸收项 ≤ 5 → 派 1-2 个 E;E 合同(scope / completion / retry / stop)继承 upstream-absorb skill §3 模板;**先读 diff 禁凭记忆移植**;编译错修 1 次;形状差异过大 → 停报告不猜

## §3 vendor 同步安全(仅当波内含 vendor 改动)

- wave 前置:bao 工作树必须干净(脏 → 全程 `DIRTY_TREE` escalate,禁混入他人改动)
- 中断恢复窄条件:改动**全部**在 `vendor/` 且无 untracked → 允许 `git checkout -- vendor/` 受控恢复;否则留人工,禁扩大恢复范围
- 批量同步后 `touch` 全树(rsync/cp 保留旧 mtime → cargo 复用 stale 产物陷阱)
- usockets 改动:`packages/` csrc 与 `src/uws_sys` 双树同步
- vendor 枚举 / 类型改名:横扫 bao 层全部调用点
- webidl 联动:DOM `.rs` 同步必须伴随 `webidls/` 同步

## §4 波末

- 基线文件 bump(`baseline` + `updated_at`)入 **wave 同一 commit**;message 列全部 hash + 判定摘要(样板 = commit `6c6ffd38`)
- 「需进一步判断」残留项逐条登记进报告,**不静默丢弃**

## §5 BCE patch 重放(自主线)

- **清单真源**:项目 CLAUDE.md「servo 定制文件清单(11 个)」+「mozjs fork BAO patch 清单(5 项)」;上游同步时先 `command grep -rln "BCE-\|BAO " vendor/servo/components/` 重建清单
- **重放纪律**:upstream 基底 + patch 精确重放(patch 锚点与完整记录在 git log 各 stage commit message,`git show <old>:vendor/servo/...` 取旧版对照);上游版与 Bao 补丁版冲突时**Bao 补丁语义恒胜**(如 handle.rs 的 JSEngineSetup 幂等 init,禁用上游裸版)
- **派工合同/陷阱库/收口协议**:全部继承 upstream-absorb skill §3/§4/§5(派工模板、并发零重叠、波末单 commit、基线 bump 同 commit、发布闭包)
- **失败语义**:差异过大/语义不明 → stop 报告不猜;中途态保留禁自动 reset;pending 次日重试 ≤1,再失败 escalate(SKILL.md §4 不变)
