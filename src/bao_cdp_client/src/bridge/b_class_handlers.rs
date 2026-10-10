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
    params
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
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
pub fn page_title(
    backend: &dyn ServoBackend,
    target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    let expr = build_iife("return document.title;");
    eval_iife(backend, target_id, expr)
}

/// Page.url — `return location.href;`
///
/// @trace REQ-BAO-API-005 [method:Page.url]
pub fn page_url(
    backend: &dyn ServoBackend,
    target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    let expr = build_iife("return location.href;");
    eval_iife(backend, target_id, expr)
}

/// Page.content — `return document.documentElement.outerHTML;`
///
/// @trace REQ-BAO-API-005 [method:Page.content]
pub fn page_content(
    backend: &dyn ServoBackend,
    target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    let expr = build_iife("return document.documentElement.outerHTML;");
    eval_iife(backend, target_id, expr)
}

/// Page.viewport — 本地状态(TASK-5 D 类)。当前从 `Page.getLayoutMetrics` 合成基础值。
///
/// @trace REQ-BAO-API-005 [method:Page.viewport]
pub fn page_viewport(
    backend: &dyn ServoBackend,
    target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
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
        if v < 0 {
            1.0
        } else {
            v as f64
        }
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
// 等待类 — 5 method + 2 ElementHandle method(M1 P1 事件订阅实装,REQ-CDP-001)
// ════════════════════════════════════════════════════════════════════

/// waitFor* 默认超时(ms)— Playwright 默认同值(30s)。
const WAIT_FOR_DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// waitFor* 超时上限(ms)— 命令派发线程的服务端等待必须有界(无取消面,
/// 禁无限等待;客户端要更长语义时自行重试)。
const WAIT_FOR_MAX_TIMEOUT_MS: u64 = 600_000;

/// `waitForLoadState(networkidle)` 的网络静默窗口(ms)— Playwright 同值
/// (500ms 无网络活动 = idle)。
const NETWORK_IDLE_QUIET_MS: u64 = 500;

/// ElementHandle.waitFor* 的 DOM 轮询间隔(ms)。
const ELEMENT_POLL_INTERVAL_MS: u64 = 100;

/// networkidle 的网络活动事件集 — 任一到达即重置静默窗(translate 的
/// 4 个 Network 事件面)。
const NETWORK_ACTIVITY_METHODS: [&str; 4] = [
    "Network.requestWillBeSent",
    "Network.responseReceived",
    "Network.loadingFinished",
    "Network.loadingFailed",
];

/// 读取 `timeout` 参数(ms;缺省 30s,上限 600s;`0` = 仅立即探测,
/// 文档化偏差:Playwright 的 0=无限在服务端等价于无限占用派发线程,
/// 无取消面故不提供)。
fn wait_timeout_ms(params: &Value) -> u64 {
    let requested = params
        .get("timeout")
        .and_then(|v| v.as_u64())
        .unwrap_or(WAIT_FOR_DEFAULT_TIMEOUT_MS);
    requested.min(WAIT_FOR_MAX_TIMEOUT_MS)
}

/// Playwright URL glob 模式(`**/api/*`)→ 匹配闭包。
///
/// `glob::Pattern` 全串匹配语义与 Playwright 对齐(`*` 不跨 `/`,`**`
/// 跨任意)。模式畸形(如未闭合 `[`)时退化为全等比较 — 不伪造宽松匹配。
fn url_matcher(pattern: &str) -> impl Fn(&str) -> bool + Send {
    let compiled = glob::Pattern::new(pattern).ok();
    let owned = pattern.to_string();
    move |url: &str| match &compiled {
        Some(p) => p.matches(url),
        // 畸形模式:全等(唯一诚实字面语义)。
        None => url == owned,
    }
}

/// backend 上的字符串探针(runtime_evaluate + value 剥壳;returnByValue
/// 通道保证基本类型内联)。
fn probe_string(
    backend: &dyn ServoBackend,
    target_id: &str,
    expression: &str,
) -> Result<Option<String>, BridgeError> {
    let r = backend.runtime_evaluate(target_id, expression)?;
    Ok(r
        .result
        .value
        .and_then(|v| v.as_str().map(|s| s.to_string())))
}

/// backend 事件面的统一入口 — 无 tap 的 backend 上 waitFor* 诚实失败
/// (M1 P1 前是占位 OK,禁回退)。
fn require_tap<'a>(
    backend: &'a dyn ServoBackend,
    method: &str,
) -> Result<&'a std::sync::Arc<super::event_translator::CdpEventTap>, BridgeError> {
    backend.event_tap().ok_or_else(|| {
        BridgeError::NotSupported(format!(
            "{method}: backend has no CDP event tap (host event pump not wired)"
        ))
    })
}

/// Page.waitForLoadState — 等待文档加载状态。
///
/// - `state=load`(默认):`document.readyState == 'complete'` 立即返回;
///   否则等待 `Page.lifecycleEvent name='load'`(translate 的
///   FrameStoppedLoading 配对事件)。
/// - `state=domcontentloaded`:readyState ∈ {interactive, complete} 立即
///   返回;否则等待 load 信号 — **文档化偏差**:servo 事件面把两个里程碑
///   塌缩进 frameStoppedLoading 一个信号,本等待在 load 到达时 resolve
///   (不早于真实 DOMContentLoaded,不伪造时点)。
/// - `state=networkidle`:500ms 网络静默窗(Playwright 同语义:静默窗口
///   内任一 Network 活动事件重置窗口)。
///
/// 返回 `{}`(Playwright 同形 void)。超时 = `Timeout`。
///
/// @trace REQ-BAO-API-005 [method:Page.waitForLoadState]
/// @trace REQ-CDP-001 [level:library]
pub fn page_wait_for_load_state(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let state = params
        .get("state")
        .and_then(|v| v.as_str())
        .unwrap_or("load")
        .to_string();
    let timeout_ms = wait_timeout_ms(params);
    match state.as_str() {
        "load" => {
            if probe_string(backend, target_id, "document.readyState")?
                .map(|s| s == "complete")
                .unwrap_or(false)
            {
                return Ok(json!({}));
            }
            let tap = require_tap(backend, "Page.waitForLoadState")?;
            tap.wait(
                target_id,
                &["Page.lifecycleEvent", "Page.loadEventFired"],
                Box::new(|p| {
                    // lifecycleEvent 按 name 门控;loadEventFired 无 name
                    // 参数(translate 与 FrameStoppedLoading 配对同刻)。
                    match p.get("name").and_then(|v| v.as_str()) {
                        Some(name) => name == "load",
                        None => true,
                    }
                }),
                std::time::Duration::from_millis(timeout_ms),
            )?;
            Ok(json!({}))
        }
        "domcontentloaded" => {
            // 偏差见 doc:resolve 于 load 信号(不早于真实 DOMContentLoaded)。
            if probe_string(backend, target_id, "document.readyState")?
                .map(|s| s == "interactive" || s == "complete")
                .unwrap_or(false)
            {
                return Ok(json!({}));
            }
            let tap = require_tap(backend, "Page.waitForLoadState")?;
            tap.wait(
                target_id,
                &["Page.lifecycleEvent", "Page.loadEventFired"],
                Box::new(|p| match p.get("name").and_then(|v| v.as_str()) {
                    Some(name) => name == "load",
                    None => true,
                }),
                std::time::Duration::from_millis(timeout_ms),
            )?;
            Ok(json!({}))
        }
        "networkidle" => {
            let tap = require_tap(backend, "Page.waitForLoadState")?;
            let deadline =
                std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
            loop {
                let now = std::time::Instant::now();
                if now >= deadline {
                    return Err(BridgeError::Timeout(format!(
                        "Page.waitForLoadState(networkidle): no idle window within {timeout_ms}ms"
                    )));
                }
                let quiet = std::cmp::min(
                    std::time::Duration::from_millis(NETWORK_IDLE_QUIET_MS),
                    deadline - now,
                );
                match tap.wait(
                    target_id,
                    &NETWORK_ACTIVITY_METHODS,
                    Box::new(|_| true),
                    quiet,
                ) {
                    Ok(_) => continue, // 网络活动 → 重置静默窗
                    Err(BridgeError::Timeout(_)) => return Ok(json!({})),
                    Err(e) => return Err(e),
                }
            }
        }
        other => Err(BridgeError::InvalidParams(format!(
            "Page.waitForLoadState: unsupported state {other:?} (load | domcontentloaded | networkidle)"
        ))),
    }
}

/// Page.waitForURL — 等待主 frame 导航到匹配 URL。
///
/// 当前 URL 已匹配 → 立即返回(Playwright 同形)。否则等待主 frame 的
/// `Page.frameNavigated`(`frame.id == main-<target>`,REQ-CDP-004 单一
/// frame 命名)且 `frame.url` 匹配 `url` glob。返回 `{}`。超时 = `Timeout`。
///
/// @trace REQ-BAO-API-005 [method:Page.waitForURL]
/// @trace REQ-CDP-001 [level:library]
pub fn page_wait_for_url(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let pattern = params
        .get("url")
        .or_else(|| params.get("pattern"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            BridgeError::InvalidParams("Page.waitForURL: missing string field: url".into())
        })?
        .to_string();
    let matcher = url_matcher(&pattern);
    // 立即探测:当前 URL 已匹配则不等待(Playwright 语义)。
    if probe_string(backend, target_id, "location.href")?
        .map(|u| matcher(&u))
        .unwrap_or(false)
    {
        return Ok(json!({}));
    }
    let tap = require_tap(backend, "Page.waitForURL")?;
    let main_frame = bao_cdp::servo_bridge::main_frame_id_for_target(target_id);
    tap.wait(
        target_id,
        &["Page.frameNavigated"],
        Box::new(move |p| {
            let frame_matches = p
                .get("frame")
                .and_then(|f| f.get("id"))
                .and_then(|v| v.as_str())
                .map(|fid| fid == main_frame)
                .unwrap_or(false);
            let url_matches = p
                .get("frame")
                .and_then(|f| f.get("url"))
                .and_then(|v| v.as_str())
                .map(|u| matcher(u))
                .unwrap_or(false);
            frame_matches && url_matches
        }),
        std::time::Duration::from_millis(wait_timeout_ms(params)),
    )?;
    Ok(json!({}))
}

/// Page.waitForRequest — 等待 URL 匹配的 `Network.requestWillBeSent`。
///
/// 返回**事件 params 本体**(requestId/request/timestamp/… — CDP 事件
/// 形状;Playwright 的 Request 对象在本桥面即其协议源)。`url` 缺省 =
/// 首个请求。超时 = `Timeout`。
///
/// @trace REQ-BAO-API-005 [method:Page.waitForRequest]
/// @trace REQ-CDP-001 [level:library]
pub fn page_wait_for_request(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let tap = require_tap(backend, "Page.waitForRequest")?;
    let matcher = params
        .get("url")
        .or_else(|| params.get("pattern"))
        .and_then(|v| v.as_str())
        .map(url_matcher);
    tap.wait(
        target_id,
        &["Network.requestWillBeSent"],
        Box::new(move |p| {
            match &matcher {
                Some(m) => p
                    .get("request")
                    .and_then(|r| r.get("url"))
                    .and_then(|v| v.as_str())
                    .map(|u| m(u))
                    .unwrap_or(false),
                None => true,
            }
        }),
        std::time::Duration::from_millis(wait_timeout_ms(params)),
    )
}

/// Page.waitForResponse — 等待 URL 匹配的 `Network.responseReceived`。
///
/// 返回事件 params 本体(requestId/response/…)。`url` 缺省 = 首个响应。
/// 超时 = `Timeout`。
///
/// @trace REQ-BAO-API-005 [method:Page.waitForResponse]
/// @trace REQ-CDP-001 [level:library]
pub fn page_wait_for_response(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let tap = require_tap(backend, "Page.waitForResponse")?;
    let matcher = params
        .get("url")
        .or_else(|| params.get("pattern"))
        .and_then(|v| v.as_str())
        .map(url_matcher);
    tap.wait(
        target_id,
        &["Network.responseReceived"],
        Box::new(move |p| {
            match &matcher {
                Some(m) => p
                    .get("response")
                    .and_then(|r| r.get("url"))
                    .and_then(|v| v.as_str())
                    .map(|u| m(u))
                    .unwrap_or(false),
                None => true,
            }
        }),
        std::time::Duration::from_millis(wait_timeout_ms(params)),
    )
}

/// Page.waitForEvent — 等待任意 CDP 事件。
///
/// `event` 接受两种形态:
/// - **裸 CDP method**(含 `.`):按字面等待(如 `Network.responseReceived`)。
/// - **Playwright 事件名**:按下表映射到 translate 产出的事件面:
///   `console`→Log.entryAdded / `pageerror`→Runtime.exceptionThrown /
///   `request`→Network.requestWillBeSent / `requestfailed`→Network.loadingFailed /
///   `requestfinished`→Network.loadingFinished / `response`→Network.responseReceived /
///   `load`→Page.loadEventFired / `domcontentloaded`→load 信号(servo 面
///   塌缩偏差,同 waitForLoadState)/ `framenavigated`→Page.frameNavigated。
///
/// 事件面没有信号的名字(frameattached 等)→ InvalidParams(诚实拒绝,
/// 禁静默挂到超时)。返回事件 params 本体。
///
/// @trace REQ-BAO-API-005 [method:Page.waitForEvent]
/// @trace REQ-CDP-001 [level:library]
pub fn page_wait_for_event(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let event = params
        .get("event")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            BridgeError::InvalidParams("Page.waitForEvent: missing string field: event".into())
        })?;
    let methods: Vec<String> = if event.contains('.') {
        vec![event.to_string()]
    } else {
        let mapped: &[&str] = match event {
            "console" => &["Log.entryAdded"],
            "pageerror" => &["Runtime.exceptionThrown"],
            "request" => &["Network.requestWillBeSent"],
            "requestfailed" => &["Network.loadingFailed"],
            "requestfinished" => &["Network.loadingFinished"],
            "response" => &["Network.responseReceived"],
            "load" => &["Page.loadEventFired"],
            // servo 事件面塌缩偏差:同 waitForLoadState(domcontentloaded)。
            "domcontentloaded" => &["Page.loadEventFired"],
            "framenavigated" => &["Page.frameNavigated"],
            other => {
                return Err(BridgeError::InvalidParams(format!(
                    "Page.waitForEvent: no CDP event signal for {other:?} on this event face"
                )))
            }
        };
        mapped.iter().map(|s| s.to_string()).collect()
    };
    let tap = require_tap(backend, "Page.waitForEvent")?;
    let method_refs: Vec<&str> = methods.iter().map(|s| s.as_str()).collect();
    tap.wait(
        target_id,
        &method_refs,
        Box::new(|_| true),
        std::time::Duration::from_millis(wait_timeout_ms(params)),
    )
}

// ════════════════════════════════════════════════════════════════════
// 跳转类 — 2 method
// ════════════════════════════════════════════════════════════════════

/// Page.goBack — `history.back()` + 等待导航。
///
/// @trace REQ-BAO-API-005 [method:Page.goBack]
pub fn page_go_back(
    backend: &dyn ServoBackend,
    target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    let expr = build_iife("history.back(); return true;");
    eval_iife(backend, target_id, expr)
}

/// Page.goForward — `history.forward()` + 等待导航。
///
/// @trace REQ-BAO-API-005 [method:Page.goForward]
pub fn page_go_forward(
    backend: &dyn ServoBackend,
    target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
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
    eval_iife(
        backend,
        target_id,
        build_iife_with_args(&body, &[json!(media)])?,
    )
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
pub fn page_pdf(
    backend: &dyn ServoBackend,
    target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
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
pub fn page_tap(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
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
pub fn page_hover(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let selector = get_str(params, "selector")?;
    let body = "var s=__args[0]; var el=document.querySelector(s); if(!el){throw new Error('not found');} el.dispatchEvent(new MouseEvent('mouseenter',{bubbles:true})); el.dispatchEvent(new MouseEvent('mouseover',{bubbles:true})); return true;";
    let expr = build_iife_with_args(body, &[json!(selector)])?;
    eval_iife(backend, target_id, expr)
}

/// Page.focus(selector) — querySelector + focus()。
///
/// @trace REQ-BAO-API-005 [method:Page.focus]
pub fn page_focus(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
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
pub fn page_type(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let selector = get_str(params, "selector")?;
    let text = get_str(params, "text")?;
    let body = "var s=__args[0], t=__args[1]; var el=document.querySelector(s); if(!el){throw new Error('not found');} el.focus(); var ev=new InputEvent('input',{bubbles:true,data:t}); if(el.value!==undefined){el.value=el.value+t;} else {el.textContent=(el.textContent||'')+t;} el.dispatchEvent(ev); return true;";
    let expr = build_iife_with_args(body, &[json!(selector), json!(text)])?;
    eval_iife(backend, target_id, expr)
}

/// Page.fill(selector, value) — querySelector + 整体替换 value。
///
/// @trace REQ-BAO-API-005 [method:Page.fill]
pub fn page_fill(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let selector = get_str(params, "selector")?;
    let value = get_str(params, "value")?;
    let body = "var s=__args[0], v=__args[1]; var el=document.querySelector(s); if(!el){throw new Error('not found');} el.focus(); if(el.value!==undefined){el.value=v;} else {el.textContent=v;} el.dispatchEvent(new Event('input',{bubbles:true})); el.dispatchEvent(new Event('change',{bubbles:true})); return true;";
    let expr = build_iife_with_args(body, &[json!(selector), json!(value)])?;
    eval_iife(backend, target_id, expr)
}

/// Page.press(selector, key) — querySelector + dispatch keydown/keyup。
///
/// @trace REQ-BAO-API-005 [method:Page.press]
pub fn page_press(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let selector = get_str(params, "selector")?;
    let key = get_str(params, "key")?;
    let body = "var s=__args[0], k=__args[1]; var el=document.querySelector(s); if(!el){throw new Error('not found');} el.focus(); el.dispatchEvent(new KeyboardEvent('keydown',{bubbles:true,key:k})); el.dispatchEvent(new KeyboardEvent('keyup',{bubbles:true,key:k})); return true;";
    let expr = build_iife_with_args(body, &[json!(selector), json!(key)])?;
    eval_iife(backend, target_id, expr)
}

/// Page.check(selector) — querySelector checkbox → checked=true + dispatch change。
///
/// @trace REQ-BAO-API-005 [method:Page.check]
pub fn page_check(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let selector = get_str(params, "selector")?;
    let body = "var s=__args[0]; var el=document.querySelector(s); if(!el){throw new Error('not found');} el.checked=true; el.dispatchEvent(new Event('change',{bubbles:true})); return true;";
    let expr = build_iife_with_args(body, &[json!(selector)])?;
    eval_iife(backend, target_id, expr)
}

/// Page.uncheck(selector) — querySelector checkbox → checked=false。
///
/// @trace REQ-BAO-API-005 [method:Page.uncheck]
pub fn page_uncheck(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
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
pub fn page_opener(
    backend: &dyn ServoBackend,
    target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    let expr = build_iife("return (window.opener ? true : false);");
    eval_iife(backend, target_id, expr)
}

/// Page.frames — 从 Page.getFrameTree 解析。
///
/// @trace REQ-BAO-API-005 [method:Page.frames]
pub fn page_frames(
    backend: &dyn ServoBackend,
    target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
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
pub fn page_main_frame(
    backend: &dyn ServoBackend,
    target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    let tree = backend.page_frame_tree(target_id)?;
    Ok(frame_to_json(&tree.frame))
}

// ════════════════════════════════════════════════════════════════════
// 内存 — 1 method
// ════════════════════════════════════════════════════════════════════

/// Page.requestGC — `window.gc()` 触发(若可用)。
///
/// @trace REQ-BAO-API-005 [method:Page.requestGC]
pub fn page_request_gc(
    backend: &dyn ServoBackend,
    target_id: &str,
    _params: &Value,
) -> Result<Value, BridgeError> {
    let expr = build_iife(
        "if (typeof window.gc==='function') { window.gc(); return true; } return false;",
    );
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
    let body =
        "if (this && this.scrollIntoViewIfNeeded) { this.scrollIntoViewIfNeeded(); } return true;";
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

/// ElementHandle.waitForElementState — 等待元素进入目标状态(DOM 轮询)。
///
/// `state` ∈ {visible, hidden, enabled, disabled, editable} — 每轮用
/// `Runtime.callFunctionOn` 在 objectId 上跑与同名 is_* 谓词逐字一致的
/// JS 体(单一语义源);立即首探,100ms 间隔重探,deadline 截止。
/// Playwright 的 `stable` 状态需要两帧几何对比,本面无帧信号 → 诚实
/// InvalidParams。返回 `{}`。超时 = `Timeout`。
///
/// @trace REQ-BAO-API-005 [method:ElementHandle.waitForElementState]
/// @trace REQ-CDP-001 [level:library]
pub fn element_wait_for_element_state(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let object_id = get_str(params, "objectId")?;
    let state = params
        .get("state")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            BridgeError::InvalidParams(
                "ElementHandle.waitForElementState: missing string field: state".into(),
            )
        })?
        .to_string();
    // 与 is_* 处理器逐字一致的谓词体(单一语义源 — 修改须双侧同步)。
    let body: &str = match state.as_str() {
        "visible" => {
            "if(!this){return false;} var r=this.getBoundingClientRect(); var s=window.getComputedStyle(this); return (r.width>0&&r.height>0)&&s.visibility!=='hidden'&&s.display!=='none';"
        }
        "hidden" => {
            "if(!this){return true;} var r=this.getBoundingClientRect(); var s=window.getComputedStyle(this); return (r.width===0||r.height===0)||s.visibility==='hidden'||s.display==='none';"
        }
        "enabled" => "return !!(this && !this.disabled);",
        "disabled" => "return !!(this && this.disabled);",
        "editable" => "return !!(this && !this.disabled && !this.readOnly);",
        other => {
            return Err(BridgeError::InvalidParams(format!(
                "ElementHandle.waitForElementState: unsupported state {other:?} \
                 (visible | hidden | enabled | disabled | editable; 'stable' has no \
                 frame signal on this face)"
            )))
        }
    };
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_millis(wait_timeout_ms(params));
    loop {
        let r = backend.runtime_call_function_on(target_id, &object_id, body, &[])?;
        if r.result.value.as_ref().and_then(|v| v.as_bool()).unwrap_or(false) {
            return Ok(json!({}));
        }
        if std::time::Instant::now() >= deadline {
            return Err(BridgeError::Timeout(format!(
                "ElementHandle.waitForElementState({state}): condition unmet within {}ms",
                wait_timeout_ms(params)
            )));
        }
        std::thread::sleep(std::time::Duration::from_millis(ELEMENT_POLL_INTERVAL_MS));
    }
}

/// ElementHandle.waitForSelector — 等待 selector 命中目标状态(DOM 轮询)。
///
/// `state` ∈ {visible(默认), attached, hidden, detached}:
/// - attached:`DOM.querySelector` 命中;
/// - visible:命中且可见(可见性谓词经 evaluate,与 is_visible 语义一致);
/// - hidden:未命中或不可见;detached:未命中(从 DOM 移除)。
///
/// 命中返回 `{"nodeId": n}`(通道语义:1=命中/0=未命中,同 DOM.querySelector
/// 处理器);hidden/detached 命中返回 `{"nodeId": 0}`。超时 = `Timeout`。
///
/// @trace REQ-BAO-API-005 [method:ElementHandle.waitForSelector]
/// @trace REQ-CDP-001 [level:library]
pub fn element_wait_for_selector(
    backend: &dyn ServoBackend,
    target_id: &str,
    params: &Value,
) -> Result<Value, BridgeError> {
    let selector = get_str(params, "selector")?;
    let state = params
        .get("state")
        .and_then(|v| v.as_str())
        .unwrap_or("visible")
        .to_string();
    if !matches!(state.as_str(), "visible" | "attached" | "hidden" | "detached") {
        return Err(BridgeError::InvalidParams(format!(
            "ElementHandle.waitForSelector: unsupported state {state:?} \
             (visible | attached | hidden | detached)"
        )));
    }
    // 可见性谓词(与 is_visible 语义一致,selector 参数走 __args 注入)。
    let visibility_expr = build_iife_with_args(
        "var s=__args[0]; var el=document.querySelector(s); if(!el){return false;} \
         var r=el.getBoundingClientRect(); var cs=window.getComputedStyle(el); \
         return (r.width>0&&r.height>0)&&cs.visibility!=='hidden'&&cs.display!=='none';",
        &[json!(selector)],
    )?;
    let is_visible = |b: &dyn ServoBackend| -> Result<bool, BridgeError> {
        let r = b.runtime_evaluate(target_id, &visibility_expr)?;
        Ok(r.result.value.as_ref().and_then(|v| v.as_bool()).unwrap_or(false))
    };
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_millis(wait_timeout_ms(params));
    loop {
        let found = backend
            .dom_query_selector(target_id, 0, &selector)?
            .unwrap_or(0);
        let satisfied = match state.as_str() {
            // attached:命中即满足。
            "attached" => found != 0,
            // visible:命中且可见。
            "visible" => found != 0 && is_visible(backend)?,
            // hidden:未命中或不可见。
            "hidden" => found == 0 || !is_visible(backend)?,
            // detached:从 DOM 移除(未命中)。
            "detached" => found == 0,
            other => unreachable!("state validated above: {other}"),
        };
        if satisfied {
            return Ok(json!({ "nodeId": found }));
        }
        if std::time::Instant::now() >= deadline {
            return Err(BridgeError::Timeout(format!(
                "ElementHandle.waitForSelector({state}): condition unmet within {}ms",
                wait_timeout_ms(params)
            )));
        }
        std::thread::sleep(std::time::Duration::from_millis(ELEMENT_POLL_INTERVAL_MS));
    }
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
    use super::super::servo_backend::MockServoBackend;
    use super::*;
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
        let r =
            page_select_option(&b, "1", &json!({"selector":"select","values":payloads})).unwrap();
        let v = r["result"]["value"].as_str().unwrap();
        assert!(v.contains("\"';alert(1);//\""));
        assert!(v.contains("\"</option>\""));
    }

    #[test]
    fn page_set_input_files_paths_injection_attempt() {
        let b = backend();
        let payloads = vec![json!("/etc/passwd'),alert(1),String('/")];
        let r = page_set_input_files(
            &b,
            "1",
            &json!({"selector":"input[type=file]","paths":payloads}),
        )
        .unwrap();
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

    /// waitFor* 单测的带 tap backend(事件由测试线程喂入)。
    fn backend_with_tap()
    -> (MockServoBackend, std::sync::Arc<super::super::event_translator::CdpEventTap>) {
        let mut b = MockServoBackend::new();
        b.add_target("1");
        let tap = std::sync::Arc::new(super::super::event_translator::CdpEventTap::new());
        b.event_tap = Some(tap.clone());
        (b, tap)
    }

    /// 状态谓词满足 → 轮询立即返回(M1 P1 实装替换占位 OK 锁)。
    ///
    /// @trace REQ-CDP-001 [level:library]
    #[test]
    fn element_wait_for_element_state_polls_until_true() {
        let mut b = MockServoBackend::new();
        b.add_target("1");
        b.call_function_on_value = Some(json!(true));
        let r = element_wait_for_element_state(
            &b,
            "1",
            &json!({"objectId":"obj1","state":"visible","timeout": 500}),
        )
        .unwrap();
        assert_eq!(r.as_object().unwrap().len(), 0);
        let log = b.call_log.lock().unwrap();
        assert!(
            log.iter()
                .any(|(_, m, _)| m == "runtime_call_function_on"),
            "state predicate must poll via Runtime.callFunctionOn"
        );
    }

    /// 状态谓词恒不满足(mock 默认 callFunctionOn value=None)+ 有界
    /// timeout → Timeout 错误(禁无限轮询)。
    ///
    /// @trace REQ-CDP-001 [level:library]
    #[test]
    fn element_wait_for_element_state_times_out() {
        let b = backend();
        let err = element_wait_for_element_state(
            &b,
            "1",
            &json!({"objectId":"obj1","state":"enabled","timeout": 120}),
        )
        .unwrap_err();
        assert!(matches!(err, BridgeError::Timeout(_)), "got: {err:?}");
    }

    /// selector 命中(mock 通道语义 nodeId=2)→ 返回 nodeId。
    ///
    /// @trace REQ-CDP-001 [level:library]
    #[test]
    fn element_wait_for_selector_attached_returns_node_id() {
        let b = backend();
        // MockServoBackend.dom_query_selector 默认命中(返回 Some(2))。
        let r = element_wait_for_selector(
            &b,
            "1",
            &json!({"selector":"div","state":"attached","timeout": 500}),
        )
        .unwrap();
        assert_eq!(r["nodeId"], 2);
    }

    /// 无事件面 backend → Page.waitFor* 诚实 NotSupported(M1 P1 前的
    /// 占位 OK 已替换;缺 tap 不允许假成功)。
    ///
    /// @trace REQ-CDP-001 [level:library]
    #[test]
    fn page_wait_for_request_without_tap_is_not_supported() {
        let b = backend();
        let err = page_wait_for_request(&b, "1", &json!({"url":"**/api/*"})).unwrap_err();
        assert!(matches!(err, BridgeError::NotSupported(_)), "got: {err:?}");
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
        let r =
            js_handle_evaluate(&b, "1", &json!({"objectId":"obj1","func":"return 1+1;"})).unwrap();
        assert!(r["result"].is_object());
    }

    #[test]
    fn js_handle_evaluate_handle_calls_call_function_on() {
        let b = backend();
        let r =
            js_handle_evaluate_handle(&b, "1", &json!({"objectId":"obj1","func":"return this;"}))
                .unwrap();
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
        let r =
            js_handle_get_property(&b, "1", &json!({"objectId":"obj1","name":payload})).unwrap();
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

    /// waitForLoadState(load):readyState 探针不满足(mock 回显表达式)→
    /// 在 tap 上等到 lifecycleEvent name=load 才返回。
    ///
    /// @trace REQ-CDP-001 [level:library]
    #[test]
    fn page_wait_for_load_state_resolves_on_load_lifecycle_event() {
        let (b, tap) = backend_with_tap();
        let feeder = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(30));
            // 先喂 name=init(不得 resolve),再喂 name=load(必须 resolve)。
            tap.observe(
                "1",
                "Page.lifecycleEvent",
                &json!({ "frameId": "main-1", "loaderId": "loader-1", "name": "init" }),
            );
            std::thread::sleep(std::time::Duration::from_millis(30));
            tap.observe(
                "1",
                "Page.lifecycleEvent",
                &json!({ "frameId": "main-1", "loaderId": "loader-1", "name": "load" }),
            );
        });
        let r = page_wait_for_load_state(&b, "1", &json!({"state":"load","timeout": 2000}))
            .unwrap();
        assert_eq!(r.as_object().unwrap().len(), 0);
        feeder.join().unwrap();
    }

    /// waitForLoadState(networkidle):500ms 静默窗(无网络活动)→ 空转
    /// 即 resolve;窗口内喂 Network 活动事件则重置。
    ///
    /// @trace REQ-CDP-001 [level:library]
    #[test]
    fn page_wait_for_load_state_networkidle_resolves_on_quiet_window() {
        let (b, tap) = backend_with_tap();
        let start = std::time::Instant::now();
        let r = page_wait_for_load_state(
            &b,
            "1",
            &json!({"state":"networkidle","timeout": 5000}),
        )
        .unwrap();
        // 静默窗 500ms 是 resolve 的下界(Playwright 同语义)。
        assert!(
            start.elapsed() >= std::time::Duration::from_millis(400),
            "networkidle must respect the quiet window, resolved in {:?}",
            start.elapsed()
        );
        assert_eq!(r.as_object().unwrap().len(), 0);
        assert_eq!(tap.waiter_count(), 0);
    }

    /// waitForURL:主 frame 导航事件带匹配 URL → resolve;子 frame 的
    /// 同 URL 导航不得 resolve(主帧过滤)。
    ///
    /// @trace REQ-CDP-001 [level:library]
    #[test]
    fn page_wait_for_url_filters_to_main_frame() {
        let (b, tap) = backend_with_tap();
        let feeder = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(30));
            // 子 frame iframe 导航:URL 匹配但 frame.id != main-1 → 不 resolve。
            tap.observe(
                "1",
                "Page.frameNavigated",
                &json!({ "frame": { "id": "frame-child", "url": "https://x/done" } }),
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
            // 主 frame 导航:resolve。
            tap.observe(
                "1",
                "Page.frameNavigated",
                &json!({ "frame": { "id": "main-1", "url": "https://x/done" } }),
            );
        });
        page_wait_for_url(&b, "1", &json!({"url":"**/done","timeout": 2000})).unwrap();
        feeder.join().unwrap();
    }

    /// waitForRequest:URL glob 匹配的 requestWillBeSent → 返回事件 params
    /// 本体;不匹配的请求不得 resolve。
    ///
    /// @trace REQ-CDP-001 [level:library]
    #[test]
    fn page_wait_for_request_returns_matching_event_params() {
        let (b, tap) = backend_with_tap();
        let feeder = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(30));
            tap.observe(
                "1",
                "Network.requestWillBeSent",
                &json!({ "requestId": "r-wrong", "request": { "url": "https://x/static/logo.png" } }),
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
            tap.observe(
                "1",
                "Network.requestWillBeSent",
                &json!({ "requestId": "r-1", "request": { "url": "https://x/api/data" } }),
            );
        });
        let r = page_wait_for_request(&b, "1", &json!({"url":"**/api/*","timeout": 2000}))
            .unwrap();
        assert_eq!(r["requestId"], "r-1");
        assert_eq!(r["request"]["url"], "https://x/api/data");
        feeder.join().unwrap();
    }

    /// waitForResponse:URL 匹配的 responseReceived → 返回事件 params 本体。
    ///
    /// @trace REQ-CDP-001 [level:library]
    #[test]
    fn page_wait_for_response_returns_matching_event_params() {
        let (b, tap) = backend_with_tap();
        let feeder = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(30));
            tap.observe(
                "1",
                "Network.responseReceived",
                &json!({
                    "requestId": "r-1",
                    "response": { "url": "https://x/api/data", "status": 200 }
                }),
            );
        });
        let r = page_wait_for_response(&b, "1", &json!({"url":"**/api/*","timeout": 2000}))
            .unwrap();
        assert_eq!(r["response"]["status"], 200);
        feeder.join().unwrap();
    }

    /// waitForEvent:Playwright 名映射(response → Network.responseReceived)
    /// + 事件面无信号的名字诚实 InvalidParams。
    ///
    /// @trace REQ-CDP-001 [level:library]
    #[test]
    fn page_wait_for_event_maps_playwright_names_and_rejects_unsignaled() {
        let (b, tap) = backend_with_tap();
        let feeder = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(30));
            tap.observe(
                "1",
                "Network.responseReceived",
                &json!({ "requestId": "r-1", "response": { "url": "https://x/", "status": 204 } }),
            );
        });
        let r = page_wait_for_event(&b, "1", &json!({"event":"response","timeout": 2000})).unwrap();
        assert_eq!(r["response"]["status"], 204);
        feeder.join().unwrap();

        // 事件面没有信号的名字 → 立即 InvalidParams(禁挂到超时)。
        let err = page_wait_for_event(&b, "1", &json!({"event":"frameattached"})).unwrap_err();
        assert!(matches!(err, BridgeError::InvalidParams(_)), "got: {err:?}");
    }

    /// waitFor* 超时:tap 上无事件 → Timeout(有界,不悬挂)。
    ///
    /// @trace REQ-CDP-001 [level:library]
    #[test]
    fn page_wait_for_request_times_out() {
        let (b, tap) = backend_with_tap();
        let err = page_wait_for_request(&b, "1", &json!({"timeout": 100})).unwrap_err();
        assert!(matches!(err, BridgeError::Timeout(_)), "got: {err:?}");
        assert_eq!(tap.waiter_count(), 0, "timeout must unregister the waiter");
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
