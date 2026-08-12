# Example 02 — Playwright / Puppeteer 连接 Bao(CDP 自动化)

展示 Bao 内置 CDP Server,Playwright / Puppeteer **无需改造**直连。
**背后不是 Chrome,是 servo** —— 但 CDP 协议层完全兼容。

## 架构

```
┌───────────────────┐   CDP/WS (ws://127.0.0.1:9222)   ┌──────────────────────┐
│  Node.js + PW     │ <------------------------------- │  Bao Process         │
│  connectOverCDP   │                                  │  (servo + SM + CDP)   │
└───────────────────┘                                  └──────────────────────┘
```

## 运行方式(两步)

### 步骤 1:启动 Bao CDP Server(独立终端)

```bash
# 在项目根目录
cargo run -p bao_bin -- browser --cdp-port 9222
# 或者:
bao browser --cdp-port 9222
```

### 步骤 2:运行示例

- **Rust 客户端**:`cargo run`(在 `examples/02-playwright/` 内)— 用 `Browser::connect("ws://127.0.0.1:9222")`
- **Node 客户端**:`node example.js`(需 `npm i playwright`)— 用 Playwright 的 `chromium.connectOverCDP`

## 预期输出(Rust 端)

```
[02-playwright] Connecting to ws://127.0.0.1:9222 ...
[02-playwright] Connected. transport_kind=WebSocket, in_memory=false
[02-playwright] CDP Browser.version = {"protocolVersion":"1.3","product":"Bao/Servo",...}
[02-playwright] Targets on the server:
[02-playwright]   - { "id": "...", "url": "about:blank", "type": "page" }
[02-playwright] Done
```

## 预期输出(Node + Playwright)

```
[02-playwright] Connected to Bao over CDP
[02-playwright] Page title: Example Domain
[02-playwright] User agent: Mozilla/5.0 ... servo/...
[02-playwright] Screenshot saved: bao-02-playwright.png
```

## 核心 API 调用(Rust)

```rust
// In-memory transport(零端口、同进程,适合测试)
let browser = Browser::connect("memory://bao")?;

// WebSocket transport(标准 CDP 端口,适合 Playwright/Puppeteer 外部客户端)
let browser = Browser::connect("ws://127.0.0.1:9222")?;

let ver = browser.version()?;           // CDP "Browser.version"
let targets = browser.pages()?;         // CDP "Target.getTargets" / "/json/list"
let new_page = browser.new_page(url)?;  // CDP "Target.createTarget"
```

## 关键点

- **`memory://bao`** 走进程内 in-memory transport,不开端口、零拷贝,适合测试
- **`ws://...`** 走标准 WebSocket CDP,任何兼容 CDP 的工具(Playwright/Puppeteer/DevTools)都能连
- Bao 的 CDP Server 实现 12 个 domain(Runtime/Page/Target/Network/...),见根目录 SPEC `06-CDP-SERVER.html`
- Playwright 会以为它连的是 Chrome,但实际渲染引擎是 servo(用户代理里会有 `servo` 字样,可以用 stealth 配置改写)
