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
    assert!(
        browser.is_in_memory(),
        "memory://bao must route to InMemory transport"
    );
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

    // Act — lazy connect:只解析 URL,不触发实际 WebSocket 连接
    let browser = Browser::connect(url).expect("ws:// URL should parse successfully");

    // Assert
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
    // Arrange — http:// 在 TASK-1 中合并到 WebSocket 分支(实际 discover 在 TASK-2)
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "http://127.0.0.1:9222";

    // Act
    let browser = Browser::connect(url)
        .expect("http:// URL should parse successfully (TASK-1 routing only, no network)");

    // Assert
    assert!(
        browser.is_websocket(),
        "http:// must route to WebSocket transport (with discovery pending in TASK-2)"
    );
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
            assert_eq!(
                scheme, "ftp",
                "InvalidScheme must carry the offending scheme"
            );
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
    assert!(
        matches!(err, ConnectError::InvalidUrl),
        "empty URL must return InvalidUrl, got {:?}",
        err
    );
}

#[test]
fn url_without_scheme_separator_returns_invalid_url() {
    // Arrange — 没有 ://,bun_url 的 URL::parse 返回空 protocol → InvalidUrl
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "localhost:9222";

    // Act
    let err = Browser::connect(url).expect_err("missing-scheme URL must fail");

    // Assert
    assert!(
        matches!(err, ConnectError::InvalidUrl),
        "missing-scheme URL must return InvalidUrl, got {:?}",
        err
    );
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
    assert!(
        display.contains("ws://127.0.0.1:9222"),
        "display: {}",
        display
    );
    assert!(display.contains("WebSocket"), "display: {}", display);
}

#[test]
fn browser_connect_preserves_parsed_state() {
    // Arrange
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    // Browser is not Clone (holds Connection which contains Box<dyn Transport>).
    // Verify that two independent connect calls produce equivalent parsed state.
    let b1 = Browser::connect("wss://example.com:443/devtools").unwrap();

    // Act — second connect for same URL
    let b2 = Browser::connect("wss://example.com:443/devtools").unwrap();

    // Assert — parsed state equivalent across two instances
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
        let browser =
            Browser::connect(url).unwrap_or_else(|e| panic!("{} should succeed: {:?}", url, e));
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

// ── 对抗验证:scheme 大小写敏感(锁定 SPEC 合约:scheme 必须小写集合) ──────
//
// SPEC 路由规则要求 scheme 精确匹配 {memory, ws, wss, http, https}(全小写)。
// 任何大小写变体(MEMORY / Memory / WS / Ws)必须落入 InvalidScheme 且保留原样
// scheme 字符串,禁止静默小写化后接受(否则用户无法察觉拼写错误)。

#[test]
fn uppercase_memory_scheme_rejected() {
    // Arrange — bun_url 协议字节区分大小写,"MEMORY" 不在白名单
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "MEMORY://bao";

    // Act
    let err = Browser::connect(url).expect_err("uppercase MEMORY:// must not be silently accepted");

    // Assert — InvalidScheme 携带原始大小写,便于诊断
    match err {
        ConnectError::InvalidScheme(s) => {
            assert_eq!(
                s, "MEMORY",
                "InvalidScheme must preserve original case for diagnosis"
            );
        }
        other => panic!("expected InvalidScheme, got {:?}", other),
    }
}

#[test]
fn mixed_case_memory_scheme_rejected() {
    // Arrange
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "Memory://bao";

    // Act
    let err = Browser::connect(url).expect_err("mixed-case Memory:// must fail");

    // Assert
    assert!(
        matches!(err, ConnectError::InvalidScheme(ref s) if s == "Memory"),
        "mixed-case scheme must be InvalidScheme preserving case, got {:?}",
        err
    );
}

#[test]
fn uppercase_ws_scheme_rejected() {
    // Arrange
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "WS://127.0.0.1:9222";

    // Act
    let err = Browser::connect(url).expect_err("uppercase WS:// must fail");

    // Assert
    assert!(
        matches!(err, ConnectError::InvalidScheme(ref s) if s == "WS"),
        "uppercase WS must be InvalidScheme, got {:?}",
        err
    );
}

// ── 对抗验证:scheme 缺 `//` 分隔符 → InvalidUrl(SPEC 合约:必须 `://`) ──
//
// SPEC docstring 明确"空串或无 `://` 返回 InvalidUrl"。`memory:` / `tel:+1234`
// / `mailto:a@b` 这类只有冒号没有 `//` 的 URL,bun_url protocol 为空,必须 InvalidUrl
// 而非 InvalidScheme(因为根本没解析出有效 scheme)。

#[test]
fn memory_scheme_without_separator_returns_invalid_url() {
    // Arrange — `memory:` 没有 `//`,bun_url 视为无 protocol
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "memory:";

    // Act
    let err = Browser::connect(url).expect_err("memory: (no //) must fail");

    // Assert
    assert!(
        matches!(err, ConnectError::InvalidUrl),
        "scheme without `//` must be InvalidUrl not InvalidScheme, got {:?}",
        err
    );
}

#[test]
fn ws_scheme_without_separator_returns_invalid_url() {
    // Arrange
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "ws:";

    // Act
    let err = Browser::connect(url).expect_err("ws: (no //) must fail");

    // Assert
    assert!(matches!(err, ConnectError::InvalidUrl), "got {:?}", err);
}

#[test]
fn tel_scheme_no_separator_returns_invalid_url() {
    // Arrange — tel: URL 没有 //,protocol 空
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "tel:+1234";

    // Act
    let err = Browser::connect(url).expect_err("tel: must fail");

    // Assert
    assert!(matches!(err, ConnectError::InvalidUrl), "got {:?}", err);
}

#[test]
fn mailto_scheme_no_separator_returns_invalid_url() {
    // Arrange
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "mailto:a@b.com";

    // Act
    let err = Browser::connect(url).expect_err("mailto: must fail");

    // Assert
    assert!(matches!(err, ConnectError::InvalidUrl), "got {:?}", err);
}

// ── 对抗验证:协议相对 URL / 裸 host / 裸 port → InvalidUrl ───────────────

#[test]
fn protocol_relative_url_returns_invalid_url() {
    // Arrange — `//host` 没有 scheme
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "//127.0.0.1:9222";

    // Act
    let err = Browser::connect(url).expect_err("protocol-relative URL must fail");

    // Assert
    assert!(matches!(err, ConnectError::InvalidUrl), "got {:?}", err);
}

#[test]
fn bare_ip_without_scheme_returns_invalid_url() {
    // Arrange
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "127.0.0.1";

    // Act
    let err = Browser::connect(url).expect_err("bare IP must fail");

    // Assert
    assert!(matches!(err, ConnectError::InvalidUrl), "got {:?}", err);
}

#[test]
fn bare_port_returns_invalid_url() {
    // Arrange
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "9222";

    // Act
    let err = Browser::connect(url).expect_err("bare port must fail");

    // Assert
    assert!(matches!(err, ConnectError::InvalidUrl), "got {:?}", err);
}

// ── 对抗验证:扩展拒绝 scheme 集合(SPEC 合约:仅 5 个 scheme 合法) ─────────
//
// SPEC docstring: 非 {memory, ws, wss, http, https} 的 scheme 返回 InvalidScheme。
// 锁定常见错误 scheme,且断言携带的 scheme 字符串精确匹配(供诊断)。

#[test]
fn tcp_scheme_rejected_with_exact_string() {
    // Arrange
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "tcp://127.0.0.1:9222";

    // Act
    let err = Browser::connect(url).expect_err("tcp:// must fail");

    // Assert
    assert!(
        matches!(err, ConnectError::InvalidScheme(ref s) if s == "tcp"),
        "expected InvalidScheme(\"tcp\"), got {:?}",
        err
    );
}

#[test]
fn ssh_scheme_rejected_with_exact_string() {
    // Arrange
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "ssh://git@github.com";

    // Act
    let err = Browser::connect(url).expect_err("ssh:// must fail");

    // Assert
    assert!(
        matches!(err, ConnectError::InvalidScheme(ref s) if s == "ssh"),
        "expected InvalidScheme(\"ssh\"), got {:?}",
        err
    );
}

#[test]
fn data_scheme_rejected() {
    // Arrange — data: URL 无 `//`,protocol 空 → InvalidUrl(非 InvalidScheme)
    // 锁定 SPEC 合约:`//` 是 scheme 有效性的前置条件
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "data:text/html,<p>hi</p>";

    // Act
    let err = Browser::connect(url).expect_err("data: must fail");

    // Assert
    assert!(
        matches!(err, ConnectError::InvalidUrl),
        "data: (no //) must be InvalidUrl, got {:?}",
        err
    );
}

// ── 对抗验证:空 host 边界(SPEC 未禁止空 host,锁定可接受行为) ───────────
//
// `memory://` / `ws://` / `wss://` / `http://` / `https://` 的 host 为空但
// scheme 有效。SPEC docstring 不要求校验 host 非空(TASK-1 只做 scheme 路由,
// 实际 host 校验在 TASK-2 握手阶段)。锁定:connect 成功 + scheme 正确 +
// transport_kind 正确,禁止 connect 阶段做 host 校验(避免与 lazy connect 合约冲突)。

#[test]
fn empty_host_memory_routes_successfully() {
    // Arrange
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "memory://";

    // Act
    let browser = Browser::connect(url).expect("memory:// (empty host) should succeed at routing");

    // Assert — 路由阶段不做 host 校验,只做 scheme 路由
    assert_eq!(browser.scheme(), "memory");
    assert_eq!(browser.transport_kind(), TransportKind::InMemory);
    assert!(browser.is_in_memory());
    assert!(!browser.is_websocket());
}

#[test]
fn empty_host_ws_routes_to_websocket() {
    // Arrange
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "ws://";

    // Act
    let browser = Browser::connect(url).expect("ws:// (empty host) should succeed at routing");

    // Assert — host 校验留给 build_websocket_transport(TCP 握手阶段)
    assert_eq!(browser.scheme(), "ws");
    assert_eq!(browser.transport_kind(), TransportKind::WebSocket);
    assert!(browser.is_websocket());
    assert!(!browser.is_in_memory());
}

#[test]
fn empty_host_all_four_ws_schemes_route() {
    // Arrange — ws/wss/http/https 空 host 都路由到 WebSocket
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let urls = ["ws://", "wss://", "http://", "https://"];

    // Act + Assert
    for url in urls {
        let browser =
            Browser::connect(url).unwrap_or_else(|e| panic!("{} should succeed: {:?}", url, e));
        assert_eq!(
            browser.transport_kind(),
            TransportKind::WebSocket,
            "empty-host {} must route to WebSocket",
            url
        );
    }
}

// ── 对抗验证:复杂 host/port/path/query/userinfo(URL 结构保留) ──────────
//
// URL 经 route() 解析后必须保留原始字符串(url() 往返不变)。锁定 IPv6 / 长路径
// / query / userinfo 等结构的 roundtrip 完整性,禁止 route() 截断或重写 URL。

#[test]
fn ipv6_host_preserved() {
    // Arrange
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "ws://[::1]:9222";

    // Act
    let browser = Browser::connect(url).expect("IPv6 host should parse");

    // Assert
    assert!(browser.is_websocket());
    assert_eq!(browser.scheme(), "ws");
    assert_eq!(browser.url(), url, "IPv6 URL must roundtrip unchanged");
}

#[test]
fn nested_path_preserved() {
    // Arrange
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "ws://127.0.0.1:9222/devtools/page/abc/def";

    // Act
    let browser = Browser::connect(url).unwrap();

    // Assert
    assert_eq!(browser.url(), url, "nested path must roundtrip unchanged");
    assert_eq!(browser.scheme(), "ws");
}

#[test]
fn query_string_preserved() {
    // Arrange
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "ws://127.0.0.1:9222?token=secret&v=2";

    // Act
    let browser = Browser::connect(url).unwrap();

    // Assert
    assert_eq!(browser.url(), url, "query string must roundtrip unchanged");
}

#[test]
fn fragment_preserved() {
    // Arrange
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "wss://example.com:443/devtools#section";

    // Act
    let browser = Browser::connect(url).unwrap();

    // Assert
    assert_eq!(browser.url(), url, "fragment must roundtrip unchanged");
}

#[test]
fn userinfo_preserved() {
    // Arrange
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "ws://user:p%40ss@host:9222";

    // Act
    let browser = Browser::connect(url).unwrap();

    // Assert
    assert_eq!(
        browser.url(),
        url,
        "userinfo (incl. percent-encoded) must roundtrip unchanged"
    );
}

#[test]
fn port_zero_preserved() {
    // Arrange — port 0 是合法端口值(bun_url 不做端口范围校验,TASK-1 只路由)
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "ws://127.0.0.1:0";

    // Act
    let browser = Browser::connect(url).expect("port 0 should route successfully");

    // Assert
    assert_eq!(browser.url(), url);
    assert_eq!(browser.scheme(), "ws");
}

// ── 对抗验证:is_in_memory / is_websocket 互斥性(SPEC 不变量) ─────────────
//
// SPEC Browser API 不变量:任意成功 connect 后,is_in_memory() 和 is_websocket()
// 必须互斥(恰好一真一假),禁止两者同真或同假。

#[test]
fn in_memory_and_websocket_are_mutually_exclusive() {
    // Arrange
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let mem = Browser::connect("memory://bao").unwrap();
    let ws = Browser::connect("ws://127.0.0.1:9222").unwrap();
    let wss = Browser::connect("wss://x:443").unwrap();
    let http = Browser::connect("http://127.0.0.1:9222").unwrap();
    let https = Browser::connect("https://x:443").unwrap();

    // Act + Assert — 互斥不变量(按引用遍历避免 move)
    let browsers: [(&str, &Browser); 5] = [
        ("memory", &mem),
        ("ws", &ws),
        ("wss", &wss),
        ("http", &http),
        ("https", &https),
    ];
    for (label, b) in browsers {
        assert!(
            b.is_in_memory() ^ b.is_websocket(),
            "{}: is_in_memory() and is_websocket() must be mutually exclusive (got {} {})",
            label,
            b.is_in_memory(),
            b.is_websocket()
        );
    }

    // memory 唯一为 in_memory;其余四个为 websocket
    assert!(mem.is_in_memory() && !mem.is_websocket());
    for b in [&ws, &wss, &http, &https] {
        assert!(!b.is_in_memory() && b.is_websocket());
    }
}

// ── 对抗验证:transport_kind / scheme / url 三者一致性 ────────────────────

#[test]
fn scheme_url_transport_consistency_for_all_supported_schemes() {
    // Arrange — 每个 scheme 的 (url, scheme, transport_kind) 三元组锁定
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let cases: &[(&str, &str, TransportKind)] = &[
        ("memory://bao", "memory", TransportKind::InMemory),
        ("ws://127.0.0.1:9222", "ws", TransportKind::WebSocket),
        ("wss://example.com:443", "wss", TransportKind::WebSocket),
        ("http://127.0.0.1:9222", "http", TransportKind::WebSocket),
        ("https://example.com:443", "https", TransportKind::WebSocket),
    ];

    // Act + Assert
    for (url, expected_scheme, expected_kind) in cases {
        let browser = Browser::connect(url).unwrap_or_else(|e| panic!("{} failed: {:?}", url, e));
        assert_eq!(
            browser.scheme(),
            *expected_scheme,
            "scheme mismatch for {}",
            url
        );
        assert_eq!(
            browser.transport_kind(),
            *expected_kind,
            "kind mismatch for {}",
            url
        );
        assert_eq!(browser.url(), *url, "url roundtrip mismatch for {}", url);
        // is_in_memory / is_websocket 与 transport_kind 一致
        assert_eq!(
            browser.is_in_memory(),
            *expected_kind == TransportKind::InMemory
        );
        assert_eq!(
            browser.is_websocket(),
            *expected_kind == TransportKind::WebSocket
        );
    }
}

// ── 对抗验证:connect 幂等性(同 URL 多次 connect 状态一致) ────────────────

#[test]
fn connect_is_idempotent_for_same_url() {
    // Arrange
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "ws://127.0.0.1:9222";

    // Act
    let b1 = Browser::connect(url).unwrap();
    let b2 = Browser::connect(url).unwrap();
    let b3 = Browser::connect(url).unwrap();

    // Assert — 三次 connect 产生完全等价的解析状态
    assert_eq!(b1.url(), b2.url());
    assert_eq!(b2.url(), b3.url());
    assert_eq!(b1.scheme(), b2.scheme());
    assert_eq!(b1.transport_kind(), b2.transport_kind());
    assert_eq!(b1.transport_kind(), b3.transport_kind());
}

// ── 对抗验证:Display 格式契约(SPEC docstring 承诺的格式) ────────────────
//
// Browser Display 格式:`Browser(<url>, kind=<TransportKind>)`。
// ConnectError::InvalidScheme Display:`invalid URL scheme: "<s>" (expected memory/ws/wss/http/https)`。
// ConnectError::InvalidUrl Display:`invalid URL (empty or missing scheme)`。
// 锁定这些格式契约,防止 Display 实现漂移破坏下游诊断/日志解析。

#[test]
fn browser_display_format_contract_for_each_scheme() {
    // Arrange — 每个 scheme 的 Display 格式锁定
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let cases: &[(&str, &str)] = &[
        ("memory://bao", "memory://bao"),
        ("ws://127.0.0.1:9222", "ws://127.0.0.1:9222"),
    ];

    // Act + Assert
    for (url, expected_url) in cases {
        let browser = Browser::connect(url).unwrap();
        let display = format!("{}", browser);
        assert!(
            display.contains(expected_url),
            "Display format must contain URL: got [{}]",
            display
        );
    }
}

#[test]
fn browser_debug_format_contract() {
    // Arrange — Debug 格式含 url / scheme / transport_kind / connected 字段
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let browser = Browser::connect("wss://x:443").unwrap();

    // Act
    let debug = format!("{:?}", browser);

    // Assert
    assert!(
        debug.contains("url"),
        "Debug must expose url field: {}",
        debug
    );
    assert!(
        debug.contains("scheme"),
        "Debug must expose scheme field: {}",
        debug
    );
    assert!(
        debug.contains("transport_kind"),
        "Debug must expose transport_kind: {}",
        debug
    );
    assert!(
        debug.contains("wss://x:443"),
        "Debug must contain url: {}",
        debug
    );
}

// ── 对抗验证:ConnectError Display + Error trait 契约 ──────────────────────

#[test]
fn invalid_scheme_display_format_contract() {
    // Arrange
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let err = Browser::connect("ftp://example.com:21").unwrap_err();

    // Act
    let display = format!("{}", err);

    // Assert — SPEC error.rs docstring 承诺的格式
    assert!(
        display.contains("invalid URL scheme"),
        "InvalidScheme Display must mention 'invalid URL scheme': got [{}]",
        display
    );
    assert!(
        display.contains("\"ftp\""),
        "InvalidScheme Display must quote the offending scheme: got [{}]",
        display
    );
    assert!(
        display.contains("memory") && display.contains("ws") && display.contains("http"),
        "InvalidScheme Display must list supported schemes: got [{}]",
        display
    );
}

#[test]
fn invalid_url_display_format_contract() {
    // Arrange
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let err = Browser::connect("").unwrap_err();

    // Act
    let display = format!("{}", err);

    // Assert
    assert!(
        display.contains("invalid URL"),
        "InvalidUrl Display must mention 'invalid URL': got [{}]",
        display
    );
}

#[test]
fn connect_error_implements_std_error() {
    // Arrange — ConnectError 必须实现 std::error::Error(可用 ? 传播)
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let err = Browser::connect("ftp://x").unwrap_err();

    // Act + Assert — 编译期 trait bound 检查 + source() 可调
    fn takes_error<E: std::error::Error>(_e: &E) {}
    takes_error(&err);

    // source() 对 ConnectError 目前为 None(无 nested cause),锁定非 panic
    let src = std::error::Error::source(&err);
    assert!(
        src.is_none(),
        "ConnectError::InvalidScheme source should be None"
    );
}

#[test]
fn connect_error_debug_roundtrip() {
    // Arrange — Browser 没有实现 Clone(含 Box<dyn Transport>);
    // 验证 Debug 格式化和多次 connect 等价性
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let b1 = Browser::connect("https://example.com:443").unwrap();

    // Act — second connect for same URL
    let b2 = Browser::connect("https://example.com:443").unwrap();

    // Assert — 两次 connect 产生等价的解析状态
    assert_eq!(b1.url(), b2.url());
    assert_eq!(b1.scheme(), b2.scheme());
    assert_eq!(b1.transport_kind(), b2.transport_kind());
    assert_eq!(b1.is_in_memory(), b2.is_in_memory());
    assert_eq!(b1.is_websocket(), b2.is_websocket());

    // Debug 输出等价
    assert_eq!(format!("{:?}", b1), format!("{:?}", b2));
}

// ── 对抗验证:lazy connect 合约(http:// 路由不触发网络) ───────────────────
//
// SPEC docstring: "TASK-1 不实际发请求(避免网络副作用)"。`http://` 路由必须
// 不发任何 HTTP 请求(GET /json/version 留给 TASK-2)。锁定:connect 立即返回
// 且不阻塞(无网络往返即瞬时完成)。

#[test]
fn http_routing_does_not_trigger_network_io() {
    // Arrange — 用一个几乎肯定无 CDP server 监听的端口;若 connect 发网络请求,
    // 要么阻塞要么超时失败。lazy connect 合约下必须立即成功。
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let url = "http://127.0.0.1:1"; // port 1 通常被拒绝,无 CDP server

    // Act — 必须瞬时返回 Ok,不能因 port 1 无服务而失败
    let start = std::time::Instant::now();
    let browser = Browser::connect(url).expect("lazy connect must not hit network");
    let elapsed = start.elapsed();

    // Assert — 路由成功 + 耗时极短(无 TCP 握手 / HTTP 请求)
    assert!(
        browser.is_websocket(),
        "http:// must route to WebSocket (discovery pending)"
    );
    assert_eq!(browser.scheme(), "http");
    assert!(
        elapsed.as_millis() < 1000,
        "lazy connect must complete in <1s (no network I/O), took {:?}",
        elapsed
    );
}

// ── 对抗验证:build_transport 类型守卫(scheme 与构造方式不匹配 → InvalidScheme) ──
//
// Browser::build_in_memory_transport 要求当前 URL 是 memory://;build_websocket_transport
// 要求当前 URL 是 ws/wss/http/https。锁定:类型不匹配时返回 InvalidScheme(携带
// 诊断信息),且不触发网络(在 TCP 握手前就拒绝)。

#[test]
fn build_in_memory_transport_rejects_ws_url_without_network() {
    // Arrange — ws URL 上调 build_in_memory_transport 必须 InvalidScheme
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    use bao_cdp_client::transport::{InMemoryBridge, InMemoryBridgeResponse};
    use serde_json::Value;
    use std::sync::Arc;

    struct NoopBridge;
    impl InMemoryBridge for NoopBridge {
        fn dispatch_command(&self, _: &str, _: Value, _: Option<&str>) -> InMemoryBridgeResponse {
            unreachable!("build_in_memory_transport must reject before dispatch")
        }
    }

    let browser = Browser::connect("ws://127.0.0.1:9222").unwrap();
    let bridge: Arc<dyn InMemoryBridge> = Arc::new(NoopBridge);

    // Act
    let err = browser
        .build_in_memory_transport(bridge)
        .expect_err("ws URL must reject in_memory build");

    // Assert — InvalidScheme 且携带诊断,且 dispatch_command 未被调用(unreachable! 没 panic)
    match err {
        ConnectError::InvalidScheme(msg) => {
            assert!(
                msg.contains("memory") || msg.contains("ws"),
                "diagnostic must mention scheme mismatch: got {}",
                msg
            );
        }
        other => panic!("expected InvalidScheme for ws+in_memory, got {:?}", other),
    }
}

#[test]
fn build_websocket_transport_rejects_memory_url_without_network() {
    // Arrange — memory URL 上调 build_websocket_transport 必须 InvalidScheme,
    // 且不能触发 TCP 握手(否则会卡住或失败)
    // @trace REQ-BAO-API-001 [interface:Browser] [level:integration]
    let browser = Browser::connect("memory://bao").unwrap();

    // Act
    let err = browser
        .build_websocket_transport()
        .expect_err("memory URL must reject ws build");

    // Assert
    match err {
        ConnectError::InvalidScheme(msg) => {
            assert!(
                msg.contains("ws") || msg.contains("http") || msg.contains("memory"),
                "diagnostic must mention scheme mismatch: got {}",
                msg
            );
        }
        other => panic!(
            "expected InvalidScheme for memory+websocket, got {:?}",
            other
        ),
    }
}
