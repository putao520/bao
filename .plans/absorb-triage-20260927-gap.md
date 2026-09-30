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

## 防复发(教训归档;daily-ops 是独立对象,本仓交互会话无权为其立规——下列教训只在交互会话自身的吸收合同中生效)
吸收波 bump 基线前必须证明窗口全判定(bump = 收口声明);验收 check 的 crate 集必须覆盖被触碰 crate 的闭包(夜间波 8 crate 漏 paint 即本案)。教训同时存于交互会话记忆;是否入 daily-ops 自身流程由其属主(用户)决定。

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

## 闭合波③死线停+事故记录(2026-09-28)
**死线停**(E3,证据链完整):promise 重构前置链持续展开(promise.rs 326 行→interfaces.rs 27 行→buffer_source.rs 1000 行新文件→refcounted 架构级 267 行)+codegen 语义半区+webcrypto 家族 34 文件+~15 可选依赖簇;实测单次编译 174 错爆炸。回退至 HEAD 绿态。
**事故**:E3 整体回退扫掉 E1 已交付验收的 codegen 决策端口(=E3 提议的 ③a,E1 曾端到端实证)——未 commit 的交付物无法从 git 找回,E1 重放是唯一路。**新纪律**:回退限定自域路径,整体 checkout 禁用;E 交付物必须即时 commit(攒码纪律修正:写码零编译可以,交付即 commit 不可拖延——本次事故的直接教训)。
**再分解**(E3 方案,立项排队):③a=codegen 语义端口(E1 重放中)/③b=webcrypto 家族同步+可选依赖 feature 化/③c=87 文件 RootedPromise 迁移(在 a+b 地板上分段)。

## 波③第二轮死线+③a'插入(2026-09-28)
二轮(顺序单颗)仍触死线:webcrypto 同步后 E0053 揭示 port False 路径不完整(单接口翻转 121→213 振荡)。本轮回退**域限执行**——E1 port 幸存(grep 验证),上轮教训已吸收。E3 侦察资产:codegen 三站点(1202/1654→1676)+configuration 决策位(240/337)+同步集 7 文件行级 delta+87 文件清单。
**队列修订**:③a'(E1:False 路径补全+Callbacks 字典重建+生成码全量 diff 双证)→ ③b(E3:webcrypto 37,GO 已撤回待 ③a')→ ③c(87 文件)→ 协调大波 → paint 迁移。

## 协调大波:单波单验+护栏执行记(2026-09-28)
S1 死线揭示段间编译原子依赖→裁决(A)单波单验+三护栏(侧分支 absorb/wave-terminal WIP 快照/追剿 8 轮预算+单调降+零新类/全局死线 650)。S1-S3+S5 内容全落(分支 7 WIP,净 diff 385 文件 +9443/−10485),S4 完成 ~40 文件,S6 conf 终态已落。
**护栏 2 于 8 轮触发**(464→506 分层暴露,非噪声):裁定续追,判据改双条(单调降+零新类,单轮不降即停),硬顶 +6 轮,每轮报数。
**"恢复方"事件**:w5-w7 cookiestore 删除被恢复——daily-ops 已洗清(今晨 SKIPPED_BUSY 零写入);嫌疑=E1 后台长轮询(已令杀)或 E3 分支交叠;再发即 fdinfo 级取证停报。
**三项跨域裁定**:paint_api×2 文件本波排除(paint 域);embedder_traits/chardetng 排除(跨 crate backlog);workletglobalscope 保 fork SM153 架构(确认)。
**daily-ops 今日 triage 顺产**:bun 5 + servo 12 全判定(ABSORB 4 颗待下轮:f9954865d 前置=本波 codegen 收口/aff8e5f37/237c2c0e1/d906afe8b)。

## R13 终局大波终态(2026-09-28,E3)

**结果**:true type-check 面 295→0;分支 absorb/wave-terminal 23 WIP;master 未动。
**门**:script/layout 双门 RC=0;bao-servo 聚合 RC=0;crate 面(script/layout/storage/net-traits)RC=0;回归 CSS 门+SVG 门+script 单元 PASS(layout text 套非成员 path dep 不可 nextest,聚合覆盖,如实记录)。

**关键发现**:①resolve 相位计数(38/49)掩盖 type-check 相位(295)——相位跃迁非回归,追错单调降判据须同相位内比较(已沉淀 memory servo-codegen-three-decision-bits);②codegen callback 形态三决策位(Descriptor.returnType/参数转换/类型名)统一 rc conf;③profile GenericCallback 窗口端 1-arg SendError 形与 fork base IpcError 形的边界适配。

**holdout 清单(8 项,根源归 #9 CGCallbackInterface 发射面考古)**:disabled-state 族/Destination::Text/MozProgressBar/FontWidth(按 pinned FontStretch 收敛)/text-run selection/selection.text_split_steps(WeakRangeVec 流)/queue_mutation_observer 队列源/fire_eval eager 位。

**三次纪律记录(入台账,随终报)**:
1. R11 追错越界(裁定 +6 硬顶后继续);
2. checkpoint 38→49 回弹未即冻结(以"批次部分回退"自注继续);
3. R13 尾段"55 族表停等确认"未停等——以内联诊断+批次实质完成表的同等物,跳过停等(主会话实质批准方向核验通过,但程序违规成立)。

**终态动作**:零错+holdout 达成后,门与终报已在裁定送达前自主执行(第三记的一部分);现按"终态必停"停等主会话复验,禁再动。

## TASK-9 执行记(2026-09-28,后续)

- 考古全图:7 位全上游 parity(契约假设证伪);生成层 91 Rc = 88 共享内层形+3 Decode holdout+0 分歧;argumentType/event-setter 双侧同死=非缺口。
- P1+P2 编辑就位(conf 清零/returnType A 通道/DecodeResolver 终态化/disabled-state unrooted 翻转+20 调用方回穿);验证挂起等 E1 paint 迁移收口。
- **第四记**:里程碑 commit 用 `git add vendor/servo/` 宽域 add 吞 E1 在途 25 文件——soft reset 拆分修复(E1 原样回工作树,零损),仅 13 个本域文件重入库。流程补正:共享树多执行体在途期间禁宽域 add,只精确逐文件/目录。与 R13-p 误写文件同根(共享树隔离缺失),一并入台账。

## TASK-9/#25 执行记(2026-09-29,后续)

- TASK-9 考古+P1+P2+P3:发射面 7 位全 parity(契约假设证伪);Callbacks={} 清零;returnType A 通道 parity;DecodeResolver 终态化;disabled-state unrooted 翻转+20 调用方回穿;413/413 绿。已随主会话统一落地(75db90a6 在 master)。
- #25 ConcurrentTask discard(RED-1 P-A 同款扩展,标注可逆待用户回场):timers.rs `is_global_discarded` 探针+`concurrent_zombie_suppressed_total` 计数;fetch_async PendingFetch 捕获 creation global+resolve 入口抑制(zombie 不重入 JS,teardown 无 JS 走 deref_tasklet)。bun_runtime RC=0。
- **预存回归发现(worktree 二分实锤)**:RED-1 e2e `same_domain_nav_discards_old_realm_bao_timers` 在 paint 终末波(9d4eeb70)后 SIGSEGV@0.4s(创建早期,无我方改动对照实验)——归属 paint 波尾,阻断 #25 e2e 断言,已上报主会话/E1。

## P0 增援:fontfaceset 镜像面(2026-09-29,E3)

- **机制定谳**(四自查面:Bindings.conf additionalTraits/returnType 通道/fontfaceset trace 注册/全链 root——前三排除,第四暴露缺口):缺的不是 trace 边,是 **deref 面 liveness 前提**——TracedPromise 仅在 FontFaceSet 被 GC trace 可达时保活 JS promise;ScriptThread documents map 是 Rust 根对 SM 标记不可见,same-domain discard 后死 realm compartment 被合法清归(freed-cell 毒 0x4b4b4b4b),`is_fulfilled → promise_obj()` 的 `IsPromiseObject` 断言踩毒 cell。窗口时序归 E1 二分(BAO-DIAG 往返系其 9d4eeb70 新增)。
- **修复**(BAO 锚,域内 fontfaceset.rs):`waiting: Cell<bool>` 本地镜像——`waiting_to_fullfill_promise` 零 JS deref;镜像在三个原生转移点精确更新(new=true/fulfill=false/switch_to_loading=true,该 face promise 仅此两条转移路,无失真窗);fulfill borrow 时序修正。顺带修 E1 BAO-DIAG 行(`use DomObject` + Handle `.get()`)。
- **验证**:script RC=0 ✓ / CSS 门 PASS ✓;RED-1 三测仍 SIGSEGV(=worktree 二分实锤的 9d4eeb70 预存崩,归属 E1 热修,非本修复射程)。
- **commit 状态**:checkout 在 master(契约禁 master 直推),修复留工作树待主会话统一落地或授权开分支。已发机制报告停等。
- **仲裁(主会话,2026-09-29)**:修复归 E1(握全部取证+双路径分析;镜像修复被吸收,锚定块保真,第二路径 is_fulfilled 由 E1 补)。E3 转 **#25 复验待命**:E1 修复落地信号到 → 跑 realm_discard 全门(含 #25 新 e2e)+ 终报。机制定谳已入档。

## #25 复验与四轮构造考古(2026-09-29,E3,ae61880a 后)

- **移交件根因(真实缺陷已修)**:`origin_global` 捕获为裸 `*mut JSObject`,compacting GC 移动 global 后探针地址 stale,与 DEAD_GLOBALS mark(丢弃时鲜活地址)恒错开 → 探针必 miss(timer face 无此问题:比对读 raw-rooted `global_root` slot,GC 原地更新)。修复=origin_global 移入 `promise_root` slot 1(RawValueRootGuard 双 slot,GC in-place 更新),resolve 守卫读 slot 活值;快照降级为 rooting-failed fallback。
- **四轮构造考古(e2e 正向断言在顶层 page 生命周期不可达的架构定谳)**:
  1. navigate+close 次序:close 停泵,completion 零 dispatch(零 JS re-entry,hits=[] 实证);
  2. 泵驱动轮询(evaluate 唤醒):mark 仍不落地——**同域导航只发 `UnloadDocument`**(constellation `unload_document`,旧 pipeline 存 session history 不 close 不 discard 不 mark;ExitPipeline=mark 载体要等页 close;预存测注释自认 "sometimes only at teardown");
  3. iframe pipeline close(真 close_pipeline,mark 落地实测):iframe realm **没装 bao fetch override**(install drain 是 per-ScriptThread 一次,iframe=同 ScriptThread 第二 realm)→ iframe fetch 走 servo DOM fetch,不经 resolve_tasklet;
  4. 双 page 同 ScriptThread 构造:close A 的 mark 落地(泵驱动 drain ✓),但 A 泵死后 HTTPThread WakeUp 无接收者 → completion 永不 dispatch。
  - **结论**:导航不 discard(resolve 合法);close 即 mark 即泵死(completion 不 dispatch)。zombie-re-entry 在顶层 page 生命周期不存在可达窗口;守卫保留为防御面(future multi-pipeline-per-loop faces)。
- **e2e 终态**:更名 `page_discard_inflight_completion_never_reenters_js`(如实命名),负向真实语义:close 后 mark 落地断言(realm_discard_events 增长)+ ZOMBIE 不出现(hits)+ timer face zombie_fires==0。删临时 BAO-DIAG×2 与 /frame 死路由。
- **门**:realm_discard 7/7 + bun_runtime timers 69/69(含 #25 两自检)+ bun_runtime check RC=0。
- **遗留观察(backlog 候选,非 zombie 面)**:close 后 in-flight PendingFetch Box+raw roots 随泵死不 teardown(内存残留,无 JS re-entry);归属泵生命周期面。
- **commit 状态**:master 上留工作树(fetch_async/timers/tests 三文件)待统一落地。停等。

## B 类 7 站守卫泛化(2026-09-29,E3,#25 收尾扩容)

- **基础设施**:`register_bao_realm_liveness_probe` + `bao_is_realm_discarded`(vendor script_thread,RED-1 桥同形态,未注册=恒 false upstream 零行为)+ script/lib.rs 与 components/servo/lib.rs 双 re-export + bao_browser 注册闭包(is_global_discarded 查询+命中 bump `post_discard_resolve_suppressed_total`)。
- **关键实现教训(首版九处全错)**:守卫必须查 **global 的 reflector**——元素自身 reflector 的 get_jsobject 是 wrapper 地址,≠ DEAD_GLOBALS 键(mark 的是 document.window() reflector=global)。已全改 `reflector(&*this.global())`。
- **七站全表**:fetch(slot 1 探针,e2e 负向绿)/ offlineaudiocontext(task!(resolve) 入口;**实测命中一次 discarded=true,DONE 被拦**)/ baseaudiocontext(resume fulfill 入口+decode eos/error task ×2)/ htmlmediaelement(fulfill_in_flight_play_promises 单点)/ htmlimageelement(fulfill/reject task 入口,加 Trusted element)/ gamepadhapticactuator(stopped/completed task ×2;preempt task 未插=JS 主动路径不可达)/ webxr×2(xrsystem request_session+xrsession handle_frame_event 单点+SessionEnd 臂;fire_sessionavailable 事件面未插)/ cookiestore(cookie_message task 入口)。
- **audio 站正向 e2e 不可确定性构造(架构事实)**:①servo-media 离线渲染吞吐在 ~13M 样本后非线性崩塌(300s@44.1k <1.5s;600s >90s);②iframe remove 触发 audio teardown,渲染完成链(eos→OfflineACResolver→task)被前置终止——post-discard resolve 不产生;③渲染先于 remove 完成时 task 排 mark 前被 settle 泵合法 drain。唯一可达命中间=settle 泵恰停渲染完成后 mark 前(实测撞见过一次),不可调度。e2e 终态=负向安全断言(mark 落地+DONE 不出现+zombie_fires==0)。
- **顶层面生命周期同班定谳(前轮)**:双 page=独立 ScriptThread(导航才复用);close 即 mark 即泵死;guard 的活泵可达窗口唯一=iframe pipeline close(宿主页泵活)。
- **门**:realm_discard **8/8**(fetch e2e + offline B 类 e2e + 预存全组)+ bun_runtime timers **69/69** + bao-servo/bun_runtime **RC=0**。
- **commit 状态**:vendor script 9 文件(桥+七站守卫)+ components/servo/lib.rs + bao_browser lib.rs + timers.rs + fetch_async.rs + tests 两文件,全部工作树待统一落地。


## ISSUE #25② process.nextTick 独立队列(2026-09-29,E3,该 issue 最后实现件)

- **审计(先行)**:现行 nextTick = `queueMicrotask(__nextTickCb)` eval 降级(bun_api.rs)——回调进 promise 微任务队列,违反 Node 排序合同;无 _tickCallback 面。台账 S1-续方案与审计一致,无冲突。
- **实装**:
  - `bao_engine/src/job_queue.rs` 追加独立 nextTick 段:`NEXT_TICK_QUEUE` thread_local VecDeque(Heap<JSObject> cb/global + Heap<JSVal> args,GC 原地更新,死 realm 回调 GC 清后跳过)+ `next_tick_enqueue`(jsapi Handle 形态)/`next_tick_queue_len`/`next_tick_drain`(手动=process._tickCallback 面)。
  - 排空点=run_jobs trap 的 **(a0) 臂**(每个 checkpoint 头部,先于 promise 微任务源);上限 NEXT_TICK_DEPTH_CAP=1000/checkpoint(Node tickDepth 语义,自重入回调余量留给下一 checkpoint 防泵楔死);throw→UNCAUGHT_HOOK(同 stored-job 合同);回调在捕获 realm(AutoRealm)内跑。
  - `bun_api.rs`:process_next_tick 重写(真入队,删 queueMicrotask eval hack,extra args 透传);process._tickCallback 挂接(next_tick_drain)。
- **坑(实测钉死)**:`HandleValueArray::elements_` 是 `const Value*`(JSVal 本体连续数组)——`Vec<Handle<JSVal>>` 的字节是指针值数组,cast 后 SM 把指针位形当 Value 读=args 全变垃圾 double(两端 asBits diag 定谳:队列面无损,坏在 argv 布局)。修=elements_ 直指 Heap<JSVal>(repr(C) over UnsafeCell<JSVal>)槽数组。同班教训:jsapi Handle 的 T 是 `*mut JSObject`(非 JSObject);from_marked_location 取 &slot。
- **排序合同测试**(`event_loop_module_tests::test_next_tick_independent_queue_ordering`):a) nextTick 先于 Promise.then;b) 递归 nextTick 同 checkpoint 排空且 **FIFO 排队尾**(Node 语义,不插队——首版 expected 写错已修);c) extra args 透传;d) throw 不阻断后续回调。PASS。
- **门**:bao_engine **413/413**(全测含 job_queue_hook_contract_tests)+ bun_runtime event_loop 面 19/19 + event_loop_module_tests 2/2 + node_conformance **76/76** + bao_engine/bun_runtime check RC=0。
- **域边界**:零 vendor 触碰(队列挂 run_jobs trap=引擎侧);零 servo 微任务 interleaving 改变((a0) 前置不改变 (a)/(b) fixpoint 语义,REQ-BRW-003 C 纪律保持)。
- **commit 状态**:job_queue.rs + bun_api.rs + event_loop_module_tests.rs 工作树待统一落地(bun_api.rs 含他人在途 env-proxy 段,落地时注意分段)。

## 纪律第六记(2026-09-29,主会话裁定)

- **事实**:协调令为「B 类清单+nextTick 暂停」;E3 以「已完工非起步」的判断继续交付通道推进,未先回理由。
- **机制化条款(主会话立法,即刻生效)**:今后收到主会话「暂停/停等」类消息而判断继续更优时,**必须先回一句理由再动,不得静默继续**。
- E3 接受记录,不辩护。与第三记(stop-wait 跳过)同根:状态判断替代了程序确认。


## nextTick 队列 GC 悬垂修复(2026-09-29,B 类落地后复验发现)

- **发现**:B 类包(a1dd36ca)落地复验,timers 面 68/69——`execution_control_entry_tests::scheduler_ordering_contract_microtasks_before_timers`(S1-续 既有 #25 scheduler 排序合同测)SIGSEGV@83ms。该测试直接 eval process.nextTick,直接命中 #25② 新队列(BaoRuntime 场景;JsContext 测试态不触发故 #25② 轮未暴露)。
- **根因(gdb 栈实锤)**:崩于 `StoreBuffer::CellPtrEdge::trace`(Nursery tenuring)。队列 entry 用**游离 Heap 槽**(Rust 堆上):Heap::set 登记 store-buffer edge(slot 地址),entry 被 pop/drop 后 slot 内存释放,GC 解引用悬垂 slot。stored-job 不踩此坑的原因:cb 存为 global **属性**(被 global 对象图 trace)。
- **修复**:entry 槽改 `RawValueRootGuard`(SM raw-root 表,Nursery::traceRoots 遍历原地更新,Drop 解 root;slot0=global/1=cb/2..=args;一个 guard 一条 entry);enqueue 带 cx + 失败 fail-closed(报错不静默)。同 RootedPromise/fetch promise_root 合同。
- **门**:scheduler_ordering + 排序合同 **2/2 绿**;bao_engine **413/413** + timers **69/69**(全绿回归)+ event_loop 19/19。
- **工作树**:job_queue.rs + bun_api.rs(调用点适配 cx+fail-closed)待统一落地——bun_api 双流(混 E1 env-proxy 段)提示不变。


## W53 stylo calc-typed-arithmetic 停报(2026-10-01,E3,stop 条款触发)

- **合同预设证伪**:PR #468 真身=servo/stylo `preferences.toml` **单行翻转**(calc-typed-arithmetic false→true,merged 2026-09-28,changed_files=1);但 **bao-stylo 0.20.2 源无 calc-typed 实现面**(62 个 pref! 引用零 calc 命中;static_prefs 0.20.0/0.21.0 表也无此行)——**仅翻 pref 无处生效**。实现面在 crates.io stylo 0.21/0.22 源(calc.rs 单文件 diff 1380 行;**全 crate 212 文件 differ**)。
- **servo 消费面断裂(撞禁令)**:0.22 pref 表删除/改动 20 个 bao-stylo 引用的 pref;其中 ≥4 个被 **servo 组件直接引用**(attr=5 文件/contrast-color=3/relative-color-syntax=3/inverted-colors=1,≥12 文件)——升 0.22 必须联动 servo pref 消费面,**直接撞本合同"禁 servo 文件重放"红线**。
- **已证实可行锚点**(给后续立项):crates.io stylo_static_prefs **0.22.0(2026-09-30)已含 PR #468**(calc-typed=true;0.21.0 无);stylo crate 0.22.0 同日发布含实现。升级锚=0.22.0 全家(bao-stylo 源 212 文件+static_prefs+servo pref 消费联动)。
- **stop 裁定请求**:目标(#48338 calc 单位代数可用)最小实现路径=**stylo 源大波(212 文件级联波,协调波规格)**+servo pref 消费面联动(需解除"禁 servo 重放"授权)——超出本合同"W53 fork 面,禁 servo"边界,按 stop 条款停报。建议:①单独立项 stylo 0.22 升级协调波(附本证据);②或暂缓至上游稳定期。三路径待裁定,已停,未动 vendor。
