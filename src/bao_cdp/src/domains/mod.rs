// @trace REQ-CDP-001 [entity:DomainRegistry]
mod page;
mod runtime;
mod dom;
mod network;
mod debugger;
mod input;
mod emulation;
mod stub;
mod target;

use cdp_server::DomainRegistry;
use crate::servo_bridge::BridgeSender;

pub use target::ServoTargetProvider;
pub use page::PageHandler;
pub use runtime::RuntimeHandler;
pub use dom::DomHandler;
pub use network::NetworkHandler;

/// Register all CDP domain handlers into an existing DomainRegistry.
pub fn register_all_domains_into(bridge: BridgeSender, registry: &DomainRegistry) {
    registry.register(Box::new(page::PageHandler::new(bridge.clone()))).expect("register Page");
    registry.register(Box::new(runtime::RuntimeHandler::new(bridge.clone()))).expect("register Runtime");
    registry.register(Box::new(dom::DomHandler::new(bridge.clone()))).expect("register DOM");
    registry.register(Box::new(network::NetworkHandler)).expect("register Network");
    registry.register(Box::new(debugger::DebuggerHandler)).expect("register Debugger");
    registry.register(Box::new(input::InputHandler::new(bridge.clone()))).expect("register Input");
    registry.register(Box::new(emulation::EmulationHandler::new(bridge.clone()))).expect("register Emulation");
    registry.register(Box::new(stub::CssHandler)).expect("register CSS");
    registry.register(Box::new(stub::OverlayHandler)).expect("register Overlay");
    registry.register(Box::new(stub::LogHandler)).expect("register Log");
    registry.register(Box::new(stub::FetchHandler)).expect("register Fetch");
}
