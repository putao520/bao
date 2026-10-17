# 行覆盖率分母治理登记(Goal C,2026-10-17)

**测量管道**:cargo-llvm-cov 0.9.1 + `CARGO_BUILD_PROFILE=test-ci cargo llvm-cov nextest -j 4 --run-ignored=all -E '<排除 filterset>' --lcov`(坑位:`--profile` 被 llvm-cov 转发给 nextest,须 env 路线;`--no-fail-fast` 与 `--ignore-run-fail` 互斥,by-design 外部依赖面用 `-E` filterset 排除)。

**电池载体**:11683 tests run / 11683 passed / 0 failed / 2 skipped(排除=`e2e_real_chrome_navigation_and_screenshot`+`e2e_real_chrome_runtime_evaluate`,真 Chrome env-gated by-design,贡献≈零覆盖)。二进制 provenance=llvm-cov-target instrumented(test-ci via CARGO_BUILD_PROFILE)。

## 层总账(bao 自有 crate 层,150841 LF)

| 口径 | LF | LH | line |
|---|---|---|---|
| bao 自有层(bao_runtime/browser/cdp/cdp_client/stealth/engine/uloop/cli/bundler/crypto/cdp-server/http/bao) | 150841 | 111896 | **74.18%** |
| 全仓(lcov 全集,含 bench/未测 bun 层,分母无意义,登记备查) | 446993 | 183816 | 41.12% |

基线对照:Z.39 基线 line 74.74%(region 218547,口径未存档);本波 74.18% 为**可复现管道首建**(lcov 全文 `/tmp/goalA/coverage-final.lcov` + 判定器 `cov_analyze.py`)。口径差异=基线 scope 未存档,本表起以 bao 自有层为准。

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
- 测量法伪影(2→✅ 已解):bao_cli/src/{cli,doctor}.rs——原 spawn 二进制非 instrumented(llvm-cov nextest 模式不产 bin 目标);**bin-mode 脚本已落地(scripts/coverage-cli-bin-mode.sh),cli.rs/doctor.rs 计入可达测**(bin-mode 实测 71.18% / 86.96%,见 C9 台账)。注意口径:bin-mode 数字为 spawn-face 口径(instrumented bin 单独 export),与主管道 nextest 口径同分母不同数据源,加总看缺口、不可直接合并成单一 line%。LF 漂移说明:cli.rs 510 vs 本账旧值 498 = e-cliarg 等在飞 WIP 增行,非口径变化。
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

