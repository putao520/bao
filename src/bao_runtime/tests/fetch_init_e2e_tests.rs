// @trace TEST-ENG-FETCH-INIT [req:REQ-ENG-001 REQ-ENG-006] [level:e2e]
// fetch init-face batch 1 end-to-end (wiring of web_fetch_classes):
//   1. extended HTTP methods (PROPFIND) reach the wire request line,
//   2. URLSearchParams body serializes with the x-www-form-urlencoded
//      content-type default,
//   3. fetch(new Request(...)) round-trips url/method/headers/body,
//   4. FormData bodies serialize to multipart/form-data (upstream Bun
//      Blob.zig fromDOMFormData framing: WebKitFormBoundary, per-part
//      Content-Disposition/Content-Type, explicit/File/blob filename,
//      boundary uniqueness across requests),
//   5. unknown method tokens fail closed (no silent GET fallback).
//
// Wire-level: a local TCP capture server asserts the raw request bytes.
// Exit strategy mirrors fetch_headers_e2e_tests (parked HTTPThread is a
// non-daemon thread; force-exit sidesteps the mimalloc atexit double-free).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
use bun_runtime::timers;

type Captured = Arc<Mutex<Vec<Vec<u8>>>>;

/// True once `buf` holds a complete HTTP/1.1 request: full header block and,
/// when Content-Length is present, the full body.
fn request_complete(buf: &[u8]) -> bool {
    let Some(pos) = find_sub(buf, b"\r\n\r\n") else {
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

fn find_sub(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|w| w == needle)
}

/// Spin up a capture server (same contract as fetch_headers_e2e_tests).
fn start_capture_server() -> (u16, Captured) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let captured: Captured = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&captured);
    listener.set_nonblocking(true).ok();
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_millis(200)))
                        .ok();
                    stream.set_nonblocking(false).ok();
                    let mut buf: Vec<u8> = Vec::new();
                    let mut chunk = [0u8; 4096];
                    loop {
                        match stream.read(&mut chunk) {
                            Ok(0) => break,
                            Ok(n) => {
                                buf.extend_from_slice(&chunk[..n]);
                                if request_complete(&buf) {
                                    break;
                                }
                            }
                            Err(_) => break,
                        }
                    }
                    sink.lock().unwrap().push(buf);
                    let resp = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
                    let _ = stream.write_all(resp.as_bytes());
                    let _ = stream.flush();
                }
                Err(_) => std::thread::sleep(Duration::from_millis(2)),
            }
        }
    });
    (port, captured)
}

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(JsValue::Null) => "null".to_string(),
        Ok(JsValue::Undefined) => "undefined".to_string(),
        Ok(JsValue::Object(_)) => "[object]".to_string(),
        Err(e) => format!("ERROR:{}", e.message),
    }
}

/// Find the captured request whose request line contains `path`.
fn request_for(captured: &Captured, path: &str) -> String {
    let guard = captured.lock().unwrap();
    for req in guard.iter() {
        let text = String::from_utf8_lossy(req).to_lowercase();
        if text.contains(path) {
            return text;
        }
    }
    String::new()
}

fn assert_contains(haystack: &str, needle: &str, ctx_msg: &str) {
    assert!(
        haystack.contains(needle),
        "TEST-ENG-FETCH-INIT: {} — expected {:?} in captured request:\n{}",
        ctx_msg,
        needle,
        haystack
    );
}

/// Extract the multipart boundary from a captured (lowercased) request —
/// the token after `boundary=` up to the header-terminating CRLF.
fn extract_boundary(req: &str) -> String {
    let Some(pos) = req.find("boundary=") else {
        return String::new();
    };
    let rest = &req[pos + "boundary=".len()..];
    rest.split("\r\n").next().unwrap_or("").to_string()
}

#[test]
fn test_fetch_init_face_batch1_wire() {
    bun_core::output::init_test();
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();

    let (port, captured) = start_capture_server();
    std::thread::sleep(Duration::from_millis(50));

    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    // ── Sync assertions: full-class surface + fail-closed errors ──────────
    let surface = eval_string(
        &mut ctx,
        r#"
        (function() {
            var out = [];
            // Full Headers surface: append/forEach/entries/iteration.
            var h = new Headers();
            h.set("X-A", "1");
            h.append("X-A", "2");
            out.push(h.get("X-A") === "1, 2" ? "hdr-append-ok" : "hdr-append-bad:" + h.get("X-A"));
            var seen = [];
            h.forEach(function(v, k) { seen.push(k + "=" + v); });
            out.push(seen.length === 1 ? "hdr-foreach-ok" : "hdr-foreach-bad");
            var spread = [];
            for (var pair of h) spread.push(pair[0] + ":" + pair[1]);
            out.push(spread[0] === "x-a:1, 2" ? "hdr-iter-ok" : "hdr-iter-bad:" + spread.join("|"));
            // Request keeps extended methods (case-normalised), rejects unknown.
            out.push(new Request("http://example.com", { method: "propfind" }).method === "PROPFIND"
                ? "req-method-ext" : "req-method-ext-bad");
            var threw = null;
            try { new Request("http://example.com", { method: "BREW" }); } catch (e) { threw = e.message; }
            out.push(threw !== null ? "req-method-throw-ok" : "req-method-throw-bad");
            // FormData surface: WHATWG append/get/getAll/has/set + iteration.
            var fd = new FormData();
            fd.append("k1", "v1");
            fd.append("k1", "v1b");
            fd.append("k2", new Blob([new Uint8Array([1, 2])]), "explicit.bin");
            fd.set("k3", "v3");
            out.push(fd.get("k1") === "v1" && fd.getAll("k1").length === 2 ? "fd-get-ok" : "fd-get-bad");
            out.push(fd.has("k3") && !fd.has("nope") ? "fd-has-ok" : "fd-has-bad");
            var fdSeen = [];
            fd.forEach(function(v, k) { fdSeen.push(k); });
            out.push(fdSeen.join(",") === "k1,k1,k2,k3" ? "fd-foreach-ok" : "fd-foreach-bad:" + fdSeen.join(","));
            var fdPairs = [];
            for (var pair of fd) fdPairs.push(pair[0]);
            out.push(fdPairs.join(",") === "k1,k1,k2,k3" ? "fd-iter-ok" : "fd-iter-bad:" + fdPairs.join(","));
            // Request with a FormData body constructs (no fail-closed throw).
            var fdReqThrew = null;
            try { new Request("http://example.com", { method: "POST", body: new FormData() }); }
            catch (e) { fdReqThrew = e.message || String(e); }
            out.push(fdReqThrew === null ? "fd-request-ok" : "fd-request-bad:" + fdReqThrew);
            // Response with a FormData body still fails closed (send-path-only
            // serialization; a toString() fallback would corrupt the body).
            var fdRespThrew = null;
            try { new Response(new FormData()); }
            catch (e) { fdRespThrew = e.message || String(e); }
            out.push(fdRespThrew !== null ? "fd-response-throw-ok" : "fd-response-throw-bad");
            // fetch() unknown method fails closed synchronously.
            var fetchThrew = null;
            try { fetch("http://127.0.0.1:1/x", { method: "BREW" }); }
            catch (e) { fetchThrew = e.message || String(e); }
            out.push(fetchThrew !== null ? "fetch-method-throw-ok" : "fetch-method-throw-bad");
            return out.join("|");
        })()
        "#,
    );
    let expected_sync = [
        "hdr-append-ok",
        "hdr-foreach-ok",
        "hdr-iter-ok",
        "req-method-ext",
        "req-method-throw-ok",
        "fd-get-ok",
        "fd-has-ok",
        "fd-foreach-ok",
        "fd-iter-ok",
        "fd-request-ok",
        "fd-response-throw-ok",
        "fetch-method-throw-ok",
    ];
    for part in &expected_sync {
        assert!(
            surface.contains(part),
            "fetch init batch1 sync surface missing {}: got {}",
            part,
            surface
        );
    }

    // ── Wire assertions: extended method / USP body / Request roundtrip ───
    let js = format!(
        r#"
        (function() {{
            var base = "http://127.0.0.1:{port}";
            var done = 0, err = null;
            var usp = new URLSearchParams();
            usp.set("a", "1");
            usp.set("b", "hello world");
            var req = new Request(base + "/m-reqobj", {{
                method: "POST",
                headers: {{ "X-Req-Obj": "rov" }},
                body: "reqobj-body"
            }});
            var uspReq = new Request(base + "/m-usp2", {{
                method: "POST",
                body: usp
            }});
            // FormData bodies: text field + typed File + untyped Blob +
            // explicit-filename Blob. Covers both fetch entry paths.
            var fd = new FormData();
            fd.append("greeting", "hello form");
            fd.append("upload", new File(["FILE-CONTENT-123"], "upload.txt", {{ type: "text/plain" }}));
            fd.append("raw", new Blob([new Uint8Array([9, 8, 7])]));
            fd.append("named", new Blob(["named-bytes"], {{ type: "application/json" }}), "explicit.json");
            var fdReq = new Request(base + "/m-fd2", {{
                method: "POST",
                body: fd
            }});
            Promise.all([
                fetch(base + "/m-propfind", {{ method: "PROPFIND" }})
                    .then(function(r) {{ return r.text(); }}).then(function() {{ done++; }}),
                fetch(base + "/m-usp", {{ method: "POST", body: usp }})
                    .then(function(r) {{ return r.text(); }}).then(function() {{ done++; }}),
                fetch(req)
                    .then(function(r) {{ return r.text(); }}).then(function() {{ done++; }}),
                fetch(uspReq)
                    .then(function(r) {{ return r.text(); }}).then(function() {{ done++; }}),
                fetch(base + "/m-fd", {{ method: "POST", body: fd }})
                    .then(function(r) {{ return r.text(); }}).then(function() {{ done++; }}),
                fetch(fdReq)
                    .then(function(r) {{ return r.text(); }}).then(function() {{ done++; }})
            ]).then(function() {{ globalThis.__all_done = true; }})
            .catch(function(e) {{ err = (e && e.message) || String(e); globalThis.__all_done = true; }});
            globalThis.__done_count = function() {{ return done; }};
            globalThis.__err = function() {{ return err; }};
            return "scheduled";
        }})()
        "#
    );

    let setup_out = eval_string(&mut ctx, &js);
    assert!(
        setup_out.contains("scheduled"),
        "fetch setup failed: {}",
        setup_out
    );

    // Drive the event loop until all 4 fetches settle.
    let cx_raw = ctx.raw_cx();
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut tick = 0usize;
    while Instant::now() < deadline {
        unsafe {
            mozjs_sys::jsapi::js::RunJobs(cx_raw);
        }
        timers::with_event_loop(|loop_| {
            loop_.tick_without_idle(std::ptr::null_mut());
        });
        std::thread::sleep(Duration::from_millis(2));
        tick += 1;
        if tick % 20 == 0 {
            let status = eval_string(
                &mut ctx,
                r#"(globalThis.__all_done ? "DONE:" : "PEND:") + globalThis.__done_count()"#,
            );
            if status.starts_with("DONE:") {
                break;
            }
        }
    }

    let final_status = eval_string(
        &mut ctx,
        r#"globalThis.__all_done ? ("DONE:" + globalThis.__done_count()) : ("PENDING:" + globalThis.__done_count())"#,
    );
    assert!(
        final_status.starts_with("DONE:6"),
        "fetch promises did not all settle: {}",
        final_status
    );
    let err = eval_string(&mut ctx, r#"String(globalThis.__err())"#);
    assert!(
        err == "null",
        "one or more fetch() calls rejected: {}",
        err
    );

    // PROPFIND reaches the request line (was silently GET before the fix).
    // The " http" suffix disambiguates prefix paths (/m-usp vs /m-usp2).
    let req = request_for(&captured, "/m-propfind http");
    assert!(!req.is_empty(), "no captured request for /m-propfind");
    assert_contains(
        &req,
        "propfind /m-propfind",
        "/m-propfind extended method not on the request line",
    );

    // URLSearchParams body: serialized form + defaulted content-type.
    let req = request_for(&captured, "/m-usp http");
    assert!(!req.is_empty(), "no captured request for /m-usp");
    assert_contains(&req, "post /m-usp", "/m-usp method not POST");
    assert_contains(
        &req,
        "content-type: application/x-www-form-urlencoded;charset=utf-8",
        "/m-usp URLSearchParams content-type default missing",
    );
    assert_contains(&req, "a=1&b=hello+world", "/m-usp serialized body lost");

    // fetch(new Request(...)) round-trip: method + header + body.
    let req = request_for(&captured, "/m-reqobj");
    assert!(!req.is_empty(), "no captured request for /m-reqobj");
    assert_contains(&req, "post /m-reqobj", "/m-reqobj method not POST");
    assert_contains(&req, "x-req-obj: rov", "/m-reqobj Request header lost");
    assert_contains(&req, "reqobj-body", "/m-reqobj Request body lost");

    // fetch(new Request(..., { body: URLSearchParams })): eager serialization
    // in the Request constructor carries the content-type default through.
    let req = request_for(&captured, "/m-usp2 http");
    assert!(!req.is_empty(), "no captured request for /m-usp2");
    assert_contains(&req, "post /m-usp2", "/m-usp2 method not POST");
    assert_contains(
        &req,
        "content-type: application/x-www-form-urlencoded;charset=utf-8",
        "/m-usp2 Request USP content-type default missing",
    );
    assert_contains(&req, "a=1&b=hello+world", "/m-usp2 Request USP body lost");

    // ── FormData multipart wire assertions ────────────────────────────────
    // Both entry paths (fetch init.body and fetch(new Request(...))) must
    // produce upstream-Bun-shaped multipart/form-data.
    let fd_req = request_for(&captured, "/m-fd http");
    let fd_req2 = request_for(&captured, "/m-fd2 http");
    assert!(!fd_req.is_empty(), "no captured request for /m-fd");
    assert!(!fd_req2.is_empty(), "no captured request for /m-fd2");
    for (path, req) in [("/m-fd", &fd_req), ("/m-fd2", &fd_req2)] {
        assert_contains(req, &format!("post {}", path), &format!("{} method not POST", path));
        assert_contains(
            req,
            "content-type: multipart/form-data; boundary=----webkitformboundary",
            &format!("{} multipart content-type + WebKit boundary prefix missing", path),
        );
        // Text field.
        assert_contains(
            req,
            "content-disposition: form-data; name=\"greeting\"",
            &format!("{} text field Content-Disposition missing", path),
        );
        assert_contains(req, "hello form", &format!("{} text field value lost", path));
        // Typed File: File.name + Blob.type per part.
        assert_contains(
            req,
            "content-disposition: form-data; name=\"upload\"; filename=\"upload.txt\"",
            &format!("{} File filename missing", path),
        );
        assert_contains(
            req,
            "content-type: text/plain\r\n\r\nfile-content-123",
            &format!("{} File per-part content-type/value framing broken", path),
        );
        // Untyped Blob: filename "blob" + application/octet-stream default.
        assert_contains(
            req,
            "filename=\"blob\"",
            &format!("{} plain-Blob default filename missing", path),
        );
        assert_contains(
            req,
            "content-type: application/octet-stream\r\n\r\n\t",
            &format!("{} plain-Blob default content-type missing (before bytes 09 08 07)", path),
        );
        // Explicit filename wins over the blob default.
        assert_contains(
            req,
            "filename=\"explicit.json\"",
            &format!("{} explicit filename override missing", path),
        );
        assert_contains(
            req,
            "content-type: application/json\r\n\r\nnamed-bytes",
            &format!("{} typed-Blob content-type/value framing broken", path),
        );
        // Terminator: final --{boundary}--.
        let boundary = extract_boundary(req);
        assert!(
            !boundary.is_empty(),
            "{} boundary could not be extracted",
            path
        );
        assert!(
            req.contains(&format!("--{}--", boundary)),
            "{} multipart terminator missing",
            path
        );
        // Exactly one boundary parameter in the header (no duplicates).
        assert_eq!(
            req.matches("boundary=").count(),
            1,
            "{} duplicate boundary parameters",
            path
        );
    }
    // Boundary uniqueness across the two sends (fresh 128-bit generation).
    let b1 = extract_boundary(&fd_req);
    let b2 = extract_boundary(&fd_req2);
    assert_ne!(
        b1, b2,
        "multipart boundary repeated across requests (must be per-send random)"
    );
    assert_eq!(b1.len(), 22 + 32, "boundary must be prefix + 32 hex chars");

    eprintln!(
        "[PASS] TEST-ENG-FETCH-INIT e2e: PROPFIND wire + USP body + Request roundtrip + FormData multipart (both entry paths) + fail-closed errors"
    );

    // Mirror fetch_api_tests exit strategy: park HTTPThread, force-exit.
    bun_http::http_thread::shutdown_for_exit();
    bun_runtime::shutdown_thread_sm();
    std::process::exit(0);
}
