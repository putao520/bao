# 开发计划: bao_cdp_client 内化 chromiumoxide + servo RDP 桥接 | epoch: 1 | status: active

## 范围
基于 DEC-CDP-001/DEC-CDP-002/DEC-URL-001,Fork chromiumoxide 改名重构为 bao_cdp_client,
完整实现 22 Domain 193 method,直接桥接 servo devtools_traits RDP,抛弃 evaluate_js 注入。

REQ 列表:REQ-BAO-API-001 ~ REQ-BAO-API-008

## 影响矩阵

| SPEC ID | 关联 TASK | 文件 |
|---------|----------|------|
| REQ-BAO-API-001 (Browser::connect URL scheme) | TASK-1 | bao_cdp_client/src/browser.rs |
| REQ-BAO-API-002 (Transport trait 双实现) | TASK-2 | bao_cdp_client/src/transport/* |
| REQ-BAO-API-003 (servo 7 事件 → CDP event) | TASK-4 | bao_cdp_client/src/bridge/event_translator.rs |
| REQ-BAO-API-004 (A 类机械映射 48) | TASK-3 | bao_cdp_client/src/bridge/command_dispatcher.rs |
| REQ-BAO-API-005 (B 类 Eval 合成 52) | TASK-3 | bao_cdp_client/src/bridge/eval_synthesizer.rs |
| REQ-BAO-API-006 (D 类纯状态 62) | TASK-5 | bao_cdp_client/src/api/* |
| REQ-BAO-API-007 (E 类 -32601 31) | TASK-3 | bao_cdp_client/src/bridge/command_dispatcher.rs |
| REQ-BAO-API-008 (公共 API + 文档) | TASK-7 | bao_cdp_client/src/lib.rs + docs/ |

## 任务树(扁平,顺序由依赖决定)

### TASK-1: Fork chromiumoxide → bao_cdp_client 基础设施
- SPEC: REQ-BAO-API-001 [验收] | 文件: src/bao_cdp_client/{Cargo.toml, lib.rs, browser.rs, error.rs} | 实现: ① 从 chromiumoxide 0.5+ 源码 Fork 到 src/bao_cdp_client/ ② Cargo.toml 改名 bao_cdp_client,删 tokio 依赖,加 bun_event_loop/bun_url/bao_cdp/bao_browser 依赖 ③ 重命名 crate 内 chromiumoxide* identifiers 为 bao_cdp_client* ④ 实现 Browser::connect(url: &str) 入口,URL scheme 路由:"memory://" → InMemoryTransport,"ws://"|"http://" → WebSocketTransport ⑤ 错误码体系(ConnectError/LaunchError/InvalidSchemeError) | 验收: cargo build -p bao_cdp_client 通过 + Browser::connect("memory://bao") 返回 Browser 实例 + Browser::connect("ws://...") 走 WebSocket 路径 | 依赖: (无) | 状态: pending
- 复用锚点:
  - spec: [REQ-BAO-API-001(新建), DEC-URL-001(已写入)]
  - code: [chromiumoxide 上游源码(cargo download 或 git subtree)]
  - code: [bao_cdp/src/protocol.rs(已实现的 CDP JSON-RPC 编解码)]
  - pattern: [bun_url::URL::from_utf8 → scheme() 判定]
  → chromiumoxide 上游为新建依赖源

### TASK-2: Transport trait + 双实现
- SPEC: REQ-BAO-API-002 [验收] | 文件: src/bao_cdp_client/src/transport/{trait.rs, in_memory_transport.rs, ws_transport.rs} | 实现: ① trait Transport { async fn send_command(method, params, session_id) → Result<Value>; async fn recv_event() → Result<Event>; async fn close() → Result<()>; } ② InMemoryTransport 包裹 Arc<CDPRdpBridge>,直调 servo devtools_traits ③ WebSocketTransport 复用 bao_cdp::ws_codec + bun_uws 替换 tungstenite ④ 错误类型 TransportError/TimeoutError/ConnectionClosed | 验收: trait 抽象完整 + InMemoryTransport 命令往返延迟 <1ms + WebSocketTransport 能与外部 Chrome 9222 通信 | 依赖: TASK-1 | 状态: pending
- 复用锚点:
  - spec: [IF-CDPC-001(已写入), DEC-CDP-002(crossbeam channel)]
  - code: [bao_cdp/src/ws_codec.rs(RFC 6455 帧编解码,730 行可直接复用)]
  - code: [bao_cdp/src/ws_handshake.rs(WebSocket 握手,175 行)]
  - code: [bao_cdp/src/ws.rs(WebSocket 顶层,177 行)]
  - code: [bao_browser/src/page.rs::PageHandle(InMemory 调用目标)]
  - pattern: [crossbeam::channel bounded(N) + select! timeout]
  → ws_codec/ws_handshake/ws 三个文件直接迁移

### TASK-3: CDPRdpBridge 核心 + 193 method 分发
- SPEC: REQ-BAO-API-004 [A 类 48] + REQ-BAO-API-005 [B 类 52] + REQ-BAO-API-007 [E 类 31] [验收] | 文件: src/bao_cdp_client/src/bridge/{cdp_rdp_bridge.rs, command_dispatcher.rs, eval_synthesizer.rs} | 实现: ① CDPRdpBridge 持有 crossbeam::channel sender + PagePool 引用 + servo delegate ② command_dispatcher.rs 用 match (domain, method) 分发 193 method:A 类机械映射直调 servo API(如 Page.navigate → PageHandle::navigate),B 类用 eval_synthesizer 通过 Runtime.evaluate + IIFE 合成(如 page.title → evaluate("document.title")),E 类 servo 不支持的 31 method 返回 -32601 错误码,External 模式透传 ③ IIFE 安全封装:(function(){...})() + JSON.stringify 参数化,禁止字符串拼接 ④ 每个 method 完整 TDD:含正常 case + JS 注入防御测试 | 验收: 193 method 100% 覆盖(spec scan 验证) + 0 evaluate_js 注入(直接调 servo API) + 每个 B 类 method 有注入测试 | 依赖: TASK-2 | 状态: pending
- 复用锚点:
  - spec: [REQ-BAO-API-004/005/007(新建), DEC-CDP-002(已写入)]
  - code: [bao_browser/src/page.rs::PageHandle::navigate/evaluate_js_web/take_screenshot]
  - code: [bao_browser/src/page_pool.rs::PagePool::create_page/get_page]
  - code: [bao_cdp/src/servo_bridge.rs::BridgeCommand(参考枚举设计)]
  - code: [servo/components/devtools_traits::DevtoolScriptControlMsg(Debugger 桥接)]
  - code: [servo/components/devtools/src/actors/{console.rs,network_event.rs,inspector.rs,breakpoint.rs,pause.rs}]
  - pattern: [match (domain, method) → 路由到 servo actor]
  → 抛弃 bao_cdp/src/domains/*.rs 旧 evaluate_js 实现

### TASK-4: servo 7 大事件 → CDP event 转换
- SPEC: REQ-BAO-API-003 [验收] | 文件: src/bao_cdp_client/src/bridge/event_translator.rs | 实现: ① servo ConsoleEvent → CDP Log.entryAdded ② servo PageErrorEvent → CDP Runtime.exceptionThrown ③ servo NetworkEvent → CDP Network.{requestWillBeSent, responseReceived, loadingFinished, loadingFailed} ④ servo DomMutationEvent → CDP DOM.{attributeModified, characterDataModified} ⑤ servo SourceInfoEvent → CDP Debugger.scriptParsed ⑥ servo FrameInfoEvent → CDP Page.{frameNavigated, frameStartedLoading, frameStoppedLoading} ⑦ servo TimelineMarkerEvent → CDP Performance.metrics ⑧ 事件订阅通过 servo delegate 回调 + crossbeam::channel 推送到 Transport::recv_event | 验收: 7 类事件全覆盖 + 零遗漏(spec audit) + 事件反向推送端到端测试(servo 触发 → CDP client 收到) | 依赖: TASK-3 | 状态: pending
- 复用锚点:
  - spec: [REQ-BAO-API-003(新建)]
  - code: [bao_browser/src/delegate.rs::BaoServoDelegate(servo 事件回调)]
  - code: [servo/components/devtools/src/actors/console.rs::ConsoleActor]
  - pattern: [servo delegate callback → event_translator → Transport::recv_event]
  → delegate 现有的事件回调扩展为 CDP event

### TASK-5: D 类 62 method 纯状态管理 + 高层 API 类
- SPEC: REQ-BAO-API-006 [验收] | 文件: src/bao_cdp_client/src/api/{browser.rs, browser_context.rs, page.rs, frame.rs, element.rs, request.rs, response.rs, dialog.rs, console_message.rs, keyboard.rs, mouse.rs, coverage.rs, tracing.rs, accessibility.rs} | 实现: ① Browser 类:isConnected/process/wsEndpoint/version/userAgent/disconnect/close ② BrowserContext:browser/pages/isIncognito/overridePermissions/clearPermissionOverrides/close ③ Page:browser/browserContext/isClosed/mainFrame/frames/workers/viewport/mouse/keyboard/coverage/tracing/accessibility/target/on/off/once/setDefaultTimeout ④ Frame:executionContext/isDetached/childFrames/parentFrame/name/url/$/$$ ⑤ Element:asElement/jsonValue/dispose ⑥ Request/Response/Dialog/ConsoleMessage:全部本地状态缓存 ⑦ EventEmitter 模式(on/off/once/removeAllListeners/listenerCount) | 验收: 62 method 100% 覆盖 + EventEmitter 完整 + 本地状态无外部往返 | 依赖: TASK-3 | 状态: pending
- 复用锚点:
  - spec: [REQ-BAO-API-006(新建), IF-CDPC-004(已写入)]
  - code: [chromiumoxide 上游 src/browser.rs / src/page.rs 参考]
  - pattern: [Rc<RefCell<>> 本地状态 + EventEmitter trait]
  → 全部新建,不依赖 bao_browser PageHandle

### TASK-6: bao_cdp 旧 domain 删除 + CDP server 重组
- SPEC: (兼容性) | 文件: src/bao_cdp/src/domains/*.{rs(11 个文件, ~3500 行) 删除}, src/bao_cdp/src/lib.rs(CdpServer 改造为对外 Playwright 入口) | 实现: ① 删除 bao_cdp/src/domains/{css.rs, debugger.rs, dom.rs, emulation.rs, fetch_domain.rs, input.rs, log_domain.rs, network.rs, overlay.rs, page.rs, runtime.rs, target.rs(共 3500 行 evaluate_js 注入实现)} ② 删除 bao_cdp/src/servo_bridge.rs(516 行 BridgeCommand 枚举,被 CDPRdpBridge 替代) ③ 改造 bao_cdp/src/lib.rs CdpServer:接受外部 Playwright 连接,内部转发到 bao_cdp_client::CDPRdpBridge(顺势实现对外 CDP server 入口) ④ bao_cdp Cargo.toml 删除冗余依赖 ⑤ 兼容性:其他 crate 依赖 bao_cdp::protocol 等保留 | 验收: bao_cdp LOC 减少 ~4000 行 + cargo build --workspace 通过 + 现有 bao_cdp 测试迁移到 bao_cdp_client | 依赖: TASK-3 | 状态: pending
- 复用锚点:
  - spec: [DEC-CDP-001(已写入, bao_cdp 重组策略)]
  - code: [bao_cdp/src/ws_codec.rs(保留作为 ws transport 复用源)]
  - code: [bao_cdp/src/protocol.rs(保留作为 CDP JSON-RPC 编解码)]
  - code: [bao_cdp/src/lib.rs CdpServer(改造为对外入口)]
  → 删除 11 个 domain 文件 + servo_bridge.rs

### TASK-7: 公共 API + 文档 + JS 全局对象暴露
- SPEC: REQ-BAO-API-008 [验收] | 文件: src/bao_cdp_client/src/lib.rs(pub use 导出), docs/api.md(每个 method doc comment + 示例), src/bao_runtime/src/globals.rs(Bao.browser 全局对象 JS 暴露) | 实现: ① lib.rs pub use 暴露:Browser/Page/Element/Frame/Cookie/Network/Dialog/ConsoleMessage/BrowserContext/HTTPRequest/HTTPResponse/JSHandle/ElementHandle/ScreenshotFormat/WaitUntilState/DeviceDescriptor ② 每个 pub method 必须有 ///doc comment + 示例代码 ③ bao_runtime globals.rs 暴露 Bao.browser.connect(url) → 返回 JS Browser proxy(通过 bao_engine JSClass 桥接) ④ examples/ 目录:basic_usage.rs(连接 + 导航 + 截图)/ external_chrome.rs(连外部 Chrome)/ multi_page.rs(多页面) | 验收: cargo doc 无 warning + 5+ examples 编译通过 + JS 全局 Bao.browser 可用 | 依赖: TASK-5 | 状态: pending
- 复用锚点:
  - spec: [REQ-BAO-API-008(新建), IF-CDPC-003(已写入)]
  - code: [bao_engine/src/class_def.rs(JSClass 注册模式)]
  - code: [bao_runtime/src/globals.rs(现有 Bun.* 全局对象模式)]
  - pattern: [bao_engine::register_class → JS proxy → Rust trait 调用]
  → JS 暴露参考现有 Bun.* 模式

### TASK-8: 端到端测试
- SPEC: 全部 [验收] | 文件: src/bao_cdp_client/tests/{e2e_internal_servo.rs, e2e_external_chrome.rs, e2e_playwright_compat.rs, injection_defense.rs, event_coverage.rs} | 实现: ① e2e_internal_servo.rs:Browser::connect("memory://bao") + page.goto + screenshot + cookie + click 完整流程 ② e2e_external_chrome.rs:Browser::connect("ws://127.0.0.1:9222") 连真实 Chrome,完整自动化流程 ③ e2e_playwright_compat.rs:启动 bao_cdp CDP server,Playwright(Node.js)连接,完整自动化流程 ④ injection_defense.rs:52 个 B 类 method 的 JS 注入防御测试 ⑤ event_coverage.rs:7 类 servo 事件 → CDP event 端到端验证 | 验收: 全部测试通过 + 0 注入漏洞 + Playwright 兼容性 E2E 通过 | 依赖: TASK-4, TASK-5, TASK-7 | 状态: pending
- 复用锚点:
  - spec: [TEST-CDP-001~008(已有), TEST-BAO-API-001~008(待新建)]
  - code: [bao_cdp/tests/ws_resilience_tests.rs(参考 CDP 测试模式)]
  - code: [bao_browser/tests/(参考 servo 真链路测试)]
  - pattern: [real servo + real Chrome 双链路测试]
  → 集成测试覆盖完整 193 method + 7 事件

### TASK-9: cargo test --workspace 连续3次通过
- SPEC: 全部 [验收] | 实现: cargo test --workspace --exclude mozjs* --exclude bun_uws_sys,连续3次通过 | 验收: 3/3 pass | 依赖: TASK-1..TASK-8 全部 completed | 状态: pending

### TASK-11: AAA 模式补全 + @trace level 标注(P-1 红线修复)
- SPEC: 全部 [验收] | 文件: src/bao_cdp_client/tests/*.rs(16 文件, 7981 行, ~500+ 测试函数) | 实现: ① 所有 #[test] 函数补全 AAA 三段式注释(`// Arrange` / `// Act` / `// Assert`),按 P-1 红线 ② 所有 integration 测试 @trace 补 `[level:integration]` 标注(修复 test_quality audit bao-api integration 0% 假象) ③ E2E 真环境 `#[ignore]` 测试 @trace 标 `[level:system]`(测试金字塔顶层) ④ 简单 assert 辅助函数(如 assert_e_class)的调用方仍要 AAA 标注(说明 arrange/act 隐含在 helper 内) | 验收: `grep -L "// Arrange" tests/*.rs` 返回空(16/16 文件全部有 AAA) + test_quality audit bao-api integration ≥80% + 0 测试函数缺 level | 依赖: TASK-8 | 状态: pending

### TASK-10: SPEC @trace 注入 + audit 100%
- SPEC: 全部 [验收] | 实现: ① bao_cdp_client 所有 pub method 标注 // @trace REQ-BAO-API-XXX [interface:Browser/Page/...] ② spec(action="scan", scanMode="trace_annotations") 验证 0 untraced ③ spec(action="check", auditAction="audit", auditMode="req_coverage") 验证 REQ-BAO-API-001~008 100% 覆盖 ④ spec(action="check", auditAction="audit", auditMode="maturity") ≥80 | 验收: 0 untraced + 100% REQ 覆盖 + maturity ≥80 | 依赖: TASK-9 | 状态: pending

## Bug 日志

(执行中追加)

## REQ 台账

| REQ ID | 验收标准 | 关联 TASK | 闭合状态 |
|--------|---------|----------|---------|
| REQ-BAO-API-001 | Browser::connect URL scheme 路由 | TASK-1 | pending |
| REQ-BAO-API-002 | Transport trait + 双实现 | TASK-2 | pending |
| REQ-BAO-API-003 | servo 7 事件 → CDP event | TASK-4 | pending |
| REQ-BAO-API-004 | A 类机械映射 48 method | TASK-3 | pending |
| REQ-BAO-API-005 | B 类 Eval 合成 52 method | TASK-3 | pending |
| REQ-BAO-API-006 | D 类纯状态 62 method | TASK-5 | pending |
| REQ-BAO-API-007 | E 类 -32601 31 method | TASK-3 | pending |
| REQ-BAO-API-008 | 公共 API + 文档 | TASK-7 | pending |
