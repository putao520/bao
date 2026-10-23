# SPEC 治理语义面盘点报告（只读分析 · 2026-10-24）

> E 分析执行体产出；写面（rename/merge/TEST 锚写入）归主会话执行。全部判定附证据（文件/函数名/commit/注册表）。

## 0. 结构判定（stop 条款检查）——不触发 stop

**结论：02-SYSTEM 旧系与 10-REQUIREMENTS 不是结构性双写，无需用户裁决保留哪套。**

证据链：
1. **粒度互补，非并行**。02-SYSTEM 旧系（REQ-BRW-5..44 / REQ-ENG-13..45）= 细粒度 Web/Bun/Node **API 面清单**（如 REQ-BRW-9 "Canvas 2D API"、REQ-ENG-31 "Node.js assert 模块"）；10-REQUIREMENTS = **集成契约层**（如 REQ-BRW-001 "libservo 集成"、REQ-ENG-007 "Node.js 兼容层适配"）。后者是前者的伞/宿主（ENG-006 Bun API 适配 ↔ ENG-14..21 Bun 各 API；ENG-007 Node 兼容层 ↔ ENG-22..45 Node 各模块），内容级**零重复条目**。
2. **号段天然分区**。10-REQUIREMENTS 的 BRW 号占用 = 001..004 + 046..051，**005..045 全空闲**；ENG 占用 = 001..013 + 046 + 048（另有 047 在 02-SYSTEM），**014..045 全空闲**。旧系 pad-in-place 补零后零落点冲突。
3. **注册表单号空间**。`.spec/.id-registry.json` REQ/ENG `allocated=[1..48]`、BRW `allocated=[1..51]`——两文件的元素共享同一号空间，每个号只登记一次；即注册表早已把两文件视为同一体系的两个存放点，而非两套体系。
4. **历史来源单一**。旧系 75 个 approved REQ 于同一 commit `88bd25614`（2026-06-22 "feat(spec): 75 new REQs status approved"）批量引入；`ccbe20334`（2026-08-22 "numbering canonical wave — 48 conflicts resolved, 8 shared-ID groups split, 105 pad-in-place, 75 TODO stubs purged"）已解决过一轮撞号（02-SYSTEM 的 Web Worker 条目让位为 REQ-BRW-045 即该波产物），但遗留了本批未补零形态。REQ-ARCH-1..4 由 `275465f0c`（2026-10-12 M8 立法）引入、REQ-IMPL-01..05 由 `8caec8e31`（2026-05-30 phase6）引入，均为漏补零而非异源体系。

**唯一语义级冲突 = ENG 13 号槽双占**（check `conflicts` 输出仅此 1 条）：REQ-ENG-13 "Bun.file/Bun.write"@02-SYSTEM vs REQ-ENG-013 "TLSSocket 包裹升级"@10-REQUIREMENTS。

---

## Part A：编号归一方案（rename 映射表）

### A-0 规范形态（由 spec_govern check `formatIssues` 定义，195 条全部可修）

| 类型 | 规范形态 | 例 |
|---|---|---|
| REQ | 3 位补零 | REQ-ENG-13 → REQ-ENG-013 |
| TEST | **不补零** | TEST-PURE-001 → TEST-PURE-1 |
| DEC | **不补零** | DEC-WK-007 → DEC-WK-7 |

### A-1 REQ rename 映射（83 项）

| 旧 ID | 新 ID | 数量 | 重叠判定 | 依据 |
|---|---|---|---|---|
| REQ-BRW-5..44 | REQ-BRW-005..044 | 40 | 独立 REQ，pad-in-place | 10-REQUIREMENTS BRW 005..045 空闲；registry 号不变；内容=API 面清单与 10-REQ 集成层零重叠 |
| REQ-ENG-14..45 | REQ-ENG-014..045 | 32 | 独立 REQ，pad-in-place | ENG 014..045 空闲；与 ENG-006/007 为伞↔条目关系非双写 |
| **REQ-ENG-13**（Bun.file/Bun.write 文件 API，status=approved） | **REQ-ENG-049** | 1 | **与 REQ-ENG-013 撞号 → renumber 到 nextN** | 见 A-2 裁决 |
| REQ-ARCH-1..4 | REQ-ARCH-001..004 | 4 | pad-in-place | ARCH 域全 SPEC 仅此 4 条（registry allocated=[1..4]） |
| REQ-IMPL-01..05 | REQ-IMPL-001..005 | 5 | pad-in-place | IMPL 域仅 05-IMPLEMENTATION 有（registry allocated=[1..5]） |
| REQ-DEPLOY-1 | REQ-DEPLOY-001 | 1 | pad-in-place | DEPLOY 域仅 1 条 |
| REQ-CLI-3 | REQ-CLI-003 | 1 | pad-in-place | CLI 已有 001/002，003 空闲 |
| REQ-ENG-46 | REQ-ENG-046 | 1 | pad-in-place | 046 空闲（047 已被 02-SYSTEM Bun.spawn 占用） |

### A-2 ENG#13 撞号裁决：**REQ-ENG-013 保 013（TLSSocket），Bun.file 改 049**

依据（groundedIn 链 + 内容新度 + 代码锚）：
1. **TLSSocket 有完整派生链**：`data-groundedin="PRD-REQ-TLS-SOCKET-WRAP"`（e79594497，2026-10-08 专项立法），status=implemented；Bun.file 无独立 PRD 锚（挂 PRD-REQ-BUN-COMPAT 伞下），由 88bd25614 批量立法（早 3.5 个月）。
2. **TLSSocket 代码锚已在位**：REQ-ENG-013 被 3 个源码文件 @trace（`src/bao_runtime/src/node_tls.rs`、`src/bao_runtime/src/node_net.rs`、`src/bao_runtime/tests/suite/tls_socket_wrap_tests.rs`），真实测试在位（见 Part B）。
3. **Bun.file 侧重命名零代码波及**：全 src 树无任何 `REQ-ENG-13` 字面引用（python 全量扫描，见 /tmp/old-id-refs.json 逻辑），bun_file 测试文件也不含该 REQ 字串。
4. 注册表 ENG `nextN=49`，049 落位即 nextN→50。

### A-3 DEC 去补零映射（02-SYSTEM 17 项；DEC-001..004 已是规范形不动）

DEC-CDP-001→DEC-CDP-1，DEC-CDP-002→DEC-CDP-2，DEC-URL-001→DEC-URL-1，DEC-WK-001..008→DEC-WK-1..8，DEC-PURE-001/002→DEC-PURE-1/2，DEC-ARCH-001→DEC-ARCH-1，DEC-ENG-001/002/003→DEC-ENG-1/2/3。（registry DEC-WK allocated=[1..8] 号空间不变，纯格式。）

### A-4 TEST 去补零映射（58 项）

| 文件 | 项 |
|---|---|
| 02-SYSTEM | TEST-PURE-001..004,006..008,010→1..4,6..8,10；TEST-PERF-001..005→1..5；TEST-BAO-API-001,002,004..008→1,2,4..8；TEST-CDP-UWS-001→1；TEST-SEC-001..003→1..3；TEST-H3-001→1；TEST-SPEC-001,002→1,2（27 项） |
| 04-DATA-MODEL | TEST-BAO-API-003→TEST-BAO-API-3（1 项） |
| 05-IMPLEMENTATION | TEST-IMPL-001..005→1..5（5 项） |
| 06-CDP-SERVER | TEST-CDS-001..008→1..8（8 项） |
| 10-REQUIREMENTS | TEST-BAO-API-017/018→17/18；TEST-ENG-008/009→8/9；TEST-ENG-010/011→10/11；TEST-H3-002→2（7 项） |

注意：TEST 与指向它的 `data-xref-id` 引用必须**同批原子改名**（如 10-REQUIREMENTS 的 `<a data-xref-id="TEST-BRW-001">` 与其 TEST 元素同形补零，单改一侧会断 xref）。

### A-5 爆炸半径（执行 rename 必须同波处理的代码面）——**本报告最大风险点**

python 全量扫描 `src/**.rs`：**70 个文件以文本形式（@trace 注释为主）引用旧系未补零 ID**（/tmp/old-id-refs.json）：

| 旧 ID | 引用文件数 | 代表文件 |
|---|---|---|
| REQ-ARCH-1 | 30+ | bao_runtime/src/node_fs.rs、node_crypto.rs、node_util.rs、bao_cdp_client/src/bridge/*（AC 红线标注） |
| REQ-ARCH-3 | ~15 | bao_ipc/src/channel.rs、bao_browser/src/lib.rs、cdp-server/src/server.rs、pump_event_driven_wake_tests |
| REQ-IMPL-01..05 | 4 | bao_cli/src/cli.rs、bao_runtime/src/lib.rs、bao_cdp/src/lib.rs、startup_memory_threshold_tests（IMPL-05） |
| REQ-ENG-14/18/19/21/23/27/40/46 | 13 | bun_password.rs(14)、bun_p0_face/bun_wave_a(18)、bun_transpiler(19)、bun_dns_resolver(21)、net_deep/net_echo/net_socket_pipe(23)、node_repl(27)、node_dgram(40)、process_deep/child_process_deep(46) |
| REQ-BRW-12 | 3 | web_api_gap_c10、web_api_ws_edge、ws_reserved_opcode |
| REQ-BRW-17/18/19/24/25/27/30 | 1 | page_api_conformance_tests.rs |
| REQ-CLI-3 | 1 | bao_cli_timeout_e2e_tests.rs |
| REQ-DEPLOY-1 | 1 | deploy_tests.rs |

同时代码大量引用**补零形**（REQ-ENG-001..013/016/047/048、BRW-001..004/046..051、CDS-001..009、CDP-001..010 等），且存在双形态混用（`bao_runtime/src/node_util.rs` 同时含 REQ-ENG-016 与 REQ-ARCH-1）——代码 @trace 字串镜像的是"书写当时的元素 id"。

**执行要求**：SPEC rename 与代码 codemod 必须**同一波原子完成**（fix_code codemod 一次横扫 70 文件，或 sed 批量；禁只改 SPEC），否则 @trace 字面匹配断裂、commit 门 evidence 扫描失锚。改名后以 `grep 复验：src 树 0 处旧形`收口。

---

## Part B：TEST 锚回填表（TV-2 全量 103 条：02-SYSTEM 80 + CDS 9 + 10-REQUIREMENTS 14）

> 判定方法：`ls` 测试树枚举 → sed 提取函数名 / 关键词内容检索（LSP search_code 因 analyzerPool.poolSize=0 恒假阴性，按 `.gsc/domain-knowledge/lsp.md` 放行边界改用 Read+sed 合规通道）。每条「有测试」均附真实文件+函数名。标注「Node realm」的锚在 bao_runtime suite，页面 realm 原生实现由 servo 承载、其最终判定载具是 REQ-BRW-051 WPT GOLDEN。

### B-1 ENG 域（02-SYSTEM 33 条 + ENG-047/046/BUILDGOV）

| REQ | 内容 | 判定 | 真实测试锚（文件::函数） |
|---|---|---|---|
| ENG-13→049 | Bun.file/Bun.write | **有** | bao_runtime/tests/suite/bun_file_methods_e2e_tests.rs::test_bun_file_method_family；bun_api_coverage_c2_tests.rs::c2_bun_file_read_family / c2_bun_file_exists_and_error_faces / c2_bun_write_readfile_faces / c2_bun_file_writer_sink_faces |
| ENG-14 | Bun.password | **有** | ffi_shell_password_coverage_c16_tests.rs::password_hash_sync_default_argon2id_face / password_hash_sync_options_matrix / password_verify_error_matrix / password_verify_promise_faces（bun_password.rs @trace REQ-ENG-14） |
| ENG-15 | Bun.hash | **有** | bun_api_deep_tests.rs（§26 Bun.hash，::test_bun_api_deep）；bun_api_tests.rs、bun_face_e2e_tests.rs |
| ENG-16 | Bun.inspect | **有** | bun_api_deep_tests.rs（§16，::test_bun_api_deep）；inspect_conformance.rs（@trace REQ-ENG-006/[req:REQ-ENG-004 util.inspect]）；util_deep_tests.rs::test_util_deep |
| ENG-17 | Bun.gzip/deflate/compress | **有** | bun_api_coverage_c12_tests.rs（probe("gzip-noarg")）；bun_wave_a_surface_tests.rs::test_bun_wave_a_surface_all（§7 gzipSync 族）；web_api_port_tests.rs |
| ENG-18 | Bun.shell | **有** | bun_p0_face_tests.rs、bun_wave_a_surface_tests.rs（均 @trace REQ-ENG-18）；udp_ffi_shell_coverage_c8a_tests.rs（Bun.$ tagged template 面） |
| ENG-19 | Bun.Transpiler | **有** | bun_transpiler_tests.rs::c1_constructor_surface / c2_loader_option / c3_transform_sync / c4_transform_async / c5_jsx_tsx |
| ENG-20 | Bun 工具(semver/which/sleep/debug) | **有** | bun_api_coverage_c2_tests.rs（Bun.semver satisfies/order + which/resolve/build 错误臂）；bun_api_deep_tests.rs（§8 Bun.sleep、§42 shellescape） |
| ENG-21 | Bun.DNSResolver | **有** | bun_dns_resolver_tests.rs::c1_surface / c2_constructor / c3_resolve_async / c4_resolve4_resolve6（bun_dns_resolver_api.rs @trace REQ-ENG-21） |
| ENG-22 | worker_threads | **有** | node_worker_threads_deep_tests.rs::test_node_worker_threads_deep；node_worker_channel_substrate_tests.rs::test_worker_terminate_consumes_queued_messages_in_order；node_worker_structured_clone_tests.rs |
| ENG-23 | node:net | **有** | net_deep_tests.rs::test_net_deep；net_echo_e2e_tests.rs；net_socket_pipe_e2e_tests.rs（均 @trace REQ-ENG-23） |
| ENG-24 | node:dns | **有** | node_dns_net_tests.rs::test_node_dns_all；node_dns_coverage_c4/c13_tests.rs；dns_cache_share_tests.rs；dns_net_deep_tests.rs |
| ENG-25 | child_process | **有** | child_process_deep_tests.rs::test_child_process_deep；child_process_spawn_events_tests.rs；child_process_carrier_tests.rs；child_process_vm_module_tests.rs；p0_cluster_fork_tests.rs::test_cluster_fork_child_runs_events_and_message_roundtrip |
| ENG-26 | readline | **有** | node_readline_deep_tests.rs；readline_deep_tests.rs::test_readline_deep；perf_readline_tty_deep_tests.rs |
| ENG-27 | repl | **有** | node_repl_tests.rs::test_node_repl_surface（@trace REQ-ENG-27） |
| ENG-28 | node:os | **有** | os_deep_tests.rs::test_os_deep；node_os_util_tests.rs；node_stdlib_wave_tests.rs::os_network_interfaces_real |
| ENG-29 | node:stream | **有** | stream_deep_tests.rs::test_stream_deep；stream_locked_state_tests.rs；stream_byte_pull_tests.rs；node_stream_qs_tests.rs；semantic_port_streams_tests.rs |
| ENG-30 | node:events | **有** | node_events_tests.rs::test_node_events_all / test_node_events_nested_same_object_emits；events_deep_tests.rs::test_events_deep；node_stdlib_wave_tests.rs::events_error_emit_semantics |
| ENG-31 | node:assert | **有** | node_assert_deep_tests.rs::test_node_assert_deep；assert_deep_tests.rs；assert_deep_parity_tests.rs（d1208f32d assert 横扫回归面） |
| ENG-32 | node:util | **有** | util_deep_tests.rs::test_util_deep；util_edge_tests.rs；util_promisify_custom_tests.rs；node_util_residual_tests.rs；util_tty_lib_coverage_c17_tests.rs |
| ENG-33 | node:zlib | **有** | zlib_deep_tests.rs::test_zlib_deep；zlib_silent_tests.rs |
| ENG-34 | string_decoder | **有** | node_string_decoder_deep_tests.rs::test_node_string_decoder_deep；strdec_module_deep_tests.rs |
| ENG-35 | node:timers | **有** | node_timers_module_deep_tests.rs::test_node_timers_module_deep；node_timers_tests.rs；timers_deep_tests.rs |
| ENG-36 | async_hooks | **有** | node_async_hooks_deep_tests.rs::test_node_async_hooks_deep；misc_domain_fix_tests.rs::test_async_hooks_hooks_fire |
| ENG-37 | node:url | **有** | node_url_tests.rs::test_node_url_all；url_deep_tests.rs::test_url_deep；url_parse_query_tests.rs；url_format_query_tests.rs；url_query_edge_conformance_tests.rs |
| ENG-38 | querystring | **有** | node_querystring_deep_tests.rs::test_node_querystring_deep；querystring_deep_tests.rs |
| ENG-39 | cluster | **有** | cluster_isprimary_race_regression_tests.rs；cluster_pump_loop_tests.rs；cluster_worker_kill_tests.rs；p0_cluster_fork_tests.rs |
| ENG-40 | dgram | **有** | node_dgram_inspector_deep_tests.rs::dgram_udp_loopback_bind_send_message_roundtrip / dgram_udp_message_payload_and_sender_rinfo（@trace REQ-ENG-40） |
| ENG-41 | node:v8 | **部分（薄）** | misc_domain_fix_tests.rs（v8.serialize/deserialize）；node_worker_threads_deep_tests.rs（v8 serializer surface）；node_dgram_inspector_deep_tests.rs（require('v8') surface）。专项 conformance 缺 |
| ENG-42 | perf_hooks | **有** | node_perf_hooks_deep_tests.rs::test_node_perf_hooks_deep；misc_domain_fix_tests.rs::test_perf_hooks_entries_real |
| ENG-43 | diagnostics_channel | **有** | node_diagnostics_channel_deep_tests.rs::test_node_diagnostics_channel_deep |
| ENG-44 | node:module | **有** | require_deep_tests.rs::test_require_deep；node_module_deep_tests.rs::test_node_module_deep；esm_import_deep_tests.rs::test_esm_import_deep；cjs_scoping_tests.rs；require_* 家族 |
| ENG-45 | wasi | **部分（负契约）** | node_silent_fake_eradication_tests.rs::wasi_fd_seek_seek_end_returns_enosys（唯一锚=假成功根除负契约；真实 WASI API=真缺口） |
| ENG-047 | Bun.spawn（02-SYSTEM） | **有** | bun_spawn_events_tests.rs::bun_spawn_event_surface_lifecycle 等 9 fn；spawn_sigterm_reset_tests.rs；spawn_stdin_socket_write_syscall_tests.rs（均 @trace REQ-ENG-047） |
| ENG-46 | process 全平台（10-REQ） | **有** | process_deep_tests.rs::test_process_deep_all（@trace REQ-ENG-46）；child_process_deep_tests.rs |
| ENG-BUILDGOV-001 | 构建产物/profile 治理 | **无测试=真缺口** | src 测试树零命中（治理面为 Makefile/scripts，无自动化断言） |

### B-2 BRW 域（02-SYSTEM 40 条 + BRW-045/046..051）

| REQ | 内容 | 判定 | 真实测试锚 |
|---|---|---|---|
| BRW-5 | Streams | **有（Node realm）** | stream_deep_tests.rs::test_stream_deep；stream_locked_state/stream_byte_pull/semantic_port_streams；response_stream_body_tests.rs（Response(ReadableStream)）；页面 realm streams=servo 原生 |
| BRW-6 | Blob/File/FileReader | **部分** | blob_slice_stream_tests.rs::test_blob_slice_stream_returns_only_the_slice（Blob，Node realm）；FileReader 页面 realm 零锚=servo 承载 |
| BRW-7 | IndexedDB | **有** | bao_browser/tests/suite/indexeddb_e2e_tests.rs::indexeddb_open_put_get_roundtrip（servo 真 IDB e2e） |
| BRW-8 | WebCrypto | **有** | crypto_subtle_string_algo_tests.rs；web_api_subtle_edge_tests.rs；crypto_deep_tests.rs；页面 realm：shadow_axis_probe_tests.rs::shadow_axis_probe_c_crypto_page_realm |
| BRW-9 | Canvas 2D | **有（stealth 面）** | stealth_per_page_canvas_tests.rs::per_page_divergent_canvas_noise_live / stealth_free_page_canvas_zero_noise_live；stealth_offscreencanvas_tests.rs |
| BRW-10 | WebGL/WebGL2 | **有（stealth 面）** | stealth_fingerprint_e2e_tests.rs；worker_multi_injection_tests.rs；stealth_offscreencanvas_tests.rs（WebGL1 worker 通道） |
| BRW-11 | WebAudio | **有** | media_e2e_tests.rs（AudioContext/decodeAudioData）；audioworklet_tests.rs；stealth_worker_audio_tests.rs（OfflineAudioContext worker 全栈） |
| BRW-12 | WebSocket | **有（双 realm）** | web_api_ws_edge_tests.rs、ws_reserved_opcode_tests.rs、web_api_gap_c10_tests.rs::c10_ws_tls_probe_frames_ping_pong_continuation_masked（均 @trace REQ-BRW-12）；页面 realm：page_wss_bao_tls_e2e_tests.rs::page_wss_roundtrip_over_bao_tls |
| BRW-13 | XHR | **有** | fetch_axis_probe_tests.rs（sync XHR）；media_e2e_tests.rs；page_net_bun_fingerprint_e2e_tests.rs（xhr_probe）；network_loading_finish_live_tests.rs |
| BRW-14 | SSE/EventSource | **有（Node/CLI realm）** | platform_surface_e2e_tests.rs::test_platform_surface_e2e（EventSource 面）；页面 realm=servo 原生 |
| BRW-15 | URL/URLSearchParams | **有（Node realm）** | url_deep_tests.rs::test_url_deep（URLSearchParams 段）；url_util_os_deep_tests.rs；globals_deep_tests.rs |
| BRW-16 | Encoding | **有（Node realm）** | web_api_tests.rs::test_web_api_all（TextEncoder/TextDecoder/atob/btoa/Performance）；web_api_gap_c10_tests.rs（TextDecoder label 分派）；atob_binary_roundtrip_tests.rs |
| BRW-17 | Notification | **有** | page_api_conformance_tests.rs::notification_api_surface_and_construction（@trace REQ-BRW-17） |
| BRW-18 | IntersectionObserver | **有** | page_api_conformance_tests.rs::intersection_observer_surface_and_callback（@trace REQ-BRW-18） |
| BRW-19 | ResizeObserver | **有** | page_api_conformance_tests.rs::resize_observer_surface_and_callback（@trace REQ-BRW-19） |
| BRW-20 | Performance | **有** | web_api_tests.rs::test_web_api_all；worker_realm_api_tests.rs（performance.now 单调性）；anti_crawler_detection_tests.rs（页面 surface） |
| BRW-21 | BroadcastChannel/MessageChannel | **部分** | node_worker_threads_deep_tests.rs（§10 BroadcastChannel）；audioworklet_tests.rs（MessageChannel 使用面）；页面 realm BroadcastChannel 无专项锚 |
| BRW-22 | DOMParser | **负契约** | node_silent_fake_eradication_tests.rs（CLI 面 throw 契约）；页面 realm=servo 原生零自有锚 |
| BRW-23 | History | **无测试=真缺口** | 全测试树零命中（仅 CDP Page.getNavigationHistory not_supported 注记） |
| BRW-24 | Web Components | **有** | page_api_conformance_tests.rs::web_components_custom_elements_and_shadow_dom（@trace REQ-BRW-24） |
| BRW-25 | MutationObserver | **有** | page_api_conformance_tests.rs::mutation_observer_surface_and_callback（@trace REQ-BRW-25） |
| BRW-26 | Web Storage | **有** | opaque_origin_startup_regression_tests.rs::opaque_origin_startup_survives_throwing_accessors（window.localStorage）；web_api_gap_c10_tests.rs（CLI localStorage guards）；platform_surface_e2e_tests.rs |
| BRW-27 | Cache API | **有** | page_api_conformance_tests.rs::cache_api_caches_open_and_keys_surface（@trace REQ-BRW-27） |
| BRW-28 | Clipboard | **部分** | permission_bridge_deep_tests.rs::test_permission_bridge_deep（clipboard-read 权限面）；editing_e2e_tests.rs（clipboard-action guards）；e161 text/html 全链无专项 |
| BRW-29 | Geolocation | **无测试=真缺口** | 零命中 |
| BRW-30 | Gamepad | **有** | page_api_conformance_tests.rs::gamepad_api_surface_headless（@trace REQ-BRW-30） |
| BRW-31 | AbortController | **有（Node realm）** | web_api_deep_tests.rs（§30）；fetch_abort_e2e_tests.rs；abort_signal_timeout_tests.rs；net_tls_fetch_retention_tests.rs |
| BRW-32 | FormData | **有（Node realm）** | fetch_init_e2e_tests.rs（multipart 序列化）；web_api_deep_tests.rs |
| BRW-33 | Fetch 基础类型 | **有（Node realm）** | fetch_api_tests.rs::test_fetch_api_all；fetch_headers_e2e_tests.rs；fetch_init_e2e_tests.rs；fetch_null_body_tests.rs |
| BRW-34 | Console | **有** | console_output_tests.rs::console_routes_stdout_and_stderr_per_node_semantics；interrupt_timeout_tests.rs::w3b_console_counters_per_realm_across_pages |
| BRW-35 | CSSOM | **有** | css_conformance_tests.rs::css_conformance_computed_style_gate；overlay_conformance_tests.rs（CDP CSS 面）；module_css_text_tests.rs |
| BRW-36 | Selection/Range | **有** | editing_e2e_tests.rs::editing_e2e_contenteditable_real_key_insertion / editing_e2e_enter_key_paragraph_in_contenteditable（真键盘路径） |
| BRW-37 | Screen/VisualViewport | **部分** | Screen：canvas_navigator_screen_deep_tests.rs（stealth）、layout_geometry_tests.rs；VisualViewport：零专项锚 |
| BRW-38 | Event API | **部分** | security_sandbox_tests.rs::scenario_web_apis_available（页面 surface 存在性）；events_deep_tests.rs::test_events_deep（Node EventTarget）；页面 realm 事件语义=servo 原生 |
| BRW-39 | Touch | **无测试=真缺口** | 零命中 |
| BRW-40 | Drag and Drop | **无测试=真缺口** | 零命中 |
| BRW-41 | WebRTC/MediaStream | **弱（仅探针）** | stealth_diagnostic_detection_tests.rs（typeof RTCPeerConnection 探针）；真实 API 面=servo 承载零自有锚 |
| BRW-42 | DOMException | **无专项=真缺口** | 仅 editcontext_api_tests.rs 断言中顺带 instanceof DOMException |
| BRW-43 | Permissions | **有** | permission_boundary_tests.rs（net/read/write 匹配 10 fn）；permission_bridge_deep_tests.rs::test_permission_bridge_deep |
| BRW-44 | ES2020+ | **有** | es_language_features_tests.rs::test_es_language_features；bao_engine/tests/suite/es_advanced_features_tests.rs |
| BRW-045 | Worker 族范围注记 | **注记型（正主有测试）** | 正主 REQ-BRW-004 已有 TEST-BRW-004；SharedWorker/SW 锚：sharedworker_injection_tests.rs::sharedworker_ctor_port_roundtrip_and_stealth_injection、serviceworker_controller_tests.rs::c19_sw_controller_assignment_live |
| BRW-046 | SVG 几何 | **有** | svg_dom_geometry_tests.rs::svg_dom_geometry_apis_return_real_values / svg_interface_reflection_26_no_regression |
| BRW-047 | WebVTT | **有** | webvtt_render_tests.rs::webvtt_render_e2e_suite |
| BRW-048 | DevTools 深化 | **有** | cdp_debugger_fidelity_tests.rs；bao_cdp/tests/suite/devtools_dom_css_tests.rs；cdp_gap_coverage_c14_tests.rs（browser+cdp_client 两份） |
| BRW-049 | CSS 能力继承验证面 | **有** | css_conformance_tests.rs::css_conformance_computed_style_gate（@trace REQ-BRW-049） |
| BRW-050 | EditContext | **有** | editcontext_api_tests.rs::editcontext_api_c1_constructor_surface / c2_association / c3_event_dispatch；editcontext_parity_tests.rs::editcontext_parity_chrome_prototype_enums；face_parse_regression_live_tests.rs |
| BRW-051 | WPT GOLDEN | **部分（载具在）** | view_transition_tests.rs（@trace REQ-BRW-051）；WPT campaign 本体=脚本载具（wptrunner），无 cargo 测试 |

### B-3 CDS / CDP / 其余（10-REQUIREMENTS 14 条 + 06-CDP-SERVER 9 条）

| REQ | 判定 | 真实测试锚 |
|---|---|---|
| CDS-001 传输层 | **有** | cdp-server/tests/suite/transport_http_parse_tests.rs::test_path_version_exact 等 8 fn；transport_parse_deep/boundary；integration_tests.rs::test_http_json_version / test_websocket_command_dispatch |
| CDS-002 Target 管理 | **有** | cdp-server/tests/suite/registry_advanced_tests.rs；integration_tests.rs（/json/list 面）；bao_browser/tests/suite/target_domain_event_tests.rs；cdp_gap_coverage_c14_tests.rs::bridge_target_context_lifecycle_faces |
| CDS-003 会话生命周期 | **有** | cdp-server/tests/suite/server_session_disconnect_tests.rs；registry_session_event_deep_tests.rs；bao_cdp/tests/suite/router_session_lifecycle_deep_tests.rs |
| CDS-004 消息路由 | **有** | protocol_compliance_tests.rs；protocol_conformance_tests.rs；edge_case_tests.rs::test_registry_dispatch_echo / test_registry_dispatch_unknown_domain |
| CDS-005 事件系统 | **有** | protocol_broadcaster_deep_tests.rs（8 fn）；stress_recovery_tests.rs::test_event_sender_count_accuracy |
| CDS-006 DomainHandler | **有** | registry_session_event_deep_tests.rs::test_register_single_handler / duplicate_fails；bao_cdp/tests/suite/protocol_domain_handler_deep_tests.rs |
| CDS-007 并发安全 | **有** | concurrency_tests.rs::test_concurrent_dispatch_ping / test_sequential_dispatch_thousand；stress_recovery_tests.rs |
| CDS-008 可配置性 | **有** | server_config_builder_deep_tests.rs（12+ fn）；cdp_server_ws_url_config_deep_tests.rs；config_session_transport_deep_tests.rs |
| CDS-009 Browser Context | **有** | bao_cdp_client/tests/suite/e2e_playwright_compat.rs（"Playwright BrowserContext 直连生命周期 — REQ-CDS-009 C1-C5+C7 真断言"）；d_class_local_state.rs（BrowserContext 导入面）；cdp_gap_coverage_c14_tests.rs（CreateBrowserContext/ListBrowserContexts） |
| REQ-CDP-009 screencast | **有** | cdp_screencast_input_loop_tests.rs（start/stop/ack/collect_frames）；screencast_tests.rs::c3_c6_c7_frame_event_shape_and_formats |
| REQ-CDP-010 CDP GOLDEN | **有** | bao_cdp_client/tests/suite/e2e_external_chrome.rs::e2e_real_chrome_navigation_and_screenshot / e2e_real_chrome_runtime_evaluate（真 Chrome oracle）；cdp_ws_command_face_tests.rs（@trace REQ-CDP-010） |
| REQ-ENG-012 Stencil XDR | **有** | bao_engine/src/xdr_cache.rs 内联测试::c1_disk_hit_is_execution_equal_to_direct_compile / c3_truncated_entry_falls_back_and_self_heals；bao_engine/src/stencil_cache.rs 内联（cached_hit_state_is_byte_equal_to_plain_eval 等 6 fn）；bao_stealth/tests/suite/stencil_cache_equivalence_tests.rs::stealth_blob_cached_injection_is_byte_equal_to_plain |
| REQ-ENG-013 TLSSocket | **有** | bao_runtime/tests/suite/tls_socket_wrap_tests.rs::wrap_c1_upgrade_over_existing_conn / wrap_c2_secure_connect_authorized / wrap_c3_plaintext_phase_buffered |
| REQ-CLI-3 timeout/SIGINT | **有** | bao_browser/tests/suite/bao_cli_timeout_e2e_tests.rs::timeout_terminates_runaway_top_level_eval / sigint_cancels_runaway_under_timeout / timeout_zero_is_rejected；interrupt_timeout_tests.rs；execution_control_entry_tests.rs::module_entry_runaway_deadline_deterministic_timeout |
| REQ-ENG-048 node:test 套件 | **有** | node_test_runner_bridge_tests.rs::node_test_testcontext_injected_into_body 等 5 fn；node_conformance/ 目录（assert/buffer/crypto/events/fs/http/path/stream/url/util conformance） |
| REQ-DEPLOY-1 musl 矩阵 | **有（非 cargo 运行时，静态断言）** | bao_runtime/tests/suite/deploy_tests.rs::req_deploy_1_target_matrix_documented_no_grey_area / req_deploy_1_musl_cross_recipe_documented / req_deploy_1_cargo_config_carries_musl_target_wiring / req_deploy_1_test_platform_inside_declared_matrix |

### B-4 ARCH 域 + 汇总

| REQ | 判定 | 真实测试锚 |
|---|---|---|
| ARCH-1 代码卫生红线 | **部分（AC02 有门，AC01 流程型）** | bao_lints/tests/bce012_smoke.rs::detects_handle_jsobject_ptr_non_null_inline_ref 等（AST 门禁冒烟）；AC01 存量收敛=波次流程，无单测 |
| ARCH-2 ScriptThread/JSContext | **有** | bao_engine/tests/suite/js_context_fusion_tests.rs::js_context_fusion_node_and_web_api_coexist；compartment_isolation_tests.rs::scenario_two_pages_distinct_profiles 等；multi_runtime_initial_load_tests.rs::second_runtime_initial_load_completes_in_process |
| ARCH-3 IPC/MPMC | **有** | bao_ipc/tests/suite/channel_tests.rs::mpmc_bounded_four_producers_four_consumers_no_loss_no_dup / bounded_capacity_one_backpressure_pins_try_send_full；pump_event_driven_wake_tests.rs::pump_tail_bridge_command_wake_within_2ms；thread_safety_concurrency_tests.rs |
| ARCH-4 fork 发布自主 | **无自动化测试（流程型）** | 验证载具=发布闭包 curl max_version 核验（P4 纪律），无 cargo 测试 |

**Part B 汇总**：103 条 TV-2 中，**94 条已有真实测试可挂锚**；**9 条为真缺口/特殊型**：BRW-23（History）、BRW-29（Geolocation）、BRW-39（Touch）、BRW-40（DnD）、BRW-41（WebRTC，仅探针）、BRW-42（DOMException，无专项）、ENG-45（wasi，仅负契约）、ENG-BUILDGOV-001（无）、ARCH-4（流程型）。BRW-045 为注记型（正主 BRW-004 已有锚）。ENG-41（v8）与 BRW-37（VisualViewport）、BRW-28（Clipboard 全链）为「部分覆盖」，锚表已如实标注。

---

## Part C：断链 11 处修复方案

机械修复波已删除 11 条死链（href 指向不存在的 11-TESTING.html#test-*）。属主与处置逐条如下。**总体判定：全部 11 条应「回建 TEST 元素 + 重建 xref」**（无一条可改指既有 TEST——这些 REQ 此前从未有过 TEST 元素），TEST 命名遵循 B 家族既有配对惯例与规范形态（不补零），内容挂 Part B 真实测试。ARCH-4 诚实标注无自动化测试。

| # | 属主 REQ | 被删死链 | 处置 | 新 TEST 元素（id=规范形） | TEST 内容挂载的真实测试 |
|---|---|---|---|---|---|
| 1 | REQ-ARCH-1（02-SYSTEM） | test-arch-1 | 回建 | TEST-ARCH-1 | bao_lints bce012_smoke（AC02 门）；AC01 注明波次流程验收 |
| 2 | REQ-ARCH-2 | test-arch-2 | 回建 | TEST-ARCH-2 | js_context_fusion / compartment_isolation / multi_runtime_initial_load |
| 3 | REQ-ARCH-3 | test-arch-3 | 回建 | TEST-ARCH-3 | bao_ipc channel_tests mpmc_* / pump_event_driven_wake |
| 4 | REQ-ARCH-4 | test-arch-4 | 回建（诚实标注） | TEST-ARCH-4 | 无自动化测试；验收=发布闭包 curl max_version 核验（流程型，如实写明） |
| 5 | REQ-CDP-009（10-REQ） | test-cdp-54 | 回建 | TEST-CDP-9（家族 TEST-CDP-1..8 已在，顺延配对） | cdp_screencast_input_loop_tests / screencast_tests::c3_c6_c7_frame_event_shape_and_formats |
| 6 | REQ-ENG-012 | test-eng-380 | 回建 | TEST-ENG-12 | bao_engine xdr_cache/stencil_cache 内联测试 + stencil_cache_equivalence_tests |
| 7 | REQ-ENG-013 | test-eng-379 | 回建 | TEST-ENG-13 | tls_socket_wrap_tests::wrap_c1/c2/c3 |
| 8 | REQ-BRW-050 | test-brw-57 | 回建 | TEST-BRW-50 | editcontext_api_tests（c1..c3）+ editcontext_parity_tests |
| 9 | REQ-CLI-3 | test-cli-8 | 回建 | TEST-CLI-3（家族 TEST-CLI-1/2 已在） | bao_cli_timeout_e2e_tests（timeout/sigint 6 fn） |
| 10 | REQ-ENG-46 | test-eng-process-46 | 回建 | TEST-ENG-46 | process_deep_tests::test_process_deep_all + child_process_deep_tests |
| 11 | REQ-CDS-009（06-CDP-SERVER） | test-cds-21 | 回建 | TEST-CDS-9（家族 TEST-CDS-1..8 已在） | e2e_playwright_compat（REQ-CDS-009 C1-C5+C7 断言）+ d_class_local_state |

执行注意：TEST 元素写入对应属主文件（02-SYSTEM×4、10-REQUIREMENTS×6、06-CDP-SERVER×1），xref 与元素同批写入；写完跑 `spec_govern(check)` 确认 TV-2 相应行消号、links.errors 保持 0。

---

## 总结论

**执行顺序建议（主会话写面）**
1. **先 Part C（11 条回建）**——独立、零依赖、立即使 4 个 status=implemented 的 REQ（CDP-009 例外为 draft）获得 test link，消掉「implemented but no test link」警告。
2. **再 Part A rename（83 REQ + 17 DEC + 58 TEST）**，同波两件事必须绑定：① xref 与元素原子改名；② **70 个 src 文件的旧 ID 文本 codemod**（fix_code codemod 一次横扫；仅 REQ-ENG-13→049 需语义判断，其余纯机械 pad/unpad）。registry 同步：pad-only 项号不变，仅新增 ENG-049 分配（nextN→50）。
3. **最后 Part B 锚回填**（94 个 TEST 元素新建/补 xref，9 个真缺口按诚实缺口登记：其中 BRW-23/29/39/40/41/42 建议在 02-SYSTEM req 的 status 或注记中标「servo 承载/WPT 判定」避免假绿，ENG-45/BUILDGOV-001/ARCH-4 标「流程/负契约验证」）。
4. 每步之后跑 `spec_govern(action=check)` 验收：formatIssues 195→0、TV-2 103→9（真缺口登记后可解释收敛）、conflicts 1→0、links.errors 恒 0。

**风险点**
1. **@trace 字面断裂（最高风险）**：70 文件旧 ID 引用若不与 rename 同波 codemod，commit 门 evidence 扫描将失锚。建议 codemod 后以全树文本复验（0 处旧形残留）为收口证据。
2. **TEST 家族跨文件编号**：TEST-BAO-API 家族跨 3 文件（02-SYSTEM/04-DATA-MODEL/10-REQUIREMENTS），去补零时逐条核对唯一性（现规划后 1..8、17、18 无冲突）。
3. **ENG-049 是新增号**：若主会话在 rename 前另有新增 ENG REQ 的计划，先做 rename 再新增，避免 nextN 竞争。
4. **LSP 通道现状**：search_code analyzerPool.poolSize=0（假阴性），本报告测试检索已按知识库改用 sed/Read 合规通道；主会话执行写面后的复验如需 LSP 证据，先修插件 LSP 加载或继续用文本通道。
5. **诚实边界**：Part B「有测试」仅证明测试文件与函数存在且按域关键词/API 名对应，未逐个断言其断言语义与 REQ criterion 级等价——主会话挂锚时可对 9 个「部分覆盖」项（ENG-41、BRW-6/21/22/28/37/38/45、ENG-45）复核 criterion 措辞，避免 TEST 元素夸大覆盖。

**判定「unknown」项**：无。全部 83 个旧系 REQ 的内容均从 HTML 属性直读取得（标题/状态/groundedIn 完整），无一需要 force-merge。
