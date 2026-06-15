//! 命令分发器 — `match (domain, method)` 路由 193 method。
//!
//! # 三类方法
//!
//! - **A 类 48**:机械映射,直调 servo API
//! - **E 类 31+**:servo 不支持,返回 -32601
//! - **B 类 52**:IIFE Eval 合成 + 多步合成(TASK-3b 实现)
//! - **D 类 62**(TASK-5 实现):纯状态管理,本地缓存,无 servo 调用
//!
//! # 分发逻辑
//!
//! 1. `method.split_once('.')` → `(domain, command)`,缺 `.` 返回 `InvalidMethod`
//! 2. 优先检查 E 类(domain/method 在 E_CLASS_* 集合)
//! 3. A 类 match 分发(48 method)
//! 4. B 类 match 分发(52 method)
//! 5. 其他 → `MethodNotFound`
//!
//! @trace REQ-BAO-API-004 [level:library]
//! @trace REQ-BAO-API-005 [level:library]
//! @trace REQ-BAO-API-007 [level:library]

use serde_json::Value;

use super::a_class_handlers;
use super::b_class_handlers;
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
        // B 类 52 method:IIFE Eval 合成 + 多步合成
        // ──────────────────────────────────────────────────────────
        // B 类是高层 API 的合成 method,分为三类:
        //   1. 纯 Eval(build_iife):page.title → document.title
        //   2. 带参数 Eval(build_iife_with_args):el.setAttribute(n, v)
        //      — 强制 JSON.stringify 参数化,禁止字符串拼接
        //   3. 多步合成:click → DOM.getBoxModel + Input.dispatchMouseEvent
        //
        // @trace REQ-BAO-API-005 [level:library]
        //
        // ============ Page domain — B 类 (33 method) ============
        ("Page", "title") => b_class_handlers::page_title(backend, target_id, &params),
        ("Page", "url") => b_class_handlers::page_url(backend, target_id, &params),
        ("Page", "content") => b_class_handlers::page_content(backend, target_id, &params),
        ("Page", "viewport") => b_class_handlers::page_viewport(backend, target_id, &params),
        ("Page", "setViewport") => b_class_handlers::page_set_viewport(backend, target_id, &params),
        ("Page", "opener") => b_class_handlers::page_opener(backend, target_id, &params),
        ("Page", "frames") => b_class_handlers::page_frames(backend, target_id, &params),
        ("Page", "mainFrame") => b_class_handlers::page_main_frame(backend, target_id, &params),
        ("Page", "setDefaultNavigationTimeout") => {
            b_class_handlers::page_set_default_navigation_timeout(backend, target_id, &params)
        }
        ("Page", "setDefaultTimeout") => {
            b_class_handlers::page_set_default_timeout(backend, target_id, &params)
        }
        ("Page", "waitForLoadState") => {
            b_class_handlers::page_wait_for_load_state(backend, target_id, &params)
        }
        ("Page", "waitForURL") => b_class_handlers::page_wait_for_url(backend, target_id, &params),
        ("Page", "waitForRequest") => {
            b_class_handlers::page_wait_for_request(backend, target_id, &params)
        }
        ("Page", "waitForResponse") => {
            b_class_handlers::page_wait_for_response(backend, target_id, &params)
        }
        ("Page", "waitForEvent") => {
            b_class_handlers::page_wait_for_event(backend, target_id, &params)
        }
        ("Page", "goBack") => b_class_handlers::page_go_back(backend, target_id, &params),
        ("Page", "goForward") => b_class_handlers::page_go_forward(backend, target_id, &params),
        ("Page", "emulateMedia") => {
            b_class_handlers::page_emulate_media(backend, target_id, &params)
        }
        ("Page", "addScriptTag") => {
            b_class_handlers::page_add_script_tag(backend, target_id, &params)
        }
        ("Page", "addStyleTag") => {
            b_class_handlers::page_add_style_tag(backend, target_id, &params)
        }
        ("Page", "exposeFunction") => {
            b_class_handlers::page_expose_function(backend, target_id, &params)
        }
        ("Page", "pdf") => b_class_handlers::page_pdf(backend, target_id, &params),
        ("Page", "screenshot") => b_class_handlers::page_screenshot(backend, target_id, &params),
        ("Page", "tap") => b_class_handlers::page_tap(backend, target_id, &params),
        ("Page", "hover") => b_class_handlers::page_hover(backend, target_id, &params),
        ("Page", "focus") => b_class_handlers::page_focus(backend, target_id, &params),
        ("Page", "type") => b_class_handlers::page_type(backend, target_id, &params),
        ("Page", "fill") => b_class_handlers::page_fill(backend, target_id, &params),
        ("Page", "press") => b_class_handlers::page_press(backend, target_id, &params),
        ("Page", "check") => b_class_handlers::page_check(backend, target_id, &params),
        ("Page", "uncheck") => b_class_handlers::page_uncheck(backend, target_id, &params),
        ("Page", "selectOption") => {
            b_class_handlers::page_select_option(backend, target_id, &params)
        }
        ("Page", "setInputFiles") => {
            b_class_handlers::page_set_input_files(backend, target_id, &params)
        }
        ("Page", "requestGC") => b_class_handlers::page_request_gc(backend, target_id, &params),

        // ============ ElementHandle domain — B 类 (14 method) ============
        // @trace REQ-BAO-API-005 [domain:ElementHandle]
        ("ElementHandle", "click") => b_class_handlers::page_tap(backend, target_id, &params),
        ("ElementHandle", "contentFrame") => {
            b_class_handlers::element_content_frame(backend, target_id, &params)
        }
        ("ElementHandle", "ownerFrame") => {
            b_class_handlers::element_owner_frame(backend, target_id, &params)
        }
        ("ElementHandle", "getAttribute") => {
            b_class_handlers::element_get_attribute(backend, target_id, &params)
        }
        ("ElementHandle", "innerHTML") => {
            b_class_handlers::element_inner_html(backend, target_id, &params)
        }
        ("ElementHandle", "innerText") => {
            b_class_handlers::element_inner_text(backend, target_id, &params)
        }
        ("ElementHandle", "textContent") => {
            b_class_handlers::element_text_content(backend, target_id, &params)
        }
        ("ElementHandle", "isChecked") => {
            b_class_handlers::element_is_checked(backend, target_id, &params)
        }
        ("ElementHandle", "isDisabled") => {
            b_class_handlers::element_is_disabled(backend, target_id, &params)
        }
        ("ElementHandle", "isEditable") => {
            b_class_handlers::element_is_editable(backend, target_id, &params)
        }
        ("ElementHandle", "isEnabled") => {
            b_class_handlers::element_is_enabled(backend, target_id, &params)
        }
        ("ElementHandle", "isHidden") => {
            b_class_handlers::element_is_hidden(backend, target_id, &params)
        }
        ("ElementHandle", "isVisible") => {
            b_class_handlers::element_is_visible(backend, target_id, &params)
        }
        ("ElementHandle", "scrollIntoViewIfNeeded") => {
            b_class_handlers::element_scroll_into_view(backend, target_id, &params)
        }
        ("ElementHandle", "waitForElementState") => {
            b_class_handlers::element_wait_for_element_state(backend, target_id, &params)
        }
        ("ElementHandle", "waitForSelector") => {
            b_class_handlers::element_wait_for_selector(backend, target_id, &params)
        }

        // ============ JSHandle domain — B 类 (7 method) ============
        // @trace REQ-BAO-API-005 [domain:JSHandle]
        ("JSHandle", "asElement") => {
            b_class_handlers::js_handle_as_element(backend, target_id, &params)
        }
        ("JSHandle", "dispose") => {
            b_class_handlers::js_handle_dispose(backend, target_id, &params)
        }
        ("JSHandle", "evaluate") => {
            b_class_handlers::js_handle_evaluate(backend, target_id, &params)
        }
        ("JSHandle", "evaluateHandle") => {
            b_class_handlers::js_handle_evaluate_handle(backend, target_id, &params)
        }
        ("JSHandle", "getProperties") => {
            b_class_handlers::js_handle_get_properties(backend, target_id, &params)
        }
        ("JSHandle", "getProperty") => {
            b_class_handlers::js_handle_get_property(backend, target_id, &params)
        }
        ("JSHandle", "jsonValue") => {
            b_class_handlers::js_handle_json_value(backend, target_id, &params)
        }

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

    // ── B 类路由测试 ──
    // @trace REQ-BAO-API-005 [level:library]

    #[test]
    fn dispatch_page_title_b_class_routes_to_handler() {
        let b = MockServoBackend::new();
        let r = dispatch_command(&b, "Page.title", empty_params(), "1").unwrap();
        // Mock backend echo evaluate expression
        let v = r["result"]["value"].as_str().unwrap();
        assert!(v.contains("(function(){"));
        assert!(v.contains("return document.title;"));
    }

    #[test]
    fn dispatch_page_url_b_class_routes_to_handler() {
        let b = MockServoBackend::new();
        let r = dispatch_command(&b, "Page.url", empty_params(), "1").unwrap();
        let v = r["result"]["value"].as_str().unwrap();
        assert!(v.contains("return location.href;"));
    }

    #[test]
    fn dispatch_page_content_b_class_routes_to_handler() {
        let b = MockServoBackend::new();
        let r = dispatch_command(&b, "Page.content", empty_params(), "1").unwrap();
        let v = r["result"]["value"].as_str().unwrap();
        assert!(v.contains("document.documentElement.outerHTML"));
    }

    #[test]
    fn dispatch_page_viewport_b_class_routes_to_handler() {
        let b = MockServoBackend::new();
        let r = dispatch_command(&b, "Page.viewport", empty_params(), "1").unwrap();
        assert!(r["width"].is_number());
    }

    #[test]
    fn dispatch_element_handle_click_b_class_routes_to_tap() {
        let b = MockServoBackend::new();
        // ElementHandle.click → page_tap(selector)
        let r = dispatch_command(
            &b,
            "ElementHandle.click",
            json!({"selector":"button"}),
            "1",
        )
        .unwrap();
        let v = r["result"]["value"].as_str().unwrap();
        assert!(v.contains("var s=__args[0]"));
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
