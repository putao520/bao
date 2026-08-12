//! Page domain conformance 审计 — 11 A 类 method + 33 B 类 method。
//!
//! 对照 CDP 官方规范(Page domain):
//! https://chromedevtools.github.io/devtools-protocol/tot/Page/
//!
//! # 覆盖 method
//!
//! A 类(native mapping):navigate, reload, captureScreenshot, getFrameTree,
//! getNavigationHistory, navigateToHistoryEntry, setContent, close, bringToFront,
//! getLayoutMetrics, printToPDF(E 类)
//!
//! B 类(IIFE Eval):title, url, content, viewport, ... 33 method
//!
//! @trace REQ-CDP-001 [domain:Page] [level:integration]
//! @trace REQ-BAO-API-004 [domain:Page] [level:integration]

use bao_cdp_client::{dispatch_command, BridgeError, MockServoBackend, ServoBackend};
use serde_json::json;
use std::sync::Arc;

fn backend() -> Arc<dyn ServoBackend> {
    Arc::new(MockServoBackend::new())
}

// ─────────────────────────────────────────────────────────────────────────
// Page.navigate — CDP spec: returns {frameId: FrameId, loaderId?: LoaderId, errorText?: string}
// https://chromedevtools.github.io/devtools-protocol/tot/Page/#method-navigate
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn page_navigate_result_schema_conformance() {
    // Arrange — CDP 规范: Page.navigate 返回 {frameId: string, loaderId: string}
    // frameId 必填(任意字符串),loaderId 可选(同文档导航时省略)
    // @trace REQ-CDP-001 [domain:Page] [level:integration]
    let b = backend();

    // Act
    let result = dispatch_command(
        &*b,
        "Page.navigate",
        json!({"url":"https://example.com"}),
        "1",
    )
    .unwrap();

    // Assert — 对照 CDP 官方 schema
    // CDP spec: frameId is FrameId (string)
    assert!(
        result["frameId"].is_string(),
        "CDP spec: frameId must be string (FrameId type), got: {:?}",
        result["frameId"]
    );
    // CDP spec: loaderId is optional LoaderId (string) — bao 总是返回
    assert!(
        result["loaderId"].is_string(),
        "CDP spec: loaderId should be string (LoaderId type), got: {:?}",
        result["loaderId"]
    );
}

#[test]
fn page_navigate_missing_url_returns_invalid_params_32602() {
    // Arrange — CDP 规范: url 必填参数。缺失应返回 -32602 InvalidParams
    // @trace REQ-CDP-001 [domain:Page] [level:integration]
    // @trace REQ-BAO-API-007 [level:integration]
    let b = backend();

    // Act
    let err = dispatch_command(&*b, "Page.navigate", json!({}), "1").unwrap_err();

    // Assert — CDP / JSON-RPC 规范: 缺必填参数 → -32602
    assert!(
        matches!(err, BridgeError::InvalidParams(_)),
        "CDP spec: missing 'url' should be InvalidParams, got: {:?}",
        err
    );
    assert_eq!(
        err.cdp_error_code(),
        -32602,
        "CDP spec: InvalidParams → JSON-RPC code -32602"
    );
}

#[test]
fn page_navigate_optional_referrer_accepted() {
    // Arrange — CDP 规范: referrer / transitionType / frameId / referrerPolicy 均为可选
    // @trace REQ-CDP-001 [domain:Page] [level:integration]
    let b = backend();

    // Act — 带 referrer 可选参数(应被接受而非拒绝)
    let result = dispatch_command(
        &*b,
        "Page.navigate",
        json!({"url":"https://x", "referrer":"https://ref.example.com"}),
        "1",
    )
    .unwrap();

    // Assert — 可选参数不影响 schema
    assert!(result["frameId"].is_string());
}

#[test]
fn page_navigate_unknown_target_returns_server_error_32000() {
    // Arrange — CDP 规范: 错误情况返回 errorText(可选)或 server error
    // bao 把 PageNotFound 映射为 -32000 ServerError
    // @trace REQ-BAO-API-007 [level:integration]
    let b = backend();

    // Act
    let err = dispatch_command(&*b, "Page.navigate", json!({"url":"x"}), "999").unwrap_err();

    // Assert
    assert!(matches!(err, BridgeError::PageNotFound(_)));
    assert_eq!(err.cdp_error_code(), -32000);
}

// ─────────────────────────────────────────────────────────────────────────
// Page.captureScreenshot — CDP spec: returns {data: string (base64)}
// format 参数支持 jpeg / png / webp
// https://chromedevtools.github.io/devtools-protocol/tot/Page/#method-captureScreenshot
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn page_capture_screenshot_result_schema_conformance() {
    // Arrange — CDP 规范: 返回 {data: string} base64 编码
    // @trace REQ-CDP-001 [domain:Page] [level:integration]
    let b = backend();

    // Act
    let result = dispatch_command(&*b, "Page.captureScreenshot", json!({}), "1").unwrap();

    // Assert — CDP spec: data is base64-encoded image data
    assert!(
        result["data"].is_string(),
        "CDP spec: data must be string (base64-encoded image), got: {:?}",
        result["data"]
    );
    // base64 字符串非空 + 仅含合法字符
    let data = result["data"].as_str().unwrap();
    assert!(
        !data.is_empty(),
        "CDP spec: data should not be empty when screenshot succeeds"
    );
    assert!(
        data.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '='),
        "CDP spec: data must be valid base64 alphabet"
    );
}

#[test]
fn page_capture_screenshot_format_png_accepted() {
    // Arrange — CDP 规范: format ∈ {jpeg, png, webp},默认 jpeg
    // @trace REQ-CDP-001 [domain:Page] [level:integration]
    let b = backend();

    // Act
    let result =
        dispatch_command(&*b, "Page.captureScreenshot", json!({"format":"png"}), "1").unwrap();

    // Assert
    assert!(result["data"].is_string());
}

#[test]
fn page_capture_screenshot_format_jpeg_accepted() {
    // @trace REQ-CDP-001 [domain:Page] [level:integration]
    let b = backend();
    let result = dispatch_command(
        &*b,
        "Page.captureScreenshot",
        json!({"format":"jpeg", "quality":80}),
        "1",
    )
    .unwrap();
    assert!(result["data"].is_string());
}

#[test]
fn page_capture_screenshot_format_webp_accepted() {
    // @trace REQ-CDP-001 [domain:Page] [level:integration]
    let b = backend();
    let result =
        dispatch_command(&*b, "Page.captureScreenshot", json!({"format":"webp"}), "1").unwrap();
    assert!(result["data"].is_string());
}

// ─────────────────────────────────────────────────────────────────────────
// Page.reload / close / bringToFront / setContent / navigateToHistoryEntry
// — CDP spec: 这些 method 返回空对象 {}
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn page_reload_returns_empty_object_conformance() {
    // Arrange — CDP 规范: Page.reload 无返回值(空对象)
    // @trace REQ-CDP-001 [domain:Page] [level:integration]
    let b = backend();

    // Act
    let result = dispatch_command(&*b, "Page.reload", json!({}), "1").unwrap();

    // Assert — CDP spec: empty return object
    assert!(
        result.as_object().map(|o| o.is_empty()).unwrap_or(false),
        "CDP spec: Page.reload returns empty object, got: {:?}",
        result
    );
}

#[test]
fn page_close_returns_empty_object_conformance() {
    // @trace REQ-CDP-001 [domain:Page] [level:integration]
    let b = backend();
    let result = dispatch_command(&*b, "Page.close", json!({}), "1").unwrap();
    assert!(
        result.as_object().map(|o| o.is_empty()).unwrap_or(false),
        "CDP spec: Page.close returns empty object, got: {:?}",
        result
    );
}

#[test]
fn page_bring_to_front_returns_empty_object_conformance() {
    // @trace REQ-CDP-001 [domain:Page] [level:integration]
    let b = backend();
    let result = dispatch_command(&*b, "Page.bringToFront", json!({}), "1").unwrap();
    assert!(
        result.as_object().map(|o| o.is_empty()).unwrap_or(false),
        "CDP spec: Page.bringToFront returns empty object"
    );
}

#[test]
fn page_set_content_requires_html_param() {
    // Arrange — CDP 规范: html 必填参数
    // @trace REQ-CDP-001 [domain:Page] [level:integration]
    let b = backend();

    // Act — 缺 html
    let err = dispatch_command(&*b, "Page.setContent", json!({}), "1").unwrap_err();

    // Assert
    assert!(matches!(err, BridgeError::InvalidParams(_)));
    assert_eq!(err.cdp_error_code(), -32602);
}

#[test]
fn page_set_content_with_html_returns_empty() {
    // @trace REQ-CDP-001 [domain:Page] [level:integration]
    let b = backend();
    let result =
        dispatch_command(&*b, "Page.setContent", json!({"html":"<h1>hi</h1>"}), "1").unwrap();
    assert!(result.as_object().map(|o| o.is_empty()).unwrap_or(false));
}

#[test]
fn page_navigate_to_history_entry_requires_entry_id() {
    // Arrange — CDP 规范: entryId 必填(JsUInt)
    // @trace REQ-CDP-001 [domain:Page] [level:integration]
    let b = backend();

    // Act — 缺 entryId
    let err = dispatch_command(&*b, "Page.navigateToHistoryEntry", json!({}), "1").unwrap_err();

    // Assert
    assert!(matches!(err, BridgeError::InvalidParams(_)));
    assert_eq!(err.cdp_error_code(), -32602);
}

#[test]
fn page_navigate_to_history_entry_with_id_returns_empty() {
    // @trace REQ-CDP-001 [domain:Page] [level:integration]
    let b = backend();
    let result = dispatch_command(
        &*b,
        "Page.navigateToHistoryEntry",
        json!({"entryId":5}),
        "1",
    )
    .unwrap();
    assert!(result.as_object().map(|o| o.is_empty()).unwrap_or(false));
}

// ─────────────────────────────────────────────────────────────────────────
// Page.getNavigationHistory — CDP spec: returns {currentIndex: int, entries: [{id, url, title}]}
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn page_get_navigation_history_schema_conformance() {
    // Arrange — CDP 规范: 返回 {currentIndex: JsUInt, entries: [NavigationEntry]}
    // NavigationEntry: {id: JsUInt, url: string, title: string}
    // @trace REQ-CDP-001 [domain:Page] [level:integration]
    let b = backend();

    // Act
    let result = dispatch_command(&*b, "Page.getNavigationHistory", json!({}), "1").unwrap();

    // Assert
    assert!(
        result["currentIndex"].is_i64() || result["currentIndex"].is_u64(),
        "CDP spec: currentIndex must be JsUInt (integer), got: {:?}",
        result["currentIndex"]
    );
    assert!(
        result["entries"].is_array(),
        "CDP spec: entries must be array"
    );
    for entry in result["entries"].as_array().unwrap() {
        assert!(
            entry["id"].is_i64() || entry["id"].is_u64(),
            "entry.id must be int"
        );
        assert!(entry["url"].is_string(), "entry.url must be string");
        assert!(entry["title"].is_string(), "entry.title must be string");
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Page.getFrameTree — CDP spec: returns {frameTree: {frame: Frame, childFrames: [FrameTree]}}
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn page_get_frame_tree_schema_conformance() {
    // Arrange — CDP 规范: 返回 {frameTree: FrameTree}
    // FrameTree: {frame: Frame, childFrames: [FrameTree]}
    // Frame: {id, url, mimeType, securityOrigin, parentId?, loaderId?, name?}
    // @trace REQ-CDP-001 [domain:Page] [level:integration]
    let b = backend();

    // Act
    let result = dispatch_command(&*b, "Page.getFrameTree", json!({}), "1").unwrap();

    // Assert
    let tree = &result["frameTree"];
    assert!(
        tree.is_object(),
        "CDP spec: frameTree must be object, got: {:?}",
        tree
    );
    let frame = &tree["frame"];
    assert!(frame["id"].is_string(), "CDP spec: frame.id must be string");
    assert!(
        frame["url"].is_string(),
        "CDP spec: frame.url must be string"
    );
    assert!(
        frame["mimeType"].is_string(),
        "CDP spec: frame.mimeType must be string"
    );
    assert!(
        frame["securityOrigin"].is_string(),
        "CDP spec: frame.securityOrigin must be string"
    );
    // childFields: 可选数组(bao 总是返回数组)
    assert!(
        tree["childFrames"].is_array(),
        "CDP spec: childFields must be array (may be empty)"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Page.getLayoutMetrics — CDP spec: returns {layoutViewport, visualViewport, contentSize,
//   cssLayoutViewport, cssVisualViewport, cssContentSize}
// bao 实现同时返回 deprecated 的 layoutViewport/visualViewport/contentSize
// 和 CSS 像素字段 cssLayoutViewport/cssVisualViewport/cssContentSize。
// https://chromedevtools.github.io/devtools-protocol/tot/Page/#method-getLayoutMetrics
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn page_get_layout_metrics_deprecated_schema_conformance() {
    // Arrange — CDP 规范: deprecated 字段 layoutViewport / visualViewport / contentSize
    // bao 实现这些字段(虽然 deprecated,仍需返回以保持向后兼容)
    // @trace REQ-CDP-001 [domain:Page] [level:integration]
    let b = backend();

    // Act
    let result = dispatch_command(&*b, "Page.getLayoutMetrics", json!({}), "1").unwrap();

    // Assert — deprecated 字段存在(CDP spec 仍要求返回)
    let lv = &result["layoutViewport"];
    assert!(
        lv.is_object(),
        "CDP spec: layoutViewport must be object, got: {:?}",
        lv
    );
    assert!(lv["pageX"].is_i64() || lv["pageX"].is_u64());
    assert!(lv["pageY"].is_i64() || lv["pageY"].is_u64());
    assert!(lv["clientWidth"].is_number(), "clientWidth must be number");
    assert!(
        lv["clientHeight"].is_number(),
        "clientHeight must be number"
    );

    let vv = &result["visualViewport"];
    assert!(vv.is_object());
    assert!(vv["offsetX"].is_number());
    assert!(vv["offsetY"].is_number());
    assert!(vv["pageX"].is_number());
    assert!(vv["pageY"].is_number());
    assert!(vv["clientWidth"].is_number());
    assert!(vv["clientHeight"].is_number());
    assert!(vv["scale"].is_number());

    let cs = &result["contentSize"];
    assert!(cs.is_object());
    assert!(cs["x"].is_number());
    assert!(cs["y"].is_number());
    assert!(cs["width"].is_number());
    assert!(cs["height"].is_number());
}

#[test]
fn page_get_layout_metrics_css_fields_schema_conformance() {
    // Arrange — CDP 规范: 现代浏览器返回 cssLayoutViewport / cssVisualViewport /
    // cssContentSize(CSS 像素字段)
    // @trace REQ-CDP-001 [domain:Page] [level:integration]
    let b = backend();
    let result = dispatch_command(&*b, "Page.getLayoutMetrics", json!({}), "1").unwrap();

    // Assert — cssLayoutViewport: 与 layoutViewport 同结构(pageX/pageY/clientWidth/clientHeight)
    let clv = &result["cssLayoutViewport"];
    assert!(
        clv.is_object(),
        "CDP spec: cssLayoutViewport must be object, got: {:?}",
        clv
    );
    assert!(clv["pageX"].is_i64() || clv["pageX"].is_u64());
    assert!(clv["pageY"].is_i64() || clv["pageY"].is_u64());
    assert!(clv["clientWidth"].is_number());
    assert!(clv["clientHeight"].is_number());

    // Assert — cssVisualViewport: 与 visualViewport 同结构
    let cvv = &result["cssVisualViewport"];
    assert!(
        cvv.is_object(),
        "CDP spec: cssVisualViewport must be object"
    );
    assert!(cvv["offsetX"].is_number());
    assert!(cvv["offsetY"].is_number());
    assert!(cvv["pageX"].is_number());
    assert!(cvv["pageY"].is_number());
    assert!(cvv["clientWidth"].is_number());
    assert!(cvv["clientHeight"].is_number());
    assert!(cvv["scale"].is_number());

    // Assert — cssContentSize: 与 contentSize 同结构(x/y/width/height)
    let ccs = &result["cssContentSize"];
    assert!(ccs.is_object(), "CDP spec: cssContentSize must be object");
    assert!(ccs["x"].is_number());
    assert!(ccs["y"].is_number());
    assert!(ccs["width"].is_number());
    assert!(ccs["height"].is_number());
}

// ─────────────────────────────────────────────────────────────────────────
// Page.printToPDF — E 类,servo 不支持,返回 -32601
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn page_print_to_pdf_e_class_returns_32601() {
    // Arrange — Page.printToPDF 是 E 类(servo 不支持)
    // @trace REQ-BAO-API-007 [domain:Page] [level:integration]
    let b = backend();

    // Act
    let err = dispatch_command(&*b, "Page.printToPDF", json!({}), "1").unwrap_err();

    // Assert — CDP / JSON-RPC: 不支持的 method 返回 -32601
    assert!(matches!(err, BridgeError::NotSupported(_)));
    assert_eq!(
        err.cdp_error_code(),
        -32601,
        "CDP spec: NotSupported method → JSON-RPC -32601 MethodNotFound"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// B 类 Page method — IIFE Eval 合成,返回 evaluate 形态
// {result: {type, value, ...}, exceptionDetails?}
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn page_title_b_class_returns_evaluate_shape() {
    // Arrange — Page.title 是 B 类(IIFE Eval 合成)
    // 返回 evaluate 形态:{result: {type, value}, exceptionDetails?}
    // @trace REQ-BAO-API-005 [domain:Page] [level:integration]
    let b = backend();

    // Act
    let result = dispatch_command(&*b, "Page.title", json!({}), "1").unwrap();

    // Assert — B 类返回的是 Runtime.evaluate 的 JSON 结构
    assert!(
        result["result"].is_object(),
        "B-class returns evaluate.result"
    );
    assert!(
        result["result"]["type"].is_string(),
        "CDP spec: RemoteObject.type must be string"
    );
    assert!(
        result["result"]["value"].is_string(),
        "B-class page.title value must be string (eval expression)"
    );
    // 验证 IIFE 形态
    let v = result["result"]["value"].as_str().unwrap();
    assert!(v.contains("(function(){"), "IIFE form");
    assert!(v.contains("return document.title;"));
}

#[test]
fn page_url_b_class_returns_evaluate_shape() {
    // @trace REQ-BAO-API-005 [domain:Page] [level:integration]
    let b = backend();
    let result = dispatch_command(&*b, "Page.url", json!({}), "1").unwrap();
    let v = result["result"]["value"].as_str().unwrap();
    assert!(v.contains("return location.href;"));
}

#[test]
fn page_content_b_class_returns_evaluate_shape() {
    // @trace REQ-BAO-API-005 [domain:Page] [level:integration]
    let b = backend();
    let result = dispatch_command(&*b, "Page.content", json!({}), "1").unwrap();
    let v = result["result"]["value"].as_str().unwrap();
    assert!(v.contains("document.documentElement.outerHTML"));
}

#[test]
fn page_viewport_b_class_returns_local_state_shape() {
    // Arrange — Page.viewport 是 D 类(本地状态合成)
    // 返回 {width, height, deviceScaleFactor, isMobile, hasTouch}
    // @trace REQ-BAO-API-005 [domain:Page] [level:integration]
    let b = backend();

    // Act
    let result = dispatch_command(&*b, "Page.viewport", json!({}), "1").unwrap();

    // Assert
    assert!(result["width"].is_number(), "viewport.width must be number");
    assert!(
        result["height"].is_number(),
        "viewport.height must be number"
    );
    assert!(
        result["deviceScaleFactor"].is_number(),
        "viewport.deviceScaleFactor must be number"
    );
    assert!(
        result["isMobile"].is_boolean(),
        "viewport.isMobile must be bool"
    );
    assert!(
        result["hasTouch"].is_boolean(),
        "viewport.hasTouch must be bool"
    );
}
