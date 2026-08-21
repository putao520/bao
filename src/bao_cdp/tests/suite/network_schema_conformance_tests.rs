// @trace TEST-CDP-037 [req:REQ-CDP-001] [level:unit]
//
// GitHub issue #3: CDP Network 域 method schema conformance。
//
// Oracle(schema 真源): cdp-protocol 0.3.1 — 生成自 Chrome 146.0.7680.165,
// Network 域 40 method 全集(params + ReturnObject serde 类型)。
// 被测面: bao_cdp::protocol::handle_command 的 Network 域 dispatch
// (handle_network match 分支 = Bao 声明面,14 method)。
//
// 三层断言:
// 1. method 清单 — Bao 声明面 ⊆ cdp-protocol 规范面(逐 method 对照
//    Method::NAME 常量),规范面未实现 method 一律 -32601(禁静默成功);
// 2. params round-trip — cdp-protocol 类型构造 → serde 序列化(camelCase
//    wire 形状)→ handle_command 入口 → 断言接受 + 参数键命中;
// 3. return 形状 — 结果 JSON 反序列化进 cdp-protocol ReturnObject 类型
//    (schema 级;no-bridge 确定性路径 + bridge 直通路径)。
//
// 偏差状态(初版报告 → 续刀修复):
// - [已修复] Network.setCookie no-bridge fallback 返回 {} → 现返回 spec
//   形状 {"success":true}(与 bridge/浏览器路径一致);
// - [已修复] bao_browser cookie_to_cdp 补齐 priority/sourceScheme/sourcePort
//   (servo cookie crate 无这三个概念,按 Chromium 文档化默认值填:Medium/
//   Unset/0,证据见 cdp_handler.rs 注释)→ 现可反序列化进 cdp-protocol Cookie;
// - [保持现状] emulateNetworkConditions / setRequestInterception /
//   continueInterceptedRequest 为 no-op 受理(spec 已 deprecated,形状合规
//   语义空转是 deprecated method 的合法实现);
// - [登记不修] 规范面 26 method 未实现(一律 -32601 显式拒绝)。

#![allow(deprecated)]
// cdp-protocol 0.3.1 的 Network 类型整体标注 #[allow(deprecated)]
// (生成器对全协议打标);本测试消费该 crate 作为 oracle,允许引用。

use bao_cdp::{bridge_channel, handle_command, BridgeCommand, BridgeResponse, CdpMessage, CdpResponse};
use cdp_protocol::types::Method;
use cdp_protocol::network as spec;
use serde_json::{json, Value};
use std::time::Duration;

const TID: &str = "test-target";
// JSON-RPC 2.0 method-not-found(未声明 method 的唯一合法应答)。
const ERR_METHOD_NOT_FOUND: i64 = -32601;
// bao_cdp no-bridge 显式失败路径(fail-closed,非假成功)。
const ERR_NO_BRIDGE: i64 = -32603;

/// cdp-protocol 0.3.1 Network 域规范面 — 40 method(Chrome 146 全集)。
const SPEC_NETWORK_METHODS: &[&str] = &[
    "Network.setAcceptedEncodings",
    "Network.clearAcceptedEncodingsOverride",
    "Network.canClearBrowserCache",
    "Network.canClearBrowserCookies",
    "Network.canEmulateNetworkConditions",
    "Network.clearBrowserCache",
    "Network.clearBrowserCookies",
    "Network.continueInterceptedRequest",
    "Network.deleteCookies",
    "Network.disable",
    "Network.emulateNetworkConditions",
    "Network.emulateNetworkConditionsByRule",
    "Network.overrideNetworkState",
    "Network.enable",
    "Network.configureDurableMessages",
    "Network.getAllCookies",
    "Network.getCertificate",
    "Network.getCookies",
    "Network.getResponseBody",
    "Network.getRequestPostData",
    "Network.getResponseBodyForInterception",
    "Network.takeResponseBodyForInterceptionAsStream",
    "Network.replayXHR",
    "Network.searchInResponseBody",
    "Network.setBlockedURLs",
    "Network.setBypassServiceWorker",
    "Network.setCacheDisabled",
    "Network.setCookie",
    "Network.setCookies",
    "Network.setExtraHTTPHeaders",
    "Network.setAttachDebugStack",
    "Network.setRequestInterception",
    "Network.setUserAgentOverride",
    "Network.streamResourceContent",
    "Network.getSecurityIsolationStatus",
    "Network.enableReportingApi",
    "Network.enableDeviceBoundSessions",
    "Network.fetchSchemefulSite",
    "Network.loadNetworkResource",
    "Network.setCookieControls",
];

/// Bao 声明面 — bao_cdp::protocol::handle_network match 分支全集(14 method)。
/// 变更该面时必须同步本清单(清单测试会把两侧差集钉死)。
const BAO_DECLARED_NETWORK_METHODS: &[&str] = &[
    "Network.enable",
    "Network.disable",
    "Network.getCookies",
    "Network.getAllCookies",
    "Network.setCookie",
    "Network.deleteCookies",
    "Network.getResponseBody",
    "Network.setCacheDisabled",
    "Network.setExtraHTTPHeaders",
    "Network.clearBrowserCache",
    "Network.clearBrowserCookies",
    "Network.emulateNetworkConditions",
    "Network.setRequestInterception",
    "Network.continueInterceptedRequest",
];

// ─────────────────────────────────────────────────────────────────────────
// helpers
// ─────────────────────────────────────────────────────────────────────────

/// Dispatch without a servo bridge — deterministic paths only.
fn dispatch_no_bridge(method: &str, params: Option<Value>) -> CdpResponse {
    let msg = CdpMessage {
        id: Some(1),
        method: method.to_string(),
        params: None,
        session_id: None,
    };
    handle_command(msg, TID, &params, None)
}

/// Method-not-found error code discriminator: a *recognized* method never
/// answers -32601 (it answers a result or the explicit no-bridge -32603).
fn error_code(resp: &CdpResponse) -> Option<i64> {
    resp.error.as_ref().map(|e| e.code)
}

// ─────────────────────────────────────────────────────────────────────────
// 1. method 清单 — Bao 声明面 vs cdp-protocol 规范面
// ─────────────────────────────────────────────────────────────────────────

/// 每个 Bao 声明的 method 在 oracle 中都有同名类型,且 Method::NAME wire
/// 字符串与 dispatcher match 字面量一致(禁自创 method)。
#[test]
fn network_declared_methods_pin_oracle_name_constants() {
    // @trace TEST-CDP-037 [req:REQ-CDP-001] [level:unit]
    assert_eq!(<spec::Enable as Method>::NAME, "Network.enable");
    assert_eq!(<spec::Disable as Method>::NAME, "Network.disable");
    assert_eq!(<spec::GetCookies as Method>::NAME, "Network.getCookies");
    assert_eq!(<spec::GetAllCookies as Method>::NAME, "Network.getAllCookies");
    assert_eq!(<spec::SetCookie as Method>::NAME, "Network.setCookie");
    assert_eq!(<spec::DeleteCookies as Method>::NAME, "Network.deleteCookies");
    assert_eq!(
        <spec::GetResponseBody as Method>::NAME,
        "Network.getResponseBody"
    );
    assert_eq!(
        <spec::SetCacheDisabled as Method>::NAME,
        "Network.setCacheDisabled"
    );
    assert_eq!(
        <spec::SetExtraHTTPHeaders as Method>::NAME,
        "Network.setExtraHTTPHeaders"
    );
    assert_eq!(
        <spec::ClearBrowserCache as Method>::NAME,
        "Network.clearBrowserCache"
    );
    assert_eq!(
        <spec::ClearBrowserCookies as Method>::NAME,
        "Network.clearBrowserCookies"
    );
    assert_eq!(
        <spec::EmulateNetworkConditions as Method>::NAME,
        "Network.emulateNetworkConditions"
    );
    assert_eq!(
        <spec::SetRequestInterception as Method>::NAME,
        "Network.setRequestInterception"
    );
    assert_eq!(
        <spec::ContinueInterceptedRequest as Method>::NAME,
        "Network.continueInterceptedRequest"
    );
}

/// 规范清单自检:oracle 侧确有这 40 个 NAME(手抄清单与 crate 逐项钉死,
/// 防止规范面清单本身失真)。
#[test]
fn network_spec_method_inventory_matches_oracle_constants() {
    // @trace TEST-CDP-037 [req:REQ-CDP-001] [level:unit]
    assert_eq!(
        <spec::SetAcceptedEncodings as Method>::NAME,
        "Network.setAcceptedEncodings"
    );
    assert_eq!(
        <spec::ClearAcceptedEncodingsOverride as Method>::NAME,
        "Network.clearAcceptedEncodingsOverride"
    );
    assert_eq!(
        <spec::CanClearBrowserCache as Method>::NAME,
        "Network.canClearBrowserCache"
    );
    assert_eq!(
        <spec::CanClearBrowserCookies as Method>::NAME,
        "Network.canClearBrowserCookies"
    );
    assert_eq!(
        <spec::CanEmulateNetworkConditions as Method>::NAME,
        "Network.canEmulateNetworkConditions"
    );
    assert_eq!(
        <spec::EmulateNetworkConditionsByRule as Method>::NAME,
        "Network.emulateNetworkConditionsByRule"
    );
    assert_eq!(
        <spec::OverrideNetworkState as Method>::NAME,
        "Network.overrideNetworkState"
    );
    assert_eq!(
        <spec::ConfigureDurableMessages as Method>::NAME,
        "Network.configureDurableMessages"
    );
    assert_eq!(<spec::GetCertificate as Method>::NAME, "Network.getCertificate");
    assert_eq!(
        <spec::GetRequestPostData as Method>::NAME,
        "Network.getRequestPostData"
    );
    assert_eq!(
        <spec::GetResponseBodyForInterception as Method>::NAME,
        "Network.getResponseBodyForInterception"
    );
    assert_eq!(
        <spec::TakeResponseBodyForInterceptionAsStream as Method>::NAME,
        "Network.takeResponseBodyForInterceptionAsStream"
    );
    assert_eq!(<spec::ReplayXHR as Method>::NAME, "Network.replayXHR");
    assert_eq!(
        <spec::SearchInResponseBody as Method>::NAME,
        "Network.searchInResponseBody"
    );
    assert_eq!(<spec::SetBlockedURLs as Method>::NAME, "Network.setBlockedURLs");
    assert_eq!(
        <spec::SetBypassServiceWorker as Method>::NAME,
        "Network.setBypassServiceWorker"
    );
    assert_eq!(<spec::SetCookies as Method>::NAME, "Network.setCookies");
    assert_eq!(
        <spec::SetAttachDebugStack as Method>::NAME,
        "Network.setAttachDebugStack"
    );
    assert_eq!(
        <spec::SetUserAgentOverride as Method>::NAME,
        "Network.setUserAgentOverride"
    );
    assert_eq!(
        <spec::StreamResourceContent as Method>::NAME,
        "Network.streamResourceContent"
    );
    assert_eq!(
        <spec::GetSecurityIsolationStatus as Method>::NAME,
        "Network.getSecurityIsolationStatus"
    );
    assert_eq!(
        <spec::EnableReportingApi as Method>::NAME,
        "Network.enableReportingApi"
    );
    assert_eq!(
        <spec::EnableDeviceBoundSessions as Method>::NAME,
        "Network.enableDeviceBoundSessions"
    );
    assert_eq!(
        <spec::FetchSchemefulSite as Method>::NAME,
        "Network.fetchSchemefulSite"
    );
    assert_eq!(
        <spec::LoadNetworkResource as Method>::NAME,
        "Network.loadNetworkResource"
    );
    assert_eq!(
        <spec::SetCookieControls as Method>::NAME,
        "Network.setCookieControls"
    );
    // 声明面是规范面的真子集(14 < 40,双倒查防自创 method)。
    for declared in BAO_DECLARED_NETWORK_METHODS {
        assert!(
            SPEC_NETWORK_METHODS.contains(declared),
            "declared method {declared} is not in the cdp-protocol spec surface"
        );
    }
}

/// Bao 声明面逐 method 受理:recognized method 绝不回答 -32601
/// (回答 result 或显式 -32603 no-bridge)。
#[test]
fn network_declared_surface_all_recognized() {
    // @trace TEST-CDP-037 [req:REQ-CDP-001] [level:unit]
    for method in BAO_DECLARED_NETWORK_METHODS {
        let resp = dispatch_no_bridge(method, Some(json!({})));
        let code = error_code(&resp);
        assert_ne!(
            code,
            Some(ERR_METHOD_NOT_FOUND),
            "{method} is declared in handle_network but answered -32601"
        );
    }
}

/// 规范面未实现的 26 method 一律 -32601(禁静默成功/假绿)。
/// 这是"死方法探测":声明的缺席必须以显式错误暴露。
#[test]
fn network_spec_methods_not_declared_answer_32601() {
    // @trace TEST-CDP-037 [req:REQ-CDP-001] [level:unit]
    let mut rejected = 0usize;
    for method in SPEC_NETWORK_METHODS {
        if BAO_DECLARED_NETWORK_METHODS.contains(method) {
            continue;
        }
        let resp = dispatch_no_bridge(method, Some(json!({})));
        assert_eq!(
            error_code(&resp),
            Some(ERR_METHOD_NOT_FOUND),
            "{method} is not declared; the only legal answer is -32601, got result {:?}",
            resp.result
        );
        rejected += 1;
    }
    // 声明面 14 / 规范面 40 → 拒绝面恒 26。数字变化 = 声明面扩了,同步清单。
    assert_eq!(rejected, 26);
    assert_eq!(BAO_DECLARED_NETWORK_METHODS.len(), 14);
    assert_eq!(SPEC_NETWORK_METHODS.len(), 40);
}

/// 规范外未知 method(死方法)→ -32601。
#[test]
fn network_unknown_method_rejected() {
    // @trace TEST-CDP-037 [req:REQ-CDP-001] [level:unit]
    let resp = dispatch_no_bridge("Network.definitelyNotAMethod", Some(json!({})));
    assert_eq!(error_code(&resp), Some(ERR_METHOD_NOT_FOUND));
}

// ─────────────────────────────────────────────────────────────────────────
// 2. params round-trip — cdp-protocol 类型 → wire → Bao 入口
// ─────────────────────────────────────────────────────────────────────────

/// Network.enable — spec params 全可选(camelCase wire),Bao 必须受理。
/// wire 形状自证:serde 序列化产物就是 Bao 解析的键名。
#[test]
fn network_enable_params_round_trip() {
    // @trace TEST-CDP-037 [req:REQ-CDP-001] [level:unit]
    let params = spec::Enable {
        max_total_buffer_size: Some(10_000_000),
        max_resource_buffer_size: None,
        max_post_data_size: Some(1024),
        report_direct_socket_traffic: None,
        enable_durable_messages: None,
    };
    let wire = serde_json::to_value(&params).unwrap();
    // camelCase + skip_serializing_if(None) — oracle wire 契约自证。
    assert_eq!(wire, json!({"maxTotalBufferSize": 10_000_000, "maxPostDataSize": 1024}));

    let resp = dispatch_no_bridge("Network.enable", Some(wire));
    assert!(resp.error.is_none(), "spec-shaped params must be accepted");
    // return 形状:{} 反序列化进 EnableReturnObject。
    let result = resp.result.unwrap();
    let ret: spec::EnableReturnObject = serde_json::from_value(result).unwrap();
    assert!(ret.0.is_some());
}

/// Network.enable 空 params(全缺省)受理。
#[test]
fn network_enable_empty_params_accepted() {
    // @trace TEST-CDP-037 [req:REQ-CDP-001] [level:unit]
    let resp = dispatch_no_bridge("Network.enable", Some(json!({})));
    assert!(resp.error.is_none());
    let _: spec::EnableReturnObject = serde_json::from_value(resp.result.unwrap()).unwrap();
}

/// Network.disable — newtype params(Option<Json>),空 params 受理。
#[test]
fn network_disable_params_round_trip() {
    // @trace TEST-CDP-037 [req:REQ-CDP-001] [level:unit]
    let wire = serde_json::to_value(spec::Disable(None)).unwrap();
    let resp = dispatch_no_bridge("Network.disable", Some(wire));
    assert!(resp.error.is_none());
    let _: spec::DisableReturnObject = serde_json::from_value(resp.result.unwrap()).unwrap();
}

/// Network.getCookies — spec urls 可选;Bao 解析 params.urls(键名一致)。
#[test]
fn network_get_cookies_params_round_trip() {
    // @trace TEST-CDP-037 [req:REQ-CDP-001] [level:unit]
    let params = spec::GetCookies {
        urls: Some(vec![
                "https://example.com/".to_string(),
                "https://other.example/".to_string(),
            ]),
    };
    let wire = serde_json::to_value(&params).unwrap();
    assert_eq!(
        wire,
        json!({"urls": ["https://example.com/", "https://other.example/"]})
    );

    let resp = dispatch_no_bridge("Network.getCookies", Some(wire));
    assert!(resp.error.is_none());
    // no-bridge 确定性路径:{"cookies":[]} — 反序列化进 GetCookiesReturnObject。
    let ret: spec::GetCookiesReturnObject =
        serde_json::from_value(resp.result.unwrap()).unwrap();
    assert!(ret.cookies.is_empty());
}

/// Network.getAllCookies(spec 已 deprecated 但仍在规范面)受理。
#[test]
fn network_get_all_cookies_round_trip() {
    // @trace TEST-CDP-037 [req:REQ-CDP-001] [level:unit]
    let wire = serde_json::to_value(spec::GetAllCookies(None)).unwrap();
    let resp = dispatch_no_bridge("Network.getAllCookies", Some(wire));
    assert!(resp.error.is_none());
    let ret: spec::GetAllCookiesReturnObject =
        serde_json::from_value(resp.result.unwrap()).unwrap();
    assert!(ret.cookies.is_empty());
}

/// Network.setCookie — spec 13 字段构造,单词键直传;no-bridge fallback
/// 返回 spec 形状 {"success":true}(SetCookieReturnObject;与 bridge/
/// 浏览器路径同一形状)。
#[test]
fn network_set_cookie_params_round_trip() {
    // @trace TEST-CDP-037 [req:REQ-CDP-001] [level:unit]
    let params = spec::SetCookie {
        name: "session".to_string(),
        value: "abc123".to_string(),
        url: Some("https://example.com/".to_string()),
        domain: None,
        path: None,
        secure: Some(true),
        http_only: None,
        same_site: None,
        expires: None,
        priority: None,
        source_scheme: None,
        source_port: None,
        partition_key: None,
    };
    let wire = serde_json::to_value(&params).unwrap();
    // 只序列化 set 键(name/value 必填 + url/secure 可选已设)。
    assert_eq!(
        wire,
        json!({"name": "session", "value": "abc123", "url": "https://example.com/", "secure": true})
    );

    let resp = dispatch_no_bridge("Network.setCookie", Some(wire));
    assert!(resp.error.is_none(), "spec-shaped params must be accepted");
    let result = resp.result.unwrap();
    // spec SetCookieReturnObject:no-bridge stub face 与 bridge 路径同形。
    assert_eq!(result, json!({"success": true}));
    let ret: spec::SetCookieReturnObject = serde_json::from_value(result).unwrap();
    assert!(ret.success);
}

/// Network.deleteCookies — spec name 必填/url 可选;Bao 解析同名键。
#[test]
fn network_delete_cookies_params_round_trip() {
    // @trace TEST-CDP-037 [req:REQ-CDP-001] [level:unit]
    let params = spec::DeleteCookies {
        name: "gone".to_string(),
        url: Some("https://example.com/".to_string()),
        domain: None,
        path: None,
        partition_key: None,
    };
    let wire = serde_json::to_value(&params).unwrap();
    assert_eq!(wire, json!({"name": "gone", "url": "https://example.com/"}));

    let resp = dispatch_no_bridge("Network.deleteCookies", Some(wire));
    assert!(resp.error.is_none());
    let _: spec::DeleteCookiesReturnObject =
        serde_json::from_value(resp.result.unwrap()).unwrap();
}

/// Network.getResponseBody — requestId 键 camelCase wire 自证;no-bridge
/// 显式 -32603(fail-closed:无假 body,无空对象冒充成功)。
#[test]
fn network_get_response_body_params_wire_and_fail_closed() {
    // @trace TEST-CDP-037 [req:REQ-CDP-001] [level:unit]
    let params = spec::GetResponseBody {
        request_id: "REQ-1".to_string(),
    };
    let wire = serde_json::to_value(&params).unwrap();
    // camelCase 键名(requestId)— Bao handle_network 解析同名键。
    assert_eq!(wire, json!({"requestId": "REQ-1"}));

    let resp = dispatch_no_bridge("Network.getResponseBody", Some(wire));
    // 显式失败优于假成功(禁 fallback 冒充)。
    assert_eq!(error_code(&resp), Some(ERR_NO_BRIDGE));
    assert!(resp.result.is_none(), "no fake body on the no-bridge path");
}

/// Network.setCacheDisabled — cacheDisabled 键 camelCase + 默认 false 受理。
#[test]
fn network_set_cache_disabled_params_round_trip() {
    // @trace TEST-CDP-037 [req:REQ-CDP-001] [level:unit]
    let params = spec::SetCacheDisabled {
        cache_disabled: true,
    };
    let wire = serde_json::to_value(&params).unwrap();
    assert_eq!(wire, json!({"cacheDisabled": true}));

    let resp = dispatch_no_bridge("Network.setCacheDisabled", Some(wire));
    assert!(resp.error.is_none());
    let _: spec::SetCacheDisabledReturnObject =
        serde_json::from_value(resp.result.unwrap()).unwrap();

    // 缺省 params(cacheDisabled 默认 false)同样受理。
    let resp_default = dispatch_no_bridge("Network.setCacheDisabled", Some(json!({})));
    assert!(resp_default.error.is_none());
}

/// Network.setExtraHTTPHeaders — headers 对象直传;no-bridge 显式 -32603
/// (servo 无注入 API,fail-closed 报真实支持状态)。
#[test]
fn network_set_extra_http_headers_params_wire_and_fail_closed() {
    // @trace TEST-CDP-037 [req:REQ-CDP-001] [level:unit]
    let params = spec::SetExtraHTTPHeaders {
        headers: spec::Headers(Some(json!({"X-Bao-Test": "1"}))),
    };
    let wire = serde_json::to_value(&params).unwrap();
    assert_eq!(wire, json!({"headers": {"X-Bao-Test": "1"}}));

    let resp = dispatch_no_bridge("Network.setExtraHTTPHeaders", Some(wire));
    assert_eq!(error_code(&resp), Some(ERR_NO_BRIDGE));
    assert!(resp.result.is_none(), "headers must not be silently dropped");
}

/// Network.clearBrowserCache / clearBrowserCookies — 无 params,空对象返回。
#[test]
fn network_clear_browser_cache_and_cookies_round_trip() {
    // @trace TEST-CDP-037 [req:REQ-CDP-001] [level:unit]
    let cache_wire = serde_json::to_value(spec::ClearBrowserCache(None)).unwrap();
    let resp = dispatch_no_bridge("Network.clearBrowserCache", Some(cache_wire));
    assert!(resp.error.is_none());
    let _: spec::ClearBrowserCacheReturnObject =
        serde_json::from_value(resp.result.unwrap()).unwrap();

    let cookies_wire = serde_json::to_value(spec::ClearBrowserCookies(None)).unwrap();
    let resp = dispatch_no_bridge("Network.clearBrowserCookies", Some(cookies_wire));
    assert!(resp.error.is_none());
    let _: spec::ClearBrowserCookiesReturnObject =
        serde_json::from_value(resp.result.unwrap()).unwrap();
}

/// Network.emulateNetworkConditions(spec deprecated)— camelCase 数值键
/// 序列化 + 受理(已知语义为 no-op,shape 层合规)。
#[test]
fn network_emulate_network_conditions_params_round_trip() {
    // @trace TEST-CDP-037 [req:REQ-CDP-001] [level:unit]
    let params = spec::EmulateNetworkConditions {
        offline: true,
        latency: 100.0,
        download_throughput: -1.0,
        upload_throughput: -1.0,
        connection_type: None,
        packet_loss: None,
        packet_queue_length: None,
        packet_reordering: None,
    };
    let wire = serde_json::to_value(&params).unwrap();
    assert_eq!(
        wire,
        json!({"offline": true, "latency": 100.0, "downloadThroughput": -1.0, "uploadThroughput": -1.0})
    );

    let resp = dispatch_no_bridge("Network.emulateNetworkConditions", Some(wire));
    assert!(resp.error.is_none());
    let _: spec::EmulateNetworkConditionsReturnObject =
        serde_json::from_value(resp.result.unwrap()).unwrap();
}

/// Network.setRequestInterception(spec deprecated)— patterns 数组受理。
#[test]
fn network_set_request_interception_params_round_trip() {
    // @trace TEST-CDP-037 [req:REQ-CDP-001] [level:unit]
    let params = spec::SetRequestInterception {
        patterns: vec![spec::RequestPattern {
            url_pattern: Some("*example.com".to_string()),
            resource_type: None,
            interception_stage: None,
        }],
    };
    let wire = serde_json::to_value(&params).unwrap();
    assert_eq!(wire, json!({"patterns": [{"urlPattern": "*example.com"}]}));

    let resp = dispatch_no_bridge("Network.setRequestInterception", Some(wire));
    assert!(resp.error.is_none());
    let _: spec::SetRequestInterceptionReturnObject =
        serde_json::from_value(resp.result.unwrap()).unwrap();
}

/// Network.continueInterceptedRequest(spec deprecated)— interceptionId
/// camelCase 键自证 + 受理。
#[test]
fn network_continue_intercepted_request_params_round_trip() {
    // @trace TEST-CDP-037 [req:REQ-CDP-001] [level:unit]
    let params = spec::ContinueInterceptedRequest {
        interception_id: "INT-1".to_string(),
        error_reason: None,
        raw_response: None,
        url: None,
        method: None,
        post_data: None,
        headers: None,
        auth_challenge_response: None,
    };
    let wire = serde_json::to_value(&params).unwrap();
    assert_eq!(wire, json!({"interceptionId": "INT-1"}));

    let resp = dispatch_no_bridge("Network.continueInterceptedRequest", Some(wire));
    assert!(resp.error.is_none());
    let _: spec::ContinueInterceptedRequestReturnObject =
        serde_json::from_value(resp.result.unwrap()).unwrap();
}

// ─────────────────────────────────────────────────────────────────────────
// 3. bridge 直通 — oracle 形状结果经真实 bridge 通路保持形状
// ─────────────────────────────────────────────────────────────────────────

/// getResponseBody 直通:bridge 回 oracle 形状 body/base64Encoded,
/// handle_command 透传不变形。
#[test]
fn network_get_response_body_bridge_passthrough_oracle_shape() {
    // @trace TEST-CDP-037 [req:REQ-CDP-001] [level:unit]
    let (bridge, rx) = bridge_channel(Duration::from_secs(5));
    let keeper = bridge.clone();
    std::thread::spawn(move || {
        let _keeper = keeper;
        loop {
            let handled = rx.try_process(|_cmd: BridgeCommand| BridgeResponse {
                result: Ok(json!({"body": "<html>hello</html>", "base64Encoded": false})),
            });
            if !handled {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    });

    let params = spec::GetResponseBody {
        request_id: "REQ-9".to_string(),
    };
    let wire = serde_json::to_value(&params).unwrap();
    let msg = CdpMessage {
        id: Some(7),
        method: "Network.getResponseBody".to_string(),
        params: None,
        session_id: None,
    };
    let resp = handle_command(msg, TID, &Some(wire), Some(&bridge));
    assert!(resp.error.is_none(), "bridge path must answer, got {:?}", resp.error);
    let ret: spec::GetResponseBodyReturnObject =
        serde_json::from_value(resp.result.unwrap()).unwrap();
    assert_eq!(ret.body, "<html>hello</html>");
    assert!(!ret.base_64_encoded);
}

/// setCookie 直通:浏览器侧(bao_browser cmd_set_cookie)返回
/// {"success":true} — 与 spec SetCookieReturnObject 一致;对照 no-bridge
/// fallback 的 {} 偏差。
#[test]
fn network_set_cookie_bridge_passthrough_oracle_shape() {
    // @trace TEST-CDP-037 [req:REQ-CDP-001] [level:unit]
    let (bridge, rx) = bridge_channel(Duration::from_secs(5));
    let keeper = bridge.clone();
    std::thread::spawn(move || {
        let _keeper = keeper;
        loop {
            let handled = rx.try_process(|_cmd: BridgeCommand| BridgeResponse {
                result: Ok(json!({"success": true})),
            });
            if !handled {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    });

    let params = spec::SetCookie {
        name: "sid".to_string(),
        value: "v".to_string(),
        url: None,
        domain: Some(".example.com".to_string()),
        path: None,
        secure: None,
        http_only: None,
        same_site: None,
        expires: None,
        priority: None,
        source_scheme: None,
        source_port: None,
        partition_key: None,
    };
    let wire = serde_json::to_value(&params).unwrap();
    let msg = CdpMessage {
        id: Some(8),
        method: "Network.setCookie".to_string(),
        params: None,
        session_id: None,
    };
    let resp = handle_command(msg, TID, &Some(wire), Some(&bridge));
    assert!(resp.error.is_none());
    let ret: spec::SetCookieReturnObject =
        serde_json::from_value(resp.result.unwrap()).unwrap();
    assert!(ret.success, "bridge/browser path returns the spec shape {{success:true}}");
}

/// getCookies 直通:oracle 完整 Cookie(含 priority/sourceScheme/sourcePort)
/// 经通路可反序列化;浏览器侧 cookie_to_cdp 修复后的形状(三字段按
/// Chromium 文档化默认值 Medium/Unset/0 补齐)同样通过 oracle Cookie 校验。
#[test]
fn network_get_cookies_bridge_passthrough_cookie_shape() {
    // @trace TEST-CDP-037 [req:REQ-CDP-001] [level:unit]
    // oracle 完整 Cookie(spec:priority/sourceScheme 无 serde default,必填)。
    let oracle_cookie = json!({
        "name": "sid",
        "value": "v",
        "domain": ".example.com",
        "path": "/",
        "expires": -1.0,
        "size": 5,
        "httpOnly": false,
        "secure": true,
        "session": true,
        "sameSite": "Lax",
        "priority": "Medium",
        "sourceScheme": "Secure",
        "sourcePort": 443
    });

    let (bridge, rx) = bridge_channel(Duration::from_secs(5));
    let keeper = bridge.clone();
    let reply = json!({ "cookies": [oracle_cookie] });
    std::thread::spawn(move || {
        let _keeper = keeper;
        loop {
            let handled = rx.try_process(|_cmd: BridgeCommand| BridgeResponse {
                result: Ok(reply.clone()),
            });
            if !handled {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    });

    let wire = serde_json::to_value(spec::GetCookies { urls: None }).unwrap();
    let msg = CdpMessage {
        id: Some(9),
        method: "Network.getCookies".to_string(),
        params: None,
        session_id: None,
    };
    let resp = handle_command(msg, TID, &Some(wire), Some(&bridge));
    assert!(resp.error.is_none());
    let ret: spec::GetCookiesReturnObject =
        serde_json::from_value(resp.result.unwrap()).unwrap();
    assert_eq!(ret.cookies.len(), 1);
    assert_eq!(ret.cookies[0].name, "sid");

    // cookie_to_cdp 修复后形状(Medium/Unset/0 为 servo cookie crate 拿不到
    // 时的 Chromium 文档化默认值)— 必须通过 oracle Cookie 校验。
    let browser_shape = json!({
        "cookies": [{
            "name": "sid",
            "value": "v",
            "domain": ".example.com",
            "path": "/",
            "expires": -1.0,
            "size": 5,
            "httpOnly": false,
            "secure": true,
            "sameSite": "Lax",
            "session": true,
            "priority": "Medium",
            "sourceScheme": "Unset",
            "sourcePort": 0
        }]
    });
    let parsed: spec::GetCookiesReturnObject = serde_json::from_value(browser_shape)
        .expect("cookie_to_cdp full shape must pass oracle Cookie validation");
    assert_eq!(parsed.cookies[0].priority, spec::CookiePriority::Medium);
    assert_eq!(parsed.cookies[0].source_scheme, spec::CookieSourceScheme::Unset);
    assert_eq!(parsed.cookies[0].source_port, 0);
}
