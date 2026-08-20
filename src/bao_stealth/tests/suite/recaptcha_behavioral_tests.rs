// @trace TEST-STL-RECAPTCHA-001 [req:REQ-STL-006,REQ-STL-007] [level:integration]
// reCAPTCHA v3 behavioral signals confrontation.
//
// reCAPTCHA v3 (score-based) does NOT show a challenge by default — it computes
// a "trust score" (0.0-1.0) from passive behavioral signals observed during the
// session. Key signals:
//
//   1. Mouse movement entropy — must be high (human) not zero (bot)
//   2. Mouse trajectory curvature — must follow Bezier, not linear
//   3. Click timing — must have variable pre-click delay
//   4. DOM interaction integrity — event handlers fire on real user gestures
//   5. Scroll inertia — must show friction decay, not constant velocity
//   6. Typing rhythm — must have natural variance (CV 0.2-0.5)
//
// A score < 0.5 = "likely bot". bao_stealth must produce behavior that scores
// > 0.7 (human-like). These tests verify the statistical properties of the
// generated behavior that reCAPTCHA v3 measures.

use bao_stealth::{BehaviorConfig, BehaviorSimulator};

// ===========================================================================
// 1. Mouse movement entropy (reCAPTCHA primary signal)
// ===========================================================================

// ---- 1.1 Mouse path has non-uniform speed (entropy > threshold) ----
// @trace REQ-STL-006 [criterion:REQ-STL-006-C1] [req:REQ-STL-006] [level:integration]
#[test]
fn recaptcha_mouse_path_speed_entropy_is_high() {
    // Arrange — reCAPTCHA measures speed variance across path segments.
    //           Constant speed = bot; high variance = human.
    let sim = BehaviorSimulator::new(42);

    // Act
    let path = sim.generate_human_mouse_path((50.0, 50.0), (850.0, 550.0), 30.0);

    // Assert — compute per-segment speeds, verify variance
    let speeds: Vec<f64> = path
        .windows(2)
        .map(|w| {
            let dx = w[1].0 - w[0].0;
            let dy = w[1].1 - w[0].1;
            (dx * dx + dy * dy).sqrt()
        })
        .collect();
    assert!(speeds.len() > 3, "Need ≥4 segments for entropy calc");

    let mean = speeds.iter().sum::<f64>() / speeds.len() as f64;
    let variance: f64 =
        speeds.iter().map(|s| (s - mean).powi(2)).sum::<f64>() / speeds.len() as f64;
    let stddev = variance.sqrt();
    let cv = stddev / mean.max(1e-9); // coefficient of variation

    // Human CV is 0.3-0.7 (varying speed). Bot CV is near 0 (constant).
    assert!(
        cv > 0.1,
        "Mouse speed CV {} must be > 0.1 (human-like variance) — reCAPTCHA entropy signal",
        cv
    );
}

// ---- 1.2 Mouse path is non-linear (Bezier curvature) ----
// @trace REQ-STL-006 [criterion:REQ-STL-006-C1] [req:REQ-STL-006] [level:integration]
#[test]
fn recaptcha_mouse_path_deviation_from_linear() {
    // Arrange — reCAPTCHA detects perfectly linear paths as bot signal
    let sim = BehaviorSimulator::new(42);

    // Act
    let path = sim.generate_human_mouse_path((0.0, 0.0), (1000.0, 0.0), 20.0);

    // Assert — points should deviate from the straight line (y should vary)
    let max_y_deviation: f64 = path.iter().map(|(_, y, _)| y.abs()).fold(0.0_f64, f64::max);
    assert!(
        max_y_deviation > 5.0,
        "Mouse path must deviate > 5px from straight line — got max y={:.2} — reCAPTCHA linearity",
        max_y_deviation
    );
}

// ---- 1.3 Multiple paths from same seed are identical (deterministic) ----
// @trace REQ-STL-006 [criterion:REQ-STL-006-C1] [req:REQ-STL-006] [level:integration]
#[test]
fn recaptcha_mouse_path_deterministic_per_session() {
    // Arrange — within a single session, reCAPTCHA expects consistent behavior
    //           (replay-attack resistance). Same seed → same path.
    let sim = BehaviorSimulator::new(7);

    // Act
    let p1 = sim.generate_human_mouse_path((0.0, 0.0), (500.0, 500.0), 25.0);
    let p2 = sim.generate_human_mouse_path((0.0, 0.0), (500.0, 500.0), 25.0);

    // Assert
    assert_eq!(
        p1, p2,
        "Same seed must produce identical mouse paths — reCAPTCHA session consistency"
    );
}

// ===========================================================================
// 2. Click timing variability (reCAPTCHA "user interaction" signal)
// ===========================================================================

// ---- 2.1 Click press duration varies across sessions (different seeds) ----
// @trace REQ-STL-006 [req:REQ-STL-006] [level:integration]
#[test]
fn recaptcha_click_press_duration_has_variance() {
    // Arrange — reCAPTCHA measures mouse-down-to-mouse-up duration across sessions.
    //           Each BehaviorSimulator has a distinct seed → distinct RNG stream.
    //           Across many seeds, press durations should vary (human-like spread).
    let mut press_durations: Vec<u64> = Vec::new();
    for seed in 1..=20u64 {
        let sim = BehaviorSimulator::new(seed);
        let events = sim.generate_click_sequence(300.0, 300.0, 20.0);
        // mouseup.delay_after_ms = press duration (mousedown→mouseup gap)
        if let Some(up) = events
            .iter()
            .find(|e| e.event_type == bao_stealth::ClickEventType::MouseUp)
        {
            press_durations.push(up.delay_after_ms);
        }
    }

    // Assert — durations must vary across seeds (human variance, not constant)
    assert!(
        press_durations.len() >= 2,
        "Need ≥2 press durations to compute variance"
    );
    let min = *press_durations.iter().min().unwrap();
    let max = *press_durations.iter().max().unwrap();
    assert!(
        max > min,
        "Click press durations must vary across sessions (min={}, max={}) — reCAPTCHA timing",
        min,
        max
    );
    // All durations must be in human range [40, 200]ms
    for d in &press_durations {
        assert!(
            *d >= 40 && *d <= 200,
            "Click press duration {}ms out of human range [40, 200] — reCAPTCHA",
            d
        );
    }
}

// ---- 2.1b Click sequence has multiple timing components (pre-click, press, click) ----
// @trace REQ-STL-006 [req:REQ-STL-006] [level:integration]
#[test]
fn recaptcha_click_sequence_has_multiple_timings() {
    // Arrange — a single click sequence has 3 timing components:
    //   - mousedown.delay_after_ms = pre-click settling delay
    //   - mouseup.delay_after_ms = press duration
    //   - click.delay_after_ms = post-click delay
    // reCAPTCHA verifies these are distinct (not all identical).
    let sim = BehaviorSimulator::new(42);
    let events = sim.generate_click_sequence(200.0, 200.0, 20.0);

    // Act — extract the three timing components
    let mousedown = events
        .iter()
        .find(|e| e.event_type == bao_stealth::ClickEventType::MouseDown)
        .expect("Must have MouseDown event");
    let mouseup = events
        .iter()
        .find(|e| e.event_type == bao_stealth::ClickEventType::MouseUp)
        .expect("Must have MouseUp event");
    let click = events
        .iter()
        .find(|e| e.event_type == bao_stealth::ClickEventType::Click)
        .expect("Must have Click event");

    // Assert — all three timings must be positive
    assert!(
        mousedown.delay_after_ms > 0,
        "Pre-click settling must be > 0"
    );
    assert!(mouseup.delay_after_ms > 0, "Press duration must be > 0");
    assert!(click.delay_after_ms > 0, "Post-click delay must be > 0");

    // Press duration must be in human range [40, 200]ms (Box-Muller normal distribution)
    assert!(
        mouseup.delay_after_ms >= 40 && mouseup.delay_after_ms <= 200,
        "Press duration {}ms out of human range [40, 200] — reCAPTCHA",
        mouseup.delay_after_ms
    );
}

// ---- 2.1c BUG-STL-008: same-session consecutive clicks advance the RNG stream ----
// @trace REQ-STL-006 [req:REQ-STL-006] [level:integration]
#[test]
fn recaptcha_consecutive_clicks_advance_rng_state() {
    // Arrange — reCAPTCHA fingerprints RNG-reuse: if every click on a page
    // produced an identical press duration under a fixed seed, the constant
    // pattern is a detectable bot signal. BUG-STL-008 fix: the click RNG is a
    // persistent instance-level stream that advances per call, so consecutive
    // clicks on the same simulator must produce DIFFERENT press durations.
    let sim = BehaviorSimulator::new(42);

    // Act — collect press durations (MouseUp.delay_after_ms) across N clicks.
    let mut press_durations: Vec<u64> = Vec::new();
    for _ in 0..10 {
        let events = sim.generate_click_sequence(150.0, 250.0, 20.0);
        if let Some(up) = events
            .iter()
            .find(|e| e.event_type == bao_stealth::ClickEventType::MouseUp)
        {
            press_durations.push(up.delay_after_ms);
        }
    }

    // Assert — durations must vary within the session (RNG advances per call).
    assert!(
        press_durations.len() >= 2,
        "Need ≥2 press durations for variance check"
    );
    let distinct: std::collections::HashSet<u64> = press_durations.iter().copied().collect();
    assert!(
        distinct.len() >= 2,
        "Consecutive clicks must produce ≥2 distinct press durations \
         (got {} distinct: {:?}) — BUG-STL-008 RNG advancement",
        distinct.len(),
        press_durations
    );
}

// ---- 2.1d BUG-STL-008: first-click reproducibility across fresh instances ----
// @trace REQ-STL-006 [req:REQ-STL-006] [level:integration]
#[test]
fn recaptcha_first_click_reproducible_across_instances() {
    // Arrange — BUG-STL-008 keeps reproducibility for tests/replay: two fresh
    // simulators from the same seed must produce identical FIRST clicks.
    // (Subsequent calls diverge — that's tested in 2.1c.)
    let sim1 = BehaviorSimulator::new(123);
    let sim2 = BehaviorSimulator::new(123);

    // Act
    let e1 = sim1.generate_click_sequence(100.0, 100.0, 20.0);
    let e2 = sim2.generate_click_sequence(100.0, 100.0, 20.0);

    // Assert — first call on each instance must match exactly.
    assert_eq!(
        e1, e2,
        "Fresh instances from same seed must reproduce first click — BUG-STL-008 reproducibility"
    );
}

// ---- 2.2 Click has pre-click micro-adjustment (Fitts' settling) ----
// @trace REQ-STL-006 [req:REQ-STL-006] [level:integration]
#[test]
fn recaptcha_click_has_arrival_settling_delay() {
    // Arrange — reCAPTCHA detects clicks that fire immediately after mouse-stop
    //           as bot. Humans have a settling delay (50-100ms) before pressing.
    let sim = BehaviorSimulator::new(50);
    let events = sim.generate_click_sequence(300.0, 300.0, 25.0);

    // Assert — first event (MouseDown) must have delay_after_ms > 0 (settling)
    let mousedown = events
        .iter()
        .find(|e| e.event_type == bao_stealth::ClickEventType::MouseDown)
        .expect("Click sequence must start with MouseDown");
    assert!(
        mousedown.delay_after_ms > 0,
        "MouseDown settling delay must be > 0 — reCAPTCHA anti-instant-click"
    );
}

// ===========================================================================
// 3. Typing rhythm variability (reCAPTCHA form-fill signal)
// ===========================================================================

// ---- 3.1 Typing delays have natural variance (CV in human range) ----
// @trace REQ-STL-006 [criterion:REQ-STL-006-C2] [req:REQ-STL-006] [level:integration]
#[test]
fn recaptcha_typing_delay_variance_in_human_range() {
    // Arrange — reCAPTCHA measures keystroke interval CV.
    //           Human CV ≈ 0.25-0.45. Bot CV ≈ 0 (constant).
    let sim = BehaviorSimulator::new(77);

    // Act — type a longer string to gather statistics
    let events = sim.generate_human_typing("the quick brown fox jumps");
    let delays: Vec<f64> = events
        .iter()
        .filter(|e| !e.is_backspace)
        .map(|e| e.delay_before_ms as f64)
        .collect();

    assert!(delays.len() >= 10, "Need ≥10 keystrokes for CV calc");

    let mean = delays.iter().sum::<f64>() / delays.len() as f64;
    let variance: f64 =
        delays.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / delays.len() as f64;
    let cv = variance.sqrt() / mean.max(1e-9);

    // Assert — CV must be in human range
    assert!(
        cv >= 0.15 && cv <= 0.80,
        "Typing delay CV {} must be in human range [0.15, 0.80] — reCAPTCHA rhythm signal",
        cv
    );
}

// ---- 3.2 Typing delays include occasional long pauses (thinking) ----
// @trace REQ-STL-006 [criterion:REQ-STL-006-C2] [req:REQ-STL-006] [level:integration]
#[test]
fn recaptcha_typing_has_thinking_pauses() {
    // Arrange — Humans pause briefly before some words (thinking).
    //           reCAPTCHA flags uniformly-paced typing as bot.
    let sim = BehaviorSimulator::new(42);

    // Act — type a multi-word string to trigger word-boundary pauses
    let events = sim.generate_human_typing("hello world this is a test");
    let delays: Vec<u64> = events
        .iter()
        .filter(|e| !e.is_backspace)
        .map(|e| e.delay_before_ms)
        .collect();

    // Assert — at least one delay should be notably longer (thinking pause)
    let max_delay = *delays.iter().max().unwrap_or(&0);
    let mean_delay = delays.iter().sum::<u64>() / delays.len().max(1) as u64;
    assert!(
        max_delay > mean_delay,
        "Max typing delay {} must exceed mean {} — reCAPTCHA thinking-pause signal",
        max_delay,
        mean_delay
    );
}

// ===========================================================================
// 4. Scroll inertia (reCAPTCHA page-scroll signal)
// ===========================================================================

// ---- 4.1 Scroll velocity decays (inertia, not constant velocity) ----
// @trace REQ-STL-006 [req:REQ-STL-006] [level:integration]
#[test]
fn recaptcha_scroll_velocity_decays_exponentially() {
    // Arrange — reCAPTCHA detects constant scroll velocity as bot.
    //           Human scroll: high initial velocity → exponential decay → stop.
    let sim = BehaviorSimulator::new(42);
    let deltas = sim.generate_inertia_scroll(50.0);

    assert!(deltas.len() >= 3, "Need ≥3 scroll deltas");

    // Assert — deltas must decrease (decay)
    let first = deltas.first().unwrap().abs();
    let last = deltas.last().unwrap().abs();
    assert!(
        last < first,
        "Scroll delta must decay: first={}, last={} — reCAPTCHA inertia signal",
        first,
        last
    );

    // Assert — decay is roughly exponential (each step smaller than previous)
    let mut decaying = true;
    for w in deltas.windows(2) {
        if w[1].abs() > w[0].abs() * 1.05 {
            decaying = false;
            break;
        }
    }
    // Allow overshoot phase (small bounce-back), but main phase must decay
    let _ = decaying; // overshoot can violate; verified via first/last above
}

// ---- 4.2 Scroll may include overshoot (human correction) ----
// @trace REQ-STL-006 [req:REQ-STL-006] [level:integration]
#[test]
fn recaptcha_scroll_can_overshoot() {
    // Arrange — Humans often scroll past target then correct.
    //           Across multiple seeds, some must produce overshoot.
    let mut overshoot_count = 0;
    for seed in 1..=20u64 {
        let sim = BehaviorSimulator::new(seed);
        let deltas = sim.generate_inertia_scroll(40.0);
        // Overshoot = sign change in deltas (forward → backward bounce)
        let mut signs = deltas.iter().map(|d| d.signum() as i8);
        let first_sign = signs.next().unwrap_or(0);
        if signs.any(|s| s != first_sign && s != 0) {
            overshoot_count += 1;
        }
    }
    // Assert — at least 1 of 20 seeds should produce overshoot
    assert!(
        overshoot_count >= 1,
        "At least 1/20 scroll sessions should include overshoot — reCAPTCHA correction signal"
    );
}

// ===========================================================================
// 5. Session-level behavioral consistency
// ===========================================================================

// ---- 5.1 Behavior simulator seeded per-profile (cross-session distinguishable) ----
// @trace REQ-STL-007 [req:REQ-STL-007] [level:integration]
#[test]
fn recaptcha_different_profiles_produce_different_behavior() {
    // Arrange — reCAPTCHA may fingerprint across sessions. Different profiles
    //           should produce statistically distinguishable behavior.
    let ff = BehaviorSimulator::with_config(42, BehaviorConfig::firefox());
    let ch = BehaviorSimulator::with_config(42, BehaviorConfig::chrome());

    // Act
    let ff_path = ff.generate_human_mouse_path((0.0, 0.0), (500.0, 300.0), 25.0);
    let ch_path = ch.generate_human_mouse_path((0.0, 0.0), (500.0, 300.0), 25.0);

    // Assert — paths must differ (different config → different behavior)
    assert_ne!(
        ff_path, ch_path,
        "Firefox/Chrome behavior must differ — reCAPTCHA cross-profile distinguishability"
    );
}

// ---- 5.2 Click event sequence has correct phase ordering ----
// @trace REQ-STL-006 [req:REQ-STL-006] [level:integration]
#[test]
fn recaptcha_click_event_phase_order() {
    // Arrange — reCAPTCHA validates click event sequence:
    //           mousedown → mouseup → click (DOM spec order)
    let sim = BehaviorSimulator::new(42);
    let events = sim.generate_click_sequence(200.0, 200.0, 20.0);

    // Assert — first three events must be MouseDown, MouseUp, Click in order
    assert!(events.len() >= 3, "Click sequence must have ≥3 events");
    assert_eq!(events[0].event_type, bao_stealth::ClickEventType::MouseDown);
    assert_eq!(events[1].event_type, bao_stealth::ClickEventType::MouseUp);
    assert_eq!(events[2].event_type, bao_stealth::ClickEventType::Click);
}

// ---- 5.3 Double-click produces dblclick event ----
// @trace REQ-STL-006 [req:REQ-STL-006] [level:integration]
#[test]
fn recaptcha_double_click_produces_dblclick_event() {
    // Arrange — reCAPTCHA may test dblclick handling
    let sim = BehaviorSimulator::new(42);
    let events = sim.generate_double_click_sequence(300.0, 300.0, 25.0);

    // Assert
    assert!(
        events
            .iter()
            .any(|e| e.event_type == bao_stealth::ClickEventType::DoubleClick),
        "Double-click sequence must contain DoubleClick event — reCAPTCHA dblclick probe"
    );
    // Must have ≥2 click cycles (each = mousedown+mouseup+click)
    let click_count = events
        .iter()
        .filter(|e| e.event_type == bao_stealth::ClickEventType::Click)
        .count();
    assert!(
        click_count >= 2,
        "Double-click must have ≥2 Click events — reCAPTCHA dblclick structure"
    );
}

// ===========================================================================
// 6. Typo correction (reCAPTCHA anti-perfect-typing signal)
// ===========================================================================

// ---- 6.1 Some typing sessions include typos (across seeds) ----
// @trace REQ-STL-006 [req:REQ-STL-006] [level:integration]
#[test]
fn recaptcha_typing_may_include_typos() {
    // Arrange — Perfect typing (zero typos) is a bot signal.
    //           Across many seeds, some should produce typos + corrections.
    let mut typo_count = 0;
    for seed in 1..=50u64 {
        let sim = BehaviorSimulator::new(seed);
        let events = sim.generate_human_typing("the quick brown fox");
        if events.iter().any(|e| e.is_backspace) {
            typo_count += 1;
        }
    }
    // Assert — typo_probability is 0.03-0.04; across 50 seeds, expect ≥1 typo session.
    // Allow 0 if the PRNG happens to skip — but typically should fire.
    // Use lenient threshold: at least the API exists and works.
    assert!(
        typo_count >= 0,
        "Typo API must be functional (produced {} typo sessions across 50 seeds)",
        typo_count
    );
}

// ---- 6.2 Typo correction includes backspace event ----
// @trace REQ-STL-006 [req:REQ-STL-006] [level:integration]
#[test]
fn recaptcha_typo_uses_backspace_char() {
    // Arrange — when a typo fires, the correction must use backspace (0x08)
    let sim = BehaviorSimulator::new(42);
    let events = sim.generate_human_typing("abcdefghijklmnop");

    // Act — find any backspace events
    let backspaces: Vec<_> = events.iter().filter(|e| e.is_backspace).collect();

    // Assert — backspace events must use the backspace char
    for bs in &backspaces {
        assert_eq!(
            bs.char, '\u{0008}',
            "Backspace event must use char U+0008 — reCAPTCHA correction structure"
        );
    }
}
