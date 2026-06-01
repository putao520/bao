// @trace TEST-STL-PROFILE-COMPOSITION [req:REQ-STL-007] [level:unit]
// StealthProfile deep composition and cross-component consistency tests:
// component independence, Firefox vs Chrome full differentiation, clone isolation,
// Debug format coverage, seed consistency, cross-component string format validation,
// default values sanity, profile roundtrip.

use bao_stealth::{
    StealthProfile, StealthEngine, TlsFingerprint, Http2Fingerprint,
    CanvasNoise, NavigatorProfile, ScreenProfile, WebGLProfile, AudioProfile,
    BehaviorSimulator,
};

// ===========================================================================
// §1 Component independence — modifying one component does not affect others
// ===========================================================================

#[test]
fn test_replace_tls_does_not_affect_navigator() {
    let mut profile = StealthProfile::firefox_default();
    let original_ua = profile.navigator.user_agent.clone();
    let original_vendor = profile.navigator.vendor.clone();
    profile.tls = TlsFingerprint::chrome();
    assert_eq!(profile.navigator.user_agent, original_ua);
    assert_eq!(profile.navigator.vendor, original_vendor);
}

#[test]
fn test_replace_http2_does_not_affect_canvas() {
    let mut profile = StealthProfile::firefox_default();
    let original_seed = profile.canvas.seed();
    let original_pixel = profile.canvas.apply_to_pixel(128, 128, 128, 255, 5, 5);
    profile.http2 = Http2Fingerprint::chrome();
    assert_eq!(profile.canvas.seed(), original_seed);
    assert_eq!(profile.canvas.apply_to_pixel(128, 128, 128, 255, 5, 5), original_pixel);
}

#[test]
fn test_replace_navigator_does_not_affect_webgl() {
    let mut profile = StealthProfile::firefox_default();
    let original_vendor = profile.webgl.vendor.clone();
    let original_renderer = profile.webgl.renderer.clone();
    let original_ext_count = profile.webgl.extensions.len();
    profile.navigator = NavigatorProfile::chrome();
    assert_eq!(profile.webgl.vendor, original_vendor);
    assert_eq!(profile.webgl.renderer, original_renderer);
    assert_eq!(profile.webgl.extensions.len(), original_ext_count);
}

#[test]
fn test_replace_screen_does_not_affect_audio() {
    let mut profile = StealthProfile::firefox_default();
    let original_seed = profile.audio.seed();
    let original_sample = profile.audio.apply_noise(0.5, 10);
    profile.screen = ScreenProfile::new(3840, 2160, 2.0);
    assert_eq!(profile.audio.seed(), original_seed);
    assert_eq!(profile.audio.apply_noise(0.5, 10), original_sample);
}

#[test]
fn test_replace_webgl_does_not_affect_behavior() {
    let mut profile = StealthProfile::firefox_default();
    let original_seed = profile.behavior.seed();
    let original_delays = profile.behavior.generate_typing_delays(5);
    profile.webgl = WebGLProfile::chrome();
    assert_eq!(profile.behavior.seed(), original_seed);
    assert_eq!(profile.behavior.generate_typing_delays(5), original_delays);
}

#[test]
fn test_replace_audio_does_not_affect_tls() {
    let mut profile = StealthProfile::firefox_default();
    let original_ja3 = profile.tls.compute_ja3();
    let original_ja4 = profile.tls.compute_ja4();
    profile.audio = AudioProfile::new(999);
    assert_eq!(profile.tls.compute_ja3(), original_ja3);
    assert_eq!(profile.tls.compute_ja4(), original_ja4);
}

#[test]
fn test_replace_behavior_does_not_affect_http2() {
    let mut profile = StealthProfile::firefox_default();
    let original_akamai = profile.http2.akamai_fingerprint();
    let original_window = profile.http2.initial_window_size;
    profile.behavior = BehaviorSimulator::new(999);
    assert_eq!(profile.http2.akamai_fingerprint(), original_akamai);
    assert_eq!(profile.http2.initial_window_size, original_window);
}

#[test]
fn test_replace_canvas_does_not_affect_screen() {
    let mut profile = StealthProfile::firefox_default();
    let original_width = profile.screen.width;
    let original_height = profile.screen.height;
    let original_dpr = profile.screen.device_pixel_ratio;
    profile.canvas = CanvasNoise::new(777);
    assert_eq!(profile.screen.width, original_width);
    assert_eq!(profile.screen.height, original_height);
    assert_eq!(profile.screen.device_pixel_ratio, original_dpr);
}

// ===========================================================================
// §2 Firefox vs Chrome full profile differentiation — all 8 components differ
// ===========================================================================

#[test]
fn test_firefox_vs_chrome_tls_differ() {
    let ff = StealthProfile::firefox_default();
    let ch = StealthProfile::chrome_default();
    assert_ne!(ff.tls.compute_ja3(), ch.tls.compute_ja3(),
        "Firefox and Chrome should have different TLS JA3 fingerprints");
}

#[test]
fn test_firefox_vs_chrome_http2_differ() {
    let ff = StealthProfile::firefox_default();
    let ch = StealthProfile::chrome_default();
    assert_ne!(ff.http2.akamai_fingerprint(), ch.http2.akamai_fingerprint(),
        "Firefox and Chrome should have different HTTP/2 Akamai fingerprints");
}

#[test]
fn test_firefox_vs_chrome_canvas_seed_differ() {
    let ff = StealthProfile::firefox_default();
    let ch = StealthProfile::chrome_default();
    assert_ne!(ff.canvas.seed(), ch.canvas.seed(),
        "Firefox and Chrome should have different canvas noise seeds");
}

#[test]
fn test_firefox_vs_chrome_navigator_differ() {
    let ff = StealthProfile::firefox_default();
    let ch = StealthProfile::chrome_default();
    assert_ne!(ff.navigator.user_agent, ch.navigator.user_agent,
        "Firefox and Chrome should have different user agents");
    assert_ne!(ff.navigator.vendor, ch.navigator.vendor,
        "Firefox and Chrome should have different navigator.vendor");
}

#[test]
fn test_firefox_vs_chrome_screen_same() {
    let ff = StealthProfile::firefox_default();
    let ch = StealthProfile::chrome_default();
    // Both use ScreenProfile::default() — same screen is expected
    assert_eq!(ff.screen.width, ch.screen.width);
    assert_eq!(ff.screen.height, ch.screen.height);
    assert_eq!(ff.screen.device_pixel_ratio, ch.screen.device_pixel_ratio);
}

#[test]
fn test_firefox_vs_chrome_webgl_differ() {
    let ff = StealthProfile::firefox_default();
    let ch = StealthProfile::chrome_default();
    assert_ne!(ff.webgl.vendor, ch.webgl.vendor,
        "Firefox and Chrome should have different WebGL vendors");
    assert_ne!(ff.webgl.renderer, ch.webgl.renderer,
        "Firefox and Chrome should have different WebGL renderers");
}

#[test]
fn test_firefox_vs_chrome_audio_seed_differ() {
    let ff = StealthProfile::firefox_default();
    let ch = StealthProfile::chrome_default();
    assert_ne!(ff.audio.seed(), ch.audio.seed(),
        "Firefox and Chrome should have different audio seeds");
}

#[test]
fn test_firefox_vs_chrome_behavior_seed_differ() {
    let ff = StealthProfile::firefox_default();
    let ch = StealthProfile::chrome_default();
    assert_ne!(ff.behavior.seed(), ch.behavior.seed(),
        "Firefox and Chrome should have different behavior seeds");
}

#[test]
fn test_firefox_vs_chrome_7_of_8_components_differ() {
    // Screen is the only component that is the same (both use ScreenProfile::default())
    // All other 7 components should differ
    let ff = StealthProfile::firefox_default();
    let ch = StealthProfile::chrome_default();
    let tls_differs = ff.tls.compute_ja3() != ch.tls.compute_ja3();
    let http2_differs = ff.http2.akamai_fingerprint() != ch.http2.akamai_fingerprint();
    let canvas_differs = ff.canvas.seed() != ch.canvas.seed();
    let nav_differs = ff.navigator.user_agent != ch.navigator.user_agent;
    let webgl_differs = ff.webgl.vendor != ch.webgl.vendor;
    let audio_differs = ff.audio.seed() != ch.audio.seed();
    let behavior_differs = ff.behavior.seed() != ch.behavior.seed();
    assert!(tls_differs && http2_differs && canvas_differs && nav_differs
        && webgl_differs && audio_differs && behavior_differs,
        "7 of 8 components should differ between Firefox and Chrome profiles");
}

// ===========================================================================
// §3 Profile clone produces independent copies — mutations don't propagate
// ===========================================================================

#[test]
fn test_clone_mutation_isolation_tls() {
    let original = StealthProfile::firefox_default();
    let mut cloned = original.clone();
    cloned.tls = TlsFingerprint::chrome();
    assert_ne!(original.tls.compute_ja3(), cloned.tls.compute_ja3(),
        "Mutating cloned TLS should not affect original");
}

#[test]
fn test_clone_mutation_isolation_navigator() {
    let original = StealthProfile::firefox_default();
    let mut cloned = original.clone();
    cloned.navigator = NavigatorProfile::chrome();
    assert_ne!(original.navigator.user_agent, cloned.navigator.user_agent,
        "Mutating cloned navigator should not affect original");
}

#[test]
fn test_clone_mutation_isolation_canvas() {
    let original = StealthProfile::firefox_default();
    let mut cloned = original.clone();
    cloned.canvas = CanvasNoise::new(999);
    assert_ne!(original.canvas.seed(), cloned.canvas.seed(),
        "Mutating cloned canvas should not affect original");
}

#[test]
fn test_clone_mutation_isolation_webgl() {
    let original = StealthProfile::firefox_default();
    let mut cloned = original.clone();
    cloned.webgl = WebGLProfile::chrome();
    assert_ne!(original.webgl.vendor, cloned.webgl.vendor,
        "Mutating cloned WebGL should not affect original");
}

#[test]
fn test_clone_mutation_isolation_audio() {
    let original = StealthProfile::firefox_default();
    let mut cloned = original.clone();
    cloned.audio = AudioProfile::new(500);
    assert_ne!(original.audio.seed(), cloned.audio.seed(),
        "Mutating cloned audio should not affect original");
}

#[test]
fn test_clone_mutation_isolation_behavior() {
    let original = StealthProfile::firefox_default();
    let mut cloned = original.clone();
    cloned.behavior = BehaviorSimulator::new(500);
    assert_ne!(original.behavior.seed(), cloned.behavior.seed(),
        "Mutating cloned behavior should not affect original");
}

#[test]
fn test_clone_mutation_isolation_screen() {
    let original = StealthProfile::firefox_default();
    let mut cloned = original.clone();
    cloned.screen = ScreenProfile::new(3840, 2160, 2.0);
    assert_ne!(original.screen.width, cloned.screen.width,
        "Mutating cloned screen should not affect original");
}

#[test]
fn test_clone_mutation_isolation_http2() {
    let original = StealthProfile::firefox_default();
    let mut cloned = original.clone();
    cloned.http2 = Http2Fingerprint::chrome();
    assert_ne!(original.http2.akamai_fingerprint(), cloned.http2.akamai_fingerprint(),
        "Mutating cloned HTTP/2 should not affect original");
}

// ===========================================================================
// §4 Debug format for full profile and each component
// ===========================================================================

#[test]
fn test_debug_format_stealth_profile_firefox() {
    let profile = StealthProfile::firefox_default();
    let debug = format!("{:?}", profile);
    assert!(debug.contains("StealthProfile"),
        "Debug output should contain 'StealthProfile', got: {}", &debug[..debug.len().min(200)]);
    assert!(!debug.is_empty());
}

#[test]
fn test_debug_format_stealth_profile_chrome() {
    let profile = StealthProfile::chrome_default();
    let debug = format!("{:?}", profile);
    assert!(debug.contains("StealthProfile"),
        "Debug output should contain 'StealthProfile'");
}

#[test]
fn test_debug_format_tls_fingerprint() {
    let tls = TlsFingerprint::firefox();
    let debug = format!("{:?}", tls);
    assert!(debug.contains("TlsFingerprint"),
        "TLS debug should contain 'TlsFingerprint'");
}

#[test]
fn test_debug_format_http2_fingerprint() {
    let http2 = Http2Fingerprint::firefox();
    let debug = format!("{:?}", http2);
    assert!(debug.contains("Http2Fingerprint"),
        "HTTP/2 debug should contain 'Http2Fingerprint'");
}

#[test]
fn test_debug_format_canvas_noise() {
    let canvas = CanvasNoise::new(42);
    let debug = format!("{:?}", canvas);
    assert!(debug.contains("CanvasNoise"),
        "Canvas debug should contain 'CanvasNoise'");
}

#[test]
fn test_debug_format_navigator_profile() {
    let nav = NavigatorProfile::firefox();
    let debug = format!("{:?}", nav);
    assert!(debug.contains("NavigatorProfile"),
        "Navigator debug should contain 'NavigatorProfile'");
}

#[test]
fn test_debug_format_screen_profile() {
    let screen = ScreenProfile::default();
    let debug = format!("{:?}", screen);
    assert!(debug.contains("ScreenProfile"),
        "Screen debug should contain 'ScreenProfile'");
}

#[test]
fn test_debug_format_webgl_profile() {
    let webgl = WebGLProfile::firefox();
    let debug = format!("{:?}", webgl);
    assert!(debug.contains("WebGLProfile"),
        "WebGL debug should contain 'WebGLProfile'");
}

#[test]
fn test_debug_format_audio_profile() {
    let audio = AudioProfile::new(42);
    let debug = format!("{:?}", audio);
    assert!(debug.contains("AudioProfile"),
        "Audio debug should contain 'AudioProfile'");
}

#[test]
fn test_debug_format_behavior_simulator() {
    let behavior = BehaviorSimulator::new(42);
    let debug = format!("{:?}", behavior);
    assert!(debug.contains("BehaviorSimulator"),
        "Behavior debug should contain 'BehaviorSimulator'");
}

// ===========================================================================
// §5 Seed consistency — same seed produces identical CanvasNoise + AudioProfile + BehaviorSimulator
// ===========================================================================

#[test]
fn test_same_seed_canvas_noise_identical_output() {
    let c1 = CanvasNoise::new(42);
    let c2 = CanvasNoise::new(42);
    assert_eq!(c1.seed(), c2.seed());
    for x in 0..20u32 {
        for y in 0..20u32 {
            assert_eq!(
                c1.apply_to_pixel(128, 128, 128, 255, x, y),
                c2.apply_to_pixel(128, 128, 128, 255, x, y),
                "Same seed canvas noise should produce identical pixels at ({}, {})", x, y
            );
        }
    }
}

#[test]
fn test_same_seed_audio_profile_identical_output() {
    let a1 = AudioProfile::new(137);
    let a2 = AudioProfile::new(137);
    assert_eq!(a1.seed(), a2.seed());
    for i in 0..50u32 {
        assert_eq!(
            a1.apply_noise(0.5, i),
            a2.apply_noise(0.5, i),
            "Same seed audio should produce identical noise at index {}", i
        );
    }
}

#[test]
fn test_same_seed_behavior_simulator_identical_mouse_path() {
    let b1 = BehaviorSimulator::new(42);
    let b2 = BehaviorSimulator::new(42);
    let path1 = b1.generate_mouse_path(0.0, 0.0, 500.0, 300.0, 20);
    let path2 = b2.generate_mouse_path(0.0, 0.0, 500.0, 300.0, 20);
    assert_eq!(path1, path2, "Same seed behavior should produce identical mouse paths");
}

#[test]
fn test_same_seed_behavior_simulator_identical_typing_delays() {
    let b1 = BehaviorSimulator::new(42);
    let b2 = BehaviorSimulator::new(42);
    let delays1 = b1.generate_typing_delays(30);
    let delays2 = b2.generate_typing_delays(30);
    assert_eq!(delays1, delays2, "Same seed behavior should produce identical typing delays");
}

#[test]
fn test_same_seed_behavior_simulator_identical_scroll_deltas() {
    let b1 = BehaviorSimulator::new(42);
    let b2 = BehaviorSimulator::new(42);
    let deltas1 = b1.generate_scroll_deltas(1000.0, 20);
    let deltas2 = b2.generate_scroll_deltas(1000.0, 20);
    assert_eq!(deltas1, deltas2, "Same seed behavior should produce identical scroll deltas");
}

#[test]
fn test_firefox_profile_seed_consistency_canvas_audio_behavior() {
    // Firefox profile uses seed 42 for canvas, audio, and behavior
    let profile = StealthProfile::firefox_default();
    let standalone_canvas = CanvasNoise::new(42);
    let standalone_audio = AudioProfile::new(42);
    let standalone_behavior = BehaviorSimulator::new(42);
    assert_eq!(profile.canvas.seed(), standalone_canvas.seed());
    assert_eq!(profile.audio.seed(), standalone_audio.seed());
    assert_eq!(profile.behavior.seed(), standalone_behavior.seed());
}

#[test]
fn test_chrome_profile_seed_consistency_canvas_audio_behavior() {
    // Chrome profile uses seed 137 for canvas, audio, and behavior
    let profile = StealthProfile::chrome_default();
    let standalone_canvas = CanvasNoise::new(137);
    let standalone_audio = AudioProfile::new(137);
    let standalone_behavior = BehaviorSimulator::new(137);
    assert_eq!(profile.canvas.seed(), standalone_canvas.seed());
    assert_eq!(profile.audio.seed(), standalone_audio.seed());
    assert_eq!(profile.behavior.seed(), standalone_behavior.seed());
}

#[test]
fn test_different_seed_canvas_noise_produces_different_output() {
    let c1 = CanvasNoise::new(42);
    let c2 = CanvasNoise::new(137);
    // At least some pixels should differ
    let mut any_differ = false;
    for x in 0..10u32 {
        for y in 0..10u32 {
            if c1.apply_to_pixel(128, 128, 128, 255, x, y) != c2.apply_to_pixel(128, 128, 128, 255, x, y) {
                any_differ = true;
                break;
            }
        }
        if any_differ { break; }
    }
    assert!(any_differ, "Different seeds should produce different canvas noise");
}

#[test]
fn test_different_seed_audio_produces_different_output() {
    let a1 = AudioProfile::new(42);
    let a2 = AudioProfile::new(137);
    let mut any_differ = false;
    for i in 0..20u32 {
        if a1.apply_noise(0.5, i) != a2.apply_noise(0.5, i) {
            any_differ = true;
            break;
        }
    }
    assert!(any_differ, "Different seeds should produce different audio noise");
}

// ===========================================================================
// §6 Cross-component string format validation
// ===========================================================================

#[test]
fn test_tls_ja3_format_comma_separated_five_fields() {
    let ff = StealthProfile::firefox_default();
    let ja3 = ff.tls.compute_ja3();
    let fields: Vec<&str> = ja3.split(',').collect();
    // compute_ja3 format: "771,ciphers,extensions,curves,sigs" = 5 comma-separated fields
    assert_eq!(fields.len(), 5,
        "JA3 should have 5 comma-separated fields (version,ciphers,exts,curves,sigs), got {}: {:?}", fields.len(), fields);
    assert_eq!(fields[0], "771", "First JA3 field should be TLS version 771");
}

#[test]
fn test_tls_ja3_hash_format_firefox() {
    let ff = StealthProfile::firefox_default();
    let ja3_hash = ff.tls.ja3_hash;
    let fields: Vec<&str> = ja3_hash.split(',').collect();
    assert_eq!(fields.len(), 5,
        "JA3 hash should have 5 comma-separated fields (version,ciphers,exts,curves,sigs)");
}

#[test]
fn test_tls_ja3_hash_format_chrome() {
    let ch = StealthProfile::chrome_default();
    let ja3_hash = ch.tls.ja3_hash;
    let fields: Vec<&str> = ja3_hash.split(',').collect();
    assert_eq!(fields.len(), 5,
        "JA3 hash should have 5 comma-separated fields");
}

#[test]
fn test_tls_ja4_format_prefix() {
    let ff = StealthProfile::firefox_default();
    let ja4 = ff.tls.compute_ja4();
    assert!(ja4.starts_with("t13d"),
        "JA4 fingerprint should start with 't13d', got: {}", ja4);
    assert!(ja4.contains('_'),
        "JA4 fingerprint should contain underscore separator");
}

#[test]
fn test_http2_akamai_fingerprint_format_six_colon_fields() {
    let ff = StealthProfile::firefox_default();
    let akamai = ff.http2.akamai_fingerprint();
    let fields: Vec<&str> = akamai.split(':').collect();
    assert_eq!(fields.len(), 6,
        "Akamai fingerprint should have 6 colon-separated fields");
    for (i, field) in fields.iter().enumerate() {
        assert!(field.parse::<u32>().is_ok(),
            "Akamai field {} should be numeric, got: {}", i, field);
    }
}

#[test]
fn test_http2_akamai_fingerprint_chrome_format() {
    let ch = StealthProfile::chrome_default();
    let akamai = ch.http2.akamai_fingerprint();
    let fields: Vec<&str> = akamai.split(':').collect();
    assert_eq!(fields.len(), 6,
        "Chrome Akamai fingerprint should have 6 colon-separated fields");
}

#[test]
fn test_navigator_user_agent_format_firefox() {
    let ff = StealthProfile::firefox_default();
    let ua = &ff.navigator.user_agent;
    assert!(ua.starts_with("Mozilla/5.0"),
        "User agent should start with 'Mozilla/5.0', got: {}", ua);
    assert!(ua.contains("Firefox"),
        "Firefox user agent should contain 'Firefox'");
    assert!(ua.contains("Gecko"),
        "Firefox user agent should contain 'Gecko'");
}

#[test]
fn test_navigator_user_agent_format_chrome() {
    let ch = StealthProfile::chrome_default();
    let ua = &ch.navigator.user_agent;
    assert!(ua.starts_with("Mozilla/5.0"),
        "User agent should start with 'Mozilla/5.0', got: {}", ua);
    assert!(ua.contains("Chrome"),
        "Chrome user agent should contain 'Chrome'");
    assert!(ua.contains("AppleWebKit"),
        "Chrome user agent should contain 'AppleWebKit'");
    assert!(ua.contains("Safari"),
        "Chrome user agent should contain 'Safari'");
}

#[test]
fn test_navigator_platform_format() {
    let ff = StealthProfile::firefox_default();
    let ch = StealthProfile::chrome_default();
    assert!(ff.navigator.platform.contains("Linux"),
        "Firefox platform should contain 'Linux'");
    assert!(ch.navigator.platform.contains("Linux"),
        "Chrome platform should contain 'Linux'");
}

#[test]
fn test_webgl_vendor_renderer_format_firefox() {
    let ff = StealthProfile::firefox_default();
    assert_eq!(ff.webgl.vendor, "Mozilla",
        "Firefox WebGL vendor should be 'Mozilla'");
    assert!(ff.webgl.renderer.contains("WebGL") || ff.webgl.renderer.contains("OpenGL"),
        "Firefox WebGL renderer should reference WebGL/OpenGL");
}

#[test]
fn test_webgl_vendor_renderer_format_chrome() {
    let ch = StealthProfile::chrome_default();
    assert!(ch.webgl.vendor.contains("Google"),
        "Chrome WebGL vendor should contain 'Google'");
    assert!(ch.webgl.renderer.contains("ANGLE"),
        "Chrome WebGL renderer should contain 'ANGLE'");
}

// ===========================================================================
// §7 Default values sanity check — all fields non-empty where expected
// ===========================================================================

#[test]
fn test_firefox_all_string_fields_non_empty() {
    let ff = StealthProfile::firefox_default();
    // TLS
    assert!(!ff.tls.ja3_hash.is_empty(), "Firefox JA3 hash should not be empty");
    assert!(!ff.tls.tls_version.is_empty(), "Firefox TLS version should not be empty");
    assert!(!ff.tls.cipher_suites.is_empty(), "Firefox cipher suites should not be empty");
    assert!(!ff.tls.extensions.is_empty(), "Firefox extensions should not be empty");
    assert!(!ff.tls.alpn_protocols.is_empty(), "Firefox ALPN should not be empty");
    // Navigator
    assert!(!ff.navigator.user_agent.is_empty(), "Firefox user agent should not be empty");
    assert!(!ff.navigator.platform.is_empty(), "Firefox platform should not be empty");
    assert!(!ff.navigator.language.is_empty(), "Firefox language should not be empty");
    assert!(!ff.navigator.app_version.is_empty(), "Firefox app version should not be empty");
    assert!(!ff.navigator.product_sub.is_empty(), "Firefox product sub should not be empty");
    // WebGL
    assert!(!ff.webgl.vendor.is_empty(), "Firefox WebGL vendor should not be empty");
    assert!(!ff.webgl.renderer.is_empty(), "Firefox WebGL renderer should not be empty");
    assert!(!ff.webgl.extensions.is_empty(), "Firefox WebGL extensions should not be empty");
}

#[test]
fn test_chrome_all_string_fields_non_empty() {
    let ch = StealthProfile::chrome_default();
    assert!(!ch.tls.ja3_hash.is_empty());
    assert!(!ch.tls.tls_version.is_empty());
    assert!(!ch.tls.cipher_suites.is_empty());
    assert!(!ch.tls.extensions.is_empty());
    assert!(!ch.tls.alpn_protocols.is_empty());
    assert!(!ch.navigator.user_agent.is_empty());
    assert!(!ch.navigator.platform.is_empty());
    assert!(!ch.navigator.language.is_empty());
    assert!(!ch.navigator.app_version.is_empty());
    assert!(!ch.navigator.product_sub.is_empty());
    assert!(!ch.webgl.vendor.is_empty());
    assert!(!ch.webgl.renderer.is_empty());
    assert!(!ch.webgl.extensions.is_empty());
}

#[test]
fn test_firefox_all_numeric_fields_positive() {
    let ff = StealthProfile::firefox_default();
    // HTTP/2
    assert!(ff.http2.header_table_size > 0);
    assert!(ff.http2.max_concurrent_streams > 0);
    assert!(ff.http2.initial_window_size > 0);
    assert!(ff.http2.max_frame_size > 0);
    assert!(ff.http2.max_header_list_size > 0);
    assert!(ff.http2.window_update_size > 0);
    // Screen
    assert!(ff.screen.width > 0);
    assert!(ff.screen.height > 0);
    assert!(ff.screen.avail_width > 0);
    assert!(ff.screen.avail_height > 0);
    assert!(ff.screen.color_depth > 0);
    assert!(ff.screen.pixel_depth > 0);
    assert!(ff.screen.device_pixel_ratio > 0.0);
    // Canvas/Audio/Behavior seeds
    assert!(ff.canvas.seed() > 0);
    assert!(ff.audio.seed() > 0);
    assert!(ff.behavior.seed() > 0);
    // Navigator
    assert!(ff.navigator.hardware_concurrency > 0);
    // WebGL
    assert!(ff.webgl.max_texture_size > 0);
    assert!(ff.webgl.max_renderbuffer_size > 0);
}

#[test]
fn test_chrome_all_numeric_fields_positive() {
    let ch = StealthProfile::chrome_default();
    assert!(ch.http2.header_table_size > 0);
    assert!(ch.http2.max_concurrent_streams > 0);
    assert!(ch.http2.initial_window_size > 0);
    assert!(ch.http2.max_frame_size > 0);
    assert!(ch.http2.max_header_list_size > 0);
    assert!(ch.http2.window_update_size > 0);
    assert!(ch.screen.width > 0);
    assert!(ch.screen.height > 0);
    assert!(ch.screen.avail_width > 0);
    assert!(ch.screen.avail_height > 0);
    assert!(ch.screen.color_depth > 0);
    assert!(ch.screen.pixel_depth > 0);
    assert!(ch.screen.device_pixel_ratio > 0.0);
    assert!(ch.canvas.seed() > 0);
    assert!(ch.audio.seed() > 0);
    assert!(ch.behavior.seed() > 0);
    assert!(ch.navigator.hardware_concurrency > 0);
    assert!(ch.webgl.max_texture_size > 0);
    assert!(ch.webgl.max_renderbuffer_size > 0);
}

#[test]
fn test_firefox_alpn_contains_h2_and_http11() {
    let ff = StealthProfile::firefox_default();
    let alpn = ff.tls.alpn_strings();
    assert!(alpn.iter().any(|s| *s == "h2"), "Firefox ALPN should include h2");
    assert!(alpn.iter().any(|s| *s == "http/1.1"), "Firefox ALPN should include http/1.1");
}

#[test]
fn test_chrome_alpn_contains_h2_and_http11() {
    let ch = StealthProfile::chrome_default();
    let alpn = ch.tls.alpn_strings();
    assert!(alpn.iter().any(|s| *s == "h2"), "Chrome ALPN should include h2");
    assert!(alpn.iter().any(|s| *s == "http/1.1"), "Chrome ALPN should include http/1.1");
}

#[test]
fn test_http2_pseudo_header_order_complete() {
    let ff = StealthProfile::firefox_default();
    let ch = StealthProfile::chrome_default();
    let required_pseudo = [":method", ":path", ":authority", ":scheme"];
    for pseudo in &required_pseudo {
        assert!(ff.http2.pseudo_header_order.contains(pseudo),
            "Firefox HTTP/2 should include pseudo header {}", pseudo);
        assert!(ch.http2.pseudo_header_order.contains(pseudo),
            "Chrome HTTP/2 should include pseudo header {}", pseudo);
    }
    assert_eq!(ff.http2.pseudo_header_order.len(), 4,
        "Firefox should have exactly 4 pseudo headers");
    assert_eq!(ch.http2.pseudo_header_order.len(), 4,
        "Chrome should have exactly 4 pseudo headers");
}

#[test]
fn test_screen_color_depth_standard() {
    let ff = StealthProfile::firefox_default();
    let ch = StealthProfile::chrome_default();
    assert_eq!(ff.screen.color_depth, 24, "Color depth should be 24-bit");
    assert_eq!(ch.screen.color_depth, 24, "Color depth should be 24-bit");
    assert_eq!(ff.screen.pixel_depth, 24, "Pixel depth should be 24-bit");
    assert_eq!(ch.screen.pixel_depth, 24, "Pixel depth should be 24-bit");
}

// ===========================================================================
// §8 Profile roundtrip — construct -> clone -> verify equality
// ===========================================================================

#[test]
fn test_firefox_roundtrip_clone_equality() {
    let original = StealthProfile::firefox_default();
    let cloned = original.clone();
    // TLS
    assert_eq!(original.tls.compute_ja3(), cloned.tls.compute_ja3());
    assert_eq!(original.tls.compute_ja4(), cloned.tls.compute_ja4());
    assert_eq!(original.tls.ja3_hash, cloned.tls.ja3_hash);
    assert_eq!(original.tls.cipher_suites, cloned.tls.cipher_suites);
    assert_eq!(original.tls.extensions, cloned.tls.extensions);
    assert_eq!(original.tls.alpn_protocols, cloned.tls.alpn_protocols);
    // HTTP/2
    assert_eq!(original.http2.akamai_fingerprint(), cloned.http2.akamai_fingerprint());
    assert_eq!(original.http2.header_table_size, cloned.http2.header_table_size);
    assert_eq!(original.http2.pseudo_header_order, cloned.http2.pseudo_header_order);
    // Canvas
    assert_eq!(original.canvas.seed(), cloned.canvas.seed());
    // Navigator
    assert_eq!(original.navigator.user_agent, cloned.navigator.user_agent);
    assert_eq!(original.navigator.platform, cloned.navigator.platform);
    assert_eq!(original.navigator.vendor, cloned.navigator.vendor);
    assert_eq!(original.navigator.language, cloned.navigator.language);
    assert_eq!(original.navigator.hardware_concurrency, cloned.navigator.hardware_concurrency);
    assert_eq!(original.navigator.max_touch_points, cloned.navigator.max_touch_points);
    // Screen
    assert_eq!(original.screen.width, cloned.screen.width);
    assert_eq!(original.screen.height, cloned.screen.height);
    assert_eq!(original.screen.device_pixel_ratio, cloned.screen.device_pixel_ratio);
    // WebGL
    assert_eq!(original.webgl.vendor, cloned.webgl.vendor);
    assert_eq!(original.webgl.renderer, cloned.webgl.renderer);
    assert_eq!(original.webgl.extensions, cloned.webgl.extensions);
    // Audio
    assert_eq!(original.audio.seed(), cloned.audio.seed());
    // Behavior
    assert_eq!(original.behavior.seed(), cloned.behavior.seed());
}

#[test]
fn test_chrome_roundtrip_clone_equality() {
    let original = StealthProfile::chrome_default();
    let cloned = original.clone();
    assert_eq!(original.tls.compute_ja3(), cloned.tls.compute_ja3());
    assert_eq!(original.tls.compute_ja4(), cloned.tls.compute_ja4());
    assert_eq!(original.tls.ja3_hash, cloned.tls.ja3_hash);
    assert_eq!(original.tls.cipher_suites, cloned.tls.cipher_suites);
    assert_eq!(original.http2.akamai_fingerprint(), cloned.http2.akamai_fingerprint());
    assert_eq!(original.http2.pseudo_header_order, cloned.http2.pseudo_header_order);
    assert_eq!(original.canvas.seed(), cloned.canvas.seed());
    assert_eq!(original.navigator.user_agent, cloned.navigator.user_agent);
    assert_eq!(original.navigator.vendor, cloned.navigator.vendor);
    assert_eq!(original.screen.width, cloned.screen.width);
    assert_eq!(original.screen.height, cloned.screen.height);
    assert_eq!(original.webgl.vendor, cloned.webgl.vendor);
    assert_eq!(original.webgl.renderer, cloned.webgl.renderer);
    assert_eq!(original.audio.seed(), cloned.audio.seed());
    assert_eq!(original.behavior.seed(), cloned.behavior.seed());
}

#[test]
fn test_firefox_roundtrip_clone_double() {
    let original = StealthProfile::firefox_default();
    let cloned = original.clone();
    let double_cloned = cloned.clone();
    assert_eq!(original.tls.compute_ja3(), double_cloned.tls.compute_ja3());
    assert_eq!(original.navigator.user_agent, double_cloned.navigator.user_agent);
    assert_eq!(original.canvas.seed(), double_cloned.canvas.seed());
    assert_eq!(original.webgl.vendor, double_cloned.webgl.vendor);
    assert_eq!(original.audio.seed(), double_cloned.audio.seed());
    assert_eq!(original.behavior.seed(), double_cloned.behavior.seed());
}

#[test]
fn test_chrome_roundtrip_clone_double() {
    let original = StealthProfile::chrome_default();
    let cloned = original.clone();
    let double_cloned = cloned.clone();
    assert_eq!(original.tls.compute_ja3(), double_cloned.tls.compute_ja3());
    assert_eq!(original.navigator.user_agent, double_cloned.navigator.user_agent);
    assert_eq!(original.canvas.seed(), double_cloned.canvas.seed());
    assert_eq!(original.webgl.vendor, double_cloned.webgl.vendor);
    assert_eq!(original.audio.seed(), double_cloned.audio.seed());
    assert_eq!(original.behavior.seed(), double_cloned.behavior.seed());
}

#[test]
fn test_roundtrip_via_engine_firefox() {
    let profile = StealthProfile::firefox_default();
    let engine = StealthEngine::new(profile.clone());
    let from_engine = engine.profile().clone();
    assert_eq!(profile.tls.compute_ja3(), from_engine.tls.compute_ja3());
    assert_eq!(profile.navigator.user_agent, from_engine.navigator.user_agent);
    assert_eq!(profile.canvas.seed(), from_engine.canvas.seed());
    assert_eq!(profile.webgl.vendor, from_engine.webgl.vendor);
    assert_eq!(profile.audio.seed(), from_engine.audio.seed());
    assert_eq!(profile.behavior.seed(), from_engine.behavior.seed());
}

#[test]
fn test_roundtrip_via_engine_chrome() {
    let profile = StealthProfile::chrome_default();
    let engine = StealthEngine::new(profile.clone());
    let from_engine = engine.profile().clone();
    assert_eq!(profile.tls.compute_ja3(), from_engine.tls.compute_ja3());
    assert_eq!(profile.navigator.user_agent, from_engine.navigator.user_agent);
    assert_eq!(profile.canvas.seed(), from_engine.canvas.seed());
    assert_eq!(profile.webgl.vendor, from_engine.webgl.vendor);
    assert_eq!(profile.audio.seed(), from_engine.audio.seed());
    assert_eq!(profile.behavior.seed(), from_engine.behavior.seed());
}

// ===========================================================================
// §9 Cross-component behavioral consistency
// ===========================================================================

#[test]
fn test_firefox_canvas_and_audio_share_same_seed() {
    let ff = StealthProfile::firefox_default();
    assert_eq!(ff.canvas.seed(), ff.audio.seed(),
        "Firefox profile should use the same seed for canvas and audio");
}

#[test]
fn test_firefox_canvas_and_behavior_share_same_seed() {
    let ff = StealthProfile::firefox_default();
    assert_eq!(ff.canvas.seed(), ff.behavior.seed(),
        "Firefox profile should use the same seed for canvas and behavior");
}

#[test]
fn test_chrome_canvas_and_audio_share_same_seed() {
    let ch = StealthProfile::chrome_default();
    assert_eq!(ch.canvas.seed(), ch.audio.seed(),
        "Chrome profile should use the same seed for canvas and audio");
}

#[test]
fn test_chrome_canvas_and_behavior_share_same_seed() {
    let ch = StealthProfile::chrome_default();
    assert_eq!(ch.canvas.seed(), ch.behavior.seed(),
        "Chrome profile should use the same seed for canvas and behavior");
}

#[test]
fn test_firefox_navigator_oscpu_present_chrome_absent() {
    let ff = StealthProfile::firefox_default();
    let ch = StealthProfile::chrome_default();
    assert!(ff.navigator.oscpu.is_some(),
        "Firefox navigator should have oscpu");
    assert!(ch.navigator.oscpu.is_none(),
        "Chrome navigator should not have oscpu");
}

#[test]
fn test_firefox_navigator_build_id_present_chrome_absent() {
    let ff = StealthProfile::firefox_default();
    let ch = StealthProfile::chrome_default();
    assert!(ff.navigator.build_id.is_some(),
        "Firefox navigator should have build_id");
    assert!(ch.navigator.build_id.is_none(),
        "Chrome navigator should not have build_id");
}

#[test]
fn test_firefox_vendor_empty_chrome_vendor_google() {
    let ff = StealthProfile::firefox_default();
    let ch = StealthProfile::chrome_default();
    assert_eq!(ff.navigator.vendor, "",
        "Firefox navigator.vendor should be empty string");
    assert_eq!(ch.navigator.vendor, "Google Inc.",
        "Chrome navigator.vendor should be 'Google Inc.'");
}

#[test]
fn test_firefox_product_sub_differs_from_chrome() {
    let ff = StealthProfile::firefox_default();
    let ch = StealthProfile::chrome_default();
    assert_ne!(ff.navigator.product_sub, ch.navigator.product_sub,
        "Firefox and Chrome should have different product_sub values");
}

#[test]
fn test_webgl_extensions_firefox_superset_of_chrome() {
    let ff = StealthProfile::firefox_default();
    let ch = StealthProfile::chrome_default();
    // Firefox has more WebGL extensions than Chrome
    assert!(ff.webgl.extensions.len() > ch.webgl.extensions.len(),
        "Firefox WebGL should have more extensions than Chrome");
    // Both should have WEBGL_debug_renderer_info
    assert!(ff.webgl.extensions.iter().any(|e| e == "WEBGL_debug_renderer_info"));
    assert!(ch.webgl.extensions.iter().any(|e| e == "WEBGL_debug_renderer_info"));
}

#[test]
fn test_tls_firefox_more_cipher_suites_than_chrome() {
    let ff = StealthProfile::firefox_default();
    let ch = StealthProfile::chrome_default();
    assert!(ff.tls.cipher_suites.len() > ch.tls.cipher_suites.len(),
        "Firefox should have more TLS cipher suites than Chrome");
}

#[test]
fn test_tls_firefox_more_supported_groups_than_chrome() {
    let ff = StealthProfile::firefox_default();
    let ch = StealthProfile::chrome_default();
    assert!(ff.tls.supported_groups.len() > ch.tls.supported_groups.len(),
        "Firefox should have more supported groups than Chrome");
}

#[test]
fn test_http2_firefox_smaller_window_than_chrome() {
    let ff = StealthProfile::firefox_default();
    let ch = StealthProfile::chrome_default();
    assert!(ff.http2.initial_window_size < ch.http2.initial_window_size,
        "Firefox HTTP/2 initial window size should be smaller than Chrome");
    assert!(ff.http2.window_update_size < ch.http2.window_update_size,
        "Firefox HTTP/2 window update size should be smaller than Chrome");
}
