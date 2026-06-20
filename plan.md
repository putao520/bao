# 开发计划: BCE 根治 GC-unsafe Handle 模式 | epoch: 4 | status: active

## Epoch 3 交付回顾

| BCE | 状态 | Commit |
|-----|------|--------|
| BCE-002 (to_private 守卫 17 处) | ✅ 已根治 | TASK-40 + TASK-44 |
| BCE-003 (timer 死锁) | ✅ 已根治 | TASK-40 + TASK-45 |
| BCE-004 (bundler minify) | ✅ 已根治 | TASK-40 |
| BCE-005 (server.port) | ✅ 已根治 | TASK-40 + TASK-44 |
| BCE-006 (assert/strict) | ✅ 已根治 | TASK-40 |
| BCE-012 (stale Handle — 首批 37 文件) | ✅ 已根治 | 3396741c |

## Epoch 4 交付

| BCE | 状态 | Commit |
|-----|------|--------|
| BCE-012-R2 (Handle 构造残留 91 处) | ✅ 已根治 | 83bcc545 + ae27d14e |
| BCE-013 (to_object 未 rooted 35 处) | ✅ 已根治 | 83bcc545 + ae27d14e |

### 5 Agent 并行根治详情

| Agent | 文件域 | 修复数 | 状态 |
|-------|--------|--------|------|
| TASK-1 | fetch_async.rs | 17 | ✅ |
| TASK-2 | bun_sm/{gc,strong,value,js_value,method_jsc,regular_expression}.rs | 17 | ✅ |
| TASK-3 | node_dns.rs, node_util.rs, bun_test.rs, gc_store.rs, timers.rs, bao_browser_global.rs, globals.rs | 40 | ✅ |
| TASK-4 | bun_sm/{js_promise,host_fn,global_object,js_object}.rs | 17 | ✅ |
| TASK-5 | node_events.rs, globals.rs, node_timers_module.rs, bun_sqlite.rs, node_http.rs, node_buffer.rs, bun_ffi.rs, node_crypto.rs, web_api.rs | 53 | ✅ |

### 残留确认

- `Handle::<*mut JSObject> { ptr: &non_null_local }` = **0** across src/
- `Handle::<Value> { ptr: &local }` (ObjectValue/StringValue) = **0** across src/
- Build: `cargo check` 0 errors
- Tests: 595(bun_runtime) + 203(bun_sm) = **798 pass**

### 已知残留（不在当前文件域）

- `bao_browser/src/runtime_bridge.rs`: 3 处 `from_marked_location` 用未 rooted 全局变量
- 需要下一 epoch 处理

## 铁律
1. 文件域:只改 task.file,bao_runtime/bun_sm 内按文件分(不撞)
2. 编译验证:每 task cargo check 通过
3. 复用 > 手写(rooted! macro + .handle().into() 统一模式)
4. 无 TODO/FIXME/stub/console.log
5. 禁改 bun_* 上游 + servo 上游(bun_sm 是 BAO SM 封装层,可改)
6. HandleValueArray 用 local Value copy 模式: `let elem = ObjectValue(obj); &elem as *const Value`
7. 安全模式保留: MutableHandle / Int32Value/DoubleValue/BooleanValue/UndefinedValue/PrivateValue 在 Handle 中
