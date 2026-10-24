// REQ-CDP-008 Target Domain event-face pins: the auto-attach and
// created/destroyed notification paths of `BaoWsRegistry`
// (`src/bao_browser/src/ws_registry.rs`).
//
// Coverage audit (.plans/coverage-audit-cdp-20261014.md GAP G2/G6, P2):
// `emit_target_created_attached` and `emit_target_closed_events` were
// implemented but had ZERO test触达 — a regression in either (Chrome close
// semantics, puppeteer W40 S11; auto-attach, Playwright
// context.new_page()) would have been silent. These pins exercise the two
// functions through the real `RegistryDispatch::dispatch_message` surface
// (the same path the CdpServer drives), with a scripted bridge responder
// answering `Target.createTarget` / `Target.closeTarget` — the two commands
// these paths are hung off.
//
// Scope: pure test additions (no src/ implementation change).
// @trace REQ-CDP-008 [req:REQ-CDP-008] [level:unit]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use bao_browser::BaoWsRegistry;
use bao_cdp::servo_bridge::{bridge_channel, BridgeCommand, BridgeResponse};
use cdp_server::{CdpMessage, EventSender, RegistryDispatch};
use serde_json::{json, Value};

/// Event sender that records each delivery face separately: untagged
/// broadcasts (`send_event` — the browser-endpoint stream the closed/destroyed
/// notifications and browser-level attachedToTarget arrive on) and
/// session-tagged events (`send_session_event` — the per-flattened-session
/// stream). The lifecycle notifications this file pins are UNTAGGED by
/// design (Chrome shape, ws_registry emit_target_closed_events), so the two
/// faces must be asserted independently.
#[derive(Default)]
struct RecordingSender {
    untagged: Mutex<Vec<(String, Value)>>,
    tagged: Mutex<Vec<(String, String, Value)>>,
}

impl RecordingSender {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn untagged(&self) -> Vec<(String, Value)> {
        self.untagged.lock().unwrap().clone()
    }

    fn untagged_methods(&self) -> Vec<String> {
        self.untagged()
            .into_iter()
            .map(|(m, _)| m)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect()
    }
}

impl EventSender for RecordingSender {
    fn send_event(&self, method: &str, params: Value) {
        self.untagged
            .lock()
            .unwrap()
            .push((method.to_string(), params));
    }

    fn send_session_event(&self, session_id: &str, method: &str, params: Value) {
        self.tagged
            .lock()
            .unwrap()
            .push((session_id.to_string(), method.to_string(), params));
    }
}

/// Scripted bridge responder standing in for the main-thread PagePool drain:
/// answers the two commands the pinned paths hang off, exactly like the real
/// `cmd_create_target` / `cmd_close_page` answers
/// (`src/bao_browser/src/cdp_handler.rs`):
/// - `CreateTarget` → `{"targetId": "<fresh id>"}` (the real handler returns
///   the genuinely new PageId — here a fixed id is fine: the registry treats
///   it as the created target's identity);
/// - `ClosePage` → `Ok({})` (the real close succeeds).
/// Every other command answers `Ok({})` (idle-pool default shape).
fn scripted_responder(rx: bao_cdp::servo_bridge::BridgeReceiver) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || loop {
        let handled = rx.try_process(|cmd| match cmd {
            BridgeCommand::CreateTarget { .. } => BridgeResponse {
                result: Ok(json!({ "targetId": "1" })),
            },
            BridgeCommand::ClosePage { .. } => BridgeResponse {
                result: Ok(json!({ "success": true })),
            },
            _ => BridgeResponse {
                result: Ok(json!({})),
            },
        });
        if !handled {
            std::thread::sleep(Duration::from_millis(1));
        }
    })
}

fn browser_msg(method: &str, params: Value) -> CdpMessage {
    CdpMessage {
        id: Some(1),
        method: method.to_string(),
        params: Some(params),
        session_id: None,
    }
}

const BROWSER_TARGET: &str = "__browser__";

fn dispatch(
    registry: &BaoWsRegistry,
    msg: &CdpMessage,
    sender: &RecordingSender,
) -> Result<Value, cdp_server::CdpError> {
    registry
        .dispatch_message(msg, BROWSER_TARGET, sender)
        .expect("Target domain is served by the registry")
}

/// C3: `Target.setAutoAttach` 自动附加新 target — after the browser session
/// enables auto-attach, a subsequently created target
/// (`Target.createTarget`) emits `Target.attachedToTarget` carrying a minted,
/// routable flattened sessionId. The companion in-src test
/// (`set_auto_attach_emits_attached_to_target_for_existing_pages`) pins the
/// existing-page arm of the same path; this pins the NEW-target arm
/// (`emit_target_created_attached`), which Playwright's
/// context.new_page() depends on.
// @trace TEST-CDP-8 [req:REQ-CDP-008] [level:unit]
#[test]
fn set_auto_attach_then_create_target_emits_attached_to_target() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    let _keeper = scripted_responder(rx);
    let registry = BaoWsRegistry::new(tx);
    let sender = RecordingSender::new();

    // Browser session enables auto-attach (flatten=true — the mode clients use).
    let r = dispatch(
        &registry,
        &browser_msg(
            "Target.setAutoAttach",
            json!({ "autoAttach": true, "flatten": true }),
        ),
        &sender,
    )
    .expect("setAutoAttach must succeed");
    assert_eq!(r, json!({}), "setAutoAttach answers empty ok");
    // The setAutoAttach round itself must not fabricate attached events for
    // pages that do not exist (the scripted bridge lists no pages — the
    // existing-page arm is pinned by the in-src test).
    assert!(
        !sender.untagged_methods().contains(&"Target.attachedToTarget".to_string()),
        "setAutoAttach alone must not emit attachedToTarget without listed pages"
    );

    // Create a new target through the real dispatch face.
    let r = dispatch(
        &registry,
        &browser_msg("Target.createTarget", json!({ "url": "about:blank" })),
        &sender,
    )
    .expect("createTarget must succeed");
    let new_target = r["targetId"].as_str().expect("created targetId").to_string();
    assert_eq!(new_target, "1", "the bridge-created target id is the truth");

    // The auto-attach event fires on the browser-endpoint (untagged) stream,
    // Chrome shape: exactly one, carrying the NEW target id + a minted session.
    let attached: Vec<(String, Value)> = sender
        .untagged()
        .into_iter()
        .filter(|(m, _)| m == "Target.attachedToTarget")
        .collect();
    assert_eq!(
        attached.len(),
        1,
        "exactly one attachedToTarget for the created target; got: {:?}",
        sender.untagged()
    );
    let params = &attached[0].1;
    assert_eq!(params["targetInfo"]["targetId"], new_target);
    assert_eq!(params["targetInfo"]["type"], "page");
    assert_eq!(params["targetInfo"]["attached"], true);
    let session_id = params["sessionId"].as_str().expect("minted sessionId").to_string();

    // The minted session is really routable — a flattened command carrying it
    // must reach the new target (proves the mint is wired into the routing
    // table, not just an id echoed into the event).
    let r = dispatch(
        &registry,
        &CdpMessage {
            id: Some(2),
            method: "Page.enable".into(),
            params: Some(json!({})),
            session_id: Some(session_id.clone()),
        },
        &sender,
    )
    .expect("flattened command on the minted session must dispatch");
    assert!(r.is_object(), "session-routed command answers, got: {r:?}");
    // The routed command must NOT have been treated as a browser-target miss:
    // if the session were unknown the dispatch would fail with -32001.
}

/// C4 (positive): `Target.closeTarget` 关闭语义 — closing a target emits
/// `Target.targetDestroyed` on the browser stream (to everyone who saw the
/// target) and one `Target.detachedFromTarget` per attached flattened session
/// (Chrome shape: untagged, params naming the closed session). The puppeteer
/// W40 S11 page.close() stall pin: without the detach signal the client
/// resolves page.close() on the session-detach notification and hangs forever.
// @trace TEST-CDP-8 [req:REQ-CDP-008] [level:unit]
#[test]
fn close_target_emits_target_destroyed_and_detached_events() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    let _keeper = scripted_responder(rx);
    let registry = BaoWsRegistry::new(tx);
    let sender = RecordingSender::new();

    // Two flattened sessions attached to the same target, one to another —
    // the detach notification must fire per SESSION bound to the closed
    // target only.
    let sid_a = dispatch(
        &registry,
        &browser_msg(
            "Target.attachToTarget",
            json!({ "targetId": "1", "flatten": true }),
        ),
        &sender,
    )
    .expect("attach a")["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    let sid_b = dispatch(
        &registry,
        &browser_msg(
            "Target.attachToTarget",
            json!({ "targetId": "1", "flatten": true }),
        ),
        &sender,
    )
    .expect("attach b")["sessionId"]
        .as_str()
        .unwrap()
        .to_string();
    let sid_other = dispatch(
        &registry,
        &browser_msg(
            "Target.attachToTarget",
            json!({ "targetId": "9", "flatten": true }),
        ),
        &sender,
    )
    .expect("attach other")["sessionId"]
        .as_str()
        .unwrap()
        .to_string();

    // Close the target through the real dispatch face (bridge answers Ok).
    let r = dispatch(
        &registry,
        &browser_msg("Target.closeTarget", json!({ "targetId": "1" })),
        &sender,
    )
    .expect("closeTarget must succeed");
    assert_eq!(r["success"], true);

    // targetDestroyed: one, untagged, naming the closed target.
    let destroyed: Vec<(String, Value)> = sender
        .untagged()
        .into_iter()
        .filter(|(m, _)| m == "Target.targetDestroyed")
        .collect();
    assert_eq!(destroyed.len(), 1, "one targetDestroyed; got: {:?}", sender.untagged());
    assert_eq!(destroyed[0].1["targetId"], "1");

    // detachedFromTarget: one per session bound to the closed target
    // (sid_a, sid_b) — untagged on the parent stream, params naming the
    // session; the other target's session (sid_other) gets none.
    let detached: Vec<(String, Value)> = sender
        .untagged()
        .into_iter()
        .filter(|(m, _)| m == "Target.detachedFromTarget")
        .collect();
    assert_eq!(
        detached.len(),
        2,
        "one detach per session bound to the closed target; got: {:?}",
        sender.untagged()
    );
    let detached_sessions: Vec<&str> = detached
        .iter()
        .map(|(_, p)| p["sessionId"].as_str().unwrap())
        .collect();
    assert!(detached_sessions.contains(&sid_a.as_str()));
    assert!(detached_sessions.contains(&sid_b.as_str()));
    for (_, p) in &detached {
        assert_eq!(p["targetId"], "1", "detach names the closed target");
    }
    assert!(
        !detached_sessions.contains(&sid_other.as_str()),
        "another target's session must not be detached"
    );

    // The routing table was purged: a command on either closed session now
    // fails explicitly with the unknown-session error (the detached session
    // is gone, not silently routable).
    for sid in [&sid_a, &sid_b] {
        let err = dispatch(
            &registry,
            &CdpMessage {
                id: Some(3),
                method: "Page.enable".into(),
                params: Some(json!({})),
                session_id: Some(sid.clone()),
            },
            &sender,
        )
        .expect_err("detached session must no longer route");
        assert_eq!(err.code, -32001, "session not found after detach");
    }
    // The untouched target's session still routes.
    dispatch(
        &registry,
        &CdpMessage {
            id: Some(4),
            method: "Page.enable".into(),
            params: Some(json!({})),
            session_id: Some(sid_other),
        },
        &sender,
    )
    .expect("other target's session must still route");
}

/// C4 (negative guard): the lifecycle notifications fire for a real
/// closeTarget even when auto-attach was never enabled — close semantics are
/// independent of auto-attach (Chrome shape). And when auto-attach IS off, a
/// created target emits NO attachedToTarget (the C3 arm stays silent).
// @trace TEST-CDP-8 [req:REQ-CDP-008] [level:unit]
#[test]
fn target_lifecycle_events_respect_auto_attach_gate() {
    let (tx, rx) = bridge_channel(Duration::from_secs(2));
    let _keeper = scripted_responder(rx);
    let registry = BaoWsRegistry::new(tx);
    let sender = RecordingSender::new();

    // Auto-attach OFF (default): createTarget answers, but no attached event.
    let r = dispatch(
        &registry,
        &browser_msg("Target.createTarget", json!({ "url": "about:blank" })),
        &sender,
    )
    .expect("createTarget must succeed without auto-attach");
    assert!(r["targetId"].is_string());
    assert!(
        !sender.untagged_methods().contains(&"Target.attachedToTarget".to_string()),
        "no attachedToTarget when auto-attach is off; got: {:?}",
        sender.untagged()
    );

    // Close semantics are NOT gated by auto-attach: targetDestroyed fires.
    let r = dispatch(
        &registry,
        &browser_msg("Target.closeTarget", json!({ "targetId": "1" })),
        &sender,
    )
    .expect("closeTarget must succeed");
    assert_eq!(r["success"], true);
    assert!(
        sender.untagged_methods().contains(&"Target.targetDestroyed".to_string()),
        "targetDestroyed must fire even with auto-attach off; got: {:?}",
        sender.untagged()
    );
}
