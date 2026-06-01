// @trace TEST-STL-050 [req:REQ-STL-006] [level:unit]
// Multi-threaded concurrency safety for BehaviorSimulator/CanvasNoise/StealthProfile

use bao_stealth::{BehaviorSimulator, CanvasNoise, StealthProfile};
use std::sync::Arc;
use std::thread;

// BehaviorSimulator: same seed across threads
#[test]
fn test_behavior_simulator_cross_thread_consistency() {
    let seed = 42u64;
    let handles: Vec<_> = (0..8).map(|_| {
        thread::spawn(move || {
            let bs = BehaviorSimulator::new(seed);
            let path = bs.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 20);
            let delays = bs.generate_typing_delays(10);
            let deltas = bs.generate_scroll_deltas(500.0, 10);
            (path, delays, deltas)
        })
    }).collect();

    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    // All threads should produce identical results (same seed)
    for i in 1..results.len() {
        assert_eq!(results[0].0, results[i].0, "Mouse path differs across threads");
        assert_eq!(results[0].1, results[i].1, "Typing delays differ across threads");
        assert_eq!(results[0].2, results[i].2, "Scroll deltas differ across threads");
    }
}

// CanvasNoise: same seed across threads
#[test]
fn test_canvas_noise_cross_thread_consistency() {
    let handles: Vec<_> = (0..8).map(|_| {
        thread::spawn(|| {
            let noise = CanvasNoise::new(42);
            (0..10).map(|i| {
                noise.apply_to_pixel(128, 128, 128, 255, i, i)
            }).collect::<Vec<_>>()
        })
    }).collect();

    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    for i in 1..results.len() {
        assert_eq!(results[0], results[i], "CanvasNoise differs across threads");
    }
}

// StealthProfile: clone across threads
#[test]
fn test_stealth_profile_clone_across_threads() {
    let profile = Arc::new(StealthProfile::chrome_default());
    let handles: Vec<_> = (0..4).map(|_| {
        let p = Arc::clone(&profile);
        thread::spawn(move || {
            let ua1 = p.navigator.user_agent.clone();
            let ua2 = p.navigator.user_agent.clone();
            assert_eq!(ua1, ua2);
            ua1
        })
    }).collect();

    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    for r in &results {
        assert_eq!(&results[0], r);
    }
}

// BehaviorSimulator: different seeds across threads
#[test]
fn test_behavior_simulator_different_seeds_isolation() {
    let handles: Vec<_> = (0..8).map(|seed| {
        thread::spawn(move || {
            let bs = BehaviorSimulator::new(seed as u64 + 1);
            bs.generate_mouse_path(0.0, 0.0, 100.0, 100.0, 10)
        })
    }).collect();

    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    // Different seeds should produce different results
    for i in 1..results.len() {
        assert_ne!(results[0], results[i], "Different seeds produced same path");
    }
}

// High concurrency stress test
#[test]
fn test_behavior_simulator_stress_16_threads() {
    let handles: Vec<_> = (0..16).map(|i| {
        thread::spawn(move || {
            let bs = BehaviorSimulator::new(i as u64);
            for _ in 0..100 {
                let _ = bs.generate_mouse_path(0.0, 0.0, 500.0, 500.0, 50);
                let _ = bs.generate_typing_delays(50);
                let _ = bs.generate_scroll_deltas(1000.0, 50);
            }
        })
    }).collect();

    for h in handles {
        h.join().expect("Thread panicked under stress");
    }
}

// CanvasNoise: different seeds produce different results across threads
#[test]
fn test_canvas_noise_different_seeds_isolation() {
    let handles: Vec<_> = (0..8).map(|seed| {
        thread::spawn(move || {
            let noise = CanvasNoise::new((seed + 1) as u64);
            (0..5).map(|i| {
                noise.apply_to_pixel(100, 100, 100, 255, i, i)
            }).collect::<Vec<_>>()
        })
    }).collect();

    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    for i in 1..results.len() {
        assert_ne!(results[0], results[i], "Different seeds produced same canvas noise");
    }
}

// CanvasNoise: high concurrency stress test
#[test]
fn test_canvas_noise_stress_16_threads() {
    let handles: Vec<_> = (0..16).map(|i| {
        thread::spawn(move || {
            let noise = CanvasNoise::new((i + 1) as u64);
            for x in 0..50u32 {
                for y in 0..50u32 {
                    let _ = noise.apply_to_pixel(128, 64, 32, 255, x, y);
                }
            }
        })
    }).collect();

    for h in handles {
        h.join().expect("CanvasNoise thread panicked under stress");
    }
}

// StealthProfile: concurrent read access via Arc
#[test]
fn test_stealth_profile_concurrent_read_access() {
    let profile = Arc::new(StealthProfile::firefox_default());
    let handles: Vec<_> = (0..8).map(|_| {
        let p = Arc::clone(&profile);
        thread::spawn(move || {
            // Multiple concurrent reads
            let _ = &p.navigator.user_agent;
            let _ = &p.tls.ja3_hash;
            let _ = &p.http2.header_table_size;
            let _ = p.canvas.seed();
            let _ = &p.screen.width;
            let _ = &p.webgl.vendor;
            let _ = p.audio.noise_amplitude();
            let _ = p.behavior.seed();
        })
    }).collect();

    for h in handles {
        h.join().expect("StealthProfile read thread panicked");
    }
}

// BehaviorSimulator: repeated calls within same thread are consistent
#[test]
fn test_behavior_simulator_intra_thread_consistency() {
    let bs = BehaviorSimulator::new(12345);
    let path1 = bs.generate_mouse_path(0.0, 0.0, 200.0, 200.0, 15);
    let path2 = bs.generate_mouse_path(0.0, 0.0, 200.0, 200.0, 15);
    assert_eq!(path1, path2, "Same thread, same seed should produce identical paths");
}

// CanvasNoise: repeated calls within same thread are consistent
#[test]
fn test_canvas_noise_intra_thread_consistency() {
    let noise = CanvasNoise::new(999);
    let p1 = noise.apply_to_pixel(50, 50, 50, 255, 100, 200);
    let p2 = noise.apply_to_pixel(50, 50, 50, 255, 100, 200);
    assert_eq!(p1, p2, "Same thread, same seed should produce identical pixel noise");
}
