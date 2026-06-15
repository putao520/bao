//! 高层 API 类 — Playwright 风格的 Browser/BrowserContext/Page/Frame/... 抽象。
//!
//! # D 类 method(REQ-BAO-API-006)
//!
//! D 类 62 method 是"纯本地状态访问"的 getter — 不触发任何 CDP 命令往返。
//! 它们读取 Page/BrowserContext 等高层类内部缓存的本地状态(包括
//! EventEmitter handler 列表、frame tree、本地实例引用等)。
//!
//! # 模块组织
//!
//! - [`event_emitter`]:共享 EventEmitter trait + Inner
//! - [`browser`]:Browser 类(is_connected/version/disconnect/close/...)
//! - [`browser_context`]:BrowserContext(pages/permissions/close/...)
//! - [`page`]:Page 类(核心,~30 D method)
//! - [`frame`]:Frame 类
//! - [`element_handle`]:ElementHandle(继承自 JSHandle)
//! - [`js_handle`]:JSHandle(jsonValue/dispose)
//! - [`request`]:Request(url/method/headers/...)
//! - [`response`]:Response(status/headers/...)
//! - [`dialog`]:Dialog(type/message/...)
//! - [`console_message`]:ConsoleMessage(type/text)
//! - [`keyboard`]:Keyboard(Page 持有的本地实例)
//! - [`mouse`]:Mouse
//! - [`coverage`]:Coverage
//! - [`tracing`]:Tracing
//! - [`accessibility`]:Accessibility
//!
//! # 单线程约束
//!
//! 所有类 `!Send + !Sync`,用 `Rc<RefCell<...>>` 共享 — 与 servo
//! `JSContext` 单线程寄生模型一致(DEC-JSC-001)。
//!
//! @trace REQ-BAO-API-006 [level:library]

#[macro_use]
pub mod event_emitter;
pub mod accessibility;
pub mod browser;
pub mod browser_context;
pub mod console_message;
pub mod coverage;
pub mod dialog;
pub mod element_handle;
pub mod frame;
pub mod js_handle;
pub mod keyboard;
pub mod mouse;
pub mod page;
pub mod request;
pub mod response;
pub mod touchscreen;
pub mod tracing;

// 顶层 re-export
pub use accessibility::Accessibility;
pub use browser::{Browser as HighLevelBrowser, BrowserOptions, Pid};
pub use browser_context::{BrowserContext, ContextOptions, PermissionOverride};
pub use console_message::ConsoleMessage;
pub use coverage::Coverage;
pub use dialog::Dialog;
pub use element_handle::ElementHandle;
pub use event_emitter::{
    EventHandler, HandlerId, EventEmitter, EventEmitterInner,
    SubscriptionResult,
};
// `delegate_event_emitter!` 是 #[macro_export],挂在 crate 根。
pub use crate::delegate_event_emitter;
pub use frame::Frame;
pub use js_handle::JSHandle;
pub use keyboard::Keyboard;
pub use mouse::Mouse;
pub use page::{Page, TargetInfo as PageTargetInfo, Viewport};
pub use request::Request;
pub use response::Response;
pub use touchscreen::Touchscreen;
pub use tracing::Tracing;
