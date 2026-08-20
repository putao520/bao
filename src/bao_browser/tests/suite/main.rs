//! bao_browser integration test suite (single harness target).
//!
//! Every former top-level `tests/*.rs` target is now a module of this one
//! crate, so the workspace links ONE full-engine test binary instead of one
//! per file. Runtime isolation is provided by cargo-nextest: it runs each
//! test in its own process, so merging targets does not change test
//! isolation semantics. With plain `cargo test` (libtest multithreading
//! inside a single process), run with `--test-threads=1`.
//!
//! `suite/common/` is a shared helper module (not a target): sibling files
//! pull it in with `#[path = "common/mod.rs"] mod common;` — the `#[path]`
//! is REQUIRED: a bare `mod common;` in a non-root file resolves to
//! `<file_name>/common.rs`, not this directory (the old root-target layout
//! made the bare form work; the suite layout does not).

mod anti_crawler_detection_tests;
mod bao_api_json_stringify_tests;
mod bao_api_method_routing_tests;
mod bao_cli_e2e_tests;
mod bce004_isolate_tests;
mod bce004_parent_multinav_tests;
mod bce004_repro_tests;
mod bce004_stress_tests;
mod browser_config_tests;
mod browser_core_unit_tests;
mod browser_runtime_tests;
mod cdp_ws_command_face_tests;
mod click_human_e2e_tests;
mod compartment_isolation_tests;
mod config_boundary_deep_tests;
mod config_deep_tests;
mod config_pool_stats_deep_tests;
mod config_state_error_deep_tests;
mod config_validate_conversion_deep_tests;
mod cross_crate_compat_tests;
mod dom_node_interop_tests;
mod error_permission_screenshot_comprehensive_tests;
mod error_permission_screenshot_deep_tests;
mod evaluate_result_tests;
mod fingerprint_website_eval_e2e_tests;
mod h2_fetch_node_stack_e2e_tests;
mod indexeddb_e2e_tests;
mod media_e2e_tests;
mod mouse_bezier_e2e_tests;
mod multi_page_security_e2e_tests;
mod opaque_origin_startup_regression_tests;
mod page_lifecycle_tests;
mod page_net_bun_fingerprint_e2e_tests;
mod page_net_bun_full_matrix_e2e_tests;
mod page_net_bun_streaming_upload_e2e_tests;
mod pagepool_chaos_memory_safety_tests;
mod page_pool_delegate_deep_tests;
mod page_screenshot_deep_tests;
mod page_state_config_tests;
mod pagestate_lifecycle_tests;
mod page_wss_bao_tls_e2e_tests;
mod permission_boundary_tests;
mod permission_guard_error_deep_tests;
mod permission_guard_net_env_run_deep_tests;
mod permission_screenshot_error_tests;
mod realworld_anti_scraping_e2e_tests;
mod realworld_browser_automation_tests;
mod realworld_full_stack_tests;
mod rendering_pipeline_tests;
mod runtime_bridge_deep_tests;
mod screenshot_permission_error_tests;
mod security_sandbox_tests;
mod servo_render_pipeline_tests;
mod stealth_diagnostic_detection_tests;
mod stealth_fingerprint_e2e_tests;
mod stealth_profile_config_tests;
mod task_10_integration;
mod thread_safety_concurrency_tests;
mod worker_concurrent_servo_tests;
mod worker_onerror_integration_tests;
mod worker_tests;
