//! Example 02 — 通过 CDP 连接到正在运行的 Bao 进程。
//!
//! 本示例假设你已经启动了 Bao 的 CDP Server(通常通过 `bao browser --cdp-port 9222`)。
//! 也可以用 `Browser::connect("memory://bao")` 做同进程内嵌(不开端口、零拷贝)。
//!
//! 这里演示两件事:
//!   1. 用 `ws://` 连接到独立运行的 bao 进程
//!   2. 调用 CDP "Browser.version" + 列出当前 targets(/json/list 等价)
//!
//! 对应的 Node + Playwright 版本见同目录的 `example.js`。

use bao::{Browser, ConnectError};

const WS_URL: &str = "ws://127.0.0.1:9222";

fn main() -> Result<(), ConnectError> {
    println!("[02-playwright] Connecting to {WS_URL} ...");

    // Browser::connect 接受两种 URL:
    //   "memory://bao"          — 进程内 in-memory transport(不开端口)
    //   "ws://host:port"        — 标准 WebSocket CDP(Playwright/Puppeteer 通用)
    //
    // 这里用 ws://,因为本示例演示「外部客户端连 Bao」的场景。
    // 如果你还没启动 Bao,会得到 ConnectError(详见报错提示)。
    let mut browser = Browser::connect(WS_URL)?;
    println!(
        "[02-playwright] Connected. in_memory={}, is_websocket={}",
        browser.is_in_memory(),
        browser.is_websocket()
    );

    // CDP: Browser.version(返回 serde_json::Value)
    let version = browser.version()?;
    println!("[02-playwright] CDP Browser.version = {:?}", version);

    // CDP: 列出所有 target(等价于 GET /json/list)
    let targets = browser.pages()?;
    println!("[02-playwright] Targets on the server:");
    if let Some(arr) = targets.as_array() {
        for t in arr {
            let id = t.get("targetId").or_else(|| t.get("id")).cloned().unwrap_or_default();
            let url = t.get("url").cloned().unwrap_or_default();
            let kind = t.get("type").cloned().unwrap_or_default();
            println!("[02-playwright]   - id={id}, url={url}, type={kind}");
        }
    } else {
        println!("[02-playwright]   (raw) {:?}", targets);
    }

    // 注意:实际页面级自动化(Page.goto / Page.screenshot / Runtime.evaluate)
    // 需要在某个 target 上开 session。完整流程见 bao_cdp_client 的 API 文档。
    // 本示例只演示 browser-level 入口。

    browser.close_connection()?;
    println!("[02-playwright] Done");
    Ok(())
}
