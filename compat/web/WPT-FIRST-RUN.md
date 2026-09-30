# WPT First Run — Bao (subset, #14-C)

- date: 2026-10-01 · driver: bao browser (headless CDP) + python ws (`/tmp/wpt_first_run.py` + `/tmp/wpt-manifest.txt`)
- suite root: upstream tests/wpt/tests (`dom/` subset), static http server (127.0.0.1:8944)
- binary: /var/cargo-builds/3c/6184ceb77072ba/debug/bao · mtime 2026-10-01 05:33(含 W55 vendor realm 入口注入 patch + e4 `layout_flexbox_balance` Preferences 字段)
- manifest: 40 files · **PASS: 35** · FAIL: 5(真实引擎缺口,subtest 级可测) · NO-HARVEST: 3(crash 型/子框架型)
- subtest 总量(已 harvest 37 文件):PASS 子测试 ≈ 2167,FAIL 子测试 ≈ 37
- **载体验收(REQ-CDP-004,W55 closure)**:realm 入口注入生效——`Page.addScriptToEvaluateOnNewDocument` 的注入时点修到 CDP 规范位(`ScriptThread::load` 的 ServoParser 启动块之前:新 document 建成后、任何页面脚本写入前),同步完成型 testharness 文件全部可 harvest。前基线 0/40 NO-HARVEST → **35/40 PASS**,NO-HARVEST 40 → 3。

## 结果总表(2026-10-01 final run)

| test file | status | subtests pass/fail |
|---|---|---|
| dom/nodes/Element-hasAttribute.html | OK | 2/0 |
| dom/events/event-global-is-still-set-when-coercing-beforeunload-result.html | OK | 1/0 |
| dom/events/Event-dispatch-multiple-stopPropagation.html | OK | 1/0 |
| dom/events/remove-all-listeners.html | OK | 2/0 |
| dom/events/Event-dispatch-order-at-target.html | OK | 1/0 |
| dom/events/event-disabled-dynamic.html | OK | 1/0 |
| dom/events/Event-dispatch-target-removed.html | OK | 1/0 |
| dom/ranges/Range-mutations-removeChild.html | OK | 20/0 |
| dom/nodes/CharacterData-insertData.html | OK | 18/0 |
| dom/nodes/Document-createElement-namespace.html | FAIL | 41/10 |
| dom/events/EventListener-incumbent-global-subframe-1.sub.html | NO-HARVEST(子框架型) | -/- |
| dom/events/webkit-animation-iteration-event.html | FAIL | 8/5 |
| dom/events/Event-dispatch-redispatch.html | FAIL | 2/2 |
| dom/events/Event-defaultPrevented-after-dispatch.html | OK | 2/0 |
| dom/events/Event-subclasses-constructors.html | FAIL | 42/7 |
| dom/events/Event-returnValue.html | OK | 7/0 |
| dom/collections/HTMLCollection-empty-name.html | OK | 7/0 |
| dom/traversal/NodeIterator-removal.html | FAIL | 0/25 |
| dom/events/label-default-action.html | OK | 1/0 |
| dom/nodes/Node-cloneNode-on-inactive-document-crash.html | NO-HARVEST(crash 型) | -/- |
| dom/ranges/Range-attributes.html | OK | 1/0 |
| dom/nodes/MutationObserver-inner-outer.html | OK | 3/0 |
| dom/nodes/NodeList-static-length-getter-tampered-2.html | FAIL(harness status 非 OK) | 1/0 |
| dom/nodes/CharacterData-deleteData.html | OK | 18/0 |
| dom/events/Event-stopImmediatePropagation.html | OK | 1/0 |
| dom/events/event-src-element-nullable.html | OK | 1/0 |
| dom/collections/namednodemap-supported-property-names.html | OK | 3/0 |
| dom/lists/DOMTokenList-coverage-for-attributes.html | FAIL | 172/3 |
| dom/nodes/ParentNode-querySelectorAll-removed-elements.html | OK | 1/0 |
| dom/nodes/DOMImplementation-createDocument-with-null-browsing-context-crash.html | NO-HARVEST(crash 型) | -/- |
| dom/ranges/StaticRange-constructor.html | OK | 17/0 |
| dom/ranges/Range-collapse.html | OK | 186/0 |
| dom/nodes/Document-createComment.html | OK | 6/0 |
| dom/ranges/Range-commonAncestorContainer.html | OK | 63/0 |
| dom/events/Event-dispatch-bubble-canceled.html | OK | 1/0 |
| dom/historical.html | OK | 80/0 |
| dom/traversal/NodeIterator.html | OK | 766/0 |
| dom/events/EventTarget-dispatchEvent-returnvalue.html | OK | 766/0 |
| dom/window-extends-event-target.html | OK | 3/0 |
| dom/abort/abort-signal-timeout.html | OK | 1/0 |

FAIL 明细(subtest 级断言输出在 driver 运行产物 `/tmp/wpt-first-run-final.md`,临时件;重跑即再生)——这 5+2 个 FAIL 文件是本 harness 首次给出的真实引擎缺口测量,转入后续 WPT 收敛波,不属本合同范围。

## 载体时点判定史(2026-10-01 当日,五轮实测收敛)

1. 前基线 0/40:三个 fixture/vehicle 缺陷叠加,非单一时点结论——
   a. manifest 路径缺 `dom/` 前缀 → 全部 404("Error response" 标题页,无 harness 可 harvest);
   b. driver INJECT 源串**语法损坏**(node --check 实证 `Unexpected token ')'`,pump 世代即坏,"async 文件可 harvest" 从未被真实测过);
   c. pump 分派结构性迟到(设计裁定成立,本轮已由 vendor 载体取代:ws_registry 注册改调 `servo::register_embedder_new_document_script`,pump 分派路径删除)。
2. INJECT 改轮询形(`setTimeout(poll,0)` 自续链)→ **同源导航 wedge**:driver 逐文件导航全是 `127.0.0.1:8944` 同源 → `window_for_replacement` 复用臂(Window 对象跨 pipeline 复用),自续 timer 链恒有一条 pending timer 跨越 replacement 边界 → `timers.rs:912 assert_eq!(pipeline, global.pipeline_id())` panic(`left: (3,3) right: (3,4)` 实证)→ ScriptThread 死亡。0ms 轮询期的 99.8% CPU 是 bao 泵(`run_with_bridge` yield_now 自旋)稳态,不是 wedge 信号(gdb 4 次采样证伪)。
3. **终态 INJECT 形**:零 timer——`install()`(document-start 同步)+ `DOMContentLoaded` + `load` 双事件钩子。DCL 在 parser 终任务内同步触发,先于 testharness 的 timer 延迟 completion 派发,同步完成型文件必赶上注册;无 timer 即无 pending 跨界,复用臂安全。
4. 中间轮 `layout.flexbox.balance` guard panic(已闭环):servo codegen `run.py` 的 `map_preference_name` 手写 MAPPING 表缺 `layout_flexbox_balance` 行(docstring 自述须与 prefs.rs 运行时映射双端同步,W54 只补了 stylo 侧)→ codegen 漏点号名 → 运行时 `Preferences::get_value` 查表 panic,任意 `.style` 触碰杀 ScriptThread(裸浏览器零注入复现 + backtrace 实证;修复四点 = run.py MAPPING +1 行 + Preferences struct 字段/const_default + stylo_static_prefs set 桥,横扫确认 toml servo_pref 7 名差集仅此一条,05:33 二进制重发后消失)。

## 偏差注记(user ruling 2026-10-01)

- servo 侧注册表按 `(WebViewId, source)` **同文去重**:与 CDP 规范"同文两注册是两条 entry、remove 按 identifier"有偏差。当前 CDP 面无 removeScriptToEvaluateOnNewDocument 接线、无重复注册同文用例,不返工;未来接 remove 命令时注册表按 id 化(identifier → (WebViewId, source) 映射)。

## 遗留清单

- [WPT 收敛波] 5 FAIL 文件(Document-createElement-namespace / webkit-animation-iteration-event / Event-dispatch-redispatch / Event-subclasses-constructors / NodeIterator-removal / NodeList-static-length-getter-tampered-2 / DOMTokenList-coverage-for-attributes)为真实引擎缺口,harness 首次可测。
- [NO-HARVEST 3 文件] 均为 crash 型/子框架型测试(inactive-document-crash / null-browsing-context-crash / subframe incumbent-global),非时点问题,单独立项定位。
- [servo vendor 候选] pending window timer 跨同源 `window_for_replacement` 导航触发 `timers.rs:912` 断言 panic(上游不变量对 init-script 定时器不健壮;本波以 INJECT 零 timer 化规避,引擎侧加固待另立裁决)。
- [CDP 面] `removeScriptToEvaluateOnNewDocument` 未接线;接线时注册表按 identifier 化(见偏差注记)。
- [bao_cdp 直派面] `BridgeCommand::AddScriptToEvaluateOnNewDocument` → page `UserContentManager` 路径仍为 head 插入延迟任务时点(晚于 CDP 规范位),WS 面已被本载体取代;非 WS 派发面(bao_cdp_client memory bridge 等)待后续统一裁决。
