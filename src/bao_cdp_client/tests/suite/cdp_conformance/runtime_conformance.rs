//! Runtime domain conformance 审计 — 6 method。
//!
//! 对照 CDP 官方规范(Runtime domain):
//! https://chromedevtools.github.io/devtools-protocol/tot/Runtime/
//!
//! # 覆盖 method
//!
//! evaluate, callFunctionOn, getProperties, releaseObject, enable, disable
//!
//! @trace REQ-CDP-001 [domain:Runtime] [level:integration]
//! @trace REQ-BAO-API-004 [domain:Runtime] [level:integration]

use bao_cdp_client::{dispatch_command, BridgeError, MockServoBackend, ServoBackend};
use serde_json::json;
use std::sync::Arc;

fn backend() -> Arc<dyn ServoBackend> {
    Arc::new(MockServoBackend::new())
}

// ─────────────────────────────────────────────────────────────────────────
// Runtime.evaluate — CDP spec: returns {result: RemoteObject, exceptionDetails?: ExceptionDetails}
// RemoteObject: {type, subtype?, className?, value?, unserializableValue?,
//   description?, objectId?, preview?, ...}
// https://chromedevtools.github.io/devtools-protocol/tot/Runtime/#method-evaluate
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn runtime_evaluate_result_schema_conformance() {
    // Arrange — CDP 规范: 返回 {result: RemoteObject, exceptionDetails?: object}
    // @trace REQ-CDP-001 [domain:Runtime] [level:integration]
    let b = backend();

    // Act
    let result =
        dispatch_command(&*b, "Runtime.evaluate", json!({"expression":"1+1"}), "1").unwrap();

    // Assert — CDP spec: result field is RemoteObject
    let remote = &result["result"];
    assert!(
        remote.is_object(),
        "CDP spec: result must be RemoteObject, got: {:?}",
        remote
    );
    // CDP spec: type is one of {object, function, undefined, string, number, boolean, symbol, bigint}
    assert!(
        remote["type"].is_string(),
        "CDP spec: RemoteObject.type must be string"
    );
    let t = remote["type"].as_str().unwrap();
    assert!(
        matches!(
            t,
            "object"
                | "function"
                | "undefined"
                | "string"
                | "number"
                | "boolean"
                | "symbol"
                | "bigint"
        ),
        "CDP spec: type must be valid RemoteObjectType, got: {}",
        t
    );
}

#[test]
fn runtime_evaluate_missing_expression_returns_32602() {
    // Arrange — CDP 规范: expression 必填
    // @trace REQ-BAO-API-007 [domain:Runtime] [level:integration]
    let b = backend();

    // Act
    let err = dispatch_command(&*b, "Runtime.evaluate", json!({}), "1").unwrap_err();

    // Assert
    assert!(matches!(err, BridgeError::InvalidParams(_)));
    assert_eq!(err.cdp_error_code(), -32602);
}

#[test]
fn runtime_evaluate_exception_details_schema_when_present() {
    // Arrange — CDP 规范: 异常时 exceptionDetails 字段出现
    // {exceptionId, text, lineNumber, columnNumber, exception?}
    // @trace REQ-CDP-001 [domain:Runtime] [level:integration]
    let b = backend();

    // Act — Mock backend 总是成功,exceptionDetails 应缺失或 null
    let result = dispatch_command(
        &*b,
        "Runtime.evaluate",
        json!({"expression":"throw new Error('x')"}),
        "1",
    )
    .unwrap();

    // Assert — Mock 无异常 → exceptionDetails 缺失或 null
    if let Some(ed) = result.get("exceptionDetails") {
        if !ed.is_null() {
            // 如果存在,必须符合 schema
            assert!(ed["exceptionId"].is_i64() || ed["exceptionId"].is_u64());
            assert!(ed["text"].is_string());
            assert!(ed["lineNumber"].is_number());
            assert!(ed["columnNumber"].is_number());
        }
    }
}

#[test]
fn runtime_evaluate_with_optional_params_accepted() {
    // Arrange — CDP 规范: 可选参数 includeCommandLineAPI / silent / returnByValue /
    // awaitPromise / userGesture / ... 均可选
    // @trace REQ-CDP-001 [domain:Runtime] [level:integration]
    let b = backend();

    // Act — 带可选参数(应被接受)
    let result = dispatch_command(
        &*b,
        "Runtime.evaluate",
        json!({"expression":"1", "returnByValue":true, "awaitPromise":false, "userGesture":true}),
        "1",
    )
    .unwrap();

    // Assert — 可选参数不影响 schema
    assert!(result["result"].is_object());
}

// ─────────────────────────────────────────────────────────────────────────
// Runtime.callFunctionOn — CDP spec: returns {result: RemoteObject, exceptionDetails?}
// params: {functionDeclaration: string (required), objectId?, arguments?, ...}
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn runtime_call_function_on_result_schema_conformance() {
    // Arrange — CDP 规范: 返回 {result: RemoteObject, exceptionDetails?}
    // @trace REQ-CDP-001 [domain:Runtime] [level:integration]
    let b = backend();

    // Act
    let result = dispatch_command(
        &*b,
        "Runtime.callFunctionOn",
        json!({"objectId":"obj1", "functionDeclaration":"function(){return 1;}"}),
        "1",
    )
    .unwrap();

    // Assert
    assert!(
        result["result"].is_object(),
        "CDP spec: result must be RemoteObject"
    );
    assert!(result["result"]["type"].is_string());
}

#[test]
fn runtime_call_function_on_missing_required_returns_32602() {
    // Arrange — CDP 规范: objectId 必填 + functionDeclaration 必填
    // @trace REQ-BAO-API-007 [domain:Runtime] [level:integration]
    let b = backend();

    // Act — 缺 functionDeclaration
    let err = dispatch_command(
        &*b,
        "Runtime.callFunctionOn",
        json!({"objectId":"obj1"}),
        "1",
    )
    .unwrap_err();

    // Assert
    assert!(matches!(err, BridgeError::InvalidParams(_)));
    assert_eq!(err.cdp_error_code(), -32602);
}

#[test]
fn runtime_call_function_on_missing_object_id_returns_32602() {
    // @trace REQ-BAO-API-007 [domain:Runtime] [level:integration]
    let b = backend();
    let err = dispatch_command(
        &*b,
        "Runtime.callFunctionOn",
        json!({"functionDeclaration":"function(){}"}),
        "1",
    )
    .unwrap_err();
    assert!(matches!(err, BridgeError::InvalidParams(_)));
    assert_eq!(err.cdp_error_code(), -32602);
}

#[test]
fn runtime_call_function_on_with_arguments_accepted() {
    // Arrange — CDP 规范: arguments 为可选 RemoteObject 数组
    // @trace REQ-CDP-001 [domain:Runtime] [level:integration]
    let b = backend();
    let result = dispatch_command(
        &*b,
        "Runtime.callFunctionOn",
        json!({
            "objectId":"o1",
            "functionDeclaration":"function(a,b){return a+b;}",
            "arguments":[{"value":1},{"value":2}]
        }),
        "1",
    )
    .unwrap();
    assert!(result["result"].is_object());
}

// ─────────────────────────────────────────────────────────────────────────
// Runtime.getProperties — CDP spec: returns {result: [PropertyDescriptor],
//   internalProperties?: [InternalPropertyDescriptor], exceptionDetails?}
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn runtime_get_properties_result_schema_conformance() {
    // Arrange — CDP 规范: 返回 {result: array, internalProperties?: array}
    // PropertyDescriptor: {name, value?, writable?, get?, set?, configurable, enumerable, ...}
    // @trace REQ-CDP-001 [domain:Runtime] [level:integration]
    let b = backend();

    // Act
    let result = dispatch_command(
        &*b,
        "Runtime.getProperties",
        json!({"objectId":"obj1"}),
        "1",
    )
    .unwrap();

    // Assert
    assert!(
        result["result"].is_array(),
        "CDP spec: result must be array of PropertyDescriptor"
    );
    assert!(
        result["internalProperties"].is_array(),
        "CDP spec: internalProperties should be array (may be empty)"
    );
}

#[test]
fn runtime_get_properties_missing_object_id_returns_32602() {
    // @trace REQ-BAO-API-007 [domain:Runtime] [level:integration]
    let b = backend();
    let err = dispatch_command(&*b, "Runtime.getProperties", json!({}), "1").unwrap_err();
    assert!(matches!(err, BridgeError::InvalidParams(_)));
    assert_eq!(err.cdp_error_code(), -32602);
}

#[test]
fn runtime_get_properties_property_descriptor_schema() {
    // Arrange — CDP 规范: 每个 PropertyDescriptor 必须有 name / configurable / enumerable
    // isOwn / symbol / value / get / set 可选
    // @trace REQ-CDP-001 [domain:Runtime] [level:integration]
    let b = backend();
    let result = dispatch_command(
        &*b,
        "Runtime.getProperties",
        json!({"objectId":"obj1"}),
        "1",
    )
    .unwrap();
    let props = result["result"].as_array().unwrap();
    // Mock 返回空数组 — 验证 schema 准备好(当真实数据出现时立即 conformance)
    for p in props {
        assert!(
            p["name"].is_string(),
            "PropertyDescriptor.name must be string"
        );
        assert!(
            p["configurable"].is_boolean(),
            "PropertyDescriptor.configurable must be boolean"
        );
        assert!(
            p["enumerable"].is_boolean(),
            "PropertyDescriptor.enumerable must be boolean"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Runtime.releaseObject — CDP spec: empty return {}
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn runtime_release_object_returns_empty_object() {
    // Arrange — CDP 规范: 无返回值(空对象)
    // @trace REQ-CDP-001 [domain:Runtime] [level:integration]
    let b = backend();

    // Act
    let result = dispatch_command(
        &*b,
        "Runtime.releaseObject",
        json!({"objectId":"obj1"}),
        "1",
    )
    .unwrap();

    // Assert
    assert!(
        result.as_object().map(|o| o.is_empty()).unwrap_or(false),
        "CDP spec: releaseObject returns empty object"
    );
}

#[test]
fn runtime_release_object_missing_object_id_returns_32602() {
    // @trace REQ-BAO-API-007 [domain:Runtime] [level:integration]
    let b = backend();
    let err = dispatch_command(&*b, "Runtime.releaseObject", json!({}), "1").unwrap_err();
    assert!(matches!(err, BridgeError::InvalidParams(_)));
    assert_eq!(err.cdp_error_code(), -32602);
}

// ─────────────────────────────────────────────────────────────────────────
// Runtime.enable / disable — CDP spec: empty return {}
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn runtime_enable_returns_empty_object() {
    // @trace REQ-CDP-001 [domain:Runtime] [level:integration]
    let b = backend();
    let result = dispatch_command(&*b, "Runtime.enable", json!({}), "1").unwrap();
    assert!(
        result.as_object().map(|o| o.is_empty()).unwrap_or(false),
        "CDP spec: Runtime.enable returns empty object"
    );
}

#[test]
fn runtime_disable_returns_empty_object() {
    // @trace REQ-CDP-001 [domain:Runtime] [level:integration]
    let b = backend();
    let result = dispatch_command(&*b, "Runtime.disable", json!({}), "1").unwrap();
    assert!(
        result.as_object().map(|o| o.is_empty()).unwrap_or(false),
        "CDP spec: Runtime.disable returns empty object"
    );
}
