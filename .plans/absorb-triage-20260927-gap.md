# 吸收裁决积压:b820a9679..7ca99fe3f(143 commits)

> 缺陷:2026-09-27 夜间波只吸收 6 颗 cherry-pick 却把 upstream-baseline.json bump 到 7ca99fe3f(基线语义=triage 收口边界,被虚假推进)。
> 本台账 = 逐桶裁决 + 已执行动作。吸收方向恒为「向基线同步」;BAO 补丁文件(CLAUDE.md servo 清单 31+ 条)禁盲同步,走重放。

## 已执行(本轮)

| 动作 | 文件 | 状态 |
|---|---|---|
| **webvtt 家族吸收(已整体回退,终态=fork 旧自洽态)**:家族同步后暴露 crown 纪元耦合——new texttracklist 构造器变更耦合 htmlmediaelement(580 行 crown 结构漂移)、no_gc codegen 形态(fork 前 crown)、bytes::Bytes/job_queue 运行时形态(fork 已改 microtask+Vec<u8>)。逐项适配 5 错后又见更深消费者耦合,与 paint 同款螺旋,死线纪律全回退(dom 7 文件+crate+htmltrackelement;孤儿 cue/tests/collectors 一并移除)。**GetCueAsHTML/activeCues/cue-order 功能件归闭合波**(与 E3 REQ-BRW-047 合同一体:需同步 dom 家族+htmlmediaelement+fork 形态适配打包)。 | dom/webvtt/* + webvtt crate + htmltrackelement.rs | 回退后 `cargo check -p bao-servo-script` **RC=0 全绿**(E1 域内自修+回退自洽双因素);webvtt crate RC=0 |
| fonts freetype 双修(16.16 转换/average_advance 缩放) | fonts/platform/freetype/font.rs | 零 BAO 锚;回退验证 `cargo check -p bao-servo-fonts -p bao-servo-constellation` RC=0(13.7s) |
| **paint timing 闭合实验(已回退)**:paint crate 预存断裂(HEAD 20 错:消费 PaintTimingInfo/PaintTimingReport 而 paint_api 无生产侧)。逐跳同步 paint-api→paint→embedder→shared/constellation 后单 crate 绿,但宽域验证爆出 BAO 补丁 constellation crate 期望 per-WebView 消息变体(FocusWebView/BlurWebView/SetWebViewThrottled 等)——**vendor 的 shared/constellation 本就超前于基线,基线同步=降级**。死线触发,全部回退至 HEAD(paint 回到预存断裂态,归本闭合波)。 | 实验集:paint/src 7 + shared/paint 3 + shared/embedder 7 + shared/constellation 5 + id.rs 增量 LCPCandidateID | 已回退;结论:**闭合集 ≥6 crate 且含 2 个 BAO 补丁 crate(paint 消费链+constellation),零散文件同步不可行,必须协调波**(同步集+补丁重放一体) |

### paint 闭合波合同要素(给执行 E)
- 同步集:paint、shared/paint(除 rendering_context.rs WGL 补丁)、shared/embedder、shared/constellation、shared/base/id.rs(增量 LCPCandidateID)
- 重放集:components/constellation(per-instance RouterProxy 补丁,其消费的 per-WebView 消息族 FocusWebView/BlurWebView/SetWebViewThrottled/GetInternalAncestorOriginObjectsList/SetThrottledComplete/SetDocumentState 需与基线 shared/constellation 对齐——基线可能已含同名变体,需 API 面比对而非盲保留)、script 侧消费(PaintTiming* 生产链)
- vendor 残岛:shared/paint/largest_contentful_paint_candidate.rs(上游 baseline 与 origin/main 均无此文件,vendor 独有迭代残岛,有消费者;迁基线面后删除)

## 裁决(按桶)

### ABSORB(向基线同步;已完成或随缺口闭合波)
- webvtt 系列 5 颗:cfa8441bd(GetCueAsHTML)/78307842f(activeCues)/6bd06116e(cue order)/8dd2551cb(track URL)/8d51181ba(parsing tests)→ **已随家族同步完成**(8dd2551cb 的 htmltrackelement.rs 已含)。
- fonts 三颗中两颗(b724f6f80/b9ed51223)已同步;694133bbc(lazy metrics)**defer**:触 BAO 补丁 canvas_state.rs(R53-A)+layout 文件,需重放。
- 79eb20c15(paint 移除 closed WebView pipelines)→ 随 paint crate 整体同步(在联合 check 内验证)。

### DEFER-重放波(触 BAO 补丁文件/结构性耦合;单独 E 合同)
- **GC 根安全化系列 ~15 颗**(RootedPromise/TracedCallback/MaybeUnreflectedDom/safe conversions/stream callbacks/TrustedScript/useRcPromise 移除):高价值(正对 opt-only SIGSEGV GC 根治历史)+高碰撞(SM153 消费表 script_thread/codegen/principals/structuredclone 全在射程)。
- 9d381ca6c(Sec-Fetch-* 用 current URL):http_loader.rs 重补丁(C19 S2b)。
- 7256c05b1(cookie domain 匹配降 alloc):shared/net/lib.rs BAO 补丁 + 新 fn 在补丁文件内。
- media-audio 两颗(AudioParam automation/clamping):audio/*.rs 是 C15 补丁区。
- 346d5a9b3(constellation WebViewState):constellation 是 per-instance RouterProxy 补丁区,结构性。
- 48123c17b(devtools Evaluate primitive throw args):script crate,依赖未同步邻居,随缺口闭合波。

### 已有裁决维持(09-19 波 deferred 清单)
- ae51d9785/39fd4909c/31f660d20(paint timing 三颗)维持 deferred——但注意 paint 消费侧已在树,撕裂已由本轮闭合生产侧;PaintTimingMixin 大件本体仍 deferred。

### N-A-HOST(上游基础设施,不吸收)
- android 7 颗 + Revert Android 1 颗;build 14;ci/cargo/tidy/bootstrap/etc;servoshell/libservo/webdriver 打包面;WPT meta/tests 同步(sync 类,tests 政策外);deps 2 颗(下次 lock 刷新时重估)。

### 随缺口闭合波(script 71 主体 + script_bindings 系列)
- 纯上游文件(零 BAO 锚)→ 文件级向基线同步;BAO 锚文件 → 逐文件重放(以 CLAUDE.md servo 表为图)。
- ca37ad20f(SVG a/pattern/text/tspan DOM 元素):**等 E1 落地后**(dom/svg/ 是 E1 在途域)。
- e814d42b5(IDBIndex openCursor/openKeyCursor):idbindex.rs 锚检查后同步(idbtransaction.rs 是 BCE-20260910-004b 补丁,注意邻接)。

## 防复发(daily-ops 流程缺陷)
吸收波 bump 基线前必须证明窗口全判定(bump = 收口声明);验收 check 的 crate 集必须覆盖被触碰 crate 的闭包(夜间波 8 crate 漏 paint 即本案)。

## 基线诚实性
143 颗全处置完成前,upstream-baseline.json 的 servo notes 必须显式记录「b820a9679..7ca99fe3f 缺口 triage 在案(本文件)」;全处置后维持 7ca99fe3f。

## 闭合波①执行口径(2026-09-27,E3 枚举后主会话裁决)

全窗扫描 36 颗真候选,四桶:
- **桶 A(12 颗,直接重放)**+**桶 B(12 颗,先补 callback.rs ~50 行 pre-window delta:RootedCallback/TracedCallback)= 本波(口径 2 批准)**;2b71c3cc1 codegen 语义面按 SM153 表纪律逐处锚。
- **桶 C(~20 颗,promise 重构前置:RootedPromise/TracedPromise+87 文件 Rc<Promise> 迁移)= 单独立项另波**(体量风险自成一波,枚举分析留作合同基础)。
- **桶 D(3 颗 N/A 确认)**:hyper-rustls bump/activeCues(WebVTT 波已吸)/IDBIndex openCursor(特性候选非安全系列,记 backlog)。

## 闭合波①战果(2026-09-28 落地,commit 95986e4e)
- 落地:桶 A 7 颗 + 基础设施 2 件(callback.rs RootedCallback/TracedCallback pre-window delta + codegen fused getter no_gc 支持)。四门绿+主会话独立 V 3/3。
- 死线停(如实):桶 B 13 颗——批量应用 1234 错爆炸;63/77 文件相对窗口基线漂移;与桶 C 前置纠缠(3596f53ad 依赖 resolve_or_wrap_promise/rooted_heap_handle)。回退一致绿态验证。
- 重分类:**桶 B → script 协调大波**(与 script-71 主体合一,排在桶 C 之后);3596f53ad 并入桶 C;b4b6adf49 fork 已含等价(N/A)。
- **队列终序**:paint 协调波(E1 在跑)→ 桶 C promise 重构波 → script 协调大波(71+B)→ task#8。

## 闭合波②停报改判(2026-09-28,E1 侦察后主会话裁决)

**事实反转**:paint 链当前 RC=0(撕裂从未入库——系主会话在途实验态被误判为 HEAD 断裂)。真结构=**双向互斥实现**:vendor 持 fork 独有 LCP 迭代岛(largest_contentful_paint_calculator+孤儿模块+display_list 岛字段),基线持上游 PaintTiming 面(PaintTimingReport/Info+performance/LCP)——基线没有 vendor 的岛,vendor 没有基线的面。

**改判**:paint 闭合波 → **岛→基线迁移波**(特性级:删岛两模块+采基线面,跨 paint/shared-paint/layout(3-4 文件)/script(performance+document+window+2 webidl))。蓝图六点(E1 停报)采纳为合同基础。**队列终位**:script 协调大波之后(需 script 域安静)。shared/constellation+constellation 维持 vendor(per-WebView 消息族是 RouterProxy 补丁消费面,基线无此族——API 比对结论=不同步,已实证)。completion②③ 待 E3 检查点补取证。

## 闭合波②可验部分收口(2026-09-28 00:43 窗口取证)
completion 四项齐备:①链检查 RC=0(断裂=在途实验态误判,已澄清);②servo 聚合 RC=0(script+layout+paint+constellation 全链);③回归双绿(CSS 门 73/73+SVG 13/13);④BCE 根因陈述(双向互斥实现)。**迁移本体**(岛→基线 PaintTiming)按六点蓝图排队 script 协调大波后。

## 预起草:script 协调大波合同(排队 promise③ 之后;E3 枚举+E1 波②侦察为基)
前置双地基:①promise 重构(promise③ 在跑,7 文件同步集+87 文件迁移);②codegen callbackUsesRc/useRc/needTraced 决策端口(configuration.py+codegen.py 三站点,涟漪 107 处/22 文件 Option<Rc<XxxCallback>>)。
主体:25 颗(桶 B 13 callback 转换+桶 C 直接相关 ~12:17b27476c/baa669709/8ac219068/9c9d02c2e/4bbb8ce78/5f1362851/6b03bc4e7/d6e83757a/531762343/b5a1f5e6e/2d63455e7/261725c76+3596f53ad 并入)+script-71 主体(纯上游零锚文件级同步+BAO 锚文件按 CLAUDE.md 表重放;含 ca37ad20f SVG 四元素——E1 已落地解锁)。
分段:地基②先行(独立绿检查点)→25 颗按文件族分段→script-71 主体殿后。双门口径+回归三件套+113 级死线(按波实际面×1.3)。

## 预起草:paint 岛→基线迁移波合同(队列终位;E1 六点蓝图)
打包集=vendor 4 crate+script 3 面一次 PR:①直取基线 paint/{lib,painter,pipeline_details}.rs+shared/paint/{lib,display_list,viewport_description}.rs;②删双孤儿(largest_contentful_paint_candidate.rs+largest_contentful_paint_calculator.rs);③layout 同步 display_list/{mod,paint_timing_handler}.rs+layout_impl.rs(先 diff 定界);④script 同步 dom/performance/*+document.rs+window.rs+webidls×2(**须 script 协调大波后**);⑤shared/embedder 仅 3 文件 delta 待查性质;⑥shared/constellation+constellation 保留 vendor(per-WebView 消息族=RouterProxy 消费面,实证不同步);⑦servo/tests/largest_contentful_paint.rs 随面。
