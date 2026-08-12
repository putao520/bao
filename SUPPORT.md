# Support

本文档帮你找到正确的求助渠道。**它不是 issue tracker 的替代品**,而是把不同问题分流到合适的地方。

---

## 文档(Documentation)

动手前先查文档,大多数基础问题文档里已有答案:

| 资源 | 路径 | 用途 |
|------|------|------|
| README | [`README.md`](./README.md) | 项目概览、快速开始、构建与测试命令 |
| 项目规范 | [`CLAUDE.md`](./CLAUDE.md) | 架构分层、核心原则、Bun crate 复用映射、编程规范 |
| SPEC 体系 | [`.spec/`](./.spec/) | 唯一契约真源(01-BUSINESS / 02-SYSTEM / 03-PROCESS / 04-DATA-MODEL / 10-REQUIREMENTS / 11-TESTING) |
| 设计文档 | [`docs/`](./docs/) | 设计参考、域知识库 |

---

## Bug 报告(Bug Reports)→ GitHub Issues

如果你确认遇到了 bug(不是安全问题),请到 GitHub Issues 提交:

👉 https://github.com/putao520/bao/issues/new

### 如何写一个好的 bug report

**糟糕的报告**:"bao 跑不起来" — 无法诊断。

**好的报告**应该包含:

1. **标题**:一句话描述症状,带组件名
   - 例:`[bao_browser] PagePool 在 navigate 第二个 page 时 SIGSEGV`
2. **复现步骤**(最重要):编号列出,从干净状态开始
   ```
   1. cargo build -p bao_bin
   2. ./target/debug/bao browser --url https://example.com
   3. 在 REPL 里执行 PagePool.create("https://example.org")
   4. 观察到 SIGSEGV
   ```
3. **期望行为**:你预期发生什么
4. **实际行为**:实际发生了什么(完整错误输出、stack trace、coredump)
5. **环境信息**:
   - `bao --version` 或 git commit hash
   - `bao doctor` 的完整输出(诊断信息:引擎/浏览器/CDP/Stealth 模块状态)
   - 操作系统 + 架构(`uname -a`)
   - Rust 工具链(`rustc --version` / `cargo --version`)
   - 是否启用 CDP(`--cdp-port`),Stealth profile
6. **最小复现脚本**:能附上一个 `.js` / `.sh` 脚本最理想
7. **日志**:开了 `RUST_LOG=debug` 的输出

> 💡 **提示**:运行 `bao doctor` 会输出系统环境诊断,直接把它的完整输出贴进 issue 能省很多来回。

---

## 问答 / 讨论(Questions & Discussions)→ GitHub Discussions

通用问题、用法咨询、架构讨论、展示你用 Bao 做的东西,**不要开 issue**——issue 只接 bug 和具体 feature request。

👉 https://github.com/putao520/bao/discussions

 Discussions 打开后,推荐分类:

- 💬 **Q&A** — 用法问题
- 💡 **Ideas** — 新功能建议、方向讨论
- 🙌 **Show and tell** — 用 Bao 做的项目展示

> Discussions 当前可能尚未开启(取决于仓库配置)。如果未开启,暂用 issue 的 `question` label 替代。

---

## 安全漏洞(Security Vulnerabilities)→ SECURITY.md

**绝对不要在公开 issue / discussion 里报告安全漏洞。**

请走私密渠道:

👉 见 [`SECURITY.md`](./SECURITY.md)

支持的漏洞类别:CDP 鉴权绕过 / 反指纹泄露 / RCE / 沙箱逃逸 / Node.js 兼容层权限越界 / 加密实现错误 / 内存安全 bug。

---

## 保持更新(Stay Updated)

- **Watch repository** — 点 repo 右上角 ⭐ 旁的 Watch,选 Custom → Releases / Discussions / Security alerts
- **Discussions Announcements** — 关注 `announcements` category(若有重要变更、breaking change、CVE 修复会发在这里)
- **Release notes** — 每个 tagged release 会在 GitHub Releases 页有详细 changelog

---

## 维护者预期(Alpha 阶段)

Bao 当前是 `0.1.0-alpha`,由小团队维护。意味着:

- ✅ 我们会认真对待每个 issue 和 discussion,但响应时间可能较长(几天到一周)
- ✅ Critical bug / 安全漏洞 优先
- ❌ 我们无法保证每个 feature request 都会被实现
- ❌ alpha 阶段 API 可能有 breaking change,不保证向后兼容

感谢耐心 💛 —— 报告问题、写文档、补测试都是贡献。
