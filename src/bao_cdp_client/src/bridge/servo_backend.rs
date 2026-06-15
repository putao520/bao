//! `ServoBackend` trait — servo 调用抽象层。
//!
//! # 设计动机
//!
//! servo `WebView` 是 `!Send + !Sync`(内部 Rc/RefCell),无法直接被
//! `Arc<dyn InMemoryBridge + Send + Sync>` 持有。引入 `ServoBackend` trait
//! 作为桥接抽象:
//!
//! - **`ServoBackend` 自身 `Send + Sync`**:CDPRdpBridge 可安全持有 `Arc<dyn ServoBackend>`
//! - **具体实现**:可基于 PagePool(同进程)、crossbeam channel(servo 线程)、
//!   或测试 mock。本文件提供 [`MockServoBackend`] 用于单元测试。
//! - **TARGET-ID 抽象**:用 `&str` 标识 Page(对应 CDP `Target.targetId` 或
//!   session_id),内部由实现决定如何映射到 PagePool::get_page / 远端 target
//!
//! # A 类 48 method 的映射
//!
//! 详见 [`command_dispatcher`](super::command_dispatcher) 与 [`handlers`](super::a_class_handlers)。
//! 每个 A 类 method 拆为 "参数解析 + backend 调用 + 响应构造":
//! - 参数解析:在 `a_class_handlers.rs`,从 `serde_json::Value` 抽字段
//! - backend 调用:本 trait 的对应方法
//! - 响应构造:在 handler 内组装 CDP-compatible JSON
//!
//! @trace REQ-BAO-API-004 [level:library]

use std::sync::Arc;
use std::sync::Mutex;

use serde_json::Value;

use super::error::BridgeError;

/// 单个 DOM 节点的简略描述(用于 DOM.* 命令响应)。
///
/// @trace REQ-BAO-API-004 [domain:DOM]
#[derive(Debug, Clone, Default)]
pub struct NodeDescriptor {
    pub node_id: i64,
    pub node_name: String,
    pub node_value: String,
    pub backend_node_id: i64,
    pub children: Vec<NodeDescriptor>,
}

/// 截图格式(对应 CDP `Page.captureScreenshot.format`)。
///
/// @trace REQ-BAO-API-004 [domain:Page]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BridgeScreenshotFormat {
    Jpeg,
    Png,
    Webp,
}

impl BridgeScreenshotFormat {
    /// 从 CDP format 字符串解析,默认 Png。
    ///
    /// @trace REQ-BAO-API-004 [domain:Page]
    pub fn from_cdp(s: Option<&str>) -> Self {
        match s {
            Some("jpeg") => Self::Jpeg,
            Some("webp") => Self::Webp,
            _ => Self::Png,
        }
    }
}

/// servo 后端操作抽象。
///
/// 实现必须保证 `Send + Sync`(可被 `Arc<dyn ServoBackend>` 跨线程持有)。
///
/// 所有方法接收 `target_id: &str`(CDP Target.targetId 或 session_id),
/// 实现内部决定如何映射到 PagePool/PageHandle/远端 target。
///
/// @trace REQ-BAO-API-004 [level:library]
pub trait ServoBackend: Send + Sync {
    // ──────────────────────────────────────────────────────────────
    // Page domain — 11 method (A 类机械映射)
    // ──────────────────────────────────────────────────────────────

    /// Page.navigate — 加载 URL。
    ///
    /// @trace REQ-BAO-API-004 [domain:Page]
    fn page_navigate(&self, target_id: &str, url: &str) -> Result<NavigateResult, BridgeError>;

    /// Page.reload — 重新加载当前页面。
    ///
    /// @trace REQ-BAO-API-004 [domain:Page]
    fn page_reload(&self, target_id: &str) -> Result<(), BridgeError>;

    /// Page.captureScreenshot — 截屏。
    ///
    /// @trace REQ-BAO-API-004 [domain:Page]
    fn page_screenshot(
        &self,
        target_id: &str,
        format: BridgeScreenshotFormat,
    ) -> Result<Vec<u8>, BridgeError>;

    /// Page.getFrameTree — 返回 frame 树。
    ///
    /// @trace REQ-BAO-API-004 [domain:Page]
    fn page_frame_tree(&self, target_id: &str) -> Result<FrameTree, BridgeError>;

    /// Page.getNavigationHistory — 返回导航历史。
    ///
    /// @trace REQ-BAO-API-004 [domain:Page]
    fn page_navigation_history(&self, target_id: &str) -> Result<NavigationHistory, BridgeError>;

    /// Page.navigateToHistoryEntry — 跳转历史条目。
    ///
    /// @trace REQ-BAO-API-004 [domain:Page]
    fn page_navigate_to_history_entry(
        &self,
        target_id: &str,
        entry_id: i64,
    ) -> Result<(), BridgeError>;

    /// Page.setContent — 设置 HTML 内容。
    ///
    /// @trace REQ-BAO-API-004 [domain:Page]
    fn page_set_content(&self, target_id: &str, html: &str) -> Result<(), BridgeError>;

    /// Page.close — 关闭页面。
    ///
    /// @trace REQ-BAO-API-004 [domain:Page]
    fn page_close(&self, target_id: &str) -> Result<(), BridgeError>;

    /// Page.bringToFront — 把页面带到前台(No-op for headless)。
    ///
    /// @trace REQ-BAO-API-004 [domain:Page]
    fn page_bring_to_front(&self, target_id: &str) -> Result<(), BridgeError>;

    /// Page.getLayoutMetrics — 返回布局度量。
    ///
    /// @trace REQ-BAO-API-004 [domain:Page]
    fn page_layout_metrics(&self, target_id: &str) -> Result<LayoutMetrics, BridgeError>;

    /// Page.printToPDF — 打印为 PDF(返回 PDF bytes;servo 无 PDF 渲染,实现可返回空)。
    ///
    /// @trace REQ-BAO-API-004 [domain:Page]
    fn page_print_to_pdf(&self, target_id: &str) -> Result<Vec<u8>, BridgeError>;

    // ──────────────────────────────────────────────────────────────
    // Runtime domain — 6 method
    // ──────────────────────────────────────────────────────────────

    /// Runtime.evaluate — 执行 JS 表达式。
    ///
    /// 注意:本方法在 A 类内仅服务于部分 Runtime 命令。B 类高层 API
    /// (`Page.title` / `Page.url` 等)在 TASK-3b 通过 eval_synthesizer 实现。
    ///
    /// @trace REQ-BAO-API-004 [domain:Runtime]
    fn runtime_evaluate(&self, target_id: &str, expression: &str)
        -> Result<EvaluateResult, BridgeError>;

    /// Runtime.callFunctionOn — 在指定对象上调用函数。
    ///
    /// @trace REQ-BAO-API-004 [domain:Runtime]
    fn runtime_call_function_on(
        &self,
        target_id: &str,
        object_id: &str,
        function_declaration: &str,
        args: &[Value],
    ) -> Result<EvaluateResult, BridgeError>;

    /// Runtime.getProperties — 返回 RemoteObject 的属性。
    ///
    /// @trace REQ-BAO-API-004 [domain:Runtime]
    fn runtime_get_properties(
        &self,
        target_id: &str,
        object_id: &str,
        own_properties: bool,
    ) -> Result<Vec<PropertyDescriptor>, BridgeError>;

    /// Runtime.releaseObject — 释放 RemoteObject。
    ///
    /// @trace REQ-BAO-API-004 [domain:Runtime]
    fn runtime_release_object(&self, target_id: &str, object_id: &str) -> Result<(), BridgeError>;

    /// Runtime.enable — 启用 Runtime 域(状态切换,no-op for backend)。
    ///
    /// @trace REQ-BAO-API-004 [domain:Runtime]
    fn runtime_enable(&self, target_id: &str) -> Result<(), BridgeError>;

    /// Runtime.disable — 禁用 Runtime 域。
    ///
    /// @trace REQ-BAO-API-004 [domain:Runtime]
    fn runtime_disable(&self, target_id: &str) -> Result<(), BridgeError>;

    // ──────────────────────────────────────────────────────────────
    // DOM domain — 11 method
    // ──────────────────────────────────────────────────────────────

    /// DOM.getDocument — 返回 root document 节点。
    ///
    /// @trace REQ-BAO-API-004 [domain:DOM]
    fn dom_get_document(&self, target_id: &str, depth: i64) -> Result<NodeDescriptor, BridgeError>;

    /// DOM.querySelector — CSS selector 查询第一个匹配节点。
    ///
    /// @trace REQ-BAO-API-004 [domain:DOM]
    fn dom_query_selector(
        &self,
        target_id: &str,
        node_id: i64,
        selector: &str,
    ) -> Result<Option<i64>, BridgeError>;

    /// DOM.querySelectorAll — CSS selector 查询所有匹配节点。
    ///
    /// @trace REQ-BAO-API-004 [domain:DOM]
    fn dom_query_selector_all(
        &self,
        target_id: &str,
        node_id: i64,
        selector: &str,
    ) -> Result<Vec<i64>, BridgeError>;

    /// DOM.getBoxModel — 返回节点 box model。
    ///
    /// @trace REQ-BAO-API-004 [domain:DOM]
    fn dom_get_box_model(&self, target_id: &str, node_id: i64) -> Result<BoxModel, BridgeError>;

    /// DOM.resolveNode — 解析节点为 RemoteObject。
    ///
    /// @trace REQ-BAO-API-004 [domain:DOM]
    fn dom_resolve_node(
        &self,
        target_id: &str,
        backend_node_id: i64,
    ) -> Result<RemoteObject, BridgeError>;

    /// DOM.describeNode — 描述节点结构。
    ///
    /// @trace REQ-BAO-API-004 [domain:DOM]
    fn dom_describe_node(
        &self,
        target_id: &str,
        node_id: i64,
        depth: i64,
    ) -> Result<NodeDescriptor, BridgeError>;

    /// DOM.setAttributeValue — 设置属性。
    ///
    /// @trace REQ-BAO-API-004 [domain:DOM]
    fn dom_set_attribute(
        &self,
        target_id: &str,
        node_id: i64,
        name: &str,
        value: &str,
    ) -> Result<(), BridgeError>;

    /// DOM.removeAttribute — 移除属性。
    ///
    /// @trace REQ-BAO-API-004 [domain:DOM]
    fn dom_remove_attribute(
        &self,
        target_id: &str,
        node_id: i64,
        name: &str,
    ) -> Result<(), BridgeError>;

    /// DOM.getOuterHTML — 返回 outerHTML 字符串。
    ///
    /// @trace REQ-BAO-API-004 [domain:DOM]
    fn dom_get_outer_html(&self, target_id: &str, node_id: i64) -> Result<String, BridgeError>;

    /// DOM.setOuterHTML — 替换 outerHTML。
    ///
    /// @trace REQ-BAO-API-004 [domain:DOM]
    fn dom_set_outer_html(
        &self,
        target_id: &str,
        node_id: i64,
        html: &str,
    ) -> Result<(), BridgeError>;

    /// DOM.requestNode — 请求节点(展开/确保可用)。
    ///
    /// @trace REQ-BAO-API-004 [domain:DOM]
    fn dom_request_node(&self, target_id: &str, object_id: &str) -> Result<i64, BridgeError>;

    // ──────────────────────────────────────────────────────────────
    // Network domain — 4 method
    // ──────────────────────────────────────────────────────────────

    /// Network.enable — 启用 Network 域。
    ///
    /// @trace REQ-BAO-API-004 [domain:Network]
    fn network_enable(&self, target_id: &str) -> Result<(), BridgeError>;

    /// Network.disable — 禁用 Network 域。
    ///
    /// @trace REQ-BAO-API-004 [domain:Network]
    fn network_disable(&self, target_id: &str) -> Result<(), BridgeError>;

    /// Network.getResponseBody — 获取请求体。
    ///
    /// @trace REQ-BAO-API-004 [domain:Network]
    fn network_get_response_body(
        &self,
        target_id: &str,
        request_id: &str,
    ) -> Result<ResponseBody, BridgeError>;

    /// Network.setCacheDisabled — 禁用/启用缓存。
    ///
    /// @trace REQ-BAO-API-004 [domain:Network]
    fn network_set_cache_disabled(&self, target_id: &str, disabled: bool)
        -> Result<(), BridgeError>;

    // ──────────────────────────────────────────────────────────────
    // Input domain — 4 method
    // ──────────────────────────────────────────────────────────────

    /// Input.dispatchMouseEvent — 派发鼠标事件。
    ///
    /// @trace REQ-BAO-API-004 [domain:Input]
    fn input_dispatch_mouse_event(
        &self,
        target_id: &str,
        event: MouseEvent,
    ) -> Result<(), BridgeError>;

    /// Input.dispatchKeyEvent — 派发键盘事件。
    ///
    /// @trace REQ-BAO-API-004 [domain:Input]
    fn input_dispatch_key_event(
        &self,
        target_id: &str,
        event: KeyEvent,
    ) -> Result<(), BridgeError>;

    /// Input.dispatchTouchEvent — 派发触摸事件。
    ///
    /// @trace REQ-BAO-API-004 [domain:Input]
    fn input_dispatch_touch_event(
        &self,
        target_id: &str,
        event_type: &str,
        touch_points: &[TouchPoint],
    ) -> Result<(), BridgeError>;

    /// Input.setIgnoreInputEvents — 启用/禁用输入忽略。
    ///
    /// @trace REQ-BAO-API-004 [domain:Input]
    fn input_set_ignore_input_events(&self, target_id: &str, ignore: bool) -> Result<(), BridgeError>;

    // ──────────────────────────────────────────────────────────────
    // Emulation domain — 4 method
    // ──────────────────────────────────────────────────────────────

    /// Emulation.setDeviceMetricsOverride — 设置设备度量覆盖。
    ///
    /// @trace REQ-BAO-API-004 [domain:Emulation]
    fn emulation_set_device_metrics(
        &self,
        target_id: &str,
        metrics: DeviceMetrics,
    ) -> Result<(), BridgeError>;

    /// Emulation.clearDeviceMetricsOverride — 清除设备度量覆盖。
    ///
    /// @trace REQ-BAO-API-004 [domain:Emulation]
    fn emulation_clear_device_metrics(&self, target_id: &str) -> Result<(), BridgeError>;

    /// Emulation.setUserAgentOverride — 覆盖 User-Agent。
    ///
    /// @trace REQ-BAO-API-004 [domain:Emulation]
    fn emulation_set_user_agent_override(
        &self,
        target_id: &str,
        user_agent: &str,
    ) -> Result<(), BridgeError>;

    /// Emulation.setGeolocationOverride — 覆盖地理位置。
    ///
    /// @trace REQ-BAO-API-004 [domain:Emulation]
    fn emulation_set_geolocation_override(
        &self,
        target_id: &str,
        latitude: f64,
        longitude: f64,
        accuracy: f64,
    ) -> Result<(), BridgeError>;

    // ──────────────────────────────────────────────────────────────
    // Target domain — 6 method
    // ──────────────────────────────────────────────────────────────

    /// Target.getTargets — 列出所有 target。
    ///
    /// @trace REQ-BAO-API-004 [domain:Target]
    fn target_get_targets(&self) -> Result<Vec<TargetInfo>, BridgeError>;

    /// Target.createTarget — 创建新页面(target)。
    ///
    /// 返回新 target_id。
    ///
    /// @trace REQ-BAO-API-004 [domain:Target]
    fn target_create_target(&self, url: &str) -> Result<String, BridgeError>;

    /// Target.closeTarget — 关闭 target。
    ///
    /// @trace REQ-BAO-API-004 [domain:Target]
    fn target_close_target(&self, target_id: &str) -> Result<(), BridgeError>;

    /// Target.attachToTarget — 附加到 target。
    ///
    /// 返回 session_id(本实现简单返回 target_id 作为 session_id)。
    ///
    /// @trace REQ-BAO-API-004 [domain:Target]
    fn target_attach_to_target(&self, target_id: &str) -> Result<String, BridgeError>;

    /// Target.detachFromTarget — 分离 target。
    ///
    /// @trace REQ-BAO-API-004 [domain:Target]
    fn target_detach_from_target(&self, session_id: &str) -> Result<(), BridgeError>;

    /// Target.setAutoAttach — 自动附加。
    ///
    /// @trace REQ-BAO-API-004 [domain:Target]
    fn target_set_auto_attach(
        &self,
        target_id: &str,
        auto_attach: bool,
        wait_for_debugger_on_start: bool,
    ) -> Result<(), BridgeError>;

    // ──────────────────────────────────────────────────────────────
    // CSS domain — 2 method
    // ──────────────────────────────────────────────────────────────

    /// CSS.getComputedStyleForNode — 返回计算样式。
    ///
    /// @trace REQ-BAO-API-004 [domain:CSS]
    fn css_get_computed_style_for_node(
        &self,
        target_id: &str,
        node_id: i64,
    ) -> Result<Vec<CSSComputedStyleProperty>, BridgeError>;

    /// CSS.getMatchedStylesForNode — 返回匹配样式。
    ///
    /// @trace REQ-BAO-API-004 [domain:CSS]
    fn css_get_matched_styles_for_node(
        &self,
        target_id: &str,
        node_id: i64,
    ) -> Result<MatchedStyles, BridgeError>;
}

// ────────────────────────────────────────────────────────────────────
// 数据结构
// ────────────────────────────────────────────────────────────────────

/// Page.navigate 响应。
///
/// @trace REQ-BAO-API-004 [domain:Page]
#[derive(Debug, Clone, Default)]
pub struct NavigateResult {
    pub frame_id: String,
    pub loader_id: String,
    pub error_text: Option<String>,
}

/// Page.getFrameTree 响应。
///
/// @trace REQ-BAO-API-004 [domain:Page]
#[derive(Debug, Clone)]
pub struct FrameTree {
    pub frame: Frame,
    pub child_frames: Vec<FrameTree>,
}

/// 单个 Frame 描述。
///
/// @trace REQ-BAO-API-004 [domain:Page]
#[derive(Debug, Clone, Default)]
pub struct Frame {
    pub id: String,
    pub parent_id: Option<String>,
    pub loader_id: String,
    pub name: Option<String>,
    pub url: String,
    pub security_origin: String,
    pub mime_type: String,
}

/// Page.getNavigationHistory 响应。
///
/// @trace REQ-BAO-API-004 [domain:Page]
#[derive(Debug, Clone, Default)]
pub struct NavigationHistory {
    pub current_index: i64,
    pub entries: Vec<NavigationEntry>,
}

/// 历史条目。
///
/// @trace REQ-BAO-API-004 [domain:Page]
#[derive(Debug, Clone, Default)]
pub struct NavigationEntry {
    pub id: i64,
    pub url: String,
    pub title: String,
}

/// Page.getLayoutMetrics 响应。
///
/// @trace REQ-BAO-API-004 [domain:Page]
#[derive(Debug, Clone, Default)]
pub struct LayoutMetrics {
    pub layout_width: f64,
    pub layout_height: f64,
    pub content_width: f64,
    pub content_height: f64,
}

/// Runtime.evaluate / callFunctionOn 响应。
///
/// @trace REQ-BAO-API-004 [domain:Runtime]
#[derive(Debug, Clone, Default)]
pub struct EvaluateResult {
    pub result: RemoteObject,
    pub exception_details: Option<ExceptionDetails>,
}

/// Runtime.RemoteObject CDP 等价结构。
///
/// @trace REQ-BAO-API-004 [domain:Runtime]
#[derive(Debug, Clone, Default)]
pub struct RemoteObject {
    pub object_id: Option<String>,
    pub type_: String,
    pub subtype: Option<String>,
    pub value: Option<Value>,
    pub unserializable_value: Option<String>,
    pub class_name: Option<String>,
    pub description: Option<String>,
}

/// Runtime.ExceptionDetails CDP 等价结构(精简版)。
///
/// @trace REQ-BAO-API-004 [domain:Runtime]
#[derive(Debug, Clone, Default)]
pub struct ExceptionDetails {
    pub exception_id: i64,
    pub text: String,
    pub line_number: i64,
    pub column_number: i64,
    pub exception: Option<RemoteObject>,
}

/// Runtime.PropertyDescriptor CDP 等价结构。
///
/// @trace REQ-BAO-API-004 [domain:Runtime]
#[derive(Debug, Clone)]
pub struct PropertyDescriptor {
    pub name: String,
    pub value: Option<RemoteObject>,
    pub writable: Option<bool>,
    pub get: Option<RemoteObject>,
    pub set: Option<RemoteObject>,
    pub configurable: Option<bool>,
    pub enumerable: Option<bool>,
    pub is_own: bool,
    pub symbol: Option<RemoteObject>,
}

/// DOM.getBoxModel 响应。
///
/// @trace REQ-BAO-API-004 [domain:DOM]
#[derive(Debug, Clone, Default)]
pub struct BoxModel {
    pub content: Vec<f64>,   // 8 values (4 points × 2 coords)
    pub padding: Vec<f64>,
    pub border: Vec<f64>,
    pub margin: Vec<f64>,
    pub width: i64,
    pub height: i64,
}

/// Network.getResponseBody 响应。
///
/// @trace REQ-BAO-API-004 [domain:Network]
#[derive(Debug, Clone, Default)]
pub struct ResponseBody {
    pub body: String,
    pub base64_encoded: bool,
}

/// Input.dispatchMouseEvent 输入参数。
///
/// @trace REQ-BAO-API-004 [domain:Input]
#[derive(Debug, Clone, Default)]
pub struct MouseEvent {
    pub event_type: String, // mousePressed/mouseReleased/mouseMoved
    pub x: f64,
    pub y: f64,
    pub button: String, // none/left/right/middle
    pub click_count: i64,
    pub modifiers: i64,
}

/// Input.dispatchKeyEvent 输入参数。
///
/// @trace REQ-BAO-API-004 [domain:Input]
#[derive(Debug, Clone, Default)]
pub struct KeyEvent {
    pub event_type: String, // keyDown/keyUp/rawKeyDown/char
    pub key: String,
    pub code: String,
    pub modifiers: i64,
    pub text: String,
    pub windows_virtual_key_code: i64,
}

/// Input.dispatchTouchEvent 触点描述。
///
/// @trace REQ-BAO-API-004 [domain:Input]
#[derive(Debug, Clone, Default)]
pub struct TouchPoint {
    pub state: String,
    pub x: f64,
    pub y: f64,
    pub radius_x: f64,
    pub radius_y: f64,
    pub force: f64,
}

/// Emulation.setDeviceMetricsOverride 输入。
///
/// @trace REQ-BAO-API-004 [domain:Emulation]
#[derive(Debug, Clone, Default)]
pub struct DeviceMetrics {
    pub width: i64,
    pub height: i64,
    pub device_scale_factor: f64,
    pub mobile: bool,
}

/// Target.getTargets 单项。
///
/// @trace REQ-BAO-API-004 [domain:Target]
#[derive(Debug, Clone, Default)]
pub struct TargetInfo {
    pub target_id: String,
    pub type_: String, // page/background_page/worker/...
    pub title: String,
    pub url: String,
    pub attached: bool,
    pub browser_context_id: Option<String>,
}

/// CSS.getComputedStyleForNode 单项。
///
/// @trace REQ-BAO-API-004 [domain:CSS]
#[derive(Debug, Clone, Default)]
pub struct CSSComputedStyleProperty {
    pub name: String,
    pub value: String,
}

/// CSS.getMatchedStylesForNode 响应。
///
/// @trace REQ-BAO-API-004 [domain:CSS]
#[derive(Debug, Clone, Default)]
pub struct MatchedStyles {
    pub inline_style: Option<CSSStyle>,
    pub attributes_style: Option<CSSStyle>,
    pub matched_rules: Vec<MatchedRule>,
}

/// CSS Style(精简版)。
///
/// @trace REQ-BAO-API-004 [domain:CSS]
#[derive(Debug, Clone, Default)]
pub struct CSSStyle {
    pub style_sheet_id: String,
    pub css_properties: Vec<CSSProperty>,
}

/// CSS 属性。
///
/// @trace REQ-BAO-API-004 [domain:CSS]
#[derive(Debug, Clone, Default)]
pub struct CSSProperty {
    pub name: String,
    pub value: String,
    pub important: bool,
}

/// Matched rule。
///
/// @trace REQ-BAO-API-004 [domain:CSS]
#[derive(Debug, Clone, Default)]
pub struct MatchedRule {
    pub selector: String,
    pub style: CSSStyle,
}

// ────────────────────────────────────────────────────────────────────
// MockServoBackend — 单元测试用,记录所有调用并返回可配置响应。
// ────────────────────────────────────────────────────────────────────

/// Mock backend — 用于单元测试 command dispatcher。
///
/// 默认所有方法返回成功空响应。可通过字段调整特定响应。
///
/// @trace REQ-BAO-API-004 [level:library]
pub struct MockServoBackend {
    /// 调用记录 (target_id, method, payload_summary)。
    pub call_log: Mutex<Vec<(String, String, String)>>,

    /// Page.navigate 默认返回的 frame_id。
    pub default_frame_id: String,
    /// 已存在的 target_id 集合 — page_* 命令查不到时返回 PageNotFound。
    pub known_targets: Mutex<Vec<String>>,
}

impl Default for MockServoBackend {
    fn default() -> Self {
        Self {
            call_log: Mutex::new(Vec::new()),
            default_frame_id: "FRAME_0".to_string(),
            known_targets: Mutex::new(vec!["1".to_string(), "default".to_string()]),
        }
    }
}

impl MockServoBackend {
    /// 构造默认 mock。
    pub fn new() -> Self {
        Self::default()
    }

    /// 包装为 `Arc<dyn ServoBackend>`。
    pub fn into_backend(self) -> Arc<dyn ServoBackend> {
        Arc::new(self)
    }

    fn log(&self, target_id: &str, method: &str, payload: &str) {
        self.call_log.lock().unwrap().push((
            target_id.to_string(),
            method.to_string(),
            payload.to_string(),
        ));
    }

    fn ensure_target(&self, target_id: &str) -> Result<(), BridgeError> {
        let known = self.known_targets.lock().unwrap();
        if !known.iter().any(|t| t == target_id) {
            return Err(BridgeError::PageNotFound(target_id.to_string()));
        }
        Ok(())
    }

    /// 添加已知 target。
    pub fn add_target(&self, target_id: impl Into<String>) {
        self.known_targets.lock().unwrap().push(target_id.into());
    }
}

impl ServoBackend for MockServoBackend {
    fn page_navigate(&self, target_id: &str, url: &str) -> Result<NavigateResult, BridgeError> {
        self.log(target_id, "page_navigate", url);
        self.ensure_target(target_id)?;
        Ok(NavigateResult {
            frame_id: self.default_frame_id.clone(),
            loader_id: format!("LOADER_{url:?}"),
            error_text: None,
        })
    }

    fn page_reload(&self, target_id: &str) -> Result<(), BridgeError> {
        self.log(target_id, "page_reload", "");
        self.ensure_target(target_id)?;
        Ok(())
    }

    fn page_screenshot(
        &self,
        target_id: &str,
        format: BridgeScreenshotFormat,
    ) -> Result<Vec<u8>, BridgeError> {
        self.log(target_id, "page_screenshot", &format!("{format:?}"));
        self.ensure_target(target_id)?;
        // Tiny valid PNG header (mock payload).
        Ok(vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A])
    }

    fn page_frame_tree(&self, target_id: &str) -> Result<FrameTree, BridgeError> {
        self.log(target_id, "page_frame_tree", "");
        self.ensure_target(target_id)?;
        Ok(FrameTree {
            frame: Frame {
                id: self.default_frame_id.clone(),
                url: "about:blank".to_string(),
                mime_type: "text/html".to_string(),
                security_origin: "://".to_string(),
                ..Default::default()
            },
            child_frames: vec![],
        })
    }

    fn page_navigation_history(&self, target_id: &str) -> Result<NavigationHistory, BridgeError> {
        self.log(target_id, "page_navigation_history", "");
        self.ensure_target(target_id)?;
        Ok(NavigationHistory {
            current_index: 0,
            entries: vec![NavigationEntry {
                id: 0,
                url: "about:blank".to_string(),
                title: String::new(),
            }],
        })
    }

    fn page_navigate_to_history_entry(
        &self,
        target_id: &str,
        entry_id: i64,
    ) -> Result<(), BridgeError> {
        self.log(target_id, "page_navigate_to_history_entry", &entry_id.to_string());
        self.ensure_target(target_id)?;
        Ok(())
    }

    fn page_set_content(&self, target_id: &str, html: &str) -> Result<(), BridgeError> {
        self.log(target_id, "page_set_content", html);
        self.ensure_target(target_id)?;
        Ok(())
    }

    fn page_close(&self, target_id: &str) -> Result<(), BridgeError> {
        self.log(target_id, "page_close", "");
        self.ensure_target(target_id)?;
        let mut known = self.known_targets.lock().unwrap();
        known.retain(|t| t != target_id);
        Ok(())
    }

    fn page_bring_to_front(&self, target_id: &str) -> Result<(), BridgeError> {
        self.log(target_id, "page_bring_to_front", "");
        self.ensure_target(target_id)?;
        Ok(())
    }

    fn page_layout_metrics(&self, target_id: &str) -> Result<LayoutMetrics, BridgeError> {
        self.log(target_id, "page_layout_metrics", "");
        self.ensure_target(target_id)?;
        Ok(LayoutMetrics {
            layout_width: 1280.0,
            layout_height: 720.0,
            content_width: 1280.0,
            content_height: 720.0,
        })
    }

    fn page_print_to_pdf(&self, target_id: &str) -> Result<Vec<u8>, BridgeError> {
        self.log(target_id, "page_print_to_pdf", "");
        self.ensure_target(target_id)?;
        Ok(b"%PDF-1.4 mock".to_vec())
    }

    fn runtime_evaluate(
        &self,
        target_id: &str,
        expression: &str,
    ) -> Result<EvaluateResult, BridgeError> {
        self.log(target_id, "runtime_evaluate", expression);
        self.ensure_target(target_id)?;
        Ok(EvaluateResult {
            result: RemoteObject {
                type_: "string".to_string(),
                value: Some(Value::String(expression.to_string())),
                ..Default::default()
            },
            exception_details: None,
        })
    }

    fn runtime_call_function_on(
        &self,
        target_id: &str,
        object_id: &str,
        function_declaration: &str,
        args: &[Value],
    ) -> Result<EvaluateResult, BridgeError> {
        self.log(
            target_id,
            "runtime_call_function_on",
            &format!("{object_id}|{function_declaration}|{}args", args.len()),
        );
        self.ensure_target(target_id)?;
        Ok(EvaluateResult {
            result: RemoteObject {
                type_: "undefined".to_string(),
                ..Default::default()
            },
            exception_details: None,
        })
    }

    fn runtime_get_properties(
        &self,
        target_id: &str,
        object_id: &str,
        own_properties: bool,
    ) -> Result<Vec<PropertyDescriptor>, BridgeError> {
        self.log(
            target_id,
            "runtime_get_properties",
            &format!("{object_id}|own={own_properties}"),
        );
        self.ensure_target(target_id)?;
        Ok(vec![])
    }

    fn runtime_release_object(&self, target_id: &str, object_id: &str) -> Result<(), BridgeError> {
        self.log(target_id, "runtime_release_object", object_id);
        self.ensure_target(target_id)?;
        Ok(())
    }

    fn runtime_enable(&self, target_id: &str) -> Result<(), BridgeError> {
        self.log(target_id, "runtime_enable", "");
        Ok(())
    }

    fn runtime_disable(&self, target_id: &str) -> Result<(), BridgeError> {
        self.log(target_id, "runtime_disable", "");
        Ok(())
    }

    fn dom_get_document(&self, target_id: &str, depth: i64) -> Result<NodeDescriptor, BridgeError> {
        self.log(target_id, "dom_get_document", &depth.to_string());
        self.ensure_target(target_id)?;
        Ok(NodeDescriptor {
            node_id: 1,
            node_name: "#document".to_string(),
            backend_node_id: 1,
            children: vec![],
            ..Default::default()
        })
    }

    fn dom_query_selector(
        &self,
        target_id: &str,
        node_id: i64,
        selector: &str,
    ) -> Result<Option<i64>, BridgeError> {
        self.log(target_id, "dom_query_selector", &format!("{node_id}|{selector}"));
        self.ensure_target(target_id)?;
        Ok(Some(2))
    }

    fn dom_query_selector_all(
        &self,
        target_id: &str,
        node_id: i64,
        selector: &str,
    ) -> Result<Vec<i64>, BridgeError> {
        self.log(
            target_id,
            "dom_query_selector_all",
            &format!("{node_id}|{selector}"),
        );
        self.ensure_target(target_id)?;
        Ok(vec![2, 3])
    }

    fn dom_get_box_model(&self, target_id: &str, node_id: i64) -> Result<BoxModel, BridgeError> {
        self.log(target_id, "dom_get_box_model", &node_id.to_string());
        self.ensure_target(target_id)?;
        Ok(BoxModel {
            content: vec![0.0; 8],
            padding: vec![0.0; 8],
            border: vec![0.0; 8],
            margin: vec![0.0; 8],
            width: 100,
            height: 100,
        })
    }

    fn dom_resolve_node(
        &self,
        target_id: &str,
        backend_node_id: i64,
    ) -> Result<RemoteObject, BridgeError> {
        self.log(target_id, "dom_resolve_node", &backend_node_id.to_string());
        self.ensure_target(target_id)?;
        Ok(RemoteObject {
            object_id: Some(format!("{{node-{backend_node_id}}}")),
            type_: "node".to_string(),
            ..Default::default()
        })
    }

    fn dom_describe_node(
        &self,
        target_id: &str,
        node_id: i64,
        depth: i64,
    ) -> Result<NodeDescriptor, BridgeError> {
        self.log(target_id, "dom_describe_node", &format!("{node_id}|{depth}"));
        self.ensure_target(target_id)?;
        Ok(NodeDescriptor {
            node_id,
            node_name: "DIV".to_string(),
            backend_node_id: node_id,
            ..Default::default()
        })
    }

    fn dom_set_attribute(
        &self,
        target_id: &str,
        node_id: i64,
        name: &str,
        value: &str,
    ) -> Result<(), BridgeError> {
        self.log(
            target_id,
            "dom_set_attribute",
            &format!("{node_id}|{name}={value}"),
        );
        self.ensure_target(target_id)?;
        Ok(())
    }

    fn dom_remove_attribute(
        &self,
        target_id: &str,
        node_id: i64,
        name: &str,
    ) -> Result<(), BridgeError> {
        self.log(target_id, "dom_remove_attribute", &format!("{node_id}|{name}"));
        self.ensure_target(target_id)?;
        Ok(())
    }

    fn dom_get_outer_html(&self, target_id: &str, node_id: i64) -> Result<String, BridgeError> {
        self.log(target_id, "dom_get_outer_html", &node_id.to_string());
        self.ensure_target(target_id)?;
        Ok(format!("<div id=\"node-{node_id}\"></div>"))
    }

    fn dom_set_outer_html(
        &self,
        target_id: &str,
        node_id: i64,
        html: &str,
    ) -> Result<(), BridgeError> {
        self.log(target_id, "dom_set_outer_html", &format!("{node_id}|{html}"));
        self.ensure_target(target_id)?;
        Ok(())
    }

    fn dom_request_node(&self, target_id: &str, object_id: &str) -> Result<i64, BridgeError> {
        self.log(target_id, "dom_request_node", object_id);
        self.ensure_target(target_id)?;
        Ok(3)
    }

    fn network_enable(&self, target_id: &str) -> Result<(), BridgeError> {
        self.log(target_id, "network_enable", "");
        Ok(())
    }

    fn network_disable(&self, target_id: &str) -> Result<(), BridgeError> {
        self.log(target_id, "network_disable", "");
        Ok(())
    }

    fn network_get_response_body(
        &self,
        target_id: &str,
        request_id: &str,
    ) -> Result<ResponseBody, BridgeError> {
        self.log(target_id, "network_get_response_body", request_id);
        self.ensure_target(target_id)?;
        Ok(ResponseBody {
            body: String::new(),
            base64_encoded: false,
        })
    }

    fn network_set_cache_disabled(
        &self,
        target_id: &str,
        disabled: bool,
    ) -> Result<(), BridgeError> {
        self.log(target_id, "network_set_cache_disabled", &disabled.to_string());
        Ok(())
    }

    fn input_dispatch_mouse_event(
        &self,
        target_id: &str,
        event: MouseEvent,
    ) -> Result<(), BridgeError> {
        self.log(
            target_id,
            "input_dispatch_mouse_event",
            &format!("{:?}", event),
        );
        self.ensure_target(target_id)?;
        Ok(())
    }

    fn input_dispatch_key_event(
        &self,
        target_id: &str,
        event: KeyEvent,
    ) -> Result<(), BridgeError> {
        self.log(target_id, "input_dispatch_key_event", &format!("{:?}", event));
        self.ensure_target(target_id)?;
        Ok(())
    }

    fn input_dispatch_touch_event(
        &self,
        target_id: &str,
        event_type: &str,
        touch_points: &[TouchPoint],
    ) -> Result<(), BridgeError> {
        self.log(
            target_id,
            "input_dispatch_touch_event",
            &format!("{event_type}|{}points", touch_points.len()),
        );
        self.ensure_target(target_id)?;
        Ok(())
    }

    fn input_set_ignore_input_events(
        &self,
        target_id: &str,
        ignore: bool,
    ) -> Result<(), BridgeError> {
        self.log(target_id, "input_set_ignore_input_events", &ignore.to_string());
        Ok(())
    }

    fn emulation_set_device_metrics(
        &self,
        target_id: &str,
        metrics: DeviceMetrics,
    ) -> Result<(), BridgeError> {
        self.log(target_id, "emulation_set_device_metrics", &format!("{:?}", metrics));
        self.ensure_target(target_id)?;
        Ok(())
    }

    fn emulation_clear_device_metrics(&self, target_id: &str) -> Result<(), BridgeError> {
        self.log(target_id, "emulation_clear_device_metrics", "");
        self.ensure_target(target_id)?;
        Ok(())
    }

    fn emulation_set_user_agent_override(
        &self,
        target_id: &str,
        user_agent: &str,
    ) -> Result<(), BridgeError> {
        self.log(target_id, "emulation_set_user_agent_override", user_agent);
        self.ensure_target(target_id)?;
        Ok(())
    }

    fn emulation_set_geolocation_override(
        &self,
        target_id: &str,
        latitude: f64,
        longitude: f64,
        accuracy: f64,
    ) -> Result<(), BridgeError> {
        self.log(
            target_id,
            "emulation_set_geolocation_override",
            &format!("{latitude},{longitude},{accuracy}"),
        );
        self.ensure_target(target_id)?;
        Ok(())
    }

    fn target_get_targets(&self) -> Result<Vec<TargetInfo>, BridgeError> {
        // No target_id; this is a global operation.
        let known = self.known_targets.lock().unwrap();
        let targets: Vec<TargetInfo> = known
            .iter()
            .map(|t| TargetInfo {
                target_id: t.clone(),
                type_: "page".to_string(),
                title: format!("Mock page {t}"),
                url: "about:blank".to_string(),
                attached: true,
                browser_context_id: None,
            })
            .collect();
        Ok(targets)
    }

    fn target_create_target(&self, url: &str) -> Result<String, BridgeError> {
        // Generate a new numeric target id.
        let known = self.known_targets.lock().unwrap();
        let max_id = known
            .iter()
            .filter_map(|t| t.parse::<usize>().ok())
            .max()
            .unwrap_or(1);
        drop(known);
        let new_id = (max_id + 1).to_string();
        self.add_target(&new_id);
        // Log under the new target.
        self.log(&new_id, "target_create_target", url);
        Ok(new_id)
    }

    fn target_close_target(&self, target_id: &str) -> Result<(), BridgeError> {
        self.log(target_id, "target_close_target", "");
        let mut known = self.known_targets.lock().unwrap();
        if !known.iter().any(|t| t == target_id) {
            return Err(BridgeError::PageNotFound(target_id.to_string()));
        }
        known.retain(|t| t != target_id);
        Ok(())
    }

    fn target_attach_to_target(&self, target_id: &str) -> Result<String, BridgeError> {
        self.log(target_id, "target_attach_to_target", "");
        self.ensure_target(target_id)?;
        // session_id is target_id + "-session".
        Ok(format!("{target_id}-session"))
    }

    fn target_detach_from_target(&self, session_id: &str) -> Result<(), BridgeError> {
        self.log(session_id, "target_detach_from_target", "");
        Ok(())
    }

    fn target_set_auto_attach(
        &self,
        target_id: &str,
        auto_attach: bool,
        wait_for_debugger_on_start: bool,
    ) -> Result<(), BridgeError> {
        self.log(
            target_id,
            "target_set_auto_attach",
            &format!("{auto_attach}|{wait_for_debugger_on_start}"),
        );
        Ok(())
    }

    fn css_get_computed_style_for_node(
        &self,
        target_id: &str,
        node_id: i64,
    ) -> Result<Vec<CSSComputedStyleProperty>, BridgeError> {
        self.log(
            target_id,
            "css_get_computed_style_for_node",
            &node_id.to_string(),
        );
        self.ensure_target(target_id)?;
        Ok(vec![CSSComputedStyleProperty {
            name: "display".to_string(),
            value: "block".to_string(),
        }])
    }

    fn css_get_matched_styles_for_node(
        &self,
        target_id: &str,
        node_id: i64,
    ) -> Result<MatchedStyles, BridgeError> {
        self.log(
            target_id,
            "css_get_matched_styles_for_node",
            &node_id.to_string(),
        );
        self.ensure_target(target_id)?;
        Ok(MatchedStyles {
            inline_style: None,
            attributes_style: None,
            matched_rules: vec![],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_navigate_records_call() {
        let b = MockServoBackend::new();
        let r = b.page_navigate("1", "https://example.com").unwrap();
        assert_eq!(r.frame_id, "FRAME_0");
        let log = b.call_log.lock().unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].0, "1");
        assert_eq!(log[0].1, "page_navigate");
        assert_eq!(log[0].2, "https://example.com");
    }

    #[test]
    fn mock_navigate_unknown_target_returns_page_not_found() {
        let b = MockServoBackend::new();
        let err = b.page_navigate("999", "x").unwrap_err();
        assert!(matches!(err, BridgeError::PageNotFound(_)));
    }

    #[test]
    fn mock_screenshot_returns_png_header() {
        let b = MockServoBackend::new();
        let bytes = b.page_screenshot("1", BridgeScreenshotFormat::Png).unwrap();
        assert_eq!(&bytes[..4], &[0x89, 0x50, 0x4E, 0x47]);
    }

    #[test]
    fn mock_target_create_then_close_roundtrip() {
        let b = MockServoBackend::new();
        let new_id = b.target_create_target("about:blank").unwrap();
        // New target should be valid for navigate.
        b.page_navigate(&new_id, "https://x").unwrap();
        // Close it.
        b.target_close_target(&new_id).unwrap();
        // Now navigate should fail.
        let err = b.page_navigate(&new_id, "x").unwrap_err();
        assert!(matches!(err, BridgeError::PageNotFound(_)));
    }

    #[test]
    fn screenshot_format_parsing() {
        assert_eq!(
            BridgeScreenshotFormat::from_cdp(Some("jpeg")),
            BridgeScreenshotFormat::Jpeg
        );
        assert_eq!(
            BridgeScreenshotFormat::from_cdp(Some("png")),
            BridgeScreenshotFormat::Png
        );
        assert_eq!(
            BridgeScreenshotFormat::from_cdp(None),
            BridgeScreenshotFormat::Png
        );
        assert_eq!(
            BridgeScreenshotFormat::from_cdp(Some("unknown")),
            BridgeScreenshotFormat::Png
        );
    }

    #[test]
    fn target_get_targets_lists_known() {
        let b = MockServoBackend::new();
        let ts = b.target_get_targets().unwrap();
        assert!(!ts.is_empty());
        assert!(ts.iter().any(|t| t.target_id == "1"));
    }

    #[test]
    fn target_attach_returns_session_id() {
        let b = MockServoBackend::new();
        let s = b.target_attach_to_target("1").unwrap();
        assert!(s.contains("1"));
        assert!(s.contains("session"));
    }

    #[test]
    fn mock_runtime_evaluate_returns_expression_echo() {
        let b = MockServoBackend::new();
        let r = b.runtime_evaluate("1", "1+1").unwrap();
        assert_eq!(r.result.type_, "string");
        assert_eq!(r.result.value.as_ref().unwrap(), &Value::String("1+1".into()));
    }
}

// ────────────────────────────────────────────────────────────────────
// Blanket impl:`Arc<dyn ServoBackend>` 自己也满足 `ServoBackend`。
// 这样 `dispatch_command<B: ServoBackend>` 与 `CDPRdpBridge` 可直接接受它。
// ────────────────────────────────────────────────────────────────────

// @trace REQ-BAO-API-004 [level:library]
impl ServoBackend for Arc<dyn ServoBackend> {
    fn page_navigate(&self, target_id: &str, url: &str) -> Result<NavigateResult, BridgeError> {
        (**self).page_navigate(target_id, url)
    }
    fn page_reload(&self, target_id: &str) -> Result<(), BridgeError> {
        (**self).page_reload(target_id)
    }
    fn page_screenshot(
        &self,
        target_id: &str,
        format: BridgeScreenshotFormat,
    ) -> Result<Vec<u8>, BridgeError> {
        (**self).page_screenshot(target_id, format)
    }
    fn page_frame_tree(&self, target_id: &str) -> Result<FrameTree, BridgeError> {
        (**self).page_frame_tree(target_id)
    }
    fn page_navigation_history(&self, target_id: &str) -> Result<NavigationHistory, BridgeError> {
        (**self).page_navigation_history(target_id)
    }
    fn page_navigate_to_history_entry(
        &self,
        target_id: &str,
        entry_id: i64,
    ) -> Result<(), BridgeError> {
        (**self).page_navigate_to_history_entry(target_id, entry_id)
    }
    fn page_set_content(&self, target_id: &str, html: &str) -> Result<(), BridgeError> {
        (**self).page_set_content(target_id, html)
    }
    fn page_close(&self, target_id: &str) -> Result<(), BridgeError> {
        (**self).page_close(target_id)
    }
    fn page_bring_to_front(&self, target_id: &str) -> Result<(), BridgeError> {
        (**self).page_bring_to_front(target_id)
    }
    fn page_layout_metrics(&self, target_id: &str) -> Result<LayoutMetrics, BridgeError> {
        (**self).page_layout_metrics(target_id)
    }
    fn page_print_to_pdf(&self, target_id: &str) -> Result<Vec<u8>, BridgeError> {
        (**self).page_print_to_pdf(target_id)
    }
    fn runtime_evaluate(
        &self,
        target_id: &str,
        expression: &str,
    ) -> Result<EvaluateResult, BridgeError> {
        (**self).runtime_evaluate(target_id, expression)
    }
    fn runtime_call_function_on(
        &self,
        target_id: &str,
        object_id: &str,
        function_declaration: &str,
        args: &[Value],
    ) -> Result<EvaluateResult, BridgeError> {
        (**self).runtime_call_function_on(
            target_id,
            object_id,
            function_declaration,
            args,
        )
    }
    fn runtime_get_properties(
        &self,
        target_id: &str,
        object_id: &str,
        own_properties: bool,
    ) -> Result<Vec<PropertyDescriptor>, BridgeError> {
        (**self).runtime_get_properties(target_id, object_id, own_properties)
    }
    fn runtime_release_object(&self, target_id: &str, object_id: &str) -> Result<(), BridgeError> {
        (**self).runtime_release_object(target_id, object_id)
    }
    fn runtime_enable(&self, target_id: &str) -> Result<(), BridgeError> {
        (**self).runtime_enable(target_id)
    }
    fn runtime_disable(&self, target_id: &str) -> Result<(), BridgeError> {
        (**self).runtime_disable(target_id)
    }
    fn dom_get_document(&self, target_id: &str, depth: i64) -> Result<NodeDescriptor, BridgeError> {
        (**self).dom_get_document(target_id, depth)
    }
    fn dom_query_selector(
        &self,
        target_id: &str,
        node_id: i64,
        selector: &str,
    ) -> Result<Option<i64>, BridgeError> {
        (**self).dom_query_selector(target_id, node_id, selector)
    }
    fn dom_query_selector_all(
        &self,
        target_id: &str,
        node_id: i64,
        selector: &str,
    ) -> Result<Vec<i64>, BridgeError> {
        (**self).dom_query_selector_all(target_id, node_id, selector)
    }
    fn dom_get_box_model(&self, target_id: &str, node_id: i64) -> Result<BoxModel, BridgeError> {
        (**self).dom_get_box_model(target_id, node_id)
    }
    fn dom_resolve_node(
        &self,
        target_id: &str,
        backend_node_id: i64,
    ) -> Result<RemoteObject, BridgeError> {
        (**self).dom_resolve_node(target_id, backend_node_id)
    }
    fn dom_describe_node(
        &self,
        target_id: &str,
        node_id: i64,
        depth: i64,
    ) -> Result<NodeDescriptor, BridgeError> {
        (**self).dom_describe_node(target_id, node_id, depth)
    }
    fn dom_set_attribute(
        &self,
        target_id: &str,
        node_id: i64,
        name: &str,
        value: &str,
    ) -> Result<(), BridgeError> {
        (**self).dom_set_attribute(target_id, node_id, name, value)
    }
    fn dom_remove_attribute(
        &self,
        target_id: &str,
        node_id: i64,
        name: &str,
    ) -> Result<(), BridgeError> {
        (**self).dom_remove_attribute(target_id, node_id, name)
    }
    fn dom_get_outer_html(&self, target_id: &str, node_id: i64) -> Result<String, BridgeError> {
        (**self).dom_get_outer_html(target_id, node_id)
    }
    fn dom_set_outer_html(
        &self,
        target_id: &str,
        node_id: i64,
        html: &str,
    ) -> Result<(), BridgeError> {
        (**self).dom_set_outer_html(target_id, node_id, html)
    }
    fn dom_request_node(&self, target_id: &str, object_id: &str) -> Result<i64, BridgeError> {
        (**self).dom_request_node(target_id, object_id)
    }
    fn network_enable(&self, target_id: &str) -> Result<(), BridgeError> {
        (**self).network_enable(target_id)
    }
    fn network_disable(&self, target_id: &str) -> Result<(), BridgeError> {
        (**self).network_disable(target_id)
    }
    fn network_get_response_body(
        &self,
        target_id: &str,
        request_id: &str,
    ) -> Result<ResponseBody, BridgeError> {
        (**self).network_get_response_body(target_id, request_id)
    }
    fn network_set_cache_disabled(
        &self,
        target_id: &str,
        disabled: bool,
    ) -> Result<(), BridgeError> {
        (**self).network_set_cache_disabled(target_id, disabled)
    }
    fn input_dispatch_mouse_event(
        &self,
        target_id: &str,
        event: MouseEvent,
    ) -> Result<(), BridgeError> {
        (**self).input_dispatch_mouse_event(target_id, event)
    }
    fn input_dispatch_key_event(
        &self,
        target_id: &str,
        event: KeyEvent,
    ) -> Result<(), BridgeError> {
        (**self).input_dispatch_key_event(target_id, event)
    }
    fn input_dispatch_touch_event(
        &self,
        target_id: &str,
        event_type: &str,
        touch_points: &[TouchPoint],
    ) -> Result<(), BridgeError> {
        (**self).input_dispatch_touch_event(target_id, event_type, touch_points)
    }
    fn input_set_ignore_input_events(&self, target_id: &str, ignore: bool) -> Result<(), BridgeError> {
        (**self).input_set_ignore_input_events(target_id, ignore)
    }
    fn emulation_set_device_metrics(
        &self,
        target_id: &str,
        metrics: DeviceMetrics,
    ) -> Result<(), BridgeError> {
        (**self).emulation_set_device_metrics(target_id, metrics)
    }
    fn emulation_clear_device_metrics(&self, target_id: &str) -> Result<(), BridgeError> {
        (**self).emulation_clear_device_metrics(target_id)
    }
    fn emulation_set_user_agent_override(
        &self,
        target_id: &str,
        user_agent: &str,
    ) -> Result<(), BridgeError> {
        (**self).emulation_set_user_agent_override(target_id, user_agent)
    }
    fn emulation_set_geolocation_override(
        &self,
        target_id: &str,
        latitude: f64,
        longitude: f64,
        accuracy: f64,
    ) -> Result<(), BridgeError> {
        (**self).emulation_set_geolocation_override(target_id, latitude, longitude, accuracy)
    }
    fn target_get_targets(&self) -> Result<Vec<TargetInfo>, BridgeError> {
        (**self).target_get_targets()
    }
    fn target_create_target(&self, url: &str) -> Result<String, BridgeError> {
        (**self).target_create_target(url)
    }
    fn target_close_target(&self, target_id: &str) -> Result<(), BridgeError> {
        (**self).target_close_target(target_id)
    }
    fn target_attach_to_target(&self, target_id: &str) -> Result<String, BridgeError> {
        (**self).target_attach_to_target(target_id)
    }
    fn target_detach_from_target(&self, session_id: &str) -> Result<(), BridgeError> {
        (**self).target_detach_from_target(session_id)
    }
    fn target_set_auto_attach(
        &self,
        target_id: &str,
        auto_attach: bool,
        wait_for_debugger_on_start: bool,
    ) -> Result<(), BridgeError> {
        (**self).target_set_auto_attach(target_id, auto_attach, wait_for_debugger_on_start)
    }
    fn css_get_computed_style_for_node(
        &self,
        target_id: &str,
        node_id: i64,
    ) -> Result<Vec<CSSComputedStyleProperty>, BridgeError> {
        (**self).css_get_computed_style_for_node(target_id, node_id)
    }
    fn css_get_matched_styles_for_node(
        &self,
        target_id: &str,
        node_id: i64,
    ) -> Result<MatchedStyles, BridgeError> {
        (**self).css_get_matched_styles_for_node(target_id, node_id)
    }
}
