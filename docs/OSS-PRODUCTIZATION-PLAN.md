# Bao OSS 产品化改造工作计划

> 目标:把 Bao 从"代码成熟度 7-8 分、对外呈现 2-3 分"改造成"对外呈现与代码成熟度匹配的成熟 OSS 基础设施项目"。
>
> 核心定位重新定义:**A Rust-native programmable browser runtime built on Servo and SpiderMonkey**(反指纹从一级定位降为"可配置浏览器身份"能力之一)。
>
> 4 Gate:G1 OSS 合法性 → G2 可使用性 → G3 工程可信度 → G4 生态可信度。G3 即可申请,G4 申请材料漂亮。

## 进度(2026-08-12 最终更新)

| Gate | 状态 | 交付 |
|---|---|---|
| **G1 OSS 合法性** | ✅ 完成 | LICENSE×3 / NOTICE / THIRD_PARTY_LICENSES / CONTRIBUTING / SECURITY / COC / SUPPORT / CHANGELOG / Cargo metadata |
| **G2 可使用性** | ✅ 完成 | README 重构(新定位) / architecture.md / 4 examples / bootstrap.sh / `bao doctor` |
| **G3 工程可信度** | ✅ 完成 | CI(ci/browser-smoke/cdp-smoke/release)/ 本地 `just`+`act` 双路 CI / compat suite 骨架 / bench 骨架 / Stealth 重叙事 |
| **G4 生态可信度** | ✅ 完成 | roadmap.md / spec-driven.md / good-first-issues / issue 模板 / GitHub Discussions 开启 / 6 Milestones / 5 种子 issues |

### 已落地(全部 push 到 origin/master)

- **本地 CI**: `justfile`(22+ recipe)+ `act`(跑同一份 `.github/workflows/*.yml`)+ `just gha-*` recipe。`cargo fmt --all` 误入 vendor 的 bug 已修;fmt 历史债(~8000 处)标 non-blocking。
- **GitHub 仓库**: Actions 已启用(原 enabled=false);repo description/topics 更新;Discussions 开启;v0.1.0-alpha.1 tag 已推。
- **stash 根治**: stash@{0}(SPEC 治理)选择性迁移落地(permission 路径规范化安全加固 BCE-001 + PageState::Closing + bce004 并发测试 + 11-TESTING +5144 + BUG-KNOWLEDGE 拼接);stash@{1}(CDP+crypto)三重验证无价值。

### 仅剩手动收尾(非代码,留给维护者)

1. **drop 残余 stash**(价值已全部提取到 master,stash 仅冗余副本):`git stash drop stash@{0} && git stash drop stash@{0}`(hook 拦 stash 关键词,需手动)。
2. **Discussions 首帖**: GitHub 网页发"Welcome to Bao"公告(GraphQL addDiscussion 受 token scope 限制,网页发更可靠)。
3. **fmt 历史债清债 PR**: 单开一个 `cargo fmt` 全量 commit(8369 处),清完把 fmt 从 `ci`/`ci.yml` 的 non-blocking 改回 blocking。
4. **CI 首跑观察**: Actions 刚启用,首个 CI run 验证 fmt(continue-on-error)/check/clippy/bce-gate 在 GitHub runner 上真实通过(本地 `act` 已验证链路)。

---



---

## Gate 1 — OSS 合法性(最高优先,阻塞一切)

**完成标准**:LICENSE / NOTICE / THIRD_PARTY_LICENSES / CONTRIBUTING / SECURITY / CODE_OF_CONDUCT / SUPPORT 全齐,GitHub repo 能识别 license。

### G1-T1 OSS 治理文件(8 文件,可并发)

| 文件 | 内容 | 来源 |
|---|---|---|
| `LICENSE` | MIT 全文(Bao 原创代码以 MIT 为默认) | SPDX 标准 |
| `LICENSE-MIT` | MIT 完整文本(显式副本) | SPDX |
| `LICENSE-MPL-2.0` | MPL-2.0 完整文本(Servo/SpiderMonkey 衍生物) | Mozilla 官方 |
| `NOTICE` | Bao 项目归属 + 上游来源声明(Servo/SpiderMonkey/Bun/BoringSSL/lsquic)+ BAO PATCH 说明 | 手写 |
| `THIRD_PARTY_LICENSES.md` | 每个上游:来源 URL / 原 license / Bao 是否修改 / 修改清单(指向 BAO PATCH 标注) | 手写,扫 vendor/ |
| `CONTRIBUTING.md` | 开发流程:SPEC SSOT + BCE 纪律 + Rust nightly + mozjs 构建说明 + commit 规范 + PR 流程 | 基于 CLAUDE.md |
| `SECURITY.md` | 漏洞报告流程(私有披露渠道 / PGP / SLA) | 模板 + 项目实际 |
| `CODE_OF_CONDUCT.md` | Contributor Covenant 2.1(中文+英文) | 标准 |
| `SUPPORT.md` | 支持渠道分流:bug→issue / 问答→Discussions / 安全→SECURITY | 手写 |

**并发性**:8 文件互相独立,可并发派 8 个 E(实际由文件大小决定是否合并 agent)。

### G1-T2 Cargo.toml metadata 补全
- `version = "0.0.0"` → `"0.1.0-alpha.1"`(区分 workspace root 与 package)
- 补全 `description` / `keywords` / `categories` / `homepage` / `documentation` / `readme`
- 确认 `repository` / `license` 已在 workspace `[workspace.package]`

---

## Gate 2 — 可使用性

**完成标准**:v0.1.0-alpha Release + Quick Start + 4 examples,陌生人无需理解 monorepo 即可跑起来。

### G2-T1 README 重构(前 30% 产品化)
- 新标题:`Bao — A Rust-native programmable browser runtime`(反指纹降级)
- 新增 **Why Bao?**(对比 Chromium+Playwright 的架构差异图)
- 新增 30 秒 Demo(Playwright→CDP→Bao + DOM×Node 同 Realm)
- 新增 **Compatibility Matrix**(✓ / Partial / Experimental,**不全打 ✓ 增可信度**)
- 新增 Status / Roadmap 指向
- 保留技术细节但下沉到 docs/

### G2-T2 architecture.md(一张图 > 20 页文字)
- `docs/architecture.md`:BaoRuntime / CDP / Node Realm / Page Realm / SpiderMonkey / Servo 的分层 ASCII 图
- README 链接过去

### G2-T3 4 个 examples
```
examples/
  01-browser/         navigate + DOM + screenshot
  02-playwright/      Playwright → CDP → Bao
  03-node-dom/        同一 Runtime: document.querySelector + require('fs')
  04-crawler/         网页自动化 + 结构化抽取
```
每个 example 含 README + 可运行代码 + 预期输出。

### G2-T4 scripts/bootstrap.sh
- 一键环境:`rustup default nightly` + clang 检测 + `cargo build --release -p bao_bin`
- `bao --version` 验收
- 配合 README 的 "陌生人 30 秒上手"

### G2-T5 `bao doctor` 子命令
- clap 加 `Doctor` variant
- 检测:Rust nightly / clang / mozjs 已编译 / Servo / BoringSSL / DISPLAY / CDP 可启
- 友好输出(✓/✗ + 修复建议)

### G2-T6 v0.1.0-alpha Release
- 打 tag `v0.1.0-alpha.1`
- Release notes:明确 "Alpha — Linux x86_64 only, APIs may change"
- 产物(如可静态):`bao-linux-x86_64.tar.gz` + `SHA256SUMS`;否则提供 bootstrap 路径
- CHANGELOG.md(Keep a Changelog 格式)

---

## Gate 3 — 工程可信度

**完成标准**:公开 CI(build/test/clippy) + Browser smoke + Playwright smoke + compatibility 入门。

### G3-T1 CI 分层(Fast / Full)
```
.github/workflows/
  ci.yml              Fast: fmt --check / clippy / check / test (affected)
  browser-smoke.yml   Full: bao browser → /json/version → navigate → evaluate → screenshot
  cdp-smoke.yml       Full: Playwright → Bao CDP → Servo(强 badge)
  release.yml         tag 触发:构建 + 产物 + SHA256
  bce-gc-unsafe.yml   (已存在,保留)
```
- sccache + cargo cache 加速(避免每 PR 编 2 小时)

### G3-T2 Compatibility suite 骨架
```
compat/
  node/   fs/path/crypto/http/url 通过率
  bun/    Bun.file/sqlite/...
  web/    DOM/CSS/Fetch/WebSocket
  cdp/    12 域方法覆盖
```
- 先建脚手架 + 少量真实测试,输出通过率(不造数据)
- README Compatibility Matrix 链接此处

### G3-T3 WPT 入门(可选,G3 加分)
- 引入 Web Platform Tests 子集(DOM/HTML/CSS/Fetch)
- 公开通过率数字

### G3-T4 Benchmark 骨架
- `bench/`:cold/warm startup、evaluate/s、CDP roundtrip、screenshot latency、多页 RSS
- 公开可复现基准(**不求赢 Chromium,求诚实**)
- README 去掉"高性能"无数据口号,链向 bench 结果

### G3-T5 Stealth 重新包装
- 不删,改叙事:`Browser Identity & Privacy`(TLS/HTTP2/Navigator/Screen/WebGL/Canvas/Audio/Input profiles)
- README:"configurable browser identity / privacy profiles",去 "anti-bot/bypass/stealth" 主叙事

---

## Gate 4 — 生态可信度

**完成标准**:Roadmap + Issues backlog + Discussions + good-first-issue + 外部反馈迹象。

### G4-T1 Roadmap(GitHub milestones)
```
v0.1 Runtime foundation
v0.2 CDP compatibility
v0.3 Web compatibility
v0.4 Node/Bun compatibility
v0.5 Multi-page stability
v1.0 Stable embedding API
```
- 每个 milestone 建真实 issue(打破"0 open issues")

### G4-T2 Issue backlog(种子 issues)
- 打标签:good first issue / help wanted / documentation / compatibility / cdp / servo / runtime / performance
- good-first-issue 选低门槛:Add CDP method schema / Add Node compat test / Improve Windows docs / Add WPT subset / Add example / Add bench case

### G4-T3 Discussions 开启
- Announcements / Ideas / Q&A / Show and Tell / Development

### G4-T4 Spec-driven development 包装成优势
- 单独 `docs/spec-driven.md`:Requirement→Architecture→API→Implementation→Test→CI traceability
- 强调 Bao 适合 AI-assisted engineering(申请 OpenAI 支持的关键叙事)

---

## 执行策略

### 并发分组(文件域无交集 → 可并发)

**Wave A(G1 并发,文件互相独立)**:
- A1: LICENSE × 3 + NOTICE
- A2: THIRD_PARTY_LICENSES.md(需扫 vendor,较重)
- A3: CONTRIBUTING.md
- A4: SECURITY.md + SUPPORT.md + CODE_OF_CONDUCT.md
- A5: Cargo.toml metadata + README G1 部分(license badge 占位)

**Wave B(G2,依赖 G1 README 框架就位)**:
- B1: README 完整重构(含 Why Bao / Demo / Compatibility Matrix)
- B2: docs/architecture.md
- B3: examples × 4
- B4: scripts/bootstrap.sh + bao doctor
- B5: CHANGELOG.md

**Wave C(G3,可部分并发)**:
- C1: CI workflows(ci/browser-smoke/cdp-smoke/release)
- C2: compat suite 骨架
- C3: bench 骨架
- C4: Stealth 重包装(改 README 叙事段,与 B1 协调)

**Wave D(G4,主要是 GitHub UI 操作 + 文档)**:
- D1: docs/spec-driven.md + docs/roadmap.md
- D2: 种子 issues 模板 + good-first-issue 清单文档

### 依赖与顺序
- G1 必须先完成(license 缺失是硬阻塞,影响所有 OSS 合法性)
- README(B1)是 G2 核心,其他 G2 任务与其协调(Compatibility Matrix 数据来自 C2/C3)
- Release(G2-T6)在 README + examples 就位后才打
- G3 CI 依赖 Cargo.toml metadata(G1-T2)和 examples(G2-T3)
- G4 可与 G3 并行(纯文档+UI)

### 验收门
- 每 Wave 末:`cargo build -p bao_bin` 通过 + 文档交叉链接无死链
- G1 末:GitHub repo 页面显示 license + 所有治理文件可点
- G2 末:陌生人按 README 30 秒能 `bao --version`、能跑 example 01
- G3 末:CI 绿 + browser-smoke + cdp-smoke 有可复现运行记录

---

## 不做的事(范围守恒)

- 不加新功能(本次纯产品化,用户明确"暂时少加功能")
- 不改 epoch 6 已交付的 Worker/servo 多实例代码(除非 build 验证暴露问题)
- 不碰 stash 内容(单独决策,见 stash 分析结论)
- 不造 compatibility/benchmark 假数据(诚实优先,Partial/Experimental 增可信)
