# Issue 根治波续跑计划(2026-09-10 16:25 配额中断交接)

> 背景:用户指令「大量 ISSUE 根治」。今日已关 #16/#37/#10/#39(4 件),SM-EVOLUTION 8 slices 落地(S0/S1/S1-续/S2/S2-续/S2-续2/S2-续3/#28/#27 审计),#30 自动化,#19 Phase A,#32 Phase 0,R53 提案,CSP 类闭口(4ad23d61)。
> 16:21 配额阵亡:e85(Debugger 保真)/e83(soak)。**18:33:38 重置**。

## ① e85 遗产续跑(Debugger 裁决 2/3/9 落地,实现已在树)

- 树内:cdp_handler.rs+delegate.rs+protocol.rs+bao_engine/lib.rs(M)、bun_sm/debugger.rs(D)、bun_sm/lib.rs(M)——死于编译检查前,形态未验
- 合同:见账本 §8 #27 节裁决 2/3/9(六缺陷项:callFrames 恒空/硬编码 location/假 API offsetLine→getLineOffsets/逐行 possibleBreakpoints→原生/blackbox 假成功→显式 unsupported/getEnvironment;+死代码删除)
- 续跑:①git diff 全面复核六项是否完整(blackbox 显式化/delegate/protocol 改动面可能是事件面接线)②编译③live 保真测试(断点命中帧非空+location 真值)④回归(cdp 域+374/1213 基线)⑤账本裁决 2/3/9 状态更新⑥commit+push
- 注意:若实现不完整,按裁决补齐而非重写;retry 2

## ② e83 soak 数据收账(进程已独立存活至 ~16:55 自落盘)

- bench-harness soak 60min 进程 15:55 起(PID 3214308),数据落 bench/results/2026-09-10-d644e5df/soak.run-1.json(+ab114dce 目录)
- 续跑:①收数据(分段指标)②判词三件:线性斜率外推/突变段/回落性,对照 S2 必要 bounded leak 清单③账本 soak 节+72h 调度建议(systemd timer 或 daily-ops 接力)④commit
- 若进程异常死:重跑一轮 60min(基线方法在 ab114dce)

## ③ 后续未启动面(重置后按序)

- #26 Stencil bench-gated 波(Phase A 基线已可消费)
- SM-EVOLUTION 全审计已毕(#23-25/27-30),程序进入消费/立法面
- **用户裁决队列**(累计,回来即问):RED-1(P-A/P-B)/R53(A/B/C+第零项)/#28 locale+tz+时间精度立法(StealthProfile 扩字段)/CLI --timeout 立法/#10 纯静态产物产品裁决/59-crate lockstep 归下轮 scheduled wave
