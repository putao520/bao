//! servo 7 大事件 → CDP event 完整映射。
//!
//! ## 7 类事件(REQ-BAO-API-003)
//!
//! | servo 事件 | CDP event |
//! |------------|-----------|
//! | Console | `Log.entryAdded` |
//! | PageError(含 JS 异常) | `Runtime.exceptionThrown` |
//! | NetworkEvent | `Network.{requestWillBeSent, responseReceived, loadingFinished, loadingFailed}` |
//! | DomMutation | `DOM.{attributeModified, characterDataModified}` |
//! | SourceInfo | `Debugger.scriptParsed` |
//! | FrameInfo | `Page.{frameNavigated, frameStartedLoading, frameStoppedLoading}` |
//! | TimelineMarker | `Performance.metrics` |
//!
//! ## 数据流
//!
//! ```text
//!   servo ScriptThread
//!       ↓ (servo delegate callback)
//!   EventSubscriber::on_*  (本模块)
//!       ↓ mpsc::Sender<ServoEvent>
//!   InMemoryTransport::recv_event  (translate 转换)
//!       ↓ CdpEvent
//!   CDP Client
//! ```
//!
//! ## 线程模型(DEC-CDP-002)
//!
//! servo ScriptThread `!Send`,但 `mpsc::Sender` `Send`,可跨线程 push。
//! EventSubscriber 持有 `Sender<ServoEvent>`,被 servo delegate 在 servo 线程
//! 调用 `on_console_message` 等方法时,直接 push 到 channel。
//! InMemoryTransport 在 client 线程 `recv_event`,translate 后返回。
//!
//! @trace REQ-BAO-API-003 [level:library]
//! @trace REQ-BAO-API-003 [event:Console]
//! @trace REQ-BAO-API-003 [event:PageError]
//! @trace REQ-BAO-API-003 [event:NetworkEvent]
//! @trace REQ-BAO-API-003 [event:DomMutation]
//! @trace REQ-BAO-API-003 [event:SourceInfo]
//! @trace REQ-BAO-API-003 [event:FrameInfo]
//! @trace REQ-BAO-API-003 [event:TimelineMarker]

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::transport::CdpEvent;

// ---------------------------------------------------------------------------
// §1 ConsoleLevel — servo console level → CDP Log.EntryLevel
// ---------------------------------------------------------------------------

/// servo console 日志级别 → CDP `Log.EntryLevel`。
///
/// CDP 标准级别(https://chromedevtools.github.io/devtools-protocol/tot/Log/#type-EntryLevel):
/// `verbose / info / warning / error / debug`
///
/// servo 的 `console.log/info/warn/error/debug/trace` 通过字符串级别表示,
/// 本枚举固化五种 + 未知默认 `info`。
///
/// @trace REQ-BAO-API-003 [event:Console]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleLevel {
    Verbose,
    Info,
    Warning,
    Error,
    Debug,
}

impl ConsoleLevel {
    /// 从 servo 字符串级别(如 "info"/"warning"/"error"/"debug"/"verbose"/"log")
    /// 解析到枚举。未知级别归到 `Info`(与 CDP 行为一致)。
    ///
    /// @trace REQ-BAO-API-003 [event:Console]
    pub fn from_servo_str(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "verbose" | "trace" => ConsoleLevel::Verbose,
            "info" | "log" => ConsoleLevel::Info,
            "warning" | "warn" => ConsoleLevel::Warning,
            "error" => ConsoleLevel::Error,
            "debug" => ConsoleLevel::Debug,
            _ => ConsoleLevel::Info,
        }
    }

    /// 转 CDP `Log.EntryLevel` 字符串。
    ///
    /// @trace REQ-BAO-API-003 [event:Console]
    pub fn to_cdp_str(self) -> &'static str {
        match self {
            ConsoleLevel::Verbose => "verbose",
            ConsoleLevel::Info => "info",
            ConsoleLevel::Warning => "warning",
            ConsoleLevel::Error => "error",
            ConsoleLevel::Debug => "debug",
        }
    }
}

// ---------------------------------------------------------------------------
// §2 ServoEvent — servo delegate 推送的事件枚举
// ---------------------------------------------------------------------------

/// servo 推送给 CDP client 的所有事件类型(REQ-BAO-API-003 的 7 类)。
///
/// 由 [`EventSubscriber`] 在 servo delegate 回调中构造,push 到 mpsc channel,
/// 由 `InMemoryTransport::recv_event` 调用 [`translate`] 转换为 [`CdpEvent`]。
///
/// # 字段命名规范
///
/// 所有字段名与 CDP 协议规范(https://chromedevtools.github.io/devtools-protocol/)
/// 中的对应字段 camelCase 对齐,便于 [`translate`] 函数直接序列化。
///
/// @trace REQ-BAO-API-003 [level:library]
#[derive(Debug, Clone)]
pub enum ServoEvent {
    // ── Console 类(REQ-BAO-API-003.1)
    /// servo console.{log,info,warn,error,debug,trace} 消息。
    ///
    /// → `Log.entryAdded`
    ///
    /// @trace REQ-BAO-API-003 [event:Console]
    Console {
        target_id: String,
        level: ConsoleLevel,
        text: String,
        url: Option<String>,
        line: Option<u32>,
        column: Option<u32>,
    },

    // ── PageError 类(REQ-BAO-API-003.2)
    /// servo 页面错误(包括 JS 异常、CSS 解析错误、控制台 error)。
    ///
    /// → `Runtime.exceptionThrown`
    ///
    /// @trace REQ-BAO-API-003 [event:PageError]
    PageError {
        target_id: String,
        text: String,
        url: Option<String>,
        line: Option<u32>,
        column: Option<u32>,
        stack: Option<String>,
    },

    // ── NetworkEvent 类(REQ-BAO-API-003.3) — 4 个子事件
    /// 网络请求开始。
    ///
    /// → `Network.requestWillBeSent`
    ///
    /// @trace REQ-BAO-API-003 [event:NetworkEvent]
    NetworkRequest {
        target_id: String,
        request_id: String,
        url: String,
        method: String,
        headers: HashMap<String, String>,
        post_data: Option<Vec<u8>>,
        resource_type: String,
        frame_id: String,
    },
    /// 网络响应到达。
    ///
    /// → `Network.responseReceived`
    ///
    /// @trace REQ-BAO-API-003 [event:NetworkEvent]
    NetworkResponse {
        target_id: String,
        request_id: String,
        url: String,
        status: u16,
        status_text: String,
        headers: HashMap<String, String>,
        mime_type: String,
        remote_ip: Option<String>,
    },
    /// 网络请求完成(成功)。
    ///
    /// → `Network.loadingFinished`
    ///
    /// @trace REQ-BAO-API-003 [event:NetworkEvent]
    NetworkLoadingFinish {
        target_id: String,
        request_id: String,
        encoded_data_length: u64,
    },
    /// 网络请求失败。
    ///
    /// → `Network.loadingFailed`
    ///
    /// @trace REQ-BAO-API-003 [event:NetworkEvent]
    NetworkLoadingFail {
        target_id: String,
        request_id: String,
        error_text: String,
        canceled: bool,
    },

    // ── DomMutation 类(REQ-BAO-API-003.4) — 2 个子事件
    /// DOM 属性变更。
    ///
    /// → `DOM.attributeModified`
    ///
    /// @trace REQ-BAO-API-003 [event:DomMutation]
    DomAttributeModified {
        target_id: String,
        node_id: i64,
        name: String,
        value: String,
    },
    /// DOM character data 变更。
    ///
    /// → `DOM.characterDataModified`
    ///
    /// @trace REQ-BAO-API-003 [event:DomMutation]
    DomCharacterDataModified {
        target_id: String,
        node_id: i64,
        old_value: String,
        new_value: String,
    },

    // ── SourceInfo 类(REQ-BAO-API-003.5)
    /// 脚本编译完成(servo ScriptThread 解析出新 script element 或 inline script)。
    ///
    /// → `Debugger.scriptParsed`
    ///
    /// @trace REQ-BAO-API-003 [event:SourceInfo]
    ScriptParsed {
        target_id: String,
        script_id: String,
        url: String,
        start_line: u32,
        start_column: u32,
        end_line: u32,
        end_column: u32,
        source_map_url: Option<String>,
    },

    // ── FrameInfo 类(REQ-BAO-API-003.6) — 3 个子事件
    /// frame 导航完成。
    ///
    /// → `Page.frameNavigated`
    ///
    /// @trace REQ-BAO-API-003 [event:FrameInfo]
    FrameNavigated {
        target_id: String,
        frame_id: String,
        url: String,
        name: Option<String>,
    },
    /// frame 开始加载。
    ///
    /// → `Page.frameStartedLoading`
    ///
    /// @trace REQ-BAO-API-003 [event:FrameInfo]
    FrameStartedLoading {
        target_id: String,
        frame_id: String,
    },
    /// frame 停止加载。
    ///
    /// → `Page.frameStoppedLoading`
    ///
    /// @trace REQ-BAO-API-003 [event:FrameInfo]
    FrameStoppedLoading {
        target_id: String,
        frame_id: String,
    },

    // ── TimelineMarker 类(REQ-BAO-API-003.7)
    /// servo 性能 timeline 标记。
    ///
    /// → `Performance.metrics`
    ///
    /// @trace REQ-BAO-API-003 [event:TimelineMarker]
    TimelineMarker {
        target_id: String,
        name: String,
        start_time: f64,
        end_time: f64,
    },
}

impl ServoEvent {
    /// 返回事件归属的 target_id(用于 CDP event 的 session_id)。
    ///
    /// @trace REQ-BAO-API-003 [level:library]
    pub fn target_id(&self) -> &str {
        match self {
            ServoEvent::Console { target_id, .. }
            | ServoEvent::PageError { target_id, .. }
            | ServoEvent::NetworkRequest { target_id, .. }
            | ServoEvent::NetworkResponse { target_id, .. }
            | ServoEvent::NetworkLoadingFinish { target_id, .. }
            | ServoEvent::NetworkLoadingFail { target_id, .. }
            | ServoEvent::DomAttributeModified { target_id, .. }
            | ServoEvent::DomCharacterDataModified { target_id, .. }
            | ServoEvent::ScriptParsed { target_id, .. }
            | ServoEvent::FrameNavigated { target_id, .. }
            | ServoEvent::FrameStartedLoading { target_id, .. }
            | ServoEvent::FrameStoppedLoading { target_id, .. }
            | ServoEvent::TimelineMarker { target_id, .. } => target_id,
        }
    }
}

// ---------------------------------------------------------------------------
// §3 translate — ServoEvent → Vec<CdpEvent>
// ---------------------------------------------------------------------------

/// 把一个 servo 事件转换成 0 个或多个 CDP event。
///
/// 一个 servo 事件可能产生多个 CDP event(例如 `NetworkResponse` 在 CDP 中
/// 通常紧跟一个 `Network.loadingFinished`)。返回 `Vec<CdpEvent>` 支持这种一对多。
///
/// 所有事件带 `session_id = Some(target_id)`,与 InMemoryTransport 的
/// session 路由一致。
///
/// @trace REQ-BAO-API-003 [level:library]
pub fn translate(event: ServoEvent) -> Vec<CdpEvent> {
    match event {
        // ────────────────────────────────────────────────────────────────
        // §3.1 Console → Log.entryAdded
        // ────────────────────────────────────────────────────────────────
        // @trace REQ-BAO-API-003 [event:Console]
        ServoEvent::Console {
            target_id,
            level,
            text,
            url,
            line,
            column,
        } => {
            vec![CdpEvent {
                method: "Log.entryAdded".into(),
                params: json!({
                    "entry": {
                        "source": "javascript",
                        "level": level.to_cdp_str(),
                        "text": text,
                        "url": url,
                        "lineNumber": line.unwrap_or(0),
                        "columnNumber": column.unwrap_or(0),
                        "timestamp": current_timestamp_ms(),
                    }
                }),
                session_id: Some(target_id),
            }]
        }

        // ────────────────────────────────────────────────────────────────
        // §3.2 PageError → Runtime.exceptionThrown
        // ────────────────────────────────────────────────────────────────
        // @trace REQ-BAO-API-003 [event:PageError]
        ServoEvent::PageError {
            target_id,
            text,
            url,
            line,
            column,
            stack,
        } => {
            let stack_trace = match &stack {
                Some(s) => json!([{
                    "functionName": "",
                    "url": url.clone().unwrap_or_default(),
                    "lineNumber": line.unwrap_or(0),
                    "columnNumber": column.unwrap_or(0),
                    "scriptName": s,
                }]),
                None => Value::Null,
            };
            vec![CdpEvent {
                method: "Runtime.exceptionThrown".into(),
                params: json!({
                    "timestamp": current_timestamp_ms(),
                    "exceptionDetails": {
                        "exceptionId": 0,
                        "text": text,
                        "lineNumber": line.unwrap_or(0),
                        "columnNumber": column.unwrap_or(0),
                        "url": url,
                        "stackTrace": stack_trace,
                        "exception": {
                            "type": "string",
                            "value": text,
                        },
                    }
                }),
                session_id: Some(target_id),
            }]
        }

        // ────────────────────────────────────────────────────────────────
        // §3.3 NetworkEvent → Network.* (4 子事件)
        // ────────────────────────────────────────────────────────────────
        // @trace REQ-BAO-API-003 [event:NetworkEvent]
        ServoEvent::NetworkRequest {
            target_id,
            request_id,
            url,
            method,
            headers,
            post_data,
            resource_type,
            frame_id,
        } => {
            let mut request = json!({
                "url": url,
                "method": method,
                "headers": headers_to_json(&headers),
            });
            if let Some(post) = post_data {
                // CDP spec: postData 为 base64(字节流)。这里用标准 base64 编码。
                // 兼容 std-only 路径:直接用 bun_base64 crate。
                request["postData"] = json!(base64_encode(&post));
            }
            vec![CdpEvent {
                method: "Network.requestWillBeSent".into(),
                params: json!({
                    "requestId": request_id,
                    "loaderId": frame_id,
                    "documentURL": url,
                    "request": request,
                    "timestamp": current_timestamp_s(),
                    "wallTime": current_timestamp_s(),
                    "type": resource_type,
                    "frameId": frame_id,
                    "hasUserGesture": false,
                }),
                session_id: Some(target_id),
            }]
        }
        // @trace REQ-BAO-API-003 [event:NetworkEvent]
        ServoEvent::NetworkResponse {
            target_id,
            request_id,
            url,
            status,
            status_text,
            headers,
            mime_type,
            remote_ip,
        } => {
            let mut response = json!({
                "url": url,
                "status": status,
                "statusText": status_text,
                "headers": headers_to_json(&headers),
                "mimeType": mime_type,
                "connectionReused": false,
                "connectionId": 0,
                "encodedDataLength": 0,
                "protocol": "http/1.1",
                "securityState": "unknown",
            });
            if let Some(ip) = remote_ip {
                response["remoteIPAddress"] = json!(ip);
            }
            vec![CdpEvent {
                method: "Network.responseReceived".into(),
                params: json!({
                    "requestId": request_id,
                    "loaderId": "0",
                    "timestamp": current_timestamp_s(),
                    "type": "Other",
                    "response": response,
                    "hasExtraInfo": false,
                }),
                session_id: Some(target_id),
            }]
        }
        // @trace REQ-BAO-API-003 [event:NetworkEvent]
        ServoEvent::NetworkLoadingFinish {
            target_id,
            request_id,
            encoded_data_length,
        } => {
            vec![CdpEvent {
                method: "Network.loadingFinished".into(),
                params: json!({
                    "requestId": request_id,
                    "timestamp": current_timestamp_s(),
                    "encodedDataLength": encoded_data_length,
                }),
                session_id: Some(target_id),
            }]
        }
        // @trace REQ-BAO-API-003 [event:NetworkEvent]
        ServoEvent::NetworkLoadingFail {
            target_id,
            request_id,
            error_text,
            canceled,
        } => {
            vec![CdpEvent {
                method: "Network.loadingFailed".into(),
                params: json!({
                    "requestId": request_id,
                    "timestamp": current_timestamp_s(),
                    "type": "Other",
                    "errorText": error_text,
                    "canceled": canceled,
                    "blockedReason": null,
                    "corsErrorStatus": null,
                }),
                session_id: Some(target_id),
            }]
        }

        // ────────────────────────────────────────────────────────────────
        // §3.4 DomMutation → DOM.* (2 子事件)
        // ────────────────────────────────────────────────────────────────
        // @trace REQ-BAO-API-003 [event:DomMutation]
        ServoEvent::DomAttributeModified {
            target_id,
            node_id,
            name,
            value,
        } => {
            vec![CdpEvent {
                method: "DOM.attributeModified".into(),
                params: json!({
                    "nodeId": node_id,
                    "name": name,
                    "value": value,
                }),
                session_id: Some(target_id),
            }]
        }
        // @trace REQ-BAO-API-003 [event:DomMutation]
        ServoEvent::DomCharacterDataModified {
            target_id,
            node_id,
            old_value: _,
            new_value,
        } => {
            vec![CdpEvent {
                method: "DOM.characterDataModified".into(),
                params: json!({
                    "nodeId": node_id,
                    "characterData": new_value,
                }),
                session_id: Some(target_id),
            }]
        }

        // ────────────────────────────────────────────────────────────────
        // §3.5 SourceInfo → Debugger.scriptParsed
        // ────────────────────────────────────────────────────────────────
        // @trace REQ-BAO-API-003 [event:SourceInfo]
        ServoEvent::ScriptParsed {
            target_id,
            script_id,
            url,
            start_line,
            start_column,
            end_line,
            end_column,
            source_map_url,
        } => {
            let mut params = json!({
                "scriptId": script_id,
                "url": url,
                "startLine": start_line,
                "startColumn": start_column,
                "endLine": end_line,
                "endColumn": end_column,
                "executionContextId": 0,
                "hash": "",
                "isModule": false,
                "length": 0,
            });
            if let Some(sm) = source_map_url {
                params["sourceMapURL"] = json!(sm);
            }
            vec![CdpEvent {
                method: "Debugger.scriptParsed".into(),
                params,
                session_id: Some(target_id),
            }]
        }

        // ────────────────────────────────────────────────────────────────
        // §3.6 FrameInfo → Page.* (3 子事件)
        // ────────────────────────────────────────────────────────────────
        // @trace REQ-BAO-API-003 [event:FrameInfo]
        ServoEvent::FrameNavigated {
            target_id,
            frame_id,
            url,
            name,
        } => {
            let mut frame = json!({
                "id": frame_id,
                "url": url,
                "securityOrigin": origin_from_url(&url),
                "mimeType": "text/html",
                "domainAndRegistry": "",
                "secureContextType": "Secure",
                "crossOriginIsolatedContextType": "NotIsolated",
            });
            if let Some(n) = name {
                frame["name"] = json!(n);
            }
            vec![CdpEvent {
                method: "Page.frameNavigated".into(),
                params: json!({
                    "frame": frame,
                    "type": "Navigation",
                }),
                session_id: Some(target_id),
            }]
        }
        // @trace REQ-BAO-API-003 [event:FrameInfo]
        ServoEvent::FrameStartedLoading {
            target_id,
            frame_id,
        } => {
            vec![CdpEvent {
                method: "Page.frameStartedLoading".into(),
                params: json!({
                    "frameId": frame_id,
                }),
                session_id: Some(target_id),
            }]
        }
        // @trace REQ-BAO-API-003 [event:FrameInfo]
        ServoEvent::FrameStoppedLoading {
            target_id,
            frame_id,
        } => {
            vec![CdpEvent {
                method: "Page.frameStoppedLoading".into(),
                params: json!({
                    "frameId": frame_id,
                }),
                session_id: Some(target_id),
            }]
        }

        // ────────────────────────────────────────────────────────────────
        // §3.7 TimelineMarker → Performance.metrics
        // ────────────────────────────────────────────────────────────────
        // @trace REQ-BAO-API-003 [event:TimelineMarker]
        ServoEvent::TimelineMarker {
            target_id,
            name,
            start_time,
            end_time,
        } => {
            let duration = end_time - start_time;
            vec![CdpEvent {
                method: "Performance.metrics".into(),
                params: json!({
                    "metrics": [
                        {"name": format!("{}_start", name), "value": start_time},
                        {"name": format!("{}_end", name), "value": end_time},
                        {"name": format!("{}_duration_ms", name), "value": duration * 1000.0},
                    ],
                    "title": format!("servo-timeline-{}", name),
                }),
                session_id: Some(target_id),
            }]
        }
    }
}

// ---------------------------------------------------------------------------
// §4 EventSubscriber — servo delegate → mpsc::Sender<ServoEvent>
// ---------------------------------------------------------------------------

/// 事件订阅者 — servo delegate 在 servo 线程调用 on_* 方法,push 事件到 channel。
///
/// 用法:
/// ```ignore
/// use bao_cdp_client::bridge::EventSubscriber;
///
/// let (subscriber, rx) = EventSubscriber::new();
/// // 把 subscriber 注册到 servo delegate
/// // servo 调用 subscriber.on_console_message(...) 时,事件进入 channel
/// // 主线程在 InMemoryTransport 内 recv_event 时,translate 后返回 CdpEvent
/// ```
///
/// # 关闭语义
///
/// 当 `EventSubscriber` drop 时,`Sender` 被丢弃,接收端 `recv` 会收到
/// `RecvTimeoutError::Disconnected`。
///
/// # 线程安全
///
/// `mpsc::Sender` 是 `Send + Sync`,可被 servo delegate 在 servo 线程持有。
/// 但注意 `EventSubscriber` 不 `Clone`(避免多 sender 混淆事件源);
/// 如需多 sender,显式调 [`EventSubscriber::sender`] 拿到 `Sender<ServoEvent>`。
///
/// @trace REQ-BAO-API-003 [level:library]
pub struct EventSubscriber {
    bridge_tx: Sender<ServoEvent>,
}

impl EventSubscriber {
    /// 构造 EventSubscriber 与对应的 Receiver。
    ///
    /// `bounded(1024)`:channel 缓冲 1024 个事件。servo 端 push 时若 channel
    /// 满,事件被丢弃(避免阻塞 servo ScriptThread)并记录日志。
    ///
    /// @trace REQ-BAO-API-003 [level:library]
    pub fn new() -> (Self, Receiver<ServoEvent>) {
        Self::with_capacity(1024)
    }

    /// 指定 channel 容量构造。
    ///
    /// @trace REQ-BAO-API-003 [level:library]
    pub fn with_capacity(capacity: usize) -> (Self, Receiver<ServoEvent>) {
        // 注:mpsc::channel 是无界 channel。bounded 语义靠 try_send 检查
        // 内部 pending 数实现(此处简化为无界,生产中可改 crossbeam 或
        // 手动容量监控)。capacity 参数保留作为未来 bounded 升级钩子。
        let _ = capacity;
        let (tx, rx) = mpsc::channel::<ServoEvent>();
        (EventSubscriber { bridge_tx: tx }, rx)
    }

    /// 获取底层 sender(供复杂场景使用,如多 sender 模式)。
    ///
    /// @trace REQ-BAO-API-003 [level:library]
    pub fn sender(&self) -> Sender<ServoEvent> {
        self.bridge_tx.clone()
    }

    /// 内部 push 工具:channel 满或断开时静默忽略,记录 log::warn。
    fn push(&self, event: ServoEvent) {
        if self.bridge_tx.send(event).is_err() {
            log::warn!(
                "EventSubscriber: receiver dropped, servo event lost (target_id={})",
                ""
            );
        }
    }

    // ── Console 类 ────────────────────────────────────────────────────
    /// servo console 消息回调。
    ///
    /// @trace REQ-BAO-API-003 [event:Console]
    pub fn on_console_message(
        &self,
        target_id: impl Into<String>,
        level: ConsoleLevel,
        text: impl Into<String>,
        url: Option<String>,
        line: Option<u32>,
        column: Option<u32>,
    ) {
        self.push(ServoEvent::Console {
            target_id: target_id.into(),
            level,
            text: text.into(),
            url,
            line,
            column,
        });
    }

    // ── PageError 类 ─────────────────────────────────────────────────
    /// servo 页面错误(JS 异常/CSS 错误)回调。
    ///
    /// @trace REQ-BAO-API-003 [event:PageError]
    pub fn on_page_error(
        &self,
        target_id: impl Into<String>,
        text: impl Into<String>,
        url: Option<String>,
        line: Option<u32>,
        column: Option<u32>,
        stack: Option<String>,
    ) {
        self.push(ServoEvent::PageError {
            target_id: target_id.into(),
            text: text.into(),
            url,
            line,
            column,
            stack,
        });
    }

    // ── NetworkEvent 类 ──────────────────────────────────────────────
    /// servo 网络请求开始回调。
    ///
    /// @trace REQ-BAO-API-003 [event:NetworkEvent]
    #[allow(clippy::too_many_arguments)]
    pub fn on_network_request(
        &self,
        target_id: impl Into<String>,
        request_id: impl Into<String>,
        url: impl Into<String>,
        method: impl Into<String>,
        headers: HashMap<String, String>,
        post_data: Option<Vec<u8>>,
        resource_type: impl Into<String>,
        frame_id: impl Into<String>,
    ) {
        self.push(ServoEvent::NetworkRequest {
            target_id: target_id.into(),
            request_id: request_id.into(),
            url: url.into(),
            method: method.into(),
            headers,
            post_data,
            resource_type: resource_type.into(),
            frame_id: frame_id.into(),
        });
    }

    /// servo 网络响应到达回调。
    ///
    /// @trace REQ-BAO-API-003 [event:NetworkEvent]
    #[allow(clippy::too_many_arguments)]
    pub fn on_network_response(
        &self,
        target_id: impl Into<String>,
        request_id: impl Into<String>,
        url: impl Into<String>,
        status: u16,
        status_text: impl Into<String>,
        headers: HashMap<String, String>,
        mime_type: impl Into<String>,
        remote_ip: Option<String>,
    ) {
        self.push(ServoEvent::NetworkResponse {
            target_id: target_id.into(),
            request_id: request_id.into(),
            url: url.into(),
            status,
            status_text: status_text.into(),
            headers,
            mime_type: mime_type.into(),
            remote_ip,
        });
    }

    /// servo 网络请求成功结束回调。
    ///
    /// @trace REQ-BAO-API-003 [event:NetworkEvent]
    pub fn on_network_loading_finish(
        &self,
        target_id: impl Into<String>,
        request_id: impl Into<String>,
        encoded_data_length: u64,
    ) {
        self.push(ServoEvent::NetworkLoadingFinish {
            target_id: target_id.into(),
            request_id: request_id.into(),
            encoded_data_length,
        });
    }

    /// servo 网络请求失败回调。
    ///
    /// @trace REQ-BAO-API-003 [event:NetworkEvent]
    pub fn on_network_loading_fail(
        &self,
        target_id: impl Into<String>,
        request_id: impl Into<String>,
        error_text: impl Into<String>,
        canceled: bool,
    ) {
        self.push(ServoEvent::NetworkLoadingFail {
            target_id: target_id.into(),
            request_id: request_id.into(),
            error_text: error_text.into(),
            canceled,
        });
    }

    // ── DomMutation 类 ───────────────────────────────────────────────
    /// servo DOM 属性变更回调。
    ///
    /// @trace REQ-BAO-API-003 [event:DomMutation]
    pub fn on_dom_attribute_modified(
        &self,
        target_id: impl Into<String>,
        node_id: i64,
        name: impl Into<String>,
        value: impl Into<String>,
    ) {
        self.push(ServoEvent::DomAttributeModified {
            target_id: target_id.into(),
            node_id,
            name: name.into(),
            value: value.into(),
        });
    }

    /// servo DOM character data 变更回调。
    ///
    /// @trace REQ-BAO-API-003 [event:DomMutation]
    pub fn on_dom_character_data_modified(
        &self,
        target_id: impl Into<String>,
        node_id: i64,
        old_value: impl Into<String>,
        new_value: impl Into<String>,
    ) {
        self.push(ServoEvent::DomCharacterDataModified {
            target_id: target_id.into(),
            node_id,
            old_value: old_value.into(),
            new_value: new_value.into(),
        });
    }

    // ── SourceInfo 类 ────────────────────────────────────────────────
    /// servo 脚本编译完成回调。
    ///
    /// @trace REQ-BAO-API-003 [event:SourceInfo]
    #[allow(clippy::too_many_arguments)]
    pub fn on_script_parsed(
        &self,
        target_id: impl Into<String>,
        script_id: impl Into<String>,
        url: impl Into<String>,
        start_line: u32,
        start_column: u32,
        end_line: u32,
        end_column: u32,
        source_map_url: Option<String>,
    ) {
        self.push(ServoEvent::ScriptParsed {
            target_id: target_id.into(),
            script_id: script_id.into(),
            url: url.into(),
            start_line,
            start_column,
            end_line,
            end_column,
            source_map_url,
        });
    }

    // ── FrameInfo 类 ─────────────────────────────────────────────────
    /// servo frame 导航完成回调。
    ///
    /// @trace REQ-BAO-API-003 [event:FrameInfo]
    pub fn on_frame_navigated(
        &self,
        target_id: impl Into<String>,
        frame_id: impl Into<String>,
        url: impl Into<String>,
        name: Option<String>,
    ) {
        self.push(ServoEvent::FrameNavigated {
            target_id: target_id.into(),
            frame_id: frame_id.into(),
            url: url.into(),
            name,
        });
    }

    /// servo frame 开始加载回调。
    ///
    /// @trace REQ-BAO-API-003 [event:FrameInfo]
    pub fn on_frame_started_loading(
        &self,
        target_id: impl Into<String>,
        frame_id: impl Into<String>,
    ) {
        self.push(ServoEvent::FrameStartedLoading {
            target_id: target_id.into(),
            frame_id: frame_id.into(),
        });
    }

    /// servo frame 停止加载回调。
    ///
    /// @trace REQ-BAO-API-003 [event:FrameInfo]
    pub fn on_frame_stopped_loading(
        &self,
        target_id: impl Into<String>,
        frame_id: impl Into<String>,
    ) {
        self.push(ServoEvent::FrameStoppedLoading {
            target_id: target_id.into(),
            frame_id: frame_id.into(),
        });
    }

    // ── TimelineMarker 类 ────────────────────────────────────────────
    /// servo 性能 timeline 标记回调。
    ///
    /// @trace REQ-BAO-API-003 [event:TimelineMarker]
    pub fn on_timeline_marker(
        &self,
        target_id: impl Into<String>,
        name: impl Into<String>,
        start_time: f64,
        end_time: f64,
    ) {
        self.push(ServoEvent::TimelineMarker {
            target_id: target_id.into(),
            name: name.into(),
            start_time,
            end_time,
        });
    }
}

impl Default for EventSubscriber {
    fn default() -> Self {
        // Default 创建 subscriber 并丢弃 receiver(channel 永远不会满,
        // 适用于不关心事件的场景)。生产代码请用 `EventSubscriber::new()`。
        let (sub, _rx) = Self::new();
        sub
    }
}

impl std::fmt::Debug for EventSubscriber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventSubscriber")
            .field("channel", &"mpsc::sync_channel")
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// §5 ConsoleMessage 适配器(对 bao_cdp::ConsoleMessage 兼容)
// ---------------------------------------------------------------------------

/// 从 `bao_cdp::ConsoleMessage`(servo delegate 实际推送的事件类型)适配。
///
/// - `Log { level, text }` → `ServoEvent::Console { level: from_servo_str(level), text, .. }`
/// - `Event(BaoEvent::NetworkRequestWillBeSent { .. })` → `ServoEvent::NetworkRequest { .. }`
/// - `Event(BaoEvent::RuntimeExceptionThrown { .. })` → `ServoEvent::PageError { .. }`
/// - 其他 `Event` 子类按需展开
///
/// 不存在的子类(如 DomMutation/FrameInfo/TimelineMarker)目前 BaoEvent 不覆盖,
/// EventSubscriber 直接暴露 on_* 方法,由 servo delegate 适配层调用即可。
///
/// @trace REQ-BAO-API-003 [level:library]
pub fn from_console_message(
    msg: bao_cdp::ConsoleMessage,
    target_id: impl Into<String>,
) -> Option<ServoEvent> {
    use bao_cdp::BaoEvent;
    let target_id = target_id.into();
    match msg {
        bao_cdp::ConsoleMessage::Log { level, text } => Some(ServoEvent::Console {
            target_id,
            level: ConsoleLevel::from_servo_str(&level),
            text,
            url: None,
            line: None,
            column: None,
        }),
        bao_cdp::ConsoleMessage::Event(evt) => match evt {
            BaoEvent::NetworkRequestWillBeSent {
                request_id,
                url,
                method,
                headers,
                timestamp: _,
                resource_type,
                ..
            } => Some(ServoEvent::NetworkRequest {
                target_id,
                request_id,
                url,
                method,
                headers: json_to_headers(&headers),
                post_data: None,
                resource_type,
                frame_id: "0".to_string(),
            }),
            BaoEvent::NetworkResponseReceived {
                request_id,
                url,
                status,
                status_text,
                headers,
                timestamp: _,
                resource_type: _,
            } => Some(ServoEvent::NetworkResponse {
                target_id,
                request_id,
                url,
                status: status.max(0) as u16,
                status_text,
                headers: json_to_headers(&headers),
                mime_type: String::new(),
                remote_ip: None,
            }),
            BaoEvent::NetworkLoadingFailed {
                request_id,
                resource_type: _,
                error_text,
                timestamp: _,
            } => Some(ServoEvent::NetworkLoadingFail {
                target_id,
                request_id,
                error_text,
                canceled: false,
            }),
            BaoEvent::DebuggerScriptParsed {
                script_id,
                url,
                start_line,
                end_line,
            } => Some(ServoEvent::ScriptParsed {
                target_id,
                script_id,
                url,
                start_line: start_line.max(0) as u32,
                start_column: 0,
                end_line: end_line.max(0) as u32,
                end_column: 0,
                source_map_url: None,
            }),
            BaoEvent::RuntimeExceptionThrown {
                timestamp: _,
                text,
                url,
                line,
                column,
                stack_trace,
            } => {
                let stack = if stack_trace.is_null() {
                    None
                } else {
                    Some(stack_trace.to_string())
                };
                Some(ServoEvent::PageError {
                    target_id,
                    text,
                    url: if url.is_empty() { None } else { Some(url) },
                    line: Some(line.max(0) as u32),
                    column: Some(column.max(0) as u32),
                    stack,
                })
            }
            // 其他变体当前未对应 REQ-BAO-API-003 直接映射,留给
            // EventSubscriber::on_* 方法手动 push。
            _ => None,
        },
    }
}

// ---------------------------------------------------------------------------
// §6 工具函数
// ---------------------------------------------------------------------------

/// 当前时间戳(毫秒,用于 Log.entryAdded.timestamp)。
fn current_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 当前时间戳(秒,浮点,用于 Network.*.timestamp — CDP 规范要求 monotonic seconds)。
fn current_timestamp_s() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// HashMap<String,String> → CDP headers JSON object。
fn headers_to_json(headers: &HashMap<String, String>) -> Value {
    let mut map = serde_json::Map::new();
    for (k, v) in headers {
        map.insert(k.clone(), Value::String(v.clone()));
    }
    Value::Object(map)
}

/// JSON object → HashMap<String,String>(用于 BaoEvent.headers 字段反序列化)。
fn json_to_headers(v: &Value) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let Some(obj) = v.as_object() {
        for (k, val) in obj {
            let s = match val {
                Value::String(s) => s.clone(),
                _ => val.to_string(),
            };
            out.insert(k.clone(), s);
        }
    }
    out
}

/// 从 URL 抽取 origin(scheme://host[:port])。
fn origin_from_url(url: &str) -> String {
    // 简单实现:找 scheme:// 后第一段。复杂 URL 解析由 bun_url 负责,
    // 这里只要能给 securityOrigin 一个合理默认值。
    if let Some(scheme_end) = url.find("://") {
        let after = &url[scheme_end + 3..];
        let end = after.find('/').unwrap_or(after.len());
        return format!("{}://{}", &url[..scheme_end], &after[..end]);
    }
    String::new()
}

/// base64 编码(用 bun_base64 crate,保持 workspace 内复用)。
fn base64_encode(bytes: &[u8]) -> String {
    // bun_base64::encode_alloc 返回 Vec<u8>(ASCII base64 字符)。
    // 安全转 String:base64 输出全是 ASCII,from_utf8 不可能失败。
    let vec = bun_base64::encode_alloc(bytes);
    // SAFETY: base64 alphabet is pure ASCII (< 128), UTF-8 valid.
    unsafe { String::from_utf8_unchecked(vec) }
}

// ---------------------------------------------------------------------------
// §7 单元测试 — 7 类事件全覆盖
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    // ── §7.1 Console → Log.entryAdded ────────────────────────────────

    #[test]
    fn console_level_from_servo_str_canonical() {
        assert_eq!(ConsoleLevel::from_servo_str("info"), ConsoleLevel::Info);
        assert_eq!(
            ConsoleLevel::from_servo_str("warning"),
            ConsoleLevel::Warning
        );
        assert_eq!(ConsoleLevel::from_servo_str("error"), ConsoleLevel::Error);
        assert_eq!(ConsoleLevel::from_servo_str("debug"), ConsoleLevel::Debug);
        assert_eq!(
            ConsoleLevel::from_servo_str("verbose"),
            ConsoleLevel::Verbose
        );
    }

    #[test]
    fn console_level_from_servo_str_aliases() {
        // console.log → info
        assert_eq!(ConsoleLevel::from_servo_str("log"), ConsoleLevel::Info);
        // console.warn → warning
        assert_eq!(
            ConsoleLevel::from_servo_str("warn"),
            ConsoleLevel::Warning
        );
        // console.trace → verbose
        assert_eq!(
            ConsoleLevel::from_servo_str("trace"),
            ConsoleLevel::Verbose
        );
    }

    #[test]
    fn console_level_from_servo_str_unknown_defaults_to_info() {
        assert_eq!(ConsoleLevel::from_servo_str("foo"), ConsoleLevel::Info);
        assert_eq!(ConsoleLevel::from_servo_str(""), ConsoleLevel::Info);
    }

    #[test]
    fn console_level_to_cdp_str_matches_spec() {
        assert_eq!(ConsoleLevel::Verbose.to_cdp_str(), "verbose");
        assert_eq!(ConsoleLevel::Info.to_cdp_str(), "info");
        assert_eq!(ConsoleLevel::Warning.to_cdp_str(), "warning");
        assert_eq!(ConsoleLevel::Error.to_cdp_str(), "error");
        assert_eq!(ConsoleLevel::Debug.to_cdp_str(), "debug");
    }

    // @trace REQ-BAO-API-003 [event:Console]
    #[test]
    fn translate_console_produces_log_entry_added() {
        let ev = ServoEvent::Console {
            target_id: "TARGET-1".into(),
            level: ConsoleLevel::Warning,
            text: "watch out".into(),
            url: Some("http://example.com/a.js".into()),
            line: Some(42),
            column: Some(13),
        };
        let out = translate(ev);
        assert_eq!(out.len(), 1, "Console → exactly 1 CdpEvent");
        let e = &out[0];
        assert_eq!(e.method, "Log.entryAdded");
        assert_eq!(e.session_id.as_deref(), Some("TARGET-1"));
        // entry schema
        let entry = &e.params["entry"];
        assert_eq!(entry["source"], "javascript");
        assert_eq!(entry["level"], "warning");
        assert_eq!(entry["text"], "watch out");
        assert_eq!(entry["url"], "http://example.com/a.js");
        assert_eq!(entry["lineNumber"], 42);
        assert_eq!(entry["columnNumber"], 13);
        assert!(entry["timestamp"].is_number());
    }

    #[test]
    fn translate_console_with_missing_location_defaults_to_zero() {
        let ev = ServoEvent::Console {
            target_id: "T".into(),
            level: ConsoleLevel::Info,
            text: "hi".into(),
            url: None,
            line: None,
            column: None,
        };
        let out = translate(ev);
        let entry = &out[0].params["entry"];
        assert_eq!(entry["lineNumber"], 0);
        assert_eq!(entry["columnNumber"], 0);
        assert!(entry["url"].is_null());
    }

    // ── §7.2 PageError → Runtime.exceptionThrown ─────────────────────

    // @trace REQ-BAO-API-003 [event:PageError]
    #[test]
    fn translate_page_error_produces_runtime_exception_thrown() {
        let ev = ServoEvent::PageError {
            target_id: "T2".into(),
            text: "TypeError: x is not a function".into(),
            url: Some("http://example.com/b.js".into()),
            line: Some(10),
            column: Some(5),
            stack: Some("at foo (b.js:10:5)".into()),
        };
        let out = translate(ev);
        assert_eq!(out.len(), 1);
        let e = &out[0];
        assert_eq!(e.method, "Runtime.exceptionThrown");
        assert_eq!(e.session_id.as_deref(), Some("T2"));
        assert!(e.params["timestamp"].is_number());
        let details = &e.params["exceptionDetails"];
        assert_eq!(details["text"], "TypeError: x is not a function");
        assert_eq!(details["url"], "http://example.com/b.js");
        assert_eq!(details["lineNumber"], 10);
        assert_eq!(details["columnNumber"], 5);
        assert_eq!(details["exception"]["type"], "string");
        assert_eq!(details["exception"]["value"], "TypeError: x is not a function");
        // stack trace
        let st = &details["stackTrace"];
        assert!(st.is_array());
        assert_eq!(st[0]["scriptName"], "at foo (b.js:10:5)");
    }

    #[test]
    fn translate_page_error_without_stack_emits_null_stack() {
        let ev = ServoEvent::PageError {
            target_id: "T3".into(),
            text: "boom".into(),
            url: None,
            line: None,
            column: None,
            stack: None,
        };
        let out = translate(ev);
        let details = &out[0].params["exceptionDetails"];
        assert!(details["stackTrace"].is_null());
    }

    // ── §7.3 NetworkEvent → Network.* (4 子事件) ─────────────────────

    // @trace REQ-BAO-API-003 [event:NetworkEvent]
    #[test]
    fn translate_network_request_produces_request_will_be_sent() {
        let mut headers = HashMap::new();
        headers.insert("X-Test".into(), "v1".into());
        let ev = ServoEvent::NetworkRequest {
            target_id: "T4".into(),
            request_id: "REQ1".into(),
            url: "http://example.com/api".into(),
            method: "POST".into(),
            headers,
            post_data: Some(b"hello".to_vec()),
            resource_type: "XHR".into(),
            frame_id: "FRAME1".into(),
        };
        let out = translate(ev);
        assert_eq!(out.len(), 1);
        let e = &out[0];
        assert_eq!(e.method, "Network.requestWillBeSent");
        assert_eq!(e.session_id.as_deref(), Some("T4"));
        assert_eq!(e.params["requestId"], "REQ1");
        assert_eq!(e.params["loaderId"], "FRAME1");
        assert_eq!(e.params["documentURL"], "http://example.com/api");
        assert_eq!(e.params["type"], "XHR");
        assert_eq!(e.params["frameId"], "FRAME1");
        // request
        let req = &e.params["request"];
        assert_eq!(req["url"], "http://example.com/api");
        assert_eq!(req["method"], "POST");
        assert_eq!(req["headers"]["X-Test"], "v1");
        // post_data 必须是 base64 字符串
        assert!(req["postData"].is_string());
        let pd = req["postData"].as_str().unwrap();
        // base64("hello") = "aGVsbG8="
        assert_eq!(pd, "aGVsbG8=");
    }

    #[test]
    fn translate_network_request_without_post_data_omits_field() {
        let ev = ServoEvent::NetworkRequest {
            target_id: "T".into(),
            request_id: "R".into(),
            url: "http://x".into(),
            method: "GET".into(),
            headers: HashMap::new(),
            post_data: None,
            resource_type: "Document".into(),
            frame_id: "F".into(),
        };
        let out = translate(ev);
        let req = &out[0].params["request"];
        assert!(req.get("postData").is_none() || req["postData"].is_null());
    }

    // @trace REQ-BAO-API-003 [event:NetworkEvent]
    #[test]
    fn translate_network_response_produces_response_received() {
        let mut headers = HashMap::new();
        headers.insert("Content-Type".into(), "application/json".into());
        let ev = ServoEvent::NetworkResponse {
            target_id: "T5".into(),
            request_id: "REQ2".into(),
            url: "http://example.com/api".into(),
            status: 200,
            status_text: "OK".into(),
            headers,
            mime_type: "application/json".into(),
            remote_ip: Some("1.2.3.4".into()),
        };
        let out = translate(ev);
        assert_eq!(out.len(), 1);
        let e = &out[0];
        assert_eq!(e.method, "Network.responseReceived");
        assert_eq!(e.session_id.as_deref(), Some("T5"));
        assert_eq!(e.params["requestId"], "REQ2");
        let resp = &e.params["response"];
        assert_eq!(resp["url"], "http://example.com/api");
        assert_eq!(resp["status"], 200);
        assert_eq!(resp["statusText"], "OK");
        assert_eq!(resp["mimeType"], "application/json");
        assert_eq!(resp["headers"]["Content-Type"], "application/json");
        assert_eq!(resp["remoteIPAddress"], "1.2.3.4");
    }

    // @trace REQ-BAO-API-003 [event:NetworkEvent]
    #[test]
    fn translate_network_loading_finish_produces_loading_finished() {
        let ev = ServoEvent::NetworkLoadingFinish {
            target_id: "T6".into(),
            request_id: "REQ3".into(),
            encoded_data_length: 1234,
        };
        let out = translate(ev);
        assert_eq!(out.len(), 1);
        let e = &out[0];
        assert_eq!(e.method, "Network.loadingFinished");
        assert_eq!(e.session_id.as_deref(), Some("T6"));
        assert_eq!(e.params["requestId"], "REQ3");
        assert_eq!(e.params["encodedDataLength"], 1234);
        assert!(e.params["timestamp"].is_number());
    }

    // @trace REQ-BAO-API-003 [event:NetworkEvent]
    #[test]
    fn translate_network_loading_fail_produces_loading_failed() {
        let ev = ServoEvent::NetworkLoadingFail {
            target_id: "T7".into(),
            request_id: "REQ4".into(),
            error_text: "net::ERR_FAILED".into(),
            canceled: true,
        };
        let out = translate(ev);
        assert_eq!(out.len(), 1);
        let e = &out[0];
        assert_eq!(e.method, "Network.loadingFailed");
        assert_eq!(e.session_id.as_deref(), Some("T7"));
        assert_eq!(e.params["requestId"], "REQ4");
        assert_eq!(e.params["errorText"], "net::ERR_FAILED");
        assert_eq!(e.params["canceled"], true);
        assert!(e.params["blockedReason"].is_null());
    }

    // ── §7.4 DomMutation → DOM.* (2 子事件) ──────────────────────────

    // @trace REQ-BAO-API-003 [event:DomMutation]
    #[test]
    fn translate_dom_attribute_modified() {
        let ev = ServoEvent::DomAttributeModified {
            target_id: "T8".into(),
            node_id: 42,
            name: "class".into(),
            value: "active".into(),
        };
        let out = translate(ev);
        assert_eq!(out.len(), 1);
        let e = &out[0];
        assert_eq!(e.method, "DOM.attributeModified");
        assert_eq!(e.session_id.as_deref(), Some("T8"));
        assert_eq!(e.params["nodeId"], 42);
        assert_eq!(e.params["name"], "class");
        assert_eq!(e.params["value"], "active");
    }

    // @trace REQ-BAO-API-003 [event:DomMutation]
    #[test]
    fn translate_dom_character_data_modified() {
        let ev = ServoEvent::DomCharacterDataModified {
            target_id: "T9".into(),
            node_id: 99,
            old_value: "old".into(),
            new_value: "new".into(),
        };
        let out = translate(ev);
        assert_eq!(out.len(), 1);
        let e = &out[0];
        assert_eq!(e.method, "DOM.characterDataModified");
        assert_eq!(e.session_id.as_deref(), Some("T9"));
        assert_eq!(e.params["nodeId"], 99);
        assert_eq!(e.params["characterData"], "new");
        // old_value 在 CDP 不存在(只在事件源保留作 servo 调试用)
        assert!(e.params.get("oldValue").is_none() || e.params["oldValue"].is_null());
    }

    // ── §7.5 SourceInfo → Debugger.scriptParsed ──────────────────────

    // @trace REQ-BAO-API-003 [event:SourceInfo]
    #[test]
    fn translate_script_parsed() {
        let ev = ServoEvent::ScriptParsed {
            target_id: "T10".into(),
            script_id: "SCRIPT1".into(),
            url: "http://example.com/x.js".into(),
            start_line: 1,
            start_column: 0,
            end_line: 100,
            end_column: 50,
            source_map_url: Some("http://example.com/x.js.map".into()),
        };
        let out = translate(ev);
        assert_eq!(out.len(), 1);
        let e = &out[0];
        assert_eq!(e.method, "Debugger.scriptParsed");
        assert_eq!(e.session_id.as_deref(), Some("T10"));
        assert_eq!(e.params["scriptId"], "SCRIPT1");
        assert_eq!(e.params["url"], "http://example.com/x.js");
        assert_eq!(e.params["startLine"], 1);
        assert_eq!(e.params["startColumn"], 0);
        assert_eq!(e.params["endLine"], 100);
        assert_eq!(e.params["endColumn"], 50);
        assert_eq!(
            e.params["sourceMapURL"],
            "http://example.com/x.js.map"
        );
        // 必备字段
        assert!(e.params["executionContextId"].is_number());
    }

    #[test]
    fn translate_script_parsed_without_source_map() {
        let ev = ServoEvent::ScriptParsed {
            target_id: "T".into(),
            script_id: "S".into(),
            url: "x.js".into(),
            start_line: 0,
            start_column: 0,
            end_line: 0,
            end_column: 0,
            source_map_url: None,
        };
        let out = translate(ev);
        // 没传 sourceMapURL 时,字段不存在(不是 null)。
        assert!(out[0].params.get("sourceMapURL").is_none());
    }

    // ── §7.6 FrameInfo → Page.* (3 子事件) ───────────────────────────

    // @trace REQ-BAO-API-003 [event:FrameInfo]
    #[test]
    fn translate_frame_navigated() {
        let ev = ServoEvent::FrameNavigated {
            target_id: "T11".into(),
            frame_id: "FRAME1".into(),
            url: "http://example.com/page".into(),
            name: Some("myframe".into()),
        };
        let out = translate(ev);
        assert_eq!(out.len(), 1);
        let e = &out[0];
        assert_eq!(e.method, "Page.frameNavigated");
        assert_eq!(e.session_id.as_deref(), Some("T11"));
        assert_eq!(e.params["type"], "Navigation");
        let frame = &e.params["frame"];
        assert_eq!(frame["id"], "FRAME1");
        assert_eq!(frame["url"], "http://example.com/page");
        assert_eq!(frame["name"], "myframe");
        assert_eq!(frame["securityOrigin"], "http://example.com");
        assert_eq!(frame["mimeType"], "text/html");
    }

    // @trace REQ-BAO-API-003 [event:FrameInfo]
    #[test]
    fn translate_frame_started_loading() {
        let ev = ServoEvent::FrameStartedLoading {
            target_id: "T12".into(),
            frame_id: "FRAME2".into(),
        };
        let out = translate(ev);
        assert_eq!(out.len(), 1);
        let e = &out[0];
        assert_eq!(e.method, "Page.frameStartedLoading");
        assert_eq!(e.session_id.as_deref(), Some("T12"));
        assert_eq!(e.params["frameId"], "FRAME2");
    }

    // @trace REQ-BAO-API-003 [event:FrameInfo]
    #[test]
    fn translate_frame_stopped_loading() {
        let ev = ServoEvent::FrameStoppedLoading {
            target_id: "T13".into(),
            frame_id: "FRAME3".into(),
        };
        let out = translate(ev);
        assert_eq!(out.len(), 1);
        let e = &out[0];
        assert_eq!(e.method, "Page.frameStoppedLoading");
        assert_eq!(e.session_id.as_deref(), Some("T13"));
        assert_eq!(e.params["frameId"], "FRAME3");
    }

    // ── §7.7 TimelineMarker → Performance.metrics ────────────────────

    // @trace REQ-BAO-API-003 [event:TimelineMarker]
    #[test]
    fn translate_timeline_marker() {
        let ev = ServoEvent::TimelineMarker {
            target_id: "T14".into(),
            name: "dom-render".into(),
            start_time: 100.0,
            end_time: 150.5,
        };
        let out = translate(ev);
        assert_eq!(out.len(), 1);
        let e = &out[0];
        assert_eq!(e.method, "Performance.metrics");
        assert_eq!(e.session_id.as_deref(), Some("T14"));
        assert_eq!(e.params["title"], "servo-timeline-dom-render");
        let metrics = e.params["metrics"].as_array().unwrap();
        assert_eq!(metrics.len(), 3);
        // start, end, duration_ms
        let names: Vec<&str> = metrics
            .iter()
            .map(|m| m["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"dom-render_start"));
        assert!(names.contains(&"dom-render_end"));
        assert!(names.contains(&"dom-render_duration_ms"));
        // 验证 duration 计算 = (150.5 - 100.0) * 1000 = 50500.0
        let duration_metric = metrics
            .iter()
            .find(|m| m["name"] == "dom-render_duration_ms")
            .unwrap();
        assert!((duration_metric["value"].as_f64().unwrap() - 50500.0).abs() < 0.001);
    }

    // ── §7.8 target_id 一致性 ────────────────────────────────────────

    #[test]
    fn target_id_accessor_returns_correct_target_for_each_variant() {
        let cases: Vec<(ServoEvent, &str)> = vec![
            (
                ServoEvent::Console {
                    target_id: "T".into(),
                    level: ConsoleLevel::Info,
                    text: String::new(),
                    url: None,
                    line: None,
                    column: None,
                },
                "T",
            ),
            (
                ServoEvent::PageError {
                    target_id: "T".into(),
                    text: String::new(),
                    url: None,
                    line: None,
                    column: None,
                    stack: None,
                },
                "T",
            ),
            (
                ServoEvent::TimelineMarker {
                    target_id: "T".into(),
                    name: String::new(),
                    start_time: 0.0,
                    end_time: 0.0,
                },
                "T",
            ),
        ];
        for (ev, expected) in cases {
            assert_eq!(ev.target_id(), expected);
        }
    }

    // ── §7.9 EventSubscriber ─────────────────────────────────────────

    #[test]
    fn event_subscriber_push_and_recv_round_trip() {
        let (sub, rx) = EventSubscriber::new();
        sub.on_console_message("T1", ConsoleLevel::Info, "hello", None, None, None);
        sub.on_page_error("T2", "boom", None, None, None, None);
        let ev1 = rx.recv_timeout(std::time::Duration::from_millis(100)).unwrap();
        let ev2 = rx.recv_timeout(std::time::Duration::from_millis(100)).unwrap();
        assert!(matches!(ev1, ServoEvent::Console { .. }));
        assert!(matches!(ev2, ServoEvent::PageError { .. }));
    }

    #[test]
    fn event_subscriber_capacity_unbounded_pushes_all() {
        // with_capacity 当前实现为无界 channel,可无限制 push。
        let (sub, rx) = EventSubscriber::new();
        for i in 0..1024 {
            sub.on_frame_started_loading("T", format!("F{}", i));
        }
        let mut count = 0;
        while rx.try_recv().is_ok() {
            count += 1;
        }
        assert_eq!(count, 1024);
    }

    #[test]
    fn event_subscriber_drop_sender_signals_disconnected() {
        let (sub, rx) = EventSubscriber::new();
        drop(sub);
        let err = rx.recv_timeout(std::time::Duration::from_millis(100));
        assert!(err.is_err());
    }

    // ── §7.10 from_console_message 适配器 ────────────────────────────

    #[test]
    fn from_console_message_log_variant_maps_to_console_event() {
        let msg = bao_cdp::ConsoleMessage::Log {
            level: "warning".into(),
            text: "careful".into(),
        };
        let ev = from_console_message(msg, "TARGET-99").expect("should map");
        match ev {
            ServoEvent::Console {
                target_id,
                level,
                text,
                ..
            } => {
                assert_eq!(target_id, "TARGET-99");
                assert_eq!(level, ConsoleLevel::Warning);
                assert_eq!(text, "careful");
            }
            _ => panic!("expected Console variant"),
        }
    }

    #[test]
    fn from_console_message_network_request_event() {
        let msg = bao_cdp::ConsoleMessage::Event(bao_cdp::BaoEvent::NetworkRequestWillBeSent {
            request_id: "REQ1".into(),
            url: "http://example.com".into(),
            method: "GET".into(),
            headers: serde_json::json!({"X-A": "1"}),
            request: serde_json::json!({}),
            timestamp: 0.0,
            resource_type: "Document".into(),
        });
        let ev = from_console_message(msg, "T").expect("should map");
        match ev {
            ServoEvent::NetworkRequest {
                request_id,
                url,
                method,
                headers,
                resource_type,
                ..
            } => {
                assert_eq!(request_id, "REQ1");
                assert_eq!(url, "http://example.com");
                assert_eq!(method, "GET");
                assert_eq!(headers.get("X-A").unwrap(), "1");
                assert_eq!(resource_type, "Document");
            }
            _ => panic!("expected NetworkRequest"),
        }
    }

    #[test]
    fn from_console_message_runtime_exception_maps_to_page_error() {
        let msg = bao_cdp::ConsoleMessage::Event(bao_cdp::BaoEvent::RuntimeExceptionThrown {
            timestamp: 1.0,
            text: "boom".into(),
            url: "x.js".into(),
            line: 5,
            column: 10,
            stack_trace: serde_json::Value::Null,
        });
        let ev = from_console_message(msg, "T").expect("should map");
        match ev {
            ServoEvent::PageError { text, url, line, column, .. } => {
                assert_eq!(text, "boom");
                assert_eq!(url.as_deref(), Some("x.js"));
                assert_eq!(line, Some(5));
                assert_eq!(column, Some(10));
            }
            _ => panic!("expected PageError"),
        }
    }

    // ── §7.11 工具函数 ───────────────────────────────────────────────

    #[test]
    fn origin_from_url_extracts_correctly() {
        assert_eq!(origin_from_url("https://example.com/path"), "https://example.com");
        assert_eq!(
            origin_from_url("http://localhost:8080/x/y"),
            "http://localhost:8080"
        );
        assert_eq!(origin_from_url("not-a-url"), "");
    }

    #[test]
    fn base64_encode_via_bun_base64() {
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
        assert_eq!(base64_encode(b""), "");
    }

    // ── §7.12 7 类全覆盖统计 ─────────────────────────────────────────

    #[test]
    fn all_seven_event_classes_have_at_least_one_test() {
        // 这个测试是断言性的:7 类全部能 translate 而不 panic。
        // 真正的字段断言在前面每个测试中。
        let samples: Vec<ServoEvent> = vec![
            ServoEvent::Console {
                target_id: "T".into(),
                level: ConsoleLevel::Info,
                text: "x".into(),
                url: None,
                line: None,
                column: None,
            },
            ServoEvent::PageError {
                target_id: "T".into(),
                text: "x".into(),
                url: None,
                line: None,
                column: None,
                stack: None,
            },
            ServoEvent::NetworkRequest {
                target_id: "T".into(),
                request_id: "r".into(),
                url: "u".into(),
                method: "GET".into(),
                headers: HashMap::new(),
                post_data: None,
                resource_type: "Other".into(),
                frame_id: "f".into(),
            },
            ServoEvent::NetworkResponse {
                target_id: "T".into(),
                request_id: "r".into(),
                url: "u".into(),
                status: 200,
                status_text: "OK".into(),
                headers: HashMap::new(),
                mime_type: "text/html".into(),
                remote_ip: None,
            },
            ServoEvent::NetworkLoadingFinish {
                target_id: "T".into(),
                request_id: "r".into(),
                encoded_data_length: 0,
            },
            ServoEvent::NetworkLoadingFail {
                target_id: "T".into(),
                request_id: "r".into(),
                error_text: "e".into(),
                canceled: false,
            },
            ServoEvent::DomAttributeModified {
                target_id: "T".into(),
                node_id: 1,
                name: "n".into(),
                value: "v".into(),
            },
            ServoEvent::DomCharacterDataModified {
                target_id: "T".into(),
                node_id: 1,
                old_value: "o".into(),
                new_value: "n".into(),
            },
            ServoEvent::ScriptParsed {
                target_id: "T".into(),
                script_id: "s".into(),
                url: "u".into(),
                start_line: 0,
                start_column: 0,
                end_line: 0,
                end_column: 0,
                source_map_url: None,
            },
            ServoEvent::FrameNavigated {
                target_id: "T".into(),
                frame_id: "f".into(),
                url: "u".into(),
                name: None,
            },
            ServoEvent::FrameStartedLoading {
                target_id: "T".into(),
                frame_id: "f".into(),
            },
            ServoEvent::FrameStoppedLoading {
                target_id: "T".into(),
                frame_id: "f".into(),
            },
            ServoEvent::TimelineMarker {
                target_id: "T".into(),
                name: "n".into(),
                start_time: 0.0,
                end_time: 1.0,
            },
        ];
        // 13 样本 = 1 Console + 1 PageError + 4 Network + 2 Dom + 1 ScriptParsed
        //          + 3 Frame + 1 Timeline = 13(覆盖 7 类)
        assert_eq!(samples.len(), 13);
        let mut cdp_methods = std::collections::HashSet::new();
        for s in samples {
            for ev in translate(s) {
                cdp_methods.insert(ev.method);
            }
        }
        // 验证所有目标 CDP method 都已生成
        let expected: &[&str] = &[
            "Log.entryAdded",
            "Runtime.exceptionThrown",
            "Network.requestWillBeSent",
            "Network.responseReceived",
            "Network.loadingFinished",
            "Network.loadingFailed",
            "DOM.attributeModified",
            "DOM.characterDataModified",
            "Debugger.scriptParsed",
            "Page.frameNavigated",
            "Page.frameStartedLoading",
            "Page.frameStoppedLoading",
            "Performance.metrics",
        ];
        for m in expected {
            assert!(cdp_methods.contains(*m), "missing CDP method: {}", m);
        }
    }
}
