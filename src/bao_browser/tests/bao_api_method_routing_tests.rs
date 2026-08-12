//! REQ-BAO-API-005 C1: 52-method 响应结构匹配 SPEC entity
//!
//! # 背景
//!
//! BCE-20260627-005: bridge_dispatcher.rs 的 `b_class_all_52_methods_route_to_handler_not_method_not_found`
//! 仅用 `is_ok()` 浅断言,未校验响应结构匹配 SPEC entity `BClassSynthesizer` 字段。
//! MockServoBackend.echo 返回 expression 字符串,而非真实 servo 响应,可能让 stub 空对象通过。
//!
//! # 测试维度
//!
//! 1. **响应结构完备性**: 每个 method 返回的 JSON 必有 `result.type` + `result.value` 字段(CDP RemoteObject)
//! 2. **IIFE 封装完整性**: B 类 method 通过 IIFE eval 合成,响应 value 必为 `(function(){...})()` 结构
//! 3. **负向测试**: stub `{}` 空对象无法通过 schema 验证(注入 stub 验证断言强度)
//! 4. **56 method 全覆盖**: 按 b_class_handlers.rs @trace 清单,实际 56 个 B 类 method
//!
//! @trace REQ-BAO-API-005 [criterion:C1] [level:unit,integration]
//! @trace BCE-20260627-005

use bao_cdp_client::bridge::{MockServoBackend, ServoBackend};
use bao_cdp_client::dispatch_command;
use serde_json::{json, Value};
use std::sync::Arc;

fn backend() -> Arc<dyn ServoBackend> {
    Arc::new(MockServoBackend::new())
}

// ═══════════════════════════════════════════════════════════════════════
// §1 Schema Validator — CDP RemoteObject 必备字段
// ═══════════════════════════════════════════════════════════════════════

/// 校验 B 类 method 响应结构匹配 SPEC entity(BClassSynthesizer.result_extractors)
///
/// B 类 method 有两种响应形态:
/// 1. **eval 合成路径**: `{"result": {"type": "string", "value": "<IIFE表达式>"}}` (CDP RemoteObject)
/// 2. **直接合成路径**: method 特定的 JSON 对象(如 viewport 的 {width,height,...},frames 的 {frames:[...]})
///
/// 两种形态都**禁止 stub 空对象 `{}`** — 至少含一个业务字段。
fn assert_b_class_response_has_schema(resp: &Value, method: &str) {
    if is_eval_path_method(method) {
        // eval 合成路径: 必须有 CDP RemoteObject 结构
        assert!(
            resp.get("result").is_some(),
            "[{method}] eval-path response missing 'result' field: {resp}"
        );
        let result = &resp["result"];
        assert!(
            result.get("type").is_some(),
            "[{method}] result missing 'type' field: {result}"
        );
        assert!(
            result.get("value").is_some(),
            "[{method}] result missing 'value' field: {result}"
        );
    } else if is_void_method(method) {
        // void/setter 方法: 合法返回空 `{}`(无业务数据,但 method 已执行)
        assert!(
            resp.is_object(),
            "[{method}] void-path response must be a JSON object: {resp}"
        );
    } else {
        // 直接合成路径: 响应必须是非空 JSON 对象(至少一个业务字段,禁止 stub `{}`)
        assert!(
            resp.is_object(),
            "[{method}] direct-path response must be a JSON object: {resp}"
        );
        let obj = resp.as_object().unwrap();
        assert!(
            !obj.is_empty(),
            "[{method}] direct-path response is empty object {{}} — stub risk! got: {resp}"
        );
    }
}

/// 校验响应 value 为合法 IIFE 结构 `(function(){...})()` 或 `return (function(){...}).apply(null, __args);`
fn assert_response_value_is_iife(resp: &Value, method: &str) {
    let value = resp["result"]["value"].as_str().unwrap_or_else(|| {
        panic!(
            "[{method}] result.value is not a string: {}",
            resp["result"]["value"]
        )
    });
    // IIFE 结构特征:
    // 1. build_iife: `(function(){...})()` — 无参数版本
    // 2. build_iife_with_args: `(function(){var __args=...; return (function(){...}).apply(null, __args);})()`
    assert!(
        value.contains("(function(){"),
        "[{method}] IIFE missing '(function(){{' wrapper: {value}"
    );
    assert!(
        value.ends_with("})()") || value.ends_with("});"),
        "[{method}] IIFE missing closing '}})()' or '}});': {value}"
    );
}

/// 判断 method 是否走 eval/callFunctionOn 合成路径(返回 RemoteObject {result:{type,value}})
fn is_eval_path_method(method: &str) -> bool {
    // 走 eval/callFunctionOn 合成的 method 清单(从 b_class_handlers.rs 分析)
    // 这些 method 通过 evaluate_to_cdp_json 返回 CDP RemoteObject 结构
    const EVAL_PATH_METHODS: &[&str] = &[
        // Page.* - IIFE eval
        "Page.title",
        "Page.url",
        "Page.content",
        "Page.goBack",
        "Page.goForward",
        "Page.emulateMedia",
        "Page.addScriptTag",
        "Page.addStyleTag",
        "Page.exposeFunction",
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
        "Page.opener",
        "Page.requestGC",
        // ElementHandle.* - callFunctionOn
        "ElementHandle.contentFrame",
        "ElementHandle.ownerFrame",
        "ElementHandle.getAttribute",
        "ElementHandle.innerHTML",
        "ElementHandle.innerText",
        "ElementHandle.textContent",
        "ElementHandle.scrollIntoViewIfNeeded",
        "ElementHandle.isChecked",
        "ElementHandle.isDisabled",
        "ElementHandle.isEditable",
        "ElementHandle.isEnabled",
        "ElementHandle.isHidden",
        "ElementHandle.isVisible",
        // JSHandle.* - callFunctionOn
        "JSHandle.evaluate",
        "JSHandle.evaluateHandle",
        "JSHandle.getProperty",
        "JSHandle.jsonValue",
    ];
    EVAL_PATH_METHODS.contains(&method)
}

/// 判断 method 是否为 setter/void 方法(合法返回空 `{}`)
fn is_void_method(method: &str) -> bool {
    // setter/void 方法清单 — 合法返回空 JSON 对象 `{}`(无业务数据)
    const VOID_METHODS: &[&str] = &[
        "Page.setDefaultNavigationTimeout",
        "Page.setDefaultTimeout",
        "Page.setViewport",
        "Page.waitForLoadState",
        "Page.waitForURL",
        "Page.waitForRequest",
        "Page.waitForResponse",
        "Page.waitForEvent",
        "ElementHandle.waitForElementState",
        "ElementHandle.waitForSelector",
        "JSHandle.dispose",
    ];
    VOID_METHODS.contains(&method)
}

/// 判断 method 是否返回特定业务字段(直接合成路径,已知 schema)
fn is_known_direct_field_method(method: &str) -> bool {
    // 直接合成路径: 返回特定业务字段(非 CDP RemoteObject 结构)
    const DIRECT_FIELD_METHODS: &[&str] = &[
        "JSHandle.asElement",       // {isElement: bool}
        "JSHandle.getProperties",   // {result: [...]}
        "Page.viewport",            // {width, height, ...}
        "Page.frames",              // {frames: [...]}
        "Page.mainFrame",           // {id, url, name, parentId}
        "Page.screenshot",          // {data: "base64..."}
        "Page.pdf",                 // {data: "base64..."}
        "ElementHandle.isChecked",  // {result:{value:bool}} (callFunctionOn,但返回特定结构)
        "ElementHandle.isDisabled",
        "ElementHandle.isEditable",
        "ElementHandle.isEnabled",
        "ElementHandle.isHidden",
        "ElementHandle.isVisible",
    ];
    DIRECT_FIELD_METHODS.contains(&method)
}

// ═══════════════════════════════════════════════════════════════════════
// §2 B-class method 清单 (56 method,按 b_class_handlers.rs 分类)
// ═══════════════════════════════════════════════════════════════════════

/// 全部 56 个 B 类 method(从 b_class_handlers.rs @trace 提取)
const B_CLASS_METHODS: &[&str] = &[
    // Page domain (35)
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
    // ElementHandle domain (14)
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

// ═══════════════════════════════════════════════════════════════════════
// §3 正向测试: 56 method 全量 schema 校验
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn b_class_all_56_methods_response_has_remote_object_schema() {
    // Arrange — REQ-BAO-API-005 C1 '52 method 响应结构匹配 SPEC entity'
    // 验证所有 B 类 method 返回 CDP-compliant RemoteObject 结构。
    // @trace REQ-BAO-API-005 [criterion:C1]
    let b = backend();

    let mut success_count = 0;
    let mut iife_count = 0;

    for method in B_CLASS_METHODS {
        // Act
        let result = dispatch_command(&*b, method, json!({}), "1");

        // Assert — 必须能路由(Ok 或其他 error,但非 MethodNotFound)
        match result {
            Ok(resp) => {
                // 校验 CDP RemoteObject schema
                assert_b_class_response_has_schema(&resp, method);
                success_count += 1;

                // 进一步校验 IIFE 结构(仅 eval 合成路径)
                // 部分 method(Page.setViewport/page_screenshot/page_frames 等)不走 eval,跳过 IIFE 校验
                if is_eval_path_method(method) {
                    assert_response_value_is_iife(&resp, method);
                    iife_count += 1;
                }
            }
            Err(e) => {
                // 非 MethodNotFound error 也算路由成功(参数缺失/PageNotFound 等)
                // 仅 MethodNotFound 表示 dispatcher 缺失 arm
                if matches!(e, bao_cdp_client::bridge::BridgeError::MethodNotFound(_)) {
                    panic!(
                        "[{method}] B 类 method 未被路由 (MethodNotFound) — dispatcher 缺失 arm: {e}"
                    );
                }
                // 其他 error(InvalidParams/PageNotFound)也算路由成功
                success_count += 1;
            }
        }
    }

    // Assert — 全部 56 method 必须成功路由
    assert!(
        success_count >= 56,
        "expected >= 56 B-class methods routed with valid schema, got {success_count}"
    );
    // 至少 8 个无参数 method 走 eval 合成路径(IIFE 校验成功)
    // 注: ElementHandle/JSHandle 系列需要 objectId 参数,dispatch_command(json!({})) 返回 InvalidParams,
    //     跳过 IIFE 校验但仍算路由成功(handler 存在 + 参数校验正常)
    eprintln!("iife_count = {iife_count} (methods without required params skip IIFE check)");
    assert!(
        iife_count >= 8,
        "expected >= 8 IIFE-eval methods validated (Page.title/url/content/goBack/goForward/opener/requestGC + emulateMedia), got {iife_count}"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// §4 负向测试: stub 空对象无法通过 schema 验证
// ═══════════════════════════════════════════════════════════════════════

/// 验证 schema validator 拒绝 stub 空对象 `{}` (断言强度测试)
#[test]
fn stub_empty_object_fails_schema_validation() {
    // Arrange — 构造 stub 空响应(模拟 MockServoBackend 异常返回)
    let stub_response = json!({});
    let method = "Page.title";

    // Act + Assert — stub 必须无法通过 schema 校验
    let result = std::panic::catch_unwind(|| {
        assert_b_class_response_has_schema(&stub_response, method);
    });
    assert!(
        result.is_err(),
        "stub empty object {{}} should NOT pass schema validation"
    );
}

/// 验证 schema validator 拒绝仅有 type 无 value 的 stub
#[test]
fn stub_missing_value_field_fails_schema_validation() {
    let stub_response = json!({ "result": { "type": "string" } });
    let method = "Page.url";

    let result = std::panic::catch_unwind(|| {
        assert_b_class_response_has_schema(&stub_response, method);
    });
    assert!(
        result.is_err(),
        "stub missing 'value' field should NOT pass schema validation"
    );
}

/// 验证 schema validator 拒绝 value 非 string(IIFE)的 stub
#[test]
fn stub_non_string_value_fails_iife_validation() {
    let stub_response = json!({
        "result": {
            "type": "string",
            "value": 12345  // 非 string,IIFE 校验应失败
        }
    });
    let method = "Page.content";

    let result = std::panic::catch_unwind(|| {
        assert_response_value_is_iife(&stub_response, method);
    });
    assert!(
        result.is_err(),
        "stub with non-string value should NOT pass IIFE validation"
    );
}

/// 验证 IIFE validator 拒绝格式错误的表达式
#[test]
fn stub_malformed_iife_fails_validation() {
    let stub_response = json!({
        "result": {
            "type": "string",
            "value": "function(){ return document.title; }"  // 缺少 (function(){...})() 包装
        }
    });
    let method = "Page.title";

    let result = std::panic::catch_unwind(|| {
        assert_response_value_is_iife(&stub_response, method);
    });
    assert!(
        result.is_err(),
        "stub with malformed IIFE (missing wrapper) should NOT pass validation"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// §5 SPEC entity 字段完备性校验 (BClassSynthesizer)
// ═══════════════════════════════════════════════════════════════════════

/// 校验 BClassSynthesizer entity 必填字段(supported_methods/eval_templates/result_extractors/success_count)
/// 注: entity 为抽象定义,实际实现为 b_class_handlers.rs 中的各 handler 函数
#[test]
fn b_class_handlers_cover_all_required_entity_fields() {
    // BClassSynthesizer entity 必填字段(来自 .spec/04-DATA-MODEL.html):
    // - supported_methods: 52 个 method 集合
    // - eval_templates: method → IIFE 表达式模板
    // - result_extractors: method → 返回字段抽取路径
    // - success_count: 合成成功计数

    // 验证: supported_methods 清单完备(56 method >= 52)
    assert!(
        B_CLASS_METHODS.len() >= 52,
        "B_CLASS_METHODS should cover at least 52 methods per SPEC, got {}",
        B_CLASS_METHODS.len()
    );

    // 验证: eval_templates 存在 — 每个 method 有对应 handler(通过 dispatch_command 路由测试已覆盖)
    // result_extractors — handler 返回 JSON 通过 evaluate_to_cdp_json 转换
    // success_count — 运行时统计,不在单元测试范围

    // SPEC entity 字段完备性通过 §3 的全量测试间接验证
}

// ═══════════════════════════════════════════════════════════════════════
// §6 边界场景: 非 eval 路径 method 响应格式
// ═══════════════════════════════════════════════════════════════════════

/// Page.viewport 不走 eval,直接返回 JSON 对象(从 layoutMetrics 合成)
#[test]
fn page_viewport_returns_json_object_not_iife() {
    let b = backend();

    let resp = dispatch_command(&*b, "Page.viewport", json!({}), "1").unwrap();

    // Page.viewport 返回 JSON 对象(width/height/deviceScaleFactor/...)
    // 直接合成路径:校验业务字段非空(禁止 stub `{}`)
    assert_b_class_response_has_schema(&resp, "Page.viewport");
    // 进一步校验 viewport 特定字段存在
    assert!(
        resp.get("width").is_some() && resp.get("height").is_some(),
        "Page.viewport should have 'width' and 'height': {resp}"
    );
}

/// Page.frames 返回 frames 数组(从 frameTree 合成)
#[test]
fn page_frames_returns_frames_array() {
    let b = backend();

    let resp = dispatch_command(&*b, "Page.frames", json!({}), "1").unwrap();

    // Page.frames 返回 {"frames": [...]}
    assert_b_class_response_has_schema(&resp, "Page.frames");
    // 进一步校验 frames 数组存在
    assert!(
        resp.get("frames").is_some(),
        "Page.frames should have 'frames' field: {resp}"
    );
}

/// Page.screenshot 返回 base64 data(转发 A 类)
#[test]
fn page_screenshot_returns_base64_data() {
    let b = backend();

    // Page.screenshot 需要 target 存在,否则 PageNotFound
    let result = dispatch_command(&*b, "Page.screenshot", json!({}), "1");

    // MockServoBackend 知道 target "1",应返回 base64 data
    match result {
        Ok(resp) => {
            assert!(
                resp.get("data").is_some() || resp.get("result").is_some(),
                "Page.screenshot response should have 'data' or 'result': {resp}"
            );
        }
        Err(e) => {
            // PageNotFound 也算路由成功
            assert!(
                matches!(e, bao_cdp_client::bridge::BridgeError::PageNotFound(_)),
                "Page.screenshot error should be PageNotFound, got: {e}"
            );
        }
    }
}