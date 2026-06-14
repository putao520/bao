# Bao Zig→Rust 翻译质量重构计划（RFQ 系列）

> **状态**: 草稿 | **创建**: 2026-06-12 | **SPEC**: .spec/02-SYSTEM.html
> **废弃声明**: REQ-PERF-001~005 被 RFQ 系列完全替代

---

## 0. 审计基线

| 指标 | 当前值 | 目标值 | 减幅 |
|------|--------|--------|------|
| `unsafe` 块 | 28,053 | ≤19,600 | -30% |
| `unwrap()` | 9,979 | ≤1,000 | -90% |
| `clone()` | 4,214 | ≤850 | -80% |
| `#[repr(C)]` | 1,419 | ≤425 | -70% |
| `Vec::new()` 无预分配 | 3,154 | ≤315 | -90% |
| PORT 注解 | 19,407 | ≤1,940 | -90% |
| Miri UB 报告 | 未检测 | 0 | 100% |
| `std::mem::transmute` | ~200+ | ≤10 | -95% |
| `detach_lifetime` | ~30+ | 0 | 100% |
| TODO/FIXME | ~50+ | 0 | 100% |

## 0.1 Crate 分类

| 类别 | Crate 数量 | 说明 | 重构策略 |
|------|-----------|------|---------|
| C FFI 绑定 | ~15 | mozjs_sys/uws_sys/spawn_sys 等 | unsafe 保留，加 SAFETY 注释 |
| Bao 自写 | 6 | bao_engine/browser/cdp/stealth/runtime/native_stubs | 部分重构 |
| Bun 移植 | ~60 | bun_http/resolver/install/event_loop 等 | **重点重构目标** |

---

## Phase 0: 基础设施（无代码变更）

**目标**: 建立质量度量和检测工具链
**前置依赖**: 无
**可并行**: 3 个 REQ 全部可并行

### TASK-001: Miri CI 集成 (REQ-RFQ-001)

**原子任务**:
- [ ] 001-1: 创建 `.github/workflows/miri.yml`
  - nightly toolchain 安装
  - `rustup component add miri`
  - `cargo miri test -p bao_engine`
  - `cargo miri test -p bao_runtime`
  - `cargo miri test -p bao_cdp`
  - `cargo miri test -p bao_browser`
  - `cargo miri test -p bao_stealth`
- [ ] 001-2: 对不支持 Miri 的测试添加 `#[cfg_attr(miri, ignore)]`
- [ ] 001-3: 创建 `MIRI_SUPPRESSIONS.md` 记录已知 false positive
- [ ] 001-4: CI merge gate 配置（miri fail → merge blocked）

**验证**: TEST-RFQ-001
**产出**: CI workflow + suppression 文档

### TASK-002: 基准测试框架 (REQ-RFQ-002)

**原子任务**:
- [ ] 002-1: `Cargo.toml` 添加 `criterion = { version = "0.5", features = ["html_reports"] }` dev-dep
- [ ] 002-2: 创建 `benches/` 目录
- [ ] 002-3: 实现 `benches/http_response.rs` — HTTP 响应构建基准
- [ ] 002-4: 实现 `benches/cdp_message.rs` — CDP JSON 解析/序列化基准
- [ ] 002-5: 实现 `benches/js_string.rs` — JS→Rust 字符串转换基准
- [ ] 002-6: 实现 `benches/module_resolve.rs` — 模块解析基准
- [ ] 002-7: 实现 `benches/crypto_hash.rs` — SHA-256/512 吞吐基准
- [ ] 002-8: 实现 `benches/timer_heap.rs` — IntrusiveHeap 插入/弹出基准
- [ ] 002-9: 运行全量基准，记录 `benches/baseline.json`
- [ ] 002-10: CI 集成：`cargo bench` 自动运行 + artifact 上传

**验证**: TEST-RFQ-002
**产出**: 6 个基准测试 + 基线数据

### TASK-003: 质量审计工具链 (REQ-RFQ-003)

**原子任务**:
- [ ] 003-1: 创建 `.clippy.toml` 配置自定义 lint
  - `disallowed-methods = ["std::mem::transmute", ...]`
  - `cognitive-complexity-threshold = 10`
- [ ] 003-2: 创建 `deny.toml` (cargo-deny 配置)
  - 禁止新 C 依赖引入
  - 许可证合规检查
- [ ] 003-3: 创建 `scripts/quality-audit.sh`
  - 统计 unsafe 块数量
  - 统计 unwrap() 数量
  - 统计 clone() 数量
  - 统计 repr(C) 数量
  - 统计 Vec::new() 无预分配数量
  - 统计 PORT 注解数量
- [ ] 003-4: 运行 `scripts/quality-audit.sh` 记录 `quality-baseline.json`
- [ ] 003-5: CI 集成：质量门控脚本

**验证**: TEST-RFQ-003
**产出**: 审计脚本 + 基线数据 + CI 配置

---

## Phase 1: 内存安全 — lifetime UB 根治

**目标**: 消除所有 detach_lifetime 和 transmute 相关 UB
**前置依赖**: Phase 0 (TASK-001 Miri 就绪)
**可并行**: TASK-011 完成后 TASK-012/013 可并行

### TASK-011: 消除 detach_lifetime (REQ-RFQ-011) — P0

**受影响 Crate**: bao_engine, bao_runtime, bun_core

**原子任务**:
- [ ] 011-1: `grep -rn 'detach_lifetime\|transmute_lifetime' src/` 定位所有调用点
- [ ] 011-2: 按 crate 分类每个调用点：
  - bao_engine/src/context.rs
  - bao_engine/src/conversions.rs
  - bao_engine/src/module_loader.rs
  - bao_runtime/src/globals.rs
  - bao_runtime/src/bun_api.rs
  - bao_runtime/src/web_api.rs
  - bao_runtime/src/node_*.rs (全部)
  - bun_core/src/*.rs
- [ ] 011-3: 逐个替换：
  - **索引模式**: 引用 → `usize` index，通过 `&arena[idx]` 访问
  - **Arena 模式**: `typed_arena` / `bumpalo` 统一管理 lifetime
  - **所有权转移**: 传 `move` 而非传引用
  - **Cow 模式**: `Cow<'a, str>` 替代 `&'static str` 欺骗
- [ ] 011-4: `cargo miri test -p bao_engine` 验证
- [ ] 011-5: `cargo miri test -p bao_runtime` 验证
- [ ] 011-6: `cargo test` 全量通过

**验证**: TEST-RFQ-011
**风险**: 高（lifetime 变更可能引发级联编译错误）

### TASK-012: raw pointer 安全化 (REQ-RFQ-012) — P1

**受影响 Crate**: bao_engine, bao_runtime, bao_browser, bao_cdp

**原子任务**:
- [ ] 012-1: `grep -rn 'unsafe' src/bao_engine/ src/bao_runtime/ src/bao_browser/ src/bao_cdp/` 统计当前 unsafe 块
- [ ] 012-2: 分类每个 unsafe 块：
  - **必要**: SpiderMonkey FFI → 保留 + SAFETY 注释
  - **可消除**: 手动指针算术 → Cell/RefCell/NonNull
  - **可缩小**: 大块中只有 1-2 行需要 → 提取为单独 unsafe 块
- [ ] 012-3: 逐个 crate 执行替换：
  - `bao_cdp/domains/*.rs`: Mutex → Cell/RefCell（已知单线程）
  - `bao_engine/*.rs`: `*mut T` → `NonNull<T>` + `PhantomData`
  - `bao_runtime/*.rs`: `mem::uninitialized` → `MaybeUninit`
  - `bao_browser/*.rs`: slice 构造 → `slice::from_ref`/`from_mut`
- [ ] 012-4: 每个 unsafe 块添加 `// SAFETY:` 注释
- [ ] 012-5: `cargo miri test` 验证零新 UB
- [ ] 012-6: `cargo test` 全量通过

**验证**: TEST-RFQ-012
**风险**: 中（FFI 层 unsafe 必须保留）

### TASK-013: transmute 安全化 (REQ-RFQ-013) — P1

**受影响 Crate**: bao_engine, bao_runtime, bun_http, bun_install

**原子任务**:
- [ ] 013-1: `grep -rn 'std::mem::transmute' src/` 定位所有 transmute 调用
- [ ] 013-2: `Cargo.toml` 添加 `bytemuck = "1"` + `zerocopy = "0.8"` 依赖
- [ ] 013-3: 逐个替换：
  - `&T → &[u8]` 类型双关 → `bytemuck::bytes_of`
  - `*mut T → *mut U` → `ptr::cast` / `NonNull::cast`
  - `&[u8; N] → Struct` → `zerocopy::FromBytes::read_from`
  - `lifetime 欺骗` → Phase 1 的索引/arena 模式
  - `整数类型转换` → `From<T>/TryFrom<T>`
- [ ] 013-4: `cargo miri test` 验证
- [ ] 013-5: `cargo test` 全量通过

**验证**: TEST-RFQ-013
**风险**: 中（需仔细审查每个 transmute 的语义）

---

## Phase 2: 内存安全 — 别名 UB 根治

**目标**: 消除所有指针/切片别名冲突
**前置依赖**: Phase 1 (TASK-011)
**可并行**: TASK-021 完成后 TASK-022/023 可并行

### TASK-021: &mut + raw pointer 别名消除 (REQ-RFQ-021) — P1

**受影响 Crate**: bao_engine, bun_http, bun_uws

**原子任务**:
- [ ] 021-1: `cargo miri test -- -Zmiri-tag-raw-pointers` 检测当前别名冲突
- [ ] 021-2: 按 Miri 报告逐个定位 `&mut self` + `*mut T` 共存点
- [ ] 021-3: 修复策略：
  - 方法拆分：raw pointer 操作提取到独立 unsafe 方法
  - UnsafeCell 封装：内部可变性明确表达
  - Pin 约束：自引用结构体用 `Pin<Box<T>>`
- [ ] 021-4: `cargo miri test` 验证（Stacked Borrows 模式）
- [ ] 021-5: `cargo test` 全量通过

**验证**: TEST-RFQ-021

### TASK-022: slice 别名消除 (REQ-RFQ-022) — P1

**受影响 Crate**: bun_http, bun_picohttp, bao_runtime/fetch_api, bao_runtime/http_client

**原子任务**:
- [ ] 022-1: 定位所有同时持有 `&[]` 和 `&mut []` 的代码模式
- [ ] 022-2: 逐个替换：
  - `split_at_mut` / `split_first_mut` 安全分割
  - `Cell<[u8]>` 共享可变 slice
  - 索引模式：传 `(start, len)` 而非切片引用
  - `Bytes::slice` 不可变零拷贝引用
- [ ] 022-3: `cargo miri test` 验证（Tree Borrows 模式）
- [ ] 022-4: `cargo test` 全量通过

**验证**: TEST-RFQ-022

### TASK-023: UnsafeCell 安全封装 (REQ-RFQ-023) — P1

**受影响 Crate**: bao_engine, bao_runtime, bao_cdp, bun_core

**原子任务**:
- [ ] 023-1: `grep -rn 'UnsafeCell::get()' src/` 定位所有裸 UnsafeCell 访问
- [ ] 023-2: 逐个替换：
  - Copy 类型 → `Cell<T>`
  - 非 Copy 类型 → `RefCell<T>`
  - 一次性写入 → `OnceCell<T>`
  - 跨线程 → `AtomicBool` / `AtomicUsize`
  - 性能关键 + SAFETY 证明 → 保留 UnsafeCell + SAFETY 注释
- [ ] 023-3: `cargo miri test` 验证
- [ ] 023-4: `cargo test` 全量通过

**验证**: TEST-RFQ-023

---

## Phase 3: 错误处理 — unwrap 消除 + Result 传播

**目标**: 9,979 处 unwrap → Result，建立类型化错误体系
**前置依赖**: Phase 1-2（先消除 UB，再做错误处理改进）
**可并行**: TASK-031 → TASK-032 → TASK-033 串行（有依赖关系）

### TASK-031: unwrap/expect 消除 (REQ-RFQ-031) — P1

**受影响 Crate**: 全 workspace (~80 crate)

**原子任务**:
- [ ] 031-1: `grep -rn '\.unwrap()' src/ | wc -l` 确认基线数量
- [ ] 031-2: 按 crate 分批处理（每批 5-10 个 crate）：
  - **FFI 绑定层** (mozjs_sys 等): 保留 unwrap + SAFETY 注释
  - **JS 桥接层** (bao_engine/bao_runtime): 改为 `Result + JS 异常抛出`
  - **网络/IO 层** (bun_http/bun_uws): 改为 `Result + 错误日志`
  - **解析层** (bun_resolver/bun_install): 改为 `Result + 用户友好错误`
- [ ] 031-3: 每批完成后 `cargo test -p <crate>` 验证
- [ ] 031-4: 最终 `cargo test` 全量通过
- [ ] 031-5: `grep -rn '\.unwrap()' src/ | wc -l` 确认目标值 ≤1,000

**验证**: TEST-RFQ-031
**风险**: 高（级联类型签名变更，函数签名需改返回类型）

### TASK-032: Panic 边界定义 (REQ-RFQ-032) — P2

**原子任务**:
- [ ] 032-1: 创建 `docs/PANIC_BOUNDARY.md` 定义 panic 边界规则
- [ ] 032-2: 审计所有 `panic!` / `assert!` / `todo!` / `unimplemented!` 调用
- [ ] 032-3: 非 FFI/测试 panic 添加 `// PANIC:` 注释
- [ ] 032-4: 无合理理由的 panic 改为 `Result`

**验证**: TEST-RFQ-032

### TASK-033: 错误类型标准化 (REQ-RFQ-033) — P2

**原子任务**:
- [ ] 033-1: 设计错误类型层级：
  ```
  BaoError → EngineError / NetworkError / CryptoError / ResolveError / Io
  ```
- [ ] 033-2: 选择实现方式：`thiserror` crate 或手写 Error enum
- [ ] 033-3: 实现 `BaoError` 及子类型
- [ ] 033-4: 逐 crate 替换 `Err(String)` → `Err(BaoError::*)`
- [ ] 033-5: 实现 Display/Debug/Error trait
- [ ] 033-6: JS 错误桥接：BaoError → JS 异常消息

**验证**: TEST-RFQ-033

---

## Phase 4: 性能优化 — 零拷贝热路径

**目标**: 消除不必要 clone/堆分配，达到原生 Rust 性能
**前置依赖**: Phase 0 (TASK-002 benchmark 就绪)
**可并行**: TASK-041/042/044 可并行，TASK-043 依赖 TASK-041

### TASK-041: Bytes/BytesMut 引入 (REQ-RFQ-041) — P1

**受影响 Crate**: bun_http, bao_runtime, bao_cdp

**原子任务**:
- [ ] 041-1: workspace `Cargo.toml` 添加 `bytes = "1"` 依赖
- [ ] 041-2: `bun_http` HttpResponse.body: `Vec<u8>` → `Bytes`
- [ ] 041-3: `bao_runtime/fetch_api`: 响应体零拷贝
- [ ] 041-4: `bao_runtime/http_client`: 请求/响应 buffer → `BytesMut`
- [ ] 041-5: `bao_cdp` CDP 响应体 → `Bytes`
- [ ] 041-6: 运行 `cargo bench --bench http_response` 对比基线
- [ ] 041-7: `cargo test` 全量通过

**验证**: TEST-RFQ-041
**性能目标**: HTTP 响应路径 ≥5x 吞吐提升

### TASK-042: CompactString/SmallVec (REQ-RFQ-042) — P1

**受影响 Crate**: bao_cdp, bun_http, bun_url, bao_runtime

**原子任务**:
- [ ] 042-1: 确认 `compact_str` / `smallvec` 已在 workspace deps
- [ ] 042-2: `bao_cdp` session ID / event method → `CompactString`
- [ ] 042-3: `bun_http` headers → `SmallVec<[(CompactString, CompactString); 8]>`
- [ ] 042-4: `bun_url` pathname / query → `CompactString`
- [ ] 042-5: `bao_runtime` JS 字符串桥接 → `CompactString`
- [ ] 042-6: 运行 criterion 基准对比
- [ ] 042-7: `cargo test` 全量通过

**验证**: TEST-RFQ-042
**性能目标**: 短字符串堆分配减少 ≥80%

### TASK-043: clone() 消除 (REQ-RFQ-043) — P1

**受影响 Crate**: 全 workspace

**原子任务**:
- [ ] 043-1: `grep -rn '\.clone()' src/ | wc -l` 确认基线
- [ ] 043-2: 分类每个 clone：
  - **可 move**: 最后一次使用 → 改为 move（~1,500 处）
  - **可 borrow**: 只读引用 → 改为 `&`（~1,200 处）
  - **零拷贝**: 大 buffer → `Bytes::slice` / `take()`（~800 处）
  - **必须保留**: 多所有者（~700 处）→ 添加注释
- [ ] 043-3: 按优先级分批处理（热点路径优先）
- [ ] 043-4: 运行 criterion 基准对比
- [ ] 043-5: `cargo test` 全量通过

**验证**: TEST-RFQ-043
**性能目标**: 热路径内存分配减少 ≥50%

### TASK-044: Vec::new() → with_capacity (REQ-RFQ-044) — P2

**受影响 Crate**: 全 workspace

**原子任务**:
- [ ] 044-1: `grep -rn 'Vec::new()\|Vec::with_capacity(0)\|vec!\[\]' src/` 定位
- [ ] 044-2: 逐个评估合理预分配大小
- [ ] 044-3: 替换为 `Vec::with_capacity(N)`
- [ ] 044-4: 运行 criterion 基准对比
- [ ] 044-5: `cargo test` 全量通过

**验证**: TEST-RFQ-044
**性能目标**: 热路径 realloc 次数减少 ≥70%

---

## Phase 5: 代码质量 — Zig 翻译痕迹清除

**目标**: 清理 repr(C)/PORT/死代码，达到原生 Rust 惯用风格
**前置依赖**: Phase 1 (TASK-013 transmute 消除后 repr(C) 不再被依赖)
**可并行**: TASK-051/052/053 全部可并行

### TASK-051: repr(C) 审计 (REQ-RFQ-051) — P2

**原子任务**:
- [ ] 051-1: `grep -rn '#\[repr(C)\]' src/ | wc -l` 确认基线
- [ ] 051-2: 逐个评估：
  - SpiderMonkey FFI 结构体 → 保留 + 注释 `// FFI: mozjs binding`
  - uSockets/uWS FFI 结构体 → 保留 + 注释 `// FFI: uws binding`
  - 纯 Rust 内部结构体 → 删除 repr(C)
  - Bun API 类型 → 评估，无 C 依赖则删除
- [ ] 051-3: `std::mem::size_of` 对比关键结构体
- [ ] 051-4: `cargo test` 全量通过

**验证**: TEST-RFQ-051

### TASK-052: PORT 注解清理 (REQ-RFQ-052) — P2

**原子任务**:
- [ ] 052-1: `grep -rn 'PORT\|// PORT' src/ | wc -l` 确认基线
- [ ] 052-2: 分类：
  - 已验证翻译正确 → 删除 PORT 注解
  - 翻译有已知问题 → 保留 + 添加 ISSUE 链接
  - 正在重构中 → 保留 + 添加 REQ-RFQ-* 链接
- [ ] 052-3: 批量删除已验证的 PORT 注解（纯注释变更）

**验证**: TEST-RFQ-052

### TASK-053: 死代码消除 (REQ-RFQ-053) — P2

**原子任务**:
- [ ] 053-1: `cargo +nightly udeps` 检测未使用依赖
- [ ] 053-2: `cargo machete` 检测未使用 import
- [ ] 053-3: `grep -rn 'TODO\|FIXME\|unimplemented!\|todo!' src/` 定位
- [ ] 053-4: 清理所有 TODO/FIXME（实现或删除）
- [ ] 053-5: 删除注释掉的代码块
- [ ] 053-6: 删除空实现（`fn foo() {}` / `fn foo() { unimplemented!() }`）
- [ ] 053-7: 配置 `#![deny(warnings)]` 零 warning
- [ ] 053-8: `cargo test` 全量通过

**验证**: TEST-RFQ-053

---

## 执行依赖 DAG

```
Phase 0 (基础设施)
├── TASK-001: Miri CI ─────────────────────────┐
├── TASK-002: Benchmark ───────────────────────┤
└── TASK-003: 质量审计 ─────────────────────────┤
                                                │
Phase 1 (lifetime UB) ←── TASK-001             │
├── TASK-011: detach_lifetime ─────────┐        │
├── TASK-012: raw pointer ←── TASK-011 ├───┐   │
└── TASK-013: transmute ←── TASK-011    │   │   │
                                         │   │   │
Phase 2 (aliasing UB) ←── TASK-011      │   │   │
├── TASK-021: &mut+raw ptr ─────────────┤   │   │
├── TASK-022: slice alias ←── TASK-021  │   │   │
└── TASK-023: UnsafeCell ←── TASK-021   │   │   │
                                         │   │   │
Phase 3 (错误处理) ←── Phase 1+2        │   │   │
├── TASK-031: unwrap 消除 ──────────────┤   │   │
├── TASK-032: panic 边界 ←── TASK-031   │   │   │
└── TASK-033: 错误类型 ←── TASK-031     │   │   │
                                         │   │   │
Phase 4 (零拷贝) ←── TASK-002 ──────────┘   │   │
├── TASK-041: Bytes ─────────────────────────┘   │
├── TASK-042: CompactString/SmallVec ────────────┤
├── TASK-043: clone 消除 ←── TASK-041 ──────────┤
└── TASK-044: Vec 预分配 ───────────────────────┤
                                                  │
Phase 5 (代码质量) ←── TASK-013 ─────────────────┘
├── TASK-051: repr(C) 审计
├── TASK-052: PORT 清理
└── TASK-053: 死代码消除
```

**关键路径**: TASK-001 → TASK-011 → TASK-012/013 → TASK-021 → TASK-031

**并行机会**:
- Phase 0: 3 个 TASK 全部并行
- Phase 1: TASK-012/013 在 TASK-011 完成后并行
- Phase 2: TASK-022/023 在 TASK-021 完成后并行
- Phase 4: TASK-041/042/044 并行，TASK-043 在 TASK-041 后
- Phase 5: 3 个 TASK 全部并行

---

## 每个 Phase 完成验证门控

| Phase | 验证条件 | 门控工具 |
|-------|---------|---------|
| 0 | Miri CI 可运行 + benchmark 基线 + 审计基线 | 手动验证 |
| 1 | `detach_lifetime` = 0, `transmute` ≤10, Miri 通过 | Miri + grep |
| 2 | Miri Stacked Borrows + Tree Borrows 通过 | Miri |
| 3 | `unwrap()` ≤1,000, panic 边界文档 | grep + 文档 |
| 4 | criterion 基准对比达标 | criterion |
| 5 | repr(C) ≤425, PORT ≤1,940, warnings = 0 | quality-audit.sh |

---

## SPEC 追溯矩阵

| REQ | TEST | NFR | Phase |
|-----|------|-----|-------|
| REQ-RFQ-001 | TEST-RFQ-001 | NFR-SAFE-001 | 0 |
| REQ-RFQ-002 | TEST-RFQ-002 | NFR-PERF-003 | 0 |
| REQ-RFQ-003 | TEST-RFQ-003 | NFR-QUAL-001 | 0 |
| REQ-RFQ-011 | TEST-RFQ-011 | NFR-SAFE-001 | 1 |
| REQ-RFQ-012 | TEST-RFQ-012 | NFR-SAFE-001 | 1 |
| REQ-RFQ-013 | TEST-RFQ-013 | NFR-SAFE-001 | 1 |
| REQ-RFQ-021 | TEST-RFQ-021 | NFR-SAFE-001 | 2 |
| REQ-RFQ-022 | TEST-RFQ-022 | NFR-SAFE-001 | 2 |
| REQ-RFQ-023 | TEST-RFQ-023 | NFR-SAFE-001 | 2 |
| REQ-RFQ-031 | TEST-RFQ-031 | NFR-SAFE-002 | 3 |
| REQ-RFQ-032 | TEST-RFQ-032 | NFR-SAFE-002 | 3 |
| REQ-RFQ-033 | TEST-RFQ-033 | NFR-SAFE-002 | 3 |
| REQ-RFQ-041 | TEST-RFQ-041 | NFR-PERF-003 | 4 |
| REQ-RFQ-042 | TEST-RFQ-042 | NFR-PERF-003 | 4 |
| REQ-RFQ-043 | TEST-RFQ-043 | NFR-PERF-003 | 4 |
| REQ-RFQ-044 | TEST-RFQ-044 | NFR-PERF-003 | 4 |
| REQ-RFQ-051 | TEST-RFQ-051 | NFR-QUAL-001 | 5 |
| REQ-RFQ-052 | TEST-RFQ-052 | NFR-QUAL-001 | 5 |
| REQ-RFQ-053 | TEST-RFQ-053 | NFR-QUAL-001 | 5 |

**废弃映射**: REQ-PERF-001~005 被 REQ-RFQ-041~044 + REQ-RFQ-012 完全覆盖
