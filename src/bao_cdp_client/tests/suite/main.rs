//! bao_cdp_client integration test suite (single harness target).
//!
//! Every former top-level `tests/*.rs` target is now a module of this one
//! crate, so the workspace links ONE full-engine test binary instead of one
//! per file. Runtime isolation is provided by cargo-nextest: it runs each
//! test in its own process, so merging targets does not change test
//! isolation semantics. With plain `cargo test` (libtest multithreading
//! inside a single process), run with `--test-threads=1`.
//!
//! `cdp_conformance/` keeps its own module root (mod.rs) — the former
//! explicit `[[test]] cdp_conformance` target, now a submodule of the suite.

mod b_class_injection_defense;
mod bridge_a_class;
mod bridge_dispatcher;
mod bridge_e_class;
mod cdp_full_chain_tests;
mod cdp_timing_tests;
mod d_class_local_state;
mod debugger_sm_api_tests;
mod e2e_external_chrome;
mod e2e_internal_servo;
mod e2e_playwright_compat;
mod event_coverage_full;
mod event_translation;
mod injection_defense_full;
mod public_api_complete;
mod regression_no_eval_injection;
mod transport_in_memory;
mod transport_ws_handshake;
mod url_scheme_routing;
mod cdp_conformance;
