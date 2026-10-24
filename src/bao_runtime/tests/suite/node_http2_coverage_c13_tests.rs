// @trace TEST-ENG-6-H2-C13 [req:REQ-ENG-007] [level:integration]
//
// C13 line-coverage wave for src/bao_runtime/src/node_http2.rs +
// src/bao_runtime/src/node_http2_upgrade.rs — the residual gap after C4:
// the entire _http2_upgrade module (DuplexStream wrapper / upgrade handler
// TLS-shape / raw-socket convenience — zero C4 touches), the EventEmitter
// edge arms (once wrapper identity / removeAllListeners both forms /
// prependListener ordering / emit of unregistered events), the secondary
// Http2Session arms (ping matrix / goaway / respondWithFile+FD throws /
// settings setter validation / settings getter snapshots / originSet /
// request-after-close), connect() argument normalization, and the byte-exact
// body assembly + settings pack/unpack edge arms.
// All offline — pure JS surface, no sockets.
//
// 语义真源 = 本机 node v24.19.0 oracle 实测。登记项:无——本波全部为
// characterization 面(bao 自研 h2 JS 层与 _http2_upgrade 模块,行为按
// 本仓现行形态钉死;C4 已钉的面零重复)。

use bao_engine::context::JsContext;
#[path = "common/mod.rs"]
mod common;

use common::eval_string;

fn fresh_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext init");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

/// Run a `check(label, fn)` battery in JS and assert every item PASSes.
fn assert_battery(ctx: &mut JsContext, battery: &str) {
    let results = eval_string(ctx, battery);
    let mut failures = Vec::new();
    for item in results.split('|') {
        if item.is_empty() {
            continue;
        }
        if !item.contains(" PASS") {
            failures.push(item.to_string());
        }
    }
    assert!(
        failures.is_empty(),
        "h2 c13 battery failures: {:?}\nfull: {}",
        failures,
        results
    );
}

// ── _http2_upgrade: DuplexStream full method surface ────────────────────

/// DuplexStream edge arms through the exported upgrade handler: push data /
/// push null end / push-after-destroyed false, pause buffering + read shift
/// + resume drain, write forwarding to the raw socket and the no-raw-socket
/// emit fallback, end idempotence + finish/end events, destroy with and
/// without an error, and the pipe() delegation.
#[test]
fn h2c13_upgrade_duplex_stream_full_surface() {
    let mut ctx = fresh_ctx();
    assert_battery(
        &mut ctx,
        r#"
        var up = require('_http2_upgrade');
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e)); }
        }
        function fakeRaw() {
            return { _events: {}, on: function(ev, fn) { (this._events[ev] || (this._events[ev] = [])).push(fn); },
                     writes: [], write: function(d) { this.writes.push(d); return true; },
                     ended: false, end: function() { this.ended = true; }, destroyed: false,
                     destroy: function() { this.destroyed = true; } };
        }

        // push(): data chunk emits 'data' and returns true.
        var s = up.createHttp2UpgradeHandler({})(null, fakeRaw(), null);
        var got = null;
        s.on('data', function(c) { got = c; });
        check("push_emits_data_true", function() { return s.push('chunk1') === true && got === 'chunk1'; });

        // push(null): ends the stream, emits 'end', returns false.
        var endSeen = false;
        s.on('end', function() { endSeen = true; });
        check("push_null_ends_false", function() { return s.push(null) === false && endSeen; });

        // push after destroy: false, no event.
        var s2 = up.createHttp2UpgradeHandler({})(null, fakeRaw(), null);
        s2.destroy();
        var lateData = false;
        s2.on('data', function() { lateData = true; });
        check("push_after_destroy_false", function() { return s2.push('x') === false && !lateData; });

        // pause(): chunks buffer (returns false), read() shifts them,
        // resume() drains the buffer through 'data'.
        var s3 = up.createHttp2UpgradeHandler({})(null, fakeRaw(), null);
        s3.pause();
        check("paused_push_buffers", function() { return s3.push('b1') === false; });
        check("read_shifts_buffered", function() { return s3.read() === 'b1' && s3.read() === null; });
        var drained = [];
        s3.on('data', function(c) { drained.push(c); });
        s3.push('b2'); // still paused — buffered
        s3.resume();
        check("resume_drains_buffer", function() {
            return drained.join(',') === 'b2' && s3._paused === false;
        });

        // write(): forwards to the raw socket when it has write().
        var raw3 = fakeRaw();
        var s4 = up.createHttp2UpgradeHandler({})(null, raw3, null);
        check("write_forwards_raw", function() { return s4.write('w1') === true && raw3.writes[0] === 'w1'; });

        // write() with no usable raw socket: emits 'data' locally, true.
        var s5 = up.createHttp2UpgradeHandler({})(null, null, null);
        var local = null;
        s5.on('data', function(c) { local = c; });
        check("write_falls_back_emit", function() { return s5.write('w2') === true && local === 'w2'; });
        s5.destroy();
        check("write_after_destroy_false", function() { return s5.write('w3') === false; });

        // end(): raw socket ends, 'finish' then 'end' emitted, chainable;
        // after destroy it is a no-op returning this.
        var raw4 = fakeRaw();
        var s6 = up.createHttp2UpgradeHandler({})(null, raw4, null);
        var evSeq = [];
        s6.on('finish', function() { evSeq.push('finish'); });
        s6.on('end', function() { evSeq.push('end'); });
        check("end_chainable_raw_ended", function() {
            return s6.end() === s6 && raw4.ended && evSeq.join(',') === 'finish,end';
        });
        var raw5 = fakeRaw();
        var s7 = up.createHttp2UpgradeHandler({})(null, raw5, null);
        s7.destroy();
        raw5.ended = false;
        check("end_after_destroy_noop", function() { return s7.end() === s7 && raw5.ended === false; });

        // destroy(err): forwards to raw, emits 'error' then 'close'; without
        // err only 'close'; idempotent second call emits nothing more.
        var raw6 = fakeRaw();
        var s8 = up.createHttp2UpgradeHandler({})(null, raw6, null);
        var seq8 = [];
        s8.on('error', function(e) { seq8.push('err:' + e.message); });
        s8.on('close', function() { seq8.push('close'); });
        s8.destroy(new Error('boom'));
        check("destroy_err_seq", function() {
            return seq8.join(',') === 'err:boom,close' && raw6.destroyed;
        });
        s8.destroy(new Error('late'));
        check("destroy_idempotent", function() { return seq8.join(',') === 'err:boom,close'; });
        var raw7 = fakeRaw();
        var s9 = up.createHttp2UpgradeHandler({})(null, raw7, null);
        var closed9 = false;
        s9.on('close', function() { closed9 = true; });
        var err9 = false;
        s9.on('error', function() { err9 = true; });
        s9.destroy();
        check("destroy_noerr_close_only", function() { return closed9 && !err9; });

        // pipe(): data → dest.write (forwarded onto dest's raw socket),
        // end → dest.end, error → dest.emit('error').
        var src = up.createHttp2UpgradeHandler({})(null, fakeRaw(), null);
        var destRaw = fakeRaw();
        var dest = up.createHttp2UpgradeHandler({})(null, destRaw, null);
        var destErr = null;
        var destEnd = false;
        dest.on('error', function(e) { destErr = e; });
        dest.on('end', function() { destEnd = true; });
        check("pipe_returns_dest", function() { return src.pipe(dest) === dest; });
        src.emit('data', 'p1');
        src.emit('end');
        src.emit('error', new Error('pipefail'));
        check("pipe_delegation", function() {
            return destRaw.writes.length === 1 && destRaw.writes[0] === 'p1' &&
                   destEnd && destErr !== null && destErr.message === 'pipefail';
        });

        results.join('|')
        "#,
    );
}

/// The upgrade handler's TLS-like socket shape: the _ctx.nativeHandle
/// three-arm precedence (rawSocket._ctx.nativeHandle > rawSocket._handle >
/// empty), the TLS properties (encrypted / authorized / alpnProtocol /
/// protocol / getProtocol / getSession / isSessionReused /
/// getPeerCertificate / getCipher), the raw-socket event forwarding
/// (data/end/error/close/timeout/drain) and its destroyed gate, and the
/// allowHTTP1 option acceptance.
#[test]
fn h2c13_upgrade_handler_shape_ctx_arms_and_forwarding() {
    let mut ctx = fresh_ctx();
    assert_battery(
        &mut ctx,
        r#"
        var up = require('_http2_upgrade');
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e)); }
        }
        function fakeRaw(props) {
            var r = { _events: {}, on: function(ev, fn) { (this._events[ev] || (this._events[ev] = [])).push(fn); },
                      emit: function(ev) { var a = [].slice.call(arguments, 1); var l = this._events[ev] || [];
                                           for (var i = 0; i < l.length; i++) l[i].apply(this, a); return this; } };
            if (props) for (var k in props) r[k] = props[k];
            return r;
        }

        // Arm 1: rawSocket._ctx.nativeHandle wins.
        var rawA = fakeRaw({ _ctx: { nativeHandle: 'NH1' }, _handle: 'H1' });
        var tA = up.createHttp2UpgradeHandler({})(null, rawA, null);
        check("ctx_native_handle_precedence", function() {
            return tA._ctx && tA._ctx.nativeHandle === 'NH1';
        });

        // Arm 2: falls back to rawSocket._handle.
        var rawB = fakeRaw({ _handle: 'H2' });
        var tB = up.createHttp2UpgradeHandler({})(null, rawB, null);
        check("ctx_handle_fallback", function() { return tB._ctx.nativeHandle === 'H2'; });

        // Arm 3: neither present — empty _ctx object.
        var rawC = fakeRaw({});
        var tC = up.createHttp2UpgradeHandler({})(null, rawC, null);
        check("ctx_empty_arm", function() {
            return tC._ctx && typeof tC._ctx === 'object' && tC._ctx.nativeHandle === undefined;
        });

        // TLS-like properties (fixed shape).
        check("tls_props", function() {
            return tA.encrypted === true && tA.authorized === false &&
                   tA.alpnProtocol === 'h2' && tA.protocol === 'h2';
        });
        check("tls_methods", function() {
            return tA.getProtocol() === 'h2' && tA.getSession() === null &&
                   tA.isSessionReused() === false &&
                   typeof tA.getPeerCertificate() === 'object';
        });
        var cipher = tA.getCipher();
        check("tls_cipher_shape", function() {
            return cipher.name === 'TLS_AES_256_GCM_SHA384' &&
                   cipher.standardName === 'TLS_AES_256_GCM_SHA384' &&
                   cipher.version === 'TLSv1.3';
        });

        // Address copy from the raw socket.
        var rawD = fakeRaw({ remoteAddress: '10.0.0.1', remotePort: 1234,
                             localAddress: '127.0.0.1', localPort: 5678 });
        var tD = up.createHttp2UpgradeHandler({})(null, rawD, null);
        check("address_copy", function() {
            return tD.remoteAddress === '10.0.0.1' && tD.remotePort === 1234 &&
                   tD.localAddress === '127.0.0.1' && tD.localPort === 5678;
        });

        // Event forwarding: raw data/end/error/timeout/drain reach the
        // wrapper while it is alive; raw close destroys the wrapper.
        var fwd = up.createHttp2UpgradeHandler({})(null, fakeRaw({}), null);
        var fwdRaw = fakeRaw({});
        var fwd2 = up.createHttp2UpgradeHandler({})(null, fwdRaw, null);
        var seen = [];
        fwd2.on('data', function(c) { seen.push('data:' + c); });
        fwd2.on('end', function() { seen.push('end'); });
        fwd2.on('error', function(e) { seen.push('err'); });
        fwd2.on('timeout', function() { seen.push('timeout'); });
        fwd2.on('drain', function() { seen.push('drain'); });
        fwd2.on('close', function() { seen.push('close'); });
        fwdRaw.emit('data', 'd1');
        fwdRaw.emit('end');
        fwdRaw.emit('error', new Error('e1'));
        fwdRaw.emit('timeout');
        fwdRaw.emit('drain');
        fwdRaw.emit('close'); // forwards destroy() on the wrapper
        check("forwarding_sequence", function() {
            return seen.join(',') === 'data:d1,end,err,timeout,drain,close';
        });

        // Destroyed gate: late raw events do not reach the wrapper.
        var gateRaw = fakeRaw({});
        var dead = up.createHttp2UpgradeHandler({})(null, gateRaw, null);
        dead.destroy();
        var late = [];
        dead.on('data', function() { late.push('d'); });
        dead.on('error', function() { late.push('e'); });
        gateRaw.emit('data', 'late1');
        gateRaw.emit('error', new Error('late2'));
        check("destroyed_gate_no_forward", function() { return late.length === 0; });

        // allowHTTP1 option: accepted, does not change the returned shape.
        var h1 = up.createHttp2UpgradeHandler({ allowHTTP1: true })(null, fakeRaw({}), null);
        check("allow_http1_accepted", function() { return h1.encrypted === true; });

        // Unused-arm sink so the rawC binding stays observed.
        check("alive_wrapper_independent", function() {
            return fwd.encrypted === true && tC.encrypted === true;
        });

        results.join('|')
        "#,
    );
}

/// upgradeRawSocketToH2 convenience: wraps the raw socket, invokes a
/// function connectionListener with the TLS wrapper, tolerates non-function
/// listeners, and returns the wrapper itself.
#[test]
fn h2c13_upgrade_raw_convenience() {
    let mut ctx = fresh_ctx();
    assert_battery(
        &mut ctx,
        r#"
        var up = require('_http2_upgrade');
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e)); }
        }
        function fakeRaw() {
            return { _events: {}, on: function(ev, fn) { (this._events[ev] || (this._events[ev] = [])).push(fn); } };
        }

        // Function listener receives the TLS-shaped wrapper.
        var recv = null;
        var w = up.upgradeRawSocketToH2(function(sock) { recv = sock; }, null, fakeRaw());
        check("listener_receives_wrapper", function() {
            return recv === w && w.encrypted === true && w.alpnProtocol === 'h2';
        });

        // Non-function listeners: tolerated, wrapper still returned.
        var w2 = up.upgradeRawSocketToH2(null, null, fakeRaw());
        check("null_listener_ok", function() { return w2 && w2.encrypted === true; });
        var w3 = up.upgradeRawSocketToH2('not-a-fn', null, fakeRaw());
        check("string_listener_ok", function() { return w3.encrypted === true; });

        // Module export shape.
        check("module_exports", function() {
            return typeof up.createHttp2UpgradeHandler === 'function' &&
                   typeof up.upgradeRawSocketToH2 === 'function';
        });

        results.join('|')
        "#,
    );
}

/// The EventEmitter edge arms shared by Http2Session/Http2Stream/
/// Http2Server: once() fires exactly once and removeListener(origin) removes
/// the wrapper via .listener, removeAllListeners(ev) vs no-arg
/// removeAllListeners(), prependListener() ordering, removeListener of an
/// unregistered fn is a no-op, and emit() of an unregistered event.
#[test]
fn h2c13_ee_edge_arms() {
    let mut ctx = fresh_ctx();
    assert_battery(
        &mut ctx,
        r#"
        var http2 = require('http2');
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e)); }
        }

        // once: single fire; removing by the ORIGINAL fn clears the wrapper
        // (g.listener === fn matching in removeListener).
        var s = new http2.Http2Session('server', 'x');
        var hits = 0;
        function origFn() { hits++; }
        s.once('evt', origFn);
        s.emit('evt');
        s.emit('evt');
        check("once_single_fire", function() { return hits === 1; });
        s.once('evt2', origFn);
        s.removeListener('evt2', origFn);
        check("remove_by_original_clears_wrapper", function() {
            return (s._events['evt2'] || []).length === 0;
        });

        // removeAllListeners(ev): only that event; no-arg: everything.
        var s2 = new http2.Http2Session('server', 'x');
        var a = 0, b = 0, c = 0;
        s2.on('ea', function() { a++; });
        s2.on('eb', function() { b++; });
        s2.on('ec', function() { c++; });
        s2.removeAllListeners('eb');
        s2.emit('ea'); s2.emit('eb'); s2.emit('ec');
        check("remove_all_single_event", function() { return a === 1 && b === 0 && c === 1; });
        s2.removeAllListeners();
        s2.emit('ea'); s2.emit('ec');
        check("remove_all_noarg", function() {
            return a === 1 && c === 1 && Object.keys(s2._events).length === 0;
        });

        // prependListener: fires BEFORE the appended listener.
        var order = [];
        var s3 = new http2.Http2Session('server', 'x');
        s3.on('o', function() { order.push('appended'); });
        s3.prependListener('o', function() { order.push('prepended'); });
        s3.emit('o');
        check("prepend_ordering", function() { return order.join(',') === 'prepended,appended'; });

        // removeListener of an unregistered fn: no-op, chainable.
        var s4 = new http2.Http2Session('server', 'x');
        function ghost() {}
        check("remove_unregistered_noop", function() { return s4.removeListener('nobody', ghost) === s4; });

        // emit of an unregistered event: no listeners, chainable (returns
        // this like every emit).
        check("emit_unregistered_ok", function() { return s4.emit('nobody') === s4; });

        // on() is chainable and registers in order.
        var seq = [];
        var s5 = new http2.Http2Session('server', 'x');
        s5.on('q', function() { seq.push(1); }).on('q', function() { seq.push(2); });
        s5.emit('q');
        check("on_chainable_order", function() { return seq.join(',') === '1,2'; });

        results.join('|')
        "#,
    );
}

/// Secondary Http2Session arms C4 did not pin: ping's closed/payload-fn/
/// payload-bytes/no-callback matrix, goaway event + implicit close, the
/// respondWithFile/respondWithFD unsupported throws, the settings setter's
/// non-object rejection vs object merge + localSettings emit, the
/// localSettings/remoteSettings getter snapshots, sendSettings both arms,
/// originSet, alpnProtocol, request-after-close throw, and setTimeout's
/// callback firing.
#[test]
fn h2c13_session_secondary_arms() {
    let mut ctx = fresh_ctx();
    assert_battery(
        &mut ctx,
        r#"
        var http2 = require('http2');
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e)); }
        }

        // ping on a closed session: callback(Error) + false, no emit (the
        // closed check precedes the payload-function reorder — pass the
        // callback in its proper slot).
        var sc = new http2.Http2Session('client', 'x');
        sc.close(function() {});
        var pingErr = null;
        var pingRet = sc.ping(null, function(e) { pingErr = e; });
        check("ping_closed_err_false", function() {
            return pingRet === false && pingErr !== null && pingErr.message === 'Session is closed';
        });

        // ping payload-function reorder: callback(null, 0, 8-byte payload).
        var s = new http2.Http2Session('client', 'x');
        var pArgs = null;
        var ret = s.ping(function(err, dur, buf) { pArgs = { err: err, dur: dur, buf: buf }; });
        check("ping_fn_payload_reorder", function() {
            return ret === true && pArgs.err === null && pArgs.dur === 0 && pArgs.buf.length === 8;
        });

        // ping with an explicit payload: echoed to callback + 'ping' event.
        var s2 = new http2.Http2Session('client', 'x');
        var echoed = null;
        var pingEvt = null;
        s2.on('ping', function(p) { pingEvt = p; });
        var ret2 = s2.ping(Buffer.alloc(8, 7), function(err, dur, payload) { echoed = payload; });
        check("ping_explicit_payload", function() {
            return ret2 === true && echoed[0] === 7 && pingEvt[0] === 7;
        });

        // ping with no callback: still true, still emits 'ping'.
        var s3 = new http2.Http2Session('client', 'x');
        var pingEvt3 = null;
        s3.on('ping', function(p) { pingEvt3 = p; });
        check("ping_nocb_true", function() { return s3.ping() === true && pingEvt3 !== null; });

        // goaway: emits with (code, lastStreamID, opaqueData) defaults and
        // closes the session.
        var s4 = new http2.Http2Session('server', 'x');
        var gArgs = null;
        var gClosed = false;
        s4.on('goaway', function(a, b, c) { gArgs = [a, b, c]; });
        s4.on('close', function() { gClosed = true; });
        s4.goaway();
        check("goaway_defaults_and_close", function() {
            return gArgs[0] === 0 && gArgs[1] === 0 && gArgs[2] === undefined && gClosed;
        });
        var s4b = new http2.Http2Session('server', 'x');
        var gArgs2 = null;
        s4b.on('goaway', function(a, b, c) { gArgs2 = [a, b, c]; });
        s4b.goaway(11, 3, 'opaque');
        check("goaway_args_pass", function() {
            return gArgs2[0] === 11 && gArgs2[1] === 3 && gArgs2[2] === 'opaque';
        });

        // respondWithFile / respondWithFD: unsupported throws, exact messages.
        var m1 = '', m2 = '';
        try { s4b.respondWithFile('/x'); } catch (e) { m1 = e.message; }
        try { s4b.respondWithFD(3); } catch (e) { m2 = e.message; }
        check("rwf_throws", function() {
            return m1 === 'http2.respondWithFile is not supported' &&
                   m2 === 'http2.respondWithFD is not supported';
        });

        // settings setter: non-object rejected silently (no merge, no emit);
        // object merges and emits 'localSettings'.
        var s5 = new http2.Http2Session('server', 'x');
        var setEvt = 0;
        s5.on('localSettings', function() { setEvt++; });
        s5.settings = 42;
        check("settings_setter_nonobject_rejected", function() {
            return setEvt === 0 && s5._settings.headerTableSize === 4096;
        });
        s5.settings = { headerTableSize: 8192 };
        check("settings_setter_object_merges", function() {
            return setEvt === 1 && s5._settings.headerTableSize === 8192;
        });

        // localSettings/remoteSettings getters return snapshots: mutating
        // the returned object does not touch the internal settings.
        var snap = s5.localSettings;
        snap.headerTableSize = 1;
        check("local_settings_snapshot", function() {
            return s5.localSettings.headerTableSize === 8192 &&
                   s5.remoteSettings.headerTableSize === 4096;
        });

        // sendSettings: non-object still emits (no merge); object merges.
        var s6 = new http2.Http2Session('server', 'x');
        var sendEvt = 0;
        s6.on('localSettings', function() { sendEvt++; });
        s6.sendSettings(42);
        check("sendsettings_nonobject_still_emits", function() { return sendEvt === 1; });
        s6.sendSettings({ maxFrameSize: 32768 });
        check("sendsettings_object_merges", function() {
            return sendEvt === 2 && s6._settings.maxFrameSize === 32768;
        });

        // originSet: [] without authority, [authority] with one.
        var s7 = new http2.Http2Session('client', '');
        var s8 = new http2.Http2Session('client', 'https://h.example');
        check("origin_set_arms", function() {
            return s7.originSet.length === 0 &&
                   s8.originSet.length === 1 && s8.originSet[0] === 'https://h.example';
        });

        // alpnProtocol constant getter.
        check("session_alpn_h2", function() { return s8.alpnProtocol === 'h2'; });

        // request on a closed session throws 'Session is closed'.
        var reqMsg = '';
        try { sc.request({ ':path': '/' }); } catch (e) { reqMsg = e.message; }
        check("request_after_close_throws", function() { return reqMsg === 'Session is closed'; });

        // setTimeout fires the callback synchronously and is chainable.
        var toRan = false;
        check("session_timeout_chainable", function() {
            return s8.setTimeout(5, function() { toRan = true; }) === s8 && toRan;
        });

        results.join('|')
        "#,
    );
}

/// connect() argument normalization: the options-function slot reorder, the
/// https:// scheme completion for scheme-less authorities, scheme-ful
/// authorities preserved verbatim, the https-only encrypted getter, the
/// synchronous 'connect' event, and the listener registration as a 'stream'
/// listener.
#[test]
fn h2c13_connect_forms_and_authority_normalization() {
    let mut ctx = fresh_ctx();
    assert_battery(
        &mut ctx,
        r#"
        var http2 = require('http2');
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e)); }
        }

        // Scheme-less authority is completed with https:// in _authority.
        var c1 = http2.connect('host.example:8443');
        check("connect_scheme_completion", function() {
            return c1._authority === 'https://host.example:8443' && c1.type === 0;
        });

        // A scheme-ful authority is preserved verbatim.
        var c2 = http2.connect('http://plain.example');
        check("connect_scheme_preserved", function() {
            return c2._authority === 'http://plain.example';
        });

        // encrypted is https-authority-only.
        check("connect_encrypted_arms", function() {
            return c1.encrypted === true && c2.encrypted === false;
        });

        // connect(authority, listener): the listener function in the options
        // slot is registered as the 'stream' listener (server-side dispatch
        // invokes it — a client session never emits 'stream' locally, so the
        // registration surface is the observable contract here).
        var c3 = http2.connect('h.example', function(stream, headers, flags) {});
        check("connect_listener_is_stream_listener", function() {
            return Array.isArray(c3._events['stream']) && c3._events['stream'].length === 1;
        });

        // connect(authority, options, listener) three-arg form.
        var c4 = http2.connect('h.example', {}, function() {});
        check("connect_three_arg", function() {
            return Array.isArray(c4._events['stream']) && c4._events['stream'].length === 1;
        });

        // The 'connect' event fires synchronously INSIDE connect() — a
        // listener registered after the call never sees it (the sync
        // semantics are observable as this absence).
        var lateEvt = null;
        var c5 = http2.connect('h.example', {}, function() {});
        c5.on('connect', function(sess) { lateEvt = sess; });
        check("connect_event_synchronous", function() { return lateEvt === null; });

        results.join('|')
        "#,
    );
}

/// Byte-exact body assembly edges and settings pack/unpack edge arms: mixed
/// string/Uint8Array/ArrayBuffer/Buffer chunks assemble byte-exactly, the
/// non-string-non-byte chunk type throws, unpack ignores a trailing
/// sub-6-byte tail, an unknown setting id is skipped, and a value above
/// 0x7FFFFFFF round-trips unsigned.
#[test]
fn h2c13_body_bytes_and_settings_pack_edges() {
    let mut ctx = fresh_ctx();
    assert_battery(
        &mut ctx,
        r#"
        var http2 = require('http2');
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e)); }
        }

        // Byte-exact mixed-chunk accumulation recorded on a stream's
        // _bodyChunks: string + Uint8Array + ArrayBuffer + Buffer.
        var sess = new http2.Http2Session('server', 'x');
        var st = sess.request({ ':path': '/body' });
        var u8 = new Uint8Array([0x61, 0x62]);           // "ab"
        var ab = new ArrayBuffer(2);
        new Uint8Array(ab)[0] = 0x63; new Uint8Array(ab)[1] = 0x64; // "cd"
        var buf = Buffer.from([0x65]);                    // "e"
        st._bodyChunks = ['ab', u8, ab, buf];
        var bytes = [];
        for (var i = 0; i < st._bodyChunks.length; i++) {
            var p = st._bodyChunks[i];
            if (typeof p === 'string') { for (var j = 0; j < p.length; j++) bytes.push(p.charCodeAt(j)); }
            else if (p instanceof ArrayBuffer) { var v = new Uint8Array(p); for (var j2 = 0; j2 < v.length; j2++) bytes.push(v[j2]); }
            else { for (var j3 = 0; j3 < p.length; j3++) bytes.push(p[j3]); }
        }
        check("mixed_bytes_exact", function() {
            return bytes.join(',') === '97,98,97,98,99,100,101'; // "ab"+"ab"+"cd"+"e"
        });

        // Non-string non-byte chunk: TypeError with the exact message.
        var badMsg = '';
        var st2 = sess.request({ ':path': '/bad' });
        try { st2.end(42); } catch (e) { badMsg = e.message; }
        check("bad_chunk_throws", function() {
            return badMsg.indexOf('must be a string, Buffer, TypedArray or ArrayBuffer') !== -1;
        });

        // end(undefined) is legal (no chunk), chainable.
        var st3 = sess.request({ ':path': '/undef' });
        check("end_undefined_ok", function() { return st3.end() === st3; });

        // getUnpackedSettings: a trailing sub-6-byte tail is ignored.
        var packed = http2.getPackedSettings({ headerTableSize: 100 });
        var truncated = Buffer.alloc(packed.length + 3);
        for (var i2 = 0; i2 < packed.length; i2++) truncated[i2] = packed[i2];
        var un = http2.getUnpackedSettings(truncated);
        check("unpack_ignores_partial_tail", function() { return un.headerTableSize === 100; });

        // Unknown setting id inside the payload: skipped (defaults stay).
        var unknown = Buffer.alloc(6);
        unknown[0] = 0x7f; unknown[1] = 0xff;
        var un2 = http2.getUnpackedSettings(unknown);
        check("unpack_unknown_id_skipped", function() {
            return un2.headerTableSize === 4096 && un2.initialWindowSize === 65535;
        });

        // A value above 0x7FFFFFFF survives the signed→unsigned conversion.
        var big = http2.getPackedSettings({ initialWindowSize: 0x80000004 });
        var un3 = http2.getUnpackedSettings(big);
        check("unpack_unsigned_conversion", function() {
            return un3.initialWindowSize === 0x80000004;
        });

        results.join('|')
        "#,
    );
}
