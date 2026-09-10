# BRW-004 战役终态认证报告(独立 V)

- 验收角色:独立 V(tester 终验)。**认证 HEAD = `4771b599`**(2026-09-10 08:45 +0800)。
- 基线:V47 报告(`.claude/prompts/brw004-v-batch-report.md`,HEAD=5627cf8e,1795 run / 1793 绿 / 2 红)。V47 后共 **14 个 commit**(任务考古点名 5 个代码 commit;实际窗口另含 fe6298e1/d2db4c55/a6870f0e/586d355f/2a7af156/23d26731/642da220 等——其中 **d2db4c55 即 V47 R2 SIGSEGV 的根治 commit**,fe6298e1 为 R1 部分修)。
- 测试范式(与 V47 一致):`BAO_TEST_NETWORK=1 xvfb-run -a cargo nextest run --cargo-profile test-ci -p <crate> [--no-fail-fast]`,共享 target `/var/cargo-builds/3c/6184ceb77072ba`,退出码直采(无管道截断;波末三重判据纪律)。
- 工作树:clean(除 `vendor/servo/Cargo.lock` 已发布 crate 版本号 bump 残留,与 V47 同形态;另他人 untracked 报告文件)。本认证零源码改动;仅落盘本报告。

---

## ① 终态全族回归矩阵

| 套件 | 范围 | 绿 | 红 | 退出码 | nextest run ID | Summary 原文 |
|---|---|---|---|---|---|---|
| bao-browser | 全套 --no-fail-fast | **1815** | **2** | **100** | 98b4d7d4 | `1817 tests run: 1815 passed (1 slow), 2 failed, 0 skipped` [972.127s] |
| bao_stealth | 全量 | 1685 | 0 | 0 | f8b47260 | `1685 tests run: 1685 passed, 0 skipped` |
| bun_runtime | lib | 657 | 0 | 0 | 88adc944 | `657 tests run: 657 passed, 0 skipped` |
| cdp-server | 全量 | 1310 | 0 | 0 | e84fdd51 | `1310 tests run: 1310 passed, 0 skipped` |
| bun_uws | 全量 | 35 | 0 | 0 | — | `35 tests run: 35 passed, 0 skipped`(UWS_EXIT=0) |

**②③④ 两红的确定性双跑定案(V47 双跑纪律)**

| 测试 | 全套内 | 隔离跑1 | 隔离跑2 | 定案 |
|---|---|---|---|---|
| `page_net_bun_full_matrix_e2e_tests::page_net_bun_full_destination_matrix` | FAIL 62.952s | **PASS 16.761s**(0437c7fa) | **PASS 16.668s**(95d8cd73) | **fleet 条件 flake(1/3),非确定性回归**(见④-2) |
| `realworld_anti_scraping_e2e_tests::realworld_anti_scraping_e2e` | 948.088s 忙旋→V 侧 SIGTERM 定点放行 | timeout 500s 兜底 SIGTERM(exit 124,c1148bad) | 同左(同签名) | **确定性 wedge 2/2 + Script#3 panic 2/2**(见④-1) |

> anti_scraping 处置说明:nextest 无 per-test 超时,该测 118–163% CPU 忙旋永不自主终结;为放行套件,V 对**该测试进程单独** SIGTERM(r2 由脚本 `timeout 500` 兜底),未触碰 nextest/链。网络出口旁证正常(58.com HTTP/2 302 可达),wedge 为进程内缺陷。

---

## ② 族级明细(bao-browser 全套内提取;计数含各文件 `common::tests` 3 子测,V47 同口径)

| 族 | 本认证 | V47 | 变化 |
|---|---|---|---|
| worker_tests(单元+通道+AutoClose+REALM_PROFILES) | 74/74 | 74/74 | 持平绿 |
| worker_concurrent_servo_tests(C18) | 8/8 | 8/8 | 持平绿 |
| worker_fingerprint_consistency_tests(C12/C16/C17) | 9/9 | 9/9 | 持平绿 |
| worker_multi_injection_tests | 5/5 | 5/5 | 持平绿 |
| worker_onerror_integration_tests(C9) | 10/10 | 10/10 | 持平绿 |
| **worker_realm_api_tests(新,C2/C6/C8)** | **7/7**(4 真测+3 common) | — | **新增全绿**(23d26731) |
| **sw_stealth_profile_tests(新,C19 三子句+injector)** | **7/7**(4 真测+3 common) | — | **新增全绿**(2a7af156+baaf7db7) |
| **shadow_axis_probe_tests(新)** | **6/6**(3 真测+3 common) | — | **新增全绿**(439f4ec1/4771b599) |
| serviceworker_mediation_tests(C19 mechanics) | 1/1 | 1/1 | 持平绿 |
| **serviceworker_fetchevent_tests** | **1/1(1.198s)** | **0/1 红** | **红→绿**(fe6298e1+586d355f+vendor sync-XHR fail-closed) |
| serviceworker_controller_tests(C19 controller) | 1/1 | 1/1 | 持平绿 |
| stealth_offscreencanvas_tests(C13/C14) | 15/15 | 15/15 | 持平绿 |
| stealth_worker_audio_tests(C15) | 7/7 | 7/7 | 持平绿(016747b1 内部重塑,面数不变) |
| fetch_axis_probe_tests | 9/9(6 真测+3 common) | 7/7(4+3) | +2 新测绿(fe6298e1 urlshape/query) |
| pagepool_chaos_memory_safety_tests | 3/3(PASS 168.185s,slow) | 3/3 | 持平绿 |
| fingerprint_website_eval_e2e | 1/1 | 1/1 | 持平绿(V47 定案(a) 维持) |
| h2_fetch_node_stack_e2e_tests | 7/7 | 绿 | 持平绿 |
| page_net_bun_full_matrix_e2e_tests | 4 测:3 common 绿 + **destination_matrix 红(仅全套内)** | 绿 | **绿→红(flake 类,隔离 2/2 绿)** |
| page_net_bun_{fingerprint,streaming_upload} | 4/4、4/4 | 绿 | 持平绿 |
| realworld_full_stack / browser_automation | 1/1、1/1 | 绿 | 持平绿 |
| **realworld_anti_scraping_e2e** | **wedge 红(内部断言 0 failed)** | **SIGSEGV 红** | **红→红,签名更替**(SIGSEGV 已根治,新签名见④-1) |
| 其余全家(permission/screenshot/config/evaluate/rendering/…) | 全绿 | 全绿 | 持平 |

---

## ③ 与 V47 矩阵对比:总账精确闭合

**1795 + 22 = 1817(零删测、零 ignore 翻转,双向账目闭合)**

- +22 = **13 个新真测**(worker_realm_api 4 + sw_stealth_profile 4 + shadow_axis 3 + fetch_axis 2;git diff 5627cf8e..HEAD 逐文件核销)+ **9 个 common 子测副本**(3 个新族文件各 `mod common` 引入 3 个 `test_wait_for_condition_*`;V47 的族计数含 common 子测,同口径)。
- lib 侧 579/579 两版完全一致(git grep 双向 diff 零增零删)——V47 报告 1795 与树重建 1804 的 9 差即 common 副本口径,已闭合,无 V47 侧记账问题。
- 绿数 +22 精确分解:**22 新测全绿 + fetchevent 红转绿(+1)− page_net destination_matrix 绿转红(−1)= 1815**。
- **绿→红仅 1 处**(page_net destination_matrix,定性 flake 见④-2);**红→绿 1 处**(fetchevent);**红→红签名更替 1 处**(anti_scraping);其余 1792 处逐族持平绿。

**逐新增测试归属**(全部绿,无需定性红):

| 测试 | commit | 关闭面 |
|---|---|---|
| worker_realm_api_c2_page_to_worker_postmessage_echo | 23d26731 | C2 |
| worker_realm_api_c6_structured_clone_roundtrip_five_types / c6_import_scripts_helper_globals | 23d26731 | C6/C8 |
| worker_realm_api_c8_crypto_performance_location | 23d26731 | C8 |
| c19_sub1_sw_forwarded_fetch_rides_page_tls_h2_profile_live | 2a7af156 | C19-① |
| c19_sub2_cdp_network_observability_of_sw_intercepted_fetch_live | 2a7af156 | C19-②(PASS 12.995s) |
| c19_sub3_sw_cross_page_inheritance_and_terminate_deregistration_live | 2a7af156 | C19-③ |
| sw_scope_injector_starvation_after_dedicated_worker_live | baaf7db7 | SW 注入层 |
| shadow_axis_probe_i_set_immediate_page_realm | 439f4ec1 | BCE-20260910-003 |
| shadow_axis_probe_q_queue_microtask_order / c_crypto_page_realm | 4771b599 | C16 shadow |
| fetch_axis_probe_urlshape_double_slash / query_string_xhr | fe6298e1 | fetch 轴 |

V47 五项放行条件的兑现核对:C2/C6/C8 落测 ✓(全绿);C19 三子句落测 ✓(sub1/2/3 全绿);fetchevent 载具出口修复 ✓(绿);R2 SIGSEGV 符号化定案与根治 ✓(d2db4c55,本认证 2/2 无 SIGSEGV 实证);唯 anti_scraping 换新签名红(④-1)。

---

## ④ 红项归属定案

### 1. `realworld_anti_scraping_e2e` — **确定性 wedge + Script#3 panic(P0,2/2 同签名)**

- **V47 R2 的 SIGSEGV(Script#2 @~5s,3/3)已被 d2db4c55 根治**:本认证 2/2 跑均越过原崩溃点(238+ evaluates、三站全部走完、无 signal 11)。
- **新签名(2/2 确定性)**,三段式:
  1. `thread 'Script#3' panicked at vendor/servo/components/script/dom/bindings/settings_stack.rs:36:26: called Option::unwrap() on a None value`——即 `entry_global()` 在 settings/execution 栈为空时被调(meituan 阶段;test-ci stripped 不可再符号化);
  2. 该 panic 毒化 meituan 页 JS 求值:内部 `navigate_meituan::page_content` / `ua_persists` 两检查 SKIP(`evaluate_js: javascript error: InternalError`);
  3. **内部断言全部完成后**(内部汇总 `--- 17 passed, 2 skipped, 0 failed ---`)teardown 段忙旋(118–163% CPU)r1 948s/V 侧 SIGTERM、r2 timeout 500s 兜底——**进程永不自主终结**。
- **归属:post-V47 窗口暴露的新缺陷**(V47 两档均未见过此 panic:dev 档同测 35.99s 干净通过且无 panic、test-ci 档 5s 即崩根本走不到 meituan;d2db4c55 让执行越过原崩溃点后,更深的 settings-stack 缺陷显形)。机制与嫌疑 commit 类(entry_global 空栈 = 回调在未被 push 上 settings 栈的 realm 上派发;候选:439f4ec1 raw-rooted global + AutoRealm dispatch / 4771b599 queueMicrotask / d2db4c55 node-realm rooting;需 BCE 归因,test-ci-dbg 档可符号化)。
- 附带观察(非阻断):stderr 反复出现 `[node_timers_module] promisify-custom wiring call failed: g.setTimeout is undefined`(raw-rooted global 上无 setTimeout——与候选机制同域,归因时一并查)。
- **性质:非 BRW-004 criterion 载体红**(该测内部断言 0 failed;红在进程生命周期),但属 P0 工程缺陷:真实站点页面 panic + 测试永不终结 + CPU 忙旋。

### 2. `page_net_bun_full_destination_matrix` — **fleet 条件 flake(P1,1/3;唯一绿→红,非确定性)**

- 全套内 FAIL 62.952s:`h2 document never arrived (streams: [], alpn-h2 conns: 1, non-h2: 0)`(page_net_bun_full_matrix_e2e_tests.rs:607 panic;ALPN h2 握手完成但零 stream 开出);隔离双跑 **2/2 PASS(~16.7s,4× deadline 余量)**。
- 与 `.config/nextest.toml` 已立案隔离的 `h2_fetch_node_stack_e2e_tests` 饥饿 flake 同分类学(20 路并发 fleet 下 deadline-bound h2 往返被饿);全套 fleet V47 时 22 测更少、V47 全套该测绿,post-V47 HTTP Client tick(a6870f0e)/拦截器(586d355f)改动可能收窄了时序余量——**无法二值定案引入 vs 幸存偏差**,按 flake 挂账。
- 建议同机制处置(threads-required 隔离或 deadline 放宽)——留主会话裁决,本认证不动配置。

### 3. bao_stealth 1685/1685、bun_runtime lib 657/657、cdp-server 1310/1310、bun_uws 35/35 全绿,零归属事项。

---

## ⑤ 终态判词

**BRW-004 战役终态 HEAD(4771b599)不是全绿:1815/1817(退出码 100),两红全部归属+定性完毕。**

- **作为 BRW-004 战役认证:19 条 criterion 载体在 HEAD 全绿**——V47 的 4 红(C2/C6/C8/C19)已由 23d26731/2a7af156 落测全数关闭并绿,fetchevent 载具转绿,C12–C18 主战场族全数持平绿;战役层面与 `642da220`(SPEC → implemented)的判定相容。
- **作为终态 HEAD 认证:不可宣告全绿**,阻断项两条:
  1. **P0 新缺陷**(post-V47 窗口暴露):servo `settings_stack.rs:36 entry_global()` 空栈 panic(Script#3,meituan)+ anti_scraping teardown 忙旋永不终结——真实站点可用性 + 测试基建双重影响,须走 BCE(建议 test-ci-dbg 档符号化 + 439f4ec1/4771b599/d2db4c55 嫌疑链归因);
  2. **P1 flake**:page_net destination_matrix fleet 饥饿(隔离 2/2 绿),建议按 nextest.toml 既有 threads-required 先例隔离。
- bao_stealth / bun_runtime(lib)/ cdp-server / bun_uws 四套**全绿确认**,零红。

---

*证据文件:/tmp/vfinal/{browser-run,stealth-run,runtime-lib,cdp-run,uws-run,pagenet-r1,pagenet-r2,antiscrape-r2}.log + status.txt/redo-status.txt(真退出码)/ browser-list.txt(1817 测清单)。本报告为本次认证唯一落盘产物。*
