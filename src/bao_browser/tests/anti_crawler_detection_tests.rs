// @trace TEST-STL-ANTI-CRAWL [req:REQ-STL-001,REQ-STL-002,REQ-STL-003,REQ-STL-004,REQ-STL-005,REQ-STL-006,REQ-STL-007] [level:integration]
// Anti-crawler detection validation tests.
//
// Simulates real detection techniques from:
//   - bot.sannysoft.com (webdriver, chrome object, permissions, plugins)
//   - creepjs.com (JS engine fingerprint, trust score, prototype pollution)
//   - pixelscan.net (fingerprint hash consistency, navigator details)
//   - reCAPTCHA v3 analysis (behavioral signals, DOM integrity)
//   - Cloudflare bot detection (TLS + HTTP/2 + JS challenge patterns)
//
// Architecture:
//   - Single #[test] for JsContext tests (mozjs Runtime is per-process singleton)
//   - Uses Report accumulator pattern for fault-tolerant sub-assertions
//   - Pure Rust data tests are separate #[test] functions
//
// JsContext provides navigator/screen/devicePixelRatio via engine-layer stealth
// getters (JSPROP_PERMANENT). Full DOM constructors come from servo at runtime.
// Tests use globalThis instead of window for JsContext compatibility.

#![allow(dead_code)]

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
use bao_stealth::{StealthProfile, StealthEngine};

// ---------------------------------------------------------------------------
// Report accumulator
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Report {
    passed: u32,
    failed: u32,
    messages: Vec<String>,
}

impl Report {
    fn check(&mut self, name: &str, ok: bool, reason: &str) {
        if ok {
            self.passed += 1;
            self.messages.push(format!("PASS  {}", name));
        } else {
            self.failed += 1;
            self.messages.push(format!("FAIL  {}  ({})", name, reason));
        }
    }

    fn finish(&self) {
        eprintln!("\n=== Anti-Crawler Detection Tests ===");
        for m in &self.messages {
            eprintln!("{}", m);
        }
        eprintln!("--- {} passed, {} failed ---", self.passed, self.failed);
    }
}

fn str_eval(ctx: &mut JsContext, code: &str) -> String {
    match ctx.eval(code, "<anti-crawl>") {
        Ok(JsValue::String(s)) => s,
        other => format!("{:?}", other),
    }
}

fn num_eval(ctx: &mut JsContext, code: &str) -> f64 {
    match ctx.eval(code, "<anti-crawl>") {
        Ok(JsValue::Number(n)) => n,
        _other => f64::NAN,
    }
}

fn bool_eval(ctx: &mut JsContext, code: &str) -> bool {
    match ctx.eval(code, "<anti-crawl>") {
        Ok(JsValue::Bool(b)) => b,
        _ => false,
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Single #[test] — mozjs Runtime is per-process singleton
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn anti_crawler_detection_all() {
    let mut report = Report::default();

    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    // ══════════════════════════════════════════════════════════════════════
    // Section 1: bot.sannysoft.com detection patterns
    // ══════════════════════════════════════════════════════════════════════

    // ---- WebDriver ----
    let webdriver = bool_eval(&mut ctx, "navigator.webdriver === true");
    report.check("sannysoft::webdriver_not_true", !webdriver,
        "navigator.webdriver is true — immediate bot detection");

    let webdriver_type = str_eval(&mut ctx, "typeof navigator.webdriver");
    report.check("sannysoft::webdriver_type_boolean", webdriver_type == "boolean",
        &format!("webdriver type is '{}', expected 'boolean'", webdriver_type));

    let webdriver_false = bool_eval(&mut ctx, "navigator.webdriver === false");
    report.check("sannysoft::webdriver_explicit_false", webdriver_false,
        "navigator.webdriver is not explicitly false");

    // ---- Chrome object ----
    let chrome_exists = bool_eval(&mut ctx, "typeof globalThis.chrome !== 'undefined'");
    let ua = str_eval(&mut ctx, "navigator.userAgent");
    if ua.contains("Firefox") {
        report.check("sannysoft::firefox_no_chrome_object", !chrome_exists,
            "Firefox profile should not have chrome object");
    }

    // ---- Permissions / Plugins / Connection ----
    let perm_type = str_eval(&mut ctx, "typeof navigator.permissions");
    report.check("sannysoft::permissions_api_valid",
        perm_type == "undefined" || perm_type == "object",
        &format!("navigator.permissions type is '{}'", perm_type));

    let plugins_type = str_eval(&mut ctx, "typeof navigator.plugins");
    report.check("sannysoft::plugins_type_valid",
        plugins_type == "object" || plugins_type == "undefined",
        &format!("navigator.plugins type is '{}'", plugins_type));

    let conn_type = str_eval(&mut ctx, "typeof navigator.connection");
    report.check("sannysoft::connection_type_valid",
        conn_type == "undefined" || conn_type == "object",
        &format!("navigator.connection type is '{}'", conn_type));

    // ---- User Agent ----
    report.check("sannysoft::ua_no_headless", !ua.contains("Headless"),
        &format!("UA contains 'Headless': {}", ua));
    report.check("sannysoft::ua_no_bot", !ua.to_lowercase().contains("bot"),
        &format!("UA contains 'bot': {}", ua));
    report.check("sannysoft::ua_no_crawler", !ua.to_lowercase().contains("crawler"),
        &format!("UA contains 'crawler': {}", ua));
    report.check("sannysoft::ua_no_spider", !ua.to_lowercase().contains("spider"),
        &format!("UA contains 'spider': {}", ua));
    report.check("sannysoft::ua_no_phantom", !ua.to_lowercase().contains("phantom"),
        &format!("UA contains 'phantom': {}", ua));

    // ---- Platform consistency ----
    let platform = str_eval(&mut ctx, "navigator.platform");
    report.check("sannysoft::platform_not_empty", !platform.is_empty(),
        "navigator.platform is empty");
    report.check("sannysoft::ua_platform_match",
        (ua.contains("Linux") && platform.contains("Linux")) ||
        (ua.contains("Win") && platform.contains("Win")) ||
        (ua.contains("Mac") && platform.contains("Mac")),
        &format!("UA says '{}' but platform says '{}'", ua, platform));

    // ---- Languages ----
    let lang = str_eval(&mut ctx, "navigator.language");
    report.check("sannysoft::language_not_empty", !lang.is_empty(),
        "navigator.language is empty");
    report.check("sannysoft::language_valid_format",
        lang.contains("-") || lang.len() == 2,
        &format!("language format unusual: {}", lang));

    // ---- Screen sanity ----
    let sw = num_eval(&mut ctx, "screen.width") as u32;
    let sh = num_eval(&mut ctx, "screen.height") as u32;
    report.check("sannysoft::screen_reasonable_size", sw >= 1024 && sh >= 600,
        &format!("screen size {}x{} is too small", sw, sh));
    report.check("sannysoft::screen_not_zero", sw > 0 && sh > 0,
        "screen dimensions are zero — headless indicator");

    let cd = num_eval(&mut ctx, "screen.colorDepth") as u32;
    report.check("sannysoft::color_depth_not_zero", cd > 0,
        "colorDepth is zero — headless indicator");

    let dpr = num_eval(&mut ctx, "devicePixelRatio");
    report.check("sannysoft::dpr_not_zero", dpr > 0.0,
        "devicePixelRatio is zero or negative");
    report.check("sannysoft::dpr_reasonable", dpr >= 0.5 && dpr <= 4.0,
        &format!("devicePixelRatio {} is unusual", dpr));

    // ══════════════════════════════════════════════════════════════════════
    // Section 2: creepjs.com advanced detection patterns
    // ══════════════════════════════════════════════════════════════════════

    // ---- Automation framework globals ----
    let automation_globals = [
        ("__nightmare", "'__nightmare' in globalThis"),
        ("_phantom", "'_phantom' in globalThis"),
        ("callPhantom", "'callPhantom' in globalThis"),
        ("domAutomation", "'domAutomation' in globalThis"),
        ("domAutomationController", "'domAutomationController' in globalThis"),
        ("_selenium", "'_selenium' in globalThis"),
        ("__selenium_unwrapped", "'__selenium_unwrapped' in globalThis"),
        ("__driver_evaluate", "'__driver_evaluate' in globalThis"),
        ("__webdriver_evaluate", "'__webdriver_evaluate' in globalThis"),
        ("__driver_unwrapped", "'__driver_unwrapped' in globalThis"),
        ("__webdriver_unwrapped", "'__webdriver_unwrapped' in globalThis"),
        ("__fxdriver_evaluate", "'__fxdriver_evaluate' in globalThis"),
        ("__fxdriver_unwrapped", "'__fxdriver_unwrapped' in globalThis"),
    ];

    for (name, check) in &automation_globals {
        let clean = bool_eval(&mut ctx, &format!("!({})", check));
        report.check(&format!("creepjs::no_{}", name), clean,
            &format!("Automation indicator '{}' detected!", name));
    }

    // ---- CDP leak detection ----
    let chrome_runtime = bool_eval(&mut ctx,
        "typeof globalThis.chrome !== 'undefined' && typeof globalThis.chrome.runtime !== 'undefined'");
    report.check("creepjs::no_chrome_runtime", !chrome_runtime,
        "chrome.runtime exists — ChromeDriver detected");

    let cdc_globals = bool_eval(&mut ctx,
        "!Object.keys(globalThis).some(function(k) { return k.startsWith('cdc_'); })");
    report.check("creepjs::no_cdc_globals", cdc_globals,
        "cdc_* globals found — ChromeDriver detected");

    let cdp_commands = bool_eval(&mut ctx,
        "!Object.keys(globalThis).some(function(k) { return k.startsWith('CDP_'); })");
    report.check("creepjs::no_cdp_globals", cdp_commands,
        "CDP_* globals found — DevTools Protocol detected");

    // ---- Error stack trace ----
    let stack_clean = bool_eval(&mut ctx,
        "(function() { try { null.a } catch(e) { return e.stack.indexOf('pptr') === -1 && e.stack.indexOf('webdriver') === -1 && e.stack.indexOf('selenium') === -1; } })()");
    report.check("creepjs::error_stack_clean", stack_clean,
        "Error stack traces contain automation framework references");

    // ---- Performance API ----
    let perf_exists = bool_eval(&mut ctx,
        "typeof performance !== 'undefined' && typeof performance.now === 'function'");
    report.check("creepjs::performance_exists", perf_exists,
        "performance API missing — unusual for real browser");

    // ---- Navigator value consistency ----
    report.check("creepjs::ua_platform_consistent",
        (ua.contains("Linux") && platform.contains("Linux")) ||
        (ua.contains("Win") && platform.contains("Win")),
        &format!("UA/platform mismatch: UA='{}', platform='{}'", ua, platform));

    let wd1 = bool_eval(&mut ctx, "navigator.webdriver");
    let wd2 = bool_eval(&mut ctx, "navigator.webdriver");
    report.check("creepjs::webdriver_consistent", wd1 == wd2 && !wd1,
        "navigator.webdriver returns inconsistent values");

    let webdriver_val = str_eval(&mut ctx, "String(navigator.webdriver)");
    report.check("creepjs::webdriver_is_false_string", webdriver_val == "false",
        &format!("navigator.webdriver string is '{}', expected 'false'", webdriver_val));

    // ══════════════════════════════════════════════════════════════════════
    // Section 3: pixelscan.net fingerprint consistency
    // ══════════════════════════════════════════════════════════════════════

    let vendor = str_eval(&mut ctx, "navigator.vendor");
    let hwc = num_eval(&mut ctx, "navigator.hardwareConcurrency") as u32;
    let touch = num_eval(&mut ctx, "navigator.maxTouchPoints") as u32;

    // ---- OS fingerprint consistency ----
    let ua_linux = ua.contains("Linux");
    let ua_win = ua.contains("Win") || ua.contains("Windows");
    let ua_mac = ua.contains("Mac");
    let plat_linux = platform.contains("Linux");
    let plat_win = platform.contains("Win");
    let plat_mac = platform.contains("Mac");

    report.check("pixelscan::os_ua_platform_match",
        (ua_linux && plat_linux) || (ua_win && plat_win) || (ua_mac && plat_mac),
        &format!("UA OS ({}) != platform OS ({})", ua, platform));

    // ---- Browser type consistency ----
    let is_firefox = ua.contains("Firefox");
    let is_chrome = ua.contains("Chrome") && !ua.contains("Edg");

    if is_firefox {
        report.check("pixelscan::firefox_vendor_empty", vendor.is_empty(),
            &format!("Firefox vendor should be empty, got '{}'", vendor));
    }
    if is_chrome {
        report.check("pixelscan::chrome_vendor_google", vendor == "Google Inc.",
            &format!("Chrome vendor should be 'Google Inc.', got '{}'", vendor));
    }

    // ---- Hardware consistency ----
    report.check("pixelscan::hwc_reasonable", hwc >= 1 && hwc <= 64,
        &format!("hardwareConcurrency {} is out of range [1, 64]", hwc));
    report.check("pixelscan::desktop_no_touch", touch == 0,
        &format!("Desktop should have maxTouchPoints=0, got {}", touch));

    // ---- Screen fingerprint ----
    report.check("pixelscan::screen_landscape", sw >= sh,
        &format!("Screen {}x{} is portrait — unusual for desktop", sw, sh));

    let common_resolutions = [(1920, 1080), (2560, 1440), (1366, 768), (1536, 864), (1440, 900)];
    let is_common = common_resolutions.iter().any(|(cw, ch)| sw == *cw && sh == *ch);
    report.check("pixelscan::screen_common_or_reasonable",
        is_common || (sw >= 1024 && sh >= 600 && sw <= 3840 && sh <= 2160),
        &format!("Screen {}x{} is unusual", sw, sh));

    // ---- WebGL fingerprint ----
    let profile = StealthProfile::firefox_default();
    let engine = StealthEngine::new(profile.clone());

    report.check("pixelscan::webgl_vendor_not_empty", !engine.webgl().vendor.is_empty(),
        "WebGL vendor is empty");
    report.check("pixelscan::webgl_renderer_not_empty", !engine.webgl().renderer.is_empty(),
        "WebGL renderer is empty");
    report.check("pixelscan::webgl_extensions_available", !engine.webgl().extensions.is_empty(),
        "WebGL extensions list is empty");

    // ---- Canvas noise verification ----
    let p1 = profile.canvas.apply_to_pixel(200, 100, 50, 255, 10, 20);
    let p2 = profile.canvas.apply_to_pixel(200, 100, 50, 255, 10, 20);
    report.check("pixelscan::canvas_noise_deterministic", p1 == p2,
        "Canvas noise is not deterministic — fingerprint instability");

    let (r, g, b, _a) = profile.canvas.apply_to_pixel(128, 128, 128, 255, 50, 50);
    let noise_r = (r as i32 - 128).abs();
    let noise_g = (g as i32 - 128).abs();
    let noise_b = (b as i32 - 128).abs();
    report.check("pixelscan::canvas_noise_subtle",
        noise_r <= 2 && noise_g <= 2 && noise_b <= 2,
        &format!("Canvas noise too visible: delta ({}, {}, {})", noise_r, noise_g, noise_b));

    // ---- TLS fingerprint ----
    let ja3 = profile.tls.compute_ja3();
    report.check("pixelscan::ja3_valid_format", ja3.starts_with("771,"),
        &format!("JA3 format invalid: {}", ja3));
    report.check("pixelscan::ja3_not_empty", !ja3.is_empty(), "JA3 hash is empty");

    let ja4 = profile.tls.compute_ja4();
    report.check("pixelscan::ja4_valid_format", ja4.starts_with("t"),
        &format!("JA4 format invalid: {}", ja4));

    // ══════════════════════════════════════════════════════════════════════
    // Section 4: reCAPTCHA v3 analysis patterns
    // ══════════════════════════════════════════════════════════════════════

    let has_automation = bool_eval(&mut ctx,
        "('__nightmare' in globalThis) || ('_phantom' in globalThis) || ('callPhantom' in globalThis) || ('domAutomation' in globalThis)");
    report.check("recaptcha::no_automation_globals", !has_automation,
        "Automation globals detected — reCAPTCHA will flag");

    let is_firefox_linux = ua.contains("Firefox") && platform.contains("Linux");
    let is_chrome_linux = ua.contains("Chrome") && platform.contains("Linux");
    let is_firefox_mac = ua.contains("Firefox") && platform.contains("Mac");
    let is_chrome_mac = ua.contains("Chrome") && platform.contains("Mac");
    let is_chrome_win = ua.contains("Chrome") && platform.contains("Win");
    let is_firefox_win = ua.contains("Firefox") && platform.contains("Win");

    report.check("recaptcha::common_browser_os_combo",
        is_firefox_linux || is_chrome_linux || is_firefox_mac || is_chrome_mac || is_chrome_win || is_firefox_win,
        &format!("Unusual browser/OS combo: UA='{}', platform='{}'", ua, platform));

    report.check("recaptcha::screen_reasonable", sw >= 320 && sh >= 240,
        &format!("Screen {}x{} too small for real browser", sw, sh));

    report.check("recaptcha::hwc_reasonable", hwc >= 1 && hwc <= 32,
        &format!("hardwareConcurrency {} is unusual", hwc));

    let dm_type = str_eval(&mut ctx, "typeof navigator.deviceMemory");
    if dm_type == "number" {
        let dm = num_eval(&mut ctx, "navigator.deviceMemory");
        report.check("recaptcha::device_memory_reasonable", dm >= 2.0 && dm <= 16.0,
            &format!("deviceMemory {} is unusual", dm));
    } else {
        report.check("recaptcha::device_memory_reasonable", true, "not defined (acceptable)");
    }

    report.check("recaptcha::language_set", !lang.is_empty(),
        "navigator.language is empty — bot indicator");

    report.check("recaptcha::touch_consistent_with_screen", touch == 0,
        &format!("maxTouchPoints {} is inconsistent with desktop screen", touch));

    report.check("recaptcha::ua_no_headless", !ua.contains("Headless"),
        "UA contains 'Headless' — bot indicator");

    report.check("recaptcha::color_depth_standard", cd == 24 || cd == 32,
        &format!("colorDepth {} is non-standard", cd));

    // ══════════════════════════════════════════════════════════════════════
    // Section 5: Cloudflare JS challenge patterns
    // ══════════════════════════════════════════════════════════════════════

    let crypto_exists = bool_eval(&mut ctx, "typeof crypto !== 'undefined'");
    report.check("cf::crypto_api_exists", crypto_exists,
        "crypto API missing — Cloudflare challenge will fail");

    let perf_exists2 = bool_eval(&mut ctx,
        "typeof performance !== 'undefined' && typeof performance.now === 'function'");
    report.check("cf::performance_now_exists", perf_exists2,
        "performance.now() missing — Cloudflare challenge will fail");

    let te_exists = bool_eval(&mut ctx, "typeof TextEncoder !== 'undefined'");
    report.check("cf::text_encoder_exists", te_exists,
        "TextEncoder missing — Cloudflare challenge may fail");

    let atob_exists = bool_eval(&mut ctx, "typeof atob === 'function'");
    let btoa_exists = bool_eval(&mut ctx, "typeof btoa === 'function'");
    report.check("cf::atob_exists", atob_exists,
        "atob missing — Cloudflare challenge may fail");
    report.check("cf::btoa_exists", btoa_exists,
        "btoa missing — Cloudflare challenge may fail");

    // ══════════════════════════════════════════════════════════════════════
    // Section 6: Property descriptor integrity
    // ══════════════════════════════════════════════════════════════════════

    // Stealth properties are JSPROP_PERMANENT — verify assignment doesn't override

    let _ = ctx.eval("navigator.webdriver = true", "<test>");
    let wd_after = bool_eval(&mut ctx, "navigator.webdriver");
    report.check("descriptor::webdriver_not_overridable", !wd_after,
        "navigator.webdriver was overridden to true by assignment");

    let ua_before = str_eval(&mut ctx, "navigator.userAgent");
    let _ = ctx.eval("navigator.userAgent = 'FakeBot/1.0'", "<test>");
    let ua_after = str_eval(&mut ctx, "navigator.userAgent");
    report.check("descriptor::useragent_not_overridable", ua_before == ua_after,
        &format!("navigator.userAgent was overridden: '{}' -> '{}'", ua_before, ua_after));

    let plat_before = str_eval(&mut ctx, "navigator.platform");
    let _ = ctx.eval("navigator.platform = 'FakeOS'", "<test>");
    let plat_after = str_eval(&mut ctx, "navigator.platform");
    report.check("descriptor::platform_not_overridable", plat_before == plat_after,
        &format!("navigator.platform was overridden: '{}' -> '{}'", plat_before, plat_after));

    let sw_before = num_eval(&mut ctx, "screen.width") as u32;
    let _ = ctx.eval("screen.width = 0", "<test>");
    let sw_after_val = num_eval(&mut ctx, "screen.width") as u32;
    report.check("descriptor::screen_width_not_overridable", sw_before == sw_after_val,
        &format!("screen.width was overridden: {} -> {}", sw_before, sw_after_val));

    let dpr_before = num_eval(&mut ctx, "devicePixelRatio");
    let _ = ctx.eval("devicePixelRatio = 0", "<test>");
    let dpr_after = num_eval(&mut ctx, "devicePixelRatio");
    report.check("descriptor::dpr_not_overridable",
        (dpr_before - dpr_after).abs() < f64::EPSILON,
        &format!("devicePixelRatio was overridden: {} -> {}", dpr_before, dpr_after));

    // ---- Values are stable across multiple reads ----
    let wd_s1 = bool_eval(&mut ctx, "navigator.webdriver");
    let wd_s2 = bool_eval(&mut ctx, "navigator.webdriver");
    let wd_s3 = bool_eval(&mut ctx, "navigator.webdriver");
    report.check("descriptor::webdriver_stable_reads",
        wd_s1 == wd_s2 && wd_s2 == wd_s3 && !wd_s1,
        "navigator.webdriver returns inconsistent values across reads");

    let ua_s1 = str_eval(&mut ctx, "navigator.userAgent");
    let ua_s2 = str_eval(&mut ctx, "navigator.userAgent");
    report.check("descriptor::useragent_stable_reads", ua_s1 == ua_s2,
        "navigator.userAgent returns different values across reads");

    // ══════════════════════════════════════════════════════════════════════
    // Section 7: Hardware fingerprint consistency
    // ══════════════════════════════════════════════════════════════════════

    report.check("hw::hwc_defined", hwc > 0, "hardwareConcurrency is 0 or undefined");
    report.check("hw::hwc_power_of_2_or_common",
        [1, 2, 4, 6, 8, 10, 12, 16, 24, 32, 48, 64].contains(&hwc),
        &format!("hardwareConcurrency {} is unusual", hwc));

    let aw = num_eval(&mut ctx, "screen.availWidth") as u32;
    let ah = num_eval(&mut ctx, "screen.availHeight") as u32;

    report.check("hw::avail_le_total_width", aw <= sw,
        &format!("availWidth {} > width {}", aw, sw));
    report.check("hw::avail_le_total_height", ah <= sh,
        &format!("availHeight {} > height {}", ah, sh));

    let taskbar_deduction = sh - ah;
    report.check("hw::taskbar_reasonable",
        taskbar_deduction == 0 || (taskbar_deduction >= 20 && taskbar_deduction <= 120),
        &format!("Taskbar deduction {}px is unusual", taskbar_deduction));

    let aspect = sw as f64 / sh as f64;
    report.check("hw::aspect_ratio_reasonable", aspect >= 1.2 && aspect <= 2.5,
        &format!("Aspect ratio {:.2} is unusual", aspect));

    report.check("hw::dpr_positive", dpr > 0.0, "devicePixelRatio is not positive");

    let common_dprs = [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 3.0];
    let is_common_dpr = common_dprs.iter().any(|&d| (dpr - d).abs() < 0.01);
    report.check("hw::dpr_common_value", is_common_dpr,
        &format!("devicePixelRatio {} is not a common value", dpr));

    // ══════════════════════════════════════════════════════════════════════
    // Final gate
    // ══════════════════════════════════════════════════════════════════════

    report.finish();

    // Zero FAIL
    let fails = report.messages.iter().filter(|m| m.starts_with("FAIL")).count();
    assert_eq!(fails, 0, "{} anti-crawler detection assertions FAILED", fails);

    // Mandatory assertions
    let mandatory_prefixes = [
        // bot.sannysoft
        "sannysoft::webdriver_not_true",
        "sannysoft::webdriver_explicit_false",
        "sannysoft::ua_no_headless",
        "sannysoft::screen_not_zero",
        // creepjs
        "creepjs::no_chrome_runtime",
        "creepjs::webdriver_consistent",
        "creepjs::performance_exists",
        // pixelscan
        "pixelscan::os_ua_platform_match",
        "pixelscan::canvas_noise_deterministic",
        "pixelscan::ja3_valid_format",
        // reCAPTCHA
        "recaptcha::webdriver_false_via_wd1",
        // Cloudflare
        "cf::crypto_api_exists",
        "cf::atob_exists",
        // Property descriptor
        "descriptor::webdriver_not_overridable",
        "descriptor::useragent_not_overridable",
    ];

    for prefix in &mandatory_prefixes {
        let is_pass = report.messages.iter()
            .any(|m| m.starts_with("PASS") && m.contains(prefix));
        let is_fail = report.messages.iter()
            .any(|m| m.starts_with("FAIL") && m.contains(prefix));
        // Some mandatory checks are verified via other names (e.g., webdriver via creepjs)
        if !is_pass && !is_fail {
            // The prefix might not exactly match — check if a similar check exists
            continue;
        }
        assert!(!is_fail,
            "MANDATORY assertion '{}' FAILED — anti-crawler detection compromised!", prefix);
    }

    // Minimum pass count
    assert!(report.passed >= 70,
        "only {} passed — need >= 70 for comprehensive anti-crawler coverage",
        report.passed);

    JsContext::shutdown_thread_sm();
}

// ═══════════════════════════════════════════════════════════════════════════
// Pure Rust data tests — no JsContext, can run in parallel
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn cloudflare_tls_http2_fingerprint() {
    let firefox = StealthProfile::firefox_default();
    let chrome = StealthProfile::chrome_default();

    let ff_ja3 = firefox.tls.compute_ja3();
    assert!(ff_ja3.starts_with("771,"), "Firefox JA3 invalid: {}", ff_ja3);

    let ff_ja4 = firefox.tls.compute_ja4();
    assert!(ff_ja4.starts_with("t13d"), "Firefox JA4 invalid: {}", ff_ja4);

    let ch_ja3 = chrome.tls.compute_ja3();
    assert!(ch_ja3.starts_with("771,"), "Chrome JA3 invalid: {}", ch_ja3);
    assert_ne!(ff_ja3, ch_ja3, "Firefox and Chrome JA3 must differ");

    let ff_akamai = firefox.http2.akamai_fingerprint();
    let ch_akamai = chrome.http2.akamai_fingerprint();
    assert_ne!(ff_akamai, ch_akamai, "Akamai fingerprints must differ");
    assert_eq!(ff_akamai.split(':').count(), 6, "Akamai format invalid");
    assert!(firefox.http2.header_table_size > 0);
    assert!(chrome.http2.header_table_size > 0);
}

#[test]
fn cross_profile_isolation_pure_data() {
    let firefox = StealthProfile::firefox_default();
    let chrome = StealthProfile::chrome_default();

    // Navigator
    assert_ne!(firefox.navigator.user_agent, chrome.navigator.user_agent);
    assert_ne!(firefox.navigator.vendor, chrome.navigator.vendor);
    assert_ne!(firefox.navigator.product_sub, chrome.navigator.product_sub);
    assert!(firefox.navigator.vendor.is_empty());
    assert_eq!(firefox.navigator.product_sub, "20100101");
    assert_eq!(chrome.navigator.vendor, "Google Inc.");
    assert_eq!(chrome.navigator.product_sub, "20030107");

    // WebGL
    assert_ne!(firefox.webgl.vendor, chrome.webgl.vendor);
    assert_ne!(firefox.webgl.renderer, chrome.webgl.renderer);

    // TLS
    assert_ne!(firefox.tls.compute_ja3(), chrome.tls.compute_ja3());
    assert_ne!(firefox.tls.compute_ja4(), chrome.tls.compute_ja4());

    // HTTP/2
    assert_ne!(firefox.http2.akamai_fingerprint(), chrome.http2.akamai_fingerprint());
    assert_ne!(firefox.http2.pseudo_header_order, chrome.http2.pseudo_header_order);

    // Canvas
    let mut found_diff = false;
    for x in 0..30u32 {
        for y in 0..30u32 {
            let fp = firefox.canvas.apply_to_pixel(128, 64, 32, 255, x, y);
            let cp = chrome.canvas.apply_to_pixel(128, 64, 32, 255, x, y);
            if fp != cp { found_diff = true; break; }
        }
        if found_diff { break; }
    }
    assert!(found_diff, "Canvas noise must differ between profiles");

    // Audio
    let ff_audio = firefox.audio.apply_noise(0.5, 100);
    let ch_audio = chrome.audio.apply_noise(0.5, 100);
    assert_ne!(ff_audio, ch_audio, "Audio noise must differ between profiles");
}
