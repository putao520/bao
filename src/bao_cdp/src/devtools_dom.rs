// REQ-BRW-048: DevTools DOM/CSS method data face — canonical NodeId identity
// and the page-evaluation channel the DOM/CSS query methods run through.
// @trace REQ-BRW-048 [level:e2e]
//
// ── Canonical NodeId identity (bao_cdp-owned) ──────────────────────────────
//
// Every DOM/CSD query method resolves nodes by the SAME positional encoding:
// the root document is nodeId 1, and the i-th child of the node with id P is
// `P * 100 + i + 1` (indices are `childNodes` indices — text/comment nodes
// included). This is the encoding the servo-bridge `DOM.getDocument` walker
// emits (bao_browser cmd_get_document), so DOM.getDocument trees,
// DOM.getFlattenedDocument output and every nodeId accepted by the CSS query
// methods share one identity space.
//
// Known bound (documented deviation from Chrome's registry-allocated ids):
// each tree level multiplies the id by 100, so ids exceed JS safe-integer
// precision beyond ~7 nested levels. Nodes past that bound are NOT
// addressable — the methods below report an explicit error for them instead
// of silently returning a wrong id.
//
// The evaluation channel is the existing `BridgeCommand::EvaluateJs`
// (web-scope page realm) — no new bridge channel is introduced, and the
// Web-scope security contract (REQ-SEC-002/003) applies: these are page
// queries, not privileged bao code.

use serde_json::Value;

use crate::protocol::{
    eval_json, CdpError, HandlerResult, ERR_INVALID_PARAMS, ERR_NOT_SUPPORTED,
};
use crate::servo_bridge::BridgeSender;

/// JS prelude inlined into every page evaluation below: the canonical node-id
/// resolver (`__baoResolve`) and its inverse (`__baoCanonical`). Pure
/// functions of the live DOM — no page state is created or mutated.
const IDENTITY_JS: &str = r#"
var __baoResolve = function(id) {
    if (!(typeof id === 'number' && Number.isSafeInteger(id) && id >= 1)) return null;
    if (id === 1) return document;
    var q = id - 1;
    var parent = __baoResolve(Math.floor(q / 100));
    if (!parent) return null;
    return parent.childNodes[q % 100] || null;
};
var __baoCanonical = function(node) {
    var chain = [];
    var n = node;
    while (n && n !== document) {
        var p = n.parentNode;
        if (!p) return null;
        var i = Array.prototype.indexOf.call(p.childNodes, n);
        if (i < 0) return null;
        chain.push(i);
        n = p;
    }
    if (n !== document) return null;
    var id = 1;
    for (var k = chain.length - 1; k >= 0; k--) {
        id = id * 100 + chain[k] + 1;
        if (!Number.isSafeInteger(id)) return null;
    }
    return id;
};
var __baoDecls = function(style) {
    var css = [];
    for (var i = 0; i < style.length; i++) {
        var name = style[i];
        css.push({
            name: name,
            value: style.getPropertyValue(name),
            important: style.getPropertyPriority(name) === "important"
        });
    }
    return { cssProperties: css, shorthandEntries: [] };
};
var __baoNodeFields = function(node, id) {
    var e = {
        nodeId: id,
        backendNodeId: id,
        nodeType: node.nodeType,
        nodeName: node.nodeName,
        localName: node.localName || "",
        nodeValue: node.nodeValue || "",
        childNodeCount: node.childNodes.length
    };
    if (node.nodeType === 1) {
        var attrs = [];
        for (var a = 0; a < node.attributes.length; a++) {
            attrs.push(node.attributes[a].name);
            attrs.push(node.attributes[a].value);
        }
        e.attributes = attrs;
    }
    return e;
};
"#;

/// Read the required integer `nodeId` param (CDP: required for the CSS query
/// methods). Missing/malformed → -32602, evaluated before any bridge round
/// trip so the error names the request, not the transport.
pub(crate) fn require_node_id(params: &Option<Value>) -> Result<i64, CdpError> {
    params
        .as_ref()
        .and_then(|p| p.get("nodeId"))
        .and_then(|v| v.as_i64())
        .ok_or_else(|| CdpError {
            code: ERR_INVALID_PARAMS,
            message: "missing required parameter: nodeId".into(),
        })
}

/// Read the required non-empty string `objectId` param (CDP: required for
/// DOM.getNodeForOwner). Missing → -32602.
pub(crate) fn require_object_id(params: &Option<Value>) -> Result<String, CdpError> {
    params
        .as_ref()
        .and_then(|p| p.get("objectId"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .ok_or_else(|| CdpError {
            code: ERR_INVALID_PARAMS,
            message: "missing required parameter: objectId".into(),
        })
}

/// Evaluate `body` on the target page (with the identity prelude) and return
/// the JSON document it produced. The body must end by returning
/// `JSON.stringify({...})`; returning `{"__bao_err": "<reason>"}` fails the
/// command with an explicit -32000 carrying that reason — page-side dead ends
/// (unknown node id, detached node, missing registry) are never flattened
/// into a shape-only success.
pub(crate) fn eval_dom(
    bridge: Option<&BridgeSender>,
    target_id: &str,
    body: &str,
) -> HandlerResult {
    let expression = format!("(function() {{{IDENTITY_JS}{body}\n}})()");
    let doc = eval_json(bridge, target_id, &expression)?;
    if let Some(reason) = doc.get("__bao_err").and_then(|e| e.as_str()) {
        // -32000 is Chrome's DevTools server-error class (its own
        // "No node with given id found" response uses exactly this code).
        return Err(CdpError {
            code: ERR_NOT_SUPPORTED,
            message: reason.to_string(),
        });
    }
    Ok(doc)
}

/// Resolve a positional nodeId and hand the element to `on_element`. Shared
/// tail of the CSS query bodies: unknown ids and non-element nodes are
/// explicit page-side errors (Chrome semantics — never an empty style list).
pub(crate) fn css_element_body(node_id: i64, on_element: &str) -> String {
    format!(
        r#"var el = __baoResolve({node_id});
if (!el) return JSON.stringify({{__bao_err: "No node with given id found"}});
if (el.nodeType !== 1) return JSON.stringify({{__bao_err: "node is not an element (styles require an Element node)"}});
{on_element}"#,
        node_id = node_id,
        on_element = on_element,
    )
}

/// Build the `CSS.getComputedStyleForNode` body: every property is the
/// engine's real computed value read through `getComputedStyle`.
pub(crate) fn computed_style_body(node_id: i64) -> String {
    css_element_body(
        node_id,
        r#"var styles = window.getComputedStyle(el);
var out = [];
for (var i = 0; i < styles.length; i++) {
    var name = styles[i];
    out.push({ name: name, value: styles.getPropertyValue(name) });
}
return JSON.stringify({ computedStyle: out });"#,
    )
}

/// Build the `CSS.getInlineStylesForNode` body: the style attribute's real
/// declarations via the element's CSSStyleDeclaration.
pub(crate) fn inline_styles_body(node_id: i64) -> String {
    css_element_body(
        node_id,
        r#"var inline = null;
if (el.style && el.style.length > 0) inline = __baoDecls(el.style);
return JSON.stringify({ inlineStyle: inline, attributesStyle: null });"#,
    )
}

/// Build the `CSS.getMatchedStylesForNode` body: author stylesheet rules
/// whose selectors match the element, with their real declarations.
/// `document.styleSheets` holds the author sheets only (origin "regular");
/// grouping rules (media/supports) are not descended on this face.
pub(crate) fn matched_styles_body(node_id: i64) -> String {
    css_element_body(
        node_id,
        r#"var rules = [];
var sheets = document.styleSheets;
for (var s = 0; s < sheets.length; s++) {
    var sheetRules = null;
    try { sheetRules = sheets[s].cssRules; } catch (e) { sheetRules = null; }
    if (!sheetRules) continue;
    for (var r = 0; r < sheetRules.length; r++) {
        var rule = sheetRules[r];
        if (!rule || typeof rule.selectorText !== "string" || !rule.style) continue;
        var matching = [];
        var selectors = rule.selectorText.split(",");
        for (var si = 0; si < selectors.length; si++) {
            var sel = selectors[si].replace(/^\s+|\s+$/g, "");
            if (!sel) continue;
            try { if (el.matches(sel)) matching.push(si); } catch (e2) {}
        }
        if (matching.length === 0) continue;
        rules.push({
            rule: {
                selectorList: { selectors: [{ text: rule.selectorText }] },
                style: __baoDecls(rule.style),
                origin: "regular",
                sourceURL: sheets[s].href || "",
                styleSheetId: String(s)
            },
            matchingSelectors: matching
        });
    }
}
var inline = null;
if (el.style && el.style.length > 0) inline = __baoDecls(el.style);
return JSON.stringify({ matchedCSSRules: rules, inlineStyle: inline, attributesStyle: null });"#,
    )
}

/// Build the `DOM.getFlattenedDocument` body: the live tree flattened into a
/// document-order node list. `depth` follows the CDP contract (default 1 =
/// root plus one level of children; negative = the entire subtree); every
/// node except the root carries `parentId`, and `children` arrays are
/// attached for nodes whose children are within depth. `pierce` has no
/// observable effect: the exposed DOM face has no shadow-root boundary, so
/// this walk IS the whole exposed tree.
pub(crate) fn flattened_document_body(depth: i64) -> String {
    format!(
        r#"var depth = {depth};
var nodes = [];
function push(node, parentId, level) {{
    var id = __baoCanonical(node);
    if (id === null) throw "node-id-overflow";
    var e = __baoNodeFields(node, id);
    if (parentId !== 0) e.parentId = parentId;
    nodes.push(e);
    var within = (depth < 0) || (level < depth);
    if (e.childNodeCount > 0 && within) {{
        e.children = [];
        for (var i = 0; i < node.childNodes.length; i++) {{
            e.children.push(push(node.childNodes[i], id, level + 1));
        }}
    }}
    return e;
}}
var root;
try {{ root = push(document, 0, 0); }}
catch (e) {{
    if (e === "node-id-overflow") return JSON.stringify({{__bao_err: "subtree exceeds addressable node-id precision (positional ids are safe through ~7 nested levels)"}});
    throw e;
}}
return JSON.stringify({{ nodes: nodes }});"#,
        depth = depth,
    )
}

/// Build the `DOM.getNodeForOwner` body: map a RemoteObject objectId (page
/// registry entry minted by Runtime.evaluate with returnByValue:false, or the
/// legacy "node-N" DOM handle) back to the canonical nodeId of the Node it
/// references. Dead handles (navigated-away document, released object,
/// non-Node value) are explicit errors.
pub(crate) fn node_for_owner_body(object_id: &str) -> String {
    let oid_json = serde_json::to_string(object_id).unwrap_or_default();
    format!(
        r#"var oid = {oid_json};
var v = null;
if (oid.lastIndexOf("node-", 0) === 0) {{
    var n = parseInt(oid.slice(5), 10);
    v = (n <= 1) ? document
        : (n === 2 ? document.documentElement
                   : (document.body ? document.body.childNodes[n - 3] : null));
}} else {{
    if (!window.__bao_cdp) return JSON.stringify({{__bao_err: "object registry not present: obtain objectIds via Runtime.evaluate(returnByValue:false) on this document"}});
    v = window.__bao_cdp.get(oid);
}}
if (v === null || v === undefined) return JSON.stringify({{__bao_err: "No object with given id found"}});
if (typeof Node === "undefined" || !(v instanceof Node)) return JSON.stringify({{__bao_err: "object is not a DOM Node"}});
var id = __baoCanonical(v);
if (id === null) return JSON.stringify({{__bao_err: "node is detached from the document or exceeds addressable node-id precision"}});
return JSON.stringify({{ nodeId: id, backendNodeId: id }});"#,
        oid_json = oid_json,
    )
}

// ---------------------------------------------------------------------------
// REQ-BRW-048 follow-up wave: the remaining DOM/CSS query + write faces,
// all through the same canonical identity + page-evaluation channel.
// ---------------------------------------------------------------------------

/// A CDP node reference: `nodeId` (canonical positional id) or `objectId`
/// (page-realm registry handle). describeNode/getBoxModel/resolveNode accept
/// either; neither present is a -32602.
pub(crate) enum NodeRef {
    NodeId(i64),
    ObjectId(String),
}

/// Read `nodeId` or `objectId` (whichever the request carries; nodeId wins on
/// both — Chrome's tie-break is unspecified and this face is single-realm).
pub(crate) fn require_node_ref(params: &Option<Value>) -> Result<NodeRef, CdpError> {
    let node_id = params
        .as_ref()
        .and_then(|p| p.get("nodeId"))
        .and_then(|v| v.as_i64());
    let object_id = params
        .as_ref()
        .and_then(|p| p.get("objectId"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());
    match (node_id, object_id) {
        (Some(id), _) => Ok(NodeRef::NodeId(id)),
        (None, Some(oid)) => Ok(NodeRef::ObjectId(oid.to_string())),
        _ => Err(CdpError {
            code: ERR_INVALID_PARAMS,
            message: "missing required parameter: nodeId or objectId".into(),
        }),
    }
}

/// Read the required non-empty string `selector` param (querySelector /
/// querySelectorAll). Missing/empty → -32602.
pub(crate) fn require_selector(params: &Option<Value>) -> Result<String, CdpError> {
    params
        .as_ref()
        .and_then(|p| p.get("selector"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .ok_or_else(|| CdpError {
            code: ERR_INVALID_PARAMS,
            message: "missing required parameter: selector".into(),
        })
}

/// JS prelude resolving a [`NodeRef`] into the element variable `el`:
/// canonical positional id, or the registry handle (which requires the
/// Runtime-minted registry to be present — the same dependency
/// getNodeForOwner documents; this face never mints a parallel registry).
fn node_ref_prelude(r#ref: &NodeRef) -> String {
    match r#ref {
        NodeRef::NodeId(id) => format!(
            r#"var el = __baoResolve({id});
if (!el) return JSON.stringify({{__bao_err: "No node with given id found"}});"#
        ),
        NodeRef::ObjectId(oid) => {
            let oid_json = serde_json::to_string(oid).unwrap_or_default();
            format!(
                r#"if (!window.__bao_cdp) return JSON.stringify({{__bao_err: "object registry not present: obtain objectIds via Runtime.evaluate(returnByValue:false) on this document"}});
var el = window.__bao_cdp.get({oid_json});
if (el === null || el === undefined) return JSON.stringify({{__bao_err: "No object with given id found"}});"#
            )
        }
    }
}

/// Build the `DOM.querySelector` body: the first match's canonical nodeId
/// (0 when nothing matches — the CDP convention this face keeps).
pub(crate) fn query_selector_body(selector: &str) -> String {
    let sel_json = serde_json::to_string(selector).unwrap_or_default();
    format!(
        r#"var el = document.querySelector({sel_json});
if (!el) return JSON.stringify({{ nodeId: 0 }});
var id = __baoCanonical(el);
if (id === null) return JSON.stringify({{__bao_err: "node is detached from the document or exceeds addressable node-id precision"}});
return JSON.stringify({{ nodeId: id, backendNodeId: id }});"#,
        sel_json = sel_json,
    )
}

/// Build the `DOM.querySelectorAll` body: every match's canonical nodeId, in
/// document order. Ids that exceed addressable precision fail the whole call
/// (never silently dropped).
pub(crate) fn query_selector_all_body(selector: &str) -> String {
    let sel_json = serde_json::to_string(selector).unwrap_or_default();
    format!(
        r#"var list = document.querySelectorAll({sel_json});
var ids = [];
for (var i = 0; i < list.length; i++) {{
    var id = __baoCanonical(list[i]);
    if (id === null) return JSON.stringify({{__bao_err: "matched node is detached or exceeds addressable node-id precision"}});
    ids.push(id);
}}
return JSON.stringify({{ nodeIds: ids }});"#,
        sel_json = sel_json,
    )
}

/// Build the `DOM.describeNode` body: one node's real data, children
/// expanded to `depth` (CDP default 1 = one level of children; negative =
/// the entire subtree).
pub(crate) fn describe_node_body(r#ref: &NodeRef, depth: i64) -> String {
    format!(
        r#"{}var depth = {depth};
if (__baoCanonical(el) === null) return JSON.stringify({{__bao_err: "node is detached from the document or exceeds addressable node-id precision"}});
function build(node, parentId, depthLeft) {{
    var id = __baoCanonical(node);
    if (id === null) throw "node-id-overflow";
    var e = __baoNodeFields(node, id);
    if (parentId !== 0) e.parentId = parentId;
    if (e.childNodeCount > 0 && (depthLeft < 0 || depthLeft > 0)) {{
        e.children = [];
        for (var i = 0; i < node.childNodes.length; i++) {{
            e.children.push(build(node.childNodes[i], id, depthLeft < 0 ? -1 : depthLeft - 1));
        }}
    }}
    return e;
}}
var node;
try {{ node = build(el, 0, depth); }}
catch (e) {{
    if (e === "node-id-overflow") return JSON.stringify({{__bao_err: "subtree exceeds addressable node-id precision (positional ids are safe through ~7 nested levels)"}});
    throw e;
}}
return JSON.stringify({{ node: node }});"#,
        node_ref_prelude(r#ref),
        depth = depth,
    )
}

/// Build the `DOM.getBoxModel` body: the element's real geometry — border
/// box from getBoundingClientRect, the other boxes derived from the computed
/// border/padding/margin widths. "auto" margins read as 0 (the honest
/// numeric reading of a non-px computed value on this face).
pub(crate) fn box_model_body(r#ref: &NodeRef) -> String {
    format!(
        r#"{}if (el.nodeType !== 1) return JSON.stringify({{__bao_err: "node is not an element (box model requires an Element node)"}});
var r = el.getBoundingClientRect();
var cs = window.getComputedStyle(el);
function px(name) {{ var v = parseFloat(cs.getPropertyValue(name)); return isNaN(v) ? 0 : v; }}
var bl = px("border-left-width"), bt = px("border-top-width");
var br = px("border-right-width"), bb = px("border-bottom-width");
var pl = px("padding-left"), pt = px("padding-top");
var pr = px("padding-right"), pb = px("padding-bottom");
var ml = px("margin-left"), mt = px("margin-top");
var mr = px("margin-right"), mb = px("margin-bottom");
function quad(x, y, w, h) {{ return [x, y, x + w, y, x + w, y + h, x, y + h]; }}
return JSON.stringify({{ model: {{
    width: r.width,
    height: r.height,
    content: quad(r.left + bl + pl, r.top + bt + pt, r.width - bl - br - pl - pr, r.height - bt - bb - pt - pb),
    padding: quad(r.left + bl, r.top + bt, r.width - bl - br, r.height - bt - bb),
    border: quad(r.left, r.top, r.width, r.height),
    margin: quad(r.left - ml, r.top - mt, r.width + ml + mr, r.height + mt + mb)
}} }});"#,
        node_ref_prelude(r#ref),
    )
}

/// Build the `DOM.resolveNode` body: mint a RemoteObject handle for the node
/// through the page-realm registry (Runtime-minted; absent registry is an
/// explicit error, never a parallel table).
pub(crate) fn resolve_node_body(r#ref: &NodeRef, object_group: Option<&str>) -> String {
    let group_json = serde_json::to_string(object_group.unwrap_or("")).unwrap_or_default();
    format!(
        r##"{}var oid = window.__bao_cdp.alloc(el, {group_json});
var cn = "Object";
try {{ if (el.constructor && el.constructor.name) cn = el.constructor.name; }} catch (e) {{}}
var desc = String(el.nodeName);
try {{ if (el.nodeType === 1) desc = el.nodeName.toLowerCase() + (el.id ? "#" + el.id : ""); }} catch (e) {{}}
return JSON.stringify({{ object: {{ type: "object", subtype: "node", className: cn, description: desc, objectId: oid }} }});"##,
        node_ref_prelude(r#ref),
        group_json = group_json,
    )
}

/// Build the `DOM.pushNodesByBackendIdsToFrontend` body: backendNodeId ≡
/// canonical nodeId on this face (documented identity), so the push is a
/// liveness check — ids that resolve are returned, unknown ids are omitted
/// (never fabricated).
pub(crate) fn push_nodes_body(backend_node_ids: &[i64]) -> String {
    let ids_json = serde_json::to_string(backend_node_ids).unwrap_or_default();
    format!(
        r#"var requested = {ids_json};
var out = [];
for (var i = 0; i < requested.length; i++) {{
    if (__baoResolve(requested[i])) out.push(requested[i]);
}}
return JSON.stringify({{ nodeIds: out }});"#,
        ids_json = ids_json,
    )
}

/// Read the required `edits` array param for `CSS.setStyleTexts`. Missing →
/// -32602.
pub(crate) fn require_edits(params: &Option<Value>) -> Result<Vec<Value>, CdpError> {
    params
        .as_ref()
        .and_then(|p| p.get("edits"))
        .and_then(|v| v.as_array())
        .cloned()
        .ok_or_else(|| CdpError {
            code: ERR_INVALID_PARAMS,
            message: "missing required parameter: edits".into(),
        })
}

/// Build the `CSS.setStyleTexts` body — the real write path.
///
/// Addressing on this face (documented deviation from Chrome's span model,
/// which requires stylesheet-text bookkeeping this face never minted):
/// `styleSheetId` is the ordinal of `document.styleSheets` (decimal string —
/// the same id getMatchedStylesForNode now reports), and `range.startLine`
/// is the rule index within that sheet. The edit replaces the rule's
/// declaration block via `style.cssText` — a real CSSOM write that restyles
/// the page — and the response carries the resulting declarations for
/// read-back comparison.
pub(crate) fn set_style_texts_body(edits: &[Value]) -> String {
    let edits_json = serde_json::to_string(edits).unwrap_or_default();
    format!(
        r#"var edits = {edits_json};
var outStyles = [];
for (var i = 0; i < edits.length; i++) {{
    var e = edits[i];
    var sheetIdx = parseInt(e.styleSheetId, 10);
    var sheet = (isNaN(sheetIdx)) ? null : document.styleSheets[sheetIdx];
    if (!sheet) return JSON.stringify({{__bao_err: "unknown styleSheetId (this face addresses document.styleSheets ordinals; discover ids via getMatchedStylesForNode)"}});
    var ruleIdx = (e.range && typeof e.range.startLine === "number") ? e.range.startLine : -1;
    var rules = null;
    try {{ rules = sheet.cssRules; }} catch (x) {{}}
    if (!rules || ruleIdx < 0 || ruleIdx >= rules.length) return JSON.stringify({{__bao_err: "edit range must carry the rule index in range.startLine (0-based, within the sheet's cssRules)"}});
    var rule = rules[ruleIdx];
    if (!rule || !rule.style) return JSON.stringify({{__bao_err: "rule at range.startLine is not a style rule (no declaration block to edit)"}});
    rule.style.cssText = e.text;
    var edited = __baoDecls(rule.style);
    edited.styleSheetId = e.styleSheetId;
    outStyles.push(edited);
}}
return JSON.stringify({{ styles: outStyles }});"#,
        edits_json = edits_json,
    )
}
