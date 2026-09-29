# Example 06 — 优雅关闭

关闭序三件事:pending work 真实落定 → 逐页 close(状态机 Closing → Closed)→ runtime drop(有界 join)。

## 运行

```bash
cargo run
```

## 核心 API 调用

```rust
page.evaluate_js_web("setTimeout(...)")?;                     // arm 异步面
page.wait_for_function("globalThis.__done === 1", timeout)?;  // drain pending work
runtime.page_pool().close_page(id)?;                          // PageState::Closed
drop(runtime);                                                // close_all 兜底 + 有界 join
```

## 关键点

- 优雅关闭的前提是先让页面自己的异步面(定时器/promise)走完——`wait_for_function` 是 drain 原语
- `close_page` 走完整清理链:Worker 终止 → per-webview 注册表清除 → 状态机 Closed
- runtime drop 的 servo join 自旋有 15s 上限(W27):即使某线程卡死,teardown 也有界、泄漏被如实记录
