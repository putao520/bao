# Example 03 — Node.js API × DOM 同一 runtime 共存(Bao 核心卖点)

展示 Bao 最核心的能力:**同一个 SpiderMonkey JSContext 下,Node.js API 和 DOM 共存**。
脚本可以同时调用 `document.querySelector`(Web API)和 `require('fs').readFileSync`(Node.js API),
因为在 Node Realm 执行时,SpiderMonkey 的 global 上同时挂了 DOM 对象和 Node.js host functions。

## 双 Realm 架构

```
                 SpiderMonkey JSContext(servo ScriptThread thread-local)
                              │
              ┌───────────────┴───────────────┐
              ▼                               ▼
     ┌─────────────────┐             ┌─────────────────┐
     │  Page Realm     │             │  Node Realm     │
     │  (Window global)│             │  (Node global)  │
     │                 │             │                 │
     │  ✓ document     │             │  ✓ document     │
     │  ✓ navigator    │             │  ✓ navigator    │
     │  ✓ fetch        │             │  ✓ require      │
     │  ✗ require      │             │  ✓ Bun.*        │
     │  ✗ process      │             │  ✓ process      │
     │                 │             │  ✓ Bun.fs / http│
     │  evaluate_js_web│             │  evaluate_js    │
     └─────────────────┘             └─────────────────┘
              ▲                               ▲
              │                               │
        Page JS(不可信)                Trusted JS(可信)
        typeof require === 'undefined'  Node.js + DOM 全权限
```

Realm 物理隔离:Page Realm 上的脚本无法发现 Node Realm 的对象(Compartment 隔离,
SPEC REQ-BRW-003-C5)。

## 运行

```bash
cargo run
```

## 预期输出

```
[03-node-dom] BaoRuntime ready
[03-node-dom] Writing a local file via Rust std::fs ...
[03-node-dom] Page created (id=0)
[03-node-dom] Navigating to https://example.com ...
[03-node-dom] Dual-realm JS executing ...
[03-node-dom]   document.querySelector('h1').textContent  = "Example Domain"
[03-node-dom]   require('fs').readFileSync('demo.txt','utf8') = "hello from fs\n"
[03-node-dom]   typeof Bun === 'object' = true
[03-node-dom]   typeof process.versions.node === 'string' = true
[03-node-dom] Page Realm check (evaluate_js_web): typeof require = "undefined"
[03-node-dom] Done
```

## 核心 API 调用

```rust
// Node Realm —— 同时拥有 DOM 和 Node.js API
let result = page.evaluate_js(r#"
    const title = document.querySelector('h1').textContent;
    const file  = require('fs').readFileSync('demo.txt', 'utf8');
    JSON.stringify({ title, file })
"#)?;

// Page Realm —— 只有 Web API
page.evaluate_js_web("typeof require")?;  // → "undefined"
```

## 关键点

- **`evaluate_js`** = 可信脚本(Node Realm)。必须只运行你自己的代码,因为它有 Node.js 全权限(可读任意文件)
- **`evaluate_js_web`** = 不可信页面脚本(Page Realm)。等价于浏览器侧 JS,被 Compartment 隔离
- 两个 Realm 共享同一个 GC(由 servo `spin_event_loop` 统一管理,SPEC REQ-BRW-003-C8)
- 这是 Bao 相对于 Chrome/Puppeteer 的最大优势:**服务端自动化时,DOM 抽取 + 文件/HTTP 直接在同一段 JS 里完成**,不需要在 Node 和 Browser 之间序列化数据
