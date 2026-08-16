// @trace TEST-BRW-003 [req:REQ-BRW-003] [level:e2e]
// BCE regression (P0 browser startup panic, servo error.rs:74):
// "swallowed JSAPI failure leaves a pending exception on the servo
// ScriptThread context". The original trigger face: capability probes
// against the servo Window global / Page Realm DOM objects during page init
// (stealth engine_props, install_all_native, lazy DOM getters) hit throwing
// accessors — opaque origins (data: URLs, about:blank) raise SecurityError
// from storage getters — and the failed JS_GetProperty/JS_HasProperty/
// JS_Call left the exception pending. servo's `throw_dom_exception` then
// hit `assert!(!JS_IsExceptionPending)` and killed the ScriptThread at page
// init: pipeline never ready, CDP never answered.
//
// These tests pin BOTH observables of that crash class:
//   1. opaque-origin pages with stealth profiles boot N times, every page
//      reaches ready, and evaluation keeps answering AFTER the storage
//      getter throws (ScriptThread alive).
//   2. a real CDP WebSocket session against an opaque-origin page answers
//      Runtime.evaluate (the "CDP never listens" observable, inverted).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use bao_browser::{BaoConfig, BaoRuntime, PageConfig, PageHandle, PageState};
use bao_cdp::domains::ServoTargetProvider;
use bao_cdp::servo_bridge::bridge_channel;
use bao_stealth::StealthProfile;
use bun_uws::ws_client::{RecvOutcome, WebSocketClient};
use cdp_server::{CdpServer, ServerConfig};
use serde_json::{json, Value};

/// Browser boots are serialized within this binary: two servo BaoRuntimes
/// racing in one process competes for the single embedder slot — the failure
/// would be flaky infra, not the class under test.
static BOOT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn boot_lock() -> MutexGuard<'static, ()> {
    let m = BOOT_LOCK.get_or_init(|| Mutex::new(()));
    match m.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn require_display() -> bool {
    if std::env::var("DISPLAY").is_err() && std::env::var("WAYLAND_DISPLAY").is_err() {
        eprintln!("[skip] no DISPLAY or WAYLAND_DISPLAY — servo requires a display server");
        return false;
    }
    true
}

/// Pump servo's loop until the page answers a page-realm evaluation. The P0
/// crash manifested exactly here: the ScriptThread died at init, so every
/// evaluation errored forever ("pipeline never ready, CDP never listens").
/// `readyState` is deliberately NOT the oracle — data:/about:blank pages may
/// legitimately sit in Loading in some harnesses while the ScriptThread is
/// perfectly alive; conversely a dead ScriptThread never answers evaluation.
fn wait_thread_alive(page: &PageHandle, max_ms: u64) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed().as_millis() < max_ms as u128 {
        let _ = page.evaluate_js("");
        if matches!(page.get_state(), PageState::Interactive | PageState::Idle) {
            return true;
        }
        if let Ok(v) = page.evaluate_js_web("1") {
            if v.trim().trim_matches('"') == "1" {
                return true;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    matches!(page.get_state(), PageState::Interactive | PageState::Idle)
}

fn create_opaque_page(
    runtime: &BaoRuntime,
    url: &str,
) -> Result<PageHandle, String> {
    // Retries mirror the established e2e harness: WebView creation can
    // transiently fail while servo warms up; the class under test is NOT
    // "create never fails" but "a created page must not die at init".
    let mut last_err = String::new();
    for _ in 0..3 {
        match runtime.create_page(&PageConfig {
            url: Some(url.to_string()),
            // The stealth profile drives the install-time probe surface
            // (engine_props capability probes + install_all_native) — the
            // original crash path. It MUST stay on (BAO_REG_NO_STEALTH=1 is
            // the differential-diagnosis knob only).
            stealth_profile: if std::env::var_os("BAO_REG_NO_STEALTH").is_some() {
                None
            } else {
                Some(StealthProfile::firefox_default())
            },
            ..Default::default()
        }) {
            Ok(p) => return Ok(p),
            Err(e) => {
                last_err = format!("{e}");
                std::thread::sleep(std::time::Duration::from_secs(3));
            }
        }
    }
    Err(last_err)
}

/// N opaque-origin boots (data: URLs and about:blank are BOTH opaque
/// origins — the storage getters throw SecurityError on them), each with the
/// stealth probe surface active, each page probed with the throwing
/// accessor AFTER ready, then re-evaluated to prove the ScriptThread
/// survived.
#[test]
fn opaque_origin_startup_survives_throwing_accessors() {
    if !require_display() {
        return;
    }
    let _guard = boot_lock();
    bun_core::Output::init_test();

    let runtime = BaoRuntime::new(BaoConfig::default()).expect("BaoRuntime::new");

    let data_url = "data:text/html,<html><body><p>opaque</p></body></html>";
    let urls: [&str; 6] = [
        data_url,
        "about:blank",
        data_url,
        "about:blank",
        data_url,
        "about:blank",
    ];

    for (round, url) in urls.iter().enumerate() {
        let page = create_opaque_page(&runtime, url)
            .unwrap_or_else(|e| panic!("round {round}: create_page({url}) failed: {e}"));

        assert!(
            wait_thread_alive(&page, 15_000),
            "round {round} ({url}): ScriptThread never answered evaluation — \
             dead at init (the error.rs:74 crash signature)"
        );

        // Hit the exact original trigger: the opaque-origin storage getter.
        // data:/about:blank are opaque origins — the getter must throw
        // SecurityError (spec + servo behavior, pinned by 6/6 rounds) — and
        // the page realm must still answer afterwards.
        let storage = page
            .evaluate_js_web(
                "var r; try { r = 'access:' + String(window.localStorage !== undefined) } \
                 catch (e) { r = 'threw:' + e.name } r",
            )
            .unwrap_or_else(|e| panic!("round {round}: storage probe eval failed: {e}"));
        let storage = storage.trim().trim_matches('"');
        eprintln!("[opaque-reg] round {round} ({url}) storage probe: {storage}");
        assert_eq!(
            storage, "threw:SecurityError",
            "round {round}: opaque-origin storage getter must throw SecurityError"
        );

        // ScriptThread liveness: evaluation still answers after the throw.
        let alive = page
            .evaluate_js_web("40 + 2")
            .unwrap_or_else(|e| panic!("round {round}: post-throw eval failed: {e}"));
        assert_eq!(
            alive.trim().trim_matches('"'),
            "42",
            "round {round}: ScriptThread dead after throwing accessor (error.rs:74 class)"
        );

        // Node-realm lazy DOM getter (lazy_dom_getter_impl probes the Page
        // Realm Window from the node realm — the runtime twin of the P0).
        // Must answer without killing the thread; wrapped proxy or absent
        // are both acceptable outcomes, an Err is not.
        let node_probe = page
            .evaluate_js("typeof window")
            .unwrap_or_else(|e| panic!("round {round}: node-realm lazy getter failed: {e}"));
        let node_probe = node_probe.trim().trim_matches('"');
        assert!(
            node_probe == "object" || node_probe == "undefined",
            "round {round}: unexpected node-realm typeof window: {node_probe}"
        );

        // Second evaluate after the cross-realm probe — still alive.
        let still = page
            .evaluate_js_web("7 * 6")
            .unwrap_or_else(|e| panic!("round {round}: post-lazy-getter eval failed: {e}"));
        assert_eq!(still.trim().trim_matches('"'), "42");
    }
}

/// The "CDP never listens" observable, inverted: a real CDP WebSocket
/// session against an opaque-origin page must complete a
/// Runtime.evaluate round-trip through the production wiring
/// (BaoWsRegistry → bridge → servo PagePool).
#[test]
fn cdp_answers_on_opaque_origin_page() {
    if !require_display() {
        return;
    }
    let _guard = boot_lock();
    bun_core::Output::init_test();

    let runtime = BaoRuntime::new(BaoConfig::default()).expect("BaoRuntime::new");
    let page = create_opaque_page(&runtime, "data:text/html,<html><body><p>cdp</p></body></html>")
        .expect("create_page(data:)");
    assert!(
        wait_thread_alive(&page, 15_000),
        "ScriptThread never answered evaluation — dead at init (error.rs:74 class)"
    );

    let (bridge_tx, bridge_rx) = bridge_channel(std::time::Duration::from_secs(30));
    let (event_subscriber, servo_event_rx) = bao_cdp_client::bridge::EventSubscriber::new();
    runtime.set_event_channel(event_subscriber.sender());

    let registry = Arc::new(bao_browser::BaoWsRegistry::new(bridge_tx.clone()));
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let server_config = ServerConfig::builder()
        .host("127.0.0.1")
        .port(port)
        .build();
    let mut server = CdpServer::with_registry(server_config, registry);
    server.set_target_provider(Arc::new(ServoTargetProvider::new(
        bridge_tx,
        page.id().to_string(),
        "127.0.0.1".into(),
        port,
    )));
    std::thread::spawn(move || {
        let _ = server.run();
    });

    let ws_url = format!("ws://127.0.0.1:{port}/devtools/page/{}", page.id());

    // Client phase: connect, Runtime.evaluate 40+2, report the value.
    let done = Arc::new(AtomicBool::new(false));
    let result = Arc::new(Mutex::new(None::<i64>));
    {
        let done = Arc::clone(&done);
        let result = Arc::clone(&result);
        std::thread::spawn(move || {
            // Connect retries: the server thread needs a beat to bind.
            for _ in 0..200 {
                let mut client = match WebSocketClient::connect(&ws_url) {
                    Ok(c) => c,
                    Err(_) => {
                        std::thread::sleep(std::time::Duration::from_millis(50));
                        continue;
                    }
                };
                client.set_read_timeout(std::time::Duration::from_secs(5));
                let msg = json!({
                    "id": 1,
                    "method": "Runtime.evaluate",
                    "params": { "expression": "40 + 2", "returnByValue": true }
                });
                if client.send_text(&msg.to_string()).is_err() {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    continue;
                }
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                while std::time::Instant::now() < deadline {
                    match client.recv() {
                        Ok(RecvOutcome::Message(_op, payload)) => {
                            let text = match std::str::from_utf8(&payload) {
                                Ok(t) => t,
                                Err(_) => continue,
                            };
                            if let Ok(v) = serde_json::from_str::<Value>(text) {
                                if v.get("id").and_then(|i| i.as_i64()) == Some(1) {
                                    let value = v
                                        .pointer("/result/result/value")
                                        .and_then(|x| x.as_i64());
                                    *result.lock().unwrap() = value;
                                    done.store(true, Ordering::SeqCst);
                                    return;
                                }
                            }
                        }
                        Ok(_) => continue,
                        Err(_) => break,
                    }
                }
                break;
            }
        });
    }

    // Main thread: the run_with_bridge loop shape (servo spin + bridge
    // drain + event translation), bounded by the client's done flag.
    use bao_browser::handle_bridge_command;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while !done.load(Ordering::Relaxed) && std::time::Instant::now() < deadline {
        runtime.spin_event_loop();
        bridge_rx.drain(|cmd| handle_bridge_command(cmd, runtime.page_pool()));
        while let Ok(servo_event) = servo_event_rx.try_recv() {
            let _ = servo_event;
        }
    }

    assert!(
        done.load(Ordering::SeqCst),
        "CDP session never answered Runtime.evaluate on the opaque-origin page \
         (the error.rs:74 'CDP never listens' signature)"
    );
    assert_eq!(
        *result.lock().unwrap(),
        Some(42),
        "Runtime.evaluate round-trip returned the wrong value"
    );
}
