//! A 类 48 method 处理器 — 机械映射 CDP method 到 `ServoBackend` 调用。
//!
//! 每个 handler 完成三步:
//! 1. 从 `serde_json::Value` 抽取参数(类型安全)
//! 2. 调用 `ServoBackend` 对应方法
//! 3. 把 backend 返回值组装为 CDP-compatible JSON 响应
//!
//! # 列表(按 domain 分组,共 48 method)
//!
//! | Domain | Methods |
//! |--------|---------|
//! | Page | navigate, reload, captureScreenshot, getFrameTree, getNavigationHistory, navigateToHistoryEntry, setContent, close, bringToFront, getLayoutMetrics, printToPDF (11) |
//! | Runtime | evaluate, callFunctionOn, getProperties, releaseObject, enable, disable (6) |
//! | DOM | getDocument, querySelector, querySelectorAll, getBoxModel, resolveNode, describeNode, setAttributeValue, removeAttribute, getOuterHTML, setOuterHTML, requestNode (11) |
//! | Network | enable, disable, getResponseBody, setCacheDisabled (4) |
//! | Input | dispatchMouseEvent, dispatchKeyEvent, dispatchTouchEvent, setIgnoreInputEvents (4) |
//! | Emulation | setDeviceMetricsOverride, clearDeviceMetricsOverride, setUserAgentOverride, setGeolocationOverride (4) |
//! | Target | getTargets, createTarget, closeTarget, attachToTarget, detachFromTarget, setAutoAttach (6) |
//! | CSS | getComputedStyleForNode, getMatchedStylesForNode (2) |
//!
//! @trace REQ-BAO-API-004 [level:library]

use serde_json::{json, Value};

use super::error::BridgeError;
use super::servo_backend::{
    CSSComputedStyleProperty, DeviceMetrics, EvaluateResult, ExceptionDetails, Frame, FrameTree,
    KeyEvent, MatchedStyles, MouseEvent, NavigateResult, NavigationEntry, NavigationHistory,
    NodeDescriptor, PropertyDescriptor, RemoteObject, ResponseBody, TargetInfo, TouchPoint,
};

// ────────────────────────────────────────────────────────────────────
// 通用 JSON 助手 — 集中处理"字段缺失/类型错误"错误。
// ────────────────────────────────────────────────────────────────────

/// 从 params 抽取字符串字段,缺失时返回 InvalidParams。
fn get_str(params: &Value, key: &str) -> Result<String, BridgeError> {
    params
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| BridgeError::InvalidParams(format!("missing string field: {key}")))
}

/// 从 params 抽取可选字符串字段(允许缺失)。
fn get_opt_str(params: &Value, key: &str) -> Option<String> {
    params.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}

/// 从 params 抽取可选 i64 字段(允许缺失,缺失返回 default)。
fn get_opt_i64(params: &Value, key: &str, default: i64) -> i64 {
    params.get(key).and_then(|v| v.as_i64()).unwrap_or(default)
}

/// 从 params 抽取可选 f64 字段。
fn get_opt_f64(params: &Value, key: &str, default: f64) -> f64 {
    params.get(key).and_then(|v| v.as_f64()).unwrap_or(default)
}

/// 从 params 抽取可选 bool 字段。
fn get_opt_bool(params: &Value, key: &str, default: bool) -> bool {
    params.get(key).and_then(|v| v.as_bool()).unwrap_or(default)
}

/// 从 params 抽取必填 i64。
fn get_i64(params: &Value, key: &str) -> Result<i64, BridgeError> {
    params
        .get(key)
        .and_then(|v| v.as_i64())
        .ok_or_else(|| BridgeError::InvalidParams(format!("missing int field: {key}")))
}

/// 从 params 抽取必填 f64。
#[allow(dead_code)]
fn get_f64(params: &Value, key: &str) -> Result<f64, BridgeError> {
    params
        .get(key)
        .and_then(|v| v.as_f64())
        .ok_or_else(|| BridgeError::InvalidParams(format!("missing float field: {key}")))
}

/// 从 params 抽取 array 字段(返回 Value::Array clone)。
fn get_array<'a>(params: &'a Value, key: &str) -> Result<&'a Vec<Value>, BridgeError> {
    params
        .get(key)
        .and_then(|v| v.as_array())
        .ok_or_else(|| BridgeError::InvalidParams(format!("missing array field: {key}")))
}

// ────────────────────────────────────────────────────────────────────
// Page domain — 11 method
// ────────────────────────────────────────────────────────────────────

// @trace REQ-BAO-API-004 [domain:Page]
pub fn page_navigate(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let url = get_str(params, "url")?;
    let r = backend.page_navigate(target_id, &url)?;
    Ok(navigate_result_to_json(&r))
}

// @trace REQ-BAO-API-004 [domain:Page]
pub fn page_reload(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    backend.page_reload(target_id)?;
    Ok(Value::Object(Default::default()))
}

// @trace REQ-BAO-API-004 [domain:Page]
pub fn page_capture_screenshot(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let fmt_str = get_opt_str(params, "format");
    let fmt = super::servo_backend::BridgeScreenshotFormat::from_cdp(fmt_str.as_deref());
    let bytes = backend.page_screenshot(target_id, fmt)?;
    let b64 = base64_encode(&bytes);
    Ok(json!({ "data": b64 }))
}

// @trace REQ-BAO-API-004 [domain:Page]
pub fn page_get_frame_tree(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    let tree = backend.page_frame_tree(target_id)?;
    Ok(json!({ "frameTree": frame_tree_to_json(&tree) }))
}

// @trace REQ-BAO-API-004 [domain:Page]
pub fn page_get_navigation_history(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    let h = backend.page_navigation_history(target_id)?;
    Ok(navigation_history_to_json(&h))
}

// @trace REQ-BAO-API-004 [domain:Page]
pub fn page_navigate_to_history_entry(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let entry_id = get_i64(params, "entryId")?;
    backend.page_navigate_to_history_entry(target_id, entry_id)?;
    Ok(Value::Object(Default::default()))
}

// @trace REQ-BAO-API-004 [domain:Page]
pub fn page_set_content(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let html = get_str(params, "html")?;
    backend.page_set_content(target_id, &html)?;
    Ok(Value::Object(Default::default()))
}

// @trace REQ-BAO-API-004 [domain:Page]
pub fn page_close(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    backend.page_close(target_id)?;
    Ok(Value::Object(Default::default()))
}

// @trace REQ-BAO-API-004 [domain:Page]
pub fn page_bring_to_front(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    backend.page_bring_to_front(target_id)?;
    Ok(Value::Object(Default::default()))
}

// @trace REQ-BAO-API-004 [domain:Page]
pub fn page_get_layout_metrics(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    let m = backend.page_layout_metrics(target_id)?;
    Ok(layout_metrics_to_json(&m))
}

// @trace REQ-BAO-API-004 [domain:Page]
pub fn page_print_to_pdf(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    let bytes = backend.page_print_to_pdf(target_id)?;
    let b64 = base64_encode(&bytes);
    Ok(json!({ "data": b64 }))
}

// ────────────────────────────────────────────────────────────────────
// Runtime domain — 6 method
// ────────────────────────────────────────────────────────────────────

// @trace REQ-BAO-API-004 [domain:Runtime]
pub fn runtime_evaluate(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let expr = get_str(params, "expression")?;
    let r = backend.runtime_evaluate(target_id, &expr)?;
    Ok(evaluate_result_to_json(&r))
}

// @trace REQ-BAO-API-004 [domain:Runtime]
pub fn runtime_call_function_on(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let object_id = get_str(params, "objectId")?;
    let function_declaration = get_str(params, "functionDeclaration")?;
    let args: Vec<Value> = params
        .get("arguments")
        .and_then(|v| v.as_array())
        .map(|a| a.clone())
        .unwrap_or_default();
    let r = backend.runtime_call_function_on(
        target_id,
        &object_id,
        &function_declaration,
        &args,
    )?;
    Ok(evaluate_result_to_json(&r))
}

// @trace REQ-BAO-API-004 [domain:Runtime]
pub fn runtime_get_properties(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let object_id = get_str(params, "objectId")?;
    let own = get_opt_bool(params, "ownProperties", true);
    let props = backend.runtime_get_properties(target_id, &object_id, own)?;
    let arr: Vec<Value> = props.iter().map(property_descriptor_to_json).collect();
    Ok(json!({ "result": arr, "internalProperties": [] }))
}

// @trace REQ-BAO-API-004 [domain:Runtime]
pub fn runtime_release_object(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let object_id = get_str(params, "objectId")?;
    backend.runtime_release_object(target_id, &object_id)?;
    Ok(Value::Object(Default::default()))
}

// @trace REQ-BAO-API-004 [domain:Runtime]
pub fn runtime_enable(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    backend.runtime_enable(target_id)?;
    Ok(Value::Object(Default::default()))
}

// @trace REQ-BAO-API-004 [domain:Runtime]
pub fn runtime_disable(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    backend.runtime_disable(target_id)?;
    Ok(Value::Object(Default::default()))
}

// ────────────────────────────────────────────────────────────────────
// DOM domain — 11 method
// ────────────────────────────────────────────────────────────────────

// @trace REQ-BAO-API-004 [domain:DOM]
pub fn dom_get_document(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let depth = get_opt_i64(params, "depth", 1);
    let node = backend.dom_get_document(target_id, depth)?;
    Ok(json!({ "root": node_descriptor_to_json(&node) }))
}

// @trace REQ-BAO-API-004 [domain:DOM]
pub fn dom_query_selector(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let node_id = get_i64(params, "nodeId")?;
    let selector = get_str(params, "selector")?;
    let found = backend.dom_query_selector(target_id, node_id, &selector)?;
    Ok(json!({ "nodeId": found.unwrap_or(0) }))
}

// @trace REQ-BAO-API-004 [domain:DOM]
pub fn dom_query_selector_all(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let node_id = get_i64(params, "nodeId")?;
    let selector = get_str(params, "selector")?;
    let ids = backend.dom_query_selector_all(target_id, node_id, &selector)?;
    Ok(json!({ "nodeIds": ids }))
}

// @trace REQ-BAO-API-004 [domain:DOM]
pub fn dom_get_box_model(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let node_id = get_i64(params, "nodeId")?;
    let m = backend.dom_get_box_model(target_id, node_id)?;
    Ok(box_model_to_json(&m))
}

// @trace REQ-BAO-API-004 [domain:DOM]
pub fn dom_resolve_node(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let backend_node_id = get_i64(params, "backendNodeId")?;
    let obj = backend.dom_resolve_node(target_id, backend_node_id)?;
    Ok(json!({ "object": remote_object_to_json(&obj) }))
}

// @trace REQ-BAO-API-004 [domain:DOM]
pub fn dom_describe_node(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let node_id = get_i64(params, "nodeId")?;
    let depth = get_opt_i64(params, "depth", 1);
    let node = backend.dom_describe_node(target_id, node_id, depth)?;
    Ok(json!({ "node": node_descriptor_to_json(&node) }))
}

// @trace REQ-BAO-API-004 [domain:DOM]
pub fn dom_set_attribute_value(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let node_id = get_i64(params, "nodeId")?;
    let name = get_str(params, "name")?;
    let value = get_str(params, "value")?;
    backend.dom_set_attribute(target_id, node_id, &name, &value)?;
    Ok(Value::Object(Default::default()))
}

// @trace REQ-BAO-API-004 [domain:DOM]
pub fn dom_remove_attribute(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let node_id = get_i64(params, "nodeId")?;
    let name = get_str(params, "name")?;
    backend.dom_remove_attribute(target_id, node_id, &name)?;
    Ok(Value::Object(Default::default()))
}

// @trace REQ-BAO-API-004 [domain:DOM]
pub fn dom_get_outer_html(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let node_id = get_i64(params, "nodeId")?;
    let html = backend.dom_get_outer_html(target_id, node_id)?;
    Ok(json!({ "outerHTML": html }))
}

// @trace REQ-BAO-API-004 [domain:DOM]
pub fn dom_set_outer_html(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let node_id = get_i64(params, "nodeId")?;
    let html = get_str(params, "html")?;
    backend.dom_set_outer_html(target_id, node_id, &html)?;
    Ok(Value::Object(Default::default()))
}

// @trace REQ-BAO-API-004 [domain:DOM]
pub fn dom_request_node(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let object_id = get_str(params, "objectId")?;
    let node_id = backend.dom_request_node(target_id, &object_id)?;
    Ok(json!({ "nodeId": node_id }))
}

// ────────────────────────────────────────────────────────────────────
// Network domain — 4 method
// ────────────────────────────────────────────────────────────────────

// @trace REQ-BAO-API-004 [domain:Network]
pub fn network_enable(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    backend.network_enable(target_id)?;
    Ok(Value::Object(Default::default()))
}

// @trace REQ-BAO-API-004 [domain:Network]
pub fn network_disable(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    backend.network_disable(target_id)?;
    Ok(Value::Object(Default::default()))
}

// @trace REQ-BAO-API-004 [domain:Network]
pub fn network_get_response_body(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let request_id = get_str(params, "requestId")?;
    let r = backend.network_get_response_body(target_id, &request_id)?;
    Ok(response_body_to_json(&r))
}

// @trace REQ-BAO-API-004 [domain:Network]
pub fn network_set_cache_disabled(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let disabled = get_opt_bool(params, "cacheDisabled", false);
    backend.network_set_cache_disabled(target_id, disabled)?;
    Ok(Value::Object(Default::default()))
}

// ────────────────────────────────────────────────────────────────────
// Input domain — 4 method
// ────────────────────────────────────────────────────────────────────

// @trace REQ-BAO-API-004 [domain:Input]
pub fn input_dispatch_mouse_event(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let event = parse_mouse_event(params)?;
    backend.input_dispatch_mouse_event(target_id, event)?;
    Ok(Value::Object(Default::default()))
}

// @trace REQ-BAO-API-004 [domain:Input]
pub fn input_dispatch_key_event(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let event = parse_key_event(params)?;
    backend.input_dispatch_key_event(target_id, event)?;
    Ok(Value::Object(Default::default()))
}

// @trace REQ-BAO-API-004 [domain:Input]
pub fn input_dispatch_touch_event(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let event_type = get_str(params, "type")?;
    let touch_points = parse_touch_points(params)?;
    backend.input_dispatch_touch_event(target_id, &event_type, &touch_points)?;
    Ok(Value::Object(Default::default()))
}

// @trace REQ-BAO-API-004 [domain:Input]
pub fn input_set_ignore_input_events(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let ignore = get_opt_bool(params, "ignore", false);
    backend.input_set_ignore_input_events(target_id, ignore)?;
    Ok(Value::Object(Default::default()))
}

// ────────────────────────────────────────────────────────────────────
// Emulation domain — 4 method
// ────────────────────────────────────────────────────────────────────

// @trace REQ-BAO-API-004 [domain:Emulation]
pub fn emulation_set_device_metrics_override(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let metrics = parse_device_metrics(params)?;
    backend.emulation_set_device_metrics(target_id, metrics)?;
    Ok(Value::Object(Default::default()))
}

// @trace REQ-BAO-API-004 [domain:Emulation]
pub fn emulation_clear_device_metrics_override(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    backend.emulation_clear_device_metrics(target_id)?;
    Ok(Value::Object(Default::default()))
}

// @trace REQ-BAO-API-004 [domain:Emulation]
pub fn emulation_set_user_agent_override(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let ua = get_str(params, "userAgent")?;
    backend.emulation_set_user_agent_override(target_id, &ua)?;
    Ok(Value::Object(Default::default()))
}

// @trace REQ-BAO-API-004 [domain:Emulation]
pub fn emulation_set_geolocation_override(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    // All fields optional — defaults follow CDP spec.
    let latitude = get_opt_f64(params, "latitude", 0.0);
    let longitude = get_opt_f64(params, "longitude", 0.0);
    let accuracy = get_opt_f64(params, "accuracy", 100.0);
    backend.emulation_set_geolocation_override(target_id, latitude, longitude, accuracy)?;
    Ok(Value::Object(Default::default()))
}

// ────────────────────────────────────────────────────────────────────
// Target domain — 6 method
// ────────────────────────────────────────────────────────────────────

// @trace REQ-BAO-API-004 [domain:Target]
pub fn target_get_targets(
    backend: &dyn super::servo_backend::ServoBackend,
    _target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    let targets = backend.target_get_targets()?;
    let arr: Vec<Value> = targets.iter().map(target_info_to_json).collect();
    Ok(json!({ "targetInfos": arr }))
}

// @trace REQ-BAO-API-004 [domain:Target]
pub fn target_create_target(
    backend: &dyn super::servo_backend::ServoBackend,
    _target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let url = get_str(params, "url")?;
    let new_id = backend.target_create_target(&url)?;
    Ok(json!({ "targetId": new_id }))
}

// @trace REQ-BAO-API-004 [domain:Target]
pub fn target_close_target(
    backend: &dyn super::servo_backend::ServoBackend,
    _target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let id = get_str(params, "targetId")?;
    backend.target_close_target(&id)?;
    Ok(json!({ "success": true }))
}

// @trace REQ-BAO-API-004 [domain:Target]
pub fn target_attach_to_target(
    backend: &dyn super::servo_backend::ServoBackend,
    _target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let id = get_str(params, "targetId")?;
    let session_id = backend.target_attach_to_target(&id)?;
    Ok(json!({ "sessionId": session_id }))
}

// @trace REQ-BAO-API-004 [domain:Target]
pub fn target_detach_from_target(
    backend: &dyn super::servo_backend::ServoBackend,
    _target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let session_id = params
        .get("sessionId")
        .or_else(|| params.get("targetId"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| BridgeError::InvalidParams("missing sessionId/targetId".into()))?;
    backend.target_detach_from_target(session_id)?;
    Ok(Value::Object(Default::default()))
}

// @trace REQ-BAO-API-004 [domain:Target]
pub fn target_set_auto_attach(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let auto_attach = get_opt_bool(params, "autoAttach", false);
    let wait = get_opt_bool(params, "waitForDebuggerOnStart", false);
    backend.target_set_auto_attach(target_id, auto_attach, wait)?;
    Ok(Value::Object(Default::default()))
}

// ────────────────────────────────────────────────────────────────────
// CSS domain — 2 method
// ────────────────────────────────────────────────────────────────────

// @trace REQ-BAO-API-004 [domain:CSS]
pub fn css_get_computed_style_for_node(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let node_id = get_i64(params, "nodeId")?;
    let props = backend.css_get_computed_style_for_node(target_id, node_id)?;
    let arr: Vec<Value> = props.iter().map(css_computed_prop_to_json).collect();
    Ok(json!({ "computedStyle": arr }))
}

// @trace REQ-BAO-API-004 [domain:CSS]
pub fn css_get_matched_styles_for_node(
    backend: &dyn super::servo_backend::ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let node_id = get_i64(params, "nodeId")?;
    let m = backend.css_get_matched_styles_for_node(target_id, node_id)?;
    Ok(matched_styles_to_json(&m))
}

// ────────────────────────────────────────────────────────────────────
// JSON 序列化助手 — Rust struct → CDP-compatible JSON。
// ────────────────────────────────────────────────────────────────────

fn navigate_result_to_json(r: &NavigateResult) -> Value {
    json!({
        "frameId": r.frame_id,
        "loaderId": r.loader_id,
    })
}

fn frame_to_json(f: &Frame) -> Value {
    json!({
        "id": f.id,
        "parentId": f.parent_id,
        "loaderId": f.loader_id,
        "name": f.name,
        "url": f.url,
        "securityOrigin": f.security_origin,
        "mimeType": f.mime_type,
    })
}

fn frame_tree_to_json(t: &FrameTree) -> Value {
    let children: Vec<Value> = t.child_frames.iter().map(frame_tree_to_json).collect();
    json!({
        "frame": frame_to_json(&t.frame),
        "childFrames": children,
    })
}

fn navigation_history_to_json(h: &NavigationHistory) -> Value {
    let entries: Vec<Value> = h
        .entries
        .iter()
        .map(|e: &NavigationEntry| {
            json!({ "id": e.id, "url": e.url, "title": e.title })
        })
        .collect();
    json!({
        "currentIndex": h.current_index,
        "entries": entries,
    })
}

fn layout_metrics_to_json(m: &super::servo_backend::LayoutMetrics) -> Value {
    json!({
        "layoutViewport": {
            "pageX": 0,
            "pageY": 0,
            "clientWidth": m.layout_width,
            "clientHeight": m.layout_height,
        },
        "visualViewport": {
            "offsetX": 0,
            "offsetY": 0,
            "pageX": 0,
            "pageY": 0,
            "clientWidth": m.layout_width,
            "clientHeight": m.layout_height,
            "scale": 1,
        },
        "contentSize": {
            "x": 0,
            "y": 0,
            "width": m.content_width,
            "height": m.content_height,
        },
    })
}

fn remote_object_to_json(o: &RemoteObject) -> Value {
    let mut v = json!({
        "type": o.type_,
    });
    if let Some(s) = &o.subtype {
        v["subtype"] = Value::String(s.clone());
    }
    if let Some(s) = &o.class_name {
        v["className"] = Value::String(s.clone());
    }
    if let Some(val) = &o.value {
        v["value"] = val.clone();
    }
    if let Some(s) = &o.unserializable_value {
        v["unserializableValue"] = Value::String(s.clone());
    }
    if let Some(s) = &o.description {
        v["description"] = Value::String(s.clone());
    }
    if let Some(s) = &o.object_id {
        v["objectId"] = Value::String(s.clone());
    }
    v
}

fn exception_details_to_json(e: &ExceptionDetails) -> Value {
    let mut v = json!({
        "exceptionId": e.exception_id,
        "text": e.text,
        "lineNumber": e.line_number,
        "columnNumber": e.column_number,
    });
    if let Some(exc) = &e.exception {
        v["exception"] = remote_object_to_json(exc);
    }
    v
}

fn evaluate_result_to_json(r: &EvaluateResult) -> Value {
    let mut v = json!({ "result": remote_object_to_json(&r.result) });
    if let Some(e) = &r.exception_details {
        v["exceptionDetails"] = exception_details_to_json(e);
    }
    v
}

fn property_descriptor_to_json(p: &PropertyDescriptor) -> Value {
    let mut v = json!({
        "name": p.name,
        "isOwn": p.is_own,
        "configurable": p.configurable,
        "enumerable": p.enumerable,
    });
    if let Some(val) = &p.value {
        v["value"] = remote_object_to_json(val);
    }
    if let Some(b) = p.writable {
        v["writable"] = Value::Bool(b);
    }
    if let Some(g) = &p.get {
        v["get"] = remote_object_to_json(g);
    }
    if let Some(s) = &p.set {
        v["set"] = remote_object_to_json(s);
    }
    if let Some(sym) = &p.symbol {
        v["symbol"] = remote_object_to_json(sym);
    }
    v
}

fn node_descriptor_to_json(n: &NodeDescriptor) -> Value {
    let children: Vec<Value> = n.children.iter().map(node_descriptor_to_json).collect();
    json!({
        "nodeId": n.node_id,
        "parentId": 0,
        "backendNodeId": n.backend_node_id,
        "nodeType": 1,
        "nodeName": n.node_name,
        "nodeValue": n.node_value,
        "children": children,
    })
}

fn box_model_to_json(m: &super::servo_backend::BoxModel) -> Value {
    json!({
        "content": m.content.clone(),
        "padding": m.padding.clone(),
        "border": m.border.clone(),
        "margin": m.margin.clone(),
        "width": m.width,
        "height": m.height,
    })
}

fn response_body_to_json(r: &ResponseBody) -> Value {
    json!({
        "body": r.body,
        "base64Encoded": r.base64_encoded,
    })
}

fn target_info_to_json(t: &TargetInfo) -> Value {
    let mut v = json!({
        "targetId": t.target_id,
        "type": t.type_,
        "title": t.title,
        "url": t.url,
        "attached": t.attached,
    });
    if let Some(id) = &t.browser_context_id {
        v["browserContextId"] = Value::String(id.clone());
    }
    v
}

fn css_computed_prop_to_json(p: &CSSComputedStyleProperty) -> Value {
    json!({ "name": p.name, "value": p.value })
}

fn css_property_to_json(p: &super::servo_backend::CSSProperty) -> Value {
    json!({ "name": p.name, "value": p.value, "important": p.important })
}

fn css_style_to_json(s: &super::servo_backend::CSSStyle) -> Value {
    let props: Vec<Value> = s.css_properties.iter().map(css_property_to_json).collect();
    json!({
        "styleSheetId": s.style_sheet_id,
        "cssProperties": props,
    })
}

fn matched_rule_to_json(r: &super::servo_backend::MatchedRule) -> Value {
    json!({
        "rule": {
            "selectorList": { "text": r.selector },
            "style": css_style_to_json(&r.style),
        }
    })
}

fn matched_styles_to_json(m: &MatchedStyles) -> Value {
    let mut v = json!({ "matchedRules": m.matched_rules.iter().map(matched_rule_to_json).collect::<Vec<_>>() });
    if let Some(s) = &m.inline_style {
        v["inlineStyle"] = css_style_to_json(s);
    }
    if let Some(s) = &m.attributes_style {
        v["attributesStyle"] = css_style_to_json(s);
    }
    v
}

// ────────────────────────────────────────────────────────────────────
// 参数解析助手 — Value → struct。
// ────────────────────────────────────────────────────────────────────

fn parse_mouse_event(params: &Value) -> Result<MouseEvent, BridgeError> {
    Ok(MouseEvent {
        event_type: get_str(params, "type")?,
        x: get_opt_f64(params, "x", 0.0),
        y: get_opt_f64(params, "y", 0.0),
        button: get_opt_str(params, "button").unwrap_or_else(|| "none".to_string()),
        click_count: get_opt_i64(params, "clickCount", 0),
        modifiers: get_opt_i64(params, "modifiers", 0),
    })
}

fn parse_key_event(params: &Value) -> Result<KeyEvent, BridgeError> {
    Ok(KeyEvent {
        event_type: get_str(params, "type")?,
        key: get_opt_str(params, "key").unwrap_or_default(),
        code: get_opt_str(params, "code").unwrap_or_default(),
        modifiers: get_opt_i64(params, "modifiers", 0),
        text: get_opt_str(params, "text").unwrap_or_default(),
        windows_virtual_key_code: get_opt_i64(params, "windowsVirtualKeyCode", 0),
    })
}

fn parse_touch_points(params: &Value) -> Result<Vec<TouchPoint>, BridgeError> {
    let arr = get_array(params, "touchPoints")?;
    let mut out = Vec::with_capacity(arr.len());
    for v in arr {
        out.push(TouchPoint {
            state: get_opt_str(v, "state").unwrap_or_else(|| "touchMoved".to_string()),
            x: get_opt_f64(v, "x", 0.0),
            y: get_opt_f64(v, "y", 0.0),
            radius_x: get_opt_f64(v, "radiusX", 1.0),
            radius_y: get_opt_f64(v, "radiusY", 1.0),
            force: get_opt_f64(v, "force", 1.0),
        });
    }
    Ok(out)
}

fn parse_device_metrics(params: &Value) -> Result<DeviceMetrics, BridgeError> {
    Ok(DeviceMetrics {
        width: get_opt_i64(params, "width", 0),
        height: get_opt_i64(params, "height", 0),
        device_scale_factor: get_opt_f64(params, "deviceScaleFactor", 1.0),
        mobile: get_opt_bool(params, "mobile", false),
    })
}

/// Base64 编码(无依赖,符合 RFC 4648)。
///
/// 这里手写是因为包内 base64 crate 未引入;若未来加 bun_base64,可替换。
fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    let mut i = 0;
    while i + 3 <= input.len() {
        let b0 = input[i] as u32;
        let b1 = input[i + 1] as u32;
        let b2 = input[i + 2] as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 6) & 0x3F) as usize] as char);
        out.push(TABLE[(n & 0x3F) as usize] as char);
        i += 3;
    }
    let rem = input.len() - i;
    if rem == 1 {
        let n = (input[i] as u32) << 16;
        out.push(TABLE[((n >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3F) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8);
        out.push(TABLE[((n >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 6) & 0x3F) as usize] as char);
        out.push('=');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::servo_backend::MockServoBackend;

    #[test]
    fn base64_encode_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn page_navigate_handler_returns_frame_id() {
        let b = MockServoBackend::new();
        let r = page_navigate(&b, "1", &json!({"url":"https://x"})).unwrap();
        assert_eq!(r["frameId"], "FRAME_0");
    }

    #[test]
    fn page_navigate_handler_missing_url_returns_invalid_params() {
        let b = MockServoBackend::new();
        let err = page_navigate(&b, "1", &json!({})).unwrap_err();
        assert!(matches!(err, BridgeError::InvalidParams(_)));
    }

    #[test]
    fn runtime_evaluate_handler_returns_remote_object() {
        let b = MockServoBackend::new();
        let r = runtime_evaluate(&b, "1", &json!({"expression":"1+1"})).unwrap();
        assert_eq!(r["result"]["type"], "string");
    }

    #[test]
    fn dom_query_selector_handler_returns_node_id() {
        let b = MockServoBackend::new();
        let r = dom_query_selector(&b, "1", &json!({"nodeId":1,"selector":"div"})).unwrap();
        assert_eq!(r["nodeId"], 2);
    }

    #[test]
    fn target_create_target_handler_returns_target_id() {
        let b = MockServoBackend::new();
        let r = target_create_target(&b, "1", &json!({"url":"about:blank"})).unwrap();
        let new_id = r["targetId"].as_str().unwrap();
        assert!(new_id.parse::<usize>().is_ok());
    }
}
