/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Servo, the mighty web browser engine from the future.
//!
//! This crate wires all of Servo's components together as type [`Servo`], along with
//! a Webview implementation, [`WebView`], to create a working web engine.
//!
//! To embed Servo in your application, you need to:
//! 1) Create an instance of a type that implements the [`EventLoopWaker`] trait.
//!    This trait allows Servo to integrate with your application's event loop. Your
//!    [`EventLoopWaker::wake`] implementation needs to ensure that
//!    [`Servo::spin_event_loop`] is eventually called to let Servo process user input,
//!    network events etc.
//! 2) Create a [`Servo`] instance using the [`ServoBuilder`] type. You can optionally
//!    register a [`ServoDelegate`] implementation to subscribe to notifications about
//!    events and customize certain behaviours.
//! 3) Create an instance of a type that implements the [`RenderingContext`] trait.
//!    Note: You can either use one of the existing types in this crate (such as
//!    [`WindowRenderingContext`], [`OffscreenRenderingContext`] or [`SoftwareRenderingContext`])
//!    or provide a custom implementation.
//! 4) For each [`WebView`] in your application, create a [`WebViewBuilder`] by passing
//!    it the [`RenderingContext`] and [`Servo`] instance, and then configure the
//!    builder and use it to create a [`WebView`] instance. The builder must be provided
//!    with a [`WebViewDelegate`] implementation which, at a minimum, should handle
//!    [`WebViewDelegate::notify_new_frame_ready`] by calling [`WebView::paint`]
//!    and presenting the rendered page using [`RenderingContext::present`]. Refer to the
//!    documentation of the [`WebView`] type to learn more about the relation
//!    between a [`WebView`] and its [`RenderingContext`].
//! 5) Run the application's event loop. Your application's event handlers need to
//!    forward input events for a particular [`WebView`] using one of the `notify_*` methods
//!    on that [`WebView`] instance. For example, to forward a mouse event to a [`WebView`],
//!    call the [`WebView::notify_input_event`]. You can also invoke methods on a [`WebView`]
//!    to request certain actions. For instance, the [`WebView::load`] method requests that
//!    Servo navigate to a new page. In both cases, the calls to the [`WebView`] methods
//!    must be followed by calls to [`Servo::spin_event_loop`] to allow Servo to process
//!    those requests.
//!
//! For a minimal working example, refer to the [`winit_minimal`] code.
//!
//! [`winit_minimal`]: https://github.com/servo/servo/blob/main/components/servo/examples/winit_minimal.rs

mod clipboard_delegate;
#[cfg(feature = "gamepad")]
mod gamepad_delegate;
#[cfg(feature = "media-gstreamer")]
mod gstreamer_plugins;
mod javascript_evaluator;
mod network_manager;
mod proxies;
mod responders;
mod servo;
mod servo_delegate;
mod site_data_manager;
mod user_content_manager;
mod webview;
mod webview_delegate;

// These are Servo's public exports. Everything (apart from a couple exceptions below)
// should be exported at the root. See <https://github.com/servo/servo/issues/18475>.
pub use accesskit;
pub use embedder_traits::user_contents::UserScript;
pub use embedder_traits::{submit_resource_reader, *};
pub use image::RgbaImage;
pub use keyboard_types::{
    Code, CompositionEvent, CompositionState, Key, KeyState, Location, Modifiers, NamedKey,
};
pub use media::{
    GlApi as MediaGlApi, GlContext as MediaGlContext, NativeDisplay as MediaNativeDisplay,
};
pub use net::image_cache::should_panic_hook_suppress_termination;
pub use net_traits::CookieSource;
// This API should probably not be exposed in this way. Instead there should be a fully
// fleshed out public domains API if we want to expose it.
pub use net_traits::pub_domains::is_reg_domain;
pub use paint::WebRenderDebugOption;
pub use paint_api::rendering_context::{
    OffscreenRenderingContext, RenderingContext, SoftwareRenderingContext, WindowRenderingContext,
};
// Expose our profile traits for servoshell, so we can instrument code there, but don't
// add it as an official API.
#[doc(hidden)]
pub use profile_traits;
// This should be replaced with an API on ServoBuilder.
// See <https://github.com/servo/servo/issues/40950>.
pub use resources;
pub use servo_base::generic_channel::GenericSender;
pub use servo_base::id::WebViewId;
pub use servo_config::opts::{DiagnosticsLogging, DiagnosticsLoggingOption, Opts, OutputOptions};
pub use servo_config::prefs::{PrefValue, Preferences, UserAgentPlatform};
pub use servo_config::{opts, pref, prefs};
pub use servo_geometry::{
    DeviceIndependentIntRect, DeviceIndependentPixel, convert_rect_to_css_pixel,
};
#[doc(hidden)]
pub use servo_tracing;
pub use servo_url::ServoUrl;
pub use style::Zero;
pub use style_traits::CSSPixel;
pub use webrender_api::units::{
    DeviceIntPoint, DeviceIntRect, DeviceIntSize, DevicePixel, DevicePoint, DeviceVector2D,
};

/// Register a callback to be executed on this WebView's ScriptThread the next
/// time `handle_evaluate_javascript` runs, before the evaluated JS executes.
///
/// The callback receives `(cx, global)` as `(*mut c_void, *mut c_void)` which
/// are actually `(*mut mozjs::jsapi::JSContext, *mut mozjs::jsapi::JSObject)`.
/// Cast them to the correct types in your callback.
///
/// This enables embedders (e.g., Bao) to register Rust host functions on
/// servo's Window global object, making them available to page JavaScript.
pub fn register_script_thread_callback(
    webview_id: WebViewId,
    callback: Box<dyn FnOnce(*mut std::ffi::c_void, *mut std::ffi::c_void) + Send>,
) {
    script::register_embedder_callback(webview_id, callback);
}

/// Register the process-global embedder event-loop pump (Bao vendor patch —
/// page-realm async fetch settlement).
///
/// Bao's page-realm `fetch` override resolves through a ConcurrentTask on the
/// ScriptThread's bao MiniEventLoop, which servo never ticks by itself. The
/// registered pump is called by `handle_msgs` on each ScriptThread right
/// after its blocking recv returns (a fetch completion wakes that recv via
/// [`bao_current_thread_wake_fn`]), with the thread's JSContext as
/// `*mut c_void` (actually `*mut mozjs::jsapi::JSContext`).
pub fn register_bao_event_loop_pump(pump: script::BaoEventLoopPump) {
    script::register_bao_event_loop_pump(pump);
}

/// The current thread's wake closure, if this thread is a servo ScriptThread
/// (Bao vendor patch — the fetch side of the pump bridge above). The fetch
/// machinery captures this ON THE CREATING THREAD at fetch start and fires
/// it from its HTTPThread when a resolve lands; the closure sends a
/// `MainThreadScriptMsg::WakeUp` to the owning ScriptThread.
pub fn bao_current_thread_wake_fn() -> Option<std::sync::Arc<dyn Fn() + Send + Sync>> {
    script::bao_current_thread_wake_fn()
}

/// Register a callback to be executed on the Worker thread the next time a
/// servo-native `DedicatedWorkerGlobalScope::run_worker_scope` finishes
/// constructing the Worker global object **for `webview_id`**.
///
/// The callback receives `(cx: *mut c_void, global: *mut c_void)` which are
/// actually `(*mut mozjs::jsapi::JSContext, *mut mozjs::jsapi::JSObject)`.
///
/// Bao vendor patch (DEC-WK-001 / TASK-1: servo-native Worker path): inject
/// stealth profile inheritance, WorkerHandle lifecycle tracking, and
/// self.close()/importScripts native hooks.
/// Bao patch (per-worker association): keyed by WebViewId — a Worker scope
/// creation only drains callbacks registered for its own webview, so page-JS
/// `new Worker()` can never consume another page's queued callback.
pub fn register_worker_scope_callback(
    webview_id: WebViewId,
    callback: Box<dyn FnOnce(*mut std::ffi::c_void, *mut std::ffi::c_void) + Send>,
) {
    script::register_worker_scope_callback(webview_id, callback);
}

/// Register a callback to be executed on the Worker thread the next time a
/// servo-native worker global's WebIDL interfaces are defined **for
/// `webview_id`** — drained inside `WorkerGlobalScope::run_worker_script`
/// right after `define_all_exposed_interfaces` and before the worker script
/// runs.
///
/// The callback receives `(cx: *mut c_void, global: *mut c_void)` which are
/// actually `(*mut mozjs::jsapi::JSContext, *mut mozjs::jsapi::JSObject)`.
///
/// Bao vendor patch (REQ-BRW-004 C15, user ruling 2026-09-09): the FIRST
/// worker-scope drain point runs before the worker's interface objects
/// exist, so bao_stealth's JS prototype hooks (W1a `typeof` guards) were
/// silently skipped there. This second, later drain point lets the embedder
/// re-run its idempotent install once interfaces are defined.
pub fn register_worker_interfaces_ready_callback(
    webview_id: WebViewId,
    callback: Box<dyn FnOnce(*mut std::ffi::c_void, *mut std::ffi::c_void) + Send>,
) {
    script::register_worker_interfaces_ready_callback(webview_id, callback);
}

/// Register a per-Worker injector delivered to EVERY Dedicated Worker scope
/// created for `webview_id` (Bao vendor patch, REQ-BRW-004, user ruling
/// 2026-09-09).
///
/// Unlike the consume-once callbacks above — which only the FIRST Worker of
/// the webview drains — an injector is delivered at every Worker's scope
/// construction (first drain point, after the one-shot callbacks) and is
/// never consumed, so the SECOND and later `new Worker()` in the same page
/// receive the same embedder install (engine getters + JS hooks) instead of
/// a bare, fingerprintable Worker global. The injector receives
/// `(cx: *mut c_void, global: *mut c_void)` which are actually
/// `(*mut mozjs::jsapi::JSContext, *mut mozjs::jsapi::JSObject)` and runs on
/// each Worker thread. Registration is an upsert: one injector per webview.
pub fn register_worker_scope_injector(
    webview_id: WebViewId,
    injector: script::EmbedderWorkerInjector,
) {
    script::register_worker_scope_injector(webview_id, injector);
}

/// Register a per-Worker injector delivered at the SECOND drain point —
/// right after `define_all_exposed_interfaces` and before the worker script
/// runs — for EVERY Dedicated Worker of `webview_id` (never consumed).
///
/// Bao vendor patch (REQ-BRW-004, user ruling 2026-09-09): same per-Worker
/// delivery semantics as [`register_worker_scope_injector`], landing the
/// W1a-guarded JS prototype hooks that require defined interfaces.
pub fn register_worker_interfaces_ready_injector(
    webview_id: WebViewId,
    injector: script::EmbedderWorkerInjector,
) {
    script::register_worker_interfaces_ready_injector(webview_id, injector);
}

/// Remove every per-Worker injector registered for `webview_id` (both drain
/// phases). The embedder calls this when the page closes so a closed page's
/// injectors do not linger in the registries.
pub fn unregister_worker_injectors(webview_id: WebViewId) {
    script::unregister_worker_injectors(webview_id);
}

/// The per-Worker injector type behind
/// [`register_worker_scope_injector`] /
/// [`register_worker_interfaces_ready_injector`]: a shared closure receiving
/// `(cx: *mut c_void, global: *mut c_void)` on each Worker thread of the
/// webview it is registered for.
pub use script::EmbedderWorkerInjector;

/// Set anti-fingerprinting TLS/HTTP2 configuration for servo's network layer
/// (Bao vendor patch, REQ-STL-001: browser-level JA3/JA4 anti-fingerprinting).
///
/// When set, servo's TLS consumers (the page-network bun bridge and the
/// boringssl-backed WebSocket loader) use these
/// values for TLS cipher suite/curves/signature algorithm reordering and ALPN
/// negotiation, plus HTTP/2 connection parameters (SETTINGS frame, window
/// sizes). boringssl supports full JA3/JA4 fingerprint configuration.
pub use net::connector::StealthTlsWireConfig;

pub fn set_stealth_tls_config(config: Option<StealthTlsWireConfig>) {
    net::connector::set_stealth_tls_config(config);
}

/// Network event tap types + installer for embedder-side network observability
/// (Bao vendor patch, REQ-BRW-004 criterion #19 subclause ②: CDP Network
/// domain observability of SW-intercepted and regular fetches).
///
/// When a tap is installed, `main_fetch`'s request/response instrumentation
/// points forward every fetch (service-worker-mediated responses and the SW
/// realm's own sub-fetches included) to the closure. The closure runs on
/// fetch worker threads and must not block.
pub use net::http_loader::{BaoNetworkTap, BaoNetworkTapEvent};

pub fn set_network_event_tap(tap: Option<BaoNetworkTap>) {
    net::http_loader::set_network_event_tap(tap);
}

/// Webview-less `WebResourceRequested` local-verdict types + installer
/// (Bao vendor patch, BCE-20260910-002).
///
/// Upstream servo answers every `WebResourceRequested` round-trip from a
/// resident embedder main loop (`Servo::spin_event_loop`). Bao's embedder
/// is a lazy pump (PageHandle interaction APIs) and `Servo` is !Send, so a
/// fetch with `target_webview_id == None` (SW/worker realms) could park in
/// the interceptor's embedder wait forever while the owning page sat idle.
/// The embedder (bao_browser, at `BaoRuntime::new`) installs one
/// process-wide handler that the net request interceptor consults locally
/// for those requests. With no handler installed, the upstream embedder
/// round-trip is preserved unchanged; webview-owned requests always keep
/// the full round-trip.
pub use net::request_interceptor::{
    BaoWebviewlessResourceHandler, BaoWebviewlessResourceVerdict,
};

pub fn set_webviewless_resource_handler(handler: Option<BaoWebviewlessResourceHandler>) {
    net::request_interceptor::set_webviewless_resource_handler(handler);
}

/// Set anti-fingerprinting canvas noise seed and amplitude (REQ-STL-003).
/// The noise is applied at the servo rendering layer, undetectable from JS.
pub fn set_canvas_noise_seed(seed: u64, noise_amplitude: f64) {
    servo_canvas::canvas_noise::set_global_canvas_noise(seed, noise_amplitude);
}

pub use crate::clipboard_delegate::{ClipboardDelegate, StringRequest};
#[cfg(feature = "gamepad")]
pub use crate::gamepad_delegate::{
    GamepadDelegate, GamepadHapticEffectRequest, GamepadHapticEffectRequestType,
};
pub use crate::network_manager::{CacheEntry, NetworkManager};
pub use crate::servo::{Servo, ServoBuilder, run_content_process};
pub use crate::servo_delegate::{ServoDelegate, ServoError};
pub use crate::site_data_manager::{SiteData, SiteDataManager, StorageType};
pub use crate::user_content_manager::UserContentManager;
pub use crate::webview::{WebView, WebViewBuilder};
pub use crate::webview_delegate::{
    AlertDialog, AllowOrDenyRequest, AuthenticationRequest, BluetoothDeviceSelectionRequest,
    ColorPicker, ConfirmDialog, ContextMenu, CreateNewWebViewRequest, EmbedderControl, FilePicker,
    InputMethodControl, NavigationRequest, PermissionRequest, PromptDialog, SelectElement,
    SimpleDialog, WebResourceLoad, WebViewDelegate,
};

#[cfg(feature = "webxr")]
pub mod webxr {
    #[cfg(not(any(target_os = "android", target_env = "ohos")))]
    pub use webxr::glwindow::{GlWindow, GlWindowDiscovery, GlWindowMode, GlWindowRenderTarget};
    #[cfg(not(any(target_os = "android", target_env = "ohos")))]
    pub use webxr::headless::HeadlessMockDiscovery;
    #[cfg(target_os = "windows")]
    pub use webxr::openxr::{AppInfo as OpenXrAppInfo, OpenXrDiscovery};
    pub use webxr::{Discovery, MainThreadRegistry, WebXrRegistry};
}

// TODO: The protocol handler interface needs to be cleaned and simplified.
pub mod protocol_handler {
    pub use net::fetch::methods::{DoneChannel, FetchContext};
    pub use net::filemanager_thread::FILE_CHUNK_SIZE;
    pub use net::protocols::{ProtocolHandler, ProtocolRegistry};
    pub use net_traits::filemanager_thread::RelativePos;
    pub use net_traits::http_status::HttpStatus;
    pub use net_traits::request::Request;
    pub use net_traits::response::{Response, ResponseBody};
    pub use net_traits::{NetworkError, ResourceFetchTiming};

    pub use crate::webview_delegate::ProtocolHandlerRegistration;
}

// We need to reference this crate, in order for the linker not to remove it.
#[cfg(all(feature = "baked-in-resources", not(target_env = "ohos")))]
use servo_default_resources as _;
