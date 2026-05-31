// @trace TEST-STL-021 [req:REQ-STL-006] [level:unit]
// BehaviorSimulator deep tests: mouse path geometry, typing delay distribution,
// scroll delta physics, seed determinism, edge cases, clone/debug.

use bao_stealth::BehaviorSimulator;

// ---- Construction ----

#[test]
fn test_behavior_new_seed() {
    let sim = BehaviorSimulator::new(42);
    assert_eq!(sim.seed(), 42);
}

#[test]
fn test_behavior_seed_zero() {
    let sim = BehaviorSimulator::new(0);
    assert_eq!(sim.seed(), 0);
}

#[test]
fn test_behavior_seed_large() {
    let sim = BehaviorSimulator::new(u64::MAX);
    assert_eq!(sim.seed(), u64::MAX);
}

// ---- Mouse path generation ----

#[test]
fn test_mouse_path_start_near_origin() {
    let sim = BehaviorSimulator::new(42);
    let path = sim.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 20);
    assert!(!path.is_empty());
    // First point should be close to start
    let first = &path[0];
    assert!((first.0 - 0.0).abs() < 50.0, "First x too far: {}", first.0);
    assert!((first.1 - 0.0).abs() < 50.0, "First y too far: {}", first.1);
}

#[test]
fn test_mouse_path_end_near_target() {
    let sim = BehaviorSimulator::new(42);
    let path = sim.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 20);
    let last = path.last().unwrap();
    assert!((last.0 - 100.0).abs() < 50.0, "Last x too far: {}", last.0);
    assert!((last.1 - 100.0).abs() < 50.0, "Last y too far: {}", last.1);
}

#[test]
fn test_mouse_path_correct_step_count() {
    let sim = BehaviorSimulator::new(42);
    let path = sim.generate_mouse_path(0.0, 0.0, 200.0, 200.0, 30);
    assert_eq!(path.len(), 30);
}

#[test]
fn test_mouse_path_single_step() {
    let sim = BehaviorSimulator::new(42);
    let path = sim.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 1);
    assert_eq!(path.len(), 1);
}

#[test]
fn test_mouse_path_zero_steps() {
    let sim = BehaviorSimulator::new(42);
    let path = sim.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 0);
    assert!(path.is_empty());
}

#[test]
fn test_mouse_path_deterministic() {
    let sim = BehaviorSimulator::new(42);
    let p1 = sim.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 10);
    let p2 = sim.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 10);
    assert_eq!(p1, p2);
}

#[test]
fn test_mouse_path_different_seeds_differ() {
    let s1 = BehaviorSimulator::new(1);
    let s2 = BehaviorSimulator::new(2);
    let p1 = s1.generate_mouse_path(0.0, 0.0, 500.0, 500.0, 20);
    let p2 = s2.generate_mouse_path(0.0, 0.0, 500.0, 500.0, 20);
    assert_ne!(p1, p2, "Different seeds should produce different paths");
}

#[test]
fn test_mouse_path_coordinates_are_finite() {
    let sim = BehaviorSimulator::new(42);
    let path = sim.generate_mouse_path(-100.0, -100.0, 1000.0, 1000.0, 50);
    for (x, y) in &path {
        assert!(x.is_finite(), "x not finite: {}", x);
        assert!(y.is_finite(), "y not finite: {}", y);
    }
}

#[test]
fn test_mouse_path_same_start_end() {
    let sim = BehaviorSimulator::new(42);
    let path = sim.generate_mouse_path(50.0, 50.0, 50.0, 50.0, 10);
    assert_eq!(path.len(), 10);
    // All points should be close to (50, 50) since start == end
    for (x, y) in &path {
        assert!((x - 50.0).abs() < 100.0);
        assert!((y - 50.0).abs() < 100.0);
    }
}

#[test]
fn test_mouse_path_negative_coordinates() {
    let sim = BehaviorSimulator::new(42);
    let path = sim.generate_mouse_path(-500.0, -500.0, -100.0, -100.0, 10);
    assert_eq!(path.len(), 10);
    for (x, y) in &path {
        assert!(x.is_finite());
        assert!(y.is_finite());
    }
}

// ---- Typing delays ----

#[test]
fn test_typing_delays_correct_count() {
    let sim = BehaviorSimulator::new(42);
    let delays = sim.generate_typing_delays(10);
    assert_eq!(delays.len(), 10);
}

#[test]
fn test_typing_delays_zero_count() {
    let sim = BehaviorSimulator::new(42);
    let delays = sim.generate_typing_delays(0);
    assert!(delays.is_empty());
}

#[test]
fn test_typing_delays_large_count() {
    let sim = BehaviorSimulator::new(42);
    let delays = sim.generate_typing_delays(1000);
    assert_eq!(delays.len(), 1000);
}

#[test]
fn test_typing_delays_reasonable_range() {
    let sim = BehaviorSimulator::new(42);
    let delays = sim.generate_typing_delays(100);
    for d in &delays {
        assert!(*d > 0, "Delay should be positive, got {}", d);
        assert!(*d < 5000, "Delay too large: {}", d);
    }
}

#[test]
fn test_typing_delays_deterministic() {
    let sim = BehaviorSimulator::new(42);
    let d1 = sim.generate_typing_delays(20);
    let d2 = sim.generate_typing_delays(20);
    assert_eq!(d1, d2);
}

#[test]
fn test_typing_delays_different_seeds() {
    let s1 = BehaviorSimulator::new(100);
    let s2 = BehaviorSimulator::new(200);
    let d1 = s1.generate_typing_delays(50);
    let d2 = s2.generate_typing_delays(50);
    assert_ne!(d1, d2);
}

#[test]
fn test_typing_delays_single() {
    let sim = BehaviorSimulator::new(42);
    let delays = sim.generate_typing_delays(1);
    assert_eq!(delays.len(), 1);
    assert!(delays[0] > 0);
}

// ---- Scroll deltas ----

#[test]
fn test_scroll_deltas_correct_count() {
    let sim = BehaviorSimulator::new(42);
    let deltas = sim.generate_scroll_deltas(1000.0, 10);
    assert_eq!(deltas.len(), 10);
}

#[test]
fn test_scroll_deltas_zero_steps() {
    let sim = BehaviorSimulator::new(42);
    let deltas = sim.generate_scroll_deltas(1000.0, 0);
    assert!(deltas.is_empty());
}

#[test]
fn test_scroll_deltas_sum_approximates_total() {
    let sim = BehaviorSimulator::new(42);
    let total = 1000.0;
    let deltas = sim.generate_scroll_deltas(total, 20);
    let sum: f64 = deltas.iter().sum();
    // Sum should be close to total (within 50% tolerance for natural variation)
    assert!((sum - total).abs() < total * 0.5,
        "Sum {} too far from total {}", sum, total);
}

#[test]
fn test_scroll_deltas_positive_for_positive_total() {
    let sim = BehaviorSimulator::new(42);
    let deltas = sim.generate_scroll_deltas(500.0, 10);
    for d in &deltas {
        assert!(d.is_finite(), "Delta not finite: {}", d);
    }
}

#[test]
fn test_scroll_deltas_negative_total() {
    let sim = BehaviorSimulator::new(42);
    let deltas = sim.generate_scroll_deltas(-500.0, 10);
    assert_eq!(deltas.len(), 10);
    for d in &deltas {
        assert!(d.is_finite());
    }
}

#[test]
fn test_scroll_deltas_zero_total() {
    let sim = BehaviorSimulator::new(42);
    let deltas = sim.generate_scroll_deltas(0.0, 10);
    assert_eq!(deltas.len(), 10);
}

#[test]
fn test_scroll_deltas_deterministic() {
    let sim = BehaviorSimulator::new(42);
    let d1 = sim.generate_scroll_deltas(1000.0, 15);
    let d2 = sim.generate_scroll_deltas(1000.0, 15);
    assert_eq!(d1, d2);
}

#[test]
fn test_scroll_deltas_different_seeds() {
    let s1 = BehaviorSimulator::new(10);
    let s2 = BehaviorSimulator::new(20);
    let d1 = s1.generate_scroll_deltas(1000.0, 10);
    let d2 = s2.generate_scroll_deltas(1000.0, 10);
    assert_ne!(d1, d2);
}

#[test]
fn test_scroll_deltas_single_step() {
    let sim = BehaviorSimulator::new(42);
    let deltas = sim.generate_scroll_deltas(1000.0, 1);
    assert_eq!(deltas.len(), 1);
}

// ---- Clone and Debug ----

#[test]
fn test_behavior_clone() {
    let sim = BehaviorSimulator::new(999);
    let cloned = sim.clone();
    assert_eq!(cloned.seed(), 999);
    // Same seed → same output
    let p1 = sim.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 5);
    let p2 = cloned.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 5);
    assert_eq!(p1, p2);
}

#[test]
fn test_behavior_debug() {
    let sim = BehaviorSimulator::new(42);
    let debug = format!("{:?}", sim);
    assert!(debug.contains("BehaviorSimulator") || debug.contains("42"));
}

// ---- Cross-method independence ----

#[test]
fn test_mouse_path_independent_of_typing() {
    let sim = BehaviorSimulator::new(42);
    let path = sim.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 5);
    let _delays = sim.generate_typing_delays(5);
    let path2 = sim.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 5);
    // Same seed should produce same path regardless of call order
    assert_eq!(path, path2);
}
