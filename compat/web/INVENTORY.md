# Web 目标面 INVENTORY(W13 · 2026-09-29 · #14 G1 收官)

> 分类准绳:**测试在树 + 电池 v3 绿**(test-ci 档;三红 CDP 面已另修,见
> soak-leak-rca.md W12 段)。家族级粒度;每行 = servo 上游支持度 × Bao 集成
> 状态(Supported/Partial/Unsupported)× 证据(测试文件/立法 REQ)。
> **不造 servo 的 WPT 数字**——Bao 侧无自己跑分,WPT 首跑=后续波(见文末缺口)。

## 锚定

| 项 | 值 |
|----|----|
| servo vendor ref | `c9fed3ed`(2026-09-29 W12-B tip;基线=2026-08-13 上游 HEAD + 定制文件,清单见 CLAUDE.md servo 定制文件表 31+ 行) |
| WPT ref | `vendor/servo/tests/wpt/tests/html` 在树(上游 wpt 子集检入;Bao 未接 runner) |
| Bao 电池 | `cargo nextest -p bao-browser/-p bun_runtime --cargo-profile test-ci`(v3:865+ 绿) |
| 立法 | .spec/10-REQUIREMENTS.html REQ-BRW-046(SVG 几何)/047(WebVTT)/048(CDP DevTools)/049(CSS 能力继承) |

## 家族矩阵

| # | 家族 | servo 上游 | Bao 集成 | 证据(在树+绿) | 缺口注记 |
|---|------|-----------|:---:|----------------|----------|
| 1 | DOM 核心 | ✓ 持续测(wpt.fyi servo) | **Supported** | web_api_tests/web_api_deep_tests/globals_deep_tests(bao_runtime)+ dom_node_interop(bao_browser) | — |
| 2 | DOM SVG 几何与反射(REQ-BRW-046,fork 自实现) | 上游 stub | **Supported** | svg_dom_geometry_tests(13 断言) | 26 接口反射行为保持已锁 |
| 3 | HTML(含 **WebVTT 渲染** REQ-BRW-047,fork 自实现) | parse 在位/render 上游缺 | **Supported** | webvtt_render_tests | track 元素随 ④7 吸收波在树 |
| 4 | CSS 能力继承(REQ-BRW-049) | ✓ Stylo 全引擎 | **Supported** | css_conformance_tests(73 断言门 + 探针) | 像素回归=后续波 |
| 5 | Fetch | ✓ + Bao 统一 fetch 面(U2) | **Supported** | fetch_axis_probe_tests + h2_fetch_node_stack_e2e(nextest 隔离注记在案) | — |
| 6 | WebSocket | ✓ + Bao WS 面 | **Partial** | page_wss_bao_tls_e2e_tests(wss/TLS);README Partial 标记 | 完成度差距未枚举到 method 级 |
| 7 | Workers(Dedicated/Shared/SW) | ✓ | **Supported**(Dedicated/SW)/ **Partial**(Shared) | worker_lifecycle_tests(W12-B)/worker_realm_api/worker_onerror/serviceworker_controller+fetchevent+mediation ×3 | Shared Worker 独立 e2e 未落 |
| 8 | Canvas 2D / WebGL / Offscreen | ✓ | **Partial** | stealth_per_page_canvas + stealth_offscreencanvas + media_e2e(stealth 域视角) | canvas API 一致性 conformance 面=未落(仅 stealth 视角) |
| 9 | Storage(IDB / cookies / cache) | ✓(IDB 双终局门 patch 在树) | **Supported**(IDB)/ **Partial**(cache/cookies) | indexeddb_e2e_tests | cookies/cache 独立 e2e 未落 |
| 10 | Navigation / lifecycle | ✓ + RED-1 discard 家族 | **Supported** | pagestate_lifecycle + realm_discard_timers ×4(RED-1/W7) | — |
| 11 | Events | ✓ | **Supported** | events_deep_tests + events_path_deep_tests(bao_runtime);页面事件随 1/10 面 | — |
| 12 | Timers + 微任务 | ✓ + SM153 引擎微任务队列 | **Supported** | event_loop_timing_tests + interrupt_timeout_tests(#24;含 timeout/cancel 联动) | — |
| 13 | URL + 编码 | ✓ | **Supported** | url_deep_tests + node_url_tests(bao_runtime) | — |
| 14 | 媒体(audio/video) | ✓(servo-media + GStreamer 后端) | **Partial** | media_e2e_tests(GStreamer 后端视角) | 无 GStreamer 的环境=该面不可测(环境依赖) |

**分布小结**:Supported 10 / Partial 4 / Unsupported 0。

## BAO↔Servo divergence 面

- **手工清单**:CLAUDE.md「servo 定制文件清单」表(31+ 行:BCE/RED-1/C19/R53/
  W12-B console.rs safe 变体等,每行含 patch 概要与立法链)——本表引用之。
- **自动化 divergence 检测**:未落(#14-G 项缺口,如实记录)= 后续波。

## WPT 面(缺口,列后续波)

- Bao 自跑 WPT 子集(DOM/HTML/CSS/Fetch):runner 未接(vendor 内 wpt/tests/html
  在树但需 servo wpt runner + 显示环境);首跑产出 servo↔Bao 偏差表 = 后续波。
- 像素级渲染回归:依赖 display/webrender 基线设施 = 后续波(与 §4 缺口同源)。
- `bao compat web` 聚合命令:未实现(与 CDP 命令同形,后续波)。

## 版式说明

- 「Supported」= 测试在树且电池 v3 绿;「Partial」= 有测试面但覆盖维度不全或
  依赖外部环境;「Unsupported」= 无任何在树覆盖(本表现状为零)。
- 后续波把 Partial 逐格补齐后,本表升级为与 servo wpt.fyi 对齐的偏差表。
