//! Dispatcher 边界条件测试 — 未知 method / 无效格式 / 不存在的 domain。
//!
//! @trace REQ-BAO-API-004 [level:integration]
//! @trace REQ-BAO-API-007 [level:integration]

use bao_cdp_client::bridge::{BridgeError, MockServoBackend, ServoBackend};
use bao_cdp_client::dispatch_command;
use serde_json::{json, Value};
use std::sync::Arc;

fn backend() -> Arc<dyn ServoBackend> {
    Arc::new(MockServoBackend::new())
}

#[test]
fn invalid_method_no_dot_returns_invalid_method_error() {
    // @trace REQ-BAO-API-004 [level:library]
    let b = backend();
    let err = dispatch_command(&*b, "noDotHere", json!({}), "1").unwrap_err();
    assert!(matches!(err, BridgeError::InvalidMethod(_)));
    // InvalidMethod → -32602 invalid params.
    assert_eq!(err.cdp_error_code(), -32602);
}

#[test]
fn unknown_domain_returns_method_not_found() {
    let b = backend();
    let err = dispatch_command(&*b, "UnknownDomain.foo", json!({}), "1").unwrap_err();
    assert!(matches!(err, BridgeError::MethodNotFound(_)));
    assert_eq!(err.cdp_error_code(), -32601);
}

#[test]
fn unknown_method_in_known_domain_returns_method_not_found() {
    let b = backend();
    let err = dispatch_command(&*b, "Page.totallyBogus", json!({}), "1").unwrap_err();
    assert!(matches!(err, BridgeError::MethodNotFound(_)));
}

#[test]
fn unknown_method_in_runtime_returns_method_not_found() {
    let b = backend();
    let err = dispatch_command(&*b, "Runtime.totallyBogus", json!({}), "1").unwrap_err();
    assert!(matches!(err, BridgeError::MethodNotFound(_)));
}

#[test]
fn b_class_routes_to_eval_synthesizer() {
    // @trace REQ-BAO-API-005 [level:integration]
    // TASK-3b: B 类 method 通过 IIFE Eval 合成,返回 evaluate echo expression。
    let b = backend();
    let r = dispatch_command(&*b, "Page.title", json!({}), "1").unwrap();
    // Mock backend echo:返回 evaluate expression 字符串
    let v = r["result"]["value"].as_str().unwrap();
    assert!(v.contains("(function(){"));
    assert!(v.contains("return document.title;"));
    assert!(v.ends_with("})()"));
}

#[test]
fn empty_method_returns_invalid_method() {
    let b = backend();
    let err = dispatch_command(&*b, "", json!({}), "1").unwrap_err();
    assert!(matches!(err, BridgeError::InvalidMethod(_)));
}

#[test]
fn method_with_only_dot_returns_invalid_method() {
    let b = backend();
    // "." splits into ("", ""), which falls through to MethodNotFound
    // (empty domain, empty command).
    let err = dispatch_command(&*b, ".", json!({}), "1").unwrap_err();
    // Empty domain → not in A class → not in E class → MethodNotFound.
    assert!(matches!(err, BridgeError::MethodNotFound(_)));
}

#[test]
fn unknown_target_id_returns_page_not_found() {
    let b = backend();
    let err = dispatch_command(
        &*b,
        "Page.navigate",
        json!({"url":"https://x"}),
        "999",
    )
    .unwrap_err();
    assert!(matches!(err, BridgeError::PageNotFound(_)));
    assert_eq!(err.cdp_error_code(), -32000);
}

#[test]
fn valid_dispatch_returns_value_not_error() {
    // Sanity: ensure dispatch returns Ok for valid command.
    let b = backend();
    let r = dispatch_command(&*b, "Page.navigate", json!({"url":"https://x"}), "1").unwrap();
    assert!(r.is_object());
    assert!(r["frameId"].is_string());
}

#[test]
fn default_session_used_when_session_id_is_none() {
    // The CDPRdpBridge uses "default" when session_id is None.
    // MockServoBackend knows about "default".
    use bao_cdp_client::transport::in_memory::{InMemoryBridge, InMemoryBridgeResponse};
    use bao_cdp_client::CDPRdpBridge;
    let b: Arc<dyn ServoBackend> = Arc::new(MockServoBackend::new());
    let bridge = CDPRdpBridge::new(b);
    let bridge_dyn: Arc<dyn InMemoryBridge> = bridge.into_in_memory_bridge();
    let r = bridge_dyn.dispatch_command("Page.navigate", json!({"url":"x"}), None);
    match r {
        InMemoryBridgeResponse::Ok(v) => assert_eq!(v["frameId"], "FRAME_0"),
        InMemoryBridgeResponse::Err(e) => panic!("expected Ok, got Err: {e}"),
    }
}

#[test]
fn cdp_error_response_payload_carries_code_and_message() {
    // E-class error: serialized as JSON payload `{code, message}`.
    use bao_cdp_client::transport::in_memory::{InMemoryBridge, InMemoryBridgeResponse};
    use bao_cdp_client::CDPRdpBridge;
    let b: Arc<dyn ServoBackend> = Arc::new(MockServoBackend::new());
    let bridge = CDPRdpBridge::new(b);
    let bridge_dyn: Arc<dyn InMemoryBridge> = bridge.into_in_memory_bridge();
    let r = bridge_dyn.dispatch_command("HeapProfiler.takeHeapSnapshot", json!({}), Some("1"));
    match r {
        InMemoryBridgeResponse::Err(s) => {
            let v: Value = serde_json::from_str(&s).unwrap();
            assert_eq!(v["code"], -32601);
            assert!(v["message"].as_str().unwrap().contains("HeapProfiler"));
        }
        _ => panic!("expected Err"),
    }
}

#[test]
fn cdp_error_response_for_method_not_found_carries_correct_code() {
    use bao_cdp_client::transport::in_memory::{InMemoryBridge, InMemoryBridgeResponse};
    use bao_cdp_client::CDPRdpBridge;
    let b: Arc<dyn ServoBackend> = Arc::new(MockServoBackend::new());
    let bridge = CDPRdpBridge::new(b);
    let bridge_dyn: Arc<dyn InMemoryBridge> = bridge.into_in_memory_bridge();
    let r = bridge_dyn.dispatch_command("Foo.bar", json!({}), Some("1"));
    match r {
        InMemoryBridgeResponse::Err(s) => {
            let v: Value = serde_json::from_str(&s).unwrap();
            assert_eq!(v["code"], -32601);
            assert!(v["message"].as_str().unwrap().contains("Foo.bar"));
        }
        _ => panic!("expected Err"),
    }
}

#[test]
fn cdp_error_response_for_invalid_params_carries_32602() {
    use bao_cdp_client::transport::in_memory::{InMemoryBridge, InMemoryBridgeResponse};
    use bao_cdp_client::CDPRdpBridge;
    let b: Arc<dyn ServoBackend> = Arc::new(MockServoBackend::new());
    let bridge = CDPRdpBridge::new(b);
    let bridge_dyn: Arc<dyn InMemoryBridge> = bridge.into_in_memory_bridge();
    // Missing required "url" parameter.
    let r = bridge_dyn.dispatch_command("Page.navigate", json!({}), Some("1"));
    match r {
        InMemoryBridgeResponse::Err(s) => {
            let v: Value = serde_json::from_str(&s).unwrap();
            assert_eq!(v["code"], -32602);
            assert!(v["message"].as_str().unwrap().contains("url"));
        }
        _ => panic!("expected Err"),
    }
}
