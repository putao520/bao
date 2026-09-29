# Bun API Compatibility

> **Honesty-first.** 分类只回答「有/无+证据」,不造通过率数据。
> 全量盘点 SSOT:**[INVENTORY.md](./INVENTORY.md)**(上游 ref 锁定
> `e85606d48421d6c5aa6a41882f39b93d313616fd`,2026-05-28,v1.4.0)。

## 汇总(2026-09-29 盘点,W8)

总面 **N=58**(Bun.* 静态 51 行 + bun:* 模块 7 行;node:* 40 项与 CLI 24 子命令以聚合行另列):

- **Supported 40** — 实现存在 + 测试佐证(佐证文件逐行见 INVENTORY.md)
- **Partial 4** — Bun.connect/tcpSocket(语义面窄于上游)、Bun.JSON5(无专属测试)、Bun.password(实现无直接面测试)、Bun.ResourceUsage(部分字段经 process 面)
- **Unsupported 14** — Bun.Transpiler/JSX、Bun.openInEditor、Bun.embeddedFiles/Archive、Bun.S3、Bun.Valkey、Bun.CSRF、Bun.FileSystemRouter、Bun.Image、Bun.Markdown、Bun.Terminal、Bun.x509/TLS 面(11 行);bun:jsc / bun:main / bun:app(3 行)——上游有 bao 无,如实列明(非义务)

## 原未定行逐项落定(2026-09-29,原标记已全部消除)

| 原未定行(2026-09-27 前) | 落定 |
|---|---|
| `Bun.file` / `Bun.write` | **Supported**(bun_api.rs `bun_file`/`bun_write`;专测 bun_file_methods_e2e_tests.rs;BunFile 方法面 text/json/arrayBuffer/stream/writer/slice/exists/read) |
| `Bun.serve` | **Supported**(bun_api.rs + bun_listen.rs;三专测 serve_abort/serve_binary_body/listen_tcp_behavior) |
| `Bun.spawn` | **Supported**(bun_spawn_sync.rs spawn/spawnSync;专测 spawn_events + child_process_spawn_events) |
| `Bun.password` / `Bun.hash` / `Bun.CryptoHasher` | hash/CryptoHasher=**Supported**(bun_hash_api.rs,face_e2e 全算法族);password=**Partial**(bun_password.rs,无直接面测试) |
| `Bun.readableStreamToArray` 等 stream 工具 | **Supported**(streams 面;face_e2e 命中) |
| `Bun.Glob` | **Supported**(bun_glob_api.rs) |
| `Bun.deflateSync`/`Bun.gunSync`/压缩工具 | **Supported**(gzip/gunzip/deflate/inflate +Sync 族;face_e2e 命中 adler32/zip) |
| `Bun.env` | **Supported**(getter→process.env 代理) |
| `Bun.main` / `Bun.cwd` / `Bun.origin` | main/cwd=**Supported**;origin=上游 getter 面,bao 未单列(见 INVENTORY) |
| `Bun.dns` | node:dns 面归 G1-Node inventory(bun_dns 复用实现) |
| `Bun.pathToFileURL` / `Bun.fileURLToPath` | **Supported** |
| `Bun.openInEditor` | **Unsupported**(无实现) |
| `Bun.semver` | 上游 getSemver 面;bao 经 node:semver/工具面承载,**Unsupported**(Bun.semver 单列无实现) |
| `Bun.embeddedFiles` / asset bundle | **Unsupported**(无实现) |
| `Bun.Transpiler` | **Unsupported**(API 面 0 命中;转译能力经 Bun.build/bao_bundler 承载) |
| `Bun.JSX` | **Unsupported**(无实现) |
| `Bun.TOML` | **Supported**(bun_serde_api.rs;含 YAML/JSONC) |

## 上游对照

- Bun 官方 test suite: <https://github.com/oven-sh/bun/tree/main/test>
- Bun API docs: <https://bun.com/docs/api>
- 上游 ref 锚定与面来源:见 [INVENTORY.md](./INVENTORY.md)「上游锚」

## 后续波(非本盘点范围)

- 通过率聚合(跑既有测试面出真实数字)
- Partial 4 项语义补齐(测试先行)
- bun:jsc/bun:main/bun:app 与 Transpiler 族:产品决策项(非义务,需用户裁决)
