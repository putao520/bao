//! M1 平行 CDP 死宇宙接线锁(REQ-CDP-001;e152 审计 DUP-CDP-PARALLEL;
//! 用户裁决「留并接线」)。
//!
//! 锁定三个不变量:
//!
//! 1. **生产入口可达**——B 类 Playwright 方法面(Page.title / Page.url /
//!    ElementHandle.*)经真实 memory:// 客户端入口(`Browser::connect` →
//!    `MemoryCdpBridge`)与 WS 入口(`BaoWsRegistry`)可达,且终结于同一
//!    生产 BridgeCommand 通道(伪 responder 断言收到的就是通道命令)。
//! 2. **生产命令核优先**——生产 `bao_cdp::handle_command` 已知的方法
//!    (Page.navigate)由生产臂应答,-32601 才落 fallback;显式
//!    not_supported(-32000)不被遮蔽。
//! 3. **未知方法保持生产错误形**——双宇宙都未命中时,客户端看到生产
//!    Chrome 形错误("'X.y' wasn't found"),不是内部 miss。
//!
//! 伪 responder 按生产 `cdp_handler` 的真实响应形状作答(单一真值后端
//! 的形状契约),不起全 servo。
//!
//! @trace REQ-CDP-001 [level:integration]
//! @trace REQ-CDP-003 [level:integration]

use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use bao_browser::cdp_memory::MemoryCdpBridge;
use bao_cdp::servo_bridge::{bridge_channel, BridgeCommand, BridgeReceiver, BridgeResponse};
use bao_cdp::CdpMessage;
use bao_cdp_client::browser::{clear_process_memory_bridge, set_process_memory_bridge, Browser};
use bao_cdp_client::transport::in_memory::InMemoryBridgeResponse;
use cdp_server::{EventSender, RegistryDispatch};
use serde_json::{json, Value};

struct NopSender;
impl EventSender for NopSender {
    fn send_event(&self, _: &str, _: Value) {}
}

/// memory:// 入口的测试共享进程全局 bridge 注册表(last-writer-wins)——
/// libtest 并行下必须互斥(每测试独立进程的 nextest 不受影响)。
static REGISTRY_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 起伪主线程 responder:按生产 cdp_handler 响应形状作答,并把收到的
/// BridgeCommand 快照回传(供断言「死宇宙方法面真的走生产通道」)。
fn spawn_shape_responder(
    rx: BridgeReceiver,
    seen: mpsc::Sender<String>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || loop {
        let handled = rx.try_process(|cmd| {
            let _ = seen.send(format!("{cmd:?}"));
            BridgeResponse {
                result: match cmd {
                    BridgeCommand::Navigate { .. } => Ok(json!({
                        "frameId": "main-1", "loaderId": "loader-1"
                    })),
                    BridgeCommand::EvaluateJs { expression, .. } => {
                        // 生产 cmd_evaluate(rbv)envelope 形状;按表达式
                        // 内容返回可判别的值。
                        if expression.contains("document.title") {
                            Ok(json!({
                                "result": { "type": "string", "value": "Wired Title" },
                                "exceptionDetails": null
                            }))
                        } else if expression.contains("location.href") {
                            Ok(json!({
                                "result": { "type": "string", "value": "https://wired/" },
                                "exceptionDetails": null
                            }))
                        } else {
                            Ok(json!({
                                "result": { "type": "string", "value": "attr-value" },
                                "exceptionDetails": null
                            }))
                        }
                    }
                    BridgeCommand::RuntimeCallFunctionOn { .. } => Ok(json!({
                        "result": { "type": "string", "value": "attr-value" },
                        "exceptionDetails": null
                    })),
                    _ => Ok(json!({})),
                },
            }
        });
        if !handled {
            std::thread::sleep(Duration::from_millis(1));
        }
    })
}

// ─── memory:// 生产入口 ─────────────────────────────────────────────────────

/// B 类 Page.title 经真实客户端入口(memory://bao)到达生产通道并被应答。
///
/// @trace REQ-CDP-001 [level:integration]
#[test]
fn memory_entry_serves_b_class_page_title() {
    let _registry_guard = REGISTRY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let (sender, receiver) = bridge_channel(Duration::from_secs(5));
    let (seen_tx, seen_rx) = mpsc::channel::<String>();
    let responder = spawn_shape_responder(receiver, seen_tx);

    let bridge: Arc<MemoryCdpBridge> = MemoryCdpBridge::new_with_sender("1", sender.clone());
    let token = set_process_memory_bridge(bridge);
    let mut browser = Browser::connect("memory://bao").expect("connect");
    let result = browser
        .send_command("Page.title", json!({}))
        .expect("Page.title must be served");

    // B 类 page_title → runtime_evaluate(IIFE document.title)→ envelope。
    assert_eq!(result["result"]["value"], "Wired Title");
    assert_eq!(result["result"]["type"], "string");

    // 死宇宙方法面真的走了生产 BridgeCommand 通道(单一真值后端)。
    let commands: Vec<String> = seen_rx.try_iter().collect();
    assert!(
        commands.iter().any(|c| c.contains("EvaluateJs") && c.contains("document.title")),
        "Page.title must ride the production EvaluateJs channel, saw: {commands:?}"
    );

    clear_process_memory_bridge(token);
    // responder is a daemon (infinite drain loop) — never join it; process
    // teardown at test end reclaims everything.
    drop(responder);
    drop(browser);
    drop(sender);
}

/// 生产命令核优先:Page.navigate 由生产臂应答(生产响应形状)。
///
/// @trace REQ-CDP-001 [level:integration]
#[test]
fn memory_entry_production_arm_wins_for_known_commands() {
    let _registry_guard = REGISTRY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let (sender, receiver) = bridge_channel(Duration::from_secs(5));
    let (seen_tx, seen_rx) = mpsc::channel::<String>();
    let responder = spawn_shape_responder(receiver, seen_tx);

    let bridge: Arc<MemoryCdpBridge> = MemoryCdpBridge::new_with_sender("1", sender.clone());
    let token = set_process_memory_bridge(bridge);
    let mut browser = Browser::connect("memory://bao").expect("connect");
    let result = browser
        .send_command("Page.navigate", json!({"url": "https://wired/"}))
        .expect("navigate must be served by the production arm");

    // 生产臂的响应形状:frameId + loaderId(REQ-CDP-004 单一 frame 命名)。
    assert_eq!(result["frameId"], "main-1");
    assert_eq!(result["loaderId"], "loader-1");

    let commands: Vec<String> = seen_rx.try_iter().collect();
    assert!(
        commands.iter().any(|c| c.contains("Navigate")),
        "Page.navigate must ride the production Navigate channel, saw: {commands:?}"
    );

    clear_process_memory_bridge(token);
    // responder is a daemon (infinite drain loop) — never join it; process
    // teardown at test end reclaims everything.
    drop(responder);
    drop(browser);
    drop(sender);
}

/// 未知方法:双宇宙都未命中 → 客户端看到生产 Chrome 形错误。
///
/// @trace REQ-CDP-001 [level:integration]
#[test]
fn memory_entry_unknown_method_keeps_production_error() {
    let _registry_guard = REGISTRY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let (sender, receiver) = bridge_channel(Duration::from_secs(5));
    let (seen_tx, _seen_rx) = mpsc::channel::<String>();
    let responder = spawn_shape_responder(receiver, seen_tx);

    let bridge: Arc<MemoryCdpBridge> = MemoryCdpBridge::new_with_sender("1", sender.clone());
    let token = set_process_memory_bridge(bridge);
    let mut browser = Browser::connect("memory://bao").expect("connect");
    let err = browser
        .send_command("Nope.nada", json!({}))
        .expect_err("unknown method must fail");

    let msg = format!("{err}");
    assert!(
        msg.contains("wasn't found"),
        "production error shape must be kept, got: {msg}"
    );

    clear_process_memory_bridge(token);
    // responder is a daemon (infinite drain loop) — never join it; process
    // teardown at test end reclaims everything.
    drop(responder);
    drop(browser);
    drop(sender);
}

/// 显式 not_supported(-32000)不被 fallback 遮蔽:Page.getNavigationHistory
/// 保持生产的诚实失败。
///
/// @trace REQ-CDP-001 [level:integration]
#[test]
fn memory_entry_explicit_not_supported_stays_authoritative() {
    let _registry_guard = REGISTRY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let (sender, receiver) = bridge_channel(Duration::from_secs(5));
    let (seen_tx, _seen_rx) = mpsc::channel::<String>();
    let responder = spawn_shape_responder(receiver, seen_tx);

    let bridge: Arc<MemoryCdpBridge> = MemoryCdpBridge::new_with_sender("1", sender.clone());
    let token = set_process_memory_bridge(bridge);
    let mut browser = Browser::connect("memory://bao").expect("connect");
    let err = browser
        .send_command("Page.getNavigationHistory", json!({}))
        .expect_err("getNavigationHistory must stay an explicit not-supported");

    let msg = format!("{err}");
    assert!(
        msg.contains("not supported"),
        "explicit not-supported verdict must stay authoritative, got: {msg}"
    );

    clear_process_memory_bridge(token);
    // responder is a daemon (infinite drain loop) — never join it; process
    // teardown at test end reclaims everything.
    drop(responder);
    drop(browser);
    drop(sender);
}

// ─── WS 生产入口 ───────────────────────────────────────────────────────────

/// WS 面:BaoWsRegistry 的 -32601 fallback 服务 B 类方法(Playwright 直连
/// 形态的方法面扩张)。
///
/// @trace REQ-CDP-001 [level:integration]
#[test]
fn ws_entry_serves_b_class_page_url() {
    let (sender, receiver) = bridge_channel(Duration::from_secs(5));
    let (seen_tx, seen_rx) = mpsc::channel::<String>();
    let responder = spawn_shape_responder(receiver, seen_tx);

    let registry = bao_browser::BaoWsRegistry::new(sender);
    let result = registry
        .dispatch_command("Page.url", json!({}), &NopSender)
        .expect("Page.url must be served on the WS face")
        .expect("Page.url must answer Ok");
    assert_eq!(result["result"]["value"], "https://wired/");

    let commands: Vec<String> = seen_rx.try_iter().collect();
    assert!(
        commands.iter().any(|c| c.contains("EvaluateJs") && c.contains("location.href")),
        "Page.url must ride the production EvaluateJs channel, saw: {commands:?}"
    );

    drop(responder);
}

/// WS 面:ElementHandle 域(生产核心不知道的域)经 fallback 可达,并走
/// RuntimeCallFunctionOn 生产通道。
///
/// @trace REQ-CDP-001 [level:integration]
#[test]
fn ws_entry_element_handle_domain_reachable() {
    let (sender, receiver) = bridge_channel(Duration::from_secs(5));
    let (seen_tx, seen_rx) = mpsc::channel::<String>();
    let responder = spawn_shape_responder(receiver, seen_tx);

    let registry = bao_browser::BaoWsRegistry::new(sender);
    let result = registry.dispatch_command(
        "ElementHandle.getAttribute",
        json!({"objectId": "obj-1", "name": "id"}),
        &NopSender,
    );
    let result = result
        .expect("ElementHandle.getAttribute must be served")
        .expect("ElementHandle.getAttribute must answer Ok");
    assert_eq!(result["result"]["value"], "attr-value");

    let commands: Vec<String> = seen_rx.try_iter().collect();
    assert!(
        commands.iter().any(|c| c.contains("RuntimeCallFunctionOn")),
        "ElementHandle.getAttribute must ride RuntimeCallFunctionOn, saw: {commands:?}"
    );

    drop(responder);
}

/// WS 面:未知方法保持生产错误形。
///
/// @trace REQ-CDP-001 [level:integration]
#[test]
fn ws_entry_unknown_method_keeps_production_error() {
    let (sender, receiver) = bridge_channel(Duration::from_secs(5));
    let (seen_tx, _seen_rx) = mpsc::channel::<String>();
    let responder = spawn_shape_responder(receiver, seen_tx);

    let registry = bao_browser::BaoWsRegistry::new(sender);
    let err = registry
        .dispatch_command("Nope.nada", json!({}), &NopSender)
        .expect("dispatch must produce a verdict")
        .expect_err("unknown method must fail");
    let msg = err.message;
    assert!(
        msg.contains("wasn't found"),
        "production error shape must be kept, got: {msg}"
    );

    drop(responder);
}

/// MemoryCdpBridge 直接面(fallback 逻辑单元锁,不经进程注册表)。
///
/// @trace REQ-CDP-001 [level:integration]
#[test]
fn memory_bridge_direct_dispatch_serves_and_preserves() {
    let _registry_guard = REGISTRY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let (sender, receiver) = bridge_channel(Duration::from_secs(5));
    let (seen_tx, _seen_rx) = mpsc::channel::<String>();
    let responder = spawn_shape_responder(receiver, seen_tx);

    let bridge: Arc<MemoryCdpBridge> = MemoryCdpBridge::new_with_sender("1", sender);
    use bao_cdp_client::transport::in_memory::InMemoryBridge;

    // B 类命中。
    match bridge.dispatch_command("Page.content", json!({}), None) {
        InMemoryBridgeResponse::Ok(v) => assert!(v["result"].is_object()),
        InMemoryBridgeResponse::Err(e) => panic!("Page.content must be served, got: {e}"),
    }
    // 未知保持生产错误形。
    match bridge.dispatch_command("Nope.nada", json!({}), None) {
        InMemoryBridgeResponse::Err(e) => assert!(e.contains("wasn't found")),
        InMemoryBridgeResponse::Ok(_) => panic!("unknown method must fail"),
    }

    drop(responder);
}

// ─── M1 P1:waitFor* 事件订阅面(translate → tap → waitFor,REQ-CDP-001)───

/// M1 P1 wiring 形状锁:宿主泵对每个 translate 产出的 CDP 事件调用
/// `tap.observe`(与 WS 广播同一事件流)。事件源用真 `translate`(单一
/// 语义源),非手写 params。
fn feed_translated_event(tap: &bao_cdp_client::bridge::CdpEventTap, url: &str) {
    let servo_event = bao_cdp_client::bridge::ServoEvent::NetworkRequest {
        target_id: "1".to_string(),
        request_id: "req-7".to_string(),
        url: url.to_string(),
        method: "GET".to_string(),
        headers: [("host".to_string(), "x".to_string())].into_iter().collect(),
        post_data: None,
        resource_type: "Document".to_string(),
        frame_id: "main-1".to_string(),
    };
    // 与 run_with_bridge 泵同一组合:translate → observe(target, method, params)。
    for ev in bao_cdp_client::bridge::translate(servo_event) {
        tap.observe("1", &ev.method, &ev.params);
    }
}

/// WS 面:waitFor* 经生产入口(dispatch_command → -32601 fallback)在
/// registry 的事件 tap 上等到真实 translate 事件并返回其 params。
///
/// 锁三面:①tap 在 WS 生产入口存在(registry.event_tap)②waitFor* 消费
/// tap(事件订阅实装,占位 OK 已替换)③事件形状 = translate 真源。
///
/// @trace REQ-CDP-001 [level:integration]
#[test]
fn ws_entry_wait_for_request_resolves_on_translated_event_tap() {
    let (sender, receiver) = bridge_channel(Duration::from_secs(5));
    let (seen_tx, _seen_rx) = mpsc::channel::<String>();
    let responder = spawn_shape_responder(receiver, seen_tx);

    let registry = Arc::new(bao_browser::BaoWsRegistry::new(sender));
    let tap = registry
        .event_tap()
        .expect("WS registry must expose the fallback universe's event tap");

    let dispatch_registry = registry.clone();
    let waiter = std::thread::spawn(move || {
        // Page-endpoint shape(target "1")— waitFor 的 tap 订阅按命令解析的
        // target 注册,事件也按 servo 事件流的 target(十进制页 id)喂入。
        let m = CdpMessage {
            id: None,
            method: "Page.waitForRequest".to_string(),
            params: Some(json!({"url": "**/api/*"})),
            session_id: None,
        };
        dispatch_registry
            .dispatch_message(&m, "1", &NopSender)
            .expect("waitForRequest must be served on the WS face")
            .expect("waitForRequest must answer Ok with the event params")
    });

    // 等待者注册后喂入(泵时序:事件在等待之后到达)。
    std::thread::sleep(Duration::from_millis(80));
    feed_translated_event(&tap, "https://x/static/logo.png"); // 不匹配 → 不得 resolve
    std::thread::sleep(Duration::from_millis(50));
    feed_translated_event(&tap, "https://x/api/data"); // 匹配 → resolve

    let result = waiter.join().expect("waiter must not panic");
    assert_eq!(result["requestId"], "req-7");
    assert_eq!(result["request"]["url"], "https://x/api/data");
    assert_eq!(result["type"], "Document", "translate's real event shape");

    drop(responder);
}

/// memory:// 面:同一 tap 组合在 MemoryCdpBridge 上可达(waitFor* 的
/// 第二生产入口)。
///
/// @trace REQ-CDP-001 [level:integration]
#[test]
fn memory_entry_wait_for_request_resolves_on_translated_event_tap() {
    let (sender, receiver) = bridge_channel(Duration::from_secs(5));
    let (seen_tx, _seen_rx) = mpsc::channel::<String>();
    let responder = spawn_shape_responder(receiver, seen_tx);

    let bridge: Arc<MemoryCdpBridge> = MemoryCdpBridge::new_with_sender("1", sender);
    let tap = bridge
        .event_tap()
        .expect("memory bridge must expose the fallback universe's event tap");

    use bao_cdp_client::transport::in_memory::InMemoryBridge;
    let dispatch_bridge = bridge.clone();
    let waiter = std::thread::spawn(move || {
        dispatch_bridge.dispatch_command("Page.waitForRequest", json!({"url": "**/api/*"}), Some("1"))
    });

    std::thread::sleep(Duration::from_millis(80));
    feed_translated_event(&tap, "https://x/api/data");

    match waiter.join().expect("waiter must not panic") {
        InMemoryBridgeResponse::Ok(v) => {
            assert_eq!(v["requestId"], "req-7");
            assert_eq!(v["request"]["url"], "https://x/api/data");
        }
        InMemoryBridgeResponse::Err(e) => panic!("waitForRequest must resolve, got: {e}"),
    }

    drop(responder);
}

/// waitFor* 超时面:无事件到达 → 有界 Timeout,不悬挂派发线程。
///
/// @trace REQ-CDP-001 [level:integration]
#[test]
fn ws_entry_wait_for_request_times_out_bounded() {
    let (sender, receiver) = bridge_channel(Duration::from_secs(5));
    let (seen_tx, _seen_rx) = mpsc::channel::<String>();
    let responder = spawn_shape_responder(receiver, seen_tx);

    let registry = bao_browser::BaoWsRegistry::new(sender);
    let start = std::time::Instant::now();
    let m = CdpMessage {
        id: None,
        method: "Page.waitForRequest".to_string(),
        params: Some(json!({"timeout": 150})),
        session_id: None,
    };
    let err = registry
        .dispatch_message(&m, "1", &NopSender)
        .expect("dispatch must produce a verdict")
        .expect_err("no event → timeout");
    assert!(
        start.elapsed() < Duration::from_secs(3),
        "timeout must be bounded, took {:?}",
        start.elapsed()
    );
    assert!(
        err.message.contains("Timeout"),
        "timeout verdict shape, got: {}",
        err.message
    );

    drop(responder);
}
