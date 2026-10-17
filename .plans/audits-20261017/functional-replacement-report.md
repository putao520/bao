# bao 功能级库替换测绘报告(手写功能模块 → 成熟薄库全谱)

> 只测绘,证据制。工作目录 /home/putao/code/rust/bao。日期 2026-10-17。
> 口径:「整块手写功能模块 → 成熟 crate」功能级替换(非 syscall 级);修正前审 thin-lib-audit 的 3 条断言(§0)。
> 平台前提:bao 官方矩阵 = Linux + Windows(macOS 仅交叉探针级)。
> 执行体:e-functional-replacement;主会话代落盘(READ-GATE 拦截登记)。

## §0 前审断言修正(3 条)

| 前审断言 | 实测 | 判定 |
|---|---|---|
| 「network-interface 有 MAC 无 CIDR」 | network-interface 2.0.5:V4IfAddr/V6IfAddr 两族都给 netmask + mac_addr + index + internal;CIDR 可由 netmask popcount 派生 | 断言错误;但仍不足以替换(真差集见 #4) |
| 「sysinfo/procfs 过重 → 无薄库」 | sysinfo latest 支持 Linux+Windows,load_average/cpus/total_memory 齐全 | 有此库,但替换面仅 ~60 行,依重不成比例,keep |
| 「signal-hook 只能替换一小截」 | interrupt_bridge.rs 353 行全文核验:Drop 恢复 prev sigaction→close→join→raise(SIGINT) eaten_unhandled 忠实重发 + Windows SetConsoleCtrlHandler 独立臂 | 断言成立,keep |

## §1 功能级替换矩阵(9 项全测完)

### #1 c-ares → hickory-resolver 【KEEP — 最大 C 依赖项(36K 行 msvc C + 2271 行 FFI),三重证据否决】
① Node dns.resolve* 参考实现就是 c-ares(同库=parity 特征);② hickory latest 无 blocking 模块,async-only 需自备 tokio runtime,与 bao 同步泵模型冲突;③ bun 上游同持 src/cares_sys(替换=fork 分叉,每吸收波重放)。顺手项:boringssl inet_pton/ntop 三处 std::net 化。

### #2 fs.watch OS 后端 → notify 8.1.0 【REPLACE 可行 — 唯一大面替换(~600-900 行)】
- 可收编:node_fs.rs:2424-3860 的 inotify 解码/OVERLAPPED/worker 循环 OS 后端半(含全部 unsafe 协议解码)
- 必留四面(Node 语义本体,~400-500 行):persistent 存活控制/文件级 watch→父目录+basename 过滤/watchFile stat-poll 融合/EventKind→rename|change 翻译层
- 前审「挂起」判定反转:测试护栏(fs_watch_events/fs_deep/fs_coverage_c1)刚过双平台收敛,现为替换窗最优时点;notify 已知问题与 libuv 局限同族,非新增劣化
- 平台:L✓ W✓ M✓(FSEvents 免费增益)。建议立项独立波,以现有 fs_watch 测试面为回归门。

### #3 bun_glob → globset 【KEEP — 上游同源+语法差集】
globset 明文不支持嵌套 brace(bun_glob 10 层栈)+ `**` 位法更严 + 不做遍历;决定性=bun 上游正持 GlobWalker.rs 做 Zig→Rust 移植,src/glob 是该链一环,换 globset=脱离上游自维护第二实现。

### #4 networkInterfaces → network-interface 2.0.5 【KEEP — 3 轴覆盖不足(修正前审后仍 keep)】
真差集:①无接口 flags(IFF_UP|RUNNING libuv 门不可复现,node_os.rs:470-478)②无 IPv6 scope_id(:595-601)③internal 语义差。385 行是 parity 必要成本。

### #5 os.loadavg/sysinfo/cpus → sysinfo 【KEEP — 面积依重不成比例】
~60-80 行直调 vs 全家桶依赖(MSRV 1.95,常驻 System 实例+refresh 模型)。前审「无薄库」修正为「有库但不值得」。

### #6 node_tty termios → rustix::termios 【REPLACE(POSIX 半边 ~50 行,零险)】
TIOCGWINSZ :266-276/:483 + tty_set_raw_mode :377-445(tcgetattr/cfmakeraw);Windows 臂不动。并入 #67 syscall 扫除波。

### #7 SIGINT/SIGTERM bridge → signal-hook 【KEEP(353 行全文核验成立)】
prev-sigaction 恢复+eaten_unhandled 重发+双平台控制台臂组合不可表达;pipe2 单点并入 #67。

### #8 bun_semver → nodejs-semver 【KEEP(现位),候选登记】
semver crate 无 npm 语义(否决);nodejs-semver 7.0.0(2026-09-25 活跃,4667 行)是未来唯一合格候选(前置=Node/Bun 真值差分 harness;现测试面仅 2 处 #[test] 是独立待补面)。

### #9 bun_sourcemap → sourcemap crate 【KEEP — 引擎集成面】
5173 行与 bundler 引擎深度集成,非独立解析/生成库;上游 Bun 同源。

### #10 bun_md / MIME 【已达标/无手写面,闭项】
bun_md 已是 pulldown-cmark 0.12 底座(剩余=ansi_renderer 2732 行+entity 2162 行,无现成 crate 承载);MIME 已由 mime_guess 承载。

## §2 必留手写清单(防重复审计)
stealth/boringssl 桥、servo 桥、uloop epoll(铁律)、WHATWG url(上游阶梯达标)、JS 引擎桥、fsw 四面、networkInterfaces 385 行、interrupt_bridge、node_os syscall 信息面。

## §3 行动汇总
1. #2 fs.watch→notify:立项独立波(等 #67 释放 node_fs.rs 文件域)
2. #6 node_tty→rustix::termios + inet_pton/ntop std::net 化:并入 #67
3. 其余 keep,差集证据入档
