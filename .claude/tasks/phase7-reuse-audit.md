# Phase 7: SPEC ↔ Code 复用对齐方案

## 审计时间: 2026-05-30
## 审计范围: SPEC 设计 vs 实际代码 vs 可复用 Crate

---

## 一、SPEC 设计偏差（SPEC 描述了理想架构但未实现）

| # | SPEC 约束 | 实际状态 | 类型 | 严重度 |
|---|----------|---------|------|--------|
| S1 | 02-SYSTEM §4: "Bun Infrastructure (100% 复用, 零修改)" | 0% 使用 bun_event_loop/bun_uws/bun_resolver/bun_http | SPEC-VIOLATION | CRITICAL |
| S2 | 02-SYSTEM §4.1: 12 个 `bao_engine_*` 桥接 crate | 仅存在 1 个 bao_engine，其余不存在 | SPEC-ASPIRATIONAL | HIGH |
| S3 | 03-PROCESS §5: "Bun Event Loop (uSockets) → epoll/kqueue" | 无 uSockets 使用，手写 TimerHeap + sleep | SPEC-VIOLATION | CRITICAL |
| S4 | 05-IMPLEMENTATION §1.3: "Timer 桥接: 委托 Bun Timer (uSockets)" | 手写 thread::sleep 轮询 | SPEC-VIOLATION | CRITICAL |
| S5 | 05-IMPLEMENTATION §1.4: "ESM resolve → Bun resolver" | 手写 resolve_specifier | SPEC-VIOLATION | HIGH |
| S6 | 05-IMPLEMENTATION §1.3: "I/O 回调桥接: epoll/kqueue" | 无 epoll/kqueue 抽象 | SPEC-ASPIRATIONAL | HIGH |

## 二、代码偏离 SPEC（代码手写而非复用）

| # | 手写模块 | LOC | SPEC 约束 | 应复用 Crate | JSC 依赖 | 迁移可行性 |
|---|---------|-----|----------|-------------|---------|-----------|
| C1 | timers.rs (TimerHeap + sleep) | 391 | REQ-ENG-004: "Bun Timer 管理" | bun_event_loop (MiniEventLoop) | 零硬依赖 | ✅ 高 |
| C2 | node_http.rs (TcpListener) | 686 | 02-SYSTEM: "HTTP 栈" | bun_uws + bun_picohttp | 零 | ✅ 高 |
| C3 | require.rs (resolve_specifier) | 348 | REQ-ENG-005: "Bun resolver" | bun_resolver | 零 | ✅ 高 |
| C4 | module_loader.rs (resolve) | ~100 | REQ-ENG-005: "Bun resolver" | bun_resolver | 零 | ✅ 高 |
| C5 | node_url.rs (URL 解析) | 871 | 02-SYSTEM: "URL 解析" | bun_url | Soft | ✅ 高 |
| C6 | node_fs.rs (base64) | ~50 | — | bun_base64 (SIMD) | 零 | ✅ 高 |
| C7 | node_dns.rs (DNS 解析) | 484 | — | bun_dns | Soft | ✅ 高 |
| C8 | node_child_process.rs | 672 | — | bun_spawn | 零 | ✅ 高 |
| C9 | 全模块 (std::fs 直接调用) | ~8 处 | — | bun_io | Soft | ✅ 高 |
| **合计** | **~3,610 LOC** | | | | | |

## 三、根因分析

### 为什么 SPEC 设计没被遵守？

1. **Phase 1 快速验证策略**：先用最小手写实现验证 SM 引擎可行性，复用被标记为 "Phase 8 未来迁移"
2. **SPEC 过度设计**：12 个桥接 crate 在 Phase 1 不需要，可以简化为 bao_runtime monolith
3. **JSC 依赖恐惧**：误以为 bun_event_loop 必须 JSC，实际 MiniEventLoop 零 JSC 依赖
4. **渐进式开发**：功能先跑通，复用放后续。但 CLAUDE.md 明确禁止这种行为

### 真实优先级（复用价值 × 可行性）

```
P0 (立即执行):  Event Loop → bun_event_loop (MiniEventLoop)
                HTTP Server → bun_uws
                Resolver → bun_resolver

P1 (第二轮):   URL Parsing → bun_url
                Base64 → bun_base64
                DNS → bun_dns

P2 (第三轮):   Child Process → bun_spawn
                I/O → bun_io
                String Encoding → bun_string_encoding
```

## 四、SPEC 更新方案

### 4.1 需要更新的 SPEC 文件

| 文件 | 更新内容 |
|------|---------|
| 02-SYSTEM.html §4.1 | 删除 12 个 bao_engine_* bridge crate，保留 bao_runtime monolith + 复用映射表 |
| 03-PROCESS.html §5 | 更新 Event Loop 桥接描述：MiniEventLoop 集成方案 |
| 05-IMPLEMENTATION.html §1.3 | 更新 Timer 桥接：bun_event_loop MiniEventLoop 而非 "JSC RunLoop" |
| 05-IMPLEMENTATION.html §1.4 | 更新 Module Loader：bun_resolver 集成方案 |
| 10-REQUIREMENTS.html | REQ-ENG-004 验收标准补充 bun_event_loop 复用约束 |
| CLAUDE.md | 迁移计划表更新：Phase 8 → Phase 7（现在执行）|

### 4.2 SPEC 新增内容

| 新增 | 内容 |
|------|------|
| 02-SYSTEM.html §N | "可复用 Crate 集成映射" — 每个 bun_* crate 的集成点、API 桥接方式、JSC 依赖度 |
| 03-PROCESS.html §N | "bun_event_loop MiniEventLoop 集成流程" — tick 循环、timer firing、I/O polling |

## 五、代码迁移方案

### 5.1 P0-1: Event Loop 迁移 (timers.rs → bun_event_loop)

**当前**: `TimerHeap` + `thread::sleep` + 手动 drain
**目标**: `MiniEventLoop` 集成

```rust
// 当前 (手写)
pub fn drain_and_check(cx: &mut JSContext) -> bool {
    accept_connections();
    poll_http_requests(cx);
    drain_timers(cx);
    if has_pending_timers() {
        wait_for_next_timer();  // thread::sleep!
        true
    } else { ... }
}

// 目标 (bun_event_loop)
pub fn drain_and_check(cx: &mut JSContext) -> bool {
    let loop_ref = mini_event_loop.uws_loop();
    // uSockets 单次 tick: epoll/kqueue + timer + I/O
    loop_ref.tick();  // 零 sleep！事件驱动
    drain_timers(cx);
    JobQueue::drain(cx);
    has_pending_work()
}
```

**改动范围**: timers.rs (~391 行 → ~150 行), runtime.rs (post_eval_hook)
**JSC 依赖**: 零。MiniEventLoop 不需要任何 JS 引擎

### 5.2 P0-2: HTTP Server 迁移 (node_http.rs → bun_uws)

**当前**: `std::net::TcpListener` + 手写 HTTP 请求解析 + 手写响应格式化
**目标**: `bun_uws::App` HTTP server + `bun_picohttp` 解析

```rust
// 当前 (手写 686 行)
let listener = TcpListener::bind(addr)?;
loop {
    let (stream, _) = listener.accept()?;
    // 手写 HTTP 解析...
}

// 目标 (bun_uws)
let app = bun_uws::App::new()
    .get("/*", |res, req| { ... })
    .post("/*", |res, req| { ... });
app.listen(addr, None);
```

**改动范围**: node_http.rs (~686 行 → ~200 行)
**JSC 依赖**: 零。bun_uws 是纯 C/Rust (uSockets 封装)

### 5.3 P0-3: Module Resolution 迁移 (require.rs → bun_resolver)

**当前**: 手写 `resolve_specifier` + `resolve_node_modules` + `try_resolve`
**目标**: `bun_resolver::Resolver` 集成

```rust
// 当前 (手写 348 行)
fn resolve_specifier(specifier: &str, base_dir: Option<&Path>) -> Option<PathBuf> {
    // 手写 .js/.mjs/.json/.ts/.tsx 后缀尝试
    // 手写 node_modules 向上搜索
    // 手写 index.js 回退
}

// 目标 (bun_resolver)
let resolver = Resolver::new(ResolutionMode::Bundler, ...);
let resolved = resolver.resolve(specifier, base_dir)?;
```

**改动范围**: require.rs (~348 行 → ~50 行 resolve 部分)
**JSC 依赖**: 零。bun_resolver 是纯 Rust

### 5.4 迁移后预期效果

| 指标 | 当前 | 迁移后 |
|------|------|--------|
| bao_runtime LOC | ~15,000 | ~8,000 |
| 手写事件循环 | TimerHeap + sleep | bun_event_loop (epoll/kqueue) |
| HTTP 性能 | 同步阻塞 | uSockets 异步 |
| 模块解析完整度 | 5 种后缀 + node_modules | 完整 Bun resolver (ESM/TS/package.json/tsconfig) |
| SPEC 符合度 | ~60% | ~95% |

## 六、执行计划

### Phase 7A: SPEC 更新 (architect 执行)
1. 更新 02-SYSTEM.html §4.1 — 删除 12 bridge crate，加复用映射表
2. 更新 03-PROCESS.html §5 — MiniEventLoop 集成流程
3. 更新 05-IMPLEMENTATION.html — Phase 7 迁移任务
4. 更新 CLAUDE.md — Phase 8 → Phase 7
5. spec_lint + spec_audit 验证

### Phase 7B: Event Loop 迁移 (programmer 执行)
1. 添加 bun_event_loop 依赖到 bao_runtime/Cargo.toml
2. 创建 event_loop.rs — MiniEventLoop 集成
3. 替换 timers.rs 的 TimerHeap + sleep
4. 更新 runtime.rs post_eval_hook
5. 全量测试通过

### Phase 7C: HTTP 迁移 (programmer 执行)
1. 添加 bun_uws + bun_picohttp 依赖
2. 重写 node_http.rs — uWS App
3. 保持 API 兼容 (http.createServer, http.request)
4. 全量测试通过

### Phase 7D: Resolver 迁移 (programmer 执行)
1. 添加 bun_resolver 依赖
2. 替换 require.rs + module_loader.rs 的 resolve_specifier
3. 全量测试通过

### Phase 7E: P1 级迁移 (URL/Base64/DNS)
1. bun_url 替换 node_url.rs 手写解析
2. bun_base64 替换外部 base64 crate
3. bun_dns 替换 node_dns.rs 手写解析
4. 全量测试通过

### Phase 7F: 验证
1. spec_lint health — 0 errors
2. spec_audit maturity — 100%
3. Z3 state_machine — SOUND
4. 全量测试 — ALL PASS
5. architect(task_type=review) — 独立审查

## 七、风险与依赖

| 风险 | 影响 | 缓解 |
|------|------|------|
| bun_event_loop 编译需要特殊 feature flag | 可能增加编译时间 | 先用 MiniEventLoop，最小化依赖 |
| bun_uws 需要 C 编译 (uSockets) | 交叉编译复杂 | 已在 Bun workspace 编译通过 |
| bun_resolver 依赖较多 Bun crate | 增加依赖树 | 仅引入 resolve 核心功能 |
| 迁移期间测试可能中断 | 测试套件暂时失败 | 逐模块迁移，每步全量验证 |

---

## 附录: 当前测试基线

- **881 assertions**, 25 suites, ALL PASS
- **SPEC Maturity**: 100.0%
- **Z3**: PageLifecycle SOUND, WebViewLifecycle SOUND
- **Shannon Entropy**: 95.43%
- **SPEC Lint**: HEALTHY (0 errors)
- **编译**: 零 bao crate warning
