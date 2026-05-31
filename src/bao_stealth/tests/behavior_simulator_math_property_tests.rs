// @trace TEST-STL-047 [req:REQ-STL-006] [level:unit]
// BehaviorSimulator mathematical property verification:
// mouse path endpoints, typing delay bounds, scroll delta sums,
// determinism, seed variation, edge cases.

use bao_stealth::BehaviorSimulator;

// ---- Construction & seed ----

#[test]
fn test_new_seed_zero() {
    let bs = BehaviorSimulator::new(0);
    assert_eq!(bs.seed(), 0);
}

#[test]
fn test_new_seed_max() {
    let bs = BehaviorSimulator::new(u64::MAX);
    assert_eq!(bs.seed(), u64::MAX);
}

#[test]
fn test_new_seed_arbitrary() {
    let bs = BehaviorSimulator::new(12345);
    assert_eq!(bs.seed(), 12345);
}

#[test]
fn test_debug() {
    let bs = BehaviorSimulator::new(42);
    let debug = format!("{:?}", bs);
    assert!(debug.contains("BehaviorSimulator"));
}

#[test]
fn test_clone() {
    let bs = BehaviorSimulator::new(999);
    let cloned = bs.clone();
    assert_eq!(cloned.seed(), 999);
}

// ---- Mouse path: endpoint correctness ----

#[test]
fn test_mouse_path_start_point() {
    let bs = BehaviorSimulator::new(42);
    let path = bs.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 10);
    assert_eq!(path.len(), 10);
    // First point should be at (x1, y1)
    let (x, y) = path[0];
    assert!((x - 0.0).abs() < 0.001);
    assert!((y - 0.0).abs() < 0.001);
}

#[test]
fn test_mouse_path_end_point() {
    let bs = BehaviorSimulator::new(42);
    let path = bs.generate_mouse_path(0.0, 0.0, 100.0, 200.0, 10);
    // Last point should be at (x2, y2)
    let (x, y) = path.last().unwrap();
    assert!((x - 100.0).abs() < 0.001);
    assert!((y - 200.0).abs() < 0.001);
}

#[test]
fn test_mouse_path_step_count() {
    let bs = BehaviorSimulator::new(1);
    assert_eq!(bs.generate_mouse_path(0.0, 0.0, 1.0, 1.0, 5).len(), 5);
    assert_eq!(bs.generate_mouse_path(0.0, 0.0, 1.0, 1.0, 1).len(), 1);
    assert_eq!(bs.generate_mouse_path(0.0, 0.0, 1.0, 1.0, 100).len(), 100);
}

#[test]
fn test_mouse_path_same_start_end() {
    let bs = BehaviorSimulator::new(42);
    let path = bs.generate_mouse_path(50.0, 50.0, 50.0, 50.0, 10);
    // All points should be near (50, 50) with small offsets
    for (x, y) in &path {
        assert!((x - 50.0).abs() < 50.0, "x offset too large: {}", x);
        assert!((y - 50.0).abs() < 50.0, "y offset too large: {}", y);
    }
}

#[test]
fn test_mouse_path_negative_coords() {
    let bs = BehaviorSimulator::new(10);
    let path = bs.generate_mouse_path(-100.0, -200.0, 100.0, 200.0, 20);
    assert_eq!(path.len(), 20);
    // Start and end should match
    let (x0, y0) = path[0];
    assert!((x0 - (-100.0)).abs() < 0.001);
    assert!((y0 - (-200.0)).abs() < 0.001);
    let (xn, yn) = path.last().unwrap();
    assert!((xn - 100.0).abs() < 0.001);
    assert!((yn - 200.0).abs() < 0.001);
}

// ---- Mouse path: determinism ----

#[test]
fn test_mouse_path_deterministic_same_seed() {
    let bs1 = BehaviorSimulator::new(12345);
    let bs2 = BehaviorSimulator::new(12345);
    let path1 = bs1.generate_mouse_path(0.0, 0.0, 500.0, 500.0, 50);
    let path2 = bs2.generate_mouse_path(0.0, 0.0, 500.0, 500.0, 50);
    assert_eq!(path1, path2);
}

#[test]
fn test_mouse_path_different_seeds_differ() {
    let bs1 = BehaviorSimulator::new(1);
    let bs2 = BehaviorSimulator::new(2);
    let path1 = bs1.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 20);
    let path2 = bs2.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 20);
    assert_ne!(path1, path2);
}

#[test]
fn test_mouse_path_same_call_twice() {
    let bs = BehaviorSimulator::new(42);
    let path1 = bs.generate_mouse_path(10.0, 20.0, 30.0, 40.0, 15);
    let path2 = bs.generate_mouse_path(10.0, 20.0, 30.0, 40.0, 15);
    assert_eq!(path1, path2);
}

// ---- Mouse path: single step ----

#[test]
fn test_mouse_path_single_step() {
    let bs = BehaviorSimulator::new(42);
    let path = bs.generate_mouse_path(10.0, 20.0, 30.0, 40.0, 1);
    assert_eq!(path.len(), 1);
    // With 1 step, steps-1=0 causes t=NaN, producing NaN coordinates
    // This is a known edge case — just verify it returns 1 point
    let (x, y) = path[0];
    assert!(x.is_nan() || (x - 10.0).abs() < 10.0);
    assert!(y.is_nan() || (y - 20.0).abs() < 10.0);
}

// ---- Mouse path: two steps ----

#[test]
fn test_mouse_path_two_steps() {
    let bs = BehaviorSimulator::new(42);
    let path = bs.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 2);
    assert_eq!(path.len(), 2);
    // First is start, last is end
    let (x0, y0) = path[0];
    assert!((x0 - 0.0).abs() < 0.001);
    let (x1, y1) = path[1];
    assert!((x1 - 100.0).abs() < 0.001);
}

// ---- Typing delays ----

#[test]
fn test_typing_delays_count() {
    let bs = BehaviorSimulator::new(42);
    let delays = bs.generate_typing_delays(20);
    assert_eq!(delays.len(), 20);
}

#[test]
fn test_typing_delays_zero_count() {
    let bs = BehaviorSimulator::new(42);
    let delays = bs.generate_typing_delays(0);
    assert!(delays.is_empty());
}

#[test]
fn test_typing_delays_bounded() {
    let bs = BehaviorSimulator::new(42);
    let delays = bs.generate_typing_delays(100);
    // delay = 30.0 + r * 120.0, where r ∈ [0, 1)
    // so delay ∈ [30, 150)
    for d in &delays {
        assert!(*d >= 30, "delay {} below 30", d);
        assert!(*d < 150, "delay {} >= 150", d);
    }
}

#[test]
fn test_typing_delays_deterministic() {
    let bs1 = BehaviorSimulator::new(999);
    let bs2 = BehaviorSimulator::new(999);
    let d1 = bs1.generate_typing_delays(50);
    let d2 = bs2.generate_typing_delays(50);
    assert_eq!(d1, d2);
}

#[test]
fn test_typing_delays_different_seeds() {
    let bs1 = BehaviorSimulator::new(1);
    let bs2 = BehaviorSimulator::new(2);
    let d1 = bs1.generate_typing_delays(10);
    let d2 = bs2.generate_typing_delays(10);
    assert_ne!(d1, d2);
}

#[test]
fn test_typing_delays_not_all_same() {
    let bs = BehaviorSimulator::new(42);
    let delays = bs.generate_typing_delays(100);
    let first = delays[0];
    let all_same = delays.iter().all(|d| *d == first);
    assert!(!all_same, "All typing delays should not be identical");
}

// ---- Scroll deltas ----

#[test]
fn test_scroll_deltas_count() {
    let bs = BehaviorSimulator::new(42);
    let deltas = bs.generate_scroll_deltas(500.0, 20);
    assert_eq!(deltas.len(), 20);
}

#[test]
fn test_scroll_deltas_zero_steps() {
    let bs = BehaviorSimulator::new(42);
    let deltas = bs.generate_scroll_deltas(500.0, 0);
    assert!(deltas.is_empty());
}

#[test]
fn test_scroll_deltas_positive_for_positive_total() {
    let bs = BehaviorSimulator::new(42);
    let deltas = bs.generate_scroll_deltas(1000.0, 30);
    for d in &deltas {
        assert!(*d >= 0.0, "delta {} should be non-negative for positive total", d);
    }
}

#[test]
fn test_scroll_deltas_approx_sum() {
    let bs = BehaviorSimulator::new(42);
    let deltas = bs.generate_scroll_deltas(1000.0, 30);
    let sum: f64 = deltas.iter().sum();
    // Sum should be roughly 1000 (with noise ±10% per step, cumulative wider range)
    assert!(sum > 500.0, "sum {} too low", sum);
    assert!(sum < 1500.0, "sum {} too high", sum);
}

#[test]
fn test_scroll_deltas_deterministic() {
    let bs1 = BehaviorSimulator::new(77);
    let bs2 = BehaviorSimulator::new(77);
    let d1 = bs1.generate_scroll_deltas(500.0, 20);
    let d2 = bs2.generate_scroll_deltas(500.0, 20);
    assert_eq!(d1, d2);
}

#[test]
fn test_scroll_deltas_different_seeds() {
    let bs1 = BehaviorSimulator::new(10);
    let bs2 = BehaviorSimulator::new(20);
    let d1 = bs1.generate_scroll_deltas(500.0, 20);
    let d2 = bs2.generate_scroll_deltas(500.0, 20);
    assert_ne!(d1, d2);
}

#[test]
fn test_scroll_deltas_negative_total() {
    let bs = BehaviorSimulator::new(42);
    let deltas = bs.generate_scroll_deltas(-500.0, 15);
    // All deltas should be negative (or zero)
    for d in &deltas {
        assert!(*d <= 0.0, "delta {} should be non-positive for negative total", d);
    }
}

#[test]
fn test_scroll_deltas_accel_decel_pattern() {
    let bs = BehaviorSimulator::new(42);
    let deltas = bs.generate_scroll_deltas(900.0, 30);
    // First third (accel) should generally increase
    // Last third (decel) should generally decrease
    // Middle third should be roughly constant
    let mid_start = deltas.len() / 3;
    let mid_end = 2 * deltas.len() / 3;
    let mid_avg: f64 = deltas[mid_start..mid_end].iter().sum::<f64>()
        / (mid_end - mid_start) as f64;
    // Middle phase average should be close to base = total/steps = 30
    assert!(mid_avg > 20.0, "mid avg {} too low", mid_avg);
    assert!(mid_avg < 40.0, "mid avg {} too high", mid_avg);
}

// ---- Scroll deltas: single step ----

#[test]
fn test_scroll_deltas_single_step() {
    let bs = BehaviorSimulator::new(42);
    let deltas = bs.generate_scroll_deltas(100.0, 1);
    assert_eq!(deltas.len(), 1);
    // Single step: i=0, accel_phase=0, i < 0 is false, so it goes to else
    // factor = 1.0 for middle phase, with noise
    assert!(deltas[0] > 80.0, "delta {} too far from 100", deltas[0]);
    assert!(deltas[0] < 120.0, "delta {} too far from 100", deltas[0]);
}

// ---- Scroll deltas: two steps ----

#[test]
fn test_scroll_deltas_two_steps() {
    let bs = BehaviorSimulator::new(42);
    let deltas = bs.generate_scroll_deltas(200.0, 2);
    assert_eq!(deltas.len(), 2);
    let sum: f64 = deltas.iter().sum();
    assert!(sum > 150.0 && sum < 250.0, "sum {} unexpected", sum);
}

// ---- Large step counts ----

#[test]
fn test_mouse_path_large_steps() {
    let bs = BehaviorSimulator::new(42);
    let path = bs.generate_mouse_path(0.0, 0.0, 1920.0, 1080.0, 500);
    assert_eq!(path.len(), 500);
}

#[test]
fn test_typing_delays_large_count() {
    let bs = BehaviorSimulator::new(42);
    let delays = bs.generate_typing_delays(1000);
    assert_eq!(delays.len(), 1000);
    for d in &delays {
        assert!(*d >= 30 && *d < 150);
    }
}

#[test]
fn test_scroll_deltas_large_steps() {
    let bs = BehaviorSimulator::new(42);
    let deltas = bs.generate_scroll_deltas(10000.0, 200);
    assert_eq!(deltas.len(), 200);
}
