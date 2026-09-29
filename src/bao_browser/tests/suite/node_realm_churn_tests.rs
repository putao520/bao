// @trace TEST-BRW-003 [req:REQ-BRW-003] [level:e2e]
// W26 (W21a evidence, commit b911642f): one churn page's
// create+navigate+close lifecycle wrongly cleared a LONG-LIVED page's
// NODE_REALM_BY_WEBVIEW registration — the survivor's next evaluate_js hit
// the "Node Realm not initialized — this is a bug, eager init failed"
// null-realm guard (page.rs evaluate_js main-thread check). Bench mixed-soak
// repro was 3/3 (sqlite class on pages[0] after one churn cycle); this file
// is the directed minimal carrier: long-lived pages (probe + N) → the exact
// page_bench::churn_cycle shape → every survivor must keep evaluating in
// the Node realm.
//
// Fidelity notes (mirrors bench soak_bench run_mixed):
// - startup per-page `typeof Bun.gc` gate (the bench's fail-closed check),
// - the async-pump node-timer probe shape on pages[0] (evaluate_js arm +
//   evaluate_js_with_timeout polls — the only pages[0]-specific history
//   before the first churn in the failing bench runs),
// - a CDP bridge EvaluateJs op (the CLS_CDP face, same dispatch layer).

use std::time::Duration;

use bao_browser::{handle_bridge_command, BaoConfig, BrowserRuntime, PageConfig, PageHandle};
use bao_cdp::BridgeCommand;

/// servo/BrowserRuntime carry process-global slots — serialize the tests in
/// this suite file (same pattern as pagestate_lifecycle_tests).
static RUNTIME_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn node_ok(page: &PageHandle) -> Result<(), String> {
    match page.evaluate_js("typeof Bun.gc") {
        Ok(ref s) if s.contains("function") => Ok(()),
        other => Err(format!("typeof Bun.gc -> {other:?}")),
    }
}

fn make_page(runtime: &BrowserRuntime) -> Result<PageHandle, String> {
    let page = runtime
        .create_page(&PageConfig {
            url: Some("about:blank".into()),
            ..Default::default()
        })
        .map_err(|e| format!("create_page failed: {e}"))?;
    page.wait_for_pipeline_ready(Duration::from_secs(15))
        .map_err(|e| format!("pipeline not ready: {e}"))?;
    Ok(page)
}

/// The page_bench::churn_cycle shape, byte-equivalent semantics:
/// create(about:blank) → wait_for_pipeline_ready → navigate(data:URL with
/// marker) → wait_for_navigation → marker eval → pool.close_page.
fn churn_cycle(runtime: &BrowserRuntime, i: usize) -> Result<(), String> {
    let marker = format!("churnw26-{i}");
    // Space-free / quote-free HTML (WHATWG percent-encoding constraint,
    // same as page_bench).
    let data_url = format!("data:text/html,<h1 id=b>{marker}</h1>");
    let page = runtime
        .create_page(&PageConfig {
            url: Some("about:blank".into()),
            ..Default::default()
        })
        .map_err(|e| format!("churn {i}: create_page failed: {e}"))?;
    page.wait_for_pipeline_ready(Duration::from_secs(15))
        .map_err(|e| format!("churn {i}: pipeline not ready: {e}"))?;
    page.navigate(&data_url)
        .map_err(|e| format!("churn {i}: navigate failed: {e}"))?;
    page.wait_for_navigation(Duration::from_secs(15))
        .map_err(|e| format!("churn {i}: wait_for_navigation failed: {e}"))?;
    let check = page
        .evaluate_js_web(&format!(
            "String(document.getElementById('b') && document.getElementById('b').textContent === '{marker}')"
        ))
        .map_err(|e| format!("churn {i}: marker eval failed: {e}"))?;
    if !check.contains("true") {
        return Err(format!("churn {i}: marker verification failed (got {check:?})"));
    }
    runtime
        .page_pool()
        .close_page(page.id())
        .map_err(|e| format!("churn {i}: close_page failed: {e}"))?;
    Ok(())
}

fn assert_all_node_ok(pages: &[PageHandle], stage: &str) {
    for (i, page) in pages.iter().enumerate() {
        match node_ok(page) {
            Ok(()) => {}
            Err(e) => panic!("{stage}: long-lived page {i} lost its Node realm: {e}"),
        }
    }
}

#[test]
fn churn_close_keeps_long_lived_pages_node_realm() {
    let _guard = RUNTIME_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let runtime = match BrowserRuntime::new(BaoConfig::default()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[skip] runtime init failed: {e}");
            return;
        }
    };

    // ── Cold: probe page + 4 long-lived pages, startup per-page gate ──────
    let probe = make_page(&runtime).expect("probe page");
    node_ok(&probe).expect("probe page startup Node realm gate");

    let mut pages: Vec<PageHandle> = Vec::with_capacity(4);
    for i in 0..4 {
        let page = make_page(&runtime).unwrap_or_else(|e| panic!("mixed page {i}: {e}"));
        node_ok(&page).unwrap_or_else(|e| panic!("mixed page {i} startup gate: {e}"));
        pages.push(page);
    }

    // ── pages[0] pre-churn history: the async-pump node-timer probe shape ─
    // (arm in the Node realm, then pump — the only pages[0]-specific
    // operation the bench runs before churn.) W26 root cause: this fire
    // panicked the ScriptThread (settings runner fed the NODE-realm global
    // to the DOM-only bao_run_in_script_settings). The fix must BOTH keep
    // the thread alive AND actually fire the timer (bare dispatch).
    pages[0]
        .evaluate_js(
            "globalThis.__w26nt=0;setTimeout(function(){globalThis.__w26nt=1;},10);'armed'",
        )
        .expect("pages[0] node timer arm");
    let mut nt_fired = false;
    for _ in 0..10 {
        runtime.spin_event_loop();
        let _ = pages[0].evaluate_js_web(";");
        std::thread::sleep(Duration::from_millis(50));
        match pages[0].evaluate_js_with_timeout(
            "String(globalThis.__w26nt)",
            Some(Duration::from_millis(250)),
        ) {
            Ok(v) if v.trim_matches('"') == "1" => {
                nt_fired = true;
                break;
            }
            Ok(_) => {}
            Err(_) => break, // unhealthy page — walk away (bench probe shape)
        }
    }
    assert!(
        nt_fired,
        "W26: node-realm timer must FIRE under the bare dispatch (not just avoid the panic)"
    );
    node_ok(&pages[0]).expect("pages[0] after node-timer probe shape");

    // ── CDP bridge evaluate (CLS_CDP face) on pages[0] ────────────────────
    {
        let resp = handle_bridge_command(
            BridgeCommand::EvaluateJs {
                target_id: pages[0].id().to_string(),
                expression: "(function(){return 'w26-cdp';})()".into(),
                return_by_value: true,
            },
            runtime.page_pool(),
        );
        let value = resp.result.expect("cdp bridge evaluate result");
        let got = value
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_str())
            .unwrap_or("<missing>");
        assert_eq!(got, "w26-cdp", "cdp bridge evaluate round-trip");
    }

    // ── One churn cycle, then every survivor must keep its Node realm ─────
    churn_cycle(&runtime, 0).expect("churn cycle 0");
    assert_all_node_ok(&pages, "after churn cycle 0");
    node_ok(&probe).expect("probe page after churn cycle 0");

    // Second cycle (bench 2-min run reaches many; two proves persistence).
    churn_cycle(&runtime, 1).expect("churn cycle 1");
    assert_all_node_ok(&pages, "after churn cycle 1");
    node_ok(&probe).expect("probe page after churn cycle 1");

    // ── Node-realm functional depth on every survivor (not just Bun.gc):
    //    require('bun:sqlite') round-trip — the sqlite class shape that
    //    failed 3/3 in the bench.
    for (i, page) in pages.iter().enumerate() {
        let out = page
            .evaluate_js(
                "(function(){var {Database}=require('bun:sqlite');var db=new Database(':memory:');\
                 db.exec('CREATE TABLE t(a INTEGER)');db.prepare('INSERT INTO t VALUES (1)').run();\
                 var c=db.prepare('SELECT COUNT(*) AS c FROM t').get().c;db.close();\
                 return c===1?'ok':'bad:'+c;})()",
            )
            .unwrap_or_else(|e| panic!("sqlite depth page {i} after churn: {e}"));
        assert_eq!(out.trim_matches('"'), "ok", "sqlite depth page {i} after churn");
    }
}
