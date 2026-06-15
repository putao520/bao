# bao_cdp_client API 概览

`bao_cdp_client` 是 Bao 项目的统一浏览器控制客户端,基于 CDP (Chrome DevTools Protocol)。
通过 URL scheme 自动路由到内嵌 servo 或外部 Chrome,顶层 API 完全一致。

## 顶层入口

### `Browser::connect(url)` — 唯一入口

```rust
use bao_cdp_client::Browser;

// 同进程 servo(零网络往返)
let browser = Browser::connect("memory://bao")?;

// 外部 Chrome(ws:// / wss:// 直连)
let browser = Browser::connect("ws://127.0.0.1:9222")?;

// HTTP 自动发现 ws endpoint
let browser = Browser::connect("http://127.0.0.1:9222")?;
```

URL scheme 路由规则:

| Scheme               | Transport          | 备注                              |
|----------------------|--------------------|-----------------------------------|
| `memory://...`       | InMemoryTransport  | 同进程 servo,通过 InMemoryBridge 桥接 |
| `ws://` / `wss://`   | WebSocketTransport | 直连外部 Chrome                   |
| `http://` / `https://` | WebSocketTransport | GET /json/version 发现 ws endpoint |

非法 scheme 返回 `ConnectError::InvalidScheme`。

## 公共 API 表面

### 高层 API 类(Playwright 风格)

| 类型               | 模块路径                          | 用途                            |
|--------------------|-----------------------------------|---------------------------------|
| `Browser`          | `bao_cdp_client::Browser`         | URL 路由入口                    |
| `HighLevelBrowser` | `bao_cdp_client::HighLevelBrowser` | 浏览器实例(version/disconnect) |
| `BrowserContext`   | `bao_cdp_client::BrowserContext`  | 隔离上下文(incognito-like)     |
| `Page`             | `bao_cdp_client::Page`            | 一个 tab(顶层 frame)           |
| `Frame`            | `bao_cdp_client::Frame`           | iframe / 主 frame               |
| `ElementHandle`    | `bao_cdp_client::ElementHandle`   | DOM 元素引用                    |
| `JSHandle`         | `bao_cdp_client::JSHandle`        | 任意 JS 对象引用                |
| `Request`          | `bao_cdp_client::Request`         | HTTP 请求                       |
| `Response`         | `bao_cdp_client::Response`        | HTTP 响应                       |
| `Dialog`           | `bao_cdp_client::Dialog`          | alert/prompt/confirm            |
| `ConsoleMessage`   | `bao_cdp_client::ConsoleMessage`  | console.log 等消息              |

### 工具类

| 类型              | 模块路径                          |
|-------------------|-----------------------------------|
| `Keyboard`        | `bao_cdp_client::Keyboard`        |
| `Mouse`           | `bao_cdp_client::Mouse`           |
| `Touchscreen`     | `bao_cdp_client::Touchscreen`     |
| `Coverage`        | `bao_cdp_client::Coverage`        |
| `Tracing`         | `bao_cdp_client::Tracing`         |
| `Accessibility`   | `bao_cdp_client::Accessibility`   |

### 公共类型

```rust
use bao_cdp_client::types::{
    ScreenshotFormat,    // Png | Jpeg | Webp
    WaitUntilState,      // Load | DomContentLoaded | NetworkIdle0 | NetworkIdle2
    Cookie,              // 完整 Cookie 表示(builder API)
    Viewport,            // 视口配置
    DeviceDescriptor,    // 设备模拟(user_agent + viewport)
};
```

### Trait

| Trait           | 用途                                  |
|-----------------|---------------------------------------|
| `Transport`     | 自定义 transport 实现                 |
| `EventEmitter`  | Page/BrowserContext 等共享的事件订阅  |
| `InMemoryBridge`| 自定义 servo 后端                     |
| `ServoBackend`  | servo 操作抽象                        |

### 错误类型

```rust
use bao_cdp_client::{ConnectError, CdpError, BridgeError};

// ConnectError: 连接阶段(URL 解析 / scheme / TCP 握手)
// CdpError:     通信阶段(JSON-RPC / I/O / Timeout)
// BridgeError:  servo RDP 桥接层错误
```

## 命名约定

bao_cdp_client 在 API 表面上有两种 `Browser` 类型,通过别名区分:

- `Browser` — URL 路由入口(`Browser::connect(url)`),用于连接到 CDP 端点
- `HighLevelBrowser` — 高层 API 类(持有 transport + 多 Page 状态),用于操作浏览器

这种命名分离遵循"关注点分离"原则:连接逻辑与操作逻辑分离,便于用户理解。

## 单线程约束

所有高层 API 类(`!Send + !Sync`)与 servo `JSContext` 单线程模型一致。
内部用 `Rc<RefCell<...>>` 共享状态,无需 `Mutex`。
