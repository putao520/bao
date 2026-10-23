# BRW 域覆盖对等审计 — 2026-10-14

- **任务**: 54 条(实为 **66** 条,见 §0)criterion 逐条判定「已有功能对等测试 / 无测试(真缺口)」
- **范围**: 只读审计,零产品代码写入;本文件为唯一产出
- **方法**: SPEC criterion 全量提取(`.spec/10-REQUIREMENTS.html` data-criterion-id)→ 测试实体群功能索引(`src/bao_browser/tests/suite/` 109 文件 1334 个 `#[test]`,含 src 内联单测;bao_cdp/bao_engine 相关文件按域引用)→ 逐条读断言面判功能对等
- **判定口径**: COVERED = 有功能对等测试;PARTIAL = 有对等测试但断言面有已识别薄边;GAP = 无测试(真缺口)。判定基于静态断言面阅读 + 既有波末/soak 证据,本审计未复跑测试(零改动纯审计,live 族需 BAO_TEST_NETWORK/DISPLAY 门)
- **SUSPECT**: 无(未复跑,无失败输出可录)

## §0 总量修正

任务头按 54 条立项;实测 SPEC 在案 criterion = **66 条**(001:8 + 002:9 + 003:11 + 004:19 + 046:4 + 047:4 + 048:4 + 049:3 + 050:4)。下表 66 条全覆盖,无遗漏。

**汇总**: COVERED **55** / PARTIAL **9** / GAP **2**

## §1 逐条判定

图例: covered-by = `文件:测试名`;判定后一句话依据。

### REQ-BRW-001 libservo 集成 (8)

| criterion | 判定 | 依据 |
|---|---|---|
| C1 单进程运行(ServoBuilder + multiprocess=false) | PARTIAL | 功能形态全 e2e 隐式覆盖(multi_runtime_initial_load_tests 双臂=单进程内多 runtime 全链活;opts 派生自 `Opts::default()` 即 multiprocess=false,src/bao_browser/src/lib.rs:209);但无任何测试断言 flag 值 |
| C2 ServoDelegate 完整(notify_error/show_console_message/load_web_resource) | COVERED | page_pool_delegate_deep_tests.rs(servo_delegate_notify_error_*×4 + console_message)+ servo_delegate.rs 内联 22 测试;load_web_resource 为立法豁免(BCE-20260910-002 默认 DoNotIntercept 字节等价,lib.rs:534 PassThrough),行为面=fetch_axis_probe_tests::fetch_axis_probe_c_worker_realm |
| C3 WebViewDelegate 完整(screen_geometry/notify_url_changed/notify_load_status_changed/notify_new_frame_ready) | COVERED | delegate/webview_delegate.rs:52-459 四方法全实现 + 内联单测 4 个(test_notify_url_changed_emits_frame_navigated 等)+ 行为面(nav_race_repro/pagestate_lifecycle/screencast/webdriver_signal_exit) |
| C4 DOM 操作正常 | COVERED | dom_node_interop_tests.rs::dom_node_interop_full_chain + servo_render_pipeline_tests §3-§7(查询/遍历/mutate/createElement) |
| C5 CSS 样式计算(Stylo) | COVERED | css_conformance_tests.rs::css_conformance_computed_style_gate(73 断言/10 大类,精确值钉死) |
| C6 HTML 导航(WebView::load) | COVERED | servo_render_pipeline §1 + nav_race_repro_tests(6)+ pagestate_lifecycle_tests(12,状态机全序) |
| C7 无头截图(→RgbaImage) | COVERED | servo_render_pipeline §9(真渲染 PNG magic + >1KB)+ page_screenshot_deep_tests(encode 族 70) |
| C8 JS 在 webview 上下文执行 | COVERED | cdp_ws_command_face_tests::ws_cdp_evaluate_stays_in_page_realm + 全 suite evaluate 通道(evaluate_result_tests 29) |

### REQ-BRW-002 内存渲染 (9)

| criterion | 判定 | 依据 |
|---|---|---|
| C1 管线完整(HTML→Stylo→DL→webrender→RGBA) | COVERED | servo_render_pipeline_data_url_default_run(navigate→DOM 查询→screenshot 非空+PNG magic 全链) |
| C2 Flexbox 布局 | PARTIAL | css_conformance 有 flexbox 10 条 computed-style 探针(display/flex-*/gap/order/align-self);布局几何面(实际盒子位置)零断言 |
| C3 Grid 布局 | PARTIAL | css_conformance 仅 1 条 gate 探针(display:grid computed + pref 门 layout.grid.enabled);布局几何面零断言 |
| C4 文字渲染 | COVERED | webvtt_render_tests::webvtt_render_e2e_suite B2::cue_visible_in_render(字幕文字像素 diff,区域内外对比;gstreamer 门控,screenshot 通道坏时 loud skip)+ render pipeline nonempty |
| C5 PNG 输出 | COVERED | screenshot.rs 内联(encode_1x1/100x100_png magic)+ page_screenshot_deep png 全族 + 真渲染 PNG magic |
| C6 JPEG 输出 | COVERED | screenshot.rs 内联 encode_jpeg 族 + page_screenshot_deep jpeg 全族(alpha 剥离有专测) |
| C7 WebP 输出 | COVERED | screenshot.rs 内联 encode_1x1_webp_returns_nonempty_with_riff_magic + encode_100x100_webp_produces_valid_data |
| C8 headless SoftwareRenderingContext | COVERED | 全部 headless e2e 无窗口系统跑通真截图(render pipeline §9/webvtt B2/svg) |
| C9 单次截图延迟 ≤500ms@1920×1080 | **GAP** | 全仓无任何截图延迟断言;bench/harness stress_bench 只测页面生命周期 8 stage(Pending→Closed),无 screenshot stage |

### REQ-BRW-003 SpiderMonkey JSContext 融合 (11)

| criterion | 判定 | 依据 |
|---|---|---|
| C1 全局唯一 JSEngine + 每 ScriptThread 线程局部 JSContext | COVERED | multi_runtime_initial_load_tests(同进程第 2 runtime 初始 load 两臂)+ bce004_isolate_tests::bce004_iso_create_close_create;唯一性由 mozjs patch#2 OnceLock 保证(无显式计数断言,注) |
| C2 register_script_thread_callback 串行 + drain_callbacks 取回 | COVERED | servo_event_delivery_tests::real_path_delivery_probe_emitted_equals_pumped_under_high_load + cdp_run_pump_event_tests(pump 三面)+ evaluate_result_tests::shared_result_channel_*(OnceLock 跨线程) |
| C3 host fns 限 ScriptThread 初始化期注册 | COVERED | dom_node_interop(require/Buffer/process 页上 Node Realm 可达)+ worker_realm_api_tests::worker_realm_api_c8_crypto_performance_location(worker 侧同栈) |
| C4 主线程禁直接 SM API | COVERED | bao_lints BCE-012 AST 检测器(pattern.rs:127 `*mut JSObject` payload 分类,make bce-check 主门禁,bce012_smoke)+ 全 e2e 无跨线程 SM 崩溃行为面 |
| C5 Node/Page Realm 物理隔离 | COVERED | dom_node_interop §6::web_realm_isolated_from_require(evaluate_js_web "typeof require"==="undefined")+ compartment_isolation_tests::compartment_isolation_unconditional + security_sandbox_tests(window 属性枚举无 require/process/Buffer) |
| C6 evaluate_js EnterRealm(Node) 全权限 | COVERED | evaluate_result_tests::evaluate_in_node_realm_*(7 条,含错误面)+ dom_node_interop §1-§5(Buffer→DOM/path→href/crypto→dataset) |
| C7 Node Realm global 注册 console/require/fs 等 | COVERED | dom_node_interop_full_chain(互操作四场景)+ bao_runtime suite(fs/http/crypto 全 API 面,域属 ENG 详审) |
| C8 GC 由 servo spin_event_loop 统一管理 | PARTIAL | 无显式 GC 断言;行为面=churn/realm-discard/pagepool_chaos 族在 servo 泵下稳定(间接) |
| C9 无 JSEngine 重复初始化 | COVERED | multi_runtime_initial_load_tests 两臂 + bce004_iso_create_close_create(close→re-create) |
| C10 禁跨线程传递 JSObject 裸指针 | COVERED | task_10_integration::test_zero_jsobject_cross_thread_safe + worker_tests(global_addr 原子回填/unregister,zero-addr safe)+ bao_lints JSObject payload 门 |
| C11 opts.disable_script_debugger=true | PARTIAL | lib.rs:219 显式置位(源码面在案);无测试断言 flag 值;行为面=bce004_stress/isolate 族(OSR SIGSEGV 不复发回归钉) |

### REQ-BRW-004 浏览器 Web Worker API (19)

| criterion | 判定 | 依据 |
|---|---|---|
| C1 new Worker 构造执行 | COVERED | worker_onerror_integration_tests::servo_native_worker_executes_probe + worker_lifecycle_tests::worker_create_postmessage_shutdown_recreate_lifecycle(live) |
| C2 postMessage page→worker + onmessage | COVERED | worker_realm_api_tests::worker_realm_api_c2_page_to_worker_postmessage_echo(live 往返) |
| C3 worker→page + worker.onmessage | COVERED | 同上 echo 往返 + worker_tests::test_worker_channel_bridge_worker_to_page(单元) |
| C4 worker.terminate()(closing+interrupt) | COVERED | worker_tests::test_worker_handle_terminate_sets_closing/idempotent + c18_concurrent_terminate_closing_flag_consistent + c18 teardown path1(live) |
| C5 self.close() | COVERED | worker_concurrent_servo_tests::c18_three_path_teardown_crash_free path2(worker 脚本 self.close,live crash-free) |
| C6 Structured Clone(对象/数组/Buffer/AB/Transferable) | COVERED | worker_realm_api_c6_structured_clone_roundtrip_five_types(live 五型)+ worker_tests::test_structured_clone_payload_with_transferable |
| C7 Worker 独立 Runtime/JSContext | PARTIAL | 无直接断言(worker runtime 为 servo 内部物);功能面=worker 脚本执行/onerror/echo 全活(间接) |
| C8 DedicatedWorkerGlobalScope API 面(self/close/importScripts/setTimeout/fetch/crypto/performance/location/navigator) | COVERED | worker_realm_api_c8_crypto_performance_location + c6_import_scripts_helper_globals + fetch_axis_probe_tests C 轴(worker realm fetch) + worker_navigator 族;setTimeout 无 worker 内显式断言(薄边注记) |
| C9 onerror 传播(ErrorEvent 四字段) | COVERED | worker_onerror_integration_tests 7 条(servo_native_onerror_error_event_fields 断言 message/filename/lineno/colno 全四) |
| C10 页面卸载自动终止(track_worker+AutoCloseWorker) | COVERED | worker_tests::test_auto_close_worker_drop_terminates + test_three_teardown_paths_crash_safe + c18 path3(page.close()→teardown,live) |
| C11 bun_sm stub 删除 | COVERED | 静态事实实测:src/bun_sm/src/web_worker.rs 不存在(find 零命中);完成性 criterion 非测试面 |
| C12 navigator 一致 | COVERED | worker_fingerprint_consistency_tests::c12_worker_navigator_matches_main_thread_per_field |
| C13 Canvas 一致(OffscreenCanvas 同 profile) | COVERED | stealth_offscreencanvas_tests c13 族(2d pipeline/retirement toDataURL+getImageData parity)+ stealth_per_page_canvas_tests::worker_offscreencanvas_rides_host_page_profile_live |
| C14 WebGL 一致 | COVERED | stealth_offscreencanvas_tests c14 族(webgl1/2 pipeline + getshaderprecisionformat + window control probe 对照) |
| C15 Audio 一致 | COVERED | stealth_worker_audio_tests c15 族(offlineaudiocontext full stack/cross-realm 一致/seed 分歧/window 零回归) |
| C16 无原生泄漏(对无 stealth 基线不同) | COVERED | worker_fingerprint_consistency_tests::c16_worker_surface_differs_from_no_stealth_baseline |
| C17 跨线程指纹摘要一致 | COVERED | worker_fingerprint_consistency_tests::c17_worker_fingerprint_digest_equals_main_thread_digest |
| C18 三路径 teardown crash-safe | COVERED | worker_concurrent_servo_tests c18 族(5)+ task_10_integration::test_crash_safe_worker_concurrent_destroy + bce004_stress c18 三兄弟 + bce004_nfr_memsaf_001_ebusy_mutex_destroy_regression(EBUSY 回归钉) |
| C19 SW fetch 拦截 × stealth/CDP 边界(三子断言) | COVERED | sw_stealth_profile_tests c19_sub1(TLS/H2 profile)/sub2(CDP Network 可观测)/sub3(跨页继承+terminate 注销)+ serviceworker_mediation_tests(4 live)+ fetchevent/controller(live) |

### REQ-BRW-046 SVG 几何 (4)

| criterion | 判定 | 依据 |
|---|---|---|
| AC01 getBBox 真实包围盒 | COVERED | svg_dom_geometry_tests::svg_dom_geometry_apis_return_real_values AC01×3(circle/rect/g union 精确值) |
| AC02 getCTM/getScreenCTM 真实矩阵 | COVERED | 同测试 AC02×4(自身 transform/祖先继承/单位阵/getScreenCTM) |
| AC03 getTotalLength/getPointAtLength | COVERED | 同测试 AC03×5(3-4-5 斜边=50/端点/中点/ellipse Ramanujan≈96.9) |
| AC04 既有 26 接口反射与类型不回退 | PARTIAL | 同测试 AC04 为代表性断言(SVGCircleElement/SVGElement instanceof + 5 方法面 + SVGGraphicsElement/SVGGeometryElement 在位),非 26 接口逐一枚举;WPT svg 域为补充载体 |

### REQ-BRW-047 WebVTT (4)

| criterion | 判定 | 依据 |
|---|---|---|
| AC01 active cue 渲染可见 | COVERED | webvtt_render_e2e_suite B2::cue_visible_in_render(截图区域像素 diff,gstreamer 门控 loud-skip) |
| AC02 currentTime 驱动激活(进出区间) | COVERED | 同测试 B2::active_cues_during_window + B4::cue_exits_window(seek 驱动 enter/exit) |
| AC03 定位属性(line/position/align/size)CSS WebVTT 布局生效 | PARTIAL | B3::positioning_changes_output 仅证 align:left/position:10%/size:40% 左右半区分;**line 属性无探针** |
| AC04 解析/JS API 不回退(TextTrack/VTTCue/CueList) | COVERED | 同测试 A1-A6(track_loaded/cue_list_parsed/cue_track_association/cue_order_sorted/getCueAsHTML 构造规则/activeCues)+ vendor WPT track-element 域 |

### REQ-BRW-048 DevTools CDP 域 (4)

| criterion | 判定 | 依据 |
|---|---|---|
| AC01 CSS.getMatched/getComputed 真实样式 | COVERED | bao_cdp/tests/suite/devtools_dom_css_tests.rs::computed_style_matches_page_declarations + matched_styles_report_author_rules + inline_styles_report_style_attribute(production dispatch+bridge+servo 全栈) |
| AC02 DOM 查询面经等价 CDP 客户端断言 | COVERED | 同文件 flattened_document_depth_semantics_and_parent_links + node_for_owner_maps_object_id_to_canonical_node_id + query_selector_returns_canonical_node_ids + describe_node/get_box_model/resolve_node/push_nodes_by_backend_ids(canonical NodeId 公式跨面等式断言) |
| AC03 既有 12 域零回归(全量 CDP suite 绿) | COVERED | cdp-server suite 在案(套级回归声明;分域命令/事件细节归并行合同 cov-audit-cdp 详审,不重复) |
| AC04 方法族覆盖清单落 SPEC 记录 | COVERED | SPEC 载体:.spec/10-REQUIREMENTS.html REQ-BRW-048 description「覆盖清单记录(AC04,2026-09-27 实施波+后续波)」含 setStyleTexts 寻址偏差裁定与 fail-closed 错误面 |

### REQ-BRW-049 白空间继承面 (3)

| criterion | 判定 | 依据 |
|---|---|---|
| AC01 CSS 能力验证面落盘可执行测试 | COVERED | css_conformance_computed_style_gate(73 断言/10 类,语料=~/code/tools/wpt-css 稳定核心属性面,头注释立法波末门语义) |
| AC02 pref-gated CSS 能力启用状态清单 | COVERED | 载体=css_conformance_tests.rs 头注释立法表(22 条 pref 逐项默认状态)+ grid gate 探针防漂移(门翻转=探针红,强制同步更新表) |
| AC03 每次 Stylo 吸收波执行该验证面,结果进波末证据 | PARTIAL | 过程性要求:头注释立法「吸收波合入前必须跑」在案,执行留痕见 .plans/soak-leak-rca.md(族回归含 css_conformance 50/50、65/65);无吸收波机检强制(靠纪律非工具) |

### REQ-BRW-050 EditContext (4)

| criterion | 判定 | 依据 |
|---|---|---|
| C1 构造器 + beforeinput/textupdate/textformatupdate W3C 语义 | COVERED | editcontext_api_tests::editcontext_api_c1_constructor_surface(dictionary/defaults/枚举校验/updateText 状态机)+ c3_event_dispatch(三事件 addEventListener/on* 属性) |
| C2 editContext 读写 + 解绑恢复 | COVERED | c2_association(roundtrip/detach/单元素 NotSupportedError/TypeError/disallowed 列表) |
| C3 CDP Input.insertText/键投递驱动可验证 | COVERED | c4_driver_interop(真键投递 trusted beforeinput+textupdate+零 DOM 突变+cancel 语义;insertText 载体面由 editing_e2e_tests::editing_e2e_insert_text_carrier_input_and_textarea 补) |
| C4 原型面/getOwnPropertyNames 与真 Chrome 逐格不可区分 | **GAP** | c1 断言 editContext∈HTMLElement.prototype、c3 断言五个 on* 属性在位,但「getOwnPropertyNames 全枚举与真 Chrome 基线逐格对照」无任何测试载体;WPT editing/edit-context 是行为面补充,非枚举对照 |

## §2 GAP 清单(按补什么测试分组,建议合同粒度)

### 硬缺口(2 条 criterion → 2 个小合同)

| # | 补什么 | criterion | 建议合同(粒度:单文件可判完成) |
|---|---|---|---|
| G1 | 截图延迟断言:1920×1080 视口 `take_screenshot` 计时 assert ≤500ms(或 bench stress_bench 增 Screenshot stage 采样) | REQ-BRW-002-C9 | 新增 `screenshot_latency_tests.rs` 一个 live 测试(复用 servo_render_pipeline 骨架);或 bench harness 加 stage。二选一,推荐前者(criterion 语义是验收断言) |
| G2 | EditContext 原型面 Chrome parity:getOwnPropertyNames(EditContext.prototype / TextFormat / 两 Event.prototype)对 Chrome 采集基线 fixture 逐格集合相等断言 | REQ-BRW-050-C4 | editcontext_api_tests 增 1 个测试 + Chrome 基线 fixture(常量数组);Chrome 侧采集一次人肉/CDP 脚本即可 |

### PARTIAL 补强(9 条 criterion → 3 个合同)

| # | 补什么 | criterion | 建议合同 |
|---|---|---|---|
| P1 | opts 不变量 pin:BrowserRuntime::new 后断言 `servo::opts::get().multiprocess == false` 且 `.disable_script_debugger == true`(进程单例,断言须在首个 runtime 前读或用专门单测进程) | REQ-BRW-001-C1 + REQ-BRW-003-C11(同形状合并) | 单文件单测(bao_browser suite),1 个合同 |
| P2 | 布局几何探针:flex 容器子项 getBoundingClientRect 断言(row/column/justify 对齐)+ grid 轨道位置;进 css_conformance 或新 layout_geometry_tests | REQ-BRW-002-C2 + C3 | 单文件 live 测试,1 个合同 |
| P3 | 零散薄边:①REQ-BRW-003-C8 GC 显式断言(多轮 churn 后 `js::gc` 计量面/或 memory stats 稳定,可与 P1 合并为不变量 pin 合同);②REQ-BRW-004-C7 worker 独立 runtime 可观测断言(弱价值,可接受现状);③REQ-BRW-046-AC04 26 接口逐一 instanceof+关键方法枚举(表驱动一测);④REQ-BRW-047-AC03 cue line 属性探针(负 position/像素行偏移);⑤REQ-BRW-049-AC03 吸收波机检钩子(absorb skill 流水把 `cargo nt -E 'test(css_conformance)'` 列为 stylo 波必跑步,属流程件非测试件) | 见左 | ④⑤ 小合同各 0.5 粒度;③ 单测合同;①②可选 |

## §3 审计证据锚

- 测试索引: bao_browser suite 109 文件 1334 `#[test]`(含 `#[test]`/fn 间注释容差提取);src 内联单测(bao_browser/src/screenshot.rs 10、delegate/* 26+)另计
- SPEC 提取: `.spec/10-REQUIREMENTS.html` data-criterion-id 全量(66 条,data-req 块边界切分)
- 判定读过的关键断言文件: svg_dom_geometry_tests.rs(全)/webvtt_render_tests.rs(全)/editcontext_api_tests.rs(全)/css_conformance_tests.rs(头 140 行+探针注释)/servo_render_pipeline_tests.rs(全)/dom_node_interop_tests.rs(头+§6)/compartment_isolation_tests.rs(头)/devtools_dom_css_tests.rs(头+测试名)/worker_concurrent_servo_tests.rs(path 注释)/delegate/{servo,webview}_delegate.rs(方法面)
- 未复跑测试: 零改动纯审计,live 族门控(BAO_TEST_NETWORK/DISPLAY/gstreamer)不在本合同触发;近期绿证据=soak 2026-10-13 streak 12 + 波末验收记录(.plans/soak-leak-rca.md 族回归数)
