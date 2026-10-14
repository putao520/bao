//! Network domain conformance 审计 — 4 method。
//!
//! 对照 CDP 官方规范(Network domain):
//! https://chromedevtools.github.io/devtools-protocol/tot/Network/
//!
//! # 覆盖 method
//!
//! enable, disable, getResponseBody, setCacheDisabled
//! Fetch.enable/disable/continueRequest/failRequest/fulfillRequest +
//! Fetch.requestPaused / Network.requestIntercepted 事件(REQ-CDP-006 C6
//! 拦截验收段,E5 — 请求拦截链 client 侧可观测面,见文末专节)
//!
//! @trace REQ-CDP-001 [domain:Network] [level:integration]
//! @trace REQ-BAO-API-004 [domain:Network] [level:integration]
//! @trace REQ-CDP-006 [domain:Fetch] [level:integration]

use bao_cdp_client::{dispatch_command, BridgeError, MockServoBackend, ServoBackend};
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

// ── REQ-CDP-006 C6 拦截验收段(E5)依赖 ─────────────────────────────────
// 命令下行走生产命令核(bao_cdp::handle_command)+ 真 bridge 通道 + 泵线程;
// 事件上行走生产 push 面(EventSubscriber/ServoEvent)+ transport translate。
use bao_cdp::servo_bridge::{
    BridgeCommand, BridgeResponse, BridgeSender, FetchRequestStage, FetchResolveAction,
};
use bao_cdp::{bridge_channel, handle_command, CdpMessage};
use bao_cdp_client::bridge::{CDPRdpBridge, EventSubscriber, ServoEvent};
use bao_cdp_client::transport::{InMemoryTransport, Transport};

fn backend() -> Arc<dyn ServoBackend> {
    Arc::new(MockServoBackend::new())
}

// ─────────────────────────────────────────────────────────────────────────
// Network.enable / disable — CDP spec: empty return {}
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn network_enable_returns_empty_object() {
    // @trace REQ-CDP-001 [domain:Network] [level:integration]
    let b = backend();
    let result = dispatch_command(&*b, "Network.enable", json!({}), "1").unwrap();
    assert!(
        result.as_object().map(|o| o.is_empty()).unwrap_or(false),
        "CDP spec: Network.enable returns empty object"
    );
}

#[test]
fn network_disable_returns_empty_object() {
    // @trace REQ-CDP-001 [domain:Network] [level:integration]
    let b = backend();
    let result = dispatch_command(&*b, "Network.disable", json!({}), "1").unwrap();
    assert!(
        result.as_object().map(|o| o.is_empty()).unwrap_or(false),
        "CDP spec: Network.disable returns empty object"
    );
}

#[test]
fn network_enable_optional_params_accepted() {
    // Arrange — CDP 规范: maxTotalBufferSize / maxResourceBufferSize / maxPostDataSize 可选
    // @trace REQ-CDP-001 [domain:Network] [level:integration]
    let b = backend();
    let result = dispatch_command(
        &*b,
        "Network.enable",
        json!({"maxTotalBufferSize":10_000_000, "maxPostDataSize":1024}),
        "1",
    )
    .unwrap();
    assert!(result.as_object().map(|o| o.is_empty()).unwrap_or(false));
}

// ─────────────────────────────────────────────────────────────────────────
// Network.getResponseBody — CDP spec: returns {body: string, base64Encoded: boolean}
// https://chromedevtools.github.io/devtools-protocol/tot/Network/#method-getResponseBody
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn network_get_response_body_result_schema_conformance() {
    // Arrange — CDP 规范: 返回 {body: string, base64Encoded: boolean}
    // @trace REQ-CDP-001 [domain:Network] [level:integration]
    let b = backend();

    // Act
    let result = dispatch_command(
        &*b,
        "Network.getResponseBody",
        json!({"requestId":"req1"}),
        "1",
    )
    .unwrap();

    // Assert
    assert!(
        result["body"].is_string(),
        "CDP spec: body must be string, got: {:?}",
        result["body"]
    );
    assert!(
        result["base64Encoded"].is_boolean(),
        "CDP spec: base64Encoded must be boolean, got: {:?}",
        result["base64Encoded"]
    );
}

#[test]
fn network_get_response_body_missing_request_id_returns_32602() {
    // @trace REQ-BAO-API-007 [domain:Network] [level:integration]
    let b = backend();
    let err = dispatch_command(&*b, "Network.getResponseBody", json!({}), "1").unwrap_err();
    assert!(matches!(err, BridgeError::InvalidParams(_)));
    assert_eq!(err.cdp_error_code(), -32602);
}

// ─────────────────────────────────────────────────────────────────────────
// Network.setCacheDisabled — CDP spec: empty return {}
// ─────────────────────────────────────────────────────────────────────────

#[test]
fn network_set_cache_disabled_returns_empty_object() {
    // @trace REQ-CDP-001 [domain:Network] [level:integration]
    let b = backend();
    let result = dispatch_command(
        &*b,
        "Network.setCacheDisabled",
        json!({"cacheDisabled":true}),
        "1",
    )
    .unwrap();
    assert!(
        result.as_object().map(|o| o.is_empty()).unwrap_or(false),
        "CDP spec: setCacheDisabled returns empty object"
    );
}

#[test]
fn network_set_cache_disabled_default_param_accepted() {
    // Arrange — cacheDisabled 默认 false(缺省时应被接受)
    // @trace REQ-CDP-001 [domain:Network] [level:integration]
    let b = backend();
    let result = dispatch_command(&*b, "Network.setCacheDisabled", json!({}), "1").unwrap();
    assert!(result.as_object().map(|o| o.is_empty()).unwrap_or(false));
}

// ─────────────────────────────────────────────────────────────────────────
// Fetch 请求拦截(REQ-CDP-006 C6)— E5 验收段
//
// 拦截链在本 crate(client 侧)的可观测半边:
// - **命令下行**:生产命令核 `bao_cdp::handle_command` → 真 `bridge_channel`
//   → 泵线程 `recv_and_process`(生产拓扑:CDP 线程 send / 主线程泵;泵与
//   bao_browser cdp_handler 同位,按序记录到达的 BridgeCommand 并应答)。
// - **事件上行**:`ServoEvent`(生产 push 面 = bao_browser delegate 持有的
//   EventSubscriber 同一载体)→ `recv_event` 有界等待 → translate → CDP
//   wire 事件。
//
// pause 决策(pattern 匹配/槽状态机)与 load 决议(drop/intercept/cancel、
// fulfill body 解码)的引擎半边归 bao_browser(E3 纯函数矩阵 / E4 handler 臂 /
// 真引擎 oracle),不在本文件断言;这里钉死的是 client 侧契约:命令编码、
// 事件 wire 形状、interceptionId 从 pause 事件到决议命令的数据流,以及
// 设计 §4 的事件序(①requestWillBeSent → ②requestPaused → ③a continue
// 完成面 / ③b fulfill 合成响应 / ③c fail 网络错误)。
// ─────────────────────────────────────────────────────────────────────────

/// 拦截面固定 target(CdpMessage 与 BridgeCommand 双侧同源)。
const INTERCEPT_TARGET: &str = "t-net";
/// 泵线程空闲 tick(recv_and_process 的有界等待;等待闭包驱动,禁纯 sleep)。
const PUMP_TICK: Duration = Duration::from_millis(20);
/// 事件消费有界等待上限(recv_event 的 event_timeout)。
const EVENT_WAIT: Duration = Duration::from_secs(5);

/// 构造 sessionless CdpMessage(生产 wire 形态,id 为 JSON-RPC 2.0 请求 id)。
fn intercept_cdp_msg(id: i64, method: &str, params: Option<serde_json::Value>) -> CdpMessage {
    CdpMessage {
        id: Some(id),
        method: method.into(),
        params,
        session_id: None,
    }
}

/// 拦截命令面驱动台:真 bridge 通道 + 泵线程。
///
/// 泵线程与 bao_browser 桥消费端(cdp_handler)同位:`recv_and_process` 阻塞
/// 等待命令、按序记录、应答 ok_empty(生产桥对 enable/disable/resolve 三动词
/// 的应答同形)。`handle_command` 的 send 是同步阻塞往返——返回即命令已被泵
/// 处理,断言无需额外等待。
fn spawn_bridge_pump() -> (
    BridgeSender,
    Arc<Mutex<Vec<BridgeCommand>>>,
    Arc<AtomicBool>,
    JoinHandle<()>,
) {
    let (sender, receiver) = bridge_channel(Duration::from_secs(5));
    let captured = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let (captured_in_pump, stop_in_pump) = (Arc::clone(&captured), Arc::clone(&stop));
    let handle = thread::spawn(move || {
        while !stop_in_pump.load(Ordering::SeqCst) {
            receiver.recv_and_process(PUMP_TICK, |cmd| {
                captured_in_pump.lock().unwrap().push(cmd);
                BridgeResponse {
                    result: Ok(json!({})),
                }
            });
        }
    });
    (sender, captured, stop, handle)
}

fn join_pump(stop: Arc<AtomicBool>, handle: JoinHandle<()>) {
    stop.store(true, Ordering::SeqCst);
    handle.join().expect("bridge pump thread must not panic");
}

/// 断言一次拦截命令往返成功(ok_empty)并取回泵记录的最新命令。
fn pump_popped(captured: &Arc<Mutex<Vec<BridgeCommand>>>) -> BridgeCommand {
    captured
        .lock()
        .unwrap()
        .pop()
        .expect("bridge pump must have captured exactly one command")
}

/// 事件半边:内存桥 transport + 事件订阅(生产 push 面)。
fn interception_event_chain() -> (InMemoryTransport, EventSubscriber) {
    let bridge = CDPRdpBridge::new(backend());
    let mut transport = InMemoryTransport::new(bridge.into_in_memory_bridge());
    let (subscriber, rx) = EventSubscriber::new();
    transport.attach_servo_event_receiver(rx);
    transport.set_event_timeout(EVENT_WAIT);
    (transport, subscriber)
}

/// continueRequest 续行全回路:enable(pattern)→ ①requestWillBeSent →
/// ②requestPaused(真实请求字段)→ 以事件携带的 interceptionId 回指决议 →
/// Continue → ③a 请求照常完成(responseReceived/loadingFinished)。
#[test]
fn fetch_interception_pause_then_continue_request_completes() {
    // @trace REQ-CDP-006 [domain:Fetch] [level:integration]
    let (sender, captured, stop, pump) = spawn_bridge_pump();

    // Act ① — arm:单条 urlPattern(Request stage 缺省)。
    let msg = intercept_cdp_msg(
        1,
        "Fetch.enable",
        Some(json!({"patterns": [{"urlPattern": "https://api.example.com/*"}]})),
    );
    let params = msg.params.clone();
    let resp = handle_command(msg, INTERCEPT_TARGET, &params, Some(&sender));
    // Assert ① — enable ok_empty;泵收到的 FetchEnable 携带归一化 pattern。
    assert!(
        resp.error.is_none(),
        "Fetch.enable must arm the slot: {:?}",
        resp.error
    );
    assert!(
        resp.result
            .as_ref()
            .and_then(|v| v.as_object())
            .map(|o| o.is_empty())
            .unwrap_or(false),
        "CDP spec: Fetch.enable returns empty object"
    );
    match pump_popped(&captured) {
        BridgeCommand::FetchEnable {
            target_id,
            patterns,
            legacy_network_events,
        } => {
            assert_eq!(target_id, INTERCEPT_TARGET);
            assert!(!legacy_network_events, "Fetch 域命中发 requestPaused");
            assert_eq!(patterns.len(), 1);
            assert_eq!(
                patterns[0].url_pattern.as_deref(),
                Some("https://api.example.com/*")
            );
            assert_eq!(patterns[0].resource_type, None);
            assert_eq!(
                patterns[0].request_stage,
                FetchRequestStage::Request,
                "requestStage 缺省归一化为 Request"
            );
        }
        other => panic!("expected BridgeCommand::FetchEnable, got {other:?}"),
    }

    // Act ② — 命中 pattern 的流量到达(引擎半边由本测试代演 servo net 栈 +
    // delegate:先 requestWillBeSent 再 pause,设计 §4 ①→②)。
    let (mut transport, subscriber) = interception_event_chain();
    let mut headers = HashMap::new();
    headers.insert("User-Agent".to_string(), "bao-conformance/1.0".to_string());
    subscriber.on_network_request(
        INTERCEPT_TARGET,
        "REQ-A",
        "https://api.example.com/v1/data",
        "GET",
        headers.clone(),
        None,
        "Fetch",
        "main-t-net",
    );
    subscriber
        .sender()
        .send(ServoEvent::FetchRequestPaused {
            target_id: INTERCEPT_TARGET.to_string(),
            request_id: "REQ-A".to_string(),
            interception_id: "interception-1".to_string(),
            url: "https://api.example.com/v1/data".to_string(),
            method: "GET".to_string(),
            headers: headers.clone(),
            resource_type: "Fetch".to_string(),
            is_redirect: false,
        })
        .expect("pause event push");

    // Assert ② — 事件序 ①→② 与真实请求字段。
    let will_be_sent = transport.recv_event().unwrap().expect("requestWillBeSent");
    assert_eq!(will_be_sent.method, "Network.requestWillBeSent");
    assert_eq!(will_be_sent.session_id.as_deref(), Some(INTERCEPT_TARGET));
    assert_eq!(will_be_sent.params["requestId"], "REQ-A");

    let paused = transport.recv_event().unwrap().expect("requestPaused");
    assert_eq!(paused.method, "Fetch.requestPaused");
    assert_eq!(paused.session_id.as_deref(), Some(INTERCEPT_TARGET));
    assert_eq!(
        paused.params["requestId"], "REQ-A",
        "关联键:与 requestWillBeSent 同源(设计 §4)"
    );
    assert_eq!(
        paused.params["interceptionId"], "interception-1",
        "决议键:continueRequest/failRequest/fulfillRequest 以它回指"
    );
    assert_eq!(paused.params["request"]["url"], "https://api.example.com/v1/data");
    assert_eq!(paused.params["request"]["method"], "GET");
    assert_eq!(
        paused.params["request"]["headers"]["User-Agent"],
        "bao-conformance/1.0"
    );
    assert_eq!(paused.params["resourceType"], "Fetch");
    assert_eq!(paused.params["isRedirect"], false);

    // Act ③ — 以 pause 事件携带的 interceptionId 回指决议(真客户端数据流,
    // 禁硬编码决议键)。
    let interception_id = paused.params["interceptionId"].as_str().unwrap().to_string();
    let msg = intercept_cdp_msg(
        2,
        "Fetch.continueRequest",
        Some(json!({"interceptionId": interception_id})),
    );
    let params = msg.params.clone();
    let resp = handle_command(msg, INTERCEPT_TARGET, &params, Some(&sender));
    assert!(
        resp.error.is_none(),
        "continueRequest must resolve the pause: {:?}",
        resp.error
    );
    match pump_popped(&captured) {
        BridgeCommand::FetchResolve {
            target_id,
            interception_id: id,
            action,
        } => {
            assert_eq!(target_id, INTERCEPT_TARGET);
            assert_eq!(id, "interception-1");
            assert!(
                matches!(action, FetchResolveAction::Continue),
                "Continue = 放行 held load → DoNotIntercept → 请求照常进网络(设计 §3.2)"
            );
        }
        other => panic!("expected BridgeCommand::FetchResolve(Continue), got {other:?}"),
    }

    // Assert ④ — 续行完成面:同请求 responseReceived + loadingFinished(§4 ③a)。
    subscriber.on_network_response(
        INTERCEPT_TARGET,
        "REQ-A",
        "https://api.example.com/v1/data",
        200,
        "OK",
        headers,
        "application/json",
        Some("1.2.3.4".to_string()),
    );
    subscriber.on_network_loading_finish(INTERCEPT_TARGET, "REQ-A", 1024);
    let responded = transport.recv_event().unwrap().expect("responseReceived");
    assert_eq!(responded.method, "Network.responseReceived");
    assert_eq!(responded.params["requestId"], "REQ-A");
    let finished = transport.recv_event().unwrap().expect("loadingFinished");
    assert_eq!(finished.method, "Network.loadingFinished");
    assert_eq!(finished.params["requestId"], "REQ-A");

    join_pump(stop, pump);
}

/// failRequest 错误面:enable(无 patterns → catch-all 归一化)→ pause →
/// failRequest{errorReason} → ③c 同请求 Network.loadingFailed。
#[test]
fn fetch_interception_fail_request_surfaces_network_error_face() {
    // @trace REQ-CDP-006 [domain:Fetch] [level:integration]
    let (sender, captured, stop, pump) = spawn_bridge_pump();

    // Act ① — 无 patterns 参数 = catch-all Request-stage 归一化(Chromium
    // omitted-patterns 语义)。
    let msg = intercept_cdp_msg(1, "Fetch.enable", None);
    let resp = handle_command(msg, INTERCEPT_TARGET, &None, Some(&sender));
    assert!(resp.error.is_none(), "catch-all enable must arm: {:?}", resp.error);
    match pump_popped(&captured) {
        BridgeCommand::FetchEnable { patterns, .. } => {
            assert_eq!(patterns.len(), 1, "缺省归一化为单条 catch-all");
            assert_eq!(patterns[0].url_pattern, None);
            assert_eq!(patterns[0].resource_type, None);
            assert_eq!(patterns[0].request_stage, FetchRequestStage::Request);
        }
        other => panic!("expected BridgeCommand::FetchEnable, got {other:?}"),
    }

    // Act ② — pause 事件(沿用上一事件链;每个 nextest 用例独立进程)。
    let (mut transport, subscriber) = interception_event_chain();
    subscriber
        .sender()
        .send(ServoEvent::FetchRequestPaused {
            target_id: INTERCEPT_TARGET.to_string(),
            request_id: "REQ-B".to_string(),
            interception_id: "interception-2".to_string(),
            url: "https://blocked.example.com/track.js".to_string(),
            method: "GET".to_string(),
            headers: HashMap::new(),
            resource_type: "Script".to_string(),
            is_redirect: false,
        })
        .expect("pause event push");
    let paused = transport.recv_event().unwrap().expect("requestPaused");
    assert_eq!(paused.method, "Fetch.requestPaused");
    assert_eq!(paused.params["interceptionId"], "interception-2");

    // Act ③ — failRequest{interceptionId, errorReason}。
    let msg = intercept_cdp_msg(
        2,
        "Fetch.failRequest",
        Some(json!({"interceptionId": "interception-2", "errorReason": "Failed"})),
    );
    let params = msg.params.clone();
    let resp = handle_command(msg, INTERCEPT_TARGET, &params, Some(&sender));
    assert!(
        resp.error.is_none(),
        "failRequest must resolve the pause: {:?}",
        resp.error
    );
    // Assert ③ — errorReason 仅 advisory(servo pause 面无错误码通道,
    // page 侧统一网络错误;设计 §7)。
    match pump_popped(&captured) {
        BridgeCommand::FetchResolve { action, .. } => match action {
            FetchResolveAction::Fail { error_reason } => {
                assert_eq!(error_reason.as_deref(), Some("Failed"));
            }
            other => panic!("expected FetchResolveAction::Fail, got {other:?}"),
        },
        other => panic!("expected BridgeCommand::FetchResolve, got {other:?}"),
    }

    // Assert ④ — 网络错误面:同请求 loadingFailed(§4 ③c)。
    subscriber.on_network_loading_fail(INTERCEPT_TARGET, "REQ-B", "Failed", false);
    let failed = transport.recv_event().unwrap().expect("loadingFailed");
    assert_eq!(failed.method, "Network.loadingFailed");
    assert_eq!(failed.session_id.as_deref(), Some(INTERCEPT_TARGET));
    assert_eq!(failed.params["requestId"], "REQ-B");

    join_pump(stop, pump);
}

/// fulfillRequest 换响应体:pause → fulfillRequest{responseCode/responsePhrase/
/// responseHeaders/body(base64)} → ③b responseReceived 携带合成响应
/// (status/headers 即 fulfill 值;body 字节生效面=引擎半边 bao_browser 解码)。
#[test]
fn fetch_interception_fulfill_request_synthesizes_response() {
    // @trace REQ-CDP-006 [domain:Fetch] [level:integration]
    let (sender, captured, stop, pump) = spawn_bridge_pump();

    // Act ① — arm catch-all。
    let msg = intercept_cdp_msg(1, "Fetch.enable", None);
    let resp = handle_command(msg, INTERCEPT_TARGET, &None, Some(&sender));
    assert!(resp.error.is_none());
    let _ = pump_popped(&captured);

    // Act ② — pause。
    let (mut transport, subscriber) = interception_event_chain();
    subscriber
        .sender()
        .send(ServoEvent::FetchRequestPaused {
            target_id: INTERCEPT_TARGET.to_string(),
            request_id: "REQ-C".to_string(),
            interception_id: "interception-3".to_string(),
            url: "https://api.example.com/v1/data".to_string(),
            method: "GET".to_string(),
            headers: HashMap::new(),
            resource_type: "Fetch".to_string(),
            is_redirect: false,
        })
        .expect("pause event push");
    let paused = transport.recv_event().unwrap().expect("requestPaused");
    assert_eq!(paused.method, "Fetch.requestPaused");

    // Act ③ — fulfillRequest:换 status/headers/body("hello" 的 base64)。
    let msg = intercept_cdp_msg(
        2,
        "Fetch.fulfillRequest",
        Some(json!({
            "interceptionId": "interception-3",
            "responseCode": 201,
            "responsePhrase": "Created",
            "responseHeaders": {"Content-Type": "text/html", "X-Bao-Fulfill": "yes"},
            "body": "aGVsbG8="
        })),
    );
    let params = msg.params.clone();
    let resp = handle_command(msg, INTERCEPT_TARGET, &params, Some(&sender));
    assert!(
        resp.error.is_none(),
        "fulfillRequest must resolve the pause: {:?}",
        resp.error
    );
    // Assert ③ — 决议载荷逐字段保真(base64 体原样下行,解码归引擎半边)。
    match pump_popped(&captured) {
        BridgeCommand::FetchResolve { action, .. } => match action {
            FetchResolveAction::Fulfill {
                response_code,
                status_message,
                response_headers,
                body,
            } => {
                assert_eq!(response_code, 201);
                assert_eq!(status_message.as_deref(), Some("Created"));
                assert_eq!(
                    response_headers.as_ref().and_then(|h| h.get("Content-Type")),
                    Some(&json!("text/html"))
                );
                assert_eq!(
                    response_headers.as_ref().and_then(|h| h.get("X-Bao-Fulfill")),
                    Some(&json!("yes"))
                );
                assert_eq!(body.as_deref(), Some("aGVsbG8="));
            }
            other => panic!("expected FetchResolveAction::Fulfill, got {other:?}"),
        },
        other => panic!("expected BridgeCommand::FetchResolve, got {other:?}"),
    }

    // Assert ④ — §4 ③b:response tap 对合成响应照常发,status/headers 即
    // fulfill 值(与 Chromium「fulfill 后 responseReceived 携带合成响应」一致)。
    let mut synth_headers = HashMap::new();
    synth_headers.insert("Content-Type".to_string(), "text/html".to_string());
    subscriber.on_network_response(
        INTERCEPT_TARGET,
        "REQ-C",
        "https://api.example.com/v1/data",
        201,
        "Created",
        synth_headers,
        "text/html",
        None,
    );
    let responded = transport.recv_event().unwrap().expect("responseReceived");
    assert_eq!(responded.method, "Network.responseReceived");
    assert_eq!(responded.params["requestId"], "REQ-C");
    assert_eq!(
        responded.params["response"]["status"], 201,
        "合成响应 status = fulfill responseCode"
    );
    assert_eq!(responded.params["response"]["statusText"], "Created");
    assert_eq!(
        responded.params["response"]["headers"]["Content-Type"],
        "text/html",
        "合成响应 headers 携带 fulfill 注入头"
    );

    join_pump(stop, pump);
}

/// Fetch.disable 排空:pause 后 disable(命令面 = FetchDisable 到泵)→
/// 后续同 pattern 域请求直达完成,零 requestPaused(disable-is-the-exit,
/// 设计 §3.1:排空 pending = 全部 drop = continue)。
#[test]
fn fetch_interception_disable_drains_and_subsequent_requests_flow() {
    // @trace REQ-CDP-006 [domain:Fetch] [level:integration]
    let (sender, captured, stop, pump) = spawn_bridge_pump();
    let (mut transport, subscriber) = interception_event_chain();

    // Act ① — arm → pause(同 pattern 域)。
    let msg = intercept_cdp_msg(1, "Fetch.enable", None);
    let resp = handle_command(msg, INTERCEPT_TARGET, &None, Some(&sender));
    assert!(resp.error.is_none());
    let _ = pump_popped(&captured);
    subscriber
        .sender()
        .send(ServoEvent::FetchRequestPaused {
            target_id: INTERCEPT_TARGET.to_string(),
            request_id: "REQ-D1".to_string(),
            interception_id: "interception-4".to_string(),
            url: "https://api.example.com/v1/first".to_string(),
            method: "GET".to_string(),
            headers: HashMap::new(),
            resource_type: "Fetch".to_string(),
            is_redirect: false,
        })
        .expect("pause event push");
    let paused = transport.recv_event().unwrap().expect("requestPaused");
    assert_eq!(paused.method, "Fetch.requestPaused");

    // Act ② — disable:唯一解困出口,排空一切挂起(设计 §3.1)。
    let msg = intercept_cdp_msg(2, "Fetch.disable", None);
    let resp = handle_command(msg, INTERCEPT_TARGET, &None, Some(&sender));
    assert!(
        resp.error.is_none(),
        "Fetch.disable must clear the slot: {:?}",
        resp.error
    );
    match pump_popped(&captured) {
        BridgeCommand::FetchDisable { target_id } => {
            assert_eq!(target_id, INTERCEPT_TARGET);
        }
        other => panic!("expected BridgeCommand::FetchDisable, got {other:?}"),
    }

    // Act ③ — disable 后同 pattern 域的后续请求照常完成(零 pause)。
    subscriber.on_network_request(
        INTERCEPT_TARGET,
        "REQ-D2",
        "https://api.example.com/v1/second",
        "GET",
        HashMap::new(),
        None,
        "Fetch",
        "main-t-net",
    );
    subscriber.on_network_loading_finish(INTERCEPT_TARGET, "REQ-D2", 64);
    let first = transport.recv_event().unwrap().expect("post-disable event 1");
    let second = transport.recv_event().unwrap().expect("post-disable event 2");
    // Assert — 直达完成面,两个事件均非 requestPaused。
    for ev in [&first, &second] {
        assert_ne!(
            ev.method, "Fetch.requestPaused",
            "disable 后不得再发 pause 事件"
        );
    }
    assert_eq!(first.method, "Network.requestWillBeSent");
    assert_eq!(first.params["requestId"], "REQ-D2");
    assert_eq!(second.method, "Network.loadingFinished");
    assert_eq!(second.params["requestId"], "REQ-D2");

    join_pump(stop, pump);
}

/// 旧版 Network 拦截面(criterion 字面命名面):setRequestInterception /
/// continueInterceptedRequest 与 Fetch 设施同桥同槽(设计 §5),命中发
/// `Network.requestIntercepted`(POP6-C6 措辞载体)。
#[test]
fn fetch_interception_legacy_network_face_shares_facility() {
    // @trace REQ-CDP-006 [domain:Network] [level:integration]
    let (sender, captured, stop, pump) = spawn_bridge_pump();

    // Act ① — legacy arm(唯一合法 stage = Request)。
    let msg = intercept_cdp_msg(
        1,
        "Network.setRequestInterception",
        Some(json!({"patterns": [{"urlPattern": "*", "interceptionStage": "Request"}]})),
    );
    let params = msg.params.clone();
    let resp = handle_command(msg, INTERCEPT_TARGET, &params, Some(&sender));
    assert!(
        resp.error.is_none(),
        "legacy arm must route the shared facility: {:?}",
        resp.error
    );
    match pump_popped(&captured) {
        BridgeCommand::FetchEnable {
            target_id,
            patterns,
            legacy_network_events,
        } => {
            assert_eq!(target_id, INTERCEPT_TARGET);
            assert!(
                legacy_network_events,
                "legacy 面命中发 Network.requestIntercepted(criterion 字面)"
            );
            assert_eq!(patterns.len(), 1);
            assert_eq!(patterns[0].url_pattern.as_deref(), Some("*"));
            assert_eq!(patterns[0].request_stage, FetchRequestStage::Request);
        }
        other => panic!("expected BridgeCommand::FetchEnable, got {other:?}"),
    }

    // Act ② — legacy 命中事件。
    let (mut transport, subscriber) = interception_event_chain();
    subscriber
        .sender()
        .send(ServoEvent::NetworkRequestIntercepted {
            target_id: INTERCEPT_TARGET.to_string(),
            request_id: "REQ-E".to_string(),
            interception_id: "interception-5".to_string(),
            url: "https://legacy.example.com/x".to_string(),
            method: "GET".to_string(),
            headers: HashMap::new(),
            resource_type: "Document".to_string(),
            is_redirect: false,
        })
        .expect("intercepted event push");
    let intercepted = transport
        .recv_event()
        .unwrap()
        .expect("requestIntercepted");
    assert_eq!(intercepted.method, "Network.requestIntercepted");
    assert_eq!(intercepted.session_id.as_deref(), Some(INTERCEPT_TARGET));
    assert_eq!(
        intercepted.params["interceptionId"], "interception-5",
        "legacy 规范首参 = 决议键"
    );
    assert_eq!(
        intercepted.params["requestId"], "REQ-E",
        "bao 附加:与 requestWillBeSent 同源(供客户端关联)"
    );
    assert_eq!(intercepted.params["request"]["url"], "https://legacy.example.com/x");
    assert_eq!(intercepted.params["resourceType"], "Document");

    // Act ③ — legacy 决议与 Fetch 同一 dispatch(Continue 臂)。
    let msg = intercept_cdp_msg(
        2,
        "Network.continueInterceptedRequest",
        Some(json!({"interceptionId": "interception-5"})),
    );
    let params = msg.params.clone();
    let resp = handle_command(msg, INTERCEPT_TARGET, &params, Some(&sender));
    assert!(
        resp.error.is_none(),
        "legacy continue must resolve: {:?}",
        resp.error
    );
    match pump_popped(&captured) {
        BridgeCommand::FetchResolve { action, .. } => {
            assert!(matches!(action, FetchResolveAction::Continue));
        }
        other => panic!("expected BridgeCommand::FetchResolve, got {other:?}"),
    }

    join_pump(stop, pump);
}

/// 能力边界矩阵(设计 §7,诚实线):auth / 修改型 continue / 无 body 载体 /
/// 无桥连接,全部显式错误、码位合规,禁假成功。这些门都在 bridge_send 之前
/// 返回,无需泵。
#[test]
fn fetch_interception_capability_boundaries_are_explicit_errors() {
    // @trace REQ-CDP-006 [domain:Fetch] [level:integration]

    // 边界断言工具:错误码 + message 片段一次钉死。
    fn expect_boundary_error(
        id: i64,
        method: &str,
        params: Option<serde_json::Value>,
        code: i64,
        fragment: &str,
    ) {
        let msg = intercept_cdp_msg(id, method, params);
        let params = msg_params(&msg);
        let resp = handle_command(msg, INTERCEPT_TARGET, &params, None);
        let err = resp
            .error
            .unwrap_or_else(|| panic!("{method}: explicit error required"));
        assert_eq!(err.code, code, "{method}: wrong error code ({err:?})");
        assert!(
            err.message.contains(fragment),
            "{method}: message lacks {fragment:?}: {:?}",
            err.message
        );
    }

    // auth 拦截面未接线:enable 拒收 handleAuthRequests=true(-32602)。
    expect_boundary_error(
        1,
        "Fetch.enable",
        Some(json!({"handleAuthRequests": true})),
        -32602,
        "handleAuthRequests",
    );
    // 修改型 continue:servo 面只能放行或合成应答,不能重发重构请求(-32602)。
    expect_boundary_error(
        2,
        "Fetch.continueRequest",
        Some(json!({"interceptionId": "interception-9", "url": "https://evil.example.com/redirect"})),
        -32602,
        "modification is not supported",
    );
    // WebResourceRequest 无 body 字段:getRequestPostData 显式 -32000。
    expect_boundary_error(
        3,
        "Fetch.getRequestPostData",
        Some(json!({"requestId": "REQ-F"})),
        -32000,
        "no request body",
    );
    // auth 决议命令同面显式 -32000。
    expect_boundary_error(
        4,
        "Fetch.continueWithAuth",
        Some(json!({"interceptionId": "interception-9"})),
        -32000,
        "auth interception facet is not wired",
    );
    // 无桥连接(无 slot owner):enable 拒绝 -32603,禁 canned "enabled"。
    expect_boundary_error(5, "Fetch.enable", None, -32603, "no servo bridge connected");
}

/// handle_command 消费 msg 前取 params 的辅助(签名要求 msg 与 params 双传)。
fn msg_params(msg: &CdpMessage) -> Option<serde_json::Value> {
    msg.params.clone()
}
