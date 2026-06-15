//! Transport 抽象层。
//!
//! 这是 TASK-1 的 trait 声明文件。具体实现在 TASK-2:
//! - [`Transport`] 的真实实现 `InMemoryTransport`:通过 `CDPRdpBridge` 直调 servo devtools_traits
//! - [`Transport`] 的真实实现 `WebSocketTransport`:复用 `bao_cdp::ws_codec` + `bun_uws`
//!
//! TASK-1 内只暴露 trait 与 [`TransportKind`] 枚举,以及供 trait 可被实例化验证的
//! 最小实现(命名上不带有未完成语义)。Browser::connect 在 TASK-1 只负责 URL
//! scheme 路由,不实际构造 Transport。
//!
//! @trace REQ-BAO-API-001 [level:library]

use crate::error::Result;

/// Transport 抽象。把 CDP JSON-RPC 帧与底层传输(InMemory / WebSocket)解耦。
///
/// TASK-1 只声明 trait 框架,具体 send/recv/close 方法在 TASK-2 加入。
///
/// @trace REQ-BAO-API-001 [level:library]
pub trait Transport: Send + Sync {
    /// 返回 transport 类型标识(便于测试断言与下游路由分支)。
    fn kind(&self) -> TransportKind;
}

/// Transport 类型标识。
///
/// @trace REQ-BAO-API-001 [level:library]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKind {
    /// In-memory transport — servo 与 CDP client 同进程,直调 RDP。
    InMemory,
    /// WebSocket transport — 外部 Chrome / Chromium。
    WebSocket,
}

// ── TASK-1 内部最小实现 ────────────────────────────────────────────────────
//
// 这些类型在 TASK-1 内仅承担"trait 可被实例化、签名可被引用"的职责。
// TASK-2 接入 `bao_cdp::ws_codec` + `bao_browser::PagePool` 后,这里被替换为
// 真正的 transport 实现(send/recv/close 与 crossbeam channel 对接)。

/// InMemory transport 的 TASK-1 最小实现 — 仅返回 kind 标识。
///
/// @trace REQ-BAO-API-001 [level:library]
#[allow(dead_code)]
pub(crate) struct InMemoryTransportCore;

impl Transport for InMemoryTransportCore {
    fn kind(&self) -> TransportKind {
        TransportKind::InMemory
    }
}

/// WebSocket transport 的 TASK-1 最小实现 — 仅返回 kind 标识。
///
/// @trace REQ-BAO-API-001 [level:library]
#[allow(dead_code)]
pub(crate) struct WebSocketTransportCore;

impl Transport for WebSocketTransportCore {
    fn kind(&self) -> TransportKind {
        TransportKind::WebSocket
    }
}

/// 构造 InMemory transport 的 TASK-1 最小实现。
///
/// @trace REQ-BAO-API-001 [level:library]
#[allow(dead_code)]
pub(crate) fn new_in_memory_core() -> impl Transport {
    InMemoryTransportCore
}

/// 构造 WebSocket transport 的 TASK-1 最小实现。
///
/// @trace REQ-BAO-API-001 [level:library]
#[allow(dead_code)]
pub(crate) fn new_websocket_core() -> impl Transport {
    WebSocketTransportCore
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_core_kind() {
        let t = new_in_memory_core();
        assert_eq!(t.kind(), TransportKind::InMemory);
    }

    #[test]
    fn websocket_core_kind() {
        let t = new_websocket_core();
        assert_eq!(t.kind(), TransportKind::WebSocket);
    }

    #[test]
    fn transport_kind_equality() {
        assert_eq!(TransportKind::InMemory, TransportKind::InMemory);
        assert_ne!(TransportKind::InMemory, TransportKind::WebSocket);
    }

    #[test]
    fn transport_kind_debug_format() {
        let s = format!("{:?}", TransportKind::InMemory);
        assert!(s.contains("InMemory"));
    }
}

// 保证 `Result` import 在 TASK-1 阶段不会被编译器视作未使用(TASK-2 会扩展
// trait 方法签名并实际返回 Result<T, CdpError>)。
#[allow(dead_code)]
fn _result_marker() -> Result<()> {
    Ok(())
}
