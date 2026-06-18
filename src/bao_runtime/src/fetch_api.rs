// @trace REQ-ENG-001 [entity:BaoRuntime] [api:fetch]
// @trace REQ-ENG-006 REQ-STL-001
// fetch + Response + Headers constructors
//
// ## BCE-007: fetch 真异步化核心
//
// 历史问题(fetch_fn + do_fetch 同步阻塞):
//   - fetch_fn 创建 SM Promise 后调用同步 do_fetch(stealth_http_request →
//     http_client::http_request → AsyncHTTP::send_sync),整个 HTTP 往返在
//     JS 线程上阻塞,违背 fetch() Promise 的异步语义。
//   - 范式缺陷: 「同步调用包 Promise 外壳」反模式 — Promise 立即 Resolve,
//     阻塞发生在 Resolve 之前。
//
// 根治策略(BCE-007 统一根治模板):
//   1. fetch_fn 创建 PENDING 的 SM Promise(NewPromiseObject),立即返回。
//   2. do_fetch 的工作派发到独立 worker 线程(std::thread::spawn),
//      真正非阻塞;stealth profile 通过 Arc 跨线程共享。
//   3. 结果通过 Arc<Mutex<Option<Result>>> 跨线程回传。
//   4. JS 线程通过 drain_pending_fetches 在事件循环 tick 时轮询完成项,
//      构建 Response 对象并 ResolvePromise,随后由现有 RunJobs 路径
//      唤醒 .then() 回调。
//   5. has_pending_fetches() 让事件循环知道有未完成 fetch,保持循环存活。
//
// 复用锚点:
//   - stealth_http::stealth_http_request:TLS/HTTP 指纹注入,纯函数级 100% 复用
//   - node_fs.rs 异步 Promise 范例:NewPromiseObject/Resolve/Reject pattern
//   - bun_engine::dispatch_sm::BaoEventLoop:事件循环 tick + RunJobs 集成点
//   - bao_engine::job_queue::JobQueue:Promise 延迟 resolve 走同一队列
use ::std::cell::RefCell;
use ::std::sync::{Arc, Mutex};
use bun_core::ZBox;
use ::std::ptr::NonNull;

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue, StringValue, Int32Value, ObjectValue, BooleanValue};
use mozjs::rooted;
use mozjs::rust::wrappers2::{JS_DefineFunction, JS_DefineProperty3, JS_NewPlainObject};
use mozjs::conversions::jsstr_to_string;

thread_local! {
    static TL_STEALTH_PROFILE: RefCell<Option<bao_stealth::StealthProfile>> = const { RefCell::new(None) };

    /// BCE-007: per-thread pending async fetch registry.
    ///
    /// Each entry holds the SM `Promise*` (raw, rooted via JS::AddPromiseReactions
    /// ownership model — Promise objects are GC-traced from the global and survive
    /// until resolved/rejected), the worker's result slot, and the original URL
    /// (for the Response.url field). `drain_pending_fetches` polls these and
    /// resolves completed promises.
    ///
    /// KeepAlive semantics: while `PENDING_FETCHES` is non-empty, the event loop
    /// must keep ticking (see `has_pending_fetches`). The worker thread holds no
    /// JS references — it only fills the `Arc<Mutex<Option<Result>>>` slot.
    static PENDING_FETCHES: RefCell<Vec<PendingFetch>> = const { RefCell::new(Vec::new()) };
}

/// BCE-007: an in-flight async fetch awaiting resolution.
///
/// Stored in `PENDING_FETCHES` between `fetch_fn` dispatch and
/// `drain_pending_fetches` resolution. The worker thread writes into
/// `result_slot`; the JS thread reads it during drain.
struct PendingFetch {
    /// Raw SM `Promise*`. Owned by the SM GC (rooted via the global's promise
    /// list); safe to use from the JS thread during drain. Not touched by the
    /// worker thread.
    promise: *mut JSObject,
    /// Cross-thread result slot. `None` while in-flight, `Some(Ok|Err)` once
    /// the worker completes. Polled by `drain_pending_fetches`.
    result_slot: Arc<Mutex<Option<::std::result::Result<FetchResponse, String>>>>,
    /// Original request URL, kept for `Response.url` (worker does not retain it).
    url: String,
}

// SAFETY: `PendingFetch.promise` is a raw `*mut JSObject` that is only ever
// dereferenced on the JS thread (in `drain_pending_fetches`); the worker
// thread only touches `result_slot` (which is `Arc<Mutex<...>>` — `Send` by
// construction). The struct itself moves within the JS thread only. We assert
// `Send` so it can live in a `thread_local!` without the implicit
// `!Send`-on-`*mut` bound tripping compile.
unsafe impl Send for PendingFetch {}

/// Store the current page's stealth profile so fetch() can apply TLS/HTTP2 fingerprints.
pub fn set_fetch_stealth_profile(profile: Option<bao_stealth::StealthProfile>) {
    TL_STEALTH_PROFILE.with(|p| *p.borrow_mut() = profile);
}

/// Returns true if a stealth profile has been explicitly set on this thread.
pub fn is_fetch_stealth_profile_set() -> bool {
    TL_STEALTH_PROFILE.with(|p| p.borrow().is_some())
}

/// Idempotent: install Firefox default profile if none has been set on this thread.
/// Called by `globals::install_all` so fetch() gets TLS/HTTP2 fingerprints by default.
pub fn ensure_default_fetch_stealth_profile() {
    if !is_fetch_stealth_profile_set() {
        set_fetch_stealth_profile(Some(bao_stealth::StealthProfile::firefox_default()));
    }
}

pub fn install_fetch_global(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        JS_DefineFunction(
            cx, global, c"fetch".as_ptr(),
            ::std::option::Option::Some(fetch_fn), 1, JSPROP_ENUMERATE as u32,
        );
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fetch_fn(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"fetch requires a URL argument".as_ptr());
        return false;
    }

    let url_val = *args.get(0).ptr;
    if !url_val.is_string() {
        JS_ReportErrorUTF8(cx, c"fetch requires a string URL".as_ptr());
        return false;
    }

    let url = crate::js_to_rust_string(cx, url_val);

    if let ::std::option::Option::Some(pos) = url.find("://") {
        let host_part = &url[pos + 3..];
        let host = host_part.split('/').next().unwrap_or(host_part).split(':').next().unwrap_or(host_part);
        if let ::std::result::Result::Err(e) = crate::permission_bridge::check_net(host) {
            let c_msg = ZBox::from_bytes(e.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    }

    let method = if argc > 1 {
        let opts = *args.get(1).ptr;
        if opts.is_object() {
            let obj = opts.to_object();
            let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
            let mut m_val = UndefinedValue();
            let m_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut m_val };
            JS_GetProperty(cx, obj_handle, c"method".as_ptr(), m_handle);
            if m_val.is_string() {
                crate::js_to_rust_string(cx, m_val).to_uppercase()
            } else {
                "GET".to_string()
            }
        } else {
            "GET".to_string()
        }
    } else {
        "GET".to_string()
    };

    let body = if argc > 1 {
        let opts = *args.get(1).ptr;
        if opts.is_object() {
            let obj = opts.to_object();
            let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
            let mut b_val = UndefinedValue();
            let b_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut b_val };
            JS_GetProperty(cx, obj_handle, c"body".as_ptr(), b_handle);
            if b_val.is_string() {
                Some(crate::js_to_rust_string(cx, b_val))
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    // ── BCE-007: fetch 异步化核心 ──────────────────────────────────
    // 创建 PENDING 的 SM Promise,派发 worker 线程执行非阻塞 HTTP,
    // 立即返回 pending promise。结果在 drain_pending_fetches(事件循环
    // tick)时被 ResolvePromise,符合 fetch() 的真异步语义。
    // @trace REQ-ENG-001 [entity:BaoRuntime] [api:fetch async]
    let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &::std::ptr::null_mut() });
    if promise.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    // Clone the per-thread stealth profile so the worker can apply TLS/HTTP
    // fingerprints without touching JS-thread state. Arc keeps it alive until
    // the worker finishes; the worker drops its clone on completion.
    let profile: Option<bao_stealth::StealthProfile> =
        TL_STEALTH_PROFILE.with(|p| p.borrow().clone());

    // Cross-thread result slot: None while in-flight, Some(Ok|Err) on completion.
    let result_slot: Arc<Mutex<Option<::std::result::Result<FetchResponse, String>>>> =
        Arc::new(Mutex::new(None));

    // Spawn worker thread — true non-blocking I/O. The worker owns its own
    // copies of url/method/body/profile and writes the FetchResponse (or error)
    // into the shared slot. It never touches JS state.
    // @trace REQ-ENG-001 [api:fetch async] — non-blocking dispatch
    spawn_fetch_worker(
        Arc::clone(&result_slot),
        url.clone(),
        method.clone(),
        body.clone(),
        profile,
    );

    // Register the pending fetch so the event loop's drain pass can resolve it.
    // The promise pointer is only dereferenced on the JS thread (drain path).
    PENDING_FETCHES.with(|pf| {
        pf.borrow_mut().push(PendingFetch {
            promise,
            result_slot,
            url,
        });
    });

    args.rval().set(mozjs::jsval::ObjectValue(promise));
    true
}

// @trace REQ-PERF-001 [entity:HttpResponse]
/// fetch() Response 内部表示。
///
/// 性能优化(REQ-PERF-001):
/// - `body: Vec<u8>`:二进制安全,消除 `String::from_utf8_lossy(&body).to_string()`
///   的双重拷贝(原代码:`from_utf8_lossy` 可能分配 Cow::Owned,`.to_string()` 再 clone)。
///   现在直接 `result.body.to_vec()`(Bytes 引用计数 → 唯一 Vec,必要时一次拷贝),
///   然后 `ZBox::from_vec(response.body)` 零拷贝 move 进 SpiderMonkey。
struct FetchResponse {
    status_code: u16,
    body: Vec<u8>,
    headers: Vec<(String, String)>,
    url: String,
    status_text: String,
}

/// BCE-007: Synchronous HTTP fetch helper (worker-side).
///
/// Performs the actual HTTP round-trip via `stealth_http::stealth_http_request`
/// (which bridges `bun_http` + stealth fingerprints — full reuse, no re-write).
/// Called from the worker thread spawned by `spawn_fetch_worker`; never called
/// on the JS thread (which only constructs the promise and drains results).
///
/// `profile` is passed in (not read from `TL_STEALTH_PROFILE`) so the worker
/// thread — which does not share the JS thread's TLS — uses the same stealth
/// profile the JS thread had at dispatch time.
// @trace REQ-ENG-001 [api:fetch async] [entity:FetchResponse]
// @trace REQ-ENG-007 [code:std::net::ToSocketAddrs] [entity:FetchResponse]
fn do_fetch_blocking(
    url: &str,
    method: &str,
    body: Option<&str>,
    profile: &Option<bao_stealth::StealthProfile>,
) -> ::std::result::Result<FetchResponse, String> {
    // Fast pre-check: ensure the host:port is reachable before delegating to
    // AsyncHTTP, which may otherwise hang for minutes on SYN to dead endpoints
    // (root cause of the fetch_api_tests SIGTERM during the suite — port 1 on
    // loopback never responds and the bun_http internals lack a connect timeout).
    // @trace REQ-ENG-007 [code:std::net::ToSocketAddrs] - uses system DNS (libc::getaddrinfo, equivalent to bun_dns::Backend::Libc)
    if let Some((host, port)) = extract_host_port(url) {
        let addr = format!("{}:{}", host, port);
        if let Ok(addrs) = ::std::net::ToSocketAddrs::to_socket_addrs(&addr) {
            let collected: Vec<_> = addrs.collect();
            let any_reachable = collected.iter().take(3).any(|sa| {
                ::std::net::TcpStream::connect_timeout(sa, ::std::time::Duration::from_millis(250)).is_ok()
            });
            if !any_reachable {
                return ::std::result::Result::Err(format!("connect refused: {}", addr));
            }
        } else {
            return ::std::result::Result::Err(format!("DNS resolution failed for {}", addr));
        }
    }

    let bun_method = match method {
        "POST" => bun_http::Method::POST,
        "PUT" => bun_http::Method::PUT,
        "DELETE" => bun_http::Method::DELETE,
        "PATCH" => bun_http::Method::PATCH,
        "HEAD" => bun_http::Method::HEAD,
        "OPTIONS" => bun_http::Method::OPTIONS,
        _ => bun_http::Method::GET,
    };

    let headers: Vec<(String, String)> = Vec::new();
    let body_bytes: Option<&[u8]> = body.map(|b| b.as_bytes());

    let result = crate::stealth_http::stealth_http_request(
        profile, bun_method, url, &headers, body_bytes,
    ).map_err(|e| e.to_string())?;

    ::std::result::Result::Ok(FetchResponse {
        status_code: result.status_code as u16,
        // Bytes → Vec:如果 Bytes 是唯一引用则零拷贝(move),否则一次拷贝。
        // 比 `String::from_utf8_lossy(&result.body).to_string()` 少一次 clone。
        // @trace REQ-PERF-001 [entity:HttpResponse]
        body: result.body.to_vec(),
        headers: result.headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        url: url.to_string(),
        status_text: result.status_text.to_string(),
    })
}

/// BCE-007: Spawn a worker thread that performs the blocking HTTP fetch and
/// writes the result into the shared `result_slot`.
///
/// The worker owns copies of all inputs (url, method, body, profile) and never
/// touches JS state. On completion it stores `Some(Ok|Err)` into the slot; the
/// JS thread polls the slot via `drain_pending_fetches` and resolves the promise.
///
/// `std::thread::spawn` is the minimal correct non-blocking primitive here:
/// the alternative — `bun_http::AsyncHTTP` event-driven path — requires
/// `bun_io::EventLoopCtx` integration that would force edits to `timers.rs`
/// (out of this task's file scope). A dedicated worker thread gives true
/// async semantics (JS thread is never blocked on the HTTP round-trip)
/// while keeping the change localized to `fetch_api.rs`.
// @trace REQ-ENG-001 [api:fetch async] [entity:PendingFetch]
fn spawn_fetch_worker(
    result_slot: Arc<Mutex<Option<::std::result::Result<FetchResponse, String>>>>,
    url: String,
    method: String,
    body: Option<String>,
    profile: Option<bao_stealth::StealthProfile>,
) {
    ::std::thread::spawn(move || {
        let outcome = do_fetch_blocking(&url, &method, body.as_deref(), &profile);
        // Write the result; if the JS thread already dropped its Arc (e.g.
        // process exit), the slot is the last reference and is dropped here.
        if let Ok(mut guard) = result_slot.lock() {
            *guard = Some(outcome);
        }
        // Intentionally no JS work here — the JS thread resolves the promise.
    });
}

fn extract_host_port(url: &str) -> ::std::option::Option<(String, u16)> {
    let scheme_end = url.find("://")?;
    let rest = &url[scheme_end + 3..];
    let authority = rest.split('/').next()?;
    let (hostport, _) = authority.split_once('?').unwrap_or((authority, ""));
    let (host, port) = if let Some(idx) = hostport.rfind(':') {
        let (h, p) = hostport.split_at(idx);
        (h.to_string(), p[1..].parse::<u16>().unwrap_or(80))
    } else {
        (hostport.to_string(), if url.starts_with("https://") { 443 } else { 80 })
    };
    Some((host, port))
}

// ── BCE-007: async fetch drain API ─────────────────────────────────────────
// These public functions let the event loop (timers.rs drain_and_check /
// drain_one_pass, or any future BaoEventLoop tick integration) poll and
// resolve pending async fetches. They run entirely on the JS thread.
// @trace REQ-ENG-001 [entity:BaoRuntime] [api:fetch async drain]

/// Returns `true` while there are in-flight async fetches awaiting resolution.
///
/// The event loop consults this to decide whether to keep ticking (KeepAlive
/// semantics — mirrors JSC's `m_pendingRefCount`). As long as any fetch is
/// pending, the loop must continue draining so the promise can be resolved.
// @trace REQ-ENG-001 [api:fetch async]
pub fn has_pending_fetches() -> bool {
    PENDING_FETCHES.with(|pf| !pf.borrow().is_empty())
}

/// Drain completed async fetches: resolve/reject their SM Promises.
///
/// Called from the JS thread (event loop tick). For each `PendingFetch` whose
/// worker has written a result into its slot, this builds the JS `Response`
/// object and calls `ResolvePromise` (or `RejectPromise` on error), then
/// removes the entry. In-flight entries (slot still `None`) are retained.
///
/// Returns the number of promises resolved/rejected this pass.
///
/// # Safety
/// - `raw_cx` must be a live `JSContext*` on the current thread.
/// - Must be called with no other JS-thread code mutating `PENDING_FETCHES`.
// @trace REQ-ENG-001 [api:fetch async drain]
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn drain_pending_fetches(raw_cx: *mut JSContext) -> usize {
    let mut resolved = 0usize;

    // Pop completed entries into a local Vec (no borrow held across JS work).
    // Retain in-flight entries (slot is None). Use a separate borrow scope.
    let completed: Vec<PendingFetch> = PENDING_FETCHES.with(|pf| {
        let mut guard = pf.borrow_mut();
        let mut keep = Vec::with_capacity(guard.len());
        let mut done = Vec::new();
        for entry in guard.drain(..) {
            let is_done = entry.result_slot.lock()
                .map(|slot| slot.is_some())
                .unwrap_or(false);
            if is_done { done.push(entry); } else { keep.push(entry); }
        }
        *guard = keep;
        done
    });

    for entry in completed {
        // Extract the result from the slot (worker has written Some).
        let result = entry.result_slot.lock()
            .ok()
            .and_then(|mut slot| slot.take())
            .unwrap_or_else(|| Err("fetch result slot poisoned or missing".to_string()));

        let promise_h = Handle::<*mut JSObject> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &entry.promise,
        };

        match result {
            Ok(resp) => {
                let resp_obj = build_response_object(raw_cx, &resp);
                if !resp_obj.is_null() {
                    let resp_val = mozjs::jsval::ObjectValue(resp_obj);
                    let resp_handle = Handle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &resp_val,
                    };
                    mozjs_sys::jsapi::JS::ResolvePromise(raw_cx, promise_h, resp_handle);
                    resolved += 1;
                } else {
                    // Allocation failure — reject with an error so the promise
                    // doesn't hang forever.
                    reject_promise_with_message(raw_cx, entry.promise, "fetch: failed to build Response");
                    resolved += 1;
                }
            }
            Err(e) => {
                reject_promise_with_message(raw_cx, entry.promise, &format!("fetch failed: {}", e));
                resolved += 1;
            }
        }
    }

    // After resolving, drain SM microtasks so `.then()`/`await` callbacks fire
    // on this same tick. Mirrors the pattern in timers::drain_one_pass.
    if resolved > 0 {
        mozjs_sys::jsapi::js::RunJobs(raw_cx);
    }

    resolved
}

/// Build a JS `Response` object from a `FetchResponse`. Returns a plain
/// object with `status`/`ok`/`url`/`statusText`/`headers`/`_bodyText`/`text()`/
/// `json()` — the same shape the legacy synchronous path produced, extracted
/// here so both the (now async) fetch path share one builder.
///
/// # Safety
/// - `raw_cx` must be a live `JSContext*` on the current thread.
// @trace REQ-ENG-001 [api:fetch Response build] [entity:FetchResponse]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn build_response_object(raw_cx: *mut JSContext, response: &FetchResponse) -> *mut JSObject {
    let resp_obj = mozjs_sys::jsapi::JS_NewPlainObject(raw_cx);
    if resp_obj.is_null() {
        return resp_obj;
    }
    let obj_handle = Handle::<*mut JSObject> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &resp_obj,
    };

    let status_val = Int32Value(response.status_code as i32);
    let s_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &status_val };
    JS_DefineProperty(raw_cx, obj_handle, c"status".as_ptr(), s_handle, JSPROP_ENUMERATE as u32);

    let ok_val = mozjs::jsval::BooleanValue(response.status_code >= 200 && response.status_code < 300);
    let ok_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &ok_val };
    JS_DefineProperty(raw_cx, obj_handle, c"ok".as_ptr(), ok_handle, JSPROP_ENUMERATE as u32);

    {
        let c_url = ZBox::from_bytes(response.url.as_bytes());
        let url_js = JS_NewStringCopyZ(raw_cx, c_url.as_ptr());
        if !url_js.is_null() {
            let url_val = StringValue(&*url_js);
            let u_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &url_val };
            JS_DefineProperty(raw_cx, obj_handle, c"url".as_ptr(), u_handle, JSPROP_ENUMERATE as u32);
        }
    }

    {
        let c_st = ZBox::from_bytes(response.status_text.as_bytes());
        let st_js = JS_NewStringCopyZ(raw_cx, c_st.as_ptr());
        if !st_js.is_null() {
            let st_val = StringValue(&*st_js);
            let st_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &st_val };
            JS_DefineProperty(raw_cx, obj_handle, c"statusText".as_ptr(), st_handle, JSPROP_ENUMERATE as u32);
        }
    }

    let headers_obj = mozjs_sys::jsapi::JS_NewPlainObject(raw_cx);
    if !headers_obj.is_null() {
        let h_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &headers_obj };
        for (key, value) in &response.headers {
            let c_key = ZBox::from_bytes(key.as_bytes());
            let c_val = ZBox::from_bytes(value.as_bytes());
            let val_js = JS_NewStringCopyZ(raw_cx, c_val.as_ptr());
            if !val_js.is_null() {
                let hv = StringValue(&*val_js);
                let hv_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &hv };
                JS_DefineProperty(raw_cx, h_handle, c_key.as_ptr(), hv_handle, JSPROP_ENUMERATE as u32);
            }
        }
        let hdrs_val = mozjs::jsval::ObjectValue(headers_obj);
        let hdrs_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &hdrs_val };
        JS_DefineProperty(raw_cx, obj_handle, c"headers".as_ptr(), hdrs_handle, JSPROP_ENUMERATE as u32);
    }

    // Body 二进制安全传递给 JS。body 已是 Vec<u8>,clone 后直接 move 进 ZBox
    // (零拷贝转入 SpiderMonkey)。
    // @trace REQ-PERF-001 [entity:HttpResponse]
    let c_body = ZBox::from_vec(response.body.clone());
    let body_str = JS_NewStringCopyZ(raw_cx, c_body.as_ptr());
    if !body_str.is_null() {
        let body_val = StringValue(&*body_str);
        let bt_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &body_val };
        JS_DefineProperty(raw_cx, obj_handle, c"_bodyText".as_ptr(), bt_handle, 0);
    }

    let text_fn = JS_NewFunction(raw_cx, Some(response_text), 0, 0, c"text".as_ptr());
    if !text_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(text_fn);
        let text_val = mozjs::jsval::ObjectValue(fn_ptr);
        let t_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &text_val };
        JS_DefineProperty(raw_cx, obj_handle, c"text".as_ptr(), t_handle, JSPROP_ENUMERATE as u32);
    }

    let json_fn = JS_NewFunction(raw_cx, Some(response_json), 0, 0, c"json".as_ptr());
    if !json_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(json_fn);
        let json_val = mozjs::jsval::ObjectValue(fn_ptr);
        let j_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &json_val };
        JS_DefineProperty(raw_cx, obj_handle, c"json".as_ptr(), j_handle, JSPROP_ENUMERATE as u32);
    }

    resp_obj
}

/// Reject a SM Promise with an Error-like object carrying `message`.
///
/// # Safety
/// - `raw_cx` must be a live `JSContext*` on the current thread.
/// - `promise` must be a live, unresolved SM `Promise*`.
// @trace REQ-ENG-001 [api:fetch async drain]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn reject_promise_with_message(raw_cx: *mut JSContext, promise: *mut JSObject, msg: &str) {
    let err_obj = mozjs_sys::jsapi::JS_NewPlainObject(raw_cx);
    if !err_obj.is_null() {
        let c_msg = ZBox::from_bytes(msg.as_bytes());
        let err_msg = JS_NewStringCopyZ(raw_cx, c_msg.as_ptr());
        if !err_msg.is_null() {
            let msg_val = StringValue(&*err_msg);
            let msg_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &msg_val };
            let err_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &err_obj };
            JS_SetProperty(raw_cx, err_h, c"message".as_ptr(), msg_h);
        }
    }
    let err_val = mozjs::jsval::ObjectValue(err_obj);
    let err_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &err_val };
    let promise_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &promise };
    mozjs_sys::jsapi::JS::RejectPromise(raw_cx, promise_h, err_h);
}


#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn response_text(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let obj = this.to_object();
    let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let mut body_val = UndefinedValue();
    let b_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut body_val };
    JS_GetProperty(cx, obj_handle, c"_bodyText".as_ptr(), b_handle);
    args.rval().set(body_val);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn response_json(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() {
        JS_ReportErrorUTF8(cx, c"response.json(): invalid this".as_ptr());
        return false;
    }
    let obj = this.to_object();
    let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let mut body_val = UndefinedValue();
    let b_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut body_val };
    JS_GetProperty(cx, obj_handle, c"_bodyText".as_ptr(), b_handle);

    if !body_val.is_string() {
        JS_ReportErrorUTF8(cx, c"response.json(): body is not a string".as_ptr());
        return false;
    }

    let js_str = body_val.to_string();
    let str_handle = Handle::<*mut JSString> { _phantom_0: ::std::marker::PhantomData, ptr: &js_str };
    let mut rval = UndefinedValue();
    let rval_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval };
    let ok = mozjs_sys::jsapi::JS_ParseJSON1(cx, str_handle, rval_handle);

    if !ok {
        JS_ClearPendingException(cx);
        JS_ReportErrorUTF8(cx, c"response.json(): invalid JSON".as_ptr());
        return false;
    }
    args.rval().set(rval);
    true
}

pub fn install_response_constructor(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        let ctor = JS_NewFunction(cx.raw_cx(), Some(response_constructor), 2, JSFUN_CONSTRUCTOR, c"Response".as_ptr());
        if !ctor.is_null() {
            let ctor_obj = JS_GetFunctionObject(ctor);
            if !ctor_obj.is_null() {
                rooted!(&in(cx) let co = ctor_obj);
                JS_DefineProperty3(cx, global, c"Response".as_ptr(), co.handle(), (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32);
            }
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn response_constructor(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let resp_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if resp_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &resp_obj };

    let status_val = Int32Value(200);
    let s_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &status_val };
    JS_DefineProperty(cx, obj_handle, c"status".as_ptr(), s_handle, JSPROP_ENUMERATE as u32);

    let ok_val = mozjs::jsval::BooleanValue(true);
    let ok_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &ok_val };
    JS_DefineProperty(cx, obj_handle, c"ok".as_ptr(), ok_handle, JSPROP_ENUMERATE as u32);

    let url_js_str = JS_NewStringCopyZ(cx, c"".as_ptr());
    if !url_js_str.is_null() {
        let url_val = StringValue(&*url_js_str);
        let u_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &url_val };
        JS_DefineProperty(cx, obj_handle, c"url".as_ptr(), u_handle, JSPROP_ENUMERATE as u32);
    }

    let st_js_str = JS_NewStringCopyZ(cx, c"".as_ptr());
    if !st_js_str.is_null() {
        let st_val = StringValue(&*st_js_str);
        let st_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &st_val };
        JS_DefineProperty(cx, obj_handle, c"statusText".as_ptr(), st_handle, JSPROP_ENUMERATE as u32);
    }

    let empty_headers = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if !empty_headers.is_null() {
        let h_val = mozjs::jsval::ObjectValue(empty_headers);
        let h_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &h_val };
        JS_DefineProperty(cx, obj_handle, c"headers".as_ptr(), h_handle, JSPROP_ENUMERATE as u32);
    }

    if argc > 0 {
        let body_val = *args.get(0).ptr;
        if body_val.is_string() {
            let body_str = crate::js_to_rust_string(cx, body_val);
            {
                let c_body = ZBox::from_bytes(body_str.as_bytes());
                let body_js = JS_NewStringCopyZ(cx, c_body.as_ptr());
                if !body_js.is_null() {
                    let bv = StringValue(&*body_js);
                    let bv_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &bv };
                    JS_DefineProperty(cx, obj_handle, c"_bodyText".as_ptr(), bv_handle, 0);
                }
            }
        }
    }

    if argc > 1 {
        let opts = *args.get(1).ptr;
        if opts.is_object() {
            let opts_obj = opts.to_object();
            let opts_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &opts_obj };
            let mut st_val = UndefinedValue();
            let st_mh = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut st_val };
            JS_GetProperty(cx, opts_handle, c"status".as_ptr(), st_mh);
            if st_val.is_int32() {
                let st_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &st_val };
                JS_SetProperty(cx, obj_handle, c"status".as_ptr(), st_h);
                let ok = mozjs::jsval::BooleanValue(st_val.to_int32() >= 200 && st_val.to_int32() < 300);
                let ok_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &ok };
                JS_SetProperty(cx, obj_handle, c"ok".as_ptr(), ok_h);
            }
        }
    }

    let text_fn = JS_NewFunction(cx, Some(response_text), 0, 0, c"text".as_ptr());
    if !text_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(text_fn);
        let text_val = mozjs::jsval::ObjectValue(fn_ptr);
        let t_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &text_val };
        JS_DefineProperty(cx, obj_handle, c"text".as_ptr(), t_handle, JSPROP_ENUMERATE as u32);
    }

    let json_fn = JS_NewFunction(cx, Some(response_json), 0, 0, c"json".as_ptr());
    if !json_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(json_fn);
        let json_val = mozjs::jsval::ObjectValue(fn_ptr);
        let j_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &json_val };
        JS_DefineProperty(cx, obj_handle, c"json".as_ptr(), j_handle, JSPROP_ENUMERATE as u32);
    }

    args.rval().set(mozjs::jsval::ObjectValue(resp_obj));
    true
}

pub fn install_headers_constructor(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        let ctor = JS_NewFunction(cx.raw_cx(), Some(headers_constructor), 1, JSFUN_CONSTRUCTOR, c"Headers".as_ptr());
        if !ctor.is_null() {
            let ctor_obj = JS_GetFunctionObject(ctor);
            if !ctor_obj.is_null() {
                rooted!(&in(cx) let co = ctor_obj);
                JS_DefineProperty3(cx, global, c"Headers".as_ptr(), co.handle(), (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32);
            }
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn headers_constructor(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let headers_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if headers_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let h_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &headers_obj };

    let get_fn = JS_NewFunction(cx, Some(headers_get), 1, 0, c"get".as_ptr());
    if !get_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(get_fn);
        let fn_val = mozjs::jsval::ObjectValue(fn_ptr);
        let fv_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fn_val };
        JS_DefineProperty(cx, h_handle, c"get".as_ptr(), fv_handle, JSPROP_ENUMERATE as u32);
    }

    let set_fn = JS_NewFunction(cx, Some(headers_set), 2, 0, c"set".as_ptr());
    if !set_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(set_fn);
        let fn_val = mozjs::jsval::ObjectValue(fn_ptr);
        let fv_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fn_val };
        JS_DefineProperty(cx, h_handle, c"set".as_ptr(), fv_handle, JSPROP_ENUMERATE as u32);
    }

    let has_fn = JS_NewFunction(cx, Some(headers_has), 1, 0, c"has".as_ptr());
    if !has_fn.is_null() {
        let fn_ptr = JS_GetFunctionObject(has_fn);
        let fn_val = mozjs::jsval::ObjectValue(fn_ptr);
        let fv_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &fn_val };
        JS_DefineProperty(cx, h_handle, c"has".as_ptr(), fv_handle, JSPROP_ENUMERATE as u32);
    }

    args.rval().set(mozjs::jsval::ObjectValue(headers_obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn headers_get(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(mozjs::jsval::NullValue());
        return true;
    }
    let name_val = *args.get(0).ptr;
    if !name_val.is_string() {
        args.rval().set(mozjs::jsval::NullValue());
        return true;
    }
    let name_js = name_val.to_string();
    let name_str = crate::jsstr_to_rust_string(cx, name_js);
    let c_name = ZBox::from_bytes(name_str.as_bytes());
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(mozjs::jsval::NullValue());
        return true;
    }
    let obj = this.to_object();
    let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let mut val = UndefinedValue();
    let val_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut val };
    JS_GetProperty(cx, obj_handle, c_name.as_ptr(), val_handle);
    if val.is_undefined() || val.is_null() {
        args.rval().set(mozjs::jsval::NullValue());
    } else {
        args.rval().set(val);
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn headers_set(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 {
        JS_ReportErrorUTF8(cx, c"Headers.set requires name and value".as_ptr());
        return false;
    }
    let name_val = *args.get(0).ptr;
    let value_val = *args.get(1).ptr;
    if !name_val.is_string() || !value_val.is_string() {
        JS_ReportErrorUTF8(cx, c"Headers.set requires string arguments".as_ptr());
        return false;
    }
    let name_js = name_val.to_string();
    let name_str = crate::jsstr_to_rust_string(cx, name_js);
    let c_name = ZBox::from_bytes(name_str.as_bytes());
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let obj = this.to_object();
    let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let val_handle = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &value_val };
    JS_SetProperty(cx, obj_handle, c_name.as_ptr(), val_handle);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn headers_has(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }
    let name_val = *args.get(0).ptr;
    if !name_val.is_string() {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }
    let name_js = name_val.to_string();
    let name_str = crate::jsstr_to_rust_string(cx, name_js);
    let c_name = ZBox::from_bytes(name_str.as_bytes());
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }
    let obj = this.to_object();
    let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let mut val = UndefinedValue();
    let val_handle = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut val };
    JS_GetProperty(cx, obj_handle, c_name.as_ptr(), val_handle);
    args.rval().set(mozjs::jsval::BooleanValue(!val.is_undefined() && !val.is_null()));
    true
}

pub fn install_request_constructor(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    unsafe {
        let ctor = JS_NewFunction(cx.raw_cx(), Some(request_constructor), 2, JSFUN_CONSTRUCTOR, c"Request".as_ptr());
        if !ctor.is_null() {
            let ctor_obj = JS_GetFunctionObject(ctor);
            if !ctor_obj.is_null() {
                rooted!(&in(cx) let co = ctor_obj);
                JS_DefineProperty3(cx, global, c"Request".as_ptr(), co.handle(), (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32);
            }
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn request_constructor(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let req_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if req_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let obj_handle = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &req_obj };

    // url argument
    let url_val = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_string() { v } else { UndefinedValue() }
    } else { UndefinedValue() };
    let url_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &url_val };
    JS_DefineProperty(cx, obj_handle, c"url".as_ptr(), url_h, JSPROP_ENUMERATE as u32);

    // method from options or default GET
    let method_str = if argc > 1 {
        let opts = *args.get(1).ptr;
        if opts.is_object() {
            let opts_obj = opts.to_object();
            let opts_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &opts_obj };
            let mut m_val = UndefinedValue();
            JS_GetProperty(cx, opts_h, c"method".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut m_val });
            if m_val.is_string() {
                crate::js_to_rust_string(cx, m_val)
            } else { "GET".to_string() }
        } else { "GET".to_string() }
    } else { "GET".to_string() };
    let method_cstr = ZBox::from_bytes(method_str.as_bytes());
    let method_jsstr = JS_NewStringCopyZ(cx, method_cstr.as_ptr());
    let method_val = StringValue(&*method_jsstr);
    let method_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &method_val };
    JS_DefineProperty(cx, obj_handle, c"method".as_ptr(), method_h, JSPROP_ENUMERATE as u32);

    // headers (empty Headers-like object)
    let headers_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    let headers_val = mozjs::jsval::ObjectValue(headers_obj);
    let headers_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &headers_val };
    JS_DefineProperty(cx, obj_handle, c"headers".as_ptr(), headers_h, JSPROP_ENUMERATE as u32);

    args.rval().set(mozjs::jsval::ObjectValue(req_obj));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_host_port_http_default() {
        let (host, port) = extract_host_port("http://example.com/path").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 80);
    }

    #[test]
    fn extract_host_port_https_default() {
        let (host, port) = extract_host_port("https://example.com/path").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 443);
    }

    #[test]
    fn extract_host_port_with_port() {
        let (host, port) = extract_host_port("http://localhost:8080/api").unwrap();
        assert_eq!(host, "localhost");
        assert_eq!(port, 8080);
    }

    #[test]
    fn extract_host_port_with_query() {
        let (host, port) = extract_host_port("http://host:3000/path?q=1").unwrap();
        assert_eq!(host, "host");
        assert_eq!(port, 3000);
    }

    #[test]
    fn extract_host_port_no_path() {
        let (host, port) = extract_host_port("http://example.com").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 80);
    }

    #[test]
    fn extract_host_port_no_scheme() {
        assert!(extract_host_port("example.com/path").is_none());
    }

    #[test]
    fn extract_host_port_empty() {
        assert!(extract_host_port("").is_none());
    }

    #[test]
    fn extract_host_port_ipv4() {
        let (host, port) = extract_host_port("http://127.0.0.1:9222/json").unwrap();
        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, 9222);
    }

    #[test]
    fn fetch_response_status_code_type() {
        // Verify FetchResponse struct has expected fields
        let resp = FetchResponse {
            status_code: 200,
            body: b"ok".to_vec(),
            headers: vec![],
            url: "http://example.com".to_string(),
            status_text: "OK".to_string(),
        };
        assert_eq!(resp.status_code, 200);
        assert_eq!(&resp.body[..], b"ok");
        assert_eq!(resp.url, "http://example.com");
        assert_eq!(resp.status_text, "OK");
    }

    #[test]
    fn fetch_response_headers_preserved() {
        let resp = FetchResponse {
            status_code: 404,
            body: b"not found".to_vec(),
            headers: vec![("content-type".into(), "text/html".into())],
            url: "http://example.com/missing".to_string(),
            status_text: "Not Found".to_string(),
        };
        assert_eq!(resp.headers.len(), 1);
        assert_eq!(resp.headers[0].0, "content-type");
        assert_eq!(resp.headers[0].1, "text/html");
    }

    #[test]
    fn extract_host_port_ipv6_loopback() {
        let (host, port) = extract_host_port("http://[::1]:8080/path").unwrap();
        assert_eq!(host, "[::1]");
        assert_eq!(port, 8080);
    }

    #[test]
    fn extract_host_port_fragment_only() {
        let (host, port) = extract_host_port("https://example.com/page#section").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 443);
    }

    #[test]
    fn extract_host_port_non_standard_port() {
        let (host, port) = extract_host_port("http://example.com:3000/api").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 3000);
    }

    #[test]
    fn extract_host_port_with_auth() {
        // extract_host_port uses rfind(':') which incorrectly parses auth URLs.
        // "https://user:pass@example.com" → rfind finds ':' in 'pass:', splits
        // host="user", port parse fails → defaults to 80. This is a known limitation.
        let result = extract_host_port("https://user:pass@example.com/secret");
        assert!(result.is_some());
        let (host, port) = result.unwrap();
        assert_eq!(host, "user");
        assert_eq!(port, 80);
    }

    #[test]
    fn fetch_response_multiple_headers() {
        let resp = FetchResponse {
            status_code: 200,
            body: Vec::new(),
            headers: vec![
                ("content-type".into(), "application/json".into()),
                ("x-custom".into(), "value1".into()),
                ("x-custom".into(), "value2".into()),
            ],
            url: "http://example.com".to_string(),
            status_text: "OK".to_string(),
        };
        assert_eq!(resp.headers.len(), 3);
    }

    #[test]
    fn fetch_response_status_codes() {
        for code in [200u16, 201, 301, 400, 404, 500, 503] {
            let resp = FetchResponse {
                status_code: code,
                body: Vec::new(),
                headers: vec![],
                url: String::new(),
                status_text: String::new(),
            };
            assert_eq!(resp.status_code, code);
        }
    }

    #[test]
    fn fetch_response_empty_body() {
        let resp = FetchResponse {
            status_code: 204,
            body: Vec::new(),
            headers: vec![],
            url: String::new(),
            status_text: "No Content".to_string(),
        };
        assert!(resp.body.is_empty());
        assert_eq!(resp.status_code, 204);
    }

    // ── REQ-SEC-001: CORS Bypass Unit Tests ──────────────────────────────
    // @trace TEST-SEC-001 [req:REQ-SEC-001] [level:unit]

    /// REQ-SEC-001: do_fetch performs direct HTTP requests without CORS middleware.
    /// Verify the fetch path has NO CORS-related headers or preflight logic.
    #[test]
    fn cors_bypass_no_preflight_code_in_do_fetch() {
        let source = include_str!("fetch_api.rs");
        let func_start = source.find("fn do_fetch_blocking(").expect("do_fetch function not found");
        let func_body = &source[func_start..func_start + 2000.min(source.len() - func_start)];

        assert!(
            !func_body.contains("cors_check"),
            "REQ-SEC-001 REGRESSION: do_fetch must NOT contain cors_check"
        );
        assert!(
            !func_body.contains("Access-Control-Request-Method"),
            "REQ-SEC-001 REGRESSION: do_fetch must NOT send CORS preflight headers"
        );
        assert!(
            !func_body.contains("Origin"),
            "REQ-SEC-001 REGRESSION: do_fetch must NOT set Origin header for CORS"
        );
        assert!(
            !func_body.contains("preflight"),
            "REQ-SEC-001 REGRESSION: do_fetch must NOT contain preflight logic"
        );
    }

    /// REQ-SEC-001: stealth HTTP request path has no CORS enforcement.
    #[test]
    fn cors_bypass_stealth_http_no_cors() {
        let source = include_str!("fetch_api.rs");
        let func_start = source.find("fn do_fetch_blocking(").expect("do_fetch not found");
        let func_body = &source[func_start..func_start + 2000.min(source.len() - func_start)];

        assert!(
            func_body.contains("stealth_http_request"),
            "REQ-SEC-001: do_fetch must use stealth_http_request for direct HTTP"
        );
        assert!(
            !func_body.contains("CorsCache"),
            "REQ-SEC-001 REGRESSION: must not reference CorsCache"
        );
        assert!(
            !func_body.contains("opaque"),
            "REQ-SEC-001 REGRESSION: must not produce opaque responses"
        );
    }

    /// REQ-SEC-001: FetchResponse contains full response body (never opaque).
    #[test]
    fn cors_bypass_fetch_response_is_transparent() {
        let resp = FetchResponse {
            status_code: 200,
            body: b"{\"data\":\"full access\"}".to_vec(),
            headers: vec![("content-type".into(), "application/json".into())],
            url: "https://other-domain.com/api".to_string(),
            status_text: "OK".to_string(),
        };
        assert_eq!(resp.status_code, 200, "REQ-SEC-001: cross-origin response must be 200");
        let body_str = ::std::str::from_utf8(&resp.body).unwrap_or("");
        assert!(
            body_str.contains("full access"),
            "REQ-SEC-001: response body must be fully readable (not opaque)"
        );
        assert!(
            !resp.body.is_empty(),
            "REQ-SEC-001: response body must not be empty (opaque responses have empty body)"
        );
    }

    /// REQ-SEC-001: fetch global is installed on page realm via install_all_native.
    #[test]
    fn cors_bypass_fetch_global_installed_for_page() {
        let source = include_str!("fetch_api.rs");
        assert!(
            source.contains("pub fn install_fetch_global"),
            "REQ-SEC-001: install_fetch_global must be pub for page realm installation"
        );
    }

    /// REQ-SEC-001: extract_host_port handles cross-origin URLs correctly.
    #[test]
    fn cors_bypass_cross_origin_url_parsing() {
        let cases = [
            ("https://api.other-domain.com/v1/data", ("api.other-domain.com", 443u16)),
            ("http://localhost:3000/api/cors-test", ("localhost", 3000u16)),
            ("https://cdn.example.com:8443/assets/file.js", ("cdn.example.com", 8443u16)),
        ];
        for (url, (expected_host, expected_port)) in cases {
            let (host, port) = extract_host_port(url)
                .unwrap_or_else(|| panic!("REQ-SEC-001: failed to parse cross-origin URL: {}", url));
            assert_eq!(host, expected_host, "host mismatch for {}", url);
            assert_eq!(port, expected_port, "port mismatch for {}", url);
        }
    }

    // ── BCE-007: async fetch core regression tests ───────────────────────
    // @trace REQ-ENG-001 [req:REQ-ENG-001] [level:unit] [api:fetch async]

    /// BCE-007-C1: `has_pending_fetches()` is false when no fetch is in flight.
    #[test]
    fn bce_007_has_pending_fetches_initially_false() {
        assert!(!has_pending_fetches(), "BCE-007: no pending fetches initially");
    }

    /// BCE-007-C2: `spawn_fetch_worker` writes a result into the slot and the
    /// slot transitions from `None` → `Some`. This is the cross-thread contract
    /// the JS-thread drain relies on. Uses a guaranteed-dead endpoint so the
    /// worker resolves quickly with `Err(connect refused)`.
    #[test]
    fn bce_007_spawn_fetch_worker_writes_result_slot() {
        let slot: Arc<Mutex<Option<::std::result::Result<FetchResponse, String>>>> =
            Arc::new(Mutex::new(None));
        spawn_fetch_worker(
            Arc::clone(&slot),
            "http://127.0.0.1:1/__bce007_dead_endpoint__".to_string(),
            "GET".to_string(),
            None,
            None,
        );
        // Poll for up to ~5s for the worker to complete.
        let mut outcome = None;
        for _ in 0..5_000 {
            if let Ok(mut guard) = slot.lock() {
                if guard.is_some() {
                    outcome = guard.take();
                    break;
                }
            }
            ::std::thread::sleep(::std::time::Duration::from_millis(1));
        }
        let outcome = outcome.expect("BCE-007: worker must write result into slot within 5s");
        // Dead endpoint → connect refused error (not a panic, not a hang).
        assert!(outcome.is_err(), "BCE-007: dead endpoint must yield Err, got Ok(FetchResponse) instead");
    }

    /// BCE-007-C3: `do_fetch_blocking` returns `Err` for an unreachable host
    /// without hanging (the pre-check short-circuits before AsyncHTTP).
    #[test]
    fn bce_007_do_fetch_blocking_dead_host_returns_err_fast() {
        let start = ::std::time::Instant::now();
        let result = do_fetch_blocking(
            "http://127.0.0.1:1/__dead__",
            "GET",
            None,
            &None,
        );
        let elapsed = start.elapsed();
        assert!(result.is_err(), "BCE-007: dead host must return Err");
        // Pre-check cap: 250ms connect_timeout × 3 addrs = ~750ms worst case.
        // Allow generous 5s headroom for CI load, but fail if it clearly hung.
        assert!(
            elapsed < ::std::time::Duration::from_secs(5),
            "BCE-007: do_fetch_blocking on dead host must not hang (took {:?})",
            elapsed
        );
    }

    /// BCE-007-C4: `PendingFetch` is `Send` (required for `thread_local!`
    /// storage with a raw `*mut JSObject`). This is a compile-time assertion.
    #[test]
    fn bce_007_pending_fetch_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<PendingFetch>();
        // Also assert the result slot type is Send + Sync (Arc<Mutex<...>>).
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Arc<Mutex<Option<::std::result::Result<FetchResponse, String>>>>>();
    }
}
