//! 命令分发器 — `match (domain, method)` 路由 193 method。
//!
//! # 三类方法
//!
//! - **A 类 48**:机械映射,直调 servo API
//! - **E 类 31+**:servo 不支持,返回 -32601
//! - **B 类 52**(TASK-3b 实现):占位 `NotImplementedYet`,后续通过 eval_synthesizer 实现
//! - **D 类 62**(TASK-5 实现):纯状态管理,本地缓存,无 servo 调用
//!
//! 当前 TASK-3a 范围:A 类 + E 类完整,B 类占位。
//!
//! # 分发逻辑
//!
//! 1. `method.split_once('.')` → `(domain, command)`,缺 `.` 返回 `InvalidMethod`
//! 2. 优先检查 E 类(domain/method 在 E_CLASS_* 集合)
//! 3. A 类 match 分发(48 method)
//! 4. 其他 → `MethodNotFound`
//!
//! @trace REQ-BAO-API-004 [level:library]
//! @trace REQ-BAO-API-007 [level:library]

use serde_json::Value;

use super::a_class_handlers;
use super::e_class;
use super::error::BridgeError;
use super::servo_backend::ServoBackend;

/// 派发 CDP 命令。
///
/// # 参数
/// - `backend`: servo 后端抽象
/// - `method`: CDP method 名(如 `Page.navigate`)
/// - `params`: JSON 参数
/// - `target_id`: 目标 Page 标识
///
/// # 返回
/// - `Ok(Value)`: CDP-compatible JSON 响应
/// - `Err(BridgeError)`: 各种错误(E 类 NotSupported / A 类执行错误 / MethodNotFound)
///
/// @trace REQ-BAO-API-004 [level:library]
/// @trace REQ-BAO-API-007 [level:library]
pub fn dispatch_command(
    backend: &dyn ServoBackend,
    method: &str,
    params: Value,
    target_id: &str,
) -> Result<Value, BridgeError> {
    let (domain, command) = method
        .split_once('.')
        .ok_or_else(|| BridgeError::InvalidMethod(method.to_string()))?;

    // ──────────────────────────────────────────────────────────────
    // E 类 31+ method:servo 不支持
    // ──────────────────────────────────────────────────────────────
    if e_class::is_e_class(domain, command) {
        // @trace REQ-BAO-API-007 [level:library]
        return Err(e_class::not_supported(domain, command));
    }

    // ──────────────────────────────────────────────────────────────
    // A 类 48 method:机械映射
    // ──────────────────────────────────────────────────────────────
    match (domain, command) {
        // ============ Page domain (11 method) ============
        // @trace REQ-BAO-API-004 [domain:Page]
        ("Page", "navigate") => a_class_handlers::page_navigate(backend, target_id, &params),
        ("Page", "reload") => a_class_handlers::page_reload(backend, target_id, &params),
        ("Page", "captureScreenshot") => {
            a_class_handlers::page_capture_screenshot(backend, target_id, &params)
        }
        ("Page", "getFrameTree") => {
            a_class_handlers::page_get_frame_tree(backend, target_id, &params)
        }
        ("Page", "getNavigationHistory") => {
            a_class_handlers::page_get_navigation_history(backend, target_id, &params)
        }
        ("Page", "navigateToHistoryEntry") => {
            a_class_handlers::page_navigate_to_history_entry(backend, target_id, &params)
        }
        ("Page", "setContent") => a_class_handlers::page_set_content(backend, target_id, &params),
        ("Page", "close") => a_class_handlers::page_close(backend, target_id, &params),
        ("Page", "bringToFront") => {
            a_class_handlers::page_bring_to_front(backend, target_id, &params)
        }
        ("Page", "getLayoutMetrics") => {
            a_class_handlers::page_get_layout_metrics(backend, target_id, &params)
        }
        // Page.printToPDF is E-class — handled above.

        // ============ Runtime domain (6 method) ============
        // @trace REQ-BAO-API-004 [domain:Runtime]
        ("Runtime", "evaluate") => a_class_handlers::runtime_evaluate(backend, target_id, &params),
        ("Runtime", "callFunctionOn") => {
            a_class_handlers::runtime_call_function_on(backend, target_id, &params)
        }
        ("Runtime", "getProperties") => {
            a_class_handlers::runtime_get_properties(backend, target_id, &params)
        }
        ("Runtime", "releaseObject") => {
            a_class_handlers::runtime_release_object(backend, target_id, &params)
        }
        ("Runtime", "enable") => a_class_handlers::runtime_enable(backend, target_id, &params),
        ("Runtime", "disable") => a_class_handlers::runtime_disable(backend, target_id, &params),

        // ============ DOM domain (11 method) ============
        // @trace REQ-BAO-API-004 [domain:DOM]
        ("DOM", "getDocument") => a_class_handlers::dom_get_document(backend, target_id, &params),
        ("DOM", "querySelector") => {
            a_class_handlers::dom_query_selector(backend, target_id, &params)
        }
        ("DOM", "querySelectorAll") => {
            a_class_handlers::dom_query_selector_all(backend, target_id, &params)
        }
        ("DOM", "getBoxModel") => a_class_handlers::dom_get_box_model(backend, target_id, &params),
        ("DOM", "resolveNode") => a_class_handlers::dom_resolve_node(backend, target_id, &params),
        ("DOM", "describeNode") => a_class_handlers::dom_describe_node(backend, target_id, &params),
        ("DOM", "setAttributeValue") => {
            a_class_handlers::dom_set_attribute_value(backend, target_id, &params)
        }
        ("DOM", "removeAttribute") => {
            a_class_handlers::dom_remove_attribute(backend, target_id, &params)
        }
        ("DOM", "getOuterHTML") => a_class_handlers::dom_get_outer_html(backend, target_id, &params),
        ("DOM", "setOuterHTML") => a_class_handlers::dom_set_outer_html(backend, target_id, &params),
        ("DOM", "requestNode") => a_class_handlers::dom_request_node(backend, target_id, &params),

        // ============ Network domain (4 method) ============
        // @trace REQ-BAO-API-004 [domain:Network]
        ("Network", "enable") => a_class_handlers::network_enable(backend, target_id, &params),
        ("Network", "disable") => a_class_handlers::network_disable(backend, target_id, &params),
        ("Network", "getResponseBody") => {
            a_class_handlers::network_get_response_body(backend, target_id, &params)
        }
        ("Network", "setCacheDisabled") => {
            a_class_handlers::network_set_cache_disabled(backend, target_id, &params)
        }

        // ============ Input domain (4 method) ============
        // @trace REQ-BAO-API-004 [domain:Input]
        ("Input", "dispatchMouseEvent") => {
            a_class_handlers::input_dispatch_mouse_event(backend, target_id, &params)
        }
        ("Input", "dispatchKeyEvent") => {
            a_class_handlers::input_dispatch_key_event(backend, target_id, &params)
        }
        ("Input", "dispatchTouchEvent") => {
            a_class_handlers::input_dispatch_touch_event(backend, target_id, &params)
        }
        ("Input", "setIgnoreInputEvents") => {
            a_class_handlers::input_set_ignore_input_events(backend, target_id, &params)
        }

        // ============ Emulation domain (4 method) ============
        // @trace REQ-BAO-API-004 [domain:Emulation]
        ("Emulation", "setDeviceMetricsOverride") => {
            a_class_handlers::emulation_set_device_metrics_override(backend, target_id, &params)
        }
        ("Emulation", "clearDeviceMetricsOverride") => {
            a_class_handlers::emulation_clear_device_metrics_override(backend, target_id, &params)
        }
        ("Emulation", "setUserAgentOverride") => {
            a_class_handlers::emulation_set_user_agent_override(backend, target_id, &params)
        }
        ("Emulation", "setGeolocationOverride") => {
            a_class_handlers::emulation_set_geolocation_override(backend, target_id, &params)
        }

        // ============ Target domain (6 method) ============
        // @trace REQ-BAO-API-004 [domain:Target]
        ("Target", "getTargets") => {
            a_class_handlers::target_get_targets(backend, target_id, &params)
        }
        ("Target", "createTarget") => {
            a_class_handlers::target_create_target(backend, target_id, &params)
        }
        ("Target", "closeTarget") => {
            a_class_handlers::target_close_target(backend, target_id, &params)
        }
        ("Target", "attachToTarget") => {
            a_class_handlers::target_attach_to_target(backend, target_id, &params)
        }
        ("Target", "detachFromTarget") => {
            a_class_handlers::target_detach_from_target(backend, target_id, &params)
        }
        ("Target", "setAutoAttach") => {
            a_class_handlers::target_set_auto_attach(backend, target_id, &params)
        }

        // ============ CSS domain (2 method) ============
        // @trace REQ-BAO-API-004 [domain:CSS]
        ("CSS", "getComputedStyleForNode") => {
            a_class_handlers::css_get_computed_style_for_node(backend, target_id, &params)
        }
        ("CSS", "getMatchedStylesForNode") => {
            a_class_handlers::css_get_matched_styles_for_node(backend, target_id, &params)
        }

        // ──────────────────────────────────────────────────────────
        // B 类 52 method(TASK-3b 实现)— 显式占位
        // ──────────────────────────────────────────────────────────
        // B 类是高层 API 的 Eval 合成,如:
        //   Page.title → Runtime.evaluate("document.title")
        //   Page.url → Runtime.evaluate("location.href")
        //   Page.content → Runtime.evaluate("document.documentElement.outerHTML")
        //   Page.viewport → 本地状态(TASK-5)
        //   Element.click → Runtime.evaluate("...click()")
        // 等等。TASK-3b 用 eval_synthesizer.rs 实现。
        ("Page", "title")
        | ("Page", "url")
        | ("Page", "content")
        | ("Page", "viewport")
        | ("Page", "setViewport")
        | ("Page", "opener")
        | ("Page", "frames")
        | ("Page", "mainFrame")
        | ("Page", "setDefaultNavigationTimeout")
        | ("Page", "setDefaultTimeout")
        | ("Page", "waitForLoadState")
        | ("Page", "waitForURL")
        | ("Page", "waitForRequest")
        | ("Page", "waitForResponse")
        | ("Page", "waitForEvent")
        | ("Page", "goBack")
        | ("Page", "goForward")
        | ("Page", "emulateMedia")
        | ("Page", "addScriptTag")
        | ("Page", "addStyleTag")
        | ("Page", "exposeFunction")
        | ("Page", "pdf") // B 类 pdf 已在 E 类排除? 不,E 类已拦截;这里走 B 类占位兜底
        | ("Page", "screenshot")
        | ("Page", "tap")
        | ("Page", "hover")
        | ("Page", "focus")
        | ("Page", "type")
        | ("Page", "fill")
        | ("Page", "press")
        | ("Page", "check")
        | ("Page", "uncheck")
        | ("Page", "selectOption")
        | ("Page", "setInputFiles")
        | ("Page", "requestGC")
        | ("ElementHandle", "click")
        | ("ElementHandle", "contentFrame")
        | ("ElementHandle", "ownerFrame")
        | ("ElementHandle", "getAttribute")
        | ("ElementHandle", "innerHTML")
        | ("ElementHandle", "innerText")
        | ("ElementHandle", "textContent")
        | ("ElementHandle", "isChecked")
        | ("ElementHandle", "isDisabled")
        | ("ElementHandle", "isEditable")
        | ("ElementHandle", "isEnabled")
        | ("ElementHandle", "isHidden")
        | ("ElementHandle", "isVisible")
        | ("ElementHandle", "scrollIntoViewIfNeeded")
        | ("ElementHandle", "waitForElementState")
        | ("ElementHandle", "waitForSelector")
        | ("JSHandle", "asElement")
        | ("JSHandle", "dispose")
        | ("JSHandle", "evaluate")
        | ("JSHandle", "evaluateHandle")
        | ("JSHandle", "getProperties")
        | ("JSHandle", "getProperty")
        | ("JSHandle", "jsonValue") => Err(BridgeError::NotImplementedYet(format!(
            "B 类 method `{method}` TASK-3b 实现"
        ))),

        // ──────────────────────────────────────────────────────────
        // 默认:未知 method
        // ──────────────────────────────────────────────────────────
        _ => Err(BridgeError::MethodNotFound(method.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::servo_backend::MockServoBackend;
    use serde_json::json;

    fn empty_params() -> Value {
        Value::Object(Default::default())
    }

    // ── A 类测试 ──

    #[test]
    fn dispatch_page_navigate_a_class() {
        let b = MockServoBackend::new();
        let r = dispatch_command(&b, "Page.navigate", json!({"url":"https://x"}), "1").unwrap();
        assert_eq!(r["frameId"], "FRAME_0");
    }

    #[test]
    fn dispatch_runtime_evaluate_a_class() {
        let b = MockServoBackend::new();
        let r = dispatch_command(&b, "Runtime.evaluate", json!({"expression":"1"}), "1").unwrap();
        assert_eq!(r["result"]["type"], "string");
    }

    #[test]
    fn dispatch_dom_query_selector_a_class() {
        let b = MockServoBackend::new();
        let r = dispatch_command(
            &b,
            "DOM.querySelector",
            json!({"nodeId":1,"selector":"div"}),
            "1",
        )
        .unwrap();
        assert_eq!(r["nodeId"], 2);
    }

    #[test]
    fn dispatch_target_create_target_a_class() {
        let b = MockServoBackend::new();
        let r = dispatch_command(&b, "Target.createTarget", json!({"url":"x"}), "1").unwrap();
        assert!(r["targetId"].is_string());
    }

    // ── E 类测试 ──

    #[test]
    fn dispatch_heap_profiler_e_class() {
        let b = MockServoBackend::new();
        let err = dispatch_command(&b, "HeapProfiler.takeHeapSnapshot", empty_params(), "1").unwrap_err();
        assert!(matches!(err, BridgeError::NotSupported(_)));
        assert_eq!(err.cdp_error_code(), -32601);
    }

    #[test]
    fn dispatch_profiler_enable_e_class() {
        let b = MockServoBackend::new();
        let err = dispatch_command(&b, "Profiler.enable", empty_params(), "1").unwrap_err();
        assert!(matches!(err, BridgeError::NotSupported(_)));
    }

    #[test]
    fn dispatch_dom_storage_e_class() {
        let b = MockServoBackend::new();
        let err = dispatch_command(&b, "DOMStorage.getDOMStorageItems", empty_params(), "1").unwrap_err();
        assert!(matches!(err, BridgeError::NotSupported(_)));
    }

    #[test]
    fn dispatch_indexed_db_e_class() {
        let b = MockServoBackend::new();
        let err = dispatch_command(&b, "IndexedDB.requestDatabaseNames", empty_params(), "1").unwrap_err();
        assert!(matches!(err, BridgeError::NotSupported(_)));
    }

    #[test]
    fn dispatch_service_worker_e_class() {
        let b = MockServoBackend::new();
        let err = dispatch_command(&b, "ServiceWorker.enable", empty_params(), "1").unwrap_err();
        assert!(matches!(err, BridgeError::NotSupported(_)));
    }

    #[test]
    fn dispatch_page_print_to_pdf_e_class() {
        let b = MockServoBackend::new();
        let err = dispatch_command(&b, "Page.printToPDF", empty_params(), "1").unwrap_err();
        assert!(matches!(err, BridgeError::NotSupported(_)));
    }

    #[test]
    fn dispatch_debugger_pause_e_class() {
        let b = MockServoBackend::new();
        let err = dispatch_command(&b, "Debugger.pause", empty_params(), "1").unwrap_err();
        assert!(matches!(err, BridgeError::NotSupported(_)));
    }

    #[test]
    fn dispatch_performance_metrics_e_class() {
        let b = MockServoBackend::new();
        let err = dispatch_command(&b, "Performance.getMetrics", empty_params(), "1").unwrap_err();
        assert!(matches!(err, BridgeError::NotSupported(_)));
    }

    // ── B 类占位测试 ──

    #[test]
    fn dispatch_page_title_b_class_placeholder() {
        let b = MockServoBackend::new();
        let err = dispatch_command(&b, "Page.title", empty_params(), "1").unwrap_err();
        assert!(matches!(err, BridgeError::NotImplementedYet(_)));
    }

    #[test]
    fn dispatch_element_handle_click_b_class_placeholder() {
        let b = MockServoBackend::new();
        let err = dispatch_command(&b, "ElementHandle.click", empty_params(), "1").unwrap_err();
        assert!(matches!(err, BridgeError::NotImplementedYet(_)));
    }

    // ── 错误格式测试 ──

    #[test]
    fn dispatch_invalid_method_no_dot_returns_invalid_method() {
        let b = MockServoBackend::new();
        let err = dispatch_command(&b, "noDotHere", empty_params(), "1").unwrap_err();
        assert!(matches!(err, BridgeError::InvalidMethod(_)));
    }

    #[test]
    fn dispatch_unknown_method_returns_method_not_found() {
        let b = MockServoBackend::new();
        let err = dispatch_command(&b, "Unknown.foo", empty_params(), "1").unwrap_err();
        assert!(matches!(err, BridgeError::MethodNotFound(_)));
    }

    #[test]
    fn dispatch_unknown_method_in_known_domain_returns_method_not_found() {
        let b = MockServoBackend::new();
        let err = dispatch_command(&b, "Page.totallyBogus", empty_params(), "1").unwrap_err();
        assert!(matches!(err, BridgeError::MethodNotFound(_)));
    }
}
