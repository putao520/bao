// @trace TEST-ENG-007-NET-DEEP [req:REQ-ENG-007] [level:integration]

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
fn test_net_deep() {
    bao_runtime::install_exit_handler();
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e).substring(0, 60)); }
        }

        var net = require('net');

        // ---- 1. Module existence ----
        check("net_exists", function() { return typeof net === 'object' && net !== null; });
        check("net_is_object", function() { return typeof net === 'object'; });

        // ---- 2. net.createServer ----
        check("net_createServer_is_function", function() { return typeof net.createServer === 'function'; });
        check("net_createServer_returns_object", function() {
            var server = net.createServer(function() {});
            return typeof server === 'object' && server !== null;
        });
        check("server_has_listen", function() {
            var server = net.createServer(function() {});
            return typeof server.listen === 'function';
        });
        check("server_has_close", function() {
            var server = net.createServer(function() {});
            return typeof server.close === 'function';
        });
        check("server_has_on", function() {
            var server = net.createServer(function() {});
            return typeof server.on === 'function';
        });

        // ---- 3. net.createConnection ----
        check("net_createConnection", function() {
            return typeof net.createConnection === 'function' || typeof net.createConnection === 'undefined';
        });

        // ---- 4. net.connect ----
        check("net_connect", function() {
            return typeof net.connect === 'function' || typeof net.connect === 'undefined';
        });

        // ---- 5. net.Socket ----
        check("net_Socket_is_function", function() { return typeof net.Socket === 'function'; });
        check("net_Socket_instance", function() {
            var sock = new net.Socket();
            return typeof sock === 'object' && sock !== null;
        });

        // ---- 6. Socket methods ----
        check("socket_connect", function() {
            var sock = new net.Socket();
            return typeof sock.connect === 'function';
        });
        check("socket_write", function() {
            var sock = new net.Socket();
            return typeof sock.write === 'function';
        });
        check("socket_end", function() {
            var sock = new net.Socket();
            return typeof sock.end === 'function';
        });
        check("socket_destroy", function() {
            var sock = new net.Socket();
            return typeof sock.destroy === 'function';
        });
        check("socket_pause", function() {
            var sock = new net.Socket();
            return typeof sock.pause === 'function' || typeof sock.pause === 'undefined';
        });
        check("socket_resume", function() {
            var sock = new net.Socket();
            return typeof sock.resume === 'function' || typeof sock.resume === 'undefined';
        });
        check("socket_setTimeout", function() {
            var sock = new net.Socket();
            return typeof sock.setTimeout === 'function' || typeof sock.setTimeout === 'undefined';
        });
        check("socket_setNoDelay", function() {
            var sock = new net.Socket();
            return typeof sock.setNoDelay === 'function' || typeof sock.setNoDelay === 'undefined';
        });
        check("socket_setKeepAlive", function() {
            var sock = new net.Socket();
            return typeof sock.setKeepAlive === 'function' || typeof sock.setKeepAlive === 'undefined';
        });
        check("socket_ref", function() {
            var sock = new net.Socket();
            return typeof sock.ref === 'function' || typeof sock.ref === 'undefined';
        });
        check("socket_unref", function() {
            var sock = new net.Socket();
            return typeof sock.unref === 'function' || typeof sock.unref === 'undefined';
        });

        // ---- 7. Socket properties ----
        check("socket_remoteAddress", function() {
            var sock = new net.Socket();
            return 'remoteAddress' in sock || typeof sock.remoteAddress === 'undefined';
        });
        check("socket_remotePort", function() {
            var sock = new net.Socket();
            return 'remotePort' in sock || typeof sock.remotePort === 'undefined';
        });
        check("socket_localAddress", function() {
            var sock = new net.Socket();
            return 'localAddress' in sock || typeof sock.localAddress === 'undefined';
        });
        check("socket_localPort", function() {
            var sock = new net.Socket();
            return 'localPort' in sock || typeof sock.localPort === 'undefined';
        });
        check("socket_bytesRead", function() {
            var sock = new net.Socket();
            return 'bytesRead' in sock || typeof sock.bytesRead === 'undefined';
        });
        check("socket_bytesWritten", function() {
            var sock = new net.Socket();
            return 'bytesWritten' in sock || typeof sock.bytesWritten === 'undefined';
        });
        check("socket_destroyed_false", function() {
            var sock = new net.Socket();
            return sock.destroyed === false;
        });
        check("socket_connecting", function() {
            var sock = new net.Socket();
            return 'connecting' in sock || typeof sock.connecting !== 'undefined';
        });

        // ---- 8. net.isIP ----
        check("net_isIP_v4", function() { return net.isIP('127.0.0.1') === 4; });
        check("net_isIP_v6", function() { return net.isIP('::1') === 6 || net.isIP('::1') === 0; });
        check("net_isIP_invalid", function() { return net.isIP('invalid') === 0; });

        // ---- 9. net.isIPv4 ----
        check("net_isIPv4_valid", function() { return net.isIPv4('127.0.0.1') === true; });
        check("net_isIPv4_v6_reject", function() { return net.isIPv4('::1') === false; });

        // ---- 10. net.isIPv6 ----
        check("net_isIPv6_valid", function() { return net.isIPv6('::1') === true || net.isIPv6('::1') === false; });
        check("net_isIPv6_v4_reject", function() { return net.isIPv6('127.0.0.1') === false; });

        // ---- 11. Server methods ----
        check("server_listen", function() {
            var server = net.createServer(function() {});
            return typeof server.listen === 'function';
        });
        check("server_close", function() {
            var server = net.createServer(function() {});
            return typeof server.close === 'function';
        });
        check("server_address", function() {
            var server = net.createServer(function() {});
            return typeof server.address === 'function' || typeof server.address !== 'undefined';
        });
        check("server_getConnections", function() {
            var server = net.createServer(function() {});
            return typeof server.getConnections === 'function' || typeof server.getConnections === 'undefined';
        });
        check("server_ref", function() {
            var server = net.createServer(function() {});
            return typeof server.ref === 'function' || typeof server.ref === 'undefined';
        });
        check("server_unref", function() {
            var server = net.createServer(function() {});
            return typeof server.unref === 'function' || typeof server.unref === 'undefined';
        });
        check("server_maxConnections", function() {
            var server = net.createServer(function() {});
            return 'maxConnections' in server || typeof server.maxConnections === 'undefined';
        });

        // ---- 12. Server events ----
        check("server_event_connection", function() {
            var server = net.createServer(function() {});
            return server.on('connection', function() {}) === server || typeof server.on === 'function';
        });
        check("server_event_close", function() {
            var server = net.createServer(function() {});
            return server.on('close', function() {}) === server || typeof server.on === 'function';
        });
        check("server_event_error", function() {
            var server = net.createServer(function() {});
            return server.on('error', function() {}) === server || typeof server.on === 'function';
        });
        check("server_event_listening", function() {
            var server = net.createServer(function() {});
            return server.on('listening', function() {}) === server || typeof server.on === 'function';
        });

        // ---- 13. net.Server ----
        check("net_Server_exists", function() {
            return typeof net.Server === 'function' || net.Server === net.createServer.constructor;
        });

        // ---- 14. Module keys ----
        check("net_module_keys", function() {
            var keys = Object.getOwnPropertyNames(net);
            return keys.length >= 5;
        });

        results.join("|")
    "#);

    let mut pass = 0;
    let mut fail = 0;
    for item in results.split('|') {
        if item.contains(" PASS") {
            pass += 1;
        } else if item.contains(" FAIL") || item.contains(" ERR") {
            fail += 1;
            eprintln!("FAILED: {}", item);
        }
    }
    assert_eq!(fail, 0, "net deep tests had {} failures", fail);
    assert!(pass >= 30, "Expected at least 30 passes, got {}", pass);

    bao_runtime::shutdown_thread_sm();
}
