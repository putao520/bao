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
//!   runtime's event loop (`BaoRuntime::run`) drains it on its own thread.
//!   When nothing drains (a bare `BaoRuntime::new` consumer that never
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
use bao_cdp_client::transport::in_memory::{InMemoryBridge, InMemoryBridgeResponse};

/// How long an undrained bridge command waits before failing. Short by
/// design: the documented no-pump consumer shape (`connect` → `version()` /
/// `pages()`) must not hang; servo-routed commands degrade to honest
/// errors, and `run()`-driven consumers get full fidelity.
const UNDRAINED_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// The host-side bridge installed into `bao_cdp_client`'s process registry
/// by [`crate::BaoRuntime::new`].
pub struct MemoryCdpBridge {
    sender: BridgeSender,
    /// Target used when the client sends no sessionId (flat/single-target
    /// memory clients — the `memory://bao` shape has no discovery step).
    /// Tracks the runtime's most recently created page so flat clients
    /// (no Target.attachTarget dance) land on a live page.
    default_target: std::sync::Mutex<String>,
}

impl MemoryCdpBridge {
    /// Create the bridge pair: the sender side for the client registry, and
    /// the receiver the runtime must drain (`BaoRuntime::run` does).
    pub fn new(default_target: impl Into<String>) -> (Arc<Self>, bao_cdp::servo_bridge::BridgeReceiver) {
        let (sender, receiver) = bridge_channel(UNDRAINED_TIMEOUT);
        (
            Arc::new(Self {
                sender,
                default_target: std::sync::Mutex::new(default_target.into()),
            }),
            receiver,
        )
    }

    /// Point the flat (sessionId-less) client face at a live page. Called by
    /// [`crate::BaoRuntime::create_page`] so `memory://` clients without an
    /// explicit target route to the newest page.
    pub fn set_default_target(&self, target: impl Into<String>) {
        *self.default_target.lock().unwrap() = target.into();
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
        let msg = CdpMessage {
            id: Some(0),
            method: method.to_string(),
            params: Some(params),
            session_id: None,
        };
        let params_ref = msg.params.clone();
        let response = handle_command(msg, target, &params_ref, Some(&self.sender));
        match response.error {
            Some(err) => InMemoryBridgeResponse::Err(err.message),
            None => InMemoryBridgeResponse::Ok(response.result.unwrap_or(serde_json::Value::Null)),
        }
    }
}
