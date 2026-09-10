# C15 实现前置侦察报告 — servo 音频栈 + worker Audio 载体现状(只读)

- 任务: REQ-BRW-004 C15 实现波设计输入(用户裁决 2026-09-09: 重核 Chrome 基准后再定 worker AudioContext+噪声路线)
- 执行: 只读侦察,零改盘;本文件为唯一落盘
- 日期: 2026-09-09

---

## ★ 核心结论先行(TL;DR)

1. **Chrome 基准重核结果 = 阴性**:Chromium main 源码 `OfflineAudioContext.idl` / `AudioContext.idl` 均为 **`[Exposed=Window]`**(逐字引证见 §3);Web Audio spec 同为 `[Exposed=Window]`;WebAudio WG issue #2423(worker 暴露)自 2016-11 至今 **Open 未落地**。**Chrome 的 DedicatedWorker 不暴露 OfflineAudioContext / AudioContext / AudioBuffer / AnalyserNode——一个都没有。**
2. e26 考古记录里「Chrome 同无 worker AudioContext」的原判定**经查证为真**(不是未查证猜测);真正未查证的是被推翻前的「N/A 建议」链路。用户裁决要求的重核已完成,结论支持「worker 不暴露 = Chrome 对齐」。
3. **指纹悖论**:若 Bao 在 worker 暴露 OfflineAudioContext,任何 worker 内 `typeof OfflineAudioContext !== 'undefined'` 探针可一发识别 Bao(Bao 独有暴露)——反指纹目标下,自建暴露比不暴露**更**危险。此为路线选择的最高权重事实,提交用户裁决。
4. servo 侧 audio 栈是**完整的 Window-only 实现**(24 文件,~5673 行):AudioContext(实时,gstreamer sink)/ OfflineAudioContext(**纯软件 OfflineAudioSink,零 gstreamer、零音频硬件**)/ AudioBuffer(getChannelData 噪声注入载体)/ AnalyserNode。worker 化只是 `[Exposed=Window]`→`[Exposed=(Window,Worker)]` + `&Window`→`&GlobalScope` 签名面,**不存在线程模型障碍**(渲染全在 servo-media 自有线程,见 §2/§6)。
5. bao_stealth 侧 worker 接线**已经全部就位**(W1a 波已做):worker realm 的 `install_stealth_props → inject_js_hooks`(含 audio 噪声 JS)已随 DEC-WK-001 worker-scope 回调链运行,且 hooks 已带 `typeof AudioBuffer/OfflineAudioContext` 守卫——**只要 servo 暴露,噪声自动生效,bao_stealth 零 patch**;若 servo 不暴露,守卫静默跳过,同样零 patch。
6. 附加缺口(与 worker 无关,影响指纹向量保真):servo `createDynamicsCompressor`/`createWaveShaper`/`createConvolver`/`createDelay` 在 `BaseAudioContext.webidl:41-50` **被注释掉未实现**——经典 audiofp 算法(Oscillator→DynamicsCompressor→destination)在 Bao 的 **Window 侧也跑不了**,现只有 Oscillator/Gain/BufferSource 路径。

---

## ① servo 音频栈现状

### 1.1 载体清单(vendor/servo,全部 `[Exposed=Window]`)

| 载体 | webidl(Exposed 行) | 实现 | 状态 |
|---|---|---|---|
| `BaseAudioContext` | `webidls/BaseAudioContext.webidl:18` | `dom/audio/baseaudiocontext.rs`(619 行) | 完整:SampleRate/CurrentTime/State/Resume/Destination/Listener/createOscillator/createGain/createAnalyser/createBiquadFilter/createStereoPanner/createConstantSource/createChannelMerger/Splitter/createBuffer/createBufferSource/decodeAudioData/createIIRFilter |
| `AudioContext` | `webidls/AudioContext.webidl:25` | `dom/audio/audiocontext.rs`(321 行) | 完整:实时上下文,suspend/resume/close |
| `OfflineAudioContext` | `webidls/OfflineAudioContext.webidl:15` | `dom/audio/offlineaudiocontext.rs`(231 行) | 完整:startRendering→Promise→AudioBuffer+complete 事件 |
| `AudioBuffer` | `webidls/AudioBuffer.webidl:15` | `dom/audio/audiobuffer.rs`(331 行) | 完整:getChannelData/**js_channels(HeapBufferSource<Float32>)↔shared_channels(no_trace servo-media buffer)detach/reattach 语义**(spec "acquire the content" 逐条实现) |
| `AnalyserNode` | `webidls/AnalyserNode.webidl:16` | `dom/audio/analysernode.rs`(250 行) | 有 |
| 节点族 | AudioNode:26 / AudioParam:14 / AudioListener:9 / AudioDestinationNode:9 / AudioScheduledSourceNode:9 / OscillatorNode:24 / GainNode:13 / AudioBufferSourceNode:18 / BiquadFilterNode / StereoPannerNode / PannerNode / IIRFilterNode / ConstantSourceNode / ChannelMergerNode / ChannelSplitterNode | `dom/audio/*.rs` | 有 |
| **未实现(注释掉)** | `BaseAudioContext.webidl:41-50` | — | `createDelay` / `createWaveShaper` / `createConvolver` / `createDynamicsCompressor` / `createPeriodicWave` 全部注释 |

### 1.2 线程模型(关键:渲染不占 DOM/worker 线程)

- `BaseAudioContext::new_inherited`(`baseaudiocontext.rs:117-146`):`ServoMedia::get().create_audio_context(&client_context_id, options)` → 得 `Arc<Mutex<AudioContext>>`,字段标注 `#[no_trace]`(非 GC 对象,线程安全);`client_context_id` 由 `PipelineId` 构造(`baseaudiocontext.rs:128-129`),`GlobalScope::pipeline_id()` 对 worker 同样存在。
- servo-media `AudioContext::new`(`components/media/audio/context.rs:149-198`):**自带 spawn "AudioRenderThread"** + `init_receiver.recv()` 握手;DOM 线程只做控制面(全部经 channel 收发:`current_time`/`create_node`/`connect_ports`/state 变更)。**调用线程无 thread-local SM 依赖,可在任意线程发起。**
- **OfflineAudioSink 是纯软件 sink**(`components/media/audio/offline_sink.rs`):`render_thread.rs:156-159` 对 `OfflineAudioContextOptions` 直接 `Sink::Offline(OfflineAudioSink::new(...))`——**不走 `B::make_sink()`(gstreamer),不碰音频设备**。gstreamer 仅 RealTime 上下文用到,且也在 render 线程上初始化。
- `OfflineAudioContext.startRendering`(`offlineaudiocontext.rs:140-230`):eos callback 收集 PCM → 自 spawn `OfflineACResolver` 线程等 mpsc → 经 `dom_manipulation_task_source().to_sendable()` 队回**构造上下文的全局**解析 Promise + fire complete 事件。task source 是 `GlobalScope::task_manager()`,worker 全局同样具备(servo 树内证据:`workers/sharedworkerglobalscope.rs:670`、`workers/workerglobalscope.rs:598` 已用同一 task source)。
- `ServoMedia::get()`(`components/media/servo-media/lib.rs:110-112`):`OnceLock::wait()`,init 在 servo 启动时完成(`components/servo/servo.rs:89-141`;Bao 经 `bao_browser/Cargo.toml:24` 启用 `media-gstreamer` feature → Linux 走 `ServoMedia::init::<GStreamerBackend>()`)。worker 线程必然晚于启动,`wait()` 立即返回。

### 1.3 getChannelData / createBuffer / startRendering 三载体全部就位

- `createBuffer`:`baseaudiocontext.rs:424-447`(MAX_CHANNEL_COUNT/采样率 8000-192000 校验,采样率上限 192000 对齐 Firefox,见 `audiobuffer.rs:22-24`)。
- `startRendering`:`offlineaudiocontext.rs:140-230`(见上)。
- `getChannelData`:`audiobuffer.rs:130-160 restore_js_channel_data`(attach Float32Array)+ `acquire_contents`(detach 收集)。**这正是音频指纹噪声的注入点,bao_stealth 的 JS hook 已对准它(见 §5)。**

---

## ② worker 暴露缺口清单

### 2.1 webidl Exposed 限制(唯一入口)

全部 13+ 个 audio webidl 为 `[Exposed=Window]`(行号见 §1.1 表)。worker 侧 `typeof OfflineAudioContext === 'undefined'` 即此原因。

### 2.2 实现签名耦合 `&Window`(23 文件)

`grep -l "window: &Window" dom/audio/` 命中 23 个文件;`as_window()` 调用 20 处(`baseaudiocontext.rs:334,346,354,359,366,373,383,396,406,416,441,529,558` + `offlineaudiocontext.rs:189,204` 等)。多暴露接口的 codegen 惯例 = 构造器/工厂收 `global: &GlobalScope`(树内先例:`dom/eventsource.rs:577-583`,`[Exposed=(Window,Worker)]`;WebGL 全家 `[Exposed=(Window,Worker)]`,`webidls/WebGLRenderingContext.webidl:47`)。

### 2.3 成员级 Exposed 门(唯一 codegen 陷阱)

`AudioContext.createMediaElementSource/createMediaStreamSource/createMediaStreamDestination` 及 MediaStream 源节点族引用 `HTMLMediaElement`/`MediaStream`(保持 Window-only)→ 接口改 `(Window,Worker)` 后这些**成员必须加成员级 `[Exposed=Window]`**,否则 codegen 报类型暴露不一致。AudioTrack/AudioTrackList/AudioTrackList 相关 3 文件可整体留在 Window-only。

### 2.4 线程模型约束核查结论

**无障碍**。渲染在 servo-media 自有线程(AudioRenderThread / OfflineACResolver),跨线程面仅 `Arc<Mutex<AudioContext>>`(no_trace)+ task source 队回,worker 线程只承担控制面调用与 Promise 落点。`init_receiver.recv()` 握手在 worker 线程短暂阻塞(初始化完成信号),有界。

---

## ③ Chrome 基准查证结论(本次重核的核心交付)

**结论:Chrome 的 DedicatedWorker 不暴露 OfflineAudioContext / AudioContext(以及全部 Web Audio 接口)。三源交叉一致:**

1. **Chromium main 源码**(权威,base64 解码逐字核验):
   - `third_party/blink/renderer/modules/webaudio/offline_audio_context.idl`:
     `[\n    Exposed=Window\n] interface OfflineAudioContext : BaseAudioContext {`(无 RuntimeEnabled 门)
   - `third_party/blink/renderer/modules/webaudio/audio_context.idl`:
     `"Exposed=Window,"` / `"] interface AudioContext : BaseAudioContext {"`(唯一 RuntimeEnabled 在 playbackStats 属性上,与暴露无关)
   - URL: https://chromium.googlesource.com/chromium/src/+/refs/heads/main/third_party/blink/renderer/modules/webaudio/offline_audio_context.idl 与 .../audio_context.idl
2. **W3C Web Audio API 1.1 spec**(https://www.w3.org/TR/webaudio-1.1/):
   `[Exposed=Window] interface BaseAudioContext : EventTarget {`、`[Exposed=Window] interface AudioContext : BaseAudioContext {`(§1.1/§1.2 逐字);OfflineAudioContext(§1.3)同 Window-only(Chromium 源码已独立证实)。
3. **MDN BCD**(https://raw.githubusercontent.com/mdn/browser-compat-data/main/api/OfflineAudioContext.json 与 api/AudioContext.json):**零 worker 条目**;`OfflineAudioContext` 构造 Chrome 35 / Firefox 25 / Safari 14.1,`startRendering` 返 Promise Chrome 42。BCD 有 worker 数据时会显式建 worker scope 条目(如 `__compat` scope:worker),此处整份无 → 与 Window-only 一致。

**旁证**:WebAudio WG issue https://github.com/WebAudio/web-audio-api/issues/2423(2016-11 hoch 提出 `[Exposed=(Window,Worker)]`)至今 **Open,零评论,未进 spec**;https://github.com/WebAudio/web-audio-api-v2/issues/111(AudioBuffer 进 DedicatedWorker)同为 open 提案。Chrome 的 worker 音频机制是 **AudioWorklet**(独立 AudioWorkletGlobalScope,非 DedicatedWorker,也不提供 OfflineAudioContext)。

**判定**:audio fingerprint 的 OfflineAudioContext 载体**在 Chrome worker 内不存在**。主流 audiofp 库(FingerprintJS 等)的音频指纹全部在 Window/主文档上下文采集。worker 侧暴露 OfflineAudioContext 的浏览器(若有)不是 Chrome;Bao stealth 基准 = `chrome_default`(`bao_stealth/src/engine_props.rs`),对齐目标是 Chrome。

---

## ④ 最小 patch 面建议(两案)

### 案 A:Chrome 对齐路线(worker 不暴露)— patch 面 ≈ 0

- servo vendor:**零 patch**。现状 `[Exposed=Window]` 已与 Chrome main 逐字一致。
- bao_stealth:**零 patch**。`hooks.rs:314-318` 的 worker-realm typeof 守卫(`if (typeof AudioBuffer !== 'undefined')` / `if (typeof OfflineAudioContext !== 'undefined')`)在 worker realm 自动短路——行为等价 Chrome worker(探针返回 undefined)。
- 真实可做的增强(可选,均 Window 侧):④.3 缺口(DynamicsCompressor/WaveShaper 未实现)才是「audio fingerprint 主流载体」在 Bao 的真实短板——补齐 Window 侧 DynamicsCompressorNode 才能让经典 audiofp 算法跑通并被噪声覆盖。这是与「worker AudioContext」完全不同的一块工作,若走本案建议作为独立任务提案。
- 代价:C15 的「worker AudioContext+噪声」裁决目标在此案下不可达成(与 Chrome 不一致的部分自动一致化)。

### 案 B:全自建路线(worker 暴露 OfflineAudioContext+最小节点面)— vendor patch 面

> 按最小差分,只暴露指纹向量所需面,不搬 MediaStream 族:

1. **webidl(10 文件,1 行/文件)**:`[Exposed=Window]` → `[Exposed=(Window,Worker)]`:
   `OfflineAudioContext.webidl:15`、`BaseAudioContext.webidl:18`、`AudioBuffer.webidl:15`、`AudioNode.webidl:26`、`AudioParam.webidl:14`、`AudioDestinationNode.webidl:9`、`OscillatorNode.webidl:24`、`GainNode.webidl:13`、`AudioBufferSourceNode.webidl:18`、(`AudioContext.webidl:25` 可选——实时上下文在 worker 无指纹意义,建议不暴露,进一步缩小面)
   **不动**:AnalyserNode(可留 Window)、AudioScheduledSourceNode 若 AudioBufferSourceNode 继承它则必须同改、全部 Media*/AudioTrack* 文件、AudioListener(PannerNode 不暴露则不需要)。
2. **成员级 `[Exposed=Window]` 门**:`BaseAudioContext.webidl` 的 decodeAudioData 可保留(纯软件);`AudioContext` 的 createMedia* 仅在 AudioContext.webidl,案 B 不暴露 AudioContext 则无需成员门;若暴露则 `audiocontext.rs` 的 createMedia* 三成员需门。
3. **实现签名(~10 文件)**:案 B 触碰的文件 `offlineaudiocontext.rs`(构造器 `window: &Window`→`&GlobalScope`,`:89 pipeline_id()` 换 `global.pipeline_id()`,`:189/:204 as_window()` 移除)、`baseaudiocontext.rs`(createBuffer/createOscillator/createGain/createBufferSource 等 `self.global().as_window()`→`self.global()`,Listener 除外)、`audiobuffer.rs`(new/new_with_proto/构造器签名)、`audionode.rs`/`audioparam.rs`/`oscillatornode.rs`/`gainnode.rs`/`audiobuffersourcenode.rs`/`audiodestinationnode.rs` 同型签名改。**不改**:渲染线程、servo-media、task source 逻辑(全部已是 global 无关)。
4. **注册面**:多暴露接口 codegen 自动装到 DedicatedWorkerGlobalScope(servo 树内 `(Window,Worker)` 先例:WebGL 全家、OffscreenCanvas `OffscreenCanvas.webidl:18`、EventSource、Notification)。
5. **bao_stealth:零 patch**(见 §5)。
6. **预算**:webidl+签名机械改 ~10-12 文件;真正成本在 crown linter(Servo GC 静态检查)对新签名的清理与 codegen 重跑。属于 C14 WebGL worker 暴露(W3a/W3b)同构、已在本仓验证过两轮的 patch 形态。

### 两案对照

| 维度 | 案 A(Chrome 对齐) | 案 B(自建 worker 暴露) |
|---|---|---|
| Chrome 一致性 | 100%(探针 undefined) | **负一致**(Bao 独有暴露,可被 `typeof` 探针一发识别) |
| vendor patch | 0 | ~10-12 文件(机械) |
| bao_stealth patch | 0 | 0(自动生效) |
| 反指纹净效果 | 与 Chrome 全等 | worker 音频面扩大 = 新暴露面 |
| 工作量 | 近零 | 机械但面广(含 crown 清理) |

---

## ⑤ bao_stealth 现有 audio 噪声与 worker 接线位

- **噪声算法**:`bao_stealth/src/webgl_audio.rs:87-118` `AudioProfile`(seed + 振幅 1e-7 + 确定性 splitmix 型 PRNG,`apply_noise`/`deterministic_noise`);Rust 侧与 JS 侧同构。
- **JS hook**:`bao_stealth/src/hooks.rs:295-355 build_audio_js`:BigInt 64 位确定性噪声 `detNoise(index)`;hook 点 ① `AudioBuffer.prototype.getChannelData`(逐样本加性噪声)② `OfflineAudioContext.prototype.startRendering`(Promise 后处理,逐通道 `buffer.getChannelData`——注意此处调用的是被 hook 过的 getChannelData,随后循环再加一次,等效双倍振幅;现状实现如此,记录备查)。
- **worker 守卫已在位**:`hooks.rs:313-318` 注释明示 REQ-BRW-004 C13/C15——typeof 守卫防 worker realm ReferenceError 杀整块 hooks blob。
- **worker 注入链已通**(W1a 波):`bao_browser/src/lib.rs:405 register_worker_scope_callback_native` → servo vendor patch `EMBEDDER_WORKER_SCOPE_CALLBACKS`(`script/event_loop/script_thread.rs:240,254`,drain 于 `dom/workers/dedicatedworkerglobalscope.rs:607`,在 worker 全局建好之后、事件循环之前,worker 线程上执行)→ `bao_browser/src/runtime_bridge.rs:1595 install_stealth_props(raw_cx, global)` → `engine_props.rs:1586 inject_js_hooks`(profile 解析:REALM_PROFILES 按全局地址 → thread-local → firefox_default,`engine_props.rs:1392-1410`)。
- **结论**:无论案 A/B,**bao_stealth 层零新增工作**;案 B 下现有 hook 直接落在 worker realm 生效(REALM_PROFILES 按 worker 全局地址回填,`runtime_bridge.rs:1488-1493`)。
- **测试现状**:`bao_stealth/tests/suite/headless_fingerprint_hiding_tests.rs:419-474`(audio 噪声非零 + hook 存在断言);`bao_browser/tests/suite/media_e2e_tests.rs:931-980` 已有真实 `new OfflineAudioContext(2, 22050, 44100)` 渲染 e2e(Window 侧)——worker 版可直接同构复制。

---

## ⑥ 风险清单

1. **[最高] 指纹悖论(案 B 战略风险)**:Chrome worker 无任何 Audio 接口;Bao worker 暴露即产生独有特征(`typeof OfflineAudioContext` 探针)。反指纹产品暴露超集=负资产。需用户裁决确认是否仍按 2026-09-09 裁决推进案 B。
2. **成员级 Exposed 门遗漏** → codegen 编译错误(类型在 worker 未暴露),报错直白,属机械修复,无隐蔽风险。
3. **worker 生命周期 vs render 线程**:servo 的 BaseAudioContext 无 Drop→close 链(Window 侧同样存在);worker 销毁时未 close 的上下文其 AudioRenderThread 继续空转,离线渲染线程(OfflineACResolver)等待 mpsc 自然退出。风险为线程驻留非 UAF(无 JSObject 跨线程,`#[no_trace]` Arc)。案 B 上线前建议补 worker-scope close 路径观察项,非阻断。
4. **GC/trace**:AudioBuffer js_channels 为标准 traced DOM 字段,enter_auto_realm(`audiobuffer.rs:132`)realm 无关;`audio_context_impl` `#[no_trace]`。无新增 GC 风险。
5. **ServoMedia 未初始化窗口**:worker 线程必然晚于 `media_platform::init()`(servo 启动序),`OnceLock::wait()` 兜底;但若 gstreamer init 失败(Linux `ServoMedia::init::<GStreamerBackend>()` 失败行为)= 既有 Window 侧行为,非 worker 新增。
6. **crown linter**:签名 `&Window`→`&GlobalScope` 触发 servo crown(unrooted_must_root)重新评估,`#[cfg_attr(crown, expect(...))]` 属性需随签名迁移;C14 波已趟过同类。
7. **现状 quirk 顺带记录**:`hooks.rs` startRendering 后处理内 `buffer.getChannelData(ch)` 走已 hook 的 getter,叠加循环内二次加噪 → 实际振幅≈2×;若后续校准噪声振幅需注意(与 C15 路线无关,独立事项)。

---

## 附:证据文件绝对路径索引

- webidl:`vendor/servo/components/script_bindings/webidls/{AudioContext,OfflineAudioContext,BaseAudioContext,AudioBuffer,AnalyserNode,AudioNode,AudioParam,AudioListener,AudioDestinationNode,AudioScheduledSourceNode,OscillatorNode,GainNode,AudioBufferSourceNode}.webidl`
- 实现:`vendor/servo/components/script/dom/audio/*.rs`(mod.rs + 23 实现文件,5673 行)
- 后端:`vendor/servo/components/media/servo-media/lib.rs`(OnceLock 单例)、`media/audio/context.rs`(AudioRenderThread spawn)、`media/audio/render_thread.rs:152-160`(sink 选择)、`media/audio/offline_sink.rs`(纯软件)、`media/backends/gstreamer/lib.rs:228-247`(create_audio_context)
- init:`vendor/servo/components/servo/servo.rs:89-141`;Bao feature:`src/bao_browser/Cargo.toml:24`
- stealth:`src/bao_stealth/src/hooks.rs:295-355`、`src/bao_stealth/src/webgl_audio.rs:87-118`、`src/bao_stealth/src/engine_props.rs:1392-1410,1586`
- worker 接线:`src/bao_browser/src/lib.rs:395-426`、`src/bao_browser/src/runtime_bridge.rs:1412-1600`、`vendor/servo/components/script/event_loop/script_thread.rs:212-254`、`vendor/servo/components/script/dom/workers/dedicatedworkerglobalscope.rs:607`
- Chrome 基准:chromium.googlesource.com/chromium/src/+/refs/heads/main/third_party/blink/renderer/modules/webaudio/{offline_audio_context,audio_context}.idl;w3.org/TR/webaudio-1.1/ §1.1-1.3;mdn/browser-compat-data api/{OfflineAudioContext,AudioContext}.json;github.com/WebAudio/web-audio-api/issues/2423
