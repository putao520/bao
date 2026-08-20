//! Integration test suite (single harness target).
//!
//! Every former top-level `tests/*.rs` target is now a module of this one
//! crate, so the workspace links ONE full-engine test binary instead of one
//! per file. Runtime isolation is provided by cargo-nextest: it runs each
//! `#[test]` in its own process, so merging targets does not change test
//! isolation semantics. With plain `cargo test` (libtest multithreading
//! inside a single process), run with `--test-threads=1`.

mod backend_bridge_channel_deep_tests;
mod bridge_channel_deep_tests;
mod bridge_channel_stress_concurrent_tests;
mod bridge_channel_timeout_clone_deep_tests;
mod bridge_channel_timeout_edge_deep_tests;
mod bridge_command_exhaustive_tests;
mod cdp_types_deep_tests;
mod domain_handler_response_field_boundary_tests;
mod domain_stress_tests;
mod perf_refactor_integration_tests;
mod protocol_all_domains_internal_backend_tests;
mod protocol_domain_handler_deep_tests;
mod protocol_edge_case_tests;
mod protocol_message_deep_tests;
mod protocol_serialize_boundary_tests;
mod protocol_subcommand_full_coverage_tests;
mod router_backend_deep_tests;
mod router_external_detach_edge_tests;
mod router_lifecycle_tests;
mod router_session_internal_backend_deep_tests;
mod router_session_lifecycle_deep_tests;
