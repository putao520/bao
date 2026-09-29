# Example 08 — 错误处理链

Bao 的公开错误类型都实现 `std::error::Error`;本示例收集四类真实错误并逐层打印
`source()` 因果链——写自动化时按 variant 分派,按链定位根因。

## 运行

```bash
cargo run
```

## 核心 API 调用

```rust
let err: CdpError = browser.version().expect_err("dead port");  // CDP 握手失败
let err: BrowserError = page.navigate("not a url")?;            // BrowserError::Navigation
let err = closed_page.evaluate_js_web("1+1")?;                  // "page is closed"
let err = page.evaluate_js_web("null.x")?;                      // BrowserError::JavaScript
```

## 关键点

- 按 variant 分派(`matches!(err, BrowserError::Navigation(_))`),不 parse 错误字符串
- `Browser::connect` 是惰性的(只做 URL 路由);真正的握手错误发生在第一个命令上
- 更深的 source 链(CdpError::Connect → ConnectError::Io → io 根因)是 W29 契约面,
  由 `bao-core` error_model 测试锁定
