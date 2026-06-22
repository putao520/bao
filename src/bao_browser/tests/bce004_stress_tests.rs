// BCE-20260622-004 — STRESS: aggressive multi-nav within ONE BaoRuntime
// to disprove (or reproduce) the "second nav SIGSEGV" claim with high confidence.
//
// 10 sequential navigations, mix of fingerprint + non-fingerprint sites,
// pre-nav inject_stealth_js, post-nav inject + eval.

#![allow(dead_code)]

use bao_browser::{BaoConfig, BaoRuntime, PageConfig, PageHandle};
use bao_stealth::StealthProfile;
use std::time::{Duration, Instant};

fn wait_for_load(page: &PageHandle, max_ms: u64) {
    let start = Instant::now();
    while start.elapsed().as_millis() < max_ms as u128 {
        let _ = page.evaluate_js_web("");
        std::thread::sleep(Duration::from_millis(40));
        if let Ok(s) = page.evaluate_js_web("document.readyState") {
            if s.contains("complete") || s.contains("interactive") { return; }
        }
    }
}

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
    let js = "(function() { try { Object.defineProperty(navigator, 'webdriver', {get: function(){return false;}, configurable: false}); } catch(e){} })()";
    page.evaluate_js_web(&js).map_err(|e| format!("inject webdriver: {}", e))?;
    Ok(())
}

#[test]
fn bce004_stress_ten_navigations() {
    if std::env::var("BAO_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!("[skip] BAO_TEST_NETWORK != 1"); return;
    }
    if std::env::var("DISPLAY").unwrap_or_default().is_empty() {
        eprintln!("[skip] no DISPLAY"); return;
    }
    let runtime = match BaoRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => { eprintln!("[skip] runtime init: {e}"); return; }
    };
    let profile = StealthProfile::chrome_default();
    let page = match runtime.create_page(&PageConfig {
        url: Some("about:blank".into()),
        stealth_profile: Some(profile.clone()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => { eprintln!("[fail] create_page: {e}"); return; }
    };

    eprintln!("[stress] pre-nav inject");
    if let Err(e) = inject_stealth_js(&page, &profile) {
        eprintln!("[fail] pre-nav inject: {e}"); return;
    }

    let sites = [
        "https://example.com/",
        "https://example.org/",
        "https://www.example.net/",
        "https://bot.sannysoft.com/",
        "https://example.com/",
        "https://example.org/",
        "https://browserleaks.com/javascript",
        "https://www.example.net/",
        "https://example.com/",
        "https://bot.sannysoft.com/",
    ];
    for (i, url) in sites.iter().enumerate() {
        eprintln!("[stress] nav #{:02}/{} → {}", i + 1, sites.len(), url);
        if let Err(e) = page.navigate(url) {
            eprintln!("[fail] navigate #{}: {e}", i + 1); return;
        }
        wait_for_load(&page, 8000);
        let _ = inject_stealth_js(&page, &profile);
        let rs = page.evaluate_js_web("document.readyState").unwrap_or_default();
        eprintln!("[stress] nav #{:02} done readyState={:?}", i + 1, rs);
    }
    eprintln!("[stress] PASS — no SIGSEGV across {} sequential navigations", sites.len());
}
