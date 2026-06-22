// BCE-20260622-004 — isolating the SIGSEGV trigger.
// Test 1: single page, multi evaluate (no nav, no inject)
// Test 2: single page, multi nav (no evaluate between)
// Test 3: single page, evaluate + nav + evaluate (the original sequence)

#![allow(dead_code)]

use bao_browser::{BaoConfig, BaoRuntime, PageConfig, PageHandle};
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

fn env_ok() -> bool {
    if std::env::var("BAO_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!("[skip] BAO_TEST_NETWORK != 1"); return false;
    }
    if std::env::var("DISPLAY").unwrap_or_default().is_empty() {
        eprintln!("[skip] no DISPLAY"); return false;
    }
    true
}

#[test]
fn bce004_iso_multi_eval_no_nav() {
    if !env_ok() { return; }
    let runtime = match BaoRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => { eprintln!("[skip] init: {e}"); return; }
    };
    let page = match runtime.create_page(&PageConfig {
        url: Some("about:blank".into()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => { eprintln!("[fail] create: {e}"); return; }
    };
    // 50 evaluate_js calls (Node Realm path, filename "bao_evaluate_js")
    for i in 0..50 {
        let r = page.evaluate_js(&format!("({} + 1)", i));
        if let Err(e) = r {
            eprintln!("[fail] evaluate #{}: {e}", i); return;
        }
    }
    eprintln!("[iso-eval] PASS — 50 Node Realm evals no SIGSEGV");
}

#[test]
fn bce004_iso_multi_nav_no_eval() {
    if !env_ok() { return; }
    let runtime = match BaoRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => { eprintln!("[skip] init: {e}"); return; }
    };
    let page = match runtime.create_page(&PageConfig {
        url: Some("about:blank".into()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => { eprintln!("[fail] create: {e}"); return; }
    };
    let urls = [
        "https://example.com/",
        "https://example.org/",
        "https://www.example.net/",
    ];
    for (i, u) in urls.iter().enumerate() {
        if let Err(e) = page.navigate(u) {
            eprintln!("[fail] nav #{}: {e}", i); return;
        }
        wait_for_load(&page, 8000);
    }
    eprintln!("[iso-nav] PASS — 3 navs no SIGSEGV");
}

#[test]
fn bce004_iso_eval_nav_eval() {
    if !env_ok() { return; }
    let runtime = match BaoRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => { eprintln!("[skip] init: {e}"); return; }
    };
    let page = match runtime.create_page(&PageConfig {
        url: Some("about:blank".into()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => { eprintln!("[fail] create: {e}"); return; }
    };
    eprintln!("[iso-eval-nav-eval] eval #1");
    let _ = page.evaluate_js("1+1");
    eprintln!("[iso-eval-nav-eval] nav → https://example.com/");
    if let Err(e) = page.navigate("https://example.com/") {
        eprintln!("[fail] nav: {e}"); return;
    }
    wait_for_load(&page, 8000);
    eprintln!("[iso-eval-nav-eval] eval #2 (post-nav)");
    let r2 = page.evaluate_js("2+2");
    if let Err(e) = r2 {
        eprintln!("[fail] post-nav eval: {e}"); return;
    }
    eprintln!("[iso-eval-nav-eval] PASS — eval+nav+eval no SIGSEGV");
}

#[test]
fn bce004_iso_create_close_create() {
    if !env_ok() { return; }
    let runtime = match BaoRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => { eprintln!("[skip] init: {e}"); return; }
    };
    let pool = runtime.page_pool();
    // Create 5 pages, eval, close, repeat — accumulates destroyed Node Realms.
    for n in 0..5 {
        let page = match pool.create_page(&PageConfig {
            url: Some("about:blank".into()),
            ..Default::default()
        }) {
            Ok(p) => p,
            Err(e) => { eprintln!("[fail] create #{}: {e}", n); return; }
        };
        for i in 0..5 {
            if let Err(e) = page.evaluate_js(&format!("({} + {})", n, i)) {
                eprintln!("[fail] eval #{}.{}: {e}", n, i); return;
            }
        }
        let _ = page.close();
    }
    eprintln!("[iso-close-create] PASS — 5 create/eval/close cycles no SIGSEGV");
}
