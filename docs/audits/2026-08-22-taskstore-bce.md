# 审计报告:任务存储终态回收歧义 + E 越权任务操作(BCE 闭环记录)

- 日期:2026-08-22 | 触发:bce-domain-guard(错误类失败×3,spec-gov 域) | 状态:confirmed(防复发已落地)
- 现象:同会话两次 TaskUpdate "Task not found"(taskId=28/#32),均发生在 E 子 agent 回报「已自行标记 completed」之后;TaskCreate 序号连续(#33 紧接)证明存储健康
- 根因(双层,SOL 归因):
  1. 流程层:E 越权 TaskUpdate(status=completed) 提前终结条目,违「E 只执行、状态收口归 C」边界(CLAUDE.md 硬门②)
  2. 工具层:TaskUpdate 错误语义不区分 never-existed vs terminal-pruned;TaskList 不保留终态条目,C 无法核验 E 回报真伪
- 泛化类:E self-claim/自评同族;共享存储终态生命周期错配(同构先例:.id-registry 墓碑号拒绝重用、file_lock stale 歧义)
- 横扫结论:全会话 E 合同模板自本日起增补第 5 要素「禁任务系统工具」;wave7 的 TEST-BUG-ENG-370 悬挂引用为同域邻接问题,独立立案(见任务登记)
- 防复发(本 commit 落地):CLAUDE.md 硬门②第 5 要素 + daily-ops prompt 第 10 条
- 上游建议(不在本仓 hack):TaskUpdate 区分性错误码;TaskList 保留终态条目
- spec_write 工具缺陷记录(2026-08-22 本会话实证):bug create 两形态失败("Bug requires id" → 补 id 后 ok=true 但本体未写入,仅 registry/00-INDEX 副作用)——建议 gsc-spec 维护方排查 bug 元素写入路由(疑似非法宿主静默丢弃)
