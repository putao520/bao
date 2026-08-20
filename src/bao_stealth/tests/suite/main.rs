//! bao_stealth integration test suite (single harness target).
//!
//! Every former top-level `tests/*.rs` target is now a module of this one
//! crate, so the workspace links ONE full-engine test binary instead of 36.
//! Runtime isolation is provided by cargo-nextest: it runs each `#[test]` in
//! its own process, so merging targets does not change test isolation
//! semantics. With plain `cargo test` (libtest multithreading inside a single
//! process), run with `--test-threads=1`.

mod anti_detection_verification_tests;
mod behavior_deep_tests;
mod behavior_simulator_deep_tests;
mod behavior_simulator_math_property_tests;
mod canvas_navigator_screen_deep_tests;
mod canvas_noise_deep_tests;
mod canvas_webgl_audio_property_tests;
mod cdp_stealth_traces_tests;
mod cloudflare_bot_management_tests;
mod concurrency_tests;
mod fingerprint_deep_tests;
mod headless_fingerprint_hiding_tests;
mod http2_fingerprint_deep_tests;
mod navigator_screen_http2_profile_deep_tests;
mod navigator_screen_webgl_audio_deep_tests;
mod profile_integration_tests;
mod real_detection_vectors_tests;
mod recaptcha_behavioral_tests;
mod stealth_consistency_tests;
mod stealth_cross_profile_tests;
mod stealth_deep_tests;
mod stealth_diagnostic_detection_tests;
mod stealth_edge_case_tests;
mod stealth_engine_integration_tests;
mod stealth_integration_tests;
mod stealth_js_injection_profile_consistency_tests;
mod stealth_profile_composition_tests;
mod stealth_tests;
mod subcomponent_deep_tests;
mod tls_fingerprint_deep_tests;
mod tls_http2_navigator_screen_cross_profile_deep_tests;
mod tls_ja3_ja4_suite_deep_tests;
mod tls_profile_deep_tests;
mod webgl_audio_canvas_property_deep_tests;
mod webgl_audio_http2_deep_tests;
mod webgl_audio_screen_deep_tests;
