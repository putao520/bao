# 开发计划: 复用审计违规消除 | epoch: 2 | status: active

## 范围（现有 SPEC REQ，不新建）
REQ-ENG-007/006、REQ-CLI-001、REQ-CDP-001、REQ-CDP-UWS-001

## 用户决策
- DEC-ENG-002: 含 AES-CBC/CTR（完整 Node crypto）
- DEC-ENG-003: pbkdf2 统一 bao_crypto::kdf，删 sha_hmac/pbkdf2.rs

## 任务（文件域 crate 级 disjoint，并行）
- TASK-1-CRYPTO: bao_crypto 全量 Node crypto 接入（BoringSSL）+ kdf 统一
- TASK-2-BUNDLER: bao_bundler 接入 bun_bundler
- TASK-3-RUNTIME: bao_runtime 复用迁移（buffer/path/ws/dns/tls）
- TASK-4-CDP: bao_cdp 清理死代码
- TASK-5-CDP-CLIENT: bao_cdp_client ws.rs 迁移 bun_uws

## 铁律
1. 文件域：只改 task.file，禁改 bun_* 上游 crate（含 bun_boringssl_sys — 需 BoringSSL FFI 时在 bao_crypto 自加 extern "C" 绑定 link libboringssl，不改 bun_boringssl_sys 源码）
2. 编译验证：每 task cargo check 通过
3. 复用优先 > 直接替换不留兼容层 > 无 TODO/FIXME/stub
