# 发版前全量程序(用户裁决 2026-09-29:「全部做完再发版」)

> 输入:/tmp/inv14-queue.md(90 项逐项表)。本文件=执行波次 SSOT。
> 槽位纪律:≤2 并发子 Agent(用户既有指令);文件域互斥;交付即 commit(C 收口)。

## 已完成

| 波 | 内容 | 证据 |
|---|---|---|
| W0 构建门 | cargo build --workspace RC=0(10m58s) | /tmp/rel-build-full.log |
| W0 soak 伪影定性 | 09-27/28 连败=WIP 树编译死,非产品回归 | 空结果目录+rc=101 |
| W1 #30 drift 首跑 | 1106 symbol,+29;removed_bao_used=3 全注释假阳;7/7 patch Survived | 9e90f270 |
| W-fix E0063 L1 | cdp-server 69 处+BAO-DIAG 清零 | 5c6cc86c |
| W3a 平台矩阵 | Win=Supported/macOS=Experimental 三态落地 | docs commit+#18 评论 |
| 裁决三件 | BrowserRuntime/NodeRuntime 收口;Win/mac 矩阵;console 混合 | 7af9efe1 |

## 在途

| 波 | 执行体 | 内容 |
|---|---|---|
| E0063 L2 | e2 | bao_cdp 79 处+全仓 pattern 预扫 |
| W2 重命名 | e1 | BrowserRuntime/NodeRuntime+deprecated alias |
| soak 重跑 | systemd | 60min 计时中(计 streak) |

## 队列(依序,槽位空即派)

1. **电池 v3**(e2 后,主会话 V):xvfb nextest test-ci 全量 → 绿后链 msvc cross check(task #14)
2. **W3b console 混合作用域**(e1 后,bao_runtime 域空出):实证页面 realm console.time 路由(servo dom console per-global vs node_console 进程全局)→ 按裁决实现/验证+跨 realm 隔离测试+CLI 全局测试
3. **W4 #17-3 semver gate**:cargo-semver-checks 安装+workspace 接入(发布前自动 break 检测)
4. **W5 #29 尾**:Zone reclamation 评估(M)+intentional leak ledger 正式化(M)
5. **W6 G1-Node inventory**(XL):锚 node v24.19.0 本地(72 builtin)逐模块 exports diff vs bao 面;分类矩阵+聚合+`bao compat node` 报告
6. **W7 G1-Bun inventory**(XL):锚 ~/code/rust/bun/src/js+Bun.* 全表面 diff;四高优先项(file/serve/spawn/write)量化+分类
7. **W8 G1-CDP/Web 矩阵**:CDP 12 域 method 矩阵+`bao compat cdp`;Web WPT 子集首跑+`bao compat web`
8. **W9 #15 Runtime 域**:page 状态机显式化(L)/shutdown 序测试(M)/取消终态横扫(M)/Send-Sync 合同(M,W2 后)
9. **W10 #19 bench 族**:并发 page stress 10/100/500(L,Gate B 硬缺口)+mixed soak 场景(L)+REPORT 生成器+回归门
10. **W11 #18 剩余**:consumer 三路 gate 自动化/doctor 依赖检测/nightly bump 门/bootstrap 复现/发布闭包自动化
11. **W12 #26 stage2 XDR persistent cache**(M-L)
12. **W13 #31/#32 尾**:wss fallback 加固/spawn_sys signal-reset/row19 复核/B2 gap table 收口
13. **W14 soak 梯队**(事件):streak≥3→ratchet≥6 探针→24h→mixed 72h(依赖 W10 场景落地)
14. **收口**:四 README TBD=0、#21 四 Gate 证据对账、全量电池终跑、BCE 门、版本 bump+发布闭包(仿 e18b7158 流)、tag

## 发版事实(已去险)

- daily-ops 已有成熟发布闭包流(e18b7158=49 crate,09-27);多数 crate 已沿途 patch 发布(io=local)
- 待发布 delta=程序波次全部落地后的一次闭包+伞 bump
- bao-stylo-atoms 上游名待核(io 空查,可能未发布/别名)

## 用户裁决转录队列(PRD 专用工具待可用)

1. BrowserRuntime/NodeRuntime 收口(7af9efe1)
2. Win=Supported/macOS=Experimental 矩阵(7af9efe1+docs commit)
3. console 混合作用域(7af9efe1)
