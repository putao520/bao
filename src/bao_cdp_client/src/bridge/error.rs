//! Bridge 层错误类型。
//!
//! [`BridgeError`] 是 `CDPRdpBridge` 派发命令时可能返回的错误。它覆盖:
//! - 协议级错误(无效 method / 未知 domain / -32601 不支持)
//! - servo 调用错误(Page 不存在 / Servo 内部异常)
//! - 参数错误(JSON schema 不匹配)
//! - 任务占位(TASK-3b 未实现的 B 类 method)
//!
//! 通过 [`BridgeError::cdp_error_code`] 映射到 JSON-RPC 标准错误码:
//! - `-32601`:Method not found / Not supported(E 类 31 method)
//! - `-32602`:Invalid params
//! - `-32000`:Server error(其他)
//!
//! @trace REQ-BAO-API-004 [level:library]
//! @trace REQ-BAO-API-007 [level:library]

use std::fmt;

/// JSON-RPC 标准错误码 — Method not found / 不支持。
///
/// @trace REQ-BAO-API-007 [level:library]
pub const CDP_ERR_METHOD_NOT_FOUND: i32 = -32601;

/// JSON-RPC 标准错误码 — Invalid params。
pub const CDP_ERR_INVALID_PARAMS: i32 = -32602;

/// JSON-RPC 服务器错误范围起点( reserved for implementation-defined server-errors)。
pub const CDP_ERR_SERVER_ERROR: i32 = -32000;

/// `CDPRdpBridge` 命令派发错误。
///
/// @trace REQ-BAO-API-004 [level:library]
/// @trace REQ-BAO-API-007 [level:library]
#[derive(Debug)]
pub enum BridgeError {
    /// method 字符串缺 `.` 分隔符,无法拆分 domain/command。
    InvalidMethod(String),
    /// method 在已知 domain 下找不到对应处理器(完全未实现)。
    MethodNotFound(String),
    /// E 类 — servo 不支持该 method,返回 -32601。
    ///
    /// 携带的字符串是 method 全名(如 `HeapProfiler.takeHeapSnapshot`)。
    ///
    /// @trace REQ-BAO-API-007 [domain:HeapProfiler]
    NotSupported(String),
    /// B 类占位 — TASK-3b 会实现。返回 server error 提示尚未实现。
    NotImplementedYet(String),
    /// target_id 不是合法的 usize 字符串。
    InvalidTargetId(String),
    /// target_id 合法但 PagePool 中找不到对应 Page。
    PageNotFound(String),
    /// servo 内部错误(navigate / evaluate / screenshot 等失败)。
    ServoError(String),
    /// 参数缺失或类型错误。
    InvalidParams(String),
}

impl BridgeError {
    /// 映射到 JSON-RPC 标准错误码。
    ///
    /// - `MethodNotFound` / `NotSupported` → `-32601`
    /// - `InvalidParams` → `-32602`
    /// - 其他 → `-32000`
    ///
    /// @trace REQ-BAO-API-007 [level:library]
    pub fn cdp_error_code(&self) -> i32 {
        match self {
            BridgeError::MethodNotFound(_) => CDP_ERR_METHOD_NOT_FOUND,
            BridgeError::NotSupported(_) => CDP_ERR_METHOD_NOT_FOUND,
            BridgeError::InvalidParams(_) => CDP_ERR_INVALID_PARAMS,
            // InvalidMethod 视为 invalid params(请求格式错误)
            BridgeError::InvalidMethod(_) => CDP_ERR_INVALID_PARAMS,
            BridgeError::NotImplementedYet(_)
            | BridgeError::InvalidTargetId(_)
            | BridgeError::PageNotFound(_)
            | BridgeError::ServoError(_) => CDP_ERR_SERVER_ERROR,
        }
    }

    /// 错误消息(用于 JSON-RPC error.data 字段)。
    pub fn message(&self) -> String {
        match self {
            BridgeError::InvalidMethod(m) => format!("invalid method format: {m}"),
            BridgeError::MethodNotFound(m) => format!("method not found: {m}"),
            BridgeError::NotSupported(m) => format!("method not supported by servo: {m}"),
            BridgeError::NotImplementedYet(m) => format!("not implemented yet: {m}"),
            BridgeError::InvalidTargetId(t) => format!("invalid target id: {t}"),
            BridgeError::PageNotFound(t) => format!("page not found: {t}"),
            BridgeError::ServoError(msg) => format!("servo error: {msg}"),
            BridgeError::InvalidParams(msg) => format!("invalid params: {msg}"),
        }
    }
}

impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message())
    }
}

impl std::error::Error for BridgeError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_not_found_returns_32601() {
        let e = BridgeError::MethodNotFound("Foo.bar".into());
        assert_eq!(e.cdp_error_code(), CDP_ERR_METHOD_NOT_FOUND);
    }

    #[test]
    fn not_supported_returns_32601() {
        // @trace REQ-BAO-API-007 [domain:HeapProfiler]
        let e = BridgeError::NotSupported("HeapProfiler.takeHeapSnapshot".into());
        assert_eq!(e.cdp_error_code(), CDP_ERR_METHOD_NOT_FOUND);
    }

    #[test]
    fn invalid_params_returns_32602() {
        let e = BridgeError::InvalidParams("missing url".into());
        assert_eq!(e.cdp_error_code(), CDP_ERR_INVALID_PARAMS);
    }

    #[test]
    fn invalid_method_returns_32602() {
        let e = BridgeError::InvalidMethod("noDot".into());
        assert_eq!(e.cdp_error_code(), CDP_ERR_INVALID_PARAMS);
    }

    #[test]
    fn not_implemented_yet_returns_server_error() {
        let e = BridgeError::NotImplementedYet("Runtime.evaluate".into());
        assert_eq!(e.cdp_error_code(), CDP_ERR_SERVER_ERROR);
    }

    #[test]
    fn servo_error_returns_server_error() {
        let e = BridgeError::ServoError("navigate failed".into());
        assert_eq!(e.cdp_error_code(), CDP_ERR_SERVER_ERROR);
    }

    #[test]
    fn page_not_found_returns_server_error() {
        let e = BridgeError::PageNotFound("999".into());
        assert_eq!(e.cdp_error_code(), CDP_ERR_SERVER_ERROR);
        assert!(e.message().contains("page not found"));
        assert!(e.message().contains("999"));
    }

    #[test]
    fn invalid_target_id_returns_server_error() {
        let e = BridgeError::InvalidTargetId("not-a-number".into());
        assert_eq!(e.cdp_error_code(), CDP_ERR_SERVER_ERROR);
    }

    #[test]
    fn display_uses_message() {
        let e = BridgeError::ServoError("boom".into());
        let s = format!("{e}");
        assert!(s.contains("servo error"));
        assert!(s.contains("boom"));
    }

    #[test]
    fn is_std_error() {
        let e = BridgeError::NotSupported("X.y".into());
        let _: &dyn std::error::Error = &e;
    }
}
