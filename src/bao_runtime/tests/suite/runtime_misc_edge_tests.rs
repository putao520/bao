// @trace TEST-ENG-007-MISC-EDGE [req:REQ-ENG-007] [level:integration]
//
// C8b line-coverage wave, node-misc surface: node_tls.rs JS faces the deep
// families don't reach (inert TLSSocket method matrix, wrap-ctor refusal
// arms, createSecureContext setter matrix, server listen-without-credentials
// + address/close faces, tls.connect argument-form dispatch, connect-refused
// error delivery), node_zlib.rs async + brotli/unzip arms, node_cluster.rs
// JS surface, and the bao_runtime root exit/string helper fns.

use bao_engine::context::JsContext;
#[path = "common/mod.rs"]
mod common;

use common::{drive_event_loop, eval_string_full};

fn make_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

fn run_checks(ctx: &mut JsContext, body: &str) -> String {
    let wrapped = format!(
        r#"
        var __results = [];
        function check(label, fn) {{
            try {{ var ok = fn(); __results.push(label + (ok ? " PASS" : " FAIL")); }}
            catch(e) {{ __results.push(label + " ERR:" + (e.message || e).toString().substring(0, 90)); }}
        }}
        {body}
        __results.join("\n");
        "#
    );
    eval_string_full(ctx, &wrapped)
}

fn assert_all_pass(results: &str) {
    assert!(
        !results.contains("FAIL") && !results.contains("ERR:"),
        "misc edge checks failed:\n{}",
        results
    );
}

#[test]
fn tls_inert_socket_method_matrix() {
    let mut ctx = make_ctx();
    let results = run_checks(
        &mut ctx,
        r#"
        var tls = require('tls');

        // A TLSSocket built with no connection is inert: every getter answers
        // Node's "no live session" shape, write refuses, destroy flags.
        var s = new tls.TLSSocket();
        check("ctor_authorized_false_encrypted_true", function() {
            return s.authorized === false && s.encrypted === true;
        });
        check("write_no_conn_false", function() {
            return s.write('x') === false;
        });
        check("end_returns_this", function() {
            return s.end() === s;
        });
        check("destroy_flags_destroyed", function() {
            s.destroy();
            return s.destroyed === true;
        });
        check("getProtocol_null", function() {
            return s.getProtocol() === null;
        });
        check("getCipher_null", function() {
            return s.getCipher() === null;
        });
        check("getPeerCertificate_null", function() {
            return s.getPeerCertificate() === null;
        });
        check("getFinished_false_getPeerFinished_false", function() {
            return s.getFinished() === false && s.getPeerFinished() === false;
        });
        check("getSession_undefined", function() {
            return s.getSession() === undefined;
        });
        check("getALPNProtocol_undefined", function() {
            return s.getALPNProtocol() === undefined;
        });
        check("renegotiate_defect_characterization", function() {
            // DEFECT (registered, C8b report: TLS-RENEGOTIATE-NAME-TYPO):
            // node_tls.rs registers the renegotiate method under the name
            // " renegotiate" (leading space in the c-string literal), so the
            // documented surface name is undefined. Characterizing the
            // shipped state; the fix flips this check.
            return typeof s.renegotiate === 'undefined';
        });
        check("setEncoding_returns_this", function() {
            var s2 = new tls.TLSSocket();
            return s2.setEncoding('utf8') === s2;
        });
        check("ref_unref_return_this", function() {
            var s3 = new tls.TLSSocket();
            return s3.ref() === s3 && s3.unref() === s3;
        });
        check("proto_shape", function() {
            return typeof s.on === 'function' && typeof s.once === 'function'
                && typeof s.emit === 'function' && typeof s.addListener === 'function'
                && typeof s.removeListener === 'function' && typeof s.off === 'function'
                && typeof s.removeAllListeners === 'function'
                && typeof s.prependListener === 'function'
                && typeof s.prependOnceListener === 'function';
        });
        "#
    );
    assert_all_pass(&results);
}

#[test]
fn tls_ctor_wrap_refusal_arms() {
    let mut ctx = make_ctx();
    let results = run_checks(
        &mut ctx,
        r#"
        var tls = require('tls');
        function catchMsg(fn) {
            try { fn(); return null; } catch (e) { return String(e.message || e); }
        }

        // Wrapping a plain object (no live net.Socket `_ptr`) refuses loudly.
        check("wrap_plain_object_refused", function() {
            var m = catchMsg(function() { new tls.TLSSocket({}, {}); });
            return m !== null && m.indexOf('requires a live connected net.Socket') !== -1;
        });
        // isServer: true refuses (client-side upgrade is the scope). The
        // fake `_ptr` must be a NON-integral double: SM canonicalizes 1.0 to
        // an int32 jsval, which the live-socket guard rejects first.
        check("wrap_isServer_refused", function() {
            var m = catchMsg(function() {
                new tls.TLSSocket({ _ptr: 1.5 }, { isServer: true });
            });
            if (m === null || m.indexOf('server-side wrap') === -1) {
                throw new Error('GOT:' + m);
            }
            return true;
        });
        // A destroyed socket refuses.
        check("wrap_destroyed_socket_refused", function() {
            var m = catchMsg(function() {
                new tls.TLSSocket({ _ptr: 1.5, destroyed: true }, {});
            });
            if (m === null || m.indexOf('destroyed socket') === -1) {
                throw new Error('GOT:' + m);
            }
            return true;
        });
        // The refusal does not leave a half-built socket behind: the ctor
        // threw, so no return value exists (implicitly proven by catchMsg).
        "#
    );
    assert_all_pass(&results);
}

#[test]
fn tls_connect_argument_forms_and_refusal() {
    let mut ctx = make_ctx();
    let results = run_checks(
        &mut ctx,
        r#"
        var tls = require('tls');

        // No args / non-address args → undefined (the option-less guard).
        check("connect_no_args_undefined", function() {
            return tls.connect() === undefined;
        });
        check("connect_string_arg_undefined", function() {
            return tls.connect('nope') === undefined;
        });

        // Object form returns the early TLSSocket synchronously with the
        // full event surface.
        var s = tls.connect({ port: 1, host: '127.0.0.1' });
        check("connect_object_form_returns_socket", function() {
            return typeof s === 'object' && s !== null
                && typeof s.on === 'function' && typeof s.write === 'function';
        });
        check("connect_socket_encrypted_flag", function() {
            return s.encrypted === true;
        });

        // Port-only form (error listener attached up front: the refusal
        // below delivers 'error' on this socket and an unhandled 'error'
        // would route to the uncaught-exception path).
        var s2 = tls.connect(1);
        s2.on('error', function() {});
        check("connect_port_form_returns_socket", function() {
            return typeof s2 === 'object' && s2 !== null;
        });
        "#
    );
    assert_all_pass(&results);

    // Refused connect delivers 'error' + 'close' on the socket (the driver
    // worker's failure path) once the event loop is pumped.
    eval_string_full(
        &mut ctx,
        r#"
        var __tlsEvents = [];
        var tls = require('tls');
        var sk = tls.connect({ port: 1, host: '127.0.0.1' });
        sk.on('error', function(e) { __tlsEvents.push('error:' + String(e && e.message || '').substring(0, 40)); });
        sk.on('close', function() { __tlsEvents.push('close'); });
        sk.on('secureConnect', function() { __tlsEvents.push('secureConnect'); });
        "#,
    );
    drive_event_loop(&mut ctx, 120);
    let events = eval_string_full(&mut ctx, "__tlsEvents.join('|')");
    assert!(
        events.contains("error:") && events.contains("close") && !events.contains("secureConnect"),
        "refused connect should deliver error+close, got: {}",
        events
    );
}

#[test]
fn tls_secure_context_setter_matrix() {
    let (cert_pem, key_pem) =
        bao_boringssl_bridge::generate_self_signed_pem("c8b-edge.local", 365)
            .expect("generate self-signed PEM");
    let mut ctx = make_ctx();
    let script = format!(
        r#"
        var tls = require('tls');
        var CERT = "{cert}";
        var KEY = "{key}";
        var __results = [];
        function check(label, fn) {{
            try {{ var ok = fn(); __results.push(label + (ok ? " PASS" : " FAIL")); }}
            catch(e) {{ __results.push(label + " ERR:" + (e.message || e).toString().substring(0, 90)); }}
        }}

        var sc = tls.createSecureContext();
        check("sc_shape", function() {{
            return typeof sc === 'object' && typeof sc.setKey === 'function'
                && typeof sc.setCert === 'function' && typeof sc.addCACert === 'function'
                && typeof sc.setCA === 'function' && typeof sc.setALPNProtocols === 'function'
                && typeof sc.setSession === 'function';
        }});
        check("sc_setters_accept_strings", function() {{
            sc.setKey(KEY);
            sc.setCert(CERT);
            sc.addCACert(CERT);
            sc.setCA([CERT]);
            return true;
        }});
        check("sc_setALPNProtocols_array", function() {{
            return sc.setALPNProtocols(['h2', 'http/1.1']) === undefined;
        }});
        check("sc_setSession_string", function() {{
            return sc.setSession('session-bytes') === undefined;
        }});
        // Non-parseable / wrong-shape inputs are silent no-ops (never throw).
        check("sc_setters_garbage_noop", function() {{
            sc.setKey(42);
            sc.setCert(undefined);
            sc.addCACert({{}});
            sc.setALPNProtocols('h2');        // non-array → rejected
            sc.setALPNProtocols([]);          // empty wire → rejected
            sc.setSession(42);                // non-string → ignored
            return true;
        }});

        // Server-side: createServer stores key/cert and exposes the surface;
        // listen() without credentials fails closed with false.
        check("createServer_surface", function() {{
            var srv = tls.createServer({{ key: KEY, cert: CERT }});
            return typeof srv.listen === 'function' && typeof srv.close === 'function'
                && typeof srv.address === 'function' && typeof srv.on === 'function';
        }});
        check("listen_without_credentials_false", function() {{
            var bare = tls.createServer({{}});
            return bare.listen(0) === false;
        }});
        check("address_before_listen_undefined", function() {{
            var bare = tls.createServer({{}});
            return bare.address() === undefined;
        }});
        check("close_on_unlistened_server_ok", function() {{
            var bare = tls.createServer({{}});
            bare.close();
            return true;
        }});

        // checkServerIdentity stub face (real verification lives in-handshake).
        check("checkServerIdentity_undefined", function() {{
            return tls.checkServerIdentity('h', {{}}) === undefined;
        }});
        // getCiphers array face.
        check("getCiphers_shape", function() {{
            var cs = tls.getCiphers();
            return Array.isArray(cs) && cs.length >= 7 && cs.indexOf('TLS_AES_256_GCM_SHA384') !== -1;
        }});
        __results.join("\n");
        "#,
        cert = cert_pem.replace('\n', "\\n"),
        key = key_pem.replace('\n', "\\n"),
    );
    let results = eval_string_full(&mut ctx, &script);
    assert_all_pass(&results);
}

#[test]
fn tls_constants_and_module_shape() {
    let mut ctx = make_ctx();
    let results = run_checks(
        &mut ctx,
        r#"
        var tls = require('tls');
        check("default_versions", function() {
            return tls.DEFAULT_MIN_VERSION === 'TLSv1.2' && tls.DEFAULT_MAX_VERSION === 'TLSv1.3';
        });
        check("default_ciphers_string", function() {
            return tls.DEFAULT_CIPHERS.indexOf('TLS_AES_256_GCM_SHA384') === 0
                && tls.DEFAULT_CIPHERS.indexOf('TLS_AES_128_GCM_SHA256') !== -1;
        });
        check("TLSSocket_ctor_function", function() {
            return typeof tls.TLSSocket === 'function';
        });
        "#
    );
    assert_all_pass(&results);
}

#[test]
fn zlib_async_and_remaining_sync_arms() {
    let mut ctx = make_ctx();
    eval_string_full(
        &mut ctx,
        r#"
        var __zres = [];
        var zlib = require('zlib');
        function once(label) {
            return function(err, out) {
                if (err) { __zres.push(label + ':ERR'); return; }
                __zres.push(label + ':' + (out && out.length > 0 ? 'ok' : 'empty'));
            };
        }

        var plain = Buffer.from('async zlib payload');

        // Async faces registered by node_zlib: deflate/inflate/gzip/gunzip/
        // unzip/brotliCompress/brotliDecompress. (deflateRaw/inflateRaw have
        // NO async registration — node-parity gap registered in the C8b
        // report; the raw faces are sync-only here.)
        zlib.deflate(plain, once('deflate'));
        zlib.gzip(plain, once('gzip'));
        zlib.unzip(zlib.gzipSync(plain), once('unzip'));

        zlib.deflate(plain, function(err, z) { if (!err) zlib.inflate(z, once('inflate')); });
        zlib.gzip(plain, function(err, g) { if (!err) zlib.gunzip(g, once('gunzip')); });

        // Raw sync roundtrip.
        var rw = zlib.deflateRawSync(Buffer.from('raw payload'));
        __zres.push('rawSync:' + zlib.inflateRawSync(rw).toString());

        // Multi-member unzip (sync face): two gzip members concatenated.
        var m1 = zlib.gzipSync(Buffer.from('member-one'));
        var m2 = zlib.gzipSync(Buffer.from('member-two'));
        var joined = zlib.unzipSync(Buffer.concat([m1, m2]));
        __zres.push('unzipSync:' + joined.toString());

        // brotli sync + async roundtrips.
        var br = zlib.brotliCompressSync(Buffer.from('brotli payload here'));
        var brOut = zlib.brotliDecompressSync(br);
        __zres.push('brotli:' + brOut.toString());
        zlib.brotliCompress(plain, function(err, b) {
            if (err) { __zres.push('brotli-async:ERR'); return; }
            zlib.brotliDecompress(b, function(err2, out) {
                __zres.push('brotli-async:' + (err2 ? 'ERR' : out.toString()));
            });
        });

        // crc32 face.
        __zres.push('crc32:' + (typeof zlib.crc32 === 'function'));
        "#,
    );
    drive_event_loop(&mut ctx, 200);
    let results = eval_string_full(&mut ctx, "__zres.sort().join('|')");
    for expected in [
        "brotli:brotli payload here",
        "brotli-async:async zlib payload",
        "crc32:true",
        "deflate:ok",
        "gunzip:ok",
        "gzip:ok",
        "inflate:ok",
        "rawSync:raw payload",
        "unzip:ok",
        "unzipSync:member-one",
    ] {
        assert!(
            results.contains(expected),
            "zlib expected `{}` in: {}",
            expected,
            results
        );
    }
}

#[test]
fn cluster_surface_arms() {
    let mut ctx = make_ctx();
    let results = run_checks(
        &mut ctx,
        r#"
        var cluster = require('cluster');

        // This test process never forked: it is the primary.
        check("isPrimary_true", function() {
            return cluster.isPrimary === true || cluster.isMaster === true;
        });
        check("workers_object_face", function() {
            return typeof cluster.workers === 'object' || cluster.workers === undefined;
        });
        check("fork_disconnect_scheduling_faces", function() {
            return typeof cluster.fork === 'function'
                && typeof cluster.disconnect === 'function'
                && typeof cluster.setupPrimary === 'function'
                && typeof cluster.setupMaster === 'function'
                && typeof cluster.on === 'function'
                && typeof cluster.schedulingPolicy !== 'undefined';
        });
        check("settings_object_face", function() {
            return typeof cluster.settings === 'object' || cluster.settings === undefined;
        });
        check("isWorker_false", function() {
            return cluster.isWorker === false;
        });
        "#
    );
    assert_all_pass(&results);
}

#[test]
fn runtime_root_exit_and_string_helpers() {
    // bao_runtime root fns: exit flag lifecycle.
    bun_runtime::clear_exit();
    assert!(!bun_runtime::should_exit());
    assert_eq!(bun_runtime::exit_code(), 0);

    bun_runtime::set_exit_code(3);
    assert_eq!(bun_runtime::exit_code(), 3);
    assert!(!bun_runtime::should_exit(), "set_exit_code alone must not raise the exit flag");

    bun_runtime::request_exit(7);
    assert!(bun_runtime::should_exit());
    assert_eq!(bun_runtime::exit_code(), 7);

    bun_runtime::clear_exit();
    assert!(!bun_runtime::should_exit());
    assert_eq!(bun_runtime::exit_code(), 0);

    // The boringssl bridge's PEM generator (used by the TLS faces above)
    // links and produces a parseable certificate body.
    let (cert, _key) = bao_boringssl_bridge::generate_self_signed_pem("c8b-root", 1)
        .expect("pem for link check");
    assert!(cert.contains("BEGIN CERTIFICATE"));

    // force_link_bun_install: keeps the bun_install compilation unit linked.
    bun_runtime::force_link_bun_install();

    // shutdown_thread_sm is callable between contexts (no-op when idle).
    bun_runtime::shutdown_thread_sm();
}

#[test]
fn http_surface_arms() {
    let mut ctx = make_ctx();
    let results = run_checks(
        &mut ctx,
        r#"
        var http = require('http');

        check("STATUS_CODES_map", function() {
            return http.STATUS_CODES['200'] === 'OK'
                && http.STATUS_CODES['404'] === 'Not Found'
                && http.STATUS_CODES['500'] === 'Internal Server Error';
        });
        check("METHODS_array", function() {
            return Array.isArray(http.METHODS)
                && http.METHODS.indexOf('GET') !== -1
                && http.METHODS.indexOf('HEAD') !== -1
                && http.METHODS.indexOf('POST') !== -1;
        });
        check("globalAgent_face", function() {
            return typeof http.globalAgent === 'object' || typeof http.globalAgent === 'function';
        });
        check("Agent_ctor_defect_characterization", function() {
            // DEFECT (registered, C8b report: HTTP-AGENT-CTOR-MISSING):
            // node_http.rs defines http.Agent by JS::Evaluate2-ing a top-level
            // `function Agent(...)` DECLARATION — Evaluate2's completion value
            // for a declaration is undefined, so `aval.is_object()` fails and
            // the property is never registered. globalAgent (plain object)
            // IS present. Characterizing the shipped state; the fix flips
            // this check.
            return typeof http.Agent === 'undefined'
                && (typeof http.globalAgent === 'object' || typeof http.globalAgent === 'function');
        });
        check("maxRedirects_default", function() {
            return typeof http.maxRedirects === 'number' || http.maxRedirects === undefined;
        });
        check("createServer_options_form", function() {
            var srv = http.createServer({}, function(req, res) {});
            return typeof srv === 'object' && srv !== null;
        });
        check("createServer_with_handler_shape", function() {
            var srv = http.createServer(function() {});
            return typeof srv.listen === 'function' && typeof srv.close === 'function'
                && typeof srv.on === 'function' && typeof srv.emit === 'function'
                && typeof srv.address === 'function';
        });
        "#
    );
    assert_all_pass(&results);

    // ServerResponse write-after-end guard exercised on a real served
    // response: the second write after end() throws the node-parity error.
    eval_string_full(
        &mut ctx,
        r#"
        var __httpProbe = 'unset';
        var http = require('http');
        var srv = http.createServer(function(req, res) {
            res.end('done');
            try { res.write('late'); __httpProbe = 'no-throw'; }
            catch (e) { __httpProbe = String(e.message || e); }
        });
        srv.listen(0, '127.0.0.1');
        http.get({ port: srv.address().port, host: '127.0.0.1' }, function(incoming) {
            incoming.resume();
            incoming.on('end', function() { srv.close(); });
        });
        "#,
    );
    drive_event_loop(&mut ctx, 120);
    let probe = eval_string_full(&mut ctx, "__httpProbe");
    assert_eq!(probe, "write after end", "write-after-end guard on live response");
}

#[test]
fn url_surface_edge_arms() {
    let mut ctx = make_ctx();
    let results = run_checks(
        &mut ctx,
        r#"
        var url = require('url');

        check("parse_auth_host_port", function() {
            var u = url.parse('http://user:pw@example.com:8080/x?q=1#frag');
            return u.protocol === 'http:' && u.auth === 'user:pw'
                && u.hostname === 'example.com' && u.port === '8080'
                && u.pathname === '/x' && u.query === 'q=1' && u.hash === '#frag';
        });
        check("parse_parse_query_object", function() {
            var u = url.parse('http://h/p?a=1&b=2', true);
            return u.query && u.query.a === '1' && u.query.b === '2';
        });
        check("parse_slashes_false_relative", function() {
            var u = url.parse('//h/p', false, true);
            return u.host === 'h' && u.pathname === '/p';
        });
        check("format_roundtrip", function() {
            var u = url.parse('http://example.com:99/p?x=1');
            return url.format(u) === 'http://example.com:99/p?x=1';
        });
        check("format_from_object_parts", function() {
            var s = url.format({ protocol: 'https', hostname: 'h.io', pathname: '/a', query: { b: '2' } });
            return s === 'https://h.io/a?b=2' || s === 'https://h.io/a?b=2';
        });
        check("resolve_relative", function() {
            return url.resolve('http://h/a/b', 'c') === 'http://h/a/c'
                && url.resolve('http://h/a/b', '/z') === 'http://h/z';
        });

        check("URL_class_face", function() {
            var u = new URL('https://ex.io/p/a?b=2');
            return u.origin === 'https://ex.io' && u.pathname === '/p/a'
                && u.searchParams.get('b') === '2';
        });
        check("URL_searchparams_matrix", function() {
            var sp = new URLSearchParams('a=1&b=2&a=3');
            return sp.get('a') === '1' && sp.getAll('a').length === 2
                && sp.has('b') === true && !sp.has('zz')
                && sp.get('zz') === null;
        });
        check("URL_searchparams_mutators", function() {
            var sp = new URLSearchParams('a=1');
            sp.append('a', '2');
            sp.set('c', '3');
            sp.delete('a');
            return sp.get('a') === null && sp.get('c') === '3'
                && sp.toString().indexOf('c=3') !== -1;
        });
        check("URL_ipv6_host", function() {
            var u = new URL('http://[::1]:8080/x');
            return u.hostname === '[::1]' && u.port === '8080';
        });
        check("URL_requires_arg_throws", function() {
            try { new URL(); return false; }
            catch (e) { return String(e.message).indexOf('at least 1 argument') !== -1; }
        });
        check("URLSearchParams_empty", function() {
            var sp = new URLSearchParams();
            return sp.toString() === '' && sp.get('x') === null;
        });
        "#
    );
    assert_all_pass(&results);
}

#[test]
fn fetch_async_public_surface_arms() {
    // new_abort_id is a monotonic process counter.
    let a = bun_runtime::fetch_async::new_abort_id();
    let b = bun_runtime::fetch_async::new_abort_id();
    assert!(b > a, "abort ids must be monotonic: {} then {}", a, b);

    // No fetch scheduled on this fresh thread → no pending fetch.
    assert!(!bun_runtime::fetch_async::has_pending());

    // stream_phase on a null pointer answers Pending (observation hook).
    assert_eq!(
        bun_runtime::fetch_async::stream_phase(::std::ptr::null_mut()),
        bun_runtime::fetch_async::StreamPhase::Pending
    );

    // The staging high-water mark is a positive bound.
    assert!(bun_runtime::fetch_async::UNOBSERVED_BODY_HIGH_WATER_MARK > 0);

    // Abort registry: a late trigger_abort on a never-registered id is a
    // no-op (the settled-fetch branch).
    bun_runtime::fetch_async::trigger_abort(u32::MAX - 1);

    // FetchTlsInit default = no per-fetch TLS overrides.
    let tls_init = bun_runtime::fetch_async::FetchTlsInit::default();
    assert!(tls_init.ca_certs_der.is_empty());
    assert!(tls_init.reject_unauthorized.is_none());
    assert!(tls_init.servername.is_none());
}
