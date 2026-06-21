// @trace REQ-H3-001 [req:REQ-H3-001] [entity:H3Client] [entity:AltSvc]
// @trace REQ-PURE-007 [level:library] (status: deprecated/overruled — lsquic C stack retained, quinn migration NOT done)
//! HTTP/3 (h3) fetch 能力补全模块。
//!
//! ## 设计依据
//!
//! BAO 作为正常 BUN 必须暴露 h3/HTTP3 fetch 能力（用户决策：「BAO 是正常 BUN，
//! BUN 用了 BAO 就应该有 h3 能力」）。底层 `bun_http::h3_client`（基于 lsquic）
//! 已完整实现 HTTP/3 客户端，包括：
//!
//! - QUIC 连接管理（`ClientContext` / `ClientSession`）
//! - h3 流多路复用（`Stream`）
//! - Alt-Svc (RFC 7838) 缓存与协商（`h3_client::alt_svc`）
//! - 显式 `protocol: "http3"` 强制 h3（`flags.force_http3`）
//!
//! 但 bun_http 的 h3 能力默认是**关闭**的，需要显式开启
//! `bun_http::EXPERIMENTAL_HTTP3_CLIENT_FROM_CLI`（对应 Bun 的
//! `--experimental-http3-fetch` / `BUN_FEATURE_FLAG_EXPERIMENTAL_HTTP3_CLIENT=1`）。
//!
//! 本模块的职责就是在 BAO 运行时初始化时把该开关设为 `true`，让 fetch() 默认
//! 支持 HTTP/3 协议升级（Alt-Svc 协商 + force_http3 显式协议选项）。
//!
//! ## 协议选择策略
//!
//! 1. URL 是 `http://` → 强制 HTTP/1.1（h3 仅 HTTPS）
//! 2. URL 是 `https://` + `fetch(url, { protocol: "http3" })` → 强制 h3
//!    （`flags.force_http3`，路由到 `ClientContext::connect`）
//! 3. URL 是 `https://` + 无显式协议 → 查询 Alt-Svc 缓存：
//!    - 服务器曾响应 `Alt-Svc: h3=":port"` → 升级到 h3
//!    - 否则 → HTTP/1.1（fallback，不破坏现有行为）
//!
//! ## 连接复用与多路复用
//!
//! 由 `bun_http::h3_client` 原生提供：
//! - 连接复用：`ClientContext::sessions` 按 `hostname:port` 复用，支持 0-RTT/1-RTT
//! - 流多路复用：同一 QUIC 连接可承载多个 h3 stream（`ClientSession::pending`）
//!
//! ## 不替换 lsquic
//!
//! REQ-PURE-007（lsquic → quinn）已推翻，lsquic C 栈保留。本 REQ 仅是「暴露已有能力」，
//! 不涉及 C 库替换。

use core::sync::atomic::Ordering;

/// h3 默认启用的运行时开关（Once 保护，幂等）。
///
/// 使用 `std::sync::Once` 保证多次调用 `enable_h3_by_default()` 只设置一次，
/// 避免在测试或重新初始化场景下重复打印日志或竞争。
static H3_DEFAULT_ENABLED: std::sync::Once = std::sync::Once::new();

/// 在 BAO 运行时初始化时默认启用 h3 能力。
///
/// 设置 `bun_http::EXPERIMENTAL_HTTP3_CLIENT_FROM_CLI = true`，使得：
/// - `bun_http::h3_alt_svc_enabled()` 返回 `true`
/// - `HttpClient::can_try_h3_alt_svc()` 允许查询 Alt-Svc 缓存
/// - `flags.force_http3` 分支（`fetch(url, { protocol: "http3" })`）可路由到 h3_client
///
/// 幂等：多次调用安全（Once 保护）。
///
/// 调用点：`BaoRuntime::new()`（runtime.rs）在 SpiderMonkey 初始化前调用，
/// 确保 HTTP 线程启动时 h3 开关已就绪。
///
// @trace REQ-H3-001 [req:REQ-H3-001]
pub fn enable_h3_by_default() {
    H3_DEFAULT_ENABLED.call_once(|| {
        bun_http::EXPERIMENTAL_HTTP3_CLIENT_FROM_CLI.store(true, Ordering::Relaxed);
    });
}

/// 查询 h3 是否已被默认启用。
///
/// 反映 `enable_h3_by_default()` 是否已执行 + bun_http 的综合开关状态
/// （`h3_alt_svc_enabled()` 同时检查 CLI 开关和环境变量）。
///
// @trace REQ-H3-001 [req:REQ-H3-001]
pub fn is_h3_enabled() -> bool {
    bun_http::h3_alt_svc_enabled()
}

/// 查询 h3 默认启用的运行时开关是否已被显式设置（不依赖环境变量）。
///
/// 用于单元测试验证 `enable_h3_by_default()` 的副作用。
///
// @trace REQ-H3-001 [req:REQ-H3-001]
pub fn is_h3_default_enabled_set() -> bool {
    bun_http::EXPERIMENTAL_HTTP3_CLIENT_FROM_CLI.load(Ordering::Relaxed)
}

/// 解析 `Alt-Svc` 头部字段值，提取 h3 替代端点（RFC 7838）。
///
/// 薄封装 `bun_http::h3::alt_svc::parse`，供测试和文档化使用。
///
/// 返回 `Some(port)` 表示服务器建议使用 h3 到指定端口；`None` 表示无 h3 替代；
/// `Err` 表示 `clear`（清空缓存）。
///
// @trace REQ-H3-001 [req:REQ-H3-001] [entity:AltSvc]
pub fn parse_alt_svc(field_value: &[u8]) -> Result<Option<u16>, AltSvcClear> {
    use bun_http::h3::alt_svc::{parse, ParseError};
    match parse(field_value) {
        Ok(Some(entry)) => Ok(Some(entry.port)),
        Ok(None) => Ok(None),
        Err(ParseError::Clear) => Err(AltSvcClear),
    }
}

/// Alt-Svc 头部值为 `clear`，表示清空该 origin 的 h3 缓存。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AltSvcClear;

impl core::fmt::Display for AltSvcClear {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("alt-svc clear")
    }
}

impl std::error::Error for AltSvcClear {}

#[cfg(test)]
mod tests {
    use super::*;

    // ── REQ-H3-001: h3 默认启用 ────────────────────────────────────
    // @trace REQ-H3-001 [req:REQ-H3-001] [level:unit]

    /// REQ-H3-001-C1: `enable_h3_by_default` 设置 CLI 开关为 true。
    #[test]
    fn h3_default_enabled_sets_cli_flag() {
        enable_h3_by_default();
        assert!(
            is_h3_default_enabled_set(),
            "REQ-H3-001: enable_h3_by_default must set EXPERIMENTAL_HTTP3_CLIENT_FROM_CLI = true"
        );
    }

    /// REQ-H3-001-C1: `is_h3_enabled` 反映综合开关状态（CLI 或环境变量）。
    #[test]
    fn h3_enabled_reflects_cli_flag() {
        enable_h3_by_default();
        assert!(
            is_h3_enabled(),
            "REQ-H3-001: h3 must be enabled after enable_h3_by_default"
        );
    }

    /// REQ-H3-001: `enable_h3_by_default` 幂等（Once 保护，多次调用安全）。
    #[test]
    fn enable_h3_by_default_is_idempotent() {
        enable_h3_by_default();
        enable_h3_by_default();
        enable_h3_by_default();
        assert!(is_h3_default_enabled_set());
    }

    // ── REQ-H3-001: Alt-Svc 解析（RFC 7838）────────────────────────
    // @trace REQ-H3-001 [req:REQ-H3-001] [entity:AltSvc] [level:unit]

    /// REQ-H3-001-C2: 标准的 `h3=":443"` 解析为端口 443。
    #[test]
    fn alt_svc_parse_standard_h3_port() {
        let result = parse_alt_svc(b"h3=\":443\"");
        assert_eq!(result.unwrap(), Some(443), "REQ-H3-001: standard Alt-Svc h3=\":443\"");
    }

    /// REQ-H3-001-C2: 自定义端口的 `h3=":8443"` 解析为端口 8443。
    #[test]
    fn alt_svc_parse_custom_port() {
        let result = parse_alt_svc(b"h3=\":8443\"");
        assert_eq!(result.unwrap(), Some(8443));
    }

    /// REQ-H3-001-C2: `ma=` 参数被忽略（仅关心端口）。
    #[test]
    fn alt_svc_parse_with_ma_param() {
        let result = parse_alt_svc(b"h3=\":443\"; ma=86400");
        assert_eq!(result.unwrap(), Some(443));
    }

    /// REQ-H3-001: 多个替代项时返回第一个 h3 替代。
    #[test]
    fn alt_svc_parse_multiple_alternatives() {
        let result = parse_alt_svc(b"h3=\":443\", h3-29=\":443\"");
        assert_eq!(result.unwrap(), Some(443));
    }

    /// REQ-H3-001: 非 h3 协议 ID（如 h3-29 草案版本）被忽略。
    #[test]
    fn alt_svc_parse_ignores_draft_versions() {
        let result = parse_alt_svc(b"h3-29=\":443\"");
        assert_eq!(result.unwrap(), None, "draft h3-NN must be ignored, only final h3");
    }

    /// REQ-H3-001: 空 Alt-Svc 返回 None。
    #[test]
    fn alt_svc_parse_empty() {
        let result = parse_alt_svc(b"");
        assert_eq!(result.unwrap(), None);
    }

    /// REQ-H3-001: `clear` 返回 Err(AltSvcClear)。
    #[test]
    fn alt_svc_parse_clear() {
        let result = parse_alt_svc(b"clear");
        assert_eq!(result, Err(AltSvcClear));
    }

    /// REQ-H3-001: 端口 0 被拒绝（无效端口）。
    #[test]
    fn alt_svc_parse_rejects_zero_port() {
        let result = parse_alt_svc(b"h3=\":0\"");
        assert_eq!(result.unwrap(), None);
    }

    /// REQ-H3-001: 跨主机替代（如 `h3="other.host:443"`）被拒绝（仅同主机）。
    #[test]
    fn alt_svc_parse_rejects_cross_host() {
        let result = parse_alt_svc(b"h3=\"other.host:443\"");
        assert_eq!(result.unwrap(), None, "cross-host alternatives must be rejected");
    }

    /// REQ-H3-001: 带 OWS（可选空白）的 Alt-Svc 正确解析。
    #[test]
    fn alt_svc_parse_with_whitespace() {
        let result = parse_alt_svc(b"  h3=\":443\"  ");
        assert_eq!(result.unwrap(), Some(443));
    }

    /// REQ-H3-001: AltSvcClear 实现 Display + Error。
    #[test]
    fn alt_svc_clear_implements_error() {
        let err = AltSvcClear;
        assert_eq!(err.to_string(), "alt-svc clear");
        // 可作为 std::error::Error 使用
        let _: &dyn std::error::Error = &err;
    }
}
