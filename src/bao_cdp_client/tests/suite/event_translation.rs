//! TASK-4 — servo 7 大事件 → CDP event 端到端集成测试。
//!
//! ## 验收(REQ-BAO-API-003)
//!
//! 1. Console → Log.entryAdded
//! 2. PageError → Runtime.exceptionThrown
//! 3. NetworkRequest/Response/LoadingFinish/LoadingFail → Network.*
//! 4. DomAttributeModified/CharacterDataModified → DOM.*
//! 5. ScriptParsed → Debugger.scriptParsed
//! 6. FrameNavigated/StartedLoading/StoppedLoading → Page.*
//! 7. TimelineMarker → Performance.metrics
//! 8. 事件零遗漏(7 类全覆盖)
//!
//! ## 测试策略
//!
//! - **单元级**:对每类事件直接 `translate(ServoEvent::...)` 验证 CDP method + params schema
//! - **集成级**:通过 `EventSubscriber` + `InMemoryTransport::attach_servo_event_receiver`
//!   端到端模拟 servo 端 push → CDP client 端 recv,验证完整链路
//!
//! @trace REQ-BAO-API-003 [level:integration]
//! @trace TEST-BAO-API-003

use std::collections::HashMap;
use std::sync::Arc;

use bao_cdp_client::bridge::event_translator::{translate, ConsoleLevel, ServoEvent};
use bao_cdp_client::bridge::EventSubscriber;
use bao_cdp_client::transport::{
    CdpEvent, InMemoryBridge, InMemoryBridgeResponse, InMemoryTransport, Transport,
};
use serde_json::Value;

// ────────────────────────────────────────────────────────────────────────────
// §1 单元级 — 7 类事件 schema 验证
// ────────────────────────────────────────────────────────────────────────────

/// 断言 CdpEvent 的 method + session_id,返回 params 供进一步检查。
fn assert_event<'a>(ev: &'a CdpEvent, expected_method: &str, expected_session: &str) -> &'a Value {
    assert_eq!(ev.method, expected_method, "method mismatch");
    assert_eq!(
        ev.session_id.as_deref(),
        Some(expected_session),
        "session_id mismatch"
    );
    &ev.params
}

#[test]
// @trace REQ-BAO-API-003 [event:Console] [level:integration]
fn e2e_console_to_log_entry_added() {
    // Arrange
    let ev = ServoEvent::Console {
        // Act
        target_id: "TARGET-CON".into(),
        level: ConsoleLevel::Error,
        text: "boom".into(),
        url: Some("http://example.com/x.js".into()),
        line: Some(10),
        column: Some(5),
    };
    let out = translate(ev);
    // Assert
    assert_eq!(out.len(), 1);
    let params = assert_event(&out[0], "Log.entryAdded", "TARGET-CON");
    let entry = &params["entry"];
    assert_eq!(entry["source"], "javascript");
    assert_eq!(entry["level"], "error");
    assert_eq!(entry["text"], "boom");
    assert_eq!(entry["lineNumber"], 10);
    assert_eq!(entry["columnNumber"], 5);
}

#[test]
// @trace REQ-BAO-API-003 [event:PageError] [level:integration]
fn e2e_page_error_to_runtime_exception_thrown() {
    // Arrange
    let ev = ServoEvent::PageError {
        // Act
        target_id: "TARGET-PE".into(),
        text: "Uncaught TypeError".into(),
        url: Some("a.js".into()),
        line: Some(20),
        column: Some(3),
        stack: Some("at f (a.js:20:3)".into()),
    };
    let out = translate(ev);
    // Assert
    assert_eq!(out.len(), 1);
    let params = assert_event(&out[0], "Runtime.exceptionThrown", "TARGET-PE");
    assert!(params["timestamp"].is_number());
    let details = &params["exceptionDetails"];
    assert_eq!(details["text"], "Uncaught TypeError");
    assert_eq!(details["lineNumber"], 20);
    assert_eq!(details["columnNumber"], 3);
    assert!(details["stackTrace"].is_array());
}

#[test]
// @trace REQ-BAO-API-003 [event:NetworkEvent] [level:integration]
fn e2e_network_request_to_request_will_be_sent() {
    // Arrange
    let mut headers = HashMap::new();
    // Act
    headers.insert("X-A".into(), "1".into());
    let ev = ServoEvent::NetworkRequest {
        target_id: "T-NET".into(),
        request_id: "R1".into(),
        url: "http://x".into(),
        method: "GET".into(),
        headers,
        post_data: None,
        resource_type: "Document".into(),
        frame_id: "F1".into(),
    };
    let out = translate(ev);
    let params = assert_event(&out[0], "Network.requestWillBeSent", "T-NET");
    // Assert
    assert_eq!(params["requestId"], "R1");
    assert_eq!(params["frameId"], "F1");
    assert_eq!(params["request"]["url"], "http://x");
    assert_eq!(params["request"]["method"], "GET");
    assert_eq!(params["request"]["headers"]["X-A"], "1");
}

#[test]
// @trace REQ-BAO-API-003 [event:NetworkEvent] [level:integration]
fn e2e_network_response_to_response_received() {
    // Arrange
    let ev = ServoEvent::NetworkResponse {
        // Act
        target_id: "T-NET".into(),
        request_id: "R2".into(),
        url: "http://x".into(),
        status: 404,
        status_text: "Not Found".into(),
        headers: HashMap::new(),
        mime_type: "text/html".into(),
        remote_ip: None,
    };
    let out = translate(ev);
    let params = assert_event(&out[0], "Network.responseReceived", "T-NET");
    // Assert
    assert_eq!(params["response"]["status"], 404);
    assert_eq!(params["response"]["statusText"], "Not Found");
}

#[test]
// @trace REQ-BAO-API-003 [event:NetworkEvent] [level:integration]
fn e2e_network_loading_finish_to_loading_finished() {
    // Arrange
    let ev = ServoEvent::NetworkLoadingFinish {
        // Act
        target_id: "T-NET".into(),
        request_id: "R3".into(),
        encoded_data_length: 999,
    };
    let out = translate(ev);
    let params = assert_event(&out[0], "Network.loadingFinished", "T-NET");
    // Assert
    assert_eq!(params["requestId"], "R3");
    assert_eq!(params["encodedDataLength"], 999);
}

#[test]
// @trace REQ-BAO-API-003 [event:NetworkEvent] [level:integration]
fn e2e_network_loading_fail_to_loading_failed() {
    // Arrange
    let ev = ServoEvent::NetworkLoadingFail {
        // Act
        target_id: "T-NET".into(),
        request_id: "R4".into(),
        error_text: "fail".into(),
        canceled: true,
    };
    let out = translate(ev);
    let params = assert_event(&out[0], "Network.loadingFailed", "T-NET");
    // Assert
    assert_eq!(params["errorText"], "fail");
    assert_eq!(params["canceled"], true);
}

#[test]
// @trace REQ-BAO-API-003 [event:DomMutation] [level:integration]
fn e2e_dom_attribute_modified() {
    // Arrange
    let ev = ServoEvent::DomAttributeModified {
        // Act
        target_id: "T-DOM".into(),
        node_id: 1,
        name: "id".into(),
        value: "foo".into(),
    };
    let out = translate(ev);
    let params = assert_event(&out[0], "DOM.attributeModified", "T-DOM");
    // Assert
    assert_eq!(params["nodeId"], 1);
    assert_eq!(params["name"], "id");
    assert_eq!(params["value"], "foo");
}

#[test]
// @trace REQ-BAO-API-003 [event:DomMutation] [level:integration]
fn e2e_dom_character_data_modified() {
    // Arrange
    let ev = ServoEvent::DomCharacterDataModified {
        // Act
        target_id: "T-DOM".into(),
        node_id: 2,
        old_value: "old".into(),
        new_value: "new".into(),
    };
    let out = translate(ev);
    let params = assert_event(&out[0], "DOM.characterDataModified", "T-DOM");
    // Assert
    assert_eq!(params["nodeId"], 2);
    assert_eq!(params["characterData"], "new");
}

#[test]
// @trace REQ-BAO-API-003 [event:SourceInfo] [level:integration]
fn e2e_script_parsed() {
    // Arrange
    let ev = ServoEvent::ScriptParsed {
        // Act
        target_id: "T-SRC".into(),
        script_id: "S1".into(),
        url: "x.js".into(),
        start_line: 0,
        start_column: 0,
        end_line: 10,
        end_column: 0,
        source_map_url: None,
    };
    let out = translate(ev);
    let params = assert_event(&out[0], "Debugger.scriptParsed", "T-SRC");
    // Assert
    assert_eq!(params["scriptId"], "S1");
    assert_eq!(params["url"], "x.js");
    assert_eq!(params["endLine"], 10);
}

#[test]
// @trace REQ-BAO-API-003 [event:FrameInfo] [level:integration]
fn e2e_frame_navigated() {
    // Arrange
    let ev = ServoEvent::FrameNavigated {
        // Act
        target_id: "T-FR".into(),
        frame_id: "F1".into(),
        url: "http://x".into(),
        name: None,
    };
    let out = translate(ev);
    let params = assert_event(&out[0], "Page.frameNavigated", "T-FR");
    // Assert
    assert_eq!(params["frame"]["id"], "F1");
    assert_eq!(params["frame"]["url"], "http://x");
}

#[test]
// @trace REQ-BAO-API-003 [event:FrameInfo] [level:integration]
fn e2e_frame_started_loading() {
    // Arrange
    let ev = ServoEvent::FrameStartedLoading {
        // Act
        target_id: "T-FR".into(),
        frame_id: "F2".into(),
    };
    let out = translate(ev);
    let params = assert_event(&out[0], "Page.frameStartedLoading", "T-FR");
    // Assert
    assert_eq!(params["frameId"], "F2");
}

#[test]
// @trace REQ-BAO-API-003 [event:FrameInfo] [level:integration]
fn e2e_frame_stopped_loading() {
    // Arrange
    let ev = ServoEvent::FrameStoppedLoading {
        // Act
        target_id: "T-FR".into(),
        frame_id: "F3".into(),
    };
    let out = translate(ev);
    let params = assert_event(&out[0], "Page.frameStoppedLoading", "T-FR");
    // Assert
    assert_eq!(params["frameId"], "F3");
}

#[test]
// @trace REQ-BAO-API-003 [event:TimelineMarker] [level:integration]
fn e2e_timeline_marker_to_performance_metrics() {
    // Arrange
    let ev = ServoEvent::TimelineMarker {
        // Act
        target_id: "T-TL".into(),
        name: "render".into(),
        start_time: 0.0,
        end_time: 1.5,
    };
    let out = translate(ev);
    let params = assert_event(&out[0], "Performance.metrics", "T-TL");
    // Assert
    assert!(params["metrics"].is_array());
    assert_eq!(params["title"], "servo-timeline-render");
}

// ────────────────────────────────────────────────────────────────────────────
// §2 集成级 — EventSubscriber + InMemoryTransport 端到端
// ────────────────────────────────────────────────────────────────────────────

/// 测试用 minimal InMemoryBridge(命令响应固定 Null)。
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

#[test]
// @trace REQ-BAO-API-003 [level:integration]
fn e2e_full_chain_console_event_through_transport() {
    // Arrange
    let bridge: Arc<dyn InMemoryBridge> = Arc::new(NullBridge);
    let mut transport = InMemoryTransport::new(bridge);
    let (subscriber, rx) = EventSubscriber::new();
    // Act
    transport.attach_servo_event_receiver(rx);

    // 模拟 servo 端 push 一个 console 事件
    subscriber.on_console_message("TARGET-X", ConsoleLevel::Info, "hello", None, None, None);

    // CDP client 端 recv — 应该是 Log.entryAdded
    let ev = transport
        .recv_event()
        .expect("recv_event ok")
        .expect("got event");
    // Assert
    assert_eq!(ev.method, "Log.entryAdded");
    assert_eq!(ev.session_id.as_deref(), Some("TARGET-X"));
    assert_eq!(ev.params["entry"]["text"], "hello");
}

#[test]
// @trace REQ-BAO-API-003 [level:integration]
fn e2e_full_chain_multiple_events_in_order() {
    // Arrange
    let bridge: Arc<dyn InMemoryBridge> = Arc::new(NullBridge);
    let mut transport = InMemoryTransport::new(bridge);
    let (subscriber, rx) = EventSubscriber::new();
    // Act
    transport.attach_servo_event_receiver(rx);

    // 依次 push 3 个不同类型的事件
    subscriber.on_console_message("T", ConsoleLevel::Info, "log1", None, None, None);
    subscriber.on_page_error("T", "err1", None, None, None, None);
    subscriber.on_frame_started_loading("T", "F1");

    let ev1 = transport.recv_event().unwrap().unwrap();
    let ev2 = transport.recv_event().unwrap().unwrap();
    let ev3 = transport.recv_event().unwrap().unwrap();
    // Assert
    assert_eq!(ev1.method, "Log.entryAdded");
    assert_eq!(ev2.method, "Runtime.exceptionThrown");
    assert_eq!(ev3.method, "Page.frameStartedLoading");
}

#[test]
// @trace REQ-BAO-API-003 [level:integration]
fn e2e_fallback_to_direct_cdp_event_channel_when_no_servo_events() {
    // Arrange
    let bridge: Arc<dyn InMemoryBridge> = Arc::new(NullBridge);
    let mut transport = InMemoryTransport::new(bridge);
    let (_subscriber, rx) = EventSubscriber::new();
    // Act
    transport.attach_servo_event_receiver(rx);

    // 直接通过 event_sender push 一个 CdpEvent(测试 mock 模式)
    let sender = transport.event_sender();
    sender
        .send(CdpEvent::new("Custom.event", serde_json::json!({"k": "v"})))
        .unwrap();

    let ev = transport.recv_event().unwrap().unwrap();
    // servo channel 空,fallback 到 event_rx
    // Assert
    assert_eq!(ev.method, "Custom.event");
}

#[test]
// @trace REQ-BAO-API-003 [level:integration]
fn e2e_no_event_returns_none_on_timeout() {
    // Arrange
    let bridge: Arc<dyn InMemoryBridge> = Arc::new(NullBridge);
    let mut transport = InMemoryTransport::new(bridge);
    let (_subscriber, rx) = EventSubscriber::new();
    // Act
    transport.attach_servo_event_receiver(rx);
    transport.set_event_timeout(std::time::Duration::from_millis(50));

    let ev = transport.recv_event().unwrap();
    // Assert
    assert!(ev.is_none(), "expected None on timeout");
}

#[test]
// @trace REQ-BAO-API-003 [level:integration]
fn e2e_seven_classes_each_route_to_correct_method() {
    // Arrange
    let bridge: Arc<dyn InMemoryBridge> = Arc::new(NullBridge);
    let mut transport = InMemoryTransport::new(bridge);
    let (subscriber, rx) = EventSubscriber::new();
    // Act
    transport.attach_servo_event_receiver(rx);

    // 7 类事件全部 push
    subscriber.on_console_message("T", ConsoleLevel::Debug, "c", None, None, None);
    subscriber.on_page_error("T", "p", None, None, None, None);
    subscriber.on_network_request("T", "N1", "u", "GET", HashMap::new(), None, "Other", "F");
    subscriber.on_network_response("T", "N2", "u", 200, "OK", HashMap::new(), "text/html", None);
    subscriber.on_network_loading_finish("T", "N3", 10);
    subscriber.on_network_loading_fail("T", "N4", "err", false);
    subscriber.on_dom_attribute_modified("T", 1, "n", "v");
    subscriber.on_dom_character_data_modified("T", 2, "o", "n");
    subscriber.on_script_parsed("T", "S", "u", 0, 0, 0, 0, None);
    subscriber.on_frame_navigated("T", "F", "u", None);
    subscriber.on_frame_started_loading("T", "F");
    subscriber.on_frame_stopped_loading("T", "F");
    subscriber.on_timeline_marker("T", "m", 0.0, 1.0);

    transport.set_event_timeout(std::time::Duration::from_millis(500));

    let mut methods = Vec::new();
    while let Ok(Some(ev)) = transport.recv_event() {
        methods.push(ev.method);
    }

    // 验证 7 类全覆盖(13 events)
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
    // Assert
    assert_eq!(
        methods.len(),
        expected.len(),
        "expected {} events",
        expected.len()
    );
    for (i, exp) in expected.iter().enumerate() {
        assert_eq!(&methods[i].as_str(), exp, "event {} method mismatch", i);
    }
}

// ────────────────────────────────────────────────────────────────────────────
// §3 全覆盖统计 — 7 类零遗漏
// ────────────────────────────────────────────────────────────────────────────

#[test]
// @trace REQ-BAO-API-003 [level:integration]
fn all_seven_classes_zero_omission() {
    // Arrange
    use std::collections::HashSet;

    // 列举 7 类的样本,translate 后收集所有 CDP method
    let samples: Vec<ServoEvent> = vec![
        ServoEvent::Console {
            // Act
            target_id: "T".into(),
            level: ConsoleLevel::Info,
            text: String::new(),
            url: None,
            line: None,
            column: None,
        },
        ServoEvent::PageError {
            target_id: "T".into(),
            text: String::new(),
            url: None,
            line: None,
            column: None,
            stack: None,
        },
        ServoEvent::NetworkRequest {
            target_id: "T".into(),
            request_id: "r".into(),
            url: "u".into(),
            method: "GET".into(),
            headers: HashMap::new(),
            post_data: None,
            resource_type: "Other".into(),
            frame_id: "f".into(),
        },
        ServoEvent::NetworkResponse {
            target_id: "T".into(),
            request_id: "r".into(),
            url: "u".into(),
            status: 200,
            status_text: "OK".into(),
            headers: HashMap::new(),
            mime_type: "text/html".into(),
            remote_ip: None,
        },
        ServoEvent::NetworkLoadingFinish {
            target_id: "T".into(),
            request_id: "r".into(),
            encoded_data_length: 0,
        },
        ServoEvent::NetworkLoadingFail {
            target_id: "T".into(),
            request_id: "r".into(),
            error_text: "e".into(),
            canceled: false,
        },
        ServoEvent::DomAttributeModified {
            target_id: "T".into(),
            node_id: 1,
            name: "n".into(),
            value: "v".into(),
        },
        ServoEvent::DomCharacterDataModified {
            target_id: "T".into(),
            node_id: 1,
            old_value: "o".into(),
            new_value: "n".into(),
        },
        ServoEvent::ScriptParsed {
            target_id: "T".into(),
            script_id: "s".into(),
            url: "u".into(),
            start_line: 0,
            start_column: 0,
            end_line: 0,
            end_column: 0,
            source_map_url: None,
        },
        ServoEvent::FrameNavigated {
            target_id: "T".into(),
            frame_id: "f".into(),
            url: "u".into(),
            name: None,
        },
        ServoEvent::FrameStartedLoading {
            target_id: "T".into(),
            frame_id: "f".into(),
        },
        ServoEvent::FrameStoppedLoading {
            target_id: "T".into(),
            frame_id: "f".into(),
        },
        ServoEvent::TimelineMarker {
            target_id: "T".into(),
            name: "n".into(),
            start_time: 0.0,
            end_time: 1.0,
        },
    ];

    let mut all_methods: HashSet<String> = HashSet::new();
    for s in samples {
        for ev in translate(s) {
            all_methods.insert(ev.method);
        }
    }

    // 7 类的 13 个目标 CDP method 全部覆盖
    let expected: &[&str] = &[
        "Log.entryAdded",          // Console
        "Runtime.exceptionThrown", // PageError
        "Network.requestWillBeSent",
        "Network.responseReceived",
        "Network.loadingFinished",
        "Network.loadingFailed", // NetworkEvent
        "DOM.attributeModified",
        "DOM.characterDataModified", // DomMutation
        "Debugger.scriptParsed",     // SourceInfo
        "Page.frameNavigated",
        "Page.frameStartedLoading",
        "Page.frameStoppedLoading", // FrameInfo
        "Performance.metrics",      // TimelineMarker
    ];
    for m in expected {
        // Assert
        assert!(
            all_methods.contains(*m),
            "FAIL: 7 类事件零遗漏 — 缺少 CDP method {}",
            m
        );
    }
    assert_eq!(
        all_methods.len(),
        expected.len(),
        "CDP method 数量异常,可能多出未定义 method"
    );
}
