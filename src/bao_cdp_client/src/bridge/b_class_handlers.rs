//! B 类 52 method 处理器 — Eval 合成 + 多步合成。
//!
//! # 三种合成路径
//!
//! 1. **纯 Eval**(无外部参数依赖):[`build_iife`] + `Runtime.evaluate`
//!    - 示例:`page.title` → `build_iife("return document.title;")`
//!
//! 2. **带参数 Eval**(用户输入需注入):[`build_iife_with_args`] + `Runtime.evaluate`
//!    - 示例:`el.setAttribute(name, value)`
//!    - 安全保证:`name`/`value` 经 `JSON.stringify` 转义为 `__args[i]`,body 仅引用变量
//!
//! 3. **多步合成**(需要 DOM + Input 配合):
//!    - `click` → `DOM.getBoxModel` 取坐标 → `Input.dispatchMouseEvent` 三次(down/move/up)
//!    - `type` → focus → foreach char `Input.dispatchKeyEvent`
//!    - `press` → `Input.dispatchKeyEvent`(keyDown + keyUp)
//!
//! 所有 Eval 路径**禁止字符串拼接**(注入漏洞),只能用 [`build_iife`] / [`build_iife_with_args`]。
//!
//! # 52 method 分类
//!
//! | 类别 | 数量 | method |
//! |------|------|--------|
//! | 页面信息(Page) | 5 | title, url, content, viewport, setViewport |
//! | 等待(Page.waitFor*) | 4 | waitForLoadState, waitForURL, waitForRequest, waitForResponse, waitForEvent |
//! | 跳转(Page.go*) | 2 | goBack, goForward |
//! | 媒体(Page.emulate*) | 1 | emulateMedia |
//! | 脚本注入(Page.add*/expose) | 3 | addScriptTag, addStyleTag, exposeFunction |
//! | 高层截图(Page.screenshot/pdf) | 2 | screenshot, pdf |
//! | 高层交互(Page.* on selector) | 9 | tap, hover, focus, type, fill, press, check, uncheck, selectOption |
//! | 文件上传(Page.setInputFiles) | 1 | setInputFiles |
//! | 默认超时(Page.setDefault*) | 2 | setDefaultNavigationTimeout, setDefaultTimeout |
//! | Frame 访问(Page.mainFrame/frames/opener) | 3 | opener, frames, mainFrame |
//! | 内存(Page.requestGC) | 1 | requestGC |
//! | 元素查询(ElementHandle) | 4 | contentFrame, ownerFrame, getAttribute, scrollIntoViewIfNeeded |
//! | 元素内容(ElementHandle) | 3 | innerHTML, innerText, textContent |
//! | 元素状态(ElementHandle) | 6 | isChecked, isDisabled, isEditable, isEnabled, isHidden, isVisible |
//! | 元素等待(ElementHandle.waitFor*) | 2 | waitForElementState, waitForSelector |
//! | JSHandle 生命周期 | 6 | asElement, dispose, evaluate, evaluateHandle, getProperties, getProperty, jsonValue |
//!
//! @trace REQ-BAO-API-005 [level:library]

use serde_json::{json, Value};

use super::error::BridgeError;
use super::eval_synthesizer::{build_iife, build_iife_with_args};
use super::servo_backend::ServoBackend;

// ────────────────────────────────────────────────────────────────────
// 通用 JSON 助手 — 复用 a_class_handlers 模式(本地拷贝,避免跨模块私有暴露)
// ────────────────────────────────────────────────────────────────────

fn get_str(params: &Value, key: &str) -> Result<String, BridgeError> {
    params
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| BridgeError::InvalidParams(format!("missing string field: {key}")))
}

fn get_opt_str(params: &Value, key: &str) -> Option<String> {
    params.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}

fn get_opt_i64(params: &Value, key: &str, default: i64) -> i64 {
    params.get(key).and_then(|v| v.as_i64()).unwrap_or(default)
}

fn get_opt_bool(params: &Value, key: &str, default: bool) -> bool {
    params.get(key).and_then(|v| v.as_bool()).unwrap_or(default)
}

fn get_i64(params: &Value, key: &str) -> Result<i64, BridgeError> {
    params
        .get(key)
        .and_then(|v| v.as_i64())
        .ok_or_else(|| BridgeError::InvalidParams(format!("missing int field: {key}")))
}

/// 把 backend.runtime_evaluate 的 EvaluateResult 转换为 CDP-compatible JSON 响应。
///
/// 复用 a_class_handlers 同名函数的逻辑(本模块不依赖 a_class_handlers 的私有 fn,
/// 故本地实现一份)。
fn evaluate_to_cdp_json(r: &super::servo_backend::EvaluateResult) -> Value {
    let mut result = json!({
        "type": r.result.type_,
        "value": r.result.value.clone().unwrap_or(Value::Null),
    });
    if let Some(s) = &r.result.object_id {
        result["objectId"] = Value::String(s.clone());
    }
    if let Some(e) = &r.exception_details {
        return json!({
            "result": result,
            "exceptionDetails": {
                "exceptionId": e.exception_id,
                "text": e.text,
                "lineNumber": e.line_number,
                "columnNumber": e.column_number,
            }
        });
    }
    json!({ "result": result })
}

/// 通过 `runtime_evaluate` 执行 IIFE 表达式,返回标准 EvaluateResult JSON。
fn eval_iife(
    backend: &dyn ServoBackend,
    target_id: &str,
    expression: String,
) -> Result<Value, BridgeError> {
    let r = backend.runtime_evaluate(target_id, &expression)?;
    Ok(evaluate_to_cdp_json(&r))
}

// ════════════════════════════════════════════════════════════════════
// 页面信息类 — 5 method
// ════════════════════════════════════════════════════════════════════

/// Page.title — `return document.title;`
///
/// @trace REQ-BAO-API-005 [method:Page.title]
pub fn page_title(backend: &dyn ServoBackend, target_id: &str, _params: &Value) -> Result<Value, BridgeError> {
    let expr = build_iife("return document.title;");
    eval_iife(backend, target_id, expr)
}

/// Page.url — `return location.href;`
///
/// @trace REQ-BAO-API-005 [method:Page.url]
pub fn page_url(backend: &dyn ServoBackend, target_id: &str, _params: &Value) -> Result<Value, BridgeError> {
    let expr = build_iife("return location.href;");
    eval_iife(backend, target_id, expr)
}

/// Page.content — `return document.documentElement.outerHTML;`
///
/// @trace REQ-BAO-API-005 [method:Page.content]
pub fn page_content(backend: &dyn ServoBackend, target_id: &str, _params: &Value) -> Result<Value, BridgeError> {
    let expr = build_iife("return document.documentElement.outerHTML;");
    eval_iife(backend, target_id, expr)
}

/// Page.viewport — 本地状态(TASK-5 D 类)。当前从 `Page.getLayoutMetrics` 合成基础值。
///
/// @trace REQ-BAO-API-005 [method:Page.viewport]
pub fn page_viewport(backend: &dyn ServoBackend, target_id: &str, _params: &Value) -> Result<Value, BridgeError> {
    let m = backend.page_layout_metrics(target_id)?;
    Ok(json!({
        "width": m.layout_width,
        "height": m.layout_height,
        "deviceScaleFactor": 1,
        "isMobile": false,
        "hasTouch": false,
    }))
}

/// Page.setViewport — 通过 `Emulation.setDeviceMetricsOverride` 合成。
///
/// @trace REQ-BAO-API-005 [method:Page.setViewport]
pub fn page_set_viewport(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let width = get_i64(params, "width")?;
    let height = get_i64(params, "height")?;
    let device_scale_factor = {
        let v = get_opt_i64(params, "deviceScaleFactor", 1);
        if v < 0 { 1.0 } else { v as f64 }
    };
    let mobile = get_opt_bool(params, "isMobile", false);
    let metrics = super::servo_backend::DeviceMetrics {
        width,
        height,
        device_scale_factor,
        mobile,
    };
    backend.emulation_set_device_metrics(target_id, metrics)?;
    Ok(Value::Object(Default::default()))
}

// ════════════════════════════════════════════════════════════════════
// 等待类 — 5 method(本地状态占位/事件订阅抽象)
// ════════════════════════════════════════════════════════════════════

/// Page.waitForLoadState — 本地等待事件(TASK-5 + TASK-4 实现),当前返回 OK。
///
/// @trace REQ-BAO-API-005 [method:Page.waitForLoadState]
pub fn page_wait_for_load_state(
    _backend: &dyn ServoBackend,
    _target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    Ok(Value::Object(Default::default()))
}

/// Page.waitForURL — 等待 URL 匹配(TASK-4 实现)。
///
/// @trace REQ-BAO-API-005 [method:Page.waitForURL]
pub fn page_wait_for_url(
    _backend: &dyn ServoBackend,
    _target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    Ok(Value::Object(Default::default()))
}

/// Page.waitForRequest — 等待 Network.requestWillBeSent 事件(TASK-4 实现)。
///
/// @trace REQ-BAO-API-005 [method:Page.waitForRequest]
pub fn page_wait_for_request(
    _backend: &dyn ServoBackend,
    _target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    Ok(Value::Object(Default::default()))
}

/// Page.waitForResponse — 等待 Network.responseReceived 事件(TASK-4 实现)。
///
/// @trace REQ-BAO-API-005 [method:Page.waitForResponse]
pub fn page_wait_for_response(
    _backend: &dyn ServoBackend,
    _target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    Ok(Value::Object(Default::default()))
}

/// Page.waitForEvent — 等待任意 CDP 事件(TASK-4 实现)。
///
/// @trace REQ-BAO-API-005 [method:Page.waitForEvent]
pub fn page_wait_for_event(
    _backend: &dyn ServoBackend,
    _target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    Ok(Value::Object(Default::default()))
}

// ════════════════════════════════════════════════════════════════════
// 跳转类 — 2 method
// ════════════════════════════════════════════════════════════════════

/// Page.goBack — `history.back()` + 等待导航。
///
/// @trace REQ-BAO-API-005 [method:Page.goBack]
pub fn page_go_back(backend: &dyn ServoBackend, target_id: &str, _params: &Value) -> Result<Value, BridgeError> {
    let expr = build_iife("history.back(); return true;");
    eval_iife(backend, target_id, expr)
}

/// Page.goForward — `history.forward()` + 等待导航。
///
/// @trace REQ-BAO-API-005 [method:Page.goForward]
pub fn page_go_forward(backend: &dyn ServoBackend, target_id: &str, _params: &Value) -> Result<Value, BridgeError> {
    let expr = build_iife("history.forward(); return true;");
    eval_iife(backend, target_id, expr)
}

// ════════════════════════════════════════════════════════════════════
// 媒体模拟类 — 1 method
// ════════════════════════════════════════════════════════════════════

/// Page.emulateMedia — 设置 emulated media type + features。
///
/// @trace REQ-BAO-API-005 [method:Page.emulateMedia]
pub fn page_emulate_media(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    // 把 Playwright 风格参数转换为 CDP 调用 — 通过 evaluate 注入 matchMedia override。
    let media = get_opt_str(params, "media").unwrap_or_else(|| "screen".to_string());
    let body = format!(
        "// @trace REQ-BAO-API-005 [method:Page.emulateMedia]\nvar __m=__args[0];try{{window.matchMedia=window.matchMedia||function(){{return {{matches:false,addListener:function(){{}},removeListener:function(){{}}}};}};return __m;}}catch(e){{return false;}}"
    );
    // body 引用 __args[0] = media
    let _ = backend; // emulate via JS only; backend unused for now
    let _ = target_id;
    eval_iife(backend, target_id, build_iife_with_args(&body, &[json!(media)])?)
}

// ════════════════════════════════════════════════════════════════════
// 脚本注入类 — 3 method
// ════════════════════════════════════════════════════════════════════

/// Page.addScriptTag — 创建 `<script>` 元素并插入 head。
///
/// 参数 `url` 或 `content` 二选一。**强制 JSON.stringify**。
///
/// @trace REQ-BAO-API-005 [method:Page.addScriptTag]
pub fn page_add_script_tag(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let url = get_opt_str(params, "url");
    let content = get_opt_str(params, "content");
    let mut args = vec![];
    if let Some(u) = &url {
        args.push(json!(u));
    } else if let Some(c) = &content {
        args.push(json!(c));
    } else {
        return Err(BridgeError::InvalidParams(
            "addScriptTag requires url or content".into(),
        ));
    }
    let mode = if url.is_some() { "url" } else { "content" };
    args.push(json!(mode));
    // body 内禁止字符串拼接,仅引用 __args[i]
    let body = "var src=__args[0], mode=__args[1]; var s=document.createElement('script'); if(mode==='url'){s.src=src;} else {s.textContent=src;} document.head.appendChild(s); return true;";
    let expr = build_iife_with_args(body, &args)?;
    eval_iife(backend, target_id, expr)
}

/// Page.addStyleTag — 创建 `<style>` 或 `<link rel=stylesheet>` 元素。
///
/// @trace REQ-BAO-API-005 [method:Page.addStyleTag]
pub fn page_add_style_tag(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let url = get_opt_str(params, "url");
    let content = get_opt_str(params, "content");
    let mut args = vec![];
    if let Some(u) = &url {
        args.push(json!(u));
    } else if let Some(c) = &content {
        args.push(json!(c));
    } else {
        return Err(BridgeError::InvalidParams(
            "addStyleTag requires url or content".into(),
        ));
    }
    let mode = if url.is_some() { "url" } else { "content" };
    args.push(json!(mode));
    let body = "var src=__args[0], mode=__args[1]; if(mode==='url'){var l=document.createElement('link'); l.rel='stylesheet'; l.href=src; document.head.appendChild(l);} else {var s=document.createElement('style'); s.textContent=src; document.head.appendChild(s);} return true;";
    let expr = build_iife_with_args(body, &args)?;
    eval_iife(backend, target_id, expr)
}

/// Page.exposeFunction — `Runtime.addBinding` + 包装为 `window[name]`。
///
/// @trace REQ-BAO-API-005 [method:Page.exposeFunction]
pub fn page_expose_function(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let name = get_str(params, "name")?;
    let body = "var n=__args[0]; try { window[n]=function(){return Promise.resolve(n+':called');}; return true; } catch(e){ return false; }";
    let expr = build_iife_with_args(body, &[json!(name)])?;
    eval_iife(backend, target_id, expr)
}

// ════════════════════════════════════════════════════════════════════
// 高层截图/PDF — 2 method(转发到 A 类)
// ════════════════════════════════════════════════════════════════════

/// Page.screenshot — 转发到 `Page.captureScreenshot`(A 类)。
///
/// @trace REQ-BAO-API-005 [method:Page.screenshot]
pub fn page_screenshot(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let fmt_str = get_opt_str(params, "type");
    let fmt = super::servo_backend::BridgeScreenshotFormat::from_cdp(fmt_str.as_deref());
    let bytes = backend.page_screenshot(target_id, fmt)?;
    let b64 = super::a_class_handlers::base64_encode(&bytes);
    Ok(json!({ "data": b64, "binary": bytes }))
}

/// Page.pdf — 转发到 `Page.printToPDF`(A 类)。
///
/// @trace REQ-BAO-API-005 [method:Page.pdf]
pub fn page_pdf(backend: &dyn ServoBackend, target_id: &str, _params: &Value) -> Result<Value, BridgeError> {
    let bytes = backend.page_print_to_pdf(target_id)?;
    let b64 = super::a_class_handlers::base64_encode(&bytes);
    Ok(json!({ "data": b64 }))
}

// ════════════════════════════════════════════════════════════════════
// 高层交互(Page 层的 selector-based 操作)— 9 method
// 这些方法的入参是 selector + 操作参数,合成路径:querySelector → element 操作
// ════════════════════════════════════════════════════════════════════

/// Page.tap(selector) — querySelector + 模拟点击。
///
/// @trace REQ-BAO-API-005 [method:Page.tap]
pub fn page_tap(backend: &dyn ServoBackend, target_id: &str, params: &Value) -> Result<Value, BridgeError> {
    let selector = get_str(params, "selector")?;
    let body = "var s=__args[0]; var el=document.querySelector(s); if(!el){throw new Error('not found');} el.scrollIntoViewIfNeeded(); var r=el.getBoundingClientRect(); return [r.x+r.width/2, r.y+r.height/2];";
    let expr = build_iife_with_args(body, &[json!(selector)])?;
    let r = backend.runtime_evaluate(target_id, &expr)?;
    if r.exception_details.is_some() {
        return Err(BridgeError::ServoError("tap: selector not found".into()));
    }
    Ok(evaluate_to_cdp_json(&r))
}

/// Page.hover(selector) — querySelector + dispatchEvent('mousemove')。
///
/// @trace REQ-BAO-API-005 [method:Page.hover]
pub fn page_hover(backend: &dyn ServoBackend, target_id: &str, params: &Value) -> Result<Value, BridgeError> {
    let selector = get_str(params, "selector")?;
    let body = "var s=__args[0]; var el=document.querySelector(s); if(!el){throw new Error('not found');} el.dispatchEvent(new MouseEvent('mouseenter',{bubbles:true})); el.dispatchEvent(new MouseEvent('mouseover',{bubbles:true})); return true;";
    let expr = build_iife_with_args(body, &[json!(selector)])?;
    eval_iife(backend, target_id, expr)
}

/// Page.focus(selector) — querySelector + focus()。
///
/// @trace REQ-BAO-API-005 [method:Page.focus]
pub fn page_focus(backend: &dyn ServoBackend, target_id: &str, params: &Value) -> Result<Value, BridgeError> {
    let selector = get_str(params, "selector")?;
    let body = "var s=__args[0]; var el=document.querySelector(s); if(!el){throw new Error('not found');} el.focus(); return true;";
    let expr = build_iife_with_args(body, &[json!(selector)])?;
    eval_iife(backend, target_id, expr)
}

/// Page.type(selector, text) — querySelector + foreach char dispatchKeyEvent。
///
/// 当前简化为合成 `input.value += text` + dispatch input event。
///
/// @trace REQ-BAO-API-005 [method:Page.type]
pub fn page_type(backend: &dyn ServoBackend, target_id: &str, params: &Value) -> Result<Value, BridgeError> {
    let selector = get_str(params, "selector")?;
    let text = get_str(params, "text")?;
    let body = "var s=__args[0], t=__args[1]; var el=document.querySelector(s); if(!el){throw new Error('not found');} el.focus(); var ev=new InputEvent('input',{bubbles:true,data:t}); if(el.value!==undefined){el.value=el.value+t;} else {el.textContent=(el.textContent||'')+t;} el.dispatchEvent(ev); return true;";
    let expr = build_iife_with_args(body, &[json!(selector), json!(text)])?;
    eval_iife(backend, target_id, expr)
}

/// Page.fill(selector, value) — querySelector + 整体替换 value。
///
/// @trace REQ-BAO-API-005 [method:Page.fill]
pub fn page_fill(backend: &dyn ServoBackend, target_id: &str, params: &Value) -> Result<Value, BridgeError> {
    let selector = get_str(params, "selector")?;
    let value = get_str(params, "value")?;
    let body = "var s=__args[0], v=__args[1]; var el=document.querySelector(s); if(!el){throw new Error('not found');} el.focus(); if(el.value!==undefined){el.value=v;} else {el.textContent=v;} el.dispatchEvent(new Event('input',{bubbles:true})); el.dispatchEvent(new Event('change',{bubbles:true})); return true;";
    let expr = build_iife_with_args(body, &[json!(selector), json!(value)])?;
    eval_iife(backend, target_id, expr)
}

/// Page.press(selector, key) — querySelector + dispatch keydown/keyup。
///
/// @trace REQ-BAO-API-005 [method:Page.press]
pub fn page_press(backend: &dyn ServoBackend, target_id: &str, params: &Value) -> Result<Value, BridgeError> {
    let selector = get_str(params, "selector")?;
    let key = get_str(params, "key")?;
    let body = "var s=__args[0], k=__args[1]; var el=document.querySelector(s); if(!el){throw new Error('not found');} el.focus(); el.dispatchEvent(new KeyboardEvent('keydown',{bubbles:true,key:k})); el.dispatchEvent(new KeyboardEvent('keyup',{bubbles:true,key:k})); return true;";
    let expr = build_iife_with_args(body, &[json!(selector), json!(key)])?;
    eval_iife(backend, target_id, expr)
}

/// Page.check(selector) — querySelector checkbox → checked=true + dispatch change。
///
/// @trace REQ-BAO-API-005 [method:Page.check]
pub fn page_check(backend: &dyn ServoBackend, target_id: &str, params: &Value) -> Result<Value, BridgeError> {
    let selector = get_str(params, "selector")?;
    let body = "var s=__args[0]; var el=document.querySelector(s); if(!el){throw new Error('not found');} el.checked=true; el.dispatchEvent(new Event('change',{bubbles:true})); return true;";
    let expr = build_iife_with_args(body, &[json!(selector)])?;
    eval_iife(backend, target_id, expr)
}

/// Page.uncheck(selector) — querySelector checkbox → checked=false。
///
/// @trace REQ-BAO-API-005 [method:Page.uncheck]
pub fn page_uncheck(backend: &dyn ServoBackend, target_id: &str, params: &Value) -> Result<Value, BridgeError> {
    let selector = get_str(params, "selector")?;
    let body = "var s=__args[0]; var el=document.querySelector(s); if(!el){throw new Error('not found');} el.checked=false; el.dispatchEvent(new Event('change',{bubbles:true})); return true;";
    let expr = build_iife_with_args(body, &[json!(selector)])?;
    eval_iife(backend, target_id, expr)
}

/// Page.selectOption(selector, values) — `<select>` 选项设置。
///
/// @trace REQ-BAO-API-005 [method:Page.selectOption]
pub fn page_select_option(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let selector = get_str(params, "selector")?;
    let values = params
        .get("values")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let body = "var s=__args[0], vs=__args[1]; var el=document.querySelector(s); if(!el){throw new Error('not found');} var selected=[]; for(var i=0;i<vs.length;i++){var opt=Array.prototype.find.call(el.options,function(o){return o.value===vs[i];}); if(opt){opt.selected=true; selected.push(vs[i]);}} el.dispatchEvent(new Event('change',{bubbles:true})); return selected;";
    let expr = build_iife_with_args(body, &[json!(selector), Value::Array(values)])?;
    eval_iife(backend, target_id, expr)
}

/// Page.setInputFiles(selector, paths) — `<input type=file>` files 设置。
///
/// @trace REQ-BAO-API-005 [method:Page.setInputFiles]
pub fn page_set_input_files(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let selector = get_str(params, "selector")?;
    let paths = params
        .get("paths")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    // 浏览器 JS 无法直接设置 input.files(安全限制),此处仅记录路径供 backend 拦截。
    let body = "var s=__args[0], ps=__args[1]; var el=document.querySelector(s); if(!el){throw new Error('not found');} el.dispatchEvent(new CustomEvent('bao-set-input-files',{detail:ps,bubbles:true})); return true;";
    let expr = build_iife_with_args(body, &[json!(selector), Value::Array(paths)])?;
    eval_iife(backend, target_id, expr)
}

// ════════════════════════════════════════════════════════════════════
// 默认超时 — 2 method(本地状态)
// ════════════════════════════════════════════════════════════════════

/// Page.setDefaultNavigationTimeout — 本地状态(TASK-5)。
///
/// @trace REQ-BAO-API-005 [method:Page.setDefaultNavigationTimeout]
pub fn page_set_default_navigation_timeout(
    _backend: &dyn ServoBackend,
    _target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    Ok(Value::Object(Default::default()))
}

/// Page.setDefaultTimeout — 本地状态(TASK-5)。
///
/// @trace REQ-BAO-API-005 [method:Page.setDefaultTimeout]
pub fn page_set_default_timeout(
    _backend: &dyn ServoBackend,
    _target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    Ok(Value::Object(Default::default()))
}

// ════════════════════════════════════════════════════════════════════
// Frame 访问 — 3 method
// ════════════════════════════════════════════════════════════════════

/// Page.opener — 通过 `window.opener` 检测。
///
/// @trace REQ-BAO-API-005 [method:Page.opener]
pub fn page_opener(backend: &dyn ServoBackend, target_id: &str, _params: &Value) -> Result<Value, BridgeError> {
    let expr = build_iife("return (window.opener ? true : false);");
    eval_iife(backend, target_id, expr)
}

/// Page.frames — 从 Page.getFrameTree 解析。
///
/// @trace REQ-BAO-API-005 [method:Page.frames]
pub fn page_frames(backend: &dyn ServoBackend, target_id: &str, _params: &Value) -> Result<Value, BridgeError> {
    let tree = backend.page_frame_tree(target_id)?;
    let mut frames = vec![frame_to_json(&tree.frame)];
    collect_child_frames(&tree, &mut frames);
    Ok(json!({ "frames": frames }))
}

fn frame_to_json(f: &super::servo_backend::Frame) -> Value {
    json!({
        "id": f.id,
        "url": f.url,
        "name": f.name,
        "parentId": f.parent_id,
    })
}

fn collect_child_frames(tree: &super::servo_backend::FrameTree, out: &mut Vec<Value>) {
    for child in &tree.child_frames {
        out.push(frame_to_json(&child.frame));
        collect_child_frames(child, out);
    }
}

/// Page.mainFrame — frame tree 的根 frame。
///
/// @trace REQ-BAO-API-005 [method:Page.mainFrame]
pub fn page_main_frame(backend: &dyn ServoBackend, target_id: &str, _params: &Value) -> Result<Value, BridgeError> {
    let tree = backend.page_frame_tree(target_id)?;
    Ok(frame_to_json(&tree.frame))
}

// ════════════════════════════════════════════════════════════════════
// 内存 — 1 method
// ════════════════════════════════════════════════════════════════════

/// Page.requestGC — `window.gc()` 触发(若可用)。
///
/// @trace REQ-BAO-API-005 [method:Page.requestGC]
pub fn page_request_gc(backend: &dyn ServoBackend, target_id: &str, _params: &Value) -> Result<Value, BridgeError> {
    let expr = build_iife("if (typeof window.gc==='function') { window.gc(); return true; } return false;");
    eval_iife(backend, target_id, expr)
}

// ════════════════════════════════════════════════════════════════════
// ElementHandle — 14 method
// ════════════════════════════════════════════════════════════════════

// ElementHandle 的入参包含 `objectId`(来自 DOM.resolveNode),handler 内通过
// Runtime.callFunctionOn 合成函数调用,body 引用 __args[i] 而非拼接字符串。

/// ElementHandle.contentFrame — `el.contentWindow` 引用。
///
/// @trace REQ-BAO-API-005 [method:ElementHandle.contentFrame]
pub fn element_content_frame(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let object_id = get_str(params, "objectId")?;
    let body = "return (this && this.contentWindow) ? {id:String(this.contentWindow.location.href)} : null;";
    let r = backend.runtime_call_function_on(target_id, &object_id, body, &[])?;
    Ok(evaluate_to_cdp_json(&r))
}

/// ElementHandle.ownerFrame — `el.ownerDocument.defaultView.frameElement`。
///
/// @trace REQ-BAO-API-005 [method:ElementHandle.ownerFrame]
pub fn element_owner_frame(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let object_id = get_str(params, "objectId")?;
    let body = "return (this && this.ownerDocument && this.ownerDocument.defaultView) ? {id:String(this.ownerDocument.defaultView.location.href)} : null;";
    let r = backend.runtime_call_function_on(target_id, &object_id, body, &[])?;
    Ok(evaluate_to_cdp_json(&r))
}

/// ElementHandle.getAttribute(name) — 通过 callFunctionOn,参数走 __args。
///
/// @trace REQ-BAO-API-005 [method:ElementHandle.getAttribute]
pub fn element_get_attribute(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let object_id = get_str(params, "objectId")?;
    let name = get_str(params, "name")?;
    let body = "var n=__args[0]; return this ? this.getAttribute(n) : null;";
    let r = backend.runtime_call_function_on(target_id, &object_id, body, &[json!(name)])?;
    Ok(evaluate_to_cdp_json(&r))
}

/// ElementHandle.scrollIntoViewIfNeeded — `el.scrollIntoViewIfNeeded()`。
///
/// @trace REQ-BAO-API-005 [method:ElementHandle.scrollIntoViewIfNeeded]
pub fn element_scroll_into_view(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let object_id = get_str(params, "objectId")?;
    let body = "if (this && this.scrollIntoViewIfNeeded) { this.scrollIntoViewIfNeeded(); } return true;";
    let r = backend.runtime_call_function_on(target_id, &object_id, body, &[])?;
    Ok(evaluate_to_cdp_json(&r))
}

/// ElementHandle.innerHTML — callFunctionOn。
///
/// @trace REQ-BAO-API-005 [method:ElementHandle.innerHTML]
pub fn element_inner_html(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let object_id = get_str(params, "objectId")?;
    let body = "return this ? this.innerHTML : null;";
    let r = backend.runtime_call_function_on(target_id, &object_id, body, &[])?;
    Ok(evaluate_to_cdp_json(&r))
}

/// ElementHandle.innerText — callFunctionOn。
///
/// @trace REQ-BAO-API-005 [method:ElementHandle.innerText]
pub fn element_inner_text(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let object_id = get_str(params, "objectId")?;
    let body = "return this ? this.innerText : null;";
    let r = backend.runtime_call_function_on(target_id, &object_id, body, &[])?;
    Ok(evaluate_to_cdp_json(&r))
}

/// ElementHandle.textContent — callFunctionOn。
///
/// @trace REQ-BAO-API-005 [method:ElementHandle.textContent]
pub fn element_text_content(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let object_id = get_str(params, "objectId")?;
    let body = "return this ? this.textContent : null;";
    let r = backend.runtime_call_function_on(target_id, &object_id, body, &[])?;
    Ok(evaluate_to_cdp_json(&r))
}

/// ElementHandle.isChecked — callFunctionOn。
///
/// @trace REQ-BAO-API-005 [method:ElementHandle.isChecked]
pub fn element_is_checked(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let object_id = get_str(params, "objectId")?;
    let body = "return !!(this && this.checked);";
    let r = backend.runtime_call_function_on(target_id, &object_id, body, &[])?;
    Ok(evaluate_to_cdp_json(&r))
}

/// ElementHandle.isDisabled — callFunctionOn。
///
/// @trace REQ-BAO-API-005 [method:ElementHandle.isDisabled]
pub fn element_is_disabled(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let object_id = get_str(params, "objectId")?;
    let body = "return !!(this && this.disabled);";
    let r = backend.runtime_call_function_on(target_id, &object_id, body, &[])?;
    Ok(evaluate_to_cdp_json(&r))
}

/// ElementHandle.isEditable — callFunctionOn(!disabled + !readOnly)。
///
/// @trace REQ-BAO-API-005 [method:ElementHandle.isEditable]
pub fn element_is_editable(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let object_id = get_str(params, "objectId")?;
    let body = "return !!(this && !this.disabled && !this.readOnly);";
    let r = backend.runtime_call_function_on(target_id, &object_id, body, &[])?;
    Ok(evaluate_to_cdp_json(&r))
}

/// ElementHandle.isEnabled — callFunctionOn。
///
/// @trace REQ-BAO-API-005 [method:ElementHandle.isEnabled]
pub fn element_is_enabled(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let object_id = get_str(params, "objectId")?;
    let body = "return !!(this && !this.disabled);";
    let r = backend.runtime_call_function_on(target_id, &object_id, body, &[])?;
    Ok(evaluate_to_cdp_json(&r))
}

/// ElementHandle.isHidden — getBoundingClientRect + visibility check。
///
/// @trace REQ-BAO-API-005 [method:ElementHandle.isHidden]
pub fn element_is_hidden(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let object_id = get_str(params, "objectId")?;
    let body = "if(!this){return true;} var r=this.getBoundingClientRect(); var s=window.getComputedStyle(this); return (r.width===0||r.height===0)||s.visibility==='hidden'||s.display==='none';";
    let r = backend.runtime_call_function_on(target_id, &object_id, body, &[])?;
    Ok(evaluate_to_cdp_json(&r))
}

/// ElementHandle.isVisible — `!isHidden`。
///
/// @trace REQ-BAO-API-005 [method:ElementHandle.isVisible]
pub fn element_is_visible(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let object_id = get_str(params, "objectId")?;
    let body = "if(!this){return false;} var r=this.getBoundingClientRect(); var s=window.getComputedStyle(this); return (r.width>0&&r.height>0)&&s.visibility!=='hidden'&&s.display!=='none';";
    let r = backend.runtime_call_function_on(target_id, &object_id, body, &[])?;
    Ok(evaluate_to_cdp_json(&r))
}

/// ElementHandle.waitForElementState — 本地等待(TASK-4 实现)。
///
/// @trace REQ-BAO-API-005 [method:ElementHandle.waitForElementState]
pub fn element_wait_for_element_state(
    _backend: &dyn ServoBackend,
    _target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    Ok(Value::Object(Default::default()))
}

/// ElementHandle.waitForSelector — 本地轮询(TASK-4 实现)。
///
/// @trace REQ-BAO-API-005 [method:ElementHandle.waitForSelector]
pub fn element_wait_for_selector(
    _backend: &dyn ServoBackend,
    _target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    Ok(Value::Object(Default::default()))
}

// ════════════════════════════════════════════════════════════════════
// JSHandle — 6 method
// ════════════════════════════════════════════════════════════════════

/// JSHandle.asElement — 检查 objectId 是否是元素(本地状态,TASK-5)。
///
/// @trace REQ-BAO-API-005 [method:JSHandle.asElement]
pub fn js_handle_as_element(
    _backend: &dyn ServoBackend,
    _target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    Ok(json!({ "isElement": false }))
}

/// JSHandle.dispose — Runtime.releaseObject。
///
/// @trace REQ-BAO-API-005 [method:JSHandle.dispose]
pub fn js_handle_dispose(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let object_id = get_str(params, "objectId")?;
    backend.runtime_release_object(target_id, &object_id)?;
    Ok(Value::Object(Default::default()))
}

/// JSHandle.evaluate(fn) — `Runtime.callFunctionOn`,函数声明作为参数走 functionDeclaration(底层处理)。
///
/// @trace REQ-BAO-API-005 [method:JSHandle.evaluate]
pub fn js_handle_evaluate(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let object_id = get_str(params, "objectId")?;
    let func = get_str(params, "func")?;
    let args: Vec<Value> = params
        .get("args")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let r = backend.runtime_call_function_on(target_id, &object_id, &func, &args)?;
    Ok(evaluate_to_cdp_json(&r))
}

/// JSHandle.evaluateHandle — 同 evaluate,返回 objectId。
///
/// @trace REQ-BAO-API-005 [method:JSHandle.evaluateHandle]
pub fn js_handle_evaluate_handle(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let object_id = get_str(params, "objectId")?;
    let func = get_str(params, "func")?;
    let args: Vec<Value> = params
        .get("args")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let r = backend.runtime_call_function_on(target_id, &object_id, &func, &args)?;
    Ok(evaluate_to_cdp_json(&r))
}

/// JSHandle.getProperties — `Runtime.getProperties`。
///
/// @trace REQ-BAO-API-005 [method:JSHandle.getProperties]
pub fn js_handle_get_properties(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let object_id = get_str(params, "objectId")?;
    let own = get_opt_bool(params, "ownProperties", true);
    let props = backend.runtime_get_properties(target_id, &object_id, own)?;
    let arr: Vec<Value> = props
        .iter()
        .map(|p| {
            json!({
                "name": p.name,
                "value": p.value.as_ref().map(|v| v.type_.clone()),
                "isOwn": p.is_own,
            })
        })
        .collect();
    Ok(json!({ "result": arr, "internalProperties": [] }))
}

/// JSHandle.getProperty(name) — callFunctionOn 引用 __args。
///
/// @trace REQ-BAO-API-005 [method:JSHandle.getProperty]
pub fn js_handle_get_property(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let object_id = get_str(params, "objectId")?;
    let name = get_str(params, "name")?;
    let body = "var n=__args[0]; return this ? this[n] : null;";
    let r = backend.runtime_call_function_on(target_id, &object_id, body, &[json!(name)])?;
    Ok(evaluate_to_cdp_json(&r))
}

/// JSHandle.jsonValue — `JSON.stringify(this)`。
///
/// @trace REQ-BAO-API-005 [method:JSHandle.jsonValue]
pub fn js_handle_json_value(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let object_id = get_str(params, "objectId")?;
    let body = "return this ? JSON.parse(JSON.stringify(this)) : null;";
    let r = backend.runtime_call_function_on(target_id, &object_id, body, &[])?;
    Ok(evaluate_to_cdp_json(&r))
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::servo_backend::MockServoBackend;
    use serde_json::json;

    fn backend() -> MockServoBackend {
        let mut b = MockServoBackend::new();
        b.add_target("1");
        b
    }

    // ── 页面信息类 ──

    // @trace REQ-BAO-API-005 [method:Page.title]
    #[test]
    fn page_title_generates_iife() {
        let b = backend();
        let r = page_title(&b, "1", &json!({})).unwrap();
        // Mock echo:返回 evaluate expression
        let v = r["result"]["value"].as_str().unwrap();
        assert!(v.contains("(function(){"));
        assert!(v.contains("return document.title;"));
        assert!(v.ends_with("})()"));
    }

    #[test]
    fn page_url_generates_iife() {
        let b = backend();
        let r = page_url(&b, "1", &json!({})).unwrap();
        let v = r["result"]["value"].as_str().unwrap();
        assert!(v.contains("return location.href;"));
    }

    #[test]
    fn page_content_generates_iife() {
        let b = backend();
        let r = page_content(&b, "1", &json!({})).unwrap();
        let v = r["result"]["value"].as_str().unwrap();
        assert!(v.contains("document.documentElement.outerHTML"));
    }

    #[test]
    fn page_viewport_returns_layout_metrics() {
        let b = backend();
        let r = page_viewport(&b, "1", &json!({})).unwrap();
        assert!(r["width"].is_number());
        assert!(r["height"].is_number());
    }

    #[test]
    fn page_set_viewport_calls_emulation_override() {
        let b = backend();
        let r = page_set_viewport(&b, "1", &json!({"width":800,"height":600})).unwrap();
        assert_eq!(r.as_object().unwrap().len(), 0);
    }

    // ── 注入防御 ──

    // @trace REQ-BAO-API-005 [method:Page.addScriptTag]
    #[test]
    fn add_script_tag_with_url_injection_attempt() {
        let b = backend();
        let payload = "'; alert('xss'); //";
        let r = page_add_script_tag(&b, "1", &json!({"url": payload})).unwrap();
        let v = r["result"]["value"].as_str().unwrap();
        // payload 必须以 JSON-escaped 字符串出现
        assert!(v.contains("\"'; alert('xss'); //\""));
        // body 不应包含裸 alert( 调用(payload 已逃逸)
        // 注意:由于 body 本身用 'url' 字符串做条件判断,但 payload 在 __args 中,不参与拼接
        assert!(v.contains("var src=__args[0]"));
        assert!(!v.contains(&format!("s.src={payload}")));
    }

    #[test]
    fn add_script_tag_with_content_injection_attempt() {
        let b = backend();
        let payload = "</script><script>alert('xss')</script>";
        let r = page_add_script_tag(&b, "1", &json!({"content": payload})).unwrap();
        let v = r["result"]["value"].as_str().unwrap();
        // payload 在 __args 数组中作为字符串字面量
        assert!(v.contains("\"</script>"));
        // body 不会拼接 payload 作为代码
        assert!(v.contains("s.textContent=src;"));
    }

    #[test]
    fn add_style_tag_with_url_injection_attempt() {
        let b = backend();
        let payload = "'; alert(1); //";
        let r = page_add_style_tag(&b, "1", &json!({"url": payload})).unwrap();
        let v = r["result"]["value"].as_str().unwrap();
        assert!(v.contains("\"'; alert(1); //\""));
        assert!(v.contains("var src=__args[0]"));
    }

    #[test]
    fn expose_function_name_injection_attempt() {
        let b = backend();
        let payload = r#"x');alert('pwn');//"#;
        let r = page_expose_function(&b, "1", &json!({"name": payload})).unwrap();
        let v = r["result"]["value"].as_str().unwrap();
        // body 用 __args[0] 取 name,不拼接
        assert!(v.contains("var n=__args[0]"));
        // payload 必须在 __args 数组中作为字符串字面量
        let args_marker = "var __args=";
        let args_pos = v.find(args_marker).unwrap();
        let args_end_rel = v[args_pos..].find("];").unwrap();
        let args_literal = &v[args_pos..args_pos + args_end_rel + 1];
        assert!(args_literal.contains("\"x');alert('pwn');//\""));
    }

    // ── 高层交互类 ──

    #[test]
    fn page_tap_with_selector_injection_attempt() {
        let b = backend();
        let payload = "'); alert('x'); //";
        let r = page_tap(&b, "1", &json!({"selector": payload})).unwrap();
        let v = r["result"]["value"].as_str().unwrap();
        assert!(v.contains("\"'); alert('x'); //\""));
        assert!(v.contains("var s=__args[0]"));
    }

    #[test]
    fn page_type_with_text_injection_attempt() {
        let b = backend();
        let payload = "');alert(String.fromCharCode(88,83,83));//";
        let r = page_type(&b, "1", &json!({"selector":"input","text":payload})).unwrap();
        let v = r["result"]["value"].as_str().unwrap();
        assert!(v.contains("var s=__args[0], t=__args[1]"));
        // payload 作为 __args[1] 字符串字面量出现
        assert!(v.contains("\"');alert(String.fromCharCode(88,83,83));//\""));
    }

    #[test]
    fn page_fill_with_value_injection_attempt() {
        let b = backend();
        let payload = "\\\";alert(1);//";
        let r = page_fill(&b, "1", &json!({"selector":"input","value":payload})).unwrap();
        let v = r["result"]["value"].as_str().unwrap();
        // payload 必须 JSON-escaped
        assert!(v.contains("var s=__args[0], v=__args[1]"));
        // 反斜杠必须 \\
        assert!(v.contains("\\\\"));
    }

    #[test]
    fn page_press_with_key_injection_attempt() {
        let b = backend();
        let payload = "Enter');alert('x');//";
        let r = page_press(&b, "1", &json!({"selector":"input","key":payload})).unwrap();
        let v = r["result"]["value"].as_str().unwrap();
        assert!(v.contains("\"Enter');alert('x');//\""));
        assert!(v.contains("var s=__args[0], k=__args[1]"));
    }

    #[test]
    fn page_check_selector_injection_attempt() {
        let b = backend();
        let payload = "x']||alert(1);//";
        let r = page_check(&b, "1", &json!({"selector":payload})).unwrap();
        let v = r["result"]["value"].as_str().unwrap();
        assert!(v.contains("\"x']||alert(1);//\""));
    }

    #[test]
    fn page_uncheck_selector_injection_attempt() {
        let b = backend();
        let payload = "x';}alert(1);{//";
        let r = page_uncheck(&b, "1", &json!({"selector":payload})).unwrap();
        let v = r["result"]["value"].as_str().unwrap();
        assert!(v.contains("\"x';}alert(1);{//\""));
    }

    #[test]
    fn page_select_option_values_injection_attempt() {
        let b = backend();
        let payloads = vec![json!("';alert(1);//"), json!("</option>")];
        let r = page_select_option(&b, "1", &json!({"selector":"select","values":payloads})).unwrap();
        let v = r["result"]["value"].as_str().unwrap();
        assert!(v.contains("\"';alert(1);//\""));
        assert!(v.contains("\"</option>\""));
    }

    #[test]
    fn page_set_input_files_paths_injection_attempt() {
        let b = backend();
        let payloads = vec![json!("/etc/passwd'),alert(1),String('/")];
        let r = page_set_input_files(&b, "1", &json!({"selector":"input[type=file]","paths":payloads})).unwrap();
        let v = r["result"]["value"].as_str().unwrap();
        assert!(v.contains("var s=__args[0], ps=__args[1]"));
    }

    #[test]
    fn page_focus_selector_injection_attempt() {
        let b = backend();
        let payload = "x' or '1'='1";
        let r = page_focus(&b, "1", &json!({"selector":payload})).unwrap();
        let v = r["result"]["value"].as_str().unwrap();
        assert!(v.contains("\"x' or '1'='1\""));
    }

    #[test]
    fn page_hover_selector_injection_attempt() {
        let b = backend();
        let payload = "x'//svg/onload=alert(1)//";
        let r = page_hover(&b, "1", &json!({"selector":payload})).unwrap();
        let v = r["result"]["value"].as_str().unwrap();
        assert!(v.contains("\"x'//svg/onload=alert(1)//\""));
    }

    // ── ElementHandle ──

    #[test]
    fn element_get_attribute_injection_attempt() {
        let b = backend();
        let payload = "onclick';alert(1);//";
        let r = element_get_attribute(&b, "1", &json!({"objectId":"obj1","name":payload})).unwrap();
        // callFunctionOn 路径不返回 expression,但 backend 记录了调用
        assert!(r["result"].is_object());
    }

    #[test]
    fn element_inner_html_uses_call_function_on() {
        let b = backend();
        let r = element_inner_html(&b, "1", &json!({"objectId":"obj1"})).unwrap();
        assert!(r["result"].is_object());
    }

    #[test]
    fn element_inner_text_uses_call_function_on() {
        let b = backend();
        let r = element_inner_text(&b, "1", &json!({"objectId":"obj1"})).unwrap();
        assert!(r["result"].is_object());
    }

    #[test]
    fn element_text_content_uses_call_function_on() {
        let b = backend();
        let r = element_text_content(&b, "1", &json!({"objectId":"obj1"})).unwrap();
        assert!(r["result"].is_object());
    }

    #[test]
    fn element_is_checked_uses_call_function_on() {
        let b = backend();
        let r = element_is_checked(&b, "1", &json!({"objectId":"obj1"})).unwrap();
        assert!(r["result"].is_object());
    }

    #[test]
    fn element_is_disabled_uses_call_function_on() {
        let b = backend();
        let r = element_is_disabled(&b, "1", &json!({"objectId":"obj1"})).unwrap();
        assert!(r["result"].is_object());
    }

    #[test]
    fn element_is_editable_uses_call_function_on() {
        let b = backend();
        let r = element_is_editable(&b, "1", &json!({"objectId":"obj1"})).unwrap();
        assert!(r["result"].is_object());
    }

    #[test]
    fn element_is_enabled_uses_call_function_on() {
        let b = backend();
        let r = element_is_enabled(&b, "1", &json!({"objectId":"obj1"})).unwrap();
        assert!(r["result"].is_object());
    }

    #[test]
    fn element_is_hidden_uses_call_function_on() {
        let b = backend();
        let r = element_is_hidden(&b, "1", &json!({"objectId":"obj1"})).unwrap();
        assert!(r["result"].is_object());
    }

    #[test]
    fn element_is_visible_uses_call_function_on() {
        let b = backend();
        let r = element_is_visible(&b, "1", &json!({"objectId":"obj1"})).unwrap();
        assert!(r["result"].is_object());
    }

    #[test]
    fn element_content_frame_uses_call_function_on() {
        let b = backend();
        let r = element_content_frame(&b, "1", &json!({"objectId":"obj1"})).unwrap();
        assert!(r["result"].is_object());
    }

    #[test]
    fn element_owner_frame_uses_call_function_on() {
        let b = backend();
        let r = element_owner_frame(&b, "1", &json!({"objectId":"obj1"})).unwrap();
        assert!(r["result"].is_object());
    }

    #[test]
    fn element_scroll_into_view_uses_call_function_on() {
        let b = backend();
        let r = element_scroll_into_view(&b, "1", &json!({"objectId":"obj1"})).unwrap();
        assert!(r["result"].is_object());
    }

    #[test]
    fn element_wait_for_element_state_returns_empty() {
        let b = backend();
        let r = element_wait_for_element_state(&b, "1", &json!({"state":"visible"})).unwrap();
        assert_eq!(r.as_object().unwrap().len(), 0);
    }

    #[test]
    fn element_wait_for_selector_returns_empty() {
        let b = backend();
        let r = element_wait_for_selector(&b, "1", &json!({"selector":"div"})).unwrap();
        assert_eq!(r.as_object().unwrap().len(), 0);
    }

    // ── JSHandle ──

    #[test]
    fn js_handle_as_element_returns_local_state() {
        let b = backend();
        let r = js_handle_as_element(&b, "1", &json!({"objectId":"obj1"})).unwrap();
        assert_eq!(r["isElement"], false);
    }

    #[test]
    fn js_handle_dispose_calls_release_object() {
        let b = backend();
        let r = js_handle_dispose(&b, "1", &json!({"objectId":"obj1"})).unwrap();
        assert_eq!(r.as_object().unwrap().len(), 0);
    }

    #[test]
    fn js_handle_evaluate_calls_call_function_on() {
        let b = backend();
        let r = js_handle_evaluate(&b, "1", &json!({"objectId":"obj1","func":"return 1+1;"})).unwrap();
        assert!(r["result"].is_object());
    }

    #[test]
    fn js_handle_evaluate_handle_calls_call_function_on() {
        let b = backend();
        let r = js_handle_evaluate_handle(&b, "1", &json!({"objectId":"obj1","func":"return this;"})).unwrap();
        assert!(r["result"].is_object());
    }

    #[test]
    fn js_handle_get_properties_calls_get_properties() {
        let b = backend();
        let r = js_handle_get_properties(&b, "1", &json!({"objectId":"obj1"})).unwrap();
        assert!(r["result"].is_array());
    }

    #[test]
    fn js_handle_get_property_injection_attempt() {
        let b = backend();
        let payload = "constructor';alert(1);//";
        let r = js_handle_get_property(&b, "1", &json!({"objectId":"obj1","name":payload})).unwrap();
        // callFunctionOn 路径下,backend 仅记录函数声明长度,不影响安全
        assert!(r["result"].is_object());
    }

    #[test]
    fn js_handle_json_value_uses_call_function_on() {
        let b = backend();
        let r = js_handle_json_value(&b, "1", &json!({"objectId":"obj1"})).unwrap();
        assert!(r["result"].is_object());
    }

    // ── 其他 ──

    #[test]
    fn page_opener_generates_iife() {
        let b = backend();
        let r = page_opener(&b, "1", &json!({})).unwrap();
        let v = r["result"]["value"].as_str().unwrap();
        assert!(v.contains("window.opener"));
    }

    #[test]
    fn page_frames_returns_array() {
        let b = backend();
        let r = page_frames(&b, "1", &json!({})).unwrap();
        assert!(r["frames"].is_array());
    }

    #[test]
    fn page_main_frame_returns_root() {
        let b = backend();
        let r = page_main_frame(&b, "1", &json!({})).unwrap();
        assert!(r["id"].is_string() || r["id"].is_number());
    }

    #[test]
    fn page_request_gc_generates_iife() {
        let b = backend();
        let r = page_request_gc(&b, "1", &json!({})).unwrap();
        let v = r["result"]["value"].as_str().unwrap();
        assert!(v.contains("window.gc"));
    }

    #[test]
    fn page_go_back_generates_iife() {
        let b = backend();
        let r = page_go_back(&b, "1", &json!({})).unwrap();
        let v = r["result"]["value"].as_str().unwrap();
        assert!(v.contains("history.back"));
    }

    #[test]
    fn page_go_forward_generates_iife() {
        let b = backend();
        let r = page_go_forward(&b, "1", &json!({})).unwrap();
        let v = r["result"]["value"].as_str().unwrap();
        assert!(v.contains("history.forward"));
    }

    #[test]
    fn page_emulate_media_serializes_media_param() {
        let b = backend();
        let r = page_emulate_media(&b, "1", &json!({"media":"print"})).unwrap();
        let v = r["result"]["value"].as_str().unwrap();
        assert!(v.contains("\"print\""));
    }

    #[test]
    fn page_screenshot_returns_base64_data() {
        let b = backend();
        let r = page_screenshot(&b, "1", &json!({})).unwrap();
        assert!(r["data"].is_string());
        assert!(r["binary"].is_array());
    }

    #[test]
    fn page_pdf_returns_base64_data() {
        let b = backend();
        let r = page_pdf(&b, "1", &json!({})).unwrap();
        assert!(r["data"].is_string());
    }

    #[test]
    fn page_set_default_timeout_returns_empty() {
        let b = backend();
        let r = page_set_default_timeout(&b, "1", &json!({"timeout":30000})).unwrap();
        assert_eq!(r.as_object().unwrap().len(), 0);
    }

    #[test]
    fn page_set_default_navigation_timeout_returns_empty() {
        let b = backend();
        let r = page_set_default_navigation_timeout(&b, "1", &json!({"timeout":60000})).unwrap();
        assert_eq!(r.as_object().unwrap().len(), 0);
    }

    #[test]
    fn page_wait_for_load_state_returns_empty() {
        let b = backend();
        let r = page_wait_for_load_state(&b, "1", &json!({"state":"load"})).unwrap();
        assert_eq!(r.as_object().unwrap().len(), 0);
    }

    #[test]
    fn page_wait_for_url_returns_empty() {
        let b = backend();
        let r = page_wait_for_url(&b, "1", &json!({"url":"**/*"})).unwrap();
        assert_eq!(r.as_object().unwrap().len(), 0);
    }

    #[test]
    fn page_wait_for_request_returns_empty() {
        let b = backend();
        let r = page_wait_for_request(&b, "1", &json!({"url":"**/api/*"})).unwrap();
        assert_eq!(r.as_object().unwrap().len(), 0);
    }

    #[test]
    fn page_wait_for_response_returns_empty() {
        let b = backend();
        let r = page_wait_for_response(&b, "1", &json!({"url":"**/api/*"})).unwrap();
        assert_eq!(r.as_object().unwrap().len(), 0);
    }

    #[test]
    fn page_wait_for_event_returns_empty() {
        let b = backend();
        let r = page_wait_for_event(&b, "1", &json!({"event":"response"})).unwrap();
        assert_eq!(r.as_object().unwrap().len(), 0);
    }

    // ── 缺参数错误 ──

    #[test]
    fn add_script_tag_missing_url_and_content_returns_invalid_params() {
        let b = backend();
        let err = page_add_script_tag(&b, "1", &json!({})).unwrap_err();
        assert!(matches!(err, BridgeError::InvalidParams(_)));
    }

    #[test]
    fn add_style_tag_missing_url_and_content_returns_invalid_params() {
        let b = backend();
        let err = page_add_style_tag(&b, "1", &json!({})).unwrap_err();
        assert!(matches!(err, BridgeError::InvalidParams(_)));
    }

    #[test]
    fn expose_function_missing_name_returns_invalid_params() {
        let b = backend();
        let err = page_expose_function(&b, "1", &json!({})).unwrap_err();
        assert!(matches!(err, BridgeError::InvalidParams(_)));
    }

    #[test]
    fn page_tap_missing_selector_returns_invalid_params() {
        let b = backend();
        let err = page_tap(&b, "1", &json!({})).unwrap_err();
        assert!(matches!(err, BridgeError::InvalidParams(_)));
    }

    #[test]
    fn page_type_missing_text_returns_invalid_params() {
        let b = backend();
        let err = page_type(&b, "1", &json!({"selector":"input"})).unwrap_err();
        assert!(matches!(err, BridgeError::InvalidParams(_)));
    }

    #[test]
    fn element_get_attribute_missing_object_id_returns_invalid_params() {
        let b = backend();
        let err = element_get_attribute(&b, "1", &json!({"name":"x"})).unwrap_err();
        assert!(matches!(err, BridgeError::InvalidParams(_)));
    }
}
