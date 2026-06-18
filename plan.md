# 开发计划: BCE 根治 8 个验收失败 bug | epoch: 3 | status: active

## 归因完成(BCE 阶段 0-3)
8 个验收失败 bug(JS 5 + Rust 3)聚为 5 类 BCE,根因 + 横扫 + 根治方向已明确。

### BCE-20260618-002: JSVal to_private 守卫缺失(17 处)
- 现象: test_acceptance.js `http.createServer(fn).close()` 不 listen → server_close 取 `_appPtr`(undefined)→ `to_private()` 触发 SM `assert!(self.is_double())` panic → extern "C" 边界 panic_cannot_unwind → abort,阻断 172 SPEC criteria 覆盖
- 根因: node_http.rs:649/662/284 等 17 处 `.to_private()` 缺 `is_double`/`val_is_private` 守卫(undefined JSVal tag ≠ PrivateValue 编码)
- 根治: 每处 `to_private()` 前加 `val_is_private` 守卫,复用 node_tls.rs:47 `val_is_private(v) -> bool { v.is_double() && (v.asBits_ & 0xFFFF000000000000) == 0 }`,未守卫返回默认/None/false
- 已守卫参照: node_tls.rs:47-49 / node_events.rs:253-258 / node_url.rs:208-212

### BCE-20260618-003: CLI 事件循环 timer-only 死锁
- 现象: test_event_loop_order.js + phase1_integration.js timeout 30s 卡死
- 根因: timers.rs:164 `drain_and_check` 在 timer-only(BAO_REGISTRY 有 timer,uSockets heap 空)调 `tick_once(null)` → epoll_wait 永久阻塞;context.rs:459 loop 恒 true
- 根治: `drain_and_check` 复制 `drain_one_pass`(timers.rs:217-222)防阻塞逻辑 — timer-only 分支用 `sleep(1ms)` 兜底,不调 tick_once

### BCE-20260618-004: bundler minify 遗漏注释移除
- 现象: 3 Rust 测试(build_with_minify_removes_comments / e2e_bundle_simple_module / e2e_bundle_minify_collapses_complex_js)失败,断言 `!contains("comment")` 等
- 根因: bao_bundler/lib.rs `build()` minify 用 `collapse_whitespace`(只折叠空白),注释保留
- 根治: minify 路径实现完整注释移除 — 用 `bun_transpiler` SWC minify 选项(drop comments)或 SWC JscMinifyOptions

### BCE-20260618-005: Bun.serve server.port / port:0 动态绑定
- 现象: HTTP-001 `Bun.serve({port:0})` 后 `server.port` 不 > 0
- 根因: bun_api.rs `bun_serve` 的 server 对象 port 属性未暴露实际绑定端口(ls_port at line 1453)
- 根治: server 对象暴露 `port` getter 返回 `ls_port`(实际绑定端口)

### BCE-20260618-006: assert/strict 注册冲突
- 现象: NFR-SEC-001 `require("assert/strict")` unavailable
- 根因: node_util.rs:468 `cache_builtin("assert/strict", ...)` 注册,但 node_stubs.rs:66 列出 + line 84 "Skip registration" 覆盖导致 stub 跳过实际注册
- 根治: 移除 node_stubs.rs 对 assert/strict 的 skip(让 node_util.rs 注册生效),或统一注册路径

## 任务(文件域按 crate 内文件细分,disjoint)
- TASK-1 BCE-002-A: node_http(3)+node_tls(4)+node_url(1)+bun_ffi(1)+bun_sqlite(2)守卫
- TASK-2 BCE-002-B+005: bun_api(守卫1 + server.port)+bun_sm(守卫2)
- TASK-3 BCE-003+006: timers(drain_and_check)+node_util/node_stubs(assert/strict)
- TASK-4 BCE-004: bao_bundler/lib.rs minify 注释移除

## 铁律
1. 文件域:只改 task.file,bao_runtime 内按文件分(不撞)
2. 编译验证:每 task cargo check 通过
3. 复用 > 手写(val_is_private / drain_one_pass / SWC minifier)
4. 无 TODO/FIXME/stub/console.log
5. 禁改 bun_* 上游 + servo 上游(bun_sm 是 BAO SM 封装层,可改)
