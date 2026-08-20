//! Integration test suite (single harness target).
//!
//! Every former top-level `tests/*.rs` target is now a module of this one
//! crate, so the workspace links ONE full-engine test binary instead of one
//! per file. Runtime isolation is provided by cargo-nextest: it runs each
//! `#[test]` in its own process, so merging targets does not change test
//! isolation semantics. With plain `cargo test` (libtest multithreading
//! inside a single process), run with `--test-threads=1`.

mod cdp_server_ws_url_config_deep_tests;
mod concurrency_tests;
mod config_session_transport_deep_tests;
mod edge_case_tests;
mod extended_tests;
mod final_remaining_cdp_tests;
mod integration_tests;
mod protocol_broadcaster_deep_tests;
mod protocol_compliance_tests;
mod protocol_conformance_tests;
mod protocol_error_session_tests;
mod protocol_robustness_tests;
mod protocol_serverconfig_deep_tests;
mod registry_advanced_tests;
mod registry_session_event_deep_tests;
mod server_api_boundary_tests;
mod server_api_lifecycle_deep_tests;
mod server_config_builder_deep_tests;
mod server_transport_protocol_deep_tests;
mod stress_recovery_tests;
mod transport_http_parse_tests;
mod transport_parse_boundary_tests;
mod transport_parse_deep_tests;
mod unit_tests;
