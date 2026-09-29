# Example 05 — 多页面管理(PagePool)

一个 BrowserRuntime 并发持有多个 page:交错导航、独立求值、池级统计与 idle 归还。
全程 `data:` URL,零外部网络依赖。

## 运行

```bash
cargo run
```

## 核心 API 调用

```rust
let p1 = runtime.create_page(&PageConfig { url: Some(data_url_a), ..Default::default() })?;
let p2 = runtime.create_page(&PageConfig { url: Some(data_url_b), ..Default::default() })?;
let stats = runtime.page_pool().stats();        // active/idle/created/destroyed
runtime.page_pool().release_page(p1.id());     // active → idle(不销毁)
let p1_back = runtime.page_pool().get_page(p1.id())?; // idle → active,内容原样
runtime.page_pool().close_page(p2.id())?;      // 销毁,PageState → Closed
```

## 关键点

- 每个 page 一个 ScriptThread/JSContext,页间完全隔离(同选择器各自返回各自内容)
- `release_page` / `get_page` 是**池级**生命周期(归还/取回);`close_page` 是**页级**销毁
- `stats()` 是 PagePool 的权威账本(active + idle = 常驻页数)
