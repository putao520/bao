// @trace TEST-STL-E2E [req:REQ-STL-007] [level:e2e]
// SM-EVOLUTION #28 (verdict consumed 2026-09-10, user ruling — REQ-STL
// existing identity-consistency defect fix): host locale / timezone / time
// precision identity-leak eradication, LIVE page proof.
//
// Pre-fix leak state (captured on the 2026-09-10 dev host, zh-CN / UTC+8):
// a page's `Intl` default locale derived from the HOST environment
// (LANG/LC_* → ICU → zh-CN), every Date local-time computation ran in the
// host zone (`getTimezoneOffset() === -480`), and the DOM timestamp grid
// was servo's 10µs native tell — all three orthogonal to the profile.
//
// Post-fix contract, asserted here on live servo pages:
//   ① Profiled page (chrome_default): UTC+0 at January AND July instants
//      (no host/DST leakage), `Intl` default locale === profile locale
//      (en-US), navigator.language === en-US (JS-hook layer agrees),
//      and the profile's 100µs grid holds on BOTH time layers.
//   ② Per-page override: locale → en-GB and precision → 1s reproduce on the
//      SAME surfaces (Intl default follows the override; Date.now quantized
//      to whole seconds engine-natively; performance.now on the same 1s
//      grid — the two-layer consistency requirement).
//   ③ Stealth-free page (profile: None): host-derived state is RESTORED —
//      a stealthed page earlier in the process must not leak its policy
//      onto a stealth-free page (the observed host values are printed as
//      the standing leak-state evidence; exact values are host-dependent
//      and only weakly asserted).
//
// Engine sink granularity (documented in bao_engine::realm_policy): locale
// is runtime-scoped (per ScriptThread context) and the precision grids are
// process-wide — page ②'s overrides land AFTER page ①'s assertions, which
// is also why the test's page order is part of its contract.
//
// Usage:
//   BAO_TEST_NETWORK=1 xvfb-run cargo nt -p bao-browser \
//     -E 'test(stealth_identity_locale_tz)'

#![allow(dead_code)]

#[path = "common/mod.rs"]
mod common;

use std::sync::Mutex;
use std::time::{Duration, Instant};

use bao_browser::{BaoConfig, BaoRuntime, PageConfig};
use bao_stealth::StealthProfile;

/// Serializes servo-touching phases inside one isolated test process.
static TEST_SERIALIZER: Mutex<()> = Mutex::new(());

fn should_skip() -> bool {
    if std::env::var("BAO_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!("[skip] BAO_TEST_NETWORK != 1");
        return true;
    }
    if std::env::var("DISPLAY").unwrap_or_default().is_empty() {
        eprintln!("[skip] no DISPLAY");
        return true;
    }
    false
}

/// Drive servo's ScriptThread with a no-op evaluate so page-realm async
/// dispatches progress (the pump pattern every live harness here uses).
fn pump(page: &bao_browser::PageHandle, ms: u64) {
    let deadline = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < deadline {
        let _ = page.evaluate_js("");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Evaluate a boolean JS expression in the page realm.
fn eval_bool(page: &bao_browser::PageHandle, js: &str) -> bool {
    match page.evaluate_js_web(js) {
        Ok(s) => {
            let s = s.trim().trim_matches('"');
            s == "true"
        },
        Err(e) => {
            eprintln!("[eval-err] {e}: {js}");
            false
        },
    }
}

/// Evaluate a JS expression whose result is a string, returning it with the
/// servo bridge's outer quoting stripped.
fn eval_str(page: &bao_browser::PageHandle, js: &str) -> String {
    match page.evaluate_js_web(js) {
        Ok(s) => {
            let mut s = s.trim().to_string();
            if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
                s = s[1..s.len() - 1].replace("\\\"", "\"").replace("\\\\", "\\");
            }
            s
        },
        Err(e) => {
            eprintln!("[eval-err] {e}: {js}");
            String::new()
        },
    }
}

#[test]
fn stealth_identity_locale_tz_precision_live() {
    if !common::run_isolated(
        "stealth_identity_locale_tz_tests::stealth_identity_locale_tz_precision_live",
    ) {
        return;
    }
    if should_skip() {
        return;
    }
    let _guard = TEST_SERIALIZER.lock().unwrap_or_else(|e| e.into_inner());

    // ── ① Profiled page: default identity en-US / UTC / 100µs ──────────
    let runtime = BaoRuntime::new(BaoConfig::default())
        .expect("gated live test: BaoRuntime::new must succeed");
    let page_a = runtime
        .create_page(&PageConfig {
            url: None,
            stealth_profile: Some(StealthProfile::chrome_default()),
            ..Default::default()
        })
        .expect("gated live test: create_page (profiled) must succeed");
    pump(&page_a, 300);

    // Timezone: UTC+0 at a January instant — under the host zone (+0800 on
    // the leak-state host) this is -480; under forceUTC it is 0 by
    // construction.
    assert!(
        eval_bool(
            &page_a,
            "new Date(1672531200000).getTimezoneOffset() === 0"
        ),
        "① profiled page must report UTC offset 0 (January instant), got {}",
        eval_str(&page_a, "new Date(1672531200000).getTimezoneOffset()")
    );
    // ...and at a July instant — excludes host DST leakage.
    assert!(
        eval_bool(
            &page_a,
            "new Date(1688169600000).getTimezoneOffset() === 0"
        ),
        "① profiled page must report UTC offset 0 (July instant)"
    );
    let tz_name = eval_str(
        &page_a,
        "Intl.DateTimeFormat().resolvedOptions().timeZone",
    );
    eprintln!("[identity] ① Intl resolved timeZone under forceUTC = {tz_name}");

    // Locale: the ENGINE sink (JS_SetDefaultLocale) owns the Intl default —
    // the pre-fix zh-CN host leak surface.
    let intl_locale =
        eval_str(&page_a, "Intl.DateTimeFormat().resolvedOptions().locale");
    assert_eq!(
        intl_locale, "en-US",
        "① Intl default locale must follow the profile (en-US), got {intl_locale}"
    );
    // The JS-hook navigator layer must agree with the engine layer.
    assert_eq!(
        eval_str(&page_a, "navigator.language"),
        "en-US",
        "① navigator.language (hook layer) must agree with the engine locale"
    );

    // Precision (default 100µs): performance.now must sit on the 0.1ms grid
    // (bun's native performance — the surface the page actually observes —
    // quantizes from the same profile field as the engine Date clamp and
    // servo's DOM grid). Epsilon 0.01 on the t*10 scale: bun's
    // performance.now is epoch-based (~1.8e12 ms) where f64 representation
    // of a 0.1ms grid value costs up to ~0.005, while the pre-fix raw value
    // carried a 40µs tail (0.4 on this scale) and a 10µs grid would land on
    // 0.1 — both still fail this assertion.
    assert!(
        eval_bool(
            &page_a,
            "(function(){ var t = performance.now(); \
               return Math.abs(t * 10 - Math.round(t * 10)) < 0.01; })()"
        ),
        "① performance.now must be quantized to the 100µs grid, got {}",
        eval_str(&page_a, "performance.now()")
    );

    // ── ② Per-page override: en-GB locale + 1s precision ───────────────
    let mut override_profile = StealthProfile::chrome_default();
    override_profile.locale.locale = "en-GB".into();
    override_profile.timing.precision_us = 1_000_000;
    let page_b = runtime
        .create_page(&PageConfig {
            url: None,
            stealth_profile: Some(override_profile),
            ..Default::default()
        })
        .expect("gated live test: create_page (override) must succeed");
    pump(&page_b, 300);

    let intl_locale_b =
        eval_str(&page_b, "Intl.DateTimeFormat().resolvedOptions().locale");
    assert_eq!(
        intl_locale_b, "en-GB",
        "② per-page locale override must reproduce in the Intl default, got {intl_locale_b}"
    );
    // Timezone dimension untouched by the override (still default UTC).
    assert!(
        eval_bool(
            &page_b,
            "new Date(1688169600000).getTimezoneOffset() === 0"
        ),
        "② override page keeps the default UTC timezone"
    );
    // Engine Date layer at the overridden 1s grid.
    assert!(
        eval_bool(&page_b, "Date.now() % 1000 === 0"),
        "② Date.now must be quantized to the 1s override grid, got {}",
        eval_str(&page_b, "Date.now()")
    );
    // DOM performance layer on the SAME 1s grid — two-layer consistency.
    assert!(
        eval_bool(&page_b, "performance.now() % 1000 === 0"),
        "② performance.now must sit on the same 1s grid as Date.now, got {}",
        eval_str(&page_b, "performance.now()")
    );

    // ── ③ Stealth-free page: host-derived state restored ───────────────
    let page_c = runtime
        .create_page(&PageConfig {
            url: None,
            stealth_profile: None,
            ..Default::default()
        })
        .expect("gated live test: create_page (stealth-free) must succeed");
    pump(&page_c, 300);

    let host_locale =
        eval_str(&page_c, "Intl.DateTimeFormat().resolvedOptions().locale");
    let host_offset =
        eval_str(&page_c, "new Date(1672531200000).getTimezoneOffset()");
    eprintln!(
        "[identity] ③ stealth-free page host state: Intl locale={host_locale} tz offset={host_offset} \
         (leak-state evidence — the pre-fix profiled page showed these same host values)"
    );
    assert!(
        !host_locale.is_empty(),
        "③ stealth-free page must still resolve an Intl locale"
    );
    assert!(
        eval_bool(
            &page_c,
            "isFinite(new Date(1672531200000).getTimezoneOffset())"
        ),
        "③ stealth-free page must report a finite (host) tz offset"
    );
}
