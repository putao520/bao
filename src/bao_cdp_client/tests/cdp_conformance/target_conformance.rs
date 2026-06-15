//! Target domain conformance 审计 — 6 method。
//!
//! 对照 CDP 官方规范(Target domain):
//! https://chromedevtools.github.io/devtools-protocol/tot/Target/
//!
//! # 覆盖 method
//!
//! getTargets, createTarget, closeTarget, attachToTarget, detachFromTarget,
//! setAutoAttach
//!
//! @trace REQ-CDP-001 [domain:Target] [level:integration]
//! @trace REQ-BAO-API-004 [domain:Target] [level:integration]

use bao_cdp_client::{dispatch_command, BridgeError, MockServoBackend, ServoBackend};
use serde_json::json;
use std::sync::Arc;

fn backend() -> Arc<dyn ServoBackend> {
    Arc::new(MockServoBackend::new())
}

// ─────────────────────────────────────────────────────────────────────────
// Target.getTargets — CDP spec: returns {targetInfos: [TargetInfo]}
// TargetInfo: {targetId, type, title, url, attached, browserContextId?, ...}
// https://chromedevtools.github.io/devtools-protocol/tot/Target/#method-getTargets
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn target_get_targets_result_schema_conformance() {
    // Arrange — CDP 规范: 返回 {targetInfos: array}
    // @trace REQ-CDP-001 [domain:Target] [level:integration]
    let b = backend();

    // Act
    let result = dispatch_command(&*b, "Target.getTargets", json!({}), "1").unwrap();

    // Assert
    assert!(
        result["targetInfos"].is_array(),
        "CDP spec: targetInfos must be array, got: {:?}",
        result["targetInfos"]
    );
    for info in result["targetInfos"].as_array().unwrap() {
        assert!(
            info["targetId"].is_string(),
            "TargetInfo.targetId must be string"
        );
        assert!(info["type"].is_string(), "TargetInfo.type must be string");
        assert!(info["title"].is_string(), "TargetInfo.title must be string");
        assert!(info["url"].is_string(), "TargetInfo.url must be string");
        assert!(
            info["attached"].is_boolean(),
            "TargetInfo.attached must be boolean"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Target.createTarget — CDP spec: returns {targetId: TargetId (string)}
// params: {url (required), width?, height?, browserContextId?, ...}
// https://chromedevtools.github.io/devtools-protocol/tot/Target/#method-createTarget
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn target_create_target_result_schema_conformance() {
    // Arrange — CDP 规范: 返回 {targetId: string}
    // @trace REQ-CDP-001 [domain:Target] [level:integration]
    let b = backend();

    // Act
    let result = dispatch_command(&*b, "Target.createTarget", json!({"url":"about:blank"}), "1").unwrap();

    // Assert
    assert!(
        result["targetId"].is_string(),
        "CDP spec: targetId must be string (TargetId type), got: {:?}",
        result["targetId"]
    );
}

#[test]
fn target_create_target_missing_url_returns_32602() {
    // @trace REQ-BAO-API-007 [domain:Target] [level:integration]
    let b = backend();
    let err = dispatch_command(&*b, "Target.createTarget", json!({}), "1").unwrap_err();
    assert!(matches!(err, BridgeError::InvalidParams(_)));
    assert_eq!(err.cdp_error_code(), -32602);
}

#[test]
fn target_create_target_with_optional_params_accepted() {
    // Arrange — CDP 规范: width / height / browserContextId / enableBeginFrameControl 可选
    // @trace REQ-CDP-001 [domain:Target] [level:integration]
    let b = backend();
    let result = dispatch_command(
        &*b,
        "Target.createTarget",
        json!({"url":"https://x", "width":1024, "height":768}),
        "1",
    )
    .unwrap();
    assert!(result["targetId"].is_string());
}

// ─────────────────────────────────────────────────────────────────────────
// Target.closeTarget — CDP spec: returns {success: boolean} (deprecated) or empty
// params: {targetId (required)}
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn target_close_target_returns_success_or_empty() {
    // Arrange — CDP 规范: deprecated 返回 {success: bool},新版可能返回空对象
    // bao 实现: {success: true}
    // @trace REQ-CDP-001 [domain:Target] [level:integration]
    let b = backend();

    // 先创建一个 target,然后关闭它
    let create_result =
        dispatch_command(&*b, "Target.createTarget", json!({"url":"about:blank"}), "1").unwrap();
    let new_target_id = create_result["targetId"].as_str().unwrap().to_string();

    // Act
    let result = dispatch_command(
        &*b,
        "Target.closeTarget",
        json!({"targetId":new_target_id}),
        "1",
    )
    .unwrap();

    // Assert — bao 返回 {success: true}(deprecated CDP schema)
    assert!(
        result["success"].is_boolean(),
        "CDP spec (deprecated): closeTarget returns {{success: boolean}}, got: {:?}",
        result
    );
    assert_eq!(result["success"], true);
}

#[test]
fn target_close_target_missing_target_id_returns_32602() {
    // @trace REQ-BAO-API-007 [domain:Target] [level:integration]
    let b = backend();
    let err = dispatch_command(&*b, "Target.closeTarget", json!({}), "1").unwrap_err();
    assert!(matches!(err, BridgeError::InvalidParams(_)));
    assert_eq!(err.cdp_error_code(), -32602);
}

// ─────────────────────────────────────────────────────────────────────────
// Target.attachToTarget — CDP spec: returns {sessionId: string}
// params: {targetId (required), flatten?}
// https://chromedevtools.github.io/devtools-protocol/tot/Target/#method-attachToTarget
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn target_attach_to_target_result_schema_conformance() {
    // Arrange — CDP 规范: flat-mode 返回 {sessionId: string}
    // @trace REQ-CDP-001 [domain:Target] [level:integration]
    let b = backend();

    // Act
    let result = dispatch_command(&*b, "Target.attachToTarget", json!({"targetId":"1"}), "1").unwrap();

    // Assert
    assert!(
        result["sessionId"].is_string(),
        "CDP spec: sessionId must be string (flat-mode), got: {:?}",
        result["sessionId"]
    );
}

#[test]
fn target_attach_to_target_missing_target_id_returns_32602() {
    // @trace REQ-BAO-API-007 [domain:Target] [level:integration]
    let b = backend();
    let err = dispatch_command(&*b, "Target.attachToTarget", json!({}), "1").unwrap_err();
    assert!(matches!(err, BridgeError::InvalidParams(_)));
    assert_eq!(err.cdp_error_code(), -32602);
}

#[test]
fn target_attach_to_target_with_flatten_param_accepted() {
    // Arrange — CDP 规范: flatten (bool) 可选,默认 false
    // @trace REQ-CDP-001 [domain:Target] [level:integration]
    let b = backend();
    let result = dispatch_command(
        &*b,
        "Target.attachToTarget",
        json!({"targetId":"1", "flatten":true}),
        "1",
    )
    .unwrap();
    assert!(result["sessionId"].is_string());
}

// ─────────────────────────────────────────────────────────────────────────
// Target.detachFromTarget — CDP spec: empty return {}
// params: {sessionId?} or {targetId?} (one of)
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn target_detach_from_target_with_session_id_returns_empty() {
    // @trace REQ-CDP-001 [domain:Target] [level:integration]
    let b = backend();
    let result = dispatch_command(
        &*b,
        "Target.detachFromTarget",
        json!({"sessionId":"1-session"}),
        "1",
    )
    .unwrap();
    assert!(
        result.as_object().map(|o| o.is_empty()).unwrap_or(false),
        "CDP spec: detachFromTarget returns empty object"
    );
}

#[test]
fn target_detach_from_target_with_target_id_returns_empty() {
    // Arrange — CDP 规范: sessionId 或 targetId 二选一
    // @trace REQ-CDP-001 [domain:Target] [level:integration]
    let b = backend();
    let result = dispatch_command(
        &*b,
        "Target.detachFromTarget",
        json!({"targetId":"1"}),
        "1",
    )
    .unwrap();
    assert!(result.as_object().map(|o| o.is_empty()).unwrap_or(false));
}

#[test]
fn target_detach_from_target_missing_both_returns_32602() {
    // @trace REQ-BAO-API-007 [domain:Target] [level:integration]
    let b = backend();
    let err = dispatch_command(&*b, "Target.detachFromTarget", json!({}), "1").unwrap_err();
    assert!(matches!(err, BridgeError::InvalidParams(_)));
    assert_eq!(err.cdp_error_code(), -32602);
}

// ─────────────────────────────────────────────────────────────────────────
// Target.setAutoAttach — CDP spec: empty return {}
// params: {autoAttach (required), waitForDebuggerOnStart (required)}
// https://chromedevtools.github.io/devtools-protocol/tot/Target/#method-setAutoAttach
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn target_set_auto_attach_returns_empty_object() {
    // @trace REQ-CDP-001 [domain:Target] [level:integration]
    let b = backend();
    let result = dispatch_command(
        &*b,
        "Target.setAutoAttach",
        json!({"autoAttach":true, "waitForDebuggerOnStart":false}),
        "1",
    )
    .unwrap();
    assert!(
        result.as_object().map(|o| o.is_empty()).unwrap_or(false),
        "CDP spec: setAutoAttach returns empty object"
    );
}

#[test]
fn target_set_auto_attach_default_params_accepted() {
    // Arrange — CDP 规范: autoAttach / waitForDebuggerOnStart 默认 false
    // @trace REQ-CDP-001 [domain:Target] [level:integration]
    let b = backend();
    let result = dispatch_command(&*b, "Target.setAutoAttach", json!({}), "1").unwrap();
    assert!(result.as_object().map(|o| o.is_empty()).unwrap_or(false));
}
