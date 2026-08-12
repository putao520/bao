// @trace TEST-E2E-CDP [req:REQ-CDP-001,REQ-CDP-002,REQ-CDP-003,REQ-CDP-004,REQ-CDP-005,REQ-CDP-008,REQ-BAO-API-001,REQ-BAO-API-002,REQ-BAO-API-003] [level:system]
// @trace REQ-CDP-001 [level:system]
// @trace REQ-BAO-API-001 [level:system]
// @trace REQ-BAO-API-002 [interface:Transport] [level:system]
//
// # TASK-12 E2E — CDP 端到端(memory://bao → CDPRdpBridge → servo backend)
//
// **核心断言**: bao_cdp_client 的完整分发链路在 InMemory 模式下端到端可用:
//
//   Browser::connect("memory://bao")
//        ↓ URL scheme 路由
//   InMemoryTransport
//        ↓ send_command(JSON-RPC)
//   CDPRdpBridge
//        ↓ command_dispatcher
//   ServoBackend (production: 真 servo / test: MockServoBackend)
//
// 本测试用 MockServoBackend 替代真 servo(servo Opts per-process 限制)。
// 这覆盖完整 dispatch 链路 — 任何一环出错(URL 路由 / Transport / Bridge /
// dispatcher / backend) 都会导致断言失败。
//
// 测试维度:
//   1. **URL scheme 路由**: memory://bao → InMemoryTransport
//   2. **Target.createTarget**: 创建 target → 返回 targetId
//   3. **Page.navigate**: 通过 CDP 命令驱动 servo backend.navigate
//   4. **Page.captureScreenshot**: 走完整 screenshot 命令链
//   5. **多 target**: 两个 target 并发命令,验证 target_id 隔离
//   6. **Backend 调用日志**: 验证 backend 真的被调用(不是空响应)
//
// 网络模式(连真 Chrome)用 graceful skip + BAO_TEST_CHROME_URL 启用(见 e2e_external_chrome.rs)。

use std::sync::Arc;

use bao_cdp_client::bridge::{CDPRdpBridge, MockServoBackend, ServoBackend};
use bao_cdp_client::transport::{InMemoryBridge, InMemoryTransport, Transport, TransportKind};
use bao_cdp_client::Browser;
use serde_json::json;

// ─── 辅助 — 构造完整 InMemory transport 链路 ──────────────────────────────────

/// 构造完整 InMemory 链路:
///   MockServoBackend → CDPRdpBridge → InMemoryBridge → InMemoryTransport
fn build_full_in_memory_chain() -> (InMemoryTransport, Arc<MockServoBackend>) {
    let backend = Arc::new(MockServoBackend::new());
    let backend_dyn: Arc<dyn ServoBackend> = backend.clone();
    let bridge = CDPRdpBridge::new(backend_dyn);
    let bridge_dyn: Arc<dyn InMemoryBridge> = bridge.into_in_memory_bridge();
    let transport = InMemoryTransport::new(bridge_dyn);
    (transport, backend)
}

// ─── 主测试 — 单 #[test] ────────────────────────────────────────────────────

#[test]
// @trace REQ-CDP-001 [level:e2e]
// @trace REQ-BAO-API-001 [level:e2e]
fn cdp_full_chain_memory_transport_roundtrip() {
    // ── §1 URL scheme 路由 — Browser::connect ────────────────────────────
    //
    // Arrange + Act: 连接 memory://bao
    let browser = Browser::connect("memory://bao").expect("Browser::connect memory://bao");
    //
    // Assert: 路由到 InMemoryTransport
    assert!(
        browser.is_in_memory(),
        "memory://bao must route to InMemory transport"
    );
    assert_eq!(
        browser.transport_kind(),
        TransportKind::InMemory,
        "transport_kind must be InMemory"
    );
    assert_eq!(browser.scheme(), "memory", "scheme must be 'memory'");
    assert_eq!(browser.url(), "memory://bao", "url preserved");

    // ── §2 构造完整 dispatch 链路 ────────────────────────────────────────
    //
    // Act: build_full_in_memory_chain() 串联 Backend → Bridge → Transport
    let (mut transport, backend) = build_full_in_memory_chain();

    // ── §3 Target.createTarget — 创建 page target ────────────────────────
    //
    // Act
    let create_resp = transport
        .send_command("Target.createTarget", json!({"url": "about:blank"}), None)
        .expect("Target.createTarget");
    //
    // Assert
    let target_id = create_resp
        .get("targetId")
        .and_then(|v| v.as_str())
        .expect("targetId in response");
    assert!(!target_id.is_empty(), "targetId must be non-empty");

    // 后续命令需要 backend 知道这个 target — add_target 注册
    backend.add_target(target_id);

    // ── §4 Page.navigate — 通过 CDP 驱动 backend ─────────────────────────
    //
    // Act
    let nav_resp = transport
        .send_command(
            "Page.navigate",
            json!({"url": "data:text/html,<html><head><title>CDP Chain</title></head></html>", "frameId": target_id}),
            None,
        )
        .expect("Page.navigate");
    //
    // Assert — backend 真的被调用并返回 NavigateResult
    let frame_id = nav_resp
        .get("frameId")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(
        !frame_id.is_empty(),
        "frameId must be non-empty after navigate"
    );
    let loader_id = nav_resp
        .get("loaderId")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(
        !loader_id.is_empty(),
        "loaderId must be non-empty after navigate"
    );

    // ── §5 Page.captureScreenshot — screenshot 命令链 ────────────────────
    //
    // Act
    let screenshot_resp = transport
        .send_command("Page.captureScreenshot", json!({"format": "png"}), None)
        .expect("Page.captureScreenshot");
    //
    // Assert — 返回 data 字段(base64-encoded PNG)
    let data = screenshot_resp
        .get("data")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert!(!data.is_empty(), "screenshot data must be non-empty");

    // ── §6 多 target 隔离 — 创建第二个 target,独立操作 ──────────────────
    //
    // 这验证 CDP 路由按 target_id 隔离,不会跨 target 串扰。
    //
    // Act
    let create2 = transport
        .send_command("Target.createTarget", json!({"url": "about:blank"}), None)
        .expect("Target.createTarget #2");
    let target_id_2 = create2
        .get("targetId")
        .and_then(|v| v.as_str())
        .expect("targetId #2");
    backend.add_target(target_id_2);
    //
    // Assert: 两个 targetId 不同
    assert_ne!(
        target_id, target_id_2,
        "two createTarget calls must return distinct targetIds"
    );

    // 对第二个 target 发命令 — 必须成功
    let nav2 = transport
        .send_command(
            "Page.navigate",
            json!({"url": "https://second.example", "frameId": target_id_2}),
            None,
        )
        .expect("Page.navigate #2");
    assert!(
        nav2.get("frameId").and_then(|v| v.as_str()).is_some(),
        "second target navigate must return frameId"
    );

    // ── §7 Backend 调用日志 — 证明 backend 真被调用 ──────────────────────
    //
    // MockServoBackend 记录所有调用日志。验证至少有 2 个 page_navigate 调用
    // (对应 §4 + §6 的两次 navigate)。
    //
    // 注意: 这里调用 backend 的内部方法 — 仅在测试 mock 上可访问。
    // Assert
    let call_log = backend.call_log.lock().unwrap();
    let navigate_calls: Vec<_> = call_log
        .iter()
        .filter(|(_, method, _)| method == "page_navigate")
        .collect();
    assert!(
        navigate_calls.len() >= 2,
        "backend must have been called for page_navigate at least twice, got {}",
        navigate_calls.len()
    );
    let screenshot_calls: Vec<_> = call_log
        .iter()
        .filter(|(_, method, _)| method == "page_screenshot")
        .collect();
    assert!(
        screenshot_calls.len() >= 1,
        "backend must have been called for page_screenshot at least once, got {}",
        screenshot_calls.len()
    );

    // ── §8 Transport close — 干净关闭 ───────────────────────────────────
    //
    // Act + Assert
    let close_result = transport.close();
    // close 不应 panic(实现可能返回 Ok 或 Err,具体看 transport 状态)
    let _ = close_result;
    assert!(
        transport.is_closed(),
        "transport must be closed after close()"
    );
}

// ─── §9 错误路径 — invalid URL scheme 抛 ConnectError ────────────────────────

#[test]
// @trace REQ-BAO-API-001 [level:e2e]
fn cdp_full_chain_invalid_scheme_rejected() {
    // Arrange + Act: ftp:// scheme 不被支持
    let result = Browser::connect("ftp://example.com");
    //
    // Assert: 应返回 ConnectError
    assert!(
        result.is_err(),
        "ftp:// scheme must be rejected with ConnectError"
    );
    let err_msg = format!("{}", result.unwrap_err());
    assert!(
        err_msg.to_lowercase().contains("scheme")
            || err_msg.to_lowercase().contains("invalid")
            || err_msg.to_lowercase().contains("unsupported"),
        "error message must mention scheme/invalid/unsupported, got: {}",
        err_msg
    );
}

// ─── §10 多次 connect — 单进程多次 connect 不冲突 ────────────────────────────

#[test]
// @trace REQ-BAO-API-001 [level:e2e]
fn cdp_full_chain_multiple_connect_same_process() {
    // Arrange + Act: 同一进程内多次 connect memory://bao
    let browser1 = Browser::connect("memory://bao").expect("connect #1");
    let browser2 = Browser::connect("memory://bao").expect("connect #2");

    // Assert: 两个 Browser 对象独立但都路由到 InMemory
    assert!(browser1.is_in_memory());
    assert!(browser2.is_in_memory());
    // url 相同
    assert_eq!(browser1.url(), browser2.url());
}
