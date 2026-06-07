// @trace TEST-STL-DIAG [req:REQ-STL-007] [level:integration]
// CDP stealth diagnostic tests for REQ-STL-007.
// Verifies that chrome.runtime and cdc_adoQpoasnfa76pfcZLmcfl_* globals
// are not exposed after stealth property installation.
//
// Architecture:
//   - Single #[test] for JsContext tests (mozjs Runtime is per-process singleton)
//   - Uses Report accumulator pattern for fault-tolerant sub-assertions
//   - Pure Rust data tests are separate #[test] functions

#![allow(dead_code)]

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
use bao_stealth::StealthProfile;

// ---------------------------------------------------------------------------
// Report -- fault-tolerant sub-assertion accumulator
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
    fn fail(&mut self, name: &str, why: &str) {
        self.failed += 1;
        self.messages.push(format!("FAIL  {}  ({})", name, why));
    }
    fn check(&mut self, name: &str, condition: bool, reason: &str) {
        if condition {
            self.pass(name);
        } else {
            self.fail(name, reason);
        }
    }
    fn finish(&self) {
        eprintln!("\n=== CDP Stealth Diagnostic Tests ===");
        for m in &self.messages {
            eprintln!("{}", m);
        }
        eprintln!("--- {} passed, {} skipped, {} failed ---", self.passed, self.skipped, self.failed);
    }
}

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

fn str_eval(ctx: &mut JsContext, code: &str) -> String {
    match ctx.eval(code, "<cdp-stealth>") {
        Ok(JsValue::String(s)) => s,
        other => format!("{:?}", other),
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Single #[test] -- mozjs Runtime is per-process singleton
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn cdp_stealth_diagnostic_all() {
    let mut report = Report::default();

    // ---- Phase 1: Firefox profile CDP stealth ----
    {
        let mut ctx = JsContext::for_test().expect("JsContext");
        ctx.set_global_setup(bao_runtime::globals::install_all);

        // chrome.runtime must not be accessible
        let chrome_runtime = match ctx.eval(
            "try { typeof chrome !== 'undefined' && typeof chrome.runtime !== 'undefined' } catch(e) { false }",
            "<cdp-stealth>",
        ) {
            Ok(JsValue::Bool(b)) => b,
            _ => false,
        };
        report.check("cdp_stealth::firefox_chrome_runtime_undefined", !chrome_runtime,
            "chrome.runtime is defined in Firefox profile -- ChromeDriver indicator detected!");

        // No cdc_ prefixed globals
        let no_cdc = match ctx.eval(
            "!Object.keys(window).some(function(k) { return k.startsWith('cdc_'); })",
            "<cdp-stealth>",
        ) {
            Ok(JsValue::Bool(b)) => b,
            _ => true,
        };
        report.check("cdp_stealth::firefox_no_cdc_globals", no_cdc,
            "cdc_ prefixed globals found in Firefox profile -- ChromeDriver indicator detected!");

        // Verify stealth profile is active (sanity check)
        let ua = str_eval(&mut ctx, "navigator.userAgent");
        report.check("cdp_stealth::firefox_profile_active", ua.contains("Firefox"),
            &format!("Expected Firefox UA, got: {}", ua));
    }

    // ---- Phase 2: Chrome profile CDP stealth ----
    {
        let profile = StealthProfile::chrome_default();
        bao_stealth::engine_props::set_profile(&profile);

        let mut ctx = JsContext::for_test().expect("JsContext");
        ctx.set_global_setup(bao_runtime::globals::install_all);

        let chrome_runtime = match ctx.eval(
            "try { typeof chrome !== 'undefined' && typeof chrome.runtime !== 'undefined' } catch(e) { false }",
            "<cdp-stealth>",
        ) {
            Ok(JsValue::Bool(b)) => b,
            _ => false,
        };
        report.check("cdp_stealth::chrome_chrome_runtime_undefined", !chrome_runtime,
            "chrome.runtime is defined in Chrome profile -- ChromeDriver indicator detected!");

        let no_cdc = match ctx.eval(
            "!Object.keys(window).some(function(k) { return k.startsWith('cdc_'); })",
            "<cdp-stealth>",
        ) {
            Ok(JsValue::Bool(b)) => b,
            _ => true,
        };
        report.check("cdp_stealth::chrome_no_cdc_globals", no_cdc,
            "cdc_ prefixed globals found in Chrome profile -- ChromeDriver indicator detected!");

        let ua = str_eval(&mut ctx, "navigator.userAgent");
        report.check("cdp_stealth::chrome_profile_active", ua.contains("Chrome"),
            &format!("Expected Chrome UA, got: {}", ua));
    }

    report.finish();

    // ---- Strict verification gate ----
    let fails = report.messages.iter().filter(|m| m.starts_with("FAIL")).count();
    assert_eq!(fails, 0, "{} CDP stealth assertions FAILED!", fails);

    // Mandatory assertions
    let mandatory_prefixes = [
        "cdp_stealth::firefox_chrome_runtime_undefined",
        "cdp_stealth::firefox_no_cdc_globals",
        "cdp_stealth::chrome_chrome_runtime_undefined",
        "cdp_stealth::chrome_no_cdc_globals",
    ];
    for prefix in &mandatory_prefixes {
        let is_pass = report.messages.iter()
            .any(|m| m.starts_with("PASS") && m.contains(prefix));
        assert!(is_pass,
            "MANDATORY assertion '{}' was not PASS -- CDP stealth verification failed!", prefix);
    }

    assert!(report.passed >= 6,
        "only {} sub-assertions passed -- need at least 6 for CDP stealth coverage",
        report.passed);

    JsContext::shutdown_thread_sm();
}

// ═══════════════════════════════════════════════════════════════════════════
// Pure Rust data tests -- CDP stealth profile validation
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn cdp_stealth_profile_no_cdc_artifacts() {
    let firefox = StealthProfile::firefox_default();
    let chrome = StealthProfile::chrome_default();

    // Verify profiles themselves don't contain cdc_ indicators
    let ua_ff = &firefox.navigator.user_agent;
    let ua_ch = &chrome.navigator.user_agent;
    assert!(!ua_ff.contains("cdc_"), "Firefox UA contains cdc_ prefix");
    assert!(!ua_ch.contains("cdc_"), "Chrome UA contains cdc_ prefix");
    assert!(!ua_ff.contains("chromedriver"), "Firefox UA contains chromedriver");
    assert!(!ua_ch.contains("chromedriver"), "Chrome UA contains chromedriver");
}

#[test]
fn cdp_stealth_webgl_extensions_no_cdc_indicators() {
    let firefox = StealthProfile::firefox_default();
    let chrome = StealthProfile::chrome_default();

    for ext in &firefox.webgl.extensions {
        assert!(!ext.contains("cdc_"),
            "Firefox WebGL extension contains cdc_ prefix: {}", ext);
    }
    for ext in &chrome.webgl.extensions {
        assert!(!ext.contains("cdc_"),
            "Chrome WebGL extension contains cdc_ prefix: {}", ext);
    }
}
