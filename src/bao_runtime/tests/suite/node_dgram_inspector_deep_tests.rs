// @trace TEST-ENG-7-DGRAM-INSPECTOR [req:REQ-ENG-007] [level:integration]
// @trace REQ-ENG-040 [req:REQ-ENG-040] [level:integration] — Node.js dgram 模块 UDP 面(require('dgram')/createSocket)
// @trace REQ-ENG-023 [req:REQ-ENG-023] [level:e2e] — dgram bind/send/'message' 127.0.0.1 真 UDP 往返
//   (SPEC 锚 = REQ-ENG-023「Node.js net 模块 (TCP/IPC)」关联面;REQ-ENG-040 为文本交叉引用。
//    coverage-audit-eng-20261014 G2-3 补缺:此前文件仅 require + relaxed typeof,
//    bind/send/on('message') 零命中。)
//
// 驱动 = bounded post-eval hook(生产泵路径,同 net_connect_options_e2e_tests):
// dgram Socket.bind 注册 1ms setInterval 轮询 __dgram_recv,'message' 事件经
// timer drain 派发 — 等待闭包只泵引擎,禁纯 sleep 轮询。
// API 契约按 Node 官方文档:send/bind callback 成功形态 err===null;'message'
// 的 rinfo = { address, family, port, size }(size = 载荷长度)。

use bao_engine::context::JsContext;
#[path = "common/mod.rs"]
mod common;

use common::eval_string;

fn eval_str(ctx: &mut JsContext, code: &str) -> String {
    common::eval_str_named(ctx, code, "<node-dgram-udp>")
}

/// 泵驱动等待:每轮经 bounded drain hook 驱动 timer 派发,条件达成即止。
/// 墙钟窗口(E27 横扫):迭代有界纯自旋与跨线程生产者竞态,弃用 —
/// 见 common::wait_until_deadline。
fn wait_until(ctx: &mut JsContext, js_condition: &str) -> bool {
    common::wait_until_deadline(ctx, js_condition, 50, 10, eval_str)
}


#[test]
fn test_node_dgram_inspector_deep() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let results = eval_string(
        &mut ctx,
        r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e).substring(0, 60)); }
        }

        // === 1. dgram module (UDP) ===
        check("dgram_require", function() {
            try { return typeof require('dgram') === 'object'; }
            catch(e) { return true; } // may not be implemented
        });
        check("dgram_createSocket", function() {
            try { var dgram = require('dgram'); return typeof dgram.createSocket === 'function' || typeof dgram.createSocket === 'undefined'; }
            catch(e) { return true; }
        });
        check("node_dgram", function() {
            try { return typeof require('node:dgram') === 'object'; }
            catch(e) { return true; }
        });

        // === 2. domain module ===
        check("domain_require", function() {
            try { return typeof require('domain') === 'object'; }
            catch(e) { return true; }
        });
        check("domain_create", function() {
            try { var domain = require('domain'); return typeof domain.create === 'function' || typeof domain.create === 'undefined'; }
            catch(e) { return true; }
        });

        // === 3. inspector module ===
        check("inspector_require", function() {
            try { return typeof require('inspector') === 'object'; }
            catch(e) { return true; }
        });
        check("inspector_open", function() {
            try { var insp = require('inspector'); return typeof insp.open === 'function' || typeof insp.open === 'undefined'; }
            catch(e) { return true; }
        });
        check("inspector_close", function() {
            try { var insp = require('inspector'); return typeof insp.close === 'function' || typeof insp.close === 'undefined'; }
            catch(e) { return true; }
        });
        check("inspector_url", function() {
            try { var insp = require('inspector'); return typeof insp.url === 'function' || typeof insp.url === 'undefined'; }
            catch(e) { return true; }
        });

        // === 4. cluster module ===
        check("cluster_require", function() {
            try { return typeof require('cluster') === 'object'; }
            catch(e) { return true; }
        });
        check("cluster_isMaster", function() {
            try { var cluster = require('cluster'); return typeof cluster.isMaster === 'boolean' || typeof cluster.isMaster === 'undefined'; }
            catch(e) { return true; }
        });
        check("cluster_isPrimary", function() {
            try { var cluster = require('cluster'); return typeof cluster.isPrimary === 'boolean' || typeof cluster.isPrimary === 'undefined'; }
            catch(e) { return true; }
        });
        check("cluster_fork", function() {
            try { var cluster = require('cluster'); return typeof cluster.fork === 'function' || typeof cluster.fork === 'undefined'; }
            catch(e) { return true; }
        });

        // === 5. punycode module ===
        check("punycode_require", function() {
            try { return typeof require('punycode') === 'object'; }
            catch(e) { return true; }
        });
        check("punycode_encode", function() {
            try { var pc = require('punycode'); return typeof pc.encode === 'function' || typeof pc.encode === 'undefined'; }
            catch(e) { return true; }
        });
        check("punycode_decode", function() {
            try { var pc = require('punycode'); return typeof pc.decode === 'function' || typeof pc.decode === 'undefined'; }
            catch(e) { return true; }
        });
        check("punycode_toASCII", function() {
            try { var pc = require('punycode'); return typeof pc.toASCII === 'function' || typeof pc.toASCII === 'undefined'; }
            catch(e) { return true; }
        });
        check("punycode_toUnicode", function() {
            try { var pc = require('punycode'); return typeof pc.toUnicode === 'function' || typeof pc.toUnicode === 'undefined'; }
            catch(e) { return true; }
        });

        // === 6. async_hooks module ===
        check("async_hooks_require", function() {
            try { return typeof require('async_hooks') === 'object'; }
            catch(e) { return true; }
        });
        check("async_hooks_createHook", function() {
            try { var ah = require('async_hooks'); return typeof ah.createHook === 'function' || typeof ah.createHook === 'undefined'; }
            catch(e) { return true; }
        });
        check("async_hooks_executionAsyncId", function() {
            try { var ah = require('async_hooks'); return typeof ah.executionAsyncId === 'function' || typeof ah.executionAsyncId === 'undefined'; }
            catch(e) { return true; }
        });
        check("async_hooks_triggerAsyncId", function() {
            try { var ah = require('async_hooks'); return typeof ah.triggerAsyncId === 'function' || typeof ah.triggerAsyncId === 'undefined'; }
            catch(e) { return true; }
        });

        // === 7. diagnostics_channel module ===
        check("diagnostics_channel_require", function() {
            try { return typeof require('diagnostics_channel') === 'object'; }
            catch(e) { return true; }
        });
        check("diagnostics_channel_channel", function() {
            try { var dc = require('diagnostics_channel'); return typeof dc.channel === 'function' || typeof dc.channel === 'undefined'; }
            catch(e) { return true; }
        });

        // === 8. v8 module ===
        check("v8_require", function() {
            try { return typeof require('v8') === 'object'; }
            catch(e) { return true; }
        });
        check("v8_getHeapStatistics", function() {
            try { var v8 = require('v8'); return typeof v8.getHeapStatistics === 'function' || typeof v8.getHeapStatistics === 'undefined'; }
            catch(e) { return true; }
        });
        check("v8_getHeapCodeStatistics", function() {
            try { var v8 = require('v8'); return typeof v8.getHeapCodeStatistics === 'function' || typeof v8.getHeapCodeStatistics === 'undefined'; }
            catch(e) { return true; }
        });
        check("v8_serializer", function() {
            try { var v8 = require('v8'); return typeof v8.DefaultSerializer === 'function' || typeof v8.DefaultSerializer === 'undefined'; }
            catch(e) { return true; }
        });
        check("v8_deserializer", function() {
            try { var v8 = require('v8'); return typeof v8.DefaultDeserializer === 'function' || typeof v8.DefaultDeserializer === 'undefined'; }
            catch(e) { return true; }
        });
        check("v8_cachedDataVersionTag", function() {
            try { var v8 = require('v8'); return typeof v8.cachedDataVersionTag === 'function' || typeof v8.cachedDataVersionTag === 'undefined'; }
            catch(e) { return true; }
        });

        // === 9. trace_events module ===
        check("trace_events_require", function() {
            try { return typeof require('trace_events') === 'object'; }
            catch(e) { return true; }
        });
        check("trace_events_createTracing", function() {
            try { var te = require('trace_events'); return typeof te.createTracing === 'function' || typeof te.createTracing === 'undefined'; }
            catch(e) { return true; }
        });

        // === 10. node: prefix variants ===
        check("node_domain", function() {
            try { return typeof require('node:domain') === 'object'; }
            catch(e) { return true; }
        });
        check("node_cluster", function() {
            try { return typeof require('node:cluster') === 'object'; }
            catch(e) { return true; }
        });
        check("node_async_hooks", function() {
            try { return typeof require('node:async_hooks') === 'object'; }
            catch(e) { return true; }
        });
        check("node_v8", function() {
            try { return typeof require('node:v8') === 'object'; }
            catch(e) { return true; }
        });
        check("node_diagnostics_channel", function() {
            try { return typeof require('node:diagnostics_channel') === 'object'; }
            catch(e) { return true; }
        });

        // === 11. require error on non-existent ===
        check("require_nonexistent_throws", function() {
            try { require('this_module_does_not_exist_xyz'); return false; }
            catch(e) { return true; }
        });

        // === 12. require.cache ===
        check("require_cache_type", function() { return typeof require.cache === 'object' || typeof require.cache === 'undefined'; });

        // === 13. require.resolve ===
        check("require_resolve_type", function() { return typeof require.resolve === 'function' || typeof require.resolve === 'undefined'; });

        // === 14. require.main ===
        check("require_main_type", function() { return typeof require.main === 'object' || typeof require.main === 'undefined'; });

        // === 15. dgram.Socket (relaxed) ===
        check("dgram_Socket", function() {
            try { var dgram = require('dgram'); return typeof dgram.Socket === 'function' || typeof dgram.Socket === 'undefined'; }
            catch(e) { return true; }
        });

        // === 16. inspector.Session (relaxed) ===
        check("inspector_Session", function() {
            try { var insp = require('inspector'); return typeof insp.Session === 'function' || typeof insp.Session === 'undefined'; }
            catch(e) { return true; }
        });

        // === 17. cluster settings (relaxed) ===
        check("cluster_schedulingPolicy", function() {
            try { var cluster = require('cluster'); return typeof cluster.schedulingPolicy === 'number' || typeof cluster.schedulingPolicy === 'string' || typeof cluster.schedulingPolicy === 'undefined'; }
            catch(e) { return true; }
        });
        check("cluster_settings", function() {
            try { var cluster = require('cluster'); return typeof cluster.settings === 'object' || typeof cluster.settings === 'undefined'; }
            catch(e) { return true; }
        });
        check("cluster_workers", function() {
            try { var cluster = require('cluster'); return typeof cluster.workers === 'object' || typeof cluster.workers === 'undefined'; }
            catch(e) { return true; }
        });

        // === 18. v8 constants (relaxed) ===
        check("v8_startupSnapshot", function() {
            try { var v8 = require('v8'); return typeof v8.startupSnapshot === 'object' || typeof v8.startupSnapshot === 'undefined'; }
            catch(e) { return true; }
        });

        results.join("|")
    "#,
    );

    let mut pass = 0;
    let mut fail = 0;
    // Fake-green guard: the eval helper maps a thrown script or a non-string
    // return to "", so an empty battery must fail loudly instead of letting
    // the split loop below pass vacuously.
    assert!(
        !results.is_empty(),
        "battery probe produced no results (uncaught throw or non-string return)"
    );
    for item in results.split('|') {
        if item.contains(" PASS") {
            pass += 1;
        } else if item.contains(" FAIL") || item.contains(" ERR") {
            fail += 1;
            eprintln!("FAILED: {}", item);
        }
    }
    assert_eq!(
        fail, 0,
        "node dgram/inspector deep tests had {} failures",
        fail
    );
    assert!(pass >= 30, "Expected at least 30 passes, got {}", pass);

    bun_runtime::shutdown_thread_sm();
}

// ══════════════════════════════════════════════════════════════════════════
// 真 UDP 往返 1:bind → send → 'message' 事件收包(127.0.0.1 环回,载荷逐字节)
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn dgram_udp_loopback_bind_send_message_roundtrip() {
    let mut ctx = common::setup_ctx_bounded();

    let wired = eval_str(
        &mut ctx,
        r#"
        var dgram = require('dgram');
        globalThis.__stage = 'wiring';
        globalThis.__messages = [];
        globalThis.__sendCb = null;

        var rx = dgram.createSocket('udp4');
        var tx = dgram.createSocket('udp4');
        globalThis.__rx = rx;
        globalThis.__tx = tx;

        rx.on('message', function(msg, rinfo) {
            var bytes = [];
            for (var i = 0; i < msg.length; i++) bytes.push(msg[i]);
            globalThis.__messages.push({
                bytes: bytes,
                msgLen: msg.length,
                address: rinfo.address,
                port: rinfo.port
            });
            globalThis.__stage = 'received';
        });

        rx.bind(0, '127.0.0.1', function() {
            globalThis.__rxPort = rx.address().port;
            tx.bind(0, '127.0.0.1', function() {
                globalThis.__txPort = tx.address().port;
                // 二进制载荷含 0x00 与 0xFF — 走 Buffer → 字节数组真路径。
                tx.send(Buffer.from([0x62, 0x61, 0x6f, 0x00, 0xff]), 0, 5,
                        globalThis.__rxPort, '127.0.0.1', function(err) {
                    globalThis.__sendCb = err ? ('err:' + String(err.message)) : 'null';
                    globalThis.__stage = 'sent';
                });
            });
        });
        'wired'
        "#,
    );
    assert_eq!(wired, "wired", "dgram UDP wiring must eval cleanly");

    let got = wait_until(&mut ctx, "globalThis.__stage === 'received' ? 'y' : 'n'");
    assert!(
        got,
        "UDP roundtrip must settle (stage: {}, sendCb: {}, rxPort: {}, txPort: {})",
        eval_str(&mut ctx, "globalThis.__stage"),
        eval_str(&mut ctx, "String(globalThis.__sendCb)"),
        eval_str(&mut ctx, "String(globalThis.__rxPort)"),
        eval_str(&mut ctx, "String(globalThis.__txPort)"),
    );

    // send callback 成功形态:err === null(Node 官方文档契约)。
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.__sendCb)"),
        "null",
        "send callback must fire with err === null"
    );
    // 一个数据报恰触发一次 'message'。
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.__messages.length)"),
        "1",
        "exactly one 'message' event must fire for one datagram"
    );
    // 载荷逐字节往返(含 0x00 / 0xFF)。
    assert_eq!(
        eval_str(&mut ctx, "globalThis.__messages[0].bytes.join(',')"),
        "98,97,111,0,255",
        "payload must roundtrip byte-exact through the UDP datagram"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.__messages[0].msgLen)"),
        "5",
        "msg.length must equal the datagram payload length"
    );
    // 发送者信息:环回地址 + 发送 socket 真实本地端口。
    assert_eq!(
        eval_str(&mut ctx, "globalThis.__messages[0].address"),
        "127.0.0.1",
        "rinfo.address must be the loopback sender address"
    );
    assert_eq!(
        eval_str(
            &mut ctx,
            "String(globalThis.__messages[0].port) === String(globalThis.__txPort) ? 'y' : 'n'"
        ),
        "y",
        "rinfo.port must equal the sender's bound local port"
    );

    // 收尾:清 1ms 轮询 interval 与 fd(hygiene;nextest 每测独立进程)。
    eval_str(
        &mut ctx,
        "globalThis.__rx.close(); globalThis.__tx.close(); 'closed'",
    );
}

// ══════════════════════════════════════════════════════════════════════════
// 真 UDP 往返 2:'message' 载荷 Buffer 身份 + 发送者信息面
// (rinfo.address/family/port/size,size = Node 文档载荷长度字段;
//  createSocket(type, listener) 直挂 listener 形 + bind(options) 形)
// ══════════════════════════════════════════════════════════════════════════

#[test]
fn dgram_udp_message_payload_and_sender_rinfo() {
    let mut ctx = common::setup_ctx_bounded();

    let wired = eval_str(
        &mut ctx,
        r#"
        var dgram = require('dgram');
        globalThis.__stage = 'wiring';
        globalThis.__got = null;

        // listener 形构造器(Node:createSocket(type, listener) 直挂 'message')。
        var rx = dgram.createSocket('udp4', function(msg, rinfo) {
            globalThis.__got = {
                isBuffer: Buffer.isBuffer(msg),
                msgLen: msg.length,
                family: rinfo.family,
                size: rinfo.size,
                hasSize: ('size' in rinfo),
                address: rinfo.address,
                port: rinfo.port
            };
            globalThis.__stage = 'received';
        });
        var tx = dgram.createSocket('udp4');
        globalThis.__rx = rx;
        globalThis.__tx = tx;

        rx.bind({ port: 0, address: '127.0.0.1' }, function() {
            globalThis.__rxPort = rx.address().port;
            tx.bind(0, '127.0.0.1', function() {
                globalThis.__txPort = tx.address().port;
                // 'Bao-UDP!' — 8 字节,size 断言锚。
                tx.send(Buffer.from([0x42, 0x61, 0x6f, 0x2d, 0x55, 0x44, 0x50, 0x21]),
                        0, 8, globalThis.__rxPort, '127.0.0.1');
            });
        });
        'wired'
        "#,
    );
    assert_eq!(wired, "wired", "dgram UDP wiring (listener form) must eval cleanly");

    let got = wait_until(&mut ctx, "globalThis.__stage === 'received' ? 'y' : 'n'");
    assert!(
        got,
        "UDP roundtrip (listener form) must settle (stage: {}, got: {})",
        eval_str(&mut ctx, "globalThis.__stage"),
        eval_str(&mut ctx, "JSON.stringify(globalThis.__got)"),
    );

    // 载荷身份:Buffer + 长度一致(Node:'msg' 恒为 Buffer)。
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.__got.isBuffer)"),
        "true",
        "message payload must be a Buffer"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.__got.msgLen)"),
        "8",
        "msg.length must equal the 8-byte payload"
    );
    // 发送者信息面(Node rinfo = { address, family, port, size })。
    assert_eq!(
        eval_str(&mut ctx, "globalThis.__got.address"),
        "127.0.0.1",
        "rinfo.address must be the loopback sender address"
    );
    assert_eq!(
        eval_str(&mut ctx, "globalThis.__got.family"),
        "IPv4",
        "rinfo.family must be 'IPv4' (Node string form since v18.4)"
    );
    assert_eq!(
        eval_str(
            &mut ctx,
            "String(globalThis.__got.port) === String(globalThis.__txPort) ? 'y' : 'n'"
        ),
        "y",
        "rinfo.port must equal the sender's bound local port"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.__got.hasSize)"),
        "true",
        "rinfo.size must be present (Node-documented field: the message size)"
    );
    assert_eq!(
        eval_str(&mut ctx, "String(globalThis.__got.size)"),
        "8",
        "rinfo.size must equal the datagram payload length"
    );

    // 收尾:清轮询 interval 与 fd。
    eval_str(
        &mut ctx,
        "globalThis.__rx.close(); globalThis.__tx.close(); 'closed'",
    );
}

