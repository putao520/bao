//! C15 coverage wave — env-dependent knob arm of `bun_http` lib.rs
//! (REQ-ENG-010). Own test binary on purpose: `BUN_FEATURE_FLAG_EXPERIMENTAL_HTTP3_CLIENT`
//! feeds `h3_alt_svc_enabled()` through bun_core's env-var accessor, which
//! must observe the variable set before this process's first read. A separate
//! top-level target guarantees process isolation under both nextest and plain
//! `cargo test` (one binary = one process), mirroring the C7
//! `idle_timeout_tests` precedent for env-before-read wiring.

/// Link leg (C7 suites convention): referencing `force_link` keeps
/// bao_native_stubs in the binary; its .init_array ctor drags in the
/// lsquic/boringssl native archives that bun_uws_sys's quic/socket objects
/// reference at link time.
#[test]
fn native_stubs_link_leg_c15() {
    bao_native_stubs::force_link();
}

/// The env arm of the h3 Alt-Svc gate: the feature flag alone (no CLI static
/// flip) must enable `h3_alt_svc_enabled`. The CLI arm lives in
/// `lib_surface_c15_tests`.
#[test]
fn h3_alt_svc_enabled_env_flag_arm_c15() {
    // Must precede the first accessor read in this process. Single-threaded
    // test bring-up — the established pre-runtime env idiom in this repo.
    unsafe { std::env::set_var("BUN_FEATURE_FLAG_EXPERIMENTAL_HTTP3_CLIENT", "1") };
    // CLI static stays off — the env arm must carry the gate on its own.
    assert!(!bun_http::EXPERIMENTAL_HTTP3_CLIENT_FROM_CLI.load(std::sync::atomic::Ordering::Relaxed));
    assert!(
        bun_http::h3_alt_svc_enabled(),
        "env feature flag must enable the h3 Alt-Svc gate"
    );
}
