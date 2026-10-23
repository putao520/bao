---
techTag: nodejs-compat
lastVerified: 2026-10-14
source: https://nodejs.org/api/dgram.html, https://nodejs.org/api/net.html, https://nodejs.org/api/vm.html, https://nodejs.org/api/child_process.html, https://nodejs.org/api/tls.html, https://nodejs.org/api/process.html (Node v26.11.1)
---

# Node.js API 兼容面行为契约(bao_runtime)领域资料库

> 来源:nodejs.org/api 六页(dgram/net/vm/child_process/tls/process,v26.11.1,2026-10-14 实抓)+ bao 仓兼容性测试群(`src/bao_runtime/tests/suite/` 211 文件,部分钉 `src/bao_browser/tests/suite/`)
> 建库触发:兼容性测试批(覆盖审计 2026-10-14)反复以「Node 官方文档契约 vs bao 实证行为」对照定红绿——dgram rinfo.size 缺口(commit 50427248a 根治)即此范式;该对照依据此前无受信沉淀,每次都要重抓文档
> 最后验证:2026-10-14

兼容性测试视角的判定总则:每条契约 = **官方文档字面**(行为真源,含版本变更标注)+ **bao 实证状态**(一致 / 已修差异 / 已知差异 / 待验 / 未核实)。红绿判定以官方契约字面为尺子,以「真往返/真进程/真 TLS 握手」的活测试为证据;绿 ≠ 语义对(断言容忍形 `|| true` 的格子不算实证)。

## 核心机制(与兼容性判定相关的)

### 1. node:dgram — `rinfo` 四字段契约(重点锚定,2026-10-14 刚根治)

**官方契约**(dgram.html `Event: 'message'`,v0.1.99 起):

- `'message'` 事件回调 `(msg, rinfo)`,`msg` 恒为 `Buffer`
- `rinfo = { address, family, port, size }` **四字段**:
  - `address` `<string>` 发送方地址
  - `family` `<string>` `'IPv4'` 或 `'IPv6'`(**v18.4.0 起字符串**;v18.0.0 一度改数字,又翻回)
  - `port` `<number>` 发送方端口
  - `size` `<number>` 报文大小(payload 字节数)
- IPv6 link-local 源地址:`address` 带 `%zone` 后缀(如 `'fe80::...%en0'`)
- `send()` 成功回调 `err === null`(v6.0.0 起,此前是 `0`);失败时 error 作 callback 首参,无 callback 则 emit `'error'`
- `bind()` 完成时 emit `'listening'` + 调用 callback;失败 emit `'error'`(极少数如已关 socket 上 bind 才 throw)
- `address()` 在未 bind 的 socket 上调用 throws `EBADF`
- `createSocket(type, callback)` 形:callback 直接挂为 `'message'` listener;禁 `new dgram.Socket()`
- 报文上限 65,507 字节(64K − UDP 头 8 − IP 头 20)

**bao 实证状态**:

| 面 | 状态 | 证据 |
|---|---|---|
| rinfo 四字段全齐 | **已修差异**(2026-10-14,50427248a) | 修前 `node_dgram.rs` emit 字面量缺 `size`(原生 `__dgram_recv` len 在手未透出);修后真 UDP 环回往返断言 `address==='127.0.0.1'` / `family==='IPv4'` / `port===txPort` / `size===payload 长度` |
| `family` 字符串形 | 一致 | 测试注释锚「Node string form since v18.4」 |
| payload 是 Buffer + 长度逐字节 | 一致 | `dgram_udp_message_payload_and_sender_rinfo` |
| send/bind callback 成功形 `err===null` | 一致 | 文件头契约注释 + 往返测试 |
| `createSocket(type, listener)` 直挂形 + `bind(options {port,address})` 形 | 一致 | 同上 |
| `connect()` / `bindSync` (v26.4.0) / multicast 族 / `%zone` 后缀 | **未核实** | 测试群零覆盖 |

测试配方:`node_dgram_inspector_deep_tests.rs`——rx/tx 双 socket 环回(127.0.0.1 临时端口),1ms `setInterval` 轮询 `__dgram_recv` 驱动 `'message'` 派发,等待闭包走 bounded drain hook 泵引擎;收尾 `close()` 双 socket 清 fd。

### 2. node:net — Socket/Server 事件序

**官方契约**(net.html):

- 客户端 socket 事件序:`'lookup'`(v0.11.3,解析 host 后、连接前;Unix socket 不适用)→ `'connect'`(v0.1.90,连接建立)→ **`'ready'`(v9.11.0,"Triggered immediately after 'connect'")** → `'data'` / `'end'` / `'close'`
- `'end'`:对端 FIN;默认(`allowHalfOpen: false`)socket 自动 `end()` 回 FIN;`allowHalfOpen: true` 则需用户显式 `end()`
- `'close'`:socket **完全**关闭后 emit,参数 `hadError` boolean
- `'error'` 之后 **`'close'` 直接跟随**(socket 面);Server 面相反——`'error'` 后**不**自动 `'close'`,除非手动 `server.close()`
- Server:`'listening'`(listen() 绑定后)→ `'connection'`(每新连接,带 net.Socket)/ `'drop'`(v18.6.0,超 maxConnections);Server `'close'` 在**全部连接结束**后才 emit
- `socket.address()` 返回 `{ port, family: 'IPv4', address }`(family 字符串,v18.4 起)
- `connectionAttempt`/`connectionAttemptFailed`/`connectionAttemptTimeout`(v21.6.0,v20.12.0):autoselection 面

**bao 实证状态**:

| 面 | 状态 | 证据 |
|---|---|---|
| `'connect'` 不在 `net.connect()` 调用内同步 fire,延到**后一 tick** | 一致 | `node_net.rs:1194` 注释明写 Node 语义;修前同步形破坏 `var c = net.connect(p,h,cb); c.on(...)` |
| `'connect'` 先于任何 `'data'`(同 deadline setTimeout 按注册序) | 一致 | connect 的 setTimeout 调度在 `_startPoll` 之前 |
| `'error'`(ECONNREFUSED)同样延迟一 tick(支持后注册 listener 形) | 一致 | 同上 |
| **DRAIN BEFORE END**:`'data'` 先排空缓冲,缓冲空 + 对端 FIN 才 `'end'`,且 **end 先于 close** | 一致 | `node_net.rs:766` 注释 + `net_peer_fin_delivers_end_event`(saw_end → sock_closed 序) |
| `allowHalfOpen=false` 默认自动 `end()`(emit `'end'`+`'close'` 并关 fd) | 一致 | `_pollTick` FIN 臂 |
| `end()` 幂等(重入 no-op,防「too much recursion」) | 一致 | `node_net.rs:664` 守卫 |
| `'data'` chunk 恒为 Buffer(非 ArrayBuffer) | 一致 | BCE-20260816-NET-DATABUFFER(修前 chunk 是 ArrayBuffer,`Buffer.isBuffer(chunk)===false`) |
| Server:`'listening'` → `'connection'`(`remotePort>0`)→ `'close'`;EADDRINUSE → `'error'` | 一致 | `net_echo_e2e_real_tcp_roundtrip`(listening 恒首条,server_closed 恒末条) |
| 环回上「server `'connection'` vs client `'connect'`」**跨 socket 无确定序**(accept 与 connect 完成落在同一次 epoll 唤醒) | 一致(官方也只定 socket 内序) | 测试只断言成员 + 确定性锚,禁跨 socket 序断言 |
| **`'ready'` 事件** | **已知差异(缺)** | `src/bao_runtime/src/node_net.rs` 零 `emit("ready")`;官方 v9.11.0 契约要求 connect 后立即 fire。源码级 grep 实证,尚无 runtime 红钉 |
| **socket `'error'` 后紧跟 `'close'`** | **已知差异(缺)** | ECONNREFUSED 路径只 `emit("error")` 不 emit `'close'`(`node_net.rs:645`);官方契约「'close' will be called directly following this event」。`p0_refused_net_close_tests` 只钉 fetch 面,不钉 net.Socket close 序 |
| `'lookup'` / `'drain'` / `'timeout'` / `connectionAttempt*` | **未核实(疑缺)** | 测试群零覆盖 |

测试配方:`net_echo_e2e_tests.rs`——真 TCP 环回 echo(server+client 同 realm),`set_post_eval_hook(bounded_drain_hook)` 泵;生命周期断言用「成员 + 首末条锚」而非全序(环回 epoll 批次依赖)。

### 3. node:vm — Realm 隔离(REQ-ENG-011)

**官方契约**(vm.html):

- `node:vm` **不是安全机制**,禁跑不可信代码
- contextify(`vm.createContext(sandbox)`):sandbox 属性变成该 realm 的全局变量;代码内对全局的写**回写** sandbox 对象;调用方realm 的同名变量不受影响
- `new vm.Script(code)` **构造即编译**(语法错在 ctor 抛 `SyntaxError`,非运行时);`script.runInContext(contextifiedObject)` 切到**目标 realm** 执行
- `vm.runInNewContext(code[, sandbox])`:每次调用新建 context;`vm.runInThisContext(code)`:当前 context;`vm.isContext(obj)` / `vm.compileFunction()`

**bao 实证状态**:

| 面 | 状态 | 证据 |
|---|---|---|
| Script ctor 立即编译(SyntaxError 在 ctor 抛) | 一致 | `script_ctor_compiles_eagerly` |
| 一 Script 两 realm:`runInContext` 切目标 realm,per-realm 全局 + per-sandbox 写穿 + **零跨 realm 污染**(断言真值 11/21、20/40,非容忍形) | 一致 | `test_vm_script_run_in_context_two_contexts_isolated`(REQ-ENG-011 criterion 3) |
| caller-realm 隔离面(Script 体运行在目标 realm,不泄漏到调用 realm) | 一致 | `test_vm_script_run_in_context_caller_realm_isolation` |
| realm 经受 FULL GC(createContext 与 runInContext 之间强制 `JS_GC(API)`) | 一致(已修差异,SM-EVOLUTION #29) | 修前 realm registry 持裸 `*mut JSObject` 于 thread-local Vec(对 tracer 不可见),全 GC 扫走 realm → opt-only SIGSEGV 族;修后 GC 边以 registered-symbol property 挂 sandbox 对象。`node_vm_gc_rooting_tests` 钉:GC 后 realm 存活/身份持久/写穿仍达/`isContext` 无假阳/丢弃最后 sandbox 后 context 仍健康 |
| `runInNewContext` / `runInThisContext` / `createContext` / `isContext` / `compileFunction` 面 | 一致(基础形) | `vm_deep_tests.rs` —— **注意该文件大量格子是 `|| true` 容忍形,只证存在性不证语义**;语义级证据以上两行为准 |
| `microtaskMode` / `importModuleDynamically` / `measureMemory` / timeout 交互 | **未核实** | 测试群零覆盖 |

### 4. node:child_process — stdio 与退出码

**官方契约**(child_process.html):

- 事件序:`'spawn'`(v15.1.0/v14.17.0,**先于一切其他事件与任何 stdout/stderr data**;spawn 失败不 fire 而改 fire `'error'`)→ stdout/stderr `'data'` → `'exit'` → `'close'`
- `'exit'` `(code, signal)`:**两者恰一非 null**(自行退出 code=最终码/signal=null;信号终止 signal=信号名/code=null);**`'exit'` 触发时 stdio 流可能还开着**
- `'close'`:进程结束**且 stdio 流关闭**后 emit;**恒在 `'exit'` 之后**(spawn 失败则在 `'error'` 后)
- `'error'` 之后 `'exit'` 可能 fire 也可能不 fire——同时监听两事件须防 handler 双调
- `child.exitCode`:运行中 `null`;信号终止时 `exitCode=null` + `signalCode` 置位
- `options.stdio`:`'pipe'`(默认,=`['pipe','pipe','pipe']`)/`'overlapped'`(Windows)/`'ignore'`/`'inherit'`(=`[0,1,2]`);数组逐 fd:`'pipe'`/`'ipc'`(至多一个,启用 `send()`)/`'ignore'`(/dev/null)/`'inherit'`(父流;fd≥3 位置等价 `'ignore'`)/Stream 对象/正整数 fd(父进程已开 fd)/null|undefined(fd 0-2 默认 pipe,fd≥3 默认 ignore)

**bao 实证状态**:

| 面 | 状态 | 证据 |
|---|---|---|
| 返回对象面 `on/once/emit/kill/stdin/pid` | 一致 | `child_process_spawn_event_lifecycle`(真 `/bin/sh` 子进程) |
| stdout `'data'` 字节 + `'exit'` `(7, null)` + `'close'` `(7, null)` 在后 + close 时 `child.exitCode===7` + close 时 stdout 内容已可读 | 一致 | 同上(含 /proc 僵尸残留负断言——close fire 证明 waitpid 已收尸) |
| `once('exit')` 恰 fire 一次 + stderr 交付 + listener throw 不祸及兄弟 | 一致 | `child_process_spawn_once_stderr_and_throw_isolation` |
| `spawn(..., {stdio:'pipe'})` / `exitCode` 属性存在性 | 一致(容忍形) | `child_process_deep_tests.rs` |
| `'spawn'` 事件 / spawn 失败 ENOENT 面(unix) | 一致(注释在案) | `child_process_spawn_events_tests.rs` 头注释「windows product correctly resolves these to ENOENT — cover on unix」 |
| stdio 数组逐 fd 全值域(`'ipc'`/Stream 对象/正整数 fd/fd≥3 默认) | **未核实** | 测试群只钉 `'pipe'` 字符串形 |
| exec/execFile/fork 家族 code 语义 + `signalCode` | **未核实** | 兼容测试群零命中 |

### 5. process.exit 语义与退出码优先级

**官方契约**(process.html):

- `process.exit([code])`(v0.1.13;**v20.0.0 只收 number 或整数字符串**,默认 0):同步终止;code 省略 → 0 或已设的 `process.exitCode`;**所有 `'exit'` listener 跑完才终止**;强杀不清异步 I/O(stdout/stderr 写可能被截断)
- `process.exitCode`(v0.11.8,默认 `undefined`):优雅退出时的码;**显式优先**——`exitCode=9; exit(42)` → **42**;Node 隐式设置(如未 settle 的 top-level await → 13)被显式设置覆盖(`exitCode=9; await 永不` → **9**)
- `'exit'` 事件:显式 `exit()` 或事件循环排空时 emit,listener 收最终码;**listener 只许同步操作**(排队的异步工作被弃);`'beforeExit'` 对显式终止**不** emit
- `'exit'` listener 无法阻止退出

**bao 实证状态**:

| 面 | 状态 | 证据 |
|---|---|---|
| `'exit'` listener 收到 code;listener 内改 `process.exitCode` **胜出**(code 3 → +4 → 7) | 一致 | `exit_listener_receives_code_and_exitcode_respected` |
| 自然退出 dispatch `'exit'` code=0;单独赋 `exitCode` 不触发退出,退出时生效 | 一致 | `natural_exit_dispatches_with_zero_and_honours_exitcode` |
| 多 listener 按注册序跑 | 一致 | `multiple_listeners_run_in_registration_order` |
| listener throw 不阻断后续 listener | 一致 | `throwing_listener_does_not_block_later_listeners` |
| `exitCode` 属性 roundtrip | 一致 | `exitcode_property_roundtrip` |
| **退出码优先级(进程级,CLI 面)**:显式 `process.exit(0/7)` > 受控码(超时 **124**、SIGINT 取消 **130**) > 错误机器 `request_exit(1)`;`--timeout 0` → clap 拒绝 exit 2;无 `--timeout` 的 SIGINT → 默认信号死(负对照) | 一致(自定面:Node 无此机器,bao 语义锚官方 exit 显式优先原则) | `bao_cli_timeout_e2e_tests.rs` T10-T13(真二进制,4 对照,stderr 双证据) |

### 6. node:tls — TLSSocket 升级路径(STARTTLS)

**官方契约**(tls.html):

- `new tls.TLSSocket(socket[, options])`:把既有明文 socket 升级为 TLS
- `'secureConnect'`(v0.11.4):握手完成后 emit,**无论证书是否被授权都调 listener**;由调用方查 `tlsSocket.authorized`(false 时看 `authorizationError`;ALPN 场景看 `alpnProtocol`)。**官方字面:该事件在 `new tls.TLSSocket()` 构造器路径不 emit**
- `'secure'`(v0.11.4):client/server TLSSocket 都 emit,**含 `new tls.TLSSocket()` 构造器路径**
- `authorized` / `authorizationError` / `alpnProtocol` / `encrypted` / `getProtocol()` 观测面

**bao 实证状态**:

| 面 | 状态 | 证据 |
|---|---|---|
| wrap 握手跑在**既有连接上**(server 侧单流:greeting→ack→ClientHello→TLS data,零重连) | 一致 | `wrap_c1_upgrade_over_existing_conn`(真 STARTTLS fixture:Rust 线程收明文 TCP,bao_boringssl_bridge `TlsServer` 升级) |
| wrap 路径 **`'secureConnect'` fire** + `getProtocol()` 真实协议 | 一致(行为)/**与官方文档字面分歧(待核 Bun 侧语义)** | 官方说构造器路径只应 fire `'secure'`;bao 钉的是 `'secureConnect'`(C1-C3 全按此断言,STARTTLS 消费者惯形)。`'secure'` 事件是否也 fire=未核实 |
| `authorized=true`(ca + 默认 rejectUnauthorized)/ `authorized=false`(`rejectUnauthorized:false`,握手照常完成) | 一致 | `wrap_c2_secure_connect_authorized` |
| ctor 响亮错误面(非 net.Socket 对象 / `isServer:true`)不弄死 socket | 一致 | C2 锁 |
| **明文期 RX 重放**:升级前收到的明文字节在 `'secureConnect'` **之前**作为**第一条 `'data'`** 重投;握手完成前的 write 停车后走 TLS 通道投递 | 一致(超越官方字面的工程契约,官方无明文期缓冲条文) | `wrap_c3_plaintext_phase_buffered`(server 侧双证明) |
| `_tls_wrap` 模块 = `node:tls` 同一对象别名 | 一致(Bun parity,HardcodedModule.zig 同形) | `node_tls_wrap.rs`(31 行 alias) |
| ALPN / session 重用 / keylog / renegotiation | **未核实** | 测试群零覆盖 |

## AI 易误判点

- ❌ 误判:`rinfo` 只有 address/port/family 三字段(`size` 可有可无)。
  ✅ 正解:官方契约恒四字段 `{address, family, port, size}`,`size`=载荷字节数。bao 已于 50427248a 补齐(2026-10-14);断言时四字段全查。
- ❌ 误判:`family` 是数字(4/6)——SM/V8 老版本行为。
  ✅ 正解:v18.4 起 dgram rinfo 与 `socket.address()` 的 `family` 均为字符串 `'IPv4'`/`'IPv6'`;v18.0.0 一度改数字是翻转窗口,勿以此为准。
- ❌ 误判:net `'connect'`(连同 connect callback)在 `net.connect()` 调用语句内同步 fire。
  ✅ 正解:官方语义为连接建立后(异步);bao 显式钉「延到后一 tick」——否则 `var c = net.connect(p,h,cb); c.on(...)` 形里 callback 跑在 `c` 还未赋值时。
- ❌ 误判:对端 FIN 后立刻 emit `'end'`(可能还有缓冲数据没投)。
  ✅ 正解:DRAIN BEFORE END——先排空 `'data'`,缓冲空才 `'end'`,end 恒先于 close。
- ❌ 误判:环回测试里 server `'connection'` 与 client `'connect'` 有确定先后序。
  ✅ 正解:两者与 accept/connect 完成落在同一次 epoll 唤醒,顺序是批次依赖;断言用成员 + socket 内确定序(`'end'`→`'close'`、listening 首条、server close 末条)。
- ❌ 误判:child `'exit'` 与 `'close'` 同义可互换。
  ✅ 正解:`'exit'`=进程终止(此时 stdio 可能还开);`'close'`=stdio 流也关了(恒在 exit 后,或 spawn 失败时在 error 后);close 才证明 waitpid 收尸(无僵尸)。
- ❌ 误判:`process.exit(1)` 后 `process.exitCode=7` 会覆盖退出码 / 反之 exitCode 先设后 `exit()` 会被忽略。
  ✅ 正解:显式 `exit(code)` 参数恒胜(`exitCode=9; exit(42)` → 42);listener 内改 exitCode 胜过传入码(bao 与官方一致钉)。
- ❌ 误判:`'exit'` listener 里还能调度异步工作。
  ✅ 正解:listener 只许同步操作,退出不可阻止、排队异步工作被弃。
- ❌ 误判:`'secureConnect'` 在任何 TLSSocket 上都会 fire / 或构造器路径只该用 `'secure'`。
  ✅ 正解:官方字面=构造器路径不 emit `'secureConnect'`(只 `'secure'`);bao 的 STARTTLS wrap 钉 `'secureConnect'`(与官方字面分歧,消费侧惯形优先)。写兼容测试前先定基准(官方 vs Bun),勿混用。
- ❌ 误判:`vm` 模块可以当沙箱跑不可信代码。
  ✅ 正解:官方明文「not a security mechanism」;兼容面要钉的是 realm 隔离语义(per-realm 全局/写穿/零串扰),不是安全边界。
- ❌ 误判:`vm.Script` 语法错误在 `runInContext` 时才抛。
  ✅ 正解:ctor 构造即编译,SyntaxError 在 `new vm.Script()` 处抛。

## 测试配方(bao 兼容性测试群惯形)

1. **引擎内面**(dgram/net/vm/process event):`JsContext::for_test()` + `set_global_setup(globals::install_all)` + **`set_post_eval_hook(bounded_drain_hook)`**(生产泵路径);等待闭包只泵引擎(bounded drain / timer 派发),**禁纯 sleep 轮询**(见 memory:live 测试等待窗必须驱动泵)。
2. **真 I/O 面**:UDP/TCP 走 127.0.0.1 环回临时端口;TLS 走 Rust 线程真 fixture(`bao_boringssl_bridge::TlsServer`);子进程走真 `/bin/sh`,close 后加 /proc 非僵尸断言。
3. **进程级面**(退出码优先级):真二进制 subprocess(`BAO_BIN` env 定位,`cargo build -p bao_bin` 前置,缺失 skip 非 fail);经 `exit_isolation::dispatch_timeout` 包 crash-class 测试体。
4. **GC 面的 realm 断言**:强制 `JS_GC(cx, GCReason::API)` 前后各断言一次状态(realm 身份持久/写穿仍达)。

## 已知问题 / 边界

- **net `'ready'` 缺**(官方 v9.11.0:connect 后立即 fire)——源码级实证,无 runtime 红钉;同类候选:`'lookup'`/`'drain'`/`'timeout'`/`connectionAttempt*`。
- **net socket `'error'` 后无 `'close'`**(官方:直接跟随)——ECONNREFUSED 路径只 emit error;`fetch` 面有钉,`net.Socket` 面无。
- **TLS wrap 路径 `'secureConnect'` 与官方字面分歧**(官方:构造器路径不 emit)——行为三面(C1-C3)已钉绿,Bun 侧基准未核实;裁基准前禁改行为。
- **`vm_deep_tests.rs` 大量 `|| true` 容忍格子**——只证存在性不证语义;语义证据看 `test_vm_script_run_in_context_*` 与 `node_vm_gc_rooting_tests`(真值断言)。
- **未核实域**(本条目视角下零覆盖,建测试前先立红钉):dgram `connect()`/`bindSync`/multicast/`%zone`;net lookup/drain/timeout;child_process stdio 数组全值域与 exec 家族;tls ALPN/session/keylog;`process.exit` 非整数实参拒绝(v20.0.0 形)。
- CLAUDE.md `# 领域资料库索引` 行与 `hooks/lib/tech-dict.mjs` TECH_DICT 注册(本条目 scope 外)由主会话收口补登,否则 kb_index_check 会报 file-but-not-indexed。


## 2026-10-24 GOLDEN A 实证契约追加(bao 侧根治记录)

1. **require() CJS 目录索引永不看 index.mjs**(Node v24 oracle 双探针):LOAD_AS_DIRECTORY 序=.js>.json>.node;仅 index.mjs 时 MODULE_NOT_FOUND;显式全名 `.mjs` 才走 require(ESM)。bao 根治=require.rs correct_directory_index_mjs 挂 resolve_specifier 全部返回路径(bun_resolver Stmt 序对 require 面必须校正,ESM import 面不动)。
2. **Buffer.prototype.constructor === Buffer**(writable/non-enumerable/configurable):NewPlainObject 造 prototype 后 constructor 沿 __proto__ 链错指 Uint8Array——assert inspect 的 Buffer 判别链断裂根因(bao globals.rs 单行修)。
3. **vm 六顶层函数全部可构造**(runInNewContext/runInThisContext/runInContext/createContext/isContext/compileFunction):`new vm.fn(code)` 返回求值结果(SM 构造调用 rval override,JSFUN_CONSTRUCTOR=0x400 注册;Script.prototype 方法不可构造)。
4. **net.getDefaultAutoSelectFamilyAttemptTimeout** 默认 250ms 进程级 AtomicU64;set 校验=非 number→ERR_INVALID_ARG_TYPE,负/非 finite→ERR_OUT_OF_RANGE,0 允许。
5. **process.features 13 字段**(v24):inspector/debug/uv/ipv6/tls_alpn/tls_sni/tls_ocsp/tls/openssl_is_boringssl/cached_builtins/require_module/quic/typescript("strip"=TS 直跑支持)——值按实现如实标定非 Node 默认值(boringssl 构建 openssl_is_boringssl=true)。
6. **node_* 模块三段式导出陷阱**:NET_JS IIFE return 块+install 白名单拷贝清单+单测断言清单三处必须同步(IIFE 加导出不进白名单=require 面静默 undefined,编译绿假象)。
