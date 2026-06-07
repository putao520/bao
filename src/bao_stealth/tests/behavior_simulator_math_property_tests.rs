// @trace TEST-STL-047 [req:REQ-STL-006] [level:unit]
// BehaviorSimulator mathematical property verification:
// mouse path endpoints, typing delay bounds, scroll delta sums,
// determinism, seed variation, edge cases.
// Updated for cubic Bezier + Fitts' Law + Box-Muller + inertia scroll.

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
    // Legacy API returns steps+1 via resampling
    assert_eq!(path.len(), 11);
    let (x, y) = path[0];
    assert!((x - 0.0).abs() < 1.0, "Start x should be near 0: {}", x);
    assert!((y - 0.0).abs() < 1.0, "Start y should be near 0: {}", y);
}

#[test]
fn test_mouse_path_end_point() {
    let bs = BehaviorSimulator::new(42);
    let path = bs.generate_mouse_path(0.0, 0.0, 100.0, 200.0, 10);
    let (x, y) = path.last().unwrap();
    assert!((x - 100.0).abs() < 1.0, "End x should be near 100: {}", x);
    assert!((y - 200.0).abs() < 1.0, "End y should be near 200: {}", y);
}

#[test]
fn test_mouse_path_step_count() {
    let bs = BehaviorSimulator::new(1);
    assert_eq!(bs.generate_mouse_path(0.0, 0.0, 1.0, 1.0, 5).len(), 6);
    assert_eq!(bs.generate_mouse_path(0.0, 0.0, 1.0, 1.0, 1).len(), 2);
    assert_eq!(bs.generate_mouse_path(0.0, 0.0, 1.0, 1.0, 100).len(), 101);
}

#[test]
fn test_mouse_path_same_start_end() {
    let bs = BehaviorSimulator::new(42);
    let path = bs.generate_mouse_path(50.0, 50.0, 50.0, 50.0, 10);
    assert!(!path.is_empty());
    for (x, y) in &path {
        assert!((x - 50.0).abs() < 100.0, "x offset too large: {}", x);
        assert!((y - 50.0).abs() < 100.0, "y offset too large: {}", y);
    }
}

#[test]
fn test_mouse_path_negative_coords() {
    let bs = BehaviorSimulator::new(10);
    let path = bs.generate_mouse_path(-100.0, -200.0, 100.0, 200.0, 20);
    assert_eq!(path.len(), 21);
    let (x0, y0) = path[0];
    assert!((x0 - (-100.0)).abs() < 1.0);
    assert!((y0 - (-200.0)).abs() < 1.0);
    let (xn, yn) = path.last().unwrap();
    assert!((xn - 100.0).abs() < 1.0);
    assert!((yn - 200.0).abs() < 1.0);
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

// ---- Mouse path: single/two steps ----

#[test]
fn test_mouse_path_single_step() {
    let bs = BehaviorSimulator::new(42);
    let path = bs.generate_mouse_path(10.0, 20.0, 30.0, 40.0, 1);
    assert_eq!(path.len(), 2);
    let (x, y) = path[0];
    assert!((x - 10.0).abs() < 10.0);
    assert!((y - 20.0).abs() < 10.0);
}

#[test]
fn test_mouse_path_two_steps() {
    let bs = BehaviorSimulator::new(42);
    let path = bs.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 2);
    assert_eq!(path.len(), 3);
    let (x0, _) = path[0];
    assert!((x0 - 0.0).abs() < 1.0);
    let (x1, _) = path.last().unwrap();
    assert!((x1 - 100.0).abs() < 1.0);
}

// ---- Typing delays ----

#[test]
fn test_typing_delays_count() {
    let bs = BehaviorSimulator::new(42);
    let delays = bs.generate_typing_delays(20);
    // May have extra backspace events from typo correction
    assert!(delays.len() >= 20, "Expected >= 20 delays, got {}", delays.len());
}

#[test]
fn test_typing_delays_zero_count() {
    let bs = BehaviorSimulator::new(42);
    let delays = bs.generate_typing_delays(0);
    assert!(delays.is_empty());
}

#[test]
fn test_typing_delays_all_positive() {
    let bs = BehaviorSimulator::new(42);
    let delays = bs.generate_typing_delays(100);
    for d in &delays {
        assert!(*d > 0, "delay {} should be positive", d);
    }
}

#[test]
fn test_typing_delays_reasonable_range() {
    // Human typing: base 85-95ms ± 25ms stddev + possible pauses
    let bs = BehaviorSimulator::new(42);
    let delays = bs.generate_typing_delays(100);
    for d in &delays {
        assert!(*d < 5000, "delay {} too large", d);
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
fn test_scroll_deltas_non_empty() {
    let bs = BehaviorSimulator::new(42);
    let deltas = bs.generate_scroll_deltas(500.0, 20);
    assert!(!deltas.is_empty());
}

#[test]
fn test_scroll_deltas_zero_steps() {
    let bs = BehaviorSimulator::new(42);
    let deltas = bs.generate_scroll_deltas(500.0, 0);
    assert!(deltas.is_empty());
}

#[test]
fn test_scroll_deltas_all_finite() {
    let bs = BehaviorSimulator::new(42);
    let deltas = bs.generate_scroll_deltas(1000.0, 30);
    for d in &deltas {
        assert!(d.is_finite(), "delta not finite: {}", d);
    }
}

#[test]
fn test_scroll_deltas_approx_sum() {
    let bs = BehaviorSimulator::new(42);
    let deltas = bs.generate_scroll_deltas(1000.0, 30);
    let sum: f64 = deltas.iter().sum();
    // Legacy API normalizes to match total
    assert!((sum - 1000.0).abs() < 100.0, "sum {} too far from 1000", sum);
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
    assert!(!deltas.is_empty());
    for d in &deltas {
        assert!(d.is_finite());
    }
}

// ---- Large step counts ----

#[test]
fn test_mouse_path_large_steps() {
    let bs = BehaviorSimulator::new(42);
    let path = bs.generate_mouse_path(0.0, 0.0, 1920.0, 1080.0, 500);
    assert_eq!(path.len(), 501);
}

#[test]
fn test_typing_delays_large_count() {
    let bs = BehaviorSimulator::new(42);
    let delays = bs.generate_typing_delays(1000);
    assert!(delays.len() >= 1000, "Expected >= 1000 delays, got {}", delays.len());
    for d in &delays {
        assert!(*d > 0 && *d < 5000, "delay {} out of range", d);
    }
}

#[test]
fn test_scroll_deltas_large_steps() {
    let bs = BehaviorSimulator::new(42);
    let deltas = bs.generate_scroll_deltas(10000.0, 200);
    assert!(!deltas.is_empty());
    let sum: f64 = deltas.iter().sum();
    assert!((sum - 10000.0).abs() < 1000.0, "sum {} too far from 10000", sum);
}
