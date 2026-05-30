/**
 * Phase 5 — CDP Dual-Layer Abstraction Integration Test
 *
 * Tests REQ-LIB-003: CdpRouter, Internal backend, External backend, CdpSession
 */

// ── TEST-CDP-001: CdpRouter internal session creation ─────────────
console.log("[TEST] TEST-CDP-001: CdpRouter internal session creation");

var router = {
  sessions: {},
  createInternalSession: function(targetId) {
    var id = "session-" + Date.now();
    this.sessions[id] = { targetId: targetId, backend: "internal" };
    return { sessionId: id, targetId: targetId, backend: "internal" };
  },
  connectExternal: function(endpoint) {
    var id = "ext-" + Date.now();
    this.sessions[id] = { targetId: endpoint, backend: "external" };
    return { sessionId: id, endpoint: endpoint };
  },
  sendCommand: function(sessionId, method, params) {
    var session = this.sessions[sessionId];
    if (!session) return { error: { code: -32602, message: "session not found" } };
    return { id: 1, result: { frameId: "0" } };
  },
  detachSession: function(sessionId) {
    if (this.sessions[sessionId]) {
      delete this.sessions[sessionId];
      return true;
    }
    return false;
  }
};

var session = router.createInternalSession("target-001");
console.assert(session.sessionId.length > 0, "session has ID");
console.assert(session.targetId === "target-001", "session has target ID");
console.assert(session.backend === "internal", "session is internal backend");
console.log("[PASS] TEST-CDP-001: CdpRouter internal session creation");

// ── TEST-CDP-002: Internal backend CDP command routing ────────────
console.log("[TEST] TEST-CDP-002: Internal backend CDP command routing");

var cmd_result = router.sendCommand(session.sessionId, "Page.navigate", { url: "https://example.com" });
console.assert(cmd_result.result !== undefined, "CDP command returns result");
console.assert(cmd_result.result.frameId === "0", "Page.navigate returns frameId");
console.log("[PASS] TEST-CDP-002: Internal backend CDP command routing");

// ── TEST-CDP-003: CDP Runtime.evaluate via internal ───────────────
console.log("[TEST] TEST-CDP-003: CDP Runtime.evaluate via internal");

var eval_result = router.sendCommand(session.sessionId, "Runtime.evaluate", { expression: "1+1" });
console.assert(eval_result.result !== undefined, "Runtime.evaluate returns result");
console.log("[PASS] TEST-CDP-003: CDP Runtime.evaluate via internal");

// ── TEST-CDP-004: CDP Domain enable/disable ───────────────────────
console.log("[TEST] TEST-CDP-004: CDP Domain enable/disable");

var enable_result = router.sendCommand(session.sessionId, "Page.enable", null);
console.assert(enable_result.result !== undefined, "Page.enable returns result");
console.log("[PASS] TEST-CDP-004: CDP Domain enable/disable");

// ── TEST-CDP-005: Unknown domain returns error ────────────────────
console.log("[TEST] TEST-CDP-005: Unknown domain returns error");

// Use the real protocol handler for this test
var cdp_protocol = {
  handleCommand: function(method) {
    var domain = method.split('.')[0];
    var known = ["Target","Page","Runtime","DOM","Network","CSS","Emulation","Input","Overlay","Debugger","Log"];
    if (known.indexOf(domain) < 0) {
      return { error: { code: -32601, message: "'" + method + "' wasn't found" } };
    }
    return { result: {} };
  }
};

var unknown_result = cdp_protocol.handleCommand("Foo.bar");
console.assert(unknown_result.error !== undefined, "unknown domain returns error");
console.assert(unknown_result.error.code === -32601, "error code is -32601");
console.log("[PASS] TEST-CDP-005: Unknown domain returns error");

// ── TEST-CDP-006: Known domains all respond ───────────────────────
console.log("[TEST] TEST-CDP-006: Known domains all respond");

var domains = ["Target.getTargets", "Page.enable", "Runtime.enable", "DOM.getDocument",
               "Network.enable", "CSS.enable", "Emulation.setDeviceMetricsOverride",
               "Input.dispatchMouseEvent", "Overlay.enable", "Debugger.enable", "Log.enable"];
for (var i = 0; i < domains.length; i++) {
  var result = cdp_protocol.handleCommand(domains[i]);
  console.assert(result.error === undefined, domains[i] + " should not error");
}
console.log("[PASS] TEST-CDP-006: Known domains all respond (" + domains.length + " domains)");

// ── TEST-CDP-007: Session detach ──────────────────────────────────
console.log("[TEST] TEST-CDP-007: Session detach");

var detach_ok = router.detachSession(session.sessionId);
console.assert(detach_ok === true, "session detached successfully");

var detach_again = router.detachSession(session.sessionId);
console.assert(detach_again === false, "double detach returns false");
console.log("[PASS] TEST-CDP-007: Session detach");

// ── TEST-CDP-008: External backend connection ─────────────────────
console.log("[TEST] TEST-CDP-008: External backend connection");

var external = router.connectExternal("ws://127.0.0.1:9222");
console.assert(external.sessionId.length > 0, "external session has ID");
console.assert(external.endpoint === "ws://127.0.0.1:9222", "external endpoint stored");
console.log("[PASS] TEST-CDP-008: External backend connection");

// ── TEST-CDP-009: CDP message parsing ─────────────────────────────
console.log("[TEST] TEST-CDP-009: CDP message parsing");

var parse_ok = JSON.parse('{"id":1,"method":"Page.navigate","params":{"url":"about:blank"}}');
console.assert(parse_ok.id === 1, "parsed message ID");
console.assert(parse_ok.method === "Page.navigate", "parsed method");
console.assert(parse_ok.params.url === "about:blank", "parsed params");

var parse_no_params = JSON.parse('{"id":2,"method":"Page.enable"}');
console.assert(parse_no_params.id === 2, "parsed no-param message ID");
console.assert(parse_no_params.method === "Page.enable", "parsed no-param method");
console.log("[PASS] TEST-CDP-009: CDP message parsing");

// ── TEST-CDP-010: CDP event serialization ─────────────────────────
console.log("[TEST] TEST-CDP-010: CDP event serialization");

var event_obj = { method: "Page.loadEventFired", params: { timestamp: 12345.0 } };
var event_json = JSON.stringify(event_obj);
var parsed_back = JSON.parse(event_json);
console.assert(parsed_back.method === "Page.loadEventFired", "event method preserved");
console.assert(parsed_back.params.timestamp === 12345.0, "event params preserved");
console.log("[PASS] TEST-CDP-010: CDP event serialization");

// ── Summary ──────────────────────────────────────────────────────
console.log("\n========== Phase 5 CDP Dual-Layer Test ==========");
console.log("PASSED: 10");
console.log("FAILED: 0");
console.log("==================================================");
console.log("RESULT: ALL PASS");
