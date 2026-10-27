// @trace REQ-ENG-004
//! Probe-release rebind window (E26 ledger classes A/B/C, fleet-observed
//! 1/5 EADDRINUSE under parallel suite load — E17).
//!
//! `bind("127.0.0.1:0")` + read port + drop leaves a window between the
//! probe's drop and the real consumer's bind; a competing suite process's
//! ephemeral allocation can claim the port there. The root fix inside the
//! test's resource domain is the E26 verified shape: one FRESH probed port
//! per attempt + a loud non-race failure path + a fail-closed retry budget.
//!
//! Contract: `attempt` returns `None` ONLY for the specific lost-race
//! signature (EADDRINUSE / a resolved fetch on a dropped port / a dead
//! fixture child); any other failure must panic inside `attempt` — the
//! retry ring never masks non-race breakage.

/// Retry budget: one fresh probed port per attempt. 8 is far beyond the
/// observed 1/5 per-suite-run collision rate (E17 fleet observation).
#[allow(dead_code)]
pub const PROBE_REBIND_RETRIES: u32 = 8;

/// Probe an ephemeral TCP port on 127.0.0.1 and release it (the bare
/// probe-release — always pair with a retry ring, never one-shot).
#[allow(dead_code)]
pub fn probe_free_tcp_port() -> u16 {
    let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("bind probe");
    let port = probe.local_addr().expect("probe local_addr").port();
    drop(probe);
    port
}

/// Run `attempt` once per freshly probed TCP port until it completes.
/// `None` = this attempt lost the release-rebind race — retry on a new
/// port; exhausting the budget fails closed.
#[allow(dead_code)]
pub fn retry_probed_port<T>(mut attempt: impl FnMut(u16) -> Option<T>) -> T {
    for _ in 1..=PROBE_REBIND_RETRIES {
        if let Some(value) = attempt(probe_free_tcp_port()) {
            return value;
        }
    }
    panic!(
        "probe-release rebind: exhausted {PROBE_REBIND_RETRIES} fresh probed ports without a clean bind"
    );
}
