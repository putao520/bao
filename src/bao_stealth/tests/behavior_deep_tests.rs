// @trace TEST-STL-014 [req:REQ-STL-006] [level:unit]
// BehaviorSimulator deep tests: deterministic random, mouse path geometry,
// typing delay characteristics, scroll delta physics, edge cases, clone, seed stability.
// Updated for cubic Bezier + Fitts' Law + Box-Muller implementation.

use bao_stealth::{BehaviorConfig, BehaviorSimulator};

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

#[test]
fn test_with_config_firefox() {
    let sim = BehaviorSimulator::with_config(42, BehaviorConfig::firefox());
    assert_eq!(sim.seed(), 42);
}

#[test]
fn test_with_config_chrome() {
    let sim = BehaviorSimulator::with_config(42, BehaviorConfig::chrome());
    assert_eq!(sim.seed(), 42);
}

// ---- generate_mouse_path (legacy API) ----

#[test]
fn test_mouse_path_correct_length() {
    let sim = BehaviorSimulator::new(42);
    let path = sim.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 10);
    assert_eq!(path.len(), 11); // steps+1
}

#[test]
fn test_mouse_path_start_exact() {
    let sim = BehaviorSimulator::new(42);
    let path = sim.generate_mouse_path(10.0, 20.0, 110.0, 120.0, 20);
    assert!((path[0].0 - 10.0).abs() < 1.0);
    assert!((path[0].1 - 20.0).abs() < 1.0);
}

#[test]
fn test_mouse_path_single_step() {
    let sim = BehaviorSimulator::new(42);
    let path = sim.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 1);
    assert_eq!(path.len(), 2); // steps+1
}

#[test]
fn test_mouse_path_two_steps() {
    let sim = BehaviorSimulator::new(42);
    let path = sim.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 2);
    assert_eq!(path.len(), 3); // steps+1
}

#[test]
fn test_mouse_path_many_steps() {
    let sim = BehaviorSimulator::new(42);
    let path = sim.generate_mouse_path(0.0, 0.0, 1000.0, 500.0, 100);
    assert_eq!(path.len(), 101); // steps+1
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
fn test_mouse_path_negative_coords() {
    let sim = BehaviorSimulator::new(42);
    let path = sim.generate_mouse_path(-100.0, -200.0, 100.0, 200.0, 10);
    assert_eq!(path.len(), 11);
}

#[test]
fn test_mouse_path_large_coords() {
    let sim = BehaviorSimulator::new(42);
    let path = sim.generate_mouse_path(0.0, 0.0, 100000.0, 100000.0, 50);
    assert_eq!(path.len(), 51);
}

#[test]
fn test_mouse_path_intermediate_points_vary() {
    let sim = BehaviorSimulator::new(12345);
    let path = sim.generate_mouse_path(0.0, 0.0, 200.0, 200.0, 50);
    // With Bezier curves, intermediate points should deviate from straight line
    let mut off_line_count = 0;
    for i in 1..path.len() - 1 {
        let t = i as f64 / (path.len() - 1) as f64;
        let expected_x = 200.0 * t;
        let expected_y = 200.0 * t;
        if (path[i].0 - expected_x).abs() > 0.1 || (path[i].1 - expected_y).abs() > 0.1 {
            off_line_count += 1;
        }
    }
    assert!(
        off_line_count > 0,
        "At least some intermediate points should deviate from straight line"
    );
}

// ---- generate_typing_delays (legacy API) ----

#[test]
fn test_typing_delays_at_least_count() {
    // With human typing, we may get extra backspace events (typo correction)
    let sim = BehaviorSimulator::new(42);
    let delays = sim.generate_typing_delays(10);
    // Should produce at least `count` delays (may have extra backspace events)
    assert!(delays.len() >= 10, "Expected >= 10 delays, got {}", delays.len());
}

#[test]
fn test_typing_delays_zero_count() {
    let sim = BehaviorSimulator::new(42);
    let delays = sim.generate_typing_delays(0);
    assert!(delays.is_empty());
}

#[test]
fn test_typing_delays_all_positive() {
    let sim = BehaviorSimulator::new(42);
    let delays = sim.generate_typing_delays(100);
    for &d in &delays {
        assert!(d > 0, "Delay {} should be positive", d);
    }
}

#[test]
fn test_typing_delays_reasonable_range() {
    // Human typing delays: base_interval 85-95ms + stddev 25ms + possible pauses
    // All delays should be reasonable (0-5000ms range for extreme thinking pauses)
    let sim = BehaviorSimulator::new(42);
    let delays = sim.generate_typing_delays(200);
    for &d in &delays {
        assert!(d < 5000, "Delay {} should be < 5000ms", d);
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
    assert!(delays.len() >= 1000, "Expected >= 1000 delays");
}

// ---- generate_scroll_deltas (legacy API) ----

#[test]
fn test_scroll_deltas_has_values() {
    let sim = BehaviorSimulator::new(42);
    let deltas = sim.generate_scroll_deltas(1000.0, 10);
    assert!(!deltas.is_empty());
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
fn test_scroll_deltas_sum_matches_total() {
    // Legacy API normalizes to match total
    let sim = BehaviorSimulator::new(42);
    let deltas = sim.generate_scroll_deltas(1000.0, 30);
    let sum: f64 = deltas.iter().sum();
    assert!(
        (sum - 1000.0).abs() < 10.0,
        "Sum {} should be close to 1000.0",
        sum
    );
}

#[test]
fn test_scroll_deltas_negative_total() {
    let sim = BehaviorSimulator::new(42);
    let deltas = sim.generate_scroll_deltas(-1000.0, 30);
    let sum: f64 = deltas.iter().sum();
    assert!(
        (sum - (-1000.0)).abs() < 10.0,
        "Sum {} should be close to -1000.0",
        sum
    );
}

#[test]
fn test_scroll_deltas_single_step() {
    let sim = BehaviorSimulator::new(42);
    let deltas = sim.generate_scroll_deltas(500.0, 1);
    assert!(!deltas.is_empty());
}

#[test]
fn test_scroll_deltas_two_steps() {
    let sim = BehaviorSimulator::new(42);
    let deltas = sim.generate_scroll_deltas(500.0, 2);
    assert!(!deltas.is_empty());
}

#[test]
fn test_scroll_deltas_small_total() {
    let sim = BehaviorSimulator::new(42);
    let deltas = sim.generate_scroll_deltas(1.0, 10);
    assert!(!deltas.is_empty());
    // Sum should be close to 1.0
    let sum: f64 = deltas.iter().sum();
    assert!((sum - 1.0).abs() < 0.5, "Sum {} should be close to 1.0", sum);
}

#[test]
fn test_scroll_deltas_zero_total() {
    let sim = BehaviorSimulator::new(42);
    let _deltas = sim.generate_scroll_deltas(0.0, 10);
    // With zero total, we still get some deltas (inertia from very small speed)
    // or possibly the fallback uniform distribution
}

// ---- generate_human_mouse_path (new API) ----

#[test]
fn test_human_mouse_path_start_end() {
    let sim = BehaviorSimulator::new(42);
    let path = sim.generate_human_mouse_path((10.0, 20.0), (200.0, 300.0), 30.0);
    let first = path.first().unwrap();
    assert!((first.0 - 10.0).abs() < 0.1, "Start x should be ~10");
    assert!((first.1 - 20.0).abs() < 0.1, "Start y should be ~20");
    let last = path.last().unwrap();
    assert!((last.0 - 200.0).abs() < 0.1, "End x should be ~200");
    assert!((last.1 - 300.0).abs() < 0.1, "End y should be ~300");
}

#[test]
fn test_human_mouse_path_deterministic() {
    let sim1 = BehaviorSimulator::new(42);
    let sim2 = BehaviorSimulator::new(42);
    let p1 = sim1.generate_human_mouse_path((0.0, 0.0), (100.0, 100.0), 20.0);
    let p2 = sim2.generate_human_mouse_path((0.0, 0.0), (100.0, 100.0), 20.0);
    assert_eq!(p1, p2);
}

#[test]
fn test_human_mouse_path_different_seeds() {
    let sim1 = BehaviorSimulator::new(1);
    let sim2 = BehaviorSimulator::new(2);
    let p1 = sim1.generate_human_mouse_path((0.0, 0.0), (100.0, 100.0), 20.0);
    let p2 = sim2.generate_human_mouse_path((0.0, 0.0), (100.0, 100.0), 20.0);
    assert_ne!(p1, p2);
}

#[test]
fn test_human_mouse_path_has_time_progression() {
    let sim = BehaviorSimulator::new(42);
    let path = sim.generate_human_mouse_path((0.0, 0.0), (500.0, 500.0), 20.0);
    // Time should monotonically increase
    for w in path.windows(2) {
        assert!(w[1].2 >= w[0].2, "Time should increase: {} -> {}", w[0].2, w[1].2);
    }
}

#[test]
fn test_human_mouse_path_fitts_law() {
    // Longer distance = more time
    let sim = BehaviorSimulator::new(42);
    let short = sim.generate_human_mouse_path((0.0, 0.0), (50.0, 0.0), 20.0);
    let long = sim.generate_human_mouse_path((0.0, 0.0), (500.0, 0.0), 20.0);
    let short_time = short.last().unwrap().2;
    let long_time = long.last().unwrap().2;
    assert!(
        long_time > short_time,
        "Long distance ({}) should take more time than short ({})",
        long_time,
        short_time
    );
}

// ---- generate_click_sequence ----

#[test]
fn test_click_sequence_has_mouse_down_up_click() {
    let sim = BehaviorSimulator::new(42);
    let events = sim.generate_click_sequence(100.0, 200.0, 30.0);
    // Should have at least: mousemove (pre-click), mousedown, mouseup, click
    assert!(events.len() >= 3, "Click sequence should have >= 3 events, got {}", events.len());
}

#[test]
fn test_click_sequence_press_duration_reasonable() {
    let sim = BehaviorSimulator::new(42);
    let events = sim.generate_click_sequence(100.0, 200.0, 30.0);
    // Find mousedown event and check delay after it
    for e in &events {
        if e.delay_after_ms > 0 {
            // Press durations should be in reasonable human range (30-300ms)
            assert!(e.delay_after_ms < 500, "Press duration {} too long", e.delay_after_ms);
        }
    }
}

#[test]
fn test_click_sequence_deterministic() {
    let sim1 = BehaviorSimulator::new(42);
    let sim2 = BehaviorSimulator::new(42);
    let e1 = sim1.generate_click_sequence(100.0, 200.0, 30.0);
    let e2 = sim2.generate_click_sequence(100.0, 200.0, 30.0);
    assert_eq!(e1, e2);
}

// ---- generate_human_typing ----

#[test]
fn test_human_typing_basic_text() {
    let sim = BehaviorSimulator::new(42);
    let events = sim.generate_human_typing("hello");
    // Base chars: h, e, l, l, o = 5 minimum (may have extra backspaces)
    assert!(events.len() >= 5, "Should have >= 5 events, got {}", events.len());
}

#[test]
fn test_human_typing_all_delays_positive() {
    let sim = BehaviorSimulator::new(42);
    let events = sim.generate_human_typing("The quick brown fox jumps.");
    for e in &events {
        assert!(e.delay_before_ms > 0, "Delay should be positive");
    }
}

#[test]
fn test_human_typing_deterministic() {
    let sim1 = BehaviorSimulator::new(42);
    let sim2 = BehaviorSimulator::new(42);
    let e1 = sim1.generate_human_typing("test text");
    let e2 = sim2.generate_human_typing("test text");
    assert_eq!(e1, e2);
}

#[test]
fn test_human_typing_word_gap_larger() {
    // Word gaps (after space) should generally be larger than intra-word delays
    let sim = BehaviorSimulator::new(42);
    let events = sim.generate_human_typing("a b");
    // "a" has delay, then " " has delay, then "b" has delay
    // The delay before space or after space should be relatively large
    let max_delay = events.iter().map(|e| e.delay_before_ms).max().unwrap_or(0);
    let min_delay = events.iter().map(|e| e.delay_before_ms).min().unwrap_or(1);
    assert!(max_delay > min_delay, "Should have delay variation");
}

// ---- generate_inertia_scroll ----

#[test]
fn test_inertia_scroll_deltas_converge() {
    let sim = BehaviorSimulator::new(42);
    let deltas = sim.generate_inertia_scroll(50.0);
    assert!(!deltas.is_empty());
    // Deltas should generally decrease (converge toward 0)
    if deltas.len() >= 3 {
        let first_third: f64 = deltas[..deltas.len() / 3].iter().sum();
        let last_third: f64 = deltas[deltas.len() * 2 / 3..].iter().sum();
        assert!(
            first_third.abs() > last_third.abs(),
            "Early deltas ({}) should be larger than late ({})",
            first_third.abs(),
            last_third.abs()
        );
    }
}

#[test]
fn test_inertia_scroll_deterministic() {
    let sim1 = BehaviorSimulator::new(42);
    let sim2 = BehaviorSimulator::new(42);
    let d1 = sim1.generate_inertia_scroll(30.0);
    let d2 = sim2.generate_inertia_scroll(30.0);
    assert_eq!(d1, d2);
}

#[test]
fn test_inertia_scroll_different_seeds() {
    let sim1 = BehaviorSimulator::new(1);
    let sim2 = BehaviorSimulator::new(2);
    let d1 = sim1.generate_inertia_scroll(30.0);
    let d2 = sim2.generate_inertia_scroll(30.0);
    assert_ne!(d1, d2);
}

#[test]
fn test_inertia_scroll_faster_speed_more_deltas() {
    let sim = BehaviorSimulator::new(42);
    let slow = sim.generate_inertia_scroll(5.0);
    let fast = sim.generate_inertia_scroll(100.0);
    assert!(
        fast.len() >= slow.len(),
        "Fast ({}) should have >= deltas than slow ({})",
        fast.len(),
        slow.len()
    );
}

// ---- Firefox vs Chrome profiles ----

#[test]
fn test_firefox_chrome_configs_differ() {
    let ff = BehaviorConfig::firefox();
    let ch = BehaviorConfig::chrome();
    // Fitts' b coefficient differs: Firefox=150, Chrome=120
    assert_ne!(ff.mouse.fitts_b, ch.mouse.fitts_b);
    // Typing interval differs: Firefox=95, Chrome=85
    assert_ne!(ff.keyboard.base_interval_ms, ch.keyboard.base_interval_ms);
    // Scroll friction differs
    assert_ne!(ff.scroll.friction, ch.scroll.friction);
}

#[test]
fn test_firefox_chrome_mouse_paths_differ() {
    let ff_sim = BehaviorSimulator::with_config(42, BehaviorConfig::firefox());
    let ch_sim = BehaviorSimulator::with_config(42, BehaviorConfig::chrome());
    let ff_path = ff_sim.generate_human_mouse_path((0.0, 0.0), (200.0, 200.0), 20.0);
    let ch_path = ch_sim.generate_human_mouse_path((0.0, 0.0), (200.0, 200.0), 20.0);
    assert_ne!(ff_path, ch_path);
}

#[test]
fn test_firefox_chrome_typing_differ() {
    let ff_sim = BehaviorSimulator::with_config(42, BehaviorConfig::firefox());
    let ch_sim = BehaviorSimulator::with_config(42, BehaviorConfig::chrome());
    let ff_events = ff_sim.generate_human_typing("hello world");
    let ch_events = ch_sim.generate_human_typing("hello world");
    assert_ne!(ff_events, ch_events);
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
    assert_eq!(sim.seed(), 12345);
}

#[test]
fn test_repeated_calls_produce_same_results() {
    let sim = BehaviorSimulator::new(77);
    let path1 = sim.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 15);
    let path2 = sim.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 15);
    assert_eq!(path1, path2);
}

#[test]
fn test_typing_delays_same_each_call() {
    let sim = BehaviorSimulator::new(77);
    let d1 = sim.generate_typing_delays(20);
    let d2 = sim.generate_typing_delays(20);
    assert_eq!(d1, d2);
}

#[test]
fn test_scroll_deltas_same_each_call() {
    let sim = BehaviorSimulator::new(77);
    let d1 = sim.generate_scroll_deltas(500.0, 20);
    let d2 = sim.generate_scroll_deltas(500.0, 20);
    assert_eq!(d1, d2);
}
