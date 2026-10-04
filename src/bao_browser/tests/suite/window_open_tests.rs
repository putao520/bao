// @trace TEST-LIB-001-WINDOW-OPEN [req:REQ-LIB-001] [level:e2e]
// window.open popup landing E2E (REQ-LIB-001): the
// `BaoWebViewDelegate::request_create_new` no-op root fix.
//
// Root cause chain (e46 attribution, V-verified): the no-op delegate method
// let the constellation answer the opener ScriptThread's blocked creation
// channel with None → vendor windowproxy.rs `recv().unwrap()?` → JS null.
// Every test here pins one face of the fix:
//   1. same-origin open → non-null Window + pooled child page, evaluable
//      after the pump-side deferred init (`init_pending_pages`);
//   2. cross-origin open (opaque data:-URL origins both sides) → non-null;
//   3. pool-limit exhaustion → JS null (Chromium resource-exhaustion
//      semantics — the delegate drops the creation request);
//   4. `window.close()` from page JS → WebViewClosed → pool entry retired
//      (notify_closed path, criterion ⑤).
//
// Graceful strategy (suite convention): requires BAO_TEST_REAL_SERVO=1 and a
// display server; absent either → skip (never a false red).

#![allow(dead_code)]

use bao_browser::{BaoConfig, BrowserRuntime, PageConfig, PagePool};

const OPENER_HTML: &str = "data:text/html;charset=utf-8,<html><body>opener</body></html>";
const CROSS_HTML: &str = "data:text/html;charset=utf-8,<html><body>cross</body></html>";

fn runtime_or_skip(tag: &str) -> Option<BrowserRuntime> {
    runtime_or_skip_with(tag, BaoConfig::default())
}

fn runtime_or_skip_with(tag: &str, config: BaoConfig) -> Option<BrowserRuntime> {
    if std::env::var("BAO_TEST_REAL_SERVO").as_deref() != Ok("1") {
        eprintln!("[skip] {tag}: BAO_TEST_REAL_SERVO != 1");
        return None;
    }
    if std::env::var("DISPLAY").is_err() && std::env::var("WAYLAND_DISPLAY").is_err() {
        eprintln!("[skip] {tag}: no DISPLAY or WAYLAND_DISPLAY");
        return None;
    }
    match BrowserRuntime::new(config) {
        Ok(runtime) => Some(runtime),
        Err(e) => {
            eprintln!("[skip] {tag}: BrowserRuntime::new failed: {e}");
            None
        }
    }
}

fn create_opener(pool: &PagePool) -> bao_browser::PageHandle {
    pool.create_page(&PageConfig {
        url: Some(OPENER_HTML.into()),
        ..Default::default()
    })
    .expect("opener page creation")
}

/// ① window.open (same-origin target `about:blank`) returns a live Window,
/// the popup is pooled accounting-wise, becomes evaluable after the deferred
/// pump-side init, and an explicit close reclaims the accounting.
#[test]
fn window_open_same_origin_returns_window_and_child_page_evaluable() {
    let Some(runtime) = runtime_or_skip("window_open_same_origin") else {
        return;
    };
    let pool: &PagePool = runtime.page_pool();
    let opener = create_opener(pool);

    let before = pool.stats();
    let result = opener
        .evaluate_js_web("String(window.open('about:blank') !== null)")
        .expect("opener evaluate");
    assert_eq!(
        result, "true",
        "window.open() must return a non-null Window (pre-fix: null) — got {result}"
    );

    // ② accounting: the popup is a pooled page the moment the open resolves.
    let after = pool.stats();
    assert_eq!(
        after.total_created,
        before.total_created + 1,
        "popup must be adopted into pool accounting"
    );
    assert_eq!(after.active, before.active + 1, "popup lands in active_pages");
    let live = pool.live_page_ids();
    assert_eq!(
        live.len(),
        before.active + before.idle + 1,
        "live_page_ids must include the popup"
    );

    // Deferred init (pump-side drain): pipeline ready + the single injection
    // entry — the popup becomes a fully initialized pool page.
    let initialized = pool.init_pending_pages();
    assert_eq!(initialized, 1, "exactly one popup awaited deferred init");
    assert_eq!(pool.init_pending_pages(), 0, "drain is empty afterwards");

    let child_id = live
        .into_iter()
        .find(|id| *id != opener.id())
        .expect("child page id in pool");
    let child = pool.get_page(child_id).expect("child handle after init");
    let child_result = child
        .evaluate_js_web("'pong-' + (2 + 3)")
        .expect("child page evaluate");
    assert_eq!(child_result, "pong-5", "child page must be evaluable");

    // Explicit close reclaims the accounting.
    pool.close_page(child_id).expect("close child page");
    let closed = pool.stats();
    assert_eq!(closed.active, before.active, "popup removed from active");
    assert_eq!(
        closed.total_destroyed,
        before.total_destroyed + 1,
        "popup close bumps total_destroyed"
    );
    assert!(
        !pool.live_page_ids().contains(&child_id),
        "closed popup must leave live_page_ids"
    );

    pool.close_all();
}

/// ② cross-origin open: both documents carry opaque (never-equal) origins, so
/// the returned WindowProxy is the cross-origin state — still non-null (the
/// embedder ask is origin-independent; servo has no popup blocker gate).
#[test]
fn window_open_cross_origin_returns_window() {
    let Some(runtime) = runtime_or_skip("window_open_cross_origin") else {
        return;
    };
    let pool: &PagePool = runtime.page_pool();
    let opener = create_opener(pool);

    let before = pool.stats();
    let result = opener
        .evaluate_js_web(&format!(
            "String(window.open('{CROSS_HTML}') !== null)"
        ))
        .expect("opener evaluate");
    assert_eq!(
        result, "true",
        "cross-origin window.open() must return a non-null Window — got {result}"
    );
    assert_eq!(pool.stats().total_created, before.total_created + 1);

    assert_eq!(pool.init_pending_pages(), 1);
    pool.close_all();
}

/// ④ pool-limit exhaustion: the delegate drops the creation request → the
/// script side's blocked recv closes → JS null. Chromium maps resource
/// exhaustion to exactly this; a panic or hang here would be a regression.
#[test]
fn window_open_denied_at_pool_limit_returns_null() {
    let config = BaoConfig {
        max_pages: 1,
        ..BaoConfig::default()
    };
    let Some(runtime) = runtime_or_skip_with("window_open_pool_limit", config) else {
        return;
    };
    let pool: &PagePool = runtime.page_pool();
    let opener = create_opener(pool);
    assert_eq!(pool.stats().total_created, 1, "opener fills the pool");

    let result = opener
        .evaluate_js_web("String(window.open('about:blank') !== null)")
        .expect("opener evaluate");
    assert_eq!(
        result, "false",
        "pool-limit window.open() must return null (Chromium semantics) — got {result}"
    );
    assert_eq!(
        pool.stats().total_created, 1,
        "denied open must not touch pool accounting"
    );
    assert!(pool.init_pending_pages() == 0, "nothing pending after denial");

    pool.close_all();
}

/// ⑤ window.close() from page JS: servo notifies the embedder
/// (WebViewClosed → notify_closed) and the pool entry is retired through the
/// regular close path — keyeds cleaned up, accounting drops.
#[test]
fn window_close_via_page_js_retires_pool_entry() {
    let Some(runtime) = runtime_or_skip("window_close_retires_pool_entry") else {
        return;
    };
    let pool: &PagePool = runtime.page_pool();
    let opener = create_opener(pool);

    let before = pool.stats();
    let result = opener
        .evaluate_js_web("String(window.open('about:blank') !== null)")
        .expect("opener evaluate");
    assert_eq!(result, "true");
    assert_eq!(pool.init_pending_pages(), 1);

    let child_id = pool
        .live_page_ids()
        .into_iter()
        .find(|id| *id != opener.id())
        .expect("child page id");

    let child = pool.get_page(child_id).expect("child handle");
    let _ = child.evaluate_js_web("window.close(); 'closing'");

    // The WebViewClosed embedder message is delivered on the next spin —
    // pump briefly (the loop also drains any pending popup inits).
    runtime.pump_cdp(std::time::Duration::from_millis(500));

    assert!(
        !pool.live_page_ids().contains(&child_id),
        "window.close() must retire the popup from pool accounting"
    );
    let closed = pool.stats();
    assert_eq!(
        closed.total_destroyed,
        before.total_destroyed + 1,
        "notify_closed path must destroy the popup exactly once"
    );
    assert_eq!(closed.active, before.active, "opener unaffected");

    pool.close_all();
}
