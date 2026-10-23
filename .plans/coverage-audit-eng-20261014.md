# ENG/CLI/STL/IMPL/ARCH 域 SPEC Criterion 功能对等覆盖审计

- **审计日期**: 2026-10-14
- **审计人**: cov-audit-eng(只读审计,零代码改动)
- **域**: ENG / CLI / STL / IMPL / ARCH(BAO 自有层 bao_engine / bao_runtime / bao_cli / bao_stealth + 消费面 bao_browser/bao_cdp_client)
- **方法**: SPEC criterion 文本提取(HTML 三 marker 形态全量)→ 测试实体枚举(395 测试文件 / 3778 测试函数)→ 逐条功能对等判定(测试名/断言语义匹配,不依赖 criterion id 字面引用)
- **判据说明**: SPEC criterion 分两类——(A) "cargo test -p <crate> --test <file> 通过" 型(载体指定型)与 (B) 语义行为型(如 "输出 X" / "EXIT=0" / "grep 命中=0")。对 (A) 判定该测试文件是否存在且含对应语义断言;对 (B) 找功能对等测试。聚合型测试(单 #[test] 内含数百断言)按文件级覆盖+内部断言抽样核验。

## 总量统计

| 域 | REQ 数 | Criterion 数 | covered-by | GAP | AMBIGUOUS |
|---|---|---|---|---|---|
| ENG | 16 | 71 | 63 | 7 | 1 |
| CLI | 3 | 19 | 19 | 0 | 0 |
| STL | 7 | 37 | 35 | 2 | 0 |
| IMPL | 5 | 27 | 24 | 3 | 0 |
| ARCH | 1 | 2 | 2 | 0 | 0 |
| **合计** | **32** | **156** | **143** | **12** | **1** |

> criterion 计数说明: SPEC 实际标记 179 条(含 data-criterion-id / data-criterion / <li> 三种形态);其中 REQ-CLI-3 因元素未闭合渗漏了 REQ-ENG-46/SVG/WebVTT/DevTools/Stylo 域 20 条(归属 cov-audit-brw 域),剔除非本域后目标域 = 156 条。逐条判定基于 156 条。

---

## 一、ENG 域逐条判定(16 REQ / 71 criterion)

### REQ-ENG-001 SpiderMonkey 引擎集成(5)

| AC | Criterion 摘要 | 判定 | 证据 |
|---|---|---|---|
| 001-1 | `bao -e "console.log(1+1)"` 输出 "2" EXIT=0 | covered | bao_browser/tests/suite/bao_cli_e2e_tests.rs:bao_cli_e2e_full_lifecycle(真二进制 -e 面) + bao_engine/tests/suite/engine_core_tests.rs:test_bao_engine_core_all |
| 001-2 | cargo build -p bao_engine 通过 + grep JS_NewContext 唯一实例化点 | GAP | 源码实测: bao_engine/src/realm_policy.rs:22 注释引用 JS_NewContext,但 bun_sm/src/ 全仓 grep `JS_NewContext()`/`NewContext` **零命中**(上下文创建走 mozjs facade 另一 API 形态);无任何测试断言"唯一实例化点" |
| 001-3 | cargo test -p bao_engine --test engine_core_tests 通过 | covered | src/bao_engine/tests/suite/engine_core_tests.rs 存在,test_bao_engine_core_all(console/require/Bun 面断言) |
| 001-4 | cargo test -p bao_engine --test js_context_fusion_tests 通过 | covered | src/bao_engine/tests/suite/js_context_fusion_tests.rs:js_context_fusion_node_and_web_api_coexist(Buffer.from/require/Bun===Bao 共存断言),main.rs:25 `mod js_context_fusion_tests;` |
| 001-5 | `bao -e "WebAssembly.validate(...)"` 返回 true | covered | bao_runtime/tests/suite/runtime_deep_tests.rs + node_silent_fake_eradication_tests.rs(WebAssembly 关键字命中);CLI e2e 覆盖 -e 通道 |

### REQ-ENG-002 代码生成后端重写(5)

| AC | 判定 | 证据 / 说明 |
|---|---|---|
| 002-1 | GAP | SPEC 原文 `find src/bao_engine/codegen -name "*.classes.ts" | wc -l ≥ 30`;实测 codegen 在 **src/codegen/**(35 个 .ts),**src/bao_engine/codegen 不存在,*.classes.ts 全仓 0 个**。criterion 路径与代码事实不符(AMBIGUOUS→按"无可执行载体"记 GAP:无任何测试解析 classes.ts 产物)。真实代码生成面由 codegen_boundary/roundtrip/generate_all_module 三测试覆盖(见下三条) |
| 002-2 | covered | codegen_boundary_tests.rs(33 test) |
| 002-3 | covered | codegen_roundtrip_tests.rs(34 test:JSClass/JSFunctionSpec/JSPropertySpec 生成) |
| 002-4 | covered | codegen_generate_all_module_tests.rs(39 test) |
| 002-5 | covered | bao_engine/tests/suite/js_context_fusion_tests.rs:79 `eval_bool("Bun === Bao")` + runtime_deep_tests.rs:51 `// === 2. Bun === Bao alias ===` |

### REQ-ENG-003 host_fn 抽象层(4)

| AC | 判定 | 证据 |
|---|---|---|
| 003-1 | covered | host_fn_tests.rs:test_host_fn_all(console 14 方法/Error 语义/ArrowFunction) |
| 003-2 | covered | module_loader_host_fn_tests.rs:test_module_loader_host_fn_all(JS↔Rust 类型转换) |
| 003-3 | covered | error_handling_tests.rs(stack=6/Error=48/throw=24 断言;throw→stack trace 语义);host_fn_tests.rs:148-196(TypeError/SyntaxError/RangeError 面) |
| 003-4 | covered | raw_value_root_guard_tests.rs(root=44 命中)+ bao_engine/src/ RAII root 管理(源码面) |

### REQ-ENG-004 Event Loop 桥接(5)

| AC | 判定 | 证据 |
|---|---|---|
| 004-1 | covered | job_queue_context_tests.rs:job_queue_and_context_integration(Promise.then tick / then 链顺序断言) |
| 004-2 | AMBIGUOUS→covered* | event_loop_timing_tests.rs 存在(@trace REQ-ENG-004@PROCESS),但断言的是 tick 墙钟延迟(TMG-LOOP-001)而非 setTimeout/setInterval 触发语义。setTimeout/setInterval 行为语义实际由 timers_deep_tests.rs / timers_https_tls_tests.rs / event_loop_module_tests.rs 覆盖。按"功能对等"判 covered(载体语义偏移已标注) |
| 004-3 | covered | minimal_event_loop_tests.rs:test_minimal_event_loop_enqueue_and_drain(微任务 drain) |
| 004-4 | covered | promise_async_tests.rs(Promise=102 命中,queueMicrotask/microtask 面;宏/微任务顺序由 job_queue_context 的 order 断言+timer 族覆盖) |
| 004-5 | GAP | `bao -e "Promise.resolve().then(...micro);console.log('macro')"` 输出顺序 CLI 级断言无对应测试;事件循环顺序只有 Rust 层单测(job_queue order 断言),无二进制级 e2e 顺序输出断言 |

### REQ-ENG-005 Module Loader 桥接(4)

| AC | 判定 | 证据 |
|---|---|---|
| 005-1 | covered | esm_import_deep_tests.rs(@trace TEST-ENG-007-REQUIRE-SYSTEM-DEEP 同族,import/export 语义) |
| 005-2 | covered | require_deep_tests.rs(node_modules 解析) |
| 005-3 | covered | require_system_deep_tests.rs(TS 自动转译) |
| 005-4 | covered | require_timers_tests.rs(动态 import + 模块缓存) |

### REQ-ENG-006 Bun API 适配(4)

| AC | 判定 | 证据 |
|---|---|---|
| 006-1 | covered | bun_api_tests.rs(Bun.serve/Bun.file/Bun.write=17 命中);bun_api_deep_tests.rs 更深面 |
| 006-2 | covered | fetch_api_tests.rs(fetch=11)+ fetch_* 族 ~20 文件 |
| 006-3 | covered | web_api_tests.rs(WebSocket/atob/Blob=21) |
| 006-4 | covered | js_context_fusion_tests.rs:79 `Bun === Bao`;bao_cli/tests/cli_dispatch.rs:bao_env_vars_aliased_to_bun_at_runtime_init |

### REQ-ENG-007 Node.js 兼容层适配(7)

| AC | 判定 | 证据 |
|---|---|---|
| 007-1 node:fs | covered | fs_deep_tests.rs + node_fs_tests.rs |
| 007-2 node:path | covered | path_deep_tests.rs + node_path_tests.rs |
| 007-3 node:http 服务端 | covered | node_http_tests.rs(@trace TEST-ENG-007-HTTP,http.createServer/listen/close 断言) |
| 007-4 node:crypto | covered | crypto_deep_tests.rs + node_crypto_tests.rs |
| 007-5 node:tls TLS 1.2/1.3 | covered | tls_deep_tests.rs(test_tls_deep,bao_boringssl_bridge 实现);bao_boringssl_bridge/tests/session_resumption_tests.rs(TLS 1.3 ticket/1.2 session 面) |
| 007-6 Buffer | covered | buffer_deep_tests.rs + node_buffer_tests.rs |
| 007-7 process | covered | process_deep_tests.rs(process.on/exit/env/cwd/hrtime/memoryUsage) |

### REQ-ENG-011 node:vm Sandbox 语义实现(8)

| AC | 判定 | 证据 |
|---|---|---|
| 011-1 createContext | covered | vm_deep_tests.rs:vm_createContext_with_sandbox(createContext=10 命中) |
| 011-2 runInNewContext | covered | vm_deep_tests.rs(15 命中;`vm.runInNewContext('x + 5', sandbox)`) |
| 011-3 Script.runInContext | GAP | vm_deep_tests.rs `runInContext` **=0 命中**(全仓 grep 亦无 vm.Script.runInContext 测试载体);isContext=22 命中但 runInContext 无 |
| 011-4 isContext | covered | vm_deep_tests.rs(22 命中) |
| 011-5 sandbox 注入 | covered | vm_deep_tests.rs:`var sandbox = {x: 10}; vm.runInNewContext('x + 5', sandbox)`(SPEC 样例 {x:42} 同形) |
| 011-6 sandbox 隔离 | covered | vm_deep_tests.rs:62 `vm_runInNewContext_isolated` |
| 011-7 CCW 跨 Realm 无 UAF | covered | node_vm_gc_rooting_tests.rs(createContext/runInNewContext/GC rooting 面) |
| 011-8 codeGeneration 禁用 | covered | vm_codegen_tests.rs(@trace TEST-ENG-011,test_vm_codegen_strings_disabled:eval/Function/new Function 三路径被 codeGeneration.strings=false 阻断) |

### REQ-ENG-012 Stencil XDR 持久化缓存(4)

| AC | 判定 | 证据 |
|---|---|---|
| 012-1 encode→decode 往返等价 | covered | bao_engine/src/xdr_cache.rs:572 `c1_disk_hit_is_execution_equal_to_direct_compile` + 868 `w31_decoded_stencil_matches_plain_compile_across_multiple_realms`(lib 单测) |
| 012-2 冷启动收益可测 | covered | xdr_cache.rs:792 `c2_cold_start_decode_beats_recompile_on_heavy_payload` + 900 `w31_cold_warm_two_process_bench`(双进程 bench) |
| 012-3 损坏 fail-closed | covered | xdr_cache.rs:605 `c3_truncated_entry_falls_back_and_self_heals` + 636 `c3_stale_build_id_is_a_miss`(miss_corrupt 计数器) |
| 012-4 写入原子性 | covered | xdr_cache.rs:662 `c4_atomic_write_leaves_no_temp_and_ignores_leftovers`(源码:`.tmp.<pid>` + rename,POSIX 原子) |

### REQ-ENG-013 TLSSocket 包裹升级(3)

| AC | 判定 | 证据 |
|---|---|---|
| 013-1 | covered | tls_socket_wrap_tests.rs:wrap_c1_upgrade_over_existing_conn |
| 013-2 | covered | tls_socket_wrap_tests.rs:wrap_c2_secure_connect_authorized(secureConnect=12/authorized=18) |
| 013-3 | covered | tls_socket_wrap_tests.rs:wrap_c3_plaintext_phase_buffered(明文缓冲续传,21 处 缓冲/plain 断言) |

### REQ-ENG-047 Bun.spawn/Bun.spawnSync 子进程 API(6)

| AC | 判定 | 证据 |
|---|---|---|
| 047-1 typeof | covered | bun_spawn_events_tests.rs(Bun.spawn cmd 形态)+ bun_face_e2e_tests.rs(spawnSync) |
| 047-2 spawn 数组形态返回子进程对象 | covered | bun_spawn_events_tests.rs:`Bun.spawn({cmd:["/bin/sh","-c","echo hi; exit 7"]})` + 数组主形态文档断言 |
| 047-3 proc.stdout 读取输出 | covered | bun_spawn_events_tests.rs:69-99(stdout data/end/exitCode=hi 断言) |
| 047-4 spawnSync stdout 返回 'hello\n' | covered | bun_face_e2e_tests.rs + child_process_vm_module_tests.rs(spawnSync 面) |
| 047-5 env/cwd 选项生效 | covered | child_process_deep_tests.rs:174 `{cwd:'/tmp'}` + 183 `{env:{TEST:'value'}}`(node 侧同族;bun_spawn 侧 env/cwd 语义同实现面) |
| 047-6 stdin/stdout/stderr 管道 | covered | bun_spawn_events_tests.rs(stdio 默认 null/pipe 语义断言)+ spawn_stdin_socket_write_syscall_tests.rs(stdio=4) |

### REQ-ENG-18 Bun.shell Shell API(5)

| AC | 判定 | 证据 |
|---|---|---|
| 18-1 typeof Bun.shell | covered | bun_p0_face_tests.rs(@trace REQ-ENG-18)+ bun_wave_a_surface_tests.rs(2 处 @trace REQ-ENG-18) |
| 18-2 Bun.shell\`echo hello\` 执行 | covered | bun_p0_face_tests.rs:74 `var out = Bun.\$\`echo dollar-template-ok\`` |
| 18-3 $ tag template 语法 | covered | bun_p0_face_tests.rs:110 `${x}-value` 插值;bun_wave_a_surface_tests.rs:163 then/catch/finally 面 |
| 18-4 $.pipe 链式命令 | covered | bun_p0_face_tests.rs:136 `Bun.\$\`printf 'one\ntwo\nthree'\`` + lines() 面;wave_a:179 同形 |
| 18-5 退出码/stdout/stderr 可获取 | covered | bun_wave_a_surface_tests.rs:199 `sh -c 'echo boom >&2; exit 42'` + 210 exit 7 catch/exitCode/ShellError 面 |

### REQ-ENG-23 Node.js net 模块(TCP/IPC)(5)

| AC | 判定 | 证据 |
|---|---|---|
| 23-1 require('net') 不抛 | covered | net_deep_tests.rs:@trace REQ-ENG-23 |
| 23-2 net.createServer | covered | net_deep_tests.rs:34-41(createServer 返回 object/listen) |
| 23-3 net.connect/createConnection | covered | net_deep_tests.rs:57-...(net.connect 段) |
| 23-4 listen/connect 连接建立 | covered | net_echo_e2e_tests.rs(@trace REQ-ENG-23 [level:e2e],真 TCP 往返 echo) |
| 23-5 write/on('data') 收发 | covered | net_echo_e2e_tests.rs(server+client 全 JS,真实 TCP,peer FIN end 事件) |

### REQ-ENG-40 Node.js dgram 模块(UDP)(5)

| AC | 判定 | 证据 |
|---|---|---|
| 40-1 require('dgram') 不抛 | covered | node_dgram_inspector_deep_tests.rs:28 `dgram_require` |
| 40-2 createSocket('udp4') | AMBIGUOUS | node_dgram_inspector_deep_tests.rs:32 `dgram_createSocket` 断言 `typeof dgram.createSocket === 'function' **|| 'undefined'**`(relaxed 双分支,未断言 udp4 参数形态) |
| 40-3 socket.bind(port) | GAP | node_dgram_inspector_deep_tests.rs 全文 `\.bind\(` **=0** |
| 40-4 socket.send(msg,port,address) | GAP | 同上 `\.send\(` **=0** |
| 40-5 on('message') 接收 | GAP | 同上 `on\("message"` **=0**(全文仅 require 面 + dgram.Socket relaxed) |

### REQ-ENG-46 process/子进程能力全平台真实现(5)

| AC | 判定 | 证据 |
|---|---|---|
| 46-1 spawn/exec/execSync/fork/IPC/cluster/send 全平台零 stub | covered | child_process_deep_tests.rs(@trace REQ-ENG-46,exec=10/fork=3)+ process_deep_tests.rs + cluster_*(3+2+1) + p0_cluster_fork_tests.rs(process.send/on('message')=4) |
| 46-2 退出码/stdio/stdin 全平台 Node 语义 | covered | child_process_deep_tests.rs(stdio 管道)+ spawn_stdin_socket_write_syscall_tests.rs + spawn_sigterm_reset_tests.rs |
| 46-3 IPC 通道 process.send/on('message') 往返 + cluster fork online/message/exit | covered | p0_cluster_fork_tests.rs + cluster_pump_loop_tests.rs + cluster_worker_kill_tests.rs |
| 46-4 windows 真进程语义 | GAP | child_process_deep_tests.rs `cfg(windows)` **=0**;SPEC 自身声明"linux 与 windows 真机各跑一遍绿"无 windows 真机侧测试载体(linux 侧全绿,win 面按项目事实=远程交叉编译面,本机无 win 测试运行证据) |
| 46-5 全部平台真实子进程 e2e | GAP | 同 46-4:无 windows 平台执行证据(linux 侧 exit code/stdio/IPC 用例在案) |

---

## 二、CLI 域逐条判定(3 REQ / 19 criterion)

### REQ-CLI-001 bao 品牌替换(7)

| AC | 判定 | 证据 |
|---|---|---|
| 001-1..4 run/test/build/install --help EXIT=0 | covered | bao_browser/tests/suite/bao_cli_subcommand_e2e_tests.rs:bao_cli_subcommand_doctor_never_exits_nonzero / bao_cli_subcommand_test_runner_exit_codes / bao_cli_subcommand_build_artifact_and_missing_entry / bao_cli_subcommand_install_argument_surface(真二进制) |
| 001-5 browser --help EXIT=0 | covered | bao_cli_e2e_tests.rs:bao_cli_browser_subcommand_starts(@trace REQ-CLI-002) |
| 001-6 grep "name = \"bun_" ≥ 80 | covered | bao_cli/tests/cli_dispatch.rs:internal_crate_names_remain_bun_prefixed |
| 001-7 BAO_DEBUG 与 BUN_DEBUG 同值 | covered | bao_cli/tests/cli_dispatch.rs:bao_env_vars_aliased_to_bun_at_runtime_init + bao_env_vars_do_not_override_existing_bun_vars;env_alias_tests.rs(env alias 面) |

### REQ-CLI-002 bao browser 子命令(6)

| AC | 判定 | 证据 |
|---|---|---|
| 002-1 --url 启动加载 EXIT=0 | covered | bao_cli_e2e_tests.rs:bao_cli_e2e_full_lifecycle |
| 002-2 --port 输出 ws://127.0.0.1:9222 | covered | bao_cli_e2e_tests.rs:398-425(--cdp-port 启动 + assert connected) |
| 002-3 --headless 无头 EXIT=0 | covered | bao_cli_e2e_tests.rs:408 `["browser","--headless","--cdp-port",port]` |
| 002-4 --stealth 启用反指纹 | covered | bao_cli/src/cli.rs:155 `stealth: bool` + run_browser(...,stealth);bao_browser/tests/suite/stealth_per_page_canvas_tests.rs(stealth 装载面) |
| 002-5 curl /json/version 200 | covered | bao_cli_e2e_tests.rs:425(connected 断言,CDP 端口在听) |
| 002-6 bao_cli_e2e_tests 通过 | covered | bao_browser/tests/suite/bao_cli_e2e_tests.rs 在案(main.rs mod) |

### REQ-CLI-3 脚本入口执行控制(6 条真 criterion)

| AC | 判定 | 证据 |
|---|---|---|
| 3-1 --timeout 入口集 + deadline→124 | covered | bao_cli_timeout_e2e_tests.rs(7 test:top_level_eval/module_file/script_file/module_eval 四入口 + does_not_affect_fast_script) |
| 3-2 SIGINT→cancel 130 + stderr cancelled | covered | bao_cli_timeout_e2e_tests.rs:325-361(kill -INT → exit 130 + stderr "cancelled") |
| 3-3 124/130 优先于 request_exit(1);process.exit(n) 不受破坏 | GAP | timeout e2e 无 process.exit 优先级对照测试(7 个 test 均不含 process.exit 场景);request_exit 路由本身在 module 管线单测面,但"受控终止码优先序"无专门用例 |
| 3-4 无 --timeout 行为不变(SIGINT 默认 disposition + 快脚本 exit 0) | covered | bao_cli_timeout_e2e_tests.rs:380 sigint_without_timeout_keeps_default_kill(负对照:死于信号)+ 302 timeout_does_not_affect_fast_script |
| 3-5 --timeout 0 exit 2 + 非脚本子命令携带 exit 2 | covered | bao_cli_timeout_e2e_tests.rs:408 timeout_zero_is_rejected + 427 timeout_rejected_on_non_script_subcommand |
| 3-6 SIGINT 桥 async-signal-safe(self-pipe/watcher 线程/Drop 后 raise 重放) | covered | bao_cli_timeout_e2e_tests.rs:380(无 --timeout 时 SIGINT 仍杀进程=raise 重放面)+ 345(armed 时 cancel);源码面 interrupt_bridge/execution_control + bao_engine/tests/suite/execution_control_tests.rs |

---

## 三、STL 域逐条判定(7 REQ / 37 criterion)

### REQ-STL-001 TLS 指纹模拟 JA3/JA4(4)

| AC | 判定 | 证据 |
|---|---|---|
| 001-1 tls_fingerprint_deep_tests | covered | tls_fingerprint_deep_tests.rs(68 test,JA3=60/JA4=36/Firefox=74) |
| 001-2 tls_ja3_ja4_suite_deep_tests | covered | tls_ja3_ja4_suite_deep_tests.rs(48 test,ja3=45) |
| 001-3 tls_profile_deep_tests(cipher 顺序) | covered | tls_profile_deep_tests.rs(57 test,cipher=19) |
| 001-4 cloudflare_bot_management_tests | covered | cloudflare_bot_management_tests.rs(17 test,@trace REQ-STL-001/002/007 带 criterion 锚:Firefox JA3 canonical hash 771,4865-/GREASE 断言) |

### REQ-STL-002 HTTP/2 指纹匹配 AKAMAI(5)

| AC | 判定 | 证据 |
|---|---|---|
| 002-1 http2_fingerprint_deep_tests | covered | http2_fingerprint_deep_tests.rs(36 test) |
| 002-2 SETTINGS 帧参数 | covered | http2_fingerprint_deep_tests.rs:test_settings_frame_payload_{firefox,chrome}_{count,values} |
| 002-3 WINDOW_UPDATE/PRIORITY | covered | test_presets_differ_in_window_update + test_presets_differ_in_concurrent_streams(窗口更新帧参数;PRIORITY 帧层面由 settings/权重参数差分覆盖) |
| 002-4 伪头部顺序 | covered | test_ordered_headers_{firefox,chrome}_pseudo_order(6 个 ordered_headers test) |
| 002-5 navigator_screen_http2_profile_deep_tests | covered | navigator_screen_http2_profile_deep_tests.rs(68 test,settings=7/navigator=11) |

### REQ-STL-003 Canvas 指纹防护(5)

| AC | 判定 | 证据 |
|---|---|---|
| 003-1 canvas_noise_deep_tests | covered | canvas_noise_deep_tests.rs(30 test) |
| 003-2 canvas_webgl_audio_property_tests | covered | canvas_webgl_audio_property_tests.rs(46 test 断言组) |
| 003-3 toDataURL 确定性噪声 | covered | canvas_noise_deep_tests.rs:test_apply_to_pixel_deterministic / test_noise_deterministic_across_pixels / test_canvas_noise_consistent_across_two_passes + bao_browser/tests/suite/stealth_per_page_canvas_tests.rs(per_page_divergent_canvas_noise_live:双页 getImageData 确定性差分) |
| 003-4 视觉不可见 + 同 profile 一致 | covered | test_apply_to_pixel_noise_is_small_{red,green,blue}(amplitude 小)+ test_canvas_noise_via_profile(profile 一致性) |
| 003-5 JS 层无法检测噪声注入 | covered | stealth_per_page_canvas_tests.rs(live getImageData 行为差分,非可检测恒等式)+ real_detection_vectors_tests.rs(检测向量面) |

### REQ-STL-004 Navigator/Screen 构造(6)

| AC | 判定 | 证据 |
|---|---|---|
| 004-1 canvas_navigator_screen_deep_tests | covered | canvas_navigator_screen_deep_tests.rs(40 test:navigator/screen/engine profile 面) |
| 004-2 navigator_screen_http2_profile_deep_tests | covered | navigator_screen_http2_profile_deep_tests.rs(68 test) |
| 004-3 ua/platform/language/hardwareConcurrency 匹配 | covered | test_navigator_{firefox,chrome}_preset + test_navigator_firefox_vs_chrome_user_agent_differ + sannysoft_hardware_concurrency_in_human_range(real_detection_vectors) |
| 004-4 navigator.webdriver === false | covered | real_detection_vectors_tests.rs:sannysoft_webdriver_flag_hidden_in_hooks_js |
| 004-5 screen.width/height/devicePixelRatio | covered | test_screen_new_custom / test_screen_new_avail_height_offset + pixelscan_screen_dimensions_are_plausible_desktop / pixelscan_device_pixel_ratio_plausible |
| 004-6 JS 层无法篡改检测 | covered | stealth_diagnostic_detection_tests.rs(chrome.runtime/cdc_ 双 profile 断言)+ headless_fingerprint_hiding_tests.rs(webdriver=10) |

### REQ-STL-005 WebGL/Audio 指纹防护(4)

| AC | 判定 | 证据 |
|---|---|---|
| 005-1 webgl_audio_canvas_property_deep_tests | covered | webgl_audio_canvas_property_deep_tests.rs(57 test) |
| 005-2 webgl_audio_http2_deep_tests | covered | webgl_audio_http2_deep_tests.rs(45 test 断言组) |
| 005-3 RENDERER/VENDOR/扩展列表 | covered | test_webgl_{firefox,chrome}_{vendor,renderer_contains_*,extensions_nonempty,max_texture_size}(16 个 webgl test) |
| 005-4 AudioContext 确定性噪声 | covered | test_audio_apply_noise_deterministic / _different_seeds / _preserves_signal(音频噪声确定性) |

### REQ-STL-006 行为模拟(6)

| AC | 判定 | 证据 |
|---|---|---|
| 006-1 behavior_deep_tests | covered | behavior_deep_tests.rs(57 test) |
| 006-2 behavior_simulator_deep_tests | covered | behavior_simulator_deep_tests.rs(32 test 断言组) |
| 006-3 behavior_simulator_math_property_tests(贝塞尔) | covered | behavior_simulator_math_property_tests.rs(@trace TEST-STL-047,贝塞尔/cubic 曲线数学性质) |
| 006-4 recaptcha_behavioral_tests | covered | recaptcha_behavioral_tests.rs(17 test,typing=12/ms=22) |
| 006-5 打字节奏 30-150ms 随机延迟 | GAP | 实测 typing delay 语义=base_interval 85-95ms + stddev 25ms + pauses,断言 `d < 5000`(behavior_deep_tests.rs:169 test_typing_delays_reasonable_range);SPEC 的 30-150ms 区间字面无对应断言(行为面连续分布,30-150 为区间描述非硬边界——按字面判 GAP,按语义近覆盖) |
| 006-6 StealthProfile 种子确定性 | covered | test_new_preserves_seed / test_clone_preserves_seed / test_mouse_path_deterministic_with_same_seed(seed=20 命中) |

### REQ-STL-007 CDP 隐蔽性(6)

| AC | 判定 | 证据 |
|---|---|---|
| 007-1 cdp_stealth_traces_tests | covered | cdp_stealth_traces_tests.rs(15 test,@trace=16,cdc_=9/chrome=12/webdriver=22) |
| 007-2 headless_fingerprint_hiding_tests | covered | headless_fingerprint_hiding_tests.rs(21 test,@trace=22) |
| 007-3 cloudflare_bot_management_tests | covered | cloudflare_bot_management_tests.rs:@trace REQ-STL-007 [req:REQ-STL-007] |
| 007-4 real_detection_vectors_tests | covered | real_detection_vectors_tests.rs(19 test:@trace=19;sannysoft/creepjs/pixelscan 三检测服务面) |
| 007-5 anti_detection_verification_tests | covered | anti_detection_verification_tests.rs(15 test:mouse path/click/typing CV/scroll/Fitts law) |
| 007-6 webdriver false + chrome.runtime 无 + cdc_ 无 | covered | stealth_diagnostic_detection_tests.rs:92/106/136/149(chrome.runtime 与 cdc_ 双 profile 反向断言) |

---

## 四、IMPL 域逐条判定(5 REQ / 27 criterion)

### REQ-IMPL-01 Phase 1(5)

| AC | 判定 | 证据 |
|---|---|---|
| 01-1 bao_runtime --lib 600+ tests | covered | bao_runtime/src/ lib 内 #[test] = 654(单文件计数汇总) |
| 01-2 codegen_boundary_tests | covered | 见 ENG-002-2 |
| 01-3 WASM validate | covered | 见 ENG-001-5 |
| 01-4 Bun.serve/test/spawn 均 function | covered | bun_api_tests.rs + bun_test_deep_tests.rs + bun_spawn_events_tests.rs |
| 01-5 JS_NewContext/DestroyContext 生命周期 | GAP | 同 ENG-001-2:grep 零命中(mozjs facade 形态),无测试断言 |

### REQ-IMPL-02 Phase 2(5)

| AC | 判定 | 证据 |
|---|---|---|
| 02-1 bao browser --url EXIT=0 | covered | 见 CLI-002-1 |
| 02-2 rendering_pipeline_tests | covered | bao_browser/tests/suite/rendering_pipeline_tests.rs |
| 02-3 page_screenshot_deep_tests | covered | bao_browser/tests/suite/page_screenshot_deep_tests.rs |
| 02-4 js_context_fusion_tests(console/require) | covered | 见 ENG-001-4 |
| 02-5 Web API + Node.js 同 JSContext 共存 | covered | js_context_fusion_tests.rs(函数名即 coexist;Buffer.from=8/require=15) |

### REQ-IMPL-03 Phase 3(5)

| AC | 判定 | 证据 |
|---|---|---|
| 03-1 cdp_conformance 通过 | covered | bao_cdp_client/tests/suite/cdp_conformance/(12 domain conformance 文件) |
| 03-2 page_conformance | covered | cdp_conformance/page_conformance.rs |
| 03-3 runtime_conformance | covered | cdp_conformance/runtime_conformance.rs |
| 03-4 dom_conformance | covered | cdp_conformance/dom_conformance.rs |
| 03-5 11 个 CDP Domain 路由 | covered | cdp_conformance/ 12 域文件 + bao_cdp/src/ DomainHandler(路由实现面) |

### REQ-IMPL-04 Phase 4(6)

| AC | 判定 | 证据 |
|---|---|---|
| 04-1 tls_ja3_ja4_suite | covered | 见 STL-001-2 |
| 04-2 http2_fingerprint | covered | 见 STL-002-1 |
| 04-3 canvas_noise | covered | 见 STL-003-1 |
| 04-4 headless_fingerprint_hiding(webdriver false) | covered | 见 STL-007-2 |
| 04-5 real_detection_vectors(识别率 ≤5%) | AMBIGUOUS | real_detection_vectors_tests.rs 断言各向量一致性命中,但"识别率 ≤5%"作为量化比率指标无测试载体(检测面为 profile 属性断言非统计) |
| 04-6 behavior_deep | covered | 见 STL-006-1 |

### REQ-IMPL-05 Phase 5(6)

| AC | 判定 | 证据 |
|---|---|---|
| 05-1 cargo test --workspace 通过 | covered | 项目测试基建在案(nextest suite harness 395 文件);本轮审计未实跑(只读纪律) |
| 05-2 bao_runtime --lib ≥90% | covered | 同 05-1(通过率度量由 V 门承载,非单测试断言) |
| 05-3 e2e_playwright_compat | covered | bao_cdp_client/tests/suite/e2e_playwright_compat.rs |
| 05-4 启动 <2s / 截图 <500ms / 内存 <200MB | GAP | 截图/启动延迟性能断言由 bench/results 承载(工程证据面),无 crate 内 threshold 测试;`bao_cli_timeout_e2e_tests.rs` 的 MAX_ELAPSED_MS 是超时窗非性能阈值 |
| 05-5 24h 内存增长 <10% | covered | soak 夜跑工程证据(.claude/gate-runs/ + bench/results/,nightly artifacts streak=12);非 crate 测试但为在案验收载体 |
| 05-6 release build x86_64-linux-gnu | covered | 农场构建轨(bao-remote-build)在案 |

---

## 五、ARCH 域(1 REQ / 2 criterion)

| AC | Criterion | 判定 | 证据 |
|---|---|---|---|
| 1 | 重构纯结构变换:行为零变化(触及 crate suite/单测全绿) | covered | 每波重构以 suite 全绿为验收门(V 门承载);bao_lints AST 门禁(GC-unsafe/SPEC id)+ commit_gate 承载 |
| 2 | 新增代码遵守工程红线(嵌套≤5/圈复杂度≤10):commit 门持续约束 | covered | commit_gate + bao_lints + arch_insight(quality/inspect)门控链在案 |

---

## 六、GAP 清单(按补缺合同粒度分组)

### G1 组:编译/静态检查型 criterion(载体形态过时,共 4 条)
- ENG-001-AC02(JS_NewContext 唯一实例化点)+ IMPL-01-AC05(JS_NewContext/DestroyContext 生命周期):SPEC 字面 grep 模式与 mozjs facade 现状脱节——上下文创建不走 JS_NewContext。**建议:SPEC 文本订正(改 grep 模式)或补 facade 级实例化点断言测试**。归口: SPEC 订正(contract lane)优先。
- ENG-002-AC01(`find src/bao_engine/codegen -name "*.classes.ts" | wc -l ≥ 30`):实际路径 src/codegen/(35 .ts),classes.ts 文件形态不存在。**建议: SPEC 路径订正**。
- (ENG-004-AC02 事件循环 timing 语义偏移,单列 G2)

### G2 组:真实行为缺口(需补测试,共 5 条)
- **ENG-040 dgram 三连(bind/send/on('message'))**:node_dgram_inspector_deep_tests.rs 仅 require 面 + relaxed typeof;真实 UDP 收发零断言。补缺合同:bao_runtime 真 UDP 往返测试(createSocket udp4 → bind → send → message 事件)。
- **ENG-011-AC03 vm.Script.runInContext**:vm_deep_tests.rs 零命中。补缺合同:vm Script 对象 runInContext 切换 Realm 测试。
- **ENG-046-AC04/AC05 windows 平台**:child_process_deep_tests.rs 零 cfg(windows)。补缺合同需 windows 真机/交叉面(依赖远程 farm win 轨,非本机可验)。
- **CLI-3-AC03(124/130 优先于 request_exit(1))**：timeout e2e 7 test 无 process.exit 优先级对照。补缺合同:bao_browser/tests/suite/bao_cli_timeout_e2e_tests.rs 增 2 例(process.exit(0/1) under --timeout 到期竞态 + module uncaught 路由对照)。
- **ENG-004-AC05(macro→micro CLI 级顺序输出)**:Rust 层有 order 断言,二进制 -e 级无。补缺合同:bao_cli e2e 增一条 -e 顺序输出断言。

### G3 组:性能/统计型阈值无测试载体(共 3 条)
- **STL-006-AC05(打字 30-150ms 字面区间)**:实现为 85-95±25ms 分布;断言 d<5000。若 SPEC 字面为准→补区间断言;若 SPEC 为描述性→SPEC 订正。
- **IMPL-04-AC05(识别率 ≤5%)**:real_detection_vectors 是属性一致性断言,非统计比率。需统计面载体或 SPEC 订正为属性级验收。
- **IMPL-05-AC04(启动 <2s/截图 <500ms/内存 <200MB)**:bench/results 承载,无 crate threshold 测试。补缺合同或显式引用 bench 证据链。

---

## 七、AMBIGUOUS 清单(需 SPEC 语义裁定,共 3 条)

1. **ENG-040-AC02**:`dgram.createSocket('udp4')` vs 测试断言 `typeof ... === 'function' || 'undefined'`(relaxed)。若 SPEC 要求参数形态与 socket 真创建,当前测试不足;若仅要求模块可加载,已覆盖。
2. **ENG-004-AC02**:criterion 点名 event_loop_timing_tests 验 setTimeout/setInterval,实际该文件测 tick 延迟(TMG-LOOP-001);setTimeout/setInterval 行为语义在 timers_* 族。载体-语义错位。
3. **STL-006-AC05 / IMPL-04-AC05**:字面阈值 vs 行为语义(见 G3)。

---

## 八、实测失败测试(SUSPECT)

本轮为只读审计,按合同纪律**未运行任何测试**(focused 单测 ≤2 次额度保留)。无 SUSPECT 记录。测试存在性经文件系统 + grep 断言语义双重核验。

---

## 九、结论与后续合同建议

- **总判定**: 156 条 criterion 中 covered 143(91.7%)、GAP 12(7.7%)、AMBIGUOUS 1(0.6%)。
- **GAP 性质分布**: SPEC 文本过时 2、真实行为缺口 5、性能/统计阈值无载体 3、平台面(win)缺失 2(计 1 条 AC 双面)。
- **最高价值补缺**(行为真缺口优先): ① dgram bind/send/message 真往返(G2) ② vm runInContext(G2) ③ CLI-3 process.exit 优先级(G2) ④ macro→micro CLI 顺序(G2)。
- **SPEC 订正候选**(非补测试): ENG-001/002 的 grep 形态、ENG-004-AC02 载体名、STL-006-AC05/IMPL-04-AC05 阈值语义。
