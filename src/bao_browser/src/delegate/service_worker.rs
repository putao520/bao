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

// ─── ServiceWorker Registration & Fetch Interception (REQ-BRW-004 criterion #19) ────
// @trace REQ-BRW-004 [entity:ServiceWorker] [entity:ServiceWorkerGlobalScope]
//   [criterion:19] DF-WK-8 / DF-WK-10
//
// SPEC criterion #19: "ServiceWorker fetch 拦截 × stealth/CDP 边界一致:
//   SW 拦截并转发的 fetch 仍走主页同一 stealth TLS(JA3/JA4)+HTTP2(AKAMAI) profile
//   (不绕过反指纹); CDP Network 域可观测 SW 发起的请求/响应; SW 持久生命周期
//   (跨页存活)下 profile 继承注册页且 terminate 后正确注销"
//
// DF-WK-8: "navigator.serviceWorker.register(url,{scope}) → serviceworker_manager 注册
//   → scope 匹配的导航/fetch 经 SW 拦截 → fetch 事件"
//   Thread: SW 独立线程 + constellation serviceworker.rs 管理
//
// DF-WK-10: "ServiceWorkerGlobalScope 首次解析 stealth getter → 按 D7 机制从
//   bao-stealth REALM_PROFILES 继承注册页 profile"
//
// Architecture (mirrors DedicatedWorker/SharedWorker pattern):
//   - Servo handles the actual ServiceWorker DOM binding internally (if/when
//     implemented). Bao's responsibility is:
//     1. Track per-delegate ServiceWorker registrations for lifecycle management
//     2. Track per-page ServiceWorker references (navigator.serviceWorker.controller)
//     3. Ensure stealth profile propagation: SW-intercepted fetch uses the same
//        TLS(JA3/JA4)/HTTP2(AKAMAI) profile as the registering page
//     4. Provide CDP Network domain observability for SW-initiated requests
//     5. Ensure SW persistent lifecycle: profile inherits from registering page
//        and is properly unregistered on terminate
//
// Thread safety: ServiceWorkerHandle only holds Arc<AtomicBool> flags — no
// JSObject, no raw pointer. The actual ServiceWorker DOM object lives in
// servo's ScriptThread; we never touch it from bao_browser.

/// Unique identifier for a ServiceWorker registration, keyed by (script_url, scope).
///
/// Per SPEC DF-WK-8: "navigator.serviceWorker.register(url,{scope})" creates a
/// registration keyed by (script_url, scope). Multiple pages within the same
/// scope share the same ServiceWorker registration.
///
/// @trace REQ-BRW-004 [entity:ServiceWorker] DF-WK-8
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ServiceWorkerRegistrationId {
    /// ServiceWorker script URL.
    pub script_url: String,
    /// Registration scope (URL prefix). Defaults to the script URL's directory.
    pub scope: String,
}

/// Lifecycle state of a ServiceWorker registration.
///
/// Per the Service Worker spec, a registration transitions through states:
/// installing → installed(waiting) → activating → activated(active) → redundant
///
/// @trace REQ-BRW-004 [entity:ServiceWorker] DF-WK-8
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceWorkerRegistrationState {
    /// No active ServiceWorker for this registration.
    /// @trace REQ-BRW-004 [entity:ServiceWorker] DF-WK-8
    Idle,
    /// ServiceWorker is being installed (install event fired).
    /// @trace REQ-BRW-004 [entity:ServiceWorker] DF-WK-8
    Installing,
    /// ServiceWorker has been installed but is waiting to activate.
    /// @trace REQ-BRW-004 [entity:ServiceWorker] DF-WK-8
    Installed,
    /// ServiceWorker is activating (activate event fired).
    /// @trace REQ-BRW-004 [entity:ServiceWorker] DF-WK-8
    Activating,
    /// ServiceWorker is active and controlling pages within its scope.
    /// @trace REQ-BRW-004 [entity:ServiceWorker] DF-WK-8
    Activated,
    /// ServiceWorker is redundant (replaced by a new version).
    /// @trace REQ-BRW-004 [entity:ServiceWorker] DF-WK-8
    Redundant,
}

/// The fetch interception mode for a ServiceWorker.
///
/// When a ServiceWorker is activated, it can intercept fetch events within
/// its scope. This enum tracks whether the SW is actively intercepting
/// fetch requests.
///
/// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceWorkerFetchInterceptMode {
    /// ServiceWorker is not intercepting fetch requests.
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19]
    None,
    /// ServiceWorker is intercepting fetch requests within its scope.
    /// Intercepted requests still use the registering page's stealth profile
    /// (TLS JA3/JA4 + HTTP2 AKAMAI) per SPEC criterion #19.
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19]
    Intercepting,
}

/// A Send+Sync handle to a ServiceWorker's lifecycle state.
///
/// Does NOT hold JSObject references — only atomic flags and IDs.
/// This is safe to store across threads (unlike ServiceWorker DOM objects).
///
/// @trace REQ-BRW-004 [entity:ServiceWorker]
#[derive(Debug, Clone)]
pub struct ServiceWorkerHandle {
    /// ServiceWorker script URL.
    pub script_url: String,
    /// Registration scope.
    pub scope: String,
    /// Whether the ServiceWorker's closing flag is set.
    /// Set by terminate() or when the registration is unregistered.
    pub closing: Arc<AtomicBool>,
    /// Whether the ServiceWorker thread has fully exited.
    pub terminated: Arc<AtomicBool>,
    /// Current registration state.
    pub state: Arc<std::sync::Mutex<ServiceWorkerRegistrationState>>,
    /// Current fetch interception mode.
    pub fetch_intercept_mode: Arc<std::sync::Mutex<ServiceWorkerFetchInterceptMode>>,
    /// StealthProfile inherited from the registering page.
    /// Per DF-WK-10: "ServiceWorkerGlobalScope 首次解析 stealth getter → 按 D7 机制
    /// 从 bao-stealth REALM_PROFILES 继承注册页 profile"
    /// Per SPEC criterion #19: "SW 持久生命周期(跨页存活)下 profile 继承注册页且
    /// terminate 后正确注销"
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19] DF-WK-10
    pub stealth_profile: Option<bao_stealth::StealthProfile>,
}

impl ServiceWorkerHandle {
    /// Create a new ServiceWorkerHandle in the installing state.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker] DF-WK-8
    pub fn new(
        script_url: String,
        scope: String,
        stealth_profile: Option<bao_stealth::StealthProfile>,
    ) -> Self {
        ServiceWorkerHandle {
            script_url,
            scope,
            closing: Arc::new(AtomicBool::new(false)),
            terminated: Arc::new(AtomicBool::new(false)),
            state: Arc::new(std::sync::Mutex::new(
                ServiceWorkerRegistrationState::Installing,
            )),
            fetch_intercept_mode: Arc::new(std::sync::Mutex::new(
                ServiceWorkerFetchInterceptMode::None,
            )),
            stealth_profile,
        }
    }

    /// Returns the ServiceWorkerRegistrationId for this handle.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker]
    pub fn id(&self) -> ServiceWorkerRegistrationId {
        ServiceWorkerRegistrationId {
            script_url: self.script_url.clone(),
            scope: self.scope.clone(),
        }
    }

    /// Returns true if the closing flag has been set.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker]
    pub fn is_closing(&self) -> bool {
        self.closing.load(Ordering::Acquire)
    }

    /// Returns true if the ServiceWorker thread has fully exited.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker]
    pub fn is_terminated(&self) -> bool {
        self.terminated.load(Ordering::Acquire)
    }

    /// Returns the current registration state.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker]
    pub fn registration_state(&self) -> ServiceWorkerRegistrationState {
        self.state
            .lock()
            .expect("ServiceWorkerHandle state lock poisoned")
            .clone()
    }

    /// Returns the current fetch interception mode.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19]
    pub fn fetch_intercept_mode(&self) -> ServiceWorkerFetchInterceptMode {
        self.fetch_intercept_mode
            .lock()
            .expect("ServiceWorkerHandle fetch_intercept_mode lock poisoned")
            .clone()
    }

    /// Returns true if the ServiceWorker is actively intercepting fetch requests.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19]
    pub fn is_intercepting_fetch(&self) -> bool {
        matches!(
            self.fetch_intercept_mode(),
            ServiceWorkerFetchInterceptMode::Intercepting
        )
    }

    /// Transition the registration state to a new state.
    ///
    /// Valid transitions: Installing → Installed → Activating → Activated → Redundant
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker] DF-WK-8
    pub fn transition_state(&self, new_state: ServiceWorkerRegistrationState) {
        let mut state = self
            .state
            .lock()
            .expect("ServiceWorkerHandle state lock poisoned");
        *state = new_state;
    }

    /// Enable fetch interception mode.
    ///
    /// Called when the ServiceWorker becomes activated and starts intercepting
    /// fetch events within its scope. Per SPEC criterion #19, intercepted
    /// fetch requests MUST use the registering page's stealth TLS/HTTP2 profile.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19]
    pub fn enable_fetch_interception(&self) {
        let mut mode = self
            .fetch_intercept_mode
            .lock()
            .expect("ServiceWorkerHandle fetch_intercept_mode lock poisoned");
        *mode = ServiceWorkerFetchInterceptMode::Intercepting;
    }

    /// Disable fetch interception mode.
    ///
    /// Called when the ServiceWorker becomes redundant or is terminated.
    /// Per SPEC criterion #19: "terminate 后正确注销".
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19]
    pub fn disable_fetch_interception(&self) {
        let mut mode = self
            .fetch_intercept_mode
            .lock()
            .expect("ServiceWorkerHandle fetch_intercept_mode lock poisoned");
        *mode = ServiceWorkerFetchInterceptMode::None;
    }

    /// Signal the ServiceWorker to terminate.
    /// Idempotent — calling multiple times is safe.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker]
    pub fn terminate(&self) {
        self.closing.store(true, Ordering::Release);
        // Per SPEC criterion #19: "terminate 后正确注销"
        // Disable fetch interception so subsequent requests don't try to
        // route through a terminated ServiceWorker.
        self.disable_fetch_interception();
    }

    /// Mark the ServiceWorker as fully terminated (called after thread join).
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorker]
    pub fn mark_terminated(&self) {
        self.terminated.store(true, Ordering::Release);
    }
}

/// The state of a ServiceWorker registration as tracked by bao_browser.
///
/// This struct represents the bao-side view of a ServiceWorker registration.
/// The actual DOM ServiceWorkerRegistration lives in servo; this struct tracks
/// the state that bao needs for lifecycle management, stealth consistency,
/// and CDP observability.
///
/// @trace REQ-BRW-004 [entity:ServiceWorker] DF-WK-8
#[derive(Debug, Clone)]
pub struct ServiceWorkerRegistrationTracking {
    /// The registration ID (script_url + scope).
    /// @trace REQ-BRW-004 [entity:ServiceWorker] DF-WK-8
    pub registration_id: ServiceWorkerRegistrationId,
    /// Current lifecycle state of the registration.
    /// @trace REQ-BRW-004 [entity:ServiceWorker] DF-WK-8
    pub state: ServiceWorkerRegistrationState,
    /// Whether fetch interception is active for this registration.
    /// @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19]
    pub fetch_intercept_active: bool,
    /// The URL of the page that registered this ServiceWorker.
    /// Used for stealth profile inheritance (DF-WK-10).
    /// @trace REQ-BRW-004 [entity:ServiceWorker] DF-WK-10
    pub registering_page_url: String,
    /// Whether the onfetch event handler is registered in the ServiceWorker.
    /// Tracked for CDP observability.
    /// @trace REQ-BRW-004 [entity:ServiceWorker]
    pub has_fetch_handler: bool,
}

/// A ServiceWorkerGlobalScope state tracked by bao_browser.
///
/// This struct represents the bao-side view of a ServiceWorker's global scope.
/// The actual DOM ServiceWorkerGlobalScope lives in servo's ScriptThread;
/// this struct tracks the state that bao needs for lifecycle management,
/// CDP observability, and stealth consistency verification.
///
/// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope] DF-WK-8 / DF-WK-10
#[derive(Debug, Clone)]
pub struct ServiceWorkerGlobalScopeState {
    /// The base WorkerGlobalScope state.
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope] [entity:WorkerGlobalScope]
    pub scope: WorkerGlobalScopeState,
    /// The ServiceWorkerRegistrationId this scope belongs to.
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope]
    pub registration_id: ServiceWorkerRegistrationId,
    /// Whether onfetch event handler is registered.
    /// When true, the ServiceWorker intercepts fetch events within its scope.
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope] [criterion:19]
    pub has_fetch_handler: bool,
    /// Whether onactivate event handler is registered.
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope]
    pub has_activate_handler: bool,
    /// Whether oninstall event handler is registered.
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope]
    pub has_install_handler: bool,
    /// Whether onmessage event handler is registered (for SW-to-page messages).
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope]
    pub has_message_handler: bool,
    /// The registration scope URL (used for fetch interception matching).
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope] DF-WK-8
    pub scope_url: String,
}

/// Configuration for initializing a ServiceWorker's ServiceWorkerGlobalScope
/// with stealth-consistent properties from the registering page.
///
/// DF-WK-10: "ServiceWorkerGlobalScope 首次解析 stealth getter → 按 D7 机制从
/// bao-stealth REALM_PROFILES 继承注册页 profile"
/// SPEC criterion #19: "SW 持久生命周期(跨页存活)下 profile 继承注册页且
/// terminate 后正确注销"
///
/// Unlike DedicatedWorker (one parent page) and SharedWorker (first connecting page),
/// ServiceWorker inherits from the REGISTERING page — the page that called
/// navigator.serviceWorker.register(url, {scope}). The profile is fixed for the
/// ServiceWorker's lifetime (per DEC-WK-007).
///
/// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope] [criterion:19] DF-WK-10
#[derive(Debug, Clone)]
pub struct ServiceWorkerScopeConfig {
    /// The StealthProfile to apply in the ServiceWorker's global scope.
    /// Set from the registering page's profile and fixed for lifetime.
    /// Per SPEC criterion #19: SW-intercepted fetch uses the same stealth profile.
    /// @trace REQ-BRW-004 [criterion:19] CRIT-STL-WK ServiceWorker stealth boundary
    pub stealth_profile: Option<bao_stealth::StealthProfile>,
    /// Navigator userAgent — must match registering page's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub user_agent: String,
    /// Navigator platform — must match registering page's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub platform: String,
    /// Navigator hardwareConcurrency — must match registering page's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub hardware_concurrency: usize,
    /// Navigator language — must match registering page's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub language: String,
    /// Navigator languages — must match registering page's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub languages: Vec<String>,
    /// The registering page's URL — used for CDP observability and
    /// profile inheritance tracking.
    /// @trace REQ-BRW-004 [entity:ServiceWorker] DF-WK-10
    pub registering_page_url: String,
}

impl Default for ServiceWorkerScopeConfig {
    fn default() -> Self {
        ServiceWorkerScopeConfig {
            stealth_profile: None,
            user_agent: String::new(),
            platform: String::new(),
            hardware_concurrency: std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
            language: "en-US".to_string(),
            languages: vec!["en-US".to_string(), "en".to_string()],
            registering_page_url: String::new(),
        }
    }
}

impl From<&bao_stealth::StealthProfile> for ServiceWorkerScopeConfig {
    /// Convert a StealthProfile into a ServiceWorkerScopeConfig for Service Worker inheritance.
    ///
    /// DF-WK-10: ServiceWorkerGlobalScope inherits the registering page's profile.
    /// Per SPEC criterion #19: SW-intercepted fetch must use the same stealth profile.
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope] [criterion:19] DF-WK-10
    fn from(profile: &bao_stealth::StealthProfile) -> Self {
        ServiceWorkerScopeConfig {
            stealth_profile: Some(profile.clone()),
            user_agent: profile.navigator.user_agent.clone(),
            platform: profile.navigator.platform.clone(),
            hardware_concurrency: profile.navigator.hardware_concurrency as usize,
            language: profile.navigator.language.clone(),
            languages: profile.navigator.languages.clone(),
            registering_page_url: String::new(),
        }
    }
}

impl WorkerGlobalScopeState {
    /// Create a WorkerGlobalScopeState from a script URL and service scope config.
    ///
    /// @trace REQ-BRW-004 [entity:WorkerGlobalScope]
    pub fn new_service(worker_url: String, config: &ServiceWorkerScopeConfig) -> Self {
        Self::from_scope_config(worker_url, config)
    }
}

impl ServiceWorkerGlobalScopeState {
    /// Create a ServiceWorkerGlobalScopeState for the given ServiceWorker registration.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope] DF-WK-8 / DF-WK-10
    pub fn new(
        registration_id: ServiceWorkerRegistrationId,
        config: &ServiceWorkerScopeConfig,
    ) -> Self {
        let worker_url = registration_id.script_url.clone();
        let scope_url = registration_id.scope.clone();
        ServiceWorkerGlobalScopeState {
            scope: WorkerGlobalScopeState::new_service(worker_url, config),
            registration_id,
            has_fetch_handler: false,
            has_activate_handler: false,
            has_install_handler: false,
            has_message_handler: false,
            scope_url,
        }
    }

    /// Get the WorkerLocation for this scope.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope] [entity:WorkerLocation]
    pub fn location(&self) -> Option<&WorkerLocation> {
        self.scope.location.as_ref()
    }

    /// Get the WorkerNavigator for this scope.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope] [entity:WorkerNavigator]
    pub fn navigator(&self) -> &WorkerNavigator {
        &self.scope.navigator
    }

    /// Mark onfetch handler as registered.
    /// When set, the ServiceWorker will intercept fetch events within its scope.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope] [criterion:19]
    pub fn set_fetch_handler(&mut self) {
        self.has_fetch_handler = true;
    }

    /// Mark onactivate handler as registered.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope]
    pub fn set_activate_handler(&mut self) {
        self.has_activate_handler = true;
    }

    /// Mark oninstall handler as registered.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope]
    pub fn set_install_handler(&mut self) {
        self.has_install_handler = true;
    }

    /// Mark onmessage handler as registered.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope]
    pub fn set_message_handler(&mut self) {
        self.has_message_handler = true;
    }

    /// Returns true if the given URL falls within this ServiceWorker's scope.
    ///
    /// Per DF-WK-8: "scope 匹配的导航/fetch 经 SW 拦截".
    /// A URL is within scope if it starts with the scope URL prefix.
    ///
    /// @trace REQ-BRW-004 [entity:ServiceWorkerGlobalScope] DF-WK-8
    pub fn is_url_in_scope(&self, url: &str) -> bool {
        url.starts_with(&self.scope_url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── ServiceWorker Registration & Fetch Interception (REQ-BRW-004 criterion #19) ────
    // @trace REQ-BRW-004 [entity:ServiceWorker] [entity:ServiceWorkerGlobalScope]
    //   [criterion:19] DF-WK-8 / DF-WK-10

    #[test]
    fn test_service_worker_registration_id_equality() {
        let id1 = ServiceWorkerRegistrationId {
            script_url: "sw.js".to_string(),
            scope: "/".to_string(),
        };
        let id2 = ServiceWorkerRegistrationId {
            script_url: "sw.js".to_string(),
            scope: "/".to_string(),
        };
        let id3 = ServiceWorkerRegistrationId {
            script_url: "sw.js".to_string(),
            scope: "/app/".to_string(),
        };
        assert_eq!(id1, id2);
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_service_worker_handle_lifecycle() {
        let handle = ServiceWorkerHandle::new("sw.js".to_string(), "/".to_string(), None);
        assert!(!handle.is_closing());
        assert!(!handle.is_terminated());
        assert_eq!(
            handle.registration_state(),
            ServiceWorkerRegistrationState::Installing
        );
        assert!(!handle.is_intercepting_fetch());
    }

    #[test]
    fn test_service_worker_handle_state_transitions() {
        let handle = ServiceWorkerHandle::new("sw.js".to_string(), "/".to_string(), None);
        // Installing → Installed
        handle.transition_state(ServiceWorkerRegistrationState::Installed);
        assert_eq!(
            handle.registration_state(),
            ServiceWorkerRegistrationState::Installed
        );

        // Installed → Activating
        handle.transition_state(ServiceWorkerRegistrationState::Activating);
        assert_eq!(
            handle.registration_state(),
            ServiceWorkerRegistrationState::Activating
        );

        // Activating → Activated + enable fetch interception
        handle.transition_state(ServiceWorkerRegistrationState::Activated);
        handle.enable_fetch_interception();
        assert_eq!(
            handle.registration_state(),
            ServiceWorkerRegistrationState::Activated
        );
        assert!(handle.is_intercepting_fetch());
        assert_eq!(
            handle.fetch_intercept_mode(),
            ServiceWorkerFetchInterceptMode::Intercepting
        );
    }

    #[test]
    fn test_service_worker_handle_terminate_disables_interception() {
        let handle = ServiceWorkerHandle::new("sw.js".to_string(), "/".to_string(), None);
        handle.enable_fetch_interception();
        assert!(handle.is_intercepting_fetch());

        // Per SPEC criterion #19: "terminate 后正确注销"
        handle.terminate();
        assert!(handle.is_closing());
        assert!(!handle.is_intercepting_fetch());
        assert_eq!(
            handle.fetch_intercept_mode(),
            ServiceWorkerFetchInterceptMode::None
        );
    }

    #[test]
    fn test_service_worker_scope_config_from_stealth_profile() {
        let profile = bao_stealth::StealthProfile::chrome_default();
        let config = ServiceWorkerScopeConfig::from(&profile);
        assert!(config.stealth_profile.is_some());
        assert_eq!(config.user_agent, profile.navigator.user_agent);
        assert_eq!(config.platform, profile.navigator.platform);
        assert_eq!(
            config.hardware_concurrency,
            profile.navigator.hardware_concurrency as usize
        );
        assert_eq!(config.language, profile.navigator.language);
    }

    #[test]
    fn test_service_worker_global_scope_state() {
        let reg_id = ServiceWorkerRegistrationId {
            script_url: "sw.js".to_string(),
            scope: "/app/".to_string(),
        };
        let config = ServiceWorkerScopeConfig::default();
        let scope = ServiceWorkerGlobalScopeState::new(reg_id.clone(), &config);

        assert!(!scope.has_fetch_handler);
        assert!(!scope.has_activate_handler);
        assert!(!scope.has_install_handler);
        assert!(!scope.has_message_handler);
        assert_eq!(scope.scope_url, "/app/");
        assert!(scope.is_url_in_scope("/app/page1"));
        assert!(scope.is_url_in_scope("/app/sub/page2"));
        assert!(!scope.is_url_in_scope("/other/page"));
    }

    #[test]
    fn test_service_worker_global_scope_fetch_handler() {
        let reg_id = ServiceWorkerRegistrationId {
            script_url: "sw.js".to_string(),
            scope: "/".to_string(),
        };
        let config = ServiceWorkerScopeConfig::default();
        let mut scope = ServiceWorkerGlobalScopeState::new(reg_id, &config);

        scope.set_fetch_handler();
        assert!(scope.has_fetch_handler);
        assert!(scope.is_url_in_scope("/anything"));
    }

    #[test]
    fn test_webview_state_service_worker_control() {
        let mut state = BaoWebViewState::default();
        assert!(!state.is_controlled_by_service_worker());

        let handle = ServiceWorkerHandle::new("sw.js".to_string(), "/".to_string(), None);
        state.set_controlling_service_worker(handle);
        assert!(state.is_controlled_by_service_worker());

        state.clear_controlling_service_worker();
        assert!(!state.is_controlled_by_service_worker());
    }

    #[test]
    fn test_webview_state_service_worker_scope_matching() {
        let mut state = BaoWebViewState::default();
        assert!(!state.is_url_in_service_worker_scope("/app/page1"));

        let reg_id = ServiceWorkerRegistrationId {
            script_url: "sw.js".to_string(),
            scope: "/app/".to_string(),
        };
        let config = ServiceWorkerScopeConfig::default();
        let scope = ServiceWorkerGlobalScopeState::new(reg_id, &config);
        let handle = ServiceWorkerHandle::new("sw.js".to_string(), "/app/".to_string(), None);
        state.set_controlling_service_worker(handle);
        state.register_service_worker_scope(scope);

        assert!(state.is_url_in_service_worker_scope("/app/page1"));
        assert!(state.is_url_in_service_worker_scope("/app/sub/page2"));
        assert!(!state.is_url_in_service_worker_scope("/other/page"));
    }

    #[test]
    fn test_delegate_service_worker_registration() {
        let delegate = BaoServoDelegate::new();
        assert_eq!(delegate.service_worker_count(), 0);

        let (handle, is_new) = delegate.get_or_create_service_worker("sw.js", "/", None);
        assert!(is_new);
        assert_eq!(delegate.service_worker_count(), 1);

        // Re-register same (script_url, scope) returns existing
        let (handle2, is_new2) = delegate.get_or_create_service_worker("sw.js", "/", None);
        assert!(!is_new2);
        assert_eq!(delegate.service_worker_count(), 1);
    }

    #[test]
    fn test_delegate_find_service_worker_for_url() {
        let delegate = BaoServoDelegate::new();

        // Register a ServiceWorker for /app/ scope
        let handle = delegate
            .get_or_create_service_worker("sw.js", "/app/", None)
            .0;

        // Not intercepting yet — find_service_worker_for_url returns None
        assert!(delegate.find_service_worker_for_url("/app/page1").is_none());

        // Activate and enable fetch interception
        handle.transition_state(ServiceWorkerRegistrationState::Activated);
        handle.enable_fetch_interception();

        // Now it should be found for URLs in scope
        let found = delegate.find_service_worker_for_url("/app/page1");
        assert!(found.is_some());
        assert_eq!(found.unwrap().script_url, "sw.js");

        // Not found for URLs outside scope
        assert!(
            delegate
                .find_service_worker_for_url("/other/page")
                .is_none()
        );
    }

    #[test]
    fn test_delegate_service_worker_unregistration() {
        let delegate = BaoServoDelegate::new();
        let (handle, _) = delegate.get_or_create_service_worker("sw.js", "/", None);
        assert_eq!(delegate.service_worker_count(), 1);

        let id = handle.id();
        assert!(delegate.unregister_service_worker(&id));
        assert_eq!(delegate.service_worker_count(), 0);

        // Double unregister returns false
        assert!(!delegate.unregister_service_worker(&id));
    }

    #[test]
    fn test_delegate_service_worker_stealth_consistency_no_violations() {
        let delegate = BaoServoDelegate::new();
        let profile = bao_stealth::StealthProfile::chrome_default();

        // Register a ServiceWorker with matching stealth profile
        let handle =
            ServiceWorkerHandle::new("sw.js".to_string(), "/".to_string(), Some(profile.clone()));
        delegate.register_service_worker(handle);
        // Activate to enable fetch interception
        let all = delegate.all_service_workers();
        all[0].transition_state(ServiceWorkerRegistrationState::Activated);
        all[0].enable_fetch_interception();

        let violations = delegate.verify_service_worker_stealth_consistency(&profile);
        assert!(violations.is_empty());
    }

    #[test]
    fn test_delegate_service_worker_stealth_consistency_violation_no_profile() {
        let delegate = BaoServoDelegate::new();
        let profile = bao_stealth::StealthProfile::chrome_default();

        // Register a ServiceWorker without stealth profile — this is a violation
        let handle = ServiceWorkerHandle::new("sw.js".to_string(), "/".to_string(), None);
        delegate.register_service_worker(handle);
        let all = delegate.all_service_workers();
        all[0].transition_state(ServiceWorkerRegistrationState::Activated);
        all[0].enable_fetch_interception();

        let violations = delegate.verify_service_worker_stealth_consistency(&profile);
        assert_eq!(violations.len(), 1);
    }

    #[test]
    fn test_service_worker_persistent_lifecycle_across_page_navigation() {
        // @trace REQ-BRW-004 [entity:ServiceWorker] [criterion:19]
        // SPEC criterion #19: "SW 持久生命周期(跨页存活)下 profile 继承注册页
        // 且 terminate 后正确注销"
        let delegate = BaoServoDelegate::new();
        let profile = bao_stealth::StealthProfile::chrome_default();

        // Page 1 registers a ServiceWorker
        let (handle, is_new) =
            delegate.get_or_create_service_worker("sw.js", "/", Some(profile.clone()));
        assert!(is_new);
        handle.transition_state(ServiceWorkerRegistrationState::Activated);
        handle.enable_fetch_interception();

        // Page 1 is controlled by the ServiceWorker
        let mut page_state = BaoWebViewState::default();
        page_state.set_controlling_service_worker(handle.clone());
        let reg_id = ServiceWorkerRegistrationId {
            script_url: "sw.js".to_string(),
            scope: "/".to_string(),
        };
        let config = ServiceWorkerScopeConfig::from(&profile);
        page_state
            .register_service_worker_scope(ServiceWorkerGlobalScopeState::new(reg_id, &config));
        assert!(page_state.is_controlled_by_service_worker());

        // Page navigation: clear controlling reference (SW survives in delegate registry)
        page_state.clear_controlling_service_worker();
        assert!(!page_state.is_controlled_by_service_worker());

        // ServiceWorker still exists in delegate registry
        assert_eq!(delegate.service_worker_count(), 1);
        assert!(delegate.find_service_worker_for_url("/page2").is_some());

        // Page 2 can be controlled by the same ServiceWorker
        let mut page2_state = BaoWebViewState::default();
        page2_state.set_controlling_service_worker(handle.clone());
        assert!(page2_state.is_controlled_by_service_worker());
    }

    #[test]
    fn test_service_worker_fetch_intercept_mode() {
        let handle = ServiceWorkerHandle::new("sw.js".to_string(), "/".to_string(), None);
        assert_eq!(
            handle.fetch_intercept_mode(),
            ServiceWorkerFetchInterceptMode::None
        );

        handle.enable_fetch_interception();
        assert_eq!(
            handle.fetch_intercept_mode(),
            ServiceWorkerFetchInterceptMode::Intercepting
        );

        handle.disable_fetch_interception();
        assert_eq!(
            handle.fetch_intercept_mode(),
            ServiceWorkerFetchInterceptMode::None
        );
    }

    #[test]
    fn test_service_worker_registration_state_all_transitions() {
        let handle = ServiceWorkerHandle::new("sw.js".to_string(), "/".to_string(), None);
        assert_eq!(
            handle.registration_state(),
            ServiceWorkerRegistrationState::Installing
        );

        handle.transition_state(ServiceWorkerRegistrationState::Installed);
        assert_eq!(
            handle.registration_state(),
            ServiceWorkerRegistrationState::Installed
        );

        handle.transition_state(ServiceWorkerRegistrationState::Activating);
        assert_eq!(
            handle.registration_state(),
            ServiceWorkerRegistrationState::Activating
        );

        handle.transition_state(ServiceWorkerRegistrationState::Activated);
        assert_eq!(
            handle.registration_state(),
            ServiceWorkerRegistrationState::Activated
        );

        handle.transition_state(ServiceWorkerRegistrationState::Redundant);
        assert_eq!(
            handle.registration_state(),
            ServiceWorkerRegistrationState::Redundant
        );
    }

    #[test]
    fn test_service_worker_navigator_from_scope_config() {
        let config = ServiceWorkerScopeConfig {
            stealth_profile: None,
            user_agent: "Mozilla/5.0 Test".to_string(),
            platform: "Linux x86_64".to_string(),
            hardware_concurrency: 4,
            language: "zh-CN".to_string(),
            languages: vec!["zh-CN".to_string(), "zh".to_string()],
            registering_page_url: "https://example.com/".to_string(),
        };
        let nav = WorkerNavigator::from_scope_config(&config);
        assert_eq!(nav.user_agent, "Mozilla/5.0 Test");
        assert_eq!(nav.platform, "Linux x86_64");
        assert_eq!(nav.hardware_concurrency, 4);
        assert_eq!(nav.language, "zh-CN");
        assert_eq!(nav.languages, vec!["zh-CN".to_string(), "zh".to_string()]);
    }

    #[test]
    fn test_webview_state_service_worker_scope_config() {
        let mut state = BaoWebViewState::default();
        let reg_id = ServiceWorkerRegistrationId {
            script_url: "sw.js".to_string(),
            scope: "/".to_string(),
        };
        let config = ServiceWorkerScopeConfig::default();
        let scope = ServiceWorkerGlobalScopeState::new(reg_id, &config);
        let handle = ServiceWorkerHandle::new("sw.js".to_string(), "/".to_string(), None);
        state.set_controlling_service_worker(handle);
        state.register_service_worker_scope(scope);

        let new_config = ServiceWorkerScopeConfig {
            user_agent: "Updated Agent".to_string(),
            ..ServiceWorkerScopeConfig::default()
        };
        state.set_service_worker_scope_config(&new_config);
        assert_eq!(
            state.service_worker_scope().unwrap().navigator().user_agent,
            "Updated Agent"
        );
    }
}
