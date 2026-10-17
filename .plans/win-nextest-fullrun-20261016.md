# Windows 全 workspace nextest 首跑台账(2026-10-16)

> 任务:e-winnextest / REQ-ENG-007(Windows Supported 平台测试面)/ 目标 D 执行面
> 历史:Windows 真机此前只跑过手工探针面(winrun_* 11 面)与单 crate 面,全 workspace 测试首见天日。

## 树态(构建输入)

- 本地 HEAD:1d14df38357f7cde9955b52bc0016ee824cbb5dc + 84 个未 commit 改动(Goal A 假绿改造 / Goal B ENG-19/21+Gamepad / Goal C 四测试文件 / subtle 两缺陷根修)——rsync 快照即全波终态
- farm:bao-winx 隔离项目(镜像 bao-winx-build:ubuntu24,llvm-20,私有卷 bao-winx-target,免 sudo bind /opt/bao-win-cross)
- profile:test-ci(stripped + opt-level 2)
- target:x86_64-pc-windows-msvc(clang-cl + xwin sysroot,`source scripts/win-cross-env.sh` 注入)

## 构建命令(farm)

```bash
# 同步(本地 .205 → farm)
rsync -a --delete --exclude '/target/' --exclude '/.claude/' --exclude '/bench/results/' \
  --exclude '/.venv/' --exclude 'node_modules/' --exclude '__pycache__/' \
  /home/putao/code/rust/bao/ putao@16.18.0.1:build-farm/bao-winx/src/

# 构建(farm 宿主执行 ~/build-farm/bao-winx/build-win-tests.sh,容器内)
source scripts/win-cross-env.sh
cargo build --target x86_64-pc-windows-msvc --profile test-ci -p bao_bin          # bao.exe(BAO_BIN)
cargo build --target x86_64-pc-windows-msvc --profile test-ci --tests --keep-going # 全图测试目标
cargo nextest list --target x86_64-pc-windows-msvc --profile test-ci --workspace --tests
cargo nextest archive --archive-file /outbox/bao-win-tests.tar.zst --target x86_64-pc-windows-msvc --profile test-ci --workspace --tests
```

- nextest 双平台 0.9.146(farm 容器 /outbox/bin/cargo-nextest;.200 C:\bao-win-test\nt\nextest.exe)

## 构建结果(farm 四段迭代)

| 步骤 | 结果 |
|------|------|
| rsync 树同步 | HEAD 1d14df3835 一致;84 改动文件同步(farm .claude/ 排除面 D 252 条=构建无关) |
| build1 全图首跑 | bun_runtime(lib) E0308 失败 → farm shim 修复;test-ci 全图 keep-going 收口 |
| build2 增量重跑 | **bao.exe RC=0**(215,692,800B);test-ci tests 全图:FAIL 72 包 / ALL 116 / OK 44;44 包白名单 verify RC=0 |
| build4 archive | **ARCHIVE_RC=0**:88 个测试二进制(含 1 非 test bin)、1820 文件、1,047,116,676B(.tar.zst)→ /outbox/bao-win-tests.tar.zst |

**重 crate suite 失败详情(平台门缺失致整 suite 缺席,登记不修)**:

| crate | 缺席原因 |
|-------|---------|
| bao-browser (test "suite") | 单点 unix-only API 无 cfg 门:`webdriver_signal_exit_e2e_tests.rs:126 libc::kill`(E0425)→ 整 suite 编不过 |
| bun_runtime (test "suite") | 20 errors:unix-only API 族(libc SOL_SOCKET/SO_SNDBUF/SO_RCVBUF/EPOLL*/AF_INET*/sockaddr*/pthread_attr_t/std::os::unix/std::os::fd)+ `bao_uloop::poll` unresolved(bao_uloop lib 的 Windows cfg 下无该模块)+ lld-link 6 例 |
| bao_cdp / bao_cdp_client / bao_crypto | lib test + suite link 失败(usockets crypto C 桩族) |
| bao_engine / bao_stealth / cdp-server / bao_cli / bao_bin / bao_bundler | **tests 全部编过,进 archive** ✓ |

## 执行命令(.200 真机,终形态)

```bash
# 传输(farm→本地→.200 双跳;scp 直写 /mnt/c 9p 慢通道 40min 断连,rsync→WSL home→本地 cp 快通道 ~3min)
rsync --partial --inplace -e ssh archive.tar.zst gpu5070ti:bao-nt/   # sha256 cdc33a796463c06f 双端一致
# 源树:C:\src(robocopy \\wsl.localhost→C:\ 实拷,93MB/s 12s)——CARGO_MANIFEST_DIR 烘焙 /src/... 在 Windows 解析为 C:\src\...
# 单次全量(nextest 0.9.146:裸 exe 需 'nextest' 子命令桥;-E filterset 值必须加引号;junit 走 profile 配置)
nextest.exe nextest run --archive-file C:\bao-win-test\nt\archive.tar.zst --extract-to C:\bao-win-test\nt\xt \
  --workspace-remap C:\src --no-fail-fast --final-status-level skip -E "package(a) or package(b) or ..."
# env:BAO_BIN + BAO_TEST_BAO_BIN=C:\bao-win-test\bao.exe(cli_snapshot 双 env 名)
# 挂死兜底:.config/nextest.toml [profile.default] slow-timeout={period="60s",terminate-after=5}(300s 硬杀)
```

- .200:C:\bao-win-test\(368G 可用)+ C:\src(源树);DISPLAY 类依赖面恒 SKIP(无 Xvfb,预期形态)
- 默认 env(无 BAO_TEST_NETWORK / BAO_TEST_REAL_SERVO)= CI 基线形态

## 逐 crate 台账(终版,2026-10-17 单次全量 run)

**总账:4167 run / 4163 passed / 4 failed / 1 skipped(Summary 29.9s)+ 8 个 #[ignore] 前置排除**
**复验(第二次全量 run):4167/4163/4/1 完全一致,同 4 失败 = 确定性复现非 flaky**

| crate(组) | run | passed | failed | 备注 |
|------------|-----|--------|--------|------|
| bao_stealth | 1701 | 1701 | 0 | 全绿 4.6s(Linux 同构) |
| cdp-server | 1319 | 1319 | 0 | 全绿 3.0s(Linux 同构) |
| bao_engine | 417 | 413 | 4 | 3 ABORT 0xc0000409(内存计量族)+1 FAIL xdr 时序;w31 双进程 bench 本轮 PASS(时序敏感) |
| bao-mozjs | 23 | 22 | 1 | ABORT collect_servo_sizes 0xc0000409(同计量族) |
| bao-mozjs-icu-collator / collections / normalizer | 51/44/32 | 全 | 0 | 全绿 |
| bao_cli | 19 | 19 | 0 | 初跑 2 fail(golden 路径 CARGO_MANIFEST_DIR 烘焙)→ C:\src 源树方案修复后全绿 |
| bao_bundler | 40 | 40 | 0 | 全绿 |
| bun_sm | 208 | 208 | 0 | 全绿 |
| bun_highway | 26 | 26 | 0 | 全绿 |
| bao_lints | 14 | 14 | 0 | 全绿 |
| bao_ipc | 8 | 8 | 0 | 全绿 |
| bao-mozjs-sys | 1 | 1 | 0 | 全绿 |
| bao_bin / bench-harness / bun_alloc / bun_hash / bun_lsquic_sys / bun_opaque / bun_platform / bun_output_tags / bun_simdutf_sys / bun_sm 边角 / bun_windows_sys / bun_wyhash | 0 | — | — | 0 tests(NO_TESTS_RUN:包无 unit tests 或测试在别 crate suite) |
| 7 个 proc-macro 包(bao_engine_macros / bun_clap_macros / bun_collections_macros / bun_core_macros / bun_css_derive / bun_dispatch / bun_jsc_macros) | — | — | — | **error 193(not a valid Win32 app):proc-macro host 测试二进制=Linux ELF,交叉 archive 无法携带 host 产物=平台固有限制** |

## 真缺陷清单(平台无关逻辑在 Windows 红)

> **2026-10-17 根治更新:下表 4 项已全部根治,复跑证据见「根治段」。**

| crate | 测试名 | 失败形态 | 归因 |
|-------|--------|---------|------|
| bao_engine | memory_stats_tests::test_engine_memory_stats_native_collect | ABORT 0xc0000409(STATUS_STACK_BUFFER_OVERRUN fast-fail) | mozjs BaoCollectRuntimeStats 计量桥(mozjs patch #6)在 Windows msvc 的栈溢出——计量探针族真平台缺陷,非环境缺失 |
| bao-mozjs | servo_sizes::collect_servo_sizes | ABORT 0xc0000409 同上 | 同族(ServoSizes rollup 计量) |
| bao_engine | zone_topology_measurement_tests::test_zone_topology_measurement_all | ABORT 0xc0000409 同上 | 同族(GC zone 拓扑计量) |
| bao_engine | xdr_cache::tests::c2_cold_start_decode_beats_recompile_on_heavy_payload | FAIL(xdr_cache.rs:713 断言) | 性能比测时序敏感(Windows 首跑 IO 特征;w31 同文件本轮 PASS=时序类非硬缺陷) |

## 根治段(2026-10-17,3×ABORT + c2 flake 全收口)

### 根因(证据链)

1. **3× ABORT 0xc0000409 共根**:`vendor/mozjs/mozjs-sys/src/jsglue.cpp` 的 `MallocSizeOf`(计量堆遍历的 malloc-sizeof 回调)Windows 臂直接 `_msize(ptr)`——**`_msize` 无 NULL 契约**(glibc `malloc_usable_size(NULL)`/macOS `malloc_size(NULL)` 文档化返 0),遍历遇 NULL/非 CRT 指针即触发 CRT invalid-parameter → `__fastfail` = 0xc0000409。证据:Windows 事件日志 faulting module=**ucrtbase.dll**(CRT abort 面,非栈溢出 0xC00000FD);三测同走此一个回调函数。
2. **c2 flake 双根**:①XDR store 依赖 memory 层不短路(跨 trial/跨测试的 memory 残留使 store eval 短路、盘上无文件,decode eval miss_absent 走 compile);②严格 `decode_us < compile_us` 不等式在 realm-setup 噪声(realm 构建数百 µs 淹没差值)+ IO/CPU 不对称(decode 首触盘 IO 对负载敏感)下不可确定。

### 修法(产品/桥侧为主,无测试绕行)

| 文件 | 改动 |
|------|------|
| vendor/mozjs/mozjs-sys/src/jsglue.cpp | `MallocSizeOf` 顶部 NULL→0 守卫(全平台语义统一,~+10 行);+fail-loud 诊断设施:invalid-parameter handler(打印 CRT 站点)+ SIGABRT 退栈打印器(CaptureStackBackTrace 模块+偏移,无需 PDB)——fastfail 族绕过用户态 handler 时由 SIGABRT 腿取证 |
| src/bao_engine/src/xdr_cache.rs (c2 测试重写) | 每 trial 前 clear memory(store eval 不被残留条目短路);断言从严格时序不等式改为确定性计数器不变量(hits 增 + miss_decode_failed==0 + decode_us>0);严格时序证据归 #26 bench(4.9×)。注:初版加过 `#[cfg(not(windows))]` 门,产品修复生效后**撤销**(禁测试绕行) |
| src/bao_browser/tests/suite/webdriver_signal_exit_e2e_tests.rs | 全文件 `#[cfg(unix)]`(信号语义=unix-only 合法平台门)→ bao-browser suite Windows 编译解锁 |

### 复跑证据(.200 真机,2026-10-17)

| 面 | 结果 |
|----|------|
| bao_engine suite(fresh 二进制,392 tests,含 memory_stats+zone_topology) | **392/392 PASS**,RC=0 |
| bao-mozjs servo_sizes(fresh 二进制 3d7d1bad) | **1/1 PASS**,RC=0 |
| bao_engine lib c2_cold_start(fresh 二进制 08d4604) | **3/3 连跑 PASS** |
| Linux 侧 | xdr+memory+zone+stencil 全家 **19/19**;servo_sizes 1/1;cargo check RC=0 |
| bao-browser suite(#[cfg(unix)] 门后) | **1381/1381 PASS**(Windows) |

### 运维要害沉淀(防复发)

1. **cargo 不跟踪 native static lib(.a)内容变更**——jsglue.cpp 改后依赖测试二进制不自动重链,出现「修了没生效」假象。判别法:`strings <test-exe> | grep <本波新串>`(本波=bao-crash 诊断串,0=stale);处置=强删 `/t/.../deps/<bin>-*.exe` 再 build 强制重链。
2. farm /t 同名 hash 的测试二进制可能多代并存,部署前必须验串,禁按文件名/mtime 取信。
3. msvc 交叉构建中 clang-cl 走 `_MSC_VER` 分支;jsglue 的 Windows 专项代码守卫用 `#[cfg]` 对应的 `defined(_MSC_VER)`,MinGW 形态(如启用)需补 `__MINGW32__` 臂。

### 裁决收口段(2026-10-17,team-lead 四点裁决落地)

| 裁决项 | 落地 |
|--------|------|
| 三门处置 | memory_stats/zone_topology 门撤销(产品根修已验证,门=掩盖层);xdr c2 平台门撤销+方法学根治(计数器不变量形态,8 连绿 Linux+3 连绿 Windows) |
| 诊断设施保留 | jsglue.cpp 两 handler(_MSC_VER 门)+ MallocSizeOf NULL 守卫恒保留 ✓ |
| Linux 复归 | 门撤销后 xdr+memory+zone+stencil 全家 **19/19** 零回归 ✓ |
| servo_sizes 非 CRT 指针理论面 | **.200 stderr 取证:fresh 二进制 RC=0 且 [bao-crt]/[bao-crash] 零触达**——当前测试路径零 invalid-parameter 发生,NULL 守卫对该面充分。「非 CRT 堆指针(HeapAlloc)」为更宽理论面,当前无实证触达;quiet-return handler(镜像 glibc「垃圾但不死」)登记为后续吸收候选(进程级 handler 语义放宽与 fail-closed 有张力,无触达面时不预落地) |
| c2 方法学形态说明 | 裁决 (ii) 中位数+margin 优先——实测证伪:decode/compile 差值被 per-eval realm 构建噪声(数百 µs 同量级)淹没,中位数+任意 margin 仍随负载翻转(380vs287 反向实测)。落定形态=契约重新操作化:decode 路径被走到(hits 增)+零回退(miss_decode_failed==0)+decode_us 记录(三计数器,确定性强),严格时序证据归 #26 bench(4.9×)——职责分工:CI 抓「路径回退」类回归,bench 抓「性能退化」类回归 |

## 平台不支持项清单(编译失败面 / 运行时平台差异面)

### 编译失败面(Windows msvc target 首次暴露)

**产品 lib 失败(1 例,已 farm shim 解锁,主树待属主根治)**:

| crate | 位置 | 失败形态 | 归因类 |
|-------|------|---------|--------|
| bun_runtime | src/bao_runtime/src/bun_dns_resolver_api.rs:237 | E0308:cfg(windows) 分支 `host_arg: Vec<u16>` 直传 `platform_resolve(host_arg: *const u16, ...)` | 平台编译缺陷(Windows 交叉编译准入律样本:cfg(windows) 面从未交叉暴露;farm shim=`#[cfg(windows)] let host_arg = host_arg.as_ptr();`)——**已闭环:e-dnsfix 主树修复与 farm shim 同字节,本合同构建/执行结果即含修复等效形态,下轮 rsync 零冲突** |

**测试目标编译/link 失败面(对应 crate 测试面本轮缺席,登记不修)**:

| crate | 失败 target | 失败形态 | 归因类 |
|-------|------------|---------|--------|
| bun_http | lib test + 7 个 test target | lld-link undefined:`WindowsNamedPipe__*` / `__bun_stdio_blob_store_new` / `__bun_crash_handler_*` / `uv_disable_stdio_inheritance` / `ares_inet_pton` / `Bun__addrinfo_cancel` / `GetProcessMemoryInfo` | 平台不支持(Windows 测试面 C stub/link 缺口族) |
| bao_boringssl_bridge | lib test + sni_loopback/session_resumption | lld-link undefined:`bun_ssl_ctx_cache_on_free`(bun_usockets openssl.c Windows 面) | 平台不支持(同 link 缺口族) |
| bao_cdp_client | test "suite"(全引擎 harness) | lld-link undefined:`bun_ssl_ctx_cache_on_free` 族(us_socket crypto Windows 面) | 平台不支持(同 link 缺口族) |
| bao-mozjs-normalizer-glue | lib test | lld-link undefined:`js_call_js_normalize_latin1/utf16` / `js_new_ucstring_copy_n_dont_deflate`(mozjs glue C 面) | 平台不支持(同 link 缺口族) |
| bao_native_stubs | lib test | link 失败(1 error,详见 build2 log) | 平台不支持(同 link 缺口族) |
| bun_shell_parser | lib test | 1 error(待 per-crate 详查) | 平台不支持(初判 link 族) |
| bun_clap | lib test | 1 error(待 per-crate 详查) | 平台不支持(初判 link 族) |
| bao_workflow_host | lib test + test "host_runtime" | `pthread_attr_t`/`pthread_getattr_np`/`pthread_self`/`pthread_attr_getstack`/`pthread_attr_destroy`(libc Windows 无 pthread) | 平台不支持(unix-only API 测试面) |
| bao_uloop | lib test(17 errors) | `std::os::fd` / `libc::SOL_SOCKET/SO_SNDBUF/SO_RCVBUF` / `std::os::unix`(测试代码 unix-only API 无 cfg 门) | 平台不支持(unix-only API 测试面) |
| bun_js_parser | lib test(5 errors) | `pthread_getattr_np` 族(栈探测测试面) | 平台不支持(unix-only API 测试面) |

## 真缺陷清单(平台无关逻辑在 Windows 红)

| crate | 测试名 | 失败形态 | 归因 |
|-------|--------|---------|------|
| (待填) | | | |
