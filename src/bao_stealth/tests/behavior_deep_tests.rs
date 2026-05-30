// @trace TEST-STL-014 [req:REQ-STL-006] [level:unit]
// BehaviorSimulator deep tests: deterministic random, mouse path geometry,
// typing delay ranges, scroll delta physics, edge cases, clone, seed stability.

use bao_stealth::BehaviorSimulator;

// ---- Construction and seed ----

#[test]
fn test_new_preserves_seed() {
    let sim = BehaviorSimulator::new(12345);
    assert_eq!(sim.seed(), 12345);
}

#[test]
fn test_new_zero_seed() {
    let sim = BehaviorSimulator::new(0);
    assert_eq!(sim.seed(), 0);
}

#[test]
fn test_new_max_seed() {
    let sim = BehaviorSimulator::new(u64::MAX);
    assert_eq!(sim.seed(), u64::MAX);
}

#[test]
fn test_clone_preserves_seed() {
    let sim = BehaviorSimulator::new(999);
    let cloned = sim.clone();
    assert_eq!(cloned.seed(), 999);
}

// ---- generate_mouse_path: basic geometry ----

#[test]
fn test_mouse_path_correct_length() {
    let sim = BehaviorSimulator::new(42);
    let path = sim.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 10);
    assert_eq!(path.len(), 10);
}

#[test]
fn test_mouse_path_start_and_end_exact() {
    let sim = BehaviorSimulator::new(42);
    let path = sim.generate_mouse_path(10.0, 20.0, 110.0, 120.0, 20);
    // First point should be exact start (offset is 0 for i=0)
    assert_eq!(path[0].0, 10.0);
    assert_eq!(path[0].1, 20.0);
    // Last point: i == steps-1, offset is 0, base = end
    let last = path.last().unwrap();
    // Last point: base_x = x2, offset_x = 0, so cx = x2
    assert_eq!(last.0, 110.0);
    assert_eq!(last.1, 120.0);
}

#[test]
fn test_mouse_path_single_step() {
    let sim = BehaviorSimulator::new(42);
    let path = sim.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 1);
    assert_eq!(path.len(), 1);
    // t = 0/(0) = NaN — but steps=1 means t = 0/0
    // Actually steps=1: i=0, t = 0/0 = NaN. This is a corner case.
    // The code computes t = i as f64 / (steps - 1) as f64 = 0/0 = NaN
    // NaN propagates. Let's verify it doesn't panic.
}

#[test]
fn test_mouse_path_two_steps() {
    let sim = BehaviorSimulator::new(42);
    let path = sim.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 2);
    assert_eq!(path.len(), 2);
    // First point: start
    assert_eq!(path[0].0, 0.0);
    assert_eq!(path[0].1, 0.0);
    // Last point: end
    assert_eq!(path[1].0, 100.0);
    assert_eq!(path[1].1, 100.0);
}

#[test]
fn test_mouse_path_many_steps() {
    let sim = BehaviorSimulator::new(42);
    let path = sim.generate_mouse_path(0.0, 0.0, 1000.0, 500.0, 100);
    assert_eq!(path.len(), 100);
}

#[test]
fn test_mouse_path_deterministic_with_same_seed() {
    let sim1 = BehaviorSimulator::new(42);
    let sim2 = BehaviorSimulator::new(42);
    let path1 = sim1.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 20);
    let path2 = sim2.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 20);
    assert_eq!(path1, path2);
}

#[test]
fn test_mouse_path_different_seeds_differ() {
    let sim1 = BehaviorSimulator::new(1);
    let sim2 = BehaviorSimulator::new(2);
    let path1 = sim1.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 20);
    let path2 = sim2.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 20);
    assert_ne!(path1, path2);
}

#[test]
fn test_mouse_path_same_start_and_end() {
    let sim = BehaviorSimulator::new(42);
    let path = sim.generate_mouse_path(50.0, 50.0, 50.0, 50.0, 10);
    // Start and end are the same
    assert_eq!(path[0].0, 50.0);
    assert_eq!(path[0].1, 50.0);
    // Last point = end = (50, 50)
    let last = path.last().unwrap();
    assert_eq!(last.0, 50.0);
    assert_eq!(last.1, 50.0);
}

#[test]
fn test_mouse_path_negative_coords() {
    let sim = BehaviorSimulator::new(42);
    let path = sim.generate_mouse_path(-100.0, -200.0, 100.0, 200.0, 10);
    assert_eq!(path.len(), 10);
    assert_eq!(path[0].0, -100.0);
    assert_eq!(path[0].1, -200.0);
}

#[test]
fn test_mouse_path_large_coords() {
    let sim = BehaviorSimulator::new(42);
    let path = sim.generate_mouse_path(0.0, 0.0, 100000.0, 100000.0, 50);
    assert_eq!(path.len(), 50);
}

// ---- generate_mouse_path: intermediate points have noise ----

#[test]
fn test_mouse_path_intermediate_points_vary() {
    let sim = BehaviorSimulator::new(12345);
    let path = sim.generate_mouse_path(0.0, 0.0, 200.0, 200.0, 50);
    // With enough steps, intermediate points should not be on the straight line
    let mut off_line_count = 0;
    for i in 1..path.len() - 1 {
        let t = i as f64 / (path.len() - 1) as f64;
        let expected_x = 200.0 * t;
        let expected_y = 200.0 * t;
        if (path[i].0 - expected_x).abs() > 0.1 || (path[i].1 - expected_y).abs() > 0.1 {
            off_line_count += 1;
        }
    }
    assert!(off_line_count > 0, "At least some intermediate points should deviate from straight line");
}

// ---- generate_typing_delays ----

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
fn test_typing_delays_range() {
    // Delays should be in range [30, 150] (30 + [0, 120])
    let sim = BehaviorSimulator::new(42);
    let delays = sim.generate_typing_delays(100);
    for &d in &delays {
        assert!(d >= 30, "Delay {} should be >= 30", d);
        assert!(d <= 150, "Delay {} should be <= 150", d);
    }
}

#[test]
fn test_typing_delays_deterministic() {
    let sim1 = BehaviorSimulator::new(42);
    let sim2 = BehaviorSimulator::new(42);
    let d1 = sim1.generate_typing_delays(20);
    let d2 = sim2.generate_typing_delays(20);
    assert_eq!(d1, d2);
}

#[test]
fn test_typing_delays_different_seeds() {
    let sim1 = BehaviorSimulator::new(1);
    let sim2 = BehaviorSimulator::new(99999);
    let d1 = sim1.generate_typing_delays(20);
    let d2 = sim2.generate_typing_delays(20);
    assert_ne!(d1, d2);
}

#[test]
fn test_typing_delays_large_count() {
    let sim = BehaviorSimulator::new(42);
    let delays = sim.generate_typing_delays(1000);
    assert_eq!(delays.len(), 1000);
}

// ---- generate_scroll_deltas ----

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
fn test_scroll_deltas_deterministic() {
    let sim1 = BehaviorSimulator::new(42);
    let sim2 = BehaviorSimulator::new(42);
    let d1 = sim1.generate_scroll_deltas(500.0, 20);
    let d2 = sim2.generate_scroll_deltas(500.0, 20);
    assert_eq!(d1, d2);
}

#[test]
fn test_scroll_deltas_different_seeds() {
    let sim1 = BehaviorSimulator::new(1);
    let sim2 = BehaviorSimulator::new(2);
    let d1 = sim1.generate_scroll_deltas(500.0, 20);
    let d2 = sim2.generate_scroll_deltas(500.0, 20);
    assert_ne!(d1, d2);
}

#[test]
fn test_scroll_deltas_positive_with_positive_total() {
    let sim = BehaviorSimulator::new(42);
    let deltas = sim.generate_scroll_deltas(1000.0, 30);
    // Sum should be approximately positive (noise can make individual deltas near-zero)
    let sum: f64 = deltas.iter().sum();
    assert!(sum > 0.0, "Sum {} should be positive with positive total", sum);
    // Majority should be positive
    let pos_count = deltas.iter().filter(|&&d| d > 0.0).count();
    assert!(pos_count > deltas.len() / 2, "Most deltas should be positive");
}

#[test]
fn test_scroll_deltas_negative_with_negative_total() {
    let sim = BehaviorSimulator::new(42);
    let deltas = sim.generate_scroll_deltas(-1000.0, 30);
    let sum: f64 = deltas.iter().sum();
    assert!(sum < 0.0, "Sum {} should be negative with negative total", sum);
    let neg_count = deltas.iter().filter(|&&d| d < 0.0).count();
    assert!(neg_count > deltas.len() / 2, "Most deltas should be negative");
}

#[test]
fn test_scroll_deltas_accel_decel_phases() {
    // With 30 steps: accel_phase = 10, decel_phase = 10
    // First 10 should generally be smaller than middle
    let sim = BehaviorSimulator::new(42);
    let deltas = sim.generate_scroll_deltas(3000.0, 30);
    let accel_avg: f64 = deltas[..10].iter().sum::<f64>() / 10.0;
    let mid_avg: f64 = deltas[10..20].iter().sum::<f64>() / 10.0;
    // Mid-phase should generally be larger than accel phase
    assert!(mid_avg > accel_avg, "Mid avg {} should be > accel avg {}", mid_avg, accel_avg);
}

#[test]
fn test_scroll_deltas_single_step() {
    let sim = BehaviorSimulator::new(42);
    let deltas = sim.generate_scroll_deltas(500.0, 1);
    assert_eq!(deltas.len(), 1);
}

#[test]
fn test_scroll_deltas_two_steps() {
    let sim = BehaviorSimulator::new(42);
    let deltas = sim.generate_scroll_deltas(500.0, 2);
    assert_eq!(deltas.len(), 2);
}

#[test]
fn test_scroll_deltas_large_steps() {
    let sim = BehaviorSimulator::new(42);
    let deltas = sim.generate_scroll_deltas(10000.0, 200);
    assert_eq!(deltas.len(), 200);
}

#[test]
fn test_scroll_deltas_small_total() {
    let sim = BehaviorSimulator::new(42);
    let deltas = sim.generate_scroll_deltas(1.0, 10);
    assert_eq!(deltas.len(), 10);
    // All should be small
    for &d in &deltas {
        assert!(d.abs() < 1.0, "Small total should produce small deltas");
    }
}

#[test]
fn test_scroll_deltas_zero_total() {
    let sim = BehaviorSimulator::new(42);
    let deltas = sim.generate_scroll_deltas(0.0, 10);
    assert_eq!(deltas.len(), 10);
    // All zeros with noise only
    for &d in &deltas {
        assert!(d.abs() < 1.0, "Zero total should produce near-zero deltas");
    }
}

// ---- Debug trait ----

#[test]
fn test_behavior_simulator_debug() {
    let sim = BehaviorSimulator::new(42);
    let debug = format!("{:?}", sim);
    assert!(debug.contains("BehaviorSimulator"));
}

// ---- Seed stability across calls ----

#[test]
fn test_seed_not_mutated_by_calls() {
    let sim = BehaviorSimulator::new(12345);
    let _ = sim.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 10);
    let _ = sim.generate_typing_delays(5);
    let _ = sim.generate_scroll_deltas(500.0, 10);
    // Seed should remain the same (methods take &self, use local rng)
    assert_eq!(sim.seed(), 12345);
}

#[test]
fn test_repeated_calls_produce_same_results() {
    let sim = BehaviorSimulator::new(77);
    let path1 = sim.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 15);
    let path2 = sim.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 15);
    assert_eq!(path1, path2, "Same seed should produce same mouse path each time");
}

#[test]
fn test_typing_delays_same_each_call() {
    let sim = BehaviorSimulator::new(77);
    let d1 = sim.generate_typing_delays(20);
    let d2 = sim.generate_typing_delays(20);
    assert_eq!(d1, d2, "Same seed should produce same typing delays each time");
}

#[test]
fn test_scroll_deltas_same_each_call() {
    let sim = BehaviorSimulator::new(77);
    let d1 = sim.generate_scroll_deltas(500.0, 20);
    let d2 = sim.generate_scroll_deltas(500.0, 20);
    assert_eq!(d1, d2, "Same seed should produce same scroll deltas each time");
}
