# BRW-004 波末 V 批验收报告

- 验收角色:独立 V(tester),HEAD = `5627cf8e`(2026-09-09 22:57:22 +0800),工作树 clean(除 vendor/servo/Cargo.lock 与他人 untracked 报告)。
- 测试范式:`BAO_TEST_NETWORK=1 xvfb-run -a cargo nextest run --cargo-profile test-ci -p <crate> [-E filterset] [--no-fail-fast]`,退出码取 PIPESTATUS(波末三重判据纪律)。
- 本报告为本次验收唯一落盘产物;未改任何产品源码/vendor。

---

## ① 全族回归矩阵

| 套件 | 范围 | 绿 | 红 | 退出码 | 备注 |
|---|---|---|---|---|---|
| bao-browser | 全套 --no-fail-fast | 1793 | **2** | 100 | Summary: 1795 run, 1793 passed (1 slow), 2 failed, 0 skipped |
| bao_stealth | 全量 | 1685 | 0 | 0 | 1685/1685 全绿 |
| bun_runtime(补充,scoped) | node_worker_structured_clone + node_silent_fake_eradication | 19 | 0 | 0 | node `worker_threads` 面,非浏览器 Web Worker 面 |

### 族级明细(bao-browser 全套内提取)

| 族 | 绿/总数 | 红 |
|---|---|---|
| worker_tests(单元+通道+AutoCloseWorker+REALM_PROFILES) | 74/74 | 0 |
| worker_concurrent_servo_tests(C18) | 8/8 | 0 |
| worker_fingerprint_consistency_tests(C12/C16/C17) | 9/9 | 0 |
| worker_multi_injection_tests(per-Worker 注入) | 5/5 | 0 |
| worker_onerror_integration_tests(C9) | 10/10 | 0 |
| serviceworker_mediation_tests(C19 mechanics) | 1/1 | 0 |
| **serviceworker_fetchevent_tests** | **0/1** | **1(确定性红,见②③④)** |
| serviceworker_controller_tests(C19 controller) | 1/1 | 0 |
| stealth_offscreencanvas_tests(C13/C14) | 15/15 | 0 |
| stealth_worker_audio_tests(C15) | 7/7 | 0 |
| fetch_axis_probe_tests(fetch 三轴) | 7/7 | 0 |
| pagepool_chaos_memory_safety_tests(C18/内存安全) | 3/3 | 0 | 
| realworld 族 | **anti_scraping 0/1 SIGSEGV;full_stack/automation 绿** | **1(确定性 SIGSEGV,见④)** |

---

## ② 重点定案三件

### (a) fingerprint_website_eval_e2e — **GREEN(终态定案)**

- HEAD 单测:PASS 33.066s,退出码 0。
- e37 在途快照看到的 `:351` 栈是树内 churn 期的瞬态;终态 HEAD 干净通过。无需 gdb。

### (b) pagepool_chaos_memory_safety — **GREEN(修后首通复验成立)**

- Scoped 单测:PASS 166.879s(slow),退出码 0,族 3/3。
- 全套内同测再次 PASS。e37 自报的 AutoRealm 修(60fb645c validate node-realm context identity)在独立复验下成立。

### (c) c19_sw_*(实为 3 测)+ 双跑定案

| 测试 | 跑1 | 跑2 | 定案 |
|---|---|---|---|
| c19_sw_mediates_page_fetch_end_to_end_live | PASS | PASS | 稳定绿 |
| c19_sw_controller_assignment_live | PASS | PASS | 稳定绿 |
| **c19_sw_realm_exposes_fetchevent_pipeline_live** | **FAIL** | **FAIL** | **确定性红,非 flaky(退出码 100)** |

- fetchevent 失败形态(register 现已 settle——「register outcome = ok」,与 e45 所见不同):SW 探针 verdict 永不到达,fixture 只记录到 `["/", "/sw.js"]`;SW 线程 panic 于 `vendor/servo/components/script/dom/xmlhttprequest/xmlhttprequest.rs:1640:61` — sync XHR 事件泵 `script_port.recv().unwrap()` 收到 `Err(())`(通道对端被 drop 且无 settle task)。该 sync XHR 正是探针的主发布通道(SW-realm async fetch 亦引擎级不 settle,见 mediation 头注释)。
- 任务提到的 "known-transient 注释" 不在测试文件里,而在 commit `71c90281` message 内:S2a/S2b 落地窗口因 e37/e43 churn 将 fetchevent 面测与 worker 族横扫 parked,从未验证绿。

---

## ③ SPEC criterion 矩阵(尺 = `.spec/10-REQUIREMENTS.html` REQ-BRW-004,共 19 条,逐条独立核对,不以 E 自报为准)

| # | criterion(SPEC 原文摘要) | 判定 | 证据(测试 + commit) |
|---|---|---|---|
| C1 | new Worker(url) 创建线程并执行脚本 | **green** | 全 worker 族 live 均以 `new Worker("data:text/javascript,…")` 创建并取回执行产物:onerror 族 10/10、fingerprint 族 9/9、offscreencanvas 15/15、audio 7/7(全套 1793 绿含全部) |
| C2 | worker.postMessage(msg) 页→Worker,Worker onmessage 接收 | **red(覆盖缺口)** | 仅有单元级 `worker_tests::test_worker_channel_bridge_page_to_worker`(Rust 通道结构);全 suite `command grep` 终审:browser-realm live `w.postMessage()` **零断言**(所有 live 流量均为 worker→page 字符串)。缺口系历史存量,非本波引入 |
| C3 | self.postMessage(msg) Worker→页 | **green** | 所有 live worker verdict 经 `w.onmessage` 到达(fingerprint/canvas/audio/onerror 全绿) |
| C4 | worker.terminate() closing 标志 | **green** | 单元 `test_worker_handle_terminate_sets_closing`/`_idempotent` + live `c18_three_path_teardown_crash_free`(8/8) |
| C5 | self.close() | **green** | 单元 `test_worker_handle_mark_terminated` + live 三路径 teardown(c18) |
| C6 | Structured Clone(对象/数组/Buffer/ArrayBuffer/Transferable) | **red(部分缺口)** | 单元 payload/transferable 结构绿;node `worker_threads` 面 19/19 绿(bun_runtime,不同 API 面);**browser-realm live 结构化克隆交换零断言**(live 只传字符串)。历史存量缺口 |
| C7 | Worker 独立 SM Runtime+JSContext(协作 GC) | **green(附注)** | 独立 Runtime/Context 由 live worker 执行架构性证明;`c18_concurrent_create_destroy_zero_crash` 高频建销(GC 压力)零崩溃。附注:无专用"协作 GC"断言测试 |
| C8 | DedicatedWorkerGlobalScope 完整 API(self/close/importScripts/setTimeout/fetch/crypto/performance/location/navigator) | **red(缺口)** | 已覆盖:self/close(C18 live)、worker-realm fetch(`fetch_axis_probe_c_worker_realm` 绿)、worker-realm 定时器(c18 `setInterval` live)、navigator(C12)。**零覆盖:importScripts(全 suite grep 零命中)、crypto、performance、location(worker-realm)**。历史存量缺口 |
| C9 | onerror ErrorEvent 四字段 | **green** | `servo_native_onerror_error_event_fields`(4 字段断言)等 10/10 |
| C10 | 页面卸载自动终止(track_worker+AutoCloseWorker) | **green** | 单元 AutoCloseWorker×4 + live 三路径 teardown(unload 路径) |
| C11 | bun_sm WebWorker stub 删除 | **green** | `src/bun_sm/src/web_worker.rs` 不存在(删于 `6911eda9` DEC-WK-001,静态核验) |
| C12 | navigator 一致(ua/platform/hwConcurrency/language(s)) | **green** | `c12_worker_navigator_matches_main_thread_per_field`(9/9 族);commit `5b5876b2`(languages 真 Array) |
| C13 | OffscreenCanvas 噪声同 profile(同 seed 同结果) | **green** | `stealth_offscreencanvas_tests` 15/15(2d 管线/window 暴露/retirement parity×2);commits `4b394420`(pref flip)/`6bcf30af`(paint-thread readback)/`c833f77c`(JS hook 退役,单一噪声咽喉) |
| C14 | WebGL UNMASKED_VENDOR/RENDERER 及参数一致 | **green** | webgl1/webgl2 管线+getShaderPrecisionFormat+window 对照全绿;commits `18e6f876`/`98532c0c`/`ae306baa`(getParameter 单注入根治) |
| C15 | AudioContext 指纹噪声同 profile | **green** | `stealth_worker_audio_tests` 7/7(OfflineAudioContext full stack/跨 realm 一致/种子分化/window 零回归);commit `fa084a64` |
| C16 | 无原生泄漏(vs 无 stealth 基线) | **green** | `c16_worker_surface_differs_from_no_stealth_baseline` + multi_injection 5/5(per-Worker 全注入;commit `5627cf8e` tier) |
| C17 | 跨线程摘要一致(CreepJS/sannysoft 式) | **green** | `c17_worker_fingerprint_digest_equals_main_thread_digest` + `fingerprint_website_eval_e2e`(真实指纹站)HEAD 双绿 |
| C18 | 三路径 teardown crash-safe + 并发建销零崩溃零泄漏 | **green** | `worker_concurrent_servo_tests` 8/8 + REALM_PROFILES 注销单元 + `pagepool_chaos_memory_safety` PASS(166.9s);commits `1d75200c`(注销死路径)/`60fb645c`(registry 身份校验)/`9f5f4692`(gc_store GC 根) |
| C19 | SW 拦截 × stealth/CDP 边界一致(stealth TLS+H2 profile 继承/CDP Network 可观测 SW 请求/跨页存活继承+terminate 注销) | **red(子句缺口 + 载具红)** | 机制面绿:`c19_sw_mediates_page_fetch_end_to_end_live`(respondWith 定制响应/pass-through/ServiceWorkersMode::None 防环)+ `c19_sw_controller_assignment_live`(controller 链 4 探针);commits `57ccec60`/`fc726515`/`eff5781d`/`71c90281`/`dffa8f4e`。**但 SPEC 三个子句零测试覆盖**:①SW 转发 fetch 与主页同 JA3/JA4+AKAMAI profile(SW 测试内 ja3/akamai/tls 断言 grep=0);②CDP Network 域观测 SW 请求(=0);③跨页存活 profile 继承+terminate 注销(=0)。另 FetchEvent 面自查载具 `c19_sw_realm_exposes_fetchevent_pipeline_live` 确定性红(见②④) |

**计:green 14(C1/C3/C4/C5/C7*/C9/C10/C11/C12–C18)· red 4(C2/C6/C8/C19)。**C7 附注无专用断言。

---

## ④ 红项归属定案

### R1 `serviceworker_fetchevent_tests::c19_sw_realm_exposes_fetchevent_pipeline_live`

- **定性:确定性红(3/3 同签名),非 flaky;引入即红,非从绿转红的回归。**
- 归属链:测试由本波 S2a `57ccec60` 引入 → e38 un-ignore → `71c90281` 接手并在 commit message 明确 parked(known-transient,e37/e43 churn 窗口)→ **从未在任何 commit 验证绿**。
- 技术根:SW-realm verdict 出口双断——async fetch 引擎级不 settle(mediation 头注释:无 S2b 文件基线同样复现,FetchThread/NetworkListener 回程 wedge,pre-existing);退而用的 sync XHR 又在 `xmlhttprequest.rs:1640` panic(`script_port.recv().unwrap()` Err;`fc726515` 给 SW 补了 `new_script_pair` 第三臂,但该通道 sender 无 settle task 即被 drop 的生命周期未处理)。
- 类别:**本波遗留红(engine 级 SW-realm egress 缺口)**,阻断 C19 面自查,不阻断已绿的 mediation/controller 机制证据。

### R2 `realworld_anti_scraping_e2e_tests::realworld_anti_scraping_e2e` — **SIGSEGV(P0)**

- **定性:确定性 SIGSEGV(3/3,~5s,signal 11),非 flaky。**全套内 1788/1795 处崩,scoped 双跑同崩,退出码 100。
- 崩溃点:线程 **`Script#2`**(第 2 个 servo ScriptThread;该测试经 PagePool 连建 5 页)。test-ci(stripped)栈不可符号化(gdb 全 `??`)。
- **Profile 差分(关键证据)**:同源 dev 二进制(`debug/deps/suite-4d26bcf84df7c353`,22:48 构建;mtime 审计证实已含 `5627cf8e` 全部代码——最晚代码文件 22:41)**同测 PASS 35.99s**;test-ci(opt-2)3/3 崩 → **仅优化档显现的内存安全故障(时序/GC/生命周期类 UB 候选)**。
- **归属:无法定案 本波回归 vs pre-existing**——本波前无 test-ci 全套基线(本波报告与历史会话均无该测近期结果;最近 realworld 全绿记录 = 2026-08-15,相隔 suite 收敛与多波)。波内 5627cf8e 恰好触碰 page_pool/page/lib(多页+ScriptThread 领域),60fb645c/9f5f4692 修的正是相邻 ScriptThread 崩溃类——嫌疑存在但无基线可证。
- **后续必要动作(P0)**:以 opt-level 2 + debug=2 + 不 strip 的专用构建复现并符号化栈(gdb/dev-profile 不复现,无法直接抓)。

### 附:bao_stealth 1685/1685、bun_runtime 补充 19/19 零红,无归属事项。

---

## ⑤ 结论:REQ-BRW-004 是否达到 implemented

**未达到 implemented。**

- **green 14/19**(C1/C3/C4/C5/C7/C9/C10/C11/C12/C13/C14/C15/C16/C17/C18——本波主战场 C12–C18 worker stealth 一致性全绿,且三件定案(a)(b) 双绿、C19 机制面两测绿)。
- **red 4/19**:
  - C2/C6/C8:browser-realm live 覆盖缺口(postMessage 页→Worker / 结构化克隆 live / importScripts+crypto+performance+location),均历史存量、非本波引入,但按 SPEC 文本为尺不满足 green;
  - C19:SPEC 明文的 stealth profile 等价、CDP Network 可观测、跨页生命周期继承/注销三子句零测试覆盖 + FetchEvent 面自查载具确定性红。
- 波内质量:**本波 24 commits 无一把既有绿测打红**(两红均为"引入即红/归属未定",非绿→红回归);但 R2 anti_scraping SIGSEGV(P0,profile 相关)在本波多页/ScriptThread 改动邻域,须先定案后关波。

**放行条件(达到 implemented 的最小差集)**:C19 三子句落测试(或用户书面豁免)+ fetchevent 载具出口修复/换通道 + R2 SIGSEGV 符号化定案与根治 + C2/C6/C8 缺口补测或豁免裁决。
