// @trace REQ-BRW-001 [entity:BrowserContext]  REQ-CDP-006: Servo delegate hooks for CDP event forwarding
// @trace REQ-BRW-004 [entity:Worker] [entity:DedicatedWorkerGlobalScope] Worker lifecycle + DedicatedWorkerGlobalScope API
// @trace REQ-BRW-004 [entity:SharedWorker] [entity:SharedWorkerGlobalScope] SharedWorker cross-page routing + connect event
// @trace REQ-BRW-004 [entity:ServiceWorker] [entity:ServiceWorkerGlobalScope] ServiceWorker registration + fetch interception + stealth/CDP boundary consistency
// @trace REQ-CDP-006 [entity:ServoDelegateHooks] (servo delegate → CDP event forwarding)

mod service_worker;
mod shared_worker;
mod servo_delegate;
mod webview_delegate;
mod webview_state;
mod worker_channel;
mod worker_lifecycle;
mod worker_scope;
mod worker_script;

pub use service_worker::*;
pub use shared_worker::*;
pub use servo_delegate::*;
pub use webview_delegate::*;
pub use webview_state::*;
pub use worker_channel::*;
pub use worker_lifecycle::*;
pub use worker_scope::*;
pub use worker_script::*;
