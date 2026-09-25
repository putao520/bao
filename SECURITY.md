# Security Policy

Bao 是一个可嵌入 Rust 应用的 JS/TS system runtime，组合 SpiderMonkey、Rust-native Node.js/Bun-compatible system APIs、Servo Web Runtime、CDP compatibility 与 Stealth。Bao 当前仍处于 0.x 快速迭代阶段，安全加固持续进行中。本文档说明如何负责任地报告安全漏洞。

> **诚实声明**:Bao 尚未达到稳定版（1.0）承诺。0.x 阶段 API、平台支持与安全边界仍可能变化；尤其是 CDP、浏览器/Web Runtime、Node.js/Bun 兼容层、FFI/native bridge 与宿主资源边界仍在持续加固。我们欢迎安全研究，但请不要将 0.x 版本视为已经 fully hardened 的生产软件。

---

## 支持的版本

| Version | Supported | Notes |
|---------|-----------|-------|
| 最新 tagged 0.x release | ✅ 安全修复 | 当前支持版本；报告前请先在最新 tagged release 上复现 |
| 旧的 0.x release / 开发快照 | ❌ 不承诺回补 | 请升级到最新 tagged release 后再验证 |

0.x 阶段我们**不会承诺为旧版本 backport 安全修复**——安全修复进入最新 tagged release。若问题只存在于旧版本而最新版本已修复，请在报告中注明受影响版本范围。

---

## 报告漏洞(Security Vulnerability Disclosure)

**请勿通过公开 GitHub Issue 报告安全漏洞。** 公开 issue 会让漏洞在修复前暴露,危害所有用户。

### 正确的报告渠道

使用 GitHub Private Security Advisory(私密安全公告):

1. 前往 https://github.com/putao520/bao/security/advisories/new
2. 点击 **"Report a vulnerability"**
3. 填写:
   - **漏洞标题**(简短描述,例如 "CDP Server 默认监听 0.0.0.0 导致远程代码执行")
   - **影响版本**(确认在最新 tagged release 上可复现)
   - **严重度自评**(参考下方"范围"和 CVSS 量表)
   - **复现步骤**(可运行的 PoC 最理想;最小复现脚本/命令)
   - **影响面**(攻击者能做什么?需要什么前置条件?本地 / 同网段 / 远程?)
   - **建议的修复方向**(可选,但欢迎)

### 报告时应包含的信号

越完整的报告越快得到响应。**至少**包含:

- 受影响的组件(`bao_engine` / `bao_browser` / `bao_cdp` / `bao_stealth` / `bao_runtime` / `bao_uloop` / 其他)
- 操作系统 + 架构(`uname -a` 的输出,Rust 项目跨平台行为差异大)
- Bao 版本(`bao --version` 或 git commit hash)
- `bao doctor` 的输出(如果可运行)
- 是否启用 CDP Server(`--cdp-port` 参数)
- Stealth 配置 profile(如果涉及反指纹泄露)

---

## 响应 SLA

| 阶段 | 承诺时间 | 说明 |
|------|---------|------|
| **确认收到报告** | 48 小时内 | 维护者确认收到,指派处理人 |
| **初步评估** | 7 天内 | 给出严重度评级(Critical / High / Medium / Low)与是否接受为安全问题 |
| **修复 ETA** | 视严重度 | Critical 7 天 / High 30 天 / Medium 60 天 / Low 下个 release |
| **修复发布 + 致谢** | 修复后 7 天内 | 发布 patch release,在 release notes 致谢报告人(除非报告人要求匿名) |

> alpha 阶段维护者精力有限,Critical/High 优先。如约定 ETA 无法达成,我们会在 advisory 内同步进展,不会静默失联。

---

## 范围(Scope)

### 接受为安全问题

以下类别**属于**安全漏洞,请走私密报告渠道:

- **CDP 鉴权绕过 / 未授权访问** — CDP Server 默认监听 `127.0.0.1:9222`,若可被远程连接、target 越权或缺失鉴权则属于安全问题
- **反指纹(Stealth)泄露** — `bao_stealth` 未能覆盖的指纹向量(TLS JA3/JA4 / HTTP2 AKAMAI / Canvas / WebGL / Audio / Navigator / Behavior),导致用户被追踪
- **远程代码执行(RCE)** — 任何能让攻击者在宿主机执行任意命令的漏洞(渲染引擎 / SpiderMonkey / FFI 桥接 / Bun API 兼容层)
- **沙箱逃逸** — Web content 逃出 servo 沙箱访问文件系统 / 网络 / 子进程
- **Node.js / Bun 兼容层权限越界** — `bun_runtime` 的 `fs` / `http` / `crypto` / `sqlite` / `ffi` API 被绕过访问到不该访问的资源
- **加密实现错误** — `bao_crypto` / `bao_boringssl_bridge` 中的密钥处理、随机数、TLS 配置错误
- **内存安全 bug** — `unsafe` 块导致的 use-after-free / buffer overflow / SIGSEGV(需能被外部输入触发)

### 不算安全问题(走普通 GitHub Issue)

- 普通功能性 bug(渲染异常 / API 行为不符合 spec / 性能问题)
- 编译警告 / 文档错别字 / 依赖版本陈旧
- 未被外部输入触发的 panic(纯内部状态机错误)
- "理论上的"问题,无法给出复现路径
- 已经在公开渠道讨论过 / 已知限制

如果你拿不准是安全问题还是普通 bug,**优先走私密报告**让维护者评估。

---

## 不予奖励 / 不允许的测试行为

- ❌ **未经授权对任何公开部署的 Bao 实例进行测试**。Bao 目前没有官方公开实例,但如果你发现了某个第三方部署,**不要**对其测试,即使是为了"验证漏洞"——请直接报告给该部署的运营者,或向本项目报告设计层面的缺陷。
- ❌ **DoS / 大规模 fuzzing 公开服务**。本地 fuzzing 欢迎并鼓励,但对任何非你自己的实例进行 DoS 测试都是禁止的。
- ❌ **社工 / 钓鱼项目维护者**以获取信息或访问权限。
- ❌ **公开发布未修复漏洞的 PoC**(在协调披露窗口期内)。

### 允许且鼓励的测试

- ✅ 在你**自己**的本地构建上测试 / fuzz / 逆向
- ✅ 对 `bao run` 起的本地实例做任何测试
- ✅ 研究 servo / SpiderMonkey / boringssl 上游的已知 CVE 是否影响 Bao

---

## 致谢

安全报告者会在漏洞修复后的 release notes 中得到致谢(除非要求匿名)。Bao 暂无 bug bounty 奖金,但你的贡献会被公开记录。

---

## 联系

- 私密报告:**GitHub Private Security Advisory**(https://github.com/putao520/bao/security/advisories/new)— 首选
- 公开问题(非漏洞):GitHub Issues(https://github.com/putao520/bao/issues)

感谢帮助 Bao 变得更安全。
