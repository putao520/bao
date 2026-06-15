//! Error types for CDP client operations.
//!
//! 错误类型层次:
//! - [`ConnectError`]: 连接阶段(初始化/路由/握手)的错误
//! - [`CdpError`]: 连接之后协议/通信阶段的错误(TASK-2+ 使用)
//!
//! @trace REQ-BAO-API-001 [level:library]

use std::fmt;

/// 连接阶段错误(`Browser::connect` 入口返回)。
///
/// 五种变体覆盖 Plan MD 要求的全部错误码:
/// - [`ConnectError::InvalidUrl`]: URL 解析失败(空串、缺 `://`、非 UTF-8 等)
/// - [`ConnectError::InvalidScheme`]: scheme 已识别但不在 `{memory, ws, wss, http, https}` 集合
/// - [`ConnectError::LaunchError`]: 外部浏览器进程拉起失败
/// - [`ConnectError::ConnectionFailed`]: TCP/WebSocket 握手失败
/// - [`ConnectError::Timeout`]: 连接超时
///
/// @trace REQ-BAO-API-001 [level:library]
#[derive(Debug)]
pub enum ConnectError {
    /// URL 解析失败(空串、缺 `://`、非 UTF-8 等)。
    InvalidUrl,
    /// URL scheme 不支持。携带出错的 scheme 字符串便于诊断。
    InvalidScheme(String),
    /// 外部浏览器进程拉起失败(如 Chrome 二进制不存在)。
    LaunchError(String),
    /// 连接失败(TCP 拒绝、WebSocket 握手失败等)。
    ConnectionFailed(String),
    /// 连接超时。
    Timeout(String),
}

impl fmt::Display for ConnectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConnectError::InvalidUrl => write!(f, "invalid URL (empty or missing scheme)"),
            ConnectError::InvalidScheme(scheme) => {
                write!(f, "invalid URL scheme: {:?} (expected memory/ws/wss/http/https)", scheme)
            }
            ConnectError::LaunchError(msg) => write!(f, "browser launch failed: {}", msg),
            ConnectError::ConnectionFailed(msg) => write!(f, "connection failed: {}", msg),
            ConnectError::Timeout(msg) => write!(f, "connect timeout: {}", msg),
        }
    }
}

impl std::error::Error for ConnectError {}

impl From<std::io::Error> for ConnectError {
    fn from(err: std::io::Error) -> Self {
        ConnectError::ConnectionFailed(err.to_string())
    }
}

/// 通信阶段错误(`Transport` 使用,具体实现在 TASK-2)。
///
/// 变体覆盖 Plan MD 与 REQ-BAO-API-002 要求的完整错误码集合:
/// - [`CdpError::ProtocolError`]: 协议层(JSON-RPC error / -32601 / 解析失败)
/// - [`CdpError::JsonError`]: 序列化/反序列化失败
/// - [`CdpError::IoError`]: 底层 I/O 错误
/// - [`CdpError::ConnectionClosed`]: Transport 已关闭或对端断开
/// - [`CdpError::Timeout`]: 命令调用或 recv_event 超时(TimeoutError)
/// - [`CdpError::TransportError`]: Transport 实现内部错误(TransportError)
/// - [`CdpError::HandshakeError`]: WebSocket 握手失败
///
/// @trace REQ-BAO-API-002 [interface:Transport]
#[derive(Debug)]
pub enum CdpError {
    /// 协议层错误(JSON-RPC error object / unknown method 等)。
    ProtocolError(String),
    /// 序列化/反序列化失败。
    JsonError(String),
    /// I/O 错误。
    IoError(std::io::Error),
    /// 连接已关闭。
    ConnectionClosed,
    /// 调用超时(TimeoutError)。
    Timeout(String),
    /// Transport 实现内部错误(TransportError)。携带上下文用于诊断。
    TransportError(String),
    /// WebSocket 握手失败。
    HandshakeError(String),
}

impl fmt::Display for CdpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CdpError::ProtocolError(msg) => write!(f, "CDP protocol error: {}", msg),
            CdpError::JsonError(msg) => write!(f, "JSON error: {}", msg),
            CdpError::IoError(err) => write!(f, "I/O error: {}", err),
            CdpError::ConnectionClosed => write!(f, "connection closed"),
            CdpError::Timeout(msg) => write!(f, "timeout: {}", msg),
            CdpError::TransportError(msg) => write!(f, "transport error: {}", msg),
            CdpError::HandshakeError(msg) => write!(f, "WebSocket handshake error: {}", msg),
        }
    }
}

impl std::error::Error for CdpError {}

impl From<std::io::Error> for CdpError {
    fn from(err: std::io::Error) -> Self {
        CdpError::IoError(err)
    }
}

impl From<serde_json::Error> for CdpError {
    fn from(err: serde_json::Error) -> Self {
        CdpError::JsonError(err.to_string())
    }
}

impl From<ConnectError> for CdpError {
    fn from(err: ConnectError) -> Self {
        CdpError::ProtocolError(err.to_string())
    }
}

/// 通信阶段 Result 别名。
///
/// @trace REQ-BAO-API-001 [level:library]
pub type Result<T> = std::result::Result<T, CdpError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_error_invalid_url_display() {
        let err = ConnectError::InvalidUrl;
        let s = err.to_string();
        assert!(s.contains("invalid URL"), "got: {}", s);
    }

    #[test]
    fn connect_error_invalid_scheme_carries_scheme() {
        let err = ConnectError::InvalidScheme("ftp".to_string());
        let s = err.to_string();
        assert!(s.contains("ftp"), "got: {}", s);
        assert!(s.contains("invalid URL scheme"), "got: {}", s);
    }

    #[test]
    fn connect_error_launch_message() {
        let err = ConnectError::LaunchError("chrome not found".to_string());
        assert!(err.to_string().contains("chrome not found"));
    }

    #[test]
    fn connect_error_connection_failed_message() {
        let err = ConnectError::ConnectionFailed("refused".to_string());
        assert!(err.to_string().contains("refused"));
    }

    #[test]
    fn connect_error_timeout_message() {
        let err = ConnectError::Timeout("30s elapsed".to_string());
        let s = err.to_string();
        assert!(s.contains("timeout"), "got: {}", s);
        assert!(s.contains("30s elapsed"), "got: {}", s);
    }

    #[test]
    fn connect_error_is_std_error() {
        let err = ConnectError::InvalidUrl;
        let _: &dyn std::error::Error = &err;
    }

    #[test]
    fn cdp_error_from_connect_error() {
        let err = ConnectError::InvalidScheme("foo".into());
        let cdp: CdpError = err.into();
        assert!(cdp.to_string().contains("invalid URL scheme"));
    }

    #[test]
    fn cdp_error_from_serde_json() {
        let json_err = serde_json::from_str::<serde_json::Value>("not json").unwrap_err();
        let cdp: CdpError = json_err.into();
        assert!(matches!(cdp, CdpError::JsonError(_)));
    }

    #[test]
    fn cdp_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::Other, "boom");
        let cdp: CdpError = io_err.into();
        assert!(matches!(cdp, CdpError::IoError(_)));
    }

    #[test]
    fn cdp_error_connection_closed() {
        let err = CdpError::ConnectionClosed;
        assert_eq!(err.to_string(), "connection closed");
    }

    #[test]
    fn cdp_error_transport_error_message() {
        let err = CdpError::TransportError("channel closed".into());
        let s = err.to_string();
        assert!(s.contains("transport error"), "got: {}", s);
        assert!(s.contains("channel closed"), "got: {}", s);
    }

    #[test]
    fn cdp_error_handshake_error_message() {
        let err = CdpError::HandshakeError("missing key".into());
        let s = err.to_string();
        assert!(s.contains("WebSocket handshake error"), "got: {}", s);
        assert!(s.contains("missing key"), "got: {}", s);
    }
}
