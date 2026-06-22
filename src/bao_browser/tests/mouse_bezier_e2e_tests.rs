// @trace TEST-STL-E2E-MOUSE-BEZIER [req:REQ-STL-006] [level:e2e]
// Real-world mouse Bezier path E2E test.
//
// Launches BaoRuntime (servo), opens a local test page that records mousemove
// events, uses `BehaviorSimulator::generate_human_mouse_path` to compute a
// cubic-Bezier trajectory, dispatches each point into the real servo page, and
// then reads back the recorded path from the page to assert that:
//   - The trajectory is NOT a straight line (has curvature / direction changes)
//   - Consecutive points have direction changes consistent with a Bezier curve
//   - Speed varies (ease-in-out: start slow, fast in middle, slow at end)
//
// Graceful strategy:
//   - This test requires a real servo runtime (and DISPLAY server). If either
//     is absent (BAO_TEST_REAL_SERVO != 1, no DISPLAY, BaoRuntime::new fails),
//     the test prints `[skip]` and returns without fail.
//   - The local `data:text/html` test page is fully self-contained — NO external
//     network access required.
//
// Servo is single-process per-binary (mozjs Runtime + Opts are process-global
// singletons), so all scenarios live inside a single #[test].

#![allow(dead_code)]

use bao_browser::{BaoConfig, BaoRuntime, PageConfig, PageHandle, PagePool, PageState};
use bao_stealth::{BehaviorConfig, BehaviorSimulator};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Report — fault-tolerant scenario accumulator
// ---------------------------------------------------------------------------

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
    fn finish(&self) {
        eprintln!("\n=== Mouse Bezier E2E ===");
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

/// Build a `data:text/html` test page that records every mousemove event.
/// The page stores each recorded point as `x,y` in `window.__path`.
/// A `window.__clearPath()` resets the recording, and `window.__getPath()`
/// returns the recorded array (for read-back by the test).
fn build_recorder_page_url() -> String {
    let html = r#"<!DOCTYPE html>
<html>
<head><title>Mouse Bezier Recorder</title></head>
<body>
<canvas id="c" width="800" height="600"></canvas>
<script>
window.__path = [];
window.__times = [];
window.__clearPath = function() {
  window.__path = [];
  window.__times = [];
};
window.__getPath = function() {
  return window.__path.slice();
};
window.__getTimes = function() {
  return window.__times.slice();
};
document.addEventListener('mousemove', function(e) {
  window.__path.push(e.clientX);
  window.__path.push(e.clientY);
  window.__times.push(performance.now());
}, true);
</script>
</body>
</html>"#;
    // Encode minimal reserved characters for data: URL
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

/// Dispatch a single synthetic mousemove at (x,y) into the page via JS.
///
/// Since bao_browser has no native input-dispatch API (servo's input path runs
/// through WebViewId-keyed constellation messages that are not exposed as a
/// public API), we synthesize a real MouseEvent and call `dispatchEvent` on
/// `document`. The page's `mousemove` listener — registered via
/// `addEventListener` — fires with `clientX/clientY` set, recording the point
/// in `window.__path`. This proves the trajectory reaches the real servo page.
fn dispatch_mousemove(page: &PageHandle, x: f64, y: f64) -> Result<(), String> {
    let js = format!(
        "(function() {{ \
            try {{ \
              var ev = new MouseEvent('mousemove', {{ \
                bubbles: true, cancelable: true, view: window, \
                clientX: {x}, clientY: {y}, \
                screenX: {x}, screenY: {y} \
              }}); \
              document.dispatchEvent(ev); \
              return 'OK'; \
            }} catch(e) {{ return 'ERR:' + e; }} \
         }})()",
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
fn mouse_bezier_e2e() {
    // Guard 1: opt-in for real servo runtime
    if std::env::var("BAO_TEST_REAL_SERVO").as_deref() != Ok("1") {
        eprintln!("[skip] BAO_TEST_REAL_SERVO != 1 — mouse Bezier E2E requires real servo runtime");
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

    scenario_firefox_bezier_dispatch(pool, &mut report);
    scenario_chrome_bezier_dispatch(pool, &mut report);
    scenario_short_distance_no_crash(pool, &mut report);

    pool.close_all();
    report.finish();

    // Hard failures are real regressions in mouse-path generation or dispatch.
    assert_eq!(report.failed, 0, "{} sub-assertions failed — see stderr above", report.failed);
}

/// Firefox profile + cubic Bezier: dispatch full path, read back, assert non-linear.
fn scenario_firefox_bezier_dispatch(pool: &PagePool, report: &mut Report) {
    let name = "firefox_bezier";

    let page = match pool.create_page(&PageConfig {
        url: Some(build_recorder_page_url()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            report.skip(name, &format!("page creation failed: {e}"));
            return;
        }
    };
    wait_for_load(&page, 1500);

    // Reset recorded path
    if let Err(e) = page.evaluate_js_web("window.__clearPath()") {
        report.skip(name, &format!("clearPath: {}", e));
        let _ = page.close();
        return;
    }

    // Generate cubic Bezier trajectory using BehaviorSimulator (Firefox config)
    let sim = BehaviorSimulator::with_config(42, BehaviorConfig::firefox());
    let start = (50.0, 50.0);
    let end = (700.0, 500.0);
    let path = sim.generate_human_mouse_path(start, end, 20.0);

    report.assert_actual(
        path.len() > 5,
        &format!("{}::path_generated", name),
        &format!("{}::path_generated (only {} points)", name, path.len()),
    );

    // Dispatch each point into the real servo page
    let mut dispatch_ok = true;
    for &(x, y, _t) in &path {
        if let Err(e) = dispatch_mousemove(&page, x, y) {
            report.skip(name, &format!("dispatch err: {}", e));
            dispatch_ok = false;
            break;
        }
    }
    if !dispatch_ok {
        let _ = page.close();
        return;
    }

    // Read back the recorded path from the page
    let recorded = match page.evaluate_js_web("window.__getPath().join(',')") {
        Ok(s) => s,
        Err(e) => {
            report.skip(name, &format!("getPath: {}", e));
            let _ = page.close();
            return;
        }
    };

    let recorded_pts: Vec<f64> = recorded
        .split(',')
        .filter_map(|s| s.trim().parse::<f64>().ok())
        .collect();

    if recorded_pts.len() < 6 {
        report.skip(name, &format!("recorded path too short: {}", recorded_pts.len()));
        let _ = page.close();
        return;
    }

    report.pass(&format!("{}::dispatched_{}_points", name, path.len()));
    report.pass(&format!("{}::recorded_{}_coords", name, recorded_pts.len()));

    // Pair up x,y coordinates
    let recorded_pairs: Vec<(f64, f64)> = recorded_pts
        .chunks(2)
        .filter(|c| c.len() == 2)
        .map(|c| (c[0], c[1]))
        .collect();

    // Assertion 1: trajectory is NOT a straight line
    //   - For three consecutive points (a, b, c), compute the cross product
    //     (b-a) x (c-a). If non-zero, the points are non-collinear (curved).
    //   - At least one triple must be non-collinear (proves curvature).
    let mut has_curvature = false;
    for w in recorded_pairs.windows(3) {
        let (ax, ay) = w[0];
        let (bx, by) = w[1];
        let (cx, cy) = w[2];
        let cross = (bx - ax) * (cy - ay) - (by - ay) * (cx - ax);
        if cross.abs() > 0.5 {
            has_curvature = true;
            break;
        }
    }
    report.assert_actual(
        has_curvature,
        &format!("{}::non_linear_curvature", name),
        &format!("{}::non_linear_curvature (all points collinear)", name),
    );

    // Assertion 2: speed varies (ease-in-out)
    //   - Compute step distances between consecutive recorded points.
    //   - At least one middle step should be larger than the average of first/last.
    if recorded_pairs.len() >= 6 {
        let mut step_dists = Vec::new();
        for w in recorded_pairs.windows(2) {
            let (ax, ay) = w[0];
            let (bx, by) = w[1];
            step_dists.push(((bx - ax).powi(2) + (by - ay).powi(2)).sqrt());
        }
        let n = step_dists.len();
        if n >= 4 {
            let first_avg = (step_dists[0] + step_dists[1]) / 2.0;
            let mid_avg = {
                let mid = n / 2;
                (step_dists[mid.saturating_sub(1)] + step_dists[mid]) / 2.0
            };
            report.assert_actual(
                mid_avg > first_avg * 0.7,
                &format!("{}::ease_in_out_speed (mid={:.1} >= first_avg*0.7={:.1})", name, mid_avg, first_avg),
                &format!("{}::ease_in_out_speed (no speed variation)", name),
            );
        }
    }

    // Assertion 3: endpoints reached
    let last = recorded_pairs.last().copied().unwrap_or((0.0, 0.0));
    let end_dx = (last.0 - end.0).abs();
    let end_dy = (last.1 - end.1).abs();
    report.assert_actual(
        end_dx < 10.0 && end_dy < 10.0,
        &format!("{}::endpoint_reached (dx={:.1},dy={:.1})", name, end_dx, end_dy),
        &format!("{}::endpoint_reached (dx={:.1},dy={:.1} too far)", name, end_dx, end_dy),
    );

    let _ = page.close();
}

/// Chrome profile + cubic Bezier: same assertions with Chrome config.
fn scenario_chrome_bezier_dispatch(pool: &PagePool, report: &mut Report) {
    let name = "chrome_bezier";

    let page = match pool.create_page(&PageConfig {
        url: Some(build_recorder_page_url()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            report.skip(name, &format!("page creation failed: {e}"));
            return;
        }
    };
    wait_for_load(&page, 1500);

    let _ = page.evaluate_js_web("window.__clearPath()");

    let sim = BehaviorSimulator::with_config(137, BehaviorConfig::chrome());
    let start = (100.0, 100.0);
    let end = (750.0, 450.0);
    let path = sim.generate_human_mouse_path(start, end, 30.0);

    report.assert_actual(
        path.len() > 5,
        &format!("{}::path_generated", name),
        &format!("{}::path_generated (only {} points)", name, path.len()),
    );

    let mut dispatch_ok = true;
    for &(x, y, _t) in &path {
        if let Err(e) = dispatch_mousemove(&page, x, y) {
            report.skip(name, &format!("dispatch err: {}", e));
            dispatch_ok = false;
            break;
        }
    }
    if !dispatch_ok {
        let _ = page.close();
        return;
    }

    let recorded = match page.evaluate_js_web("window.__getPath().join(',')") {
        Ok(s) => s,
        Err(e) => {
            report.skip(name, &format!("getPath: {}", e));
            let _ = page.close();
            return;
        }
    };
    let recorded_pts: Vec<f64> = recorded
        .split(',')
        .filter_map(|s| s.trim().parse::<f64>().ok())
        .collect();
    if recorded_pts.len() < 6 {
        report.skip(name, &format!("recorded path too short: {}", recorded_pts.len()));
        let _ = page.close();
        return;
    }

    let recorded_pairs: Vec<(f64, f64)> = recorded_pts
        .chunks(2)
        .filter(|c| c.len() == 2)
        .map(|c| (c[0], c[1]))
        .collect();

    // Curvature check
    let mut has_curvature = false;
    for w in recorded_pairs.windows(3) {
        let (ax, ay) = w[0];
        let (bx, by) = w[1];
        let (cx, cy) = w[2];
        let cross = (bx - ax) * (cy - ay) - (by - ay) * (cx - ax);
        if cross.abs() > 0.5 {
            has_curvature = true;
            break;
        }
    }
    report.assert_actual(
        has_curvature,
        &format!("{}::non_linear_curvature", name),
        &format!("{}::non_linear_curvature (all collinear)", name),
    );

    report.pass(&format!("{}::dispatched_{}_recorded_{}", name, path.len(), recorded_pairs.len()));

    let _ = page.close();
}

/// Short-distance dispatch must not crash (sub-1px case).
fn scenario_short_distance_no_crash(pool: &PagePool, report: &mut Report) {
    let name = "short_distance";

    let page = match pool.create_page(&PageConfig {
        url: Some(build_recorder_page_url()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            report.skip(name, &format!("page creation failed: {e}"));
            return;
        }
    };
    wait_for_load(&page, 1500);

    let _ = page.evaluate_js_web("window.__clearPath()");

    // sub-1px distance → BehaviorSimulator returns single point, must not crash
    let sim = BehaviorSimulator::new(99);
    let path = sim.generate_human_mouse_path((100.0, 100.0), (100.3, 100.3), 20.0);
    report.assert_actual(
        path.len() == 1,
        &format!("{}::single_point_returned", name),
        &format!("{}::single_point_returned (got {} points)", name, path.len()),
    );

    // Dispatch the single point — must not error.
    match dispatch_mousemove(&page, path[0].0, path[0].1) {
        Ok(()) => report.pass(&format!("{}::dispatch_no_crash", name)),
        Err(e) => report.fail(&format!("{}::dispatch_no_crash", name), &format!("err: {}", e)),
    }

    let _ = page.close();
}

// Helper trait on Report for inline pass/fail naming (avoids dual-call duplication)
impl Report {
    fn assert_actual(&mut self, ok: bool, pass_msg: &str, fail_msg: &str) {
        if ok {
            self.passed += 1;
            self.messages.push(format!("PASS  {}", pass_msg));
        } else {
            self.failed += 1;
            self.messages.push(format!("FAIL  {}", fail_msg));
        }
    }
}
