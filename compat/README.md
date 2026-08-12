# Bao Compatibility Suite

> **诚实优先 (Honesty-first)。** Partial / Experimental 状态是**可信度**,不是缺陷。
> 本目录**不造数据**:任何未实际测量的通过率一律标 `TBD` 或 `tests exist, not aggregated`。
> 我们公开 Web / Node / Bun / CDP 兼容性的**真实**通过率,目标是让用户、贡献者、批评者都能复现。

---

## 这是什么

`compat/` 是 Bao 的兼容性测试套件骨架。目标是公开回答这些问题:

- Bao 覆盖了多少 Node.js API?(对照 Node 上游 test suite)
- Bao 覆盖了多少 Bun API?(对照 Bun 上游 test suite)
- Bao 的 Web Platform 覆盖?(对照 servo WPT)
- Bao 的 CDP 域方法覆盖?(对照 CDP spec 与 Playwright/Puppeteer 调用面)

**当前阶段**:骨架 + 已有测试盘点。**通过率表全部 TBD**,逐步替换为真实测量值。

## 子目录

| 目录 | 范围 | 上游对照 |
|------|------|----------|
| [`node/`](./node/README.md) | Node.js API 兼容(fs/path/crypto/http/url/buffer/assert/events/stream/...) | Node.js 上游 test suite |
| [`bun/`](./bun/README.md) | Bun API 兼容(`Bun.file` / `Bun.write` / `bun:sqlite` / `Bun.serve` / `Bun.spawn` / ...) | Bun 上游 test suite |
| [`web/`](./web/README.md) | Web Platform 兼容(DOM / CSS / Fetch / WebSocket / Layout) | servo WPT (Web Platform Tests) |
| [`cdp/`](./cdp/README.md) | CDP 12 域方法覆盖 | CDP spec + Playwright/Puppeteer 调用面 |

## 不变原则

1. **不填假数字** — 未测量 = `TBD`,不臆测、不四舍五入、不"看起来差不多"
2. **Partial 是可信度** — 明确标注哪些域 `Partial` / `Experimental`,而非包装成"基本可用"
3. **可复现** — 每个通过率数字必须可由 CI 或本地命令复现,列出跑法
4. **版本绑定** — 通过率报告绑定 Bao 版本 + 测试日期 + 上游 ref

## 现状(对照 README.md Compatibility Matrix)

| Capability | Bao 状态 | 在本目录的位置 |
|---|:---:|---|
| HTML DOM / CSS / WebRender | ✓(servo 上游) | `web/` |
| SpiderMonkey / CommonJS / ESM | ✓ | (引擎层,不在 compat 范围) |
| `fs` / `path` / `crypto` / `http` | ✓(Bao/Bun) | `node/` |
| `bun:sqlite` / `bun:ffi` | ✓(Bao/Bun) | `bun/` |
| WebSocket | Partial | `web/` |
| CDP Page / Runtime / DOM / Network | ✓(12 domains) | `cdp/` |
| CDP Debugger | Partial | `cdp/` |
| Playwright over CDP | Experimental | `cdp/` |
| Puppeteer over CDP | Experimental | `cdp/` |

"✓" 表示功能存在并有测试覆盖;**不等于**通过率 100%。通过率数字在子目录的表格里,当前都是 TBD。

## 跑法

兼容性测试当前散落在各 crate 的 `tests/` 目录,尚未统一聚合。临时跑法:

```bash
# Rust 侧全量(cargo test 默认多线程,EBUSY patch 生效)
cargo test

# 仅 Node 兼容(bao_runtime)
cargo test -p bao_runtime

# 仅 CDP
cargo test -p bao_cdp

# 仅浏览器集成
cargo test -p bao_browser
```

**统一的 `bao compat` 子命令**(聚合所有 compat 测试 + 输出通过率报告):**TBD**,尚未实现。

## 贡献

填补 TBD 的方式:

1. 选一个 TBD 模块
2. 对照上游测试规范(Node/Bun/WPT/CDP)写测试或聚合已有测试
3. 跑测试,**真实记录**通过/失败/跳过数
4. 更新对应 README 的通过率表,注明:版本 / 日期 / 上游 ref / 跑法

禁止:为凑通过率修改测试逻辑、跳过失败用例不报告、把 flaky 标 pass。
