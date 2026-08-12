# Web Platform Compatibility

> **Honesty-first.** 通过率表全部 `TBD`,**不造数据**。

## 范围

Bao 用 servo 作为浏览器引擎,Web Platform 行为**继承 servo 上游**。本目录的目标是:

1. 不重复造 WPT (Web Platform Tests),**指向 servo 上游 WPT 结果**
2. 跟踪 Bao 集成 servo 时引入的偏差(自定义 patch、桥接层 bug)
3. 公开 Fetch / WebSocket / Layout 等关键域的**实测**通过率

## 上游对照

- servo WPT results: <https://wpt.fyi/results?product=servo>
- WPT 官方: <https://wpt.fyi/>
- servo CI: <https://github.com/servo/servo/actions>

> **servo 上游 WPT 结果 ≠ Bao WPT 结果**。servo 在 wpt.fyi 上有持续测量,
> Bao 作为 servo 嵌入方需要在集成边界做自己的回归测试。Bao 自身跑 WPT 子集:**TBD**。

## 已覆盖(继承 servo + Bao 桥接测试)

| Domain | servo 上游 | Bao 侧测试 | Pass Rate | Notes |
|--------|:---:|:---:|:---:|-------|
| HTML DOM | servo 上游持续测 | `web_api_tests.rs`, `web_api_deep_tests.rs`, `globals_deep_tests.rs` | TBD | Bao 桥接层有覆盖,未聚合 |
| CSS Layout | servo 上游持续测 | — | TBD | 继承 servo,未独立测 |
| WebRender | servo 上游持续测 | — | TBD | 继承 servo |
| Fetch | servo 上游 + Bao `fetch_api_tests.rs` | ✓ | TBD | tests exist, not aggregated |
| WebSocket | servo 上游 + `test_websocket.js`, `test_ws_upgrade.js` | ✓ Partial | TBD | 已知 Partial(见 README.md) |
| URL / URLSearchParams | servo 上游 + `test_upstream_url.js` | ✓ | TBD | tests exist |
| Events | servo 上游 + `test_upstream_events.js`, `events_deep_tests.rs` | ✓ | TBD | tests exist |
| Web APIs (日常) | servo 上游 + `web_api_deep_tests.rs` | ✓ | TBD | tests exist |

## TODO

- 选定 WPT 子集(DOM / HTML / CSS / Fetch)首次跑通,得到与 servo 上游的偏差
- WebSocket 完成度补齐(对照 README.md 的 Partial 标记,找具体缺什么)
- Navigation lifecycle 状态机回归
- 全页面渲染回归(对照 Render 真实像素,而非只测 DOM)

## 跑法

```bash
# Bao 桥接层 Web API 测试
cargo test -p bao_runtime web_api
cargo test -p bao_runtime globals_deep

# WebSocket(JS 侧)
# bao test tests/test_websocket.js
```

## 统一聚合命令

`bao compat web`:**TBD**,尚未实现。
