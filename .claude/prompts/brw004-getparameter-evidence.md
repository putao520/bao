# 取证报告 — `gl.getParameter(0x1F02 / 0x8B8C)` 返回 undefined(e36 · 只读取证,禁自修)

**结论一句话:`undefined` 不是 servo 上游缺陷,是 Bao 侧缺陷**——Window realm 的 stealth
注入链被**双重安装**,`WebGLRenderingContext.prototype.__originalGetParameter__` 槽位里
存的是一个 JS stealth hook 而非 servo 原生方法;未被任一层拦截的参数在
「JS hook ↔ 原生 override」之间自指往返,最终由
`bao_stealth/src/engine_props.rs` 的 `Err(_) => UndefinedValue()` 吞错,以
**字面 undefined + getError()==0** 呈现。stealth-free 环境(sthealth_profile: None)
**同样复现**(注入是无条件的)。

**不成立的上游证据**:worker realm 对照组走的是零 Bao 包装的 servo 原生
`getParameter`(GPNATIVE=true),返回精确正确:
`0x1F02 → "WebGL 1.0"`、`0x8B8C → "WebGL GLSL ES 1.0"`、`0x1F00/0x1F01 → "Mozilla/Servo"`。
vendor `webglrenderingcontext.rs` 的 `constants::VERSION`(:2347)/
`SHADING_LANGUAGE_VERSION`(:2355)handler 实测无缺陷。**不应向 servo 提 issue。**

---

## 1. 双态 × 双 realm 实测矩阵

`d()` 标记:`undefined` / `null` / `str[...]` / `num[...]`。ERR=`gl.getError()`。

| 事实 | Window · stealth OFF | Window · stealth ON | Worker · stealth OFF | Worker · stealth ON |
|---|---|---|---|---|
| proto.getParameter 来源 | JS hook(GPNATIVE=false, GPNAME="") | JS hook(GPNATIVE=false, GPNAME="") | servo 原生(GPNATIVE=true) | servo 原生(GPNATIVE=true) |
| `__originalGetParameter__` 在 proto | true | true | false | false |
| `__originalGetParameter__` 的源码 | **与 getParameter 同一段 JS hook 源码** | 同左 | —(不存在) | —(不存在) |
| 0x1F00 VENDOR | str[Mozilla] | str[Mozilla] | str[Mozilla/Servo] | str[Mozilla/Servo] |
| 0x1F01 RENDERER | str[WebGL 1.0 (OpenGL ES 2.0 Chromium)] | 同左 | str[Mozilla/Servo] | str[Mozilla/Servo] |
| **0x1F02 VERSION** | **undefined** | **undefined** | **str[WebGL 1.0]** | **str[WebGL 1.0]** |
| **0x8B8C SHADING_LANGUAGE_VERSION** | **undefined** | **undefined** | **str[WebGL GLSL ES 1.0]** | **str[WebGL GLSL ES 1.0]** |
| 0x0D33 MAX_TEXTURE_SIZE | num[16384] | num[16384] | num[16384] | num[16384] |
| ERR(getError) | 0 | 0 | 0 | 0 |
| 直呼 `__originalGetParameter__.call(gl,0x1F02)` | **undefined** | **undefined** | — | — |
| 直呼 `__originalGetParameter__.call(gl,0x8B8C)` | **undefined** | **undefined** | — | — |
| 直呼 `__originalGetParameter__.call(gl,0x0D33)` | num[16384] | num[16384] | — | — |

Window realm 的 `__originalGetParameter__` 源码文本 == proto.getParameter 源码文本
(两者都是 `function(param) { var dbgRenderer = 0x9246; var dbgVendor = 0x9245; ...`
即 hooks.rs 生成的 JS hook)——**「原始方法」本身就是一个 stealth hook,这是双重安装的直接物证**
(servo 原生方法形态应为 `function getParameter() { [native code] }`,见 worker 列)。

## 2. 环境构造法 / commit / 执行命令原文

- **BAO commit**: `18e6f876e9a30e0d40b09a9462a4a7ca92cf4030`(e31 wave1 落点;工作树含
  e32/e33/e34/e35 在途编辑,其中 `vendor/.../webglrenderingcontext.rs` 仅改
  `GetShaderPrecisionFormat` 的 global 反射,不触碰 GetParameter 路径,已核对 diff)
- **「无 stealth」构造法**:`PageConfig { stealth_profile: None, ..Default::default() }`
  (`BaoConfig::default()` 同为 None)。**注意:None 只是不注册 profile 值,不关注入**——
  `install_all_native`(bao_browser/src/runtime_bridge.rs:1211)对 Some/None 两条分支
  都执行 `bao_stealth::engine_props::install_stealth_props(raw_cx, raw_global)`
  (runtime_bridge.rs:1291),其内部无条件跑 `install_webgl_override`
  (engine_props.rs:1553)与 `inject_js_hooks`(:1561)。
- **执行命令原文**(两态同一命令,测试内分别构造两态;探针为一次性临时测试
  `brw004_getparam_forensics_tests.rs`,运行后已按任务约束删除,工作树已还原):

  ```
  BAO_TEST_NETWORK=1 xvfb-run -a cargo nt -p bao-browser --cargo-profile test-ci \
      -E 'test(forensics_getparam)' --success-output immediate
  # 4/4 PASS, digests 见 §1(cargo nt = cargo nextest run,dev alias;
  # 测试体 = 复用 stealth_offscreencanvas_tests.rs 的 live_page/worker data:URL 车辆)
  ```

- **Worker realm 车辆**:页面 JS `new Worker(data:URL)` → servo 原生 Worker 线程 →
  `new OffscreenCanvas(64,48).getContext('webgl')` → digest 经 postMessage 回 Window,
  与既有 C13/C14 测试同构。

## 3. 分层判定(指认层)

因果链(全部为读源码 + 实测交叉证实,未改动任何代码):

1. **双重安装向量**:`BaoRuntime::create_page`(bao_browser/src/lib.rs:290)→
   `PagePool::create_page` 内部已调 `inject_all`(page_pool.rs:87);返回后 lib.rs:289
   再调 `inject_all_with_profile`。同一条 create_page 路径把
   `install_webgl_override`(原生)+ `inject_js_hooks`(hooks.rs JS)各跑 **两遍**。
2. **`__originalGetParameter__` 被二次覆盖**:`install_webgl_override`
   (engine_props.rs:1191)每次安装都把「当前 proto.getParameter」存进
   `__originalGetParameter__`(:1219-1231)再覆盖 getParameter。第二次安装时当前值已是
   第一遍的产物 → `__originalGetParameter__` 最终指向 JS hook(§1 物证)。
3. **自指环**:JS hook 未拦截的参数(0x1F02/0x8B8C)→ JS hook 透传到其捕获的
   `origGetParameter`(原生 override)→ 原生 override 运行时**实时读**
   `__originalGetParameter__`(engine_props.rs:944-968,读的是当前 proto 值 = JS hook)
   → 回到 JS hook → 无限往返,servo 原生方法被孤儿化,**永远不会到达**。
4. **吞错出口**:`bao_engine::host_fn::call_function` 失败 →
   `Err(_) => { args.rval().set(UndefinedValue()); true }`(engine_props.rs:970-974)
   → 对外呈字面 `undefined`,异常被吃掉,`getError()==0`,页面无任何信号。
5. **为何 0x0D33 幸存**:hooks.rs JS hook 拦截 0x0D33/0x84E8/0x0D3A/0x9246/0x9245,
   原生 override 拦截 0x1F00/0x1F01——被任一层拦截的参数在环内被截停返回,只有
   「两层都不拦」的参数(VERSION/GLSL 等全部 core string/int 查询)进死环。
   即 **所有非拦截参数在 Window realm 全部返回 undefined**(不止这两个枚举)。
6. **Worker realm 无恙的原因**:worker scope 初始化走
   `register_worker_scope_callback_native` → `worker_scope_init_native`
   (runtime_bridge.rs:1421+),不跑 `install_webgl_override`/`inject_js_hooks`,
   proto 保持 servo 原生(§1 实测),vendor handler 正确返回。

**指认**:Bao 层 `bao_stealth`(hooks.rs getParameter JS 包装 +
engine_props.rs `install_webgl_override` 原生包装)与安装位点
`bao_browser`(page_pool.rs:87 与 lib.rs:289 的重复 inject)。servo 上游无罪。

## 4. 对既有断言的修正

- e31 回报「无 stealth hook 时相同」:**不成立**——worker realm(无 hook)返回正确
  字符串;undefined 只出现在带 Bao 注入链的 Window realm,且 stealth-free 一样复现。
- `stealth_offscreencanvas_tests.rs` 注释「0x1F02 … pre-existing upstream quirk,
  verified identical on the Window path」:**判定错误**,应随本报告订正(该文件现由
  wave 在途,不在本任务改动)。
- C14 测试用 0x1F01 探活:值来自 Bao 原生 override 的 profile/TL 值
  (`WebGL 1.0 (OpenGL ES 2.0 Chromium)`),不是 servo 的驱动串——该测试从未触达
  破损的透传路径,故绿。

## 5. 修复方向(记录,未执行 · 需按流程立项)

- ① 去重安装位点:create_page 只保留一处 inject(page_pool.rs:87 或 lib.rs:289 二选一);
- ② `install_webgl_override` 幂等化:已见 `getParameter` 为自有 override 时跳过重存
  `__originalGetParameter__`(防自我吞并),或改存到独立隐藏槽位避免运行时读 live 值;
- ③ `Err(_) => UndefinedValue()` 吞错出口改为 fail-loud(至少 console error / 记录),
  消除「静默 undefined」;
- ④ stealth-free 语义:profile 为 None 时应整体跳过 stealth 注入
  (现 install_stealth_props 无 profile 参数,注入无条件)。
- 回归断言(修后应绿):Window realm `getParameter(0x1F02)==="WebGL 1.0"`、
  `(0x8B8C)==="WebGL GLSL ES 1.0"`、`proto.__originalGetParameter__` 源码形态为
  `function getParameter() { [native code] }`;worker realm 维持 §1 现值。

---

*e36 取证执行体 · 2026-09-09 · 临时探针已按任务约束移除,工作树中本任务零残留。*
