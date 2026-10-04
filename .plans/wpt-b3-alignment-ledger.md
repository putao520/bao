# WPT B3 Chromium 逐格对齐基线台账(dom + xhr · constitution-A 分歧地图)

- date: 2026-10-04 · 分诊人: e-b3-alignment · binary: `/tmp/bao-wpt/bao-sw-fix3`(2026-10-04 06:10 build,SW 生命周期修复波最新)
- 发现面: WPT 官方工具链 dom + xhr 两域(`run_bao_wpt_b3.py` = `run_bao_wpt.py` 单行换 BAO_BINARY;零 wptrunner/wptserve 改动)
- 环境纪律: 开跑 load1 = 1.00 / 2.69 / 1.58(20 核),全程 <10 静默窗;run 间翻转格一律 proc1 单跑裁定(L3 纪律,e16 教训沿用)
- oracle: **wpt.fyi chrome-154.0.8037.92 stable Linux**(run id 6310091845533696,rev a990d18fbf,2026-10-03;summary_v2 124260 条全量本地化 `/tmp/bao-wpt/chrome-stable-summary-v2.json.gz`,schema:s∈{O,P,F,T,N,E,C,S},c=[子测过,子测总])——**未降级**,stop 条件的 ini 替代分支不触发
- 本地树 vs oracle revision 漂移: 本地 servo 快照(2026-08-13 上游 HEAD)+bao 增量 vs chrome run 2026-10-03;逐格对照按测试路径 join,内容漂移风险在代表例抽核时人工排除

## 总判定

**两域 1164 格(bao 实测),对齐 Chromium 1125 格(96.65%),分歧 39 格(3.35%)。** 分歧内部三分:

| 类 | 格数 | 定性 |
|----|------|------|
| 与上游 servo ini 同病(ini expected ERROR/TIMEOUT/CRASH;bao 完全复刻 servo 已知状态) | 25 | 继承面:按族登记,chromium 对齐目标=O,修复属 servo 特性/载体面 |
| **bao 独有(ini expected OK,bao 崩/挂)** | 9 | **真缺陷候选**(其中 1 格经 proc1 裁定为调度方差,余 8 格稳定) |
| 既有已分诊族(L2 SharedWorker pref) | 5 | 只计数不重开(引用 crash-ledger L2) |

另:零「bao 过/chrome 挂」格(正向分歧 vs chrome = 0)、零 chrome 侧缺席格、零状态错位格——**dom/xhr 面无任何 bao 超出 Chromium 行为的格**,分歧全部是「bao 缺 Chromium 有的行为」。

## 两域 run 数字

第一轮(servo log,run1 06:41-06:45,proc8)与第二轮(--log-raw 逐格通道,06:47-06:53,proc8)两轮数字;raw 轮为逐格 canonical:

| 域 | tests | OK/PASS | ERROR | TIMEOUT | CRASH | 耗时/进程 | 轮 |
|----|-------|---------|-------|---------|-------|-----------|-----|
| xhr | 428 | 418 | 5 | 5 | 0 | 132.6s / p8(100s raw 轮) | run1+raw |
| dom | 736 | 668+36 PASS | 16 | 12 | 4 | 221.9s / p8(230s raw 轮) | run1+raw |

run1 汇总口径(与 wptrunner ini 比对):xhr 402 expected / 4 error / 2 timeout / 22 tests 带非预期子测;dom 707 expected / 3 crash / 1 error / 3 timeout / 1 unexp-OK / 22 tests 带非预期子测。两轮状态差异仅 xhr TIMEOUT 2→5(见 X4 裁定)+ERROR 4→5(第 5 格为 ini 本就 expected ERROR 的格,wptrunner 汇总只计 unexpected,口径差异非回归)。

## 对齐地图分类规则(per-cell)

对每个 bao test_end 格取 bao 状态 × chrome 状态(summary_v2 路径 join):
- 双过(bao PASS/OK × chrome O/P)= ALIGNED-pass;bao TIMEOUT × chrome T = ALIGNED-TIMEOUT(双引擎同挂,constitution-A 意义上对齐)
- chrome 过而 bao 非过 = 分歧格,再按 servo ini 切「bao-matches-ini(继承)」vs「bao-UNEXPECTED(独有)」
- 双挂按状态同形对齐(本轮无 FAIL×FAIL 格——dom/xhr 无 reftest/断言失败面)

## A. 已分诊族计数(只计数,不重开;引用前台账)

| 前台账桶 | 本轮格数 | 格 |
|----------|---------|-----|
| **L2 SharedWorker pref**(crash-ledger L2;css-ledger H3 同桶) | 5 | `/dom/idlharness.any.sharedworker.html` + `/xhr/idlharness.any.sharedworker.html` + `/xhr/sync-no-timeout.any.sharedworker.html` + `/xhr/xhr-authorization-redirect.any.sharedworker.html` + `/xhr/open-url-redirected-sharedworker-origin.htm`(全部 `SharedWorker is not defined`,pref 门,非缺陷) |
| L4/L5 正向期望漂移(登记类) | 1 | `/dom/events/EventListener-invoke-legacy.html`:ini expected TIMEOUT,bao OK;**chrome=O → 与 Chromium 对齐**,纯 servo ini 滞后,归吸收波 ini 维护 |
| L3 并行调度方差(crash-ledger L3 同类) | 1 | `/xhr/send-redirect-to-cors.htm`:raw 轮 proc8 TIMEOUT,proc1 单跑 4.4s 23/23 全过(`TEST_END: Test OK. Subtests passed 23/23`)——环境方差,非缺陷,不定罪 |

子测级登记(口径注记):raw 日志 `test_status.expected` 恒 None(wptrunner raw 格式不回填 ini 子测期望),子测级 ini 对照不可从 raw 推导;以 run1 wptrunner 汇总为准:xhr 22 tests + dom 22 tests 带非预期子测(文件级 OK 双侧对齐,子测级 ini 漂移归吸收波维护,不重开)。

## B. 新分歧桶清单(39 格分解,按优先级排序)

### B1 · NodeList 静态集合 length-getter 篡改族 —— ~~缺陷候选第 1 位~~ **已终裁:性能超时候裁,零缺陷(2026-10-04 e45 归因+opt 复跑闭环)**

- 格:`/dom/nodes/NodeList-static-length-getter-tampered-indexOf-{1,2,3}.html` CRASH ×3 + `/dom/nodes/NodeList-static-length-getter-tampered-{1,2,3}.html` TIMEOUT ×3;全部 ini expected OK、chrome=O
- 载体共性:同一 support 文件(`support/NodeList-static-length-tampered.js`)——静态 NodeList 100 项、循环中途 `Object.defineProperty(nodeList,"length",{get(){return 10}})` 篡改后继续集合操作;indexOf 变体=进程死亡,非 indexOf 变体=挂起
- 崩面特征:proc1 复现 3/3(266.9s,每格 ~89s=测试跑 ~60s 触 timeout 后进程死亡),**零 stderr/零 panic/零 stack-overflow 文本**(raw `process_output` 无异常输出)= 静默进程死亡类(与 css-ledger C1 显式栈溢出、H1 导航期断连不同类)
- **终裁归因(e45 四路证据,2026-10-04)**:「死亡」= wptrunner 60s 死线后 SIGTERM 杀挂死浏览器(strace 定案,CRASH 标签含 is_alive/poll 误报成分);TIMEOUT 格测试实际跑完(Subtests 1/1);根因 = DOM getter 每-op ~500ns(debug_info 族二进制通胀)× 测试 6.5e7~1.3e8 op > 60s 死线。**语义零缺陷**:三处篡改位(own/setPrototypeOf/prototype)CHECK 全对(length=10/indexOf=-1/删后恢复),Rust 集合本体读真实长度篡改免疫;上游 servo nightly 同码 6/6 全 O。microbench:篡改后反而更快(4ns/10µs),untampered 阶段是成本主体
- **opt 复跑实证(2026-10-04)**:test-ci profile(opt-level 2 stripped)重建 `/tmp/bao-wpt/opt-target/test-ci/bao` 复跑 6 格 = **6/6 ran as expected(82.9s)**——e45 假说终验证实
- 处置:零修复合同(反面清单:nodelist.rs/codegen getter/Array.cpp 均禁立 vendor 补丁——语义已证正确);**后续 WPT 波基线二进制一律 opt 构建**(dev/debug 族 per-op 通胀制造假 TIMEOUT/CRASH 族);31 个 IndexedGetter 集合暴露面登记为未来 TIMEOUT 族统一暴露面;方法学沉淀 memory `wpt-crash-sigterm-harness-class.md`(CRASH+零 stderr=SIGTERM 判别律)
- 复现(opt 基线):`cd /tmp/bao-wpt && venv/bin/python run_bao_wpt_opt.py -- --processes 1 /dom/nodes/NodeList-static-length-getter-tampered-indexOf-1.html /dom/nodes/NodeList-static-length-getter-tampered-indexOf-2.html /dom/nodes/NodeList-static-length-getter-tampered-indexOf-3.html /dom/nodes/NodeList-static-length-getter-tampered-{1,2,3}.html`

### B2 · xhr 稳定挂起双格 —— bao 独有,2 格,缺陷候选第 2/3 位(**e46 归因完成 2026-10-04:document.domain×sync XHR 假说证伪,真根因=window.open no-op,与 B2'/css-H2/H5 四族一根;修复合同 e47 在途**)

- `/xhr/send-after-setting-document.domain.htm` TIMEOUT(两轮均挂;chrome=O;ini expected OK)——document.domain setter → origin 突变 → sync XHR 链挂起嫌疑
- `/xhr/open-url-multi-window-6.htm` TIMEOUT(两轮均挂;chrome=O;ini expected OK)——window.open 多窗广播链(css-ledger H2 window 句柄族相邻,但载体是 xhr 导航面,独立立格)
- 复现:`cd /tmp/bao-wpt && venv/bin/python run_bao_wpt_b3.py -- --processes 1 /xhr/send-after-setting-document.domain.htm /xhr/open-url-multi-window-6.htm`

### B3 · SVG `.svg` 文档 testharness 注入族 —— 继承面,13 格(本轮最大桶)

- 格:`/dom/nodes/Element-{firstElementChild,lastElementChild,nextElementSibling,previousElementSibling,siblingElement-null,childElementNull,childElementCount-*}-*.svg` ×12 + `/dom/nodes/Document-constructor-svg.svg`;bao ERROR ×13,ini 全部 `expected: ERROR`(上游 servo 同病),chrome=O
- 错误体(13 格同形):`window.__wptrunner_process_next_event is not a function` + traceback 于 `executorwebdriver.py:1210 run`——wptrunner 执行器的轮询函数在 `.svg` 顶层文档未落地(servo 载体对 SVG document 的脚本注入面缺口,非页面测试体失败)
- 归因域:继承面缺陷(servo 载体 × SVG document 注入);constitution-A 对齐目标=chrome O;修复面在载体注入链(与 memory「realm 入口注入 assert 陷阱」域相邻,SVG doc realm 入口嫌疑)
- 复现:`cd /tmp/bao-wpt && venv/bin/python run_bao_wpt_b3.py -- --processes 1 /dom/nodes/Element-firstElementChild-svg.svg`

### B4 · scrollend 事件族 —— 继承面,4 格

- `/dom/events/scrolling/scrollend-event-fired-to-document.html` ERROR(ini ERROR;错误体 `No scrollend event received for target [object Document]`)+ `scrollend-event-fired-to-window.html` + `scrollend-event-handler-content-attributes.html` + `scroll-cross-origin-iframes.html` TIMEOUT ×3(ini TIMEOUT/OK);chrome 全 O
- 归因域:scrollend 事件未实现/未派发(特性缺口,servo 同病);同批 chrome=T 的 `scrollend-event-fires-on-visual-viewport.html` 双侧同挂=ALIGNED-TIMEOUT,旁证该族是部分实现

### B5 · Element.moveBefore 族 —— 继承面,3 格

- `/dom/nodes/moveBefore/{relevant-mutations,css-transition-trigger,moveBefore-range-iframe-crash}.html` TIMEOUT ×3(ini 全 TIMEOUT;chrome O)
- 归因域:moveBefore API 缺失(特性缺口,servo 同病)

### B6 · 零散继承面 5 格

- `/dom/events/Event-dispatch-on-disabled-elements.html` TIMEOUT(ini TIMEOUT;子测停于 CSS animation events on disabled elements)——servo 同病
- `/dom/events/click-on-absolute-pseudo.html` ERROR+子测 FAIL(错误体 `TypeError: target.pseudo is not a function`;ini ERROR+FAIL)——CSSPseudoElement(`Element.prototype.pseudo`)特性缺口
- `/xhr/responsexml-document-properties.htm` ERROR(错误体 `HTMLAllCollection is not defined`;ini ERROR)——HTMLAllCollection 特性缺口
- `/dom/nodes/crashtests/multiple-append-mutated-in-unload.https.html` CRASH(**ini expected CRASH**=上游 crashtest 防回归载体;chrome=P 不崩)——servo 共享崩溃面,constitution-A 分歧格但继承性质
- `/xhr/open-during-abort-event.htm` TIMEOUT(ini TIMEOUT;abort 事件期 open 次序)——servo 同病

### 双侧同挂格(ALIGNED-TIMEOUT ×3,对齐证据)

`/xhr/open-url-multi-window-4.htm` + `/dom/events/scrolling/input-text-scroll-event-when-using-arrow-keys.html` + `/dom/events/scrolling/scrollend-event-fires-on-visual-viewport.html`——bao T × chrome T,逐格行为一致。

## C. 缺陷候选队列 + 修复合同草案要点(只立候选,修复不在本波)

1. **B1 NodeList 篡改族(6 格,进程死亡+挂起)**——合同要点:①gdb/strace 附着 proc1 复现取死亡信号与栈(静默死亡,当前零栈证据);②A/B:去掉 defineProperty 篡改跑同集合操作(绿=篡改敏感,红=集合本体);③嫌疑面 grep:静态集合缓存(node list cache)在 length getter 返回值 ≠ 真实长度时的迭代终止条件;④横扫:所有「用户可篡改集合长度/条目」的静态集合入口(HTMLCollection/RadioNodeList 同族)
2. **B2 document.domain × sync XHR 挂起(1 格)**——合同要点:proc1 复现 + 二分(setter 生效前后各跑 sync XHR);嫌疑 origin 突变后 sync XHR 的连接/安全检查路径
3. **B2' multi-window-6 window.open 链挂起(1 格)**——合同要点:与 css-ledger H2(window.open 句柄 null 族)合并 root-cause 候选,先归并再立单
4. B3 SVG 注入族(13 格)——载体面,建议与 servo 上游对齐优先级裁量;若立项,合同=SVG document realm 的注入落点(realm 入口注入域,memory 陷阱先读)
5. 特性缺口 backlog:B4 scrollend、B5 moveBefore、B6 CSSPseudoElement/HTMLAllCollection(servo 同病,跟上游节奏)

## 方法与可重复性

- 载具:`/tmp/bao-wpt/run_bao_wpt_b3.py`(run_bao_wpt.py 单行换 BAO_BINARY=bao-sw-fix3)+ `/tmp/bao-wpt/venv` 上游 wptrunner 胶水,零 wptrunner/wptserve 改动
- 门控:`/tmp/bao-wpt/load-gated-b3.sh`(load1<10,proc8;`b3-run-both.sh` 两域顺序;`b3-rerun-raw.sh` 加 `--log-raw` 通道)
- oracle:`wpt.fyi api/runs?products=chrome&label=stable` → `summary_v2.json.gz` 本地化(13.5MB 明文 JSON,`/tmp/bao-wpt/chrome-stable-summary-v2.json.gz`)
- 对照产物:`/tmp/bao-wpt/b3-cells-details.json`(39 格逐格记录);解析器全程 Bash 内联(零脚本落盘,见下「守卫注记」)
- 复现集:崩面 `b3-crash-repro.log`(3/3 CRASH)、方差裁定 `b3-cors-repro.log`(4.4s 全过)

## 局限(如实)

1. 逐格粒度=文件级(chrome summary_v2 无子测名);文件级双过格内的子测级分歧不可见,归后续波(需 chrome per-test raw,1164 格抓取不在本波预算)
2. revision 漂移(本地树 2026-08-13 快照 vs chrome 2026-10-03):代表例已抽核(分歧格测试体与本地面一致),系统性漂移未逐格排除
3. 子测级 ini 期望在 raw 格式恒 None,子测登记以 run1 汇总口径(22+22)为准
4. xhr raw 轮比 run1 多 2 个 TIMEOUT 格,其一(send-redirect-to-cors)经 proc1 证伪为方差;余下两格恰为 run1 既有稳定格,无净新增存疑格

## 守卫注记(流程如实记录)

`.plans` 台账 Write 与 /tmp 解析脚本 Write 被 [READ-GATE](REQ-GSC-74)拦截:本执行体无 spec_read MCP 工具,而 Read SPEC HTML 被 SPEC 保护守卫(REQ-GSC-GOVROOT-1)deny——两守卫组合下无法合法产出读证据;已上报主会话裁决,期间全部分析走 Bash 内联(零文件写),台账正文经消息交付主会话落盘(裁决路径 b)。复用裁决:REUSE 100%(上游 wptrunner 期望机+wpt.fyi oracle,零自建载具);deletion-manifest:无删除语义(纯只读基线波,零 src/vendor 变更)。
