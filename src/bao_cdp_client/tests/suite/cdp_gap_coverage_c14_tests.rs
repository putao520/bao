//! C14 覆盖率缺口波 — browser/cdp 域残余缺口补测(bao_cdp_client 消费面)。
//!
//! 覆盖三面(lcov 2026-10-18 基线缺口账,`.plans/coverage-governance-20261017.md`):
//!
//! 1. **bao_cdp `protocol.rs` 残余 domain 臂**(gap 410)— Storage / Security /
//!    Profiler / HeapProfiler / Memory / Performance / SystemInfo / Browser /
//!    ServiceWorker 全 domain 经公共入口 `bao_cdp::handle_command`,bridge 臂
//!    用泵线程应答器(伪 responder = 生产 `cdp_handler` 响应形状单一真值,
//!    与 `cdp_universe_wiring_tests` 同惯形),无 bridge 臂直接传 `None`。
//!    参数提取面的断言 oracle = 泵捕获的 `BridgeCommand`(浏览器侧真实收到的
//!    命令形状)——应答器按命令变体形状作答,捕获向量逐字段断言。
//! 2. **`servo_backend.rs` blanket 转发层**(`impl ServoBackend for
//!    Arc<dyn ServoBackend>`,gap ~310 行)— 经 `Arc<dyn>` 接收者形态逐方法
//!    调用,转发正确性 oracle = 同输入下 direct mock 结果的 Debug 串恒等。
//! 3. **`bridge_sender_backend.rs` 通道 trait 实现**(gap ~350 行)—
//!    `bridge_channel` + 泵线程应答,断言每个方法从通道响应 Value 解析出的
//!    结构体字段(解析形状契约)。
//!
//! @trace REQ-CDP-001 [level:integration]
//! @trace REQ-CDP-004 [level:integration]
//! @trace REQ-CDP-006 [level:integration]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use bao_cdp::servo_bridge::{BridgeCommand, BridgeReceiver, BridgeResponse, bridge_channel};
use bao_cdp::{CdpMessage, handle_command};
use bao_cdp_client::bridge::DebugStepAction;
use bao_cdp_client::{BridgeError, BridgeSenderBackend, MockServoBackend, ServoBackend};
use serde_json::{Value, json};

const PUMP_TIMEOUT: Duration = Duration::from_secs(5);
const TARGET: &str = "1";

/// 泵线程应答器:`f` 决定每条 BridgeCommand 的应答 Value(`None` = 通道对端
/// 显式失败,用于错误传播面)。空转 400 轮后退出(nextest 每 test 独立进程,
/// 退出前不再自旋)。
fn spawn_answer_pump(
    rx: BridgeReceiver,
    f: impl Fn(&BridgeCommand) -> Option<Value> + Send + 'static,
) {
    std::thread::spawn(move || {
        let mut idle = 0u32;
        while idle < 400 {
            let handled = rx.recv_and_process(Duration::from_millis(50), |cmd| BridgeResponse {
                result: f(&cmd).ok_or_else(|| {
                    bao_cdp::servo_bridge::BridgeError::from("c14 pump: peer refused")
                }),
            });
            if handled {
                idle = 0;
            } else {
                idle += 1;
            }
        }
    });
}

/// 生产 `cdp_handler` 响应形状的单一真值应答(与 `cdp_universe_wiring_tests`
/// 的 spawn_shape_responder 同惯形;每臂形状 = `bridge_sender_backend` 各方法
/// 解析面读的字段)。
fn canned_bridge_answer(cmd: &BridgeCommand) -> Value {
    use BridgeCommand as C;
    match cmd {
        C::Navigate { .. } => json!({ "frameId": "frame-1", "loaderId": "loader-1" }),
        C::TakeScreenshot { .. } => json!({ "data": "aGVsbG8=" }), // base64("hello")
        // evaluate(rbv) envelope:frame-tree / layout-metrics 合成表达式返回
        // JSON 编码字符串(生产 eval_json 剥壳语义),其余 undefined 形状。
        C::EvaluateJs { expression, .. } => {
            if expression.contains("location.href") {
                json!({ "result": { "type": "string", "value":
                    r#"{"url":"https://x.test/p","mimeType":"text/html","name":"pf","securityOrigin":"https://x.test"}"# } })
            } else if expression.contains("innerWidth") {
                json!({ "result": { "type": "string", "value":
                    r#"{"iw":1280,"ih":720,"cw":1280,"ch":900}"# } })
            } else {
                json!({ "result": { "type": "undefined" } })
            }
        }
        C::RuntimeGetProperties { .. } => json!({ "result": [ {
            "name": "len", "writable": true, "configurable": true, "enumerable": true,
            "isOwn": true,
            "value": { "type": "number", "value": 3 }
        } ] }),
        C::GetDocument { .. } => json!({ "root": {
            "nodeId": 1, "nodeName": "#document", "nodeValue": "", "backendNodeId": 10,
            "children": [ { "nodeId": 2, "nodeName": "HTML", "nodeValue": "", "backendNodeId": 11 } ]
        } }),
        C::QuerySelector { .. } => json!({ "nodeId": 7 }),
        C::QuerySelectorAll { .. } => json!({ "nodeIds": [7, 8, 9] }),
        C::CssGetComputedStyleForNode { .. } => json!({ "computedStyle": [
            { "name": "color", "value": "rgb(1, 2, 3)" },
            { "name": "width", "value": "10px" }
        ] }),
        C::CssGetMatchedStylesForNode { .. } => json!({
            "inlineStyle": { "cssProperties": [ { "name": "color", "value": "red", "important": true } ] },
            "matchedCSSRules": [ { "rule": {
                "selectorList": { "selectors": [ { "text": "div.cls" } ] },
                "style": { "cssProperties": [ { "name": "margin", "value": "0" } ] }
            } } ]
        }),
        C::GetOuterHtml { .. } => json!({ "outerHTML": "<div id=\"a\"></div>" }),
        C::GetResponseBody { .. } => json!({ "body": "Zm9v", "base64Encoded": true }),
        C::DebuggerSetBreakpoint { .. } => json!({
            "breakpointId": "bp-1",
            "locations": [ { "scriptId": "42", "lineNumber": 7, "columnNumber": 3 } ]
        }),
        C::DebuggerGetPossibleBreakpoints { .. } => json!({ "locations": [
            { "scriptId": "42", "lineNumber": 1, "columnNumber": 0 },
            { "scriptId": "42", "lineNumber": 2, "columnNumber": 4 }
        ] }),
        C::DebuggerGetScriptSource { .. } => json!({ "scriptSource": "function f(){return 1}" }),
        C::DebuggerEval { .. } => json!({ "result": { "type": "string", "value": "2" } }),
        C::ListTargets => {
            json!([ { "id": "t-1", "title": "Title-1", "url": "https://u.test/", "browserContextId": "ctx-9" } ])
        }
        C::CreateTarget { .. } => json!({ "targetId": "t-new" }),
        C::GetTargetContextId { .. } => json!({ "browserContextId": "ctx-9" }),
        // Target-info 真查询面(getTargetInfo 经 live_target_info 逐字段
        // 查询,bridge 在场 ⇒ 应答必须携带承诺字段——缺字段=协议违规报错,
        // 不再落到 `_ => {}` 形状)。
        C::GetTitle { .. } => json!("Title-1"),
        C::GetUrl { .. } => json!("https://u.test/"),
        C::GetTargetOpenerAccess { .. } => json!(false),
        C::ListWorkerTargets { .. } => json!({ "workerTargets": [
            { "id": "w-1", "type": "worker", "title": "worker", "url": "https://u.test/w.js" }
        ] }),
        C::PerformanceGetMetrics { .. } => json!({ "metrics": [
            { "name": "Timestamp", "value": 12.5 },
            { "name": "Frames", "value": 3 }
        ] }),
        _ => json!({}),
    }
}

/// 捕获 + 应答通道对:`f` 收 `(sender, seen)`,seen 记录泵收到的每条命令。
fn with_recording_channel(
    f: impl FnOnce(&bao_cdp::servo_bridge::BridgeSender, Arc<Mutex<Vec<BridgeCommand>>>),
) {
    let (tx, rx) = bridge_channel(PUMP_TIMEOUT);
    let seen: Arc<Mutex<Vec<BridgeCommand>>> = Arc::new(Mutex::new(Vec::new()));
    let seen_for_pump = seen.clone();
    spawn_answer_pump(rx, move |cmd| {
        seen_for_pump.lock().unwrap().push(cmd.clone());
        Some(canned_bridge_answer(cmd))
    });
    f(&tx, seen);
}

fn last_cmd(seen: &Arc<Mutex<Vec<BridgeCommand>>>) -> BridgeCommand {
    seen.lock()
        .unwrap()
        .last()
        .expect("pump captured at least one command")
        .clone()
}

/// `handle_command` 便捷入口:`(method, params, bridge)` → 响应。
/// params 恒以 `Some` 传递 — 全部 handler 经 `params.as_ref().and_then(..)`
/// 读取,`Some(空对象)` 与 `None` 在缺省键语义上等价。
fn cdp_call(
    method: &str,
    params: Value,
    bridge: Option<&bao_cdp::servo_bridge::BridgeSender>,
) -> bao_cdp::CdpResponse {
    handle_command(
        CdpMessage {
            id: Some(1),
            method: method.to_string(),
            params: Some(params.clone()),
            session_id: None,
        },
        TARGET,
        &Some(params),
        bridge,
    )
}

// ═════════════════════════════════════════════════════════════════════════
// 第一面:bao_cdp protocol.rs 残余 domain 臂(handle_command 公共入口)
// ═════════════════════════════════════════════════════════════════════════

#[test]
fn page_capture_screenshot_extracts_format_and_quality() {
    // @trace REQ-CDP-004 [level:integration]
    with_recording_channel(|tx, seen| {
        let r = cdp_call(
            "Page.captureScreenshot",
            json!({ "format": "jpeg", "quality": 80 }),
            Some(tx),
        );
        assert!(
            r.error.is_none(),
            "screenshot must round-trip: {:?}",
            r.error
        );
        match last_cmd(&seen) {
            BridgeCommand::TakeScreenshot {
                format, quality, ..
            } => {
                assert_eq!(format, "jpeg");
                assert_eq!(quality, Some(80));
            }
            other => panic!("expected TakeScreenshot, got {other:?}"),
        }
    });
}

#[test]
fn page_set_content_synthesizes_document_write_and_rejects_empty() {
    // @trace REQ-CDP-004 [level:integration]
    with_recording_channel(|tx, seen| {
        let r = cdp_call("Page.setContent", json!({ "html": "<p>hi</p>" }), Some(tx));
        assert!(r.error.is_none(), "{:?}", r.error);
        match last_cmd(&seen) {
            BridgeCommand::EvaluateJs {
                expression,
                return_by_value,
                ..
            } => {
                assert!(expression.contains("document.open()"));
                assert!(expression.contains("document.write"));
                assert!(expression.contains("<p>hi</p>"));
                assert!(return_by_value);
            }
            other => panic!("expected EvaluateJs, got {other:?}"),
        }

        // 空 html = -32602(显式拒绝,不静默写空文档)。
        let r = cdp_call("Page.setContent", json!({ "html": "" }), Some(tx));
        let err = r.error.expect("empty html must be rejected");
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("non-empty"), "{}", err.message);
    });
}

#[test]
fn page_get_layout_metrics_reads_live_document_shape() {
    // @trace REQ-CDP-004 [level:integration]
    with_recording_channel(|tx, _seen| {
        let r = cdp_call("Page.getLayoutMetrics", json!({}), Some(tx));
        assert!(r.error.is_none(), "{:?}", r.error);
        let res = r.result.unwrap();
        assert_eq!(res["layoutViewport"]["width"], 1280);
        assert_eq!(res["layoutViewport"]["height"], 720);
        assert_eq!(res["contentSize"]["width"], 1280);
        assert_eq!(res["contentSize"]["height"], 900);
        // css* 面与物理面同值(bao 无 device-pixel 缩放通道)。
        assert_eq!(res["cssLayoutViewport"]["width"], 1280);
        assert_eq!(res["cssContentSize"]["height"], 900);
    });
}

#[test]
fn page_frame_tree_parses_json_object_and_rejects_non_object() {
    // @trace REQ-CDP-004 [level:integration]
    with_recording_channel(|tx, _seen| {
        // JSON 对象 → frameTree.frame 形状(url/name/securityOrigin 逐字段)。
        let r = cdp_call("Page.getFrameTree", json!({}), Some(tx));
        assert!(r.error.is_none(), "{:?}", r.error);
        let frame = &r.result.unwrap()["frameTree"]["frame"];
        assert_eq!(frame["url"], "https://x.test/p");
        assert_eq!(frame["name"], "pf");
        assert_eq!(frame["securityOrigin"], "https://x.test");
        assert_eq!(frame["mimeType"], "text/html");
    });
    // 非 JSON 对象文档 → page query failed(-32000 形态;不伪造 frame)。
    let (tx, rx) = bridge_channel(PUMP_TIMEOUT);
    spawn_answer_pump(rx, |_| {
        Some(json!({ "result": { "type": "string", "value": "42" } }))
    });
    let r = cdp_call("Page.getFrameTree", json!({}), Some(&tx));
    let err = r.error.expect("non-object document must fail");
    assert_eq!(err.code, -32000);
    assert!(err.message.contains("page query failed"), "{}", err.message);
}

#[test]
fn target_get_targets_appends_worker_targets() {
    // @trace REQ-CDP-001 [level:integration]
    // REQ-BRW-004:worker 子目标并入 getTargets。
    with_recording_channel(|tx, _seen| {
        let r = cdp_call("Target.getTargets", json!({}), Some(tx));
        assert!(r.error.is_none(), "{:?}", r.error);
        let infos = r.result.unwrap()["targetInfos"]
            .as_array()
            .cloned()
            .unwrap();
        // 页面条目走 live_target_info(targetId 键);worker 条目是通道原始
        // 对象 verbatim append(id 键)。
        let page_ids: Vec<&str> = infos
            .iter()
            .filter_map(|t| t["targetId"].as_str())
            .collect();
        assert!(
            page_ids.contains(&"t-1"),
            "page target listed: {page_ids:?}"
        );
        let worker_ids: Vec<&str> = infos.iter().filter_map(|t| t["id"].as_str()).collect();
        assert!(
            worker_ids.contains(&"w-1"),
            "worker target appended: {worker_ids:?}"
        );
        let w = infos.iter().find(|t| t["id"] == "w-1").unwrap();
        assert_eq!(w["type"], "worker");
    });
}

#[test]
fn target_context_id_faces_bridge_empty_and_none() {
    // @trace REQ-CDS-009 [level:integration]
    // (a) bridge 应答非空 → 逐字返回;
    with_recording_channel(|tx, _seen| {
        let r = cdp_call(
            "Target.getTargetInfo",
            json!({ "targetId": TARGET }),
            Some(tx),
        );
        assert!(r.error.is_none(), "{:?}", r.error);
        let info = r.result.unwrap()["targetInfo"].clone();
        assert_eq!(info["browserContextId"], "ctx-9");
        assert_eq!(info["type"], "page");
        assert_eq!(info["attached"], true);
    });
    // (b) bridge 应答空串 → 协议违规报错(兜底根治:空串归属不再被洗成
    //     隐式默认——registry 写入侧 page_context_id() 恒产非空,空串回=
    //     违约,读取点 surface 不掩盖)。
    {
        let (tx, rx) = bridge_channel(PUMP_TIMEOUT);
        spawn_answer_pump(rx, |cmd| match cmd {
            BridgeCommand::GetTargetContextId { .. } => Some(json!({ "browserContextId": "" })),
            _ => Some(canned_bridge_answer(cmd)),
        });
        let r = cdp_call(
            "Target.getTargetInfo",
            json!({ "targetId": TARGET }),
            Some(&tx),
        );
        let err = r
            .error
            .expect("empty-context answer is a registry-protocol violation");
        assert_eq!(err.code, -32603);
        assert!(
            err.message.contains("Target context query failed"),
            "{err:?}"
        );
    }
    // (c) 无 bridge → 隐式默认(list miss / bridge-less 同 fallback)。
    let r = cdp_call("Target.getTargetInfo", json!({ "targetId": TARGET }), None);
    let info = r.result.unwrap()["targetInfo"].clone();
    assert_eq!(info["browserContextId"], "bao-default-context");
}

#[test]
fn network_cookie_and_cache_faces_forward_with_bridge() {
    // @trace REQ-CDP-006 [level:integration]
    with_recording_channel(|tx, seen| {
        let r = cdp_call(
            "Network.deleteCookies",
            json!({ "name": "sid", "url": "https://x.test/" }),
            Some(tx),
        );
        assert!(r.error.is_none(), "{:?}", r.error);
        match last_cmd(&seen) {
            BridgeCommand::DeleteCookie { name, url, .. } => {
                assert_eq!(name, "sid");
                assert_eq!(url.as_deref(), Some("https://x.test/"));
            }
            other => panic!("expected DeleteCookie, got {other:?}"),
        }

        let r = cdp_call(
            "Network.setCacheDisabled",
            json!({ "cacheDisabled": true }),
            Some(tx),
        );
        assert!(r.error.is_none(), "{:?}", r.error);
        match last_cmd(&seen) {
            BridgeCommand::NetworkSetCacheDisabled { cache_disabled, .. } => {
                assert!(cache_disabled)
            }
            other => panic!("expected NetworkSetCacheDisabled, got {other:?}"),
        }

        let r = cdp_call(
            "Network.setExtraHTTPHeaders",
            json!({ "headers": { "x-bao": "1" } }),
            Some(tx),
        );
        assert!(r.error.is_none(), "{:?}", r.error);
        match last_cmd(&seen) {
            BridgeCommand::NetworkSetExtraHTTPHeaders { headers, .. } => {
                assert_eq!(headers["x-bao"], "1");
            }
            other => panic!("expected NetworkSetExtraHTTPHeaders, got {other:?}"),
        }

        let r = cdp_call("Network.clearBrowserCache", json!({}), Some(tx));
        assert!(r.error.is_none(), "{:?}", r.error);
        assert!(matches!(
            last_cmd(&seen),
            BridgeCommand::NetworkClearBrowserCache { .. }
        ));

        let r = cdp_call("Network.clearBrowserCookies", json!({}), Some(tx));
        assert!(r.error.is_none(), "{:?}", r.error);
        assert!(matches!(
            last_cmd(&seen),
            BridgeCommand::NetworkClearBrowserCookies { .. }
        ));

        // 带 url/domain 的 setCookie 提取。
        let r = cdp_call(
            "Network.setCookie",
            json!({ "name": "k", "value": "v", "url": "https://x.test/", "domain": "x.test" }),
            Some(tx),
        );
        assert!(r.error.is_none(), "{:?}", r.error);
        match last_cmd(&seen) {
            BridgeCommand::SetCookie {
                name,
                value,
                url,
                domain,
                ..
            } => {
                assert_eq!(name, "k");
                assert_eq!(value, "v");
                assert_eq!(url.as_deref(), Some("https://x.test/"));
                assert_eq!(domain.as_deref(), Some("x.test"));
            }
            other => panic!("expected SetCookie, got {other:?}"),
        }
    });
}

#[test]
fn network_no_bridge_fallbacks_return_empty_cookie_sets() {
    // @trace REQ-CDP-006 [level:integration]
    // 无 bridge 的 getCookies/getAllCookies = 空 cookies 集(诚实空,不报错)。
    let r = cdp_call(
        "Network.getCookies",
        json!({ "urls": ["https://x.test/"] }),
        None,
    );
    assert_eq!(r.result.unwrap()["cookies"], json!([]));
    let r = cdp_call("Network.getAllCookies", json!({}), None);
    assert_eq!(r.result.unwrap()["cookies"], json!([]));
}

#[test]
fn fetch_resolution_param_validation_is_strict() {
    // @trace REQ-CDP-006 [level:integration]
    with_recording_channel(|tx, seen| {
        // interceptionId 缺失 / 非串 → -32602(解析键 = requestPaused 携带的
        // interceptionId,禁止占位)。
        for method in [
            "Fetch.failRequest",
            "Fetch.fulfillRequest",
            "Fetch.continueRequest",
        ] {
            let r = cdp_call(method, json!({}), Some(tx));
            let err = r
                .error
                .unwrap_or_else(|| panic!("{method} must reject missing interceptionId"));
            assert_eq!(err.code, -32602, "{method}");
            assert!(err.message.contains("interceptionId"), "{}", err.message);

            let r = cdp_call(method, json!({ "interceptionId": 42 }), Some(tx));
            let err = r
                .error
                .unwrap_or_else(|| panic!("{method} must reject non-string interceptionId"));
            assert_eq!(err.code, -32602, "{method}");
        }

        // failRequest:errorReason 非串 → -32602;串 → Fail 转发。
        let r = cdp_call(
            "Fetch.failRequest",
            json!({ "interceptionId": "i-1", "errorReason": 42 }),
            Some(tx),
        );
        assert_eq!(r.error.unwrap().code, -32602);

        let r = cdp_call(
            "Fetch.failRequest",
            json!({ "interceptionId": "i-1", "errorReason": "Aborted" }),
            Some(tx),
        );
        assert!(r.error.is_none(), "{:?}", r.error);
        match last_cmd(&seen) {
            BridgeCommand::FetchResolve {
                interception_id,
                action,
                ..
            } => {
                assert_eq!(interception_id, "i-1");
                match action {
                    bao_cdp::servo_bridge::FetchResolveAction::Fail { error_reason } => {
                        assert_eq!(error_reason.as_deref(), Some("Aborted"));
                    }
                    other => panic!("expected Fail, got {other:?}"),
                }
            }
            other => panic!("expected FetchResolve, got {other:?}"),
        }

        // fulfillRequest:responseCode 非整型 → -32602;合法整型 → 真转发。
        let r = cdp_call(
            "Fetch.fulfillRequest",
            json!({ "interceptionId": "i-1", "responseCode": "200" }),
            Some(tx),
        );
        assert_eq!(r.error.unwrap().code, -32602);

        let r = cdp_call(
            "Fetch.fulfillRequest",
            json!({ "interceptionId": "i-1", "responseCode": 200, "body": "aGk=" }),
            Some(tx),
        );
        assert!(r.error.is_none(), "{:?}", r.error);
        match last_cmd(&seen) {
            BridgeCommand::FetchResolve {
                interception_id,
                action,
                ..
            } => {
                assert_eq!(interception_id, "i-1");
                match action {
                    bao_cdp::servo_bridge::FetchResolveAction::Fulfill {
                        response_code,
                        body,
                        ..
                    } => {
                        assert_eq!(response_code, 200);
                        assert_eq!(body.as_deref(), Some("aGk="));
                    }
                    other => panic!("expected Fulfill, got {other:?}"),
                }
            }
            other => panic!("expected FetchResolve, got {other:?}"),
        }

        // Network.continueInterceptedRequest:errorReason 非串 → -32602。
        let r = cdp_call(
            "Network.continueInterceptedRequest",
            json!({ "interceptionId": "i-2", "errorReason": [1] }),
            Some(tx),
        );
        let err = r.error.expect("non-string errorReason must be rejected");
        assert_eq!(err.code, -32602);
        assert!(err.message.contains("ErrorReason"), "{}", err.message);
    });
}

#[test]
fn debugger_pause_resume_steps_forward_step_types() {
    // @trace REQ-CDP-003 [level:integration]
    with_recording_channel(|tx, seen| {
        cdp_call("Debugger.pause", json!({}), Some(tx));
        assert!(matches!(
            last_cmd(&seen),
            BridgeCommand::DebuggerInterrupt { .. }
        ));

        cdp_call("Debugger.resume", json!({}), Some(tx));
        match last_cmd(&seen) {
            BridgeCommand::DebuggerResume { step_type, .. } => assert_eq!(step_type, None),
            other => panic!("expected DebuggerResume, got {other:?}"),
        }
        // stepOver/stepInto/stepOut → servo Resume limit 字面 next/step/finish。
        for (method, want) in [
            ("Debugger.stepOver", "next"),
            ("Debugger.stepInto", "step"),
            ("Debugger.stepOut", "finish"),
        ] {
            cdp_call(method, json!({}), Some(tx));
            match last_cmd(&seen) {
                BridgeCommand::DebuggerResume { step_type, .. } => {
                    assert_eq!(step_type.as_deref(), Some(want), "{method}");
                }
                other => panic!("expected DebuggerResume, got {other:?}"),
            }
        }

        // evaluateOnCallFrame:bridge + expression → DebuggerEval 转发。
        let r = cdp_call(
            "Debugger.evaluateOnCallFrame",
            json!({ "callFrameId": "frame-0", "expression": "1+1" }),
            Some(tx),
        );
        assert!(r.error.is_none(), "{:?}", r.error);
        match last_cmd(&seen) {
            BridgeCommand::DebuggerEval {
                expression,
                frame_actor_id,
                ..
            } => {
                assert_eq!(expression, "1+1");
                assert_eq!(frame_actor_id.as_deref(), Some("frame-0"));
            }
            other => panic!("expected DebuggerEval, got {other:?}"),
        }
    });
}

#[test]
fn emulation_override_faces_synthesize_js_and_ack() {
    // @trace REQ-CDP-004 [level:integration]
    with_recording_channel(|tx, seen| {
        // setUserAgentOverride:UA 走 SetUserAgent 通道,language/platform 走
        // navigator defineProperty JS。
        let r = cdp_call(
            "Network.setUserAgentOverride",
            json!({ "userAgent": "BaoUA/1", "acceptLanguage": "zh-CN", "platform": "Linux" }),
            Some(tx),
        );
        assert!(r.error.is_none(), "{:?}", r.error);
        let seen_all = seen.lock().unwrap().clone();
        assert!(
            seen_all.iter().any(|c| matches!(c,
                BridgeCommand::SetUserAgent { user_agent, .. } if user_agent == "BaoUA/1")),
            "UA override must reach the wire: {seen_all:?}"
        );
        assert!(
            seen_all.iter().any(|c| matches!(c,
                BridgeCommand::EvaluateJs { expression, .. }
                    if expression.contains("'language'") && expression.contains("'platform'"))),
            "language/platform must synthesize navigator JS: {seen_all:?}"
        );

        // overrideNetworkState(Playwright setOffline):offline=true →
        // navigator.onLine 读 false。
        cdp_call(
            "Network.overrideNetworkState",
            json!({ "offline": true }),
            Some(tx),
        );
        match last_cmd(&seen) {
            BridgeCommand::EvaluateJs { expression, .. } => {
                assert!(expression.contains("return false"), "{expression}");
            }
            other => panic!("expected EvaluateJs, got {other:?}"),
        }

        // Emulation ack 面(setTouchEmulationEnabled 族)恒 ok_empty。
        for method in [
            "Emulation.setTouchEmulationEnabled",
            "Emulation.setScriptExecutionDisabled",
            "Emulation.setFocusEmulationEnabled",
            "Emulation.setCPUThrottlingRate",
            "Emulation.setDefaultBackgroundColorOverride",
        ] {
            let r = cdp_call(method, json!({}), Some(tx));
            assert!(r.error.is_none(), "{method}: {:?}", r.error);
        }
    });
}

#[test]
fn storage_faces_forward_origin_type_and_urls() {
    // @trace REQ-CDP-004 [level:integration]
    with_recording_channel(|tx, seen| {
        // getStorageItemsForOrigin:origin/storageType 提取后转发。
        let r = cdp_call(
            "Storage.getStorageItemsForOrigin",
            json!({ "origin": "https://x.test", "storageType": "local_storage" }),
            Some(tx),
        );
        assert!(r.error.is_none(), "{:?}", r.error);
        match last_cmd(&seen) {
            BridgeCommand::StorageGetStorageItemsForOrigin {
                origin,
                storage_type,
                ..
            } => {
                assert_eq!(origin, "https://x.test");
                assert_eq!(storage_type, "local_storage");
            }
            other => panic!("expected StorageGetStorageItemsForOrigin, got {other:?}"),
        }

        // clearDataForOrigin 同形。
        let r = cdp_call(
            "Storage.clearDataForOrigin",
            json!({ "origin": "https://y.test", "storageType": "indexeddb" }),
            Some(tx),
        );
        assert!(r.error.is_none(), "{:?}", r.error);
        match last_cmd(&seen) {
            BridgeCommand::StorageClearDataForOrigin {
                origin,
                storage_type,
                ..
            } => {
                assert_eq!(origin, "https://y.test");
                assert_eq!(storage_type, "indexeddb");
            }
            other => panic!("expected StorageClearDataForOrigin, got {other:?}"),
        }

        // getCookies = Network.getCookies 别名(urls 数组过滤非串元素)。
        let r = cdp_call(
            "Storage.getCookies",
            json!({ "urls": ["https://x.test/", 7, "https://y.test/"] }),
            Some(tx),
        );
        assert!(r.error.is_none(), "{:?}", r.error);
        match last_cmd(&seen) {
            BridgeCommand::GetCookies { urls, .. } => {
                assert_eq!(
                    urls,
                    vec!["https://x.test/".to_string(), "https://y.test/".to_string()]
                );
            }
            other => panic!("expected GetCookies, got {other:?}"),
        }

        // 未知 Storage method → -32601。
        let r = cdp_call("Storage.resetQuota", json!({}), Some(tx));
        assert_eq!(r.error.unwrap().code, -32601);
    });
}

#[test]
fn security_faces_ack_and_certificate_error_noop() {
    // @trace REQ-CDP-006 [level:integration]
    with_recording_channel(|tx, seen| {
        cdp_call("Security.enable", json!({}), Some(tx));
        assert!(matches!(
            last_cmd(&seen),
            BridgeCommand::SecurityEnable { .. }
        ));
        cdp_call("Security.disable", json!({}), Some(tx));
        assert!(matches!(
            last_cmd(&seen),
            BridgeCommand::SecurityDisable { .. }
        ));

        let r = cdp_call(
            "Security.setOverrideCertificateErrors",
            json!({ "override": true }),
            Some(tx),
        );
        assert!(r.error.is_none(), "{:?}", r.error);
        match last_cmd(&seen) {
            BridgeCommand::SecuritySetOverrideCertificateErrors {
                override_errors, ..
            } => {
                assert!(override_errors);
            }
            other => panic!("expected SecuritySetOverrideCertificateErrors, got {other:?}"),
        }

        // handleCertificateError = 显式 no-op ack(不转发 — 捕获向量停在上一条)。
        let before = seen.lock().unwrap().len();
        let r = cdp_call(
            "Security.handleCertificateError",
            json!({ "eventId": 1, "action": "continue" }),
            Some(tx),
        );
        assert!(r.error.is_none(), "{:?}", r.error);
        assert_eq!(
            seen.lock().unwrap().len(),
            before,
            "handleCertificateError must not forward"
        );
        // 别名字面 "certificateError" 同判。
        let r = cdp_call("Security.certificateError", json!({}), Some(tx));
        assert!(r.error.is_none(), "{:?}", r.error);
        assert_eq!(
            seen.lock().unwrap().len(),
            before,
            "certificateError alias must not forward"
        );

        // 未知 Security method → -32601。
        let r = cdp_call("Security.resetCertificates", json!({}), Some(tx));
        assert_eq!(r.error.unwrap().code, -32601);
    });
}

#[test]
fn profiler_faces_forward_and_reject_unknown() {
    // @trace REQ-CDP-006 [level:integration]
    with_recording_channel(|tx, seen| {
        let r = cdp_call("Profiler.enable", json!({}), Some(tx));
        assert!(r.error.is_none(), "{:?}", r.error);
        let r = cdp_call("Profiler.disable", json!({}), Some(tx));
        assert!(r.error.is_none(), "{:?}", r.error);

        cdp_call("Profiler.start", json!({}), Some(tx));
        assert!(matches!(
            last_cmd(&seen),
            BridgeCommand::ProfilerStart { .. }
        ));
        cdp_call("Profiler.stop", json!({}), Some(tx));
        assert!(matches!(
            last_cmd(&seen),
            BridgeCommand::ProfilerStop { .. }
        ));

        // setSamplingInterval 提取 interval(缺省 1000)。
        cdp_call(
            "Profiler.setSamplingInterval",
            json!({ "interval": 250 }),
            Some(tx),
        );
        match last_cmd(&seen) {
            BridgeCommand::ProfilerSetSamplingInterval { interval, .. } => {
                assert_eq!(interval, 250)
            }
            other => panic!("expected ProfilerSetSamplingInterval, got {other:?}"),
        }
        let r = cdp_call("Profiler.getProfile", json!({}), Some(tx));
        assert_eq!(r.error.unwrap().code, -32601);
    });
}

#[test]
fn heap_profiler_faces_forward() {
    // @trace REQ-CDP-006 [level:integration]
    with_recording_channel(|tx, seen| {
        for (method, want) in [
            ("HeapProfiler.takeHeapSnapshot", "snapshot"),
            ("HeapProfiler.startTrackingHeapObjects", "start"),
            ("HeapProfiler.stopTrackingHeapObjects", "stop"),
            ("HeapProfiler.collectGarbage", "gc"),
        ] {
            let r = cdp_call(method, json!({}), Some(tx));
            assert!(r.error.is_none(), "{method}: {:?}", r.error);
            let matched = match last_cmd(&seen) {
                BridgeCommand::HeapProfilerTakeSnapshot { .. } => want == "snapshot",
                BridgeCommand::HeapProfilerStartTracking { .. } => want == "start",
                BridgeCommand::HeapProfilerStopTracking { .. } => want == "stop",
                BridgeCommand::HeapProfilerCollectGarbage { .. } => want == "gc",
                other => panic!("{method}: unexpected {other:?}"),
            };
            assert!(matched, "{method} forwarded wrong variant");
        }
        let r = cdp_call("HeapProfiler.getObjectByHeapObjectId", json!({}), Some(tx));
        assert_eq!(r.error.unwrap().code, -32601);
    });
}

#[test]
fn memory_faces_forward_and_propagate_peer_error() {
    // @trace REQ-CDP-006 [level:integration]
    // getDOMCounters:通道对端拒绝(mozjs FFI 无 DOM 计数器)→ 协议错误穿透,
    // 不吞成零计数对象。专用泵:MemoryGetDOMCounters → None,其余 → {}。
    let (tx, rx) = bridge_channel(PUMP_TIMEOUT);
    spawn_answer_pump(rx, |cmd| match cmd {
        BridgeCommand::MemoryGetDOMCounters { .. } => None,
        _ => Some(json!({})),
    });
    let seen: Arc<Mutex<Vec<BridgeCommand>>> = Arc::new(Mutex::new(Vec::new()));

    let r = cdp_call("Memory.getDOMCounters", json!({}), Some(&tx));
    assert!(
        r.error.is_some(),
        "peer error must propagate, not be swallowed"
    );

    let r = cdp_call("Memory.prepareForLeakDetection", json!({}), Some(&tx));
    assert!(r.error.is_none(), "{:?}", r.error);

    let r = cdp_call("Memory.forciblyPurgeJavaScriptMemory", json!({}), Some(&tx));
    assert!(r.error.is_none(), "{:?}", r.error);

    let r = cdp_call("Memory.getForAllProcesses", json!({}), Some(&tx));
    assert_eq!(r.error.unwrap().code, -32601);
    let _ = seen;
}

#[test]
fn performance_get_metrics_forwards() {
    // @trace REQ-CDP-006 [level:integration]
    with_recording_channel(|tx, seen| {
        let r = cdp_call("Performance.enable", json!({}), Some(tx));
        assert!(r.error.is_none(), "{:?}", r.error);
        let r = cdp_call("Performance.getMetrics", json!({}), Some(tx));
        assert!(r.error.is_none(), "{:?}", r.error);
        let metrics = r.result.unwrap()["metrics"].as_array().unwrap().clone();
        assert!(metrics.iter().any(|m| m["name"] == "Timestamp"));
        assert!(matches!(
            last_cmd(&seen),
            BridgeCommand::PerformanceGetMetrics { .. }
        ));
        let r = cdp_call("Performance.bogus", json!({}), Some(tx));
        assert_eq!(r.error.unwrap().code, -32601);
    });
}

#[test]
fn system_info_reports_process_truth() {
    // @trace REQ-CDP-006 [level:integration]
    // oracle = std::env::consts / process::id(getInfo 契约即进程事实回显)。
    let r = cdp_call("SystemInfo.getInfo", json!({}), None);
    let info = r.result.unwrap();
    assert_eq!(info["osName"], std::env::consts::OS);
    assert_eq!(info["platform"], std::env::consts::OS);
    assert_eq!(info["cpu"]["arch"], std::env::consts::ARCH);
    assert_eq!(info["pid"], std::process::id());
    assert_eq!(info["product"], "Bao");
    let cpus = info["cpu"]["processors"].as_u64().unwrap();
    assert!(cpus >= 1, "processors must be >= 1");

    let r = cdp_call("SystemInfo.getProcessInfo", json!({}), None);
    let procs = r.result.unwrap()["processInfo"].as_array().unwrap().clone();
    assert_eq!(procs.len(), 1);
    assert_eq!(procs[0]["type"], "browser");
    assert_eq!(procs[0]["id"], std::process::id());

    let r = cdp_call("SystemInfo.bogus", json!({}), None);
    assert_eq!(r.error.unwrap().code, -32601);
}

#[test]
fn browser_download_behavior_ack_and_unknown() {
    // @trace REQ-CDP-001 [level:integration]
    // setDownloadBehavior = 偏好 ack(bao 无下载管理器 — 存偏好,不伪造下载)。
    let r = cdp_call(
        "Browser.setDownloadBehavior",
        json!({ "behavior": "default" }),
        None,
    );
    assert!(r.error.is_none(), "{:?}", r.error);
    let r = cdp_call("Browser.bogus", json!({}), None);
    assert_eq!(r.error.unwrap().code, -32601);
}

#[test]
fn service_worker_observability_faces() {
    // @trace REQ-BRW-004 [criterion:19]
    with_recording_channel(|tx, seen| {
        // deliverPushMessage:echo origin/registrationId + delivered(形状契约
        // 即回显;delivered=true 是 ack 面)。
        let r = cdp_call(
            "ServiceWorker.deliverPushMessage",
            json!({ "origin": "https://x.test", "registrationId": "reg-1" }),
            Some(tx),
        );
        assert!(r.error.is_none(), "{:?}", r.error);
        let res = r.result.unwrap();
        assert_eq!(res["origin"], "https://x.test");
        assert_eq!(res["registrationId"], "reg-1");
        assert_eq!(res["delivered"], true);

        for method in [
            "ServiceWorker.dispatchSyncEvent",
            "ServiceWorker.dispatchPeriodicSyncEvent",
            "ServiceWorker.updateRegistration",
        ] {
            let r = cdp_call(method, json!({}), Some(tx));
            assert!(r.error.is_none(), "{method}: {:?}", r.error);
        }

        // getAllRegistrations:bridge → ListServiceWorkerRegistrations 转发。
        let r = cdp_call("ServiceWorker.getAllRegistrations", json!({}), Some(tx));
        assert!(r.error.is_none(), "{:?}", r.error);
        assert!(matches!(
            last_cmd(&seen),
            BridgeCommand::ListServiceWorkerRegistrations { .. }
        ));
        // 无 bridge → 空 registrations(诚实空)。
        let r = cdp_call("ServiceWorker.getAllRegistrations", json!({}), None);
        assert_eq!(r.result.unwrap()["registrations"], json!([]));

        // getRegistration:bridge + id → 转发;无 bridge → null registration。
        let r = cdp_call(
            "ServiceWorker.getRegistration",
            json!({ "registrationId": "reg-1" }),
            Some(tx),
        );
        assert!(r.error.is_none(), "{:?}", r.error);
        match last_cmd(&seen) {
            BridgeCommand::GetServiceWorkerRegistrationInfo {
                registration_id, ..
            } => {
                assert_eq!(registration_id, "reg-1");
            }
            other => panic!("expected GetServiceWorkerRegistrationInfo, got {other:?}"),
        }
        let r = cdp_call("ServiceWorker.getRegistration", json!({}), None);
        assert_eq!(r.result.unwrap()["registration"], json!(null));

        // stopWorker / unregister:bridge + id → 转发 Stop/Terminate。
        cdp_call(
            "ServiceWorker.stopWorker",
            json!({ "registrationId": "reg-2" }),
            Some(tx),
        );
        match last_cmd(&seen) {
            BridgeCommand::StopServiceWorker {
                registration_id, ..
            } => {
                assert_eq!(registration_id, "reg-2");
            }
            other => panic!("expected StopServiceWorker, got {other:?}"),
        }
        cdp_call(
            "ServiceWorker.unregister",
            json!({ "registrationId": "reg-3" }),
            Some(tx),
        );
        match last_cmd(&seen) {
            BridgeCommand::TerminateServiceWorker {
                registration_id, ..
            } => {
                assert_eq!(registration_id, "reg-3");
            }
            other => panic!("expected TerminateServiceWorker, got {other:?}"),
        }

        let r = cdp_call("ServiceWorker.bogus", json!({}), Some(tx));
        assert_eq!(r.error.unwrap().code, -32601);
    });
}

// ═════════════════════════════════════════════════════════════════════════
// 第二面:servo_backend.rs blanket 转发层(Arc<dyn ServoBackend> 接收者形态)
// ═════════════════════════════════════════════════════════════════════════

/// 转发恒等断言:blanket(`Arc<dyn>` 接收者)结果 ≡ 同输入 direct mock 结果。
/// mock 确定性(call_log 只增不读),双实例同输入必同输出。
fn assert_forwarding_eq<T: std::fmt::Debug>(blanket: &T, direct: &T, face: &str) {
    assert_eq!(
        format!("{blanket:?}"),
        format!("{direct:?}"),
        "blanket forwarding diverged from direct mock on {face}"
    );
}

#[test]
fn blanket_impl_forwards_page_and_runtime_faces() {
    // @trace REQ-CDP-001 [level:integration]
    let blanket: Arc<dyn ServoBackend> = Arc::new(MockServoBackend::new());
    let direct = MockServoBackend::new();

    assert_forwarding_eq(
        &blanket.page_navigate(TARGET, "https://x.test/"),
        &direct.page_navigate(TARGET, "https://x.test/"),
        "page_navigate",
    );
    assert_forwarding_eq(
        &blanket.page_reload(TARGET),
        &direct.page_reload(TARGET),
        "page_reload",
    );
    assert_forwarding_eq(
        &blanket.page_screenshot(TARGET, bao_cdp_client::BridgeScreenshotFormat::Png),
        &direct.page_screenshot(TARGET, bao_cdp_client::BridgeScreenshotFormat::Png),
        "page_screenshot",
    );
    assert_forwarding_eq(
        &blanket.page_frame_tree(TARGET),
        &direct.page_frame_tree(TARGET),
        "page_frame_tree",
    );
    assert_forwarding_eq(
        &blanket.page_navigation_history(TARGET),
        &direct.page_navigation_history(TARGET),
        "page_navigation_history",
    );
    assert_forwarding_eq(
        &blanket.page_navigate_to_history_entry(TARGET, 1),
        &direct.page_navigate_to_history_entry(TARGET, 1),
        "page_navigate_to_history_entry",
    );
    assert_forwarding_eq(
        &blanket.page_set_content(TARGET, "<p>x</p>"),
        &direct.page_set_content(TARGET, "<p>x</p>"),
        "page_set_content",
    );
    assert_forwarding_eq(
        &blanket.page_close(TARGET),
        &direct.page_close(TARGET),
        "page_close",
    );
    assert_forwarding_eq(
        &blanket.page_bring_to_front(TARGET),
        &direct.page_bring_to_front(TARGET),
        "page_bring_to_front",
    );
    assert_forwarding_eq(
        &blanket.page_layout_metrics(TARGET),
        &direct.page_layout_metrics(TARGET),
        "page_layout_metrics",
    );
    assert_forwarding_eq(
        &blanket.page_print_to_pdf(TARGET),
        &direct.page_print_to_pdf(TARGET),
        "page_print_to_pdf",
    );

    assert_forwarding_eq(
        &blanket.runtime_evaluate(TARGET, "1+1"),
        &direct.runtime_evaluate(TARGET, "1+1"),
        "runtime_evaluate",
    );
    assert_forwarding_eq(
        &blanket.runtime_call_function_on(TARGET, "obj-1", "function(){}", &[]),
        &direct.runtime_call_function_on(TARGET, "obj-1", "function(){}", &[]),
        "runtime_call_function_on",
    );
    assert_forwarding_eq(
        &blanket.runtime_get_properties(TARGET, "obj-1", true),
        &direct.runtime_get_properties(TARGET, "obj-1", true),
        "runtime_get_properties",
    );
    assert_forwarding_eq(
        &blanket.runtime_release_object(TARGET, "obj-1"),
        &direct.runtime_release_object(TARGET, "obj-1"),
        "runtime_release_object",
    );
    assert_forwarding_eq(
        &blanket.runtime_enable(TARGET),
        &direct.runtime_enable(TARGET),
        "runtime_enable",
    );
    assert_forwarding_eq(
        &blanket.runtime_disable(TARGET),
        &direct.runtime_disable(TARGET),
        "runtime_disable",
    );
}

#[test]
fn blanket_impl_forwards_dom_network_input_emulation_faces() {
    // @trace REQ-CDP-004 [level:integration]
    // @trace REQ-CDP-006 [level:integration]
    let blanket: Arc<dyn ServoBackend> = Arc::new(MockServoBackend::new());
    let direct = MockServoBackend::new();

    assert_forwarding_eq(
        &blanket.dom_get_document(TARGET, 3),
        &direct.dom_get_document(TARGET, 3),
        "dom_get_document",
    );
    assert_forwarding_eq(
        &blanket.dom_query_selector(TARGET, 1, "div"),
        &direct.dom_query_selector(TARGET, 1, "div"),
        "dom_query_selector",
    );
    assert_forwarding_eq(
        &blanket.dom_query_selector_all(TARGET, 1, "div"),
        &direct.dom_query_selector_all(TARGET, 1, "div"),
        "dom_query_selector_all",
    );
    assert_forwarding_eq(
        &blanket.dom_get_box_model(TARGET, 7),
        &direct.dom_get_box_model(TARGET, 7),
        "dom_get_box_model",
    );
    assert_forwarding_eq(
        &blanket.dom_resolve_node(TARGET, 11),
        &direct.dom_resolve_node(TARGET, 11),
        "dom_resolve_node",
    );
    assert_forwarding_eq(
        &blanket.dom_describe_node(TARGET, 7, 1),
        &direct.dom_describe_node(TARGET, 7, 1),
        "dom_describe_node",
    );
    assert_forwarding_eq(
        &blanket.dom_set_attribute(TARGET, 7, "id", "v"),
        &direct.dom_set_attribute(TARGET, 7, "id", "v"),
        "dom_set_attribute",
    );
    assert_forwarding_eq(
        &blanket.dom_remove_attribute(TARGET, 7, "id"),
        &direct.dom_remove_attribute(TARGET, 7, "id"),
        "dom_remove_attribute",
    );
    assert_forwarding_eq(
        &blanket.dom_get_outer_html(TARGET, 7),
        &direct.dom_get_outer_html(TARGET, 7),
        "dom_get_outer_html",
    );
    assert_forwarding_eq(
        &blanket.dom_set_outer_html(TARGET, 7, "<b/>"),
        &direct.dom_set_outer_html(TARGET, 7, "<b/>"),
        "dom_set_outer_html",
    );
    assert_forwarding_eq(
        &blanket.dom_request_node(TARGET, "obj-1"),
        &direct.dom_request_node(TARGET, "obj-1"),
        "dom_request_node",
    );

    assert_forwarding_eq(
        &blanket.network_enable(TARGET),
        &direct.network_enable(TARGET),
        "network_enable",
    );
    assert_forwarding_eq(
        &blanket.network_disable(TARGET),
        &direct.network_disable(TARGET),
        "network_disable",
    );
    assert_forwarding_eq(
        &blanket.network_get_response_body(TARGET, "req-1"),
        &direct.network_get_response_body(TARGET, "req-1"),
        "network_get_response_body",
    );
    assert_forwarding_eq(
        &blanket.network_set_cache_disabled(TARGET, true),
        &direct.network_set_cache_disabled(TARGET, true),
        "network_set_cache_disabled",
    );

    let mouse = |button: &str| bao_cdp_client::MouseEvent {
        event_type: "mousePressed".into(),
        x: 1.0,
        y: 2.0,
        button: button.into(),
        click_count: 1,
        modifiers: 0,
    };
    assert_forwarding_eq(
        &blanket.input_dispatch_mouse_event(TARGET, mouse("left")),
        &direct.input_dispatch_mouse_event(TARGET, mouse("left")),
        "input_dispatch_mouse_event",
    );
    let key = || bao_cdp_client::KeyEvent {
        event_type: "keyDown".into(),
        key: "a".into(),
        code: "KeyA".into(),
        modifiers: 0,
        text: "a".into(),
        windows_virtual_key_code: 65,
    };
    assert_forwarding_eq(
        &blanket.input_dispatch_key_event(TARGET, key()),
        &direct.input_dispatch_key_event(TARGET, key()),
        "input_dispatch_key_event",
    );
    let touch = || bao_cdp_client::TouchPoint {
        state: "touchStart".into(),
        x: 1.0,
        y: 1.0,
        radius_x: 1.0,
        radius_y: 1.0,
        force: 0.5,
    };
    assert_forwarding_eq(
        &blanket.input_dispatch_touch_event(TARGET, "touchStart", &[touch()]),
        &direct.input_dispatch_touch_event(TARGET, "touchStart", &[touch()]),
        "input_dispatch_touch_event",
    );
    assert_forwarding_eq(
        &blanket.input_set_ignore_input_events(TARGET, true),
        &direct.input_set_ignore_input_events(TARGET, true),
        "input_set_ignore_input_events",
    );

    let metrics = || bao_cdp_client::DeviceMetrics {
        width: 1280,
        height: 720,
        device_scale_factor: 1.0,
        mobile: false,
    };
    assert_forwarding_eq(
        &blanket.emulation_set_device_metrics(TARGET, metrics()),
        &direct.emulation_set_device_metrics(TARGET, metrics()),
        "emulation_set_device_metrics",
    );
    assert_forwarding_eq(
        &blanket.emulation_clear_device_metrics(TARGET),
        &direct.emulation_clear_device_metrics(TARGET),
        "emulation_clear_device_metrics",
    );
    assert_forwarding_eq(
        &blanket.emulation_set_user_agent_override(TARGET, "UA/1"),
        &direct.emulation_set_user_agent_override(TARGET, "UA/1"),
        "emulation_set_user_agent_override",
    );
    assert_forwarding_eq(
        &blanket.emulation_set_geolocation_override(TARGET, 1.0, 2.0, 3.0),
        &direct.emulation_set_geolocation_override(TARGET, 1.0, 2.0, 3.0),
        "emulation_set_geolocation_override",
    );
}

#[test]
fn blanket_impl_forwards_target_css_debugger_and_event_tap() {
    // @trace REQ-CDP-003 [level:integration]
    let blanket: Arc<dyn ServoBackend> = Arc::new(MockServoBackend::new());
    let direct = MockServoBackend::new();

    assert_forwarding_eq(
        &blanket.target_get_targets(),
        &direct.target_get_targets(),
        "target_get_targets",
    );
    assert_forwarding_eq(
        &blanket.target_create_target("https://x.test/"),
        &direct.target_create_target("https://x.test/"),
        "target_create_target",
    );
    assert_forwarding_eq(
        &blanket.target_close_target(TARGET),
        &direct.target_close_target(TARGET),
        "target_close_target",
    );
    assert_forwarding_eq(
        &blanket.target_attach_to_target(TARGET),
        &direct.target_attach_to_target(TARGET),
        "target_attach_to_target",
    );
    assert_forwarding_eq(
        &blanket.target_detach_from_target("s-1"),
        &direct.target_detach_from_target("s-1"),
        "target_detach_from_target",
    );
    assert_forwarding_eq(
        &blanket.target_set_auto_attach(TARGET, true, false),
        &direct.target_set_auto_attach(TARGET, true, false),
        "target_set_auto_attach",
    );

    assert_forwarding_eq(
        &blanket.css_get_computed_style_for_node(TARGET, 7),
        &direct.css_get_computed_style_for_node(TARGET, 7),
        "css_get_computed_style_for_node",
    );
    assert_forwarding_eq(
        &blanket.css_get_matched_styles_for_node(TARGET, 7),
        &direct.css_get_matched_styles_for_node(TARGET, 7),
        "css_get_matched_styles_for_node",
    );

    assert_forwarding_eq(
        &blanket.debugger_enable(TARGET),
        &direct.debugger_enable(TARGET),
        "debugger_enable",
    );
    assert_forwarding_eq(
        &blanket.debugger_disable(TARGET),
        &direct.debugger_disable(TARGET),
        "debugger_disable",
    );
    assert_forwarding_eq(
        &blanket.debugger_set_breakpoint_by_url(TARGET, "https://x.test/a.js", 3, 0),
        &direct.debugger_set_breakpoint_by_url(TARGET, "https://x.test/a.js", 3, 0),
        "debugger_set_breakpoint_by_url",
    );
    assert_forwarding_eq(
        &blanket.debugger_remove_breakpoint(TARGET, 42, 3, 0),
        &direct.debugger_remove_breakpoint(TARGET, 42, 3, 0),
        "debugger_remove_breakpoint",
    );
    assert_forwarding_eq(
        &blanket.debugger_pause(TARGET),
        &direct.debugger_pause(TARGET),
        "debugger_pause",
    );
    assert_forwarding_eq(
        &blanket.debugger_resume(TARGET, Some(DebugStepAction::Into)),
        &direct.debugger_resume(TARGET, Some(DebugStepAction::Into)),
        "debugger_resume",
    );
    assert_forwarding_eq(
        &blanket.debugger_evaluate_on_call_frame(TARGET, "frame-0", "1+1"),
        &direct.debugger_evaluate_on_call_frame(TARGET, "frame-0", "1+1"),
        "debugger_evaluate_on_call_frame",
    );
    assert_forwarding_eq(
        &blanket.debugger_get_possible_breakpoints(TARGET, 42),
        &direct.debugger_get_possible_breakpoints(TARGET, 42),
        "debugger_get_possible_breakpoints",
    );
    assert_forwarding_eq(
        &blanket.debugger_get_script_source(TARGET, 42),
        &direct.debugger_get_script_source(TARGET, 42),
        "debugger_get_script_source",
    );

    // event_tap:mock 无 tap → blanket 转发同样 None(缺臂会静默遮蔽内层 tap)。
    assert!(
        blanket.event_tap().is_none(),
        "Arc<dyn> over tap-less mock must forward None"
    );
    assert!(direct.event_tap().is_none());
}

#[test]
fn trait_default_event_tap_and_mock_helpers() {
    // @trace REQ-CDP-001 [level:integration]
    // trait 默认 event_tap 体(None)与 mock into_backend/call_function_on_value。
    let plain = MockServoBackend::new();
    assert!(
        plain.event_tap().is_none(),
        "trait default event_tap must be None"
    );

    // call_function_on_value 缺省 → undefined 形状;Some(json!(7)) → number。
    let mut m = MockServoBackend::new();
    let r = m
        .runtime_call_function_on(TARGET, "obj-1", "function(){}", &[])
        .unwrap();
    assert_eq!(r.result.type_, "undefined");
    assert!(r.result.value.is_none());

    m.call_function_on_value = Some(json!(7));
    let r = m
        .runtime_call_function_on(TARGET, "obj-1", "function(){}", &[])
        .unwrap();
    assert_eq!(r.result.type_, "number");
    assert_eq!(r.result.value.as_ref().unwrap(), &json!(7));

    // into_backend:mock → Arc<dyn>,经 blanket 形态仍可用。
    let arc = m.into_backend();
    let nav = arc
        .page_navigate(TARGET, "https://x.test/")
        .expect("blanket page_navigate");
    assert_eq!(nav.frame_id, "FRAME_0", "mock default_frame_id forwarding");
}

// ═════════════════════════════════════════════════════════════════════════
// 第三面:bridge_sender_backend.rs 通道 trait 实现(泵线程应答)
// ═════════════════════════════════════════════════════════════════════════

fn backend_with(
    f: impl Fn(&BridgeCommand) -> Option<Value> + Send + 'static,
) -> BridgeSenderBackend {
    let (tx, rx) = bridge_channel(PUMP_TIMEOUT);
    spawn_answer_pump(rx, f);
    BridgeSenderBackend::new(tx)
}

fn default_backend() -> BridgeSenderBackend {
    backend_with(|cmd| Some(canned_bridge_answer(cmd)))
}

#[test]
fn bridge_page_faces_parse_channel_shapes() {
    // @trace REQ-CDP-001 [level:integration]
    let b = default_backend();
    let nav = b.page_navigate(TARGET, "https://x.test/").unwrap();
    assert_eq!(nav.frame_id, "frame-1");
    assert_eq!(nav.loader_id, "loader-1");
    assert!(nav.error_text.is_none());

    b.page_reload(TARGET).unwrap();
    b.page_set_content(TARGET, "<p>x</p>").unwrap();
    b.page_close(TARGET).unwrap();
    b.page_bring_to_front(TARGET).unwrap();

    // 截图:base64 data 解码(三格式通道名映射)。
    for (fmt, want_wire) in [
        (bao_cdp_client::BridgeScreenshotFormat::Png, "png"),
        (bao_cdp_client::BridgeScreenshotFormat::Jpeg, "jpeg"),
        (bao_cdp_client::BridgeScreenshotFormat::Webp, "webp"),
    ] {
        let shot = b.page_screenshot(TARGET, fmt).unwrap();
        assert_eq!(shot, b"hello", "base64 data must decode ({want_wire})");
    }
    // 无 data 字段 → 显式错误(不伪造空图)。
    let b2 = backend_with(|_| Some(json!({})));
    assert!(
        b2.page_screenshot(TARGET, bao_cdp_client::BridgeScreenshotFormat::Png)
            .is_err()
    );

    // frame_tree:合成表达式 JSON 剥壳 + main-{target} frame id。
    let tree = b.page_frame_tree(TARGET).unwrap();
    assert_eq!(tree.frame.id, format!("main-{TARGET}"));
    assert_eq!(tree.frame.url, "https://x.test/p");
    assert_eq!(tree.frame.name.as_deref(), Some("pf"));
    assert_eq!(tree.frame.mime_type, "text/html");
    assert!(
        tree.child_frames.is_empty(),
        "child frames are not enumerable (same boundary as production)"
    );

    // layout_metrics:JSON 剥壳读 viewport/content 尺寸。
    let m = b.page_layout_metrics(TARGET).unwrap();
    assert_eq!(m.layout_width, 1280.0);
    assert_eq!(m.layout_height, 720.0);
    assert_eq!(m.content_width, 1280.0);
    assert_eq!(m.content_height, 900.0);
}

#[test]
fn bridge_runtime_faces_parse_envelope_and_properties() {
    // @trace REQ-CDP-001 [level:integration]
    let b = default_backend();
    let r = b.runtime_evaluate(TARGET, "1+1").unwrap();
    // 默认 canned envelope = undefined 形状(evaluate 契约由异常面测试补)。
    assert_eq!(r.result.type_, "undefined");
    assert!(
        r.exception_details.is_none(),
        "no exceptionDetails in canned envelope"
    );

    // 带 exceptionDetails 的 envelope → 解析出 exception(text/line/col)。
    let b2 = backend_with(|_| {
        Some(json!({
            "result": { "type": "object" },
            "exceptionDetails": { "exceptionId": 9, "text": "TypeError: x", "lineNumber": 4, "columnNumber": 7,
                "exception": { "type": "object", "className": "TypeError" } }
        }))
    });
    let r = b2.runtime_evaluate(TARGET, "boom").unwrap();
    let exc = r.exception_details.expect("exceptionDetails must parse");
    assert_eq!(exc.text, "TypeError: x");
    assert_eq!(exc.line_number, 4);
    assert_eq!(exc.column_number, 7);
    assert_eq!(
        exc.exception.as_ref().and_then(|e| e.class_name.as_deref()),
        Some("TypeError")
    );

    // getProperties:属性对象逐字段解析(含 value RemoteObject)。
    let props = b.runtime_get_properties(TARGET, "obj-1", true).unwrap();
    assert_eq!(props.len(), 1);
    let p = &props[0];
    assert_eq!(p.name, "len");
    assert_eq!(p.writable, Some(true));
    assert_eq!(p.configurable, Some(true));
    assert_eq!(p.enumerable, Some(true));
    assert_eq!(p.is_own, true);
    let v = p.value.as_ref().expect("value RemoteObject parsed");
    assert_eq!(v.type_, "number");
    assert_eq!(v.value.as_ref().unwrap(), &json!(3));

    b.runtime_release_object(TARGET, "obj-1").unwrap();
    b.runtime_enable(TARGET).unwrap();
    b.runtime_disable(TARGET).unwrap();
    b.runtime_call_function_on(TARGET, "obj-1", "function(){}", &[])
        .unwrap();
}

#[test]
fn bridge_dom_css_faces_parse_node_and_style_shapes() {
    // @trace REQ-CDP-004 [level:integration]
    let b = default_backend();

    let doc = b.dom_get_document(TARGET, 1).unwrap();
    assert_eq!(doc.node_id, 1);
    assert_eq!(doc.node_name, "#document");
    assert_eq!(doc.backend_node_id, 10);
    assert_eq!(doc.children.len(), 1);
    assert_eq!(doc.children[0].node_name, "HTML");

    assert_eq!(b.dom_query_selector(TARGET, 1, "div").unwrap(), Some(7));
    // nodeId 0 = 未命中 → None(通道语义)。
    let b2 = backend_with(|_| Some(json!({ "nodeId": 0 })));
    assert_eq!(b2.dom_query_selector(TARGET, 1, "div").unwrap(), None);

    let all = b.dom_query_selector_all(TARGET, 1, "div").unwrap();
    assert_eq!(all, vec![7, 8, 9]);

    let html = b.dom_get_outer_html(TARGET, 7).unwrap();
    assert_eq!(html, "<div id=\"a\"></div>");

    b.dom_set_attribute(TARGET, 7, "id", "b").unwrap();
    // remove_attribute:通道无生产路径 → 显式 NotSupported(不伪造删除)。
    match b.dom_remove_attribute(TARGET, 7, "id") {
        Err(BridgeError::NotSupported(m)) => assert!(m.contains("DOM.removeAttribute"), "{m}"),
        other => panic!("expected NotSupported, got {other:?}"),
    }
    // resolve_node:通道无 nodeId → RemoteObject 路径 → 显式 NotSupported。
    match b.dom_resolve_node(TARGET, 11) {
        Err(BridgeError::NotSupported(m)) => assert!(m.contains("DOM.resolveNode"), "{m}"),
        other => panic!("expected NotSupported, got {other:?}"),
    }
    b.dom_describe_node(TARGET, 7, 1).unwrap();

    let computed = b.css_get_computed_style_for_node(TARGET, 7).unwrap();
    assert_eq!(computed.len(), 2);
    assert_eq!(computed[0].name, "color");
    assert_eq!(computed[0].value, "rgb(1, 2, 3)");

    let matched = b.css_get_matched_styles_for_node(TARGET, 7).unwrap();
    let inline = matched.inline_style.expect("inlineStyle parsed");
    assert_eq!(inline.css_properties.len(), 1);
    assert_eq!(inline.css_properties[0].name, "color");
    assert_eq!(matched.matched_rules.len(), 1);
    assert_eq!(matched.matched_rules[0].selector, "div.cls");
    assert_eq!(
        matched.matched_rules[0].style.css_properties[0].name,
        "margin"
    );
}

#[test]
fn bridge_network_input_emulation_target_faces() {
    // @trace REQ-CDP-006 [level:integration]
    let b = default_backend();
    b.network_enable(TARGET).unwrap();
    b.network_disable(TARGET).unwrap();
    b.network_set_cache_disabled(TARGET, true).unwrap();

    // getResponseBody:body/base64Encoded 逐字段(通道对端可诚实给值或报错,
    // 两侧都穿透)。
    let body = b.network_get_response_body(TARGET, "req-1").unwrap();
    assert_eq!(body.body, "Zm9v");
    assert!(body.base64_encoded);
    // 通道对端显式拒绝(servo 不向嵌入者暴露存储响应体)→ 错误穿透不吞。
    let b2 = backend_with(|cmd| match cmd {
        BridgeCommand::GetResponseBody { .. } => None,
        _ => Some(json!({})),
    });
    assert!(
        b2.network_get_response_body(TARGET, "req-1").is_err(),
        "peer error must propagate"
    );

    use bao_cdp_client::{KeyEvent, MouseEvent};
    b.input_dispatch_mouse_event(
        TARGET,
        MouseEvent {
            event_type: "mousePressed".into(),
            x: 5.0,
            y: 6.0,
            button: String::new(),
            click_count: 1,
            modifiers: 0,
        },
    )
    .unwrap();
    // 空 button → None(不传 "none" 字面到通道)。
    b.input_dispatch_key_event(
        TARGET,
        KeyEvent {
            event_type: "keyDown".into(),
            key: "Enter".into(),
            code: "Enter".into(),
            modifiers: 0,
            text: String::new(),
            windows_virtual_key_code: 13,
        },
    )
    .unwrap();

    b.emulation_set_device_metrics(
        TARGET,
        bao_cdp_client::DeviceMetrics {
            width: 100,
            height: 200,
            device_scale_factor: 2.0,
            mobile: false,
        },
    )
    .unwrap();
    // 负宽 clamp 到 0(max(0))——不发负 u32。
    b.emulation_set_device_metrics(
        TARGET,
        bao_cdp_client::DeviceMetrics {
            width: -5,
            height: 200,
            device_scale_factor: 1.0,
            mobile: false,
        },
    )
    .unwrap();
    b.emulation_set_user_agent_override(TARGET, "UA/2").unwrap();

    b.target_close_target(TARGET).unwrap();
    b.target_set_auto_attach(TARGET, true, false).unwrap();

    let targets = b.target_get_targets().unwrap();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].target_id, "t-1");
    assert_eq!(targets[0].type_, "page");
    assert!(
        targets[0].attached,
        "in-memory client is inherently attached"
    );
    assert_eq!(
        targets[0].browser_context_id.as_deref(),
        Some("bao-default-context")
    );

    let created = b.target_create_target("https://x.test/").unwrap();
    assert_eq!(created, "t-new");
}

#[test]
fn bridge_debugger_faces_parse_channel_shapes() {
    // @trace REQ-CDP-003 [level:integration]
    let b = default_backend();
    b.debugger_enable(TARGET).unwrap();
    b.debugger_disable(TARGET).unwrap();
    b.debugger_pause(TARGET).unwrap();
    b.debugger_remove_breakpoint(TARGET, 42, 3, 0).unwrap();
    b.debugger_resume(TARGET, Some(DebugStepAction::Out))
        .unwrap();

    let bp = b
        .debugger_set_breakpoint_by_url(TARGET, "https://x.test/a.js", 10, 2)
        .unwrap();
    assert_eq!(bp.breakpoint_id, "bp-1");
    assert_eq!(bp.script_id, 42);
    assert_eq!(bp.actual_line, 7);
    assert_eq!(bp.actual_column, 3);

    let bps = b.debugger_get_possible_breakpoints(TARGET, 42).unwrap();
    assert_eq!(bps.len(), 2);
    assert_eq!(bps[1].line_number, 2);
    assert_eq!(bps[1].column_number, 4);

    let src = b.debugger_get_script_source(TARGET, 42).unwrap();
    assert_eq!(src, "function f(){return 1}");

    let ev = b
        .debugger_evaluate_on_call_frame(TARGET, "frame-0", "1+1")
        .unwrap();
    assert_eq!(ev.result.type_, "string");
}

#[test]
fn bridge_not_supported_faces_are_explicit() {
    // @trace REQ-CDP-001 [level:integration]
    // 通道无生产路径的方法 = 显式 NotSupported(禁假成功)。每臂逐字面登记。
    let b = default_backend();
    let cases: Vec<(&str, Box<dyn Fn() -> Result<(), BridgeError>>)> = vec![
        (
            "Page.getNavigationHistory",
            Box::new(|| b.page_navigation_history(TARGET).map(|_| ())),
        ),
        (
            "Page.navigateToHistoryEntry",
            Box::new(|| b.page_navigate_to_history_entry(TARGET, 1)),
        ),
        (
            "Page.printToPDF",
            Box::new(|| b.page_print_to_pdf(TARGET).map(|_| ())),
        ),
        (
            "Input.dispatchTouchEvent",
            Box::new(|| b.input_dispatch_touch_event(TARGET, "touchStart", &[])),
        ),
        (
            "Input.setIgnoreInputEvents",
            Box::new(|| b.input_set_ignore_input_events(TARGET, false)),
        ),
        (
            "Emulation.clearDeviceMetricsOverride",
            Box::new(|| b.emulation_clear_device_metrics(TARGET)),
        ),
        (
            "Emulation.setGeolocationOverride",
            Box::new(|| b.emulation_set_geolocation_override(TARGET, 0.0, 0.0, 0.0)),
        ),
        (
            "Target.attachToTarget",
            Box::new(|| b.target_attach_to_target(TARGET).map(|_| ())),
        ),
        (
            "Target.detachFromTarget",
            Box::new(|| b.target_detach_from_target("s-1")),
        ),
        (
            "DOM.setOuterHTML",
            Box::new(|| b.dom_set_outer_html(TARGET, 7, "<b/>")),
        ),
        (
            "DOM.requestNode",
            Box::new(|| b.dom_request_node(TARGET, "obj-1").map(|_| ())),
        ),
        (
            "DOM.getBoxModel",
            Box::new(|| b.dom_get_box_model(TARGET, 7).map(|_| ())),
        ),
    ];
    for (face, call) in cases {
        let err = call().expect_err(&format!("{face} must be explicitly NotSupported"));
        match err {
            BridgeError::NotSupported(msg) => assert!(
                msg.contains(face.split('.').next().unwrap()),
                "{face}: {msg}"
            ),
            other => panic!("{face}: expected NotSupported, got {other:?}"),
        }
    }
}

#[test]
fn bridge_eval_json_doc_error_arms_and_closed_channel() {
    // @trace REQ-CDP-001 [level:integration]
    // (a) envelope 无 result.value → 显式合成失败(不伪造文档)。
    let b1 = backend_with(|_| Some(json!({})));
    let err = b1.page_frame_tree(TARGET).unwrap_err();
    match err {
        BridgeError::ServoError(m) => assert!(m.contains("no result.value"), "{m}"),
        other => panic!("expected ServoError, got {other:?}"),
    }
    // (b) result.value 非法 JSON 字符串 → 解析失败。
    let b2 =
        backend_with(|_| Some(json!({ "result": { "type": "string", "value": "not-json{{{" } })));
    let err = b2.page_frame_tree(TARGET).unwrap_err();
    match err {
        BridgeError::ServoError(m) => assert!(m.contains("unparseable JSON"), "{m}"),
        other => panic!("expected ServoError, got {other:?}"),
    }

    // (c) 通道对端已 drop → bridge closed → ServoError 包装(不 panic)。
    let (tx, rx) = bridge_channel(PUMP_TIMEOUT);
    drop(rx);
    let b3 = BridgeSenderBackend::new(tx);
    let err = b3.page_navigate(TARGET, "https://x.test/").unwrap_err();
    match err {
        BridgeError::ServoError(m) => assert!(m.contains("bridge"), "{m}"),
        other => panic!("expected ServoError, got {other:?}"),
    }
}

#[test]
fn bridge_tap_is_shared_arc_and_pump_record_shape() {
    // @trace REQ-CDP-001 [level:integration]
    let b = default_backend();
    // tap 与 backend 同生命周期,宿主泵经 tap() 喂事件(同 Arc)。
    let t1 = b.tap();
    let t2 = b.tap();
    assert!(Arc::ptr_eq(t1, t2), "tap() must return the same Arc");

    // 泵捕获面:命令确实到达通道对端(recording pump 形状)。
    let (tx, rx) = bridge_channel(PUMP_TIMEOUT);
    let seen: Arc<Mutex<Vec<BridgeCommand>>> = Arc::new(Mutex::new(Vec::new()));
    let seen_for_pump = seen.clone();
    spawn_answer_pump(rx, move |cmd| {
        seen_for_pump.lock().unwrap().push(cmd.clone());
        Some(canned_bridge_answer(cmd))
    });
    let sender = BridgeSenderBackend::new(tx);
    let _ = sender.page_reload(TARGET);
    // 收尾等泵记账(有界自旋,泵在应答线程内 push 先于 send 返回 — 直接断言)。
    let seen = seen.lock().unwrap();
    assert!(
        seen.iter()
            .any(|c| matches!(c, BridgeCommand::Reload { target_id, .. } if target_id == TARGET)),
        "Reload must cross the channel: {seen:?}"
    );
}
