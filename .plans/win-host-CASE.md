# Windows Host 构建面设计案卷 (2026-09-21)

> 任务:全构建 Windows 宿主路径设计——环境规格 / mozjs host 门 / 农场集成 / 交叉-vs-host 分工 / 验证矩阵 / 分阶段落地序。
> 裁据:用户书面裁决 2026-09-21「可以给 WINDOWS HOST」;host 来源已答询闭合 = **192.168.1.200(LAN 现成 Windows 机)**。
> 校准(2026-09-21 第二轮):ew1 实测 Linux→msvc **configure 已全绿**(E15)→ §4 矩阵重校准:交叉路由可承载全构建(条件:编译+链接层过),Windows host 角色转为**运行时验证真机 + 备胎**。
> 性质:只读设计,零产品语义,零仓库写盘。挂靠 #18(Platform/Build 板)、#37(mozjs 真机余项)、#33/#34/#35(native 三门)、#48(Windows 跟踪)。
> 验证矩阵对齐裁决字面「交叉编译的支持与验证」:**host 面是交叉面的补全,不是替代**——交叉面既有 gate 全保留。

---

## 0. 证据基底(本设计不引入任何未实测断言)

| # | 证据 | 出处 |
|---|------|------|
| E1 | Linux→Windows cross 在 **configure 层不可行**:SM configure 的 `windows.configure` import `winreg`(Windows-only Python stdlib),2026-09-10 Linux cross probe 实证到真 configure 调用层拒绝 | `docs/platform-support.md` §2/§3(#37) |
| E2 | `find_moztools` 四级解析链 4 个 tier 全部 live 验证(2026-09-10,强制 cfg):`MOZTOOLS_PATH` → `<cargo-target-root>/dependencies/moztools/4.0` → `MOZILLABUILD`/`MOZILLA_BUILD` → `panic!`(panic 文案指向 servo/mozjs README) | `vendor/mozjs/mozjs-sys/build.rs:928-948` |
| E3 | windows host 分支:`PATH` 前置 `msys2/usr/bin` + `bin`、回写 `MOZILLABUILD`、make 驱动硬编码 `mozmake` | `vendor/mozjs/mozjs-sys/build.rs:294-308`(行 354 附近 `mozmake`) |
| E4 | 上游 servo/mozjs CI 的 Windows job = **权威 env 全集真源**:`LINKER=lld-link.exe`、`CC/CXX=clang-cl`、`MOZTOOLS_PATH=<ws>\target\dependencies\moztools-4.0`(zip 直下解包),GH runner(windows-latest / windows-11-arm,VS Build Tools 预装),`shell: cmd` | `vendor/mozjs/.github/workflows/build.yml` windows job |
| E5 | servo CI Windows:`LIBCLANG_PATH=C:\Program Files\LLVM\bin`(注明 clang-sys 先搜 msys 路径的绕行)、`RUSTUP_WINDOWS_PATH_ADD_BIN=1`、`SHELL=/bin/bash`;runner=GH `windows-2022` 或自托管镜像 `servo-windows10`;装 GStreamer(`mach bootstrap-gstreamer`) | `vendor/servo/.github/workflows/windows.yml` |
| E6 | servo `mach bootstrap` Windows 装=Winget(`Kitware.CMake`/`LLVM.LLVM`/`Ninja-build.Ninja`/`WiXToolset.WiXCLI 7.0.0.0`)+ moztools 4.0 zip(钉版 URL)+ GStreamer 1.22.8 MSVC x86_64 双 MSI(admin-less `/a TARGETDIR` 解到 `<target>/dependencies/gstreamer/1.0`);choco 等价清单 `cmake 3.26.4 / llvm / wixtoolset 3.11.2 / ninja` | `vendor/servo/python/servo/platform/windows.py` + `vendor/servo/support/windows/chocolatey.config` |
| E7 | servo 构建期 GStreamer 发现顺序:①`<target>/dependencies/gstreamer/1.0/msvc_X86_64`(探测 `bin/ffi-7.dll`)②env `GSTREAMER_1_0_ROOT_MSVC_X86_64` ③默认 `C:\gstreamer\1.0\msvc_X86_64`;mach `build_env` 把 `<gst-root>\bin` 前置进 PATH(注释明言为了让 GStreamer 自带 pkg-config 优先) | `python/servo/platform/windows.py::gstreamer_root` + `python/servo/command_base.py:372-380` |
| E8 | 上游 bun 的 Windows 发布产物 = **Linux 交叉编译**(amazonlinux glibc + `clang-cl --target` + xwin MSVC/SDK sysroot + lld-link),**无 native Windows build lane**;Windows 舰队(Azure D 系)只跑测试/签名/baseline 验证 | `~/code/rust/bun/.buildkite/ci.mjs:155-176, 378-380, 425-431` |
| E9 | bun 交叉钉版配方:xwin **0.9.0**,`--arch x86_64,aarch64 --sdk-version 10.0.26100 --crt-version 14.44.17.14 --include-atl`;编译侧 clang-cl `/winsysroot <splat>`,链接侧 lld-link `/winsysroot:<splat>`;另有 UCRT servicing 覆盖层(xwin 载荷 arm64 UCRT 陈旧,x64 影响小) | `bun/.buildkite/Dockerfile:185-198`、`bun/scripts/build/{winsysroot.ts,flags.ts:104-110,950-956}`、`bun/docs/project/building-windows.mdx` |
| E10 | bun native Windows host 前置:VS2022 NativeDesktop workload、LLVM(其文档钉 21.1.8)、pwsh 7 推荐 | `bun/docs/project/building-windows.mdx` |
| E11 | bao 侧 Windows fail-closed 三门:`uws_sys/build.rs` exit(1)(#34,uv_* 供给缺)、`bao_uloop/src/lib.rs:44-49` compile_error!(#35,IOCP 臂缺);#33(mimalloc MSVC arm)状态随本会话 allocator 波(bun_alloc 已弃 mimalloc 依赖)需实现期复核 | `src/uws_sys/build.rs:95-115`、`src/bao_uloop/src/lib.rs:40-49`、`docs/platform-support.md` §2 |
| E12 | bao boringssl 为 cc-rs 策展编译(非 cmake/GO 管线);asm 走 **linux 变体无条件编译**(`gen/bcm/*-x86_64-linux.S`),而 `-win.asm`(nasm)/`-win.S`(arm)变体**已在 csrc vendored**——Windows 臂 = build.rs 加 target 分支换 asm 清单,非供给缺口 | `src/boringssl_sys/build.rs:372-400` + `src/boringssl_sys/csrc/boringssl/gen/{bcm,crypto}/` |
| E13 | 农场侧(调度方 archaeology):16.18.0.1 KVM 硬件在 / 虚拟化栈未装 / 无 ISO / 96G 内存闲;frog 项目已有 target-windows 交叉缓存(gnu 路由旁证);`frog-remote-build` 同步法 = git fetch+reset 增量(保 mtime,fail-closed) | 调度工单 archaeology + `~/.local/bin/frog-remote-build` |
| E14 | mozjs Windows 工具链要求原文:moztools 4.0 zip(143MB,钉版 release URL)、LLVM≥14、VS2019/2022(MSBuildTools + Windows11SDK + VC.Tools.x86.x64 + VC.ATL,`vs_buildtools.exe` 命令行给出)、Python 3.11(`PYTHON`/`PYTHON3` 必设)、`LIBCLANG_PATH`/`MOZTOOLS_PATH`/`CC/CXX/LD` env 全集 | `vendor/mozjs/README.md` Windows 节(50-97 行) |
| E15 | **(ew1 实测,probe7 RC=0,2026-09-21)** Linux→`x86_64-pc-windows-msvc` 的 SM153 **configure 全绿**:配方 = xwin **0.10** `--include-atl` 拼 winsysroot(~650MB)+ 三类 shim(`cl→clang-cl`、`ml64→llvm-ml64`、link→`lld-link`);**winreg 门可 env 100% 绕过,零 patch**;SM C++ 交叉编译进行中(ICU 层 clang-cl + 真 SDK 头 0 error)。**修正 E1**:「configure 层 Linux 交叉不可行」被实测推翻,`docs/platform-support.md` §2 该句需随 ew1 终报修订 | ew1 终报(进行中);本行系执行体实测转述,终报落盘前标注为**条件性** |
| E16 | **实靶事实(用户答询,2026-09-21)**:Windows host = `192.168.1.200`,LAN 现成机(ping 0.2ms);OpenSSH **22 开**、SMB **445 开**、WinRM 5985 / RDP 3389 **关**;我方两键均未授权 → 访问待用户授权 | 调度方实靶通报 |

**停条件命中记录**:上游 Windows CI 参照**可得**(E4/E5/E6,均在仓内文件),未触发 stop;E12 asm 旗标、E9 交叉 flags 的逐项实测属实现期,标注见 §6。

---

## 1. 环境规格(host 无关,Windows x86_64;§8「环境即代码」等价形)

设计原则:全部组件走**钉版 URL / 钉版包名 + 幂等脚本**(§8 哲学:环境由脚本现场重组,禁镜像搬运)。唯一环境真源 = `bootstrap.ps1`(§3)。

| 组件 | 版本钉 | 来源 / 安装(PowerShell 形) | 必需性 | 证据 |
|---|---|---|---|---|
| **Rust 工具链** | `nightly-2026-07-20`(随 `rust-toolchain.toml` 自动装) | `winget install Rustlang.Rustup`;首次 cargo 调用自动装钉版 + `x86_64-pc-windows-msvc` host 工具;components rustfmt/clippy 随 toml | 必需 | `rust-toolchain.toml:7`(CI/local 同钉);E10 |
| **MSVC Build Tools 2022**(link.exe + SDK libs + ATL headers) | VS17 线(aka.ms 最新) | `vs_buildtools.exe --add Microsoft.VisualStudio.Workload.MSBuildTools --add Microsoft.VisualStudio.Component.Windows11SDK --add Microsoft.VisualStudio.Component.VC.Tools.x86.x64 --add Microsoft.VisualStudio.Component.VC.ATL`(上游原文逐字);等价形 `winget install "Visual Studio Community 2022" --override "--add Microsoft.VisualStudio.Workload.NativeDesktop …"` | 必需(mozjs README 第 3 步;lld-link 替代 link.exe 时 SDK/ATL 头仍必需) | E14;E10 |
| **LLVM(clang-cl / lld-link / libclang)** | 下限 ≥14;**建议钉 21.1.8**(与 bun host lane 对齐,便于旗标经验迁移) | `winget install LLVM.LLVM`(servo 未钉→本案钉);装后手动把 `C:\Program Files\LLVM\bin` 进 PATH(servo `_ensure_llvm_in_user_path` 同动作) | 必需 | E14(≥14);E10(21.1.8);E6(winget 项) |
| **Windows SDK** | Windows 11 SDK(VS 组件携带) | 随上行 VS 组件 | 必需 | E14 |
| **Python** | **3.11** | `winget install Python.Python.3.11`;装后设 **用户级 env `PYTHON` 与 `PYTHON3`** 指向 `python.exe`,并进系统 PATH(mozjs README 第 4 步逐字要求) | 必需(configure 驱动;winreg 门只在 Windows host 成立) | E14;servo `.python-version`=3.11 |
| **moztools** | **4.0**(`MOZTOOLS_VERSION="4.0"`,与 servo `DEPENDENCIES["moztools"]="4.0"` 同钉) | `Invoke-WebRequest https://github.com/servo/servo-build-deps/releases/download/msvc-deps/moztools-4.0.zip`(143MB)→ 解包到 `<target-root>\dependencies\moztools\4.0`(tier-2 自动命中,零 env;servo passive_bootstrap 布局)或任意路径 + `MOZTOOLS_PATH` 指之(tier-1,mozjs CI 形态) | 必需(mozmake + msys2 sh + PATH 供给) | E2/E3/E4/E14 |
| **GStreamer MSVC**(runtime + devel) | **1.22.8**(servo 钉版) | 双 MSI(`gstreamer-1.0-msvc-x86_64-1.22.8.msi` + `…-devel-….msi`,servo-build-deps 钉版 URL)→ `msiexec /a <msi> TARGETDIR=<target>\dependencies\gstreamer\1.0 /qn`(admin-less 解包,servo bootstrap 逐字);构建 env:`PATH` 前置 `<gst-root>\bin`(自带 pkg-config)+ `PKG_CONFIG_PATH=<gst-root>\lib\pkgconfig` | **必需**(bao_browser 依赖 `media-gstreamer` feature,非可选) | E7;`src/bao_browser/Cargo.toml:27` |
| **CMake / Ninja** | servo winget 未钉(choco 侧 cmake 3.26.4);建议 `Kitware.CMake` 最新稳定 + `Ninja-build.Ninja` | winget | 必需(部分 -sys crate / 未来 libuv CMake 通道) | E6 |
| **git for Windows** | 最新稳定 | `winget install Git.Git`;**建议 `git config --global core.longpaths true`**(Rust 依赖树深层路径超 MAX_PATH,运维注意项,非上游引用) | 必需(同步面 + 版本内嵌 hash) | §3 |
| **OpenSSH Server** | OS capability | `Add-WindowsCapability -Online -Name OpenSSH.Server~~~~0.0.1.0`;`Set-Service sshd -StartupType Automatic`;管理员用户 `authorized_keys` 装好;默认 shell 指向 pwsh(`New-ItemProperty -Path "HKLM:\SOFTWARE\OpenSSH" -Name DefaultShell …`) | 必需(农场触发面) | §3 |
| **PowerShell 7(pwsh)** | 最新稳定 | `winget install Microsoft.PowerShell`(bun 明文推荐 pwsh 代替 powershell.exe) | 建议 | E10 |
| **nasm / perl** | 未钉 | bun native host 需 nasm+perl(JSC 栈);**bao 侧仅 boringssl `-win.asm` 编译通道可能需要**(或走 clang-cl 集成汇编)→ 实现期裁,先不装,门红灯再补 | 待定(实现期) | E10 vs E12 |
| **WiX** | 7.0.0.0(servo CI 钉) | 不装——仅 `mach package` msi 打包用,bao 构建门不涉 | 不需要 | E5/E6 |

版本钉差异声明:LLVM 上游两处口径(≥14 下限 / bun host 21.1.8)并存,本案取「下限 ≥14、建议钉 21.1.8」;CMake/Ninja 上游未钉,采纳即视为 **未实测来源**,以 bootstrap.ps1 落盘时的实测为准补钉。

---

## 2. mozjs host 门走通路径(E1/E15 校准后的 host-native 面)

> **校准(E15)**:ew1 实测 winreg 门可 env 100% 绕过、Linux 交叉 configure 全绿 → 本节内容**降级为双态之一**(host-native 面 = 备胎构建面 + 真机工具链基准),不再是对 mozjs 的唯一通路。全链机制不变,仍是交叉链任何一层红灯时的兜底真源,且为真机运行时验证面供给 native 工具链基准(交叉产物 vs host 产物的 parity 参照)。

### 2.1 build.rs windows 分支全链(现有代码,零改动预期)

```
cargo build -p bao-mozjs-sys            # should_build_from_source() 恒 true(BAO patch,禁 prebuilt 回退)
  └─ build.rs [cfg(windows)]            # vendor/mozjs/mozjs-sys/build.rs
       1. find_moztools(build_dir)      # :928 四级链
            tier-1  MOZTOOLS_PATH env                       ← bootstrap 显式供给(推荐,与上游 CI 同形)
            tier-2  <cargo-target-root>/dependencies/moztools/4.0   ← bootstrap.ps1 默认落点(target 自愈,零 env)
            tier-3  MOZILLABUILD / MOZILLA_BUILD env        ← 兜底(MozillaBuild 整包用户)
            tier-4  panic(文案指 servo/mozjs README)        ← fail-closed,保持
       2. PATH 前置 <moztools>/msys2/usr/bin + <moztools>/bin   # :297-300
       3. env MOZILLABUILD=<moztools> 回写                   # :305
       4. make = "mozmake"                                   # :307(硬编码)
  └─ mozmake -R -f makefile.cargo
       ├─ [[ ]] 语法 recipes(makefile.cargo:210-213)← 需 msys2 sh(moztools 供给;**禁裸 make/MSYS 整包替换**,因 make 版本 buggy 探测 :525-545 与 sh 依赖均钉在 moztools 形态)
       └─ maybe-configure → python 驱动 js/src/configure     # configure 的 winreg import 只在 Windows host 成立(E1)
  └─ make 跑 C/C++:CC/CXX 经 SM_TARGET_ENV_VARS 前向(build.rs:376-380)→ clang-cl
  └─ 链接:CRT/SDK libs(VS Build Tools)+ winmm/psapi/user32/Dbghelp/advapi32(build.rs:682-688)
  └─ bindgen:LIBCLANG_PATH → LLVM 的 libclang
```

### 2.2 所需 env 全集(对照上游逐项)

| env | 值 | 上游出处 |
|---|---|---|
| `MOZTOOLS_PATH` | `<…>\moztools-4.0` 绝对路径(tier-1;若按 tier-2 布局落盘可省) | E4(mozjs CI)/ E14(README) |
| `CC` / `CXX` | `clang-cl` | E4 / E14;build.rs 经 SM_TARGET_ENV_VARS 前向给 make |
| `LD` | `lld-link`(或省略 → VS Build Tools 的 `link.exe` 为 cargo msvc target 默认链接器) | E4(`LINKER=lld-link.exe`)/ E14 |
| `LIBCLANG_PATH` | `C:\Program Files\LLVM\bin` | E5(servo CI,注明 clang-sys msys 搜索序绕行) |
| `PYTHON` / `PYTHON3` | `C:\Python311\python.exe` 形 | E14(README 第 4 步) |
| `RUSTUP_WINDOWS_PATH_ADD_BIN` | `1` | E5(servo CI) |
| `PKG_CONFIG_PATH` + `PATH` 前置 | `<gst-root>\bin` 与 `<gst-root>\lib\pkgconfig`(gstreamer 面,非 mozjs 本体;mach `build_env` 的 bao 等价形) | E7 |
| `MOZILLABUILD` / `MOZILLA_BUILD` | 不设(tier-3 兜底,仅手工 MozillaBuild 用户) | E2 |

build.rs 自管、host 无须供给:`MOZBUILD_STATE_PATH`(默认 build_dir 内)、`PYTHONDONTWRITEBYTECODE`、`MOZ_DEBUG_FLAGS="-gdwarf-4 -g1"`(BAO DWARF 降级,包内分发)、`CPPFLAGS` 拼装、make-4.4 jobserver 规避。

### 2.3 与上游的两处 bao 特有约束

1. **`should_build_from_source()` 恒真**:Windows host 也必须源码编;上游 servo/mozjs 的 prebuilt archive 通道是**未打 BAO patch 的异引擎**(§3.1 platform-support),引入即静默 fallback 违宪。BAO 6 patch 均平台中性(C++/Rust),不涉 host 面。
2. **153 forward(em1 波在途)**:windows 分支(find_moztools/PATH 前置/mozmake)是上游 servo/mozjs 机制、跨版本稳定;em1 重放 BAO patch 时该分支**原样携带**,不重写。

---

## 3. 农场集成(实靶 192.168.1.200 · LAN 现成 Windows 机;控制面 = SSH-over-Windows)

### 3.1 实靶事实与授权前置(E16)

| 项 | 值 | 设计决定 |
|---|---|---|
| 机器 | `192.168.1.200`,LAN 现成机(ping 0.2ms) | host 来源闭合;VM-on-16.18.0.1 形态不启用(留档于文末变更记录) |
| OpenSSH **22** 开 | 可达 | **控制面唯一化 = SSH-over-Windows**;与 frog-remote-build 同构,无新面 |
| SMB **445** 开 | 可达 | 同步/大产物**备选路**(默认不用——免凭据与挂载面;仅在 bundle/大产物 scp 不敷时启用) |
| WinRM 5985 / RDP 3389 关 | — | 不启用,保持暴露面最小;控制面单路 SSH 即足 |
| **授权:我方两键均未授权** | 阻塞项 | **W0 第一动作 = 用户授权一把公钥入机**;未授权前 W0 全部步骤不可开跑(存在性断言:钥匙不在 = 面不存在) |

### 3.2 节点布局(实靶落点)

```
192.168.1.200(现成机;CPU/内存/盘实测后补记入案)
  C:\build-farm\bao\
    src\           git fetch + reset --hard 增量(含 .git,版本内嵌 hash)
    outbox\        产物发件箱(HOST 回拉)
    bootstrap.ps1  环境真源:§1 全组件幂等安装(钉版 URL;winget/msiexec /a/zip 解包)
    target\        CARGO_TARGET_DIR 节点本地(独卷;Windows 无 docker 卷等价物,target 即项目私有卷)
```

布局对 §4 的 host 双态(全构建主力[交叉链未闭合期]→运行时验证真机+备胎[交叉链闭合后])**零返工通用**——变的只是跑什么,不变的是怎么摆。

### 3.3 同步面(SSH/SCP 主路;禁全量覆盖;保 mtime 防 cargo 全量重编)

- 主路:**git over SSH**——`git fetch + reset --hard` 增量(与 frog 2026-09-21 同形)。rsync 在 Windows 非原生,scp/robocopy 只承担 bundle 与产物传输,**不做树同步**(rsync 全树覆盖 = 未变文件 mtime 全 touch = 每次全量重编,frog 已禁回退)。节点 fetch 源二选一:①16.18.0.1 bare 镜像(免节点持 GitHub 凭据,推荐);②直连 origin。fetch 失败 **fail-closed 中止**(禁回退陈旧引用,frog 事故同款纪律)。
- 产物回拉:scp(outbox → HOST `target/<profile>/`)。
- 旁路:git bundle(HOST 打包 → scp → 节点 `git pull`),GitHub 不可达窗口用;445 SMB 为同场景备选。
- **.git 必含**(版本号内嵌 hash,§8 铁律)。

### 3.4 触发脚本形态:`~/.local/bin/bao-win-remote-build`(frog-remote-build 镜像,靶默认 `BAO_WIN_HOST=192.168.1.200`)

```
用法: bao-win-remote-build check|test|build|raw [cargo 参数…]
骨架: 1) 探活  ssh -o ConnectTimeout=8 -o BatchMode=yes "$BAO_WIN_HOST" true      # 失败→本机同命令(frog 同款回退,回退=降级非默认)
       2) 同步  ssh … "cd C:\build-farm\bao\src && git fetch <src> && git reset --hard <ref>"   # fail-closed
       3) 执行  ssh … "cd C:\build-farm\bao\src && cargo <sub> --locked --jobs 16 …"            # 日志落 /tmp/bao-win-remote/<cmd>-<stamp>.log + latest
       4) 回拉  scp "$BAO_WIN_HOST:C:/build-farm/bao/outbox/bao.exe" → 本地 target/<profile>/
       5) 冒烟  本机执行拉回产物 --version(§8 冒烟校验铁律;check/test 退出码 = 唯一验收信号)
```

- 执行 shell:cmd 或 pwsh(上游 mozjs CI 用 `shell: cmd`;makefile.cargo 的 msys2 sh 由 build.rs PATH 前置供给,与外层 shell 无关)。
- 缓存分层(§8 卷分类学的 Windows 映射):`target\` = 项目私有;sccache/rustup 共享卷的 Windows 等价物 = 二阶段(先跑通,再谈共享)。
- 与 frog 农场分工:16.18.0.1 Linux 容器农场 = 交叉面(类型门/链接门/全构建承载,§4)+ Linux 原生全量;192.168.1.200 = 运行时验证真机 + host-native 备胎。同一 HOST 脚本族触发,`BAO_WIN_HOST` 与 `FROG_REMOTE` 并列。

---

## 4. 交叉-vs-host 分工矩阵(2026-09-21 ew1 校准版)

### 4.1 分叉点的演变:E1 → E15

- 上游 bun **没有** native Windows build lane(E8/E9):Windows 产物全部从 Linux 交叉,Windows 舰队只跑测试/签名/baseline。
- 本案 v1 的分叉点(mozjs configure winreg,Linux 交叉不可行,E1)**已被 ew1 probe7 推翻**(E15):xwin 0.10 `--include-atl` winsysroot(~650MB)+ `cl→clang-cl`/`ml64→llvm-ml64`/link→`lld-link` 三类 shim + **winreg 门 env 100% 绕过零 patch**,configure RC=0;SM C++ 交叉编译推进中(ICU 层 clang-cl + 真 SDK 头 0 error)。
- 校准结论(**条件性**——ew1 终报前标注):若编译+链接层过,**交叉路由可承载全构建**,bao 完整回到 bun 的交叉模型;Windows host(192.168.1.200)角色转为:
  1. **运行时验证真机(不可替代)**——交叉产物必须在真 Windows 上执行测试/冒烟/行为面;编译过 ≠ 跑得对,这一面交叉机永远无法自证;
  2. **备胎构建面**——交叉链任一层(shim/winsysroot/asm/链接)红灯时的兜底;§2 全链即备胎说明书,布局零返工(§3.2)。
- **platform-support.md §2 修订点(入案,随 ew1 终报执行)**:mozjs 行 Windows 列「Linux→Windows cross is impossible at the configure layer / Windows builds require a Windows host」句改为实测新态(configure 绿 + 配方 + shim 清单);§1 表 Windows 行 `Unsupported` 与三门(#33/#34/#35)的 fail-closed 存在性不变——交叉全构建不改变门,只是门的解锁换了执行面。

### 4.2 分工矩阵(校准后)

| 层 | 构建面(主) | 理由 / 判据 |
|---|---|---|
| 纯 Rust 层(bun_* ~85 crate + bao_runtime/cli/cdp_client/engine/stealth/uloop) | **Linux 交叉** | 零 native 构建脚本门;`cargo check --target` 即类型门;gnu 缓存(frog,E13)作冒烟,msvc(+xwin)为正式 ABI 面;**破口:bun_alloc msvc 臂缺(alloc 波遗留)** |
| uv 供给层(#34,ew4 在途)+ uWS C++ | **Linux 交叉** | C libuv / libuwsockets.cpp 走 cc-rs + clang-cl `--target`,与 bun 交叉模型同构(E8/E9) |
| boringssl_sys / bao_crypto | **Linux 交叉 + build.rs asm 分支** | cc-rs 策展编译;**已知破口:`bcm` linux-asm 撞 msvc 交叉**(ew1 在案,定位待其终报)——`*-win.asm`/`-win.S` 变体已 vendored(E12),换分支即解,ew1 终报对表 |
| harfbuzz / freetype / sqlite 等 -sys | **Linux 交叉** | cc-rs / pkg-config 系,与 musl 交叉波(#10)同机制 |
| **mozjs(bao-mozjs-sys)** | **Linux 交叉(条件已实证至 configure;编译/链接以 ew1 终报为准)** | E15:configure RC=0 + winreg env 绕过零 patch;`should_build_from_source` 恒真不变(§2.3,两态同守) |
| **servo 全树(libservo + script/net/constellation + gstreamer 面)** | **Linux 交叉(跟随 mozjs 同树)** | 依赖 bao-mozjs-sys;gstreamer 交叉供给 = xwin splat 头/lib + pc 文件对齐 E7 三级探测(实现期实测) |
| **运行时验证(全栈)** | **Windows host 独占(192.168.1.200)** | 交叉产物真机执行:单测/冒烟/行为面——交叉机不可替代(§4.1 角色 ①) |
| **备胎构建(全栈)** | **Windows host** | 交叉链红灯时按 §2 全链 host-native 走通(E14/E2/E3 上游机制在位) |

### 4.3 验证矩阵(校准后:交叉面=构建证明,host 面=运行证明+备胎)

| 面门 | 命令形(执行面) | 证明什么 | 跑在哪 |
|---|---|---|---|
| **交叉·类型门**(常态,每波) | `cargo check --target x86_64-pc-windows-msvc -p <crate>`(frog Linux 农场,jobs=16) | cfg 门内 Rust 声明面回归即红灯(ew3 已证其真回归信号地位) | Linux frog 容器 |
| **交叉·链接门**(供给波 gate) | `cargo build --target x86_64-pc-windows-msvc -p <leaf>` + xwin splat(E15 钉版 xwin 0.10/E9)+ shim 族 | 供给层符号闭合(#34 后):uv_*/uws C++/boringssl 实际链接 | Linux frog 容器 |
| **交叉·全构建门**(ew1 终报后启用) | `cargo build --workspace --locked --target x86_64-pc-windows-msvc`(窄命令日常迭代;全量仅发布/波末,§10 粒度纪律) | 交叉路由承载全构建:mozjs/servo 全树在 Linux 面出 Windows 产物 | Linux frog 容器 |
| **host·运行时验证门(不可替代)** | 交叉产物部署到 192.168.1.200 → `cargo nextest` scoped(引擎进程单例纪律同 Linux)+ `bao.exe --version`/`bao run <smoke>`/`bao browser` headless 冒烟 | **行为面**:编出来的东西在真 Windows 上跑得对;设置栈/realm/事件循环语义在 Windows 路径成立 | 192.168.1.200 |
| **host·备胎构建门**(交叉链红灯时启用) | §2 全链:`cargo build -p bao-mozjs-sys` 起,host-native 走全树 | 交叉链故障期的构建能力保全 + 交叉 vs host 产物 parity 参照 | 192.168.1.200 |
| **产物清单门**(随产出 lane) | 链接产物清单(bao.exe + 依赖 dll 面:gstreamer dll、VC runtime、SDK 依赖),入 outbox 与案卷 | 交付物可枚举,禁「编出来了但不知道带了什么」 | 产出所在节点 |

平台状态推进(`docs/platform-support.md` 语汇,校准后):交叉类型门+链接门绿 = 「交叉面支持与验证」成立;**+ 真机运行时验证门绿(192.168.1.200)= `Experimental` → `Supported` 的完整实证链**。裁决字面映射:交叉面证明「交叉编译的支持与验证」,host 面补全「运行时的真实成立」——两面缺一,谁也不替代谁。

---

## 5. 分阶段落地序与验收判据(校准后:交叉=构建主线,host=运行验证+备胎)

| 阶段 | 内容 | 验收判据(可判定,逐条) | 依赖 |
|---|---|---|---|
| **W0 实靶就绪** | **第一动作:用户授权公钥入 192.168.1.200(E16 阻塞项,钥匙不在 = 面不存在)** → `bootstrap.ps1`(§1 全组件幂等)→ `bao-win-remote-build` 骨架通 | ①`ssh -o BatchMode=yes 192.168.1.200 true` RC=0(免密);②bootstrap 二次执行零 diff 幂等;③`clang-cl/lld-link/python(3.11)/mozmake/git` 版本冒烟各 RC=0 且与钉一致;④HOST `raw` 子命令走通探活→同步→执行→回拉→本机冒烟全链 | **用户授权钥匙** |
| **W1 交叉面收敛(与 ew1 合流)** | ew1 终报对表:configure(已绿,E15)→ SM C++ 编译 → 链接;在案破口修复:boringssl bcm asm 分支(E12/ew1 对表)+ bun_alloc msvc 臂;三门按依赖序解锁(#33 状态复核[allocator 波]→ #34 uws/uv[ew4]→ #35 IOCP 臂) | 类型门 `cargo check --target x86_64-pc-windows-msvc` 逐 crate RC=0;链接门 RC=0;终报 compile+link 过 → 交叉·全构建门条件解除 | ew1 终报;ew4(#34);#35 波 |
| **W2 交叉·全构建门** | `cargo build --workspace --locked --target x86_64-pc-windows-msvc`(frog,jobs=16;窄命令迭代,全量仅波末) | RC=0 + 产物清单落盘 outbox(bao.exe + 依赖 dll 面) | W1 |
| **W3 host·运行时验证(192.168.1.200,不可替代面)** | 交叉产物部署真机:`cargo nextest` scoped(引擎单例纪律同 Linux)+ `bao.exe --version` / `bao run <smoke>` / `bao browser` headless 冒烟 | 全绿;Windows 特有缺陷按 BCE 立案(不降级绕行);此门绿 = `Experimental`→`Supported` 实证链闭合 | W2 |
| **W4 host·备胎构建实证(§2 全链;可与 W1-W3 并行)** | host-native `cargo build -p bao-mozjs-sys`(§2.2 env 全集,tier-1 `MOZTOOLS_PATH` 起步)→ 全树;交叉 vs host 产物 parity 抽检 | configure RC=0 → `js_static` 链接 RC=0 → `cargo test -p mozjs` scoped 绿;关 #37 Windows 真机余项;parity 记录入案 | W0 |
| **W5 收口入档** | `docs/platform-support.md` §2 mozjs Windows 列按 E15 终报修订 + §1 表行状态推进 + issue 关闭(#33/#34/#35/#37/#48 波及面)+ 验证矩阵/bootstrap 索引入档 | 文档/issue 与实证一一对应;W0-W4 判据全留痕(日志/outbox/bundle) | W3+W4 |

每阶段一测纪律(§硬门⑤)与功能级 git 提交(§硬门⑦)适用;Windows 节点上的验收日志回拉 outbox 留痕。

---

## 6. 风险与未决(显式,无灰区)

| 项 | 状态 | 处置 |
|---|---|---|
| **192.168.1.200 访问授权**(我方两键均未授权,E16) | **阻塞项(W0 前置)** | 用户授权公钥后 W0 才可开跑 |
| **ew1 终报未到,交叉全构建系条件性结论**(configure 已绿 E15;编译/链接进行中) | 条件性 | 本案按双态设计(host=运行验证+备胎,布局零返工);终报到 → §4.1/§5 状态字更新 |
| **boringssl `bcm` linux-asm 撞 msvc 交叉**(ew1 在案破口) | 破口已立案,定位待 ew1 终报 | W1 修:build.rs 加 target 分支换 `*-win.asm`/`-win.S`(变体已 vendored,E12);nasm vs clang-cl 集成汇编旗标实测定 |
| **bun_alloc msvc 臂缺**(alloc 波遗留) | 破口在案 | W1 修;#33 状态一并复核(mimalloc 弃用后门的存在性) |
| GStreamer 交叉供给(xwin splat + pc 文件 vs host MSI 三级探测,E7) | 机制推出,未实跑 | W2 实测;失败回退 host-MSI 形态(§3.2 布局不变) |
| #33 mimalloc 门 | 状态漂移中(bun_alloc 已弃 mimalloc 依赖,platform-support §2 行待复核) | W1 实现期复核;若已移除则 #33 关闭形态改为「门随依赖移除失效」 |
| xwin 交叉钉版(ew1 实用 **0.10**;上游 bun 钉 0.9.0 + sdk 10.0.26100 + crt 14.44.17.14,E9) | 两处口径并存 | 以 ew1 实测配方为 bao 主钉,终报落版入 bootstrap 交叉段 |
| UCRT servicing 覆盖(xwin 载荷陈旧 UCRT,E9) | bun 实测事实,x64 影响小 | 交叉链接门红灯时按 winsysroot.ts 同款覆盖层处理 |
| `core.longpaths` / Defender 排除 / 页文件 | 运维建议(非上游引用) | bootstrap.ps1 附注,出问题先查此三项 |

---

## 附:变更记录

| 轮 | 变更 |
|---|---|
| v1(2026-09-21) | 初版五节;host 无关设计(VM/现成机未定) |
| v2(2026-09-21) | ①实靶闭合:§3 重写为 192.168.1.200 实靶(SSH 控制面唯一化,445 备选,授权前置);②ew1 校准:E15 入证据基底,§2 降级为备胎面,§4 矩阵重校准(交叉承载全构建,host=运行时验证真机+备胎),§5 重排(W0 授权前置/W1 与 ew1 合流/W3 真机验证为不可替代面),§6 新增四行;platform-support.md §2 修订点入案(随 ew1 终报执行) |
