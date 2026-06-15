//! Connection 层占位。
//!
//! 在 chromiumoxide 中 `Connection` 持有 Transport 并提供 CDP 命令收发循环。
//! TASK-1 只声明模块,具体 Connection 实现在 TASK-2 引入。
//!
//! @trace REQ-BAO-API-001 [level:library]

use crate::error::Result;
use crate::transport::TransportKind;

/// Connection 配置(超时、重试、session_id 等)。
///
/// @trace REQ-BAO-API-001 [level:library]
#[derive(Debug, Clone)]
pub struct ConnectionConfig {
    /// 命令调用默认超时(毫秒)。
    pub default_timeout_ms: u64,
    /// Transport 类型。
    pub transport_kind: TransportKind,
}

impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            default_timeout_ms: 30_000,
            transport_kind: TransportKind::InMemory,
        }
    }
}

/// 连接 URL 解析结果,在 Browser::connect 内部使用。
///
/// @trace REQ-BAO-API-001 [level:library]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedConnectUrl {
    /// 原 URL。
    pub raw: String,
    /// 解析出的 scheme(`memory` / `ws` / `wss` / `http` / `https`)。
    pub scheme: String,
    /// 路由后的 transport 类型。
    pub transport_kind: TransportKind,
}

impl ParsedConnectUrl {
    /// 构造新的解析结果。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn new(raw: impl Into<String>, scheme: impl Into<String>, kind: TransportKind) -> Self {
        Self {
            raw: raw.into(),
            scheme: scheme.into(),
            transport_kind: kind,
        }
    }
}

/// Connection 占位 — TASK-2 会扩展为持有 Transport 的真实实现。
///
/// @trace REQ-BAO-API-001 [level:library]
#[derive(Debug)]
pub struct Connection {
    config: ConnectionConfig,
}

impl Connection {
    /// 构造新 Connection(TASK-2 会接 Transport)。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn new(config: ConnectionConfig) -> Self {
        Self { config }
    }

    /// 取配置引用。
    ///
    /// @trace REQ-BAO-API-001 [level:library]
    pub fn config(&self) -> &ConnectionConfig {
        &self.config
    }
}

// 占位函数,确保 Result 在 trait 占位阶段不会触发未使用 import 警告。
#[allow(dead_code)]
fn _result_marker() -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_config_default() {
        let cfg = ConnectionConfig::default();
        assert_eq!(cfg.default_timeout_ms, 30_000);
        assert_eq!(cfg.transport_kind, TransportKind::InMemory);
    }

    #[test]
    fn parsed_connect_url_construction() {
        let parsed = ParsedConnectUrl::new("memory://bao", "memory", TransportKind::InMemory);
        assert_eq!(parsed.raw, "memory://bao");
        assert_eq!(parsed.scheme, "memory");
        assert_eq!(parsed.transport_kind, TransportKind::InMemory);
    }

    #[test]
    fn connection_new_carries_config() {
        let cfg = ConnectionConfig {
            default_timeout_ms: 5000,
            transport_kind: TransportKind::WebSocket,
        };
        let conn = Connection::new(cfg);
        assert_eq!(conn.config().default_timeout_ms, 5000);
        assert_eq!(conn.config().transport_kind, TransportKind::WebSocket);
    }
}
