// Plain-HTTP/1.1 recording fixture (e158b M3 consolidation of the e152
// audit §一 DUP-TEST-FIXTURES SW-family): the per-file inline accept-loop
// servers in sw_stealth_profile_tests (SwC19HttpFixture) /
// serviceworker_mediation_tests (SwMediationFixture) /
// serviceworker_fetchevent_tests (SwHttpFixture) /
// serviceworker_controller_tests (SwControllerFixture) /
// page_net_bun_fingerprint_e2e_tests (HttpFixture).
//
// The ~90-line accept/read/parse/respond skeleton was byte-identical across
// the five copies (nonblocking listener + 300ms read timeout + `\r\n\r\n`
// head read under a 2s deadline + first-line path extraction + canned
// `200` response with `Connection: close`); the only per-file variation is
// the route table — injected here as the [`RouteFn`] closure, a verbatim
// lift of each file's former if/else chain.
//
// Recorder semantics (reconciled, observationally identical for every
// consumer): `paths` records every request path unconditionally (the SW
// copies' semantic — page_net's `!path.is_empty()` guard only ever filtered
// malformed heads, and its path assertions are contains-based);
// `count` increments only on non-empty paths (page_net's semantic, its only
// consumer); the (path, sec-fetch-dest) `dests` log is always populated
// (the mediation e71 face; other files never read it).
//
// The `sw_script` slot (set later via [`HttpFixture::set_script`]) is fed
// to the router as `Option<&str>` — the three SW copies' templating hook.

#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Per-request router: `(path, current sw_script)` →
/// `(content_type, body)`. A verbatim lift of the former per-file
/// if/else chain.
pub type RouteFn = Arc<dyn Fn(&str, Option<&str>) -> (&'static str, String) + Send + Sync>;

pub struct HttpFixture {
    pub port: u16,
    shutdown: Arc<AtomicBool>,
    paths: Arc<Mutex<Vec<String>>>,
    dests: Arc<Mutex<Vec<(String, Option<String>)>>>,
    count: Arc<AtomicUsize>,
    sw_script: Arc<Mutex<Option<String>>>,
}

impl HttpFixture {
    /// Spawn the fixture under `name` (thread name + bind-expect label,
    /// keeping log attribution identical to the former per-file copies).
    pub fn spawn(name: &str, route: RouteFn) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .unwrap_or_else(|e| panic!("bind {name} fixture: {e}"));
        let port = listener.local_addr().unwrap().port();
        let _ = listener.set_nonblocking(true);
        let shutdown = Arc::new(AtomicBool::new(false));
        let paths: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let dests: Arc<Mutex<Vec<(String, Option<String>)>>> = Arc::new(Mutex::new(Vec::new()));
        let count = Arc::new(AtomicUsize::new(0));
        let sw_script: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let shutdown_c = Arc::clone(&shutdown);
        let paths_c = Arc::clone(&paths);
        let dests_c = Arc::clone(&dests);
        let count_c = Arc::clone(&count);
        let script_c = Arc::clone(&sw_script);
        let thread_name = name.to_string();
        std::thread::Builder::new()
            .name(thread_name)
            .spawn(move || {
                while !shutdown_c.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((mut tcp, _)) => {
                            let _ = tcp.set_nonblocking(false);
                            let _ = tcp.set_read_timeout(Some(Duration::from_millis(300)));
                            let mut buf = Vec::new();
                            let mut tmp = [0u8; 2048];
                            let deadline = Instant::now() + Duration::from_secs(2);
                            while buf.windows(4).position(|w| w == b"\r\n\r\n").is_none() &&
                                Instant::now() < deadline
                            {
                                match tcp.read(&mut tmp) {
                                    Ok(0) => break,
                                    Ok(n) => buf.extend_from_slice(&tmp[..n]),
                                    Err(_) => break,
                                }
                            }
                            let head = String::from_utf8_lossy(&buf).to_string();
                            let path = head
                                .lines()
                                .next()
                                .and_then(|line| line.split_whitespace().nth(1))
                                .unwrap_or("")
                                .to_string();
                            paths_c.lock().unwrap().push(path.clone());
                            if !path.is_empty() {
                                count_c.fetch_add(1, Ordering::SeqCst);
                            }
                            // sec-fetch-dest header (lowercased scan; header
                            // order/value casing varies across stacks) — the
                            // mediation e71 destination face.
                            let lower = head.to_lowercase();
                            let dest = lower.lines().find_map(|l| {
                                l.trim()
                                    .strip_prefix("sec-fetch-dest:")
                                    .map(|v| v.trim().to_string())
                            });
                            dests_c.lock().unwrap().push((path.clone(), dest));
                            let sw_script_body = script_c.lock().unwrap().clone();
                            let (content_type, body) =
                                route(&path, sw_script_body.as_deref());
                            let response = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n",
                                ct = content_type,
                                len = body.len()
                            );
                            let _ = tcp.write_all(response.as_bytes());
                            let _ = tcp.write_all(body.as_bytes());
                            let _ = tcp.shutdown(std::net::Shutdown::Both);
                        },
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        },
                        Err(_) => return,
                    }
                }
            })
            .unwrap_or_else(|e| panic!("spawn {name} fixture thread: {e}"));
        HttpFixture {
            port,
            shutdown,
            paths,
            dests,
            count,
            sw_script,
        }
    }

    /// Templated SW script slot consulted by the router on `/sw.js` hits.
    pub fn set_script(&self, script: String) {
        *self.sw_script.lock().unwrap() = Some(script);
    }

    pub fn recorded_paths(&self) -> Vec<String> {
        self.paths.lock().unwrap().clone()
    }

    /// `(path, sec-fetch-dest)` per request, in arrival order — the e71
    /// destination face reads the wire header the egress carried.
    pub fn recorded_dests(&self) -> Vec<(String, Option<String>)> {
        self.dests.lock().unwrap().clone()
    }

    pub fn count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }

    /// Poll until `n` non-empty-path requests arrived (or timeout).
    pub fn wait_for_count(&self, n: usize, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.count.load(Ordering::SeqCst) >= n {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    pub fn stop(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

impl Drop for HttpFixture {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}
