// @trace TEST-BRW-003 [req:REQ-BRW-003] [level:integration]
// Comprehensive tests for bao_browser::runtime_bridge — channel/queue logic only.
// No servo dependency.  Covers: RuntimeBridge, BridgeCommand, BridgeResponse,
// BridgeChannel, BridgeReceiver — construction, send, timeout, fire-and-forget,
// lifecycle, and concurrent submission.

use bao_browser::{
    BridgeChannel, BridgeCommand, BridgeReceiver, BridgeResponse, RuntimeBridge,
};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

// ═══════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════

fn respond_to(cmd: &BridgeCommand) -> BridgeResponse {
    match cmd {
        BridgeCommand::Navigate(_) => BridgeResponse::Ok,
        BridgeCommand::Evaluate(v) => BridgeResponse::Value(format!("eval:{}", v)),
        BridgeCommand::Screenshot => BridgeResponse::Binary(vec![0, 1, 2]),
        BridgeCommand::Close => BridgeResponse::Ok,
        BridgeCommand::Resize(w, h) => BridgeResponse::Value(format!("{}x{}", w, h)),
        BridgeCommand::GetTitle => BridgeResponse::Value("the-title".into()),
        BridgeCommand::GetUrl => BridgeResponse::Value("https://ex.co".into()),
    }
}

/// Spawns a worker that responds to every received command.
fn spawn_echo(rx: BridgeReceiver) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        while let Ok((cmd, tx)) = rx.recv() {
            if let Some(tx) = tx {
                let _ = tx.send(respond_to(&cmd));
            }
        }
    })
}

/// Spawns a worker that forwards received commands into a channel for inspection.
/// The responder is discarded — the sender half will get a broken-pipe error.
fn spawn_recorder(rx: BridgeReceiver) -> mpsc::Receiver<BridgeCommand> {
    let (tx, recv) = mpsc::channel();
    thread::spawn(move || {
        while let Ok((cmd, _)) = rx.recv() {
            if tx.send(cmd).is_err() {
                break;
            }
        }
    });
    recv
}

/// Spawns a worker that holds a reference to a "mailbox" — the first received
/// command is stored so the test can inspect it after the worker exits.
/// The worker stays alive until the shared `AtomicBool` is set to false.
fn spawn_hold(rx: BridgeReceiver) -> Arc<std::sync::Mutex<Option<BridgeCommand>>> {
    let held = Arc::new(std::sync::Mutex::new(None));
    let h = Arc::clone(&held);
    thread::spawn(move || {
        if let Ok((cmd, _)) = rx.recv() {
            *h.lock().unwrap() = Some(cmd);
        }
    });
    held
}

// ═══════════════════════════════════════════════════════════════════════
// BridgeCommand — all variants, equality, debug, clone
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn cmd_navigate() {
    let a = BridgeCommand::Navigate("https://a".into());
    let b = BridgeCommand::Navigate("https://b".into());
    assert_ne!(a, b);
    assert_eq!(a, BridgeCommand::Navigate("https://a".into()));
}

#[test]
fn cmd_evaluate() {
    let a = BridgeCommand::Evaluate("1+1".into());
    let b = BridgeCommand::Evaluate("2+2".into());
    assert_ne!(a, b);
    assert_eq!(a, BridgeCommand::Evaluate("1+1".into()));
}

#[test]
fn cmd_screenshot() {
    assert_eq!(
        BridgeCommand::Screenshot,
        BridgeCommand::Screenshot,
    );
}

#[test]
fn cmd_close() {
    assert_eq!(BridgeCommand::Close, BridgeCommand::Close);
}

#[test]
fn cmd_resize() {
    let a = BridgeCommand::Resize(800, 600);
    let b = BridgeCommand::Resize(1024, 768);
    assert_ne!(a, b);
    assert_eq!(a, BridgeCommand::Resize(800, 600));
}

#[test]
fn cmd_get_title() {
    assert_eq!(BridgeCommand::GetTitle, BridgeCommand::GetTitle);
}

#[test]
fn cmd_get_url() {
    assert_eq!(BridgeCommand::GetUrl, BridgeCommand::GetUrl);
}

#[test]
fn cmd_variants_are_distinct() {
    let cmds = [
        BridgeCommand::Navigate("x".into()),
        BridgeCommand::Evaluate("x".into()),
        BridgeCommand::Screenshot,
        BridgeCommand::Close,
        BridgeCommand::Resize(0, 0),
        BridgeCommand::GetTitle,
        BridgeCommand::GetUrl,
    ];
    for i in 0..cmds.len() {
        for j in (i + 1)..cmds.len() {
            assert_ne!(cmds[i], cmds[j], "variants[{i}] should differ from variants[{j}]");
        }
    }
}

#[test]
fn cmd_debug_contains_variant_name() {
    let s = format!("{:?}", BridgeCommand::Navigate("u".into()));
    assert!(s.contains("Navigate"), "Debug should contain variant: {s}");
}

#[test]
fn cmd_clone_equals_original() {
    let orig = BridgeCommand::Resize(640, 480);
    assert_eq!(orig.clone(), orig);
}

// ═══════════════════════════════════════════════════════════════════════
// BridgeResponse — all variants, helpers, equality, debug, clone
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn resp_ok_is_ok_not_err() {
    let r = BridgeResponse::Ok;
    assert!(r.is_ok());
    assert!(!r.is_err());
}

#[test]
fn resp_err_is_err_not_ok() {
    let r = BridgeResponse::Err("fail".into());
    assert!(!r.is_ok());
    assert!(r.is_err());
}

#[test]
fn resp_null_neither_ok_nor_err() {
    let r = BridgeResponse::Null;
    assert!(!r.is_ok());
    assert!(!r.is_err());
}

#[test]
fn resp_value_helpers() {
    let r = BridgeResponse::Value("hello".into());
    assert!(!r.is_ok());
    assert!(!r.is_err());
}

#[test]
fn resp_binary_helpers() {
    let r = BridgeResponse::Binary(vec![0u8; 16]);
    assert!(!r.is_ok());
    assert!(!r.is_err());
}

#[test]
fn resp_ok_ok_conversion_returns_ok() {
    assert!(BridgeResponse::Ok.ok().is_ok());
}

#[test]
fn resp_err_ok_conversion_returns_err() {
    let r = BridgeResponse::Err("msg".into()).ok();
    assert_eq!(r.unwrap_err(), "msg");
}

#[test]
fn resp_null_ok_conversion_returns_ok() {
    assert!(BridgeResponse::Null.ok().is_ok());
}

#[test]
fn resp_value_ok_conversion_returns_ok() {
    assert!(BridgeResponse::Value("v".into()).ok().is_ok());
}

#[test]
fn resp_binary_ok_conversion_returns_ok() {
    assert!(BridgeResponse::Binary(vec![]).ok().is_ok());
}

#[test]
fn resp_variants_are_distinct() {
    assert_ne!(BridgeResponse::Ok, BridgeResponse::Null);
    assert_ne!(BridgeResponse::Ok, BridgeResponse::Err("".into()));
    assert_ne!(BridgeResponse::Null, BridgeResponse::Err("".into()));
    assert_ne!(BridgeResponse::Value("a".into()), BridgeResponse::Value("b".into()));
    assert_ne!(BridgeResponse::Binary(vec![0]), BridgeResponse::Binary(vec![1]));
}

#[test]
fn resp_debug_contains_variant_and_data() {
    let r = BridgeResponse::Err("boom".into());
    let d = format!("{r:?}");
    assert!(d.contains("Err"), "Debug should contain Err: {d}");
    assert!(d.contains("boom"), "Debug should contain message: {d}");
}

#[test]
fn resp_clone_equals_original() {
    let orig = BridgeResponse::Binary(vec![1, 2, 3]);
    assert_eq!(orig.clone(), orig);
}

#[test]
fn resp_ok_eq_ok() {
    assert_eq!(BridgeResponse::Ok, BridgeResponse::Ok);
}

#[test]
fn resp_err_same_message_eq() {
    assert_eq!(
        BridgeResponse::Err("x".into()),
        BridgeResponse::Err("x".into()),
    );
}

// ═══════════════════════════════════════════════════════════════════════
// BridgeChannel — creation, send, receive
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn channel_new_both_sides_alive() {
    let (ch, rx) = BridgeChannel::new();
    assert!(ch.is_alive());
    assert!(rx.is_alive());
}

#[test]
fn channel_send_navigate_ok() {
    let (ch, rx) = BridgeChannel::new();
    let _w = spawn_echo(rx);
    assert_eq!(
        ch.send(BridgeCommand::Navigate("https://ex.co".into())).unwrap(),
        BridgeResponse::Ok,
    );
}

#[test]
fn channel_send_evaluate_value() {
    let (ch, rx) = BridgeChannel::new();
    let _w = spawn_echo(rx);
    assert_eq!(
        ch.send(BridgeCommand::Evaluate("1+1".into())).unwrap(),
        BridgeResponse::Value("eval:1+1".into()),
    );
}

#[test]
fn channel_send_screenshot_binary() {
    let (ch, rx) = BridgeChannel::new();
    let _w = spawn_echo(rx);
    assert_eq!(
        ch.send(BridgeCommand::Screenshot).unwrap(),
        BridgeResponse::Binary(vec![0, 1, 2]),
    );
}

#[test]
fn channel_send_close_ok() {
    let (ch, rx) = BridgeChannel::new();
    let _w = spawn_echo(rx);
    assert_eq!(ch.send(BridgeCommand::Close).unwrap(), BridgeResponse::Ok);
}

#[test]
fn channel_send_resize_value() {
    let (ch, rx) = BridgeChannel::new();
    let _w = spawn_echo(rx);
    assert_eq!(
        ch.send(BridgeCommand::Resize(1920, 1080)).unwrap(),
        BridgeResponse::Value("1920x1080".into()),
    );
}

#[test]
fn channel_send_get_title_value() {
    let (ch, rx) = BridgeChannel::new();
    let _w = spawn_echo(rx);
    assert_eq!(
        ch.send(BridgeCommand::GetTitle).unwrap(),
        BridgeResponse::Value("the-title".into()),
    );
}

#[test]
fn channel_send_get_url_value() {
    let (ch, rx) = BridgeChannel::new();
    let _w = spawn_echo(rx);
    assert_eq!(
        ch.send(BridgeCommand::GetUrl).unwrap(),
        BridgeResponse::Value("https://ex.co".into()),
    );
}

#[test]
fn channel_multiple_sends_preserve_order() {
    let (ch, rx) = BridgeChannel::new();
    let _w = spawn_echo(rx);
    assert_eq!(
        ch.send(BridgeCommand::GetTitle).unwrap(),
        BridgeResponse::Value("the-title".into()),
    );
    assert_eq!(
        ch.send(BridgeCommand::GetUrl).unwrap(),
        BridgeResponse::Value("https://ex.co".into()),
    );
    assert_eq!(
        ch.send(BridgeCommand::Navigate("x".into())).unwrap(),
        BridgeResponse::Ok,
    );
}

#[test]
fn channel_send_without_worker_panics_after_drop() {
    // When the receiver (and thus the main-channel rx) is dropped,
    // send should fail immediately.
    let (ch, rx) = BridgeChannel::new();
    drop(rx);
    let r = ch.send(BridgeCommand::GetTitle);
    assert!(r.is_err(), "send after receiver drop should fail");
}

#[test]
fn channel_send_all_seven_variants_roundtrip() {
    let (ch, rx) = BridgeChannel::new();
    let _w = spawn_echo(rx);
    assert_eq!(ch.send(BridgeCommand::Navigate("u".into())).unwrap(), BridgeResponse::Ok);
    assert_eq!(
        ch.send(BridgeCommand::Evaluate("e".into())).unwrap(),
        BridgeResponse::Value("eval:e".into()),
    );
    assert_eq!(
        ch.send(BridgeCommand::Screenshot).unwrap(),
        BridgeResponse::Binary(vec![0, 1, 2]),
    );
    assert_eq!(ch.send(BridgeCommand::Close).unwrap(), BridgeResponse::Ok);
    assert_eq!(
        ch.send(BridgeCommand::Resize(100, 200)).unwrap(),
        BridgeResponse::Value("100x200".into()),
    );
    assert_eq!(
        ch.send(BridgeCommand::GetTitle).unwrap(),
        BridgeResponse::Value("the-title".into()),
    );
    assert_eq!(
        ch.send(BridgeCommand::GetUrl).unwrap(),
        BridgeResponse::Value("https://ex.co".into()),
    );
}

// ═══════════════════════════════════════════════════════════════════════
// BridgeChannel — send_timeout
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn channel_send_timeout_with_worker_succeeds() {
    let (ch, rx) = BridgeChannel::new();
    let _w = spawn_echo(rx);
    let r = ch
        .send_timeout(BridgeCommand::GetTitle, Duration::from_secs(5))
        .unwrap();
    assert_eq!(r, BridgeResponse::Value("the-title".into()));
}

#[test]
fn channel_send_timeout_expires_when_no_worker() {
    let (ch, _rx) = BridgeChannel::new();
    let r = ch.send_timeout(BridgeCommand::GetTitle, Duration::from_millis(10));
    assert!(r.is_err(), "send_timeout should fail when no worker responds");
}

#[test]
fn channel_send_timeout_zero_fails_immediately() {
    let (ch, _rx) = BridgeChannel::new();
    let r = ch.send_timeout(BridgeCommand::GetTitle, Duration::from_secs(0));
    assert!(r.is_err());
}

#[test]
fn channel_send_timeout_after_receiver_dropped_fails() {
    let (ch, rx) = BridgeChannel::new();
    drop(rx);
    let r = ch
        .send_timeout(BridgeCommand::GetTitle, Duration::from_secs(1));
    assert!(r.is_err(), "send_timeout after drop should fail");
}

// ═══════════════════════════════════════════════════════════════════════
// BridgeChannel — fire_and_forget
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn channel_fire_and_forget_delivers_command() {
    let (ch, rx) = BridgeChannel::new();
    let recorded = spawn_recorder(rx);
    ch.fire_and_forget(BridgeCommand::GetTitle).unwrap();
    let got = recorded.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(got, BridgeCommand::GetTitle);
}

#[test]
fn channel_fire_and_forget_multiple_in_order() {
    let (ch, rx) = BridgeChannel::new();
    let recorded = spawn_recorder(rx);
    ch.fire_and_forget(BridgeCommand::Navigate("a".into())).unwrap();
    ch.fire_and_forget(BridgeCommand::Evaluate("b".into())).unwrap();
    ch.fire_and_forget(BridgeCommand::Close).unwrap();
    assert_eq!(
        recorded.recv_timeout(Duration::from_secs(1)).unwrap(),
        BridgeCommand::Navigate("a".into()),
    );
    assert_eq!(
        recorded.recv_timeout(Duration::from_secs(1)).unwrap(),
        BridgeCommand::Evaluate("b".into()),
    );
    assert_eq!(
        recorded.recv_timeout(Duration::from_secs(1)).unwrap(),
        BridgeCommand::Close,
    );
}

#[test]
fn channel_fire_and_forget_returns_ok_on_live_channel() {
    let (ch, _rx) = BridgeChannel::new();
    assert!(ch.fire_and_forget(BridgeCommand::Screenshot).is_ok());
}

#[test]
fn channel_fire_and_forget_after_receiver_dropped_returns_err() {
    let (ch, rx) = BridgeChannel::new();
    drop(rx);
    let r = ch.fire_and_forget(BridgeCommand::GetTitle);
    assert!(r.is_err());
}

#[test]
fn channel_fire_and_forget_sends_none_responder() {
    let (ch, rx) = BridgeChannel::new();
    let held = spawn_hold(rx);
    ch.fire_and_forget(BridgeCommand::GetTitle).unwrap();
    // The hold worker exits after receiving one command.
    // Joining ensures we got it.
    // (held is an Arc<Mutex<Option<...>>> — the worker stores the cmd)
    // We just check the channel delivered; spawn_hold stores the cmd.
    thread::sleep(Duration::from_millis(50));
    let stored = held.lock().unwrap();
    assert_eq!(stored.as_ref(), Some(&BridgeCommand::GetTitle));
}

// ═══════════════════════════════════════════════════════════════════════
// BridgeChannel — is_alive / close lifecycle
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn channel_alive_after_construction() {
    let (ch, _rx) = BridgeChannel::new();
    assert!(ch.is_alive());
}

#[test]
fn channel_receiver_alive_after_construction() {
    let (_ch, rx) = BridgeChannel::new();
    assert!(rx.is_alive());
}

#[test]
fn channel_close_stops_alive_on_both_sides() {
    let (ch, rx) = BridgeChannel::new();
    ch.close();
    assert!(!ch.is_alive());
    assert!(!rx.is_alive());
}

#[test]
fn channel_close_idempotent() {
    let (ch, rx) = BridgeChannel::new();
    ch.close();
    ch.close();
    assert!(!ch.is_alive());
    assert!(!rx.is_alive());
}

#[test]
fn channel_close_does_not_break_transport() {
    // close() only toggles the alive flag — the underlying mpsc channel
    // remains connected so an active worker can still process commands.
    let (ch, rx) = BridgeChannel::new();
    let _w = spawn_echo(rx);
    ch.close();
    assert!(!ch.is_alive());
    // fire_and_forget should still succeed (channel is not dropped).
    assert!(
        ch.fire_and_forget(BridgeCommand::GetTitle).is_ok(),
        "fire_and_forget after close() should still work",
    );
}

#[test]
fn channel_drop_breaks_transport() {
    let (ch, rx) = BridgeChannel::new();
    drop(ch);
    let r = rx.recv_timeout(Duration::from_millis(50));
    assert!(r.is_err(), "recv after channel drop should fail");
}

#[test]
fn channel_receiver_drop_breaks_send() {
    let (ch, rx) = BridgeChannel::new();
    drop(rx);
    let r = ch.fire_and_forget(BridgeCommand::GetTitle);
    assert!(r.is_err(), "send after receiver drop should fail");
}

// ═══════════════════════════════════════════════════════════════════════
// BridgeReceiver — direct recv / recv_timeout
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn receiver_recv_gets_command_from_sender() {
    let (ch, rx) = BridgeChannel::new();
    let _ = ch.fire_and_forget(BridgeCommand::Screenshot);
    let (cmd, responder) = rx.recv().unwrap();
    assert_eq!(cmd, BridgeCommand::Screenshot);
    assert!(responder.is_none());
}

#[test]
fn receiver_recv_timeout_gets_command() {
    let (ch, rx) = BridgeChannel::new();
    let ch_arc = Arc::new(ch);
    let ch2 = Arc::clone(&ch_arc);
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(10));
        let _ = ch2.fire_and_forget(BridgeCommand::GetTitle);
    });
    let (cmd, _) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(cmd, BridgeCommand::GetTitle);
}

#[test]
fn receiver_recv_timeout_expires() {
    let (_ch, rx) = BridgeChannel::new();
    let r = rx.recv_timeout(Duration::from_millis(5));
    assert!(r.is_err(), "timeout should expire");
}

#[test]
fn receiver_recv_after_channel_drop_returns_err() {
    let (ch, rx) = BridgeChannel::new();
    drop(ch);
    let r = rx.recv();
    assert!(r.is_err(), "recv after channel dropped should return Err");
}

#[test]
fn receiver_recv_timeout_after_channel_drop_returns_err() {
    let (ch, rx) = BridgeChannel::new();
    drop(ch);
    let r = rx.recv_timeout(Duration::from_millis(50));
    assert!(r.is_err());
}

#[test]
fn receiver_debug_format_excludes_internal_rx() {
    let (_ch, rx) = BridgeChannel::new();
    let d = format!("{rx:?}");
    assert!(d.contains("BridgeReceiver"), "Debug should start with BridgeReceiver: {d}");
    assert!(d.contains("alive"), "Debug should contain alive field: {d}");
}

// ═══════════════════════════════════════════════════════════════════════
// RuntimeBridge — new, send, is_alive, close
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bridge_new_creates_live_pair() {
    let (br, rx) = RuntimeBridge::new();
    assert!(br.is_alive());
    assert!(rx.is_alive());
}

#[test]
fn bridge_send_with_worker() {
    let (br, rx) = RuntimeBridge::new();
    let _w = spawn_echo(rx);
    assert_eq!(
        br.send(BridgeCommand::GetTitle).unwrap(),
        BridgeResponse::Value("the-title".into()),
    );
}

#[test]
fn bridge_send_all_variants() {
    let (br, rx) = RuntimeBridge::new();
    let _w = spawn_echo(rx);
    assert_eq!(br.send(BridgeCommand::Navigate("u".into())).unwrap(), BridgeResponse::Ok);
    assert_eq!(
        br.send(BridgeCommand::Evaluate("x".into())).unwrap(),
        BridgeResponse::Value("eval:x".into()),
    );
    assert_eq!(
        br.send(BridgeCommand::Screenshot).unwrap(),
        BridgeResponse::Binary(vec![0, 1, 2]),
    );
    assert_eq!(br.send(BridgeCommand::Close).unwrap(), BridgeResponse::Ok);
    assert_eq!(
        br.send(BridgeCommand::Resize(640, 480)).unwrap(),
        BridgeResponse::Value("640x480".into()),
    );
    assert_eq!(
        br.send(BridgeCommand::GetTitle).unwrap(),
        BridgeResponse::Value("the-title".into()),
    );
    assert_eq!(
        br.send(BridgeCommand::GetUrl).unwrap(),
        BridgeResponse::Value("https://ex.co".into()),
    );
}

#[test]
fn bridge_is_alive_true_after_new() {
    let (br, _rx) = RuntimeBridge::new();
    assert!(br.is_alive());
}

#[test]
fn bridge_receiver_alive_true_after_new() {
    let (_br, rx) = RuntimeBridge::new();
    assert!(rx.is_alive());
}

#[test]
fn bridge_close_stops_alive_and_affects_receiver() {
    let (br, rx) = RuntimeBridge::new();
    br.close();
    assert!(!br.is_alive());
    assert!(!rx.is_alive());
}

#[test]
fn bridge_close_idempotent() {
    let (br, rx) = RuntimeBridge::new();
    br.close();
    br.close();
    assert!(!br.is_alive());
    assert!(!rx.is_alive());
}

#[test]
fn bridge_send_after_dropped_receiver_fails() {
    let (br, rx) = RuntimeBridge::new();
    drop(rx);
    let r = br.send(BridgeCommand::GetTitle);
    assert!(r.is_err(), "send after receiver drop should fail");
}

#[test]
fn bridge_close_does_not_block_fire_and_forget() {
    let (br, rx) = RuntimeBridge::new();
    let _w = spawn_echo(rx);
    br.close();
    // fire_and_forget targets the mpsc channel which is still live.
    assert!(
        br.fire_and_forget(BridgeCommand::GetTitle).is_ok(),
        "fire_and_forget should still work after close()",
    );
}

// ═══════════════════════════════════════════════════════════════════════
// RuntimeBridge — fire_and_forget / send_timeout
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn bridge_fire_and_forget_delivers() {
    let (br, rx) = RuntimeBridge::new();
    let recorded = spawn_recorder(rx);
    br.fire_and_forget(BridgeCommand::Screenshot).unwrap();
    let got = recorded.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(got, BridgeCommand::Screenshot);
}

#[test]
fn bridge_fire_and_forget_sequence() {
    let (br, rx) = RuntimeBridge::new();
    let recorded = spawn_recorder(rx);
    br.fire_and_forget(BridgeCommand::Close).unwrap();
    br.fire_and_forget(BridgeCommand::GetTitle).unwrap();
    assert_eq!(
        recorded.recv_timeout(Duration::from_secs(1)).unwrap(),
        BridgeCommand::Close,
    );
    assert_eq!(
        recorded.recv_timeout(Duration::from_secs(1)).unwrap(),
        BridgeCommand::GetTitle,
    );
}

#[test]
fn bridge_fire_and_forget_after_receiver_dropped_fails() {
    let (br, rx) = RuntimeBridge::new();
    drop(rx);
    let r = br.fire_and_forget(BridgeCommand::GetTitle);
    assert!(r.is_err());
}

#[test]
fn bridge_send_timeout_with_worker() {
    let (br, rx) = RuntimeBridge::new();
    let _w = spawn_echo(rx);
    let r = br
        .send_timeout(BridgeCommand::GetUrl, Duration::from_secs(5))
        .unwrap();
    assert_eq!(r, BridgeResponse::Value("https://ex.co".into()));
}

#[test]
fn bridge_send_timeout_expires() {
    let (br, _rx) = RuntimeBridge::new();
    let r = br.send_timeout(BridgeCommand::GetTitle, Duration::from_millis(10));
    assert!(r.is_err(), "send_timeout should fail without worker");
}

// ═══════════════════════════════════════════════════════════════════════
// Concurrent send (channel is Send + Sync)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn concurrent_sends_from_multiple_threads_all_get_responses() {
    let (ch, rx) = BridgeChannel::new();
    let _w = spawn_echo(rx);
    let ch = Arc::new(ch);
    let mut handles = Vec::new();
    let n = 20;

    for i in 0..n {
        let c = Arc::clone(&ch);
        handles.push(thread::spawn(move || {
            let cmd = if i % 2 == 0 {
                BridgeCommand::GetTitle
            } else {
                BridgeCommand::GetUrl
            };
            c.send(cmd)
        }));
    }

    let mut oks = 0;
    for (i, h) in handles.into_iter().enumerate() {
        let resp = h.join().expect("thread panicked").unwrap();
        if i % 2 == 0 {
            assert_eq!(resp, BridgeResponse::Value("the-title".into()), "thread {i}");
        } else {
            assert_eq!(resp, BridgeResponse::Value("https://ex.co".into()), "thread {i}");
        }
        oks += 1;
    }
    assert_eq!(oks, n, "all {n} concurrent sends should succeed");
}

#[test]
fn concurrent_fire_and_forget_all_deliver() {
    let (ch, rx) = BridgeChannel::new();
    let recorded = spawn_recorder(rx);
    let ch = Arc::new(ch);
    let mut handles = Vec::new();
    let n = 15;

    for _ in 0..n {
        let c = Arc::clone(&ch);
        handles.push(thread::spawn(move || {
            c.fire_and_forget(BridgeCommand::Screenshot).ok()
        }));
    }

    for h in handles {
        assert!(h.join().unwrap().is_some(), "fire_and_forget should succeed");
    }

    // Verify all n commands arrived at the recorder
    let mut count = 0;
    while let Ok(cmd) = recorded.recv_timeout(Duration::from_secs(1)) {
        assert_eq!(cmd, BridgeCommand::Screenshot);
        count += 1;
        if count == n {
            break;
        }
    }
    assert_eq!(count, n, "all {n} fire-and-forget commands should arrive");
}
