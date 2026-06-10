# Pure Rust 迁移开发升级方案

> 创建日期: 2026-06-11
> 状态: APPROVED
> 触发: 用户决策 — 所有 C/C++/Zig 依赖替换为纯 Rust 库
> SPEC 变更: +10 REQ-PURE-*, +1 NFR-PURE-001

---

## 0. 迁移动机

Bao 已完全独立于 Bun/Servo 上游，拥有自己的 fork 仓库。消除所有 C/C++/Zig 原生依赖可：
1. **统一内存安全模型** — 所有代码在 Rust 安全保证下运行
2. **消除供应链攻击面** — 不再依赖 C/C++ vendor 代码
3. **简化交叉编译** — 纯 Rust 零工具链依赖（除 mozjs_sys）
4. **降低构建复杂度** — 不再需要 CMake/ninja/clang 等构建工具

---

## 1. 当前 C/C++/Zig 依赖清单

| 依赖 | 语言 | 用途 | 当前 crate | 目标替换 |
|------|------|------|-----------|---------|
| BoringSSL | C++ | TLS 1.2/1.3 | bun_boringssl_sys | rustls + RustCrypto |
| brotli | C | 压缩/解压 | bun_brotli_sys | rust-brotli (纯 Rust) |
| zstd | C | 压缩/解压 | bun_zstd_sys (via zstd-sys) | zstd-rs (纯 Rust feature) |
| zlib/libdeflate | C | gzip/deflate | bun_zlib_sys, bun_libdeflate_sys | miniz_oxide / flate2 (纯 Rust) |
| mimalloc | C | 内存分配器 | bun_mimalloc_sys | jemallocator 或 Rust 默认 |
| highway | C++ | Hash (CityHash) | bun_highway | wide + std::simd 或 highway-rs |
| lsquic | C | QUIC 协议 | bun_lsquic_sys | quinn (纯 Rust) |
| lolhtml | C++ | HTML 重写 | bun_lolhtml_sys | html5ever (servo 内置) |
| uWS/uSockets | C++ | HTTP/WebSocket | bun_uws_sys | hyper + tokio |
| lshpack | C | HPACK 压缩 | bun_lshpack_sys | hyper 内置 HPACK |
| libuv | C | 事件循环 | bun_libuv_sys | bao_uloop (已完成) |
| simdutf | C++ | SIMD 文本 | bun_simdutf_sys | simdutf-rs (纯 Rust feature) |
| spawn | C | 进程管理 | bun_spawn_sys | std::process + Rust |
| TCC | C | JIT 编译 | bun_tcc_sys | cranelift (纯 Rust) |
| cares | C | DNS 解析 | bun_cares_sys | trust-dns-resolver (hickory) |
| MozJS | C++ | JS 引擎 | mozjs_sys | **保留** — SpiderMonkey 是核心 |

---

## 2. 迁移 Phase 规划

### Phase P1: 低风险压缩库 (无 API 变更)

| REQ | 依赖 | 替换 | 风险 | 预计工时 |
|-----|------|------|------|---------|
| REQ-PURE-002 | brotli C | rust-brotli | 低 | 2d |
| REQ-PURE-003 | zstd C | zstd-rs (wasm-safe feature) | 低 | 1d |
| REQ-PURE-004 | zlib/libdeflate C | miniz_oxide + flate2 | 低 | 2d |

**依赖关系**: 互相独立，可并行
**验收标准**: 所有现有 brotli/zstd/gzip/deflate 测试通过，性能回归 ≤ 5%

### Phase P2: Hash + 内存分配器

| REQ | 依赖 | 替换 | 风险 | 预计工时 |
|-----|------|------|------|---------|
| REQ-PURE-005 | mimalloc | jemallocator 或 Rust 默认 | 中 | 1d |
| REQ-PURE-006 | highway C++ | wide/std::simd 或 highway-rs | 低 | 1d |

**依赖关系**: 互相独立
**验收标准**: hash 输出完全一致，分配器性能基准测试

### Phase P3: TLS 核心替换

| REQ | 依赖 | 替换 | 风险 | 预计工时 |
|-----|------|------|------|---------|
| REQ-PURE-001 | BoringSSL | rustls + RustCrypto | 高 | 10d |

**依赖关系**: 无前置，但这是最高风险项
**详细方案**: 见 `.plans/audit-remediation-2026-06-10.md` Phase A (A1-A5)
**验收标准**:
- `tls.connect('google.com:443')` 握手成功
- `https.get()` 返回数据
- TLS 指纹可定制 (JA3/JA4)
- 零 BoringSSL 符号

### Phase P4: 网络协议

| REQ | 依赖 | 替换 | 风险 | 预计工时 |
|-----|------|------|------|---------|
| REQ-PURE-007 | lsquic C | quinn | 中 | 5d |
| REQ-PURE-008 | lolhtml C++ | html5ever | 中 | 3d |

**依赖关系**: P3 完成后开始（QUIC 需要 TLS）
**验收标准**:
- HTTP/3 连接正常
- HTML 重写/SSR 功能正常

### Phase P5: HTTP 引擎替换

| REQ | 依赖 | 替换 | 风险 | 预计工时 |
|-----|------|------|------|---------|
| REQ-PURE-009 | uWS/uSockets C++ | hyper + tokio | 极高 | 20d |

**依赖关系**: P3 + P4 完成后开始
**详细说明**: 这是最大规模的替换，涉及：
- bun_uws → bun_http_hyper
- uws_callback 宏 → hyper Service trait
- 所有 HTTP server/client API 重写
- WebSocket (uWS → tokio-tungstenite)
- 事件循环集成 (bao_uloop → tokio runtime)

**验收标准**:
- Bun.serve() HTTP 服务正常
- fetch() HTTP 客户端正常
- WebSocket 服务端/客户端正常
- 性能基准 ≤ 10% 回退

### Phase P6: 清理 + 最终验证

| REQ | 依赖 | 替换 | 风险 | 预计工时 |
|-----|------|------|------|---------|
| REQ-PURE-010 | bun_runtime (Zig+JSC) | bao_runtime (纯 Rust+SM) | 中 | 3d |

**依赖关系**: P1-P5 全部完成后
**验收标准**:
- 零 Zig 文件
- 零 JSC 引用
- C/C++ FFI 调用点 ≤ 1 (仅 mozjs_sys)
- `cargo build` 零 C 工具链依赖

---

## 3. 依赖 DAG

```
P1 (brotli/zstd/zlib) ──┐
P2 (highway/mimalloc) ──┤
                         ├──→ P3 (TLS/rustls) ──→ P4 (QUIC/lolhtml) ──→ P5 (HTTP/hyper) ──→ P6 (清理)
```

P1, P2 可并行。P3 可与 P1/P2 并行启动但风险高需专注。

---

## 4. 风险矩阵

| Phase | 风险 | 缓解策略 |
|-------|------|---------|
| P1 | 性能回退 | 基准测试对比，miniz_oxide SIMD 路径 |
| P2 | hash 输出不一致 | 先验证输出完全一致再切换 |
| P3 | rustls CryptoProvider 不完整 | 先实现最小可用集，逐步扩展 |
| P3 | TLS 指纹伪装难度 | rustls 支持自定义 ClientHello 扩展顺序 |
| P4 | quinn API 与 lsquic 差异大 | 适配层封装，渐进替换 |
| P5 | uWS 性能极高，hyper 可能不及 | 压测对比，必要时保留 uWS 作为可选后端 |
| P5 | 事件循环集成复杂 | bao_uloop 已有 epoll 基础，tokio 可对接 |
| P6 | bun_runtime 删除后编译错误 | 逐文件迁移，确保 bao_runtime 完全替代 |

---

## 5. 不替换项

| 依赖 | 原因 |
|------|------|
| mozjs_sys | SpiderMonkey 是 Bao 核心 JS 引擎，C++ 实现无法替换 |
| servo | 浏览器引擎核心，C++ + Rust 混合，保留 |
| libuv | 已被 bao_uloop 替代 |
| cares | 迁移到 hickory-dns (trust-dns-resolver) |

---

## 6. 验证检查点

每个 Phase 完成后：

1. `cargo build` 成功
2. `cargo test` 全量通过
3. SPEC `spec(check,audit,req_coverage)` 验证
4. 性能基准测试（与替换前对比）
5. FFI 调用点数量统计（目标：逐 Phase 递减）

最终目标：
- C/C++ FFI 调用点 ≤ 1（仅 mozjs_sys）
- 零 CMake/ninja/clang 构建依赖
- 纯 `cargo build` 可编译
- NFR-PURE-001 达标

---

## 7. 与审计修复计划的关系

`.plans/audit-remediation-2026-06-10.md` 中的 Phase A（Crypto+TLS）与本方案的 P3 重叠。
P3 直接使用审计修复计划的详细设计（bao_crypto + bao_tls）。

执行优先级：先完成审计修复的 Phase A-D，再按 P1→P2→P4→P5→P6 推进纯 Rust 迁移。
