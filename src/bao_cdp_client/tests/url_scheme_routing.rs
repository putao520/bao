//! 集成测试:Browser::connect URL scheme 路由。
//!
//! 对应 SPEC: REQ-BAO-API-001 [验收 1-4]
//!
//! 验收标准:
//! 1. `memory://bao` → 命中 InMemory 分支(`is_in_memory() == true`)
//! 2. `ws://127.0.0.1:9222` → 命中 WebSocket 分支(`is_websocket() == true`)
//! 3. `http://127.0.0.1:9222` → 命中 HTTP-discover 分支(同样落到 WebSocket,但 scheme 标识为 http)
//! 4. scheme 错误(如 ftp)返回 `ConnectError::InvalidScheme(scheme)`
//!
//! 另外覆盖:
//! - `wss://` / `https://` 加密 scheme 也路由到 WebSocket
//! - 空 URL / 无 `://` 返回 `ConnectError::InvalidUrl`
//!
//! @trace REQ-BAO-API-001 [level:library]

use bao_cdp_client::error::ConnectError;
use bao_cdp_client::transport::TransportKind;
use bao_cdp_client::Browser;

// ── SPEC 验收 1: memory://bao → InMemory ──────────────────────────────────

#[test]
fn memory_scheme_routes_to_in_memory() {
    // @trace REQ-BAO-API-001 [interface:Browser]
    let browser = Browser::connect("memory://bao").expect("memory://bao should succeed");
    assert!(
        browser.is_in_memory(),
        "memory://bao must route to InMemory transport"
    );
    assert_eq!(
        browser.transport_kind(),
        TransportKind::InMemory,
        "transport kind must be InMemory for memory://"
    );
    assert_eq!(browser.scheme(), "memory");
    assert_eq!(browser.url(), "memory://bao");
}

// ── SPEC 验收 2: ws://... → WebSocket 直连 ────────────────────────────────

#[test]
fn ws_scheme_routes_to_websocket() {
    // @trace REQ-BAO-API-001 [interface:Browser]
    let browser =
        Browser::connect("ws://127.0.0.1:9222").expect("ws:// URL should parse successfully");
    assert!(
        browser.is_websocket(),
        "ws:// must route to WebSocket transport"
    );
    assert!(!browser.is_in_memory());
    assert_eq!(browser.scheme(), "ws");
    assert_eq!(browser.url(), "ws://127.0.0.1:9222");
}

// ── SPEC 验收 3: http://... → 自动发现 ws endpoint → WebSocket ────────────

#[test]
fn http_scheme_routes_to_websocket_with_discovery() {
    // @trace REQ-BAO-API-001 [interface:Browser]
    let browser = Browser::connect("http://127.0.0.1:9222")
        .expect("http:// URL should parse successfully (TASK-1 routing only, no network)");
    // http:// 在 TASK-1 中合并到 WebSocket 分支(实际 discover 在 TASK-2)
    assert!(
        browser.is_websocket(),
        "http:// must route to WebSocket transport (with discovery pending in TASK-2)"
    );
    assert_eq!(browser.scheme(), "http");
}

// ── SPEC 验收 4: 错误 scheme → InvalidScheme ─────────────────────────────

#[test]
fn invalid_scheme_returns_invalid_scheme_error() {
    // @trace REQ-BAO-API-001 [interface:Browser]
    let result = Browser::connect("ftp://example.com:21");
    let err = result.expect_err("ftp:// should fail with InvalidScheme");
    match err {
        ConnectError::InvalidScheme(scheme) => {
            assert_eq!(scheme, "ftp", "InvalidScheme must carry the offending scheme");
        }
        other => panic!("expected InvalidScheme, got {:?}", other),
    }
}

// ── 扩展:wss / https 加密 scheme ─────────────────────────────────────────

#[test]
fn wss_scheme_routes_to_websocket() {
    // @trace REQ-BAO-API-001 [interface:Browser]
    let browser = Browser::connect("wss://example.com:443/devtools/page/abc").unwrap();
    assert!(browser.is_websocket());
    assert_eq!(browser.scheme(), "wss");
}

#[test]
fn https_scheme_routes_to_websocket() {
    // @trace REQ-BAO-API-001 [interface:Browser]
    let browser = Browser::connect("https://127.0.0.1:9443").unwrap();
    assert!(browser.is_websocket());
    assert_eq!(browser.scheme(), "https");
}

// ── 扩展:空 URL / 无 scheme → InvalidUrl ────────────────────────────────

#[test]
fn empty_url_returns_invalid_url() {
    // @trace REQ-BAO-API-001 [interface:Browser]
    let err = Browser::connect("").expect_err("empty string must fail");
    assert!(
        matches!(err, ConnectError::InvalidUrl),
        "empty URL must return InvalidUrl, got {:?}",
        err
    );
}

#[test]
fn url_without_scheme_separator_returns_invalid_url() {
    // @trace REQ-BAO-API-001 [interface:Browser]
    // 没有 ://,bun_url 的 URL::parse 返回空 protocol → InvalidUrl
    let err = Browser::connect("localhost:9222").expect_err("missing-scheme URL must fail");
    assert!(
        matches!(err, ConnectError::InvalidUrl),
        "missing-scheme URL must return InvalidUrl, got {:?}",
        err
    );
}

#[test]
fn url_with_only_scheme_separator_returns_invalid_url() {
    // @trace REQ-BAO-API-001 [interface:Browser]
    // "://" 前没有 scheme,protocol 空 → InvalidUrl
    let err = Browser::connect("://no-scheme").expect_err("schemeless URL must fail");
    assert!(matches!(err, ConnectError::InvalidUrl));
}

// ── 扩展:其他常见 scheme 拒绝 ────────────────────────────────────────────

#[test]
fn file_scheme_rejected() {
    // @trace REQ-BAO-API-001 [interface:Browser]
    let err = Browser::connect("file:///etc/passwd").unwrap_err();
    assert!(matches!(err, ConnectError::InvalidScheme(s) if s == "file"));
}

#[test]
fn unix_scheme_rejected() {
    // @trace REQ-BAO-API-001 [interface:Browser]
    let err = Browser::connect("unix:///var/run/cdp.sock").unwrap_err();
    assert!(matches!(err, ConnectError::InvalidScheme(s) if s == "unix"));
}

#[test]
fn javascript_scheme_rejected() {
    // @trace REQ-BAO-API-001 [interface:Browser]
    // 注意:javascript: 没有 //,bun_url 视为无 scheme — InvalidUrl
    let err = Browser::connect("javascript:alert(1)").unwrap_err();
    assert!(matches!(err, ConnectError::InvalidUrl));
}

// ── 扩展:Browser 元数据 ─────────────────────────────────────────────────

#[test]
fn browser_url_and_scheme_roundtrip() {
    // @trace REQ-BAO-API-001 [interface:Browser]
    let url = "memory://bao";
    let browser = Browser::connect(url).unwrap();
    assert_eq!(browser.url(), url);
    assert_eq!(browser.scheme(), "memory");
}

#[test]
fn browser_display_includes_url() {
    // @trace REQ-BAO-API-001 [interface:Browser]
    let browser = Browser::connect("ws://127.0.0.1:9222").unwrap();
    let display = format!("{}", browser);
    assert!(display.contains("ws://127.0.0.1:9222"), "display: {}", display);
    assert!(display.contains("WebSocket"), "display: {}", display);
}

#[test]
fn browser_clone_preserves_parsed_state() {
    // @trace REQ-BAO-API-001 [interface:Browser]
    let b1 = Browser::connect("wss://example.com:443/devtools").unwrap();
    let b2 = b1.clone();
    assert_eq!(b1.url(), b2.url());
    assert_eq!(b1.scheme(), b2.scheme());
    assert_eq!(b1.transport_kind(), b2.transport_kind());
}

#[test]
fn all_five_supported_schemes_succeed() {
    // @trace REQ-BAO-API-001 [interface:Browser]
    for url in [
        "memory://bao",
        "ws://127.0.0.1:9222",
        "wss://example.com:443",
        "http://127.0.0.1:9222",
        "https://example.com:443",
    ] {
        let browser = Browser::connect(url).unwrap_or_else(|e| panic!("{} should succeed: {:?}", url, e));
        // memory → InMemory; 其他 → WebSocket
        let expected = if url.starts_with("memory://") {
            TransportKind::InMemory
        } else {
            TransportKind::WebSocket
        };
        assert_eq!(
            browser.transport_kind(),
            expected,
            "wrong transport kind for {}",
            url
        );
    }
}
