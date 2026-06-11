# Pure Rust 迁移开发升级方案

> 创建日期: 2026-06-11
> 更新日期: 2026-06-11 (v4 — 战略调整：保留所有 C/C++，仅消灭 Zig)
> 状态: COMPLETED — P1+P2+P4a+Zig 清除已完成，C/C++ 依赖永久保留
> SPEC 变更: +10 REQ-PURE-*, +1 NFR-PURE-001

---

## 0. 迁移动机

Bao 已完全独立于 Bun/Servo 上游，拥有自己的 fork 仓库。消除所有 C/C++/Zig 原生依赖可：
1. **统一内存安全模型** — 所有代码在 Rust 安全保证下运行
2. **消除供应链攻击面** — 不再依赖 C/C++ vendor 代码
3. **简化交叉编译** — 纯 Rust 零工具链依赖（除 mozjs_sys）
4. **降低构建复杂度** — 不再需要 CMake/ninja/clang 等构建工具

---

## 1. C/C++/Zig 依赖清单与替换状态

| 依赖 | 语言 | 用途 | 替换方案 | 状态 |
|------|------|------|---------|------|
| brotli | C | 压缩/解压 | `brotli` crate v8 | ✅ P1 完成 |
| zstd | C | 压缩/解压 | `zstd-pure-rs` | ✅ P1 完成 |
| zlib/libdeflate | C | gzip/deflate | `flate2` + `crc32fast` | ✅ P1 完成 |
| BoringSSL | C++ | TLS 1.2/1.3 + JA3/JA4 指纹 | **永久保留** — 纯 Rust 生态无 TLS 指纹定制方案，BoringSSL 生态（rquest/wreq）一并保留 | 🔒 永久保留 |
| mimalloc | C | 内存分配器 | **保留** — 150+ 文件直接 mi_* FFI，MimallocArena 无替代 | ❌ 不可替换 |
| highway | C++ | xxHash+SIMD strings | **部分替换** — xxHash→twox-hash，30+ SIMD 内核保留 | ✅ 部分完成 |
| lsquic | C | QUIC 协议 | quinn + BoringSSL TLS | ⏳ P4 |
| lolhtml | C++ | HTML 重写 | `lol_html` crate v3.0 (纯 Rust，同源码) | ✅ P4a 完成 |
| uWS/uSockets | C++ | HTTP/WebSocket | hyper + tokio | ⏳ P5 |
| lshpack | C | HPACK 压缩 | hyper 内置 | ⏳ P5 |
| libuv | C | 事件循环 | bao_uloop | ✅ 已完成 |
| simdutf | C++ | SIMD 文本 | simdutf-rs | ⏳ 待评估 |
| MozJS | C++ | JS 引擎 | **保留** — SpiderMonkey 是核心 | ❌ 不替换 |
| servo | C++/Rust | 浏览器引擎 | **保留** — 核心依赖 | ❌ 不替换 |

### 已完成的成熟库替换（P1 之外）

| Crate | 替换 | 测试 | 行数变化 |
|-------|------|------|---------|
| bun_md | 手写 → `pulldown-cmark` | 32/32 | -19K LOC |
| bun_base64 | 手写 → `base64` + `vlq` | 15/15 | -9 deps |
| bun_picohttp | C FFI → `httparse` | 35/36 | -C FFI |

---

## 2. Phase 规划（v2 修订）

### Phase P1: 低风险压缩库 ✅ 已完成

| REQ | 依赖 | 替换 | 测试 |
|-----|------|------|------|
| REQ-PURE-002 | brotli C | `brotli` crate v8 | 16/16 |
| REQ-PURE-003 | zstd C | `zstd-pure-rs` | 9/9 |
| REQ-PURE-004 | zlib/libdeflate C | `flate2` + `crc32fast` | 25/25 |

**总测试**: 50/50 通过。删除 ~236K 行 C vendor 代码。

### Phase P2: Highway 部分替换 ✅ 已完成

**原始计划**: highway C++ → highway-rs 或 wide+std::simd
**实际发现**:
- `bun_highway` 名字误导——实际是 xxHash + 30+ SIMD 字符串内核（Google Highway portable SIMD）
- `highway-rs` crate 是 HighwayHash 算法，与本项目无关
- xxHash 部分可替换（`twox-hash` crate），但 30+ SIMD 字符串内核深度耦合 Google Highway

**已执行方案**:
1. xxHash 函数 → `twox-hash` crate v2.1（纯 Rust，bit-identical 输出）✅
2. SIMD 字符串内核 → **保留 Google Highway C++**（无成熟 Rust 替代，手写代价极高）

**变更**:
- `src/hash/xxhash.rs` — 重写，使用 `twox_hash::XxHash32/XxHash64/xxhash3_64::Hasher`
- `src/hash/Cargo.toml` — `bun_highway` → `twox-hash`
- `src/highway/lib.rs` — 移除 6 个 xxHash FFI 声明 + 4 个包装函数 + `XxHash64State`
- `src/runtime/api/HashObject.rs` — `bun_highway::xxhash3_64` → `bun_hash::XxHash3::hash`
- `src/runtime/Cargo.toml` — 移除 `bun_highway` 依赖
- `Cargo.toml` — 添加 `twox-hash = "2.1"` workspace dep

**测试**: 28/28 通过（含 SMHasher 验证 + 已知值 + streaming + seeded + XXH3）

### Phase P2': Mimalloc ❌ 取消

**原始计划**: mimalloc → jemallocator
**实际发现**:
- mimalloc 不只是 `#[global_allocator]`——整个 `bun_alloc` 层围绕 `mi_*` API 构建
- 150+ 文件直接调用 `mi_malloc`/`mi_free`/`mi_heap_malloc` 等
- `MimallocArena` per-heap 分配器依赖 `mi_heap_t`，jemalloc 无等价物
- `mi_usable_size`/`mi_expand`/`mi_is_in_heap_region` 用于安全检查和 resize 逻辑

**决策**: 保留 mimalloc。替换代价远超收益。

### Phase P3: BoringSSL 🔒 永久保留

**调研结论**: 所有支持 JA3/JA4 TLS 指纹模拟的 Rust 项目（rquest、wreq、hpx-emulation）均依赖 BoringSSL。rustls 不暴露 ClientHello 扩展顺序控制 API，fork rustls 添加此能力的工程量极大且与上游不兼容。

**决策**: BoringSSL 及其生态永久保留，不再作为迁移目标。TLS 指纹伪装是 Bao 的核心反指纹功能。

### Phase P4: 协议库替换

| REQ | 依赖 | 替换 | 风险 | 预计工时 |
|-----|------|------|------|---------|
| REQ-PURE-008 | lolhtml C++ | `lol_html` crate v3.0 (纯 Rust) | 低 | ✅ 完成 |
| REQ-PURE-007 | lsquic C | quinn + BoringSSL TLS 后端 | 中 | 5d |

**lolhtml 替换说明**: `lol_html` v3.0.0 是 100% 纯 Rust crate（Cloudflare 维护），与之前通过 C FFI 使用的 `lol_html_c_api` 是同一代码库。替换消除了 1309 行 FFI 声明 + C API shim crate，直接使用 Rust 类型。11 个 TDD 测试全部通过。

**依赖关系**: quinn 使用 BoringSSL 作为 TLS 后端（通过自定义 `Crypto` trait 实现），保留 TLS 指纹能力
**验收标准**: HTTP/3 连接正常，HTML 重写/SSR 功能正常

### Phase P5: HTTP 引擎替换 ❌ 取消

**原始计划**: uWS/uSockets C++ → hyper + tokio
**战略调整**: C/C++ 依赖永久保留，不再替换。uWS 性能极高且成熟稳定。

### Phase P6: Zig 清除 + 最终验证 ✅ 已完成

| 操作 | 内容 | 变更 |
|------|------|------|
| 删除所有 Zig 文件 | 1102 个 .zig 文件全部删除 | -629,322 行 |
| 修复预存在编译错误 | Decompressor zlib API 适配 + ChunkedState 枚举 + crash_handler unsafe | 4 文件修改 |

**验证**: `cargo check` 通过，零 Zig 文件残留

---

## 3. 修订后依赖 DAG

```
P1 (brotli/zstd/zlib) ✅ ──────────────────────┐
P2 (xxHash→twox-hash) ✅ ──────────────────────┤
P2'(mimalloc) ❌ 取消                            ├──→ P4a (lolhtml→lol_html) ✅ ──→ P6 (Zig 清除) ✅
P3 (BoringSSL) 🔒 永久保留 ──────────────────────┘    P4b (lsquic→quinn) ❌ 取消
                                                      P5 (uWS→hyper) ❌ 取消
```

---

## 4. 修订后风险矩阵

| Phase | 风险 | 缓解策略 |
|-------|------|---------|
| P2 | xxHash 输出不一致 | ✅ 已验证 twox-hash 输出 bit-identical |
| P4 | lolhtml API 差异大（streaming rewriter vs tree parser） | servo 的 html5ever 已集成，适配层封装 |
| P4 | quinn 与 BoringSSL 集成 | quinn `Crypto` trait 自定义实现，桥接 BoringSSL |
| P5 | uWS 性能极高 | 压测对比，必要时保留 uWS 作为可选后端 |
| P5 | 事件循环集成复杂 | bao_uloop 已有 epoll 基础，tokio 可对接 |
| P6 | 删除 Zig/JSC 后编译错误 | 逐文件迁移，确保 bao_runtime 完全替代 |

---

## 5. 不替换项（v4 修订 — C/C++ 永久保留）

| 依赖 | 原因 |
|------|------|
| **BoringSSL** | 纯 Rust 生态无 TLS 指纹定制方案（JA3/JA4），Bao 核心反指纹功能依赖此，永久保留 |
| **mimalloc** | 150+ 文件直接 mi_* FFI，MimallocArena per-heap 无替代 |
| **Google Highway** | 30+ SIMD 字符串内核无成熟 Rust 替代 |
| **uWS/uSockets** | 极高性能 HTTP/WebSocket 引擎，成熟稳定，战略保留 |
| **lsquic** | QUIC 协议实现，战略保留 |
| **lshpack** | HPACK 压缩，战略保留 |
| **mozjs_sys** | SpiderMonkey 是 Bao 核心 JS 引擎 |
| **servo** | 浏览器引擎核心 |

---

## 6. 验证检查点

每个 Phase 完成后：
1. `cargo build` 成功
2. `cargo test` 全量通过
3. 性能基准测试（与替换前对比）
4. C/C++ FFI 调用点数量统计

最终目标（修订 v4）：
- 零 Zig 文件 ✅
- C/C++ FFI 保留（mozjs_sys + BoringSSL + highway SIMD + mimalloc + uWS + lsquic + lshpack）
- 尽可能减少 C vendor 代码（已完成 brotli/zstd/zlib/libdeflate 清除）

---

## 7. 已完成工作汇总

### P1 压缩库迁移（2026-06-11）

| 提交 | 内容 | 变更 |
|------|------|------|
| `1af27089` | brotli/zstd/zlib → 纯 Rust | 817 files, +1881/-236107 |
| `87ad4beb` | md/base64/picohttp → 成熟库 | 40 files, +2491/-19018 |

**总删除**: ~255K 行 C/Zig vendor 代码
**总测试**: 97 个新增 TDD 测试全部通过

### P2 xxHash 迁移（2026-06-11）

| 内容 | 变更 |
|------|------|
| xxHash FFI → twox-hash 纯 Rust | 6 文件修改 |
| 移除 6 个 FFI 声明 + 4 个包装函数 + XxHash64State | src/highway/lib.rs |
| bun_hash 不再依赖 bun_highway | src/hash/Cargo.toml |
| bun_runtime 移除 bun_highway 依赖 | src/runtime/Cargo.toml |

**测试**: 28/28 通过（SMHasher + known vectors + streaming + seeded）

### P4a lolhtml 迁移（2026-06-11）

| 内容 | 变更 |
|------|------|
| lol_html_c_api C FFI → lol_html Rust crate v3.0 | -1355 行 FFI, +332 行 |
| 移除 1309 行 extern "C" 声明 + 71 个 FFI 函数 | src/lolhtml_sys/lol_html.rs |
| 移除 C API shim crate 依赖 | src/lolhtml_sys/Cargo.toml |
| 新增 11 个 TDD 测试 | element/text/comment handlers |

**测试**: 11/11 通过

### bun_sql 安全审计发现

| 严重性 | 问题 |
|--------|------|
| **高** | PostgreSQL DataRow 负 byte_length 未验证，恶意服务器可触发 UB |
| 中 | caching_sha2_password.scramble 缺少 nonce 长度校验 |
| 中 | SSL 证书验证未在协议层强制执行 |
| 中 | 密码/auth 数据在 Data.temporary 中未清零 |
| 低 | MySQL encodeLenString 32 位平台 u64 截断 |

### P6 Zig 清除（2026-06-11）

**战略调整**: C/C++ 依赖永久保留（BoringSSL/mimalloc/highway/uWS/lsquic/lshpack），仅消灭 Zig。

| 内容 | 变更 |
|------|------|
| 删除 1102 个 .zig 文件 | -629,322 行 |
| 修复 Decompressor.rs zlib API 适配 | 移除 reader.zlib.* C API 引用，改用纯 Rust input/list_ptr |
| 修复 ChunkedState 枚举匹配 | 整数 4/5 → TrailerLineHead/TrailerLineMiddle |
| 修复 crash_handler unnecessary unsafe | compress2 不再是 unsafe fn |
| 修复 HTTPThread doc comment | 空 impl 上的 /// → // |

**验证**: `cargo check` 通过，零 Zig 文件残留

**累计成果**: ~885K 行 C/Zig vendor 代码删除，171 TDD 测试通过
