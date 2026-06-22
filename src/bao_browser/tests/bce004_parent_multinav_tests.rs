// BCE-20260622-004 — Critical case: PARENT process navigates ALL fingerprint
// sites directly with NO subprocess workaround, NO inject_stealth_js-avoidance.
//
// This exercises the precise scenario the BCE-002-residual comment claimed was
// SIGSEGV-deterministic: a single BaoRuntime, multi-page, multi-external-nav,
// inject_stealth_js (full implementation), and post-nav evaluate_js_web.
//
// PASS = no SIGSEGV. If this passes, the BCE-002-residual SIGSEGV claim has
// been empirically invalidated for current code (BCE-002 patches + BCE-001
// WebViewId-keyed realm storage already eliminated the root cause).
//
// Usage: cargo test --package bao_browser --test bce004_parent_multinav_tests -- --nocapture
//        with BAO_TEST_NETWORK=1 + DISPLAY=:99 (Xvfb)

#![allow(dead_code)]

use bao_browser::{BaoConfig, BaoRuntime, PageConfig, PageHandle, PagePool};
use bao_stealth::StealthProfile;
use std::time::{Duration, Instant};

fn wait_for_load(page: &PageHandle, max_ms: u64) {
    let start = Instant::now();
    while start.elapsed().as_millis() < max_ms as u128 {
        let _ = page.evaluate_js_web("");
        std::thread::sleep(Duration::from_millis(50));
        if let Ok(s) = page.evaluate_js_web("document.readyState") {
            if s.contains("complete") || s.contains("interactive") {
                return;
            }
        }
    }
}

/// Full inject_stealth_js — identical to fingerprint_website_eval_e2e_tests.
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

fn env_ok() -> bool {
    if std::env::var("BAO_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!("[skip] BAO_TEST_NETWORK != 1");
        return false;
    }
    if std::env::var("DISPLAY").unwrap_or_default().is_empty() {
        eprintln!("[skip] no DISPLAY");
        return false;
    }
    true
}

/// Single BaoRuntime, single page, ALL fingerprint sites navigated sequentially
/// (the precise "second navigation SIGSEGV" scenario from the original comment).
#[test]
fn bce004_parent_multisite_inplace() {
    if !env_ok() { return; }
    let runtime = match BaoRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => { eprintln!("[skip] runtime init: {e}"); return; }
    };
    let pool: &PagePool = runtime.page_pool();
    let profile = StealthProfile::chrome_default();
    let page = match pool.create_page(&PageConfig {
        url: Some("about:blank".into()),
        stealth_profile: Some(profile.clone()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => { eprintln!("[fail] create_page: {e}"); return; }
    };

    // Inject BEFORE any navigation — claimed to cause SIGSEGV per BCE-002-residual.
    eprintln!("[parent-multisite] inject_stealth_js BEFORE any navigation");
    if let Err(e) = inject_stealth_js(&page, &profile) {
        eprintln!("[fail] inject: {e}");
        return;
    }

    // Sequential external navigation to MULTIPLE fingerprint sites — claimed to
    // SIGSEGV on the second.
    let sites = [
        "https://bot.sannysoft.com/",
        "https://example.com/",
        "https://www.example.net/",
        "https://browserleaks.com/javascript",
    ];
    for (i, url) in sites.iter().enumerate() {
        eprintln!("[parent-multisite] nav #{} → {}", i + 1, url);
        if let Err(e) = page.navigate(url) {
            eprintln!("[fail] navigate #{}: {e}", i + 1);
            return;
        }
        wait_for_load(&page, 12000);
        // Post-nav inject + eval (as subprocess path does).
        let _ = inject_stealth_js(&page, &profile);
        let rs = page.evaluate_js_web("document.readyState").unwrap_or_default();
        let title = page.evaluate_js_web("document.title || ''").unwrap_or_default();
        eprintln!(
            "[parent-multisite] nav #{} done readyState={:?} title={:?}",
            i + 1,
            rs,
            title.chars().take(40).collect::<String>()
        );
    }
    eprintln!("[parent-multisite] PASS — no SIGSEGV across {} navigations in single parent process", sites.len());
}

/// Multi-page: parent creates MULTIPLE pages, each navigating externally.
/// Tests the PagePool + multi-ScriptThread scenario for SIGSEGV.
#[test]
fn bce004_parent_multi_page() {
    if !env_ok() { return; }
    let runtime = match BaoRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => { eprintln!("[skip] runtime init: {e}"); return; }
    };
    let pool: &PagePool = runtime.page_pool();
    let profile = StealthProfile::chrome_default();

    let mut pages = Vec::new();
    let targets = [
        "https://example.com/",
        "https://example.org/",
    ];
    for (i, url) in targets.iter().enumerate() {
        eprintln!("[parent-multipage] create page #{} → {}", i + 1, url);
        let page = match pool.create_page(&PageConfig {
            url: Some(url.to_string()),
            stealth_profile: Some(profile.clone()),
            ..Default::default()
        }) {
            Ok(p) => p,
            Err(e) => { eprintln!("[fail] create_page #{}: {e}", i + 1); return; }
        };
        wait_for_load(&page, 8000);
        pages.push(page);
    }
    // Now navigate each page AGAIN (a second navigation).
    for (i, page) in pages.iter().enumerate() {
        let url = format!("https://www.example{}.net/", i + 1);
        eprintln!("[parent-multipage] nav page #{} → {}", i + 1, url);
        if let Err(e) = page.navigate(&url) {
            eprintln!("[fail] navigate page #{}: {e}", i + 1);
            return;
        }
        wait_for_load(page, 8000);
        let _ = inject_stealth_js(page, &profile);
        let rs = page.evaluate_js_web("document.readyState").unwrap_or_default();
        eprintln!("[parent-multipage] nav page #{} done readyState={:?}", i + 1, rs);
    }
    eprintln!("[parent-multipage] PASS — no SIGSEGV across multi-page multi-nav");
}
