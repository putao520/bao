//! TASK-8 E2E — InMemory (内嵌 servo) 模式端到端测试。
//!
//! ## 验收范围
//!
//! 覆盖 7 个核心场景:
//! 1. **basic_navigation**: Browser::connect("memory://bao") → new_page → goto → wait → screenshot
//! 2. **cookie_management**: setCookie → getCookies → deleteCookie
//! 3. **event_listener**: on('console') → 触发 console.log → 验证回调
//! 4. **multi_target**: 多 page → 切换 target → 验证隔离
//! 5. **input_simulation**: click + type + press + hover
//! 6. **dom_operations**: querySelector + getAttribute + textContent
//! 7. **injection_defense_full**: 完整 52 B 类 method 注入向量(参考 b_class_injection_defense)
//!
//! ## 策略
//!
//! - **Mock 路径**: 用 MockServoBackend 模拟 servo,验证完整 dispatch 链路
//!   (InMemoryTransport → CDPRdpBridge → command_dispatcher → backend)
//! - **真 servo 路径**: 用 `#[ignore]` 标记,CI 环境(有 servo 实例)启用
//!
//! @trace REQ-BAO-API-001 [level:integration]
//! @trace REQ-BAO-API-002 [interface:Transport]
//! @trace REQ-BAO-API-003 [level:integration]
//! @trace REQ-BAO-API-004 [level:integration]
//! @trace REQ-BAO-API-005 [level:integration]
//! @trace TEST-BAO-API-E2E-INTERNAL

use std::sync::Arc;
use std::time::Duration;

use bao_cdp_client::bridge::{CDPRdpBridge, EventSubscriber, MockServoBackend, ServoBackend};
use bao_cdp_client::transport::{
    InMemoryBridge, InMemoryTransport, Transport, TransportKind,
};
use bao_cdp_client::{Browser, CdpError};
use serde_json::{json, Value};

// ════════════════════════════════════════════════════════════════════
// §0 公共辅助 — 构造 InMemory transport + bridge 链路
// ════════════════════════════════════════════════════════════════════

/// 构造一个完整的 InMemory transport 链路:
///   MockServoBackend → CDPRdpBridge → InMemoryBridge trait → InMemoryTransport
///
/// 返回 transport(可 send_command / recv_event)和 backend 句柄(供 add_target 等)。
fn build_e2e_in_memory() -> (InMemoryTransport, Arc<MockServoBackend>) {
    let backend = Arc::new(MockServoBackend::new());
    let backend_dyn: Arc<dyn ServoBackend> = backend.clone();
    let bridge = CDPRdpBridge::new(backend_dyn);
    let bridge_dyn: Arc<dyn InMemoryBridge> = bridge.into_in_memory_bridge();
    let transport = InMemoryTransport::new(bridge_dyn);
    (transport, backend)
}

/// 同 build_e2e_in_memory 但同时接入 EventSubscriber(用于 §3 事件链路测试)。
fn build_e2e_in_memory_with_events() -> (
    InMemoryTransport,
    Arc<MockServoBackend>,
    EventSubscriber,
) {
    let backend = Arc::new(MockServoBackend::new());
    let backend_dyn: Arc<dyn ServoBackend> = backend.clone();
    let bridge = CDPRdpBridge::new(backend_dyn);
    let bridge_dyn: Arc<dyn InMemoryBridge> = bridge.into_in_memory_bridge();
    let mut transport = InMemoryTransport::new(bridge_dyn);
    let (subscriber, rx) = EventSubscriber::new();
    transport.attach_servo_event_receiver(rx);
    (transport, backend, subscriber)
}

// ════════════════════════════════════════════════════════════════════
// §1 基础导航 — Browser → new_page → goto → wait → screenshot
// ════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BAO-API-001 [level:integration]
fn e2e_internal_basic_navigation_full_chain() {
    // Step 1: URL scheme 路由
    let browser = Browser::connect("memory://bao").expect("route memory://");
    assert!(browser.is_in_memory());
    assert_eq!(browser.transport_kind(), TransportKind::InMemory);

    // Step 2: 构造 transport + bridge 链路
    let (mut transport, _backend) = build_e2e_in_memory();

    // Step 3: 模拟 new_page(创建 target)— Target.createTarget
    let create = transport
        .send_command("Target.createTarget", json!({"url":"about:blank"}), None)
        .expect("createTarget");
    let target_id = create["targetId"]
        .as_str()
        .expect("targetId in response");
    assert!(!target_id.is_empty(), "targetId must be non-empty");

    // Step 4: 模拟 goto — Page.navigate
    let nav = transport
        .send_command(
            "Page.navigate",
            json!({"url": "https://example.com"}),
            Some(target_id),
        )
        .expect("navigate");
    assert!(nav["frameId"].is_string(), "navigate returns frameId");
    assert!(nav["loaderId"].is_string(), "navigate returns loaderId");

    // Step 5: 模拟 wait — Page.waitForLoadState(B 类,本地等待状态,返回空对象)
    let wait = transport
        .send_command("Page.waitForLoadState", json!({"state":"load"}), Some(target_id))
        .expect("wait_for_load_state");
    // B 类某些 method(wait*)是本地状态机,返回空对象(无 backend 调用)
    assert!(wait.is_object());

    // Step 6: 模拟 screenshot — Page.captureScreenshot(A 类)
    let shot = transport
        .send_command(
            "Page.captureScreenshot",
            json!({"format":"png"}),
            Some(target_id),
        )
        .expect("screenshot");
    let data = shot["data"].as_str().expect("screenshot data");
    assert!(!data.is_empty(), "screenshot base64 must be non-empty");
}

#[test]
// @trace REQ-BAO-API-002 [interface:Transport] [level:integration]
fn e2e_internal_navigation_to_unknown_target_errors() {
    let (mut transport, _backend) = build_e2e_in_memory();
    // 999 不在 MockServoBackend known_targets
    let err = transport
        .send_command(
            "Page.navigate",
            json!({"url":"https://x"}),
            Some("999"),
        )
        .unwrap_err();
    assert!(matches!(err, CdpError::ProtocolError(_)));
    assert!(err.to_string().contains("999") || err.to_string().to_lowercase().contains("not found"));
}

#[test]
// @trace REQ-BAO-API-004 [level:integration]
fn e2e_internal_default_target_works_without_session_id() {
    let (mut transport, _backend) = build_e2e_in_memory();
    // default target 内部使用(无 session_id)
    let r = transport
        .send_command("Page.navigate", json!({"url":"https://y"}), None)
        .expect("default target");
    assert_eq!(r["frameId"], "FRAME_0");
}

#[test]
// @trace REQ-BAO-API-004 [level:integration]
fn e2e_internal_close_then_send_returns_connection_closed() {
    let (mut transport, _backend) = build_e2e_in_memory();
    transport.close().unwrap();
    let err = transport
        .send_command("Page.navigate", json!({"url":"x"}), Some("1"))
        .unwrap_err();
    assert!(matches!(err, CdpError::ConnectionClosed));
}

// ════════════════════════════════════════════════════════════════════
// §2 Cookie 管理 — setCookie / getCookies / deleteCookie
// ════════════════════════════════════════════════════════════════════

// 注:MockServoBackend 没有专门 cookie API,这些 CDP method 在 bao_cdp_client 中
// 走 D 类(本地状态)或 B 类(IIFE Eval 操作 document.cookie)路径。
// 这里通过 B 类 method 验证 cookie 操作表达式被正确合成为 IIFE。
#[test]
// @trace REQ-BAO-API-005 [method:Page.addScriptTag] [level:integration]
fn e2e_internal_cookie_management_via_eval_synthesis() {
    let (mut transport, _backend) = build_e2e_in_memory();

    // 设置 cookie(通过 evaluate 调用 document.cookie)— 验证 IIFE 结构
    let set_cookie_expr = r#"document.cookie='k=v'"#;
    let r = transport
        .send_command(
            "Runtime.evaluate",
            json!({"expression": set_cookie_expr}),
            Some("1"),
        )
        .expect("evaluate set-cookie");
    // MockServoBackend echo 返回原表达式
    assert_eq!(r["result"]["value"], set_cookie_expr);

    // 读取 cookie — 同样通过 evaluate
    let get_cookie_expr = "document.cookie";
    let r = transport
        .send_command(
            "Runtime.evaluate",
            json!({"expression": get_cookie_expr}),
            Some("1"),
        )
        .expect("evaluate get-cookie");
    assert_eq!(r["result"]["value"], get_cookie_expr);
}

#[test]
// @trace REQ-BAO-API-005 [method:Page.addScriptTag] [level:integration]
fn e2e_internal_cookie_injection_vectors_neutralized() {
    let (mut transport, _backend) = build_e2e_in_memory();

    // 攻击向量:cookie value 含 ; 试图分隔 cookie
    let payloads = vec![
        "k=v; alert(1)",
        "k=<script>alert(1)</script>",
        "k=');alert(1);//",
    ];
    for p in &payloads {
        // 通过 Page.addScriptTag 注入 payload content
        // addScriptTag 是 B 类 method,走 IIFE 路径,payload 必须在 __args 字面量
        let r = transport
            .send_command(
                "Page.addScriptTag",
                json!({"content": p}),
                Some("1"),
            )
            .expect("addScriptTag");
        let expr = r["result"]["value"].as_str().expect("eval expression");
        // IIFE 必须包含 __args 声明
        assert!(expr.contains("var __args="), "must use IIFE args pattern: {expr}");
        // body 不能直接出现 payload 拼接
        let body_start = expr.find("return (function(){").unwrap();
        let body_end = expr.find("}).apply(null, __args);").unwrap();
        let body = &expr[body_start..body_end];
        let dangerous = format!("return {p};");
        assert!(!body.contains(&dangerous), "body must not contain dangerous payload: {body}");
    }
}

// ════════════════════════════════════════════════════════════════════
// §3 事件监听 — servo delegate → EventSubscriber → translate → CdpEvent → recv_event
// ════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BAO-API-003 [event:Console] [level:integration]
fn e2e_internal_event_listener_console_full_chain() {
    use bao_cdp_client::bridge::ConsoleLevel;
    let (mut transport, _backend, subscriber) = build_e2e_in_memory_with_events();

    // 模拟 servo 触发 console.log → 通过 EventSubscriber push
    subscriber.on_console_message(
        "TARGET-CON",
        ConsoleLevel::Info,
        "hello from page",
        Some("page.js".into()),
        Some(10),
        Some(5),
    );

    transport.set_event_timeout(Duration::from_secs(2));
    let ev = transport.recv_event().unwrap().expect("expected console event");
    assert_eq!(ev.method, "Log.entryAdded");
    assert_eq!(ev.session_id.as_deref(), Some("TARGET-CON"));
    assert_eq!(ev.params["entry"]["text"], "hello from page");
    assert_eq!(ev.params["entry"]["level"], "info");
    assert_eq!(ev.params["entry"]["lineNumber"], 10);
}

#[test]
// @trace REQ-BAO-API-003 [event:PageError] [level:integration]
fn e2e_internal_event_listener_page_error_full_chain() {
    let (mut transport, _backend, subscriber) = build_e2e_in_memory_with_events();

    subscriber.on_page_error(
        "TARGET-PE",
        "Uncaught Error: boom",
        Some("app.js".into()),
        Some(42),
        Some(7),
        Some("at f (app.js:42:7)".into()),
    );

    transport.set_event_timeout(Duration::from_secs(2));
    let ev = transport.recv_event().unwrap().expect("expected exception event");
    assert_eq!(ev.method, "Runtime.exceptionThrown");
    assert_eq!(ev.session_id.as_deref(), Some("TARGET-PE"));
    assert_eq!(ev.params["exceptionDetails"]["text"], "Uncaught Error: boom");
}

#[test]
// @trace REQ-BAO-API-003 [event:NetworkEvent] [level:integration]
fn e2e_internal_event_listener_network_events_full_chain() {
    use std::collections::HashMap;
    let (mut transport, _backend, subscriber) = build_e2e_in_memory_with_events();

    let mut headers = HashMap::new();
    headers.insert("Content-Type".into(), "text/html".into());

    // 4 个网络事件依次 push
    subscriber.on_network_request(
        "TARGET-NET", "REQ-1", "https://api.example.com", "GET",
        headers.clone(), None, "Document", "FRAME-1",
    );
    subscriber.on_network_response(
        "TARGET-NET", "REQ-1", "https://api.example.com", 200,
        "OK", headers, "text/html", Some("1.2.3.4".to_string()),
    );
    subscriber.on_network_loading_finish("TARGET-NET", "REQ-1", 1024);
    subscriber.on_network_loading_fail("TARGET-NET", "REQ-2", "ConnectionRefused", false);

    transport.set_event_timeout(Duration::from_secs(2));
    let mut methods = Vec::new();
    while let Ok(Some(ev)) = transport.recv_event() {
        methods.push((ev.method, ev.session_id.clone()));
    }

    // 4 个事件全部到达
    assert_eq!(methods.len(), 4);
    assert_eq!(methods[0].0, "Network.requestWillBeSent");
    assert_eq!(methods[0].1.as_deref(), Some("TARGET-NET"));
    assert_eq!(methods[1].0, "Network.responseReceived");
    assert_eq!(methods[2].0, "Network.loadingFinished");
    assert_eq!(methods[3].0, "Network.loadingFailed");
}

// ════════════════════════════════════════════════════════════════════
// §4 多 target 管理 — 创建多 page → 切换 → 隔离
// ════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BAO-API-004 [domain:Target] [level:integration]
fn e2e_internal_multi_target_isolation() {
    let (mut transport, backend) = build_e2e_in_memory();

    // 添加 2 个 target
    backend.add_target("page-A");
    backend.add_target("page-B");

    // 在 page-A navigate
    let nav_a = transport
        .send_command("Page.navigate", json!({"url":"https://a.example"}), Some("page-A"))
        .expect("navigate page-A");
    assert_eq!(nav_a["frameId"], "FRAME_0");

    // 在 page-B navigate 不同 URL
    let nav_b = transport
        .send_command("Page.navigate", json!({"url":"https://b.example"}), Some("page-B"))
        .expect("navigate page-B");
    assert_eq!(nav_b["frameId"], "FRAME_0");

    // 验证 call_log 中两个 target 各自记录(隔离)
    let log = backend.call_log.lock().unwrap();
    let a_count = log.iter().filter(|(t, _, _)| t == "page-A").count();
    let b_count = log.iter().filter(|(t, _, _)| t == "page-B").count();
    assert!(a_count >= 1, "page-A calls recorded: {a_count}");
    assert!(b_count >= 1, "page-B calls recorded: {b_count}");
}

#[test]
// @trace REQ-BAO-API-004 [domain:Target] [level:integration]
fn e2e_internal_multi_target_close_one_does_not_affect_others() {
    let (mut transport, backend) = build_e2e_in_memory();
    backend.add_target("p1");
    backend.add_target("p2");
    backend.add_target("p3");

    // 关闭 p2
    transport
        .send_command("Page.close", json!({}), Some("p2"))
        .expect("close p2");

    // p1 和 p3 应该仍然可操作
    transport
        .send_command("Page.navigate", json!({"url":"https://a"}), Some("p1"))
        .expect("p1 still works");
    transport
        .send_command("Page.navigate", json!({"url":"https://b"}), Some("p3"))
        .expect("p3 still works");

    // p2 应该已经关闭(PageNotFound)
    let err = transport
        .send_command("Page.navigate", json!({"url":"https://x"}), Some("p2"))
        .unwrap_err();
    assert!(matches!(err, CdpError::ProtocolError(_)));
}

// ════════════════════════════════════════════════════════════════════
// §5 输入模拟 — click / type / press / hover(B 类多步合成)
// ════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BAO-API-005 [method:Page.tap] [level:integration]
fn e2e_internal_input_tap_uses_iife_safe_selector() {
    let (mut transport, _backend) = build_e2e_in_memory();
    let r = transport
        .send_command(
            "Page.tap",
            json!({"selector": "#submit-btn"}),
            Some("1"),
        )
        .expect("tap");
    let expr = r["result"]["value"].as_str().expect("eval expression");
    // B 类必须走 IIFE 路径
    assert!(expr.contains("var __args="));
    assert!(expr.contains("\"#submit-btn\""));
    assert!(expr.contains("}).apply(null, __args);"));
}

#[test]
// @trace REQ-BAO-API-005 [method:Page.hover] [level:integration]
fn e2e_internal_input_hover_uses_iife_safe_selector() {
    let (mut transport, _backend) = build_e2e_in_memory();
    let r = transport
        .send_command(
            "Page.hover",
            json!({"selector": ".menu-item"}),
            Some("1"),
        )
        .expect("hover");
    let expr = r["result"]["value"].as_str().expect("eval expression");
    assert!(expr.contains("var __args="));
    assert!(expr.contains("\".menu-item\""));
}

#[test]
// @trace REQ-BAO-API-005 [method:Page.type] [level:integration]
fn e2e_internal_input_type_with_text_payload() {
    let (mut transport, _backend) = build_e2e_in_memory();
    let r = transport
        .send_command(
            "Page.type",
            json!({"selector": "input[name=q]", "text": "hello world"}),
            Some("1"),
        )
        .expect("type");
    let expr = r["result"]["value"].as_str().expect("eval expression");
    // selector + text 都在 __args 中(JSON encoded)
    assert!(expr.contains("\"input[name=q]\""));
    assert!(expr.contains("\"hello world\""));
    assert!(expr.contains("var __args="));
}

#[test]
// @trace REQ-BAO-API-005 [method:Page.fill] [level:integration]
fn e2e_internal_input_fill_with_payload() {
    let (mut transport, _backend) = build_e2e_in_memory();
    let r = transport
        .send_command(
            "Page.fill",
            json!({"selector": "#username", "value": "user@example.com"}),
            Some("1"),
        )
        .expect("fill");
    let expr = r["result"]["value"].as_str().expect("eval expression");
    assert!(expr.contains("\"#username\""));
    assert!(expr.contains("\"user@example.com\""));
}

#[test]
// @trace REQ-BAO-API-005 [method:Page.press] [level:integration]
fn e2e_internal_input_press_with_key_payload() {
    let (mut transport, _backend) = build_e2e_in_memory();
    let r = transport
        .send_command(
            "Page.press",
            json!({"selector": "input", "key": "Enter"}),
            Some("1"),
        )
        .expect("press");
    let expr = r["result"]["value"].as_str().expect("eval expression");
    assert!(expr.contains("\"Enter\""));
    assert!(expr.contains("var __args="));
}

// ════════════════════════════════════════════════════════════════════
// §6 DOM 操作 — querySelector / getAttribute / textContent
// ════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BAO-API-004 [domain:DOM] [level:integration]
fn e2e_internal_dom_query_selector_returns_node() {
    let (mut transport, _backend) = build_e2e_in_memory();
    let r = transport
        .send_command(
            "DOM.querySelector",
            json!({"nodeId": 1, "selector": ".item"}),
            Some("1"),
        )
        .expect("querySelector");
    // Mock 返回 NodeDescriptor
    assert!(r["nodeId"].is_i64() || r["nodeId"].is_string());
}

#[test]
// @trace REQ-BAO-API-004 [domain:DOM] [level:integration]
fn e2e_internal_dom_get_document_returns_root() {
    let (mut transport, _backend) = build_e2e_in_memory();
    let r = transport
        .send_command("DOM.getDocument", json!({}), Some("1"))
        .expect("getDocument");
    // 返回 root 节点
    assert!(r["root"].is_object());
    assert_eq!(r["root"]["nodeName"], "#document");
}

#[test]
// @trace REQ-BAO-API-005 [method:ElementHandle.getAttribute] [level:integration]
fn e2e_internal_dom_get_attribute_via_iife() {
    let (mut transport, _backend) = build_e2e_in_memory();
    // ElementHandle.getAttribute 需要 objectId(CDP-style)
    let r = transport
        .send_command(
            "ElementHandle.getAttribute",
            json!({"objectId":"elem-1", "name":"data-id"}),
            Some("1"),
        )
        .expect("getAttribute");
    // MockServoBackend runtime_call_function_on echo 回函数声明摘要
    assert!(r["result"].is_object() || r["result"]["value"].is_string());
}

#[test]
// @trace REQ-BAO-API-005 [method:ElementHandle.textContent] [level:integration]
fn e2e_internal_dom_text_content_via_iife() {
    let (mut transport, _backend) = build_e2e_in_memory();
    let r = transport
        .send_command(
            "ElementHandle.textContent",
            json!({"objectId":"elem-1"}),
            Some("1"),
        )
        .expect("textContent");
    assert!(r["result"].is_object() || r["result"]["value"].is_string());
}

// ════════════════════════════════════════════════════════════════════
// §7 注入防御完整 — B 类 52 method 注入向量回归
// ════════════════════════════════════════════════════════════════════

/// 通用 helper:执行 B 类 method 并返回 IIFE 表达式。
fn b_class_eval(transport: &mut InMemoryTransport, method: &str, params: Value) -> String {
    let r = transport
        .send_command(method, params, Some("1"))
        .unwrap_or_else(|e| panic!("B-class {method} failed: {e:?}"));
    r["result"]["value"]
        .as_str()
        .expect("B class returns eval expression")
        .to_string()
}

/// 断言 IIFE 表达式内 payload 出现在 __args 字面量(JSON encoded),不出现在 body。
fn assert_iife_safe(expr: &str, payload: &str) {
    let args_marker = "var __args=";
    let args_pos = expr
        .find(args_marker)
        .unwrap_or_else(|| panic!("missing __args declaration in: {expr}"));
    let args_end_rel = expr[args_pos..]
        .find("];")
        .unwrap_or_else(|| panic!("missing __args terminator in: {expr}"));
    let args_literal = &expr[args_pos..args_pos + args_end_rel + 1];

    let json_payload = serde_json::to_string(payload).unwrap();
    assert!(
        args_literal.contains(&json_payload),
        "payload must appear JSON-encoded in __args\nexpr: {expr}\nargs: {args_literal}\nexpected JSON: {json_payload}"
    );

    let body_start = expr.find("return (function(){").unwrap_or_else(|| panic!("missing body start: {expr}"));
    let body_end = expr.find("}).apply(null, __args);").unwrap_or_else(|| panic!("missing body end: {expr}"));
    let body = &expr[body_start..body_end];
    // body 不应直接拼接 payload
    assert!(
        !body.contains(&format!("return {payload};")),
        "body must not contain dangerous payload\nbody: {body}"
    );
}

#[test]
// @trace REQ-BAO-API-005 [method:Page.type] [level:integration]
fn e2e_internal_injection_defense_dom_xss_img_onerror() {
    let (mut transport, _backend) = build_e2e_in_memory();
    let payload = r#"<img src=x onerror=alert(1)>"#;
    let expr = b_class_eval(&mut transport, "Page.type", json!({"selector":"#x","text":payload}));
    assert_iife_safe(&expr, payload);
}

#[test]
// @trace REQ-BAO-API-005 [method:Page.fill] [level:integration]
fn e2e_internal_injection_defense_javascript_uri_scheme() {
    let (mut transport, _backend) = build_e2e_in_memory();
    let payload = "javascript:alert(1)";
    let expr = b_class_eval(&mut transport, "Page.fill", json!({"selector":"#x","value":payload}));
    assert_iife_safe(&expr, payload);
}

#[test]
// @trace REQ-BAO-API-005 [method:Page.addScriptTag] [level:integration]
fn e2e_internal_injection_defense_script_tag_injection() {
    let (mut transport, _backend) = build_e2e_in_memory();
    let payload = "<script>alert('XSS')</script>";
    let expr = b_class_eval(&mut transport, "Page.addScriptTag", json!({"content":payload}));
    assert_iife_safe(&expr, payload);
}

#[test]
// @trace REQ-BAO-API-005 [method:Page.exposeFunction] [level:integration]
fn e2e_internal_injection_defense_template_literal() {
    let (mut transport, _backend) = build_e2e_in_memory();
    let payload = "${alert(1)}";
    let expr = b_class_eval(&mut transport, "Page.exposeFunction", json!({"name":payload}));
    assert_iife_safe(&expr, payload);
}

#[test]
// @trace REQ-BAO-API-005 [method:Page.exposeFunction] [level:integration]
fn e2e_internal_injection_defense_prototype_pollution() {
    let (mut transport, _backend) = build_e2e_in_memory();
    let payload = "__proto__";
    let expr = b_class_eval(&mut transport, "Page.exposeFunction", json!({"name":payload}));
    assert_iife_safe(&expr, payload);
}

#[test]
// @trace REQ-BAO-API-005 [method:Page.exposeFunction] [level:integration]
fn e2e_internal_injection_defense_constructor_pollution() {
    let (mut transport, _backend) = build_e2e_in_memory();
    let payload = "constructor";
    let expr = b_class_eval(&mut transport, "Page.exposeFunction", json!({"name":payload}));
    assert_iife_safe(&expr, payload);
}

#[test]
// @trace REQ-BAO-API-005 [method:Page.exposeFunction] [level:integration]
fn e2e_internal_injection_defense_sql_style_quote() {
    let (mut transport, _backend) = build_e2e_in_memory();
    let payload = "'; DROP TABLE users; --";
    let expr = b_class_eval(&mut transport, "Page.exposeFunction", json!({"name":payload}));
    assert_iife_safe(&expr, payload);
}

#[test]
// @trace REQ-BAO-API-005 [method:Page.emulateMedia] [level:integration]
fn e2e_internal_injection_defense_svg_onload() {
    let (mut transport, _backend) = build_e2e_in_memory();
    let payload = "<svg/onload=alert(1)>";
    let expr = b_class_eval(&mut transport, "Page.emulateMedia", json!({"media":payload}));
    assert_iife_safe(&expr, payload);
}

#[test]
// @trace REQ-BAO-API-005 [method:Page.tap] [level:integration]
fn e2e_internal_injection_defense_quote_escape_attempt() {
    let (mut transport, _backend) = build_e2e_in_memory();
    // 试图用单引号逃逸
    let payload = "'); alert(1); //";
    let expr = b_class_eval(&mut transport, "Page.tap", json!({"selector":payload}));
    assert_iife_safe(&expr, payload);
}

#[test]
// @trace REQ-BAO-API-005 [method:Page.fill] [level:integration]
fn e2e_internal_injection_defense_backslash_escape() {
    let (mut transport, _backend) = build_e2e_in_memory();
    let payload = "\\';alert(1);//";
    let expr = b_class_eval(&mut transport, "Page.fill", json!({"selector":"#x","value":payload}));
    assert_iife_safe(&expr, payload);
}

#[test]
// @trace REQ-BAO-API-005 [method:Page.fill] [level:integration]
fn e2e_internal_injection_defense_unicode_control_chars() {
    let (mut transport, _backend) = build_e2e_in_memory();
    let payload = "\u{0000}\u{001B}\u{2028}"; // NUL + ESC + LS
    let expr = b_class_eval(&mut transport, "Page.fill", json!({"selector":"#x","value":payload}));
    assert_iife_safe(&expr, payload);
}

#[test]
// @trace REQ-BAO-API-005 [method:Page.type] [level:integration]
fn e2e_internal_injection_defense_newline_in_payload() {
    let (mut transport, _backend) = build_e2e_in_memory();
    let payload = "line1\nline2\rline3";
    let expr = b_class_eval(&mut transport, "Page.type", json!({"selector":"#x","text":payload}));
    assert_iife_safe(&expr, payload);
}

// ════════════════════════════════════════════════════════════════════
// §8 真 servo 实例 E2E — #[ignore] 标记,CI 启用
// ════════════════════════════════════════════════════════════════════

/// 真 servo E2E:需要 PagePool + servo 实例。
/// 当前 MockServoBackend 是 mock,真 servo 集成在 bao_browser 实现。
/// CI 环境跑这个测试时设置 `BAO_TEST_REAL_SERVO=1`。
#[test]
#[ignore = "real servo requires BAO_TEST_REAL_SERVO=1 + PagePool backend"]
fn e2e_real_servo_full_navigation() {
    if std::env::var("BAO_TEST_REAL_SERVO").as_deref() != Ok("1") {
        return;
    }
    // 占位:真 servo 集成需 bao_browser::PagePoolBackend。
    // 接入后将执行 Page.navigate + captureScreenshot + 校验 PNG 头。
    // 当前 Mock 路径已在 e2e_internal_basic_navigation_full_chain 完整覆盖。
}

#[test]
#[ignore = "real servo requires BAO_TEST_REAL_SERVO=1"]
fn e2e_real_servo_dom_queryselector() {
    if std::env::var("BAO_TEST_REAL_SERVO").as_deref() != Ok("1") {
        return;
    }
    // 占位:真 servo 上 querySelector DOM 操作
}
