// Raw-request capture server family (e158b M3 consolidation of the e152
// audit §一 DUP-TEST-FIXTURES): the per-file inline copies of the
// request-framing helpers (`find_sub` ×5, `request_complete` ×6 — two
// formatting variants, identical semantics) and the identical
// `start_capture_server` + `request_for` pair that lived in
// fetch_headers_e2e_tests / fetch_init_e2e_tests.
//
// The `request_complete` arm of the slow/abort path-routed servers
// (fetch_abort_e2e_tests, abort_signal_timeout_tests) consumes the same
// helpers; their server bodies stay file-local (path-routing + delay +
// reset-counting semantics are per-file).

#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Raw request bytes per accepted connection.
pub type CapturedRequests = Arc<Mutex<Vec<Vec<u8>>>>;

pub fn find_sub(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// True once `buf` holds a complete HTTP/1.1 request: full header block and,
/// when Content-Length is present, the full body.
pub fn request_complete(buf: &[u8]) -> bool {
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

/// Spin up a capture server: every accepted connection's raw request bytes
/// are stored; a fixed 200 response is returned. Serves for 30s max.
pub fn start_capture_server() -> (u16, CapturedRequests) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let captured: CapturedRequests = Arc::new(Mutex::new(Vec::new()));
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

/// Find the captured request whose request line contains `path`.
pub fn request_for(captured: &CapturedRequests, path: &str) -> String {
    let guard = captured.lock().unwrap();
    for req in guard.iter() {
        let text = String::from_utf8_lossy(req).to_lowercase();
        if text.contains(path) {
            return text;
        }
    }
    String::new()
}
