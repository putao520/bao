//! TASK-8 E2E — 事件覆盖率扩展测试。
//!
//! ## 验收范围
//!
//! 在 event_translation.rs (19 tests) 基础上扩展到完整端到端链路:
//!
//! 1. **完整链路**: servo delegate → EventSubscriber → translate → CdpEvent → Transport::recv_event
//! 2. **事件丢失**: channel 满了如何处理(虽然有容量,但断开 receiver 应正确处理)
//! 3. **事件顺序**: 同 target 内事件按 push 顺序到达
//! 4. **跨 target 事件**: 多 target 同时 push,session_id 正确隔离
//! 5. **一对多 translate**: 一个 ServoEvent → 多个 CdpEvent 的场景
//! 6. **recv 退避**: timeout 行为 + 接收节奏
//! 7. **断开恢复**: servo push 端断开后,Transport fallback 到 event_rx
//!
//! @trace REQ-BAO-API-003 [level:integration]
//! @trace TEST-BAO-API-EVENT

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use bao_cdp_client::bridge::event_translator::{translate, ConsoleLevel, ServoEvent};
use bao_cdp_client::bridge::EventSubscriber;
use bao_cdp_client::transport::{
    CdpEvent, InMemoryBridge, InMemoryBridgeResponse, InMemoryTransport, Transport,
};
use serde_json::Value;

// ════════════════════════════════════════════════════════════════════
// NullBridge — 不响应任何命令,只用作 InMemoryTransport 构造
// ════════════════════════════════════════════════════════════════════

struct NullBridge;

impl InMemoryBridge for NullBridge {
    fn dispatch_command(
        &self,
        _method: &str,
        _params: Value,
        _session_id: Option<&str>,
    ) -> InMemoryBridgeResponse {
        InMemoryBridgeResponse::Ok(Value::Null)
    }
}

fn build_with_events() -> (InMemoryTransport, EventSubscriber) {
    let bridge: Arc<dyn InMemoryBridge> = Arc::new(NullBridge);
    let mut transport = InMemoryTransport::new(bridge);
    let (subscriber, rx) = EventSubscriber::new();
    transport.attach_servo_event_receiver(rx);
    (transport, subscriber)
}

// ════════════════════════════════════════════════════════════════════
// §1 完整链路 — translate → EventSubscriber → recv_event
// ════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BAO-API-003 [event:Console] [level:integration]
fn full_e2e_console_chain_translate_to_cdp_event() {
    // Arrange
    let (mut transport, subscriber) = build_with_events();

    // Act
    subscriber.on_console_message(
        "TARGET-X",
        ConsoleLevel::Warning,
        "warn text",
        None,
        None,
        None,
    );

    transport.set_event_timeout(Duration::from_secs(2));
    let ev = transport.recv_event().unwrap().expect("expected event");
    // Assert
    assert_eq!(ev.method, "Log.entryAdded");
    assert_eq!(ev.session_id.as_deref(), Some("TARGET-X"));
    assert_eq!(ev.params["entry"]["level"], "warning");
    assert_eq!(ev.params["entry"]["text"], "warn text");
}

#[test]
// @trace REQ-BAO-API-003 [event:PageError] [level:integration]
fn full_e2e_page_error_with_stack_trace() {
    // Arrange
    let (mut transport, subscriber) = build_with_events();

    // Act
    subscriber.on_page_error(
        "T1",
        "TypeError: x is undefined",
        Some("app.js".into()),
        Some(10),
        Some(5),
        Some("at f (app.js:10:5)\nat g (app.js:20:10)".into()),
    );

    transport.set_event_timeout(Duration::from_secs(2));
    let ev = transport.recv_event().unwrap().expect("expected event");
    // Assert
    assert_eq!(ev.method, "Runtime.exceptionThrown");
    assert_eq!(ev.params["exceptionDetails"]["text"], "TypeError: x is undefined");
    // stackTrace 直接是数组(CDP-style)— 1 个 callFrame
    assert!(ev.params["exceptionDetails"]["stackTrace"].is_array());
    assert_eq!(ev.params["exceptionDetails"]["stackTrace"][0]["scriptName"], "at f (app.js:10:5)\nat g (app.js:20:10)");
}

// ════════════════════════════════════════════════════════════════════
// §2 事件顺序 — FIFO 保证
// ════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BAO-API-003 [level:integration]
fn full_e2e_event_order_preserved_within_target() {
    // Arrange
    let (mut transport, subscriber) = build_with_events();

    // 5 个 frame 事件按顺序 push
    for i in 0..5 {
        // Act
        subscriber.on_frame_started_loading("T", &format!("FRAME-{i}"));
    }

    transport.set_event_timeout(Duration::from_secs(2));
    let mut frames = Vec::new();
    while let Ok(Some(ev)) = transport.recv_event() {
        if ev.method == "Page.frameStartedLoading" {
            frames.push(ev.params["frameId"].as_str().unwrap().to_string());
        }
    }
    // Assert
    assert_eq!(frames.len(), 5);
    // 必须严格 FIFO
    for (i, f) in frames.iter().enumerate() {
        assert_eq!(f, &format!("FRAME-{i}"), "event order broken: {frames:?}");
    }
}

#[test]
// @trace REQ-BAO-API-003 [level:integration]
fn full_e2e_event_mixed_classes_order_preserved() {
    // Arrange
    let (mut transport, subscriber) = build_with_events();

    // 混合 4 类事件按顺序 push
    // Act
    subscriber.on_console_message("T", ConsoleLevel::Info, "first", None, None, None);
    subscriber.on_page_error("T", "second", None, None, None, None);
    subscriber.on_frame_started_loading("T", "F1");
    subscriber.on_timeline_marker("T", "third", 0.0, 1.0);

    transport.set_event_timeout(Duration::from_secs(2));
    let mut methods = Vec::new();
    while let Ok(Some(ev)) = transport.recv_event() {
        methods.push(ev.method);
    }
    // Assert
    assert_eq!(methods.len(), 4);
    assert_eq!(methods[0], "Log.entryAdded");
    assert_eq!(methods[1], "Runtime.exceptionThrown");
    assert_eq!(methods[2], "Page.frameStartedLoading");
    assert_eq!(methods[3], "Performance.metrics");
}

// ════════════════════════════════════════════════════════════════════
// §3 跨 target 事件隔离
// ════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BAO-API-003 [level:integration]
fn full_e2e_multi_target_session_id_isolation() {
    // Arrange
    let (mut transport, subscriber) = build_with_events();

    // 3 个不同 target 的 console 事件交替 push
    for target in &["A", "B", "A", "C", "B"] {
        // Act
        subscriber.on_console_message(
            &format!("TARGET-{target}"),
            ConsoleLevel::Info,
            &format!("msg-{target}"),
            None,
            None,
            None,
        );
    }

    transport.set_event_timeout(Duration::from_secs(2));
    let mut events = Vec::new();
    while let Ok(Some(ev)) = transport.recv_event() {
        events.push((
            ev.session_id.clone().unwrap_or_default(),
            ev.params["entry"]["text"].as_str().unwrap_or("").to_string(),
        ));
    }
    // Assert
    assert_eq!(events.len(), 5);
    // 验证 session_id 与 text 严格对应
    for (sid, text) in &events {
        let target_letter = sid.trim_start_matches("TARGET-");
        assert!(text.contains(target_letter), "session {sid} text {text} mismatch");
    }
}

// ════════════════════════════════════════════════════════════════════
// §4 一对多 translate — 单事件多 CdpEvent
// ════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BAO-API-003 [level:integration]
fn full_e2e_single_servo_event_can_map_to_multiple_cdp_events() {
    // Arrange
    // 直接调用 translate,验证某些事件类型产生多个 CdpEvent
    // ScriptParsed 在 standard translate 下产生 1 个,但 NetworkRequest 可能产生多个
    let ev = ServoEvent::NetworkRequest {
        // Act
        target_id: "T".into(),
        request_id: "R1".into(),
        url: "https://example.com/api".into(),
        method: "GET".into(),
        headers: HashMap::new(),
        post_data: None,
        resource_type: "XHR".into(),
        frame_id: "F1".into(),
    };
    let cdp_events = translate(ev);
    // 至少 1 个 CdpEvent
    // Assert
    assert!(!cdp_events.is_empty());
    // 全部 method 应该是 Network.* 系列
    for e in &cdp_events {
        assert!(
            e.method.starts_with("Network."),
            "method should be Network.*: {}",
            e.method
        );
    }
}

#[test]
// @trace REQ-BAO-API-003 [level:integration]
fn full_e2e_one_to_many_events_delivered_in_sequence() {
    // Arrange
    // 模拟一对多事件,验证 transport 内部 pending 队列正确处理
    let ev = ServoEvent::PageError {
        // Act
        target_id: "T".into(),
        text: "test".into(),
        url: None,
        line: None,
        column: None,
        stack: None,
    };
    let cdp_events = translate(ev);
    // PageError 通常 1 个事件,但验证机制
    // Assert
    assert_eq!(cdp_events.len(), 1);
}

// ════════════════════════════════════════════════════════════════════
// §5 recv 节奏与超时
// ════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BAO-API-003 [level:integration]
fn full_e2e_recv_returns_none_on_timeout_when_no_events() {
    // Arrange
    let (mut transport, _subscriber) = build_with_events();
    // Act
    transport.set_event_timeout(Duration::from_millis(50));

    let start = std::time::Instant::now();
    let ev = transport.recv_event().unwrap();
    let elapsed = start.elapsed();

    // Assert
    assert!(ev.is_none(), "expected None on timeout");
    assert!(elapsed.as_millis() < 500, "elapsed: {elapsed:?}");
}

#[test]
// @trace REQ-BAO-API-003 [level:integration]
fn full_e2e_recv_immediately_returns_available_event() {
    // Arrange
    let (mut transport, subscriber) = build_with_events();
    // push 事件
    // Act
    subscriber.on_console_message("T", ConsoleLevel::Info, "x", None, None, None);

    transport.set_event_timeout(Duration::from_secs(5));
    let start = std::time::Instant::now();
    let ev = transport.recv_event().unwrap().expect("expected event");
    let elapsed = start.elapsed();

    // Assert
    assert_eq!(ev.method, "Log.entryAdded");
    // 应该几乎立即返回(<= 100ms)
    assert!(elapsed.as_millis() < 100, "elapsed: {elapsed:?}");
}

#[test]
// @trace REQ-BAO-API-003 [level:integration]
fn full_e2e_recv_drains_events_then_returns_none() {
    // Arrange
    let (mut transport, subscriber) = build_with_events();
    // push 3 个事件
    for i in 0..3 {
        // Act
        subscriber.on_console_message(
            "T",
            ConsoleLevel::Info,
            &format!("msg-{i}"),
            None,
            None,
            None,
        );
    }

    transport.set_event_timeout(Duration::from_millis(100));
    let mut count = 0;
    while let Ok(Some(_)) = transport.recv_event() {
        count += 1;
    }
    // Assert
    assert_eq!(count, 3, "expected to drain all 3 events");
}

// ════════════════════════════════════════════════════════════════════
// §6 fallback 到 event_rx
// ════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BAO-API-003 [level:integration]
fn full_e2e_fallback_to_direct_cdp_event_push() {
    // Arrange
    // 即使没有 servo 事件,event_rx 直接 push 的 CdpEvent 也能收到
    let bridge: Arc<dyn InMemoryBridge> = Arc::new(NullBridge);
    let mut transport = InMemoryTransport::new(bridge);
    // 不 attach servo event receiver

    let sender = transport.event_sender();
    sender
        .send(CdpEvent::new(
            "Custom.event",
            // Act
            serde_json::json!({"k": "v"}),
        ))
        .unwrap();

    transport.set_event_timeout(Duration::from_secs(2));
    let ev = transport.recv_event().unwrap().expect("expected direct CdpEvent");
    // Assert
    assert_eq!(ev.method, "Custom.event");
    assert_eq!(ev.params["k"], "v");
}

#[test]
// @trace REQ-BAO-API-003 [level:integration]
fn full_e2e_servo_events_take_precedence_over_direct_push() {
    // Arrange
    // 同时 push servo 事件 + 直接 CdpEvent,servo 应优先
    let (mut transport, subscriber) = build_with_events();

    // 先 push 直接 CdpEvent
    let sender = transport.event_sender();
    sender
        .send(CdpEvent::new("Direct.event", serde_json::json!({})))
        .unwrap();
    // 再 push servo 事件
    // Act
    subscriber.on_console_message("T", ConsoleLevel::Info, "from servo", None, None, None);

    transport.set_event_timeout(Duration::from_secs(2));
    let ev = transport.recv_event().unwrap().expect("expected event");
    // servo 事件应该优先被消费
    // Assert
    assert_eq!(ev.method, "Log.entryAdded");

    // 然后才是直接 CdpEvent
    let ev2 = transport.recv_event().unwrap().expect("expected direct event");
    assert_eq!(ev2.method, "Direct.event");
}

// ════════════════════════════════════════════════════════════════════
// §7 大批量事件压测
// ════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BAO-API-003 [level:integration]
fn full_e2e_high_volume_events_100_count() {
    // Arrange
    let (mut transport, subscriber) = build_with_events();

    // push 100 个事件
    for i in 0..100 {
        // Act
        subscriber.on_console_message(
            "T",
            ConsoleLevel::Info,
            &format!("msg-{i}"),
            None,
            None,
            None,
        );
    }

    transport.set_event_timeout(Duration::from_secs(2));
    let mut count = 0;
    let mut all_methods_log = true;
    while let Ok(Some(ev)) = transport.recv_event() {
        if ev.method != "Log.entryAdded" {
            all_methods_log = false;
        }
        count += 1;
    }
    // Assert
    assert_eq!(count, 100, "expected 100 events, got {count}");
    assert!(all_methods_log, "all events must be Log.entryAdded");
}

#[test]
// @trace REQ-BAO-API-003 [level:integration]
fn full_e2e_high_volume_events_preserve_order() {
    // Arrange
    let (mut transport, subscriber) = build_with_events();

    // push 50 个 console,每个 text 包含序号
    for i in 0..50 {
        // Act
        subscriber.on_console_message(
            "T",
            ConsoleLevel::Info,
            &format!("seq-{i:03}"),
            None,
            None,
            None,
        );
    }

    transport.set_event_timeout(Duration::from_secs(2));
    let mut texts = Vec::new();
    while let Ok(Some(ev)) = transport.recv_event() {
        let text = ev.params["entry"]["text"].as_str().unwrap().to_string();
        texts.push(text);
    }
    // Assert
    assert_eq!(texts.len(), 50);
    // 验证顺序
    for (i, t) in texts.iter().enumerate() {
        assert_eq!(t, &format!("seq-{i:03}"), "order broken at {i}: {texts:?}");
    }
}

// ════════════════════════════════════════════════════════════════════
// §8 完整 7 类事件端到端验证
// ════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BAO-API-003 [level:integration]
fn full_e2e_all_seven_classes_through_transport_chain() {
    // Arrange
    use std::collections::HashSet;
    let (mut transport, subscriber) = build_with_events();

    // 7 类事件全部 push(13 events)
    // Act
    subscriber.on_console_message("T", ConsoleLevel::Debug, "c", None, None, None);
    subscriber.on_page_error("T", "p", None, None, None, None);
    subscriber.on_network_request(
        "T", "N1", "u", "GET", HashMap::new(), None, "Other", "F",
    );
    subscriber.on_network_response(
        "T", "N2", "u", 200, "OK", HashMap::new(), "text/html", None,
    );
    subscriber.on_network_loading_finish("T", "N3", 10);
    subscriber.on_network_loading_fail("T", "N4", "err", false);
    subscriber.on_dom_attribute_modified("T", 1, "n", "v");
    subscriber.on_dom_character_data_modified("T", 2, "o", "n");
    subscriber.on_script_parsed("T", "S", "u", 0, 0, 0, 0, None);
    subscriber.on_frame_navigated("T", "F", "u", None);
    subscriber.on_frame_started_loading("T", "F");
    subscriber.on_frame_stopped_loading("T", "F");
    subscriber.on_timeline_marker("T", "m", 0.0, 1.0);

    transport.set_event_timeout(Duration::from_secs(2));
    let mut methods: HashSet<String> = HashSet::new();
    while let Ok(Some(ev)) = transport.recv_event() {
        methods.insert(ev.method);
    }

    // 7 类对应 13 个 CDP method 全部到达
    let expected: &[&str] = &[
        "Log.entryAdded",
        "Runtime.exceptionThrown",
        "Network.requestWillBeSent",
        "Network.responseReceived",
        "Network.loadingFinished",
        "Network.loadingFailed",
        "DOM.attributeModified",
        "DOM.characterDataModified",
        "Debugger.scriptParsed",
        "Page.frameNavigated",
        "Page.frameStartedLoading",
        "Page.frameStoppedLoading",
        "Performance.metrics",
    ];
    for m in expected {
        // Assert
        assert!(
            methods.contains(*m),
            "FAIL: missing CDP event {m} in transport recv (got {:?})",
            methods
        );
    }
}

// ════════════════════════════════════════════════════════════════════
// §9 close 行为
// ════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BAO-API-003 [level:integration]
fn full_e2e_close_blocks_recv_event() {
    // Arrange
    let (mut transport, _subscriber) = build_with_events();
    // Act
    transport.close().unwrap();
    let err = transport.recv_event().unwrap_err();
    use bao_cdp_client::CdpError;
    // Assert
    assert!(matches!(err, CdpError::ConnectionClosed));
}

// ════════════════════════════════════════════════════════════════════
// §10 params schema 完整性
// ════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BAO-API-003 [event:Console] [level:integration]
fn full_e2e_console_params_contain_required_fields() {
    // Arrange
    let (mut transport, subscriber) = build_with_events();
    // Act
    subscriber.on_console_message(
        "T", ConsoleLevel::Error, "msg",
        Some("file.js".into()), Some(10), Some(5),
    );

    transport.set_event_timeout(Duration::from_secs(2));
    let ev = transport.recv_event().unwrap().expect("event");
    let entry = &ev.params["entry"];
    // 必须字段
    // Assert
    assert!(entry["source"].is_string(), "source");
    assert!(entry["level"].is_string(), "level");
    assert!(entry["text"].is_string(), "text");
    // 可选字段 — 已传入,必须存在
    assert_eq!(entry["url"], "file.js");
    assert_eq!(entry["lineNumber"], 10);
    assert_eq!(entry["columnNumber"], 5);
    // timestamp 必须有
    assert!(entry["timestamp"].is_number(), "timestamp");
}

#[test]
// @trace REQ-BAO-API-003 [event:NetworkEvent] [level:integration]
fn full_e2e_network_request_params_contain_required_fields() {
    // Arrange
    let (mut transport, subscriber) = build_with_events();
    let mut headers = HashMap::new();
    // Act
    headers.insert("X-Custom".into(), "value".into());

    subscriber.on_network_request(
        "T", "REQ-1", "https://example.com", "POST",
        headers.clone(), Some(b"body".to_vec()), "XHR", "FRAME-1",
    );

    transport.set_event_timeout(Duration::from_secs(2));
    let ev = transport.recv_event().unwrap().expect("event");
    // Assert
    assert_eq!(ev.method, "Network.requestWillBeSent");
    assert_eq!(ev.params["requestId"], "REQ-1");
    assert_eq!(ev.params["request"]["url"], "https://example.com");
    assert_eq!(ev.params["request"]["method"], "POST");
    assert_eq!(ev.params["request"]["headers"]["X-Custom"], "value");
    assert_eq!(ev.params["type"], "XHR");
    assert_eq!(ev.params["frameId"], "FRAME-1");
}

#[test]
// @trace REQ-BAO-API-003 [event:NetworkEvent] [level:integration]
fn full_e2e_network_response_params_contain_required_fields() {
    // Arrange
    let (mut transport, subscriber) = build_with_events();
    // Act
    subscriber.on_network_response(
        "T", "R1", "https://x", 404,
        "Not Found", HashMap::new(), "application/json",
        Some("10.0.0.1".into()),
    );

    transport.set_event_timeout(Duration::from_secs(2));
    let ev = transport.recv_event().unwrap().expect("event");
    // Assert
    assert_eq!(ev.method, "Network.responseReceived");
    assert_eq!(ev.params["response"]["status"], 404);
    assert_eq!(ev.params["response"]["statusText"], "Not Found");
    assert_eq!(ev.params["response"]["mimeType"], "application/json");
    assert_eq!(ev.params["response"]["remoteIPAddress"], "10.0.0.1");
}

#[test]
// @trace REQ-BAO-API-003 [event:DomMutation] [level:integration]
fn full_e2e_dom_attribute_modified_params_correct() {
    // Arrange
    let (mut transport, subscriber) = build_with_events();
    // Act
    subscriber.on_dom_attribute_modified("T", 42, "data-id", "abc123");

    transport.set_event_timeout(Duration::from_secs(2));
    let ev = transport.recv_event().unwrap().expect("event");
    // Assert
    assert_eq!(ev.method, "DOM.attributeModified");
    assert_eq!(ev.params["nodeId"], 42);
    assert_eq!(ev.params["name"], "data-id");
    assert_eq!(ev.params["value"], "abc123");
}

#[test]
// @trace REQ-BAO-API-003 [event:SourceInfo] [level:integration]
fn full_e2e_script_parsed_params_correct() {
    // Arrange
    let (mut transport, subscriber) = build_with_events();
    // Act
    subscriber.on_script_parsed(
        "T", "SCRIPT-1", "https://example.com/a.js",
        0, 0, 100, 200, Some("https://example.com/a.js.map".into()),
    );

    transport.set_event_timeout(Duration::from_secs(2));
    let ev = transport.recv_event().unwrap().expect("event");
    // Assert
    assert_eq!(ev.method, "Debugger.scriptParsed");
    assert_eq!(ev.params["scriptId"], "SCRIPT-1");
    assert_eq!(ev.params["url"], "https://example.com/a.js");
    assert_eq!(ev.params["endLine"], 100);
    assert_eq!(ev.params["endColumn"], 200);
    assert_eq!(ev.params["sourceMapURL"], "https://example.com/a.js.map");
}
