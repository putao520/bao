// @trace REQ-BRW-001 [entity:BrowserContext]  REQ-CDP-006: Servo delegate hooks for CDP event forwarding
// @trace REQ-BRW-004 [entity:Worker] [entity:DedicatedWorkerGlobalScope] Worker lifecycle + DedicatedWorkerGlobalScope API
// @trace REQ-BRW-004 [entity:SharedWorker] [entity:SharedWorkerGlobalScope] SharedWorker cross-page routing + connect event
// @trace REQ-BRW-004 [entity:ServiceWorker] [entity:ServiceWorkerGlobalScope] ServiceWorker registration + fetch interception + stealth/CDP boundary consistency
// @trace REQ-CDP-006 [entity:ServoDelegateHooks] (servo delegate → CDP event forwarding)
// @trace REQ-CDP-010 [entity:RuntimeBinding] [entity:DomContentEvent] page-realm CDP marker carrier + parser

use bao_cdp_client::bridge::ServoEvent;

mod service_worker;
mod servo_delegate;
mod shared_worker;
mod webview_delegate;
mod webview_state;
mod worker_channel;
mod worker_lifecycle;
mod worker_scope;
mod worker_script;

pub use service_worker::*;
pub use servo_delegate::*;
pub use shared_worker::*;
pub use webview_delegate::*;
pub use webview_state::*;
pub use worker_channel::*;
pub use worker_lifecycle::*;
pub use worker_scope::*;
pub use worker_script::*;

/// Parse the page-realm `__BAO_CDPEVT__` carrier into a structured
/// [`ServoEvent`] bound to `target_id` (REQ-CDP-010: `Runtime.bindingCalled`
/// + `Page.domContentEventFired` observability — the engine surface has no
/// native embedder hook for either).
///
/// Carrier format: `__BAO_CDPEVT__<Json>` where Json =
/// `{"kind":"bindingCalled", "name":"<binding>", "payload":"<arg0>"}` or
/// `{"kind":"domContentLoaded"}`. `None` for every non-carrier string (the
/// normal console flow) and for malformed carriers — a malformed carrier is
/// dropped, never parsed as a log line.
///
/// @trace REQ-CDP-010 [entity:RuntimeBinding] [entity:DomContentEvent]
pub(crate) fn parse_cdp_marker_event(message: &str, target_id: String) -> Option<ServoEvent> {
    let body = message.strip_prefix(CDP_EVT_MARKER)?;
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    match v.get("kind").and_then(|k| k.as_str())? {
        "bindingCalled" => Some(ServoEvent::BindingCalled { // @evidence SPEC/10-REQUIREMENTS.html:2324
            target_id,
            name: v.get("name").and_then(|p| p.as_str()).unwrap_or("").to_string(),
            payload: v
                .get("payload")
                .and_then(|p| p.as_str())
                .unwrap_or("")
                .to_string(),
        }),
        "domContentLoaded" => Some(ServoEvent::DomContentLoaded { target_id }), // @evidence SPEC/10-REQUIREMENTS.html:2324
        _ => None, // @evidence SPEC/10-REQUIREMENTS.html:2324
    }
}

/// The page-realm → embedder CDP event carrier prefix (REQ-CDP-010).
/// Distinct from the legacy `__BAO_EVT__` Debugger transport (which stays
/// the console-transport face); the modern route consumes only this marker.
pub(crate) const CDP_EVT_MARKER: &str = "__BAO_CDPEVT__";
