//! `CDPRdpBridge` — Chrome DevTools Protocol ↔ servo RDP 桥接核心。
//!
//! `CDPRdpBridge` 是 CDP 命令进入 servo 后端的统一入口:
//!
//! ```text
//!   CDP Client (jsonrpc)
//!        ↓
//!   InMemoryTransport / WebSocketTransport
//!        ↓
//!   CDPRdpBridge::dispatch_command (本模块)
//!        ↓
//!   command_dispatcher::dispatch_command
//!        ↓
//!   ServoBackend impl (PagePool / crossbeam channel / mock)
//! ```
//!
//! # 设计要点
//!
//! - **零 tokio**:同步 dispatch,所有 I/O 阻塞在 backend
//! - **Send + Sync**:实现 `InMemoryBridge` trait,可被 `Arc` 跨线程共享
//! - **target_id 抽象**:用 `&str` 标识 Page(对应 CDP `Target.targetId` 或 session_id),
//!   由 backend 决定如何映射到具体 servo 资源
//! - **错误转换**:`BridgeError` → `InMemoryBridgeResponse::Err`(JSON-RPC error.data)
//!
//! @trace REQ-BAO-API-004 [level:library]
//! @trace REQ-BAO-API-007 [level:library]

use std::sync::Arc;

use serde_json::Value;

use crate::transport::in_memory::{InMemoryBridge, InMemoryBridgeResponse};

use super::command_dispatcher;
use super::error::BridgeError;
use super::servo_backend::ServoBackend;

/// CDPRdpBridge — servo RDP 桥接核心。
///
/// 持有 `Arc<dyn ServoBackend>` 实现命令派发。
///
/// # 使用
///
/// ```ignore
/// use bao_cdp_client::bridge::{CDPRdpBridge, ServoBackend};
/// use bao_cdp_client::transport::InMemoryTransport;
/// use std::sync::Arc;
///
/// let backend: Arc<dyn ServoBackend> = /* ... */;
/// let bridge = CDPRdpBridge::new(backend);
/// let transport_bridge: Arc<dyn InMemoryBridge> = bridge.into_bridge();
/// let transport = InMemoryTransport::new(transport_bridge);
/// ```
///
/// @trace REQ-BAO-API-004 [level:library]
pub struct CDPRdpBridge {
    backend: Arc<dyn ServoBackend>,
}

impl CDPRdpBridge {
    /// 构造 bridge。
    ///
    /// @trace REQ-BAO-API-004 [level:library]
    pub fn new(backend: Arc<dyn ServoBackend>) -> Self {
        Self { backend }
    }

    /// 派发 CDP 命令。
    ///
    /// # 参数
    /// - `target_id`:目标 Page 标识(对应 session_id 或 targetId)
    /// - `method`:CDP method 名(如 `Page.navigate`)
    /// - `params`:JSON 参数
    ///
    /// @trace REQ-BAO-API-004 [level:library]
    /// @trace REQ-BAO-API-007 [level:library]
    pub fn dispatch(
        &self,
        target_id: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, BridgeError> {
        command_dispatcher::dispatch_command(&*self.backend, method, params, target_id)
    }

    /// 获取 backend 引用(供子模块/测试使用)。
    pub fn backend(&self) -> &dyn ServoBackend {
        &*self.backend
    }

    /// 把自身包装为 `Arc<dyn InMemoryBridge>`。
    ///
    /// 这是 InMemoryTransport 构造所需的类型。
    ///
    /// @trace REQ-BAO-API-004 [level:library]
    pub fn into_in_memory_bridge(self) -> Arc<dyn InMemoryBridge> {
        Arc::new(self)
    }
}

impl InMemoryBridge for CDPRdpBridge {
    fn dispatch_command(
        &self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
    ) -> InMemoryBridgeResponse {
        // session_id 用作 target_id。缺省时使用 "default"。
        let target_id = session_id.unwrap_or("default");
        match self.dispatch(target_id, method, params) {
            Ok(v) => InMemoryBridgeResponse::Ok(v),
            Err(e) => {
                // BridgeError → JSON-RPC error.data。
                // 把错误码 + 消息打包,便于 InMemoryTransport 还原为 CdpError。
                let code = e.cdp_error_code();
                let msg = e.message();
                let payload = serde_json::json!({
                    "code": code,
                    "message": msg,
                });
                InMemoryBridgeResponse::Err(payload.to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::servo_backend::MockServoBackend;
    use serde_json::json;

    #[test]
    fn bridge_dispatch_page_navigate_succeeds() {
        let backend: Arc<dyn ServoBackend> = Arc::new(MockServoBackend::new());
        let bridge = CDPRdpBridge::new(backend);
        let r = bridge
            .dispatch("1", "Page.navigate", json!({"url":"https://x"}))
            .unwrap();
        assert_eq!(r["frameId"], "FRAME_0");
    }

    #[test]
    fn bridge_dispatch_heap_profiler_returns_not_supported() {
        let backend: Arc<dyn ServoBackend> = Arc::new(MockServoBackend::new());
        let bridge = CDPRdpBridge::new(backend);
        let err = bridge
            .dispatch("1", "HeapProfiler.takeHeapSnapshot", json!({}))
            .unwrap_err();
        assert!(matches!(err, BridgeError::NotSupported(_)));
        assert_eq!(err.cdp_error_code(), -32601);
    }

    #[test]
    fn bridge_implements_in_memory_bridge_trait() {
        let backend: Arc<dyn ServoBackend> = Arc::new(MockServoBackend::new());
        let bridge = CDPRdpBridge::new(backend);
        let in_memory: Arc<dyn InMemoryBridge> = bridge.into_in_memory_bridge();

        // Test that we can dispatch through the trait object.
        let r = in_memory.dispatch_command("Page.navigate", json!({"url":"x"}), Some("1"));
        match r {
            InMemoryBridgeResponse::Ok(v) => assert_eq!(v["frameId"], "FRAME_0"),
            InMemoryBridgeResponse::Err(e) => panic!("expected Ok, got Err: {e}"),
        }
    }

    #[test]
    fn bridge_in_memory_trait_e_class_returns_error_with_code() {
        let backend: Arc<dyn ServoBackend> = Arc::new(MockServoBackend::new());
        let bridge = CDPRdpBridge::new(backend);
        let in_memory: Arc<dyn InMemoryBridge> = bridge.into_in_memory_bridge();

        let r = in_memory.dispatch_command("HeapProfiler.takeHeapSnapshot", json!({}), Some("1"));
        match r {
            InMemoryBridgeResponse::Err(msg) => {
                let v: Value = serde_json::from_str(&msg).unwrap();
                assert_eq!(v["code"], -32601);
                assert!(v["message"].as_str().unwrap().contains("HeapProfiler"));
            }
            InMemoryBridgeResponse::Ok(_) => panic!("expected Err"),
        }
    }

    #[test]
    fn bridge_default_session_id_used_when_none() {
        let backend: Arc<dyn ServoBackend> = Arc::new(MockServoBackend::new());
        let bridge = CDPRdpBridge::new(backend);
        let in_memory: Arc<dyn InMemoryBridge> = bridge.into_in_memory_bridge();

        // MockServoBackend accepts "default" as a known target.
        let r = in_memory.dispatch_command("Page.navigate", json!({"url":"x"}), None);
        match r {
            InMemoryBridgeResponse::Ok(v) => assert_eq!(v["frameId"], "FRAME_0"),
            _ => panic!("expected Ok"),
        }
    }

    #[test]
    fn bridge_backend_accessor_returns_reference() {
        let backend: Arc<dyn ServoBackend> = Arc::new(MockServoBackend::new());
        let bridge = CDPRdpBridge::new(backend);
        // Backend is accessible.
        let _b: &dyn ServoBackend = bridge.backend();
    }

    #[test]
    fn bridge_unknown_method_returns_method_not_found_error() {
        let backend: Arc<dyn ServoBackend> = Arc::new(MockServoBackend::new());
        let bridge = CDPRdpBridge::new(backend);
        let in_memory: Arc<dyn InMemoryBridge> = bridge.into_in_memory_bridge();

        let r = in_memory.dispatch_command("Unknown.foo", json!({}), Some("1"));
        match r {
            InMemoryBridgeResponse::Err(msg) => {
                let v: Value = serde_json::from_str(&msg).unwrap();
                assert_eq!(v["code"], -32601);
            }
            _ => panic!("expected Err"),
        }
    }
}
