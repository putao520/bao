# incumbent_global 解析优先级波(立项设计,2026-10-02)

## 现象(e16 探针,/tmp/wpt_crash_semantics.py)
listener 于 iframe realm 创建、click 自 top 派发 → 回调内 `e.source === top`(spec 预期 iframe window)。

## 已核实的机制底账(主会话读码,2026-10-02)
两套栈,分工明确,形似忠实实现:
- `script_bindings/settings_stack.rs` `run_a_callback`(AutoIncumbentScript):HTML spec "prepare-to-run-a-callback" Step 2-3 = **HideScriptedCaller**(回调体内 JS 引擎"说谎"无 caller)→ push Incumbent entry;clean-up 后 UnhideScriptedCaller。WebIDL `call_setup`(script_bindings/callback.rs:365)确按规范走此封装。
- `script/dom/bindings/settings_stack.rs:56` `incumbent_global()`:Step 1/3 先 `GetScriptedCallerGlobal`(非空即返)→ Step 2 落显式栈顶。

**互锁正确性依赖一个前提**:凡 WebIDL stored callback 必经 call_setup(Hide 过 caller)。若存在**不经 call_setup 的调用路径**(native 直调 listener、promise job、定时器泵、或 cross-realm dispatch 的 invoke 面),caller 未隐藏 → Step 1 拿到 JS 栈顶真 caller(本例:top 的 click 派发栈)→ incumbent= top ≠ callback 注册 realm。

## 波的假设树(待证)
H1 存在绕过 call_setup 的 listener invoke 路径(event dispatch 面 eventhandler.rs?)——查 eventhandler 的 invoke 是否走 CallSetup/AutoIncumbentScript。
H2 postMessage 的 e.source 捕获时点用错 settings(捕获应发生在 postMessage 调用者上下文,而非 listener 上下文)——若 e.source 在 listener 侧才解析,则 incumbent 面只是表象。
H3 跨 realm dispatch 的 spec 语义本身(spec: incumbent = "回调被排队时调用者的 settings"经 job/callback 链传播) servo 缺 job 面的 settings 传播(bao_run_in_script_settings 的 settings-stack 借用面是相关前情,BCE-20260910-004)。

## 破坏面评估
incumbent 影响全体 WebIDL 回调的"回调内再入权限面"(e.source/postMessage target/权限判定)。修复 = 只允许以下二选一:
- A:补齐绕行路径的 call_setup 包裹(每个绕行点对齐 spec,风险局部)
- B:改 incumbent_global 解析序(全局语义变,影响面大,须全 WPT+双锚回归)
**禁**:在 MessageEvent.source 单点打补丁(掩盖性修)。

## 验收 oracle
1. e16 判别探针转绿(e.source === iframe window)
2. WPT 40 文件全量不回归 + 双真客户端锚绿
3. WebIDL 回调面 scoped 回归(event/media 全绿)

## 环境底账
- 上游对照:~/code/tools/servo;上游同形实现,其官方测试被 same-origin 松弛掩蔽(e16 已证)——上游 issue 候选(修后可报)
- 探针资产:/tmp/wpt_crash_semantics.py(判别链在 WPT-FIRST-RUN.md「NO-HARVEST 定性」节)
