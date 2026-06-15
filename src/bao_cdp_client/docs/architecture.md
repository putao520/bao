# 架构设计

bao_cdp_client 的核心设计是 **URL scheme 路由 + Transport 抽象 + servo RDP 桥接**。

## 总体架构

```text
                ┌──────────────────────────────────────────┐
                │         Browser::connect(url)            │
                │            (URL 解析 + scheme 路由)      │
                └────────────────┬─────────────────────────┘
                                 │
        ┌────────────────────────┼────────────────────────┐
        │                        │                        │
        ▼                        ▼                        ▼
   memory://                ws:// / wss://          http:// / https://
        │                        │                        │
        ▼                        ▼                        ▼
  InMemoryTransport      WebSocketTransport        GET /json/version
        │                        │                        │
        │                        └─────────┬──────────────┘
        │                                  │ (auto-discover)
        │                                  ▼
        │                          WebSocketTransport
        │                                  │
        ▼                                  ▼
  ┌─────────────────┐              ┌─────────────────┐
  │  InMemoryBridge │              │  std::net::Tcp  │
  │   (servo RDP)   │              │  + RFC 6455 WS  │
  └─────────────────┘              └─────────────────┘
        │                                  │
        ▼                                  ▼
  ┌─────────────────┐              ┌─────────────────┐
  │  CDPRdpBridge   │              │  外部 Chrome    │
  │ servo ScriptThd │              │  (CDP 端点)     │
  └─────────────────┘              └─────────────────┘
```

## 层次划分

### 第 1 层:URL scheme 路由(DEC-URL-001)

`Browser::connect(url)` 通过 URL scheme 决定 transport 类型,与具体实现解耦:

| Scheme             | 路由分支              | Transport 类型          |
|--------------------|-----------------------|-------------------------|
| `memory://`        | `connect_in_memory`   | InMemoryTransport       |
| `ws://` / `wss://` | `connect_ws`          | WebSocketTransport      |
| `http://` / `https://` | `connect_http_discover` | WebSocketTransport (自动发现 ws endpoint) |

非法 scheme 返回 `ConnectError::InvalidScheme`,空 URL 返回 `InvalidUrl`。

**为什么用 URL scheme 而不是 enum?**
- 与 Puppeteer / Playwright 的 `connect(url)` 兼容
- 字符串 URL 易于通过 env / config / CLI 传递
- HTTP discover 流程对调用方透明

### 第 2 层:Transport 抽象(REQ-BAO-API-002)

`Transport` trait 抽象三种操作:

```rust
pub trait Transport: Send {
    fn kind(&self) -> TransportKind;
    fn send_command(&mut self, method: &str, params: Value, session_id: Option<&str>) -> Result<Value>;
    fn recv_event(&mut self) -> Result<Option<CdpEvent>>;
    fn close(&mut self) -> Result<()>;
    fn set_command_timeout(&mut self, _timeout: Duration) {}
    fn set_event_timeout(&mut self, _timeout: Duration) {}
}
```

**设计要点**:
- **零 tokio**:同步阻塞 I/O,由调用方决定调度策略(bun_event_loop / std::thread)
- **Send + Sync**:可跨线程持有,虽然 InMemory 模式下底层 channel 是 !Send 友好的
- **错误码分离**:命令错误(CdpError)、连接错误(ConnectError)、桥接错误(BridgeError)

### 第 3 层:InMemoryTransport + Bridge

```text
  CDP Client              InMemoryTransport
      │                          │
      │ send_command             │
      ├─────────────────────────►│
      │                          │
      │                          ▼
      │                  InMemoryBridge trait
      │                          │
      │                          ▼
      │                  CDPRdpBridge (默认实现)
      │                          │
      │                  ┌───────┴────────┐
      │                  │                │
      │                  ▼                ▼
      │            ServoBackend      eval_synthesizer
      │            (A 类 48 method)   (B 类 52 method)
      │                  │                │
      │                  ▼                ▼
      │            PagePool /        IIFE Eval
      │            servo WebView     + JSON.stringify
      │                          │
      │  recv_event               │
      │◄──────────────────────────┤
                              servo 7 类事件
                              → translate()
                              → CdpEvent
```

**Bridge 模式优势**:
- 解耦 CDP 协议与 servo ScriptThread 实现
- 允许用户实现自定义 Bridge(如远端 servo / mock / record-replay)
- A/B/E 类 method 分发清晰(A=servo 直接支持,B=JS eval 合成,E=未支持 -32601)

### 第 4 层:servo RDP 桥接(REQ-BAO-API-004/005)

CDPRdpBridge 把 CDP JSON-RPC 命令分发到三类 handler:

| 类别 | 数量 | 策略                           |
|------|------|--------------------------------|
| A 类 | 48   | servo ServoBackend trait 直接映射 |
| B 类 | 52   | IIFE Eval 合成(JS 注入)        |
| E 类 | 31+  | 返回 -32601 NotSupported       |

### 第 5 层:高层 API(REQ-BAO-API-006)

Playwright 风格的 Browser/Context/Page/Frame/ElementHandle 等:
- 共享 EventEmitter trait(`on/once/off/emit`)
- 单线程约束(`!Send + !Sync`),用 `Rc<RefCell<...>>` 共享
- 与 servo JSContext 寄生模型一致(DEC-JSC-001)

## 数据流示例:Page.navigate

### InMemory 模式

```text
  Page.goto(url)
     │
     ▼
  InMemoryTransport::send_command("Page.navigate", {"url": url})
     │
     ▼
  InMemoryBridge::dispatch_command
     │
     ▼
  CDPRdpBridge::dispatch_command
     │
     ▼
  command_dispatcher::dispatch_command (match "Page.navigate")
     │
     ▼
  A 类 handler → ServoBackend::page_navigate(target_id, url)
     │
     ▼
  servo WebView.load_url(url)
     │
     ▼
  PageLifecycleEvent(LOAD) → translate() → CdpEvent::new("Page.lifecycleEvent")
     │
     ▼
  EventSubscriber → channel → InMemoryTransport::recv_event
```

### WebSocket 模式

```text
  Page.goto(url)
     │
     ▼
  WebSocketTransport::send_command("Page.navigate", {"url": url})
     │
     ▼
  JSON-RPC 编码 → TcpStream write
     │
     ▼
  外部 Chrome 接收 → Page.navigate → frame 加载
     │
     ▼
  Chrome 推送 Page.frameNavigated / Page.lifecycleEvent
     │
     ▼
  TcpStream read → JSON-RPC 解码 → CdpEvent
     │
     ▼
  WebSocketTransport::recv_event
```

## servo 事件 → CDP event 翻译表

完整映射 7 类 servo 事件到 CDP event:

| servo 原始事件              | CDP event method               |
|----------------------------|--------------------------------|
| `ConsoleMessage`           | `Runtime.consoleAPICalled`     |
| `PageLifecycleEvent`       | `Page.lifecycleEvent`          |
| `NetworkRequest`           | `Network.requestWillBeSent`    |
| `NetworkResponse`          | `Network.responseReceived`     |
| `Dialog`                   | `Page.javascriptDialogOpening` |
| `FileChooser`              | `Page.fileChooserOpened`       |
| `PageError`                | `Runtime.exceptionThrown`      |

详细实现见 `bridge/event_translator.rs`。

## 错误码层次

```text
ConnectError (连接阶段)
├── InvalidUrl          (空 URL / 缺 :// / 非 UTF-8)
├── InvalidScheme       (ftp / file / javascript / ...)
├── LaunchError         (Chrome 二进制不存在)
├── ConnectionFailed    (TCP 拒绝 / WS 握手失败)
└── Timeout             (超时)

CdpError (通信阶段)
├── ProtocolError       (JSON-RPC error / -32601)
├── JsonError           (序列化失败)
├── IoError             (I/O 错误)
├── ConnectionClosed    (Transport 关闭)
├── Timeout             (命令超时)
├── TransportError      (Transport 内部错误)
└── HandshakeError      (WS 握手失败)

BridgeError (servo 桥接层)
├── NotSupported        (-32601,E 类 method)
├── InvalidParams       (-32600,参数错误)
├── InternalError       (-32603,servo 内部错误)
└── NoSuchTarget        (target_id 不存在)
```

## 设计原则总结

1. **URL scheme 路由**:与 Puppeteer / Playwright 兼容
2. **Transport 抽象**:用户可实现自定义 transport
3. **Bridge 模式**:解耦 CDP 协议与 servo 实现
4. **零 tokio**:同步 I/O,与 Bao 运行栈一致
5. **单线程约束**:与 servo JSContext 寄生模型一致
6. **完整错误码**:每个错误码都对应明确语义
7. **类型安全**:所有公共 API 含完整 doc + 示例
