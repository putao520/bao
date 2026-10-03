# WPT 官方栈 crash 台账(encoding 发现面 · 44 CRASH 复跑对照收口)

- date: 2026-10-04 · 分诊人: e16 · binary 对照: `/tmp/bao-wpt/bao`(基线,97f32ed9 面)vs `/tmp/bao-wpt/bao-e26-final`(45284afe livelock 根治后)
- 发现面: WPT 官方工具链 encoding 域全量(`run_bao_wpt.py encoding`,1343 tests,wptrunner log-servo 三态)
- 基线跑: `/tmp/bao-wpt/encoding-run2.log`(proc4,2379.1s,2026-10-03):**44 CRASH** / 13 ERROR / 1 TIMEOUT / 14 unexp-OK / 3 subtest / 1271 expected
- 复跑跑: `/tmp/bao-wpt/enc-rerun-e26final.log`(proc8,670.6s,2026-10-04 02:50 静默窗 load 6→7,负载门控 `/tmp/bao-wpt/load-gated-enc-rerun.sh`):**0 CRASH** / 14 ERROR / 1 TIMEOUT / 14 unexp-OK / 3 subtest / **1314 expected**
- 环境纪律(实测教训): co-tenant 负载风暴期(load 30-62 vs 20 核)e26 曾跑出 **32 CRASH + 30 TIMEOUT** 假象(`/tmp/bao-wpt/e26-enc8-final.log`,00:35)——同一二进制静默窗 0 CRASH(`/tmp/bao-wpt/e26-enc8-final2.log` + 本复跑)。**CRASH/TIMEOUT 聚集先疑环境饥饿,关键面 load < 核数一半再跑。**

## 总判定

**CRASH 桶 44 → 0,零新增,零残留——44 CRASH 全部为 livelock 类(丢失唤醒竞态),已由 45284afe(webdriver completed_loads 边沿锁存 + 泵环 1ms 有界轮询)根治。** 剩余非 crash 残桶 4 类逐条归因如下,均为非缺陷或登记类,无待修 crash。

## 台账条目

### L1 · CRASH ×44(livelock 类)— 已消,零残留

- 复现面: 基线 44 条清单 `/tmp/bao-wpt/baseline-44-crash-dedup.txt`(42 条 legacy-mb 千项分块 decode/encode:euc-kr 14 / shift_jis 13 / big5 7 / euc-jp 6 / iso-2022-jp 2;+ textdecoder-fatal-single-byte?3001-4000 + textdecoder-streaming.any.sharedworker)
- 证据: 基线 44 CRASH(proc4,2379s)→ 复跑 **0 CRASH**(proc8,670s,零新增零保留);e26 终验独立复证 0 CRASH(698s)。修复 = commit 45284afe(根因: webdriver completed_loads 丢失唤醒竞态,泵死等→wptrunner 判 CRASH)
- 归因域: src/bao_browser webdriver 泵/唤醒面(已根治)
- 处置: **收口,零残留**。防复发锚 = 本对照(44→0)+ `compare-baseline.sh` 可重复执行;负载纪律见头部

### L2 · ERROR ×14「SharedWorker is not defined」— pref 关闭面,非缺陷非 crash

- 复现面: 全部 `.any.sharedworker.html`(13 条,基线即有;第 14 条 textdecoder-streaming.any.sharedworker 基线被 CRASH 掩蔽,livelock 修复后显形——ERROR 13→14 的全部来由,非回归)
- 证据: 页面运行时 `SharedWorker is not defined`;`SharedWorker.webidl` 在树且 `[Pref="dom_sharedworker_enabled"]`(`vendor/servo/components/script_bindings/webidls/SharedWorker.webidl:7`);pref 默认 `false`(`vendor/servo/components/config/prefs.rs:424`);实现在树(`script/dom/workers/sharedworkerglobalscope.rs` 等)
- 归因域: 引擎特性开关面(pref 默认关闭),实现非缺失
- 处置建议: 两条路任选——①载具面: WPT prefs 注入 `dom.sharedworker.enabled=true`(wpt-prefs.json)先验证在树 SW 实现功能面,绿则转长期 prefs;②登记 known-disabled 特性,在吸收波按上游 servo 对 SharedWorker 的启用节奏跟进。禁未验证直接开 pref

### L3 · TIMEOUT ×1 — 并行调度余量类,非测试缺陷

- 复现面: 长超时(`meta timeout=long`,60s)千项分块 decode chunk。基线倒下的是 `big5-decode.html?10001-11000`,复跑倒下的是 `euckr-decode-cseuckr.html?6001-7000`——**不稳定归属**,谁在 8 进程调度下被挤过 60s 线谁倒
- 证据: 静默窗单进程计时(`/tmp/bao-wpt/timing-probe?6001-7000.log` 等 3 份,load 2.1): ?6001-7000 = **13.1s**、?5001-6000 = 13.2s、?7001-8000 = 13.3s——60s 上限 4.6x 余量,相邻 chunk 同耗;并行跑单 chunk 被邻居挤压即触线
- 归因域: 环境调度方差(proc8 共享机),非引擎性能缺陷、非测试缺陷
- 处置建议: 记录为已知方差;CI 判读规则=单 chunk 超时忽略、聚集超时先查负载;若未来要稳态清零可对 legacy-mb 分块族放宽 wptrunner chunk timeout(载具面,非必须)

### L4 · FAIL ×4「encodeInto SAB 子测试 PASS」— 期望基线漂移(正向),登记类

- 复现面: `encodeInto()` 的 SharedArrayBuffer 子测试 ×4(encodeInto()/Invalid/Modify/Streaming 组)
- 证据: `.ini` 期望 FAIL(`tests/wpt/meta/encoding/encodeInto.any.js.ini`,上游 servo 无 SAB)——bao 有 SAB 且 encodeInto 语义过测,真值=PASS
- 归因域: 期望文件漂移(产品能力面超出 servo 基线),非缺陷
- 处置建议: 按自维护 fork 规则在吸收波维护 meta ini(登记;注意 ini 是 servo product 共用,bao 侧若长期独有 SAB 需产品级期望分面再裁)

### L5 · unexpectedly-okay ×14 serviceworker — 期望基线漂移(正向),登记类

- 复现面: 全部 `.any.serviceworker.html`(14 条),ini 均文件级 `expected: ERROR`(上游 servo 无 SW)
- 证据: bao 的 SW 实现(REQ-BRW-004 大波)使整文件跑通 → OK,真值=PASS;基线/复跑两轮 14 条完全一致(稳定)
- 归因域: 同 L4,期望文件漂移(正向)
- 处置建议: 同 L4(吸收波 ini 维护登记;此桶同时是 SW 实现的持续回归信号面——若未来某轮翻回 ERROR 即 SW 回归,值钱)

## 方法与可重复性

- 载具: `/tmp/bao-wpt/run_bao_wpt_e26final.py`(=run_bao_wpt.py 单行换 BAO_BINARY)+ venv 上游 wptrunner 胶水,零 wptrunner 改动
- 复跑命令: `/tmp/bao-wpt/load-gated-enc-rerun.sh`(负载门控 load1<10 后 `--processes 8 encoding`)
- 对照工具: `/tmp/bao-wpt/compare-baseline.sh <rerun-log>`(逐桶 eliminated/kept/new)
- 门控取证链: gate log = `/tmp/claude-1000/.../tasks/b1tt0jxh1.output`(load 36→6 等待 68 分钟,02:50:44 开闸)
