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
//! @trace REQ-BAO-API-001 [level:integration]

use bao_cdp_client::error::ConnectError;
use bao_cdp_client::transport::TransportKind;
use bao_cdp_client::Browser;

// ── SPEC 验收 1: memory://bao → InMemory ──────────────────────────────────

#[test]
fn memory_scheme_routes_to_in_memory() {
    // Arrange
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "memory://bao";

    // Act
    let browser = Browser::connect(url).expect("memory://bao should succeed");

    // Assert
    assert!(browser.is_in_memory(), "memory://bao must route to InMemory transport");
    assert_eq!(browser.transport_kind(), TransportKind::InMemory);
    assert_eq!(browser.scheme(), "memory");
    assert_eq!(browser.url(), "memory://bao");
}

// ── SPEC 验收 2: ws://... → WebSocket 直连 ────────────────────────────────

#[test]
fn ws_scheme_routes_to_websocket() {
    // Arrange
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "ws://127.0.0.1:9222";

    // Act
    let browser = Browser::connect(url).expect("ws:// URL should parse successfully");

    // Assert
    assert!(browser.is_websocket(), "ws:// must route to WebSocket transport");
    assert!(!browser.is_in_memory());
    assert_eq!(browser.scheme(), "ws");
    assert_eq!(browser.url(), "ws://127.0.0.1:9222");
}

// ── SPEC 验收 3: http://... → 自动发现 ws endpoint → WebSocket ────────────

#[test]
fn http_scheme_routes_to_websocket_with_discovery() {
    // Arrange — http:// 在 TASK-1 中合并到 WebSocket 分支(实际 discover 在 TASK-2)
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "http://127.0.0.1:9222";

    // Act
    let browser = Browser::connect(url)
        .expect("http:// URL should parse successfully (TASK-1 routing only, no network)");

    // Assert
    assert!(browser.is_websocket(), "http:// must route to WebSocket transport (with discovery pending in TASK-2)");
    assert_eq!(browser.scheme(), "http");
}

// ── SPEC 验收 4: 错误 scheme → InvalidScheme ─────────────────────────────

#[test]
fn invalid_scheme_returns_invalid_scheme_error() {
    // Arrange
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "ftp://example.com:21";

    // Act
    let result = Browser::connect(url);

    // Assert
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
    // Arrange
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "wss://example.com:443/devtools/page/abc";

    // Act
    let browser = Browser::connect(url).unwrap();

    // Assert
    assert!(browser.is_websocket());
    assert_eq!(browser.scheme(), "wss");
}

#[test]
fn https_scheme_routes_to_websocket() {
    // Arrange
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "https://127.0.0.1:9443";

    // Act
    let browser = Browser::connect(url).unwrap();

    // Assert
    assert!(browser.is_websocket());
    assert_eq!(browser.scheme(), "https");
}

// ── 扩展:空 URL / 无 scheme → InvalidUrl ────────────────────────────────

#[test]
fn empty_url_returns_invalid_url() {
    // Arrange
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "";

    // Act
    let err = Browser::connect(url).expect_err("empty string must fail");

    // Assert
    assert!(matches!(err, ConnectError::InvalidUrl), "empty URL must return InvalidUrl, got {:?}", err);
}

#[test]
fn url_without_scheme_separator_returns_invalid_url() {
    // Arrange — 没有 ://,bun_url 的 URL::parse 返回空 protocol → InvalidUrl
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "localhost:9222";

    // Act
    let err = Browser::connect(url).expect_err("missing-scheme URL must fail");

    // Assert
    assert!(matches!(err, ConnectError::InvalidUrl), "missing-scheme URL must return InvalidUrl, got {:?}", err);
}

#[test]
fn url_with_only_scheme_separator_returns_invalid_url() {
    // Arrange — "://" 前没有 scheme,protocol 空 → InvalidUrl
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "://no-scheme";

    // Act
    let err = Browser::connect(url).expect_err("schemeless URL must fail");

    // Assert
    assert!(matches!(err, ConnectError::InvalidUrl));
}

// ── 扩展:其他常见 scheme 拒绝 ────────────────────────────────────────────

#[test]
fn file_scheme_rejected() {
    // Arrange
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "file:///etc/passwd";

    // Act
    let err = Browser::connect(url).unwrap_err();

    // Assert
    assert!(matches!(err, ConnectError::InvalidScheme(s) if s == "file"));
}

#[test]
fn unix_scheme_rejected() {
    // Arrange
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "unix:///var/run/cdp.sock";

    // Act
    let err = Browser::connect(url).unwrap_err();

    // Assert
    assert!(matches!(err, ConnectError::InvalidScheme(s) if s == "unix"));
}

#[test]
fn javascript_scheme_rejected() {
    // Arrange — 注意:javascript: 没有 //,bun_url 视为无 scheme — InvalidUrl
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "javascript:alert(1)";

    // Act
    let err = Browser::connect(url).unwrap_err();

    // Assert
    assert!(matches!(err, ConnectError::InvalidUrl));
}

// ── 扩展:Browser 元数据 ─────────────────────────────────────────────────

#[test]
fn browser_url_and_scheme_roundtrip() {
    // Arrange
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "memory://bao";

    // Act
    let browser = Browser::connect(url).unwrap();

    // Assert
    assert_eq!(browser.url(), url);
    assert_eq!(browser.scheme(), "memory");
}

#[test]
fn browser_display_includes_url() {
    // Arrange
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let browser = Browser::connect("ws://127.0.0.1:9222").unwrap();

    // Act
    let display = format!("{}", browser);

    // Assert
    assert!(display.contains("ws://127.0.0.1:9222"), "display: {}", display);
    assert!(display.contains("WebSocket"), "display: {}", display);
}

#[test]
fn browser_clone_preserves_parsed_state() {
    // Arrange
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let b1 = Browser::connect("wss://example.com:443/devtools").unwrap();

    // Act
    let b2 = b1.clone();

    // Assert
    assert_eq!(b1.url(), b2.url());
    assert_eq!(b1.scheme(), b2.scheme());
    assert_eq!(b1.transport_kind(), b2.transport_kind());
}

#[test]
fn all_five_supported_schemes_succeed() {
    // Arrange
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let urls = [
        "memory://bao",
        "ws://127.0.0.1:9222",
        "wss://example.com:443",
        "http://127.0.0.1:9222",
        "https://example.com:443",
    ];

    // Act + Assert — memory → InMemory; 其他 → WebSocket
    for url in urls {
        let browser = Browser::connect(url).unwrap_or_else(|e| panic!("{} should succeed: {:?}", url, e));
        let expected = if url.starts_with("memory://") {
            TransportKind::InMemory
        } else {
            TransportKind::WebSocket
        };
        assert_eq!(browser.transport_kind(), expected, "wrong transport kind for {}", url);
    }
}
