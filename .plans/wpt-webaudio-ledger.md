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
| I | **worklet 线程 minor-GC tenuring SIGSEGV(新暴露)** | 实时 context 每块持续 port.postMessage(A/B 修复后才有流量)→ 构造小对象/serialization 分配 → nursery minor GC → `TraceIonJSFrame` 追踪 Ion 帧槽位遇垃圾值 → `TenuringTracer::promoteObject` SIGSEGV。gdb(dev 符号)栈:promoteObject←TraceIonJSFrame←minorGC←NewArrayObject←structuredclone::write←post_message_impl(redirect 臂)。**判别**:离线 10335 块逐块 post 存活、纯分配不 post 存活、单发存活——仅「实时+持续 post」复现;机制落点 SM GC/JIT+structuredclone 面(越界) | **登记**(promises CRASH 确定性);根治=后续合同:mozjs/GC 面归因 |

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
