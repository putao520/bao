# Bao (包子) — Bun + SpiderMonkey + Servo

Bun 的 SpiderMonkey 引擎分支，支持 Node.js 运行时模式和内嵌浏览器模式。

## 核心差异化

1. **SpiderMonkey 引擎**：替代 JavaScriptCore (MPL-2.0)
2. **内嵌浏览器**：DOM + CSS + 布局 + 内存渲染 + CDP (libservo)
3. **反指纹模块**：TLS/HTTP/Canvas/Navigator/行为模拟 (Stealth)
4. **单进程多线程**：零 GPU、纯内存渲染

## 命名规范

| 层级 | 规则 |
|------|------|
| 用户品牌 | `bao` (bao run / bao test / bao browser) |
| JS 全局对象 | `Bun.*` (保留) + `Bao.*` (别名，同一对象) |
| 内部 Rust crate | `bun_*` 不改 (保持上游兼容) |
| 环境变量 | `BUN_*` (保留) + `BAO_*` (新增别名) |
| 代码引用 | 保留所有 Bun 内部引用 |

原则：用户输入 `bao`，代码里还是 `bun`。最小化与上游 Bun 的 diff。

---

## 上游项目参考

### Bun — 基础运行时来源

| 路径 | 参考价值 |
|------|---------|
| `~/code/rust/bun/CLAUDE.md` | 构建命令、测试规范、代码架构、crate 组织、开发规范 |
| `~/code/rust/bun/Cargo.toml` | 105 crate workspace 定义、依赖版本锁定 |
| `~/code/rust/bun/src/` | **核心复用来源**：~85 个纯 Rust crate (L0-L10) 零修改复用 |
| `~/code/rust/bun/src/jsc/` | JSC 绑定层 (L11-L13, 13 crate) — SpiderMonkey 迁移目标，理解 JSC→SM 映射的参考 |
| `~/code/rust/bun/src/runtime/` | Bun JS API 实现 — .classes.ts 定义复用 95% |
| `~/code/rust/bun/src/resolver/` | 模块解析器 — 100% 复用，SM hook 桥接参考 |
| `~/code/rust/bun/src/event_loop/` | 事件循环 — SM JobQueue 桥接设计参考 |

**Bun SPEC 测绘成果**：已测绘为 `.spec/02-SYSTEM.html` §2 (Crate DAG 13 层 + JSC 边界分析 + 两维分离模式)

### Servo — 浏览器引擎来源

| 路径 | 参考价值 |
|------|---------|
| `~/code/rust/servo/Cargo.toml` | 36 组件 workspace 定义 |
| `~/code/rust/servo/components/servo/` | **libservo crate** (v0.1.0) — Bao 浏览器模式嵌入入口 |
| `~/code/rust/servo/components/script/` | DOM 实现 (280K LOC) — 依赖 mozjs，共享 SM JSContext |
| `~/code/rust/servo/components/script_bindings/` | SpiderMonkey ↔ DOM 桥接 (WebIDL 代码生成) — SM GC 管理参考 |
| `~/code/rust/servo/components/style/` | Stylo CSS 引擎 — Firefox 共用，样式计算 |
| `~/code/rust/servo/components/layout/` | 布局引擎 — Layout Box 生成 |
| `~/code/rust/servo/components/net/` | Fetch 实现 (WHATWG 规范) — 网络栈参考 |
| `~/code/rust/servo/components/constellation/` | 中央协调器 — IPC 中枢，单进程模式自动优化 |
| `~/code/rust/servo/ports/servoshell/` | Embedder 实现参考 — BaoEmbedder 设计模板 |

**Servo SPEC 测绘成果**：已测绘为 `.spec/02-SYSTEM.html` §3 (36 组件分层 + 依赖关系 + 关键数据)

### 辅助参考项目

| 路径 | 参考价值 |
|------|---------|
| `~/code/rust/mozjs/` | mozjs crate 源码 — SM FFI 绑定细节、API 覆盖率、缺失绑定清单 |
| `~/code/rust/vello/` | Vello CPU 渲染器源码 — CPU 后端配置、glyph_run 文字渲染 |
| `~/code/rust/taffy/` | CSS 布局引擎源码 — Flexbox/Grid 算法细节 |
| `~/code/rust/blitz/` | DioxusLabs 模块化浏览器 — 嵌入式浏览器参考架构 |

---

## SPEC 目录

| SPEC 目录 | .spec/00-INDEX.html |
|-----------|---------------------|

### SPEC 文件清单

| 文件 | 内容 | 核心设计产物 | 状态 |
|------|------|-------------|------|
| 01-BUSINESS.html | 业务架构 | 功能模块树 · 用例图 · 指标维度表 · 许可证 · 活动图 | 草稿 |
| 02-SYSTEM.html | 系统架构 | Bun Crate DAG · Servo 组件 · 融合映射 · 管道 · 集成 · NFR · SLA · 接口协议 · 事件目录 · 运行时状态图 | 草稿 |
| 03-PROCESS.html | 核心流程 | JS 执行管线 · 渲染管线 · CDP 路由 · 状态机 · 时序约束 · 线程模型 · 异常流程 | 草稿 |
| 04-DATA-MODEL.html | 数据模型 | 13 Entity · 模型树 · 缓存策略 · Crate 数据流 | 草稿 |
| **05-IMPLEMENTATION.html** | **实施路线图** | **6 阶段任务分解 · 复用矩阵 · 风险矩阵 · 验证点** | 草稿 |
| 10-REQUIREMENTS.html | 功能需求 | 27 REQ · 5 域 (ENG/CLI/BRW/CDP/STL) · 5 NFR · 追溯矩阵 | 草稿 |

### 验证状态

`spec_validate` — 0 errors / 0 warnings / HEALTHY

---

## 开发计划 (详见 .spec/05-IMPLEMENTATION.html)

| 阶段 | 名称 | 核心 REQ | 新建 Crate |
|------|------|---------|-----------|
| Phase 1 | SpiderMonkey 替换 JSC | REQ-ENG-001~007 · REQ-CLI-001 | bao_engine + 12 桥接 + bao_runtime |
| Phase 2 | 浏览器模式接入 | REQ-CLI-002 · REQ-BRW-001/003 | bao_browser |
| Phase 3 | 内存渲染 | REQ-BRW-002 | bao_render |
| Phase 4 | CDP Server | REQ-CDP-001~008 | bao_cdp |
| Phase 5 | Stealth 反指纹 | REQ-STL-001~007 | bao_stealth |
| Phase 6 | 集成测试与发布 | NFR | — |

---

## 技术栈

| 组件 | 来源 | 用途 |
|------|------|------|
| SpiderMonkey | mozjs crate (MPL-2.0) | JS 引擎 |
| libservo | servo crate (MPL-2.0) | DOM + CSS + Layout |
| Vello CPU | linebender/vello (Apache-2.0/MIT) | 内存渲染 |
| taffy | DioxusLabs/taffy (MIT) | CSS 布局 |
| cdp-protocol | cdp-protocol crate (MIT) | CDP 类型定义 |
| Bun 基础设施 | ~85 个纯 Rust crate (MIT) | HTTP/FS/Resolver/Bundler/... |

## 新增 Crate 结构

```
src/
├── [Bun 现有 crate 保留不动 (~85 个)]
├── bao_engine/         # SpiderMonkey 引擎封装 (替代 bun_jsc)
├── bao_browser/        # 浏览器模式 (libservo 封装)
├── bao_render/         # 内存渲染 (Vello CPU + taffy)
├── bao_cdp/            # CDP 协议服务 (11 domain)
└── bao_stealth/        # 反指纹模块 (TLS/HTTP/Canvas/Navigator/Behavior)
```
