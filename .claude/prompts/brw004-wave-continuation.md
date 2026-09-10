# BRW-004 战役终态(2026-09-10 04:22 残差清零 · 本文件转为历史档案)

**REQ-BRW-004 = implemented(SPEC 642da220),残余观察项三波全清**:
- ✅ C15 字面补全:`016747b1` 分支 A——实时 AudioContext worker 可支撑(sink 在 render 线程构造零 Window 依赖,e42 疑虑证伪),criterion 字面完整闭合,**原措辞立法问题自消**
- ✅ SW 注入饥饿:`baaf7db7`——SW scope 接 injector 层;关键副产物:**SW realm 的 JS hooks 此前任何情况都不落地**(SW 无 on_complete),本波打通
- ✅ shadow 三探针:`439f4ec1`——setImmediate BLACK→GREEN(裸根 global+AutoRealm 派发,BCE-20260910-003:realm 进入前 fire=callback 蒸发类);queueMicrotask/crypto GREEN as found
- 全日 33 commits(`1d75200c`→`439f4ec1`),含 8 个 P0 根治与 2 次证据级证伪
- 观察项(未断言,留档):worker 销毁未 close 的实时 AudioContext AudioRenderThread 驻留(上游同形态);queueMicrotask 非函数静默忽略(spec 应 TypeError);crypto plain object 类名;C19-③ REALM_PROFILES 条目级注销无公开观测面
- 以下原文为过程档案,供考古

---

# BRW-004 波次续跑计划(2026-09-09 配额中断交接 · 供 18:37 重派及后续会话/daily-ops 接力)

> 背景:用户 2026-09-09 三裁决(①破例授权 vendor patch C13-15;②servo 上游 TODO 自己完善=C19 SW fetch;③per-worker 注册小 patch)。
> 当日 17:30 API 配额阵亡 E30/E31/E32;E32 遗产(S1)已由主会话收口。本文为 W2/W3a 完整合同 + S2+ 队列 + C15 悬置。
> 状态快照:15/19 criterion live 绿(C1-C13、C16-C18);commit 链 `1d75200c`→`f77faf8b`(8 个,全 push),HEAD=f77faf8b。

## ① C15(worker Audio)——⏸ 待用户判定,禁自行实现

servo 与 Chrome 规范级均无 worker AudioContext 载体;自建非规范载体会比 Chrome 多一个可检测向量,与反指纹意图直接冲突。已建议 N/A 判定,等用户改判。**未判定前禁开工。**

## ② W2(E30 合同 · C13 补强 paint 线程噪声)——18:37 重派首选槽 1

原合同(2026-09-09 会话 L2962),含主会话前置查证修正:

- **task**: vendor canvas paint 线程接入全局噪声(JS hook 无法覆盖的 ConvertToBlob/TransferToImageBitmap/worker 2d 通路)
- **archaeology**: `set_canvas_noise_seed`→`set_global_canvas_noise`(vendor servo/lib.rs:153-154)写进程级原子,但 `get_global_canvas_noise`/`CanvasNoiseConfig` 全 vendor 零消费点(死代码)——真实读取点应接 GetImageData
- **⚠ 路径修正**(E26 侦察笔误,已实测核验):真实文件是 `vendor/servo/components/canvas/canvas_paint_thread.rs`(**不是** `script/dom/canvas/...`);注入位 = `CanvasCommand::GetImageData` handler 内 `read_pixels` 之后、`sender.send(snapshot.to_shared())` 之前(实测约 :303);该文件**当前零 Bao 定制**(将成 vendor 清单新条目)
- **scope**: canvas_paint_thread.rs(GetImageData 接噪声)+ servo/lib.rs(若 CanvasNoiseConfig 需暴露,已定制文件);禁碰 bao_stealth/hooks.rs(W1a 已收)、bao_browser/lib.rs
- **completion**: ①seed=0 时字节零 diff(硬保证),seed≠0 时确定性噪声(以 hooks.rs JS 算法 detNoise/addNoise 为规范做 Rust 等价,per-pixel deterministic)②查证 Window 侧 getImageData 实际路径(JS hook 还是 paint 线程)→双噪声风险处置:若 Window 也走 GetImageData 则 JS hook 该路径退役属 bao_stealth 域,本波只记录+升级,不代改③测试:seed=0 零 diff 断言 + 同 seed 两次同结果断言④既有 canvas/stealth 测试零回归⑤commit(注明 vendor 清单新增条目+破例裁决)+push
- **retry**: 2
- **stop**: GetImageData 调用面比预期广(screenshot/CDP 也走)需语义裁定;双噪声去重需跨 bao_stealth 大改

## ③ W3a(E31 合同 · C14 WebGL1 worker 通道)——18:37 重派槽 2

原合同(2026-09-09 会话 L2978),E26 侦察三断点:

- **task**: WorkerGlobalScope 存 webgl_chan/webview_id + WebGLRenderingContext 解 Window 锚定
- **三断点**: ①`WorkerGlobalScopeInit.webgl_chan` 从父 Window 带到 worker(worker.rs:243-247→workerglobalscope.rs:141)但 `new_inherited` 丢弃不存(struct 字段止于 font_context ~:382);②`WebGLRenderingContext::new_inherited` 吃 `&Window`(webglrenderingcontext.rs:217)只用 `window.webgl_chan()+webview_id()`(:228-236);③offscreencanvas.rs:220-223 Window downcast(WebGL2 同 :259);WebGL1 webidl 已 Exposed=(Window,Worker)
- **scope**: workerglobalscope.rs(存字段)+webglrenderingcontext.rs(签名解 Window)+offscreencanvas.rs(WebGL 路径走 GlobalScope accessor);禁碰 canvas_paint_thread(W2 在途)/lib.rs/hooks.rs
- **completion**: ①worker 内 `new OffscreenCanvas+getContext('webgl')` 非 null + 基本 getParameter live 验证②Window 侧 WebGL 零回归③W3b(offscreencanvas downcast 解除)若天然在波内一并落;patch 面超三文件+40 行则只落断点 1/2 记录升级④vendor 清单注记+commit+push
- **retry**: 2
- **stop**: new_inherited 签名重构波及>5 调用点;live 验证暴露 webgl_chan 在 worker 的 constellation 侧拒绝(上游更深缺口)
- 注:W3c(WebGL2 worker)未侦察,接 W3a 结论展开

## ④ C19 S2+ 周级链

S1 已收口(`f77faf8b`:dom_serviceworker_enabled bao 侧翻开 + ServiceWorkerGlobalScope 接 WebViewId-keyed drain)。剩余:
- **S2a/b**: SW FetchEvent + respondWith 管线(controller 匹配→FetchEvent 派发→respondWith promise)
- **S3**: net 层拦截回注(fetch 事件响应回注 net 层;SW 转发 fetch 走同一 TLS profile)
- **S4**: tracking wiring

### S2 前置侦察合同(只读,轻量,可作槽 3 与 W2/W3a 并行)

> ✅ **侦察已收口 2026-09-09(e33)**,完整报告:`.claude/prompts/brw004-s2-recon.md`。下方为侦察产出的 S2a/b 合同。

### S2a 合同:SW realm 侧 FetchEvent 管线(重 E,占强槽;写域=serviceworker/ + webidl,与 W2/W3a 互异)

- **task**: 新建 FetchEvent DOM 类型 + SW fetch 分支重写——fire 真 FetchEvent(含 respondWith)替代裸 Event
- **archaeology**(e33 侦察,行号以报告为准): serviceworkerglobalscope.rs:597-603 现状 fire 裸 `Event(atom!("fetch"))` 且恒 `response_chan.send(None)`;FetchEvent.webidl/respondWith 全树不存在;Request/Response webidl 已存在且经 Global=(Worker,ServiceWorker) 在 SW realm 可达(:599 TODO 前提 stale)
- **scope**(5 断点,侦察⑤): ①`webidls/Servo/ServiceWorkerGlobalScope.webidl` onfetch 取消注释 + 新建 `FetchEvent.webidl` ②新建 `dom/serviceworker/fetchevent.rs`(FetchEvent + respondWith promise 捕获)③`serviceworkerglobalscope.rs:597` 分支重写:fire FetchEvent,事件持 respondWith 接收器 ④`dom/serviceworker/mod.rs` 注册 ⑤Codegen绳子(webidl 注册清单)
- **completion**: ①SW 内 `self.addEventListener('fetch', e => { e instanceof FetchEvent === true; e.request instanceof Request === true; e.respondWith 为函数 })` live 验证 ②respondWith(promise) settle 后经 CustomResponse::new → `response_chan.send(Some(...))`;未调用 respondWith → send(None) 兜底 pass-through(既有行为不回归)③既有 SW/worker 测试零回归 ④vendor 定制清单注记(新文件条目)+commit(注明 user ruling 2026-09-09 vendor patch)+push
- **retry**: 2
- **stop**: promise settle 的跨任务等待与 SW event loop 语义冲突(需 async 框架裁定,记录已证事实升级);webidl codegen 注册面波及 servo 全局 CodegenList(>5 文件)

### S2b 合事:net 层拦截回注(依赖 S2a;写域=resource_thread/http_loader)

- **scope**(4 断点,侦察②③): ①`resource_thread.rs:695` CustomResponseMediator 补读路径(现只写不读)②`http_loader.rs:578-600` "invoke handle fetch" TODO 落地:ServiceWorkersMode 就绪字段(shared/net/request.rs:117/:466)驱动,SW 命中→发 mediator→等 CustomResponse ③SW 子 fetch 防环:SW realm 发起的 fetch 降级 ServiceWorkersMode=None(防自拦截死锁)④**B4 首验证点(侦察 stop 部分命中)**:async 语境 http_fetch 如何等待阻塞 ipc recv——DoneChannel vs async_runtime 包装,开工先定此机制
- **stealth 约束**(侦察⑥): SW fetch 与页面 fetch 同 resource_threads(manager create :783-796 注入)⇒ 同一 TLS profile 自动继承,**零新增接线**;S2b 禁引入绕 net/connector.rs 的旁路
- **completion**: SW 拦截 + respondWith 自定义响应端到端 live(页面 fetch 命中 SW→respondWith Response→页面收到);pass-through(SW 不 respondWith)端到端;防环(SW 内 fetch 不再触发拦截);既有 fetch/net 测试零回归;commit+push
- **retry**: 2
- **stop**: B4 机制证实为上游架构级缺口(需 constellation 层改动);net 层 async 重构波及面>5 文件

### S2 侦察遗留判定(侦察④): container.controller 全树无人赋值——文档导航 SW 化(Navigator.serviceWorker.controller)是独立缺口,**未纳入 S2/S3/S4 范围**,待用户裁决是否单独立波

## V 批判词(2026-09-09 23:19,v47,HEAD=5627cf8e)

**14/19 green,未达 implemented**。报告:`.claude/prompts/brw004-v-batch-report.md`。全族 1793/1795 绿+stealth 1685/1685+runtime 19/19;零绿→红回归。三定案:(a)e37 双修复终态成立(fingerprint_eval GREEN/chaos GREEN)(b)fetchevent 引入即红(SW verdict 出口:sync XHR panic xmlhttprequest.rs:1640 + SW async fetch 不 settle)(c)**R2 realworld_anti_scraping_e2e SIGSEGV=test-ci 档 3/3 确定性,dev 同码 PASS=opt 独显 UB,P0,归因未定案**(无本波前 test-ci 基线)。

**SPEC 尺红项**:C2/C6/C8(页→Worker postMessage/结构化克隆/importScripts+crypto+performance+location 零 live 断言——历史存量);C19 三子句(TLS/H2 等价/CDP 可观测/跨页继承+terminate 注销)零覆盖。

**终局双波**(2026-09-10 01:35 派):✅ e48 R2 已根治(d2db4c55:三月老 GC UAF,JS_AddExtraGCRootsTracer;test-ci 3/3 转绿;opt 符号化档 test-ci-dbg 永久留用)。✅ e44 R1 部分收口(fe6298e1:sync XHR fail-closed 根除 panic+测试 URL bug;R1 假设证伪,真凶升级=tokio spawn 后零 poll × bun HTTPThread 无 park 忙转)。▶ e51(升级工单:bun_threading/bun_http+async_runtime 饿死根治;fetchevent 3/3 为验收)。▶ e52(C19-② CDP Network tap:cdp_handler+delegate+net tap;sub2 转绿为验收)。**二者落地 → 终局 V 复验(fetchevent+sub2+全族)→ SPEC implemented 收口**。

## 波末 V 批要求(e45/e46 落地后,稳定窗执行)

> ✅ 已执行,见上节判词。

## 引擎级工单(排队,下一空强槽):页面 realm async fetch 永不 settle

> ✅ **已收口 09cabe17**(e44):根因=ScriptThread 上的 bao MiniEventLoop 零 pumper(e39「FetchThread 黑洞」系传输层误读)+ timer natives 遮蔽 + 相对 URL 无 base。修=embedder pump 桥+skip-if-exists+baseURI 解析。三轴 A 绿/B 黑→绿/T 黑→绿/C 本就绿。

e39 基线实证(scoped revert,双缺陷):①页面 realm 纯 async `fetch()`(无 SW)本树永不 settle(probe 15s 超时;FetchThread 停在 recv、tokio 全 park,StartFetch 消息黑洞)②SW realm fetch/sync XHR 静默不出网(e34/e38 先后独立观察)。三者指向 vendor net FetchThread/IPC 路由域(shared/net/lib.rs 系 Bao 在册 patch 文件)。**注意交叉证据**:fetch 过滤测试 15/15 绿(bao_runtime 路径)vs 页面路径黑洞——先分路径(bao_runtime fetch / servo 页面 fetch / SW fetch 三轴)再归因;e40 曾把 fingerprint_website_eval_e2e 崩溃归因 e39 在途 diag 打点,此工单须复核该归因(可能即本缺陷)。

## 用户三裁决(2026-09-09 晚,AskUserQuestion 实答)

1. **C15=坚持实现**:vendor 自建 worker AudioContext+噪声(推翻 N/A 建议;侦察须核实 Chrome worker 侧 OfflineAudioContext 暴露态——若 Chrome 有,则为对齐而非非规范载体)
2. **canvas=退役 JS canvas 段**:W1a JS hook canvas 四段退役,paint 层(W2 6bcf30af)单咽喉
3. **SW controller=立波排队**:独立小波排 S2b/S4 之后

## 战况快照(2026-09-09 21:45 更新)

- ✅ 今日 17 commits:`1d75200c`波0 →`6b3caa34`per-worker →`4f59b1ac`W1a →`5b5876b2`getter →`4b394420`W1b →`72722b95`authorship →`f77faf8b`S1 →`6bcf30af`W2 →`fa53c69a`清单 →`18e6f876`W3a →`57ccec60`S2a →`fc726515`SW 臂 →`9f5f4692`GC-UAF P0 →`98532c0c`W3b(C14 全绿)→`ae306baa`双注入 P0 →`c833f77c`canvas 退役(裁决2)→`eff5781d`S2b
- ▶ 在途:e37(AutoRealm 同族残留 P0)、e43(C15 实现+第二 drain 点授权扩面)、e34(SW promise SIGSEGV,C19 最后断点)
- ⏳ 队列:引擎 fetch 黑洞工单(上节)、S4(tracking wiring)、controller 波(裁决3)
- criterion:C1-C14/C16-C18 绿;C13 跨通路逐字节一致;C15 实现在途;C19 端到端只差 SW promise 崩溃修复

- ✅ 已落地:W2 `6bcf30af`(C13 paint 噪声)、W3a `18e6f876`(C14 WebGL1)、S1 `f77faf8b`、P0-抉择 `6b3caa34`、vendor 清单 `fa53c69a`(17 条,e35 树内另有 23 条版待随 W3b 落地)
- ▶ 在途:e34(S2a FetchEvent,五断点文件已落+专属测试)、e37(P0 worker realm SIGSEGV 根治——dom_webgl2 flip 暴露的 landed 代码潜伏 UAF,crash 栈/rooting 无罪/复现材料 /tmp/nt_env.json+/tmp/repro_dev.py,红面收敛 c16/c17)
- ⏸ 就绪待落地:**W3b(e35 树内,实现 100%+live 全绿)**——等 e37 修复后唤醒 e35 精确 commit(webgl2 worker 通道+shader precision 锚+6 webidl Exposed+pref flip;334/334 届时应全绿)。e35 文件清单:4 vendor webgl/canvas .rs+6 webidl+bao_browser/lib.rs+stealth_offscreencanvas_tests.rs §5+CLAUDE.md+node_timers_module.rs(后者归 e37 接管)
- 队列序:e37 落地 → 唤醒 e35 commit W3b → lib.rs 释放 → 派双注入 P0(下节⑤)→ e34 收口 → S2b → S3/S4
- 用户裁决悬置:C15(N/A?)/canvas JS 段退役/controller 立波(三问已问三次 60s 超时)

## ⑤ P0 根治:stealth 双重注入自指死环(W3b commit 释放 lib.rs 后派)

- **archaeology**(e36 取证,报告 `.claude/prompts/brw004-getparameter-evidence.md`): 双注入 `page_pool.rs:87 inject_all` + `lib.rs:289 inject_all_with_profile` → 第二遍 `install_webgl_override` 把首遍 JS hook 存进 `__originalGetParameter__`(物证:槽位源码文本与 proto.getParameter 全同,正常应为 `[native code]`)→ 自指死环 → 未拦参数(0x1F02/0x8B8C 及**一切非拦截枚举**)到不了 servo → `engine_props.rs:970 Err(_) => UndefinedValue()` 吞成字面 undefined+NO_ERROR。worker 零包装对照返回正确 "WebGL 1.0"/"WebGL GLSL ES 1.0"(vendor 无罪,upstream issue 不立)。附带:注入无条件(profile=None 也装 stealth 链);e31 的「upstream quirk」注释(18e6f876 内)系误判需订正;C14 用 0x1F01 探活恰好绕开破损路径。
- **scope**: `bao_browser/src/lib.rs`(:289 注入点去重,全仓单真源)、`bao_browser/src/page_pool.rs`(:87)、`bao_stealth/src/engine_props.rs`(:970 吞点 + 同类 `Err→Undefined` 横扫)、`bao_stealth/src/hooks.rs`(install_* 幂等护栏,若需)、`stealth_offscreencanvas_tests.rs`(订正错误注释 + 新增回归)
- **completion**: ①Window realm getParameter(0x1F02/0x8B8C) 返回 servo 真值(与 worker 零包装一致)②拦截枚举仍回 profile 值③profile=None 不装 stealth 链④grep 全仓 inject_all 调用点=1⑤新增回归(死环不存在:未拦参数透传真值)+既有 stealth/webgl 全绿⑥BCE 归因引 e36 报告 commit+push
- **retry**: 2
- **stop**: 删双注入后有测试依赖双注入行为(记录依赖面升级);单注入点选择波及 SW/worker 注入语义(需设计裁定)

## 派发纪律提醒

- 三槽模型:W2/W3a 双 E 并行(写域互异)+ 主会话看门狗;合同照抄上文 TASK-HEADER 形态(scope/completion/retry/stop 齐全)
- W2/W3a 均为 vendor 破例 patch,commit message 须注明「user ruling 2026-09-09 vendor patch」+ vendor 定制清单条目
- C15 未判定前禁碰 worker Audio
