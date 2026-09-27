// @trace REQ-BRW-049 [level:e2e]
// @trace REQ-BRW-049-AC01 [level:e2e]
// @trace REQ-BRW-049-AC02 [level:e2e]
// @trace REQ-BRW-049-AC03 [level:e2e]
// @trace REQ-BRW-049-AC04 [level:e2e]
//
// ═══════════════════════════════════════════════════════════════════════════
// REQ-BRW-049 — CSS 能力继承验证面(Stylo 吸收波回归门)
// ═══════════════════════════════════════════════════════════════════════════
//
// **波末门语义(合同立法)**:本 suite 是 stylo/prefs/样式引擎吸收波的 CSS
// 回归门。任何触碰 vendor 样式引擎或 config/prefs.rs 的吸收波,合入前必须
// 跑 `cargo nt -p bao-browser -E 'test(css_conformance)'` 并全绿;红灯 =
// 吸收引入 CSS 能力静默回退,禁止合入,回滚或显式更新探针+本头注释立法。
//
// **语料来源(只读)**:~/code/tools/wpt-css(WPT css/ 稀疏检出,只读消费,
// 不进构建)。探针场景取材于下列 WPT 类别目录的稳定核心属性面,转换为
// computed-style 值断言(比 reftest 像素对比稳定,且规避本机 #40 渲染死锁
// 家族对截图通道的限制)。
//
// **探针选择判据**:①属性属 servo/stylo 稳定支持面(WPT 对应目录大量
// passing);②computed-style 可观测(值序列化确定,无浮动噪声);
// ③覆盖 CSS 能力大类(下表);④pref 门控面单独成类(gate 探针),
// 门翻转时探针红灯强制更新头表 = 零静默回退。
//
// **探针类别 × 语料映射(10 类 / 73 断言)**:
//   category        wpt 语料目录              探针数  断言面
//   css-color       css/css-color, css/CSS1   8      named/hex/rgb/rgba/hsl/currentcolor/bg
//   css-cascade     css/css-cascade           10     特异性/源序/important/inline/继承/shorthand/initial
//   css-variables   css/css-variables         3      定义替换/fallback/继承
//   css-flexbox     css/css-flexbox           10     display/flex-* 六属性/gap/order/align-self
//   css-grid(gate) css/css-grid              1      display:grid 门态(layout.grid.enabled=false→block)
//   css-fonts       css/css-fonts, css/CSS1   9      family/size(px+em 解析)/weight/style/line-height/spacing/transform
//   css-transforms  css/css-transforms        6      rotate180/translate/scale/scale0.5/none/origin
//   css-position    css/css-position, css/css-box  9  position 三态/z-index/margin em 解析/padding/border/display/box-sizing
//   css-animations  css/css-animations, css/css-transitions  5  name/duration/timing/iteration/transition
//   css-backgrounds css/css-backgrounds       5      image/repeat/border-style/color/shadow/outline
//
// **pref-gated CSS 能力启用清单(vendor/servo/components/config/prefs.rs 全量
//   CSS 相关门,逐项状态 = 头注释立法;吸收波翻转任一门必须同步更新本表 +
//   对应 gate 探针)**:
//   pref(点分)                    字段                              默认    CSS 能力面
//   layout.grid.enabled            layout_grid_enabled               false   display:grid 布局与解析门(gate 探针)
//   layout.columns.enabled         layout_columns_enabled            false   多列布局 columns/*
//   layout.css.attr.enabled        layout_css_attr_enabled           false   attr() 函数
//   layout.writing-mode.enabled    layout_writing_mode_enabled       false   竖排 writing-mode
//   layout.variable-fonts.enabled  layout_variable_fonts_enabled     false   可变字体 font-variation-settings
//   layout.container-queries.enabled layout_container_queries_enabled false  @container 查询
//   layout.animations.test.enabled layout_animations_test_enabled    false  动画测试钩子
//   layout.unimplemented           layout_unimplemented              false   未实现布局位开关
//   layout.threads                 layout_threads                    3       stylo 并行遍历数(性能,非能力门)
//   layout.style-sharing-cache.enabled layout_style_sharing_cache_enabled true 样式共享缓存(性能)
//   css.animations.testing.enabled css_animations_testing_enabled    false   CSS 动画测试钩子
//   dom.fontface.enabled           dom_fontface_enabled              false   @font-face 加载
//   dom.web-animations.enabled     dom_web_animations_enabled        false   Web Animations API
//   dom.parallel-css-parsing.enabled dom_parallel_css_parsing_enabled true  CSS 并行解析
//   fonts.default                  fonts_default                     ""(回退内置)  默认字体族
//   fonts.serif                    fonts_serif                       ""(回退内置)  serif 族
//   fonts.sans-serif               fonts_sans_serif                  ""(回退内置)  sans-serif 族
//   fonts.monospace                fonts_monospace                   ""(回退内置)  monospace 族
//   fonts.default-size             fonts_default_size                16      根 font-size(影响 em/rem 解析,探针依赖)
//   fonts.default-monospace-size   fonts_default_monospace_size      13      monospace 根字号
//   shell.background-color-rgba    shell_background_color_rgba       白色     默认画布底色
//
// **运行约束**:servo Opts per-process 单例 → 单 #[test];探针经
// evaluate_js_web(Window Realm,web 暴露面)逐条断言 computed style。
// 断言口径:精确匹配优先;序列化存在合法浮动/格式方差的面(matrix/字族
// 列表)用 contains,均钉住真实计算值而非形状壳。

use bao_browser::{BaoConfig, BaoRuntime, PageConfig, PagePool};
use std::time::{Duration, Instant};

/// CSS 能力探针 fixture:单页内嵌 <style> 全部探针规则 + 带探针 id 的元素。
/// '#' 转义 %23(data URL fragment 保留字)。
fn conformance_page_url() -> String {
    let html = concat!(
        "<!DOCTYPE html><html><head><style>",
        // css-color
        "#c-named{color:teal}",
        "#c-hex{color:#ff0000}",
        "#c-hex-short{color:#0f0}",
        "#c-rgb{color:rgb(1,2,3)}",
        "#c-rgba{color:rgba(10,20,30,0.5)}",
        "#c-hsl{color:hsl(120,100%,50%)}",
        "#c-bg{background-color:rgb(250,240,230)}",
        "#c-parent{color:rgb(255,0,0)}",
        "#c-currentcolor{color:currentcolor}",
        // css-cascade
        "#s-id{color:rgb(0,0,255)}",
        ".s-class{color:rgb(255,0,0)}",
        ".s-later-a{color:rgb(255,0,0)}",
        ".s-later-b{color:rgb(0,128,0)}",
        ".s-imp{color:rgb(255,0,0) !important}",
        "#s-stronger{color:rgb(0,0,255)}",
        ".s-inline{color:rgb(255,0,0)}",
        "body{color:rgb(0,0,255)}",
        ".s-inherit-child{display:block}",
        ":root{--probe-accent:rgb(0,128,0)}",
        ".v-use{color:var(--probe-accent)}",
        ".v-fallback{color:var(--probe-undefined,rgb(1,2,3))}",
        ".v-parent{--probe-inherited:rgb(255,128,0)}",
        ".v-child{color:var(--probe-inherited)}",
        ".m-shorthand{margin:1px 2px}",
        ".m-initial{font-weight:initial;display:block}",
        // css-flexbox
        "#f-box{display:flex;flex-direction:column;justify-content:center;align-items:flex-end;flex-wrap:wrap;gap:8px}",
        "#f-item{flex-grow:2;order:3;align-self:center}",
        "#f-shorthand{flex:2}",
        // css-grid(gate:layout.grid.enabled=false)
        "#g-box{display:grid}",
        // css-fonts
        "#ft-family{font-family:'Courier New',monospace}",
        "#ft-size{font-size:20px}",
        "#ft-em{font-size:1.5em}",
        "#ft-weight-bold{font-weight:bold}",
        "#ft-weight-num{font-weight:600}",
        "#ft-style{font-style:italic}",
        "#ft-lh{line-height:32px}",
        "#ft-spacing{letter-spacing:2px}",
        "#ft-transform{text-transform:uppercase}",
        // css-transforms
        "#t-rotate{transform:rotate(180deg)}",
        "#t-translate{transform:translate(10px,20px)}",
        "#t-scale{transform:scale(2)}",
        "#t-scale-half{transform:scale(0.5)}",
        "#t-none{transform:none}",
        "#t-origin{transform-origin:10px 20px}",
        // css-position / css-box
        "#p-abs{position:absolute}",
        "#p-rel{position:relative}",
        "#p-fixed{position:fixed}",
        "#p-z{position:relative;z-index:5}",
        "#p-margin-em{margin-left:2em}",
        "#p-padding{padding-top:8px}",
        "#p-border{border-top:3px solid rgb(0,0,255)}",
        "#p-inline-block{display:inline-block}",
        "#p-none{display:none}",
        "#p-box-sizing{box-sizing:border-box}",
        // css-animations / css-transitions
        "#a-name{animation:probe-spin 2s linear infinite}",
        "#a-duration{animation-duration:2s}",
        "#a-timing{animation-timing-function:linear}",
        "#a-iteration{animation-iteration-count:infinite}",
        "#a-transition{transition:opacity 300ms}",
        // css-backgrounds / borders
        "#b-image{background-image:none}",
        "#b-repeat{background-repeat:repeat}",
        "#b-style{border-bottom-style:solid}",
        "#b-color{border-left-color:rgb(0,0,255)}",
        "#b-shadow{box-shadow:none}",
        "#b-outline{outline:1px solid rgb(0,0,255)}",
        // css-selectors(经计算色值生效验证)
        "#sel-class{color:rgb(1,10,100)}",
        "#sel-desc p{color:rgb(2,20,90)}",
        "#sel-child>p{color:rgb(3,30,80)}",
        "#sel-attr[data-probe=on]{color:rgb(4,40,70)}",
        "#sel-first>p:first-child{color:rgb(5,50,60)}",
        "</style></head><body>",
        "<div id=\"c-parent\"><div id=\"c-named\">x</div><div id=\"c-currentcolor\">x</div></div>",
        "<div id=\"c-hex\">x</div>",
        "<div id=\"c-hex-short\">x</div>",
        "<div id=\"c-rgb\">x</div>",
        "<div id=\"c-rgba\">x</div>",
        "<div id=\"c-hsl\">x</div>",
        "<div id=\"c-bg\">x</div>",
        "",
        "<div id=\"s-id\" class=\"s-class\">x</div>",
        "<div class=\"s-later-a s-later-b\" id=\"s-later-b\">x</div>",
        "<div class=\"s-imp\" id=\"s-stronger\">x</div>",
        "<div class=\"s-inline\" id=\"s-inline\" style=\"color:rgb(0,128,0)\">x</div>",
        "<div id=\"s-inherit-root\"><div class=\"s-inherit-child\" id=\"s-inherit-child\">x</div></div>",
        "<div class=\"v-use\" id=\"v-use\">x</div>",
        "<div class=\"v-fallback\" id=\"v-fallback\">x</div>",
        "<div class=\"v-parent\" id=\"v-parent\"><div class=\"v-child\" id=\"v-child\">x</div></div>",
        "<div class=\"m-shorthand\" id=\"m-shorthand\">x</div>",
        "<div class=\"m-initial\" id=\"m-initial\">x</div>",
        "<div id=\"f-box\"><div id=\"f-item\">x</div><div id=\"f-shorthand\">x</div></div>",
        "<div id=\"g-box\">x</div>",
        "<div id=\"ft-family\">x</div>",
        "<div id=\"ft-size\">x</div>",
        "<div id=\"ft-em\">x</div>",
        "<div id=\"ft-weight-bold\">x</div>",
        "<div id=\"ft-weight-num\">x</div>",
        "<div id=\"ft-style\">x</div>",
        "<div id=\"ft-lh\">x</div>",
        "<div id=\"ft-spacing\">x</div>",
        "<div id=\"ft-transform\">x</div>",
        "<div id=\"t-rotate\">x</div>",
        "<div id=\"t-translate\">x</div>",
        "<div id=\"t-scale\">x</div>",
        "<div id=\"t-scale-half\">x</div>",
        "<div id=\"t-none\">x</div>",
        "<div id=\"t-origin\">x</div>",
        "<div id=\"p-abs\">x</div>",
        "<div id=\"p-rel\">x</div>",
        "<div id=\"p-fixed\">x</div>",
        "<div id=\"p-z\">x</div>",
        "<div id=\"p-margin-em\">x</div>",
        "<div id=\"p-padding\">x</div>",
        "<div id=\"p-border\">x</div>",
        "<div id=\"p-inline-block\">x</div>",
        "<div id=\"p-none\">x</div>",
        "<div id=\"p-box-sizing\">x</div>",
        "<div id=\"a-name\">x</div>",
        "<div id=\"a-duration\">x</div>",
        "<div id=\"a-timing\">x</div>",
        "<div id=\"a-iteration\">x</div>",
        "<div id=\"a-transition\">x</div>",
        "<div id=\"b-image\">x</div>",
        "<div id=\"b-repeat\">x</div>",
        "<div id=\"b-style\">x</div>",
        "<div id=\"b-color\">x</div>",
        "<div id=\"b-shadow\">x</div>",
        "<div id=\"b-outline\">x</div>",
        "<div id=\"sel-class\" class=\"sel-class\">x</div>",
        "<div id=\"sel-desc\"><p>x</p></div>",
        "<div id=\"sel-child\"><p>x</p></div>",
        "<div id=\"sel-attr\" data-probe=\"on\">x</div>",
        "<div id=\"sel-first\"><p>x</p></div>",
        "</body></html>"
    );
    let encoded = html.replace('#', "%23");
    format!("data:text/html;charset=utf-8,{}", encoded)
}

#[test]
// @trace REQ-BRW-049 [level:e2e]
fn css_conformance_computed_style_gate() {
    let runtime = BaoRuntime::new(BaoConfig::default()).expect("BaoRuntime::new failed");
    let pool: &PagePool = runtime.page_pool();
    let page = pool
        .create_page(&PageConfig {
            url: Some(conformance_page_url()),
            ..Default::default()
        })
        .expect("pool.create_page failed");
    // 宽等待预算:开发机常驻并行构建负载,引擎初始化可能超出默认探针窗。
    let start = Instant::now();
    while start.elapsed().as_millis() < 20000 {
        let _ = page.evaluate_js_web("");
        if matches!(
            page.get_state(),
            bao_browser::PageState::Interactive | bao_browser::PageState::Idle
        ) {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    let mut report = Report::default();

    // computed style 读取助手(Window Realm web 暴露面)。
    // Bracket property access: hyphenated properties ("background-color")
    // must not go through dot syntax (parsed as subtraction).
    let probe = |report: &mut Report, id: &str, property: &str, expected: &str| {
        let script = format!(
            "getComputedStyle(document.getElementById('{}'))[\"{}\"]",
            id, property
        );
        let actual = page
            .evaluate_js_web(&script)
            .map_err(|e| format!("{}", e));
        report.expect(&format!("{}[{}]", id, property), actual, expected);
    };

    // contains 口径:序列化为列表/带合理方差的面(字族列表等)。
    let probe_contains = |report: &mut Report, id: &str, property: &str, needle: &str| {
        let script = format!(
            "getComputedStyle(document.getElementById('{}'))[\"{}\"]",
            id, property
        );
        let actual = page
            .evaluate_js_web(&script)
            .map_err(|e| format!("{}", e));
        match &actual {
            Ok(value) if value.contains(needle) => {
                report.passed += 1;
                report.messages.push(format!("PASS  {}[{}] contains {}", id, property, value));
            },
            Ok(value) => {
                report.failed += 1;
                report.messages.push(format!(
                    "FAIL  {}[{}] = {} (expected to contain {})",
                    id, property, value, needle
                ));
            },
            Err(error) => {
                report.failed += 1;
                report.messages.push(format!("FAIL  {}[{}] evaluate error: {}", id, property, error));
            },
        }
    };

    // ── css-color(8) ────────────────────────────────────────────────
    probe(&mut report, "c-named", "color", "rgb(0, 128, 128)");
    probe(&mut report, "c-hex", "color", "rgb(255, 0, 0)");
    probe(&mut report, "c-hex-short", "color", "rgb(0, 255, 0)");
    probe(&mut report, "c-rgb", "color", "rgb(1, 2, 3)");
    probe(&mut report, "c-rgba", "color", "rgba(10, 20, 30, 0.5)");
    probe(&mut report, "c-hsl", "color", "rgb(0, 255, 0)");
    probe(&mut report, "c-bg", "background-color", "rgb(250, 240, 230)");
    probe(&mut report, "c-currentcolor", "color", "rgb(255, 0, 0)");

    // ── css-cascade(10) ─────────────────────────────────────────────
    probe(&mut report, "s-id", "color", "rgb(0, 0, 255)");
    probe(&mut report, "s-later-b", "color", "rgb(0, 128, 0)");
    probe(&mut report, "s-stronger", "color", "rgb(255, 0, 0)");
    probe(&mut report, "s-inline", "color", "rgb(0, 128, 0)");
    // 继承:子元素未声明 color,继承 body 的蓝色
    probe(&mut report, "s-inherit-child", "color", "rgb(0, 0, 255)");
    // css-variables(3)
    probe(&mut report, "v-use", "color", "rgb(0, 128, 0)");
    probe(&mut report, "v-fallback", "color", "rgb(1, 2, 3)");
    probe(&mut report, "v-child", "color", "rgb(255, 128, 0)");
    // shorthand 展开 + initial 复位
    probe(&mut report, "m-shorthand", "margin-left", "2px");
    probe(&mut report, "m-shorthand", "margin-top", "1px");
    probe(&mut report, "m-initial", "font-weight", "400");

    // ── css-flexbox(10) ─────────────────────────────────────────────
    probe(&mut report, "f-box", "display", "flex");
    probe(&mut report, "f-box", "flex-direction", "column");
    probe(&mut report, "f-box", "justify-content", "center");
    probe(&mut report, "f-box", "align-items", "flex-end");
    probe(&mut report, "f-box", "flex-wrap", "wrap");
    probe(&mut report, "f-box", "row-gap", "8px");
    probe(&mut report, "f-item", "flex-grow", "2");
    probe(&mut report, "f-item", "order", "3");
    probe(&mut report, "f-item", "align-self", "center");
    probe(&mut report, "f-shorthand", "flex-grow", "2");

    // ── css-grid(gate 探针:layout.grid.enabled=false)───────────────
    // 门关 → grid 解析被 stylo 拒绝 → computed 回落 block。门翻转时本探针
    // 红灯,强制同步更新头表立法(REQ-BRW-049 零静默回退语义)。
    probe(&mut report, "g-box", "display", "block");

    // ── css-fonts(9) ────────────────────────────────────────────────
    probe_contains(&mut report, "ft-family", "font-family", "monospace");
    probe(&mut report, "ft-size", "font-size", "20px");
    probe(&mut report, "ft-em", "font-size", "24px");
    probe(&mut report, "ft-weight-bold", "font-weight", "700");
    probe(&mut report, "ft-weight-num", "font-weight", "600");
    probe(&mut report, "ft-style", "font-style", "italic");
    probe(&mut report, "ft-lh", "line-height", "32px");
    probe(&mut report, "ft-spacing", "letter-spacing", "2px");
    probe(&mut report, "ft-transform", "text-transform", "uppercase");

    // ── css-transforms(6) ───────────────────────────────────────────
    // SM 的 sin(π)/cos(π) 带 1.22e-16 量级残差,servo 不做归一化;
    // 钉实测确定值 = 对三角函数归一化/SM 版本漂移的回归哨。
    probe(
        &mut report,
        "t-rotate",
        "transform",
        "matrix(-1, 1.22465e-16, -1.22465e-16, -1, 0, 0)",
    );
    probe(&mut report, "t-translate", "transform", "matrix(1, 0, 0, 1, 10, 20)");
    probe(&mut report, "t-scale", "transform", "matrix(2, 0, 0, 2, 0, 0)");
    probe(&mut report, "t-scale-half", "transform", "matrix(0.5, 0, 0, 0.5, 0, 0)");
    probe(&mut report, "t-none", "transform", "none");
    probe(&mut report, "t-origin", "transform-origin", "10px 20px 0px");

    // ── css-position / css-box(9) ───────────────────────────────────
    probe(&mut report, "p-abs", "position", "absolute");
    probe(&mut report, "p-rel", "position", "relative");
    probe(&mut report, "p-fixed", "position", "fixed");
    probe(&mut report, "p-z", "z-index", "5");
    probe(&mut report, "p-margin-em", "margin-left", "32px");
    probe(&mut report, "p-padding", "padding-top", "8px");
    probe(&mut report, "p-border", "border-top-width", "3px");
    probe(&mut report, "p-inline-block", "display", "inline-block");
    probe(&mut report, "p-box-sizing", "box-sizing", "border-box");

    // ── css-animations / css-transitions(5) ─────────────────────────
    probe(&mut report, "a-name", "animation-name", "probe-spin");
    probe(&mut report, "a-duration", "animation-duration", "2s");
    probe(&mut report, "a-timing", "animation-timing-function", "linear");
    probe(&mut report, "a-iteration", "animation-iteration-count", "infinite");
    probe(&mut report, "a-transition", "transition-duration", "0.3s");

    // ── css-backgrounds / borders(5) ────────────────────────────────
    probe(&mut report, "b-image", "background-image", "none");
    probe(&mut report, "b-repeat", "background-repeat", "repeat");
    probe(&mut report, "b-style", "border-bottom-style", "solid");
    probe(&mut report, "b-color", "border-left-color", "rgb(0, 0, 255)");
    probe(&mut report, "b-shadow", "box-shadow", "none");

    // ── css-selectors(6,经计算色值生效验证)─────────────────────────
    probe(&mut report, "sel-class", "color", "rgb(1, 10, 100)");
    let probe_selector = |report: &mut Report, selector: &str, property: &str, expected: &str| {
        let script = format!(
            "getComputedStyle(document.querySelector('{}'))[\"{}\"]",
            selector, property
        );
        let actual = page
            .evaluate_js_web(&script)
            .map_err(|e| format!("{}", e));
        report.expect(&format!("{}[{}]", selector, property), actual, expected);
    };
    // div 层:选择器命中的是 p,div 自身继承 body 蓝(继承语义顺带钉住)
    probe(&mut report, "sel-desc", "color", "rgb(0, 0, 255)");
    probe_selector(&mut report, "#sel-desc p", "color", "rgb(2, 20, 90)");
    probe(&mut report, "sel-child", "color", "rgb(0, 0, 255)");
    probe_selector(&mut report, "#sel-child > p", "color", "rgb(3, 30, 80)");
    probe(&mut report, "sel-attr", "color", "rgb(4, 40, 70)");
    probe(&mut report, "sel-first", "color", "rgb(0, 0, 255)");
    probe_selector(&mut report, "#sel-first > p", "color", "rgb(5, 50, 60)");
    probe(&mut report, "p-none", "display", "none");

    report.finish();
    assert_eq!(
        report.failed, 0,
        "REQ-BRW-049 CSS conformance gate: failures = absorption-wave silent capability regression"
    );
    assert!(
        report.passed >= 60,
        "REQ-BRW-049 probe floor (60) violated — update the header inventory table"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Report(容错收集,单进程单测试约束下的断言聚合)
// ═══════════════════════════════════════════════════════════════════════════

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
                    .push(format!("FAIL  {} evaluate error: {}", name, error));
            },
        }
    }

    fn finish(&self) {
        eprintln!("=== CSS Conformance Gate (REQ-BRW-049) ===");
        for message in &self.messages {
            eprintln!("{}", message);
        }
        eprintln!("--- {} passed, {} failed ---", self.passed, self.failed);
    }
}
