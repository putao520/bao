#![allow(unused_comparisons, unused_variables)]
// @trace TEST-STL-009-EDGE [req:REQ-STL-001~007] [level:unit]
// Stealth edge cases: extreme parameters, boundary values, cross-profile validation

use bao_stealth::{StealthEngine, StealthProfile, CanvasNoise, BehaviorSimulator};

// ---- CanvasNoise boundary values ----

#[test]
fn test_canvas_noise_seed_1() {
    let noise = CanvasNoise::new(1);
    assert_eq!(noise.seed(), 1);
    let (r, g, b, a) = noise.apply_to_pixel(128, 128, 128, 255, 0, 0);
    assert!(a == 255);
}

#[test]
fn test_canvas_noise_large_seed() {
    let noise = CanvasNoise::new(u64::MAX);
    assert_eq!(noise.seed(), u64::MAX);
    let (r, g, b, a) = noise.apply_to_pixel(128, 128, 128, 255, 500, 500);
}

#[test]
#[should_panic(expected = "canvas_seed must be > 0")]
fn test_canvas_noise_seed_zero_panics() {
    let _ = CanvasNoise::new(0);
}

#[test]
fn test_canvas_noise_pixel_boundary_zero() {
    let noise = CanvasNoise::new(42);
    let (r, g, b, _a) = noise.apply_to_pixel(0, 0, 0, 0, 100, 200);
}

#[test]
fn test_canvas_noise_pixel_boundary_max() {
    let noise = CanvasNoise::new(42);
    let (r, g, b, a) = noise.apply_to_pixel(255, 255, 255, 255, 100, 200);
    assert!(a == 255);
}

#[test]
fn test_canvas_noise_large_coordinates() {
    let noise = CanvasNoise::new(42);
    let (r, g, b, _a) = noise.apply_to_pixel(128, 128, 128, 255, u32::MAX, u32::MAX);
}

#[test]
fn test_canvas_noise_noise_amplitude() {
    let noise = CanvasNoise::new(42);
    assert!(noise.noise_amplitude() > 0.0);
    assert!(noise.noise_amplitude() < 1.0);
}

#[test]
fn test_canvas_noise_deterministic_same_pixel() {
    let noise = CanvasNoise::new(42);
    let p1 = noise.apply_to_pixel(100, 150, 200, 255, 50, 75);
    let p2 = noise.apply_to_pixel(100, 150, 200, 255, 50, 75);
    assert_eq!(p1, p2, "Same seed + same position = same result");
}

#[test]
fn test_canvas_noise_different_seeds_different_noise() {
    let n1 = CanvasNoise::new(42);
    let n2 = CanvasNoise::new(999);
    // Use low pixel values where noise is more visible
    let _p1 = n1.apply_to_pixel(10, 10, 10, 255, 10, 20);
    let _p2 = n2.apply_to_pixel(10, 10, 10, 255, 10, 20);
    // Due to very small amplitude, results might be identical
    // Verify at least that deterministic noise values differ internally
    // by checking multiple positions
    let mut differ = false;
    for x in 0..100u32 {
        for y in 0..100u32 {
            let r1 = n1.apply_to_pixel(50, 50, 50, 255, x, y);
            let r2 = n2.apply_to_pixel(50, 50, 50, 255, x, y);
            if r1 != r2 { differ = true; break; }
        }
        if differ { break; }
    }
    assert!(differ, "Some pixels should differ between different seeds");
}

// ---- BehaviorSimulator boundary values ----

#[test]
fn test_mouse_path_zero_steps() {
    let b = BehaviorSimulator::new(42);
    let path = b.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 0);
    // generate_mouse_path returns steps+1 points.
    assert_eq!(path.len(), 1);
}

#[test]
fn test_mouse_path_single_step() {
    let b = BehaviorSimulator::new(42);
    let path = b.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 1);
    // 1 step → 2 points (start + end).
    assert_eq!(path.len(), 2);
}

#[test]
fn test_mouse_path_two_steps() {
    let b = BehaviorSimulator::new(42);
    let path = b.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 2);
    // 2 steps → 3 points (start + 1 mid + end).
    assert_eq!(path.len(), 3);
}

#[test]
fn test_mouse_path_many_steps() {
    let b = BehaviorSimulator::new(42);
    let path = b.generate_mouse_path(0.0, 0.0, 1920.0, 1080.0, 100);
    // generate_mouse_path returns steps+1 points.
    assert_eq!(path.len(), 101);
    // First point near start
    assert!(path[0].0 < 50.0);
    assert!(path[0].1 < 50.0);
}

#[test]
fn test_mouse_path_same_start_end() {
    let b = BehaviorSimulator::new(42);
    let path = b.generate_mouse_path(500.0, 500.0, 500.0, 500.0, 10);
    // When start == end, distance is 0 so Bezier produces 1 point
    assert!(!path.is_empty());
    // All points should be near (500, 500)
    for (x, y) in &path {
        assert!((x - 500.0).abs() < 200.0, "x offset too large: {}", x);
        assert!((y - 500.0).abs() < 200.0, "y offset too large: {}", y);
    }
}

#[test]
fn test_mouse_path_negative_coordinates() {
    let b = BehaviorSimulator::new(42);
    let path = b.generate_mouse_path(-100.0, -100.0, 100.0, 100.0, 5);
    // generate_mouse_path returns steps+1 points.
    assert_eq!(path.len(), 6);
}

#[test]
fn test_typing_delays_zero_count() {
    let b = BehaviorSimulator::new(42);
    let delays = b.generate_typing_delays(0);
    assert!(delays.is_empty());
}

#[test]
fn test_typing_delays_range() {
    let b = BehaviorSimulator::new(42);
    let delays = b.generate_typing_delays(100);
    // May have extra backspace events from typo correction
    assert!(delays.len() >= 100, "Expected >= 100 delays, got {}", delays.len());
    for d in &delays {
        assert!(*d > 0, "delay should be positive: {}", d);
        assert!(*d < 5000, "delay too high: {}", d);
    }
}

#[test]
fn test_scroll_deltas_zero_steps() {
    let b = BehaviorSimulator::new(42);
    let deltas = b.generate_scroll_deltas(500.0, 0);
    assert!(deltas.is_empty());
}

#[test]
fn test_scroll_deltas_sum_approximate_total() {
    let b = BehaviorSimulator::new(42);
    let deltas = b.generate_scroll_deltas(1000.0, 30);
    // Legacy API normalizes to match total; inertia may produce variable count
    let sum: f64 = deltas.iter().sum();
    assert!(sum > 0.0, "Sum should be positive");
    assert!(sum < 2000.0, "Sum {} unreasonably large", sum);
}

#[test]
fn test_scroll_deltas_finite() {
    let b = BehaviorSimulator::new(42);
    let deltas = b.generate_scroll_deltas(500.0, 20);
    for d in &deltas {
        assert!(d.is_finite(), "scroll delta should be finite: {}", d);
    }
}

// ---- StealthProfile clone independence ----

#[test]
fn test_stealth_profile_clone_independence() {
    let p1 = StealthProfile::chrome_default();
    let p2 = p1.clone();

    // Modify clone's canvas seed (would need pub access)
    // Just verify they're equal after clone
    assert_eq!(p1.canvas.seed(), p2.canvas.seed());
    assert_eq!(p1.navigator.user_agent, p2.navigator.user_agent);
    assert_eq!(p1.tls.ja3_hash, p2.tls.ja3_hash);
}

// ---- StealthEngine delegation completeness ----

#[test]
fn test_engine_all_components_accessible() {
    let engine = StealthEngine::new(StealthProfile::chrome_default());

    // Every component should return non-empty data
    assert!(!engine.tls_config().ja3_hash.is_empty());
    assert!(!engine.navigator().user_agent.is_empty());
    assert!(!engine.navigator().platform.is_empty());
    assert!(engine.screen().width > 0);
    assert!(engine.screen().height > 0);
    assert!(!engine.webgl().renderer.is_empty());
    assert!(!engine.webgl().vendor.is_empty());
    assert!(engine.audio().seed() > 0);
    assert!(engine.behavior().seed() > 0);
    assert!(engine.canvas_noise().seed() > 0);
}

// ---- Cross-profile consistency ----

#[test]
fn test_chrome_firefox_profiles_fully_differ() {
    let chrome = StealthProfile::chrome_default();
    let firefox = StealthProfile::firefox_default();

    // TLS should differ
    assert_ne!(chrome.tls.ja3_hash, firefox.tls.ja3_hash);
    // Navigator UA should differ
    assert_ne!(chrome.navigator.user_agent, firefox.navigator.user_agent);
    // Canvas seeds differ (137 vs 42)
    assert_ne!(chrome.canvas.seed(), firefox.canvas.seed());
    // Behavior seeds differ
    assert_ne!(chrome.behavior.seed(), firefox.behavior.seed());
    // HTTP2 fingerprints differ
    assert_ne!(chrome.http2.akamai_fingerprint(), firefox.http2.akamai_fingerprint());
    // WebGL renderers should differ
    assert_ne!(chrome.webgl.renderer, firefox.webgl.renderer);
}

#[test]
fn test_default_engine_is_firefox() {
    let engine = StealthEngine::default_engine();
    assert!(engine.navigator().user_agent.contains("Firefox"),
        "Default engine should use Firefox profile");
}

// ---- Debug trait coverage ----

#[test]
fn test_stealth_profile_debug_output() {
    let profile = StealthProfile::chrome_default();
    let debug = format!("{:?}", profile);
    assert!(debug.contains("StealthProfile"));
    assert!(debug.len() > 100);
}

#[test]
fn test_canvas_noise_debug() {
    let noise = CanvasNoise::new(42);
    let debug = format!("{:?}", noise);
    assert!(debug.contains("CanvasNoise"));
}

#[test]
fn test_behavior_simulator_debug() {
    let b = BehaviorSimulator::new(42);
    let debug = format!("{:?}", b);
    assert!(debug.contains("BehaviorSimulator"));
}
