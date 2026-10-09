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

// ─── WorkerLocation (REQ-BRW-004 entity:WorkerLocation) ──────────────
// @trace REQ-BRW-004 [entity:WorkerLocation]
// SPEC entity:WorkerLocation — represents the Worker's location object
// (self.location in DedicatedWorkerGlobalScope). Parsed from the Worker's
// script URL. All fields are derived from the script URL per the Web IDL
// WorkerLocation interface.

/// Represents the Worker's location object (self.location).
///
/// Parsed from the Worker's script URL. All fields are derived per the
/// Web IDL WorkerLocation interface:
///   href = the full URL
///   protocol = the URL scheme (e.g., "https:")
///   host = hostname:port (port omitted if default)
///   hostname = the URL hostname
///   port = the URL port (empty string if default)
///   pathname = the URL path
///   search = the URL query string (including "?")
///   hash = the URL fragment (including "#")
///   origin = the origin (scheme + host + port)
///
/// @trace REQ-BRW-004 [entity:WorkerLocation]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerLocation {
    /// The full URL of the Worker script.
    /// @trace REQ-BRW-004 [entity:WorkerLocation]
    pub href: String,
    /// The URL scheme (e.g., "https:").
    /// @trace REQ-BRW-004 [entity:WorkerLocation]
    pub protocol: String,
    /// The host (hostname:port, port omitted if default).
    /// @trace REQ-BRW-004 [entity:WorkerLocation]
    pub host: String,
    /// The URL hostname.
    /// @trace REQ-BRW-004 [entity:WorkerLocation]
    pub hostname: String,
    /// The URL port (empty string if default for the scheme).
    /// @trace REQ-BRW-004 [entity:WorkerLocation]
    pub port: String,
    /// The URL path.
    /// @trace REQ-BRW-004 [entity:WorkerLocation]
    pub pathname: String,
    /// The URL query string (including "?", or empty string).
    /// @trace REQ-BRW-004 [entity:WorkerLocation]
    pub search: String,
    /// The URL fragment (including "#", or empty string).
    /// @trace REQ-BRW-004 [entity:WorkerLocation]
    pub hash: String,
    /// The origin (scheme + host + port).
    /// @trace REQ-BRW-004 [entity:WorkerLocation]
    pub origin: String,
}

impl WorkerLocation {
    /// Parse a WorkerLocation from a script URL string.
    ///
    /// Returns None if the URL cannot be parsed.
    ///
    /// @trace REQ-BRW-004 [entity:WorkerLocation]
    pub fn from_url(url_str: &str) -> Option<Self> {
        let parsed = url::Url::parse(url_str).ok()?;
        let scheme = parsed.scheme();
        let host = parsed.host_str().unwrap_or("");
        let port = parsed.port();
        let default_port_for_scheme = match scheme {
            "http" => Some(80),
            "https" => Some(443),
            _ => None,
        };
        let is_default_port = port.map_or(true, |p| Some(p) == default_port_for_scheme);
        let host_with_port = if is_default_port {
            host.to_string()
        } else {
            format!("{}:{}", host, port.unwrap())
        };
        let origin = if scheme == "http" || scheme == "https" {
            if is_default_port {
                format!("{}://{}", scheme, host)
            } else {
                format!("{}://{}:{}", scheme, host, port.unwrap())
            }
        } else {
            "null".to_string()
        };

        Some(WorkerLocation {
            href: url_str.to_string(),
            protocol: format!("{}:", scheme),
            host: host_with_port,
            hostname: host.to_string(),
            port: port.map_or(String::new(), |p| p.to_string()),
            pathname: parsed.path().to_string(),
            search: parsed.query().map_or(String::new(), |q| format!("?{}", q)),
            hash: parsed
                .fragment()
                .map_or(String::new(), |f| format!("#{}", f)),
            origin,
        })
    }

    /// Create a WorkerLocation for a local/file URL (used in tests or
    /// when the Worker script is a data: or blob: URL).
    ///
    /// @trace REQ-BRW-004 [entity:WorkerLocation]
    pub fn from_url_value(url: url::Url) -> Self {
        let scheme = url.scheme();
        let host = url.host_str().unwrap_or("");
        let port = url.port();
        let default_port_for_scheme = match scheme {
            "http" => Some(80),
            "https" => Some(443),
            _ => None,
        };
        let is_default_port = port.map_or(true, |p| Some(p) == default_port_for_scheme);
        let host_with_port = if is_default_port {
            host.to_string()
        } else {
            format!("{}:{}", host, port.unwrap())
        };
        let origin = if scheme == "http" || scheme == "https" {
            if is_default_port {
                format!("{}://{}", scheme, host)
            } else {
                format!("{}://{}:{}", scheme, host, port.unwrap())
            }
        } else {
            "null".to_string()
        };
        let href = url.to_string();

        WorkerLocation {
            href,
            protocol: format!("{}:", scheme),
            host: host_with_port,
            hostname: host.to_string(),
            port: port.map_or(String::new(), |p| p.to_string()),
            pathname: url.path().to_string(),
            search: url.query().map_or(String::new(), |q| format!("?{}", q)),
            hash: url.fragment().map_or(String::new(), |f| format!("#{}", f)),
            origin,
        }
    }
}

// ─── WorkerNavigator (REQ-BRW-004 entity:WorkerNavigator) ──────────────
// @trace REQ-BRW-004 [entity:WorkerNavigator]
// SPEC entity:WorkerNavigator — represents the Worker's navigator object
// (self.navigator in DedicatedWorkerGlobalScope). Must match the parent
// page's navigator values per criterion #12 (CRIT-STL-WK).

/// Represents the Worker's navigator object (self.navigator).
///
/// All fingerprint-relevant fields must match the parent page's values
/// per SPEC criterion #12: "CRIT-STL-WK navigator 一致: worker 内
/// navigator.userAgent/platform/hardwareConcurrency/language(s) === 主线程对应值".
///
/// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
#[derive(Debug, Clone)]
pub struct WorkerNavigator {
    /// navigator.userAgent — must match main thread's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub user_agent: String,
    /// navigator.platform — must match main thread's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub platform: String,
    /// navigator.hardwareConcurrency — must match main thread's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub hardware_concurrency: usize,
    /// navigator.language — must match main thread's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub language: String,
    /// navigator.languages — must match main thread's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub languages: Vec<String>,
    /// navigator.connection — NetworkInformation (optional, read-only).
    /// @trace REQ-BRW-004 [entity:WorkerNavigator]
    pub connection: Option<WorkerNetworkInformation>,
    /// navigator.cookieEnabled — mirrors main thread value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator]
    pub cookie_enabled: bool,
    /// navigator.maxTouchPoints — mirrors main thread value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator]
    pub max_touch_points: u32,
    /// navigator.product — always "Gecko" per spec.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator]
    pub product: String,
    /// navigator.appCodeName — always "Mozilla" per spec.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator]
    pub app_code_name: String,
    /// navigator.appName — always "Netscape" per spec.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator]
    pub app_name: String,
    /// navigator.appVersion — mirrors main thread value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator]
    pub app_version: String,
}

/// Network information for WorkerNavigator.connection.
///
/// Represents the NavigatorNetworkInformation subset available in Workers.
///
/// @trace REQ-BRW-004 [entity:WorkerNavigator]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerNetworkInformation {
    /// Effective connection type (e.g., "4g").
    pub effective_type: String,
    /// Downlink speed in Mbps.
    pub downlink: u64,
    /// Round-trip time in ms.
    pub rtt: u64,
    /// Whether the user has requested reduced data usage.
    pub save_data: bool,
}

/// Navigator fingerprint transport shared by every worker scope config
/// (criterion #12): dedicated, shared, and service configs all carry the
/// parent page's navigator values, differing only in scope-specific extras.
///
/// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
pub trait ScopeNavigatorConfig {
    /// The transported navigator fingerprint fields:
    /// (user_agent, platform, hardware_concurrency, language, languages).
    fn navigator_fields(&self) -> (&str, &str, usize, &str, &[String]);
}

/// Implement [`ScopeNavigatorConfig`] for scope configs that declare the
/// five navigator fingerprint fields flat (all current configs do).
macro_rules! impl_scope_navigator_config {
    ($($config:ty),* $(,)?) => {
        $(
            impl ScopeNavigatorConfig for $config {
                fn navigator_fields(&self) -> (&str, &str, usize, &str, &[String]) {
                    (
                        &self.user_agent,
                        &self.platform,
                        self.hardware_concurrency,
                        &self.language,
                        &self.languages,
                    )
                }
            }
        )*
    };
}

impl_scope_navigator_config!(
    WorkerScopeConfig,
    SharedWorkerScopeConfig,
    ServiceWorkerScopeConfig
);

impl WorkerNavigator {
    /// Shared constructor core: every scope config carries the same navigator
    /// fingerprint fields (criterion #12), differing only in the config type
    /// that transports them.
    ///
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    fn from_scope_core(
        user_agent: &str,
        platform: &str,
        hardware_concurrency: usize,
        language: &str,
        languages: &[String],
    ) -> Self {
        WorkerNavigator {
            user_agent: user_agent.to_string(),
            platform: platform.to_string(),
            hardware_concurrency,
            language: language.to_string(),
            languages: languages.to_vec(),
            connection: None,
            cookie_enabled: false,
            max_touch_points: 0,
            product: "Gecko".to_string(),
            app_code_name: "Mozilla".to_string(),
            app_name: "Netscape".to_string(),
            app_version: user_agent.to_string(),
        }
    }

    /// Create a WorkerNavigator from any scope config (dedicated / shared /
    /// service worker). The navigator values are populated from the config
    /// which carries the parent page's fingerprint values (criterion #12).
    ///
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub fn from_scope_config<C: ScopeNavigatorConfig>(config: &C) -> Self {
        let (user_agent, platform, hardware_concurrency, language, languages) =
            config.navigator_fields();
        Self::from_scope_core(
            user_agent,
            platform,
            hardware_concurrency,
            language,
            languages,
        )
    }
}

impl Default for WorkerNavigator {
    fn default() -> Self {
        WorkerNavigator {
            user_agent: String::new(),
            platform: String::new(),
            hardware_concurrency: std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
            language: "en-US".to_string(),
            languages: vec!["en-US".to_string(), "en".to_string()],
            connection: None,
            cookie_enabled: false,
            max_touch_points: 0,
            product: "Gecko".to_string(),
            app_code_name: "Mozilla".to_string(),
            app_name: "Netscape".to_string(),
            app_version: String::new(),
        }
    }
}

// ─── WorkerGlobalScope (REQ-BRW-004 entity:WorkerGlobalScope) ─────────
// @trace REQ-BRW-004 [entity:WorkerGlobalScope]
// SPEC entity:WorkerGlobalScope — the base global scope shared by
// DedicatedWorkerGlobalScope and SharedWorkerGlobalScope. Contains
// the common APIs: self/close/importScripts/setTimeout/fetch/crypto/
// performance/location/navigator/console.

/// The base Worker global scope state tracked by bao_browser.
///
/// This struct represents the bao-side view of a Worker's WorkerGlobalScope.
/// The actual DOM WorkerGlobalScope lives in servo's ScriptThread; this struct
/// tracks the state that bao needs for lifecycle management and CDP observability.
///
/// @trace REQ-BRW-004 [entity:WorkerGlobalScope]
#[derive(Debug, Clone)]
pub struct WorkerGlobalScopeState {
    /// The Worker script URL.
    /// @trace REQ-BRW-004 [entity:WorkerGlobalScope]
    pub worker_url: String,
    /// Whether the Worker is closing (mirrors servo's Worker::closing).
    /// @trace REQ-BRW-004 [entity:WorkerGlobalScope]
    pub closing: bool,
    /// The Worker's location (parsed from worker_url).
    /// @trace REQ-BRW-004 [entity:WorkerGlobalScope] [entity:WorkerLocation]
    pub location: Option<WorkerLocation>,
    /// The Worker's navigator (populated from parent page's config).
    /// @trace REQ-BRW-004 [entity:WorkerGlobalScope] [entity:WorkerNavigator]
    pub navigator: WorkerNavigator,
}

impl WorkerGlobalScopeState {
    /// Shared constructor core: every scope config builds the same
    /// WorkerGlobalScopeState (dedicated / shared / service differ only in
    /// the config type that transports the navigator fingerprint, criterion #12).
    ///
    /// @trace REQ-BRW-004 [entity:WorkerGlobalScope]
    pub fn from_scope_config<C: ScopeNavigatorConfig>(worker_url: String, config: &C) -> Self {
        WorkerGlobalScopeState {
            location: WorkerLocation::from_url(&worker_url),
            navigator: WorkerNavigator::from_scope_config(config),
            worker_url,
            closing: false,
        }
    }

    /// Create a WorkerGlobalScopeState from a script URL and scope config.
    ///
    /// @trace REQ-BRW-004 [entity:WorkerGlobalScope]
    pub fn new(worker_url: String, config: &WorkerScopeConfig) -> Self {
        Self::from_scope_config(worker_url, config)
    }

    /// Create a WorkerGlobalScopeState from a script URL and shared scope config.
    ///
    /// @trace REQ-BRW-004 [entity:WorkerGlobalScope]
    pub fn new_shared(worker_url: String, config: &SharedWorkerScopeConfig) -> Self {
        Self::from_scope_config(worker_url, config)
    }
}

// ─── DedicatedWorkerGlobalScope (REQ-BRW-004 entity) ────────────────
// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope]
// SPEC entity:DedicatedWorkerGlobalScope — the global scope for a
// Dedicated Worker. Extends WorkerGlobalScope with:
//   - parent: reference to the parent page (via WorkerId)
//   - receiver: channel for page→worker messages
//   - onmessage/onerror event handlers
//   - All WorkerGlobalScope APIs (self/close/importScripts/setTimeout/
//     fetch/crypto/performance/location/navigator)

/// The DedicatedWorkerGlobalScope state tracked by bao_browser.
///
/// This struct represents the bao-side view of a Dedicated Worker's global
/// scope. The actual DOM DedicatedWorkerGlobalScope lives in servo's
/// ScriptThread; this struct tracks the state that bao needs for lifecycle
/// management, CDP observability, and message routing.
///
/// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope]
#[derive(Debug, Clone)]
pub struct DedicatedWorkerGlobalScopeState {
    /// The base WorkerGlobalScope state.
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope] [entity:WorkerGlobalScope]
    pub scope: WorkerGlobalScopeState,
    /// The WorkerId identifying this Dedicated Worker.
    /// Links the scope to its WorkerHandle and channel bridge.
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope]
    pub worker_id: WorkerId,
    /// Whether onmessage event handler is registered.
    /// Tracked for CDP observability (Runtime binding reporting).
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope]
    pub has_onmessage: bool,
    /// Whether onerror event handler is registered.
    /// Tracked for CDP observability (Runtime binding reporting).
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope]
    pub has_onerror: bool,
}

impl DedicatedWorkerGlobalScopeState {
    /// Create a DedicatedWorkerGlobalScopeState for the given Worker.
    ///
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope]
    pub fn new(worker_id: WorkerId, config: &WorkerScopeConfig) -> Self {
        let worker_url = worker_id.0.clone();
        DedicatedWorkerGlobalScopeState {
            scope: WorkerGlobalScopeState::new(worker_url, config),
            worker_id,
            has_onmessage: false,
            has_onerror: false,
        }
    }

    /// Get the WorkerLocation for this scope.
    ///
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope] [entity:WorkerLocation]
    pub fn location(&self) -> Option<&WorkerLocation> {
        self.scope.location.as_ref()
    }

    /// Get the WorkerNavigator for this scope.
    ///
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope] [entity:WorkerNavigator]
    pub fn navigator(&self) -> &WorkerNavigator {
        &self.scope.navigator
    }

    /// Mark onmessage handler as registered.
    ///
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope]
    pub fn set_onmessage(&mut self) {
        self.has_onmessage = true;
    }

    /// Mark onerror handler as registered.
    ///
    /// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope]
    pub fn set_onerror(&mut self) {
        self.has_onerror = true;
    }
}

// ─── Worker Scope Config (REQ-BRW-004 criteria #12-17) ────────────
// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope] [criterion:12..17]
// SPEC criterion #12: "CRIT-STL-WK navigator 一致: worker 内
// navigator.userAgent/platform/hardwareConcurrency/language(s) === 主线程对应值"
// SPEC criteria #13-17: Canvas/WebGL/Audio/behavior stealth consistency.
//
// Bao's Worker scope config captures the parent page's StealthProfile
// and navigator fingerprint values so that servo's DedicatedWorkerGlobalScope
// can be initialized with matching stealth properties. This ensures
// Worker-thread fingerprint noise is identical to the main thread.

/// Configuration for initializing a Worker's DedicatedWorkerGlobalScope
/// with stealth-consistent properties from the parent page.
///
/// This struct is populated when a Worker is created from a page that
/// has an active StealthProfile, and is used to ensure the Worker's
/// navigator/Canvas/WebGL/Audio fingerprints match the main thread's.
///
/// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope] [criterion:12..17]
#[derive(Debug, Clone)]
pub struct WorkerScopeConfig {
    /// The StealthProfile to apply in the Worker's global scope.
    /// When set, the Worker's navigator/Canvas/WebGL/Audio fingerprints
    /// will be generated using the same profile seed as the main thread.
    /// @trace REQ-BRW-004 [criterion:12] CRIT-STL-WK navigator 一致
    pub stealth_profile: Option<bao_stealth::StealthProfile>,
    /// Navigator userAgent — must match main thread's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub user_agent: String,
    /// Navigator platform — must match main thread's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub platform: String,
    /// Navigator hardwareConcurrency — must match main thread's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub hardware_concurrency: usize,
    /// Navigator language — must match main thread's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub language: String,
    /// Navigator languages — must match main thread's value.
    /// @trace REQ-BRW-004 [entity:WorkerNavigator] [criterion:12]
    pub languages: Vec<String>,
}

impl Default for WorkerScopeConfig {
    fn default() -> Self {
        WorkerScopeConfig {
            stealth_profile: None,
            user_agent: String::new(),
            platform: String::new(),
            hardware_concurrency: std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
            language: "en-US".to_string(),
            languages: vec!["en-US".to_string(), "en".to_string()],
        }
    }
}

// ─── StealthProfile → WorkerScopeConfig conversion (REQ-BRW-004 criteria #12-17) ───
// @trace REQ-BRW-004 [entity:DedicatedWorkerGlobalScope] [criterion:12..17]
// CRIT-STL-WK: Worker global scope inherits the parent page's StealthProfile
// so that navigator/Canvas/WebGL/Audio fingerprints are identical between
// the main thread and the Worker thread.

impl From<&bao_stealth::StealthProfile> for WorkerScopeConfig {
    /// Convert a StealthProfile into a WorkerScopeConfig for Dedicated Worker inheritance.
    ///
    /// Ensures the Worker thread sees identical navigator/Canvas/WebGL/Audio
    /// fingerprint values as the parent page.
    /// @trace REQ-BRW-004 [criterion:12] CRIT-STL-WK navigator 一致
    fn from(profile: &bao_stealth::StealthProfile) -> Self {
        WorkerScopeConfig {
            stealth_profile: Some(profile.clone()),
            user_agent: profile.navigator.user_agent.clone(),
            platform: profile.navigator.platform.clone(),
            hardware_concurrency: profile.navigator.hardware_concurrency as usize,
            language: profile.navigator.language.clone(),
            languages: profile.navigator.languages.clone(),
        }
    }
}

impl From<&bao_stealth::StealthProfile> for SharedWorkerScopeConfig {
    /// Convert a StealthProfile into a SharedWorkerScopeConfig for Shared Worker inheritance.
    ///
    /// DF-WK-9: SharedWorkerGlobalScope inherits the first connecting page's profile.
    /// @trace REQ-BRW-004 [entity:SharedWorkerGlobalScope] [criterion:12] CRIT-STL-WK navigator 一致
    fn from(profile: &bao_stealth::StealthProfile) -> Self {
        SharedWorkerScopeConfig {
            stealth_profile: Some(profile.clone()),
            user_agent: profile.navigator.user_agent.clone(),
            platform: profile.navigator.platform.clone(),
            hardware_concurrency: profile.navigator.hardware_concurrency as usize,
            language: profile.navigator.language.clone(),
            languages: profile.navigator.languages.clone(),
        }
    }
}

// ─── AutoCloseWorker (REQ-BRW-004 criterion #10) ───────────────────
// @trace REQ-BRW-004 [entity:Worker] [criterion:10]
// SPEC criterion #10: "页面卸载时自动终止所有 Worker
// (GlobalScope::track_worker + AutoCloseWorker)".
//
// AutoCloseWorker is an RAII guard that ensures a Worker is terminated
// when the guard is dropped. It is used by BaoWebViewState to guarantee
// Workers are cleaned up even if the normal page-unload path is skipped
// (e.g., during BrowserRuntime::drop or panic unwinding).

/// RAII guard that terminates a Worker when dropped.
///
/// Created by `BaoWebViewState::track_worker_with_guard`. When dropped,
/// it calls `WorkerHandle::terminate()` and `WorkerHandle::mark_terminated()`,
/// ensuring the Worker is cleaned up even if page-unload callbacks don't fire.
///
/// @trace REQ-BRW-004 [entity:Worker] [criterion:10]
pub struct AutoCloseWorker {
    handle: WorkerHandle,
    /// Tracks which teardown path triggered the close.
    /// Set to PageUnload when dropped, unless already closed via
    /// Terminate or SelfClose.
    teardown_path: WorkerTeardownPath,
}

impl AutoCloseWorker {
    /// Create a new AutoCloseWorker guard for the given WorkerHandle.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:10]
    pub fn new(handle: WorkerHandle) -> Self {
        AutoCloseWorker {
            handle,
            teardown_path: WorkerTeardownPath::PageUnload,
        }
    }

    /// Get the Worker's lifecycle state.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:18]
    pub fn lifecycle_state(&self) -> WorkerLifecycleState {
        if self.handle.is_terminated() {
            WorkerLifecycleState::Terminated(self.teardown_path.clone())
        } else if self.handle.is_closing() {
            WorkerLifecycleState::Closing(self.teardown_path.clone())
        } else {
            WorkerLifecycleState::Running
        }
    }

    /// Signal the Worker to terminate via the given teardown path.
    /// Only transitions from Running → Closing if not already closing.
    ///
    /// @trace REQ-BRW-004 [entity:Worker] [criterion:4] [criterion:10]
    pub fn terminate_via(&mut self, path: WorkerTeardownPath) {
        if !self.handle.is_closing() {
            self.teardown_path = path;
            self.handle.terminate();
        }
    }

    /// Access the underlying WorkerHandle.
    pub fn handle(&self) -> &WorkerHandle {
        &self.handle
    }
}

impl Drop for AutoCloseWorker {
    fn drop(&mut self) {
        // @trace REQ-BRW-004 [entity:Worker] [criterion:10] [criterion:18]
        // Crash-safe teardown on drop (RAII guarantee).
        //
        // When AutoCloseWorker is dropped (page unload, BrowserRuntime::drop,
        // or panic unwinding), we perform crash-safe teardown:
        // 1. Set the closing flag (signals worker event loop to exit)
        // 2. Unregister the Worker's stealth profile from REALM_PROFILES
        // 3. Mark as terminated (RAII guarantee — when the guard is dropped,
        //    the Worker is considered terminated regardless of thread state)
        //
        // The `terminated` flag is set here as an RAII guarantee. In the normal
        // flow, terminate_all_workers() also sets terminated (after joining threads).
        // Both paths are idempotent — mark_terminated() just sets an AtomicBool.
        //
        // The actual thread join is handled by either:
        // - WebWorker::Drop (for bao_engine workers), triggered when
        //   BaoWebViewState.web_workers is cleared in terminate_all_workers()
        // - servo's Worker::drop (for DOM Workers), triggered when the
        //   Worker DOM object is garbage collected
        //
        // We cannot join the thread here because:
        // - AutoCloseWorker::drop may run during panic unwinding, and
        //   joining a thread during unwinding can deadlock
        // - The WebWorker instance is held separately in BaoWebViewState
        if !self.handle.is_closing() {
            self.teardown_path = WorkerTeardownPath::PageUnload;
            self.handle.terminate();
        }
        // @trace REQ-BRW-004 [criterion:18] REALM_PROFILES 条目注销
        // Unregister the Worker's stealth profile to prevent stale entries.
        self.handle.unregister_stealth_profile();
        // @trace REQ-BRW-004 [criterion:18] mark terminated (RAII guarantee)
        // Mark terminated as RAII guarantee — the guard is the last line of defense.
        // In the normal terminate_all_workers() flow, this runs after thread join.
        // In the RAII Drop path (panic/BrowserRuntime::drop), this is the final cleanup.
        self.handle.mark_terminated();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── AutoCloseWorker (REQ-BRW-004 criterion #10) ─────────────────
    // @trace REQ-BRW-004 [req:REQ-BRW-004] [criterion:10] [level:unit]

    #[test]
    fn test_auto_close_worker_new_is_running() {
        let handle = WorkerHandle::new("worker.js".to_string());
        let guard = AutoCloseWorker::new(handle);
        assert!(!guard.handle().is_closing());
        assert!(!guard.handle().is_terminated());
    }

    #[test]
    fn test_auto_close_worker_terminate_via() {
        let handle = WorkerHandle::new("worker.js".to_string());
        let mut guard = AutoCloseWorker::new(handle);
        guard.terminate_via(WorkerTeardownPath::Terminate);
        assert!(guard.handle().is_closing());
        assert_eq!(
            guard.lifecycle_state(),
            WorkerLifecycleState::Closing(WorkerTeardownPath::Terminate)
        );
    }

    #[test]
    fn test_auto_close_worker_terminate_via_idempotent() {
        let handle = WorkerHandle::new("worker.js".to_string());
        let mut guard = AutoCloseWorker::new(handle);
        guard.terminate_via(WorkerTeardownPath::Terminate);
        guard.terminate_via(WorkerTeardownPath::SelfClose);
        // Should still be Terminate (first call wins)
        assert_eq!(
            guard.lifecycle_state(),
            WorkerLifecycleState::Closing(WorkerTeardownPath::Terminate)
        );
    }

    #[test]
    fn test_auto_close_worker_drop_terminates() {
        let handle = WorkerHandle::new("worker.js".to_string());
        let handle_clone = handle.clone();
        let guard = AutoCloseWorker::new(handle);
        assert!(!handle_clone.is_closing());
        drop(guard);
        // AutoCloseWorker::drop should terminate the worker
        assert!(handle_clone.is_closing());
    }

    #[test]
    fn test_auto_close_worker_drop_already_closing() {
        let handle = WorkerHandle::new("worker.js".to_string());
        let handle_clone = handle.clone();
        let mut guard = AutoCloseWorker::new(handle);
        guard.terminate_via(WorkerTeardownPath::Terminate);
        drop(guard);
        // Already closing — drop should not change teardown path
        assert!(handle_clone.is_closing());
        // @trace REQ-BRW-004 [criterion:18] crash-safe teardown: drop marks terminated
        assert!(handle_clone.is_terminated());
    }

    // ─── WorkerScopeConfig (REQ-BRW-004 criteria #12-17) ─────────────
    // @trace REQ-BRW-004 [req:REQ-BRW-004] [criterion:12..17] [level:unit]

    #[test]
    fn test_worker_scope_config_default() {
        let config = WorkerScopeConfig::default();
        assert!(config.stealth_profile.is_none());
        assert!(config.user_agent.is_empty());
        assert!(config.platform.is_empty());
        assert!(config.hardware_concurrency > 0);
        assert_eq!(config.language, "en-US");
        assert!(!config.languages.is_empty());
    }

    #[test]
    fn test_worker_scope_config_set_on_state() {
        let mut state = BaoWebViewState::default();
        let config = WorkerScopeConfig {
            stealth_profile: None,
            user_agent: "Bao/1.0".to_string(),
            platform: "Linux x86_64".to_string(),
            hardware_concurrency: 8,
            language: "zh-CN".to_string(),
            languages: vec!["zh-CN".to_string(), "zh".to_string(), "en".to_string()],
        };
        state.set_worker_scope_config(config);
        assert_eq!(state.worker_scope_config.user_agent, "Bao/1.0");
        assert_eq!(state.worker_scope_config.platform, "Linux x86_64");
        assert_eq!(state.worker_scope_config.hardware_concurrency, 8);
        assert_eq!(state.worker_scope_config.language, "zh-CN");
        assert_eq!(state.worker_scope_config.languages.len(), 3);
    }

    #[test]
    fn test_webview_state_default_worker_scope_config() {
        let state = BaoWebViewState::default();
        assert!(state.worker_scope_config.stealth_profile.is_none());
        assert!(state.worker_scope_config.hardware_concurrency > 0);
    }

    // ─── WorkerLocation (REQ-BRW-004 entity:WorkerLocation) ──────────────
    // @trace REQ-BRW-004 [req:REQ-BRW-004] [entity:WorkerLocation] [level:unit]

    #[test]
    fn test_worker_location_from_https_url() {
        let loc = WorkerLocation::from_url("https://example.com:8080/path?q=1#hash").unwrap();
        assert_eq!(loc.href, "https://example.com:8080/path?q=1#hash");
        assert_eq!(loc.protocol, "https:");
        assert_eq!(loc.host, "example.com:8080");
        assert_eq!(loc.hostname, "example.com");
        assert_eq!(loc.port, "8080");
        assert_eq!(loc.pathname, "/path");
        assert_eq!(loc.search, "?q=1");
        assert_eq!(loc.hash, "#hash");
        assert_eq!(loc.origin, "https://example.com:8080");
    }

    #[test]
    fn test_worker_location_from_default_port() {
        let loc = WorkerLocation::from_url("https://example.com/path").unwrap();
        assert_eq!(loc.host, "example.com");
        assert_eq!(loc.port, "");
        assert_eq!(loc.origin, "https://example.com");
    }

    #[test]
    fn test_worker_location_from_http_url() {
        let loc = WorkerLocation::from_url("http://localhost:3000/worker.js").unwrap();
        assert_eq!(loc.protocol, "http:");
        assert_eq!(loc.hostname, "localhost");
        assert_eq!(loc.port, "3000");
        assert_eq!(loc.pathname, "/worker.js");
    }

    #[test]
    fn test_worker_location_from_url_no_query_no_hash() {
        let loc = WorkerLocation::from_url("https://example.com/worker.js").unwrap();
        assert_eq!(loc.search, "");
        assert_eq!(loc.hash, "");
    }

    #[test]
    fn test_worker_location_from_invalid_url() {
        assert!(WorkerLocation::from_url("not a url").is_none());
    }

    #[test]
    fn test_worker_location_from_url_value() {
        let url = url::Url::parse("https://example.com/worker.js").unwrap();
        let loc = WorkerLocation::from_url_value(url);
        assert_eq!(loc.protocol, "https:");
        assert_eq!(loc.hostname, "example.com");
        assert_eq!(loc.pathname, "/worker.js");
    }

    // ─── WorkerNavigator (REQ-BRW-004 entity:WorkerNavigator) ──────────
    // @trace REQ-BRW-004 [req:REQ-BRW-004] [entity:WorkerNavigator] [level:unit]

    #[test]
    fn test_worker_navigator_default() {
        let nav = WorkerNavigator::default();
        assert!(nav.user_agent.is_empty());
        assert!(nav.platform.is_empty());
        assert!(nav.hardware_concurrency > 0);
        assert_eq!(nav.language, "en-US");
        assert!(!nav.languages.is_empty());
        assert!(nav.connection.is_none());
        assert!(!nav.cookie_enabled);
        assert_eq!(nav.max_touch_points, 0);
        assert_eq!(nav.product, "Gecko");
        assert_eq!(nav.app_code_name, "Mozilla");
        assert_eq!(nav.app_name, "Netscape");
        assert!(nav.app_version.is_empty());
    }

    #[test]
    fn test_worker_navigator_from_scope_config() {
        let config = WorkerScopeConfig {
            stealth_profile: None,
            user_agent: "Bao/1.0".to_string(),
            platform: "Linux x86_64".to_string(),
            hardware_concurrency: 8,
            language: "zh-CN".to_string(),
            languages: vec!["zh-CN".to_string(), "zh".to_string()],
        };
        let nav = WorkerNavigator::from_scope_config(&config);
        assert_eq!(nav.user_agent, "Bao/1.0");
        assert_eq!(nav.platform, "Linux x86_64");
        assert_eq!(nav.hardware_concurrency, 8);
        assert_eq!(nav.language, "zh-CN");
        assert_eq!(nav.languages.len(), 2);
        assert_eq!(nav.app_version, "Bao/1.0"); // app_version mirrors user_agent
        assert_eq!(nav.product, "Gecko");
        assert_eq!(nav.app_code_name, "Mozilla");
        assert_eq!(nav.app_name, "Netscape");
    }

    #[test]
    fn test_worker_navigator_from_shared_scope_config() {
        let config = SharedWorkerScopeConfig {
            stealth_profile: None,
            user_agent: "Bao/2.0".to_string(),
            platform: "MacOS".to_string(),
            hardware_concurrency: 4,
            language: "ja".to_string(),
            languages: vec!["ja".to_string(), "en".to_string()],
        };
        let nav = WorkerNavigator::from_scope_config(&config);
        assert_eq!(nav.user_agent, "Bao/2.0");
        assert_eq!(nav.platform, "MacOS");
        assert_eq!(nav.hardware_concurrency, 4);
        assert_eq!(nav.app_version, "Bao/2.0");
    }

    #[test]
    fn test_worker_network_information() {
        let info = WorkerNetworkInformation {
            effective_type: "4g".to_string(),
            downlink: 10,
            rtt: 50,
            save_data: false,
        };
        assert_eq!(info.effective_type, "4g");
        assert_eq!(info.downlink, 10);
        assert_eq!(info.rtt, 50);
        assert!(!info.save_data);
    }

    // ─── WorkerGlobalScopeState (REQ-BRW-004 entity:WorkerGlobalScope) ──
    // @trace REQ-BRW-004 [req:REQ-BRW-004] [entity:WorkerGlobalScope] [level:unit]

    #[test]
    fn test_worker_global_scope_state_new() {
        let config = WorkerScopeConfig {
            stealth_profile: None,
            user_agent: "Bao/1.0".to_string(),
            platform: "Linux".to_string(),
            hardware_concurrency: 8,
            language: "en-US".to_string(),
            languages: vec!["en-US".to_string()],
        };
        let scope =
            WorkerGlobalScopeState::new("https://example.com/worker.js".to_string(), &config);
        assert_eq!(scope.worker_url, "https://example.com/worker.js");
        assert!(!scope.closing);
        assert!(scope.location.is_some());
        assert_eq!(scope.navigator.user_agent, "Bao/1.0");
    }

    #[test]
    fn test_worker_global_scope_state_new_shared() {
        let config = SharedWorkerScopeConfig {
            stealth_profile: None,
            user_agent: "Bao/2.0".to_string(),
            platform: "MacOS".to_string(),
            hardware_concurrency: 4,
            language: "ja".to_string(),
            languages: vec!["ja".to_string()],
        };
        let scope =
            WorkerGlobalScopeState::new_shared("https://example.com/sw.js".to_string(), &config);
        assert_eq!(scope.worker_url, "https://example.com/sw.js");
        assert_eq!(scope.navigator.user_agent, "Bao/2.0");
    }

    #[test]
    fn test_worker_global_scope_state_location_parsed() {
        let config = WorkerScopeConfig::default();
        let scope = WorkerGlobalScopeState::new(
            "https://example.com:8080/app/worker.js?debug=true#section".to_string(),
            &config,
        );
        let loc = scope.location.unwrap();
        assert_eq!(loc.hostname, "example.com");
        assert_eq!(loc.port, "8080");
        assert_eq!(loc.pathname, "/app/worker.js");
        assert_eq!(loc.search, "?debug=true");
        assert_eq!(loc.hash, "#section");
    }

    #[test]
    fn test_worker_global_scope_state_invalid_url_no_location() {
        let config = WorkerScopeConfig::default();
        let scope = WorkerGlobalScopeState::new("not-a-url".to_string(), &config);
        assert!(scope.location.is_none());
    }

    // ─── DedicatedWorkerGlobalScopeState (REQ-BRW-004 entity) ────────────
    // @trace REQ-BRW-004 [req:REQ-BRW-004] [entity:DedicatedWorkerGlobalScope] [level:unit]

    #[test]
    fn test_dedicated_worker_global_scope_state_new() {
        let worker_id = WorkerId("https://example.com/worker.js".to_string());
        let config = WorkerScopeConfig {
            stealth_profile: None,
            user_agent: "Bao/1.0".to_string(),
            platform: "Linux".to_string(),
            hardware_concurrency: 8,
            language: "en-US".to_string(),
            languages: vec!["en-US".to_string()],
        };
        let scope = DedicatedWorkerGlobalScopeState::new(worker_id.clone(), &config);
        assert_eq!(scope.worker_id, worker_id);
        assert!(!scope.has_onmessage);
        assert!(!scope.has_onerror);
        assert_eq!(scope.scope.navigator.user_agent, "Bao/1.0");
    }

    #[test]
    fn test_dedicated_worker_global_scope_state_location() {
        let worker_id = WorkerId("https://example.com/worker.js".to_string());
        let config = WorkerScopeConfig::default();
        let scope = DedicatedWorkerGlobalScopeState::new(worker_id, &config);
        let loc = scope.location().unwrap();
        assert_eq!(loc.hostname, "example.com");
        assert_eq!(loc.pathname, "/worker.js");
    }

    #[test]
    fn test_dedicated_worker_global_scope_state_navigator() {
        let worker_id = WorkerId("worker.js".to_string());
        let config = WorkerScopeConfig {
            stealth_profile: None,
            user_agent: "Bao/1.0".to_string(),
            platform: "Linux".to_string(),
            hardware_concurrency: 8,
            language: "zh-CN".to_string(),
            languages: vec!["zh-CN".to_string()],
        };
        let scope = DedicatedWorkerGlobalScopeState::new(worker_id, &config);
        let nav = scope.navigator();
        assert_eq!(nav.user_agent, "Bao/1.0");
        assert_eq!(nav.hardware_concurrency, 8);
    }

    #[test]
    fn test_dedicated_worker_global_scope_state_event_handlers() {
        let worker_id = WorkerId("worker.js".to_string());
        let config = WorkerScopeConfig::default();
        let mut scope = DedicatedWorkerGlobalScopeState::new(worker_id, &config);
        assert!(!scope.has_onmessage);
        assert!(!scope.has_onerror);
        scope.set_onmessage();
        assert!(scope.has_onmessage);
        assert!(!scope.has_onerror);
        scope.set_onerror();
        assert!(scope.has_onmessage);
        assert!(scope.has_onerror);
    }

    // ─── StealthProfile → WorkerScopeConfig conversion (REQ-BRW-004 criteria #12-17) ───
    // @trace REQ-BRW-004 [criterion:12..17] CRIT-STL-WK

    #[test]
    fn test_worker_scope_config_from_stealth_profile_chrome() {
        let profile = bao_stealth::StealthProfile::chrome_default();
        let config = WorkerScopeConfig::from(&profile);

        assert!(
            config.stealth_profile.is_some(),
            "stealth_profile must be Some"
        );
        assert_eq!(config.user_agent, profile.navigator.user_agent);
        assert_eq!(config.platform, profile.navigator.platform);
        assert_eq!(
            config.hardware_concurrency,
            profile.navigator.hardware_concurrency as usize
        );
        assert_eq!(config.language, profile.navigator.language);
        assert_eq!(config.languages, profile.navigator.languages);
        assert!(
            config.user_agent.contains("Chrome"),
            "Chrome profile UA must contain Chrome"
        );
    }

    #[test]
    fn test_worker_scope_config_from_stealth_profile_firefox() {
        let profile = bao_stealth::StealthProfile::firefox_default();
        let config = WorkerScopeConfig::from(&profile);

        assert!(
            config.stealth_profile.is_some(),
            "stealth_profile must be Some"
        );
        assert_eq!(config.user_agent, profile.navigator.user_agent);
        assert_eq!(config.platform, profile.navigator.platform);
        assert_eq!(
            config.hardware_concurrency,
            profile.navigator.hardware_concurrency as usize
        );
        assert_eq!(config.language, profile.navigator.language);
        assert_eq!(config.languages, profile.navigator.languages);
        assert!(
            config.user_agent.contains("Firefox"),
            "Firefox profile UA must contain Firefox"
        );
    }

    #[test]
    fn test_shared_worker_scope_config_from_stealth_profile() {
        let profile = bao_stealth::StealthProfile::chrome_default();
        let config = SharedWorkerScopeConfig::from(&profile);

        assert!(
            config.stealth_profile.is_some(),
            "stealth_profile must be Some"
        );
        assert_eq!(config.user_agent, profile.navigator.user_agent);
        assert_eq!(config.platform, profile.navigator.platform);
        assert_eq!(
            config.hardware_concurrency,
            profile.navigator.hardware_concurrency as usize
        );
        assert_eq!(config.language, profile.navigator.language);
        assert_eq!(config.languages, profile.navigator.languages);
    }

    #[test]
    fn test_worker_scope_config_from_stealth_profile_carries_canvas_webgl_audio() {
        // CRIT-STL-WK #13-17: Canvas/WebGL/Audio seeds must be identical
        // between the profile and the WorkerScopeConfig's embedded profile.
        let profile = bao_stealth::StealthProfile::chrome_default();
        let config = WorkerScopeConfig::from(&profile);
        let worker_profile = config.stealth_profile.unwrap();

        assert_eq!(
            worker_profile.canvas.seed(),
            profile.canvas.seed(),
            "Canvas seed must match"
        );
        assert!(
            (worker_profile.canvas.noise_amplitude() - profile.canvas.noise_amplitude()).abs()
                < f64::EPSILON,
            "Canvas amplitude must match"
        );
        assert_eq!(
            worker_profile.audio.seed(),
            profile.audio.seed(),
            "Audio seed must match"
        );
        assert_eq!(
            worker_profile.webgl.vendor, profile.webgl.vendor,
            "WebGL vendor must match"
        );
        assert_eq!(
            worker_profile.webgl.renderer, profile.webgl.renderer,
            "WebGL renderer must match"
        );
    }

    #[test]
    fn test_worker_scope_config_from_different_profiles_produces_different_configs() {
        // @trace REQ-BRW-004 [criterion:17] new Worker 后 worker 回传指纹摘要 === 主线程指纹摘要
        let chrome = bao_stealth::StealthProfile::chrome_default();
        let firefox = bao_stealth::StealthProfile::firefox_default();
        let chrome_config = WorkerScopeConfig::from(&chrome);
        let firefox_config = WorkerScopeConfig::from(&firefox);

        assert_ne!(chrome_config.user_agent, firefox_config.user_agent);
        assert_ne!(
            chrome_config.stealth_profile.unwrap().canvas.seed(),
            firefox_config.stealth_profile.unwrap().canvas.seed()
        );
    }
}
