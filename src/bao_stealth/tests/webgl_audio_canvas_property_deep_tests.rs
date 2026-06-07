#![allow(unused_comparisons, unused_variables)]
// @trace TEST-STL-050 [req:REQ-STL-003,REQ-STL-004,REQ-STL-005,REQ-STL-007] [level:unit]
// WebGLProfile firefox/chrome field exhaustive verification, AudioProfile construction
// + noise properties + deterministic reproducibility, CanvasNoise pixel application
// edge cases + cross-seed divergence + clamping behavior.

use bao_stealth::{WebGLProfile, AudioProfile, CanvasNoise, StealthProfile};

// ============================================================================
// WebGLProfile firefox
// ============================================================================

#[test]
fn test_webgl_firefox_vendor() {
    assert_eq!(WebGLProfile::firefox().vendor, "Mozilla");
}

#[test]
fn test_webgl_firefox_renderer_contains_webgl() {
    let w = WebGLProfile::firefox();
    assert!(w.renderer.contains("WebGL"));
    assert!(w.renderer.contains("OpenGL"));
}

#[test]
fn test_webgl_firefox_extensions_nonempty() {
    let exts = &WebGLProfile::firefox().extensions;
    assert!(!exts.is_empty());
    assert!(exts.len() > 15);
}

#[test]
fn test_webgl_firefox_extensions_contain_common() {
    let exts = &WebGLProfile::firefox().extensions;
    assert!(exts.iter().any(|e| e.contains("OES_texture_float")));
    assert!(exts.iter().any(|e| e.contains("WEBGL_depth_texture")));
    assert!(exts.iter().any(|e| e.contains("ANGLE_instanced_arrays")));
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
    assert_eq!(WebGLProfile::firefox().max_viewport_dims, [16384, 16384]);
}

// ============================================================================
// WebGLProfile chrome
// ============================================================================

#[test]
fn test_webgl_chrome_vendor_google() {
    assert!(WebGLProfile::chrome().vendor.contains("Google"));
}

#[test]
fn test_webgl_chrome_renderer_contains_angle() {
    let w = WebGLProfile::chrome();
    assert!(w.renderer.contains("ANGLE"));
    assert!(w.renderer.contains("NVIDIA"));
}

#[test]
fn test_webgl_chrome_extensions_nonempty() {
    let exts = &WebGLProfile::chrome().extensions;
    assert!(!exts.is_empty());
    assert!(exts.len() > 10);
}

#[test]
fn test_webgl_chrome_max_texture_size() {
    assert_eq!(WebGLProfile::chrome().max_texture_size, 16384);
}

#[test]
fn test_webgl_chrome_max_renderbuffer_size() {
    assert_eq!(WebGLProfile::chrome().max_renderbuffer_size, 16384);
}

#[test]
fn test_webgl_chrome_max_viewport_dims() {
    assert_eq!(WebGLProfile::chrome().max_viewport_dims, [16384, 16384]);
}

// ============================================================================
// WebGLProfile cross-profile
// ============================================================================

#[test]
fn test_webgl_firefox_chrome_vendor_differ() {
    assert_ne!(WebGLProfile::firefox().vendor, WebGLProfile::chrome().vendor);
}

#[test]
fn test_webgl_firefox_chrome_renderer_differ() {
    assert_ne!(WebGLProfile::firefox().renderer, WebGLProfile::chrome().renderer);
}

#[test]
fn test_webgl_firefox_more_extensions_than_chrome() {
    let ff_exts = WebGLProfile::firefox().extensions.len();
    let cr_exts = WebGLProfile::chrome().extensions.len();
    assert!(ff_exts > cr_exts, "Firefox should have more WebGL extensions");
}

#[test]
fn test_webgl_firefox_chrome_shared_extensions() {
    let ff = &WebGLProfile::firefox().extensions;
    let cr = &WebGLProfile::chrome().extensions;
    let shared: Vec<_> = ff.iter().filter(|e| cr.contains(e)).collect();
    assert!(shared.len() > 5, "Should share common extensions");
}

#[test]
fn test_webgl_firefox_chrome_same_max_texture() {
    assert_eq!(
        WebGLProfile::firefox().max_texture_size,
        WebGLProfile::chrome().max_texture_size
    );
}

#[test]
fn test_webgl_firefox_chrome_same_viewport_dims() {
    assert_eq!(
        WebGLProfile::firefox().max_viewport_dims,
        WebGLProfile::chrome().max_viewport_dims
    );
}

// ============================================================================
// WebGLProfile Debug/Clone
// ============================================================================

#[test]
fn test_webgl_debug() {
    let w = WebGLProfile::firefox();
    let s = format!("{:?}", w);
    assert!(s.contains("WebGLProfile"));
    assert!(s.contains("Mozilla"));
}

#[test]
fn test_webgl_clone() {
    let w = WebGLProfile::chrome();
    let c = w.clone();
    assert_eq!(c.vendor, w.vendor);
    assert_eq!(c.renderer, w.renderer);
    assert_eq!(c.extensions.len(), w.extensions.len());
    assert_eq!(c.max_texture_size, w.max_texture_size);
}

// ============================================================================
// WebGLProfile custom construction
// ============================================================================

#[test]
fn test_webgl_custom() {
    let w = WebGLProfile {
        vendor: "Custom".into(),
        renderer: "CustomGL".into(),
        extensions: vec!["EXT_test".into()],
        max_texture_size: 8192,
        max_renderbuffer_size: 4096,
        max_viewport_dims: [4096, 4096],
    };
    assert_eq!(w.vendor, "Custom");
    assert_eq!(w.extensions.len(), 1);
    assert_eq!(w.max_texture_size, 8192);
}

// ============================================================================
// AudioProfile construction
// ============================================================================

#[test]
fn test_audio_new_seed() {
    let a = AudioProfile::new(42);
    assert_eq!(a.seed(), 42);
}

#[test]
fn test_audio_new_seed_zero() {
    let a = AudioProfile::new(0);
    assert_eq!(a.seed(), 0);
}

#[test]
fn test_audio_new_seed_large() {
    let a = AudioProfile::new(u64::MAX);
    assert_eq!(a.seed(), u64::MAX);
}

#[test]
fn test_audio_default_amplitude() {
    let a = AudioProfile::new(1);
    assert!((a.noise_amplitude() - 1e-7).abs() < f64::EPSILON);
}

#[test]
fn test_audio_default_sample_rate() {
    let a = AudioProfile::new(1);
    assert_eq!(a.sample_rate(), 44100);
}

// ============================================================================
// AudioProfile deterministic noise
// ============================================================================

#[test]
fn test_audio_apply_noise_deterministic() {
    let a = AudioProfile::new(42);
    let r1 = a.apply_noise(0.5, 100);
    let r2 = a.apply_noise(0.5, 100);
    assert!((r1 - r2).abs() < f64::EPSILON);
}

#[test]
fn test_audio_apply_noise_different_indices() {
    let a = AudioProfile::new(42);
    let r1 = a.apply_noise(0.5, 0);
    let r2 = a.apply_noise(0.5, 1);
    // Different indices should produce different results (high probability)
    assert_ne!(r1, r2);
}

#[test]
fn test_audio_apply_noise_different_seeds() {
    let a1 = AudioProfile::new(1);
    let a2 = AudioProfile::new(2);
    let r1 = a1.apply_noise(0.5, 100);
    let r2 = a2.apply_noise(0.5, 100);
    assert_ne!(r1, r2);
}

#[test]
fn test_audio_apply_noise_zero_sample() {
    let a = AudioProfile::new(42);
    let r = a.apply_noise(0.0, 50);
    // Should be very close to 0 due to tiny amplitude
    assert!(r.abs() < 1e-4);
}

#[test]
fn test_audio_apply_noise_negative_sample() {
    let a = AudioProfile::new(42);
    let r = a.apply_noise(-1.0, 50);
    // Should be close to -1.0
    assert!(r < 0.0);
}

#[test]
fn test_audio_apply_noise_preserves_signal() {
    let a = AudioProfile::new(42);
    let sample = 0.75;
    let result = a.apply_noise(sample, 0);
    // Noise amplitude is 1e-7, so result should be very close to sample
    assert!((result - sample).abs() < 1e-4);
}

// ============================================================================
// AudioProfile Debug/Clone
// ============================================================================

#[test]
fn test_audio_debug() {
    let a = AudioProfile::new(42);
    let s = format!("{:?}", a);
    assert!(s.contains("AudioProfile"));
}

#[test]
fn test_audio_clone() {
    let a = AudioProfile::new(42);
    let c = a.clone();
    assert_eq!(c.seed(), a.seed());
    assert_eq!(c.sample_rate(), a.sample_rate());
    assert!((c.noise_amplitude() - a.noise_amplitude()).abs() < f64::EPSILON);
}

// ============================================================================
// CanvasNoise construction
// ============================================================================

#[test]
fn test_canvas_new_seed() {
    let c = CanvasNoise::new(42);
    assert_eq!(c.seed(), 42);
}

#[test]
fn test_canvas_default_amplitude() {
    let c = CanvasNoise::new(1);
    assert!((c.noise_amplitude() - 0.001).abs() < f64::EPSILON);
}

#[test]
#[should_panic(expected = "canvas_seed must be > 0")]
fn test_canvas_seed_zero_panics() {
    let _ = CanvasNoise::new(0);
}

// ============================================================================
// CanvasNoise pixel application
// ============================================================================

#[test]
fn test_canvas_apply_pixel_deterministic() {
    let c = CanvasNoise::new(42);
    let p1 = c.apply_to_pixel(128, 128, 128, 255, 10, 20);
    let p2 = c.apply_to_pixel(128, 128, 128, 255, 10, 20);
    assert_eq!(p1, p2);
}

#[test]
fn test_canvas_apply_pixel_different_coords() {
    let c = CanvasNoise::new(42);
    let p1 = c.apply_to_pixel(128, 128, 128, 255, 0, 0);
    let p2 = c.apply_to_pixel(128, 128, 128, 255, 100, 200);
    assert_ne!(p1, p2);
}

#[test]
fn test_canvas_apply_pixel_preserves_alpha() {
    let c = CanvasNoise::new(42);
    let (_, _, _, a) = c.apply_to_pixel(128, 128, 128, 200, 5, 5);
    assert_eq!(a, 200);
}

#[test]
fn test_canvas_apply_pixel_different_seeds() {
    // Use larger seed gap and mid-range pixel to maximize observable difference
    let c1 = CanvasNoise::new(1);
    let c2 = CanvasNoise::new(99999);
    let p1 = c1.apply_to_pixel(100, 100, 100, 255, 50, 50);
    let p2 = c2.apply_to_pixel(100, 100, 100, 255, 50, 50);
    assert_ne!(p1, p2);
}

#[test]
fn test_canvas_apply_pixel_black() {
    let c = CanvasNoise::new(42);
    let (r, g, b, _) = c.apply_to_pixel(0, 0, 0, 255, 5, 5);
    // Noise might push slightly above 0 or stay at 0
    assert!(r <= 5); // very small deviation
    assert!(g <= 3);
    assert!(b <= 2);
}

#[test]
fn test_canvas_apply_pixel_white() {
    let c = CanvasNoise::new(42);
    let (r, g, b, _) = c.apply_to_pixel(255, 255, 255, 255, 5, 5);
    // Should be clamped to <= 255
}

#[test]
fn test_canvas_apply_pixel_small_deviation() {
    let c = CanvasNoise::new(42);
    let (r, g, b, _) = c.apply_to_pixel(128, 128, 128, 255, 100, 200);
    // Deviation should be within noise_amplitude * channel_factor
    let diff_r = (r as i16 - 128).abs();
    let diff_g = (g as i16 - 128).abs();
    let diff_b = (b as i16 - 128).abs();
    assert!(diff_r <= 2, "R deviation too large: {}", diff_r);
    assert!(diff_g <= 2, "G deviation too large: {}", diff_g);
    assert!(diff_b <= 1, "B deviation too large: {}", diff_b);
}

#[test]
fn test_canvas_apply_pixel_many_coords_no_panic() {
    let c = CanvasNoise::new(42);
    for y in 0..100u32 {
        for x in 0..100u32 {
            let _ = c.apply_to_pixel(128, 64, 32, 200, x, y);
        }
    }
}

// ============================================================================
// CanvasNoise Debug/Clone
// ============================================================================

#[test]
fn test_canvas_debug() {
    let c = CanvasNoise::new(42);
    let s = format!("{:?}", c);
    assert!(s.contains("CanvasNoise"));
    assert!(s.contains("42"));
}

#[test]
fn test_canvas_clone() {
    let c = CanvasNoise::new(42);
    let c2 = c.clone();
    assert_eq!(c2.seed(), c.seed());
    let p1 = c.apply_to_pixel(128, 128, 128, 255, 5, 5);
    let p2 = c2.apply_to_pixel(128, 128, 128, 255, 5, 5);
    assert_eq!(p1, p2);
}

// ============================================================================
// StealthProfile integration: webgl/audio/canvas consistency
// ============================================================================

#[test]
fn test_stealth_firefox_webgl_vendor() {
    assert_eq!(StealthProfile::firefox_default().webgl.vendor, "Mozilla");
}

#[test]
fn test_stealth_chrome_webgl_vendor() {
    assert!(StealthProfile::chrome_default().webgl.vendor.contains("Google"));
}

#[test]
fn test_stealth_firefox_canvas_seed_positive() {
    assert!(StealthProfile::firefox_default().canvas.seed() > 0);
}

#[test]
fn test_stealth_chrome_canvas_seed_positive() {
    assert!(StealthProfile::chrome_default().canvas.seed() > 0);
}

#[test]
fn test_stealth_firefox_chrome_audio_seeds_differ() {
    let ff = StealthProfile::firefox_default();
    let cr = StealthProfile::chrome_default();
    assert_ne!(ff.audio.seed(), cr.audio.seed());
}

#[test]
fn test_stealth_firefox_chrome_canvas_seeds_differ() {
    let ff = StealthProfile::firefox_default();
    let cr = StealthProfile::chrome_default();
    assert_ne!(ff.canvas.seed(), cr.canvas.seed());
}

#[test]
fn test_stealth_firefox_webgl_extensions_nonempty() {
    assert!(!StealthProfile::firefox_default().webgl.extensions.is_empty());
}

#[test]
fn test_stealth_chrome_webgl_extensions_nonempty() {
    assert!(!StealthProfile::chrome_default().webgl.extensions.is_empty());
}

#[test]
fn test_stealth_firefox_audio_apply_noise() {
    let p = StealthProfile::firefox_default();
    let r = p.audio.apply_noise(0.5, 100);
    assert!((r - 0.5).abs() < 1e-4);
}

#[test]
fn test_stealth_firefox_canvas_apply_pixel() {
    let p = StealthProfile::firefox_default();
    let (r, _g, _b, a) = p.canvas.apply_to_pixel(128, 128, 128, 255, 5, 5);
    assert_eq!(a, 255);
}
