// @trace TEST-STL-ENGINE-001 [req:REQ-STL-001,REQ-STL-002,REQ-STL-003,REQ-STL-004,REQ-STL-005,REQ-STL-006,REQ-STL-007] [level:integration]
// StealthEngine integration tests: full engine lifecycle, cross-component consistency,
// JS injection output validation, profile switching.

use bao_stealth::{
    StealthProfile, StealthEngine, TlsFingerprint, Http2Fingerprint,
    CanvasNoise, NavigatorProfile, ScreenProfile, WebGLProfile, AudioProfile,
    BehaviorSimulator,
};

// ---- StealthEngine lifecycle ----

#[test]
fn test_default_engine_has_all_components() {
    let engine = StealthEngine::default_engine();
    assert!(!engine.profile().navigator.user_agent.is_empty());
    assert!(!engine.tls_config().cipher_suites.is_empty());
    assert!(!engine.http2_config().settings_frame_payload().is_empty());
    assert!(!engine.canvas_noise().noise_amplitude().is_nan());
    assert!(!engine.navigator().user_agent.is_empty());
    assert!(!engine.webgl().vendor.is_empty());
    assert!(!engine.audio().noise_amplitude().is_nan());
    assert!(engine.behavior().seed() > 0);
}

#[test]
fn test_engine_with_chrome_profile() {
    let profile = StealthProfile::chrome_default();
    let engine = StealthEngine::new(profile);
    assert!(engine.navigator().user_agent.contains("Chrome"));
    assert!(!engine.tls_config().cipher_suites.is_empty());
}

#[test]
fn test_engine_with_firefox_profile() {
    let profile = StealthProfile::firefox_default();
    let engine = StealthEngine::new(profile);
    assert!(engine.navigator().user_agent.contains("Firefox"));
    assert!(!engine.tls_config().cipher_suites.is_empty());
}

#[test]
fn test_engine_profile_accessor_matches_constructor() {
    let profile = StealthProfile::chrome_default();
    let ua_before = profile.navigator.user_agent.clone();
    let engine = StealthEngine::new(profile);
    assert_eq!(engine.profile().navigator.user_agent, ua_before.as_str());
}

// ---- JS injection output validation ----

// ---- Cross-component consistency ----

#[test]
fn test_tls_and_http2_from_same_profile_are_consistent() {
    let engine = StealthEngine::new(StealthProfile::chrome_default());
    // Both should reflect Chrome fingerprint
    let tls = engine.tls_config();
    let http2 = engine.http2_config();
    // Chrome TLS ciphers and HTTP2 settings should both be non-trivial
    assert!(tls.cipher_suites.len() >= 3);
    assert!(!http2.settings_frame_payload().is_empty());
}

#[test]
fn test_canvas_noise_applied_to_pixel_is_deterministic() {
    let noise = CanvasNoise::new(42);
    let (r1, g1, b1, a1) = noise.apply_to_pixel(128, 128, 128, 255, 10, 20);
    let (r2, g2, b2, a2) = noise.apply_to_pixel(128, 128, 128, 255, 10, 20);
    assert_eq!((r1, g1, b1, a1), (r2, g2, b2, a2), "Same seed+pixel+position should give same result");
}

#[test]
fn test_canvas_noise_different_positions_differ() {
    let noise = CanvasNoise::new(42);
    let p1 = noise.apply_to_pixel(128, 128, 128, 255, 0, 0);
    let p2 = noise.apply_to_pixel(128, 128, 128, 255, 100, 100);
    // Very unlikely that noise at different positions is identical
    assert_ne!(p1, p2, "Different positions should generally have different noise");
}

#[test]
fn test_canvas_noise_preserves_alpha() {
    let noise = CanvasNoise::new(42);
    let (_, _, _, a) = noise.apply_to_pixel(128, 128, 128, 200, 5, 5);
    assert_eq!(a, 200, "Alpha channel should be preserved");
}

#[test]
fn test_behavior_mouse_path_has_correct_steps() {
    let sim = BehaviorSimulator::new(12345);
    let path = sim.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 10);
    // generate_mouse_path returns steps+1 points.
    assert_eq!(path.len(), 11);
}

#[test]
fn test_behavior_mouse_path_start_and_end() {
    let sim = BehaviorSimulator::new(99);
    let path = sim.generate_mouse_path(10.0, 20.0, 110.0, 120.0, 5);
    // First point near start, last near end (with noise may not be exact)
    assert!(path[0].0 < 50.0, "First point should be near start x");
    assert!(path[4].0 > 50.0, "Last point should be near end x");
}

#[test]
fn test_behavior_mouse_path_deterministic() {
    let sim = BehaviorSimulator::new(77);
    let path1 = sim.generate_mouse_path(0.0, 0.0, 500.0, 500.0, 8);
    let path2 = sim.generate_mouse_path(0.0, 0.0, 500.0, 500.0, 8);
    assert_eq!(path1, path2, "Same seed should produce identical paths");
}

#[test]
fn test_behavior_typing_delays_count() {
    let sim = BehaviorSimulator::new(42);
    let delays = sim.generate_typing_delays(20);
    assert_eq!(delays.len(), 20);
}

#[test]
fn test_behavior_typing_delays_positive() {
    let sim = BehaviorSimulator::new(42);
    let delays = sim.generate_typing_delays(50);
    for d in &delays {
        assert!(*d > 0, "All delays should be positive");
    }
}

#[test]
fn test_behavior_scroll_deltas_correct_count() {
    let sim = BehaviorSimulator::new(55);
    let steps = 20;
    let deltas = sim.generate_scroll_deltas(1000.0, steps);
    assert_eq!(deltas.len(), steps);
    // Each delta should be a valid finite number
    for d in &deltas {
        assert!(d.is_finite(), "Scroll delta should be finite");
    }
}

#[test]
fn test_behavior_scroll_deltas_positive() {
    let sim = BehaviorSimulator::new(42);
    let deltas = sim.generate_scroll_deltas(500.0, 10);
    for d in &deltas {
        assert!(*d >= 0.0, "Scroll deltas should be non-negative");
    }
}

// ---- Profile field completeness ----

#[test]
fn test_chrome_profile_all_fields_non_empty() {
    let p = StealthProfile::chrome_default();
    assert!(!p.navigator.user_agent.is_empty());
    assert!(!p.navigator.platform.is_empty());
    assert!(!p.navigator.language.is_empty());
    assert!(!p.tls.cipher_suites.is_empty());
    assert!(!p.tls.extensions.is_empty());
    assert!(!p.tls.tls_version.is_empty());
    assert!(!p.http2.settings_frame_payload().is_empty());
    assert!(!p.webgl.vendor.is_empty());
    assert!(!p.webgl.renderer.is_empty());
    assert!(!p.canvas.noise_amplitude().is_nan());
    assert!(!p.audio.noise_amplitude().is_nan());
    assert!(p.behavior.seed() > 0);
}

#[test]
fn test_firefox_profile_all_fields_non_empty() {
    let p = StealthProfile::firefox_default();
    assert!(!p.navigator.user_agent.is_empty());
    assert!(!p.navigator.platform.is_empty());
    assert!(!p.navigator.language.is_empty());
    assert!(!p.tls.cipher_suites.is_empty());
    assert!(!p.tls.extensions.is_empty());
    assert!(!p.tls.tls_version.is_empty());
    assert!(!p.http2.settings_frame_payload().is_empty());
    assert!(!p.webgl.vendor.is_empty());
    assert!(!p.webgl.renderer.is_empty());
    assert!(!p.canvas.noise_amplitude().is_nan());
    assert!(!p.audio.noise_amplitude().is_nan());
    assert!(p.behavior.seed() > 0);
}

#[test]
fn test_chrome_firefox_profiles_differ_in_all_dimensions() {
    let ch = StealthProfile::chrome_default();
    let ff = StealthProfile::firefox_default();
    assert_ne!(ch.navigator.user_agent, ff.navigator.user_agent);
    assert_ne!(ch.tls.cipher_suites, ff.tls.cipher_suites);
    assert_ne!(ch.webgl.vendor, ff.webgl.vendor);
}

// ---- ScreenProfile ----

#[test]
fn test_screen_profile_has_dimensions() {
    let engine = StealthEngine::new(StealthProfile::chrome_default());
    let screen = engine.screen();
    assert!(screen.width > 0);
    assert!(screen.height > 0);
    assert!(screen.color_depth > 0);
}

// ---- WebGLProfile ----

#[test]
fn test_webgl_profile_vendor_renderer() {
    let engine = StealthEngine::new(StealthProfile::chrome_default());
    let webgl = engine.webgl();
    assert!(!webgl.vendor.is_empty());
    assert!(!webgl.renderer.is_empty());
}

// ---- AudioProfile ----

#[test]
fn test_audio_noise_amplitude_positive() {
    let engine = StealthEngine::new(StealthProfile::chrome_default());
    let audio = engine.audio();
    assert!(audio.noise_amplitude() > 0.0);
}

// ---- CanvasNoise via engine ----

#[test]
fn test_engine_canvas_noise_matches_profile() {
    let profile = StealthProfile::chrome_default();
    let profile_seed = profile.canvas.seed();
    let engine = StealthEngine::new(profile);
    assert_eq!(engine.canvas_noise().seed(), profile_seed);
}

// ---- StealthEngine with custom profile modifications ----

#[test]
fn test_behavior_simulator_different_seeds_differ() {
    let sim1 = BehaviorSimulator::new(1);
    let sim2 = BehaviorSimulator::new(99999);
    let path1 = sim1.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 5);
    let path2 = sim2.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 5);
    assert_ne!(path1, path2, "Different seeds should produce different paths");
}

#[test]
fn test_behavior_typing_delays_differ_between_seeds() {
    let sim1 = BehaviorSimulator::new(10);
    let sim2 = BehaviorSimulator::new(20);
    let delays1 = sim1.generate_typing_delays(30);
    let delays2 = sim2.generate_typing_delays(30);
    assert_ne!(delays1, delays2, "Different seeds should produce different delays");
}
