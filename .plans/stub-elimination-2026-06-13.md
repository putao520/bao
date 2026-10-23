# Stub/Hack/降级 彻底消除计划

## 分类标准

| 类别 | 定义 | 行动 |
|------|------|------|
| A-可立即实现 | 有明确的技术路径，依赖已可用 | 立即实现 |
| B-需外部依赖 | 需要尚未集成的 crate 或 API | 桥接+真实实现 |
| C-设计限制 | JS 层面无法真正实现（如 Fetch pause） | 保留但标注设计决策 |
| D-有意 no-op | SM 架构决定（如 EnsureStillAlive） | 不动 |

---

## A 类：可立即实现（优先执行）

### A1. bun_sm/debugger.rs — 真实 SM Debugger API 集成
- **当前**：enable/disable 返回 NotBound 错误
- **目标**：集成 `mozjs::jsapi::Debugger` API（`JS_NewDebugger`, `JS_SetDebuggerOnNewGlobalObject`）
- **实现**：创建 Debugger 对象并挂载到 JSContext，管理 breakpoint 存储

### A2. bun_sm/async_module.rs — 真实 top-level await 追踪
- **当前**：所有方法返回硬编码值（is_fulfilled=true, is_rejected=false, result=UNDEFINED）
- **目标**：集成 SM `JS::ModuleAsync` 状态查询
- **实现**：
  - AsyncModule 持有 `*mut JSObject`（模块对象）
  - is_fulfilled/is_rejected 通过 SM API 查询模块状态
  - Queue.on_wake_handler / on_dependency_error 注册真实回调

### A3. bun_sm/host_call.rs — 真实值传递
- **当前**：`to_js_host_call` 直接 clone JSValue
- **目标**：使用 `JS_CallFunctionName` 进行跨模块函数调用
- **实现**：
  - `to_js_host_call(value)` → 将值包装为可调用对象
  - `from_js_host_call(value)` → 解包调用结果
  - `to_js_host_fn_result(result)` → 提取结果或抛出异常

### A4. bun_sm/initialize.rs — 真实 eval+print
- **当前**：eval_and_print 空函数，initialize 空函数
- **目标**：调用 JsContext::eval + 打印结果
- **实现**：
  - `eval_and_print(cx, source, filename)` → JS::Evaluate2 + 打印 rval
  - `initialize()` → 初始化 JS runtime（注册标准类等）

### A5. bun_sm/node_path.rs — 完整 path 模块实现
- **当前**：install 为空，posix_join 简单 join("/")，normalize 直接返回
- **目标**：移植 Node.js path 模块核心函数
- **实现**：
  - install() 注册 JS path 对象，定义 join/resolve/normalize/dirname/basename/extname/parse/format/isAbsolute/relative
  - posix_join: 正确处理 ./ ../ 等路径段
  - normalize: 正确解析 . 和 ..

### A6. bun_sm/method_jsc.rs — 方法注册系统
- **当前**：install 为空
- **目标**：提供 JSC 风格的方法注册宏辅助
- **实现**：定义 install() 将方法名→JSNative 映射注册到全局对象

### A7. bun_sm/headers_jsc.rs — Headers JS 对象
- **当前**：install 为空
- **目标**：创建 JS Headers 对象，支持 get/set/has/append/delete/forEach
- **实现**：注册 Headers 构造函数到全局对象

### A8. bao_runtime/node_dns.rs — dns_resolve6 真实 IPv6 解析
- **当前**：dns_resolve6 总是返回空数组
- **目标**：使用 `to_socket_addrs` 过滤 IPv6 地址
- **实现**：复用 dns_resolve 逻辑，添加 IPv6 过滤

### A9. bao_runtime/node_dns.rs — dns_reverse 真实反向查找
- **当前**：返回 IP 本身作为 hostname
- **目标**：使用 lookupAddr 或 getnameinfo 进行反向 DNS
- **实现**：集成 `dns-lookup` 或手写 getnameinfo FFI

### A10. bao_runtime/bun_api.rs — process EventEmitter 真实实现
- **当前**：process_noop 处理 emit/off/removeListener/removeAllListeners
- **目标**：真实 EventEmitter 实现
- **实现**：基于 gc_store 注册事件监听器，emit 时查找并调用

### A11. bao_runtime/bun_api.rs — stdin stream 真实实现
- **当前**：stdin_noop 处理 resume/pause/destroy
- **目标**：从 stdin 读取数据并推送
- **实现**：使用 bun_io 的非阻塞 stdin 读取

### A12. bao_runtime/node_tls.rs — TLSSocket 关键方法
- **当前**：getFinished/getPeerFinished/getSession 返回硬编码值
- **目标**：集成 rustls 获取真实 TLS 会话数据
- **实现**：
  - getFinished: 从 rustls 连接提取 verify data
  - getPeerFinished: 同上（peer 端）
  - getSession: 从 rustls 提取 session ticket

---

## B 类：需外部依赖（次优先执行）

### B1. bun_sm/ipc.rs — 真实 IPC 通道
- **依赖**：bun_io::StreamBuffer + JSONLineBuffer
- **实现**：基于 Unix socket 的消息帧协议
- **当前**：send() 返回 NotImplemented 错误

### B2. bun_sm/web_worker.rs — Worker 线程
- **依赖**：SM JSContext+Runtime per thread
- **实现**：std::thread + mozjs::Runtime::new() per worker
- **当前**：所有方法 no-op

### B3. bun_sm/hot_reloader.rs — 文件监听
- **依赖**：bun_watcher crate
- **实现**：notify crate 或 bun_watcher 监听文件变化
- **当前**：所有方法 no-op

### B4. bun_sm/runtime_transpiler_cache.rs — 转译缓存
- **依赖**：bun_cache crate
- **实现**：HashMap + 文件系统缓存
- **当前**：IS_DISABLED=true, get/set/clear 全 no-op

### B5. bun_sm/bun_cpu_profiler.rs — CPU 性能分析
- **依赖**：mozjs GeckoProfiler API 绑定
- **实现**：JS_SetProfilingCallbacks + 采样数据收集
- **当前**：所有方法 no-op

### B6. bao_runtime/dispatch.rs — 11 个 FilePoll tag
- **依赖**：对应 bun_* crate 的实现
- **实现**：逐个接入 FILE_SINK/DNS_RESOLVER 等
- **当前**：空 match arm

### B7. bao_runtime/node_readline.rs — 真实终端交互
- **依赖**：bun_io + termios
- **实现**：TTY 控制 + 键盘事件
- **当前**：createInterface 的 on/close/pause/resume/write/prompt/setPrompt/question 全 noop

### B8. bao_runtime/web_api.rs — wss:// 支持
- **依赖**：rustls/openssl TLS 集成
- **实现**：WebSocket + TLS 层组合
- **当前**：显式拒绝 wss://

### B9. bao_runtime/node_crypto.rs — RSA 签名
- **依赖**：rsa/openssl crate
- **实现**：真实 RSA-SHA256 签名
- **当前**：降级到 HMAC

### B10. bao_cdp — 12 domain 的 no-op 命令
- **依赖**：servo bridge 真实连接
- **实现**：通过 servo_bridge 发送真实 CDP 命令
- **当前**：大量 acknowledge-only 命令

---

## C 类：设计限制（标注不修改）

### C1. bao_cdp Fetch domain — JS 层拦截无法真正 pause
- 原因：servo 的 Fetch 在 C++ 层执行，JS 层无法暂停
- 行动：文档标注，保持 no-op acknowledge

### C2. bao_cdp Runtime.evaluateAsync — 异步 eval 需要 event loop 集成
- 原因：需要与 SM JobQueue 集成
- 行动：保持当前返回 undefined，标注为 event loop 依赖

### C3. bun_sm/ensure_alive.rs — SM 架构决定
- 原因：SM 用显式 rooting，不需要 JSC 的保守 GC 守卫
- 行动：保持 no-op，这是正确行为

### C4. bun_sm/mark_binding.rs — SM 架构决定
- 原因：SM 用显式 rooting，不需要 JSC 的绑定标记
- 行动：保持 no-op，这是正确行为

### C5. bao_runtime/bun_test.rs — describe.skip/it.skip 等
- 原因：这些在 Node.js 中也是有意 no-op
- 行动：保持不变

### C6. bao_engine/context.rs — shutdown_thread_sm() 空函数
- 原因：SM C++ TLS 状态由 OS 回收，ManuallyDrop 防止析构
- 行动：保持不变，这是正确的生命周期管理

---

## 执行计划

### 批次 1（A1-A7）：bun_sm 层真实实现
A1 debugger → A2 async_module → A3 host_call → A4 initialize → A5 node_path → A6 method_jsc → A7 headers_jsc

### 批次 2（A8-A12）：bao_runtime 层真实实现
A8 dns_resolve6 → A9 dns_reverse → A10 process EventEmitter → A11 stdin stream → A12 TLSSocket methods

### 批次 3（B1-B5）：需依赖的 bun_sm 模块
B1 ipc → B2 web_worker → B3 hot_reloader → B4 runtime_transpiler_cache → B5 bun_cpu_profiler

### 批次 4（B6-B10）：需依赖的 bao_runtime/CDP 模块
B6 FilePoll tags → B7 readline → B8 wss → B9 RSA signing → B10 CDP domains

### 每批次验证
- `cargo check -p bun_sm` 0 error, 0 warning
- `cargo check -p bao_runtime` 0 error
- `cargo test -p bun_sm` 全绿
- `cargo test -p bao_runtime` 全绿
- `cargo test -p bao_cdp` 全绿
