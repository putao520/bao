// @trace REQ-ENG-010 [entity:FetchTasklet] [req:REQ-ENG-010] [level:library]
//! Async fetch/HTTP integration with the event loop (FetchTasklet pattern).
//!
//! ## Why this exists (BCE-20260618-007)
//!
//! The legacy JS-native http/https/tls entries called
//! `stealth_http_request` (→ `http_client::http_request` →
//! `AsyncHTTP::send_sync`) directly *inside* the JS-native call frame. That
//! blocked the JS thread on the `SingleHTTPChannel` Condvar while
//! `evaluate_script` was still on the stack, so the post-eval event-loop
//! hook never ran. In a `Bun.serve({ fetch() { return fetch(self) } })`
//! self-loop, the server's uWS App could never `accept` the in-flight
//! self-loop connection → bidirectional deadlock.
//!
//! ## Fix (paradigm-level)
//!
//! Every JS-native http/https/tls entry (this module's consumers) now
//! returns a *pending* `Promise`, hands the actual network I/O to a
//! detached Rust worker thread, and registers a `PendingFetch` on this
//! thread's registry. The JS-thread drain step (`drain_pending`) consumes
//! completed fetches, builds the Response/error JS object, and
//! `ResolvePromise`/`RejectPromise`s. The pending Promise is heap-rooted
//! (`AddRawValueRoot`) across the async window so SM GC cannot collect it;
//! `RemoveRawValueRoot` runs on every exit path (resolve/reject), satisfying
//! RISK-A (GC root) and RISK-C (poll_ref lifetime).
//!
//! This mirrors Bun's `FetchTasklet` design (schedule + cross-thread result
//! channel + JS-thread resolve) using the simpler `thread::spawn` worker
//! model, which is sound because Bao runs JS single-threaded and the worker
//! only touches pure Rust (no SM API) — keeping the INV-5 cross-thread
//! invariant trivially satisfied.
//!
//! ## Scope
//!
//! Shared helper used by the HTTP-sweep entries:
//! - `node_http.rs:http_request` / `http_get` (http_get delegates to http_request)
//! - `node_https.rs:https_request`
//! - `node_tls.rs:tls_connect`
//!
//! `h3_fetch.rs` is excluded — it has no `send_sync` path.

use ::std::cell::RefCell;
use ::std::ffi::c_char;
use ::std::sync::{Arc, Mutex};

use bun_core::ZBox;
use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, ObjectValue, StringValue, UndefinedValue};

use crate::stealth_http::{StealthSyncResult, stealth_http_request};

// ──────────────────────────────────────────────────────────────────────────
// Public types
// ──────────────────────────────────────────────────────────────────────────

/// Worker-thread result of a scheduled fetch. Pure data — no SM handles — so
/// it can cross the thread boundary freely (INV-5: no SM API on HTTP thread).
type FetchOutcome = ::std::result::Result<StealthSyncResult, String>;

/// How to materialize the worker's result as a JS object on resolve. Different
/// JS-native entries want different shapes: `fetch`/`http.request`/`https.request`
/// want a `Response`; `tls.connect` (a TLS handshake probe) wants a `TLSSocket`.
#[derive(Clone, Copy)]
pub enum ResolveKind {
    /// Build a fetch-style Response object (status/ok/headers/text()).
    Response,
    /// Build a TLSSocket object (authorized/encrypted/servername). `host` is
    /// captured by the probe caller and surfaced as `servername`.
    TlsSocket { host_idx: usize },
}

// Host strings captured at JS-native-call time, indexed by `ResolveKind`-
// carried indices. The worker must not touch JS state, so we pass plain Rust
// strings across and let the JS-thread resolver look them up by index.
// (Doc comment moved to a plain comment because `thread_local!` does not
// surface doc-comments on the generated item.)
thread_local! {
    static HOST_STRINGS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// A fetch tasklet: pending Promise + the channel back from the worker thread.
///
/// Invariants (FetchTaskletLifecycle SM):
/// - `rooted == true` while the Promise is outstanding (heap root held).
/// - Every terminal transition (resolve/reject) must `RemoveRawValueRoot`.
pub struct PendingFetch {
    /// SpiderMonkey context that owns the Promise. Only touched on the JS thread.
    pub cx: *mut JSContext,
    /// Heap-rooted pending Promise *value*. Rooted while `outcome.is_none()`.
    pub promise_val: JSVal,
    /// `true` while `AddRawValueRoot` is in effect.
    pub rooted: bool,
    /// Worker-thread result slot. `None` until the worker writes the outcome.
    pub outcome: Arc<Mutex<Option<FetchOutcome>>>,
    /// How to materialize the result on the JS thread.
    pub kind: ResolveKind,
}

// SAFETY: `cx`/`promise_val` are only ever dereferenced on the JS thread that
// created them; the worker thread only touches `outcome` (pure Rust). Sending
// the struct across threads is sound as long as no SM API is called off the
// JS thread — enforced here by keeping all SM access behind `drain_pending`
// (JS-thread only).
unsafe impl Send for PendingFetch {}

// ──────────────────────────────────────────────────────────────────────────
// Pending-fetch registry (JS-thread local)
// ──────────────────────────────────────────────────────────────────────────

thread_local! {
    static PENDING: RefCell<Vec<PendingFetch>> = const { RefCell::new(Vec::new()) };
}

/// JS-thread poll: are there any outstanding async fetches on this thread?
pub fn has_pending() -> bool {
    PENDING.with(|p| !p.borrow().is_empty())
}

// ──────────────────────────────────────────────────────────────────────────
// start() — JS-thread: register a pending fetch + schedule worker
// ──────────────────────────────────────────────────────────────────────────

/// Schedule an async fetch on a detached worker thread. The caller must have
/// already created the pending Promise via `JS::NewPromiseObject(cx, null)`,
/// pass it here as `promise_val` (an Object JSVal), and then set `args.rval()`
/// to the same value before returning from the extern-C trampoline.
///
/// This function:
///   1. Heap-roots the Promise value (GUARD-A: SM GC safety across ticks).
///   2. Spawns a worker that calls `stealth_http_request` (true non-blocking).
///   3. Pushes a `PendingFetch` onto the JS-thread registry.
///
/// # Safety
///
/// - `cx` must be a live `JSContext*` on the current thread.
/// - `promise_val` must be an Object JSVal holding a *pending* Promise.
pub unsafe fn start(
    cx: *mut JSContext,
    promise_val: JSVal,
    profile: Option<crate::stealth_http::StealthProfile>,
    method: bun_http::Method,
    url: String,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
) {
    // SAFETY: delegate to the kind-aware start with the default Response form.
    unsafe { start_with_kind(cx, promise_val, profile, method, url, headers, body, ResolveKind::Response, None) }
}

/// Schedule a TLS handshake probe: a single stealth HTTPS HEAD against
/// `host:port`. The Promise resolves to a TLSSocket-shaped object
/// (`authorized`/`encrypted`/`servername`) on success, or rejects on error.
/// `host` is captured so the resolver can surface it as `servername`.
///
/// # Safety
///
/// - `cx` must be a live `JSContext*` on the current thread.
/// - `promise_val` must be an Object JSVal holding a *pending* Promise.
pub unsafe fn start_tls_probe(
    cx: *mut JSContext,
    promise_val: JSVal,
    host: String,
    port: u16,
) {
    let test_url = format!("https://{}:{}", host, port);
    // Capture the host string on the JS thread; the resolver looks it up by
    // index so the worker never touches JS state.
    let host_idx = HOST_STRINGS.with(|h| {
        let mut g = h.borrow_mut();
        let idx = g.len();
        g.push(host);
        idx
    });
    // SAFETY: cx live on this thread; promise_val is the pending Promise.
    unsafe {
        start_with_kind(
            cx,
            promise_val,
            None,
            bun_http::Method::HEAD,
            test_url,
            Vec::new(),
            None,
            ResolveKind::TlsSocket { host_idx },
            None,
        )
    }
}

/// Kind-aware scheduler. `body_slice_override` lets a caller pass a body that
/// is *not* owned by the worker closure (reserved for future use; pass None).
///
/// # Safety
///
/// - `cx` must be a live `JSContext*` on the current thread.
/// - `promise_val` must be an Object JSVal holding a *pending* Promise.
unsafe fn start_with_kind(
    cx: *mut JSContext,
    promise_val: JSVal,
    profile: Option<crate::stealth_http::StealthProfile>,
    method: bun_http::Method,
    url: String,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
    kind: ResolveKind,
    _body_slice_override: Option<Vec<u8>>,
) {
    // GUARD-A (GC root): heap-root the pending Promise value across the async
    // window. The async window spans ticks, so the stack-rooted!() macro
    // (whose roots die with the frame) is unsound here — we use the runtime's
    // raw root table instead.
    let mut pv = promise_val;
    let rooted = unsafe {
        let name = b"FetchTasklet.promise\0".as_ptr() as *const c_char;
        AddRawValueRoot(cx, &mut pv, name)
    };
    let rooted_val = if rooted { pv } else { promise_val };

    let outcome: Arc<Mutex<Option<FetchOutcome>>> = Arc::new(Mutex::new(None));
    let worker_outcome = Arc::clone(&outcome);

    // Detached worker: pure Rust only — no SM API on this thread (INV-5).
    // DNS precheck, connect timeout, redirect handling all live inside
    // stealth_http_request → http_client::http_request (CTRL-6: precheck is
    // moved off the synchronous JS-native frame).
    let _ = ::std::thread::Builder::new()
        .name("bao-http-worker".into())
        .spawn(move || {
            let body_ref: Option<&[u8]> = body.as_deref();
            let result = stealth_http_request(&profile, method, &url, &headers, body_ref);
            if let Ok(mut slot) = worker_outcome.lock() {
                *slot = Some(result.map_err(|e| e.to_string()));
            }
        });

    PENDING.with(|p| {
        p.borrow_mut().push(PendingFetch {
            cx,
            promise_val: rooted_val,
            rooted,
            outcome,
            kind,
        });
    });
}

// ──────────────────────────────────────────────────────────────────────────
// drain_pending() — JS-thread: resolve/reject completed fetches
// ──────────────────────────────────────────────────────────────────────────

/// Drain completed fetches on the JS thread. Called from the event-loop hook.
/// Returns `true` if any fetch was resolved/rejected this pass (caller may
/// re-run `RunJobs`).
///
/// # Safety
///
/// - `cx` must be the same `JSContext*` that created the pending Promises.
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn drain_pending(cx: *mut JSContext) -> bool {
    let mut completed = false;
    PENDING.with(|p| {
        let mut guard = p.borrow_mut();
        let mut i = 0;
        while i < guard.len() {
            // try_lock: if the worker holds the lock mid-write we just skip
            // this entry this tick — the next drain pass will pick it up.
            let ready = guard[i]
                .outcome
                .try_lock()
                .map(|slot| slot.is_some())
                .unwrap_or(false);
            if !ready {
                i += 1;
                continue;
            }
            // Move the tasklet out and resolve on the JS thread (INV-5:
            // all SM API calls happen here, not in the worker).
            let tasklet = guard.swap_remove(i);
            let outcome = tasklet
                .outcome
                .lock()
                .ok()
                .and_then(|mut slot| slot.take())
                .unwrap_or_else(|| Err("fetch worker dropped result".into()));
            resolve_tasklet(cx, tasklet, outcome);
            completed = true;
        }
    });
    if completed {
        // Flush microtasks queued by ResolvePromise/RejectPromise.
        mozjs_sys::jsapi::js::RunJobs(cx);
    }
    completed
}

/// Build the Response/error JS object and resolve/reject the rooted Promise.
/// Then unroot (terminal transition). Every exit path unroots (INV-2: zero
/// exceptions).
///
/// # Safety
///
/// `cx` must be the Promise's owning `JSContext*` on the current thread.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn resolve_tasklet(cx: *mut JSContext, tasklet: PendingFetch, outcome: FetchOutcome) {
    let promise_obj = tasklet.promise_val.to_object();
    let promise_h = Handle::<*mut JSObject> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &promise_obj,
    };

    match (outcome, tasklet.kind) {
        (Ok(resp), ResolveKind::Response) => {
            let resp_obj = build_response_js(cx, &resp);
            if !resp_obj.is_null() {
                let resp_val = ObjectValue(resp_obj);
                let resp_handle = Handle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &resp_val,
                };
                JS::ResolvePromise(cx, promise_h, resp_handle);
            } else {
                // Allocation failure — reject so the promise doesn't hang.
                reject_with_message(cx, promise_h, "http: failed to build Response");
            }
        }
        (Ok(_resp), ResolveKind::TlsSocket { host_idx }) => {
            // Handshake succeeded — build the TLSSocket object.
            let host = HOST_STRINGS
                .with(|h| h.borrow().get(host_idx).cloned())
                .unwrap_or_default();
            let tls_obj = build_tls_socket_js(cx, &host);
            if !tls_obj.is_null() {
                let tls_val = ObjectValue(tls_obj);
                let tls_handle = Handle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &tls_val,
                };
                JS::ResolvePromise(cx, promise_h, tls_handle);
            } else {
                reject_with_message(cx, promise_h, "tls: failed to build socket object");
            }
            // Drop the captured host string (best-effort leak-prevention).
            HOST_STRINGS.with(|h| {
                if host_idx < h.borrow().len() {
                    h.borrow_mut()[host_idx].clear();
                }
            });
        }
        (Err(msg), _) => {
            reject_with_message(cx, promise_h, &msg);
        }
    }

    // Terminal cleanup: unroot (every exit path).
    if tasklet.rooted {
        let mut pv = tasklet.promise_val;
        RemoveRawValueRoot(cx, &mut pv);
    }
}

/// Build a TLSSocket-shaped JS object: `{ authorized: true, encrypted: true,
/// servername: host }`. Mirrors the legacy synchronous `tls.connect` return
/// shape so consumers are unaffected.
///
/// # Safety
///
/// `cx` must be a live `JSContext*` on the current thread.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn build_tls_socket_js(cx: *mut JSContext, host: &str) -> *mut JSObject {
    let obj = JS_NewPlainObject(cx);
    if obj.is_null() {
        return obj;
    }
    let obj_handle = Handle::<*mut JSObject> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &obj,
    };

    let auth_val = mozjs::jsval::BooleanValue(true);
    let auth_h = Handle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &auth_val,
    };
    JS_DefineProperty(cx, obj_handle, c"authorized".as_ptr(), auth_h, JSPROP_ENUMERATE as u32);

    let enc_val = mozjs::jsval::BooleanValue(true);
    let enc_h = Handle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &enc_val,
    };
    JS_DefineProperty(cx, obj_handle, c"encrypted".as_ptr(), enc_h, JSPROP_ENUMERATE as u32);

    if !host.is_empty() {
        let c_host = ZBox::from_bytes(host.as_bytes());
        let host_js = JS_NewStringCopyZ(cx, c_host.as_ptr());
        if !host_js.is_null() {
            let hv = StringValue(&*host_js);
            let hv_h = Handle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &hv,
            };
            JS_DefineProperty(cx, obj_handle, c"servername".as_ptr(), hv_h, JSPROP_ENUMERATE as u32);
        }
    }

    obj
}

/// Construct the JS Response object from a `StealthSyncResult`. Shape mirrors
/// fetch_api's Response: `status`/`ok`/`statusText`/`headers`/`_bodyText`/
/// `text()`.
///
/// # Safety
///
/// `cx` must be a live `JSContext*` on the current thread.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn build_response_js(cx: *mut JSContext, resp: &StealthSyncResult) -> *mut JSObject {
    let obj = JS_NewPlainObject(cx);
    if obj.is_null() {
        return obj;
    }
    let obj_handle = Handle::<*mut JSObject> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &obj,
    };

    // status: int32
    let status_val = mozjs::jsval::Int32Value(resp.status_code as i32);
    let s_handle = Handle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &status_val,
    };
    JS_DefineProperty(cx, obj_handle, c"status".as_ptr(), s_handle, JSPROP_ENUMERATE as u32);

    // ok: boolean (2xx)
    let ok_val = mozjs::jsval::BooleanValue((200..300).contains(&resp.status_code));
    let ok_handle = Handle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &ok_val,
    };
    JS_DefineProperty(cx, obj_handle, c"ok".as_ptr(), ok_handle, JSPROP_ENUMERATE as u32);

    // statusText
    {
        let c_st = ZBox::from_bytes(resp.status_text.as_bytes());
        let st_js = JS_NewStringCopyZ(cx, c_st.as_ptr());
        if !st_js.is_null() {
            let st_val = StringValue(&*st_js);
            let st_handle = Handle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &st_val,
            };
            JS_DefineProperty(
                cx,
                obj_handle,
                c"statusText".as_ptr(),
                st_handle,
                JSPROP_ENUMERATE as u32,
            );
        }
    }

    // headers (flattened to a plain enumerable object)
    {
        let headers_obj = JS_NewPlainObject(cx);
        if !headers_obj.is_null() {
            let hdr_handle = Handle::<*mut JSObject> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &headers_obj,
            };
            for (k, v) in resp.headers.iter() {
                let c_k = ZBox::from_bytes(k.as_bytes());
                let k_js = JS_NewStringCopyZ(cx, c_k.as_ptr());
                if k_js.is_null() {
                    continue;
                }
                let kv = StringValue(&*k_js);
                let kv_handle = Handle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &kv,
                };
                let c_v = ZBox::from_bytes(v.as_bytes());
                let v_js = JS_NewStringCopyZ(cx, c_v.as_ptr());
                if v_js.is_null() {
                    continue;
                }
                let vv = StringValue(&*v_js);
                let vv_handle = Handle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &vv,
                };
                // header keys arrive lowercase from stealth_http; use as-is.
                let c_key = ZBox::from_bytes(k.as_bytes());
                JS_DefineProperty(
                    cx,
                    hdr_handle,
                    c_key.as_ptr(),
                    vv_handle,
                    JSPROP_ENUMERATE as u32,
                );
                let _ = kv_handle; // suppress unused-assign
            }
            let hv = ObjectValue(headers_obj);
            let hv_handle = Handle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &hv,
            };
            JS_DefineProperty(
                cx,
                obj_handle,
                c"headers".as_ptr(),
                hv_handle,
                JSPROP_ENUMERATE as u32,
            );
        }
    }

    // body — stored as `_bodyText` (lossy UTF-8), surfaced via `.text()`.
    {
        let body_lossy = String::from_utf8_lossy(&resp.body);
        let c_body = ZBox::from_bytes(body_lossy.as_bytes());
        let body_js = JS_NewStringCopyZ(cx, c_body.as_ptr());
        if !body_js.is_null() {
            let bv = StringValue(&*body_js);
            let b_handle = Handle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &bv,
            };
            JS_DefineProperty(cx, obj_handle, c"_bodyText".as_ptr(), b_handle, 0);
        }
    }

    // text() — no-arg JS function returning `_bodyText`.
    {
        let text_fn = JS_NewFunction(cx, Some(response_text_fn), 0, 0, c"text".as_ptr());
        if !text_fn.is_null() {
            let fn_obj = JS_GetFunctionObject(text_fn);
            if !fn_obj.is_null() {
                let fv = ObjectValue(fn_obj);
                let fv_handle = Handle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &fv,
                };
                JS_DefineProperty(
                    cx,
                    obj_handle,
                    c"text".as_ptr(),
                    fv_handle,
                    JSPROP_ENUMERATE as u32,
                );
            }
        }
    }

    obj
}

/// `.text()` method: reads `_bodyText` off the Response and returns it.
#[allow(non_snake_case)]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn response_text_fn(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let this_obj = this.to_object();
    let this_h = Handle::<*mut JSObject> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &this_obj,
    };
    let mut bt = UndefinedValue();
    let bt_h = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut bt,
    };
    JS_GetProperty(cx, this_h, c"_bodyText".as_ptr(), bt_h);
    if bt.is_string() {
        args.rval().set(bt);
    } else {
        args.rval().set(UndefinedValue());
    }
    true
}

/// Reject a Promise with a plain Error-like object carrying `.message`.
///
/// # Safety
///
/// `cx` must be live on the current thread.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn reject_with_message(
    cx: *mut JSContext,
    promise_h: Handle<*mut JSObject>,
    msg: &str,
) {
    let err_obj = JS_NewPlainObject(cx);
    if !err_obj.is_null() {
        let c_msg = ZBox::from_bytes(msg.as_bytes());
        let js_str = JS_NewStringCopyZ(cx, c_msg.as_ptr());
        if !js_str.is_null() {
            let msg_val = StringValue(&*js_str);
            let msg_handle = Handle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &msg_val,
            };
            let err_h = Handle::<*mut JSObject> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &err_obj,
            };
            JS_DefineProperty(
                cx,
                err_h,
                c"message".as_ptr(),
                msg_handle,
                JSPROP_ENUMERATE as u32,
            );
        }
    }
    let ev = if err_obj.is_null() {
        UndefinedValue()
    } else {
        ObjectValue(err_obj)
    };
    let ev_handle = Handle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &ev,
    };
    JS::RejectPromise(cx, promise_h, ev_handle);
}

// ──────────────────────────────────────────────────────────────────────────
// Unit tests — pure logic (no live JSContext)
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn has_pending_false_initially() {
        // Each test thread has its own thread_local; should start empty.
        assert!(!has_pending());
    }

    #[test]
    fn pending_fetch_is_send() {
        // Compile-time check: PendingFetch must be Send to live in the
        // thread_local registry that the worker writes back into.
        fn assert_send<T: Send>() {}
        assert_send::<PendingFetch>();
    }

    #[test]
    fn outcome_slot_roundtrip() {
        let slot: Arc<Mutex<Option<FetchOutcome>>> = Arc::new(Mutex::new(None));
        {
            let mut g = slot.lock().unwrap();
            *g = Some(Ok(stealth_result_for_test()));
        }
        let taken = slot.lock().unwrap().take();
        assert!(taken.is_some());
        match taken.unwrap() {
            Ok(r) => assert_eq!(r.status_code, 200),
            Err(_) => panic!("expected Ok"),
        }
    }

    fn stealth_result_for_test() -> StealthSyncResult {
        use compact_str::CompactString;
        use smallvec::smallvec;
        StealthSyncResult {
            status_code: 200,
            status_text: CompactString::new("OK"),
            headers: smallvec![],
            body: bytes::Bytes::from_static(b"hello"),
        }
    }
}
