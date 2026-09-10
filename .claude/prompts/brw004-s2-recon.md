# BRW-004 C19 S2 前置侦察报告(只读盘点)

- 日期:2026-09-09
- 范围:servo 上游 SW fetch 拦截现状(vendor 快照 2026-08-13 上游 HEAD + Bao 定制)
- archaeology 锚点:`serviceworkerglobalscope.rs:598` TODO XXXcreativcoder "This will eventually use a FetchEvent interface to fire event"
- S1 已收口:f77faf8b(dom_serviceworker_enabled 翻开 + WebViewId-keyed drain_worker_scope_callbacks)

> 路径勘误:任务书写 `dom/serviceworker/`(单数)为正确路径;不存在 `dom/serviceworkers/`。

---

## ① FetchEvent 上游缺口清单

### 现状::598 处 fire 什么

`vendor/servo/components/script/dom/serviceworker/serviceworkerglobalscope.rs:597-603`(`handle_script_event` 的 `Response(mediator)` 分支):

```rust
Response(mediator) => {
    // TODO XXXcreativcoder This will eventually use a FetchEvent interface to fire event
    // when we have the Request and Response dom api's implemented
    // https://w3c.github.io/ServiceWorker/#fetchevent-interface
    self.upcast::<EventTarget>().fire_event(cx, atom!("fetch"));
    let _ = mediator.response_chan.send(None);
},
```

即:fire 一个**裸 `Event`(atom!("fetch"))**,不带 Request payload、无 FetchEvent 类型、无 respondWith;随后**恒 `response_chan.send(None)`**(pass-through,零拦截)。

### 缺口类型对照(对照 webidl 暴露面)

| 缺口 | 证据 |
|---|---|
| **FetchEvent 接口整体不存在** | 全树唯一 `FetchEvent` 引用 = :598 的 TODO 注释;`components/script_bindings/webidls/` **无 FetchEvent.webidl** |
| **respondWith 不存在** | 全树 grep `respondWith` 零命中 |
| **onfetch 未暴露** | `ServiceWorkerGlobalScope.webidl`:`//attribute EventHandler onfetch;` 处于注释态;`oninstall`/`onactivate`/`clients`/`registration`/`skipWaiting` 同样注释态,当前只暴露 `onmessage`/`onmessageerror` |
| **TODO 注释已 stale** | :599 声称前提 "when we have the Request and Response dom api's implemented" 已失效:`Request.webidl` / `Response.webidl` 均已存在(`[Exposed=(Window,Worker)]`,含 constructor) |
| NavigationPreloadManager 类型存在但非 SW 语境 | `navigationpreloadmanager.rs`(134 行)+ `NavigationPreloadManager.webidl`(`[Exposed=(Window,Worker)]`,经 `ServiceWorkerRegistration.navigationPreload` 暴露);preloadResponse 与 fetch 事件无任何接线 |
| SW realm 实际可用面 | `ServiceWorkerGlobalScope.webidl`:`[Global=(Worker,ServiceWorker), Exposed=ServiceWorker, Pref="dom_serviceworker_enabled"]`。因 Global 名含 `Worker`,Request/Response/`fetch()`(均 `Exposed=(Window,Worker)`)经 Worker 名在 SW global 可达——**无需为 Request/Response 补 Exposed**;FetchEvent 落地时应 `[Exposed=ServiceWorker]` |

---

## ② fetch 管线与 SW 的分派点(上游现有分派代码在哪)

### net 层(上游预留的合法分派点,当前是死 TODO)

`vendor/servo/components/net/http_loader.rs:559` `pub(crate) async fn http_fetch(...)`,SW 分派检查在 **:578-600**:

```rust
// Step 3. If request's service-workers mode is "all", then
if request.service_workers_mode == ServiceWorkersMode::All {
    // TODO: Substep 1
    // Set response to the result of invoking handle fetch for request.

    // Substep 2
    if let Some(ref res) = response {   // response 恒 None → 此分支死码
        ...
    }
}
```

- `response: Option<Response>` 在 :571 声明后从未赋值 → "invoke handle fetch" 未实现,fetch 恒走 Step 4 网络路径。
- 该点即 fetch spec "HTTP fetch → handle fetch" 的规范位置,**上游有意把 SW 分派放在 net 层 http_fetch,不是 script fetch 层**。

### 请求字段已就绪

- `shared/net/request.rs:117` `pub enum ServiceWorkersMode { All, None }`;`Request.service_workers_mode` 字段 :466,默认 `All`(:540);内部请求变体 :714/:820/:889。
- 现有降级点(间接证据,说明字段在用):`http_loader.rs:639`(redirect follow 时置 `None`)、`:1632`(revalidation 置 `None`)。
- `net/fetch/headers.rs:178`:`Destination::ServiceWorker => "serviceworker"`——request destination 枚举已含 SW 值。

### 主入口链(供拦截点选择)

`net/fetch/methods.rs`:`:179 pub async fn fetch` → `:400 pub async fn main_fetch` → `http_fetch`(http_loader.rs:559)。

### script 层:零分派

`get_matching_scope` 的调用方**只有** `serviceworker_manager.rs:290`(mediator 路径,见 ③)。script/dom fetch 与文档导航路径均无任何 SW 询问——**上游连文档导航的 SW controller 关联也未实现**(controller getter 存在但无人赋值,见 ④)。

---

## ③ respondWith promise 回注路径的上游钩子

上游只搭了**半条信道**(死码),回注契约形状已定:

### 信道定义

`vendor/servo/components/shared/net/lib.rs:118-123`:

```rust
pub struct CustomResponseMediator {
    pub response_chan: IpcSender<Option<CustomResponse>>,
    pub load_url: ServoUrl,
}
```

`CustomResponse`(同文件 :93-114 区段):`{ headers: HeaderMap, raw_status: (StatusCode, String), body: Vec<u8> }`。

### 发送端(应存在于 net 侧,当前不存在)

- `net/resource_thread.rs:695`:`sw_managers: HashMap<ImmutableOrigin, IpcSender<CustomResponseMediator>>`。
- 该 map **只写不读**:全树命中仅 :600-602(`CoreResourceMsg::NetworkMediator` 处理时插入)、:695(字段声明)、:716(init)。**net fetch 路径从不查询此 map,从不构造 `CustomResponseMediator`**——`CustomResponseMediator {` 结构体字面量全树零构造,`CustomResponse` 全树零构造。
- `CoreResourceMsg::NetworkMediator(IpcSender<CustomResponseMediator>, ImmutableOrigin)` 定义于 `shared/net/lib.rs:791`,由 SW 侧注册(见下)。

### 接收端(已实现,活的)

1. 注册:`serviceworker_manager.rs:783-815` `ServiceWorkerManagerFactory::create` → `ipc::channel()` 产生 `(resource_chan, resource_port)` → `core_thread.send(CoreResourceMsg::NetworkMediator(resource_chan, origin))`(:798);`resource_port` 由 ROUTER 转成 crossbeam receiver 供 manager 线程 select。
2. manager 线程:`serviceworker_manager.rs:288-299` `handle_message_from_resource`:enabled + `get_matching_scope(mediator.load_url)` + `registration.active_worker` 三连命中 → `worker.send_message(ServiceWorkerScriptMsg::Response(mediator))`;否则 `mediator.response_chan.send(None)`(pass-through)。
3. worker realm:`serviceworkerglobalscope.rs:597-603` —— fire 裸 fetch 事件 + `send(None)`。

### respondWith 回注该补什么(S2a 侧)

- FetchEvent 持 promise;respondWith 存 promise;事件 dispatch 完成后 settle promise:
  - fulfilled → 在 SW realm 读 Response 的 status/headers/body **字节** → `CustomResponse::new(headers, raw_status, body)` → `mediator.response_chan.send(Some(..))`;
  - rejected / 未调用 respondWith → `send(None)`(上游 pass-through 语义不变)。
- 结构性约束(如实标注):`CustomResponse.body: Vec<u8>` 是一次性字节,不支持流式/不透明流——首版回注必须整读 body;promise 等待复用 `extendableevent.rs`(103 行)已有的 ExtendableEvent waitUntil 型生命周期机制。
- **S2b 结构性难点(未决项)**:`http_fetch` 是 async fn,而 `IpcSender::recv` 是阻塞调用;net 侧 await mediator 响应需把同步 recv 包成异步(参照 http_loader 现有 `DoneChannel` 模式 :568 附近用法)或经 `net/async_runtime.rs`(该文件有 Bao patch)。此深度本轮未实证,**已命中 stop 条件的一部分,如实标注未决**。

---

## ④ controller 匹配链(scope→registration→active worker)上游现状

### 匹配链(在 SW manager 线程,已实现)

`serviceworker_manager.rs`:

| 环节 | 位置 | 内容 |
|---|---|---|
| 注册表 | 字段区 :255 附近 | `registrations: HashMap<ServoUrl, ServiceWorkerRegistration>` |
| scope 匹配 | `:263-270` `get_matching_scope(load_url)` | 遍历 registrations keys → `longest_prefix_match(scope, load_url)` |
| 前缀匹配原语 | `serviceworkerregistration.rs:175` `pub(crate) fn longest_prefix_match(stored_scope, potential_match) -> bool` | (另被 cookiestoremanager.rs:87 复用) |
| active worker 选取 | `:288-296` | `registration.active_worker`(installing/waiting/active 三态字段在 registration 上) |
| 注册写入 | `:516` `handle_register_job` / `:608` `install` / `:661` `handle_update_job` / `:211` `update_registration_state` / `:197` `get_newest_worker` | 完整 register/install/update 状态机 |

### 客户端侧(半残)

- `serviceworkercontainer.rs:42`:`controller: MutNullableDom<ServiceWorker>`;`:317-318` `GetController` 暴露。
- **全树无人写 `.controller`**(SetController 零命中)→ 文档侧 controller 恒 None;文档导航不经过 `get_matching_scope`。即上游"页面 ↔ controller"关联未接线,只有 manager 侧按 scope 前缀匹配 ready。
- constellation 侧仅做进程/线程胶水:`constellation/serviceworker.rs`(55 行,agent-cluster spawn)+ `constellation.rs:394` `sw_managers: HashMap<ImmutableOrigin, GenericSender<ServiceWorkerMsg>>`(消息转发用,:1980 读)。

**结论**:S2 若只做 fetch 拦截,匹配链可直接复用 `handle_message_from_resource` 的既有三连(enabled → get_matching_scope → active_worker);文档导航 SW 化是另一独立缺口,不在本轮 S2a/b 面。

---

## ⑤ S2a/b patch 面建议(文件+断点清单,E26 三断点形态)

### S2a — FetchEvent 类型 + SW 侧事件处理(script/dom 层,servo patch)

| # | 断点 | 动作 |
|---|---|---|
| A1 | 新建 `components/script_bindings/webidls/FetchEvent.webidl` | `[Pref="dom_serviceworker_enabled", Exposed=ServiceWorker] interface FetchEvent : ExtendableEvent`;attrs:`request`(Request)、`preloadResponse`(后置);methods:`respondWith(Promise<Response>)`;`replacesClientId`/`resultingClientId` 可后置 |
| A2 | 新建 `components/script/dom/serviceworker/fetchevent.rs` | FetchEvent DOM 对象;respondWith 存 promise;settle 后 realm 内读 status/headers/body → `CustomResponse::new` → `response_chan.send(Some)`;reject/缺失 → `send(None)` |
| A3 | `serviceworkerglobalscope.rs:597-603` | `Response(mediator)` 分支重写:mediator.load_url → 构造 Request → FetchEvent dispatch(替换裸 `fire_event`)→ 等 respondWith settle;保持 `send(None)` 兜底语义 |
| A4 | `components/script/dom/serviceworker/mod.rs` | 加 `pub(crate) mod fetchevent;`(现文件 21 行,模块清单见该文件) |
| A5 | `ServiceWorkerGlobalScope.webidl` | 取消注释 `attribute EventHandler onfetch;`(oninstall/onactivate 是否同波放开 = 范围决策,留给 S2 合同) |

说明:Request/Response/fetch 在 SW realm 经 `Global=(Worker,ServiceWorker)` 的 Worker 名可达,预期免 patch;实现时若 binding 不可达再补 `Request.webidl`/`Response.webidl`/`Body.webidl` 的 `Exposed` 列表(备选断点)。

### S2b — net 层 mediator 接线(servo patch)

| # | 断点 | 动作 |
|---|---|---|
| B1 | `net/resource_thread.rs:695` `sw_managers` | 补读路径:按 origin 查 `IpcSender<CustomResponseMediator>` 的查询 helper(供 fetch 路径调用);当前只有写入(:600-602) |
| B2 | `net/http_loader.rs:578-580` | TODO 落地:`mode==All` 时查 B1 → 命中则构造 `CustomResponseMediator{response_chan, load_url:request 当前 url}` → send → await recv;`Some(CustomResponse)` → 转 net `Response`(status/headers/body);`None`/未命中 → 落入 Step 4 网络路径 |
| B3 | 防环降级 | SW 子 fetch(`respondWith(fetch(event.request))`)必须 `service_workers_mode=None`,否则无限递归。上游仅 :639(redirect)与 :1632(revalidation)降级;SW realm 发起的 fetch 降级点需在 script 侧 Request 构造处(SW global 判定)——具体落点实现期定 |
| B4 | async recv 包装(未决项) | `http_fetch` async 与 `IpcSender::recv` 阻塞的结构矛盾:参照 http_loader 既有 `DoneChannel` 模式或 `net/async_runtime.rs` 包装;**本轮未实证,列 S2b 首个验证点** |

**层位裁定**:拦截在 **net 层 http_fetch:578**(上游预留的规范位置),不在 script fetch 层——net 层覆盖一切 `mode=All` 请求;script 层会漏文档导航与 worker 子资源。文档导航 SW 化(controller 关联)是独立缺口,不并入 S2。

---

## ⑥ bao stealth 边界约束点(SW 转发 fetch 同一 TLS profile 的接线位)

- **TLS profile SSOT**:`net/connector.rs` 为 Bao vendor patch 模块(:5 注释头),:21 `use bao_stealth::{boringssl_cipher_list_string, ...}`;:55 `pub struct StealthTlsWireConfig`(镜像 `bao_stealth::StealthTlsWireConfig`,注释声明两处必须手工同步);:84 起为全局 stealth TLS/HTTP2 wire 配置,embedder 经 `set_stealth_tls_config()` 注入、`get_stealth_tls_config()` 读取,用于塑形 `SSLConfig`。
- **传导路径**:SW realm 的 `fetch()`(经 `WindowOrWorkerGlobalScope.webidl:56` 的全局 fetch)→ net_traits resource thread(SW 线程使用的 resource_threads 与页面同源:`ServiceWorkerManagerFactory::create`(:783-796)收 `SWManagerSenders.resource_threads` 注入 manager/worker 侧)→ `http_fetch` → connector → **同一全局 StealthTlsWireConfig 塑形的 SSLConfig**。单进程单例 ⇒ SW 转发 fetch 自动继承页面同款 JA3/JA4/cipher/curves/sigalgs/ALPN/H2,**无需新增接线**。
- **约束点(S2 实现必须保住的)**:
  1. 防环降级(B3)保证 SW 转发 fetch 走**真实网络路径**(stealth connector),不会二次进 SW 或绕开 connector;
  2. respondWith 由 SW JS 手工构造 `new Response(body)` 时响应不经网络——这是 SW 语义本身,不属于 TLS 约束面;
  3. S2b 改造 http_fetch:578 时不得引入绕过 connector 的旁路响应构造(回注 Response 直接来自 SW 内存,不触及 TLS——符合 C19 语义:拦截发生在 TLS 之前的应用层)。

---

## 未决项(如实标注)

1. **B4 async recv 包装**:`http_fetch` async 语境下等待 `response_chan` 的具体机制(DoneChannel / async_runtime 包装)未实证,S2b 首个验证点。
2. **文档导航 SW 化**:上游 controller 关联零接线(无人写 `container.controller`),属于 fetch 拦截之外的独立缺口,未纳入本轮 patch 面。
3. Request/Response 在 SW realm 的 binding 可达性按 WebIDL Global 名推得(Global=(Worker,ServiceWorker) 含 Worker 名),未跑 binding 生成实证;实现期首个 sanity check。

## stop 条件判定

**命中**:net trait 层分派深度中 "async 语境如何等待阻塞 ipc recv"(B4)未能定位确定机制,已按合同把已证事实+断点清单写入本报告并如实标注;其余五项均已定位到文件:行号级证据。
