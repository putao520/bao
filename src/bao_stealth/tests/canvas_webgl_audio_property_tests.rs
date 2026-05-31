// @trace TEST-STL-048 [req:REQ-STL-003,REQ-STL-005] [level:unit]
// CanvasNoise pixel boundary tests, WebGLProfile field completeness,
// AudioProfile noise properties, cross-profile isolation.

use bao_stealth::{CanvasNoise, WebGLProfile, AudioProfile};

// ---- CanvasNoise construction ----

#[test]
fn test_canvas_noise_seed_positive() {
    let cn = CanvasNoise::new(42);
    assert_eq!(cn.seed(), 42);
}

#[test]
fn test_canvas_noise_seed_max() {
    let cn = CanvasNoise::new(u64::MAX);
    assert_eq!(cn.seed(), u64::MAX);
}

#[test]
#[should_panic(expected = "canvas_seed must be > 0")]
fn test_canvas_noise_seed_zero_panics() {
    let _cn = CanvasNoise::new(0);
}

#[test]
fn test_canvas_noise_amplitude() {
    let cn = CanvasNoise::new(1);
    assert!((cn.noise_amplitude() - 0.001).abs() < f64::EPSILON);
}

#[test]
fn test_canvas_noise_debug() {
    let cn = CanvasNoise::new(42);
    let debug = format!("{:?}", cn);
    assert!(debug.contains("CanvasNoise"));
}

#[test]
fn test_canvas_noise_clone() {
    let cn = CanvasNoise::new(123);
    let cloned = cn.clone();
    assert_eq!(cloned.seed(), 123);
    assert_eq!(cloned.noise_amplitude(), cn.noise_amplitude());
}

// ---- CanvasNoise apply_to_pixel ----

#[test]
fn test_apply_to_pixel_black_pixel() {
    let cn = CanvasNoise::new(42);
    let (r, g, b, a) = cn.apply_to_pixel(0, 0, 0, 255, 10, 20);
    assert_eq!(a, 255);
    // Noise should be small, channels near 0
    assert!(r < 5, "r={} too far from 0", r);
    assert!(g < 5, "g={} too far from 0", g);
    assert!(b < 5, "b={} too far from 0", b);
}

#[test]
fn test_apply_to_pixel_white_pixel() {
    let cn = CanvasNoise::new(42);
    let (r, g, b, a) = cn.apply_to_pixel(255, 255, 255, 255, 10, 20);
    assert_eq!(a, 255);
    // Noise should be small, channels near 255
    assert!(r > 250, "r={} too far from 255", r);
    assert!(g > 250, "g={} too far from 255", g);
    assert!(b > 250, "b={} too far from 255", b);
}

#[test]
fn test_apply_to_pixel_alpha_unchanged() {
    let cn = CanvasNoise::new(42);
    let (_, _, _, a) = cn.apply_to_pixel(128, 128, 128, 200, 10, 20);
    assert_eq!(a, 200);
}

#[test]
fn test_apply_to_pixel_alpha_zero() {
    let cn = CanvasNoise::new(42);
    let (_, _, _, a) = cn.apply_to_pixel(128, 128, 128, 0, 10, 20);
    assert_eq!(a, 0);
}

#[test]
fn test_apply_to_pixel_deterministic() {
    let cn = CanvasNoise::new(42);
    let p1 = cn.apply_to_pixel(128, 64, 32, 255, 100, 200);
    let p2 = cn.apply_to_pixel(128, 64, 32, 255, 100, 200);
    assert_eq!(p1, p2);
}

#[test]
fn test_apply_to_pixel_many_positions_not_all_same() {
    let cn = CanvasNoise::new(42);
    let pixels: Vec<_> = (0..20)
        .map(|i| cn.apply_to_pixel(128, 128, 128, 255, i, i))
        .collect();
    // With 20 different positions, not all should be identical
    let first = pixels[0];
    let all_same = pixels.iter().all(|p| *p == first);
    assert!(!all_same, "All pixels identical across 20 positions");
}

#[test]
fn test_apply_to_pixel_many_seeds_not_all_same() {
    let pixels: Vec<_> = (1..=20)
        .map(|seed| {
            let cn = CanvasNoise::new(seed);
            cn.apply_to_pixel(128, 128, 128, 255, 50, 50)
        })
        .collect();
    let first = pixels[0];
    let all_same = pixels.iter().all(|p| *p == first);
    assert!(!all_same, "All pixels identical across 20 seeds");
}

#[test]
fn test_apply_to_pixel_clamp_at_zero() {
    let cn = CanvasNoise::new(42);
    // Noise range is [-0.5, 0.5], with 0 amplitude it stays 0
    // With very low values (0,0,0), negative noise clamps to 0
    let (r, _, _, _) = cn.apply_to_pixel(0, 0, 0, 255, 0, 0);
    // r should be 0 or very small (0 + noise*0.001*255 can be -0.127 → clamped to 0)
    assert!(r == 0 || r <= 1);
}

#[test]
fn test_apply_to_pixel_clamp_at_255() {
    let cn = CanvasNoise::new(42);
    let (r, _, _, _) = cn.apply_to_pixel(255, 255, 255, 255, 0, 0);
    assert!(r >= 254);
}

// ---- WebGLProfile firefox ----

#[test]
fn test_webgl_firefox_vendor() {
    let w = WebGLProfile::firefox();
    assert_eq!(w.vendor, "Mozilla");
}

#[test]
fn test_webgl_firefox_renderer_contains_webgl() {
    let w = WebGLProfile::firefox();
    assert!(w.renderer.contains("WebGL"));
}

#[test]
fn test_webgl_firefox_extensions_nonempty() {
    let w = WebGLProfile::firefox();
    assert!(!w.extensions.is_empty());
    assert!(w.extensions.len() > 10);
}

#[test]
fn test_webgl_firefox_extensions_common_ones() {
    let w = WebGLProfile::firefox();
    assert!(w.extensions.iter().any(|e| e.contains("ANGLE_instanced_arrays")));
    assert!(w.extensions.iter().any(|e| e.contains("WEBGL_lose_context")));
    assert!(w.extensions.iter().any(|e| e.contains("OES_texture_float")));
}

#[test]
fn test_webgl_firefox_max_texture_size() {
    assert_eq!(WebGLProfile::firefox().max_texture_size, 16384);
}

#[test]
fn test_webgl_firefox_max_renderbuffer_size() {
    assert_eq!(WebGLProfile::firefox().max_renderbuffer_size, 16384);
}

#[test]
fn test_webgl_firefox_max_viewport_dims() {
    let w = WebGLProfile::firefox();
    assert_eq!(w.max_viewport_dims, [16384, 16384]);
}

// ---- WebGLProfile chrome ----

#[test]
fn test_webgl_chrome_vendor() {
    let w = WebGLProfile::chrome();
    assert_eq!(w.vendor, "Google Inc. (NVIDIA)");
}

#[test]
fn test_webgl_chrome_renderer_contains_angle() {
    let w = WebGLProfile::chrome();
    assert!(w.renderer.contains("ANGLE"));
}

#[test]
fn test_webgl_chrome_extensions_nonempty() {
    let w = WebGLProfile::chrome();
    assert!(!w.extensions.is_empty());
}

#[test]
fn test_webgl_chrome_max_texture_size() {
    assert_eq!(WebGLProfile::chrome().max_texture_size, 16384);
}

#[test]
fn test_webgl_chrome_max_viewport_dims() {
    assert_eq!(WebGLProfile::chrome().max_viewport_dims, [16384, 16384]);
}

// ---- WebGLProfile cross-profile ----

#[test]
fn test_webgl_firefox_chrome_vendor_differ() {
    assert_ne!(WebGLProfile::firefox().vendor, WebGLProfile::chrome().vendor);
}

#[test]
fn test_webgl_firefox_chrome_renderer_differ() {
    assert_ne!(WebGLProfile::firefox().renderer, WebGLProfile::chrome().renderer);
}

#[test]
fn test_webgl_firefox_chrome_extensions_differ() {
    let f = WebGLProfile::firefox();
    let c = WebGLProfile::chrome();
    assert_ne!(f.extensions.len(), c.extensions.len());
}

#[test]
fn test_webgl_firefox_has_more_extensions() {
    let f = WebGLProfile::firefox();
    let c = WebGLProfile::chrome();
    assert!(f.extensions.len() > c.extensions.len());
}

// ---- WebGLProfile Debug/Clone ----

#[test]
fn test_webgl_debug() {
    let w = WebGLProfile::firefox();
    let debug = format!("{:?}", w);
    assert!(debug.contains("WebGLProfile"));
    assert!(debug.contains("Mozilla"));
}

#[test]
fn test_webgl_clone() {
    let w = WebGLProfile::chrome();
    let cloned = w.clone();
    assert_eq!(cloned.vendor, w.vendor);
    assert_eq!(cloned.extensions.len(), w.extensions.len());
    assert_eq!(cloned.max_texture_size, w.max_texture_size);
}

// ---- AudioProfile construction ----

#[test]
fn test_audio_new_seed() {
    let a = AudioProfile::new(42);
    assert_eq!(a.seed(), 42);
}

#[test]
fn test_audio_new_zero_seed() {
    // AudioProfile::new allows seed=0 (unlike CanvasNoise)
    let a = AudioProfile::new(0);
    assert_eq!(a.seed(), 0);
}

#[test]
fn test_audio_noise_amplitude() {
    let a = AudioProfile::new(1);
    assert!((a.noise_amplitude() - 1e-7).abs() < f64::EPSILON);
}

#[test]
fn test_audio_sample_rate() {
    let a = AudioProfile::new(1);
    assert_eq!(a.sample_rate(), 44100);
}

#[test]
fn test_audio_debug() {
    let a = AudioProfile::new(42);
    let debug = format!("{:?}", a);
    assert!(debug.contains("AudioProfile"));
}

#[test]
fn test_audio_clone() {
    let a = AudioProfile::new(999);
    let cloned = a.clone();
    assert_eq!(cloned.seed(), 999);
    assert_eq!(cloned.sample_rate(), a.sample_rate());
}

// ---- AudioProfile apply_noise ----

#[test]
fn test_audio_apply_noise_deterministic() {
    let a = AudioProfile::new(42);
    let n1 = a.apply_noise(1.0, 100);
    let n2 = a.apply_noise(1.0, 100);
    assert_eq!(n1, n2);
}

#[test]
fn test_audio_apply_noise_small_perturbation() {
    let a = AudioProfile::new(42);
    let original = 0.5;
    let noisy = a.apply_noise(original, 0);
    // Noise amplitude is 1e-7, so perturbation should be tiny
    assert!((noisy - original).abs() < 1e-5);
}

#[test]
fn test_audio_apply_noise_different_indices() {
    let a = AudioProfile::new(42);
    let n1 = a.apply_noise(0.0, 0);
    let n2 = a.apply_noise(0.0, 1);
    // Different indices should produce different noise
    assert_ne!(n1, n2);
}

#[test]
fn test_audio_apply_noise_different_seeds() {
    let a1 = AudioProfile::new(1);
    let a2 = AudioProfile::new(2);
    let n1 = a1.apply_noise(1.0, 100);
    let n2 = a2.apply_noise(1.0, 100);
    assert_ne!(n1, n2);
}

#[test]
fn test_audio_apply_noise_zero_sample() {
    let a = AudioProfile::new(42);
    let noisy = a.apply_noise(0.0, 50);
    // Should be very close to 0
    assert!(noisy.abs() < 1e-5);
}

#[test]
fn test_audio_apply_noise_negative_sample() {
    let a = AudioProfile::new(42);
    let noisy = a.apply_noise(-1.0, 50);
    // Should be very close to -1.0
    assert!((noisy - (-1.0)).abs() < 1e-5);
}

#[test]
fn test_audio_apply_noise_large_index() {
    let a = AudioProfile::new(42);
    let noisy = a.apply_noise(0.5, u32::MAX);
    // Should not panic and produce valid result
    assert!(noisy.is_finite());
}

// ---- WebGLProfile custom construction ----

#[test]
fn test_webgl_custom_profile() {
    let custom = WebGLProfile {
        vendor: "CustomVendor".into(),
        renderer: "CustomRenderer".into(),
        extensions: vec!["EXT_test".into()],
        max_texture_size: 8192,
        max_renderbuffer_size: 8192,
        max_viewport_dims: [4096, 4096],
    };
    assert_eq!(custom.vendor, "CustomVendor");
    assert_eq!(custom.max_texture_size, 8192);
    assert_eq!(custom.extensions.len(), 1);
}
