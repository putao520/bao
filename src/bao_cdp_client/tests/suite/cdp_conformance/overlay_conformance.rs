//! Overlay domain conformance 审计 — REQ-CDP-007-C6(截图标注高亮)。
//!
//! 对照 CDP 官方规范(Overlay domain):
//! https://chromedevtools.github.io/devtools-protocol/tot/Overlay/
//!
//! # 覆盖面(与既有测试的边界)
//!
//! - `bao_cdp` protocol.rs 单测(protocol.rs:4180-4242)已覆盖 **no-bridge**
//!   形态(无 bridge → -32603 / 缺 node 参 → -32602 / setInspectMode →
//!   不支持)。
//! - 本文件覆盖 **生产通道 with-bridge** 形态:dispatch 经过真实
//!   `bao_cdp::servo_bridge::bridge_channel`,断言浏览器侧消费者
//!   (`bao_browser` cdp_handler)所匹配的 `BridgeCommand` 线契约——
//!   `OverlayHighlightNode` / `OverlayHideHighlight` 的字段形状、三形态
//!   node 身份折叠(nodeId / objectId / backendNodeId)、highlightSpec
//!   原样透传(bao_cdp 不解释颜色语义,绘制语义在 browser 绘制面)。
//!
//! # 真引擎像素验收
//!
//! highlight → Page.captureScreenshot → quad 内 spec 色 / 区域外不变、
//! hideHighlight → 复拍还原、导航后高亮失效(清槽钩子)——由 bao_browser
//! tests/suite 的 `overlay_conformance_tests` 承担(本 crate 无 bao_browser /
//! image 依赖,进程内无法构造真 servo 截图链;e2e_playwright_compat.rs §4
//! 同款 delegate 形态)。
//!
//! @trace REQ-CDP-007 [domain:Overlay] [level:integration]

use bao_cdp::{bridge_channel, handle_command, BridgeCommand, BridgeResponse, CdpMessage};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// ─────────────────────────────────────────────────────────────────────────
// 公共辅助 — 真实 bridge 通道 + 生产 dispatch 面
// ─────────────────────────────────────────────────────────────────────────

/// Bridge-capture fixture(与 protocol.rs REQ-CDP-006 测试的
/// `capture_bridge_command` 同形):responder 线程应答 `{}` 并捕获 handler
/// 实际发送的 BridgeCommand;dispatch 线程拿到的是真实回程 BridgeResponse。
fn dispatch_with_bridge(method: &str, params: Value) -> (bao_cdp::CdpResponse, BridgeCommand) {
    let (bridge, receiver) = bridge_channel(Duration::from_secs(5));
    let captured: Arc<Mutex<Option<BridgeCommand>>> = Arc::default();
    let sink = Arc::clone(&captured);
    let responder = std::thread::spawn(move || {
        receiver.recv_and_process(Duration::from_secs(5), |cmd| {
            *sink.lock().unwrap() = Some(cmd);
            BridgeResponse {
                result: Ok(json!({})),
            }
        });
    });
    let msg = CdpMessage {
        id: Some(1),
        method: method.to_string(),
        params: Some(params),
        session_id: None,
    };
    let forwarded = msg.params.clone();
    let resp = handle_command(msg, "t1", &forwarded, Some(&bridge));
    let cmd = captured
        .lock()
        .unwrap()
        .clone()
        .expect("handler should have sent exactly one bridge command");
    responder.join().unwrap();
    (resp, cmd)
}

/// Success ack shape for state/lifecycle commands: empty result object,
/// no error (CDP: these commands carry no result payload).
fn assert_ok_empty(resp: &bao_cdp::CdpResponse, method: &str) {
    assert!(
        resp.error.is_none(),
        "{method} must succeed, got: {:?}",
        resp.error
    );
    assert_eq!(
        resp.result.as_ref(),
        Some(&json!({})),
        "{method} returns the empty result object"
    );
}

// ─────────────────────────────────────────────────────────────────────────
// Overlay.highlightNode — BridgeCommand 线契约(三形态 node 身份)
// ─────────────────────────────────────────────────────────────────────────

#[test]
// @trace REQ-CDP-007 [domain:Overlay] [level:integration]
fn overlay_highlight_node_node_id_form_reaches_bridge() {
    // Arrange — CDP spec: Overlay.highlightNode{nodeId, highlightSpec?}
    let (resp, cmd) = dispatch_with_bridge(
        "Overlay.highlightNode",
        json!({"nodeId": 5, "highlightSpec": {"contentColor": {"r": 1, "g": 0, "b": 0, "a": 1}}}),
    );

    // Assert — success ack on the dispatch face.
    assert_ok_empty(&resp, "Overlay.highlightNode");
    // The browser-side consumer matches this exact shape
    // (bao_browser cdp_handler BridgeCommand::OverlayHighlightNode arm).
    match cmd {
        BridgeCommand::OverlayHighlightNode {
            target_id,
            node_id,
            object_id,
            highlight_spec,
        } => {
            assert_eq!(target_id, "t1", "target rides to the browser layer");
            assert_eq!(node_id, Some(5), "nodeId form");
            assert_eq!(object_id, None, "no objectId on the nodeId form");
            assert_eq!(
                highlight_spec,
                Some(json!({"contentColor": {"r": 1, "g": 0, "b": 0, "a": 1}})),
                "highlightSpec rides as raw JSON (paint semantics live on the browser face)"
            );
        }
        other => panic!("expected OverlayHighlightNode, got: {other:?}"),
    }
}

#[test]
// @trace REQ-CDP-007 [domain:Overlay] [level:integration]
fn overlay_highlight_node_object_id_form_reaches_bridge() {
    // Arrange — objectId form (RemoteObject handle from a returnByValue:false
    // evaluate; DOM.getNodeForOwner documents the same identity space).
    let (_, cmd) = dispatch_with_bridge("Overlay.highlightNode", json!({"objectId": "1:3"}));

    // Assert
    match cmd {
        BridgeCommand::OverlayHighlightNode {
            node_id,
            object_id,
            highlight_spec,
            ..
        } => {
            assert_eq!(node_id, None, "objectId form carries no nodeId");
            assert_eq!(object_id.as_deref(), Some("1:3"));
            assert_eq!(
                highlight_spec, None,
                "absent spec stays absent (defaults applied browser-side)"
            );
        }
        other => panic!("expected OverlayHighlightNode, got: {other:?}"),
    }
}

#[test]
// @trace REQ-CDP-007 [domain:Overlay] [level:integration]
fn overlay_highlight_node_backend_node_id_folds_to_canonical() {
    // Arrange — backendNodeId folds into the canonical positional id
    // (DOM.resolveNode documented identity: backend ≡ canonical on this face).
    let (_, cmd) = dispatch_with_bridge("Overlay.highlightNode", json!({"backendNodeId": 7}));

    // Assert
    match cmd {
        BridgeCommand::OverlayHighlightNode {
            node_id, object_id, ..
        } => {
            assert_eq!(node_id, Some(7), "backendNodeId ≡ canonical nodeId");
            assert_eq!(object_id, None);
        }
        other => panic!("expected OverlayHighlightNode, got: {other:?}"),
    }
}

#[test]
// @trace REQ-CDP-007 [domain:Overlay] [level:integration]
fn overlay_highlight_node_full_spec_rides_verbatim() {
    // Arrange — the full four-layer Overlay.HighlightSpec (CDP field names:
    // contentColor / paddingColor / borderColor / marginColor; per-layer
    // DOM.RGBA). bao_cdp must not rewrite or drop any layer.
    let spec = json!({
        "contentColor": {"r": 1, "g": 0, "b": 0, "a": 1},
        "paddingColor": {"r": 0, "g": 1, "b": 0, "a": 0.55},
        "borderColor": {"r": 1, "g": 1, "b": 0, "a": 0.66},
        "marginColor": {"r": 0, "g": 0, "b": 1}
    });
    let (_, cmd) = dispatch_with_bridge(
        "Overlay.highlightNode",
        json!({"nodeId": 2, "highlightSpec": spec}),
    );

    // Assert — verbatim round-trip (including the implicit-alpha layer:
    // DOM.RGBA `a` is optional per spec; interpretation is the browser face).
    match cmd {
        BridgeCommand::OverlayHighlightNode { highlight_spec, .. } => {
            assert_eq!(
                highlight_spec.as_ref(),
                Some(&spec),
                "spec must not be rewritten"
            );
        }
        other => panic!("expected OverlayHighlightNode, got: {other:?}"),
    }
}

#[test]
// @trace REQ-CDP-007 [domain:Overlay] [level:integration]
fn overlay_highlight_node_both_forms_node_id_wins() {
    // Arrange — nodeId + objectId both present: nodeId wins (the same
    // tie-break the DOM face keeps).
    let (_, cmd) = dispatch_with_bridge(
        "Overlay.highlightNode",
        json!({"nodeId": 9, "objectId": "1:1"}),
    );

    // Assert
    match cmd {
        BridgeCommand::OverlayHighlightNode {
            node_id, object_id, ..
        } => {
            assert_eq!(node_id, Some(9));
            assert_eq!(object_id, None, "the losing form is not carried");
        }
        other => panic!("expected OverlayHighlightNode, got: {other:?}"),
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Overlay.hideHighlight / enable / disable — 状态命令与域生命周期
// ─────────────────────────────────────────────────────────────────────────

#[test]
// @trace REQ-CDP-007 [domain:Overlay] [level:integration]
fn overlay_hide_highlight_reaches_bridge() {
    // Arrange — CDP spec: Overlay.hideHighlight() clears the highlight.
    let (resp, cmd) = dispatch_with_bridge("Overlay.hideHighlight", json!({}));

    // Assert — a real state command on the browser layer (slot clear),
    // never a shape-only ack.
    assert_ok_empty(&resp, "Overlay.hideHighlight");
    match cmd {
        BridgeCommand::OverlayHideHighlight { target_id } => {
            assert_eq!(target_id, "t1");
        }
        other => panic!("expected OverlayHideHighlight, got: {other:?}"),
    }
}

#[test]
// @trace REQ-CDP-007 [domain:Overlay] [level:integration]
fn overlay_enable_disable_are_lifecycle_noops_without_bridge_traffic() {
    // Arrange — CDP spec: Overlay.enable/disable are domain lifecycle with
    // no side-effect face here; conformance = `{}` empty-result acks and NO
    // bridge command (unlike highlightNode / hideHighlight, which are real
    // browser-layer state commands).
    for method in ["Overlay.enable", "Overlay.disable"] {
        let (bridge, receiver) = bridge_channel(Duration::from_secs(5));
        let msg = CdpMessage {
            id: Some(1),
            method: method.to_string(),
            params: Some(json!({})),
            session_id: None,
        };
        let forwarded = msg.params.clone();
        let resp = handle_command(msg, "t1", &forwarded, Some(&bridge));

        // Assert — empty-object ack, no error.
        assert_ok_empty(&resp, method);
        // The channel stays empty: lifecycle commands carry no browser-side
        // state transition on this face.
        let got = receiver.recv_and_process(Duration::from_millis(200), |_| BridgeResponse {
            result: Ok(json!({})),
        });
        assert!(
            !got,
            "{method} must not send a bridge command (channel must stay empty)"
        );
    }
}

// ─────────────────────────────────────────────────────────────────────────
// 错误码 conformance(客户端可见的显式失败面)
// ─────────────────────────────────────────────────────────────────────────

#[test]
// @trace REQ-CDP-007 [domain:Overlay] [level:integration]
fn overlay_missing_node_ref_is_invalid_params_32602() {
    // Arrange — none of the three accepted shapes present → -32602
    // (fires before the bridge check: no channel needed).
    let msg = CdpMessage {
        id: Some(1),
        method: "Overlay.highlightNode".to_string(),
        params: Some(json!({"highlightSpec": {"contentColor": {"r": 1}}})),
        session_id: None,
    };
    let forwarded = msg.params.clone();

    // Act
    let resp = handle_command(msg, "t1", &forwarded, None);

    // Assert
    let err = resp.error.expect("explicit error required");
    assert_eq!(err.code, -32602, "CDP / JSON-RPC: missing required param");
    assert!(
        err.message.contains("nodeId, objectId or backendNodeId"),
        "error names all three accepted shapes: {}",
        err.message
    );
}

#[test]
// @trace REQ-CDP-007 [domain:Overlay] [level:integration]
fn overlay_set_inspect_mode_is_explicit_unsupported_32000() {
    // Arrange — inspect mode needs the hover→Overlay.nodeHighlightRequested
    // event chain which does not exist on this face; a canned ok would be a
    // fake success (design §6 boundary matrix).
    let msg = CdpMessage {
        id: Some(1),
        method: "Overlay.setInspectMode".to_string(),
        params: Some(json!({"mode": "searchForNode"})),
        session_id: None,
    };
    let forwarded = msg.params.clone();

    // Act
    let resp = handle_command(msg, "t1", &forwarded, None);

    // Assert
    let err = resp.error.expect("explicit unsupported required");
    assert_eq!(err.code, -32000, "CDP server-error class");
    assert!(err.message.contains("nodeHighlightRequested"));
}

#[test]
// @trace REQ-CDP-007 [domain:Overlay] [level:integration]
fn overlay_unknown_subcommand_is_method_not_found_32601() {
    // Arrange — CDP / JSON-RPC: unknown method → -32601.
    let msg = CdpMessage {
        id: Some(1),
        method: "Overlay.setShowHinge".to_string(),
        params: Some(json!({})),
        session_id: None,
    };
    let forwarded = msg.params.clone();

    // Act
    let resp = handle_command(msg, "t1", &forwarded, None);

    // Assert
    let err = resp.error.expect("unknown subcommand must fail");
    assert_eq!(err.code, -32601);
    assert!(err.message.contains("Overlay.setShowHinge"));
}

// ─────────────────────────────────────────────────────────────────────────
// 像素验收委派锚 — 真引擎承担者存在性守护
// ─────────────────────────────────────────────────────────────────────────

/// Delegate 形态存在性断言(e2e_playwright_compat.rs §4 同款惯形):确认
/// 被指派的真引擎像素验收承担者仍在场。
///
/// 本 crate 无 bao_browser / image 依赖,进程内无法构造真 servo 截图链;
/// highlight→captureScreenshot→quad 像素断言 / hideHighlight 复拍还原 /
/// 导航失效(清槽钩子)由 bao_browser tests/suite 的承担者完整覆盖。
///
/// @trace REQ-CDP-007 [domain:Overlay] [level:integration]
#[test]
fn overlay_pixel_acceptance_delegate_bearer_present() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../bao_browser/tests/suite/overlay_conformance_tests.rs");
    let src = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("delegate bearer missing: {} ({e})", path.display()));
    assert!(
        src.contains("fn overlay_pixel_acceptance_full_chain"),
        "delegate bearer {} no longer defines the pixel-acceptance chain — re-audit the delegation",
        path.display()
    );
}
