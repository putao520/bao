# 开发计划: BCE 根治 GC-unsafe Handle 模式 | epoch: 5 | status: active

## Epoch 3-4 交付回顾

| BCE | 状态 | Commit |
|-----|------|--------|
| BCE-002/003/004/005/006 (验收失败 5 类) | ✅ 已根治 | TASK-40/44/45 |
| BCE-012 (stale Handle — 首批 37 文件) | ✅ 已根治 | 3396741c |
| BCE-012-R2 残留 + BCE-013 (epoch 4 首批) | ✅ 已根治 | 83bcc545 + ae27d14e |

## Epoch 5: BCE-012 真实残留根治

### 关键教训
Epoch 4 的 Agent 报告了**虚假成功**：单行 grep 被 rustfmt 多行格式化欺骗，
`Handle::<...> { ptr: &X }` 被拆成多行后单行 grep 抓不到。改用**行扫描器**
（逐行找 `ptr: &`，向上回溯 Handle 类型，覆盖 bare/qualified 全路径形态）确认
真实残留后根治。

### 行扫描器 v3（可信验证工具）
覆盖 `Handle::<*mut JSObject>`、`Handle::<Value>`、`Handle::<JSVal>`、
`Handle::<*mut JSString>` 全部形态，区分安全（MutableHandle / 非 GC 值）与危险。

### 根治清单（epoch 5，40+ 处，commit 6d882278）

| 文件 | 处数 | 模式 |
|------|------|------|
| fetch_async.rs | 14 | HandleObject + HandleValue(Object/String) |
| bun_sm/gc.rs | 8 | HandleObject(global) + HandleValue(ObjectValue) |
| bun_sm/{strong,value,js_value,method_jsc,regular_expression,js_promise,host_fn,global_object,js_object}.rs | 18 | HandleObject + HandleValue |
| bao_browser/runtime_bridge.rs | 3 | from_marked_location(global) |
| node_path/node_crypto/js_value/value/initialize | 6 | from_marked_location(&local JSVal) |
| bun_sm/{async_module,host_call}.rs | 7 | qualified-form Handle (mozjs::jsapi::) |
| require.rs | 1 | Handle<*mut JSString>(js_str) |
| node_https.rs | 1 | val 未root (exports loop) |

### 残留确认（行扫描器 v3）
- HandleObject/String dangerous: **0**
- HandleValue/JSVal dangerous: **0**
- from_marked_location(&local): **0**
- Build: `cargo check` 0 errors
- Tests: 595(bun_runtime) + 203(bun_sm) = **798 pass**

## BCE-013 to_object 未 rooted 状态

| 模式 | 状态 |
|------|------|
| `let X = val.to_object()` 后跨 GC 使用 | ✅ epoch 4 已根治 (node_events 19, globals 17 等) |
| `let X = val.to_object()` 后立即 `rooted!(... let X_root = X)` | ✅ 安全模式 (无 GC 间隔，node_dns/node_stream/node_tls/node_querystring/node_net/node_zlib/node_string_decoder) |

行扫描器确认 to_object 裸用残留 = 0。

## 铁律
1. 文件域:只改 task.file,按 crate 内文件分(不撞)
2. 编译验证:每 task cargo check 通过
3. 复用 > 手写(rooted! macro + .handle().into() 统一模式)
4. 无 TODO/FIXME/stub/console.log
5. 禁改 bun_* 上游 + servo 上游(bun_sm 是 BAO SM 封装层,可改)
6. HandleValueArray 用 local Value copy 模式: `let elem = ObjectValue(obj); &elem as *const Value`
7. 安全模式保留: MutableHandle / Int32Value/DoubleValue/BooleanValue/UndefinedValue/PrivateValue 在 Handle 中
8. **验证铁律**: Agent 自我验证不可信，必须用行扫描器独立确认残留=0（单行 grep 会被多行格式欺骗）
