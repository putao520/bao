//! Integration test suite (single harness target).
//!
//! Every former top-level `tests/*.rs` target is now a module of this one
//! crate, so the workspace links ONE full-engine test binary instead of one
//! per file. Runtime isolation is provided by cargo-nextest: it runs each
//! `#[test]` in its own process, so merging targets does not change test
//! isolation semantics. With plain `cargo test` (libtest multithreading
//! inside a single process), run with `--test-threads=1`.

mod codegen_boundary_tests;
mod codegen_constructor_finalizer_tests;
mod codegen_deep_tests;
mod codegen_edge_case_tests;
mod codegen_generate_all_module_tests;
mod codegen_roundtrip_tests;
mod console_routing_tests;
mod dispatch_sm_tests;
mod engine_core_tests;
mod error_handling_tests;
mod es_advanced_features_tests;
mod host_fn_tests;
mod job_queue_context_tests;
mod js_context_fusion_tests;
mod jserror_parseresult_deep_tests;
mod jsvalue_display_format_tests;
mod module_loader_host_fn_tests;
mod raw_value_root_guard_tests;
mod resource_exhaustion_tests;
mod value_boundary_tests;
mod value_error_tests;
