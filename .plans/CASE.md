# libuv Windows 供给路径缺口普查案卷 (2026-09-21)

- 上游基准: oven-sh/bun `origin/main` = **4af1842c8c** (2026-09-21 fetch, RC=0; 本地 clone e85606d484 落后 1162,工作树陈旧未采用)
- 我方基准: bao master 66bdb414 工作树 (只读探测)
- 上游 libuv 真源: **oven-sh/libuv fork `bun` branch @ `8023581113b276e7c1aee3f82da57ca0893faab1`,uv 1.51.0** (`scripts/build/deps/libuv.ts:LIBUV_COMMIT`;含 fork 专属修复 #13-#16 + f3ce527e win-pipe CancelIoEx)

---

## ① 上游最新态 (origin/main 4af1842c8c)

### 结论先行:上游 Windows libuv 供给路径**已全部 Rust 化,Zig 已清零**

- `git ls-tree -r origin/main src/ | grep '\.zig$'` = **0 个**(shallow 边界前的历史不可考,但树态铁证:src/ 下零 .zig)。
- `src/libuv_sys/` = 纯 Rust FFI crate:`libuv.rs` 3132L(108 个 `extern "C"` uv_* 声明 + `#[repr(C)]` 镜像 + Loop/Stream/ write 高层封装)+ **`open_handles.rs` 228L(新增,bao 无)** + lib.rs。
- `src/sys/` = 16 个 .rs,**零 .zig**;`src/sys/windows/{mod.rs 2102L, env.rs 90L}` 纯 Rust。
- uv_run 调用方全为 .rs:`src/jsc/VirtualMachine.rs`、`src/event_loop/{SpawnSyncEventLoop,MiniEventLoop,AnyEventLoop}.rs`、`src/io/windows_event_loop.rs 254L`、`src/runtime/{timer,server}/mod.rs`、`src/spawn/process.rs`、`src/io/lib.rs`。
- C 层真源不变:上游仍是 **真 C libuv**(37 个 .c 直编:`SHARED` 12 + `src/win/*` 25,defines/cflags 见 `scripts/build/deps/libuv.ts`)+ C usockets **libuv eventing 后端**(`packages/bun-usockets/src/internal/eventing/libuv.{c,h}`,`libusockets.h` 默认 `_WIN32 → LIBUS_USE_LIBUV`)。Rust 只做声明与封装,C 做实现。
- 上游官方口径:**Windows (x64 & arm64) 为正式支持平台**(origin README:46)。
- 上游自维护 2 个 libuv win patch(`patches/libuv/`,均打 `src/win/poll.c`):`win-poll-rearm-before-callback`(AFD level-triggered,重提交 ioctl 先于 poll_cb,堵 loopback RST 窗口;注释自述"to upstream: send to libuv/libuv")、`win-poll-abort-with-disconnect`(UV_DISCONNECT 订阅补 AFD_POLL_ABORT + UV_PRIORITIZED 做 ABORT-only 通道)。

### 近期演进(commit 锚,均为 2026-08~09)

| commit | 日期 | 内容 |
|---|---|---|
| 2b3f660116 | 08-28 | sys(windows): 未映射 Win32 错误码报 EUNKNOWN 而非 success (#40860) |
| 5fba7bd23f | 08-28 | Win32Error::get() 解包 FACILITY_WIN32 HRESULT (#40864) |
| 9dd73746c7 | 08-23 | bun run: Windows CTRL_C_EVENT 归还脚本 (#39729) |
| ef3bcfddc6 | 08-22 | Windows pipe reader buffer use-after-free(libuv in-flight 读)(#31940) |
| 6dfa8f4227 | 08-19 | libuv + WebKit bump(Windows startup work)(#39614) |
| bb8aa37afa / d0aafac5d3 | 09-10 | usockets(windows) closed-poll 存活期修复 → **被 revert** (#39643/#42258) |
| e8541037c4 | 09-05 | 路径 scratch buffer 改池化 (#41442) |

### 直接可搬面(bao 缺失/漂移的上游 Rust 产物)

1. `src/libuv_sys/open_handles.rs`(228L,新文件,整体可搬);
2. `src/libuv_sys/libuv.rs` 当前 face:extern 面 **108 vs bao 276**——上游做过大裁剪(`2af91e1b67` 等 dead-code removal),唯一 origin-only 新符号 `uv__winsock_ensure`;bao 独有 168 个声明为上游已删面(uv_udp_*/uv_thread_*/uv_os_*/uv_fs_scandir…);
3. `src/sys/windows/mod.rs`:上游新增 `last_system_errno`/`is_app_container`/`disposition`/`exe_image_range`/`StdinModeGuard`/`is_watcher_child`/`become_watcher_manager`/`move_opened_file_at`/`kernel32_2`/`GetEnvironmentVariableW`;bao 独有约 2800L 的 `mod _win32error_full_table`(上游已外移出该文件);
4. `src/sys/windows/env.rs`(90L vs bao 97L,diff 36 行)、`src/windows_sys/externs.rs`(1856L vs 1774L)。

---

## ② 我方缺口清单 (bao 消费面盘点)

### 架构定位(与上游对齐)

bao 的 Rust 吸收面已覆盖上游 Windows 链条的**声明侧全链**:
`src/windows_sys`(Win32 tier-0 FFI)→ `src/libuv_sys`(uv 1.51.0 镜像,**与上游同版本**)→ `src/sys::{windows/mod.rs 5064L, sys_uv.rs 893L(cfg windows), libuv_error_map.rs 140L}` → `src/io/windows_event_loop.rs 453L`(FilePoll/Loop over uv)+ `src/event_loop/{AnyEventLoop 724L, MiniEventLoop 683L, SpawnSyncEventLoop 493L}` → `src/uws_sys::Loop.rs::WindowsLoop`(285L 起,`uv_loop: *mut uv::Loop` 所持形)→ C usockets `eventing/libuv.c`(已 vendor 在 `src/uws_sys/csrc/bun-usockets/`)。
全树 `cfg(windows)` 125 文件(sys 11 / uws_sys 10 / io 6 / event_loop 4 / spawn(_sys) 6 / dns / paths 6 …)。

### 逐功能判定

| 功能域 | 声明/镜像面 | C 符号供给 | 状态 |
|---|---|---|---|
| fs(uv_fs_* / sys_uv) | `sys/windows/mod.rs` + `sys_uv.rs` 已移植(Rust) | 无独立需求(fs 走 uv 线程池→需 C libuv) | **部分**(声明全,链接断) |
| net/TCP/pipe(uWS) | `uws_sys` WindowsLoop + vendored `eventing/libuv.c` | **缺**:只 vendor 了 `<uv.h>` include 面,零 libuv 目标码 | **stub 供给**(#34) |
| timer(uv Timer / uws timer) | `libuv.rs` Timer 镜像 + `uws_sys` timer mod | 同上 | **部分** |
| dns | `dns/lib.rs` 带 cfg(windows) ws2_32 臂;`libuv.rs` uv_getaddrinfo 声明在 | uv_getaddrinfo→C libuv | **部分** |
| poll(FilePoll) | `io/windows_event_loop.rs` 已移植(453L,较上游 254L 反而多) | uv_poll → C libuv | **部分**(声明全,链接断) |
| event loop 核心 | `bao_uloop` = **epoll-only Rust 版 Posix loop ABI**(`us_loop_run_bun_tick` 自研);**Windows/IOCP 臂不存在**,crate 根 `compile_error!(not(linux|macos))` (`src/bao_uloop/src/lib.rs:44`) | 上游 Windows 循环 = C usockets libuv.c 驱动真 C libuv | **缺**(#35) |
| io_uring | n/a | n/a | **n/a**(Linux-only,Windows 链条不涉及) |

### 供给缺口的权威在仓登记

- `docs/platform-support.md` §1/§2/§4:Windows=**Unsupported,fail-closed by construction at three native points**(#33/#34/#35);§4 明记 #34 "uv_* symbol closure(real libuv source/static-lib decision)= Open"。
- `src/uws_sys/build.rs:101-115`:windows 目标 **exit(1) fail-closed**(issue #34 关闭方式;真实供给"vendor libuv or port the CMake path"留作后续波)。
- `bao_libuv_sys` = 纯声明 crate(README/`lib.rs` 自证:零符号定义);#34 关闭评论(b6455fb4)明确"claimed supplier produces no symbols"。
- 跟踪面:#48(OPEN,Windows 进度跟踪,下游 MW Desktop `--features bao`)、#18(Platform/Build 板)。
- 文档漂移警示:`platform-support.md` §2 称 `bao_uloop` 有 "kqueue backend (src/bao_uloop/src/kqueue.rs)" — 当前树无该文件(kqueue 死臂已删,`lib.rs:26/138`);macOS 口径需另波核实。

---

## ③ 工作量与路线

### 路线判定:**上游搬运(C libuv vendor)是必经之路;Rust 重写 libuv 本体不可行**

- 用户两选项中"上游最新做完没"的答案:**声明侧上游已做完且 bao 已基本同步;缺的是 C 侧符号供给——上游的供给方式(CMake/direct 37 .c)本身是上游做完的,Bao 化 = 搬运该供给**。
- "自维护移植完"若指 Rust 重写 libuv 本体 → 否决(千万级 C,零复用,违复用优先);若指仿 `bao_uloop` 用 Rust 重写 **usockets 适配层**(WindowsLoop ABI over C libuv)→ 可选的二阶段,但 **C libuv 供给无论如何都在关键路径上**。

### 逐块归属

| 块 | 动作 | 归属 | 量级 |
|---|---|---|---|
| C libuv 供给(关 #34) | vendor oven-sh/libuv @8023581113(1.51.0)入 `src/libuv_sys`(新增 `build.rs`,cc-crate 编 37 .c + 2 个 `patches/libuv/*.patch` 重放;flags 照抄 `deps/libuv.ts`,MSVC/clang-cl)或链接预编译 .lib | E 合同单 crate(build.rs+patches+供应商主文件) | build.rs ~150L + patch 重放;**小-中** |
| uws_sys windows 臂解锁 | 撤 `build.rs:101` fail-closed 改真编译(eventing/libuv.c + libuwsockets.cpp,-DLIBUS_USE_LIBUV,include 指向 vendor uv.h) | 同上一波次波 | 小 |
| bao_uloop Windows 臂(关 #35) | 二选一:①C usockets libuv.c 直用(上游同款,零自研);②Rust 实现 WindowsLoop ABI over libuv_sys(仿 bao_uloop 形,~1-2kL) | 建议先 ①,②留作高性能化后续 | ①小 ②中 |
| mimalloc MSVC 臂(关 #33) | clang-cl flags 修正 | 独立小合同 | 小 |
| FFI 面再同步(漂移收敛) | `open_handles.rs` 整搬;`libuv.rs` 276→108 面裁剪(需先 grep bao 消费方对 168 个已删声明的引用);`sys/windows/mod.rs` 上游新增 10 函数吸收;`env.rs`/`externs.rs` 小 diff | 吸收波(batch) | 中(rust-only,风险低) |
| 消费侧贯通 | bao_runtime/bao_cli 的 windows 启动臂、stdio、Ctrl+C(上游 9dd73746c7 同款) | 后续波 | 中 |
| (边界,非 uv)mozjs/servo | Windows 构建需 **Windows host**(configure 层 winreg/python,跨编不可行,platform-support §2 已证) | 独立轨 | 大 |

### 总量估计

uv 供给闭环本体 = **1 个 E 合同(build.rs vendor+编译)+ 1 个解锁合同(uws_sys windows 臂)+ 1 个吸收波(FFI 再同步)**;纯 uv 链条 Rust 侧新增代码 ≈ 数百行(build.rs+patch 重放为主),C 侧零手写(全部 vendor)。全栈 Windows(SM+servo+runtime)另需 Windows host 与 #33/#35/#48 各波,不在 uv 案卷承诺内。

---

## ④ 与 ew1 win-probe 的接合点(交叉断点预告)

`cargo check --target x86_64-pc-windows-msvc` 断点序列预测(按依赖序):

1. `bun_uws_sys` → **build.rs exit(1)**(`src/uws_sys/build.rs:101-115`,#34 fail-closed)——第一个硬门,符合预期非回归;
2. `bao_uloop` → **compile_error!**(`src/bao_uloop/src/lib.rs:44-49`,#35)——第二个硬门;
3. `bun_mimalloc_sys` → build.rs panic(#33)——第三个硬门;
4. `bun_libuv_sys` / `bun_sys` / `bun_io` / `bun_event_loop` / `bun_windows_sys` → **纯声明,cargo check 应通过**(cfg(windows) 门内零 C);其错误才是真回归信号;
5. 全二进制链接层:`uv_*` undefined(若 1 被强解锁)+ mozjs configure 拒绝 Linux host(`--target=x86_64-pc-windows-msvc` 走到 configure 要求 Windows Python/winreg → Linux→Windows cross **不可行**,Windows 构建需 Windows host——platform-support.md §2 实证结论)。

ew1 侧若见 4 号位红 → 回报为吸收面回归;1/2/3 号位红 = 已登记 fail-closed 门的正常表现,应记 PASS-by-design。

---

### 附:提取产物(本目录)

- `origin_libuv_sys_libuv.rs` / `origin_libuv_sys_open_handles.rs` / `origin_libuv_sys_lib.rs` / `origin_libuv_sys_Cargo.toml` — origin/main 原文
- `origin_sys_windows_mod.rs` / `origin_sys_windows_env.rs` / `origin_sys_lib.rs` — origin/main sys 面
- `bao_uv_fns.txt` / `origin_uv_fns.txt` — 两面 pub fn 清单
