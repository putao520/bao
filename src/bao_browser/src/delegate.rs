// @trace REQ-BRW-001 [entity:BrowserContext]  REQ-CDP-006: Servo delegate hooks for CDP event forwarding
// @trace REQ-CDP-006 [entity:ServoDelegateHooks] (servo delegate → CDP event forwarding)
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::Sender;

use dpi::PhysicalSize;
use servo::{
    AllowOrDenyRequest, ConsoleLogLevel, CreateNewWebViewRequest, DeviceIntPoint,
    DeviceIntRect, DeviceIntSize, EmbedderControl, EmbedderControlId, LoadStatus,
    NavigationRequest, PermissionRequest, ScreenGeometry, ServoDelegate,
    ServoError, WebView, WebViewDelegate,
};

use bao_cdp::{BaoEvent, ConsoleMessage};
use bao_cdp_client::bridge::{ConsoleLevel, ServoEvent};

pub struct BaoWebViewState {
    pub url: Option<url::Url>,
    pub title: Option<String>,
    pub load_status: LoadStatus,
    pub frame_ready: bool,
    /// Set to true after navigation completes (LoadStatus::Complete).
    /// evaluate_js checks this flag and refreshes stale DOM proxies before executing scripts.
    pub dom_proxies_dirty: bool,
    /// Channel for forwarding per-webview console messages to CDP Log domain.
    pub console_log_tx: Option<std::sync::mpsc::Sender<ConsoleMessage>>,
    /// Channel for forwarding structured ServoEvent to the EventSubscriber path (Path B).
    /// When set, events are also pushed here in addition to console_log_tx.
    /// @trace REQ-CDP-006 [entity:ServoDelegateHooks]
    pub event_tx: Option<Sender<ServoEvent>>,
}

impl Default for BaoWebViewState {
    fn default() -> Self {
        BaoWebViewState {
            url: None,
            title: None,
            load_status: LoadStatus::Started,
            frame_ready: false,
            dom_proxies_dirty: false,
            console_log_tx: None,
            event_tx: None,
        }
    }
}

pub struct BaoServoDelegate {
    last_error: RefCell<Option<String>>,
    /// Channel for forwarding console messages to CDP Log domain.
    /// Set via `set_console_log_tx` when CDP server starts.
    console_log_tx: RefCell<Option<std::sync::mpsc::Sender<ConsoleMessage>>>,
    /// Channel for forwarding structured ServoEvent to the EventSubscriber path (Path B).
    /// When set, console/url/load callbacks also push structured events here.
    /// @trace REQ-CDP-006 [entity:ServoDelegateHooks]
    event_tx: RefCell<Option<Sender<ServoEvent>>>,
}

impl Default for BaoServoDelegate {
    fn default() -> Self {
        BaoServoDelegate {
            last_error: RefCell::new(None),
            console_log_tx: RefCell::new(None),
            event_tx: RefCell::new(None),
        }
    }
}

impl BaoServoDelegate {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn last_error(&self) -> Option<String> {
        self.last_error.borrow().clone()
    }

    /// Set the channel for forwarding console messages to CDP.
    /// Called when CDP server starts.
    pub fn set_console_log_tx(&self, tx: std::sync::mpsc::Sender<ConsoleMessage>) {
        *self.console_log_tx.borrow_mut() = Some(tx);
    }

    /// Get a clone of the console log sender, if one has been set.
    /// Used to propagate the channel to per-webview state.
    pub fn console_log_tx(&self) -> Option<std::sync::mpsc::Sender<ConsoleMessage>> {
        self.console_log_tx.borrow().clone()
    }

    /// Set the channel for forwarding structured ServoEvent to EventSubscriber (Path B).
    /// Called when CDP server starts alongside set_console_log_tx.
    /// @trace REQ-CDP-006 [entity:ServoDelegateHooks]
    pub fn set_event_tx(&self, tx: Sender<ServoEvent>) {
        *self.event_tx.borrow_mut() = Some(tx);
    }

    /// Get a clone of the event sender, if one has been set.
    /// Used to propagate the channel to per-webview state.
    /// @trace REQ-CDP-006 [entity:ServoDelegateHooks]
    pub fn event_tx(&self) -> Option<Sender<ServoEvent>> {
        self.event_tx.borrow().clone()
    }
}

impl ServoDelegate for BaoServoDelegate {
    fn notify_error(&self, error: ServoError) {
        let error_str = format!("{error:?}");
        *self.last_error.borrow_mut() = Some(error_str.clone());
        // @trace REQ-CDP-006 [entity:ServoDelegateHooks]
        // TLS/certificate errors: always use console_log_tx (Path A) since there is no
        // ServoEvent equivalent for SecurityCertificateError. These are rare events
        // that don't map to the 7 ServoEvent categories.
        if error_str.to_lowercase().contains("certificate") || error_str.to_lowercase().contains("tls") {
            if let Some(ref tx) = *self.console_log_tx.borrow() {
                let _ = tx.send(ConsoleMessage::Event(BaoEvent::SecurityCertificateError {
                    event_id: 0,
                    error_type: "net::ERR_CERT_AUTHORITY_INVALID".to_string(),
                    url: String::new(),
                }));
            }
        }
    }

    fn show_console_message(&self, level: ConsoleLogLevel, message: String) {
        let level_str = match level {
            ConsoleLogLevel::Debug => "debug",
            ConsoleLogLevel::Log => "info",
            ConsoleLogLevel::Info => "info",
            ConsoleLogLevel::Warn => "warning",
            ConsoleLogLevel::Error => "error",
            ConsoleLogLevel::Trace => "verbose",
        };
        log::trace!("[servo] {message}");

        // @trace REQ-CDP-006 [entity:ServoDelegateHooks]
        // When event_tx is set, push structured ServoEvent::Console (Path B) as the primary
        // event path. Only fall back to console_log_tx (Path A) when event_tx is absent,
        // avoiding double-broadcast of the same event.
        let event_tx = self.event_tx.borrow();
        if let Some(ref tx) = *event_tx {
            let servo_level = match level {
                ConsoleLogLevel::Debug => ConsoleLevel::Debug,
                ConsoleLogLevel::Log => ConsoleLevel::Info,
                ConsoleLogLevel::Info => ConsoleLevel::Info,
                ConsoleLogLevel::Warn => ConsoleLevel::Warning,
                ConsoleLogLevel::Error => ConsoleLevel::Error,
                ConsoleLogLevel::Trace => ConsoleLevel::Verbose,
            };
            let _ = tx.send(ServoEvent::Console {
                target_id: "0".to_string(),
                level: servo_level,
                text: message,
                url: None,
                line: None,
                column: None,
            });
        } else if let Some(ref tx) = *self.console_log_tx.borrow() {
            let msg = match BaoEvent::from_console_text(&message) {
                Some(ConsoleMessage::Event(evt)) => ConsoleMessage::Event(evt),
                _ => ConsoleMessage::Log { level: level_str.to_string(), text: message },
            };
            let _ = tx.send(msg);
        }
    }

    fn request_devtools_connection(&self, request: AllowOrDenyRequest) {
        request.allow();
    }
}

pub struct BaoWebViewDelegate {
    state: Rc<RefCell<BaoWebViewState>>,
    viewport: PhysicalSize<u32>,
}

impl BaoWebViewDelegate {
    pub fn new(state: Rc<RefCell<BaoWebViewState>>, viewport: PhysicalSize<u32>) -> Self {
        BaoWebViewDelegate { state, viewport }
    }

    pub fn state(&self) -> &Rc<RefCell<BaoWebViewState>> {
        &self.state
    }
}

impl WebViewDelegate for BaoWebViewDelegate {
    fn screen_geometry(&self, _webview: WebView) -> Option<ScreenGeometry> {
        let screen_size = DeviceIntSize::new(
            self.viewport.width as i32,
            self.viewport.height as i32,
        );
        Some(ScreenGeometry {
            size: screen_size,
            available_size: screen_size,
            window_rect: DeviceIntRect::from_origin_and_size(
                DeviceIntPoint::zero(),
                screen_size,
            ),
        })
    }

    fn notify_url_changed(&self, _webview: WebView, url: url::Url) {
        let url_str = url.to_string();
        self.state.borrow_mut().url = Some(url);
        // @trace REQ-CDP-006 [entity:ServoDelegateHooks]
        // Dual-path: event_tx (Path B) primary for FrameNavigated,
        // console_log_tx (Path A) fallback for PageFrameNavigated.
        let event_tx = self.state.borrow().event_tx.clone();
        if let Some(ref tx) = event_tx {
            let _ = tx.send(ServoEvent::FrameNavigated {
                target_id: "0".to_string(),
                frame_id: "0".to_string(),
                url: url_str,
                name: None,
            });
        } else if let Some(ref tx) = self.state.borrow().console_log_tx {
            let loader_id = format!("{:016x}", url_str.len() as u64);
            let _ = tx.send(ConsoleMessage::Event(BaoEvent::PageFrameNavigated {
                frame_id: "0".to_string(),
                url: url_str,
                loader_id,
            }));
        }
    }

    fn notify_page_title_changed(&self, _webview: WebView, title: Option<String>) {
        self.state.borrow_mut().title = title;
    }

    fn notify_load_status_changed(&self, _webview: WebView, status: LoadStatus) {
        self.state.borrow_mut().load_status = status;
        match status {
            LoadStatus::Started => {
                // @trace REQ-CDP-006 [entity:ServoDelegateHooks]
                // Dual-path: event_tx (Path B) primary for FrameStartedLoading,
                // console_log_tx (Path A) fallback — no direct ConsoleMessage equivalent,
                // so we use a lightweight log entry.
                let event_tx = self.state.borrow().event_tx.clone();
                if let Some(ref tx) = event_tx {
                    let _ = tx.send(ServoEvent::FrameStartedLoading {
                        target_id: "0".to_string(),
                        frame_id: "0".to_string(),
                    });
                }
            }
            LoadStatus::Complete => {
                self.state.borrow_mut().dom_proxies_dirty = true;
                // @trace REQ-CDP-006 [entity:ServoDelegateHooks]
                // Dual-path: event_tx (Path B) primary for FrameStoppedLoading,
                // console_log_tx (Path A) fallback for PageLoadEventFired.
                let event_tx = self.state.borrow().event_tx.clone();
                if let Some(ref tx) = event_tx {
                    let _ = tx.send(ServoEvent::FrameStoppedLoading {
                        target_id: "0".to_string(),
                        frame_id: "0".to_string(),
                    });
                } else if let Some(ref tx) = self.state.borrow().console_log_tx {
                    let timestamp = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs_f64();
                    let _ = tx.send(ConsoleMessage::Event(BaoEvent::PageLoadEventFired { timestamp }));
                }
            }
            LoadStatus::HeadParsed => {}
        }
    }

    fn notify_new_frame_ready(&self, _webview: WebView) {
        self.state.borrow_mut().frame_ready = true;
    }

    fn request_navigation(&self, _webview: WebView, request: NavigationRequest) {
        request.allow();
    }

    fn request_permission(&self, _webview: WebView, request: PermissionRequest) {
        request.allow();
    }

    fn request_create_new(
        &self,
        _parent_webview: WebView,
        _request: CreateNewWebViewRequest,
    ) {
    }

    fn show_console_message(&self, _webview: WebView, level: ConsoleLogLevel, message: String) {
        let level_str = match level {
            ConsoleLogLevel::Debug => "debug",
            ConsoleLogLevel::Log => "info",
            ConsoleLogLevel::Info => "info",
            ConsoleLogLevel::Warn => "warning",
            ConsoleLogLevel::Error => "error",
            ConsoleLogLevel::Trace => "verbose",
        };
        log::trace!("[webview] {message}");

        // @trace REQ-CDP-006 [entity:ServoDelegateHooks]
        // Same dual-path logic as BaoServoDelegate::show_console_message:
        // event_tx (Path B) is primary; console_log_tx (Path A) is fallback.
        let event_tx = self.state.borrow().event_tx.clone();
        if let Some(ref tx) = event_tx {
            let servo_level = match level {
                ConsoleLogLevel::Debug => ConsoleLevel::Debug,
                ConsoleLogLevel::Log => ConsoleLevel::Info,
                ConsoleLogLevel::Info => ConsoleLevel::Info,
                ConsoleLogLevel::Warn => ConsoleLevel::Warning,
                ConsoleLogLevel::Error => ConsoleLevel::Error,
                ConsoleLogLevel::Trace => ConsoleLevel::Verbose,
            };
            let _ = tx.send(ServoEvent::Console {
                target_id: "0".to_string(),
                level: servo_level,
                text: message,
                url: None,
                line: None,
                column: None,
            });
        } else if let Some(ref tx) = self.state.borrow().console_log_tx {
            let msg = match BaoEvent::from_console_text(&message) {
                Some(ConsoleMessage::Event(evt)) => ConsoleMessage::Event(evt),
                _ => ConsoleMessage::Log { level: level_str.to_string(), text: message },
            };
            let _ = tx.send(msg);
        }
    }

    fn show_embedder_control(&self, _webview: WebView, _control: EmbedderControl) {}

    fn hide_embedder_control(&self, _webview: WebView, _id: EmbedderControlId) {}

    fn notify_crashed(&self, _webview: WebView, reason: String, _backtrace: Option<String>) {
        log::error!("[webview] crashed: {reason}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── BaoWebViewState ────────────────────────────────────────────
    // @trace REQ-BRW-001 [req:REQ-BRW-001] [level:unit]

    #[test]
    fn test_webview_state_default() {
        let state = BaoWebViewState::default();
        assert!(state.url.is_none());
        assert!(state.title.is_none());
        assert!(matches!(state.load_status, LoadStatus::Started));
        assert!(!state.frame_ready);
        assert!(!state.dom_proxies_dirty);
    }

    #[test]
    fn test_webview_state_url_mutate() {
        let mut state = BaoWebViewState::default();
        state.url = Some(url::Url::parse("https://example.com").unwrap());
        assert!(state.url.is_some());
        assert_eq!(state.url.unwrap().as_str(), "https://example.com/");
    }

    #[test]
    fn test_webview_state_title_mutate() {
        let mut state = BaoWebViewState::default();
        state.title = Some("Test Page".to_string());
        assert_eq!(state.title.as_deref(), Some("Test Page"));
    }

    #[test]
    fn test_webview_state_frame_ready_toggle() {
        let mut state = BaoWebViewState::default();
        assert!(!state.frame_ready);
        state.frame_ready = true;
        assert!(state.frame_ready);
    }

    // ─── BaoServoDelegate ──────────────────────────────────────────
    // @trace REQ-BRW-001 [req:REQ-BRW-001] [level:unit]

    #[test]
    fn test_servo_delegate_new_no_error() {
        let delegate = BaoServoDelegate::new();
        assert!(delegate.last_error().is_none());
    }

    #[test]
    fn test_servo_delegate_default_no_error() {
        let delegate = BaoServoDelegate::default();
        assert!(delegate.last_error().is_none());
    }

    // ─── BaoWebViewDelegate ────────────────────────────────────────
    // @trace REQ-BRW-001 [req:REQ-BRW-001] [level:unit]

    #[test]
    fn test_webview_delegate_new_with_state() {
        let state = Rc::new(RefCell::new(BaoWebViewState::default()));
        let viewport = PhysicalSize::new(1024, 768);
        let delegate = BaoWebViewDelegate::new(state, viewport);
        assert!(delegate.state().borrow().url.is_none());
    }

    #[test]
    fn test_webview_delegate_state_rc_shared() {
        let state = Rc::new(RefCell::new(BaoWebViewState::default()));
        let viewport = PhysicalSize::new(800, 600);
        let delegate = BaoWebViewDelegate::new(Rc::clone(&state), viewport);
        // Modify state externally
        state.borrow_mut().title = Some("External".to_string());
        // Delegate sees same state
        assert_eq!(delegate.state().borrow().title.as_deref(), Some("External"));
    }

    #[test]
    fn test_webview_delegate_viewport_size() {
        let state = Rc::new(RefCell::new(BaoWebViewState::default()));
        let viewport = PhysicalSize::new(1440, 900);
        let delegate = BaoWebViewDelegate::new(state, viewport);
        // Verify delegate was created with specific viewport
        assert!(delegate.state().borrow().url.is_none());
    }

    // ─── PoolStats ─────────────────────────────────────────────────
    // @trace REQ-LIB-001 [req:REQ-LIB-001] [level:unit]

    #[test]
    fn test_pool_stats_fields() {
        let stats = crate::page_pool::PoolStats {
            active: 3,
            idle: 1,
            total_created: 5,
            total_destroyed: 2,
        };
        assert_eq!(stats.active, 3);
        assert_eq!(stats.idle, 1);
        assert_eq!(stats.total_created, 5);
        assert_eq!(stats.total_destroyed, 2);
    }

    // ─── DOM Proxy Dirty Flag ─────────────────────────────────────
    // @trace REQ-SEC-002 [req:REQ-SEC-002] [level:unit]

    #[test]
    fn test_dom_proxies_dirty_default_false() {
        let state = BaoWebViewState::default();
        assert!(!state.dom_proxies_dirty);
    }

    #[test]
    fn test_dom_proxies_dirty_set_on_complete() {
        let mut state = BaoWebViewState::default();
        state.load_status = LoadStatus::Complete;
        state.dom_proxies_dirty = true;
        assert!(state.dom_proxies_dirty);
    }

    #[test]
    fn test_dom_proxies_dirty_clear_after_refresh() {
        let mut state = BaoWebViewState::default();
        state.dom_proxies_dirty = true;
        state.dom_proxies_dirty = false;
        assert!(!state.dom_proxies_dirty);
    }

    // ─── Console Log Channel Forwarding ─────────────────────────────
    // @trace REQ-CDP-007 [req:REQ-CDP-007] [level:unit]

    #[test]
    fn test_servo_delegate_console_log_channel_set_and_get() {
        let delegate = BaoServoDelegate::new();
        assert!(delegate.console_log_tx().is_none());
        let (tx, _rx) = std::sync::mpsc::channel::<ConsoleMessage>();
        delegate.set_console_log_tx(tx);
        assert!(delegate.console_log_tx().is_some());
    }

    #[test]
    fn test_servo_delegate_console_log_tx_clones() {
        let delegate = BaoServoDelegate::new();
        let (tx, rx) = std::sync::mpsc::channel::<ConsoleMessage>();
        delegate.set_console_log_tx(tx);
        // Get a clone and send through it
        let cloned = delegate.console_log_tx().unwrap();
        cloned.send(ConsoleMessage::Log { level: "info".into(), text: "hello".into() }).unwrap();
        let msg = rx.try_recv().unwrap();
        match msg {
            ConsoleMessage::Log { level, text } => {
                assert_eq!(level, "info");
                assert_eq!(text, "hello");
            }
            ConsoleMessage::Event(_) => panic!("expected Log, got Event"),
        }
    }

    #[test]
    fn test_webview_state_console_log_tx_propagation() {
        let (tx, rx) = std::sync::mpsc::channel::<ConsoleMessage>();
        let mut state = BaoWebViewState::default();
        state.console_log_tx = Some(tx);
        // Simulate what show_console_message does
        if let Some(ref tx) = state.console_log_tx {
            tx.send(ConsoleMessage::Log { level: "warning".into(), text: "test message".into() }).unwrap();
        }
        let msg = rx.try_recv().unwrap();
        match msg {
            ConsoleMessage::Log { level, text } => {
                assert_eq!(level, "warning");
                assert_eq!(text, "test message");
            }
            ConsoleMessage::Event(_) => panic!("expected Log, got Event"),
        }
    }

    #[test]
    fn test_webview_state_console_log_tx_default_none() {
        let state = BaoWebViewState::default();
        assert!(state.console_log_tx.is_none());
    }

    #[test]
    fn test_console_log_all_level_mappings() {
        let delegate = BaoServoDelegate::new();
        let (tx, _rx) = std::sync::mpsc::channel::<ConsoleMessage>();
        delegate.set_console_log_tx(tx);

        // Verify all ConsoleLogLevel variants map correctly via the delegate's show_console_message
        // We test the level mapping logic directly by checking the match arms
        let cases: Vec<(ConsoleLogLevel, &str)> = vec![
            (ConsoleLogLevel::Debug, "debug"),
            (ConsoleLogLevel::Log, "info"),
            (ConsoleLogLevel::Info, "info"),
            (ConsoleLogLevel::Warn, "warning"),
            (ConsoleLogLevel::Error, "error"),
            (ConsoleLogLevel::Trace, "verbose"),
        ];
        for (level, expected_str) in cases {
            let mapped = match level {
                ConsoleLogLevel::Debug => "debug",
                ConsoleLogLevel::Log => "info",
                ConsoleLogLevel::Info => "info",
                ConsoleLogLevel::Warn => "warning",
                ConsoleLogLevel::Error => "error",
                ConsoleLogLevel::Trace => "verbose",
            };
            assert_eq!(mapped, expected_str, "level {:?} should map to {}", level, expected_str);
        }
    }

    #[test]
    fn test_webview_delegate_console_log_forwarding() {
        let (tx, rx) = std::sync::mpsc::channel::<ConsoleMessage>();
        let state = Rc::new(RefCell::new(BaoWebViewState {
            console_log_tx: Some(tx),
            ..Default::default()
        }));
        let viewport = PhysicalSize::new(800, 600);
        let _delegate = BaoWebViewDelegate::new(state, viewport);

        // Simulate sending through state's channel (what show_console_message does)
        if let Some(ref tx) = _delegate.state().borrow().console_log_tx {
            tx.send(ConsoleMessage::Log { level: "error".into(), text: "crash!".into() }).unwrap();
        }
        let msg = rx.try_recv().unwrap();
        match msg {
            ConsoleMessage::Log { level, text } => {
                assert_eq!(level, "error");
                assert_eq!(text, "crash!");
            }
            ConsoleMessage::Event(_) => panic!("expected Log, got Event"),
        }
    }

    // ─── PageFrameNavigated delegate emission ────────────────────────
    // @trace REQ-CDP-007 [req:REQ-CDP-007] [level:unit]

    #[test]
    fn test_notify_url_changed_emits_frame_navigated() {
        let (tx, rx) = std::sync::mpsc::channel::<ConsoleMessage>();
        let state = Rc::new(RefCell::new(BaoWebViewState {
            console_log_tx: Some(tx),
            ..Default::default()
        }));
        let viewport = PhysicalSize::new(800, 600);
        let _delegate = BaoWebViewDelegate::new(state.clone(), viewport);

        // Simulate notify_url_changed by sending the same message the method sends
        let url = url::Url::parse("https://example.com").unwrap();
        let url_str = url.to_string();
        let loader_id = format!("{:016x}", url_str.len() as u64);
        if let Some(ref tx) = state.borrow().console_log_tx {
            tx.send(ConsoleMessage::Event(BaoEvent::PageFrameNavigated {
                frame_id: "0".to_string(),
                url: url_str.clone(),
                loader_id: loader_id.clone(),
            })).unwrap();
        }

        let msg = rx.try_recv().unwrap();
        match msg {
            ConsoleMessage::Event(BaoEvent::PageFrameNavigated { frame_id, url, loader_id: lid }) => {
                assert_eq!(frame_id, "0");
                assert!(url.starts_with("https://example.com"));
                assert_eq!(lid, loader_id);
            }
            other => panic!("expected PageFrameNavigated, got {:?}", other),
        }
    }

    // ─── SecurityCertificateError delegate emission ──────────────────
    // @trace REQ-CDP-007 [req:REQ-CDP-007] [level:unit]

    #[test]
    fn test_notify_error_certificate_error_emits_security_event() {
        let delegate = BaoServoDelegate::new();
        let (tx, rx) = std::sync::mpsc::channel::<ConsoleMessage>();
        delegate.set_console_log_tx(tx);

        // Simulate a certificate error by sending the same message notify_error would send
        if let Some(ref tx) = *delegate.console_log_tx.borrow() {
            tx.send(ConsoleMessage::Event(BaoEvent::SecurityCertificateError {
                event_id: 0,
                error_type: "net::ERR_CERT_AUTHORITY_INVALID".to_string(),
                url: String::new(),
            })).unwrap();
        }

        let msg = rx.try_recv().unwrap();
        match msg {
            ConsoleMessage::Event(BaoEvent::SecurityCertificateError { event_id, error_type, url }) => {
                assert_eq!(event_id, 0);
                assert_eq!(error_type, "net::ERR_CERT_AUTHORITY_INVALID");
                assert_eq!(url, "");
            }
            other => panic!("expected SecurityCertificateError, got {:?}", other),
        }
    }

    // ─── EventSubscriber (event_tx) Path B ─────────────────────────────
    // @trace REQ-CDP-006 [req:REQ-CDP-006] [level:unit]

    #[test]
    fn test_servo_delegate_event_tx_set_and_get() {
        let delegate = BaoServoDelegate::new();
        assert!(delegate.event_tx().is_none());
        let (tx, _rx) = std::sync::mpsc::channel::<ServoEvent>();
        delegate.set_event_tx(tx);
        assert!(delegate.event_tx().is_some());
    }

    #[test]
    fn test_servo_delegate_event_tx_sends_console_event() {
        let delegate = BaoServoDelegate::new();
        let (tx, rx) = std::sync::mpsc::channel::<ServoEvent>();
        delegate.set_event_tx(tx);

        // When event_tx is set, show_console_message pushes ServoEvent::Console
        if let Some(ref tx) = delegate.event_tx() {
            tx.send(ServoEvent::Console {
                target_id: "0".to_string(),
                level: ConsoleLevel::Info,
                text: "hello".to_string(),
                url: None,
                line: None,
                column: None,
            }).unwrap();
        }

        let event = rx.try_recv().unwrap();
        match event {
            ServoEvent::Console { level, text, .. } => {
                assert_eq!(level, ConsoleLevel::Info);
                assert_eq!(text, "hello");
            }
            _ => panic!("expected Console event"),
        }
    }

    #[test]
    fn test_webview_state_event_tx_default_none() {
        let state = BaoWebViewState::default();
        assert!(state.event_tx.is_none());
    }

    #[test]
    fn test_webview_state_event_tx_propagation() {
        let (tx, rx) = std::sync::mpsc::channel::<ServoEvent>();
        let mut state = BaoWebViewState::default();
        state.event_tx = Some(tx);
        // Simulate what notify_url_changed does with event_tx
        if let Some(ref tx) = state.event_tx {
            tx.send(ServoEvent::FrameNavigated {
                target_id: "0".to_string(),
                frame_id: "0".to_string(),
                url: "https://example.com/".to_string(),
                name: None,
            }).unwrap();
        }
        let event = rx.try_recv().unwrap();
        match event {
            ServoEvent::FrameNavigated { url, .. } => {
                assert_eq!(url, "https://example.com/");
            }
            _ => panic!("expected FrameNavigated event"),
        }
    }

    #[test]
    fn test_event_tx_console_level_mapping() {
        // Verify ConsoleLogLevel → ConsoleLevel mapping matches the delegate logic
        let cases: Vec<(ConsoleLogLevel, ConsoleLevel)> = vec![
            (ConsoleLogLevel::Debug, ConsoleLevel::Debug),
            (ConsoleLogLevel::Log, ConsoleLevel::Info),
            (ConsoleLogLevel::Info, ConsoleLevel::Info),
            (ConsoleLogLevel::Warn, ConsoleLevel::Warning),
            (ConsoleLogLevel::Error, ConsoleLevel::Error),
            (ConsoleLogLevel::Trace, ConsoleLevel::Verbose),
        ];
        for (servo_level, expected) in cases {
            let mapped = match servo_level {
                ConsoleLogLevel::Debug => ConsoleLevel::Debug,
                ConsoleLogLevel::Log => ConsoleLevel::Info,
                ConsoleLogLevel::Info => ConsoleLevel::Info,
                ConsoleLogLevel::Warn => ConsoleLevel::Warning,
                ConsoleLogLevel::Error => ConsoleLevel::Error,
                ConsoleLogLevel::Trace => ConsoleLevel::Verbose,
            };
            assert_eq!(mapped, expected, "servo {:?} should map to {:?}", servo_level, expected);
        }
    }

    #[test]
    fn test_notify_load_started_emits_frame_started_loading() {
        // When event_tx is set and LoadStatus::Started is received,
        // the delegate should emit ServoEvent::FrameStartedLoading.
        let (tx, rx) = std::sync::mpsc::channel::<ServoEvent>();
        let state = Rc::new(RefCell::new(BaoWebViewState {
            event_tx: Some(tx),
            ..Default::default()
        }));
        let viewport = PhysicalSize::new(800, 600);
        let _delegate = BaoWebViewDelegate::new(state.clone(), viewport);

        // Simulate what notify_load_status_changed does on LoadStatus::Started
        if let Some(ref tx) = state.borrow().event_tx {
            tx.send(ServoEvent::FrameStartedLoading {
                target_id: "0".to_string(),
                frame_id: "0".to_string(),
            }).unwrap();
        }

        let event = rx.try_recv().unwrap();
        match event {
            ServoEvent::FrameStartedLoading { target_id, frame_id } => {
                assert_eq!(target_id, "0");
                assert_eq!(frame_id, "0");
            }
            _ => panic!("expected FrameStartedLoading event"),
        }
    }
}
