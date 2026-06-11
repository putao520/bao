// @trace REQ-CDP-003
use serde_json::{json, Value};

use cdp_server::{CdpError, DomainHandler, EventSender};
use crate::servo_bridge::BridgeSender;

/// JS script that sets up a SpiderMonkey Debugger to monitor script parsing and handle pause/step.
/// SpiderMonkey's `Debugger` API is a built-in debugging reflection API.
const DEBUGGER_SETUP_JS: &str = r#"
(function() {
    if (window.__bao_debugger_active) return;
    window.__bao_debugger_active = true;
    window.__bao_breakpoints = {};
    window.__bao_breakpoint_counter = 0;
    window.__bao_paused = false;
    window.__bao_skip_all_pauses = false;
    window.__bao_breakpoints_active = true;
    window.__bao_step_mode = null; // null | 'over' | 'into' | 'out'
    window.__bao_pause_depth = 0;

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

        // Handle debugger statements (pause points)
        dbg.onDebuggerStatement = function(frame) {
            if (window.__bao_skip_all_pauses || !window.__bao_breakpoints_active) return;

            window.__bao_paused = true;

            const callFrames = [];
            let f = frame;
            let frameIndex = 0;
            while (f && frameIndex < 100) {
                const script = f.script;
                const offset = f.offset;
                let line = script ? script.startLine : 0;
                if (script && offset && script.source && script.source.text) {
                    const text = script.source.text;
                    let charCount = 0;
                    let lineNum = script.startLine;
                    for (let i = 0; i < text.length && i < offset; i++) {
                        if (text[i] === '\n') lineNum++;
                    }
                    line = lineNum;
                }

                callFrames.push({
                    callFrameId: 'frame-' + frameIndex + '-' + (script ? script.id : 'unknown'),
                    functionName: f.callee ? (f.callee.name || '(anonymous)') : '(anonymous)',
                    location: {
                        scriptId: script ? String(script.id) : '',
                        lineNumber: line,
                        columnNumber: 0
                    },
                    scopeChain: [{ type: 'local', object: { type: 'object', objectId: 'local-' + frameIndex } }]
                });
                f = f.older;
                frameIndex++;
            }

            const pausedInfo = JSON.stringify({
                callFrames: callFrames,
                reason: 'debuggerStatement',
                hitBreakpoints: []
            });
            console.log('__BAO_DEBUGGER_PAUSED__' + pausedInfo);
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

/// Debugger domain handler — script monitoring and pause/step via SpiderMonkey Debugger API.
///
/// When Debugger.enable is called, injects a JS script that creates a SpiderMonkey
/// `Debugger` object with both `onNewScript` and `onDebuggerStatement` handlers.
/// - `onNewScript` reports parsed scripts through the console channel, routed to
///   `Debugger.scriptParsed` events.
/// - `onDebuggerStatement` builds CDP-compatible callFrames and emits
///   `Debugger.paused` events via `__BAO_DEBUGGER_PAUSED__` console markers.
///
/// Pause/resume/step commands inject JS that toggles `__bao_paused` and
/// `__bao_step_mode` flags. `setBreakpointsActive` and `setSkipAllPauses`
/// control the corresponding JS flags that the onDebuggerStatement handler checks.
///
/// True single-step execution requires servo ScriptThread integration (servo is
/// upstream), so step modes are recorded for future use when the JS runtime
/// next hits a debugger statement.
pub struct DebuggerHandler {
    bridge: BridgeSender,
    target_id: String,
    breakpoints: std::sync::Mutex<u64>,
}

impl DebuggerHandler {
    pub fn new(bridge: BridgeSender, target_id: String) -> Self {
        DebuggerHandler {
            bridge,
            target_id,
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
                    target_id: self.target_id.clone(),
                    expression: DEBUGGER_SETUP_JS.to_string(),
                    return_by_value: false,
                });
                Ok(json!({}))
            }
            "Debugger.disable" => {
                // Remove debugger via JS
                let _ = self.bridge.send(crate::servo_bridge::BridgeCommand::EvaluateJs {
                    target_id: self.target_id.clone(),
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
                    target_id: self.target_id.clone(),
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
                        target_id: self.target_id.clone(),
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
                    target_id: self.target_id.clone(),
                    expression: js,
                    return_by_value: false,
                });
                Ok(json!({}))
            }
            "Debugger.pause" => {
                let js = "(function() { window.__bao_paused = true; window.__bao_step_mode = null; })()";
                let _ = self.bridge.send(crate::servo_bridge::BridgeCommand::EvaluateJs {
                    target_id: self.target_id.clone(),
                    expression: js.to_string(),
                    return_by_value: false,
                });
                Ok(json!({}))
            }
            "Debugger.resume" => {
                let js = "(function() { window.__bao_paused = false; window.__bao_step_mode = null; })()";
                let _ = self.bridge.send(crate::servo_bridge::BridgeCommand::EvaluateJs {
                    target_id: self.target_id.clone(),
                    expression: js.to_string(),
                    return_by_value: false,
                });
                Ok(json!({}))
            }
            "Debugger.stepOver" => {
                let js = "(function() { window.__bao_step_mode = 'over'; window.__bao_paused = true; })()";
                let _ = self.bridge.send(crate::servo_bridge::BridgeCommand::EvaluateJs {
                    target_id: self.target_id.clone(),
                    expression: js.to_string(),
                    return_by_value: false,
                });
                Ok(json!({}))
            }
            "Debugger.stepInto" => {
                let js = "(function() { window.__bao_step_mode = 'into'; window.__bao_paused = true; })()";
                let _ = self.bridge.send(crate::servo_bridge::BridgeCommand::EvaluateJs {
                    target_id: self.target_id.clone(),
                    expression: js.to_string(),
                    return_by_value: false,
                });
                Ok(json!({}))
            }
            "Debugger.stepOut" => {
                let js = "(function() { window.__bao_step_mode = 'out'; window.__bao_paused = true; })()";
                let _ = self.bridge.send(crate::servo_bridge::BridgeCommand::EvaluateJs {
                    target_id: self.target_id.clone(),
                    expression: js.to_string(),
                    return_by_value: false,
                });
                Ok(json!({}))
            }
            "Debugger.setBreakpointsActive" => {
                let active = params.get("active").and_then(|v| v.as_bool()).unwrap_or(true);
                let js = format!("window.__bao_breakpoints_active = {}", active);
                let _ = self.bridge.send(crate::servo_bridge::BridgeCommand::EvaluateJs {
                    target_id: self.target_id.clone(),
                    expression: js,
                    return_by_value: false,
                });
                Ok(json!({}))
            }
            "Debugger.setSkipAllPauses" => {
                let skip = params.get("skip").and_then(|v| v.as_bool()).unwrap_or(true);
                let js = format!("window.__bao_skip_all_pauses = {}", skip);
                let _ = self.bridge.send(crate::servo_bridge::BridgeCommand::EvaluateJs {
                    target_id: self.target_id.clone(),
                    expression: js,
                    return_by_value: false,
                });
                Ok(json!({}))
            }
            "Debugger.evaluateOnCallFrame" => {
                let expression = params.get("expression").and_then(|v| v.as_str()).unwrap_or("");
                let call_frame_id = params.get("callFrameId").and_then(|v| v.as_str()).unwrap_or("");
                if !expression.is_empty() {
                    // When paused, attempt evaluation in the call frame context.
                    // SpiderMonkey Debugger.Frame.eval() requires the frame object,
                    // which we cannot access from here. Instead, we evaluate globally
                    // and include the callFrameId in the response for client correlation.
                    let _ = call_frame_id; // acknowledged but not used for dispatch
                    let resp = self.bridge.send(crate::servo_bridge::BridgeCommand::EvaluateJs {
                        target_id: self.target_id.clone(),
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
                    target_id: self.target_id.clone(),
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
#[allow(non_snake_case)]
mod tests {
    use super::*;
    const TID: &str = "test-target";
    use crate::servo_bridge::bridge_channel;
    use std::time::Duration;

    struct NoopSender;
    impl EventSender for NoopSender {
        fn send_event(&self, _method: &str, _params: Value) {}
    }

    #[test]
    fn domain_name_returns_Debugger() {
        let (bridge, _) = bridge_channel(Duration::from_millis(100));
        let handler = DebuggerHandler::new(bridge, TID.into());
        assert_eq!(handler.domain_name(), "Debugger");
    }

    #[test]
    fn enable_sends_bridge_evaluate_js() {
        let (bridge, rx) = bridge_channel(Duration::from_millis(100));
        let handler = DebuggerHandler::new(bridge, TID.into());
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
        let handler = DebuggerHandler::new(bridge, TID.into());
        handler.handle_command("Debugger.enable", json!({}), &collector).unwrap();
        let events = collector.0.lock().unwrap();
        assert!(events.is_empty(), "Debugger.enable must NOT emit fabricated scriptParsed");
    }

    #[test]
    fn disable_sends_bridge_cleanup() {
        let (bridge, rx) = bridge_channel(Duration::from_millis(100));
        let handler = DebuggerHandler::new(bridge, TID.into());
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
        let handler = DebuggerHandler::new(bridge, TID.into());
        let r1 = handler.handle_command("Debugger.setBreakpointByUrl", json!({"lineNumber": 10, "url": "test.js"}), &NoopSender).unwrap();
        let r2 = handler.handle_command("Debugger.setBreakpointByUrl", json!({"lineNumber": 20}), &NoopSender).unwrap();
        assert_ne!(r1["breakpointId"], r2["breakpointId"]);
        assert_eq!(r1["locations"], json!([]));
    }

    #[test]
    fn removeBreakpoint_returns_ok_empty() {
        let (bridge, _) = bridge_channel(Duration::from_millis(100));
        let handler = DebuggerHandler::new(bridge, TID.into());
        let result = handler.handle_command("Debugger.removeBreakpoint", json!({"breakpointId": "1"}), &NoopSender).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn pause_sends_js_that_sets_paused_flag() {
        let (bridge, rx) = bridge_channel(Duration::from_millis(100));
        let handler = DebuggerHandler::new(bridge, TID.into());
        handler.handle_command("Debugger.pause", json!({}), &NoopSender).unwrap();
        let mut found = false;
        rx.try_process(|cmd| {
            if let crate::servo_bridge::BridgeCommand::EvaluateJs { expression, .. } = cmd {
                assert!(expression.contains("__bao_paused = true"));
                assert!(expression.contains("__bao_step_mode = null"));
                found = true;
            }
            crate::servo_bridge::BridgeResponse { result: Ok(json!({})) }
        });
        assert!(found, "Debugger.pause should inject JS setting __bao_paused = true");
    }

    #[test]
    fn resume_sends_js_that_clears_paused_flag() {
        let (bridge, rx) = bridge_channel(Duration::from_millis(100));
        let handler = DebuggerHandler::new(bridge, TID.into());
        handler.handle_command("Debugger.resume", json!({}), &NoopSender).unwrap();
        let mut found = false;
        rx.try_process(|cmd| {
            if let crate::servo_bridge::BridgeCommand::EvaluateJs { expression, .. } = cmd {
                assert!(expression.contains("__bao_paused = false"));
                assert!(expression.contains("__bao_step_mode = null"));
                found = true;
            }
            crate::servo_bridge::BridgeResponse { result: Ok(json!({})) }
        });
        assert!(found, "Debugger.resume should inject JS setting __bao_paused = false");
    }

    #[test]
    fn stepOver_sends_js_that_sets_step_mode_over() {
        let (bridge, rx) = bridge_channel(Duration::from_millis(100));
        let handler = DebuggerHandler::new(bridge, TID.into());
        handler.handle_command("Debugger.stepOver", json!({}), &NoopSender).unwrap();
        let mut found = false;
        rx.try_process(|cmd| {
            if let crate::servo_bridge::BridgeCommand::EvaluateJs { expression, .. } = cmd {
                assert!(expression.contains("__bao_step_mode = 'over'"));
                assert!(expression.contains("__bao_paused = true"));
                found = true;
            }
            crate::servo_bridge::BridgeResponse { result: Ok(json!({})) }
        });
        assert!(found, "Debugger.stepOver should inject JS setting __bao_step_mode = 'over'");
    }

    #[test]
    fn stepInto_sends_js_that_sets_step_mode_into() {
        let (bridge, rx) = bridge_channel(Duration::from_millis(100));
        let handler = DebuggerHandler::new(bridge, TID.into());
        handler.handle_command("Debugger.stepInto", json!({}), &NoopSender).unwrap();
        let mut found = false;
        rx.try_process(|cmd| {
            if let crate::servo_bridge::BridgeCommand::EvaluateJs { expression, .. } = cmd {
                assert!(expression.contains("__bao_step_mode = 'into'"));
                assert!(expression.contains("__bao_paused = true"));
                found = true;
            }
            crate::servo_bridge::BridgeResponse { result: Ok(json!({})) }
        });
        assert!(found, "Debugger.stepInto should inject JS setting __bao_step_mode = 'into'");
    }

    #[test]
    fn stepOut_sends_js_that_sets_step_mode_out() {
        let (bridge, rx) = bridge_channel(Duration::from_millis(100));
        let handler = DebuggerHandler::new(bridge, TID.into());
        handler.handle_command("Debugger.stepOut", json!({}), &NoopSender).unwrap();
        let mut found = false;
        rx.try_process(|cmd| {
            if let crate::servo_bridge::BridgeCommand::EvaluateJs { expression, .. } = cmd {
                assert!(expression.contains("__bao_step_mode = 'out'"));
                assert!(expression.contains("__bao_paused = true"));
                found = true;
            }
            crate::servo_bridge::BridgeResponse { result: Ok(json!({})) }
        });
        assert!(found, "Debugger.stepOut should inject JS setting __bao_step_mode = 'out'");
    }

    #[test]
    fn setBreakpointsActive_sends_js_that_sets_flag() {
        let (bridge, rx) = bridge_channel(Duration::from_millis(100));
        let handler = DebuggerHandler::new(bridge, TID.into());
        handler.handle_command("Debugger.setBreakpointsActive", json!({"active": false}), &NoopSender).unwrap();
        let mut found = false;
        rx.try_process(|cmd| {
            if let crate::servo_bridge::BridgeCommand::EvaluateJs { expression, .. } = cmd {
                assert!(expression.contains("__bao_breakpoints_active = false"));
                found = true;
            }
            crate::servo_bridge::BridgeResponse { result: Ok(json!({})) }
        });
        assert!(found, "Debugger.setBreakpointsActive should inject JS setting __bao_breakpoints_active");
    }

    #[test]
    fn setBreakpointsActive_default_is_true() {
        let (bridge, rx) = bridge_channel(Duration::from_millis(100));
        let handler = DebuggerHandler::new(bridge, TID.into());
        handler.handle_command("Debugger.setBreakpointsActive", json!({}), &NoopSender).unwrap();
        let mut found = false;
        rx.try_process(|cmd| {
            if let crate::servo_bridge::BridgeCommand::EvaluateJs { expression, .. } = cmd {
                assert!(expression.contains("__bao_breakpoints_active = true"));
                found = true;
            }
            crate::servo_bridge::BridgeResponse { result: Ok(json!({})) }
        });
        assert!(found, "Debugger.setBreakpointsActive defaults to true when no params");
    }

    #[test]
    fn setSkipAllPauses_sends_js_that_sets_flag() {
        let (bridge, rx) = bridge_channel(Duration::from_millis(100));
        let handler = DebuggerHandler::new(bridge, TID.into());
        handler.handle_command("Debugger.setSkipAllPauses", json!({"skip": true}), &NoopSender).unwrap();
        let mut found = false;
        rx.try_process(|cmd| {
            if let crate::servo_bridge::BridgeCommand::EvaluateJs { expression, .. } = cmd {
                assert!(expression.contains("__bao_skip_all_pauses = true"));
                found = true;
            }
            crate::servo_bridge::BridgeResponse { result: Ok(json!({})) }
        });
        assert!(found, "Debugger.setSkipAllPauses should inject JS setting __bao_skip_all_pauses");
    }

    #[test]
    fn setSkipAllPauses_default_is_true() {
        let (bridge, rx) = bridge_channel(Duration::from_millis(100));
        let handler = DebuggerHandler::new(bridge, TID.into());
        handler.handle_command("Debugger.setSkipAllPauses", json!({}), &NoopSender).unwrap();
        let mut found = false;
        rx.try_process(|cmd| {
            if let crate::servo_bridge::BridgeCommand::EvaluateJs { expression, .. } = cmd {
                assert!(expression.contains("__bao_skip_all_pauses = true"));
                found = true;
            }
            crate::servo_bridge::BridgeResponse { result: Ok(json!({})) }
        });
        assert!(found, "Debugger.setSkipAllPauses defaults to true when no params");
    }

    #[test]
    fn evaluate_on_call_frame_empty_returns_undefined() {
        let (bridge, _) = bridge_channel(Duration::from_millis(100));
        let handler = DebuggerHandler::new(bridge, TID.into());
        let result = handler.handle_command("Debugger.evaluateOnCallFrame", json!({}), &NoopSender).unwrap();
        assert_eq!(result, json!({ "result": { "type": "undefined" } }));
    }

    #[test]
    fn get_script_source_returns_structure() {
        let (bridge, _) = bridge_channel(Duration::from_millis(100));
        let handler = DebuggerHandler::new(bridge, TID.into());
        let result = handler.handle_command("Debugger.getScriptSource", json!({"scriptId": "1"}), &NoopSender).unwrap();
        assert!(result.get("scriptSource").is_some());
    }

    #[test]
    fn unknown_command_returns_error_32601() {
        let (bridge, _) = bridge_channel(Duration::from_millis(100));
        let handler = DebuggerHandler::new(bridge, TID.into());
        let err = handler.handle_command("Debugger.nonexistent", json!({}), &NoopSender).unwrap_err();
        assert_eq!(err.code, -32601);
    }

    #[test]
    fn debugger_setup_js_contains_key_elements() {
        assert!(DEBUGGER_SETUP_JS.contains("__bao_debugger_active"));
        assert!(DEBUGGER_SETUP_JS.contains("__BAO_DEBUGGER_SCRIPT__"));
        assert!(DEBUGGER_SETUP_JS.contains("onNewScript"));
        assert!(DEBUGGER_SETUP_JS.contains("findScripts"));
        // Pause/step elements
        assert!(DEBUGGER_SETUP_JS.contains("__bao_paused"));
        assert!(DEBUGGER_SETUP_JS.contains("__bao_skip_all_pauses"));
        assert!(DEBUGGER_SETUP_JS.contains("__bao_breakpoints_active"));
        assert!(DEBUGGER_SETUP_JS.contains("__bao_step_mode"));
        assert!(DEBUGGER_SETUP_JS.contains("onDebuggerStatement"));
        assert!(DEBUGGER_SETUP_JS.contains("__BAO_DEBUGGER_PAUSED__"));
    }

    #[test]
    fn debugger_setup_js_creates_debugger_object() {
        assert!(DEBUGGER_SETUP_JS.contains("new Debugger()"));
        assert!(DEBUGGER_SETUP_JS.contains("window.__bao_dbg"));
    }

    #[test]
    fn debugger_setup_js_builds_call_frames_in_paused_handler() {
        assert!(DEBUGGER_SETUP_JS.contains("callFrames"));
        assert!(DEBUGGER_SETUP_JS.contains("callFrameId"));
        assert!(DEBUGGER_SETUP_JS.contains("functionName"));
        assert!(DEBUGGER_SETUP_JS.contains("location"));
        assert!(DEBUGGER_SETUP_JS.contains("scopeChain"));
        assert!(DEBUGGER_SETUP_JS.contains("reason: 'debuggerStatement'"));
    }
}
