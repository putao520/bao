// REQ-BRW-048 DevTools CDP domain e2e: the DOM/CSS query methods return the
// live page's real data through the production dispatch + bridge + servo
// stack. The client half runs handle_command against a real BridgeSender on
// a helper thread; the main thread spins servo and drains the bridge with
// bao_browser::handle_bridge_command — the run_with_bridge loop shape minus
// the WS transport (socket routing is covered by bao_browser's
// cdp_ws_command_face_tests).
//
// Covered faces:
//   CSS.getComputedStyleForNode — engine-computed values match the page's
//     stylesheet declarations (font-size / color)
//   CSS.getInlineStylesForNode — the style attribute's real declarations
//   CSS.getMatchedStylesForNode — author rules whose selectors match, with
//     their declarations (servo CSSOM: document.styleSheets.cssRules)
//   DOM.getFlattenedDocument — document-order flat node list, parentId
//     links, children arrays, CDP depth semantics
//   DOM.getNodeForOwner — RemoteObject objectId → canonical nodeId
//
// Canonical NodeId identity (devtools_dom): root document = 1, child =
// parent*100 + childIndex + 1 — the same encoding the DOM.getDocument
// bridge walker emits, asserted here as a cross-face equality.
// @trace REQ-BRW-048 [req:REQ-BRW-048] [level:e2e]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bao_browser::{handle_bridge_command, BaoConfig, BrowserRuntime, PageConfig};
use bao_cdp::{bridge_channel, handle_command, BridgeSender, CdpMessage, CdpResponse};
use serde_json::{json, Value};

/// The styled test document. No `#` characters (a data-URL fragment would
/// truncate the document) and a single line (URL parsing drops newlines).
const STYLED_HTML: &str = r#"<html><head><style>div.t { color: rgb(12, 34, 56); font-size: 20px; } p.x { font-weight: bold; }</style></head><body><div class="t" id="target" style="margin-top: 4px">styled</div><p class="x">para</p></body></html>"#;

/// Dispatch one command against the live bridge; any error response is a
/// test failure (the error-path tests use [`dispatch_raw`]).
fn dispatch(
    bridge: &BridgeSender,
    target: &str,
    id: i64,
    method: &str,
    params: Value,
) -> Value {
    let resp = dispatch_raw(bridge, target, id, method, params);
    if let Some(e) = resp.error {
        panic!("{method} failed: {} {}", e.code, e.message);
    }
    resp.result.expect("success response carries a result")
}

/// Dispatch one command and hand back the raw CdpResponse (error-path face).
fn dispatch_raw(
    bridge: &BridgeSender,
    target: &str,
    id: i64,
    method: &str,
    params: Value,
) -> CdpResponse {
    let msg = CdpMessage {
        id: Some(id),
        method: method.to_string(),
        params: Some(params),
        session_id: None,
    };
    let params = msg.params.clone();
    handle_command(msg, target, &params, Some(bridge))
}

/// Runtime.evaluate helper that tolerates transient mid-navigation errors:
/// returns the unwrapped `result.result` value, or None while the page is
/// still navigating.
fn eval_tolerant(
    bridge: &BridgeSender,
    target: &str,
    id: i64,
    expression: &str,
) -> Option<Value> {
    let resp = dispatch_raw(
        bridge,
        target,
        id,
        "Runtime.evaluate",
        json!({ "expression": expression, "returnByValue": true }),
    );
    if resp.error.is_some() {
        return None;
    }
    resp.result.map(|r| r["result"]["value"].clone())
}

/// Wait until the navigated document exposes `div.t` — the gate is content
/// only the real document has (the about:blank placeholder also has a body,
/// so weak gates pass mid-navigation).
fn wait_div_ready(bridge: &BridgeSender, target: &str, id: i64) {
    for _ in 0..200 {
        if eval_tolerant(bridge, target, id, "!!document.querySelector('div.t')")
            == Some(json!(true))
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("navigated document never became ready");
}

/// Navigate the fresh page to the styled document and wait for it.
fn navigate_to_styled_page(bridge: &BridgeSender, target: &str) {
    let url = format!("data:text/html;charset=utf-8,{STYLED_HTML}");
    let resp = dispatch(
        bridge,
        target,
        1,
        "Page.navigate",
        json!({ "url": url }),
    );
    assert!(resp["frameId"].is_string(), "navigate returns a frameId");
    wait_div_ready(bridge, target, 2);
}

/// Depth-first search for the first node with `nodeName` in a
/// DOM.getDocument-style tree.
fn find_node<'a>(node: &'a Value, name: &str) -> Option<&'a Value> {
    if node["nodeName"].as_str() == Some(name) {
        return Some(node);
    }
    node["children"]
        .as_array()
        .and_then(|kids| kids.iter().find_map(|k| find_node(k, name)))
}

/// Depth-first search for the first node with `nodeName` in a flat
/// DOM.getFlattenedDocument node list.
fn find_flat<'a>(nodes: &'a [Value], name: &str) -> &'a Value {
    nodes
        .iter()
        .find(|n| n["nodeName"].as_str() == Some(name))
        .unwrap_or_else(|| panic!("flat nodes must contain {name}"))
}

/// Run `client` against a live page navigated to [`STYLED_HTML`]. The main
/// thread spins the servo event loop and drains the bridge with the
/// production handler while the client thread drives CDP dispatches.
fn with_styled_page(client: impl FnOnce(&BridgeSender, &str) + Send + 'static) {
    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new");
    let page = runtime
        .create_page(&PageConfig {
            url: None,
            ..Default::default()
        })
        .expect("initial page");
    let target = page.id().to_string();

    let (bridge_tx, bridge_rx) = bridge_channel(Duration::from_secs(30));
    let done = Arc::new(AtomicBool::new(false));
    let done_client = done.clone();
    let sender = bridge_tx.clone();
    let client = std::thread::spawn(move || {
        client(&sender, &target);
        done_client.store(true, Ordering::Relaxed);
    });

    let deadline = std::time::Instant::now() + Duration::from_secs(120);
    while !done.load(Ordering::Relaxed) && std::time::Instant::now() < deadline {
        runtime.spin_event_loop();
        bridge_rx.drain(|cmd| handle_bridge_command(cmd, runtime.page_pool()));
        std::thread::yield_now();
    }

    client.join().expect("client phase must not panic");
    assert!(
        done.load(Ordering::Relaxed),
        "client phase completed all assertions"
    );
}

// ---------------------------------------------------------------------------
// 1. CSS.getComputedStyleForNode — real engine-computed values
// ---------------------------------------------------------------------------

#[test]
fn computed_style_matches_page_declarations() {
    with_styled_page(|bridge, target| {
        navigate_to_styled_page(bridge, target);

        // Identity unification: the DOM.getDocument walker's nodeIds and the
        // flattened list's nodeIds must address the same node.
        let doc = dispatch(bridge, target, 3, "DOM.getDocument", json!({}));
        let doc_div = find_node(&doc["root"], "DIV").expect("document tree has the div");
        let div_id = doc_div["nodeId"].as_i64().expect("nodeId is an integer");

        let flat = dispatch(
            bridge,
            target,
            4,
            "DOM.getFlattenedDocument",
            json!({ "depth": -1 }),
        );
        let nodes = flat["nodes"].as_array().expect("nodes array");
        let flat_div = find_flat(nodes, "DIV");
        assert_eq!(
            flat_div["nodeId"].as_i64(),
            Some(div_id),
            "flatten and getDocument must share one node identity"
        );

        let cs = dispatch(
            bridge,
            target,
            5,
            "CSS.getComputedStyleForNode",
            json!({ "nodeId": div_id }),
        );
        let styles = cs["computedStyle"].as_array().expect("computedStyle array");
        assert!(
            !styles.is_empty(),
            "a styled element has a real computed style list"
        );

        // font-size: declared 20px — the engine's computed value.
        let font_size = styles
            .iter()
            .find(|s| s["name"] == "font-size")
            .unwrap_or_else(|| panic!("font-size present in {styles:?}"));
        assert_eq!(font_size["value"], "20px");

        // color: declared rgb(12, 34, 56) — match the channel-insensitive
        // serialization (rgb vs rgba form).
        let color = styles
            .iter()
            .find(|s| s["name"] == "color")
            .unwrap_or_else(|| panic!("color present in {styles:?}"));
        let color_value = color["value"].as_str().expect("color value is a string");
        assert!(
            color_value.contains("12, 34, 56"),
            "computed color must reflect the declaration, got: {color_value}"
        );
    });
}

// ---------------------------------------------------------------------------
// 2. CSS.getInlineStylesForNode — the style attribute's real declarations
// ---------------------------------------------------------------------------

#[test]
fn inline_styles_report_style_attribute() {
    with_styled_page(|bridge, target| {
        navigate_to_styled_page(bridge, target);

        let flat = dispatch(
            bridge,
            target,
            3,
            "DOM.getFlattenedDocument",
            json!({ "depth": -1 }),
        );
        let nodes = flat["nodes"].as_array().expect("nodes array");
        let div_id = find_flat(nodes, "DIV")["nodeId"].as_i64().unwrap();
        let p_id = find_flat(nodes, "P")["nodeId"].as_i64().unwrap();

        let inline = dispatch(
            bridge,
            target,
            4,
            "CSS.getInlineStylesForNode",
            json!({ "nodeId": div_id }),
        );
        let props = inline["inlineStyle"]["cssProperties"]
            .as_array()
            .expect("inlineStyle.cssProperties array");
        let margin_top = props
            .iter()
            .find(|p| p["name"] == "margin-top")
            .unwrap_or_else(|| panic!("margin-top present in {props:?}"));
        assert_eq!(margin_top["value"], "4px");
        assert_eq!(margin_top["important"], false);

        // A node without a style attribute reports a real null — there is
        // nothing to report, not a suppressed answer.
        let bare = dispatch(
            bridge,
            target,
            5,
            "CSS.getInlineStylesForNode",
            json!({ "nodeId": p_id }),
        );
        assert!(
            bare["inlineStyle"].is_null(),
            "node without a style attribute has no inline style"
        );
    });
}

// ---------------------------------------------------------------------------
// 3. CSS.getMatchedStylesForNode — matching author rules + declarations
// ---------------------------------------------------------------------------

#[test]
fn matched_styles_report_author_rules() {
    with_styled_page(|bridge, target| {
        navigate_to_styled_page(bridge, target);

        let flat = dispatch(
            bridge,
            target,
            3,
            "DOM.getFlattenedDocument",
            json!({ "depth": -1 }),
        );
        let nodes = flat["nodes"].as_array().expect("nodes array");
        let div_id = find_flat(nodes, "DIV")["nodeId"].as_i64().unwrap();
        let p_id = find_flat(nodes, "P")["nodeId"].as_i64().unwrap();

        let ms = dispatch(
            bridge,
            target,
            4,
            "CSS.getMatchedStylesForNode",
            json!({ "nodeId": div_id }),
        );
        let rules = ms["matchedCSSRules"].as_array().expect("matchedCSSRules");
        assert!(
            !rules.is_empty(),
            "the div matches the stylesheet's div.t rule"
        );
        let rule = &rules[0];
        assert_eq!(
            rule["rule"]["selectorList"]["selectors"][0]["text"],
            json!("div.t"),
            "the reported selector is the stylesheet's selector"
        );
        let decls = rule["rule"]["style"]["cssProperties"]
            .as_array()
            .expect("rule style declarations");
        let color = decls
            .iter()
            .find(|d| d["name"] == "color")
            .unwrap_or_else(|| panic!("color declaration present in {decls:?}"));
        let color_value = color["value"].as_str().expect("declared color value");
        assert!(
            color_value.contains("12, 34, 56"),
            "declared color must match the stylesheet, got: {color_value}"
        );
        assert_eq!(color["important"], false);

        // Selector fidelity: the p.x element matches p.x, not div.t.
        let ms_p = dispatch(
            bridge,
            target,
            5,
            "CSS.getMatchedStylesForNode",
            json!({ "nodeId": p_id }),
        );
        let rules_p = ms_p["matchedCSSRules"].as_array().expect("matchedCSSRules");
        let selectors: Vec<&str> = rules_p
            .iter()
            .filter_map(|r| r["rule"]["selectorList"]["selectors"][0]["text"].as_str())
            .collect();
        assert!(
            selectors.contains(&"p.x"),
            "p.x must match its own rule, got: {selectors:?}"
        );
        assert!(
            !selectors.contains(&"div.t"),
            "div.t must not match the p element, got: {selectors:?}"
        );
    });
}

// ---------------------------------------------------------------------------
// 4. DOM.getFlattenedDocument — flat list, parentId links, depth semantics
// ---------------------------------------------------------------------------

#[test]
fn flattened_document_depth_semantics_and_parent_links() {
    with_styled_page(|bridge, target| {
        navigate_to_styled_page(bridge, target);

        // depth: 1 → root plus one level of children (document + doctype +
        // html). BODY is two levels down and must NOT appear.
        let shallow = dispatch(
            bridge,
            target,
            3,
            "DOM.getFlattenedDocument",
            json!({ "depth": 1 }),
        );
        let shallow_nodes = shallow["nodes"].as_array().expect("nodes array");
        assert_eq!(
            shallow_nodes[0]["nodeId"],
            1,
            "the root document is nodeId 1"
        );
        assert!(
            shallow_nodes
                .iter()
                .any(|n| n["nodeName"].as_str() == Some("HTML")),
            "depth 1 includes the html element"
        );
        assert!(
            shallow_nodes.iter().all(|n| n["nodeName"].as_str() != Some("BODY")),
            "depth 1 must not descend to BODY, got: {shallow_nodes:?}"
        );

        // Every node except the root carries parentId.
        for node in &shallow_nodes[1..] {
            assert!(
                node.get("parentId").is_some(),
                "non-root node lacks parentId: {node}"
            );
        }

        // depth: -1 → the entire subtree.
        let full = dispatch(
            bridge,
            target,
            4,
            "DOM.getFlattenedDocument",
            json!({ "depth": -1 }),
        );
        let nodes = full["nodes"].as_array().expect("nodes array");
        let body = find_flat(nodes, "BODY");
        let div = find_flat(nodes, "DIV");
        assert!(
            div.get("parentId").is_some() && div["parentId"] == body["nodeId"],
            "the div's parentId links to the body"
        );

        // Element nodes carry their real attributes as a flat name/value list.
        let attrs = div["attributes"].as_array().expect("attributes array");
        let class_idx = attrs
            .iter()
            .position(|a| a.as_str() == Some("class"))
            .expect("class attribute present");
        assert_eq!(attrs[class_idx + 1], "t", "class attribute value");

        // The div's text child is in the flat list and linked back.
        let div_id = div["nodeId"].as_i64().unwrap();
        let text_nodes: Vec<&Value> = nodes
            .iter()
            .filter(|n| n["parentId"].as_i64() == Some(div_id))
            .collect();
        assert!(
            text_nodes
                .iter()
                .any(|n| n["nodeType"].as_i64() == Some(3)),
            "the div's text child is flattened with a parentId link"
        );
    });
}

// ---------------------------------------------------------------------------
// 5. DOM.getNodeForOwner — RemoteObject objectId → canonical nodeId
// ---------------------------------------------------------------------------

#[test]
fn node_for_owner_maps_object_id_to_canonical_node_id() {
    with_styled_page(|bridge, target| {
        navigate_to_styled_page(bridge, target);

        let flat = dispatch(
            bridge,
            target,
            3,
            "DOM.getFlattenedDocument",
            json!({ "depth": -1 }),
        );
        let nodes = flat["nodes"].as_array().expect("nodes array");
        let div_id = find_flat(nodes, "DIV")["nodeId"].as_i64().unwrap();
        let html_id = find_flat(nodes, "HTML")["nodeId"].as_i64().unwrap();

        // RemoteObject handle → nodeId roundtrip. dispatch() returns the
        // CDP result value, which for returnByValue:false is
        // {result: RemoteObject, exceptionDetails} — the objectId lives at
        // ev["result"]["objectId"].
        let ev = dispatch(
            bridge,
            target,
            4,
            "Runtime.evaluate",
            json!({
                "expression": "document.querySelector('div.t')",
                "returnByValue": false
            }),
        );
        let object_id = ev["result"]["objectId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "evaluateHandle mints an objectId, got: {ev}"
                )
            })
            .to_string();

        let owner = dispatch(
            bridge,
            target,
            5,
            "DOM.getNodeForOwner",
            json!({ "objectId": object_id }),
        );
        assert_eq!(
            owner["nodeId"].as_i64(),
            Some(div_id),
            "the object's nodeId is the canonical DOM-face id"
        );

        // Legacy "node-N" DOM handles normalize to the same identity space.
        let legacy = dispatch(
            bridge,
            target,
            6,
            "DOM.getNodeForOwner",
            json!({ "objectId": "node-2" }),
        );
        assert_eq!(
            legacy["nodeId"].as_i64(),
            Some(html_id),
            "legacy node-2 (documentElement) resolves to the html nodeId"
        );

        // Dead handle → explicit error, never a fabricated id.
        let bogus = dispatch_raw(
            bridge,
            target,
            7,
            "DOM.getNodeForOwner",
            json!({ "objectId": "obj-does-not-exist" }),
        );
        let err = bogus.error.expect("unknown objectId must fail");
        assert_eq!(err.code, -32000, "Chrome's DevTools server-error class");

        // A non-Node RemoteObject is not a node — explicit error.
        let ev_obj = dispatch(
            bridge,
            target,
            8,
            "Runtime.evaluate",
            json!({ "expression": "({a: 1})", "returnByValue": false }),
        );
        let plain_object_id = ev_obj["result"]["objectId"]
            .as_str()
            .expect("plain object handle")
            .to_string();
        let not_node = dispatch_raw(
            bridge,
            target,
            9,
            "DOM.getNodeForOwner",
            json!({ "objectId": plain_object_id }),
        );
        let err = not_node.error.expect("non-Node object must fail");
        assert_eq!(err.code, -32000);
        assert!(err.message.contains("not a DOM Node"));
    });
}

// ---------------------------------------------------------------------------
// 6. Fail-closed data paths — unknown node ids and dead object registries
// ---------------------------------------------------------------------------

#[test]
fn css_dom_query_fail_closed_on_unknown_node() {
    with_styled_page(|bridge, target| {
        navigate_to_styled_page(bridge, target);

        // An addressable-shape id that matches no real node → -32000
        // (Chrome's "No node with given id found" class), never an empty
        // style list.
        for method in [
            "CSS.getComputedStyleForNode",
            "CSS.getMatchedStylesForNode",
            "CSS.getInlineStylesForNode",
        ] {
            let resp = dispatch_raw(
                bridge,
                target,
                3,
                method,
                json!({ "nodeId": 999_999 }),
            );
            let err = resp
                .error
                .unwrap_or_else(|| panic!("{method} must fail on an unknown node"));
            assert_eq!(err.code, -32000, "{method}: {}", err.message);
            assert!(err.message.contains("No node with given id found"));
        }

        // Non-element node (the document itself, nodeId 1) — styles require
        // an Element, and the face says so instead of returning a list.
        let resp = dispatch_raw(
            bridge,
            target,
            4,
            "CSS.getComputedStyleForNode",
            json!({ "nodeId": 1 }),
        );
        let err = resp.error.expect("document node must fail the style query");
        assert_eq!(err.code, -32000);
        assert!(err.message.contains("not an element"));
    });
}

// ===========================================================================
// REQ-BRW-048 follow-up wave: the remaining DevTools panel faces —
// querySelector/querySelectorAll canonical ids, describeNode, getBoxModel,
// resolveNode, pushNodesByBackendIdsToFrontend, setStyleTexts. Same harness,
// same canonical identity SSOT.
// ===========================================================================

/// Dispatch without a bridge (pure dispatch face — no runtime needed).
fn dispatch_no_bridge(method: &str, params: Value) -> CdpResponse {
    let msg = CdpMessage {
        id: Some(1),
        method: method.to_string(),
        params: Some(params),
        session_id: None,
    };
    let params = msg.params.clone();
    handle_command(msg, "t", &params, None)
}

// ---------------------------------------------------------------------------
// 7. querySelector / querySelectorAll — canonical nodeIds, no fake sequences
// ---------------------------------------------------------------------------

#[test]
fn query_selector_returns_canonical_node_ids() {
    with_styled_page(|bridge, target| {
        navigate_to_styled_page(bridge, target);

        let flat = dispatch(
            bridge,
            target,
            3,
            "DOM.getFlattenedDocument",
            json!({ "depth": -1 }),
        );
        let nodes = flat["nodes"].as_array().expect("nodes array");
        let div_id = find_flat(nodes, "DIV")["nodeId"].as_i64().unwrap();
        let p_id = find_flat(nodes, "P")["nodeId"].as_i64().unwrap();

        // querySelector → the same canonical id the flatten face reported.
        let qs = dispatch(
            bridge,
            target,
            4,
            "DOM.querySelector",
            json!({ "selector": "div.t" }),
        );
        assert_eq!(
            qs["nodeId"].as_i64(),
            Some(div_id),
            "querySelector must return the canonical identity"
        );

        // No match → nodeId 0 (the CDP convention, on a live page).
        let miss = dispatch(
            bridge,
            target,
            5,
            "DOM.querySelector",
            json!({ "selector": "#does-not-exist" }),
        );
        assert_eq!(miss["nodeId"], 0);

        // querySelectorAll → real ids in document order (the former
        // 1..=count sequence addressed wrong nodes).
        let qsa = dispatch(
            bridge,
            target,
            6,
            "DOM.querySelectorAll",
            json!({ "selector": "body > *" }),
        );
        let ids: Vec<i64> = qsa["nodeIds"]
            .as_array()
            .expect("nodeIds array")
            .iter()
            .map(|v| v.as_i64().expect("integer id"))
            .collect();
        assert_eq!(ids, vec![div_id, p_id]);
    });
}

// ---------------------------------------------------------------------------
// 8. describeNode — real node data, children within depth
// ---------------------------------------------------------------------------

#[test]
fn describe_node_reports_live_node_data() {
    with_styled_page(|bridge, target| {
        navigate_to_styled_page(bridge, target);

        let flat = dispatch(
            bridge,
            target,
            3,
            "DOM.getFlattenedDocument",
            json!({ "depth": -1 }),
        );
        let nodes = flat["nodes"].as_array().expect("nodes array");
        let flat_div = find_flat(nodes, "DIV").clone();
        let div_id = flat_div["nodeId"].as_i64().unwrap();

        let desc = dispatch(
            bridge,
            target,
            4,
            "DOM.describeNode",
            json!({ "nodeId": div_id, "depth": 1 }),
        );
        let node = &desc["node"];

        // Fields agree with the flatten face for the same node.
        assert_eq!(node["nodeId"], flat_div["nodeId"]);
        assert_eq!(node["nodeName"], "DIV");
        assert_eq!(node["nodeType"], flat_div["nodeType"]);
        assert_eq!(node["childNodeCount"], flat_div["childNodeCount"]);
        assert_eq!(node["attributes"], flat_div["attributes"]);

        // depth 1 → children present (parentId-linked), grandchildren not.
        let children = node["children"].as_array().expect("children array");
        assert!(
            !children.is_empty(),
            "the styled div has a text child"
        );
        for child in children {
            assert_eq!(child["parentId"], div_id);
            assert!(
                child.get("children").is_none(),
                "depth 1 must not descend past one level: {child}"
            );
        }

        // depth discrimination must be observable: describe the body, whose
        // grandchildren exist. depth 1 → children carry no "children" key;
        // depth -1 → the whole subtree (the div child has its text child).
        let body_id = find_flat(nodes, "BODY")["nodeId"].as_i64().unwrap();
        let body_shallow = dispatch(
            bridge,
            target,
            5,
            "DOM.describeNode",
            json!({ "nodeId": body_id, "depth": 1 }),
        );
        let shallow_kids = body_shallow["node"]["children"].as_array().unwrap();
        assert!(!shallow_kids.is_empty());
        for kid in shallow_kids {
            assert!(
                kid.get("children").is_none(),
                "depth 1 must not descend below one level: {kid}"
            );
        }
        let body_full = dispatch(
            bridge,
            target,
            6,
            "DOM.describeNode",
            json!({ "nodeId": body_id, "depth": -1 }),
        );
        let full_kids = body_full["node"]["children"].as_array().unwrap();
        let div_child = full_kids
            .iter()
            .find(|k| k["nodeName"] == "DIV")
            .expect("body subtree contains the div");
        assert!(
            div_child.get("children").is_some(),
            "depth -1 descends below one level"
        );

        // objectId form resolves the same node.
        let ev = dispatch(
            bridge,
            target,
            7,
            "Runtime.evaluate",
            json!({ "expression": "document.querySelector('div.t')", "returnByValue": false }),
        );
        let object_id = ev["result"]["objectId"].as_str().unwrap().to_string();
        let by_oid = dispatch(
            bridge,
            target,
            8,
            "DOM.describeNode",
            json!({ "objectId": object_id }),
        );
        assert_eq!(by_oid["node"]["nodeId"], flat_div["nodeId"]);
    });
}

// ---------------------------------------------------------------------------
// 9. getBoxModel — real geometry, cross-checked against the page's own rect
// ---------------------------------------------------------------------------

#[test]
fn get_box_model_reports_page_rect() {
    with_styled_page(|bridge, target| {
        navigate_to_styled_page(bridge, target);

        let flat = dispatch(
            bridge,
            target,
            3,
            "DOM.getFlattenedDocument",
            json!({ "depth": -1 }),
        );
        let div_id = find_flat(flat["nodes"].as_array().unwrap(), "DIV")["nodeId"]
            .as_i64()
            .unwrap();

        // Independent read: the page reports its own border box.
        let baseline = dispatch(
            bridge,
            target,
            4,
            "Runtime.evaluate",
            json!({
                "expression": "(function(){ var r = document.querySelector('div.t').getBoundingClientRect(); return JSON.stringify({x: r.left, y: r.top, w: r.width, h: r.height}); })()",
                "returnByValue": true
            }),
        );
        let bx = baseline["result"]["value"]["x"].as_f64().unwrap();
        let by = baseline["result"]["value"]["y"].as_f64().unwrap();
        let bw = baseline["result"]["value"]["w"].as_f64().unwrap();
        let bh = baseline["result"]["value"]["h"].as_f64().unwrap();

        let bm = dispatch(
            bridge,
            target,
            5,
            "DOM.getBoxModel",
            json!({ "nodeId": div_id }),
        );
        let model = &bm["model"];
        let width = model["width"].as_f64().unwrap();
        let height = model["height"].as_f64().unwrap();
        assert!(
            (width - bw).abs() < 0.5 && (height - bh).abs() < 0.5,
            "model border box {width}x{height} must match the page rect {bw}x{bh}"
        );

        // The border quad starts at the rect's top-left corner; the content
        // quad is inset inside it (the div has no border/padding here).
        let border = model["border"].as_array().unwrap();
        let border_x = border[0].as_f64().unwrap();
        let border_y = border[1].as_f64().unwrap();
        assert!((border_x - bx).abs() < 0.5 && (border_y - by).abs() < 0.5);
        let content = model["content"].as_array().unwrap();
        assert_eq!(content.len(), 8, "content is a 4-corner quad");
        assert!(content[0].as_f64().unwrap() >= border_x - 0.5);
        assert!(content[1].as_f64().unwrap() >= border_y - 0.5);
    });
}

// ---------------------------------------------------------------------------
// 10. resolveNode — nodeId → RemoteObject objectId → getNodeForOwner roundtrip
// ---------------------------------------------------------------------------

#[test]
fn resolve_node_roundtrips_to_canonical_node_id() {
    with_styled_page(|bridge, target| {
        navigate_to_styled_page(bridge, target);

        let flat = dispatch(
            bridge,
            target,
            3,
            "DOM.getFlattenedDocument",
            json!({ "depth": -1 }),
        );
        let div_id = find_flat(flat["nodes"].as_array().unwrap(), "DIV")["nodeId"]
            .as_i64()
            .unwrap();

        // The registry is Runtime-minted — establish it the way a client
        // would (returnByValue:false evaluate).
        dispatch(
            bridge,
            target,
            4,
            "Runtime.evaluate",
            json!({ "expression": "({})", "returnByValue": false }),
        );

        let resolved = dispatch(
            bridge,
            target,
            5,
            "DOM.resolveNode",
            json!({ "nodeId": div_id }),
        );
        let object = &resolved["object"];
        assert_eq!(object["subtype"], "node");
        assert_eq!(object["description"], "div#target");
        let object_id = object["objectId"].as_str().expect("minted objectId");

        // Roundtrip: the handle maps back to the same canonical nodeId.
        let owner = dispatch(
            bridge,
            target,
            6,
            "DOM.getNodeForOwner",
            json!({ "objectId": object_id }),
        );
        assert_eq!(owner["nodeId"].as_i64(), Some(div_id));

        // Before any registry-minting evaluate, resolveNode fails explicitly
        // rather than conjuring a parallel registry.
        let fresh_runtime_check = dispatch_raw(
            bridge,
            target,
            7,
            "DOM.resolveNode",
            json!({ "objectId": "obj-never-minted" }),
        );
        let err = fresh_runtime_check.error.expect("dead handle must fail");
        assert_eq!(err.code, -32000);
    });
}

// ---------------------------------------------------------------------------
// 11. pushNodesByBackendIdsToFrontend — backend ≡ canonical, liveness only
// ---------------------------------------------------------------------------

#[test]
fn push_nodes_by_backend_ids_frontends_canonical_ids() {
    with_styled_page(|bridge, target| {
        navigate_to_styled_page(bridge, target);

        let flat = dispatch(
            bridge,
            target,
            3,
            "DOM.getFlattenedDocument",
            json!({ "depth": -1 }),
        );
        let nodes = flat["nodes"].as_array().unwrap();
        let div_id = find_flat(nodes, "DIV")["nodeId"].as_i64().unwrap();
        let body_id = find_flat(nodes, "BODY")["nodeId"].as_i64().unwrap();

        let resp = dispatch(
            bridge,
            target,
            4,
            "DOM.pushNodesByBackendIdsToFrontend",
            json!({ "backendNodeIds": [div_id, 999_999, body_id] }),
        );
        let ids: Vec<i64> = resp["nodeIds"]
            .as_array()
            .expect("nodeIds array")
            .iter()
            .map(|v| v.as_i64().unwrap())
            .collect();
        assert_eq!(
            ids,
            vec![div_id, body_id],
            "live ids pass through, unknown ids are omitted (never fabricated)"
        );
    });
}

// ---------------------------------------------------------------------------
// 12. setStyleTexts — real CSSOM write, read back through the style faces
// ---------------------------------------------------------------------------

#[test]
fn set_style_texts_writes_and_reads_back() {
    with_styled_page(|bridge, target| {
        navigate_to_styled_page(bridge, target);

        let flat = dispatch(
            bridge,
            target,
            3,
            "DOM.getFlattenedDocument",
            json!({ "depth": -1 }),
        );
        let div_id = find_flat(flat["nodes"].as_array().unwrap(), "DIV")["nodeId"]
            .as_i64()
            .unwrap();

        // Discover the stylesheet + rule through the read face: the sheet
        // ordinal is the styleSheetId; the div.t rule is index 0 in the
        // test sheet (declared before p.x).
        let matched = dispatch(
            bridge,
            target,
            4,
            "CSS.getMatchedStylesForNode",
            json!({ "nodeId": div_id }),
        );
        let rule0 = &matched["matchedCSSRules"][0];
        assert_eq!(rule0["rule"]["selectorList"]["selectors"][0]["text"], "div.t");
        let style_sheet_id = rule0["rule"]["styleSheetId"]
            .as_str()
            .expect("matched rules expose the stylesheet ordinal")
            .to_string();

        let edit = |text: &str| {
            dispatch(
                bridge,
                target,
                8,
                "CSS.setStyleTexts",
                json!({
                    "edits": [{
                        "styleSheetId": style_sheet_id,
                        "range": { "startLine": 0 },
                        "text": text
                    }]
                })
            )
        };

        // Write: new declarations replace the rule's declaration block.
        let written = edit("color: rgb(9, 9, 9); font-size: 40px;");
        let style = &written["styles"][0];
        assert_eq!(style["styleSheetId"], json!(style_sheet_id));
        let props = style["cssProperties"].as_array().expect("cssProperties");
        let color = props
            .iter()
            .find(|p| p["name"] == "color")
            .expect("color declaration");
        assert!(color["value"].as_str().unwrap().contains("9, 9, 9"));

        // Read-back 1: the computed style really restyled (not a stored echo).
        let cs = dispatch(
            bridge,
            target,
            9,
            "CSS.getComputedStyleForNode",
            json!({ "nodeId": div_id }),
        );
        let styles = cs["computedStyle"].as_array().unwrap();
        let fs = styles.iter().find(|s| s["name"] == "font-size").unwrap();
        assert_eq!(fs["value"], "40px", "the write must restyle the page");

        // Read-back 2: the matched-rule declarations reflect the edit.
        let matched2 = dispatch(
            bridge,
            target,
            10,
            "CSS.getMatchedStylesForNode",
            json!({ "nodeId": div_id }),
        );
        let decls = matched2["matchedCSSRules"][0]["rule"]["style"]["cssProperties"]
            .as_array()
            .unwrap();
        let fs_decl = decls.iter().find(|d| d["name"] == "font-size").unwrap();
        assert_eq!(fs_decl["value"], "40px");

        // Restore the original declaration (clean state for later waves).
        edit("color: rgb(12, 34, 56); font-size: 20px;");
        let cs2 = dispatch(
            bridge,
            target,
            11,
            "CSS.getComputedStyleForNode",
            json!({ "nodeId": div_id }),
        );
        let styles2 = cs2["computedStyle"].as_array().unwrap();
        let fs2 = styles2.iter().find(|s| s["name"] == "font-size").unwrap();
        assert_eq!(fs2["value"], "20px", "restore returns the page to baseline");

        // Dead ends are explicit: unknown sheet ordinal, missing range,
        // non-style rule target.
        for (edits, needle) in [
            (
                json!([{ "styleSheetId": "9", "range": { "startLine": 0 }, "text": "color: red" }]),
                "unknown styleSheetId",
            ),
            (
                json!([{ "styleSheetId": style_sheet_id, "text": "color: red" }]),
                "range must carry the rule index",
            ),
        ] {
            let resp = dispatch_raw(bridge, target, 12, "CSS.setStyleTexts", json!({ "edits": edits }));
            let err = resp.error.expect("bad edit must fail explicitly");
            assert_eq!(err.code, -32000, "{needle}: {}", err.message);
            assert!(err.message.contains(needle));
        }
    });
}

// ---------------------------------------------------------------------------
// 13. No-bridge face: every live-document query fails closed (-32603 / -32602)
// ---------------------------------------------------------------------------

#[test]
fn dom_live_query_methods_fail_closed_without_bridge() {
    // Pure dispatch face — no runtime, no bridge.
    let with_node = json!({ "nodeId": 1 });
    for (method, params, code) in [
        ("DOM.getDocument", json!({}), -32603),
        ("DOM.querySelector", json!({ "selector": "div" }), -32603),
        ("DOM.querySelectorAll", json!({ "selector": "div" }), -32603),
        ("DOM.describeNode", with_node.clone(), -32603),
        ("DOM.getBoxModel", with_node.clone(), -32603),
        ("DOM.resolveNode", with_node.clone(), -32603),
        ("DOM.pushNodesByBackendIdsToFrontend", json!({ "backendNodeIds": [1] }), -32603),
        // Missing required params are rejected before the bridge check.
        ("DOM.querySelector", json!({}), -32602),
        ("DOM.describeNode", json!({}), -32602),
        ("DOM.getBoxModel", json!({}), -32602),
        ("DOM.resolveNode", json!({}), -32602),
        ("DOM.pushNodesByBackendIdsToFrontend", json!({}), -32602),
        ("CSS.setStyleTexts", json!({}), -32602),
    ] {
        let resp = dispatch_no_bridge(method, params);
        let err = resp
            .error
            .unwrap_or_else(|| panic!("{method}: explicit error required"));
        assert_eq!(err.code, code, "{method}: {}", err.message);
        assert!(resp.result.is_none(), "{method}: error carries no result");
    }
}
