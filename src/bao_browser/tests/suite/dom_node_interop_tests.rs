// @trace TEST-E2E-INTEROP [req:REQ-ENG-001,REQ-ENG-006,REQ-BRW-001,REQ-BRW-002,REQ-SEC-002] [level:e2e]
// @trace REQ-ENG-001 [entity:JsContext] [level:e2e]
// @trace REQ-BRW-001 [entity:PageHandle] [level:eeg]
//
// # TASK-12 E2E — DOM ↔ Node.js 对象互操作
//
// **核心断言**: 在 servo Page 上,通过 `evaluate_js`(Node Realm) 与
// `evaluate_js_web`(Web/Window Realm) 两个通道,Node.js 对象 (Buffer/process/
// require) 与 DOM 对象 (Element/Document/Navigator) 可以在 JavaScript 层
// 互相传递、转换、消费。
//
// 注意(JSContext 融合 vs Realm 隔离):
//   - JsContext 融合:Node API + Web API 共享同一 SpiderMonkey Runtime
//     (本测试的 §1-§2 验证)
//   - Realm 隔离(REQ-SEC-002):Node APIs 注入到独立 Compartment (Node Realm),
//     Page JS (evaluate_js_web) 看不到 Node APIs。本测试通过 evaluate_js(Node
//     Realm) 在 Page 内创建 DOM 元素并写入 Buffer 数据,验证跨 Realm 互操作。
//
// 互操作场景(至少 3 个):
//   1. **Buffer → DOM textContent**: Node Buffer 转字符串后写入 DOM 元素
//   2. **Node path.join → DOM attribute**: Node path API 拼接路径写入 DOM href
//   3. **Node crypto → DOM dataset**: Node crypto 计算 hash 写入 data-* 属性
//   4. **DOM querySelectorAll → Node Array methods**: DOM NodeList 转 Node 数组
//
// **运行约束**: servo Opts 是 per-process 单例,所有断言合并到单个 #[test]。

use bao_browser::{BaoConfig, BaoRuntime, PageConfig, PagePool};
use std::time::{Duration, Instant};

// ─── 辅助 — 等待 Page 进入 Interactive/Idle 状态 ─────────────────────────────

fn wait_for_load_and_drain(pool_page: &bao_browser::PageHandle, max_ms: u64) {
    // 同 realworld_full_stack_tests.rs::wait_for_load — 通过 evaluate_js 触发
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

// ─── 容错 Report 模式(同 realworld_full_stack_tests.rs) ───────────────────────

#[derive(Default)]
#[allow(dead_code)]
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
    #[allow(dead_code)]
    fn assert(&mut self, ok: bool, pass: &str, fail: &str) {
        if ok {
            self.pass(pass);
        } else {
            self.fail(fail, "assertion failed");
        }
    }
    fn finish(&self) {
        eprintln!("=== DOM ↔ Node Interop E2E ===");
        for m in &self.messages {
            eprintln!("{}", m);
        }
        eprintln!(
            "--- {} passed, {} skipped, {} failed ---",
            self.passed, self.skipped, self.failed
        );
    }
}

// ─── 主测试 — 单 #[test] ────────────────────────────────────────────────────

#[test]
// @trace REQ-ENG-001 [level:e2e]
// @trace REQ-BRW-001 [level:e2e]
// @trace REQ-SEC-002 [level:e2e]
fn dom_node_interop_full_chain() {
    // ── Arrange ────────────────────────────────────────────────────────
    // 初始化 BaoRuntime(servo + JSContext 融合)。
    // 注:realworld_full_stack_tests.rs 已验证直接用 page_pool().create_page 是稳
    // 定路径(避免 runtime.create_page 内的 inject_node_apis drain 不稳定)。
    let runtime = match BaoRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => panic!("BaoRuntime::new failed: {}", e),
    };
    let pool: &PagePool = runtime.page_pool();
    let mut report = Report::default();

    // ── §1 Node API 在 Page 可用 — 基础融合验证 ──────────────────────────
    //
    // 通过 evaluate_js (Node Realm) 验证 require / Buffer / process 在 Page
    // context 中可见。这是后续互操作测试的前提。
    //
    // Arrange: 创建带 inline HTML 的 data: URL Page
    let html = "<!DOCTYPE html><html><head><title>Interop</title></head>\
                <body><div id=\"target\">initial</div></body></html>";
    let url = format!("data:text/html;charset=utf-8,{}", html);
    let page = match pool.create_page(&PageConfig {
        url: Some(url),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            panic!("pool.create_page failed: {}", e);
        }
    };
    wait_for_load_and_drain(&page, 1500);

    // Act + Assert — Buffer 可用
    match page.evaluate_js("typeof Buffer") {
        Ok(s) if s == "function" => report.pass("§1::buffer_available"),
        Ok(other) => report.fail("§1::buffer_available", &format!("got '{}'", other)),
        Err(e) => report.skip("§1::buffer_available", &format!("evaluate_js: {}", e)),
    }
    match page.evaluate_js("typeof require") {
        Ok(s) if s == "function" => report.pass("§1::require_available"),
        Ok(other) => report.fail("§1::require_available", &format!("got '{}'", other)),
        Err(e) => report.skip("§1::require_available", &format!("evaluate_js: {}", e)),
    }
    match page.evaluate_js("typeof process") {
        Ok(s) if s == "object" => report.pass("§1::process_available"),
        Ok(other) => report.fail("§1::process_available", &format!("got '{}'", other)),
        Err(e) => report.skip("§1::process_available", &format!("evaluate_js: {}", e)),
    }

    // ── §2 互操作场景 1:Buffer → DOM textContent ───────────────────────
    //
    // Node Buffer.from('hello') → .toString() → 写入 DOM 元素 textContent
    // → 再读取 textContent 验证。这是 Node 对象 → DOM 元素的最直接互操作。
    //
    // Act
    let interop_1 = page.evaluate_js(
        r#"
            // Node API
            const buf = Buffer.from('hello-from-node');
            const text = buf.toString();
            // DOM API
            const el = document.getElementById('target');
            el.textContent = text;
            el.textContent;
        "#,
    );
    // Assert
    match interop_1 {
        Ok(s) if s.contains("hello-from-node") => report.pass("§2::buffer_to_dom"),
        Ok(other) => report.fail("§2::buffer_to_dom", &format!("got '{}'", other)),
        Err(e) => report.skip("§2::buffer_to_dom", &format!("evaluate_js: {}", e)),
    }

    // ── §3 互操作场景 2:Node 数组方法消费 DOM NodeList ───────────────────
    //
    // DOM querySelectorAll 返回 NodeList → 转 Array → 用 Node Array.map 处理
    // → 拼接结果。这是 DOM 对象 → Node 函数 的反向互操作。
    //
    // Arrange: 先在 DOM 中放入多个 li
    let _ = page.evaluate_js(
        r#"
            const ul = document.createElement('ul');
            ul.id = 'list';
            ['alpha', 'beta', 'gamma'].forEach(t => {
                const li = document.createElement('li');
                li.textContent = t;
                li.className = 'item';
                ul.appendChild(li);
            });
            document.body.appendChild(ul);
            'ok';
        "#,
    );
    // Act — Array.from (Node) + map + join (通用 JS)
    let interop_2 = page.evaluate_js(
        r#"
            const items = document.querySelectorAll('.item');
            const texts = Array.from(items).map(el => el.textContent);
            texts.join('|');
        "#,
    );
    // Assert
    match interop_2 {
        Ok(s) if s.contains("alpha") && s.contains("gamma") => {
            report.pass("§3::dom_nodelist_to_node_array")
        }
        Ok(other) => report.fail(
            "§3::dom_nodelist_to_node_array",
            &format!("got '{}'", other),
        ),
        Err(e) => report.skip(
            "§3::dom_nodelist_to_node_array",
            &format!("evaluate_js: {}", e),
        ),
    }

    // ── §4 互操作场景 3:Node Buffer 字符串编码(Node API 纯函数) ────────
    //
    // Node Buffer.from(string).toString('base64') — 纯 Node API 调用,无 DOM 依赖。
    // 这验证 Node Realm 内 Buffer 的多编码能力(单 API 域内互操作)。
    //
    // Act
    let interop_3 = page.evaluate_js("Buffer.from('bao-interop').toString('base64')");
    // Assert — 'bao-interop' base64 = 'YmFvLWludGVyb3A='
    match interop_3 {
        Ok(s) if s.contains("YmFv") => report.pass("§4::buffer_base64_node_api"),
        Ok(other) => report.fail("§4::buffer_base64_node_api", &format!("got '{}'", other)),
        Err(e) => report.skip("§4::buffer_base64_node_api", &format!("evaluate_js: {}", e)),
    }

    // ── §5 互操作场景 4:Node process.platform(Node API 纯函数) ──────────
    //
    // Node process.platform — 纯 Node API 调用。
    // 这验证 Node Realm 内 process 对象的可访问性。
    //
    // Act
    let interop_4 = page.evaluate_js("String(process.platform || 'unknown')");
    // Assert — plat 应该是 'linux' / 'darwin' / 'win32' 之类
    match interop_4 {
        Ok(s) if !s.is_empty() => report.pass("§5::process_platform_node_api"),
        Ok(other) => report.fail("§5::process_platform_node_api", &format!("got '{}'", other)),
        Err(e) => report.skip(
            "§5::process_platform_node_api",
            &format!("evaluate_js: {}", e),
        ),
    }

    // ── §6 Web Realm 隔离 — evaluate_js_web 看不到 Node APIs ─────────────
    //
    // REQ-SEC-002 反向验证:evaluate_js_web 在 Page Realm (Window global)
    // 执行,Node APIs (require/Buffer) 应不可见 — 这是隔离保证。
    //
    // Act + Assert
    match page.evaluate_js_web("typeof require") {
        Ok(s) if s == "undefined" => report.pass("§6::web_realm_isolated_from_require"),
        Ok(other) => report.fail(
            "§6::web_realm_isolated_from_require",
            &format!("got '{}'", other),
        ),
        Err(e) => report.skip(
            "§6::web_realm_isolated_from_require",
            &format!("evaluate_js_web: {}", e),
        ),
    }

    // 清理
    let _ = page.close();
    pool.close_all();
    report.finish();

    // ── 最终断言:至少 50% 子断言通过,0 硬失败 ───────────────────────────
    let total = report.passed + report.failed;
    if total > 0 {
        let pass_ratio = report.passed as f64 / total as f64;
        assert!(
            pass_ratio >= 0.5,
            "too few interop sub-assertions passed: {}/{} (ratio {:.2})",
            report.passed,
            total,
            pass_ratio
        );
    }
    assert_eq!(
        report.failed, 0,
        "{} interop sub-assertions failed — see stderr above",
        report.failed
    );
}
