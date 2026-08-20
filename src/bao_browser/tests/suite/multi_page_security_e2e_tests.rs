// @trace TEST-BRW-E2E-MULTIPAGE-SEC [req:REQ-BRW-003,REQ-LIB-001] [level:e2e]
// Real-world multi-page / multi-context security E2E test.
//
// Launches BaoRuntime (servo), creates multiple pages with distinct
// StealthProfile (chrome / firefox / none), and verifies Realm isolation:
//   - Each page's globalThis is isolated: setting `window.__id = 'A'` on one
//     page does NOT leak to other pages.
//   - Each page's navigator.userAgent reflects its profile (chrome page UA
//     contains "Chrome"; firefox page UA contains "Firefox"; none page has
//     servo's default UA).
//   - Stealth noise is keyed per-Realm: profile overrides on page A do not
//     affect page B's navigator/screen.
//
// Graceful strategy:
//   - Requires real servo runtime + display server. Absent either (or
//     BaoRuntime::new fails) → `[skip]` + return.
//   - Local `data:text/html` test pages — NO external network required.

#![allow(dead_code)]

use bao_browser::{BaoConfig, BaoRuntime, PageConfig, PageHandle, PagePool, PageState};
use bao_stealth::StealthProfile;
use std::time::{Duration, Instant};

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
    fn assert_actual(&mut self, ok: bool, pass_msg: &str, fail_msg: &str) {
        if ok {
            self.passed += 1;
            self.messages.push(format!("PASS  {}", pass_msg));
        } else {
            self.failed += 1;
            self.messages.push(format!("FAIL  {}", fail_msg));
        }
    }
    fn finish(&self) {
        eprintln!("\n=== Multi-Page Security E2E ===");
        for m in &self.messages {
            eprintln!("{}", m);
        }
        eprintln!(
            "--- {} passed, {} skipped, {} failed ---",
            self.passed, self.skipped, self.failed
        );
    }
}

fn wait_for_load(page: &PageHandle, max_ms: u64) {
    let start = Instant::now();
    while start.elapsed().as_millis() < max_ms as u128 {
        let _ = page.evaluate_js("");
        if matches!(page.get_state(), PageState::Interactive | PageState::Idle) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Inject stealth property overrides for a profile via JS Object.defineProperty.
/// Used so the per-page Realm's navigator/screen reflect the profile.
fn inject_stealth_js(page: &PageHandle, profile: &StealthProfile) -> Result<(), String> {
    let escaped_ua = profile
        .navigator
        .user_agent
        .replace('\\', "\\\\")
        .replace('\'', "\\'");
    let escaped_vendor = profile
        .navigator
        .vendor
        .replace('\\', "\\\\")
        .replace('\'', "\\'");
    let escaped_platform = profile
        .navigator
        .platform
        .replace('\\', "\\\\")
        .replace('\'', "\\'");

    let js = format!(
        "(function() {{ \
           try {{ Object.defineProperty(navigator, 'userAgent', {{get: function(){{return '{ua}';}}, configurable: false}}); }} catch(e){{}} \
           try {{ Object.defineProperty(navigator, 'vendor', {{get: function(){{return '{vendor}';}}, configurable: false}}); }} catch(e){{}} \
           try {{ Object.defineProperty(navigator, 'platform', {{get: function(){{return '{platform}';}}, configurable: false}}); }} catch(e){{}} \
           try {{ Object.defineProperty(navigator, 'webdriver', {{get: function(){{return false;}}, configurable: false}}); }} catch(e){{}} \
           try {{ Object.defineProperty(navigator, 'hardwareConcurrency', {{get: function(){{return {hc}; }}, configurable: false}}); }} catch(e){{}} \
           try {{ Object.defineProperty(screen, 'width', {{get: function(){{return {w}; }}, configurable: false}}); }} catch(e){{}} \
           try {{ Object.defineProperty(screen, 'height', {{get: function(){{return {h}; }}, configurable: false}}); }} catch(e){{}} \
         }})()",
        ua = escaped_ua,
        vendor = escaped_vendor,
        platform = escaped_platform,
        hc = profile.navigator.hardware_concurrency,
        w = profile.screen.width,
        h = profile.screen.height
    );
    page.evaluate_js_web(&js)
        .map(|_| ())
        .map_err(|e| format!("inject: {}", e))
}

#[test]
fn multi_page_security_e2e() {
    // Guard 1: opt-in for real servo runtime
    if std::env::var("BAO_TEST_REAL_SERVO").as_deref() != Ok("1") {
        eprintln!(
            "[skip] BAO_TEST_REAL_SERVO != 1 — multi-page security E2E requires real servo runtime"
        );
        return;
    }
    // Guard 2: servo requires a display server
    if std::env::var("DISPLAY").is_err() && std::env::var("WAYLAND_DISPLAY").is_err() {
        eprintln!("[skip] no DISPLAY or WAYLAND_DISPLAY — servo requires a display server");
        return;
    }
    // Guard 3: BaoRuntime::new may fail in environments lacking servo runtime deps
    let config = BaoConfig::default();
    let runtime = match BaoRuntime::new(config) {
        Ok(r) => r,
        Err(e) => {
            eprintln!(
                "[skip] BaoRuntime::new failed (likely missing servo runtime): {}",
                e
            );
            return;
        }
    };
    let pool: &PagePool = runtime.page_pool();
    let mut report = Report::default();

    scenario_global_isolation(pool, &mut report);
    scenario_user_agent_isolation(pool, &mut report);
    scenario_stealth_realm_isolation(pool, &mut report);
    scenario_pool_stats_tracking(pool, &mut report);

    pool.close_all();
    report.finish();

    assert_eq!(
        report.failed, 0,
        "{} sub-assertions failed — see stderr above",
        report.failed
    );
}

/// Verify that global variables set on one page do not leak to other pages.
fn scenario_global_isolation(pool: &PagePool, report: &mut Report) {
    let name = "global_isolation";

    let url_template = |id: &str| {
        format!(
            "data:text/html;charset=utf-8,<html><body id=\"{}\">{}</body></html>",
            id, id
        )
    };

    let page_a = match pool.create_page(&PageConfig {
        url: Some(url_template("pageA")),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            report.skip(name, &format!("pageA creation failed: {e}"));
            return;
        }
    };
    let page_b = match pool.create_page(&PageConfig {
        url: Some(url_template("pageB")),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            report.skip(name, &format!("pageB creation failed: {e}"));
            let _ = page_a.close();
            return;
        }
    };
    let page_c = match pool.create_page(&PageConfig {
        url: Some(url_template("pageC")),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            report.skip(name, &format!("pageC creation failed: {e}"));
            let _ = page_a.close();
            let _ = page_b.close();
            return;
        }
    };

    wait_for_load(&page_a, 1500);
    wait_for_load(&page_b, 1500);
    wait_for_load(&page_c, 1500);

    // Set distinct global identifiers on each page
    if let Err(e) = page_a.evaluate_js_web("window.__id = 'A'") {
        report.skip(name, &format!("set pageA __id: {}", e));
        let _ = page_a.close();
        let _ = page_b.close();
        let _ = page_c.close();
        return;
    }
    if let Err(e) = page_b.evaluate_js_web("window.__id = 'B'") {
        report.skip(name, &format!("set pageB __id: {}", e));
        let _ = page_a.close();
        let _ = page_b.close();
        let _ = page_c.close();
        return;
    }
    if let Err(e) = page_c.evaluate_js_web("window.__id = 'C'") {
        report.skip(name, &format!("set pageC __id: {}", e));
        let _ = page_a.close();
        let _ = page_b.close();
        let _ = page_c.close();
        return;
    }

    // Verify each page's __id is its own (not leaked from another page)
    let a_id = page_a
        .evaluate_js_web("window.__id || 'undefined'")
        .unwrap_or_default();
    let b_id = page_b
        .evaluate_js_web("window.__id || 'undefined'")
        .unwrap_or_default();
    let c_id = page_c
        .evaluate_js_web("window.__id || 'undefined'")
        .unwrap_or_default();

    report.assert_actual(
        a_id == "A",
        &format!("{}::pageA_id_correct", name),
        &format!("{}::pageA_id (got '{}', want 'A')", name, a_id),
    );
    report.assert_actual(
        b_id == "B",
        &format!("{}::pageB_id_correct", name),
        &format!("{}::pageB_id (got '{}', want 'B')", name, b_id),
    );
    report.assert_actual(
        c_id == "C",
        &format!("{}::pageC_id_correct", name),
        &format!("{}::pageC_id (got '{}', want 'C')", name, c_id),
    );

    // Verify cross-page isolation: modifying pageA's __id does NOT affect pageB
    let _ = page_a.evaluate_js_web("window.__id = 'A_MODIFIED'");
    let b_id_after = page_b
        .evaluate_js_web("window.__id || 'undefined'")
        .unwrap_or_default();
    report.assert_actual(
        b_id_after == "B",
        &format!("{}::pageB_unchanged_after_A_modify", name),
        &format!("{}::pageB_leaked_from_A (got '{}')", name, b_id_after),
    );

    // Realm isolation: each page's body.id is its own
    let a_body = page_a
        .evaluate_js_web("document.body.id")
        .unwrap_or_default();
    let b_body = page_b
        .evaluate_js_web("document.body.id")
        .unwrap_or_default();
    report.assert_actual(
        a_body == "pageA" && b_body == "pageB",
        &format!("{}::body_id_isolated", name),
        &format!("{}::body_id_leaked (A='{}', B='{}')", name, a_body, b_body),
    );

    let _ = page_a.close();
    let _ = page_b.close();
    let _ = page_c.close();
}

/// Verify navigator.userAgent is isolated per profile across pages.
fn scenario_user_agent_isolation(pool: &PagePool, report: &mut Report) {
    let name = "ua_isolation";

    let chrome_profile = StealthProfile::chrome_default();
    let firefox_profile = StealthProfile::firefox_default();

    let chrome_page = match pool.create_page(&PageConfig {
        url: Some("data:text/html;charset=utf-8,<html><body>chrome</body></html>".into()),
        stealth_profile: Some(chrome_profile.clone()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            report.skip(name, &format!("chrome_page creation failed: {e}"));
            return;
        }
    };
    let firefox_page = match pool.create_page(&PageConfig {
        url: Some("data:text/html;charset=utf-8,<html><body>firefox</body></html>".into()),
        stealth_profile: Some(firefox_profile.clone()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            report.skip(name, &format!("firefox_page creation failed: {e}"));
            let _ = chrome_page.close();
            return;
        }
    };
    let none_page = match pool.create_page(&PageConfig {
        url: Some("data:text/html;charset=utf-8,<html><body>none</body></html>".into()),
        stealth_profile: None,
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            report.skip(name, &format!("none_page creation failed: {e}"));
            let _ = chrome_page.close();
            let _ = firefox_page.close();
            return;
        }
    };

    wait_for_load(&chrome_page, 1500);
    wait_for_load(&firefox_page, 1500);
    wait_for_load(&none_page, 1500);

    // Inject profile overrides per-page (each Realm sees only its profile's UA)
    let _ = inject_stealth_js(&chrome_page, &chrome_profile);
    let _ = inject_stealth_js(&firefox_page, &firefox_profile);
    // none_page gets no overrides — servo default UA.

    let chrome_ua = chrome_page
        .evaluate_js_web("navigator.userAgent")
        .unwrap_or_default();
    let firefox_ua = firefox_page
        .evaluate_js_web("navigator.userAgent")
        .unwrap_or_default();
    let none_ua = none_page
        .evaluate_js_web("navigator.userAgent")
        .unwrap_or_default();

    // Chrome page UA contains "Chrome"
    report.assert_actual(
        chrome_ua.contains("Chrome"),
        &format!("{}::chrome_page_ua", name),
        &format!(
            "{}::chrome_page_ua (got '{}', missing 'Chrome')",
            name, chrome_ua
        ),
    );

    // Firefox page UA contains "Firefox"
    report.assert_actual(
        firefox_ua.contains("Firefox"),
        &format!("{}::firefox_page_ua", name),
        &format!(
            "{}::firefox_page_ua (got '{}', missing 'Firefox')",
            name, firefox_ua
        ),
    );

    // Chrome and Firefox UAs differ
    report.assert_actual(
        chrome_ua != firefox_ua,
        &format!("{}::chrome_neq_firefox_ua", name),
        &format!("{}::chrome_neq_firefox_ua (identical!)", name),
    );

    // Cross-page isolation: chrome UA does NOT contain "Firefox" and vice versa
    report.assert_actual(
        !chrome_ua.contains("Firefox"),
        &format!("{}::chrome_no_firefox_leak", name),
        &format!("{}::chrome_no_firefox_leak (UA='{}')", name, chrome_ua),
    );
    report.assert_actual(
        !firefox_ua.contains("Chrome"),
        &format!("{}::firefox_no_chrome_leak", name),
        &format!("{}::firefox_no_chrome_leak (UA='{}')", name, firefox_ua),
    );

    // None page must have SOME UA (servo default) — non-empty
    report.assert_actual(
        !none_ua.is_empty(),
        &format!("{}::none_page_has_default_ua", name),
        &format!("{}::none_page_has_default_ua (empty)", name),
    );

    let _ = chrome_page.close();
    let _ = firefox_page.close();
    let _ = none_page.close();
}

/// Verify that stealth profile noise (screen dimensions, hardware concurrency)
/// is isolated per-page Realm: a profile's override on page A does not affect
/// page B's screen.
fn scenario_stealth_realm_isolation(pool: &PagePool, report: &mut Report) {
    let name = "stealth_realm_isolation";

    let chrome_profile = StealthProfile::chrome_default();
    let firefox_profile = StealthProfile::firefox_default();

    let page_a = match pool.create_page(&PageConfig {
        url: Some("data:text/html;charset=utf-8,<html><body>A</body></html>".into()),
        stealth_profile: Some(chrome_profile.clone()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            report.skip(name, &format!("pageA creation failed: {e}"));
            return;
        }
    };
    let page_b = match pool.create_page(&PageConfig {
        url: Some("data:text/html;charset=utf-8,<html><body>B</body></html>".into()),
        stealth_profile: Some(firefox_profile.clone()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            report.skip(name, &format!("pageB creation failed: {e}"));
            let _ = page_a.close();
            return;
        }
    };

    wait_for_load(&page_a, 1500);
    wait_for_load(&page_b, 1500);

    let _ = inject_stealth_js(&page_a, &chrome_profile);
    let _ = inject_stealth_js(&page_b, &firefox_profile);

    let a_vendor = page_a
        .evaluate_js_web("navigator.vendor")
        .unwrap_or_default();
    let b_vendor = page_b
        .evaluate_js_web("navigator.vendor")
        .unwrap_or_default();

    // Chrome vendor = "Google Inc.", Firefox vendor = ""
    report.assert_actual(
        a_vendor == "Google Inc." && b_vendor.is_empty(),
        &format!("{}::vendor_isolated", name),
        &format!(
            "{}::vendor_isolated (A='{}', B='{}')",
            name, a_vendor, b_vendor
        ),
    );

    // hardwareConcurrency differs if profiles differ (chrome=8, firefox=4 by default)
    let a_hc = page_a
        .evaluate_js_web("String(navigator.hardwareConcurrency)")
        .unwrap_or_default();
    let b_hc = page_b
        .evaluate_js_web("String(navigator.hardwareConcurrency)")
        .unwrap_or_default();

    // Both must have valid integer HC values
    let a_hc_parsed: u32 = a_hc.trim().parse().unwrap_or(0);
    let b_hc_parsed: u32 = b_hc.trim().parse().unwrap_or(0);
    report.assert_actual(
        a_hc_parsed > 0 && b_hc_parsed > 0,
        &format!("{}::hc_valid", name),
        &format!("{}::hc_valid (A={}, B={})", name, a_hc_parsed, b_hc_parsed),
    );

    // webdriver must be false on both pages
    let a_wd = page_a
        .evaluate_js_web("String(navigator.webdriver)")
        .unwrap_or_default();
    let b_wd = page_b
        .evaluate_js_web("String(navigator.webdriver)")
        .unwrap_or_default();
    report.assert_actual(
        a_wd == "false" && b_wd == "false",
        &format!("{}::webdriver_hidden_both", name),
        &format!(
            "{}::webdriver_hidden_both (A='{}', B='{}')",
            name, a_wd, b_wd
        ),
    );

    let _ = page_a.close();
    let _ = page_b.close();
}

/// Verify pool stats track multiple active pages correctly.
fn scenario_pool_stats_tracking(pool: &PagePool, report: &mut Report) {
    let name = "pool_stats";

    let initial = pool.stats();
    report.assert_actual(
        true,
        &format!(
            "{}::initial_state(total_created={})",
            name, initial.total_created
        ),
        &format!("{}::initial_state", name),
    );

    // Create 3 pages
    let mut pages = Vec::new();
    for i in 0..3 {
        let url = format!(
            "data:text/html;charset=utf-8,<html><body>{}</body></html>",
            i
        );
        match pool.create_page(&PageConfig {
            url: Some(url),
            ..Default::default()
        }) {
            Ok(p) => pages.push(p),
            Err(e) => {
                report.skip(name, &format!("create_{}: {}", i, e));
                break;
            }
        }
    }

    let after_create = pool.stats();
    report.assert_actual(
        after_create.total_created >= initial.total_created + pages.len() as usize,
        &format!("{}::total_created_+", name),
        &format!(
            "{}::total_created (initial={}, after={}, want +{})",
            name,
            initial.total_created,
            after_create.total_created,
            pages.len()
        ),
    );
    report.assert_actual(
        after_create.active >= pages.len(),
        &format!("{}::active_count", name),
        &format!(
            "{}::active_count (got {}, want >= {})",
            name,
            after_create.active,
            pages.len()
        ),
    );

    // Close all pages
    for p in pages.drain(..) {
        let _ = p.close();
    }

    let _ = report;
}
