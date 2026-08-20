//! Emulation domain conformance 审计 — 4 method。
//!
//! 对照 CDP 官方规范(Emulation domain):
//! https://chromedevtools.github.io/devtools-protocol/tot/Emulation/
//!
//! # 覆盖 method
//!
//! setDeviceMetricsOverride, clearDeviceMetricsOverride, setUserAgentOverride,
//! setGeolocationOverride
//!
//! @trace REQ-CDP-001 [domain:Emulation] [level:integration]
//! @trace REQ-BAO-API-004 [domain:Emulation] [level:integration]

use bao_cdp_client::{dispatch_command, BridgeError, MockServoBackend, ServoBackend};
use serde_json::json;
use std::sync::Arc;

fn backend() -> Arc<dyn ServoBackend> {
    Arc::new(MockServoBackend::new())
}

// ─────────────────────────────────────────────────────────────────────────
// Emulation.setDeviceMetricsOverride — CDP spec: empty return {}
// params: {width, height, deviceScaleFactor, mobile, ...}
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn emulation_set_device_metrics_override_returns_empty_object() {
    // @trace REQ-CDP-001 [domain:Emulation] [level:integration]
    let b = backend();
    let result = dispatch_command(
        &*b,
        "Emulation.setDeviceMetricsOverride",
        json!({
            "width":375,
            "height":667,
            "deviceScaleFactor":2,
            "mobile":true
        }),
        "1",
    )
    .unwrap();
    assert!(
        result.as_object().map(|o| o.is_empty()).unwrap_or(false),
        "CDP spec: setDeviceMetricsOverride returns empty object"
    );
}

#[test]
fn emulation_set_device_metrics_override_accepts_optional_fields() {
    // Arrange — CDP 规范: screenWidth / screenHeight / positionX / positionY 等可选
    // @trace REQ-CDP-001 [domain:Emulation] [level:integration]
    let b = backend();
    let result = dispatch_command(
        &*b,
        "Emulation.setDeviceMetricsOverride",
        json!({
            "width":800,
            "height":600,
            "deviceScaleFactor":1,
            "mobile":false,
            "screenWidth":1920,
            "screenHeight":1080
        }),
        "1",
    )
    .unwrap();
    assert!(result.as_object().map(|o| o.is_empty()).unwrap_or(false));
}

// ─────────────────────────────────────────────────────────────────────────
// Emulation.clearDeviceMetricsOverride — CDP spec: empty return {}
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn emulation_clear_device_metrics_override_returns_empty_object() {
    // @trace REQ-CDP-001 [domain:Emulation] [level:integration]
    let b = backend();
    let result =
        dispatch_command(&*b, "Emulation.clearDeviceMetricsOverride", json!({}), "1").unwrap();
    assert!(
        result.as_object().map(|o| o.is_empty()).unwrap_or(false),
        "CDP spec: clearDeviceMetricsOverride returns empty object"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Emulation.setUserAgentOverride — CDP spec: empty return {}
// params: {userAgent (required), acceptLanguage?, platform?, ...}
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn emulation_set_user_agent_override_returns_empty_object() {
    // @trace REQ-CDP-001 [domain:Emulation] [level:integration]
    let b = backend();
    let result = dispatch_command(
        &*b,
        "Emulation.setUserAgentOverride",
        json!({"userAgent":"Mozilla/5.0"}),
        "1",
    )
    .unwrap();
    assert!(
        result.as_object().map(|o| o.is_empty()).unwrap_or(false),
        "CDP spec: setUserAgentOverride returns empty object"
    );
}

#[test]
fn emulation_set_user_agent_override_missing_ua_returns_32602() {
    // @trace REQ-BAO-API-007 [domain:Emulation] [level:integration]
    let b = backend();
    let err = dispatch_command(&*b, "Emulation.setUserAgentOverride", json!({}), "1").unwrap_err();
    assert!(matches!(err, BridgeError::InvalidParams(_)));
    assert_eq!(err.cdp_error_code(), -32602);
}

#[test]
fn emulation_set_user_agent_override_with_optional_fields_accepted() {
    // Arrange — CDP 规范: acceptLanguage / platform / userAgentMetadata 可选
    // @trace REQ-CDP-001 [domain:Emulation] [level:integration]
    let b = backend();
    let result = dispatch_command(
        &*b,
        "Emulation.setUserAgentOverride",
        json!({
            "userAgent":"Mozilla/5.0",
            "acceptLanguage":"en-US",
            "platform":"Linux"
        }),
        "1",
    )
    .unwrap();
    assert!(result.as_object().map(|o| o.is_empty()).unwrap_or(false));
}

// ─────────────────────────────────────────────────────────────────────────
// Emulation.setGeolocationOverride — CDP spec: empty return {}
// params: {latitude?, longitude?, accuracy?} — all optional
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn emulation_set_geolocation_override_returns_empty_object() {
    // @trace REQ-CDP-001 [domain:Emulation] [level:integration]
    let b = backend();
    let result = dispatch_command(
        &*b,
        "Emulation.setGeolocationOverride",
        json!({"latitude":37.77, "longitude":-122.41, "accuracy":100}),
        "1",
    )
    .unwrap();
    assert!(
        result.as_object().map(|o| o.is_empty()).unwrap_or(false),
        "CDP spec: setGeolocationOverride returns empty object"
    );
}

#[test]
fn emulation_set_geolocation_override_all_optional_accepted() {
    // Arrange — CDP 规范: 所有字段可选(空参数合法)
    // @trace REQ-CDP-001 [domain:Emulation] [level:integration]
    let b = backend();
    let result = dispatch_command(&*b, "Emulation.setGeolocationOverride", json!({}), "1").unwrap();
    assert!(result.as_object().map(|o| o.is_empty()).unwrap_or(false));
}
