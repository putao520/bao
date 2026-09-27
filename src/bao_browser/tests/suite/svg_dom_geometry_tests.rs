// @trace REQ-BRW-046 [level:e2e]
// @trace REQ-BRW-046-AC01 [level:e2e]
// @trace REQ-BRW-046-AC02 [level:e2e]
// @trace REQ-BRW-046-AC03 [level:e2e]
// @trace REQ-BRW-046-AC04 [level:e2e]
//
// REQ-BRW-046 E2E — SVG DOM 几何与反射 API 实装
//
// **核心断言**: servo fork 自实现的 SVG 几何 API 返回真实计算值(非 stub 空值):
//   1. getBBox 从几何属性计算(circle r*2、rect 属性值、容器 union)
//   2. getCTM 从 transform 属性链构造(自身 + 祖先,局部→viewport)
//   3. getTotalLength/getPointAtLength 返回真实路径计算值(已知线段精确值 + 端点命中)
//   4. 既有 SVG 接口反射不回退(instanceof SVGCircleElement、方法面在位)
//
// **运行约束**: servo Opts 是 per-process 单例,所有断言合并到单个 #[test]。

use bao_browser::{BaoConfig, BaoRuntime, PageConfig, PagePool};
use std::time::{Duration, Instant};

fn wait_for_load_and_drain(pool_page: &bao_browser::PageHandle, max_ms: u64) {
    // 同 dom_node_interop_tests.rs::wait_for_load_and_drain — evaluate_js 触发
    // servo script thread 回调 drain,同时观察 PageState。
    let start = Instant::now();
    while start.elapsed().as_millis() < max_ms as u128 {
        let _ = pool_page.evaluate_js("");
        if matches!(
            pool_page.get_state(),
            bao_browser::PageState::Interactive | bao_browser::PageState::Idle
        ) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[derive(Default)]
struct Report {
    passed: u32,
    failed: u32,
    messages: Vec<String>,
}

impl Report {
    fn expect(&mut self, name: &str, actual: Result<String, String>, expected: &str) {
        match actual {
            Ok(value) if value == expected => {
                self.passed += 1;
                self.messages.push(format!("PASS  {} = {}", name, value));
            },
            Ok(value) => {
                self.failed += 1;
                self.messages
                    .push(format!("FAIL  {} = {} (expected {})", name, value, expected));
            },
            Err(error) => {
                self.failed += 1;
                self.messages
                    .push(format!("FAIL  {} evaluate_js error: {}", name, error));
            },
        }
    }

    fn finish(&self) {
        eprintln!("=== SVG DOM Geometry E2E (REQ-BRW-046) ===");
        for message in &self.messages {
            eprintln!("{}", message);
        }
        eprintln!("--- {} passed, {} failed ---", self.passed, self.failed);
    }
}

#[test]
// @trace REQ-BRW-046 [level:e2e]
fn svg_dom_geometry_apis_return_real_values() {
    let runtime = match BaoRuntime::new(BaoConfig::default()) {
        Ok(runtime) => runtime,
        Err(error) => panic!("BaoRuntime::new failed: {}", error),
    };
    let pool: &PagePool = runtime.page_pool();

    let html = concat!(
        "<!DOCTYPE html><html><body>",
        "<svg id=\"s\" xmlns=\"http://www.w3.org/2000/svg\" width=\"200\" height=\"200\">",
        "<circle id=\"c\" cx=\"100\" cy=\"50\" r=\"25\"/>",
        "<rect id=\"rc\" x=\"10\" y=\"20\" width=\"30\" height=\"40\"/>",
        "<rect id=\"rt\" x=\"0\" y=\"0\" width=\"5\" height=\"5\" transform=\"translate(50,30)\"/>",
        "<g transform=\"translate(50,30)\"><rect id=\"rg\" width=\"5\" height=\"5\"/></g>",
        "<path id=\"p\" d=\"M0,0 L30,40\"/>",
        "<path id=\"p2\" d=\"M10,20 L110,20\"/>",
        "<ellipse id=\"el\" cx=\"0\" cy=\"0\" rx=\"20\" ry=\"10\"/>",
        "</svg>",
        "</body></html>"
    );
    let url = format!("data:text/html;charset=utf-8,{}", html);
    let page = match pool.create_page(&PageConfig {
        url: Some(url),
        ..Default::default()
    }) {
        Ok(page) => page,
        Err(error) => panic!("pool.create_page failed: {}", error),
    };
    // Generous load tolerance: the dev box runs parallel worker builds, and
    // engine init can exceed the default probe window under that contention.
    wait_for_load_and_drain(&page, 20000);

    let evaluate = |script: &str| {
        // eval() preserves the statement-list completion value while the
        // surrounding try/catch surfaces real JS exception messages (the
        // bao evaluate wrapper discards the pending exception detail).
        fn json_escape(s: &str) -> String {
            let mut out = String::with_capacity(s.len() + 2);
            for c in s.chars() {
                match c {
                    '\\' => out.push_str("\\\\"),
                    '"' => out.push_str("\\\""),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    c => out.push(c),
                }
            }
            out
        }
        let wrapped = format!(
            "(function(){{ try {{ return eval(\"{}\"); }} catch (e) {{ return 'THROW:' + ((e && e.message) || String(e)); }} }})()",
            json_escape(script)
        );
        // SVG DOM APIs live in the Window Realm (web-exposed surface); assert
        // there via the Web-Realm evaluate channel.
        page.evaluate_js_web(&wrapped)
            .map_err(|error| format!("{}", error))
    };

    let mut report = Report::default();

    // ── AC01: getBBox 真实几何包围盒 ─────────────────────────────────────
    // circle: (cx-r, cy-r, 2r, 2r) = (75, 25, 50, 50)
    report.expect(
        "AC01::circle_getBBox",
        evaluate(
            "const b = document.getElementById('c').getBBox();
             [b.x, b.y, b.width, b.height].join(',');",
        ),
        "75,25,50,50",
    );
    // rect: 属性值原样 (10, 20, 30, 40)
    report.expect(
        "AC01::rect_getBBox",
        evaluate(
            "const b = document.getElementById('rc').getBBox();
             [b.x, b.y, b.width, b.height].join(',');",
        ),
        "10,20,30,40",
    );
    // 容器 g: 子元素 union(g 自身 transform 不参与 — bbox 在自身用户坐标系)
    report.expect(
        "AC01::g_getBBox_union",
        evaluate(
            "const b = document.getElementById('s').querySelector('g').getBBox();
             [b.x, b.y, b.width, b.height].join(',');",
        ),
        "0,0,5,5",
    );

    // ── AC02: getCTM / getScreenCTM 真实变换矩阵 ─────────────────────────
    // 自身 transform="translate(50,30)" → e=50, f=30
    report.expect(
        "AC02::own_transform_getCTM",
        evaluate(
            "const m = document.getElementById('rt').getCTM();
             [m.e, m.f].join(',');",
        ),
        "50,30",
    );
    // 祖先 g 的 transform 继承进子元素 CTM
    report.expect(
        "AC02::inherited_transform_getCTM",
        evaluate(
            "const m = document.getElementById('rg').getCTM();
             [m.e, m.f].join(',');",
        ),
        "50,30",
    );
    // 无 transform 的元素 → 单位矩阵
    report.expect(
        "AC02::identity_getCTM",
        evaluate(
            "const m = document.getElementById('rc').getCTM();
             [m.e, m.f, m.is2D].join(',');",
        ),
        "0,0,true",
    );
    // getScreenCTM(v1 边界 = 局部→viewport,非 null 且与 CTM 同值)
    report.expect(
        "AC02::getScreenCTM",
        evaluate(
            "const m = document.getElementById('rt').getScreenCTM();
             [m.e, m.f].join(',');",
        ),
        "50,30",
    );

    // ── AC03: getTotalLength / getPointAtLength 真实路径计算值 ────────────
    // M0,0 L30,40 → 3-4-5 直角三角形斜边 = 50
    report.expect(
        "AC03::path_getTotalLength",
        evaluate("document.getElementById('p').getTotalLength().toString();"),
        "50",
    );
    // 端点命中:getPointAtLength(0) = 起点
    report.expect(
        "AC03::getPointAtLength_start",
        evaluate(
            "const pt = document.getElementById('p2').getPointAtLength(0);
             [pt.x, pt.y].join(',');",
        ),
        "10,20",
    );
    // 端点命中:getPointAtLength(100) = 终点
    report.expect(
        "AC03::getPointAtLength_end",
        evaluate(
            "const pt = document.getElementById('p2').getPointAtLength(100);
             [pt.x, pt.y].join(',');",
        ),
        "110,20",
    );
    // 中点
    report.expect(
        "AC03::getPointAtLength_mid",
        evaluate(
            "const pt = document.getElementById('p2').getPointAtLength(50);
             [pt.x, pt.y].join(',');",
        ),
        "60,20",
    );
    // 非路径几何:ellipse 周长(Ramanujan)≈ 96.88
    report.expect(
        "AC03::ellipse_getTotalLength",
        evaluate(
            "document.getElementById('el').getTotalLength().toFixed(1).toString();",
        ),
        "96.9",
    );

    // ── AC04: 既有接口反射不回退(类型面 + 方法面) ──────────────────────
    report.expect(
        "AC04::interface_reflection",
        evaluate(
            "const c = document.getElementById('c');
             (c instanceof SVGCircleElement)
               && (c instanceof SVGElement)
               && (typeof c.getBBox === 'function')
               && (typeof c.getTotalLength === 'function')
               && (typeof c.getCTM === 'function')
               && (typeof c.getScreenCTM === 'function')
               && (typeof c.getPointAtLength === 'function')
               && (typeof SVGGraphicsElement !== 'undefined')
               && (typeof SVGGeometryElement !== 'undefined');",
        ),
        "true",
    );

    report.finish();
    assert_eq!(
        report.failed, 0,
        "REQ-BRW-046 SVG DOM geometry E2E had failures"
    );
    assert_eq!(
        report.passed, 13,
        "REQ-BRW-046 SVG DOM geometry E2E assertion count drifted"
    );
}
