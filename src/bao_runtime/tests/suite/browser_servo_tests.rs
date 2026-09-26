// @trace TEST-BRW-SRV [req:REQ-BRW-001,REQ-BRW-002,REQ-BRW-003,REQ-BRW-004] [level:integration]
//
// REQ-BRW-001~004 — servo browser integration, runtime-side faces.
//
// Layer boundary: the servo WebView stack (libservo, SoftwareRenderingContext,
// DOM Worker threads) lives in `bao_browser`, which is NOT in bao_runtime's
// dependency closure (zero `bao_browser` nodes in
// `cargo tree -p bun_runtime --edges normal,dev`). The rendering/lifecycle
// criteria are tested at their SPEC-designated location:
//   - bao_browser/tests/suite/browser_core_unit_tests.rs       (REQ-BRW-001)
//   - bao_browser/tests/suite/browser_runtime_tests.rs         (REQ-BRW-001)
//   - bao_browser/tests/suite/page_screenshot_deep_tests.rs    (REQ-BRW-002)
//   - bao_browser/tests/suite/ (worker/e2e faces)              (REQ-BRW-004)
//   - bao_cdp_client/tests/suite/e2e_internal_servo.rs         (in-servo e2e)
//
// What IS reachable — and really tested — from this crate:
// - REQ-BRW-001/002: the servo→CDP event seam owned by the linked
//   bao_cdp_client (`translate_servo_event`): real ServoEvent values in,
//   exact CDP 2.0 event names/payloads out. This is the contract every
//   browser render/net/console fact must traverse to become CDP-observable.
// - REQ-BRW-003: the JSContext fusion criterion at the runtime face —
//   Node/Bun host APIs and the browser entry coexist in ONE JSContext
//   (the servo ScriptThread face is bao_browser-side).
// - REQ-BRW-004: the structured-clone primitive Worker messages rely on,
//   roundtripping the exact criterion payload {a:[1,2], b:new ArrayBuffer(8)}.

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
use bao_cdp_client::{translate_servo_event, ConsoleLevel, ServoEvent};
use std::collections::HashMap;

fn eval_bool(ctx: &mut JsContext, source: &str) -> bool {
    matches!(ctx.eval(source, "<brw-servo>"), Ok(JsValue::Bool(true)))
}

fn test_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext::for_test");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

// ---------------------------------------------------------------------------
// REQ-BRW-001 / REQ-BRW-002 — servo event → CDP event seam
// ---------------------------------------------------------------------------

/// Every servo console fact surfaces as Log.entryAdded with level/text kept.
#[test]
fn req_brw_001_servo_console_translates_to_log_entry_added() {
    let out = translate_servo_event(ServoEvent::Console {
        target_id: "t1".into(),
        level: ConsoleLevel::Error,
        text: "boom".into(),
        url: Some("https://example.com/x.js".into()),
        line: Some(10),
        column: Some(5),
    });
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].method, "Log.entryAdded");
    assert_eq!(out[0].params["entry"]["text"], "boom");
    assert_eq!(out[0].params["entry"]["level"], "error");
    assert_eq!(out[0].params["entry"]["lineNumber"], 10);
}

/// Page errors surface as Runtime.exceptionThrown with a stack shape.
#[test]
fn req_brw_002_servo_page_error_translates_to_exception_thrown() {
    let out = translate_servo_event(ServoEvent::PageError {
        target_id: "t1".into(),
        text: "ReferenceError: x is not defined".into(),
        url: Some("https://example.com/".into()),
        line: Some(3),
        column: Some(7),
        stack: Some("frame0".into()),
    });
    assert_eq!(out[0].method, "Runtime.exceptionThrown");
    let details = &out[0].params["exceptionDetails"];
    assert_eq!(
        details["exception"]["value"],
        "ReferenceError: x is not defined"
    );
    assert_eq!(details["text"], "ReferenceError: x is not defined");
    assert_eq!(details["lineNumber"], 3);
}

/// Network lifecycle: request → response → finished translate to the CDP
/// Network event sequence with ids threaded through.
#[test]
fn req_brw_002_servo_network_lifecycle_translates_to_cdp_sequence() {
    let mut headers = HashMap::new();
    headers.insert("content-type".to_string(), "text/html".to_string());

    let req = translate_servo_event(ServoEvent::NetworkRequest {
        target_id: "t1".into(),
        request_id: "r-1".into(),
        url: "https://example.com/".into(),
        method: "GET".into(),
        headers: headers.clone(),
        post_data: None,
        resource_type: "Document".into(),
        frame_id: "f-1".into(),
    });
    assert_eq!(req[0].method, "Network.requestWillBeSent");
    assert_eq!(req[0].params["requestId"], "r-1");
    assert_eq!(req[0].params["request"]["method"], "GET");
    assert_eq!(req[0].params["request"]["url"], "https://example.com/");

    let resp = translate_servo_event(ServoEvent::NetworkResponse {
        target_id: "t1".into(),
        request_id: "r-1".into(),
        url: "https://example.com/".into(),
        status: 200,
        status_text: "OK".into(),
        headers: headers.clone(),
        mime_type: "text/html".into(),
        remote_ip: Some("93.184.216.34".into()),
    });
    assert_eq!(resp[0].method, "Network.responseReceived");
    assert_eq!(resp[0].params["response"]["status"], 200);
    assert_eq!(resp[0].params["response"]["mimeType"], "text/html");

    let fin = translate_servo_event(ServoEvent::NetworkLoadingFinish {
        target_id: "t1".into(),
        request_id: "r-1".into(),
        encoded_data_length: 1024,
    });
    assert_eq!(fin[0].method, "Network.loadingFinished");
    assert_eq!(fin[0].params["requestId"], "r-1");

    let fail = translate_servo_event(ServoEvent::NetworkLoadingFail {
        target_id: "t1".into(),
        request_id: "r-2".into(),
        error_text: "connection refused".into(),
        canceled: false,
    });
    assert_eq!(fail[0].method, "Network.loadingFailed");
    assert_eq!(fail[0].params["errorText"], "connection refused");
}

/// Script/frame lifecycle: Debugger.scriptParsed + the Page frame trio.
#[test]
fn req_brw_001_servo_script_and_frame_events_translate() {
    let parsed = translate_servo_event(ServoEvent::ScriptParsed {
        target_id: "t1".into(),
        script_id: "s-1".into(),
        url: "https://example.com/a.js".into(),
        start_line: 0,
        start_column: 0,
        end_line: 9,
        end_column: 1,
        source_map_url: None,
    });
    assert_eq!(parsed[0].method, "Debugger.scriptParsed");
    assert_eq!(parsed[0].params["scriptId"], "s-1");

    let nav = translate_servo_event(ServoEvent::FrameNavigated {
        target_id: "t1".into(),
        frame_id: "f-1".into(),
        url: "https://example.com/".into(),
        name: None,
    });
    assert_eq!(nav[0].method, "Page.frameNavigated");
    assert_eq!(nav[0].params["frame"]["id"], "f-1");
    assert_eq!(nav[0].params["frame"]["url"], "https://example.com/");

    let start = translate_servo_event(ServoEvent::FrameStartedLoading {
        target_id: "t1".into(),
        frame_id: "f-1".into(),
    });
    assert_eq!(start[0].method, "Page.frameStartedLoading");

    let stop = translate_servo_event(ServoEvent::FrameStoppedLoading {
        target_id: "t1".into(),
        frame_id: "f-1".into(),
    });
    assert_eq!(stop[0].method, "Page.frameStoppedLoading");
}

// ---------------------------------------------------------------------------
// REQ-BRW-003 — JSContext fusion (runtime face)
// ---------------------------------------------------------------------------

/// One JSContext carries the Node/Bun host APIs AND the browser entry at the
/// same time — no conversion layer, exactly the fusion criterion at the
/// runtime side of the seam.
#[test]
fn req_brw_003_node_and_browser_apis_coexist_in_one_context() {
    let mut ctx = test_ctx();
    assert!(eval_bool(
        &mut ctx,
        "typeof require === 'function' && typeof process !== 'undefined'"
    ));
    assert!(eval_bool(&mut ctx, "typeof Bun === 'object' && Bun !== null"));
    assert!(eval_bool(
        &mut ctx,
        "typeof require('path').join === 'function'"
    ));
    // Browser entry reachable from the same context, same globals object.
    assert!(eval_bool(
        &mut ctx,
        "typeof Bao === 'object' && Bao !== null && typeof Bao.browser.connect === 'function'"
    ));
    // `Bao.*` is an alias of `Bun.*` (same object) — documented brand rule.
    assert!(eval_bool(&mut ctx, "Bao === Bun"));
}

// ---------------------------------------------------------------------------
// REQ-BRW-004 — Worker-path primitive: structured clone
// ---------------------------------------------------------------------------

/// Worker messages are structured-cloned. The criterion payload of
/// REQ-BRW-004 ({a:[1,2], b:new ArrayBuffer(8)}) must roundtrip with identity
/// separation (a fresh ArrayBuffer, same bytes) on the shared clone engine.
#[test]
fn req_brw_004_structured_clone_worker_payload_roundtrip() {
    let mut ctx = test_ctx();
    assert!(eval_bool(
        &mut ctx,
        "typeof structuredClone === 'function'"
    ));
    assert!(eval_bool(
        &mut ctx,
        r#"(function(){
            var payload = { a: [1, 2], b: new ArrayBuffer(8) };
            var c = structuredClone(payload);
            return !!c
                && c !== payload
                && c.a.length === 2
                && c.a[0] === 1 && c.a[1] === 2
                && (c.b instanceof ArrayBuffer)
                && c.b.byteLength === 8
                && c.b !== payload.b;
        })()"#
    ));
}
