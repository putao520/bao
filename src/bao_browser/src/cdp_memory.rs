//! Production `InMemoryBridge` — the memory:// CDP transport's host side.
//!
//! `Browser::connect("memory://bao")` (eager form, via the client's
//! process-global registry) dispatches every CDP command here. This bridge
//! routes through the REAL protocol dispatcher (`bao_cdp::handle_command`)
//! with a REAL `BridgeSender`, so:
//!
//! - Pure-protocol domains (`Browser.getVersion`, …) answer instantly.
//! - Servo-touching commands (`Runtime.evaluate`, `Target.getTargets`
//!   listing, …) ride the bridge channel to whoever drains it — the
//!   runtime's event loop (`BrowserRuntime::run`) drains it on its own thread.
//!   When nothing drains (a bare `BrowserRuntime::new` consumer that never
//!   pumps), those commands fail FAST with an honest timeout error (the
//!   channel is created with a short timeout) instead of returning
//!   fabricated results — the bridge-less protocol fallback fabricates
//!   `undefined` for `Runtime.evaluate`, which is exactly the silent-fake
//!   class this workspace eradicates.
//!
//! @trace REQ-CDP-001 [level:library]

use std::sync::Arc;

use bao_cdp::servo_bridge::{bridge_channel, BridgeSender};
use bao_cdp::{handle_command, CdpMessage};
use bao_cdp_client::bridge::{BridgeSenderBackend, CDPRdpBridge};
use bao_cdp_client::transport::in_memory::{InMemoryBridge, InMemoryBridgeResponse};

/// How long an undrained bridge command waits before failing. Short by
/// design: the documented no-pump consumer shape (`connect` → `version()` /
/// `pages()`) must not hang; servo-routed commands degrade to honest
/// errors, and `run()`-driven consumers get full fidelity.
const UNDRAINED_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// The host-side bridge installed into `bao_cdp_client`'s process registry
/// by [`crate::BrowserRuntime::new`].
pub struct MemoryCdpBridge {
    sender: BridgeSender,
    /// Target used when the client sends no sessionId (flat/single-target
    /// memory clients — the `memory://bao` shape has no discovery step).
    /// Tracks the runtime's most recently created page so flat clients
    /// (no Target.attachTarget dance) land on a live page.
    default_target: std::sync::Mutex<String>,
    /// M1 wiring (REQ-CDP-001, user ruling "留并接线"): the parallel-universe
    /// dispatcher (`bao_cdp_client::bridge`, e152 audit DUP-CDP-PARALLEL)
    /// mounted as the -32601 fallback arm. Its backend is the production
    /// channel itself ([`BridgeSenderBackend`] over the SAME sender), so
    /// every method it serves terminates in the single servo truth — no
    /// second command core, no second backend.
    rdp: CDPRdpBridge,
}

impl MemoryCdpBridge {
    /// Create the bridge pair: the sender side for the client registry, and
    /// the receiver the runtime must drain (`BrowserRuntime::run` does).
    pub fn new(
        default_target: impl Into<String>,
    ) -> (Arc<Self>, bao_cdp::servo_bridge::BridgeReceiver) {
        let (sender, receiver) = bridge_channel(UNDRAINED_TIMEOUT);
        (Self::new_with_sender(default_target, sender), receiver)
    }

    /// Point the flat (sessionId-less) client face at a live page. Called by
    /// [`crate::BrowserRuntime::create_page`] so `memory://` clients without an
    /// explicit target route to the newest page.
    pub fn set_default_target(&self, target: impl Into<String>) {
        *self.default_target.lock().unwrap() = target.into();
    }

    /// Build over an EXISTING channel pair (host-managed drain loop — the
    /// test face and embedders that already own a receiver). The M1 fallback
    /// dispatcher shares this same sender: one channel, one servo truth.
    ///
    /// @trace REQ-CDP-001 [level:library]
    pub fn new_with_sender(default_target: impl Into<String>, sender: BridgeSender) -> Arc<Self> {
        Arc::new(Self {
            rdp: CDPRdpBridge::new(Arc::new(BridgeSenderBackend::new(sender.clone()))),
            sender,
            default_target: std::sync::Mutex::new(default_target.into()),
        })
    }
}

impl InMemoryBridge for MemoryCdpBridge {
    fn dispatch_command(
        &self,
        method: &str,
        params: serde_json::Value,
        session_id: Option<&str>,
    ) -> InMemoryBridgeResponse {
        let owned_default = self.default_target.lock().unwrap().clone();
        let target = session_id.unwrap_or(&owned_default);
        // REQ-CDP-009: the screencast trio is served by bao_browser's own
        // frame-production domain BEFORE the bao_cdp dispatch (which has no
        // screencast handler) — registration touches only process-global
        // plain state, so this dispatch-thread call never reaches servo.
        // @trace REQ-CDP-009 [level:library]
        if let Some(response) = crate::screencast::dispatch_memory_command(method, &params, target)
        {
            return response;
        }
        let msg = CdpMessage {
            id: Some(0),
            method: method.to_string(),
            params: Some(params),
            session_id: None,
        };
        let params_ref = msg.params.clone();
        let response = handle_command(msg, target, &params_ref, Some(&self.sender));
        match response.error {
            // M1 wiring (REQ-CDP-001): method-not-found is the ONLY fall-
            // through code. The parallel universe serves what the production
            // core doesn't know (B-class Playwright surface: Page.title /
            // ElementHandle.* / JSHandle.* ...); explicit -32000
            // not-supported verdicts stay authoritative (no shadowing).
            Some(err) if err.code == -32601 => {
                let fallback_params = params_ref.clone().unwrap_or(serde_json::Value::Null);
                match self.rdp.dispatch(target, method, fallback_params) {
                    Ok(v) => InMemoryBridgeResponse::Ok(v),
                    // The fallback didn't know it either — report the
                    // PRODUCTION error (Chrome-shaped "'X.y' wasn't found"
                    // message), not the universe's internal miss.
                    Err(
                        bao_cdp_client::bridge::BridgeError::MethodNotFound(_)
                        | bao_cdp_client::bridge::BridgeError::InvalidMethod(_),
                    ) => InMemoryBridgeResponse::Err(err.message),
                    // The universe knows the method but the channel failed it
                    // (E-class NotSupported / servo error) — that verdict is
                    // more specific than the production miss; report it.
                    Err(e) => InMemoryBridgeResponse::Err(e.message()),
                }
            }
            Some(err) => InMemoryBridgeResponse::Err(err.message),
            None => InMemoryBridgeResponse::Ok(response.result.unwrap_or(serde_json::Value::Null)),
        }
    }
}
