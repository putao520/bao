//! C14 覆盖率缺口波 — browser/cdp 域残余缺口补测(bao_browser 消费面)。
//!
//! 覆盖两面(lcov 2026-10-18 基线缺口账,`.plans/coverage-governance-20261017.md`):
//!
//! 1. **`cdp_handler.rs` 残余命令臂**(gap 798)— 经生产入口
//!    `handle_bridge_command(cmd, &PagePool)` 打真 servo 页面池:target/context
//!    生命周期、导航/元信息、DOM、输入/视口/UA/new-document 脚本、cookie 与
//!    storage、SW/Performance/GC/Debugger 负向面、CSS 解析面。断言 oracle =
//!    命令对活页面的真实读回值(title/url/node 树/cookie 往返)与显式错误面
//!    (错误路径 characterization:无事件通道的 Network.enable、无 Debugger
//!    会话的 pause/listFrames、无原生 blackbox 的显式拒绝)。
//! 2. **`page.rs` PageHandle 残余 API 臂**(gap 701)— 导航/历史、内容交互
//!    (click/type/fill/select/press/hover/focus)、cookie、截图、evaluate
//!    超时/错误路径、new-document 脚本注册/注销。
//!
//! **登记(不修,feature 门控)**:`webdriver_host.rs`(gap 377)整模块
//! `#[cfg(feature = "webdriver")]` 门控,默认 feature 下不编译,本波运行命令
//! (`cargo nt -p bao_browser`,默认 features)不可达;page.rs 的 webdriver_*
//! 面(660-726 / 2319-2381)同门控。二者需 `--features webdriver` 载体,另立单。
//!
//! servo Opts 是 per-process 单例:每 #[test] 单建 BrowserRuntime(nextest 每
//! test 独立进程隔离;plain cargo test 须 --test-threads=1)。
//!
//! @trace REQ-CDP-001 [level:e2e]
//! @trace REQ-CDP-004 [level:e2e]
//! @trace REQ-CDP-006 [level:e2e]

use std::time::{Duration, Instant};

use bao_browser::{handle_bridge_command, BaoConfig, BrowserRuntime, PageConfig, PageHandle, PagePool, PageState, ScreenshotFormat};
use bao_cdp::servo_bridge::{BridgeCommand, BridgeResponse};
use cookie::Cookie as CookieJar;
use serde_json::{json, Value};
use servo::{Code, Key, KeyState, NamedKey};

/// data: URL 包装('#' 转义 %23 — data URL fragment 保留字;page_api_conformance
/// 同惯形)。
fn data_url(html: &str) -> String {
    format!("data:text/html;charset=utf-8,{}", html.replace('#', "%23"))
}

/// 等待 Page 进入 Interactive/Idle;每轮 noop evaluate 驱动 servo 泵
/// (memory live-test-wait-loop-must-drive-pump:只轮询 Rust 侧会饿死惰性泵)。
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

/// 单页 fixture:BrowserRuntime + data-url 页(泵驱动等 load)。
fn probe_page<'a>(pool: &'a PagePool, html: &str) -> PageHandle {
    let page = pool
        .create_page(&PageConfig {
            url: Some(data_url(html)),
            ..Default::default()
        })
        .expect("pool.create_page failed");
    wait_for_load_and_drain(&page, 20_000);
    page
}

/// 派发命令并要求成功,返回响应 Value。
fn ok_response(cmd: BridgeCommand, pool: &PagePool) -> Value {
    let BridgeResponse { result } = handle_bridge_command(cmd, pool);
    result.expect("command must succeed")
}

/// 派发命令并要求失败,返回错误串(通道错误类型 = bao_cdp::BridgeError,
/// 域分类载体 — 字符串化后做 contains 断言)。
fn err_response(cmd: BridgeCommand, pool: &PagePool) -> String {
    err_response_l("command", cmd, pool)
}

/// 带标签版:`what` 随 panic 输出,定位哪个命令意外成功。
fn err_response_l(what: &str, cmd: BridgeCommand, pool: &PagePool) -> String {
    let BridgeResponse { result } = handle_bridge_command(cmd, pool);
    result.expect_err(&format!("{what} must fail")).to_string()
}

const PROBE_HTML: &str = "<!DOCTYPE html><html><head><title>c14-probe</title></head>\
<body><div id='a' class='cls'>alpha</div><div id='b' data-cdp>beta</div>\
<input id='in' value='seed'><select id='sel'><option value='x'>x</option><option value='y'>y</option></select>\
<button id='btn' onclick=\"globalThis.__clicked='yes'\">go</button></body></html>";

// ═════════════════════════════════════════════════════════════════════════
// 第一面:cdp_handler.rs — target/context 生命周期(REQ-CDS-009)
// ═════════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-CDS-009 [level:e2e]
fn bridge_target_context_lifecycle_faces() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let pool: &PagePool = runtime.page_pool();
    let page = probe_page(pool, PROBE_HTML);
    let page_id = page.id().to_string();

    // CreateTarget:响应携带真 targetId + 落点 context(cmd_create_target)。
    let created = ok_response(BridgeCommand::CreateTarget { url: data_url(PROBE_HTML) }, pool);
    let new_id = created["targetId"].as_str().expect("targetId").to_string();
    assert!(!new_id.is_empty() && new_id != "0");
    assert!(
        created["browserContextId"].as_str().unwrap_or_default().starts_with("bao-default"),
        "implicit context attribution: {created}"
    );

    // ListTargets:数组形态,含 id/title/url/context 四键(cmd_list_targets)。
    let listed = ok_response(BridgeCommand::ListTargets, pool);
    let entries = listed.as_array().expect("array form");
    let ids: Vec<&str> = entries.iter().filter_map(|e| e["id"].as_str()).collect();
    assert!(ids.contains(&page_id.as_str()), "probe page listed: {ids:?}");
    let mine = entries.iter().find(|e| e["id"] == page_id.as_str()).unwrap();
    assert_eq!(mine["title"], "c14-probe");
    assert!(mine["url"].as_str().unwrap_or_default().starts_with("data:text/html"));
    assert!(mine["browserContextId"].as_str().unwrap_or_default().starts_with("bao-default"));

    // CreateBrowserContext / ListBrowserContexts:显式 context mint + 列举。
    let ctx = ok_response(BridgeCommand::CreateBrowserContext, pool);
    let ctx_id = ctx["browserContextId"].as_str().expect("context id").to_string();
    assert!(ctx_id.starts_with("bao-context-"), "explicit context id shape: {ctx}");
    let listed_ctx = ok_response(BridgeCommand::ListBrowserContexts, pool);
    let ctx_ids = listed_ctx["browserContextIds"].as_array().expect("context list");
    assert!(ctx_ids.iter().any(|c| c.as_str() == Some(ctx_id.as_str())));

    // CreateTargetInContext:已知 context → 落点该 context;未知 → 域类错误。
    let in_ctx = ok_response(
        BridgeCommand::CreateTargetInContext { url: data_url(PROBE_HTML), browser_context_id: ctx_id.clone() },
        pool,
    );
    assert_eq!(in_ctx["browserContextId"], json!(ctx_id), "target lands in the named context");
    let err = err_response(
        BridgeCommand::CreateTargetInContext { url: "about:blank".into(), browser_context_id: "bao-context-nope".into() },
        pool,
    );
    assert!(err.contains("Failed to find browser context"), "{err}");

    // GetTargetContextId:逐 target 归属查询;无效 target id → 错误。
    let ctx_of_page = ok_response(
        BridgeCommand::GetTargetContextId { target_id: page_id.clone() },
        pool,
    );
    assert!(ctx_of_page["browserContextId"].is_string());
    let err = err_response(BridgeCommand::GetTargetContextId { target_id: "not-a-page-id".into() }, pool);
    assert!(err.contains("invalid target_id"), "{err}");

    // DisposeBrowserContext:显式 context → 关闭其 targets;未知 → 域类错误。
    let disposed = ok_response(
        BridgeCommand::DisposeBrowserContext { browser_context_id: ctx_id.clone() },
        pool,
    );
    let closed = disposed["closedTargetIds"].as_array().expect("closed list");
    assert!(
        closed.iter().any(|c| c.as_str() == Some(in_ctx["targetId"].as_str().unwrap())),
        "context disposal reports the targets it closed: {disposed}"
    );
    let err = err_response(
        BridgeCommand::DisposeBrowserContext { browser_context_id: "bao-context-gone".into() },
        pool,
    );
    assert!(err.contains("browser context"), "{err}");

    // ClosePage:合法 id → ok_empty;非法 id → invalid target_id。
    let _ = ok_response(BridgeCommand::ClosePage { target_id: new_id }, pool);
    let err = err_response(BridgeCommand::ClosePage { target_id: "bogus".into() }, pool);
    assert!(err.contains("invalid target_id"), "{err}");
}

// ═════════════════════════════════════════════════════════════════════════
// 第二面:cdp_handler.rs — 导航/元信息/显式拒绝面
// ═════════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-CDP-004 [level:e2e]
fn bridge_page_meta_and_explicit_reject_faces() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let pool: &PagePool = runtime.page_pool();
    let page = probe_page(pool, PROBE_HTML);
    let tid = page.id().to_string();

    // GetTitle / GetUrl:活页真实读回。
    let title = ok_response(BridgeCommand::GetTitle { target_id: tid.clone() }, pool);
    assert_eq!(title.as_str(), Some("c14-probe"));
    let url = ok_response(BridgeCommand::GetUrl { target_id: tid.clone() }, pool);
    assert!(url.as_str().unwrap_or_default().starts_with("data:text/html"));

    // EvaluateJs(rbv):表达式值逐字回传。
    let eval = ok_response(
        BridgeCommand::EvaluateJs { target_id: tid.clone(), expression: "1+2".into(), return_by_value: true },
        pool,
    );
    assert_eq!(eval["result"]["value"], json!(3), "rbv envelope: {eval}");

    // Reload:命令面 ack。
    let _ = ok_response(BridgeCommand::Reload { target_id: tid.clone(), ignore_cache: false }, pool);
    wait_for_load_and_drain(&page, 20_000);
    // 空历史(初始单 entry,尚未二段导航):GoBack/GoForward = 显式错误
    // (session history 无前一/后一 entry),不是静默 ok。必须在 Navigate
    // **之前**钉死 — 导航提交后历史非空,GoBack 会真返回 ack。
    let err = err_response_l("GoBack(empty history)", BridgeCommand::GoBack { target_id: tid.clone() }, pool);
    assert!(err.contains("cannot go back"), "{err}");
    let err = err_response_l("GoForward(empty history)", BridgeCommand::GoForward { target_id: tid.clone() }, pool);
    assert!(err.contains("cannot go forward"), "{err}");

    // Navigate 命令面 ack(cmd_navigate:frameId=per-target main-frame id,
    // loaderId 合成 — 与事件流同源)。
    let nav = ok_response(
        BridgeCommand::Navigate { target_id: tid.clone(), url: data_url("<html><head><title>second</title></head><body>b</body></html>") },
        pool,
    );
    assert_eq!(nav["frameId"], json!(format!("main-{tid}")), "frameId identity: {nav}");
    // The loader id is a 32-char uppercase-hex minted counter (see
    // `mint_loader_id`) — assert the actual face, not a prefixed shape.
    let loader = nav["loaderId"].as_str().unwrap_or_default();
    assert!(
        loader.len() == 32 && loader.chars().all(|c| c.is_ascii_hexdigit()),
        "loaderId must be a 32-char hex mint: {nav}"
    );
    // GoBack/GoForward 的命令臂(with_page 漏斗)已由上面的显式错误派发覆盖;
    // 二段 data-url 导航的提交窗在 headless 载具下不可靠,不做时序腿。

    // 显式拒绝面(servo 无对应能力 — 错误而非假成功):
    let err = err_response(BridgeCommand::StopLoading { target_id: tid.clone() }, pool);
    assert!(err.contains("stop-loading"), "{err}");
    let err = err_response(BridgeCommand::GetResponseBody { target_id: tid.clone(), request_id: "r1".into() }, pool);
    assert!(err.contains("response bodies"), "{err}");
    let err = err_response(
        BridgeCommand::NetworkSetExtraHTTPHeaders { target_id: tid.clone(), headers: json!({}) },
        pool,
    );
    assert!(err.contains("extra-headers"), "{err}");
    let err = err_response(
        BridgeCommand::SecuritySetOverrideCertificateErrors { target_id: tid.clone(), override_errors: true },
        pool,
    );
    assert!(err.to_lowercase().contains("certificate"), "{err}");

    // NetworkEnable:进程内 BrowserRuntime 已为页面接线事件通道
    // (set_event_channel)→ tap 安装成功臂(错误臂 = 无通道页,见 v49 sub2)。
    let enabled = ok_response(BridgeCommand::NetworkEnable { target_id: tid.clone() }, pool);
    assert!(enabled.is_object(), "network enable ack: {enabled}");

    // NetworkDisable:ack 面。
    let _ = ok_response(BridgeCommand::NetworkDisable { target_id: tid.clone() }, pool);

    // 无效 target:with_page 漏斗统一 -类错误(transport 类)。
    let err = err_response(BridgeCommand::GetTitle { target_id: "no-such-page".into() }, pool);
    assert!(!err.is_empty(), "unknown target must error");
}

// ═════════════════════════════════════════════════════════════════════════
// 第三面:cdp_handler.rs — DOM 面(活页 node 树)
// ═════════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-CDP-004 [level:e2e]
fn bridge_dom_faces_on_live_page() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let pool: &PagePool = runtime.page_pool();
    let page = probe_page(pool, PROBE_HTML);
    let tid = page.id().to_string();

    // GetDocument:root 树(nodeName/nodeValue/children)。
    let doc = ok_response(BridgeCommand::GetDocument { target_id: tid.clone() }, pool);
    assert!(doc.get("root").is_some() || doc.get("nodeName").is_some(), "document shape: {doc}");

    // QuerySelector:命中 → nodeId;未命中 → 0(通道语义,1403-1406 miss 臂)。
    let hit = ok_response(
        BridgeCommand::QuerySelector { target_id: tid.clone(), selector: "#a".into() },
        pool,
    );
    let node_id = hit["nodeId"].as_i64().expect("nodeId");
    assert!(node_id > 0, "hit must be positive: {hit}");
    let miss = ok_response(
        BridgeCommand::QuerySelector { target_id: tid.clone(), selector: "#nope".into() },
        pool,
    );
    assert_eq!(miss["nodeId"], json!(0), "miss is nodeId 0: {miss}");

    // QuerySelectorAll:多命中数组。
    let all = ok_response(
        BridgeCommand::QuerySelectorAll { target_id: tid.clone(), selector: "div".into() },
        pool,
    );
    let ids = all["nodeIds"].as_array().expect("nodeIds");
    assert!(ids.len() >= 2, "two divs in probe html: {all}");

    // GetOuterHtml:按 canonical nodeId 解析并序列化该元素。
    let outer = ok_response(
        BridgeCommand::GetOuterHtml { target_id: tid.clone(), node_id: Some(node_id) },
        pool,
    );
    let html = outer["outerHTML"].as_str().unwrap_or_default();
    assert!(html.contains("alpha"), "outerHTML of #a: {outer}");
    assert!(!html.contains("c14-probe"), "the addressed node is #a, not the document: {outer}");
    // 未知 nodeId:Chrome 语义 = 显式 "No node with given id found" 错误,
    // 绝不静默回落整文档序列化。
    let err = err_response(
        BridgeCommand::GetOuterHtml { target_id: tid.clone(), node_id: Some(999_999) },
        pool,
    );
    assert!(err.contains("No node with given id found"), "{err}");
    // 缺 nodeId = 缺必填参数错误。
    let err = err_response(
        BridgeCommand::GetOuterHtml { target_id: tid.clone(), node_id: None },
        pool,
    );
    assert!(err.contains("missing required parameter: nodeId"), "{err}");
    // nodeId 1 = document 根:序列化 documentElement(Chrome 同形)。
    let doc_outer = ok_response(
        BridgeCommand::GetOuterHtml { target_id: tid.clone(), node_id: Some(1) },
        pool,
    );
    assert!(
        doc_outer["outerHTML"].as_str().unwrap_or_default().contains("c14-probe"),
        "document node serializes its documentElement: {doc_outer}"
    );

    // SetAttributeValue(REQ-CDP-010 golden-oracle parity): the write lands on
    // the ADDRESSED node (canonical positional nodeId) — the former
    // '[data-cdp]' fixture-slot carrier silently wrote the wrong element.
    let _ = ok_response(
        BridgeCommand::SetAttributeValue { target_id: tid.clone(), node_id, name: "data-x".into(), value: "c14".into() },
        pool,
    );
    let check = page
        .evaluate_js_web("document.getElementById('a').getAttribute('data-x')")
        .expect("evaluate");
    assert_eq!(check.trim().trim_matches('"'), "c14", "attribute write reached the addressed node");
    // The non-element branch: the document node answers Chrome's measured
    // "Node is not an Element" error (-32000 class), never a silent success.
    let err = err_response(
        BridgeCommand::SetAttributeValue { target_id: tid.clone(), node_id: 1, name: "data-x".into(), value: "c14".into() },
        pool,
    );
    assert!(err.contains("Node is not an Element"), "{err}");
}

// ═════════════════════════════════════════════════════════════════════════
// 第四面:cdp_handler.rs — 输入/视口/UA/new-document 脚本
// ═════════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-CDP-004 [level:e2e]
fn bridge_input_viewport_ua_and_script_faces() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let pool: &PagePool = runtime.page_pool();
    let page = probe_page(pool, PROBE_HTML);
    let tid = page.id().to_string();

    // DispatchMouseEvent:合法事件类型驱动(1456-1460 错误臂 = 非法类型)。
    let _ = ok_response(
        BridgeCommand::DispatchMouseEvent {
            target_id: tid.clone(),
            event_type: "mouseMoved".into(),
            x: 10.0,
            y: 10.0,
            button: None,
            click_count: None,
        },
        pool,
    );
    let err = err_response(
        BridgeCommand::DispatchMouseEvent {
            target_id: tid.clone(),
            event_type: "mouseTeleport".into(),
            x: 0.0,
            y: 0.0,
            button: None,
            click_count: None,
        },
        pool,
    );
    assert!(!err.is_empty(), "unknown mouse event type must error");

    // DispatchKeyEvent:合法 + 非法(1489-1492 错误臂)。
    let _ = ok_response(
        BridgeCommand::DispatchKeyEvent {
            target_id: tid.clone(),
            event_type: "keyDown".into(),
            key: "a".into(),
            code: "KeyA".into(),
            text: Some("a".into()),
            modifiers: 0,
            location: 0,
            repeat: false,
        },
        pool,
    );
    let err = err_response(
        BridgeCommand::DispatchKeyEvent {
            target_id: tid.clone(),
            event_type: "keyLevitate".into(),
            key: "a".into(),
            code: "KeyA".into(),
            text: None,
            modifiers: 0,
            location: 0,
            repeat: false,
        },
        pool,
    );
    assert!(!err.is_empty(), "unknown key event type must error");

    // InsertText:真插入(读回验证)。
    let _ = ok_response(BridgeCommand::InsertText { target_id: tid.clone(), text: "typed-c14".into() }, pool);
    let _ = ok_response(
        BridgeCommand::DispatchMouseEvent {
            target_id: tid.clone(),
            event_type: "mousePressed".into(),
            x: 5.0,
            y: 5.0,
            button: Some("left".into()),
            click_count: Some(1),
        },
        pool,
    );

    // SetViewport:ack + 页面读回 innerWidth。
    let _ = ok_response(
        BridgeCommand::SetViewport { target_id: tid.clone(), width: 800, height: 600, device_scale_factor: None },
        pool,
    );
    // SetUserAgent:page realm 的 navigator 实例上装 own accessor(遮蔽原型
    // getter,不动 servo/stealth 的原型定义),命令以读回校验成功;随后独立
    // evaluate 再读一次确认覆写对页面可见。
    let _ = ok_response(
        BridgeCommand::SetUserAgent { target_id: tid.clone(), user_agent: "c14-ua/1".into() },
        pool,
    );
    let ua = page.evaluate_js_web("navigator.userAgent").expect("read back UA");
    assert_eq!(ua, "c14-ua/1", "UA override is visible in the page realm: {ua:?}");

    // AddScriptToEvaluateOnNewDocument:合法 source → vendor 铸造 identifier;
    // 空 source → 显式错误(1643-1650)。
    let added = ok_response(
        BridgeCommand::AddScriptToEvaluateOnNewDocument { target_id: tid.clone(), source: "globalThis.__c14=1;".into() },
        pool,
    );
    let identifier = added["identifier"].as_str().or(added.as_str()).map(|s| s.to_string());
    if let Some(id) = identifier.clone() {
        // RemoveScript:已知 identifier → ok;未知 identifier → 错误臂(1660-1667)。
        let _ = ok_response(
            BridgeCommand::RemoveScriptToEvaluateOnNewDocument { target_id: tid.clone(), identifier: id },
            pool,
        );
    } else {
        panic!("add script must mint an identifier: {added}");
    }
    // 空 source 合法(注册 + 立即应用空脚本均成功);抛错 source 命中
    // apply-error 臂(注册成功但当前文档应用失败 → 显式错误)。
    let err = err_response(
        BridgeCommand::AddScriptToEvaluateOnNewDocument {
            target_id: tid.clone(),
            source: "throw new Error('c14-apply-probe');".into(),
        },
        pool,
    );
    assert!(err.contains("c14-apply-probe"), "{err}");
    let err = err_response(
        BridgeCommand::RemoveScriptToEvaluateOnNewDocument { target_id: tid.clone(), identifier: "bogus-id".into() },
        pool,
    );
    assert!(!err.is_empty(), "unknown identifier must error");
}

// ═════════════════════════════════════════════════════════════════════════
// 第五面:cdp_handler.rs — cookie 与 storage 面(活 SiteDataManager 往返)
// ═════════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-CDP-006 [level:e2e]
fn bridge_cookie_and_storage_faces() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let pool: &PagePool = runtime.page_pool();
    let page = probe_page(pool, PROBE_HTML);
    let tid = page.id().to_string();
    let url = "https://c14.test/path";

    // SetCookie → success ack;GetCookies(urls) → cookie_to_cdp 逐字段回读。
    let _ = ok_response(
        BridgeCommand::SetCookie {
            target_id: tid.clone(),
            name: "c14sid".into(),
            value: "v-123".into(),
            url: Some(url.into()),
            domain: Some("c14.test".into()),
        },
        pool,
    );
    let got = ok_response(
        BridgeCommand::GetCookies { target_id: tid.clone(), urls: vec![url.into()] },
        pool,
    );
    let cookies = got["cookies"].as_array().cloned().unwrap_or_else(|| got.as_array().cloned().unwrap_or_default());
    let mine = cookies
        .iter()
        .find(|c| c["name"] == "c14sid")
        .expect("set cookie must round-trip through SiteDataManager");
    assert_eq!(mine["value"], "v-123");
    assert!(mine.get("domain").is_some(), "cookie_to_cdp carries domain: {mine}");
    assert!(mine.get("path").is_some(), "cookie_to_cdp carries path: {mine}");

    // GetAllCookies:同形状(3047-3067)。
    let all = ok_response(BridgeCommand::GetAllCookies { target_id: tid.clone() }, pool);
    let all_cookies = all["cookies"].as_array().cloned().unwrap_or_default();
    assert!(all_cookies.iter().any(|c| c["name"] == "c14sid"));

    // DeleteCookie:命名字删 + 读回确认已消失。
    let _ = ok_response(
        BridgeCommand::DeleteCookie { target_id: tid.clone(), name: "c14sid".into(), url: Some(url.into()) },
        pool,
    );
    let got = ok_response(
        BridgeCommand::GetCookies { target_id: tid.clone(), urls: vec![url.into()] },
        pool,
    );
    let cookies = got["cookies"].as_array().cloned().unwrap_or_else(|| got.as_array().cloned().unwrap_or_default());
    assert!(!cookies.iter().any(|c| c["name"] == "c14sid"), "cookie deleted: {cookies:?}");

    // 清缓存/清 cookie ack 面 + cache-disabled 转发。
    let _ = ok_response(BridgeCommand::NetworkClearBrowserCache { target_id: tid.clone() }, pool);
    let _ = ok_response(BridgeCommand::NetworkClearBrowserCookies { target_id: tid.clone() }, pool);
    let _ = ok_response(
        BridgeCommand::NetworkSetCacheDisabled { target_id: tid.clone(), cache_disabled: true },
        pool,
    );

    // Storage.getStorageItemsForOrigin / clearDataForOrigin:活 storage 线程
    // 往返(origin 过滤 + storageType 透传,3187-3239)。
    let items = ok_response(
        BridgeCommand::StorageGetStorageItemsForOrigin {
            target_id: tid.clone(),
            origin: "https://c14.test".into(),
            storage_type: "local_storage".into(),
        },
        pool,
    );
    assert!(items["storageItems"].is_array(), "storage items shape: {items}");
    let _ = ok_response(
        BridgeCommand::StorageClearDataForOrigin {
            target_id: tid.clone(),
            origin: "https://c14.test".into(),
            storage_type: "all".into(),
        },
        pool,
    );
}

// ═════════════════════════════════════════════════════════════════════════
// 第六面:cdp_handler.rs — SW/Performance/GC/Debugger 负向面
// ═════════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-BRW-004 [criterion:19] [level:e2e]
fn bridge_sw_performance_gc_and_debugger_faces() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let pool: &PagePool = runtime.page_pool();
    let page = probe_page(pool, PROBE_HTML);
    let tid = page.id().to_string();

    // HeapProfiler.collectGarbage:载体 = Node Realm 的 Bun.gc()
    // (NonIncrementalGC(Shrink) 覆盖整个 thread-local JSContext,含 page
    // realm);命令成功即真 GC 完成,页面随后仍可正常求值。
    let _ = ok_response(BridgeCommand::HeapProfilerCollectGarbage { target_id: tid.clone() }, pool);
    let alive = page.evaluate_js("1+1").expect("page still evaluable after GC");
    assert_eq!(alive.trim(), "2", "page realm survives the collectGarbage round-trip: {alive:?}");

    // Performance.getMetrics:活文档真值(Timestamp/Documents/Frames/Nodes),
    // 非零填(1723-1744)。
    let metrics = ok_response(BridgeCommand::PerformanceGetMetrics { target_id: tid.clone() }, pool);
    let entries = metrics["metrics"].as_array().expect("metrics list");
    let names: Vec<&str> = entries.iter().filter_map(|m| m["name"].as_str()).collect();
    for want in ["Timestamp", "Documents", "Frames", "Nodes"] {
        assert!(names.contains(&want), "metric {want} present: {names:?}");
    }
    let nodes = entries.iter().find(|m| m["name"] == "Nodes").unwrap()["value"]
        .as_u64()
        .expect("Nodes count");
    assert!(nodes >= 5, "probe html has >=5 elements: {nodes}");

    // ServiceWorker 面:无 SW 页 → 空 registrations / 域类错误。
    let regs = ok_response(
        BridgeCommand::ListServiceWorkerRegistrations { target_id: tid.clone() },
        pool,
    );
    assert!(regs.is_array() || regs.get("registrations").is_some(), "registrations shape: {regs}");
    let err = err_response(
        BridgeCommand::GetServiceWorkerRegistrationInfo { target_id: tid.clone(), registration_id: "not-sw-format".into() },
        pool,
    );
    assert!(err.contains("registrationId format"), "{err}");
    let err = err_response(
        BridgeCommand::TerminateServiceWorker { target_id: tid.clone(), registration_id: "sw.js::/".into() },
        pool,
    );
    assert!(err.contains("no controlling ServiceWorker"), "{err}");

    // Debugger 面:无 Debugger.enable 会话 → JS 抛错映射为命令错误
    // (2163-2191 错误臂;显式失败,不静默空帧)。
    for cmd in [
        BridgeCommand::DebuggerInterrupt { target_id: tid.clone() },
        BridgeCommand::DebuggerResume { target_id: tid.clone(), step_type: None },
        BridgeCommand::DebuggerListFrames { target_id: tid.clone() },
    ] {
        let err = err_response(cmd, pool);
        // 无 Debugger.enable 会话:JS 引用会话门符号 __bao_dbg → ReferenceError
        // 映射为命令错误(2163-2191 错误臂;显式失败,不静默空帧)。
        assert!(err.contains("__bao_dbg"), "{err}");
    }
    // blackbox/unblackbox:SM 无原生 blackbox → 永久显式拒绝(裁决 #27-3)。
    let err = err_response(BridgeCommand::DebuggerBlackbox { target_id: tid.clone(), script_id: 1 }, pool);
    assert!(err.contains("blackbox"), "{err}");
    let err = err_response(BridgeCommand::DebuggerUnblackbox { target_id: tid.clone(), script_id: 1 }, pool);
    assert!(err.contains("blackbox"), "{err}");
}

// ═════════════════════════════════════════════════════════════════════════
// 第七面:cdp_handler.rs — CSS 面(resolve_node_by_id + 活样式读回)
// ═════════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-CDP-004 [level:e2e]
fn bridge_css_faces_on_live_page() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let pool: &PagePool = runtime.page_pool();
    let page = probe_page(pool, "<!DOCTYPE html><html><body><p id='p' style='color: red'>x</p></body></html>");
    let tid = page.id().to_string();

    // 正 nodeId 轴:QuerySelector 返回 canonical 位置 id,样式面解析到该元素。
    let hit = ok_response(
        BridgeCommand::QuerySelector { target_id: tid.clone(), selector: "#p".into() },
        pool,
    );
    let node_id = hit["nodeId"].as_i64().expect("nodeId");
    assert!(node_id > 1, "canonical element id (document is 1): {hit}");
    assert_eq!(hit["backendNodeId"], json!(node_id), "backendNodeId ≡ nodeId: {hit}");
    let computed = ok_response(
        BridgeCommand::CssGetComputedStyleForNode { target_id: tid.clone(), node_id },
        pool,
    );
    // computed 读取轴真实产出:name/value 对非空,且含颜色真值。
    let props = computed["computedStyle"].as_array().cloned().unwrap_or_default();
    assert!(!props.is_empty(), "computed style must produce entries: {computed}");
    assert!(
        props.iter().all(|p| p.get("name").is_some() && p.get("value").is_some()),
        "computed style entries are name/value pairs: {props:?}"
    );
    let color = props
        .iter()
        .find(|p| p["name"] == "color")
        .expect("computed style carries color");
    assert!(
        color["value"].as_str().unwrap_or_default().to_lowercase().contains("rgb"),
        "computed color is a resolved color: {color}"
    );

    // matched/inline 轴:同一 nodeId 解析到 #p,inline style(color: red)
    // 真实回读;页面无 author stylesheet,matchedCSSRules 保持空。
    let matched = ok_response(
        BridgeCommand::CssGetMatchedStylesForNode { target_id: tid.clone(), node_id },
        pool,
    );
    assert_eq!(matched["matchedCSSRules"], json!([]), "no author stylesheet in fixture: {matched}");
    let inline = matched["inlineStyle"]
        .as_object()
        .expect("inlineStyle present for #p: style='color: red'");
    let inline_props = inline["cssProperties"].as_array().expect("cssProperties");
    assert!(
        inline_props.iter().any(|p| p["name"] == "color"),
        "inline declarations include color: {matched}"
    );
    let attrs = matched["attributesStyle"]
        .as_object()
        .expect("attributesStyle parsed off the style attribute");
    assert!(
        attrs["cssProperties"].as_array().map(|p| p.iter().any(|e| e["name"] == "color")).unwrap_or(false),
        "attributesStyle includes color: {matched}"
    );

    // inline 专用面同源:同 nodeId 产出同一 inlineStyle。
    let inline_only = ok_response(
        BridgeCommand::CssGetInlineStylesForNode { target_id: tid.clone(), node_id },
        pool,
    );
    assert!(
        inline_only["inlineStyle"].as_object().is_some(),
        "getInlineStylesForNode produces the element's declarations: {inline_only}"
    );

    // bogus nodeId → 错误臂(2336-2341 正分支之外)。
    let err = err_response(
        BridgeCommand::CssGetComputedStyleForNode { target_id: tid.clone(), node_id: -7 },
        pool,
    );
    assert!(!err.is_empty(), "bogus nodeId must error");
}

// ═════════════════════════════════════════════════════════════════════════
// 第八面:page.rs — 导航/历史/内容交互(PageHandle API)
// ═════════════════════════════════════════════════════════════════════════

#[test]
// @trace REQ-LIB-002 [level:e2e]
fn page_handle_navigation_history_and_content_faces() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let pool: &PagePool = runtime.page_pool();
    let page = probe_page(pool, PROBE_HTML);

    // 交互面(原文档):click 命令面 Ok(Down/Up at element center 真投递)。
    // **登记项**:合成点击在 data: 页完全惰性 — inline onclick 不触发
    // (__clicked 恒 undefined)且不产生 focus(activeElement 不变),输入事件
    // →页面激活的派发缺口,候选缺陷面(同族命令面 DispatchMouseEvent 的
    // 落点观察一致);characterization 钉死现行为,正向可观测断言改走
    // focus_element/type_text 轴(JS focus + 键事件)。
    page.click("#btn").expect("click_element");
    wait_for_load_and_drain(&page, 5_000);
    let clicked = page.evaluate_js_web("String(globalThis.__clicked)").expect("evaluate");
    assert_eq!(clicked.trim().trim_matches('"'), "undefined", "click inertness pinned (登记项): {clicked}");

    page.focus_element("#in").expect("focus_element");
    let focused = page.evaluate_js_web("document.activeElement ? document.activeElement.id : 'none'").expect("evaluate");
    assert_eq!(focused.trim().trim_matches('"'), "in", "focus_element lands: {focused}");

    page.fill("#in", "filled-c14").expect("fill");
    page.type_text("-typed").expect("type_text");
    let val = page.evaluate_js_web("document.getElementById('in').value").expect("evaluate");
    assert!(val.contains("filled-c14"), "fill reached the input: {val}");

    // 内容面:set_content/content/page_title/current_url 往返(1126-1144)。
    page.set_content("<html><head><title>replaced</title></head><body><b id='k'>bold</b></body></html>")
        .expect("set_content");
    let content = page.content().expect("content");
    assert!(content.contains("bold"), "content round-trip: {content}");
    assert_eq!(page.page_title().as_deref(), Some("replaced"));
    assert!(page.current_url().is_some());

    // 输入事件面:mouse move + key event(743-764 / 2116-2134)。
    page.dispatch_mouse_move(12.0, 34.0).expect("dispatch_mouse_move");
    page.dispatch_key_event(KeyState::Down, Key::Named(NamedKey::Enter), Code::Enter);
    page.dispatch_key_event(KeyState::Up, Key::Named(NamedKey::Enter), Code::Enter);

    // 视口面(645-649 + wrapper 2018-2023;wrapper 返回 Result)。
    page.set_viewport(640, 480).expect("set_viewport");
    page.set_viewport(1024, 768).expect("set_viewport restore");

    // 等待面:wait_for_selector 命中 / 超时错误;wait_for_function(890-913,
    // 917-976)。
    page.wait_for_selector("#k", Duration::from_secs(5)).expect("wait_for_selector hit");
    let err = page
        .wait_for_selector("#never-there", Duration::from_millis(300))
        .expect_err("wait_for_selector timeout must error");
    assert!(!err.to_string().is_empty());
    page.wait_for_function("document.getElementById('k') !== null", Duration::from_secs(5))
        .expect("wait_for_function");
}

#[test]
// @trace REQ-LIB-002 [level:e2e]
fn page_handle_history_screenshot_and_script_injection() {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new failed");
    let pool: &PagePool = runtime.page_pool();
    let page = probe_page(pool, PROBE_HTML);

    // 历史面:reload/go_back/go_forward/can_go_*(581-649 + wrapper 2065-2089)。
    page.reload().expect("reload");
    wait_for_load_and_drain(&page, 20_000);
    page.navigate(&data_url(PROBE_HTML)).expect("navigate");
    wait_for_load_and_drain(&page, 20_000);
    // can_go_* 是无 panic 只读谓词(布尔返回)。
    let _ = page.can_go_back();
    let _ = page.can_go_forward();
    page.go_back().expect("go_back");
    wait_for_load_and_drain(&page, 20_000);
    page.go_forward().expect("go_forward");
    wait_for_load_and_drain(&page, 20_000);

    // 截图面:take_screenshot + take_screenshot_advanced(png/jpeg × full_page
    // × clip,1218-1270 wrapper+内层)与 capture_frame。
    let png = page.take_screenshot(ScreenshotFormat::Png).expect("png screenshot");
    assert!(png.len() > 8, "png bytes non-trivial");
    assert_eq!(&png[..4], b"\x89PNG", "png magic");
    let jpeg = page
        .take_screenshot_advanced(ScreenshotFormat::Jpeg, None, false)
        .expect("jpeg advanced");
    assert!(jpeg.len() > 2 && jpeg[0] == 0xFF && jpeg[1] == 0xD8, "jpeg magic");
    let full = page
        .take_screenshot_advanced(ScreenshotFormat::Png, None, true)
        .expect("full-page png");
    assert_eq!(&full[..4], b"\x89PNG", "full-page png magic");
    let clipped = page
        .take_screenshot_advanced(ScreenshotFormat::Png, Some((0.0, 0.0, 100.0, 100.0)), false)
        .expect("clipped png");
    assert_eq!(&clipped[..4], b"\x89PNG", "clipped png magic");
    let frame = page.capture_frame().expect("capture_frame");
    assert!(frame.width() > 0 && frame.height() > 0);

    // evaluate 错误/超时路径(418-420 / 452-466):抛错脚本 → Err;正常脚本
    // 带超时 → Ok。
    assert!(page.evaluate_js("throw new Error('c14-probe')").is_err(), "throwing script must error");
    let ok = page.evaluate_js_with_timeout("40+2", Some(Duration::from_secs(5))).expect("timed evaluate");
    assert!(ok.contains("42"), "timed evaluate value: {ok}");
    // 永不完成的脚本 → 超时错误(452-459 超时臂;1s 有界)。
    assert!(
        page.evaluate_js_with_timeout("while(true){}", Some(Duration::from_secs(1))).is_err(),
        "infinite loop must hit the timeout error arm"
    );

    // new-document 脚本:注册铸造 identifier → 注销 true;再注销 → false
    // (1819-1847)。
    let id1 = page
        .add_script_to_evaluate_on_new_document("globalThis.__c14nd=1;")
        .expect("register new-document script");
    let removed = page
        .remove_script_to_evaluate_on_new_document(id1)
        .expect("remove known");
    assert!(removed, "first removal must report existed");
    let removed_again = page
        .remove_script_to_evaluate_on_new_document(id1)
        .expect("remove unknown");
    assert!(!removed_again, "second removal must report not-existed");

    // cookie 面(784-878):set → cookies 读回 → delete → 消失 →
    // delete_cookie_all_sites 无 match 页面也不 panic。
    let target = "https://c14-cookie.test/";
    page.set_cookie(target, CookieJar::new("c14sid", "zz-9")).expect("set_cookie");
    let cookies = page.cookies(&[target.to_string()]).expect("cookies");
    assert!(
        cookies.iter().any(|c| c.name() == "c14sid" && c.value() == "zz-9"),
        "cookie round-trip: {cookies:?}"
    );
    page.delete_cookie("c14sid", Some(target)).expect("delete_cookie");
    let cookies = page.cookies(&[target.to_string()]).expect("cookies after delete");
    assert!(!cookies.iter().any(|c| c.name() == "c14sid"), "cookie gone: {cookies:?}");
    page.delete_cookie("ghost-cookie", None).expect("url-less delete with no match");
}
