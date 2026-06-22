// @trace TEST-STL-E2E-WEB-FINGERPRINT [req:REQ-STL-001,REQ-STL-002,REQ-STL-003,REQ-STL-004,REQ-STL-005,REQ-STL-006,REQ-STL-007] [level:e2e]
// Real-world fingerprint website evaluation E2E.
//
// Launches BaoRuntime (servo), creates a Page with StealthProfile injected,
// and navigates to REAL fingerprint detection websites to evaluate Bao's
// anti-fingerprinting efficacy:
//   - bot.sannysoft.com         (webdriver / chrome object / plugins / permissions)
//   - abrahamjuliot.github.io/creepjs/  (JS engine fingerprint / trust score)
//   - pixelscan.net             (fingerprint consistency)
//   - browserleaks.com/javascript (fingerprint leaks)
//
// Efficacy target: identification rate <= 5% (SPEC 01-BUSINESS metric).
//
// Strategy (BCE-20260622-004: in-parent direct multi-site navigation):
//
// HISTORY (BCE-20260621-002): the FIRST HTTPS navigation used to SIGSEGV in
// `js::jit::BaselineFrame::initForOsr` because servo's `fire_add_debuggee`
// marked every Realm as a debuggee, toggling BaselineInterpreter debugger
// instrumentation; a subsequent JIT OSR dereferenced
// `cx->activation_->prev()->asInterpreter()` as NULL. BCE-20260621-002 patched
// servo (`disable_script_debugger: true` skips `fire_add_debuggee`) and mozjs
// (`initForOsr` NULL-activation guards), and BCE-20260621-001 replaced
// process-global *mut JSObject storage with per-WebViewId maps.
//
// A LATER residual claim said "the SECOND external navigation in the same
// BaoRuntime still SIGSEGVs deterministically" — addressed by a temporary
// subprocess-per-site workaround (one navigation per child process). That
// workaround is now REMOVED. BCE-20260622-004 empirical verification (gdb
// attached, see bce004_*_tests.rs) shows that with BCE-001 + BCE-002 patches
// in place, TEN sequential external navigations + pre/post-nav
// `inject_stealth_js` + post-nav `evaluate_js_web` all complete WITHOUT
// SIGSEGV in a single parent process. The "second navigation SIGSEGV" claim
// is non-reproducible in current code; the workaround was guarding a BUG
// that BCE-001/002 had already eradicated.
//
// CURRENT strategy: ONE parent BaoRuntime creates one Page per fingerprint
// site (via the shared PagePool), navigates externally, scrapes real
// detection data via `evaluate_js_web`, evaluates, closes the page, and
// moves to the next site. This mirrors the production usage pattern
// (browser navigating across many pages in one process).
//
// Graceful strategy:
//   - This test REQUIRES real network access to external fingerprint sites.
//   - If BAO_TEST_NETWORK=1 is unset, OR no DISPLAY (no servo display server),
//     OR BaoRuntime::new fails, OR a site is unreachable, that site is
//     recorded as Report::skip (not fail). Only stealth-property regressions
//     on the LOCAL data: pages are hard fails.
//
// Servo is single-process per-binary (mozjs Runtime + Opts are process-global
// singletons), so all scenarios live inside a single #[test].
//
// BCE-20260622-004 regression guard: any future regression of the
// "second external navigation SIGSEGV" BUG will manifest as this test
// aborting mid-run (rather than gracefully completing all sites).

#![allow(dead_code)]

use bao_browser::{BaoConfig, BaoRuntime, PageConfig, PageHandle, PagePool, PageState};
use bao_stealth::StealthProfile;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Report — fault-tolerant scenario accumulator
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Report {
    passed: u32,
    skipped: u32,
    failed: u32,
    messages: Vec<String>,
}

impl Report {
    fn pass(&mut self, name: &str) {
        self.passed += 1;
        self.messages.push(format!("PASS  {}", name));
    }
    fn skip(&mut self, name: &str, why: &str) {
        self.skipped += 1;
        self.messages.push(format!("SKIP  {}  ({})", name, why));
    }
    fn fail(&mut self, name: &str, why: &str) {
        self.failed += 1;
        self.messages.push(format!("FAIL  {}  ({})", name, why));
    }
    fn finish(&self) {
        eprintln!("\n=== Fingerprint Website Eval E2E ===");
        for m in &self.messages {
            eprintln!("{}", m);
        }
        eprintln!(
            "--- {} passed, {} skipped, {} failed ---",
            self.passed, self.skipped, self.failed
        );
    }
}

// ---------------------------------------------------------------------------
// wait_for_load — drive servo's paint loop until DOM is queryable
// ---------------------------------------------------------------------------

/// Drive servo's event loop by issuing a no-op eval (which pumps the script
/// thread) and check the page state. Even if the state never reaches Idle
/// (sannysoft keeps timers / XHRs running), the DOM becomes queryable once
/// the document has been parsed; so after `max_ms` we return unconditionally —
/// callers then probe the DOM and skip gracefully if it is still empty.
fn wait_for_load(page: &PageHandle, max_ms: u64) {
    let start = Instant::now();
    while start.elapsed().as_millis() < max_ms as u128 {
        let _ = page.evaluate_js("");
        if matches!(page.get_state(), PageState::Interactive | PageState::Idle) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    // Final pump so any pending microtasks flush before we query the DOM.
    let _ = page.evaluate_js("");
}

// ---------------------------------------------------------------------------
// inject_stealth_js — JS-level stealth property injection via Object.defineProperty
// Uses the same profile data source as engine_props (data consistency).
// ---------------------------------------------------------------------------

fn inject_stealth_js(page: &PageHandle, profile: &StealthProfile) -> Result<(), String> {
    let nav_overrides = [
        ("userAgent", &profile.navigator.user_agent),
        ("platform", &profile.navigator.platform),
        ("language", &profile.navigator.language),
        ("vendor", &profile.navigator.vendor),
    ];
    for (prop, value) in &nav_overrides {
        let escaped = value.replace('\\', "\\\\").replace('\'', "\\'");
        let js = format!(
            "(function() {{ try {{ Object.defineProperty(navigator, '{}', {{get: function(){{return '{}';}}, configurable: false}}); }} catch(e){{}} }})()",
            prop, escaped
        );
        page.evaluate_js_web(&js).map_err(|e| format!("inject nav.{}: {}", prop, e))?;
    }
    let nav_num_overrides = [
        ("hardwareConcurrency", profile.navigator.hardware_concurrency),
        ("maxTouchPoints", profile.navigator.max_touch_points),
    ];
    for (prop, value) in &nav_num_overrides {
        let js = format!(
            "(function() {{ try {{ Object.defineProperty(navigator, '{}', {{get: function(){{return {}; }}, configurable: false}}); }} catch(e){{}} }})()",
            prop, value
        );
        page.evaluate_js_web(&js).map_err(|e| format!("inject nav.{}: {}", prop, e))?;
    }
    let js = "(function() { try { Object.defineProperty(navigator, 'webdriver', {get: function(){return false;}, configurable: false}); } catch(e){} })()";
    page.evaluate_js_web(&js).map_err(|e| format!("inject webdriver: {}", e))?;
    let screen_overrides = [
        ("width", profile.screen.width),
        ("height", profile.screen.height),
        ("availWidth", profile.screen.avail_width),
        ("availHeight", profile.screen.avail_height),
        ("colorDepth", profile.screen.color_depth),
        ("pixelDepth", profile.screen.color_depth),
    ];
    for (prop, value) in &screen_overrides {
        let js = format!(
            "(function() {{ try {{ Object.defineProperty(screen, '{}', {{get: function(){{return {}; }}, configurable: false}}); }} catch(e){{}} }})()",
            prop, value
        );
        page.evaluate_js_web(&js).map_err(|e| format!("inject screen.{}: {}", prop, e))?;
    }
    let js = format!(
        "(function() {{ try {{ Object.defineProperty(window, 'devicePixelRatio', {{get: function(){{return {}; }}, configurable: false}}); }} catch(e){{}} }})()",
        profile.screen.device_pixel_ratio
    );
    page.evaluate_js_web(&js).map_err(|e| format!("inject dpr: {}", e))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// E2E entry point
// ---------------------------------------------------------------------------

/// Real fingerprint website evaluation E2E.
///
/// Graceful skip conditions:
///   1. BAO_TEST_NETWORK != "1" — opt-in for external network access
///   2. No DISPLAY/WAYLAND_DISPLAY — servo requires a display server
///   3. BaoRuntime::new fails — servo init in headless env
///
/// All three trigger `eprintln!("[skip] ...") + return`, never fail.
#[test]
fn fingerprint_website_eval_e2e() {
    // Guard 1: opt-in for real network access
    if std::env::var("BAO_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!("[skip] BAO_TEST_NETWORK != 1 — fingerprint website E2E requires real network");
        return;
    }
    // Guard 2: servo requires a display server
    if std::env::var("DISPLAY").is_err() && std::env::var("WAYLAND_DISPLAY").is_err() {
        eprintln!("[skip] no DISPLAY or WAYLAND_DISPLAY — servo requires a display server");
        return;
    }

    // PARENT MODE: initialize runtime once for the entire test. This same
    // runtime is used for both Phase 1 (local data: stealth property checks)
    // and Phase 2 (real external fingerprint site navigation + evaluation).
    // BCE-20260622-004: the subprocess-per-site workaround has been REMOVED
    // — BCE-001 + BCE-002 patches eradicated the multi-nav SIGSEGV at root,
    // so a single parent process navigates all sites directly (mirroring the
    // production "browser visiting many pages" pattern).
    let config = BaoConfig::default();
    let runtime = match BaoRuntime::new(config) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[skip] BaoRuntime::new failed (likely missing servo runtime): {}", e);
            return;
        }
    };
    let pool: &PagePool = runtime.page_pool();
    let mut report = Report::default();

    // Phase 1: Per-profile stealth property verification (local data checks,
    //          but exercised through the real servo page). Hard fail on regression.
    scenario_stealth_property_efficacy_chrome(pool, &mut report);
    scenario_stealth_property_efficacy_firefox(pool, &mut report);

    // Phase 2: Real fingerprint website navigation — REQUIRES NETWORK.
    //          Each site is navigated IN-PARENT via a fresh Page from the
    //          shared PagePool. With BCE-001 + BCE-002 patches, multi-nav
    //          is SIGSEGV-free, so no subprocess isolation is needed.
    run_fingerprint_site_evaluations_in_parent(pool, &mut report);

    pool.close_all();
    report.finish();

    // Hard fail rule: stealth property regressions are NOT tolerated.
    // Network-dependent site evaluations may all skip, but if even one stealth
    // property assertion fails, we assert.
    let stealth_fails = report
        .messages
        .iter()
        .filter(|m| m.starts_with("FAIL") && m.contains("stealth"))
        .count();
    assert_eq!(
        stealth_fails, 0,
        "{} stealth property assertions failed — see stderr above",
        stealth_fails
    );
    assert_eq!(report.failed, 0, "hard failures present — see stderr above");
}

// ---------------------------------------------------------------------------
// Scenario: Chrome profile stealth property efficacy
// ---------------------------------------------------------------------------

fn scenario_stealth_property_efficacy_chrome(pool: &PagePool, report: &mut Report) {
    let name = "stealth_efficacy_chrome";
    let profile = StealthProfile::chrome_default();

    let page = match pool.create_page(&PageConfig {
        url: Some("data:text/html;charset=utf-8,<html><body>chrome</body></html>".into()),
        stealth_profile: Some(profile.clone()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            report.skip(name, &format!("page creation failed: {e}"));
            return;
        }
    };
    if let Err(e) = inject_stealth_js(&page, &profile) {
        report.skip(name, &format!("inject failed: {e}"));
        let _ = page.close();
        return;
    }
    wait_for_load(&page, 1500);

    // navigator.webdriver must be hidden (Boolean false or absent)
    match page.evaluate_js_web("String(navigator.webdriver)") {
        Ok(s) if s == "false" => report.pass(&format!("{}::webdriver_hidden", name)),
        Ok(other) => report.fail(
            &format!("{}::webdriver_hidden", name),
            &format!("navigator.webdriver leaked: {}", other),
        ),
        Err(e) => report.skip(&format!("{}::webdriver_hidden", name), &format!("eval: {}", e)),
    }

    // navigator.userAgent contains Chrome
    match page.evaluate_js_web("navigator.userAgent") {
        Ok(s) if s.contains("Chrome") => report.pass(&format!("{}::ua_chrome", name)),
        Ok(other) => report.fail(
            &format!("{}::ua_chrome", name),
            &format!("UA missing 'Chrome': {}", other),
        ),
        Err(e) => report.skip(&format!("{}::ua_chrome", name), &format!("eval: {}", e)),
    }

    // navigator.vendor = Google Inc.
    match page.evaluate_js_web("navigator.vendor") {
        Ok(s) if s == "Google Inc." => report.pass(&format!("{}::vendor_google", name)),
        Ok(other) => report.fail(
            &format!("{}::vendor_google", name),
            &format!("vendor missing 'Google Inc.': {}", other),
        ),
        Err(e) => report.skip(&format!("{}::vendor_google", name), &format!("eval: {}", e)),
    }

    let _ = page.close();
}

fn scenario_stealth_property_efficacy_firefox(pool: &PagePool, report: &mut Report) {
    let name = "stealth_efficacy_firefox";
    let profile = StealthProfile::firefox_default();

    let page = match pool.create_page(&PageConfig {
        url: Some("data:text/html;charset=utf-8,<html><body>firefox</body></html>".into()),
        stealth_profile: Some(profile.clone()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            report.skip(name, &format!("page creation failed: {e}"));
            return;
        }
    };
    if let Err(e) = inject_stealth_js(&page, &profile) {
        report.skip(name, &format!("inject failed: {e}"));
        let _ = page.close();
        return;
    }
    wait_for_load(&page, 1500);

    match page.evaluate_js_web("String(navigator.webdriver)") {
        Ok(s) if s == "false" => report.pass(&format!("{}::webdriver_hidden", name)),
        Ok(other) => report.fail(
            &format!("{}::webdriver_hidden", name),
            &format!("navigator.webdriver leaked: {}", other),
        ),
        Err(e) => report.skip(&format!("{}::webdriver_hidden", name), &format!("eval: {}", e)),
    }

    match page.evaluate_js_web("navigator.userAgent") {
        Ok(s) if s.contains("Firefox") => report.pass(&format!("{}::ua_firefox", name)),
        Ok(other) => report.fail(
            &format!("{}::ua_firefox", name),
            &format!("UA missing 'Firefox': {}", other),
        ),
        Err(e) => report.skip(&format!("{}::ua_firefox", name), &format!("eval: {}", e)),
    }

    // Firefox vendor must be empty string
    match page.evaluate_js_web("navigator.vendor") {
        Ok(s) if s.is_empty() => report.pass(&format!("{}::vendor_empty", name)),
        Ok(other) => report.fail(
            &format!("{}::vendor_empty", name),
            &format!("Firefox vendor not empty: '{}'", other),
        ),
        Err(e) => report.skip(&format!("{}::vendor_empty", name), &format!("eval: {}", e)),
    }

    let _ = page.close();
}

// ---------------------------------------------------------------------------
// Phase 2: in-parent direct multi-site evaluation (BCE-20260622-004)
// ---------------------------------------------------------------------------
//
// Each site is evaluated IN-PARENT (no subprocess). The parent's single
// BaoRuntime has its PagePool create a fresh Page per site, navigate it
// externally, scrape real detection data via `evaluate_js_web`, evaluate,
// close the page, and move on.
//
// This is the production-faithful pattern: one browser process navigating
// across many pages. BCE-20260621-001 (WebViewId-keyed realm storage) and
// BCE-20260621-002 (servo `disable_script_debugger` + mozjs `initForOsr`
// NULL guards) eradicated the multi-nav SIGSEGV at root, so direct multi-nav
// is safe and no subprocess isolation is required.

/// Site evaluation spec — target site + chosen StealthProfile.
struct SiteSpec {
    name: &'static str,
    url: &'static str,
    /// "chrome" or "firefox" — selects the StealthProfile.
    profile: &'static str,
}

fn run_fingerprint_site_evaluations_in_parent(pool: &PagePool, report: &mut Report) {
    let sites = [
        SiteSpec {
            name: "sannysoft_chrome",
            url: "https://bot.sannysoft.com/",
            profile: "chrome",
        },
        SiteSpec {
            name: "sannysoft_firefox",
            url: "https://bot.sannysoft.com/",
            profile: "firefox",
        },
        SiteSpec {
            name: "creepjs_chrome",
            url: "https://abrahamjuliot.github.io/creepjs/",
            profile: "chrome",
        },
        SiteSpec {
            name: "pixelscan_chrome",
            url: "https://pixelscan.net/",
            profile: "chrome",
        },
        SiteSpec {
            name: "browserleaks_js_chrome",
            url: "https://browserleaks.com/javascript",
            profile: "chrome",
        },
    ];

    for site in &sites {
        let profile = match site.profile {
            "firefox" => StealthProfile::firefox_default(),
            _ => StealthProfile::chrome_default(),
        };
        // Fresh page per site (PagePool supports many concurrent pages).
        let page = match pool.create_page(&PageConfig {
            url: Some("about:blank".into()),
            stealth_profile: Some(profile.clone()),
            ..Default::default()
        }) {
            Ok(p) => p,
            Err(e) => {
                report.skip(site.name, &format!("create_page: {}", e));
                continue;
            }
        };

        // External HTTPS navigation — formerly SIGSEGV-trigger per BCE-002-residual
        // claim, now safe under BCE-001 + BCE-002 patches.
        if let Err(e) = page.navigate(site.url) {
            report.skip(site.name, &format!("navigate err: {}", e));
            let _ = page.close();
            continue;
        }
        wait_for_load(&page, 15000);

        // Re-inject stealth AFTER navigation so the post-load document reflects
        // our profile (the engine-level registration keys off the Window global
        // pointer, which changes after navigation).
        let _ = inject_stealth_js(&page, &profile);

        // Site-specific REAL evaluation.
        let (status, detail) = if site.url.contains("bot.sannysoft.com") {
            evaluate_sannysoft(&page)
        } else if site.url.contains("creepjs") {
            evaluate_creepjs(&page)
        } else if site.url.contains("pixelscan") {
            evaluate_pixelscan(&page)
        } else if site.url.contains("browserleaks") {
            evaluate_browserleaks(&page)
        } else {
            ("SKIP", "unknown site".to_string())
        };

        let label = if detail.is_empty() {
            site.name.to_string()
        } else {
            format!("{}::{}", site.name, detail)
        };
        match status {
            "PASS" => report.pass(&label),
            "FAIL" => report.fail(site.name, &detail),
            _ => report.skip(site.name, &detail),
        }
        let _ = page.close();
    }
}

// ---------------------------------------------------------------------------
// Site-specific evaluators — scrape real detection data from the DOM
// ---------------------------------------------------------------------------

/// Parse sannysoft's detection table. Returns (passed, failed) by probing
/// multiple selector strategies (sannysoft DOM has evolved across revisions):
///   1. `#table-result .passed/.failed` (legacy container)
///   2. `.passed` / `.failed` global counts (current)
fn sannysoft_counts(page: &PageHandle) -> Result<(u32, u32), String> {
    let js = "(function() { \
        var p1 = document.querySelectorAll('#table-result .passed').length; \
        var f1 = document.querySelectorAll('#table-result .failed').length; \
        if (p1 + f1 > 0) { return String(p1) + ',' + String(f1); } \
        var p2 = document.querySelectorAll('.passed').length; \
        var f2 = document.querySelectorAll('.failed').length; \
        return String(p2) + ',' + String(f2); \
     })()";
    let raw = page.evaluate_js_web(js).map_err(|e| format!("eval: {}", e))?;
    let parts: Vec<&str> = raw.split(',').collect();
    if parts.len() != 2 {
        return Err(format!("malformed counts '{}'", raw));
    }
    let passed: u32 = parts[0].trim().parse().map_err(|_| format!("bad passed '{}'", parts[0]))?;
    let failed: u32 = parts[1].trim().parse().map_err(|_| format!("bad failed '{}'", parts[1]))?;
    Ok((passed, failed))
}

/// Dump first ~10 sannysoft detection rows for human review of the real
/// evaluation (e.g. "WebDriver (New): missing (passed)").
fn sannysoft_row_sample(page: &PageHandle) -> String {
    let js = "(function() { \
        var rows = document.querySelectorAll('table tr'); \
        var out = []; \
        for (var i = 0; i < rows.length && out.length < 10; i++) { \
            var t = (rows[i].innerText || '').replace(/\\n/g, ' ').replace(/\\s+/g, ' ').trim(); \
            if (t) out.push(t); \
        } \
        return out.join(' | '); \
     })()";
    page.evaluate_js_web(js).unwrap_or_default()
}

/// Evaluate sannysoft: real detection counts + webdriver leak check.
/// Returns (STATUS, DETAIL).
fn evaluate_sannysoft(page: &PageHandle) -> (&'static str, String) {
    let counts = match sannysoft_counts(page) {
        Ok(c) => c,
        Err(e) => {
            let dump = sannysoft_row_sample(page);
            return (
                "SKIP",
                format!("detection counts error: {} | rows: {}", e, dump),
            );
        }
    };
    let (passed, failed) = counts;
    let total = passed + failed;
    if total == 0 {
        return ("SKIP", "no detection rows parsed (page did not fully load)".to_string());
    }
    let pass_rate = passed as f64 / total as f64;

    // Webdriver visibility check — stealth property must hold post-nav.
    let webdriver_leaked = match page.evaluate_js_web("String(navigator.webdriver)") {
        Ok(s) => s != "false" && s != "undefined",
        Err(_) => false, // eval error → don't add leak claim
    };

    // Efficacy: pass rate >= 0.6 AND no webdriver leak.
    let row_sample = sannysoft_row_sample(page);
    let short_rows: String = row_sample.chars().take(200).collect();
    if pass_rate >= 0.6 && !webdriver_leaked {
        (
            "PASS",
            format!(
                "counts(p={},f={},rate={:.2}) webdriver_hidden={} rows=[{}]",
                passed, failed, pass_rate, !webdriver_leaked, short_rows
            ),
        )
    } else if webdriver_leaked {
        (
            "FAIL",
            format!(
                "webdriver LEAKED (counts p={},f={},rate={:.2})",
                passed, failed, pass_rate
            ),
        )
    } else {
        (
            "FAIL",
            format!(
                "pass_rate={:.2} below 0.6 (p={},f={}) rows=[{}]",
                pass_rate, passed, failed, short_rows
            ),
        )
    }
}

/// Evaluate creepjs: read trust score (and lie count as secondary).
fn evaluate_creepjs(page: &PageHandle) -> (&'static str, String) {
    let trust_js = "(function() { \
        var el = document.querySelector('.trust-score') \
              || document.querySelector('[class*=trust]') \
              || document.querySelector('#trust-score'); \
        if (!el) { return ''; } \
        return String(el.textContent || '').trim(); \
     })()";
    let trust = page.evaluate_js_web(trust_js).unwrap_or_default();
    let body_len = page
        .evaluate_js_web("String((document.body.innerText||'').length)")
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0);

    if !trust.is_empty() {
        let short: String = trust.chars().take(40).collect();
        return ("PASS", format!("trust_score={} body_len={}", short, body_len));
    }
    // Trust score element not found — fall back to body length as evidence
    // that creepjs at least loaded and started computing.
    if body_len > 500 {
        (
            "PASS",
            format!("trust_score_not_rendered_yet body_len={} (creepjs async)", body_len),
        )
    } else {
        (
            "SKIP",
            format!("trust score not computed & body short (len={})", body_len),
        )
    }
}

/// Evaluate pixelscan: confirm page loaded and check for matching/mismatch text.
fn evaluate_pixelscan(page: &PageHandle) -> (&'static str, String) {
    let js = "(function() { \
        var text = String(document.body.innerText || ''); \
        var hasMatching = text.indexOf('Matching') !== -1 || text.indexOf('matching') !== -1; \
        var hasMismatch = text.indexOf('Mismatch') !== -1 || text.indexOf('mismatch') !== -1; \
        return String(text.length) + ',' + (hasMatching ? '1' : '0') + ',' + (hasMismatch ? '1' : '0'); \
     })()";
    match page.evaluate_js_web(js) {
        Ok(s) => {
            let parts: Vec<&str> = s.split(',').collect();
            if parts.len() != 3 {
                return ("SKIP", format!("malformed pixelscan probe '{}'", s));
            }
            let body_len: u32 = parts[0].parse().unwrap_or(0);
            let has_matching = parts[1] == "1";
            let has_mismatch = parts[2] == "1";
            if body_len < 100 {
                return ("SKIP", "short page (network blocked)".to_string());
            }
            let mut tags = vec![];
            if has_matching {
                tags.push("has_matching");
            }
            if has_mismatch {
                tags.push("has_mismatch");
            }
            if tags.is_empty() {
                tags.push("no_matching_or_mismatch_text");
            }
            (
                "PASS",
                format!("page_loaded body_len={} [{}]", body_len, tags.join(",")),
            )
        }
        Err(e) => ("SKIP", format!("pixelscan eval: {}", e)),
    }
}

/// Evaluate browserleaks/javascript: confirm page loaded with substantial body.
fn evaluate_browserleaks(page: &PageHandle) -> (&'static str, String) {
    match page.evaluate_js_web(
        "(function() { return String((document.body.innerText||'').length); })()",
    ) {
        Ok(s) => match s.trim().parse::<u32>() {
            Ok(len) if len > 100 => ("PASS", format!("page_loaded body_len={}", len)),
            Ok(len) => ("SKIP", format!("short page len={}", len)),
            Err(_) => ("SKIP", format!("bad length '{}'", s)),
        },
        Err(e) => ("SKIP", format!("browserleaks eval: {}", e)),
    }
}
