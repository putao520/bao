// @trace REQ-BRW-002 [req:REQ-BRW-002] [level:e2e]
// e80 — module type "css"/"text" end-to-end over the in-memory rendering
// integration surface (fork autonomy, upstream c406ade26 + c2e5e5452 +
// c001c1e49 ported onto the SM153 fork).
//
// Each test drives a real page through the module pipeline:
// fetch (destination mapped from module type) → module_type_allowed gate →
// compile (CreateDefaultExportSyntheticModule / CSSStyleSheet+ReplaceSync) →
// evaluate (default export observable from page script).

use bao_browser::{BaoConfig, BrowserRuntime, PageConfig, PagePool};
use std::time::{Duration, Instant};

fn wait_for_load_and_drain(pool_page: &bao_browser::PageHandle, max_ms: u64) {
    let start = Instant::now();
    while start.elapsed().as_millis() < max_ms as u128 {
        let _ = pool_page.evaluate_js("");
        if matches!(
            pool_page.get_state(),
            bao_browser::PageState::Interactive | bao_browser::PageState::Idle
        ) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn fresh_page<'a>(runtime: &'a BrowserRuntime) -> bao_browser::PageHandle {
    let pool: &PagePool = runtime.page_pool();
    pool.create_page(&PageConfig {
        url: Some("data:text/html,<!DOCTYPE html><html><body></body></html>".into()),
        ..Default::default()
    })
    .expect("pool.create_page failed")
}

/// Poll a page-global until it stops being `undefined` (module evaluation
/// latches it), returning the evaluated string.
fn poll_global(page: &bao_browser::PageHandle, name: &str, max_ms: u64) -> String {
    let start = Instant::now();
    loop {
        if let Ok(value) = page.evaluate_js_web(&format!("String(globalThis.{name})")) {
            if value != "undefined" {
                return value;
            }
        }
        assert!(
            start.elapsed().as_millis() < max_ms as u128,
            "global {name} never latched, last value: {:?}",
            page.evaluate_js_web(&format!("String(globalThis.{name})"))
                .unwrap_or_default()
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// <https://html.spec.whatwg.org/multipage/#creating-a-text-module-script>
// A text module's default export is the fetched text: import arrival →
// module_type_allowed("text") → CreateDefaultExportSyntheticModule(string) →
// the module body observes the string as `default`.
#[test]
fn text_module_import_compiles_and_evaluates_default_export() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let page = fresh_page(&runtime);
    let html = r#"data:text/html,<!DOCTYPE html><html><body><script type="module">
        import t from "data:text/plain,hello-text-module" with { type: "text" };
        globalThis.__textDefault = t;
        globalThis.__textType = typeof t;
    </script></body></html>"#;
    page.navigate(html).expect("navigate");
    wait_for_load_and_drain(&page, 30000);

    let kind = poll_global(&page, "__textType", 30000);
    assert_eq!(kind, "string", "text module default export must be a string");
    let value = poll_global(&page, "__textDefault", 10000);
    assert_eq!(
        value, "hello-text-module",
        "text module default export must be the fetched text, byte-exact"
    );
}

/// <https://html.spec.whatwg.org/multipage/#creating-a-css-module-script>
// A CSS module's default export is a constructed CSSStyleSheet whose rules
// were synchronously replaced from the fetched text.
#[test]
fn css_module_import_yields_constructed_stylesheet() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let page = fresh_page(&runtime);
    let html = r#"data:text/html,<!DOCTYPE html><html><body><script type="module">
        import sheet from "data:text/css,body{color:red}" with { type: "css" };
        globalThis.__cssIsSheet = sheet instanceof CSSStyleSheet;
        globalThis.__cssRules = sheet.cssRules.length;
    </script></body></html>"#;
    page.navigate(html).expect("navigate");
    wait_for_load_and_drain(&page, 30000);

    let is_sheet = poll_global(&page, "__cssIsSheet", 30000);
    assert_eq!(
        is_sheet, "true",
        "css module default export must be a CSSStyleSheet"
    );
    let rules = poll_global(&page, "__cssRules", 10000);
    assert_eq!(
        rules, "1",
        "ReplaceSync must populate exactly one rule from the fetched css text"
    );
}

/// Negative control for the gate: only the intended types were opened.
/// An unrecognised `type` attribute must still raise the spec TypeError
/// (module type allowed steps), not silently load.
#[test]
fn unknown_module_type_still_rejected_by_allowed_steps() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let page = fresh_page(&runtime);
    wait_for_load_and_drain(&page, 30000);

    page.evaluate_js_web(
        r#"
        globalThis.__rej = "";
        import("data:text/plain,x", { with: { type: "bogus-type" } })
            .catch(e => { globalThis.__rej = String(e.message); });
    "#,
    )
    .expect("dynamic import eval must succeed");

    let start = Instant::now();
    let rejection = loop {
        let value = page
            .evaluate_js_web("String(globalThis.__rej)")
            .expect("poll eval must succeed");
        if !value.is_empty() && value != "undefined" {
            break value;
        }
        assert!(
            start.elapsed().as_millis() < 30000,
            "unknown module type import must reject, got: {value:?}"
        );
        std::thread::sleep(Duration::from_millis(25));
    };

    assert!(
        rejection.contains("module type"),
        "unknown module type must raise the module-type-allowed TypeError, got: {rejection:?}"
    );
}
