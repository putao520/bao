# 架构性能升级计划

> 创建日期: 2026-06-11
> 状态: IN-PROGRESS
> 触发: 深度架构分析 — "Bun 翻译" → "Pure Rust 高性能应用"
> SPEC 变更: +5 REQ-PERF-*

---

## 0. 分析摘要

| 维度 | 严重 | 高 | 中 | 低 |
|------|------|----|----|-----|
| 反模式 | 2 | 2 | 2 | 0 |
| 性能瓶颈 | 2 | 2 | 1 | 0 |
| Rust特性缺失 | 1 | 2 | 2 | 2 |
| 成熟库缺失 | 2 | 2 | 2 | 1 |
| SPEC差距 | 1 | 1 | 2 | 1 |
| **合计** | **8** | **9** | **9** | **4** |

---

## 1. Phase 1: P0 — 去锁化 + HTTP 零拷贝 (REQ-PERF-001, REQ-PERF-002)

### 1a: CDP Domain Handler 去锁化 (REQ-PERF-002) — ✅ COMPLETED

**结论**：`Cell`/`RefCell` 因 `DomainHandler: Send + Sync` 约束不可行。Handler 在 `Arc<DomainRegistry>` 中跨 session 共享，`handle_command(&self)` 可被多线程并发调用。

**实际完成状态**：
- `emulation.rs`: 已使用 `AtomicBool`/`AtomicU64`，无需改动
- `network.rs`: `Mutex<Value>`/`Mutex<Option<NetworkConditions>>` — 跨线程共享，`Mutex` 合理
- `debugger.rs`: `Mutex<HashMap>` ×2 — 跨线程共享，`Mutex` 合理
- `fetch_domain.rs`: `Mutex<Vec<Value>>` — 跨线程共享，`Mutex` 合理

**原计划 `Cell`/`RefCell` 替代方案不可行的原因**：`DomainHandler` trait 要求 `Send + Sync`，`Cell`/`RefCell` 不满足 `Sync`。即使当前 `CdpServer::run()` 是单线程事件循环，trait bound 是设计契约，未来多线程扩展时 `RefCell` 会 panic。

### 1b: HTTP 响应路径零拷贝 (REQ-PERF-001) — ✅ COMPLETED

**实际完成状态**：
- `http_client.rs`: `status_text: CompactString`, `headers: SmallVec<[(CompactString, CompactString); 8]>`, `body: Bytes`
- 零拷贝验证测试已通过（`test_body_take_no_clone` 验证 `Bytes::from(Vec)` 复用同一分配）
- Workspace deps: `bytes=1`, `compact_str=0.8`, `smallvec=1` 已在 `Cargo.toml`

---

## 2. Phase 2: P1 — 成熟库 + enum dispatch (REQ-PERF-003, REQ-PERF-004)

### 2a: 引入 bytes/compact_str/smallvec/dashmap (REQ-PERF-003) — ✅ COMPLETED

**实际完成状态**：
- `runtime_bridge.rs`: `NODE_REALMS`/`PAGE_GLOBALS` 已用 `DashMap<usize, usize>`
- `runtime_bridge.rs`: `Arc<OnceLock<EvaluateResult>>` 已替代 `Arc<Mutex<EvaluateResult>>`
- `http_client.rs`: `Bytes`/`CompactString`/`SmallVec` 已应用
- `router.rs`: `RefCell<HashMap<String, Rc<SessionInner>>>` — 单线程 `!Send`，`DashMap` 不适用

### 2b: CDP enum dispatch + OnceLock (REQ-PERF-004) — ⏸️ DEFERRED

**原因**：`Box<dyn DomainHandler>` → enum dispatch 是大规模重构（12 个 handler 各有不同字段），虚表→直接分派的性能收益有限（CDP 命令频率低，热点在 JS 注入和 bridge 通信）。`OnceLock` 已在 `runtime_bridge.rs` 完成。

---

## 3. Phase 3: P2 — 消除 Bun 翻译残留 (REQ-PERF-005)

### 3a: Box::new([]) → Vec::new() — ✅ COMPLETED

**实际完成状态**：grep 确认零 `Box::new([])` 实例。已自然清理。

### 3b: Option<Box<T>> → Option<T> — 🔄 IN-PROGRESS

**文件**: `node_events.rs:240`, `node_url.rs:194`
- `Option<Box<EmitterState>>` → `Option<EmitterState>`
- `Option<Box<UrlState>>` → `Option<UrlState>`

### 3c: C 风格索引循环 → 迭代器适配器 — 🔄 IN-PROGRESS

**文件**: host_fn.rs (4处), node_path.rs (2处), node_crypto.rs (3处), web_api.rs (1处)

### 3d: thread_local clone → take()/replace() — 🔄 IN-PROGRESS

**文件**: node_crypto.rs (9处可优化), module_loader.rs (4处保持 clone — 共享读取正确)

---

## 4. 执行顺序

```
Phase 1a (CDP去锁) ✅ → Phase 1b (HTTP零拷贝) ✅ → Phase 2a (成熟库) ✅ → Phase 2b (enum dispatch) ⏸️ → Phase 3 (残留清除) 🔄
```

---

## 5. 验证标准

- [x] Phase 1a: Mutex 在 Send+Sync 约束下是必要的，emulation 已原子化
- [x] Phase 1b: HTTP 响应路径零拷贝（CompactString + SmallVec + Bytes）
- [x] Phase 2a: bytes/compact_str/smallvec/dashmap 正确引入
- [ ] Phase 2b: DEFERRED — 枚举分派收益有限
- [ ] Phase 3b: 零 Option<Box<T>>（T: Sized）
- [ ] Phase 3c: 零 C 风格索引循环
- [ ] Phase 3d: 零不必要的 thread_local clone
- [ ] cargo check 全部通过
