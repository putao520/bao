//! `BridgeSenderBackend` — `ServoBackend` 的生产通道实现(M1 接线,REQ-CDP-001)。
//!
//! e152 审计 DUP-CDP-PARALLEL:`bridge/` 派发宇宙(command_dispatcher 193
//! method + CDPRdpBridge + ServoBackend)此前唯一实现是 `MockServoBackend`,
//! 生产不可达。用户裁决「留并接线」——本文件把该宇宙的方法面挂接到
//! **既有生产链**:
//!
//! ```text
//!   Browser::connect("memory://bao") / WS CDP client
//!       ↓
//!   MemoryCdpBridge / BaoWsRegistry(bao_browser)
//!       ↓ ① bao_cdp::handle_command(生产命令核,-32601 时)
//!       ↓ ② CDPRdpBridge::dispatch(死宇宙命令核 fallback)
//!       ↓
//!   ServoBackend = BridgeSenderBackend(本文件)
//!       ↓ BridgeCommand(生产通道枚举,零第二命令核)
//!   BridgeSender → 主线程 cdp_handler(servo 真值,单一后端)
//! ```
//!
//! # 映射纪律
//!
//! - 有对应 `BridgeCommand` 变体的 trait 方法 → 直映射(响应 Value → struct 解析)。
//! - B 类高频合成所需(frame_tree / layout_metrics)→ `EvaluateJs` 合成,
//!   表达式与生产 `protocol.rs` 的同名合成逐字一致(单一语义源)。
//! - 无通道且无法诚实合成的(navigation history 枚举、touch、geolocation、
//!   session minting …)→ `BridgeError::NotSupported`(显式失败,禁假成功)。
//!
//! # Debugger 统一(BUG-CDP-006)
//!
//! 生产 `handle_debugger` 与本宇宙 `debugger_handlers` 双套此前各自为政;
//! 接线后两套都终结于同一组 `Debugger*` BridgeCommand → 同一 servo SM
//! Debugger API(`DevtoolScriptControlMsg`)——通道级统一,单一真值后端。
//!
//! @trace REQ-CDP-001 [level:library]
//! @trace REQ-BAO-API-004 [level:library]

use bao_cdp::servo_bridge::{BridgeCommand, BridgeSender};
use serde_json::Value;

use super::error::BridgeError;
use super::servo_backend::{
    BreakpointResult, CSSComputedStyleProperty, CSSProperty, CSSStyle, DebugStepAction,
    DebuggerEvalResult, DebuggerRemoteObject, DeviceMetrics, EvaluateResult, ExceptionDetails,
    Frame, FrameTree, KeyEvent, LayoutMetrics, MatchedRule, MatchedStyles, MouseEvent,
    NavigateResult, NodeDescriptor, PropertyDescriptor, PossibleBreakpoint, RemoteObject,
    ResponseBody, TargetInfo, BridgeScreenshotFormat,
};

/// servo 后端的生产通道实现。
///
/// 每个 trait 方法构造对应的 [`BridgeCommand`] 经 [`BridgeSender`] 送往
/// 主线程(`BrowserRuntime` 泵 / WS worker drain 的同一接收端),响应
/// `Value` 在此解析为 backend struct。`Send + Sync`(通道句柄天然满足)。
///
/// @trace REQ-CDP-001 [level:library]
/// @trace REQ-BAO-API-004 [level:library]
pub struct BridgeSenderBackend {
    sender: BridgeSender,
}

impl BridgeSenderBackend {
    /// 构造 backend(通道 sender 由宿主 runtime 创建并与生产命令核共享)。
    ///
    /// @trace REQ-CDP-001 [level:library]
    pub fn new(sender: BridgeSender) -> Self {
        Self { sender }
    }

    /// 发送命令并把响应归一为 `Ok(Value)` / `Err(BridgeError)`。
    fn send(&self, cmd: BridgeCommand) -> Result<Value, BridgeError> {
        let resp = self.sender.send(cmd);
        resp.result
            .map_err(|e| BridgeError::ServoError(format!("bridge: {e}")))
    }

    /// 发送只需成功确认的命令(`{}` 响应)。
    fn send_ok(&self, cmd: BridgeCommand) -> Result<(), BridgeError> {
        self.send(cmd).map(|_| ())
    }

    /// 无生产通道且无法诚实合成的方法 — 显式 NotSupported。
    fn not_supported(method: &str, reason: &str) -> BridgeError {
        BridgeError::NotSupported(format!("{method}: {reason}"))
    }

    /// `EvaluateJs`(returnByValue)合成辅助:表达式必须返回
    /// `JSON.stringify(...)`;envelope `result.value` 是 JSON 编码字符串,
    /// 剥壳为对象(与生产 `eval_json` 的 W19a 剥壳语义一致)。
    fn eval_json_doc(&self, target_id: &str, expression: &str) -> Result<Value, BridgeError> {
        let envelope = self.send(BridgeCommand::EvaluateJs {
            target_id: target_id.to_string(),
            expression: expression.to_string(),
            return_by_value: true,
        })?;
        let value = envelope
            .get("result")
            .and_then(|r| r.get("value"))
            .cloned()
            .ok_or_else(|| {
                BridgeError::ServoError(format!(
                    "synthesis query failed: no result.value (expr: {expression:.80})"
                ))
            })?;
        match value {
            Value::String(ref text) => serde_json::from_str::<Value>(text).map_err(|e| {
                BridgeError::ServoError(format!(
                    "synthesis query returned unparseable JSON: {e} (expr: {expression:.80})"
                ))
            }),
            other => Ok(other),
        }
    }

    /// evaluate envelope(`{result: RemoteObject-ish, exceptionDetails?}`)
    /// → `EvaluateResult`。`cmd_evaluate` / `cmd_runtime_call_function_on`
    /// 双路径共用此形状。
    fn parse_envelope(envelope: &Value) -> EvaluateResult {
        let result = envelope.get("result").cloned().unwrap_or(Value::Null);
        let exception = envelope.get("exceptionDetails").filter(|d| !d.is_null());
        EvaluateResult {
            result: Self::parse_remote_object(&result),
            exception_details: exception.map(Self::parse_exception_details),
        }
    }

    /// `window.__bao_cdp.wrap(...)` 产出的 RemoteObject 形状 → struct。
    fn parse_remote_object(v: &Value) -> RemoteObject {
        RemoteObject {
            object_id: v
                .get("objectId")
                .and_then(|s| s.as_str())
                .map(|s| s.to_string()),
            type_: v
                .get("type")
                .and_then(|s| s.as_str())
                .unwrap_or("undefined")
                .to_string(),
            subtype: v
                .get("subtype")
                .and_then(|s| s.as_str())
                .map(|s| s.to_string()),
            value: v.get("value").filter(|x| !x.is_null()).cloned(),
            unserializable_value: v
                .get("unserializableValue")
                .and_then(|s| s.as_str())
                .map(|s| s.to_string()),
            class_name: v
                .get("className")
                .and_then(|s| s.as_str())
                .map(|s| s.to_string()),
            description: v
                .get("description")
                .and_then(|s| s.as_str())
                .map(|s| s.to_string()),
        }
    }

    /// CDP exceptionDetails 形状 → struct。
    fn parse_exception_details(v: &Value) -> ExceptionDetails {
        ExceptionDetails {
            exception_id: v.get("exceptionId").and_then(|n| n.as_i64()).unwrap_or(0),
            text: v
                .get("text")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
            line_number: v.get("lineNumber").and_then(|n| n.as_i64()).unwrap_or(0),
            column_number: v.get("columnNumber").and_then(|n| n.as_i64()).unwrap_or(0),
            exception: v.get("exception").map(Self::parse_remote_object),
        }
    }

    /// cdp_handler 的 node 树(`nodeId/backendNodeId/nodeName/nodeValue/
    /// children`)→ `NodeDescriptor`(递归)。
    fn parse_node_descriptor(v: &Value) -> NodeDescriptor {
        NodeDescriptor {
            node_id: v.get("nodeId").and_then(|n| n.as_i64()).unwrap_or(0),
            node_name: v
                .get("nodeName")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
            node_value: v
                .get("nodeValue")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
            backend_node_id: v
                .get("backendNodeId")
                .and_then(|n| n.as_i64())
                .unwrap_or(0),
            children: v
                .get("children")
                .and_then(|c| c.as_array())
                .map(|arr| arr.iter().map(Self::parse_node_descriptor).collect())
                .unwrap_or_default(),
        }
    }

    /// cdp_handler 的 CSS property 数组 → `Vec<CSSProperty>`。
    fn parse_css_properties(v: Option<&Value>) -> Vec<CSSProperty> {
        v.and_then(|p| p.get("cssProperties"))
            .and_then(|p| p.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|p| CSSProperty {
                        name: p
                            .get("name")
                            .and_then(|s| s.as_str())
                            .unwrap_or("")
                            .to_string(),
                        value: p
                            .get("value")
                            .and_then(|s| s.as_str())
                            .unwrap_or("")
                            .to_string(),
                        important: p
                            .get("important")
                            .and_then(|b| b.as_bool())
                            .unwrap_or(false),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// screenshot format → 通道字符串。
    fn screenshot_format_name(format: BridgeScreenshotFormat) -> &'static str {
        match format {
            BridgeScreenshotFormat::Jpeg => "jpeg",
            BridgeScreenshotFormat::Png => "png",
            BridgeScreenshotFormat::Webp => "webp",
        }
    }
}

impl super::servo_backend::ServoBackend for BridgeSenderBackend {
    // ──────────────────────────────────────────────────────────────────
    // Page domain
    // ──────────────────────────────────────────────────────────────────

    fn page_navigate(&self, target_id: &str, url: &str) -> Result<NavigateResult, BridgeError> {
        let resp = self.send(BridgeCommand::Navigate {
            target_id: target_id.to_string(),
            url: url.to_string(),
        })?;
        Ok(NavigateResult {
            frame_id: resp
                .get("frameId")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
            loader_id: resp
                .get("loaderId")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
            error_text: None,
        })
    }

    fn page_reload(&self, target_id: &str) -> Result<(), BridgeError> {
        self.send_ok(BridgeCommand::Reload {
            target_id: target_id.to_string(),
            ignore_cache: false,
        })
    }

    fn page_screenshot(
        &self,
        target_id: &str,
        format: BridgeScreenshotFormat,
    ) -> Result<Vec<u8>, BridgeError> {
        let resp = self.send(BridgeCommand::TakeScreenshot {
            target_id: target_id.to_string(),
            format: Self::screenshot_format_name(format).to_string(),
            quality: None,
        })?;
        let b64 = resp
            .get("data")
            .and_then(|s| s.as_str())
            .ok_or_else(|| BridgeError::ServoError("screenshot: no data field".into()))?;
        bun_base64::decode_alloc(b64.as_bytes())
            .map_err(|e| BridgeError::ServoError(format!("screenshot base64 decode: {e:?}")))
    }

    fn page_frame_tree(&self, target_id: &str) -> Result<FrameTree, BridgeError> {
        // 与生产 handle_page "getFrameTree" 的合成表达式逐字一致(单一语义源)。
        let mut frame = self.eval_json_doc(
            target_id,
            r#"(function(){ return JSON.stringify({
                url: location.href,
                mimeType: document.contentType,
                name: window.name,
                securityOrigin: location.origin
            }); })()"#,
        )?;
        let frame_id =
            bao_cdp::servo_bridge::main_frame_id_for_target(target_id);
        if let Some(obj) = frame.as_object_mut() {
            obj.insert("id".into(), Value::String(frame_id));
        }
        Ok(FrameTree {
            frame: Frame {
                id: frame
                    .get("id")
                    .and_then(|s| s.as_str())
                    .unwrap_or_default()
                    .to_string(),
                parent_id: None,
                loader_id: String::new(),
                name: frame
                    .get("name")
                    .and_then(|s| s.as_str())
                    .map(|s| s.to_string()),
                url: frame
                    .get("url")
                    .and_then(|s| s.as_str())
                    .unwrap_or_default()
                    .to_string(),
                security_origin: frame
                    .get("securityOrigin")
                    .and_then(|s| s.as_str())
                    .unwrap_or_default()
                    .to_string(),
                mime_type: frame
                    .get("mimeType")
                    .and_then(|s| s.as_str())
                    .unwrap_or_default()
                    .to_string(),
            },
            // 子 frame 无法从嵌入者枚举(与生产同边界),不伪造。
            child_frames: Vec::new(),
        })
    }

    fn page_navigation_history(&self, _target_id: &str) -> Result<super::servo_backend::NavigationHistory, BridgeError> {
        // 生产同判:not_supported(servo WebView 无 session-history 枚举)。
        Err(Self::not_supported(
            "Page.getNavigationHistory",
            "servo WebView does not expose session-history entry enumeration",
        ))
    }

    fn page_navigate_to_history_entry(&self, _target_id: &str, _entry_id: i64) -> Result<(), BridgeError> {
        Err(Self::not_supported(
            "Page.navigateToHistoryEntry",
            "no bridge channel for history-entry jumping (GoBack/GoForward only)",
        ))
    }

    fn page_set_content(&self, target_id: &str, html: &str) -> Result<(), BridgeError> {
        // 与生产 handle_page "setContent" 同形(document.open/write/close)。
        let js = format!(
            r#"(function() {{ document.open(); document.write({}); document.close(); }})()"#,
            serde_json::to_string(html).unwrap_or_default(),
        );
        self.send_ok(BridgeCommand::EvaluateJs {
            target_id: target_id.to_string(),
            expression: js,
            return_by_value: true,
        })
    }

    fn page_close(&self, target_id: &str) -> Result<(), BridgeError> {
        self.send_ok(BridgeCommand::ClosePage {
            target_id: target_id.to_string(),
        })
    }

    fn page_bring_to_front(&self, target_id: &str) -> Result<(), BridgeError> {
        // 与生产 handle_page "bringToFront" 同形(window.focus())。
        self.send_ok(BridgeCommand::EvaluateJs {
            target_id: target_id.to_string(),
            expression: "window.focus()".into(),
            return_by_value: true,
        })
    }

    fn page_layout_metrics(&self, target_id: &str) -> Result<LayoutMetrics, BridgeError> {
        // 与生产 handle_page "getLayoutMetrics" 的合成表达式逐字一致。
        let m = self.eval_json_doc(
            target_id,
            r#"(function(){ var d = document.documentElement;
                return JSON.stringify({
                    iw: window.innerWidth, ih: window.innerHeight,
                    cw: d.scrollWidth, ch: d.scrollHeight
                }); })()"#,
        )?;
        let num = |k: &str| m.get(k).and_then(|v| v.as_f64()).unwrap_or(0.0);
        Ok(LayoutMetrics {
            layout_width: num("iw"),
            layout_height: num("ih"),
            content_width: num("cw"),
            content_height: num("ch"),
        })
    }

    fn page_print_to_pdf(&self, _target_id: &str) -> Result<Vec<u8>, BridgeError> {
        // dispatcher 把 Page.printToPDF 归 E 类,本方法经生产入口不可达;
        // 直连库面同样诚实失败。
        Err(Self::not_supported(
            "Page.printToPDF",
            "servo has no PDF rendering pipeline",
        ))
    }

    // ──────────────────────────────────────────────────────────────────
    // Runtime domain
    // ──────────────────────────────────────────────────────────────────

    fn runtime_evaluate(&self, target_id: &str, expression: &str) -> Result<EvaluateResult, BridgeError> {
        let envelope = self.send(BridgeCommand::EvaluateJs {
            target_id: target_id.to_string(),
            expression: expression.to_string(),
            return_by_value: true,
        })?;
        Ok(Self::parse_envelope(&envelope))
    }

    fn runtime_call_function_on(
        &self,
        target_id: &str,
        object_id: &str,
        function_declaration: &str,
        args: &[Value],
    ) -> Result<EvaluateResult, BridgeError> {
        let envelope = self.send(BridgeCommand::RuntimeCallFunctionOn {
            target_id: target_id.to_string(),
            object_id: Some(object_id.to_string()),
            execution_context_id: None,
            function_declaration: function_declaration.to_string(),
            arguments: Some(Value::Array(args.to_vec())),
            return_by_value: Some(true),
            await_promise: None,
            object_group: None,
        })?;
        Ok(Self::parse_envelope(&envelope))
    }

    fn runtime_get_properties(
        &self,
        target_id: &str,
        object_id: &str,
        own_properties: bool,
    ) -> Result<Vec<PropertyDescriptor>, BridgeError> {
        let resp = self.send(BridgeCommand::RuntimeGetProperties {
            target_id: target_id.to_string(),
            object_id: object_id.to_string(),
            own_properties: Some(own_properties),
        })?;
        let arr = resp
            .get("result")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(arr
            .iter()
            .map(|p| PropertyDescriptor {
                name: p
                    .get("name")
                    .and_then(|s| s.as_str())
                    .unwrap_or("")
                    .to_string(),
                value: p.get("value").map(Self::parse_remote_object),
                writable: p.get("writable").and_then(|b| b.as_bool()),
                get: p.get("get").map(Self::parse_remote_object),
                set: p.get("set").map(Self::parse_remote_object),
                configurable: p.get("configurable").and_then(|b| b.as_bool()),
                enumerable: p.get("enumerable").and_then(|b| b.as_bool()),
                is_own: p
                    .get("isOwn")
                    .and_then(|b| b.as_bool())
                    .unwrap_or(own_properties),
                symbol: p.get("symbol").map(Self::parse_remote_object),
            })
            .collect())
    }

    fn runtime_release_object(&self, target_id: &str, object_id: &str) -> Result<(), BridgeError> {
        self.send_ok(BridgeCommand::RuntimeReleaseObject {
            target_id: target_id.to_string(),
            object_id: object_id.to_string(),
        })
    }

    fn runtime_enable(&self, _target_id: &str) -> Result<(), BridgeError> {
        // 状态切换 ack(生产 Runtime.enable 同语义,ok_empty)。
        Ok(())
    }

    fn runtime_disable(&self, _target_id: &str) -> Result<(), BridgeError> {
        Ok(())
    }

    // ──────────────────────────────────────────────────────────────────
    // DOM domain
    // ──────────────────────────────────────────────────────────────────

    fn dom_get_document(&self, target_id: &str, _depth: i64) -> Result<NodeDescriptor, BridgeError> {
        let resp = self.send(BridgeCommand::GetDocument {
            target_id: target_id.to_string(),
        })?;
        let root = resp.get("root").cloned().unwrap_or(Value::Null);
        Ok(Self::parse_node_descriptor(&root))
    }

    fn dom_query_selector(
        &self,
        target_id: &str,
        _node_id: i64,
        selector: &str,
    ) -> Result<Option<i64>, BridgeError> {
        let resp = self.send(BridgeCommand::QuerySelector {
            target_id: target_id.to_string(),
            selector: selector.to_string(),
        })?;
        // 通道语义:nodeId 1=命中 / 0=未命中。
        Ok(match resp.get("nodeId").and_then(|n| n.as_i64()) {
            Some(0) | None => None,
            Some(n) => Some(n),
        })
    }

    fn dom_query_selector_all(
        &self,
        target_id: &str,
        _node_id: i64,
        selector: &str,
    ) -> Result<Vec<i64>, BridgeError> {
        let resp = self.send(BridgeCommand::QuerySelectorAll {
            target_id: target_id.to_string(),
            selector: selector.to_string(),
        })?;
        Ok(resp
            .get("nodeIds")
            .and_then(|n| n.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_i64()).collect())
            .unwrap_or_default())
    }

    fn dom_get_box_model(
        &self,
        _target_id: &str,
        node_id: i64,
    ) -> Result<super::servo_backend::BoxModel, BridgeError> {
        // 通道按 nodeId 寻址几何无诚实载体(nodeId 是合成占位,见
        // cdp_handler resolve_node_by_id 注释)——不伪造坐标。
        Err(Self::not_supported(
            "DOM.getBoxModel",
            &format!("no servo channel for box-model geometry by nodeId ({node_id})"),
        ))
    }

    fn dom_resolve_node(
        &self,
        _target_id: &str,
        _backend_node_id: i64,
    ) -> Result<RemoteObject, BridgeError> {
        Err(Self::not_supported(
            "DOM.resolveNode",
            "no servo channel for nodeId → RemoteObject resolution",
        ))
    }

    fn dom_describe_node(
        &self,
        target_id: &str,
        _node_id: i64,
        _depth: i64,
    ) -> Result<NodeDescriptor, BridgeError> {
        // 与 getDocument 同载体(整树描述,depth 裁剪在 handler 侧)。
        let resp = self.send(BridgeCommand::GetDocument {
            target_id: target_id.to_string(),
        })?;
        let root = resp.get("root").cloned().unwrap_or(Value::Null);
        Ok(Self::parse_node_descriptor(&root))
    }

    fn dom_set_attribute(
        &self,
        target_id: &str,
        _node_id: i64,
        name: &str,
        value: &str,
    ) -> Result<(), BridgeError> {
        self.send_ok(BridgeCommand::SetAttributeValue {
            target_id: target_id.to_string(),
            node_id: 0,
            name: name.to_string(),
            value: value.to_string(),
        })
    }

    fn dom_remove_attribute(
        &self,
        _target_id: &str,
        _node_id: i64,
        _name: &str,
    ) -> Result<(), BridgeError> {
        Err(Self::not_supported(
            "DOM.removeAttribute",
            "no bridge channel for attribute removal by nodeId",
        ))
    }

    fn dom_get_outer_html(&self, target_id: &str, _node_id: i64) -> Result<String, BridgeError> {
        let resp = self.send(BridgeCommand::GetOuterHtml {
            target_id: target_id.to_string(),
            node_id: None,
        })?;
        Ok(resp
            .get("outerHTML")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string())
    }

    fn dom_set_outer_html(
        &self,
        _target_id: &str,
        _node_id: i64,
        _html: &str,
    ) -> Result<(), BridgeError> {
        Err(Self::not_supported(
            "DOM.setOuterHTML",
            "no bridge channel for outerHTML replacement by nodeId",
        ))
    }

    fn dom_request_node(&self, _target_id: &str, _object_id: &str) -> Result<i64, BridgeError> {
        Err(Self::not_supported(
            "DOM.requestNode",
            "no bridge channel for objectId → nodeId materialization",
        ))
    }

    // ──────────────────────────────────────────────────────────────────
    // Network domain
    // ──────────────────────────────────────────────────────────────────

    fn network_enable(&self, target_id: &str) -> Result<(), BridgeError> {
        self.send_ok(BridgeCommand::NetworkEnable {
            target_id: target_id.to_string(),
        })
    }

    fn network_disable(&self, target_id: &str) -> Result<(), BridgeError> {
        self.send_ok(BridgeCommand::NetworkDisable {
            target_id: target_id.to_string(),
        })
    }

    fn network_get_response_body(
        &self,
        target_id: &str,
        request_id: &str,
    ) -> Result<ResponseBody, BridgeError> {
        // 通道对端诚实报错(servo 不向嵌入者暴露存储的响应体)——错误
        // 原样穿透,不吞成空 body 假成功。
        let resp = self.send(BridgeCommand::GetResponseBody {
            target_id: target_id.to_string(),
            request_id: request_id.to_string(),
        })?;
        Ok(ResponseBody {
            body: resp
                .get("body")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
            base64_encoded: resp
                .get("base64Encoded")
                .and_then(|b| b.as_bool())
                .unwrap_or(false),
        })
    }

    fn network_set_cache_disabled(&self, target_id: &str, disabled: bool) -> Result<(), BridgeError> {
        self.send_ok(BridgeCommand::NetworkSetCacheDisabled {
            target_id: target_id.to_string(),
            cache_disabled: disabled,
        })
    }

    // ──────────────────────────────────────────────────────────────────
    // Input domain
    // ──────────────────────────────────────────────────────────────────

    fn input_dispatch_mouse_event(
        &self,
        target_id: &str,
        event: MouseEvent,
    ) -> Result<(), BridgeError> {
        self.send_ok(BridgeCommand::DispatchMouseEvent {
            target_id: target_id.to_string(),
            event_type: event.event_type,
            x: event.x,
            y: event.y,
            button: if event.button.is_empty() {
                None
            } else {
                Some(event.button)
            },
            click_count: Some(event.click_count),
        })
    }

    fn input_dispatch_key_event(
        &self,
        target_id: &str,
        event: KeyEvent,
    ) -> Result<(), BridgeError> {
        self.send_ok(BridgeCommand::DispatchKeyEvent {
            target_id: target_id.to_string(),
            event_type: event.event_type,
            key: event.key,
            code: event.code,
            text: if event.text.is_empty() {
                None
            } else {
                Some(event.text)
            },
            modifiers: event.modifiers as u32,
            location: 0,
            repeat: false,
        })
    }

    fn input_dispatch_touch_event(
        &self,
        _target_id: &str,
        _event_type: &str,
        _touch_points: &[super::servo_backend::TouchPoint],
    ) -> Result<(), BridgeError> {
        Err(Self::not_supported(
            "Input.dispatchTouchEvent",
            "no bridge channel for touch event synthesis",
        ))
    }

    fn input_set_ignore_input_events(&self, _target_id: &str, _ignore: bool) -> Result<(), BridgeError> {
        Err(Self::not_supported(
            "Input.setIgnoreInputEvents",
            "no bridge channel for input suppression",
        ))
    }

    // ──────────────────────────────────────────────────────────────────
    // Emulation domain
    // ──────────────────────────────────────────────────────────────────

    fn emulation_set_device_metrics(
        &self,
        target_id: &str,
        metrics: DeviceMetrics,
    ) -> Result<(), BridgeError> {
        self.send_ok(BridgeCommand::SetViewport {
            target_id: target_id.to_string(),
            width: metrics.width.max(0) as u32,
            height: metrics.height.max(0) as u32,
            device_scale_factor: Some(metrics.device_scale_factor),
        })
    }

    fn emulation_clear_device_metrics(&self, _target_id: &str) -> Result<(), BridgeError> {
        Err(Self::not_supported(
            "Emulation.clearDeviceMetricsOverride",
            "no bridge channel for metrics-override clearing",
        ))
    }

    fn emulation_set_user_agent_override(
        &self,
        target_id: &str,
        user_agent: &str,
    ) -> Result<(), BridgeError> {
        self.send_ok(BridgeCommand::SetUserAgent {
            target_id: target_id.to_string(),
            user_agent: user_agent.to_string(),
        })
    }

    fn emulation_set_geolocation_override(
        &self,
        _target_id: &str,
        _latitude: f64,
        _longitude: f64,
        _accuracy: f64,
    ) -> Result<(), BridgeError> {
        Err(Self::not_supported(
            "Emulation.setGeolocationOverride",
            "no bridge channel for geolocation override",
        ))
    }

    // ──────────────────────────────────────────────────────────────────
    // Target domain
    // ──────────────────────────────────────────────────────────────────

    fn target_get_targets(&self) -> Result<Vec<TargetInfo>, BridgeError> {
        // 通道返回数组形态([{id,title,url}] — cmd_list_targets 单一形态)。
        let resp = self.send(BridgeCommand::ListTargets)?;
        let entries = resp.as_array().cloned().unwrap_or_default();
        Ok(entries
            .iter()
            .map(|e| TargetInfo {
                target_id: e
                    .get("id")
                    .and_then(|s| s.as_str())
                    .unwrap_or("")
                    .to_string(),
                type_: "page".to_string(),
                title: e
                    .get("title")
                    .and_then(|s| s.as_str())
                    .unwrap_or("")
                    .to_string(),
                url: e
                    .get("url")
                    .and_then(|s| s.as_str())
                    .unwrap_or("")
                    .to_string(),
                // 与生产 live_target_info 同判:attached=true(内存客户端
                // 天然附着)。
                attached: true,
                browser_context_id: Some("bao-default-context".to_string()),
            })
            .collect())
    }

    fn target_create_target(&self, url: &str) -> Result<String, BridgeError> {
        let resp = self.send(BridgeCommand::CreateTarget {
            url: url.to_string(),
        })?;
        Ok(resp
            .get("targetId")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string())
    }

    fn target_close_target(&self, target_id: &str) -> Result<(), BridgeError> {
        self.send_ok(BridgeCommand::ClosePage {
            target_id: target_id.to_string(),
        })
    }

    fn target_attach_to_target(&self, _target_id: &str) -> Result<String, BridgeError> {
        // sessionId 铸造的唯一真源是 WS session registry(bao_browser);
        // 生产 Target.attachToTarget 同判 not_supported。禁伪造 session。
        Err(Self::not_supported(
            "Target.attachToTarget",
            "session minting requires the WS session registry; the bridge channel has no session table",
        ))
    }

    fn target_detach_from_target(&self, _session_id: &str) -> Result<(), BridgeError> {
        Err(Self::not_supported(
            "Target.detachFromTarget",
            "session routing requires the WS session registry",
        ))
    }

    fn target_set_auto_attach(
        &self,
        _target_id: &str,
        _auto_attach: bool,
        _wait_for_debugger_on_start: bool,
    ) -> Result<(), BridgeError> {
        // 订阅 ack(生产 Target.setAutoAttach 同语义 ok_empty;WS registry
        // 持有 auto_attach 状态的会话面在生产入口先答)。
        Ok(())
    }

    // ──────────────────────────────────────────────────────────────────
    // CSS domain
    // ──────────────────────────────────────────────────────────────────

    fn css_get_computed_style_for_node(
        &self,
        target_id: &str,
        node_id: i64,
    ) -> Result<Vec<CSSComputedStyleProperty>, BridgeError> {
        let resp = self.send(BridgeCommand::CssGetComputedStyleForNode {
            target_id: target_id.to_string(),
            node_id,
        })?;
        Ok(resp
            .get("computedStyle")
            .and_then(|c| c.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|p| CSSComputedStyleProperty {
                        name: p
                            .get("name")
                            .and_then(|s| s.as_str())
                            .unwrap_or("")
                            .to_string(),
                        value: p
                            .get("value")
                            .and_then(|s| s.as_str())
                            .unwrap_or("")
                            .to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    fn css_get_matched_styles_for_node(
        &self,
        target_id: &str,
        node_id: i64,
    ) -> Result<MatchedStyles, BridgeError> {
        let resp = self.send(BridgeCommand::CssGetMatchedStylesForNode {
            target_id: target_id.to_string(),
            node_id,
        })?;
        let inline_style = resp.get("inlineStyle").filter(|v| !v.is_null()).map(|s| CSSStyle {
            style_sheet_id: String::new(),
            css_properties: Self::parse_css_properties(Some(s)),
        });
        let attributes_style = resp
            .get("attributesStyle")
            .filter(|v| !v.is_null())
            .map(|s| CSSStyle {
                style_sheet_id: String::new(),
                css_properties: Self::parse_css_properties(Some(s)),
            });
        let matched_rules = resp
            .get("matchedCSSRules")
            .and_then(|m| m.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|entry| {
                        let rule = entry.get("rule").cloned().unwrap_or(Value::Null);
                        MatchedRule {
                            selector: rule
                                .pointer("/selectorList/selectors/0/text")
                                .and_then(|s| s.as_str())
                                .unwrap_or("")
                                .to_string(),
                            style: CSSStyle {
                                style_sheet_id: String::new(),
                                // cssProperties lives under rule.style (the
                                // inline/attributes faces carry it at the top
                                // level — parse_css_properties takes the node
                                // that OWNS the array).
                                css_properties: Self::parse_css_properties(rule.get("style")),
                            },
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(MatchedStyles {
            inline_style,
            attributes_style,
            matched_rules,
        })
    }

    // ──────────────────────────────────────────────────────────────────
    // Debugger domain(BUG-CDP-006;通道级统一见模块文档)
    // ──────────────────────────────────────────────────────────────────

    fn debugger_enable(&self, target_id: &str) -> Result<(), BridgeError> {
        self.send_ok(BridgeCommand::DebuggerEnable {
            target_id: target_id.to_string(),
        })
    }

    fn debugger_disable(&self, target_id: &str) -> Result<(), BridgeError> {
        self.send_ok(BridgeCommand::DebuggerDisable {
            target_id: target_id.to_string(),
        })
    }

    fn debugger_set_breakpoint_by_url(
        &self,
        target_id: &str,
        url: &str,
        line: u32,
        column: u32,
    ) -> Result<BreakpointResult, BridgeError> {
        let resp = self.send(BridgeCommand::DebuggerSetBreakpoint {
            target_id: target_id.to_string(),
            url: Some(url.to_string()),
            url_regex: None,
            line,
            column: Some(column),
        })?;
        let loc = resp
            .pointer("/locations/0")
            .cloned()
            .unwrap_or(Value::Null);
        let script_id = loc
            .get("scriptId")
            .and_then(|s| s.as_str())
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        Ok(BreakpointResult {
            breakpoint_id: resp
                .get("breakpointId")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string(),
            script_id,
            actual_line: loc
                .get("lineNumber")
                .and_then(|n| n.as_u64())
                .unwrap_or(line as u64) as u32,
            actual_column: loc
                .get("columnNumber")
                .and_then(|n| n.as_u64())
                .unwrap_or(column as u64) as u32,
        })
    }

    fn debugger_remove_breakpoint(
        &self,
        target_id: &str,
        script_id: u32,
        line: u32,
        column: u32,
    ) -> Result<(), BridgeError> {
        // 通道对端铸造格式:'bp-{sid}-{line}-{col}'(cmd_debugger_set_breakpoint)。
        self.send_ok(BridgeCommand::DebuggerRemoveBreakpoint {
            target_id: target_id.to_string(),
            breakpoint_id: format!("bp-{script_id}-{line}-{column}"),
        })
    }

    fn debugger_pause(&self, target_id: &str) -> Result<(), BridgeError> {
        self.send_ok(BridgeCommand::DebuggerInterrupt {
            target_id: target_id.to_string(),
        })
    }

    fn debugger_resume(
        &self,
        target_id: &str,
        step_action: Option<DebugStepAction>,
    ) -> Result<(), BridgeError> {
        // DebugStepAction::servo_resume_limit 与生产 stepOver/Into/Out 的
        // step_type 字面同源(next/step/finish)。
        self.send_ok(BridgeCommand::DebuggerResume {
            target_id: target_id.to_string(),
            step_type: step_action.map(|a| a.servo_resume_limit().to_string()),
        })
    }

    fn debugger_evaluate_on_call_frame(
        &self,
        target_id: &str,
        call_frame_id: &str,
        expression: &str,
    ) -> Result<DebuggerEvalResult, BridgeError> {
        // 通道对端走 cmd_evaluate(rbv)envelope(与 Runtime.evaluate 同形)。
        let envelope = self.send(BridgeCommand::DebuggerEval {
            target_id: target_id.to_string(),
            expression: expression.to_string(),
            frame_actor_id: Some(call_frame_id.to_string()),
        })?;
        let exception = envelope
            .get("exceptionDetails")
            .filter(|d| !d.is_null())
            .is_some();
        let remote = envelope
            .get("result")
            .map(Self::parse_remote_object)
            .unwrap_or_default();
        Ok(DebuggerEvalResult {
            result: DebuggerRemoteObject {
                type_: remote.type_,
                object_id: remote.object_id,
                class_name: remote.class_name,
                description: remote.description,
                value: remote.value,
            },
            has_exception: exception,
        })
    }

    fn debugger_get_possible_breakpoints(
        &self,
        target_id: &str,
        script_id: u32,
    ) -> Result<Vec<PossibleBreakpoint>, BridgeError> {
        let resp = self.send(BridgeCommand::DebuggerGetPossibleBreakpoints {
            target_id: target_id.to_string(),
            start_script_id: script_id.to_string(),
        })?;
        Ok(resp
            .get("locations")
            .and_then(|l| l.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|l| PossibleBreakpoint {
                        script_id: l
                            .get("scriptId")
                            .and_then(|s| s.as_str())
                            .and_then(|s| s.parse::<u32>().ok())
                            .unwrap_or(0),
                        line_number: l
                            .get("lineNumber")
                            .and_then(|n| n.as_u64())
                            .unwrap_or(0) as u32,
                        column_number: l
                            .get("columnNumber")
                            .and_then(|n| n.as_u64())
                            .unwrap_or(0) as u32,
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    fn debugger_get_script_source(
        &self,
        target_id: &str,
        script_id: u32,
    ) -> Result<String, BridgeError> {
        let resp = self.send(BridgeCommand::DebuggerGetScriptSource {
            target_id: target_id.to_string(),
            script_id,
        })?;
        Ok(resp
            .get("scriptSource")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string())
    }
}

// @trace TEST-CDP-001 [req:REQ-CDP-001] [level:unit]
#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::servo_backend::ServoBackend;
    use bao_cdp::servo_bridge::{bridge_channel, BridgeResponse};
    use serde_json::json;
    use std::time::Duration;

    /// 起 responder 线程:按 method 面答复生产 cdp_handler 形状的响应。
    fn spawn_responder(
        rx: bao_cdp::servo_bridge::BridgeReceiver,
        answers: impl Fn(&BridgeCommand) -> Option<Value> + Send + 'static,
    ) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            while rx.recv_and_process(Duration::from_millis(200), |cmd| {
                BridgeResponse {
                    result: answers(&cmd).ok_or_else(|| "unanswered".to_string()),
                }
            }) {}
        })
    }

    fn backend_with(answers: impl Fn(&BridgeCommand) -> Option<Value> + Send + 'static) -> BridgeSenderBackend {
        let (sender, receiver) = bridge_channel(Duration::from_secs(2));
        std::mem::forget(spawn_responder(receiver, answers));
        BridgeSenderBackend::new(sender)
    }

    const TID: &str = "1";

    #[test]
    fn page_navigate_parses_frame_and_loader_ids() {
        let b = backend_with(|cmd| match cmd {
            BridgeCommand::Navigate { .. } => Some(json!({
                "frameId": "main-1", "loaderId": "loader-7"
            })),
            _ => None,
        });
        let r = b.page_navigate(TID, "https://x").unwrap();
        assert_eq!(r.frame_id, "main-1");
        assert_eq!(r.loader_id, "loader-7");
        assert!(r.error_text.is_none());
    }

    #[test]
    fn page_screenshot_decodes_base64_data() {
        let b = backend_with(|cmd| match cmd {
            BridgeCommand::TakeScreenshot { format, .. } => {
                assert_eq!(format, "png");
                Some(json!({ "data": "aGVsbG8=" })) // "hello"
            }
            _ => None,
        });
        let bytes = b.page_screenshot(TID, BridgeScreenshotFormat::Png).unwrap();
        assert_eq!(bytes, b"hello");
    }

    #[test]
    fn runtime_evaluate_parses_envelope() {
        let b = backend_with(|cmd| match cmd {
            BridgeCommand::EvaluateJs { expression, return_by_value, .. } => {
                assert!(return_by_value);
                assert!(expression.contains("document.title"));
                Some(json!({
                    "result": { "type": "string", "value": "Hello" },
                    "exceptionDetails": null
                }))
            }
            _ => None,
        });
        let r = b.runtime_evaluate(TID, "(function(){return document.title;})()").unwrap();
        assert_eq!(r.result.type_, "string");
        assert_eq!(r.result.value, Some(json!("Hello")));
        assert!(r.exception_details.is_none());
    }

    #[test]
    fn runtime_evaluate_parses_exception_details() {
        let b = backend_with(|cmd| match cmd {
            BridgeCommand::EvaluateJs { .. } => Some(json!({
                "result": { "type": "undefined" },
                "exceptionDetails": {
                    "exceptionId": 0,
                    "text": "TypeError: boom",
                    "lineNumber": 3,
                    "columnNumber": 5
                }
            })),
            _ => None,
        });
        let r = b.runtime_evaluate(TID, "boom()").unwrap();
        let d = r.exception_details.expect("exceptionDetails");
        assert_eq!(d.text, "TypeError: boom");
        assert_eq!(d.line_number, 3);
    }

    #[test]
    fn runtime_call_function_on_rides_production_channel() {
        let b = backend_with(|cmd| match cmd {
            BridgeCommand::RuntimeCallFunctionOn { object_id, return_by_value, .. } => {
                assert_eq!(object_id.as_deref(), Some("obj-1"));
                assert_eq!(*return_by_value, Some(true));
                Some(json!({
                    "result": { "type": "string", "value": "attr" },
                    "exceptionDetails": null
                }))
            }
            _ => None,
        });
        let r = b
            .runtime_call_function_on(TID, "obj-1", "function(){return 'attr';}", &[])
            .unwrap();
        assert_eq!(r.result.value, Some(json!("attr")));
    }

    #[test]
    fn dom_query_selector_maps_zero_to_none() {
        let b = backend_with(|cmd| match cmd {
            BridgeCommand::QuerySelector { selector, .. } => {
                assert_eq!(selector, "div");
                Some(json!({ "nodeId": 1 }))
            }
            _ => None,
        });
        assert_eq!(b.dom_query_selector(TID, 1, "div").unwrap(), Some(1));

        let b2 = backend_with(|cmd| match cmd {
            BridgeCommand::QuerySelector { .. } => Some(json!({ "nodeId": 0 })),
            _ => None,
        });
        assert_eq!(b2.dom_query_selector(TID, 1, "div").unwrap(), None);
    }

    #[test]
    fn dom_get_document_parses_node_tree_recursively() {
        let b = backend_with(|cmd| match cmd {
            BridgeCommand::GetDocument { .. } => Some(json!({
                "root": {
                    "nodeId": 1, "backendNodeId": 1,
                    "nodeName": "#document", "nodeValue": "",
                    "children": [
                        { "nodeId": 2, "backendNodeId": 2, "nodeName": "html", "nodeValue": "" }
                    ]
                }
            })),
            _ => None,
        });
        let doc = b.dom_get_document(TID, 1).unwrap();
        assert_eq!(doc.node_name, "#document");
        assert_eq!(doc.children.len(), 1);
        assert_eq!(doc.children[0].node_name, "html");
    }

    #[test]
    fn dom_get_outer_html_extracts_field() {
        let b = backend_with(|cmd| match cmd {
            BridgeCommand::GetOuterHtml { .. } => Some(json!({ "outerHTML": "<html></html>" })),
            _ => None,
        });
        assert_eq!(b.dom_get_outer_html(TID, 0).unwrap(), "<html></html>");
    }

    #[test]
    fn target_get_targets_parses_array_form() {
        let b = backend_with(|cmd| match cmd {
            BridgeCommand::ListTargets => Some(json!([
                { "id": "1", "title": "A", "url": "https://a" },
                { "id": "2", "title": "B", "url": "https://b" }
            ])),
            _ => None,
        });
        let ts = b.target_get_targets().unwrap();
        assert_eq!(ts.len(), 2);
        assert_eq!(ts[0].target_id, "1");
        assert_eq!(ts[1].url, "https://b");
        assert!(ts[0].attached);
    }

    #[test]
    fn target_create_target_returns_new_id() {
        let b = backend_with(|cmd| match cmd {
            BridgeCommand::CreateTarget { url } => {
                assert_eq!(url, "about:blank");
                Some(json!({ "targetId": "5" }))
            }
            _ => None,
        });
        assert_eq!(b.target_create_target("about:blank").unwrap(), "5");
    }

    #[test]
    fn emulation_set_device_metrics_rides_set_viewport() {
        let b = backend_with(|cmd| match cmd {
            BridgeCommand::SetViewport { width, height, .. } => {
                assert_eq!((*width, *height), (800, 600));
                Some(json!({}))
            }
            _ => None,
        });
        b.emulation_set_device_metrics(
            TID,
            DeviceMetrics { width: 800, height: 600, device_scale_factor: 1.0, mobile: false },
        )
        .unwrap();
    }

    #[test]
    fn css_computed_style_parses_entries() {
        let b = backend_with(|cmd| match cmd {
            BridgeCommand::CssGetComputedStyleForNode { node_id, .. } => {
                assert_eq!(*node_id, 3);
                Some(json!({ "computedStyle": [
                    { "name": "color", "value": "rgb(0, 0, 0)" },
                    { "name": "display", "value": "block" }
                ]}))
            }
            _ => None,
        });
        let props = b.css_get_computed_style_for_node(TID, 3).unwrap();
        assert_eq!(props.len(), 2);
        assert_eq!(props[0].name, "color");
    }

    #[test]
    fn css_matched_styles_parses_rules_and_inline() {
        let b = backend_with(|cmd| match cmd {
            BridgeCommand::CssGetMatchedStylesForNode { .. } => Some(json!({
                "matchedCSSRules": [{
                    "rule": {
                        "selectorList": { "selectors": [{ "text": ".a" }] },
                        "style": { "cssProperties": [{ "name": "color", "value": "red", "important": true }] }
                    },
                    "matchingSelectors": [0]
                }],
                "inlineStyle": { "cssProperties": [{ "name": "margin", "value": "0" }] },
                "attributesStyle": null
            })),
            _ => None,
        });
        let m = b.css_get_matched_styles_for_node(TID, 1).unwrap();
        assert_eq!(m.matched_rules.len(), 1);
        assert_eq!(m.matched_rules[0].selector, ".a");
        assert_eq!(m.matched_rules[0].style.css_properties[0].important, true);
        let inline = m.inline_style.expect("inlineStyle");
        assert_eq!(inline.css_properties[0].name, "margin");
        assert!(m.attributes_style.is_none());
    }

    #[test]
    fn debugger_set_breakpoint_parses_locations() {
        let b = backend_with(|cmd| match cmd {
            BridgeCommand::DebuggerSetBreakpoint { url, line, column, .. } => {
                assert_eq!(url.as_deref(), Some("x.js"));
                assert_eq!(*line, 10);
                assert_eq!(*column, Some(0));
                Some(json!({
                    "breakpointId": "bp-3-10-0",
                    "locations": [{ "scriptId": "3", "lineNumber": 10, "columnNumber": 0 }]
                }))
            }
            _ => None,
        });
        let bp = b.debugger_set_breakpoint_by_url(TID, "x.js", 10, 0).unwrap();
        assert_eq!(bp.breakpoint_id, "bp-3-10-0");
        assert_eq!(bp.script_id, 3);
        assert_eq!(bp.actual_line, 10);
    }

    #[test]
    fn debugger_remove_breakpoint_mints_channel_format() {
        let b = backend_with(|cmd| match cmd {
            BridgeCommand::DebuggerRemoveBreakpoint { breakpoint_id, .. } => {
                assert_eq!(breakpoint_id, "bp-3-10-0");
                Some(json!({}))
            }
            _ => None,
        });
        b.debugger_remove_breakpoint(TID, 3, 10, 0).unwrap();
    }

    #[test]
    fn debugger_resume_maps_step_actions() {
        let b = backend_with(|cmd| match cmd {
            BridgeCommand::DebuggerResume { step_type, .. } => {
                assert_eq!(step_type.as_deref(), Some("next"));
                Some(json!({}))
            }
            _ => None,
        });
        b.debugger_resume(TID, Some(DebugStepAction::Next)).unwrap();
    }

    #[test]
    fn debugger_eval_parses_envelope_and_exception_flag() {
        let b = backend_with(|cmd| match cmd {
            BridgeCommand::DebuggerEval { expression, frame_actor_id, .. } => {
                assert_eq!(expression, "x");
                assert_eq!(frame_actor_id.as_deref(), Some("frame-0"));
                Some(json!({
                    "result": { "type": "number", "value": 42 },
                    "exceptionDetails": { "exceptionId": 0, "text": "e" }
                }))
            }
            _ => None,
        });
        let r = b.debugger_evaluate_on_call_frame(TID, "frame-0", "x").unwrap();
        assert!(r.has_exception);
        assert_eq!(r.result.value, Some(json!(42)));
    }

    #[test]
    fn debugger_possible_breakpoints_and_script_source() {
        let b = backend_with(|cmd| match cmd {
            BridgeCommand::DebuggerGetPossibleBreakpoints { start_script_id, .. } => {
                assert_eq!(start_script_id, "2");
                Some(json!({ "locations": [
                    { "scriptId": "2", "lineNumber": 1, "columnNumber": 0 }
                ]}))
            }
            BridgeCommand::DebuggerGetScriptSource { script_id, .. } => {
                assert_eq!(*script_id, 2);
                Some(json!({ "scriptSource": "var x = 1;" }))
            }
            _ => None,
        });
        let locs = b.debugger_get_possible_breakpoints(TID, 2).unwrap();
        assert_eq!(locs.len(), 1);
        assert_eq!(locs[0].script_id, 2);
        assert_eq!(b.debugger_get_script_source(TID, 2).unwrap(), "var x = 1;");
    }

    #[test]
    fn channel_error_propagates_as_servo_error() {
        // 无 responder:bridge channel closed / timeout → ServoError(不吞)。
        let (sender, receiver) = bridge_channel(Duration::from_millis(50));
        drop(receiver);
        let b = BridgeSenderBackend::new(sender);
        let err = b.page_navigate(TID, "https://x").unwrap_err();
        assert!(matches!(err, BridgeError::ServoError(_)));
    }

    #[test]
    fn no_channel_methods_fail_closed_not_supported() {
        let b = backend_with(|_| None);
        assert!(matches!(
            b.page_navigation_history(TID).unwrap_err(),
            BridgeError::NotSupported(_)
        ));
        assert!(matches!(
            b.target_attach_to_target(TID).unwrap_err(),
            BridgeError::NotSupported(_)
        ));
        assert!(matches!(
            b.page_print_to_pdf(TID).unwrap_err(),
            BridgeError::NotSupported(_)
        ));
    }

    #[test]
    fn layout_metrics_synthesis_peels_json_string() {
        // B 类 Page.viewport 依赖 layout_metrics;envelope value 是
        // JSON.stringify 字符串(W19a 剥壳语义)。
        let b = backend_with(|cmd| match cmd {
            BridgeCommand::EvaluateJs { expression, .. } => {
                assert!(expression.contains("innerWidth"));
                Some(json!({
                    "result": {
                        "type": "string",
                        "value": "{\"iw\":800,\"ih\":600,\"cw\":1024,\"ch\":768}"
                    },
                    "exceptionDetails": null
                }))
            }
            _ => None,
        });
        let m = b.page_layout_metrics(TID).unwrap();
        assert_eq!(m.layout_width, 800.0);
        assert_eq!(m.content_height, 768.0);
    }

    #[test]
    fn frame_tree_synthesis_carries_main_frame_id() {
        let b = backend_with(|cmd| match cmd {
            BridgeCommand::EvaluateJs { expression, .. } => {
                assert!(expression.contains("location.href"));
                Some(json!({
                    "result": {
                        "type": "string",
                        "value": "{\"url\":\"https://a/\",\"mimeType\":\"text/html\",\"name\":\"\",\"securityOrigin\":\"https://a\"}"
                    },
                    "exceptionDetails": null
                }))
            }
            _ => None,
        });
        let t = b.page_frame_tree(TID).unwrap();
        assert_eq!(t.frame.id, "main-1");
        assert_eq!(t.frame.url, "https://a/");
        assert_eq!(t.frame.mime_type, "text/html");
        assert!(t.child_frames.is_empty());
    }

    #[test]
    fn network_get_response_body_passes_honest_error_through() {
        // 通道对端对 GetResponseBody 诚实报错(servo 不暴露存储响应体)。
        let (sender, receiver) = bridge_channel(Duration::from_secs(2));
        std::mem::forget(spawn_responder(receiver, |cmd| match cmd {
            BridgeCommand::GetResponseBody { .. } => None, // responder 答 Err
            _ => Some(json!({})),
        }));
        let b = BridgeSenderBackend::new(sender);
        let err = b.network_get_response_body(TID, "req-1").unwrap_err();
        assert!(matches!(err, BridgeError::ServoError(_)));
    }

    #[test]
    fn send_and_sync_static_assertion() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<BridgeSenderBackend>();
    }
}
