// @trace REQ-BRW-051 [criterion:view-transitions-c2] live
// CSS View Transitions DOM surface (REQ-BRW-051 C2, Bao fork self-build) —
// locks over the real servo engine. The rendering half (snapshot capture and
// the `::view-transition` pseudo tree) is still a registered seam, so these
// tests lock the *API and state machine* contract, including the documented
// degraded path: a transition that cannot capture is skipped with an
// `InvalidStateError`, the update callback still runs, `updateCallbackDone`
// and `finished` settle, and `document.activeViewTransition` is cleared.
//
// When the capture seam is wired the `ready` assertions flip from
// "rejects InvalidStateError" to "fulfils"; the rest of this file is
// expectation-neutral.
//
// Gated like the rest of the live servo suite:
//   BAO_TEST_NETWORK=1 xvfb-run cargo nt -p bao-browser -E 'test(view_transition)'

#![allow(dead_code)]

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use bao_browser::{BaoConfig, BrowserRuntime, PageConfig, PageHandle};

fn serializer() -> &'static Mutex<()> {
    static SERIALIZER: OnceLock<Mutex<()>> = OnceLock::new();
    SERIALIZER.get_or_init(|| Mutex::new(()))
}

fn data_url(html: &str) -> String {
    let mut encoded = String::with_capacity(html.len() * 3);
    for b in html.bytes() {
        match b {
            b'#' => encoded.push_str("%23"),
            b'%' => encoded.push_str("%25"),
            b'\n' => encoded.push_str("%0A"),
            b'\r' => encoded.push_str("%0D"),
            b' ' => encoded.push_str("%20"),
            b'"' => encoded.push_str("%22"),
            b'\'' => encoded.push_str("%27"),
            _ => encoded.push(b as char),
        }
    }
    format!("data:text/html;charset=utf-8,{}", encoded)
}

fn wait_for_load(page: &PageHandle, max_ms: u64) {
    let start = Instant::now();
    while start.elapsed().as_millis() < max_ms as u128 {
        if page
            .evaluate_js("document.readyState")
            .map(|s| s.contains("complete") || s.contains("interactive"))
            .unwrap_or(false)
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn poll_until_contains(page: &PageHandle, js: &str, target: &str, max_ms: u64) -> String {
    let start = Instant::now();
    let mut last = String::new();
    while start.elapsed().as_millis() < max_ms as u128 {
        if let Ok(s) = page.evaluate_js_web(js) {
            if s.contains(target) {
                return s;
            }
            last = s;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    last
}

/// Returns the runtime alongside the page: `BrowserRuntime::drop` runs
/// `close_all()`, so the runtime must outlive every page interaction.
fn make_page(html: &str) -> (BrowserRuntime, PageHandle) {
    let runtime = BrowserRuntime::new(BaoConfig::default())
        .expect("gated live test: BrowserRuntime::new must succeed");
    let page = runtime
        .create_page(&PageConfig {
            url: Some(data_url(html)),
            ..Default::default()
        })
        .expect("gated live test: create_page must succeed");
    wait_for_load(&page, 10_000);
    (runtime, page)
}

const PAGE: &str = r#"<!DOCTYPE html><html><body>
<div id="target" style="width:100px;height:100px;background:blue"></div>
</body></html>"#;

/// @trace REQ-BRW-051 [criterion:view-transitions-api-surface] live
///
/// The exposed surface: `document.startViewTransition`, `activeViewTransition`,
/// the three promises, `skipTransition`, the `types` set and `waitUntil`.
#[ignore = "需要 DISPLAY: servo WebRender GL-based(headless 亦需),scripts/local-smoke.sh:97 Xvfb :99 配方; 真跑: pgrep -x Xvfb || Xvfb :99 -screen 0 1280x720x24 -ac -nolisten tcp & 后 DISPLAY=:99 cargo nextest run --run-ignored=all -p bao-browser"]
#[test]
fn view_transition_api_surface() {
    let _guard = serializer().lock().unwrap_or_else(|e| e.into_inner());
    let (_runtime, page) = make_page(PAGE);

    let result = page.evaluate_js_web(
        r#"
        var out = [];
        function check(name, cond) { out.push((cond ? 'ok:' : 'FAIL:') + name); }

        check('startViewTransition', typeof document.startViewTransition === 'function');
        check('activeViewTransition.null', document.activeViewTransition === null);

        var transition = document.startViewTransition();
        check('activeViewTransition.isObject', typeof transition === 'object' && transition !== null);
        check('activeViewTransition.identity', document.activeViewTransition === transition);
        check('has.ready', transition.ready instanceof Promise);
        check('has.finished', transition.finished instanceof Promise);
        check('has.updateCallbackDone', transition.updateCallbackDone instanceof Promise);
        check('has.skipTransition', typeof transition.skipTransition === 'function');
        check('has.waitUntil', typeof transition.waitUntil === 'function');
        check('types.isSetlike', typeof transition.types.has === 'function' &&
            typeof transition.types.add === 'function' &&
            typeof transition.types.delete === 'function' &&
            typeof transition.types.size === 'number');
        check('types.empty', transition.types.size === 0);
        check('types.addReturnsSelf',
            transition.types.add('probe') === transition.types &&
            transition.types.has('probe') &&
            transition.types.size === 1);
        check('types.delete', transition.types.delete('probe') && transition.types.size === 0);

        // Level 1 interface shape: the three promises are exposed as
        // Promise<undefined>.
        check('skipTransition.returnsUndefined', transition.skipTransition() === undefined);

        window.__surface = out.join(';');
        "String(window.__surface) || 'missing'",
    );

    let payload = result.expect("evaluate must succeed");
    assert!(
        !payload.contains("FAIL:") && payload.contains("ok:skipTransition.returnsUndefined"),
        "view transition surface mismatches: {payload}"
    );
}

/// @trace REQ-BRW-051 [criterion:view-transitions-state-machine] live
///
/// The phase machine and promise ordering: the update callback always runs,
/// `updateCallbackDone` settles first, `activeViewTransition` is cleared, and
/// the degraded (no-capture) path rejects `ready` with `InvalidStateError`
/// while still settling `finished`.
#[ignore = "需要 DISPLAY: servo WebRender GL-based(headless 亦需),scripts/local-smoke.sh:97 Xvfb :99 配方; 真跑: pgrep -x Xvfb || Xvfb :99 -screen 0 1280x720x24 -ac -nolisten tcp & 后 DISPLAY=:99 cargo nextest run --run-ignored=all -p bao-browser"]
#[test]
fn view_transition_state_machine() {
    let _guard = serializer().lock().unwrap_or_else(|e| e.into_inner());
    let (_runtime, page) = make_page(PAGE);

    page.evaluate_js_web(
        r#"
        window.__vt = { calls: 0, active: null, ready: null, updateDone: null, finished: null,
                        order: [] };
        var state = window.__vt;
        var transition = document.startViewTransition(function () {
            state.calls += 1;
            state.active = document.activeViewTransition;
        });
        transition.updateCallbackDone.then(function () { state.order.push('updateCallbackDone'); });
        transition.ready.then(
            function () { state.order.push('ready-fulfilled'); },
            function (reason) {
                state.order.push('ready-rejected');
                state.ready = String(reason && reason.name);
            });
        transition.finished.then(function () { state.order.push('finished'); });
        "scheduled"
        "#,
    )
    .expect("evaluate must succeed");

    let report = poll_until_contains(
        &page,
        "JSON.stringify({ calls: window.__vt.calls, ready: window.__vt.ready, \
          order: window.__vt.order, active: String(document.activeViewTransition) })",
        "finished",
        10_000,
    );

    assert!(
        report.contains("\"calls\":1"),
        "the update callback must run exactly once: {report}"
    );
    // Documented degraded path: no capture interface => the transition is
    // skipped with InvalidStateError before the callback runs.
    assert!(
        report.contains("InvalidStateError"),
        "ready must reject with InvalidStateError while capture is unwired: {report}"
    );
    assert!(
        report.contains("\"active\":\"null\""),
        "activeViewTransition must be cleared once the transition ends: {report}"
    );
    assert!(
        report.contains("ready-rejected") && report.contains("updateCallbackDone"),
        "updateCallbackDone must settle: {report}"
    );
    let ready_at = report.find("ready-rejected").unwrap_or(usize::MAX);
    let finished_at = report.find("finished").unwrap_or(usize::MAX);
    assert!(
        ready_at < finished_at,
        "ready must settle before finished: {report}"
    );
}

/// @trace REQ-BRW-051 [criterion:view-transitions-skip] live
///
/// `skipTransition()`: synchronous clearing of `activeViewTransition`, an
/// `AbortError` on `ready`, and `finished` settling with `updateCallbackDone`
/// (never with the skip reason).
#[ignore = "需要 DISPLAY: servo WebRender GL-based(headless 亦需),scripts/local-smoke.sh:97 Xvfb :99 配方; 真跑: pgrep -x Xvfb || Xvfb :99 -screen 0 1280x720x24 -ac -nolisten tcp & 后 DISPLAY=:99 cargo nextest run --run-ignored=all -p bao-browser"]
#[test]
fn view_transition_skip_and_recovery() {
    let _guard = serializer().lock().unwrap_or_else(|e| e.into_inner());
    let (_runtime, page) = make_page(PAGE);

    let result = page.evaluate_js_web(
        r#"
        var out = [];
        function check(name, cond) { out.push((cond ? 'ok:' : 'FAIL:') + name); }

        // skipTransition() before the rendering opportunity: the active slot
        // clears synchronously and ready rejects with AbortError.
        var first = document.startViewTransition(function () {});
        var firstReady = null;
        first.ready.catch(function (reason) { firstReady = String(reason && reason.name); });
        first.skipTransition();
        check('skip.clearsActive', document.activeViewTransition === null);
        check('skip.doneIsNoop', first.skipTransition() === undefined);

        // A second transition while one is active skips the first with
        // AbortError and takes over the active slot.
        var a = document.startViewTransition(function () {});
        var aReady = null;
        a.ready.catch(function (reason) { aReady = String(reason && reason.name); });
        var b = document.startViewTransition(function () {});
        check('supersede.takesOver', document.activeViewTransition === b);
        window.__skipSecond = { aReady: function () { return aReady; }, firstReady: function () { return firstReady; } };
        "armed"
        "#,
    );
    assert!(result.is_ok(), "skip setup must not throw: {result:?}");

    let report = poll_until_contains(
        &page,
        "JSON.stringify({ a: window.__skipSecond.aReady(), f: window.__skipSecond.firstReady() })",
        "AbortError",
        10_000,
    );
    assert!(
        report.contains("\"f\":\"AbortError\""),
        "skipTransition must reject ready with AbortError: {report}"
    );
    assert!(
        report.contains("\"a\":\"AbortError\""),
        "starting a transition over an active one must skip it with AbortError: {report}"
    );
}

/// @trace REQ-BRW-051 [criterion:view-transitions-options-and-rejection] live
///
/// Argument conversion (`StartViewTransitionOptions` dictionary and the
/// callback form), rejection propagation into `finished`, and the TypeError
/// for a value that is neither.
#[ignore = "需要 DISPLAY: servo WebRender GL-based(headless 亦需),scripts/local-smoke.sh:97 Xvfb :99 配方; 真跑: pgrep -x Xvfb || Xvfb :99 -screen 0 1280x720x24 -ac -nolisten tcp & 后 DISPLAY=:99 cargo nextest run --run-ignored=all -p bao-browser"]
#[test]
fn view_transition_options_and_rejection() {
    let _guard = serializer().lock().unwrap_or_else(|e| e.into_inner());
    let (_runtime, page) = make_page(PAGE);

    let result = page.evaluate_js_web(
        r#"
        var out = [];
        function check(name, cond) { out.push((cond ? 'ok:' : 'FAIL:') + name); }

        // Dictionary form: update callback + types.
        var called = 0;
        var withTypes = document.startViewTransition({
            update: function () { called += 1; },
            types: ['slide', 'fade'],
        });
        check('options.update', typeof withTypes === 'object');
        check('options.types.size', withTypes.types.size === 2);
        check('options.types.has', withTypes.types.has('slide') && withTypes.types.has('fade'));
        // Setlike iteration: `values()` is the generated iterator entry point.
        var typesIterator = withTypes.types.values();
        var firstType = typesIterator.next();
        var secondType = typesIterator.next();
        check('options.types.iterable',
            !firstType.done && firstType.value === 'slide' &&
            !secondType.done && secondType.value === 'fade');

        // Union conversion failure: not a callback and not an object.
        var threw = null;
        try { document.startViewTransition(42); } catch (e) { threw = String(e && e.name); }
        check('options.nonObjectThrows', threw === 'TypeError');

        // Rejection propagation: the callback's rejection reaches
        // updateCallbackDone and finished (never ready's skip reason).
        var boom = new Error('boom');
        var rejected = document.startViewTransition(function () { return Promise.reject(boom); });
        var seen = { update: null, finished: null, ready: null };
        rejected.updateCallbackDone.catch(function (reason) { seen.update = reason; });
        rejected.finished.catch(function (reason) { seen.finished = reason; });
        rejected.ready.catch(function (reason) { seen.ready = String(reason && reason.name); });

        window.__options = { out: out.join(';'),
                             probe: function () {
                                 return JSON.stringify({
                                     called: called,
                                     updateIsBoom: seen.update === boom,
                                     finishedIsBoom: seen.finished === boom,
                                     ready: seen.ready,
                                 });
                             } };
        "armed"
        "#,
    );
    assert!(result.is_ok(), "options setup must not throw: {result:?}");

    let payload = page
        .evaluate_js_web("window.__options.out")
        .expect("evaluate must succeed");
    assert!(
        !payload.contains("FAIL:") && payload.contains("ok:options.types.iterable"),
        "view transition options mismatches: {payload}"
    );

    let report = poll_until_contains(
        &page,
        "window.__options.probe()",
        "\"finishedIsBoom\":true",
        10_000,
    );
    assert!(
        report.contains("\"called\":1"),
        "dictionary update callback must run exactly once: {report}"
    );
    assert!(
        report.contains("\"updateIsBoom\":true") && report.contains("\"finishedIsBoom\":true"),
        "a rejected update callback must reject both updateCallbackDone and finished: {report}"
    );
    assert!(
        report.contains("InvalidStateError"),
        "ready keeps the skip reason while capture is unwired: {report}"
    );
}
