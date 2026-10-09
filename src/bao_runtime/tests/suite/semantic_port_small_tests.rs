// @trace TEST-ENG-007-SEMANTIC-PORT-SMALL [req:REQ-ENG-007] [level:integration]
//
//! SEMANTIC-PORT small batch 2 (9-18 backlog) — four upstream Bun node-API
//! semantic fixes triaged against the bao_runtime carriers:
//!
//! 1. `aafd78b6d5` node:crypto randomInt entropy-cache sampling → **PORTED**
//!    (bao drew one `RAND_bytes` per sample at `node_crypto.rs` randomInt;
//!    now samples the 2 KiB thread-local cache, one `RAND_bytes` per
//!    refill — `bun_runtime::node_crypto::entropy_cache`).
//! 2. `e85f06be16` console inspect depth cap on Map/Set/Array entries and
//!    Error cause chains → **EQUIVALENT** (bao's inspect carrier,
//!    `bun_inspect_api.rs`, applies `at_depth_cap` uniformly to every
//!    container class — Map/Set entries, array elements, typed arrays — and
//!    never walks Error `cause` / `AggregateError.errors` chains, so the
//!    unbounded-walk defect has no carrier; the console.log face routes
//!    through JSON.stringify, equally bounded).
//! 3. `baa67e3f4e` node:zlib chunk output without slice → **EQUIVALENT**
//!    (bao one-shot + streaming shims return exact-size fresh buffers via
//!    `bytes_to_js_uint8array` + `Buffer.from`; there is no shared 16 KiB
//!    output chunk object, so the JSC heap double-accounting the slice fix
//!    targets has no SM carrier. The post-fix upstream contract — small
//!    one-shot results own an exact-size backing store — is locked).
//! 4. `dc078c8bc4` node:https no connection reuse across per-request
//!    `checkServerIdentity` → **N-A-HOST** (the https.request carrier has no
//!    agent pool, no session cache, no per-request identity hook — every
//!    request is its own `start_fetch` TLS exchange; the probe locks the
//!    stronger wire property: N requests = N fresh handshakes, and the JS
//!    Agent bookkeeping never engages).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bao_boringssl_bridge::{TlsServer, generate_self_signed_pem};
use bao_engine::context::JsContext;
#[path = "common/mod.rs"]
mod common;
use common::capture_server::request_complete;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    common::eval_string_full_named(ctx, source, "<semantic-port-small>")
}

fn setup_ctx() -> JsContext {
    bun_core::output::init_test();
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

// ---------------------------------------------------------------------------
// 1. aafd78b6d5 — randomInt entropy cache (PORTED)
// ---------------------------------------------------------------------------

/// Carrier-level mechanics of the ported cache: CAPACITY one-byte takes
/// drain it exactly (byte-once handout, cursor only advances), the first
/// take past exhaustion refills with one `RAND_bytes` and restarts the
/// cursor at zero, consecutive refills hand out fresh entropy, and a
/// cache-filling draw bypasses the cursor entirely.
#[test]
fn random_int_entropy_cache_port_locks() {
    use bun_runtime::node_crypto::entropy_cache;

    let mut snap1 = [0u8; entropy_cache::CAPACITY];
    for i in 0..entropy_cache::CAPACITY {
        entropy_cache::take(&mut snap1[i..i + 1]);
    }
    assert_eq!(
        entropy_cache::remaining(),
        0,
        "CAPACITY one-byte takes must exhaust the cache exactly (byte-once handout)"
    );

    // Boundary: one take past exhaustion refills (cursor restarts at zero,
    // then the 8 drawn bytes are accounted).
    let mut probe = [0u8; 8];
    entropy_cache::take(&mut probe);
    assert_eq!(
        entropy_cache::remaining(),
        entropy_cache::CAPACITY - 8,
        "post-refill cursor must start at zero (one RAND_bytes per CAPACITY)"
    );

    let mut snap2 = [0u8; entropy_cache::CAPACITY];
    for i in 0..entropy_cache::CAPACITY {
        entropy_cache::take(&mut snap2[i..i + 1]);
    }
    assert_ne!(
        snap1, snap2,
        "two consecutive refills must hand out fresh entropy (equal would be a 2^-16384 collision)"
    );

    // A draw that fills the cache bypasses it — and never moves the cursor.
    let before = entropy_cache::remaining();
    let mut big = vec![0u8; entropy_cache::CAPACITY];
    entropy_cache::take(&mut big);
    assert_eq!(
        entropy_cache::remaining(),
        before,
        "cache-filling draw must bypass the cursor"
    );
    assert_ne!(&big[..], &snap2[..], "direct draw must be fresh entropy");
}

/// JS-visible sampling contract with the cache in the loop (≈39 refills per
/// 10000 draws): edge ranges hold (upstream's own boundary matrix), results
/// are integers, and 10000 draws over a 2^32 range do not repeat materially
/// (expected collisions ≈ 0.01).
#[test]
fn random_int_sampling_wire_locks() {
    let mut ctx = setup_ctx();
    let out = eval_string(
        &mut ctx,
        r#"
        (function() {
            var crypto = require('crypto');
            var bad = 0;
            var ranges = [
                [-5, 5],
                [0, 1],
                [Number.MIN_SAFE_INTEGER, Number.MIN_SAFE_INTEGER + Math.pow(2, 48) - 1],
                [0, Math.pow(2, 48) - 1]
            ];
            for (var ri = 0; ri < ranges.length; ri++) {
                for (var i = 0; i < 500; i++) {
                    var v = crypto.randomInt(ranges[ri][0], ranges[ri][1]);
                    if (!Number.isInteger(v) || v < ranges[ri][0] || v >= ranges[ri][1]) bad++;
                }
            }
            for (var i = 0; i < 500; i++) {
                var v = crypto.randomInt(7);
                if (!Number.isInteger(v) || v < 0 || v >= 7) bad++;
            }
            var seen = Object.create(null);
            var distinct = 0;
            for (var i = 0; i < 10000; i++) {
                var v = crypto.randomInt(0, 4294967296);
                if (!seen[v]) { seen[v] = 1; distinct++; }
            }
            return bad + ":" + distinct;
        })()
        "#,
    );
    let parts: Vec<&str> = out.split(':').collect();
    assert_eq!(parts.len(), 2, "randomInt sampling probe: {}", out);
    assert_eq!(parts[0], "0", "all samples must be in-range integers: {}", out);
    let distinct: usize = parts[1].parse().expect("distinct count");
    assert!(
        distinct >= 9990,
        "10000 draws over 2^32 must be materially distinct, got {}",
        distinct
    );
}

// ---------------------------------------------------------------------------
// 2. e85f06be16 — inspect depth cap on Map/Set/Array + Error chains
//    (EQUIVALENT — carrier bun_inspect_api.rs already caps uniformly and
//    never walks cause/AggregateError chains)
// ---------------------------------------------------------------------------

#[test]
fn inspect_depth_cap_map_set_array_error_chain_locks() {
    let mut ctx = setup_ctx();
    let out = eval_string(
        &mut ctx,
        r#"
        (function() {
            function mkMap(n) { var m = new Map(); var cur = m; for (var i = 0; i < n - 1; i++) { var nx = new Map(); cur.set(i, nx); cur = nx; } return m; }
            function mkSet(n) { var s = new Set(); var cur = s; for (var i = 0; i < n - 1; i++) { var nx = new Set(); cur.add(nx); cur = nx; } return s; }
            function mkArr(n) { var a = []; var cur = a; for (var i = 0; i < n - 1; i++) { var nx = []; cur.push(nx); cur = nx; } return a; }
            function mkCause(n) { var e = new Error("leaf"); for (var i = 0; i < n; i++) { e = new Error("e" + i, { cause: e }); } return e; }
            function mkAgg(n) { var es = []; for (var i = 0; i < n; i++) es.push(new Error("m" + i)); return new AggregateError(es, "many"); }
            var m = Bun.inspect(mkMap(1000));
            var s = Bun.inspect(mkSet(1000));
            var a = Bun.inspect(mkArr(1000));
            var c = Bun.inspect(mkCause(200));
            var g = Bun.inspect(mkAgg(100));
            var inf = Bun.inspect({ a: { b: { c: 1 } } }, { depth: Infinity });
            return [
                "mapLen=" + m.length, "mapMarker=" + (m.indexOf("[Map]") >= 0),
                "setLen=" + s.length, "setMarker=" + (s.indexOf("[Set]") >= 0),
                "arrLen=" + a.length, "arrMarker=" + (a.indexOf("[Array]") >= 0),
                "causeLen=" + c.length, "causeHead=" + (c.indexOf("Error") === 0),
                "causeWalked=" + (c.indexOf("cause") >= 0),
                "aggLen=" + g.length, "aggHead=" + (g.indexOf("AggregateError") === 0),
                "aggWalked=" + (g.indexOf("m0") >= 0),
                "infDeep=" + (inf.indexOf("c: 1") >= 0)
            ].join("|");
        })()
        "#,
    );
    let kv: std::collections::HashMap<&str, &str> = out
        .split('|')
        .filter_map(|p| p.split_once('='))
        .collect();
    assert_eq!(kv.len(), 13, "inspect depth probes: {}", out);

    // 1000-deep Map/Set/Array chains truncate at the depth cap with the
    // container marker (upstream's defect printed every level, MBs of
    // output, RangeError at deeper chains).
    let map_len: usize = kv["mapLen"].parse().expect("mapLen");
    assert!(map_len < 500 && kv["mapMarker"] == "true", "deep Map: {}", out);
    let set_len: usize = kv["setLen"].parse().expect("setLen");
    assert!(set_len < 500 && kv["setMarker"] == "true", "deep Set: {}", out);
    let arr_len: usize = kv["arrLen"].parse().expect("arrLen");
    assert!(arr_len < 500 && kv["arrMarker"] == "true", "deep Array: {}", out);

    // Error cause chain: bao prints the head (+ first stack frame) and does
    // NOT walk `cause` — the upstream unbounded-cause-walk defect has no
    // carrier. Same for AggregateError members.
    let cause_len: usize = kv["causeLen"].parse().expect("causeLen");
    assert!(
        cause_len < 1000 && kv["causeHead"] == "true" && kv["causeWalked"] == "false",
        "deep cause chain must render bounded head-only: {}",
        out
    );
    let agg_len: usize = kv["aggLen"].parse().expect("aggLen");
    assert!(
        agg_len < 1000 && kv["aggHead"] == "true" && kv["aggWalked"] == "false",
        "AggregateError members must not be walked: {}",
        out
    );

    // Control: the cap is the depth option, not a wall.
    assert_eq!(kv["infDeep"], "true", "depth: Infinity must print deep: {}", out);
}

// ---------------------------------------------------------------------------
// 3. baa67e3f4e — zlib one-shot output without slice (EQUIVALENT — bao
//    hands out exact-size fresh buffers; no shared 16 KiB chunk exists)
// ---------------------------------------------------------------------------

#[test]
fn zlib_one_shot_exact_size_output_locks() {
    let mut ctx = setup_ctx();
    let out = eval_string(
        &mut ctx,
        r#"
        (function() {
            var zlib = require('zlib');
            function round(n, fill) {
                var payload = Buffer.alloc(n, fill);
                var gz = zlib.gzipSync(payload);
                var out = zlib.gunzipSync(gz);
                var bytesOk = out.length === n;
                for (var i = 0; i < out.length; i++) { if (out[i] !== fill) { bytesOk = false; break; } }
                // Post-fix upstream contract: the one-shot result owns an
                // exact-size backing store (bun's slice path handed a view
                // into a 16384-byte chunk).
                return bytesOk + ":" + (out.buffer.byteLength === out.length) + ":" + out.buffer.byteLength;
            }
            var r = [];
            r.push("small=" + round(1024, 0x62));
            r.push("tiny=" + round(10, 7));
            r.push("large=" + round(65536, 0x21));
            var e = zlib.gunzipSync(zlib.gzipSync(Buffer.alloc(0)));
            r.push("empty=" + (e.length === 0) + ":" + (e.buffer.byteLength === 0));
            return r.join("|");
        })()
        "#,
    );
    assert!(
        out.contains("small=true:true:1024"),
        "1 KB one-shot result (upstream's regression shape) must round-trip with exact backing: {}",
        out
    );
    assert!(
        out.contains("tiny=true:true:10"),
        "tiny one-shot result must round-trip with exact backing: {}",
        out
    );
    assert!(
        out.contains("large=true:true:65536"),
        "64 KB one-shot result (past Z_DEFAULT_CHUNK) must still be exact: {}",
        out
    );
    assert!(
        out.contains("empty=true:true"),
        "empty-payload one-shot must succeed with a zero-length backing: {}",
        out
    );
}

// ---------------------------------------------------------------------------
// 4. dc078c8bc4 — https connection no-reuse across per-request
//    checkServerIdentity (N-A-HOST — no pool / session cache / per-request
//    hook in the carrier; probe locks N requests = N fresh handshakes)
// ---------------------------------------------------------------------------

/// One served connection: the (lossy) request head decrypted off the wire.
type ConnRecords = Arc<Mutex<Vec<String>>>;

/// Serve exactly one HTTPS connection: handshake, read the request, answer
/// a fixed 200 + close_notify, record the request head. One record = one
/// fresh TLS handshake = one independent identity verification.
fn serve_one(server: &TlsServer, mut stream: TcpStream, records: &ConnRecords) {
    let Ok(mut conn) = server.accept() else {
        return;
    };
    stream.set_read_timeout(Some(Duration::from_millis(300))).ok();

    let mut plaintext = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(15);
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
        if !conn.is_handshaking() && request_complete(&plaintext) {
            break;
        }
        let mut buf = [0u8; 16 * 1024];
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => conn.feed(&buf[..n]),
            Err(_) => std::thread::sleep(Duration::from_millis(2)),
        }
    }
    if request_complete(&plaintext) {
        let resp = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
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
    let head = String::from_utf8_lossy(&plaintext).to_lowercase();
    records.lock().unwrap().push(head);
}

fn start_tls_capture_server(cert: &str, key: &str) -> (u16, ConnRecords) {
    let server = TlsServer::new(cert, key).expect("TlsServer::new");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let records: ConnRecords = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&records);
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(60);
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

/// First record (connection) whose request line mentions `path` — bounded
/// wait, same starved-observer rationale as fetch_tls_init_e2e_tests.
fn wait_record_for(records: &ConnRecords, path: &str) -> Option<String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let hit = records
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.contains(path))
            .cloned();
        if hit.is_some() {
            return hit;
        }
        if Instant::now() > deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn test_https_no_connection_reuse_probe() {
    // Force-exit semantics via self-re-exec isolation (exit_isolation.rs):
    // the body ends in shutdown_for_exit + process::exit(0), which is only
    // legal in a process whose death IS the test's success exit.
    crate::exit_isolation::dispatch(
        "semantic_port_small_tests::test_https_no_connection_reuse_probe",
        test_https_no_connection_reuse_probe_body,
    );
}

fn test_https_no_connection_reuse_probe_body() {
    let (cert, key) = generate_self_signed_pem("localhost", 365).expect("cert");
    let (port, records) = start_tls_capture_server(&cert, &key);
    std::thread::sleep(Duration::from_millis(50));

    let mut ctx = setup_ctx();
    let js = format!(
        r#"
        (function() {{
            var https = require('https');
            var PEM = "{pem}";
            globalThis.__r = {{}};
            globalThis.__alldone = false;
            function one(name, path) {{
                return new Promise(function(resolve) {{
                    var req = https.request({{
                        hostname: 'localhost', port: {port}, path: path, method: 'GET',
                        ca: [PEM]
                    }}, function(res) {{
                        var body = '';
                        res.on('data', function(c) {{ body += (c && c.toString) ? c.toString() : String(c); }});
                        res.on('end', function() {{ globalThis.__r[name] = res.statusCode + ':' + body; resolve(); }});
                    }});
                    req.on('error', function(e) {{ globalThis.__r[name] = 'ERR:' + ((e && e.message) || String(e)); resolve(); }});
                    req.end();
                }});
            }}
            (async function() {{
                await one('p1', '/p1-first');
                await one('p2', '/p2-second');
            }})().then(function() {{ globalThis.__alldone = true; }},
                      function(e) {{ globalThis.__fatal = String(e); globalThis.__alldone = true; }});
            return "scheduled";
        }})()
        "#,
        port = port,
        pem = cert.replace('\\', "\\\\").replace('\n', "\\n").replace('\r', "\\r"),
    );
    let setup_out = eval_string(&mut ctx, &js);
    assert!(
        setup_out.contains("scheduled"),
        "https probe setup failed: {}",
        setup_out
    );

    // Drive the event loop until both requests settle.
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
        if eval_string(&mut ctx, "String(globalThis.__alldone === true)") == "true" {
            break;
        }
    }

    let fatal = eval_string(&mut ctx, "String(globalThis.__fatal)");
    assert_eq!(fatal, "undefined", "phase driver crashed: {}", fatal);
    let p1 = eval_string(&mut ctx, "String(globalThis.__r.p1)");
    let p2 = eval_string(&mut ctx, "String(globalThis.__r.p2)");
    assert_eq!(p1, "200:ok", "first https.request must round-trip: {}", p1);
    assert_eq!(p2, "200:ok", "second https.request must round-trip: {}", p2);

    // N requests = N fresh handshakes: each request head lands on its own
    // connection record. There is no pooled connection a second request
    // could ride — the exact property upstream's fix restores (its defect
    // let request 2 reuse request 1's approved connection and skip the
    // per-request identity check entirely).
    let rec1 = wait_record_for(&records, "/p1-first");
    let rec2 = wait_record_for(&records, "/p2-second");
    assert!(
        rec1.is_some() && rec2.is_some(),
        "both requests must reach the server on decrypted connections: p1={:?} p2={:?} all={:?}",
        rec1,
        rec2,
        records.lock().unwrap()
    );
    // No-reuse, stated directly: no single connection carries both request
    // heads (a keep-alive pool would put both on one record).
    let both_on_one = records
        .lock()
        .unwrap()
        .iter()
        .any(|r| r.contains("/p1-first") && r.contains("/p2-second"));
    assert!(
        !both_on_one,
        "requests must not share a connection: both heads landed on one TLS connection"
    );
    let served = records
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.contains("get /"))
        .count();
    assert!(
        served >= 2,
        "each https.request must be its own TLS handshake (found {} served connections)",
        served
    );

    // The JS Agent bookkeeping never engages: no pooled socket is ever
    // recorded, so no request can ride another request's approved
    // connection through `agent.sockets` either.
    let agent_sockets = eval_string(
        &mut ctx,
        "JSON.stringify(require('https').globalAgent.sockets)",
    );
    assert_eq!(
        agent_sockets, "{}",
        "globalAgent must stay un-pooled while requests succeed: {}",
        agent_sockets
    );

    eprintln!(
        "[PASS] semantic-port small #4: https.request = one fresh TLS handshake per request, no cross-request connection reuse (dc078c8bc4 N-A-HOST probe)"
    );

    // Mirror fetch_tls_init_e2e_tests exit strategy: park HTTPThread, force-exit.
    bun_http::http_thread::shutdown_for_exit();
    bun_runtime::shutdown_thread_sm();
    std::process::exit(0);
}
