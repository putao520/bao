// @trace REQ-CDP-002 [entity:CdpCommand]
mod page;
mod runtime;
mod dom;
pub mod network;
mod debugger;
mod input;
mod emulation;
mod css;
mod overlay;
mod log_domain;
pub mod fetch_domain;
mod target;

use cdp_server::DomainRegistry;
use crate::servo_bridge::BridgeSender;
use crate::DomainDispatch;

pub use target::ServoTargetProvider;
pub use target::TargetHandler;
pub use page::PageHandler;
pub use runtime::RuntimeHandler;
pub use dom::DomHandler;
pub use network::NetworkHandler;
pub use debugger::DebuggerHandler;
pub use emulation::EmulationHandler;
pub use input::InputHandler;
pub use css::CssHandler;
pub use overlay::OverlayHandler;
pub use log_domain::LogHandler;
pub use fetch_domain::FetchHandler;

/// Register all CDP domain handlers into an existing DomainRegistry.
/// Uses DomainDispatch enum for compile-time exhaustiveness — no vtable indirection.
pub fn register_all_domains_into(bridge: BridgeSender, target_id: String, registry: &DomainRegistry<DomainDispatch>) {
    registry.register(DomainDispatch::Page(page::PageHandler::new(bridge.clone(), target_id.clone()))).expect("register Page");
    registry.register(DomainDispatch::Runtime(runtime::RuntimeHandler::new(bridge.clone(), target_id.clone()))).expect("register Runtime");
    registry.register(DomainDispatch::Dom(dom::DomHandler::new(bridge.clone(), target_id.clone()))).expect("register DOM");
    registry.register(DomainDispatch::Network(network::NetworkHandler::new(bridge.clone(), target_id.clone()))).expect("register Network");
    registry.register(DomainDispatch::Debugger(debugger::DebuggerHandler::new(bridge.clone(), target_id.clone()))).expect("register Debugger");
    registry.register(DomainDispatch::Input(input::InputHandler::new(bridge.clone(), target_id.clone()))).expect("register Input");
    registry.register(DomainDispatch::Emulation(emulation::EmulationHandler::new(bridge.clone(), target_id.clone()))).expect("register Emulation");
    registry.register(DomainDispatch::Css(css::CssHandler::new(bridge.clone(), target_id.clone()))).expect("register CSS");
    registry.register(DomainDispatch::Overlay(overlay::OverlayHandler::new(bridge.clone(), target_id.clone()))).expect("register Overlay");
    registry.register(DomainDispatch::Log(log_domain::LogHandler::new())).expect("register Log");
    registry.register(DomainDispatch::Fetch(fetch_domain::FetchHandler::new(bridge, target_id.clone()))).expect("register Fetch");
}

/// Register all 12 CDP domain handlers (including Target) into a DomainRegistry.
/// Uses DomainDispatch enum for compile-time exhaustiveness — no vtable indirection.
pub fn register_all_domains_with_target(bridge: BridgeSender, target_id: String, registry: &DomainRegistry<DomainDispatch>) {
    register_all_domains_into(bridge.clone(), target_id.clone(), registry);
    registry.register(DomainDispatch::Target(target::TargetHandler::new(bridge, target_id))).expect("register Target");
}

// @trace TEST-CDP-DOM-001 [req:REQ-CDP-001] [level:unit] [nfr:TMG-CDP-01]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::servo_bridge::bridge_channel;
    use cdp_server::EventSender;
    use serde_json::{json, Value};
    use std::time::Duration;

    const TIMEOUT: Duration = Duration::from_millis(100);

    struct NoopSender;
    impl EventSender for NoopSender {
        fn send_event(&self, _method: &str, _params: Value) {}
    }

    // 1. register_all_domains_into registers 11 domains
    #[test]
    fn register_all_domains_into_registers_11_domains() {
        let (bridge, _receiver) = bridge_channel(TIMEOUT);
        let registry: DomainRegistry<DomainDispatch> = DomainRegistry::new();
        register_all_domains_into(bridge, "test-target".into(), &registry);

        let expected_domains = [
            "Page", "Runtime", "DOM", "Network", "Debugger",
            "Input", "Emulation", "CSS", "Overlay", "Log", "Fetch",
        ];

        for domain in &expected_domains {
            assert!(registry.has_domain(domain), "domain '{}' should be registered", domain);
        }
    }

    // 2. all domains have correct names and respond to known commands
    #[test]
    fn all_domains_have_correct_names_and_respond_to_known_commands() {
        let (bridge, rx) = bridge_channel(TIMEOUT);
        let registry: DomainRegistry<DomainDispatch> = DomainRegistry::new();
        register_all_domains_into(bridge.clone(), "test-target".into(), &registry);

        // Background responder for bridge commands (Debugger.enable sends BridgeCommand)
        let keeper = bridge.clone();
        std::thread::spawn(move || {
            let _keeper = keeper;
            loop {
                let handled = rx.try_process(|_| crate::servo_bridge::BridgeResponse { result: Ok(json!({})) });
                if !handled {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            }
        });

        // Test non-bridge commands for each domain
        let known_commands: &[(&str, Value)] = &[
            ("Page.enable", json!({})),
            ("Runtime.enable", json!({})),
            ("DOM.enable", json!({})),
            ("Network.enable", json!({})),
            ("Debugger.enable", json!({})),
            ("Input.setIgnoreInputEvents", json!({})),
            ("Emulation.clearDeviceMetricsOverride", json!({})),
            ("CSS.enable", json!({})),
            ("Overlay.enable", json!({})),
            ("Log.enable", json!({})),
            ("Fetch.disable", json!({})),
        ];

        for (command, params) in known_commands {
            let result = registry.dispatch_command(command, params.clone(), &NoopSender);
            assert!(result.is_some(), "{} should return Some", command);
            assert!(result.unwrap().is_ok(), "{} should return Ok", command);
        }
    }

    // 3. Page domain responds to non-bridge commands
    #[test]
    fn page_domain_responds_to_non_bridge_commands() {
        let (bridge, _receiver) = bridge_channel(TIMEOUT);
        let registry: DomainRegistry<DomainDispatch> = DomainRegistry::new();
        register_all_domains_into(bridge, "test-target".into(), &registry);

        // Page.setContent doesn't require bridge
        let result = registry.dispatch_command("Page.setContent", json!({ "html": "<html></html>" }), &NoopSender);
        assert!(result.is_some());
        assert!(result.unwrap().is_ok());

        // Page.getLayoutMetrics doesn't require bridge
        let result = registry.dispatch_command("Page.getLayoutMetrics", json!({}), &NoopSender);
        assert!(result.is_some());
        let response = result.unwrap().unwrap();
        assert!(response.get("contentSize").is_some());
    }

    // 4. CSS domain returns computedStyle structure (bridge returns error, but structure present)
    #[test]
    fn css_domain_returns_computed_style_structure() {
        let (bridge, _receiver) = bridge_channel(TIMEOUT);
        let registry: DomainRegistry<DomainDispatch> = DomainRegistry::new();
        register_all_domains_into(bridge, "test-target".into(), &registry);

        let result = registry.dispatch_command("CSS.getComputedStyleForNode", json!({ "nodeId": 1 }), &NoopSender);
        assert!(result.is_some());
        let response = result.unwrap().unwrap();
        assert!(response.get("computedStyle").is_some(), "CSS should return computedStyle field");
    }

    // 5. Overlay domain returns highlight result structure
    #[test]
    fn overlay_domain_returns_highlight_structure() {
        let (bridge, _receiver) = bridge_channel(TIMEOUT);
        let registry: DomainRegistry<DomainDispatch> = DomainRegistry::new();
        register_all_domains_into(bridge, "test-target".into(), &registry);

        let result = registry.dispatch_command("Overlay.highlightNode", json!({ "nodeId": 1 }), &NoopSender);
        assert!(result.is_some());
        let response = result.unwrap().unwrap();
        assert!(response.get("highlighted").is_some(), "Overlay should return highlighted field");
    }

    // 6. Log domain handles enable/disable/clear
    #[test]
    fn log_domain_handles_enable_disable_clear() {
        let (bridge, _receiver) = bridge_channel(TIMEOUT);
        let registry: DomainRegistry<DomainDispatch> = DomainRegistry::new();
        register_all_domains_into(bridge, "test-target".into(), &registry);

        let result = registry.dispatch_command("Log.enable", json!({}), &NoopSender);
        assert!(result.is_some());
        assert!(result.unwrap().is_ok());

        let result = registry.dispatch_command("Log.clear", json!({}), &NoopSender);
        assert!(result.is_some());
        assert!(result.unwrap().is_ok());

        let result = registry.dispatch_command("Log.disable", json!({}), &NoopSender);
        assert!(result.is_some());
        assert!(result.unwrap().is_ok());
    }

    // 7. Fetch domain handles enable with patterns
    #[test]
    fn fetch_domain_handles_enable_with_patterns() {
        let (bridge, _receiver) = bridge_channel(TIMEOUT);
        let registry: DomainRegistry<DomainDispatch> = DomainRegistry::new();
        register_all_domains_into(bridge, "test-target".into(), &registry);

        let result = registry.dispatch_command(
            "Fetch.enable",
            json!({ "patterns": [{ "urlPattern": "*" }] }),
            &NoopSender,
        );
        assert!(result.is_some());
        let response = result.unwrap().unwrap();
        assert_eq!(response.get("patternCount").unwrap(), 1);
    }

    // 8. Unknown command returns method not found error
    #[test]
    fn unknown_command_returns_method_not_found_error() {
        let (bridge, _receiver) = bridge_channel(TIMEOUT);
        let registry: DomainRegistry<DomainDispatch> = DomainRegistry::new();
        register_all_domains_into(bridge, "test-target".into(), &registry);

        let result = registry.dispatch_command("Page.unknownMethod", json!({}), &NoopSender);
        assert!(result.is_some());
        let err = result.unwrap().unwrap_err();
        assert_eq!(err.code, -32601);
        assert!(err.message.contains("unknownMethod"));
    }

    // 9. Domain not registered returns None
    #[test]
    fn unregistered_domain_returns_none() {
        let registry: DomainRegistry<DomainDispatch> = DomainRegistry::new();
        // Don't register any domains
        let result = registry.dispatch_command("Page.enable", json!({}), &NoopSender);
        assert!(result.is_none());
    }

    // 10. All 11 domains are distinct (no duplicate registration)
    #[test]
    fn all_domains_are_distinct() {
        let (bridge, _receiver) = bridge_channel(TIMEOUT);
        let registry: DomainRegistry<DomainDispatch> = DomainRegistry::new();

        // First registration should succeed
        register_all_domains_into(bridge.clone(), "test-target".into(), &registry);

        // Attempting to register Page again should fail (using DomainDispatch)
        let duplicate_result = registry.register(DomainDispatch::Page(page::PageHandler::new(bridge, "test-target".into())));
        assert!(duplicate_result.is_err());
        assert!(duplicate_result.unwrap_err().contains("already registered"));
    }

    // 11. register_all_domains_with_target registers 12 domains including Target
    #[test]
    fn register_all_domains_with_target_registers_12_domains() {
        let (bridge, _receiver) = bridge_channel(TIMEOUT);
        let registry: DomainRegistry<DomainDispatch> = DomainRegistry::new();
        register_all_domains_with_target(bridge, "test-target-id".into(), &registry);

        let expected_domains = [
            "Page", "Runtime", "DOM", "Network", "Debugger",
            "Input", "Emulation", "CSS", "Overlay", "Log", "Fetch", "Target",
        ];

        for domain in &expected_domains {
            assert!(registry.has_domain(domain), "domain '{}' should be registered", domain);
        }
    }

    // 12. Target domain handles getTargets via registry
    #[test]
    fn target_domain_handles_get_targets_via_registry() {
        let (bridge, _receiver) = bridge_channel(TIMEOUT);
        let registry: DomainRegistry<DomainDispatch> = DomainRegistry::new();
        register_all_domains_with_target(bridge, "my-target-123".into(), &registry);

        let result = registry.dispatch_command("Target.getTargets", json!({}), &NoopSender);
        assert!(result.is_some());
        let response = result.unwrap();
        assert!(response.is_ok());
        let result_val = response.unwrap();
        let infos = result_val["targetInfos"].as_array().unwrap();
        assert_eq!(infos.len(), 1);
        assert_eq!(infos[0]["targetId"], "my-target-123");
    }
}
