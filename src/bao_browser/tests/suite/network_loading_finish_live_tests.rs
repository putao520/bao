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
        RawBytesFixture { port, shutdown }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
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
