//! Dispatcher 边界条件测试 — 未知 method / 无效格式 / 不存在的 domain。
//!
//! @trace REQ-BAO-API-004 [level:integration]
//! @trace REQ-BAO-API-005 [level:integration]
//! @trace REQ-BAO-API-007 [level:integration]

use bao_cdp_client::bridge::{BridgeError, MockServoBackend, ServoBackend};
use bao_cdp_client::dispatch_command;
use serde_json::{json, Value};
use std::sync::Arc;

fn backend() -> Arc<dyn ServoBackend> {
    Arc::new(MockServoBackend::new())
}

#[test]
fn invalid_method_no_dot_returns_invalid_method_error() {
    // Arrange
    // @trace REQ-BAO-API-004 [level:integration]
    let b = backend();
    let method = "noDotHere";

    // Act
    let err = dispatch_command(&*b, method, json!({}), "1").unwrap_err();

    // Assert — InvalidMethod → -32602 invalid params
    assert!(matches!(err, BridgeError::InvalidMethod(_)));
    assert_eq!(err.cdp_error_code(), -32602);
}

#[test]
fn unknown_domain_returns_method_not_found() {
    // Arrange
    // @trace REQ-BAO-API-004 [level:integration]
    let b = backend();

    // Act
    let err = dispatch_command(&*b, "UnknownDomain.foo", json!({}), "1").unwrap_err();

    // Assert
    assert!(matches!(err, BridgeError::MethodNotFound(_)));
    assert_eq!(err.cdp_error_code(), -32601);
}

#[test]
fn unknown_method_in_known_domain_returns_method_not_found() {
    // Arrange
    // @trace REQ-BAO-API-004 [level:integration]
    let b = backend();

    // Act
    let err = dispatch_command(&*b, "Page.totallyBogus", json!({}), "1").unwrap_err();

    // Assert
    assert!(matches!(err, BridgeError::MethodNotFound(_)));
}

#[test]
fn unknown_method_in_runtime_returns_method_not_found() {
    // Arrange
    // @trace REQ-BAO-API-004 [level:integration]
    let b = backend();

    // Act
    let err = dispatch_command(&*b, "Runtime.totallyBogus", json!({}), "1").unwrap_err();

    // Assert
    assert!(matches!(err, BridgeError::MethodNotFound(_)));
}

#[test]
fn b_class_routes_to_eval_synthesizer() {
    // Arrange — TASK-3b: B 类 method 通过 IIFE Eval 合成
    // @trace REQ-BAO-API-005 [level:integration]
    let b = backend();

    // Act — Mock backend echo:返回 evaluate expression 字符串
    let r = dispatch_command(&*b, "Page.title", json!({}), "1").unwrap();
    let v = r["result"]["value"].as_str().unwrap();

    // Assert
    assert!(v.contains("(function(){"));
    assert!(v.contains("return document.title;"));
    assert!(v.ends_with("})()"));
}

#[test]
fn empty_method_returns_invalid_method() {
    // Arrange
    // @trace REQ-BAO-API-004 [level:integration]
    let b = backend();

    // Act
    let err = dispatch_command(&*b, "", json!({}), "1").unwrap_err();

    // Assert
    assert!(matches!(err, BridgeError::InvalidMethod(_)));
}

#[test]
fn method_with_only_dot_returns_method_not_found() {
    // Arrange — "." splits into ("", ""), empty domain → not in A/E class
    // @trace REQ-BAO-API-004 [level:integration]
    let b = backend();

    // Act
    let err = dispatch_command(&*b, ".", json!({}), "1").unwrap_err();

    // Assert
    assert!(matches!(err, BridgeError::MethodNotFound(_)));
}

#[test]
fn unknown_target_id_returns_page_not_found() {
    // Arrange
    // @trace REQ-BAO-API-004 [level:integration]
    let b = backend();

    // Act
    let err =
        dispatch_command(&*b, "Page.navigate", json!({"url":"https://x"}), "999").unwrap_err();

    // Assert
    assert!(matches!(err, BridgeError::PageNotFound(_)));
    assert_eq!(err.cdp_error_code(), -32000);
}

#[test]
fn valid_dispatch_returns_value_not_error() {
    // Arrange — Sanity: ensure dispatch returns Ok for valid command
    // @trace REQ-BAO-API-004 [level:integration]
    let b = backend();

    // Act
    let r = dispatch_command(&*b, "Page.navigate", json!({"url":"https://x"}), "1").unwrap();

    // Assert
    assert!(r.is_object());
    assert!(r["frameId"].is_string());
}

#[test]
fn default_session_used_when_session_id_is_none() {
    // Arrange — CDPRdpBridge uses "default" when session_id is None
    // @trace REQ-BAO-API-004 [level:integration]
    use bao_cdp_client::transport::in_memory::{InMemoryBridge, InMemoryBridgeResponse};
    use bao_cdp_client::CDPRdpBridge;
    let b: Arc<dyn ServoBackend> = Arc::new(MockServoBackend::new());
    let bridge = CDPRdpBridge::new(b);
    let bridge_dyn: Arc<dyn InMemoryBridge> = bridge.into_in_memory_bridge();

    // Act
    let r = bridge_dyn.dispatch_command("Page.navigate", json!({"url":"x"}), None);

    // Assert
    match r {
        InMemoryBridgeResponse::Ok(v) => assert_eq!(v["frameId"], "FRAME_0"),
        InMemoryBridgeResponse::Err(e) => panic!("expected Ok, got Err: {e}"),
    }
}

#[test]
fn cdp_error_response_payload_carries_code_and_message() {
    // Arrange — E-class error serialized as JSON payload `{code, message}`
    // @trace REQ-BAO-API-007 [level:integration]
    use bao_cdp_client::transport::in_memory::{InMemoryBridge, InMemoryBridgeResponse};
    use bao_cdp_client::CDPRdpBridge;
    let b: Arc<dyn ServoBackend> = Arc::new(MockServoBackend::new());
    let bridge = CDPRdpBridge::new(b);
    let bridge_dyn: Arc<dyn InMemoryBridge> = bridge.into_in_memory_bridge();

    // Act
    let r = bridge_dyn.dispatch_command("HeapProfiler.takeHeapSnapshot", json!({}), Some("1"));

    // Assert
    match r {
        InMemoryBridgeResponse::Err(s) => {
            let v: Value = serde_json::from_str(&s).unwrap();
            assert_eq!(v["code"], -32601);
            assert!(v["message"].as_str().unwrap().contains("HeapProfiler"));
        }
        _ => panic!("expected Err"),
    }
}

#[test]
fn cdp_error_response_for_method_not_found_carries_correct_code() {
    // Arrange
    // @trace REQ-BAO-API-004 [level:integration]
    use bao_cdp_client::transport::in_memory::{InMemoryBridge, InMemoryBridgeResponse};
    use bao_cdp_client::CDPRdpBridge;
    let b: Arc<dyn ServoBackend> = Arc::new(MockServoBackend::new());
    let bridge = CDPRdpBridge::new(b);
    let bridge_dyn: Arc<dyn InMemoryBridge> = bridge.into_in_memory_bridge();

    // Act
    let r = bridge_dyn.dispatch_command("Foo.bar", json!({}), Some("1"));

    // Assert
    match r {
        InMemoryBridgeResponse::Err(s) => {
            let v: Value = serde_json::from_str(&s).unwrap();
            assert_eq!(v["code"], -32601);
            assert!(v["message"].as_str().unwrap().contains("Foo.bar"));
        }
        _ => panic!("expected Err"),
    }
}

#[test]
fn cdp_error_response_for_invalid_params_carries_32602() {
    // Arrange — Missing required "url" parameter
    // @trace REQ-BAO-API-004 [level:integration]
    use bao_cdp_client::transport::in_memory::{InMemoryBridge, InMemoryBridgeResponse};
    use bao_cdp_client::CDPRdpBridge;
    let b: Arc<dyn ServoBackend> = Arc::new(MockServoBackend::new());
    let bridge = CDPRdpBridge::new(b);
    let bridge_dyn: Arc<dyn InMemoryBridge> = bridge.into_in_memory_bridge();

    // Act
    let r = bridge_dyn.dispatch_command("Page.navigate", json!({}), Some("1"));

    // Assert
    match r {
        InMemoryBridgeResponse::Err(s) => {
            let v: Value = serde_json::from_str(&s).unwrap();
            assert_eq!(v["code"], -32602);
            assert!(v["message"].as_str().unwrap().contains("url"));
        }
        _ => panic!("expected Err"),
    }
}

// ════════════════════════════════════════════════════════════════════
// 补充测试 — 覆盖对抗验证缺口
// ════════════════════════════════════════════════════════════════════
//
// 对抗验证发现以下三类缺口,本节逐一补全:
//
// [正确性] REQ-BAO-API-004 C2(无 Eval 注入)/ REQ-BAO-API-005 C1/C3/C4
//          (52-method 完备性 + 序列化 + 每 method 注入)/ REQ-BAO-API-007 C1
// [边界]   并发 / 超时 / 极端值 / 错误状态变体 / target_id 边界 / 空值
// [对齐]   reqRef criteria 断言完整性

// ─── 错误状态变体:8 个 BridgeError 变体的 cdp_error_code 隔离测试 ───

#[test]
fn invalid_target_id_variant_maps_to_server_error_32000() {
    // Arrange — InvalidTargetId(非数字 target 'abc')→ -32000 server error
    // 当前 dispatcher 不解析 target_id 数值,但错误变体映射须隔离断言。
    // @trace REQ-BAO-API-004 [level:integration]
    let err = BridgeError::InvalidTargetId("abc".to_string());

    // Assert
    assert_eq!(err.cdp_error_code(), -32000);
    assert!(err.message().contains("invalid target id"));
    assert!(err.message().contains("abc"));
}

#[test]
fn servo_error_variant_maps_to_server_error_32000() {
    // Arrange — ServoError(backend 故障)→ -32000 server error
    // @trace REQ-BAO-API-004 [level:integration]
    let err = BridgeError::ServoError("navigate failed: net error".to_string());

    // Assert
    assert_eq!(err.cdp_error_code(), -32000);
    assert!(err.message().contains("servo error"));
    assert!(err.message().contains("navigate failed"));
}

#[test]
fn not_implemented_yet_variant_maps_to_server_error_32000() {
    // Arrange — NotImplementedYet(B 类占位)→ -32000 server error
    // @trace REQ-BAO-API-005 [level:integration]
    let err = BridgeError::NotImplementedYet("Runtime.compileScript".to_string());

    // Assert
    assert_eq!(err.cdp_error_code(), -32000);
    assert!(err.message().contains("not implemented yet"));
}

#[test]
fn invalid_params_variant_maps_to_invalid_params_32602() {
    // Arrange — InvalidParams(参数缺失/类型错)→ -32602
    // @trace REQ-BAO-API-004 [level:integration]
    let err = BridgeError::InvalidParams("missing url".to_string());

    // Assert
    assert_eq!(err.cdp_error_code(), -32602);
    assert!(err.message().contains("invalid params"));
}

#[test]
fn not_supported_variant_maps_to_method_not_found_32601() {
    // Arrange — NotSupported(E 类)→ -32601,与 MethodNotFound 同码
    // @trace REQ-BAO-API-007 [level:integration]
    let err = BridgeError::NotSupported("HeapProfiler.takeHeapSnapshot".to_string());

    // Assert
    assert_eq!(err.cdp_error_code(), -32601);
    assert!(err.message().contains("not supported"));
}

#[test]
fn all_eight_bridge_error_variants_covered_by_code_mapping() {
    // 完备性守卫:8 个变体全部 cdp_error_code 断言,防止新增变体漏测。
    // @trace REQ-BAO-API-007 [level:integration]
    let cases = [
        (BridgeError::InvalidMethod("x".into()), -32602),
        (BridgeError::MethodNotFound("x".into()), -32601),
        (BridgeError::NotSupported("x".into()), -32601),
        (BridgeError::NotImplementedYet("x".into()), -32000),
        (BridgeError::InvalidTargetId("x".into()), -32000),
        (BridgeError::PageNotFound("x".into()), -32000),
        (BridgeError::ServoError("x".into()), -32000),
        (BridgeError::InvalidParams("x".into()), -32602),
    ];
    for (err, expected) in cases {
        assert_eq!(
            err.cdp_error_code(),
            expected,
            "variant {:?} should map to {expected}",
            err
        );
    }
}

// ─── E 类 vs MethodNotFound 区分测试 ───

#[test]
fn e_class_domain_returns_not_supported_not_method_not_found() {
    // Arrange — HeapProfiler.* 整 domain 是 E 类,返回 NotSupported(非 MethodNotFound)
    // 区分:E 类是"servo 明确不支持",MethodNotFound 是"完全未知 method"。
    // @trace REQ-BAO-API-007 [level:integration]
    let b = backend();

    // Act
    let err = dispatch_command(&*b, "HeapProfiler.collectGarbage", json!({}), "1").unwrap_err();

    // Assert
    assert!(matches!(err, BridgeError::NotSupported(_)));
    assert_eq!(err.cdp_error_code(), -32601);
}

#[test]
fn e_class_method_in_known_domain_returns_not_supported() {
    // Arrange — Page.printToPDF 在已知 domain Page 下,但是 E 类
    // 必须返回 NotSupported(-32601),而非 MethodNotFound。
    // @trace REQ-BAO-API-007 [level:integration]
    let b = backend();

    // Act
    let err = dispatch_command(&*b, "Page.printToPDF", json!({}), "1").unwrap_err();

    // Assert
    assert!(matches!(err, BridgeError::NotSupported(_)));
    assert_eq!(err.cdp_error_code(), -32601);
}

#[test]
fn totally_unknown_domain_returns_method_not_found_not_not_supported() {
    // Arrange — UnknownDomain.foo 不在 E_CLASS_DOMAINS,必须 MethodNotFound
    // (与 E 类 NotSupported 区分)
    // @trace REQ-BAO-API-004 [level:integration]
    let b = backend();

    // Act
    let err = dispatch_command(&*b, "Foo.bar", json!({}), "1").unwrap_err();

    // Assert
    assert!(matches!(err, BridgeError::MethodNotFound(_)));
    assert!(!matches!(err, BridgeError::NotSupported(_)));
}

// ─── target_id 边界 ───

#[test]
fn target_id_zero_is_valid_target() {
    // Arrange — '0' 是合法 usize target_id(已知 target 列表未含 → PageNotFound)
    // 这验证 target_id 边界 '0' 不触发 InvalidMethod / InvalidTargetId,
    // 而是正常路由到 backend.ensure_target。
    // @trace REQ-BAO-API-004 [level:integration]
    let b = backend();

    // Act — '0' 不在 mock known_targets ["1","default"]
    let err = dispatch_command(&*b, "Page.navigate", json!({"url":"https://x"}), "0").unwrap_err();

    // Assert — 路由到 backend,返回 PageNotFound(而非 InvalidMethod)
    assert!(matches!(err, BridgeError::PageNotFound(_)));
    assert_eq!(err.cdp_error_code(), -32000);
}

#[test]
fn target_id_default_is_known() {
    // Arrange — MockServoBackend 默认 known_targets 含 "default"
    // @trace REQ-BAO-API-004 [level:integration]
    let b = backend();

    // Act
    let r = dispatch_command(&*b, "Page.navigate", json!({"url":"https://x"}), "default").unwrap();

    // Assert — 正常路由成功
    assert!(r["frameId"].is_string());
}

#[test]
fn target_id_non_numeric_string_routes_to_page_not_found() {
    // Arrange — 非数字 target 'abc' 路由到 backend,不在 known_targets → PageNotFound
    // 注意:当前 dispatcher 不解析 target_id 数值,故 'abc' 不会触发 InvalidTargetId,
    // 而是经 backend.ensure_target 返回 PageNotFound。
    // @trace REQ-BAO-API-004 [level:integration]
    let b = backend();

    // Act
    let err =
        dispatch_command(&*b, "Page.navigate", json!({"url":"https://x"}), "abc").unwrap_err();

    // Assert
    assert!(matches!(err, BridgeError::PageNotFound(_)));
    assert_eq!(err.cdp_error_code(), -32000);
}

#[test]
fn target_id_oversize_usize_string_routes_to_page_not_found() {
    // Arrange — 超大数字 '18446744073709551616'(u64::MAX+1)
    // 不在 known_targets → PageNotFound,不 panic / overflow
    // @trace REQ-BAO-API-004 [level:integration]
    let b = backend();

    // Act
    let err = dispatch_command(
        &*b,
        "Page.navigate",
        json!({"url":"https://x"}),
        "18446744073709551616",
    )
    .unwrap_err();

    // Assert
    assert!(matches!(err, BridgeError::PageNotFound(_)));
}

#[test]
fn target_id_negative_string_routes_to_page_not_found() {
    // Arrange — 负数 '-1' 不在 known_targets → PageNotFound
    // @trace REQ-BAO-API-004 [level:integration]
    let b = backend();

    // Act
    let err = dispatch_command(&*b, "Page.navigate", json!({"url":"https://x"}), "-1").unwrap_err();

    // Assert
    assert!(matches!(err, BridgeError::PageNotFound(_)));
}

// ─── 空值 / null / 畸形 params ───

#[test]
fn params_null_routes_successfully_when_no_required_fields() {
    // Arrange — params = Value::Null,method 无必填字段(Runtime.enable)
    // @trace REQ-BAO-API-004 [level:integration]
    let b = backend();

    // Act
    let r = dispatch_command(&*b, "Runtime.enable", Value::Null, "1").unwrap();

    // Assert — Runtime.enable 不读 params,Null 也 OK
    assert!(r.is_object());
}

#[test]
fn params_null_for_required_url_field_returns_invalid_params() {
    // Arrange — params = Value::Null,Page.navigate 需要 url → InvalidParams
    // @trace REQ-BAO-API-004 [level:integration]
    let b = backend();

    // Act
    let err = dispatch_command(&*b, "Page.navigate", Value::Null, "1").unwrap_err();

    // Assert
    assert!(matches!(err, BridgeError::InvalidParams(_)));
    assert_eq!(err.cdp_error_code(), -32602);
}

#[test]
fn params_array_for_object_expected_field_returns_invalid_params() {
    // Arrange — params 是数组(非对象),Page.navigate 期望对象含 url
    // @trace REQ-BAO-API-004 [level:integration]
    let b = backend();

    // Act
    let err = dispatch_command(&*b, "Page.navigate", json!(["https://x"]), "1").unwrap_err();

    // Assert — get_str 在 array 上 .get("url") 返回 None → InvalidParams
    assert!(matches!(err, BridgeError::InvalidParams(_)));
}

#[test]
fn params_with_null_field_value_returns_invalid_params() {
    // Arrange — params = {"url": null},url 字段存在但值非 string
    // @trace REQ-BAO-API-004 [level:integration]
    let b = backend();

    // Act
    let err = dispatch_command(&*b, "Page.navigate", json!({"url": null}), "1").unwrap_err();

    // Assert — as_str() on Null → None → InvalidParams
    assert!(matches!(err, BridgeError::InvalidParams(_)));
}

#[test]
fn params_with_wrong_type_field_value_returns_invalid_params() {
    // Arrange — params = {"url": 123},url 字段是数字非 string
    // @trace REQ-BAO-API-004 [level:integration]
    let b = backend();

    // Act
    let err = dispatch_command(&*b, "Page.navigate", json!({"url": 123}), "1").unwrap_err();

    // Assert
    assert!(matches!(err, BridgeError::InvalidParams(_)));
}

#[test]
fn params_deeply_nested_does_not_panic() {
    // Arrange — 极端值:深度嵌套 JSON params,验证不 overflow / panic
    // @trace REQ-BAO-API-004 [level:integration]
    let b = backend();
    let mut nested = json!({"url":"https://x"});
    for _ in 0..50 {
        nested = json!({"deep": nested, "url": "https://x"});
    }

    // Act — 嵌套字段不影响 Page.navigate(只取顶层 url)
    let r = dispatch_command(&*b, "Page.navigate", nested, "1").unwrap();

    // Assert
    assert!(r["frameId"].is_string());
}

#[test]
fn params_large_string_does_not_panic() {
    // Arrange — 极端值:超大 string 字段值(1MB url)
    // @trace REQ-BAO-API-004 [level:integration]
    let b = backend();
    let big_url = "x".repeat(1_048_576);

    // Act
    let r = dispatch_command(&*b, "Page.navigate", json!({"url": big_url}), "1").unwrap();

    // Assert — 路由成功(mock 不验证 URL 长度)
    assert!(r["frameId"].is_string());
}

// ─── 并发:Arc<dyn ServoBackend> 线程安全 ───

#[test]
fn concurrent_dispatch_is_thread_safe() {
    // Arrange — bao 是单进程多线程架构,验证 Arc<dyn ServoBackend>
    // 可被多线程并发 dispatch 无竞态 / 无 panic。
    // @trace REQ-BAO-API-004 [level:integration]
    use std::sync::Barrier;
    use std::thread;

    // 使用具体 MockServoBackend(非 trait object)以便读取 call_log,
    // 同时通过 Arc<dyn ServoBackend> clone 验证 trait object 线程安全。
    let mock = Arc::new(MockServoBackend::new());
    let b: Arc<dyn ServoBackend> = mock.clone();
    let barrier = Arc::new(Barrier::new(8));
    let mut handles = vec![];

    // Act — 8 线程并发 dispatch 不同 method
    for i in 0..8u32 {
        let b_clone = Arc::clone(&b);
        let barrier_clone = Arc::clone(&barrier);
        let method = match i % 4 {
            0 => "Page.navigate",
            1 => "Runtime.enable",
            2 => "DOM.getDocument",
            _ => "Page.reload",
        };
        handles.push(thread::spawn(move || {
            barrier_clone.wait();
            // 100 次连续 dispatch
            for _ in 0..100 {
                let result =
                    dispatch_command(&*b_clone, method, json!({"url":"https://x","depth":1}), "1");
                // 全部必须 Ok(无竞态 panic / 无 corrupted state)
                assert!(result.is_ok(), "thread {i} dispatch {method} failed");
            }
        }));
    }

    // Assert — 所有线程完成无 panic
    for h in handles {
        h.join().expect("thread panicked");
    }

    // 验证 call_log 不被并发写损坏(Mutex 保护下应连续递增)
    let total = mock.call_log.lock().unwrap().len();
    assert_eq!(
        total, 800,
        "expected 800 (8 threads × 100 calls) call_log entries"
    );
}

#[test]
fn concurrent_dispatch_mixed_ok_and_error_thread_safe() {
    // Arrange — 并发混合成功/失败路径(部分 target 未知 → PageNotFound),
    // 验证 Mutex 保护下错误路径也不竞态。
    // @trace REQ-BAO-API-004 [level:integration]
    use std::thread;

    let b: Arc<dyn ServoBackend> = Arc::new(MockServoBackend::new());
    let mut handles = vec![];

    for i in 0..4u32 {
        let b_clone = Arc::clone(&b);
        handles.push(thread::spawn(move || {
            let target = if i % 2 == 0 { "1" } else { "999" };
            for _ in 0..50 {
                let result = dispatch_command(
                    &*b_clone,
                    "Page.navigate",
                    json!({"url":"https://x"}),
                    target,
                );
                if target == "1" {
                    assert!(result.is_ok());
                } else {
                    assert!(matches!(result.unwrap_err(), BridgeError::PageNotFound(_)));
                }
            }
        }));
    }

    for h in handles {
        h.join().expect("thread panicked");
    }
}

// ─── A 类无 Eval 注入(REQ-BAO-API-004 C2)───

#[test]
fn a_class_page_navigate_response_has_no_iife_eval_pattern() {
    // Arrange — REQ-BAO-API-004 C2 '1:1 参数/结果映射,无 Eval 注入'
    // A 类 Page.navigate 直调 servo API,响应是 {frameId, loaderId},
    // 绝不含 B 类 IIFE 模式 "(function(){"。
    // @trace REQ-BAO-API-004 [level:integration]
    let b = backend();

    // Act
    let r = dispatch_command(&*b, "Page.navigate", json!({"url":"https://x"}), "1").unwrap();
    let serialized = serde_json::to_string(&r).unwrap();

    // Assert — A 类响应是机械映射,绝不包含 Eval/IIFE 合成痕迹
    assert!(!serialized.contains("(function(){"));
    assert!(!serialized.contains("__args"));
    assert!(!serialized.contains("return document"));
    assert!(!serialized.contains("eval"));
    // 响应结构是 1:1 servo API 映射(frameId + loaderId)
    assert!(r["frameId"].is_string());
    assert!(r["loaderId"].is_string());
}

#[test]
fn a_class_runtime_evaluate_response_not_iife_synthesized() {
    // Arrange — Runtime.evaluate 是 A 类(直调 servo runtime_evaluate),
    // 用户传入的 expression 经 servo 真实求值,响应 value = expression echo。
    // 与 B 类合成(返回 IIFE 源码字符串)区分。
    // @trace REQ-BAO-API-004 [level:integration]
    let b = backend();

    // Act
    let r = dispatch_command(&*b, "Runtime.evaluate", json!({"expression":"1+1"}), "1").unwrap();

    // Assert — A 类 Runtime.evaluate 响应结构
    assert!(r["result"].is_object());
    assert_eq!(r["result"]["type"], "string");
    // value 是用户 expression 经 servo 求值(mock echo),不是 B 类 IIFE 包装
    let value = r["result"]["value"].as_str().unwrap();
    assert_eq!(value, "1+1");
    assert!(!value.contains("(function(){"));
}

#[test]
fn a_class_dom_query_selector_response_shape() {
    // Arrange — A 类 DOM.querySelector 1:1 映射,响应 {nodeId}
    // @trace REQ-BAO-API-004 [level:integration]
    let b = backend();

    // Act
    let r = dispatch_command(
        &*b,
        "DOM.querySelector",
        json!({"nodeId":1,"selector":"div"}),
        "1",
    )
    .unwrap();

    // Assert — 机械映射,无 Eval
    assert_eq!(r["nodeId"], 2);
    let serialized = serde_json::to_string(&r).unwrap();
    assert!(!serialized.contains("(function(){"));
}

#[test]
fn a_class_target_get_targets_returns_array() {
    // Arrange — A 类 Target.getTargets 1:1 映射,响应 {targetInfos: [...]}
    // @trace REQ-BAO-API-004 [level:integration]
    let b = backend();

    // Act
    let r = dispatch_command(&*b, "Target.getTargets", json!({}), "1").unwrap();

    // Assert
    assert!(r["targetInfos"].is_array());
    let serialized = serde_json::to_string(&r).unwrap();
    assert!(!serialized.contains("(function(){"));
}

// ─── B 类 52-method 完备性断言(REQ-BAO-API-005 C1)───

#[test]
fn b_class_all_52_methods_route_to_handler_not_method_not_found() {
    // Arrange — REQ-BAO-API-005 C1 '52 method 全部实现'
    // 验证 dispatcher 中所有 B 类 method 都被路由(不被 MethodNotFound 拦截)。
    // 本测试直接断言 52 个 B 类 method 在 dispatcher 中有匹配 arm。
    // @trace REQ-BAO-API-005 [level:integration]
    let b = backend();

    // 全部 52 个 B 类 method(按 b_class_handlers.rs 分类)
    let b_class_methods: &[&str] = &[
        // Page domain (33)
        "Page.title",
        "Page.url",
        "Page.content",
        "Page.viewport",
        "Page.setViewport",
        "Page.opener",
        "Page.frames",
        "Page.mainFrame",
        "Page.setDefaultNavigationTimeout",
        "Page.setDefaultTimeout",
        "Page.waitForLoadState",
        "Page.waitForURL",
        "Page.waitForRequest",
        "Page.waitForResponse",
        "Page.waitForEvent",
        "Page.goBack",
        "Page.goForward",
        "Page.emulateMedia",
        "Page.addScriptTag",
        "Page.addStyleTag",
        "Page.exposeFunction",
        "Page.pdf",
        "Page.screenshot",
        "Page.tap",
        "Page.hover",
        "Page.focus",
        "Page.type",
        "Page.fill",
        "Page.press",
        "Page.check",
        "Page.uncheck",
        "Page.selectOption",
        "Page.setInputFiles",
        "Page.requestGC",
        // ElementHandle domain (16)
        "ElementHandle.click",
        "ElementHandle.contentFrame",
        "ElementHandle.ownerFrame",
        "ElementHandle.getAttribute",
        "ElementHandle.innerHTML",
        "ElementHandle.innerText",
        "ElementHandle.textContent",
        "ElementHandle.isChecked",
        "ElementHandle.isDisabled",
        "ElementHandle.isEditable",
        "ElementHandle.isEnabled",
        "ElementHandle.isHidden",
        "ElementHandle.isVisible",
        "ElementHandle.scrollIntoViewIfNeeded",
        "ElementHandle.waitForElementState",
        "ElementHandle.waitForSelector",
        // JSHandle domain (7)
        "JSHandle.asElement",
        "JSHandle.dispose",
        "JSHandle.evaluate",
        "JSHandle.evaluateHandle",
        "JSHandle.getProperties",
        "JSHandle.getProperty",
        "JSHandle.jsonValue",
    ];

    let mut count_routed = 0;
    for method in b_class_methods {
        // 每个 method 都必须能路由(Ok),不能 MethodNotFound
        let result = dispatch_command(&*b, method, json!({}), "1");
        if result.is_ok() {
            count_routed += 1;
        } else if let Err(BridgeError::MethodNotFound(_)) = result {
            panic!("B 类 method {method} 未被路由(MethodNotFound)— dispatcher 缺失 arm");
        }
        // 其他错误(InvalidParams / PageNotFound)也算路由成功(arm 存在)
        else {
            count_routed += 1;
        }
    }

    // Assert — 至少 52 个全部路由成功
    assert!(
        count_routed >= 52,
        "expected >= 52 B-class methods routed, got {count_routed}"
    );
}

// ─── B 类 JSON.stringify 参数化(REQ-BAO-API-005 C3)───

#[test]
fn b_class_with_args_uses_json_stringify_not_string_concat() {
    // Arrange — REQ-BAO-API-005 C3 'JSON.stringify 参数化,禁止字符串拼接'
    // ElementHandle.click 带 selector 参数 → build_iife_with_args,
    // 参数经 JSON.stringify 序列化为 __args[i],body 仅引用变量。
    // @trace REQ-BAO-API-005 [level:integration]
    let b = backend();

    // Act — 带 selector 参数触发序列化路径
    let r = dispatch_command(
        &*b,
        "ElementHandle.click",
        json!({"selector":"button.primary"}),
        "1",
    )
    .unwrap();
    let v = r["result"]["value"].as_str().unwrap();

    // Assert — 参数以 JSON 字面量出现在 __args 数组(经 JSON.stringify)
    assert!(v.contains("var __args="));
    // selector 值作为 JSON 字符串字面量(带引号)出现在 __args
    assert!(v.contains("\"button.primary\""));
    // body 引用 __args[0],不直接拼接 selector 字面量作为代码
    assert!(v.contains("__args[0]"));
}

#[test]
fn b_class_args_with_injection_payload_is_neutralized() {
    // Arrange — REQ-BAO-API-005 C3 'JSON.stringify 参数化'的注入防御验证
    // 注入载荷 ');alert(1);// 经 JSON.stringify 转义,不逃逸出字符串字面量。
    // Page.fill(selector, value) 是带两个参数的序列化路径。
    // @trace REQ-BAO-API-005 [level:integration]
    let b = backend();
    let payload = "');alert(1);//";

    // Act
    let r = dispatch_command(
        &*b,
        "Page.fill",
        json!({"selector":"input","value": payload}),
        "1",
    )
    .unwrap();
    let v = r["result"]["value"].as_str().unwrap();

    // Assert — payload 作为 JSON 字符串字面量出现(引号包裹)
    // 不会作为代码执行(body 内只引用 __args[i])
    assert!(v.contains("var __args="));
    // payload 必须以 JSON-escaped 字符串字面量出现(带引号包裹)
    assert!(v.contains("\"');alert(1);//\""));
    // body 内不应出现 payload 作为裸可执行代码
    let body_marker = "return (function(){";
    if let Some(body_start) = v.find(body_marker) {
        let body = &v[body_start..];
        assert!(
            !body.contains(&format!("return {payload}")),
            "injection payload leaked into body as code"
        );
    }
}

// ─── B 类 IIFE 封装(REQ-BAO-API-005 C2)— 完备性扩展 ───

#[test]
fn b_class_page_url_uses_iife_wrapper() {
    // Arrange — REQ-BAO-API-005 C2 '所有 Eval 使用 IIFE 安全封装'
    // @trace REQ-BAO-API-005 [level:integration]
    let b = backend();

    // Act
    let r = dispatch_command(&*b, "Page.url", json!({}), "1").unwrap();
    let v = r["result"]["value"].as_str().unwrap();

    // Assert
    assert!(v.starts_with("(function(){"));
    assert!(v.ends_with("})()"));
    assert!(v.contains("location.href"));
}

#[test]
fn b_class_page_content_uses_iife_wrapper() {
    // Arrange — REQ-BAO-API-005 C2 '所有 Eval 使用 IIFE 安全封装'
    // @trace REQ-BAO-API-005 [level:integration]
    let b = backend();

    // Act
    let r = dispatch_command(&*b, "Page.content", json!({}), "1").unwrap();
    let v = r["result"]["value"].as_str().unwrap();

    // Assert
    assert!(v.contains("(function(){"));
    assert!(v.contains("outerHTML"));
}

// ─── E 类 Internal 模式 -32601(REQ-BAO-API-007 C1)— 多 domain 完备性 ───

#[test]
fn e_class_all_e_domains_return_32601() {
    // Arrange — REQ-BAO-API-007 C1 'E 类 servo 不支持 31 method 在 Internal 模式返回 -32601'
    // 完备性:验证所有 E_CLASS_DOMAINS 都返回 -32601。
    // @trace REQ-BAO-API-007 [level:integration]
    let b = backend();
    let e_domains = [
        "HeapProfiler",
        "Profiler",
        "DOMStorage",
        "IndexedDB",
        "ServiceWorker",
        "Tracing",
    ];

    for domain in e_domains {
        let method = format!("{domain}.anything");
        let err = dispatch_command(&*b, &method, json!({}), "1").unwrap_err();
        assert!(
            matches!(err, BridgeError::NotSupported(_)),
            "{domain} should be E-class (NotSupported)"
        );
        assert_eq!(
            err.cdp_error_code(),
            -32601,
            "{domain} should map to -32601"
        );
    }
}

#[test]
fn e_class_method_level_returns_32601() {
    // Arrange — E_CLASS_METHODS 精确匹配(Page.printToPDF / Page.startJSCoverage 等)
    // @trace REQ-BAO-API-007 [level:integration]
    let b = backend();
    let e_methods = [
        "Page.printToPDF",
        "Page.startJSCoverage",
        "Page.stopJSCoverage",
        "Page.startCSSCoverage",
        "Page.stopCSSCoverage",
    ];

    for method in e_methods {
        let err = dispatch_command(&*b, method, json!({}), "1").unwrap_err();
        assert!(
            matches!(err, BridgeError::NotSupported(_)),
            "{method} should be E-class method"
        );
    }
}

// ─── cdp_error_response payload 结构完备性 ───

#[test]
fn cdp_error_response_always_carries_both_code_and_message_fields() {
    // Arrange — 错误 payload 结构完备性:{code, message} 两字段缺一不可
    // 覆盖三种错误码路径(-32601 / -32602 / -32000)
    // @trace REQ-BAO-API-007 [level:integration]
    use bao_cdp_client::transport::in_memory::{InMemoryBridge, InMemoryBridgeResponse};
    use bao_cdp_client::CDPRdpBridge;
    let b: Arc<dyn ServoBackend> = Arc::new(MockServoBackend::new());
    let bridge = CDPRdpBridge::new(b);
    let bridge_dyn: Arc<dyn InMemoryBridge> = bridge.into_in_memory_bridge();

    // -32601 E class
    let r = bridge_dyn.dispatch_command("HeapProfiler.takeHeapSnapshot", json!({}), Some("1"));
    match r {
        InMemoryBridgeResponse::Err(s) => {
            let v: Value = serde_json::from_str(&s).unwrap();
            assert!(v["code"].is_i64());
            assert!(v["message"].is_string());
            assert_eq!(v["code"], -32601);
        }
        _ => panic!("E class case: expected Err"),
    }

    // -32602 missing url
    let r = bridge_dyn.dispatch_command("Page.navigate", json!({}), Some("1"));
    match r {
        InMemoryBridgeResponse::Err(s) => {
            let v: Value = serde_json::from_str(&s).unwrap();
            assert!(v["code"].is_i64());
            assert!(v["message"].is_string());
            assert_eq!(v["code"], -32602);
        }
        _ => panic!("missing url case: expected Err"),
    }

    // -32000 PageNotFound(target "999")
    let r = bridge_dyn.dispatch_command("Page.navigate", json!({"url":"https://x"}), Some("999"));
    match r {
        InMemoryBridgeResponse::Err(s) => {
            let v: Value = serde_json::from_str(&s).unwrap();
            assert_eq!(v["code"], -32000);
            assert!(v["message"].is_string());
        }
        _ => panic!("PageNotFound case: expected Err"),
    }
}

// ─── Dispatcher 路由优先级:E 类 > A 类 > B 类 ───

#[test]
fn e_class_check_precedes_a_and_b_class() {
    // Arrange — dispatcher 先检查 E 类,再 match A/B 类。
    // Page.printToPDF 虽在 Page domain(A 类 domain),但本身是 E 类 method,
    // 必须返回 NotSupported 而非路由到 A/B 类 handler。
    // @trace REQ-BAO-API-007 [level:integration]
    let b = backend();

    // Act
    let err = dispatch_command(&*b, "Page.printToPDF", json!({}), "1").unwrap_err();

    // Assert — E 类优先,不进 A/B match
    assert!(matches!(err, BridgeError::NotSupported(_)));
}

// ─── 默认 session 路由(REQ-BAO-API-004)— 边界扩展 ───

#[test]
fn default_session_routes_to_default_target() {
    // Arrange — session_id = None 时,bridge 用 "default" 作 target_id
    // MockServoBackend known_targets 含 "default"
    // @trace REQ-BAO-API-004 [level:integration]
    use bao_cdp_client::transport::in_memory::{InMemoryBridge, InMemoryBridgeResponse};
    use bao_cdp_client::CDPRdpBridge;
    let b: Arc<dyn ServoBackend> = Arc::new(MockServoBackend::new());
    let bridge = CDPRdpBridge::new(b);
    let bridge_dyn: Arc<dyn InMemoryBridge> = bridge.into_in_memory_bridge();

    // Act — 多个 A 类 method 在 default target 上路由
    let methods = [
        ("Page.reload", json!({})),
        ("Runtime.enable", json!({})),
        ("Network.enable", json!({})),
        ("DOM.getDocument", json!({"depth": 1})),
    ];

    for (method, params) in methods {
        let r = bridge_dyn.dispatch_command(method, params, None);
        match r {
            InMemoryBridgeResponse::Ok(_) => { /* ok */ }
            InMemoryBridgeResponse::Err(e) => panic!("{method} on default session failed: {e}"),
        }
    }
}

// ─── BridgeError Display / message 一致性 ───

#[test]
fn bridge_error_display_matches_message() {
    // Arrange — Display trait 实现一致性
    // @trace REQ-BAO-API-004 [level:integration]
    let cases = [
        BridgeError::InvalidMethod("noDot".into()),
        BridgeError::MethodNotFound("Foo.bar".into()),
        BridgeError::NotSupported("HeapProfiler.x".into()),
        BridgeError::PageNotFound("999".into()),
        BridgeError::ServoError("boom".into()),
        BridgeError::InvalidParams("missing".into()),
    ];

    for err in cases {
        let display = format!("{err}");
        assert_eq!(display, err.message(), "Display should equal message()");
    }
}

#[test]
fn bridge_error_is_send_sync_std_error() {
    // Arrange — BridgeError 必须满足 std::error::Error + Send + Sync
    // (Arc<dyn ServoBackend> 跨线程持有要求错误类型线程安全)
    // @trace REQ-BAO-API-004 [level:integration]
    fn assert_error<T: std::error::Error + Send + Sync + 'static>() {}
    assert_error::<BridgeError>();

    let err = BridgeError::ServoError("test".into());
    let _: &dyn std::error::Error = &err;
}
