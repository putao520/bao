// @trace REQ-ENG-001 [entity:BaoRuntime] [api:fetch]
// @trace REQ-ENG-006 REQ-STL-001
// fetch() entry point. The WHATWG Headers/Request/Response classes live in
// web_fetch_classes.rs (full JS implementations installed by
// globals::install_web_apis); this module owns the native fetch() function
// and its input/init parsing.
//
// ## BCE-007/R4 + BCE-20260619-010: FetchTasklet event-driven paradigm
//
// fetch() now delegates to `fetch_async::start` which uses
// `AsyncHTTP::init + HTTPThread::schedule` (single epoll thread, O(1) OS
// threads). The HTTPThread calls back `on_http_done` (pure-Rust), which
// enqueues a `ConcurrentTask` on the JS thread's MiniEventLoop. The JS
// thread auto-wakes and resolves/rejects the Promise in `resolve_tasklet`.
//
// This replaced the `thread::spawn` + `drain_pending` polling model which
// had three flaws (O(N) OS threads, busy-poll sleep, fragile drain coupling).
// See `fetch_async.rs` module-level doc for the full BCE analysis.
use ::std::sync::Arc;
use ::std::sync::atomic::AtomicBool;

use bun_core::ZBox;

use mozjs::conversions::unsafe_jsstr_to_string;
use mozjs::jsapi::*;
use mozjs::jsval::{Int32Value, JSVal, ObjectValue, StringValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2::JS_DefineFunction;

thread_local! {
    static TL_STEALTH_PROFILE: ::std::cell::RefCell<Option<bao_stealth::StealthProfile>> = const { ::std::cell::RefCell::new(None) };
}

/// Store the current page's stealth profile so fetch() can apply TLS/HTTP2 fingerprints.
pub fn set_fetch_stealth_profile(profile: Option<bao_stealth::StealthProfile>) {
    TL_STEALTH_PROFILE.with(|p| *p.borrow_mut() = profile);
}

/// Returns true if a stealth profile has been explicitly set on this thread.
pub fn is_fetch_stealth_profile_set() -> bool {
    TL_STEALTH_PROFILE.with(|p| p.borrow().is_some())
}

/// Clone the current thread's stealth profile. Single source shared by every
/// page egress path (fetch, WebSocket wss://) so all TLS handshakes from one
/// page present the identical JA3/JA4 fingerprint (REQ-STL-001 fingerprint
/// consistency).
pub fn get_fetch_stealth_profile() -> Option<bao_stealth::StealthProfile> {
    TL_STEALTH_PROFILE.with(|p| p.borrow().clone())
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
            cx,
            global,
            c"fetch".as_ptr(),
            ::std::option::Option::Some(fetch_fn),
            1,
            JSPROP_ENUMERATE as u32,
        );
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fetch_fn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"fetch requires a URL or Request argument".as_ptr());
        return false;
    }

    // ── Input: string URL or Request-like object (WHATWG fetch(input, init)) ──
    // A Request object contributes url/method/headers/body as the base; init
    // overrides any field it carries. The full classes live in
    // web_fetch_classes.rs; their instance shape is url (string), method
    // (uppercased string), headers (Headers instance) and _bodyText /
    // _bodyBytes / _bodyBlob body slots.
    let input_val = *args.get(0).ptr;
    let url: String;
    let mut method: String = "GET".to_string();
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut body: Option<Vec<u8>> = None;
    // WHATWG fetch signal: init.signal wins over the Request base. Snapshot
    // of the raw JSVal here (the object stays reachable through args —
    // init/Request object on the argv stack); rooted where consumed below.
    let mut signal_val: Option<JSVal> = None;

    if input_val.is_string() {
        url = crate::js_to_rust_string(cx, input_val);
    } else if input_val.is_object() {
        // BCE-012: root to_object() result — JS_GetProperty can trigger GC
        rooted!(&in(wrapped_cx) let req_obj = input_val.to_object());
        // Request base signal: read the raw `_signal` slot, not the `signal`
        // getter (which lazily allocates a fresh AbortController signal per
        // read — wrong object to wire and needless allocation per fetch).
        {
            let sv = get_val_prop(cx, req_obj.handle(), "_signal");
            if sv.is_object() {
                signal_val = Some(sv);
            }
        }
        match get_string_prop(cx, req_obj.handle().into(), "url") {
            ::std::option::Option::Some(u) => url = u,
            ::std::option::Option::None => {
                JS_ReportErrorUTF8(
                    cx,
                    c"fetch requires a string URL or a Request object".as_ptr(),
                );
                return false;
            }
        }
        if let ::std::option::Option::Some(m) =
            get_string_prop(cx, req_obj.handle().into(), "method")
        {
            method = m;
        }
        let mut h_val = UndefinedValue();
        JS_GetProperty(
            cx,
            req_obj.handle().into(),
            c"headers".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut h_val,
            },
        );
        if h_val.is_object() {
            headers = parse_headers_init(cx, h_val);
        }
        // Body slots, in the order the Request constructor stores them.
        if let ::std::option::Option::Some(t) =
            get_string_prop(cx, req_obj.handle().into(), "_bodyText")
        {
            body = Some(t.into_bytes());
        } else {
            let mut b_val = UndefinedValue();
            JS_GetProperty(
                cx,
                req_obj.handle().into(),
                c"_bodyBytes".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut b_val,
                },
            );
            if b_val.is_object() {
                body = crate::node_buffer::collect_byte_view(cx, b_val);
            } else {
                let mut blob_val = UndefinedValue();
                JS_GetProperty(
                    cx,
                    req_obj.handle().into(),
                    c"_bodyBlob".as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut blob_val,
                    },
                );
                if blob_val.is_object() {
                    match extract_blob_bytes(cx, blob_val) {
                        Ok(b) => body = b,
                        Err(msg) => {
                            let c_msg = ZBox::from_bytes(msg.as_bytes());
                            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
                            return false;
                        }
                    }
                } else {
                    // FormData slot: web_fetch_classes Request parks the live
                    // object on _bodyFormData; the multipart serialization
                    // (boundary generated here, at send time) happens in
                    // extract_formdata_multipart.
                    let mut fd_val = UndefinedValue();
                    JS_GetProperty(
                        cx,
                        req_obj.handle().into(),
                        c"_bodyFormData".as_ptr(),
                        MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &mut fd_val,
                        },
                    );
                    if fd_val.is_object() {
                        match extract_formdata_multipart(cx, fd_val, &mut headers) {
                            Ok(b) => body = b,
                            Err(msg) => {
                                let c_msg = ZBox::from_bytes(msg.as_bytes());
                                JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
                                return false;
                            }
                        }
                    }
                }
            }
        }
    } else {
        JS_ReportErrorUTF8(
            cx,
            c"fetch requires a string URL or a Request object".as_ptr(),
        );
        return false;
    }

    // ── init overrides (WHATWG: init fields win over the Request base) ──
    if argc > 1 {
        let opts = *args.get(1).ptr;
        if opts.is_object() {
            // BCE-012: root to_object() result — JS_GetProperty can trigger GC
            rooted!(&in(wrapped_cx) let opts_obj = opts.to_object());
            if let ::std::option::Option::Some(m) =
                get_string_prop(cx, opts_obj.handle().into(), "method")
            {
                method = m;
            }
            let mut h_val = UndefinedValue();
            JS_GetProperty(
                cx,
                opts_obj.handle().into(),
                c"headers".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut h_val,
                },
            );
            if h_val.is_object() {
                headers = parse_headers_init(cx, h_val);
            }
            // body: only an explicitly present init.body overrides (null
            // clears it), matching the WHATWG "init wins when present" rule.
            let mut has_body = false;
            JS_HasProperty(
                cx,
                opts_obj.handle().into(),
                c"body".as_ptr(),
                &mut has_body,
            );
            if has_body {
                let mut b_val = UndefinedValue();
                JS_GetProperty(
                    cx,
                    opts_obj.handle().into(),
                    c"body".as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut b_val,
                    },
                );
                match extract_body_bytes(cx, b_val, &mut headers) {
                    Ok(b) => body = b,
                    Err(msg) => {
                        let c_msg = ZBox::from_bytes(msg.as_bytes());
                        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
                        return false;
                    }
                }
            }
            // init.signal wins over the Request base signal (WHATWG
            // "init wins when present" — same rule as body/method/headers).
            {
                let sv = get_val_prop(cx, opts_obj.handle(), "signal");
                if sv.is_object() {
                    signal_val = Some(sv);
                }
            }
        }
    }

    if let ::std::option::Option::Some(pos) = url.find("://") {
        let host_part = &url[pos + 3..];
        let host = host_part
            .split('/')
            .next()
            .unwrap_or(host_part)
            .split(':')
            .next()
            .unwrap_or(host_part);
        if let ::std::result::Result::Err(e) = crate::permission_bridge::check_net(host) {
            let c_msg = ZBox::from_bytes(e.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    }

    // ── Method resolution ──
    // The full bun_http::Method table (IANA method registry: PROPFIND,
    // REPORT, MKCOL, ...). Unknown tokens throw instead of silently falling
    // back to GET — a PROPFIND answered by a GET handler is a misroute, not
    // a degradation. Arbitrary (non-registry) tokens would need a
    // method-as-string plumbing through AsyncHTTP; the closed enum is the
    // wire contract inherited from upstream Bun.
    let method_upper = method.to_uppercase();
    let Some(bun_method) = bun_http::Method::which(method_upper.as_bytes()) else {
        let msg = format!(
            "fetch: HTTP method \"{}\" is not supported by the Bao HTTP wire layer (supported: IANA method registry tokens such as GET/POST/PROPFIND/REPORT)",
            method_upper
        );
        let c_msg = ZBox::from_bytes(msg.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    };

    // ── AbortSignal triage (WHATWG fetch init.signal / Request.signal) ──────
    // Pre-aborted: reject immediately, no request is scheduled. Live signal:
    // wire the cancellation channel and register the abort listener. Shape
    // probe: boolean `aborted` + callable `addEventListener` (holds for the
    // globals.rs shim AND servo's native DOM AbortSignal).
    let mut signal_active: Option<JSVal> = None;
    let mut signal_pre_aborted = false;
    if let Some(sv) = signal_val {
        if sv.is_object() {
            rooted!(&in(wrapped_cx) let sig_obj = sv.to_object());
            if is_abort_signal_shape(cx, sig_obj.handle()) {
                let mut ab_val = UndefinedValue();
                JS_GetProperty(
                    cx,
                    sig_obj.handle().into(),
                    c"aborted".as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut ab_val,
                    },
                );
                if ab_val.is_boolean() && ab_val.to_boolean() {
                    signal_pre_aborted = true;
                } else {
                    signal_active = Some(sv);
                }
            }
        }
    }

    // ── FetchTasklet event-driven: create PENDING Promise, delegate to fetch_async ──
    // @trace REQ-ENG-010 [entity:FetchTasklet] — O(1) OS threads
    rooted!(&in(wrapped_cx) let null_global = ::std::ptr::null_mut::<JSObject>());
    let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, null_global.handle().into());
    if promise.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let promise_val = ObjectValue(promise);

    if signal_pre_aborted {
        // Pre-aborted signal: the fetch never reaches the network. Reject
        // with DOMException AbortError ("The operation was aborted").
        rooted!(&in(wrapped_cx) let promise_obj = promise);
        // SAFETY: cx is live on this thread; promise_obj is a pending Promise.
        unsafe {
            crate::fetch_async::reject_promise_with_abort_error(cx, promise_obj.handle().into());
        }
        args.rval().set(promise_val);
        return true;
    }

    let profile: Option<bao_stealth::StealthProfile> =
        TL_STEALTH_PROFILE.with(|p| p.borrow().clone());

    if let Some(sv) = signal_active {
        // Live signal: wire the cancellation channel (flag → AsyncHTTP
        // Signals.aborted) and register the JS abort listener.
        let abort_id = crate::fetch_async::new_abort_id();
        let flag = Arc::new(AtomicBool::new(false));
        // SAFETY: cx is live on this thread; promise_val is the pending Promise.
        unsafe {
            crate::fetch_async::start_with_signal(
                cx,
                promise_val,
                profile,
                bun_method,
                url,
                headers,
                body,
                crate::fetch_async::AbortRequest {
                    id: abort_id,
                    flag: ::std::sync::Arc::clone(&flag),
                },
            );
            register_abort_listener(cx, sv, abort_id);
        }
        args.rval().set(promise_val);
        return true;
    }

    // SAFETY: cx is live on this thread; promise_val is the pending Promise.
    unsafe {
        crate::fetch_async::start(cx, promise_val, profile, bun_method, url, headers, body);
    }

    args.rval().set(promise_val);
    true
}

// ── AbortSignal listener wiring ─────────────────────────────────────────────

/// Native `abort` event trampoline. The abort id is stamped onto the
/// function object itself (`_abortId`, permanent + readonly); the callee is
/// recovered via `args.calleev()` so no global lookup is needed (works in
/// any realm, page or CLI).
#[allow(non_snake_case)]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn bao_abort_listener_native(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let callee_v = args.calleev();
    if callee_v.is_object() {
        let mut wrapped_cx =
            mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        // BCE-012: root the callee across the property read (can trigger GC)
        rooted!(&in(cx_ref) let callee_obj = callee_v.to_object());
        let mut id_val = UndefinedValue();
        JS_GetProperty(
            cx,
            callee_obj.handle().into(),
            c"_abortId".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut id_val,
            },
        );
        if id_val.is_int32() {
            crate::fetch_async::trigger_abort(id_val.to_int32() as u32);
        }
    }
    args.rval().set(UndefinedValue());
    true
}

/// Register the abort listener on a live AbortSignal:
/// `signal.addEventListener('abort', trampoline)`. The trampoline carries
/// the abort id; when the signal fires it calls
/// [`crate::fetch_async::trigger_abort`], which sets the shared flag and
/// schedules the HTTPThread shutdown for the in-flight request.
///
/// # Safety
///
/// `cx` must be a live `JSContext*` on the current thread; `signal_val`
/// must be an object value protected from GC by the caller's stack frame.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn register_abort_listener(cx: *mut JSContext, signal_val: JSVal, abort_id: u32) {
    unsafe {
        let mut wrapped_cx =
            mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        // BCE-012: root the signal across the addEventListener call
        rooted!(&in(cx_ref) let signal_obj = signal_val.to_object());

        let listener_fn = JS_NewFunction(
            cx,
            Some(bao_abort_listener_native),
            1,
            0,
            c"__baoFetchAbort".as_ptr(),
        );
        let listener_fn = JS_NewFunction(
            cx,
            Some(bao_abort_listener_native),
            1,
            0,
            c"__baoFetchAbort".as_ptr(),
        );
        if listener_fn.is_null() {
            return;
        }
        let listener_obj = JS_GetFunctionObject(listener_fn);
        if listener_obj.is_null() {
            return;
        }
        // BCE-012: root the listener function object across the calls below
        rooted!(&in(cx_ref) let listener = listener_obj);

        // Stamp the abort id as the listener's identity (permanent +
        // readonly: JS must not be able to repoint one fetch's abort at
        // another's channel).
        rooted!(&in(cx_ref) let id_val = Int32Value(abort_id as i32));
        JS_DefineProperty(
            cx,
            listener.handle().into(),
            c"_abortId".as_ptr(),
            id_val.handle().into(),
            (JSPROP_PERMANENT | JSPROP_READONLY) as u32,
        );

        // signal.addEventListener('abort', listener)
        let c_type = ZBox::from_bytes(b"abort");
        let type_js = JS_NewStringCopyZ(cx, c_type.as_ptr());
        if type_js.is_null() {
            return;
        }
        rooted!(&in(cx_ref) let type_val = StringValue(&*type_js));
        rooted!(&in(cx_ref) let listener_val = ObjectValue(listener.get()));
        let call_args_arr = [type_val.get(), listener_val.get()];
        let call_args = HandleValueArray {
            length_: call_args_arr.len(),
            elements_: call_args_arr.as_ptr(),
        };
        let mut rval = UndefinedValue();
        JS_CallFunctionName(
            cx,
            signal_obj.handle().into(),
            c"addEventListener".as_ptr(),
            &call_args,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut rval,
            },
        );
    }
}

/// AbortSignal structural probe: boolean `aborted` property + callable
/// `addEventListener` (the globals.rs shim has both as own/prototype
/// members; servo's DOM AbortSignal satisfies the same surface).
///
/// # Safety
///
/// `cx` must be a live `JSContext*` on the current thread; `obj` must be
/// GC-protected by the caller.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn is_abort_signal_shape(
    cx: *mut JSContext,
    obj: mozjs::rust::Handle<*mut JSObject>,
) -> bool {
    unsafe {
        let ab_val = get_val_prop(cx, obj, "aborted");
        if !ab_val.is_boolean() {
            return false;
        }
        let ael_val = get_val_prop(cx, obj, "addEventListener");
        ael_val.is_object() && IsCallable(ael_val.to_object())
    }
}

// ── fetch input/init value helpers ─────────────────────────────────────────

/// Read a string-valued property off `obj`; `None` when absent or non-string.
///
/// # Safety
///
/// `cx` must be a live `JSContext*` on the current thread; `obj` must be
/// protected from GC by the caller's stack frame.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn get_string_prop(
    cx: *mut JSContext,
    obj: mozjs::rust::Handle<*mut JSObject>,
    name: &str,
) -> Option<String> {
    unsafe {
        let c_name = ZBox::from_bytes(name.as_bytes());
        let mut v = UndefinedValue();
        JS_GetProperty(
            cx,
            obj.into(),
            c_name.as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut v,
            },
        );
        if v.is_string() {
            Some(crate::js_to_rust_string(cx, v))
        } else {
            None
        }
    }
}

/// True when `constructor_val` is (pointer-identical to) the global
/// `global_name` constructor — constructor identity check for FormData /
/// Blob inputs (JS-defined classes whose instances link back to the
/// constructor through the prototype chain).
///
/// NOT usable for URLSearchParams: that constructor is native and its
/// instances are plain JSClass objects with own method props and no
/// prototype/constructor linkage, so `.constructor` resolves to Object —
/// use [`is_url_search_params_shape`] for those.
///
/// # Safety
///
/// `cx` must be a live `JSContext*` on the current thread; `constructor_val`
/// must be protected from GC by the caller's stack frame.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn is_global_ctor(cx: *mut JSContext, constructor_val: JSVal, global_name: &str) -> bool {
    unsafe {
        if !constructor_val.is_object() {
            return false;
        }
        let global = mozjs_sys::jsapi::JS::CurrentGlobalOrNull(cx);
        if global.is_null() {
            return false;
        }
        let wrapped_cx =
            mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        // BCE-012: root the global across the property read (can trigger GC)
        rooted!(&in(wrapped_cx) let global_rooted = global);
        let c_name = ZBox::from_bytes(global_name.as_bytes());
        let mut g_val = UndefinedValue();
        JS_GetProperty(
            cx,
            global_rooted.handle().into(),
            c_name.as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut g_val,
            },
        );
        g_val.is_object() && g_val.to_object() == constructor_val.to_object()
    }
}

/// URLSearchParams structural probe (mirrors `_bao_is_urlsearchparams` in
/// web_fetch_classes.rs). The runtime's URLSearchParams is a native
/// constructor whose instances are plain JSClass objects with own method
/// props and no prototype/constructor linkage — `instanceof` against the
/// prototype-less constructor throws (JSMSG_BAD_PROTOTYPE) and
/// `.constructor` identity resolves to Object. The method surface
/// append+getAll+entries+forEach is the discriminator: FormData lacks
/// forEach/entries, Map lacks append/getAll, Blob lacks all four.
///
/// # Safety
///
/// `cx` must be a live `JSContext*` on the current thread; `obj` must be
/// protected from GC by the caller's stack frame.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn is_url_search_params_shape(
    cx: *mut JSContext,
    obj: mozjs::rust::Handle<*mut JSObject>,
) -> bool {
    unsafe {
        for name in ["append", "getAll", "entries", "forEach"] {
            let c_name = ZBox::from_bytes(name.as_bytes());
            let mut v = UndefinedValue();
            JS_GetProperty(
                cx,
                obj.into(),
                c_name.as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut v,
                },
            );
            if !v.is_object() || !IsCallable(v.to_object()) {
                return false;
            }
        }
        true
    }
}

/// Concatenate a Bao Blob's `_chunks` (array of Uint8Array) into owned bytes.
///
/// # Safety
///
/// `cx` must be a live `JSContext*`; `blob_val` must be a protected object
/// value whose `_chunks` (when present) is a JS array of byte views.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn extract_blob_bytes(
    cx: *mut JSContext,
    blob_val: JSVal,
) -> ::std::result::Result<Option<Vec<u8>>, String> {
    unsafe {
        let wrapped_cx =
            mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        // BCE-012: root to_object() result — JS_GetProperty can trigger GC
        rooted!(&in(wrapped_cx) let blob = blob_val.to_object());
        let mut chunks_val = UndefinedValue();
        JS_GetProperty(
            cx,
            blob.handle().into(),
            c"_chunks".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut chunks_val,
            },
        );
        if !chunks_val.is_object() {
            // Blob-like without synchronous byte storage (e.g. a DOM Blob
            // from another realm whose bytes live behind an async
            // arrayBuffer()). Fail closed — no empty-body substitute.
            return Err("fetch: Blob bodies without synchronous byte storage are not supported yet (no streaming request-body infrastructure)".to_string());
        }
        // BCE-012: root to_object() result — JS_GetElement can trigger GC
        rooted!(&in(wrapped_cx) let chunks = chunks_val.to_object());
        let mut is_array = false;
        rooted!(&in(wrapped_cx) let arr_probe = chunks_val);
        IsArrayObject(cx, arr_probe.handle().into(), &mut is_array);
        if !is_array {
            return Err("fetch: Blob bodies without synchronous byte storage are not supported yet (no streaming request-body infrastructure)".to_string());
        }
        let mut len_val = UndefinedValue();
        JS_GetProperty(
            cx,
            chunks.handle().into(),
            c"length".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut len_val,
            },
        );
        let len = if len_val.is_int32() && len_val.to_int32() > 0 {
            len_val.to_int32() as usize
        } else {
            0
        };
        let mut out: Vec<u8> = Vec::new();
        for i in 0..len as u32 {
            let mut el = UndefinedValue();
            JS_GetElement(
                cx,
                chunks.handle().into(),
                i,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut el,
                },
            );
            match crate::node_buffer::collect_byte_view(cx, el) {
                ::std::option::Option::Some(bytes) => out.extend_from_slice(&bytes),
                ::std::option::Option::None => {
                    return Err("fetch: Blob chunk is not a byte view".to_string());
                }
            }
        }
        Ok(Some(out))
    }
}

// ── FormData multipart serialization ────────────────────────────────────────
//
// Mirrors upstream Bun `Blob.zig fromDOMFormData` byte-for-byte:
//   boundary:   "----WebKitFormBoundary" + hex of 16 random bytes (Bun prints
//               its VM's nextUUID() bytes as hex; zero-padded here so the
//               boundary is always 22+32 chars)
//   per entry:  "--{b}\r\n"
//               "Content-Disposition: form-data; name=\"{name}\""
//               string:  "\"\r\n\r\n{value}\r\n"
//               file:    "\"; filename=\"{filename}\"\r\n"
//                        "Content-Type: {ct}\r\n\r\n{bytes}\r\n"
//               (ct = Blob.type when non-empty, else application/octet-stream)
//   terminator: "--{b}--\r\n"
//   header:     "multipart/form-data; boundary={b}"
// Upstream Bun performs no quote escaping in name/filename — aligned.
// Default filename follows the WHATWG/servo rule (dom/formdata.rs
// create_an_entry): explicit filename > File.name > "blob".

/// One classified FormData entry value.
enum MultipartValue {
    /// String field.
    Text(String),
    /// Blob/File field: filename, per-part content-type, raw bytes.
    File {
        filename: String,
        content_type: String,
        bytes: Vec<u8>,
    },
}

/// Generate the multipart boundary (WebKit-style, upstream Bun shape).
fn generate_multipart_boundary() -> ::std::result::Result<String, String> {
    let mut raw = [0u8; 16];
    getrandom::fill(&mut raw)
        .map_err(|e| format!("fetch: multipart boundary randomness unavailable: {}", e))?;
    let mut s = String::with_capacity("----WebKitFormBoundary".len() + 32);
    s.push_str("----WebKitFormBoundary");
    for b in raw {
        s.push_str(&format!("{:02x}", b));
    }
    Ok(s)
}

/// Pure encoder: entries + boundary → multipart/form-data body bytes.
/// Kept free of JS interaction so the wire format is unit-testable.
fn encode_multipart(entries: &[(String, MultipartValue)], boundary: &str) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    for (name, value) in entries {
        out.extend_from_slice(b"--");
        out.extend_from_slice(boundary.as_bytes());
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(b"Content-Disposition: form-data; name=\"");
        out.extend_from_slice(name.as_bytes());
        match value {
            MultipartValue::Text(text) => {
                out.extend_from_slice(b"\"\r\n\r\n");
                out.extend_from_slice(text.as_bytes());
            }
            MultipartValue::File {
                filename,
                content_type,
                bytes,
            } => {
                out.extend_from_slice(b"\"; filename=\"");
                out.extend_from_slice(filename.as_bytes());
                out.extend_from_slice(b"\"\r\n");
                out.extend_from_slice(b"Content-Type: ");
                out.extend_from_slice(content_type.as_bytes());
                out.extend_from_slice(b"\r\n\r\n");
                out.extend_from_slice(bytes);
            }
        }
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"--");
    out.extend_from_slice(boundary.as_bytes());
    out.extend_from_slice(b"--\r\n");
    out
}

/// Convert an arbitrary JSVal to a Rust String via ToString (names, string
/// field values, filenames).
///
/// # Safety
///
/// `cx` must be a live `JSContext*` on the current thread; `v` must be
/// protected from GC by the caller's stack frame.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn val_to_rust_string(
    cx: *mut JSContext,
    v: JSVal,
) -> ::std::result::Result<String, String> {
    unsafe {
        if v.is_string() {
            return Ok(crate::js_to_rust_string(cx, v));
        }
        if v.is_null_or_undefined() {
            return Ok(String::new());
        }
        let mut wrapped_cx =
            mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        rooted!(&in(wrapped_cx) let root = v);
        let jsstr = mozjs::rust::ToString(&mut wrapped_cx, root.handle());
        if jsstr.is_null() {
            return Err(
                "fetch: FormData entry name/value could not be converted to string".to_string(),
            );
        }
        let str_val = StringValue(&*jsstr);
        Ok(crate::js_to_rust_string(cx, str_val))
    }
}

/// Read a property off `obj` as a raw JSVal.
///
/// # Safety
///
/// `cx` must be a live `JSContext*`; `obj` must be GC-protected by the caller.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn get_val_prop(
    cx: *mut JSContext,
    obj: mozjs::rust::Handle<*mut JSObject>,
    name: &str,
) -> JSVal {
    unsafe {
        let c_name = ZBox::from_bytes(name.as_bytes());
        let mut v = UndefinedValue();
        JS_GetProperty(
            cx,
            obj.into(),
            c_name.as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut v,
            },
        );
        v
    }
}

/// FormData structural probe: `_data` array + callable getAll (mirrors
/// `_bao_is_formdata` in web_fetch_classes.rs). Runs BEFORE the
/// URLSearchParams probe — FormData's WHATWG iteration surface
/// (entries/forEach) also satisfies that predicate.
///
/// # Safety
///
/// `cx` must be a live `JSContext*`; `obj` must be GC-protected by the caller.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn is_formdata_shape(cx: *mut JSContext, obj: mozjs::rust::Handle<*mut JSObject>) -> bool {
    unsafe {
        let data_val = get_val_prop(cx, obj, "_data");
        if !data_val.is_object() {
            return false;
        }
        let wrapped_cx =
            mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        rooted!(&in(wrapped_cx) let data_probe = data_val);
        let mut is_array = false;
        IsArrayObject(cx, data_probe.handle().into(), &mut is_array);
        if !is_array {
            return false;
        }
        let get_all = get_val_prop(cx, obj, "getAll");
        get_all.is_object() && IsCallable(get_all.to_object())
    }
}

/// Serialize a FormData object (globals.rs class: `_data` array of
/// `{ name, value, filename }` records) into multipart/form-data bytes and
/// default the Content-Type header. Blob/File values are read through their
/// synchronous `_chunks` storage; anything else fails closed.
///
/// # Safety
///
/// `cx` must be a live `JSContext*`; `formdata_val` must be protected from
/// GC by the caller's stack frame. `headers` receives the defaulted
/// content-type.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn extract_formdata_multipart(
    cx: *mut JSContext,
    formdata_val: JSVal,
    headers: &mut Vec<(String, String)>,
) -> ::std::result::Result<Option<Vec<u8>>, String> {
    unsafe {
        let wrapped_cx =
            mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        // BCE-012: root to_object() result — the JS reads below can trigger GC
        rooted!(&in(wrapped_cx) let form = formdata_val.to_object());

        let data_val = get_val_prop(cx, form.handle(), "_data");
        if !data_val.is_object() {
            return Err("fetch: FormData body has no _data entries array".to_string());
        }
        // BCE-012: root the entries array across element reads
        rooted!(&in(wrapped_cx) let data = data_val.to_object());
        let mut is_array = false;
        rooted!(&in(wrapped_cx) let arr_probe = data_val);
        IsArrayObject(cx, arr_probe.handle().into(), &mut is_array);
        if !is_array {
            return Err("fetch: FormData body has no _data entries array".to_string());
        }
        let len_val = get_val_prop(cx, data.handle(), "length");
        let len = if len_val.is_int32() && len_val.to_int32() > 0 {
            len_val.to_int32() as usize
        } else {
            0
        };

        let boundary = generate_multipart_boundary()?;
        let mut entries: Vec<(String, MultipartValue)> = Vec::new();
        for i in 0..len as u32 {
            let mut el = UndefinedValue();
            JS_GetElement(
                cx,
                data.handle().into(),
                i,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut el,
                },
            );
            if !el.is_object() {
                return Err("fetch: FormData entry is not an object".to_string());
            }
            // BCE-012: root the entry across its property reads
            rooted!(&in(wrapped_cx) let entry = el.to_object());
            let name_val = get_val_prop(cx, entry.handle(), "name");
            let name = val_to_rust_string(cx, name_val)?;
            let value_val = get_val_prop(cx, entry.handle(), "value");

            if value_val.is_object() {
                // Blob/File field. Filename: explicit > File.name > "blob".
                // Content-type: Blob.type > application/octet-stream.
                rooted!(&in(wrapped_cx) let blob = value_val.to_object());
                let filename_val = get_val_prop(cx, entry.handle(), "filename");
                let mut filename = if filename_val.is_string() {
                    ::std::option::Option::Some(crate::js_to_rust_string(cx, filename_val))
                } else {
                    ::std::option::Option::None
                };
                if filename.as_deref().map_or(true, |f| f.is_empty()) {
                    let name_prop = get_val_prop(cx, blob.handle(), "name");
                    filename = if name_prop.is_string() {
                        ::std::option::Option::Some(crate::js_to_rust_string(cx, name_prop))
                    } else {
                        ::std::option::Option::Some("blob".to_string())
                    };
                }
                let type_prop = get_val_prop(cx, blob.handle(), "type");
                let content_type = if type_prop.is_string() {
                    let t = crate::js_to_rust_string(cx, type_prop);
                    if t.is_empty() {
                        "application/octet-stream".to_string()
                    } else {
                        t
                    }
                } else {
                    "application/octet-stream".to_string()
                };
                let bytes = extract_blob_bytes(cx, value_val)?
                    .ok_or_else(|| {
                        "fetch: FormData file entry without synchronous byte storage is not supported yet (no streaming request-body infrastructure)".to_string()
                    })?;
                entries.push((
                    name,
                    MultipartValue::File {
                        filename: filename.unwrap_or_else(|| "blob".to_string()),
                        content_type,
                        bytes,
                    },
                ));
            } else {
                entries.push((
                    name,
                    MultipartValue::Text(val_to_rust_string(cx, value_val)?),
                ));
            }
        }

        let body = encode_multipart(&entries, &boundary);
        let has_ct = headers
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case("content-type"));
        if !has_ct {
            headers.push((
                "Content-Type".to_string(),
                format!("multipart/form-data; boundary={}", boundary),
            ));
        }
        Ok(Some(body))
    }
}

/// Extract fetch `init.body` bytes. Accepted forms: string, byte views
/// (Buffer/Uint8Array/TypedArray/DataView/ArrayBuffer), Bao Blob (`_chunks`
/// storage), URLSearchParams (serialized; defaults
/// `application/x-www-form-urlencoded;charset=UTF-8` when no content-type
/// header is set), FormData (multipart/form-data with a generated boundary).
/// Anything else fails closed with an explicit error — silently dropping a
/// body turns a POST into an empty POST.
///
/// # Safety
///
/// `cx` must be a live `JSContext*`; `body_val` must be protected from GC by
/// the caller's stack frame. `headers` receives the defaulted content-type.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn extract_body_bytes(
    cx: *mut JSContext,
    body_val: JSVal,
    headers: &mut Vec<(String, String)>,
) -> ::std::result::Result<Option<Vec<u8>>, String> {
    unsafe {
        if body_val.is_null_or_undefined() {
            return Ok(None);
        }
        if body_val.is_string() {
            return Ok(Some(crate::js_to_rust_string(cx, body_val).into_bytes()));
        }
        if !body_val.is_object() {
            return Err(format!(
                "fetch: unsupported body type (expected string / BufferSource / Blob / URLSearchParams / FormData)"
            ));
        }
        let wrapped_cx =
            mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        // BCE-012: root to_object() result — the JS calls below can trigger GC
        rooted!(&in(wrapped_cx) let obj = body_val.to_object());

        // Byte views: Buffer / Uint8Array / TypedArray / DataView / ArrayBuffer.
        if let ::std::option::Option::Some(bytes) =
            crate::node_buffer::collect_byte_view(cx, body_val)
        {
            return Ok(Some(bytes));
        }

        // Constructor-identity probe for the class forms.
        let mut ctor_val = UndefinedValue();
        JS_GetProperty(
            cx,
            obj.handle().into(),
            c"constructor".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut ctor_val,
            },
        );

        // FormData → multipart/form-data. BEFORE the URLSearchParams probe:
        // FormData's WHATWG iteration surface (entries/forEach) also
        // satisfies that predicate.
        if is_formdata_shape(cx, obj.handle()) || is_global_ctor(cx, ctor_val, "FormData") {
            return extract_formdata_multipart(cx, body_val, headers);
        }

        // URLSearchParams → serialize via toString(), default the content-type.
        if is_url_search_params_shape(cx, obj.handle()) {
            // BCE-012: root the object across the toString call
            let mut s_val = UndefinedValue();
            let called = JS_CallFunctionName(
                cx,
                obj.handle().into(),
                c"toString".as_ptr(),
                &HandleValueArray::empty(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut s_val,
                },
            );
            if !called || !s_val.is_string() {
                return Err("fetch: URLSearchParams body could not be serialized".to_string());
            }
            let has_ct = headers
                .iter()
                .any(|(n, _)| n.eq_ignore_ascii_case("content-type"));
            if !has_ct {
                headers.push((
                    "Content-Type".to_string(),
                    "application/x-www-form-urlencoded;charset=UTF-8".to_string(),
                ));
            }
            return Ok(Some(crate::js_to_rust_string(cx, s_val).into_bytes()));
        }

        // Blob-ish (numeric size + callable arrayBuffer). The Bao Blob stores
        // `_chunks` synchronously; realm-foreign Blobs fail closed inside.
        let mut size_val = UndefinedValue();
        JS_GetProperty(
            cx,
            obj.handle().into(),
            c"size".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut size_val,
            },
        );
        let mut ab_val = UndefinedValue();
        JS_GetProperty(
            cx,
            obj.handle().into(),
            c"arrayBuffer".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut ab_val,
            },
        );
        if size_val.is_number() && ab_val.is_object() && IsCallable(ab_val.to_object()) {
            return extract_blob_bytes(cx, body_val);
        }

        Err("fetch: unsupported body type (expected string / BufferSource / Blob / URLSearchParams / FormData; streams are not supported)".to_string())
    }
}

// ── WHATWG fetch init.headers parsing (BCE-20260814-FETCH-H) ──────────────

/// Safety valve: a hostile/broken iterator that never reports `done` must
/// not hang the JS thread. WHATWG has no hard limit but a 1024-entry cap is
/// far beyond any legitimate header list.
const MAX_HEADER_ENTRIES: usize = 1024;

/// Parse WHATWG fetch `init.headers` into header entries.
///
/// Accepted forms (WHATWG Fetch spec Headers-fill):
/// 1. Sequence of pairs: `[["name","value"], ...]` or `[{name, value}, ...]`.
/// 2. Headers-like object with callable `entries()` (servo DOM Headers, the
///    orphaned web_fetch_classes Headers) — drained through the JS iterator
///    protocol (`next()` until `done`).
/// 3. Record `{ "name": "value" }` — also covers this module's `Headers`
///    class, whose entries are own enumerable string-valued data props; its
///    installed `get`/`set`/`has` method props are skipped by the
///    string-value filter.
///
/// # Safety
///
/// `cx` must be a live `JSContext*` on the current thread; `headers_val`
/// must be protected from GC by the caller's stack frame.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn parse_headers_init(cx: *mut JSContext, headers_val: JSVal) -> Vec<(String, String)> {
    unsafe {
        let mut out: Vec<(String, String)> = Vec::new();
        if !headers_val.is_object() {
            return out;
        }
        let wrapped_cx =
            mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        // BCE-012: root to_object() result — the JS calls below can trigger GC
        rooted!(&in(wrapped_cx) let obj = headers_val.to_object());

        // Form 1: sequence of pairs
        let mut is_array = false;
        rooted!(&in(wrapped_cx) let obj_val = headers_val);
        IsArrayObject(cx, obj_val.handle().into(), &mut is_array);
        if is_array {
            let mut len_val = UndefinedValue();
            JS_GetProperty(
                cx,
                obj.handle().into(),
                c"length".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut len_val,
                },
            );
            let len = if len_val.is_int32() && len_val.to_int32() > 0 {
                (len_val.to_int32() as usize).min(MAX_HEADER_ENTRIES)
            } else {
                0
            };
            for i in 0..len as u32 {
                let mut el_val = UndefinedValue();
                JS_GetElement(
                    cx,
                    obj.handle().into(),
                    i,
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut el_val,
                    },
                );
                if let Some(pair) = parse_header_entry(cx, el_val) {
                    out.push(pair);
                }
            }
            return out;
        }

        // Form 2: Headers-like with callable entries() — iterator protocol.
        // JS_GetProperty walks the prototype chain, so prototype methods
        // (servo DOM Headers / web_fetch_classes) resolve here too.
        let mut entries_val = UndefinedValue();
        JS_GetProperty(
            cx,
            obj.handle().into(),
            c"entries".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut entries_val,
            },
        );
        if entries_val.is_object() && IsCallable(entries_val.to_object()) {
            // BCE-012: root the entries function across the call
            rooted!(&in(wrapped_cx) let _entries_fn = entries_val.to_object());
            let mut iter_val = UndefinedValue();
            let called = JS_CallFunctionName(
                cx,
                obj.handle().into(),
                c"entries".as_ptr(),
                &HandleValueArray::empty(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut iter_val,
                },
            );
            if called && iter_val.is_object() {
                // BCE-012: root the iterator across the next() calls
                rooted!(&in(wrapped_cx) let iter = iter_val.to_object());
                loop {
                    if out.len() >= MAX_HEADER_ENTRIES {
                        break;
                    }
                    let mut next_val = UndefinedValue();
                    let advanced = JS_CallFunctionName(
                        cx,
                        iter.handle().into(),
                        c"next".as_ptr(),
                        &HandleValueArray::empty(),
                        MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &mut next_val,
                        },
                    );
                    if !advanced || !next_val.is_object() {
                        break;
                    }
                    // BCE-012: root the iterator result across property reads
                    rooted!(&in(wrapped_cx) let res = next_val.to_object());
                    let mut done_val = UndefinedValue();
                    JS_GetProperty(
                        cx,
                        res.handle().into(),
                        c"done".as_ptr(),
                        MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &mut done_val,
                        },
                    );
                    if done_val.is_boolean() && done_val.to_boolean() {
                        break;
                    }
                    let mut pair_val = UndefinedValue();
                    JS_GetProperty(
                        cx,
                        res.handle().into(),
                        c"value".as_ptr(),
                        MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &mut pair_val,
                        },
                    );
                    if let Some(pair) = parse_header_entry(cx, pair_val) {
                        out.push(pair);
                    }
                }
            }
            return out;
        }

        // Form 3: record / this module's Headers class — own enumerable
        // string-keyed props with string values. The string-value filter
        // skips the class's get/set/has method props (they are functions).
        let mut ids = mozjs::rust::IdVector::new(cx);
        if GetPropertyKeys(cx, obj.handle().into(), JSITER_OWNONLY, ids.handle_mut()) {
            for jsid in &*ids {
                if !jsid.is_string() {
                    continue;
                }
                let key_str_ptr = jsid.to_string();
                if key_str_ptr.is_null() {
                    continue;
                }
                let key =
                    unsafe_jsstr_to_string(cx, ::std::ptr::NonNull::new_unchecked(key_str_ptr));
                let c_key = ZBox::from_bytes(key.as_bytes());
                let mut v_val = UndefinedValue();
                JS_GetProperty(
                    cx,
                    obj.handle().into(),
                    c_key.as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut v_val,
                    },
                );
                if v_val.is_string() {
                    out.push((key, crate::js_to_rust_string(cx, v_val)));
                }
            }
        }
        out
    }
}

/// Parse one `[name, value]` / `{name, value}` pair — an element of the
/// sequence form, or the `value` of a Headers-like iterator result.
///
/// # Safety
///
/// `cx` must be a live `JSContext*` on the current thread; `pair_val` must
/// be protected from GC by the caller's stack frame.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn parse_header_entry(cx: *mut JSContext, pair_val: JSVal) -> Option<(String, String)> {
    unsafe {
        if !pair_val.is_object() {
            return None;
        }
        let wrapped_cx =
            mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        // BCE-012: root to_object() result — JS_GetElement/JS_GetProperty can trigger GC
        rooted!(&in(wrapped_cx) let pair = pair_val.to_object());

        let mut is_array = false;
        rooted!(&in(wrapped_cx) let pair_root = pair_val);
        IsArrayObject(cx, pair_root.handle().into(), &mut is_array);
        if is_array {
            // ["name", "value"]
            let mut n_val = UndefinedValue();
            let mut v_val = UndefinedValue();
            JS_GetElement(
                cx,
                pair.handle().into(),
                0,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut n_val,
                },
            );
            JS_GetElement(
                cx,
                pair.handle().into(),
                1,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut v_val,
                },
            );
            if n_val.is_string() && v_val.is_string() {
                return Some((
                    crate::js_to_rust_string(cx, n_val),
                    crate::js_to_rust_string(cx, v_val),
                ));
            }
            return None;
        }

        // { name, value }
        let mut n_val = UndefinedValue();
        let mut v_val = UndefinedValue();
        JS_GetProperty(
            cx,
            pair.handle().into(),
            c"name".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut n_val,
            },
        );
        JS_GetProperty(
            cx,
            pair.handle().into(),
            c"value".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut v_val,
            },
        );
        if n_val.is_string() && v_val.is_string() {
            return Some((
                crate::js_to_rust_string(cx, n_val),
                crate::js_to_rust_string(cx, v_val),
            ));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{MultipartValue, encode_multipart, generate_multipart_boundary};

    // ── REQ-SEC-001: CORS Bypass Unit Tests ──────────────────────────────
    // @trace TEST-SEC-001 [req:REQ-SEC-001] [level:unit]

    /// REQ-SEC-001: fetch global is installed on page realm via install_all_native.
    #[test]
    fn cors_bypass_fetch_global_installed_for_page() {
        let source = include_str!("fetch_api.rs");
        assert!(
            source.contains("pub fn install_fetch_global"),
            "REQ-SEC-001: install_fetch_global must be pub for page realm installation"
        );
    }

    /// REQ-SEC-001: fetch delegates to fetch_async::start (event-driven, no CORS).
    #[test]
    fn cors_bypass_fetch_uses_event_driven_no_cors() {
        let source = include_str!("fetch_api.rs");
        assert!(
            source.contains("crate::fetch_async::start"),
            "REQ-SEC-001: fetch must delegate to fetch_async::start"
        );
        // Split string literal to avoid self-match in include_str source
        let forbidden_cors = ["cors", "_check"].join("");
        assert!(
            !source.contains(&forbidden_cors),
            "REQ-SEC-001 REGRESSION: fetch must NOT contain cors check"
        );
        // Split string literal to avoid self-match in include_str source
        let forbidden_cors_preflight = ["Access-Control", "-Request-Method"].join("");
        assert!(
            !source.contains(&forbidden_cors_preflight),
            "REQ-SEC-001 REGRESSION: fetch must NOT send CORS preflight headers"
        );
    }

    /// BCE-20260619-010: old thread::spawn/drain code is removed.
    #[test]
    fn bce_010_no_spawn_or_drain() {
        let source = include_str!("fetch_api.rs");
        // Split string literals to avoid self-match in include_str source
        let forbidden_spawn = ["spawn", "_fetch_worker"].join("");
        let forbidden_drain = ["drain", "_pending_fetches"].join("");
        let forbidden_blocking = ["do_fetch", "_blocking"].join("");
        assert!(
            !source.contains(&forbidden_spawn),
            "BCE-010 REGRESSION: spawn fetch worker must be removed"
        );
        assert!(
            !source.contains(&forbidden_drain),
            "BCE-010 REGRESSION: drain pending fetches must be removed"
        );
        assert!(
            !source.contains(&forbidden_blocking),
            "BCE-010 REGRESSION: do fetch blocking must be removed"
        );
    }

    /// BCE-20260814-FETCH-H: init.headers must be parsed (three WHATWG
    /// forms), not dropped. Split string literals to avoid self-match in
    /// include_str source.
    #[test]
    fn bce_fetch_h_headers_not_dropped() {
        let source = include_str!("fetch_api.rs");
        let parse_call = ["parse_", "headers_init"].join("");
        assert!(
            source.contains(&parse_call),
            "BCE-20260814-FETCH-H REGRESSION: fetch_fn must parse init.headers"
        );
        let dropped_form = ["let headers: Vec<(String, String)> = ", "Vec::new();"].join("");
        assert!(
            !source.contains(&dropped_form),
            "BCE-20260814-FETCH-H REGRESSION: init.headers must not be dropped as an empty Vec"
        );
        // All three WHATWG init forms must be handled.
        let seq_form = ["[\"name\",\"value\"]"].join("");
        assert!(
            source.contains(&seq_form),
            "BCE-20260814-FETCH-H: sequence pair form must be documented/parseable"
        );
    }

    // ── FormData multipart serialization unit tests ─────────────────────
    // @trace TEST-ENG-FETCH-FORMDATA [req:REQ-ENG-001 REQ-ENG-006] [level:unit]

    const TEST_BOUNDARY: &str = "----WebKitFormBoundary0123456789abcdef0123456789abcdef";

    /// Upstream Bun Blob.zig fromDOMFormData wire format: per-entry framing,
    /// Content-Disposition, per-file Content-Type, terminator.
    #[test]
    fn multipart_encode_text_and_file_entries() {
        let entries = vec![
            (
                "field".to_string(),
                MultipartValue::Text("hello world".to_string()),
            ),
            (
                "upload".to_string(),
                MultipartValue::File {
                    filename: "a.txt".to_string(),
                    content_type: "text/plain".to_string(),
                    bytes: b"file-bytes".to_vec(),
                },
            ),
            (
                "noType".to_string(),
                MultipartValue::File {
                    filename: "blob".to_string(),
                    content_type: "application/octet-stream".to_string(),
                    bytes: vec![0u8, 1, 2],
                },
            ),
        ];
        let body = encode_multipart(&entries, TEST_BOUNDARY);
        let text = String::from_utf8_lossy(&body).to_string();
        let expected = concat!(
            "------WebKitFormBoundary0123456789abcdef0123456789abcdef\r\n",
            "Content-Disposition: form-data; name=\"field\"\r\n",
            "\r\n",
            "hello world\r\n",
            "------WebKitFormBoundary0123456789abcdef0123456789abcdef\r\n",
            "Content-Disposition: form-data; name=\"upload\"; filename=\"a.txt\"\r\n",
            "Content-Type: text/plain\r\n",
            "\r\n",
            "file-bytes\r\n",
            "------WebKitFormBoundary0123456789abcdef0123456789abcdef\r\n",
            "Content-Disposition: form-data; name=\"noType\"; filename=\"blob\"\r\n",
            "Content-Type: application/octet-stream\r\n",
            "\r\n",
        );
        assert!(
            text.starts_with(expected),
            "multipart per-entry framing mismatch:\n{}",
            text
        );
        assert!(
            text.ends_with("------WebKitFormBoundary0123456789abcdef0123456789abcdef--\r\n"),
            "multipart terminator missing:\n{}",
            text
        );
        // Binary file bytes survive verbatim (no lossy transform).
        assert!(body.windows(3).any(|w| w == [0u8, 1, 2]));
    }

    /// Empty FormData → boundary terminator only (RFC 7578 permits an empty
    /// parts list; Bun emits the same shape).
    #[test]
    fn multipart_encode_empty_formdata() {
        let body = encode_multipart(&[], TEST_BOUNDARY);
        assert_eq!(
            body,
            format!("{}--\r\n", format!("--{}", TEST_BOUNDARY)).into_bytes()
        );
    }

    /// Boundary uniqueness: two generations must differ (random 128-bit).
    #[test]
    fn multipart_boundary_unique_per_generation() {
        let a = generate_multipart_boundary().expect("boundary gen");
        let b = generate_multipart_boundary().expect("boundary gen");
        assert_ne!(a, b, "multipart boundary repeated across generations");
        assert!(a.starts_with("----WebKitFormBoundary"));
        assert_eq!(a.len(), 22 + 32, "boundary must be prefix + 32 hex chars");
        assert!(
            a["----WebKitFormBoundary".len()..]
                .chars()
                .all(|c| c.is_ascii_hexdigit()),
            "boundary suffix must be hex"
        );
    }
}
