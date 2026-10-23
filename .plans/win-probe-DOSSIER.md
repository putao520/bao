# Windows 交叉编译探测案卷(xwin-msvc vs mingw-gnu vs win-host)

> 任务:用户裁决 2026-09-21"增加对WINDOWS交叉编译的支持与验证"探测。
> 执行:ew1-win-probe,2026-09-21。只读仓库;全部探针在 /tmp/win-probe(源码副本 /tmp/win-probe/vsrc,仓库零写盘)。
> 证据:全部命令日志在 /tmp/win-probe/logs/(probe1~9、make1~4、xwin-*)。

---

## ① winreg 门 153 复核(树内原文 + 真跑 configure 断点实证)

**结论:门仍在,但 153 已把它变成可 env 绕过项;configure 层零 patch 需求。**

| 项 | 140 形态(记忆 #37 波) | 153 形态(树内实证) |
|---|---|---|
| winreg import 位置 | `build/moz.configure/windows.configure:54` 直 `import winreg` | 移入 `build/moz.configure/util.configure:356` — `get_registry_values` 头部 `@imports(_import="winreg")`(moz.configure 沙箱在函数首调 `_apply_imports` 时才 exec import) |
| 触发点 | windows.configure 直接调用 | `build/moz.configure/windows.configure:19` `windows_sdk_dir` 内 `get_registry_values(r"HKLM\...\KitsRoot*", ...)` |
| 触发条件 | target/host abi=msvc 即触发 | **仅当 `WINDOWSSDKDIR`、`WINSYSROOT`、`VC_PATH` 三 env 全缺**(windows-toolchain.configure 新增 `bootstrap_winsysroot`/`WINSYSROOT` option,上游 153 演进) |
| 实测 | 140 探针判"交叉 configure 层不可能" | probe3(VC_PATH 有、SDK env 无)复现:`ModuleNotFoundError: No module named 'winreg'`(probe3-winreg.log 栈:windows.configure:19 → _apply_imports → _get_one_import → exec) |

**patch 面评估:0。** 不需要改造条件 import(=不需要 BAO 新偏差)。绕过即上游原生支持的 `WINSYSROOT` env(或 `WINDOWSSDKDIR`+`VC_PATH` 二件套)。Python configure 层随 tarball 原样在树(em1 31db4d73 移植仅动 build.rs+makefile.cargo,python 面未触碰,实证一致)。

**注意(需修订的旧权威载体):** `docs/platform-support.md` §2 记"Linux→Windows 交叉在 configure 层不可能、Windows 构建必须 Windows host"(140 判定,#37)。153 实测 configure 层已不是不可能(见②);该行应按本探针修订(仅 configure 层结论;compile/link 层原判不变,由④继续实证)。

---

## ② 路由对比案(A xwin-msvc / B mingw-gnu / C win-host)

### 案 A:x86_64-pc-windows-msvc 跨给 clang-cl + xwin sysroot —— **configure 层已实测全绿**

probe7 RC=0 配方(可复刻,全部 /tmp/win-probe/logs):

1. **SDK 面**:`xwin 0.10.0 --accept-license --arch x86_64 --include-atl splat` → `.xwin-cache/splat/{crt,sdk}`(crt 114M + sdk 521M;含真 atls.lib/atlbase.h、um/ucrt/shared/winrt/cppwinrt 头、ucrt>=10.0.17134 满足 moz 最低 UCRT 门)。
2. **拼 moz winsysroot 布局**(上游 `-winsysroot` 术语,windowstoolchain.configure 原生消费):
   ```
   wsroot/VC/Tools/MSVC/14.44.35207/{include←crt/include, lib/x64←crt/lib/x86_64,
     atlmfc/include→symlink ../include, atlmfc/lib/x64→symlink ../../lib/x64,
     bin/Hostx64/x64/{cl.exe→clang-cl, ml64.exe→llvm-ml64-23}}
   wsroot/Windows Kits/10/{Include/10.0.26100/{shared,um,ucrt,winrt,cppwinrt}←sdk/include,
     Lib/10.0.26100/{um,ucrt}/x64←sdk/lib(x86_64 需改名 x64)}
   ```
   关键门全为存在性检查,真货满足:ATL/MFC isdir(windows.configure:281)、winrt/cppwinrt isdir(include_path)、winsdkver.h WINVER_MAXVER ≥ 0x603、`--include-atl` 让 atlmfc 有真货(空目录也能过 isdir,但按零捷径纪律用真货)。
3. **工具 shim**(/tmp/win-probe/shim-bin 入 PATH):lld-link→lld-link-17(moz 153 LINKER check_prog 只认 `lld-link`,windows.configure:380,可用 `LINKER` env 指名)、llvm-rc→llvm-rc-23、llvm-lib→llvm-ar-23。cl.exe/clang-cl 23 本体在 PATH。ml64.exe(shim 入 VC bin)在 `--disable-jit` 下仍被 check_prog 要求(toolchain.configure:3009)。
4. **configure 参数**:基组 `--disable-jemalloc --disable-js-shell --disable-tests --disable-export-js --disable-shared-js --build-backends=RecursiveMake --enable-explicit-resource-management --target=x86_64-pc-windows-msvc`(jit/intl 缺省开;jit 需求未单测,ml64 shim 已就位)。
   **跨给编译必须 `--enable-libz-rs`**(probe8-10 实测链):缺省 feature 的 libz-sys 轨走 `pkg_check_modules("MOZ_ZLIB","zlib")` → 交叉下 pkg-config 抓 **host** zlib(`MOZ_SYSTEM_ZLIB:'1'` 静默落 config.status,probe10 前 config.status 实录)→ 编译期吃 host 头、链接期无 msvc zlib.lib = 断链;`use_system_zlib = not libz_rs`(moz.configure:921-926),libz-rs 轨 = 纯 Rust zlib + `libz-rs-sys/include/{zlib.h,zconf.h}` 头面(0.6.8 在 registry 实存),makefile.cargo 已按 `CARGO_FEATURE_LIBZ_RS` 自动传 flag、build.rs:344 已有 libz-rs/libz-sys 互斥门——**feature 选择纪律:cross 目标一律 libz-rs**。
5. **CPPFLAGS 注入**(cargo face 由 build.rs 自动;手动探针需复刻):encoding_c_mem/include + mozjs-extracted-crates 五个 cbindgen glue include(collator/locale/normalizer/properties/unicode_bidi_ffi 的 OUT_DIR/out/include)+ libz-rs-sys/include。缺 encoding 头 → mfbt/Latin1.h fatal(make1/make2 实测);缺 bidi 头 → SelfHosting.cpp 链(make4 实测);缺 zlib 头 → SelfHosting.cpp→Compression.h(make4 实测)。

逐墙梯(probe1→7,每墙一条实测 log):

| 墙序 | 触发 | 解法 |
|---|---|---|
| 1 | `Cannot find a Visual C++ install for e.g. ATL headers.`(windows-toolchain.configure:235,find_program cl.exe) | cl.exe→clang-cl shim + VC_PATH/WINSYSROOT |
| 2 | `Cannot find the ATL/MFC libraries…`(windows.configure:281) | atlmfc/lib/x64 目录(xwin --include-atl 真货) |
| 3 | `Cannot find the assembler`(Looking for ml64.exe) | ml64.exe→llvm-ml64-23 shim |
| 4 | `Cannot find rc`(Looking for llvm-rc) | llvm-rc→llvm-rc-23 shim |
| — | (若三 SDK env 全缺)winreg ModuleNotFoundError | WINSYSROOT env(上游原生) |

clang-cl 23 以 msvc 形态被 configure 全套接受(target C/C++ compiler works、Windows SDK 0x0a00、UCRT 10.0.26100、linker lld-link-17 全绿)。

### 六层可行性(A 案)

| 层 | 可行性 | 证据/破口 |
|---|---|---|
| mozjs(SM153) | **configure + 全量 C++ 交叉编译 + js_static.lib 打包三层全绿(实测)**:make5 RC=0,655 个 .obj 全部 coff-x86-64,llvm-lib-23 按清单打包 js_static.lib 342MB(镜像 build.rs:474 windows 分支 js_static_lib.list 流程);jsglue/bindgen/Rust 侧链接待 cargo 实测 | 上述配方;build.rs 的 DEP encoding/glue/zlib -I 由 build.rs 自动注入(手动探针已复刻) |
| servo(含 stylo/webrender) | 中-高(未实测):上游 servo 有 Windows 产物形态(stylo MSVC 支持在上游);bao 30 文件 patch 面 platform-neutral(platform-support.md §2 原文判定) | cargo check --target 未跑(超本轮预算);依赖 mozjs/boringssl/uws 三层先行闭合 |
| boringssl | 有独立破口:probe 实测 `bcm.cc` 撞 `gen/bcm/*-x86_64-linux.S` linux-gas asm 集 + `-Wa,--noexecstack`(gnu-as 专属 flag) | 破口=boringssl_sys build.rs 的 asm flavor 锁 linux;解法=perlasm win64 再生成(上游 x86_64-xlate.pl 支持 win64)或 cfg 分 flavor;boringssl_sys build.rs:30 显式 `clang++` 需按 target 换 clang-cl 形态 |
| uws | **设计级 fail-closed(#34)**:uws_sys/build.rs windows 臂 exit(1) 带 uv_* 供给分析(LIBUS_USE_LIBUV 需 libuv 目标码,树内只 vendor uv.h include 面,bun_libuv_sys 纯 FFI 声明) | 关闭需 vendor libuv 或移植 Bun CMake 供给路径(独立波) |
| fonts(freetype/harfbuzz) | 未实测;与 musl #10 同类(pkg-config/cmake 交叉面):msvc target 无系统 freetype,走 bundled cmake 需交叉工具链配置 | 声明为破口,未实测 |
| 事件循环/分配器(bao_uloop/bun_alloc) | bao_uloop `compile_error!` 门(#35,lib.rs:44-49 实测引文);bun_alloc default_alloc 无 windows 臂(③) | 两处都是显式 fail-closed,非隐性破 |

### 案 B:x86_64-pc-windows-gnu(mingw-w64)

- 树内投资≈0:makefile.cargo 只有 **windows-host** 的 `MSYSTEM=MINGW64` msys2 分支(upstream host-form),Linux cross-gnu 无任何 arm;build.rs 仅 `rustc-link-lib=stdc++`(gnu 分支)。
- 上游形态:SM/Firefox 从无 mingw 产出形态;SM mingw 仅社区 patch 史(gjs/fedora mingw 路线),153 无 any 上游 CI 背书。
- 连带面:boringssl/uws/stylo 全要按 gnu 再各自验证一遍(winpthreads、SEH、C++ runtime 混链),patch 面 = A 案全部 + SM mingw patch 集(未知量,历史痛点类)。
- **判:A 案严格劣于 B 案是错的——是 B 严格劣于 A**:上游全部原生 arm 都在 msvc 形态(windows-toolchain.configure/ml64/lld-link/moztools),gnu 等于放弃全部上游对齐,自开第二战线。

### 案 C:Windows host 原生构建(runner/真机)

- 上游最完整形态(moztools 4.0 + mozmake,build.rs `#[cfg(windows)]` 全套;platform-support.md §3 已逐 tier 验证 moztools 链)。
- 但违背用户裁决字面"交叉编译的支持与验证";且 GitHub Actions 已弃用(2026-08-22 用户裁决,CI 全本地),win runner 与现有 farm 拓扑不合。
- **定位:仅作对照/最后兜底**,不进主路由。

---

## ③A libuv 供给线(用户裁决首要根因;合同增补专项)

**结论:首要根因的"供给侧"已在树内落地并经本轮探针实证闭合;真正的残余 = 消费侧开闸(bun_uws_sys un-gate)+ bao_uloop IOCP(#35)+ bun_alloc 单点。**

| 项 | 实测结果 |
|---|---|
| 供给 crate | `src/libuv_sys`(bun_libuv_sys 0.2.0,`links="uv"`):build.rs 头注释明示 = vendored libuv(含上游 2 个 win-poll patch)→ 静态 `uv` 库,**"the uv_* symbol supply that closes #34"**;WIN 25 源 + SHARED 12 源镜像上游 `deps/libuv.ts`,build.rs 明文支持非 Windows host 交叉(xwin 式 sysroot:INCLUDE env 或 CFLAGS_<triple> -imsvc + AR_<triple>)——**本探针即其设计工况** |
| 实测(无 SDK env) | clang-cl 编 fs-poll.c → `stdio.h not found`(纯 env 面,非设计缺口) |
| **实测(xwin INCLUDE env)** | **uv.lib 产出:2,093,250 B,685 个已定义 `uv_*` 文本符号**(uv_loop_new/uv_run/uv_fs_req_cleanup 抽验在);路径 `x86_64-pc-windows-msvc/debug/build/bun_libuv_sys-*/out/uv.lib`;唯一告警 `-g1` unknown-argument(良性)。**#34 供给侧 L2 探针级达标**(ew4-uv-supply 线的 C 符号供给已被本轮独立复现) |
| 消费侧 1:bun_uws_sys | exit(1) fail-closed(#34,build.rs:84-115)——供给侧闭合后需 un-gate(改判据:uv 符号已可供给)。**PASS-by-design 门,当前树内被 boringssl/lsquic build script 先行挡住,未到达** |
| 消费侧 2:bao_uloop | `compile_error!`(lib.rs:44-49,#35,IOCP 臂不存在)——PASS-by-design 门 |
| 声明层(bun_sys/bun_io/bun_event_loop) | **BLOCKED-BY bun_alloc**(三 crate check 全在 bun_alloc 3×E0425 处终止,lib.rs:495 `#[cfg(not(windows))] pub fn malloc_aligned` 即门原);无法判"clean",红与否待 W1 后回归探针(lead 预测项 4 的判定条件尚不成立) |
| mimalloc panic(lead 预测项 3) | **门已不存在**:#45 整删 mimalloc;替代 Layer-0 门 = bun_alloc 缺 msvc 臂(真缺口,非 by-design) |
| **新增破口:bun_lsquic_sys** | build script exit(1) @ msvc target(QUIC/HTTP-3 C 层无 windows 臂);uws_sys h3 面依赖。需供给决策(vendor lsquic win 编 vs Windows 面 feature-off) |
| uv 消费分派点(fs/net/timer/dns/poll) | 声明/镜像侧全链在树:`windows_sys`(tier-0 FFI)→`libuv_sys`(uv 1.51.0 镜像)→`sys::{windows/mod.rs, sys_uv.rs 893L(cfg windows), libuv_error_map.rs}`→`io/windows_event_loop.rs 453L`(FilePoll/Loop over uv)→`event_loop/{AnyEventLoop,MiniEventLoop,SpawnSyncEventLoop}`→`uws_sys` WindowsLoop(uv_loop 所持形);逐功能状态表(fs/net/timer/dns/poll/event-loop/声明 vs C 供给 vs 断点)直接引用 ew3 案卷 `/tmp/uv-gap/CASE.md` §②(已出,本卷不重复) |

与 ew3 接合:上游 `origin/main`=4af1842c8c 声明侧全 Rust 化 + Windows 官方支持平台——bao 声明面已覆盖该链条(CASE.md §①②);**供给边界已从"mozjs configure"(本轮已证绿)移到"L3 全链链接 + uws/uloop 开闸"**。

## ③ cargo 层破口清单(逐层实测,`cargo check --target x86_64-pc-windows-msvc`,共享 target)

| # | 层 | crate | 结果 | 断点 |
|---|---|---|---|---|
| 0 | 纯叶参照 | bun_wyhash | **绿**(2m21s) | — |
| 1 | 分配器(Layer-0 万堵点) | bun_alloc | **红** RC=101 | `default_alloc::{malloc_aligned,realloc_aligned,zalloc_aligned}` 3×E0425 "configured out"(basic.rs:22/74/103,门原=lib.rs:495 `#[cfg(not(windows))]`);**41 个 crate 直接依赖 bun_alloc 全体被堵,含 libuv 声明线**(③A);→ 在途 s1 alloc 波面收编 |
| 1.5 | QUIC | bun_lsquic_sys | **红**(build script exit 1) | msvc target 无 windows 臂;uws_sys h3 面依赖;需供给决策(vendor win 编 vs feature-off) |
| 2 | boringssl | bun_boringssl_sys | **红**(build.rs cc 期) | 实测命令:`clang++ --target=x86_64-pc-windows-msvc -c bcm.cc` exit1;根因=linux-gas `gen/bcm/*-linux.S` asm 集 + `-Wa,--noexecstack` + 编译器未按 msvc 形态选 clang-cl(build.rs:30 固定 clang++) |
| 3 | uws | bun_uws_sys | **红(设计级 PASS-by-design)** | probe 先撞 2 的 boringssl 依赖墙;自身 windows 臂为 fail-closed exit(1)(#34,build.rs:84-115 原文);**供给侧本轮已证闭合(③A),残余=开闸** |
| 4 | 事件循环 | bao_uloop | 红(未跑,compile_error! 静态断言) | `#[cfg(not(any(linux,macos)))] compile_error!`(lib.rs:44-49,#35) |
| 5 | mozjs | bao-mozjs-sys | configure+SM 全量 C++ 编译+lib 打包绿(②④);cargo face(bindgen/jsglue/Rust 链接)未跑;make ≥4.4 jobserver workaround 已随 em1 采纳,host make 4.3 不触 bug | 待 s1 波解 1 后实测;需 `CC_x86_64_pc_windows_msvc` 族 env 钉 clang-cl(cc-rs 惯例,未实测);**cross 必须 libz-rs feature**(②配方第 4 条) |
| 6 | fonts/servo | freetype-sys 等 | 未跑 | 声明未实测(#10 同类) |

结构性事实:131 个 src 文件带 windows cfg(任务书 126 口径,现测 131);`src/windows_sys`=`bun_windows_sys`(windows API 基建在树);`docs/platform-support.md` 是平台 SSOT(Windows=Unsupported,三 fail-closed 点 #33/#34/#35 与本探针一一对应,#33 已关闭)。

---

## ④ 验证口径建议(交叉"验证"判据)

分级判据,"支持"=L1+L2+L3 全绿;L4 仅 stretch 非承诺:

- **L1 compile surface**:纯 Rust 子树 `cargo check --target x86_64-pc-windows-msvc` 绿。
- **L2 native closure**:SM/boringssl/uws 三静态库在交叉工具链下产出。**SM 层 L2 本轮已达标(探针级)**:configure RC=0 + 655 .obj(coff-x86-64)+ llvm-lib 打包 js_static.lib 342MB。
- **L3 full link**:`bao.exe`(lld-link)全链链接成功(+`--version` 冒烟需 Windows 机或 Wine,L3 本身只断言链接)。
- **L4(stretch,非承诺)**:Wine smoke `bao run` 基本面——超出"交叉编译验证"字面,单列。
- 每级留证据物:probe log + 产物哈希;farm 化后进 `run_all.sh` 口径(线上 CI 已弃用,本地等价套件为权威验收)。

---

## ⑤ 建议路由 + 首波修复 DAG

**路由裁定建议:A(xwin-msvc)**。理由:上游 153 全部 Windows arm 原生在 msvc 形态(WINSYSROOT/ml64/lld-link/moztools/clang-cl 分支),configure 层已实测零 patch 全绿;B(gnu)放弃全部上游对齐、SM mingw 无上游背书;C(win-host)违背"交叉"字面且与本地 CI 政策冲突。

```
W0  farm 化配方(§8 纪律):xwin cache → 独立卷(winsysroot 654MB);Dockerfile 钉
    llvm-23(clang-cl/lld-link/llvm-ml64/llvm-rc)+ python3 + make<4.4 或用 em1 采纳的
    4.4 workaround;shim-bin 进镜像。镜像=glibc ≤ HOST。
W1  bun_alloc default_alloc windows(msvc)臂(单文件;在途 s1 波 scope 收编)→
    L1 重测:41 依赖 crate cargo check 绿 + libuv 声明线(bun_sys/io/event_loop)
    回归探针(③A BLOCKED-BY 解除后才能判 clean/红)
W1.5 WINSYSROOT/INCLUDE farm env 契约固化:xwin winsysroot + INCLUDE 七段 +
    AR_<triple>=llvm-lib(本轮实测 uv.lib/js_static.lib 双依赖此 env;进 Dockerfile)
W2  boringssl_sys:asm flavor(win64 perlasm 生成物入库 or cfg 切换)+ 编译器选择
    (CXX_<target>=clang-cl)→ js 侧静态库产出
W2.5 lsquic_sys windows 臂供给决策(vendor win 编 vs Windows 面 feature-off;
    uws h3 面依赖,若 feature-off 需与 uws h3 cfg 对齐)
W3  mozjs-sys cargo face:WINSYSROOT+shim 环境注入 build.rs run;SM_TARGET_ENV_VARS
    贯通;bindgen libclang msvc-args(build.rs:583-595)实测;js_static.lib 打包 +
    lld-link 链接面 → mozjs 层 L2(SM C++ 侧已由本探针实证,见②)
W4  #34 收口:供给侧已在树(libuv_sys build.rs)且经本探针 uv.lib 685 符号实证
    (③A);残余=uws_sys exit(1) 开闸(改判据为 uv 供给存在)+ 链接面 → 归
    ew4-uv-supply 线(已派)
W5  #35 bao_uloop IOCP arm(参照上游 io/windows_event_loop.rs uv 形态选型:
    bao_uloop 自研 tick ABI vs uWS libuv eventing 驱动——归 ew3/ew4 接合裁决)
W6  fonts 双 sys crate 交叉(musl #10 同类解法)+ servo 层 cargo check --target
W7  L3:全链 link bao.exe → 宣称"x86_64-pc-windows-msvc 交叉编译支持(编译+链接绿)"
    ——最大边界事实:SM C++(②)与 libuv C 供给(③A)均已实测绿,边界在
    链接面与消费侧开闸,不在 mozjs configure(140 旧判已废)
W8  (stretch,非承诺)Wine smoke → L4
```

依赖关系:W1→W3 串行(L1 堵点);W2/W2.5/W4/W5/W6 互相独立可并行;W7 依赖全部;W1.5 与 W0 同文件批次落地。

---

## 附:本轮实测账目

| 产物 | 路径 |
|---|---|
| 案卷 | /tmp/win-probe/DOSSIER.md |
| 全部日志 | /tmp/win-probe/logs/{probe1..9,make1..4,xwin-install,xwin-splat} |
| vsrc 副本(仓库拷贝,174M) | /tmp/win-probe/vsrc |
| **uv.lib(#34 供给 L2 产物,2MB,685 uv_* 符号)** | /var/cargo-builds/3c/6184ceb77072ba/x86_64-pc-windows-msvc/debug/build/bun_libuv_sys-*/out/uv.lib |
| libuv 供给探针 log | /tmp/win-probe/logs/libuv-include-probe.log |
| ew3 接合案卷 | /tmp/uv-gap/CASE.md(引用其 §①②,本卷不重复) |
| winsysroot(xwin splat+拼装,654M) | /tmp/win-probe/wsroot |
| shim-bin | /tmp/win-probe/shim-bin |
| objdir(SM 交叉构建,662M) | /tmp/win-probe/objdir6 |
| **js_static.lib(L2 产物,342M COFF)** | /tmp/win-probe/objdir6/js/src/build/js_static.lib |
| xwin root/cache | /tmp/win-probe/xwin-root、/tmp/win-probe/.xwin-cache |

仓库零写盘(仅只读 git log/read);共享 target 仅写入 x86_64-pc-windows-msvc 子树(bun_wyhash/bun_alloc 探针)。
