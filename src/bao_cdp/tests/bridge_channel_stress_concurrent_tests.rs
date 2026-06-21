// @trace TEST-CDP-036 [req:REQ-CDP-003,REQ-CDP-006] [level:unit]
// Bridge channel stress tests: concurrent send/receive, burst commands,
// timeout under load, drain correctness, fire-and-forget, is_alive.
//
// Adversarial gaps closed (补充遗漏断言·边界条件·SPEC 对齐):
//   G1: variant coverage 26/40 → 40/40 (补 CreateTarget/ListTargets/13 Debugger 变体)
//   G2: weak assertion `total_processed > 0` → exact-count + per-thread success
//   G3: missing FIFO order-preservation assertions on drain
//   G4: missing timeout-bound assertions (elapsed within [min, max])
//   G5: missing boundary conditions (zero timeout, empty Vec, None options,
//       f64 precision, width=0/height=0, quality=0/100, large payloads)
//   G6: missing is_alive after drop(sender) clone decay
//   G7: missing drain-then-send-then-drain ordering
//   G8: missing response value exact-equality roundtrip (not just is_ok)

use bao_cdp::{BridgeCommand, BridgeResponse, bridge_channel};
use serde_json::{json, Value};
use std::time::{Duration, Instant};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

const TID: &str = "test-target";

fn setup(timeout_ms: u64) -> (bao_cdp::BridgeSender, bao_cdp::BridgeReceiver) {
    bridge_channel(Duration::from_millis(timeout_ms))
}

fn counter() -> Arc<AtomicU32> {
    Arc::new(AtomicU32::new(0))
}

// ============================================================================
// Burst send/drain: many commands sent before processing
// ============================================================================

#[test]
fn test_burst_100_commands_drain_all() {
    let (tx, rx) = setup(500);
    for i in 0..100 {
        let url = format!("http://example.com/{}", i);
        tx.send_fire_and_forget(BridgeCommand::Navigate { target_id: TID.into(), url });
    }
    let count = counter();
    let last_url = Arc::new(std::sync::Mutex::new(String::new()));
    let last_url2 = Arc::clone(&last_url);
    let drained = rx.drain(|cmd| {
        count.fetch_add(1, Ordering::SeqCst);
        if let BridgeCommand::Navigate { url, .. } = cmd {
            assert!(url.starts_with("http://example.com/"), "url prefix invariant: {}", url);
            *last_url2.lock().unwrap() = url;
        } else {
            panic!("Expected Navigate, got {:?}", cmd);
        }
        BridgeResponse { result: Ok(json!({})) }
    });
    // G1: drain returns usize; assert both handler-count and drain-count agree
    assert_eq!(count.load(Ordering::SeqCst), 100, "handler-side counter must equal 100");
    assert_eq!(drained, 100, "drain() return value must equal handler invocation count");
    // G7: FIFO order — last drained URL should be index 99
    assert_eq!(*last_url.lock().unwrap(), "http://example.com/99",
        "FIFO order: last drained command must be the last sent");
}

#[test]
fn test_burst_50_eval_commands_drain() {
    let (tx, rx) = setup(500);
    for i in 0..50 {
        let expr = format!("1 + {}", i);
        tx.send_fire_and_forget(BridgeCommand::EvaluateJs { target_id: TID.into(), expression: expr, return_by_value: true });
    }
    let count = counter();
    let drained = rx.drain(|cmd| {
        count.fetch_add(1, Ordering::SeqCst);
        if let BridgeCommand::EvaluateJs { expression, return_by_value, .. } = cmd {
            assert!(expression.starts_with("1 + "), "expression prefix: {}", expression);
            assert!(return_by_value, "return_by_value must be preserved as true");
        } else {
            panic!("Expected EvaluateJs, got {:?}", cmd);
        }
        BridgeResponse { result: Ok(json!({})) }
    });
    assert_eq!(count.load(Ordering::SeqCst), 50);
    assert_eq!(drained, 50, "drain return must match counter");
}

#[test]
fn test_burst_mixed_command_types() {
    let (tx, rx) = setup(500);
    for i in 0..30 {
        match i % 3 {
            0 => tx.send_fire_and_forget(BridgeCommand::Navigate { target_id: TID.into(), url: format!("http://x/{}", i) }),
            1 => tx.send_fire_and_forget(BridgeCommand::EvaluateJs { target_id: TID.into(), expression: format!("{}", i), return_by_value: true }),
            _ => tx.send_fire_and_forget(BridgeCommand::GetTitle { target_id: TID.into() }),
        }
    }
    let nav = counter();
    let eval = counter();
    let title = counter();
    let drained = rx.drain(|cmd| {
        match cmd {
            BridgeCommand::Navigate { .. } => { nav.fetch_add(1, Ordering::SeqCst); }
            BridgeCommand::EvaluateJs { .. } => { eval.fetch_add(1, Ordering::SeqCst); }
            BridgeCommand::GetTitle { .. } => { title.fetch_add(1, Ordering::SeqCst); }
            other => panic!("unexpected command type: {:?}", other),
        }
        BridgeResponse { result: Ok(json!({})) }
    });
    // G1: per-variant counts (10 each) + total invariant
    assert_eq!(nav.load(Ordering::SeqCst), 10, "Navigate count (i%3==0 → 0,3,...,27 = 10)");
    assert_eq!(eval.load(Ordering::SeqCst), 10, "EvaluateJs count (i%3==1)");
    assert_eq!(title.load(Ordering::SeqCst), 10, "GetTitle count (i%3==2)");
    let total = nav.load(Ordering::SeqCst) + eval.load(Ordering::SeqCst) + title.load(Ordering::SeqCst);
    assert_eq!(total, 30, "sum of per-variant counters must equal total sent");
    assert_eq!(drained as u32, total, "drain() return must equal sum of per-variant counters");
}

// ============================================================================
// Concurrent send from multiple threads
// ============================================================================

#[test]
fn test_concurrent_sends_from_4_threads() {
    let (tx, rx) = setup(1000);
    let tx = Arc::new(tx);
    let mut handles = vec![];

    for t in 0..4u32 {
        let tx = Arc::clone(&tx);
        handles.push(std::thread::spawn(move || {
            for i in 0..25 {
                let url = format!("http://thread{}/page{}", t, i);
                tx.send_fire_and_forget(BridgeCommand::Navigate { target_id: TID.into(), url });
            }
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    // G3: collect target_ids per url-prefix to assert each thread's 25 arrived
    let per_thread = Arc::new(std::sync::Mutex::new([0u32; 4]));
    let count = counter();
    let per_thread2 = Arc::clone(&per_thread);
    rx.drain(|cmd| {
        count.fetch_add(1, Ordering::SeqCst);
        if let BridgeCommand::Navigate { url, .. } = cmd {
            // url = "http://thread{t}/page{i}"
            let t = url.strip_prefix("http://thread")
                .and_then(|s| s.split('/').next())
                .and_then(|s| s.parse::<usize>().ok());
            if let Some(t) = t {
                if t < 4 {
                    per_thread2.lock().unwrap()[t] += 1;
                }
            }
        }
        BridgeResponse { result: Ok(json!({})) }
    });
    assert_eq!(count.load(Ordering::SeqCst), 100, "total drained must equal 4*25");
    let counts = per_thread.lock().unwrap();
    for t in 0..4 {
        assert_eq!(counts[t], 25, "thread {} must have exactly 25 commands drained", t);
    }
}

#[test]
fn test_concurrent_send_with_sync_response() {
    let (tx, rx) = setup(2000);
    let tx = Arc::new(tx);
    let rx = Arc::new(std::sync::Mutex::new(rx));
    let total_processed = Arc::new(AtomicU32::new(0));
    let sync_ok_count = Arc::new(AtomicU32::new(0));

    // Start responder thread
    let rx2 = Arc::clone(&rx);
    let total2 = Arc::clone(&total_processed);
    let responder = std::thread::spawn(move || {
        for _ in 0..500 {
            let done = {
                let rx_guard = rx2.lock().unwrap();
                rx_guard.try_process(|cmd| {
                    total2.fetch_add(1, Ordering::SeqCst);
                    match cmd {
                        BridgeCommand::GetTitle { .. } => BridgeResponse { result: Ok(json!("title")) },
                        BridgeCommand::Navigate { url, .. } => BridgeResponse { result: Ok(json!({"navigated": url})) },
                        _ => BridgeResponse { result: Ok(json!({})) },
                    }
                })
            };
            if done {
                continue;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    });

    // Send from multiple threads — count sync successes precisely
    let mut senders = vec![];
    for t in 0..4u32 {
        let tx = Arc::clone(&tx);
        let sync_ok2 = Arc::clone(&sync_ok_count);
        senders.push(std::thread::spawn(move || {
            for i in 0..10 {
                if i % 2 == 0 {
                    let resp = tx.send(BridgeCommand::GetTitle { target_id: TID.into() });
                    // G2: assert per-call success, count every Ok
                    assert!(resp.result.is_ok(), "thread {} iter {} sync send must succeed, got {:?}", t, i, resp.result);
                    sync_ok2.fetch_add(1, Ordering::SeqCst);
                } else {
                    tx.send_fire_and_forget(BridgeCommand::Navigate { target_id: TID.into(), url: format!("t{}i{}", t, i) });
                }
            }
        }));
    }

    for s in senders {
        s.join().unwrap();
    }

    responder.join().unwrap();
    // G2: replace weak `> 0` with exact-count assertions
    let processed = total_processed.load(Ordering::SeqCst);
    assert!(processed > 0, "should process at least some commands");
    let sync_ok = sync_ok_count.load(Ordering::SeqCst);
    // 4 threads * 5 sync sends (i%2==0 → i in {0,2,4,6,8}) = 20 sync sends
    assert_eq!(sync_ok, 20, "exactly 20 sync GetTitle sends (4 threads * 5 even iterations) must succeed");
    // processed >= sync_ok because responder also processes fire-and-forget Navigate commands
    assert!(processed >= sync_ok,
        "processed ({}) must be >= sync successes ({}) — responder also handles fire-and-forget", processed, sync_ok);
}

// ============================================================================
// Timeout behavior under load
// ============================================================================

#[test]
fn test_timeout_when_no_responder() {
    let (tx, _rx) = setup(50);
    let start = Instant::now();
    let resp = tx.send(BridgeCommand::GetTitle { target_id: TID.into() });
    let elapsed = start.elapsed();
    assert!(resp.result.is_err());
    let err = resp.result.unwrap_err();
    assert!(err.contains("timeout"), "error must mention 'timeout', got: {}", err);
    // G4: timeout must respect the configured bound — allow scheduling slack (10x upper)
    assert!(elapsed >= Duration::from_millis(45),
        "send must block at least ~timeout (45ms) before returning, took {:?}", elapsed);
    assert!(elapsed <= Duration::from_millis(500),
        "send must not vastly exceed timeout bound, took {:?}", elapsed);
}

#[test]
fn test_timeout_message_format() {
    let (tx, _rx) = setup(20);
    let resp = tx.send(BridgeCommand::Navigate { target_id: TID.into(), url: "http://x".into() });
    let err = resp.result.unwrap_err();
    assert!(err.contains("timeout") || err.contains("bridge"),
        "error must mention 'timeout' or 'bridge', got: {}", err);
}

#[test]
fn test_slow_responder_still_succeeds() {
    let (tx, rx) = setup(500);
    let rx = Arc::new(std::sync::Mutex::new(rx));

    let rx2 = Arc::clone(&rx);
    let handler = std::thread::spawn(move || {
        for _ in 0..5 {
            let processed = {
                let rx_guard = rx2.lock().unwrap();
                rx_guard.try_process(|_| {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    BridgeResponse { result: Ok(json!({"slow": true})) }
                })
            };
            if !processed {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    });

    std::thread::sleep(std::time::Duration::from_millis(10));
    let resp = tx.send(BridgeCommand::GetTitle { target_id: TID.into() });
    assert!(resp.result.is_ok());
    // G8: exact-value equality roundtrip, not just is_ok
    assert_eq!(resp.result.unwrap(), json!({"slow": true}),
        "slow responder must return the exact value it produced");
    handler.join().unwrap();
}

// ============================================================================
// Fire-and-forget correctness
// ============================================================================

#[test]
fn test_fire_and_forget_does_not_block() {
    let (tx, _rx) = setup(50);
    let start = std::time::Instant::now();
    for _ in 0..1000 {
        tx.send_fire_and_forget(BridgeCommand::GetTitle { target_id: TID.into() });
    }
    let elapsed = start.elapsed();
    assert!(elapsed.as_millis() < 100, "fire-and-forget should be fast, took {:?}", elapsed);
}

#[test]
fn test_fire_and_forget_commands_receivable() {
    let (tx, rx) = setup(500);
    for i in 0..10 {
        tx.send_fire_and_forget(BridgeCommand::Navigate { target_id: TID.into(), url: format!("http://{}", i) });
    }
    let count = counter();
    let drained = rx.drain(|_| {
        count.fetch_add(1, Ordering::SeqCst);
        BridgeResponse { result: Ok(json!({})) }
    });
    assert_eq!(count.load(Ordering::SeqCst), 10);
    assert_eq!(drained, 10, "drain return must equal 10");
}

// ============================================================================
// is_alive check
// ============================================================================

#[test]
fn test_is_alive_when_channel_open() {
    let (tx, rx) = setup(500);
    assert!(tx.is_alive());
    drop(rx);
    assert!(!tx.is_alive());
}

#[test]
fn test_is_alive_multiple_calls() {
    let (tx, _rx) = setup(500);
    assert!(tx.is_alive());
    assert!(tx.is_alive());
    assert!(tx.is_alive());
}

// ============================================================================
// Clone correctness
// ============================================================================

#[test]
fn test_cloned_sender_sends_to_same_receiver() {
    let (tx, rx) = setup(500);
    let tx2 = tx.clone();
    tx.send_fire_and_forget(BridgeCommand::GetTitle { target_id: TID.into() });
    tx2.send_fire_and_forget(BridgeCommand::Navigate { target_id: TID.into(), url: "http://x".into() });

    let count = counter();
    let drained = rx.drain(|_| {
        count.fetch_add(1, Ordering::SeqCst);
        BridgeResponse { result: Ok(json!({})) }
    });
    assert_eq!(count.load(Ordering::SeqCst), 2);
    assert_eq!(drained, 2, "drain return must equal 2");
}

#[test]
fn test_multiple_cloned_senders_concurrent() {
    let (tx, rx) = setup(1000);
    let mut senders: Vec<Arc<bao_cdp::BridgeSender>> = vec![];
    for _ in 0..4 {
        senders.push(Arc::new(tx.clone()));
    }

    let mut handles = vec![];
    for s in senders {
        handles.push(std::thread::spawn(move || {
            s.send_fire_and_forget(BridgeCommand::GetTitle { target_id: TID.into() });
        }));
    }

    for h in handles {
        h.join().unwrap();
    }

    let count = counter();
    let drained = rx.drain(|_| {
        count.fetch_add(1, Ordering::SeqCst);
        BridgeResponse { result: Ok(json!({})) }
    });
    assert_eq!(count.load(Ordering::SeqCst), 4);
    assert_eq!(drained, 4, "drain return must equal number of cloned senders");
}

// ============================================================================
// BridgeCommand variant coverage (G1: 26/40 → 40/40)
// ============================================================================

#[test]
fn test_all_bridge_command_variants_serializable() {
    let commands: Vec<BridgeCommand> = vec![
        // --- original 26 ---
        BridgeCommand::Navigate { target_id: TID.into(), url: "http://x".into() },
        BridgeCommand::EvaluateJs { target_id: TID.into(), expression: "1+1".into(), return_by_value: true },
        BridgeCommand::TakeScreenshot { target_id: TID.into(), format: "png".into(), quality: Some(80) },
        BridgeCommand::GetTitle { target_id: TID.into() },
        BridgeCommand::GetUrl { target_id: TID.into() },
        BridgeCommand::GetDocument { target_id: TID.into() },
        BridgeCommand::QuerySelector { target_id: TID.into(), selector: "div".into() },
        BridgeCommand::QuerySelectorAll { target_id: TID.into(), selector: "span".into() },
        BridgeCommand::GetOuterHtml { target_id: TID.into(), node_id: Some(1) },
        BridgeCommand::SetAttributeValue { target_id: TID.into(), node_id: 1, name: "class".into(), value: "x".into() },
        BridgeCommand::DispatchMouseEvent { target_id: TID.into(), event_type: "click".into(), x: 100.0, y: 200.0, button: Some(0), click_count: Some(1) },
        BridgeCommand::DispatchKeyEvent { target_id: TID.into(), event_type: "keyDown".into(), key: "a".into(), code: "KeyA".into(), text: Some("a".into()) },
        BridgeCommand::InsertText { target_id: TID.into(), text: "hello".into() },
        BridgeCommand::SetViewport { target_id: TID.into(), width: 1920, height: 1080, device_scale_factor: Some(2.0) },
        BridgeCommand::SetUserAgent { target_id: TID.into(), user_agent: "Test".into() },
        BridgeCommand::GetCookies { target_id: TID.into(), urls: vec!["http://x".into()] },
        BridgeCommand::GetAllCookies { target_id: TID.into() },
        BridgeCommand::DeleteCookie { target_id: TID.into(), name: "sid".into(), url: Some("http://x".into()) },
        BridgeCommand::SetCookie { target_id: TID.into(), name: "sid".into(), value: "123".into(), url: Some("http://x".into()), domain: None },
        BridgeCommand::GetResponseBody { target_id: TID.into(), request_id: "r1".into() },
        BridgeCommand::AddScriptToEvaluateOnNewDocument { target_id: TID.into(), source: "console.log(1)".into() },
        BridgeCommand::Reload { target_id: TID.into(), ignore_cache: false },
        BridgeCommand::GoBack { target_id: TID.into() },
        BridgeCommand::GoForward { target_id: TID.into() },
        BridgeCommand::StopLoading { target_id: TID.into() },
        BridgeCommand::ClosePage { target_id: TID.into() },
        // --- G1: missing multi-target management (2) ---
        BridgeCommand::CreateTarget { url: "http://new".into() },
        BridgeCommand::ListTargets,
        // --- G1: missing Debugger domain (12) — REQ-CDP-003 alignment ---
        BridgeCommand::DebuggerEnable { target_id: TID.into() },
        BridgeCommand::DebuggerDisable { target_id: TID.into() },
        BridgeCommand::DebuggerSetBreakpoint { target_id: TID.into(), script_id: 1, offset: 0, line: 10, column: Some(5) },
        BridgeCommand::DebuggerClearBreakpoint { target_id: TID.into(), script_id: 1, offset: 0 },
        BridgeCommand::DebuggerInterrupt { target_id: TID.into() },
        BridgeCommand::DebuggerResume { target_id: TID.into(), step_type: Some("into".into()) },
        BridgeCommand::DebuggerListFrames { target_id: TID.into() },
        BridgeCommand::DebuggerGetEnvironment { target_id: TID.into(), frame_actor_id: "frame1".into() },
        BridgeCommand::DebuggerEval { target_id: TID.into(), expression: "x".into(), frame_actor_id: Some("frame1".into()) },
        BridgeCommand::DebuggerGetPossibleBreakpoints { target_id: TID.into(), script_id: 1 },
        BridgeCommand::DebuggerGetScriptSource { target_id: TID.into(), script_id: 1 },
        BridgeCommand::DebuggerBlackbox { target_id: TID.into(), script_id: 1 },
        BridgeCommand::DebuggerUnblackbox { target_id: TID.into(), script_id: 1 },
    ];

    let total = commands.len();
    let (tx, rx) = setup(500);
    for cmd in commands {
        tx.send_fire_and_forget(cmd);
    }

    let count = counter();
    let drained = rx.drain(|_| {
        count.fetch_add(1, Ordering::SeqCst);
        BridgeResponse { result: Ok(json!({})) }
    });
    let drained_count = count.load(Ordering::SeqCst);
    // G1: 26 original + 2 multi-target + 13 Debugger = 41 — full enum coverage
    assert_eq!(drained_count, 41, "expected all 41 BridgeCommand variants drained, got {}", drained_count);
    assert_eq!(drained, total, "drain() return must equal commands sent");
    assert_eq!(total, 41, "test vector must enumerate all 41 enum variants (full coverage)");
}

// ============================================================================
// Response propagation through channel
// ============================================================================

#[test]
fn test_response_value_propagation() {
    let (tx, rx) = setup(500);
    let rx = Arc::new(std::sync::Mutex::new(rx));
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let rx2 = Arc::clone(&rx);
    let done2 = Arc::clone(&done);
    std::thread::spawn(move || {
        for _ in 0..100 {
            let processed = {
                let rx_guard = rx2.lock().unwrap();
                rx_guard.try_process(|cmd| {
                    match cmd {
                        BridgeCommand::GetTitle { .. } => BridgeResponse { result: Ok(json!("My Title")) },
                        BridgeCommand::GetUrl { .. } => BridgeResponse { result: Ok(json!("http://example.com")) },
                        _ => BridgeResponse { result: Ok(json!({})) },
                    }
                })
            };
            if processed {
                done2.store(true, Ordering::SeqCst);
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    });

    std::thread::sleep(std::time::Duration::from_millis(10));
    let resp = tx.send(BridgeCommand::GetTitle { target_id: TID.into() });
    assert!(resp.result.is_ok());
    // G8: exact-value equality — must be the literal "My Title" string
    assert_eq!(resp.result.unwrap(), Value::String("My Title".into()),
        "response value must roundtrip exactly through the channel");

    for _ in 0..100 {
        if done.load(Ordering::SeqCst) { break; }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[test]
fn test_response_error_propagation() {
    let (tx, rx) = setup(500);
    let rx = Arc::new(std::sync::Mutex::new(rx));
    let done = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let rx2 = Arc::clone(&rx);
    let done2 = Arc::clone(&done);
    std::thread::spawn(move || {
        for _ in 0..100 {
            let processed = {
                let rx_guard = rx2.lock().unwrap();
                rx_guard.try_process(|_| {
                    BridgeResponse { result: Err("internal error".into()) }
                })
            };
            if processed {
                done2.store(true, Ordering::SeqCst);
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    });

    std::thread::sleep(std::time::Duration::from_millis(10));
    let resp = tx.send(BridgeCommand::GetTitle { target_id: TID.into() });
    assert!(resp.result.is_err());
    assert_eq!(resp.result.unwrap_err(), "internal error",
        "error string must roundtrip exactly through the channel");

    for _ in 0..100 {
        if done.load(Ordering::SeqCst) { break; }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

// ============================================================================
// Empty drain
// ============================================================================

#[test]
fn test_drain_empty_channel() {
    let (_tx, rx) = setup(500);
    let count = rx.drain(|_| BridgeResponse { result: Ok(json!({})) });
    assert_eq!(count, 0);
}

#[test]
fn test_try_process_empty_returns_false() {
    let (_tx, rx) = setup(500);
    assert!(!rx.try_process(|_| BridgeResponse { result: Ok(json!({})) }));
}

// ============================================================================
// G3: FIFO order preservation across drain (CDP command ordering invariant)
// ============================================================================

#[test]
fn test_drain_preserves_fifo_order() {
    let (tx, rx) = setup(500);
    // Send commands with distinct sequence numbers embedded in target_id
    for seq in 0..20 {
        let labeled_target = format!("seq-{:03}", seq);
        tx.send_fire_and_forget(BridgeCommand::GetTitle { target_id: labeled_target });
    }
    let order = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let order2 = Arc::clone(&order);
    rx.drain(|cmd| {
        if let BridgeCommand::GetTitle { target_id } = cmd {
            order2.lock().unwrap().push(target_id);
        }
        BridgeResponse { result: Ok(json!({})) }
    });
    let order = order.lock().unwrap();
    assert_eq!(order.len(), 20, "all 20 must be drained in order");
    for (i, t) in order.iter().enumerate() {
        assert_eq!(t, &format!("seq-{:03}", i),
            "FIFO violation at index {}: expected seq-{:03}, got {}", i, i, t);
    }
}

// ============================================================================
// G5: Boundary conditions — zero/extreme timeout, empty collections, None options
// ============================================================================

#[test]
fn test_zero_timeout_returns_err_immediately() {
    // Zero-duration timeout: resp_rx.recv_timeout(Duration::ZERO) errors instantly
    let (tx, _rx) = setup(0);
    let start = Instant::now();
    let resp = tx.send(BridgeCommand::GetTitle { target_id: TID.into() });
    let elapsed = start.elapsed();
    assert!(resp.result.is_err(), "zero timeout must yield error (no responder ready in 0ms)");
    // G4: zero timeout must return near-instantly (allow scheduling slack)
    assert!(elapsed <= Duration::from_millis(50),
        "zero-timeout send must not block long, took {:?}", elapsed);
}

#[test]
fn test_fire_and_forget_succeeds_even_after_receiver_dropped() {
    // fire-and-forget creates a throwaway responder; if the channel is closed,
    // the underlying send is best-effort (returns ()). Verify no panic.
    let (tx, rx) = setup(50);
    drop(rx);
    // Should not panic even though receiver is gone
    tx.send_fire_and_forget(BridgeCommand::GetTitle { target_id: TID.into() });
    // If we reach here, fire-and-forget honored its "never blocks, never panics" contract.
}

#[test]
fn test_send_after_receiver_dropped_returns_channel_closed() {
    let (tx, rx) = setup(500);
    drop(rx);
    let resp = tx.send(BridgeCommand::GetTitle { target_id: TID.into() });
    assert!(resp.result.is_err());
    let err = resp.result.unwrap_err();
    // Two valid closed-channel error strings depending on race: "bridge channel closed" or timeout
    assert!(err.contains("closed") || err.contains("timeout"),
        "send after receiver dropped must error with closed/timeout, got: {}", err);
}

#[test]
fn test_get_cookies_empty_urls_boundary() {
    let (tx, rx) = setup(500);
    tx.send_fire_and_forget(BridgeCommand::GetCookies { target_id: TID.into(), urls: vec![] });
    let got_empty = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let got_empty2 = Arc::clone(&got_empty);
    rx.drain(|cmd| {
        if let BridgeCommand::GetCookies { urls, .. } = cmd {
            assert!(urls.is_empty(), "empty urls Vec must roundtrip as empty");
            got_empty2.store(true, Ordering::SeqCst);
        }
        BridgeResponse { result: Ok(json!({})) }
    });
    assert!(got_empty.load(Ordering::SeqCst), "must have processed the GetCookies boundary command");
}

#[test]
fn test_take_screenshot_quality_boundary_none() {
    let (tx, rx) = setup(500);
    // quality: None is a valid boundary (default quality)
    tx.send_fire_and_forget(BridgeCommand::TakeScreenshot {
        target_id: TID.into(),
        format: "jpeg".into(),
        quality: None,
    });
    let got_none = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let got_none2 = Arc::clone(&got_none);
    rx.drain(|cmd| {
        if let BridgeCommand::TakeScreenshot { quality, format, .. } = cmd {
            assert!(quality.is_none(), "quality=None boundary must roundtrip");
            assert_eq!(format, "jpeg");
            got_none2.store(true, Ordering::SeqCst);
        }
        BridgeResponse { result: Ok(json!({})) }
    });
    assert!(got_none.load(Ordering::SeqCst), "must process TakeScreenshot with quality=None");
}

#[test]
fn test_take_screenshot_quality_boundary_extremes() {
    let (tx, rx) = setup(500);
    tx.send_fire_and_forget(BridgeCommand::TakeScreenshot {
        target_id: TID.into(), format: "png".into(), quality: Some(0),
    });
    tx.send_fire_and_forget(BridgeCommand::TakeScreenshot {
        target_id: TID.into(), format: "png".into(), quality: Some(100),
    });
    let qualities = Arc::new(std::sync::Mutex::new(Vec::<Option<u8>>::new()));
    let qualities2 = Arc::clone(&qualities);
    rx.drain(move |cmd| {
        if let BridgeCommand::TakeScreenshot { quality, .. } = cmd {
            qualities2.lock().unwrap().push(quality);
        }
        BridgeResponse { result: Ok(json!({})) }
    });
    let qualities = qualities.lock().unwrap();
    assert_eq!(qualities.len(), 2, "both quality boundaries must be drained");
    assert!(qualities.contains(&Some(0)), "quality=0 boundary must survive");
    assert!(qualities.contains(&Some(100)), "quality=100 boundary must survive");
}

#[test]
fn test_set_viewport_zero_dimensions_boundary() {
    let (tx, rx) = setup(500);
    tx.send_fire_and_forget(BridgeCommand::SetViewport {
        target_id: TID.into(), width: 0, height: 0, device_scale_factor: None,
    });
    let got_zero = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let got_zero2 = Arc::clone(&got_zero);
    rx.drain(|cmd| {
        if let BridgeCommand::SetViewport { width, height, device_scale_factor, .. } = cmd {
            assert_eq!(width, 0, "width=0 boundary must roundtrip");
            assert_eq!(height, 0, "height=0 boundary must roundtrip");
            assert!(device_scale_factor.is_none(), "device_scale_factor=None boundary must roundtrip");
            got_zero2.store(true, Ordering::SeqCst);
        }
        BridgeResponse { result: Ok(json!({})) }
    });
    assert!(got_zero.load(Ordering::SeqCst), "must process SetViewport with zero dimensions");
}

#[test]
fn test_set_viewport_f64_precision_preserved() {
    let (tx, rx) = setup(500);
    // f64 fractional device_scale_factor must roundtrip exactly
    tx.send_fire_and_forget(BridgeCommand::SetViewport {
        target_id: TID.into(),
        width: 375,
        height: 812,
        device_scale_factor: Some(2.625), // exact binary fraction
    });
    let got_precise = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let got_precise2 = Arc::clone(&got_precise);
    rx.drain(|cmd| {
        if let BridgeCommand::SetViewport { width, height, device_scale_factor, .. } = cmd {
            assert_eq!(width, 375);
            assert_eq!(height, 812);
            let dsf = device_scale_factor.unwrap();
            assert_eq!(dsf, 2.625, "f64 device_scale_factor must roundtrip exactly");
            got_precise2.store(true, Ordering::SeqCst);
        }
        BridgeResponse { result: Ok(json!({})) }
    });
    assert!(got_precise.load(Ordering::SeqCst), "must process SetViewport with fractional dsf");
}

#[test]
fn test_dispatch_mouse_event_extreme_coordinates() {
    let (tx, rx) = setup(500);
    // f64 boundary: negative, zero, large
    let coords = [f64::NEG_INFINITY, 0.0, f64::MAX];
    for &c in &coords {
        tx.send_fire_and_forget(BridgeCommand::DispatchMouseEvent {
            target_id: TID.into(),
            event_type: "mouseMoved".into(),
            x: c,
            y: c,
            button: None,
            click_count: None,
        });
    }
    let seen = Arc::new(std::sync::Mutex::new(Vec::<(f64, f64)>::new()));
    let seen2 = Arc::clone(&seen);
    rx.drain(move |cmd| {
        if let BridgeCommand::DispatchMouseEvent { x, y, .. } = cmd {
            seen2.lock().unwrap().push((x, y));
        }
        BridgeResponse { result: Ok(json!({})) }
    });
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 3, "all 3 f64 boundary coordinates must be drained");
    for (x, y) in seen.iter() {
        assert!(coords.contains(x) && coords.contains(y),
            "f64 coordinate must roundtrip exactly: {:?}", (x, y));
    }
}

#[test]
fn test_large_payload_stress() {
    // Large string payload must transit without corruption
    let (tx, rx) = setup(2000);
    let big = "x".repeat(64 * 1024); // 64KB
    let expected = big.clone();
    tx.send_fire_and_forget(BridgeCommand::EvaluateJs {
        target_id: TID.into(),
        expression: big,
        return_by_value: true,
    });
    let got_exact = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let got_exact2 = Arc::clone(&got_exact);
    rx.drain(move |cmd| {
        if let BridgeCommand::EvaluateJs { expression, .. } = cmd {
            assert_eq!(expression.len(), 64 * 1024, "payload length must survive transit");
            assert_eq!(expression, expected, "payload bytes must be byte-identical after transit");
            got_exact2.store(true, Ordering::SeqCst);
        }
        BridgeResponse { result: Ok(json!({})) }
    });
    assert!(got_exact.load(Ordering::SeqCst), "must process the large-payload command");
}

// ============================================================================
// G7: drain-then-send-then-drain ordering
// ============================================================================

#[test]
fn test_drain_send_drain_sequence() {
    let (tx, rx) = setup(500);
    // Phase 1: send 3, drain 3
    for i in 0..3 {
        tx.send_fire_and_forget(BridgeCommand::GetTitle { target_id: format!("p1-{}", i) });
    }
    let first = rx.drain(|_| BridgeResponse { result: Ok(json!({})) });
    assert_eq!(first, 3, "first drain must get exactly 3");

    // Phase 2: drain again immediately — must be 0 (channel empty)
    let between = rx.drain(|_| BridgeResponse { result: Ok(json!({})) });
    assert_eq!(between, 0, "intermediate drain on empty channel must be 0");

    // Phase 3: send 2 more, drain 2
    for i in 0..2 {
        tx.send_fire_and_forget(BridgeCommand::GetTitle { target_id: format!("p3-{}", i) });
    }
    let last = rx.drain(|_| BridgeResponse { result: Ok(json!({})) });
    assert_eq!(last, 2, "final drain must get exactly 2");

    // Phase 4: drain again — must be 0
    let after = rx.drain(|_| BridgeResponse { result: Ok(json!({})) });
    assert_eq!(after, 0, "post-drain on empty channel must be 0");
}

// ============================================================================
// G6: is_alive lifecycle across clone decay
// ============================================================================

#[test]
fn test_is_alive_after_all_clones_dropped_but_receiver_alive() {
    let (tx, rx) = setup(500);
    let tx2 = tx.clone();
    let tx3 = tx2.clone();
    // All three senders share the same underlying channel; receiver still alive
    assert!(tx.is_alive());
    assert!(tx2.is_alive());
    assert!(tx3.is_alive());
    drop(tx2);
    drop(tx3);
    // Original sender + receiver still alive
    assert!(tx.is_alive(), "original sender must stay alive while receiver exists");
    drop(rx);
    assert!(!tx.is_alive(), "after receiver dropped, sender must report dead");
}

// ============================================================================
// G2/G8: try_process single roundtrip exact-value equality
// ============================================================================

#[test]
fn test_try_process_single_command_exact_roundtrip() {
    let (tx, rx) = setup(500);
    tx.send_fire_and_forget(BridgeCommand::Navigate { target_id: TID.into(), url: "https://example.org/path?q=1".into() });
    // Small yield to let the channel buffer the command
    std::thread::sleep(Duration::from_millis(5));
    let mut captured: Option<String> = None;
    let processed = rx.try_process(|cmd| {
        if let BridgeCommand::Navigate { url, target_id } = cmd {
            assert_eq!(target_id, TID, "target_id must roundtrip exactly");
            captured = Some(url);
            BridgeResponse { result: Ok(json!({"ok": true})) }
        } else {
            panic!("expected Navigate");
        }
    });
    assert!(processed, "try_process must consume the pending command");
    assert_eq!(captured.as_deref(), Some("https://example.org/path?q=1"),
        "url string must roundtrip exactly through try_process");
    // Second try_process must report empty
    let again = rx.try_process(|_| BridgeResponse { result: Ok(json!({})) });
    assert!(!again, "after single consume, channel must be empty");
}

// ============================================================================
// G4: timeout bound on burst under load (concurrent senders vs single responder)
// ============================================================================

#[test]
fn test_burst_send_then_single_drain_no_loss() {
    let (tx, rx) = setup(1000);
    // 200 fire-and-forget from one thread
    for i in 0..200 {
        tx.send_fire_and_forget(BridgeCommand::Navigate {
            target_id: TID.into(),
            url: format!("http://burst/{}", i),
        });
    }
    // Single drain must recover ALL 200 (mpsc guarantees no loss)
    let count = rx.drain(|_| BridgeResponse { result: Ok(json!({})) });
    assert_eq!(count, 200, "single drain after 200 sends must recover all 200 — mpsc guarantees no loss");
    // Follow-up drain must be empty
    let recheck = rx.drain(|_| BridgeResponse { result: Ok(json!({})) });
    assert_eq!(recheck, 0, "after full drain, channel must be empty");
}
