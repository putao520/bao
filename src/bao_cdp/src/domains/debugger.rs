// @trace REQ-CDP-003
use serde_json::{json, Value};

use cdp_server::{CdpError, DomainHandler, EventSender};
use crate::servo_bridge::BridgeSender;

/// JS script that sets up a SpiderMonkey Debugger to monitor script parsing.
/// SpiderMonkey's `Debugger` API is a built-in debugging reflection API.
const DEBUGGER_SETUP_JS: &str = r#"
(function() {
    if (window.__bao_debugger_active) return;
    window.__bao_debugger_active = true;
    window.__bao_breakpoints = {};
    window.__bao_breakpoint_counter = 0;

    try {
        const dbg = new Debugger();
        window.__bao_dbg = dbg;

        dbg.onNewScript = function(script) {
            const info = JSON.stringify({
                id: script.id || ('script-' + Date.now()),
                url: script.url || '',
                startLine: script.startLine || 0,
                endLine: script.startLine + (script.lineCount || 1) - 1,
            });
            console.log('__BAO_DEBUGGER_SCRIPT__' + info);
        };

        // Collect all existing scripts
        dbg.findScripts().forEach(function(script) {
            const info = JSON.stringify({
                id: script.id || ('script-' + Date.now()),
                url: script.url || '',
                startLine: script.startLine || 0,
                endLine: script.startLine + (script.lineCount || 1) - 1,
            });
            console.log('__BAO_DEBUGGER_SCRIPT__' + info);
        });
    } catch(e) {
        // Debugger API not available (e.g. in restricted context)
    }
})();
"#;

/// Debugger domain handler — script monitoring via SpiderMonkey Debugger API.
///
/// When Debugger.enable is called, injects a JS script that creates a SpiderMonkey
/// `Debugger` object. The `onNewScript` callback reports parsed scripts through the
/// console channel, which the CDP server routes to `Debugger.scriptParsed` events.
///
/// Breakpoint/pause/step are acknowledged but limited by the JS-level approach:
/// true breakpoint pausing requires servo ScriptThread integration (servo is upstream).
/// The handler stores breakpoints and reports script parsing — the minimum viable
/// debugging experience for Playwright/Puppeteer clients.
pub struct DebuggerHandler {
    bridge: BridgeSender,
    breakpoints: std::sync::Mutex<u64>,
}

impl DebuggerHandler {
    pub fn new(bridge: BridgeSender) -> Self {
        DebuggerHandler {
            bridge,
            breakpoints: std::sync::Mutex::new(0),
        }
    }
}

impl DomainHandler for DebuggerHandler {
    fn domain_name(&self) -> &'static str { "Debugger" }

    fn handle_command(
        &self,
        command: &str,
        params: Value,
        _event_sender: &dyn EventSender,
    ) -> Result<Value, CdpError> {
        match command {
            "Debugger.enable" => {
                // Inject SpiderMonkey Debugger setup to monitor script parsing
                let _ = self.bridge.send(crate::servo_bridge::BridgeCommand::EvaluateJs {
                    expression: DEBUGGER_SETUP_JS.to_string(),
                    return_by_value: false,
                });
                Ok(json!({}))
            }
            "Debugger.disable" => {
                // Remove debugger via JS
                let _ = self.bridge.send(crate::servo_bridge::BridgeCommand::EvaluateJs {
                    expression: "if (window.__bao_dbg) { window.__bao_dbg.onNewScript = undefined; window.__bao_dbg = null; }".to_string(),
                    return_by_value: false,
                });
                Ok(json!({}))
            }
            "Debugger.setBreakpointByUrl" => {
                let mut bp_id = self.breakpoints.lock().unwrap();
                *bp_id += 1;
                let id = *bp_id;
                let line_number = params.get("lineNumber").and_then(|v| as_u64_safe(v));
                let url = params.get("url").and_then(|v| v.as_str()).unwrap_or("");
                let url_regex = params.get("urlRegex").and_then(|v| v.as_str());

                // Store breakpoint info in page JS for potential future use
                let bp_js = format!(
                    "(function() {{ if (!window.__bao_breakpoints) return; window.__bao_breakpoints[{}] = {{line: {}, url: {}}}; }})()",
                    id,
                    line_number.unwrap_or(0),
                    serde_json::to_string(url).unwrap_or_else(|_| "\"\"".into())
                );
                let _ = self.bridge.send(crate::servo_bridge::BridgeCommand::EvaluateJs {
                    expression: bp_js,
                    return_by_value: false,
                });

                // If urlRegex provided, also store it
                if let Some(regex) = url_regex {
                    let regex_js = format!(
                        "window.__bao_breakpoints[{}].urlRegex = {}",
                        id,
                        serde_json::to_string(regex).unwrap_or_else(|_| "\"\"".into())
                    );
                    let _ = self.bridge.send(crate::servo_bridge::BridgeCommand::EvaluateJs {
                        expression: regex_js,
                        return_by_value: false,
                    });
                }

                Ok(json!({ "breakpointId": id.to_string(), "locations": [] }))
            }
            "Debugger.removeBreakpoint" => {
                let bp_id = params.get("breakpointId").and_then(|v| v.as_str()).unwrap_or("");
                let js = format!(
                    "delete window.__bao_breakpoints[{}]",
                    serde_json::to_string(bp_id).unwrap_or_else(|_| "0".into())
                );
                let _ = self.bridge.send(crate::servo_bridge::BridgeCommand::EvaluateJs {
                    expression: js,
                    return_by_value: false,
                });
                Ok(json!({}))
            }
            "Debugger.pause" | "Debugger.resume" => Ok(json!({})),
            "Debugger.stepOver" | "Debugger.stepInto" | "Debugger.stepOut" => Ok(json!({})),
            "Debugger.setSkipAllPauses" | "Debugger.setBreakpointsActive" => Ok(json!({})),
            "Debugger.evaluateOnCallFrame" => {
                let expression = params.get("expression").and_then(|v| v.as_str()).unwrap_or("");
                if !expression.is_empty() {
                    let resp = self.bridge.send(crate::servo_bridge::BridgeCommand::EvaluateJs {
                        expression: expression.to_string(),
                        return_by_value: true,
                    });
                    return resp.result.map(|v| json!({
                        "result": { "type": "string", "value": v.as_str().unwrap_or("") }
                    })).map_err(|e| CdpError { code: -32603, message: e });
                }
                Ok(json!({ "result": { "type": "undefined" } }))
            }
            "Debugger.getPossibleBreakpoints" => Ok(json!({ "locations": [] })),
            "Debugger.getScriptSource" => {
                let script_id = params.get("scriptId").and_then(|v| v.as_str()).unwrap_or("");
                // Try to get script source via Debugger API
                let js = format!(
                    "(function() {{ try {{ var s = null; window.__bao_dbg && window.__bao_dbg.findScripts().forEach(function(sc) {{ if (String(sc.id) === {}) s = sc; }}); return s ? s.source.text : ''; }} catch(e) {{ return ''; }} }})()",
                    serde_json::to_string(script_id).unwrap_or_else(|_| "''".into())
                );
                let resp = self.bridge.send(crate::servo_bridge::BridgeCommand::EvaluateJs {
                    expression: js,
                    return_by_value: true,
                });
                let source = resp.result.ok().and_then(|v| v.as_str().map(|s| s.to_string())).unwrap_or_default();
                Ok(json!({ "scriptSource": source }))
            }
            "Debugger.setPauseOnExceptions" => Ok(json!({})),
            _ => Err(CdpError { code: -32601, message: format!("'{}' wasn't found", command) }),
        }
    }
}

/// Helper to extract u64 from JSON value (handles both integer and float representations).
fn as_u64_safe(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_f64().map(|f| f as u64))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::servo_bridge::bridge_channel;
    use std::time::Duration;

    struct NoopSender;
    impl EventSender for NoopSender {
        fn send_event(&self, _method: &str, _params: Value) {}
    }

    #[test]
    fn domain_name_returns_Debugger() {
        let (bridge, _) = bridge_channel(Duration::from_millis(100));
        let handler = DebuggerHandler::new(bridge);
        assert_eq!(handler.domain_name(), "Debugger");
    }

    #[test]
    fn enable_sends_bridge_evaluate_js() {
        let (bridge, rx) = bridge_channel(Duration::from_millis(100));
        let handler = DebuggerHandler::new(bridge);
        handler.handle_command("Debugger.enable", json!({}), &NoopSender).unwrap();
        let mut found = false;
        rx.try_process(|cmd| {
            if let crate::servo_bridge::BridgeCommand::EvaluateJs { expression, .. } = cmd {
                assert!(expression.contains("__bao_debugger_active"));
                found = true;
            }
            crate::servo_bridge::BridgeResponse { result: Ok(json!({})) }
        });
        assert!(found, "Debugger.enable should inject debugger setup via bridge");
    }

    #[test]
    fn enable_does_not_fire_fabricated_script_parsed() {
        struct CollectSender(std::sync::Mutex<Vec<String>>);
        impl EventSender for CollectSender {
            fn send_event(&self, method: &str, _params: Value) {
                self.0.lock().unwrap().push(method.to_string());
            }
        }
        let collector = CollectSender(std::sync::Mutex::new(Vec::new()));
        let (bridge, _) = bridge_channel(Duration::from_millis(100));
        let handler = DebuggerHandler::new(bridge);
        handler.handle_command("Debugger.enable", json!({}), &collector).unwrap();
        let events = collector.0.lock().unwrap();
        assert!(events.is_empty(), "Debugger.enable must NOT emit fabricated scriptParsed");
    }

    #[test]
    fn disable_sends_bridge_cleanup() {
        let (bridge, rx) = bridge_channel(Duration::from_millis(100));
        let handler = DebuggerHandler::new(bridge);
        handler.handle_command("Debugger.disable", json!({}), &NoopSender).unwrap();
        let mut found = false;
        rx.try_process(|cmd| {
            if let crate::servo_bridge::BridgeCommand::EvaluateJs { expression, .. } = cmd {
                assert!(expression.contains("__bao_dbg"));
                found = true;
            }
            crate::servo_bridge::BridgeResponse { result: Ok(json!({})) }
        });
        assert!(found);
    }

    #[test]
    fn setBreakpointByUrl_returns_incrementing_id() {
        let (bridge, _) = bridge_channel(Duration::from_millis(100));
        let handler = DebuggerHandler::new(bridge);
        let r1 = handler.handle_command("Debugger.setBreakpointByUrl", json!({"lineNumber": 10, "url": "test.js"}), &NoopSender).unwrap();
        let r2 = handler.handle_command("Debugger.setBreakpointByUrl", json!({"lineNumber": 20}), &NoopSender).unwrap();
        assert_ne!(r1["breakpointId"], r2["breakpointId"]);
        assert_eq!(r1["locations"], json!([]));
    }

    #[test]
    fn removeBreakpoint_returns_ok_empty() {
        let (bridge, _) = bridge_channel(Duration::from_millis(100));
        let handler = DebuggerHandler::new(bridge);
        let result = handler.handle_command("Debugger.removeBreakpoint", json!({"breakpointId": "1"}), &NoopSender).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn pause_returns_ok_empty() {
        let (bridge, _) = bridge_channel(Duration::from_millis(100));
        let handler = DebuggerHandler::new(bridge);
        let result = handler.handle_command("Debugger.pause", json!({}), &NoopSender).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn resume_returns_ok_empty() {
        let (bridge, _) = bridge_channel(Duration::from_millis(100));
        let handler = DebuggerHandler::new(bridge);
        let result = handler.handle_command("Debugger.resume", json!({}), &NoopSender).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn stepOver_returns_ok_empty() {
        let (bridge, _) = bridge_channel(Duration::from_millis(100));
        let handler = DebuggerHandler::new(bridge);
        let result = handler.handle_command("Debugger.stepOver", json!({}), &NoopSender).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn evaluate_on_call_frame_empty_returns_undefined() {
        let (bridge, _) = bridge_channel(Duration::from_millis(100));
        let handler = DebuggerHandler::new(bridge);
        let result = handler.handle_command("Debugger.evaluateOnCallFrame", json!({}), &NoopSender).unwrap();
        assert_eq!(result, json!({ "result": { "type": "undefined" } }));
    }

    #[test]
    fn get_script_source_returns_structure() {
        let (bridge, _) = bridge_channel(Duration::from_millis(100));
        let handler = DebuggerHandler::new(bridge);
        let result = handler.handle_command("Debugger.getScriptSource", json!({"scriptId": "1"}), &NoopSender).unwrap();
        assert!(result.get("scriptSource").is_some());
    }

    #[test]
    fn unknown_command_returns_error_32601() {
        let (bridge, _) = bridge_channel(Duration::from_millis(100));
        let handler = DebuggerHandler::new(bridge);
        let err = handler.handle_command("Debugger.nonexistent", json!({}), &NoopSender).unwrap_err();
        assert_eq!(err.code, -32601);
    }

    #[test]
    fn debugger_setup_js_contains_key_elements() {
        assert!(DEBUGGER_SETUP_JS.contains("__bao_debugger_active"));
        assert!(DEBUGGER_SETUP_JS.contains("__BAO_DEBUGGER_SCRIPT__"));
        assert!(DEBUGGER_SETUP_JS.contains("onNewScript"));
        assert!(DEBUGGER_SETUP_JS.contains("findScripts"));
    }

    #[test]
    fn debugger_setup_js_creates_debugger_object() {
        assert!(DEBUGGER_SETUP_JS.contains("new Debugger()"));
        assert!(DEBUGGER_SETUP_JS.contains("window.__bao_dbg"));
    }
}
