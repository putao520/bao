# WPT webaudio 域全跑台账(e121 · REQ-BRW-004 AudioWorklet 面 P2「WPT 域扩展验收」收官)

- date: 2026-10-06 · 执行: e121 · 域: `webaudio`(WPT 官方树,经 servo 参考树)
- binary: `/tmp/e121/bao` — **worktree 钉 HEAD `00b2f15f` + 私有 CARGO_TARGET_DIR `--profile test-ci --locked` 自建**(Finished 10m41s;sccache 命中);size 173254456,sha256 `3f916bbf81be2b70a9e03bd550de5a55f9d5f37090084f7d61bd481ab23c7887`;字符串探针:`processorOptions`(e114)/`audioWorklet`/`addModule`/`registerProcessor` 全命中。主树当时仅 `examples/01-browser/Cargo.lock` 一个无关脏文件,e120 的 script 面零污染(worktree 物理隔离)。
- 载具: 官方工具链直跑(servo `wpt.run.run_tests` glue,零 wptrunner/wptserve 改动),launcher `/tmp/e121/run_e121.py`(e67 版 + `--metadata/--manifest/--tests` 三覆盖,见 §E)。venv 私有重建于 `/tmp/e121/venv`(mozlog 8.1.0/mozinfo/mozprocess/mozdebug/requests/pillow/flask + six/atomicwrites/html5lib/pyyaml/typing_extensions——后六项为 manifest 库依赖,e67 记忆清单未载,补录)。
- oracle 期望: servo 参考树 `tests/wpt/meta/webaudio/**.ini`(203 文件,同步时 git-clean @ servo HEAD `29b280def`)。

## 同步形态(拷贝,非引用)

上游 `~/code/tools/servo/tests/wpt/`(权威,webaudio 路径 git-clean)→ 本仓 `vendor/servo/tests/wpt/`,rsync -a 后 `diff -r` 全树零差(**byte-identical**,收尾复验仍零差):

| 目标 | 文件数 | 内容 |
|---|---|---|
| `vendor/servo/tests/wpt/tests/webaudio/` | 420 | the-audio-api 全 31 接口域 + the-audioworklet-interface 全套 + js/(audio-testing 库)+ resources/ |
| `vendor/servo/tests/wpt/meta/webaudio/` | 203 | servo ini 期望全量(the-audioworklet-interface 39 份逐文件 + the-audio-api 各接口) |

本仓 WPT 树此前仅 html 域(roadmap P2 实录登记缺口「树缺 webaudio 测试域」),本同步即该项收敛。本地树无 MANIFEST.json——跑法需私有 manifest(§E),非同步物。

## 跑法

```
cd /tmp/e121 && xvfb-run -a ./venv/bin/python run_e121.py <tests-root 相对路径...> \
  -- --processes 4 --log-raw <name>.raw.log [--timeout-multiplier 6]
```
- the-audioworklet-interface 39 测试文件分 5 chunk(8×4+7)顺序跑(10s 默认超时);17 滞留文件复跑(60s);3 代表文件 proc1 隔离复跑(60s)
- params 面=目录整跑;ctors 面=19 文件批跑;idlharness 单跑(proc1)
- 端口门: 跑前 `ss` 查 wptserve 全端口集(8000-8003/8443-8446/8888/8889/9000),跑后清 bao 子进程+端口持有 python(见 §E 教训)

## A. the-audioworklet-interface 全套读数(39/39 文件全部启动;基线为 28/43 并集)

| 终态类 | 文件数 | 明细 |
|---|---|---|
| **as-expected(终态=ini 期望)** | 19 | 含 2 个 ERROR-as-expected(audioparam-iterable/throw-onmessage,ini file-level `expected: ERROR`,servo 同病) |
| **正向翻转: file-level ERROR→OK** | **3** | globalscope-sample-rate / registerprocessor-dynamic / extended-audioworkletnode-with-parameters |
| **deterministic TIMEOUT** | 13 | 见 §D 挂起类(10s/60s/proc1×60s 三形全挂) |
| **deterministic TIMEOUT(ini 期望 ERROR)** | 3 | node-onerror / postmessage-sharedarraybuffer / process-parameters |
| **CRASH** | 1 | audioworkletprocessor-promises(wptrunner 杀挂死浏览器类,proc1 复现) |

子测级对 ini 期望的分账(52 个 ini `expected: FAIL` 子测):**10 翻 PASS** + 22 被挂起类阻断(TIMEOUT/NOTRUN)+ 13 维持 FAIL + 7 未发射(文件级挂死前未到);另有 expected-ERROR/NOTRUN 子测翻 PASS 2 个(合计 flip 12)。

## B. 36 条基线对照(commit 776ffd31,2026-10-05 验收录)

| 基线读数(28/43 文件并集) | 本轮(39/39 启动) | 判定 |
|---|---|---|
| 3 个 expected-ERROR 文件翻 OK | 3 个同形文件翻 OK(计数逐一对应) | **保持,零回退** |
| 36 个 expected-FAIL 子测翻 PASS | 完成文件上 10+2 翻转;其余 29 个期望-FAIL 子测落在 16 文件挂起类内(TIMEOUT/NOTRUN/未发射),**不可达非证伪** | **不可直接对账**——见 §D 归因登记;判定实验=776ffd31 二进制复跑挂起文件(另案) |
| 15 个 expected-PASS 未达(params 精细面/port transfer/SAB) | 完成文件上的真红仅 5 子测(§D 红格);其余同样压在挂起类后 | 部分推进(ctor-options 4 翻、construction 3/4 翻) |
| 其余文件被串行超时等待阻塞(15 文件未启动) | 16 文件启动后中途挂死 + 1 CRASH | **机制不同**:基线=饥饿未启动;本轮=启动后首子测挂起(两者可能同根源,基线未留 per-file 记录无法断代) |

## C. webaudio 域代表性面读数

| 面 | 文件 | file-level | 子测 | 备注 |
|---|---|---|---|---|
| **params**(`the-audioparam-interface/` 整目录) | 46/46 | **全部 as-expected**(含 4 个 ERROR-as-expected: linear/exponentialRamp、setTarget、setValueCurve——servo 同病) | 7 翻 PASS(含 k-rate-audioworklet/k-rate-oscillator AudioWorklet 参数面)+ 39 红(§D) | AudioParam automation 时间线与 WorkletParam 复用面工作正常 |
| **constructors**(18×`ctor-*.html` + audiocontextoptions) | 19/19 | **全部 as-expected** | 715 子测全绿 + **4 翻 PASS**(ctor-delay 3/ctor-oscillator 1)+ **0 红** | 全域构造器+options 字典面零缺陷 |
| **idlharness**(IDL 符合度) | 1 | OK | **1051 as-expected + 112 翻 PASS + 0 红** | webaudio IDL 面(constructor/attribute/interface 形状)全量符合,112 处超出 servo 期望 |

## D. 红格逐格归因登记(登记不修,修复另案)

**D1 · 16 文件确定性挂起类(本轮最大新桶)**:13 个 exp-OK + 3 个 exp-ERROR,10s/60s/proc1×60s 三形全挂;首子测(`setup-worklet`/render-wait/port 握手)不 settle。代表性: messageport、processor-options(processorOptions round-trip)、process-getter、processor-construction-port(4 子测全 NOTRUN)、suspend 前 setup、no-process-function、process-frozen-array/zero-outputs/unconnected、automatic-pull、rendersizehint、denormals、creation-time、output-channel-count、postmessage-SAB、onerror、process-parameters + CRASH(promises)。
- 归因候选(登记):e119 渲染泵饱和配速(9d79635b,本轮二进制含)与特定 worklet 流的交互——e119 验证面=echo 集成测试(15/15),未覆盖 WPT 挂起形;**判别实验=776ffd31(pre-e119)二进制复跑 3 代表文件**,若彼时不挂则 e119 嫌疑升级。
- 基线断代不可行:776ffd31 验收 run 未落 per-file 记录(commit message 仅聚合数),「28/43」与「16 挂」的文件集交集无法重建。

**D2 · 完成文件上的真红(5 子测)**:
- `audioworklet-suspend.https.html` ×4:worklet 启动后 `context.suspend()` 不生效(state 保持 running,currentTime 继续走)——suspend-from-worklet 面缺口
- `audioworkletnode-construction.https.html` ×1:「模块加载前建节点应 throw」未 throw(NotSupportedError 门只按注册表,addModule 前 vs 后未区分?登记)

**D3 · params 面 39 红(4 文件,全渲染数学面)**:
- `audioparam-nominal-range.html` ×16:DelayNode/BiquadFilterNode 等 AudioParam 名义范围钳制未实现(value 越界不 clamp)
- `k-rate-oscillator-connections` ×8 / `k-rate-biquad-connection` ×7 / `k-rate-panner-connections` ×6: k-rate 参数带输入连接的渲染输出逐样本不符(a-rate/k-rate 混合路径数学)
- `audioparam-setValueCurve-exceptions` ×2: setValueCurve 重叠区间的 exception 面缺一格
- 归属初判: servo-media 渲染数学继承面(ini 对同文件其他子测已大量 expected:FAIL,servo 同病域),非 AudioWorklet 自研面;逐格归因另案。

## E. 基础设施事件与教训(影响后续 WPT 波)

1. **servo 参考树 MANIFEST.json 缺 webaudio 全域**(0 audioworklet 条目;上游 2026-08-11 快照即如此)。显式路径传给 wptrunner 时过滤集为空 → **静默回退整仓 56639 测试**(首轮事故,已杀)。解法=私有 metadata root: `--metadata /tmp/e121/meta --manifest /tmp/e121/meta/MANIFEST.json --tests <servo tests root>`,manifest 用 servo `manifestupdate` 同款 API `manifest.load_and_update(rebuild=True)` 私有生成(168777 items/420 webaudio/74 audioworklet),servo 树零修改。**oct-05 基线 run 如何解析路径存疑(可能用了已退役载具的私有 manifest)——后续对账波注意。**
2. **/tmp 清扫事件×2(18:0x-18:2x)**:共享载具 `/tmp/bao-wpt` 整目录被外部删除(6GB 二进制,疑似全量电池 OOM 应对清理);本任务 `/tmp/e121/aw-chunk-*.txt` 中间列表也被清。对策=载具私有化(/tmp/e121 全套)+列表运行时从源树再生+空列表硬拒(whole-suite 逃生门)。
3. **wptserve 端口卫生**:残留的 multiprocessing 持有者(8000-8446/8888-9000)会让下轮 run 全端口 `Servers failed to start`;清理必须覆盖全端口集(只清 8888/8889/9000 不够——第二轮事故)。
4. 原始 raw log(sha256 略,路径 /tmp/e121/*.raw.log,半衰期资产):aw-{1-8,9-16,17-24,25-32,33-39} / stragglers2 / proc1probe / params-dir / ctors / idl / probe。本台账数字全部由 analyze.py 逐事件分账,可由 raw log 复算。

## 收口

- 读数: the-audioworklet-interface 全套 39 文件双跑全启动(3 file-flip 保持 + 12 sub-flip + 16 挂起 + 1 CRASH + 19 as-expected);params 46/46、ctors 19/19、idlharness 112 翻 0 红零意外终态
- 同步: vendor webaudio 域 420 测试资源 + 203 ini 期望,byte-identical,首次入库
- 红格全数登记 §D(不修,另案);判别实验与断代限制如实登记 §D1

## F. e122 挂起类清偿(2026-10-06,判别实验+根因修复+诚实分账)

### F1. 判别实验结论(合同①)
**776ffd31(pre-e119)二进制复现 17 文件同形态挂起(16 TIMEOUT+1 CRASH,per-file 终态逐一一致)——e119 饱和配速(9d79635b)无罪**;挂起类是 AudioWorklet 实现的结构面,与配速改动无涉。raw log:/tmp/e122/{776-chunk-01,776-chunk-02}.raw.log(chunk-00 被 /tmp 清扫覆写,其 6 文件终态在会话记录:全 TIMEOUT)。

### F2. 根因(e122 CDP 探针流水定位,探针组 /tmp/e122-http/*)
| # | 根因 | 机制 | 处置 |
|---|------|------|------|
| A | **构造器 postMessage 走死路径** | `instantiate_processor` 在 `Construct1`(构造器 JS 已跑)**之后**才对 processor port 装 conduit redirect——构造器体的 `this.port.postMessage` 落入 worklet 线程无投递的星座 port 路径,消息必丢(e121 集成面全绿是因为探针都先 post 再等回显,redirect 已在位;真 WPT 构造器首发全挂) | **已修**:pending-construction handoff——scope 槽 `Fresh(conduit)→Consumed`,`AudioWorkletProcessor` 基类构造器铸 port 即装 redirect(audioworkletglobalscope.rs/audioworkletprocessor.rs/audioworklethandler.rs) |
| B | **process() this=undefined** | `process_quantum` 以 `UndefinedValue()` 作 this 调 `Call`;类体严格模式 → 任何 `this.port`/`this.<字段>` 访问首块即 TypeError → 节点 processorerror 锁死静音。**echo 集成测试的 processor 只用参数不触 this,验收面从未覆盖**——这是 17 挂的最大单一根因 | **已修**:`this_value = ObjectValue(instance)` 一行 |
| C | **单次构造 TypeError 面缺失** | construction-port 电池要求:实例化在飞时第二次 `new AudioWorkletProcessor()` 抛 TypeError | **已修**:handoff `Consumed` 臂 + webidl `[Throws]` |
| D | **processorerror 是裸 Event 非 ErrorEvent** | spec 要求 ErrorEvent(message/filename/lineno/colno) | **已修**:捕获 pending exception 的 ErrorInfo 经任务过线程,`ErrorEvent::new` 派发(onerror sub1 PASS) |
| E | **worklet scope 缺 `renderQuantumSize` 全局** | rendersizehint 构造器体读该全局 → ReferenceError → 实例化 latch → 挂 | **已修**:getter 恒 128(servo-media 固定量子;renderSizeHint 选项未贯通,诚实 FAIL 与 servo ini 一致)——rendersizehint 文件级 TIMEOUT→OK |
| F | **未连接/零输出节点永不被处理** | servo-media graph.process = dests 起 DFS 拉模型;无 destination 通路的 worklet 节点 process() 永不调用(Chromium 语义:worklet 节点恒处理直到 process 返 false)——graph.rs/render_thread.rs **越出本合同 owner 边界** | **登记**(ini 多值期望);根治=后续合同:servo-media graph「worklet 节点恒处理」语义 |
| G | **SAB 经 conduit 序列化失败** | 构造器 post 含 SAB → worklet realm structuredclone write 抛 → 构造器失败 latch → 挂(postmessage-SAB) | **登记**(servo 继承序列化面) |
| H | **suspend() promise 不 settle** | servo 实时状态机继承面(suspend 后 currentTime 仍走)——no-process-function 在 `await context.suspend()` 挂死 | **登记** |
| I | **worklet 线程 minor-GC tenuring SIGSEGV(新暴露;e127 终态收口)** | 实时 context 每块持续 port.postMessage(A/B 修复后才有流量)→ 构造小对象/serialization 分配 → nursery minor GC → `TraceIonJSFrame` 追踪 Ion 帧槽位遇垃圾值 → `TenuringTracer::promoteObject` SIGSEGV。gdb(dev 符号)栈:promoteObject←TraceIonJSFrame←minorGC←NewArrayObject←structuredclone::write←post_message_impl(redirect 臂)。**判别**:离线 10335 块逐块 post 存活、纯分配不 post 存活、单发存活——仅「实时+持续 post」复现;机制落点 SM GC/JIT+structuredclone 面(越界) **e127 根因闭环(2026-10-06,毒源=servo 侧,非引擎)**:毒值=stale OBJECT Value,payload 恒为 from-chunk+0x280(非半空间模式每代 chunk 复位首分配位,`Space::clear→moveToStartOfChunk(0)`),payload[-8] 无有效 NurseryCellHeader;毒值 word 仅存于 worklet 栈(malloc 堆全扫零命中);铸出通道=store-buffer **124 个互异栈地址 CellPtrEdge**——`instantiate_processor` 原 `heap()` 闭包(audioworklethandler.rs:568)对 `Heap<T>` 执行 mozjs-sys `jsgc.rs:341-345` 文档明文禁止的「栈上 temporary Heap + set(nursery obj) + move」:post-write-barrier 以栈地址注册 edge,struct move 进 registry 后五条 edge 全悬垂,每代 minor GC 经 `*edge` 读栈残留铸毒值/经 `*edge=promoteOrForward` 回写蚀活帧——TraceIonJSFrame(:1072)/TraceExactStackRootList/StoreBuffer CellPtrEdge 三崩溃面由此统一。引擎侧排除实证(OSR fallback 0 触发/getSafepointIndex 精确/CallArgs::get 越界安全/非半空间无 to-space 晋进)= e127 终报 | **已修(e127,register-then-set)**:五 Heap 槽 default 构造→`register_processor_instance` 落最终地址→`set_instance_heap_slots` 从 instance_roots/instance/rooted global 回填(audioworklethandler.rs+audioworkletglobalscope.rs);channel 数组 `HeapBufferSource::new` 本用 `Heap::boxed` 无辜;判别形态 10/10 全窗存活(loop+promise 各 5,修复前同窗 3/3 SIGSEGV)+audioworklet 5×11/11+media 26/26 |

### F3. 17 文件终态分账(终版二进制 = A-E 全修)
| 终态 | 文件数 | 明细 |
|------|--------|------|
| **OK(完成且如 ini 期望)** | 6 | denormals / messageport / creation-time / output-channel-count / suspended-context-messageport / rendersizehint(E 修复转化) |
| **诚实非挂终态(红但真跑)** | 2 | options=ERROR(sub1 翻 PASS:e114 载荷形状残量「4 属性 vs 期望 2」,诚实红不掩盖);promises=CRASH(根因 I,确定性) |
| **确定性 TIMEOUT(继承/结构面,ini 登记)** | 9 | SAB(G)/automatic-pull(F)/onerror sub2(G 族 blob 反序列化面)/no-process-function(H)/frozen-array sub1(F,任务体不连节点)/zero-outputs(F)/process-getter(F)/process-parameters(F)/construction-port Singleton(F) |
子测级正向翻转(保持 ini 原期望,uncaught-pass 为正向信号):construction-port 3/4(constructor-port 语义全对)、onerror sub1(ErrorEvent)、options sub1(processorOptions round-trip)、messageport sub2/sub3、options/frozen-array/zero-outputs 等文件的 AUDIT 框架子测若干。

### F4. ini 登记(vendor meta,自治通道;multi-value 期望覆盖 servo 真值与 bao 确定性态)
automatic-pull / process-getter / process-parameters / zero-outputs / frozen-array / construction-port(Singleton)/ SAB(file [ERROR,TIMEOUT])/ no-process-function / onerror(sub2/sub3)/ promises(file [FAIL,CRASH])/ options(sub2)——11 文件;正向翻转子测**不**改 ini(可见性保留)。

### F5. 复验(零回退)
- `cargo nt -p bao-servo-media-audio` **26/26**;`BAO_TEST_NETWORK=1 cargo nt -p bao-browser -E 'test(audioworklet)'` **11/11**(e119 面板 9 + e122 新增回归测 2:ctor-port-message/process-this-binding)
- 绿基线 22 文件(39−17)终版二进制重跑:**20 OK + 2 ERROR-as-expected**(audioparam-iterable/throw-onmessage,与 e121 一致)——零回退
- 判别/修复全程 raw log:/tmp/e122/{776,fix2,fin,grn}-*.raw.log(半衰期资产)

### F6. 基础设施事件
- **/tmp/e121 整车再次被外部清扫**(venv/meta/296M raw logs 全失)——本波重建:venv 私建于 /tmp/e122/venv、meta 从 vendor ini 树 + manifestupdate(rebuild=True) 重生成(MANIFEST.json 40M,webaudio 全域在案)、launcher run_e122.py。/tmp/e122-cdp-probe.py 等散置 /tmp 根的探针也被清(载具资产必须全部私有化进 /tmp/e122)。
- e122 CDP 探针流水(probe.py + /tmp/e122-http/*):构造器首发/echo 对照/throw 探针/计数器/options 双向/分配-only/持续 post/离线长渲染——归因链的可复算载体。

## G. e137 WPT campaign 基线重锚(stylo 0cb50925b 收敛后的 ini 期望对齐,2026-10-07,REQ-BRW-002)

e128 终报遗留⑤清偿:stylo 0cb50925b 落地(d3b752e0)后 WPT 期望基线重锚。

### G1. 拓扑真相(差分定位前置考古,修正任务书的基准表述)

- **参考仓本地 main @29b280def = 上游 2026-08-13 快照(4842b770e)+ 仅 3 条 bao agent ini 提交(e74/e80/e91)**;与 origin/main 的 merge-base 即 4842b770e,界外 887 commit。任务书所说「cssom/font ~40 ini 删除+progress pref 翻转随参考仓 29b280def 已收敛」实为**随 648de26fa 血缘已收敛**(=e128 vendor 收敛基,在 origin/main 线上、不在 29b280def 祖先链内)。
- e128 吸收窗(240a37393→648de26fa,12 commit)本身**零 webaudio meta 变更**;全部改善面落在 4842b770e→240a 子窗口,由六个 media-audio/script 提交携带(ba2b2be24/acbfcae6e/fa06b1bfd/e7c05ec62/c66473652/12bd2ae9a),bao 侧经选择性吸收(e101 delay_node+periodic_wave=435707b6、H 收敛批、e128)落地。
- 真实「~40」构成:webaudio 面 203(29b/e121 同步态)vs 163(648d 终态)= 净 40 文件差(47 D+7 A);cssom+css-fonts 删除 21(另 css-nesting/cssom-view 边缘 3);progress-computed.html.ini 1 子测期望删除(非字面 pref 翻转)。

### G2. 修复落地映射(引擎侧实证,决定删/留)

| 上游提交 | 修复 | 本仓 vendor 状态 | 判定 |
|---|---|---|---|
| 12bd2ae9a #47957 DelayNode | delay_node/mod.rs == 648d(+bao tests) | ✓ 在 | 删 |
| c66473652 #48510 PannerNode cone gain | panner_node.rs == 648d | ✓ 在 | 删 |
| acbfcae6e #47492 PeriodicWave | periodic_wave.rs == 648d(+bao tests) | ✓ 在 | 删 |
| ba2b2be24 #46870 decodeAudioData detached | baseaudiocontext decode 面 == 648d(diff=bao AudioWorklet 增量+C15 签名形) | ✓ 在 | 删 |
| e7c05ec62 #48347 AudioParam automation | **param.rs=旧基底+bao WorkletParam;oscillator/constant_source 缺 4 行 update_parameters** | ✗ 缺 | 留 |
| fa06b1bfd #48351 clamping/NaN | param.rs val_range+audio_node SetParamRange 缺 | ✗ 缺 | 留 |
| b2170f023 #47743 CSSOM parentRule | cssrule.rs == 648d | ✓ 在(cssom 面,本仓无 ini 载体) | 跑验 |
| c749c02ff #47388 @font-face descriptors | fontface.rs == 648d | ✓ 在(font 面,无载体) | 跑验 |
| progress-computed 子测删除 | stylo css-values progress() computed(`layout_css_progress_function_enabled` 在 --enable-experimental-web-platform-features 集内,wptrunner 形态默认开) | ✓ 跑验 | 跑验 |

### G3. 重锚动作(vendor meta/webaudio,203→175)

- **删除 28 ini**(跑验证据=全部子测实绿):delaynode 族 13 + panner 族 8 + periodicwave/oscillator 族 5 + detached ×2;含静态判保守而跑验推翻的两件:panner-automation-position(全绿)、k-rate-delay(14/14 绿)。
- **更新 3 ini 到 648d 终态内容**:idlharness.https.window.js.ini(删 DelayNode/PeriodicWave IDL 陈旧 FAIL;复验 112 翻 0 红与 e121 读数一致)、automation-rate([DelayNode] 子测翻绿)、automation-changes([Listener.positionX.setValue] 翻绿)。
- **保留 19**:15 个 #48347/#48351 缺失族(EXPFAIL,bao 真红如期望)+ 4 个残留红面(k-rate-biquad-connection/k-rate-oscillator-connections/k-rate-panner-connections/constant-source-output——含超出 ini 覆盖的 unexpected-FAIL,见 G6 登记)。
- **保留 2 M 面(测试树漂移绑定)**:rendersizehint-smoke、audioworklet-messageport——648d ini 配对**新测试内容**(768000Hz 子测族移除/wasm 变体),campaign tests 树(=参考树=vendor tests,Aug-13)为旧内容,套新 ini 会错配;登记待 tests 树更新波。
- **跳过 7 个 648d 新增面(A)**:6 个测试文件在参考树缺失(messageport-wasm/no-coop-coep/oversized-resample/incremental-rendering ×2/interrupt-when-created-hidden)+ ctor-offlineaudiocontext 测试内容已变;「本仓无对应面的不动」。
- e122 的 11 文件本地登记面零触碰(47 删面与其零交集,已核)。

### G4. 跑验读数(binary=/tmp/e137/bao,f7b3da75 worktree 钉 HEAD `--profile test-ci --locked` 自建,sha256 d190dfba6b3cc9f68d51f50bce15847d6ca733afffafb1a5fc660effef40c789,173773144B,processorOptions/audioWorklet/addModule/registerProcessor 探针 4/4)

| 面 | 读数 |
|---|---|
| webaudio 47 候选(6 chunk × proc4,timeout×6) | 47/47 启动:28 全绿(7 GREEN+21 全绿翻转,含 2 个 file 级 exp-TIMEOUT→OK)/ 15 EXPFAIL(param 族真红)/ 4 残留红;**零 CRASH 零进程死亡** |
| 重锚后删除面抽样 6(delaynode/ctor-oscillator/periodicWave/panner-azimuth/k-rate-delay/detached) | **6/6 ran as expected** |
| M 面 3 复验 | 3/3 OK,0 unexpected-red;58 翻=AudioWorklet IDL 面(bao 有上游无)按 e122 F4 惯例保留可见性 |
| cssom 代表 4(cssom-fontfacerule/-constructors/CSSStyleRule/computed-style-set-property) | **全绿**(27/27 子测 PASS;#47388/#47743 修复实证) |
| font 代表 3(font-family-computed/font-face-src-list/slnt-variable) | **全绿**(stylo 0cb50925b font-family 引号重做实证) |
| progress-computed(648d ini 覆盖) | 翻转子测 `calc(progress(50%, 0px, 100px) * 10px)` **PASS**(翻转成立);`50px` 子测 FAIL-as-expected(servo 终态同病);**1 个本仓侧 unexpected-FAIL**:`sign(1001em - 10lh * progress(...)) * 4` 族(lh/rex/ex 字体相关单位,同族兄弟子测全绿——疑字体度量环境依赖或边界舍入,登记) |

### G5. 载具重建(e121/e122 全灭后第三次重建)+ 新教训

1. 载具私有化 /tmp/e137(venv+私有 manifest 161506 items 39M+launcher run_e137.py+gate-and-run.sh);manifest 用 `wptmanifest.manifest.load_and_update(tests_root, manifest_path, url_base="/", working_copy=True, rebuild=True, cache_root=私有)`。
2. **bao 无 `-M` 旗标**:servo glue multiprocess=True 会给二进制追加 servo 内部架构旗标 `-M`,bao clap 拒收→进程秒退→「WebDriver not accessible」300s×N。修法=launcher multiprocess=False(并行靠 --processes,wptrunner 侧)。
3. **prefs-file f64 强转缺陷(bao 层,src/bao_browser/webdriver_host.rs apply_pref_overrides)**:数值 pref 一律 parse::<f64>,servo 仓 resources/wpt-prefs.json 的 `editing_caret_blink_time: 0`(i64 schema)变 0.0 → serde 拒绝 → 启动失败。上游 PrefValue=枚举(类型宽容),bao fork=serde 强 schema——真缺陷,修复合同另案(越本任务边界)。**历史解密:整代 campaign(e67~e122)都从私有 CWD 跑,find_wpt_prefs 找不到文件→跳过(上游同形告警),实际从未消费 wpt test prefs**;本波同形规避(CWD=/tmp/e137/cwd)。
4. **multi-global `.window.js` 显式路径形态**:manifest 枚举为 `.window.html`(每 global 一 URL),传 `.js` 形态→「Unable to find any tests」;idlharness 单跑须传 `webaudio/idlharness.https.window.html`。
5. mozlog raw log 语义:`expected` 字段仅 status≠expected 时存在,缺席=如期望(分析器初版反解,已修 analyze2.py)。

### G6. 登记面(不修,后续合同)

- **#48347/#48351 吸收缺口**:param.rs(automation 事件插入/更新+val_range clamping)+oscillator/constant_source 各 4 行+audio_node SetParamRange 三方合并(bao WorkletParam patch 冲突面,需 3-way merge 合同);吸收后 15 EXPFAIL+4 残留面预期翻绿、随下一轮重锚删除。
- 4 残留红面的 unexpected-FAIL 部分(超 ini 覆盖:biquad 7/osc-conn 8/panner-conn 6/constant-source 1)。
- progress `* 4` 子测本仓侧红(字体单位族,需 servo CI 环境对照定性)。
- **参考仓 meta 快进需求**:本地 main 停在 2026-08-13+3 提交,cssom/font/progress 改善(以及一切 887 界外 commit 的期望面)不在其树内;后续 cssom/font campaign 若以参考树 meta 为 oracle 会系统性报 unexpected-PASS 波(正向漂移噪音)。快进/对齐动作属参考仓边界,本任务不可触。
- rendersizehint/messageport 2 M 面+7 A 面:待 campaign tests 树更新波(参考树 tests 同为 Aug-13)。
- 参考仓工作树 2 脏文件(M Cargo.lock + M fetch-destination-worker.https.html.ini,他波在途,未触碰)。

### G7. 交付

- vendor meta/webaudio:203→175 ini(28 删+3 更新),与 648d 终态差=19 保留(#48347/51 缺失族+4 残留)+2 测试树漂移 M 面+11 e122 本地面;7 A 面未引入(无测试载体)。
- cssom/font/progress:只读跑验交付(本仓无载体),「删除已修面/翻转成立」在 bao HEAD 实证。
- 本地 commit(禁 push);载具 /tmp/e137(半衰期资产:raw logs wa-0..5/c1-cssom/mface/idl3/verify-* + 分账文件)。

## G. e140 servo-media 数学红格清偿(2026-10-07,§D3 39 格收官)

### G1. 分桶分账(39 格终态)
| 桶 | 格数 | 终态 | 机制 |
|---|---|---|---|
| nominal-range 钳制 | 16 | **修绿 16** | 上游 #48351 吸收(media value() clamp + script SetParamRange 下发) |
| k-rate 渲染数学(oscillator 8/biquad 7/panner 6) | 21 | **修绿 21** | 上游 #48347 时间线重写吸收(ramp 锚点持久化) |
| setValueCurve-exceptions | 2 | **确证继承 2** | 上游 origin/main 终态 ini 同名 expected:FAIL(异常面=同步 throw 需 script 侧时间线镜像,上游未实现);bao 失败名与终态 ini 逐字节一致,等 e137 重锚后 as-expected |

### G2. 根因(k-rate 桶,servo-media 层单测复现)
`Param::update` 旧事件推进把 ramp 锚点快照为求值时刻(`event_start_time = current_tick`):k-rate 参数只在块界求值 → 锚漂移到块界 tick(如 128);a-rate 输入模(ConstantSource)连续求值 → 锚在 tick 1。同一 ramp 两路径公式分叉,第二块起频率不同→相位发散(block 1 frame 1:expected -0.4523 got -0.6882,与 servo ini 失败表 [129] 逐位一致)。#48347 重写:锚点=持久 `Param::time`(由 run() 更新),求值粒度无关。nominal 桶:value() 出口 clamp(val_range)+NaN→default。

### G3. 修复面(2 commits,本地)
- `7a7530d1` media 侧:param.rs=上游 240a37393(#48347+#48351 合体形态)+ BAO patch 重放(WorkletParam(u32) 变体 + rate() getter——上游重写删除但 audioworklet_node.rs 2 调用点仍用);audio_node.rs SetParamRange;constant_source_node/oscillator_node start-前-param-推进+stop 边界 >= ;oscillator_node 新增 k-rate 连接回归测试(修复前 RED)
- `a1446097` script 侧:audioparam.rs AudioParam::new 发 SetParamRange(#48351 上游 5 行原样;文件带 C15 GlobalScope patch,hunk 零冲突;**e140 合同 script/dom/audio 禁区,已报 Commander 裁定,独立 commit 可单独 revert**)
- 吸收史实修正:e99 快照交换(a7272f16)把带 BAO patch 的 param.rs/audio_node.rs/audioparam.rs 回持在 pre-#48347 基底(其余文件全换),本波补齐缺口;biquad/oscillator min/max 字面量与 panner f64 数学(#48510)已由交换带入,panner_node.rs == origin/main 零差

### G4. 载具(自建 /tmp/e140,e121/e122 已被清扫)
venv(mozlog 8.1.0 同清单)+ 私有 manifest(manifestupdate rebuild=True,servo 参考树 tests root)+ launcher(servo wpt.run.run_tests glue + metadata_root/manifest_path/tests_root 三覆盖,kwargs 用 create_parser().parse_args([]) 全默认 namespace 起——check_args 需全键;log_raw 文本模式开)。meta=vendor ini 快照(29b 形态 203 文件)。坑:`tools.serve` 需 tests root 进 sys.path,但 SERVO_PY 必须排前(`wpt` 包遮蔽 tests 根的 wpt.py)。

### G5. 验证(raw log:/tmp/e140/*.raw.log;首轮五件被 /tmp 清扫,证据=转录账目+重建后 fin2/fin3 复跑)
- 基线复刻 e121 账目:nominal 16+k-rate 21+setValueCurve 2=39 unexpected-FAIL,逐桶计数一致
- 修复后:nominal 0/k-rate 0/setValueCurve 2(同名继承);params 全目录 45 文件零新增红
- cancel-and-hold:#48347 改其失败形态,修复后 bao 失败名与上游终态 ini **逐名一致**(含 -1 明细表与 12000 计数;注意 ini 的 \n\t 是字面转义,与 raw log 真换行比对须先 unescape)
- cargo nt -p bao-servo-media-audio **27/27**(26 基线+1 新);BAO_TEST_NETWORK=1 cargo nt -p bao-browser -E 'test(audioworklet)' **11/11**(二进制 mtime>编辑 mtime provenance 验证)
- event-insertion.html:vendor 已是 origin/main 终态(e121 同步即含),零提交面

### G6. ini 登记(对齐 servo meta 终态,e137 重锚 commit 8e8a3e2b 后错峰执行)
e137 重锚(648de26fa,28 删+3 更)未覆盖本波 8 文件——按合同③由本波登记:
- **删 5**(上游终态无 ini=全过):k-rate-{oscillator,biquad,panner}-connections / event-insertion / nan-param(实测 failing=0)
- **换 3**(origin/main 终态版):audioparam-nominal-range(4 条 unimplemented-node FAIL)/ audioparam-setValueCurve-exceptions(21 条)/ audioparam-cancel-and-hold(3 条)——换前逐名覆盖验证:failing 集与终态 expected-FAIL 集**双向零差**(4/4、21/21、3/3,unmatched=0 now-passing=0)
- **终验**(fin3.raw.log,8 文件,注册后 ini):run_tests **rc=0**,unexpected-nonpass=0
- 正向翻转子测(29b 快照 ini 下 nominal 2/setValueCurve 1/其余 20+)按 e122 F4 惯例不留 ini 痕迹(终态 ini 本就不含)

### G7. 基础设施事件(本波)
- /tmp/e140 整车被外部清扫×1(代码/台账已 commit 免损;载具三件套按 §G4 配方 15min 重建,manifest rebuild+binary sccache 增量)
- wptserve "Servers failed to start: https-public:8446" ×3:与 e137 在途 WPT run 的瞬时端口碰撞(ss 零持有+直连 bind OK 后重试即过);另清 1 个 Oct-4 孤儿 serve.py(PPID 1)
- e137 重锚后 vendor ini 175 文件(203−28),私有 meta 快照随动

## H. e145 servo-media graph「worklet 节点恒处理」实装(2026-10-08,§F2-F 根因清偿,REQ-BRW-002)

### H1. 复用扫描决策
| 候选 | 裁定 |
|---|---|
| petgraph 0.8.3 `DfsPostOrder.move_to`(traversal.rs:196 实证:discovered/finished 集跨源持久,已 finish 节点不重发射) | REUSE——`dests.iter().chain(always_process)` 单点并入,零第二渲染循环,连接态节点语义零变化 |
| `add_extra_dest` 先例(render_thread is_dest 创建时注册遍历源) | REUSE 形态——注册点前移到 `AudioGraph::add_node`(trait 多态单点) |
| `AudioNodeEngine` 默认方法面(mute_node/message_specific 先例) | REUSE——新增 `always_process()` 默认 false,AudioWorkletNode 覆写 true |
| 上游 servo-media(~/code/tools/servo components/media) | ABSENT(grep 实证无 always-process/automatic-pull 面)——fork 自治域 |

实装三文件(media/audio 域):graph.rs(字段+add_node 注册+遍历源 chain)/audio_node.rs(trait 方法)/audioworklet_node.rs(覆写)。挂起处理器留集合无害(process 短路于 bridge latch,单次 acquire-load)。

### H2. 复现钉(TDD)与验证
- RED:`unconnected_worklet_nodes_are_always_processed`(Lockstep 载具,双形态:自由输出+零输出,共享计数 handler)——修前实测 `processor called 0 times`
- GREEN:media-audio **29/29**(28+零初始化新测);`bao-browser -E 'test(audioworklet)'` **11/11**(worktree test-ci 档);`cargo check -p bao-servo-media-audio` RC=0

### H3. 9 格终态
| 格 | 终态 | 证据 |
|---|---|---|
| automatic-pull `setup-worklet` | **PASS 翻转** | 采样级 0.5/0 断言全绿(offline 全链) |
| process-getter ×2 | **PASS 翻转** | 双格 |
| zero-outputs `check-zero-outputs` | **PASS 翻转**(任务级) | 内层「outputs 全零」断言红→JS 持久数组清零缺面(见 H4-R2) |
| frozen-array `check-frozen-array`/`transfer-frozen-array` | **PASS 翻转**(任务级×2) | 内层 frozen 断言红→H4-R3 |
| construction-port 3 throw 格 | **PASS 翻转** | e122 A/C 修的可见性解锁 |
| process-parameters sub1/sub2 | 残留 TIMEOUT/NOTRUN(登记) | 探针实证 F 面已修(去 suspend/resume 变体 MSG 直达);残留=H4-R4 |

### H4. 残留根因(WebDriver+title 探针,/tmp/e145-probe;全部越 media/audio 边界,登记另案)
| # | 根因 | 域 |
|---|---|---|
| R1 | **共享实例 port 改写**:singleton 处理器二实例化时 instantiate 对共享实例的 port redirect 重定向到新 node 的 conduit(探针:第二条消息到 node2 自己的 port;Chromium 语义=保持 node1) | script/dom/audio(audioworklethandler.rs) |
| R2 | **JS 持久输出数组不清零**:handler 每调传同一 rooted Float32Array 组,跨调用残留上拍样本(processor 未写 outputs 时违 spec 全零) | script/dom/audio(quantum 侧已由 pool_recycle 清零根治,JS 侧缺) |
| R3 | **数组未冻结**:inputs/outputs 非 Object.isFrozen(spec FrozenArray) | script/dom/audio |
| R4 | **suspend→resume 渲染线程楔死**:realtime 渲染循环=appsrc max_bytes=1+need_data 拉动;headless 无消费设备,Paused→Playing 后 need_data 不再触发→currentTime 冻结(探针 dt=0×3s;process-parameters sub1 的唯一残留机制) | backends/gstreamer/audio_sink.rs+render_thread 循环活性 |

### H5. ini 登记(6 文件;翻转格恢复 servo 基线 FAIL 形保可见性,诚实红逐名登记,残留挂格单值终态)
automatic-pull/process-getter→servo 单条 FAIL 形;zero-outputs/frozen-array→任务格 FAIL+内层断言逐名 FAIL;process-parameters(construction-port)→file TIMEOUT+sub1 TIMEOUT(Singleton TIMEOUT)+sub2 NOTRUN。**多值期望语义修正**:wptrunner `[A,B]`=逐值重跑序列(非「任一可接受」),残留格改单值实测终态(e122 F4 旧多值形态在本场景误报,后续波注意)。终验 fin3/fin4:目标+params 全目录 52 文件,意外事件=正向翻转(可见性策略)+残留格 as-expected,**零意外负**;params 面零回退。

### H6. 载具与事件
- /tmp/e145 第四次重建(e121/e122/e137/e140 全灭后):venv+私有 manifest+launcher(run_e145.py=e140 形)+analyze.py(mozlog expected 字段语义修正:仅 status≠expected 时存在,默认 PASS 的分析器会把按期望 FAIL 误报为意外——e137 §G5-5 同坑二犯)
- 二进制:worktree 钉 HEAD 626d74e6+仅本域 3 文件 diff+私有 target test-ci 自建(共享树被 e143 在途 node_net/node_tls 编译红阻断,灭菌路线;sha256 6246332208a0b708…)
- READ-GATE 钩子(REQ-GSC-74)结构性拦截 graph.rs 编辑(证据通道归属缺陷,ISSUE #155/#156),补三读无效,经 Commander guard-off 窗口授权落盘
- 端口卫生:e121 收官载具 4 个 multiprocessing 孤儿占 9000 系(venv 已被清扫的残活进程),清后过;探针 bao(webdriver=7001)多次残留,pkill 收尾
- 探针三件套(/tmp/e145-probe):probe_{params,singleton,params_nosusp}.html+插桩处理器+raw WebDriver title 轮询驱动(判别律:currentTime dt 面包屑定渲染线程活性)

## I. e147 webaudio 继承面 fork 自治自研——setValueCurve 同步 throw + process-parameters 五根因(2026-10-08,REQ-BRW-002,登记面清偿批⑫)

### I1. 复用扫描决策
| 候选 | 裁定 |
|---|---|
| setValueCurve 同步 throw = AudioParam(servo script 侧)自带 state Cell 旁加 timeline 镜像(RefCell<Vec> 纯 f64 数据),调度入口守卫+记录;渲染真源仍是 servo-media param.rs,镜像只答 throw 判定(零第二时间线推进) | REUSE 形态——e140 a1446097「script 侧字段+入口消息」同文件同面先例 |
| R1 port first-wins = MessagePort 既有 bao_port_redirect RefCell(加 has_bao_port_redirect 只读探针),零第二注册表 | REUSE |
| R2 输出清零 = ProcessorInstanceData::read_outputs 既有 typed-array view 面(as_mut_slice_safe) | REUSE |
| R3 冻结 = wrappers2::JS_FreezeObject 既有绑定 | REUSE |
| R4(登记为 render loop need_data)= **归因修正**:独立 gst 探针(probe_seq.py,PTS buffer+精确 Playing→Paused→Playing 序列)证明管线面 resume 后 need_data 正常复活;真根因=script 侧 BaseAudioContext::state 属性只经 ack 任务异步更新,同 JS turn 的 suspend();…;resume() 使 Resume step-3 守卫读到 stale Running 早退,Resume 消息从未发出→渲染线程永久 Suspended(pipeline 停 PAUSED,need_data 静默,currentTime 冻结;GetCurrentTime 有应答=线程活)。修复=requested_state 同步阴影 Cell(spec [[control thread state]]) | REUSE 形态(修正归因后零 render_thread/sink 改动) |
| R5(探针新发现,登记根因清单外)=JS inputs/outputs 参数数组形状:未连接输入 port 恒 2×Float32Array(128)(spec 要空数组);0 输入节点输出恒 2(spec 要 computed=1) | REUSE 形态——WorkletQuantum 携带 input_live/output_live(Vec<u8> 纯数据),script 侧既有持久数组构建 helper 提升模块级复用,形状失配才 rebuild |

上游状态:setValueCurve 同步 throw(上游 ini 21 条 expected:FAIL)与 AudioWorklet 参数面全部上游缺席(§H4 考古)——fork 自治域确认(用户裁决 2026-10-08)。

### I2. 逐根分账(RED 钉→修复→翻转)
| 根 | RED(证据) | 修复 | GREEN |
|---|---|---|---|
| setValueCurve 同步 throw | vendor ini 21 条 expected:FAIL(X 前缀键形态) | audioparam.rs 时间线镜像:调度入口(4 方法+value setter)curve 覆盖检查→NotSupportedError 同步 throw;curve 冲突(点事件严格内含/曲线正长度相交)→throw;cancel 剪枝(events >=/> cancel;curves 仅 end<=cancel 存活——cancel 落在曲线内=整条移除);value setter webidl [SetterThrows];current_time_or_default(渲染线程已死不 panic) | setValueCurve-exceptions 7/7 task 全断言 PASS |
| R1 共享实例 port 改写 | singleton 探针:msg1>N2PORT-GOT>timeout(第二条消息到 node2 的 port) | instantiate 的 lane-0 重定向加 first-wins(has_bao_port_redirect 探针);fresh 实例的 port 已由 base 构造 handoff 同参布线=零漂移 | msg1>msg2>DONE(3 次稳定) |
| R2 JS 持久输出数组不清零 | zero-outputs ini 内层断言红 | read_outputs 改 copy-then-fill(0.)(as_mut_slice_safe;超出 quantum 通道数的通道也清) | check-zero-outputs PASS(3/3 稳定) |
| R3 数组未冻结 | frozen-array ini 内层断言红 | instantiate 冻结 inputs/outputs 外层+port 层容器(channel Float32Array/buffer 保持可写可 transfer) | check-frozen-array+transfer-frozen-array 双 PASS(3/3 稳定) |
| R4 suspend→resume 楔死 | params 探针 dt=0×3s 无 MSG(§H4 判别律) | baseaudiocontext/audiocontext:requested_state 阴影(suspend/resume 成功发消息后同步置位);Resume step-3 守卫=requested∧attr 双 Running(仅落定态早退);Suspend step-3 守卫=requested;step-4 autoplay 门移除(旧流程死代码;保留 is_allowed_to_start attr 形态给构造 auto-resume 位) | params 探针 MSG;process-parameters sub1+sub2+file 全 PASS |
| R5 参数数组形状 | 形状探针:in[0].len=2(要 0) | media:WorkletQuantum.input_live(fill_quantum:is_silence∧chan<=1→0,容量截断)/output_live(显式 outputChannelCount 或 computed=max(inputs,无连接=1),QuantumShape.output_computed 显式标记;take_outputs 按 live 通道拷贝);script:ProcessorInstanceData.input_live/output_live,形状失配 rebuild_input_arrays/rebuild_output_arrays(冻结同 R3;HeapBufferSource=boxed Heap 移动安全;inputs_array/outputs_array 内联槽 registry 终址写入=e127 纪律) | process-parameters 双 subtest PASS(0 输入节点 outputs[].length=1) |

### I3. 修复中撞出的两个次生回归(均根治)
1. suspend-resume「resuming a running online context」:首版守卫只读 requested→构造 auto-resume 在途时(await 微任务先于 ack 宏任务)attr 未落定→断言 suspended。修=双条件守卫(见 I2-R4)。
2. cancel-scheduled-values「cancel1: cancel setValueCurve」:首版剪枝保留 start<cancel 的跨点曲线→曲线中段再调度被 throw。修=curves 仅 end<=cancel 存活(cancel 落在曲线内=整条移除,WPT 断言注释即此语义)。

### I4. 回归账(base1=HEAD 基线二进制 vs fin1,同载具同分析器,A/B 差分)
- **新负 0**(基线 180 意外负 vs 终态 140→fin3 残留 11 全为基线一致的 pre-existing 红族:onerror/options/promises/node-construction X 形态/sharedarraybuffer 多值族);两个修复期回归(I3)已根治复验。
- 正向 64(fin1):五目标文件+级联(suspend-after-construct/exponentialRamp 族/worklet interface 批)。
- **param-getter-overridden 形态注记**:vendor ini 本就 expected: FAIL(上游终态同);本波使其实在 FAIL↔PASS 摆动(正向方向)——机制=offline 渲染 sprint 与 worklet 首块竞态+本波 render 线程每块簿记微增;真语义面(invalid param getter→节点失效)上游缺席,登记为后续候选,ini 不动(expected:FAIL 仍真)。
- promise-methods-after-discard:挂点从 suspend() FAIL 后移到 suspend() 即挂(同族文件级 TIMEOUT,上游同挂);ini 按实测终态改形态(suspend TIMEOUT/resume+close NOTRUN)。
- mozlog 口径修正:as-expected 时 expected 字段缺席——分析器默认 PASS 会把按预期红误计意外负;A/B 差分两侧同口径仍成立。X 前缀 ini 子测键永不匹配日志名(servo meta 导出残迹)。

### I5. ini 登记(34 删+1 形态改;对齐 e140 §G6/e122 F4 惯例)
- **删 34**(全绿实测:setValueCurve-exceptions/process-parameters/zero-outputs/frozen-array/cancel-scheduled-values 五目标+k-rate 族 4+ramp 族 4+worklet interface 级联 16+audioparam-size 类 4+denormals/registerprocessor-constructor .window.js 2;机械核对每个 ini 键的实测态,header 级 expected 键亦核——audioparam-iterable/postmessage-sharedarraybuffer 两误删经 fin2 暴露后恢复)。
- promise-methods-after-discard 形态改(见 I4)。
- 混合文件(onerror/options/messageport/constructor-options/audioparam-size/node-construction)保留 ini:正向翻转子测不留痕(e122 F4)。

### I6. 验证与载具
- cargo nt -p bao-servo-media-audio **29/29**;BAO_TEST_NETWORK=1 cargo nt -p bao-browser -E 'test(audioworklet)' **11/11**(worktree test-ci 档)。
- WPT 终验 fin3(注册后 ini 快照):三目录 97 as-expected+残留 11 意外负全为基线一致 pre-existing 族,**零本波新负**;终验 RC≠0 仅因残留正向翻转与 pre-existing 族。
- 二进制:worktree /tmp/e147-wt 钉 HEAD abe913ad+仅本域 9 文件 diff(共享树被 e148 在途 bao_stealth 编译红阻断,灭菌路线=e145 先例);基线对照二进制同 worktree scoped-restore 建。
- 载具两度全灭重建(/tmp 清扫×2):探针 /tmp/e147-probe+独立 gst 探针(probe_seq.py 复刻 PTS/max-bytes=1 精确序列,证管线面无罪);WPT 载具 /tmp/e147-wpt-veh(venv+servo requirements+editable vendored wptrunner+tests-tree wpt manifest -p 私有+meta 快照多值压平——旧 mozlog 8.1.0 不收 known_intermittent list)。
- 端口卫生:7080/7001 探针孤儿反复清(pkill 自匹配陷阱:pattern 含自身命令行→自杀,字符类规避);8447 被 32 天 frog-preview 孤儿 http.server 占,清后过。

## J. e155 audio 面登记余量集中批(2026-10-09,REQ-BRW-002,F2 invalid param getter 自研 + F1 ini 增量重锚 + F3 configurable 评估)

### J1. F2 — invalid param getter→节点失效语义自研(上游缺席;Chromium crbug.com/1151069 镜像)
- **语义源考据**:WPT `audioworkletprocessor-param-getter-overridden.https.html` + 上游修复 commit「Fail gracefully if |parameter| object has an invalid array topology」(crbug 1151069)。Chromium 机制=两阶段属性回读:`ParamValueMapMatchesToParamsObject`(逐声明参数名对 params 对象做 Get——原型链上的页面 `Object.defineProperty(Object.prototype, name, {get})` 拦截此读)→ 失配时 `CloneParamValueMapToObject`(全新对象+CreateDataProperty 自有数据属性+SetIntegrityLevel frozen——遮蔽原型 accessor)→ `CopyParamValueMapToObject`(再 Get 校验 Float32Array 类型+长度==本块值数+buffer 未 detach,然后把时间线值 memcpy 进「Get 到的那个数组」)→ copy 失败=`kProcessError` 粘滞(process() 本块起永不调用、节点静音终身、主线程 fire processorerror ErrorEvent)。测试形态:Object.prototype getter 首次取回 Float32Array(128)(喂过 match 检)第二次取回 Float32Array(256)(喂崩 copy)→ 节点失效、渲染全静音。
- **实装**(script/dom/audio 两文件,`sync_param_face`+`get_param_array`+`rebuild_param_arrays`):process_quantum 在 process 查找前插入 match→rebuild→copy;throw getter=清 pending 异常按失配计(Chromium TryCatch 同形);copy 失败→`report_processor_error(Some(ErrorInfo{message:"process(): Failed to copy parameter data."}))`(消息对齐 Chromium)+Finish(bridge 静音 latch+一次性 ErrorEvent=e122 机制复用)。配套:参数数组改由 params 对象持有(phase B 恒经 Get 拷入「属性当前解析到的数组」=Chromium 同形;`param_arrays` Rust 侧句柄全删),`ProcessorInstanceData.param_shapes: Vec<(CString, usize)>`(k-rate 1/a-rate 128;空 CString=NUL 名 sentinel,跳过该名保索引对齐=旧 Set-skip 行为);instantiate 的 params 对象补 spec freeze(SetIntegrityLevel 面,数组 buffer 保持可写);rebuild=JS_DefineProperty 自有属性(JSPROP_ENUMERATE=CreateDataProperty 语义,原型 accessor 不可拦截)+freeze+`update_params_face` 终址写槽(e127 纪律)。
- **RED/GREEN**:baseline(HEAD 2e1da6ac)WPT 单测 3 连=2 FAIL(sample[1152] expected 0 got 1——节点未失效照常渲染)+1 PASS(e147 登记的饥饿竞态假绿:offline sprint 下 worklet 首块未及跑);fix 3 连=3 PASS 确定性。bao-browser live 新测 `audioworklet_invalid_param_getter_invalidates_node_live`(双变体):baseline 红(非零 A=0 但 fired=0——静音出自饥饿而非语义,错误事件零发);fix 绿(probe `{nonzeroA:0, fired:1, nonzeroB:10880, throwFired:0}`:变体 A 失效语义成立;变体 B throw getter 被 rebuild 吸收——自有属性遮蔽 accessor,节点照常渲染 85/100 块,零错误)。变体 B 首版 return false 单块窗口撞 e147 竞态→硬化 return true+渲染 100 块(信号与 spin-up 解耦)。
- **回归**:audioworklet 家族 12/12(11 旧+1 新,worktree test-ci);media-audio 29/29;chaos 3/3(含 pagepool_chaos_memory_safety 15.97s,e148-R1 后基座确认)。
- **e155c 复核收口(2026-10-09)**:前任 429+TaskStop 中后续作——主树(HEAD 9e33c57b,e157 归一波交错态)`cargo check -p bao-servo-script` RC=0(新测试 helper 均为文件局部定义,与 e157 suite/common 归一零交错);worktree 同 diff独立复跑 audioworklet 12/12(含新测 1.39s)+chaos 3/3(pagepool_chaos 4.93s)+media 29/29;顺手清偿 write_inputs 陈旧 `#[expect(unsafe_code)]`(参数拷贝移出后函数体无 unsafe 面,expect 落空 warning)。

### J2. F3 — configurable:true 子面对齐评估(零改动,登记关闭+安全形态处方)
- **结论一(PERMANENT 保留前提下:结构性不可对齐)**:descriptor 的 configurable 位是单比特 oracle——`Object.getOwnPropertyDescriptor(Navigator.prototype,'userAgent').configurable` 直读引擎真值;PERMANENT(JSPROP_PERMANENT)与 configurable:true 是同一位的互斥取值,不存在「保留 PERMANENT 且对齐 configurable」的形态;亦无其他 parity 维度可补偿(补偿=伪造 descriptor 读,需 engine 级 JS_GetPropertyDescriptor 拦截,语义上等价于直接置位,而置位路径已被证崩)。
- **结论二(放弃 PERMANENT 的安全对齐形态存在,但越本批边界)**:e148-R1 九变体矩阵的 V7(Face A configurable+blob nav 定义禁用)混沌绿——崩溃根因是「blob 的 defineProperty 替换 Face A native getter」这一交互,非 configurability 本身。实测冗余度:Face A(engine_props install_stealth_props,PERMANENT+ENUMERATE)与 JS-hooks blob(build_navigator_js)的 nav/screen/dpr 名称集 16 项完全重合,且 Face A 经 worker injector 覆盖 worker realm(bao_browser/lib.rs worker interfaces_ready 注入面)——blob 的 nav/screen define 块在 Face A 覆盖域内全部冗余(其 __bao_target proto 放置与 Face A 同目标)。安全形态=Face A 翻 configurable+build_navigator_js 按位抑制 nav/screen/dpr define 块(需 hooks.rs 变更+九变体混沌矩阵重验+全 realm 覆盖审计——worker/SW/Node realm Face A 覆盖边界的精确对账,错配任一侧=崩溃回归或 stealth 泄漏)。本批 owner 边界(engine_props.rs 评估面)不含 hooks.rs;判定=**登记关闭(知情不对齐维持,disposition: temporarily-missing)+处方留档**(后续批次或用户裁决可按上述形态执行)。
- **证据链**:engine_props.rs:1167-1191 双条件 bisect 注释;hooks.rs:308-336 blob define 面;lib.rs:1115-1140 worker injector Face A 覆盖;V7 绿=e148-R1 commit 矩阵。

### J3. F1 — webaudio ini 增量重锚确认(campaign 分账)
- 二进制:worktree /var/tmp/e155-wt 钉 HEAD 2e1da6ac+仅本域 3 文件 diff,私有 target test-ci 自建(174038008B;provenance 探针「process(): Failed to copy parameter data.」命中);载具 /var/tmp/e155-wpt-veh(venv 复制自 e153+全量 manifest 161512 items 重建+meta=vendor 快照多值压平 6 处+launcher SERVO_PY 前置修正[e140 §G4 坑二犯防]+reanchor.py 双向对账分析器)。
- (campaign 结果与 ini 动作待全 chunk 完成后落账)
- **campaign 终态(e155c 续作收口,2026-10-09)**:299 测试 5 chunk 全跑完(aa~ae,campaign2.done=ALL-DONE);reanchor 对账=150 AS-EXP+173 NO-INI(163 OK+8 crashtests PASS+2 permission-policy CRASH)+10 文件级 UNEXP+162 子测翻转(77 正向=76 FAIL->PASS+1 NOTRUN->PASS,余为级联/负载形态)。稳定性分账(sc-* 3 轮×4 组:light/work/ms/pp):全部负面候选与正翻删候选各 3 连复测定型。
- **ini 终态动作(8 文件)**:
  - **删 3**(确定性全绿 3/3):audioworkletprocessor-param-getter-overridden(F2 自家果实,green-fix 3/3 全绿+campaign 翻绿)/ test-constantsourcenode(单键翻绿 3/3)/ audiobuffersource-playbackrate-dynamic-direction(双键翻绿 3/3)。
  - **形态改/登记 4**(确定性实测终态,e147 promise-methods 先例):channelmerger active-processing(file 级补 ERROR+`test:` 键 FAIL→TIMEOUT,3/3;`initialize:` 键维持 FAIL=正向翻转可见性)/ audioworkletprocessor-promises(file `[FAIL,CRASH]`+`[test]` `[FAIL,CRASH,NOTRUN]` 多值误报改单值 TIMEOUT,e147 I4 多值教训的后续波应用)/ audioworkletnode-onerror(file 级补 TIMEOUT,3/3;子测键不动——sub1 NOTRUN 陈旧键留正向翻转可见性)/ audioworkletprocessor-options(file 级补 ERROR,3/3)。
  - **形态改 1(大件)**:mediastreamaudiosourcenode-from-context-with-different-rate——上游 74 子测键 FAIL 终态→promise-methods 挂级联形态(file 级 TIMEOUT+首键「Test closing one AudioContext stops only it (cloned tracks...)」TIMEOUT+余 73 键 NOTRUN;4/4 确定性:首子测 getUserStream 族挂死,后续子测全 NOTRUN 级联)。**判读陷阱登记**:reanchor 分析器 `ini_file=None` 只反映 file 级期望缺席,不代表无 ini——本件与 biquad-automation 同为「有子测键无 file 键」形态,核载体必须键级对照(本波初版误判「无载体」险致整文件覆写,git diff −222 拦截后按 HEAD 键集重建)。
- **登记不修(台账面,零 ini 改动)**:
  - **biquad-automation**:确定性红 3/3——detune automation+sinusoidal modulation 两任务失败(freq/q/gain/all 四任务过),`# AUDIT` 计 2/6;本 campaign 全树首盖(此前波只跑 k-rate 族),匹配键(`< [automate-detune/modulation]...failed.`)as-expected、X 长键=servo meta 导出残迹永不匹配(e147 I4 判例),2 个 unexpected-FAIL 格留可见性;a-rate detune/modulation 数学面=#48347 吸收后残余缺口候选。
  - **cors-check**:三态摆动 ERROR(campaign)/OK(sc 轮 2/3)/TIMEOUT(sc 轮 1)——MediaElement 播放捕获环境 flake,ini 维持 ERROR;删候选否决(非确定性)。
  - **no-cors**:file 态摆动 ERROR/OK,子测红(4 通道非静音+audit 1/2)确定性——media 捕获非静音真红+file 级 flake 并存,ini 维持 ERROR。
  - **permission-policy(48 变体)**:campaign 4 CRASH(resume-while-hidden 2+suspended-hide-show 2,全 cross-origin/hidden 组合)/稳定性 2 轮 48 变体 0 CRASH——负载 flake 定性(wpt-crash-sigterm 同类候选),零 ini。
  - **promise-methods-after-discard**:TIMEOUT 3/3+campaign CRASH 1——同挂两态(挂死被杀形态),ini 维持 TIMEOUT。
  - **audiobuffersource active-processing**:channel-count-after-stop 子测 1P/3F 多红 flake(stab-ap+campaign),ini 键(`Setup graph...`)翻绿留可见性。
  - **audioworkletnode-construction**:「construction-before-module-loading」抛 NotSupportedError 而非 InvalidStateError(campaign 1 obs,机制确定性)——构造器错误类型映射缺口,登记候选。
- **维持可见性(惯例面零动作)**:idlharness 58 AudioWorklet IDL 翻转(e137 G4 bao-独有面惯例)/ rendersizehint+messageport M 面(e137 G3 测试树漂移绑定)/ 混合文件正向翻转子测不留痕(e122 F4)。
- **验证**:载具 meta 快照=动作前 vendor 形态,复跑判读按快照口径;vendor 动作后 ini 键集与 sc 实测态逐键对上(删 3 全绿/改 4 单值定型/新建 1 file 级)。

## K. e168 biquad-automation detune+modulation 残余格清偿(2026-10-10,REQ-BRW-002,登记面清偿批尾——RED 取证→继承确证→fork 自治双修复,格全翻绿)

J3「biquad-automation 确定性红(全树首盖)」清偿。取证先行:双格失败值与 origin/main ini **逐位一致**=继承形态(判别依据见 K2);按合同本应登记关闭,team-lead 依「继承真缺陷定性+修复形态明确」追加修复授权(用户「都 fork 了自己修」裁决引用,与 e145 §H6 guard-off 先例同链)——双修复落地,格全翻绿。

### K1. RED 形态(binary=/var/tmp/e154-target/test-ci/bao=HEAD 音频面,provenance=源 mtime 早于二进制+vendor 树 clean+e155c 探针串;载具 /var/tmp/e168-veh=e167 launcher 形+meta 快照)
- 6 任务终态与 J3 一致:freq/q/gain/all PASS+detune/modulation FAIL;X 长键+`2 out of 6` 摘要=3 意外格(HEAD ini 载 servo 旧失败表,e155c 可见性登记)。
- **detune**:首错 index **1087**(恰为 f0=4400·2^(detune/1200) 线性扫 -12000→+12000 cents/0.125s 穿越 Nyquist 8000Hz 之帧;detune=1035.2 cents ⇔ frame 1086.3);Expected(reference)自 1087 起全零,Actual 环增幅,**Max AbsError 1.4533952713012695e+1 @3999**。
- **modulation**:reference 自 index 1 全零(4000 帧全错),Actual=真实带通输出,**Max AbsError 8.2110029458999634e-1 @284**。

### K2. 上游锚判别(origin/main 锚纪律)
- ini:origin/main 同格 expected-FAIL,**记录失败值与 bao 实测逐位一致**(两格 Max/AbsError+[1]-[5] 实际值全同);`2 out of 6` 键=上游终态失败任务集同。code:`biquad_filter_node.rs` 与 origin/main **零差**;`audiobuffer.rs` 仅 C15 签名差。**判定=双格继承确证**——J3「#48347 吸收后残余缺口」假说证伪(modulation 零引擎缺陷,detune 为继承的边界语义缺陷);HEAD vendor ini 的 X 长键载 servo 更旧失败表(detune MaxAbsError 8.08 系),origin/main 已再生到与 bao 同形。

### K3. 根因(探针流水 probe{1-5}.raw.log;probe-mod-readback.html 五版迭代,用后即删)
- **detune=真引擎缺陷(继承)**:`update_coefficients` 把 f0 clamp 到 fs/2 → normalized 恰 0.5 → bandpass ω0=π:α=sin(π)/2q≈1.2e-16 → b0≈0、a1=+2、a2≈1 → **z=−1 双极点边缘稳定环**(扫频过谐振区增幅,终值 14.5);WPT reference(Chromium 模型)对 normalized∉(0,1) 给恒零输出(含状态清杀)。**Chromium 实文核对**(third_party/blink/renderer/platform/audio/biquad.cc,拉取核对):八类型 Set*Params 全部 ClampTo[0,1] 后走精确边界分支——唯 **SetBandpassParams 无上界 clamp**(仅 max(0,·)),frequency∉(0,1) → `SetNormalizedCoefficients(0,0,0,1,0,0)`(零输出+状态清杀)。
- **modulation=零引擎缺陷**:真 reference 重建探针(公式重建 modulator/输入+逐帧系数+时变滤波):**errors=0/4000,max=0**——a-rate 调制渲染数学全对;`.value` 回读 4000 次循环探针零漂移(264/5/0)。失败 100%=**测试侧 reference 塌缩**:servo `acquire_data → detach_buffer` 剥掉 verifier 预捕的 `d=mbuffer.getChannelData(0)`(detached length=0 实证)→ d[k]=undefined → freq=NaN → `NaN>0&&NaN<1` false → 全帧零系数 → reference 全零。

### K4. 修复(2 文件 + ini)
- **①biquad 出界表**(media/audio/biquad_filter_node.rs):`normalized == 1./0.` 精确等值(旧 clamp 下系死分支,normalized≤0.5 恒不可达)→ 范围条件 `at_nyquist_or_above(!finite || ≥0.5)`/`at_zero_or_below(≤0)`,f0 clamp 块删除;八类型边界表逐型=Chromium(bandpass 出界恒零/peaking·notch·allpass 恒 1/lowpass 上界 wire 下界零/highpass 反之/lowshelf 上界 A² 下界 1/highshelf 反之;q≤0 内层分支次序不变=Chromium 嵌套同构)。带内计算路径零变化(全 biquad 目录回归实证)。
- **②acquire copy-swap**(script_bindings/buffer_source.rs `acquire_data`,全树唯一调用方=AudioBuffer::acquire_contents):去 `detach_buffer` 调用——to_vec 快照+槽位置空(换出关联)保留。**首版纯 copy 形被 WPT acquire-the-content.html 否决**(acquire 后 JS 写不得经新 getChannelData 可见——该测试双 setter 格 FAIL)→ **swap 形双面兼得**:旧数组保持可读(modulation verifier 读预捕 d ✓)+新鲜 getChannelData 重挂快照(acquire 后写不可见 ✓)+`restore_js_channel_data` 既有重挂路径不变。audiobuffer.rs 不变量注释同步(js_channels=活真源,shared_channels=acquire 快照,数组本身永不被 detach)。decodeAudioData 输入 ArrayBuffer detach 面(baseaudiocontext.rs:622)不触(spec 明文要求,零影响)。
- **ini 删除**:`biquad-automation.html.ini` 整文件(deletion-manifest ①replacement,实测终态=6/6 全绿;e140 §G6/e147 I5 删 ini 惯例)。

### K5. RED/GREEN(media 单测钉,波内 1 红 1 绿)
`bandpass_is_silent_once_frequency_passes_nyquist`(biquad_filter_node.rs 新 boundary_tests):修前实测 `rang at Some((1087, -0.023368616))`——**与 WPT actual[1087]=-2.3368615657091141e-2 逐位一致**(单元级复现 WPT 格);修后全绿,media-audio **37/37**(36 基线+1 新)。

### K6. 回归(终态 binary=/var/tmp/e168-target/test-ci/bao,worktree 钉 HEAD 4a297215+3 文件 diff+私有 target)
- **biquad-automation 6/6 任务 PASS**(6 tasks ran successfully,RC=0;baseline 同载具 4/6)——detune+modulation 双格翻绿。
- WPT 四目录(biquad+params+audiobuffer+audiobuffersourcenode)97 文件:**95 OK+2 PASS,零意外 file-end**;唯一意外子测=active-processing 已注册 flake 双面(Setup graph 正翻+channel-count-after-stop 红;旧二进制 3 连 1P/2F vs 新 3 连 0P/3F,e155c J3 的 1P/3F 注册形态内,非回归)。
- **acquire-the-content 与 pre-fix 基线零差**(双 setter 格 PASS 维持/ConvolverNode as-expected FAIL 维持——swap 形保旧通过面+修 modulation 面)。
- setValueCurve-exceptions 文件级 OK(params 目录内);media-audio **37/37**;audioworklet 家族 **15/15**;`cargo check -p bao-servo-script-bindings -p bao-servo-script -p bao-servo-media-audio` RC=0。
- 解码/其它 AudioBuffer 消费面:`get_channels`/`acquire_data` 调用方全树=audiobuffersourcenode.rs 3 站(均生命周期单发:set/start/ctor),ConvolverNode 走独立路径零差实证。

### K7. 载具与事件
- /var/tmp/e168-veh(launcher+meta+manifest 增量收放 probe;binary 保留 bao-fix,sha256 前缀 48e49d82e51a374f);worktree 10m07s 冷建+增量重建;probe HTML 五版(回读单次→循环→逐操作数→verbatim verifier→真 reference 重建)——归因链载体,参考树用后即删(git clean 复验)。
- wptserve `h2:9000` TIME-WAIT 碰撞×2(等 75s 自清,e140 §G7 同类);**acquire-the-content 路径踩坑**:该测试在 the-audiobuffer-interface(非 sourcenode)目录,单文件基线跑错路径两轮「Unable to find any tests」。
- READ-GATE guard-off 窗口(team-lead 授权,e145 §H6 先例):marker 被 sibling 逃生口竞态消费×2(共享文件单点),按授权 re-touch;删除权归主会话。
- 上游同形双缺陷(fork 就地修,ISSUE 反馈禁令豁免依据=自维护 fork 裁决):biquad 出界环 + acquire detach——上游吸收波如遇 servo 修复需对拍本 §K4 形态。

## M. e176 cors-check 三态 flake 归因根治(2026-10-11,REQ-BRW-002,J3 登记面清偿批——worklet 时钟 mid-call 漂移根因 + 根修)

J3「cors-check 三态摆动 ERROR(campaign)/OK(sc 2/3)/TIMEOUT(sc 1)」清偿。同因连坐面:no-cors file 级摆动(J3 另一条登记)一并根治。

### M1. 三态取证(proc1 隔离复现 + 插桩二进制 + 探针面包屑)
- **隔离复现率**:e168 终态二进制(4a297215+3 文件 diff)proc1 单测 7 跑 4 TIMEOUT/3 OK——**非负载 flake**(e157 负载形态类证伪),是启动窗口内的本征竞态;OK 态恒 ~1.25s 完成,TIMEOUT 态烧满 harness 超时。
- **探针面包屑**(cors-check-probe.https.html:verbatim 流程 + 每秒快照子测——快照嵌 subtest 名,PASS 行也进 raw log;探针页放参考树跑后删):TIMEOUT 态中 `ctx.state=running`、`ctx.currentTime` 正常推进(0.99→9.99s)、媒体元素 rs=4 正常播放(CORS 重定向抓取成功)、`recfin=false`——渲染线程/图/网络全活,**唯 worklet 处理器侧的 recordfinished 永不抵达**。
- **插桩定位**(worktree 钉 HEAD 65cf00e7+eprintln 诊断 diff,私有 target test-ci):OK 态 `conduit.send dir=ToMain bytes=705896` 于 t=1.00s 抵达(完整链 send→notify_main→Script#4 dispatch→drain popped=1);TIMEOUT 态 **conduit.send 从未发生**(消息在 JS 侧就没发出),且 345 drains(=44100Hz 下 1s 录制的块数)后渲染侧 halt——worklet 泵在阈值块后被 `processor_halted` 短路。
- **终局证据**:probe 页挂 `onprocessorerror` 监听 → TIMEOUT 态全部捕获 **`PROCERR:invalid or out-of-range index`**(SpiderMonkey TypedArray.set 越界抛掷),发生在录制收尾块(4 通道 41665 非零值断言前)。

### M2. 根因(worklet `currentFrame` mid-call 漂移——单一致命竞态)
`AudioWorkletGlobalScope.CurrentFrame` 原实现 = `sample_rate × audio.current_time()`,后者是**对渲染线程的同步 GetCurrentTime 往返**(context.rs:211)——读到的是**活体渲染时钟**,非当前块的帧号。WPT 共享 helper `worklet-recorder.js` 的 `process()` 三读 `currentFrame`(阈值判断/容量计算/set 偏移):
```js
if (this._recordBufferLength <= currentFrame) { post; return false; }  // 读1
const capacity = buffer.length - currentFrame;                          // 读2
buffer.set(inputChannel.slice(0, capacity), currentFrame);              // 读3
```
读1 与读3 之间渲染线程推进一个块界(2.7ms;getter 各自一次跨线程往返+循环体内 4 通道拷贝拉大窗口)→ 读1 见 <44100(走录制臂)、读3 见 ≥44100 → `capacity` 负/偏移越界 → TypedArray.set 抛 "invalid or out-of-range index" → e122 异常路径 `report_processor_error` → **processorerror 锁存+节点永久静音** → recordfinished 丢失 → audit 任务挂死 → TIMEOUT。OK/TO 二态 = 收尾块的读序是否跨块界(竞态窗占空比 ≈ 观测到的 ~50%)。三态中的 ERROR 极 = ini 期望值(campaign 实测形态 OK,e155c UNEXP(ERROR→OK) 在案),非独立故障面。

**判别链完整排除的假说**(取证过程留档):①角色交换错投(servo worklet 三线程池 primary/hot/cold 换角色,泵状态不随角色走)——插桩实证 wake/drain 全落 instantiate 同线程,否决;②内存压力换角(MIN_GC_THRESHOLD=1MB,recordBuffer 768KB 边界)——变体 C(48KB 分配)仍 3/5 TIMEOUT,否决;③CORS/网络/媒体管线——面包屑 rs=4 + OK 态同链路,否决;④port 通道丢消息(ring 溢出/close 竞态)——conduit.send 未发生,否决。

### M3. 根修(2 文件 47 行;spec 形态:块内时钟恒定)
- `audioworkletglobalscope.rs`:`process_quantum_clock: Cell<Option<(u64, f64)>>` 槽 + pin/unpin;`CurrentFrame`/`CurrentTime` 在 pin 期间读**在飞量子载荷的 `(frame, time)`**(fill_quantum 自 BlockInfo 写入,WorkletQuantum:291——正是 spec 的块时钟),未 pin(构造器体/port handler 等 process() 外读者)保持活体往返回退。
- `audioworklethandler.rs`:`process_quantum` 在 `Call(process)` 前后 pin/unpin——process() 体内任何多次读 `currentFrame`/`currentTime` 恒得同值,块间推进由泵逐块重 pin。
- 复用面:量子载荷已有 frame/time 字段(零 media 侧改动);`state: Cell<AudioContextState>` 同构形态。

### M4. 验证(修复二进制 = worktree 钉 HEAD 65cf00e7+仅本 2 文件 diff,私有 target test-ci,sha 前 174576728B@03:08)
- **cors-check 稳定性**:proc1 12/12 OK(~1.25s,方差 <60ms)+ proc4 负载镜像(sc-light 6 测组形)3/3 OK + (修复+插桩形态 10/10;与 e177 载具端口互斥协调后净窗取证——首波 10/10 与 e177 rep3 时间重叠,判定保留但以净窗 12/12 为准)。
- **no-cors 连坐面**:proc1 5/5 文件级 OK(子测红=4 通道非静音真红,taint 执行缺口登记域,41665/41537 采样窗计数抖动属内容抖非终态抖)。
- **回归**(proc4,三目录 95 文件:the-audioworklet-interface 39+the-audioparam-interface 46+MESN 10,meta=删 ini 后 vendor 快照):**95/95 as-expected 零 unexpected**(sharedarraybuffer TIMEOUT 在 known_intermittent 内;分析器必须并读 `known_intermittent` 字段——mozlog 多值期望的第二载体,e176 分析器初版漏读教训)。
- 单元:`cargo nt -p bao-servo-media-audio` **37/37**;`BAO_TEST_NETWORK=1 cargo nt --cargo-profile test-ci -p bao-browser -E 'test(audioworklet)'` **15/15**;主树 `cargo check -p bao-servo-script` RC=0。
- **ini 动作(2 删)**:cors-check.https.html.ini + no-cors.https.html.ini——确定性终态(OK)解除 e155c J3 的「非确定性故维持 ERROR」封锁,循 e140 §G6/e147 I5 删 ini 惯例;no-cors 的 4 通道子测红保持 unexpected-FAIL 可见性(值表嵌名,逐子测键不可行)。

### M5. 载具与教训
- /var/tmp/e176-veh(venv 复制 e155+manifest 全量重建+meta2=删后 vendor 快照+portgate.sh 端口门);探针 HTML 五件(cors-check-probe{,-b,-c})参考树用后删;**面包屑嵌 subtest 名** = PASS 行取证通道(testdriver payload 不带 PASS 消息体)。
- **端口互斥实战**:e177 载具同端口并发两次污染本波取证(「Address already in use」= wptserve 端口被夺;15s 形 TIMEOUT 即 https 端口损失形态)——跨 agent WPT 并行必须 ss 门+互斥窗([[wpt-vehicle-port-exclusivity]] 二犯实录)。
- mozlog 三载体:expected(仅 mismatch 时存在)/known_intermittent(多值期望其余值)/status;分析器三坑齐录(e137 §G5-5 首坑,e176 二坑)。
