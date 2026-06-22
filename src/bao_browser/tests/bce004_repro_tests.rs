// BCE-20260622-004 SIGSEGV reproducer.
// Minimal: single BaoRuntime, two consecutive external HTTPS navigations.
// Crashes deterministically (per BCE-002-residual notes) on the SECOND navigation.
//
// Usage: cargo test --package bao_browser --test bce004_repro_tests -- --nocapture
//        with BAO_TEST_NETWORK=1 + DISPLAY=:99 (Xvfb)

#![allow(dead_code)]

use bao_browser::{BaoConfig, BaoRuntime, PageConfig};
use std::time::Duration;

fn wait_for_load(page: &bao_browser::PageHandle, max_ms: u64) {
    let start = std::time::Instant::now();
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

#[test]
fn bce004_double_external_navigation() {
    if std::env::var("BAO_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!("[skip] BAO_TEST_NETWORK != 1");
        return;
    }
    if std::env::var("DISPLAY").unwrap_or_default().is_empty() {
        eprintln!("[skip] no DISPLAY");
        return;
    }
    eprintln!("[bce004] step 1: BaoRuntime::new");
    let runtime = match BaoRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[skip] runtime init failed: {e}");
            return;
        }
    };

    eprintln!("[bce004] step 2: create_page");
    let page = match runtime.create_page(&PageConfig {
        url: Some("about:blank".into()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[fail] create_page: {e}");
            return;
        }
    };

    eprintln!("[bce004] step 3: navigate #1 → https://example.com/");
    if let Err(e) = page.navigate("https://example.com/") {
        eprintln!("[fail] navigate #1: {e}");
        return;
    }
    wait_for_load(&page, 8000);
    let r1 = page.evaluate_js_web("document.readyState").unwrap_or_default();
    eprintln!("[bce004] step 3 done: readyState={:?}", r1);

    eprintln!("[bce004] step 4: navigate #2 → https://example.org/");
    if let Err(e) = page.navigate("https://example.org/") {
        eprintln!("[fail] navigate #2: {e}");
        return;
    }
    wait_for_load(&page, 8000);
    let r2 = page.evaluate_js_web("document.readyState").unwrap_or_default();
    eprintln!("[bce004] step 4 done: readyState={:?}", r2);

    eprintln!("[bce004] step 5: navigate #3 → https://www.example.net/");
    if let Err(e) = page.navigate("https://www.example.net/") {
        eprintln!("[fail] navigate #3: {e}");
        return;
    }
    wait_for_load(&page, 8000);
    eprintln!("[bce004] PASS — no SIGSEGV after 3 navigations");
}
