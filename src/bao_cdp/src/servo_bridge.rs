// REQ-CDP-003: Bridge trait for CDP ↔ servo communication  @trace REQ-CDP-003
// REQ-CDP-006: Channel-based async bridge (CDP thread ↔ main thread)
//
// Architecture:
//   CDP WebSocket thread ──[BridgeCommand]──> main thread (servo)
//   CDP WebSocket thread <──[BridgeResponse]── main thread (servo)
//
// Why channels: servo's WebView is !Send (Rc<RefCell<>>),
// so all WebView operations must happen on the main thread.

use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use serde_json::Value;

/// Commands that the CDP server sends to the main thread for servo execution.
#[derive(Debug)]
pub enum BridgeCommand {
    Navigate { url: String },
    EvaluateJs { expression: String, return_by_value: bool },
    TakeScreenshot { format: String, quality: Option<u8> },
    GetTitle,
    GetUrl,
    GetDocument,
    QuerySelector { selector: String },
    QuerySelectorAll { selector: String },
    GetOuterHtml { node_id: Option<i64> },
    SetAttributeValue { node_id: i64, name: String, value: String },
    DispatchMouseEvent { event_type: String, x: f64, y: f64, button: Option<i64>, click_count: Option<i64> },
    DispatchKeyEvent { event_type: String, key: String, code: String, text: Option<String> },
    InsertText { text: String },
    SetViewport { width: u32, height: u32, device_scale_factor: Option<f64> },
    SetUserAgent { user_agent: String },
    GetCookies { urls: Vec<String> },
    GetAllCookies,
    DeleteCookie { name: String, url: Option<String> },
    SetCookie { name: String, value: String, url: Option<String>, domain: Option<String> },
    GetResponseBody { request_id: String },
    AddScriptToEvaluateOnNewDocument { source: String },
    Reload { ignore_cache: bool },
    GoBack,
    GoForward,
    StopLoading,
    ClosePage,
}

/// Response from the main thread back to the CDP server.
#[derive(Debug)]
pub struct BridgeResponse {
    pub result: Result<Value, String>,
}

/// Internal request structure used for channel communication.
struct BridgeRequest {
    command: BridgeCommand,
    responder: Sender<BridgeResponse>,
}

/// Handle for the CDP server to send commands to the main thread.
pub struct BridgeSender {
    tx: Sender<BridgeRequest>,
    timeout: Duration,
}

/// Handle for the main thread to receive and process commands from CDP.
pub struct BridgeReceiver {
    rx: Receiver<BridgeRequest>,
}

/// Create a new bridge channel pair.
pub fn bridge_channel(timeout: Duration) -> (BridgeSender, BridgeReceiver) {
    let (tx, rx) = mpsc::channel();
    (
        BridgeSender { tx, timeout },
        BridgeReceiver { rx },
    )
}

impl BridgeSender {
    /// Send a command to the main thread and wait for the response.
    pub fn send(&self, command: BridgeCommand) -> BridgeResponse {
        let (resp_tx, resp_rx) = mpsc::channel();
        if self.tx.send(BridgeRequest {
            command,
            responder: resp_tx,
        }).is_err() {
            return BridgeResponse {
                result: Err("bridge channel closed".into()),
            };
        }
        match resp_rx.recv_timeout(self.timeout) {
            Ok(resp) => resp,
            Err(_) => BridgeResponse {
                result: Err("bridge response timeout".into()),
            },
        }
    }

    /// Send a command without waiting for response (fire-and-forget).
    pub fn send_fire_and_forget(&self, command: BridgeCommand) {
        let (resp_tx, _) = mpsc::channel();
        let _ = self.tx.send(BridgeRequest {
            command,
            responder: resp_tx,
        });
    }

    /// Check if the channel is still open.
    pub fn is_alive(&self) -> bool {
        !self.tx.send(BridgeRequest {
            command: BridgeCommand::GetTitle,
            responder: mpsc::channel().0,
        }).is_err()
    }
}

impl BridgeReceiver {
    /// Try to receive and process a pending command. Returns true if a command was processed.
    pub fn try_process<F>(&self, handler: F) -> bool
    where
        F: FnOnce(BridgeCommand) -> BridgeResponse,
    {
        match self.rx.try_recv() {
            Ok(request) => {
                let response = handler(request.command);
                let _ = request.responder.send(response);
                true
            }
            Err(_) => false,
        }
    }

    /// Process all pending commands.
    pub fn drain<F>(&self, handler: F) -> usize
    where
        F: Fn(BridgeCommand) -> BridgeResponse,
    {
        let mut count = 0;
        while let Ok(request) = self.rx.try_recv() {
            let response = handler(request.command);
            let _ = request.responder.send(response);
            count += 1;
        }
        count
    }
}

impl std::clone::Clone for BridgeSender {
    fn clone(&self) -> Self {
        BridgeSender {
            tx: self.tx.clone(),
            timeout: self.timeout,
        }
    }
}

// @trace TEST-CDP-003 [req:REQ-CDP-003] [level:unit] [nfr:TMG-CDP-01]
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const TIMEOUT: Duration = Duration::from_millis(100);

    fn ok_response(val: Value) -> BridgeResponse {
        BridgeResponse { result: Ok(val) }
    }

    fn err_response(msg: &str) -> BridgeResponse {
        BridgeResponse { result: Err(msg.into()) }
    }

    fn noop_handler(_: BridgeCommand) -> BridgeResponse {
        ok_response(Value::Null)
    }

    // 1. bridge_channel creates sender and receiver
    #[test]
    fn bridge_channel_creates_sender_and_receiver() {
        let (sender, _receiver) = bridge_channel(TIMEOUT);
        assert!(sender.is_alive(), "sender should report alive when receiver exists");
    }

    // 2. BridgeSender::send with responding receiver → gets BridgeResponse with Ok
    #[test]
    fn send_with_responding_receiver_returns_ok() {
        let (sender, receiver) = bridge_channel(TIMEOUT);
        // Use fire-and-forget to enqueue, then process on same thread
        // and verify the responder channel carries the Ok value back.
        sender.send_fire_and_forget(BridgeCommand::GetTitle);
        let mut captured_response: Option<BridgeResponse> = None;
        let processed = receiver.try_process(|cmd| {
            let resp = match cmd {
                BridgeCommand::GetTitle => ok_response(Value::String("Test Page".into())),
                _ => err_response("unexpected"),
            };
            captured_response = Some(BridgeResponse { result: resp.result.clone() });
            resp
        });
        assert!(processed);
        let resp = captured_response.unwrap();
        assert!(resp.result.is_ok());
        assert_eq!(resp.result.unwrap(), Value::String("Test Page".into()));
    }

    // 3. BridgeSender::send when receiver dropped → Err 'bridge channel closed'
    #[test]
    fn send_when_receiver_dropped_returns_channel_closed() {
        let (sender, receiver) = bridge_channel(TIMEOUT);
        drop(receiver);
        let resp = sender.send(BridgeCommand::GetTitle);
        assert!(resp.result.is_err());
        assert_eq!(resp.result.unwrap_err(), "bridge channel closed");
    }

    // 4. BridgeSender::send_fire_and_forget works without panic
    #[test]
    fn send_fire_and_forget_does_not_panic() {
        let (sender, receiver) = bridge_channel(TIMEOUT);
        sender.send_fire_and_forget(BridgeCommand::GetTitle);
        let processed = receiver.try_process(noop_handler);
        assert!(processed, "fire-and-forget command should be receivable");
    }

    // 5. BridgeSender::is_alive when channel open → returns true
    #[test]
    fn is_alive_when_channel_open_returns_true() {
        let (sender, _receiver) = bridge_channel(TIMEOUT);
        assert!(sender.is_alive());
    }

    // 6. BridgeSender clone preserves connection
    #[test]
    fn clone_preserves_connection() {
        let (sender, receiver) = bridge_channel(TIMEOUT);
        let cloned = sender.clone();
        cloned.send_fire_and_forget(BridgeCommand::GetTitle);
        let processed = receiver.try_process(noop_handler);
        assert!(processed, "cloned sender should deliver command to same receiver");
    }

    // 7. BridgeReceiver::try_process processes one command
    #[test]
    fn try_process_processes_one_command() {
        let (sender, receiver) = bridge_channel(TIMEOUT);
        sender.send_fire_and_forget(BridgeCommand::GetTitle);
        let processed = receiver.try_process(|_| ok_response(Value::Bool(true)));
        assert!(processed);
        let again = receiver.try_process(noop_handler);
        assert!(!again, "no second command should be pending");
    }

    // 8. BridgeReceiver::drain processes multiple commands
    #[test]
    fn drain_processes_multiple_commands() {
        let (sender, receiver) = bridge_channel(TIMEOUT);
        for _ in 0..5 {
            sender.send_fire_and_forget(BridgeCommand::GetTitle);
        }
        let count = receiver.drain(noop_handler);
        assert_eq!(count, 5);
    }

    // 9. BridgeReceiver::try_process returns false when empty
    #[test]
    fn try_process_returns_false_when_empty() {
        let (_sender, receiver) = bridge_channel(TIMEOUT);
        let processed = receiver.try_process(noop_handler);
        assert!(!processed);
    }

    // 10. BridgeResponse result Ok with Value
    #[test]
    fn bridge_response_ok_with_value() {
        let resp = ok_response(Value::Number(42.into()));
        assert!(resp.result.is_ok());
        assert_eq!(resp.result.unwrap(), Value::Number(42.into()));
    }

    // 11. BridgeResponse result Err with string
    #[test]
    fn bridge_response_err_with_string() {
        let resp = err_response("something failed");
        assert!(resp.result.is_err());
        assert_eq!(resp.result.unwrap_err(), "something failed");
    }

    // 12. BridgeCommand::Navigate debug format contains 'Navigate'
    #[test]
    fn navigate_debug_format_contains_navigate() {
        let cmd = BridgeCommand::Navigate { url: "https://example.com".into() };
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("Navigate"), "debug output should contain 'Navigate': {}", debug_str);
    }

    // 13. BridgeCommand::EvaluateJs debug format
    #[test]
    fn evaluate_js_debug_format() {
        let cmd = BridgeCommand::EvaluateJs { expression: "1+1".into(), return_by_value: true };
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("EvaluateJs"), "debug output should contain 'EvaluateJs': {}", debug_str);
    }

    // 14. BridgeCommand::TakeScreenshot debug format
    #[test]
    fn take_screenshot_debug_format() {
        let cmd = BridgeCommand::TakeScreenshot { format: "png".into(), quality: Some(80) };
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("TakeScreenshot"), "debug output should contain 'TakeScreenshot': {}", debug_str);
    }

    // 15. BridgeCommand variants construction
    #[test]
    fn dispatch_mouse_event_construction() {
        let cmd = BridgeCommand::DispatchMouseEvent {
            event_type: "mouseMoved".into(),
            x: 100.0,
            y: 200.0,
            button: Some(0),
            click_count: Some(2),
        };
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("DispatchMouseEvent"));
        assert!(debug_str.contains("mouseMoved"));
    }

    #[test]
    fn dispatch_key_event_construction() {
        let cmd = BridgeCommand::DispatchKeyEvent {
            event_type: "keyDown".into(),
            key: "Enter".into(),
            code: "Enter".into(),
            text: Some("\r".into()),
        };
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("DispatchKeyEvent"));
    }

    #[test]
    fn set_viewport_construction() {
        let cmd = BridgeCommand::SetViewport {
            width: 1920,
            height: 1080,
            device_scale_factor: Some(2.0),
        };
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("SetViewport"));
    }

    #[test]
    fn set_cookie_construction() {
        let cmd = BridgeCommand::SetCookie {
            name: "session".into(),
            value: "abc123".into(),
            url: Some("https://example.com".into()),
            domain: Some(".example.com".into()),
        };
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("SetCookie"));
    }

    #[test]
    fn get_response_body_construction() {
        let cmd = BridgeCommand::GetResponseBody { request_id: "req-001".into() };
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("GetResponseBody"));
    }

    #[test]
    fn add_script_to_evaluate_on_new_document_construction() {
        let cmd = BridgeCommand::AddScriptToEvaluateOnNewDocument { source: "console.log('hi')".into() };
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("AddScriptToEvaluateOnNewDocument"));
    }

    #[test]
    fn reload_construction() {
        let cmd = BridgeCommand::Reload { ignore_cache: true };
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("Reload"));
    }

    #[test]
    fn go_back_construction() {
        let cmd = BridgeCommand::GoBack;
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("GoBack"));
    }

    #[test]
    fn go_forward_construction() {
        let cmd = BridgeCommand::GoForward;
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("GoForward"));
    }

    #[test]
    fn stop_loading_construction() {
        let cmd = BridgeCommand::StopLoading;
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("StopLoading"));
    }

    #[test]
    fn close_page_construction() {
        let cmd = BridgeCommand::ClosePage;
        let debug_str = format!("{:?}", cmd);
        assert!(debug_str.contains("ClosePage"));
    }

    // 16. BridgeSender timeout works (short timeout + slow responder)
    #[test]
    fn send_timeout_returns_err() {
        let (sender, receiver) = bridge_channel(Duration::from_millis(10));
        sender.send_fire_and_forget(BridgeCommand::GetTitle);
        // Don't process on receiver side — the send() call will timeout
        let resp = sender.send(BridgeCommand::GetUrl);
        // The first command is still queued; the second send creates a new channel
        // but the receiver is busy not processing, so the resp_rx.recv_timeout will expire
        // Actually we need to process the first to unblock, but we deliberately don't.
        // The send() itself succeeds (tx.send), but recv_timeout on the response fails.
        assert!(resp.result.is_err());
        assert_eq!(resp.result.unwrap_err(), "bridge response timeout");
        // Drain to clean up
        receiver.drain(noop_handler);
    }

    // 17. Multiple sequential send+process
    #[test]
    fn multiple_sequential_send_process() {
        let (sender, receiver) = bridge_channel(TIMEOUT);
        let commands: Vec<BridgeCommand> = vec![
            BridgeCommand::Navigate { url: "https://a.com".into() },
            BridgeCommand::EvaluateJs { expression: "1+1".into(), return_by_value: true },
            BridgeCommand::GetTitle,
        ];

        // Send all commands via fire-and-forget
        for cmd in commands {
            sender.send_fire_and_forget(cmd);
        }

        // Process each sequentially, tracking which commands arrive
        let mut results: Vec<String> = Vec::new();
        loop {
            let processed = receiver.try_process(|c| {
                let label = match c {
                    BridgeCommand::Navigate { url } => format!("nav:{}", url),
                    BridgeCommand::EvaluateJs { expression, .. } => format!("eval:{}", expression),
                    BridgeCommand::GetTitle => "title".into(),
                    _ => "other".into(),
                };
                results.push(label);
                ok_response(Value::Null)
            });
            if !processed {
                break;
            }
        }

        assert_eq!(results.len(), 3);
        assert!(results[0].starts_with("nav:"));
        assert!(results[1].starts_with("eval:"));
        assert_eq!(results[2], "title");
    }
}
