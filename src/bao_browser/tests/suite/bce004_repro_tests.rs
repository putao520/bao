// BCE-20260622-004 SIGSEGV reproducer.
// Minimal: single BrowserRuntime, two consecutive external HTTPS navigations.
// Crashes deterministically (per BCE-002-residual notes) on the SECOND navigation.
//
// Usage: cargo test --package bao_browser --test bce004_repro_tests -- --nocapture
//        with BAO_TEST_NETWORK=1 + DISPLAY=:99 (Xvfb)

#![allow(dead_code)]

use bao_browser::{BaoConfig, BrowserRuntime, PageConfig};
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

#[ignore = "[env-gate] 需要 真外网 example.com/example.org/example.net + DISPLAY(servo WebRender GL-based); 真跑: pgrep -x Xvfb || Xvfb :99 -screen 0 1280x720x24 -ac -nolisten tcp & 后 BAO_TEST_NETWORK=1 DISPLAY=:99 cargo nextest run --run-ignored=all -p bao-browser"]
#[test]
fn bce004_double_external_navigation() {
    eprintln!("[bce004] step 1: BrowserRuntime::new");
    let runtime = match BrowserRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => {
            panic!("runtime init failed: {e}");
        }
    };

    eprintln!("[bce004] step 2: create_page");
    let page = match runtime.create_page(&PageConfig {
        url: Some("about:blank".into()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            panic!("create_page: {e}");
        }
    };

    eprintln!("[bce004] step 3: navigate #1 → https://example.com/");
    if let Err(e) = page.navigate("https://example.com/") {
        panic!("navigate #1: {e}");
    }
    wait_for_load(&page, 8000);
    let r1 = page
        .evaluate_js_web("document.readyState")
        .unwrap_or_default();
    eprintln!("[bce004] step 3 done: readyState={:?}", r1);

    eprintln!("[bce004] step 4: navigate #2 → https://example.org/");
    if let Err(e) = page.navigate("https://example.org/") {
        panic!("navigate #2: {e}");
    }
    wait_for_load(&page, 8000);
    let r2 = page
        .evaluate_js_web("document.readyState")
        .unwrap_or_default();
    eprintln!("[bce004] step 4 done: readyState={:?}", r2);

    eprintln!("[bce004] step 5: navigate #3 → https://www.example.net/");
    if let Err(e) = page.navigate("https://www.example.net/") {
        panic!("navigate #3: {e}");
    }
    wait_for_load(&page, 8000);
    eprintln!("[bce004] PASS — no SIGSEGV after 3 navigations");
}
