# Bao 统一库集成方案（Master Plan）

> SSOT for external Cargo integration.  
> Product constraint (`.spec/01-BUSINESS.html`): **统一的 Cargo 库** — SpiderMonkey + servo + Node/Bun API + CDP + Stealth **始终可用、零模式切换**。  
> **禁止**用 Cargo product features 关掉 browser / CDP / stealth / Node API。

## 1. 目标

| 目标 | 说明 |
|------|------|
| 一依赖接入 | 宿主只依赖 **一个** package：`bao` |
| 整栈始终链接 | `bao` 始终依赖 browser + engine + runtime + cdp + stealth（等） |
| 内部可多 crate | monorepo 内 `bao_*` / `bun_*` 保留维护分解；**不是**对外交付分解 |
| 运行时可选 | Stealth profile / Permission 沙箱 = 运行时配置，不是 compile feature |

## 2. 非目标

- 产品级 Cargo features 做能力拆分
- 宣称 pure-Rust 小库 / 秒编 crates.io 玩具库
- 全量 crates.io 发布流水线、重命名全部 `bun_*`
- 静默 fallback 缺 vendor/native（fail-closed）

## 3. 对外形态

```text
宿主 Cargo.toml
  bao = { path = "…/bao/src/bao" }   # 或 git + package = "bao"
        │
        ▼
  package bao (src/bao)              # 唯一公共 lib
        │ always depends on
        ├── bao_browser
        ├── bao_engine
        ├── bun_runtime (path: bao_runtime)
        ├── bao_cdp
        ├── bao_cdp_client
        ├── bao_stealth
        ├── bao_uloop
        └── (optional direct) bao_native_stubs  # feature `native-stubs`, NON-default
            # residual still arrives transitively via bun_runtime hard-dep
            # until RealImpl/dispatch noops move to owners — STUB-INVENTORY.md
```

稳定顶层 API（示例，完整见 `bao` crate docs）：

```rust
use bao::{BaoConfig, BaoRuntime, PageConfig, ScreenshotFormat, StealthProfile, Browser};
```

内部 crate 名仅供 monorepo 维护；**消费者文档不得**再列 `path = "src/bao_*"` 多包清单。

## 4. Wave DAG

| Wave | 目标 | 退出条件 (DoD) | 并行域 |
|------|------|----------------|--------|
| **W1** | 文档纠偏 | 消费者路径无多 path 嵌入菜单；单 package 配方存在；无产品 feature 菜单 | `docs` |
| **W2** | 公共 package `bao` | workspace member；始终依赖整栈；re-export 稳定 API；**无** default-off 产品 feature | `packaging`（独占 root `Cargo.toml` + `src/bao/`） |
| **W3** | 验证证据 | `cargo check -p bao` 或诚实 env 失败日志；结构测试证明 always-on deps | `verify` |
| **W4** | 构建人体工程学（不裁能力） | vendor/工具链说明 fail-closed；可选 artifact 笔记 | `build-docs` |

依赖边：`W1 ∥ W2`（文件不重叠）→ `W3` 依赖 W2 → `W4` 可与 W3 文档部分并行。

## 5. 原子任务表

### W1 — docs

| ID | 任务 | DoD | dependsOn |
|----|------|-----|-----------|
| W1-D1 | 重写 README 嵌入节为单 package | 无 `path = "src/bao_*"` 消费清单 | [] |
| W1-D2 | 示例 `use` 改为 `bao::…` | 例 4 等消费者示例用统一入口 | [W1-D1] |
| W1-D3 | CLAUDE 嵌入说明指向 facade | 不教多 crate path 嵌入 | [] |
| W1-D4 | 本 master plan 落盘 | `docs/unified-library-integration.md` 存在 | [] |

### W2 — packaging

| ID | 任务 | DoD | dependsOn |
|----|------|-----|-----------|
| W2-P1 | 创建 `src/bao` package | `name = "bao"` lib | [] |
| W2-P2 | 整栈 path deps | browser/engine/runtime/cdp/cdp_client/stealth/uloop/native_stubs | [W2-P1] |
| W2-P3 | re-export 稳定 API + 子模块 | 顶层 + `browser`/`engine`/… 命名空间 | [W2-P2] |
| W2-P4 | 注册 workspace member | root `Cargo.toml` members 含 `src/bao` | [W2-P1] |
| W2-P5 | 结构/单元测试 | Cargo.toml always-on + 真实 API 调用（如 StealthProfile） | [W2-P3] |

### W3 — verify

| ID | 任务 | DoD | dependsOn |
|----|------|-----|-----------|
| W3-V1 | `cargo check -p bao` | 日志写入 `{SCRATCH}/public-package-check.log` | [W2] |
| W3-V2 | `cargo test -p bao` | 结构测试通过；日志 `{SCRATCH}/bao-unit-tests.log` | [W2] |
| W3-V3 | wave-status 记录 | `{SCRATCH}/wave-status.md` | [W3-V1] |

### W4 — build ergonomics（不裁能力）

| ID | 任务 | DoD | dependsOn |
|----|------|-----|-----------|
| W4-B1 | 消费者构建前提文档 | 工具链/vendor fail-closed 说明并入 master plan 或 README | [W1] |
| W4-B2 | 禁止产品 feature 菜单 | 终扫 residual=0 | [W1,W2] |

## 6. 构建前提（整栈，fail-closed）

宿主/CI 需要：

1. **nightly** toolchain（见 `rust-toolchain.toml`）
2. **clang** / C++ 工具链（mozjs、boringssl、uSockets 等）
3. **vendor/** 齐全：`mozjs`、`servo`、`boringssl` 等（缺失 → 构建失败，禁止空实现伪装）
4. 首次从源码编 mozjs 耗时长；允许日后加 **预编译 artifact 缓存**，但 **不得**借此关掉任何产品能力

## 7. 命名注意

| 符号 | 归属 | 对外 |
|------|------|------|
| `bao::BaoRuntime` | `bao_browser` | 浏览器/多页面统一运行时（主嵌入入口） |
| `bao::runtime::BaoRuntime` | `bun_runtime` | Node/Bun 兼容运行时（命名空间访问） |
| `bao::Browser` | `bao_cdp_client` | Playwright 风格 CDP 入口 |
| package `bao` | 本 facade | 消费者依赖名 |
| binary `bao` | package `bao_bin` | CLI；与 lib package 名可并存 |

## 8. REQ-PURE 说明（非产品 feature）

`REQ-PURE-*` 是 **原生 C 依赖迁 pure-Rust 的工程债**，不是「可选 pure 产品形态」。  
不得在消费者文档中写成 Cargo feature 开关。

---

*Status: executed under goal unified-library-integration; update checklists in goal plan + wave-status.*
