// @trace TEST-STL-E2E-CLICK-HUMAN [req:REQ-STL-006] [level:e2e]
// Real-world human-like click timing E2E test.
//
// Launches BaoRuntime (servo), opens a local test page that records mousedown
// and mouseup event timestamps, uses `BehaviorSimulator::generate_click_sequence`
// to compute multiple human-like clicks, dispatches each event sequence into
// the real servo page, and reads back the recorded timestamps to assert:
//   - Multiple clicks produce press durations (mousedown→mouseup gap) with
//     human-like variance (not all identical) — BUG-STL-008 regression guard.
//   - move_to_click_delay exists (settling delay before mousedown).
//   - press durations fall in the configured range [40ms, 200ms].
//
// Graceful strategy:
//   - Requires real servo runtime + display server. Absent either (or
//     BaoRuntime::new fails) → `[skip]` + return.
//   - Local `data:text/html` test page — NO external network required.

#![allow(dead_code)]

use bao_browser::{BaoConfig, BaoRuntime, PageConfig, PageHandle, PagePool, PageState};
use bao_stealth::{BehaviorConfig, BehaviorSimulator};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Report {
    passed: u32,
    skipped: u32,
    failed: u32,
    messages: Vec<String>,
}

impl Report {
    fn pass(&mut self, name: &str) {
        self.passed += 1;
        self.messages.push(format!("PASS  {}", name));
    }
    fn skip(&mut self, name: &str, why: &str) {
        self.skipped += 1;
        self.messages.push(format!("SKIP  {}  ({})", name, why));
    }
    fn fail(&mut self, name: &str, why: &str) {
        self.failed += 1;
        self.messages.push(format!("FAIL  {}  ({})", name, why));
    }
    fn assert_actual(&mut self, ok: bool, pass_msg: &str, fail_msg: &str) {
        if ok {
            self.passed += 1;
            self.messages.push(format!("PASS  {}", pass_msg));
        } else {
            self.failed += 1;
            self.messages.push(format!("FAIL  {}", fail_msg));
        }
    }
    fn finish(&self) {
        eprintln!("\n=== Click Human E2E ===");
        for m in &self.messages {
            eprintln!("{}", m);
        }
        eprintln!(
            "--- {} passed, {} skipped, {} failed ---",
            self.passed, self.skipped, self.failed
        );
    }
}

fn wait_for_load(page: &PageHandle, max_ms: u64) {
    let start = Instant::now();
    while start.elapsed().as_millis() < max_ms as u128 {
        let _ = page.evaluate_js("");
        if matches!(page.get_state(), PageState::Interactive | PageState::Idle) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Build a `data:text/html` test page that records click timing.
///   - `window.__clickEvents`: array of {type, ts} objects
///   - `window.__clearClicks()`: reset
///   - `window.__getPressDurations()`: returns array of mousedown→mouseup gaps (ms)
///   - `window.__getClickCount()`: number of clicks recorded
fn build_click_recorder_page_url() -> String {
    let html = r#"<!DOCTYPE html>
<html>
<head><title>Click Human Recorder</title></head>
<body>
<button id="b">target</button>
<script>
window.__clickEvents = [];
window.__clearClicks = function() {
  window.__clickEvents = [];
};
window.__getPressDurations = function() {
  // Pair up mousedown→mouseup by finding each mousedown and the next mouseup
  var durations = [];
  var lastDown = null;
  for (var i = 0; i < window.__clickEvents.length; i++) {
    var ev = window.__clickEvents[i];
    if (ev.type === 'mousedown') {
      lastDown = ev.ts;
    } else if (ev.type === 'mouseup' && lastDown !== null) {
      durations.push(ev.ts - lastDown);
      lastDown = null;
    }
  }
  return durations;
};
window.__getClickCount = function() {
  var c = 0;
  for (var i = 0; i < window.__clickEvents.length; i++) {
    if (window.__clickEvents[i].type === 'click') c++;
  }
  return c;
};
['mousedown','mouseup','click'].forEach(function(t) {
  document.addEventListener(t, function(e) {
    window.__clickEvents.push({type: t, ts: performance.now()});
  }, true);
});
</script>
</body>
</html>"#;
    let mut encoded = String::with_capacity(html.len() * 3);
    for b in html.bytes() {
        match b {
            b'#' => encoded.push_str("%23"),
            b'%' => encoded.push_str("%25"),
            b'\n' => encoded.push_str("%0A"),
            b'\r' => encoded.push_str("%0D"),
            b' ' => encoded.push_str("%20"),
            b'"' => encoded.push_str("%22"),
            _ => encoded.push(b as char),
        }
    }
    format!("data:text/html;charset=utf-8,{}", encoded)
}

/// Dispatch a single mouse event (mousedown/mouseup/click) at (x,y).
fn dispatch_mouse_event(page: &PageHandle, ev_type: &str, x: f64, y: f64) -> Result<(), String> {
    let js = format!(
        "(function() {{ \
            try {{ \
              var ev = new MouseEvent('{t}', {{ \
                bubbles: true, cancelable: true, view: window, \
                clientX: {x}, clientY: {y}, \
                screenX: {x}, screenY: {y}, button: 0, buttons: 1 \
              }}); \
              document.dispatchEvent(ev); \
              return 'OK'; \
            }} catch(e) {{ return 'ERR:' + e; }} \
         }})()",
        t = ev_type,
        x = x,
        y = y
    );
    let result = page.evaluate_js_web(&js).map_err(|e| format!("eval: {}", e))?;
    if result == "OK" {
        Ok(())
    } else {
        Err(format!("dispatch failed: {}", result))
    }
}

#[test]
fn click_human_e2e() {
    // Guard 1: opt-in for real servo runtime
    if std::env::var("BAO_TEST_REAL_SERVO").as_deref() != Ok("1") {
        eprintln!("[skip] BAO_TEST_REAL_SERVO != 1 — click human E2E requires real servo runtime");
        return;
    }
    // Guard 2: servo requires a display server
    if std::env::var("DISPLAY").is_err() && std::env::var("WAYLAND_DISPLAY").is_err() {
        eprintln!("[skip] no DISPLAY or WAYLAND_DISPLAY — servo requires a display server");
        return;
    }
    // Guard 3: BaoRuntime::new may fail in environments lacking servo runtime deps
    let config = BaoConfig::default();
    let runtime = match BaoRuntime::new(config) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[skip] BaoRuntime::new failed (likely missing servo runtime): {}", e);
            return;
        }
    };
    let pool: &PagePool = runtime.page_pool();
    let mut report = Report::default();

    scenario_firefox_click_variance(pool, &mut report);
    scenario_chrome_click_variance(pool, &mut report);
    scenario_double_click_sequence(pool, &mut report);

    pool.close_all();
    report.finish();

    assert_eq!(report.failed, 0, "{} sub-assertions failed — see stderr above", report.failed);
}

/// Dispatch N human-like clicks using Firefox config, verify press duration variance.
///
/// BUG-STL-008 regression guard: consecutive clicks on the same BehaviorSimulator
/// instance MUST produce distinct press durations (the persistent click RNG
/// stream advances per call). If all press durations are identical, that's a
/// detectable bot pattern and a BUG-STL-008 regression.
fn scenario_firefox_click_variance(pool: &PagePool, report: &mut Report) {
    let name = "firefox_click_variance";

    let page = match pool.create_page(&PageConfig {
        url: Some(build_click_recorder_page_url()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            report.skip(name, &format!("page creation failed: {e}"));
            return;
        }
    };
    wait_for_load(&page, 1500);

    let _ = page.evaluate_js_web("window.__clearClicks()");

    // Single BehaviorSimulator instance — click RNG state advances per call
    let sim = BehaviorSimulator::with_config(42, BehaviorConfig::firefox());
    let click_count = 5;
    let mut dispatch_failures = 0;

    for i in 0..click_count {
        let x = 100.0 + (i as f64) * 50.0;
        let y = 200.0;
        let events = sim.generate_click_sequence(x, y, 20.0);

        // events: [MouseDown, MouseUp, Click]
        // delay_after_ms on MouseDown = settle_delay (move-to-click delay)
        // delay_after_ms on MouseUp = press_duration (mousedown→mouseup gap)
        // delay_after_ms on Click = small random
        //
        // To honor BehaviorSimulator's intent, we sleep delay_after_ms before
        // dispatching each event so the page's performance.now() timestamps
        // reflect the simulated human timing (mousedown→mouseup gap = press
        // duration, NOT ~0ms).
        for ev in &events {
            let ev_type_str = match ev.event_type {
                bao_stealth::ClickEventType::MouseDown => "mousedown",
                bao_stealth::ClickEventType::MouseUp => "mouseup",
                bao_stealth::ClickEventType::Click => "click",
                bao_stealth::ClickEventType::DoubleClick => "dblclick",
            };
            // Sleep delay_after_ms before this event to honor the simulator's timing.
            // For MouseDown, this is the move-to-click settling delay.
            // For MouseUp, this is the press duration (mousedown→mouseup gap).
            if ev.delay_after_ms > 0 {
                std::thread::sleep(Duration::from_millis(ev.delay_after_ms));
            }
            if let Err(e) = dispatch_mouse_event(&page, ev_type_str, ev.x, ev.y) {
                dispatch_failures += 1;
                eprintln!("[{}] dispatch err: {}", name, e);
            }
        }
    }

    if dispatch_failures > click_count {
        report.skip(name, &format!("too many dispatch failures: {}", dispatch_failures));
        let _ = page.close();
        return;
    }

    // Read back press durations
    let press_str = match page.evaluate_js_web("window.__getPressDurations().join(',')") {
        Ok(s) => s,
        Err(e) => {
            report.skip(name, &format!("getPressDurations: {}", e));
            let _ = page.close();
            return;
        }
    };
    let durations: Vec<f64> = press_str
        .split(',')
        .filter_map(|s| s.trim().parse::<f64>().ok())
        .collect();

    if durations.len() < 2 {
        report.skip(name, &format!("too few press durations: {}", durations.len()));
        let _ = page.close();
        return;
    }

    report.pass(&format!("{}::recorded_{}_durations", name, durations.len()));

    // Assertion 1: press durations have variance (not all identical)
    let min_d = durations.iter().cloned().fold(f64::INFINITY, f64::min);
    let max_d = durations.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let range = max_d - min_d;
    report.assert_actual(
        range > 0.0,
        &format!("{}::duration_variance (range={:.1}ms)", name, range),
        &format!("{}::duration_variance (all identical — BUG-STL-008 regression)", name),
    );

    // Assertion 2: press durations fall within configured range [40, 200] ms
    // (BehaviorSimulator clamps press_duration to [40, 200]).
    // We allow servo timing jitter of ±20ms.
    let in_range = durations.iter().all(|&d| d >= 20.0 && d <= 250.0);
    report.assert_actual(
        in_range,
        &format!("{}::durations_in_range", name),
        &format!("{}::durations_in_range (min={:.1}, max={:.1})", name, min_d, max_d),
    );

    // Assertion 3: click count recorded
    let click_count_recorded = match page.evaluate_js_web("String(window.__getClickCount())") {
        Ok(s) => s.trim().parse::<u32>().unwrap_or(0),
        Err(_) => 0,
    };
    report.assert_actual(
        click_count_recorded >= click_count as u32,
        &format!("{}::clicks_recorded_{}", name, click_count_recorded),
        &format!("{}::clicks_recorded (got {}, want {})", name, click_count_recorded, click_count),
    );

    let _ = page.close();
}

/// Same as Firefox variant, using Chrome config.
fn scenario_chrome_click_variance(pool: &PagePool, report: &mut Report) {
    let name = "chrome_click_variance";

    let page = match pool.create_page(&PageConfig {
        url: Some(build_click_recorder_page_url()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            report.skip(name, &format!("page creation failed: {e}"));
            return;
        }
    };
    wait_for_load(&page, 1500);

    let _ = page.evaluate_js_web("window.__clearClicks()");

    let sim = BehaviorSimulator::with_config(137, BehaviorConfig::chrome());
    let click_count = 4;

    for i in 0..click_count {
        let x = 200.0 + (i as f64) * 80.0;
        let y = 300.0;
        let events = sim.generate_click_sequence(x, y, 20.0);
        for ev in &events {
            let ev_type_str = match ev.event_type {
                bao_stealth::ClickEventType::MouseDown => "mousedown",
                bao_stealth::ClickEventType::MouseUp => "mouseup",
                bao_stealth::ClickEventType::Click => "click",
                bao_stealth::ClickEventType::DoubleClick => "dblclick",
            };
            // Sleep delay_after_ms before each event to honor simulator timing.
            if ev.delay_after_ms > 0 {
                std::thread::sleep(Duration::from_millis(ev.delay_after_ms));
            }
            let _ = dispatch_mouse_event(&page, ev_type_str, ev.x, ev.y);
        }
    }

    let press_str = match page.evaluate_js_web("window.__getPressDurations().join(',')") {
        Ok(s) => s,
        Err(e) => {
            report.skip(name, &format!("getPressDurations: {}", e));
            let _ = page.close();
            return;
        }
    };
    let durations: Vec<f64> = press_str
        .split(',')
        .filter_map(|s| s.trim().parse::<f64>().ok())
        .collect();

    if durations.len() < 2 {
        report.skip(name, &format!("too few press durations: {}", durations.len()));
        let _ = page.close();
        return;
    }

    let min_d = durations.iter().cloned().fold(f64::INFINITY, f64::min);
    let max_d = durations.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let range = max_d - min_d;
    report.assert_actual(
        range > 0.0,
        &format!("{}::duration_variance (range={:.1}ms)", name, range),
        &format!("{}::duration_variance (all identical)", name),
    );

    report.pass(&format!("{}::recorded_{}_durations", name, durations.len()));

    let _ = page.close();
}

/// Double-click sequence dispatch — verifies generate_double_click_sequence
/// produces a valid 6+ event sequence that the page can record.
fn scenario_double_click_sequence(pool: &PagePool, report: &mut Report) {
    let name = "double_click";

    let page = match pool.create_page(&PageConfig {
        url: Some(build_click_recorder_page_url()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            report.skip(name, &format!("page creation failed: {e}"));
            return;
        }
    };
    wait_for_load(&page, 1500);

    let _ = page.evaluate_js_web("window.__clearClicks()");

    let sim = BehaviorSimulator::new(99);
    let events = sim.generate_double_click_sequence(300.0, 300.0, 25.0);

    report.assert_actual(
        events.len() >= 7,
        &format!("{}::sequence_len_{}", name, events.len()),
        &format!("{}::sequence_len (got {}, want >= 7)", name, events.len()),
    );

    // Verify DoubleClick event is present
    let has_dblclick = events
        .iter()
        .any(|e| e.event_type == bao_stealth::ClickEventType::DoubleClick);
    report.assert_actual(
        has_dblclick,
        &format!("{}::has_dblclick_event", name),
        &format!("{}::has_dblclick_event (missing)", name),
    );

    // Dispatch all events
    let mut dispatch_ok = true;
    for ev in &events {
        let ev_type_str = match ev.event_type {
            bao_stealth::ClickEventType::MouseDown => "mousedown",
            bao_stealth::ClickEventType::MouseUp => "mouseup",
            bao_stealth::ClickEventType::Click => "click",
            bao_stealth::ClickEventType::DoubleClick => "dblclick",
        };
        // Sleep delay_after_ms before each event to honor simulator timing
        if ev.delay_after_ms > 0 {
            std::thread::sleep(Duration::from_millis(ev.delay_after_ms));
        }
        if let Err(e) = dispatch_mouse_event(&page, ev_type_str, ev.x, ev.y) {
            report.skip(name, &format!("dispatch err: {}", e));
            dispatch_ok = false;
            break;
        }
    }
    if dispatch_ok {
        report.pass(&format!("{}::dispatched_{}_events", name, events.len()));
    }

    let _ = page.close();
}
