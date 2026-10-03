// REQ-CDP-004 real 路径投递探针 e2e:n 页并发导航 + worker 面板的高事件
// 负载下,delegate 发射数 == 泵消费数(零丢帧事件),且每页帧事件零丢失、
// 帧序列保序。通道 = 无界可靠队列(servo delegate 回调与泵同线程,阻塞
// 形态被线程模型排除——见 delegate.rs 通道节注释)。
// @trace REQ-CDP-004 [req:REQ-CDP-004] [level:e2e]
// @trace REQ-CDP-006 [entity:ServoDelegateHooks]

use std::collections::HashMap;
use std::time::{Duration, Instant};

use bao_browser::{
    servo_event_emitted_total, servo_event_pumped_total, BaoConfig, BrowserRuntime, PageConfig,
};
use bao_cdp_client::bridge::ServoEvent;

const PAGES: usize = 4;
const ROUNDS: usize = 3;
const BURST: usize = 120;

/// Minimal pump: spin servo until the given pages all reach Complete.
/// Does NOT drain the event queue — the reliable queue absorbs the backlog
/// (the retired bounded channel would drop it), which is the load shape this
/// test drives to saturation.
fn pump_until_all_complete(
    runtime: &BrowserRuntime,
    pages: &[bao_browser::PageHandle],
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        runtime.spin_event_loop();
        if pages
            .iter()
            .all(|p| p.webview_state().borrow().load_status == servo::LoadStatus::Complete)
        {
            return true;
        }
        std::thread::yield_now();
    }
    false
}

#[test]
fn real_path_delivery_probe_emitted_equals_pumped_under_high_load() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new");
    let mut pages = Vec::with_capacity(PAGES);
    for _ in 0..PAGES {
        pages.push(
            runtime
                .create_page(&PageConfig {
                    url: None,
                    ..Default::default()
                })
                .expect("page"),
        );
    }
    // CDP target identity per page (the same id the frame events carry).
    let targets: Vec<String> = pages.iter().map(|p| p.id().to_string()).collect();
    // Real event queue (the production wiring shape — unbounded reliable mpsc).
    let (event_tx, servo_event_rx) = std::sync::mpsc::channel::<ServoEvent>();
    runtime.set_event_channel(event_tx);

    // Warmup: each page's initial about:blank load fires its Started before
    // the channel is wired but its Stopped after — absorb that straddle (and
    // any other pre-test event) with one throwaway navigation + full drain,
    // so the probe baseline below starts from a quiesced queue.
    for page in pages.iter() {
        page.navigate("data:text/html,<html><body>warmup</body></html>")
            .expect("warmup navigate");
    }
    assert!(
        pump_until_all_complete(&runtime, &pages, Duration::from_secs(60)),
        "warmup: all pages must complete their load"
    );
    let mut warmup_settle = 0u32;
    while warmup_settle < 2 {
        runtime.spin_event_loop();
        let mut drained = 0usize;
        bao_browser::drain_servo_events(&servo_event_rx, |_event| {
            drained += 1;
        });
        if drained == 0 {
            warmup_settle += 1;
        } else {
            warmup_settle = 0;
        }
        std::thread::yield_now();
    }

    let emitted_before = servo_event_emitted_total();
    let pumped_before = servo_event_pumped_total();

    // Load phase: per round, all PAGES navigate concurrently (interleaved
    // in-flight loads), then each page issues a console burst. Console events
    // are delivered through servo's embedder message queue during
    // spin_event_loop — the queue accumulates across rounds into a backlog
    // beyond the retired bounded channel's capacity (1024), exactly the load
    // shape that drop-newest used to eat.
    let worker_js = "var __w = new Worker('data:text/javascript,postMessage(7)'); \
                     __w.onmessage = function (e) { console.log('WORKER-ECHO', e.data); };";
    for round in 0..ROUNDS {
        for (i, page) in pages.iter().enumerate() {
            let url = format!(
                "data:text/html,<html><head><title>p{i}r{round}</title></head><body>round {round}</body></html>"
            );
            page.navigate(&url).expect("navigate");
        }
        assert!(
            pump_until_all_complete(&runtime, &pages, Duration::from_secs(60)),
            "round {round}: all pages must complete their load"
        );
        for (i, page) in pages.iter().enumerate() {
            page.evaluate_js_web(&format!(
                "for (var j = 0; j < {BURST}; j++) {{ console.log('p{i}r{round} burst', j); }}"
            ))
            .expect("console burst");
        }
        if round == 0 {
            pages[0].evaluate_js_web(worker_js).expect("worker spawn");
        }
    }

    // Settle phase: the run_with_bridge pump shape (spin + full drain) until
    // every expected delivery (including the worker echo) has landed and the
    // queue stays idle.
    let mut console_total = 0usize;
    let mut worker_echo = 0usize;
    let mut frame_started: HashMap<String, usize> = HashMap::new();
    let mut frame_stopped: HashMap<String, usize> = HashMap::new();
    // Per-target in-flight frame counter: Started increments, Stopped
    // decrements. Order violation = Stopped on an empty stack; loss shows up
    // as a non-zero final balance.
    let mut frame_balance: HashMap<String, i64> = HashMap::new();
    let mut order_ok = true;

    let deadline = Instant::now() + Duration::from_secs(60);
    let mut idle_spins = 0u32;
    while Instant::now() < deadline {
        runtime.spin_event_loop();
        let mut drained = 0usize;
        bao_browser::drain_servo_events(&servo_event_rx, |servo_event| {
            drained += 1;
            match servo_event {
                ServoEvent::Console { text, .. } => {
                    if text.contains("WORKER-ECHO") {
                        worker_echo += 1;
                    }
                    console_total += 1;
                }
                ServoEvent::FrameStartedLoading { target_id, .. } => {
                    *frame_started.entry(target_id.clone()).or_default() += 1;
                    *frame_balance.entry(target_id).or_default() += 1;
                }
                ServoEvent::FrameStoppedLoading { target_id, .. } => {
                    *frame_stopped.entry(target_id.clone()).or_default() += 1;
                    let bal = frame_balance.entry(target_id).or_default();
                    *bal -= 1;
                    if *bal < 0 {
                        order_ok = false;
                    }
                }
                _ => {}
            }
        });
        if drained == 0 && worker_echo >= 1 {
            idle_spins += 1;
            if idle_spins >= 64 {
                break;
            }
        } else {
            idle_spins = 0;
        }
        std::thread::yield_now();
    }

    let emitted_delta = servo_event_emitted_total() - emitted_before;
    let pumped_delta = servo_event_pumped_total() - pumped_before;

    // ── The delivery assertion (completion ②): emitted == pumped ──
    assert_eq!(
        emitted_delta, pumped_delta,
        "real-path delivery probe: every delegate emission must reach the pump"
    );
    // Substantial real load actually flowed (not a vacuous equality).
    assert!(
        pumped_delta >= (PAGES * BURST) as u64,
        "expected high-load volume, got {pumped_delta}"
    );
    // Zero-loss per page across all rounds + order preservation.
    for (i, target) in targets.iter().enumerate() {
        assert_eq!(
            frame_started.get(target).copied().unwrap_or(0),
            ROUNDS,
            "page {i}: every navigation must deliver frameStartedLoading"
        );
        assert_eq!(
            frame_stopped.get(target).copied().unwrap_or(0),
            ROUNDS,
            "page {i}: every navigation must deliver frameStoppedLoading"
        );
        assert_eq!(
            frame_balance.get(target).copied().unwrap_or(0),
            0,
            "page {i}: frame start/stop must pair up (order preserved)"
        );
    }
    assert!(order_ok, "frameStoppedLoading never precedes its start");
    assert_eq!(
        console_total,
        PAGES * ROUNDS * BURST + worker_echo,
        "every console emission must be delivered exactly once"
    );
    assert!(
        worker_echo >= 1,
        "worker panel leg: page-observed worker echo must be delivered"
    );
}
