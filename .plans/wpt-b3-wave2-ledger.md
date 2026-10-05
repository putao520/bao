# WPT B3 Chromium 逐格对齐基线台账 wave2(fetch 域 · constitution-A 分歧地图)

- date: 2026-10-04 · 分诊人: e51 · binary: `/tmp/bao-wpt/opt-target/test-ci/bao`(2026-10-04 12:27 build,test-ci profile,opt 基线纪律)
- 发现面: WPT 官方工具链 fetch 域全量(`run_bao_wpt_opt.py` = wave1 载具单行换 BAO_BINARY 指向 opt 二进制;零 wptrunner/wptserve 改动)
- 环境纪律: run1 开跑 load1=2.43 / raw 轮 load1≈2 / 两批 proc1 均静默窗启动(13:07-13:16 间一次 load1=23 并行构建风暴,3 分钟内消退,批 A 为稳定性裁定非时敏,不受污染);翻转格与缺陷候选 22 格一律 proc1 单跑裁定(L3 纪律,wave1 沿用)
- oracle: **wpt.fyi chrome-154.0.8037.92 stable Linux**(run id 6310091845533696,rev a990d18fbf,2026-10-03;summary_v2 124260 条本地化 `/tmp/bao-wpt/chrome-stable-summary-v2.json.gz`,fetch 域 952 条,s∈{O,P,F,T,N,E,C,S},c=[子测过,子测总])——未降级
- 二进制取证(防复发 e49 教训的修正版判别律):build.log(12:10)内 `Compiling usvg v0.48.1` **无** vendor 后缀=registry 时代编译行,但**该行不是二进制的 provenance**——clippath.rs 工作树修改(12:16)后出现第二次编译(fingerprint dep-lib 12:21:31 + rlib 12:21:35,`libusvg-abb175a682c7aa00`),二进制(12:27)链接该 rlib。**判别以 rlib/binary 字符串探针为准**:`"reference cycle detected"` 在二进制内命中(clipPath guard 在);旧 registry 时代 rlib(12:00)零命中(阴性对照成立)。window.open a309b714(08:00 commit)按时间线在内。**已知缺口(如实)**:linked-mask guard(54311cd4,mask.rs 12:40:58 修改)晚于二进制构建,**不在**本二进制内——fetch 域零 SVG 渲染面,不影响本 run 结论;后续 SVG 域 run 前必须重建
- 本地树 vs oracle revision 漂移: 同 wave1(本地快照 vs chrome 2026-10-03 run),按路径 join,代表例抽核排除

## 总判定

**fetch 域 938 格(bao 实测),对齐 Chromium 748 格(79.74%),分歧 190 格(20.26%)。** 与 wave1(dom+xhr 96.65%)相比对齐率显著低,原因结构性:fetch 域的 `.any.js` 变体展开把 SharedWorker 变体灌到 127 格(变体税),加上 fetch/metadata 的 SEC header 大面积 ini 已登记继承面。分歧内部三分:

| 类 | 格数 | 定性 |
|----|------|------|
| **SharedWorker pref 族**(已知名,L2/css-ledger H3 同桶,`SharedWorker is not defined`,pref 门非缺陷) | 127 | 只计数不重开 |
| **继承面**(ini expected TIMEOUT/ERROR,bao 完全复刻 servo 已知状态;chrome 过) | 34 | 按族登记(33 matches-ini + 1 ini-错位) |
| **真缺陷候选**(ini expected 过、bao 崩/挂、chrome 过;**22/22 经 proc1 稳定复现,零方差**) | 22 | 新桶,详见 B 节 |
| 正向分歧(bao 过 × chrome 挂/错,wave1 此项为 0,本轮 3) | 3 | bao 超出 Chromium 行为格,如实登记 |
| SKIP 配置面(chrome 过,bao skip) | 4 | crashtests 产物配置,非引擎分歧 |

两轮(run1 servo log 400.4s + raw 轮 442s,均 proc8)双轮对照:**零真实状态翻转、零双轮状态异形**(全部 only-in 差异为口径差:run1 意外集 vs raw+ini 分类面)。

## 两域 run 数字

| 轮 | tests | 耗时/进程 | 汇总口径 |
|----|-------|-----------|---------|
| run1(servo log) | 938 | 400.4s / p8 | 614 ran as expected / 4 skipped / 128 unexpected ERROR / 23 unexpected TIMEOUT / 125 unexpected OK / 99 tests 带非预期子测 |
| raw(--log-raw,canonical) | 938 | 442s / p8 | 逐格分类见上表与下节 |

run1 的 125 unexpected-OK 全部为 `expected ERROR` 的 serviceworker 变体格(bao 真跑通了 SW,ini 还停留在 ERROR)——SW 生命周期波(2026-10-04)的正向产出,join chrome 后 125 格里 122 格 chrome=O(=ALIGNED-pass,ini 滞后),非缺陷。

## fork 自治第一梯队(2026-10-05,用户「我们已经独立了」裁决)

| 项 | 交付 | 验证 |
|---|---|---|
| sec-fetch-site 重定向链(原④) | 50ddab13:http_redirect_fetch spec 外 origin-opaque 突变删除(§4.5 逐字核对零此步骤;现行载体 taint+序列化已在);注册表登记 33169c37 | https 双子测翻 PASS(19/19 Unexpected 0)+回归守护批 10/10 |
| ini 期望自治更新 | servo 树 ed8a211c2:4 ini 99 行删除(波内翻绿格翻期望,保留 3 登记缺口) | 逐条以本 session 实测翻转 |
| unenroll 集合卫生(多 client 收尾) | fd6727ce:ClientGone 算法变体+RED-1 地形发送点+移除键 (creation URL, pipeline) 防 reload 竞态 | live verbatim count=2→close B→count=1;SW 族 24/24 |
| deactivate-flush 第三出口 | e3ba6cc7:Document::unload Step 18 后 process_deferred_fetches(与 destroy 出口同形;spec step 6.1+Chromium BS-off 锚) | 集成级单因子钉死(563b7076 在场无 flush 仍红→仅 flush 绿 2/2);**WPT subtest5 翻转=双因子联合**(flush 使能 beacon+563b7076 解 popup executor wedge;e76 归因更正 2026-10-05,单因子对照省略——双因子皆保留皆目标,分解无行动后果) |
| D4 双载误标 | 563b7076:替换腿(window_for_replacement)同 global 复用致 P0 exit 误标后继活 realm DEAD→跳过仍有持有者的 global/末文档 exit 终局 mark | popup_global_mislabel 4/4 绿;诚实红=offline_render(其绿系误标掩盖 mark-latency 第三缺陷,#[ignore] 追踪+跟进合同 e77 续) |

## 波验收(2026-10-05,e67 环境重建后复跑,主会话 V 抽验)

WPT 载具第二次重建(venv+launcher+HEAD 二进制 9bb44a12 链)后复跑波内全部已修格:**7/10 test 级绿,5 格修前全红翻转**(new-window 12/12、activate-after 2/2、fetch-destination/iframe/font-face×2),2 锚格未回归(multi-window-6/window-open.https)。残留红格(下轮清单):①send-on-deactivate 整格(**e68 归因终态 2026-10-05:子测 3/5=上游 parity**(servo ini expected FAIL,Chromium deactivate-flush 特性面 bao/上游均未实现,登记继承面);**子测 4=liveness 慢非死**(同 binary A/B+隔离探针全链通,multiplier=2 绿,~90%;session-history restore executor 唤醒调度延迟推过 10s 边际,低优候选:hop 打点定位;fetchLater flush 出口全树仅 timer+destroy 两条,无 deactivate 钩子))②fetch-destination-worker 整格 **已翻绿**(e70 6e31fca4 多 client 模型:worker destination 子测 PASS,e71 destination 补拷+e73 targeting 随波)③fetch-destination 4 子测 **终态(2026-10-05,e72)**:SVGImageElement 已修翻绿(4fb021cc,上游裸 TODO 按 "as HTMLImageElement" 镜像补全 fetch 链,SW 观察 destination=="image" 全链贯通);AudioWorklet/CSS module/Text module 三面=特性缺口登记(webidl 无 audioWorklet 属性/SM 无 CSS+Text 编译 API/csp 0.8.1 pin 无 Text 变体——非 destination 置位问题,不硬修)④css-font-face http 变体 2 子测(sec-fetch-site 重定向,继承面)⑤锚格子测现状(sec-fetch-user forced 族/not-fully-active)。二进制 provenance:mtime>HEAD+clean tree+cargo 重链(+816B);EXCLUDED_PAGE_FACES 字符串不入 lib 二进制属 cfg(test) 设计。

## 对齐地图分类规则(per-cell,同 wave1)

对每个 bao test_end 格取 bao 状态 × chrome 状态(summary_v2 路径 join)+ servo ini 期望三参照:
- 双过(bao OK/PASS × chrome O/P)= ALIGNED-pass;bao TIMEOUT × chrome T = ALIGNED-TIMEOUT(双引擎同挂)
- chrome 过而 bao 非过 = 分歧,再按 ini 切「bao-matches-ini(继承)」vs「bao-UNEXPECTED(独有)」
- 双挂同形(bao ERROR × chrome E,ini=ERROR)= ALIGNED-double-fail;bao 过 × chrome 挂 = 正向分歧(单列)

## A. 既有族计数(只计数,不重开;引用前台账)

| 前台账桶 | 本轮格数 | 格形态 |
|----------|---------|--------|
| **L2 SharedWorker pref**(crash-ledger L2;css-ledger H3;wave1 5 格同桶) | **127** | 全部 `.any.serviceworker.html` 变体,错误体同形 `SharedWorker is not defined`(pref 门;fetch 域 `.any.js` 变体展开致桶暴涨,wave1 的 5 格→127 格是变体税不是回归) |
| sec-fetch 继承面(fetch-metadata 已登记) | 36 宿主 / 409 子测 FAIL | raw 量得 409 个 sec-fetch 子测 FAIL,其中 **406 个是 servo ini 已登记 expected FAIL**(继承面已登记),仅 3 个为 ini 未登记残差——「已知继承面」框架与实测吻合 |
| fetch-later quota cross-origin-iframe(ini ERROR) | 6 | `/fetch/fetch-later/quota/cross-origin-iframe/*` ×5 + `same-origin-iframe/multiple-iframes`,ini ERROR,servo 同病 |
| local-network-access(ini TIMEOUT) | 6 | dedicated-worker/fetch/service-worker/shared-worker/websocket/webtransport 全族,servo 同病 |
| fetch/metadata 元素族(ini TIMEOUT) | 14 | embed/element-embed/element-frame/input-image/video-poster/window-history/object/redirect ×3 等,servo 同病 |
| corb/orb(ini ERROR/FAIL) | 5 | response_block(ERROR)/nosniff/status(ERROR)/img-png-mislabeled(FAIL 子测),servo 同病 |
| compression-dictionary timing/link(ini TIMEOUT) | 3 | timing-001/with-link-element-baseURL/events,servo 同病 |
| ALIGNED-TIMEOUT 双挂同形(对齐证据) | 9 | abort/serviceworker-intercepted、cors-keepalive、corb/script-resource-nonsniffable、http-cache/basic-auth、lna/iframe-opener、css-images ×2(tentative)、element-video-poster、redirect-to-url-with-credentials——bao T × chrome T 逐格一致 |
| SKIP 配置面 | 4 | `/fetch/api/crashtests/huge-fetch.any{,.serviceworker,.sharedworker,.worker}.html`——servo wptrunner 产物配置跳过 crashtests,非引擎分歧 |
| 正向分歧 ×3(bao 过 chrome 挂/错) | 3 | `/fetch/content-type/response.window.html`(chrome=T,37/121)、`/fetch/fetch-later/quota/empty-payload.https.window.html`(chrome=E,9/9 子测全过但 chrome 整体 error)、`/fetch/metadata/unload.https.sub.html`(chrome=T)——零修复动作,登记为 constitution-A 可区分状态 |
| ini-错位 1 格 | 1 | `/fetch/stale-while-revalidate/fetch-sw.https.html`:bao TIMEOUT,ini ERROR(错位),chrome O——SW-reclaim 域继承,归继承面计数 |

## B. 新分歧桶清单(22 格,proc1 22/22 稳定,按反指纹暴露面优先级排序)

### B1 · fetch/metadata SEC header induce 挂起族 —— ~~SEC metadata 指纹面~~ **已归因(2026-10-04 e52,SEC 假说证伪):第 2 次 webfont 加载终态 EOF 丢失(net 层,bun_bridge 终态清理嫌疑第 1 位)**

- 格:`/fetch/metadata/generated/css-font-face.https.sub.tentative.html` + `/fetch/metadata/generated/css-font-face.sub.tentative.html` TIMEOUT ×2 + `/fetch/metadata/serviceworker-accessors.https.sub.html` TIMEOUT ×1;ini 全部无登记(=默认过),chrome 全 O
- 载体机制(读源实证):css-font-face 经 `fetch/metadata/resources/helper.sub.js` 的 `induceRequest` 触发 `@font-face` 请求后轮询 `record-headers.py?retrieve&key=` 回读服务端记录——**font 请求从未到达服务端**,回读轮询永挂(零子测落地);serviceworker-accessors 走 `service_worker_unregister_and_register` 同形态 setup 挂
- 归因域:`@font-face` 加载管线在 fetch/metadata 的 induce 面缺请求(或 font load promise 未 settle);反指纹语义 = Sec-Fetch-* 头在 font 请求上的缺失本身就是可指纹特征,与 STL 域直接相关
- **归因终态(2026-10-04 e52,strace+gdb+独立探针三方实证)**:上两行读源推断被修正——font 请求**有出栈且服务端有应答**(第 1 子测 PASS);挂的是**第 2 个 @font-face 加载**:同 document 第 2 次 webfont 加载必挂(与跨域/CORS/body/连接复用全无关),响应字节完整读回但终态 EOF 从未派发 → `web_fonts_still_loading()` 恒非零 → `document.fonts.ready` 永不 fulfill(纯 lost-wakeup,61 线程全停寂;tick 探针证 ScriptThread 活着,非门/唤醒问题)。**SEC 假说证伪**:Sec-Fetch-* 注入点(http_loader.rs:1409 Step 8.13)覆盖 font 请求,trustworthy 形态带全头,非 trustworthy 按 spec 省略(子测 1 PASS 即证)。**终判修正(2026-10-04 e56,gdb 断点全链)**:上列嫌疑序全部证伪——EOF 派发 4/4、计数 RET=0、泵 12 命中,net 层完全无罪;真根因双缺陷:①**W28 pin 快照化回归**(fontfaceset.rs:pin 构造时快照,switch_to_loading 换槽不重绑→首次 resolve 后门 3 恒 false,后续 fonts.ready 永久阻塞)②font_context.rs 未解析 url 静默 return 计数泄漏。修复 a393e354(RED→GREEN 0.51s,page_lifecycle 27/27);W28 行注记 b5034f1f
- 复现:`cd /tmp/bao-wpt && venv/bin/python run_bao_wpt_opt.py /fetch/metadata/generated/css-font-face.sub.tentative.html -- --processes 1`

### B2 · SW 注册/激活 setup 挂起族 —— **缺陷候选第 2 位(9 格,本轮最大新桶)**

- 格:`/fetch/api/request/destination/fetch-destination{,-frame,-iframe,-no-load-event,-worker}.https.html` ×5 + `/fetch/api/request/request-reset-attributes.https.html` ×1 + `/fetch/security/dangling-markup/dangling-markup-mitigation-allowed-apis.https.html` ×1 + `/fetch/security/dangling-markup/dangling-markup-mitigation.tentative.https.html`(ERROR ×1,错误体 `can't access property "active", registration is undefined`)
- 载体共性(读源实证):fetch-destination 全部以 `service_worker_unregister_and_register(t, kScript, kScope)` + `wait_for_state(t, registration.installing, 'activated')` 开场——**零子测落地 = setup 的 SW 激活等待永挂**;dangling-markup-mitigation 同依 SW registration(其 ERROR 形态直接暴露 registration 对象缺失)
- 归因域:SW 生命周期波(2026-10-04)修复了 register→waiting→active 主链与 updatefound/statechange(本轮 122 个 SW 变体格转绿为证),本族是**残余缺口**:`wait_for_state(installing→activated)` 等待路径 / registration 对象在部分 API 面(非 idlharness 主路径)不可达。与 SW 生命周期波同域,建议由该波 owner 顺链归因
- 复现:`cd /tmp/bao-wpt && venv/bin/python run_bao_wpt_opt.py /fetch/api/request/destination/fetch-destination.https.html -- --processes 1`
- **归因终态(2026-10-04 e53,主会话 V 采纳)**:setup 假说证伪(全部格 setup 子测 PASS,中继正常);挂点=setup 后观测通道,5 组根因:①SWGS `clients` 缺失(webidl 注释态,Clients 类型零存在)②Container `ready` 缺失(await undefined)③Container `onmessage` 缺失(expando 惰性)④**controller patch 丢失**(24255064 promise 波静默丢弃 dffa8f4e——已恢复 2918d029,e53 探针 `controller is null` 2+2→0+0)⑤preload 窄表+mediator 丢导航旗标。**修复进度**:controller 恢复✓(2918d029)· 合同 C mediator 旗标贯通✓(2e025890,isReloadNavigation RED→GREEN)· 合同 D preload 表扩容**停等用户裁决**(e55d 实证:六 token 封闭集=现行 HTML spec 逐字 parity,Chromium 接受全变体=可探测向量;codegen.py 先例同类,spec 正确性 vs 不可区分性待裁)· 合同 A/B(ready/onmessage/Clients 特性族)**待用户裁决立项**(上游全注释态无威胁收)。族外残留:`<frame>` 元素死(src 注释态)、iframe history.go(-1)(session-history 域)、isHistoryNavigation 子测 fixture 缺口(servo WPT 树无 hello.html)

### B3 · window.open popup 通信回环挂起族 —— **已根治+归因深化(2026-10-04)**

- 格:`/fetch/fetch-later/new-window.https.window.html` + `/fetch/fetch-later/activate-after.https.window.html` + `/fetch/fetch-later/send-on-deactivate.https.window.html` TIMEOUT ×3;ini 无登记,chrome 全 O
- 载体共性(读源实证):`new-window.https.window.js` 以 `window.open(popupUrl, target, features)` 开 popup 后**等待 popup 回传**——回环永挂。与 wave1 B2'(`/xhr/open-url-multi-window-6.htm`,已根治 a309b714 但本族仍挂)+ css-ledger H2(window.open 句柄族)同族:popup 打开已落地,popup→opener 通信/卸载语义仍有缺口;fetchLater 的 deactivate 依赖页卸载语义,三格同根
- 复现:`cd /tmp/bao-wpt && venv/bin/python run_bao_wpt_opt.py /fetch/fetch-later/new-window.https.window.html -- --processes 1`

- **B3 终态(2026-10-04)**:三格归约双根因已根治——D1 js: popup top-level load 缺步(315450b3,e62,spec navigate-to-javascript:-url 裁据+top-level 臂)/D2 HTTPS popup fetch 静默灭(6e5e8f8b,e63,**根因=REQ-SEC-001 遗留的页面 realm fetch 双栈覆盖与底层统一裁决冲突**,用户裁「底层统一」后页面 realm 排除 Node 栈 fetch,servo WHATWG fetch 独占页面传输面,e59 嫌疑面全证伪);测试夹具竞态另修(f30f7140,e64:非回归,基线自 born 红率 50-67%)。fetchLater 本体完整非特性缺口;附 D4 候选登记(popup 双载 DEAD_GLOBALS 误标,popup 生命周期域)+十面 WHATWG 遮蔽审查清单(页面 realm 同原则,CLAUDE.md 挂账)

### B4 · compression-dictionary Link-header 预载族 —— 缺陷候选第 4 位(4 格)

- 格:`/fetch/compression-dictionary/dictionary-fetch-with-link-element-{crossorigin,integrity,referrer-and-referrerpolicy}.tentative.https.html` ×3 + `/fetch/compression-dictionary/fetch-destination.tentative.https.html` ×1;ini 无登记,chrome 全 O
- 载体共性:`<link rel=preload>` 触发的 compression dictionary 获取链挂起(Link header 解析/字典获取任务未落地);与继承面 B-A 的 timing-001(同目录 ini TIMEOUT)同目录不同格——本轮 4 格是 ini 未登记的新形态

### B5 · 零散单格(5 格)

- `/fetch/http-cache/split-cache.html` TIMEOUT——HTTP cache 分区键(split cache by top-frame origin)未实现或测试载体挂
- `/fetch/security/embedded-credentials.tentative.sub.html` TIMEOUT——URL 内嵌凭证剥离(SEC 面)
- `/fetch/api/redirect/redirect-keepalive.https.any.html` TIMEOUT——redirect × keepalive 组合
- `/fetch/orb/tentative/hls.html` TIMEOUT——ORB media playlist(hls)判定路径
- 同格 proc1 全部稳定复现

## C. 子测级层(ini 感知;文件级双过格内的分歧,raw+ini 双解析)

- wptrunner 汇总口径:99 tests 带非预期子测;raw+ini 解析(文件级 OK 且带 ini-意外子测 FAIL):**63 宿主格 / 366 个意外子测 FAIL**
- 最大桶:`/fetch/redirect-navigate/preserve-fragment.html`(**120** 子测,导航片段保留)、`/fetch/orb/tentative/img-mime-types-coverage.tentative.sub.html`(46,ORB MIME 覆盖面)、`/fetch/api/headers/header-values-normalize.any.serviceworker.html`(26,header 值规范化)、`/fetch/corb/img-mime-types-coverage.tentative.sub.html`(15)、`/fetch/api/basic/response-null-body.any.serviceworker.html`(10)、`/fetch/content-length/parsing.window.html`(9)
- sec-fetch 残差:409 个 sec-fetch 子测 FAIL 中仅 **3 个** ini 未登记(继承面 406 个已登记)——task-header 所引「H2 残差 8 子测」在本轮实测为 3 格残差 + 406 已登记,按实测数登记
- 明细产物:`/tmp/e51-subtest-residual.json`(63 宿主逐格)+ `/tmp/e51-cells-details.json`(938 格全量,bao 态/chrome 态/ini/子测/消息)

## D. 缺陷候选队列 + 修复合同草案要点(只立候选,修复不在本波)

1. **B1 font-face metadata induce 挂起(2 格,SEC 指纹面)**——合同要点:①proc1 + 手工 page 加载 @font-face 探针确认 font 请求是否出栈(net 日志面);②嫌疑 `@font-face` 触发的加载是否走 fetch 管线(servo font 载体 vs Fetch 管线分叉);③与 STL Sec-Fetch-* 头注入面对齐验收
2. **B2 SW wait_for_state 残余缺口(9 格)**——合同要点:①与 SW 生命周期波 owner 顺链;②二分 setup:`service_worker_unregister_and_register` 单独跑(registration 返回?)→ `wait_for_state(installing,'activated')`(statechange 是否 fire);③横扫所有 test-helpers.sub.js 载体(SW 依赖的 fetch 域外还有 WPT 全域)
3. **B3 window.open popup 回环(3 格)**——与 wave1 B2' 根治波(a309b714)衔接:popup 打开后 opener↔popup 消息/卸载语义;fetchLater deactivate 面一并验
4. B4 compression-dictionary Link 预载(4 格)——特性域,先归因 Link header 是否进入 fetch 管线
5. B5 零散 5 格——逐格特性缺口 backlog(cache 分区/凭证剥离/keepalive-redirect/ORB-hls)

## 方法与可重复性

- 载具:`/tmp/bao-wpt/run_bao_wpt_opt.py`(wave1 载具同形,BAO_BINARY=opt-target/test-ci/bao)+ `/tmp/bao-wpt/venv` 上游 wptrunner 胶水,零 wptrunner/wptserve 改动
- 命令:`cd /tmp/bao-wpt && venv/bin/python run_bao_wpt_opt.py fetch -- --processes 8 [--log-raw /tmp/e51-fetch-raw.json]`
- oracle join:`/tmp/e51-join.py`(raw × summary_v2 × servo ini 三方;ini 解析含 subtest 级,owner=最深键)+ `/tmp/e51-subtest-residual.py`
- proc1 裁定:批 A 12 格 398.3s(``/tmp/e51-proc1-batchA.log``)+ 批 B 10 格 221.2s(`/tmp/e51-proc1-batchB.log`),22/22 稳定
- 二进制取证:`strings` rlib/binary 探针(clipPath guard 串,阳性+阴性对照)

## 局限(如实)

1. 逐格粒度=文件级(chrome summary_v2 无子测名);63 宿主的 366 意外子测为 bao 侧 ini 感知计量,chrome 侧子测对照不可行(同 wave1 局限)
2. revision 漂移(本地快照 vs chrome 2026-10-03 run)未逐格排除,代表例抽核
3. linked-mask guard 不在本 run 二进制(12:27 build 早于 12:40 mask.rs 修改)——fetch 域零影响,后续 SVG 域 run 前必须重建二进制
4. run1 的 wptrunner 子测口径(99 tests)与 raw+ini 口径(63 宿主)覆盖面不同(前者含非 OK 文件的子测与 NOTRUN/TIMEOUT 子测态),两数并存按各自口径引用
5. load 风暴(13:09-13:12,load1 峰值 23)与批 A 后半段重叠——批 A 结论为稳定性复现(挂/过二值),非时敏测量,判定不受污染;但 60s 死线附近的边界格(如有)理论上可能被负载推迟,22 格中无边界格(全部 0 或整 60s 形态)
