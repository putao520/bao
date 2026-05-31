// @trace REQ-CDP-003
use serde_json::{json, Value};

use cdp_server::{CdpError, DomainHandler, EventSender};

pub struct DebuggerHandler;

impl DomainHandler for DebuggerHandler {
    fn domain_name(&self) -> &'static str { "Debugger" }

    fn handle_command(
        &self,
        command: &str,
        _params: Value,
        _event_sender: &dyn EventSender,
    ) -> Result<Value, CdpError> {
        match command {
            "Debugger.enable" | "Debugger.disable" => Ok(json!({})),
            "Debugger.setBreakpointByUrl" => Ok(json!({ "breakpointId": "1", "locations": [] })),
            "Debugger.removeBreakpoint" | "Debugger.pause" | "Debugger.resume" => Ok(json!({})),
            "Debugger.stepOver" | "Debugger.stepInto" | "Debugger.stepOut" => Ok(json!({})),
            "Debugger.setSkipAllPauses" | "Debugger.setBreakpointsActive" => Ok(json!({})),
            "Debugger.evaluateOnCallFrame" => Ok(json!({ "result": { "type": "undefined" } })),
            "Debugger.getPossibleBreakpoints" => Ok(json!({ "locations": [] })),
            "Debugger.getScriptSource" => Ok(json!({ "scriptSource": "" })),
            "Debugger.setPauseOnExceptions" => Ok(json!({})),
            _ => Err(CdpError { code: -32601, message: format!("'{}' wasn't found", command) }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct NoopSender;
    impl EventSender for NoopSender {
        fn send_event(&self, _method: &str, _params: Value) {}
    }

    #[test]
    fn domain_name_returns_Debugger() {
        let handler = DebuggerHandler;
        assert_eq!(handler.domain_name(), "Debugger");
    }

    #[test]
    fn enable_returns_ok_empty() {
        let handler = DebuggerHandler;
        let es = NoopSender;
        let result = handler.handle_command("Debugger.enable", json!({}), &es).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn disable_returns_ok_empty() {
        let handler = DebuggerHandler;
        let es = NoopSender;
        let result = handler.handle_command("Debugger.disable", json!({}), &es).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn setBreakpointByUrl_returns_breakpointId_and_locations() {
        let handler = DebuggerHandler;
        let es = NoopSender;
        let result = handler.handle_command("Debugger.setBreakpointByUrl", json!({}), &es).unwrap();
        assert_eq!(result, json!({ "breakpointId": "1", "locations": [] }));
    }

    #[test]
    fn removeBreakpoint_returns_ok_empty() {
        let handler = DebuggerHandler;
        let es = NoopSender;
        let result = handler.handle_command("Debugger.removeBreakpoint", json!({}), &es).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn pause_returns_ok_empty() {
        let handler = DebuggerHandler;
        let es = NoopSender;
        let result = handler.handle_command("Debugger.pause", json!({}), &es).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn resume_returns_ok_empty() {
        let handler = DebuggerHandler;
        let es = NoopSender;
        let result = handler.handle_command("Debugger.resume", json!({}), &es).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn stepOver_returns_ok_empty() {
        let handler = DebuggerHandler;
        let es = NoopSender;
        let result = handler.handle_command("Debugger.stepOver", json!({}), &es).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn stepInto_returns_ok_empty() {
        let handler = DebuggerHandler;
        let es = NoopSender;
        let result = handler.handle_command("Debugger.stepInto", json!({}), &es).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn stepOut_returns_ok_empty() {
        let handler = DebuggerHandler;
        let es = NoopSender;
        let result = handler.handle_command("Debugger.stepOut", json!({}), &es).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn setSkipAllPauses_returns_ok_empty() {
        let handler = DebuggerHandler;
        let es = NoopSender;
        let result = handler.handle_command("Debugger.setSkipAllPauses", json!({}), &es).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn setBreakpointsActive_returns_ok_empty() {
        let handler = DebuggerHandler;
        let es = NoopSender;
        let result = handler.handle_command("Debugger.setBreakpointsActive", json!({}), &es).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn evaluate_on_call_frame_returns_undefined_result() {
        let handler = DebuggerHandler;
        let es = NoopSender;
        let result = handler.handle_command("Debugger.evaluateOnCallFrame", json!({}), &es).unwrap();
        assert_eq!(result, json!({ "result": { "type": "undefined" } }));
    }

    #[test]
    fn get_possible_breakpoints_returns_empty_locations() {
        let handler = DebuggerHandler;
        let es = NoopSender;
        let result = handler.handle_command("Debugger.getPossibleBreakpoints", json!({}), &es).unwrap();
        assert_eq!(result, json!({ "locations": [] }));
    }

    #[test]
    fn get_script_source_returns_empty_string() {
        let handler = DebuggerHandler;
        let es = NoopSender;
        let result = handler.handle_command("Debugger.getScriptSource", json!({}), &es).unwrap();
        assert_eq!(result, json!({ "scriptSource": "" }));
    }

    #[test]
    fn set_pause_on_exceptions_returns_ok_empty() {
        let handler = DebuggerHandler;
        let es = NoopSender;
        let result = handler.handle_command("Debugger.setPauseOnExceptions", json!({}), &es).unwrap();
        assert_eq!(result, json!({}));
    }

    #[test]
    fn unknown_command_returns_error_32601() {
        let handler = DebuggerHandler;
        let es = NoopSender;
        let err = handler.handle_command("Debugger.nonexistent", json!({}), &es).unwrap_err();
        assert_eq!(err.code, -32601);
    }
}
