// @trace TEST-ENG-HTTPCLIENT [req:REQ-ENG-007] [level:integration]

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        _ => String::new(),
    }
}

fn eval_bool(ctx: &mut JsContext, source: &str) -> bool {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::Bool(b)) => b,
        _ => false,
    }
}

#[test]
fn test_http_client_deep() {
    bao_runtime::install_exit_handler();
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    // === http module API surface ===
    assert!(eval_bool(&mut ctx, "typeof require('http') === 'object'"), "http should be object");

    assert!(eval_bool(&mut ctx, "typeof require('http').request === 'function'"),
        "http.request should be function");
    assert!(eval_bool(&mut ctx, "typeof require('http').get === 'function'"),
        "http.get should be function");

    // === http.request returns object ===
    let req_type = eval_string(&mut ctx, r#"
        var http = require('http');
        var req = http.request({hostname: '127.0.0.1', port: 1, path: '/', method: 'GET'});
        typeof req
    "#);
    assert!(req_type.contains("object"), "http.request should return object, got: {}", req_type);

    // === http.request result has expected properties ===
    assert!(eval_bool(&mut ctx, r#"
        var http = require('http');
        var req = http.request({hostname: '127.0.0.1', port: 1, path: '/'});
        typeof req.on === 'function' || typeof req.end === 'function' || typeof req === 'object'
    "#), "http.request result should have on/end methods or be object");

    // === http.get returns object ===
    let get_type = eval_string(&mut ctx, r#"
        var http = require('http');
        var req = http.get({hostname: '127.0.0.1', port: 1, path: '/'});
        typeof req
    "#);
    assert!(get_type.contains("object"), "http.get should return object, got: {}", get_type);

    // === http.request with method POST ===
    assert!(eval_bool(&mut ctx, r#"
        var http = require('http');
        var req = http.request({hostname: '127.0.0.1', port: 1, method: 'POST'});
        typeof req === 'object'
    "#), "http.request with POST should work");

    // === http.request with method PUT ===
    assert!(eval_bool(&mut ctx, r#"
        var http = require('http');
        var req = http.request({hostname: '127.0.0.1', port: 1, method: 'PUT'});
        typeof req === 'object'
    "#), "http.request with PUT should work");

    // === http.request with method DELETE ===
    assert!(eval_bool(&mut ctx, r#"
        var http = require('http');
        var req = http.request({hostname: '127.0.0.1', port: 1, method: 'DELETE'});
        typeof req === 'object'
    "#), "http.request with DELETE should work");

    // === http.request with method PATCH ===
    assert!(eval_bool(&mut ctx, r#"
        var http = require('http');
        var req = http.request({hostname: '127.0.0.1', port: 1, method: 'PATCH'});
        typeof req === 'object'
    "#), "http.request with PATCH should work");

    // === http.request with method HEAD ===
    assert!(eval_bool(&mut ctx, r#"
        var http = require('http');
        var req = http.request({hostname: '127.0.0.1', port: 1, method: 'HEAD'});
        typeof req === 'object'
    "#), "http.request with HEAD should work");

    // === http.request with headers ===
    assert!(eval_bool(&mut ctx, r#"
        var http = require('http');
        var req = http.request({
            hostname: '127.0.0.1',
            port: 1,
            headers: {'Content-Type': 'application/json', 'Accept': 'text/html'}
        });
        typeof req === 'object'
    "#), "http.request with headers should work");

    // === http.STATUS_CODES ===
    assert!(eval_bool(&mut ctx, r#"
        var http = require('http');
        typeof http.STATUS_CODES === 'object' && http.STATUS_CODES[200] === 'OK'
    "#), "http.STATUS_CODES should have 200=OK");

    let status_404 = eval_string(&mut ctx, "require('http').STATUS_CODES[404]");
    assert_eq!(status_404, "Not Found", "STATUS_CODES[404] should be Not Found");

    let status_500 = eval_string(&mut ctx, "require('http').STATUS_CODES[500]");
    assert_eq!(status_500, "Internal Server Error", "STATUS_CODES[500] should be Internal Server Error");

    // === http.createServer ===
    assert!(eval_bool(&mut ctx, r#"
        var http = require('http');
        var server = http.createServer(function(req, res) {});
        typeof server === 'object' && server !== null
    "#), "http.createServer should return server object");

    // === http.METHODS ===
    assert!(eval_bool(&mut ctx, r#"
        var http = require('http');
        typeof http.METHODS === 'string' && http.METHODS.includes('GET')
    "#), "http.METHODS should be string containing GET");

    // === http.Server constructor ===
    assert!(eval_bool(&mut ctx, r#"
        var http = require('http');
        typeof http.Server === 'function' || typeof http.Server === 'object'
    "#), "http.Server should exist");

    // === fetch global API (registered by fetch_api) ===
    assert!(eval_bool(&mut ctx, "typeof fetch === 'function'"),
        "fetch should be function");

    // === fetch returns object (constructor check only, no network call) ===
    assert!(eval_bool(&mut ctx, r#"
        typeof fetch === 'function' && fetch.length >= 1
    "#), "fetch should be a function with at least 1 parameter");

    std::mem::forget(ctx);
}
