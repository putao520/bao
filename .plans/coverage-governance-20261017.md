# 行覆盖率分母治理登记(Goal C,2026-10-17)

**测量管道**:cargo-llvm-cov 0.9.1 + `CARGO_BUILD_PROFILE=test-ci cargo llvm-cov nextest -j 4 --run-ignored=all -E '<排除 filterset>' --lcov`(坑位:`--profile` 被 llvm-cov 转发给 nextest,须 env 路线;`--no-fail-fast` 与 `--ignore-run-fail` 互斥,by-design 外部依赖面用 `-E` filterset 排除)。

**电池载体**:11683 tests run / 11683 passed / 0 failed / 2 skipped(排除=`e2e_real_chrome_navigation_and_screenshot`+`e2e_real_chrome_runtime_evaluate`,真 Chrome env-gated by-design,贡献≈零覆盖)。二进制 provenance=llvm-cov-target instrumented(test-ci via CARGO_BUILD_PROFILE)。

## 层总账(bao 自有 crate 层,150841 LF)

| 口径 | LF | LH | line |
|---|---|---|---|
| bao 自有层(bao_runtime/browser/cdp/cdp_client/stealth/engine/uloop/cli/bundler/crypto/cdp-server/http/bao) | 150841 | 111896 | **74.18%** |
| 全仓(lcov 全集,含 bench/未测 bun 层,分母无意义,登记备查) | 446993 | 183816 | 41.12% |

基线对照:Z.39 基线 line 74.74%(region 218547,口径未存档);本波 74.18% 为**可复现管道首建**(lcov 全文 `/tmp/goalA/coverage-final.lcov` + 判定器 `cov_analyze.py`)。口径差异=基线 scope 未存档,本表起以 bao 自有层为准。

## 2026-10-18 farm 终测(battery-4,首次跨机可复现)

**测量面**:farm 16.18.0.1 容器(64 核)+ `CARGO_BUILD_PROFILE=test-ci BAO_TEST_BAO_BIN=/t/test-ci/bao BAO_TEST_NETWORK=1 BAO_TEST_DNS=1 BAO_TEST_REAL_SERVO=1 DISPLAY=:99 cargo llvm-cov nextest -j 8 --run-ignored=all --ignore-run-fail -E 'not test(e2e_real_chrome) and not test(puppeteer)' --lcov`。lcov 全文 `/var/cargo-builds/bao-cov-final/coverage-final.lcov`(27.8MB)。

| 口径 | LF | LH | line |
|---|---|---|---|
| **bao 自有层(同上口径)** | **150696** | **122947** | **81.59%** |
| 全仓(lcov 全集) | 446812 | 195323 | 43.71% |

**对照 74.18% 首建:+7.41pp**。分解:①分子 +11051 覆盖行(C2-C10 缺口波+#71 修复解锁面+#74 三缺陷负测试+#69 notify 后端+cli bin 面经 BAO_TEST_BAO_BIN 首次入主管道)②分母 −145(C10 `#[coverage(off)]` 分母排除首次机械生效:webcore_faces 99+bao_native_stubs 200 注解面出插桩)。**98% 字面值判定不变**(天花板核算:排除杠杆上限 +3.2pp,缺口由真功能代码构成;81.59% 为结构上限路径的当前实测点,续推=缺口排行+#76/#75 能力波)。

**电池形态变化**:12008 tests(较首建 +325=C1-C10 新测),24 失败=农场环境面(node_dns c4 ×10=容器 DNS 形态/udp_ffi_shell ×2/spawn_sigterm_reset ×2/h3_fetch ×2/cluster ×4/cp_path_tty c5 ×2=#75/#76 已立案精确吻合)+puppeteer 族测量排除(内存墙,非覆盖结构)。**#77 判别落定**:cli-spawn 面家族红=测量 env 缺口(BAO_TEST_BAO_BIN 漏带),非产品回归——补带后 cli_faces/eng004/bao_cli_timeout 全绿。

**farm 测量基建沉淀**(坑位入 memory llvm-cov-pipeline-pitfalls):本机多会话 OOM 四连(−j4→−j1 均破界)→farm 路线;farm 容器三坑=缺 cargo-nextest(装 `CARGO_HOME=/usr/local/cargo cargo install cargo-nextest --locked`,缺前缀落 /root/.cargo 随 --rm 消失)/llvm-tools 每 run 重下(rustup home 非持久,2 分钟容忍)/BAO_TEST_BAO_BIN 必带(spawn 面无 bin=大面积假红)。

## Goal C 六目标文件兑现(本波补测)

| 文件 | 基线未覆盖行 | 本波 line | 载体 |
|---|---|---|---|
| web_api.rs | 479 | **88.04%**(gap 368) | web_api_ws_edge + web_api_subtle_edge(57 例矩阵+WS 真 RFC6455) |
| engine_props.rs | 232 | **92.69%** | engine_props_edge(install 全体+per-Realm+异常消费) |
| addrinfo.rs | 84 | **94.01%** | uloop_edge(C-ABI 快路径全臂+缓存+QUIC 注册) |
| web_streams.rs | 44 | **99.00%** | web_api_subtle_edge(on_target 直调+行为面) |
| uloop/lib.rs | 92 | (路径 suffix 未匹配,lcov 重切后补记) | uloop_edge 入口点+FFI 臂 |
| uloop/poll.rs | 33 | (同上) | uloop_edge 位域访问器 |

诚实边界(e-coverc 已登记):poll.rs `dispatch_ready_polls` 私有臂与 lib.rs `us_dispatch_*` 实 socket 臂 crate 内测试已覆盖,bao_runtime 侧不可达;`Bun__addrinfo_set` completed 即时通知臂需真 usockets socket。

## 缺口结构(98% 路径账,gap=38945)

**top-30 文件=70% 缺口(27101 行)**——全部为可达-未测透产品面(非结构性),按域分组(后续波合同域):

| 域 | 文件(gap) | 合计 |
|---|---|---|
| runtime 大文件增量 | node_fs(4523)/node_crypto(2391)/bun_api(2102)/globals(1427)/bun_listen(1414)/node_http2(1389)/node_dns(1253)/child_process(1128)/node_path(972)/node_tls(682)/bun_ffi(581)/bun_shell(467)/node_util(451)/fetch_async(407)/bun_password(404)/node_tty(383)/node_http(374)/node_vm(355)/lib(348) | ~20,100 |
| browser/cdp 增量 | cdp_handler(798)/page(701)/webdriver_host(377)/protocol(410)/bridge_sender(374)/servo_backend(323) | ~2,980 |
| http 层 | lib(1083)/HTTPThread(337) | ~1,420 |
| CLI 内部(测量法缺口) | cli.rs(498)/doctor.rs(229) | ~727 |

**0% 触达文件(LF≥40)9 件=1337 行**,分类:
- 测量法伪影(2→✅ 已解):bao_cli/src/{cli,doctor}.rs——原 spawn 二进制非 instrumented(llvm-cov nextest 模式不产 bin 目标);**bin-mode 脚本已落地(scripts/coverage-cli-bin-mode.sh),cli.rs/doctor.rs 计入可达测**(bin-mode 实测 71.18% / 86.96%;证据=/var/cargo-builds/bao-cli-cov-target/coverage-out/cli-bin-mode.lcov,主会话独立复算一致)。注意口径:bin-mode 数字为 spawn-face 口径(instrumented bin 单独 export),与主管道 nextest 口径同分母不同数据源,加总看缺口、不可直接合并成单一 line%。LF 漂移说明:cli.rs 510 vs 本账旧值 498 = e-cliarg 等在飞 WIP 增行,非口径变化。
- 可达-未测(7):http/h3_client/{encode,PendingConnect}(219)/bao_crypto/certificate(126)/http/websocket(109)/bao_runtime/interrupt_bridge(94)/bun_password(404 实为低覆盖非 0)/dispatch(94)——后续波合同候选。

## 98% 收敛路径(后续波,非本波)

层 98% 需再覆盖 ~35,900 行:①runtime 大文件 19 件的增量合同(每文件 1-2 单,evaluate_js 探针+负向面形态,估 20+ 波);②CLI bin-mode 测量法已落地(scripts/coverage-cli-bin-mode.sh,C9 收官,cli.rs/doctor.rs 计入可达测);③结构性排除逐项证据登记(零按比例排除)。本波兑现=管道+基线+六目标+本账。

## C1-C9 补测波台账(2026-10-17 凌晨,并发 11 波)

| 波 | 域 | 交付 | 状态 |
|---|---|---|---|
| C1 | node_fs | fs_coverage_c1_tests.rs | 收口(scoped 绿) |
| C2 | node_crypto+bun_api | crypto_coverage_c2 + bun_api_coverage_c2(向量按 RFC 4231/5869/7914/FIPS-197 真值钉死,node v24 oracle) | 收口 |
| C3 | globals+bun_listen | globals_coverage_c3(15)+ bun_listen_coverage_c3(22)=37 用例;scoped 37/37+邻族 71/71 零回归+RC=0;分支对照表齐(listen HTTP 面从零到全/connect 生命周期双臂/TCP write-end arms/globals 十余 install face) | ✅ 收官 |
| C4 | node_dns+node_http2 | node_dns_coverage_c4(890 行,14 test/~95 断言)+ node_http2_coverage_c4(1359 行,8 test/~120 断言);组合 scoped 21/21(root-fix 后 14 连绿);真外网 RR 形状(BAO_TEST_DNS=1,MX/TXT/NS/SOA/AAAA/SRV/NAPTR)+真 PEM h2 secure 实跑;flake 根治(break 条件单 marker→双 marker,microtask hop 落后,14 连绿非环境隔离) | ✅ 收官 |
| C5 | child_process+tty | cp_path_tty_coverage_c5 | 收口 |
| C6 | bao_cdp protocol | protocol_subcommand_full_coverage_tests.rs(79KB,main.rs 已挂) | 收口 |
| C7 | http 层+certificate | websocket_header(12)+certificate(14)+fetch_error_paths(12)+idle_timeout(1);121/121 全绿 | ✅ 收官 |
| C8a | udp+ffi+shell | udp_ffi_shell_coverage_c8a(47 test=43 live+4 #[ignore] 带理由);scoped 47/47+相邻族 160/160+RC=0;原 agent 死→主会话接线→c8b-node 承接收尾(9 测试错修复:ffi 系统性根因=伪造不存在的 libc 符号 key,eager dlsym fail-closed;重映射真符号) | ✅ 收官 |
| C8b | tls/util/vm/http/url/zlib/cluster/lib/fetch_async | util_edge(10/87)+vm_edge(5/55)+runtime_misc_edge(11/60);scoped 26/26+相邻族 261/261 | ✅ 收官 |
| C9 | CLI bin-mode 测量 | cli_faces.rs(1043 行,41 test)+scripts/coverage-cli-bin-mode.sh(私有 dir instrumented bao bin(`-C instrument-coverage` test-ci)→ BAO_TEST_BAO_BIN spawn 产 profraw(4498 个)→ llvm-profdata merge → llvm-cov export --format=lcov --object 单 bin;硬门=cli.rs/doctor.rs LH>0 否则 exit 1;实测 60/60 绿,端到端 SCRIPT_EXIT=0) | ✅ 收官(bin-mode 实测:cli.rs **71.18%** LF=510/LH=363;doctor.rs **86.96%** LF=230/LH=200;lcov=/var/cargo-builds/bao-cli-cov-target/coverage-out/cli-bin-mode.lcov) |

### 波内具名登记(产品缺陷/parity 偏离,characterization 钉死未修)

**C1 node_fs 19 项**(完整对照表见 fs_coverage_c1_tests.rs 头注+波报告):
1. parse_open_flags 无 'x' → wx/ax 应 EEXIST 实为截断/追加(O_EXCL 缺)
2. writeSync/write 忽略 (offset,length,position),全量写 cursor
3. readvSync 返回字节数但读入临时副本,caller Buffer 保持全零
4. rmSync/rm 忽略 {force:true},missing 照抛 ENOENT(node 静默)
5. utimesSync 未注册;6. lutimesSync 未注册
7. io_error_code 无 EBADF 臂 → fd 错误 code="ERR"(node:"EBADF")
8. fs.mkdir cb 错误路径硬编码 "EACCES";9. fs.open cb 提取 idx≥3 → 3 参形态回调永不触发
10. fs.writeFile 无 callback 分支(cb 静默忽略);11. readv/writev async 委托不展开 iovec(0 字节)
12. fs.exists cb 投递 (undefined,exists)(node:单参);13. fh.readFile 恒 Buffer,encoding 参忽略
14. fh 双重 close 解析成功(node:ERR_INVALID_HANDLE)
15. **Dir 不可迭代 — Symbol.iterator 注册误用字面名 "[]"**(node_fs.rs:10591 `c"\x5B\x5D"`)
16. FileHandle 原生普通函数,new 抛 not-a-constructor;17. fs.cp 回调形态 cb 永不被调用
18. async readFile 无编码投递 String(sync 面 Buffer,node 均 Buffer)
19. 操作性发现:fsw inotify worker 在飞时 Rust 直调 `timers::drain_and_check` 于裸 cx → SIGSEGV 无 panic;安全形态=bounded post_eval_hook 在 ctx.eval 内泵(watch/stream 测试已按此形落地)

**C8b 6 项**:
1. **TLS-RENEGOTIATE-NAME-TYPO**(node_tls.rs:2632):`c" renegotiate"` 前导空格 → `socket.renegotiate` 产品面 undefined(一行修,待立单)
2. **HTTP-AGENT-CTOR-MISSING**(node_http.rs:878):`http.Agent` 经 Evaluate2 跑顶层 function 声明,completion value undefined → 属性永不注册(globalAgent 在)
3. util.format `%`-path 不追加未消费参数(node 追加)
4. assert._matchSpec validator 函数形永不调用(prototype!==undefined 判别错;node 用 prototype instanceof Error)
5. vm_strings_disabled 报 Error 非 EvalError
6. zlib deflateRaw/inflateRaw 无异步注册

**运维登记**:
7. 共享 CARGO_TARGET_DIR 撞车实锤:scoped 跑曾链接陈旧 rlib(mtime 新于源、内容旧),touch lib.rs 强制重建后消——**llvm-cov 终判务必安静树重建**
8. c4 h2_listen_lifecycle_eaddrinuse 并发负载端口争用 flake(隔离复跑 PASS),非回归
9. **插桩×C8a 交互缺陷 → ✅ 已根治(c8b-node,task #55)**:根因坐实=UDP-RELOAD-DANGLING-ROOTED-HANDLE——udp_reload 的 sock_obj RootedGuard 声明在 if 表达式臂内,臂末即 drop,而 sock_h(指向 guard 栈槽)被其后全部 extract_js_callback+gc_store_insert 消费;普通 profile 悬垂读碰巧合法(顶层 else 臂健康),插桩 GC 时序右移→必崩。**修法**=bun_udp.rs 提取共享 helper `reload_ud_callbacks`(四组 extract+gc_store 逐字保留),两臂各自以块作用域 rooted! 包住调用(root 活到块末覆盖全窗口),+56/-39 行为零变化。验证:插桩 49/49+普通 49/49(2 个 #[ignore] 撤除复活)+相邻族 162/162+RC=0。**缺陷登记状态→fixed;对应 2 ignore 撤除,C8a 忽略账 4→2**(余 2=SHELL-BYTES-NULL-ISHARED-CRASH/FFI-F32-RETURN-XMM0-CONVERSION,待各自根治单)

10. **/tmp 大树被外部清理中途扫掉(2026-10-17,c9-binmode 实锤)**:47 分钟 instrumented 冷构建的 /tmp/bao-cli-cov-target 被外部清理机制砍到 593M(mozjs build 目录全失),构建报废重跑——**长构建产物一律落 /var/cargo-builds,禁 /tmp**(脚本默认路径已按此设)。
11. **lsquic 原生库链接指令在 fresh 全图首链丢失 → ✅ 已根治(2026-10-17,c9-binmode 发现,主会话根因+产品修)**:**根因=src/lsquic_sys/Cargo.toml 缺 `links` key**——build.rs 的 `cargo:rustc-link-lib=static=lsquic/lshpack` 不向下游转发,符号只靠增量 accretion 时 lsquic rlib 恰排在引用方(uws_sys rlib)之后的单遍归档扫描侥幸解析;canonical 共享 target 天天绿系增量掩盖,fresh 全图首链(私有 instrumented target)确定性 44 个 undefined lsquic_*。最小 probe(独立 workspace+强制 us_quic_global_init 引用)无 RUSTFLAGS 同样复现=与插桩无关。**产品修=[package] 加 `links = "lsquic"`**(cargo 机制正名:声明后发射转发到最终链接行尾部,顺序无关,全构建形态受益);决定性实验=尾附 -llsquic/-llshpack 链接成功坐实顺序假设。脚本内两阶段 -L/-l 附加降级为纵深防御保留(防 manifest 修回退),RUSTC_WRAPPER="" 绕 sccache(插桩全 miss 零收益+server 死锁实证一并防)。

12. **私有 target 双写者竞态(2026-10-17,c9-binmode vs 主会话接管期实锤)**:12:14 脚本 `rm -rf` 私有 target 与主会话 build4 并发相撞,中途文件消失致构建失败——「共享 target 撞车」教训(#7)的双写者变体。**裁定:私有 target 单一写者**,接管期间由接管方声明写权或停手;脚本幂等重建前先确认无并发写者。

### 测量法缺口(分母治理账追加)

- lib.rs ~300 行 seam 体 = 链接期 face 测量法伪影(同 cli.rs 类,入分母治理账)
- vm.Script 无 new fallback 臂 + util 数组 idx>=length 臂 = JS 侧不可达(结构性)

**C4 结构性边界(诚实排除候选,复测时核验)**:node_dns windows cfg 臂(Linux 不编译)/ resolve_rr_cares `_ =>` ENOTIMP 死臂 / status_to_code 特定错误映射臂;node_http2 四个 fallback 变体(仅 IIFE ctor 缺失时可达=实践死码)/ route-handler 无 realm 500 臂(需无 realm 线程)/ JS 字符串行(非 coverable Rust 行)

**C3 shadowing 死代码发现(unreachable-by-shadowing,~500+ 行除账候选)**:活体 `Bun.udpSocket` = `bun_udp.rs`(bun_api.rs:614 后装覆盖 bun_listen.rs:610 绑定)→ bun_listen.rs UDP 面(:1873-2311/:3200-3282)为活体 JS 面死代码;`crypto_subtle_digest`(globals.rs:5478)同理被 `web_api::install_crypto_subtle` Promise 版覆盖。两块应从 bun_listen.rs/globals.rs 可达 gap 中除账。

**C3 产品缺陷登记**:①binary-bytes-remoteAddress 类:usockets 3-arg `us_socket_remote_address`/`us_udp_socket_bound_ip` 拷贝裸二进制 sockaddr 字节,bao 侧 4 处直接 UTF-8 字符串化 → remoteAddress/localAddress/udp address() 乱码(修法在 vendor 现成:libuwsockets.cpp:1809 5-arg info+ares_inet_ntop);②install 非重入:install_all 双调 SIGSEGV(source 自证)+ **install_node_apis 单独双调也 SIGSEGV**(本轮实证,超 source 注释范围),测试钉 once-per-realm 契约;③环境不可达:V6 loopback(本机 EADDRNOTAVAIL)/cx-null 臂=harness by-design。

**C8a 产品缺陷登记(6 项,characterization/#[ignore] 钉死)**:
1. UDP-ADDRESS-RAW-BYTE-IP(bun_udp.rs udp_address):bound_ip 4 字节裸 IPv4 当 C 字符串,内嵌 NUL 截断 → 127.0.0.1 显 "\u{7f}"(与 C3 发现同类)
2. **UDP-RELOAD-DANGLING-ROOTED-HANDLE**(bun_udp.rs udp_reload):sock_h 取自 if 臂内 rooted! guard,臂末 guard drop → Handle 悬垂,socket{} 形态 reload 必 SIGSEGV(CLI 二分实证)
3. **SHELL-BYTES-NULL-ISHARED-CRASH**(bun_shell.rs:1158):JS_GetUint8ArrayData 第二参 bool* is_shared 传 null → 每次 .bytes() 必崩(全仓其余调用点都传真槽);SPEC 锚=REQ-BAO-API-018 criterion 5
4. FFI-CALLBACK-PROTO-MISSING(bun_ffi.rs):ffi.callback() 用 JS_NewObject 裸 class 建实例,FfiCallback.prototype 的 close/ptr 永不挂上
5. FFI-F32-RETURN-XMM0-CONVERSION(bun_ffi.rs):F32 返回臂 `ret.xmm0 as f32` 是裸位槽值 cast 非 low-dword bit-cast
6. zlib deflateRaw/inflateRaw 无异步注册(与 C8b 同款)

## 构建治理落地登记(REQ-ENG-BUILDGOV-001,2026-10-18)

size/compile 双审计(`.plans/audits-20261017/{size,compile}-audit-report.md`)批准矩阵的落地态登记。工程治理面,零业务能力变化;审计矩阵即选型依据真源。

| 项 | 审计锚 | 落地态 |
|---|---|---|
| **P0 发行物 strip** | size-audit §3 P0 | `[profile.release] strip="symbols"`——审计矩阵 135MiB 级路径(strip="debuginfo" 162MiB 备选未取:−27MiB 换 symtab 全失,本地符号需求已由 release-dbg 承接)。取证面核实无损:crash_handler release 形态本就不做进程内行号查找(dladdr `source_location: None`→`???:?:?` 系 strip 前即有形态),生产符号化走 trace-string+服务端重映射;`.dynsym` 在 strip --strip-all 后存活,导出符号解析不受影响。审计实测 strip 后变体 codegen 字节级等价(差 23,664B 布局噪声=0.017%) |
| **P1 调试分离** | size-audit §3 P1 | `[profile.release] debug=0`(旧 line-tables-only 在 rustc 实产函数级 DIE+行表,占 614MiB 的 debug 族 347MiB;debug=0 −52%→283MiB 级)+ 新增 `[profile.release-dbg]`(inherits=release + `debug="line-tables-only"` + `strip=false` 双显式覆盖,防继承下漏;镜像 test-ci/test-ci-dbg 先例;codegen 与 release 字节级同形,本地 gdb/perf 载具)。C 侧不受 profile 分裂影响(build.rs 编译器 flag 全 profile 共享) |
| **P2 mozjs C 面线表化** | size-audit §3 P2 | **BLOCKED——审计前提实测证伪,未落地**。审计断言「gcc 13 支持该 flag」:本机实测 gcc 13.3.0(Ubuntu 24.04 构建链,即 mozjs 906 CU 的实际 producer)对 `-gline-tables-only` 与 `-gline-directives-only` 均报 `error: unrecognized debug output level`——两 flag 是 clang 语义,gcc 无 line-tables-only 等价物(`-g1`=行表+函数/变量 DIE 已是最低非零档,audit 自己在 §2 修正过「两家互认」对 gcc 不成立,却在 P2 通道②再次误判)。`vendor/mozjs/mozjs-sys/build.rs` 的 `MOZ_DEBUG_FLAGS="-gdwarf-4 -g1"` 保持原样未动——写入未识别 flag 会令全部 mozjs 构建响亮失败;注释级改动也会翻转 build-script fingerprint→mozjs C 全量重编,零功能收益不做。候选待属主裁决:a) 接受 -g1 现状(P2 收益归零;strip 后发行物不受影响,仅 dev/release-dbg 本地形态与 .a 中间产物持有 C 调试信息);b) gcc `-gsplit-dwarf` 机制(重型调试出归档/终产物,build 树留骨架+.dwo 需 dwp 闭环——机制变更需批准);c) mozjs C 编译器换 clang(该 flag 原生可用,工具链级变更)。**连带登记**:`.cargo/config.toml:19` 注释引 `MOZ_DEBUG_FLAGS="-gdwarf-4 -g1"` 的行在 P2 落地时需同步,本波 scope 外(本波未动 build.rs,该注释仍与现状一致,无 stale) |
| **ROI-2① 测量树收敛** | compile-audit §4 ROI-2 | `scripts/coverage-cli-bin-mode.sh` 默认 CARGO_TARGET_DIR 迁移:`/var/cargo-builds/bao-cli-cov-target` → `<cargo 解析 target-dir>/llvm-cov-target`——与主管道 cargo-llvm-cov 同根(llvm-cov-target 为幸存测量树,活跃禁删),sibling of `debug/`/`test-ci/` 的同级约定;解析顺序=CARGO_TARGET_DIR env > `cargo metadata` 镜像 build.target-dir(config.toml 锚)> `$REPO_ROOT/target` 兜底(三路径探针全验)。**收敛是治理单根非缓存复用**:bin 树与 llvm-cov test 树 feature-unification 不同,工件不互救,冷树首跑仍全量。`BAO_CLI_COV_TARGET_DIR` 覆盖语义不变。**bao-cli-cov-target(117G)本波不删盘**,冷置留待属主清理裁决 |

**验证(criterion 5)**:`cargo check -p bao-mozjs-sys --jobs 4` RC=0(5m56s;任务文本 `mozjs-sys` 为 fork 改名前旧称,实际包名 `bao-mozjs-sys`)+ `cargo check -p bao_bin --profile release --jobs 4` RC=0(12m27s,profile 重配后 release 树全量 check 重编)+ 脚本 `bash -n` 过+三路径解析探针过。


## C11-C17 波台账(2026-10-18,7 波收口;缺口语汇 C11-C10 基线续账)

| 波 | 域 | 交付 | commit |
|---|---|---|---|
| C11 | node_fs+node_crypto(2710 gap) | 18 test/1467 行+**9 处 oracle 产品修**(O_EXCL/EBADF/Dir asyncIterator/fs.open cb idx/fs.constants/fstatSync/crypto async cb 门×5/generateKey bits→bytes/caller buffer 回填+getRandomValues 守卫)+C1 复核(#14 证伪/6 维持/6 新登记/8 断言翻新) | fc0c8c3a5 |
| C13 | dns/http2/path(3614) | 3 文件 1663 行/19 test/204 断言(h2 upgrade 全模块 C4 零触碰面首覆盖;win32 11 方法 oracle 矩阵);5 登记 | 07b181222 |
| C15 | http 层+0% 面(2156) | 7 文件 1840 行/53 test/220 断言(proxy 真路径 C7 空白+CONNECT 隧道端到端+interrupt_bridge 真信号三态);5 登记 | 8df70a94f |
| C14 | browser/cdp(2983) | 4 文件 2310 行/41 test/358 断言(protocol 30+ 命令臂/blanket 转发恒等/cdp_handler 引擎面/page wrapper);6 登记 | 02c9c9ec7 |
| C17 | util/fetch_async/tty/lib(1589) | 2 文件 1032 行/11 test/78 断言(真内核 pty 字节级 tty 面/inspect 类族/fetch 真线回环);15 登记(T5=clearLine debug-assert abort 真缺陷列修复候选) | 7f4869c3a |
| C12 | bun_api/globals/bun_listen(4943) | 在飞(门测) | — |
| C16 | ffi/shell/password/vm/http(2181) | 在飞(门测) | — |

**登记顶总账(characterization 钉死未修,修复候选池)**:C11 6 新登记+C13 5+C14 6+C15 5+C17 15 = **37 项**;另 C11 波 9 处产品修已根治入 commit。结构性残账:fs ~800-900(readv/writev Send 约束/Dirent d_type)/crypto ~200-300(bigint/RSA)/h3 需活 QUIC 会话/webdriver_host 377 行 feature 门控测量缺口(需专用载体)/lib.rs seam 体 ~300 行(链接期 face,入分母治理)。

## llvm-cov 终测尝试台账(2026-10-18,8 启全败;逐启归因留痕)

**结论:81.59% 保持为最后实测验证数字;C11-C17 增量未获测量(禁止引用估算值冒充实测)。**

| # | 宿主 | 形态 | 败点 | 归因 |
|---|---|---|---|---|
| 1 | 本地 | -j4 | 5849/12245 cli_snapshot 红 | BAO_TEST_BAO_BIN 指向 10-17 中毒陈旧 bin → 已根治(BCE-STALEBIN c80efc344) |
| 2 | 本地 | -j4 | 9611/12245 c2_random_bytes 红 | C11 typed-array 守卫治愈的旧期望 → 已修(e44048b08) |
| 3 | 本地 | -j4+retries2 | 9733 shell_signal_exit 红 | C16 登记 #[ignore] 面被 --run-ignored=all 拉入 → 排除(有案) |
| 4 | 本地 | -j4+retries2 | 9545 wave_a_surface 红 | 负载时序类(2/3 负载红,隔离绿) → 排除 |
| 5 | 本地 | -j4+retries2 | 10362 ffi_f32 红 | 负载时序类 → 未及排除 |
| 6 | 本地 | -j2+retries3 | 宿主 OOM 杀 | 多会话 swap 基线(78G swap 在用),truly-solo 不成立 → 转 farm |
| 7 | farm | -j4+retries3 | 1770/12005 puppeteer 红 | "node not found" — governance 已判测量排除而 -E 漏带 |
| 8 | farm | -j4+retries3+puppeteer排除 | 枚举期:28 cli_faces 系统性 0.047s 瞬败 + 10 farm-env 面(dns×5/h3×2/c5_cp/spawn_sigterm/ffi_f32,均为 10-17 farm 电池已登记同族) | cli_faces 瞬败疑 stalebin canary 在 farm 容器的行为差异(未取到 panic 原文:probe 容器被共享 farm-cargo 卷锁楔死 7min+，宿主另有 mw-ci/desktop/pipeline/ai-agent 四项目容器并发) → **开放缺陷登记:cli_faces farm 面 stalebin canary 待下次 solo 窗取证** |

**排除集(8 面,全部有案可查)**:e2e_real_chrome×2(外部 Chrome env)→ puppeteer 族(farm 无 node)→ cp_async_push_exit_latency_100x / spawn_sigterm_kill_converges / wave_a_surface / ffi_f32(负载时序类,隔离均绿)→ shell_signal_exit_code_arm / shell_output_method_this_arms / http_server_aborted_mid_body(C16 登记 #[ignore] 不可达面)。

**环境判例更新**:farm 宿主已非 bao 独占(四项目容器并发共享 farm-cargo/farm-rustup 卷)——llvm-cov 终测须 solo 窗或申请独占槽;本地终测须 truly-solo(swap 基线 <10G)。下次测量建议:Windows 侧或 farmsolo 窗口,先跑一次 no-fail-fast 枚举拿全红名单再启电池(逐启打地鼠的代价本轮实证=8 启)。
