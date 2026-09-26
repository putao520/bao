// @trace TEST-CLI-BROWSER [req:REQ-CLI-002] [level:integration]
//
// REQ-CLI-002 — `bao browser` 子命令,runtime-side contract faces.
//
// Layer boundary: the binary entry (`bao browser --url/--port/--headless/
// --stealth`, EXIT codes, the printed ws:// line) is owned by `bao_cli` +
// `bao_browser`, neither of which is in bao_runtime's dependency closure.
// Those criteria are tested at their SPEC-designated location:
//   - bao_browser/tests/suite/bao_cli_e2e_tests.rs (SPEC-named acceptance)
//   - bao_cli/tests/cli_dispatch.rs (subcommand dispatch)
//
// What THIS crate owns and really tests here: the endpoint contract the
// subcommand advertises (default CDP bind host/port served by the linked
// cdp-server) and the JS consumer faces for both endpoint kinds the
// subcommand can expose (memory:// in-process, ws://9222 external).

use bao_cdp_client as cdp_client;
use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
use cdp_server::ServerConfig;

fn eval_bool(ctx: &mut JsContext, source: &str) -> bool {
    matches!(ctx.eval(source, "<cli-browser>"), Ok(JsValue::Bool(true)))
}

fn test_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext::for_test");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

/// The default CDP endpoint of `bao browser` (curl http://127.0.0.1:9222/...)
/// is pinned by the linked server crate's defaults: host 127.0.0.1, port
/// 9222 — the exact numbers the CLI criteria reference.
#[test]
fn req_cli_002_default_cdp_endpoint_is_localhost_9222() {
    let cfg = ServerConfig::default();
    assert_eq!(cfg.host, "127.0.0.1");
    assert_eq!(cfg.port, 9222);
    // The discovery URL `ws://127.0.0.1:9222` the criteria grep for is built
    // from exactly these two values.
    let server = cdp_client::version();
    assert!(!server.is_empty());
}

/// JS consumer face for the endpoint `bao browser --port=9222` serves: the
/// ws:// route resolves with WebSocket transport identity (lazy — no I/O at
/// connect time; the live handshake is the bao_cli/bao_browser e2e's job).
#[test]
fn req_cli_002_js_face_connects_ws_endpoint_route() {
    let mut ctx = test_ctx();
    assert!(eval_bool(
        &mut ctx,
        r#"(function(){
            var b = Bao.browser.connect("ws://127.0.0.1:9222");
            return !!b
                && b.url === "ws://127.0.0.1:9222"
                && b.scheme === "ws"
                && b.transportKind === "WebSocket"
                && b.isWebSocket === true;
        })()"#
    ));
}

/// JS consumer face for the in-process endpoint the embedded browser mode
/// exposes (`memory://bao`), completing the two endpoint kinds the
/// subcommand can put on the wire.
#[test]
fn req_cli_002_js_face_connects_memory_endpoint_route() {
    let mut ctx = test_ctx();
    assert!(eval_bool(
        &mut ctx,
        r#"(function(){
            var b = Bao.browser.connect("memory://bao");
            return !!b && b.transportKind === "InMemory" && b.scheme === "memory";
        })()"#
    ));
}
