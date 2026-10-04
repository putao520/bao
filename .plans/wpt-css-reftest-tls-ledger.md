# WPT 三面实弹台账(css / reftest / https-TLS · 分诊基线)

- date: 2026-10-04 · 分诊人: e-css-reftest-tls · binary: `/tmp/bao-wpt/bao-e26-final`(45284afe,livelock 根治后,8 进程安全)
- 发现面: WPT 官方工具链三面(`run_bao_wpt_e26.py` = `run_bao_wpt.py` 单行换 BAO_BINARY;零 wptrunner 改动)
- 环境纪律: 沿用 `.plans/wpt-crash-ledger.md` 头部——关键面 load < 10(20 核半)再跑;本轮三面开跑时 load1 = 1.70 / 1.13 / 1.40,全程静默窗
- reftest 通道预判: #40 状态下 screenshot 通道存疑,先探针后全量(见 R0)。**探针判定:通道活**

## 总判定

三面全部产出 run 数字,零环境阻塞:

| 面 | 载体 | tests | expected | error | crash | timeout | unexp-OK | subtest | 耗时/proc |
|----|------|-------|----------|-------|-------|---------|----------|---------|-----------|
| reftest(探针 R0) | 3 个 floats reftest | 3 | 3 | 0 | 0 | 0 | 0 | 0 | 2.6s / p2 |
| reftest(面 R1) | css/CSS2/floats | 147 | 146 | 0 | 0 | 0 | 0 | 0 | 13.7s / p4 |
| https/TLS(H) | fetch 域 https 子面 | 113 | 76 | 5 | 1 | 23 | 1 | 34 | 471.4s / p4 |
| css(C) | css/ 全量 | 33502 | 26661(+8 skip) | 443 | 4 | 333 | 18 | 1757 | 3649.5s / p8 |

C 面另有 FAIL ×3978 与 unexpected-PASS ×417(两桶见 C6/C8)。

**screenshot RGBA 通道判定:活。** 证据三级:R0 探针 3/3 as expected(2.6s)→ R1 面 147 reftest 146 expected(13.7s,proc4)→ C 面全量 reftest 主导树 proc8 大规模复证。#40 前提(2026-10-02 修正:composite=渲染管线心跳)与本结果一致——泵循环 composite 在位,screenshot 通道可用。

## 台账条目

### R0 · reftest 探针 — screenshot 通道活

- 复现命令: `cd /tmp/bao-wpt && venv/bin/python run_bao_wpt_e26.py -- --processes 2 css/CSS2/floats/floats-placement-vertical-001a.xht css/CSS2/floats/floats-placement-vertical-001b.xht css/CSS2/floats/floats-placement-vertical-001c.xht`
- 证据: `/tmp/bao-wpt/reftest-probe1.log` — "Ran 3 tests finished in 2.6 seconds. • 3 ran as expected."
- 机制: `ServoRefTestExecutor`(WebDriver `take_screenshot`,RGBA hash 对比);log 中 "Unable to find wpt-prefs.json" 为非致命告警(上游同形)
- 处置: **通道活,css 面放行全量**;任务书预想的「环境受限如实报」分支不触发

### R1 · reftest 面(css/CSS2/floats,147 reftest)

- 复现命令: `cd /tmp/bao-wpt && venv/bin/python run_bao_wpt_e26.py -- --processes 4 css/CSS2/floats`
- 证据: `/tmp/bao-wpt/reftest-face-e26.log` — 147 tests / 146 expected / **1 FAIL** / 0 crash / 0 timeout
- 唯一 FAIL: `floats-line-wrap-shifted-001.html`(test hash `fae3f077…` ≠ ref hash `5b2db04a…`)——**布局像素真分歧**(float 折行位移),缺陷候选
- 归因域: 布局面(float 折行),非 screenshot 通道、非环境(proc4 静默窗,同批 146 条过)
- 处置建议: 归入布局缺陷 backlog(待根治面);复现=单跑该条 + dump 双截图 hash

### H1 · https 面 CRASH ×1 — `fetch/metadata/report.https.sub.html`(真缺陷,可复现)

- 复现命令: `cd /tmp/bao-wpt && venv/bin/python run_bao_wpt_e26.py -- --processes 1 fetch/metadata/report.https.sub.html`(load 1.30 静默窗)
- 证据: `/tmp/bao-wpt/https-crash-confirm.log` — `TEST_END: CRASH`;wptrunner 侧 `webdriver.url = url` POST 导航时 `RemoteDisconnected: Remote end closed connection without response`(bao 进程死亡,WebDriver 连接关闭),启动后 3.2s 内
- 载体特征: 该页唯一非常规面 = `<link rel=stylesheet href="https://{{hosts[alt][élève]}}:8443/...">` 指向 **IDN(punycode)alt-host**;嫌疑 IDN/TLS 子资源路径,但崩于导航期,崩点未定位
- 归因域: **缺陷(进程崩溃类)**,与已根治 livelock 类(45284afe)不同——本次是进程死亡非泵死等;单跑 proc1 复现,排除负载/并发
- 处置建议: 立根治任务;第一步二分(裸 about:blank→本页 URL 导航;gdb/cleanup 附着取崩溃栈),第二步验 IDN 假设(非 IDN alt-host 同形页对照)

### H2 · https 面 ERROR ×3 — window.open/handle null + cleanup-fail 类(缺陷候选)

- 复现面: `fetch/metadata/generated/window-location.https.sub.html`、`element-meta-refresh.https.optional.sub.html`、`header-refresh.https.optional.sub.html`(文件级 exp=OK)
- 证据: `/tmp/bao-wpt/https-face-e26.log` — window-location: `TypeError: can't access property "location", win is null`(testdriver `bless`→navigate 链,win 为 null);meta-refresh/header-refresh: `Test named 'sec-fetch-site - Same origin' specified 1 'cleanup' function, and 1 failed`(NOTRUN 级联)
- 归因域: 缺陷候选(window.open 返回句柄/navigation 后窗口对象生命周期),非环境
- 处置建议: 归入 window/navigation 缺陷 backlog;同族还有 H5 的 `window-open.https.sub.html` TIMEOUT(Same-origin/same-site window forced 全超时)
- **部分根治注记(2026-10-04,a309b714)**:window-location 格 + H5 的 window-open.https.sub.html 格已随 window.open 落地修复转绿(e47 实现 request_create_new 真身,主会话 V 四格复验 4/4 ran as expected,含此二格);meta-refresh/header-refresh 两格(cleanup-fail 类)不在该根因域,留桶待另归因

### H3 · https 面 ERROR ×1 — `fetch/metadata/sharedworker.https.sub.html` — **既有 L2 桶,不重开**

- 证据: `SharedWorker is not defined`(SharedWorker pref 默认关,pref 门,非缺陷)——与 crash-ledger L2 同桶同形
- 处置: 归 L2;absor波 pref 面统一裁

### H4 · https 面 ERROR ×1 — `dangling-markup-mitigation.tentative.https.html` — SW getRegistration 缺口

- 证据: `Unhandled rejection: can't access property "active", registration is undefined`(navigator.serviceWorker.getRegistration() resolve undefined)
- 归因域: SW 注册→查询链缺陷候选(与 H5 同族,SW 面非 as-global 加载路径)
- 处置建议: 与 H5 合并 root-cause(SW registration 生命周期)

### H5 · https 面 TIMEOUT ×23 — 主桶:SW 注册→激活→拦截链挂起(缺陷候选)

- 复现面(23 条,`/tmp/bao-wpt/https-face-e26.log`):
  - **SW 链特征 ~18 条**: fetch-destination-* ×6 + compression-dictionary ×4(referrer/link-integrity 变体)+ referrer-*-service-worker ×4 + serviceworker-intercepted + fetch-via-serviceworker + serviceworker-accessors + stale-while-revalidate/fetch-sw + dangling-markup-allowed-apis
  - **window 强制导航**: window-open.https.sub.html(归 H2 族)
  - **metadata/navigation 零散**: request-reset-attributes 等
- 关键证据: 前两族共享同一挂起点——首子测试 **「Initialize global state」TIMEOUT**(fetch-destination-worker 与 compression-dictionary 双例同形,`/tmp/bao-wpt/https-face-e26.log`);该 setup 步 = 注册观察用 ServiceWorker。SW 注册后不激活/不拦截 → 全链 NOTRUN/超时
- 与 L5 不矛盾: 编码面 14 条 `.any.serviceworker.html` 是**加载为 SW global**(SW 已由上游 harness 注册或直接实例化)→ 全 OK;本面是**页面侧 register()→active→intercept 流**挂起。SW 存在性 ✓,注册流 ✗
- 归因域: 缺陷候选(SW 注册流/拦截链),非环境(静默窗 proc4,同批 76 条过);timeout 面非饥饿——L3 类是单条长测被挤,本桶是同特征聚集=机制性
- 处置建议: 立根治任务(SW register→activate→fetch 拦截链时序);compression-dictionary 4 条即使 SW 通仍需特性面(登记特性缺口,勿并入 SW 修复判定)

### H6 · https 面 unexp-OK ×1 — 期望基线漂移(正向),**既有 L5 同类桶**

- 复现面: `fetch/metadata/generated/script-text-module-import-static.https.sub.html`(ini 文件级 `expected: ERROR`,上游 servo 在此文件 ERROR;bao 跑通 → OK)
- 证据: `/home/putao/code/tools/servo/tests/wpt/meta/fetch/metadata/generated/script-text-module-import-static.https.sub.html.ini`
- 归因域: 期望文件漂移(正向,产品能力超出 servo 基线)——crash-ledger L4/L5 同类
- 处置: 归既有漂移桶(吸收波 ini 维护登记)

### H7 · https 面 subtest ×34 — Sec-Fetch-* 元数据断言族(期望漂移 + 缺陷混合)

- 复现面: `sec-fetch-{site,mode,dest,user,storage-access}` 子测试族,ini 期望大量 FAIL(上游 servo 不发/发错 Sec-Fetch-* 头),bao 部分发出(部分 PASS=正向漂移、部分 FAIL=头值缺口)
- 归因域: 混合——PASS 侧=正向期望漂移(L4/L5 类);FAIL 侧=Sec-Fetch 头值实现缺口(缺陷候选,fetch metadata 面细化)
- 处置建议: 拆两半登记:正向漂移归吸收波 ini;FAIL 侧列 fetch-metadata 头值 backlog

### C · css 面(全量 33502 tests,61 分钟,proc8 静默窗)

- 复现命令: `/tmp/bao-wpt/load-gated-css-full.sh`(负载门控 load1<10,proc8,`css` 全树)
- 证据: `/tmp/bao-wpt/css-full-e26.log`(36MB)— 33502 tests / 26661 expected(+8 skipped)/ 4 CRASH / 443 ERROR / 3978 FAIL / 333 TIMEOUT / 417 unexp-PASS / 18 unexp-OK / 1757 subtest
- 负载判定: 全程 load ≤ 4.94(20 核),无 e16 类风暴假象条件;CRASH 已单跑复现(见 C1)

### C1 · css 面 CRASH ×4 — 无界递归栈溢出类(真缺陷,4/4 单跑复现)

- 复现命令: `cd /tmp/bao-wpt && venv/bin/python run_bao_wpt_e26.py -- --processes 1 css/css-cascade/scope-deep.html css/css-inline/inline-crash.html css/css-masking/clip-path-svg-content/clip-path-recursion-001.svg css/css-ui/crashtests/outline-scrollIntoView-crash.html`(load 1.62 静默窗)
- 证据: `/tmp/bao-wpt/css-crash-confirm.log` — **4/4 CRASH 复现**,崩因全部为 Rust 栈溢出 abort:3× `thread 'StyleThread#1' has overflowed its stack`(scope-deep / inline-crash / outline-scrollIntoView)+ 1× `thread 'GlobalPool#0' has overflowed its stack`(clip-path-recursion-001.svg)
- 归因域: **缺陷(样式/裁剪/内联递归无界深挖 → 线程栈溢出)**。注意 inline-crash 与 outline-scrollIntoView-crash 是**上游 crashtests**(防回归用),bao 正好复现它们防的缺陷类;scope-deep=@scope 深嵌套,clip-path-recursion=SVG clip-path 递归
- 与 H1 对比: 同为进程死亡,但崩因不同(H1=导航期断连未见 panic 输出,本类=显式 stack overflow)——两类分开立
- 处置建议: 立根治任务(递归深挖栈界:样式树/clip-path/inline 深度限制或迭代化);复现=单跑命令零依赖

### C2 · css 面 ERROR ×~243 — typed-om stylepropertymap 共享 harness TDZ 类(缺陷候选,嫌疑脚本求值面)

- 复现面: `/css/css-typed-om/the-stylepropertymap/properties/*` ≈243 条,全部同形 ERROR
- 证据: `can't access lexical declaration 'gCssWideKeywordsExamples' before initialization`,栈=`runPropertyTests@…/resources/testsuite.js:459:5` ← 各测试 inline script。该 `const` 在 testsuite.js **line 44(文件顶部)**;line 1-43 纯函数声明,单次完整求值不可能 TDZ → **testsuite.js 的顶层 const 初始化未随脚本求值生效**。上游 Chromium/Firefox 243 条全过,文件合法
- 双嫌疑面: ①bao 的 **XDR stencil cache 重放路径**(REQ-ENG-012 patch #7;243 条共享同一 harness URL,首评后缓存命中页若 replay 漏顶层 lexical 初始化即精确产生此错)②DOM classic 外链脚本装载/求值时序。line 1-43 无可抛语句 → 跨脚本 GDI 冲突假设已排除(testhelper.js 无同名声明)
- 归因域: 缺陷候选(脚本求值/缓存),**非特性缺口**(harness 层错误,连测试体都未跑)
- 处置建议: 立根治任务;判据=A/B 关 XDR stencil 缓存跑 5 条样本(绿=①,红=②)

### C3 · css 面 ERROR ×~106 — container-queries 断言类(特性缺口候选)

- 复现面: `/css/css-conditional/container-queries/*` ≈106 条(含 anchor-position/container-queries 子目录)
- 证据样本: `Error: assert_equals: expected 5 but got 0`(at-container-anchored-serialization)— CQ 求值返回空容器集合
- 归因域: container query 特性/求值缺口(样式侧),非环境非 harness
- 处置建议: 特性 backlog;与 stylo 版本面向对照

### C4 · css 面 ERROR ×~34 — typed-om value 子类工厂缺失(特性缺口)

- 证据样本: `CSS.deg is not a function`(cssHWB.html)— `CSS.{deg,…}` CSSUnitValue 工厂未实现
- 处置: Typed OM 特性 backlog(与 C2 分开——C2 是 harness TDZ 未跑到测试体,本桶是测试体真跑到后 API 缺失)

### C5 · css 面 TIMEOUT ×333 — 特性族挂起(view-transitions / clip-path-animations 主导)

- 聚集形态(按目录,`/tmp/bao-wpt/css-full-e26.log`):
  - **css-view-transitions/** ≈210 条(navigation 80 + 主目录 70 + scoped 26+22 + with-types 12)——View Transitions API 未实现,测试等待永不触发的事件 → 挂起到超时(特性缺口,非饥饿:L3 类是单条长测被挤,本类是同机制聚集)
  - **css-masking/clip-path/animations** ≈130 条——clip-path 动画族挂起(动画/合成面缺陷候选)
  - 零散: highlight-api/painting 44、content-visibility 40、responsive-iframe 26、dark-color-scheme 20、scroll-snap/anchoring 24、anchor-position 12、writing-modes 10
- 处置建议: view-transitions 整族登记为未实现特性(勿逐条分诊);clip-path-animations 130 条立动画挂起根治任务

### C6 · css 面 FAIL ×3978 — reftest 像素分歧主面(布局缺陷 backlog 地图)

- 聚集形态: css-writing-modes ≈786、css-multicol ≈346、css-view-transitions ≈216、css-anchor-position ≈191、css-contain ≈188、css-grid ≈360(累计)、css-inline/text-box-trim ≈101、css-masking/mask-image ≈97、css-motion ≈94、css-ruby ≈94、css-display/run-in ≈73、css-viewport/zoom ≈70、其余长尾
- 归因域: 混合——大头是**布局引擎特性/正确性缺口**(writing-modes 垂直排版、multicol、ruby、text-box-trim、run-in 均为布局特性族);其中 **250 行「Screenshot is solid color」**(空白帧类,含 anchor-position refs 群)= 渲染管线未出帧的独立机制桶(见 C7)
- 处置建议: 本桶=后续布局波的选择面(按族批量攻);单条分诊无意义,按目录族立项
- 注: 本面 reftest 主导(~26k reftest),146/147 通过率的 floats 对照组说明通道与基本布局健康,分歧集中在特性族

### C7 · css 面「Screenshot is solid color」×250 行 — 空白帧类(渲染/截图面独立机制)

- 证据: wptrunner FAIL 详情注记 `Screenshot is solid color 0xFFFFFF for <test/ref>`(约 125 条唯一 ×双计);面广:anchor-position 群、motion/offset-path refs 等;另含语义正常的 about:blank
- 归因域: **空白帧类**——page 应有内容但截图全白(渲染管线未绘制该帧)或截图时点早于首帧(泵/composite 时序)。与 #40 watchdog 域相邻,与 C1/C5 无关(独立机制)
- 处置建议: 立探针任务:对 1 条样本 dump 双截图(test 与 ref)+ GL composite phase 面包屑归因(#40 契约:paint:composite),先分「未绘制」vs「截图时点早」

### C8 · css 面 unexp-OK ×18 + unexp-PASS ×417 — 正向期望漂移(登记类)

- unexp-OK ×18: 全部 exp=TIMEOUT(15)/ERROR(4)(css-font-loading、css-grid layout-algorithm、css-scroll-snap snap-events 族)——上游超时/报错,bao 跑通 → **L4/L5 同类正向漂移桶**
- unexp-PASS ×417 与 1757 subtest: 动画/合成插值族为主(transform/opacity interpolation 等,ini 期望 FAIL 而 bao PASS)+ 少量真缺陷;正向面归吸收波 ini 维护,负向面并入 C6
- 处置: 正向漂移登记(不重开缺陷);若某轮翻回即回归信号

## 方法与可重复性

- 载具: `/tmp/bao-wpt/run_bao_wpt_e26.py`(= run_bao_wpt.py 单行换 BAO_BINARY → bao-e26-final)+ `/tmp/bao-wpt/venv` 上游 wptrunner 胶水,零 wptrunner/wptserve 改动
- https 面 include 清单: `/tmp/bao-wpt/https-face-list.txt`(187 条 `fetch` 域 `*.https.*` 路径;wptrunner 解析为 113 个 manifest 测试)
- css 面运行器: `/tmp/bao-wpt/load-gated-css-full.sh`(负载门控 load1<10,proc8,`css` 全树)
- 证书/TLS: wptrunner 自动接线——`run.py:63` 默认 `ca_cert_path=tests/wpt/tests/tools/certs/cacert.pem` → `--certificate-path` + `--ignore-certificate-errors` + hosts 映射 127.0.0.1;wptserve https 于 8443
- 门控: 三面开跑 load1 = 1.70 / 1.13 / 1.40(20 核),全程静默窗(css 面运行期 load ≤ 4.94);4 条 css CRASH 另做 proc1 单跑复现,无 CRASH/TIMEOUT 环境假象嫌疑

## 根治任务队列(按优先级建议)

1. **C1 栈溢出 ×4**(真崩溃,crashtests 防回归面,4/4 可复现)——递归深挖栈界
2. **C2 TDZ ×~243**(harness 层,连测试体都未跑,嫌疑 XDR stencil replay)——A/B 关缓存定位
3. **H1 report.https 导航崩**(进程崩溃,载体 IDN alt-host 子资源页)——二分定位崩点
4. **H5 SW 注册→拦截链 ×~18**(https 面 TIMEOUT 主桶)+ H4 getRegistration 缺口——SW register 流根治
5. **C7 空白帧 ×~125**——渲染/截图时序探针(#40 契约域)
6. **C5 clip-path-animations 挂起 ×~130**(view-transitions 族登记为未实现特性,不逐条分诊)
7. 特性缺口 backlog:C3 container-queries、C4 Typed OM 工厂、H7 Sec-Fetch-* 头值、compression-dictionary
8. 布局 backlog: C6 按目录族立项(writing-modes/multicol/ruby/text-box-trim/run-in/zoom…)
9. 登记类: C8/H6 正向漂移(吸收波 ini)、H3 SharedWorker pref(既有 L2)



## 桶 5(C7 空白帧)解体重分类(2026-10-04,e42 归因终局)

**三假设全否,零真空白帧缺陷**:H1 首帧竞态(probe:nav 返回 t=0 立即截图 6/6 CONTENT,截图三相就绪机+baolatch 零竞态窗)/H2 latch 饥饿(JS 注入 fixed 红盒下一张即纯红,超时串扰 0 触发)/H3 capture-composite 顺序(捕获在 Painter::render 尾部,内容页双视口全捕到)——**不存在独立渲染/截图时序机制**,每条白帧都是截图对当前布局/绘制态的正确捕获。

重分类(125 唯一 test 逐页 live DOM census+截图解码双视口+3 组 A/B 实证):
- 22 条非白 solid(by-design 全色页 0x008000/0x0000FF/0xFFFF00/0x000000)= 良性 note,FAIL 在另侧布局分歧→C6
- 9 条语义正确白(4 about:blank+5 blank/filler ref)+1 selectors 文本预期不可见 = 良性
- 94 条并入特性族:layout-api 45(display:layout() 不布局)/anchor-position 16(anchor-size() 解析期丢弃)/masking 14(**A/B 实证**:无效 clip-path url(#a) 引用整元素不绘,spec 要求忽略引用应显示——masking 家族正确性缺陷并入 C6)/contain-intrinsic-size 5/grid 内在尺寸 2/multicol column-rule 2/view-transitions 2/paint-api 1/shapes 1(shape-outside 未实现+裁剪正确 A/B)/motion 1/ruby 1(1600px 行盒推出 800x600 视口)
- 横切发现:**上游 servo 不渲染滚动条**(window.rs:2547 上游原文 TODO)——凡唯一预期 ink=滚动条的页必白;上游能力缺口非 bao 回归

**立项候选(报用户裁定,不擅自立项)**:a) 上游 servo 滚动条绘制(解锁一批 overflow:scroll reftest);b) masking 无效引用语义(clip-path/mask url(#invalid) 应忽略——14 条+mask-image 家族);c) grid 内在尺寸×overflow。

证据物:/tmp/probe-css-sf/blank-frame/(probe 脚本+PNG+log)+/tmp/solid-tests.json(125 条映射)。floats 哨兵 146/147 零回归,树零改动。

## 桶 3 重定性修正(2026-10-04,e36 归因终局)

**XDR stencil cache 证伪(无罪)**:
- A/B 四格:cache off(磁盘层 disabled)与 on(真实激活,层内落 1 条 .xdr entry)下样本错误文本字节一致
- 结构性证明(更硬,无论 A/B 如何不可能变绿):WPT 页面 classic 脚本走 servo 自有 compile+instantiate(htmlscriptelement.rs:402 create_a_classic_script),`evaluate_script_cached` 全仓唯一消费者=stealth blob 注入(bao_stealth engine_props.rs:1488)——页面脚本与 bao stencil/XDR cache 零交集
- REQ-ENG-012 涉嫌解除,AC01 无违背,xdr_cache/stencil_cache 零改动

**真机制(live 探针 /tmp/tdz-probe/,错误序完整捕获)**:
testsuite.js:44-60 的 `const gCssWideKeywordsExamples = [{ input: new CSSKeywordValue('initial') }, ...]` —— line 44 的 const 初始化表达式本身是可执行代码,bao servo 无 CSSKeywordValue → ReferenceError 于初始化器求值 → 脚本中止于 body 中段(const 绑定已建但槽位 TDZ sentinel)→ 下一 classic 脚本读该 const → spec 正确的 TDZ 报错。台账原前提「line 1-43 纯函数声明→不可能 TDZ」漏了 line 44-60 的可执行初始化器。隔离对照(同页双臂,非抛/抛初始化器)精确复现错误对。

**爆炸半径(横扫修订)**:452 错误块=226 独立文件(原「≈243」为约数),全部 css/css-typed-om/the-stylepropertymap/properties/*。

**桶 3 终定性**:真缺陷候选 → **上游共同特性缺口**(Typed OM value 族:CSSKeywordValue/CSSUnparsedValue/CSSVariableReferenceValue/CSSUnitValue/CSSMathSum/CSSStylePropertyMap 全 MISSING;上游 servo 快照 4842b770e 同缺)→ 归 BRW/dom 特性 backlog。注意:只补 CSSKeywordValue 不够——line 69 `gVarReferenceExamples` 初始化器 `new CSSUnparsedValue(...)` 是下一个中止点,226 条解锁需整个 value 族(需求候选,待用户裁定立项)。
