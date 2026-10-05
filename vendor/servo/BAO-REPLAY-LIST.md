# vendor/servo 快照替换 — 溶解核验与待回放清单（e99, REQ-BRW-002 / REQ-DEPLOY-1 P1-② 第一阶段）

- 快照点：`vendor/servo/components` 全树 4842b770e（2026-08-13）→ **614cd411f^ = 240a37393**（2026-10-05，stylo 2026-10-01 bump `614cd411f` 之前一 commit；用户暂定钉）。
- 本阶段边界：快照替换 + manifest 归一 + 机械 build-fix。**语义回放不做**（归 6 切片阶段）；本文件是回放阶段的输入清单。
- 判定方法：溶解项 = 工具实测（目标树字节级/grep 级证据 + 上游窗口 commit 号）；待回放项 = 默认态（e97 recon 126 补丁文件中未能证实已溶解者）。回放切片动手前必须先对上游终态做 per-file 语义对账——部分「待回放」条目可能在对账中升级为已溶解。

## 一、溶解核验三件（合同指定）

| # | 因子 | 判定 | 证据 |
|---|------|------|------|
| ① | SM153 消费面 11 文件（JobQueue traps / microtask / script_module / module_loading / bindings error / structuredclone / principals / buffer_source 等） | **已溶解** | 上游窗口内自行完成 SM153 迁移并继续演化：`a7b65192d`（08-29, SM153_0esr_RELEASE）→ `d8671305a`（09-06, JSContext owned JobQueue，上游自有 job_queue 替代 bao 的 microtask.ts 面）。目标树 `components/script/runtime/{job_queue,script_runtime}.rs` 在位。bao 旧 microtask.rs / buffer_source.rs / tasks/task.rs 等旧路径孤儿已随快照删除（deletion-manifest: replacement） |
| ② | GL make-current（bao WGL patch @ shared/paint/rendering_context.rs） | **已溶解** | 上游 `16e4b1591`（2026-10-05, #48636 "Make the GL context current before creating the glow context"）与本方 patch 同形；目标树 rendering_context.rs:140 `device.make_context_current(&context)` 字节级在位（Windows 面仍走 `no-wgl`→ANGLE，本 patch 使 fork 的 WGL 路径纵深防御成立） |
| ③ | csp safe-naming 名义分歧（035881080） | **升级为 mozjs 域阻塞项（见 §五）** | 035881080（08-31 "Drop safe naming from mozjs APIs"）使 servo 消费面改用 canonical 名（`describe_scripted_caller(&JSContext)`、`to_jsval`）。目标 servo 代码即此形态；但 bao-mozjs 0.24.2 wrapper 仍是 safe_ 时代（`describe_scripted_caller_safe` / `safe_to_jsval`，e98 重锚仅吸收 5 commit，未含 safe-drop）。因此 console.rs/csp.rs 的 safe-caller 面在**名字上已溶解**（上游 canonical 名即安全实现），实际编译收敛依赖 §五 的 mozjs wrapper 对齐 |

## 二、已溶解（工具实测，回放阶段无需重做）

| BAO patch | 溶解证据 |
|-----------|----------|
| W30 Reflector 空 trace 自指槽（2026-09-29） | 上游 `967e455f7`（09-09 "Trace reflector objects #47934"）；目标 reflector.rs `self.object.trace(tracer)` 在位 |
| W28 fontfaceset root-slot 读 / font-ready discard 守卫（部分） | 上游 RootedPromise 大迁移（`91b10b749`×14、`261725c76`×18、`a6c9302e1`、`d078b2cab`）重构同域；压缩 GC 危害面另被 `8067519db`（08-28 "Disable SpiderMonkey's compacting GC by default"）从配置层 neuter。**保留注意**：W28 的「pump 面 deref stored promise 必须过注册槽/discard 探针」纪律仍是回放切片的横扫检查项 |
| SM153 机械站（structuredclone WriteUint32Pair→Unchecked、principals 拆分、keyframeeffect &mut 形、for_of 双参等） | 上游 SM153 迁移终态即本形态（同 ①） |
| WebVTT 核心（GetCueAsHTML / cue order / track URL 变更 / cue 渲染脚手架 / parsing 恢复） | 上游自有实现：`cfa8441bd`（GetCueAsHTML）、`6bd06116e`（cue order）、`a79e2c048`（text track 渲染脚手架）、`8dd2551cb`+`0753df091`（track URL）、`42e9e46b1`（parse 恢复）、`ed37a2ecb`（set text track）。webvtt crate/script dom/webvtt 全部按上游终态替换 |
| ServiceWorker Cache（in-mem backend） | 上游 `1462d866b`（cache.rs 上游自有实现，替换 bao cache.rs） |
| SW 通用通道化 | 上游 `2da128da6`（net: generic channels in SW setup） |
| keyCode 对齐 | 上游 `cb9b0804b`（08-28 "Supply equivalant keyCode values as in Chrome/FF"） |
| Location ancestorOrigins | 上游 `ac50f9f3f`（08-25 "Implement ancestorOrigins for Location"） |
| WebXR IPC→callbacks | 上游 `6b09303f8`（09-29） |
| WebCrypto MLKEM768-X25519 / ML-DSA 混合族 | 上游自有：`ea4aaa3f1`/`2ae9cc36d`/`c4fac8d18`/`5f3f3ebdd`/`fa5af8b52`/`ad4d1e7ce`/`47f0cd329` 等；subtlecrypto.rs 按上游终态替换 |
| timers GC 可达性 | 上游 `3ae004938`（10-03 "pending timer callbacks always reachable from GC roots"）；上游自有 `script/event_loop/timers.rs` + `script/timers` crate 替换 bao timers.rs（RED-1/W15 钩子属待回放，见 §三） |
| script/runtime 迁移面 | 上游 `a1c26a418`（runtime 文件入 `script/runtime/`）+ `d8671305a`（JobQueue） |
| 新 upstream crate：`components/svg`（SVG model）、`components/script_webgpu`（GPU 出树） | 快照带入；bao webgpu feature 恒 off，`script/dom/webgpu` 旧树删除（deletion-manifest: replacement，上游迁至 script_webgpu crate） |

## 三、待回放（默认态；回放切片先做 per-file 对账）

### 3.1 页面网络/stealth 线（R53-A / U2 / C19-②）
- `net/fetch/bun_bridge.rs`（**保留在树，dormant**）+ `net/http_loader.rs` 的 `obtain_response_bun` 接线 + `target_webview_id` per-WebViewId stealth wire 注册表（`STEALTH_TLS_BY_WEBVIEW` / `STEALTH_H2_BY_WEBVIEW` / `resolve_stealth_{tls_config,http2_fingerprint}`）
- `net/connector.rs` boringssl stealth TLS connector（JA3/JA4/H2 SETTINGS）
- `net/request_interceptor.rs` webview-less handler（BCE-20260910-002）
- `net/resource_thread.rs` SwManagers 共享注册表 + `net/fetch/methods.rs` handle-fetch
- `net/async_runtime.rs` spawn 形态 patch

### 3.2 script_thread / embedder 桥面（最大单一回放面）
- `script/event_loop/script_thread.rs`：embedder 脚本/Worker 回调注册与 drain（含第二 drain 点）、router_proxy 安装（BCE-20260627-009）、`bao_run_in_script_settings`（BCE-20260910-004）、RED-1 realm-discard cancel 桥、W15 shrink 钩子、`EMBEDDER_NEW_DOCUMENT_SCRIPTS` new-document 注入层（REQ-CDP-004）、per-Worker injector 双层（REQ-BRW-004 e43）
- `components/servo/lib.rs` 361 行 embedder API + `servo/servo.rs` W27 join-spin 有界化（目标树 line 884 仍无 deadline，实测未溶解）
- `script/dom/window/window.rs`、`constellation/constellation.rs`、`shared/constellation/*`、`shared/base/{lib,ipc_router}.rs`（ipc_router 保留在树，dormant）、`shared/script/lib.rs`、`shared/net/lib.rs`、`messaging.rs` SW 同步 DOM 通道（上游 generic-channel 形态需重锚）
- `script/engine/handle.rs`：bao 幂等 JSEngineSetup 形态（上游自有 handle.rs 内容不同，需按上游新位重放）

### 3.3 SW FetchEvent / controller / lifecycle
- `script/dom/serviceworker/fetchevent.rs` + `FetchEvent.webidl`（**保留在树，dormant**）+ `serviceworkerglobalscope.rs` Response(mediator) 分支 + `onfetch`
- `serviceworker_manager.rs` install 激活步 / Resolve Promise 后移、`serviceworkercontainer.rs` controller 赋值链、SW 生命周期 Update Worker/Registration State 三跳（上游 2026-10-04 实查未修，目标树仍缺）
- `clients.rs`（**保留在树，dormant**）+ `Clients.webidl`

### 3.4 canvas / WebGL / Audio worker 面（C13/C14/C15 + R53-A 二阶段）
- `canvas/canvas_paint_thread.rs` GetImageData 噪声咽喉 + `canvas_noise.rs`（**保留在树，dormant**）+ per-WebViewId canvas 噪声注册表 + CanvasId 创建链 webview 身份（`shared/canvas/lib.rs` + `from_script_message.rs` + constellation relay + `canvas_state.rs`）
- `webglrenderingcontext.rs` / `webgl2renderingcontext.rs` / `webglshaderprecisionformat.rs` / `offscreencanvas.rs` 的 Window 解锚 + worker 入口
- WebGL2/Audio 12+5 webidl `Exposed=(Window,Worker)` 面；audio dom 14 文件 `&GlobalScope` 解锚
- `workerglobalscope.rs` / `dedicatedworkerglobalscope.rs` / `sharedworkerglobalscope.rs` / `workletglobalscope.rs` drain+injector 挂点
- `globalscope.rs` `egress_webview_id()` + fetch SW-realm 降级 + `xmlhttprequest.rs`（R1 sync fail-closed + R53-A）+ `fetch/request.rs`（R53-A）

### 3.5 DOM 语义小 patch 族（逐项列名）
- `document/document.rs`、`element/element.rs`、`element/attributes/storage.rs`、`characterdata/text.rs`、`clipboard/clipboardevent.rs`、`event/keyboardevent.rs`、`form/validitystate.rs`、`form_controls/text_control.rs`、`gamepad/gamepadhapticactuator.rs`、`media/medialist.rs`、`performance/*`、`raredata.rs`、`css/cssfontfacedescriptors.rs`、`css/fontfaceset.rs`（W28 余量）、`indexeddb/{idbfactory,idbindex,idbopendbrequest,idbrequest,idbtransaction}.rs`（含 BCE-20260910-004b 双终局门）、`promise/promise.rs`（bao RootedPromise 面余量）、`tasks/task_manager.rs`、`script/lib.rs` re-export 面、`script/dom/mod.rs`
- `html/{htmliframeelement,htmlimageelement,htmlmediaelement,htmltrackelement,htmlvideoelement}.rs`（含 time_marches_on `queue_throttled_timeupdate` 保活锚——media_e2e 回归根因，回放必保）
- `webcrypto/subtlecrypto.rs` bao 余量（对账后定）、`webxr/{xrsession,xrsystem}.rs` bao 余量
- `script_bindings/{callback,dom,import,interface,lock,root,trace,reflector(余量)}.rs` + `codegen/{codegen.py,configuration.py,Bindings.conf}`（fork codegen 适配：crown cx-first 形、fork codegen 画像、SVG optional-dict `= {}` 形等——上游 codegen 大改（crown 0e94d1e4d、TracedCallback 族、dictionaries-by-ref 7c4b7b9ff），必须对新 codegen 重推导）
- `script_bindings/webidls/{SVGGeometryElement,SVGGraphicsElement}.webidl`（上游 SVG crate 到来后对账）+ `AudioParamMap/AudioWorklet*/Clients/FetchEvent.webidl`（保留在树，dormant，随 e90/SW 回放接线）
- `script/dom/svg/{mod,svggeometryelement,svggraphicselement}.rs` + `svg/svg_geometry.rs`（**保留在树，dormant**；kurbo/svgtypes 依赖已在 manifest）
- `script/dom/document/{animations,image_animation}.rs` 已删除（上游 animation_manager 终态替代；deletion-manifest: replacement）
- `layout/*`（display_list/mod、stacking_context、dom.rs、fragment_tree/fragment、layout_impl、lib、replaced）+ `layout/webvtt_cue_overlay.rs`（**保留在树，dormant**；上游自有 cue 渲染脚手架到位后对账取合）
- `fonts/font_context.rs`、`config/{opts,prefs}.rs`、`allocator/{lib.rs,Cargo.toml}`、`url/Cargo.toml`、`webvtt/{Cargo.toml,src/cue/text.rs,src/lib.rs}` 余量、`pixels/benches.rs`（保留在树，dormant）
- MessagePort.webidl `[Exposed]` 增 `AudioWorklet`（本阶段为编译收敛新增的一行 fork 适配，随 e90 回放对账）

## 四、保留文件（BAO-new 零删除；dormant=未接 module 声明，随回放接线）

`canvas/canvas_noise.rs`、`layout/webvtt_cue_overlay.rs`、`net/fetch/bun_bridge.rs`、`net/tests/bun_bridge.rs`、`pixels/benches.rs`、`script/Cargo.lock`、`script/dom/audio/audioworklet{,globalscope,handler,node,port,processor,audioparammap}.rs`（e90 全链，随回放接 module）、`script/dom/serviceworker/{clients,fetchevent}.rs`、`script/dom/svg/svg_geometry.rs`、`shared/base/ipc_router.rs`、`script_bindings/webidls/{AudioParamMap,AudioWorklet,AudioWorkletGlobalScope,AudioWorkletNode,AudioWorkletProcessor,Clients,FetchEvent}.webidl`、`media/audio/{audioworklet_node,node,ring}.rs`（components/media 全程未触碰，e90 域字节级保持）

## 五、阻塞项（编译收敛剩余面，非本合同域）

- **bao-mozjs wrapper API 落后一个命名纪元**：目标 servo 代码（240a37393 + mozjs =0.26.7 时代）调用 canonical 名（`to_jsval` / `from_jsval` / `describe_scripted_caller(&JSContext)` 等）；bao-mozjs 0.24.2（vendor/mozjs，e98 重锚仅吸收上游 5 commit，未含 safe-drop 窗口）仍暴露 `safe_to_jsval` / `describe_scripted_caller_safe` 形态。script_bindings 生成代码 16k+ 错误全部源于该 trait/签名形差（生成 Bindings/* 全量命中，无独立第二病灶）。
- 归属：vendor/mozjs（e98 谱系）。修复形态 = 吸收上游 safe-drop（wrapper rust 面对齐 c2429cbe9 终态 + 版本 0.24.2→0.26.x 系列 + BAO mozjs patch #1-#8 重放核验）。native（mozjs-sys C++）侧已在 c2429cbe9，无需重编 SM。
- 该项收敛后，`cargo check -p bao-servo-script` 与 `-p bao-servo` 预期即可绿（net 已先绿：`cargo check -p bao-servo-net` RC=0）。

## 六、manifest 工程记录

- 54 个非 media 组件 manifest 按「bao HEAD + 上游 old→new delta」三方可控合并（工具：/tmp/e99_manifest2.py + ws-bump + dedup 三 pass）；media 全系 manifest 未触碰。
- 新 crate：`svg`（bao-servo-svg 0.5.30）、`script_webgpu`（bao-servo-script-webgpu 0.5.30）。
- 关键 remap：`stylo`→dep key `style`（extern name=KEY，lib 名 `style`，代码 `use style::`）、`stylo_*`=0.22.0 系、`selectors`=0.41.0、`servo_arc`=0.5.0、`js`→bao-mozjs path ^0.24、`ipc-channel`→bao-ipc-channel path 0.22.0、`servo-background-hang-monitor-api`→dep key `background_hang_monitor_api`（上游改 key 未改 lib 名）。
- 保留的 manifest 级 BAO patch：paint `default = ["webgl"]` carry-patch（bao_browser default-features=false 依赖图需要）。
- 已知上游 dep bump 随快照吸收：taffy 0.13→0.14、icu_locale_core 2.1、skrifa 0.46.2、read-fonts 0.43.3、webdriver 0.54、surfman 0.14、mozangle 0.7.1、content-security-policy 0.9（registry 0.9 req，根 [patch.crates-io] 的 freetype/usvg/stylo_atoms 重定向不变）。
