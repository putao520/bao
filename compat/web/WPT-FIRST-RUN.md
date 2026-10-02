# WPT First Run — Bao (subset, #14-C)

## 结果总表(2026-10-02 final · test-ci 档 + 载具三面)

- date: 2026-10-02 · driver: bao browser (headless CDP) + python ws,**每文件独立浏览器实例 + rAF 就绪门 + Shape A 输入合成**(`/tmp/wpt_first_run.py`)
- suite root: upstream tests/wpt/tests (`dom/` subset), static http server (127.0.0.1)
- binary: /var/cargo-builds/3c/6184ceb77072ba/test-ci/bao(mtime 2026-10-02 16:52,bao farm 产物,钉当前 master:W55/W54/absorb 波全含)
- manifest: 40 files · **PASS: 37** · FAIL: **0** · NO-HARVEST: 3(crash 型/子框架型)
- subtest 总量(已 harvest 37 文件):**PASS 1535 / FAIL 0**
- 3 NO-HARVEST 均为 crash 型/子框架型(inactive-document-crash ×2 / subframe incumbent-global),非时点问题,单独立项定位。

### 2026-10-01 → 2026-10-02 差异对照(7 文件翻转)

| test file | 10-01(debug 档,旧载具) | 10-02(test-ci 档,载具三面) | 翻转归因 |
|---|---|---|---|
| dom/nodes/Document-createElement-namespace.html | FAIL 41/10 | **OK 51/51** | 引擎修复(10-02 absorb 波,adoptNode 系) |
| dom/events/webkit-animation-iteration-event.html | FAIL 8/5 | **OK 13/13** | 引擎修复(absorb 波) |
| dom/events/Event-subclasses-constructors.html | FAIL 42/7 | **OK 49/49** | 引擎修复(absorb 波) |
| dom/traversal/NodeIterator-removal.html | FAIL 0/25 | **OK 25/25** | 引擎修复(absorb 波) |
| dom/lists/DOMTokenList-coverage-for-attributes.html | FAIL 172/3 | **OK 175/175** | 引擎修复(absorb 波) |
| dom/events/Event-dispatch-redispatch.html | FAIL 2/2 | **OK 4/4** | 载具根治(Shape A 输入合成 + rAF 就绪门;引擎语义面实证全绿,详见下节②) |
| dom/nodes/NodeList-static-length-getter-tampered-2.html | TIMEOUT 1/0 | **OK 1/1** | 载具根治(test-ci 档 8.3s < testharness 60s long-timeout,详见下节①) |
| dom/events/EventTarget-dispatchEvent-returnvalue.html | OK 766/0 | OK 2/2 | 10-01 数字系收割竞态伪影(与前行 NodeIterator 766 同值、manifest 相邻——单浏览器顺序导航下首探针读到上一页 realm 的 `__wpt_results__`;文件真实子测试数=2。每文件独立浏览器实例后该竞态结构性消失) |

3 NO-HARVEST 两轮一致(非本波范围)。

## 历史基线(2026-10-01 debug 档 final run,数字保留作对照)

- driver: bao browser (headless CDP) + python ws(单浏览器顺序导航,无载具三面)
- binary: debug 档 mtime 2026-10-01 05:33(含 W55 vendor realm 入口注入 patch + e4 `layout_flexbox_balance` Preferences 字段)
- manifest: 40 files · PASS: 30 · FAIL: 7 · NO-HARVEST: 3
- **载体验收(REQ-CDP-004,W55 closure)**:realm 入口注入生效——`Page.addScriptToEvaluateOnNewDocument` 的注入时点修到 CDP 规范位(`ScriptThread::load` 的 ServoParser 启动块之前:新 document 建成后、任何页面脚本写入前),同步完成型 testharness 文件全部可 harvest。前基线 0/40 NO-HARVEST → 30/40 PASS,NO-HARVEST 40 → 3。
- 完整旧总表见 git 历史(本次改写以 10-02 表为现行;上表差异对照已承载全部翻转行)。

## 载具波定性(2026-10-02,收割窗复跑 + testdriver 空白件)

driver 已参数化(`/tmp/wpt_first_run.py`:env `WPT_HARVEST_S` / `WPT_BAO` / `WPT_HTTP_PORT` / `WPT_CDP_PORT`,缺省值与原行为逐字节一致;三态判定逻辑零改动)。

### ① NodeList-static-length-getter-tampered-2 收割复跑 — 约束重归因,已闭环

实证 probe(debug 二进制 10-01 18:52,收割窗 300s):

| 文件 | 结果 | 耗时 |
|---|---|---|
| dom/nodes/Element-hasAttribute.html | PASS 2/2 | 0.5s |
| dom/nodes/NodeList-static-length-getter-tampered-2.html | 子测试 1/0 PASS,harness status **TIMEOUT** | **70.7s** |

- **约束重归因(修正 e10 "driver 20s 收割窗"归因)**:引擎本体 70.7s 完成(子测试 1/0 已 harvest),超的是 **testharness 自身 long-timeout 60s**(`<meta name=timeout content=long>`)——超时定时器在主线程被同步测试阻塞期间无法触发,测试函数返回后补触发,harness 以 status=TIMEOUT 收官(携带已 PASS 的子测试,即观察到的 "TIMEOUT + 1/0" 形态)。**driver 收割窗从来不是约束**(阻塞的 Runtime.evaluate 使窗口自然伸长,probe 70.7s 照常收割)——**收割窗延长无法修复,唯一诚实解 = 更快的二进制**。
- 工作量本质:`indexOfNodeList` = 50 调用 × 5 万×100 = **2.5 亿次 live NodeList 索引**(每次经引擎 binding getter 进 Rust DOM),debug 档 ~70.7s,opt-level 2 预期 <15s → 稳落 60s 内。
- **现有 test-ci 二进制不可用**:9-30 02:21 构建,早于 W55 realm-entry 注入(0e172001,10-01 05:58)与 W54 layout_flexbox codegen 修复——其上同步完成型文件 NO-HARVEST(0/40 前基线已实证该形态)。
- **[已闭环,2026-10-02]**:主会话经 bao farm 重发 test-ci 档二进制(mtime 10-02 16:52,钉当时 master)→ 全量复跑 **NodeList-tampered-2 TIMEOUT→OK 1/1 [8.3s]**(debug 70.7s → test-ci 8.3s,~8.5x)。连带收获:absorb 波修掉 5 个 FAIL 文件引擎缺口(见总表差异对照)。

### ② Event-dispatch-redispatch testdriver-vendor 空白件 — 载具限定性:已获批实施,根治

机制(source 级核实):

- `resources/testdriver-vendor.js` = 单行空白注释(上游 vendored stub,非 bao 缺陷)。
- testdriver.js 外层 `click(element)`:scrollIntoView → paint-tree 检查 → `getClientRects` 取中心坐标 → **直接调 `window.test_driver_internal.click(element, {x, y})`**,无 postMessage / wptrunner 依赖。
- 空白 vendor 下 `in_automation:false` 的缺省 internal click = `new Promise(resolve => element.addEventListener("click", resolve))`——**等待该元素收到任意 click,永不 reject**。
- ⇒ 挂死的 2 条(`test_mouseup_redispatching` / `test_redispatching_of_dispatching_event`,后者仅在前者 done() 后闭合)是**纯输入合成缺失**:按钮从未被点击,promise 恒 pending,harness 60s 超时判 FAIL("Test timed out")。引擎语义面 2/2(contentLoaded redispatch + trivial)已过,与 2026-10-01 定性一致。

最小合成路径(**Shape A,可行**):

- driver 侧在导航完成后经现有 CDP 连接读 `button.getBoundingClientRect()` 并发 `Input.dispatchMouseEvent`(mousePressed→mouseReleased @ 中心坐标)。引擎输入管线交付 **trusted** mouseup+click → ①测试自身的 mouseup/click 监听器照常跑全部引擎语义断言(isTrusted 转移 / redispatch InvalidStateError / 默认动作 click 不触发);②internal 缺省 promise 收到 click 即 resolve → done() 链闭合。
- **测试文件零改动 · vendor 文件零改动(空白 vendor 的缺省 internal click 本就是"等真点击"的合法钩子)· 三态判定逻辑零改动**——载具只供给 testdriver 存在的意义本身:一次真实用户输入。
- 防假三态自证:任何"页面内合成 dispatch 假点击"变体必被 isTrusted 断言诚实红灯("First mouseup event should be trusted"),假绿构造不可能。
- 依赖面(已落地):CDP `Input.dispatchMouseEvent` 端到端(master:`protocol.rs:1611` 路由 → `BridgeCommand::DispatchMouseEvent`(`servo_bridge.rs:77`)→ `bao_browser/src/cdp_handler.rs` 真实交付;1ae8dd4a 10-01 13:06 puppeteer full lifecycle GREEN 为正证据)。
- 运行时 contingency(实现时验证):①trusted click 由 mousedown+mouseup 对的合成语义面;②headless 面 button 布局坐标(CSS px/viewport)映射。
- **[已获批实施,2026-10-02 用户侧裁决「Shape A 批准,实施」] 实测验证通过**:`[shape-a] serviced testdriver click @ (43.0, 20.76)` → **Event-dispatch-redispatch 2/4 FAIL(TIMEOUT)→ 4/4 PASS(OK,1.1s)**。两项 contingency 双双实证:①trusted 语义——测试自身断言 "First mouseup event should be trusted" 通过(CDP Down/Up 经 servo 输入管线合成 trusted mouseup+click);②坐标映射——点击精确落 button,事件在目标上触发。引擎面全绿:redispatch InvalidStateError×4、isTrusted 转移、默认动作不触发、click 仍 trusted 全部通过——**该文件此前「2 条真实引擎缺口」假设证伪,纯载具限,引擎语义 4/4 正确**。
- 实施形态(零语义替换):INJECT 增 `test_driver_internal` 定义器包装(记录测试显式发起的 click 请求坐标到 `__wpt_pending_click__`,缺省 internal promise 原样保留);driver 收割循环内检测到待偿请求即发 CDP `Input.dispatchMouseEvent`(mousePressed+mouseReleased @ 请求坐标)。无请求=零输入,对无 testdriver 文件零行为变化。

### ③ 载具第二层:headless 帧生产停摆(rAF 门)——发现、否定性结果与终态载具

Shape A 单文件首航 4/4 后,全量复跑中 redispatch 仍 2/4 且无 tap 记录——追出**第二层载具限**:

- **现象**:`await waitForLoad`(load → `requestAnimationFrame(resolve)`)在多页顺序导航的第 2+ 页面上永不解——rAF 回调根本不触发,`new test_driver.click(...)` 未被执行,2 条 pending 到 harness 10s 默认超时。诊断三锚:2nd+ 导航 `readyState:"complete"` 且布局活性正常(`getClientRects().length===1`)但 rAF 探针不翻转;首导航必翻(3/3);新开 tab 亦不翻。
- **定性**:bao headless 的帧生产是**启动期一次性**的(boot frame 落点对首导航有竞态)——此后 CDP 载具面无法再产帧。rAF 门在 servo 由 compositor 帧 Tick 驱动,无帧即永挂。此属产品码域(帧生产策略),本合同不越界。
- **否定性结果(全部实测)**:①`Page.captureScreenshot`(→ `webview.paint()` + 15s spin,`page.rs:475`)在 2nd+ 导航上 spin 满超时无帧,且强帧推进 servo 动画时钟——animation 文件 13/13 → 7/13 扰动实锤,弃用;②`Input.dispatchMouseEvent` mouseMoved;③`Emulation.setDeviceMetricsOverride`;④`Target.createTarget` 新 tab——三者均不产帧。
- **终态载具(每文件独立浏览器实例 + rAF 就绪门)**:每文件起独立浏览器(首导航带 boot frame),INJECT 装 rAF 探针;`readyState===complete` 后 2s 探针仍未翻 → 判本实例 boot frame 未落该页 → 换新实例重试(bounded 3,env `WPT_ATTEMPTS`)。三重收益:redispatch 全量面 4/4(引擎语义全绿);所有文件获得同等「有帧载具」;顺带结构性消除 10-01 的 stale-results 收割竞态(EventTarget-dispatchEvent-returnvalue 766/0 伪影,见总表注)。终验:`[shape-a] serviced @ (43.0, 20.76)` → **4/4 PASS [1.1s]**,animation 13/13 零扰动。
- **残留**:若某文件 3 次尝试均未获帧,如实报 NO-HARVEST(attempts exhausted)——本轮 40 文件零触发;3 NO-HARVEST 为 crash/subframe 型(readyState 永不 complete,门不触发),与本载具面无关。

## 载体时点判定史(2026-10-01 当日,五轮实测收敛)

1. 前基线 0/40:三个 fixture/vehicle 缺陷叠加,非单一时点结论——
   a. manifest 路径缺 `dom/` 前缀 → 全部 404("Error response" 标题页,无 harness 可 harvest);
   b. driver INJECT 源串**语法损坏**(node --check 实证 `Unexpected token ')'`,pump 世代即坏,"async 文件可 harvest" 从未被真实测过);
   c. pump 分派结构性迟到(设计裁定成立,本轮已由 vendor 载体取代:ws_registry 注册改调 `servo::register_embedder_new_document_script`,pump 分派路径删除)。
2. INJECT 改轮询形(`setTimeout(poll,0)` 自续链)→ **同源导航 wedge**:driver 逐文件导航全是 `127.0.0.1:8944` 同源 → `window_for_replacement` 复用臂(Window 对象跨 pipeline 复用),自续 timer 链恒有一条 pending timer 跨越 replacement 边界 → `timers.rs:912 assert_eq!(pipeline, global.pipeline_id())` panic(`left: (3,3) right: (3,4)` 实证)→ ScriptThread 死亡。0ms 轮询期的 99.8% CPU 是 bao 泵(`run_with_bridge` yield_now 自旋)稳态,不是 wedge 信号(gdb 4 次采样证伪)。
3. **终态 INJECT 形**:零 timer——`install()`(document-start 同步)+ `DOMContentLoaded` + `load` 双事件钩子。DCL 在 parser 终任务内同步触发,先于 testharness 的 timer 延迟 completion 派发,同步完成型文件必赶上注册;无 timer 即无 pending 跨界,复用臂安全。
4. 中间轮 `layout.flexbox.balance` guard panic(已闭环):servo codegen `run.py` 的 `map_preference_name` 手写 MAPPING 表缺 `layout_flexbox_balance` 行(docstring 自述须与 prefs.rs 运行时映射双端同步,W54 只补了 stylo 侧)→ codegen 漏点号名 → 运行时 `Preferences::get_value` 查表 panic,任意 `.style` 触碰杀 ScriptThread(裸浏览器零注入复现 + backtrace 实证;修复四点 = run.py MAPPING +1 行 + Preferences struct 字段/const_default + stylo_static_prefs set 桥,横扫确认 toml servo_pref 7 名差集仅此一条,05:33 二进制重发后消失)。

## 偏差注记(user ruling 2026-10-01;remove-按-id 闭环 2026-10-01 同日)

- ~~servo 侧注册表按 `(WebViewId, source)` **同文去重**:与 CDP 规范"同文两注册是两条 entry、remove 按 identifier"有偏差~~ **已由 removeScript 按 id 合同闭环**:注册表升 `(WebViewId, u64, String)` 三元,vendor 自铸进程级单调 identifier(register 返回值 = CDP `identifier`,唯一 id 源),同文同页重注册幂等返回同一 id(一个 registry entry = 一个 CDP handle),`Page.removeScriptToEvaluateOnNewDocument` 按 id 注销(页内作用域,他页持 id 删不动;未知 id 按 Chromium page_handler.cc 实测语义回 "Script not found" 错误)。与 CDP 规范的残余差异仅:同文两注册在 Chrome 产生两条 entry 两个 id,本实现幂等合一(合同裁决 ③,消除双 handle 悬垂)。

## 遗留清单

- ~~[WPT 收敛波] 7 FAIL 文件为真实引擎缺口~~ **2026-10-02 载具波后全部翻转,0 FAIL 残留**:5 文件系引擎缺口,已由 10-02 absorb 波修复(createElement-namespace 51/51 / webkit-animation 13/13 / subclasses-constructors 49/49 / NodeIterator-removal 25/25 / DOMTokenList 175/175);2 文件系载具限,已根治(redispatch:Shape A + rAF 就绪门 → 4/4;NodeList-tampered-2:test-ci 档 8.3s → 1/1)。现行基线 = 2026-10-02 总表(37 PASS / 0 FAIL / 3 NO-HARVEST)。
- [载具面 · bao headless 帧生产] 帧生产为启动期一次性(boot frame),此后无帧 → rAF 门类测试在第 2+ 页面永挂。当前以每文件独立浏览器 + 就绪门在载具面规避;引擎侧连续产帧(如 headless 恒常 composite 或 CDP 可触发产帧面)属产品码域候选,未立项。
- [NO-HARVEST 3 文件] 均为 crash 型/子框架型测试(inactive-document-crash / null-browsing-context-crash / subframe incumbent-global),非时点问题,单独立项定位。
- [servo vendor 候选] pending window timer 跨同源 `window_for_replacement` 导航触发 `timers.rs:912` 断言 panic(上游不变量对 init-script 定时器不健壮;本波以 INJECT 零 timer 化规避,引擎侧加固待另立裁决)。
- ~~[CDP 面] `removeScriptToEvaluateOnNewDocument` 未接线;接线时注册表按 identifier 化~~ **已闭环(2026-10-01 removeScript 按 id 合同)**:注册表按 identifier 化落地,remove 按 id 真删(见偏差注记)。
- ~~[bao_cdp 直派面] `BridgeCommand::AddScriptToEvaluateOnNewDocument` → page `UserContentManager` 路径仍为 head 插入延迟任务时点~~ **已闭环(2026-10-01)**:memory bridge 面与 WS 面同落 vendor realm-entry 注入载体(CDP 规范时点),且 add 返回 vendor 自铸 identifier、remove 按其注销——双 CDP 面单源。保留面:cmd_add_script 另对当前 document 立即应用一次(evaluate_js_web,Chrome new-documents-only 的超集,行为自 W55 未变)。
