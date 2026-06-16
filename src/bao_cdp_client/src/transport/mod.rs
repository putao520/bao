//! Transport 抽象层。
//!
//! 这是 TASK-2 的实现。`Transport` trait 把 CDP JSON-RPC 帧与底层传输
//! (InMemory / WebSocket)解耦,具体两种实现:
//! - [`in_memory::InMemoryTransport`]:通过 `InMemoryBridge` trait 抽象与 servo
//!   ScriptThread 通信(servo WebView `!Send`,不可跨线程直调,DEC-CDP-002)。
//!   TASK-3 替换为真实的 `CDPRdpBridge`(桥接 servo devtools_traits RDP)。
//! - [`ws::WebSocketTransport`]:通过 `bun_uws::ws_client::WebSocketClient`
//!   完成 RFC 6455 握手与帧编解码(client-side masking),通过
//!   `std::net::TcpStream` 与外部 Chrome 通信。
//!   (REQ-CDP-UWS-001: bao_cdp::{ws_codec,ws_handshake} 已迁移至 bun_uws。)
//!
//! # 设计要点
//!
//! - **零 tokio**:同步阻塞 I/O(`std::net::TcpStream` 阻塞读、`std::sync::mpsc`
//!   channel 阻塞 recv)。上层 API 包装为 `bun_event_loop` 任务即可。
//! - **错误类型**:统一用 [`crate::error::CdpError`],新增 `TransportError` /
//!   `HandshakeError` 变体覆盖 REQ-BAO-API-002 错误码集合。
//! - **响应/事件分流**:`send_command` 返回 `Result<Value>`(命令响应),
//!   `recv_event` 返回 `Result<Option<CdpEvent>>`(server 推送事件,无事件时返回 `Ok(None)`)。
//!
//! @trace REQ-BAO-API-002 [interface:Transport]

pub mod in_memory;
pub mod r#trait;
pub mod ws;

pub use in_memory::{InMemoryBridge, InMemoryBridgeResponse, InMemoryTransport};
pub use r#trait::{CdpEvent, Transport};
pub use ws::WebSocketTransport;

/// Transport 类型标识。
///
/// 用于测试断言、日志区分与 Browser 路由分支标记。
///
/// @trace REQ-BAO-API-002 [interface:Transport]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKind {
    /// In-memory transport — servo 与 CDP client 同进程,直调 RDP。
    InMemory,
    /// WebSocket transport — 外部 Chrome / Chromium。
    WebSocket,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_kind_equality() {
        assert_eq!(TransportKind::InMemory, TransportKind::InMemory);
        assert_ne!(TransportKind::InMemory, TransportKind::WebSocket);
    }

    #[test]
    fn transport_kind_debug_format() {
        let s = format!("{:?}", TransportKind::InMemory);
        assert!(s.contains("InMemory"));
        let s = format!("{:?}", TransportKind::WebSocket);
        assert!(s.contains("WebSocket"));
    }
}
