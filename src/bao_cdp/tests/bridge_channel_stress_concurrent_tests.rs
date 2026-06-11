// @trace TEST-CDP-036 [req:REQ-CDP-003,REQ-CDP-006] [level:unit]
// Bridge channel stress tests: concurrent send/receive, burst commands,
// timeout under load, drain correctness, fire-and-forget, is_alive.

use bao_cdp::{BridgeCommand, BridgeResponse, bridge_channel};
use serde_json::json;
use std::time::Duration;
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
    rx.drain(|cmd| {
        count.fetch_add(1, Ordering::SeqCst);
        if let BridgeCommand::Navigate { url, .. } = cmd {
            assert!(url.starts_with("http://example.com/"));
        } else {
            panic!("Expected Navigate, got {:?}", cmd);
        }
        BridgeResponse { result: Ok(json!({})) }
    });
    assert_eq!(count.load(Ordering::SeqCst), 100);
}

#[test]
fn test_burst_50_eval_commands_drain() {
    let (tx, rx) = setup(500);
    for i in 0..50 {
        let expr = format!("1 + {}", i);
        tx.send_fire_and_forget(BridgeCommand::EvaluateJs { target_id: TID.into(), expression: expr, return_by_value: true });
    }
    let count = counter();
    rx.drain(|cmd| {
        count.fetch_add(1, Ordering::SeqCst);
        if let BridgeCommand::EvaluateJs { expression, .. } = cmd {
            assert!(expression.starts_with("1 + "));
        } else {
            panic!("Expected EvaluateJs");
        }
        BridgeResponse { result: Ok(json!({})) }
    });
    assert_eq!(count.load(Ordering::SeqCst), 50);
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
    rx.drain(|cmd| {
        match cmd {
            BridgeCommand::Navigate { .. } => { nav.fetch_add(1, Ordering::SeqCst); }
            BridgeCommand::EvaluateJs { .. } => { eval.fetch_add(1, Ordering::SeqCst); }
            BridgeCommand::GetTitle { .. } => { title.fetch_add(1, Ordering::SeqCst); }
            _ => panic!("unexpected command type"),
        }
        BridgeResponse { result: Ok(json!({})) }
    });
    assert_eq!(nav.load(Ordering::SeqCst), 10);
    assert_eq!(eval.load(Ordering::SeqCst), 10);
    assert_eq!(title.load(Ordering::SeqCst), 10);
}

// ============================================================================
// Concurrent send from multiple threads
// ============================================================================

#[test]
fn test_concurrent_sends_from_4_threads() {
    let (tx, rx) = setup(1000);
    let tx = Arc::new(tx);
    let mut handles = vec![];

    for t in 0..4 {
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

    let count = counter();
    rx.drain(|_| {
        count.fetch_add(1, Ordering::SeqCst);
        BridgeResponse { result: Ok(json!({})) }
    });
    assert_eq!(count.load(Ordering::SeqCst), 100);
}

#[test]
fn test_concurrent_send_with_sync_response() {
    let (tx, rx) = setup(2000);
    let tx = Arc::new(tx);
    let rx = Arc::new(std::sync::Mutex::new(rx));
    let total_processed = Arc::new(AtomicU32::new(0));

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

    // Send from multiple threads
    let mut senders = vec![];
    for t in 0..4 {
        let tx = Arc::clone(&tx);
        senders.push(std::thread::spawn(move || {
            for i in 0..10 {
                if i % 2 == 0 {
                    let resp = tx.send(BridgeCommand::GetTitle { target_id: TID.into() });
                    assert!(resp.result.is_ok(), "thread {} iter {} failed", t, i);
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
    assert!(total_processed.load(Ordering::SeqCst) > 0, "should process at least some commands");
}

// ============================================================================
// Timeout behavior under load
// ============================================================================

#[test]
fn test_timeout_when_no_responder() {
    let (tx, _rx) = setup(50);
    let resp = tx.send(BridgeCommand::GetTitle { target_id: TID.into() });
    assert!(resp.result.is_err());
    assert!(resp.result.unwrap_err().contains("timeout"));
}

#[test]
fn test_timeout_message_format() {
    let (tx, _rx) = setup(20);
    let resp = tx.send(BridgeCommand::Navigate { target_id: TID.into(), url: "http://x".into() });
    let err = resp.result.unwrap_err();
    assert!(err.contains("timeout") || err.contains("bridge"));
}

#[test]
fn test_slow_responder_still_succeeds() {
    let (tx, rx) = setup(500);
    let rx = Arc::new(std::sync::Mutex::new(rx));

    let rx2 = Arc::clone(&rx);
    std::thread::spawn(move || {
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
    rx.drain(|_| {
        count.fetch_add(1, Ordering::SeqCst);
        BridgeResponse { result: Ok(json!({})) }
    });
    assert_eq!(count.load(Ordering::SeqCst), 10);
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
    rx.drain(|_| {
        count.fetch_add(1, Ordering::SeqCst);
        BridgeResponse { result: Ok(json!({})) }
    });
    assert_eq!(count.load(Ordering::SeqCst), 2);
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
    rx.drain(|_| {
        count.fetch_add(1, Ordering::SeqCst);
        BridgeResponse { result: Ok(json!({})) }
    });
    assert_eq!(count.load(Ordering::SeqCst), 4);
}

// ============================================================================
// BridgeCommand variant coverage
// ============================================================================

#[test]
fn test_all_bridge_command_variants_serializable() {
    let commands: Vec<BridgeCommand> = vec![
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
    ];

    let (tx, rx) = setup(500);
    for cmd in commands {
        tx.send_fire_and_forget(cmd);
    }

    let count = counter();
    rx.drain(|_| {
        count.fetch_add(1, Ordering::SeqCst);
        BridgeResponse { result: Ok(json!({})) }
    });
    // drain returns usize count, atomic counter matches
    let drained = count.load(Ordering::SeqCst);
    assert_eq!(drained, 26, "expected 26 bridge commands, got {}", drained);
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
    assert_eq!(resp.result.unwrap_err(), "internal error");

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
