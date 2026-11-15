// @trace REQ-BRW-052 [level:e2e]
// @trace REQ-BRW-052 [criterion:1] [criterion:2] [criterion:3] [criterion:5] [criterion:6]
//
// ═══════════════════════════════════════════════════════════════════════════
// REQ-BRW-052 — 页面渲染模式运行时切换 (Full / Background / DomOnly)
// ═══════════════════════════════════════════════════════════════════════════
//
// **断言面**(任务契约逐条):
//   1. 枚举/配置面:PageRenderMode 默认 Full、PageConfig.render_mode 缺省
//      Full、创建时按配置落地(AC1);
//   2. Background:document.visibilityState === "hidden" + visibilitychange
//      派发 + rAF 回调不执行(AC2,servo 侧 W1 通道)+ fetch 不受影响;
//   3. DomOnly:getBoundingClientRect().width === 0(几何 fail-closed 零矩形)
//      + DOM 创建/查询 + JS 事件正常(AC3,servo 侧 W1 通道);
//   4. DomOnly→Full 几何恢复非零;Full→Background→Full 可见性恢复
//      visible(AC5 双向恢复);
//   5. 幂等:同值切换 = no-op;关闭页(状态随 WebView 回收)= 读取回默认
//      Full、切换 fail-closed 报错(AC5/AC6)。
//
// **依赖标注纪律**:visibilityState / visibilitychange / rAF 节流 / 布局抑制
// 零几何由 servo 侧 REQ-BRW-052 W1 通道(WebView::hidden +
// set_layout_suppressed + ScriptThreadMessage::SetDocumentVisible /
// SetThrottled / SetLayoutSuppressed)承载;这些断言在 servo W1 落地前
// `#[ignore = "pending servo REQ-BRW-052 W1"]`,禁以 bao 侧影子断言冒充。
// 不经 W1 语义即可成立的面(枚举/配置/幂等/创建落地/关闭回收/drift 修复/
// Background 下 fetch 不受影响)保持可执行。
//
// **运行约束**:每 #[test] 自建 BrowserRuntime(nextest 每 test 独立进程
// 隔离;plain cargo test 须 --test-threads=1)。等待闭包逐轮 evaluate noop
// 驱动 servo 泵(禁纯 sleep 等待)。

use bao_browser::{
    BaoConfig, BrowserRuntime, PageConfig, PageHandle, PagePool, PageRenderMode, PageState,
};
use std::time::{Duration, Instant};

// ─── fixture helpers ─────────────────────────────────────────────────────────

/// data: URL 包装('#' 转义 %23 — data URL fragment 保留字)。
fn data_url(html: &str) -> String {
    let encoded = html.replace('#', "%23");
    format!("data:text/html;charset=utf-8,{}", encoded)
}

/// 单页 fixture:创建 page + 泵驱动等 load 完成(同 layout_geometry_tests
/// 惯例,禁纯 sleep)。
///
/// 显式 stealth-free:本文件是渲染模式真值断言(几何/可见性),默认身份
/// (裁决 2026-10-21)会给默认页装 clientrects 噪声钩子,与几何判据叠加会
/// 引入非本域变量,故显式退出。
fn render_mode_page(pool: &PagePool, html: &str, mode: PageRenderMode) -> PageHandle {
    let page = pool
        .create_page(&PageConfig {
            url: Some(data_url(html)),
            stealth_disabled: true,
            render_mode: mode,
            ..Default::default()
        })
        .expect("pool.create_page failed");
    wait_for_load_and_drain(&page, 20_000);
    page
}

/// 等待 Page 进入 Interactive/Idle;每轮 noop evaluate 驱动 servo 泵。
fn wait_for_load_and_drain(page: &PageHandle, max_ms: u64) {
    let start = Instant::now();
    while start.elapsed().as_millis() < max_ms as u128 {
        let _ = page.evaluate_js_web("");
        if matches!(page.get_state(), PageState::Interactive | PageState::Idle) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// evaluate 并归一化返回值(去空白/去 JSON 引号),失败显式 panic。
fn eval(page: &PageHandle, script: &str) -> String {
    let raw = page
        .evaluate_js_web(script)
        .unwrap_or_else(|e| panic!("evaluate failed: {script} => {e}"));
    raw.trim().trim_matches('"').to_string()
}

/// 泵驱动等待谓词成立:每轮 evaluate noop 驱动 servo 泵,禁纯 sleep。
fn pump_until(
    page: &PageHandle,
    max_ms: u64,
    mut predicate: impl FnMut(&PageHandle) -> bool,
) -> bool {
    let start = Instant::now();
    while start.elapsed().as_millis() < max_ms as u128 {
        if predicate(page) {
            return true;
        }
        let _ = page.evaluate_js_web("");
        std::thread::sleep(Duration::from_millis(5));
    }
    predicate(page)
}

const BOX_HTML: &str = concat!(
    "<!DOCTYPE html><html><head><style>",
    "html,body{margin:0;padding:0}",
    "#box{width:200px;height:120px;background:#123456}",
    "</style></head><body><div id=\"box\"></div><div id=\"host\"></div></body></html>",
);

// ═══════════════════════════════════════════════════════════════════════════
// 1. 枚举 / 配置面(AC1,无 servo 运行时依赖)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BRW-052 [criterion:1] [level:unit]
fn page_render_mode_default_and_copy_contract() {
    // 缺省 = Full(向后兼容:既有构造零迁移)。
    assert_eq!(PageRenderMode::default(), PageRenderMode::Full);
    // 三态可区分且 Copy/Eq(运行时切换的状态机需要)。
    let modes = [
        PageRenderMode::Full,
        PageRenderMode::Background,
        PageRenderMode::DomOnly,
    ];
    for (i, a) in modes.iter().enumerate() {
        for (j, b) in modes.iter().enumerate() {
            if i == j {
                assert_eq!(a, b);
            } else {
                assert_ne!(a, b, "{a:?} must differ from {b:?}");
            }
        }
    }
    let copied = PageRenderMode::DomOnly;
    assert_eq!(copied, PageRenderMode::DomOnly);
    assert_eq!(format!("{:?}", PageRenderMode::Background), "Background");
}

#[test]
// @trace REQ-BRW-052 [criterion:1] [level:unit]
fn page_config_render_mode_defaults_full() {
    assert_eq!(PageConfig::default().render_mode, PageRenderMode::Full);
    let explicit = PageConfig {
        render_mode: PageRenderMode::DomOnly,
        ..Default::default()
    };
    assert_eq!(explicit.render_mode, PageRenderMode::DomOnly);
}

// ═══════════════════════════════════════════════════════════════════════════
// 2. 创建落地 / 幂等 / 关闭回收(AC1/AC5/AC6,不经 servo W1 语义)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BRW-052 [criterion:1] [criterion:6] [level:e2e]
fn create_page_defaults_to_full_render_mode() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let page = render_mode_page(runtime.page_pool(), BOX_HTML, PageRenderMode::Full);
    // AC6: the read-back is re-verified against servo's visibility bit.
    assert_eq!(page.render_mode(), PageRenderMode::Full);
}

#[test]
// @trace REQ-BRW-052 [criterion:1] [criterion:6] [level:e2e]
fn create_page_applies_configured_render_mode() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let pool = runtime.page_pool();

    // Background: hidden at creation (layout still live).
    let bg = render_mode_page(pool, BOX_HTML, PageRenderMode::Background);
    assert_eq!(bg.render_mode(), PageRenderMode::Background);

    // DomOnly: hidden + layout suppressed at creation (the visibility half
    // is directly observable through hidden(); the suppression half's zero
    // geometry is the servo-W1 assertion below).
    let dom = render_mode_page(pool, BOX_HTML, PageRenderMode::DomOnly);
    assert_eq!(dom.render_mode(), PageRenderMode::DomOnly);
}

#[test]
// @trace REQ-BRW-052 [criterion:1] [level:e2e]
fn set_render_mode_is_idempotent() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let page = render_mode_page(runtime.page_pool(), BOX_HTML, PageRenderMode::Full);

    // same → same is a no-op on every mode (and must not error).
    for mode in [
        PageRenderMode::Full,
        PageRenderMode::Background,
        PageRenderMode::DomOnly,
    ] {
        page.set_render_mode(mode)
            .unwrap_or_else(|e| panic!("set_render_mode({mode:?}) failed: {e}"));
        assert_eq!(page.render_mode(), mode);
        // Second identical set: still Ok, mode unchanged (idempotent arm).
        page.set_render_mode(mode)
            .unwrap_or_else(|e| panic!("idempotent set_render_mode({mode:?}) failed: {e}"));
        assert_eq!(page.render_mode(), mode);
    }
}

#[test]
// @trace REQ-BRW-052 [criterion:1] [criterion:5] [level:e2e]
fn closed_page_reclaims_render_mode_and_fails_closed() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let page = render_mode_page(runtime.page_pool(), BOX_HTML, PageRenderMode::Background);
    assert_eq!(page.render_mode(), PageRenderMode::Background);

    page.close().expect("page.close failed");

    // AC5: the mode state is reclaimed with the WebView — a closed page
    // reports the pre-creation default, never a stale stored mode.
    assert_eq!(page.render_mode(), PageRenderMode::Full);
    // AC6 fail-closed: there is no WebView left to command, so switching
    // reports an error instead of silently storing a shadow flag.
    let err = page
        .set_render_mode(PageRenderMode::DomOnly)
        .expect_err("set_render_mode on a closed page must fail closed");
    assert!(
        err.to_string().contains("page is closed"),
        "unexpected closed-page error: {err}"
    );
}

#[test]
// @trace REQ-BRW-052 [criterion:6] [level:e2e]
fn render_mode_corrects_to_engine_truth_instead_of_lying() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let page = render_mode_page(runtime.page_pool(), BOX_HTML, PageRenderMode::Full);

    // Drift #1: the stored mode claims Full while the engine is hidden from
    // the earlier Background transition (a shadow-state corruption).
    page.set_render_mode(PageRenderMode::Background)
        .expect("Full -> Background failed");
    page.webview_state().borrow_mut().render_mode = PageRenderMode::Full;

    // AC6: the getter trusts servo's hidden() bit — the real state wins and
    // the stored mode is corrected (loudly logged, never silently reported).
    assert_eq!(page.render_mode(), PageRenderMode::Background);
    // The correction persisted: the next read takes the consistent path and
    // the same-value transition is now a truthful no-op.
    assert_eq!(page.render_mode(), PageRenderMode::Background);
    page.set_render_mode(PageRenderMode::Background)
        .expect("idempotent Background set after correction failed");

    // Drift #2 (opposite direction): stored claims Background while the
    // engine is visible again.
    page.set_render_mode(PageRenderMode::Full)
        .expect("Background -> Full failed");
    page.webview_state().borrow_mut().render_mode = PageRenderMode::Background;
    assert_eq!(page.render_mode(), PageRenderMode::Full);
    assert_eq!(page.render_mode(), PageRenderMode::Full);

    // The corrected page accepts fresh transitions (engine state usable).
    page.set_render_mode(PageRenderMode::DomOnly)
        .expect("Full -> DomOnly after correction failed");
    assert_eq!(page.render_mode(), PageRenderMode::DomOnly);
}

// ═══════════════════════════════════════════════════════════════════════════
// 3. Background — fetch 不受影响(AC2 的传输面,不经 servo W1 通道)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BRW-052 [criterion:2] [level:e2e]
fn background_render_mode_keeps_fetch_working() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let page = render_mode_page(runtime.page_pool(), BOX_HTML, PageRenderMode::Full);

    page.set_render_mode(PageRenderMode::Background)
        .expect("Full -> Background failed");

    // Background hides the WebView; the network stack is orthogonal to the
    // paint face, so a page-realm fetch must still complete.
    eval(
        &page,
        "window.__rmFetch='PENDING';\
         fetch('data:text/plain;charset=utf-8,bao-render-mode').then(function(r){return r.text();})\
           .then(function(t){window.__rmFetch=t;})\
           .catch(function(e){window.__rmFetch='ERR:'+e;});\
         'started'",
    );
    let ok = pump_until(&page, 20_000, |p| {
        eval(p, "String(window.__rmFetch || '')") == "bao-render-mode"
    });
    assert!(
        ok,
        "fetch must complete in Background mode, got: {}",
        eval(&page, "String(window.__rmFetch || '')")
    );
    // Timers stay independent of the render mode as well.
    assert!(
        eval(&page, "String(typeof setTimeout === 'function')") == "true",
        "timers must stay installed in Background mode"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 4. Background — visibilityState / visibilitychange / rAF 节流
//    (AC2;servo 侧 W1 通道未落地前 ignore,禁 bao 侧影子断言冒充)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BRW-052 [criterion:2] [level:e2e]
fn background_sets_visibility_hidden_and_pauses_raf() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let page = render_mode_page(runtime.page_pool(), BOX_HTML, PageRenderMode::Full);

    // Arm the observers BEFORE the switch so no edge can be missed.
    eval(
        &page,
        "window.__rmVC=[];\
         document.addEventListener('visibilitychange',function(){\
           window.__rmVC.push(document.visibilityState);});\
         window.__rmRafRan=false;\
         'armed'",
    );
    assert_eq!(eval(&page, "String(document.visibilityState)"), "visible");

    page.set_render_mode(PageRenderMode::Background)
        .expect("Full -> Background failed");

    // AC2: the engine visibility state flips to hidden and the event fires.
    let hidden = pump_until(&page, 10_000, |p| {
        eval(p, "String(document.visibilityState)") == "hidden"
    });
    assert!(hidden, "document.visibilityState must become 'hidden'");
    let change_seen = pump_until(&page, 10_000, |p| {
        eval(p, "String(window.__rmVC.indexOf('hidden') !== -1)") == "true"
    });
    assert!(
        change_seen,
        "visibilitychange must be dispatched with hidden, got: {}",
        eval(&page, "String(JSON.stringify(window.__rmVC))")
    );

    // AC2: rAF callbacks do not execute while Background-throttled.
    eval(
        &page,
        "requestAnimationFrame(function(){window.__rmRafRan=true;}); 'queued'",
    );
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(300) {
        let _ = page.evaluate_js_web("");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        eval(&page, "String(window.__rmRafRan)"),
        "false",
        "rAF must be paused in Background mode"
    );
}

#[test]
// @trace REQ-BRW-052 [criterion:2] [criterion:5] [level:e2e]
fn full_background_full_restores_visibility() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let page = render_mode_page(runtime.page_pool(), BOX_HTML, PageRenderMode::Full);
    assert_eq!(eval(&page, "String(document.visibilityState)"), "visible");

    page.set_render_mode(PageRenderMode::Background)
        .expect("Full -> Background failed");
    assert!(
        pump_until(&page, 10_000, |p| eval(
            p,
            "String(document.visibilityState)"
        ) == "hidden"),
        "Background must report hidden"
    );

    page.set_render_mode(PageRenderMode::Full)
        .expect("Background -> Full failed");
    assert!(
        pump_until(&page, 10_000, |p| eval(
            p,
            "String(document.visibilityState)"
        ) == "visible"),
        "Full round-trip must restore visible"
    );
    assert_eq!(page.render_mode(), PageRenderMode::Full);

    // rAF resumes after the round trip (the Full frame driver is live again).
    eval(
        &page,
        "window.__rmRafResumed=false;\
         requestAnimationFrame(function(){window.__rmRafResumed=true;}); 'queued'",
    );
    assert!(
        pump_until(&page, 10_000, |p| eval(
            p,
            "String(window.__rmRafResumed)"
        ) == "true"),
        "rAF must resume after returning to Full"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// 5. DomOnly — 零几何 + DOM/JS/事件正常 + 切回 Full 几何恢复
//    (AC3/AC5;servo 侧 W1 通道未落地前 ignore)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BRW-052 [criterion:3] [level:e2e]
fn dom_only_zero_geometry_with_working_dom_and_events() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let page = render_mode_page(runtime.page_pool(), BOX_HTML, PageRenderMode::Full);

    // Baseline: real geometry exists in Full mode.
    assert!(
        pump_until(&page, 10_000, |p| {
            eval(
                p,
                "String(document.getElementById('box').getBoundingClientRect().width > 0)",
            ) == "true"
        }),
        "baseline geometry must exist before switching"
    );

    page.set_render_mode(PageRenderMode::DomOnly)
        .expect("Full -> DomOnly failed");

    // AC3: geometry fails closed to zero rects (no layout result).
    assert!(
        pump_until(&page, 10_000, |p| {
            eval(
                p,
                "String(document.getElementById('box').getBoundingClientRect().width === 0)",
            ) == "true"
        }),
        "DomOnly must expose zero geometry, got: {}",
        eval(
            &page,
            "String(document.getElementById('box').getBoundingClientRect().width)"
        )
    );

    // DOM creation / query / JS events stay fully functional.
    assert_eq!(
        eval(
            &page,
            "(function(){\
               var d=document.createElement('div'); d.id='made'; d.textContent='hello';\
               document.getElementById('host').appendChild(d);\
               return String(document.getElementById('made').textContent);\
             })()"
        ),
        "hello",
        "DOM creation/query must work in DomOnly mode"
    );
    assert_eq!(
        eval(
            &page,
            "(function(){\
               var seen='';\
               var host=document.getElementById('host');\
               host.addEventListener('rm-probe',function(e){seen=e.detail;});\
               host.dispatchEvent(new CustomEvent('rm-probe',{detail:'fired'}));\
               return String(seen);\
             })()"
        ),
        "fired",
        "JS events must dispatch in DomOnly mode"
    );
    // Network face stays live as well (fetch in DomOnly mode).
    eval(
        &page,
        "window.__rmDomFetch='PENDING';\
         fetch('data:text/plain;charset=utf-8,dom-only').then(function(r){return r.text();})\
           .then(function(t){window.__rmDomFetch=t;})\
           .catch(function(e){window.__rmDomFetch='ERR:'+e;}); 'started'",
    );
    assert!(
        pump_until(&page, 20_000, |p| eval(
            p,
            "String(window.__rmDomFetch || '')"
        ) == "dom-only"),
        "fetch must complete in DomOnly mode, got: {}",
        eval(&page, "String(window.__rmDomFetch || '')")
    );
}

#[test]
// @trace REQ-BRW-052 [criterion:3] [criterion:5] [level:e2e]
fn dom_only_to_full_restores_geometry() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let page = render_mode_page(runtime.page_pool(), BOX_HTML, PageRenderMode::Full);

    page.set_render_mode(PageRenderMode::DomOnly)
        .expect("Full -> DomOnly failed");
    assert!(
        pump_until(&page, 10_000, |p| {
            eval(
                p,
                "String(document.getElementById('box').getBoundingClientRect().width === 0)",
            ) == "true"
        }),
        "DomOnly must expose zero geometry"
    );

    // AC3/AC5: the Full return triggers the one reflow that re-materializes
    // layout — geometry must come back non-zero without a reload.
    page.set_render_mode(PageRenderMode::Full)
        .expect("DomOnly -> Full failed");
    let width = pump_until(&page, 10_000, |p| {
        eval(
            p,
            "String(document.getElementById('box').getBoundingClientRect().width)",
        ) != "0"
    });
    assert!(
        width,
        "geometry must be restored after DomOnly -> Full, got width: {}",
        eval(
            &page,
            "String(document.getElementById('box').getBoundingClientRect().width)"
        )
    );
    assert_eq!(
        eval(
            &page,
            "String(Math.round(document.getElementById('box').getBoundingClientRect().width))",
        ),
        "200",
        "restored geometry must equal the laid-out width (200px)"
    );
    assert_eq!(page.render_mode(), PageRenderMode::Full);

    // DomOnly -> Background: layout restored, still hidden.
    page.set_render_mode(PageRenderMode::DomOnly)
        .expect("Full -> DomOnly failed");
    page.set_render_mode(PageRenderMode::Background)
        .expect("DomOnly -> Background failed");
    assert_eq!(page.render_mode(), PageRenderMode::Background);
}
