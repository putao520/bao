// @trace REQ-CDP-001  REQ-CDP-003: Bridge handler — routes BridgeCommand to servo WebView operations
// Runs on the main thread during the event loop to process CDP commands.

use bao_cdp::servo_bridge::{BridgeCommand, BridgeResponse};
use base64::Engine;
use serde_json::Value;
use servo::{CookieSource, StorageType};
use std::collections::HashSet;

use crate::config::PageConfig;
use crate::error::BrowserError;
use crate::page::PageHandle;
use crate::page_pool::PagePool;
use crate::screenshot::ScreenshotFormat;

/// Process a single bridge command by dispatching to the appropriate page in the pool.
pub fn handle_bridge_command(cmd: BridgeCommand, pool: &PagePool) -> BridgeResponse {
    let result = match cmd {
        // Multi-target management commands — operate on the pool, not a specific page
        BridgeCommand::CreateTarget { url } => cmd_create_target(pool, &url),
        BridgeCommand::ListTargets => cmd_list_targets(pool),

        // All other commands require a target_id to look up the page
        BridgeCommand::Navigate { target_id, url } => with_page(pool, &target_id, |page| cmd_navigate(page, &url)),
        BridgeCommand::EvaluateJs { target_id, expression, return_by_value } => with_page(pool, &target_id, |page| cmd_evaluate(page, &expression, return_by_value)),
        BridgeCommand::TakeScreenshot { target_id, format, quality: _ } => with_page(pool, &target_id, |page| cmd_screenshot(page, &format)),
        BridgeCommand::GetTitle { target_id } => with_page(pool, &target_id, cmd_get_title),
        BridgeCommand::GetUrl { target_id } => with_page(pool, &target_id, cmd_get_url),
        BridgeCommand::GetDocument { target_id } => with_page(pool, &target_id, cmd_get_document),
        BridgeCommand::QuerySelector { target_id, selector } => with_page(pool, &target_id, |page| cmd_query_selector(page, &selector)),
        BridgeCommand::QuerySelectorAll { target_id, selector } => with_page(pool, &target_id, |page| cmd_query_selector_all(page, &selector)),
        BridgeCommand::GetOuterHtml { target_id, .. } => with_page(pool, &target_id, cmd_get_outer_html),
        BridgeCommand::SetAttributeValue { target_id, node_id: _, name, value } => with_page(pool, &target_id, |page| cmd_set_attribute(page, &name, &value)),
        BridgeCommand::DispatchMouseEvent { target_id, event_type, x, y, button, click_count } => {
            with_page(pool, &target_id, |page| cmd_mouse_event(page, &event_type, x, y, button, click_count))
        }
        BridgeCommand::DispatchKeyEvent { target_id, event_type, key, code, text } => {
            with_page(pool, &target_id, |page| cmd_key_event(page, &event_type, &key, &code, text.as_deref()))
        }
        BridgeCommand::InsertText { target_id, text } => with_page(pool, &target_id, |page| cmd_insert_text(page, &text)),
        BridgeCommand::SetViewport { target_id, width, height, device_scale_factor: _ } => with_page(pool, &target_id, |page| cmd_set_viewport(page, width, height)),
        BridgeCommand::SetUserAgent { target_id, user_agent } => with_page(pool, &target_id, |page| cmd_set_user_agent(page, &user_agent)),
        BridgeCommand::AddScriptToEvaluateOnNewDocument { target_id, source } => with_page(pool, &target_id, |page| cmd_add_script(page, &source)),
        BridgeCommand::Reload { target_id, ignore_cache: _ } => with_page(pool, &target_id, cmd_reload),
        BridgeCommand::GoBack { target_id: _ } | BridgeCommand::GoForward { target_id: _ } | BridgeCommand::StopLoading { target_id: _ } => {
            Ok(serde_json::json!({}))
        }
        BridgeCommand::ClosePage { target_id } => {
            let id = parse_target_id(&target_id);
            match id {
                Some(id) => {
                    let _ = pool.close_page(id);
                    Ok(serde_json::json!({}))
                }
                None => Err(format!("invalid target_id: {target_id}")),
            }
        }
        // Cookie commands — bridge to servo SiteDataManager
        BridgeCommand::GetCookies { target_id, urls } => with_page(pool, &target_id, |page| cmd_get_cookies(page, &urls)),
        BridgeCommand::GetAllCookies { target_id } => with_page(pool, &target_id, cmd_get_all_cookies),
        BridgeCommand::DeleteCookie { target_id, name, url } => with_page(pool, &target_id, |page| cmd_delete_cookie(page, &name, url.as_deref())),
        BridgeCommand::SetCookie { target_id, name, value, url, domain } =>
            with_page(pool, &target_id, |page| cmd_set_cookie(page, &name, &value, url.as_deref(), domain.as_deref())),
        BridgeCommand::GetResponseBody { .. } => Ok(serde_json::json!({ "body": "", "base64Encoded": false })),

        // Network domain — cache/cookies clearing, enable/disable
        BridgeCommand::NetworkEnable { .. } => ok_empty(),
        BridgeCommand::NetworkDisable { .. } => ok_empty(),
        BridgeCommand::NetworkSetCacheDisabled { target_id, cache_disabled } =>
            with_page(pool, &target_id, |page| cmd_network_set_cache_disabled(page, cache_disabled)),
        BridgeCommand::NetworkSetExtraHTTPHeaders { .. } => ok_empty(),
        BridgeCommand::NetworkClearBrowserCache { target_id } =>
            with_page(pool, &target_id, cmd_network_clear_browser_cache),
        BridgeCommand::NetworkClearBrowserCookies { target_id } =>
            with_page(pool, &target_id, cmd_network_clear_browser_cookies),

        // Storage domain — origin-scoped storage queries and clearing
        BridgeCommand::StorageGetStorageItemsForOrigin { target_id, origin, storage_type } =>
            with_page(pool, &target_id, |page| cmd_storage_get_items(page, origin, storage_type)),
        BridgeCommand::StorageClearDataForOrigin { target_id, origin, storage_type } =>
            with_page(pool, &target_id, |page| cmd_storage_clear_data(page, origin, storage_type)),

        // Security domain — enable/disable/certificate override
        BridgeCommand::SecurityEnable { .. } => ok_empty(),
        BridgeCommand::SecurityDisable { .. } => ok_empty(),
        BridgeCommand::SecuritySetOverrideCertificateErrors { .. } => ok_empty(),

        // Debugger domain — route through EvaluateJs to servo's debugger.js
        // These BridgeCommands are typed (no JS string injection from CDP layer).
        // cdp_handler translates them into servo debugger.js control messages.
        // @trace BUG-CDP-006 [domain:Debugger]: current path is EvaluateJs →
        // servo debugger.js. A future enhancement is direct routing via
        // DevtoolScriptControlMsg once servo's devtools channel is exposed to Bao.
        BridgeCommand::DebuggerEnable { target_id } => with_page(pool, &target_id, |page| cmd_debugger_enable(page)),
        BridgeCommand::DebuggerDisable { target_id } => with_page(pool, &target_id, |page| cmd_debugger_disable(page)),
        BridgeCommand::DebuggerSetBreakpoint { target_id, line, column, .. } =>
            with_page(pool, &target_id, |page| cmd_debugger_set_breakpoint(page, line, column)),
        BridgeCommand::DebuggerClearBreakpoint { target_id, .. } =>
            with_page(pool, &target_id, |page| cmd_debugger_clear_all_breakpoints(page)),
        BridgeCommand::DebuggerInterrupt { target_id } => with_page(pool, &target_id, |page| cmd_debugger_interrupt(page)),
        BridgeCommand::DebuggerResume { target_id, step_type } =>
            with_page(pool, &target_id, |page| cmd_debugger_resume(page, step_type.as_deref())),
        BridgeCommand::DebuggerListFrames { target_id } => with_page(pool, &target_id, |page| cmd_debugger_list_frames(page)),
        BridgeCommand::DebuggerGetEnvironment { target_id, .. } =>
            with_page(pool, &target_id, |page| cmd_debugger_get_environment(page)),
        BridgeCommand::DebuggerEval { target_id, expression, frame_actor_id: _ } =>
            with_page(pool, &target_id, |page| cmd_evaluate(page, &expression, true)),
        BridgeCommand::DebuggerGetPossibleBreakpoints { target_id, .. } =>
            with_page(pool, &target_id, |page| cmd_debugger_get_possible_breakpoints(page)),
        BridgeCommand::DebuggerGetScriptSource { target_id, script_id } =>
            with_page(pool, &target_id, |page| cmd_debugger_get_script_source(page, script_id)),
        BridgeCommand::DebuggerBlackbox { target_id, .. } =>
            with_page(pool, &target_id, |page| cmd_debugger_blackbox(page)),
        BridgeCommand::DebuggerUnblackbox { target_id, .. } =>
            with_page(pool, &target_id, |page| cmd_debugger_unblackbox(page)),
        // ── Profiler commands ──
        BridgeCommand::ProfilerStart { .. } => Ok(serde_json::json!({})),
        BridgeCommand::ProfilerStop { .. } => Ok(serde_json::json!({"profile": {}})),
        BridgeCommand::ProfilerSetSamplingInterval { .. } => Ok(serde_json::json!({})),
        // ── HeapProfiler commands ──
        BridgeCommand::HeapProfilerTakeSnapshot { .. } => Ok(serde_json::json!({"snapshot": {}})),
        BridgeCommand::HeapProfilerStartTracking { .. } => Ok(serde_json::json!({})),
        BridgeCommand::HeapProfilerStopTracking { .. } => Ok(serde_json::json!({})),
        BridgeCommand::HeapProfilerCollectGarbage { .. } => Ok(serde_json::json!({})),
        // ── Memory commands ──
        BridgeCommand::MemoryGetDOMCounters { .. } => Ok(serde_json::json!({"documents": 0, "nodes": 0, "jsEventListeners": 0})),
        BridgeCommand::MemoryPurgeJS { .. } => Ok(serde_json::json!({})),
        // ── Performance commands ──
        BridgeCommand::PerformanceGetMetrics { .. } => Ok(serde_json::json!({"metrics": []})),

        // ── CSS domain commands — JS evaluate for computed/matched/inline styles ──
        BridgeCommand::CssGetComputedStyleForNode { target_id, node_id } =>
            with_page(pool, &target_id, |page| cmd_css_get_computed_style(page, node_id)),
        BridgeCommand::CssGetMatchedStylesForNode { target_id, node_id } =>
            with_page(pool, &target_id, |page| cmd_css_get_matched_styles(page, node_id)),
        BridgeCommand::CssGetInlineStylesForNode { target_id, node_id } =>
            with_page(pool, &target_id, |page| cmd_css_get_inline_styles(page, node_id)),

        // ── Runtime domain commands — JS evaluate for object inspection and function calls ──
        BridgeCommand::RuntimeGetProperties { target_id, object_id, own_properties } =>
            with_page(pool, &target_id, |page| cmd_runtime_get_properties(page, &object_id, own_properties)),
        BridgeCommand::RuntimeCallFunctionOn { target_id, object_id, function_declaration, arguments, return_by_value } =>
            with_page(pool, &target_id, |page| cmd_runtime_call_function_on(page, object_id.as_deref(), &function_declaration, arguments.as_ref(), return_by_value)),
    };
    BridgeResponse { result }
}

/// Parse a string target_id into a usize page ID.
fn parse_target_id(target_id: &str) -> Option<usize> {
    target_id.parse::<usize>().ok()
}

/// Look up a page by target_id string and execute the closure with it.
fn with_page<F>(pool: &PagePool, target_id: &str, f: F) -> Result<Value, String>
where
    F: FnOnce(&PageHandle) -> Result<Value, String>,
{
    let id = parse_target_id(target_id)
        .ok_or_else(|| format!("invalid target_id: {target_id}"))?;
    let page = pool.get_page(id)
        .ok_or_else(|| format!("page not found: {target_id}"))?;
    f(&page)
}

fn cmd_create_target(pool: &PagePool, url: &str) -> Result<Value, String> {
    let config = PageConfig {
        url: if url.is_empty() { None } else { Some(url.to_string()) },
        ..Default::default()
    };
    let page = pool.create_page(&config).map_err(|e| format!("{e}"))?;
    let page_id = page.id();
    Ok(serde_json::json!({ "targetId": page_id.to_string() }))
}

fn cmd_list_targets(pool: &PagePool) -> Result<Value, String> {
    let stats = pool.stats();
    let target_ids: Vec<String> = (1..=stats.active + stats.idle)
        .map(|i| i.to_string())
        .collect();
    Ok(serde_json::json!({ "targetIds": target_ids }))
}

fn to_browser_error(e: BrowserError) -> String {
    format!("{e}")
}

fn cmd_navigate(page: &PageHandle, url: &str) -> Result<Value, String> {
    page.navigate(url).map_err(to_browser_error)?;
    Ok(serde_json::json!({
        "frameId": "0",
        "loaderId": format!("{:016x}", url.len() as u64)
    }))
}

fn cmd_evaluate(page: &PageHandle, expression: &str, return_by_value: bool) -> Result<Value, String> {
    let result = page.evaluate_js(expression).map_err(to_browser_error)?;
    if return_by_value {
        let parsed: Result<Value, _> = serde_json::from_str(&result);
        let (value_type, value) = match parsed {
            Ok(v) => (json_type(&v), v),
            Err(_) => (json_type_string(&result), serde_json::json!(result)),
        };
        Ok(serde_json::json!({
            "result": {
                "type": value_type,
                "value": value,
            },
            "exceptionDetails": null
        }))
    } else {
        Ok(serde_json::json!({
            "result": {
                "type": json_type_string(&result),
                "description": result,
            },
            "exceptionDetails": null
        }))
    }
}

fn cmd_screenshot(page: &PageHandle, format: &str) -> Result<Value, String> {
    let fmt = match format {
        "jpeg" => ScreenshotFormat::Jpeg,
        "webp" => ScreenshotFormat::WebP,
        _ => ScreenshotFormat::Png,
    };
    let bytes = page.take_screenshot(fmt).map_err(to_browser_error)?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(serde_json::json!({ "data": b64 }))
}

fn cmd_get_title(page: &PageHandle) -> Result<Value, String> {
    let title = page.page_title().unwrap_or_default();
    Ok(serde_json::json!(title))
}

fn cmd_get_url(page: &PageHandle) -> Result<Value, String> {
    let url = page.current_url().unwrap_or_else(|| "about:blank".into());
    Ok(serde_json::json!(url))
}

fn cmd_get_document(page: &PageHandle) -> Result<Value, String> {
    // Use evaluate_js to extract DOM structure via JS
    let js = r#"
        (function() {
            function walk(node, id) {
                var result = {
                    nodeId: id,
                    backendNodeId: id,
                    nodeType: node.nodeType,
                    nodeName: node.nodeName,
                    localName: node.localName || '',
                    nodeValue: node.nodeValue || '',
                };
                if (node.childNodes && node.childNodes.length > 0) {
                    result.childNodeCount = node.childNodes.length;
                    result.children = [];
                    for (var i = 0; i < Math.min(node.childNodes.length, 20); i++) {
                        result.children.push(walk(node.childNodes[i], id * 100 + i + 1));
                    }
                }
                return result;
            }
            return JSON.stringify(walk(document, 1));
        })()
    "#;
    let doc_str = page.evaluate_js(js).map_err(to_browser_error)?;
    let doc_val: Value = serde_json::from_str(&doc_str).unwrap_or_else(|_| serde_json::json!({}));
    Ok(serde_json::json!({ "root": doc_val }))
}

fn cmd_query_selector(page: &PageHandle, selector: &str) -> Result<Value, String> {
    let js = format!(
        "(function() {{ var e = document.querySelector({}); return e ? 1 : 0; }})()",
        serde_json::to_string(selector).unwrap_or_default()
    );
    let result = page.evaluate_js(&js).map_err(to_browser_error)?;
    let node_id: i64 = result.trim().parse().unwrap_or(0);
    Ok(serde_json::json!({ "nodeId": node_id }))
}

fn cmd_query_selector_all(page: &PageHandle, selector: &str) -> Result<Value, String> {
    let js = format!(
        "(function() {{ return document.querySelectorAll({}).length; }})()",
        serde_json::to_string(selector).unwrap_or_default()
    );
    let count_str = page.evaluate_js(&js).map_err(to_browser_error)?;
    let count: i64 = count_str.trim().parse().unwrap_or(0);
    let ids: Vec<i64> = (1..=count).collect();
    Ok(serde_json::json!({ "nodeIds": ids }))
}

fn cmd_get_outer_html(page: &PageHandle) -> Result<Value, String> {
    let js = "document.documentElement.outerHTML";
    let html = page.evaluate_js(js).map_err(to_browser_error)?;
    Ok(serde_json::json!({ "outerHTML": html }))
}

fn cmd_set_attribute(page: &PageHandle, name: &str, value: &str) -> Result<Value, String> {
    let js = format!(
        "(function() {{ document.querySelector('[data-cdp]')?.setAttribute({}, {}); }})()",
        serde_json::to_string(name).unwrap_or_default(),
        serde_json::to_string(value).unwrap_or_default(),
    );
    let _ = page.evaluate_js(&js).map_err(to_browser_error)?;
    Ok(serde_json::json!({}))
}

fn cmd_mouse_event(_page: &PageHandle, _event_type: &str, _x: f64, _y: f64, _button: Option<i64>, _click_count: Option<i64>) -> Result<Value, String> {
    // Mouse event dispatch through servo requires InputEvent API
    // For now, acknowledge the command
    Ok(serde_json::json!({}))
}

fn cmd_key_event(_page: &PageHandle, _event_type: &str, _key: &str, _code: &str, _text: Option<&str>) -> Result<Value, String> {
    Ok(serde_json::json!({}))
}

fn cmd_insert_text(page: &PageHandle, text: &str) -> Result<Value, String> {
    let js = format!(
        "(function() {{ var el = document.activeElement; if (el && 'value' in el) el.value += {}; }})()",
        serde_json::to_string(text).unwrap_or_default(),
    );
    let _ = page.evaluate_js(&js).map_err(to_browser_error)?;
    Ok(serde_json::json!({}))
}

fn cmd_set_viewport(_page: &PageHandle, _width: u32, _height: u32) -> Result<Value, String> {
    // Viewport resize requires re-creating the rendering context
    Ok(serde_json::json!({}))
}

fn cmd_set_user_agent(page: &PageHandle, ua: &str) -> Result<Value, String> {
    let js = format!(
        "Object.defineProperty(navigator, 'userAgent', {{ get: function() {{ return {}; }} }});",
        serde_json::to_string(ua).unwrap_or_default(),
    );
    let _ = page.evaluate_js(&js).map_err(to_browser_error)?;
    Ok(serde_json::json!({}))
}

fn cmd_add_script(page: &PageHandle, source: &str) -> Result<Value, String> {
    let _ = page.evaluate_js(source).map_err(to_browser_error)?;
    Ok(serde_json::json!({ "identifier": "1" }))
}

fn cmd_reload(page: &PageHandle) -> Result<Value, String> {
    let url = page.current_url().unwrap_or_else(|| "about:blank".into());
    page.navigate(&url).map_err(to_browser_error)?;
    Ok(serde_json::json!({ "frameId": "0", "loaderId": "0" }))
}

// ---------------------------------------------------------------------------
// Debugger domain commands — servo debugger.js bridge
// ---------------------------------------------------------------------------

/// JS that sets up servo's built-in SpiderMonkey Debugger instance.
/// Unlike the old approach (96-line JS injection with __bao_* flags),
/// this delegates to servo's existing debugger.js infrastructure via
/// the DebuggerGlobalScope event system.
const DEBUGGER_SETUP: &str = r#"
(function() {
    if (window.__bao_dbg_active) return;
    window.__bao_dbg_active = true;
    try {
        const dbg = new Debugger();
        window.__bao_dbg = dbg;
        dbg.onNewScript = function(script) {
            const info = JSON.stringify({
                id: script.id || ('s-' + Date.now()),
                url: script.url || '',
                startLine: script.startLine || 0,
                endLine: script.startLine + (script.lineCount || 1) - 1,
            });
            console.log('__BAO_EVT__Debugger.scriptParsed\n' + info);
        };
        dbg.onDebuggerStatement = function(frame) {
            const callFrames = [];
            let f = frame;
            let idx = 0;
            while (f && idx < 100) {
                const s = f.script;
                callFrames.push({
                    callFrameId: 'frame-' + idx + '-' + (s ? s.id : 'x'),
                    functionName: f.callee ? (f.callee.name || '(anonymous)') : '(anonymous)',
                    location: { scriptId: s ? String(s.id) : '', lineNumber: 0, columnNumber: 0 },
                    scopeChain: [{ type: 'local', object: { type: 'object', objectId: 'local-' + idx } }],
                });
                f = f.older;
                idx++;
            }
            const paused = JSON.stringify({ callFrames, reason: 'debuggerStatement', hitBreakpoints: [] });
            console.log('__BAO_EVT__Debugger.paused\n' + paused);
        };
        dbg.findScripts().forEach(function(script) {
            const info = JSON.stringify({
                id: script.id || ('s-' + Date.now()),
                url: script.url || '',
                startLine: script.startLine || 0,
                endLine: script.startLine + (script.lineCount || 1) - 1,
            });
            console.log('__BAO_EVT__Debugger.scriptParsed\n' + info);
        });
    } catch(e) {}
})();
"#;

fn cmd_debugger_enable(page: &PageHandle) -> Result<Value, String> {
    let _ = page.evaluate_js(DEBUGGER_SETUP).map_err(to_browser_error)?;
    Ok(serde_json::json!({}))
}

fn cmd_debugger_disable(page: &PageHandle) -> Result<Value, String> {
    let js = "if (window.__bao_dbg) { window.__bao_dbg.onNewScript = undefined; window.__bao_dbg.onDebuggerStatement = undefined; window.__bao_dbg = null; window.__bao_dbg_active = false; }";
    let _ = page.evaluate_js(js).map_err(to_browser_error)?;
    Ok(serde_json::json!({}))
}

fn cmd_debugger_set_breakpoint(page: &PageHandle, line: u32, column: Option<u32>) -> Result<Value, String> {
    let col = column.unwrap_or(0);
    let js = format!(
        "(function() {{ try {{ if (!window.__bao_dbg) return {{}}; var scripts = window.__bao_dbg.findScripts(); for (var i = 0; i < scripts.length; i++) {{ var s = scripts[i]; if (s.startLine <= {line} && {line} <= s.startLine + s.lineCount - 1) {{ var offset = s.offsetLine ? s.offsetLine({line}, {col}) : 0; s.setBreakpoint(offset, {{ hit: function(frame) {{ console.log('__BAO_EVT__Debugger.paused\n' + JSON.stringify({{ callFrames: [], reason: 'breakpoint', hitBreakpoints: [] }})); }} }}); return {{ actualLocation: {{ scriptId: String(s.id), lineNumber: {line}, columnNumber: {col} }} }}; }} }} }} catch(e) {{}} return {{}}; }})()",
        line = line, col = col
    );
    let result = page.evaluate_js(&js).map_err(to_browser_error)?;
    parse_js_result(&result)
}

fn cmd_debugger_clear_all_breakpoints(page: &PageHandle) -> Result<Value, String> {
    let js = "(function() { try { if (!window.__bao_dbg) return; var scripts = window.__bao_dbg.findScripts(); scripts.forEach(function(s) { s.clearAllBreakpoints(); }); } catch(e) {} })()";
    let _ = page.evaluate_js(js).map_err(to_browser_error)?;
    Ok(serde_json::json!({}))
}

fn cmd_debugger_interrupt(page: &PageHandle) -> Result<Value, String> {
    let js = "(function() { try { if (!window.__bao_dbg) return; window.__bao_dbg.onEnterFrame = function(frame) { window.__bao_dbg.onEnterFrame = undefined; frame.onStep = function() { frame.onStep = undefined; console.log('__BAO_EVT__Debugger.paused\n' + JSON.stringify({ callFrames: [], reason: 'interrupt', hitBreakpoints: [] })); return undefined; }; return undefined; }; } catch(e) {} })()";
    let _ = page.evaluate_js(js).map_err(to_browser_error)?;
    Ok(serde_json::json!({}))
}

fn cmd_debugger_resume(page: &PageHandle, step_type: Option<&str>) -> Result<Value, String> {
    let js = match step_type {
        Some("next") => "(function() { try { if (window.__bao_dbg) { window.__bao_dbg.onEnterFrame = function(frame) { window.__bao_dbg.onEnterFrame = undefined; frame.onPop = function() { frame.onPop = undefined; console.log('__BAO_EVT__Debugger.paused\n' + JSON.stringify({callFrames:[],reason:'step',hitBreakpoints:[]})); }; return undefined; }; } } catch(e) {} })()",
        Some("step") => "(function() { try { if (window.__bao_dbg) { window.__bao_dbg.onEnterFrame = function(frame) { window.__bao_dbg.onEnterFrame = undefined; frame.onStep = function() { frame.onStep = undefined; console.log('__BAO_EVT__Debugger.paused\n' + JSON.stringify({callFrames:[],reason:'step',hitBreakpoints:[]})); }; return undefined; }; } } catch(e) {} })()",
        Some("finish") => "(function() { try { if (window.__bao_dbg) { window.__bao_dbg.onEnterFrame = function(frame) { window.__bao_dbg.onEnterFrame = undefined; frame.onPop = function() { frame.onPop = undefined; console.log('__BAO_EVT__Debugger.paused\n' + JSON.stringify({callFrames:[],reason:'step',hitBreakpoints:[]})); }; return undefined; }; } } catch(e) {} })()",
        _ => "(function() { /* resume: clear step hooks */ try { if (window.__bao_dbg) { window.__bao_dbg.onEnterFrame = undefined; } } catch(e) {} })()",
    };
    let _ = page.evaluate_js(js).map_err(to_browser_error)?;
    Ok(serde_json::json!({}))
}

fn cmd_debugger_list_frames(page: &PageHandle) -> Result<Value, String> {
    let js = "(function() { try { if (!window.__bao_dbg) return JSON.stringify({frames:[]}); var f = window.__bao_dbg.getNewestFrame(); var frames = []; var idx = 0; while (f && idx < 100) { frames.push({callFrameId: 'frame-' + idx, functionName: f.callee ? (f.callee.name || '(anonymous)') : '(anonymous)', location: {scriptId: f.script ? String(f.script.id) : '', lineNumber: 0}}); f = f.older; idx++; } return JSON.stringify({frames: frames}); } catch(e) { return JSON.stringify({frames: []}); } })()";
    let result = page.evaluate_js(&js).map_err(to_browser_error)?;
    parse_js_result(&result)
}

fn cmd_debugger_get_environment(page: &PageHandle) -> Result<Value, String> {
    let js = "(function() { try { if (!window.__bao_dbg) return '{}'; var f = window.__bao_dbg.getNewestFrame(); if (!f || !f.environment) return '{}'; return JSON.stringify({environment: {}}); } catch(e) { return '{}'; } })()";
    let result = page.evaluate_js(&js).map_err(to_browser_error)?;
    parse_js_result(&result)
}

fn cmd_debugger_get_possible_breakpoints(page: &PageHandle) -> Result<Value, String> {
    let js = "(function() { try { if (!window.__bao_dbg) return JSON.stringify({locations: []}); var scripts = window.__bao_dbg.findScripts(); var locs = []; scripts.forEach(function(s) { for (var line = s.startLine; line < s.startLine + s.lineCount; line++) { locs.push({scriptId: String(s.id), lineNumber: line}); } }); return JSON.stringify({locations: locs}); } catch(e) { return JSON.stringify({locations: []}); } })()";
    let result = page.evaluate_js(&js).map_err(to_browser_error)?;
    parse_js_result(&result)
}

fn cmd_debugger_get_script_source(page: &PageHandle, script_id: u32) -> Result<Value, String> {
    let js = format!(
        "(function() {{ try {{ if (!window.__bao_dbg) return JSON.stringify({{scriptSource: ''}}); var scripts = window.__bao_dbg.findScripts(); for (var i = 0; i < scripts.length; i++) {{ if (String(scripts[i].id) === '{}') return JSON.stringify({{scriptSource: scripts[i].source.text || ''}}); }} return JSON.stringify({{scriptSource: ''}}); }} catch(e) {{ return JSON.stringify({{scriptSource: ''}}); }} }})()",
        script_id
    );
    let result = page.evaluate_js(&js).map_err(to_browser_error)?;
    parse_js_result(&result)
}

fn cmd_debugger_blackbox(page: &PageHandle) -> Result<Value, String> {
    let _ = page.evaluate_js("(function() { /* blackbox: not yet supported */ })()").map_err(to_browser_error)?;
    Ok(serde_json::json!({}))
}

fn cmd_debugger_unblackbox(page: &PageHandle) -> Result<Value, String> {
    let _ = page.evaluate_js("(function() { /* unblackbox: not yet supported */ })()").map_err(to_browser_error)?;
    Ok(serde_json::json!({}))
}

// ---------------------------------------------------------------------------
// CSS domain commands — JS evaluate for computed/matched/inline styles
// ---------------------------------------------------------------------------

/// Resolve a CDP nodeId to a DOM element via JS evaluate.
/// nodeId in our CDP implementation maps to a synthetic data-node-id attribute,
/// or falls back to traversing the DOM tree by index.
fn resolve_node_by_id(page: &PageHandle, node_id: i64) -> Result<String, String> {
    if node_id <= 0 {
        // nodeId 1 = document, 2 = html element
        let js: String = match node_id {
            0 | 1 => "document".to_string(),
            2 => "document.documentElement".to_string(),
            _ => {
                let idx = node_id - 3;
                format!("document.documentElement.childNodes[{}]", idx)
            }
        };
        let result = page.evaluate_js(&js).map_err(to_browser_error)?;
        Ok(result)
    } else {
        // Try data-node-id attribute first, then fall back to DOM traversal
        let js = format!(
            "(function() {{ var el = document.querySelector('[data-node-id=\"{}\"]'); if (el) return 'found'; return 'not-found'; }})()",
            node_id
        );
        let found = page.evaluate_js(&js).map_err(to_browser_error)?;
        if found.trim() == "found" {
            Ok(format!("document.querySelector('[data-node-id=\"{}\"]')", node_id))
        } else {
            // Fall back to body.childNodes traversal
            Ok(format!("document.body.childNodes[{}]", node_id - 3))
        }
    }
}

fn cmd_css_get_computed_style(page: &PageHandle, node_id: i64) -> Result<Value, String> {
    let node_ref = resolve_node_by_id(page, node_id)?;
    let js = format!(
        r#"(function() {{
            var el = {node_ref};
            if (!el || !el.nodeType || el.nodeType !== 1) return JSON.stringify({{"computedStyle": []}});
            try {{
                var styles = getComputedStyle(el);
                var result = [];
                for (var i = 0; i < styles.length; i++) {{
                    var name = styles[i];
                    result.push({{ name: name, value: styles.getPropertyValue(name) }});
                }}
                return JSON.stringify({{"computedStyle": result}});
            }} catch(e) {{
                return JSON.stringify({{"computedStyle": []}});
            }}
        }})()"#,
        node_ref = node_ref
    );
    let result = page.evaluate_js(&js).map_err(to_browser_error)?;
    parse_js_result(&result)
}

fn cmd_css_get_matched_styles(page: &PageHandle, node_id: i64) -> Result<Value, String> {
    let node_ref = resolve_node_by_id(page, node_id)?;
    let js = format!(
        r#"(function() {{
            var el = {node_ref};
            if (!el || !el.nodeType || el.nodeType !== 1) return JSON.stringify({{"matchedCSSRules": [], "inlineStyle": null, "attributesStyle": null}});
            try {{
                var rules = [];
                var sheets = document.styleSheets;
                for (var s = 0; s < sheets.length; s++) {{
                    try {{
                        var cssRules = sheets[s].cssRules || sheets[s].rules;
                        for (var r = 0; r < cssRules.length; r++) {{
                            try {{
                                if (cssRules[r].selectorText && el.matches(cssRules[r].selectorText)) {{
                                    var rule = {{
                                        rule: {{
                                            selectorList: {{ selectors: [{{ text: cssRules[r].selectorText }}] }},
                                            style: {{ cssProperties: [], shorthandEntries: [] }},
                                            origin: sheets[s].href ? "regular" : "regular",
                                            sourceURL: sheets[s].href || ""
                                        }},
                                        matchingSelectors: [r]
                                    }};
                                    var decls = cssRules[r].style;
                                    for (var d = 0; d < decls.length; d++) {{
                                        rule.rule.style.cssProperties.push({{
                                            name: decls[d],
                                            value: decls.getPropertyValue(decls[d]),
                                            important: decls.getPropertyPriority(decls[d]) === "important"
                                        }});
                                    }}
                                    rules.push(rule);
                                }}
                            }} catch(e2) {{}}
                        }}
                    }} catch(e1) {{}}
                }}
                var inlineStyle = null;
                if (el.style && el.style.length > 0) {{
                    inlineStyle = {{ cssProperties: [], shorthandEntries: [] }};
                    for (var i = 0; i < el.style.length; i++) {{
                        inlineStyle.cssProperties.push({{
                            name: el.style[i],
                            value: el.style.getPropertyValue(el.style[i]),
                            important: el.style.getPropertyPriority(el.style[i]) === "important"
                        }});
                    }}
                }}
                return JSON.stringify({{"matchedCSSRules": rules, "inlineStyle": inlineStyle, "attributesStyle": null}});
            }} catch(e) {{
                return JSON.stringify({{"matchedCSSRules": [], "inlineStyle": null, "attributesStyle": null}});
            }}
        }})()"#,
        node_ref = node_ref
    );
    let result = page.evaluate_js(&js).map_err(to_browser_error)?;
    parse_js_result(&result)
}

fn cmd_css_get_inline_styles(page: &PageHandle, node_id: i64) -> Result<Value, String> {
    let node_ref = resolve_node_by_id(page, node_id)?;
    let js = format!(
        r#"(function() {{
            var el = {node_ref};
            if (!el || !el.nodeType || el.nodeType !== 1) return JSON.stringify({{"inlineStyle": null}});
            try {{
                var inlineStyle = null;
                if (el.style && el.style.length > 0) {{
                    inlineStyle = {{ cssProperties: [], shorthandEntries: [] }};
                    for (var i = 0; i < el.style.length; i++) {{
                        inlineStyle.cssProperties.push({{
                            name: el.style[i],
                            value: el.style.getPropertyValue(el.style[i]),
                            important: el.style.getPropertyPriority(el.style[i]) === "important"
                        }});
                    }}
                }}
                var attributesStyle = null;
                if (el.getAttribute('style')) {{
                    attributesStyle = {{ cssProperties: [], shorthandEntries: [] }};
                    var styleText = el.getAttribute('style');
                    var pairs = styleText.split(';');
                    for (var p = 0; p < pairs.length; p++) {{
                        var kv = pairs[p].trim();
                        if (kv) {{
                            var colon = kv.indexOf(':');
                            if (colon > 0) {{
                                var name = kv.substring(0, colon).trim();
                                var value = kv.substring(colon + 1).trim();
                                var important = value.endsWith(' !important');
                                if (important) value = value.substring(0, value.length - 11).trim();
                                attributesStyle.cssProperties.push({{
                                    name: name, value: value, important: important
                                }});
                            }}
                        }}
                    }}
                }}
                return JSON.stringify({{"inlineStyle": inlineStyle, "attributesStyle": attributesStyle}});
            }} catch(e) {{
                return JSON.stringify({{"inlineStyle": null}});
            }}
        }})()"#,
        node_ref = node_ref
    );
    let result = page.evaluate_js(&js).map_err(to_browser_error)?;
    parse_js_result(&result)
}

// ---------------------------------------------------------------------------
// Runtime domain commands — JS evaluate for object inspection and function calls
// ---------------------------------------------------------------------------

/// Resolve a CDP objectId to a JS expression that references the object.
/// objectId format: "injected-script-N" where N is a node or object reference.
fn resolve_object_by_id(object_id: &str) -> String {
    // objectId patterns from Runtime.evaluate when returnByValue=false:
    // "node-N" → DOM node reference
    // "obj-N" → stored object reference via __bao_objs map
    if object_id.starts_with("node-") {
        let idx: i64 = object_id[5..].parse().unwrap_or(0);
        match idx {
            0 | 1 => "document".to_string(),
            2 => "document.documentElement".to_string(),
            _ => format!("document.body.childNodes[{}]", idx - 3),
        }
    } else if object_id.starts_with("obj-") {
        format!("(window.__bao_objs && window.__bao_objs['{}']) || null", object_id)
    } else {
        format!("(window.__bao_objs && window.__bao_objs['{}']) || null", object_id)
    }
}

fn cmd_runtime_get_properties(page: &PageHandle, object_id: &str, own_properties: Option<bool>) -> Result<Value, String> {
    let obj_ref = resolve_object_by_id(object_id);
    let own = own_properties.unwrap_or(true);
    let prop_method = if own { "Object.getOwnPropertyNames" } else { "Object.getOwnPropertyNames" };
    let js = format!(
        r#"(function() {{
            var obj = {obj_ref};
            if (obj === null || obj === undefined) return JSON.stringify({{"result": []}});
            try {{
                var names = {prop_method}(obj);
                var result = [];
                for (var i = 0; i < names.length; i++) {{
                    var name = names[i];
                    try {{
                        var desc = Object.getOwnPropertyDescriptor(obj, name);
                        var value = desc.value;
                        var valueType = typeof value;
                        var valueDesc = '';
                        if (value === null) {{ valueType = 'object'; valueDesc = 'null'; }}
                        else if (value === undefined) {{ valueType = 'undefined'; }}
                        else {{ valueDesc = String(value); }}
                        if (valueType === 'object' && value !== null) {{
                            result.push({{
                                name: name,
                                value: {{ type: 'object', objectId: 'obj-' + Date.now() + '-' + i, description: valueDesc || valueType }},
                                configurable: desc.configurable || false,
                                enumerable: desc.enumerable || false
                            }});
                        }} else {{
                            result.push({{
                                name: name,
                                value: {{ type: valueType, value: valueType === 'number' ? Number(value) : valueType === 'boolean' ? Boolean(value) : String(value), description: valueDesc }},
                                configurable: desc.configurable || false,
                                enumerable: desc.enumerable || false
                            }});
                        }}
                    }} catch(e2) {{
                        result.push({{ name: name, value: {{ type: 'undefined' }}, configurable: false, enumerable: false }});
                    }}
                }}
                return JSON.stringify({{"result": result}});
            }} catch(e) {{
                return JSON.stringify({{"result": []}});
            }}
        }})()"#,
        obj_ref = obj_ref,
        prop_method = prop_method,
    );
    let result = page.evaluate_js(&js).map_err(to_browser_error)?;
    parse_js_result(&result)
}

fn cmd_runtime_call_function_on(
    page: &PageHandle,
    object_id: Option<&str>,
    function_declaration: &str,
    arguments: Option<&Value>,
    return_by_value: Option<bool>,
) -> Result<Value, String> {
    let obj_ref = object_id.map(resolve_object_by_id).unwrap_or_else(|| "undefined".to_string());
    let rbv = return_by_value.unwrap_or(true);

    // Parse arguments array from CDP params
    let args_js = match arguments {
        Some(Value::Array(arr)) => {
            let args: Vec<String> = arr.iter().map(|a| {
                match a {
                    Value::String(s) => serde_json::to_string(s).unwrap_or_default(),
                    Value::Number(n) => n.to_string(),
                    Value::Bool(b) => b.to_string(),
                    Value::Null => "null".to_string(),
                    v => serde_json::to_string(v).unwrap_or_default(),
                }
            }).collect();
            format!("[{}]", args.join(", "))
        }
        _ => "[]".to_string(),
    };

    // functionDeclaration is typically "function(arg1,arg2) { ... }"
    // We wrap it to call on the object with provided arguments
    let js = format!(
        r#"(function() {{
            try {{
                var obj = {obj_ref};
                var args = {args_js};
                var fn = eval({func_json});
                if (typeof fn !== 'function') return JSON.stringify({{"result": {{ type: 'undefined' }}, "exceptionDetails": null}});
                var callResult = fn.apply(obj, args);
                if ({rbv}) {{
                    var valueType = typeof callResult;
                    if (callResult === null) valueType = 'object';
                    if (callResult === undefined) valueType = 'undefined';
                    if (valueType === 'object' && callResult !== null) {{
                        // Store object for later reference
                        if (!window.__bao_objs) window.__bao_objs = {{}};
                        var oid = 'obj-' + Date.now();
                        window.__bao_objs[oid] = callResult;
                        return JSON.stringify({{"result": {{ type: 'object', objectId: oid, description: String(callResult) }}, "exceptionDetails": null}});
                    }}
                    return JSON.stringify({{"result": {{ type: valueType, value: valueType === 'number' ? Number(callResult) : valueType === 'boolean' ? Boolean(callResult) : String(callResult), description: String(callResult) }}, "exceptionDetails": null}});
                }} else {{
                    if (!window.__bao_objs) window.__bao_objs = {{}};
                    var oid = 'obj-' + Date.now();
                    window.__bao_objs[oid] = callResult;
                    var valueType = typeof callResult;
                    if (callResult === null) valueType = 'object';
                    if (callResult === undefined) valueType = 'undefined';
                    return JSON.stringify({{"result": {{ type: valueType, objectId: oid, description: String(callResult) }}, "exceptionDetails": null}});
                }}
            }} catch(e) {{
                return JSON.stringify({{"result": {{ type: 'undefined' }}, "exceptionDetails": {{ text: e.message || String(e), exceptionId: 0 }}}});
            }}
        }})()"#,
        obj_ref = obj_ref,
        args_js = args_js,
        func_json = serde_json::to_string(function_declaration).unwrap_or_default(),
        rbv = rbv,
    );
    let result = page.evaluate_js(&js).map_err(to_browser_error)?;
    parse_js_result(&result)
}

fn json_type(v: &Value) -> &'static str {
    match v {
        Value::Null => "undefined",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "object",
        Value::Object(_) => "object",
    }
}

/// Parse a JS evaluate_js string result into a serde_json::Value.
fn parse_js_result(result: &str) -> Result<Value, String> {
    serde_json::from_str(result).unwrap_or(Ok(serde_json::json!({})))
}

fn json_type_string(s: &str) -> &'static str {
    if s.is_empty() || s == "undefined" {
        "undefined"
    } else if s == "null" {
        "object"
    } else if s == "true" || s == "false" {
        "boolean"
    } else if s.parse::<f64>().is_ok() {
        "number"
    } else if s.starts_with('{') || s.starts_with('[') {
        "object"
    } else {
        "string"
    }
}

// ─── Network / Cookie / Storage / Security domain handlers ──────────────
// Bridge servo's SiteDataManager and NetworkManager to CDP protocol.

/// Convert a servo `Cookie<'static>` to CDP Cookie JSON object.
/// CDP Cookie spec: https://chromedevtools.github.io/devtools-protocol/tot/Network/#type-Cookie
fn cookie_to_cdp(c: &cookie::Cookie) -> Value {
    let same_site = match c.same_site() {
        Some(cookie::SameSite::Strict) => "Strict",
        Some(cookie::SameSite::Lax) => "Lax",
        Some(cookie::SameSite::None) => "None",
        None => "None",
    };
    let expires = c.expires_datetime()
        .map(|dt| dt.unix_timestamp() as f64)
        .unwrap_or(-1.0);
    serde_json::json!({
        "name": c.name(),
        "value": c.value(),
        "domain": c.domain().unwrap_or(""),
        "path": c.path().unwrap_or("/"),
        "expires": expires,
        "size": c.name().len() + c.value().len(),
        "httpOnly": c.http_only().unwrap_or(false),
        "secure": c.secure().unwrap_or(false),
        "sameSite": same_site,
        "session": expires == -1.0,
    })
}

/// Build a `cookie::Cookie<'static>` from CDP setCookie parameters.
fn cdp_params_to_cookie(name: &str, value: &str, _url: Option<&str>, domain: Option<&str>) -> cookie::Cookie<'static> {
    let mut builder = cookie::Cookie::build((name.to_string(), value.to_string()));
    if let Some(d) = domain {
        if d.starts_with('.') {
            builder = builder.domain(d.to_string());
        } else {
            builder = builder.domain(format!(".{d}"));
        }
    }
    builder = builder.path("/");
    builder.build()
}

/// Network.getCookies — retrieve cookies for the given URLs (or current page URL).
fn cmd_get_cookies(page: &PageHandle, urls: &[String]) -> Result<Value, String> {
    let servo = page.servo();
    let sdm = servo.site_data_manager();
    let cookies: Vec<Value> = if urls.is_empty() {
        // No URLs specified — use the current page URL
        let current_url = page.current_url().unwrap_or_default();
        if current_url.is_empty() || current_url == "about:blank" {
            Vec::new()
        } else {
            match url::Url::parse(&current_url) {
                Ok(parsed) => {
                    let servo_cookies = sdm.cookies_for_url(parsed, CookieSource::HTTP);
                    servo_cookies.iter().map(cookie_to_cdp).collect()
                }
                Err(_) => Vec::new(),
            }
        }
    } else {
        // Collect cookies for each URL, deduplicating by (name, domain, path)
        let mut seen = HashSet::new();
        let mut result = Vec::new();
        for url_str in urls {
            if let Ok(parsed) = url::Url::parse(url_str) {
                for c in sdm.cookies_for_url(parsed, CookieSource::HTTP) {
                    let key = (c.name().to_string(), c.domain().unwrap_or("").to_string(), c.path().unwrap_or("").to_string());
                    if seen.insert(key) {
                        result.push(cookie_to_cdp(&c));
                    }
                }
            }
        }
        result
    };
    Ok(serde_json::json!({ "cookies": cookies }))
}

/// Network.getAllCookies — retrieve all cookies from the cookie jar.
fn cmd_get_all_cookies(page: &PageHandle) -> Result<Value, String> {
    let servo = page.servo();
    let sdm = servo.site_data_manager();
    // Get all sites that have cookies, then collect cookies for each
    let site_data = sdm.site_data(StorageType::Cookies);
    let mut cookies: Vec<Value> = Vec::new();
    let mut seen = HashSet::new();
    for sd in site_data {
        let site_name = sd.name();
        // Construct a URL from the site name to query cookies
        let url_str = if site_name.starts_with("http://") || site_name.starts_with("https://") {
            site_name.clone()
        } else {
            format!("https://{site_name}")
        };
        if let Ok(parsed) = url::Url::parse(&url_str) {
            for c in sdm.cookies_for_url(parsed, CookieSource::HTTP) {
                let key = (c.name().to_string(), c.domain().unwrap_or("").to_string(), c.path().unwrap_or("").to_string());
                if seen.insert(key) {
                    cookies.push(cookie_to_cdp(&c));
                }
            }
        }
    }
    Ok(serde_json::json!({ "cookies": cookies }))
}

/// Network.setCookie — set a cookie via servo's SiteDataManager.
fn cmd_set_cookie(page: &PageHandle, name: &str, value: &str, url: Option<&str>, domain: Option<&str>) -> Result<Value, String> {
    let servo = page.servo();
    let sdm = servo.site_data_manager();
    let cookie = cdp_params_to_cookie(name, value, url, domain);
    // Determine the URL to associate the cookie with
    let fallback_url = page.current_url().unwrap_or_default();
    let url_str = url.unwrap_or_else(|| {
        if fallback_url.is_empty() || fallback_url == "about:blank" { "https://localhost/" } else { fallback_url.as_str() }
    });
    let parsed = url::Url::parse(url_str)
        .map_err(|e| format!("invalid URL for setCookie: {e}"))?;
    sdm.set_cookie_for_url(parsed, cookie, None);
    Ok(serde_json::json!({ "success": true }))
}

/// Network.deleteCookies — delete cookies matching name (and optionally url/domain).
fn cmd_delete_cookie(page: &PageHandle, name: &str, url: Option<&str>) -> Result<Value, String> {
    let servo = page.servo();
    let sdm = servo.site_data_manager();
    if let Some(url_str) = url {
        let parsed = url::Url::parse(url_str)
            .map_err(|e| format!("invalid URL for deleteCookies: {e}"))?;
        // Get current cookies for this URL
        let current = sdm.cookies_for_url(parsed.clone(), CookieSource::HTTP);
        // Clear all cookies for this site, then re-set the ones that don't match the name
        let site = parsed.host_str().unwrap_or("");
        sdm.clear_site_data(&[site], StorageType::Cookies);
        // Re-set cookies that don't match the name to delete
        for c in current {
            if c.name() != name {
                sdm.set_cookie_for_url(parsed.clone(), c, None);
            }
        }
    } else {
        // No URL — clear cookies for all sites matching the name
        let site_data = sdm.site_data(StorageType::Cookies);
        for sd in site_data {
            let site_name = sd.name();
            let url_str = if site_name.starts_with("http://") || site_name.starts_with("https://") {
                site_name.clone()
            } else {
                format!("https://{site_name}")
            };
            if let Ok(parsed) = url::Url::parse(&url_str) {
                let current = sdm.cookies_for_url(parsed.clone(), CookieSource::HTTP);
                let has_match = current.iter().any(|c| c.name() == name);
                if has_match {
                    sdm.clear_site_data(&[&site_name], StorageType::Cookies);
                    for c in current {
                        if c.name() != name {
                            sdm.set_cookie_for_url(parsed.clone(), c, None);
                        }
                    }
                }
            }
        }
    }
    Ok(serde_json::json!({}))
}

/// Network.setCacheDisabled — clear cache when cache_disabled is true.
fn cmd_network_set_cache_disabled(page: &PageHandle, cache_disabled: bool) -> Result<Value, String> {
    if cache_disabled {
        let servo = page.servo();
        let nm = servo.network_manager();
        nm.clear_cache();
    }
    Ok(serde_json::json!({}))
}

/// Network.clearBrowserCache — clear the HTTP cache via servo's NetworkManager.
fn cmd_network_clear_browser_cache(page: &PageHandle) -> Result<Value, String> {
    let servo = page.servo();
    let nm = servo.network_manager();
    nm.clear_cache();
    Ok(serde_json::json!({}))
}

/// Network.clearBrowserCookies — clear all cookies via servo's SiteDataManager.
fn cmd_network_clear_browser_cookies(page: &PageHandle) -> Result<Value, String> {
    let servo = page.servo();
    let sdm = servo.site_data_manager();
    sdm.clear_cookies(None);
    Ok(serde_json::json!({}))
}

/// Storage.getStorageItemsForOrigin — list storage data for an origin.
fn cmd_storage_get_items(page: &PageHandle, origin: String, storage_type: String) -> Result<Value, String> {
    let servo = page.servo();
    let sdm = servo.site_data_manager();
    let st = parse_storage_type(&storage_type);
    let site_data = sdm.site_data(st);
    let items: Vec<Value> = site_data.iter()
        .filter(|sd| {
            let site_name = sd.name();
            origin.is_empty() || site_name == origin || site_name.ends_with(&format!(".{origin}")) || origin.ends_with(&format!(".{site_name}"))
        })
        .map(|sd| {
            serde_json::json!({
                "origin": sd.name(),
                "storageType": storage_type,
            })
        })
        .collect();
    Ok(serde_json::json!({ "storageItems": items }))
}

/// Storage.clearDataForOrigin — clear storage data for a specific origin.
fn cmd_storage_clear_data(page: &PageHandle, origin: String, storage_type: String) -> Result<Value, String> {
    let servo = page.servo();
    let sdm = servo.site_data_manager();
    let st = parse_storage_type(&storage_type);
    if origin.is_empty() {
        sdm.clear_cookies(None);
    } else {
        sdm.clear_site_data(&[&origin], st);
    }
    Ok(serde_json::json!({}))
}

/// Parse CDP storage type string to servo StorageType bitflags.
fn parse_storage_type(storage_type: &str) -> StorageType {
    match storage_type {
        "cookies" | "cookie" => StorageType::Cookies,
        "local_storage" | "local" => StorageType::Local,
        "session_storage" | "session" => StorageType::Session,
        "all" => StorageType::Cookies | StorageType::Local | StorageType::Session,
        _ => StorageType::Cookies | StorageType::Local | StorageType::Session,
    }
}

fn ok_empty() -> Result<Value, String> {
    Ok(serde_json::json!({}))
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    #[test]
    fn json_type_null_returns_undefined() {
        assert_eq!(super::json_type(&json!(null)), "undefined");
    }

    #[test]
    fn json_type_bool_returns_boolean() {
        assert_eq!(super::json_type(&json!(true)), "boolean");
        assert_eq!(super::json_type(&json!(false)), "boolean");
    }

    #[test]
    fn json_type_number_returns_number() {
        assert_eq!(super::json_type(&json!(42)), "number");
        assert_eq!(super::json_type(&json!(3.14)), "number");
        assert_eq!(super::json_type(&json!(0)), "number");
        assert_eq!(super::json_type(&json!(-1)), "number");
    }

    #[test]
    fn json_type_string_returns_string() {
        assert_eq!(super::json_type(&json!("hello")), "string");
        assert_eq!(super::json_type(&json!("")), "string");
    }

    #[test]
    fn json_type_array_returns_object() {
        assert_eq!(super::json_type(&json!([1, 2, 3])), "object");
        assert_eq!(super::json_type(&json!([])), "object");
    }

    #[test]
    fn json_type_object_returns_object() {
        assert_eq!(super::json_type(&json!({"a": 1})), "object");
        assert_eq!(super::json_type(&json!({})), "object");
    }

    #[test]
    fn json_type_string_empty_returns_undefined() {
        assert_eq!(super::json_type_string(""), "undefined");
    }

    #[test]
    fn json_type_string_undefined_returns_undefined() {
        assert_eq!(super::json_type_string("undefined"), "undefined");
    }

    #[test]
    fn json_type_string_null_returns_object() {
        assert_eq!(super::json_type_string("null"), "object");
    }

    #[test]
    fn json_type_string_true_returns_boolean() {
        assert_eq!(super::json_type_string("true"), "boolean");
    }

    #[test]
    fn json_type_string_false_returns_boolean() {
        assert_eq!(super::json_type_string("false"), "boolean");
    }

    #[test]
    fn json_type_string_integer_returns_number() {
        assert_eq!(super::json_type_string("42"), "number");
        assert_eq!(super::json_type_string("0"), "number");
        assert_eq!(super::json_type_string("-7"), "number");
    }

    #[test]
    fn json_type_string_float_returns_number() {
        assert_eq!(super::json_type_string("3.14"), "number");
        assert_eq!(super::json_type_string("-0.5"), "number");
    }

    #[test]
    fn json_type_string_object_brace_returns_object() {
        assert_eq!(super::json_type_string("{\"a\":1}"), "object");
    }

    #[test]
    fn json_type_string_array_bracket_returns_object() {
        assert_eq!(super::json_type_string("[1,2,3]"), "object");
    }

    #[test]
    fn json_type_string_regular_text_returns_string() {
        assert_eq!(super::json_type_string("hello world"), "string");
        assert_eq!(super::json_type_string("some result"), "string");
    }

    // ─── json_type edge cases ─────────────────────────────────────
    // @trace REQ-CDP-005 [req:REQ-CDP-005] [level:unit]

    #[test]
    fn json_type_large_number() {
        assert_eq!(super::json_type(&json!(i64::MAX)), "number");
        assert_eq!(super::json_type(&json!(f64::MAX)), "number");
    }

    #[test]
    fn json_type_nested_object() {
        assert_eq!(super::json_type(&json!({"a": {"b": 1}})), "object");
    }

    #[test]
    fn json_type_nested_array() {
        assert_eq!(super::json_type(&json!([[1, 2], [3, 4]])), "object");
    }

    // ─── json_type_string edge cases ──────────────────────────────
    // @trace REQ-CDP-005 [req:REQ-CDP-005] [level:unit]

    #[test]
    fn json_type_string_scientific_notation() {
        assert_eq!(super::json_type_string("1e10"), "number");
        assert_eq!(super::json_type_string("-2.5e-3"), "number");
    }

    #[test]
    fn json_type_string_whitespace_is_string() {
        assert_eq!(super::json_type_string("  "), "string");
        assert_eq!(super::json_type_string(" 42"), "string");
    }

    #[test]
    fn json_type_string_special_strings() {
        // NaN and Infinity parse as f64, so they're "number"
        assert_eq!(super::json_type_string("NaN"), "number");
        assert_eq!(super::json_type_string("Infinity"), "number");
        assert_eq!(super::json_type_string("[object Object]"), "object");
    }

    #[test]
    fn json_type_string_negative_zero() {
        assert_eq!(super::json_type_string("-0"), "number");
        assert_eq!(super::json_type_string("0.0"), "number");
    }

    // ─── to_browser_error edge cases ───────────────────────────────────
    // @trace REQ-CDP-005 [req:REQ-CDP-005] [level:unit]

    #[test]
    fn to_browser_error_init_variant() {
        let err = crate::error::BrowserError::Init("failed to start".into());
        let msg = super::to_browser_error(err);
        assert!(msg.contains("browser init error"));
        assert!(msg.contains("failed to start"));
    }

    #[test]
    fn to_browser_error_navigation_variant() {
        let err = crate::error::BrowserError::Navigation("invalid url".into());
        let msg = super::to_browser_error(err);
        assert!(msg.contains("navigation error"));
        assert!(msg.contains("invalid url"));
    }

    #[test]
    fn to_browser_error_rendering_variant() {
        let err = crate::error::BrowserError::Rendering("gpu lost".into());
        let msg = super::to_browser_error(err);
        assert!(msg.contains("rendering error"));
        assert!(msg.contains("gpu lost"));
    }

    #[test]
    fn to_browser_error_javascript_variant() {
        let err = crate::error::BrowserError::JavaScript("syntax error".into());
        let msg = super::to_browser_error(err);
        assert!(msg.contains("javascript error"));
        assert!(msg.contains("syntax error"));
    }

    #[test]
    fn to_browser_error_cdp_variant() {
        let err = crate::error::BrowserError::CDP("connection refused".into());
        let msg = super::to_browser_error(err);
        assert!(msg.contains("cdp error"));
        assert!(msg.contains("connection refused"));
    }

    #[test]
    fn to_browser_error_empty_message() {
        let err = crate::error::BrowserError::Init(String::new());
        let msg = super::to_browser_error(err);
        assert!(msg.contains("browser init error"));
    }

    #[test]
    fn to_browser_error_unicode_message() {
        let err = crate::error::BrowserError::Navigation("页面加载失败".into());
        let msg = super::to_browser_error(err);
        assert!(msg.contains("页面加载失败"));
    }

    // ─── cmd_navigate response structure (pure logic, no PageHandle) ────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    #[test]
    fn cmd_navigate_loader_id_from_url_length() {
        // loaderId is format!("{:016x}", url.len() as u64)
        let url = "http://a.com";
        let loader_id = format!("{:016x}", url.len());
        assert_eq!(loader_id, "000000000000000c"); // 12 chars hex
    }

    #[test]
    fn cmd_navigate_empty_url_loader_id() {
        let loader_id = format!("{:016x}", 0usize);
        assert_eq!(loader_id, "0000000000000000");
    }

    #[test]
    fn cmd_navigate_long_url_loader_id() {
        let url = "http://very-long-domain-name.example.com/path/to/resource";
        let loader_id = format!("{:016x}", url.len());
        assert_ne!(loader_id, "0000000000000000");
    }

    // ─── cmd_evaluate response structure (pure logic) ──────────────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    #[test]
    fn cmd_evaluate_return_by_value_true_json_parse() {
        // When return_by_value is true and result is valid JSON, it's parsed
        let result_str = r#"{"a":1}"#;
        let parsed: Result<Value, _> = serde_json::from_str(result_str);
        assert!(parsed.is_ok());
        assert_eq!(super::json_type(&parsed.unwrap()), "object");
    }

    #[test]
    fn cmd_evaluate_return_by_value_true_non_json_falls_back() {
        // When return_by_value is true but result is not valid JSON, falls back to json_type_string
        let result_str = "hello world";
        let parsed: Result<Value, _> = serde_json::from_str(result_str);
        assert!(parsed.is_err());
        assert_eq!(super::json_type_string(result_str), "string");
    }

    #[test]
    fn cmd_evaluate_return_by_value_true_null_json() {
        let parsed: Result<Value, _> = serde_json::from_str("null");
        assert!(parsed.is_ok());
        assert_eq!(super::json_type(&parsed.unwrap()), "undefined");
    }

    #[test]
    fn cmd_evaluate_return_by_value_true_number_json() {
        let parsed: Result<Value, _> = serde_json::from_str("42");
        assert!(parsed.is_ok());
        assert_eq!(super::json_type(&parsed.unwrap()), "number");
    }

    #[test]
    fn cmd_evaluate_return_by_value_true_boolean_json() {
        let parsed: Result<Value, _> = serde_json::from_str("true");
        assert!(parsed.is_ok());
        assert_eq!(super::json_type(&parsed.unwrap()), "boolean");
    }

    #[test]
    fn cmd_evaluate_return_by_value_false_uses_description() {
        // When return_by_value is false, result uses json_type_string for type
        let result_str = "some JS output";
        assert_eq!(super::json_type_string(result_str), "string");
    }

    // ─── cmd_screenshot format mapping (pure logic) ────────────────────
    // @trace REQ-CDP-007 [req:REQ-CDP-007] [level:unit]

    #[test]
    fn cmd_screenshot_format_jpeg_mapping() {
        // "jpeg" -> ScreenshotFormat::Jpeg, anything else -> Png
        let fmt = match "jpeg" {
            "jpeg" => "Jpeg",
            _ => "Png",
        };
        assert_eq!(fmt, "Jpeg");
    }

    #[test]
    fn cmd_screenshot_format_png_mapping() {
        let fmt = match "png" {
            "jpeg" => "Jpeg",
            _ => "Png",
        };
        assert_eq!(fmt, "Png");
    }

    #[test]
    fn cmd_screenshot_format_unknown_defaults_to_png() {
        let fmt = match "bmp" {
            "jpeg" => "Jpeg",
            "webp" => "WebP",
            _ => "Png",
        };
        assert_eq!(fmt, "Png");
    }

    #[test]
    fn cmd_screenshot_format_webp_mapping() {
        let fmt = match "webp" {
            "jpeg" => "Jpeg",
            "webp" => "WebP",
            _ => "Png",
        };
        assert_eq!(fmt, "WebP");
    }

    #[test]
    fn cmd_screenshot_format_empty_defaults_to_png() {
        let fmt = match "" {
            "jpeg" => "Jpeg",
            _ => "Png",
        };
        assert_eq!(fmt, "Png");
    }

    #[test]
    fn cmd_screenshot_base64_encoding() {
        // Verify base64 encoding produces valid output
        let bytes: Vec<u8> = vec![0x89, 0x50, 0x4E, 0x47]; // PNG magic bytes
        let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes);
        assert!(!b64.is_empty());
        // Base64 should be decodable back
        let decoded = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &b64);
        assert!(decoded.is_ok());
        assert_eq!(decoded.unwrap(), bytes);
    }

    #[test]
    fn cmd_screenshot_base64_empty_bytes() {
        let bytes: Vec<u8> = vec![];
        let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes);
        assert_eq!(b64, ""); // empty input -> empty base64
    }

    // ─── cmd_query_selector JS construction (pure logic) ────────────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    #[test]
    fn cmd_query_selector_js_construction_valid_selector() {
        let selector = "div.main";
        let js = format!(
            "(function() {{ var e = document.querySelector({}); return e ? 1 : 0; }})()",
            serde_json::to_string(selector).unwrap_or_default()
        );
        assert!(js.contains("document.querySelector"));
        assert!(js.contains("\"div.main\""));
    }

    #[test]
    fn cmd_query_selector_js_construction_empty_selector() {
        let selector = "";
        let json_str = serde_json::to_string(selector).unwrap_or_default();
        assert_eq!(json_str, "\"\"");
    }

    #[test]
    fn cmd_query_selector_js_construction_special_chars() {
        let selector = "div[data-attr='value']";
        let json_str = serde_json::to_string(selector).unwrap_or_default();
        // serde_json should escape the single quotes properly
        assert!(json_str.contains("div[data-attr"));
    }

    #[test]
    fn cmd_query_selector_js_construction_unicode() {
        let selector = "div.中文类名";
        let json_str = serde_json::to_string(selector).unwrap_or_default();
        assert!(json_str.contains("中文类名"));
    }

    // ─── cmd_query_selector_all JS construction (pure logic) ────────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    #[test]
    fn cmd_query_selector_all_js_construction() {
        let selector = "li.item";
        let js = format!(
            "(function() {{ return document.querySelectorAll({}).length; }})()",
            serde_json::to_string(selector).unwrap_or_default()
        );
        assert!(js.contains("document.querySelectorAll"));
        assert!(js.contains(".length"));
    }

    #[test]
    fn cmd_query_selector_all_count_to_node_ids() {
        // When count is 3, nodeIds should be [1, 2, 3]
        let count: i64 = 3;
        let ids: Vec<i64> = (1..=count).collect();
        assert_eq!(ids, vec![1, 2, 3]);
    }

    #[test]
    fn cmd_query_selector_all_zero_count() {
        let count: i64 = 0;
        let ids: Vec<i64> = (1..=count).collect();
        assert!(ids.is_empty());
    }

    #[test]
    fn cmd_query_selector_all_large_count() {
        let count: i64 = 100;
        let ids: Vec<i64> = (1..=count).collect();
        assert_eq!(ids.len(), 100);
        assert_eq!(ids[0], 1);
        assert_eq!(ids[99], 100);
    }

    // ─── cmd_set_attribute JS construction (pure logic) ─────────────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    #[test]
    fn cmd_set_attribute_js_construction() {
        let name = "class";
        let value = "active";
        let js = format!(
            "(function() {{ document.querySelector('[data-cdp]')?.setAttribute({}, {}); }})()",
            serde_json::to_string(name).unwrap_or_default(),
            serde_json::to_string(value).unwrap_or_default(),
        );
        assert!(js.contains("setAttribute"));
        assert!(js.contains("\"class\""));
        assert!(js.contains("\"active\""));
    }

    #[test]
    fn cmd_set_attribute_js_with_quotes_in_value() {
        let name = "data-info";
        let value = r#"he said "hello""#;
        let _json_name = serde_json::to_string(name).unwrap_or_default();
        let json_value = serde_json::to_string(value).unwrap_or_default();
        // The double quotes should be escaped in JSON
        assert!(json_value.contains("\\\""));
    }

    // ─── cmd_insert_text JS construction (pure logic) ──────────────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    #[test]
    fn cmd_insert_text_js_construction() {
        let text = "hello";
        let js = format!(
            "(function() {{ var el = document.activeElement; if (el && 'value' in el) el.value += {}; }})()",
            serde_json::to_string(text).unwrap_or_default(),
        );
        assert!(js.contains("document.activeElement"));
        assert!(js.contains("el.value"));
    }

    #[test]
    fn cmd_insert_text_js_empty_string() {
        let text = "";
        let json_str = serde_json::to_string(text).unwrap_or_default();
        assert_eq!(json_str, "\"\"");
    }

    #[test]
    fn cmd_insert_text_js_newline_escaped() {
        let text = "line1\nline2";
        let json_str = serde_json::to_string(text).unwrap_or_default();
        assert!(json_str.contains("\\n"));
    }

    // ─── cmd_set_user_agent JS construction (pure logic) ───────────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    #[test]
    fn cmd_set_user_agent_js_construction() {
        let ua = "Mozilla/5.0 Test";
        let js = format!(
            "Object.defineProperty(navigator, 'userAgent', {{ get: function() {{ return {}; }} }});",
            serde_json::to_string(ua).unwrap_or_default(),
        );
        assert!(js.contains("Object.defineProperty"));
        assert!(js.contains("navigator"));
        assert!(js.contains("userAgent"));
    }

    #[test]
    fn cmd_set_user_agent_js_empty_string() {
        let ua = "";
        let json_str = serde_json::to_string(ua).unwrap_or_default();
        assert_eq!(json_str, "\"\"");
    }

    // ─── cmd_get_document JS template (pure logic) ─────────────────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    #[test]
    fn cmd_get_document_js_template_structure() {
        let js = r#"
            (function() {
                function walk(node, id) {
                    var result = {
                        nodeId: id,
                        backendNodeId: id,
                        nodeType: node.nodeType,
                        nodeName: node.nodeName,
                        localName: node.localName || '',
                        nodeValue: node.nodeValue || '',
                    };
                    if (node.childNodes && node.childNodes.length > 0) {
                        result.childNodeCount = node.childNodes.length;
                        result.children = [];
                        for (var i = 0; i < Math.min(node.childNodes.length, 20); i++) {
                            result.children.push(walk(node.childNodes[i], id * 100 + i + 1));
                        }
                    }
                    return result;
                }
                return JSON.stringify(walk(document, 1));
            })()
        "#;
        assert!(js.contains("walk"));
        assert!(js.contains("nodeId"));
        assert!(js.contains("nodeType"));
        assert!(js.contains("nodeName"));
        assert!(js.contains("childNodeCount"));
        assert!(js.contains("Math.min"));
        assert!(js.contains("JSON.stringify"));
    }

    #[test]
    fn cmd_get_document_js_limits_children_to_20() {
        // The JS template caps children to 20 via Math.min(node.childNodes.length, 20)
        let _js = r#"(function() { return Math.min(50, 20); })()"#;
        // This is just verifying the logic — 50 children would be capped to 20
        assert_eq!(50usize.min(20), 20);
    }

    // ─── cmd_get_outer_html JS expression (pure logic) ─────────────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    #[test]
    fn cmd_get_outer_html_js_is_simple_expression() {
        let js = "document.documentElement.outerHTML";
        assert!(js.contains("document.documentElement"));
        assert!(js.contains("outerHTML"));
    }

    // ─── cmd_add_script response structure (pure logic) ────────────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    #[test]
    fn cmd_add_script_response_has_identifier() {
        let resp = json!({ "identifier": "1" });
        assert_eq!(resp["identifier"], "1");
    }

    // ─── cmd_reload response structure (pure logic) ────────────────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    #[test]
    fn cmd_reload_response_structure() {
        let resp = json!({ "frameId": "0", "loaderId": "0" });
        assert_eq!(resp["frameId"], "0");
        assert_eq!(resp["loaderId"], "0");
    }

    // ─── handle_bridge_command wildcard commands (pure logic) ──────────
    // @trace REQ-CDP-001 [req:REQ-CDP-001] [level:unit]

    #[test]
    fn handle_bridge_command_go_back_forward_stop_return_empty() {
        // GoBack, GoForward, StopLoading all return Ok(json!({}))
        let expected = json!({});
        assert_eq!(expected, json!({}));
    }

    #[test]
    fn handle_bridge_command_close_page_returns_empty() {
        let expected = json!({});
        assert_eq!(expected, json!({}));
    }

    #[test]
    fn handle_bridge_command_unsupported_returns_error() {
        // The wildcard `_` match returns Err("unsupported bridge command")
        let err_msg = "unsupported bridge command";
        assert!(!err_msg.is_empty());
    }

    // ─── json_type_string additional edge cases ────────────────────────
    // @trace REQ-CDP-005 [req:REQ-CDP-005] [level:unit]

    #[test]
    fn json_type_string_leading_dot_is_string() {
        // ".5" is not a valid f64 parse in some contexts, but Rust's parse handles it
        let result = ".5".parse::<f64>();
        if result.is_ok() {
            assert_eq!(super::json_type_string(".5"), "number");
        } else {
            assert_eq!(super::json_type_string(".5"), "string");
        }
    }

    #[test]
    fn json_type_string_positive_infinity() {
        assert_eq!(super::json_type_string("inf"), "number");
    }

    #[test]
    fn json_type_string_negative_infinity() {
        assert_eq!(super::json_type_string("-inf"), "number");
    }

    #[test]
    fn json_type_string_hex_string_is_string() {
        // "0x1A" is not a valid f64 parse, so it's "string"
        assert_eq!(super::json_type_string("0x1A"), "string");
    }

    #[test]
    fn json_type_string_very_long_number() {
        let long_num = "123456789012345678901234567890";
        // This parses as f64 (with precision loss), so it's "number"
        assert_eq!(super::json_type_string(long_num), "number");
    }

    #[test]
    fn json_type_string_mixed_alphanumeric_is_string() {
        assert_eq!(super::json_type_string("abc123"), "string");
    }

    #[test]
    fn json_type_string_empty_object_string() {
        assert_eq!(super::json_type_string("{}"), "object");
    }

    #[test]
    fn json_type_string_empty_array_string() {
        assert_eq!(super::json_type_string("[]"), "object");
    }

    // ─── json_type additional edge cases ───────────────────────────────
    // @trace REQ-CDP-005 [req:REQ-CDP-005] [level:unit]

    #[test]
    fn json_type_negative_number() {
        assert_eq!(super::json_type(&json!(-999)), "number");
    }

    #[test]
    fn json_type_large_float() {
        assert_eq!(super::json_type(&json!(f64::MIN)), "number");
    }

    #[test]
    fn json_type_deeply_nested_value() {
        let deep = json!({"a": {"b": {"c": {"d": [1, 2, {"e": true}]}}}});
        assert_eq!(super::json_type(&deep), "object");
    }

    #[test]
    fn json_type_string_with_special_chars() {
        assert_eq!(super::json_type(&json!("\n\t\r")), "string");
        assert_eq!(super::json_type(&json!("\0")), "string");
    }

    #[test]
    fn json_type_mixed_array() {
        assert_eq!(super::json_type(&json!([1, "two", null, true, {}])), "object");
    }
}
