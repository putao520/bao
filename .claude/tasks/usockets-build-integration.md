# uSockets C 编译集成方案

## 研究完成: 2026-05-30

## 问题
bun_uws_sys / bun_base64 / bun_simdutf_sys 等 workspace crate 是纯 FFI 绑定，
C native library 由 Bun 的 Ninja 构建系统编译，Cargo 无法感知。
bao_bin 链接时找不到 C 符号 (us_loop_run_bun_tick, simdutf__base64_encode 等)。

## 解决方案: 为 bun_uws_sys 创建 build.rs

### Phase A — C 核心文件
```rust
// src/uws_sys/build.rs — 用 cc crate 编译 uSockets
cc::Build::new()
    .files(&[
        "{bun}/packages/bun-usockets/src/socket.c",
        "{bun}/packages/bun-usockets/src/loop.c",
        "{bun}/packages/bun-usockets/src/bsd.c",
        "{bun}/packages/bun-usockets/src/context.c",
        "{bun}/packages/bun-usockets/src/udp.c",
        "{bun}/packages/bun-usockets/src/eventing/epoll_kqueue.c",
        "{bun}/packages/bun-usockets/src/crypto/openssl.c",
    ])
    .define("LIBUS_USE_OPENSSL", "1")
    .define("LIBUS_USE_BORINGSSL", "1")
    .define("WITH_BORINGSSL", "1")
    .include("{bun}/packages/bun-usockets")
    .include("{bun}/packages/bun-usockets/src")
    .include("{bao}/src/uws_sys")
    .compile("usockets");
```

### Phase B — C++ 封装器
```rust
cc::Build::new()
    .cpp(true)
    .files(&[
        "{bun}/packages/bun-usockets/src/crypto/sni_tree.cpp",
        "{bun}/packages/bun-usockets/src/crypto/root_certs.cpp",
        "{bun}/packages/bun-usockets/src/crypto/root_certs_linux.cpp",
        "{bao}/src/uws_sys/libuwsockets.cpp",
    ])
    .include("{bun}/packages/bun-uws/src")
    .flag("-std=c++17")
    .flag("-fno-exceptions")
    .flag("-fno-rtti")
    .compile("uwsockets_cpp");
```

### 关键依赖
- BoringSSL (从 bun_boringssl 链接)
- pthread, dl (系统库)
- cc crate (build dependency)

### 一旦解决，解锁的迁移
| Crate | 替代 | 预期收益 |
|-------|------|---------|
| bun_uws_sys | timers.rs event loop | uSockets timer (epoll/kqueue) |
| bun_base64 | 5处 base64 crate 调用 | SIMD 加速 |
| bun_simdutf_sys | 字符串验证 | SIMD UTF-8/ASCII |

### 风险
1. BoringSSL 链接可能需要额外配置
2. C++ 编译需要 g++ 和适当的标准库
3. 跨平台需要 cfg(target_os) 选择源文件
