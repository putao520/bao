//! run()/pump_cdp 泵面 CDP 事件通道接线锁(REQ-LIB-003;e18d48330 M1 残留
//! 闭合;用户 2026-10-12 裁决接线)。
//!
//! e18d48330 交付时登记的文档化边界:「run() 泵无事件通道,tap 存在但永不
//! 喂入,waitFor* 诚实超时」。本文件锁定闭合后的不变量:
//!
//! 1. **run() 泵面真活**——runtime 自有 servo 事件通道安装于 `new()`,真实
//!    页面加载的 Network.requestWillBeSent 经 泵 drain → translate →
//!    tap 观察,memory:// 客户端的 Page.waitForRequest 在 `run_until`
//!    (`run()` 同一循环体)下 resolve。
//! 2. **pump_cdp 泵面真活**——有界泵同一接线,waitForRequest 同样 resolve。
//!
//! RED(修前):`new()` 不安装事件通道 → `Network.enable` fail-closed
//! (「no runtime event channel is wired for this page」),waitFor* 无从
//! 谈起 → 本文件两测试红。
//!
//! 时序锚:导航线程由等待线程在 `Network.enable` 完成(即泵已活)后派生,
//! 同锚点 +300ms 触发真实加载——注册(waitForRequest 落 tap)先于导航事件,
//! 消除「事件先于订阅」的错过竞态(e18d48330 泵时序形态的锚定变体)。
//!
//! @trace REQ-LIB-003 [level:e2e]
//! @trace REQ-CDP-001 [level:e2e]

#[path = "common/mod.rs"]
mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bao_browser::{BaoConfig, BrowserRuntime, PageConfig};
use bao_cdp_client::browser::Browser;
use serde_json::{json, Value};

/// memory:// 客户端共享进程全局 bridge 注册表(last-writer-wins)——libtest
/// 串行下顺序无重叠,锁只为防御性互斥(与 cdp_universe_wiring_tests 同形)。
static REGISTRY_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 等待者裁决槽:waitForRequest 的结果(Network.enable 失败也走这里)。
type VerdictSlot = Arc<Mutex<Option<Result<Value, String>>>>;

fn spawn_http_fixture() -> common::http_fixture::HttpFixture {
    common::http_fixture::HttpFixture::spawn(
        "cdp-run-pump-fixture",
        Arc::new(|_path: &str, _script: Option<&str>| -> (&'static str, String) {
            ("text/plain", "run-pump-ok".to_string())
        }),
    )
}

/// 等待线程:connect → Network.enable(订阅网络事件;泵活后经桥通道应答)
/// → 同锚点派生导航线程(300ms 后真实页面加载)→ Page.waitForRequest 等待
/// URL 匹配的请求事件。裁决(含每步失败)写入 slot。
fn spawn_waiter(slot: VerdictSlot, fixture_port: u16) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let nav_url = format!("http://127.0.0.1:{fixture_port}/api/data");
        let mut browser = match Browser::connect("memory://bao") {
            Ok(b) => b,
            Err(e) => {
                *slot.lock().unwrap() = Some(Err(format!("connect: {e}")));
                return;
            }
        };
        if let Err(e) = browser.send_command("Network.enable", json!({})) {
            *slot.lock().unwrap() = Some(Err(format!("Network.enable: {e}")));
            return;
        }
        // 泵已活(Network.enable 经桥通道被泵应答)。注册者就位后 300ms 触发
        // 真实页面加载——注册先于导航事件,零错过竞态。
        let navigator = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            let mut b = Browser::connect("memory://bao").expect("navigator connect");
            b.send_command("Page.navigate", json!({ "url": nav_url }))
                .expect("navigate");
        });
        match browser.send_command(
            "Page.waitForRequest",
            json!({ "url": "**/api/*", "timeout": 15000 }),
        ) {
            Ok(v) => *slot.lock().unwrap() = Some(Ok(v)),
            Err(e) => *slot.lock().unwrap() = Some(Err(format!("waitForRequest: {e}"))),
        }
        let _ = navigator.join();
    })
}

/// run() 泵面(`run_until` = `run()` 同一循环体):waitForRequest 在真实
/// 页面加载的 translate 事件上 resolve。
///
/// @trace REQ-LIB-003 [level:e2e]
#[test]
fn run_pump_face_wait_for_request_resolves_on_real_page_load() {
    let _registry_guard = REGISTRY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let fixture = spawn_http_fixture();

    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new");
    let page = runtime
        .create_page(&PageConfig {
            url: None,
            ..Default::default()
        })
        .expect("page");
    let _page = page;

    let slot: VerdictSlot = Arc::new(Mutex::new(None));
    let waiter = spawn_waiter(slot.clone(), fixture.port);

    // run() 泵面:与 run() 同一循环体,退出条件 = 裁决落地(30s 硬上界)。
    let pump_slot = slot.clone();
    let pump_start = Instant::now();
    runtime
        .run_until(move || {
            pump_slot.lock().unwrap().is_some() || pump_start.elapsed() > Duration::from_secs(30)
        })
        .expect("run_until must return Ok");

    let verdict = slot
        .lock()
        .unwrap()
        .take()
        .expect("verdict must have landed inside the pump window");
    let params = verdict
        .map_err(|e| format!("RED pre-fix shape (Network.enable fail-closed / waitFor starved): {e}"))
        .expect("waitForRequest must resolve on the run() pump face");
    assert_eq!(
        params["request"]["url"],
        json!(format!("http://127.0.0.1:{}/api/data", fixture.port)),
        "waitForRequest must return the real page-load request event"
    );
    assert!(
        params["requestId"].is_string(),
        "event params must carry the CDP requestId, got: {params}"
    );
    assert_eq!(params["type"], "Document", "translate's real event shape");
    let _ = waiter.join();
}

/// pump_cdp 泵面:有界泵同一接线,waitForRequest 同样 resolve。
///
/// @trace REQ-LIB-003 [level:e2e]
#[test]
fn pump_cdp_face_wait_for_request_resolves_on_real_page_load() {
    let _registry_guard = REGISTRY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let fixture = spawn_http_fixture();

    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new");
    let page = runtime
        .create_page(&PageConfig {
            url: None,
            ..Default::default()
        })
        .expect("page");
    let _page = page;

    let slot: VerdictSlot = Arc::new(Mutex::new(None));
    let waiter = spawn_waiter(slot.clone(), fixture.port);

    let deadline = Instant::now() + Duration::from_secs(30);
    let verdict = loop {
        if let Some(verdict) = slot.lock().unwrap().take() {
            break verdict;
        }
        if Instant::now() > deadline {
            panic!("no waitForRequest verdict within the 30s pump_cdp window");
        }
        runtime.pump_cdp(Duration::from_millis(10));
    };
    let params = verdict
        .map_err(|e| format!("RED pre-fix shape (Network.enable fail-closed / waitFor starved): {e}"))
        .expect("waitForRequest must resolve on the pump_cdp face");
    assert_eq!(
        params["request"]["url"],
        json!(format!("http://127.0.0.1:{}/api/data", fixture.port)),
        "waitForRequest must return the real page-load request event"
    );
    assert!(
        params["requestId"].is_string(),
        "event params must carry the CDP requestId, got: {params}"
    );
    assert_eq!(params["type"], "Document", "translate's real event shape");
    let _ = waiter.join();
}

/// 手动 spin 形嵌入者契约锁(REQ-LIB-003):嵌入者自驱
/// `spin_event_loop` 循环时必须每迭代调 `pump_servo_events_to_taps` ——
/// 通道是无界可靠队列(REQ-CDP-004 立法:永不丢弃、无高水位自弃),不
/// drain 即无限积压(soak 10-12 线性泄漏的缺口本体)。锁两面:①不调
/// drain 时 emitted>pumped(积压真实存在,非 vacuous)②调 drain 后
/// emitted==pumped 且下一轮保持(不积压)。
///
/// @trace REQ-LIB-003 [level:e2e]
#[test]
fn manual_spin_embedder_drain_keeps_emitted_equal_pumped() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new");
    let page = runtime
        .create_page(&PageConfig {
            url: None,
            ..Default::default()
        })
        .expect("page");

    for round in 0..3 {
        // 事件产生面:console 输出经 per-webview delegate 落通道(手动 spin
        // 形嵌入者的真实事件流——无内置泵替它 drain)。
        page.evaluate_js_web(&format!("console.log('spin-drain-{round}');"))
            .expect("console eval");
        for _ in 0..8 {
            runtime.spin_event_loop();
        }

        if round == 0 {
            // 负对照(仅首轮):drain 之前通道里有未消费事件——积压形态
            // 真实存在(证明断言非 vacuous)。
            let emitted = bao_browser::servo_event_emitted_total();
            let pumped = bao_browser::servo_event_pumped_total();
            assert!(
                emitted > pumped,
                "precondition: undrained events must sit in the reliable queue \
                 (emitted={emitted} pumped={pumped})"
            );
        }

        // 嵌入者契约调用:每迭代 drain(translate → memory:// tap),排空到
        // 当前发射数。
        runtime.pump_servo_events_to_taps();

        assert_eq!(
            bao_browser::servo_event_emitted_total(),
            bao_browser::servo_event_pumped_total(),
            "round {round}: after the embedder drain, emitted must equal pumped \
             (no backlog survives the per-iteration drain)"
        );
    }
}
