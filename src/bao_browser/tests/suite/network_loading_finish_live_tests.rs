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
