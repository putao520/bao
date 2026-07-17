# 快速开始

5 分钟上手浏览器控制 API。**对外请依赖公共 package `bao`**（`bao_cdp_client` 仅 monorepo 内部实现）。

## 安装

```toml
[dependencies]
# 唯一公共入口（整栈始终链接）
bao = { path = "../bao/src/bao" }
# 或:
# bao = { git = "https://github.com/putao520/bao", package = "bao" }
```

## 最小示例

```rust
use bao::Browser;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let browser = Browser::connect("memory://bao")?;
    println!("Connected: {}", browser);
    Ok(())
}
```

运行:

```sh
cargo run
```

输出:

```text
Connected: Browser(memory://bao, kind=InMemory)
```

## 三种连接模式

### 1. 内嵌 servo(`memory://`)

同进程集成 — servo WebView 与 CDP client 共享 JSContext,零网络往返。

```rust
use bao::Browser;
use std::sync::Arc;

let browser = Browser::connect("memory://bao")?;
// let transport = browser.build_in_memory_transport(my_servo_bridge)?;
```

适用场景:嵌入式浏览器、SSR、自动化测试(零外部依赖)。

### 2. 外部 Chrome — 直连(`ws://`)

```rust
use bao::Browser;

let browser = Browser::connect("ws://127.0.0.1:9222")?;
let mut transport = browser.build_websocket_transport()?;
```

启动 Chrome:

```sh
google-chrome --headless \
              --remote-debugging-port=9222 \
              --disable-gpu \
              --no-sandbox
```

适用场景:与现有 Chrome 自动化栈集成。

### 3. 外部 Chrome — HTTP discover(`http://`)

```rust
use bao::Browser;

// GET /json/version 拿 webSocketDebuggerUrl,自动转 ws://
let browser = Browser::connect("http://127.0.0.1:9222")?;
```

适用场景:不确定 Chrome 端口时的自动发现。

## 典型工作流

```rust
use bao::{Browser, Cookie, ScreenshotFormat};
use bao::Viewport;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let browser = Browser::connect("memory://bao")?;

    // 1. 构造 transport(注入 servo bridge 或 ws 握手)
    // let mut transport = browser.build_in_memory_transport(servo_bridge)?;

    // 2. 发送 CDP 命令
    // let resp = transport.send_command(
    //     "Target.createTarget",
    //     serde_json::json!({ "url": "https://example.com" }),
    //     None,
    // )?;

    // 3. 截图
    // let png = transport.send_command(
    //     "Page.captureScreenshot",
    //     serde_json::json!({ "format": ScreenshotFormat::Png.as_cdp_str() }),
    //     Some(session_id),
    // )?;

    // 4. Cookie 操作
    let cookie = Cookie::new("session", "abc123")
        .with_domain("example.com")
        .with_secure(true);

    // transport.send_command(
    //     "Network.setCookie",
    //     serde_json::to_value(&cookie)?,
    //     None,
    // )?;

    Ok(())
}
```

## 错误处理

```rust
use bao::{Browser, ConnectError, CdpError};

match Browser::connect("ftp://x") {
    Ok(browser) => { /* ... */ },
    Err(ConnectError::InvalidScheme(s)) => {
        eprintln!("Unsupported scheme: {}", s);
    }
    Err(ConnectError::InvalidUrl) => {
        eprintln!("Malformed URL");
    }
    Err(e) => eprintln!("Other error: {}", e),
}
```

## 下一步

- 阅读 [API 概览](./api.md) 了解全部类型
- 阅读 [架构设计](./architecture.md) 了解 URL scheme 路由原理
- 查看 `examples/` 目录的完整工作示例
