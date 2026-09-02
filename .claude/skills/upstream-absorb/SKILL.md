---
name: upstream-absorb
description: 上游吸收流水线(Bun + servo)。扫描上游增量窗口 → 代码级核实分类 → 并行派工吸收 → BCE 文件 patch 重放 → 波末收口。当用户说"看看上游有什么可以吸收/吸收上游/upstream absorb/同步上游",或定期同步时使用。含真源路径、基线管理、假阳性教训、已知陷阱库。
---

# 上游吸收流水线(upstream-absorb)

Bao 是 Bun(Rust 层移植)+ servo(vendor)的下游。本 skill 固化增量吸收的完整流程,避免重新摸索。

## 0. 真源与基线(动手前先读)

| 上游 | 本地路径 | 注意 |
|---|---|---|
| Bun | `~/code/rust/bun` | `packages/bun-usockets`/`bun-uws` 的 C 源**在 Bao 仓库内**(`packages/` 目录,`src/uws_sys/build.rs:14` 编译) |
| servo | `~/code/tools/servo` | **不是** `~/code/rust/servo`(已失效,CLAUDE.md 曾错记) |
| mozjs | `vendor/mozjs/`(本仓库 vendor) | cargo 缓存的源**可能被前人改过**,取源先 diff pristine `.crate` |

**基线真源**:`.claude/upstream-baseline.json`(仓库内机器可读,含 bun/servo baseline hash+updated_at)。auto-memory 只保留方法论(陷阱/教训),不再存 hash;禁从 memory 读基线。波末收尾必须更新该文件并与 wave commit 同一提交。

**BCE 定制文件清单**:servo 侧 10 个文件(script_thread/script_runtime/dedicatedworkerglobalscope/constellation/event_loop/lock.rs/connector.rs 等)以"upstream 基底 + Bao patch 重放"维护,**完整清单与锚点见 CLAUDE.md**。任何触及它们的同步 = patch 重放,标高成本。

## 1. 扫描阶段(spawn explore-lsp agent,只读)

```bash
# Bun 窗口(基线 hash..origin/main)
cd ~/code/rust/bun && git fetch origin && git log --oneline <BASELINE>..origin/main
# servo 窗口
cd ~/code/tools/servo && git fetch origin && git log --oneline <BASELINE>..origin/main
```

### 铁律:代码级特征字符串,禁用 commit message 措辞
扫描验证某修复是否已含时,grep 的特征串必须取自**代码里的标识符/常量/条件**(如 `MAX_RESPONSE_HEADER_BUFFER`、`clear_hostname_on_redirect`),不能用 commit 标题措辞(如 "Content-Length mismatch")——措辞只存在于 commit message,代码里是别的名字。本 session 曾因此把"已全吸收"误判为"缺失 7 项"。

### 第二铁律:名字 grep 有 ~90% 假阳性
`JSC`/`jsc` 会命中 `JSContext`/`JSClass`(SpiderMonkey API 名)。任何"残留/缺失"结论必须人工抽样核实命中上下文。

## 1.5 扫描自动化三招(第一道过滤器,先跑再人工)

1. **hash 前置检测**:每个候选先 `grep -rl "<hash>" src packages vendor` —— Bao 有在注释里引用上游 hash 的惯例,命中即"已含"秒判。
2. **pre-fix 指纹比对**:对 fix 类 commit 取 diff 的**删除行**(pre-fix 形状,如 `value.len() > 1`、`const int eof = events & EPOLLHUP;`),直接 grep Bao 对应目录——命中即"同 bug 确认",比读上下文快一个量级。不命中再看形状差异。
3. **窗口规模校正**:起手 `git rev-list --count <BASELINE>..origin/main` + 按目录分类 stat 汇总(install/CI/windows 类整批跳过)——禁用"约 N 个提交"的估算规划派工(本轮估算 15 实际 73)。

### 已知路径映射(上游 → Bao,避免每轮考古)

| 上游路径 | Bao 落点 |
|---|---|
| `packages/bun-usockets/*` | `packages/bun-usockets/`(`src/uws_sys/build.rs:14` 编译;`src/uws_sys/libuwsockets.cpp` 是另一层桥、无事件循环,非吸收目标) |
| `src/http_jsc/websocket_client.rs` | 未移植(`src/http/websocket_http_client.rs` 为空) |
| `src/runtime/server/RequestContext.rs` + `webcore/streams.rs` | 无对应结构(Bao 用 servo 处理网络) |
| `src/runtime/*`(JSC 钩子层) | 不适用(SM 替代,无 JSC 钩子) |



## 2. 分类判定(每项必须落到其一)

**吸收律(用户裁决 2026-08-23 精化)**:消费面按指纹吸收;未消费面按**问题域迁移**审计——上游 commit 修的问题,Bao 自有实现是否有同款?禁为吸收破坏自有实现,禁制造无意义双实现。

| 判定 | 判据 |
|---|---|
| **吸收** | Bao **消费**的上游代码(bun_*/vendor servo 承载面)且同 bug 确认(grep 到 pre-fix 形状)——修进承载代码 |
| **已含** | 代码级特征串在 Bao 命中 post-fix 形状 |
| **不适用(纯)** | Bao 无对应结构**且无问题域重叠**(windows/darwin/test/ci/docs、上游独有 API 面)——注明理由 |
| **域审计** | Bao 未消费上游该部分,但**自有实现处于同一问题域** → 禁移植上游代码、禁双实现;以自有 idiom 检查自有实现是否有同类 bug/缺口——有则立项修自有实现(引用上游 hash 作 oracle,不搬代码),无则记证据关闭;**na 判定凡注明"Bao 自有实现"者必须落到本类并产出审计结论**,不得只标 na 就走 |
| **需进一步判断** | 证据不足时**不猜**,标出来交主会话 |

C/Zig 文件:非 Rust 但 Bao 有对应 C 源(packages/bun-usockets 等)时仍可吸收;servo 的 .ini/WPT 测试期望默认跳过。

### Provenance 门禁(分诊前置,强制)

对照 `.claude/crate-provenance.json`(crate 来源 SSOT):

1. **mirror 且 divergence≠none 的 crate 禁 DIRECT-ABSORB**——必须走**分歧核对**(patch 重放到分叉面是否仍成立 / 与自有实现语义对照),核对结论写入 triage 记录后才可吸收;跳过核对直接吸收 = 违规(会静默覆盖已登记分叉)
2. **闭包与吸收涉面(含 non_member_domains)必须已登记**:凡进入发布闭包的 crate、或本次吸收触及的目录域(含 `.claude/crate-provenance.json` 顶层 `non_member_domains` 非成员域),若不在 provenance 清单中,先补登记(目录 diff + origin 判定 + crates.io 实查)再发布/再吸收,禁发布或吸收未登记域——门禁判定源统一为该清单

## 3. 派工吸收(并行 implementer,合同模板)

每个 E 的 prompt 必含(scope/completion/retry/stop),核心结构:

```
## 修复 N:<hash> <一句话>
- 上游:git show <hash>(先读 diff,禁凭记忆移植)
- Bao 位置:<file:line>(已核实 pre-fix 状态)
## scope: 允许改 <精确文件列表>;禁 vendor 其他文件/commit/push/全量 test
## completion: 落地 + cargo check -p <crate> --jobs 1 过(+ 单测如有)
## retry: 编译错修 1 次
## stop: 上游形状与 Bao 差异过大/语义不明 → 停,报告差异,不猜
```

并行原则:不同 Agent 的文件集零重叠;servo 的 BCE 文件与普通文件分开派。

## 4. 已知陷阱库(派工前过一遍)

1. **cargo 缓存源被污染**:mozjs-0.21.4 缓存曾有方向写反的 patch(conversions.rs),必须 diff pristine `.crate`
2. **rsync -a 保留旧 mtime** → cargo 复用 stale proc-macro 产物(`crate::node` 幽影错误);批量同步后 `touch` 全树
3. **webidl 联动**:同步 servo DOM .rs 必须伴随 `webidls/` 同步;codegen 由 `script_bindings/build.rs` 自动触发(无需 mach)
4. **上游 WIP 断引**:servo main 曾有 `node/node.rs → crate::dom::context` 不存在;逐文件验证,不盲信 HEAD 可编译
5. **bindgen namespace 陷阱**:mozjs fork 的 C 声明若落 `namespace JS` 内,mangled link_name 与全局作用域定义不匹配(`JS_NewEmulatesUndefinedFunction` 踩过)
6. **双路径同步**:改 usockets 事件标记语义(如 EPOLLHUP 的 LIBUS_POLL_EOF/HUP)时,`src/bao_uloop/src/poll.rs` 的 Rust epoll 路径必须同改,否则 JS 线程与 HTTPThread 语义分叉
7. **crate 名陷阱**:目录 `src/bao_runtime` 的包名是 `bun_runtime`;`bao_engine` 测试编译用 `cargo check -p bao_engine --tests`

## 5. 收口(波末一次)

1. 全部 Agent 报告后统一验证:`cargo check` 相关 crate + 关键测试 binary
2. 一个吸收 wave 一个 commit(message 列全部 hash + 判定摘要);push
3. **更新 .claude/upstream-baseline.json 并入 wave commit**
4. "需进一步判断"残留项登记任务,不静默丢弃
5. **发布闭包(用户裁决 2026-08-23:测试过的波必须发布,与 daily-ops 波同轨)**:wave push 后立即执行 crates.io 变更闭包发布(变更闭包 patch bump → dry-run → publish → curl 200 验证)+ `git tag daily-<YYYYMMDD>` + `gh release create`(notes 取 wave commit message)。完整协议:`../daily-ops/references/publish.md`(版本策略/发布序/限流/失败语义/CLI 永不发布)。禁只 commit 不发布——发布是波末收口的组成部分,不是可选项。

## 6. 历史成果参考

- 2026-08-13/14 大吸收:Bun P0 三项(bodyless/UAF/spawn stdin)+ fetch 链路根治 + JSC 清光 + mozjs 0.21.4 全量升级(7314→0)。方法论完整记录在各 commit message 与 memory。
