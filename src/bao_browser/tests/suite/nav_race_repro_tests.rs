// e131 (REQ-BRW-002) — post-creation external navigate stale-Complete race.
//
// e124's registered out-of-scope observation: under dev-profile timing, an
// external navigate issued right after `create_page` occasionally "does not
// move" — `document.URL` still reads about:blank when the caller's load-wait
// returns (fingerprint_website_eval_e2e's honest skip carries the diagnostic;
// test-ci carrier basically unaffected).
//
// Attribution hypothesis (this file is the reproducer + regression pin):
//   hop-B  `PageInner::navigate` synchronously resets bao's `load_status`
//          copy to Started (the existing stale-Complete guard);
//   hop-S/C servo delivers the INITIAL about:blank load's Complete edge
//          AFTER that reset (create_page's `wait_for_pipeline_ready` returns
//          at first frame, which does not require the initial load to have
//          completed);
//   → the late Complete clobbers the reset, `get_state()` projects
//   `Interactive` while the URL is still about:blank, and every waiter that
//   keys on the state projection (`wait_for_load`, CDP settle polls) exits
//   before the new load even started.
//
// Vehicle: the e124 shape minimised — create_page(about:blank) → IMMEDIATE
// external navigate to a loopback origin. The race signature is observed at
// the product API level: `get_state()` reporting `Interactive`/`Idle` while
// `current_url()` is still the pre-navigation URL.
//
// Forensics: BAO_NAV_RACE_PROBE=1 enables hop-B/S/C eprintln probes (see
// page.rs navigate / servo.rs NotifyLoadStatusChanged handler /
// delegate.rs notify_load_status_changed) plus a per-spin state sample here.
// BAO_NAV_RACE_ITERS=N (default 25) bounds the create→navigate cycles.
//
// Probe mode (BAO_NAV_RACE_PROBE) reports hits without failing — the
// attribution vehicle. Without it, a single hit FAILS — the post-fix
// regression pin.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};

use bao_browser::{BaoConfig, BrowserRuntime, PageConfig, PageState, PageHandle};

fn pick_free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Loopback origin serving one instant HTML page at any path, plus the
/// slow-init carrier: `/slowinit` paints immediately (first frame early)
/// but its blocking subresource `/delay` holds the LOAD event back, so the
/// initial document's Complete edge lands ~600ms after creation — the
/// deterministic widening of the e124 statistical window (initial Complete
/// ~90ms after pipeline init, racing create's return).
fn spawn_origin() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { return };
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf);
            let req = String::from_utf8_lossy(&buf[..buf.len().min(2048)]).to_string();
            let path = req
                .split_whitespace()
                .nth(1)
                .unwrap_or("/")
                .to_string();
            if let Some(tail) = path.strip_prefix("/delay") {
                let delay_ms: u64 = tail
                    .split(['?', '&'])
                    .next()
                    .unwrap_or("600")
                    .parse()
                    .unwrap_or(600);
                std::thread::sleep(Duration::from_millis(delay_ms));
                let body = "//delayed";
                let _ = s.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/javascript\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .as_bytes(),
                );
                let _ = s.write_all(body.as_bytes());
                continue;
            }
            if let Some(tail) = path.strip_prefix("/slowinit") {
                let delay_ms: u64 = tail
                    .split(['?', '&'])
                    .next()
                    .unwrap_or("600")
                    .parse()
                    .unwrap_or(600);
                let body = format!(
                    "<html><head><title>nav-race-slowinit</title>\
                     <script src=\"/delay{delay_ms}\"></script></head>\
                     <body><div id=mark>slow</div></body></html>"
                );
                let _ = s.write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .as_bytes(),
                );
                let _ = s.write_all(body.as_bytes());
                continue;
            }
            if let Some(tail) = path.strip_prefix("/target") {
                // Slow target carrier: the e124 shape's slow external site —
                // the navigation's commit is held back so the initial load's
                // stale Complete can land in the malignant window (before the
                // commit instead of after it).
                let delay_ms: u64 = tail
                    .split(['?', '&'])
                    .next()
                    .unwrap_or("0")
                    .parse()
                    .unwrap_or(0);
                std::thread::sleep(Duration::from_millis(delay_ms));
            }
            let body = "<html><head><title>nav-race-target</title></head>\
                        <body><div id=mark>t</div></body></html>";
            let _ = s.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .as_bytes(),
            );
            let _ = s.write_all(body.as_bytes());
        }
    });
    port
}

#[test]
fn post_creation_navigate_stale_complete_race() {
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        eprintln!("[skip] no DISPLAY or WAYLAND_DISPLAY — servo requires a display server");
        return;
    }
    let port = spawn_origin();
    let runtime = match BrowserRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[skip] runtime init failed: {e}");
            return;
        }
    };
    let pool = runtime.page_pool();
    let iterations: usize = std::env::var("BAO_NAV_RACE_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(25);
    let probe = std::env::var_os("BAO_NAV_RACE_PROBE").is_some();
    // Slow target = the e124 shape's slow external site (sannysoft): the
    // initial about:blank load's Complete (~90ms after pipeline init) races
    // create's return; whenever the reset lands first, the stale Complete
    // projects Interactive before the slow target commits.
    let target = format!("http://127.0.0.1:{port}/target400");

    let mut race_hits: Vec<(usize, Duration, String)> = Vec::new();
    let mut never_moved: Vec<usize> = Vec::new();

    for i in 0..iterations {
        let page = match pool.create_page(&PageConfig {
            url: Some("about:blank".into()),
            ..Default::default()
        }) {
            Ok(p) => p,
            Err(e) => panic!("iteration {i}: create_page failed: {e}"),
        };

        // THE post-creation external navigate (e124 vehicle shape: as tight
        // against create's return as the call sequence allows).
        let t0 = Instant::now();
        if let Err(e) = page.navigate(&target) {
            panic!("iteration {i}: navigate failed: {e}");
        }

        // Drive + sample: each evaluate spin pumps servo's embedder messages,
        // so hop-S/C edges land between samples; the sampler observes the
        // state projection exactly as a state-keyed waiter would.
        let mut first_interactive: Option<(Duration, String)> = None;
        let mut moved_at: Option<Duration> = None;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let _ = page.evaluate_js_web(";");
            let state = page.get_state();
            let url = page.current_url().unwrap_or_default();
            if probe {
                let ls = page.webview_state().borrow().load_status;
                eprintln!(
                    "[nav-race sample] iter={i} +{:9.3}ms state={:?} ls={:?} url={url}",
                    t0.elapsed().as_secs_f64() * 1000.0,
                    state,
                    ls
                );
            }
            if first_interactive.is_none()
                && matches!(state, PageState::Interactive | PageState::Idle)
            {
                first_interactive = Some((t0.elapsed(), url.clone()));
            }
            if moved_at.is_none() && url.contains("127.0.0.1") {
                moved_at = Some(t0.elapsed());
            }
            if first_interactive.is_some() && moved_at.is_some() {
                break;
            }
        }

        match (&first_interactive, moved_at) {
            (Some((at, url)), Some(_)) => {
                // Interactive observed while the URL had not moved off the
                // initial document = the stale-Complete early-exit signature.
                if !url.contains("127.0.0.1") {
                    race_hits.push((i, *at, url.clone()));
                    if probe {
                        eprintln!(
                            "[nav-race HIT] iter={i}: get_state()=Interactive at +{:.3}ms while url={url:?}",
                            at.as_secs_f64() * 1000.0
                        );
                    }
                }
            }
            (Some(_), None) => never_moved.push(i),
            (None, _) => {
                // Neither Interactive nor URL movement within 10s — surface as
                // a hard anomaly in both modes (a lost load is candidate M2).
                panic!("iteration {i}: neither URL movement nor Interactive within 10s");
            }
        }
        let _ = page.close();
    }

    eprintln!(
        "[nav-race] iters={iterations} hits={} never_moved={}",
        race_hits.len(),
        never_moved.len()
    );
    for (i, at, url) in &race_hits {
        eprintln!(
            "[nav-race hit-detail] iter={i} first_interactive_at=+{:.3}ms url_then={url:?}",
            at.as_secs_f64() * 1000.0
        );
    }
    if !never_moved.is_empty() {
        panic!(
            "post-creation navigate never moved in iters {never_moved:?} (lost-load class, M2)"
        );
    }
    if !probe {
        assert!(
            race_hits.is_empty(),
            "stale-Complete race reproduced {}/{} iterations — get_state() reported Interactive \
             while the URL had not moved (first hit: iter={}, +{:.3}ms, url={:?})",
            race_hits.len(),
            iterations,
            race_hits[0].0,
            race_hits[0].1.as_secs_f64() * 1000.0,
            race_hits[0].2
        );
    }
}

/// Same race, observed the way the e124 vehicle actually observed it: the
/// state-keyed load wait returns early, the caller immediately evaluates
/// `document.URL` and reads the pre-navigation document. This pins the
/// USER-VISIBLE symptom (skip diagnostic "page did not fully load
/// [document.URL=about:blank ...]"), not just the internal projection.
#[test]
fn post_creation_navigate_wait_for_load_early_exit() {
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        eprintln!("[skip] no DISPLAY or WAYLAND_DISPLAY — servo requires a display server");
        return;
    }
    let port = spawn_origin();
    let runtime = match BrowserRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[skip] runtime init failed: {e}");
            return;
        }
    };
    let pool = runtime.page_pool();
    let iterations: usize = std::env::var("BAO_NAV_RACE_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(25);
    let probe = std::env::var_os("BAO_NAV_RACE_PROBE").is_some();
    let target = format!("http://127.0.0.1:{port}/target400");

    let mut early_exits = 0usize;
    let mut dead_loads = 0usize;
    for i in 0..iterations {
        let page = pool
            .create_page(&PageConfig {
                url: Some("about:blank".into()),
                ..Default::default()
            })
            .expect("create_page");
        page.navigate(&target).expect("navigate");

        // wait_for_load (fingerprint_website_eval_e2e:108) verbatim shape:
        // exits on the FIRST Interactive projection.
        let started = Instant::now();
        let mut wait_ms: Option<u128> = None;
        while started.elapsed().as_millis() < 15_000 {
            let _ = page.evaluate_js_web("");
            if matches!(page.get_state(), PageState::Interactive | PageState::Idle) {
                wait_ms = Some(started.elapsed().as_millis());
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        let url_now = page_url_now(&page);
        // The symptom: the state-keyed wait returned while the navigation had
        // not committed — document.URL is still the initial document. A wait
        // that TIMED OUT (wait_ms None) is a different, load-liveness failure
        // (the navigation produced no edges at all — the registered
        // load-dependent constellation flake, not this race) and is reported
        // separately so the two failure classes never masquerade as each
        // other.
        match (wait_ms, url_now.as_str()) {
            (Some(ms), "about:blank") => {
                early_exits += 1;
                eprintln!(
                    "[nav-race wait-symptom] iter={i}: wait_for_load returned after {ms}ms with document.URL={url_now:?}"
                );
            }
            (None, _) => {
                dead_loads += 1;
                eprintln!(
                    "[nav-race wait-symptom] iter={i}: wait_for_load TIMED OUT (15s) — \
                     load-liveness flake, not the stale-Complete race"
                );
            }
            _ => {}
        }
        let _ = page.close();
    }
    eprintln!(
        "[nav-race wait-symptom] iters={iterations} early_exits={early_exits} dead_loads={dead_loads}"
    );
    if !probe {
        assert_eq!(
            early_exits, 0,
            "state-keyed load wait exited pre-commit {early_exits}/{iterations} times"
        );
    }
    // Load-liveness failures are the registered load-dependent constellation
    // flake (see post_creation_navigate_fast_target_load_liveness_probe) —
    // reported, never silently folded into the race count.
    assert_eq!(
        dead_loads, 0,
        "post-creation loads died with zero edges {dead_loads}/{iterations} times —          load-liveness flake, not the stale-Complete race"
    );
}

fn page_url_now(page: &PageHandle) -> String {
    page.evaluate_js_web("String(document.URL)")
        .unwrap_or_default()
}

/// Deterministic widening of the same race: the initial document's Complete
/// edge is held back ~600ms by a blocking subresource (first frame early —
/// `create_page` still returns at frame_ready), so the post-creation
/// external navigate ALWAYS resets while the initial load's Complete edge is
/// still in flight. Attribution: with the stale edge unguarded, the state
/// projection must report `Interactive` before the navigation commits; the
/// hop timeline (BAO_NAV_RACE_PROBE=1) shows hopS/hopC Complete landing
/// after hopB's reset.
#[test]
fn post_creation_navigate_slowinit_stale_complete_deterministic() {
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        eprintln!("[skip] no DISPLAY or WAYLAND_DISPLAY — servo requires a display server");
        return;
    }
    let port = spawn_origin();
    let runtime = match BrowserRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[skip] runtime init failed: {e}");
            return;
        }
    };
    let pool = runtime.page_pool();
    let probe = std::env::var_os("BAO_NAV_RACE_PROBE").is_some();
    let target = format!("http://127.0.0.1:{port}/target400");
    let delays_ms: [u64; 5] = [20, 40, 80, 160, 320];

    let mut race_hits = 0usize;
    for (i, &delay_ms) in delays_ms.iter().enumerate() {
        let page = match pool.create_page(&PageConfig {
            url: Some(format!("http://127.0.0.1:{port}/slowinit{delay_ms}")),
            ..Default::default()
        }) {
            Ok(p) => p,
            Err(e) => panic!("iteration {i}: create_page failed: {e}"),
        };
        let t0 = Instant::now();
        let nav_epoch_ms = epoch_ms_now();
        if probe {
            eprintln!(
                "[nav-race iter-start] iter={i} delay={delay_ms}ms navigate_epoch={nav_epoch_ms}"
            );
        }
        page.navigate(&target).expect("navigate");

        // Sample exactly like a state-keyed waiter: pump + project.
        let mut hit_at: Option<Duration> = None;
        let mut moved_at: Option<Duration> = None;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let _ = page.evaluate_js_web(";");
            let state = page.get_state();
            let url = page.current_url().unwrap_or_default();
            if hit_at.is_none()
                && matches!(state, PageState::Interactive | PageState::Idle)
                && !url.contains("/t")
            {
                hit_at = Some(t0.elapsed());
                if probe {
                    eprintln!(
                        "[nav-race HIT] iter={i}: Interactive at +{:.3}ms while url={url:?}",
                        t0.elapsed().as_secs_f64() * 1000.0
                    );
                }
            }
            if moved_at.is_none() && url.contains("/t") {
                moved_at = Some(t0.elapsed());
            }
            if hit_at.is_some() && moved_at.is_some() {
                break;
            }
        }
        if hit_at.is_some() {
            race_hits += 1;
        }
        let moved = moved_at
            .map(|t| format!("+{:.3}ms", t.as_secs_f64() * 1000.0))
            .unwrap_or_else(|| "NEVER".into());
        eprintln!(
            "[nav-race slowinit] iter={i} delay={delay_ms}ms hit={} url_moved={moved}",
            hit_at.is_some()
        );
        assert!(
            moved_at.is_some(),
            "iteration {i}: navigation never committed within 10s (lost-load class)"
        );
        let _ = page.close();
    }
    if !probe {
        assert_eq!(
            race_hits, 0,
            "stale-Complete race hit {race_hits}/{} deterministic iterations",
            delays_ms.len()
        );
    }
}

/// e131 adjacent finding probe ("Face B" — the load genuinely dying): under
/// in-suite load the post-creation navigate occasionally produces ZERO
/// load-status edges for the new load (no Started/HeadParsed/Complete — the
/// navigation never runs), observable pre-fix as title-assert failures with
/// a falsely-Interactive state (masked by the stale-Complete bug this file's
/// fix roots out). This probe drives the same create→navigate shape with a
/// FAST target N times on a quiet machine: if loads die here too, Face B is
/// solo-reproducible (constellation-class); if not, it is load-dependent.
/// Honest report only — no assert on the rate (registered finding).
#[test]
fn post_creation_navigate_fast_target_load_liveness_probe() {
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        eprintln!("[skip] no DISPLAY or WAYLAND_DISPLAY — servo requires a display server");
        return;
    }
    let port = spawn_origin();
    let runtime = match BrowserRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[skip] runtime init failed: {e}");
            return;
        }
    };
    let pool = runtime.page_pool();
    let iterations: usize = std::env::var("BAO_NAV_RACE_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(50);
    let target = format!("http://127.0.0.1:{port}/t");

    let mut dead_loads = 0usize;
    for i in 0..iterations {
        let page = pool
            .create_page(&PageConfig {
                url: Some("about:blank".into()),
                ..Default::default()
            })
            .expect("create_page");
        page.navigate(&target).expect("navigate");
        let mut moved = false;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let _ = page.evaluate_js_web(";");
            if page.current_url().unwrap_or_default().contains("/t") {
                moved = true;
                break;
            }
        }
        if !moved {
            dead_loads += 1;
            eprintln!("[nav-race faceB] iter={i}: FAST target load DEAD within 5s");
        }
        let _ = page.close();
    }
    eprintln!(
        "[nav-race faceB] iters={iterations} dead_loads={dead_loads} (quiet-machine probe)"
    );
    assert_eq!(
        dead_loads, 0,
        "fast-target post-creation loads died {dead_loads}/{iterations} on a QUIET machine — \
         Face B is solo-reproducible (constellation-class), not load-only"
    );
}

fn epoch_ms_now() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

/// e131 fix regression pin: the generation gate must never STARVE a
/// navigation. The hazard: the new load's `Started` credits the generation —
/// if servo's webview-side same-value dedupe (removed in the same fix) were
/// still active, a re-navigation issued while the WebView copy still reads
/// `Started` (mid-load re-navigate) would never credit, and the new load's
/// real `Complete` would be dropped as "stale" — the page stuck Navigating
/// forever (strictly worse than the race being fixed). This test issues a
/// second navigation while the first load is still in flight and requires
/// the second load to reach Interactive with its URL committed.
#[test]
fn post_creation_navigate_midload_renavigate_not_starved() {
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        eprintln!("[skip] no DISPLAY or WAYLAND_DISPLAY — servo requires a display server");
        return;
    }
    let port = spawn_origin();
    let runtime = match BrowserRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[skip] runtime init failed: {e}");
            return;
        }
    };
    let pool = runtime.page_pool();
    let probe = std::env::var_os("BAO_NAV_RACE_PROBE").is_some();

    for round in 0..3usize {
        let page = pool
            .create_page(&PageConfig {
                url: Some("about:blank".into()),
                ..Default::default()
            })
            .expect("create_page");

        // Navigation A: doc-init fast (its `Started` edge is delivered and
        // credited), load event held back ~300ms by the blocking
        // subresource — a genuinely mid-load document when B is issued.
        // (NOT a back-to-back double navigate: two LoadUrls in the same
        // event-loop tick hit servo's constellation `pending_changes`
        // semantics — "a pending page will not be overridden" — which drops
        // the second load wholesale; that is pre-existing constellation
        // behavior outside this race's scope.)
        page.navigate(&format!("http://127.0.0.1:{port}/slowinit300"))
            .expect("navigate A");
        let a_url = format!("http://127.0.0.1:{port}/slowinit300");
        let deadline_a = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline_a {
            let _ = page.evaluate_js_web(";");
            if page.current_url().unwrap_or_default() == a_url {
                break;
            }
        }
        assert!(
            page.current_url().unwrap_or_default() == a_url,
            "round {round}: navigation A never committed"
        );
        // A is now mid-load: `Started` delivered (doc initialized), its
        // `Complete` still pending (~300ms out). Navigation B: fast page.
        let t0 = Instant::now();
        page.navigate(&format!("http://127.0.0.1:{port}/t"))
            .expect("navigate B");

        // Drive until B's marker document is Interactive — the
        // not-starved exit. B's URL is /t (path without /target prefix).
        let mut interactive_at: Option<Duration> = None;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let _ = page.evaluate_js_web(";");
            let url = page.current_url().unwrap_or_default();
            let state = page.get_state();
            if probe {
                eprintln!(
                    "[nav-race midload sample] round={round} +{:9.3}ms state={state:?} url={url}",
                    t0.elapsed().as_secs_f64() * 1000.0
                );
            }
            let url_is_b = url.ends_with("/t");
            if url_is_b && matches!(state, PageState::Interactive | PageState::Idle) {
                interactive_at = Some(t0.elapsed());
                break;
            }
        }
        assert!(
            interactive_at.is_some(),
            "round {round}: mid-load re-navigate starved — B never reached Interactive on /t \
             within 10s (generation gate credit broken; see servo.rs/webview.rs dedupe note)"
        );
        eprintln!(
            "[nav-race midload] round={round}: B Interactive at +{:.3}ms",
            interactive_at.unwrap().as_secs_f64() * 1000.0
        );
        let _ = page.close();
    }
}
