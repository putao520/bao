// @trace TEST-LIB-SRV [req:REQ-LIB-001,REQ-LIB-002] [level:integration]
//
// REQ-LIB-001 (PagePool 多页面管理) / REQ-LIB-002 (PagePool 资源管理) —
// runtime-side consumer seam.
//
// Layer boundary (read before "extending" these tests): PagePool / PageHandle
// / pool stats are owned by `bao_browser`, which is NOT in bao_runtime's
// dependency closure (verified: zero `bao_browser` nodes in
// `cargo tree -p bun_runtime --edges normal,dev`). The lifecycle criteria of
// these two REQs are therefore tested at their SPEC-designated location:
//   - bao_browser/tests/suite/page_lifecycle_tests.rs            (REQ-LIB-001)
//   - bao_browser/tests/suite/page_pool_delegate_deep_tests.rs   (REQ-LIB-001)
//   - bao_browser/tests/suite/pagepool_chaos_memory_safety_tests.rs (LIB-001/002)
//   - bao_browser/tests/suite/config_pool_stats_deep_tests.rs    (REQ-LIB-002)
//   - bao_browser/tests/suite/page_state_config_tests.rs         (REQ-LIB-002)
//
// What THIS crate owns and what is tested here: the JS-facing page-layer
// entry (`Bao.browser.connect`, bao_runtime::bao_browser_global) that page
// consumers reach through the runtime, and its fail-closed URL routing.

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn eval_bool(ctx: &mut JsContext, source: &str) -> bool {
    matches!(ctx.eval(source, "<lib-pagepool>"), Ok(JsValue::Bool(true)))
}

fn test_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext::for_test");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

/// REQ-LIB-001 consumer face: the in-process browser endpoint resolves to a
/// real client object carrying the memory:// transport identity — the seam a
/// page consumer crosses before any PageHandle exists.
#[test]
fn req_lib_001_bao_browser_connect_memory_face() {
    let mut ctx = test_ctx();
    assert!(eval_bool(
        &mut ctx,
        "typeof Bao === 'object' && Bao !== null && typeof Bao.browser === 'object'"
    ));
    assert!(eval_bool(
        &mut ctx,
        "typeof Bao.browser.connect === 'function'"
    ));
    assert!(eval_bool(
        &mut ctx,
        r#"(function(){
            var b = Bao.browser.connect("memory://bao");
            return !!b
                && b.url === "memory://bao"
                && b.scheme === "memory"
                && b.transportKind === "InMemory"
                && b.isInMemory === true
                && b.isWebSocket === false;
        })()"#
    ));
}

/// REQ-LIB-001 multi-page face at the runtime seam: each connect() yields a
/// distinct client object (no shared-singleton aliasing across consumers).
#[test]
fn req_lib_001_connect_yields_distinct_clients() {
    let mut ctx = test_ctx();
    assert!(eval_bool(
        &mut ctx,
        r#"(function(){
            var a = Bao.browser.connect("memory://bao");
            var b = Bao.browser.connect("memory://bao");
            return a !== b && a.url === b.url && a.transportKind === b.transportKind;
        })()"#
    ));
}

/// REQ-LIB-002 fail-closed routing face: an unsupported scheme is a hard JS
/// error, never a silent fallback to another transport (the same discipline
/// the pool layer applies to resource accounting).
#[test]
fn req_lib_002_invalid_scheme_fails_closed() {
    let mut ctx = test_ctx();
    assert!(eval_bool(
        &mut ctx,
        r#"(function(){
            try {
                Bao.browser.connect("ftp://x");
                return false; // silent acceptance = failure
            } catch (e) {
                return String(e && e.message || e).indexOf("invalid URL scheme") !== -1;
            }
        })()"#
    ));
}

/// REQ-LIB-002 handle-identity face: WebSocket routes are recorded with their
/// own transport kind (lazy — no I/O at connect time), so consumers can
/// account every live handle by kind before any handshake happens.
#[test]
fn req_lib_002_websocket_route_identity() {
    let mut ctx = test_ctx();
    assert!(eval_bool(
        &mut ctx,
        r#"(function(){
            var b = Bao.browser.connect("ws://127.0.0.1:9222/devtools/browser");
            return !!b
                && b.scheme === "ws"
                && b.transportKind === "WebSocket"
                && b.isWebSocket === true
                && b.isInMemory === false;
        })()"#
    ));
}
