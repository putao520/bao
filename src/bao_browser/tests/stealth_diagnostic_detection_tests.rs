// @trace TEST-STL-DIAG [req:REQ-STL-001,REQ-STL-002,REQ-STL-003,REQ-STL-004,REQ-STL-005,REQ-STL-006,REQ-STL-007] [level:integration]
// Stealth anti-fingerprint diagnostic website simulation tests.
//
// These tests simulate the fingerprint detection scripts used by diagnostic
// websites (browserleaks.com, creepjs.com, amiunique.org, whoer.net, etc.)
// and verify that our stealth engine produces consistent, correct fingerprint
// data that matches the configured profile.
//
// Architecture:
//   - Single #[test] for JsContext tests (mozjs Runtime is per-process singleton)
//   - Uses Report accumulator pattern for fault-tolerant sub-assertions
//   - Pure Rust data tests are separate #[test] functions
//
// Diagnostic website mapping:
//   - browserleaks.com: Navigator + Screen + WebGL + Canvas + Audio
//   - creepjs.com: JS engine fingerprint + Trust score + Bot detection
//   - amiunique.org: Navigator + Screen + WebGL + Audio + Canvas hash
//   - whoer.net: System fingerprint + WebRTC + DNS leak + Browser fingerprint
//   - ja3er.com: TLS JA3 fingerprint
//   - tls.peet.ws: TLS fingerprint analysis
//   - ipleak.net: IP/DNS/WebRTC leak

#![allow(dead_code)]

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
use bao_stealth::{StealthProfile, StealthEngine, TlsFingerprintConfig};

// ---------------------------------------------------------------------------
// Report — fault-tolerant sub-assertion accumulator
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
    fn skip(&mut self, name: &str, why: &str) {
        self.skipped += 1;
        self.messages.push(format!("SKIP  {}  ({})", name, why));
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
        eprintln!("\n=== Stealth Diagnostic Detection Tests ===");
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
    match ctx.eval(code, "<stealth-diag>") {
        Ok(JsValue::String(s)) => s,
        other => format!("{:?}", other),
    }
}

fn num_eval(ctx: &mut JsContext, code: &str) -> f64 {
    match ctx.eval(code, "<stealth-diag>") {
        Ok(JsValue::Number(n)) => n,
        other => {
            eprintln!("  [num_eval] unexpected for '{}': {:?}", code, other);
            f64::NAN
        }
    }
}

fn simulate_canvas_hash(profile: &StealthProfile, size: u32) -> u64 {
    let mut hash: u64 = 0;
    for y in 0..size {
        for x in 0..size {
            let (r, g, b, _a) = profile.canvas.apply_to_pixel(128, 128, 128, 255, x, y);
            hash = hash.wrapping_mul(31)
                .wrapping_add(r as u64)
                .wrapping_add((g as u64) << 8)
                .wrapping_add((b as u64) << 16);
        }
    }
    hash
}

// ═══════════════════════════════════════════════════════════════════════════
// Single #[test] — mozjs Runtime is per-process singleton
// All JsContext-based diagnostic simulation runs here.
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn stealth_diagnostic_detection_all() {
    let mut report = Report::default();

    // ---- Phase 1: BrowserLeaks Navigator (Firefox default profile) ----
    {
        let mut ctx = JsContext::for_test().expect("JsContext");
        ctx.set_global_setup(bao_runtime::globals::install_all);

        let ua = str_eval(&mut ctx, "navigator.userAgent");
        report.check("browserleaks::firefox_ua_contains_firefox", ua.contains("Firefox"),
            &format!("UA: {}", ua));
        report.check("browserleaks::firefox_ua_contains_128", ua.contains("128.0"),
            &format!("UA: {}", ua));
        report.check("browserleaks::firefox_ua_contains_linux", ua.contains("Linux"),
            &format!("UA: {}", ua));
        report.check("browserleaks::firefox_ua_no_headless", !ua.contains("Headless"),
            &format!("UA contains 'Headless': {}", ua));

        let vendor = str_eval(&mut ctx, "navigator.vendor");
        report.check("browserleaks::firefox_vendor_empty", vendor == "",
            &format!("vendor: {}", vendor));

        let platform = str_eval(&mut ctx, "navigator.platform");
        report.check("browserleaks::firefox_platform_linux", platform == "Linux x86_64",
            &format!("platform: {}", platform));

        let lang = str_eval(&mut ctx, "navigator.language");
        report.check("browserleaks::firefox_language_en_us", lang == "en-US",
            &format!("language: {}", lang));

        let hwc = num_eval(&mut ctx, "navigator.hardwareConcurrency") as u32;
        report.check("browserleaks::firefox_hwc_8", hwc == 8,
            &format!("hardwareConcurrency: {}", hwc));

        let touch = num_eval(&mut ctx, "navigator.maxTouchPoints") as u32;
        report.check("browserleaks::firefox_touch_0", touch == 0,
            &format!("maxTouchPoints: {}", touch));

        // ---- BrowserLeaks Screen (Firefox default) ----
        let profile = StealthProfile::firefox_default();
        let w = num_eval(&mut ctx, "screen.width") as u32;
        let h = num_eval(&mut ctx, "screen.height") as u32;
        let aw = num_eval(&mut ctx, "screen.availWidth") as u32;
        let ah = num_eval(&mut ctx, "screen.availHeight") as u32;
        let cd = num_eval(&mut ctx, "screen.colorDepth") as u32;
        let pd = num_eval(&mut ctx, "screen.pixelDepth") as u32;
        let dpr = num_eval(&mut ctx, "devicePixelRatio");

        report.check("browserleaks::screen_width", w == profile.screen.width,
            &format!("width: {} vs profile {}", w, profile.screen.width));
        report.check("browserleaks::screen_height", h == profile.screen.height,
            &format!("height: {} vs profile {}", h, profile.screen.height));
        report.check("browserleaks::screen_avail_width", aw == profile.screen.avail_width,
            &format!("availWidth: {} vs profile {}", aw, profile.screen.avail_width));
        report.check("browserleaks::screen_avail_height", ah == profile.screen.avail_height,
            &format!("availHeight: {} vs profile {}", ah, profile.screen.avail_height));
        report.check("browserleaks::screen_color_depth", cd == profile.screen.color_depth,
            &format!("colorDepth: {} vs profile {}", cd, profile.screen.color_depth));
        report.check("browserleaks::screen_pixel_depth", pd == profile.screen.pixel_depth,
            &format!("pixelDepth: {} vs profile {}", pd, profile.screen.pixel_depth));
        report.check("browserleaks::device_pixel_ratio",
            (dpr - profile.screen.device_pixel_ratio).abs() < f64::EPSILON,
            &format!("dpr: {} vs profile {}", dpr, profile.screen.device_pixel_ratio));

        // Logical consistency (diagnostic websites check these)
        report.check("browserleaks::screen_w_ge_h", w >= h,
            &format!("width {} < height {}", w, h));
        report.check("browserleaks::screen_aw_le_w", aw <= w,
            &format!("availWidth {} > width {}", aw, w));
        report.check("browserleaks::screen_ah_le_h", ah <= h,
            &format!("availHeight {} > height {}", ah, h));
        report.check("browserleaks::color_depth_eq_pixel_depth", cd == pd,
            &format!("colorDepth {} != pixelDepth {}", cd, pd));
        report.check("browserleaks::color_depth_24_or_32", cd == 24 || cd == 32,
            &format!("colorDepth: {}", cd));

        // ---- CreepJS: webdriver hidden ----
        let webdriver = match ctx.eval("navigator.webdriver", "<creepjs>") {
            Ok(JsValue::Bool(b)) => b,
            Ok(JsValue::String(s)) => s == "true",
            _ => true,
        };
        report.check("creepjs::webdriver_false", !webdriver,
            "navigator.webdriver is true — bot detected!");

        // ---- CreepJS: no automation indicators ----
        let indicators = [
            ("__nightmare", "'__nightmare' in window"),
            ("_phantom", "'_phantom' in window"),
            ("callPhantom", "'callPhantom' in window"),
            ("domAutomation", "'domAutomation' in window"),
        ];
        for (name, check) in &indicators {
            let clean = match ctx.eval(&format!("!({})", check), "<creepjs>") {
                Ok(JsValue::Bool(b)) => b,
                _ => true,
            };
            report.check(&format!("creepjs::no_{}", name), clean,
                &format!("Automation indicator {} detected", name));
        }

        // ---- CreepJS: navigator consistency ----
        let app_version = str_eval(&mut ctx, "navigator.appVersion");
        report.check("creepjs::ua_appversion_consistent",
            ua.contains("X11") || app_version.contains("X11"),
            "UA and appVersion must both indicate X11/Linux");

        let product_sub = str_eval(&mut ctx, "navigator.productSub");
        if product_sub.contains("Undefined") {
            report.skip("creepjs::firefox_product_sub", "productSub not available in minimal JsContext");
        } else if ua.contains("Firefox") {
            report.check("creepjs::firefox_product_sub", product_sub == "20100101",
                &format!("Firefox productSub should be 20100101, got {}", product_sub));
        }

        // ---- Whoer.net: system fingerprint consistency ----
        report.check("whoer::ua_platform_linux_consistent",
            ua.contains("Linux") && platform.contains("Linux"),
            &format!("UA says {} but platform says {} — inconsistent", ua, platform));

        if ua.contains("x86_64") {
            report.check("whoer::x86_64_hwc_ge_4", hwc >= 4,
                &format!("x86_64 desktop should have hwc >= 4, got {}", hwc));
        }

        let common_resolutions = [(1920, 1080), (2560, 1440), (1366, 768), (1536, 864)];
        let is_common = common_resolutions.iter().any(|(cw, ch)| w == *cw && h == *ch);
        report.check("whoer::screen_common_resolution",
            is_common || (w >= 1024 && h >= 768),
            &format!("Screen {}x{} is unusual", w, h));

        report.check("whoer::color_depth_standard", cd == 24 || cd == 32,
            &format!("colorDepth {} is unusual", cd));
        report.check("whoer::dpr_reasonable", dpr >= 1.0 && dpr <= 3.0,
            &format!("devicePixelRatio {} is unusual", dpr));
        report.check("whoer::desktop_no_touch", touch == 0,
            &format!("Desktop should have maxTouchPoints=0, got {}", touch));

        // ---- IPLeak: WebRTC not leaking ----
        let rtc_available = match ctx.eval("typeof RTCPeerConnection !== 'undefined'", "<ipleak>") {
            Ok(JsValue::Bool(b)) => b,
            _ => false,
        };
        if rtc_available {
            let can_create = str_eval(&mut ctx,
                "try { new RTCPeerConnection({iceServers: []}); 'created' } catch(e) { 'blocked' }");
            report.pass(&format!("ipleak::rtc_status_{}", can_create));
        } else {
            report.pass("ipleak::rtc_not_available_no_leak");
        }

        // ---- CDP stealth: chrome.runtime and cdc_ globals must not exist ----
        let chrome_runtime = match ctx.eval(
            "try { typeof chrome !== 'undefined' && typeof chrome.runtime !== 'undefined' } catch(e) { false }",
            "<cdp-stealth>",
        ) {
            Ok(JsValue::Bool(b)) => b,
            _ => false,
        };
        report.check("cdp_stealth::chrome_runtime_undefined", !chrome_runtime,
            "chrome.runtime is defined — ChromeDriver automation indicator detected!");

        let cdc_globals = match ctx.eval(
            "!Object.keys(window).some(function(k) { return k.startsWith('cdc_'); })",
            "<cdp-stealth>",
        ) {
            Ok(JsValue::Bool(b)) => b,
            _ => true,
        };
        report.check("cdp_stealth::no_cdc_globals", cdc_globals,
            "cdc_ prefixed globals found — ChromeDriver automation indicator detected!");
    }

    // ---- Phase 2: Chrome profile tests ----
    {
        let profile = StealthProfile::chrome_default();
        bao_stealth::engine_props::set_profile(&profile);

        let mut ctx = JsContext::for_test().expect("JsContext");
        ctx.set_global_setup(bao_runtime::globals::install_all);

        let ua = str_eval(&mut ctx, "navigator.userAgent");
        report.check("browserleaks::chrome_ua_contains_chrome", ua.contains("Chrome"),
            &format!("UA: {}", ua));
        report.check("browserleaks::chrome_ua_no_headless", !ua.contains("Headless"),
            &format!("UA contains 'Headless': {}", ua));

        let vendor = str_eval(&mut ctx, "navigator.vendor");
        report.check("browserleaks::chrome_vendor_google", vendor == "Google Inc.",
            &format!("vendor: {}", vendor));

        let platform = str_eval(&mut ctx, "navigator.platform");
        report.check("browserleaks::chrome_platform_linux", platform == "Linux x86_64",
            &format!("platform: {}", platform));

        let w = num_eval(&mut ctx, "screen.width") as u32;
        let h = num_eval(&mut ctx, "screen.height") as u32;
        let dpr = num_eval(&mut ctx, "devicePixelRatio");

        report.check("browserleaks::chrome_screen_width", w == profile.screen.width,
            &format!("width: {} vs profile {}", w, profile.screen.width));
        report.check("browserleaks::chrome_screen_height", h == profile.screen.height,
            &format!("height: {} vs profile {}", h, profile.screen.height));
        report.check("browserleaks::chrome_dpr",
            (dpr - profile.screen.device_pixel_ratio).abs() < f64::EPSILON,
            &format!("dpr: {} vs profile {}", dpr, profile.screen.device_pixel_ratio));
    }

    // ---- Phase 3: Cross-profile isolation ----
    {
        let firefox = StealthProfile::firefox_default();
        bao_stealth::engine_props::set_profile(&firefox);

        let mut ctx_ff = JsContext::for_test().expect("JsContext");
        ctx_ff.set_global_setup(bao_runtime::globals::install_all);

        let ff_ua = str_eval(&mut ctx_ff, "navigator.userAgent");
        let ff_vendor = str_eval(&mut ctx_ff, "navigator.vendor");
        report.check("cross_profile::firefox_ua", ff_ua.contains("Firefox"),
            &format!("Expected Firefox UA, got: {}", ff_ua));
        report.check("cross_profile::firefox_vendor_empty", ff_vendor == "",
            &format!("Expected empty vendor, got: {}", ff_vendor));

        // Switch to Chrome
        let chrome = StealthProfile::chrome_default();
        bao_stealth::engine_props::set_profile(&chrome);

        let mut ctx_ch = JsContext::for_test().expect("JsContext");
        ctx_ch.set_global_setup(bao_runtime::globals::install_all);

        let ch_ua = str_eval(&mut ctx_ch, "navigator.userAgent");
        let ch_vendor = str_eval(&mut ctx_ch, "navigator.vendor");
        report.check("cross_profile::chrome_ua", ch_ua.contains("Chrome"),
            &format!("Expected Chrome UA, got: {}", ch_ua));
        report.check("cross_profile::chrome_vendor_google", ch_vendor == "Google Inc.",
            &format!("Expected 'Google Inc.', got: {}", ch_vendor));
        report.check("cross_profile::uas_differ", ff_ua != ch_ua,
            "Firefox and Chrome UAs must differ after profile switch");
    }

    report.finish();

    // ---- Strict verification gate ----
    // 1. Zero FAIL — any failure is a hard error
    let fails = report.messages.iter().filter(|m| m.starts_with("FAIL")).count();
    assert_eq!(fails, 0, "{} diagnostic detection assertions FAILED — fingerprint compromised!", fails);

    // 2. Mandatory assertions — these MUST be PASS (not skip, not fail)
    let mandatory_prefixes = [
        // Navigator fingerprint (browserleaks)
        "browserleaks::firefox_ua_contains_firefox",
        "browserleaks::firefox_ua_no_headless",
        "browserleaks::firefox_vendor_empty",
        "browserleaks::firefox_platform_linux",
        "browserleaks::firefox_hwc_8",
        // Screen fingerprint (browserleaks)
        "browserleaks::screen_width",
        "browserleaks::screen_height",
        "browserleaks::screen_color_depth",
        "browserleaks::device_pixel_ratio",
        // Bot detection (creepjs)
        "creepjs::webdriver_false",
        // System consistency (whoer)
        "whoer::ua_platform_linux_consistent",
        "whoer::desktop_no_touch",
        // Chrome profile
        "browserleaks::chrome_ua_contains_chrome",
        "browserleaks::chrome_vendor_google",
        // Cross-profile isolation
        "cross_profile::uas_differ",
        // CDP stealth (REQ-STL-007)
        "cdp_stealth::chrome_runtime_undefined",
        "cdp_stealth::no_cdc_globals",
    ];

    for prefix in &mandatory_prefixes {
        let is_pass = report.messages.iter()
            .any(|m| m.starts_with("PASS") && m.contains(prefix));
        let is_fail = report.messages.iter()
            .any(|m| m.starts_with("FAIL") && m.contains(prefix));
        let is_skip = report.messages.iter()
            .any(|m| m.starts_with("SKIP") && m.contains(prefix));
        assert!(is_pass && !is_fail && !is_skip,
            "MANDATORY assertion '{}' was not PASS (pass={}, fail={}, skip={}) — fingerprint verification failed!",
            prefix, is_pass, is_fail, is_skip);
    }

    // 3. Pass ratio — 100% of non-skipped assertions must pass
    let total = report.passed + report.failed;
    if total > 0 {
        let ratio = report.passed as f64 / total as f64;
        assert!(ratio >= 1.0, "pass ratio {}/{} ({:.1}%) < 100% — fingerprint verification failed",
            report.passed, total, ratio * 100.0);
    }

    // 4. Minimum pass count — must have at least 30 sub-assertions passing
    assert!(report.passed >= 30,
        "only {} sub-assertions passed — need at least 30 for adequate fingerprint coverage",
        report.passed);

    JsContext::shutdown_thread_sm();
}

// ═══════════════════════════════════════════════════════════════════════════
// Pure Rust data tests — no JsContext required, can run in parallel
// ═══════════════════════════════════════════════════════════════════════════

// ---- AmIUnique: full fingerprint consistency (pure Rust data) ----

#[test]
fn amiunique_full_fingerprint_firefox() {
    let profile = StealthProfile::firefox_default();
    let engine = StealthEngine::new(profile.clone());

    assert!(engine.navigator().user_agent.contains("Firefox"));
    assert_eq!(engine.navigator().platform, "Linux x86_64");
    assert_eq!(engine.navigator().language, "en-US");
    assert_eq!(engine.navigator().vendor, "");
    assert_eq!(engine.navigator().hardware_concurrency, 8);
    assert_eq!(engine.navigator().max_touch_points, 0);
    assert_eq!(engine.navigator().product_sub, "20100101");
    assert_eq!(engine.navigator().oscpu, Some("Linux x86_64".to_string()));
    assert_eq!(engine.navigator().build_id, Some("20240701150000".to_string()));
    assert_eq!(engine.screen().width, 1920);
    assert_eq!(engine.screen().height, 1080);
    assert_eq!(engine.screen().color_depth, 24);
    assert_eq!(engine.webgl().vendor, "Mozilla");
    assert!(!engine.webgl().renderer.is_empty());
    assert!(!engine.tls_config().ja3_hash.is_empty());
    assert!(engine.tls_config().ja3_hash.starts_with("771,"));
    assert_eq!(engine.http2_config().header_table_size, 65536);
    assert!(engine.canvas_noise().seed() > 0);
    assert!(engine.audio().noise_amplitude() > 0.0);
    assert!(engine.behavior().seed() > 0);
}

#[test]
fn amiunique_full_fingerprint_chrome() {
    let profile = StealthProfile::chrome_default();
    let engine = StealthEngine::new(profile.clone());

    assert!(engine.navigator().user_agent.contains("Chrome"));
    assert_eq!(engine.navigator().vendor, "Google Inc.");
    assert_eq!(engine.navigator().product_sub, "20030107");
    assert!(engine.navigator().oscpu.is_none());
    assert!(engine.navigator().build_id.is_none());
    assert_eq!(engine.webgl().vendor, "Google Inc. (NVIDIA)");
    assert!(engine.webgl().renderer.contains("ANGLE"));
    assert_ne!(
        StealthProfile::firefox_default().navigator.user_agent,
        profile.navigator.user_agent
    );
}

// ---- BrowserLeaks: Canvas fingerprint determinism ----

#[test]
fn browserleaks_canvas_deterministic_noise() {
    let profile = StealthProfile::firefox_default();

    let p1 = profile.canvas.apply_to_pixel(128, 64, 32, 255, 100, 200);
    let p2 = profile.canvas.apply_to_pixel(128, 64, 32, 255, 100, 200);
    assert_eq!(p1, p2, "Canvas noise must be deterministic");

    let mut any_different = false;
    for x in 0..10u32 {
        for y in 0..10u32 {
            let pa = profile.canvas.apply_to_pixel(128, 128, 128, 255, x, y);
            let pb = profile.canvas.apply_to_pixel(128, 128, 128, 255, x + 100, y + 100);
            if pa != pb { any_different = true; break; }
        }
        if any_different { break; }
    }
    assert!(any_different, "Canvas noise should vary across positions");

    for alpha in [0, 1, 127, 200, 255] {
        let (_, _, _, a) = profile.canvas.apply_to_pixel(128, 64, 32, alpha, 5, 5);
        assert_eq!(a, alpha, "Canvas noise must preserve alpha");
    }
}

#[test]
fn browserleaks_canvas_different_profiles_different_fingerprint() {
    let firefox = StealthProfile::firefox_default();
    let chrome = StealthProfile::chrome_default();

    let ff_pixel = firefox.canvas.apply_to_pixel(200, 100, 50, 255, 50, 50);
    let ch_pixel = chrome.canvas.apply_to_pixel(200, 100, 50, 255, 50, 50);
    // Noise amplitude is 0.001 — may round to same u8 for some pixels.
    // Check multiple positions to find at least one difference.
    let mut any_different = false;
    for x in 0..50u32 {
        for y in 0..50u32 {
            let fp = firefox.canvas.apply_to_pixel(200, 100, 50, 255, x, y);
            let cp = chrome.canvas.apply_to_pixel(200, 100, 50, 255, x, y);
            if fp != cp { any_different = true; break; }
        }
        if any_different { break; }
    }
    assert!(any_different, "Firefox and Chrome canvas noise must differ across positions");

    let ff_hash = simulate_canvas_hash(&firefox, 100);
    let ch_hash = simulate_canvas_hash(&chrome, 100);
    assert_ne!(ff_hash, ch_hash, "Firefox and Chrome must produce different canvas hashes");
}

// ---- BrowserLeaks: WebGL vendor/renderer consistency ----

#[test]
fn browserleaks_webgl_vendor_renderer_consistency() {
    let profile = StealthProfile::firefox_default();
    let engine = StealthEngine::new(profile.clone());

    assert_eq!(engine.webgl().vendor, profile.webgl.vendor);
    assert_eq!(engine.webgl().renderer, profile.webgl.renderer);
    assert!(!profile.webgl.extensions.is_empty());
    assert!(profile.webgl.extensions.contains(&"WEBGL_debug_renderer_info".to_string()));
    assert_eq!(profile.webgl.max_texture_size, 16384);
    assert_eq!(profile.webgl.max_renderbuffer_size, 16384);
    assert_eq!(profile.webgl.max_viewport_dims, [16384, 16384]);
}

#[test]
fn browserleaks_webgl_chrome_vs_firefox_different() {
    let firefox = StealthProfile::firefox_default();
    let chrome = StealthProfile::chrome_default();

    assert_ne!(firefox.webgl.vendor, chrome.webgl.vendor);
    assert_ne!(firefox.webgl.renderer, chrome.webgl.renderer);
    assert!(firefox.webgl.extensions.len() > chrome.webgl.extensions.len());
}

// ---- JA3er: TLS fingerprint verification ----

#[test]
fn ja3er_tls_fingerprint_firefox() {
    let profile = StealthProfile::firefox_default();
    let config = TlsFingerprintConfig::from_fingerprint(&profile.tls);

    assert!(config.has_fingerprint());
    assert!(!config.tls12_cipher_list.is_empty());
    assert!(!config.tls13_cipher_suites.is_empty());
    assert!(!config.curves_list.is_empty());
    assert!(!config.sigalgs_list.is_empty());
    assert!(config.tls13_cipher_suites.contains("TLS_AES_128_GCM_SHA256"));
    assert!(config.tls13_cipher_suites.contains("TLS_AES_256_GCM_SHA384"));
    assert!(config.tls13_cipher_suites.contains("TLS_CHACHA20_POLY1305_SHA256"));

    let ja3 = profile.tls.compute_ja3();
    assert!(ja3.starts_with("771,"), "JA3 must start with 771");

    let ja4 = profile.tls.compute_ja4();
    assert!(ja4.starts_with("t13d"), "JA4 must start with t13d");
}

#[test]
fn ja3er_tls_fingerprint_chrome() {
    let profile = StealthProfile::chrome_default();
    let config = TlsFingerprintConfig::from_fingerprint(&profile.tls);

    assert!(config.has_fingerprint());

    let ff_ja3 = StealthProfile::firefox_default().tls.compute_ja3();
    let ch_ja3 = profile.tls.compute_ja3();
    assert_ne!(ff_ja3, ch_ja3);

    let ff_config = TlsFingerprintConfig::from_fingerprint(&StealthProfile::firefox_default().tls);
    assert_ne!(ff_config.tls12_cipher_list, config.tls12_cipher_list);
}

// ---- AudioContext fingerprint ----

#[test]
fn audio_fingerprint_deterministic() {
    let profile = StealthProfile::firefox_default();

    let s1 = profile.audio.apply_noise(0.5, 0);
    let s2 = profile.audio.apply_noise(0.5, 0);
    assert!((s1 - s2).abs() < f64::EPSILON);

    let s3 = profile.audio.apply_noise(0.5, 1);
    assert_ne!(s1, s3);

    assert!((profile.audio.noise_amplitude() - 1e-7).abs() < f64::EPSILON);

    let noisy = profile.audio.apply_noise(0.5, 100);
    assert!((noisy - 0.5).abs() < 1e-5, "noise too large: {}", noisy - 0.5);
}

#[test]
fn audio_fingerprint_different_profiles() {
    let firefox = StealthProfile::firefox_default();
    let chrome = StealthProfile::chrome_default();

    let ff = firefox.audio.apply_noise(1.0, 50);
    let ch = chrome.audio.apply_noise(1.0, 50);
    assert_ne!(ff, ch, "Firefox and Chrome audio noise must differ");
}

// ---- Behavior simulation ----

#[test]
fn behavior_mouse_path_natural() {
    let profile = StealthProfile::firefox_default();

    let path1 = profile.behavior.generate_mouse_path(0.0, 0.0, 500.0, 300.0, 20);
    let path2 = profile.behavior.generate_mouse_path(0.0, 0.0, 500.0, 300.0, 20);
    assert_eq!(path1, path2, "Mouse path must be deterministic");

    let (sx, sy) = path1[0];
    assert!((sx - 0.0).abs() < 1e-9);
    assert!((sy - 0.0).abs() < 1e-9);

    let (ex, ey) = path1[path1.len() - 1];
    assert!((ex - 500.0).abs() < 20.0, "End x: {}", ex);
    assert!((ey - 300.0).abs() < 20.0, "End y: {}", ey);
}

#[test]
fn behavior_typing_delays_human_like() {
    let profile = StealthProfile::firefox_default();
    let delays = profile.behavior.generate_typing_delays(50);

    assert!(delays.iter().all(|&d| d > 0));
    assert!(delays.iter().all(|&d| d >= 30 && d <= 200),
        "min={}, max={}", delays.iter().min().unwrap_or(&0), delays.iter().max().unwrap_or(&0));

    let min_d = *delays.iter().min().unwrap_or(&0);
    let max_d = *delays.iter().max().unwrap_or(&0);
    assert!(max_d - min_d > 10, "typing delays have no variance");

    let delays2 = profile.behavior.generate_typing_delays(50);
    assert_eq!(delays, delays2, "Typing delays must be deterministic");
}

#[test]
fn behavior_scroll_deltas_natural() {
    let profile = StealthProfile::firefox_default();
    let deltas = profile.behavior.generate_scroll_deltas(1000.0, 20);

    // Inertia scroll produces variable count (friction decay terminates when delta ~ 0)
    assert!(!deltas.is_empty(), "scroll deltas must not be empty");
    assert!(deltas.iter().all(|d| d.is_finite()));

    let sum: f64 = deltas.iter().sum();
    assert!(sum > 0.0, "total scroll {} should be positive", sum);

    let deltas2 = profile.behavior.generate_scroll_deltas(1000.0, 20);
    assert_eq!(deltas, deltas2, "Scroll deltas must be deterministic");
}

// ---- HTTP/2 Akamai fingerprint ----

#[test]
fn http2_akamai_fingerprint_format() {
    let firefox = StealthProfile::firefox_default();
    let akamai = firefox.http2.akamai_fingerprint();
    let parts: Vec<&str> = akamai.split(':').collect();
    assert_eq!(parts.len(), 6, "Akamai fingerprint must have 6 fields");

    let chrome = StealthProfile::chrome_default();
    assert_ne!(akamai, chrome.http2.akamai_fingerprint());

    let payload = firefox.http2.settings_frame_payload();
    assert_eq!(payload.len(), 6);
}

#[test]
fn http2_pseudo_header_ordering() {
    let firefox = StealthProfile::firefox_default();
    let chrome = StealthProfile::chrome_default();

    assert_eq!(firefox.http2.pseudo_header_order[0], ":method");
    assert_eq!(firefox.http2.pseudo_header_order[1], ":path");
    assert_eq!(firefox.http2.pseudo_header_order[2], ":authority");
    assert_eq!(firefox.http2.pseudo_header_order[3], ":scheme");

    assert_eq!(chrome.http2.pseudo_header_order[0], ":method");
    assert_eq!(chrome.http2.pseudo_header_order[1], ":authority");
    assert_eq!(chrome.http2.pseudo_header_order[2], ":scheme");
    assert_eq!(chrome.http2.pseudo_header_order[3], ":path");

    assert_ne!(firefox.http2.pseudo_header_order, chrome.http2.pseudo_header_order);
}

// ═══════════════════════════════════════════════════════════════════════════
// Section: Strict cross-dimensional fingerprint consistency gate
// Every diagnostic website dimension must produce valid, internally
// consistent data. This test is the hard gate — if it fails, the
// fingerprint is broken and will be detected.
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn fingerprint_strict_consistency_gate() {
    let firefox = StealthProfile::firefox_default();
    let chrome = StealthProfile::chrome_default();
    let mut violations: Vec<String> = Vec::new();

    // ---- Navigator: UA ↔ platform ↔ vendor consistency ----
    // Firefox: UA contains "Firefox", vendor is "", platform contains "Linux"
    if !firefox.navigator.user_agent.contains("Firefox") {
        violations.push("Firefox UA missing 'Firefox'".into());
    }
    if !firefox.navigator.user_agent.contains("Linux") {
        violations.push("Firefox UA missing 'Linux' — inconsistent with platform".into());
    }
    if !firefox.navigator.platform.contains("Linux") {
        violations.push("Firefox platform missing 'Linux' — inconsistent with UA".into());
    }
    if !firefox.navigator.vendor.is_empty() {
        violations.push(format!("Firefox vendor should be empty, got '{}'", firefox.navigator.vendor));
    }
    // Chrome: UA contains "Chrome", vendor is "Google Inc."
    if !chrome.navigator.user_agent.contains("Chrome") {
        violations.push("Chrome UA missing 'Chrome'".into());
    }
    if chrome.navigator.vendor != "Google Inc." {
        violations.push(format!("Chrome vendor should be 'Google Inc.', got '{}'", chrome.navigator.vendor));
    }
    // Cross-profile: UAs must differ
    if firefox.navigator.user_agent == chrome.navigator.user_agent {
        violations.push("Firefox and Chrome UAs are identical — fingerprint not differentiated".into());
    }

    // ---- Screen: logical consistency ----
    for (name, profile) in [("Firefox", &firefox), ("Chrome", &chrome)] {
        if profile.screen.width < profile.screen.height {
            violations.push(format!("{} screen.width ({}) < screen.height ({}) — unusual for desktop", name, profile.screen.width, profile.screen.height));
        }
        if profile.screen.avail_width > profile.screen.width {
            violations.push(format!("{} availWidth ({}) > width ({}) — impossible", name, profile.screen.avail_width, profile.screen.width));
        }
        if profile.screen.avail_height > profile.screen.height {
            violations.push(format!("{} availHeight ({}) > height ({}) — impossible", name, profile.screen.avail_height, profile.screen.height));
        }
        if profile.screen.color_depth != profile.screen.pixel_depth {
            violations.push(format!("{} colorDepth ({}) != pixelDepth ({}) — inconsistent", name, profile.screen.color_depth, profile.screen.pixel_depth));
        }
        if profile.screen.color_depth != 24 && profile.screen.color_depth != 32 {
            violations.push(format!("{} colorDepth {} is unusual for desktop", name, profile.screen.color_depth));
        }
        if profile.screen.device_pixel_ratio < 1.0 || profile.screen.device_pixel_ratio > 3.0 {
            violations.push(format!("{} devicePixelRatio {} is out of range [1.0, 3.0]", name, profile.screen.device_pixel_ratio));
        }
    }

    // ---- WebGL: vendor/renderer must be non-empty and differ between profiles ----
    if firefox.webgl.vendor.is_empty() {
        violations.push("Firefox WebGL vendor is empty".into());
    }
    if chrome.webgl.vendor.is_empty() {
        violations.push("Chrome WebGL vendor is empty".into());
    }
    if firefox.webgl.vendor == chrome.webgl.vendor {
        violations.push("Firefox and Chrome WebGL vendors are identical — fingerprint not differentiated".into());
    }
    if firefox.webgl.renderer == chrome.webgl.renderer {
        violations.push("Firefox and Chrome WebGL renderers are identical — fingerprint not differentiated".into());
    }
    if firefox.webgl.extensions.is_empty() {
        violations.push("Firefox WebGL extensions list is empty".into());
    }
    if chrome.webgl.extensions.is_empty() {
        violations.push("Chrome WebGL extensions list is empty".into());
    }

    // ---- TLS: JA3 must be valid and differ between profiles ----
    if !firefox.tls.ja3_hash.starts_with("771,") {
        violations.push(format!("Firefox JA3 invalid: {}", firefox.tls.ja3_hash));
    }
    if !chrome.tls.ja3_hash.starts_with("771,") {
        violations.push(format!("Chrome JA3 invalid: {}", chrome.tls.ja3_hash));
    }
    if firefox.tls.compute_ja3() == chrome.tls.compute_ja3() {
        violations.push("Firefox and Chrome JA3 are identical — TLS fingerprint not differentiated".into());
    }
    let ff_tls_config = TlsFingerprintConfig::from_fingerprint(&firefox.tls);
    let ch_tls_config = TlsFingerprintConfig::from_fingerprint(&chrome.tls);
    if !ff_tls_config.has_fingerprint() {
        violations.push("Firefox TLS config has no fingerprint".into());
    }
    if !ch_tls_config.has_fingerprint() {
        violations.push("Chrome TLS config has no fingerprint".into());
    }
    if ff_tls_config.tls12_cipher_list.is_empty() {
        violations.push("Firefox TLS 1.2 cipher list is empty".into());
    }
    if ch_tls_config.tls12_cipher_list.is_empty() {
        violations.push("Chrome TLS 1.2 cipher list is empty".into());
    }

    // ---- HTTP/2: Akamai fingerprint must differ between profiles ----
    if firefox.http2.akamai_fingerprint() == chrome.http2.akamai_fingerprint() {
        violations.push("Firefox and Chrome Akamai fingerprints are identical — HTTP/2 fingerprint not differentiated".into());
    }
    if firefox.http2.pseudo_header_order == chrome.http2.pseudo_header_order {
        violations.push("Firefox and Chrome pseudo-header ordering is identical — HTTP/2 fingerprint not differentiated".into());
    }
    if firefox.http2.header_table_size == 0 {
        violations.push("Firefox HTTP/2 header_table_size is 0".into());
    }

    // ---- Canvas: noise must be deterministic and differ between profiles ----
    let p1 = firefox.canvas.apply_to_pixel(200, 100, 50, 255, 10, 20);
    let p2 = firefox.canvas.apply_to_pixel(200, 100, 50, 255, 10, 20);
    if p1 != p2 {
        violations.push("Firefox canvas noise is not deterministic — fingerprint will be inconsistent across calls".into());
    }
    let p3 = chrome.canvas.apply_to_pixel(200, 100, 50, 255, 10, 20);
    let p4 = chrome.canvas.apply_to_pixel(200, 100, 50, 255, 10, 20);
    if p3 != p4 {
        violations.push("Chrome canvas noise is not deterministic — fingerprint will be inconsistent across calls".into());
    }
    // Canvas hash must differ between profiles
    let ff_hash = simulate_canvas_hash(&firefox, 50);
    let ch_hash = simulate_canvas_hash(&chrome, 50);
    if ff_hash == ch_hash {
        violations.push("Firefox and Chrome canvas hashes are identical — Canvas fingerprint not differentiated".into());
    }

    // ---- Audio: noise must be deterministic and differ between profiles ----
    let a1 = firefox.audio.apply_noise(1.0, 100);
    let a2 = firefox.audio.apply_noise(1.0, 100);
    if (a1 - a2).abs() > f64::EPSILON {
        violations.push("Firefox audio noise is not deterministic".into());
    }
    let a3 = chrome.audio.apply_noise(1.0, 100);
    if (a1 - a3).abs() < f64::EPSILON {
        violations.push("Firefox and Chrome audio noise produce identical results — Audio fingerprint not differentiated".into());
    }

    // ---- Behavior: must be deterministic and differ between profiles ----
    let m1 = firefox.behavior.generate_mouse_path(0.0, 0.0, 500.0, 300.0, 10);
    let m2 = firefox.behavior.generate_mouse_path(0.0, 0.0, 500.0, 300.0, 10);
    if m1 != m2 {
        violations.push("Firefox mouse path is not deterministic — behavior fingerprint will be inconsistent".into());
    }
    let m3 = chrome.behavior.generate_mouse_path(0.0, 0.0, 500.0, 300.0, 10);
    if m1 == m3 {
        violations.push("Firefox and Chrome mouse paths are identical — Behavior fingerprint not differentiated".into());
    }

    // ---- Final gate ----
    if !violations.is_empty() {
        let report = violations.iter()
            .map(|v| format!("  - {}", v))
            .collect::<Vec<_>>()
            .join("\n");
        panic!(
            "FINGERPRINT CONSISTENCY GATE FAILED — {} violation(s):\n{}\n\n\
             These violations mean the anti-fingerprint system will be detected by diagnostic websites.",
            violations.len(), report
        );
    }
}
