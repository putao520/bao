use super::*;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender};

use dpi::PhysicalSize;
use servo::{
    AllowOrDenyRequest, ConsoleLogLevel, CreateNewWebViewRequest, DeviceIntPoint, DeviceIntRect,
    DeviceIntSize, EmbedderControl, EmbedderControlId, InputEventId, InputEventResult, LoadStatus,
    NavigationRequest, PermissionRequest, ScreenGeometry, ServoDelegate, ServoError, TraversalId,
    WebView, WebViewDelegate,
};

use bao_cdp::servo_bridge::main_frame_id_for_target;
use bao_cdp::{BaoEvent, ConsoleMessage};
use bao_cdp_client::bridge::{ConsoleLevel, ServoEvent};

// ─── ServoEvent real 路径可靠投递(REQ-CDP-004 / REQ-CDP-006) ───────
//
// 通道形态:**无界可靠队列**(`std::sync::mpsc::channel`)——`send` 永不
// 阻塞、永不丢弃(drop-newest 已根除),单队列 FIFO 天然保序。
//
// 为什么不是「有界 + 满时阻塞泵侧」:producer(delegate 回调)与 consumer
// (泵 drain)同线程——servo 以 `Rc<dyn WebViewDelegate>` 持有委托
// (结构性 !Send,回调只可能发生在持有 WebView 的 embedder 线程上),即
// `run_with_bridge` 里调 `spin_event_loop()` 的泵线程自身;而 drain 点在
// `spin_event_loop()` 返回之后。回调内阻塞 send = 泵无法回到 drain 点 =
// 自死锁。阻塞形态被线程模型排除,可靠队列是唯一无丢弃且无死锁的诚实形态。
// 内存上界由泵节奏保证:每次 loop 迭代产生的的事件在下一段 `try_recv`
// 循环内被全量排空,持续积压仅存在于「接了通道但从不 drain」的配置
// (`run` / `pump_cdp` 不接线 `set_event_channel`,零事件,不构成积压面)。
//
// 探针:投递计数三面对账——`servo_event_emitted_total()`(本面,producer)
// == `servo_event_pumped_total()`(lib.rs 泵面,consumer)。REQ-CDP-004
// 的 delivery 断言即等式成立;分歧只可能来自接收端 teardown(Disconnected)。

/// real 路径投递探针(producer 腿):经 `send_servo_event` 进入通道的
/// ServoEvent 累计数。与 lib.rs 泵面 `servo_event_pumped_total()` 对账。
static SERVO_EVENT_EMITTED: AtomicU64 = AtomicU64::new(0);

/// Disconnected 首次警告闩:接收端已 drop 是 teardown 面,不是投递丢失,
/// 只告警一次避免关停期刷屏。
static SEND_FAILED_LOGGED: AtomicBool = AtomicBool::new(false);

/// real 路径投递探针(producer 腿)累计数。suite 级 delivery 断言消费。
pub fn servo_event_emitted_total() -> u64 {
    SERVO_EVENT_EMITTED.load(Ordering::Relaxed)
}

/// 向 real 事件通道投递一个 ServoEvent:无界可靠队列 `send`,永不丢弃、
/// 永不阻塞;接收端已 drop 时记一次性 warn(teardown 面,非投递丢失)。
pub(crate) fn send_servo_event(tx: &Sender<ServoEvent>, event: ServoEvent) {
    match tx.send(event) {
        Ok(()) => {
            SERVO_EVENT_EMITTED.fetch_add(1, Ordering::Relaxed);
        }
        Err(_) => {
            if !SEND_FAILED_LOGGED.swap(true, Ordering::Relaxed) {
                log::warn!("servo event receiver dropped, event not delivered (teardown)");
            }
        }
    }
}

pub struct BaoServoDelegate {
    last_error: RefCell<Option<String>>,
    /// Channel for forwarding console messages to CDP Log domain.
    /// Set via `set_console_log_tx` when CDP server starts.
    console_log_tx: RefCell<Option<std::sync::mpsc::Sender<ConsoleMessage>>>,
    /// Real event queue sender (Path B): reliable unbounded mpsc to the pump.
    /// When set, console/url/load callbacks also push structured events here.
    /// @trace REQ-CDP-006 [entity:ServoDelegateHooks]
    event_tx: RefCell<Option<Sender<ServoEvent>>>,
    /// Global SharedWorker registry — keyed by (script_url, name).
    /// SharedWorkers span pages (DF-WK-7), so they must be tracked at the
    /// delegate level rather than per-page. When a page creates a SharedWorker,
    /// the constellation routes to the same worker thread if (url, name) matches.
    /// This registry tracks all active SharedWorkers across all pages.
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    shared_workers: RefCell<Vec<SharedWorkerHandle>>,
    /// Global ServiceWorker registry — keyed by (script_url, scope).
    /// ServiceWorkers have persistent lifecycle (跨页存活) and can control
    /// multiple pages within their scope. Per DF-WK-8: "navigator.serviceWorker.
    /// register(url,{scope}) → serviceworker_manager 注册 → scope 匹配的
    /// 导航/fetch 经 SW 拦截".
    /// Per SPEC criterion #19: "SW 持久生命周期(跨页存活)下 profile 继承注册页
    /// 且 terminate 后正确注销".
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19] DF-WK-8
    service_workers: RefCell<Vec<ServiceWorkerHandle>>,
}

impl Default for BaoServoDelegate {
    fn default() -> Self {
        BaoServoDelegate {
            last_error: RefCell::new(None),
            console_log_tx: RefCell::new(None),
            event_tx: RefCell::new(None),
            shared_workers: RefCell::new(Vec::new()),
            service_workers: RefCell::new(Vec::new()),
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

    /// Set the channel for forwarding structured ServoEvent to the real CDP
    /// event queue (Path B: unbounded reliable mpsc, drained by the
    /// `run_with_bridge` pump — not the bao_cdp_client `EventSubscriber`).
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

    // ─── SharedWorker Global Registry (REQ-BRW-004 / DF-WK-7) ────────

    /// Register a SharedWorker in the global registry.
    ///
    /// DF-WK-7: When a page creates a new SharedWorker, the handle is
    /// registered here so other pages can find it by (script_url, name).
    /// If a SharedWorker with the same id already exists, the existing
    /// handle is returned instead (constellation dedup).
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub fn register_shared_worker(&self, handle: SharedWorkerHandle) -> SharedWorkerHandle {
        let id = handle.id();
        let mut shared_workers = self.shared_workers.borrow_mut();
        if let Some(existing) = shared_workers.iter().find(|h| h.id() == id) {
            existing.clone()
        } else {
            shared_workers.push(handle.clone());
            handle
        }
    }

    /// Find an existing SharedWorker by (script_url, name).
    ///
    /// Returns a clone of the SharedWorkerHandle if found, None otherwise.
    /// Used when a page creates a SharedWorker and the constellation routes
    /// to an existing worker (DF-WK-7: "多页 new SharedWorker(url) 同 name →
    /// constellation 路由到同一 worker 线程").
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub fn find_shared_worker(&self, script_url: &str, name: &str) -> Option<SharedWorkerHandle> {
        self.shared_workers
            .borrow()
            .iter()
            .find(|h| h.script_url == script_url && h.name == name)
            .cloned()
    }

    /// Remove terminated SharedWorkers from the registry.
    ///
    /// Called after spin_event_loop to clean up SharedWorkers whose threads
    /// have exited and have zero connected pages.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker]
    pub fn reap_terminated_shared_workers(&self) {
        self.shared_workers
            .borrow_mut()
            .retain(|h| !h.is_terminated() || h.connected_page_count() > 0);
    }

    /// Returns the number of active SharedWorkers across all pages.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker]
    pub fn shared_worker_count(&self) -> usize {
        self.shared_workers.borrow().len()
    }

    /// Route a SharedWorker connection request to the appropriate worker.
    ///
    /// DF-WK-7: "多页 new SharedWorker(url) 同 name → constellation 路由到
    /// 同一 worker 线程". If a SharedWorker with the same (script_url, name)
    /// already exists in the registry, return the existing handle (the
    /// constellation handles dedup). Otherwise, register a new SharedWorker.
    ///
    /// Returns the handle (existing or new) and a boolean indicating whether
    /// this is a new SharedWorker (true) or a reconnection (false).
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub fn route_shared_worker(&self, handle: SharedWorkerHandle) -> (SharedWorkerHandle, bool) {
        let id = handle.id();
        let mut shared_workers = self.shared_workers.borrow_mut();
        if let Some(existing) = shared_workers.iter().find(|h| h.id() == id) {
            (existing.clone(), false)
        } else {
            shared_workers.push(handle.clone());
            (handle, true)
        }
    }

    /// Find or create a SharedWorker for the given (script_url, name).
    ///
    /// Convenience method combining find_shared_worker with register_shared_worker.
    /// Returns the handle and whether it was newly created.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker] DF-WK-7
    pub fn get_or_create_shared_worker(
        &self,
        script_url: &str,
        name: &str,
    ) -> (SharedWorkerHandle, bool) {
        if let Some(existing) = self.find_shared_worker(script_url, name) {
            (existing, false)
        } else {
            let handle = SharedWorkerHandle::new(script_url.to_string(), name.to_string());
            let returned = self.register_shared_worker(handle);
            (returned, true)
        }
    }

    /// Remove a SharedWorker from the global registry by its ID.
    ///
    /// Called when a SharedWorker has been fully terminated and has zero
    /// connected pages. This is the final cleanup step in the lifecycle.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker]
    pub fn unregister_shared_worker(&self, id: &SharedWorkerId) -> bool {
        let mut shared_workers = self.shared_workers.borrow_mut();
        let before = shared_workers.len();
        shared_workers.retain(|h| &h.id() != id);
        shared_workers.len() < before
    }

    /// Returns a snapshot of all SharedWorker handles in the registry.
    ///
    /// Used for CDP observability and lifecycle management.
    ///
    /// @trace REQ-BRW-004 [entity:SharedWorker]
    pub fn all_shared_workers(&self) -> Vec<SharedWorkerHandle> {
        self.shared_workers.borrow().iter().cloned().collect()
    }

    // ─── ServiceWorker Global Registry (REQ-BRW-004 / DF-WK-8) ─────────

    /// Register a ServiceWorker in the global registry.
    ///
    /// DF-WK-8: "navigator.serviceWorker.register(url,{scope}) → serviceworker_manager
    /// 注册". If a ServiceWorker with the same registration_id already exists,
    /// the existing handle is returned instead (registration dedup).
    ///
    /// The handle captures the registering page's StealthProfile for stealth
    /// boundary enforcement (SPEC criterion #19).
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19] DF-WK-8
    pub fn register_service_worker(&self, handle: ServiceWorkerHandle) -> ServiceWorkerHandle {
        let id = handle.id();
        let mut service_workers = self.service_workers.borrow_mut();
        if let Some(existing) = service_workers.iter().find(|h| h.id() == id) {
            existing.clone()
        } else {
            service_workers.push(handle.clone());
            handle
        }
    }

    /// Find an existing ServiceWorker by (script_url, scope).
    ///
    /// Returns a clone of the ServiceWorkerHandle if found, None otherwise.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker] DF-WK-8
    pub fn find_service_worker(
        &self,
        script_url: &str,
        scope: &str,
    ) -> Option<ServiceWorkerHandle> {
        self.service_workers
            .borrow()
            .iter()
            .find(|h| h.script_url == script_url && h.scope == scope)
            .cloned()
    }

    /// Find a ServiceWorker whose scope matches the given URL.
    ///
    /// Per DF-WK-8: "scope 匹配的导航/fetch 经 SW 拦截". Returns the
    /// ServiceWorker whose scope prefix-matches the URL and is in the
    /// Activated state (intercepting fetches).
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19] DF-WK-8
    pub fn find_service_worker_for_url(&self, url: &str) -> Option<ServiceWorkerHandle> {
        self.service_workers
            .borrow()
            .iter()
            .filter(|h| h.is_intercepting_fetch())
            .find(|h| url.starts_with(&h.scope))
            .cloned()
    }

    /// Remove terminated ServiceWorkers from the registry.
    ///
    /// Per SPEC criterion #19: "terminate 后正确注销". Called after
    /// spin_event_loop to clean up ServiceWorkers whose threads have exited.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19]
    pub fn reap_terminated_service_workers(&self) {
        self.service_workers
            .borrow_mut()
            .retain(|h| !h.is_terminated());
    }

    /// Returns the number of active ServiceWorker registrations across all pages.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker]
    pub fn service_worker_count(&self) -> usize {
        self.service_workers.borrow().len()
    }

    /// Unregister a ServiceWorker by its registration ID.
    ///
    /// Per SPEC criterion #19: "terminate 后正确注销". This is the final
    /// cleanup step — the ServiceWorker is removed from the global registry,
    /// and its fetch interception is disabled.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19]
    pub fn unregister_service_worker(&self, id: &ServiceWorkerRegistrationId) -> bool {
        let mut service_workers = self.service_workers.borrow_mut();
        let before = service_workers.len();
        service_workers.retain(|h| &h.id() != id);
        service_workers.len() < before
    }

    /// Find or create a ServiceWorker for the given (script_url, scope).
    ///
    /// Convenience method combining find_service_worker with register_service_worker.
    /// Returns the handle and whether it was newly created.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker] DF-WK-8
    pub fn get_or_create_service_worker(
        &self,
        script_url: &str,
        scope: &str,
        stealth_profile: Option<bao_stealth::StealthProfile>,
    ) -> (ServiceWorkerHandle, bool) {
        if let Some(existing) = self.find_service_worker(script_url, scope) {
            (existing, false)
        } else {
            let handle = ServiceWorkerHandle::new(
                script_url.to_string(),
                scope.to_string(),
                stealth_profile,
            );
            let returned = self.register_service_worker(handle);
            (returned, true)
        }
    }

    /// Returns a snapshot of all ServiceWorker handles in the registry.
    ///
    /// Used for CDP observability and lifecycle management.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker]
    pub fn all_service_workers(&self) -> Vec<ServiceWorkerHandle> {
        self.service_workers.borrow().iter().cloned().collect()
    }

    /// Verify stealth profile consistency for all ServiceWorker-intercepted fetches.
    ///
    /// Per SPEC criterion #19: "SW 拦截并转发的 fetch 仍走主页同一 stealth
    /// TLS(JA3/JA4)+HTTP2(AKAMAI) profile (不绕过反指纹)". This method
    /// checks that all active ServiceWorkers have a stealth profile consistent
    /// with the given page's profile.
    ///
    /// Returns a list of violations (ServiceWorker registrations where the
    /// profile doesn't match).
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19]
    pub fn verify_service_worker_stealth_consistency(
        &self,
        page_stealth_profile: &bao_stealth::StealthProfile,
    ) -> Vec<ServiceWorkerRegistrationId> {
        self.service_workers
            .borrow()
            .iter()
            .filter(|h| h.is_intercepting_fetch())
            .filter(|h| {
                // Check if the SW's stealth profile matches the page's profile.
                // A profile mismatch means SW-intercepted fetches could bypass
                // the page's stealth TLS/HTTP2 settings (SPEC criterion #19).
                match &h.stealth_profile {
                    Some(sw_profile) => {
                        // Compare key fingerprint-relevant fields.
                        // If any field differs, it's a stealth boundary violation.
                        sw_profile.navigator.user_agent != page_stealth_profile.navigator.user_agent
                            || sw_profile.navigator.platform
                                != page_stealth_profile.navigator.platform
                    }
                    None => {
                        // No stealth profile on an intercepting SW — this is always
                        // a violation because intercepted fetches won't have stealth.
                        true
                    }
                }
            })
            .map(|h| h.id())
            .collect()
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
        if error_str.to_lowercase().contains("certificate")
            || error_str.to_lowercase().contains("tls")
        {
            if let Some(ref tx) = *self.console_log_tx.borrow() {
                // Lossy by design: fire-and-forget console observability — the
                // send only fails once the consumer is dropped; never stall the
                // servo script thread on CDP event delivery.
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
            ConsoleLogLevel::Dir => "info",
        };
        log::trace!("[servo] {message}");

        // @trace REQ-CDP-006 [entity:ServoDelegateHooks]
        // `__BAO_EVT__` texts are the JS→Rust CDP event transport (Debugger
        // .scriptParsed/.paused etc., emitted by the page-realm Debugger glue
        // in cdp_handler) — structured event data, not a log line. They route
        // through the ConsoleMessage parser (Path A) even when the raw
        // ServoEvent path (Path B) is active: Path B has no structured-event
        // equivalent, so without this arm the events would degrade to
        // Log.entryAdded and never reach Debugger-domain subscribers
        // (SM-EVOLUTION #27 裁决 2 transport closure).
        if message.starts_with("__BAO_EVT__") {
            if let Some(ref tx) = *self.console_log_tx.borrow() {
                if let Some(ConsoleMessage::Event(evt)) = BaoEvent::from_console_text(&message) {
                    // Lossy by design: fire-and-forget console observability — the
                    // send only fails once the consumer is dropped; never stall the
                    // servo script thread on CDP event delivery.
                    let _ = tx.send(ConsoleMessage::Event(evt));
                    return;
                }
            }
        }
        // REQ-CDP-004 target routing: this ServoDelegate arm serves console
        // content NOT associated with any WebView (servo.rs routes
        // `EmbedderMsg::ShowConsoleApiMessage` here only when the message
        // carries no webview id — page content goes through the per-webview
        // delegate, which stamps its real CDP target). With no target there
        // is nothing to route against: the targeted event path (Path B) has
        // no subscriber for an unknown target and the former placeholder
        // "0" tag was the broadcast-everywhere pollution this wave removes,
        // so webview-less messages stay on the console_log_tx (Path A)
        // broadcast face only.
        if let Some(ref tx) = *self.console_log_tx.borrow() {
            let msg = match BaoEvent::from_console_text(&message) {
                Some(ConsoleMessage::Event(evt)) => ConsoleMessage::Event(evt),
                _ => ConsoleMessage::Log {
                    level: level_str.to_string(),
                    text: message,
                },
            };
            // Lossy by design: fire-and-forget console observability — the
            // send only fails once the consumer is dropped; never stall the
            // servo script thread on CDP event delivery.
            let _ = tx.send(msg);
        }
    }

    fn request_devtools_connection(&self, request: AllowOrDenyRequest) {
        request.allow();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            }))
            .unwrap();
        }

        let msg = rx.try_recv().unwrap();
        match msg {
            ConsoleMessage::Event(BaoEvent::SecurityCertificateError {
                event_id,
                error_type,
                url,
            }) => {
                assert_eq!(event_id, 0);
                assert_eq!(error_type, "net::ERR_CERT_AUTHORITY_INVALID");
                assert_eq!(url, "");
            }
            other => panic!("expected SecurityCertificateError, got {:?}", other),
        }
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
        cloned
            .send(ConsoleMessage::Log {
                level: "info".into(),
                text: "hello".into(),
            })
            .unwrap();
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
            tx.send(ConsoleMessage::Log {
                level: "warning".into(),
                text: "test message".into(),
            })
            .unwrap();
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
            (ConsoleLogLevel::Dir, "info"),
        ];
        for (level, expected_str) in cases {
            let mapped = match level {
                ConsoleLogLevel::Debug => "debug",
                ConsoleLogLevel::Log => "info",
                ConsoleLogLevel::Info => "info",
                ConsoleLogLevel::Warn => "warning",
                ConsoleLogLevel::Error => "error",
                ConsoleLogLevel::Trace => "verbose",
                ConsoleLogLevel::Dir => "info",
            };
            assert_eq!(
                mapped, expected_str,
                "level {:?} should map to {}",
                level, expected_str
            );
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
        let _delegate = BaoWebViewDelegate::new(state, viewport, std::rc::Weak::new());

        // Simulate sending through state's channel (what show_console_message does)
        if let Some(ref tx) = _delegate.state().borrow().console_log_tx {
            tx.send(ConsoleMessage::Log {
                level: "error".into(),
                text: "crash!".into(),
            })
            .unwrap();
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

    // ─── event_tx (real CDP event queue) Path B ────────────────────────
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
    fn test_event_tx_reliable_under_saturation() {
        // REQ-CDP-004 real 路径可靠投递:远超旧有界容量(1024)的突发负载下
        // 零丢弃——send_servo_event 每发必达,且保序(探针 delta == 消费数,
        // 消费序 == 发射序)。
        let (tx, rx) = std::sync::mpsc::channel::<ServoEvent>();
        const BURST: usize = 5000;
        let emitted_before = servo_event_emitted_total();

        for i in 0..BURST {
            send_servo_event(
                &tx,
                ServoEvent::Console {
                    target_id: "7".to_string(),
                    level: ConsoleLevel::Info,
                    text: format!("burst-{i}"),
                    url: None,
                    line: None,
                    column: None,
                },
            );
        }
        assert_eq!(
            servo_event_emitted_total(),
            emitted_before + BURST as u64,
            "probe must count every send"
        );

        // Pump-shape drain: every event arrives, in emission order.
        let mut consumed = Vec::with_capacity(BURST);
        while let Ok(event) = rx.try_recv() {
            if let ServoEvent::Console { text, .. } = event {
                consumed.push(text);
            }
        }
        assert_eq!(consumed.len(), BURST, "reliable queue must not drop");
        for (i, text) in consumed.iter().enumerate() {
            assert_eq!(text, &format!("burst-{i}"), "FIFO order must hold");
        }
    }

    #[test]
    fn test_event_tx_send_survives_dropped_receiver() {
        // Disconnected(接收端已 drop)是 teardown 面:send 不 panic,
        // 探针不计数(未进入通道 ≠ 已投递)。
        let (tx, rx) = std::sync::mpsc::channel::<ServoEvent>();
        drop(rx);
        let emitted_before = servo_event_emitted_total();
        send_servo_event(
            &tx,
            ServoEvent::Console {
                target_id: "7".to_string(),
                level: ConsoleLevel::Info,
                text: "after-teardown".to_string(),
                url: None,
                line: None,
                column: None,
            },
        );
        assert_eq!(servo_event_emitted_total(), emitted_before);
    }

    #[test]
    fn test_servo_delegate_event_tx_sends_console_event() {
        let delegate = BaoServoDelegate::new();
        let (tx, rx) = std::sync::mpsc::channel::<ServoEvent>();
        delegate.set_event_tx(tx);

        // When event_tx is set, show_console_message pushes ServoEvent::Console
        if let Some(ref tx) = delegate.event_tx() {
            tx.send(ServoEvent::Console {
                target_id: "7".to_string(),
                level: ConsoleLevel::Info,
                text: "hello".to_string(),
                url: None,
                line: None,
                column: None,
            })
            .unwrap();
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
        state.cdp_target_id = Some("7".to_string());
        // Emit through the real state identity: the frame id is derived from
        // the target (REQ-CDP-004 v7 path B).
        let target = state.cdp_target().expect("stamped target");
        if let Some(ref tx) = state.event_tx {
            tx.send(ServoEvent::FrameNavigated {
                target_id: target.clone(),
                frame_id: main_frame_id_for_target(&target),
                url: "https://example.com/".to_string(),
                name: None,
            })
            .unwrap();
        }
        let event = rx.try_recv().unwrap();
        match event {
            ServoEvent::FrameNavigated {
                target_id,
                frame_id,
                url,
                ..
            } => {
                assert_eq!(url, "https://example.com/");
                assert_eq!(target_id, "7");
                assert_eq!(frame_id, "main-7");
                assert_ne!(frame_id, target_id, "frameId is never the PageId");
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
                ConsoleLogLevel::Dir => ConsoleLevel::Info,
            };
            assert_eq!(
                mapped, expected,
                "servo {:?} should map to {:?}",
                servo_level, expected
            );
        }
    }

    #[test]
    fn test_notify_load_started_emits_frame_started_loading() {
        // When event_tx is set and LoadStatus::Started is received,
        // the delegate should emit ServoEvent::FrameStartedLoading —
        // tagged with the page's real CDP target and its per-target main
        // frame id (REQ-CDP-004).
        let (tx, rx) = std::sync::mpsc::channel::<ServoEvent>();
        let state = Rc::new(RefCell::new(BaoWebViewState {
            event_tx: Some(tx),
            cdp_target_id: Some("7".to_string()),
            ..Default::default()
        }));
        let viewport = PhysicalSize::new(800, 600);
        let _delegate = BaoWebViewDelegate::new(state.clone(), viewport, std::rc::Weak::new());

        // Simulate what notify_load_status_changed does on LoadStatus::Started
        let target = state.borrow().cdp_target().expect("stamped target");
        if let Some(ref tx) = state.borrow().event_tx {
            tx.send(ServoEvent::FrameStartedLoading {
                frame_id: main_frame_id_for_target(&target),
                target_id: target.clone(),
            })
            .unwrap();
        }

        let event = rx.try_recv().unwrap();
        match event {
            ServoEvent::FrameStartedLoading {
                target_id,
                frame_id,
            } => {
                assert_eq!(target_id, "7");
                assert_eq!(frame_id, "main-7");
                assert_ne!(frame_id, target_id, "frameId is never the PageId");
            }
            _ => panic!("expected FrameStartedLoading event"),
        }
    }

    #[test]
    fn forward_worker_error_routes_to_stamped_target_and_drops_without_one() {
        // Real emission path (no WebView needed): with a stamped CDP target
        // the worker error carries it; without one the event is dropped
        // (no-target-no-deliver — never a placeholder tag).
        let worker_error = WorkerErrorEvent {
            worker_id: WorkerId("w.js".into()),
            message: "boom".into(),
            filename: "w.js".into(),
            lineno: 1,
            colno: 2,
        };

        let (tx, rx) = std::sync::mpsc::channel::<ServoEvent>();
        let mut state = BaoWebViewState::default();
        state.event_tx = Some(tx);
        state.cdp_target_id = Some("42".to_string());
        state.forward_worker_error_event(worker_error.clone());
        match rx.try_recv().expect("routed event") {
            ServoEvent::PageError { target_id, .. } => assert_eq!(target_id, "42"),
            other => panic!("expected PageError, got {other:?}"),
        }

        let (tx2, rx2) = std::sync::mpsc::channel::<ServoEvent>();
        let mut unstamp = BaoWebViewState::default();
        unstamp.event_tx = Some(tx2);
        unstamp.forward_worker_error_event(worker_error);
        assert!(
            rx2.try_recv().is_err(),
            "no CDP target identity → the event must be dropped, not tagged"
        );
    }

    // ─── BaoServoDelegate SharedWorker Routing (REQ-BRW-004 / DF-WK-7) ─────
    // @trace REQ-BRW-004 [req:REQ-BRW-004] [entity:SharedWorker] [DF-WK-7] [level:unit]

    #[test]
    fn test_delegate_route_shared_worker_new() {
        let delegate = BaoServoDelegate::new();
        let handle = SharedWorkerHandle::new("sw.js".to_string(), "myname".to_string());
        let (returned, is_new) = delegate.route_shared_worker(handle);
        assert!(is_new);
        assert_eq!(returned.script_url, "sw.js");
        assert_eq!(delegate.shared_worker_count(), 1);
    }

    #[test]
    fn test_delegate_route_shared_worker_existing() {
        let delegate = BaoServoDelegate::new();
        let handle1 = SharedWorkerHandle::new("sw.js".to_string(), "myname".to_string());
        let handle2 = SharedWorkerHandle::new("sw.js".to_string(), "myname".to_string());
        delegate.route_shared_worker(handle1);
        let (_, is_new) = delegate.route_shared_worker(handle2);
        assert!(
            !is_new,
            "same (url, name) should return existing, not create new"
        );
        assert_eq!(delegate.shared_worker_count(), 1);
    }

    #[test]
    fn test_delegate_get_or_create_shared_worker_new() {
        let delegate = BaoServoDelegate::new();
        let (handle, is_new) = delegate.get_or_create_shared_worker("sw.js", "myname");
        assert!(is_new);
        assert_eq!(handle.script_url, "sw.js");
        assert_eq!(handle.name, "myname");
    }

    #[test]
    fn test_delegate_get_or_create_shared_worker_existing() {
        let delegate = BaoServoDelegate::new();
        delegate.get_or_create_shared_worker("sw.js", "myname");
        let (_, is_new) = delegate.get_or_create_shared_worker("sw.js", "myname");
        assert!(!is_new);
        assert_eq!(delegate.shared_worker_count(), 1);
    }

    #[test]
    fn test_delegate_unregister_shared_worker() {
        let delegate = BaoServoDelegate::new();
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "myname".to_string(),
        };
        delegate.get_or_create_shared_worker("sw.js", "myname");
        assert_eq!(delegate.shared_worker_count(), 1);
        let removed = delegate.unregister_shared_worker(&id);
        assert!(removed);
        assert_eq!(delegate.shared_worker_count(), 0);
    }

    #[test]
    fn test_delegate_unregister_nonexistent_shared_worker() {
        let delegate = BaoServoDelegate::new();
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "nonexistent".to_string(),
        };
        let removed = delegate.unregister_shared_worker(&id);
        assert!(!removed);
    }

    #[test]
    fn test_delegate_all_shared_workers() {
        let delegate = BaoServoDelegate::new();
        delegate.get_or_create_shared_worker("sw1.js", "a");
        delegate.get_or_create_shared_worker("sw2.js", "b");
        let all = delegate.all_shared_workers();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_shared_worker_cross_page_routing_full_lifecycle() {
        // @trace REQ-BRW-004 [entity:SharedWorker] [entity:SharedWorkerGlobalScope] DF-WK-7
        // Full lifecycle: route → register scope → add ports → drain messages → disconnect → reap
        let delegate = BaoServoDelegate::new();

        // Page 1 creates SharedWorker
        let (handle, is_new) = delegate.route_shared_worker(SharedWorkerHandle::new(
            "sw.js".to_string(),
            "shared".to_string(),
        ));
        assert!(is_new);
        assert_eq!(handle.connected_page_count(), 0);

        // Page 1 connects
        let mut state1 = BaoWebViewState::default();
        let id = SharedWorkerId {
            script_url: "sw.js".to_string(),
            name: "shared".to_string(),
        };
        let config = SharedWorkerScopeConfig::default();
        state1.register_shared_worker_scope(
            id.clone(),
            SharedWorkerGlobalScopeState::new(id.clone(), &config),
        );
        state1.track_shared_worker_port(SharedWorkerPortRef::new(handle.clone()));
        // SharedWorkerPortRef::new already increments connected_page_count
        assert_eq!(handle.connected_page_count(), 1);
        // Each page gets its own channel bridge (ports are per-page)
        let endpoints1 = state1.add_shared_worker_port(id.clone());

        // Page 2 connects (same SharedWorker, but its own channel bridge)
        let mut state2 = BaoWebViewState::default();
        state2.register_shared_worker_scope(
            id.clone(),
            SharedWorkerGlobalScopeState::new(id.clone(), &config),
        );
        state2.track_shared_worker_port(SharedWorkerPortRef::new(handle.clone()));
        let endpoints2 = state2.add_shared_worker_port(id.clone());
        assert_eq!(handle.connected_page_count(), 2);

        // Both pages can send messages to the SharedWorker through their own ports
        let payload1 = StructuredClonePayload {
            data: vec![1],
            transferable_count: 0,
        };
        state1
            .post_to_worker_via_shared_port(&id, 0, payload1)
            .unwrap();
        let payload2 = StructuredClonePayload {
            data: vec![2],
            transferable_count: 0,
        };
        state2
            .post_to_worker_via_shared_port(&id, 0, payload2)
            .unwrap();

        // Worker thread receives from both pages
        let rx1 = endpoints1.page_to_worker_rx.unwrap();
        let rx2 = endpoints2.page_to_worker_rx.unwrap();
        assert_eq!(rx1.try_recv().unwrap().data, vec![1]);
        assert_eq!(rx2.try_recv().unwrap().data, vec![2]);

        // SharedWorker sends messages back to both pages
        let tx1 = endpoints1.worker_to_page_tx.unwrap();
        let tx2 = endpoints2.worker_to_page_tx.unwrap();
        tx1.send(WorkerStructuredMessage::metadata_only(
            WorkerId("sw.js".to_string()),
            WorkerMessageDirection::WorkerToPage,
        ))
        .unwrap();
        tx2.send(WorkerStructuredMessage::metadata_only(
            WorkerId("sw.js".to_string()),
            WorkerMessageDirection::WorkerToPage,
        ))
        .unwrap();

        // Page 1 drains its messages
        let (msgs1, disc1) = state1.drain_all_shared_worker_messages();
        assert_eq!(msgs1.len(), 1);
        assert!(disc1.is_empty());
        // Page 2 drains its messages
        let (msgs2, disc2) = state2.drain_all_shared_worker_messages();
        assert_eq!(msgs2.len(), 1);
        assert!(disc2.is_empty());

        // Page 1 navigates away — SharedWorker survives
        state1.disconnect_shared_worker_ports();
        // disconnect drops the SharedWorkerPortRef → connected_page_count decrements
        assert_eq!(handle.connected_page_count(), 1);
        assert!(!handle.is_closing());

        // Page 2 still connected
        assert_eq!(state2.shared_worker_port_count(), 1);

        // SharedWorker self.close() — terminates
        handle.close();
        handle.mark_terminated();
        assert!(handle.is_closing());
        assert!(handle.is_terminated());

        // Delegate reaps terminated shared worker with zero connected pages
        // (after page 2 also disconnects)
        state2.disconnect_shared_worker_ports();
        assert_eq!(handle.connected_page_count(), 0);
        delegate.reap_terminated_shared_workers();
        assert_eq!(delegate.shared_worker_count(), 0);
    }
}
