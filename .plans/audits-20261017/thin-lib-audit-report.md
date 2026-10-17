# bao 薄库替换测绘报告(OS 交互面)

> 只读测绘,证据制。工作目录 /home/putao/code/rust/bao。日期 2026-10-17。
> 平台前提:bao 官方支持矩阵 = **Linux + Windows**(全量测试两平台跑通,Goal D);macOS 仅交叉探针级(非交付目标)。三平台覆盖表按此解读——macOS 列是"免费增益"而非当前收益。

## 方法与全局事实

- 扫描面:`grep libc:: src/`(非 vendor)全量分布 + 逐类人工归因;rustix API 断言全部对照本地 registry 源码 `rustix-0.38.44` 实证(未凭记忆)。
- **rustix 0.38 已是 workspace 依赖**(`Cargo.toml:146`,features = std/fs/event/process/net;lock 0.38.44),`src/sys/fd.rs:119` 已有 rustix 消费先例——替换面不新增依赖族。
- **tempfile 3.27.0 / walkdir 2.5.0 / errno 0.3.14 已在 Cargo.lock**(传递依赖,零新增成本);notify / signal-hook / quanta / hostname / if-addrs / network-interface **均不在 lock**。
- rustix 0.38 **无 inotify 模块**(registry 实证 `ls src/ | grep inotify` = 空)——fs.watch 的 inotify 调用只有 notify 一条收敛路径,无"渐进迁 rustix"中间态。
- libc 直调分布(bao_runtime):node_fs 171 / node_constants 134 / node_child_process 89 / bun_api 50 / node_os 38 / ipc_channel 23 / node_tty 19 / interrupt_bridge 15。
  - node_constants 134 处全是 `S_IRWXU`/`O_NOFOLLOW` 类**纯值常量**(JS 端 process.constants 暴露),无薄库替换语义,不在候选。
  - node_child_process 89 处里 `close`×26 是 fd 清理样板(无薄库收益),spawn 本体已走 `bun_spawn` posix_spawn(复用阶梯已达标)。

---

## 候选矩阵(按 ROI 排序)

| # | 候选 | 现位点 | 规模 | 薄库 | 三平台 | 风险 | 建议 |
|---|------|--------|------|------|--------|------|------|
| 1 | node_os POSIX 信息面(uname/hostname/release/version) | node_os.rs:1256-1299, 1340-1352 | ~56 行→~15 | rustix::system::uname | L✓ M✓ W=另有臂 | 极低 | **替换** |
| 2 | errno 读取样板 `__errno_location` | node_child_process.rs:624,707,1048,2215,2237,2268 | 6 点 | std::io::Error::last_os_error()(或 errno,已在 lock) | L✓ M✓ W✓ | 极低 | **替换** |
| 3 | isTTY 判定 `libc::isatty` | bun_api.rs(isatty×5) | 5 点 | std::io::IsTerminal(stable,零新依赖) | L✓ M✓ W✓ | 极低 | **替换** |
| 4 | umask 读取/设置 | bun_api.rs:9289-9293 | 2 点 | std::fs::umask | L✓ M✓ | 极低 | **替换** |
| 5 | TIOCGWINSZ 终端尺寸 | node_tty.rs:269-276, 483-490 | ~24 行 | rustix::termios::tcgetwinsize | L✓ M✓ W=另有臂 | 极低 | **替换(POSIX 半边)** |
| 6 | termios raw mode | node_tty.rs:415-432 | ~18 行 | rustix::termios(tcgetattr/set + make_raw) | L✓ M✓ W=Get/SetConsoleMode 另有臂 | 低 | **替换(POSIX 半边)** |
| 7 | pipe2(O_CLOEXEC) 自管道 | interrupt_bridge.rs:211-218;node_child_process pipe×6 | ~8 点 | rustix::pipe::pipe_with(PipeFlags::CLOEXEC) | L✓ M✓(W 臂无 pipe) | 低 | **替换** |
| 8 | waitpid 状态手工解析 WIFEXITED/WEXITSTATUS/WTERMSIG | node_child_process.rs(WIF*×6 处) | ~40 行 | rustix::process::waitpid → WaitStatus 枚举 | L✓ M✓ | 低 | **替换** |
| 9 | kill 信号发送 | bun_api kill×3 / node_os / cluster | ~8 点 | rustix::process::{kill_process, kill_process_group} | L✓ M✓(W=TerminateProcess 另有臂) | 低 | **替换(POSIX 半边)** |
| 10 | poll(2) 多路等待(fsw worker / child_process 泵) | node_fs.rs:3275-3285;node_child_process.rs(pollfd×2) | ~3 点 | rustix::event::poll | L✓ M✓ W=N/A(Win 臂走 WaitForMultipleObjects) | 低 | **替换(Linux 臂)** |
| 11 | fs.statfs | node_fs.rs:7026-7080 | ~55 行 | rustix::fs::statvfs | L✓ M✓ W=GetDiskFreeSpaceExW 另有臂 | 低(字段映射已收敛于 create_statfs_object) | **替换(POSIX 半边)** |
| 12 | mkdtemp Windows 臂手写随机重试 | node_fs.rs:5066-5140(mkdtemp_inner;win 臂 5086-5140) | ~55 行 | tempfile::Builder(...).disable_cleanup()(已在 lock) | W 臂专用 | 中(随机字符集与 Node/CRT 不一致,可能改变产物名分布) | **部分替换** |
| 13 | socketpair(进程内 IPC 通道底座) | ipc_channel.rs(socketpair×1 + CMSG 族) | 1 点 | rustix::net::socketpair | L✓ M✓ | 低,但 CMSG fd 传递(SCM_RIGHTS recvmsg/sendmsg)rustix 0.38 无完整对等面,CMSG 部分必须留 libc | **部分替换** |
| 14 | os.getPriority/setPriority | node_os.rs:1171-1182(os_get_priority);windows 臂 :1578 | ~40 行 | rustix::process::priority(getpriority_process/setpriority_process) | L✓ M✓ W=另有臂 | 低 | **替换(POSIX 半边)** |
| 15 | **fs.watch 双平台 OS 后端 → notify** | node_fs.rs:2470-3860(fsw 族 30 fn) | ~1391 行(其中 OS 后端半 ~800-900 可替换) | notify(inotify / FSEvents+kqueue / ReadDirectoryChangesW) | L✓ M✓ W✓ 全覆盖 | **中-高**(见详述) | **暂保留,macOS 立项时再替换** |

### 高价值详述

#### #15 fs.watch 后端(最大面,但建议缓行)

- 现位点构成(node_fs.rs:2470-3860):
  - Linux inotify 臂:`inotify_init1`(:2829)、mask 组装+`inotify_add_watch`(:3452-3478,IN_ATTRIB|IN_CREATE|IN_MODIFY|IN_MOVED_*|IN_DELETE*|IN_MOVE_SELF)、批量解码 `fsw_decode_inotify_batch`(:3200)、poll worker 主循环 `fsw_worker_main`(:3245)
  - Windows 臂 `mod fsw_win`(:2498-2757):ReadDirectoryChangesW + 永久 pending OVERLAPPED read + WaitForMultipleObjects 排空 + synthetic wd 分配器(:2653)
  - 共享面:stat-poll(watchFile)与事件 hub 融合(`fsw_register_poll` :3503)、persistent 语义 liveness、事件翻译 → Node "rename"/"change"(`fsw_dispatch_event` :3750)
- notify 覆盖:inotify(Linux)/ FSEvents+PollWatcher(macOS)/ ReadDirectoryChangesW(Windows)——恰好把 #15 的 OS 后端半(unsafe 解码 + OVERLAPPED + inotify 协议)整体收编。
- **不可被 notify 收编的剩余面**(替换后仍需自写):① Node `persistent` flag 控 eval-loop 存活(notify 无此概念);② 文件级 watch 映射到父目录+basename 过滤(libuv 语义,现位 :3344-3358 已镜像);③ watchFile stat-poll 融合;④ 事件→Node 二值事件名翻译层。
- 风险:notify 事件模型(EventKind/RawEvent)与 libuv 事件语义的差异会引入回归面;现实现刚过 e201 重构、双平台实跑收敛、有 conformance 测试(tests/suite/node_conformance/fs_conformance.rs:423)。
- 判定:**现在替换收益=行数削减,风险=Node watch 语义回归**;macOS 不在交付矩阵,FSEvents 增益当前为零。建议挂起至 macOS 立项或 fsw 面再出缺陷时执行。

#### #1 node_os POSIX 信息面(性价比最高)

- `get_os_release`(node_os.rs:1269-1299)存在**双路径冗余**:先 `libc::syscall(SYS_uname, …)` 裸系统调用、失败再 fallback `libc::uname()`——同一调用两种姿势,典型样板(rot)。`get_os_version`(:1340-1352)、`get_hostname`(:1256-1267)重复 `utsname`/`gethostname` 样板。
- `rustix::system::uname()`(system.rs:52,返回 `UtsName{nodename, release, version, machine}`)一次拿全 4 个 getter 的原料,错误处理由 infallible API 收敛(uname 实际不失败)。
- 收益:~56 行 unsafe/CStr 样板 → ~15 行纯值转换;Windows 臂(GetComputerNameExW/RtlGetVersion,node parity 语义)不动。

---

## 查过但不可替换清单(防重复审计)

| 面 | 现位点 | 不可替换理由 |
|---|--------|--------------|
| bao_uloop epoll tick | src/bao_uloop/src/poll.rs | 项目铁律:必须与 FilePoll 共享 epoll fd |
| bun_* crate 群(fs/resolver/dns/base64/http) | workspace | 复用阶梯已裁决,已是"薄库"角色 |
| stealth/boringssl 桥、servo 桥 | bao_stealth / bao_browser | 必要手写层(任务给定) |
| os.networkInterfaces(getifaddrs/GetAdaptersAddresses) | node_os.rs:478-865(~385 行) | **无等价薄库**:if-addrs 不给 netmask/CIDR/MAC;network-interface 有 MAC 无 CIDR;rustix 0.38 无 ifaddrs。Node 语义需要 family/address/netmask/CIDR/mac/internal 全集。手写面是必要成本 |
| sysinfo / getloadavg | node_os.rs:1301-1335 | rustix 0.38 无 sysinfo/getloadavg;procfs crate 过重 |
| CLOCK_BOOTTIME uptime(bun.nanoseconds 基准) | bun_api.rs:9549-9555 | boot-time 参考系,std/rustix 无对等(clock_gettime(CLK_BOOTTIME) 只有 libc);Windows 臂 GetSystemTimePreciseAsFileTime 同理 |
| os.cpus 的 /proc/cpuinfo 解析 | node_os.rs:1336-1348 | 纯文本解析无薄库;rustix::procfs 不提供 cpuinfo 提取 |
| SCM_RIGHTS CMSG fd 传递 | ipc_channel.rs(CMSG_* 全族) | rustix 0.38 无完整 cmsg 构建面;socketpair 本体可换(#13)但 CMSG 留 libc |
| process.constants 的 S_*/O_*/SIG* 常量表 | node_constants.rs(134 处) | 纯值暴露,无薄库语义 |
| SIGINT bridge 整体 → signal-hook | interrupt_bridge.rs(353 行) | 语义定制三件套 signal-hook 给不了:① Drop 恢复 prev sigaction(现位 :188,:262-276);② eaten_unhandled 状态下 re-raise(永不静默吞 Ctrl-C);③ Windows SetConsoleCtrlHandler 独立臂(:246-266)。自写面质量高(文档完备),signal-hook 只能替换 pipe2+read 一小截(可并入 #7) |
| getpwuid 用户名 | node_os.rs:1236-1253 | rustix 无 passwd 面;whoami 类 crate 偏重;USER/LOGNAME fallback 语义自持 |
| fs stat 值转换 / utimes 族 / fchmod/fchown | node_fs.rs(timeval×18/futimes×4/utimes×3/lutimes×2 等) | utimes 系 rustix 0.38 无(有 utimensat 但 Node API 是秒级 tv 语义);stat 已优先走 std::fs::Metadata;fsw stat-poll 的 raw `libc::stat` 是轮询增量比对局部面,收敛收益小 |
| monotonic 时钟 | node_fs.rs:3319-3324(monotonic_ms) | 仅 10 行;quanta 引依赖不值。**附观察项(非本任务)**:函数名 monotonic_ms 但实现是 `SystemTime`(wall clock),watchFile deadline 用 wall clock 在系统时钟回拨时 deadline 会漂移——如需真单调改 `std::time::Instant` 即可,无需薄库 |
| node_child_process 的 close×26 | node_child_process.rs | fd 清理样板,任何库等价 |
| posix_spawn 面 | bun_spawn(workspace) | 已是复用载体 |

---

## 汇总判读

1. **一次性"std 先行"小扫除(#2/#3/#4)**:errno 样板、isatty、umask 三类共 ~13 点可零新依赖零风险替换(std 自带),适合顺手做一个 hygiene 波。
2. **rustix 统一化(#1/#5-#11/#14)**:node_os POSIX 信息面 + node_tty + child_process wait/kill/pipe + statfs,合计 ~250 行 unsafe 样板收敛为类型化 safe API;全部是"POSIX 半边替换、Windows 臂保留"的形态,与 bao 双平台矩阵正交,无行为面风险(错误从 errno int → io::Result,调用点已是错误转发形)。
3. **唯一战略级候选(#15 fs.watch→notify)**:最大面 1391 行,但现实现成熟、macOS 增益暂为零、语义回归风险中-高 → 建议挂起,登记为 macOS 立项时的前置工程。
4. **结构性保留**:networkInterfaces、loadavg/sysinfo、CMSG、signal bridge、epoll——均无对等薄库或属铁律,勿再审计。
