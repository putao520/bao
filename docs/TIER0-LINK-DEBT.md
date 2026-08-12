# Tier-0 独立 lib test 链接债

> bun_uws_sys 的 C 代码引用 17 个跨 crate `Bun__*` 符号,单 Tier-0 crate 的独立 lib test 链接不全,链接器报 `Undefined symbol`。

---

## 1. 现象

Tier-0 / 底层 crate 的独立 lib test 链接失败:

```bash
cargo test -p bao_uloop --lib
# ld: undefined symbol: Bun__lock__size
# ld: undefined symbol: Bun__isEpollPwait2SupportedOnLinuxKernel
# ld: undefined symbol: Bun__internal_dispatch_ready_poll
# ... (17 个 Bun__* 符号链锁报错)

cargo test -p bun_core --lib
# 同样的链接失败链
```

**对照**: `cargo check -p bao_bin` 和正式 binary(`bao`)编译运行正常 —— 正式 binary 链接全栈 runtime,17 个符号的 real owner 全部入图,链接完整。

## 2. 根因

三层叠加的结构性约束,不是单点 bug。

### 2.1 C 代码跨 crate 引用 `Bun__*` 符号

`bao_uloop` 依赖 `bun_uws_sys`(C 库 libusockets via `cc::Build`)。`bun_uws_sys` 的 C 源码(`packages/bun-usockets/src/loop.c` 及相关)硬引用 17 个 `Bun__*` 跨 crate 符号,real owner 分散在多个 Rust crate:

| 符号 | 定义位置 |
|------|----------|
| `Bun__lock` / `Bun__unlock` / `Bun__lock__size` | `bun_threading::mutex`(`src/threading/Mutex.rs`,`pub(crate) static` + `#[no_mangle]`) |
| `Bun__isEpollPwait2SupportedOnLinuxKernel` | `bun_analytics`(`src/analytics/lib.rs`) |
| `Bun__internal_dispatch_ready_poll` | bun runtime(runtime dispatch) |
| `Bun__internal_ensureDateHeaderTimerIsEnabled` | bun runtime(timer 管理) |
| `Bun__JSC_onBeforeWait` | bun runtime(JSC 集成) |
| `Bun__panic` | bun runtime(panic 兜底) |
| `Bun__Node__UseSystemCA` | bun TLS/CA |
| `Bun__addrinfo*` (6 个变体) | `bun_dns`(c-ares/resolver) |
| `Bun__doesMacOSVersionSupportSendRecvMsgX` | bun 平台抽象(macOS) |

生成完整清单的命令见 §8。

### 2.2 Bun 上游架构模式:假设最终 binary 链接全部 C 符号

Bun 上游的设计假设:**最终 binary(`bao_bin` / `bun`)链接整个 workspace 的全部 C 符号**,各 crate 的 Rust 代码不需要独立完整链接,只要最终产物能合成完整符号图即可。这套模式在 Bun 上游工作正常 —— 因为 Bun 的测试和 CI 基本以整栈 binary 为单位。

### 2.3 与单 crate 独立 lib test 不兼容

Cargo + wild linker(`-fuse-ld=wild`)对单个 crate 跑 `cargo test -p <crate> --lib` 时,只会拉入**该 crate 的 Rust dependency graph 实际 use 到的符号**。未被 Rust 代码直接 use 的 C 符号会被链接器树摇掉 —— 但 `bun_uws_sys` 的 C 代码却硬引用它们。结果:链接器在一层一层解开符号依赖时,逐个暴露 `Undefined symbol: Bun__*`。

### 2.4 `bao_native_stubs` 兜底未覆盖

`bao_native_stubs` crate 设计上是 dev/test force_link 兜底 —— 它有 `force_link()` 函数 + `.init_array` ctor,目的是在 test 链接时把所有 C 库符号钉进依赖图,防树摇。但它目前只锚定部分符号:

- ✅ lshpack(HTTP/2 HPACK)
- ✅ boringssl / boringssl_sys
- ✅ mimalloc
- ✅ highway(SIMD)
- ❌ **未覆盖**这 17 个 Rust 侧 `Bun__*` 符号

所以 `bao_native_stubs` 的现有 force_link 拦不住 Tier-0 单 crate 链接失败。

## 3. 影响范围

**只影响 Tier-0 / 底层 crate 的独立 lib test**:

| crate | lib test | 整栈集成 test |
|-------|----------|---------------|
| `bao_uloop` | ❌ 链接失败 | ✅ 通过(bao_bin 整栈) |
| `bun_core` | ❌ 链接失败 | ✅ 通过 |
| `bao_browser` | ✅ 绿 | ✅ 绿 |
| `bao_cdp` | ✅ 绿 | ✅ 绿 |
| `bun_runtime` | ✅ 绿 | ✅ 绿 |
| `bao_cdp_client` | ✅ 绿 | ✅ 绿 |
| `bao_stealth` | ✅ 绿 | ✅ 绿 |
| `bao_engine` | ✅ 绿 | ✅ 绿 |

**正式 binary(`bao_bin`)编译 + 运行正常** —— 链接全栈 runtime,17 符号 real owner 全部入图。

**BCE 红线无影响**:Tier-0 的业务逻辑验证经 bao_bin 集成 test 覆盖(整栈链接),业务层全绿。这是"单 crate 独立 test 工具链债",不是"功能缺陷"或"代码 BUG"。

## 4. 已尝试方案 + 为何不 work

记录失败经验,避免重复踩坑。

### 4.1 改可见性:`pub(crate)` → `pub` + `#[used]`

**操作**: 把 `bun_threading::mutex::Bun__lock__size` 从 `pub(crate) static` 改 `pub` + 加 `#[used]`。

**结果**: 无效。链接器仍树摇。`#[no_mangle]` + `pub` 本身不构成"必须被链接"的强约束 —— Rust 编译器认为该符号没有外部 consumer 时,仍可在 LTO/wild 树摇阶段丢弃。

### 4.2 加 dev-dependency

**操作**: 给 `bao_uloop` 的 `Cargo.toml` 加 `bun_threading` 作 dev-dependency。

**结果**: 无效。Cargo 对"Rust 代码未实际 use 的依赖"不强制链接。dev-dependency 只是把 crate 拉进构建图,不强制其 `#[no_mangle]` 符号进入最终链接图。

### 4.3 强制引用 const 锚点

**操作**: 在 `bao_uloop/src/lib.rs` 加

```rust
#[cfg(test)]
const _ANCHOR: usize = bun_threading::mutex::Bun__lock__size;
```

强制 Rust 编译器把 `bun_threading` 拉进链接图。

**结果**: 解决了 `Bun__lock__size` 一个符号。但链接器立刻暴露下一个 `Bun__isEpollPwait2SupportedOnLinuxKernel`(owner: `bun_analytics`),再加一个锚点,再暴露下一个 —— **打地鼠模式,17 个符号跨多 crate(bun_threading/bun_analytics/bun_dns/runtime/...)逐一锚定**。

**教训**: 逐符号锚点是局部修补,不解决"17 符号跨多 crate"的整体架构约束。需要批量 / 机制化兜底,不是逐个补 const 引用。

## 5. 候选根治方案(未选定)

三个候选,各有优劣。选定前需要 architect + 主会话裁决,不在此文档决策。

### 方案 A:扩展 `bao_native_stubs::force_link()` 聚合 17 符号

**做法**: 把这 17 个符号的 owner crate 全部加入 `bao_native_stubs` 的 force_link 引用图:

- 每个 owner crate(`bun_threading` / `bun_analytics` / `bun_dns` / runtime 等)加一个 `force_link()` 函数(暴露 1 个 const 锚点引用自身符号)
- `bao_native_stubs::force_link()` 聚合所有 owner crate 的 `force_link()`,形成完整引用闭包
- Tier-0 crate 加 `bao_native_stubs` dev-dependency

**优**: 符合 `bao_native_stubs` 现有设计意图(test force_link 兜底);real owner 保持不变,链接图完整;一次性、机制化。

**劣**: 改动面大 —— 17 符号 × 多个 owner crate,每个 owner 都要加 force_link 函数;新增 dev-dep 关系多;后续若 Bun 上游新增 `Bun__*` C 引用,要同步补锚点(维护负担)。

### 方案 B:`bao_native_stubs` 补 weak stub 定义

**做法**: 在 `bao_native_stubs` 用 `#[no_mangle]` + `#[link_section]` 给这 17 个符号提供 weak 定义,作为 test 链接兜底。正式 binary 链接时 real owner 的强符号覆盖 weak stub(test 不会跑到这些 stub,它们只是占位让链接通过):

```rust
#[no_mangle]
#[link_section = ".text"]
pub static WEAK_Bun__lock__size: usize = 0;
// 通过 wasm-ld/wild 的 weak symbol 语义 / alias 引用 Bun__lock__size
```

**优**: 单点改动(只改 `bao_native_stubs` 一个 crate);test 独立可链;real owner 在正式 binary 中正常覆盖。

**劣**: weak 符号语义在 Rust + wild + macOS/Linux 平台需要验证(语义不统一);real/stub 冲突的链接器行为需逐符号确认;若 test 不小心跑到 stub 会得到错误结果(需保证 stub 只占位不执行);weak 定义与 `#[no_mangle]` 的交互微妙,可能需要 linker script / asm alias。

### 方案 C:承认架构约束,Tier-0 lib test 不独立跑

**做法**: 承认这是 Bun 上游的架构假设("最终 binary 链接全栈"),Tier-0 crate(`bao_uloop` / `bun_core`)的 lib test 不作为独立验证单元,改由 bao_bin 集成 test 覆盖(整栈链接,符号齐全)。

**优**: 零代码改动;符合 Bun 上游架构模式;无维护负担;无 stub/weak 语义风险。

**劣**: 单 crate lib test 不可用 —— 无法独立 `cargo test -p bao_uloop --lib` 验证 Tier-0 的单元逻辑;Tier-0 的回归只能靠集成 test 间接覆盖;开发者修改 Tier-0 时需要启动整栈 test,迭代速度变慢。

## 6. 当前处置

- **不阻断业务 test** —— 业务 crate(`bao_browser` / `bao_cdp` / `bun_runtime` / `bao_cdp_client` / `bao_stealth` / `bao_engine`)的 lib + 集成 test 全绿,正常 CI。
- **Tier-0 lib test 文档化跳过** —— 全量 workspace test 时显式 exclude:
  ```bash
  cargo test --workspace --exclude bao_uloop --exclude bun_core
  ```
  或在这些 crate 的 lib test 上加 `#[ignore]`,留一个"已知不可独立链"的显式标记。
- **正式 binary 正常** —— `cargo build -p bao_bin` / `cargo test -p bao_bin` / 集成 test 全栈链接,功能完整。
- **不阻塞 BCE** —— Tier-0 的业务逻辑经 bao_bin 集成 test 覆盖,不是代码缺陷。

## 7. 决策时机

本债务**当前可承载**(不阻断主路径)。出现以下任一情况,升级为必须根治:

1. Tier-0 crate 需要高频独立迭代(单元 test 无法用影响开发节奏)
2. Bun 上游新增的 `Bun__*` C 引用扩散到更多 crate,集成 test 也开始受波及
3. 有方案 B 的 weak stub 实现经验可借鉴(平台验证已完成)
4. 重构 Tier-0 边界(如把 `bun_uws_sys` 的 C 引用收敛到 Rust 层)

选定根治方案前,architect + 主会话需评估 §5 三个方案的工程成本与长期维护负担,禁止自行选方案动手。

## 8. 验证命令

生成 17 符号完整清单(用于核对债务范围、检测上游新增):

```bash
# 扫描 bun_uws_sys 的 C 源码引用的所有 Bun__* 符号
grep -rhoE "Bun__[a-zA-Z0-9_]+" packages/bun-usockets/src/ src/uws_sys/ | sort -u
```

输出示例(核对当前 17 符号清单是否漂移):

```
Bun__addrinfo_complete
Bun__addrinfo_cancel
Bun__addrinfo_getaddrinfo
Bun__addrinfo_getaddrinfo_sync
Bun__addrinfo_destroy
Bun__addrinfo_set_callbacks
Bun__doesMacOSVersionSupportSendRecvMsgX
Bun__internal_dispatch_ready_poll
Bun__internal_ensureDateHeaderTimerIsEnabled
Bun__isEpollPwait2SupportedOnLinuxKernel
Bun__JSC_onBeforeWait
Bun__lock
Bun__lock__size
Bun__Node__UseSystemCA
Bun__panic
Bun__unlock
```

每个符号的 real owner 定位示例:

```bash
# 定位符号定义所在的 Rust crate
grep -rn "Bun__lock__size" --include="*.rs" src/
# src/threading/Mutex.rs — bun_threading
```

---

**债务性质**:架构性链接债(Bun 上游"整栈链接"假设 vs Cargo 单 crate 独立 test 的固有约束),非代码缺陷。处置策略:文档化 + 集成 test 覆盖,等待架构窗口根治。
