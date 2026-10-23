// @trace TEST-ENG-007-FETCH-ASYNC-C17 [req:REQ-ENG-007] [level:integration]
//
// C17 line-coverage wave: fetch_async.rs residual gaps (baseline gap 407).
// fetch_stream_tests + fetch_abort_e2e + abort_signal_timeout already cover
// the streaming machinery (chunk delivery, park/unpark, abort mid-body,
// clone refusal, RSS bounds); this file only adds the faces those assets do
// not touch:
//   1. the null-body-status arm (response_body_is_null: 204/205/304/HEAD →
//      Response.body === null, upstream bun 77afa71e9 form) — loopback
//      servers with real wire responses;
//   2. the connection-refused error arm (fetch to a closed port rejects);
//   3. the thread-wakeup bridge registration contract (same-pointer
//      idempotent re-register = documented multi-runtime shape).
//
// Oracle: node v24.19.0 local probes — 204 → r.body === null && r.text()
// === ""; HEAD 200 → r.body === null; fetch refused → TypeError.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

use bao_engine::context::JsContext;
#[path = "common/mod.rs"]
mod common;

use common::eval_string_full as eval_string;

/// One-shot response server: drains the request head, writes the canned
/// response, closes the socket (Connection: close termination so no
/// Content-Length bookkeeping is needed for bodiless statuses).
fn spawn_one_shot_server(status_line: &str, extra_headers: &str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let status = status_line.to_string();
    let extra = extra_headers.to_string();
    std::thread::spawn(move || {
        if let Ok((mut sock, _)) = listener.accept() {
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf);
            let resp = format!("HTTP/1.1 {}\r\nConnection: close\r\n{}\r\n", status, extra);
            let _ = sock.write_all(resp.as_bytes());
            let _ = sock.flush();
            // Socket dropped → FIN → client sees clean EOF.
        }
    });
    port
}

/// Settle-pump delegate: the unified suite pump (`common::pump_until`) with
/// the file's historical 2ms settle rhythm and "marker left PENDING" exit —
/// same three actuators per tick (realm-entered timer drain, MiniEventLoop
/// tick, RunJobs) plus the no-progress tripwire that turns a stalled settle
/// into a `PumpStuck` FAIL naming the frozen poll value instead of a silent
/// hang.
fn pump_until_settled(ctx: &mut JsContext, poll_js: &str, timeout: Duration) -> String {
    common::pump_until(ctx, poll_js, timeout, common::eval_string_full, |poll| {
        !poll.contains("PENDING")
    })
}

fn fresh_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

// ---------------------------------------------------------------------------
// 1. Null-body-status arm: 204/205/304 and the HEAD form all deliver
//    body === null (node v24 oracle: identical for every row).
// ---------------------------------------------------------------------------

#[test]
fn c17_fetch_null_body_status_family() {
    let cases: Vec<(&str, &str, &str)> = vec![
        ("204 No Content", "", "s204"),
        ("205 Reset Content", "", "s205"),
        ("304 Not Modified", "ETag: \"c17\"\r\n", "s304"),
    ];
    for (status, extra, tag) in cases {
        let port = spawn_one_shot_server(status, extra);
        let mut ctx = fresh_ctx();
        eval_string(
            &mut ctx,
            &format!(
                r#"
                globalThis.__r = {{ state: "PENDING" }};
                fetch("http://127.0.0.1:{port}/").then(function(r) {{
                    if (r.body === null) {{
                        __r.state = "nullbody:" + r.status;
                    }} else {{
                        return r.text().then(function(t) {{
                            __r.state = "hasbody:" + r.status + ":" + t.length;
                        }});
                    }}
                }}).catch(function(e) {{
                    __r.state = "REJECT:" + (e && e.name) + ":" + (e && e.message);
                }});
                'started'
                "#
            ),
        );
        let final_state = pump_until_settled(
            &mut ctx,
            "globalThis.__r.state",
            Duration::from_secs(20),
        );
        assert_eq!(
            final_state, format!("nullbody:{}", &status[..3]),
            "{tag}: expected null body for {status}"
        );
    }

    // HEAD: any response to a HEAD request has a null body regardless of
    // status (node oracle: head=200 bodynull=true).
    let port = spawn_one_shot_server("200 OK", "Content-Length: 5\r\n");
    let mut ctx = fresh_ctx();
    eval_string(
        &mut ctx,
        &format!(
            r#"
            globalThis.__h = {{ state: "PENDING" }};
            fetch("http://127.0.0.1:{port}/", {{ method: "HEAD" }}).then(function(r) {{
                if (r.body === null) {{
                    __h.state = "nullbody:" + r.status;
                }} else {{
                    return r.text().then(function(t) {{
                        __h.state = "hasbody:" + r.status + ":" + t.length;
                    }});
                }}
            }}).catch(function(e) {{
                __h.state = "REJECT:" + (e && e.name) + ":" + (e && e.message);
            }});
            'started'
            "#
        ),
    );
    let head_state = pump_until_settled(&mut ctx, "globalThis.__h.state", Duration::from_secs(20));
    assert_eq!(head_state, "nullbody:200", "HEAD response must carry a null body");

    // Control: a 200 GET with a body keeps a non-null body (same
    // response_body_is_null predicate, negative arm).
    let port = spawn_one_shot_server("200 OK", "Content-Length: 2\r\n\r\nhi");
    let mut ctx = fresh_ctx();
    eval_string(
        &mut ctx,
        &format!(
            r#"
            globalThis.__c = {{ state: "PENDING" }};
            fetch("http://127.0.0.1:{port}/").then(function(r) {{
                if (r.body === null) {{
                    __c.state = "nullbody:" + r.status;
                }} else {{
                    return r.text().then(function(t) {{
                        __c.state = "hasbody:" + r.status + ":" + t;
                    }});
                }}
            }}).catch(function(e) {{
                __c.state = "REJECT:" + (e && e.name) + ":" + (e && e.message);
            }});
            'started'
            "#
        ),
    );
    let ctl_state = pump_until_settled(&mut ctx, "globalThis.__c.state", Duration::from_secs(20));
    assert_eq!(ctl_state, "hasbody:200:hi", "200 GET must keep its body");
}

// ---------------------------------------------------------------------------
// 2. Connection-refused error arm: fetch to a closed port rejects (does
//    not hang, does not resolve). Node v24 oracle: TypeError ("fetch
//    failed" with an ECONNREFUSED cause). bao's exact error shape is
//    recorded in the transcript and asserted only as "rejected with an
//    Error" — the full parity pin belongs to the net layer wave.
// ---------------------------------------------------------------------------

#[test]
fn c17_fetch_conn_refused_error_arm() {
    // Bind-and-drop: a port that is deterministically not listening.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let mut ctx = fresh_ctx();
    eval_string(
        &mut ctx,
        &format!(
            r#"
            globalThis.__e = {{ state: "PENDING", detail: "" }};
            fetch("http://127.0.0.1:{port}/").then(function(r) {{
                __e.state = "RESOLVED:" + r.status;
            }}).catch(function(e) {{
                __e.state = "REJECTED";
                __e.detail = (e && e.name) + "|" + (e && e.message);
            }});
            'started'
            "#
        ),
    );
    let state = pump_until_settled(&mut ctx, "globalThis.__e.state", Duration::from_secs(20));
    assert_eq!(state, "REJECTED", "refused connection must reject, not resolve/hang");
    let detail = eval_string(&mut ctx, "globalThis.__e.detail");
    assert!(
        !detail.is_empty() && detail != "undefined|undefined",
        "rejection must carry an observable error shape, got: {detail:?}"
    );
}

// ---------------------------------------------------------------------------
// 3. Thread-wakeup bridge registration contract (Rust-direct): first
//    registration installs; re-registration with the SAME fn pointer is an
//    idempotent no-op (the documented N-runtime shape). Observable state is
//    unchanged (no panic, capture path still returns None on node-realm
//    threads).
// ---------------------------------------------------------------------------

#[test]
fn c17_fetch_async_bridge_idempotent_reregistration() {
    fn c17_wake_lookup_none() -> Option<bun_runtime::fetch_async::ThreadWakeup> {
        None
    }

    // First registration installs (Ok arm).
    bun_runtime::fetch_async::set_thread_wakeup_bridge(c17_wake_lookup_none);
    // Same-pointer re-registration: idempotent no-op (Err arm with
    // fn_addr_eq == installed → no diverged assertion).
    bun_runtime::fetch_async::set_thread_wakeup_bridge(c17_wake_lookup_none);
    bun_runtime::fetch_async::set_thread_wakeup_bridge(c17_wake_lookup_none);

    // The lookup returns None for this (node-realm) thread — identical
    // observable to the unset bridge state.
    assert!(
        !bun_runtime::fetch_async::has_pending(),
        "no fetch may be pending on a fresh thread"
    );
}
