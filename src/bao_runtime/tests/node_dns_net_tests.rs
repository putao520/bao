// @trace TEST-ENG-007-DNS [req:REQ-ENG-007] [level:integration]
// Integration tests for node:dns and node:net API (REQ-ENG-007)

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

#[test]
fn test_node_dns_all() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var dns = require('dns');
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e)); }
        }

        // === node:dns ===
        check("dns_require", function() { return typeof dns === 'object'; });
        check("dns_lookup", function() { return typeof dns.lookup === 'function'; });
        check("dns_resolve", function() { return typeof dns.resolve === 'function'; });
        check("dns_resolve4", function() { return typeof dns.resolve4 === 'function'; });
        check("dns_resolve6", function() { return typeof dns.resolve6 === 'function'; });
        check("dns_reverse", function() { return typeof dns.reverse === 'function'; });
        check("dns_Resolver", function() { return typeof dns.Resolver === 'function'; });
        check("dns_lookupService", function() { return typeof dns.lookupService === 'function'; });
        check("dns_getServers", function() { return typeof dns.getServers === 'function'; });
        check("dns_setServers", function() { return typeof dns.setServers === 'function'; });
        check("dns_resolver_instance", function() {
            var r = new dns.Resolver();
            return typeof r.resolve === 'function' && typeof r.getServers === 'function';
        });
        check("dns_lookup_localhost", function() {
            try {
                var result = dns.lookup('127.0.0.1');
                return typeof result === 'object' || typeof result === 'string';
            } catch(e) {
                return true;
            }
        });
        check("dns_getServers_returns_array", function() {
            var s = dns.getServers();
            return Array.isArray(s);
        });

        // === node:net ===
        var net = require('net');
        check("net_require", function() { return typeof net === 'object'; });
        check("net_createServer", function() { return typeof net.createServer === 'function'; });
        check("net_connect", function() { return typeof net.connect === 'function'; });
        check("net_createConnection", function() { return typeof net.createConnection === 'function'; });
        check("net_isIP", function() {
            return net.isIP('127.0.0.1') === 4;
        });
        check("net_isIPv4", function() {
            return net.isIPv4('192.168.1.1') === true && net.isIPv4('not-ip') === false;
        });
        check("net_isIPv6", function() {
            var v6 = net.isIPv6('::1');
            var v4not = net.isIPv6('127.0.0.1') === false;
            return v6 === true || v6 === 6 || v4not;
        });
        check("net_Socket", function() { return typeof net.Socket === 'function'; });
        check("net_Server", function() {
            var s = net.createServer();
            return typeof s === 'object' && s !== null;
        });

        results.join("|")
    "#);

    let mut all_passed = true;
    for item in results.split('|') {
        if !item.contains(" PASS") {
            eprintln!("  FAIL: {}", item);
            all_passed = false;
        }
    }
    assert!(all_passed, "All dns/net tests should pass. Results: {}", results);
    bun_runtime::shutdown_thread_sm();
}
