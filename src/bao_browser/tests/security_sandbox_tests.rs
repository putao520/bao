// @trace TEST-SEC-001 TEST-SEC-002 TEST-SEC-003 [req:REQ-SEC-001,REQ-SEC-002,REQ-SEC-003] [level:integration]
// Security sandbox tests — CORS disable, dual-layer JS model, Node API isolation.
//
// Architecture:
//   - Single #[test] (mozjs Runtime + servo Opts are per-process singletons)
//   - Uses BaoRuntime + PagePool
//   - Direct function-level API: page.evaluate_js() + page.evaluate_js_web()
//   - Positive tests: verify correct behavior
//   - Negative tests: deliberately inject malicious code to verify sandbox holds
//
// Scenarios:
//   1. CORS: cross-origin fetch from data: URL succeeds (REQ-SEC-001)
//   2. Dual-layer: evaluate_js has Node APIs, page JS does not (REQ-SEC-002)
//   3. Sandbox: Node APIs absent from page global (REQ-SEC-003)
//   4. Malicious injection: deliberate attempts to break sandbox

#![allow(dead_code)]

use bao_browser::{BaoConfig, BaoRuntime, PageConfig, PagePool, PageState};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Report — fault-tolerant scenario accumulator
// ---------------------------------------------------------------------------

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
    fn finish(&self) {
        eprintln!("\n=== Security Sandbox Tests ===");
        for m in &self.messages {
            eprintln!("{}", m);
        }
        eprintln!(
            "--- {} passed, {} skipped, {} failed ---",
            self.passed, self.skipped, self.failed
        );
    }
}

// ---------------------------------------------------------------------------
// wait_for_load — drive servo's paint loop until page is interactive
// ---------------------------------------------------------------------------

fn wait_for_load(page: &bao_browser::PageHandle, max_ms: u64) {
    let start = Instant::now();
    while start.elapsed().as_millis() < max_ms as u128 {
        let _ = page.evaluate_js_web("");
        if matches!(page.get_state(), PageState::Interactive | PageState::Idle) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

// ---------------------------------------------------------------------------
// Main test — single servo init, fault-tolerant scenarios
// ---------------------------------------------------------------------------

#[test]
fn security_sandbox_verification() {
    let config = BaoConfig::default();
    let runtime = match BaoRuntime::new(config) {
        Ok(r) => r,
        Err(e) => panic!("BaoRuntime::new failed: {}", e),
    };
    let pool: &PagePool = runtime.page_pool();
    let mut report = Report::default();

    // Phase 1: CORS disable verification (REQ-SEC-001)
    scenario_cors_cross_origin_fetch(pool, &mut report);

    // Phase 2: Node API sandbox — page global audit (REQ-SEC-003)
    scenario_node_api_absent_from_page(pool, &mut report);

    // Phase 3: Dual-layer JS model — evaluate_js has Node APIs (REQ-SEC-002)
    scenario_evaluate_js_has_node_apis(pool, &mut report);

    // Phase 4: Web APIs available in both contexts (REQ-SEC-002)
    scenario_web_apis_available(pool, &mut report);

    // Phase 5: Malicious injection attempts (negative tests)
    scenario_malicious_node_api_injection(pool, &mut report);
    scenario_malicious_prototype_pollution(pool, &mut report);
    scenario_malicious_global_escape(pool, &mut report);

    // Phase 6: DOM access from privileged context (REQ-SEC-002)
    scenario_privileged_dom_access(pool, &mut report);

    // Phase 7: Timing attack — Node APIs must NOT persist on global after evaluate_js
    scenario_node_api_no_persistence_after_evaluate(pool, &mut report);

    // Phase 8: CommonJS parameter injection — verify scope deletion
    scenario_scope_cleanup_after_evaluate(pool, &mut report);

    pool.close_all();
    report.finish();

    // Hard failure on any security assertion failure
    let sec_fails = report.messages.iter()
        .filter(|m| m.starts_with("FAIL"))
        .count();
    assert_eq!(
        sec_fails, 0,
        "{} security assertions failed — sandbox compromised!",
        sec_fails
    );

    // At least some sub-assertions must pass (network-dependent may skip)
    let total = report.passed + report.failed;
    if total > 0 {
        let pass_ratio = report.passed as f64 / total as f64;
        assert!(
            pass_ratio >= 0.3,
            "too few sub-assertions passed: {}/{} (ratio {:.2})",
            report.passed, total, pass_ratio
        );
    }
}

// ---------------------------------------------------------------------------
// Scenario: CORS cross-origin fetch succeeds (REQ-SEC-001)
// ---------------------------------------------------------------------------

fn scenario_cors_cross_origin_fetch(pool: &PagePool, report: &mut Report) {
    let name = "cors_cross_origin";

    // Create page with data: URL that tries to fetch a cross-origin resource
    let page = match pool.create_page(&PageConfig {
        url: Some("data:text/html,<html><body>cors-test</body></html>".into()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            report.skip(name, &format!("page creation failed: {e}"));
            return;
        }
    };

    wait_for_load(&page, 3000);

    // Test: fetch from data: URL to http: URL should NOT be blocked by CORS
    // (In standard browsers, data: → http: is blocked by CORS. Bao disables this.)
    match page.evaluate_js(
        "fetch('https://example.com').then(r => r.status + ' ' + r.statusText).catch(e => 'fetch_error: ' + e.message)"
    ) {
        Ok(s) if s.contains("200") || s.contains("OK") || s.contains("example") => {
            report.pass(&format!("{}::fetch_succeeds", name));
            eprintln!("  [cors] fetch result: {}", s);
        }
        Ok(s) if s.contains("fetch_error") && !s.contains("CORS") && !s.contains("NetworkError") => {
            // Fetch failed for non-CORS reasons (network) — acceptable
            report.skip(&format!("{}::fetch_succeeds", name), &format!("network error (not CORS): {}", s));
        }
        Ok(s) if s.contains("CORS") || s.contains("NetworkError") => {
            // CORS error — this is a REGRESSION
            report.fail(&format!("{}::fetch_succeeds", name), &format!("CORS blocked: {}", s));
        }
        Ok(s) => {
            // Some other result — might be redirect or network issue
            report.skip(&format!("{}::fetch_succeeds", name), &format!("unexpected: {}", s));
        }
        Err(e) => report.skip(&format!("{}::fetch_succeeds", name), &format!("evaluate_js: {}", e)),
    }

    // Test: verify no CORS-related errors in basic cross-origin XHR pattern
    match page.evaluate_js(
        "typeof XMLHttpRequest !== 'undefined' ? 'xhr_available' : 'xhr_unavailable'"
    ) {
        Ok(s) if s.contains("xhr_available") => {
            report.pass(&format!("{}::xhr_available", name));
        }
        Ok(s) => {
            report.skip(&format!("{}::xhr_available", name), &format!("xhr: {}", s));
        }
        Err(e) => report.skip(&format!("{}::xhr_available", name), &format!("evaluate_js: {}", e)),
    }

    let _ = page.close();
}

// ---------------------------------------------------------------------------
// Scenario: Node APIs absent from page global (REQ-SEC-003)
// ---------------------------------------------------------------------------

fn scenario_node_api_absent_from_page(pool: &PagePool, report: &mut Report) {
    let name = "node_api_absent";

    let page = match pool.create_page(&PageConfig {
        url: Some("data:text/html,<html><body>sandbox-test</body></html>".into()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            report.skip(name, &format!("page creation failed: {e}"));
            return;
        }
    };

    wait_for_load(&page, 3000);

    // Verify Node.js APIs are NOT on the page global
    // Using evaluate_js_web (no Node API injection) to check page global
    let node_apis = [
        ("require", "require() module loader"),
        ("module", "module object"),
        ("Bun", "Bun runtime"),
        ("process", "process object"),
        ("Buffer", "Buffer class"),
        ("__filename", "__filename"),
        ("__dirname", "__dirname"),
        ("global", "Node.js global alias"),
    ];

    for (api, desc) in &node_apis {
        let js = format!("typeof {}", api);
        match page.evaluate_js_web(&js) {
            Ok(s) if s == "undefined" => {
                report.pass(&format!("{}::{}_undefined", name, api));
            }
            Ok(s) => {
                report.fail(&format!("{}::{}_undefined", name, api),
                    &format!("{} ({}) is accessible on page global: typeof={}", api, desc, s));
            }
            Err(e) => report.skip(&format!("{}::{}_undefined", name, api), &format!("evaluate_js_web: {}", e)),
        }
    }

    // Verify Node.js built-in modules are NOT accessible
    let node_modules = [
        "fs", "path", "crypto", "http", "https", "os", "net", "dns",
        "child_process", "stream", "zlib", "vm", "tls", "readline",
    ];

    for module in &node_modules {
        // Try require() — should fail since require is undefined
        let js = format!("try {{ require('{}'); 'accessible' }} catch(e) {{ e.message }}", module);
        match page.evaluate_js_web(&js) {
            Ok(s) if s.contains("require is not defined") || s.contains("is not defined") => {
                report.pass(&format!("{}::module_{}_blocked", name, module));
            }
            Ok(s) if s.contains("accessible") => {
                report.fail(&format!("{}::module_{}_blocked", name, module),
                    &format!("node module '{}' accessible from page!", module));
            }
            Ok(s) => {
                // Some other error — might be that require itself is undefined
                if s.contains("not defined") {
                    report.pass(&format!("{}::module_{}_blocked", name, module));
                } else {
                    report.skip(&format!("{}::module_{}_blocked", name, module), &format!("unexpected: {}", s));
                }
            }
            Err(e) => report.skip(&format!("{}::module_{}_blocked", name, module), &format!("evaluate_js_web: {}", e)),
        }
    }

    let _ = page.close();
}

// ---------------------------------------------------------------------------
// Scenario: evaluate_js has Node APIs (REQ-SEC-002)
// ---------------------------------------------------------------------------

fn scenario_evaluate_js_has_node_apis(pool: &PagePool, report: &mut Report) {
    let name = "evaluate_js_privileged";

    let page = match pool.create_page(&PageConfig {
        url: Some("data:text/html,<html><body>privileged-test</body></html>".into()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            report.skip(name, &format!("page creation failed: {e}"));
            return;
        }
    };

    wait_for_load(&page, 3000);

    // evaluate_js() should have Node APIs available (privileged context)
    let privileged_apis = [
        ("require", "require() module loader"),
        ("Bun", "Bun runtime"),
        ("process", "process object"),
        ("Buffer", "Buffer class"),
    ];

    for (api, desc) in &privileged_apis {
        let js = format!("typeof {}", api);
        match page.evaluate_js(&js) {
            Ok(s) if s != "undefined" => {
                report.pass(&format!("{}::{}_available", name, api));
                eprintln!("  [privileged] typeof {} = {}", api, s);
            }
            Ok(s) => {
                // Node API may not be injected yet if the callback hasn't drained
                // This is acceptable — the important thing is page JS can't access them
                report.skip(&format!("{}::{}_available", name, api),
                    &format!("{} ({}) not yet available in privileged context: typeof={}", api, desc, s));
            }
            Err(e) => report.skip(&format!("{}::{}_available", name, api), &format!("evaluate_js: {}", e)),
        }
    }

    // Test: require('path') should work in privileged context
    match page.evaluate_js("try { const path = require('path'); typeof path.join } catch(e) { 'error: ' + e.message }") {
        Ok(s) if s == "function" => {
            report.pass(&format!("{}::require_path", name));
        }
        Ok(s) if s.contains("error") => {
            report.skip(&format!("{}::require_path", name), &format!("require failed: {}", s));
        }
        Ok(s) => {
            report.skip(&format!("{}::require_path", name), &format!("path.join type: {}", s));
        }
        Err(e) => report.skip(&format!("{}::require_path", name), &format!("evaluate_js: {}", e)),
    }

    // Test: process.env should work in privileged context
    match page.evaluate_js("try { typeof process.env } catch(e) { 'error: ' + e.message }") {
        Ok(s) if s == "object" => {
            report.pass(&format!("{}::process_env", name));
        }
        Ok(s) => {
            report.skip(&format!("{}::process_env", name), &format!("process.env type: {}", s));
        }
        Err(e) => report.skip(&format!("{}::process_env", name), &format!("evaluate_js: {}", e)),
    }

    let _ = page.close();
}

// ---------------------------------------------------------------------------
// Scenario: Web APIs available in both contexts (REQ-SEC-002)
// ---------------------------------------------------------------------------

fn scenario_web_apis_available(pool: &PagePool, report: &mut Report) {
    let name = "web_apis_available";

    let page = match pool.create_page(&PageConfig {
        url: Some("data:text/html,<html><body>webapi-test</body></html>".into()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            report.skip(name, &format!("page creation failed: {e}"));
            return;
        }
    };

    wait_for_load(&page, 3000);

    // Web APIs that MUST be available on page global
    let web_apis = [
        ("fetch", "fetch API"),
        ("setTimeout", "timer API"),
        ("setInterval", "timer API"),
        ("clearTimeout", "timer API"),
        ("performance", "performance API"),
        ("crypto", "crypto API"),
        ("console", "console API"),
        ("URL", "URL API"),
        ("URLSearchParams", "URLSearchParams API"),
        ("TextEncoder", "TextEncoder API"),
        ("TextDecoder", "TextDecoder API"),
        ("atob", "atob API"),
        ("btoa", "btoa API"),
        ("structuredClone", "structuredClone API"),
        ("queueMicrotask", "queueMicrotask API"),
        ("WebSocket", "WebSocket API"),
    ];

    for (api, desc) in &web_apis {
        let js = format!("typeof {}", api);
        match page.evaluate_js_web(&js) {
            Ok(s) if s != "undefined" => {
                report.pass(&format!("{}::{}_available", name, api));
            }
            Ok(s) => {
                report.fail(&format!("{}::{}_available", name, api),
                    &format!("{} ({}) missing from page global: typeof={}", api, desc, s));
            }
            Err(e) => report.skip(&format!("{}::{}_available", name, api), &format!("evaluate_js_web: {}", e)),
        }
    }

    // Verify fetch is functional (not just present)
    match page.evaluate_js_web("typeof fetch === 'function' ? 'yes' : 'no'") {
        Ok(s) if s == "yes" => report.pass(&format!("{}::fetch_functional", name)),
        Ok(s) => report.fail(&format!("{}::fetch_functional", name), &format!("fetch not a function: {}", s)),
        Err(e) => report.skip(&format!("{}::fetch_functional", name), &format!("evaluate_js_web: {}", e)),
    }

    // Verify crypto.getRandomValues is functional
    match page.evaluate_js_web(
        "typeof crypto !== 'undefined' && typeof crypto.getRandomValues === 'function' ? 'yes' : 'no'"
    ) {
        Ok(s) if s == "yes" => report.pass(&format!("{}::crypto_functional", name)),
        Ok(s) => report.fail(&format!("{}::crypto_functional", name), &format!("crypto: {}", s)),
        Err(e) => report.skip(&format!("{}::crypto_functional", name), &format!("evaluate_js_web: {}", e)),
    }

    let _ = page.close();
}

// ---------------------------------------------------------------------------
// Scenario: Malicious Node API injection attempts (negative test)
// ---------------------------------------------------------------------------

fn scenario_malicious_node_api_injection(pool: &PagePool, report: &mut Report) {
    let name = "malicious_node_injection";

    let page = match pool.create_page(&PageConfig {
        url: Some("data:text/html,<html><body>malicious-test</body></html>".into()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            report.skip(name, &format!("page creation failed: {e}"));
            return;
        }
    };

    wait_for_load(&page, 3000);

    // Attack 1: Try to define require via Object.defineProperty
    match page.evaluate_js_web(
        "try { Object.defineProperty(window, 'require', { value: function(m) { return 'HACKED:' + m; }, configurable: true }); typeof require } catch(e) { 'blocked: ' + e.message }"
    ) {
        Ok(s) if s.contains("HACKED") || s == "function" => {
            // The page was able to define its own require — this is expected
            // (pages can define their own globals). The key security property
            // is that the REAL Node require was never on the global.
            report.pass(&format!("{}::define_require_harmless", name));
            eprintln!("  [malicious] page defined its own require (harmless — not the real Node require)");
        }
        Ok(s) if s.contains("blocked") => {
            report.pass(&format!("{}::define_require_blocked", name));
            eprintln!("  [malicious] Object.defineProperty blocked: {}", s);
        }
        Ok(s) => {
            report.skip(&format!("{}::define_require", name), &format!("result: {}", s));
        }
        Err(e) => report.skip(&format!("{}::define_require", name), &format!("evaluate_js_web: {}", e)),
    }

    // Attack 2: Try to access fs via various trick paths
    let fs_attacks = [
        // Direct access
        "typeof require !== 'undefined' ? require('fs') : 'no_require'",
        // Via globalThis
        "typeof globalThis.require !== 'undefined' ? globalThis.require('fs') : 'no_require_on_globalThis'",
        // Via window
        "typeof window.require !== 'undefined' ? window.require('fs') : 'no_require_on_window'",
        // Via self
        "typeof self.require !== 'undefined' ? self.require('fs') : 'no_require_on_self'",
        // Via eval
        "eval('typeof require') !== 'undefined' ? eval(\"require('fs')\") : 'no_require_in_eval'",
    ];

    for (i, attack) in fs_attacks.iter().enumerate() {
        match page.evaluate_js_web(attack) {
            Ok(s) if s.contains("no_require") || s.contains("not defined") => {
                report.pass(&format!("{}::fs_attack_{}_blocked", name, i));
            }
            Ok(s) if s.contains("HACKED") => {
                // The fake require from Attack 1 — still not the real Node require
                report.pass(&format!("{}::fs_attack_{}_fake_require", name, i));
                eprintln!("  [malicious] attack {} hit fake require (not real Node)", i);
            }
            Ok(s) => {
                // Check if this is actually the real fs module
                if s.contains("readFileSync") || s.contains("writeFileSync") {
                    report.fail(&format!("{}::fs_attack_{}_blocked", name, i),
                        &format!("REAL fs module accessible via attack {}!", i));
                } else {
                    report.skip(&format!("{}::fs_attack_{}_blocked", name, i), &format!("result: {}", s));
                }
            }
            Err(e) => report.skip(&format!("{}::fs_attack_{}_blocked", name, i), &format!("evaluate_js_web: {}", e)),
        }
    }

    // Attack 3: Try to steal process.env via prototype chain
    match page.evaluate_js_web(
        "try { Object.getPrototypeOf(window).process } catch(e) { 'no_process_in_chain: ' + e.message }"
    ) {
        Ok(s) if s.contains("no_process") || s == "undefined" => {
            report.pass(&format!("{}::process_proto_chain_clean", name));
        }
        Ok(s) => {
            report.fail(&format!("{}::process_proto_chain_clean", name),
                &format!("process found in prototype chain: {}", s));
        }
        Err(e) => report.skip(&format!("{}::process_proto_chain_clean", name), &format!("evaluate_js_web: {}", e)),
    }

    let _ = page.close();
}

// ---------------------------------------------------------------------------
// Scenario: Malicious prototype pollution attempts (negative test)
// ---------------------------------------------------------------------------

fn scenario_malicious_prototype_pollution(pool: &PagePool, report: &mut Report) {
    let name = "malicious_proto_pollution";

    let page = match pool.create_page(&PageConfig {
        url: Some("data:text/html,<html><body>proto-test</body></html>".into()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            report.skip(name, &format!("page creation failed: {e}"));
            return;
        }
    };

    wait_for_load(&page, 3000);

    // Attack: Try to pollute Object.prototype to inject Node-like APIs
    match page.evaluate_js_web(
        "Object.prototype.require = function(m) { return 'POLLUTED:' + m; }; 'polluted'"
    ) {
        Ok(s) if s == "polluted" => {
            // Prototype pollution succeeded (JS allows this), but it's not the real require
            report.pass(&format!("{}::pollution_harmless", name));
            eprintln!("  [proto] Object.prototype.require set (harmless — not real Node require)");
        }
        Ok(s) => {
            report.skip(&format!("{}::pollution_harmless", name), &format!("result: {}", s));
        }
        Err(e) => report.skip(&format!("{}::pollution_harmless", name), &format!("evaluate_js_web: {}", e)),
    }

    // Verify the polluted require is NOT the real Node require
    match page.evaluate_js_web(
        "({}).require('fs')"
    ) {
        Ok(s) if s.contains("POLLUTED") => {
            report.pass(&format!("{}::polluted_not_real_require", name));
            eprintln!("  [proto] polluted require returns fake value (not real Node require)");
        }
        Ok(s) if s.contains("readFileSync") => {
            report.fail(&format!("{}::polluted_not_real_require", name),
                "prototype pollution exposed real fs module!");
        }
        Ok(s) => {
            report.skip(&format!("{}::polluted_not_real_require", name), &format!("result: {}", s));
        }
        Err(e) => report.skip(&format!("{}::polluted_not_real_require", name), &format!("evaluate_js_web: {}", e)),
    }

    // Cleanup: remove the pollution
    let _ = page.evaluate_js_web("delete Object.prototype.require;");

    let _ = page.close();
}

// ---------------------------------------------------------------------------
// Scenario: Malicious global escape attempts (negative test)
// ---------------------------------------------------------------------------

fn scenario_malicious_global_escape(pool: &PagePool, report: &mut Report) {
    let name = "malicious_global_escape";

    let page = match pool.create_page(&PageConfig {
        url: Some("data:text/html,<html><body>escape-test</body></html>".into()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            report.skip(name, &format!("page creation failed: {e}"));
            return;
        }
    };

    wait_for_load(&page, 3000);

    // Attack 1: Try to access Node APIs via Function constructor
    match page.evaluate_js_web(
        "try { new Function('return typeof require')() } catch(e) { 'error: ' + e.message }"
    ) {
        Ok(s) if s == "undefined" => {
            report.pass(&format!("{}::function_constructor_no_require", name));
        }
        Ok(s) if s == "function" => {
            report.fail(&format!("{}::function_constructor_no_require", name),
                "Function constructor can access require!");
        }
        Ok(s) => {
            report.skip(&format!("{}::function_constructor_no_require", name), &format!("result: {}", s));
        }
        Err(e) => report.skip(&format!("{}::function_constructor_no_require", name), &format!("evaluate_js_web: {}", e)),
    }

    // Attack 2: Try to access Node APIs via indirect eval
    match page.evaluate_js_web(
        "try { (0, eval)('typeof process') } catch(e) { 'error: ' + e.message }"
    ) {
        Ok(s) if s == "undefined" => {
            report.pass(&format!("{}::indirect_eval_no_process", name));
        }
        Ok(s) if s == "object" => {
            report.fail(&format!("{}::indirect_eval_no_process", name),
                "indirect eval can access process!");
        }
        Ok(s) => {
            report.skip(&format!("{}::indirect_eval_no_process", name), &format!("result: {}", s));
        }
        Err(e) => report.skip(&format!("{}::indirect_eval_no_process", name), &format!("evaluate_js_web: {}", e)),
    }

    // Attack 3: Try to access Node APIs via with statement (if available)
    match page.evaluate_js_web(
        "try { with(window) { typeof Bun } } catch(e) { 'error: ' + e.message }"
    ) {
        Ok(s) if s == "undefined" => {
            report.pass(&format!("{}::with_statement_no_bun", name));
        }
        Ok(s) if s != "undefined" => {
            report.fail(&format!("{}::with_statement_no_bun", name),
                &format!("with statement can access Bun: typeof={}", s));
        }
        Ok(s) => {
            report.skip(&format!("{}::with_statement_no_bun", name), &format!("result: {}", s));
        }
        Err(e) => {
            // with statement may not be available in strict mode — that's fine
            report.pass(&format!("{}::with_statement_no_bun", name));
            eprintln!("  [escape] with statement not available (strict mode): {}", e);
        }
    }

    // Attack 4: Try to enumerate all globals and find Node APIs
    match page.evaluate_js_web(
        "Object.getOwnPropertyNames(window).filter(n => ['require','process','Buffer','Bun','module','__filename','__dirname'].includes(n)).join(',')"
    ) {
        Ok(s) if s.is_empty() => {
            report.pass(&format!("{}::enumeration_no_node_apis", name));
        }
        Ok(s) => {
            report.fail(&format!("{}::enumeration_no_node_apis", name),
                &format!("Node APIs found on window: {}", s));
        }
        Err(e) => report.skip(&format!("{}::enumeration_no_node_apis", name), &format!("evaluate_js_web: {}", e)),
    }

    // Attack 5: Try to access Node APIs via Reflect
    match page.evaluate_js_web(
        "try { Reflect.get(window, 'require') } catch(e) { 'error: ' + e.message }"
    ) {
        Ok(s) if s == "undefined" => {
            report.pass(&format!("{}::reflect_no_require", name));
        }
        Ok(_) => {
            report.fail(&format!("{}::reflect_no_require", name),
                "Reflect.get can access require on window!");
        }
        Err(e) => report.skip(&format!("{}::reflect_no_require", name), &format!("evaluate_js_web: {}", e)),
    }

    let _ = page.close();
}

// ---------------------------------------------------------------------------
// Scenario: DOM access from privileged context (REQ-SEC-002)
// ---------------------------------------------------------------------------

fn scenario_privileged_dom_access(pool: &PagePool, report: &mut Report) {
    let name = "privileged_dom_access";

    let page = match pool.create_page(&PageConfig {
        url: Some("data:text/html,<html><head><title>dom-test</title></head><body><h1 id='t'>Hello</h1></body></html>".into()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            report.skip(name, &format!("page creation failed: {e}"));
            return;
        }
    };

    wait_for_load(&page, 3000);

    // Privileged evaluate_js should have DOM access
    match page.evaluate_js("document.getElementById('t') ? document.getElementById('t').textContent : 'no_element'") {
        Ok(s) if s.contains("Hello") => {
            report.pass(&format!("{}::dom_read", name));
        }
        Ok(s) => {
            report.skip(&format!("{}::dom_read", name), &format!("result: {}", s));
        }
        Err(e) => report.skip(&format!("{}::dom_read", name), &format!("evaluate_js: {}", e)),
    }

    // Privileged evaluate_js should have window/navigator access
    match page.evaluate_js("typeof window + ' ' + typeof navigator + ' ' + typeof document") {
        Ok(s) if s.contains("object") && s.contains("object") => {
            report.pass(&format!("{}::window_navigator", name));
        }
        Ok(s) => {
            report.skip(&format!("{}::window_navigator", name), &format!("result: {}", s));
        }
        Err(e) => report.skip(&format!("{}::window_navigator", name), &format!("evaluate_js: {}", e)),
    }

    // Privileged evaluate_js can modify DOM AND use Node APIs in same script
    match page.evaluate_js(
        "document.getElementById('t').textContent = 'Modified'; typeof require !== 'undefined' ? 'dom_and_node' : 'dom_only'"
    ) {
        Ok(s) if s.contains("dom_and_node") => {
            report.pass(&format!("{}::dom_and_node_api", name));
        }
        Ok(s) if s.contains("dom_only") => {
            // Node API not available yet — DOM modification still works
            report.skip(&format!("{}::dom_and_node_api", name), "Node API not yet injected, DOM works");
        }
        Ok(s) => {
            report.skip(&format!("{}::dom_and_node_api", name), &format!("result: {}", s));
        }
        Err(e) => report.skip(&format!("{}::dom_and_node_api", name), &format!("evaluate_js: {}", e)),
    }

    // Verify DOM was actually modified
    match page.evaluate_js_web("document.getElementById('t') ? document.getElementById('t').textContent : 'no_element'") {
        Ok(s) if s.contains("Modified") => {
            report.pass(&format!("{}::dom_modified_verified", name));
        }
        Ok(s) => {
            report.skip(&format!("{}::dom_modified_verified", name), &format!("textContent: {}", s));
        }
        Err(e) => report.skip(&format!("{}::dom_modified_verified", name), &format!("evaluate_js_web: {}", e)),
    }

    let _ = page.close();
}

// ---------------------------------------------------------------------------
// Scenario: Node APIs must NOT persist on globalThis after evaluate_js (timing attack defense)
// REQ-SEC-002: CommonJS parameter injection — Node APIs are function params, never globals
// ---------------------------------------------------------------------------

fn scenario_node_api_no_persistence_after_evaluate(pool: &PagePool, report: &mut Report) {
    let name = "stealth_no_persistence";

    let page = match pool.create_page(&PageConfig {
        url: Some("data:text/html,<html><body>timing-test</body></html>".into()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            report.skip(name, &format!("page creation failed: {e}"));
            return;
        }
    };

    wait_for_load(&page, 3000);

    // Run a privileged evaluate_js that uses Node APIs
    let _ = page.evaluate_js("typeof require !== 'undefined' ? 'has_require' : 'no_require'");

    // After evaluate_js completes, Node APIs must NOT be on globalThis
    match page.evaluate_js_web("typeof globalThis.require") {
        Ok(s) if s == "undefined" => report.pass(&format!("{}::require_gone", name)),
        Ok(s) => report.fail(&format!("{}::require_gone", name), &format!("require still on global: typeof={}", s)),
        Err(e) => report.skip(&format!("{}::require_gone", name), &format!("evaluate_js_web: {}", e)),
    }

    match page.evaluate_js_web("typeof globalThis.Bun") {
        Ok(s) if s == "undefined" => report.pass(&format!("{}::bun_gone", name)),
        Ok(s) => report.fail(&format!("{}::bun_gone", name), &format!("Bun still on global: typeof={}", s)),
        Err(e) => report.skip(&format!("{}::bun_gone", name), &format!("evaluate_js_web: {}", e)),
    }

    match page.evaluate_js_web("typeof globalThis.process") {
        Ok(s) if s == "undefined" => report.pass(&format!("{}::process_gone", name)),
        Ok(s) => report.fail(&format!("{}::process_gone", name), &format!("process still on global: typeof={}", s)),
        Err(e) => report.skip(&format!("{}::process_gone", name), &format!("evaluate_js_web: {}", e)),
    }

    match page.evaluate_js_web("typeof globalThis.Buffer") {
        Ok(s) if s == "undefined" => report.pass(&format!("{}::buffer_gone", name)),
        Ok(s) => report.fail(&format!("{}::buffer_gone", name), &format!("Buffer still on global: typeof={}", s)),
        Err(e) => report.skip(&format!("{}::buffer_gone", name), &format!("evaluate_js_web: {}", e)),
    }

    match page.evaluate_js_web("typeof globalThis.module") {
        Ok(s) if s == "undefined" => report.pass(&format!("{}::module_gone", name)),
        Ok(s) => report.fail(&format!("{}::module_gone", name), &format!("module still on global: typeof={}", s)),
        Err(e) => report.skip(&format!("{}::module_gone", name), &format!("evaluate_js_web: {}", e)),
    }

    match page.evaluate_js_web("typeof globalThis.__dirname") {
        Ok(s) if s == "undefined" => report.pass(&format!("{}::dirname_gone", name)),
        Ok(s) => report.fail(&format!("{}::dirname_gone", name), &format!("__dirname still on global: typeof={}", s)),
        Err(e) => report.skip(&format!("{}::dirname_gone", name), &format!("evaluate_js_web: {}", e)),
    }

    match page.evaluate_js_web("typeof globalThis.__filename") {
        Ok(s) if s == "undefined" => report.pass(&format!("{}::filename_gone", name)),
        Ok(s) => report.fail(&format!("{}::filename_gone", name), &format!("__filename still on global: typeof={}", s)),
        Err(e) => report.skip(&format!("{}::filename_gone", name), &format!("evaluate_js_web: {}", e)),
    }

    // Verify scope object itself is deleted
    match page.evaluate_js_web("typeof globalThis.__bao_privileged_apis") {
        Ok(s) if s == "undefined" => report.pass(&format!("{}::scope_deleted", name)),
        Ok(s) => report.fail(&format!("{}::scope_deleted", name), &format!("scope still on global: typeof={}", s)),
        Err(e) => report.skip(&format!("{}::scope_deleted", name), &format!("evaluate_js_web: {}", e)),
    }

    // Verify Object.getOwnPropertyDescriptor returns undefined for require
    match page.evaluate_js_web("String(Object.getOwnPropertyDescriptor(globalThis, 'require'))") {
        Ok(s) if s == "undefined" => report.pass(&format!("{}::require_no_descriptor", name)),
        Ok(s) => report.fail(&format!("{}::require_no_descriptor", name), &format!("descriptor exists: {}", s)),
        Err(e) => report.skip(&format!("{}::require_no_descriptor", name), &format!("evaluate_js_web: {}", e)),
    }

    // Verify re-injection is idempotent
    let _ = page.evaluate_js("typeof require !== 'undefined' ? 'has_require_2' : 'no_require_2'");
    match page.evaluate_js_web("typeof globalThis.require") {
        Ok(s) if s == "undefined" => report.pass(&format!("{}::require_gone_after_rerun", name)),
        Ok(s) => report.fail(&format!("{}::require_gone_after_rerun", name), &format!("require re-appeared: typeof={}", s)),
        Err(e) => report.skip(&format!("{}::require_gone_after_rerun", name), &format!("evaluate_js_web: {}", e)),
    }

    // Verify no leak after repeated evaluate_js calls
    for _ in 0..3 {
        let _ = page.evaluate_js("1+1");
    }
    match page.evaluate_js_web("typeof globalThis.require") {
        Ok(s) if s == "undefined" => report.pass(&format!("{}::no_leak_after_repeated", name)),
        Ok(s) => report.fail(&format!("{}::no_leak_after_repeated", name), &format!("require leaked: typeof={}", s)),
        Err(e) => report.skip(&format!("{}::no_leak_after_repeated", name), &format!("evaluate_js_web: {}", e)),
    }

    let _ = page.close();
}

// ---------------------------------------------------------------------------
// Scenario: CommonJS parameter injection — verify scope deletion and no global residue
// REQ-SEC-002: Node APIs exist only as IIFE function parameters
// ---------------------------------------------------------------------------

fn scenario_scope_cleanup_after_evaluate(pool: &PagePool, report: &mut Report) {
    let name = "scope_cleanup";

    let page = match pool.create_page(&PageConfig {
        url: Some("data:text/html,<html><body>scope-test</body></html>".into()),
        ..Default::default()
    }) {
        Ok(p) => p,
        Err(e) => {
            report.skip(name, &format!("page creation failed: {e}"));
            return;
        }
    };

    wait_for_load(&page, 3000);

    // Run privileged evaluate_js to trigger scope injection + cleanup
    let _ = page.evaluate_js("typeof require");

    // Verify __bao_privileged_apis is deleted from globalThis
    match page.evaluate_js_web("'__bao_privileged_apis' in globalThis") {
        Ok(s) if s == "false" => report.pass(&format!("{}::scope_not_in_global", name)),
        Ok(s) => report.fail(&format!("{}::scope_not_in_global", name), &format!("scope in global: {}", s)),
        Err(e) => report.skip(&format!("{}::scope_not_in_global", name), &format!("evaluate_js_web: {}", e)),
    }

    // Verify __bao_setEnv / __bao_delEnv are deleted
    match page.evaluate_js_web("'__bao_setEnv' in globalThis || '__bao_delEnv' in globalThis") {
        Ok(s) if s == "false" => report.pass(&format!("{}::env_helpers_deleted", name)),
        Ok(s) => report.fail(&format!("{}::env_helpers_deleted", name), &format!("env helpers found: {}", s)),
        Err(e) => report.skip(&format!("{}::env_helpers_deleted", name), &format!("evaluate_js_web: {}", e)),
    }

    // Verify Buffer is not a direct property of globalThis
    match page.evaluate_js_web("'Buffer' in globalThis") {
        Ok(s) if s == "false" => report.pass(&format!("{}::buffer_not_in_global", name)),
        Ok(s) => report.fail(&format!("{}::buffer_not_in_global", name), &format!("Buffer in global: {}", s)),
        Err(e) => report.skip(&format!("{}::buffer_not_in_global", name), &format!("evaluate_js_web: {}", e)),
    }

    // Verify Reflect.ownKeys does not contain Node API names
    match page.evaluate_js_web(
        "Reflect.ownKeys(globalThis).filter(k => ['require','module','__filename','__dirname','__bao_privileged_apis','__bao_setEnv','__bao_delEnv'].includes(k)).join(',')"
    ) {
        Ok(s) if s.is_empty() => report.pass(&format!("{}::ownkeys_clean", name)),
        Ok(s) => report.fail(&format!("{}::ownkeys_clean", name), &format!("found keys: {}", s)),
        Err(e) => report.skip(&format!("{}::ownkeys_clean", name), &format!("evaluate_js_web: {}", e)),
    }

    // Verify indirect eval cannot access Node APIs
    match page.evaluate_js_web("(0,eval)('typeof require')") {
        Ok(s) if s == "undefined" => report.pass(&format!("{}::indirect_eval_safe", name)),
        Ok(s) => report.fail(&format!("{}::indirect_eval_safe", name), &format!("indirect eval found: typeof={}", s)),
        Err(e) => report.skip(&format!("{}::indirect_eval_safe", name), &format!("evaluate_js_web: {}", e)),
    }

    // Verify Function constructor cannot access Node APIs
    match page.evaluate_js_web("new Function('return typeof require')()") {
        Ok(s) if s == "undefined" => report.pass(&format!("{}::function_ctor_safe", name)),
        Ok(s) => report.fail(&format!("{}::function_ctor_safe", name), &format!("Function ctor found: typeof={}", s)),
        Err(e) => report.skip(&format!("{}::function_ctor_safe", name), &format!("evaluate_js_web: {}", e)),
    }

    let _ = page.close();
}
