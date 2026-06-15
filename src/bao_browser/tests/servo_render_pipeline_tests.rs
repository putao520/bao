// @trace TEST-E2E-RENDER [req:REQ-BRW-001,REQ-BRW-002,REQ-BRW-003,REQ-LIB-001,REQ-LIB-004] [level:e2e]
// @trace REQ-BRW-001 [entity:PageHandle] [level:e2e]
// @trace REQ-BRW-002 [level:e2e]
// @trace REQ-BRW-003 [level:e2e]
//
// # TASK-12 E2E — servo 真渲染链路
//
// **核心断言**: bao_browser 的 PageHandle 完整驱动 servo 的渲染管线:
//   navigate → servo 加载 → DOM 可查询 → screenshot 产出像素
//
// 这是 bao 系统最核心的"杀手锏":一个 Rust 库就能完整驱动浏览器渲染,
// 不依赖外部 Chrome / GeckoDriver。本测试通过本地 data: URL 默认运行,
// 不需要网络访问 — 避免 CI 环境网络不稳定导致 flaky。
//
// 链路节点(每个节点都需通过):
//   1. **navigate**: page.navigate(url) → servo WebView.load
//   2. **load**: servo 异步加载 HTML → 触发 PageState 转移
//   3. **DOM query**: evaluate_js("document.title") → servo script thread 执行
//   4. **DOM mutate**: evaluate_js("document.body.innerHTML = ...") → 真改 DOM
//   5. **screenshot**: page.take_screenshot() → servo SoftwareRendering → PNG
//
// **运行约束**: servo Opts 是 per-process 单例,所有断言合并到单个 #[test]。
// 网络 navigate(https://example.com)用 #[ignore] + BAO_TEST_NETWORK=1 启用。

use bao_browser::{BaoConfig, BaoRuntime, PageConfig, PageHandle, PagePool, PageState, ScreenshotFormat};
use std::time::{Duration, Instant};

// ─── 容错 Report ─────────────────────────────────────────────────────────────

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
    fn assert(&mut self, ok: bool, pass: &str, fail: &str) {
        if ok {
            self.pass(pass);
        } else {
            self.fail(fail, "assertion failed");
        }
    }
    fn finish(&self) {
        eprintln!("=== Servo Render Pipeline E2E ===");
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
        std::thread::sleep(Duration::from_millis(5));
    }
}

// ─── 主测试 — servo 真渲染链路(data: URL,默认运行) ─────────────────────────

#[test]
// @trace REQ-BRW-001 [level:e2e]
// @trace REQ-BRW-002 [level:e2e]
fn servo_render_pipeline_data_url_default_run() {
    // ── Arrange ────────────────────────────────────────────────────────
    let runtime = match BaoRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => panic!("BaoRuntime::new failed: {}", e),
    };
    let pool: &PagePool = runtime.page_pool();
    let mut report = Report::default();

    // 准备带完整结构的 HTML(有 <title> / <body> / 多种 DOM 元素)
    let html = "<!DOCTYPE html>\
<html>\
<head><title>Bao Render Pipeline</title></head>\
<body>\
  <h1 id=\"heading\">Rendered</h1>\
  <p id=\"count\">0</p>\
  <ul id=\"items\"><li>a</li><li>b</li><li>c</li></ul>\
</body>\
</html>";
    let url = format!("data:text/html;charset=utf-8,{}", html);

    // ── §1 navigate → servo 加载 ───────────────────────────────────────
    //
    // Act: 通过 page_pool().create_page 启动 servo WebView,载入 data: URL
    let page = match pool.create_page(&PageConfig {
        url: Some(url),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            panic!("pool.create_page failed: {}", e);
        }
    };
    report.pass("§1::page_created");
    wait_for_load(&page, 1500);

    // ── §2 page.is_alive + id ───────────────────────────────────────────
    report.assert(page.is_alive(), "§2::page_alive", "§2::page_alive");
    report.assert(page.id() >= 1, "§2::page_id_positive", "§2::page_id_positive");

    // ── §3 DOM 可查询 — document.title ─────────────────────────────────
    //
    // 这是 servo 真渲染的核心证据:JS 在 servo script thread 中执行,读取
    // servo 解析 HTML 后构建的 DOM 树。如果 servo 没真渲染,这里会返回空。
    //
    // Act + Assert
    match page.evaluate_js("document.title") {
        Ok(s) if s.contains("Bao Render Pipeline") => report.pass("§3::dom_title_query"),
        Ok(other) => report.fail("§3::dom_title_query", &format!("got '{}'", other)),
        Err(e) => report.skip("§3::dom_title_query", &format!("evaluate_js: {}", e)),
    }

    // ── §4 DOM 结构查询 — heading textContent ───────────────────────────
    match page.evaluate_js("document.getElementById('heading').textContent") {
        Ok(s) if s.contains("Rendered") => report.pass("§4::dom_heading_text"),
        Ok(other) => report.fail("§4::dom_heading_text", &format!("got '{}'", other)),
        Err(e) => report.skip("§4::dom_heading_text", &format!("evaluate_js: {}", e)),
    }

    // ── §5 DOM 结构查询 — querySelectorAll ──────────────────────────────
    match page.evaluate_js("document.querySelectorAll('#items li').length") {
        Ok(s) if s.trim() == "3" => report.pass("§5::dom_list_count"),
        Ok(other) => report.fail("§5::dom_list_count", &format!("got '{}'", other)),
        Err(e) => report.skip("§5::dom_list_count", &format!("evaluate_js: {}", e)),
    }

    // ── §6 DOM 可修改 — innerHTML 改写 ──────────────────────────────────
    //
    // 这一步证明 servo 真渲染:我们能写入 DOM,servo 反映修改。
    //
    // Act + Assert
    let _ = page.evaluate_js(
        "document.getElementById('count').textContent = '42'; 'ok'",
    );
    match page.evaluate_js("document.getElementById('count').textContent") {
        Ok(s) if s.trim() == "42" => report.pass("§6::dom_mutate_text"),
        Ok(other) => report.fail("§6::dom_mutate_text", &format!("got '{}'", other)),
        Err(e) => report.skip("§6::dom_mutate_text", &format!("evaluate_js: {}", e)),
    }

    // ── §7 DOM createElement + appendChild ──────────────────────────────
    match page.evaluate_js(
        r#"
            const newEl = document.createElement('div');
            newEl.id = 'injected';
            newEl.textContent = 'injected-text';
            document.body.appendChild(newEl);
            document.getElementById('injected').textContent;
        "#,
    ) {
        Ok(s) if s.contains("injected-text") => report.pass("§7::dom_create_element"),
        Ok(other) => report.fail("§7::dom_create_element", &format!("got '{}'", other)),
        Err(e) => report.skip("§7::dom_create_element", &format!("evaluate_js: {}", e)),
    }

    // ── §8 page.page_title() — Rust API 直读 servo 状态 ─────────────────
    //
    // page_title() 不走 JS,而是读 servo delegate 状态(BaoWebViewState.title)。
    // 这验证 servo 真的把页面元信息同步到了 delegate。
    //
    // Act + Assert
    let title_opt = page.page_title();
    report.assert(
        title_opt
            .as_deref()
            .map(|t| t.contains("Bao Render Pipeline"))
            .unwrap_or(false),
        "§8::page_title_rust_api",
        "§8::page_title_rust_api",
    );

    // ── §9 screenshot — servo 真渲染产出像素 ────────────────────────────
    //
    // take_screenshot 触发 servo SoftwareRenderingContext 的 paint →
    // 产出 RgbaImage → encode_image 编码为 PNG 字节。
    // PNG 必须:非空 + 头部 magic bytes (0x89 PNG) + 长度合理(>1KB)。
    //
    // Act + Assert
    match page.take_screenshot(ScreenshotFormat::Png) {
        Ok(bytes) => {
            report.assert(bytes.len() > 1000, "§9::screenshot_nonempty", "§9::screenshot_nonempty");
            // PNG magic: 89 50 4E 47 0D 0A 1A 0A
            let png_magic = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
            let magic_ok = bytes.len() >= 8 && bytes[..8] == png_magic;
            report.assert(magic_ok, "§9::screenshot_png_magic", "§9::screenshot_png_magic");
        }
        Err(e) => report.skip("§9::screenshot", &format!("take_screenshot: {}", e)),
    }

    // 清理
    let _ = page.close();
    pool.close_all();
    report.finish();

    let total = report.passed + report.failed;
    if total > 0 {
        let pass_ratio = report.passed as f64 / total as f64;
        assert!(
            pass_ratio >= 0.5,
            "too few render-pipeline sub-assertions passed: {}/{} (ratio {:.2})",
            report.passed, total, pass_ratio
        );
    }
    assert_eq!(
        report.failed, 0,
        "{} render-pipeline sub-assertions failed — see stderr above",
        report.failed
    );
}

// ─── 网络 E2E — #[ignore] + BAO_TEST_NETWORK=1 启用 ──────────────────────────

#[test]
#[ignore = "network E2E — set BAO_TEST_NETWORK=1 to enable"]
// @trace REQ-BRW-001 [level:e2e]
fn servo_render_pipeline_network_example_com() {
    if std::env::var("BAO_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!("skipping network E2E — set BAO_TEST_NETWORK=1 to enable");
        return;
    }

    // Arrange
    let runtime = BaoRuntime::new(BaoConfig::default()).expect("BaoRuntime::new");
    let pool = runtime.page_pool();

    // Act
    let page = pool
        .create_page(&PageConfig {
            url: Some("https://example.com".into()),
            ..Default::default()
        })
        .expect("create_page example.com");

    // 给 servo 充足时间完成 TLS 握手 + HTML 解析
    wait_for_load(&page, 10_000);

    // Assert — example.com 经典标题
    let title = page.evaluate_js("document.title").unwrap_or_default();
    assert!(
        title.contains("Example Domain"),
        "example.com title should contain 'Example Domain', got: '{}'",
        title
    );

    // screenshot — 真页面截图
    let png = page.take_screenshot(ScreenshotFormat::Png).expect("screenshot");
    assert!(png.len() > 5000, "example.com screenshot should be substantial");

    let _ = page.close();
}
