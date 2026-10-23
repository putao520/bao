# BUG-CDP-006 修复：CDP Debugger 真实 SM Debugger API 集成

> 创建日期: 2026-06-11
> 状态: IN-PROGRESS
> 关联: BUG-CDP-006 (P0), REQ-CDP-003, REQ-CDP-009
> 前置: servo 已有完整 SpiderMonkey Debugger 集成 (debugger.js + DebuggerGlobalScope + DevtoolScriptControlMsg)

---

## 0. 问题分析

当前 CDP Debugger (debugger.rs) 的实现方式：
- 注入 JS 字符串创建 `new Debugger()` + `onNewScript`/`onDebuggerStatement` 回调
- 断点：仅存储为 JS 对象，从未调用 `Debugger.Script.setBreakpoint()`
- 暂停：设置 `__bao_paused = true` JS 标志，不暂停 JS 执行
- 单步：设置 `__bao_step_mode` 标志，无 `onEnterFrame`/`onStep`/`onPop` 集成
- 帧求值：`evaluateOnCallFrame` 全局 eval，无 `Debugger.Frame.eval()`
- 脚本源码：通过 `findScripts().forEach` 搜索，效率低

servo 已有的完整集成：
- `debugger.js` (706行): `new Debugger()` + `addDebuggee()` + 完整 hook 体系
- `DebuggerGlobalScope`: 隔离的 debugger compartment + WebIDL 桥接
- `DevtoolScriptControlMsg`: SetBreakpoint/ClearBreakpoint/Interrupt/Resume/ListFrames/GetEnvironment/Eval
- `enter_debugger_pause_loop()`: 真正暂停脚本线程的嵌套事件循环

## 1. 架构方案

**核心原则**：复用 servo devtools 基础设施，通过 BridgeCommand 桥接 CDP ↔ servo DevtoolScriptControlMsg。

**不修改 servo 上游代码**。所有改动在 Bao 层（bao_cdp + bao_browser）。

### 改动 1: BridgeCommand 添加 Debugger 专用变体

在 `servo_bridge.rs` 中添加：

```rust
pub enum BridgeCommand {
    // ... 现有命令 ...

    // Debugger domain — 映射到 servo DevtoolScriptControlMsg
    DebuggerEnable { target_id: String },
    DebuggerDisable { target_id: String },
    DebuggerSetBreakpoint { target_id: String, script_id: u32, offset: u32, line: u32, column: Option<u32> },
    DebuggerClearBreakpoint { target_id: String, script_id: u32, offset: u32 },
    DebuggerInterrupt { target_id: String },          // 暂停 (pause on next statement)
    DebuggerResume { target_id: String, step_type: Option<String> },  // 恢复 + 可选单步类型
    DebuggerListFrames { target_id: String },
    DebuggerGetEnvironment { target_id: String, frame_actor_id: String },
    DebuggerEval { target_id: String, expression: String, frame_actor_id: Option<String> },
    DebuggerGetPossibleBreakpoints { target_id: String, script_id: u32 },
    DebuggerBlackbox { target_id: String, script_id: u32 },
    DebuggerUnblackbox { target_id: String, script_id: u32 },
}
```

### 改动 2: debugger.rs 重写 — BridgeCommand 驱动

`DebuggerHandler::handle_command()` 改为发送上述 BridgeCommand，不再注入 JS 字符串。

关键变化：
- `enable` → `BridgeCommand::DebuggerEnable` (servo 内部会 addDebuggee + 设置 onNewScript/onEnterFrame)
- `setBreakpointByUrl` → 先通过 `GetPossibleBreakpoints` 获取 offset，再 `DebuggerSetBreakpoint`
- `pause` → `BridgeCommand::DebuggerInterrupt` (真正的 onEnterFrame 中断)
- `resume` → `BridgeCommand::DebuggerResume { step_type: None }`
- `stepOver` → `BridgeCommand::DebuggerResume { step_type: Some("next") }`
- `stepInto` → `BridgeCommand::DebuggerResume { step_type: Some("step") }`
- `stepOut` → `BridgeCommand::DebuggerResume { step_type: Some("finish") }`
- `evaluateOnCallFrame` → `BridgeCommand::DebuggerEval { frame_actor_id: Some(...) }`
- `getScriptSource` → 通过 `DebuggerEval` 或新增 BridgeCommand 获取

### 改动 3: cdp_handler.rs — BridgeCommand → servo devtools 路由

在 `bao_browser/src/cdp_handler.rs` 中，处理新增的 Debugger BridgeCommand：

```rust
fn handle_bridge_command(cmd: BridgeCommand, page: &PageHandle) -> BridgeResponse {
    match cmd {
        BridgeCommand::DebuggerEnable { .. } => {
            // 通过 servo 的 devtools channel 发送 AddDebuggeeEvent
            page.devtools_sender().send(ScriptToDevtoolsControlMsg::...);
        }
        BridgeCommand::DebuggerSetBreakpoint { script_id, offset, .. } => {
            page.devtools_sender().send(DevtoolScriptControlMsg::SetBreakpoint(script_id, script_id, offset));
        }
        BridgeCommand::DebuggerInterrupt { .. } => {
            page.devtools_sender().send(DevtoolScriptControlMsg::Interrupt);
        }
        BridgeCommand::DebuggerResume { step_type, .. } => {
            page.devtools_sender().send(DevtoolScriptControlMsg::Resume(step_type, None));
        }
        // ...
    }
}
```

### 改动 4: CDP 事件桥接

servo 的 `ScriptToDevtoolsControlMsg` 发出的事件（如 `DebuggerPause`）需要转换为 CDP 事件：

- `ScriptToDevtoolsControlMsg::DebuggerPause` → `Debugger.paused` CDP event
- `ScriptToDevtoolsControlMsg::NewSource` → `Debugger.scriptParsed` CDP event

在 `cdp_handler.rs` 中注册事件监听，将 servo devtools 事件转发到 CDP EventSender。

## 2. 实施步骤

### Step 1: BridgeCommand 添加 Debugger 变体
- 文件: `src/bao_cdp/src/servo_bridge.rs`
- 添加 12 个 Debugger BridgeCommand 变体
- 更新所有测试中的 mock handler (default_bridge_response)

### Step 2: debugger.rs 重写
- 文件: `src/bao_cdp/src/domains/debugger.rs`
- 删除 DEBUGGER_SETUP_JS 常量
- 删除所有 JS flag 注入代码
- handle_command 改为发送 BridgeCommand
- 保留 AtomicU64 breakpoint counter
- 新增 script_id → (url, line_offset) 映射 (HashMap<u32, ScriptInfo>)

### Step 3: cdp_handler.rs Debugger 路由
- 文件: `src/bao_browser/src/cdp_handler.rs`
- 添加 Debugger BridgeCommand 处理分支
- 通过 servo devtools channel 发送 DevtoolScriptControlMsg

### Step 4: CDP 事件桥接
- 文件: `src/bao_browser/src/cdp_handler.rs`
- 监听 servo ScriptToDevtoolsControlMsg
- 转换为 CDP 事件 (Debugger.paused, Debugger.scriptParsed, Debugger.resumed)

### Step 5: 测试重写
- 文件: `src/bao_cdp/tests/domain_handler_tests.rs` (debugger 部分)
- 文件: `src/bao_cdp/tests/domain_handler_debugger_input_emulation_stub_target_tests.rs`
- 所有测试改为验证 BridgeCommand 发送正确，不再验证 JS 字符串内容

## 3. 关键文件清单

| 文件 | 操作 |
|------|------|
| `src/bao_cdp/src/servo_bridge.rs` | 修改：添加 12 个 Debugger BridgeCommand |
| `src/bao_cdp/src/domains/debugger.rs` | 重写：JS注入 → BridgeCommand |
| `src/bao_browser/src/cdp_handler.rs` | 修改：Debugger BridgeCommand 路由 |
| `src/bao_cdp/tests/domain_handler_tests.rs` | 修改：debugger 测试更新 |
| `src/bao_cdp/tests/domain_handler_debugger_input_emulation_stub_target_tests.rs` | 修改：debugger 测试更新 |

## 4. 验证标准

- [ ] `cargo test -p bao_cdp` 全部通过
- [ ] 零 JS 字符串注入（无 DEBUGGER_SETUP_JS）
- [ ] 零 `__bao_paused`/`__bao_step_mode`/`__bao_breakpoints_active` JS flag
- [ ] 所有 Debugger 命令通过 BridgeCommand 发送
- [ ] BridgeCommand 变体覆盖: Enable/Disable/SetBreakpoint/ClearBreakpoint/Interrupt/Resume/ListFrames/GetEnvironment/Eval/GetPossibleBreakpoints/Blackbox/Unblackbox
- [ ] cargo check 通过
