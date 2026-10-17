# bao 归并 + 性能只读扫描报告(2026-10-17)

只读扫描:search_code(duplicate/code_smell/anti_pattern 全层)+ 手工核对。每条带 file:line 证据。
扫描范围:src/bao_runtime(225,986 行)/ bao_browser(97,319)/ bao_cdp(34,151)/ bao_cdp_client(39,385)/ bao_stealth(26,041)/ bao_engine(15,575)/ cdp-server(20,223)/ bao_cli(4,092)/ http(24,870)。

**先声明:检测器命中大量「已登记豁免」项**(REQ-ARCH-1 duplicate convergence 语义分述),以下候选表已全部人工核Verified并剔除豁免项,只保留真归并/真优化面。已核验排除的假阳性:`behavior.rs firefox/chrome`(画像 schema 豁免)、`web_streams.js cancel1/cancel2`(WHATWG spec 镜像豁免)、`from_cdp ×2`(双公共类型边界豁免)、`rsa_pem_op 四包装`(已收敛 thin delegation)、`push_masked_len_*`(测试函数名近似)、`sha256/512 test 对`(测试向量)。

---

## A. 归并候选表(按影响降序 top-15)

| # | 位置 | 重复形态 | 归并方向 | 风险 |
|---|------|---------|---------|------|
| A1 | `bao_runtime/src/node_http.rs:1969`、`node_http2.rs:1630,2597,2623,2865`、`fetch_api.rs:1954`、`node_url.rs:1870`、`node_cluster.rs:906`、`bun_inspect_api.rs:904`、`bun_ffi.rs:1899`、`node_vm.rs:1028,1137,1212`、`bun_api.rs:5973,6515`、`node_wasi.rs:868`、`bun_spawn_sync.rs:197`、`bun_serde_api.rs:402`、`node_util.rs:1125,1253` | **「属性遍历 → jsid.to_string() → jsstr_to_string → ZBox::from_bytes → JS_GetProperty」样板 ×20 处**,每处 ~20 行,合计 ~400 行同构代码;字面差异只在循环体后半段 | 抽 `for_each_string_property(cx, obj, \|key: &str, val: Handle<Value>\| {...})` 共享 helper(或 macro_rules 家族,memory duplicate-detector-normalization-macro-form 先例);同时是 B1 性能项的载体 | 低——纯机械变换,循环体逐点保持;20 个调用点需逐一回验(全量 suite 波末) |
| A2 | `bao_runtime/src/fetch_api.rs:687`(percent_decode,WHATWG 语义:非法 % 字面透传)、`bao_runtime/src/bun_api.rs:10601`(percent_decode_path,同语义)、`cdp-server/src/transport.rs:164`(percent_decode,**语义分叉**:form-style `+→空格`、截断 escape `unwrap_or(b'0')` 补零解码、`push(val as char)` Latin-1 字节→char 多字节 UTF-8 会 mangle) | **手写 percent-decode ×3,三种语义互不一致**;workspace `bun_url::PercentEncoding::decode` 已存在(`url/lib.rs:1889`,WHATWG 全语义,`url/lib.rs:1579` 等 8 处内部消费) | 收敛到 bun_url::PercentEncoding(或从它 re-export 一个共用 percent_decode);cdp-server 版本收敛时顺带修正 Latin-1/截断语义分叉 | 中——cdp-server 版语义分叉可能是刻意的 form 解码(需裁:若为 /json?url= 查询串解码,`+→空格` 对 URL path 部分是错的);改前先跑 cdp-server suite |
| A3 | `bao_runtime/src/node_fs.rs:630`(io_error_code,超集:raw_os_error+msg-contains+kind)、`bao_runtime/src/bun_api.rs:7704`(bunfile_io_code,子集:仅 kind,**注释自认「same taxonomy as node_fs::throw_fs_error」但已漂移缺 EEXIST**) | **errno→Node code 映射表 ×2,已实际漂移**(bunfile 版缺 `AlreadyExists => "EEXIST"`,Bun.file 覆盖已存在文件的报错 code 与 fs 面不一致) | node_fs::io_error_code 提 pub(crate),bun_api 删本地表改调 | 低——单点收敛消除已发生的漂移;fs suite 回归 |
| A4 | `bao_runtime/src/bun_api.rs:7878`(pub(crate))+ `bao_runtime/src/node_fs.rs:1221`(private) | **js_string_from_utf8 字节级相同 ×2**(同注释、同 Utf8Chars+JS_NewStringCopyUTF8N 本体,连 mojibake 教训注释都同源) | node_fs 删私有副本改用 bun_api 的 pub(crate) 版(或上移到共享 helper 模块) | 零——字面相同 |
| A5 | `bao_runtime/src/globals.rs:4488`(throw_error_with_code 超集:range+code)、`node_url.rs:1937`(throw_type_error_with_code)、`node_events.rs:574`(throw_type_error)、`bun_password.rs:285`(throw_error)、`node_crypto.rs:737`(throw_type_error) | **JS 抛错 helper ×5 散布**,各自 new Exception + SetPendingException 形态 | 收敛一个共享 throw 模块(globals 版作核,其余薄包装);顺带统一 ERR code 注入点 | 低——各文件改 import + 传参;异常语义逐点保持 |
| A6 | `bao_runtime/src/bun_listen.rs:3255,3265`(udp_on_drain/udp_on_close)+ `bao_runtime/src/bun_udp.rs:1657,1667`(同名对,bun_udp 版 on_close 多 `closed.set(true)`) | **UDP extern "C" 回调对 ×2 近同构**(null 守卫+user() 解引用+invoke_js_callback 完全同形) | 抽共享 `fire_udp_cb(socket, key)` helper,bun_udp 版在其上叠 closed 标记 | 低——回调形状逐点保持 |
| A7 | `bao_cdp_client/src/bridge/a_class_handlers.rs:931` 与 `bao_cdp_client/src/bridge/event_translator.rs:1574` | **base64_encode 薄包装 ×2**(均已走 bun_base64::encode_alloc,但错误面不一致:前者 `from_utf8().expect`,后者 `from_utf8_unchecked`) | 收敛单点(如 bridge 内 pub(crate) util);统一 ASCII 安全论证(两版注释都承认「不可能失败」,统一走 unchecked+SAFETY 注释或统一 expect) | 零语义风险;纯一致性 |
| A8 | `bao_runtime/src/bun_api.rs:3569`(BunServeUserData::fetch_handler)+ `bun_listen.rs:204`(ListenHttpUserData::fetch_handler) | **gc_store 回调解析方法 ×2 同构**(fetch_cb_key→gc_store_get,null 守卫同形) | 抽自由函数 `resolve_gc_fn(cx, key)`;两 struct 复用 | 低 |
| A9 | `bao_runtime/src/node_url.rs:92-230` parse_url 的 data:/blob:/opaque-scheme 手写分段(cyclomatic=45) | 主 parse 已走 `bun_url`(node_url.rs:17 `use bun_url`),但 data:/blob:/非特殊 scheme 手写了组件切分 | 候选上游化:该特殊 scheme 处理下沉进 bun_url crate(全仓单点);非紧迫——现形态是「bun_url 未覆盖面的权宜」 | 中——挪 crate 需重跑 node:url 全测试向量;收益是单点维护非运行时 |
| A10 | `bao_runtime/src/bun_api.rs:10618 hex_digit`、`fetch_api.rs:693 hex_val`、`cdp-server/src/transport.rs:182 hex_val` | **手写 hex 半字节解码 ×3**(内嵌于 A2 的三个 percent-decoder) | A2 收敛 percent-decode 后自动消解;独立面若有 hex 需求应走 `bun_core::decode_hex_to_bytes`(`bun_core/string/immutable.rs:1805`,highway SIMD) | 低(随 A2) |

**成熟库包概念核查结论(任务 A.3)**:
- ✅ base64:无手写本体——`bao_cdp_client` 两处均已 `bun_base64::encode_alloc`(仅薄包装重复,A7)。
- ✅ timer heap:未手写——`timers.rs:1649` 用 `bun_io::heap::Intrusive`(Bun 自带侵入式堆,注释锚定 Bun runtime/timer/mod.rs:314 比较器)。
- ✅ DNS:`node_dns.rs` 全走 `bun_dns`(含 shared cache,`node_dns.rs:1100`)。
- ✅ glob:`bao_browser/src/delegate/webview_state.rs:1733 glob_matches` 看似手写 glob,实为 **Chromium `base::MatchPattern` 语义的刻意重实现**(X.8 裁决、oracle 实测形态、`\` 转义/`?` 单字符语义与 shell glob 不同)——bun `glob` crate 仅 `detect_glob_syntax` 且语义不符,**非归并候选,属 Chromium parity 资产**。
- ⚠️ percent-decode/hex:唯一真缺口(上表 A2/A10)。

---

## B. 性能候选表(静态可证,按影响降序 top-15)

| # | 位置 | 形态 | 证据 | 预期收益 |
|---|------|------|------|---------|
| B1 | `bao_runtime/src/node_http.rs:1969`(uws_route_handler 请求头遍历)、`node_http2.rs:1630,2597,2623,2865`(h2 请求/响应头)、`fetch_api.rs:1954`(Headers 构造) | **每请求 × 每头 3 次分配的 name 往返**:`jsid.to_string()`(Atom→JSString)+ `jsstr_to_string`(JSString→Rust String 拷贝)+ `ZBox::from_bytes`(再拷一次到 NUL 终止缓冲)——而 jsid 本身已持该属性名的 string,可直接 `JS_GetPropertyById` | 树内先例:`node_vm.rs:1045`、`globals.rs:5722`、`bun_inspect_api.rs:997` 已用 `JS_GetPropertyById` 按 id 直取(A1 helper 改造时一并切换) | HTTP/h2 每请求省 (3×头数) 次堆分配+拷贝;典型 10-30 头请求省 30-90 allocs/req |
| B2 | `bao_runtime/src/node_tls.rs:1713` | `String::from_utf8_lossy(proto).to_string()` **双拷贝**:lossy 产出 Cow(Owned 时已是一次 String 分配),`.to_string()` 再 clone 一次 | ALPN 协商结果每 TLS 握手执行一次(TLSLayer secureConnection 缝) | 每握手省 1 次全串 clone;`String::from_utf8_lossy(proto).into_owned()` 或 Cow 直接用即可 |
| B3 | `bao_runtime/src/web_api.rs:338-345`(WsConn::Tls send_text) | **每 WS 消息一次额外 memcpy**:`payload.to_vec()` 整帧载荷克隆后再 mask、再 extend 进 frame(载荷被拷 2 次) | `frame.extend_from_slice(&key)` 后可直接 `frame.extend_from_slice(payload)` 再对 frame 内该区间原地 `apply_mask`——载荷零独立克隆 | 每 WS 消息省 1 次全长 memcpy(大消息/高频 ws 面可观测) |
| B4 | `bao_browser/src/runtime_bridge.rs:3137,3159`(BridgeChannel::send → recv_timeout) | **每命令一次跨线程同步往返**(page→ScriptThread 求值/截图均走此),bounded 30s fail-closed | `DEFAULT_RESPONSE_TIMEOUT=30s`(:3114 注释载明设计);JSContext thread-local(BCE-20260621-001)使跨线程往返结构上不可避免 | **结构必然而非缺陷**——列为观察项:延迟=一次跨线程 wakeup;不可归并/不可消除,批量场景(如 CDP Runtime.evaluate 风暴)已由 pump 侧消化 |
| B5 | `bao_runtime/src/node_fs.rs` 全文件(15 处 Mutex) | Mutex 审计:**全部合规**——均在 background IO worker(:76,683 result slot)与 fs watcher 后台线程(:2666-3245),属 CLAUDE.md「跨线程必要处」豁免;JS 面单线程语义用 thread_local+RefCell(timers.rs:1642 注释明示) | 逐处核对无 JS 热路径持锁 | 无(审计通过面,零行动) |
| B6 | `bao_runtime/src/fetch_async.rs:302,531` | StreamShared/outcome 的 `Arc<Mutex<>>`:JS 线程↔HTTP 线程真共享载体,`:1475` SAFETY 注释明示「shared Arc<Mutex<>>, safe to lock from any thread」;热读路径 `phase`/`parked` 已走原子(:228 注释) | 锁内无 IO;快路径原子化已完成 | 无(已是最优形态) |
| B7 | `bao_stealth` 全 crate | 每 profile 的 wire config 构建(`stealth_wire.rs:85 from_profile`)只在**每页 install 时执行一次**(`runtime_bridge.rs:1754`,install_all_native 缝),非每请求;TLS/h2 配置经 keyed 注册表按 WebViewId 缓存(vendor connector 面) | 无每请求重算热点 | 无(缓存已就位) |
| B8 | `bao_runtime/src/node_http.rs:1969` 同域 — 附注 | 头值路径 `crate::js_to_rust_string(cx, hv)` 每头一次 JSString→String(必要转换,值非 key 可复用 jsid),`node_http2.rs:1651-1653` key 做 `to_ascii_lowercase()` 再 ZBox——key 若已 ASCII 小写可跳过 | A1/B1 helper 改造时顺带:`key.is_ascii_lowercase()` 快路径 | 每头省 1-2 次拷贝(常见全小写头名场景) |

**时序概念核查结论(任务 B.3)**:
- 同步 DNS:`node_dns.rs:1083` resolve_hostname_libc 同步,但消费面为 `dns.lookup` sync 语义(Node 契约本身同步)+ shared cache 短路(:1100);async lookup 走 c-ares RR 家族——无 JS 线程串行 DNS 阻塞缺陷。
- 同步文件 IO:`node_fs.rs` sync API 是 Node 契约同步语义;async 面走 spawn_fs_async 后台线程(:683)。
- 同步 channel 往返:仅 B4 一处(结构必然);`cdp-server/src/server.rs:822 ack_rx.recv()` 为 CDP 会话 ack 初始化路径(一次性,非每请求)。
- 未发现 async 上下文内的同步阻塞 IO 违例(bao runtime 的 async 面由 bun uWS/HTTPThread 事件泵承载,本就非 tokio 模型)。

---

## 汇总

- **真归并候选 10 项**(A1-A10),其中 A1 是唯一 ×20 大簇(且与 B1 同工);A2/A3 带语义漂移修复属性(percent 语义分叉、errno 表已缺 EEXIST)——归并即根治。
- **真性能候选 4 项**(B1-B4),B1 收益最大(每请求热路径),B2/B3 小而确定,B4 为结构观察项。
- **合规面**:锁纪律(JS 面 thread_local+RefCell)全过;成熟库复用(base64/timer heap/DNS/glob)全过,glob 是 Chromium parity 刻意手写非缺口。
- 检测器 duplicate/code_smell 命中大多为已登记 REQ-ARCH-1 豁免或 essential 复杂度豁免——项目豁免纪律健康,未发现应豁未豁的错标(除 A3 表注释自认 same taxonomy 却已漂移,属归并时机信号)。
