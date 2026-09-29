# Web Platform Compatibility

> **Honesty-first.** 通过率表不造数据——逐行判定见
> [INVENTORY.md](INVENTORY.md)(W13 · 2026-09-29 · #14 G1 收官;分类准绳=测试在树+电池 v3 绿)。

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
> Bao 作为 servo 嵌入方需要在集成边界做自己的回归测试。Bao 自身跑 WPT 子集:
> runner 未接,**列后续波**(缺口见 INVENTORY.md 文末)。

## 已覆盖(继承 servo + Bao 桥接测试)

| Domain | servo 上游 | Bao 侧测试 | 状态(INVENTORY) | Notes |
|--------|:---:|:---:|:---:|-------|
| HTML DOM | servo 上游持续测 | `web_api_tests.rs`, `web_api_deep_tests.rs`, `globals_deep_tests.rs` | Supported | INVENTORY #1 |
| CSS Layout(能力继承 REQ-BRW-049) | servo 上游持续测(Stylo) | `css_conformance_tests.rs`(73 断言门) | Supported | INVENTORY #4;像素回归=后续波 |
| WebRender | servo 上游持续测 | —(webrender 继承) | Partial | 像素回归=后续波;INVENTORY #8 同源 |
| Fetch | servo 上游 + Bao `fetch_api_tests.rs` | ✓ | Supported | INVENTORY #5 |
| WebSocket | servo 上游 + `test_websocket.js`, `test_ws_upgrade.js` | ✓ Partial | Partial | 已知 Partial(见 README.md);INVENTORY #6 |
| URL / URLSearchParams | servo 上游 + `test_upstream_url.js` | ✓ | Supported | INVENTORY #13 |
| Events | servo 上游 + `test_upstream_events.js`, `events_deep_tests.rs` | ✓ | Supported | INVENTORY #11 |
| Web APIs (日常) | servo 上游 + `web_api_deep_tests.rs` | ✓ | Supported | INVENTORY #1/#12 |
| 完整家族矩阵(14 家族,含 SVG/WebVTT/Workers/Storage/媒体) | — | — | — | **见 [INVENTORY.md](INVENTORY.md)**:Supported 10 / Partial 4 / Unsupported 0 |

## TODO

- 选定 WPT 子集(DOM / HTML / CSS / Fetch)首次跑通,得到与 servo 上游的偏差(后续波;runner 未接)
- WebSocket 完成度补齐(对照 README.md 的 Partial 标记,找具体缺什么)
- Navigation lifecycle 状态机回归
- 全页面渲染回归(对照 Render 真实像素,而非只测 DOM)
- Partial 家族逐格补齐(WS 完成度 / Shared Worker e2e / canvas conformance / cookies+cache e2e / 媒体环境解耦)——INVENTORY.md 已标注

## 跑法

```bash
# Bao 桥接层 Web API 测试
cargo test -p bao_runtime web_api
cargo test -p bao_runtime globals_deep

# WebSocket(JS 侧)
# bao test tests/test_websocket.js
```

## 统一聚合命令

`bao compat web`:**未实现**(后续波,与 CDP 命令同形;分类数据先落 INVENTORY.md)。
