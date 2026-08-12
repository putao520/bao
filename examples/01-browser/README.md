# Example 01 — 最基本的 servo 浏览器

展示 Bao 的最小编码路径:创建 runtime → 创建 page → 导航 → 求值 → 截图。

## 运行

```bash
cargo run
```

## 预期输出

```
[01-browser] BaoRuntime ready
[01-browser] Navigating to https://example.com ...
[01-browser] URL after navigate: https://example.com/
[01-browser] <title> = Example Domain
[01-browser] navigator.userAgent (Page Realm) = Mozilla/5.0 ... (servo/...)
[01-browser] Screenshot saved: bao-01-browser.png (12345 bytes)
[01-browser] Page state: Idle
[01-browser] Done
```

## 核心 API 调用

```rust
let runtime = BaoRuntime::new(BaoConfig::default())?;
let page = runtime.create_page(&PageConfig::default())?;
page.navigate("https://example.com")?;
page.wait_for_pipeline_ready(Duration::from_secs(30))?;
let title = page.evaluate_js_web("document.title")?;           // Page Realm
let png  = page.take_screenshot(ScreenshotFormat::Png)?;       // RGBA → PNG
```

## 关键点

- **`evaluate_js_web`** 在 Page Realm(Window global)执行,**只有 Web API**(`document` / `navigator` / `window`),`typeof require === 'undefined'`。这是隔离的浏览器侧脚本,安全用于不可信页面代码
- **`evaluate_js`** 见示例 03,在 Node Realm 执行,同时拥有 Node.js API + DOM
- **`wait_for_pipeline_ready`** 必须在 navigate 之后调用,servo 的 ScriptThread pipeline 是异步建立的,直接 `evaluate_js_web` 会 SIGSEGV
