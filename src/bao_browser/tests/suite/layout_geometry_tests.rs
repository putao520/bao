// @trace REQ-BRW-002 [level:e2e]
// @trace REQ-BRW-002-C2 [level:e2e]
// @trace REQ-BRW-002-C3 [level:e2e]
//
// ═══════════════════════════════════════════════════════════════════════════
// REQ-BRW-002 E2E — Flexbox / Grid 布局几何探针(内存渲染 C2/C3 几何面)
// ═══════════════════════════════════════════════════════════════════════════
//
// **断言面**(覆盖审计 .plans/coverage-audit-brw-20261014.md §2 P2:css_conformance
// 仅有 computed-style 门 —— flexbox 10 条 + grid 1 条 gate;布局几何面(实际
// 盒子位置)零断言)。本文件补 getBoundingClientRect 级几何断言:
//   1. flex row:子项横向排列(左→右依次排布,无换行)
//   2. flex justify-content:flex-end / center 的主轴对齐位置
//   3. flex column:子项纵向排列 + align-items 交叉轴对齐(center/flex-end)
//      + row 容器 align-items:center 的垂直居中
//   4. grid:grid-template-columns 固定轨道几何(3×2 轨道矩阵 + 容器高度)
//   5. grid gap:column-gap/row-gap 轨道间隔几何
//
// **几何通道**:getBoundingClientRect → servo `Node::border_box` →
// `box_area_query` → `layout_reflow(QueryMsg::BoxArea)` 强制同步 reflow
// (vendor/servo/components/script/dom/window/window.rs),断言的是
// LayoutThread 真实盒子几何,非样式壳。
//
// **容差**:布局引擎亚像素抖动留 1px 裕量(|actual − expected| ≤ 1.0)。
//
// **运行约束**:每 #[test] 自建 BrowserRuntime(nextest 每 test 独立进程
// 隔离;plain cargo test 须 --test-threads=1)。等待闭包逐轮 evaluate noop
// 驱动 servo 泵(禁纯 sleep 等待);body margin 归零使坐标系从 0 起。

use bao_browser::{BaoConfig, BrowserRuntime, PageConfig, PageHandle, PagePool, PageState};
use std::time::{Duration, Instant};

// ─── fixture helpers ─────────────────────────────────────────────────────────

/// data: URL 包装('#' 转义 %23 — data URL fragment 保留字)。
fn data_url(html: &str) -> String {
    let encoded = html.replace('#', "%23");
    format!("data:text/html;charset=utf-8,{}", encoded)
}

/// 等待 Page 进入 Interactive/Idle;每轮 noop evaluate 驱动 servo 泵
/// (同 servo_render_pipeline_tests::wait_for_load 惯例,禁纯 sleep)。
fn wait_for_load_and_drain(page: &PageHandle, max_ms: u64) {
    let start = Instant::now();
    while start.elapsed().as_millis() < max_ms as u128 {
        let _ = page.evaluate_js("");
        if matches!(page.get_state(), PageState::Interactive | PageState::Idle) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// 布局几何(边框盒,CSS px)。
#[derive(Debug, Clone, Copy)]
struct Rect {
    left: f64,
    top: f64,
    width: f64,
    height: f64,
}

impl Rect {
    fn right(&self) -> f64 {
        self.left + self.width
    }
}

/// 读取元素 getBoundingClientRect(getBoundingClientRect 在 servo 侧经
/// box_area_query → layout_reflow 强制同步布局;若元素缺失则显式报 MISSING)。
fn rect_of(page: &PageHandle, id: &str) -> Rect {
    let script = format!(
        "String((function(){{var e=document.getElementById('{id}');\
         if(!e){{return 'MISSING';}}\
         var r=e.getBoundingClientRect();\
         return [r.left, r.top, r.width, r.height].join(',');}})())"
    );
    let raw = page
        .evaluate_js_web(&script)
        .unwrap_or_else(|e| panic!("evaluate geometry of #{id} failed: {e}"));
    let raw = raw.trim().trim_matches('"');
    assert!(raw != "MISSING", "#{id} not in document");
    let nums: Vec<f64> = raw
        .split(',')
        .filter_map(|v| v.trim().parse::<f64>().ok())
        .collect();
    assert_eq!(nums.len(), 4, "#{id} geometry parse failed from '{raw}'");
    Rect {
        left: nums[0],
        top: nums[1],
        width: nums[2],
        height: nums[3],
    }
}

/// 泵驱动等待首元素几何物化(每轮 evaluate 探针驱动 servo 泵,禁纯 sleep)。
fn wait_geometry_ready(page: &PageHandle, id: &str, max_ms: u64) {
    let start = Instant::now();
    while start.elapsed().as_millis() < max_ms as u128 {
        let probe = page.evaluate_js_web(&format!(
            "String(document.getElementById('{id}') ? \
             document.getElementById('{id}').getBoundingClientRect().width > 0 : false)"
        ));
        if matches!(probe.as_deref(), Ok("true")) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("#{id} layout geometry never materialized within {max_ms}ms");
}

/// 单页 fixture:创建 page + 泵驱动等 load + 等首元素几何物化。
fn geometry_page(pool: &PagePool, html: &str, first_id: &str) -> PageHandle {
    let page = pool
        .create_page(&PageConfig {
            url: Some(data_url(html)),
            ..Default::default()
        })
        .expect("pool.create_page failed");
    wait_for_load_and_drain(&page, 20_000);
    wait_geometry_ready(&page, first_id, 10_000);
    page
}

/// 容差断言(±1px):逐条打印实测值(完成口径 ⑤),失败收集到 failures。
fn expect_close(failures: &mut Vec<String>, name: &str, actual: f64, expected: f64) {
    let ok = (actual - expected).abs() <= 1.0;
    let line = format!(
        "{}  {} = {} (expected {} ± 1px)",
        if ok { "PASS" } else { "FAIL" },
        name,
        actual,
        expected
    );
    eprintln!("{}", line);
    if !ok {
        failures.push(line);
    }
}

/// 逐字段容差断言整个矩形。
fn expect_rect(failures: &mut Vec<String>, id: &str, actual: Rect, expected: Rect) {
    expect_close(failures, &format!("{id}.left"), actual.left, expected.left);
    expect_close(failures, &format!("{id}.top"), actual.top, expected.top);
    expect_close(failures, &format!("{id}.width"), actual.width, expected.width);
    expect_close(failures, &format!("{id}.height"), actual.height, expected.height);
}

/// 测试收口:有任何几何断言失败即整体失败(附全部失败明细)。
fn finish(failures: &[String], what: &str) {
    if !failures.is_empty() {
        panic!(
            "{}: {} layout geometry assertion(s) failed:\n{}",
            what,
            failures.len(),
            failures.join("\n")
        );
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// 1. flex row — 子项横向排列(REQ-BRW-002-C2)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BRW-002-C2 [level:e2e]
fn flex_row_items_lay_out_horizontally() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let pool: &PagePool = runtime.page_pool();

    // 320px 容器装 3×100px 子项:flex-start 下依次 0/100/200,无换行。
    let html = concat!(
        "<!DOCTYPE html><html><head><style>",
        "html,body{margin:0;padding:0}",
        "#row{display:flex;flex-direction:row;justify-content:flex-start;width:320px}",
        ".it{width:100px;height:30px}",
        "</style></head><body>",
        "<div id=\"row\"><div class=\"it\" id=\"a\">1</div>",
        "<div class=\"it\" id=\"b\">2</div><div class=\"it\" id=\"c\">3</div></div>",
        "</body></html>"
    );
    let page = geometry_page(pool, html, "a");

    let mut failures: Vec<String> = Vec::new();

    // 容器:块级 flex 宽 320、高收缩为单行子项高 30。
    expect_rect(
        &mut failures,
        "row",
        rect_of(&page, "row"),
        Rect {
            left: 0.0,
            top: 0.0,
            width: 320.0,
            height: 30.0,
        },
    );
    // 子项横向排列:a(0,0) b(100,0) c(200,0),尺寸 100×30。
    expect_rect(
        &mut failures,
        "a",
        rect_of(&page, "a"),
        Rect {
            left: 0.0,
            top: 0.0,
            width: 100.0,
            height: 30.0,
        },
    );
    let b = rect_of(&page, "b");
    expect_close(&mut failures, "b.left", b.left, 100.0);
    expect_close(&mut failures, "b.top", b.top, 0.0);
    let c = rect_of(&page, "c");
    expect_close(&mut failures, "c.left", c.left, 200.0);
    expect_close(&mut failures, "c.right(<=320,不换行)", c.right(), 300.0);

    finish(&failures, "flex row horizontal arrangement");
}

// ═══════════════════════════════════════════════════════════════════════════
// 2. flex justify-content — 主轴对齐位置(REQ-BRW-002-C2)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BRW-002-C2 [level:e2e]
fn flex_justify_content_places_items_along_main_axis() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let pool: &PagePool = runtime.page_pool();

    // 320px 容器装 300px 内容(3×100):自由空间 20px。
    // flex-end → 全部右移 20;center → 左右各分 10。
    let html = concat!(
        "<!DOCTYPE html><html><head><style>",
        "html,body{margin:0;padding:0}",
        "#end{display:flex;justify-content:flex-end;width:320px}",
        "#center{display:flex;justify-content:center;width:320px}",
        ".it{width:100px;height:30px}",
        "</style></head><body>",
        "<div id=\"end\"><div class=\"it\" id=\"e1\">1</div>",
        "<div class=\"it\" id=\"e2\">2</div><div class=\"it\" id=\"e3\">3</div></div>",
        "<div id=\"center\"><div class=\"it\" id=\"m1\">1</div>",
        "<div class=\"it\" id=\"m2\">2</div><div class=\"it\" id=\"m3\">3</div></div>",
        "</body></html>"
    );
    let page = geometry_page(pool, html, "e1");

    let mut failures: Vec<String> = Vec::new();

    // flex-end:子项贴容器右缘(e3.right = 320),整体左移入 20px 自由空间。
    expect_close(&mut failures, "end/e1.left", rect_of(&page, "e1").left, 20.0);
    expect_close(&mut failures, "end/e2.left", rect_of(&page, "e2").left, 120.0);
    expect_close(
        &mut failures,
        "end/e3.right",
        rect_of(&page, "e3").right(),
        320.0,
    );

    // center:两侧各 10px。
    expect_close(&mut failures, "center/m1.left", rect_of(&page, "m1").left, 10.0);
    expect_close(&mut failures, "center/m2.left", rect_of(&page, "m2").left, 110.0);
    expect_close(
        &mut failures,
        "center/m3.right",
        rect_of(&page, "m3").right(),
        310.0,
    );

    finish(&failures, "flex justify-content main-axis alignment");
}

// ═══════════════════════════════════════════════════════════════════════════
// 3. flex column + align-items — 纵向排列与交叉轴对齐(REQ-BRW-002-C2)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BRW-002-C2 [level:e2e]
fn flex_column_stacks_items_and_align_items_positions_cross_axis() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let pool: &PagePool = runtime.page_pool();

    // column:主轴纵向,子项依次 top 0/30/60;容器高收缩 90。
    // 交叉轴(横向)对齐:center → left=110;flex-end → left=220(显式宽 100)。
    // row 容器(高 100)+ align-items:center → 子项垂直居中 top=35。
    let html = concat!(
        "<!DOCTYPE html><html><head><style>",
        "html,body{margin:0;padding:0}",
        "#col{display:flex;flex-direction:column;width:320px}",
        "#x-center{display:flex;flex-direction:column;align-items:center;width:320px}",
        "#x-end{display:flex;flex-direction:column;align-items:flex-end;width:320px}",
        "#v-center{display:flex;flex-direction:row;align-items:center;width:320px;height:100px}",
        ".it{width:100px;height:30px}",
        "</style></head><body>",
        "<div id=\"col\"><div class=\"it\" id=\"c1\">1</div>",
        "<div class=\"it\" id=\"c2\">2</div><div class=\"it\" id=\"c3\">3</div></div>",
        "<div id=\"x-center\"><div class=\"it\" id=\"xc\">x</div></div>",
        "<div id=\"x-end\"><div class=\"it\" id=\"xe\">x</div></div>",
        "<div id=\"v-center\"><div class=\"it\" id=\"vc\">x</div></div>",
        "</body></html>"
    );
    let page = geometry_page(pool, html, "c1");

    let mut failures: Vec<String> = Vec::new();

    // 纵向堆叠 + 容器高度 = 3×30。
    let col = rect_of(&page, "col");
    expect_close(&mut failures, "col.height", col.height, 90.0);
    let c1 = rect_of(&page, "c1");
    expect_close(&mut failures, "c1.top", c1.top, 0.0);
    expect_close(&mut failures, "c1.left", c1.left, 0.0);
    expect_close(&mut failures, "c2.top", rect_of(&page, "c2").top, 30.0);
    expect_close(&mut failures, "c3.top", rect_of(&page, "c3").top, 60.0);

    // 交叉轴对齐:center → (320−100)/2 = 110;flex-end → 320−100 = 220。
    expect_close(&mut failures, "x-center/xc.left", rect_of(&page, "xc").left, 110.0);
    expect_close(&mut failures, "x-end/xe.left", rect_of(&page, "xe").left, 220.0);

    // row 容器垂直居中:getBoundingClientRect 是文档坐标,居中量须对容器
    // 相对取(vc.top − v-center.top = (100−30)/2 = 35)。
    let vcenter = rect_of(&page, "v-center");
    let vc = rect_of(&page, "vc");
    expect_close(&mut failures, "v-center.height", vcenter.height, 100.0);
    expect_close(
        &mut failures,
        "v-center/vc.top(relative)",
        vc.top - vcenter.top,
        35.0,
    );
    expect_close(&mut failures, "v-center/vc.height", vc.height, 30.0);

    finish(&failures, "flex column stacking + cross-axis alignment");
}

// ═══════════════════════════════════════════════════════════════════════════
// 4. grid 轨道几何 — grid-template-columns(REQ-BRW-002-C3)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BRW-002-C3 [level:e2e]
fn grid_template_columns_produce_track_geometry() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let pool: &PagePool = runtime.page_pool();

    // 3 条固定列轨(各 100px),6 个 100×30 子项自动放置 → 2 行 3 列。
    let html = concat!(
        "<!DOCTYPE html><html><head><style>",
        "html,body{margin:0;padding:0}",
        "#grid{display:grid;grid-template-columns:100px 100px 100px;width:320px}",
        ".g{width:100px;height:30px}",
        "</style></head><body>",
        "<div id=\"grid\">",
        "<div class=\"g\" id=\"g1\">1</div><div class=\"g\" id=\"g2\">2</div>",
        "<div class=\"g\" id=\"g3\">3</div><div class=\"g\" id=\"g4\">4</div>",
        "<div class=\"g\" id=\"g5\">5</div><div class=\"g\" id=\"g6\">6</div>",
        "</div>",
        "</body></html>"
    );
    let page = geometry_page(pool, html, "g1");

    let mut failures: Vec<String> = Vec::new();

    // 容器:宽 320(显式),高 = 2 行 × 30 = 60。
    let grid = rect_of(&page, "grid");
    expect_close(&mut failures, "grid.width", grid.width, 320.0);
    expect_close(&mut failures, "grid.height", grid.height, 60.0);

    // 第 1 行:g1(0,0) g2(100,0) g3(200,0)。
    expect_rect(
        &mut failures,
        "g1",
        rect_of(&page, "g1"),
        Rect {
            left: 0.0,
            top: 0.0,
            width: 100.0,
            height: 30.0,
        },
    );
    expect_close(&mut failures, "g2.left", rect_of(&page, "g2").left, 100.0);
    expect_close(&mut failures, "g3.left", rect_of(&page, "g3").left, 200.0);

    // 第 2 行:g4(0,30) g5(100,30) g6(200,30)。
    expect_close(&mut failures, "g4.left", rect_of(&page, "g4").left, 0.0);
    expect_close(&mut failures, "g4.top", rect_of(&page, "g4").top, 30.0);
    expect_close(&mut failures, "g5.left", rect_of(&page, "g5").left, 100.0);
    expect_close(&mut failures, "g5.top", rect_of(&page, "g5").top, 30.0);
    expect_close(&mut failures, "g6.left", rect_of(&page, "g6").left, 200.0);
    expect_close(&mut failures, "g6.top", rect_of(&page, "g6").top, 30.0);

    finish(&failures, "grid template-columns track geometry");
}

// ═══════════════════════════════════════════════════════════════════════════
// 5. grid gap — column-gap/row-gap 轨道间隔(REQ-BRW-002-C3)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BRW-002-C3 [level:e2e]
fn grid_gap_offsets_tracks_in_both_axes() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let pool: &PagePool = runtime.page_pool();

    // 2 条固定列轨(各 100px)+ column-gap 10 / row-gap 20,4 个 100×30 子项。
    let html = concat!(
        "<!DOCTYPE html><html><head><style>",
        "html,body{margin:0;padding:0}",
        "#gap{display:grid;grid-template-columns:100px 100px;column-gap:10px;row-gap:20px;width:210px}",
        ".g{width:100px;height:30px}",
        "</style></head><body>",
        "<div id=\"gap\">",
        "<div class=\"g\" id=\"p1\">1</div><div class=\"g\" id=\"p2\">2</div>",
        "<div class=\"g\" id=\"p3\">3</div><div class=\"g\" id=\"p4\">4</div>",
        "</div>",
        "</body></html>"
    );
    let page = geometry_page(pool, html, "p1");

    let mut failures: Vec<String> = Vec::new();

    // 容器:宽 = 100+10+100 = 210,高 = 30+20+30 = 80(gap 计入轨道盒)。
    let gap = rect_of(&page, "gap");
    expect_close(&mut failures, "gap.width", gap.width, 210.0);
    expect_close(&mut failures, "gap.height", gap.height, 80.0);

    // 列轨偏移:第 2 列 left = 100+10 = 110。
    expect_close(&mut failures, "p1.left", rect_of(&page, "p1").left, 0.0);
    expect_close(&mut failures, "p1.top", rect_of(&page, "p1").top, 0.0);
    expect_close(&mut failures, "p2.left", rect_of(&page, "p2").left, 110.0);
    expect_close(&mut failures, "p2.top", rect_of(&page, "p2").top, 0.0);

    // 行轨偏移:第 2 行 top = 30+20 = 50。
    expect_close(&mut failures, "p3.left", rect_of(&page, "p3").left, 0.0);
    expect_close(&mut failures, "p3.top", rect_of(&page, "p3").top, 50.0);
    expect_close(&mut failures, "p4.left", rect_of(&page, "p4").left, 110.0);
    expect_close(&mut failures, "p4.top", rect_of(&page, "p4").top, 50.0);

    finish(&failures, "grid gap track offsets");
}
