# Third-Party Licenses — Bao (包子)

Bao 是一个 Rust-native 的可编程浏览器运行时,在 MIT OR MPL-2.0 双 license 下发布。
本文件列出 Bao 仓库整合的所有上游第三方项目,及其原始 license、在本仓库中的位置、
**是否被 Bao 修改** 以及修改清单。

> MPL-2.0 §3.3(b) 合规要求:对 MPL 文件做出修改后,必须在分发可执行文件时声明
> "修改后的文件 + 修改说明"。本文件的 **§2.1 Servo** 和 **§2.2 mozjs** 两节
> 即为满足此要求的完整修改清单。

---

## 1. 原创代码

| 范围 | 说明 |
|------|------|
| `src/bao_*` | Bao 项目原创代码,共 17 个 crate(`bao`、`bao_engine`、`bao_browser`、`bao_cdp`、`bao_cdp_client`、`bao_stealth`、`bao_runtime`、`bao_uloop`、`bao_crypto`、`bao_boringssl_bridge`、`bao_bundler`、`bao_cli`、`bao_bin`、`bao_native_stubs`、`bao_engine_macros`、`bao_lints`、`bao_workflow_host`)。License: MIT OR MPL-2.0。Copyright (c) 2025-2026 The Bao Project Authors. |

---

## 2. 上游 vendored 第三方项目

### 2.1 Servo(浏览器引擎)— MPL-2.0,**含 BAO PATCH 修改**

| 字段 | 值 |
|------|---|
| 项目 | Servo |
| 上游 URL | https://github.com/servo/servo |
| 原 License | Mozilla Public License 2.0(**MPL-2.0**) |
| License 文件 | `vendor/servo/LICENSE` |
| 在 Bao 中的路径 | `vendor/servo/` |
| **是否修改** | **Yes — 13 个文件带 BAO PATCH(满足 MPL-2.0 §3.3(b))** |
| 排除编译 | `bao` workspace 在根 `Cargo.toml` 显式 `exclude = ["vendor/servo", "vendor/mozjs"]`,servo 以外部 crate 依赖形式链接 |

**BAO PATCH 文件清单(13 个唯一文件,按 BCE 标注分组)**:

| # | 文件路径(相对仓库根) | BCE / BAO PATCH 标注 | 修改目的 |
|---|---|---|---|
| 1 | `vendor/servo/components/allocator/lib.rs` | `BAO PATCH (embed)` | 条件化安装 `#[global_allocator]`(仅特定 feature 启用),避免与 bao 层分配器冲突 |
| 2 | `vendor/servo/components/config/opts.rs` | `BAO PATCH (BCE-20260621-002)` + `BAO PATCH (BCE-20260627-009)` | (a) 默认关闭 servo 内置 `JS::Debugger`(让 bao 层自己的 debugger 接管);(b) 幂等 `initialize_options()` 支持,允许 bao 多次初始化 servo |
| 3 | `vendor/servo/components/constellation/constellation.rs` | `BAO PATCH (BCE-20260627-009)` | 每-Constellation `RouterProxy` 用于 bao 的多 `BaoRuntime` 实例 IPC 路由;Drop 时自动 shutdown |
| 4 | `vendor/servo/components/constellation/event_loop.rs` | `BAO PATCH (BCE-20260627-009)` | 继承 `router_proxy` 到新 event loop;每实例独立 router |
| 5 | `vendor/servo/components/net/async_runtime.rs` | `BAO PATCH (BCE-20260627-009, root-caused BCE-20260628-001)` | 空 holder(无 owned runtime)支持;幂等 init;multiprocess 路径兼容 bao 多 runtime 模型 |
| 6 | `vendor/servo/components/script/dom/workers/dedicatedworkerglobalscope.rs` | `BAO PATCH (BCE-20260627-009)` | realm entry 标记;`clear_js_runtime` 前刷 cx realm stack(防止 bao 多 realm 栈损坏致 SIGSEGV) |
| 7 | `vendor/servo/components/script/script_runtime.rs` | `BAO PATCH (BCE-20260627-009)` | 幂等 `JSEngine::init`(防 `AlreadyInitialized` race);never-drop / never-clear `JS_ENGINE`(避免 secondary init 崩溃) |
| 8 | `vendor/servo/components/script/script_thread.rs` | `BAO PATCH (BCE-20260627-009)` + `BAO PATCH (BCE-20260621-002)` | (a) 安装 per-instance router;(b) 跳过 `fire_add_debuggee` 当 debugger 被禁用 |
| 9 | `vendor/servo/components/script_bindings/lock.rs` | `BAO PATCH (BCE-20260627-009)` | 幂等锁操作(若已持有则不重复加锁) |
| 10 | `vendor/servo/components/servo/servo.rs` | `BAO PATCH (BCE-20260627-009)` | multiprocess 路径创建 fresh delegate,适配 bao 多实例 |
| 11 | `vendor/servo/components/shared/base/id.rs` | `BAO PATCH (BCE-20260627-009)` | 幂等 TLS slot 初始化(若已被同线程 set 则不覆盖) |
| 12 | `vendor/servo/components/shared/net/lib.rs` | `BAO PATCH (BCE-20260627-009)` | Fetch-thread 生命周期为 bao 多 `BaoRuntime` 模型重写;per-thread sender;per-instance exit |
| 13 | `vendor/servo/components/shared/script/lib.rs` | `BAO PATCH (BCE-20260627-009)` | per-ScriptThread `RouterProxy` 字段 |

所有修改均带内联 `// BAO PATCH (BCE-XXXXXX-NNN)` 注释标注,且对应一次完整的
BCE(Bug-Class Eradication)根除记录(沉淀于 `src/BUG-KNOWLEDGE.md`)。
所有修改的目的均为:让 servo 作为嵌入式引擎支持 bao 的
"多 `BaoRuntime` 实例 + 线程局部 JSContext + 幂等初始化"运行模型,
并非对 servo 公共 API 行为的语义性改变。

---

### 2.2 mozjs(SpiderMonkey Rust 绑定)— MPL-2.0,**含 BAO PATCH 修改**

| 字段 | 值 |
|------|---|
| 项目 | mozjs(SpiderMonkey Rust 绑定,包含 `mozjs-sys` FFI) |
| 上游 URL | https://github.com/servo/mozjs |
| 原 License | Mozilla Public License 2.0(**MPL-2.0**) |
| License 字段 | `vendor/mozjs/Cargo.toml`: `license = "MPL-2.0"` |
| 在 Bao 中的路径 | `vendor/mozjs/` |
| **是否修改** | **Yes — 2 个文件带 BAO PATCH(满足 MPL-2.0 §3.3(b))** |
| 排除编译 | `bao` workspace 在根 `Cargo.toml` 显式 `exclude = ["vendor/servo", "vendor/mozjs"]` |

**BAO PATCH 文件清单(2 个)**:

| # | 文件路径 | BCE 标注 | 修改目的 |
|---|---|---|---|
| 1 | `vendor/mozjs/mozjs-sys/mozjs/js/src/jit/BaselineFrame.cpp` | `BAO PATCH (BCE-20260621-002)` | 防御 `cx->activation()` 为 NULL 的边界情况(bao 嵌入式场景下 activation 栈可能未初始化),避免 SIGSEGV |
| 2 | `vendor/mozjs/mozjs/src/rust.rs` | `BAO PATCH`(3 处,含 `BCE-20260622-004`) | (a) 序列化 `JSEngine::init` — 停止 `AlreadyInitialized` race(适配 servo `JSEngineSetup` 与 bao 自身初始化的并发场景);(b) `Err(AlreadyInitialized)` 后恢复 JSEngine handle,使 secondary init 路径不致命;(c) 暴露 `AtomCacheHashTable` 内部接口供 bao 使用 |

**额外说明**:`mozjs/mozjs-sys/mozjs/mozglue/misc/Mutex_posix.cpp` 的
`MutexImpl::~MutexImpl` 包含一个已应用的非 BAO PATCH(EBUSY patch):
原 `pthread_mutex_destroy` 返回非零即 `MOZ_CRASH`,patch 为忽略 `EBUSY`
(libtest 线程池线程在 TLS teardown 时仍持有 mutex,详见项目根 `CLAUDE.md`
"EBUSY Patch" 章节)。该修改未标 BAO PATCH 因为它源自上游社区讨论,
非 bao 特有。

---

### 2.3 Bun-derived Rust crates(Bun 项目衍生的纯 Rust crate)— MIT,**零修改复用**

| 字段 | 值 |
|------|---|
| 项目 | Bun(https://bun.sh) |
| 上游 URL | https://github.com/oven-sh/bun |
| 原 License | MIT License(Bun 项目整体) |
| 在 Bao 中的路径 | `src/{crate-name}/`(76 个 crate,见下表) |
| **是否修改** | **No — 零修改复用**(纯 Rust crate,无 JSC 依赖,直接复用 Bun 已编译/优化/测试过的实现) |
| 复用原则 | 见项目根 `CLAUDE.md` §3 "复用优先",禁止手写已有功能 |

**76 个 Bun-derived crate 清单**(以 `bun_X` 包名映射到 `src/X/` 路径):

```
alloc, analytics, api, ast, boringssl, boringssl_sys, brotli, bundler, bunfig,
cares_sys, clap, clap_macros, collections, core, core_macros, crash_handler,
css, dispatch, dns, dotenv, errno, event_loop, exe_format, glob, hash, highway,
http, http_types, install, install_types, io, jsc_macros, js_parser, js_printer,
libarchive, libuv_sys, lolhtml_sys, lsquic_sys, md, mimalloc_sys, opaque,
options_types, output, output_tags, parsers, paths, perf, picohttp, platform,
ptr, resolve_builtins, resolver, router, runtime, safety, semver, sha_hmac,
shell_parser, simdutf_sys, sourcemap, spawn, spawn_sys, standalone_graph,
sys, tcc_sys, threading, transpiler, url, uws, uws_sys, watcher, which,
windows_sys, wyhash, zlib, zstd
```

(即 `bun_alloc`→`src/alloc/`,`bun_base64`→`src/base64/`,依此类推。
`bun_sm` 是 Bao 自有 crate — SpiderMonkey 兼容层替代 `bun_jsc` — 不属于
Bun-derived 复用,归 §1 原创代码。)

---

### 2.4 BoringSSL(TLS 库)— Apache-2.0,**零修改**

| 字段 | 值 |
|------|---|
| 项目 | BoringSSL(Google 维护的 OpenSSL fork) |
| 上游 URL | https://boringssl.googlesource.com/boringssl |
| 原 License | **Apache License 2.0**(见 `vendor/boringssl/LICENSE` 首行) |
| 在 Bao 中的路径 | `vendor/boringssl/` |
| **是否修改** | **No — 零修改** |

Bao 通过 `bao_boringssl_bridge` crate 调用 BoringSSL 的 C API 实现 TLS
反指纹(JA3/JA4),所有交互通过 FFI 在 bao 层完成,BoringSSL 源码本身未改动。

---

### 2.5 lsquic(QUIC 实现)— MIT,**零修改**

| 字段 | 值 |
|------|---|
| 项目 | LiteSpeed QUIC(lsquic) |
| 上游 URL | https://github.com/litespeedtech/lsquic |
| 原 License | **MIT License**(Copyright (c) 2017 - 2026 LiteSpeed Technologies Inc) |
| 在 Bao 中的路径 | `vendor/lsquic/` |
| **是否修改** | **No — 零修改** |
| 额外 License | `vendor/lsquic/LICENSE.chrome` — 部分代码基于 Chromium `proto-quic`,BSD-3-Clause(The Chromium Authors, Copyright 2015) |

---

### 2.6 lshpack(HTTP/2 / HTTP/3 头压缩)— MIT,**零修改**

| 字段 | 值 |
|------|---|
| 项目 | LiteSpeed HPACK / QPACK(lshpack) |
| 上游 URL | https://github.com/litespeedtech/lshpack |
| 原 License | **MIT License**(Copyright (c) 2018 - 2023 LiteSpeed Technologies Inc) |
| 在 Bao 中的路径 | `vendor/lshpack/` |
| **是否修改** | **No — 零修改** |

---

### 2.7 lsqpack(QPACK 实现)— MIT,**零修改**

| 字段 | 值 |
|------|---|
| 项目 | LiteSpeed QPACK(lsqpack) |
| 上游 URL | https://github.com/litespeedtech/lsqpack |
| 原 License | **MIT License**(Copyright (c) 2018 - 2022 LiteSpeed Technologies Inc) |
| 在 Bao 中的路径 | `vendor/lsqpack/` |
| **是否修改** | **No — 零修改** |

---

### 2.8 mimalloc(分配器)— MIT,**零修改**

| 字段 | 值 |
|------|---|
| 项目 | mimalloc |
| 上游 URL | https://github.com/microsoft/mimalloc |
| 原 License | **MIT License**(Copyright (c) 2018-2025 Microsoft Corporation, Daan Leijen) |
| 在 Bao 中的路径 | `vendor/mimalloc/` |
| **是否修改** | **No — 零修改** |

---

### 2.9 ipc-channel(servo 的 IPC 库)— MIT OR Apache-2.0,**零修改**

| 字段 | 值 |
|------|---|
| 项目 | ipc-channel |
| 上游 URL | https://github.com/servo/ipc-channel |
| 原 License | **MIT OR Apache-2.0**(双 license,见 `vendor/ipc-channel/LICENSE-MIT` + `vendor/ipc-channel/LICENSE-APACHE`,Copyright (c) 2012-2013 Mozilla Foundation) |
| 在 Bao 中的路径 | `vendor/ipc-channel/` |
| **是否修改** | **No — 零修改** |

---

## 3. License 汇总表

| 项目 | 原 License | 在 Bao 路径 | 修改? | BAO PATCH 文件数 |
|------|-----------|-------------|-------|------------------|
| Servo | MPL-2.0 | `vendor/servo/` | Yes | 13 |
| mozjs | MPL-2.0 | `vendor/mozjs/` | Yes | 2 |
| Bun-derived crates | MIT | `src/{crate}/`(76 个) | No | 0 |
| BoringSSL | Apache-2.0 | `vendor/boringssl/` | No | 0 |
| lsquic | MIT(+ 部分 BSD-3) | `vendor/lsquic/` | No | 0 |
| lshpack | MIT | `vendor/lshpack/` | No | 0 |
| lsqpack | MIT | `vendor/lsqpack/` | No | 0 |
| mimalloc | MIT | `vendor/mimalloc/` | No | 0 |
| ipc-channel | MIT OR Apache-2.0 | `vendor/ipc-channel/` | No | 0 |

---

## 4. MPL-2.0 合规声明

依据 MPL-2.0 §3.3(b),对于已修改的 MPL 许可文件(Servo 13 个 + mozjs 2 个,
共 **15 个文件**),Bao 在分发源代码与可执行文件时声明:

1. **修改后的文件清单** — 见 §2.1 和 §2.2 的表格,所有文件路径完整列出。
2. **修改说明** — 每个修改都在源码中以 `// BAO PATCH (BCE-XXXXXX-NNN):`
   注释形式标注修改目的,并对应一次 BCE 根除记录(沉淀于
   `src/BUG-KNOWLEDGE.md`)。
3. **原 license 保留** — 所有修改后的文件均保留原 MPL-2.0 license header
   与 copyright 声明,未删除或覆盖。
4. **修改可追踪** — 每个 BAO PATCH 都关联一个 BCE 编号(如
   `BCE-20260627-009`),可通过 `src/BUG-KNOWLEDGE.md` 追溯完整归因链
   (症状 → 根因 → 泛化 → 根治 → 残留=0 确认)。

所有 MPL 修改的总体技术目的均为:使 servo/mozjs 作为嵌入式引擎支持 Bao 的
"多 `BaoRuntime` 实例 + 线程局部 JSContext + 幂等初始化 + 多线程并发
PagePool"运行模型(详见项目根 `CLAUDE.md` §2 "JSContext 模型"),而非对
上游公共 API 的语义性变更。

---

## 5.上游 provenance 总览

| 上游项目 | 在 Bao 中的角色 |
|---------|-----------------|
| **Servo** | 浏览器引擎核心:DOM + CSS + Layout + WebRender 渲染。Bao 通过 `bao_browser` crate 桥接 Servo 的 `libservo`,提供 `PagePool` / `PageHandle` / `BaoServoDelegate` 高层 API。 |
| **mozjs / SpiderMonkey** | JS 引擎核心。Bao 通过 `bao_engine` + `bun_sm` crate 封装 SpiderMonkey FFI,替代 Bun 原本的 JSC 引擎。 |
| **Bun-derived crates** | 76 个纯 Rust crate(HTTP / FS / Resolver / Bundler / DNS / Base64 / collections / transpiler / event_loop 等),复用 Bun 项目的成熟基础设施,禁止重写。 |
| **BoringSSL** | TLS 实现,`bao_stealth` 通过 `bao_boringssl_bridge` 调用其 C API 实现 TLS 反指纹(JA3/JA4)。 |
| **lsquic / lshpack / lsqpack** | QUIC + HTTP/2/3 头压缩栈,支撑 HTTP/3 反指纹与高性能网络。 |
| **mimalloc** | 高性能分配器(可选,通过 `bun_mimalloc_sys` 链接)。 |
| **ipc-channel** | servo 衍生的跨进程 IPC 库,服务于 servo 的 multiprocess 模式。 |

---

*本文件由 Bao 项目维护,随上游同步与 BAO PATCH 变更更新。*
*最近更新:2026-08-12*
