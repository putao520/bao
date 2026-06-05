// @trace TEST-STL-015 [req:REQ-STL-003] [level:unit]
// @trace TEST-STL-016 [req:REQ-STL-004] [level:unit]
// Canvas noise pixel manipulation, NavigatorProfile presets, ScreenProfile
// construction, deterministic noise stability, edge cases, clone/debug.

use bao_stealth::{CanvasNoise, NavigatorProfile, ScreenProfile, StealthEngine, StealthProfile};

// ---- CanvasNoise construction ----

#[test]
fn test_canvas_noise_new_valid_seed() {
    let cn = CanvasNoise::new(42);
    assert_eq!(cn.seed(), 42);
}

#[test]
#[should_panic(expected = "canvas_seed must be > 0")]
fn test_canvas_noise_new_zero_seed_panics() {
    let _ = CanvasNoise::new(0);
}

#[test]
fn test_canvas_noise_default_amplitude() {
    let cn = CanvasNoise::new(1);
    assert!((cn.noise_amplitude() - 0.001).abs() < f64::EPSILON);
}

#[test]
fn test_canvas_noise_clone() {
    let cn = CanvasNoise::new(123);
    let cloned = cn.clone();
    assert_eq!(cloned.seed(), 123);
}

#[test]
fn test_canvas_noise_debug() {
    let cn = CanvasNoise::new(42);
    let debug = format!("{:?}", cn);
    assert!(debug.contains("CanvasNoise"));
}

// ---- apply_to_pixel deterministic ----

#[test]
fn test_apply_to_pixel_deterministic() {
    let cn = CanvasNoise::new(42);
    let p1 = cn.apply_to_pixel(128, 128, 128, 255, 100, 200);
    let p2 = cn.apply_to_pixel(128, 128, 128, 255, 100, 200);
    assert_eq!(p1, p2);
}

#[test]
fn test_apply_to_pixel_different_coords_differ() {
    let cn = CanvasNoise::new(42);
    // With amplitude 0.001, noise is <1 for 8-bit values so we check that
    // deterministic_noise differs by sampling many positions and checking
    // the raw (r,g,b) triplets aren't all identical.
    let mut results = std::collections::HashSet::new();
    for x in 0..100u32 {
        for y in 0..100u32 {
            let p = cn.apply_to_pixel(128, 128, 128, 255, x, y);
            results.insert(p);
            if results.len() > 1 { return; }
        }
    }
    panic!("100x100 pixels all identical — noise not applied");
}

#[test]
fn test_apply_to_pixel_preserves_alpha() {
    let cn = CanvasNoise::new(42);
    let (_, _, _, a) = cn.apply_to_pixel(128, 128, 128, 200, 50, 50);
    assert_eq!(a, 200);
}

#[test]
fn test_apply_to_pixel_preserves_alpha_zero() {
    let cn = CanvasNoise::new(99);
    let (_, _, _, a) = cn.apply_to_pixel(0, 0, 0, 0, 10, 10);
    assert_eq!(a, 0);
}

#[test]
fn test_apply_to_pixel_preserves_alpha_max() {
    let cn = CanvasNoise::new(99);
    let (_, _, _, a) = cn.apply_to_pixel(255, 255, 255, 255, 10, 10);
    assert_eq!(a, 255);
}

#[test]
fn test_apply_to_pixel_clamps_low() {
    let cn = CanvasNoise::new(42);
    // Zero values should not underflow
    let (r, g, b, _) = cn.apply_to_pixel(0, 0, 0, 255, 0, 0);
    assert!(r <= 5, "Red should be near zero, got {}", r);
    assert!(g <= 5, "Green should be near zero, got {}", g);
    assert!(b <= 5, "Blue should be near zero, got {}", b);
}

#[test]
fn test_apply_to_pixel_clamps_high() {
    let cn = CanvasNoise::new(42);
    let (r, g, b, _) = cn.apply_to_pixel(255, 255, 255, 255, 0, 0);
    assert!(r >= 250, "Red should be near max, got {}", r);
}

#[test]
fn test_apply_to_pixel_same_seed_same_result() {
    let cn1 = CanvasNoise::new(777);
    let cn2 = CanvasNoise::new(777);
    let p1 = cn1.apply_to_pixel(100, 150, 200, 255, 500, 300);
    let p2 = cn2.apply_to_pixel(100, 150, 200, 255, 500, 300);
    assert_eq!(p1, p2);
}

#[test]
fn test_apply_to_pixel_different_seeds_differ() {
    // With amplitude 0.001, noise may round to same u8 for mid-values.
    // Instead verify by checking many pixels — at least some must differ.
    let cn1 = CanvasNoise::new(1);
    let cn2 = CanvasNoise::new(2);
    let mut differ = false;
    for x in 0..200u32 {
        for y in 0..200u32 {
            if cn1.apply_to_pixel(128, 128, 128, 255, x, y) != cn2.apply_to_pixel(128, 128, 128, 255, x, y) {
                differ = true;
                break;
            }
        }
        if differ { break; }
    }
    assert!(differ, "Different seeds should produce at least one different pixel");
}

#[test]
fn test_apply_to_pixel_large_coords() {
    let cn = CanvasNoise::new(42);
    let (r, g, b, a) = cn.apply_to_pixel(128, 128, 128, 255, u32::MAX, u32::MAX);
    // Should not panic, values should be valid
    assert!(r <= 255);
    assert!(g <= 255);
    assert!(b <= 255);
    assert_eq!(a, 255);
}

#[test]
fn test_apply_to_pixel_zero_coords() {
    let cn = CanvasNoise::new(42);
    let p = cn.apply_to_pixel(128, 128, 128, 255, 0, 0);
    assert!(p.0 <= 255 && p.1 <= 255 && p.2 <= 255);
}

#[test]
fn test_noise_amplitude_small() {
    let cn = CanvasNoise::new(42);
    // With amplitude 0.001, changes should be small (< 2 for most pixels)
    let mut max_diff = 0i32;
    for x in 0..50u32 {
        for y in 0..50u32 {
            let (r, _, _, _) = cn.apply_to_pixel(128, 128, 128, 255, x, y);
            let diff = (r as i32 - 128).abs();
            max_diff = max_diff.max(diff);
        }
    }
    // Max noise contribution: 0.5 * 0.001 * 255 ≈ 0.13, so < 1 is typical
    assert!(max_diff <= 1, "With amplitude 0.001, max diff should be tiny, got {}", max_diff);
}

// ---- NavigatorProfile ----

#[test]
fn test_navigator_firefox_preset() {
    let nav = NavigatorProfile::firefox();
    assert!(nav.user_agent.contains("Firefox"));
    assert!(nav.user_agent.contains("rv:128"));
    assert_eq!(nav.platform, "Linux x86_64");
    assert_eq!(nav.language, "en-US");
    assert!(nav.hardware_concurrency > 0);
    assert_eq!(nav.max_touch_points, 0);
    assert_eq!(nav.vendor, "");
    assert!(nav.oscpu.is_some());
    assert!(nav.build_id.is_some());
    assert!(nav.product_sub.contains("20100101"));
}

#[test]
fn test_navigator_chrome_preset() {
    let nav = NavigatorProfile::chrome();
    assert!(nav.user_agent.contains("Chrome/128"));
    assert!(nav.user_agent.contains("AppleWebKit"));
    assert_eq!(nav.platform, "Linux x86_64");
    assert_eq!(nav.language, "en-US");
    assert_eq!(nav.vendor, "Google Inc.");
    assert!(nav.oscpu.is_none());
    assert!(nav.build_id.is_none());
    assert!(nav.product_sub.contains("20030107"));
}

#[test]
fn test_navigator_firefox_vs_chrome_user_agent_differ() {
    let ff = NavigatorProfile::firefox();
    let ch = NavigatorProfile::chrome();
    assert_ne!(ff.user_agent, ch.user_agent);
}

#[test]
fn test_navigator_firefox_vs_chrome_vendor_differ() {
    let ff = NavigatorProfile::firefox();
    let ch = NavigatorProfile::chrome();
    assert_ne!(ff.vendor, ch.vendor);
}

#[test]
fn test_navigator_firefox_has_oscpu_chrome_doesnt() {
    let ff = NavigatorProfile::firefox();
    let ch = NavigatorProfile::chrome();
    assert!(ff.oscpu.is_some());
    assert!(ch.oscpu.is_none());
}

#[test]
fn test_navigator_firefox_has_build_id_chrome_doesnt() {
    let ff = NavigatorProfile::firefox();
    let ch = NavigatorProfile::chrome();
    assert!(ff.build_id.is_some());
    assert!(ch.build_id.is_none());
}

#[test]
fn test_navigator_clone() {
    let nav = NavigatorProfile::firefox();
    let cloned = nav.clone();
    assert_eq!(cloned.user_agent, nav.user_agent);
    assert_eq!(cloned.platform, nav.platform);
    assert_eq!(cloned.oscpu, nav.oscpu);
}

#[test]
fn test_navigator_debug() {
    let nav = NavigatorProfile::chrome();
    let debug = format!("{:?}", nav);
    assert!(debug.contains("Chrome/128"));
    assert!(debug.contains("Google Inc."));
}

#[test]
fn test_navigator_app_version_nonempty() {
    assert!(!NavigatorProfile::firefox().app_version.is_empty());
    assert!(!NavigatorProfile::chrome().app_version.is_empty());
}

#[test]
fn test_navigator_chrome_app_version_matches_ua() {
    let nav = NavigatorProfile::chrome();
    // Chrome appVersion typically starts with "5.0"
    assert!(nav.app_version.starts_with("5.0"));
}

// ---- ScreenProfile ----

#[test]
fn test_screen_default() {
    let scr = ScreenProfile::default();
    assert_eq!(scr.width, 1920);
    assert_eq!(scr.height, 1080);
    assert_eq!(scr.avail_width, 1920);
    assert_eq!(scr.avail_height, 1040);
    assert_eq!(scr.color_depth, 24);
    assert_eq!(scr.pixel_depth, 24);
    assert!((scr.device_pixel_ratio - 1.0).abs() < f64::EPSILON);
}

#[test]
fn test_screen_new_custom() {
    let scr = ScreenProfile::new(2560, 1440, 2.0);
    assert_eq!(scr.width, 2560);
    assert_eq!(scr.height, 1440);
    assert_eq!(scr.avail_width, 2560);
    assert_eq!(scr.avail_height, 1400); // height - 40
    assert!((scr.device_pixel_ratio - 2.0).abs() < f64::EPSILON);
}

#[test]
fn test_screen_new_avail_height_offset() {
    let scr = ScreenProfile::new(800, 600, 1.0);
    assert_eq!(scr.avail_height, 560); // 600 - 40
}

#[test]
fn test_screen_new_small_height() {
    let scr = ScreenProfile::new(320, 50, 1.0);
    // avail_height = 50 - 40 = 10, no underflow since it's u32
    assert_eq!(scr.avail_height, 10);
}

#[test]
fn test_screen_color_and_pixel_depth_always_24() {
    let scr = ScreenProfile::new(3840, 2160, 1.5);
    assert_eq!(scr.color_depth, 24);
    assert_eq!(scr.pixel_depth, 24);
}

#[test]
fn test_screen_clone() {
    let scr = ScreenProfile::new(1920, 1080, 1.25);
    let cloned = scr.clone();
    assert_eq!(cloned.width, scr.width);
    assert!((cloned.device_pixel_ratio - scr.device_pixel_ratio).abs() < f64::EPSILON);
}

#[test]
fn test_screen_debug() {
    let scr = ScreenProfile::default();
    let debug = format!("{:?}", scr);
    assert!(debug.contains("1920"));
    assert!(debug.contains("1080"));
}

// ---- StealthEngine integration with canvas/navigator/screen ----

#[test]
fn test_default_engine_has_canvas_noise() {
    let engine = StealthEngine::default_engine();
    assert_eq!(engine.canvas_noise().seed(), 42);
}

#[test]
fn test_default_engine_has_navigator() {
    let engine = StealthEngine::default_engine();
    assert!(!engine.navigator().user_agent.is_empty());
}

#[test]
fn test_default_engine_has_screen() {
    let engine = StealthEngine::default_engine();
    assert_eq!(engine.screen().width, 1920);
    assert_eq!(engine.screen().height, 1080);
}

#[test]
fn test_engine_profile_access() {
    let profile = StealthProfile::firefox_default();
    let engine = StealthEngine::new(profile);
    assert!(engine.tls_config().compute_ja3().len() > 10);
    assert!(engine.navigator().user_agent.contains("Firefox") || engine.navigator().user_agent.contains("Chrome"));
}

// ---- Cross-profile consistency ----

#[test]
fn test_firefox_default_profile_navigator_matches_firefox_preset() {
    let profile = StealthProfile::firefox_default();
    assert!(profile.navigator.user_agent.contains("Firefox"));
    assert!(profile.navigator.oscpu.is_some());
}

#[test]
fn test_canvas_noise_via_profile() {
    let profile = StealthProfile::firefox_default();
    let p = profile.canvas.apply_to_pixel(100, 100, 100, 255, 10, 10);
    assert!(p.0 <= 255 && p.1 <= 255 && p.2 <= 255);
    assert_eq!(p.3, 255);
}

#[test]
fn test_screen_via_profile_default() {
    let profile = StealthProfile::firefox_default();
    assert_eq!(profile.screen.width, 1920);
    assert_eq!(profile.screen.height, 1080);
}
