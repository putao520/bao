/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Servo, the mighty web browser engine from the future.
//!
//! This is a very simple library that wires all of Servo's components together as
//! type `Servo`, along with a Webview implementation, `WebView` to create a working
//! web browser.
//!
//! The `Servo` type is responsible for configuring a `Constellation`, which does the
//! heavy lifting of coordinating all of Servo's internal subsystems, including the
//! `ScriptThread` and the `LayoutThread`, as well maintains the navigation context.

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

/// Register a callback to be executed on the script thread the next time
/// `WebView::evaluate_javascript` is called for the given WebView.
///
/// The callback receives `(cx: *mut c_void, global: *mut c_void)` which are
/// actually `(*mut mozjs::jsapi::JSContext, *mut mozjs::jsapi::JSObject)`.
/// Cast them to the correct types in your callback.
///
/// This enables embedders (e.g., Bao) to register Rust host functions on
/// servo's Window global object, making them available to page JavaScript.
pub fn register_script_thread_callback(
    webview_id: WebViewId,
    callback: Box<dyn FnOnce(*mut std::ffi::c_void, *mut std::ffi::c_void) + Send>,
) {
    script::script_thread::register_embedder_callback(webview_id, callback);
}

/// Register a callback to be executed on the Worker thread the next time a
/// servo-native `DedicatedWorkerGlobalScope::run_worker_scope` finishes
/// constructing the Worker global object.
///
/// The callback receives `(cx: *mut c_void, global: *mut c_void)` which are
/// actually `(*mut mozjs::jsapi::JSContext, *mut mozjs::jsapi::JSObject)`.
/// Cast them to the correct types in your callback.
///
/// This enables embedders (e.g., Bao) to register Rust host functions on
/// the Worker's global object (DedicatedWorkerGlobalScope), making them
/// available to Worker-scoped JavaScript. It is the Worker-scope analogue
/// of `register_script_thread_callback`.
///
/// Use case (Bao): inject stealth profile inheritance (DEC-WK-007),
/// establish WorkerHandle lifecycle tracking (DF-WK-1), and hook
/// self.close()/importScripts natives (REQ-BRW-004 criteria #4/#5/#8).
///
/// Bao vendor patch (DEC-WK-001 / TASK-1: servo-native Worker path).
pub fn register_worker_scope_callback(
    callback: Box<dyn FnOnce(*mut std::ffi::c_void, *mut std::ffi::c_void) + Send>,
) {
    script::script_thread::register_worker_scope_callback(callback);
}
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

/// Set anti-fingerprinting canvas noise seed and amplitude.
///
/// Called from Bao's runtime bridge during stealth profile initialization.
/// The noise is applied at the servo rendering layer (CanvasData::read_pixels),
/// making it undetectable from JavaScript (REQ-STL-003).
pub fn set_canvas_noise_seed(seed: u64, noise_amplitude: f64) {
    servo_canvas::canvas_noise::set_global_canvas_noise(seed, noise_amplitude);
}

/// Set anti-fingerprinting TLS/HTTP2 configuration for servo's network layer.
///
/// Called from Bao's runtime bridge during stealth profile initialization,
/// following the same pattern as `set_canvas_noise_seed()`. When set, servo's
/// HTTP client uses these values for TLS cipher suite/curves/signature algorithm
/// reordering and ALPN negotiation, plus HTTP/2 connection parameters
/// (SETTINGS frame, window sizes).
///
/// BoringSSL supports full JA3/JA4 fingerprint configuration including cipher
/// suite reordering, curves/groups ordering, and signature algorithm ordering.
pub use net::connector::StealthTlsWireConfig;

pub fn set_stealth_tls_config(config: Option<StealthTlsWireConfig>) {
    net::connector::set_stealth_tls_config(config);
}

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
