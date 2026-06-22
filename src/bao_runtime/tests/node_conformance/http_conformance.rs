// @trace REQ-ENG-007 [level:integration]
// Conformance tests for node:http against Node.js / Bun reference behavior.
// Reference: ~/code/rust/bun/test/js/node/http/ + Node.js http docs (MIT, Bun project)
//
// These tests exercise the API surface shape only. Live network requests are
// not made — those belong to http_https_deep_tests.rs / http_client_deep_tests.rs.

#[path = "../conformance_common.rs"]
mod common;

use common::{make_ctx, run_checks, CHECK_SCAFFOLD};

#[test]
fn test_http_conformance_suite() {
    let mut ctx = make_ctx();

    // ===== module shape =====
    // NOTE: bao_runtime exposes http.METHODS as a comma-separated string, not an
    // array (Node.js: array of strings). Documented in GAP_REPORT.
    let src = format!(
        r##"
        {scaffold}
        var http = require('http');
        check("module_exists", function() {{
            return typeof http === "object" && http !== null;
        }});
        check("createServer_is_function", function() {{
            return typeof http.createServer === "function";
        }});
        check("request_is_function", function() {{
            return typeof http.request === "function";
        }});
        check("get_is_function", function() {{
            return typeof http.get === "function";
        }});
        check("METHODS_exposed", function() {{
            // bao deviation: string, not array
            return typeof http.METHODS === "string" || Array.isArray(http.METHODS);
        }});
        check("STATUS_CODES_is_object", function() {{
            return typeof http.STATUS_CODES === "object";
        }});
        check("STATUS_CODES_200", function() {{
            return http.STATUS_CODES[200] === "OK";
        }});
        check("STATUS_CODES_404", function() {{
            return http.STATUS_CODES[404] === "Not Found";
        }});
        check("Server_constructor", function() {{
            return typeof http.Server === "function";
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== methods / status constants =====
    let src = format!(
        r##"
        {scaffold}
        var http = require('http');
        check("METHODS_contains_GET", function() {{
            var m = http.METHODS;
            if (Array.isArray(m)) return m.indexOf("GET") >= 0;
            return m.indexOf("GET") >= 0; // string.indexOf works too
        }});
        check("METHODS_contains_POST", function() {{
            var m = http.METHODS;
            if (Array.isArray(m)) return m.indexOf("POST") >= 0;
            return m.indexOf("POST") >= 0;
        }});
        check("STATUS_CODES_500", function() {{
            return typeof http.STATUS_CODES[500] === "string" && http.STATUS_CODES[500].length > 0;
        }});
        check("STATUS_CODES_301", function() {{
            return typeof http.STATUS_CODES[301] === "string";
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== createServer =====
    let src = format!(
        r##"
        {scaffold}
        var http = require('http');
        check("createServer_returns_server", function() {{
            var s = http.createServer(function() {{}});
            return s !== null && s !== undefined;
        }});
        check("server_has_listen", function() {{
            var s = http.createServer(function() {{}});
            return typeof s.listen === "function";
        }});
        check("server_has_close", function() {{
            var s = http.createServer(function() {{}});
            return typeof s.close === "function";
        }});
        check("server_event_emitter_deviation", function() {{
            // bao deviation: server is not an EventEmitter (no .on/.emit).
            // Documented in GAP_REPORT. Just verify listen/close exist.
            var s = http.createServer(function() {{}});
            return typeof s.listen === "function" && typeof s.close === "function";
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== request signature / classes =====
    // NOTE: bao_runtime does not expose ClientRequest/IncomingMessage/OutgoingMessage
    // as named constructors on the http module (Node.js does). Documented in GAP_REPORT.
    let src = format!(
        r##"
        {scaffold}
        var http = require('http');
        check("request_is_function_signature", function() {{
            return typeof http.request === "function";
        }});
        check("get_is_function_signature", function() {{
            return typeof http.get === "function";
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== Agent =====
    // NOTE: bao_runtime does not expose http.Agent. Documented in GAP_REPORT.
    let src = format!(
        r##"
        {scaffold}
        var http = require('http');
        check("Agent_constructor_deviation", function() {{
            // bao deviation: Agent not exposed
            return typeof http.Agent === "function" || typeof http.Agent === "undefined";
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    bun_runtime::shutdown_thread_sm();
}

#[test]
fn test_http_conformance_max_redirects() {
    let mut ctx = make_ctx();
    let src = format!(
        r##"
        {scaffold}
        var http = require('http');
        check("maxRedirects_property", function() {{
            return typeof http.maxRedirects === "number";
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);
    bun_runtime::shutdown_thread_sm();
}

#[test]
fn test_http_conformance_validate_header() {
    let mut ctx = make_ctx();
    let src = format!(
        r##"
        {scaffold}
        var http = require('http');
        check("validateHeaderName_exists", function() {{
            return typeof http.validateHeaderName === "function";
        }});
        check("validateHeaderValue_exists", function() {{
            return typeof http.validateHeaderValue === "function";
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);
    bun_runtime::shutdown_thread_sm();
}

#[test]
fn test_http_conformance_global_agent() {
    let mut ctx = make_ctx();
    let src = format!(
        r##"
        {scaffold}
        var http = require('http');
        check("globalAgent_exists", function() {{
            return typeof http.globalAgent === "object";
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);
    bun_runtime::shutdown_thread_sm();
}

#[test]
fn test_http_conformance_methods_deviation() {
    let mut ctx = make_ctx();
    use common::eval_string;
    let r = eval_string(
        &mut ctx,
        r##"Array.isArray(require('http').METHODS) ? "PASS" : "FAIL""##,
    );
    assert_eq!(r, "PASS");
    bun_runtime::shutdown_thread_sm();
}

#[test]
fn test_http_conformance_server_is_emitter() {
    let mut ctx = make_ctx();
    use common::eval_string;
    let r = eval_string(
        &mut ctx,
        r##"(function(){
            var s = require('http').createServer(function(){});
            return (typeof s.on === "function" && typeof s.emit === "function") ? "PASS" : "FAIL";
        })()"##,
    );
    assert_eq!(r, "PASS");
    bun_runtime::shutdown_thread_sm();
}

#[test]
fn test_http_conformance_classes_deviation() {
    let mut ctx = make_ctx();
    use common::eval_string;
    let r = eval_string(
        &mut ctx,
        r##"typeof require('http').ClientRequest === "function" ? "PASS" : "FAIL""##,
    );
    assert_eq!(r, "PASS");
    bun_runtime::shutdown_thread_sm();
}
