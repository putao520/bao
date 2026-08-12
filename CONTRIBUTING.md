# 为 Bao 做贡献(Contributing to Bao)

感谢你考虑为 **Bao(包子)** 做贡献!Bao 是一个 Rust-native 的反指纹浏览器运行时,把 SpiderMonkey JS 引擎 + servo 全功能浏览器 + Node.js/Bun API + Stealth 反指纹统一到一个 Rust 二进制里。这份指南帮你快速理解项目纪律与协作方式。

## 目录

- [欢迎贡献的类型](#欢迎贡献的类型)
- [代码在哪里(Codebase Tour)](#代码在哪里codebase-tour)
- [开发环境要求](#开发环境要求)
- [构建与测试](#构建与测试)
- [SPEC 驱动开发(核心纪律)](#spec-驱动开发核心纪律)
- [BCE(BUG 类根除)](#bcebug-类根除)
- [代码规范](#代码规范)
- [Commit 规范](#commit-规范)
- [PR 流程](#pr-流程)
- [禁止事项](#禁止事项)
- [沟通](#沟通)

---

## 欢迎贡献的类型

| 类型 | 说明 |
|------|------|
| **Bug report** | 在 GitHub Issue 报告可复现的缺陷(附复现步骤 + Bao 版本 + 平台) |
| **Feature** | 新功能或对现有能力的增强(需先在 PRD/SPEC 层讨论,见下) |
| **Docs** | 改进 README / CLAUDE.md / `.spec/` / 注释 / 示例 |
| **Compat test** | 补充 Web Platform / Node.js / Bun API 兼容性测试用例 |
| **Example** | `bao browser` / `bao run` / PagePool / Stealth 的新示例 |

> 不确定从哪里开始?先到 GitHub Discussions 开个话题对齐意图。

---

## 代码在哪里(Codebase Tour)

Bao 是 monorepo,**不同目录的修改门槛不同**。提交 PR 前请先确认你要改的代码属于哪一层:

| 目录 / crate 前缀 | 角色 | 能改吗? |
|-------------------|------|---------|
| `src/bao_*` | Bao 原创层(`bao_engine` / `bao_browser` / `bao_cdp` / `bao_cdp_client` / `bao_stealth` / `bao_runtime` / `bao_uloop` / `bao_cli` / `bao_bin` / `bao_bundler` / `bao_crypto` 等) | **可以改**(走正常 PR 流程) |
| `src/bun_*` | Bun 上游纯 Rust crate(~85 个,零 JSC,生产级实现) | **禁止修改**。复用优先,详见 [代码规范](#代码规范) |
| `vendor/servo/` | servo 上游(DOM + CSS + Layout + webrender) | **默认禁止修改**(servo 是上游真源)。仅 BCE-002 / BCE-004 已授权 patch(`script_thread.rs` / `lib.rs`)沉淀为例外,新改 servo 必须先开 Issue 取得明确授权 |
| `vendor/mozjs/` | SpiderMonkey FFI + `mozjs-sys`(含已应用的 EBUSY patch) | **默认禁止**。需要 patch 时先讨论(见 CLAUDE.md `mozjs 构建经验`) |
| `vendor/boringssl/` | TLS 实现 | 通过 `bao_boringssl_bridge` 适配,**不在 vendor 改** |
| `.spec/` | SPEC(SSOT,单一真相来源) | 改 SPEC 走独立 PR,必须符合派生链(见下) |

**对外唯一公共 package 是 `bao`**(`src/bao`)。宿主项目只应依赖 `bao` 这一个包,不要分别 `path` 依赖各个 `bao_*` 子 crate。

---

## 开发环境要求

| 依赖 | 版本 / 说明 |
|------|-------------|
| **Rust toolchain** | `nightly`(`rust-toolchain.toml` 已锁定,无需手动指定) |
| **C/C++ 编译器** | clang / g++ — `mozjs` 从源码编译 SpiderMonkey,必须 |
| **平台** | 当前主路径是 **Linux x86_64**。macOS / Windows 事件循环尚未全平台验证 |
| **构建工具** | `make`(BCE 门禁脚本用)、`pkg-config`、`cmake`(部分依赖) |

> 首次构建会从源码编译 SpiderMonkey,耗时较长(数十分钟级,取决于机器)。后续增量构建很快。

---

## 构建与测试

CI 跑**本地**(`just`):mozjs 从源码编译 SpiderMonkey,hosted CI runner 上太慢,所以权威的"是否通过"判定在本地 justfile。两种方式,任选其一:

```bash
# (A) 直接 cargo recipe — 快速反馈,所有 cargo 已强制 --jobs 1
just ci          # fmt + lint + check + test + bce

# (B) 复用 GitHub Actions workflow via act(Docker)— GHA 与本地同一 workflow 真源
just gha-list    # 列出 act 识别的 workflow job
just gha-ci      # 本地容器跑 .github/workflows/ci.yml

# 或手动
cargo build -p bao_bin      # target/debug/bao
cargo test -p bao           # 公共 package 测试
just bce                    # BCE 门禁(等价 make bce-check)
```

`just`(1.57+)是必需的。所有 cargo recipe 已强制 `--jobs 1`,无需手动加。

### Rust CI 必须单线程(`--jobs 1`)

**重要**:在 CI 或任何正式验证环境跑 `cargo test` / `clippy` / `build` 时,**必须加 `--jobs 1`**(justfile 已统一处理):

```bash
cargo test   --jobs 1
cargo clippy --jobs 1
cargo build  --jobs 1
```

原因:`mozjs-sys` 的 `Mutex_posix.cpp` 已应用 EBUSY patch(`MutexImpl::~MutexImpl` 忽略 libtest 线程池 TLS teardown 时的 `EBUSY`),但并行编译/链接在部分环境仍会触发非确定性 race。CI 单线程是 fail-safe 默认。如果你看到 SIGSEGV,第一步:

```bash
nm target/debug/deps/libmozjs_sys-*.rlib | grep MutexImplD1
```

检查 rlib 是否包含旧代码(详见 CLAUDE.md `EBUSY Patch`)。

### BCE 门禁

```bash
make bce-check
```

跑 `bce-gc-unsafe`(GC-unsafe AST 检测)、`bce-ast-catch-fallback`(catch fallback 检测)、`bce-spec-id`(`@trace` SPEC id 检测)。PR 合并前本地必须通过。

---

## SPEC 驱动开发(核心纪律)

**`.spec/` 是单一真相来源(Single Source of Truth)。** 这是 Bao 与大多数项目最大的不同 —— 改代码之前先对齐 SPEC。

### 派生链(单向不可颠倒)

```
PRD (用户要什么)  →  SPEC (怎么做才算对)  →  Code (落实契约)
```

- SPEC REQ 通过 `groundedIn` 指向上游 PRD-REQ
- 代码通过 `@trace REQ-XXX` 注解关联 SPEC REQ
- PRD 零回指 SPEC,Code 不成为真源

### SPEC 文件结构(`.spec/`)

| 文件 | 内容 |
|------|------|
| `00-INDEX.html` | 索引 |
| `01-BUSINESS.html` | 业务架构(功能模块树 · 用例图 · 指标维度) |
| `02-SYSTEM.html` | 系统架构(Bun Crate DAG · Servo 组件 · 融合映射 · 多页面管理 · CDP 双层抽象) |
| `03-PROCESS.html` | 核心流程(JS 执行管线 · 渲染管线 · CDP 路由 · 状态机 · 线程模型) |
| `04-DATA-MODEL.html` | 数据模型(Entity · 模型树 · 缓存策略) |
| `05-IMPLEMENTATION.html` | 实施路线图(5 阶段任务分解 · 复用矩阵 · 风险矩阵) |
| `06-CDP-SERVER.html` | CDP Server 设计 |
| `10-REQUIREMENTS.html` | 功能需求(31 REQ · 6 域 ENG/CLI/BRW/CDP/STL/LIB · 5 NFR · 追溯矩阵) |
| `11-TESTING.html` | 测试用例 |

### 贡献者操作清单

| 你想做的事 | 必须先做的 |
|-----------|-----------|
| 实现/修 bug 到现有功能 | 读 `.spec/10-REQUIREMENTS.html` 找到对应 REQ,确认契约边界 |
| 新增功能 | **先**开 PRD/SPEC 讨论 PR,**再**写代码(不接受先码后补 SPEC) |
| 改对外 API | 必须先在 SPEC 改 Entity / API / REQ,标注影响面 |
| 修 BUG | 走 [BCE](#bcebug-类根除) 流程,不允许单点补丁 |

**SPEC 未定义的领域,停止并报告,不要自行补充。** 范围守恒:交付范围 ≡ SPEC 定义范围,双向零差集。

---

## BCE(BUG 类根除)

Bao 不接受单点补丁。任何 BUG 修复必须走 BCE(Bug-Class Eradication)闭环:

```
归因(root cause)
  → 泛化(提炼为类模式)
  → 全项目横扫(找出所有同类实例)
  → 批量根治(统一修法,≥3 处同类用 codemod)
  → 残留 = 0(全量确认)
  → 沉淀(写入 .spec/BUG-KNOWLEDGE.md 防复发)
```

- `.spec/BUG-KNOWLEDGE.md` 是 BUG 模式知识库,新沉淀的签名要能匹配未来同类问题
- 同类问题 ≥3 处必须用 `fix_code(codemod)` 横扫,不接受逐个 ad-hoc Edit
- 完整规则见 `~/.claude/rules/bug-class-eradication.md`

> 报 BUG 时请尽量描述触发条件、复现步骤、期望行为。能给出最小复现 case 的报告优先处理。

---

## 代码规范

### P-1 红线(commit gate 强制)

提交前必须清除:

- `TODO` / `FIXME` / `stub` / 空实现
- `console.log` / 残留调试输出
- 占位 string / `Option::None` 逃避 / `"待定"`

### P-2 复杂度限制

| 维度 | 上限 |
|------|------|
| 嵌套层数 | ≤ 5 |
| 圈复杂度 | ≤ 10 |
| 函数参数个数 | ≤ 5 |

超出请重构(`refactor_code` / 提取子函数)。

### P-3 架构风格

Clean Code + Rust 惯例 · DRY / KISS · ECS + Microkernel。

### 复用优先(Bun crate > 社区库 > 手写)

新增任何函数前,**先 grep workspace 内 `bun_*` crate 是否已有实现**:

```
1. workspace 内 bun_* crate(已编译、已优化、已测试)
2. crates.io 成熟库(url / sha2 / hmac / etc.)
3. 仅当 1/2 都没有时才允许手写
```

禁止手写已有实现(HTTP 解析、DNS、模块解析、Base64、URL 等 —— 见 CLAUDE.md `复用映射` 表)。只有以下情况允许手写 Rust:

1. `bao_uloop` 的 epoll tick(必须与 `FilePoll` 共享 fd)
2. JSC → SpiderMonkey 桥接层(`bao_engine`)
3. Servo 集成桥接层(`bao_browser`)
4. CDP / Stealth / Node.js 兼容层(`bao_cdp` / `bao_stealth` / `bao_runtime`)

### 三化原则

| 原则 | 检查点 |
|------|--------|
| **高性能化** | 零拷贝 / SIMD / mmap / io_uring —— 禁 `Vec::new()` 手写 buffer,禁 `String::from_utf8_lossy` 替代零拷贝 |
| **去锁化** | 单线程 JS 执行模型下禁 `Mutex`/`RwLock`,用 `thread_local!` + `RefCell`;`Mutex` 仅用于真正跨线程并发(HTTP 等) |
| **成熟库化** | workspace 已有 crate > crates.io 成熟库 > 手写 |

### 线程 / JSContext 铁律

- 全局唯一 `JSEngine` + 每个 `ScriptThread` 持有线程局部 `JSContext`(servo 上游模型)
- **禁止跨线程传递 `JSObject` 裸指针** —— 跨线程只能传 `PageId` / 句柄 / 序列化数据
- DOM ↔ Node.js 互操作必须发生在同一线程内

---

## Commit 规范

### Conventional Commits

```
<type>(<scope>): <subject>

<body>
```

| type | 用途 |
|------|------|
| `feat` | 新功能(对应 SPEC REQ 新增) |
| `fix` | BUG 修复(必须走 BCE) |
| `refactor` | 重构(无行为变化) |
| `test` | 测试用例 |
| `docs` | 文档 / SPEC / 注释 |
| `chore` | 构建 / CI / 依赖等杂项 |
| `perf` | 性能优化 |
| `revert` | 回滚 |

`scope` 通常是 crate 名或域,例如:`feat(bao_browser)`、`fix(bao_engine)`、`refactor(bao_cdp)`、`docs(spec)`。

### `@trace` 注解(强制)

每个改动必须用 `@trace` 注解关联它实现的 SPEC REQ:

```rust
/// 创建 PageHandle。
/// @trace REQ-LIB-002
pub fn create_page(&self, cfg: &PageConfig) -> Result<PageHandle, BrowserError> {
    // ...
}
```

- 业务代码用 `@trace REQ-XXX`(对应 SPEC req 必须有 `groundedIn → PRD-REQ`)
- 工具契约代码用 `// @tool REQ-XXX`(对应 req 标记 `data-req-category="tools"`,豁免 groundedIn)
- 无 `@trace` = 无 provenance = commit gate 失败

---

## PR 流程

1. **小而聚焦** —— 单个 PR 解决一个问题,控制在可 review 的体量。大改动拆成多个 PR。
2. **CI 必须绿** —— 本地先跑 `cargo test --jobs 1` + `cargo clippy --jobs 1` + `make bce-check`(CI 建立后以 CI 为准)。
3. **SPEC 变更单独 PR** —— PRD / SPEC 的改动与代码改动分开提,先合 SPEC 再合代码。
4. **PR 描述包含**:
   - 改了什么 / 为什么改(关联 Issue 或 REQ)
   - 是否改了 SPEC(`.spec/`)
   - BCE 影响面(如果是 BUG 修复,列出横扫范围 + 残留确认)
   - 测试方式(怎么验证)
5. **改 `bao_*` 层即可**,不要顺手改 `bun_*` 或 `vendor/`(详见 [禁止事项](#禁止事项))。

---

## 禁止事项

| 行为 | 原因 |
|------|------|
| 改 `src/bun_*` 上游 crate | 100% 复用,保持与上游 Bun 的 diff 最小 |
| 跳过 SPEC 直接编码 | SPEC SSOT,未定义领域停止报告而非自行补充 |
| 改 `vendor/servo/` / `vendor/mozjs/` / `vendor/boringssl/`(无授权) | 上游真源,需先开 Issue 取得授权 |
| MVP / stub / 分期交付 | 不接受占位 / fallback / 假绿测,依赖缺失必须 fail-closed 或显式 degraded |
| 单点补丁修 BUG | 必须走 BCE 闭环(归因→泛化→横扫→根治→残留=0→沉淀) |
| 跨线程持有 `JSObject` 裸指针 | 破坏 activation 栈,SIGSEGV |
| 单线程 JS 执行路径加 `Mutex`/`RwLock` | 用 `thread_local!` + `RefCell`(`Mutex` 仅限真正跨线程并发) |
| 手写已有 `bun_*` 能力(HTTP / DNS / 模块解析 / Base64 / URL 等) | 复用优先 |
| `TODO` / `FIXME` / `stub` / 空实现 / `console.log` | P-1 红线,commit gate 拦截 |

---

## 沟通

| 场景 | 渠道 |
|------|------|
| 缺陷报告 | [GitHub Issues](https://github.com/putao520/bao/issues) — 附 Bao 版本(`bao --version` 或 `Bun.version`)+ 平台 + 复现步骤 |
| 功能讨论 / 问答 / 设计讨论 | [GitHub Discussions](https://github.com/putao520/bao/discussions) |
| 安全漏洞(Stealth / TLS / 沙箱逃逸等) | **不要开公开 Issue** —— 私下联系维护者(参见仓库 SECURITY policy 或 Issue 模板里的安全联系信息) |

---

## 许可证

提交的贡献将按项目许可证发布:**MPL-2.0**(SpiderMonkey + Servo)+ **MIT**(Bun crates)。

---

再次感谢你的贡献!如果有任何疑问,欢迎在 Discussions 开话题。
