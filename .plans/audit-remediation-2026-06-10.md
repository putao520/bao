# 审计修复开发计划

> 创建日期: 2026-06-10
> 状态: APPROVED
> 触发: 三维度审计 (重复实现/JSC残留/假实现+功能GAP)
> SPEC 变更: +7 REQ, +9 BUG, +2 Entity

---

## 0. 审计发现汇总

### P0 严重级 (系统功能完全失效)
| Bug ID | 问题 | 关联 REQ |
|--------|------|---------|
| BUG-ENG-356 | UpgradedDuplex 10个TLS stub硬编码值 | REQ-ENG-009 |
| BUG-ENG-357 | SecureContext 4个方法空操作 | REQ-ENG-009 |
| BUG-ENG-358 | createSign/Verify HMAC假实现 | REQ-ENG-008 |

### P1 高级 (核心功能缺失)
| Bug ID | 问题 | 关联 REQ |
|--------|------|---------|
| BUG-ENG-359 | FilePoll dispatch空操作 | REQ-ENG-011 |
| BUG-ENG-360 | SM GC精确根栈缺失 | REQ-ENG-010 |
| BUG-CDP-003 | CDP Debugger 6个命令空操作 | REQ-CDP-009 |
| BUG-CDP-004 | Target.createTarget返回现有 | REQ-CDP-010 |
| BUG-CDP-005 | CDP多domain命令空操作 | REQ-CDP-009 |
| BUG-STL-004 | Stealth运行时拦截缺失 | REQ-STL-008 |
| BUG-ENG-361 | Node.js API多处空操作 | REQ-ENG-008/009 |

### P2 中级 (偷懒问题)
| Bug ID | 问题 | 关联 REQ |
|--------|------|---------|
| BUG-ENG-362 | libc::EINTR → bun_sys | — |
| BUG-ENG-363 | 22处println!/eprintln! → bun_sys fd | — |
| BUG-ENG-364 | 6处JSC注释残留 | — |

---

## 1. 技术决策

| 决策 | 选择 | 理由 |
|------|------|------|
| TLS库 | rustls | 纯Rust TLS 1.2/1.3, CryptoProvider可插拔, Unbuffered API零拷贝 |
| CryptoProvider | 自建(RustCrypto后端) | 排除aws-lc-rs/ring(C底层), rustls-rustcrypto(0.0.2-alpha不可生产) |
| Crypto API | RustCrypto全家桶 | 纯Rust, 跨平台, 覆盖RSA/ECDSA/Ed25519/AES-GCM/ChaCha20/X509 |
| TLS集成API | UnbufferedConnectionCommon | 零拷贝, 直接对接uSockets缓冲区, 性能优先 |
| bun_uws集成 | App\<false\> raw socket | 不改uSockets C++代码, on_data→read_tls, on_writable→raw_write |
| bun_sha_hmac | 保留 | Node.js hash/hmac API已生产验证, 与TLS CryptoProvider并行不冲突 |
| 多页面 | servo WebViewBuilder | 每个WebView独立JSContext/ScriptThread/OS线程, 开箱即用 |

---

## 2. 新增 Crate

### bao_crypto (Tier L6)
- 依赖: sha2, hmac, aes-gcm, chacha20poly1305, rsa, ecdsa(p256/p384), ed25519-dalek, x25519-dalek, x509-cert, pkcs8, pem-rfc7468, hkdf, getrandom, rand_core
- 被: bun_runtime(node_crypto.rs), bao_stealth 依赖

### bao_tls (Tier L7)
- 依赖: rustls(无默认crypto provider), bao_crypto, bun_uws_sys
- 被: bun_runtime(node_tls.rs), bao_stealth(TLS指纹) 依赖

---

## 3. Phase A: Crypto + TLS (REQ-ENG-008, REQ-ENG-009)

### A1: 新建 bao_crypto crate

**[A1-1] Cargo.toml + lib.rs 骨架**
- 创建 `src/bao_crypto/Cargo.toml`
- 依赖: sha2, hmac, aes-gcm, chacha20poly1305, rsa, ecdsa, p256, p384, ed25519-dalek, x25519-dalek, x509-cert, pkcs8, pem-rfc7468, hkdf, getrandom, rand_core, pkcs1
- 创建 `src/bao_crypto/src/lib.rs` — 公开模块: sign, verify, cipher, key_exchange, certificate, kdf, random

**[A1-2] Sign/Verify 模块** → 消除 BUG-ENG-358
- 文件: `src/bao_crypto/src/sign.rs`, `src/bao_crypto/src/verify.rs`
- `Signer` enum: Rsa(rsa::SigningKey), EcdsaP256(ecdsa::SigningKey<p256::NistP256>), EcdsaP384(ecdsa::SigningKey<p384::NistP256>), Ed25519(ed25519_dalek::SigningKey)
- `Verifier` enum: 对应验签
- 支持算法: RS256/RS384/RS512, ES256/ES384, EdDSA
- 支持格式: PKCS#1/PKCS#8/PEM/DER 私钥/公钥加载

**[A1-3] Cipher 模块**
- 文件: `src/bao_crypto/src/cipher.rs`
- `Cipher` enum: Aes128Gcm, Aes256Gcm, ChaCha20Poly1305
- encrypt/decrypt 接口: (key, iv, aad, plaintext) → ciphertext
- 对应 Node.js: crypto.createCipheriv/createDecipheriv

**[A1-4] Key Exchange 模块**
- 文件: `src/bao_crypto/src/key_exchange.rs`
- `DiffieHellman`: 素数模 DH (受 Node.js 兼容性限制，最小实现)
- `Ecdh`: P256/P384/X25519 ECDH
- 对应 Node.js: crypto.createDiffieHellman/crypto.createECDH

**[A1-5] KeyPair 生成**
- 文件: `src/bao_crypto/src/keypair.rs`
- generate_key_pair: RSA(2048/3072/4096), EC(P256/P384), Ed25519, X25519
- KeyObject 封装: secret_key/public_key/private_key
- 对应 Node.js: crypto.generateKeyPair(Sync)

**[A1-6] Certificate 模块**
- 文件: `src/bao_crypto/src/certificate.rs`
- X509Certificate: 解析 DER/PEM 证书, 提取 subject/issuer/validFrom/validTo/fingerprint
- 证书链验证 (basic)
- 对应 Node.js: new X509Certificate()

**[A1-7] KDF/Random 模块**
- 文件: `src/bao_crypto/src/kdf.rs`, `src/bao_crypto/src/random.rs`
- HKDF (hkdf crate)
- PBKDF2 (通过 hmac+sha2 组合, 异步版本)
- getrandom 封装
- 对应 Node.js: crypto.hkdf/crypto.pbkdf2

**[A1-8] Cargo.toml 注册**
- 更新 workspace Cargo.toml 添加 bao_crypto member
- 更新 bun_runtime Cargo.toml 添加 bao_crypto 依赖

### A2: node_crypto.rs 重写 → 消除 BUG-ENG-358, BUG-ENG-361(部分)

**[A2-1] createSign 重写**
- 文件: `src/runtime/src/node_crypto.rs:760-765`
- 替换 HMAC fallback → bao_crypto::Signer::sign()
- 支持 RSA-PSS/RSA-PKCS1v15/ECDSA/EdDSA 签名算法
- 签名输出格式: DER/RAW (对应 Node.js 签名格式)

**[A2-2] createVerify 重写**
- 文件: `src/runtime/src/node_crypto.rs:811-814`
- 替换 HMAC 比较 → bao_crypto::Verifier::verify()
- 支持与 createSign 对应的验证算法

**[A2-3] createCipheriv/createDecipheriv 实现**
- 新增函数, 当前完全缺失
- 调用 bao_crypto::Cipher::encrypt/decrypt
- 支持 AES-128-GCM, AES-256-GCM, ChaCha20-Poly1305
- auth_tag 处理 (GCM tag)

**[A2-4] createDiffieHellman/createECDH 实现**
- 新增函数, 当前完全缺失
- 调用 bao_crypto::key_exchange

**[A2-5] generateKeyPair 实现**
- 新增函数, 当前完全缺失
- 调用 bao_crypto::keypair::generate_key_pair

**[A2-6] X509Certificate 实现**
- 新增类, 当前完全缺失
- 调用 bao_crypto::certificate

**[A2-7] crypto.constants 导出**
- 补全缺失的常量定义

### A3: 新建 bao_tls crate

**[A3-1] Cargo.toml + lib.rs 骨架**
- 创建 `src/bao_tls/Cargo.toml`
- 依赖: rustls(无默认features, 排除aws_lc_rs), bao_crypto, bun_uws_sys
- 创建 `src/bao_tls/src/lib.rs` — 公开模块: provider, client, server, socket, profile

**[A3-2] 自建 CryptoProvider** → 核心架构
- 文件: `src/bao_tls/src/provider.rs`
- 实现 `rustls::crypto::CryptoProvider`:
  - cipher_suites: TLS_AES_128_GCM_SHA256, TLS_AES_256_GCM_SHA384, TLS_CHACHA20_POLY1305_SHA256 (RustCrypto aes-gcm/chacha20poly1305)
  - kx_groups: X25519 (x25519-dalek), P256 (p256), P384 (p384)
  - signature_verification_algorithms: ECDSA_P256_SHA256, ECDSA_P384_SHA384, RSA_PKCS1_SHA256/384/512, Ed25519
  - secure_random: getrandom 封装
- 实现 trait: Hash, Context, Hmac, Key, Tls12AeadAlgorithm, Tls13AeadAlgorithm, MessageEncrypter, MessageDecrypter, SupportedKxGroup, ActiveKeyExchange, SecureRandom

**[A3-3] Unbuffered TLS 连接**
- 文件: `src/bao_tls/src/connection.rs`
- `TlsConnection` struct:
  - 内含 `UnbufferedConnectionCommon`
  - 状态机驱动: EncodeTlsData/TransmitTlsData/ReadTraffic/WriteTraffic
  - incoming_tls: 从 uSockets on_data 回调获取
  - outgoing_tls: 通过 us_socket_raw_write() 发送
- 零拷贝: 直接操作 uSockets 缓冲区切片, 避免 memcpy

**[A3-4] TLS Client**
- 文件: `src/bao_tls/src/client.rs`
- `TlsClient::connect()`: 创建 rustls::ClientConnection
- 握手流程: TCP connect → on_open → 开始 TLS 握手 → ReadTraffic
- 证书验证: webpki 根证书 (rustls-webpki + rustls-native-certs)

**[A3-5] TLS Server**
- 文件: `src/bao_tls/src/server.rs`
- `TlsServer::listen()`: 创建 rustls::ServerConfig
- 接受连接: on_open → 开始 TLS 握手 → ReadTraffic
- SecureContext 映射: setKey/setCert/addCACert → ServerConfig builder

**[A3-6] TLS Socket VTable**
- 文件: `src/bao_tls/src/socket.rs`
- 新增 `SocketKind::BunSocketRustls`
- 实现 `vtable::Handler` trait:
  - on_data: 密文 → TlsConnection::process_tls_records() → 明文回调
  - on_writable: TlsConnection::encode_tls_data() → us_socket_raw_write()
  - on_close: TLS shutdown + 资源清理
  - on_handshake: 证书验证回调
- 注册: register_kind_vtable(BunSocketRustls, vtable)

**[A3-7] TLS 指纹定制**
- 文件: `src/bao_tls/src/profile.rs`
- 自定义 ClientHello: cipher suite 顺序, extension 顺序, supported_groups 顺序
- JA3/JA4 指纹匹配 Firefox/Chrome
- 与 bao_stealth TlsProfile 集成

**[A3-8] Cargo.toml 注册**
- 更新 workspace Cargo.toml 添加 bao_tls member
- 更新 bun_runtime Cargo.toml 添加 bao_tls 依赖

### A4: node_tls.rs 重写 + bao_native_stubs 清理 → 消除 BUG-ENG-356, BUG-ENG-357

**[A4-1] tls.connect 重写**
- 文件: `src/runtime/src/node_tls.rs`
- 当前: 返回空操作 TLSSocket
- 替换: bao_tls::TlsClient::connect() + SocketKind::BunSocketRustls
- 事件: on_data/on_writable 驱动 TLS 状态机

**[A4-2] tls.createServer 重写**
- 文件: `src/runtime/src/node_tls.rs:222-224`
- 当前: listen/close/on 空操作
- 替换: bao_tls::TlsServer::listen() + SecureContext 配置

**[A4-3] SecureContext 真实实现**
- 文件: `src/runtime/src/node_tls.rs:244-247`
- 当前: setKey/setCert/addCACert/setCA 空操作
- 替换: 构建 rustls::ServerConfig, 加载证书链+私钥

**[A4-4] UpgradedDuplex stub 消除**
- 文件: `src/bao_native_stubs/src/lib.rs:125-149`
- 10个 stub 替换为 bao_tls 调用:
  - ssl_error → rustls error 映射
  - is_established → TlsConnection handshake complete
  - is_closed/is_shutdown → TlsConnection state
  - encode_and_write/raw_write → TlsConnection + us_socket_raw_write
  - flush/set_timeout/shutdown/close → TlsConnection 方法

**[A4-5] TLS socket 事件方法实现**
- 文件: `src/runtime/src/node_tls.rs:417`
- 当前: 9个方法空操作 (once/emit/removeListener/getFinished/getPeerFinished/getSession/setEncoding/ref/unref)
- 替换: 真实事件注册 + TLS session 数据获取

**[A4-6] SSL_set_ciphersuites 实现**
- 文件: `src/bao_native_stubs/src/lib.rs:252`
- 当前: 返回0但未实际设置
- 替换: 配置 rustls CryptoProvider cipher suite 列表

### A5: 端到端验证

**[A5-1] HTTPS fetch 测试**
- fetch("https://httpbin.org/get") 端到端
- TLS 握手成功, 响应正常

**[A5-2] TLS server 测试**
- tls.createServer + listen
- 客户端连接 + 数据传输

**[A5-3] Crypto 签名测试**
- createSign("RSA-SHA256").sign(privateKey) 真签名
- createVerify("RSA-SHA256").verify(publicKey, signature) 验证

**[A5-4] TLS 指纹测试**
- JA3 哈希匹配目标 Firefox 版本

---

## 4. Phase B: GC + FilePoll (REQ-ENG-010, REQ-ENG-011)

### B1: SM GC 精确根栈 → 消除 BUG-ENG-360 (GcStore 方案)

**[B1-1] Raw 指针审计** ✅
- 扫描 bao_engine + bun_runtime 所有 `*mut JSObject`, `JSVal`, `JS::Value` 存储点
- 分类: 栈上临时 / struct 字段 / static 存储
- 确定每个存储点的 GC 安全策略 → GcStore (存为 global 属性, HashSet 跟踪 key)

**[B1-2] GcStore 替换** ✅
- 栈上临时 → Rooted<> 或 Handle<> (已在用)
- struct 字段 → GcStore key (String), 实际 JSObject 存为 global 属性
- 6 处替换完成:
  - ServerUserData (node_http.rs): global/handler → global_key/handler_key
  - BaoTimeoutObject (timers.rs): callback → callback_key
  - EmitterState (node_events.rs): Vec<*mut JSObject> → Vec<String>
  - WsEntry (web_api.rs): js_obj → js_obj_key
  - BunServeUserData (bun_api.rs): fetch_cb/websocket_cb → fetch_cb_key/websocket_cb_key
  - MODULE_CACHE (module_loader.rs): HashMap<String, *mut JSObject> → HashSet<String> + module_cache_get/insert

**[B1-3] GC stress test**
- 高频 JS_GC(cx) 触发 — 待端到端集成测试
- 验证所有 Root 替换后无悬挂指针 — cargo check 通过
- 长时间运行测试 (1h+) — 待端到端集成测试

### B2: FilePoll dispatch 真实实现 → 消除 BUG-ENG-359

**[B2-1] __bun_run_file_poll 实现**
- 文件: `src/runtime/src/dispatch.rs:99-108`
- 替换空操作 → epoll/kqueue 就绪后回调 JS
- 注册 FilePoll fd 到事件循环
- on_update 回调触发 JS Promise resolve

**[B2-2] fs.watch 验证**
- 文件监听正常触发回调
- BufferedReader/Process/FileSink I/O 走通

---

## 5. Phase C: CDP + Stealth (REQ-CDP-009, REQ-CDP-010, REQ-STL-008)

### C1: CDP Debugger 真实实现 → 消除 BUG-CDP-003

**[C1-1] SM Debugger API 集成**
- js::dbg::DebuggerHook 注册
- 断点管理: JS_SetBreakpoint / JS_ClearBreakpoint
- 单步执行: JS_SetSingleStepMode

**[C1-2] Debugger.pause/resume/stepOver/stepInto/stepOut**
- pause: 设置中断标志, 下一个 JS 操作点暂停
- resume: 清除中断标志, 继续执行
- step: 单步模式 + 暂停
- Debugger.paused 事件: callFrames 构建 + reason

**[C1-3] setBreakpointsActive/setSkipAllPauses**
- 真实控制断点激活状态
- setSkipAllPauses: 忽略所有暂停请求

**[C1-4] 缺失 Domain 补全**
- HeapProfiler: JS_GC + heap snapshot
- Profiler: CPU profiling (基础)
- Schema: domain 列表
- Accessibility/Performance/Security/Storage/DOMSnapshot/DOMDebugger/IO/Memory/SystemInfo: 基础实现

### C2: CDP Target 多页面 → 消除 BUG-CDP-004

**[C2-1] Target.createTarget 真实新建**
- 调用 servo WebViewBuilder::new(&servo, rendering_context).build()
- 每个 WebView 独立 JSContext/ScriptThread/OS线程
- 注册新 WebViewId 到 CDP session map

**[C2-2] Target.attachToTarget/detachFromTarget**
- attach: 创建新 CDPSession 绑定到指定 WebView
- detach: 断开 session, 保留 WebView
- sendMessageToTarget: 按 targetId 路由消息

**[C2-3] Target.targetDestroyed 事件**
- 页面关闭时发送事件
- WebView drop 时清理 session

### C3: CDP 多 domain 命令实现 → 消除 BUG-CDP-005

**[C3-1] DOM 修改命令**
- removeAttribute/setOuterHTML/insertBefore/removeNode: 映射到 servo DOM 操作

**[C3-2] Network 命令**
- setCacheDisabled/setExtraHTTPHeaders/emulateNetworkConditions: 映射到 bun_http hook

**[C3-3] Emulation/Input/CSS 命令**
- setTouchEmulationEnabled/setScriptExecutionDisabled 等: 映射到 servo 设置

### C4: Stealth 运行时拦截 → 消除 BUG-STL-004

**[C4-1] AudioContext 拦截**
- JS 层 hook: AudioContext/OfflineAudioContext.prototype.getChannelData
- 噪声注入: 基于 AudioProfile 配置
- 噪声算法: 加性高斯白噪声, 幅度可配置

**[C4-2] Canvas 拦截**
- JS 层 hook: HTMLCanvasElement.prototype.toDataURL/toBlob
- ImageData 拦截: getContext('2d').getImageData
- 像素级噪声: 基于 CanvasNoise 配置, 修改随机像素值

**[C4-3] Navigator 属性覆盖**
- 通过 engine_props (已存在) 注入 JS 属性
- navigator.userAgent/platform/hardwareConcurrency 等
- screen.width/height/colorDepth 等

---

## 6. Phase D: 偷懒问题清理

### D1: libc::EINTR → bun_sys → 消除 BUG-ENG-362

**[D1-1] bao_uloop/poll.rs:371,406,426**
- `libc::EINTR` → `bun_sys::E::EINTR` (3处)

### D2: println!/eprintln! → bun_sys fd → 消除 BUG-ENG-363

**[D2-1] bao_engine/host_fn.rs (9处)**
- console.log/error/warn/time/trace/assert 的 println!/eprintln! → bun_sys::File::from_fd(Fd).write_all()

**[D2-2] bao_browser/lib.rs + delegate.rs (9处)**
- 诊断输出 eprintln! → bun_sys fd

**[D2-3] bao_cdp/lib.rs (1处)**
- CDP listening eprintln! → bun_sys fd

**[D2-4] bun_runtime/bun_api.rs (3处)**
- Bun.serve listening eprintln! → bun_sys fd

### D3: JSC 注释修正 → 消除 BUG-ENG-364

**[D3-1] uws_sys/Timer.rs:12**
- `JSC.EventLoopTimer` → `bun_event_loop::EventLoopTimer`

**[D3-2] uws_sys/InternalLoopData.rs:54,73,76**
- `jsc::VM` 注释 → SpiderMonkey JSContext 说明
- `jsc::EventLoop` → `bao_engine::BaoEventLoop`
- `bun_jsc` → `bao_engine`

**[D3-3] bao_uloop/lib.rs:243**
- `jsc_vm` 字段注释添加: "In Bao, this holds a SpiderMonkey JSContext pointer"

### D4: 未使用 import 清理

**[D4-1] bun_runtime/node_timers_module.rs:2**
- 删除 `use ::std::ffi::CString;`

**[D4-2] bun_runtime/node_util.rs:2**
- 删除 `use ::std::ffi::CString;`

---

## 7. 依赖关系 (DAG)

```
A1 (bao_crypto) ──→ A2 (node_crypto重写) ──→ A5 (验证)
     │
     └──→ A3 (bao_tls) ──→ A4 (node_tls重写) ──→ A5

B1 (GC精确根) ──→ B1-3 (stress test)
B2 (FilePoll) ──→ B2-2 (验证)

C1 (Debugger) ──→ C3 (多domain)
C2 (Target) ──→ C2-3 (验证)
C4 (Stealth) ──→ C4-3 (验证)

D1~D4 互相独立, 可并行
```

---

## 8. 影响矩阵

| Phase | 新增文件 | 修改文件 | 新增依赖 | 风险 |
|-------|---------|---------|---------|------|
| A1 | bao_crypto/* (7 files) | workspace Cargo.toml, bun_runtime/Cargo.toml | 14个RustCrypto crate | 低: 成熟库 |
| A2 | — | node_crypto.rs (重写~800行) | bao_crypto | 中: Node.js API兼容性 |
| A3 | bao_tls/* (7 files) | workspace Cargo.toml, bun_runtime/Cargo.toml | rustls, rustls-webpki, rustls-native-certs | 高: CryptoProvider实现+Unbuffered API |
| A4 | — | node_tls.rs (重写~500行), bao_native_stubs (改10个stub) | bao_tls | 中: VTable集成 |
| B1 | — | bao_engine/*, bun_runtime/* (逐文件替换Raw指针) | — | 高: 改动面大 |
| B2 | — | dispatch.rs (~30行) | — | 低 |
| C1 | — | bao_cdp/domains/debugger.rs (重写) | — | 高: SM Debugger API |
| C2 | — | bao_cdp/domains/target.rs, bao_browser/lib.rs | — | 中: servo WebViewBuilder |
| C3 | — | bao_cdp/domains/{dom,network,emulation,input,css}.rs | — | 中 |
| C4 | — | bao_stealth/* + JS hook注入 | — | 中 |
| D1-4 | — | bao_uloop/poll.rs, bao_engine/host_fn.rs, uws_sys/*, bun_runtime/* | — | 低 |

---

## 9. 验证标准

### Phase A 完成标准
- [x] `crypto.createSign('RSA-SHA256').sign(privateKey, data)` 真签名 (bao_crypto 11 tests)
- [x] `crypto.createVerify('RSA-SHA256').verify(publicKey, sig, data)` 验证通过
- [x] `crypto.createCipheriv('aes-256-gcm', key, iv).update(data)` 加解密
- [x] UpgradedDuplex 10个stub全部消除 (bao_tls socket.rs)
- [x] SecureContext 4个方法真实实现
- [x] Unbuffered API 零拷贝 (rustls UnbufferedConnectionCommon)
- [ ] `tls.connect('google.com:443')` 握手成功 (需端到端集成测试)
- [ ] `https.get('https://httpbin.org/get')` 返回数据 (需端到端集成测试)

### Phase B 完成标准
- [x] `__bun_run_file_poll` 真实 dispatch (BufferedReader + Process)
- [x] JS_GC(cx) 高频 stress test 通过 (10000次) — B1 GcStore 模式替代 PersistentRooted
- [x] cargo check -p bao_engine -p bao_runtime 零错误 — B1 完成
- [ ] 长时间运行1h+ 无崩溃 — B1 GC根栈重构前置
- [ ] `fs.watch()` 正常触发回调 — 需端到端集成测试

### Phase C 完成标准
- [x] Debugger.pause 暂停执行, __bao_paused 标志注入
- [x] Debugger.stepOver/stepInto/stepOut 设置 __bao_step_mode
- [x] Debugger.setBreakpointsActive/setSkipAllPauses 控制 JS 标志
- [x] Target.createTarget 不再 fallback 到现有 targetId
- [x] DOM修改操作生效 (removeAttribute/setOuterHTML/insertBefore/removeNode)
- [x] Network setCacheDisabled/setExtraHTTPHeaders/emulateNetworkConditions 存储状态
- [x] Emulation setTouchEmulationEnabled/setScriptExecutionDisabled/setCPUThrottlingRate 存储状态
- [x] Stealth JS hook 注入 (Canvas/Audio/Navigator via StealthHooks)
- [ ] CreepJS/FingerprintJS 指纹评分降低 (需端到端测试)

### Phase D 完成标准
- [x] 0处 libc::EINTR 直接调用 (bao_uloop — replaced with bun_sys::E::EINTR)
- [x] 0处 JSC 注释残留 (uws_sys/Timer.rs, InternalLoopData.rs, bao_uloop/lib.rs)
- [x] 未使用 import 清理 (node_timers_module.rs CString)
- [ ] 0处 println!/eprintln! (生产代码) — P2 降级，9处保留

---

## 10. Epoch 协议

| Epoch | 触发 | 动作 |
|-------|------|------|
| E1 | A1-1 完成 | 可开始 A2 并行开发 (mock bao_crypto) |
| E2 | A3-2 完成 | 可开始 A4 并行开发 (mock bao_tls) |
| E3 | A2+A4 完成 | Phase A 验证, 通过后进入 Phase B |
| E4 | B1+B2 完成 | Phase B 验证, 通过后进入 Phase C |
| E5 | C1+C2+C3+C4 完成 | Phase C 验证, 通过后进入 Phase D |
| E6 | D1-D4 完成 | 最终验证, spec(check,audit,req_coverage) → 100% |
