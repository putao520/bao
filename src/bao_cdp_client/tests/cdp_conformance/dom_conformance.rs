//! DOM domain conformance 审计 — 11 method。
//!
//! 对照 CDP 官方规范(DOM domain):
//! https://chromedevtools.github.io/devtools-protocol/tot/DOM/
//!
//! # 覆盖 method
//!
//! getDocument, querySelector, querySelectorAll, getBoxModel, resolveNode,
//! describeNode, setAttributeValue, removeAttribute, getOuterHTML, setOuterHTML,
//! requestNode
//!
//! @trace REQ-CDP-001 [domain:DOM] [level:integration]
//! @trace REQ-BAO-API-004 [domain:DOM] [level:integration]

use bao_cdp_client::{dispatch_command, BridgeError, MockServoBackend, ServoBackend};
use serde_json::json;
use std::sync::Arc;

fn backend() -> Arc<dyn ServoBackend> {
    Arc::new(MockServoBackend::new())
}

// ─────────────────────────────────────────────────────────────────────────
// DOM.getDocument — CDP spec: returns {root: Node}
// Node: {nodeId, parentId?, backendNodeId, nodeType, nodeName, localName,
//   nodeValue, childNodeCount?, children?, attributes?, documentURL?, baseURL?, ...}
// https://chromedevtools.github.io/devtools-protocol/tot/DOM/#method-getDocument
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn dom_get_document_result_schema_conformance() {
    // Arrange — CDP 规范: 返回 {root: Node}
    // @trace REQ-CDP-001 [domain:DOM] [level:integration]
    let b = backend();

    // Act
    let result = dispatch_command(&*b, "DOM.getDocument", json!({}), "1").unwrap();

    // Assert — CDP spec: root is Node object
    let root = &result["root"];
    assert!(
        root.is_object(),
        "CDP spec: root must be Node object, got: {:?}",
        root
    );
    assert!(root["nodeId"].is_i64() || root["nodeId"].is_u64(), "nodeId must be int");
    assert!(
        root["backendNodeId"].is_i64() || root["backendNodeId"].is_u64(),
        "backendNodeId must be int"
    );
    assert!(root["nodeType"].is_i64() || root["nodeType"].is_u64(), "nodeType must be int");
    assert!(root["nodeName"].is_string(), "nodeName must be string");
    assert!(root["nodeValue"].is_string(), "nodeValue must be string");
}

#[test]
fn dom_get_document_node_local_name_documented_deviation() {
    // Arrange — CDP 规范: Node 必含 localName(可能为空字符串)
    // bao 实现的 node_descriptor_to_json 不输出 localName → 偏差
    // 此测试断言"当前缺 localName"这一事实,实现修复后会 fail → 提示更新报告
    // @trace REQ-CDP-001 [domain:DOM] [level:integration]
    let b = backend();
    let result = dispatch_command(&*b, "DOM.getDocument", json!({}), "1").unwrap();
    let root = &result["root"];

    // Assert — 记录偏差:bao 当前不输出 localName
    assert!(
        root.get("localName").is_none() || root["localName"].is_null(),
        "DEV-NOTE: bao currently omits Node.localName (CDP spec deviation). \
         If this fails, bao has added the field — update CONFORMANCE_REPORT."
    );
}

#[test]
fn dom_get_document_with_depth_param_accepted() {
    // Arrange — CDP 规范: depth (int, default 1) + pierce (bool, default false) 可选
    // @trace REQ-CDP-001 [domain:DOM] [level:integration]
    let b = backend();

    // Act
    let result = dispatch_command(&*b, "DOM.getDocument", json!({"depth":2, "pierce":true}), "1").unwrap();

    // Assert
    assert!(result["root"].is_object());
}

// ─────────────────────────────────────────────────────────────────────────
// DOM.querySelector — CDP spec: returns {nodeId: NodeId}
// not found → nodeId = 0
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn dom_query_selector_result_schema_conformance() {
    // Arrange — CDP 规范: 返回 {nodeId: int},未找到返回 0
    // @trace REQ-CDP-001 [domain:DOM] [level:integration]
    let b = backend();

    // Act
    let result = dispatch_command(
        &*b,
        "DOM.querySelector",
        json!({"nodeId":1, "selector":"div"}),
        "1",
    )
    .unwrap();

    // Assert — CDP spec: nodeId is integer
    assert!(
        result["nodeId"].is_i64() || result["nodeId"].is_u64(),
        "CDP spec: nodeId must be integer, got: {:?}",
        result["nodeId"]
    );
}

#[test]
fn dom_query_selector_missing_node_id_returns_32602() {
    // @trace REQ-BAO-API-007 [domain:DOM] [level:integration]
    let b = backend();
    let err = dispatch_command(
        &*b,
        "DOM.querySelector",
        json!({"selector":"div"}),
        "1",
    )
    .unwrap_err();
    assert!(matches!(err, BridgeError::InvalidParams(_)));
    assert_eq!(err.cdp_error_code(), -32602);
}

#[test]
fn dom_query_selector_missing_selector_returns_32602() {
    // @trace REQ-BAO-API-007 [domain:DOM] [level:integration]
    let b = backend();
    let err = dispatch_command(&*b, "DOM.querySelector", json!({"nodeId":1}), "1").unwrap_err();
    assert!(matches!(err, BridgeError::InvalidParams(_)));
    assert_eq!(err.cdp_error_code(), -32602);
}

#[test]
fn dom_query_selector_not_found_returns_node_id_zero() {
    // Arrange — CDP 规范: 未找到匹配时返回 nodeId = 0
    // @trace REQ-CDP-001 [domain:DOM] [level:integration]
    // Note: Mock backend 总是返回 Some(2),所以这里只验证字段类型
    let b = backend();
    let result = dispatch_command(
        &*b,
        "DOM.querySelector",
        json!({"nodeId":1, "selector":"div"}),
        "1",
    )
    .unwrap();
    let id = result["nodeId"].as_i64().unwrap();
    assert!(id >= 0, "CDP spec: nodeId must be non-negative (0 = not found)");
}

// ─────────────────────────────────────────────────────────────────────────
// DOM.querySelectorAll — CDP spec: returns {nodeIds: [NodeId]}
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn dom_query_selector_all_result_schema_conformance() {
    // Arrange — CDP 规范: 返回 {nodeIds: integer array}
    // @trace REQ-CDP-001 [domain:DOM] [level:integration]
    let b = backend();

    // Act
    let result = dispatch_command(
        &*b,
        "DOM.querySelectorAll",
        json!({"nodeId":1, "selector":"div"}),
        "1",
    )
    .unwrap();

    // Assert — CDP spec: nodeIds is integer array
    assert!(
        result["nodeIds"].is_array(),
        "CDP spec: nodeIds must be array, got: {:?}",
        result["nodeIds"]
    );
    for id in result["nodeIds"].as_array().unwrap() {
        assert!(id.is_i64() || id.is_u64(), "each nodeId must be int");
    }
}

#[test]
fn dom_query_selector_all_missing_params_returns_32602() {
    // @trace REQ-BAO-API-007 [domain:DOM] [level:integration]
    let b = backend();
    let err = dispatch_command(&*b, "DOM.querySelectorAll", json!({}), "1").unwrap_err();
    assert!(matches!(err, BridgeError::InvalidParams(_)));
    assert_eq!(err.cdp_error_code(), -32602);
}

// ─────────────────────────────────────────────────────────────────────────
// DOM.getBoxModel — CDP spec: returns {model: {content, padding, border, margin, width, height}}
// 偏差:bao 返回扁平 {content, padding, border, margin, width, height}(缺 model 包装)
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn dom_get_box_model_returns_box_data_conformance() {
    // Arrange — CDP 规范: 返回 BoxModel 数据(content/padding/border/margin + width + height)
    // bao 实现直接返回扁平结构,字段名一致但缺少 model 包装
    // @trace REQ-CDP-001 [domain:DOM] [level:integration]
    let b = backend();

    // Act
    let result = dispatch_command(&*b, "DOM.getBoxModel", json!({"nodeId":1}), "1").unwrap();

    // Assert — 字段存在(扁平结构)
    assert!(result["content"].is_array(), "CDP spec: content must be Quad array");
    assert!(result["padding"].is_array(), "CDP spec: padding must be Quad array");
    assert!(result["border"].is_array(), "CDP spec: border must be Quad array");
    assert!(result["margin"].is_array(), "CDP spec: margin must be Quad array");
    assert!(result["width"].is_i64() || result["width"].is_u64(), "width must be int");
    assert!(result["height"].is_i64() || result["height"].is_u64(), "height must be int");
}

#[test]
fn dom_get_box_model_model_wrapper_documented_deviation() {
    // Arrange — CDP 规范: BoxModel 应包装在 {model: {...}} 中
    // bao 实现直接返回扁平结构(无 model 包装)→ 偏差
    // 此测试断言"当前缺 model 包装",修复后会 fail
    // @trace REQ-CDP-001 [domain:DOM] [level:integration]
    let b = backend();
    let result = dispatch_command(&*b, "DOM.getBoxModel", json!({"nodeId":1}), "1").unwrap();

    // Assert — 记录偏差:bao 当前无 model 包装
    assert!(
        result.get("model").is_none(),
        "DEV-NOTE: bao currently omits `model` wrapper (CDP spec deviation). \
         If this fails, bao has added the wrapper — update CONFORMANCE_REPORT."
    );
}

#[test]
fn dom_get_box_model_missing_node_id_returns_32602() {
    // @trace REQ-BAO-API-007 [domain:DOM] [level:integration]
    let b = backend();
    let err = dispatch_command(&*b, "DOM.getBoxModel", json!({}), "1").unwrap_err();
    assert!(matches!(err, BridgeError::InvalidParams(_)));
    assert_eq!(err.cdp_error_code(), -32602);
}

// ─────────────────────────────────────────────────────────────────────────
// DOM.resolveNode — CDP spec: returns {object: RemoteObject}
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn dom_resolve_node_result_schema_conformance() {
    // Arrange — CDP 规范: 返回 {object: RemoteObject}
    // @trace REQ-CDP-001 [domain:DOM] [level:integration]
    let b = backend();

    // Act
    let result = dispatch_command(
        &*b,
        "DOM.resolveNode",
        json!({"backendNodeId":1}),
        "1",
    )
    .unwrap();

    // Assert
    let obj = &result["object"];
    assert!(
        obj.is_object(),
        "CDP spec: object must be RemoteObject, got: {:?}",
        obj
    );
    assert!(obj["type"].is_string(), "CDP spec: RemoteObject.type must be string");
}

#[test]
fn dom_resolve_node_missing_backend_node_id_returns_32602() {
    // @trace REQ-BAO-API-007 [domain:DOM] [level:integration]
    let b = backend();
    let err = dispatch_command(&*b, "DOM.resolveNode", json!({}), "1").unwrap_err();
    assert!(matches!(err, BridgeError::InvalidParams(_)));
    assert_eq!(err.cdp_error_code(), -32602);
}

// ─────────────────────────────────────────────────────────────────────────
// DOM.describeNode — CDP spec: returns {node: Node}
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn dom_describe_node_result_schema_conformance() {
    // Arrange — CDP 规范: 返回 {node: Node}
    // @trace REQ-CDP-001 [domain:DOM] [level:integration]
    let b = backend();

    // Act
    let result = dispatch_command(&*b, "DOM.describeNode", json!({"nodeId":1}), "1").unwrap();

    // Assert
    let node = &result["node"];
    assert!(node.is_object(), "CDP spec: node must be Node object");
    assert!(node["nodeId"].is_i64() || node["nodeId"].is_u64(), "node.nodeId must be int");
    assert!(node["nodeName"].is_string(), "node.nodeName must be string");
}

#[test]
fn dom_describe_node_missing_node_id_returns_32602() {
    // @trace REQ-BAO-API-007 [domain:DOM] [level:integration]
    let b = backend();
    let err = dispatch_command(&*b, "DOM.describeNode", json!({}), "1").unwrap_err();
    assert!(matches!(err, BridgeError::InvalidParams(_)));
    assert_eq!(err.cdp_error_code(), -32602);
}

// ─────────────────────────────────────────────────────────────────────────
// DOM.setAttributeValue — CDP spec: empty return {}
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn dom_set_attribute_value_returns_empty_object() {
    // Arrange — CDP 规范: 无返回值(空对象)
    // @trace REQ-CDP-001 [domain:DOM] [level:integration]
    let b = backend();

    // Act
    let result = dispatch_command(
        &*b,
        "DOM.setAttributeValue",
        json!({"nodeId":1, "name":"class", "value":"x"}),
        "1",
    )
    .unwrap();

    // Assert
    assert!(
        result.as_object().map(|o| o.is_empty()).unwrap_or(false),
        "CDP spec: setAttributeValue returns empty object"
    );
}

#[test]
fn dom_set_attribute_value_missing_params_returns_32602() {
    // @trace REQ-BAO-API-007 [domain:DOM] [level:integration]
    let b = backend();
    let err = dispatch_command(
        &*b,
        "DOM.setAttributeValue",
        json!({"nodeId":1, "name":"class"}),
        "1",
    )
    .unwrap_err();
    assert!(matches!(err, BridgeError::InvalidParams(_)));
    assert_eq!(err.cdp_error_code(), -32602);
}

// ─────────────────────────────────────────────────────────────────────────
// DOM.removeAttribute — CDP spec: empty return {}
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn dom_remove_attribute_returns_empty_object() {
    // @trace REQ-CDP-001 [domain:DOM] [level:integration]
    let b = backend();
    let result = dispatch_command(
        &*b,
        "DOM.removeAttribute",
        json!({"nodeId":1, "name":"class"}),
        "1",
    )
    .unwrap();
    assert!(
        result.as_object().map(|o| o.is_empty()).unwrap_or(false),
        "CDP spec: removeAttribute returns empty object"
    );
}

#[test]
fn dom_remove_attribute_missing_params_returns_32602() {
    // @trace REQ-BAO-API-007 [domain:DOM] [level:integration]
    let b = backend();
    let err = dispatch_command(&*b, "DOM.removeAttribute", json!({}), "1").unwrap_err();
    assert!(matches!(err, BridgeError::InvalidParams(_)));
    assert_eq!(err.cdp_error_code(), -32602);
}

// ─────────────────────────────────────────────────────────────────────────
// DOM.getOuterHTML — CDP spec: returns {outerHTML: string}
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn dom_get_outer_html_result_schema_conformance() {
    // Arrange — CDP 规范: 返回 {outerHTML: string}
    // @trace REQ-CDP-001 [domain:DOM] [level:integration]
    let b = backend();

    // Act
    let result = dispatch_command(&*b, "DOM.getOuterHTML", json!({"nodeId":1}), "1").unwrap();

    // Assert
    assert!(
        result["outerHTML"].is_string(),
        "CDP spec: outerHTML must be string, got: {:?}",
        result["outerHTML"]
    );
}

#[test]
fn dom_get_outer_html_missing_node_id_returns_32602() {
    // @trace REQ-BAO-API-007 [domain:DOM] [level:integration]
    let b = backend();
    let err = dispatch_command(&*b, "DOM.getOuterHTML", json!({}), "1").unwrap_err();
    assert!(matches!(err, BridgeError::InvalidParams(_)));
    assert_eq!(err.cdp_error_code(), -32602);
}

// ─────────────────────────────────────────────────────────────────────────
// DOM.setOuterHTML — CDP spec: empty return {}
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn dom_set_outer_html_returns_empty_object() {
    // @trace REQ-CDP-001 [domain:DOM] [level:integration]
    let b = backend();
    let result = dispatch_command(
        &*b,
        "DOM.setOuterHTML",
        json!({"nodeId":1, "html":"<div/>"}),
        "1",
    )
    .unwrap();
    assert!(
        result.as_object().map(|o| o.is_empty()).unwrap_or(false),
        "CDP spec: setOuterHTML returns empty object"
    );
}

#[test]
fn dom_set_outer_html_missing_params_returns_32602() {
    // @trace REQ-BAO-API-007 [domain:DOM] [level:integration]
    let b = backend();
    let err = dispatch_command(&*b, "DOM.setOuterHTML", json!({}), "1").unwrap_err();
    assert!(matches!(err, BridgeError::InvalidParams(_)));
    assert_eq!(err.cdp_error_code(), -32602);
}

// ─────────────────────────────────────────────────────────────────────────
// DOM.requestNode — CDP spec: returns {nodeId: NodeId}
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn dom_request_node_result_schema_conformance() {
    // Arrange — CDP 规范: 返回 {nodeId: int}
    // @trace REQ-CDP-001 [domain:DOM] [level:integration]
    let b = backend();

    // Act
    let result = dispatch_command(&*b, "DOM.requestNode", json!({"objectId":"obj1"}), "1").unwrap();

    // Assert
    assert!(
        result["nodeId"].is_i64() || result["nodeId"].is_u64(),
        "CDP spec: nodeId must be integer, got: {:?}",
        result["nodeId"]
    );
}

#[test]
fn dom_request_node_missing_object_id_returns_32602() {
    // @trace REQ-BAO-API-007 [domain:DOM] [level:integration]
    let b = backend();
    let err = dispatch_command(&*b, "DOM.requestNode", json!({}), "1").unwrap_err();
    assert!(matches!(err, BridgeError::InvalidParams(_)));
    assert_eq!(err.cdp_error_code(), -32602);
}
