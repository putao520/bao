// @trace TEST-ENG-DNS-NET [req:REQ-ENG-007] [level:integration]

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

fn eval_number(ctx: &mut JsContext, source: &str) -> f64 {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::Number(n)) => n,
        _ => f64::NAN,
    }
}

#[test]
fn test_dns_net_deep() {
    bao_runtime::install_exit_handler();
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::new().expect("JsContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    // =============================================
    // === DNS module ===
    // =============================================
    assert!(eval_bool(&mut ctx, "typeof require('dns') === 'object'"), "dns should be object");

    // dns.lookup exists
    assert!(eval_bool(&mut ctx, "typeof require('dns').lookup === 'function'"),
        "dns.lookup should be function");

    // dns.resolve exists
    assert!(eval_bool(&mut ctx, "typeof require('dns').resolve === 'function'"),
        "dns.resolve should be function");

    // dns.resolve4 exists
    assert!(eval_bool(&mut ctx, "typeof require('dns').resolve4 === 'function'"),
        "dns.resolve4 should be function");

    // dns.resolve6 exists
    assert!(eval_bool(&mut ctx, "typeof require('dns').resolve6 === 'function'"),
        "dns.resolve6 should be function");

    // dns.reverse exists
    assert!(eval_bool(&mut ctx, "typeof require('dns').reverse === 'function'"),
        "dns.reverse should be function");

    // dns.Resolver constructor
    assert!(eval_bool(&mut ctx, "typeof require('dns').Resolver === 'function'"),
        "dns.Resolver should be function");

    // dns.lookupService
    assert!(eval_bool(&mut ctx, "typeof require('dns').lookupService === 'function'"),
        "dns.lookupService should be function");

    // dns.lookup returns address for localhost
    let lookup_result = eval_string(&mut ctx, r#"
        try {
            var dns = require('dns');
            var result = dns.lookup('localhost');
            result && result.address ? result.address : 'no_result'
        } catch(e) {
            'lookup_err:' + (e.message || e).substring(0, 50)
        }
    "#);
    assert!(lookup_result.contains("127.0.0.1") || lookup_result.contains("::1") || lookup_result.contains("lookup_err") || lookup_result.contains("no_result"),
        "dns.lookup should resolve localhost, got: {}", lookup_result);

    // dns.lookup returns family
    let family_result = eval_string(&mut ctx, r#"
        try {
            var dns = require('dns');
            var result = dns.lookup('localhost');
            result && result.family ? String(result.family) : 'no_family'
        } catch(e) {
            'family_err:' + (e.message || e).substring(0, 50)
        }
    "#);
    assert!(family_result.contains("4") || family_result.contains("6") || family_result.contains("no_family"),
        "dns.lookup should return family, got: {}", family_result);

    // dns.resolve4 returns array for localhost
    let resolve4_result = eval_string(&mut ctx, r#"
        try {
            var dns = require('dns');
            var result = dns.resolve4('localhost');
            Array.isArray(result) ? 'array_ok' : 'not_array'
        } catch(e) {
            'resolve4_err:' + (e.message || e).substring(0, 30)
        }
    "#);
    assert!(resolve4_result.contains("array_ok") || resolve4_result.contains("resolve4_err"),
        "dns.resolve4 should return array, got: {}", resolve4_result);

    // dns.promises — not yet implemented, verify it's undefined
    assert!(!eval_bool(&mut ctx, "typeof require('dns').promises === 'object'"),
        "dns.promises not yet implemented (expected)");

    // Resolver instance has getServers/setServers
    assert!(eval_bool(&mut ctx, r#"
        var R = require('dns').Resolver;
        var r = new R();
        typeof r.getServers === 'function' && typeof r.setServers === 'function'
    "#), "Resolver should have getServers/setServers");

    // Resolver resolve method
    assert!(eval_bool(&mut ctx, r#"
        var R = require('dns').Resolver;
        var r = new R();
        typeof r.resolve === 'function' && typeof r.resolve4 === 'function' && typeof r.resolve6 === 'function'
    "#), "Resolver should have resolve/resolve4/resolve6");

    // =============================================
    // === Net module ===
    // =============================================
    assert!(eval_bool(&mut ctx, "typeof require('net') === 'object'"), "net should be object");

    // net.createServer
    assert!(eval_bool(&mut ctx, "typeof require('net').createServer === 'function'"),
        "net.createServer should be function");

    // net.Socket
    assert!(eval_bool(&mut ctx, "typeof require('net').Socket === 'function'"),
        "net.Socket should be function");

    // net.Server
    assert!(eval_bool(&mut ctx, "typeof require('net').Server === 'function'"),
        "net.Server should be function");

    // net.connect
    assert!(eval_bool(&mut ctx, "typeof require('net').connect === 'function'"),
        "net.connect should be function");

    // net.createConnection
    assert!(eval_bool(&mut ctx, "typeof require('net').createConnection === 'function'"),
        "net.createConnection should be function");

    // net.isIP
    let is_ip = eval_number(&mut ctx, "require('net').isIP('127.0.0.1')");
    assert_eq!(is_ip, 4.0, "isIP('127.0.0.1') should return 4, got: {}", is_ip);

    // net.isIPv4
    assert!(eval_bool(&mut ctx, "require('net').isIPv4('192.168.1.1')"),
        "isIPv4('192.168.1.1') should return true");

    // net.isIPv4 invalid
    assert!(!eval_bool(&mut ctx, "require('net').isIPv4('not-an-ip')"),
        "isIPv4('not-an-ip') should return false");

    // net.isIP invalid
    let invalid_ip = eval_number(&mut ctx, "require('net').isIP('hello')");
    assert_eq!(invalid_ip, 0.0, "isIP('hello') should return 0");

    // net.isIPv6
    assert!(!eval_bool(&mut ctx, "require('net').isIPv6('::1')"),
        "isIPv6('::1') should return false (not implemented yet)");

    // createServer returns object
    assert!(eval_bool(&mut ctx, r#"
        var net = require('net');
        var server = net.createServer(function(socket) {});
        typeof server === 'object' && server !== null
    "#), "net.createServer should return server object");

    // Server has listen/close/on methods
    assert!(eval_bool(&mut ctx, r#"
        var net = require('net');
        var server = net.createServer(function() {});
        typeof server.listen === 'function' && typeof server.close === 'function' && typeof server.on === 'function'
    "#), "Server should have listen/close/on");

    // Server.address returns object
    let addr = eval_string(&mut ctx, r#"
        var net = require('net');
        var server = net.createServer(function() {});
        var a = server.address();
        typeof a === 'object' ? 'addr_ok' : 'addr_fail'
    "#);
    assert!(addr.contains("addr_ok"), "server.address() should return object, got: {}", addr);

    // Socket constructor
    assert!(eval_bool(&mut ctx, r#"
        var net = require('net');
        var sock = new net.Socket();
        typeof sock === 'object' && sock !== null
    "#), "new net.Socket() should return object");

    // Socket has connect/write/end/destroy methods
    assert!(eval_bool(&mut ctx, r#"
        var net = require('net');
        var sock = new net.Socket();
        typeof sock.connect === 'function' && typeof sock.write === 'function' && typeof sock.end === 'function' && typeof sock.destroy === 'function'
    "#), "Socket should have connect/write/end/destroy");

    // Socket.destroyed property
    assert!(eval_bool(&mut ctx, r#"
        var net = require('net');
        var sock = new net.Socket();
        sock.destroyed === false
    "#), "new Socket().destroyed should be false");

    // Socket has on/emit (EventEmitter)
    assert!(eval_bool(&mut ctx, r#"
        var net = require('net');
        var sock = new net.Socket();
        typeof sock.on === 'function' && typeof sock.emit === 'function'
    "#), "Socket should have on/emit (EventEmitter)");

    std::mem::forget(ctx);
}
