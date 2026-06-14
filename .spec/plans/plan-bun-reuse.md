# 开发计划: BUN 复用违规修复(6 项) | epoch: 1 | status: active

## 范围

修复审计发现的 6 项 BUN crate 复用违规,统一到 Bun workspace 内部基础设施:
- bao_cdp 使用 tungstenite/base64/sha1 外部 crate(违反核心铁律)
- bao_stealth 使用 sha2 外部 crate
- bao_runtime/node_url.rs 手写 2127 行 URL 实现(Cargo.toml 声明 bun_url 但未使用)
- bao_runtime/node_dns.rs 手写 514 行 DNS(Cargo.toml 声明 bun_dns 但未使用)
- bao_* 中 rand::random / rand::thread_rng 用于密码学(非 CSPRNG)
- bao_browser/bao_cdp 大量 thread::sleep 轮询(应事件驱动)

## 影响矩阵

| SPEC ID | 关联 TASK | 文件 |
|---------|----------|------|
| BUG-CDP-007 (新增) | TASK-1, TASK-3 | bao_cdp/src/lib.rs, backend.rs, ws.rs |
| BUG-STEALTH-001 (新增) | TASK-2 | bao_stealth/src/tls.rs |
| BUG-RUNTIME-001 (新增) | TASK-4 | bao_runtime/src/node_url.rs |
| BUG-RUNTIME-002 (新增) | TASK-5 | bao_runtime/src/node_dns.rs, fetch_api.rs |
| BUG-RUNTIME-003 (新增) | TASK-6 | bao_runtime/src/globals.rs, node_crypto.rs, web_api.rs |
| BUG-ARCH-001 (新增) | TASK-7, TASK-8 | bao_browser/src/*, bao_cdp/src/* |

## 任务树

### TASK-1: P0-3 — bao_stealth/tls.rs 切 bun_sha_hmac(改动最小,先做验证流程)
- SPEC: BUG-STEALTH-001 [验收] | 文件: src/bao_stealth/src/tls.rs, src/bao_stealth/Cargo.toml | 实现: 删除 `sha2 = "0.10"` 依赖,把 `sha2::{Sha256, Digest}` 替换为 `bun_sha_hmac::SHA256` | 验收: cargo build -p bao_stealth 通过 + 现有测试通过 | 状态: pending
- 复用锚点:
  - spec: [BUG-STEALTH-001 新建]
  - code: [bao_runtime/src/node_crypto.rs:175-200 (参考用法)]
  - pattern: [sha_hmac::SHA256::init() → update() → r#final(out)]
  → 无匹配项标注 `新建`

### TASK-2: P2-10 — rand:: → BoringSSL CSPRNG(独立,可并行)
- SPEC: BUG-RUNTIME-003 [验收] | 文件: bao_runtime/src/globals.rs:1439-1475, node_crypto.rs:358,517,545, web_api.rs:45,165 | 实现: 替换 `rand::random` / `rand::thread_rng().fill_bytes` 为 `boringssl_sys::RAND_bytes` 包装函数 | 验收: 密码学随机数测试通过(UUID v4 / mask_key / crypto.randomBytes) | 状态: pending
- 复用锚点:
  - code: [boringssl_sys::boringssl::RAND_bytes(buf, len)]
  - pattern: [bao_crypto/src/random.rs (随机数模块)]
  → 创建 bao_crypto::random::rand_bytes(buf: &mut [u8]) 包装

### TASK-3: P0-1 — bao_cdp 切 bun_uws + bun_base64 + bun_sha_hmac(影响最大)
- SPEC: BUG-CDP-007 [验收] | 文件: bao_cdp/Cargo.toml, bao_cdp/src/lib.rs(836行), backend.rs(287行), ws.rs(111行) | 实现: ① Cargo.toml 删 tungstenite/base64/sha1,加 bun_uws/bun_picohttp/bun_base64/bun_sha_hmac ② tungstenite WebSocket → bun_uws App + WebSocketBehavior ③ base64:: → bun_base64:: ④ sha1:: → bun_sha_hmac::SHA1 | 验收: cargo test -p bao_cdp 全部通过 | 依赖: TASK-1 (验证 bun_sha_hmac 用法) | 状态: pending
- 复用锚点:
  - code: [bao_runtime/src/node_http.rs (参考 bun_uws::App 用法)]
  - code: [bao_runtime/src/globals.rs:829-982 (参考 bun_base64 用法)]
  - code: [bao_runtime/src/node_crypto.rs:175-213 (参考 bun_sha_hmac 用法)]
  - pattern: [bun_uws::App<false>::new().ws("/*", WebSocketBehavior{...}).listen(port, ...)]
  → tungstenite 是同步 Rust WS,bun_uws 是 C++ uWS 异步,API 改造量大

### TASK-4: P1-4 — node_url.rs 切 bun_url(部分迁移)
- SPEC: BUG-RUNTIME-001 [验收] | 文件: bao_runtime/src/node_url.rs(2127行) | 实现: **关键约束**: bun_url 是只读 URL(只 getters),不能完全替代。策略: ① parse_url → bun_url::URL::from_utf8() ② 所有 getter (url_get_protocol/host/pathname/search/...) → bun_url::URL getters ③ setter 部分保留手写(set_url_state/rebuild_href)或迁移到 servo_url ④ url_encode/url_decode → bun_string_encoding 或 url crate | 验收: URL parse + getter 全部通过 bun_url,setter 行为兼容 | 状态: pending
- 复用锚点:
  - spec: [BUG-RUNTIME-001 新建]
  - code: [bun_url::URL (只读), OwnedURL]
  - code: [bao_browser (servo_url 已用)]
  - pattern: [URL::from_utf8(input) → NonNull<URL> → .protocol()/.hostname()/.port()/.pathname()/.search()/.hash()/.username()/.password() → .deinit()]
  → 注: bun_url 无 setter,setter 部分需保留手写或迁移 servo_url

### TASK-5: P1-5 — node_dns.rs 切 bun_dns
- SPEC: BUG-RUNTIME-002 [验收] | 文件: bao_runtime/src/node_dns.rs(514行), fetch_api.rs:252-260 | 实现: ① std::net::ToSocketAddrs → bun_dns::GetAddrInfo ② fetch_api.rs:252 TcpStream::connect_timeout 端口检测 → bun_dns::lookup_host + bun_io 连接 ③ libc::getaddrinfo → bun_dns | 验收: node_dns 测试通过 + fetch 端口超时检测 | 状态: pending
- 复用锚点:
  - spec: [BUG-RUNTIME-002 新建]
  - code: [bun_dns::GetAddrInfo, GetAddrInfoResult]
  - code: [bun_dns::internal::prefetch(loop, hostname, port)]
  - pattern: [GetAddrInfo::default() → resolve → ResultList → Address[]]
  → bun_dns 基于 c-ares,需要 EventLoop 集成

### TASK-6: P3-11 — bao_browser thread::sleep → servo 事件回调
- SPEC: BUG-ARCH-001 [验收] | 文件: bao_browser/src/lib.rs:109,128, page.rs:87,235,344, page_pool.rs:131 | 实现: ① browser ready 轮询 → servo 启动回调 ② page 加载等待 → onload/onloadend 事件 ③ page_pool idle → 事件驱动清理 | 验收: 无 thread::sleep,bao_browser 测试通过 | 状态: pending
- 复用锚点:
  - spec: [BUG-ARCH-001 新建]
  - code: [bao_browser/src/delegate.rs (servo delegate)]
  - pattern: [WebViewDelegate::on_load_complete / onload / on_history_change 等回调]
  → 架构性改动,需要 servo delegate 桥接

### TASK-7: P3-12 — bao_cdp thread::sleep → 条件变量/事件
- SPEC: BUG-ARCH-001 [验收] | 文件: bao_cdp/src/lib.rs, ws.rs, backend.rs, servo_bridge.rs, domains/* 共 10+ 处 | 实现: CDP 命令等待 sleep 轮询 → Condvar / bun_event_loop 事件 | 验收: 无 thread::sleep,bao_cdp 测试通过 | 状态: pending
- 复用锚点:
  - spec: [BUG-ARCH-001 新建]
  - code: [bao_cdp/src/lib.rs:51 std::sync::Mutex<Option<WebSocket>>]
  - pattern: [std::sync::Condvar::wait_timeout / bun_event_loop 事件等待]
  → 需要先完成 TASK-3 (CDP 重写),所以依赖 TASK-3

### TASK-8: 最终测试 — cargo test --workspace 连续3次通过
- SPEC: 全部 [验收] | 实现: cargo test --workspace --exclude mozjs* --exclude bun_uws_sys (排除 C++ build),连续3次通过 | 验收: 3/3 pass | 依赖: TASK-1..TASK-7 全部 completed | 状态: pending

## Bug 日志

(空,执行中追加)

## REQ 台账

| REQ ID | 验收标准 | 关联 TASK | 闭合状态 |
|--------|---------|----------|---------|
| BUG-CDP-007 | bao_cdp Cargo.toml 无 tungstenite/base64/sha1,用 bun_uws/bun_base64/bun_sha_hmac | TASK-3 | pending |
| BUG-STEALTH-001 | bao_stealth Cargo.toml 无 sha2,用 bun_sha_hmac | TASK-1 | pending |
| BUG-RUNTIME-001 | node_url.rs URL parse 用 bun_url | TASK-4 | pending |
| BUG-RUNTIME-002 | node_dns.rs 用 bun_dns | TASK-5 | pending |
| BUG-RUNTIME-003 | 密码学场景无 rand::,用 BoringSSL RAND_bytes | TASK-2 | pending |
| BUG-ARCH-001 | bao_browser + bao_cdp 无 thread::sleep 轮询 | TASK-6, TASK-7 | pending |
