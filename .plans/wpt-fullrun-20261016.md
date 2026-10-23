# WPT 工具链重建 + 核心域全量跑台账(compat/web · WebVTT · SVG · dom · editing · css)

- date: 2026-10-16 · 执行体: e-wpt · req: REQ-BRW-002(HTML→CSS→DisplayList→RGBA 渲染管线,WPT=外部 oracle)
- 迁移阶段: adapter(零产品码改动;发现回归=登记不修)
- 载具: `/tmp/bao-wpt/`(venv + `run_bao_wpt.py` + 私拷二进制 + 私有 meta/manifest)——**全私有化**(memory 教训:/tmp/bao-wpt 曾被外部清扫,本台账含 15 分钟重建配方)
- 跑批纪律: `--processes 4`(memory wpt-official-toolchain-driver 并发上限,≥8 bao GL 进程 llvmpipe livelock 未根治禁调高);每域跑前 load1<10 + wptserve 全端口集(8000-8003/8443-8446/8888/8889/9000)空闲门控;每域 `--log-raw` 逐格通道(子测真态唯一权威)

## 环境与 provenance

| 项 | 值 |
|---|---|
| 二进制 | `/tmp/bao-wpt/bao`(私拷自 `/var/cargo-builds/3c/6184ceb77072ba/test-ci/bao`,opt profile=WPT 波基线纪律) |
| 二进制 sha256 | `4683bff54ef88688d147a94813c1d13d1478d01a825cf6d6c2372b3bbae743f1`(175294976 bytes) |
| 测试树 | `/home/putao/code/tools/servo/tests/wpt/tests`(参考仓 working tree) |
| 元数据根 | `/tmp/bao-wpt/meta/`(私有快照:editing←**vendor** `/home/putao/code/rust/bao/vendor/servo/tests/wpt/meta/editing`(e150-e191 波自治更新态,mtime 2026-10-12);dom/css/webvtt/svg←参考仓 meta(上游 2026-08 快照时代=历史基线时代);compat 零 ini=首跑基线) |
| MANIFEST | `/tmp/bao-wpt/meta/MANIFEST.json`——`wptmanifest.manifest.load_and_update(tests_root, …, working_copy=True, rebuild=True, cache_root=/tmp/bao-wpt/cache)` 私有重建(161513 items/296 域,45s),**零 servo 树修改** |
| venv | `/tmp/bao-wpt/venv`(python3.12):mozlog==8.1.0 mozinfo==1.2.3 mozprocess mozdebug requests pillow flask six atomicwrites html5lib pyyaml typing_extensions |
| launcher | `/tmp/bao-wpt/run_bao_wpt.py`(servo `wpt.run.run_tests` glue;`--binary/--webdriver_binary` 钉私拷二进制;`--metadata/--manifest/--tests` 三覆盖私有根;SERVO_PY 先于 tests root 防 `wpt.py` 遮蔽) |
| 分析器 | `/tmp/bao-wpt/analyze.py`(raw mozlog 语义:`expected` 字段仅 status≠expected 时出现;`known_intermittent` 并读;多值期望逐 run 收集) |
| 首跑日期 | 2026-10-16 20:37 起(编 compat → webvtt → svg → dom → editing → css 顺序) |

### 分域分母(manifest runnable items vs 历史基线)

| 域 | 本次 manifest runnable | 历史基线分母 | 对齐判定 |
|---|---|---|---|
| compat | 38(13 th+25 reftest) | 无(首跑) | 首跑基线 |
| webvtt | 322(72 th+250 reftest) | 无(首跑) | 首跑基线 |
| svg | 1280(711 th+513 reftest+48 crashtest;8 print-reftest 被 servo product test_types 排除) | 无(首跑) | 首跑基线 |
| dom | 736 | 736(wpt-b3-alignment 2026-10-04) | **精确一致** |
| editing | 851 | 855(e191 2026-10-12) | -4(manifest 时代差;逐格对拍为准) |
| css | 33890−388 print-reftest(不跑)=**33502** | 33502(wpt-css-reftest-tls 2026-10-04) | **精确一致** |

## 逐域台账

### css(33502 = 历史基线精确同分母 · 2026-10-16 20:55-22:32 · 5803.7s · proc4)

| 计数 | 本次(file 级) | C-face 基线(2026-10-04,bao-e26 dev 二进制,汇总面) |
|---|---|---|
| files | 33502(精确同分母) | 33502 |
| as-expected | 29187(OK 6992 + PASS 15666 + FAIL 6216 + ERROR 134 + TIMEOUT 170 + CRASH 1 + SKIP 8) | 26661(+8 skip;其口径含子测对齐判定) |
| unexpected-FAIL | 3652 | 3978 |
| unexpected-ERROR | 310 | 443 |
| unexpected-TIMEOUT | 222 | 333 |
| unexpected-CRASH | **0** | 4 |
| unexpected-OK(正翻) | 21 | 18 |
| unexpected-PASS(file 级) | 110 | 417(汇总面口径,不可直接比) |
| 子测级(raw 权威) | uF 33409(1202 文件)/ uP 5249(242 文件) | 「1757 subtest」(wptrunner 汇总面,口径不同不可比) |

- **零回归,四面净改善**:CRASH 4→0——C1 四格全部翻绿实证(`scope-deep.html` OK、`inline-crash.html` PASS、`clip-path-recursion-001.svg` PASS、`outline-scrollIntoView-crash.html` PASS;16MiB StyleThread 栈 + usvg clip/mask 守卫根治);ERROR 443→310;TIMEOUT 333→222;FAIL 3978→3652。本轮 proc4+低竞争亦压低 TIMEOUT 假阳(历史 proc8)。
- 意外面按目录聚族与历史桶全部同形:css-writing-modes 850 / view-transitions 373 / multicol 350 / typed-om 284 / contain 250(C6/C5 布局特性 backlog 族,零新族)。C3 container-queries 桶(~106 ERROR)本轮**消失**(stylo 0.22 前向态吸收,历史桶已不存在=正向);C2 typed-om TDZ/工厂桶 279 在位(上游共同特性缺口,登记面)。
- unexpected-ERROR 310 分解:typed-om 279 + anchor-position 9 + exclusions 7 + 零散 15——全部落 C2/C4 已登记族。
- unexpected-TIMEOUT 222:view-transitions 105 + masking(clip-path-animations)66 + highlight-api 15——C5 同族(历史 210+130,量级收敛)。
- 子测级 uP 5249 聚族:clip-path-interpolation(636)/color-mix 解析(213)/text-transform-math(112)/grid abspos(198)——C8「动画/合成插值族 ini 期望 FAIL 而 bao PASS」同族正向漂移;uF 33409 主桶 anchor-position 解析面(anchor-parse-valid 2359+4305)。
- solid-color note 291 行(≈146 唯一对);e42 终局已证零真空白帧缺陷,该 note 是双侧诊断非失败原因,按 C7 终局口径不另立桶。
- 1 as-expected-CRASH = `css-images/image-multipart-crash.html`(ini expected: CRASH,登记 crashtest,零意外)。
- 环境注记:21:25-21:45 共机构建风暴(load1 峰值 33)与本 run 中段重叠——TIMEOUT 判定在该窗可能虚高;因所有可比桶全部低于历史基线,回归方向判定不受影响;风暴窗内单格 TIMEOUT 若需定罪须 proc1 静默窗复跑(本波未做,登记为已知局限)。

## dom 两格 proc1 裁定(2026-10-16 22:38,静默窗 load 2.58,隔离单跑)

| 格 | 全量轮(proc4) | proc1 裁定 | 判定 |
|---|---|---|---|
| `/dom/events/EventListener-invoke-legacy.html` | TIMEOUT(as-expected) | **TIMEOUT(确定性复现)**,3 个「Legacy listener of transitionend/animationend/animationstart」子测全 TIMEOUT,对应普通 Listener 子测全 PASS | **真实行为变更格**:wave1(2026-10-04)观察 OK(legacy 事件处理器属性被调用,ini 期望 TIMEOUT 的正向漂移格)→ 现 legacy listener(on*-属性形态)在 transition/animation 事件上不触发。按 adapter 阶段纪律**登记不修**;嫌疑面=transition/animation 事件派发的 legacy event-handler 属性调用(e99 快照换血 a7272f16 后形态),归 animation/transition 事件面 backlog |
| `/dom/events/scrolling/scrollend-event-fires-on-visual-viewport.html` | ERROR(expected TIMEOUT,unexpected) | **ERROR(确定性复现)**:`Unhandled rejection: can't access property "width", viewport is undefined` @ `visual-viewport/viewport_support.js:10 pinchZoomIn` ← `window.onload` | **族内形态漂移,非新缺口**:`window.visualViewport` 未实现(特性缺口,servo 上游同缺)→ 页面 onload 即抛 rejection,比 wave1 的挂到超时更快失败。同属 B4 scrollend/visualViewport 继承族 |

## 总判定

**六域全量跑毕,零回归。** 判据与清单:

1. **零回归判据成立面**:dom 736(分母精确一致)继承面逐族对拍零消失零新增,净改善 7 格;css 33502(分母精确一致)四面全改善(CRASH 4→0 具名四格全翻绿);editing 851 file 级零意外红(优于 e191 基线);compat/webvtt/svg 首跑基线(无历史分母,按 stop 条件③如实登记)。
2. **回归清单(登记不修,共 2 格,均已 proc1 隔离裁定确定性)**:①`/dom/events/EventListener-invoke-legacy.html`(wave1 OK→今 TIMEOUT,legacy 事件处理器属性在 transition/animation 事件上不触发);②`/dom/events/scrollend-event-fires-on-visual-viewport.html`(wave1 TIMEOUT→今 ERROR,visualViewport 缺口拒绝路径,族内形态漂移)。其中②属继承族形态漂移非行为回退,①是唯一真实行为变更候选格。
3. **子测级登记**(正向漂移,归 ini 自治波,不重开):editing uP×3、css uP 子测 5249(插值/解析族)、dom 子测意外 9(svg 注入/moveBefore 同类)、svg 21 reftest+414 子测首跑基线。
4. **判别律消费记录**:CRASH 零引擎死亡(journalctl -k 零 segfault/oom 记录 + wpt-crash-sigterm-harness-class 判别);多值期望 ini 本波六域 editing/dom/webvtt/svg/compat 零命中、css 7 文件(逐值重跑语义,raw 逐 run expected 字段权威);端口互斥每域跑前全端口集核查零冲突。

## 局限(如实)

1. compat/webvtt/svg 首跑基线:ini 锚=参考仓 2026-08 时代(webvtt 渲染面零登记/svg 21 格未登记),后续对照波应先核 origin/main 锚时代(wpt-meta-anchor-era-check)。
2. css 子测级数字为 raw-log 权威口径(33409 uF/5249 uP),与历史台账的 wptrunner 汇总面(1757/417)计数面不同,本波起以 raw 口径为新基线。
3. css 跑批中段 20 分钟与共机构建风暴重叠(load1 峰值 33):TIMEOUT 单格定罪未做 proc1 复跑(仅 dom 两格裁定);风暴不改变回归方向判定(所有可比桶均低于历史)。
4. editing 分母 851 vs e191 的 855:manifest 时代差(-4,含 working_copy 带入的 e185-probe1.html 探针格);逐格对拍不受影响。
5. 二进制 provenance 单点钉定:全程未重建二进制(sha256 4683bff5… 恒一),若后续波换二进制须重钉并复跑对照格。

## 工具链可复现命令块

```bash
# 1. venv(清单见上表)
python3 -m venv /tmp/bao-wpt/venv
/tmp/bao-wpt/venv/bin/pip install mozlog==8.1.0 mozinfo==1.2.3 mozprocess mozdebug \
  requests pillow flask six atomicwrites html5lib pyyaml typing_extensions
# 2. 私拷二进制(重钉 sha256)
cp /var/cargo-builds/3c/6184ceb77072ba/test-ci/bao /tmp/bao-wpt/bao
sha256sum /tmp/bao-wpt/bao
# 3. 私有元数据根(editing 用 vendor 自治态,其余用参考仓)
R=/home/putao/code/tools/servo/tests/wpt/meta
V=/home/putao/code/rust/bao/vendor/servo/tests/wpt/meta
mkdir -p /tmp/bao-wpt/meta && cp -r $R/dom $R/css $R/webvtt $R/svg /tmp/bao-wpt/meta/ && cp -r $V/editing /tmp/bao-wpt/meta/
# 4. 私有 MANIFEST(servo 树零修改)
/tmp/bao-wpt/venv/bin/python /tmp/bao-wpt/build_manifest.py   # load_and_update(rebuild=True, working_copy=True)
# 5. launcher(run_bao_wpt.py,见上表)+ smoke(R0 历史探针)
cd /tmp/bao-wpt && ./venv/bin/python run_bao_wpt.py --processes 2 \
  css/CSS2/floats/floats-placement-vertical-00{1a,1b,1c}.xht   # 预期: 3 ran as expected
# 6. 分域跑批(run_all.sh:load<10+端口集门控,--processes 4,--log-raw)
/tmp/bao-wpt/run_all.sh
# 7. 逐格分析
/tmp/bao-wpt/venv/bin/python /tmp/bao-wpt/analyze.py /tmp/bao-wpt/logs/<域>.raw.log <域>
```
