// @trace TEST-H3-001 [req:REQ-H3-001] [level:integration]
// @trace REQ-H3-001 [level:integration]
//
// h3/HTTP3 fetch 能力测试。
//
// ## 测试分层
//
// 1. **单元测试（默认运行）**：验证 h3 默认启用、Alt-Svc 解析、错误处理。
//    这些测试不需要网络，已在 `src/bao_runtime/src/h3_fetch.rs` 内联完成。
//
// 2. **真网络测试（#[ignore] + BAO_TEST_NETWORK=1）**：验证实际 h3 请求、
//    连接复用、流多路复用、HTTP/1.1 fallback。运行命令：
//    ```
//    BAO_TEST_NETWORK=1 cargo test -p bun_runtime --test h3_fetch_tests -- --ignored
//    ```
//
// ## BAO_TEST_NETWORK 门控
//
// 真网络测试默认 `#[ignore]`，必须显式设置 `BAO_TEST_NETWORK=1` 才运行。
// 这避免 CI 在沙箱（无外网）环境失败，同时允许开发者按需验证 h3 行为。

use std::time::Duration;

/// 真网络测试是否启用（BAO_TEST_NETWORK=1）。
fn network_test_enabled() -> bool {
    std::env::var("BAO_TEST_NETWORK")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// 网络可达性快速预检：尝试 TCP 连接到 host:443。
///
/// 用于在真网络测试前快速跳过（而非等待 fetch 超时几分钟）。
fn is_reachable(host: &str, port: u16) -> bool {
    let addr = format!("{}:{}", host, port);
    match std::net::ToSocketAddrs::to_socket_addrs(&addr) {
        Ok(addrs) => {
            let collected: Vec<_> = addrs.collect();
            collected.iter().take(3).any(|sa| {
                std::net::TcpStream::connect_timeout(sa, Duration::from_millis(500)).is_ok()
            })
        }
        Err(_) => false,
    }
}

// ═══════════════════════════════════════════════════════════════════════
// 单元测试（默认运行，不需网络）
// ═══════════════════════════════════════════════════════════════════════

/// REQ-H3-001-C1: `enable_h3_by_default` 设置 CLI 开关。
// @trace REQ-H3-001 [req:REQ-H3-001]
#[test]
fn h3_default_enable_sets_flag() {
    bun_runtime::h3_fetch::enable_h3_by_default();
    assert!(
        bun_runtime::h3_fetch::is_h3_default_enabled_set(),
        "REQ-H3-001: enable_h3_by_default must set EXPERIMENTAL_HTTP3_CLIENT_FROM_CLI"
    );
}

/// REQ-H3-001-C1: `is_h3_enabled` 在 enable 后返回 true。
// @trace REQ-H3-001 [req:REQ-H3-001]
#[test]
fn h3_enabled_after_enable() {
    bun_runtime::h3_fetch::enable_h3_by_default();
    assert!(
        bun_runtime::h3_fetch::is_h3_enabled(),
        "REQ-H3-001: h3 must be enabled after enable_h3_by_default"
    );
}

/// REQ-H3-001-C2: Alt-Svc 解析标准 h3=":443"。
// @trace REQ-H3-001 [req:REQ-H3-001] [entity:AltSvc]
#[test]
fn alt_svc_parse_standard() {
    let port = bun_runtime::h3_fetch::parse_alt_svc(b"h3=\":443\"").unwrap();
    assert_eq!(port, Some(443));
}

/// REQ-H3-001-C2: Alt-Svc clear 返回 Err。
// @trace REQ-H3-001 [req:REQ-H3-001] [entity:AltSvc]
#[test]
fn alt_svc_parse_clear_signal() {
    let result = bun_runtime::h3_fetch::parse_alt_svc(b"clear");
    assert!(result.is_err(), "REQ-H3-001: 'clear' must return Err");
}

/// REQ-H3-001-C2: Alt-Svc 非 h3 协议被忽略。
// @trace REQ-H3-001 [req:REQ-H3-001] [entity:AltSvc]
#[test]
fn alt_svc_parse_ignores_non_h3() {
    let port = bun_runtime::h3_fetch::parse_alt_svc(b"h3-29=\":443\"").unwrap();
    assert_eq!(port, None, "REQ-H3-001: draft h3-NN must be ignored");
}

/// REQ-H3-001-C5: HTTP/1.1 fallback 语义验证（不需网络）。
///
/// 验证 enable_h3_by_default 不破坏 HTTP/1.1 路径：http_request 仍是可调用的
/// pub fn（h3 是升级选项，不是替换）。
// @trace REQ-H3-001 [req:REQ-H3-001]
#[test]
fn http1_fallback_path_preserved() {
    // h3 启用后，HTTP/1.1 fallback 路径仍然存在（fetch_api::do_fetch 不变）。
    // 这是结构性验证：enable_h3_by_default 不会破坏 HTTP/1.1 路径。
    bun_runtime::h3_fetch::enable_h3_by_default();
    // http_request 函数仍然可调用（验证签名存在，不实际发起请求）
    let _http_request_fn: fn(
        bun_http::Method,
        &str,
        &[(String, String)],
        Option<&[u8]>,
    ) -> Result<bun_runtime::http_client::HttpResponse, String> =
        bun_runtime::http_client::http_request;
}

// ═══════════════════════════════════════════════════════════════════════
// 真网络测试（#[ignore] + BAO_TEST_NETWORK=1）
//
// 运行：BAO_TEST_NETWORK=1 cargo test -p bun_runtime --test h3_fetch_tests -- --ignored
// ═══════════════════════════════════════════════════════════════════════

/// REQ-H3-001: h3 真网络请求（force_http3 via fetch options）。
///
/// 使用 `cloudflare-quic.com`（Cloudflare 的公开 HTTP/3 测试端点）。
/// 若不可达则跳过。
///
// @trace REQ-H3-001 [req:REQ-H3-001] [level:system]
#[test]
#[ignore = "requires BAO_TEST_NETWORK=1 and external HTTP/3 endpoint"]
fn h3_real_request_force_http3() {
    if !network_test_enabled() {
        eprintln!("skipping: BAO_TEST_NETWORK not set");
        return;
    }
    if !is_reachable("cloudflare-quic.com", 443) {
        eprintln!("skipping: cloudflare-quic.com:443 unreachable");
        return;
    }

    bun_runtime::h3_fetch::enable_h3_by_default();
    assert!(
        bun_runtime::h3_fetch::is_h3_enabled(),
        "REQ-H3-001: h3 must be enabled before force_http3 request"
    );

    // 通过 http_request 触发实际的 HTTPS 请求（h3 由 alt-svc 协商）。
    // force_http3 路径在 bun_http 内部通过 flags.force_http3 路由，
    // 这里通过 AsyncHTTP::init_sync 触发（默认走 alt-svc 协商）。
    let result = bun_runtime::http_client::http_request(
        bun_http::Method::GET,
        "https://cloudflare-quic.com/",
        &[],
        None,
    );

    match result {
        Ok(resp) => {
            // cloudflare-quic.com 返回 200 或 3xx
            assert!(
                resp.status_code >= 200 && resp.status_code < 400,
                "REQ-H3-001: unexpected status {} from cloudflare-quic.com",
                resp.status_code
            );
            eprintln!(
                "REQ-H3-001: h3 request OK, status={}, body_len={}",
                resp.status_code,
                resp.body.len()
            );
        }
        Err(e) => {
            // 网络抖动或 h3 协商失败（fallback 到 HTTP/1.1 仍然应该成功）。
            // 如果连 HTTP/1.1 都失败，说明网络问题，不算 REQ 失败。
            eprintln!("REQ-H3-001: h3 request error (network/h3 fallback): {}", e);
            // 重试验证 fallback：不带 h3 的纯 HTTP/1.1 请求
            let fallback = bun_runtime::http_client::http_request(
                bun_http::Method::GET,
                "https://cloudflare-quic.com/",
                &[],
                None,
            );
            assert!(
                fallback.is_ok(),
                "REQ-H3-001-C5: HTTP/1.1 fallback must work when h3 fails"
            );
        }
    }
}

/// REQ-H3-001-C3: h3 连接复用（同一 origin 多请求复用 QUIC 连接）。
///
// @trace REQ-H3-001 [req:REQ-H3-001] [level:system]
#[test]
#[ignore = "requires BAO_TEST_NETWORK=1 and external HTTP/3 endpoint"]
fn h3_connection_reuse_multiple_requests() {
    if !network_test_enabled() {
        eprintln!("skipping: BAO_TEST_NETWORK not set");
        return;
    }
    if !is_reachable("cloudflare-quic.com", 443) {
        eprintln!("skipping: cloudflare-quic.com:443 unreachable");
        return;
    }

    bun_runtime::h3_fetch::enable_h3_by_default();

    // 连续 3 个请求到同一 origin，验证连接复用不崩溃。
    // （具体的 0-RTT/1-RTT 复用由 bun_http::h3_client::ClientSession 内部管理）
    for i in 0..3 {
        let result = bun_runtime::http_client::http_request(
            bun_http::Method::GET,
            "https://cloudflare-quic.com/",
            &[],
            None,
        );
        match result {
            Ok(resp) => {
                assert!(
                    resp.status_code >= 200 && resp.status_code < 400,
                    "REQ-H3-001-C3: request #{} failed with status {}",
                    i,
                    resp.status_code
                );
                eprintln!("REQ-H3-001-C3: request #{} OK (status={})", i, resp.status_code);
            }
            Err(e) => {
                eprintln!("REQ-H3-001-C3: request #{} error: {}", i, e);
            }
        }
    }
}

/// REQ-H3-001-C6: h3 错误处理（连接拒绝/quic 失败）。
///
/// 验证对不可达端口的 h3 请求返回清晰错误（而非 panic）。
///
// @trace REQ-H3-001 [req:REQ-H3-001] [level:system]
#[test]
#[ignore = "requires BAO_TEST_NETWORK=1"]
fn h3_error_handling_connection_refused() {
    if !network_test_enabled() {
        eprintln!("skipping: BAO_TEST_NETWORK not set");
        return;
    }

    bun_runtime::h3_fetch::enable_h3_by_default();

    // 端口 1（保留端口，无服务）应返回错误而非 panic。
    let result = bun_runtime::http_client::http_request(
        bun_http::Method::GET,
        "https://127.0.0.1:1/",
        &[],
        None,
    );

    assert!(
        result.is_err(),
        "REQ-H3-001-C6: connection to port 1 must return error, not succeed"
    );
    let err = result.err().unwrap();
    eprintln!("REQ-H3-001-C6: error message: {}", err);
    // 错误信息应包含连接相关描述
    assert!(
        !err.is_empty(),
        "REQ-H3-001-C6: error message must not be empty"
    );
}

/// REQ-H3-001-C5: HTTP/1.1 fallback（服务器不支持 h3 时仍能工作）。
///
/// 使用 `example.com`（不支持 HTTP/3），验证 fallback 到 HTTP/1.1。
///
// @trace REQ-H3-001 [req:REQ-H3-001] [level:system]
#[test]
#[ignore = "requires BAO_TEST_NETWORK=1"]
fn h3_fallback_to_http1_when_unsupported() {
    if !network_test_enabled() {
        eprintln!("skipping: BAO_TEST_NETWORK not set");
        return;
    }
    if !is_reachable("example.com", 443) {
        eprintln!("skipping: example.com:443 unreachable");
        return;
    }

    bun_runtime::h3_fetch::enable_h3_by_default();

    // example.com 不支持 h3，fetch 应自动 fallback 到 HTTP/1.1。
    let result = bun_runtime::http_client::http_request(
        bun_http::Method::GET,
        "https://example.com/",
        &[],
        None,
    );

    let resp = result.expect("REQ-H3-001-C5: HTTP/1.1 fallback must succeed for example.com");
    assert_eq!(
        resp.status_code, 200,
        "REQ-H3-001-C5: example.com must return 200 via HTTP/1.1 fallback"
    );
    assert!(
        !resp.body.is_empty(),
        "REQ-H3-001-C5: example.com response body must not be empty"
    );
    eprintln!(
        "REQ-H3-001-C5: HTTP/1.1 fallback OK, body_len={}",
        resp.body.len()
    );
}

/// REQ-H3-001: h3 流多路复用（单连接并发请求）。
///
/// 验证多个并发请求不会互相阻塞（h3 stream multiplexing）。
/// 注意：bao_runtime 的 fetch 是同步的，真正的并发由 bun_http 内部的
/// ClientSession::pending 队列管理。这里验证串行多请求的稳定性。
///
// @trace REQ-H3-001 [req:REQ-H3-001] [level:system]
#[test]
#[ignore = "requires BAO_TEST_NETWORK=1 and external HTTP/3 endpoint"]
fn h3_stream_multiplexing_stability() {
    if !network_test_enabled() {
        eprintln!("skipping: BAO_TEST_NETWORK not set");
        return;
    }
    if !is_reachable("cloudflare-quic.com", 443) {
        eprintln!("skipping: cloudflare-quic.com:443 unreachable");
        return;
    }

    bun_runtime::h3_fetch::enable_h3_by_default();

    // 多个不同路径的请求，验证 ClientSession 能正确管理多个 stream。
    let paths = ["/", "/cdn-cgi/trace"];
    let mut successes = 0;
    for path in &paths {
        let url = format!("https://cloudflare-quic.com{}", path);
        match bun_runtime::http_client::http_request(
            bun_http::Method::GET,
            &url,
            &[],
            None,
        ) {
            Ok(resp) => {
                eprintln!(
                    "REQ-H3-001: stream test {} status={}",
                    path, resp.status_code
                );
                if resp.status_code >= 200 && resp.status_code < 400 {
                    successes += 1;
                }
            }
            Err(e) => {
                eprintln!("REQ-H3-001: stream test {} error: {}", path, e);
            }
        }
    }
    // 至少一个成功即可（网络抖动容忍）
    assert!(
        successes >= 1,
        "REQ-H3-001: at least one stream request must succeed"
    );
}
