// @trace TEST-ENG-FETCH-TLS [req:REQ-ENG-001 REQ-ENG-006] [level:e2e]
// fetch init.tls (undici dispatcher tls subset) end-to-end, over a real
// self-signed BoringSSL HTTPS capture server:
//   1. no tls            → fail-closed against system roots
//                          (error.DEPTH_ZERO_SELF_SIGNED_CERT rejection)
//   2. tls.ca (PEM str / PEM-in-Uint8Array / DER-in-Uint8Array)
//                        → trust-store override, 200 round-trip
//   3. tls.rejectUnauthorized (false → succeeds without ca; true + ca →
//      still verifies, succeeds against the override store)
//   4. tls.servername    → SNI override observed on the wire (server-side
//      ClientHello name ≠ URL host) + ca array mixing PEM string and DER view
//   5. wrong ca          → still fails closed against the override store
//   6. malformed tls objects throw synchronously (fail-closed parsing)
//
// Server: TlsServer (memory-BIO TlsConnection) per accepted TcpStream on a
// worker thread; records (sni, request) per connection. Two servers: A
// (CN=localhost) for phases 1-3/5, B (CN=alt-sni.test) for the servername
// override — its cert is only trusted via B's own ca AND only matches an
// identity check against the servername override, so phase 4 proves both
// the SNI and identity-check redirection in one round-trip.
//
// Exit strategy mirrors fetch_init_e2e_tests (parked HTTPThread is a
// non-daemon thread; force-exit sidesteps the mimalloc atexit double-free).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bao_boringssl_bridge::{TlsServer, generate_self_signed_pem, pem_parse_certs};
use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

/// One served connection: the ClientHello SNI name + the (lossy) HTTP/1.1
/// request bytes decrypted off the wire ("" when the client aborted before
/// sending, e.g. the fail-closed phase).
#[derive(Debug, Clone)]
struct ConnRecord {
    sni: Option<String>,
    request: String,
}

type Records = Arc<Mutex<Vec<ConnRecord>>>;

/// True once `buf` holds a complete HTTP/1.1 request (header block and, when
/// Content-Length is present, the full body) — same contract as
/// fetch_init_e2e_tests.
fn request_complete(buf: &[u8]) -> bool {
    let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
        return false;
    };
    let head = String::from_utf8_lossy(&buf[..pos]).to_lowercase();
    let clen = head.lines().find_map(|l| {
        l.strip_prefix("content-length:")
            .and_then(|v| v.trim().parse::<usize>().ok())
    });
    match clen {
        Some(n) => buf.len() >= pos + 4 + n,
        None => true,
    }
}

/// Serve exactly one HTTPS connection: drive the memory-BIO TlsConnection
/// over `stream`, capture SNI + request, answer with a fixed 200 and a clean
/// close_notify. `Connection: close` keeps the client from pooling the
/// socket, so every fetch phase is a fresh handshake with a fresh SNI record.
fn serve_one(server: &TlsServer, mut stream: TcpStream, records: &Records) {
    let Ok(mut conn) = server.accept() else {
        return;
    };
    stream.set_read_timeout(Some(Duration::from_millis(300))).ok();

    let mut plaintext = Vec::new();
    let mut sni: Option<String> = None;
    let deadline = Instant::now() + Duration::from_secs(15);

    // Phase A+B: handshake, then request accumulation.
    while Instant::now() < deadline {
        let Ok(res) = conn.process() else {
            break;
        };
        let out = conn.take_outgoing();
        if !out.is_empty() && stream.write_all(&out).is_err() {
            break;
        }
        for chunk in &res.plaintext {
            plaintext.extend_from_slice(chunk);
        }
        if !conn.is_handshaking() {
            if sni.is_none() {
                sni = conn.servername();
            }
            if request_complete(&plaintext) {
                break;
            }
        }
        let mut buf = [0u8; 16 * 1024];
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => conn.feed(&buf[..n]),
            Err(_) => std::thread::sleep(Duration::from_millis(2)),
        }
    }

    // Respond 200 + clean shutdown (only when the request actually arrived;
    // the fail-closed phases close before sending anything).
    if request_complete(&plaintext) {
        let resp = "HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nTLS-OK";
        if conn.write(resp.as_bytes()).is_ok() {
            let _ = conn.queue_close_notify();
            let flush_deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < flush_deadline {
                if conn.process().is_err() {
                    break;
                }
                let out = conn.take_outgoing();
                if out.is_empty() {
                    break;
                }
                if stream.write_all(&out).is_err() {
                    break;
                }
            }
        }
    }

    records.lock().unwrap().push(ConnRecord {
        sni,
        request: String::from_utf8_lossy(&plaintext).to_lowercase(),
    });
}

/// HTTPS capture server on 127.0.0.1:0 serving `cert`/`key` until the
/// process-wide test deadline. Returns (port, records).
fn start_tls_capture_server(cert: &str, key: &str) -> (u16, Records) {
    let server = TlsServer::new(cert, key).expect("TlsServer::new");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let records: Records = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&records);
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(120);
        listener.set_nonblocking(true).ok();
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((stream, _)) => {
                    stream.set_nonblocking(false).ok();
                    serve_one(&server, stream, &sink);
                }
                Err(_) => std::thread::sleep(Duration::from_millis(2)),
            }
        }
    });
    (port, records)
}

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<fetch-tls-test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(JsValue::Null) => "null".to_string(),
        Ok(JsValue::Undefined) => "undefined".to_string(),
        Ok(JsValue::Object(_)) => "[object]".to_string(),
        Err(e) => format!("ERROR:{}", e.message),
    }
}

/// Escape a PEM string for embedding in a JS double-quoted string literal.
fn js_str(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('"', "\\\"")
        .replace('\r', "\\r")
}

/// Format DER bytes as a JS array literal (for `new Uint8Array([...])`).
fn js_bytes_lit(der: &[u8]) -> String {
    let items: Vec<String> = der.iter().map(|b| format!("{}", b)).collect();
    format!("[{}]", items.join(","))
}

/// First server-side record whose request line mentions `path`.
fn record_for(records: &Records, path: &str) -> Option<ConnRecord> {
    records
        .lock()
        .unwrap()
        .iter()
        .find(|r| r.request.contains(path))
        .cloned()
}

#[test]
fn test_fetch_init_tls_e2e() {
    bun_core::output::init_test();
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();

    // Server A: CN=localhost (URL host matches). Server B: CN=alt-sni.test
    // (only reachable via the servername override: trust via B's ca AND
    // identity check against the override name).
    let (cert_a, key_a) = generate_self_signed_pem("localhost", 365).expect("cert A");
    let (cert_b, key_b) = generate_self_signed_pem("alt-sni.test", 365).expect("cert B");
    let der_a: Vec<u8> = pem_parse_certs(&cert_a).into_iter().next().expect("DER A");
    let der_b: Vec<u8> = pem_parse_certs(&cert_b).into_iter().next().expect("DER B");

    let (port_a, records_a) = start_tls_capture_server(&cert_a, &key_a);
    let (port_b, records_b) = start_tls_capture_server(&cert_b, &key_b);
    std::thread::sleep(Duration::from_millis(50));

    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    // ── Sync fail-closed parsing (malformed init.tls throws; nothing is
    //    silently ignored, nothing silently degrades to system roots) ──────
    let sync_out = eval_string(
        &mut ctx,
        &format!(
            r#"
            (function() {{
                var out = [];
                var base = "https://localhost:{port_a}";
                function throwsOf(init) {{
                    try {{ fetch(base + "/never", init); return "NO-THROW"; }}
                    catch (e) {{ return (e && e.message) || String(e); }}
                }}
                out.push(throwsOf({{ tls: 5 }}));
                out.push(throwsOf({{ tls: {{ ca: "not a pem" }} }}));
                out.push(throwsOf({{ tls: {{ ca: [] }} }}));
                out.push(throwsOf({{ tls: {{ servername: "" }} }}));
                out.push(throwsOf({{ tls: {{ rejectUnauthorized: "yes" }} }}));
                return out.join("|||");
            }})()
            "#,
            port_a = port_a,
        ),
    );
    let sync_parts: Vec<&str> = sync_out.split("|||").collect();
    assert_eq!(sync_parts.len(), 5, "sync fail-closed probes: {}", sync_out);
    assert!(
        sync_parts[0].contains("init.tls must be an object"),
        "tls:5 must throw, got {}",
        sync_parts[0]
    );
    assert!(
        sync_parts[1].contains("no parseable certificate"),
        "ca:\"not a pem\" must throw, got {}",
        sync_parts[1]
    );
    assert!(
        sync_parts[2].contains("no parseable certificate"),
        "ca:[] must throw, got {}",
        sync_parts[2]
    );
    assert!(
        sync_parts[3].contains("non-empty"),
        "servername:\"\" must throw, got {}",
        sync_parts[3]
    );
    assert!(
        sync_parts[4].contains("must be a boolean"),
        "rejectUnauthorized:\"yes\" must throw, got {}",
        sync_parts[4]
    );

    // ── Async phases over the live TLS servers ────────────────────────────
    let js = format!(
        r#"
        (function() {{
            var baseA = "https://localhost:{port_a}";
            var baseB = "https://localhost:{port_b}";
            var pemA = "{pem_a}";
            var pemB = "{pem_b}";
            var pemABytes = new Uint8Array(Array.from(pemA).map(function(c) {{ return c.charCodeAt(0); }}));
            var derABytes = new Uint8Array({der_a});
            var derBBytes = new Uint8Array({der_b});
            globalThis.__r = {{}};
            function phase(name, p) {{
                return p.then(
                    function(v) {{ globalThis.__r[name] = "OK:" + v; }},
                    function(e) {{ globalThis.__r[name] = "ERR:" + ((e && e.message) || String(e)); }}
                );
            }}
            (async function() {{
                await phase("p1-default", fetch(baseA + "/p1-default").then(function(r) {{ return r.text(); }}));
                await phase("p2-ca-pem", fetch(baseA + "/p2-ca-pem", {{ tls: {{ ca: pemA }} }})
                    .then(function(r) {{ return r.text(); }}));
                await phase("p2b-ca-pem-bytes", fetch(baseA + "/p2b-ca-pem-bytes", {{ tls: {{ ca: pemABytes }} }})
                    .then(function(r) {{ return r.text(); }}));
                await phase("p2c-ca-der-bytes", fetch(baseA + "/p2c-ca-der-bytes", {{ tls: {{ ca: derABytes }} }})
                    .then(function(r) {{ return r.text(); }}));
                await phase("p3-insecure", fetch(baseA + "/p3-insecure", {{ tls: {{ rejectUnauthorized: false }} }})
                    .then(function(r) {{ return r.text(); }}));
                await phase("p3b-secure-plus-ca", fetch(baseA + "/p3b-secure-plus-ca", {{ tls: {{ ca: pemA, rejectUnauthorized: true }} }})
                    .then(function(r) {{ return r.text(); }}));
                await phase("p4-sni-override", fetch(baseB + "/p4-sni-override",
                        {{ tls: {{ ca: [pemB, derBBytes], servername: "alt-sni.test" }} }})
                    .then(function(r) {{ return r.text(); }}));
                await phase("p5-wrong-ca", fetch(baseB + "/p5-wrong-ca", {{ tls: {{ ca: pemA }} }})
                    .then(function(r) {{ return r.text(); }}));
            }})().then(function() {{ globalThis.__alldone = true; }},
                      function(e) {{ globalThis.__fatal = String(e); globalThis.__alldone = true; }});
            return "scheduled";
        }})()
        "#,
        port_a = port_a,
        port_b = port_b,
        pem_a = js_str(&cert_a),
        pem_b = js_str(&cert_b),
        der_a = js_bytes_lit(&der_a),
        der_b = js_bytes_lit(&der_b),
    );
    let setup_out = eval_string(&mut ctx, &js);
    assert!(
        setup_out.contains("scheduled"),
        "fetch tls setup failed: {}",
        setup_out
    );

    // Drive the event loop until all 8 phases settle.
    let cx_raw = ctx.raw_cx();
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        unsafe {
            mozjs_sys::jsapi::js::RunJobs(cx_raw);
        }
        bun_runtime::timers::with_event_loop(|loop_| {
            loop_.tick_without_idle(std::ptr::null_mut());
        });
        std::thread::sleep(Duration::from_millis(2));
        let done = eval_string(&mut ctx, r#"String(globalThis.__alldone === true)"#);
        if done == "true" {
            break;
        }
    }

    let fatal = eval_string(&mut ctx, r#"String(globalThis.__fatal)"#);
    assert_eq!(fatal, "undefined", "phase driver crashed: {}", fatal);
    let results = eval_string(
        &mut ctx,
        r#"
        (function() {
            var r = globalThis.__r || {};
            return ["p1-default","p2-ca-pem","p2b-ca-pem-bytes","p2c-ca-der-bytes",
                    "p3-insecure","p3b-secure-plus-ca","p4-sni-override","p5-wrong-ca"]
                .map(function(k) { return k + "=" + (r[k] === undefined ? "UNSET" : r[k]); })
                .join("|||");
        })()
        "#,
    );
    let phases: Vec<&str> = results.split("|||").collect();
    assert_eq!(phases.len(), 8, "phase results: {}", results);
    let mut got = std::collections::HashMap::new();
    for p in &phases {
        let (k, v) = p.split_once('=').expect("k=v");
        got.insert(k.to_string(), v.to_string());
    }

    // 1. No tls → fail-closed against system roots (the e-f6 posture,
    //    unchanged by this feature).
    assert_eq!(
        got.get("p1-default").map(String::as_str),
        Some("ERR:error.DEPTH_ZERO_SELF_SIGNED_CERT"),
        "p1 default must fail closed: {:?}",
        got.get("p1-default")
    );
    // 2. ca override (all three input shapes) → trusted round-trip.
    for k in ["p2-ca-pem", "p2b-ca-pem-bytes", "p2c-ca-der-bytes"] {
        assert_eq!(
            got.get(k).map(String::as_str),
            Some("OK:TLS-OK"),
            "{} must succeed via ca override: {:?}",
            k,
            got.get(k)
        );
    }
    // 3. rejectUnauthorized:false succeeds without ca (explicit instruction);
    //    rejectUnauthorized:true + ca still verifies (and passes override).
    assert_eq!(
        got.get("p3-insecure").map(String::as_str),
        Some("OK:TLS-OK"),
        "p3 rejectUnauthorized:false must succeed: {:?}",
        got.get("p3-insecure")
    );
    assert_eq!(
        got.get("p3b-secure-plus-ca").map(String::as_str),
        Some("OK:TLS-OK"),
        "p3b verify-on + ca must succeed: {:?}",
        got.get("p3b-secure-plus-ca")
    );
    // 4. servername override: trusted + identity-matched round-trip (the
    //    wire SNI assertion lives in the server records below).
    assert_eq!(
        got.get("p4-sni-override").map(String::as_str),
        Some("OK:TLS-OK"),
        "p4 servername override must succeed: {:?}",
        got.get("p4-sni-override")
    );
    // 5. Wrong CA still fails closed against the override store.
    assert!(
        got.get("p5-wrong-ca").map_or(false, |v| v.starts_with("ERR:")),
        "p5 wrong ca must fail closed: {:?}",
        got.get("p5-wrong-ca")
    );

    // ── Server-side wire assertions ────────────────────────────────────────
    // SNI for server A phases = URL host (localhost), i.e. the override is
    // off unless requested.
    for path in ["/p2-ca-pem", "/p3-insecure", "/p3b-secure-plus-ca"] {
        let rec = record_for(&records_a, path)
            .unwrap_or_else(|| panic!("no server-A record for {}", path));
        assert_eq!(
            rec.sni.as_deref(),
            Some("localhost"),
            "{} SNI must be the URL host",
            path
        );
    }
    // SNI for the servername override = the override name, NOT the URL host
    // (localhost) — the ClientHello carried the user's servername.
    let rec_b = record_for(&records_b, "/p4-sni-override")
        .expect("no server-B record for /p4-sni-override");
    assert_eq!(
        rec_b.sni.as_deref(),
        Some("alt-sni.test"),
        "servername override must reach the ClientHello SNI extension"
    );
    // The wrong-ca connection must never have delivered its request to the
    // application (closed before send), and — being override-less — it must
    // have carried the URL host as SNI (the override is strictly opt-in per
    // fetch; p5 proves it does NOT stick to the next connection).
    let rec_b5 = record_for(&records_b, "/p5-wrong-ca");
    assert!(
        rec_b5.is_none(),
        "p5 wrong-ca request must not reach the server, got {:?}",
        rec_b5
    );
    let aborted_b = records_b
        .lock()
        .unwrap()
        .iter()
        .find(|r| r.request.is_empty())
        .cloned();
    assert_eq!(
        aborted_b.as_ref().and_then(|r| r.sni.as_deref()),
        Some("localhost"),
        "the override-less p5 connection must SNI the URL host, got {:?}",
        aborted_b
    );

    eprintln!(
        "[PASS] TEST-ENG-FETCH-TLS e2e: fail-closed default + ca override (PEM/bytes/DER) + rejectUnauthorized + servername SNI override + wrong-ca fail-closed + sync parse throws"
    );

    // Mirror fetch_init_e2e_tests exit strategy: park HTTPThread, force-exit.
    bun_http::http_thread::shutdown_for_exit();
    bun_runtime::shutdown_thread_sm();
    std::process::exit(0);
}
