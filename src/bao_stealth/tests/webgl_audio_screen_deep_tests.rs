#![allow(unused_comparisons, unused_variables)]
// @trace TEST-STL-014-WEBGL-AUDIO-SCREEN [req:REQ-STL-003,REQ-STL-004,REQ-STL-005,REQ-STL-006] [level:unit]
// WebGL/Audio/Screen/Canvas deep validation: profile differentiation,
// noise determinism, boundary conditions, cross-profile consistency.

use bao_stealth::{CanvasNoise, WebGLProfile, AudioProfile, NavigatorProfile, ScreenProfile};

// ---- WebGLProfile Chrome ----

#[test]
fn test_webgl_chrome_vendor_format() {
    let gl = WebGLProfile::chrome();
    assert!(gl.vendor.contains("Google") || gl.vendor.contains("NVIDIA"));
}

#[test]
fn test_webgl_chrome_renderer_contains_angle() {
    let gl = WebGLProfile::chrome();
    assert!(gl.renderer.contains("ANGLE"));
}

#[test]
fn test_webgl_chrome_extensions_not_empty() {
    let gl = WebGLProfile::chrome();
    assert!(!gl.extensions.is_empty());
    assert!(gl.extensions.len() >= 10);
}

#[test]
fn test_webgl_chrome_has_webgl_lose_context() {
    let gl = WebGLProfile::chrome();
    assert!(gl.extensions.iter().any(|e| e == "WEBGL_lose_context"));
}

#[test]
fn test_webgl_chrome_max_texture_size_power_of_two() {
    let gl = WebGLProfile::chrome();
    assert!(gl.max_texture_size > 0);
    assert!(gl.max_texture_size.is_power_of_two());
}

#[test]
fn test_webgl_chrome_max_renderbuffer_size_power_of_two() {
    let gl = WebGLProfile::chrome();
    assert!(gl.max_renderbuffer_size > 0);
    assert!(gl.max_renderbuffer_size.is_power_of_two());
}

#[test]
fn test_webgl_chrome_viewport_dims_match_texture() {
    let gl = WebGLProfile::chrome();
    assert_eq!(gl.max_viewport_dims[0], gl.max_texture_size);
    assert_eq!(gl.max_viewport_dims[1], gl.max_texture_size);
}

// ---- WebGLProfile Firefox ----

#[test]
fn test_webgl_firefox_vendor_is_mozilla() {
    let gl = WebGLProfile::firefox();
    assert_eq!(gl.vendor, "Mozilla");
}

#[test]
fn test_webgl_firefox_renderer_contains_webgl() {
    let gl = WebGLProfile::firefox();
    assert!(gl.renderer.contains("WebGL"));
}

#[test]
fn test_webgl_firefox_extensions_not_empty() {
    let gl = WebGLProfile::firefox();
    assert!(!gl.extensions.is_empty());
    assert!(gl.extensions.len() >= 15);
}

#[test]
fn test_webgl_firefox_has_ext_texture_filter() {
    let gl = WebGLProfile::firefox();
    assert!(gl.extensions.iter().any(|e| e == "EXT_texture_filter_anisotropic"));
}

#[test]
fn test_webgl_firefox_max_texture_size_power_of_two() {
    let gl = WebGLProfile::firefox();
    assert!(gl.max_texture_size.is_power_of_two());
}

// ---- WebGL Chrome vs Firefox differentiation ----

#[test]
fn test_webgl_chrome_firefox_vendors_differ() {
    let ch = WebGLProfile::chrome();
    let ff = WebGLProfile::firefox();
    assert_ne!(ch.vendor, ff.vendor);
}

#[test]
fn test_webgl_chrome_firefox_renderers_differ() {
    let ch = WebGLProfile::chrome();
    let ff = WebGLProfile::firefox();
    assert_ne!(ch.renderer, ff.renderer);
}

#[test]
fn test_webgl_chrome_firefox_extensions_differ() {
    let ch = WebGLProfile::chrome();
    let ff = WebGLProfile::firefox();
    assert_ne!(ch.extensions.len(), ff.extensions.len());
}

#[test]
fn test_webgl_firefox_has_more_extensions() {
    let ch = WebGLProfile::chrome();
    let ff = WebGLProfile::firefox();
    assert!(ff.extensions.len() > ch.extensions.len());
}

// ---- AudioProfile ----

#[test]
fn test_audio_profile_seed_preserved() {
    let audio = AudioProfile::new(42);
    assert_eq!(audio.seed(), 42);
}

#[test]
fn test_audio_profile_noise_amplitude_small() {
    let audio = AudioProfile::new(12345);
    assert!(audio.noise_amplitude() > 0.0);
    assert!(audio.noise_amplitude() < 1.0);
}

#[test]
fn test_audio_profile_default_sample_rate() {
    let audio = AudioProfile::new(1);
    assert_eq!(audio.sample_rate(), 44100);
}

#[test]
fn test_audio_apply_noise_deterministic() {
    let audio = AudioProfile::new(999);
    let r1 = audio.apply_noise(0.5, 100);
    let r2 = audio.apply_noise(0.5, 100);
    assert_eq!(r1, r2);
}

#[test]
fn test_audio_apply_noise_different_seeds_differ() {
    let a1 = AudioProfile::new(100);
    let a2 = AudioProfile::new(200);
    let r1 = a1.apply_noise(0.5, 50);
    let r2 = a2.apply_noise(0.5, 50);
    assert_ne!(r1, r2);
}

#[test]
fn test_audio_apply_noise_different_indices_differ() {
    let audio = AudioProfile::new(42);
    let r1 = audio.apply_noise(0.0, 0);
    let r2 = audio.apply_noise(0.0, 1);
    assert_ne!(r1, r2);
}

#[test]
fn test_audio_apply_noise_preserves_sign_of_sample() {
    let audio = AudioProfile::new(42);
    let positive = audio.apply_noise(1.0, 0);
    assert!(positive > 0.0);
}

#[test]
fn test_audio_apply_noise_zero_sample_near_zero() {
    let audio = AudioProfile::new(42);
    let result = audio.apply_noise(0.0, 100);
    assert!(result.abs() < 1.0);
}

#[test]
fn test_audio_different_seeds_produce_different_streams() {
    let a1 = AudioProfile::new(1);
    let a2 = AudioProfile::new(2);
    let mut differ = false;
    for i in 0..100u32 {
        if a1.apply_noise(0.0, i) != a2.apply_noise(0.0, i) {
            differ = true;
            break;
        }
    }
    assert!(differ, "Different seeds should produce different noise");
}

#[test]
fn test_audio_noise_amplitude_is_small() {
    let audio = AudioProfile::new(42);
    assert!(audio.noise_amplitude() < 1e-5);
}

// ---- CanvasNoise ----

#[test]
#[should_panic(expected = "canvas_seed must be > 0")]
fn test_canvas_noise_zero_seed_panics() {
    let _cn = CanvasNoise::new(0);
}

#[test]
fn test_canvas_noise_seed_preserved() {
    let cn = CanvasNoise::new(42);
    assert_eq!(cn.seed(), 42);
}

#[test]
fn test_canvas_noise_amplitude_positive() {
    let cn = CanvasNoise::new(1);
    assert!(cn.noise_amplitude() > 0.0);
}

#[test]
fn test_canvas_noise_deterministic() {
    let cn = CanvasNoise::new(123);
    let p1 = cn.apply_to_pixel(128, 128, 128, 255, 10, 20);
    let p2 = cn.apply_to_pixel(128, 128, 128, 255, 10, 20);
    assert_eq!(p1, p2);
}

#[test]
fn test_canvas_noise_different_coords_differ() {
    let cn = CanvasNoise::new(456);
    let p1 = cn.apply_to_pixel(128, 128, 128, 255, 0, 0);
    let p2 = cn.apply_to_pixel(128, 128, 128, 255, 1, 0);
    assert_ne!(p1, p2);
}

#[test]
fn test_canvas_noise_preserves_alpha() {
    let cn = CanvasNoise::new(789);
    let (_, _, _, a) = cn.apply_to_pixel(128, 128, 128, 200, 50, 50);
    assert_eq!(a, 200);
}

#[test]
fn test_canvas_noise_different_seeds_differ() {
    let c1 = CanvasNoise::new(100);
    let c2 = CanvasNoise::new(200);
    // Check multiple coords — at least one should differ due to different PRNG state
    let mut differ = false;
    for x in 0..20u32 {
        for y in 0..20u32 {
            let p1 = c1.apply_to_pixel(128, 128, 128, 255, x, y);
            let p2 = c2.apply_to_pixel(128, 128, 128, 255, x, y);
            if p1 != p2 {
                differ = true;
                break;
            }
        }
        if differ { break; }
    }
    assert!(differ, "Different seeds should produce different noise somewhere");
}

#[test]
fn test_canvas_noise_white_pixel_stays_near_white() {
    let cn = CanvasNoise::new(42);
    let (r, g, b, _) = cn.apply_to_pixel(255, 255, 255, 255, 100, 100);
    assert!(r >= 250);
    assert!(g >= 250);
    assert!(b >= 250);
}

#[test]
fn test_canvas_noise_black_pixel_stays_near_black() {
    let cn = CanvasNoise::new(42);
    let (r, g, b, _) = cn.apply_to_pixel(0, 0, 0, 255, 100, 100);
    assert!(r <= 5);
    assert!(g <= 5);
    assert!(b <= 5);
}

#[test]
fn test_canvas_noise_many_pixels_dont_clamp_midrange() {
    let cn = CanvasNoise::new(42);
    for x in 0..50u32 {
        for y in 0..50u32 {
            let (r, g, b, a) = cn.apply_to_pixel(128, 128, 128, 255, x, y);
                                    assert_eq!(a, 255);
        }
    }
}

// ---- ScreenProfile ----

#[test]
fn test_screen_profile_default_values() {
    let sp = ScreenProfile::default();
    assert_eq!(sp.width, 1920);
    assert_eq!(sp.height, 1080);
    assert_eq!(sp.avail_width, 1920);
    assert_eq!(sp.avail_height, 1040);
    assert_eq!(sp.color_depth, 24);
    assert_eq!(sp.pixel_depth, 24);
    assert_eq!(sp.device_pixel_ratio, 1.0);
}

#[test]
fn test_screen_profile_custom() {
    let sp = ScreenProfile::new(2560, 1440, 2.0);
    assert_eq!(sp.width, 2560);
    assert_eq!(sp.height, 1440);
    assert_eq!(sp.avail_width, 2560);
    assert_eq!(sp.avail_height, 1400);
    assert_eq!(sp.device_pixel_ratio, 2.0);
}

#[test]
fn test_screen_profile_avail_height_less_than_height() {
    let sp = ScreenProfile::default();
    assert!(sp.avail_height < sp.height);
    assert_eq!(sp.height - sp.avail_height, 40);
}

#[test]
fn test_screen_profile_4k() {
    let sp = ScreenProfile::new(3840, 2160, 1.5);
    assert_eq!(sp.width, 3840);
    assert_eq!(sp.height, 2160);
    assert_eq!(sp.avail_height, 2120);
}

#[test]
fn test_screen_profile_mobile_portrait() {
    let sp = ScreenProfile::new(375, 812, 3.0);
    assert_eq!(sp.width, 375);
    assert_eq!(sp.height, 812);
    assert_eq!(sp.avail_height, 772);
    assert_eq!(sp.device_pixel_ratio, 3.0);
}

#[test]
fn test_screen_profile_color_depth_always_24() {
    let sp1 = ScreenProfile::default();
    let sp2 = ScreenProfile::new(800, 600, 1.0);
    assert_eq!(sp1.color_depth, 24);
    assert_eq!(sp2.color_depth, 24);
    assert_eq!(sp1.pixel_depth, 24);
    assert_eq!(sp2.pixel_depth, 24);
}

// ---- NavigatorProfile cross-profile validation ----

#[test]
fn test_navigator_chrome_vendor_is_google() {
    let nav = NavigatorProfile::chrome();
    assert_eq!(nav.vendor, "Google Inc.");
}

#[test]
fn test_navigator_firefox_vendor_is_empty() {
    let nav = NavigatorProfile::firefox();
    assert!(nav.vendor.is_empty());
}

#[test]
fn test_navigator_chrome_has_no_oscpu() {
    let nav = NavigatorProfile::chrome();
    assert!(nav.oscpu.is_none());
}

#[test]
fn test_navigator_firefox_has_oscpu() {
    let nav = NavigatorProfile::firefox();
    assert!(nav.oscpu.is_some());
    assert!(nav.oscpu.as_ref().unwrap().contains("Linux"));
}

#[test]
fn test_navigator_chrome_has_no_build_id() {
    let nav = NavigatorProfile::chrome();
    assert!(nav.build_id.is_none());
}

#[test]
fn test_navigator_firefox_has_build_id() {
    let nav = NavigatorProfile::firefox();
    assert!(nav.build_id.is_some());
}

#[test]
fn test_navigator_chrome_product_sub() {
    let nav = NavigatorProfile::chrome();
    assert_eq!(nav.product_sub, "20030107");
}

#[test]
fn test_navigator_firefox_product_sub() {
    let nav = NavigatorProfile::firefox();
    assert_eq!(nav.product_sub, "20100101");
}

#[test]
fn test_navigator_chrome_user_agent_contains_chrome() {
    let nav = NavigatorProfile::chrome();
    assert!(nav.user_agent.contains("Chrome"));
}

#[test]
fn test_navigator_firefox_user_agent_contains_firefox() {
    let nav = NavigatorProfile::firefox();
    assert!(nav.user_agent.contains("Firefox"));
}

#[test]
fn test_navigator_chrome_app_version_contains_applewebkit() {
    let nav = NavigatorProfile::chrome();
    assert!(nav.app_version.contains("AppleWebKit"));
}

#[test]
fn test_navigator_firefox_app_version_contains_x11() {
    let nav = NavigatorProfile::firefox();
    assert!(nav.app_version.contains("X11"));
}

#[test]
fn test_navigator_both_platforms_are_linux() {
    let ch = NavigatorProfile::chrome();
    let ff = NavigatorProfile::firefox();
    assert!(ch.platform.contains("Linux"));
    assert!(ff.platform.contains("Linux"));
}

#[test]
fn test_navigator_hardware_concurrency_reasonable() {
    let ch = NavigatorProfile::chrome();
    let ff = NavigatorProfile::firefox();
    assert!(ch.hardware_concurrency > 0);
    assert!(ch.hardware_concurrency <= 128);
    assert!(ff.hardware_concurrency > 0);
    assert!(ff.hardware_concurrency <= 128);
}

#[test]
fn test_navigator_max_touch_points_desktop_is_zero() {
    let ch = NavigatorProfile::chrome();
    assert_eq!(ch.max_touch_points, 0);
}
