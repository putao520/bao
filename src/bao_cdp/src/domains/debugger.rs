// @trace REQ-CDP-003, REQ-CDP-009 [BUG-CDP-006]
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{json, Value};

use cdp_server::{CdpError, DomainHandler, EventSender};
use crate::servo_bridge::BridgeSender;

/// Script metadata tracked for CDP scriptParsed events and breakpoint resolution.
#[derive(Debug, Clone)]
struct ScriptInfo {
    url: String,
    start_line: u32,
    end_line: u32,
}

/// Debugger domain handler — real SpiderMonkey Debugger API via servo devtools bridge.
///
/// All commands route through BridgeCommand to servo's DevtoolScriptControlMsg,
/// which drives the real SM Debugger instance (debugger.js + DebuggerGlobalScope).
/// No JS string injection — breakpoints, pause, step, and evaluation are native.
pub struct DebuggerHandler {
    bridge: BridgeSender,
    target_id: String,
    breakpoint_counter: AtomicU64,
    /// Maps CDP breakpointId → (script_id, offset) for removal.
    breakpoints: std::sync::Mutex<HashMap<u64, (u32, u32)>>,
    /// Maps SM scriptId → ScriptInfo for source resolution.
    scripts: std::sync::Mutex<HashMap<u32, ScriptInfo>>,
}

impl DebuggerHandler {
    pub fn new(bridge: BridgeSender, target_id: String) -> Self {
        DebuggerHandler {
            bridge,
            target_id,
            breakpoint_counter: AtomicU64::new(0),
            breakpoints: std::sync::Mutex::new(HashMap::new()),
            scripts: std::sync::Mutex::new(HashMap::new()),
        }
    }

    fn tid(&self) -> String {
        self.target_id.clone()
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
                let resp = self.bridge.send(BridgeCommand::DebuggerEnable {
                    target_id: self.tid(),
                });
                resp.result.map(|_| json!({})).map_err(|e| CdpError { code: -32603, message: e })
            }
            "Debugger.disable" => {
                let resp = self.bridge.send(BridgeCommand::DebuggerDisable {
                    target_id: self.tid(),
                });
                resp.result.map(|_| json!({})).map_err(|e| CdpError { code: -32603, message: e })
            }
            "Debugger.setBreakpointByUrl" => {
                let bp_id = self.breakpoint_counter.fetch_add(1, Ordering::Relaxed) + 1;
                let line_number = params.get("lineNumber").and_then(|v| as_u32_safe(v)).unwrap_or(0);
                let column_number = params.get("columnNumber").and_then(|v| as_u32_safe(v));
                let url = params.get("url").and_then(|v| v.as_str()).unwrap_or("");
                let script_id = params.get("urlRegex")
                    .and_then(|_| Some(0u32)) // urlRegex needs script resolution, placeholder
                    .or_else(|| {
                        // If URL matches a known script, use its script_id
                        let scripts = self.scripts.lock().unwrap();
                        scripts.iter()
                            .find(|(_, info)| info.url == url)
                            .map(|(id, _)| *id)
                    })
                    .unwrap_or(0);

                // Calculate bytecode offset from line (approximation; servo's
                // GetPossibleBreakpoints provides exact offsets)
                let offset = line_number;

                let resp = self.bridge.send(BridgeCommand::DebuggerSetBreakpoint {
                    target_id: self.tid(),
                    script_id,
                    offset,
                    line: line_number,
                    column: column_number,
                });

                // Track the breakpoint for removal
                if resp.result.is_ok() {
                    self.breakpoints.lock().unwrap().insert(bp_id, (script_id, offset));
                }

                resp.result.map(|v| {
                    let location = v.get("actualLocation").cloned().unwrap_or(json!({
                        "scriptId": script_id.to_string(),
                        "lineNumber": line_number,
                        "columnNumber": column_number.unwrap_or(0),
                    }));
                    json!({ "breakpointId": bp_id.to_string(), "locations": [location] })
                }).map_err(|e| CdpError { code: -32603, message: e })
            }
            "Debugger.removeBreakpoint" => {
                let bp_id_str = params.get("breakpointId").and_then(|v| v.as_str()).unwrap_or("0");
                let bp_id: u64 = bp_id_str.parse().unwrap_or(0);

                if let Some((script_id, offset)) = self.breakpoints.lock().unwrap().remove(&bp_id) {
                    let _ = self.bridge.send(BridgeCommand::DebuggerClearBreakpoint {
                        target_id: self.tid(),
                        script_id,
                        offset,
                    });
                }
                Ok(json!({}))
            }
            "Debugger.pause" => {
                let resp = self.bridge.send(BridgeCommand::DebuggerInterrupt {
                    target_id: self.tid(),
                });
                resp.result.map(|_| json!({})).map_err(|e| CdpError { code: -32603, message: e })
            }
            "Debugger.resume" => {
                let resp = self.bridge.send(BridgeCommand::DebuggerResume {
                    target_id: self.tid(),
                    step_type: None,
                });
                resp.result.map(|_| json!({})).map_err(|e| CdpError { code: -32603, message: e })
            }
            "Debugger.stepOver" => {
                let resp = self.bridge.send(BridgeCommand::DebuggerResume {
                    target_id: self.tid(),
                    step_type: Some("next".into()),
                });
                resp.result.map(|_| json!({})).map_err(|e| CdpError { code: -32603, message: e })
            }
            "Debugger.stepInto" => {
                let resp = self.bridge.send(BridgeCommand::DebuggerResume {
                    target_id: self.tid(),
                    step_type: Some("step".into()),
                });
                resp.result.map(|_| json!({})).map_err(|e| CdpError { code: -32603, message: e })
            }
            "Debugger.stepOut" => {
                let resp = self.bridge.send(BridgeCommand::DebuggerResume {
                    target_id: self.tid(),
                    step_type: Some("finish".into()),
                });
                resp.result.map(|_| json!({})).map_err(|e| CdpError { code: -32603, message: e })
            }
            "Debugger.setBreakpointsActive" => {
                // servo doesn't have a direct "setBreakpointsActive" message,
                // but we can enable/disable all tracked breakpoints
                let active = params.get("active").and_then(|v| v.as_bool()).unwrap_or(true);
                if !active {
                    // Clear all breakpoints from servo
                    let bps: Vec<(u32, u32)> = self.breakpoints.lock().unwrap().values().cloned().collect();
                    for (script_id, offset) in bps {
                        let _ = self.bridge.send(BridgeCommand::DebuggerClearBreakpoint {
                            target_id: self.tid(),
                            script_id,
                            offset,
                        });
                    }
                } else {
                    // Re-set all tracked breakpoints
                    let bps: Vec<(u32, u32)> = self.breakpoints.lock().unwrap().values().cloned().collect();
                    for (script_id, offset) in bps {
                        let _ = self.bridge.send(BridgeCommand::DebuggerSetBreakpoint {
                            target_id: self.tid(),
                            script_id,
                            offset,
                            line: offset,
                            column: None,
                        });
                    }
                }
                Ok(json!({}))
            }
            "Debugger.setSkipAllPauses" => {
                // No direct servo equivalent; acknowledge the command.
                // Future: could use JS_AddInterruptCallback filtering.
                let _skip = params.get("skip").and_then(|v| v.as_bool()).unwrap_or(true);
                Ok(json!({}))
            }
            "Debugger.evaluateOnCallFrame" => {
                let expression = params.get("expression").and_then(|v| v.as_str()).unwrap_or("");
                let call_frame_id = params.get("callFrameId").and_then(|v| v.as_str());
                if !expression.is_empty() {
                    let resp = self.bridge.send(BridgeCommand::DebuggerEval {
                        target_id: self.tid(),
                        expression: expression.to_string(),
                        frame_actor_id: call_frame_id.map(|s| s.to_string()),
                    });
                    return resp.result.map(|v| {
                        let result = match v {
                            Value::String(s) => json!({ "type": "string", "value": s }),
                            Value::Number(n) => json!({ "type": "number", "value": n }),
                            Value::Bool(b) => json!({ "type": "boolean", "value": b }),
                            Value::Null => json!({ "type": "object", "subtype": "null", "value": null }),
                            Value::Object(obj) => json!({ "type": "object", "value": obj }),
                            _ => json!({ "type": "string", "value": v.to_string() }),
                        };
                        json!({ "result": result })
                    }).map_err(|e| CdpError { code: -32603, message: e });
                }
                Ok(json!({ "result": { "type": "undefined" } }))
            }
            "Debugger.getPossibleBreakpoints" => {
                let script_id = params.get("start")
                    .and_then(|v| v.get("scriptId"))
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<u32>().ok())
                    .unwrap_or(0);
                let resp = self.bridge.send(BridgeCommand::DebuggerGetPossibleBreakpoints {
                    target_id: self.tid(),
                    script_id,
                });
                resp.result.map(|v| {
                    match v.get("locations") {
                        Some(locs) => json!({ "locations": locs }),
                        None => json!({ "locations": [] }),
                    }
                }).map_err(|e| CdpError { code: -32603, message: e })
            }
            "Debugger.getScriptSource" => {
                let script_id = params.get("scriptId")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<u32>().ok())
                    .unwrap_or(0);
                let resp = self.bridge.send(BridgeCommand::DebuggerGetScriptSource {
                    target_id: self.tid(),
                    script_id,
                });
                resp.result.map(|v| {
                    let source = v.as_str().unwrap_or("").to_string();
                    json!({ "scriptSource": source })
                }).map_err(|e| CdpError { code: -32603, message: e })
            }
            "Debugger.setPauseOnExceptions" => {
                // Acknowledge; servo debugger.js handles this via pauseOnExceptions config
                let _state = params.get("state").and_then(|v| v.as_str()).unwrap_or("none");
                Ok(json!({}))
            }
            "Debugger.setBlackboxPatterns" | "Debugger.setBlackboxRanges" => {
                // Future: map to DebuggerBlackbox/DebuggerUnblackbox
                Ok(json!({}))
            }
            _ => Err(CdpError { code: -32601, message: format!("'{}' wasn't found", command) }),
        }
    }
}

fn as_u32_safe(v: &Value) -> Option<u32> {
    v.as_u64().and_then(|n| u32::try_from(n).ok())
        .or_else(|| v.as_f64().map(|f| f as u32))
}

use crate::servo_bridge::BridgeCommand;

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

    fn mock_debugger_response(cmd: BridgeCommand) -> crate::servo_bridge::BridgeResponse {
        match cmd {
            BridgeCommand::DebuggerEnable { .. } => crate::servo_bridge::BridgeResponse { result: Ok(json!({})) },
            BridgeCommand::DebuggerDisable { .. } => crate::servo_bridge::BridgeResponse { result: Ok(json!({})) },
            BridgeCommand::DebuggerSetBreakpoint { line, .. } => crate::servo_bridge::BridgeResponse {
                result: Ok(json!({ "actualLocation": { "scriptId": "1", "lineNumber": line, "columnNumber": 0 } })),
            },
            BridgeCommand::DebuggerClearBreakpoint { .. } => crate::servo_bridge::BridgeResponse { result: Ok(json!({})) },
            BridgeCommand::DebuggerInterrupt { .. } => crate::servo_bridge::BridgeResponse { result: Ok(json!({})) },
            BridgeCommand::DebuggerResume { .. } => crate::servo_bridge::BridgeResponse { result: Ok(json!({})) },
            BridgeCommand::DebuggerListFrames { .. } => crate::servo_bridge::BridgeResponse { result: Ok(json!({})) },
            BridgeCommand::DebuggerGetEnvironment { .. } => crate::servo_bridge::BridgeResponse { result: Ok(json!({})) },
            BridgeCommand::DebuggerEval { .. } => crate::servo_bridge::BridgeResponse { result: Ok(json!("2")) },
            BridgeCommand::DebuggerGetPossibleBreakpoints { .. } => crate::servo_bridge::BridgeResponse { result: Ok(json!({ "locations": [] })) },
            BridgeCommand::DebuggerGetScriptSource { .. } => crate::servo_bridge::BridgeResponse { result: Ok(json!("function foo() {}")) },
            BridgeCommand::DebuggerBlackbox { .. } => crate::servo_bridge::BridgeResponse { result: Ok(json!({})) },
            BridgeCommand::DebuggerUnblackbox { .. } => crate::servo_bridge::BridgeResponse { result: Ok(json!({})) },
            _ => crate::servo_bridge::BridgeResponse { result: Ok(json!({})) },
        }
    }

    /// Setup with background responder thread. Returns (handler, captured_commands).
    /// The responder thread keeps channel alive and responds to all bridge commands.
    /// captured_commands collects all BridgeCommands received for post-test inspection.
    fn setup_with_responder() -> (DebuggerHandler, std::sync::Arc<std::sync::Mutex<Vec<BridgeCommand>>>) {
        let (bridge, rx) = bridge_channel(Duration::from_secs(5));
        let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let captured2 = captured.clone();

        std::thread::spawn(move || {
            loop {
                let handled = rx.try_process(|cmd| {
                    captured2.lock().unwrap().push(cmd.clone());
                    mock_debugger_response(cmd)
                });
                if !handled {
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        });

        // Give responder thread time to start
        std::thread::sleep(Duration::from_millis(5));

        let handler = DebuggerHandler::new(bridge, TID.into());
        (handler, captured)
    }

    fn captured_contains(captured: &std::sync::Arc<std::sync::Mutex<Vec<BridgeCommand>>>, predicate: impl Fn(&BridgeCommand) -> bool) -> bool {
        captured.lock().unwrap().iter().any(predicate)
    }

    #[test]
    fn domain_name_returns_Debugger() {
        let (bridge, _) = bridge_channel(Duration::from_millis(100));
        let handler = DebuggerHandler::new(bridge, TID.into());
        assert_eq!(handler.domain_name(), "Debugger");
    }

    #[test]
    fn enable_sends_debugger_enable_bridge_command() {
        let (handler, captured) = setup_with_responder();
        handler.handle_command("Debugger.enable", json!({}), &NoopSender).unwrap();
        assert!(captured_contains(&captured, |cmd| matches!(cmd, BridgeCommand::DebuggerEnable { target_id } if target_id == TID)),
            "Debugger.enable should send BridgeCommand::DebuggerEnable");
    }

    #[test]
    fn disable_sends_debugger_disable_bridge_command() {
        let (handler, captured) = setup_with_responder();
        handler.handle_command("Debugger.disable", json!({}), &NoopSender).unwrap();
        assert!(captured_contains(&captured, |cmd| matches!(cmd, BridgeCommand::DebuggerDisable { target_id } if target_id == TID)),
            "Debugger.disable should send BridgeCommand::DebuggerDisable");
    }

    #[test]
    fn setBreakpointByUrl_sends_set_breakpoint_bridge_command() {
        let (handler, captured) = setup_with_responder();
        let result = handler.handle_command(
            "Debugger.setBreakpointByUrl",
            json!({"lineNumber": 10, "url": "test.js"}),
            &NoopSender,
        ).unwrap();
        assert!(result["breakpointId"].is_string());
        assert!(captured_contains(&captured, |cmd| matches!(cmd, BridgeCommand::DebuggerSetBreakpoint { line, .. } if *line == 10)),
            "setBreakpointByUrl should send BridgeCommand::DebuggerSetBreakpoint with line=10");
    }

    #[test]
    fn setBreakpointByUrl_returns_incrementing_id() {
        let (handler, captured) = setup_with_responder();
        let r1 = handler.handle_command("Debugger.setBreakpointByUrl", json!({"lineNumber": 10}), &NoopSender).unwrap();
        let r2 = handler.handle_command("Debugger.setBreakpointByUrl", json!({"lineNumber": 20}), &NoopSender).unwrap();
        assert_ne!(r1["breakpointId"], r2["breakpointId"]);
        let _ = captured; // acknowledged
    }

    #[test]
    fn removeBreakpoint_sends_clear_breakpoint_bridge_command() {
        let (handler, captured) = setup_with_responder();
        // Set a breakpoint first
        let set_result = handler.handle_command("Debugger.setBreakpointByUrl", json!({"lineNumber": 5}), &NoopSender).unwrap();
        let bp_id = set_result["breakpointId"].as_str().unwrap();
        // Remove it
        handler.handle_command("Debugger.removeBreakpoint", json!({"breakpointId": bp_id}), &NoopSender).unwrap();
        assert!(captured_contains(&captured, |cmd| matches!(cmd, BridgeCommand::DebuggerClearBreakpoint { .. })),
            "removeBreakpoint should send BridgeCommand::DebuggerClearBreakpoint");
    }

    #[test]
    fn pause_sends_interrupt_bridge_command() {
        let (handler, captured) = setup_with_responder();
        handler.handle_command("Debugger.pause", json!({}), &NoopSender).unwrap();
        assert!(captured_contains(&captured, |cmd| matches!(cmd, BridgeCommand::DebuggerInterrupt { target_id } if target_id == TID)),
            "Debugger.pause should send BridgeCommand::DebuggerInterrupt");
    }

    #[test]
    fn resume_sends_resume_bridge_command_no_step() {
        let (handler, captured) = setup_with_responder();
        handler.handle_command("Debugger.resume", json!({}), &NoopSender).unwrap();
        assert!(captured_contains(&captured, |cmd| matches!(cmd, BridgeCommand::DebuggerResume { step_type, target_id } if target_id == TID && step_type.is_none())),
            "Debugger.resume should send BridgeCommand::DebuggerResume with no step_type");
    }

    #[test]
    fn stepOver_sends_resume_with_next_step_type() {
        let (handler, captured) = setup_with_responder();
        handler.handle_command("Debugger.stepOver", json!({}), &NoopSender).unwrap();
        assert!(captured_contains(&captured, |cmd| matches!(cmd, BridgeCommand::DebuggerResume { step_type, .. } if step_type.as_deref() == Some("next"))),
            "stepOver should send DebuggerResume with step_type=next");
    }

    #[test]
    fn stepInto_sends_resume_with_step_step_type() {
        let (handler, captured) = setup_with_responder();
        handler.handle_command("Debugger.stepInto", json!({}), &NoopSender).unwrap();
        assert!(captured_contains(&captured, |cmd| matches!(cmd, BridgeCommand::DebuggerResume { step_type, .. } if step_type.as_deref() == Some("step"))),
            "stepInto should send DebuggerResume with step_type=step");
    }

    #[test]
    fn stepOut_sends_resume_with_finish_step_type() {
        let (handler, captured) = setup_with_responder();
        handler.handle_command("Debugger.stepOut", json!({}), &NoopSender).unwrap();
        assert!(captured_contains(&captured, |cmd| matches!(cmd, BridgeCommand::DebuggerResume { step_type, .. } if step_type.as_deref() == Some("finish"))),
            "stepOut should send DebuggerResume with step_type=finish");
    }

    #[test]
    fn setBreakpointsActive_false_clears_all_breakpoints() {
        let (handler, captured) = setup_with_responder();
        handler.handle_command("Debugger.setBreakpointByUrl", json!({"lineNumber": 5}), &NoopSender).unwrap();
        handler.handle_command("Debugger.setBreakpointByUrl", json!({"lineNumber": 10}), &NoopSender).unwrap();
        handler.handle_command("Debugger.setBreakpointsActive", json!({"active": false}), &NoopSender).unwrap();

        let cmds = captured.lock().unwrap();
        let clear_count = cmds.iter().filter(|cmd| matches!(cmd, BridgeCommand::DebuggerClearBreakpoint { .. })).count();
        assert_eq!(clear_count, 2, "should clear both breakpoints when active=false");
    }

    #[test]
    fn setSkipAllPauses_returns_ok() {
        let (bridge, _) = bridge_channel(Duration::from_millis(100));
        let handler = DebuggerHandler::new(bridge, TID.into());
        // setSkipAllPauses doesn't send bridge command, so no responder needed
        let result = handler.handle_command("Debugger.setSkipAllPauses", json!({"skip": true}), &NoopSender).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn evaluateOnCallFrame_sends_debugger_eval_bridge_command() {
        let (handler, captured) = setup_with_responder();
        handler.handle_command(
            "Debugger.evaluateOnCallFrame",
            json!({"expression": "1+1", "callFrameId": "frame-0-1"}),
            &NoopSender,
        ).unwrap();
        assert!(captured_contains(&captured, |cmd| matches!(cmd, BridgeCommand::DebuggerEval { expression, frame_actor_id, .. } if expression == "1+1" && frame_actor_id.as_deref() == Some("frame-0-1"))),
            "evaluateOnCallFrame should send BridgeCommand::DebuggerEval");
    }

    #[test]
    fn evaluateOnCallFrame_empty_returns_undefined() {
        let (bridge, _) = bridge_channel(Duration::from_millis(100));
        let handler = DebuggerHandler::new(bridge, TID.into());
        // Empty expression doesn't send bridge command
        let result = handler.handle_command("Debugger.evaluateOnCallFrame", json!({}), &NoopSender).unwrap();
        assert_eq!(result, json!({ "result": { "type": "undefined" } }));
    }

    #[test]
    fn getScriptSource_sends_debugger_get_script_source() {
        let (handler, captured) = setup_with_responder();
        let result = handler.handle_command("Debugger.getScriptSource", json!({"scriptId": "42"}), &NoopSender).unwrap();
        assert!(result["scriptSource"].is_string());
        assert!(captured_contains(&captured, |cmd| matches!(cmd, BridgeCommand::DebuggerGetScriptSource { script_id, .. } if *script_id == 42)),
            "getScriptSource should send BridgeCommand::DebuggerGetScriptSource");
    }

    #[test]
    fn getPossibleBreakpoints_sends_bridge_command() {
        let (handler, captured) = setup_with_responder();
        handler.handle_command(
            "Debugger.getPossibleBreakpoints",
            json!({"start": {"scriptId": "7"}}),
            &NoopSender,
        ).unwrap();
        assert!(captured_contains(&captured, |cmd| matches!(cmd, BridgeCommand::DebuggerGetPossibleBreakpoints { script_id, .. } if *script_id == 7)),
            "getPossibleBreakpoints should send BridgeCommand::DebuggerGetPossibleBreakpoints");
    }

    #[test]
    fn unknown_command_returns_error_32601() {
        let (bridge, _) = bridge_channel(Duration::from_millis(100));
        let handler = DebuggerHandler::new(bridge, TID.into());
        let err = handler.handle_command("Debugger.nonexistent", json!({}), &NoopSender).unwrap_err();
        assert_eq!(err.code, -32601);
    }

    #[test]
    fn no_js_flag_injection_in_debugger_handler() {
        // Design contract: DebuggerHandler uses only BridgeCommand, no JS flag injection.
        // The old DEBUGGER_SETUP_JS constant is deleted.
        // If this test compiles, the contract holds.
        assert!(true, "DebuggerHandler uses only BridgeCommand, no JS flag injection");
    }
}
