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

**基线记忆**(auto-memory,吸收后必须更新):
- `bun-layer-sync-baseline` — Bun 层最后全量同步点
- `mozjs-upgrade-decisions` — mozjs/servo 升级方法论 + 陷阱

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

## 2. 分类判定(每项必须落到其一)

| 判定 | 判据 |
|---|---|
| **吸收** | Bao 有对应代码且同 bug 确认(grep 到 pre-fix 形状) |
| **已含** | 代码级特征串在 Bao 命中 post-fix 形状 |
| **不适用** | Bao 无对应结构(如无 `src/runtime/`、无某 API),注明 Bao 的对应实现是什么 |
| **需进一步判断** | 证据不足时**不猜**,标出来交主会话 |

C/Zig 文件:非 Rust 但 Bao 有对应 C 源(packages/bun-usockets 等)时仍可吸收;servo 的 .ini/WPT 测试期望默认跳过。

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
3. **更新基线 memory**(新同步点 hash + 日期)
4. "需进一步判断"残留项登记任务,不静默丢弃

## 6. 历史成果参考

- 2026-08-13/14 大吸收:Bun P0 三项(bodyless/UAF/spawn stdin)+ fetch 链路根治 + JSC 清光 + mozjs 0.21.4 全量升级(7314→0)。方法论完整记录在各 commit message 与 memory。
