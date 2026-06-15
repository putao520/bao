//! Network domain conformance 审计 — 4 method。
//!
//! 对照 CDP 官方规范(Network domain):
//! https://chromedevtools.github.io/devtools-protocol/tot/Network/
//!
//! # 覆盖 method
//!
//! enable, disable, getResponseBody, setCacheDisabled
//!
//! @trace REQ-CDP-001 [domain:Network] [level:integration]
//! @trace REQ-BAO-API-004 [domain:Network] [level:integration]

use bao_cdp_client::{dispatch_command, BridgeError, MockServoBackend, ServoBackend};
use serde_json::json;
use std::sync::Arc;

fn backend() -> Arc<dyn ServoBackend> {
    Arc::new(MockServoBackend::new())
}

// ─────────────────────────────────────────────────────────────────────────
// Network.enable / disable — CDP spec: empty return {}
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn network_enable_returns_empty_object() {
    // @trace REQ-CDP-001 [domain:Network] [level:integration]
    let b = backend();
    let result = dispatch_command(&*b, "Network.enable", json!({}), "1").unwrap();
    assert!(
        result.as_object().map(|o| o.is_empty()).unwrap_or(false),
        "CDP spec: Network.enable returns empty object"
    );
}

#[test]
fn network_disable_returns_empty_object() {
    // @trace REQ-CDP-001 [domain:Network] [level:integration]
    let b = backend();
    let result = dispatch_command(&*b, "Network.disable", json!({}), "1").unwrap();
    assert!(
        result.as_object().map(|o| o.is_empty()).unwrap_or(false),
        "CDP spec: Network.disable returns empty object"
    );
}

#[test]
fn network_enable_optional_params_accepted() {
    // Arrange — CDP 规范: maxTotalBufferSize / maxResourceBufferSize / maxPostDataSize 可选
    // @trace REQ-CDP-001 [domain:Network] [level:integration]
    let b = backend();
    let result = dispatch_command(
        &*b,
        "Network.enable",
        json!({"maxTotalBufferSize":10_000_000, "maxPostDataSize":1024}),
        "1",
    )
    .unwrap();
    assert!(result.as_object().map(|o| o.is_empty()).unwrap_or(false));
}

// ─────────────────────────────────────────────────────────────────────────
// Network.getResponseBody — CDP spec: returns {body: string, base64Encoded: boolean}
// https://chromedevtools.github.io/devtools-protocol/tot/Network/#method-getResponseBody
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn network_get_response_body_result_schema_conformance() {
    // Arrange — CDP 规范: 返回 {body: string, base64Encoded: boolean}
    // @trace REQ-CDP-001 [domain:Network] [level:integration]
    let b = backend();

    // Act
    let result = dispatch_command(
        &*b,
        "Network.getResponseBody",
        json!({"requestId":"req1"}),
        "1",
    )
    .unwrap();

    // Assert
    assert!(
        result["body"].is_string(),
        "CDP spec: body must be string, got: {:?}",
        result["body"]
    );
    assert!(
        result["base64Encoded"].is_boolean(),
        "CDP spec: base64Encoded must be boolean, got: {:?}",
        result["base64Encoded"]
    );
}

#[test]
fn network_get_response_body_missing_request_id_returns_32602() {
    // @trace REQ-BAO-API-007 [domain:Network] [level:integration]
    let b = backend();
    let err = dispatch_command(&*b, "Network.getResponseBody", json!({}), "1").unwrap_err();
    assert!(matches!(err, BridgeError::InvalidParams(_)));
    assert_eq!(err.cdp_error_code(), -32602);
}

// ─────────────────────────────────────────────────────────────────────────
// Network.setCacheDisabled — CDP spec: empty return {}
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn network_set_cache_disabled_returns_empty_object() {
    // @trace REQ-CDP-001 [domain:Network] [level:integration]
    let b = backend();
    let result = dispatch_command(
        &*b,
        "Network.setCacheDisabled",
        json!({"cacheDisabled":true}),
        "1",
    )
    .unwrap();
    assert!(
        result.as_object().map(|o| o.is_empty()).unwrap_or(false),
        "CDP spec: setCacheDisabled returns empty object"
    );
}

#[test]
fn network_set_cache_disabled_default_param_accepted() {
    // Arrange — cacheDisabled 默认 false(缺省时应被接受)
    // @trace REQ-CDP-001 [domain:Network] [level:integration]
    let b = backend();
    let result = dispatch_command(&*b, "Network.setCacheDisabled", json!({}), "1").unwrap();
    assert!(result.as_object().map(|o| o.is_empty()).unwrap_or(false));
}
