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
// **运行约束**: servo Opts 是 per-process 单例。本文件两个 #[test](几何值断言 +
// AC04 26 接口表驱动枚举)各自在 nextest 的独立进程内运行(suite 运行时隔离由
// cargo-nextest 保证,见 CLAUDE.md「测试运行纪律」);plain cargo test 走
// --test-threads=1(项目纪律),顺序 runtime 已由 e134 根治(c6968848)。

use bao_browser::{BaoConfig, BrowserRuntime, PageConfig, PagePool};
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
    let runtime = match BrowserRuntime::new(BaoConfig::default()) {
        Ok(runtime) => runtime,
        Err(error) => panic!("BrowserRuntime::new failed: {}", error),
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

// ---------------------------------------------------------------------------
// AC04: 既有 26 个 SVG webidl 接口逐一反射与类型不回退(表驱动)
// ---------------------------------------------------------------------------

/// 26 接口清单 = `.spec/10-REQUIREMENTS.html` REQ-BRW-046 正文「既有 26 个 SVG
/// webidl」↔ `vendor/servo/components/script_bindings/webidls/SVG*.webidl` 实测
/// 清单(26 文件,2026-10-14 实测),经 `components/script/dom/element/create.rs`
/// 的 SVG 命名空间分发表实例化(抽象接口经代表实例断言)。
///
/// 每接口关键方法取自各 webidl 的未注释面:
/// - `getBBox`/`getCTM`/`getScreenCTM` = SVGGraphicsElement(BAO fork 面,REQ-BRW-046)
/// - `getTotalLength`/`getPointAtLength` = SVGGeometryElement(BAO fork 面,REQ-BRW-046)
/// - `focus` = HTMLOrSVGElement mixin(SVGElement 直系:渐变/pattern/stop 家族)
/// - `getAttribute` = Element 基面(全接口兜底)
///
/// 断言只查存在性(`typeof === "function"` / `instanceof`),值面归 AC01-AC03。
/// 吸收波后本表即 AC04「行为保持」的回归钉:接口/方法面回退 = 本测试红。
const SVG_INTERFACE_TABLE: &[(&str, &str, &[&str])] = &[
    // (接口名, 代表实例元素 id, 关键方法)
    ("SVGAElement", "ra", &["getBBox", "getCTM"]),
    (
        "SVGCircleElement",
        "rc",
        &["getBBox", "getTotalLength", "getPointAtLength"],
    ),
    ("SVGDefsElement", "rd", &["getBBox"]),
    ("SVGElement", "rr", &["getAttribute", "focus"]),
    ("SVGEllipseElement", "re", &["getBBox", "getTotalLength"]),
    ("SVGGElement", "rg", &["getBBox", "getCTM", "getScreenCTM"]),
    (
        "SVGGeometryElement",
        "rc",
        &["getTotalLength", "getPointAtLength"],
    ),
    ("SVGGradientElement", "rlg", &["getAttribute", "focus"]),
    (
        "SVGGraphicsElement",
        "rg",
        &["getBBox", "getCTM", "getScreenCTM"],
    ),
    ("SVGImageElement", "ri", &["getBBox"]),
    ("SVGLinearGradientElement", "rlg", &["getAttribute"]),
    ("SVGLineElement", "rl", &["getBBox", "getTotalLength"]),
    (
        "SVGPathElement",
        "rp",
        &["getBBox", "getTotalLength", "getPointAtLength"],
    ),
    ("SVGPatternElement", "rpt", &["getAttribute", "focus"]),
    ("SVGPolygonElement", "rpg", &["getBBox", "getTotalLength"]),
    ("SVGPolylineElement", "rpl", &["getBBox", "getTotalLength"]),
    ("SVGRadialGradientElement", "rrg", &["getAttribute"]),
    ("SVGRectElement", "rr", &["getBBox", "getTotalLength"]),
    ("SVGStopElement", "rst", &["getAttribute", "focus"]),
    ("SVGSVGElement", "rs", &["getBBox", "getCTM"]),
    ("SVGSymbolElement", "rsy", &["getBBox"]),
    ("SVGTextContentElement", "rt", &["getBBox"]),
    ("SVGTextElement", "rt", &["getBBox"]),
    ("SVGTextPositioningElement", "rt", &["getBBox"]),
    ("SVGTSpanElement", "rts", &["getBBox"]),
    ("SVGUseElement", "ru", &["getBBox"]),
];

#[test]
// @trace REQ-BRW-046 [level:e2e]
// @trace REQ-BRW-046-AC04 [level:e2e]
fn svg_interface_reflection_26_no_regression() {
    let runtime = match BrowserRuntime::new(BaoConfig::default()) {
        Ok(runtime) => runtime,
        Err(error) => panic!("BrowserRuntime::new failed: {}", error),
    };
    let pool: &PagePool = runtime.page_pool();

    // 每个具体接口一个元素实例(标签→DOM 类型映射见 element/create.rs SVG 分发表);
    // 全 fixture 禁 `#` 字符(data: URL 的 fragment 边界会把文档截断,首个红定位实证);
    // image/use 不带 href(零资源取回),stop 挂在 linearGradient 下、tspan 挂在 text 下
    // (合法内容模型,反射面与内容模型无关)。
    let html = concat!(
        "<!DOCTYPE html><html><body>",
        "<svg id=\"rs\" xmlns=\"http://www.w3.org/2000/svg\" width=\"200\" height=\"200\">",
        "<a id=\"ra\"><rect width=\"1\" height=\"1\"/></a>",
        "<circle id=\"rc\" cx=\"100\" cy=\"50\" r=\"25\"/>",
        "<defs id=\"rd\"/>",
        "<ellipse id=\"re\" cx=\"0\" cy=\"0\" rx=\"20\" ry=\"10\"/>",
        "<g id=\"rg\"><rect width=\"5\" height=\"5\"/></g>",
        "<image id=\"ri\"/>",
        "<linearGradient id=\"rlg\"><stop id=\"rst\" offset=\"0\"/></linearGradient>",
        "<line id=\"rl\" x1=\"0\" y1=\"0\" x2=\"10\" y2=\"10\"/>",
        "<path id=\"rp\" d=\"M0,0 L30,40\"/>",
        "<pattern id=\"rpt\"/>",
        "<polygon id=\"rpg\" points=\"0,0 10,0 10,10\"/>",
        "<polyline id=\"rpl\" points=\"0,0 10,10\"/>",
        "<radialGradient id=\"rrg\"/>",
        "<rect id=\"rr\" x=\"10\" y=\"20\" width=\"30\" height=\"40\"/>",
        "<symbol id=\"rsy\"/>",
        "<text id=\"rt\"><tspan id=\"rts\">t</tspan></text>",
        "<use id=\"ru\"/>",
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
    wait_for_load_and_drain(&page, 20000);

    let evaluate = |script: &str| {
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
        page.evaluate_js_web(&wrapped).map_err(|error| format!("{}", error))
    };

    let mut report = Report::default();

    // ── AC04 主体:26 接口逐一 instanceof + 关键方法存在性(单轮 evaluate,逐接口裁决)──
    let cases_js: String = SVG_INTERFACE_TABLE
        .iter()
        .map(|(iface, id, methods)| {
            let methods =
                methods.iter().map(|m| format!("\"{m}\"")).collect::<Vec<_>>().join(",");
            format!("[\"{iface}\",\"{id}\",[{}]]", methods)
        })
        .collect::<Vec<_>>()
        .join(",");
    let verdicts = evaluate(&format!(
        "(function(){{ \
             var cases=[{}]; var out=[]; \
             for (var i = 0; i < cases.length; i++) {{ \
                 var name = cases[i][0], id = cases[i][1], methods = cases[i][2]; \
                 var C = window[name]; var el = document.getElementById(id); var why = ''; \
                 if (!C) {{ why = 'ctor-missing'; }} \
                 else if (!el) {{ why = 'element-missing:' + id; }} \
                 else if (!(el instanceof C)) {{ why = 'instanceof-false'; }} \
                 else {{ \
                     for (var j = 0; j < methods.length; j++) {{ \
                         if (typeof el[methods[j]] !== 'function') {{ why = 'method-missing:' + methods[j]; break; }} \
                     }} \
                 }} \
                 out.push(name + '=' + (why === '' ? 'ok' : why)); \
             }} \
             return out.join(';'); \
         }})()",
        cases_js
    ));

    match verdicts {
        Ok(verdicts) => {
            let parts: Vec<&str> = verdicts.split(';').collect();
            if parts.len() != SVG_INTERFACE_TABLE.len() {
                report.expect(
                    "AC04::verdict_count",
                    Ok(format!("len={}", parts.len())),
                    &format!("len={}", SVG_INTERFACE_TABLE.len()),
                );
            } else {
                for ((iface, _, _), part) in SVG_INTERFACE_TABLE.iter().zip(parts.iter()) {
                    report.expect(
                        &format!("AC04::{}", iface),
                        Ok((*part).to_string()),
                        &format!("{}=ok", iface),
                    );
                }
            }
        },
        Err(error) => {
            report.expect("AC04::table_runner", Err(error), "");
        },
    }

    // ── AC04 类型不回退:具体类型互斥 + 非 graphics 类型缺 graphics 方法面 ──
    report.expect(
        "AC04::concrete_type_isolation",
        evaluate(
            "const rr = document.getElementById('rr'); \
             (!!(rr instanceof SVGRectElement)) \
               && !(rr instanceof SVGCircleElement) \
               && !(rr instanceof SVGPathElement) \
               && !(rr instanceof SVGSVGElement) ? 'true' : 'false';",
        ),
        "true",
    );
    // 渐变/pattern/stop 家族直系 SVGElement(非 SVGGraphicsElement):
    // getBBox 不在其原型链上(类型面未回退成「全体 SVG 元素都有 graphics 方法」)
    report.expect(
        "AC04::non_graphics_lacks_geometry_surface",
        evaluate(
            "const lg = document.getElementById('rlg'); \
             const rt = document.getElementById('rt'); \
             (typeof lg.getBBox === 'undefined') \
               && (typeof lg.getTotalLength === 'undefined') \
               && (rt instanceof SVGTextContentElement) \
               && (rt instanceof SVGTextPositioningElement) \
               && !(rt instanceof SVGGeometryElement) ? 'true' : 'false';",
        ),
        "true",
    );

    report.finish();
    assert_eq!(
        report.failed, 0,
        "REQ-BRW-046 SVG 26-interface reflection had failures"
    );
    assert_eq!(
        report.passed,
        SVG_INTERFACE_TABLE.len() as u32 + 2,
        "REQ-BRW-046 SVG 26-interface reflection assertion count drifted"
    );
}
