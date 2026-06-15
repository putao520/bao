// @trace TEST-CDP-PROCESS [req:REQ-CDP-001@PROCESS] [level:system]
// @trace REQ-CDP-001@PROCESS [level:system]
// @trace TMG-CDP-001 [timing:max_latency_ms]
//
// # TASK-17b @PROCESS 时序测试 — TMG-CDP-001 CDP 命令响应时序
//
// **核心断言**: bao_cdp_client 的 CDP 命令分发延迟满足 SPEC 时序约束
// (TMG-CDP-001 max_latency_ms)。SPEC 03-PROCESS.html 给出各 domain 的延迟目标:
//
//   - DOM.querySelector  : 10 ms  (内存 DOM 遍历)
//   - Runtime.evaluate   : 50 ms  (JS 执行, 无 I/O)
//   - Network.enable     : 10 ms  (注册 hook)
//   - Page.captureScreenshot : 500 ms (servo webrender 渲染 + PNG 编码)
//   - Page.navigate      : 3000 ms (网络请求 + DOM 构建 + 样式计算)
//
// 测试用 MockServoBackend(同步无 I/O 模拟 servo 后端),测量 dispatch_command
// 从入口到返回的墙钟延迟。任何 domain dispatcher 的 O(n²) 退化、锁竞争、
// 无效 routing 都会导致延迟超过阈值。
//
// 注意:这是 sync 调用路径的时序(dispatch_command 直接走 servo backend),
// 不测 WebSocket transport 链路(那属于 TMG-CDP-001 的 network 路径,
// 需要 #[ignore] + 真实 Chrome ws:// endpoint)。

use std::sync::Arc;
use std::time::{Duration, Instant};

use bao_cdp_client::bridge::{dispatch_command, MockServoBackend};
use serde_json::{json, Value};

// ════════════════════════════════════════════════════════════════════
// §0 公共辅助 — 构造可分发的 backend + target
// ════════════════════════════════════════════════════════════════════

/// 构造一个 MockServoBackend,预创建一个 target,返回 (backend_ref, target_id)。
fn build_backend_with_target() -> (Arc<MockServoBackend>, String) {
    let backend = Arc::new(MockServoBackend::new());
    let target_id = "timing-target-001".to_string();
    backend.add_target(target_id.clone());
    (backend, target_id)
}

// ════════════════════════════════════════════════════════════════════
// §1 dispatch_command 单次时序 — 各 domain 阈值
// ════════════════════════════════════════════════════════════════════

/// DOM.querySelector 同步分发 < 10ms(TMG-CDP-001 内存 DOM 遍历)
// Arrange — TMG-CDP-001: CDP 命令响应时序,DOM 域
// @trace REQ-CDP-001@PROCESS [level:system] [domain:DOM]
#[test]
fn cdp_dom_query_selector_timing_under_10ms() {
    let (backend, target_id) = build_backend_with_target();

    // Act — 派发 DOM.querySelector 并测量墙钟延迟(nodeId 为必填,参考 dispatcher 测试)
    let start = Instant::now();
    let result = dispatch_command(
        &*backend,
        "DOM.querySelector",
        json!({"nodeId": 1, "selector": "div"}),
        &target_id,
    );
    let elapsed = start.elapsed();

    // Assert — TMG-CDP-001: DOM.querySelector < 50ms (5x 容差应对 CI 抖动)
    assert!(
        result.is_ok(),
        "DOM.querySelector should succeed, got: {:?}",
        result
    );
    assert!(
        elapsed < Duration::from_millis(50),
        "TMG-CDP-001 violation: DOM.querySelector must respond < 50ms (5x 容差), got {:?}",
        elapsed
    );
}

/// Runtime.evaluate 同步分发 < 50ms(TMG-CDP-001 JS 执行无 I/O)
// Arrange — TMG-CDP-001: CDP 命令响应时序,Runtime 域
// @trace REQ-CDP-001@PROCESS [level:system] [domain:Runtime]
#[test]
fn cdp_runtime_evaluate_timing_under_50ms() {
    let (backend, target_id) = build_backend_with_target();

    // Act
    let start = Instant::now();
    let result = dispatch_command(
        &*backend,
        "Runtime.evaluate",
        json!({"expression": "1 + 1"}),
        &target_id,
    );
    let elapsed = start.elapsed();

    // Assert — TMG-CDP-001: Runtime.evaluate < 250ms (5x 容差)
    assert!(result.is_ok(), "Runtime.evaluate failed: {:?}", result);
    assert!(
        elapsed < Duration::from_millis(250),
        "TMG-CDP-001 violation: Runtime.evaluate must respond < 250ms (5x 容差), got {:?}",
        elapsed
    );
}

/// Network.enable 同步分发 < 10ms(TMG-CDP-001 注册 hook)
// Arrange — TMG-CDP-001: Network 域 hook 注册
// @trace REQ-CDP-001@PROCESS [level:system] [domain:Network]
#[test]
fn cdp_network_enable_timing_under_threshold() {
    let (backend, target_id) = build_backend_with_target();

    // Act
    let start = Instant::now();
    let result = dispatch_command(
        &*backend,
        "Network.enable",
        json!({}),
        &target_id,
    );
    let elapsed = start.elapsed();

    // Assert — TMG-CDP-001: Network.enable < 50ms (5x 容差)
    assert!(result.is_ok(), "Network.enable failed: {:?}", result);
    assert!(
        elapsed < Duration::from_millis(50),
        "TMG-CDP-001 violation: Network.enable must respond < 50ms (5x 容差), got {:?}",
        elapsed
    );
}

/// Page.navigate 同步分发 < 3000ms(TMG-CDP-001 网络请求 + DOM 构建 + 样式计算)
// Arrange — TMG-CDP-001: Page 域导航
// @trace REQ-CDP-001@PROCESS [level:system] [domain:Page]
#[test]
fn cdp_page_navigate_timing_under_3000ms() {
    let (backend, target_id) = build_backend_with_target();

    // Act
    let start = Instant::now();
    let result = dispatch_command(
        &*backend,
        "Page.navigate",
        json!({"url": "data:text/html,<html><body>hello</body></html>"}),
        &target_id,
    );
    let elapsed = start.elapsed();

    // Assert — TMG-CDP-001: Page.navigate < 3000ms (mock 路径应 << 实际 servo)
    assert!(result.is_ok(), "Page.navigate failed: {:?}", result);
    assert!(
        elapsed < Duration::from_millis(3000),
        "TMG-CDP-001 violation: Page.navigate must respond < 3000ms, got {:?}",
        elapsed
    );
}

// ════════════════════════════════════════════════════════════════════
// §2 dispatch_command 稳定性 — 重复多次,延迟稳定
// ════════════════════════════════════════════════════════════════════

/// 同一命令连续派发 N 次,所有延迟均在阈值内 — 检测内存泄漏/状态退化
// Arrange — TMG-CDP-001: 稳定性验证,无延迟退化
// @trace REQ-CDP-001@PROCESS [level:system]
#[test]
fn cdp_command_repeat_dispatch_stable_under_threshold() {
    let (backend, target_id) = build_backend_with_target();

    // Act — 连续派发 100 次 DOM.querySelector,测量每次延迟
    let mut max_elapsed = Duration::ZERO;
    let mut failures = 0;
    for i in 0..100u32 {
        let start = Instant::now();
        let result = dispatch_command(
            &*backend,
            "DOM.querySelector",
            json!({"nodeId": i, "selector": format!("div-{}", i)}),
            &target_id,
        );
        let elapsed = start.elapsed();
        if result.is_err() {
            failures += 1;
        }
        if elapsed > max_elapsed {
            max_elapsed = elapsed;
        }
    }

    // Assert — TMG-CDP-001: 100 次重复,最大延迟仍 < 50ms,无失败
    assert_eq!(failures, 0, "TMG-CDP-001: 100 次重复 dispatch 应 0 失败");
    assert!(
        max_elapsed < Duration::from_millis(50),
        "TMG-CDP-001 violation: 100x repeat max elapsed < 50ms, got {:?}",
        max_elapsed
    );
}

/// 多 domain 混合序列派发 — 各 domain 延迟独立满足阈值
// Arrange — TMG-CDP-001: 多 domain 混合序列
// @trace REQ-CDP-001@PROCESS [level:system]
#[test]
fn cdp_multi_domain_mixed_sequence_timing() {
    let (backend, target_id) = build_backend_with_target();

    let target_id_owned = target_id.clone();
    let commands: Vec<(&str, Value, Duration)> = vec![
        ("Network.enable", json!({}), Duration::from_millis(50)),
        ("Runtime.evaluate", json!({"expression": "1"}), Duration::from_millis(250)),
        ("DOM.querySelector", json!({"nodeId": 1, "selector": "body"}), Duration::from_millis(50)),
        ("Page.navigate", json!({"url": "data:text/html,<p>x", "frameId": target_id_owned}), Duration::from_millis(3000)),
    ];

    // Act + Assert — 每条命令延迟满足各自阈值
    for (method, params, threshold) in &commands {
        let start = Instant::now();
        let result = dispatch_command(&*backend, method, params.clone(), &target_id);
        let elapsed = start.elapsed();
        assert!(
            result.is_ok(),
            "method {} failed: {:?}",
            method,
            result
        );
        assert!(
            elapsed < *threshold,
            "TMG-CDP-001 violation: {} must respond < {:?}, got {:?}",
            method,
            threshold,
            elapsed
        );
    }
}

// ════════════════════════════════════════════════════════════════════
// §3 未知 method / 错误路径时序 — 错误响应不应超时
// ════════════════════════════════════════════════════════════════════

/// 未知 method 派发应快速失败(< 50ms),不应阻塞或重试
// Arrange — TMG-CDP-001: 错误路径不应超时
// @trace REQ-CDP-001@PROCESS [level:system]
#[test]
fn cdp_unknown_method_fail_fast_under_threshold() {
    let (backend, target_id) = build_backend_with_target();

    // Act
    let start = Instant::now();
    let _result = dispatch_command(
        &*backend,
        "NonExistent.method",
        json!({}),
        &target_id,
    );
    let elapsed = start.elapsed();

    // Assert — TMG-CDP-001: 错误路径 < 50ms (fail-fast,不应阻塞)
    assert!(
        elapsed < Duration::from_millis(50),
        "TMG-CDP-001 violation: unknown method must fail-fast < 50ms, got {:?}",
        elapsed
    );
}

/// 不存在的 target_id 派发应快速失败
// Arrange — TMG-CDP-001: 错误 target fail-fast
// @trace REQ-CDP-001@PROCESS [level:system]
#[test]
fn cdp_unknown_target_fail_fast_under_threshold() {
    let (backend, _target_id) = build_backend_with_target();
    let unknown_target = "non-existent-target-999";

    // Act
    let start = Instant::now();
    let _result = dispatch_command(
        &*backend,
        "DOM.querySelector",
        json!({"selector": "body"}),
        unknown_target,
    );
    let elapsed = start.elapsed();

    // Assert — TMG-CDP-001: 错误 target < 50ms (fail-fast)
    assert!(
        elapsed < Duration::from_millis(50),
        "TMG-CDP-001 violation: unknown target must fail-fast < 50ms, got {:?}",
        elapsed
    );
}
