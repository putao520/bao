# REQ-BRW-004 C19 三子句测试覆盖报告(V 批放行项)

任务:REQ-BRW-004 C19 三子句 live 测试覆盖(V 批 v47 报告③节判定零覆盖)。
产物:`src/bao_browser/tests/suite/sw_stealth_profile_tests.rs`(新文件,3 个 live 测试)
+ `src/bao_browser/tests/suite/main.rs`(+1 行 `mod sw_stealth_profile_tests;`)。
**零产品源码改动**。运行:`BAO_TEST_NETWORK=1 xvfb-run -a cargo nt -p bao-browser -E 'test(sw_stealth_profile)'`。

## 判定总表(最终树合并跑,真退出码)

| 子句 | 测试 | 判定 | 耗时 |
|---|---|---|---|
| ① SW 转发 fetch 同 stealth TLS(JA3/JA4)+H2(AKAMAI) profile | `c19_sub1_sw_forwarded_fetch_rides_page_tls_h2_profile_live` | **GREEN** | 2.18s |
| ② CDP Network 域可观测 SW 拦截的请求/响应 | `c19_sub2_cdp_network_observability_of_sw_intercepted_fetch_live` | **RED(如实,缺口指认见下)** | 92.9s |
| ③ 跨页存活 + profile 继承注册页 + terminate 注销 | `c19_sub3_sw_cross_page_inheritance_and_terminate_deregistration_live` | **GREEN** | 3.11s |

## 子句① GREEN — 证据

测法:raw-TCP ClientHello 抓取服务器(复用 page_net_bun_fingerprint_e2e 车辆形态)。
SW 拦截 `/api/forward` 并 `respondWith(fetch('https://127.0.0.1:<sw_capture>/sw_forwarded'))`
(SW realm 自发的 fetch);页面自身 `<img>` 子资源到第二台抓取服务器作主页路径基线。
抓取服务器不回 TLS 握手——SW 子 fetch 的 settlement 可能 wedge(已知 engine 缺陷),
但 ClientHello 已上线,egress 即断言面(mediation 测试③的"egress 非 settlement"纪律)。

实测(medium 输出):
- 双路 ClientHello 抓取成功,**canonical bytes 逐字节相等**(client_random/session_id/key_share
  归零后:cipher 列表+顺序、curves、sigalgs、extension 列表+顺序+payload 含 ALPN 全同);
- **JA3 相等**,实测值(两路相同):
  `771,4865-4866-4867-49195-49199-49196-49200-52393-52392-49171-49161,23-65281-10-11-35-16-5-13-18-51-45-43,8-29-23-24-25,20-1027-2052-1025-1283-2053-1281-2054-1537-515-513`
  (Firefox stealth profile 形状);
- **ALPN 双路均 `h2,http/1.1`**(无 h1 降级绕过);
- **H2(AKAMAI)侧**:`bao_stealth::global_http2_fingerprint()` 快照 === 页面 profile 的
  `pseudo_header_order` 与 preface PRIORITY 帧数(两栈 h2 编码共同读取的单源)。

结论:e33 侦察⑥的「同 resource_threads 自动继承,零新增接线」判断**实测成立**——
SW 转发 fetch 未绕过反指纹 TLS/H2 profile。

## 子句② RED — 缺口 + 指认层

测法:live CdpServer(生产 BaoWsRegistry 接线,同 cdp_ws_command_face_tests)+ WS client
`Network.enable`,页面注册 SW(拦截 `/api/data` → respondWith 201)并驱动中介 fetch。
live 前置全绿:`register ok`;中介 fetch 以 **201 + CDP_OBSERVABLE_BODY** 到达页面
(即观测对象真实发生);`Network.enable` 在 WS 面 ACK。

缺口:收集窗口(75s + 10s grace)内 **0 事件**——无 `Network.requestWillBeSent`、
无 `Network.responseReceived`。SPEC 子句不成立。

指认层(代码事实,非猜测):
1. `src/bao_browser/src/cdp_handler.rs:156-157` — `BridgeCommand::NetworkEnable { .. } => ok_empty()`
   空转应答,**不安装任何 net 层事件 tap**;
2. 产品代码中**不存在任何 `ServoEvent::NetworkRequest/NetworkResponse` 的构造点**
   (仅 bao_cdp_client EventSubscriber API + mock 后端测试;cdp-server 的
   `__BAO_EVT__Network.*` console 解析通道无任何 JS 侧发射器);
3. `src/bao_browser/src/delegate.rs:4261` `forward_service_worker_fetch_event`
   (注释明确写 C19「CDP Network 域可观测 SW 发起的请求/响应」)**零调用方**,且即使接通
   也只发 Console 事件而非 Network 域事件。

附注(对照实验):页面 `console.log('c19-sub2-channel-marker')` 的 Log.entryAdded
亦未到达(0 事件)——本 harness 下 delegate→EventSubscriber→translate→broadcaster→WS
通道对 console 亦未送达,故②的 RED 指认主要锚定上述代码层事实(发射器根本不存在),
而非依赖通道对照。修复波需同时回答:net 层 tap 装在哪(bridge/http_loader)、事件
走 EventSubscriber 还是 __BAO_EVT__ 通道、以及 console 通道在本 harness 的送达问题。

## 子句③ GREEN — 证据

测法:SW 拦截 `/api/inherit` 回 `respondWith(new Response('SWUA=' + navigator.userAgent, {status:201}))`
——UA 在 **SW realm 内 fetch 处理时**读取,中介响应体即 SW realm stealth profile 的 live 探针。

- **继承**(1):page1(Firefox profile)中介 201,body =
  `SWUA=Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0`,
  与注册页 profile UA **精确相等**(断言 `sw_ua == page1_ua`)——S1 的 SW scope drain
  (serviceworkerglobalscope.rs → drain_worker_scope_callbacks)live 生效;
- **跨页存活**(2):同 origin 第二页面(`/page2`,同 runtime)同样被**同一存活 worker**
  中介为 201 且携带同一继承 UA;
- **terminate 注销**(3):`getRegistration().unregister()` → `un:true`(vendor terminate
  路径,manager 在 resolve 前终止 worker 线程);之后 `/api/inherit` 探针返回
  **原生 `status=200 body=NATIVE_INHERIT_BODY`**,SWUA 不再出现——中介已注销。

### 子句③边界披露(非本波可测)
- REALM_PROFILES 条目级注销(SW realm global addr 的 `remove_profile_for_global`)
  无公开可观测面:`engine_props::REALM_PROFILES` 私有,`canvas_seed_for_test` 需已知
  global addr,而 SW realm 的 global addr 未跨线程暴露给 embedder(DedicatedWorker 有
  `worker_global_addr` 槽,SW 无对应)。本测试断言的是**中介层注销**(SPEC 可观测语义);
  条目级注销需产品侧暴露测试锚点后方可补测(修复波可选项)。
- 架构风险(未测,记录):SW scope 只 drain **consume-once 层**
  (serviceworkerglobalscope.rs:461 → drain_worker_scope_callbacks),不含
  DedicatedWorker 的非消费 injector 层(dedicatedworkerglobalscope.rs:627)。
  若注册页先创建过 DedicatedWorker(耗尽 one-shot 队列),后注册的 SW scope 将零注入。
  本测试用干净时序(无前置 Worker)绕开;该顺序脆弱性属产品层后续裁定项。

## 过程记录

- 工作区并行事故:子句②首次 nextest 触发时,并行 agent 在途编辑
  `vendor/servo/components/net/http_loader.rs`(S2b netintercept 波)短暂打断编译
  (`request.url.url()` E0615)。按共享脏工作区纪律未触碰他人文件,先以最后一次
  成功构建的 suite 二进制直跑取证,后轮询至对方编辑落定(约 2 分钟)重建复跑。
  本任务文件归属:`src/bao_browser/tests/suite/sw_stealth_profile_tests.rs`(新)、
  `main.rs`(+1 行)。
- 复跑稳定性:三测试在最终树合并跑(`--no-fail-fast`)中 5 PASS / 1 FAIL(= sub2 如实红)。
