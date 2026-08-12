// @trace TEST-CDP-026 [req:REQ-CDP-001,REQ-CDP-004,REQ-CDP-005,REQ-CDP-006,REQ-CDP-007] [level:unit]
// Protocol domain handler sub-command full coverage — every command path in
// Page/Runtime/DOM/Network/CSS/Emulation/Input/Overlay/Debugger/Log/Fetch/Target.
// Tests without bridge (None) to verify default/stub responses.
//
// Adversarial verification gaps closed in this file:
//   (1) JSON-RPC 2.0 invariant: every response carries exactly one of
//       {result, error} (never both, never neither) — enforced per dispatch.
//   (2) Response id == input id (including None / negative / i64::MAX).
//   (3) Error envelope carries domain+command in message (debuggability).
//   (4) Stub schema contracts: every documented field is present with the
//       documented type/value (REQ-CDP-001..007 alignment).
//   (5) Boundary conditions: empty params, missing keys, wrong-type values,
//       multiple dots, unicode, oversized inputs.
//   (6) `splitn(2, '.')` semantics: only first dot splits domain/command.
//   (7) Bridge-absent default values: title="Bao", url="about:blank",
//       type="page", attached=true (live_target_info).

use bao_cdp::{
    handle_command, serialize_event, serialize_response, CdpEvent, CdpMessage, CdpResponse,
};

const TID: &str = "test-target";
use serde_json::json;

fn dispatch(method: &str, params: Option<serde_json::Value>) -> CdpResponse {
    let p = params;
    let msg = CdpMessage {
        id: Some(1),
        method: method.to_string(),
        params: None,
        session_id: None,
    };
    handle_command(msg, "test-target", &p, None)
}

/// dispatch with a custom id — verifies id propagation invariant.
fn dispatch_id(id: Option<i64>, method: &str, params: Option<serde_json::Value>) -> CdpResponse {
    let msg = CdpMessage {
        id,
        method: method.to_string(),
        params: None,
        session_id: None,
    };
    handle_command(msg, "test-target", &params, None)
}

/// dispatch with a custom target_id — verifies target_id echoes into responses.
fn dispatch_target(
    target_id: &str,
    method: &str,
    params: Option<serde_json::Value>,
) -> CdpResponse {
    let msg = CdpMessage {
        id: Some(1),
        method: method.to_string(),
        params: None,
        session_id: None,
    };
    handle_command(msg, target_id, &params, None)
}

fn ok_resp(method: &str, params: Option<serde_json::Value>) -> bool {
    let r = dispatch(method, params);
    r.result.is_some() && r.error.is_none()
}

fn err_code(method: &str) -> i64 {
    let r = dispatch(method, None);
    r.error.map(|e| e.code).unwrap_or(0)
}

/// JSON-RPC 2.0 invariant: result XOR error (never both, never neither).
/// Every CdpResponse from handle_command MUST satisfy this.
fn assert_jsonrpc_invariant(resp: &CdpResponse, ctx: &str) {
    assert!(
        resp.result.is_some() ^ resp.error.is_some(),
        "[{}] JSON-RPC 2.0 invariant violated: result={} error={} (expected exactly one)",
        ctx,
        resp.result.is_some(),
        resp.error.is_some()
    );
}

// =====================================================================
// Adversarial: JSON-RPC 2.0 invariant + id propagation (REQ-CDP-001-C2)
// =====================================================================

#[test]
fn test_invariant_all_ok_responses() {
    // Every known ok command must carry result AND NOT error.
    for method in [
        "Target.getTargets",
        "Target.getTargetTargets",
        "Target.createTarget",
        "Target.closeTarget",
        "Target.setAutoAttach",
        "Target.setDiscoverTargets",
        "Target.getTargetInfo",
        "Target.attachToTarget",
        "Target.detachFromTarget",
        "Target.sendMessageToTarget",
        "Page.enable",
        "Page.disable",
        "Page.navigate",
        "Page.reload",
        "Page.getFrameTree",
        "Page.getNavigationHistory",
        "Page.captureScreenshot",
        "Page.setContent",
        "Page.close",
        "Page.bringToFront",
        "Page.getLayoutMetrics",
        "Page.addScriptToEvaluateOnNewDocument",
        "Page.removeScriptToEvaluateOnNewDocument",
        "Runtime.enable",
        "Runtime.disable",
        "Runtime.evaluate",
        "Runtime.callFunctionOn",
        "Runtime.getProperties",
        "Runtime.runScript",
        "Runtime.releaseObject",
        "Runtime.releaseObjectGroup",
        "Runtime.compileScript",
        "DOM.enable",
        "DOM.disable",
        "DOM.getDocument",
        "DOM.describeNode",
        "DOM.querySelector",
        "DOM.querySelectorAll",
        "DOM.getBoxModel",
        "DOM.setAttributeValue",
        "DOM.removeAttribute",
        "DOM.setOuterHTML",
        "DOM.insertBefore",
        "DOM.removeNode",
        "DOM.getOuterHTML",
        "DOM.resolveNode",
        "DOM.pushNodesByBackendIdsToFrontend",
        "Network.enable",
        "Network.disable",
        "Network.getResponseBody",
        "Network.setCacheDisabled",
        "Network.setExtraHTTPHeaders",
        "Network.emulateNetworkConditions",
        "Network.setRequestInterception",
        "Network.continueInterceptedRequest",
        "Network.getCookies",
        "Network.getAllCookies",
        "Network.deleteCookies",
        "Network.setCookie",
        "CSS.enable",
        "CSS.disable",
        "CSS.getComputedStyleForNode",
        "CSS.getMatchedStylesForNode",
        "CSS.getInlineStylesForNode",
        "CSS.setStyleTexts",
        "Emulation.setDeviceMetricsOverride",
        "Emulation.clearDeviceMetricsOverride",
        "Emulation.setUserAgentOverride",
        "Emulation.setTouchEmulationEnabled",
        "Emulation.setScriptExecutionDisabled",
        "Emulation.setFocusEmulationEnabled",
        "Emulation.setCPUThrottlingRate",
        "Emulation.setDefaultBackgroundColorOverride",
        "Input.dispatchMouseEvent",
        "Input.dispatchKeyEvent",
        "Input.dispatchTouchEvent",
        "Input.insertText",
        "Input.setIgnoreInputEvents",
        "Input.setInterceptDrags",
        "Overlay.enable",
        "Overlay.disable",
        "Overlay.highlightNode",
        "Overlay.hideHighlight",
        "Overlay.setInspectMode",
        "Overlay.setPausedInDebuggerMessage",
        "Debugger.enable",
        "Debugger.disable",
        "Debugger.setBreakpointByUrl",
        "Debugger.removeBreakpoint",
        "Debugger.pause",
        "Debugger.resume",
        "Debugger.stepOver",
        "Debugger.stepInto",
        "Debugger.stepOut",
        "Debugger.setSkipAllPauses",
        "Debugger.setBreakpointsActive",
        "Debugger.evaluateOnCallFrame",
        "Debugger.getPossibleBreakpoints",
        "Debugger.getScriptSource",
        "Debugger.setPauseOnExceptions",
        "Log.enable",
        "Log.disable",
        "Log.clear",
        "Log.startViolationsReport",
        "Log.stopViolationsReport",
        "Fetch.enable",
        "Fetch.disable",
        "Fetch.continueRequest",
        "Fetch.continueWithResponse",
        "Fetch.failRequest",
        "Fetch.fulfillRequest",
        "Fetch.getRequestPostData",
        "Fetch.continueWithAuth",
        "Fetch.takeResponseBodyAsStream",
    ] {
        let r = dispatch(method, None);
        assert_jsonrpc_invariant(&r, method);
        // Success responses must carry a non-null result object (per CDP convention).
        assert!(
            r.result.is_some(),
            "[{}] expected success but got error: {:?}",
            method,
            r.error
        );
    }
}

#[test]
fn test_invariant_all_error_responses() {
    // Every unknown command (within a known domain) must carry error AND NOT result.
    for method in [
        "Target.nonexistent",
        "Page.nonexistent",
        "Runtime.nonexistent",
        "DOM.nonexistent",
        "Network.nonexistent",
        "CSS.nonexistent",
        "Emulation.nonexistent",
        "Input.nonexistent",
        "Overlay.nonexistent",
        "Debugger.nonexistent",
        "Log.nonexistent",
        "Fetch.nonexistent",
        // Adversarial: empty method / unknown domain / no-dot method
        "",
        "Page",
        "NoDomain",
        "Page.navigate.to",
    ] {
        let r = dispatch(method, None);
        assert_jsonrpc_invariant(&r, method);
        assert!(
            r.error.is_some(),
            "[{}] expected error but got result: {:?}",
            method,
            r.result
        );
        assert_eq!(
            r.error.as_ref().unwrap().code,
            -32601,
            "[{}] JSON-RPC method-not-found code must be -32601",
            method
        );
    }
}

#[test]
fn test_id_propagation_all_boundaries() {
    // id must echo back exactly for: None, 0, 1, -1, i64::MIN, i64::MAX.
    for id in [
        None,
        Some(0i64),
        Some(1),
        Some(-1),
        Some(-42),
        Some(i64::MIN),
        Some(i64::MAX),
    ] {
        let r = dispatch_id(id, "Page.enable", None);
        assert_eq!(r.id, id, "id must propagate unchanged for {:?}", id);
    }
}

#[test]
fn test_id_propagation_on_error_path() {
    // Even error responses must echo the original id (JSON-RPC 2.0 §4.2).
    for id in [None, Some(-7), Some(1234567890)] {
        let r = dispatch_id(id, "Bogus.bogus", None);
        assert_eq!(r.id, id, "error path must echo id for {:?}", id);
        assert!(r.error.is_some());
    }
}

#[test]
fn test_error_message_contains_domain_and_command() {
    // Adversarial: error message MUST embed the offending method for debuggability.
    // This is a regression guard: a generic "method not found" message would break clients.
    for (method, expected_substr) in [
        ("Target.foo", "Target.foo"),
        ("Page.bar", "Page.bar"),
        ("Runtime.baz", "Runtime.baz"),
        ("DOM.qux", "DOM.qux"),
        ("Network.noop", "Network.noop"),
        ("CSS.x", "CSS.x"),
        ("Emulation.y", "Emulation.y"),
        ("Input.z", "Input.z"),
        ("Overlay.w", "Overlay.w"),
        ("Debugger.d", "Debugger.d"),
        ("Log.l", "Log.l"),
        ("Fetch.f", "Fetch.f"),
        ("Page", "Page"),         // no-dot: whole method embedded
        ("NoDomain", "NoDomain"), // unknown domain: whole method embedded
    ] {
        let r = dispatch(method, None);
        let msg = r.error.expect("expected error").message;
        assert!(
            msg.contains(expected_substr),
            "[{}] error message {:?} must contain {:?}",
            method,
            msg,
            expected_substr
        );
    }
}

#[test]
fn test_empty_error_message_never() {
    // Adversarial: no error message may be empty (debuggability contract).
    for method in [
        "Target.x",
        "Page.x",
        "Runtime.x",
        "DOM.x",
        "Network.x",
        "CSS.x",
        "Emulation.x",
        "Input.x",
        "Overlay.x",
        "Debugger.x",
        "Log.x",
        "Fetch.x",
        "",
        "NoDomain",
    ] {
        let r = dispatch(method, None);
        if let Some(e) = r.error {
            assert!(
                !e.message.is_empty(),
                "[{}] error message must be non-empty",
                method
            );
        }
    }
}

// =====================================================================
// splitn(2, '.') semantics — only first dot splits domain/command
// =====================================================================

#[test]
fn test_method_with_multiple_dots_routes_to_domain() {
    // "Page.navigate.to" → domain="Page", command="navigate.to" → unknown command
    // under Page domain (Page.navigate.to is NOT a known Page command).
    let r = dispatch("Page.navigate.to", None);
    assert!(r.error.is_some());
    let e = r.error.unwrap();
    assert_eq!(e.code, -32601);
    // Message embeds the full method (not just "navigate.to").
    assert!(e.message.contains("Page.navigate.to"));
}

#[test]
fn test_method_single_char_domain() {
    // Single-char domain "P" → unknown domain → -32601.
    let r = dispatch("P.x", None);
    assert!(r.error.is_some());
    assert_eq!(r.error.unwrap().code, -32601);
}

#[test]
fn test_method_trailing_dot() {
    // "Page." → domain="Page", command="" → unknown Page command.
    let r = dispatch("Page.", None);
    assert!(r.error.is_some());
    assert_eq!(r.error.unwrap().code, -32601);
}

#[test]
fn test_method_leading_dot() {
    // ".enable" → domain="", command="enable" → unknown domain "".
    let r = dispatch(".enable", None);
    assert!(r.error.is_some());
    assert_eq!(r.error.unwrap().code, -32601);
}

#[test]
fn test_method_only_dot() {
    // "." → domain="", command="" → unknown domain.
    let r = dispatch(".", None);
    assert!(r.error.is_some());
    assert_eq!(r.error.unwrap().code, -32601);
}

#[test]
fn test_method_unicode_domain_and_command() {
    // Unicode domain/command → not matched → -32601.
    let r = dispatch("测试.命令", None);
    assert!(r.error.is_some());
    let e = r.error.as_ref().unwrap();
    assert_eq!(e.code, -32601);
    assert!(e.message.contains("测试.命令"));
}

// =====================================================================
// ---- Target domain ---- (REQ-CDP-001, REQ-CDP-008)
// =====================================================================

#[test]
fn test_target_get_targets() {
    let r = dispatch("Target.getTargets", None);
    assert_jsonrpc_invariant(&r, "Target.getTargets");
    let result = r.result.unwrap();
    let infos = result["targetInfos"]
        .as_array()
        .expect("targetInfos must be array");
    assert!(!infos.is_empty(), "targetInfos must be non-empty");
    let info = &infos[0];
    // live_target_info default contract (no bridge):
    assert_eq!(info["targetId"], "test-target", "targetId must echo");
    assert_eq!(info["type"], "page");
    assert_eq!(info["title"], "Bao", "default title without bridge");
    assert_eq!(info["url"], "about:blank", "default url without bridge");
    assert_eq!(info["attached"], true);
}

#[test]
fn test_target_get_targets_target_id_echoes() {
    // Adversarial: target_id passed to handle_command must propagate into targetId field.
    let r = dispatch_target("custom-tid-42", "Target.getTargets", None);
    let info = &r.result.unwrap()["targetInfos"][0];
    assert_eq!(info["targetId"], "custom-tid-42");
}

#[test]
fn test_target_get_targets_empty_target_id() {
    // Adversarial: empty target_id is a valid boundary — must not panic.
    let r = dispatch_target("", "Target.getTargets", None);
    assert!(r.result.is_some());
    let info = &r.result.unwrap()["targetInfos"][0];
    assert_eq!(info["targetId"], "");
}

#[test]
fn test_target_get_targets_unicode_target_id() {
    // Adversarial: unicode target_id must round-trip.
    let r = dispatch_target("页面-001", "Target.getTargets", None);
    assert_eq!(r.result.unwrap()["targetInfos"][0]["targetId"], "页面-001");
}

#[test]
fn test_target_get_target_targets() {
    // getTargetTargets is an alias for getTargets — same schema.
    let r = dispatch("Target.getTargetTargets", None);
    assert_jsonrpc_invariant(&r, "Target.getTargetTargets");
    let result = r.result.unwrap();
    assert!(result["targetInfos"].is_array());
    assert!(!result["targetInfos"].as_array().unwrap().is_empty());
}

#[test]
fn test_target_get_targets_and_get_target_targets_equivalent_schema() {
    // Adversarial: both aliases must return the same set of fields.
    let a = dispatch("Target.getTargets", None).result.unwrap();
    let b = dispatch("Target.getTargetTargets", None).result.unwrap();
    let a_keys: std::collections::BTreeSet<String> = a["targetInfos"][0]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    let b_keys: std::collections::BTreeSet<String> = b["targetInfos"][0]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(
        a_keys, b_keys,
        "getTargets and getTargetTargets must have identical schema"
    );
}

#[test]
fn test_target_create_target() {
    let r = dispatch("Target.createTarget", Some(json!({"url":"http://test"})));
    assert_jsonrpc_invariant(&r, "Target.createTarget");
    let result = r.result.unwrap();
    assert_eq!(result["targetId"], "test-target");
    // createTarget returns ONLY targetId (no extra fields that could confuse clients).
    let obj = result.as_object().unwrap();
    assert_eq!(
        obj.len(),
        1,
        "createTarget result must contain only targetId"
    );
    assert!(obj.contains_key("targetId"));
}

#[test]
fn test_target_create_target_default_url() {
    // Adversarial: createTarget without url params — still returns targetId.
    let r = dispatch("Target.createTarget", None);
    assert_eq!(r.result.unwrap()["targetId"], "test-target");
}

#[test]
fn test_target_create_target_target_id_echoes() {
    let r = dispatch_target("page-99", "Target.createTarget", None);
    assert_eq!(r.result.unwrap()["targetId"], "page-99");
}

#[test]
fn test_target_close_target() {
    let r = dispatch("Target.closeTarget", Some(json!({"targetId":"t1"})));
    assert_jsonrpc_invariant(&r, "Target.closeTarget");
    let result = r.result.unwrap();
    assert_eq!(result["success"], true);
    let obj = result.as_object().unwrap();
    assert_eq!(obj.len(), 1, "closeTarget result must contain only success");
}

#[test]
fn test_target_close_target_no_params() {
    // Adversarial: closeTarget without targetId param — fire-and-forget still succeeds.
    let r = dispatch("Target.closeTarget", None);
    assert_eq!(r.result.unwrap()["success"], true);
}

#[test]
fn test_target_set_auto_attach() {
    let r = dispatch("Target.setAutoAttach", Some(json!({"flatten":true})));
    assert_jsonrpc_invariant(&r, "Target.setAutoAttach");
    assert!(ok_resp(
        "Target.setAutoAttach",
        Some(json!({"flatten":true}))
    ));
}

#[test]
fn test_target_set_discover_targets() {
    assert!(ok_resp("Target.setDiscoverTargets", None));
}

#[test]
fn test_target_get_target_info() {
    let r = dispatch("Target.getTargetInfo", None);
    assert_jsonrpc_invariant(&r, "Target.getTargetInfo");
    let info = r.result.unwrap()["targetInfo"].clone();
    assert_eq!(info["targetId"], "test-target");
    assert_eq!(info["type"], "page");
    // Full live_target_info schema:
    assert_eq!(info["title"], "Bao");
    assert_eq!(info["url"], "about:blank");
    assert_eq!(info["attached"], true);
    // targetInfo must have exactly these 5 fields.
    let keys: std::collections::BTreeSet<String> =
        info.as_object().unwrap().keys().cloned().collect();
    let expected: std::collections::BTreeSet<String> =
        ["targetId", "type", "title", "url", "attached"]
            .iter()
            .map(|s| s.to_string())
            .collect();
    assert_eq!(
        keys, expected,
        "targetInfo schema must be exactly targetId/type/title/url/attached"
    );
}

#[test]
fn test_target_attach_to_target() {
    let r = dispatch("Target.attachToTarget", None);
    assert_jsonrpc_invariant(&r, "Target.attachToTarget");
    let sid = r.result.unwrap()["sessionId"]
        .as_str()
        .expect("sessionId must be string")
        .to_string();
    assert!(!sid.is_empty(), "sessionId must be non-empty");
    // sessionId is a hex string of length 16 (format!("{:016x}", ...)).
    assert_eq!(sid.len(), 16, "sessionId must be 16-char hex");
    assert!(
        sid.chars().all(|c| c.is_ascii_hexdigit()),
        "sessionId must be hex: {}",
        sid
    );
}

#[test]
fn test_target_attach_to_target_deterministic_per_target_id() {
    // Adversarial: same target_id → same sessionId (deterministic hash of chars).
    let a = dispatch_target("abc", "Target.attachToTarget", None)
        .result
        .unwrap()["sessionId"]
        .clone();
    let b = dispatch_target("abc", "Target.attachToTarget", None)
        .result
        .unwrap()["sessionId"]
        .clone();
    assert_eq!(
        a, b,
        "sessionId must be deterministic for the same target_id"
    );
    // Different target_id → different sessionId.
    let c = dispatch_target("xyz", "Target.attachToTarget", None)
        .result
        .unwrap()["sessionId"]
        .clone();
    assert_ne!(a, c, "sessionId must differ for different target_id");
}

#[test]
fn test_target_attach_to_target_empty_target_id() {
    // Adversarial: empty target_id → sum of chars = 0 → "0000000000000000".
    let r = dispatch_target("", "Target.attachToTarget", None);
    assert_eq!(r.result.unwrap()["sessionId"], "0000000000000000");
}

#[test]
fn test_target_detach_from_target() {
    assert!(ok_resp("Target.detachFromTarget", None));
}

#[test]
fn test_target_send_message_to_target() {
    assert!(ok_resp("Target.sendMessageToTarget", None));
}

#[test]
fn test_target_unknown() {
    assert_eq!(err_code("Target.nonexistent"), -32601);
}

// =====================================================================
// ---- Page domain sub-commands ---- (REQ-CDP-004)
// =====================================================================

#[test]
fn test_page_enable() {
    let r = dispatch("Page.enable", None);
    assert_jsonrpc_invariant(&r, "Page.enable");
    assert!(ok_resp("Page.enable", None));
    // enable/disable return empty object (CDP convention for stateless acks).
    assert_eq!(r.result.unwrap(), json!({}));
}

#[test]
fn test_page_disable() {
    let r = dispatch("Page.disable", None);
    assert_jsonrpc_invariant(&r, "Page.disable");
    assert_eq!(r.result.unwrap(), json!({}));
}

#[test]
fn test_page_navigate_default_url() {
    let r = dispatch("Page.navigate", Some(json!({})));
    assert_jsonrpc_invariant(&r, "Page.navigate");
    let result = r.result.unwrap();
    assert_eq!(result["frameId"], "0");
    // NavigateReturnObject also carries loaderId (REQ-CDP-004-C1).
    assert!(
        result.get("loaderId").is_some(),
        "Page.navigate must return loaderId"
    );
    let loader_id = result["loaderId"]
        .as_str()
        .expect("loaderId must be string");
    assert!(!loader_id.is_empty(), "loaderId must be non-empty");
}

#[test]
fn test_page_navigate_no_params() {
    // Adversarial: navigate with no params at all → default url about:blank → len 11.
    let r = dispatch("Page.navigate", None);
    let result = r.result.unwrap();
    assert_eq!(result["frameId"], "0");
    // loaderId = format!("{:016x}", url.len()) where url="about:blank" (11 chars).
    assert_eq!(result["loaderId"], "000000000000000b");
}

#[test]
fn test_page_navigate_with_url() {
    let r = dispatch("Page.navigate", Some(json!({"url":"https://example.com"})));
    let result = r.result.unwrap();
    assert_eq!(result["frameId"], "0");
    assert!(result.get("loaderId").is_some());
}

#[test]
fn test_page_navigate_loader_id_depends_on_url_length() {
    // Adversarial: loaderId is hex of url.len() — different url lengths → different loaderId.
    let short = dispatch("Page.navigate", Some(json!({"url":"ab"})))
        .result
        .unwrap()["loaderId"]
        .clone();
    let long = dispatch("Page.navigate", Some(json!({"url":"abcdefghij"})))
        .result
        .unwrap()["loaderId"]
        .clone();
    assert_ne!(short, long, "loaderId must vary with url length");
    assert_eq!(short, "0000000000000002");
    assert_eq!(long, "000000000000000a");
}

#[test]
fn test_page_navigate_empty_url_uses_default() {
    // Adversarial: url="" (empty string) is falsy → defaults to about:blank.
    let r = dispatch("Page.navigate", Some(json!({"url":""})));
    let result = r.result.unwrap();
    // about:blank is 11 chars → loaderId = 0x0b = "000000000000000b".
    assert_eq!(result["loaderId"], "000000000000000b");
}

#[test]
fn test_page_navigate_url_wrong_type() {
    // Adversarial: url as number (not string) → as_str() returns None → defaults to about:blank.
    let r = dispatch("Page.navigate", Some(json!({"url":12345})));
    let result = r.result.unwrap();
    assert_eq!(result["frameId"], "0");
}

#[test]
fn test_page_reload_default() {
    let r = dispatch("Page.reload", None);
    assert_jsonrpc_invariant(&r, "Page.reload");
    let result = r.result.unwrap();
    assert_eq!(result["frameId"], "0");
    assert_eq!(result["loaderId"], "0");
}

#[test]
fn test_reload_ignore_cache_present() {
    let r = dispatch("Page.reload", Some(json!({"ignoreCache":true})));
    assert_jsonrpc_invariant(&r, "Page.reload");
    assert!(r.result.is_some());
    let result = r.result.unwrap();
    // reload always returns frameId="0", loaderId="0" regardless of ignoreCache.
    assert_eq!(result["frameId"], "0");
    assert_eq!(result["loaderId"], "0");
}

#[test]
fn test_page_reload_ignore_cache_false() {
    // Adversarial: ignoreCache=false explicitly → still ok.
    let r = dispatch("Page.reload", Some(json!({"ignoreCache":false})));
    assert!(r.result.is_some());
}

#[test]
fn test_page_get_frame_tree() {
    let r = dispatch("Page.getFrameTree", None);
    assert_jsonrpc_invariant(&r, "Page.getFrameTree");
    let frame = r.result.unwrap()["frameTree"]["frame"].clone();
    assert_eq!(frame["id"], "0");
    // Full frame schema (REQ-CDP-004-C5):
    assert!(frame["url"].is_string(), "frame.url must be string");
    assert!(
        frame["loaderId"].is_string(),
        "frame.loaderId must be string"
    );
    assert_eq!(frame["mimeType"], "text/html");
    let keys: std::collections::BTreeSet<String> =
        frame.as_object().unwrap().keys().cloned().collect();
    for required in ["id", "url", "loaderId", "mimeType"] {
        assert!(keys.contains(required), "frame must contain {}", required);
    }
}

#[test]
fn test_page_get_frame_tree_default_url() {
    // Adversarial: no bridge → frame.url defaults to about:blank.
    let frame = dispatch("Page.getFrameTree", None).result.unwrap()["frameTree"]["frame"].clone();
    assert_eq!(frame["url"], "about:blank");
}

#[test]
fn test_page_get_navigation_history() {
    let r = dispatch("Page.getNavigationHistory", None);
    assert_jsonrpc_invariant(&r, "Page.getNavigationHistory");
    let result = r.result.unwrap();
    assert_eq!(result["currentIndex"], 0);
    assert!(result["entries"].is_array());
    let entries = result["entries"].as_array().unwrap();
    assert_eq!(
        entries.len(),
        1,
        "default navigation history has exactly 1 entry"
    );
    let entry = &entries[0];
    assert_eq!(entry["id"], 0);
    assert!(entry["url"].is_string());
    assert_eq!(entry["title"], "");
}

#[test]
fn test_page_capture_screenshot_default() {
    let r = dispatch("Page.captureScreenshot", None);
    assert_jsonrpc_invariant(&r, "Page.captureScreenshot");
    // No bridge → data is empty string (not absent).
    let result = r.result.unwrap();
    assert!(result["data"].is_string(), "data must be string");
    assert_eq!(
        result["data"], "",
        "no-bridge screenshot data must be empty string"
    );
}

#[test]
fn test_page_capture_screenshot_jpeg() {
    let r = dispatch("Page.captureScreenshot", Some(json!({"format":"jpeg"})));
    assert_jsonrpc_invariant(&r, "Page.captureScreenshot");
    assert!(r.result.is_some());
    // No bridge: format/quality params are accepted but data is still empty.
    assert_eq!(r.result.unwrap()["data"], "");
}

#[test]
fn test_page_capture_screenshot_with_quality() {
    // Adversarial: quality param alone (no format) — must not panic.
    let r = dispatch("Page.captureScreenshot", Some(json!({"quality":80})));
    assert!(r.result.is_some());
}

#[test]
fn test_page_capture_screenshot_webp_format() {
    // Adversarial: webp format — accepted, no panic.
    let r = dispatch("Page.captureScreenshot", Some(json!({"format":"webp"})));
    assert!(r.result.is_some());
}

#[test]
fn test_page_set_content() {
    let r = dispatch("Page.setContent", None);
    assert_jsonrpc_invariant(&r, "Page.setContent");
    assert_eq!(r.result.unwrap(), json!({}));
}

#[test]
fn test_page_close() {
    assert!(ok_resp("Page.close", None));
}

#[test]
fn test_page_bring_to_front() {
    assert!(ok_resp("Page.bringToFront", None));
}

#[test]
fn test_page_get_layout_metrics() {
    let r = dispatch("Page.getLayoutMetrics", None);
    assert_jsonrpc_invariant(&r, "Page.getLayoutMetrics");
    let result = r.result.unwrap();
    assert!(result["contentSize"]["width"].is_number());
    // Full layout metrics schema:
    assert_eq!(result["contentSize"]["width"], 1920);
    assert_eq!(result["contentSize"]["height"], 1080);
    assert_eq!(result["contentSize"]["x"], 0);
    assert_eq!(result["contentSize"]["y"], 0);
    assert_eq!(result["cssContentSize"]["width"], 1920);
    assert_eq!(result["cssContentSize"]["height"], 1080);
}

#[test]
fn test_page_add_script() {
    let r = dispatch(
        "Page.addScriptToEvaluateOnNewDocument",
        Some(json!({"source":"console.log(1)"})),
    );
    assert_jsonrpc_invariant(&r, "Page.addScriptToEvaluateOnNewDocument");
    assert_eq!(r.result.unwrap()["identifier"], "1");
}

#[test]
fn test_page_add_script_empty_source() {
    // Adversarial: empty source — still returns identifier "1".
    let r = dispatch(
        "Page.addScriptToEvaluateOnNewDocument",
        Some(json!({"source":""})),
    );
    assert_eq!(r.result.unwrap()["identifier"], "1");
}

#[test]
fn test_page_add_script_no_source_key() {
    // Adversarial: missing source key — still returns identifier "1".
    let r = dispatch("Page.addScriptToEvaluateOnNewDocument", None);
    assert_eq!(r.result.unwrap()["identifier"], "1");
}

#[test]
fn test_page_remove_script() {
    assert!(ok_resp("Page.removeScriptToEvaluateOnNewDocument", None));
}

#[test]
fn test_page_unknown() {
    let r = dispatch("Page.nonexistent", None);
    assert!(r.error.is_some());
    let e = r.error.as_ref().unwrap();
    assert_eq!(e.code, -32601);
    assert!(e.message.contains("Page.nonexistent"));
}

// =====================================================================
// ---- Runtime domain ---- (REQ-CDP-002)
// =====================================================================

#[test]
fn test_runtime_enable() {
    let r = dispatch("Runtime.enable", None);
    assert_jsonrpc_invariant(&r, "Runtime.enable");
    let result = r.result.unwrap();
    assert!(result["executionContextId"].is_number());
    assert_eq!(
        result["executionContextId"], 1,
        "default execution context id is 1"
    );
}

#[test]
fn test_runtime_disable() {
    assert!(ok_resp("Runtime.disable", None));
}

#[test]
fn test_runtime_evaluate_default() {
    let r = dispatch("Runtime.evaluate", None);
    assert_jsonrpc_invariant(&r, "Runtime.evaluate");
    let result = r.result.unwrap();
    assert_eq!(result["result"]["type"], "undefined");
    // No-bridge / empty expression also carries exceptionDetails: null.
    assert!(
        result.get("exceptionDetails").is_some(),
        "Runtime.evaluate must return exceptionDetails"
    );
    assert_eq!(result["exceptionDetails"], serde_json::Value::Null);
}

#[test]
fn test_runtime_evaluate_empty_expression() {
    // Adversarial: empty expression string → undefined result, no bridge call.
    let r = dispatch("Runtime.evaluate", Some(json!({"expression":""})));
    let result = r.result.unwrap();
    assert_eq!(result["result"]["type"], "undefined");
    assert_eq!(result["exceptionDetails"], serde_json::Value::Null);
}

#[test]
fn test_runtime_evaluate_expression_no_bridge() {
    // Adversarial: non-empty expression with NO bridge → still returns undefined stub
    // (bridge.is_some() is false, so expression is accepted but not forwarded).
    let r = dispatch("Runtime.evaluate", Some(json!({"expression":"1+1"})));
    assert!(r.result.is_some());
    let result = r.result.unwrap();
    assert_eq!(result["result"]["type"], "undefined");
}

#[test]
fn test_runtime_evaluate_return_by_value_false() {
    // Adversarial: returnByValue=false — param is read but without bridge, stub returned.
    let r = dispatch(
        "Runtime.evaluate",
        Some(json!({"expression":"x","returnByValue":false})),
    );
    assert!(r.result.is_some());
}

#[test]
fn test_runtime_evaluate_expression_wrong_type() {
    // Adversarial: expression as number (not string) → as_str() None → defaults to "".
    let r = dispatch("Runtime.evaluate", Some(json!({"expression":42})));
    let result = r.result.unwrap();
    assert_eq!(result["result"]["type"], "undefined");
}

#[test]
fn test_runtime_call_function_on() {
    let r = dispatch("Runtime.callFunctionOn", None);
    assert_jsonrpc_invariant(&r, "Runtime.callFunctionOn");
    assert_eq!(r.result.unwrap()["result"]["type"], "undefined");
}

#[test]
fn test_runtime_get_properties() {
    let r = dispatch("Runtime.getProperties", None);
    assert_jsonrpc_invariant(&r, "Runtime.getProperties");
    let result = r.result.as_ref().unwrap();
    assert!(result["result"].is_array());
    // Default: empty array.
    assert_eq!(result["result"].as_array().unwrap().len(), 0);
}

#[test]
fn test_runtime_run_script() {
    assert!(ok_resp("Runtime.runScript", None));
}

#[test]
fn test_runtime_release_object() {
    assert!(ok_resp("Runtime.releaseObject", None));
}

#[test]
fn test_runtime_release_object_group() {
    assert!(ok_resp("Runtime.releaseObjectGroup", None));
}

#[test]
fn test_runtime_compile_script() {
    assert!(ok_resp("Runtime.compileScript", None));
}

#[test]
fn test_runtime_unknown() {
    let r = dispatch("Runtime.nonexistent", None);
    let e = r.error.unwrap();
    assert_eq!(e.code, -32601);
    assert!(e.message.contains("Runtime.nonexistent"));
}

// =====================================================================
// ---- DOM domain ---- (REQ-CDP-005)
// =====================================================================

#[test]
fn test_dom_enable() {
    assert!(ok_resp("DOM.enable", None));
}

#[test]
fn test_dom_disable() {
    assert!(ok_resp("DOM.disable", None));
}

#[test]
fn test_dom_get_document() {
    let r = dispatch("DOM.getDocument", None);
    assert_jsonrpc_invariant(&r, "DOM.getDocument");
    let root = r.result.unwrap()["root"].clone();
    assert_eq!(root["nodeType"], 9);
    assert_eq!(root["nodeName"], "#document");
    // Full root node schema (REQ-CDP-005-C1):
    assert_eq!(root["nodeId"], 1);
    assert_eq!(root["backendNodeId"], 1);
    assert_eq!(root["localName"], "");
    assert_eq!(root["nodeValue"], "");
    assert!(
        root["childNodeCount"].as_i64().unwrap_or(0) >= 1,
        "document must have >=1 child"
    );
    let children = root["children"].as_array().expect("children must be array");
    assert!(!children.is_empty());
    let html = &children[0];
    assert_eq!(html["nodeType"], 1);
    assert_eq!(html["nodeName"], "HTML");
    assert_eq!(html["localName"], "html");
}

#[test]
fn test_dom_describe_node() {
    let r = dispatch("DOM.describeNode", None);
    assert_jsonrpc_invariant(&r, "DOM.describeNode");
    let node = r.result.unwrap()["node"].clone();
    assert!(node["nodeName"].is_string());
    assert_eq!(node["nodeId"], 1);
    assert_eq!(node["nodeType"], 1);
    assert_eq!(node["nodeName"], "HTML");
}

#[test]
fn test_dom_query_selector_default() {
    let r = dispatch("DOM.querySelector", None);
    assert_jsonrpc_invariant(&r, "DOM.querySelector");
    assert_eq!(r.result.unwrap()["nodeId"], 0);
}

#[test]
fn test_dom_query_selector_with_selector_no_bridge() {
    // Adversarial: non-empty selector, no bridge → still nodeId:0 (stub).
    let r = dispatch("DOM.querySelector", Some(json!({"selector":"div.active"})));
    assert_eq!(r.result.unwrap()["nodeId"], 0);
}

#[test]
fn test_dom_query_selector_all_default() {
    let r = dispatch("DOM.querySelectorAll", None);
    assert_jsonrpc_invariant(&r, "DOM.querySelectorAll");
    assert!(r.result.unwrap()["nodeIds"].is_array());
}

#[test]
fn test_dom_query_selector_all_with_selector_no_bridge() {
    // Adversarial: non-empty selector, no bridge → empty nodeIds.
    let r = dispatch("DOM.querySelectorAll", Some(json!({"selector":"div"})));
    let result = r.result.unwrap();
    assert_eq!(result["nodeIds"], json!([]));
}

#[test]
fn test_dom_get_box_model() {
    let r = dispatch("DOM.getBoxModel", None);
    assert_jsonrpc_invariant(&r, "DOM.getBoxModel");
    let model = r.result.unwrap()["model"].clone();
    assert!(model["width"].is_number());
    assert_eq!(model["width"], 1920);
    assert_eq!(model["height"], 1080);
    let content = model["content"].as_array().expect("content must be array");
    assert_eq!(
        content.len(),
        8,
        "box model content has 8 coords (4 corners × 2)"
    );
    // Quad corners: (0,0) (1920,0) (1920,1080) (0,1080).
    assert_eq!(content[0], 0);
    assert_eq!(content[1], 0);
    assert_eq!(content[2], 1920);
    assert_eq!(content[3], 0);
    assert_eq!(content[4], 1920);
    assert_eq!(content[5], 1080);
    assert_eq!(content[6], 0);
    assert_eq!(content[7], 1080);
}

#[test]
fn test_dom_set_attribute_value() {
    let r = dispatch(
        "DOM.setAttributeValue",
        Some(json!({"nodeId":1,"name":"class","value":"active"})),
    );
    assert_jsonrpc_invariant(&r, "DOM.setAttributeValue");
    // No bridge → empty object ack.
    assert_eq!(r.result.unwrap(), json!({}));
}

#[test]
fn test_dom_set_attribute_value_missing_node_id() {
    // Adversarial: missing nodeId → defaults to 0, no panic.
    let r = dispatch(
        "DOM.setAttributeValue",
        Some(json!({"name":"class","value":"x"})),
    );
    assert!(r.result.is_some());
}

#[test]
fn test_dom_remove_attribute() {
    assert!(ok_resp("DOM.removeAttribute", None));
}

#[test]
fn test_dom_set_outer_html() {
    assert!(ok_resp("DOM.setOuterHTML", None));
}

#[test]
fn test_dom_insert_before() {
    assert!(ok_resp("DOM.insertBefore", None));
}

#[test]
fn test_dom_remove_node() {
    assert!(ok_resp("DOM.removeNode", None));
}

#[test]
fn test_dom_get_outer_html_default() {
    let r = dispatch("DOM.getOuterHTML", None);
    assert_jsonrpc_invariant(&r, "DOM.getOuterHTML");
    let result = r.result.unwrap();
    assert!(result["outerHTML"].is_string());
    // No-bridge default HTML.
    assert_eq!(result["outerHTML"], "<html><body></body></html>");
}

#[test]
fn test_dom_resolve_node() {
    let r = dispatch("DOM.resolveNode", None);
    assert_jsonrpc_invariant(&r, "DOM.resolveNode");
    assert_eq!(r.result.unwrap()["object"]["type"], "node");
}

#[test]
fn test_dom_push_nodes() {
    let r = dispatch("DOM.pushNodesByBackendIdsToFrontend", None);
    assert_jsonrpc_invariant(&r, "DOM.pushNodesByBackendIdsToFrontend");
    // Default: empty nodeIds.
    assert_eq!(r.result.unwrap()["nodeIds"], json!([]));
}

#[test]
fn test_dom_unknown() {
    let r = dispatch("DOM.nonexistent", None);
    let e = r.error.unwrap();
    assert_eq!(e.code, -32601);
    assert!(e.message.contains("DOM.nonexistent"));
}

// =====================================================================
// ---- Network domain ---- (REQ-CDP-006)
// =====================================================================

#[test]
fn test_network_enable() {
    assert!(ok_resp("Network.enable", None));
}

#[test]
fn test_network_disable() {
    assert!(ok_resp("Network.disable", None));
}

#[test]
fn test_network_get_response_body() {
    let r = dispatch("Network.getResponseBody", None);
    assert_jsonrpc_invariant(&r, "Network.getResponseBody");
    let result = r.result.unwrap();
    assert_eq!(result["base64Encoded"], false);
    // Full schema (REQ-CDP-006-C5):
    assert_eq!(result["body"], "", "default body is empty string");
    assert_eq!(result["base64Encoded"], false);
}

#[test]
fn test_network_set_cache_disabled() {
    assert!(ok_resp("Network.setCacheDisabled", None));
}

#[test]
fn test_network_set_extra_http_headers() {
    assert!(ok_resp("Network.setExtraHTTPHeaders", None));
}

#[test]
fn test_network_emulate_conditions() {
    assert!(ok_resp("Network.emulateNetworkConditions", None));
}

#[test]
fn test_network_set_request_interception() {
    assert!(ok_resp("Network.setRequestInterception", None));
}

#[test]
fn test_network_continue_intercepted() {
    assert!(ok_resp("Network.continueInterceptedRequest", None));
}

#[test]
fn test_network_get_cookies() {
    let r = dispatch("Network.getCookies", None);
    assert_jsonrpc_invariant(&r, "Network.getCookies");
    assert!(r.result.unwrap()["cookies"].is_array());
}

#[test]
fn test_network_get_all_cookies() {
    let r = dispatch("Network.getAllCookies", None);
    assert_jsonrpc_invariant(&r, "Network.getAllCookies");
    assert!(r.result.unwrap()["cookies"].is_array());
}

#[test]
fn test_network_get_cookies_and_all_cookies_default_empty() {
    // Adversarial: both default to empty arrays.
    assert_eq!(
        dispatch("Network.getCookies", None).result.unwrap()["cookies"],
        json!([])
    );
    assert_eq!(
        dispatch("Network.getAllCookies", None).result.unwrap()["cookies"],
        json!([])
    );
}

#[test]
fn test_network_delete_cookies() {
    assert!(ok_resp("Network.deleteCookies", None));
}

#[test]
fn test_network_set_cookie() {
    assert!(ok_resp("Network.setCookie", None));
}

#[test]
fn test_network_unknown() {
    let r = dispatch("Network.nonexistent", None);
    let e = r.error.unwrap();
    assert_eq!(e.code, -32601);
    assert!(e.message.contains("Network.nonexistent"));
}

// =====================================================================
// ---- CSS domain ---- (REQ-CDP-007)
// =====================================================================

#[test]
fn test_css_enable() {
    assert!(ok_resp("CSS.enable", None));
}

#[test]
fn test_css_disable() {
    assert!(ok_resp("CSS.disable", None));
}

#[test]
fn test_css_get_computed_style() {
    let r = dispatch("CSS.getComputedStyleForNode", None);
    assert_jsonrpc_invariant(&r, "CSS.getComputedStyleForNode");
    let result = r.result.unwrap();
    assert!(result["computedStyle"].is_array());
    assert_eq!(result["computedStyle"], json!([]));
}

#[test]
fn test_css_get_matched_styles() {
    let r = dispatch("CSS.getMatchedStylesForNode", None);
    assert_jsonrpc_invariant(&r, "CSS.getMatchedStylesForNode");
    let result = r.result.unwrap();
    assert!(result["matchedCSSRules"].is_array());
    // Full schema (REQ-CDP-007-C1):
    assert_eq!(result["matchedCSSRules"], json!([]));
    assert_eq!(result["inlineStyle"], serde_json::Value::Null);
    assert_eq!(result["attributesStyle"], serde_json::Value::Null);
}

#[test]
fn test_css_get_inline_styles() {
    let r = dispatch("CSS.getInlineStylesForNode", None);
    assert_jsonrpc_invariant(&r, "CSS.getInlineStylesForNode");
    assert!(r.result.unwrap()["inlineStyle"].is_null());
}

#[test]
fn test_css_set_style_texts() {
    let r = dispatch("CSS.setStyleTexts", None);
    assert_jsonrpc_invariant(&r, "CSS.setStyleTexts");
    let result = r.result.as_ref().unwrap();
    assert!(result["styles"].is_array());
    assert_eq!(result["styles"], json!([]));
}

#[test]
fn test_css_unknown() {
    let r = dispatch("CSS.nonexistent", None);
    let e = r.error.unwrap();
    assert_eq!(e.code, -32601);
    assert!(e.message.contains("CSS.nonexistent"));
}

// =====================================================================
// ---- Emulation domain ---- (REQ-CDP-007)
// =====================================================================

#[test]
fn test_emulation_set_device_metrics() {
    let r = dispatch(
        "Emulation.setDeviceMetricsOverride",
        Some(json!({"width":1280,"height":720})),
    );
    assert_jsonrpc_invariant(&r, "Emulation.setDeviceMetricsOverride");
    assert!(r.result.is_some());
}

#[test]
fn test_emulation_set_device_metrics_default() {
    let r = dispatch("Emulation.setDeviceMetricsOverride", None);
    assert_jsonrpc_invariant(&r, "Emulation.setDeviceMetricsOverride");
    // No bridge → empty ack.
    assert_eq!(r.result.unwrap(), json!({}));
}

#[test]
fn test_emulation_set_device_metrics_with_dsf() {
    // Adversarial: deviceScaleFactor present — must not panic.
    let r = dispatch(
        "Emulation.setDeviceMetricsOverride",
        Some(json!({"width":800,"height":600,"deviceScaleFactor":2.0})),
    );
    assert!(r.result.is_some());
}

#[test]
fn test_emulation_clear_device_metrics() {
    assert!(ok_resp("Emulation.clearDeviceMetricsOverride", None));
}

#[test]
fn test_emulation_set_user_agent() {
    let r = dispatch(
        "Emulation.setUserAgentOverride",
        Some(json!({"userAgent":"TestBot"})),
    );
    assert_jsonrpc_invariant(&r, "Emulation.setUserAgentOverride");
    // No bridge → empty ack (UA is accepted but not forwarded).
    assert_eq!(r.result.unwrap(), json!({}));
}

#[test]
fn test_emulation_set_user_agent_empty() {
    let r = dispatch("Emulation.setUserAgentOverride", None);
    assert_jsonrpc_invariant(&r, "Emulation.setUserAgentOverride");
    // Empty UA + no bridge → empty ack.
    assert_eq!(r.result.unwrap(), json!({}));
}

#[test]
fn test_emulation_set_touch() {
    assert!(ok_resp("Emulation.setTouchEmulationEnabled", None));
}

#[test]
fn test_emulation_set_scriptdisabled() {
    assert!(ok_resp("Emulation.setScriptExecutionDisabled", None));
}

#[test]
fn test_emulation_set_focus() {
    assert!(ok_resp("Emulation.setFocusEmulationEnabled", None));
}

#[test]
fn test_emulation_set_cpu_throttle() {
    assert!(ok_resp("Emulation.setCPUThrottlingRate", None));
}

#[test]
fn test_emulation_set_bg_color() {
    assert!(ok_resp("Emulation.setDefaultBackgroundColorOverride", None));
}

#[test]
fn test_emulation_unknown() {
    let r = dispatch("Emulation.nonexistent", None);
    let e = r.error.unwrap();
    assert_eq!(e.code, -32601);
    assert!(e.message.contains("Emulation.nonexistent"));
}

// =====================================================================
// ---- Input domain ---- (REQ-CDP-007)
// =====================================================================

#[test]
fn test_input_dispatch_mouse() {
    let r = dispatch(
        "Input.dispatchMouseEvent",
        Some(json!({"type":"mousePressed","x":10,"y":20})),
    );
    assert_jsonrpc_invariant(&r, "Input.dispatchMouseEvent");
    // No bridge → empty ack.
    assert_eq!(r.result.unwrap(), json!({}));
}

#[test]
fn test_input_dispatch_mouse_default() {
    let r = dispatch("Input.dispatchMouseEvent", None);
    assert_jsonrpc_invariant(&r, "Input.dispatchMouseEvent");
    assert_eq!(r.result.unwrap(), json!({}));
}

#[test]
fn test_input_dispatch_mouse_with_button_and_click_count() {
    // Adversarial: full mouse event params — must not panic.
    let r = dispatch(
        "Input.dispatchMouseEvent",
        Some(json!({
            "type":"mouseReleased","x":100,"y":200,"button":1,"clickCount":2
        })),
    );
    assert!(r.result.is_some());
}

#[test]
fn test_input_dispatch_key() {
    let r = dispatch(
        "Input.dispatchKeyEvent",
        Some(json!({"type":"keyDown","key":"a","code":"KeyA"})),
    );
    assert_jsonrpc_invariant(&r, "Input.dispatchKeyEvent");
    assert_eq!(r.result.unwrap(), json!({}));
}

#[test]
fn test_input_dispatch_key_with_text() {
    // Adversarial: keyDown with text payload — must not panic.
    let r = dispatch(
        "Input.dispatchKeyEvent",
        Some(json!({
            "type":"char","key":"a","code":"KeyA","text":"a"
        })),
    );
    assert!(r.result.is_some());
}

#[test]
fn test_input_dispatch_touch() {
    assert!(ok_resp("Input.dispatchTouchEvent", None));
}

#[test]
fn test_input_insert_text() {
    let r = dispatch("Input.insertText", Some(json!({"text":"hello"})));
    assert_jsonrpc_invariant(&r, "Input.insertText");
    assert_eq!(r.result.unwrap(), json!({}));
}

#[test]
fn test_input_insert_text_empty() {
    let r = dispatch("Input.insertText", None);
    assert_jsonrpc_invariant(&r, "Input.insertText");
    assert_eq!(r.result.unwrap(), json!({}));
}

#[test]
fn test_input_set_ignore() {
    assert!(ok_resp("Input.setIgnoreInputEvents", None));
}

#[test]
fn test_input_set_intercept_drags() {
    assert!(ok_resp("Input.setInterceptDrags", None));
}

#[test]
fn test_input_unknown() {
    let r = dispatch("Input.nonexistent", None);
    let e = r.error.unwrap();
    assert_eq!(e.code, -32601);
    assert!(e.message.contains("Input.nonexistent"));
}

// =====================================================================
// ---- Overlay domain ---- (REQ-CDP-007)
// =====================================================================

#[test]
fn test_overlay_enable() {
    assert!(ok_resp("Overlay.enable", None));
}

#[test]
fn test_overlay_disable() {
    assert!(ok_resp("Overlay.disable", None));
}

#[test]
fn test_overlay_highlight_node() {
    assert!(ok_resp("Overlay.highlightNode", None));
}

#[test]
fn test_overlay_hide_highlight() {
    assert!(ok_resp("Overlay.hideHighlight", None));
}

#[test]
fn test_overlay_set_inspect_mode() {
    assert!(ok_resp("Overlay.setInspectMode", None));
}

#[test]
fn test_overlay_set_paused_message() {
    assert!(ok_resp("Overlay.setPausedInDebuggerMessage", None));
}

#[test]
fn test_overlay_unknown() {
    let r = dispatch("Overlay.nonexistent", None);
    let e = r.error.unwrap();
    assert_eq!(e.code, -32601);
    assert!(e.message.contains("Overlay.nonexistent"));
}

// =====================================================================
// ---- Debugger domain ---- (REQ-CDP-003)
// =====================================================================

#[test]
fn test_debugger_enable() {
    assert!(ok_resp("Debugger.enable", None));
}

#[test]
fn test_debugger_disable() {
    assert!(ok_resp("Debugger.disable", None));
}

#[test]
fn test_debugger_set_breakpoint_by_url() {
    let r = dispatch("Debugger.setBreakpointByUrl", None);
    assert_jsonrpc_invariant(&r, "Debugger.setBreakpointByUrl");
    let result = r.result.unwrap();
    assert!(result["breakpointId"].is_string());
    // Full schema (REQ-CDP-003-C2):
    assert_eq!(result["breakpointId"], "1");
    assert!(result["locations"].is_array());
    assert_eq!(result["locations"], json!([]));
}

#[test]
fn test_debugger_remove_breakpoint() {
    assert!(ok_resp("Debugger.removeBreakpoint", None));
}

#[test]
fn test_debugger_pause() {
    assert!(ok_resp("Debugger.pause", None));
}

#[test]
fn test_debugger_resume() {
    assert!(ok_resp("Debugger.resume", None));
}

#[test]
fn test_debugger_step_over() {
    assert!(ok_resp("Debugger.stepOver", None));
}

#[test]
fn test_debugger_step_into() {
    assert!(ok_resp("Debugger.stepInto", None));
}

#[test]
fn test_debugger_step_out() {
    assert!(ok_resp("Debugger.stepOut", None));
}

#[test]
fn test_debugger_set_skip_all() {
    assert!(ok_resp("Debugger.setSkipAllPauses", None));
}

#[test]
fn test_debugger_set_breakpoints_active() {
    assert!(ok_resp("Debugger.setBreakpointsActive", None));
}

#[test]
fn test_debugger_evaluate_on_call_frame() {
    let r = dispatch("Debugger.evaluateOnCallFrame", None);
    assert_jsonrpc_invariant(&r, "Debugger.evaluateOnCallFrame");
    assert_eq!(r.result.unwrap()["result"]["type"], "undefined");
}

#[test]
fn test_debugger_get_possible_breakpoints() {
    let r = dispatch("Debugger.getPossibleBreakpoints", None);
    assert_jsonrpc_invariant(&r, "Debugger.getPossibleBreakpoints");
    let result = r.result.unwrap();
    assert!(result["locations"].is_array());
    assert_eq!(result["locations"], json!([]));
}

#[test]
fn test_debugger_get_script_source() {
    let r = dispatch("Debugger.getScriptSource", None);
    assert_jsonrpc_invariant(&r, "Debugger.getScriptSource");
    let result = r.result.unwrap();
    assert!(result["scriptSource"].is_string());
    // Default empty source.
    assert_eq!(result["scriptSource"], "");
}

#[test]
fn test_debugger_set_pause_on_exceptions() {
    assert!(ok_resp("Debugger.setPauseOnExceptions", None));
}

#[test]
fn test_debugger_unknown() {
    let r = dispatch("Debugger.nonexistent", None);
    let e = r.error.unwrap();
    assert_eq!(e.code, -32601);
    assert!(e.message.contains("Debugger.nonexistent"));
}

// =====================================================================
// ---- Log domain ----
// =====================================================================

#[test]
fn test_log_enable() {
    assert!(ok_resp("Log.enable", None));
}

#[test]
fn test_log_disable() {
    assert!(ok_resp("Log.disable", None));
}

#[test]
fn test_log_clear() {
    assert!(ok_resp("Log.clear", None));
}

#[test]
fn test_log_start_violations() {
    assert!(ok_resp("Log.startViolationsReport", None));
}

#[test]
fn test_log_stop_violations() {
    assert!(ok_resp("Log.stopViolationsReport", None));
}

#[test]
fn test_log_unknown() {
    let r = dispatch("Log.nonexistent", None);
    let e = r.error.unwrap();
    assert_eq!(e.code, -32601);
    assert!(e.message.contains("Log.nonexistent"));
}

// =====================================================================
// ---- Fetch domain ----
// =====================================================================

#[test]
fn test_fetch_enable() {
    let r = dispatch("Fetch.enable", None);
    assert_jsonrpc_invariant(&r, "Fetch.enable");
    let result = r.result.unwrap();
    assert_eq!(result["enabled"], true);
    assert_eq!(result["patternCount"], 0, "no patterns → patternCount 0");
}

#[test]
fn test_fetch_enable_with_patterns() {
    let r = dispatch(
        "Fetch.enable",
        Some(json!({"patterns":[{"urlPattern":"*"}]})),
    );
    assert_jsonrpc_invariant(&r, "Fetch.enable");
    let result = r.result.unwrap();
    assert_eq!(result["patternCount"], 1);
    assert_eq!(result["enabled"], true);
}

#[test]
fn test_fetch_enable_with_multiple_patterns() {
    // Adversarial: multiple patterns → patternCount reflects array length.
    let r = dispatch(
        "Fetch.enable",
        Some(json!({
            "patterns":[{"urlPattern":"*.js"},{"urlPattern":"*.css"},{"urlPattern":"*.png"}]
        })),
    );
    assert_eq!(r.result.unwrap()["patternCount"], 3);
}

#[test]
fn test_fetch_enable_with_empty_patterns_array() {
    // Adversarial: empty patterns array → patternCount 0.
    let r = dispatch("Fetch.enable", Some(json!({"patterns":[]})));
    assert_eq!(r.result.unwrap()["patternCount"], 0);
}

#[test]
fn test_fetch_enable_with_patterns_wrong_type() {
    // Adversarial: patterns as string (not array) → unwrap_or(0) → patternCount 0.
    let r = dispatch("Fetch.enable", Some(json!({"patterns":"not-an-array"})));
    assert_eq!(r.result.unwrap()["patternCount"], 0);
}

#[test]
fn test_fetch_disable() {
    assert!(ok_resp("Fetch.disable", None));
}

#[test]
fn test_fetch_continue_request() {
    let r = dispatch("Fetch.continueRequest", Some(json!({"requestId":"req-1"})));
    assert_jsonrpc_invariant(&r, "Fetch.continueRequest");
    let result = r.result.unwrap();
    assert_eq!(result["requestId"], "req-1");
    assert_eq!(result["continued"], true);
}

#[test]
fn test_fetch_continue_request_no_request_id() {
    // Adversarial: missing requestId → empty string.
    let r = dispatch("Fetch.continueRequest", None);
    let result = r.result.unwrap();
    assert_eq!(result["requestId"], "");
    assert_eq!(result["continued"], true);
}

#[test]
fn test_fetch_continue_with_response() {
    let r = dispatch(
        "Fetch.continueWithResponse",
        Some(json!({"requestId":"r2"})),
    );
    assert_jsonrpc_invariant(&r, "Fetch.continueWithResponse");
    let result = r.result.unwrap();
    assert_eq!(result["continued"], true);
    // continueWithResponse shares the continueRequest schema: requestId + continued.
    assert_eq!(result["requestId"], "r2");
}

#[test]
fn test_fetch_fail_request() {
    let r = dispatch(
        "Fetch.failRequest",
        Some(json!({"requestId":"r3","reason":"Aborted"})),
    );
    assert_jsonrpc_invariant(&r, "Fetch.failRequest");
    let result = r.result.unwrap();
    assert_eq!(result["failed"], true);
    assert_eq!(result["reason"], "Aborted");
    assert_eq!(result["requestId"], "r3");
}

#[test]
fn test_fetch_fail_request_default_reason() {
    // Adversarial: missing reason → empty string.
    let r = dispatch("Fetch.failRequest", Some(json!({"requestId":"r3"})));
    let result = r.result.unwrap();
    assert_eq!(result["reason"], "");
    assert_eq!(result["failed"], true);
}

#[test]
fn test_fetch_fulfill_request() {
    let r = dispatch(
        "Fetch.fulfillRequest",
        Some(json!({"requestId":"r4","responseCode":200,"body":"hello"})),
    );
    assert_jsonrpc_invariant(&r, "Fetch.fulfillRequest");
    let result = r.result.unwrap();
    assert_eq!(result["fulfilled"], true);
    assert_eq!(result["responseCode"], 200);
    assert_eq!(result["bodyLength"], 5, "body 'hello' is 5 bytes");
    assert_eq!(result["requestId"], "r4");
}

#[test]
fn test_fetch_fulfill_request_default_response_code() {
    // Adversarial: missing responseCode → defaults to 200.
    let r = dispatch(
        "Fetch.fulfillRequest",
        Some(json!({"requestId":"r4","body":"abc"})),
    );
    let result = r.result.unwrap();
    assert_eq!(result["responseCode"], 200);
    assert_eq!(result["bodyLength"], 3);
}

#[test]
fn test_fetch_fulfill_request_empty_body() {
    // Adversarial: empty body → bodyLength 0.
    let r = dispatch(
        "Fetch.fulfillRequest",
        Some(json!({"requestId":"r4","body":""})),
    );
    assert_eq!(r.result.unwrap()["bodyLength"], 0);
}

#[test]
fn test_fetch_fulfill_request_missing_body() {
    // Adversarial: missing body → body defaults to "" → bodyLength 0.
    let r = dispatch("Fetch.fulfillRequest", Some(json!({"requestId":"r4"})));
    assert_eq!(r.result.unwrap()["bodyLength"], 0);
}

#[test]
fn test_fetch_fulfill_request_unicode_body() {
    // Adversarial: unicode body — bodyLength is byte length (not char count).
    // "你好" = 6 bytes in UTF-8, 2 chars. params_str returns String; .len() is bytes.
    let r = dispatch(
        "Fetch.fulfillRequest",
        Some(json!({"requestId":"r4","body":"你好"})),
    );
    let result = r.result.unwrap();
    // Note: String::len() counts bytes, so "你好" → 6.
    assert_eq!(result["bodyLength"], 6);
}

#[test]
fn test_fetch_get_request_post_data() {
    let r = dispatch("Fetch.getRequestPostData", Some(json!({"requestId":"r5"})));
    assert_jsonrpc_invariant(&r, "Fetch.getRequestPostData");
    let result = r.result.unwrap();
    assert_eq!(result["requestId"], "r5");
    assert_eq!(result["postData"], "", "default postData is empty string");
}

#[test]
fn test_fetch_continue_with_auth() {
    let r = dispatch("Fetch.continueWithAuth", Some(json!({"requestId":"r6"})));
    assert_jsonrpc_invariant(&r, "Fetch.continueWithAuth");
    let result = r.result.unwrap();
    assert_eq!(result["requestId"], "r6");
    // continueWithAuth returns ONLY requestId (no extra fields).
    assert_eq!(result.as_object().unwrap().len(), 1);
}

#[test]
fn test_fetch_take_response_body() {
    let r = dispatch(
        "Fetch.takeResponseBodyAsStream",
        Some(json!({"requestId":"r7"})),
    );
    assert_jsonrpc_invariant(&r, "Fetch.takeResponseBodyAsStream");
    let result = r.result.unwrap();
    assert!(result["stream"].is_string());
    // stream = format!("stream-{}", request_id).
    assert_eq!(result["stream"], "stream-r7");
}

#[test]
fn test_fetch_take_response_body_no_request_id() {
    // Adversarial: missing requestId → stream = "stream-".
    let r = dispatch("Fetch.takeResponseBodyAsStream", None);
    assert_eq!(r.result.unwrap()["stream"], "stream-");
}

#[test]
fn test_fetch_unknown() {
    let r = dispatch("Fetch.nonexistent", None);
    let e = r.error.unwrap();
    assert_eq!(e.code, -32601);
    assert!(e.message.contains("Fetch.nonexistent"));
}

// =====================================================================
// ---- serialize_response / serialize_event helpers ----
// =====================================================================

#[test]
fn test_serialize_ok_response() {
    let resp = CdpResponse {
        id: Some(42),
        result: Some(json!({"ok":true})),
        error: None,
    };
    let s = serialize_response(&resp);
    let parsed: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert_eq!(parsed["id"], 42);
    assert_eq!(parsed["result"]["ok"], true);
    // Adversarial: serialized success MUST NOT carry error key.
    assert!(
        parsed.get("error").is_none(),
        "success response must not carry error"
    );
}

#[test]
fn test_serialize_error_response() {
    let resp = CdpResponse {
        id: Some(1),
        result: None,
        error: Some(bao_cdp::CdpError {
            code: -32601,
            message: "not found".into(),
        }),
    };
    let s = serialize_response(&resp);
    assert!(s.contains("-32601"));
    // Adversarial: round-trip parse and verify structure.
    let parsed: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert_eq!(parsed["error"]["code"], -32601);
    assert_eq!(parsed["error"]["message"], "not found");
    assert!(
        parsed.get("result").is_none(),
        "error response must not carry result"
    );
}

#[test]
fn test_serialize_event() {
    let ev = CdpEvent {
        method: "Page.load".into(),
        params: Some(json!({"ts":1})),
    };
    let s = serialize_event(&ev);
    assert!(s.contains("Page.load"));
    // Adversarial: round-trip parse.
    let parsed: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert_eq!(parsed["method"], "Page.load");
    assert_eq!(parsed["params"]["ts"], 1);
}

#[test]
fn test_serialize_event_no_params() {
    // Adversarial: event with None params.
    let ev = CdpEvent {
        method: "Page.load".into(),
        params: None,
    };
    let s = serialize_event(&ev);
    let parsed: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert_eq!(parsed["method"], "Page.load");
}

#[test]
fn test_serialize_response_id_none() {
    // Adversarial: None id (notification) — must round-trip as JSON null.
    let resp = CdpResponse {
        id: None,
        result: Some(json!({})),
        error: None,
    };
    let s = serialize_response(&resp);
    let parsed: serde_json::Value = serde_json::from_str(&s).unwrap();
    assert!(parsed["id"].is_null());
}

// =====================================================================
// ---- Edge cases ----
// =====================================================================

#[test]
fn test_empty_domain() {
    let r = dispatch("", None);
    assert!(r.error.is_some());
    assert_eq!(r.error.unwrap().code, -32601);
}

#[test]
fn test_domain_only_no_command() {
    let r = dispatch("Page", None);
    assert!(r.error.is_some());
    let e = r.error.as_ref().unwrap();
    assert_eq!(e.code, -32601);
    assert!(e.message.contains("Page"));
}

#[test]
fn test_response_id_matches_input() {
    let msg = CdpMessage {
        id: Some(999),
        method: "Page.enable".into(),
        params: None,
        session_id: None,
    };
    let resp = handle_command(msg, "t", &None, None);
    assert_eq!(resp.id, Some(999));
}

#[test]
fn test_negative_id_preserved() {
    let msg = CdpMessage {
        id: Some(-42),
        method: "Page.enable".into(),
        params: None,
        session_id: None,
    };
    let resp = handle_command(msg, "t", &None, None);
    assert_eq!(resp.id, Some(-42));
}

#[test]
fn test_zero_id_preserved() {
    // Adversarial: id = 0 (falsy but valid JSON-RPC id) must be preserved.
    let msg = CdpMessage {
        id: Some(0),
        method: "Page.enable".into(),
        params: None,
        session_id: None,
    };
    let resp = handle_command(msg, "t", &None, None);
    assert_eq!(resp.id, Some(0));
}

#[test]
fn test_none_id_preserved() {
    // Adversarial: None id (notification) must be preserved as None.
    let msg = CdpMessage {
        id: None,
        method: "Page.enable".into(),
        params: None,
        session_id: None,
    };
    let resp = handle_command(msg, "t", &None, None);
    assert_eq!(resp.id, None);
}

#[test]
fn test_max_id_preserved() {
    // Adversarial: i64::MAX id must be preserved.
    let msg = CdpMessage {
        id: Some(i64::MAX),
        method: "Page.enable".into(),
        params: None,
        session_id: None,
    };
    let resp = handle_command(msg, "t", &None, None);
    assert_eq!(resp.id, Some(i64::MAX));
}

#[test]
fn test_min_id_preserved() {
    // Adversarial: i64::MIN id must be preserved.
    let msg = CdpMessage {
        id: Some(i64::MIN),
        method: "Page.enable".into(),
        params: None,
        session_id: None,
    };
    let resp = handle_command(msg, "t", &None, None);
    assert_eq!(resp.id, Some(i64::MIN));
}

// =====================================================================
// ---- params type adversarial (REQ-CDP-001 robustness) ----
// =====================================================================

#[test]
fn test_params_as_array_accepted() {
    // Adversarial: params as array (unusual but valid JSON) — handlers use .get() which
    // returns None on non-objects, so defaults kick in. Must not panic.
    let r = dispatch("Page.navigate", Some(json!([{"url":"http://x"}])));
    assert!(r.result.is_some(), "array params must not crash navigate");
}

#[test]
fn test_params_as_string_accepted() {
    // Adversarial: params as string — must not panic.
    let r = dispatch("Page.navigate", Some(json!("just a string")));
    assert!(r.result.is_some());
}

#[test]
fn test_params_as_number_accepted() {
    // Adversarial: params as number — must not panic.
    let r = dispatch("Runtime.evaluate", Some(json!(42)));
    assert!(r.result.is_some());
}

#[test]
fn test_params_as_null_accepted() {
    // Adversarial: params as explicit null — treated like None.
    let r = dispatch("Page.enable", Some(serde_json::Value::Null));
    assert!(r.result.is_some());
}

#[test]
fn test_params_as_boolean_accepted() {
    // Adversarial: params as boolean — must not panic.
    let r = dispatch("Page.enable", Some(json!(true)));
    assert!(r.result.is_some());
}

// =====================================================================
// ---- Determinism: same input → same output (REQ-CDP-001 reliability) ----
// =====================================================================

#[test]
fn test_dispatch_is_deterministic() {
    // Adversarial: repeated dispatch with identical input must yield identical output.
    let r1 = dispatch("Target.getTargets", None);
    let r2 = dispatch("Target.getTargets", None);
    assert_eq!(
        serde_json::to_string(&r1.result).unwrap(),
        serde_json::to_string(&r2.result).unwrap(),
        "dispatch must be deterministic"
    );
}

#[test]
fn test_attach_session_id_is_hex_of_char_sum() {
    // Adversarial: verify the documented sessionId derivation.
    // sessionId = format!("{:016x}", target_id.chars().map(|c| c as u64).sum::<u64>()).
    // "test-target" chars sum: verify against actual.
    let target = "test-target";
    let expected_sum: u64 = target.chars().map(|c| c as u64).sum();
    let expected_sid = format!("{:016x}", expected_sum);
    let r = dispatch_target(target, "Target.attachToTarget", None);
    assert_eq!(r.result.unwrap()["sessionId"], expected_sid);
}
