# Bun API Compatibility

> **Honesty-first.** 通过率表全部 `TBD`,**不造数据**。

## 范围

Bao 复用 Bun workspace 中 ~76 个纯 Rust crate,并桥接 `Bun.*` 全局对象(`Bao.*` 是别名,指向同一对象)。
本目录对照 Bun 上游测试规范,公开每个 Bun-specific API 的**真实通过率**。

## 已覆盖 API(从测试目录盘点)

下列 API **已有测试存在**,但**尚未聚合**为统一通过率报告。

| API | 测试文件存在 | Pass Rate | Notes |
|-----|:---:|:---:|-------|
| `Bun.version` | ✓ `bun_api_tests.rs`, `bun_api_deep_tests.rs`, `test_bun_api.js` | TBD | tests exist, not aggregated |
| `Bun.file` / `Bun.write` | (待确认) | TBD | — |
| `bun:sqlite` | ✓ SPEC REQ-ENG-008 覆盖 | TBD | tests exist in bao_runtime, not aggregated |
| `bun:ffi` | ✓ SPEC REQ-ENG-009 覆盖 | TBD | tests exist in bao_runtime, not aggregated |
| `Bun.serve` | (待确认) | TBD | 复用 `bun_uws::App` |
| `Bun.spawn` | (待确认) | TBD | 复用 `bun_spawn` |
| `Bun.fetch` | ✓ `fetch_api_tests.rs`, `fetch_e2e_tests.rs`, `fetch_c13_minimal.rs`, `h3_fetch_tests.rs` | TBD | tests exist, not aggregated |
| `bun:test` | ✓ `bun_api_tests.rs`, `bun_test_deep_tests.rs`, `test_bun_test.js`, `test_bun_test_shim.js` | TBD | tests exist, not aggregated |
| `bun:build` | ✓ `test_bun_build.js` | TBD | tests exist, not aggregated |
| `Bun.password` / `Bun.hash` / `Bun.CryptoHasher` | (待确认) | TBD | — |
| `Bun.readableStreamToArray` / 等 stream 工具 | (待确认) | TBD | — |
| `Bun.Glob` | (待确认) | TBD | — |
| `Bun.deflateSync` / `Bun.gunSync` / 压缩工具 | (待确认) | TBD | — |
| `Bun.env` | (待确认) | TBD | — |
| `Bun.main` / `Bun.cwd` / `Bun.origin` | (待确认) | TBD | — |
| `Bun.dns` | (待确认) | TBD | 复用 `bun_dns` |
| `Bun.pathToFileURL` / `Bun.fileURLToPath` | (待确认) | TBD | 复用 `bun_url` |
| `Bun.openInEditor` | (待确认) | TBD | — |
| `Bun.semver` | (待确认) | TBD | — |
| `Bun.embeddedFiles` / `Bun asset bundle` | (待确认) | TBD | — |
| `Bun.Transpiler` | (待确认) | TBD | 复用 `bun_transpiler` |
| `Bun.JSX` | (待确认) | TBD | — |
| `Bun.TOML` | (待确认) | TBD | — |

## 上游对照

- Bun 官方 test suite: <https://github.com/oven-sh/bun/tree/main/test>
- Bun API docs: <https://bun.com/docs/api>

## TODO

- 每个标"(待确认)"的 API:实际跑 `typeof Bun.X` + 基本调用,确认是 `function` 还是 `undefined`
- 聚合通过率
- 跟踪 Bun 上游 test ref(版本绑定)
- `Bun.file` / `Bun.write` / `Bun.serve` / `Bun.spawn` 是高优先补全项

## 跑法

```bash
cargo test -p bao_runtime bun_api
cargo test -p bao_runtime fetch
```

## 统一聚合命令

`bao compat bun`:**TBD**,尚未实现。
