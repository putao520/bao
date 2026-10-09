// @trace TEST-ENG-FETCH-H [req:REQ-ENG-001 REQ-ENG-006] [level:e2e]
// BCE-20260814-FETCH-H end-to-end: fetch(url, init) must transmit init
// options to the wire. A local TCP server captures each raw request; every
// WHATWG init.headers form (record / sequence pairs / sequence records /
// Headers-like entries() iterator / this module's Headers class) targets a
// distinct path, plus POST bodies in string and Uint8Array form.
//
// Exit strategy mirrors fetch_api_tests: shutdown_for_exit + process::exit(0)
// (parked HTTPThread is a non-daemon thread; force-exit also sidesteps the
// mimalloc atexit double-free documented in fetch_e2e_tests.rs).

use std::time::{Duration, Instant};

use bao_engine::context::JsContext;
use bun_runtime::timers;
#[path = "common/mod.rs"]
mod common;

use common::eval_string_full as eval_string;

type Captured = common::capture_server::CapturedRequests;
use common::capture_server::{request_for, start_capture_server};


fn assert_contains(haystack: &str, needle: &str, ctx_msg: &str) {
    assert!(
        haystack.contains(needle),
        "BCE-20260814-FETCH-H: {} — expected {:?} in captured request:\n{}",
        ctx_msg,
        needle,
        haystack
    );
}

#[test]
fn test_fetch_init_headers_reach_wire_all_forms() {
    // Force-exit semantics preserved via self-re-exec isolation
    // (exit_isolation.rs): the body's shutdown_for_exit +
    // std::process::exit(0) is only legal in a process whose death IS
    // the test's success exit — running it in-harness would kill the
    // whole suite and park the shared HTTPThread for every later test.
    crate::exit_isolation::dispatch("fetch_headers_e2e_tests::test_fetch_init_headers_reach_wire_all_forms", test_fetch_init_headers_reach_wire_all_forms_body);
}

fn test_fetch_init_headers_reach_wire_all_forms_body() {

    bun_core::output::init_test();
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();

    let (port, captured) = start_capture_server();
    std::thread::sleep(Duration::from_millis(50));

    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    // Request constructor must also materialize init.headers (was dropped as
    // an always-empty object before BCE-20260814-FETCH-H). Asserted through
    // the WHATWG Headers#get — bracket access was an artifact of the old
    // simplified native Headers (own-prop storage), not a spec surface.
    let req_check = eval_string(
        &mut ctx,
        r#"
        (function() {
            var r = new Request("http://example.com/x", {
                method: "POST",
                headers: { "X-Req": "rv" }
            });
            return r.method + "|" + r.headers.get("X-Req");
        })()
        "#,
    );
    assert_eq!(
        req_check, "POST|rv",
        "Request constructor must materialize init.headers entries"
    );

    let js = format!(
        r#"
        (function() {{
            var base = "http://127.0.0.1:{port}";
            var done = 0, err = null;
            function go(p, init) {{
                return fetch(base + p, init)
                    .then(function(r) {{ return r.text(); }})
                    .then(function() {{ done++; }})
                    .catch(function(e) {{ err = (e && e.message) || String(e); done++; }});
            }}
            // Headers class instance (own-prop record form)
            var hClass = new Headers();
            hClass.set("X-Hdr-Class", "cls");
            // Headers-like with entries() — JS iterator protocol
            var hIter = {{
                entries: function() {{
                    var p = [["X-Iter-A", "ia"], ["X-Iter-B", "ib"]];
                    var i = 0;
                    return {{ next: function() {{
                        return i < p.length ? {{done: false, value: p[i++]}} : {{done: true, value: undefined}};
                    }} }};
                }}
            }};
            var u8 = new Uint8Array([104, 105, 45, 117, 56]); // "hi-u8"
            Promise.all([
                go("/h-obj",    {{ headers: {{ "X-Obj": "objv" }} }}),
                // Numeric-key record entries must reach the wire too
                // (integer-index keys were dropped by the old
                // string-only Form 3 enumeration).
                go("/h-num",    {{ headers: {{ 0: "numv", "x-a": "av" }} }}),
                go("/h-arr",    {{ headers: [["X-Arr", "arrv"]] }}),
                go("/h-arrrec", {{ headers: [{{ name: "X-Arrrec", value: "arrrecv" }}] }}),
                go("/h-iter",   {{ headers: hIter }}),
                go("/h-hdr",    {{ headers: hClass }}),
                go("/h-bodys",  {{ method: "POST", headers: {{ "X-Body-S": "bs" }}, body: "hello-body-str" }}),
                go("/h-bodyu",  {{ method: "POST", headers: {{ "X-Body-U": "bu" }}, body: u8 }}),
            ]).then(function() {{ globalThis.__all_done = true; }});
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

    // Drive the event loop until all 7 fetches settle (resolve or reject).
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
        final_status.starts_with("DONE:8"),
        "fetch promises did not all settle: {}",
        final_status
    );
    let err = eval_string(&mut ctx, r#"String(globalThis.__err())"#);
    assert!(
        err == "null",
        "one or more fetch() calls rejected: {}",
        err
    );

    // ── Assert every captured request carries its init options ──────────
    // record form
    let req = request_for(&captured, "/h-obj");
    assert!(!req.is_empty(), "no captured request for /h-obj");
    assert_contains(&req, "x-obj: objv", "/h-obj record-form header lost");

    // record form with a numeric key — the header named "0" must reach the
    // wire (ToString(0) === "0"; the old string-only enumeration dropped it).
    let req = request_for(&captured, "/h-num");
    assert!(!req.is_empty(), "no captured request for /h-num");
    assert_contains(&req, "0: numv", "/h-num numeric-key header lost");
    assert_contains(&req, "x-a: av", "/h-num string-key header lost");

    // sequence-of-pairs form
    let req = request_for(&captured, "/h-arr");
    assert!(!req.is_empty(), "no captured request for /h-arr");
    assert_contains(&req, "x-arr: arrv", "/h-arr sequence-pair header lost");

    // sequence-of-records form
    let req = request_for(&captured, "/h-arrrec");
    assert!(!req.is_empty(), "no captured request for /h-arrrec");
    assert_contains(
        &req,
        "x-arrrec: arrrecv",
        "/h-arrrec sequence-record header lost",
    );

    // Headers-like entries() iterator form
    let req = request_for(&captured, "/h-iter");
    assert!(!req.is_empty(), "no captured request for /h-iter");
    assert_contains(&req, "x-iter-a: ia", "/h-iter iterator header A lost");
    assert_contains(&req, "x-iter-b: ib", "/h-iter iterator header B lost");

    // this module's Headers class form
    let req = request_for(&captured, "/h-hdr");
    assert!(!req.is_empty(), "no captured request for /h-hdr");
    assert_contains(
        &req,
        "x-hdr-class: cls",
        "/h-hdr Headers-class header lost",
    );

    // POST + string body + headers
    let req = request_for(&captured, "/h-bodys");
    assert!(!req.is_empty(), "no captured request for /h-bodys");
    assert_contains(&req, "post /h-bodys", "/h-bodys method not POST");
    assert_contains(&req, "x-body-s: bs", "/h-bodys header lost");
    assert_contains(&req, "hello-body-str", "/h-bodys string body lost");

    // POST + Uint8Array body + headers
    let req = request_for(&captured, "/h-bodyu");
    assert!(!req.is_empty(), "no captured request for /h-bodyu");
    assert_contains(&req, "post /h-bodyu", "/h-bodyu method not POST");
    assert_contains(&req, "x-body-u: bu", "/h-bodyu header lost");
    assert_contains(&req, "hi-u8", "/h-bodyu Uint8Array body lost");

    eprintln!(
        "[PASS] BCE-20260814-FETCH-H e2e: all headers forms (record plain + numeric-key) + 2 POST body forms reached the wire"
    );

    // Mirror fetch_api_tests exit strategy: park HTTPThread, force-exit.
    bun_http::http_thread::shutdown_for_exit();
    bun_runtime::shutdown_thread_sm();
    std::process::exit(0);
}
