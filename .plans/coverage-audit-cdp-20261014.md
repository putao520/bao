# CDP 域覆盖对等核验报告(REQ-CDP-001~009 · 47 criterion)

- 日期:2026-10-14 · 合同:cov-audit-cdp(只读核验,唯一写面=本报告)
- 判定口径:「已有功能对等测试」= 存在触达该 criterion 语义的测试实体(含 in-src #[cfg(test)]);GAP=无测试;功能本体不存在=功能级真缺口(单列)
- 实测:两次 focused 运行全绿,零 SUSPECT——`cargo nt -p cdp-server -E 'test(integration)'` 9/9 PASS;`cargo nt -p bao_cdp -E 'test(protocol_all_domains)+test(protocol_subcommand)+test(devtools_dom_css)+test(network_schema)'` 302/302 PASS

## 总量

| 判定 | 数 |
|---|---|
| COVERED | 42 |
| GAP(实现存在、测试断链) | 2(001-C5 部分缺、008-C4 全缺) |
| GAP(功能级真缺口,功能本体不存在/no-op) | 2(006-C6 拦截、007-C6 overlay) |
| AMBIGUOUS(语义有测试但仅 env-gated 外网路径) | 1(004-C7) |

## 逐条判定

### REQ-CDP-001 CDP WebSocket Server(5)

| # | criterion | 判定 | covered-by |
|---|---|---|---|
| C1 | 监听指定端口接受 WS 连接 | COVERED | `src/cdp-server/tests/suite/integration_tests.rs::test_websocket_command_dispatch`(真 TcpListener+真 WS 客户端,本轮实测 PASS);关停有界=`server_stop_tests::server_run_returns_ok_after_stop_within_bound` |
| C2 | JSON-RPC 2.0 编解码正确 | COVERED | `cdp-server protocol_conformance_tests/protocol_compliance_tests::test_parse_*/test_serialize_*/test_full_roundtrip_*`;`bao_cdp protocol_edge_case_tests`(id 类型严格性/重复键/边界);`protocol_codec_single_source_tests`(单一编解码源) |
| C3 | Playwright 成功连接并列出 targets | COVERED | 真客户端 oracle:`src/bao_browser/tests/suite/puppeteer_e2e_tests.rs::puppeteer_real_lifecycle_e2e`(真 Node puppeteer-core:honest-skip 仅缺 node;discover→goto→screenshot PNG magic→network event→disconnect)+ `playwright_full_flow_loop_tests::playwright_style_full_flow_three_rounds`(hermetic 真 WS 面);协议面:`bao_cdp_client e2e_playwright_compat.rs`(http discovery json_version/json_list/json_new/browser-attach/flat-session)+ `cdp-server integration_tests::test_http_json_list`。备注:`e2e_real_playwright_full_flow`/`e2e_real_playwright_browser_context_isolation` 为空占位(跳过即返)——不构成缺口,真客户端面已由上两者承担;占位可作清理候选 |
| C4 | 多并发 session | COVERED | `bao_cdp router_external_detach_edge_tests::test_router_100_sessions`;`domain_stress_tests::test_many_sessions_create_detach/test_session_isolation_under_load/test_multiple_sessions_same_commands`;`cdp-server registry_advanced_tests::test_concurrent_dispatch*` |
| C5 | 连接断开后资源正确释放 | **GAP(部分)** | 已覆盖:关停 drain=`server_stop_tests`;destroy 回调=`protocol_robustness_tests::test_c4_notify_session_destroyed_*`;ws 面 detach=`ws_registry.rs::detach_removes_routing_entry`。**缺**:单连接断开→`cdp-server/src/server.rs:236 process_sessions_once` 的 `to_remove→begin_close→finalize→notify_session_destroyed` 路径零测试触达(`session.begin_close/finalize` 无任何测试调用者) |

### REQ-CDP-002 Runtime Domain(6)

| # | criterion | 判定 | covered-by |
|---|---|---|---|
| C1 | Runtime.evaluate | COVERED | `cdp_conformance/runtime_conformance.rs::runtime_evaluate_result_schema_conformance`(+exceptionDetails/optional params)+ 真 WS 面 `cdp_ws_command_face_tests::ws_command_face_page_navigate_and_evaluate_roundtrip`、`ws_cdp_evaluate_stays_in_page_realm`(page realm 隔离) |
| C2 | Runtime.callFunctionOn | COVERED | `runtime_call_function_on_result_schema_conformance`(+missing objectId 32602)+ 真 WS 面 `cdp_ws_command_face_tests::ws_runtime_object_protocol_roundtrip`(object protocol 真往返;真实现 `src/bao_browser/src/cdp_handler.rs:2195` CallArgument 校验+async poll) |
| C3 | Runtime.getProperties | COVERED | `runtime_get_properties_property_descriptor_schema`(+missing objectId)+ `router_backend_deep_tests::test_internal_dispatch_runtime_get_properties` + `devtools_dom_css_tests::node_for_owner_maps_object_id_to_canonical_node_id`(objectId 解析) |
| C4 | Runtime.enable→执行上下文通知 | COVERED | `src/bao_browser/src/ws_registry.rs::runtime_enable_emits_execution_context_created_on_session`(in-src 测试,断言 Runtime.executionContextCreated 会话下发)+ `emit_runtime_enabled_context`(Chrome auxData.frameId 形)+ retirement 存活断言(ws_registry.rs:1289) |
| C5 | Runtime.exceptionThrown | COVERED | `event_translation.rs::e2e_page_error_to_runtime_exception_thrown` + `runtime_conformance::runtime_evaluate_exception_details_schema_when_present` + `cdp-server final_remaining_cdp_tests::bao_event_parse_runtime_exception` |
| C6 | console.log→Runtime.consoleAPICalled | COVERED | `event_coverage_full.rs::full_e2e_console_chain_translate_to_cdp_event`(+params 必备字段)+ `e2e_internal_event_listener_console_full_chain` + `cdp-server server.rs broadcast_console_log`(consoleAPICalled+Log.entryAdded 双发)+ `log_conformance.rs` 6 tests |

### REQ-CDP-003 Debugger Domain(6)

| # | criterion | 判定 | covered-by |
|---|---|---|---|
| C1 | Debugger.enable | COVERED | `debugger_conformance.rs::debugger_enable_returns_debugger_id` + `debugger_sm_api_tests::sm_api_enable_records_wants_live_notifications` |
| C2 | Debugger.setBreakpointByUrl | COVERED | `debugger_set_breakpoint_by_url_returns_breakpoint_id` + 真帧 `cdp_debugger_fidelity_tests::debugger_breakpoint_real_frames_and_locations_e2e`(真断点命中帧+位置) |
| C3 | Debugger.pause/resume | COVERED | `debugger_pause_returns_empty_object`/`debugger_resume_returns_empty_object` + `sm_api_pause_routes_to_interrupt`/`sm_api_resume_routes_to_resume_no_limit` |
| C4 | stepOver/stepInto/stepOut | COVERED | `debugger_conformance.rs` step_over/into/out 三 route tests + `sm_api_step_*` 三件 |
| C5 | Debugger.paused 通知 | COVERED | `cdp_debugger_fidelity_tests::debugger_breakpoint_real_frames_and_locations_e2e`(真 paused 事件流 take_event)+ 阴性对照 `assert_no_paused_for` + `cdp-server bao_event_parse_debugger_paused` |
| C6 | Debugger.scriptParsed 通知 | COVERED | `debugger_script_parsed_event_schema_conformance`(+sourceMapURL optional+sessionId)+ `event_coverage_full::full_e2e_script_parsed_params_correct` |

### REQ-CDP-004 Page Domain(7)

| # | criterion | 判定 | covered-by |
|---|---|---|---|
| C1 | Page.navigate | COVERED | `page_conformance.rs::page_navigate_result_schema_conformance`(+32602/32000)+ 真 WS 面 `ws_command_face_page_navigate_and_evaluate_roundtrip` + puppeteer goto |
| C2 | Page.captureScreenshot base64 | COVERED | `page_capture_screenshot_result_schema_conformance`(png/jpeg/webp format 矩阵)+ 真实 PNG=puppeteer 测试 magic-byte 断言 + `page_screenshot_deep_tests` |
| C3 | Page.reload | COVERED | `page_reload_returns_empty_object_conformance`(+ignoreCache 分支 `protocol_subcommand::test_page_reload_*`)+ bridge reload |
| C4 | Page.setContent | COVERED | `page_set_content_with_html_returns_empty`(+缺 html 32602=`domain_handler_response_field_boundary::test_page_set_content_missing_html_rejected`)+ `a_page_set_content` |
| C5 | Page.getFrameTree | COVERED | `page_get_frame_tree_schema_conformance` + `bridge_channel_*::test_page_get_frame_tree*` |
| C6 | Page.loadEventFired | COVERED | `event_translation.rs:465/606`(servo FrameInfo→loadEventFired,W40 Chrome 双发语义配对)+ `protocol_serialize_boundary`/`protocol_message_deep` 序列化面 |
| C7 | addScriptToEvaluateOnNewDocument 规范时点 | **AMBIGUOUS** | 注册/identifier/remove 面 COVERED:`domain_handler_response_field_boundary::test_page_add_script_requires_bridge_for_identifier`、`protocol_subcommand` remove 32602 矩阵、bridge add_script。**语义缺 hermetic 测试**:「每个新 document 后、页面脚本前求值」唯一语义验证=`fingerprint_website_eval_e2e_tests.rs:572`(需 BAO_TEST_NETWORK=1+DISPLAY,真实外网站点)与 WPT 载具;vendor registry 单测在 script_thread.rs(`embedder_new_document_script_registry_tests`)。缺:仓内 hermetic「注册 marker→导航→断言先于页面脚本」测试 |

### REQ-CDP-005 DOM Domain(6) — 全 COVERED

`cdp_conformance/dom_conformance.rs` 26 tests(method schema+缺参 32602 全矩阵)+ `devtools_dom_css_tests`(真后端语义)+ `e2e_internal_servo` DOM 链。

| # | criterion | covered-by(语义面) |
|---|---|---|
| C1 | DOM.getDocument | `dom_get_document_result_schema_conformance` + `e2e_internal_dom_get_document_returns_root` + `devtools_dom_css::flattened_document_depth_semantics_and_parent_links` |
| C2 | DOM.querySelector | `dom_query_selector_result_schema_conformance` + `devtools_dom_css::query_selector_returns_canonical_node_ids`(canonical nodeId) |
| C3 | DOM.querySelectorAll | `dom_query_selector_all_result_schema_conformance` + devtools flat/depth 语义 |
| C4 | DOM.getBoxModel | `dom_get_box_model_returns_box_data_conformance`(+model wrapper schema)+ `devtools_dom_css::get_box_model_reports_page_rect`(真页面 rect) |
| C5 | DOM.setAttributeValue | `dom_set_attribute_value_returns_empty_object`(+32602)+ bridge `test_bridge_cmd_set_attribute_debug` |
| C6 | DOM.describeNode | `dom_describe_node_result_schema_conformance` + `devtools_dom_css::describe_node_reports_live_node_data` + `resolve_node_roundtrips_to_canonical_node_id` |

### REQ-CDP-006 Network Domain(6)

| # | criterion | 判定 | covered-by |
|---|---|---|---|
| C1 | Network.enable | COVERED | `network_conformance.rs::network_enable_returns_empty_object`(+optional params)+ `network_schema_conformance_tests` 32 tests |
| C2 | Network.requestWillBeSent | COVERED | `event_translation.rs::e2e_network_request_to_request_will_be_sent` + `event_coverage_full::full_e2e_network_request_params_contain_required_fields` + 真面=puppeteer 测试 network-event 断言 |
| C3 | Network.responseReceived | COVERED | `event_translation.rs::e2e_network_response_to_response_received` + params 断言 |
| C4 | Network.loadingFinished | COVERED | `event_translation.rs::e2e_network_loading_finish_to_loading_finished`(+loadingFailed) |
| C5 | Network.getResponseBody | COVERED | `network_get_response_body_result_schema_conformance`(+32602)+ `network_schema_conformance::network_get_response_body_bridge_passthrough_oracle_shape` |
| C6 | Network.requestIntercepted 支持请求拦截 | **GAP(功能级真缺口)** | 功能本体不存在:bao 无拦截设施——`Fetch.*` 显式 -32000 "no request interception facility"(`bao_cdp/src/protocol.rs:1954`;测试断言显式失败=`bridge_channel_timeout_edge_deep_tests::test_fetch_continue_request`;WS 面=`ws_registry.rs::fetch_domain_is_explicit_error`);`Network.setRequestInterception/continueInterceptedRequest`=accepted no-op(`protocol.rs:1333/2471`,测试仅参数回环)。现有测试全部是 fail-closed 形态,**零拦截功能测试,零拦截功能实现** |

### REQ-CDP-007 CSS/Input/Emulation/Overlay(6)

| # | criterion | 判定 | covered-by |
|---|---|---|---|
| C1 | CSS.getMatchedStylesForNode | COVERED | `css_conformance.rs::css_get_matched_styles_for_node_*`(schema/inline optional/32602)+ 真后端 `devtools_dom_css::matched_styles_report_author_rules` |
| C2 | CSS.getComputedStyleForNode | COVERED | `css_get_computed_style_for_node_*` + 真页面声明对照 `devtools_dom_css::computed_style_matches_page_declarations` + `set_style_texts_writes_and_reads_back` |
| C3 | Input.dispatchMouseEvent | COVERED | `input_conformance.rs::input_dispatch_mouse_event_*`(type 全枚举/32602)+ 真效果 `e2e_internal_input_tap_uses_iife_safe_selector`/`hover` |
| C4 | Input.dispatchKeyEvent | COVERED | `input_dispatch_key_event_*` + 真键入效果 `cdp_screencast_input_loop_tests::cdp_screencast_input_loop_typing_updates_input_value` |
| C5 | Emulation.setDeviceMetricsOverride | COVERED | `emulation_conformance.rs` 8 tests(+clear 幂等)+ `perf_refactor_integration::int_perf002_emulation_set_device_metrics_round_trip/clear_then_set_is_idempotent` + bridge set_viewport 全族。备注:真实布局生效面归 BRW viewport 语义;CDP 面无 real-effect 断言(轻微,不计 GAP) |
| C6 | Overlay.highlightNode 高亮节点(截图标注) | **GAP(功能级真缺口)** | 两面皆 no-op:`bao_cdp/src/protocol.rs:1698` `"highlightNode"|"hideHighlight"|"setInspectMode" => ok_empty()`;WS 面 Overlay 仅 enable/disable accepted-inert(`ws_registry.rs:219`);bao_browser 无任何高亮渲染/截图标注实现;测试仅断言 ok-empty 响应形,零高亮效果断言 |

### REQ-CDP-008 Target Domain(4)

| # | criterion | 判定 | covered-by |
|---|---|---|---|
| C1 | Target.getTargets | COVERED | `target_conformance.rs::target_get_targets_result_schema_conformance` + `cdp-server server_transport_protocol_deep::test_target_provider_list` + `integration_tests::test_http_json_list` |
| C2 | Target.attachToTarget | COVERED | 真面 `ws_registry.rs::attach_to_target_mints_real_unique_sessions` + `flattened_session_routes_to_attached_target` + `broadcast_for_target_delivers_only_to_sessions_attached_to_the_target` + conformance attach 矩阵(flatten 门/32602) |
| C3 | Target.setAutoAttach | COVERED | `ws_registry.rs::set_auto_attach_emits_attached_to_target_for_existing_pages` + 客户端 `e2e_playwright_compat::e2e_playwright_set_auto_attach_receives_attached_events`。子项缺:后续 `Target.createTarget` 的 attachedToTarget(`ws_registry.rs:605 emit_target_created_attached` 有实现零测试)——并入 GAP 清单 P3 |
| C4 | targetCreated/destroyedTarget 通知 | **GAP(实现存在、测试断链)** | `ws_registry.rs:478 emit_target_closed_events`(Target.targetDestroyed+Target.detachedFromTarget,Chrome 关闭语义/puppeteer W40 S11 依赖)与 `:605 emit_target_created_attached` 全仓零测试触达——tests 目录 grep `targetCreated|targetDestroyed` 零命中,ws_registry in-src 11 测试无一断言。无回归防护 |

### REQ-CDP-009 screencast(7) — 全 COVERED

载体:`src/bao_browser/tests/suite/screencast_tests.rs`(测试名即 C1-C7 直接锚定)+ `cdp_screencast_input_loop_tests.rs`(输入刺激负载面)。

| # | criterion | covered-by |
|---|---|---|
| C1 | change-driven 帧流 | `c1_change_driven_frames_only_on_content_change` |
| C2 | maxFrameRate 上限生效 | `c2_max_frame_rate_caps_emission_frequency` |
| C3 | screencastFrame 事件(base64+metadata) | `c3_c6_c7_frame_event_shape_and_formats` |
| C4 | ack 参与节流(未 ack 不无限出帧) | `c4_ack_gates_emission` + 负载面 `cdp_screencast_input_loop_ack_gate_holds_under_input_stimulus` |
| C5 | stopScreencast 终止+释放 | `c5_stop_terminates_and_releases` |
| C6 | format jpeg/png | `c3_c6_c7_frame_event_shape_and_formats` |
| C7 | headless 可用 | `c3_c6_c7_frame_event_shape_and_formats`(headless 形态)+ `ws_face_screencast_e2e`(真 WS 面)+ 输入环路三测(cdp_screencast_input_loop_*) |

## GAP 清单(按补缺合同粒度分组)

**P1 — 需用户裁定的需求缺口(非纯补测试)**
- **G1 · REQ-CDP-006 C6 请求拦截**:SPEC criterion 写「支持请求拦截」,现行实现架构裁决=诚实 fail-closed(无拦截设施,`Fetch.*` 显式 -32000、legacy setRequestInterception no-op)。两条路:(a) 实现 Fetch.requestPaused/continueRequest 拦截设施(大面);(b) 修订 criterion 语义(accepted-no-op+fail-closed 即达标)。**先裁定再立合同,禁直接派实现**。

**P2 — 补测试合同(实现已在库,补断言即可)**
- **G2 · REQ-CDP-008 C4**:为 `emit_target_closed_events`(targetDestroyed+detachedFromTarget)与 `emit_target_created_attached`(createTarget→attachedToTarget)补 ws_registry in-src 单测(2-3 个,纯 Rust)。合同粒度:单文件 `src/bao_browser/src/ws_registry.rs`,≤1 主文件。
- **G3 · REQ-CDP-007 C6 overlay**:功能=no-op。若裁定保留 criterion 语义→需实现高亮叠加(渲染面改动,大);若裁定降级(accepted-inert 即达标)→仅登记。**同 G1 先裁定**。

**P3 — 补测试合同(纯测试,无实现改动)**
- **G4 · REQ-CDP-001 C5**:cdp-server `process_sessions_once` 断开→`begin_close→finalize→notify_session_destroyed→remove` 路径单测(纯 Rust,轻;`src/cdp-server/src/server.rs` 单文件)。
- **G5 · REQ-CDP-004 C7**:hermetic new-document 重放测试(注册 marker 脚本→导航 data URL→断言 marker 先于页面脚本求值;消外网依赖,载体 `src/bao_browser/tests/suite` 单文件)。
- **G6 · REQ-CDP-008 C3 子项**:createTarget 后续 attachedToTarget 断言(可并入 G2 同文件同合同)。

**清理候选(非缺口,登记备查)**
- `bao_cdp_client e2e_playwright_compat.rs` 的 `e2e_real_playwright_full_flow` / `e2e_real_playwright_browser_context_isolation` 与 `e2e_internal_servo.rs::e2e_real_servo_full_navigation` / `e2e_real_servo_dom_queryselector` 为空占位(env 门内零断言);真客户端 oracle 已由 puppeteer/playwright-loop 测试承担。占位测试=绿色假象面,建议后续合同删除或接线。

## SUSPECT

无。两次 focused 实测全绿(cdp-server integration 9/9;bao_cdp 四组 302/302,零失败零 flake 迹象)。

## 附:测试实体面归属(供后续合同定位)

| 面 | 位置 | 角色 |
|---|---|---|
| 传输/服务器 | `src/cdp-server/tests/suite/`(21 文件)+ in-src server/session 测试 | REQ-CDP-001 传输+关停 |
| 协议/域 handler(内部后端) | `src/bao_cdp/tests/suite/`(25 文件)+ in-src protocol 测试 | 各域方法 schema/路由/fail-closed |
| WS 真 face | `src/bao_browser/src/ws_registry.rs`(in-src 11 tests)+ `tests/suite/cdp_ws_command_face_tests.rs`、`cdp_debugger_fidelity_tests.rs`、`cdp_universe_wiring_tests.rs`、`cdp_run_pump_event_tests.rs` | attach/autoAttach/executionContext/target 事件/断点真帧 |
| 真客户端 oracle | `tests/suite/puppeteer_e2e_tests.rs`(真 Node puppeteer-core)、`playwright_full_flow_loop_tests.rs` | REQ-CDP-001 C3/断连 |
| 客户端高层 API | `src/bao_cdp_client/tests/suite/`(20 文件,cdp_conformance 12 域 + bridge A/B/E/D 类 + 注入防御 + transport + url scheme) | 客户端协议兼容/注入安全 |
| screencast | `src/bao_browser/tests/suite/screencast_tests.rs` + `cdp_screencast_input_loop_tests.rs` | REQ-CDP-009 C1-C7 |
