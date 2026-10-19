//! `Network.loadingFinished` 活体生产者判别锁(REQ-CDP-006 criterion #4;
//! e240 loadingFinished 合同)。
//!
//! 锁定的不变量:**Finish 携 wire(压缩前)字节真值,非 decoded 冒充**。
//!
//! 1. plain(无 Content-Encoding)响应:`encodedDataLength == 发送字节数`
//!    ——wire == decoded 的恒等面。
//! 2. gzip 响应:`encodedDataLength == gzip 字节数 < 解码字节数` ——判别性
//!    断言:若生产者误用解码体长度冒充(wire 值不可得时的伪造形态),此格
//!    必红。
//!
//! 生产链:net tap `BaoNetworkTapEvent::Finish`(http_network_fetch 体读完成
//! 缝,`Decoder::wire_byte_counter` 真值)→ cdp_handler 消费臂 →
//! `ServoEvent::NetworkLoadingFinish` → translate → `Network.loadingFinished`
//! → `Page.waitForEvent("requestfinished")`。
//!
//! 载具形态沿 cdp_run_pump_event_tests:Network.enable(订阅)→ 独立导航线程
//! 300ms 后导航(注册先于事件,零错过竞态)→ waitForEvent 阻塞 → 主线程
//! pump_cdp 驱动泵。gzip 体是二进制流,共享 `HttpFixture`(String body)装
//! 不下,用本文件内最小 raw-bytes fixture。
//!
//! @trace REQ-CDP-006 [level:e2e]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// SW-mediated 第 6 格(第 4 缺口合同)需要 SW script 路由——共享
// common/http_fixture(same shape as serviceworker_mediation_tests)。
#[path = "common/mod.rs"]
mod common;

use bao_browser::{BaoConfig, BrowserRuntime, PageConfig};
use bao_cdp_client::browser::Browser;
use serde_json::{Value, json};

/// memory:// 客户端共享进程全局 bridge 注册表(与 cdp_run_pump_event_tests
/// 同形防御互斥)。
static REGISTRY_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 等待者裁决槽(waitForEvent 的结果,含每步失败)。
type VerdictSlot = Arc<Mutex<Option<Result<Value, String>>>>;

/// Raw-bytes HTTP/1.1 fixture:per-route `(extra headers, body bytes)`。
/// 二进制体形态(gzip 流非 UTF-8),共享 HttpFixture 的 String body 装不下。
/// 最小 accept loop:一连接一请求,`Connection: close`。
struct RawBytesFixture {
    port: u16,
    shutdown: Arc<AtomicBool>,
    /// 每个被请求路径的命中记录(cache-hit 判别:二次加载不得再触网)。
    hits: Arc<Mutex<Vec<String>>>,
}

impl RawBytesFixture {
    fn spawn(
        name: &'static str,
        routes: Vec<(&'static str, Vec<(&'static str, &'static str)>, Vec<u8>)>,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .unwrap_or_else(|e| panic!("bind {name} fixture: {e}"));
        let port = listener.local_addr().unwrap().port();
        let _ = listener.set_nonblocking(true);
        let shutdown = Arc::new(AtomicBool::new(false));
        let hits: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let hits_c = Arc::clone(&hits);
        let shutdown_c = Arc::clone(&shutdown);
        std::thread::Builder::new()
            .name(name.to_string())
            .spawn(move || {
                while !shutdown_c.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((mut tcp, _)) => {
                            let _ = tcp.set_nonblocking(false);
                            let _ = tcp.set_read_timeout(Some(Duration::from_millis(300)));
                            let mut buf = Vec::new();
                            let mut tmp = [0u8; 2048];
                            let deadline = Instant::now() + Duration::from_secs(2);
                            while buf.windows(4).position(|w| w == b"\r\n\r\n").is_none() &&
                                Instant::now() < deadline
                            {
                                match tcp.read(&mut tmp) {
                                    Ok(0) => break,
                                    Ok(n) => buf.extend_from_slice(&tmp[..n]),
                                    Err(_) => break,
                                }
                            }
                            let head = String::from_utf8_lossy(&buf).to_string();
                            let path = head
                                .lines()
                                .next()
                                .and_then(|line| line.split_whitespace().nth(1))
                                .unwrap_or("")
                                .split('?')
                                .next()
                                .unwrap_or("")
                                .to_string();
                            hits_c.lock().unwrap_or_else(|p| p.into_inner()).push(path.clone());
                            let Some((_, headers, body)) =
                                routes.iter().find(|(p, _, _)| *p == path)
                            else {
                                let _ = tcp.write_all(
                                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\
                                      Connection: close\r\n\r\n",
                                );
                                let _ = tcp.shutdown(std::net::Shutdown::Both);
                                continue;
                            };
                            let mut response = format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\n",
                                body.len()
                            );
                            for (k, v) in headers {
                                response.push_str(&format!("{k}: {v}\r\n"));
                            }
                            response.push_str("Connection: close\r\n\r\n");
                            let _ = tcp.write_all(response.as_bytes());
                            let _ = tcp.write_all(body);
                            let _ = tcp.flush();
                            let _ = tcp.shutdown(std::net::Shutdown::Both);
                        },
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        },
                        Err(_) => return,
                    }
                }
            })
            .unwrap_or_else(|e| panic!("spawn {name} fixture thread: {e}"));
        RawBytesFixture { port, shutdown, hits }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }

    /// 某路径被真实触达的网络次数(cache-hit 判别用)。
    fn hit_count(&self, path: &str) -> usize {
        self.hits
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .filter(|p| p.as_str() == path)
            .count()
    }
}

impl Drop for RawBytesFixture {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

/// 等待线程:connect → Network.enable(订阅)→ 独立导航线程 300ms 后导航
/// (注册先于事件)→ `Page.waitForEvent("requestfinished")` 阻塞等
/// loadingFinished。裁决(含每步失败)写入 slot。
fn spawn_finish_waiter(slot: VerdictSlot, nav_url: String) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut browser = match Browser::connect("memory://bao") {
            Ok(b) => b,
            Err(e) => {
                *slot.lock().unwrap() = Some(Err(format!("connect: {e}")));
                return;
            },
        };
        if let Err(e) = browser.send_command("Network.enable", json!({})) {
            *slot.lock().unwrap() = Some(Err(format!("Network.enable: {e}")));
            return;
        }
        // 导航线程:注册(waitForEvent 落 tap)先于导航事件。
        let navigator_url = nav_url.clone();
        let navigator = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            let mut b = match Browser::connect("memory://bao") {
                Ok(b) => b,
                Err(e) => {
                    eprintln!("[loadfin] navigator connect failed: {e}");
                    return;
                },
            };
            if let Err(e) = b.send_command("Page.navigate", json!({ "url": navigator_url })) {
                eprintln!("[loadfin] navigate failed: {e}");
            }
        });
        match browser.send_command(
            "Page.waitForEvent",
            json!({ "event": "requestfinished", "timeout": 20000 }),
        ) {
            Ok(v) => *slot.lock().unwrap() = Some(Ok(v)),
            Err(e) => *slot.lock().unwrap() = Some(Err(format!("waitForEvent: {e}"))),
        }
        let _ = navigator.join();
    })
}

/// 泵直到裁决落地(30s 硬上界),返回 waitForEvent params。
fn pump_to_verdict(runtime: &BrowserRuntime, slot: VerdictSlot) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    let outcome = loop {
        if let Some(outcome) = slot.lock().unwrap().take() {
            break outcome;
        }
        if Instant::now() > deadline {
            panic!("no waitForEvent verdict within the 30s pump window");
        }
        runtime.pump_cdp(Duration::from_millis(10));
    };
    outcome
        .map_err(|e| format!("waitForEvent did not resolve on the pump face: {e}"))
        .expect("waitForEvent must resolve")
}

/// plain(无 Content-Encoding)响应:encodedDataLength == 发送字节数
/// (wire == decoded 恒等面)。
///
/// @trace REQ-CDP-006 [level:e2e]
#[test]
fn plain_body_loading_finished_reports_wire_bytes_equal_to_body_len() {
    let _registry_guard = REGISTRY_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let plain_body = b"<html><body>plain-finish-probe</body></html>".to_vec();
    let wire_len = plain_body.len() as u64;
    let fixture = RawBytesFixture::spawn(
        "loadfin-plain-fixture",
        vec![("/plain", vec![], plain_body)],
    );

    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new");
    let page = runtime
        .create_page(&PageConfig {
            url: None,
            ..Default::default()
        })
        .expect("page");
    let _ = page;

    let slot: VerdictSlot = Arc::new(Mutex::new(None));
    let waiter = spawn_finish_waiter(slot.clone(), fixture.url("/plain"));

    let params = pump_to_verdict(&runtime, slot);
    let _ = waiter.join();

    assert_eq!(
        params["encodedDataLength"],
        json!(wire_len),
        "plain response: wire bytes == body bytes (encodedDataLength must be the \
         true wire total, got {params})"
    );
    assert!(
        params["requestId"].is_string() && !params["requestId"].as_str().unwrap().is_empty(),
        "event params must carry a real requestId, got: {params}"
    );
}

/// gzip 响应:encodedDataLength == gzip 字节数 < 解码字节数 ——判别性断言,
/// 证明是 wire 值非 decoded 冒充(REQ-CDP-006 验收增补)。
///
/// @trace REQ-CDP-006 [level:e2e]
#[test]
fn gzip_body_loading_finished_reports_compressed_wire_bytes_below_decoded() {
    let _registry_guard = REGISTRY_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    // 高度可压缩载荷:4096 字节 'A' → gzip 后远小于原体。decoded 长度由此
    // 精确已知,wire 长度 = 我们发出的 gzip 字节数,两者判别。
    let payload_len: u64 = 4096;
    let decoded_html: Vec<u8> = {
        let mut v = b"<html><body>".to_vec();
        v.extend(std::iter::repeat_n(b'A', payload_len as usize));
        v.extend(b"</body></html>");
        v
    };
    let gzip_body = gzip_bytes(&decoded_html);
    let wire_len = gzip_body.len() as u64;
    assert!(
        wire_len < decoded_html.len() as u64,
        "fixture invariant: gzip({}) must compress below decoded({})",
        wire_len,
        decoded_html.len()
    );

    let fixture = RawBytesFixture::spawn(
        "loadfin-gzip-fixture",
        vec![
            (
                "/gz",
                vec![("Content-Encoding", "gzip")],
                gzip_body,
            ),
        ],
    );

    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new");
    let page = runtime
        .create_page(&PageConfig {
            url: None,
            ..Default::default()
        })
        .expect("page");
    let _ = page;

    let slot: VerdictSlot = Arc::new(Mutex::new(None));
    let waiter = spawn_finish_waiter(slot.clone(), fixture.url("/gz"));

    let params = pump_to_verdict(&runtime, slot);
    let _ = waiter.join();

    assert_eq!(
        params["encodedDataLength"],
        json!(wire_len),
        "gzip response: encodedDataLength must be the compressed wire byte total \
         ({wire_len}); a decoded-body masquerade would report {} (RED = wire \
         counter not wired through the completion seam), got: {params}",
        decoded_html.len()
    );
    assert!(
        (params["encodedDataLength"].as_u64().unwrap_or(u64::MAX)) < payload_len,
        "discriminative assertion: encodedDataLength must be BELOW the decoded \
         size for a compressed response (wire-truth proof), got: {params}"
    );
}

/// gzip 压缩助手(flate2,workspace 已有依赖)。
fn gzip_bytes(payload: &[u8]) -> Vec<u8> {
    use std::io::Write as _;
    let mut encoder =
        flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(payload).expect("gzip write");
    encoder.finish().expect("gzip finish")
}

// ══════════════════════════════════════════════════════════════════════
// 增面判别锁(e-loadfin2 合同):cache 命中 / scheme_fetch / 纯算法步失败
// 三条缺口路径的 Finish/Failed 活体生产者。
// ══════════════════════════════════════════════════════════════════════

/// 两段等待线程:connect → Network.enable → [独立导航线程 300ms 后导航]
/// → 等第一事件(first_event)→ 等第二事件(second_event)→ 两裁决入槽。
/// 第二等待在首事件 resolve 后立刻注册;驱动侧再以延迟 evaluate 发起第二
/// 请求(注册先于事件,同上文导航线程纪律)。
fn spawn_two_stage_waiter(
    first_slot: VerdictSlot,
    second_slot: VerdictSlot,
    nav_url: String,
    first_event: &'static str,
    second_event: &'static str,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut browser = match Browser::connect("memory://bao") {
            Ok(b) => b,
            Err(e) => {
                *first_slot.lock().unwrap() = Some(Err(format!("connect: {e}")));
                return;
            },
        };
        if let Err(e) = browser.send_command("Network.enable", json!({})) {
            *first_slot.lock().unwrap() = Some(Err(format!("Network.enable: {e}")));
            return;
        }
        // 导航线程:注册(waitForEvent 落 tap)先于导航事件。
        let navigator_url = nav_url.clone();
        let navigator = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            let Ok(mut b) = Browser::connect("memory://bao") else {
                eprintln!("[loadfin2] navigator connect failed");
                return;
            };
            if let Err(e) = b.send_command("Page.navigate", json!({ "url": navigator_url })) {
                eprintln!("[loadfin2] navigate failed: {e}");
            }
        });
        let first = match browser.send_command(
            "Page.waitForEvent",
            json!({ "event": first_event, "timeout": 20000 }),
        ) {
            Ok(v) => v,
            Err(e) => {
                *first_slot.lock().unwrap() = Some(Err(format!("waitForEvent#1: {e}")));
                let _ = navigator.join();
                return;
            },
        };
        *first_slot.lock().unwrap() = Some(Ok(first));
        match browser.send_command(
            "Page.waitForEvent",
            json!({ "event": second_event, "timeout": 20000 }),
        ) {
            Ok(v) => *second_slot.lock().unwrap() = Some(Ok(v)),
            Err(e) => *second_slot.lock().unwrap() = Some(Err(format!("waitForEvent#2: {e}"))),
        }
        let _ = navigator.join();
    })
}

/// 泵直到裁决落地(30s 硬上界)。
fn pump_slot(runtime: &BrowserRuntime, slot: &VerdictSlot) -> Value {
    let deadline = Instant::now() + Duration::from_secs(30);
    let outcome = loop {
        if let Some(outcome) = slot.lock().unwrap().take() {
            break outcome;
        }
        if Instant::now() > deadline {
            panic!("no waitForEvent verdict within the 30s pump window");
        }
        runtime.pump_cdp(Duration::from_millis(10));
    };
    outcome
        .map_err(|e| format!("waitForEvent did not resolve on the pump face: {e}"))
        .expect("waitForEvent must resolve")
}

/// 延迟 evaluate 线程:等 waiter 侧第二等待注册完成(500ms 注册窗,同
/// 上文 300ms 导航线程纪律)后,经独立连接在页面里发起第二请求。
fn spawn_delayed_evaluate(expr: String) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        let Ok(mut b) = Browser::connect("memory://bao") else {
            eprintln!("[loadfin2] evaluator connect failed");
            return;
        };
        if let Err(e) = b.send_command(
            "Runtime.evaluate",
            json!({ "expression": expr, "returnByValue": true }),
        ) {
            eprintln!("[loadfin2] evaluate failed: {e}");
        }
    })
}

/// 最小文档:无子资源,保证 requestfinished #1 就是文档本体。
fn minimal_page(title: &str) -> Vec<u8> {
    format!("<html><head><title>{title}</title></head><body>loadfin2</body></html>").into_bytes()
}

/// cache 命中(max-age 新鲜):第二次同 URL 加载不得触网,且其
/// loadingFinished 携缓存体字节总量——值来源=缓存体长度,诚实非伪造。
///
/// @trace REQ-CDP-006 [level:e2e]
#[test]
fn cache_hit_loading_finished_reports_cached_body_bytes_without_network() {
    let _registry_guard = REGISTRY_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let cache_body = minimal_page("cache-hit-probe");
    let body_len = cache_body.len() as u64;
    let fixture = RawBytesFixture::spawn(
        "loadfin2-cache-fixture",
        vec![
            (
                "/cache",
                vec![("Cache-Control", "public, max-age=60")],
                cache_body,
            ),
        ],
    );

    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new");
    let page = runtime
        .create_page(&PageConfig {
            url: None,
            ..Default::default()
        })
        .expect("page");
    let _ = page;

    let first: VerdictSlot = Arc::new(Mutex::new(None));
    let second: VerdictSlot = Arc::new(Mutex::new(None));
    let waiter = spawn_two_stage_waiter(
        Arc::clone(&first),
        Arc::clone(&second),
        fixture.url("/cache"),
        "requestfinished",
        "requestfinished",
    );

    // 第一格:文档本体 Finish(wire == body 恒等面)。
    let doc_finish = pump_slot(&runtime, &first);
    assert_eq!(
        doc_finish["encodedDataLength"],
        json!(body_len),
        "document load must report the true body byte total, got: {doc_finish}"
    );

    // 第二格:同 URL 的 fetch() 由缓存回放——Finish 携缓存体长度,
    // 且 fixture 不得再有 /cache 连接(未触网=cache-hit 判别)。
    let evaluator = spawn_delayed_evaluate("fetch(location.href).then(r => r.text())".into());
    let cache_finish = pump_slot(&runtime, &second);
    let _ = evaluator.join();
    let _ = waiter.join();

    assert_eq!(
        cache_finish["encodedDataLength"],
        json!(body_len),
        "cache-hit replay must report the cached body byte total (value source = \
         cached body length, honest non-fabricated), got: {cache_finish}"
    );
    assert_eq!(
        fixture.hit_count("/cache"),
        1,
        "the second load of /cache must be served from cache (exactly one network \
         hit for the document load); a second hit means the cache path was not \
         exercised and this test proved nothing"
    );
}

/// scheme_fetch(blob:)响应 Finish:无网络腿(blob URL 永不触网),终态
/// 事件携进程内载荷字节总量。
///
/// @trace REQ-CDP-006 [level:e2e]
#[test]
fn scheme_fetch_blob_loading_finished_reports_payload_bytes() {
    let _registry_guard = REGISTRY_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    const BLOB_PAYLOAD: &str = "hello-blob-payload";
    let fixture = RawBytesFixture::spawn(
        "loadfin2-blob-fixture",
        vec![("/blobpage", vec![], minimal_page("blob-probe"))],
    );

    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new");
    let page = runtime
        .create_page(&PageConfig {
            url: None,
            ..Default::default()
        })
        .expect("page");
    let _ = page;

    let first: VerdictSlot = Arc::new(Mutex::new(None));
    let second: VerdictSlot = Arc::new(Mutex::new(None));
    let waiter = spawn_two_stage_waiter(
        Arc::clone(&first),
        Arc::clone(&second),
        fixture.url("/blobpage"),
        "requestfinished",
        "requestfinished",
    );

    let _doc = pump_slot(&runtime, &first);

    let evaluator = spawn_delayed_evaluate(format!(
        "fetch(URL.createObjectURL(new Blob([{BLOB_PAYLOAD:?}])))\
         .then(r => r.text())"
    ));
    let blob_finish = pump_slot(&runtime, &second);
    let _ = evaluator.join();
    let _ = waiter.join();

    assert_eq!(
        blob_finish["encodedDataLength"],
        json!(BLOB_PAYLOAD.len() as u64),
        "blob: scheme response must report its actual payload byte total \
         (no network leg, in-process size), got: {blob_finish}"
    );
    assert_eq!(
        fixture.hit_count("/blobpage"),
        1,
        "the blob fetch must never touch the network (only the document load \
         hits the fixture)"
    );
}

/// 纯算法步失败(CORS check):响应已到达但被算法步判死 →
/// Network.loadingFailed(canceled=false,errorText=CorsGeneral)。
///
/// @trace REQ-CDP-006 [level:e2e]
#[test]
fn cors_algorithm_failure_emits_loading_failed() {
    let _registry_guard = REGISTRY_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    // 跨源 = 第二 fixture(不同端口)。/blocked 响应 200 且不带 ACAO ——
    // CORS check 必败(算法步,非网络错误)。
    let cross = RawBytesFixture::spawn(
        "loadfin2-cors-cross-fixture",
        vec![("/blocked", vec![], minimal_page("cors-blocked"))],
    );
    let fixture = RawBytesFixture::spawn(
        "loadfin2-cors-page-fixture",
        vec![("/corspage", vec![], minimal_page("cors-page"))],
    );

    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new");
    let page = runtime
        .create_page(&PageConfig {
            url: None,
            ..Default::default()
        })
        .expect("page");
    let _ = page;

    let first: VerdictSlot = Arc::new(Mutex::new(None));
    let second: VerdictSlot = Arc::new(Mutex::new(None));
    let waiter = spawn_two_stage_waiter(
        Arc::clone(&first),
        Arc::clone(&second),
        fixture.url("/corspage"),
        "requestfinished",
        "requestfailed",
    );

    let _doc = pump_slot(&runtime, &first);

    let cross_url = cross.url("/blocked");
    let evaluator = spawn_delayed_evaluate(format!(
        "fetch({cross_url:?}).then(r => r.text()).catch(() => 'rejected')"
    ));
    let failed = pump_slot(&runtime, &second);
    let _ = evaluator.join();
    let _ = waiter.join();

    let error_text = failed["errorText"].as_str().unwrap_or_default();
    assert!(
        error_text.contains("CorsGeneral"),
        "CORS algorithm failure must surface as loadingFailed with the true \
         error (CorsGeneral), got: {failed}"
    );
    assert_eq!(
        failed["canceled"], json!(false),
        "a CORS check failure is a policy failure, not a cancellation, got: {failed}"
    );
    assert!(
        failed["requestId"].as_str().is_some_and(|id| !id.is_empty()),
        "failed event must carry a real requestId, got: {failed}"
    );
    assert_eq!(
        cross.hit_count("/blocked"),
        1,
        "the cross-origin response must have actually arrived (the failure is \
         the CORS algorithm step, not a network error)"
    );
}

// ══════════════════════════════════════════════════════════════════════
// 第 4 缺口判别锁(e-loadfin2 登记的第 4 缺口):SW-mediated fetch 的
// Finish 活体生产者——SW 介入的 fetch 既不经网络缝也不经 scheme 缝,终态
// 事件此前永缺,requestfinished 永挂。
// ══════════════════════════════════════════════════════════════════════

/// SW 体与原生体刻意不同长——Finish 的 encodedDataLength 携谁的字节是
/// 本格判别轴(SW 体=终态来自 SW 面;原生体长度=缝误接网络路径)。
const SW_FINISH_BODY: &str = "SW_TERMINAL_FINISH_BODY";
const NATIVE_SWFIN_BODY: &str = "NATIVE_SWFIN_MUST_NOT_BE_REPORTED_X";

/// SW 脚本:拦 scope(/api/)内 /api/swfin,respondWith 定制响应。其余
/// 不应答(mediator None → 网络路径自理,零双发面)。
const SW_FINISH_JS: &str = r#"
self.addEventListener('fetch', function (e) {
  var u = String(e.request.url);
  if (u.indexOf('/api/swfin') !== -1) {
    e.respondWith(new Response('SW_TERMINAL_FINISH_BODY', {
      status: 200,
      headers: { 'Content-Type': 'text/plain', 'X-Sw-Finish': 'yes' }
    }));
  }
});
"#;

/// 轮询 SW 注册到 active(触发前门;未激活即发探测会走网络路径假红)。
/// scope 用默认 `/`(同 serviceworker_mediation_tests):非导航请求(XHR)
/// 只在 client 被 SW 控制时才经 mediation——scope 收窄到 /api/ 而页面在 /
/// 时页面不受控,拦截恒不 engage(首跑实测:ACTIVE 了仍穿透到网络)。
/// 写与读必须分离——evaluate 的同步返回发生在 promise 落定前,每轮重置再
/// 读会永远读到本轮 pending(same shape as __swReg 轮询)。getRegistration
/// 轮询零网络请求,不污染事件面。
fn wait_for_sw_active(page: &bao_browser::PageHandle, origin: &str) {
    let register_js = "window.__swReg = 'pending'; try { \
         navigator.serviceWorker.register('/sw.js').then( \
           function () { window.__swReg = 'ok'; }, \
           function (e) { window.__swReg = 'error:' + e; }); \
       } catch (err) { window.__swReg = 'threw:' + err; } \
       window.__swReg";
    let initial = page
        .evaluate_js_web(register_js)
        .expect("sw register dispatch must not fail");
    eprintln!("[loadfin-sw] register dispatched, immediate eval = {initial:?}");

    let deadline = Instant::now() + Duration::from_secs(45);
    let mut reg_outcome = String::new();
    loop {
        reg_outcome = page.evaluate_js_web("window.__swReg").unwrap_or_default();
        if !reg_outcome.contains("pending") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "sw register promise never settled within 45s (last: {reg_outcome})"
        );
        std::thread::sleep(Duration::from_millis(300));
    }
    eprintln!("[loadfin-sw] register outcome = {reg_outcome}");
    assert!(
        reg_outcome.contains("ok"),
        "serviceWorker.register must resolve on the live path, got: {reg_outcome}"
    );

    // 激活轮询:arm 一次(getRegistration().then 写 __swAct),隔窗只读;
    // NOTYET(注册已见、active 未就绪)则下一轮再 arm。
    let arm_js = "window.__swAct = 'pending'; \
         navigator.serviceWorker.getRegistration().then( \
           function (r) { window.__swAct = (r && r.active) ? 'ACTIVE' : 'NOTYET'; }, \
           function (e) { window.__swAct = 'error:' + e; }); \
         'armed'";
    let read_js = "window.__swAct";
    let mut last = String::new();
    loop {
        let _ = page.evaluate_js_web(arm_js);
        std::thread::sleep(Duration::from_millis(500));
        last = page.evaluate_js_web(read_js).unwrap_or_default();
        if last.contains("ACTIVE") {
            eprintln!("[loadfin-sw] sw ACTIVE in / scope (origin {origin})");
            return;
        }
        assert!(
            Instant::now() < deadline,
            "sw never reached ACTIVE within 45s (last poll: {last})"
        );
    }
}

/// SW-mediated fetch Finish:SW respondWith 的响应经 net 层定妆
/// (CustomResponse.body → ResponseBody::Done)但既不经网络缝也不经 scheme
/// 缝——终态事件由 SW 采纳点发射,携 SW 体字节真值。判别三轴:
/// ① encodedDataLength == SW 体长度(≠ 原生体长度,缝未误接网络);
/// ② fixture 上 /api/swfin 零原生命中(拦截确成立,字节不可能来自网络);
/// ③ 第一格(同页原生文档导航)照常携原生体长度——非 SW 路径零回归。
///
/// @trace REQ-CDP-006 [level:e2e]
#[test]
fn sw_mediated_fetch_loading_finished_reports_sw_body_bytes() {
    let _registry_guard = REGISTRY_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let doc_body = minimal_page("sw-finish-probe");
    let doc_len = doc_body.len() as u64;
    let fixture = common::http_fixture::HttpFixture::spawn(
        "loadfin-sw-fixture",
        Arc::new(move |path: &str, script: Option<&str>| -> (&'static str, String) {
            if path.starts_with("/sw.js") {
                match script {
                    Some(script) => ("application/javascript", script.to_string()),
                    None => ("text/plain", "sw script not set".into()),
                }
            } else if path.starts_with("/api/swfin") {
                ("text/plain", NATIVE_SWFIN_BODY.into())
            } else {
                ("text/html", String::from_utf8(doc_body.clone()).unwrap_or_default())
            }
        }),
    );
    fixture.set_script(SW_FINISH_JS.to_owned());
    let origin = fixture.origin();

    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new");
    let page = runtime
        .create_page(&PageConfig {
            url: Some(origin.clone()),
            ..Default::default()
        })
        .expect("page");

    // 注册并等 SW 激活(此时 load#1 的 Finish 无人订阅;激活后导航的探测
    // 才是真 SW 面)。scope 限 /api/,后续文档导航在 scope 外零拦截。
    wait_for_sw_active(&page, &origin);

    let first: VerdictSlot = Arc::new(Mutex::new(None));
    let second: VerdictSlot = Arc::new(Mutex::new(None));
    // 导航 URL 带 query 绕启发式缓存;页面在 SW scope 外 → 第一格 Finish
    // 携原生文档字节(非 SW 路径零回归面)。
    let waiter = spawn_two_stage_waiter(
        Arc::clone(&first),
        Arc::clone(&second),
        fixture.url("/swpage?nav=2"),
        "requestfinished",
        "requestfinished",
    );

    let doc_finish = pump_slot(&runtime, &first);
    assert_eq!(
        doc_finish["encodedDataLength"],
        json!(doc_len),
        "document load (outside the SW scope) must still report the native body \
         byte total — the SW seam must not perturb the plain path, got: {doc_finish}"
    );

    let swfin_url = fixture.url("/api/swfin");
    let evaluator = spawn_delayed_evaluate(format!(
        "var __x = new XMLHttpRequest(); \
         __x.open('GET', '{swfin_url}', false); \
         __x.send(null); \
         'status=' + __x.status + ' body=' + __x.responseText"
    ));
    let sw_finish = pump_slot(&runtime, &second);
    let _ = evaluator.join();
    let _ = waiter.join();

    assert_eq!(
        sw_finish["encodedDataLength"],
        json!(SW_FINISH_BODY.len() as u64),
        "the SW-mediated response's terminal event must carry the SW-produced \
         body byte total ({}); the native length ({} would mean the seam read \
         the network path instead of the settled SW body), got: {sw_finish}",
        SW_FINISH_BODY.len(),
        NATIVE_SWFIN_BODY.len()
    );
    assert_ne!(
        SW_FINISH_BODY.len(),
        NATIVE_SWFIN_BODY.len(),
        "fixture invariant: SW and native bodies must differ in length for the \
         byte-source discriminant to mean anything"
    );
    assert!(
        !fixture.saw("/api/swfin"),
        "the mediated request must never touch the fixture (SW interception is \
         the only body source); a native hit means interception did not engage \
         and this test proved nothing"
    );
}

// ══════════════════════════════════════════════════════════════════════
// 字节级中间事件 + data: 载具链判别锁(REQ-CDP-006 dataReceived/data: 链
// 合同):Network.dataReceived 每 chunk 送达边 + data:-URL 导航四事件全链
// (Chrome oracle 实测序列同形)。
// ══════════════════════════════════════════════════════════════════════

/// plain(无 Content-Encoding)HTTP 响应的 `Network.dataReceived`:
/// `dataLength == encodedDataLength == 发送字节数`(plain 面 wire == decoded
/// 的恒等面,同 loadingFinished 的判别律)。
///
/// @trace REQ-CDP-006 [level:e2e]
#[test]
fn plain_body_data_received_reports_chunk_bytes_equal_to_body_len() {
    let _registry_guard = REGISTRY_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let plain_body = minimal_page("datarecv-plain-probe");
    let wire_len = plain_body.len() as u64;
    let fixture = RawBytesFixture::spawn(
        "datarecv-plain-fixture",
        vec![("/drplain", vec![], plain_body)],
    );

    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new");
    let page = runtime
        .create_page(&PageConfig {
            url: None,
            ..Default::default()
        })
        .expect("page");
    let _ = page;

    let slot: VerdictSlot = Arc::new(Mutex::new(None));
    let waiter = spawn_finish_waiter_for_event(slot.clone(), fixture.url("/drplain"), "Network.dataReceived");

    let params = pump_to_verdict(&runtime, slot);
    let _ = waiter.join();

    assert_eq!(
        params["dataLength"],
        json!(wire_len),
        "plain response: dataLength must be the decoded chunk total == body \
         bytes, got: {params}"
    );
    assert_eq!(
        params["encodedDataLength"],
        json!(wire_len),
        "plain response: encodedDataLength (wire-counter delta) must equal the \
         body bytes when there is no transfer encoding, got: {params}"
    );
    assert!(
        params["requestId"].is_string() && !params["requestId"].as_str().unwrap().is_empty(),
        "dataReceived must carry the request's real requestId, got: {params}"
    );
}

/// data:-URL 导航全链:四事件(requestWillBeSent → responseReceived →
/// dataReceived → loadingFinished)逐一送达,且字节真值跨面一致。
/// Chrome oracle 对 data: 导航实测同序列(修前 bao 对 data: 零 request/
/// terminal 事件——本锁即该缺口的回归钉)。
///
/// 收集形态=四阶段顺序等待,每阶段先注册等待再导航(300ms 导航线程纪律),
/// 该阶段事件必来自本阶段导航(前一导航的同名事件已在此之前流过,未注册
/// 的等待器不接收 tap one-shot 交付)。每阶段 payload 字节数不同,
/// dataReceived.dataLength / loadingFinished.encodedDataLength 与本阶段
/// payload 长度逐格相等,即字节真值贯通全链的跨面绑定证明。
/// (Network.enable 是进程级单 tap last-enable-wins,多连接并行等待会互
/// 相饿死;memory:// 的 recv_event 队列仅 screencast 面 direct push,
/// Network 事件不经它——顺序单等待是唯一无竞态面。)
///
/// @trace REQ-CDP-006 [level:e2e]
#[test]
fn data_url_navigation_announces_full_network_event_chain() {
    let _registry_guard = REGISTRY_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let runtime = BrowserRuntime::new(BaoConfig::default()).expect("BrowserRuntime::new");
    let page = runtime
        .create_page(&PageConfig {
            url: None,
            ..Default::default()
        })
        .expect("page");
    let _ = page;

    // 每阶段独立 payload(填充位数各异 → 字节数各异)= 该阶段导航的字节
    // 指纹;阶段断言以本阶段字节数判别,防跨阶段事件串扰假绿。
    let stage_payload =
        |n: usize| format!("<html><body>{}data-chain</body></html>", "x".repeat(n * 7));
    let stage_url = |n: usize| format!("data:text/html,{}", stage_payload(n));

    // 阶段 runner:注册等待(event,裸 CDP method 或 Playwright 名)→
    // 300ms 后导航本阶段 URL → 泵至裁决。
    let run_stage = |slot: &VerdictSlot, event: &'static str, url: String| {
        let waiter = spawn_finish_waiter_for_event(Arc::clone(slot), url, event);
        let deadline = Instant::now() + Duration::from_secs(30);
        let outcome = loop {
            if let Some(outcome) = slot.lock().unwrap().take() {
                break outcome;
            }
            assert!(
                Instant::now() < deadline,
                "stage event {event} never resolved within the 30s pump window"
            );
            runtime.pump_cdp(Duration::from_millis(10));
        };
        let _ = waiter.join();
        outcome
            .map_err(|e| format!("waitForEvent({event}) did not resolve: {e}"))
            .expect("chain stage must resolve")
    };

    let stage = |n: usize| (stage_payload(n).len() as u64, stage_url(n));

    // 阶段 1:requestWillBeSent — data: URL 本体入请求面(修前零发射格)。
    let slot: VerdictSlot = Arc::new(Mutex::new(None));
    let (len1, url1) = stage(1);
    let req = run_stage(&slot, "Network.requestWillBeSent", url1);
    assert!(
        req["request"]["url"].as_str().unwrap_or("").starts_with("data:"),
        "requestWillBeSent must announce the data: URL itself, got: {req}"
    );
    assert_eq!(
        req["request"]["method"], "GET",
        "data: navigation is a GET, got: {req}"
    );

    // 阶段 2:responseReceived — 状态 200 面同步到达。
    let slot: VerdictSlot = Arc::new(Mutex::new(None));
    let (len2, url2) = stage(2);
    let resp = run_stage(&slot, "Network.responseReceived", url2);
    assert_eq!(
        resp["response"]["status"], 200,
        "data: response is 200, got: {resp}"
    );
    assert!(
        resp["requestId"].as_str().unwrap_or("").len() > 0,
        "responseReceived carries the navigation requestId, got: {resp}"
    );

    // 阶段 3:dataReceived — dataLength == encodedDataLength == 本阶段
    // payload 字节数(inline 体无 transfer encoding,wire 即 payload)。
    let slot: VerdictSlot = Arc::new(Mutex::new(None));
    let (len3, url3) = stage(3);
    let data = run_stage(&slot, "Network.dataReceived", url3);
    assert_eq!(
        data["dataLength"],
        json!(len3),
        "dataReceived.dataLength must equal THIS navigation's payload bytes, got: {data}"
    );
    assert_eq!(
        data["encodedDataLength"],
        json!(len3),
        "dataReceived.encodedDataLength must equal the payload (inline data: \
         body has no transfer encoding), got: {data}"
    );

    // 阶段 4:loadingFinished — scheme 终态缝携同导航体字节真值。
    let slot: VerdictSlot = Arc::new(Mutex::new(None));
    let (len4, url4) = stage(4);
    let fin = run_stage(&slot, "requestfinished", url4);
    assert_eq!(
        fin["encodedDataLength"],
        json!(len4),
        "loadingFinished must carry the data: body byte total (scheme terminal \
         seam), got: {fin}"
    );
}
/// 事件名参数化的 finish 等待线程(同 spawn_finish_waiter 形态;`event`
/// 接受裸 CDP method 或 Playwright 名)。
fn spawn_finish_waiter_for_event(
    slot: VerdictSlot,
    nav_url: String,
    event: &'static str,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut browser = match Browser::connect("memory://bao") {
            Ok(b) => b,
            Err(e) => {
                *slot.lock().unwrap() = Some(Err(format!("connect: {e}")));
                return;
            },
        };
        if let Err(e) = browser.send_command("Network.enable", json!({})) {
            *slot.lock().unwrap() = Some(Err(format!("Network.enable: {e}")));
            return;
        }
        // 导航线程:注册(waitForEvent 落 tap)先于导航事件。
        let navigator_url = nav_url.clone();
        let navigator = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            let Ok(mut b) = Browser::connect("memory://bao") else {
                eprintln!("[datarecv] navigator connect failed");
                return;
            };
            if let Err(e) = b.send_command("Page.navigate", json!({ "url": navigator_url })) {
                eprintln!("[datarecv] navigate failed: {e}");
            }
        });
        match browser.send_command("Page.waitForEvent", json!({ "event": event, "timeout": 20000 })) {
            Ok(v) => *slot.lock().unwrap() = Some(Ok(v)),
            Err(e) => *slot.lock().unwrap() = Some(Err(format!("waitForEvent({event}): {e}"))),
        }
        let _ = navigator.join();
    })
}
