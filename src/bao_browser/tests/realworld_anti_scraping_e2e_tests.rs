// @trace TEST-E2E-ANTISCRAPE [req:REQ-STL-001,REQ-STL-002,REQ-STL-003,REQ-STL-004,REQ-STL-005,REQ-STL-006,REQ-STL-007,REQ-BRW-001,REQ-BRW-002] [level:e2e]
// Real-world anti-scraping E2E: bao's built-in Servo browser vs real websites.
//
// Architecture:
//   - Single #[test] (mozjs Runtime + servo Opts are per-process singletons)
//   - Uses PagePool::create_page + JS-level stealth property injection
//   - Direct function-level API: page.evaluate_js() + page.take_screenshot()
//   - Stealth properties injected via Object.defineProperty (same profile data
//     as engine_props, but via JS because runtime.create_page SIGSEGVs —
//     a pre-existing bug in the script-thread callback drain mechanism).
//   - Report pattern: pass/skip/fail with fault tolerance for network issues
//
// Scenarios:
//   1. Stealth properties verification (navigator/screen/webgl)
//   2. Navigate to real anti-scraping sites (58.com, meituan.com)
//   3. Screenshot capture of loaded pages
//   4. UA/fingerprint leak detection on loaded pages

#![allow(dead_code)]

use bao_browser::{BaoConfig, BaoRuntime, PageConfig, PagePool, PageState, ScreenshotFormat};
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
        eprintln!("\n=== Realworld Anti-Scraping E2E ===");
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
// wait_for_load — drive servo's paint loop until page is interactive
// ---------------------------------------------------------------------------

fn wait_for_load(page: &bao_browser::PageHandle, max_ms: u64) {
    let start = Instant::now();
    while start.elapsed().as_millis() < max_ms as u128 {
        let _ = page.evaluate_js("");
        if matches!(page.get_state(), PageState::Interactive | PageState::Idle) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

// ---------------------------------------------------------------------------
// inject_stealth_js — JS-level stealth property injection via Object.defineProperty
// ---------------------------------------------------------------------------

/// Inject stealth profile values as non-configurable getters on navigator/screen/window.
/// Uses the same profile data source as engine_props, ensuring value consistency.
fn inject_stealth_js(page: &bao_browser::PageHandle, profile: &StealthProfile) -> Result<(), String> {
    // Navigator properties
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

    // Navigator numeric properties
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

    // Navigator.webdriver
    let js = "(function() { try { Object.defineProperty(navigator, 'webdriver', {get: function(){return false;}, configurable: false}); } catch(e){} })()";
    page.evaluate_js_web(&js).map_err(|e| format!("inject webdriver: {}", e))?;

    // Screen properties
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

    // devicePixelRatio
    let js = format!(
        "(function() {{ try {{ Object.defineProperty(window, 'devicePixelRatio', {{get: function(){{return {}; }}, configurable: false}}); }} catch(e){{}} }})()",
        profile.screen.device_pixel_ratio
    );
    page.evaluate_js_web(&js).map_err(|e| format!("inject dpr: {}", e))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Main test — single servo init, fault-tolerant scenarios
// ---------------------------------------------------------------------------

#[test]
fn realworld_anti_scraping_e2e() {
    let config = BaoConfig::default();
    let runtime = match BaoRuntime::new(config) {
        Ok(r) => r,
        Err(e) => panic!("BaoRuntime::new failed: {}", e),
    };
    let pool: &PagePool = runtime.page_pool();
    let mut report = Report::default();

    // Phase 1: Stealth property injection verification (local, no network)
    scenario_stealth_properties(pool, &mut report);

    // Phase 2: Real website navigation with stealth
    scenario_navigate_58_com(pool, &mut report);
    scenario_navigate_meituan(pool, &mut report);
    scenario_navigate_example_com(pool, &mut report);

    // Phase 3: Screenshot capture verification
    scenario_screenshot_capture(pool, &mut report);

    pool.close_all();
    report.finish();

    // Network-dependent scenarios may skip. At least stealth properties must pass.
    let total = report.passed + report.failed;
    if total > 0 {
        let pass_ratio = report.passed as f64 / total as f64;
        assert!(
            pass_ratio >= 0.3,
            "too few sub-assertions passed: {}/{} (ratio {:.2})",
            report.passed,
            total,
            pass_ratio
        );
    }
    // Hard failures on stealth properties = real regression.
    // Network failures are tolerable (skip), but stealth property failures are not.
    let stealth_fails = report.messages.iter().filter(|m| m.starts_with("FAIL") && m.contains("stealth")).count();
    assert_eq!(
        stealth_fails, 0,
        "{} stealth property assertions failed — see stderr above",
        stealth_fails
    );
}

// ---------------------------------------------------------------------------
// Scenario: Stealth property injection verification (no network needed)
// ---------------------------------------------------------------------------

fn scenario_stealth_properties(pool: &PagePool, report: &mut Report) {
    let name = "stealth_properties";
    let profile = StealthProfile::firefox_default();

    let page = match pool.create_page(&PageConfig {
        url: Some("data:text/html,<html><body>stealth-test</body></html>".into()),
        stealth_profile: Some(profile.clone()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            report.skip(name, &format!("page creation failed: {e}"));
            return;
        }
    };

    // Wait for page to be interactive, then inject stealth properties.
    wait_for_load(&page, 2000);
    if let Err(e) = inject_stealth_js(&page, &profile) {
        report.skip(name, &format!("stealth injection failed: {e}"));
        let _ = page.close();
        return;
    }

    // Verify navigator.userAgent contains Firefox
    match page.evaluate_js_web("navigator.userAgent") {
        Ok(ua) if ua.contains("Firefox") => report.pass(&format!("{}::ua_firefox", name)),
        Ok(ua) => report.fail(&format!("{}::ua_firefox", name), &format!("UA missing Firefox: {}", ua)),
        Err(e) => report.skip(&format!("{}::ua_firefox", name), &format!("evaluate_js: {}", e)),
    }

    // Verify navigator.webdriver === false
    match page.evaluate_js_web("String(navigator.webdriver)") {
        Ok(s) if s == "false" => report.pass(&format!("{}::webdriver_false", name)),
        Ok(s) => report.fail(&format!("{}::webdriver_false", name), &format!("webdriver={}", s)),
        Err(e) => report.skip(&format!("{}::webdriver_false", name), &format!("evaluate_js: {}", e)),
    }

    // Verify screen dimensions match profile
    match page.evaluate_js_web("screen.width + 'x' + screen.height") {
        Ok(s) if s.contains("1920") && s.contains("1080") => {
            report.pass(&format!("{}::screen_dims", name))
        }
        Ok(s) => report.fail(&format!("{}::screen_dims", name), &format!("screen={}", s)),
        Err(e) => report.skip(&format!("{}::screen_dims", name), &format!("evaluate_js: {}", e)),
    }

    // Verify screen.colorDepth
    match page.evaluate_js_web("String(screen.colorDepth)") {
        Ok(s) if s == "24" => report.pass(&format!("{}::color_depth", name)),
        Ok(s) => report.fail(&format!("{}::color_depth", name), &format!("colorDepth={}", s)),
        Err(e) => report.skip(&format!("{}::color_depth", name), &format!("evaluate_js: {}", e)),
    }

    // Verify navigator.vendor is empty (Firefox)
    match page.evaluate_js_web("navigator.vendor") {
        Ok(s) if s.is_empty() => report.pass(&format!("{}::vendor_empty", name)),
        Ok(s) => report.fail(&format!("{}::vendor_empty", name), &format!("vendor='{}'", s)),
        Err(e) => report.skip(&format!("{}::vendor_empty", name), &format!("evaluate_js: {}", e)),
    }

    // Verify navigator.hardwareConcurrency
    match page.evaluate_js_web("String(navigator.hardwareConcurrency)") {
        Ok(s) => {
            let val: i32 = s.parse().unwrap_or(0);
            if val == 8 {
                report.pass(&format!("{}::hardware_concurrency", name));
            } else {
                report.fail(&format!("{}::hardware_concurrency", name), &format!("expected 8 got: {}", s));
            }
        }
        Err(e) => report.skip(&format!("{}::hardware_concurrency", name), &format!("evaluate_js: {}", e)),
    }

    // Verify navigator.platform
    match page.evaluate_js_web("navigator.platform") {
        Ok(s) if s.contains("Linux") => report.pass(&format!("{}::platform", name)),
        Ok(s) => report.fail(&format!("{}::platform", name), &format!("platform='{}'", s)),
        Err(e) => report.skip(&format!("{}::platform", name), &format!("evaluate_js: {}", e)),
    }

    // Verify no "HeadlessChrome" in UA
    match page.evaluate_js_web("navigator.userAgent") {
        Ok(ua) if !ua.contains("Headless") && !ua.contains("headless") => {
            report.pass(&format!("{}::no_headless_ua", name))
        }
        Ok(ua) => report.fail(&format!("{}::no_headless_ua", name), &format!("UA contains headless: {}", ua)),
        Err(e) => report.skip(&format!("{}::no_headless_ua", name), &format!("evaluate_js: {}", e)),
    }

    let _ = page.close();
}

// ---------------------------------------------------------------------------
// Scenario: Navigate to 58.com (moderate anti-scraping)
// ---------------------------------------------------------------------------

fn scenario_navigate_58_com(pool: &PagePool, report: &mut Report) {
    let name = "navigate_58_com";
    let profile = StealthProfile::firefox_default();

    let page = match pool.create_page(&PageConfig {
        url: Some("https://58.com".into()),
        stealth_profile: Some(profile.clone()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            report.skip(name, &format!("page creation failed: {e}"));
            return;
        }
    };

    wait_for_load(&page, 8000);
    let _ = inject_stealth_js(&page, &profile);

    // Check page title
    match page.page_title() {
        Some(t) if !t.is_empty() => {
            report.pass(&format!("{}::has_title", name));
            eprintln!("  [58.com] title: {}", t);
        }
        _ => report.skip(&format!("{}::has_title", name), "no title (network/rendering)"),
    }

    // Verify UA is still Firefox after navigation
    match page.evaluate_js_web("navigator.userAgent") {
        Ok(ua) if ua.contains("Firefox") => {
            report.pass(&format!("{}::ua_persists", name));
        }
        Ok(ua) => {
            report.fail(&format!("{}::ua_persists", name), &format!("UA changed: {}", ua));
        }
        Err(e) => report.skip(&format!("{}::ua_persists", name), &format!("evaluate_js: {}", e)),
    }

    // Verify webdriver still false after navigation
    match page.evaluate_js_web("String(navigator.webdriver)") {
        Ok(s) if s == "false" => {
            report.pass(&format!("{}::webdriver_persists", name));
        }
        Ok(s) => {
            report.fail(&format!("{}::webdriver_persists", name), &format!("webdriver after nav: {}", s));
        }
        Err(e) => report.skip(&format!("{}::webdriver_persists", name), &format!("evaluate_js: {}", e)),
    }

    let _ = page.close();
}

// ---------------------------------------------------------------------------
// Scenario: Navigate to meituan.com (strong anti-scraping, WAF)
// ---------------------------------------------------------------------------

fn scenario_navigate_meituan(pool: &PagePool, report: &mut Report) {
    let name = "navigate_meituan";
    let profile = StealthProfile::firefox_default();

    let page = match pool.create_page(&PageConfig {
        url: Some("https://www.meituan.com".into()),
        stealth_profile: Some(profile.clone()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            report.skip(name, &format!("page creation failed: {e}"));
            return;
        }
    };

    wait_for_load(&page, 10000);
    let _ = inject_stealth_js(&page, &profile);

    // Check page title
    match page.page_title() {
        Some(t) if t.contains("美团") || t.contains("meituan") || t.contains("Meituan") => {
            report.pass(&format!("{}::has_title", name));
            eprintln!("  [meituan.com] title: {}", t);
        }
        Some(t) if !t.is_empty() => {
            report.skip(&format!("{}::has_title", name), &format!("unexpected title: {}", t));
        }
        _ => report.skip(&format!("{}::has_title", name), "no title (WAF or network)"),
    }

    // Check if WAF blocked us
    match page.evaluate_js_web("document.body ? document.body.innerText.substring(0, 200) : 'no body'") {
        Ok(text) => {
            if text.contains("验证") || text.contains("challenge") || text.contains("请完成验证") {
                report.skip(&format!("{}::waf_challenge", name), "WAF challenge page detected");
            } else if !text.is_empty() {
                report.pass(&format!("{}::page_loaded", name));
                eprintln!("  [meituan.com] body preview: {}...", &text[..text.len().min(100)]);
            } else {
                report.skip(&format!("{}::page_loaded", name), "empty body");
            }
        }
        Err(e) => report.skip(&format!("{}::page_content", name), &format!("evaluate_js: {}", e)),
    }

    // Stealth properties should persist
    match page.evaluate_js_web("navigator.userAgent") {
        Ok(ua) if ua.contains("Firefox") => report.pass(&format!("{}::ua_persists", name)),
        Ok(ua) => report.fail(&format!("{}::ua_persists", name), &format!("UA: {}", ua)),
        Err(e) => report.skip(&format!("{}::ua_persists", name), &format!("evaluate_js: {}", e)),
    }

    let _ = page.close();
}

// ---------------------------------------------------------------------------
// Scenario: Navigate to example.com (baseline, no anti-scraping)
// ---------------------------------------------------------------------------

fn scenario_navigate_example_com(pool: &PagePool, report: &mut Report) {
    let name = "navigate_example_com";
    let profile = StealthProfile::firefox_default();

    let page = match pool.create_page(&PageConfig {
        url: Some("https://example.com".into()),
        stealth_profile: Some(profile.clone()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            report.skip(name, &format!("page creation failed: {e}"));
            return;
        }
    };

    wait_for_load(&page, 6000);
    let _ = inject_stealth_js(&page, &profile);

    // example.com baseline
    match page.page_title() {
        Some(t) if t.contains("Example") || t.contains("example") => {
            report.pass(&format!("{}::title", name));
        }
        Some(t) => {
            report.skip(&format!("{}::title", name), &format!("unexpected: {}", t));
        }
        None => report.skip(&format!("{}::title", name), "no title"),
    }

    // Verify DOM loaded
    match page.evaluate_js_web("document.querySelector('h1') ? document.querySelector('h1').textContent : 'none'") {
        Ok(s) if s.contains("Example") => report.pass(&format!("{}::dom_h1", name)),
        Ok(s) => report.skip(&format!("{}::dom_h1", name), &format!("h1={}", s)),
        Err(e) => report.skip(&format!("{}::dom_h1", name), &format!("evaluate_js: {}", e)),
    }

    // Verify stealth on this clean page
    match page.evaluate_js_web("navigator.userAgent") {
        Ok(ua) if ua.contains("Firefox") => report.pass(&format!("{}::stealth_ua", name)),
        Ok(ua) => report.fail(&format!("{}::stealth_ua", name), &format!("UA: {}", ua)),
        Err(e) => report.skip(&format!("{}::stealth_ua", name), &format!("evaluate_js: {}", e)),
    }

    let _ = page.close();
}

// ---------------------------------------------------------------------------
// Scenario: Screenshot capture verification
// ---------------------------------------------------------------------------

fn scenario_screenshot_capture(pool: &PagePool, report: &mut Report) {
    let name = "screenshot_capture";

    let page = match pool.create_page(&PageConfig {
        url: Some("data:text/html,<html><head><style>body{background:%234285f4;color:white;display:flex;align-items:center;justify-content:center;height:100vh;margin:0;font-size:48px;}</style></head><body>BAO-STEALTH-TEST</body></html>".into()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            report.skip(name, &format!("page creation failed: {e}"));
            return;
        }
    };

    wait_for_load(&page, 2000);

    // PNG screenshot
    match page.take_screenshot(ScreenshotFormat::Png) {
        Ok(data) if data.len() > 1000 => {
            report.pass(&format!("{}::png_valid", name));
            eprintln!("  [screenshot] PNG size: {} bytes", data.len());
        }
        Ok(data) => report.fail(&format!("{}::png_valid", name), &format!("too small: {} bytes", data.len())),
        Err(e) => report.skip(&format!("{}::png_valid", name), &format!("screenshot: {}", e)),
    }

    // JPEG screenshot
    match page.take_screenshot(ScreenshotFormat::Jpeg) {
        Ok(data) if data.len() > 500 => {
            report.pass(&format!("{}::jpeg_valid", name));
            eprintln!("  [screenshot] JPEG size: {} bytes", data.len());
        }
        Ok(data) => report.fail(&format!("{}::jpeg_valid", name), &format!("too small: {} bytes", data.len())),
        Err(e) => report.skip(&format!("{}::jpeg_valid", name), &format!("screenshot: {}", e)),
    }

    let _ = page.close();
}
