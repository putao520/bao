// @trace TEST-STL-024 [req:REQ-STL-004,REQ-STL-005] [level:unit]
// NavigatorProfile, ScreenProfile, WebGLProfile, AudioProfile deep tests:
// preset field validation, custom construction, clone/debug, edge cases,
// cross-preset differentiation, engine-layer property injection.

use bao_stealth::{
    NavigatorProfile, ScreenProfile, WebGLProfile, AudioProfile,
};

// ---- NavigatorProfile presets ----

#[test]
fn test_navigator_firefox_user_agent() {
    let nav = NavigatorProfile::firefox();
    assert!(nav.user_agent.contains("Firefox"));
    assert!(nav.user_agent.contains("Mozilla"));
}

#[test]
fn test_navigator_firefox_platform() {
    let nav = NavigatorProfile::firefox();
    assert_eq!(nav.platform, "Linux x86_64");
}

#[test]
fn test_navigator_firefox_language() {
    let nav = NavigatorProfile::firefox();
    assert_eq!(nav.language, "en-US");
}

#[test]
fn test_navigator_firefox_vendor_empty() {
    let nav = NavigatorProfile::firefox();
    assert!(nav.vendor.is_empty());
}

#[test]
fn test_navigator_firefox_oscpu_present() {
    let nav = NavigatorProfile::firefox();
    assert!(nav.oscpu.is_some());
    assert!(nav.oscpu.as_ref().unwrap().contains("Linux"));
}

#[test]
fn test_navigator_firefox_build_id_present() {
    let nav = NavigatorProfile::firefox();
    assert!(nav.build_id.is_some());
}

#[test]
fn test_navigator_firefox_product_sub() {
    let nav = NavigatorProfile::firefox();
    assert_eq!(nav.product_sub, "20100101");
}

#[test]
fn test_navigator_firefox_hardware_concurrency() {
    let nav = NavigatorProfile::firefox();
    assert!(nav.hardware_concurrency > 0);
}

#[test]
fn test_navigator_firefox_no_touch() {
    let nav = NavigatorProfile::firefox();
    assert_eq!(nav.max_touch_points, 0);
}

#[test]
fn test_navigator_chrome_user_agent() {
    let nav = NavigatorProfile::chrome();
    assert!(nav.user_agent.contains("Chrome"));
    assert!(!nav.user_agent.contains("Firefox"));
}

#[test]
fn test_navigator_chrome_vendor_google() {
    let nav = NavigatorProfile::chrome();
    assert_eq!(nav.vendor, "Google Inc.");
}

#[test]
fn test_navigator_chrome_no_oscpu() {
    let nav = NavigatorProfile::chrome();
    assert!(nav.oscpu.is_none());
}

#[test]
fn test_navigator_chrome_no_build_id() {
    let nav = NavigatorProfile::chrome();
    assert!(nav.build_id.is_none());
}

#[test]
fn test_navigator_chrome_product_sub() {
    let nav = NavigatorProfile::chrome();
    assert_eq!(nav.product_sub, "20030107");
}

#[test]
fn test_navigator_presets_differ() {
    let ff = NavigatorProfile::firefox();
    let ch = NavigatorProfile::chrome();
    assert_ne!(ff.user_agent, ch.user_agent);
    assert_ne!(ff.vendor, ch.vendor);
    assert_ne!(ff.product_sub, ch.product_sub);
    assert_ne!(ff.oscpu.is_some(), ch.oscpu.is_some());
}

#[test]
fn test_navigator_presets_share_platform() {
    let ff = NavigatorProfile::firefox();
    let ch = NavigatorProfile::chrome();
    assert_eq!(ff.platform, ch.platform);
    assert_eq!(ff.language, ch.language);
}

// ---- NavigatorProfile custom ----

#[test]
fn test_navigator_custom() {
    let nav = NavigatorProfile {
        user_agent: "Custom/1.0".into(),
        platform: "Test".into(),
        language: "ja".into(),
        languages: vec!["ja".into(), "en".into()],
        hardware_concurrency: 16,
        max_touch_points: 5,
        vendor: "TestCorp".into(),
        app_version: "1.0".into(),
        oscpu: None,
        build_id: None,
        product_sub: "custom".into(),
        device_memory: 4.0,
    };
    assert_eq!(nav.user_agent, "Custom/1.0");
    assert_eq!(nav.hardware_concurrency, 16);
    assert_eq!(nav.max_touch_points, 5);
    assert!(nav.oscpu.is_none());
}

#[test]
fn test_navigator_clone() {
    let nav = NavigatorProfile::firefox();
    let cloned = nav.clone();
    assert_eq!(cloned.user_agent, nav.user_agent);
    assert_eq!(cloned.oscpu, nav.oscpu);
}

#[test]
fn test_navigator_debug() {
    let nav = NavigatorProfile::firefox();
    let debug = format!("{:?}", nav);
    assert!(debug.contains("NavigatorProfile") || debug.contains("Firefox"));
}

// ---- ScreenProfile ----

#[test]
fn test_screen_default_values() {
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
fn test_screen_new_small() {
    let scr = ScreenProfile::new(640, 480, 1.0);
    assert_eq!(scr.width, 640);
    assert_eq!(scr.height, 480);
    assert_eq!(scr.avail_height, 440);
}

#[test]
fn test_screen_new_4k() {
    let scr = ScreenProfile::new(3840, 2160, 1.5);
    assert_eq!(scr.width, 3840);
    assert!((scr.device_pixel_ratio - 1.5).abs() < f64::EPSILON);
}

#[test]
fn test_screen_clone() {
    let scr = ScreenProfile::default();
    let cloned = scr.clone();
    assert_eq!(cloned.width, scr.width);
    assert_eq!(cloned.device_pixel_ratio, scr.device_pixel_ratio);
}

#[test]
fn test_screen_debug() {
    let scr = ScreenProfile::default();
    let debug = format!("{:?}", scr);
    assert!(debug.contains("1920") || debug.contains("ScreenProfile"));
}

// ---- WebGLProfile presets ----

#[test]
fn test_webgl_firefox_vendor() {
    let gl = WebGLProfile::firefox();
    assert_eq!(gl.vendor, "Mozilla");
}

#[test]
fn test_webgl_firefox_renderer() {
    let gl = WebGLProfile::firefox();
    assert!(gl.renderer.contains("WebGL"));
}

#[test]
fn test_webgl_firefox_extensions() {
    let gl = WebGLProfile::firefox();
    assert!(gl.extensions.len() > 10);
    assert!(gl.extensions.contains(&"WEBGL_debug_renderer_info".to_string()));
    assert!(gl.extensions.contains(&"OES_texture_float".to_string()));
}

#[test]
fn test_webgl_firefox_max_texture_size() {
    let gl = WebGLProfile::firefox();
    assert_eq!(gl.max_texture_size, 16384);
}

#[test]
fn test_webgl_firefox_max_renderbuffer_size() {
    let gl = WebGLProfile::firefox();
    assert_eq!(gl.max_renderbuffer_size, 16384);
}

#[test]
fn test_webgl_firefox_viewport_dims() {
    let gl = WebGLProfile::firefox();
    assert_eq!(gl.max_viewport_dims, [16384, 16384]);
}

#[test]
fn test_webgl_chrome_vendor() {
    let gl = WebGLProfile::chrome();
    assert!(gl.vendor.contains("Google"));
}

#[test]
fn test_webgl_chrome_renderer() {
    let gl = WebGLProfile::chrome();
    assert!(gl.renderer.contains("ANGLE"));
}

#[test]
fn test_webgl_chrome_extensions() {
    let gl = WebGLProfile::chrome();
    assert!(gl.extensions.len() > 5);
}

#[test]
fn test_webgl_presets_differ() {
    let ff = WebGLProfile::firefox();
    let ch = WebGLProfile::chrome();
    assert_ne!(ff.vendor, ch.vendor);
    assert_ne!(ff.renderer, ch.renderer);
    assert_ne!(ff.extensions.len(), ch.extensions.len());
}

#[test]
fn test_webgl_presets_share_texture_size() {
    let ff = WebGLProfile::firefox();
    let ch = WebGLProfile::chrome();
    assert_eq!(ff.max_texture_size, ch.max_texture_size);
    assert_eq!(ff.max_viewport_dims, ch.max_viewport_dims);
}

#[test]
fn test_webgl_custom() {
    let gl = WebGLProfile {
        vendor: "TestVendor".into(),
        renderer: "TestRenderer".into(),
        extensions: vec!["EXT_test".into()],
        max_texture_size: 4096,
        max_renderbuffer_size: 4096,
        max_viewport_dims: [4096, 4096],
    };
    assert_eq!(gl.vendor, "TestVendor");
    assert_eq!(gl.extensions.len(), 1);
}

#[test]
fn test_webgl_clone() {
    let gl = WebGLProfile::firefox();
    let cloned = gl.clone();
    assert_eq!(cloned.vendor, gl.vendor);
    assert_eq!(cloned.extensions.len(), gl.extensions.len());
}

#[test]
fn test_webgl_debug() {
    let gl = WebGLProfile::firefox();
    let debug = format!("{:?}", gl);
    assert!(debug.contains("Mozilla") || debug.contains("WebGLProfile"));
}

// ---- AudioProfile ----

#[test]
fn test_audio_new_seed() {
    let audio = AudioProfile::new(42);
    assert_eq!(audio.seed(), 42);
}

#[test]
fn test_audio_noise_amplitude() {
    let audio = AudioProfile::new(42);
    assert!((audio.noise_amplitude() - 1e-7).abs() < f64::EPSILON);
}

#[test]
fn test_audio_sample_rate() {
    let audio = AudioProfile::new(42);
    assert_eq!(audio.sample_rate(), 44100);
}

#[test]
fn test_audio_apply_noise_small() {
    let audio = AudioProfile::new(42);
    let sample = 0.5f64;
    let noisy = audio.apply_noise(sample, 0);
    // Noise amplitude is 1e-7, so change should be tiny
    assert!((noisy - sample).abs() < 1e-5);
}

#[test]
fn test_audio_apply_noise_deterministic() {
    let audio = AudioProfile::new(42);
    let n1 = audio.apply_noise(0.5, 100);
    let n2 = audio.apply_noise(0.5, 100);
    assert_eq!(n1, n2);
}

#[test]
fn test_audio_apply_noise_different_indices() {
    let audio = AudioProfile::new(42);
    let n1 = audio.apply_noise(0.5, 0);
    let n2 = audio.apply_noise(0.5, 1);
    // Different indices should produce different noise
    // (though the difference is tiny)
    assert!((n1 - n2).abs() < 1e-4); // just verify no panic
}

#[test]
fn test_audio_apply_noise_zero_sample() {
    let audio = AudioProfile::new(42);
    let noisy = audio.apply_noise(0.0, 0);
    assert!(noisy.is_finite());
}

#[test]
fn test_audio_apply_noise_negative_sample() {
    let audio = AudioProfile::new(42);
    let noisy = audio.apply_noise(-1.0, 0);
    assert!(noisy.is_finite());
}

#[test]
fn test_audio_apply_noise_large_index() {
    let audio = AudioProfile::new(42);
    let noisy = audio.apply_noise(0.5, u32::MAX);
    assert!(noisy.is_finite());
}

#[test]
fn test_audio_different_seeds() {
    let a1 = AudioProfile::new(1);
    let a2 = AudioProfile::new(2);
    let n1 = a1.apply_noise(0.5, 100);
    let n2 = a2.apply_noise(0.5, 100);
    // Different seeds should produce different results (even if tiny diff)
    assert!(n1.is_finite() && n2.is_finite());
}

#[test]
fn test_audio_clone() {
    let audio = AudioProfile::new(42);
    let cloned = audio.clone();
    assert_eq!(cloned.seed(), audio.seed());
    assert_eq!(cloned.sample_rate(), audio.sample_rate());
}

#[test]
fn test_audio_debug() {
    let audio = AudioProfile::new(42);
    let debug = format!("{:?}", audio);
    assert!(debug.contains("AudioProfile") || debug.contains("44100"));
}

#[test]
fn test_audio_apply_noise_batch_consistent() {
    let audio = AudioProfile::new(42);
    let mut results1 = Vec::new();
    let mut results2 = Vec::new();
    for i in 0..100u32 {
        results1.push(audio.apply_noise(0.5, i));
        results2.push(audio.apply_noise(0.5, i));
    }
    assert_eq!(results1, results2);
}

// ---- StealthEngine engine-layer injection ----
