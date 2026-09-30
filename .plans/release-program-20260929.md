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
3. **W4 #17-3 semver gate**:cargo-semver-checks 安装+workspace 接入(发布前自动 break 检测)——✅ 已落地(2026-09-29,e2):工具 0.50.0 入 ~/.cargo/bin;scripts/semver-gate.sh(发布面 crate 集动态探测+离线回退;major-required=硬 fail,0.x 语义;minor/patch=advisory;超时 SKIP 标注);local-ci 第 5 可选段(BAO_SEMVER_GATE=1 启用,默认 skip);docs/semver-gate.md;首跑报告 .claude/semver-gate-report-2026-09-29.md
4. **W5 #29 尾**:Zone reclamation 评估(M)+intentional leak ledger 正式化(M)——**已落地(2026-09-29,E1)**:`bench-harness zone-eval` 新模式(N≥100 实测:zone_count GC 后 102→2/302→2 全量回收 ✓;chunk 字节线性驻留 ~2.1 MB/realm 双 GC 不回落=**待裁决新发现**)→ `.plans/gc-leak-ledger.md` 七项正式表 + `LEAK_RAW_VALUE_ROOT_GUARD`/`LEAK_SHUTDOWN_ENGINE` 计数器(bao_engine);W7(heaptrack RCA)XDR SharedData 泄漏修复尝试无效已回退——**根因上移:死 realm zone 从不 GC(W5/W6 同源),SM realm-discard 面待上游 issue/裁决**,`.plans/soak-leak-rca.md` 全量归因
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

## BCE-20260930-PUBLISH-CLOSURE(发布链 5 失败类·第一性归因+横扫)
**根因(非增量症状)**:闭包集按**本地增量**(source-changed-since-baseline)计算,而 cargo publish 按 **registry 侧依赖图**解析——servo 族 lockstep 的真发布集由「registry 上仍 pin 旧 base 的全部 traits 族」决定,本地 touched 集只是其子集。逐 crate 补发=对系统性集合缺陷打增量补丁。
**次要根因(工具面)**:publish-closure.sh 三缺陷——①python3- stdin 挂(nohup 无 </dev/null)②execute 不按 topo 序(stealth 先于 bun_runtime)③closure 扫描漏 vendor servo+shared/media 深层路径。
**横扫(系统性,替代增量)**:W42=按 pin 图('bao-servo-base=0.5.7' 全递归 grep)确定 S 集→全族 z+1→级联 pin→三验(metadata/check/plan needs-bump=0)→fixpoint 发布。脚本三缺陷修随发布后硬化合同。
**残留=0 判据**:publish-closure --plan needs-bump=0 ∧ fixpoint 全 live(逐 crate curl 200)∧ bao-core 0.3.2 live。

## 流程事故记录 2026-09-30(共享树纪律,同日双犯既有规则)
1. **w24 误 checkout -- vendor/servo**(把主会话的 pin 扫尾残留当自己半状态清了;零损失实证但操作违规)——规则「回退限定自域路径,整体 checkout 禁用」(2026-09-28 立)再犯。强化:非我 M 禁 checkout,只能问归属。
2. **主会话三次 pathspec 盲区**(-A 宽收卷入在途/`**` glob 不达深层/lock 非 toml 扩展名漏)——规则「多执行体在途期宽域 add 禁用」的自我违反。强化:add 后必 `git status --short` 复核 staged 面;深层树用 find -name 显式清单,禁 glob 猜。

## BCE-20260930-WAVE-CLOSE(波末 6 错类·第一性归因)
**根因三类(机械证据)**:
1. **共享树竞态**(W43 产品面丢失/两次 battery 撞在途编辑):验证链与 E 编辑并发=中间态捕获。已制度化:验证只在树静点。
2. **commit pathspec 盲区**(×4:Cargo.lock×2/packages 双树/examples/.github):根因=「多执行体期禁宽域 add」纪律在**单会话收尾期被错误延续**——树全属己时 scoped add 反而是 bug。**机制修正:波末树静点一律 `git add -A`+staged 面复核;scoped pathspec 仅限并发期**。
3. **吸收波集成涟漪**(SW 类型/webgl 死臂/weakref assert):波门全数捕获(battery v10 11238/11238 全绿=网有效);6 错类=**门的命中数,非逃逸数**。
**残留=0 判据**:battery v10 全绿(已达)+发布闭包 residual=0(W46 后验)。
- **增补(发布链第 7-8 错类)**:四轮 straggler 的统一根因定谳=**同号漂移对账盲区**——W39/W42/W47 各轮对账均核「本地 pin↔本地 manifest」,漏「本地↔registry 已发布 manifest」;same-version 内容漂移(default-resources 0.5.8 embedder pin 差异=典型)不触发 bump→发布期解析冲突连环。横扫=W48(registry-manifest diff 全集扫描,漂移者 z+1,迭代至复扫零)。残留=0 判据=registry-diff 零+publish converge ALL-DONE。

## 2026-09-30 / 并行写者事故+裁决(吸收波发布段)
- **事故**:registry 版本线被第二写者推进(net 0.5.22-27/core 0.3.5-0.3.8,18:34-19:15 窗口;疑 gsc-0d 交互 peer 从含本会话工作的树上推线);本会话 e1 按双写者纪律停手,主会话冻结发布线。
- **用户裁决 A**:registry 线=权威。执行:9 crate 版本回拨 registry 锚+双形 pin 级联(target/ 毒源排除+command grep 教训)+registry-diff 零漂移+五 crate check 绿。
- **exact-pin 环死锁**(layout-api⟷script-traits 家族固有拓扑):裁方案 A 三步引导(临时针→对侧→恢复针,公开瞬态自愈)。
- 教训沉淀:①并行会话同 registry 命名空间=协调事故级,冻结+事实+用户裁②script/target/package/ 打包副本会毒化版本映射③ugrep 桥再犯(command grep 铁律)。
- **归因修正(22:47 覆写)**:gsc-75 实证清白(全程零 bao 路径写);当前 layout 针=0.5.9 临时态+mtime 22:50=e1 防覆写重放生效中。22:47 的 =0.5.10 疑=e1 自身早前 reconcile 迭代自碰撞(僵尸进程检查只覆盖活进程,串行队列内迟到迭代不可见)。历史线推手(18:34-19:15)仍未识别(机上多 claude 实例),裁决 A 下已无实际影响(registry=权威)。e1 继续独占执行。
