// @trace REQ-ENG-007
use ::std::cell::{Cell, RefCell};
use ::std::collections::VecDeque;
use ::std::fs;
use ::std::path::{Path, PathBuf};
use ::std::sync::atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering};
use ::std::sync::{Arc, Mutex};
use bao_engine::context::RawValueRootGuard;
use bun_core::ZBox;
use bun_sys::fs as bun_fs;
use mozjs::realm::AutoRealm;
// @trace REQ-ENG-005 [algorithm:base64] base64 via workspace bun_base64 (SIMD-accelerated)

use mozjs::glue::NewCompileOptions;
use mozjs::jsapi::*;
use mozjs::jsval::{DoubleValue, Int32Value, JSVal, StringValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;
use crate::node_util::register_js_functions;

// --- Async I/O infrastructure ---
// Background I/O uses std::thread::spawn + Arc<Mutex<Option<Result>>> shared slot.
// Completion crosses back to the JS thread as a ConcurrentTask on the pump's
// MiniEventLoop (A' route, user-adjudicated 2026-08-21): the worker calls
// complete_post → ConcurrentWakeup::enqueue_task_concurrent_cross_thread,
// which pushes the embedded AnyTaskWithExtraContext carrier onto the loop
// captured at spawn (timers::with_event_loop — the ConcurrentWakeup-registered
// instance the pump ticks) and wakes it. The tasklet pops inside
// MiniEventLoop::tick_without_idle, i.e. on the JS thread's own JSContext
// (single-thread iron rule). The former uws_loop_defer path never delivered:
// it deferred onto the WORKER thread's private uWS::Loop (thread-local lazy
// init in C++ Loop.h) — a loop no pump ever ticks.

/// Result of statfs() — simplified subset of fields exposed by Node.js fs.statfs().
#[allow(dead_code)]
struct StatfsResult {
    type_: u64,
    bsize: u64,
    #[allow(dead_code)]
    frsize: u64,
    blocks: u64,
    bfree: u64,
    bavail: u64,
    files: u64,
    ffree: u64,
}

#[allow(dead_code)]
enum FsAsyncResult {
    Ok(Vec<u8>),
    OkStat(bun_sys::PosixStat),
    OkString(String),
    OkVoid,
    OkBool(bool),
    OkI32(i32),
    OkOpen(i32),
    OkRead { bytes_read: i32, buffer: Vec<u8> },
    /// readv completion: `buffer` is the concatenation of everything read;
    /// the dispatch scatters it back into the rooted caller views.
    OkReadv { bytes_read: i32, buffer: Vec<u8> },
    OkWrite(i32),
    OkDirnames(Vec<String>),
    OkStatfs(StatfsResult),
    OkDirents(Vec<(String, bool)>),
}

struct FsAsyncCtx {
    cx: *mut JSContext,
    /// Raw callback pointer captured at spawn. Prefer `cb_root.get(0)` —
    /// the guard's slot is updated in place by a moving GC; this pointer is
    /// only the fallback for the rooting-failed path.
    callback: *mut JSObject,
    /// RAII heap root for the callback value, spanning the worker-thread
    /// window. Released when this Box drops (tasklet run or the worker-side
    /// fail-closed release), liveness-guarded.
    cb_root: Option<RawValueRootGuard>,
    result: Arc<Mutex<Option<::std::result::Result<FsAsyncResult, (String, String)>>>>,
    encoding: Option<String>,
    op_name: String,
    path: String,
    /// JS thread's pump MiniEventLoop, captured at spawn via
    /// `crate::timers::with_event_loop` — the ConcurrentWakeup-REGISTERED
    /// instance. NOT dispatch_sm's `BaoEventLoop` (an independent
    /// MiniEventLoop): tasklets must land on the loop the pump actually
    /// ticks, or they are never drained.
    mini_loop_ptr: *const bun_event_loop::MiniEventLoop::MiniEventLoop<'static>,
    /// ConcurrentTask carrier embedded in this Box (address-stable for the
    /// Box's lifetime; handed to the queue by address, never moved).
    concurrent_task: bun_event_loop::AnyTaskWithExtraContext::AnyTaskWithExtraContext,
    /// Prevents duplicate tasklet scheduling (single completion per op; the
    /// CAS also makes an accidental double complete_post a no-op).
    has_scheduled: AtomicBool,
    /// Caller-owned destination views that the completion writes back into
    /// (fs.read's `buffer`, fs.readv's `buffers`). Rooted here so a moving
    /// GC updates the slots across the worker window; empty/None for every
    /// op that does not backfill caller memory.
    writeback: ::std::option::Option<RawValueRootGuard>,
    /// Byte offset into the first writeback view (fs.read's `offset`).
    writeback_offset: usize,
}

/// Process-global count of in-flight fs async ops (spawn-side ++, retired by
/// `FsAsyncCtx::drop`). Keeps the pump alive across the worker window so the
/// eval loop cannot exit between spawn and completion delivery (the
/// former "no timer, no HTTP → loop breaks" wedge).
static FS_ASYNC_PENDING: AtomicUsize = AtomicUsize::new(0);

/// In-flight fs async ops on any thread (spawn-side view for pump liveness).
pub fn fs_async_pending() -> usize {
    FS_ASYNC_PENDING.load(AtomicOrdering::Acquire)
}

impl Drop for FsAsyncCtx {
    fn drop(&mut self) {
        // Terminal event on EVERY exit path — tasklet ran on the JS thread,
        // or the worker-side fail-closed release — so the pending counter
        // can never hang on a dead op (false-branch leak = fail-open hang).
        FS_ASYNC_PENDING.fetch_sub(1, AtomicOrdering::AcqRel);
    }
}

/// ConcurrentTask shim: runs `fs_async_defer_callback` on the JS thread when
/// the pump's MiniEventLoop tick pops the carrier.
fn fs_async_tasklet_shim(ctx: *mut FsAsyncCtx, _parent: *mut ()) {
    // SAFETY: ctx is the Box::into_raw pointer bound to the carrier at
    // spawn; the tasklet runs exactly once (sole consumer takes the Box).
    unsafe { fs_async_defer_callback(ctx) };
}

/// Worker-thread completion: store-then-post (the result slot write happens
/// in the worker closure before this call; the ConcurrentTask enqueue
/// release-orders it ahead of the JS-thread read). Enqueues the JS-thread
/// tasklet on the captured pump loop. Fail-closed: a `false` return means
/// the owning JS thread exited (or never registered) — release the carrier
/// Box on the spot instead of leaking it. The RAII root release on this
/// foreign thread leaks the rooted slots by design (bounded; better than a
/// cross-thread unroot — see RawValueRootGuard::drop).
unsafe fn complete_post(ctx: *mut FsAsyncCtx) {
    unsafe {
        if (*ctx)
            .has_scheduled
            .compare_exchange(false, true, AtomicOrdering::AcqRel, AtomicOrdering::Acquire)
            .is_ok()
        {
            let loop_ptr = (*ctx).mini_loop_ptr;
            if !loop_ptr.is_null() {
                let carrier = ::std::ptr::addr_of_mut!((*ctx).concurrent_task);
                // SAFETY: carrier is the embedded task in a live Box; loop_ptr
                // was captured on the JS thread at spawn. true = pushed + woken.
                let delivered =
                    bun_event_loop::ConcurrentWakeup::enqueue_task_concurrent_cross_thread(
                        loop_ptr as *mut bun_event_loop::MiniEventLoop::MiniEventLoop<'static>,
                        ::std::ptr::NonNull::new_unchecked(carrier),
                    );
                if !delivered {
                    // JS thread gone: nothing will ever drain the queue.
                    drop(Box::from_raw(ctx));
                }
            } else {
                // Pump loop never captured (no with_event_loop on the
                // spawning thread): same fail-closed release.
                drop(Box::from_raw(ctx));
            }
        }
    }
}

/// Byte window of a caller-owned typed-array view. Used by the async
/// read/readv completion to backfill the buffers the caller passed (node
/// contract: the callback receives the SAME buffer object, filled). Returns
/// None for detached/non-view objects.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn view_byte_slot(view: JSVal) -> ::std::option::Option<(*mut u8, usize)> {
    if !view.is_object() {
        return ::std::option::Option::None;
    }
    let obj = view.to_object();
    let mut is_shared = false;
    let data = JS_GetUint8ArrayData(obj, &mut is_shared, ::std::ptr::null());
    if data.is_null() {
        return ::std::option::Option::None;
    }
    let len = JS_GetTypedArrayByteLength(obj) as usize;
    ::std::option::Option::Some((data as *mut u8, len))
}

unsafe fn fs_async_defer_callback(ctx_ptr: *mut FsAsyncCtx) {
    let ctx = Box::from_raw(ctx_ptr);
    let cx = ctx.cx;
    // Live callback value: prefer the RAII root's slot (updated in place by
    // a moving GC) over the raw pointer captured at spawn time.
    let cb_value = ctx.cb_root.as_ref().map_or_else(
        || mozjs::jsval::ObjectValue(ctx.callback),
        |g| g.get(0),
    );
    let encoding = ctx.encoding.as_deref();
    let _op_name = &ctx.op_name;

    let mut result_guard = ctx.result.lock().unwrap();
    let result_opt = result_guard.take();
    ::std::mem::drop(result_guard);

    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let cb_val = cb_value);
    // BCE-BUG-ENG-370 (same class as fetch_async/bun_build tasklets): this
    // runs from the MiniEventLoop tick OUTSIDE any JS activation — cx->realm_
    // is NULL. Enter the callback object's realm for the whole dispatch
    // window; realm-derived SM API below (JS_NewPlainObject, string
    // creation, create_stats_object, ...) would otherwise NULL-deref.
    rooted!(&in(cx_ref) let cb_obj = cb_value.to_object());
    let mut realm = AutoRealm::new_from_handle(cx_ref, cb_obj.handle());
    let realm_cx: &mut mozjs::context::JSContext = &mut realm;

    let global = CurrentGlobalOrNull(cx);
    // Drain-time dispatch may run outside any entered realm; fall back to
    // the thread's persistent realm global (realm-per-context model) instead
    // of silently dropping the callback — same convention as
    // timers::fire_js_callback_raw.
    let global = if global.is_null() {
        match bao_engine::context::thread_realm_global() {
            ::std::option::Option::Some(g) if !g.is_null() => g,
            _ => return,
        }
    } else {
        global
    };
    rooted!(&in(realm_cx) let global_rooted = global);

    match result_opt {
        Some(Ok(FsAsyncResult::Ok(data))) => {
            let val = string_or_buffer(cx, &data, encoding);
            rooted!(&in(realm_cx) let val_rooted = val);
            let args_arr = [UndefinedValue(), val_rooted.get()];
            let cb_args = HandleValueArray {
                length_: 2,
                elements_: args_arr.as_ptr(),
            };
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(
                cx,
                global_rooted.handle().into(),
                cb_val.handle().into(),
                &cb_args,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut rval,
                },
            );
            JS_ClearPendingException(cx);
        }
        Some(Ok(FsAsyncResult::OkStat(stat))) => {
            let stats_obj = create_stats_object(cx, &stat);
            rooted!(&in(realm_cx) let stats_val = mozjs::jsval::ObjectValue(stats_obj));
            let args_arr = [UndefinedValue(), stats_val.get()];
            let cb_args = HandleValueArray {
                length_: 2,
                elements_: args_arr.as_ptr(),
            };
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(
                cx,
                global_rooted.handle().into(),
                cb_val.handle().into(),
                &cb_args,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut rval,
                },
            );
            JS_ClearPendingException(cx);
        }
        Some(Ok(FsAsyncResult::OkString(s))) => {
            let c_str = ZBox::from_bytes(s.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            let val = if js_str.is_null() {
                UndefinedValue()
            } else {
                mozjs::jsval::StringValue(&*js_str)
            };
            rooted!(&in(realm_cx) let val_rooted = val);
            let args_arr = [UndefinedValue(), val_rooted.get()];
            let cb_args = HandleValueArray {
                length_: 2,
                elements_: args_arr.as_ptr(),
            };
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(
                cx,
                global_rooted.handle().into(),
                cb_val.handle().into(),
                &cb_args,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut rval,
                },
            );
            JS_ClearPendingException(cx);
        }
        Some(Ok(FsAsyncResult::OkVoid)) => {
            let args_arr = [UndefinedValue()];
            let cb_args = HandleValueArray {
                length_: 1,
                elements_: args_arr.as_ptr(),
            };
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(
                cx,
                global_rooted.handle().into(),
                cb_val.handle().into(),
                &cb_args,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut rval,
                },
            );
            JS_ClearPendingException(cx);
        }
        Some(Ok(FsAsyncResult::OkBool(b))) => {
            rooted!(&in(realm_cx) let val = mozjs::jsval::BooleanValue(b));
            let args_arr = [UndefinedValue(), val.get()];
            let cb_args = HandleValueArray {
                length_: 2,
                elements_: args_arr.as_ptr(),
            };
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(
                cx,
                global_rooted.handle().into(),
                cb_val.handle().into(),
                &cb_args,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut rval,
                },
            );
            JS_ClearPendingException(cx);
        }
        Some(Ok(FsAsyncResult::OkI32(v))) => {
            rooted!(&in(realm_cx) let val = mozjs::jsval::Int32Value(v));
            let args_arr = [UndefinedValue(), val.get()];
            let cb_args = HandleValueArray {
                length_: 2,
                elements_: args_arr.as_ptr(),
            };
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(
                cx,
                global_rooted.handle().into(),
                cb_val.handle().into(),
                &cb_args,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut rval,
                },
            );
            JS_ClearPendingException(cx);
        }
        Some(Ok(FsAsyncResult::OkOpen(fd))) => {
            rooted!(&in(realm_cx) let val = mozjs::jsval::Int32Value(fd));
            let args_arr = [UndefinedValue(), val.get()];
            let cb_args = HandleValueArray {
                length_: 2,
                elements_: args_arr.as_ptr(),
            };
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(
                cx,
                global_rooted.handle().into(),
                cb_val.handle().into(),
                &cb_args,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut rval,
                },
            );
            JS_ClearPendingException(cx);
        }
        Some(Ok(FsAsyncResult::OkRead { bytes_read, buffer })) => {
            // node contract: read(fd, buffer, ...) fills the CALLER's view and
            // hands that same object to the callback. Backfill the rooted
            // view (GC-stable slot) before dispatching; the fresh Buffer is
            // only a fallback when no caller view was captured.
            let mut caller_view: ::std::option::Option<JSVal> = ::std::option::Option::None;
            if let ::std::option::Option::Some(root) = ctx.writeback.as_ref() {
                if root.len() > 0 {
                    let view = root.get(0);
                    if let ::std::option::Option::Some((slot, len)) = unsafe { view_byte_slot(view) }
                    {
                        let off = ctx.writeback_offset.min(len);
                        let n = buffer.len().min(len - off);
                        if n > 0 {
                            unsafe {
                                ::std::ptr::copy_nonoverlapping(buffer.as_ptr(), slot.add(off), n)
                            };
                        }
                        caller_view = ::std::option::Option::Some(view);
                    }
                }
            }
            let buf_val = match caller_view {
                ::std::option::Option::Some(v) => v,
                ::std::option::Option::None => {
                    let buf_obj = crate::globals::create_buffer_object(cx, &buffer);
                    if buf_obj.is_null() {
                        UndefinedValue()
                    } else {
                        mozjs::jsval::ObjectValue(buf_obj)
                    }
                }
            };
            rooted!(&in(realm_cx) let buf_rooted = buf_val);
            rooted!(&in(realm_cx) let br_val = mozjs::jsval::Int32Value(bytes_read));
            let args_arr = [UndefinedValue(), br_val.get(), buf_rooted.get()];
            let cb_args = HandleValueArray {
                length_: 3,
                elements_: args_arr.as_ptr(),
            };
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(
                cx,
                global_rooted.handle().into(),
                cb_val.handle().into(),
                &cb_args,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut rval,
                },
            );
            JS_ClearPendingException(cx);
        }
        Some(Ok(FsAsyncResult::OkReadv { bytes_read, buffer })) => {
            // Scatter the contiguous read result back into the rooted caller
            // views, in iovec order. node's readv callback is (err, n).
            if let ::std::option::Option::Some(root) = ctx.writeback.as_ref() {
                let mut consumed = 0usize;
                for i in 0..root.len() {
                    if consumed >= buffer.len() {
                        break;
                    }
                    let view = root.get(i);
                    if let ::std::option::Option::Some((slot, len)) = unsafe { view_byte_slot(view) }
                    {
                        if len == 0 || slot.is_null() {
                            continue;
                        }
                        let n = (buffer.len() - consumed).min(len);
                        unsafe {
                            ::std::ptr::copy_nonoverlapping(
                                buffer.as_ptr().add(consumed),
                                slot,
                                n,
                            )
                        };
                        consumed += n;
                    }
                }
            }
            rooted!(&in(realm_cx) let br_val = mozjs::jsval::Int32Value(bytes_read));
            let args_arr = [UndefinedValue(), br_val.get()];
            let cb_args = HandleValueArray {
                length_: 2,
                elements_: args_arr.as_ptr(),
            };
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(
                cx,
                global_rooted.handle().into(),
                cb_val.handle().into(),
                &cb_args,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut rval,
                },
            );
            JS_ClearPendingException(cx);
        }
        Some(Ok(FsAsyncResult::OkWrite(written))) => {
            rooted!(&in(realm_cx) let val = mozjs::jsval::Int32Value(written));
            let args_arr = [UndefinedValue(), val.get()];
            let cb_args = HandleValueArray {
                length_: 2,
                elements_: args_arr.as_ptr(),
            };
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(
                cx,
                global_rooted.handle().into(),
                cb_val.handle().into(),
                &cb_args,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut rval,
                },
            );
            JS_ClearPendingException(cx);
        }
        Some(Ok(FsAsyncResult::OkDirnames(names))) => {
            rooted!(&in(realm_cx) let arr = w2::NewArrayObject1(realm_cx, names.len()));
            if !arr.get().is_null() {
                for (idx, name) in names.iter().enumerate() {
                    let c_name = ZBox::from_bytes(name.as_bytes());
                    let js_str = JS_NewStringCopyZ(cx, c_name.as_ptr());
                    if !js_str.is_null() {
                        rooted!(&in(realm_cx) let val = mozjs::jsval::StringValue(&*js_str));
                        JS_DefineElement(
                            cx,
                            arr.handle().into(),
                            idx as u32,
                            val.handle().into(),
                            JSPROP_ENUMERATE as u32,
                        );
                    }
                }
            }
            rooted!(&in(realm_cx) let arr_val = mozjs::jsval::ObjectValue(arr.get()));
            let args_arr = [UndefinedValue(), arr_val.get()];
            let cb_args = HandleValueArray {
                length_: 2,
                elements_: args_arr.as_ptr(),
            };
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(
                cx,
                global_rooted.handle().into(),
                cb_val.handle().into(),
                &cb_args,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut rval,
                },
            );
            JS_ClearPendingException(cx);
        }
        Some(Ok(FsAsyncResult::OkDirents(entries))) => {
            rooted!(&in(realm_cx) let arr = w2::NewArrayObject1(realm_cx, entries.len()));
            if !arr.get().is_null() {
                for (idx, (name, is_dir)) in entries.iter().enumerate() {
                    let dirent = create_dirent(cx, name, *is_dir);
                    rooted!(&in(realm_cx) let val = mozjs::jsval::ObjectValue(dirent));
                    JS_DefineElement(
                        cx,
                        arr.handle().into(),
                        idx as u32,
                        val.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
            }
            rooted!(&in(realm_cx) let arr_val = mozjs::jsval::ObjectValue(arr.get()));
            let args_arr = [UndefinedValue(), arr_val.get()];
            let cb_args = HandleValueArray {
                length_: 2,
                elements_: args_arr.as_ptr(),
            };
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(
                cx,
                global_rooted.handle().into(),
                cb_val.handle().into(),
                &cb_args,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut rval,
                },
            );
            JS_ClearPendingException(cx);
        }
        Some(Ok(FsAsyncResult::OkStatfs(sf))) => {
            let stats_obj = create_statfs_object(cx, &sf);
            rooted!(&in(realm_cx) let stats_val = mozjs::jsval::ObjectValue(stats_obj));
            let args_arr = [UndefinedValue(), stats_val.get()];
            let cb_args = HandleValueArray {
                length_: 2,
                elements_: args_arr.as_ptr(),
            };
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(
                cx,
                global_rooted.handle().into(),
                cb_val.handle().into(),
                &cb_args,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut rval,
                },
            );
            JS_ClearPendingException(cx);
        }
        Some(Err((code, msg))) => {
            rooted!(&in(realm_cx) let err_obj = JS_NewPlainObject(cx));
            if !err_obj.get().is_null() {
                let c_msg = ZBox::from_bytes(msg.as_bytes());
                let js_str = JS_NewStringCopyZ(cx, c_msg.as_ptr());
                if !js_str.is_null() {
                    rooted!(&in(realm_cx) let msg_val = mozjs::jsval::StringValue(&*js_str));
                    JS_DefineProperty(
                        cx,
                        err_obj.handle().into(),
                        c"message".as_ptr(),
                        msg_val.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
                let c_code = ZBox::from_bytes(code.as_bytes());
                let code_str = JS_NewStringCopyZ(cx, c_code.as_ptr());
                if !code_str.is_null() {
                    rooted!(&in(realm_cx) let code_val = mozjs::jsval::StringValue(&*code_str));
                    JS_DefineProperty(
                        cx,
                        err_obj.handle().into(),
                        c"code".as_ptr(),
                        code_val.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
                let c_path = ZBox::from_bytes(ctx.path.as_bytes());
                let path_str = JS_NewStringCopyZ(cx, c_path.as_ptr());
                if !path_str.is_null() {
                    rooted!(&in(realm_cx) let path_val = mozjs::jsval::StringValue(&*path_str));
                    JS_DefineProperty(
                        cx,
                        err_obj.handle().into(),
                        c"path".as_ptr(),
                        path_val.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
            }
            rooted!(&in(realm_cx) let err_val = mozjs::jsval::ObjectValue(err_obj.get()));
            let args_arr = [err_val.get()];
            let cb_args = HandleValueArray {
                length_: 1,
                elements_: args_arr.as_ptr(),
            };
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(
                cx,
                global_rooted.handle().into(),
                cb_val.handle().into(),
                &cb_args,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut rval,
                },
            );
            JS_ClearPendingException(cx);
        }
        None => {
            let null_args = HandleValueArray::empty();
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(
                cx,
                global_rooted.handle().into(),
                cb_val.handle().into(),
                &null_args,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut rval,
                },
            );
            JS_ClearPendingException(cx);
        }
    }
    // Terminal unroot is RAII: `ctx` (Box<FsAsyncCtx>) drops at the end of
    // this callback, releasing the `cb_root` heap root with the correct
    // registered address on every exit path (including the null-global
    // early return above).
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn extract_callback_and_encoding(
    cx: *mut JSContext,
    args: &CallArgs,
    start_idx: u32,
) -> Option<(*mut JSObject, Option<String>)> {
    let mut cb_idx = None;
    let mut encoding = None;
    for i in start_idx..args.argc_ {
        let val = *args.get(i).ptr;
        if val.is_object() {
            // Only CALLABLE objects are callbacks. Plain option objects
            // (e.g. fs.rmdir(path, {recursive: true}, cb) — upstream
            // fff3b29d6) must not be captured as the callback; previously
            // the first object won, the later real callback was dropped,
            // and the deferred dispatch called a non-callable (swallowed
            // by JS_ClearPendingException) so the caller's callback never
            // fired.
            if cb_idx.is_none() && IsCallable(val.to_object()) {
                cb_idx = Some(i);
            } else if encoding.is_none() {
                encoding = get_encoding_opt(cx, args, i);
            }
        } else if val.is_string() && encoding.is_none() {
            encoding = Some(crate::jsstr_to_rust_string(cx, val.to_string()));
        }
    }
    cb_idx.map(|idx| ((*args.get(idx).ptr).to_object(), encoding))
}

/// Truncate-family `len` argument coercion, single source for every entry
/// point (truncateSync / truncate / promises.truncate / ftruncateSync /
/// ftruncate / FileHandle.truncate).
/// @trace REQ-ENG-007 — upstream 4c815c11a5 port: an explicit `undefined`
/// len is the same as an absent one — truncate to 0 (Node's
/// `if (len === undefined) len = 0`; `lib/internal/fs/promises.js` has
/// `truncate(path, len = 0)`). Numbers coerce as before; bao's loose
/// validation posture for other non-number values (→ 0) is unchanged.
fn truncate_len_from_val(len_val: JSVal) -> i64 {
    if len_val.is_int32() {
        len_val.to_int32() as i64
    } else if len_val.is_double() {
        len_val.to_double() as i64
    } else {
        0
    }
}

/// Errno → JS-visible `code` for EVERY fs error delivery face (sync throw in
/// `throw_fs_error`, callback error objects, promise rejections in
/// `reject_fs_error`) — single source of truth.
/// @trace REQ-ENG-007 — upstream 83350172b9 port: ENAMETOOLONG (a path too
/// long for ANY syscall) must surface as code "ENAMETOOLONG" on every face,
/// as Node reports it; previously it fell into the "ERR" fallback. Raw
/// errnos without an ErrorKind mapping surface verbatim (Node parity, e.g.
/// mkdtemp('') → EINVAL).
fn io_error_code(err: &::std::io::Error) -> &'static str {
    match err.raw_os_error() {
        Some(libc::EINVAL) => return "EINVAL",
        // Bad file descriptor: every fd-taking fs call on a closed/invalid fd
        // reports code="EBADF" in node (C1 registry #7 — node v24.19.0 oracle
        // probe: readSync(9999, ...) → EBADF); without this arm the errno
        // fell through to the generic "ERR".
        Some(libc::EBADF) => return "EBADF",
        Some(libc::ENAMETOOLONG) => return "ENAMETOOLONG",
        // POSIX rmdir(non-empty) is ENOTEMPTY(39); Windows
        // RemoveDirectory reports ERROR_DIR_NOT_EMPTY(145) — both are the
        // node contract's ENOTEMPTY, normalize at the single code source.
        Some(libc::ENOTEMPTY) => return "ENOTEMPTY",
        #[cfg(windows)]
        Some(145) => return "ENOTEMPTY",
        _ => {}
    }
    // Shape-normalized synthetic errors (rmdir plain arm): the message
    // carries the errno token because from_raw_os_error(POSIX errno) renders
    // as "Unknown error N" under the Windows CRT strerror table.
    {
        let msg = err.to_string();
        if msg.contains("ENOTEMPTY") {
            return "ENOTEMPTY";
        }
        if msg.contains("ENOTDIR") {
            return "ENOTDIR";
        }
    }
    match err.kind() {
        ::std::io::ErrorKind::NotFound => "ENOENT",
        ::std::io::ErrorKind::PermissionDenied => "EACCES",
        ::std::io::ErrorKind::AlreadyExists => "EEXIST",
        ::std::io::ErrorKind::IsADirectory => "EISDIR",
        ::std::io::ErrorKind::NotADirectory => "ENOTDIR",
        _ => "ERR",
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn spawn_fs_async<F>(
    cx: *mut JSContext,
    op_name: &str,
    path: String,
    callback: *mut JSObject,
    encoding: Option<String>,
    work: F,
) where
    F: FnOnce() -> ::std::result::Result<FsAsyncResult, ::std::io::Error> + Send + 'static,
{
    unsafe {
        spawn_fs_async_writeback(cx, op_name, path, callback, encoding, None, 0, work);
    }
}

/// Full spawn form carrying caller-owned destination views. `writeback` is a
/// rooted guard (spans the worker window); `writeback_offset` applies to the
/// first view (fs.read's `offset`; ignored by readv).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn spawn_fs_async_writeback<F>(
    cx: *mut JSContext,
    op_name: &str,
    path: String,
    callback: *mut JSObject,
    encoding: Option<String>,
    writeback: ::std::option::Option<RawValueRootGuard>,
    writeback_offset: usize,
    work: F,
) where
    F: FnOnce() -> ::std::result::Result<FsAsyncResult, ::std::io::Error> + Send + 'static,
{
    // Heap-root the callback value for the async window via the RAII guard
    // (stable heap slot the GC updates in place; unrooted when the
    // FsAsyncCtx Box drops, with the correct registered address).
    let cb_val = mozjs::jsval::ObjectValue(callback);
    let cb_root = unsafe {
        RawValueRootGuard::new(cx, ::std::slice::from_ref(&cb_val), c"fs_async_cb")
    };

    let result_slot: Arc<Mutex<Option<::std::result::Result<FsAsyncResult, (String, String)>>>> =
        Arc::new(Mutex::new(None));
    let result_slot_clone = result_slot.clone();

    let op_name_owned = op_name.to_string();
    let path_for_err = path.clone();

    let ctx = Box::new(FsAsyncCtx {
        cx,
        callback,
        cb_root,
        result: result_slot,
        encoding,
        op_name: op_name.to_string(),
        path,
        mini_loop_ptr: ::std::ptr::null(),
        concurrent_task:
            bun_event_loop::AnyTaskWithExtraContext::AnyTaskWithExtraContext::default(),
        has_scheduled: AtomicBool::new(false),
        writeback,
        writeback_offset,
    });
    // Increment BEFORE the Box can ever drop (Drop retires it) so the pump
    // liveness view can never miss an in-flight op.
    FS_ASYNC_PENDING.fetch_add(1, AtomicOrdering::Release);
    let ctx_ptr = Box::into_raw(ctx);

    // Capture the JS thread's pump MiniEventLoop — the ConcurrentWakeup-
    // registered instance behind timers::with_event_loop (materializes +
    // registers on first call). NOT dispatch_sm's BaoEventLoop instance.
    // SAFETY: the closure borrows the loop for the call only; the pointer
    // stays valid for the thread's lifetime (leaked at materialization).
    let loop_ptr: *const bun_event_loop::MiniEventLoop::MiniEventLoop<'static> =
        crate::timers::with_event_loop(|loop_| loop_ as *const _);
    // SAFETY: ctx_ptr is a live heap allocation we just created; the carrier
    // is embedded, so its address is stable for the Box's lifetime (the
    // completion side hands the queue this exact address via addr_of_mut).
    unsafe {
        (*ctx_ptr).mini_loop_ptr = loop_ptr;
        (*ctx_ptr).concurrent_task.from(ctx_ptr, fs_async_tasklet_shim);
    }

    // Raw pointers are not Send — the worker takes the ctx as an integer
    // token and reconstitutes it only inside the closure; off-thread it is
    // touched solely via complete_post's atomics/queue push.
    let ctx_token = ctx_ptr as usize;

    ::std::thread::spawn(move || {
        let result = work();
        let stored = match result {
            Ok(v) => Ok(v),
            Err(e) => {
                let code = io_error_code(&e).to_string();
                let msg = format!("{} '{}': {}", op_name_owned, path_for_err, e);
                Err((code, msg))
            }
        };
        {
            let mut slot = result_slot_clone.lock().unwrap();
            *slot = Some(stored);
        }
        // SAFETY: ctx_token reconstitutes the live spawn-side Box; sole
        // completion path (CAS-guarded).
        unsafe { complete_post(ctx_token as *mut FsAsyncCtx) };
    });
}

const FS_STREAM_JS: &str = r#"
(function() {
  var fs = globalThis.__fs_stream_ref;
  var flush = globalThis.__bao_fs_stream_flush;

  function EE() { this._events = {}; }
  EE.prototype.on = function(e, fn) {
    (this._events[e] || (this._events[e] = [])).push(fn);
    return this;
  };
  EE.prototype.emit = function(e) {
    var a = Array.prototype.slice.call(arguments, 1);
    var ls = this._events[e];
    if (ls) for (var i = 0; i < ls.length; i++) ls[i].apply(this, a);
    return !!ls;
  };

  function createReadStream(path, opts) {
    var s = new EE();
    s.path = path;
    s.readable = true;
    s.writable = false;
    s.bytesRead = 0;
    var encoding = (opts && opts.encoding) || null;
    try {
      var data = fs.readFileSync(path, encoding);
      s.bytesRead = (typeof data === 'string') ? data.length : 0;
      setTimeout(function() {
        s.emit('open', 0);
        if (data) s.emit('data', data);
        s.emit('end');
        s.emit('close');
      }, 0);
    } catch(e) {
      setTimeout(function() { s.emit('error', e); }, 0);
    }
    s.pipe = function(dest) {
      this.on('data', function(c) { dest.write(c); });
      this.on('end', function() { dest.end(); });
      return dest;
    };
    s.destroy = function() { this.readable = false; this.emit('close'); return this; };
    return s;
  }

  function createWriteStream(path, opts) {
    var s = new EE();
    s.path = path;
    s.readable = false;
    s.writable = true;
    s.bytesWritten = 0;
    s._buffer = [];
    s._ended = false;
    setTimeout(function() { s.emit('open', 0); }, 0);
    s.write = function(chunk) {
      if (this._ended) return false;
      // Keep chunks RAW: strings stay strings, Buffer/TypedArray chunks stay
      // binary views (flush extracts their bytes natively). Non-string
      // non-object primitives keep the legacy String() coercion.
      if (typeof chunk !== 'string' && typeof chunk !== 'object') chunk = String(chunk);
      this._buffer.push(chunk);
      this.bytesWritten += (chunk && chunk.length) || 0;
      return true;
    };
    s.end = function(chunk) {
      if (chunk) {
        if (typeof chunk !== 'string' && typeof chunk !== 'object') chunk = String(chunk);
        this._buffer.push(chunk);
        this.bytesWritten += (chunk && chunk.length) || 0;
      }
      this._ended = true;
      this.writable = false;
      try {
        // Binary-safe flush: string chunks → UTF-8 bytes (byte-identical to
        // the old join('') + writeFileSync(string)); view chunks → raw bytes.
        flush(this.path, this._buffer);
        this.emit('finish');
      } catch(e) {
        this.emit('error', e);
      }
      this.emit('close');
      return this;
    };
    s.destroy = function() { this.writable = false; this.emit('close'); return this; };
    return s;
  }

  return { createReadStream: createReadStream, createWriteStream: createWriteStream };
})();
"#;

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let fs_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if fs_obj.get().is_null() {
        return;
    }

    unsafe {
        // Sync methods (e201 REQ-ARCH-001: table-driven registration — each row
        // expands to the exact JS_DefineFunction call the handwritten wall used;
        // same order, same attrs)
        register_js_functions!(cx, fs_obj, JSPROP_ENUMERATE as u32;
            (c"readFileSync", fs_read_file_sync, 1)
            (c"writeFileSync", fs_write_file_sync, 2)
            (c"appendFileSync", fs_append_file_sync, 2)
            (c"existsSync", fs_exists_sync, 1)
            (c"mkdirSync", fs_mkdir_sync, 1)
            (c"readdirSync", fs_readdir_sync, 1)
            (c"statSync", fs_stat_sync, 1)
            (c"lstatSync", fs_lstat_sync, 1)
            (c"unlinkSync", fs_unlink_sync, 1)
            (c"rmdirSync", fs_rmdir_sync, 1)
            (c"rmSync", fs_rm_sync, 1)
            (c"renameSync", fs_rename_sync, 2)
            (c"copyFileSync", fs_copy_file_sync, 2)
            (c"chmodSync", fs_chmod_sync, 2)
            (c"realpathSync", fs_realpath_sync, 1)
            (c"readlinkSync", fs_readlink_sync, 1)
            (c"symlinkSync", fs_symlink_sync, 2)
            (c"linkSync", fs_link_sync, 2)
            (c"cpSync", fs_cp_sync, 2)
            (c"cp", fs_cp, 3)
            (c"watch", fs_watch, 2)
            (c"watchFile", fs_watch_file, 2)
            (c"unwatchFile", fs_unwatch_file, 2)
            (c"statfsSync", fs_statfs_sync, 1)
            (c"openSync", fs_open_sync, 2)
            (c"closeSync", fs_close_sync, 1)
            (c"readSync", fs_read_sync, 4)
            (c"writeSync", fs_write_sync, 4)
            (c"mkdtempSync", fs_mkdtemp_sync, 1)
            (c"fchmodSync", fs_fchmod_sync, 2)
            (c"fchownSync", fs_fchown_sync, 3)
            (c"fstatSync", fs_fstat_sync, 1)
            (c"fdatasyncSync", fs_fdatasync_sync, 1)
            (c"fsyncSync", fs_fsync_sync, 1)
            (c"ftruncateSync", fs_ftruncate_sync, 2)
            (c"truncateSync", fs_truncate_sync, 2)
            (c"opendirSync", fs_opendir_sync, 1)
            (c"futimesSync", fs_futimes_sync, 3)
            (c"lchmodSync", fs_lchmod_sync, 2)
            (c"lchownSync", fs_lchown_sync, 3)
            (c"readvSync", fs_readv_sync, 3)
            (c"writevSync", fs_writev_sync, 3)
            (c"globSync", fs_glob_sync, 2)
            (c"openAsBlob", fs_open_as_blob, 2)
        );

        // Async methods
        register_js_functions!(cx, fs_obj, JSPROP_ENUMERATE as u32;
            (c"readFile", fs_read_file, 2)
            (c"writeFile", fs_write_file, 3)
            (c"mkdir", fs_mkdir, 2)
            (c"appendFile", fs_append_file, 3)
            (c"access", fs_access, 2)
            (c"chmod", fs_chmod, 2)
            (c"chown", fs_chown, 3)
            (c"close", fs_close, 1)
            (c"copyFile", fs_copy_file, 2)
            (c"exists", fs_exists, 1)
            (c"fchmod", fs_fchmod, 2)
            (c"fchown", fs_fchown, 3)
            (c"fdatasync", fs_fdatasync, 1)
            (c"fstat", fs_fstat, 1)
            (c"fsync", fs_fsync, 1)
            (c"ftruncate", fs_ftruncate, 2)
            (c"futimes", fs_futimes, 3)
            (c"lchown", fs_lchown, 3)
            (c"link", fs_link, 2)
            (c"lstat", fs_lstat, 1)
            (c"lutimes", fs_lutimes, 3)
            (c"mkdtemp", fs_mkdtemp, 1)
            (c"open", fs_open, 3)
            (c"opendir", fs_opendir, 1)
            (c"read", fs_read, 4)
            (c"readdir", fs_readdir, 2)
            (c"readlink", fs_readlink, 1)
            (c"readv", fs_readv, 4)
            (c"realpath", fs_realpath, 1)
            (c"rename", fs_rename, 2)
            (c"rm", fs_rm, 2)
            (c"rmdir", fs_rmdir, 1)
            (c"stat", fs_stat, 1)
            (c"symlink", fs_symlink, 2)
            (c"truncate", fs_truncate, 2)
            (c"unlink", fs_unlink, 1)
            (c"utimes", fs_utimes, 3)
            (c"write", fs_write, 4)
            (c"writev", fs_writev, 4)
            (c"statfs", fs_statfs, 1)
            (c"glob", fs_glob, 3)
        );

        // Constants
        let constants: &[(&str, i32)] = &[("F_OK", 0), ("R_OK", 4), ("W_OK", 2), ("X_OK", 1)];
        for (name, value) in constants {
            let c_name = ZBox::from_bytes(name.as_bytes());
            rooted!(&in(cx) let val = mozjs::jsval::Int32Value(*value));
            JS_DefineProperty(
                cx.raw_cx(),
                fs_obj.handle().into(),
                c_name.as_ptr(),
                val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }

        // fs.constants namespace — node exposes F_OK/R_OK/W_OK/X_OK on both
        // fs and fs.constants, plus the copy-file modifiers. The missing
        // namespace made `fs.constants.R_OK` a TypeError and any script
        // touching it died mid-scheduling (C11 wave; node v24.19.0 oracle:
        // fs.constants.R_OK === 4, fs.constants.COPYFILE_EXCL === 1).
        rooted!(&in(cx) let constants_obj = unsafe { w2::JS_NewPlainObject(cx) });
        if !constants_obj.get().is_null() {
            for (name, value) in constants {
                let c_cn = ZBox::from_bytes(name.as_bytes());
                rooted!(&in(cx) let cval = mozjs::jsval::Int32Value(*value));
                JS_DefineProperty(
                    cx.raw_cx(),
                    constants_obj.handle().into(),
                    c_cn.as_ptr(),
                    cval.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
            let copyfile: &[(&str, i32)] = &[("COPYFILE_EXCL", 1)];
            for (name, value) in copyfile {
                let c_cn = ZBox::from_bytes(name.as_bytes());
                rooted!(&in(cx) let cval = mozjs::jsval::Int32Value(*value));
                JS_DefineProperty(
                    cx.raw_cx(),
                    constants_obj.handle().into(),
                    c_cn.as_ptr(),
                    cval.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
            rooted!(&in(cx) let cobj_val = mozjs::jsval::ObjectValue(constants_obj.get()));
            JS_DefineProperty(
                cx.raw_cx(),
                fs_obj.handle().into(),
                c"constants".as_ptr(),
                cobj_val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }

        // fs.promises namespace
        rooted!(&in(cx) let promises_obj = w2::JS_NewPlainObject(cx));
        if !promises_obj.get().is_null() {
            register_js_functions!(cx, promises_obj, JSPROP_ENUMERATE as u32;
                (c"readFile", fs_promises_read_file, 1)
                (c"writeFile", fs_promises_write_file, 2)
                (c"stat", fs_promises_stat, 1)
                (c"readdir", fs_promises_readdir, 1)
                (c"mkdir", fs_promises_mkdir, 1)
                (c"unlink", fs_promises_unlink, 1)
                (c"rename", fs_promises_rename, 2)
                (c"copyFile", fs_promises_copy_file, 2)
                (c"lstat", fs_promises_lstat, 1)
                (c"appendFile", fs_promises_append_file, 2)
                (c"chmod", fs_promises_chmod, 2)
                (c"chown", fs_promises_chown, 3)
                (c"access", fs_promises_access, 1)
                (c"rm", fs_promises_rm, 1)
                (c"rmdir", fs_promises_rmdir, 1)
                (c"realpath", fs_promises_realpath, 1)
                (c"readlink", fs_promises_readlink, 1)
                (c"symlink", fs_promises_symlink, 2)
                (c"link", fs_promises_link, 2)
                (c"truncate", fs_promises_truncate, 2)
                (c"utimes", fs_promises_utimes, 3)
                (c"mkdtemp", fs_promises_mkdtemp, 1)
                (c"open", fs_promises_open, 2)
                (c"read", fs_promises_read, 4)
                (c"write", fs_promises_write, 4)
                (c"statfs", fs_promises_statfs, 1)
            );

            rooted!(&in(cx) let prom_val = mozjs::jsval::ObjectValue(promises_obj.get()));
            JS_DefineProperty(
                cx.raw_cx(),
                fs_obj.handle().into(),
                c"promises".as_ptr(),
                prom_val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }

    // Evaluate createReadStream/createWriteStream polyfill
    unsafe {
        install_fs_stream_polyfill(cx, fs_obj.handle());
    }

    cache_builtin(cx, "fs", fs_obj.get());

    // Register fs/promises sub-path — reuses the same promise-based methods
    // already defined on fs.promises. The sub-path module `require("fs/promises")`
    // gets its own top-level builtin with the promise methods directly on it.
    unsafe {
        install_fs_promises_subpath(cx);
    }
}


/// Build the `require("fs/promises")` top-level builtin: a plain object with
/// the promise-based methods directly on it (same natives as `fs.promises`)
/// plus the FileHandle constructor, cached under "fs/promises".
///
/// e201 REQ-ARCH-001: extracted from `install` (control flow unchanged; the
/// registration wall collapsed to the shared table macro).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn install_fs_promises_subpath(cx: &mut mozjs::context::JSContext) {
    unsafe {
        rooted!(&in(cx) let fsp_obj = w2::JS_NewPlainObject(cx));
        if !fsp_obj.get().is_null() {
            register_js_functions!(cx, fsp_obj, JSPROP_ENUMERATE as u32;
                (c"readFile", fs_promises_read_file, 1)
                (c"writeFile", fs_promises_write_file, 2)
                (c"stat", fs_promises_stat, 1)
                (c"readdir", fs_promises_readdir, 1)
                (c"mkdir", fs_promises_mkdir, 1)
                (c"unlink", fs_promises_unlink, 1)
                (c"rename", fs_promises_rename, 2)
                (c"copyFile", fs_promises_copy_file, 2)
                (c"lstat", fs_promises_lstat, 1)
                (c"appendFile", fs_promises_append_file, 2)
                (c"chmod", fs_promises_chmod, 2)
                (c"chown", fs_promises_chown, 3)
                (c"access", fs_promises_access, 1)
                (c"rm", fs_promises_rm, 1)
                (c"rmdir", fs_promises_rmdir, 1)
                (c"realpath", fs_promises_realpath, 1)
                (c"readlink", fs_promises_readlink, 1)
                (c"symlink", fs_promises_symlink, 2)
                (c"link", fs_promises_link, 2)
                (c"truncate", fs_promises_truncate, 2)
                (c"utimes", fs_promises_utimes, 3)
                (c"mkdtemp", fs_promises_mkdtemp, 1)
                (c"open", fs_promises_open, 2)
                (c"read", fs_promises_read, 4)
                (c"write", fs_promises_write, 4)
                (c"statfs", fs_promises_statfs, 1)

                // FileHandle class — wraps a file descriptor and provides
                // promise-based read/write/close/stat etc.
                (c"FileHandle", fs_promises_filehandle_ctor, 1)
            );
            cache_builtin(cx, "fs/promises", fsp_obj.get());
        }
    }
}

/// Evaluate the node:fs:streams polyfill and re-export its
/// createReadStream/createWriteStream constructors onto the fs object.
///
/// The polyfill captures `__fs_stream_ref` (the fs object) and
/// `__bao_fs_stream_flush` (binary-safe end() flush native) from the global
/// object in its closure; both globals are deleted right after evaluation
/// (the captured references stay alive) so no extra API surface remains.
///
/// e201 REQ-ARCH-001: extracted from `install` (control flow unchanged; the
/// `global.is_null()` nest level became a guard clause).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn install_fs_stream_polyfill(
    cx: &mut mozjs::context::JSContext,
    fs_obj: mozjs::rust::Handle<'_, *mut JSObject>,
) {
    let global = JS::CurrentGlobalOrNull(cx.raw_cx());
    if global.is_null() {
        return;
    }
    rooted!(&in(cx) let global_rooted = global);
    rooted!(&in(cx) let fs_val = mozjs::jsval::ObjectValue(fs_obj.get()));
    JS_DefineProperty(
        cx.raw_cx(),
        global_rooted.handle().into(),
        c"__fs_stream_ref".as_ptr(),
        fs_val.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
    // Hidden native the polyfill captures in its closure for the
    // binary-safe end() flush; deleted right after evaluation (the
    // captured reference stays alive) so no API surface is added.
    w2::JS_DefineFunction(
        cx,
        global_rooted.handle(),
        c"__bao_fs_stream_flush".as_ptr(),
        Some(fs_write_stream_flush),
        2,
        0,
    );

    let c_filename = ZBox::from_bytes("node:fs:streams".as_bytes());
    let opts = NewCompileOptions(cx.raw_cx(), c_filename.as_ptr(), 1);
    if !opts.is_null() {
        let mut src = mozjs::rust::transform_str_to_source_text(FS_STREAM_JS);
        let mut rval = UndefinedValue();
        let rval_handle = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        };
        let ok = mozjs_sys::jsapi::JS::Evaluate2(cx.raw_cx(), opts, &mut src, rval_handle);
        mozjs::glue::DeleteCompileOptions(opts);

        if ok && rval.is_object() {
            let exports = rval.to_object();
            rooted!(&in(cx) let exports_rooted = exports);

            for name in &["createReadStream", "createWriteStream"] {
                let cname = ZBox::from_bytes(name.as_bytes());
                let mut val = UndefinedValue();
                JS_GetProperty(
                    cx.raw_cx(),
                    exports_rooted.handle().into(),
                    cname.as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut val,
                    },
                );
                if !val.is_undefined() {
                    rooted!(&in(cx) let val_root = val);
                    JS_DefineProperty(
                        cx.raw_cx(),
                        fs_obj.into(),
                        cname.as_ptr(),
                        val_root.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
            }
        }
    }

    JS_DeleteProperty1(
        cx.raw_cx(),
        global_rooted.handle().into(),
        c"__fs_stream_ref".as_ptr(),
    );
    JS_DeleteProperty1(
        cx.raw_cx(),
        global_rooted.handle().into(),
        c"__bao_fs_stream_flush".as_ptr(),
    );
}

// --- Argument helpers ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn get_path_arg(
    cx: *mut JSContext,
    args: &CallArgs,
    index: u32,
) -> ::std::result::Result<::std::string::String, bool> {
    if args.argc_ <= index {
        JS_ReportErrorUTF8(cx, c"Missing path argument".as_ptr());
        return ::std::result::Result::Err(false);
    }
    let val = *args.get(index).ptr;
    if val.is_string() {
        let s = val.to_string();
        if !s.is_null() {
            return ::std::result::Result::Ok(crate::jsstr_to_rust_string(cx, s));
        }
    }
    JS_ReportErrorUTF8(cx, c"The \"path\" argument must be of type string".as_ptr());
    ::std::result::Result::Err(false)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn get_encoding_opt(
    cx: *mut JSContext,
    args: &CallArgs,
    index: u32,
) -> ::std::option::Option<::std::string::String> {
    if args.argc_ <= index {
        return ::std::option::Option::None;
    }
    let val = *args.get(index).ptr;
    if val.is_string() {
        let s = val.to_string();
        if !s.is_null() {
            return ::std::option::Option::Some(crate::jsstr_to_rust_string(cx, s));
        }
    }
    if val.is_object() {
        let mut wrapped_cx_enc =
            mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        let cx_ref_enc = &mut wrapped_cx_enc;
        rooted!(&in(cx_ref_enc) let obj = val.to_object());
        let mut enc_val = UndefinedValue();
        let enc_h = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut enc_val,
        };
        JS_GetProperty(cx, obj.handle().into(), c"encoding".as_ptr(), enc_h);
        if enc_val.is_string() {
            let s = enc_val.to_string();
            if !s.is_null() {
                return ::std::option::Option::Some(crate::jsstr_to_rust_string(cx, s));
            }
        }
    }
    ::std::option::Option::None
}

/// Valid-UTF-8 text → JSString through the UTF-8 decoder. JS_NewStringCopyZ
/// reads the buffer as Latin-1 (one byte = one char code), which mangles
/// multibyte UTF-8 (E4 BD A0 → "ä½ " instead of 你); the JSString must be
/// built via JS_NewStringCopyUTF8N — same discipline as the Buffer.toString
/// mojibake fix in globals.rs (@trace REQ-ENG-005).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn js_string_from_utf8(cx: *mut JSContext, text: &str) -> *mut JSString {
    let chars = mozjs::conversions::Utf8Chars::from(text);
    mozjs_sys::jsapi::JS_NewStringCopyUTF8N(
        cx,
        &*chars as *const _ as *const mozjs_sys::jsapi::JS::UTF8Chars,
    )
}

/// Decode `data` into the JS value node returns for the given encoding:
/// `None` → Buffer (binary-safe), `Some(enc)` → decoded String. Single source
/// for every read face (readFileSync/readFile/promises.readFile/FileHandle.
/// readFile) so encoding behavior cannot drift between them.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn content_jsval(
    cx: *mut JSContext,
    data: &[u8],
    encoding: ::std::option::Option<&str>,
) -> JSVal {
    match encoding {
        // @trace REQ-ENG-005 [entity:Buffer]
        // Node.js: readFileSync(path) with NO encoding returns a Buffer
        // (binary-safe). Only when an encoding is supplied does it return a
        // decoded String. Previously bao returned a utf8-lossy String for the
        // no-encoding case, breaking Buffer.isBuffer() checks downstream.
        None => {
            let buf_obj = crate::globals::create_buffer_object(cx, data);
            if buf_obj.is_null() {
                UndefinedValue()
            } else {
                mozjs::jsval::ObjectValue(buf_obj)
            }
        }
        Some("utf-8" | "utf8" | "text") => {
            // @trace REQ-ENG-005 — JS_NewStringCopyUTF8N (not CopyZ): multibyte
            // UTF-8 (中文/emoji) must decode through the UTF-8 decoder, not be
            // re-read as Latin-1 (mojibake). Same fix as buffer_to_string.
            let s = ::std::string::String::from_utf8_lossy(data);
            let js_str = js_string_from_utf8(cx, &s);
            if js_str.is_null() {
                UndefinedValue()
            } else {
                mozjs::jsval::StringValue(&*js_str)
            }
        }
        Some("hex") => {
            let hex: ::std::string::String = bun_core::fmt::bytes_to_hex_lower_string(data);
            let c_str = ZBox::from_bytes(hex.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() {
                UndefinedValue()
            } else {
                mozjs::jsval::StringValue(&*js_str)
            }
        }
        Some("base64") => {
            // @trace REQ-ENG-005 [algorithm:base64]
            // SIMD-accelerated base64 encode via workspace bun_base64 (replaces crates.io base64).
            let encoded_bytes = bun_base64::encode_alloc(data);
            let encoded = ::std::str::from_utf8(&encoded_bytes).unwrap_or("");
            let c_str = ZBox::from_bytes(encoded.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() {
                UndefinedValue()
            } else {
                mozjs::jsval::StringValue(&*js_str)
            }
        }
        Some("latin1" | "binary") => {
            // Node latin1: each byte maps to code point U+0000..U+00FF. The
            // `b as char` String holds those chars (UTF-8: 2 bytes for ≥0x80);
            // js_string_from_utf8 decodes them back to the single correct
            // char per byte. CopyZ would re-read those 2 bytes as Latin-1
            // (0xE5 → "Ã¥" mojibake).
            let s: ::std::string::String = data.iter().map(|&b| b as char).collect();
            let js_str = js_string_from_utf8(cx, &s);
            if js_str.is_null() {
                UndefinedValue()
            } else {
                mozjs::jsval::StringValue(&*js_str)
            }
        }
        Some(_) => {
            let s = ::std::string::String::from_utf8_lossy(data);
            let js_str = js_string_from_utf8(cx, &s);
            if js_str.is_null() {
                UndefinedValue()
            } else {
                mozjs::jsval::StringValue(&*js_str)
            }
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn return_string_content(
    cx: *mut JSContext,
    args: &CallArgs,
    data: &[u8],
    encoding: ::std::option::Option<&str>,
) -> bool {
    args.rval().set(content_jsval(cx, data, encoding));
    true
}

// ── write-file primitives ───────────────────────────────────────────────────
// bun_sys::fs::write / fs::OpenOptions pass MSVCRT `libc::O_*` flag values;
// on Windows those collide with the Linux-shaped `bun.O` bits that
// `uv::O::from_bun_o` bit-tests (libc::O_CREAT=0x100 matches no bun bit →
// CREAT silently dropped; libc::O_NOINHERIT(0x80) reads as bun::EXCL), so
// every create-write died with EINVAL ("os error 22") — the class already
// documented at shell_parser/parse.rs:668. std::fs carries the platform-
// correct semantics on every target, so the JS-facing write routes through
// it until bun_sys::fs grows cfg-correct flag constants.
fn fs_write_bytes(path: &str, bytes: &[u8]) -> ::std::io::Result<()> {
    ::std::fs::write(path, bytes)
}

/// create+append open (appendFileSync face). `write_all` because append
/// writes of large buffers may split across syscalls.
fn fs_append_bytes(path: &str, bytes: &[u8]) -> ::std::io::Result<()> {
    use ::std::io::Write;
    let mut file = ::std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(bytes)
}

/// Write a whole file honoring node's `options.flag` (C1 registry sibling:
/// writeFileSync ignored `flag` entirely — `fs_write_bytes` is an
/// unconditional truncate-write, so `{flag:"wx"}` silently clobbered an
/// existing file instead of failing EEXIST, and `{flag:"a"}` truncated
/// instead of appending). Node v24.19.0 oracle:
///   flag wx on existing → EEXIST (content preserved);
///   flag a  → appends; flag r+ → overwrite from position 0, no truncate;
///   flag r  → EBADF on write (open succeeds read-only).
/// Flags outside the node set fail loudly (EINVAL) instead of silently
/// degrading to the default truncate form.
fn write_file_with_flag(
    path: &str,
    bytes: &[u8],
    flag: &str,
) -> ::std::io::Result<()> {
    use ::std::io::Write;
    let mut opts = ::std::fs::OpenOptions::new();
    match flag {
        "w" | "w+" | "wx" | "wx+" => {
            opts.write(true).truncate(true);
            if flag.starts_with("wx") {
                opts.create_new(true);
            } else {
                opts.create(true);
            }
        }
        "a" | "a+" | "ax" | "ax+" | "as" | "as+" => {
            opts.write(true).append(true);
            if flag.starts_with("ax") {
                opts.create_new(true);
            } else {
                opts.create(true);
            }
        }
        "r+" | "rs+" | "sr+" => {
            opts.write(true);
        }
        "r" | "rs" => {
            // Node opens read-only, then the write syscall reports EBADF.
            return ::std::result::Result::Err(::std::io::Error::from_raw_os_error(
                libc::EBADF,
            ));
        }
        _ => {
            return ::std::result::Result::Err(::std::io::Error::new(
                ::std::io::ErrorKind::InvalidInput,
                format!("Unknown file open flag: {}", flag),
            ));
        }
    }
    let mut file = opts.open(path)?;
    file.write_all(bytes)
}

/// Coerce a positional fs argument to the node `number` contract. `null`,
/// `undefined`, and non-numeric values yield None so callers apply node's
/// defaults (offset 0 / length = rest / position = current fd cursor).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn fs_number_arg(v: JSVal) -> Option<f64> {
    if v.is_number() {
        Some(v.to_number())
    } else if v.is_int32() {
        Some(v.to_int32() as f64)
    } else {
        None
    }
}

/// `options[key]` as a string (GetProperty); None when absent/not a string.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn get_string_option(
    cx: *mut JSContext,
    args: &CallArgs,
    opt_index: u32,
    key: &str,
) -> Option<::std::string::String> {
    if args.argc_ <= opt_index {
        return None;
    }
    let opt_val = *args.get(opt_index).ptr;
    if !opt_val.is_object() {
        return None;
    }
    let mut wrapped_cx_opt =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref_opt = &mut wrapped_cx_opt;
    rooted!(&in(cx_ref_opt) let obj = opt_val.to_object());
    let c_key = ZBox::from_bytes(key.as_bytes());
    let mut val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj.handle().into(),
        c_key.as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut val,
        },
    );
    if val.is_string() {
        let s = val.to_string();
        if !s.is_null() {
            return Some(crate::jsstr_to_rust_string(cx, s));
        }
    }
    None
}

/// writeFile / writeFileSync options: node accepts either the encoding
/// shorthand string or an object carrying `encoding` and `flag`.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn write_file_options(
    cx: *mut JSContext,
    args: &CallArgs,
    opt_index: u32,
) -> (Option<::std::string::String>, Option<::std::string::String>) {
    let encoding = get_encoding_opt(cx, args, opt_index);
    let flag = get_string_option(cx, args, opt_index, "flag");
    (encoding, flag)
}

/// UTF-16 code units of a JSString (node's Buffer.from(string, enc) operates
/// on code units, not on the UTF-8 projection: latin1/hex/base64 take the
/// LOW BYTE of each unit and utf16le emits the units verbatim).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn js_string_code_units(
    cx: *mut JSContext,
    s: *mut JSString,
) -> ::std::vec::Vec<u16> {
    if s.is_null() {
        return ::std::vec::Vec::new();
    }
    if JS_DeprecatedStringHasLatin1Chars(s) {
        let mut length: usize = 0;
        let ptr = JS_GetLatin1StringCharsAndLength(cx, ::std::ptr::null(), s, &mut length);
        if ptr.is_null() {
            return ::std::vec::Vec::new();
        }
        let mut out = ::std::vec::Vec::with_capacity(length);
        for i in 0..length {
            out.push(unsafe { *ptr.add(i) } as u16);
        }
        out
    } else {
        let mut length: usize = 0;
        let ptr = JS_GetTwoByteStringCharsAndLength(cx, ::std::ptr::null(), s, &mut length);
        if ptr.is_null() {
            return ::std::vec::Vec::new();
        }
        unsafe { ::std::slice::from_raw_parts(ptr, length) }.to_vec()
    }
}

fn base64_alphabet_index(b: u8, url_safe: bool) -> ::std::option::Option<u8> {
    match b {
        b'A'..=b'Z' => ::std::option::Option::Some(b - b'A'),
        b'a'..=b'z' => ::std::option::Option::Some(b - b'a' + 26),
        b'0'..=b'9' => ::std::option::Option::Some(b - b'0' + 52),
        b'+' => ::std::option::Option::Some(62),
        b'/' => ::std::option::Option::Some(63),
        b'-' if url_safe => ::std::option::Option::Some(62),
        b'_' if url_safe => ::std::option::Option::Some(63),
        _ => ::std::option::Option::None,
    }
}

/// node `Buffer.from(str, 'base64'|'base64url')`: walk code units, keep the
/// low byte, skip non-alphabet units, stop at '=' (literal or low-byte 0x3D),
/// then decode 4→3.
fn base64_low_byte_decode_units(units: &[u16], url_safe: bool) -> ::std::vec::Vec<u8> {
    let mut cleaned: ::std::vec::Vec<u8> = ::std::vec::Vec::with_capacity(units.len());
    for &unit in units {
        let low = (unit & 0xFF) as u8;
        if low == b'=' {
            break;
        }
        if let ::std::option::Option::Some(idx) = base64_alphabet_index(low, url_safe) {
            cleaned.push(idx);
        }
    }
    let mut out = ::std::vec::Vec::with_capacity(cleaned.len() * 3 / 4);
    let mut i = 0;
    while i + 1 < cleaned.len() {
        let c0 = cleaned[i] as u32;
        let c1 = cleaned[i + 1] as u32;
        let triplet = (c0 << 18) | (c1 << 12);
        out.push(((triplet >> 16) & 0xFF) as u8);
        if i + 2 < cleaned.len() {
            let triplet = triplet | ((cleaned[i + 2] as u32) << 6);
            out.push(((triplet >> 8) & 0xFF) as u8);
            if i + 3 < cleaned.len() {
                let triplet = triplet | cleaned[i + 3] as u32;
                out.push((triplet & 0xFF) as u8);
            }
        }
        i += 4;
    }
    out
}

/// Node `Buffer.from(str, encoding)` byte projection. Single source for the
/// whole writeFile/appendFile family (sync/async/promises/FileHandle), so the
/// options.encoding honored on one face cannot drift from the others.
/// None/absent/unknown → UTF-8 (bao's long-standing permissive posture).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn encode_write_string(
    cx: *mut JSContext,
    s: *mut JSString,
    encoding: ::std::option::Option<&str>,
) -> ::std::vec::Vec<u8> {
    let enc = encoding.unwrap_or("").to_ascii_lowercase();
    match enc.as_str() {
        "latin1" | "binary" | "ascii" => js_string_code_units(cx, s)
            .iter()
            .map(|u| *u as u8)
            .collect(),
        "utf16le" | "utf-16le" | "ucs2" | "ucs-2" => {
            let units = js_string_code_units(cx, s);
            let mut out = ::std::vec::Vec::with_capacity(units.len() * 2);
            for unit in units {
                out.extend_from_slice(&unit.to_le_bytes());
            }
            out
        }
        "hex" => {
            // Pairs of low bytes; stop at the first non-hex pair and discard
            // a trailing odd digit (node's lenient decode, mirroring the
            // Buffer.from face).
            let units = js_string_code_units(cx, s);
            let mut out = ::std::vec::Vec::with_capacity(units.len() / 2);
            let mut i = 0;
            while i + 1 < units.len() {
                let hi = ((units[i] & 0xFF) as u8 as char).to_digit(16);
                let lo = ((units[i + 1] & 0xFF) as u8 as char).to_digit(16);
                match (hi, lo) {
                    (::std::option::Option::Some(h), ::std::option::Option::Some(l)) => {
                        out.push(((h << 4) | l) as u8);
                        i += 2;
                    }
                    _ => break,
                }
            }
            out
        }
        "base64" => base64_low_byte_decode_units(&js_string_code_units(cx, s), false),
        "base64url" => base64_low_byte_decode_units(&js_string_code_units(cx, s), true),
        _ => crate::jsstr_to_rust_string(cx, s).into_bytes(),
    }
}

/// Node's validateFunction message tail for the "Received ..." clause: the
/// last argument is the callback slot the caller failed to fill with a
/// function (`undefined` when absent, `type string ('utf8')` for the legacy
/// third-arg encoding form, `an instance of Object` for an options object).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn describe_received_fs_arg(cx: *mut JSContext, args: &CallArgs) -> ::std::string::String {
    if args.argc_ == 0 {
        return "undefined".to_string();
    }
    let v = *args.get(args.argc_ - 1).ptr;
    if v.is_undefined() {
        "undefined".to_string()
    } else if v.is_null() {
        "null".to_string()
    } else if v.is_string() {
        let s = v.to_string();
        if s.is_null() {
            "type string".to_string()
        } else {
            format!(
                "type string ({:?})",
                crate::jsstr_to_rust_string(cx, s)
            )
        }
    } else if v.is_int32() {
        format!("type number ({})", v.to_int32())
    } else if v.is_double() {
        format!("type number ({})", v.to_double())
    } else if v.is_boolean() {
        format!("type boolean ({})", v.to_boolean())
    } else if v.is_object() {
        "an instance of Object".to_string()
    } else {
        "an unexpected value".to_string()
    }
}

/// Data argument → bytes for the writeFile/appendFile family. Strings honor
/// options.encoding; Buffer/TypedArray/DataView/ArrayBuffer inputs are raw.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn write_file_data_bytes(
    cx: *mut JSContext,
    data_val: JSVal,
    encoding: ::std::option::Option<&str>,
) -> ::std::vec::Vec<u8> {
    if data_val.is_string() {
        let s = data_val.to_string();
        if s.is_null() {
            ::std::vec::Vec::new()
        } else {
            encode_write_string(cx, s, encoding)
        }
    } else if data_val.is_object() {
        crate::node_crypto::extract_buffer_bytes(cx, data_val)
    } else {
        ::std::vec::Vec::new()
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn throw_fs_error(cx: *mut JSContext, op: &str, path: &str, err: &::std::io::Error) -> bool {
    // @trace REQ-ENG-007 — shared errno→code source (io_error_code); the
    // EINVAL verbatim rule and the ENAMETOOLONG mapping (upstream 83350172b9)
    // live there so sync throw / callback / promise faces stay identical.
    let code = io_error_code(err);
    let msg = format!("{} '{}': {}", op, path, err);
    let c_msg = ZBox::from_bytes(msg.as_bytes());
    let code_str = JS_NewStringCopyZ(cx, ZBox::from_bytes(code.as_bytes()).as_ptr());
    if !code_str.is_null() {
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        if JS_IsExceptionPending(cx) {
            rooted!(in(cx) let mut exn = UndefinedValue());
            JS_GetPendingException(cx, exn.handle_mut().into());
            let exn_val = exn.get();
            if !exn_val.is_undefined() && exn_val.is_object() {
                rooted!(in(cx) let exn_obj = exn_val.to_object());
                rooted!(in(cx) let code_val = StringValue(&*code_str));
                JS_DefineProperty(
                    cx,
                    exn_obj.handle().into(),
                    c"code".as_ptr(),
                    code_val.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
                let path_val = ZBox::from_bytes(path.as_bytes());
                let path_str = JS_NewStringCopyZ(cx, path_val.as_ptr());
                if !path_str.is_null() {
                    rooted!(in(cx) let path_v = StringValue(&*path_str));
                    JS_DefineProperty(
                        cx,
                        exn_obj.handle().into(),
                        c"path".as_ptr(),
                        path_v.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
                JS_SetPendingException(
                    cx,
                    exn.handle().into(),
                    ExceptionStackBehavior::DoNotCapture,
                );
            }
        }
    } else {
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
    }
    false
}

// --- Sync file operations ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_read_file_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_read(&path) {
        let c_msg = ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    let encoding = get_encoding_opt(cx, &args, 1);
    match bun_fs::read(&path) {
        ::std::result::Result::Ok(data) => {
            return_string_content(cx, &args, &data, encoding.as_deref())
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "readFileSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_write_file_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&path) {
        let c_msg = ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    let data_val = if argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };
    // Node: writeFileSync(file, data[, options]) — options.flag is honored
    // (C1/C11 registry: it was ignored, always truncate-write) and a string
    // data argument is encoded per options.encoding (node v24.19.0 oracle:
    // latin1 "é" → 0xE9, hex "6162" → 0x61 0x62).
    let (encoding, flag) = write_file_options(cx, &args, 2);
    let flag = flag.unwrap_or_else(|| "w".to_string());

    let bytes = write_file_data_bytes(cx, data_val, encoding.as_deref());
    let result = write_file_with_flag(&path, &bytes, &flag);

    match result {
        ::std::result::Result::Ok(()) => {
            args.rval().set(UndefinedValue());
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "writeFileSync", &path, &e),
    }
}

// ── createWriteStream binary-safe flush ─────────────────────────────────────
// The FS_STREAM_JS polyfill buffers string and Buffer chunks RAW; at end()
// it hands the chunk array here instead of String()-coercing every chunk.
// Byte semantics: string chunks → their UTF-8 bytes (byte-identical to the
// old join('') + writeFileSync(string) path — UTF-8 concatenation is
// stable); Buffer/TypedArray/DataView/ArrayBuffer chunks → raw bytes via
// collect_byte_view. The old polyfill ran Buffer chunks through
// String(chunk), which is binary-unsafe (bytes ≥ 0x80 re-encoded through
// lossy UTF-8 → corrupted file). Unrecognized chunk types throw instead of
// silently corrupting the stream (Node throws ERR_INVALID_ARG_TYPE).
// @trace REQ-ENG-005 [entity:Buffer] [api:createWriteStream]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_write_stream_flush(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&path) {
        let c_msg = ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }

    let mut total: Vec<u8> = Vec::new();
    let chunks_val = if argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };
    if chunks_val.is_object() {
        let mut wrapped_cx =
            mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let arr_root = chunks_val.to_object());
        let mut len_val = UndefinedValue();
        JS_GetProperty(
            cx,
            arr_root.handle().into(),
            c"length".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut len_val,
            },
        );
        let len = if len_val.is_int32() && len_val.to_int32() > 0 {
            len_val.to_int32() as u32
        } else {
            0
        };
        for i in 0..len {
            let mut elem = UndefinedValue();
            JS_GetElement(
                cx,
                arr_root.handle().into(),
                i,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut elem,
                },
            );
            rooted!(&in(cx_ref) let el_root = elem);
            let v = el_root.get();
            if v.is_string() {
                let s = v.to_string();
                if !s.is_null() {
                    total.extend_from_slice(crate::jsstr_to_rust_string(cx, s).as_bytes());
                }
            } else if v.is_object() {
                match crate::node_buffer::collect_byte_view(cx, v) {
                    ::std::option::Option::Some(bytes) => total.extend_from_slice(&bytes),
                    // fail-closed: an unrecognized object would previously be
                    // written as "[object Object]" garbage — throw instead.
                    ::std::option::Option::None => {
                        JS_ReportErrorUTF8(
                            cx,
                            c"The \"chunk\" argument must be a string or a binary view".as_ptr(),
                        );
                        return false;
                    }
                }
            } else {
                JS_ReportErrorUTF8(
                    cx,
                    c"The \"chunk\" argument must be a string or a binary view".as_ptr(),
                );
                return false;
            }
        }
    }

    match fs_write_bytes(&path, &total) {
        ::std::result::Result::Ok(()) => {
            args.rval().set(UndefinedValue());
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "createWriteStream", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_append_file_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&path) {
        let c_msg = ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    let data_val = if argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };
    // appendFile(string, options.encoding) — same encode face as writeFile.
    let encoding = get_encoding_opt(cx, &args, 2);
    let data = write_file_data_bytes(cx, data_val, encoding.as_deref());

    match fs_append_bytes(&path, &data) {
        ::std::result::Result::Ok(()) => {
            args.rval().set(UndefinedValue());
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "appendFileSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_exists_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    args.rval()
        .set(mozjs::jsval::BooleanValue(Path::new(&path).exists()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_mkdir_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&path) {
        let c_msg = ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    let recursive = get_bool_option(cx, &args, 1, "recursive");
    let result = if recursive {
        fs::create_dir_all(&path)
    } else {
        fs::create_dir(&path)
    };
    match result {
        ::std::result::Result::Ok(()) => {
            args.rval().set(UndefinedValue());
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "mkdirSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_readdir_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let with_file_types = get_bool_option(cx, &args, 1, "withFileTypes");

    match fs::read_dir(&path) {
        ::std::result::Result::Ok(entries) => {
            let mut names: Vec<::std::string::String> = Vec::new();
            let mut is_dirs: Vec<bool> = Vec::new();
            for entry in entries.flatten() {
                names.push(entry.file_name().to_string_lossy().into_owned());
                is_dirs.push(entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false));
            }
            // SAFETY: construct wrapped cx to use rooted! and w2:: functions
            let mut wrapped_cx = unsafe {
                mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx))
            };
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let arr = unsafe { w2::NewArrayObject1(cx_ref, names.len()) });
            if arr.get().is_null() {
                args.rval().set(UndefinedValue());
                return true;
            }
            for (i, name) in names.iter().enumerate() {
                if with_file_types {
                    let dirent = create_dirent(cx, name, is_dirs[i]);
                    if !dirent.is_null() {
                        rooted!(&in(cx_ref) let val = mozjs::jsval::ObjectValue(dirent));
                        unsafe {
                            JS_DefineElement(
                                cx,
                                arr.handle().into(),
                                i as u32,
                                val.handle().into(),
                                JSPROP_ENUMERATE as u32,
                            );
                        }
                    }
                } else {
                    let c_name = ZBox::from_bytes(name.as_bytes());
                    let js_str = unsafe { JS_NewStringCopyZ(cx, c_name.as_ptr()) };
                    if !js_str.is_null() {
                        rooted!(&in(cx_ref) let val = mozjs::jsval::StringValue(&*js_str));
                        unsafe {
                            JS_DefineElement(
                                cx,
                                arr.handle().into(),
                                i as u32,
                                val.handle().into(),
                                JSPROP_ENUMERATE as u32,
                            );
                        }
                    }
                }
            }
            args.rval().set(mozjs::jsval::ObjectValue(arr.get()));
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "readdirSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_stat_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    match bun_fs::metadata(&path) {
        ::std::result::Result::Ok(meta) => {
            let stats = create_stats_object(cx, &meta);
            args.rval().set(mozjs::jsval::ObjectValue(stats));
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "statSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_lstat_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    match fs::symlink_metadata(&path) {
        ::std::result::Result::Ok(meta) => {
            let posix = metadata_to_posix_stat(&meta);
            let stats = create_stats_object(cx, &posix);
            args.rval().set(mozjs::jsval::ObjectValue(stats));
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "lstatSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_unlink_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&path) {
        let c_msg = ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    match fs::remove_file(&path) {
        ::std::result::Result::Ok(()) => {
            args.rval().set(UndefinedValue());
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "unlinkSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_rmdir_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&path) {
        let c_msg = ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    // Upstream fff3b29d6: a truthy `options.recursive` routes rmdir through
    // rm semantics (recursive tree removal, ENOENT for a missing path);
    // absent/false keeps plain rmdir (ENOTEMPTY on non-empty, ENOTDIR on a
    // file path). Same option parse + op selection as fs_rm_sync.
    let recursive = get_bool_option(cx, &args, 1, "recursive");
    let result = if recursive {
        fs::remove_dir_all(&path).map_err(|e| {
            // node rmdirSync(missing, {recursive:true}) throws ENOENT (rm's
            // force defaults false); std's remove_dir_all tolerates a
            // missing root (rm -rf shape) and answers Ok — reshape the miss.
            if e.kind() == ::std::io::ErrorKind::NotFound {
                ::std::io::Error::new(
                    ::std::io::ErrorKind::NotFound,
                    "ENOENT: no such file or directory",
                )
            } else {
                e
            }
        })
    } else {
        fs::remove_dir(&path).map_err(|e| {
            // node's rmdir contract is SHAPE-based, and the Windows errno
            // shapes differ: RemoveDirectory on a file reports
            // ERROR_DIRECTORY ("not a directory" facts, but std's kind is
            // not NotADirectory on every path) and on a non-empty dir can
            // surface ERROR_ACCESS_DENIED under handle contention. Decide
            // by the path's actual shape: file → ENOTDIR, live directory →
            // ENOTEMPTY; anything else (missing → ENOENT) passes through.
            match fs::symlink_metadata(&path) {
                Ok(m) if m.is_file() => ::std::io::Error::new(
                    ::std::io::ErrorKind::NotADirectory,
                    "ENOTDIR: not a directory",
                ),
                Ok(m) if m.is_dir() => ::std::io::Error::other(
                    "ENOTEMPTY: directory not empty",
                ),
                _ => e,
            }
        })
    };
    match result {
        ::std::result::Result::Ok(()) => {
            args.rval().set(UndefinedValue());
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "rmdirSync", &path, &e),
    }
}

/// Node `rm(path[, options])` / `rmSync` core. `force: true` silences ENOENT
/// only (node: "exceptions are ignored if path does not exist"); every other
/// errno still surfaces. C1 #4 / C11 registry: bao ignored the force flag and
/// threw ENOENT for a missing path (node v24.19.0 oracle: NOTHROW).
fn rm_with_options(path: &str, recursive: bool, force: bool) -> ::std::io::Result<()> {
    let result = if recursive {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    match result {
        ::std::result::Result::Err(e)
            if force && e.kind() == ::std::io::ErrorKind::NotFound =>
        {
            ::std::result::Result::Ok(())
        }
        other => other,
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_rm_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&path) {
        let c_msg = ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    let recursive = get_bool_option(cx, &args, 1, "recursive");
    let force = get_bool_option(cx, &args, 1, "force");
    match rm_with_options(&path, recursive, force) {
        ::std::result::Result::Ok(()) => {
            args.rval().set(UndefinedValue());
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "rmSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_rename_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let from = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let to = match get_path_arg(cx, &args, 1) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_read(&from) {
        let c_msg = ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&to) {
        let c_msg = ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    match fs::rename(&from, &to) {
        ::std::result::Result::Ok(()) => {
            args.rval().set(UndefinedValue());
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "renameSync", &from, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_copy_file_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let from = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let to = match get_path_arg(cx, &args, 1) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_read(&from) {
        let c_msg = ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&to) {
        let c_msg = ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    match fs::copy(&from, &to) {
        ::std::result::Result::Ok(_) => {
            args.rval().set(UndefinedValue());
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "copyFileSync", &from, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_chmod_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let mode_val = if argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };
    let mode = if mode_val.is_int32() {
        mode_val.to_int32() as u32
    } else if mode_val.is_double() {
        mode_val.to_double() as u32
    } else {
        0o644
    };
    #[cfg(unix)]
    let result = {
        use ::std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(mode))
    };
    #[cfg(not(unix))]
    let result = set_win_readonly(&path, mode as i32);
    match result {
        ::std::result::Result::Ok(()) => {
            args.rval().set(UndefinedValue());
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "chmodSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_realpath_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    match fs::canonicalize(&path) {
        ::std::result::Result::Ok(resolved) => {
            let s = resolved.to_string_lossy();
            let c_str = ZBox::from_bytes(s.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() {
                args.rval().set(UndefinedValue());
            } else {
                args.rval().set(mozjs::jsval::StringValue(&*js_str));
            }
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "realpathSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_readlink_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    match fs::read_link(&path) {
        ::std::result::Result::Ok(target) => {
            let s = target.to_string_lossy();
            let c_str = ZBox::from_bytes(s.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() {
                args.rval().set(UndefinedValue());
            } else {
                args.rval().set(mozjs::jsval::StringValue(&*js_str));
            }
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "readlinkSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_symlink_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let target = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let path = match get_path_arg(cx, &args, 1) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    #[cfg(unix)]
    let result = ::std::os::unix::fs::symlink(&target, &path);
    #[cfg(not(unix))]
    let result = fs::hard_link(&target, &path);
    match result {
        ::std::result::Result::Ok(()) => {
            args.rval().set(UndefinedValue());
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "symlinkSync", &target, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_link_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let from = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let to = match get_path_arg(cx, &args, 1) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    match fs::hard_link(&from, &to) {
        ::std::result::Result::Ok(()) => {
            args.rval().set(UndefinedValue());
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "linkSync", &from, &e),
    }
}

// Recursive directory copy. Mirrors Node.js fs.cpSync(src, dst[, opts])
// behaviour for the common recursive case.
// Supports errorOnExist option: throw if destination exists and is not a directory.
#[allow(unsafe_op_in_unsafe_fn)]
fn cp_recursive(src: &Path, dst: &Path, error_on_exist: bool) -> ::std::io::Result<()> {
    if fs::metadata(src)?.is_dir() {
        if error_on_exist && dst.exists() && !dst.is_dir() {
            return Err(::std::io::Error::new(
                ::std::io::ErrorKind::AlreadyExists,
                "destination already exists",
            ));
        }
        fs::create_dir_all(dst)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            let from = entry.path();
            let to = dst.join(entry.file_name());
            cp_recursive(&from, &to, error_on_exist)?;
        }
    } else {
        fs::copy(src, dst)?;
    }
    ::std::result::Result::Ok(())
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_cp_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let from = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let to = match get_path_arg(cx, &args, 1) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let error_on_exist = get_bool_option(cx, &args, 2, "errorOnExist");
    match cp_recursive(Path::new(&from), Path::new(&to), error_on_exist) {
        ::std::result::Result::Ok(()) => {
            args.rval().set(UndefinedValue());
            true
        }
        ::std::result::Result::Err(e) => {
            let msg = format!("cpSync: {}", e);
            let c_msg = ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            false
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_cp(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    // Async variant: invoke callback (if provided) after the recursive copy.
    let args = CallArgs::from_vp(vp, argc);
    let from = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let to = match get_path_arg(cx, &args, 1) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let error_on_exist = get_bool_option(cx, &args, 2, "errorOnExist");

    // Node: fs.cp(src, dest[, options], callback). The callback branch was
    // missing (C1 #17: a supplied cb was never delivered — the copy ran
    // synchronously and the completion was silent). Deliver on the carrier
    // with the node-shaped (err) contract, including copy failures.
    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 2) {
        spawn_fs_async(cx, "cp", from.clone(), callback, None, move || {
            cp_recursive(Path::new(&from), Path::new(&to), error_on_exist)
                .map(|_| FsAsyncResult::OkVoid)
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    let res = cp_recursive(Path::new(&from), Path::new(&to), error_on_exist);
    if let ::std::result::Result::Err(e) = res {
        let msg = format!("cp: {}", e);
        let c_msg = ZBox::from_bytes(msg.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_watch(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };

    // options may be an object (position 1) or an encoding string; the
    // listener then sits at position 1 or 2 (Node: watch(filename[, options][, listener])).
    let mut persistent = true;
    // recursive option: honored on windows (bWatchSubtree semantics via
    // fsw_recursive_mode); linux throws ERR_FEATURE_UNAVAILABLE below before
    // the backend is reached.
    #[cfg(windows)]
    let mut recursive = false;
    #[cfg(not(windows))]
    let recursive = false;
    let mut listener_val = UndefinedValue();
    if _argc > 1 {
        let opt_val = *args.get(1).ptr;
        // Node overload: watch(filename[, options][, listener]) — a FUNCTION
        // at position 1 is the listener, not the options object.
        let opt_is_fn = opt_val.is_object()
            && unsafe { mozjs_sys::jsapi::js::IsFunctionObject(opt_val.to_object()) };
        if opt_val.is_object() && !opt_is_fn {
            let mut wrapped_opt =
                mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
            let cx_opt = &mut wrapped_opt;
            rooted!(&in(cx_opt) let opt_obj = opt_val.to_object());
            // recursive: Node on Linux without recursive inotify support throws
            // ERR_FEATURE_UNAVAILABLE_ON_PLATFORM — explicit, never a silent fake.
            let mut recursive_v = UndefinedValue();
            JS_GetProperty(
                cx,
                opt_obj.handle().into(),
                c"recursive".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut recursive_v,
                },
            );
            #[cfg(not(windows))]
            if recursive_v.is_boolean() && recursive_v.to_boolean() {
                JS_ReportErrorUTF8(
                    cx,
                    c"The value of \"options.recursive\" is not supported on this platform: watch recursive is unavailable (inotify backend covers the watched path only)".as_ptr(),
                );
                return false;
            }
            // windows: notify's ReadDirectoryChangesW backend covers the whole
            // subtree natively — recursive fs.watch takes the upstream windows
            // semantics (fsw_recursive_mode → RecursiveMode::Recursive below).
            // (pre-wave gap fix: the option value was read but never assigned,
            // so windows recursive watches silently ran NonRecursive.)
            #[cfg(windows)]
            if recursive_v.is_boolean() {
                recursive = recursive_v.to_boolean();
            }
            let mut persistent_v = UndefinedValue();
            JS_GetProperty(
                cx,
                opt_obj.handle().into(),
                c"persistent".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut persistent_v,
                },
            );
            if persistent_v.is_boolean() {
                persistent = persistent_v.to_boolean();
            }
            if _argc > 2 {
                listener_val = *args.get(2).ptr;
            }
        } else {
            listener_val = opt_val;
        }
    }

    let is_dir = Path::new(&path).metadata().map(|m| m.is_dir()).unwrap_or(false);

    // Register a real watch (kernel events via notify — not polling).
    let id = fsw_next_id();
    let registered = fsw_add_watch_ex(&path, recursive);
    match registered {
        ::std::result::Result::Ok(notify_root) => {
            fsw_register_watch(id, path.clone(), notify_root, is_dir, recursive, persistent);
            let watcher = fsw_make_watcher_object(cx, id, is_dir);
            if watcher.is_null() {
                args.rval().set(UndefinedValue());
                return true;
            }
            let mut wrapped_cx =
                mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let watcher_r = watcher);
            if listener_val.is_object() {
                // Attach the listener via the object's own `on('change', fn)`.
                let c_ev = ZBox::from_bytes("change".as_bytes());
                let c_name = ZBox::from_bytes("on".as_bytes());
                rooted!(&in(cx_ref) let lv = listener_val);
                let ev_str = JS_NewStringCopyZ(cx, c_ev.as_ptr());
                if !ev_str.is_null() {
                    let argv = [mozjs::jsval::StringValue(&*ev_str), lv.get()];
                    let call_args = HandleValueArray {
                        length_: argv.len(),
                        elements_: argv.as_ptr(),
                    };
                    let mut rval = UndefinedValue();
                    JS_CallFunctionName(
                        cx,
                        watcher_r.handle().into(),
                        c_name.as_ptr(),
                        &call_args,
                        MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &mut rval,
                        },
                    );
                }
            }
            args.rval().set(mozjs::jsval::ObjectValue(watcher_r.get()));
            true
        }
        ::std::result::Result::Err(e) => {
            let _ = fsw_maybe_shutdown_if_idle();
            throw_fs_error(cx, "watch", &path, &e)
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_watch_file(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };

    // watchFile(filename[, options], listener): options at 1, listener at 1/2.
    let mut interval_ms: u64 = 5007; // Node default
    let mut persistent = true;
    let mut listener_val = UndefinedValue();
    if _argc > 1 {
        let opt_val = *args.get(1).ptr;
        // Node overload: watch(filename[, options][, listener]) — a FUNCTION
        // at position 1 is the listener, not the options object.
        let opt_is_fn = opt_val.is_object()
            && unsafe { mozjs_sys::jsapi::js::IsFunctionObject(opt_val.to_object()) };
        if opt_val.is_object() && !opt_is_fn {
            let mut wrapped_opt =
                mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
            let cx_opt = &mut wrapped_opt;
            rooted!(&in(cx_opt) let opt_obj = opt_val.to_object());
            let mut interval_v = UndefinedValue();
            JS_GetProperty(
                cx,
                opt_obj.handle().into(),
                c"interval".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut interval_v,
                },
            );
            if interval_v.is_number() {
                // JSVals carry ints as int32-or-double tags; to_double asserts
                // the double tag — convert through the numeric union instead.
                interval_ms = if interval_v.is_int32() {
                    interval_v.to_int32().max(1) as u64
                } else {
                    interval_v.to_double().max(1.0) as u64
                };
            }
            let mut persistent_v = UndefinedValue();
            JS_GetProperty(
                cx,
                opt_obj.handle().into(),
                c"persistent".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut persistent_v,
                },
            );
            if persistent_v.is_boolean() {
                persistent = persistent_v.to_boolean();
            }
            if _argc > 2 {
                listener_val = *args.get(2).ptr;
            }
        } else {
            listener_val = opt_val;
        }
    }

    let id = fsw_next_id();
    // Baseline stat on the JS thread; missing file = zeroed stat (fires once
    // when the file appears — Node parity).
    let baseline = fsw_stat_path(&path).unwrap_or_else(fsw_zero_stat);
    fsw_register_poll(id, path.clone(), interval_ms, baseline, persistent);

    let watcher = fsw_make_watcher_object(cx, id, false);
    if watcher.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let watcher_r = watcher);
    if listener_val.is_object() {
        let c_ev = ZBox::from_bytes("change".as_bytes());
        let c_name = ZBox::from_bytes("on".as_bytes());
        rooted!(&in(cx_ref) let lv = listener_val);
        let ev_str = JS_NewStringCopyZ(cx, c_ev.as_ptr());
        if !ev_str.is_null() {
            let argv = [mozjs::jsval::StringValue(&*ev_str), lv.get()];
            let call_args = HandleValueArray {
                length_: argv.len(),
                elements_: argv.as_ptr(),
            };
            let mut rval = UndefinedValue();
            JS_CallFunctionName(
                cx,
                watcher_r.handle().into(),
                c_name.as_ptr(),
                &call_args,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut rval,
                },
            );
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(watcher_r.get()));
    true
}

/// fs.unwatchFile(filename[, listener]) — stop watchFile polling for `filename`.
/// Node semantics: with a listener, remove that listener from each matching
/// StatWatcher and close watchers left with no 'change' listeners; without,
/// close every StatWatcher on the path.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_unwatch_file(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let listener_val = if _argc > 1 { *args.get(1).ptr } else { UndefinedValue() };
    let canonical = fsw_canonicalize(&path);
    let ids: Vec<u64> = FSW_POLLERS.with(|p| {
        p.borrow()
            .iter()
            .filter(|e| e.path == canonical)
            .map(|e| e.id)
            .collect()
    });
    for id in ids {
        if listener_val.is_object() {
            // Remove just this listener; close only if 'change' went quiet.
            if let Some(obj) = crate::gc_store::gc_store_get_ns(cx, "fswatch", &format!("w{}", id))
            {
                let mut wrapped_cx =
                    mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
                let cx_ref = &mut wrapped_cx;
                rooted!(&in(cx_ref) let obj_r = obj);
                let c_rm = ZBox::from_bytes("removeListener".as_bytes());
                let c_ev = ZBox::from_bytes("change".as_bytes());
                rooted!(&in(cx_ref) let lv = listener_val);
                let ev_str = JS_NewStringCopyZ(cx, c_ev.as_ptr());
                if !ev_str.is_null() {
                    let argv = [mozjs::jsval::StringValue(&*ev_str), lv.get()];
                    let call_args = HandleValueArray {
                        length_: argv.len(),
                        elements_: argv.as_ptr(),
                    };
                    let mut rval = UndefinedValue();
                    JS_CallFunctionName(
                        cx,
                        obj_r.handle().into(),
                        c_rm.as_ptr(),
                        &call_args,
                        MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &mut rval,
                        },
                    );
                }
                // Still has 'change' listeners? keep polling — ask the events
                // module's static listenerCount(emitter, event) (public JS
                // surface; no EE internals touched from here).
                if let Some(events_mod) = crate::require::get_builtin(cx, "events") {
                    if !events_mod.is_null() {
                        rooted!(&in(cx_ref) let em_r = events_mod);
                        let c_cnt = ZBox::from_bytes("listenerCount".as_bytes());
                        let ev2_str = JS_NewStringCopyZ(cx, c_ev.as_ptr());
                        if !ev2_str.is_null() {
                            let argv = [mozjs::jsval::ObjectValue(obj_r.get()), mozjs::jsval::StringValue(&*ev2_str)];
                            let call_args = HandleValueArray {
                                length_: argv.len(),
                                elements_: argv.as_ptr(),
                            };
                            let mut cnt = UndefinedValue();
                            let ok = JS_CallFunctionName(
                                cx,
                                em_r.handle().into(),
                                c_cnt.as_ptr(),
                                &call_args,
                                MutableHandle::<Value> {
                                    _phantom_0: ::std::marker::PhantomData,
                                    ptr: &mut cnt,
                                },
                            );
                            if ok && cnt.is_int32() && cnt.to_int32() > 0 {
                                continue;
                            }
                        }
                    }
                }
            }
        }
        fsw_close_entry(cx, id);
    }
    args.rval().set(UndefinedValue());
    true
}

// ══════════════════════════════════════════════════════════════════════════
// fs.watch / fs.watchFile backend (BCE: silent-fake eradication)

/// windows chmod: node/libuv maps the mode's write bits to
/// FILE_ATTRIBUTE_READONLY (every other permission bit has no storage there).
#[cfg(windows)]
fn set_win_readonly(path: &str, mode: i32) -> ::std::io::Result<()> {
    use ::std::os::windows::ffi::OsStrExt;
    const FILE_ATTRIBUTE_READONLY: u32 = 0x1;
    const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub safe fn SetFileAttributesW(lpfilename: *const u16, dwfileattributes: u32) -> i32;
    }
    let wide: Vec<u16> = ::std::ffi::OsStr::new(path)
        .encode_wide()
        .chain(::std::iter::once(0))
        .collect();
    let attrs = if mode & 0o222 == 0 {
        FILE_ATTRIBUTE_READONLY
    } else {
        FILE_ATTRIBUTE_NORMAL
    };
    if unsafe { SetFileAttributesW(wide.as_ptr(), attrs) } != 0 {
        Ok(())
    } else {
        Err(::std::io::Error::last_os_error())
    }
}

// windows libc lacks the S_IF* family; carry the canonical POSIX values (the
// windows stat face synthesizes these type bits onto st_mode).
#[cfg(windows)]
const S_IFMT: i32 = 0o170000;
#[cfg(windows)]
const S_IFREG: i32 = 0o100000;
#[cfg(windows)]
const S_IFDIR: i32 = 0o040000;
#[cfg(windows)]
const S_IFLNK: i32 = 0o120000;
#[cfg(windows)]
const S_IFCHR: i32 = 0o020000;
#[cfg(windows)]
const S_IFIFO: i32 = 0o010000;
#[cfg(windows)]
const S_IFBLK: i32 = 0o060000;
#[cfg(windows)]
const S_IFSOCK: i32 = 0o140000;
//
// Architecture (single-threaded JS model + notify 8.2 backend — task #69
// functional replacement wave): the two hand-written OS protocol backends
// (linux inotify struct decode + windows OVERLAPPED/ReadDirectoryChangesW
// raw FFI protocol) are replaced by notify's RecommendedWatcher (inotify on
// linux / ReadDirectoryChangesW on windows — the same kernel primitives,
// decoded by the maintained library). The Node semantic faces are retained:
//   * fs.watch → notify RecommendedWatcher. notify runs its own internal
//     thread; the handler below (plain data only — no JS types crosses
//     threads) translates the EventKind to the Node event name and enqueues
//     a PendingFsEvent through the Mutex<VecDeque> (cross-thread share:
//     Mutex is the sanctioned tool per 去锁化). JS callbacks fire on the JS
//     thread from `fs_watch_pump_all` (driven by timers::drain_and_check /
//     drain_one_pass, same integration point as web_api::ws_pump_all).
//   * fs.watchFile → stat polling at the caller's interval on the fsw
//     worker thread (Condvar::wait_timeout on the soonest deadline).
//   * File-level fs.watch subscribes the file's PARENT directory and
//     filters by basename in the pump's fan-out matcher (notify's windows
//     backend is directory-grained; the unified parent-dir subscription is
//     one code path on both platforms).
//   * notify reports no watch refcounts (a same-path re-watch overwrites
//     the kernel watch), so multi-watcher fan-out and refcounted teardown
//     stay at this layer: every FswWatchEntry carries its notify root,
//     events fan out to ALL entries matching the event path, and the
//     backend watch is removed only when the LAST watcher on that root
//     closes (the same contract the former wd_map carried).
//   * Liveness: a persistent watcher keeps the eval loop alive via
//     `fs_watch_loop_alive` (Node semantics: persistent=true keeps the
//     process alive; persistent:false delivers events only while the loop
//     is alive for other reasons).
// ══════════════════════════════════════════════════════════════════════════

/// Plain-data event marshalled worker/notify thread → JS thread. No JS types cross threads.
enum PendingFsEvent {
    /// notify backend event for fs.watch entries: the Node event name
    /// ("change"/"rename", translated from the notify EventKind on the
    /// notify thread) plus the affected paths reported by the backend.
    Notify {
        event_type: &'static str,
        paths: Vec<PathBuf>,
    },
    /// watchFile poll detected a stat change (or ENOENT → zeroed curr).
    StatChange {
        id: u64,
        prev: libc::stat,
        curr: libc::stat,
    },
}

enum FswCommand {
    AddPoll {
        id: u64,
        path: PathBuf,
        interval_ms: u64,
        baseline: libc::stat,
    },
    RemovePoll { id: u64 },
    Shutdown,
}

struct FswShared {
    queue: VecDeque<PendingFsEvent>,
    commands: VecDeque<FswCommand>,
}

impl FswShared {
    fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            commands: VecDeque::new(),
        }
    }
}

/// JS-thread watcher registration (fs.watch entries).
struct FswWatchEntry {
    id: u64,
    /// The path as the JS caller passed it: file watches report its basename
    /// as the event filename (Node passes the basename, never null).
    path: PathBuf,
    /// The notify root actually registered — the watched directory itself
    /// for directory watches, the parent directory for file watches. The
    /// pump's fan-out matcher routes backend events by this root, and the
    /// refcounted teardown counts entries per root.
    notify_root: PathBuf,
    is_dir: bool,
    /// windows only: bWatchSubtree recursion over the notify root (linux
    /// recursive never reaches the backend — the JS face throws).
    recursive: bool,
    persistent: bool,
}

/// JS-thread poller registration (fs.watchFile entries).
struct FswPollEntry {
    id: u64,
    path: PathBuf,
    persistent: bool,
}

thread_local! {
    static FSW_NEXT_ID: Cell<u64> = const { Cell::new(1) };
    static FSW_WATCHERS: RefCell<Vec<FswWatchEntry>> = const { RefCell::new(Vec::new()) };
    static FSW_POLLERS: RefCell<Vec<FswPollEntry>> = const { RefCell::new(Vec::new()) };
    /// Hub: notify watcher + shared queues + stat-poll worker. Materialized
    /// on the first watcher, torn down (joined) when the last entry closes.
    static FSW_HUB: RefCell<Option<FswHub>> = const { RefCell::new(None) };
}

/// Hub: the notify watcher (inotify / ReadDirectoryChangesW backend with its
/// own internal thread) + the shared queues + the stat-poll worker.
struct FswHub {
    shared: Arc<Mutex<FswShared>>,
    /// Stat-poll deadline signal (watchFile face). The notify thread needs
    /// no wake — the JS-thread pump drains the queue on the timer cadence.
    poll_cv: Arc<::std::sync::Condvar>,
    watcher: notify::RecommendedWatcher,
    handle: Option<::std::thread::JoinHandle<()>>,
}

impl Drop for FswHub {
    fn drop(&mut self) {
        // Signal shutdown, wake the worker, join it. The notify watcher drops
        // right after: its Drop retires the backend (removes every kernel
        // watch and shuts its internal thread down).
        if let Ok(mut shared) = self.shared.lock() {
            shared.commands.push_back(FswCommand::Shutdown);
        }
        self.poll_cv.notify_all();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn fsw_next_id() -> u64 {
    FSW_NEXT_ID.with(|c| {
        let v = c.get();
        c.set(v + 1);
        v
    })
}

fn fsw_zero_stat() -> libc::stat {
    // SAFETY: libc::stat is plain POD; zeroed() is the "file absent" sentinel
    // (Node passes zeroed Stats when the watched file is gone).
    unsafe { ::std::mem::zeroed() }
}

fn fsw_stat_path(path: &str) -> ::std::option::Option<libc::stat> {
    let c_path = bun_core::ZBox::from_bytes(path.as_bytes());
    let mut st: libc::stat = fsw_zero_stat();
    // SAFETY: c_path is NUL-terminated; st is writable POD of the size stat expects.
    let rc = unsafe { libc::stat(c_path.as_ptr() as *const ::std::ffi::c_char, &mut st) };
    if rc == 0 {
        Some(st)
    } else {
        None
    }
}

fn fsw_stat_changed(a: &libc::stat, b: &libc::stat) -> bool {
    // windows CRT stat carries whole-second times only.
    #[cfg(windows)]
    let (a_nsec, b_nsec): (i32, i32) = (0, 0);
    #[cfg(not(windows))]
    let (a_nsec, b_nsec) = (a.st_mtime_nsec, b.st_mtime_nsec);
    // Node/libuv uv_fs_poll change predicate: size, mtime (ns), ino, mode.
    a.st_size != b.st_size
        || a.st_mtime != b.st_mtime
        || a_nsec != b_nsec
        || a.st_ino != b.st_ino
        || a.st_mode != b.st_mode
}

fn fsw_canonicalize(path: &str) -> PathBuf {
    PathBuf::from(path).canonicalize().unwrap_or_else(|_| PathBuf::from(path))
}

/// Materialize the hub (notify watcher + stat-poll worker) if absent.
fn fsw_ensure_hub() -> bool {
    FSW_HUB.with(|h| {
        if h.borrow().is_some() {
            return true;
        }
        let shared = Arc::new(Mutex::new(FswShared::new()));
        let poll_cv = Arc::new(::std::sync::Condvar::new());
        // notify's internal thread decodes the kernel events (inotify /
        // ReadDirectoryChangesW) and invokes this handler with plain data.
        // Backend errors (watch limit reached, backend io) are dropped here —
        // the former worker-side decode loop dropped its errors the same way;
        // the error-EVENT face of FSWatcher is a registered future wave.
        let handler_shared = Arc::clone(&shared);
        let watcher = notify::recommended_watcher(
            move |res: notify::Result<notify::Event>| {
                let Ok(event) = res else { return };
                let Some(event_type) = fsw_notify_kind_to_node(&event.kind) else {
                    return;
                };
                if event.paths.is_empty() {
                    return;
                }
                if let Ok(mut guard) = handler_shared.lock() {
                    guard.queue.push_back(PendingFsEvent::Notify {
                        event_type,
                        paths: event.paths,
                    });
                }
            },
        );
        let watcher = match watcher {
            Ok(w) => w,
            Err(_) => return false,
        };
        let worker_shared = Arc::clone(&shared);
        let worker_cv = Arc::clone(&poll_cv);
        let spawned = ::std::thread::Builder::new()
            .name("bao-fswatch".to_string())
            .stack_size(128 * 1024)
            .spawn(move || fsw_worker_main(worker_shared, worker_cv));
        match spawned {
            Ok(handle) => {
                *h.borrow_mut() = Some(FswHub {
                    shared,
                    poll_cv,
                    watcher,
                    handle: Some(handle),
                });
                true
            }
            Err(_) => false, // the watcher drops here → backend retires cleanly
        }
    })
}

/// notify EventKind → Node fs.watch event name (translation layer).
/// Node/libuv mapping: content/metadata mutation → "change"; create, delete
/// and any name change → "rename". Access events (open/close) never surface
/// on fs.watch. Any/Other → "rename" mirrors the former inotify decode's
/// "everything else → rename" tail.
fn fsw_notify_kind_to_node(kind: &notify::EventKind) -> Option<&'static str> {
    match kind {
        notify::EventKind::Modify(notify::event::ModifyKind::Name(_)) => Some("rename"),
        notify::EventKind::Modify(
            notify::event::ModifyKind::Data(_)
            | notify::event::ModifyKind::Metadata(_)
            | notify::event::ModifyKind::Any
            | notify::event::ModifyKind::Other,
        ) => Some("change"),
        notify::EventKind::Create(_) | notify::EventKind::Remove(_) => Some("rename"),
        notify::EventKind::Access(_) => None,
        notify::EventKind::Any | notify::EventKind::Other => Some("rename"),
    }
}

/// notify::Error → std::io::Error (the JS face throws io-style errors).
/// PathNotFound must surface as ENOENT — the c1 gate asserts the exact code.
fn fsw_notify_err_to_io(e: notify::Error) -> ::std::io::Error {
    match e.kind {
        notify::ErrorKind::Io(err) => err,
        notify::ErrorKind::PathNotFound => ::std::io::Error::from_raw_os_error(2), // ENOENT
        notify::ErrorKind::WatchNotFound => ::std::io::Error::other("fs.watch: no watch was found"),
        notify::ErrorKind::MaxFilesWatch => {
            ::std::io::Error::other("fs.watch: OS file watch limit reached")
        }
        notify::ErrorKind::Generic(msg) => ::std::io::Error::other(msg),
        notify::ErrorKind::InvalidConfig(_) => {
            ::std::io::Error::other("fs.watch: invalid watcher configuration")
        }
    }
}

/// windows: recursive fs.watch takes the native bWatchSubtree semantics;
/// linux never sees recursive here (the JS face throws
/// ERR_FEATURE_UNAVAILABLE_ON_PLATFORM before the backend is reached).
#[cfg(windows)]
fn fsw_recursive_mode(recursive: bool) -> notify::RecursiveMode {
    if recursive {
        notify::RecursiveMode::Recursive
    } else {
        notify::RecursiveMode::NonRecursive
    }
}

#[cfg(not(windows))]
fn fsw_recursive_mode(_recursive: bool) -> notify::RecursiveMode {
    notify::RecursiveMode::NonRecursive
}

/// One directory stat-poll registration (platform worker mains). Hoisted to
/// module level from the two fn-local copies (e201 REQ-ARCH-001) so the shared
/// command-drain helper can name it; fields unchanged.
struct PollSpec {
    id: u64,
    path: PathBuf,
    interval_ms: u64,
    last: libc::stat,
    next_due_ms: u128,
}

/// Fire queued StatChange events for every poll whose deadline elapsed and
/// re-arm each fired poll. Shared tail of both platform worker mains.
///
/// e201 REQ-ARCH-001: extracted from the two worker mains (text-identical
/// control flow).
fn fsw_poll_due_dispatch(shared: &Arc<Mutex<FswShared>>, polls: &mut Vec<PollSpec>) {
    let now = monotonic_ms();
    let mut due: Vec<usize> = Vec::new();
    for (i, p) in polls.iter().enumerate() {
        if p.next_due_ms <= now {
            due.push(i);
        }
    }
    for i in due {
        let Some(spec) = polls.get_mut(i) else { continue };
        spec.next_due_ms = now + spec.interval_ms as u128;
        let curr = match fsw_stat_path(&spec.path.to_string_lossy()) {
            Some(st) => st,
            None => fsw_zero_stat(),
        };
        if fsw_stat_changed(&spec.last, &curr) {
            let prev = spec.last;
            spec.last = curr;
            fsw_queue_stat_change(shared, spec.id, prev, curr);
        } else {
            spec.last = curr;
        }
    }
}

/// Push one StatChange into the shared queue when the shared state lock is
/// available; a poisoned lock drops the event exactly as the inline
/// `if let Ok` form did.
///
/// e201 REQ-ARCH-001: keeps `fsw_poll_due_dispatch` within the P-2 nesting
/// red line (the `PendingFsEvent::StatChange` literal cost one level).
fn fsw_queue_stat_change(
    shared: &Arc<Mutex<FswShared>>,
    id: u64,
    prev: libc::stat,
    curr: libc::stat,
) {
    if let Ok(mut guard) = shared.lock() {
        guard.queue.push_back(PendingFsEvent::StatChange {
            id,
            prev,
            curr,
        });
    }
}

/// Drain pending watch commands (add/remove/shutdown) into `polls`. Returns
/// true when a Shutdown command was seen — the caller exits the worker loop;
/// as in the original inline form, the remaining commands of a shutdown
/// batch are intentionally not applied.
///
/// e201 REQ-ARCH-001: shared body of the two platform worker mains
/// (text-identical control flow), extracted so both hosts stay shallow.
fn fsw_take_commands(shared: &Arc<Mutex<FswShared>>, polls: &mut Vec<PollSpec>) -> bool {
    let mut commands: VecDeque<FswCommand> = VecDeque::new();
    let mut shutdown = false;
    if let Ok(mut guard) = shared.lock() {
        ::std::mem::swap(&mut commands, &mut guard.commands);
        shutdown = commands.iter().any(|c| matches!(c, FswCommand::Shutdown));
        if shutdown {
            guard.commands.clear();
        }
    }
    if shutdown {
        return true;
    }
    for cmd in commands {
        match cmd {
            FswCommand::AddPoll { id, path, interval_ms, baseline } => {
                polls.retain(|p| p.id != id);
                polls.push(PollSpec {
                    id,
                    path,
                    interval_ms,
                    last: baseline,
                    next_due_ms: monotonic_ms().saturating_add(interval_ms as u128),
                });
            }
            FswCommand::RemovePoll { id } => polls.retain(|p| p.id != id),
            FswCommand::Shutdown => unreachable!("handled above"),
        }
    }
    false
}

/// Stat-poll worker (fs.watchFile face): wait on the poll condvar with the
/// soonest deadline as timeout — bounded (≤250ms) so shutdown and prompt
/// command pickup stay fast (the former per-platform wait loops' bound).
/// Never touches JS: StatChange events cross to the JS thread as plain data
/// through the shared queue.
fn fsw_worker_main(shared: Arc<Mutex<FswShared>>, poll_cv: Arc<::std::sync::Condvar>) {
    let mut polls: Vec<PollSpec> = Vec::new();
    loop {
        // Apply pending commands under a short lock (e201 REQ-ARCH-001:
        // extracted to `fsw_take_commands`; shutdown batch not applied, as
        // before).
        if fsw_take_commands(&shared, &mut polls) {
            break;
        }

        // Timeout: soonest poll deadline, else the bounded idle wait.
        let now = monotonic_ms();
        let timeout_ms: u64 = polls
            .iter()
            .map(|p| p.next_due_ms.saturating_sub(now) as u64)
            .min()
            .unwrap_or(250)
            .min(250);

        // The wait releases the shared lock, so queue producers (notify
        // thread) and registrants never block on the worker.
        let guard = shared.lock().unwrap_or_else(|e| e.into_inner());
        let _ = poll_cv.wait_timeout(guard, ::std::time::Duration::from_millis(timeout_ms));

        // Due stat polls (e201 REQ-ARCH-001: extracted to `fsw_poll_due_dispatch`).
        fsw_poll_due_dispatch(&shared, &mut polls);
    }
}

/// Milliseconds on a monotonic (never-backwards) clock. Every caller uses
/// this purely for relative durations (poll deadlines / timeouts) within the
/// fsw worker; the origin (first call, process-wide) is arbitrary by design.
/// The former SystemTime wall-clock form let a system clock rollback push
/// watchFile deadlines into the future (stall) or past (burst) — Instant is
/// immune (CLOCK_MONOTONIC / QPC).
fn monotonic_ms() -> u128 {
    static START: ::std::sync::OnceLock<::std::time::Instant> = ::std::sync::OnceLock::new();
    START.get_or_init(::std::time::Instant::now).elapsed().as_millis()
}

/// Register one fs.watch with the notify backend. Directory watches watch
/// the directory itself (recursive on windows — bWatchSubtree semantics).
/// FILE watches subscribe the file's PARENT directory (face 2: notify's
/// windows backend is directory-grained — the unified parent-dir
/// subscription is one code path on both platforms); the watched file's
/// basename filter lives in the pump's fan-out matcher. Missing paths are
/// fail-closed ENOENT (never a silent fake watcher). Returns the absolute
/// notify root actually registered (the fan-out matcher's routing key).
fn fsw_add_watch_ex(
    path: &str,
    recursive: bool,
) -> ::std::result::Result<PathBuf, ::std::io::Error> {
    if !fsw_ensure_hub() {
        return ::std::result::Result::Err(::std::io::Error::other(
            "fs.watch: failed to start the watcher thread",
        ));
    }
    let watch_path = Path::new(path);
    let (root, mode) = if watch_path.is_dir() {
        (watch_path.to_path_buf(), fsw_recursive_mode(recursive))
    } else {
        // fail-closed ENOENT for a missing file (same as opening it directly;
        // Node parity — the former inotify add_watch surfaced the same errno).
        if !watch_path.exists() {
            return ::std::result::Result::Err(::std::io::Error::from_raw_os_error(2)); // ENOENT
        }
        let parent = watch_path.parent().filter(|p| !p.as_os_str().is_empty());
        let parent = match parent {
            Some(p) => p.to_path_buf(),
            None => PathBuf::from("."),
        };
        (parent, notify::RecursiveMode::NonRecursive)
    };
    // notify absolutizes relative roots against the cwd internally; the
    // fan-out matcher compares event paths against the same absolute form.
    let abs_root = if root.is_absolute() {
        root
    } else {
        ::std::env::current_dir()?.join(root)
    };
    // One notify registration PER ROOT: notify's backends do not refcount
    // same-path re-watches (the windows server overwrites its WatchState map
    // entry and leaves the previous read chain alive → duplicate event
    // delivery; inotify merges masks into one kernel watch), so a second
    // watcher sharing this root must NOT re-issue watch() — the pump's
    // fan-out matcher already delivers each event to every entry on the
    // root. Mirrors the register-once / unwatch-on-last-close pairing in
    // fsw_close_entry.
    let already_registered = FSW_WATCHERS.with(|w| {
        w.borrow().iter().any(|e| e.notify_root == abs_root)
    });
    if already_registered {
        return ::std::result::Result::Ok(abs_root);
    }
    FSW_HUB.with(|h| {
        let mut hub_ref = h.borrow_mut();
        let Some(hub) = hub_ref.as_mut() else {
            return ::std::result::Result::Err(::std::io::Error::other(
                "fs.watch: watcher thread is gone",
            ));
        };
        notify::Watcher::watch(&mut hub.watcher, &abs_root, mode)
            .map(|()| abs_root.clone())
            .map_err(fsw_notify_err_to_io)
    })
}

fn fsw_register_watch(
    id: u64,
    path: String,
    notify_root: PathBuf,
    is_dir: bool,
    recursive: bool,
    persistent: bool,
) {
    FSW_WATCHERS.with(|w| {
        w.borrow_mut().push(FswWatchEntry {
            id,
            path: PathBuf::from(path),
            notify_root,
            is_dir,
            recursive,
            persistent,
        });
    });
}

fn fsw_register_poll(id: u64, path: String, interval_ms: u64, baseline: libc::stat, persistent: bool) {
    if !fsw_ensure_hub() {
        return;
    }
    FSW_POLLERS.with(|p| {
        p.borrow_mut().push(FswPollEntry {
            id,
            path: fsw_canonicalize(&path),
            persistent,
        });
    });
    FSW_HUB.with(|h| {
        if let Some(hub) = h.borrow().as_ref() {
            if let Ok(mut guard) = hub.shared.lock() {
                guard.commands.push_back(FswCommand::AddPoll {
                    id,
                    path: PathBuf::from(path),
                    interval_ms,
                    baseline,
                });
            }
        }
    });
    fsw_wake_worker();
}

/// Wake the stat-poll worker (command pickup). The notify backend thread
/// needs no wake — the JS-thread pump drains the queue on the timer cadence.
fn fsw_wake_worker() {
    FSW_HUB.with(|h| {
        if let Some(hub) = h.borrow().as_ref() {
            hub.poll_cv.notify_one();
        }
    });
}

/// Close one watcher/poller entry (fs_watch close native / unwatchFile):
/// deregister, emit 'close' on the JS object, tear the hub down when idle.
unsafe fn fsw_close_entry(cx: *mut JSContext, id: u64) {
    let watch = FSW_WATCHERS.with(|w| w.borrow_mut().iter().position(|e| e.id == id));
    let poll = FSW_POLLERS.with(|p| p.borrow_mut().iter().position(|e| e.id == id));
    let key = format!("w{}", id);

    if let Some(idx) = watch {
        let entry = FSW_WATCHERS.with(|w| w.borrow_mut().remove(idx));
        // Refcounted teardown at the notify-root level: notify does NOT
        // refcount same-path re-watches (a second watch overwrites the
        // kernel watch), so the backend watch goes away only when the LAST
        // watcher on this root closes — removing it earlier would silence
        // the surviving watchers (same contract the former wd_map carried).
        let last_on_root = FSW_WATCHERS.with(|w| {
            w.borrow().iter().all(|e| e.notify_root != entry.notify_root)
        });
        if last_on_root {
            FSW_HUB.with(|h| {
                let mut hub_ref = h.borrow_mut();
                if let Some(hub) = hub_ref.as_mut() {
                    // Best-effort: a root whose directory died may already be
                    // gone backend-side (notify reports WatchNotFound) — the
                    // hub drop removes every remaining watch regardless.
                    let _ = notify::Watcher::unwatch(&mut hub.watcher, &entry.notify_root);
                }
            });
        }
    }
    if let Some(idx) = poll {
        FSW_POLLERS.with(|p| p.borrow_mut().remove(idx));
        FSW_HUB.with(|h| {
            if let Some(hub) = h.borrow().as_ref() {
                if let Ok(mut guard) = hub.shared.lock() {
                    guard.commands.push_back(FswCommand::RemovePoll { id });
                }
            }
        });
        fsw_wake_worker();
    }

    // Emit 'close' (Node: FSWatcher/StatWatcher emit 'close' when closed),
    // then release the GcStore root.
    if let Some(obj) = crate::gc_store::gc_store_get_ns(cx, "fswatch", &key) {
        let cx_ref = &mut mozjs::context::JSContext::from_ptr(
            ::std::ptr::NonNull::new_unchecked(cx),
        );
        rooted!(&in(cx_ref) let obj_r = obj);
        let c_emit = ZBox::from_bytes("emit".as_bytes());
        let c_ev = ZBox::from_bytes("close".as_bytes());
        let ev_str = JS_NewStringCopyZ(cx, c_ev.as_ptr());
        if !ev_str.is_null() {
            let argv = [mozjs::jsval::StringValue(&*ev_str)];
            let call_args = HandleValueArray {
                length_: argv.len(),
                elements_: argv.as_ptr(),
            };
            let mut rval = UndefinedValue();
            JS_CallFunctionName(
                cx,
                obj_r.handle().into(),
                c_emit.as_ptr(),
                &call_args,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut rval,
                },
            );
            JS_ClearPendingException(cx);
        }
    }
    crate::gc_store::gc_store_remove_ns(cx, "fswatch", &key);
    let _ = fsw_maybe_shutdown_if_idle();
}

/// Tear the worker hub down when the last entry closed. Returns true if the
/// hub was torn down (or was already absent).
fn fsw_maybe_shutdown_if_idle() -> bool {
    let watchers_empty = FSW_WATCHERS.with(|w| w.borrow().is_empty());
    let pollers_empty = FSW_POLLERS.with(|p| p.borrow().is_empty());
    if watchers_empty && pollers_empty {
        FSW_HUB.with(|h| {
            *h.borrow_mut() = None; // Drop signals shutdown + joins the worker.
        });
        true
    } else {
        false
    }
}

/// Build the FSWatcher / StatWatcher JS object: EventEmitter-shaped (on/once/
/// off/emit/listenerCount wired to node_events' EE natives) + a real close().
unsafe fn fsw_make_watcher_object(cx: *mut JSContext, id: u64, _is_dir: bool) -> *mut JSObject {
    let cx_ref = &mut mozjs::context::JSContext::from_ptr(
        ::std::ptr::NonNull::new_unchecked(cx),
    );
    rooted!(&in(cx_ref) let watcher = w2::JS_NewPlainObject(cx_ref));
    if watcher.get().is_null() {
        return ::std::ptr::null_mut();
    }
    let on_op: JSNative = Some(crate::node_events::ee_on);
    let off_op: JSNative = Some(crate::node_events::ee_off);
    let once_op: JSNative = Some(crate::node_events::ee_once);
    let emit_op: JSNative = Some(crate::node_events::ee_emit);
    let close_op: JSNative = Some(fsw_close_native);
    for (name, op, nargs) in [
        ("on", on_op, 2u32),
        ("addListener", on_op, 2),
        ("off", off_op, 2),
        ("removeListener", off_op, 2),
        ("once", once_op, 2),
        ("emit", emit_op, 2),
        ("close", close_op, 0),
    ] {
        let c_name = ZBox::from_bytes(name.as_bytes());
        mozjs_sys::jsapi::JS_DefineFunction(
            cx,
            watcher.handle().into(),
            c_name.as_ptr(),
            op,
            nargs,
            JSPROP_ENUMERATE as u32,
        );
    }
    // Hidden numeric id used by fsw_close_native; rooted in GcStore for GC
    // safety across event-loop iterations.
    rooted!(&in(cx_ref) let id_v = Int32Value(id as i32));
    JS_DefineProperty(
        cx,
        watcher.handle().into(),
        c"_fswId".as_ptr(),
        id_v.handle().into(),
        0,
    );
    crate::gc_store::gc_store_insert_ns(cx, "fswatch", &format!("w{}", id), watcher.get());
    watcher.get()
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fsw_close_native(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this_v = *args.thisv().ptr;
    if this_v.is_object() {
        let obj = this_v.to_object();
        let cx_ref = &mut mozjs::context::JSContext::from_ptr(
            ::std::ptr::NonNull::new_unchecked(cx),
        );
        rooted!(&in(cx_ref) let obj_r = obj);
        let mut id_v = UndefinedValue();
        JS_GetProperty(
            cx,
            obj_r.handle().into(),
            c"_fswId".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut id_v,
            },
        );
        if id_v.is_int32() {
            fsw_close_entry(cx, id_v.to_int32() as u64);
        }
    }
    args.rval().set(UndefinedValue());
    true
}

/// Event-loop liveness: a persistent fs.watch / fs.watchFile entry keeps the
/// process alive (Node semantics); non-persistent entries never pin.
pub fn fs_watch_loop_alive() -> bool {
    let w = FSW_WATCHERS.with(|w| w.borrow().iter().any(|e| e.persistent));
    let p = FSW_POLLERS.with(|p| p.borrow().iter().any(|e| e.persistent));
    w || p
}

/// Pump all queued watch events on the JS thread. Called from
/// `timers::drain_and_check` / `drain_one_pass` (same integration point as
/// `web_api::ws_pump_all`).
pub fn fs_watch_pump_all(raw_cx: *mut JSContext) {
    // Fast path: no hub → nothing queued.
    let events: Vec<PendingFsEvent> = match FSW_HUB.with(|h| h.borrow().as_ref().map(|hub| Arc::clone(&hub.shared))) {
        Some(shared) => match shared.lock() {
            Ok(mut guard) => ::std::mem::take(&mut guard.queue).into_iter().collect(),
            Err(_) => return,
        },
        None => return,
    };
    if events.is_empty() {
        return;
    }
    for ev in events {
        match ev {
            PendingFsEvent::Notify { event_type, paths } => {
                for (id, filename) in fsw_match_watch_roots(&paths) {
                    // SAFETY: raw_cx is the live JSContext on this thread
                    // (drain hook contract); dispatch only touches JS here.
                    unsafe { fsw_dispatch_watch_event(raw_cx, id, event_type, filename) };
                }
            }
            PendingFsEvent::StatChange { id, prev, curr } => {
                // SAFETY: as above.
                unsafe { fsw_dispatch_stat_event(raw_cx, id, prev, curr) };
            }
        }
    }
}

/// Fan one backend event out to EVERY watcher whose notify root matches an
/// affected path (same-path multi-watch: every watcher gets its own event
/// copy — Node semantics). Returns (watcher id, filename) pairs.
fn fsw_match_watch_roots(paths: &[PathBuf]) -> Vec<(u64, Option<String>)> {
    FSW_WATCHERS.with(|w| {
        w.borrow()
            .iter()
            .filter_map(|e| {
                let hit = paths.iter().find(|p| fsw_event_matches_root(e, p))?;
                let filename = if e.is_dir {
                    // Directory watches report the event's name; the event on
                    // the watched root itself (e.g. the directory deleted)
                    // carries no name.
                    if hit == &e.notify_root {
                        None
                    } else {
                        hit.file_name().map(|n| n.to_string_lossy().into_owned())
                    }
                } else {
                    // File watches always carry the watched file's basename
                    // (Node passes the basename, never null).
                    e.path.file_name().map(|n| n.to_string_lossy().into_owned())
                };
                Some((e.id, filename))
            })
            .collect()
    })
}

/// Root match for one watcher entry against one backend event path.
fn fsw_event_matches_root(e: &FswWatchEntry, p: &Path) -> bool {
    if e.is_dir {
        if e.recursive {
            p.starts_with(&e.notify_root)
        } else {
            p.parent() == Some(e.notify_root.as_path()) || p == e.notify_root
        }
    } else {
        // File watch: the parent-dir subscription covers direct children
        // only; the watched file's basename is the filter.
        if p.parent() != Some(e.notify_root.as_path()) {
            return false;
        }
        match (p.file_name(), e.path.file_name()) {
            (Some(a), Some(b)) => fsw_names_match(a, b),
            _ => false,
        }
    }
}

/// windows paths are case-insensitive (libuv's fs.watch name filter too).
#[cfg(windows)]
fn fsw_names_match(a: &::std::ffi::OsStr, b: &::std::ffi::OsStr) -> bool {
    a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy())
}

#[cfg(not(windows))]
fn fsw_names_match(a: &::std::ffi::OsStr, b: &::std::ffi::OsStr) -> bool {
    a == b
}

/// Fire one fs.watch backend event on its watcher object: (eventType,
/// filename|null). The EE channel is always "change" — node's FSEventWrap
/// emits every kernel event there with the translated name as the first
/// callback arg. Listener throws route through the unified uncaught-
/// exception router — never silently swallowed.
unsafe fn fsw_dispatch_watch_event(
    raw_cx: *mut JSContext,
    id: u64,
    event_type: &str,
    filename: Option<String>,
) {
    let mut argv: Vec<mozjs::jsval::JSVal> = Vec::with_capacity(2);
    let c_type = ZBox::from_bytes(event_type.as_bytes());
    let type_str = JS_NewStringCopyZ(raw_cx, c_type.as_ptr());
    if !type_str.is_null() {
        argv.push(mozjs::jsval::StringValue(&*type_str));
    }
    match filename {
        Some(name) => {
            let c_name = ZBox::from_bytes(name.as_bytes());
            let name_str = JS_NewStringCopyZ(raw_cx, c_name.as_ptr());
            if !name_str.is_null() {
                argv.push(mozjs::jsval::StringValue(&*name_str));
            }
        }
        None => argv.push(mozjs::jsval::NullValue()),
    }
    fsw_emit_on_entry(raw_cx, id, "change", argv);
}

/// Fire one watchFile stat change on its StatWatcher: (curr, prev) Stats.
unsafe fn fsw_dispatch_stat_event(
    raw_cx: *mut JSContext,
    id: u64,
    prev: libc::stat,
    curr: libc::stat,
) {
    let curr_obj = build_stats_object(raw_cx, &curr);
    let prev_obj = build_stats_object(raw_cx, &prev);
    let mut argv: Vec<mozjs::jsval::JSVal> = Vec::with_capacity(2);
    if !curr_obj.is_null() {
        argv.push(mozjs::jsval::ObjectValue(curr_obj));
    }
    if !prev_obj.is_null() {
        argv.push(mozjs::jsval::ObjectValue(prev_obj));
    }
    fsw_emit_on_entry(raw_cx, id, "change", argv);
}

unsafe fn fsw_emit_on_entry(raw_cx: *mut JSContext, id: u64, event: &str, argv: Vec<mozjs::jsval::JSVal>) {
    let key = format!("w{}", id);
    let Some(obj) = crate::gc_store::gc_store_get_ns(raw_cx, "fswatch", &key) else {
        return; // entry closed while the event was in flight — drop it.
    };
    let cx_ref = &mut mozjs::context::JSContext::from_ptr(
        ::std::ptr::NonNull::new_unchecked(raw_cx),
    );
    rooted!(&in(cx_ref) let obj_r = obj);
    let c_emit = ZBox::from_bytes("emit".as_bytes());
    let c_ev = ZBox::from_bytes(event.as_bytes());
    let ev_str = JS_NewStringCopyZ(raw_cx, c_ev.as_ptr());
    if ev_str.is_null() {
        return;
    }
    rooted!(&in(cx_ref) let ev_root = mozjs::jsval::StringValue(&*ev_str));
    let mut all_argv: Vec<mozjs::jsval::JSVal> = Vec::with_capacity(argv.len() + 1);
    all_argv.push(ev_root.get());
    all_argv.extend(argv);
    let call_args = HandleValueArray {
        length_: all_argv.len(),
        elements_: all_argv.as_ptr(),
    };
    let mut rval = UndefinedValue();
    let ok = JS_CallFunctionName(
        raw_cx,
        obj_r.handle().into(),
        c_emit.as_ptr(),
        &call_args,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        },
    );
    if !ok {
        // Route listener throws exactly like timer callbacks (uncaught router),
        // then clear so subsequent events still dispatch.
        let mut exn = UndefinedValue();
        JS_GetPendingException(
            raw_cx,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut exn,
            },
        );
        JS_ClearPendingException(raw_cx);
        if !exn.is_undefined() {
            rooted!(&in(cx_ref) let reason_root = exn);
            crate::uncaught::route_uncaught_exception(raw_cx, exn);
        }
    }
}

// --- Async (callback-based) ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_read_file(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };

    // Callback form (Node async-first API): route through the ConcurrentTask
    // carrier — the callback receives (err, data) on the JS thread.
    if let Some((callback, encoding)) = extract_callback_and_encoding(cx, &args, 1) {
        spawn_fs_async(cx, "readFile", path.clone(), callback, encoding, move || {
            bun_fs::read(&path).map(FsAsyncResult::Ok)
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    let encoding = get_encoding_opt(cx, &args, 1);

    match bun_fs::read(&path) {
        ::std::result::Result::Ok(data) => {
            return_string_content(cx, &args, &data, encoding.as_deref())
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "readFile", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_write_file(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let data_val = if argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };
    // Buffer/TypedArray data must write its raw bytes here too; strings are
    // encoded per options.encoding (node v24.19.0 oracle: latin1 "é" → 0xE9).
    let (encoding, flag) = write_file_options(cx, &args, 2);
    let flag = flag.unwrap_or_else(|| "w".to_string());
    let bytes = write_file_data_bytes(cx, data_val, encoding.as_deref());

    // Node: writeFile(file, data[, options], callback) REQUIRES the callback
    // (ERR_INVALID_ARG_TYPE on a missing/non-function cb). The former
    // no-callback sync fallback diverged from node and is gone; callers that
    // want the synchronous face use writeFileSync.
    let callback = match extract_callback_and_encoding(cx, &args, 2) {
        Some((callback, _)) => callback,
        None => {
            return crate::bun_inspect_api::throw_invalid_arg_type(
                cx,
                &format!(
                    "The \"cb\" argument must be of type function. Received {}",
                    describe_received_fs_arg(cx, &args)
                ),
            );
        }
    };

    spawn_fs_async(cx, "writeFile", path.clone(), callback, None, move || {
        write_file_with_flag(&path, &bytes, &flag).map(|_| FsAsyncResult::OkVoid)
    });
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_mkdir(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&path) {
        let c_msg = ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    let recursive = get_bool_option(cx, &args, 1, "recursive");
    let result = if recursive {
        fs::create_dir_all(&path)
    } else {
        fs::create_dir(&path)
    };
    match result {
        ::std::result::Result::Ok(()) => {
            if argc > 1 && (*args.get(argc - 1).ptr).is_object() {
                let mut wrapped_cx_cb =
                    mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
                let cx_ref_cb = &mut wrapped_cx_cb;
                rooted!(&in(cx_ref_cb) let cb = (*args.get(argc - 1).ptr).to_object());
                rooted!(&in(cx_ref_cb) let cb_val = mozjs::jsval::ObjectValue(cb.get()));
                let null_args = HandleValueArray::empty();
                let global = CurrentGlobalOrNull(cx);
                if !global.is_null() {
                    rooted!(&in(cx_ref_cb) let global_rooted = global);
                    let mut rval = UndefinedValue();
                    JS_CallFunctionValue(
                        cx,
                        global_rooted.handle().into(),
                        cb_val.handle().into(),
                        &null_args,
                        MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &mut rval,
                        },
                    );
                    JS_ClearPendingException(cx);
                }
            }
            args.rval().set(UndefinedValue());
            true
        }
        ::std::result::Result::Err(e) => {
            if argc > 1 && (*args.get(argc - 1).ptr).is_object() {
                let mut wrapped_cx_err =
                    mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
                let cx_ref_err = &mut wrapped_cx_err;
                rooted!(&in(cx_ref_err) let cb = (*args.get(argc - 1).ptr).to_object());
                rooted!(&in(cx_ref_err) let cb_val = mozjs::jsval::ObjectValue(cb.get()));
                let err_msg = format!("EACCES: mkdir '{}': {}", path, e);
                let c_err = ZBox::from_bytes(err_msg.as_bytes());
                rooted!(&in(cx_ref_err) let err_obj = JS_NewPlainObject(cx));
                if !err_obj.get().is_null() {
                    let msg_str = JS_NewStringCopyZ(cx, c_err.as_ptr());
                    if !msg_str.is_null() {
                        rooted!(&in(cx_ref_err) let msg_val = mozjs::jsval::StringValue(&*msg_str));
                        JS_DefineProperty(
                            cx,
                            err_obj.handle().into(),
                            c"message".as_ptr(),
                            msg_val.handle().into(),
                            JSPROP_ENUMERATE as u32,
                        );
                    }
                    let code_str = JS_NewStringCopyZ(cx, c"EACCES".as_ptr());
                    if !code_str.is_null() {
                        rooted!(&in(cx_ref_err) let code_val = mozjs::jsval::StringValue(&*code_str));
                        JS_DefineProperty(
                            cx,
                            err_obj.handle().into(),
                            c"code".as_ptr(),
                            code_val.handle().into(),
                            JSPROP_ENUMERATE as u32,
                        );
                    }
                    rooted!(&in(cx_ref_err) let err_val = mozjs::jsval::ObjectValue(err_obj.get()));
                    let err_args = HandleValueArray {
                        length_: 1,
                        elements_: &err_val.get() as *const JSVal,
                    };
                    let global = CurrentGlobalOrNull(cx);
                    if !global.is_null() {
                        rooted!(&in(cx_ref_err) let global_rooted = global);
                        let mut rval = UndefinedValue();
                        JS_CallFunctionValue(
                            cx,
                            global_rooted.handle().into(),
                            cb_val.handle().into(),
                            &err_args,
                            MutableHandle::<Value> {
                                _phantom_0: ::std::marker::PhantomData,
                                ptr: &mut rval,
                            },
                        );
                        JS_ClearPendingException(cx);
                    }
                }
                args.rval().set(UndefinedValue());
                true
            } else {
                throw_fs_error(cx, "mkdir", &path, &e)
            }
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_append_file(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&path) {
        let c_msg = ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    let data_val = if argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };
    // appendFile(string, options.encoding) — same encode face as writeFile.
    let encoding = get_encoding_opt(cx, &args, 2);
    let data = write_file_data_bytes(cx, data_val, encoding.as_deref());

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 2) {
        spawn_fs_async(cx, "appendFile", path.clone(), callback, None, move || {
            ::std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .and_then(|mut f| ::std::io::Write::write_all(&mut f, &data))
                .map(|_| FsAsyncResult::OkVoid)
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    match ::std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        ::std::result::Result::Ok(mut file) => {
            match ::std::io::Write::write_all(&mut file, &data) {
                ::std::result::Result::Ok(()) => {
                    args.rval().set(UndefinedValue());
                    true
                }
                ::std::result::Result::Err(e) => throw_fs_error(cx, "appendFile", &path, &e),
            }
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "appendFile", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_access(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 1) {
        spawn_fs_async(cx, "access", path.clone(), callback, None, move || {
            fs::metadata(&path).map(|_| FsAsyncResult::OkVoid)
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    match fs::metadata(&path) {
        ::std::result::Result::Ok(_) => {
            args.rval().set(UndefinedValue());
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "access", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_chmod(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let mode_val = if argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };
    let mode = if mode_val.is_int32() {
        mode_val.to_int32() as u32
    } else if mode_val.is_double() {
        mode_val.to_double() as u32
    } else {
        0o644
    };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 2) {
        spawn_fs_async(cx, "chmod", path.clone(), callback, None, move || {
            #[cfg(unix)]
            {
                use ::std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(mode))
                    .map(|_| FsAsyncResult::OkVoid)
            }
            #[cfg(not(unix))]
            set_win_readonly(&path, mode as i32).map(|_| FsAsyncResult::OkVoid)
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    #[cfg(unix)]
    let result = {
        use ::std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(mode))
    };
    #[cfg(not(unix))]
    let result = set_win_readonly(&path, mode as i32);
    match result {
        ::std::result::Result::Ok(()) => {
            args.rval().set(UndefinedValue());
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "chmod", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_chown(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let uid = if argc > 1 {
        let v = *args.get(1).ptr;
        if v.is_int32() {
            v.to_int32() as u32
        } else if v.is_double() {
            v.to_double() as u32
        } else {
            0
        }
    } else {
        0
    };
    let gid = if argc > 2 {
        let v = *args.get(2).ptr;
        if v.is_int32() {
            v.to_int32() as u32
        } else if v.is_double() {
            v.to_double() as u32
        } else {
            0
        }
    } else {
        0
    };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 3) {
        spawn_fs_async(cx, "chown", path.clone(), callback, None, move || {
            #[cfg(unix)]
            {
                ::std::os::unix::fs::chown(&path, Some(uid), Some(gid))
                    .map(|_| FsAsyncResult::OkVoid)
            }
            #[cfg(not(unix))]
            {
                Ok(FsAsyncResult::OkVoid)
            }
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    #[cfg(unix)]
    match ::std::os::unix::fs::chown(&path, Some(uid), Some(gid)) {
        ::std::result::Result::Ok(()) => {
            args.rval().set(UndefinedValue());
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "chown", &path, &e),
    }
    #[cfg(not(unix))]
    {
        args.rval().set(UndefinedValue());
        true
    }
}

/// Shared fd-lifecycle syscall core: `close`/`fsync` run the same
/// fd-extraction + error delivery and differ only in the syscall
/// (unix-only; the non-unix faces short-circuit to undefined exactly as
/// before). REQ-ARCH-001 duplicate convergence.
#[cfg(unix)]
unsafe fn fd_void_syscall(op: &str, fd: i32) -> i32 {
    if op == "close" {
        unsafe { libc::close(fd) }
    } else {
        unsafe { libc::fsync(fd) }
    }
}

/// Shared async fd-void op core (`close`/`fsync`): identical arg extraction,
/// async spawn and sync-fallback delivery; the two faces differ only in the
/// op label and the syscall (REQ-ARCH-001 duplicate convergence).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn fs_fd_void_op(cx: *mut JSContext, argc: u32, vp: *mut JSVal, op: &'static str) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 {
        *args.get(0).ptr
    } else {
        UndefinedValue()
    };
    let fd = if fd_val.is_int32() {
        fd_val.to_int32()
    } else {
        -1
    };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 1) {
        spawn_fs_async(cx, op, format!("fd:{}", fd), callback, None, move || {
            #[cfg(unix)]
            {
                let rv = fd_void_syscall(op, fd);
                if rv == 0 {
                    Ok(FsAsyncResult::OkVoid)
                } else {
                    Err(::std::io::Error::last_os_error())
                }
            }
            #[cfg(not(unix))]
            {
                Ok(FsAsyncResult::OkVoid)
            }
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    #[cfg(unix)]
    {
        let rv = fd_void_syscall(op, fd);
        if rv == 0 {
            args.rval().set(UndefinedValue());
            true
        } else {
            throw_fs_error(
                cx,
                op,
                &format!("fd:{}", fd),
                &::std::io::Error::last_os_error(),
            )
        }
    }
    #[cfg(not(unix))]
    {
        args.rval().set(UndefinedValue());
        true
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_close(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    fs_fd_void_op(cx, argc, vp, "close")
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_fsync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    fs_fd_void_op(cx, argc, vp, "fsync")
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_copy_file(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let from = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let to = match get_path_arg(cx, &args, 1) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 2) {
        spawn_fs_async(cx, "copyFile", from.clone(), callback, None, move || {
            fs::copy(&from, &to).map(|_| FsAsyncResult::OkVoid)
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    match fs::copy(&from, &to) {
        ::std::result::Result::Ok(_) => {
            args.rval().set(UndefinedValue());
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "copyFile", &from, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_exists(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 1) {
        spawn_fs_async(cx, "exists", path.clone(), callback, None, move || {
            Ok(FsAsyncResult::OkBool(Path::new(&path).exists()))
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    args.rval()
        .set(mozjs::jsval::BooleanValue(Path::new(&path).exists()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_fchmod(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 {
        *args.get(0).ptr
    } else {
        UndefinedValue()
    };
    let fd = if fd_val.is_int32() {
        fd_val.to_int32()
    } else {
        -1
    };
    let mode_val = if argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };
    let mode = if mode_val.is_int32() {
        mode_val.to_int32() as u32
    } else {
        0o644
    };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 2) {
        spawn_fs_async(
            cx,
            "fchmod",
            format!("fd:{}", fd),
            callback,
            None,
            move || {
                #[cfg(unix)]
                {
                    let rv = unsafe { libc::fchmod(fd, mode) };
                    if rv == 0 {
                        Ok(FsAsyncResult::OkVoid)
                    } else {
                        Err(::std::io::Error::last_os_error())
                    }
                }
                #[cfg(not(unix))]
                {
                    Ok(FsAsyncResult::OkVoid)
                }
            },
        );
        args.rval().set(UndefinedValue());
        return true;
    }

    #[cfg(unix)]
    {
        let rv = unsafe { libc::fchmod(fd, mode) };
        if rv == 0 {
            args.rval().set(UndefinedValue());
            true
        } else {
            throw_fs_error(
                cx,
                "fchmod",
                &format!("fd:{}", fd),
                &::std::io::Error::last_os_error(),
            )
        }
    }
    #[cfg(not(unix))]
    {
        args.rval().set(UndefinedValue());
        true
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_fchown(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 {
        *args.get(0).ptr
    } else {
        UndefinedValue()
    };
    let fd = if fd_val.is_int32() {
        fd_val.to_int32()
    } else {
        -1
    };
    let uid = if argc > 1 {
        let v = *args.get(1).ptr;
        if v.is_int32() { v.to_int32() as u32 } else { 0 }
    } else {
        0
    };
    let gid = if argc > 2 {
        let v = *args.get(2).ptr;
        if v.is_int32() { v.to_int32() as u32 } else { 0 }
    } else {
        0
    };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 3) {
        spawn_fs_async(
            cx,
            "fchown",
            format!("fd:{}", fd),
            callback,
            None,
            move || {
                #[cfg(unix)]
                {
                    let rv = unsafe { libc::fchown(fd, uid as libc::uid_t, gid as libc::gid_t) };
                    if rv == 0 {
                        Ok(FsAsyncResult::OkVoid)
                    } else {
                        Err(::std::io::Error::last_os_error())
                    }
                }
                #[cfg(not(unix))]
                {
                    Ok(FsAsyncResult::OkVoid)
                }
            },
        );
        args.rval().set(UndefinedValue());
        return true;
    }

    #[cfg(unix)]
    {
        let rv = unsafe { libc::fchown(fd, uid as libc::uid_t, gid as libc::gid_t) };
        if rv == 0 {
            args.rval().set(UndefinedValue());
            true
        } else {
            throw_fs_error(
                cx,
                "fchown",
                &format!("fd:{}", fd),
                &::std::io::Error::last_os_error(),
            )
        }
    }
    #[cfg(not(unix))]
    {
        args.rval().set(UndefinedValue());
        true
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_fdatasync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 {
        *args.get(0).ptr
    } else {
        UndefinedValue()
    };
    let fd = if fd_val.is_int32() {
        fd_val.to_int32()
    } else {
        -1
    };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 1) {
        spawn_fs_async(
            cx,
            "fdatasync",
            format!("fd:{}", fd),
            callback,
            None,
            move || {
                #[cfg(unix)]
                {
                    let rv = unsafe { libc::fdatasync(fd) };
                    if rv == 0 {
                        Ok(FsAsyncResult::OkVoid)
                    } else {
                        Err(::std::io::Error::last_os_error())
                    }
                }
                #[cfg(not(unix))]
                {
                    Ok(FsAsyncResult::OkVoid)
                }
            },
        );
        args.rval().set(UndefinedValue());
        return true;
    }

    #[cfg(unix)]
    {
        let rv = unsafe { libc::fdatasync(fd) };
        if rv == 0 {
            args.rval().set(UndefinedValue());
            true
        } else {
            throw_fs_error(
                cx,
                "fdatasync",
                &format!("fd:{}", fd),
                &::std::io::Error::last_os_error(),
            )
        }
    }
    #[cfg(not(unix))]
    {
        args.rval().set(UndefinedValue());
        true
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_fstat(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 {
        *args.get(0).ptr
    } else {
        UndefinedValue()
    };
    let fd = if fd_val.is_int32() {
        fd_val.to_int32()
    } else {
        -1
    };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 1) {
        spawn_fs_async(
            cx,
            "fstat",
            format!("fd:{}", fd),
            callback,
            None,
            move || {
                #[cfg(unix)]
                {
                    let mut stat_buf: libc::stat = ::std::mem::zeroed();
                    let rv = unsafe { libc::fstat(fd, &mut stat_buf) };
                    if rv == 0 {
                        Ok(FsAsyncResult::OkStat(posix_stat_from_libc(&stat_buf)))
                    } else {
                        Err(::std::io::Error::last_os_error())
                    }
                }
                #[cfg(not(unix))]
                {
                    Ok(FsAsyncResult::OkVoid)
                }
            },
        );
        args.rval().set(UndefinedValue());
        return true;
    }

    #[cfg(unix)]
    {
        let mut stat_buf: libc::stat = ::std::mem::zeroed();
        let rv = unsafe { libc::fstat(fd, &mut stat_buf) };
        if rv == 0 {
            let posix = posix_stat_from_libc(&stat_buf);
            let stats = create_stats_object(cx, &posix);
            args.rval().set(mozjs::jsval::ObjectValue(stats));
            true
        } else {
            throw_fs_error(
                cx,
                "fstat",
                &format!("fd:{}", fd),
                &::std::io::Error::last_os_error(),
            )
        }
    }
    #[cfg(not(unix))]
    {
        args.rval().set(UndefinedValue());
        true
    }
}

/// fstatSync(fd) — the sync face of fs.fstat, mirroring its closure (node
/// contract: Stats of the open fd; a closed/invalid fd throws EBADF via the
/// shared io_error_code arm; node v24.19.0 oracle: fstatSync_ebadf=EBADF).
/// The C11 wave: fstatSync was absent from the registration table entirely.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_fstat_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd = if argc > 0 && (*args.get(0).ptr).is_int32() {
        (*args.get(0).ptr).to_int32()
    } else {
        -1
    };
    #[cfg(unix)]
    {
        let mut stat_buf: libc::stat = ::std::mem::zeroed();
        let rv = unsafe { libc::fstat(fd, &mut stat_buf) };
        if rv == 0 {
            let posix = posix_stat_from_libc(&stat_buf);
            let stats = create_stats_object(cx, &posix);
            args.rval().set(mozjs::jsval::ObjectValue(stats));
            true
        } else {
            throw_fs_error(
                cx,
                "fstat",
                &format!("fd:{}", fd),
                &::std::io::Error::last_os_error(),
            )
        }
    }
    #[cfg(not(unix))]
    {
        args.rval().set(UndefinedValue());
        true
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_ftruncate(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 {
        *args.get(0).ptr
    } else {
        UndefinedValue()
    };
    let fd = if fd_val.is_int32() {
        fd_val.to_int32()
    } else {
        -1
    };
    let len_val = if argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };
    let len = truncate_len_from_val(len_val);

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 2) {
        spawn_fs_async(
            cx,
            "ftruncate",
            format!("fd:{}", fd),
            callback,
            None,
            move || {
                #[cfg(unix)]
                {
                    let rv = unsafe { libc::ftruncate(fd, len) };
                    if rv == 0 {
                        Ok(FsAsyncResult::OkVoid)
                    } else {
                        Err(::std::io::Error::last_os_error())
                    }
                }
                #[cfg(not(unix))]
                {
                    Ok(FsAsyncResult::OkVoid)
                }
            },
        );
        args.rval().set(UndefinedValue());
        return true;
    }

    #[cfg(unix)]
    {
        let rv = unsafe { libc::ftruncate(fd, len) };
        if rv == 0 {
            args.rval().set(UndefinedValue());
            true
        } else {
            throw_fs_error(
                cx,
                "ftruncate",
                &format!("fd:{}", fd),
                &::std::io::Error::last_os_error(),
            )
        }
    }
    #[cfg(not(unix))]
    {
        args.rval().set(UndefinedValue());
        true
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_futimes(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 {
        *args.get(0).ptr
    } else {
        UndefinedValue()
    };
    let fd = if fd_val.is_int32() {
        fd_val.to_int32()
    } else {
        -1
    };
    let atime_val = if argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };
    let mtime_val = if argc > 2 {
        *args.get(2).ptr
    } else {
        UndefinedValue()
    };
    let atime = if atime_val.is_double() {
        atime_val.to_double()
    } else if atime_val.is_int32() {
        atime_val.to_int32() as f64
    } else {
        0.0
    };
    let mtime = if mtime_val.is_double() {
        mtime_val.to_double()
    } else if mtime_val.is_int32() {
        mtime_val.to_int32() as f64
    } else {
        0.0
    };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 3) {
        spawn_fs_async(
            cx,
            "futimes",
            format!("fd:{}", fd),
            callback,
            None,
            move || {
                #[cfg(unix)]
                {
                    let tv = [
                        libc::timeval {
                            tv_sec: atime as i64,
                            tv_usec: ((atime % 1.0) * 1_000_000.0) as i64,
                        },
                        libc::timeval {
                            tv_sec: mtime as i64,
                            tv_usec: ((mtime % 1.0) * 1_000_000.0) as i64,
                        },
                    ];
                    let rv = unsafe { libc::futimes(fd, tv.as_ptr()) };
                    if rv == 0 {
                        Ok(FsAsyncResult::OkVoid)
                    } else {
                        Err(::std::io::Error::last_os_error())
                    }
                }
                #[cfg(not(unix))]
                {
                    Ok(FsAsyncResult::OkVoid)
                }
            },
        );
        args.rval().set(UndefinedValue());
        return true;
    }

    #[cfg(unix)]
    {
        let tv = [
            libc::timeval {
                tv_sec: atime as i64,
                tv_usec: ((atime % 1.0) * 1_000_000.0) as i64,
            },
            libc::timeval {
                tv_sec: mtime as i64,
                tv_usec: ((mtime % 1.0) * 1_000_000.0) as i64,
            },
        ];
        let rv = unsafe { libc::futimes(fd, tv.as_ptr()) };
        if rv == 0 {
            args.rval().set(UndefinedValue());
            true
        } else {
            throw_fs_error(
                cx,
                "futimes",
                &format!("fd:{}", fd),
                &::std::io::Error::last_os_error(),
            )
        }
    }
    #[cfg(not(unix))]
    {
        args.rval().set(UndefinedValue());
        true
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_lchown(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let uid = if argc > 1 {
        let v = *args.get(1).ptr;
        if v.is_int32() { v.to_int32() as u32 } else { 0 }
    } else {
        0
    };
    let gid = if argc > 2 {
        let v = *args.get(2).ptr;
        if v.is_int32() { v.to_int32() as u32 } else { 0 }
    } else {
        0
    };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 3) {
        spawn_fs_async(cx, "lchown", path.clone(), callback, None, move || {
            #[cfg(unix)]
            {
                let c_p = ::std::ffi::CString::new(path.as_str()).unwrap_or_default();
                let rv = unsafe { libc::lchown(c_p.as_ptr(), uid, gid) };
                if rv == 0 {
                    Ok(FsAsyncResult::OkVoid)
                } else {
                    Err(::std::io::Error::last_os_error())
                }
            }
            #[cfg(not(unix))]
            {
                Ok(FsAsyncResult::OkVoid)
            }
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    #[cfg(unix)]
    {
        let c_path = ::std::ffi::CString::new(path.as_str()).unwrap_or_default();
        let rv = unsafe { libc::lchown(c_path.as_ptr(), uid, gid) };
        if rv == 0 {
            args.rval().set(UndefinedValue());
            true
        } else {
            throw_fs_error(cx, "lchown", &path, &::std::io::Error::last_os_error())
        }
    }
    #[cfg(not(unix))]
    {
        args.rval().set(UndefinedValue());
        true
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_link(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let from = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let to = match get_path_arg(cx, &args, 1) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 2) {
        spawn_fs_async(cx, "link", from.clone(), callback, None, move || {
            fs::hard_link(&from, &to).map(|_| FsAsyncResult::OkVoid)
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    match fs::hard_link(&from, &to) {
        ::std::result::Result::Ok(()) => {
            args.rval().set(UndefinedValue());
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "link", &from, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_lstat(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 1) {
        spawn_fs_async(cx, "lstat", path.clone(), callback, None, move || {
            fs::symlink_metadata(&path).map(|m| FsAsyncResult::OkStat(metadata_to_posix_stat(&m)))
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    match fs::symlink_metadata(&path) {
        ::std::result::Result::Ok(meta) => {
            let posix = metadata_to_posix_stat(&meta);
            let stats = create_stats_object(cx, &posix);
            args.rval().set(mozjs::jsval::ObjectValue(stats));
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "lstat", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_lutimes(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let atime_val = if argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };
    let mtime_val = if argc > 2 {
        *args.get(2).ptr
    } else {
        UndefinedValue()
    };
    let atime = if atime_val.is_double() {
        atime_val.to_double()
    } else if atime_val.is_int32() {
        atime_val.to_int32() as f64
    } else {
        0.0
    };
    let mtime = if mtime_val.is_double() {
        mtime_val.to_double()
    } else if mtime_val.is_int32() {
        mtime_val.to_int32() as f64
    } else {
        0.0
    };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 3) {
        spawn_fs_async(cx, "lutimes", path.clone(), callback, None, move || {
            #[cfg(unix)]
            {
                let c_path = ::std::ffi::CString::new(path.as_str()).unwrap_or_default();
                let tv = [
                    libc::timeval {
                        tv_sec: atime as i64,
                        tv_usec: ((atime % 1.0) * 1_000_000.0) as i64,
                    },
                    libc::timeval {
                        tv_sec: mtime as i64,
                        tv_usec: ((mtime % 1.0) * 1_000_000.0) as i64,
                    },
                ];
                let rv = unsafe { libc::lutimes(c_path.as_ptr(), tv.as_ptr()) };
                if rv == 0 {
                    Ok(FsAsyncResult::OkVoid)
                } else {
                    Err(::std::io::Error::last_os_error())
                }
            }
            #[cfg(not(unix))]
            {
                Ok(FsAsyncResult::OkVoid)
            }
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    #[cfg(unix)]
    {
        let c_path = ::std::ffi::CString::new(path.as_str()).unwrap_or_default();
        let tv = [
            libc::timeval {
                tv_sec: atime as i64,
                tv_usec: ((atime % 1.0) * 1_000_000.0) as i64,
            },
            libc::timeval {
                tv_sec: mtime as i64,
                tv_usec: ((mtime % 1.0) * 1_000_000.0) as i64,
            },
        ];
        let rv = unsafe { libc::lutimes(c_path.as_ptr(), tv.as_ptr()) };
        if rv == 0 {
            args.rval().set(UndefinedValue());
            true
        } else {
            throw_fs_error(cx, "lutimes", &path, &::std::io::Error::last_os_error())
        }
    }
    #[cfg(not(unix))]
    {
        args.rval().set(UndefinedValue());
        true
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_mkdtemp(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let prefix = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let _encoding = get_encoding_opt(cx, &args, 1);

    if let Some((callback, cb_encoding)) = extract_callback_and_encoding(cx, &args, 1) {
        spawn_fs_async(
            cx,
            "mkdtemp",
            prefix.clone(),
            callback,
            cb_encoding,
            move || mkdtemp_inner(&prefix).map(FsAsyncResult::OkString),
        );
        args.rval().set(UndefinedValue());
        return true;
    }

    match mkdtemp_inner(&prefix) {
        ::std::result::Result::Ok(dir) => {
            let c_str = ZBox::from_bytes(dir.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() {
                args.rval().set(UndefinedValue());
            } else {
                args.rval().set(mozjs::jsval::StringValue(&*js_str));
            }
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "mkdtemp", &prefix, &e),
    }
}

fn mkdtemp_inner(prefix: &str) -> ::std::io::Result<String> {
    // Node rejects an empty prefix with EINVAL (its snprintf builds a five-X
    // template here); otherwise we'd create a bare six-random-character
    // directory in the process cwd.
    if prefix.is_empty() {
        return Err(::std::io::Error::from_raw_os_error(libc::EINVAL));
    }
    let mut template = prefix.to_string();
    template.push_str("XXXXXX");
    let c_template = ::std::ffi::CString::new(template).map_err(|_| {
        ::std::io::Error::new(
            ::std::io::ErrorKind::InvalidInput,
            "prefix contains null byte",
        )
    })?;
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut c_ptr = c_template.into_raw();
    #[cfg(unix)]
    let result = unsafe { libc::mkdtemp(c_ptr) };
    #[cfg(windows)]
    // windows: CRT has no mkdtemp — create the directory at the templated
    // path directly. The X…X tail is replaced with sub-second randomness +
    // the pid; create_dir fails loudly on collision (retry ×8). Ownership:
    // on success the template buffer is reclaimed and a fresh owned CString
    // carries the created path; on failure null comes back and the outer
    // null arm reclaims the template.
    let result = {
        let template_path = unsafe { ::std::ffi::CStr::from_ptr(c_ptr) }
            .to_string_lossy()
            .to_string();
        let mut made: Option<String> = None;
        for _ in 0..8 {
            let tail = {
                use ::std::time::{SystemTime, UNIX_EPOCH};
                let nanos = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.subsec_nanos())
                    .unwrap_or(0);
                format!("{:06x}", nanos ^ (::std::process::id() << 8))
            };
            let candidate = template_path.replacen("XXXXXX", &tail, 1);
            match ::std::fs::create_dir(&candidate) {
                Ok(()) => {
                    made = Some(candidate);
                    break;
                }
                Err(e) if e.kind() == ::std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => break,
            }
        }
        match made {
            Some(dir) => unsafe {
                let _ = ::std::ffi::CString::from_raw(c_ptr);
                ::std::ffi::CString::new(dir).unwrap().into_raw()
            },
            None => ::std::ptr::null_mut(),
        }
    };
    if result.is_null() {
        let e = ::std::io::Error::last_os_error();
        unsafe {
            let _ = ::std::ffi::CString::from_raw(c_ptr);
        }
        return Err(e);
    }
    let result_cstr = unsafe { ::std::ffi::CString::from_raw(result) };
    result_cstr
        .into_string()
        .map_err(|e| ::std::io::Error::new(::std::io::ErrorKind::InvalidData, e))
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_open(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let flags_val = if argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };
    let flags = if flags_val.is_int32() {
        flags_val.to_int32()
    } else if flags_val.is_string() {
        let s = flags_val.to_string();
        if !s.is_null() {
            let rust_str = crate::jsstr_to_rust_string(cx, s);
            parse_open_flags(&rust_str)
        } else {
            0
        }
    } else {
        0
    };
    let mode_val = if argc > 2 {
        *args.get(2).ptr
    } else {
        UndefinedValue()
    };
    let mode = if mode_val.is_int32() {
        mode_val.to_int32() as u32
    } else {
        0o644
    };

    // Callback form: fs.open(path, flags[, mode], cb) — the callback sits at
    // index 2 (or 3 with an explicit mode). Scanning from index 1 matches
    // both shapes (extract_callback_and_encoding only captures callables, so
    // the string/int flags and mode args are not mis-captured). C1 registry
    // #9: the previous start index of 3 made the plain fs.open(p, flags, cb)
    // form never deliver its callback (node v24.19.0 oracle: fs.open(path,
    // 'w', cb) → cb(null, fd)).
    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 1) {
        spawn_fs_async(cx, "open", path.clone(), callback, None, move || {
            #[cfg(unix)]
            {
                let c_path = ::std::ffi::CString::new(path.as_str()).unwrap_or_default();
                let fd = unsafe { libc::open(c_path.as_ptr(), flags, mode) };
                if fd >= 0 {
                    Ok(FsAsyncResult::OkOpen(fd))
                } else {
                    Err(::std::io::Error::last_os_error())
                }
            }
            #[cfg(not(unix))]
            {
                Ok(FsAsyncResult::OkOpen(0))
            }
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    #[cfg(unix)]
    {
        let c_path = ::std::ffi::CString::new(path.as_str()).unwrap_or_default();
        let fd = unsafe { libc::open(c_path.as_ptr(), flags, mode) };
        if fd >= 0 {
            args.rval().set(mozjs::jsval::Int32Value(fd));
            true
        } else {
            throw_fs_error(cx, "open", &path, &::std::io::Error::last_os_error())
        }
    }
    #[cfg(not(unix))]
    {
        args.rval().set(mozjs::jsval::Int32Value(0));
        true
    }
}

fn parse_open_flags(s: &str) -> i32 {
    let mut flags = 0;
    if s.contains('w') {
        flags |= libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC;
    }
    if s.contains('a') {
        flags |= libc::O_WRONLY | libc::O_CREAT | libc::O_APPEND;
    }
    if s.contains('x') {
        // node contract: 'x' (wx / ax / x+) → O_EXCL; open of an existing
        // path fails with EEXIST (C1 registry #1; node v24.19.0 oracle probe:
        // openSync(existing, 'wx') → EEXIST).
        flags |= libc::O_EXCL;
    }
    if s.contains('+') {
        flags = (flags & !(libc::O_WRONLY | libc::O_RDONLY)) | libc::O_RDWR;
    }
    flags
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_opendir(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 1) {
        // With callback: create Dir object and pass to callback
        let dir_obj = create_dir_object(cx, &path);
        let mut wrapped_cx_od =
            mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        let cx_ref_od = &mut wrapped_cx_od;
        rooted!(&in(cx_ref_od) let cb = callback);
        rooted!(&in(cx_ref_od) let cb_val = mozjs::jsval::ObjectValue(cb.get()));
        rooted!(&in(cx_ref_od) let dir_val = mozjs::jsval::ObjectValue(dir_obj));
        let args_arr = [UndefinedValue(), dir_val.get()];
        let cb_args = HandleValueArray {
            length_: 2,
            elements_: args_arr.as_ptr(),
        };
        let global = CurrentGlobalOrNull(cx);
        if !global.is_null() {
            rooted!(&in(cx_ref_od) let global_rooted = global);
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(
                cx,
                global_rooted.handle().into(),
                cb_val.handle().into(),
                &cb_args,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut rval,
                },
            );
            JS_ClearPendingException(cx);
        }
        args.rval().set(UndefinedValue());
        return true;
    }

    // No callback: return Dir object directly
    match fs::metadata(&path) {
        ::std::result::Result::Ok(meta) if meta.is_dir() => {
            let dir_obj = create_dir_object(cx, &path);
            args.rval().set(mozjs::jsval::ObjectValue(dir_obj));
            true
        }
        ::std::result::Result::Ok(_) => {
            JS_ReportErrorUTF8(cx, c"opendir: path is not a directory".as_ptr());
            false
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "opendir", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_read(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 {
        *args.get(0).ptr
    } else {
        UndefinedValue()
    };
    let fd = if fd_val.is_int32() {
        fd_val.to_int32()
    } else {
        -1
    };
    let length = if argc > 3 {
        let v = *args.get(3).ptr;
        if v.is_int32() {
            v.to_int32() as usize
        } else {
            65536
        }
    } else {
        65536
    };
    // node contract: read(fd, buffer, offset, length, position) — a numeric
    // position reads at that ABSOLUTE offset without moving the cursor
    // (pread semantics); null/absent reads at the cursor (advancing it).
    // Previously position was parsed but ignored — every read landed at the
    // cursor (node v24.19.0 oracle: fs.read(fd, b, 1, 2, 0) reads file[0..2]).
    let pos = if argc > 4 {
        fs_number_arg(*args.get(4).ptr)
    } else {
        ::std::option::Option::None
    };
    let (use_pread, position) = (pos.is_some(), pos.unwrap_or(0.0) as i64);

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 5) {
        // node contract: the callback receives the SAME buffer object the
        // caller passed, filled at [offset, offset + bytesRead). Root it for
        // the worker window; fail closed (rather than silently not filling)
        // if the root registration fails.
        let buf_arg = if argc > 1 {
            *args.get(1).ptr
        } else {
            UndefinedValue()
        };
        let writeback_offset: usize = if argc > 2 {
            let v = *args.get(2).ptr;
            if v.is_int32() {
                v.to_int32().max(0) as usize
            } else {
                0
            }
        } else {
            0
        };
        let writeback: ::std::option::Option<RawValueRootGuard> = if buf_arg.is_object() {
            match RawValueRootGuard::new(cx, ::std::slice::from_ref(&buf_arg), c"fs_read_buffer") {
                ::std::option::Option::Some(g) => ::std::option::Option::Some(g),
                ::std::option::Option::None => {
                    JS_ReportErrorUTF8(cx, c"read: failed to root the caller buffer".as_ptr());
                    return false;
                }
            }
        } else {
            ::std::option::Option::None
        };
        spawn_fs_async_writeback(
            cx,
            "read",
            format!("fd:{}", fd),
            callback,
            None,
            writeback,
            writeback_offset,
            move || {
                let mut buf = vec![0u8; length];
                #[cfg(unix)]
                {
                    let bytes_read = if use_pread {
                        unsafe {
                            libc::pread(
                                fd,
                                buf.as_mut_ptr() as *mut ::std::ffi::c_void,
                                length,
                                position,
                            )
                        }
                    } else {
                        unsafe {
                            libc::read(fd, buf.as_mut_ptr() as *mut ::std::ffi::c_void, length)
                        }
                    };
                    if bytes_read >= 0 {
                        buf.truncate(bytes_read as usize);
                        Ok(FsAsyncResult::OkRead {
                            bytes_read: bytes_read as i32,
                            buffer: buf,
                        })
                    } else {
                        Err(::std::io::Error::last_os_error())
                    }
                }
                #[cfg(not(unix))]
                {
                    let _ = (use_pread, position);
                    Ok(FsAsyncResult::OkRead {
                        bytes_read: 0,
                        buffer: buf,
                    })
                }
            },
        );
        args.rval().set(UndefinedValue());
        return true;
    }

    let mut buf = vec![0u8; length];
    #[cfg(unix)]
    {
        let bytes_read = if use_pread {
            unsafe {
                libc::pread(
                    fd,
                    buf.as_mut_ptr() as *mut ::std::ffi::c_void,
                    length,
                    position,
                )
            }
        } else {
            unsafe { libc::read(fd, buf.as_mut_ptr() as *mut ::std::ffi::c_void, length) }
        };
        if bytes_read >= 0 {
            buf.truncate(bytes_read as usize);
            // node contract: read(buffer, offset, length, position) ALSO fills the
            // caller's buffer window [offset, offset + bytesRead) — the data previously
            // landed only in the fresh result buffer, leaving the caller's zero-filled
            // (C11 wave; readv C1 #3 sibling; node v24.19.0 oracle:
            // fh.read(b,0,2,0) fills b).
            {
                let buf_arg = *args.get(1).ptr;
                if buf_arg.is_object() {
                    let off_rb: u32 = if argc > 2 {
                        let v = *args.get(2).ptr;
                        if v.is_int32() { v.to_int32().max(0) as u32 } else { 0 }
                    } else {
                        0
                    };
                    let mut wrapped_rb =
                        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
                    let cx_rb = &mut wrapped_rb;
                    rooted!(&in(cx_rb) let caller_rb = buf_arg.to_object());
                    for (i_rb, &byte_rb) in buf.iter().enumerate() {
                        rooted!(&in(cx_rb) let bv_rb = mozjs::jsval::Int32Value(byte_rb as i32));
                        JS_SetElement(
                            cx,
                            caller_rb.handle().into(),
                            off_rb + i_rb as u32,
                            bv_rb.handle().into(),
                        );
                    }
                }
            }
            let buf_obj = crate::globals::create_buffer_object(cx, &buf);
            if buf_obj.is_null() {
                args.rval().set(UndefinedValue());
            } else {
                args.rval()
                    .set(mozjs::jsval::DoubleValue(bytes_read as f64));
            }
            true
        } else {
            throw_fs_error(
                cx,
                "read",
                &format!("fd:{}", fd),
                &::std::io::Error::last_os_error(),
            )
        }
    }
    #[cfg(not(unix))]
    {
        args.rval().set(UndefinedValue());
        true
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_readdir(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let with_file_types = get_bool_option(cx, &args, 1, "withFileTypes");

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 1) {
        spawn_fs_async(cx, "readdir", path.clone(), callback, None, move || {
            fs::read_dir(&path).map(|entries| {
                let items: Vec<_> = entries.flatten().collect();
                if with_file_types {
                    let dirents: Vec<(String, bool)> = items
                        .iter()
                        .map(|e| {
                            (
                                e.file_name().to_string_lossy().into_owned(),
                                e.file_type().map(|ft| ft.is_dir()).unwrap_or(false),
                            )
                        })
                        .collect();
                    FsAsyncResult::OkDirents(dirents)
                } else {
                    let names: Vec<String> = items
                        .iter()
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .collect();
                    FsAsyncResult::OkDirnames(names)
                }
            })
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    match fs::read_dir(&path) {
        ::std::result::Result::Ok(entries) => {
            let mut names: Vec<String> = Vec::new();
            let mut is_dirs: Vec<bool> = Vec::new();
            for entry in entries.flatten() {
                names.push(entry.file_name().to_string_lossy().into_owned());
                is_dirs.push(entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false));
            }
            let mut wrapped_cx = unsafe {
                mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx))
            };
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let arr = unsafe { w2::NewArrayObject1(cx_ref, names.len()) });
            if arr.get().is_null() {
                args.rval().set(UndefinedValue());
                return true;
            }
            for (i, name) in names.iter().enumerate() {
                if with_file_types {
                    let dirent = create_dirent(cx, name, is_dirs[i]);
                    if !dirent.is_null() {
                        rooted!(&in(cx_ref) let val = mozjs::jsval::ObjectValue(dirent));
                        unsafe {
                            JS_DefineElement(
                                cx,
                                arr.handle().into(),
                                i as u32,
                                val.handle().into(),
                                JSPROP_ENUMERATE as u32,
                            );
                        }
                    }
                } else {
                    let c_name = ZBox::from_bytes(name.as_bytes());
                    let js_str = unsafe { JS_NewStringCopyZ(cx, c_name.as_ptr()) };
                    if !js_str.is_null() {
                        rooted!(&in(cx_ref) let val = mozjs::jsval::StringValue(&*js_str));
                        unsafe {
                            JS_DefineElement(
                                cx,
                                arr.handle().into(),
                                i as u32,
                                val.handle().into(),
                                JSPROP_ENUMERATE as u32,
                            );
                        }
                    }
                }
            }
            args.rval().set(mozjs::jsval::ObjectValue(arr.get()));
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "readdir", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_readlink(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let _encoding = get_encoding_opt(cx, &args, 1);

    if let Some((callback, cb_encoding)) = extract_callback_and_encoding(cx, &args, 1) {
        spawn_fs_async(
            cx,
            "readlink",
            path.clone(),
            callback,
            cb_encoding,
            move || {
                fs::read_link(&path)
                    .map(|t| FsAsyncResult::OkString(t.to_string_lossy().into_owned()))
            },
        );
        args.rval().set(UndefinedValue());
        return true;
    }

    match fs::read_link(&path) {
        ::std::result::Result::Ok(target) => {
            let s = target.to_string_lossy();
            let c_str = ZBox::from_bytes(s.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() {
                args.rval().set(UndefinedValue());
            } else {
                args.rval().set(mozjs::jsval::StringValue(&*js_str));
            }
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "readlink", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_readv(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    // Node: readv(fd, buffers[, position], cb) — scatter-gather read into the
    // CALLER's views; cb(null, bytesRead). Previously this face aliased
    // fs.read, so the buffers array was misread as an offset/length pair and
    // the caller's buffers were never filled.
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 {
        *args.get(0).ptr
    } else {
        UndefinedValue()
    };
    let fd = if fd_val.is_int32() {
        fd_val.to_int32()
    } else {
        -1
    };
    let buffers_val = if argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };
    if !buffers_val.is_object() {
        JS_ReportErrorUTF8(cx, c"readv: buffers must be an array".as_ptr());
        return false;
    }
    let pos_arg = if argc > 2 {
        *args.get(2).ptr
    } else {
        UndefinedValue()
    };
    let pos = fs_number_arg(pos_arg);
    let (use_pread, position) = (pos.is_some(), pos.unwrap_or(-1.0) as i64);

    let callback = match extract_callback_and_encoding(cx, &args, 2) {
        Some((callback, _)) => callback,
        None => {
            return crate::bun_inspect_api::throw_invalid_arg_type(
                cx,
                "The \"cb\" argument must be of type function. Received undefined",
            );
        }
    };

    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let buffers_obj = buffers_val.to_object());
    let mut vals: Vec<JSVal> = Vec::new();
    let mut lens: Vec<usize> = Vec::new();
    let mut buf_idx: u32 = 0;
    loop {
        let mut elem = UndefinedValue();
        JS_GetElement(
            cx,
            buffers_obj.handle().into(),
            buf_idx,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut elem,
            },
        );
        if elem.is_undefined() {
            break;
        }
        if !elem.is_object() {
            JS_ReportErrorUTF8(cx, c"readv: buffers must be an array of Buffers".as_ptr());
            return false;
        }
        let view = elem.to_object();
        let len = JS_GetTypedArrayByteLength(view) as usize;
        let mut is_shared = false;
        let data = JS_GetUint8ArrayData(view, &mut is_shared, ::std::ptr::null());
        if data.is_null() {
            JS_ReportErrorUTF8(cx, c"readv: buffers must be an array of Buffers".as_ptr());
            return false;
        }
        if len > 0 {
            vals.push(elem);
            lens.push(len);
        }
        buf_idx += 1;
    }

    let writeback: ::std::option::Option<RawValueRootGuard> = if vals.is_empty() {
        ::std::option::Option::None
    } else {
        match RawValueRootGuard::new(cx, &vals, c"fs_readv_iovecs") {
            ::std::option::Option::Some(g) => ::std::option::Option::Some(g),
            ::std::option::Option::None => {
                JS_ReportErrorUTF8(cx, c"readv: failed to root the caller buffers".as_ptr());
                return false;
            }
        }
    };
    let total: usize = lens.iter().sum();

    spawn_fs_async_writeback(
        cx,
        "readv",
        format!("fd:{}", fd),
        callback,
        None,
        writeback,
        0,
        move || {
            let mut buf = vec![0u8; total];
            let bytes_read: usize;
            #[cfg(unix)]
            {
                if total == 0 {
                    bytes_read = 0;
                } else if use_pread {
                    let mut acc: usize = 0;
                    for &len in &lens {
                        let n = unsafe {
                            libc::pread(
                                fd,
                                buf.as_mut_ptr().add(acc) as *mut ::std::ffi::c_void,
                                len,
                                position + acc as i64,
                            )
                        };
                        if n < 0 {
                            return ::std::result::Result::Err(::std::io::Error::last_os_error());
                        }
                        acc += n as usize;
                        if (n as usize) < len {
                            break;
                        }
                    }
                    bytes_read = acc;
                } else {
                    let n = unsafe {
                        libc::read(fd, buf.as_mut_ptr() as *mut ::std::ffi::c_void, total)
                    };
                    if n < 0 {
                        return ::std::result::Result::Err(::std::io::Error::last_os_error());
                    }
                    bytes_read = n as usize;
                }
            }
            #[cfg(not(unix))]
            {
                unsafe extern "C" {
                    fn _lseek(fd: i32, off: i64, origin: i32) -> i64;
                }
                const SEEK_SET: i32 = 0;
                const SEEK_CUR: i32 = 1;
                let mut acc: usize = 0;
                if total > 0 {
                    for &len in &lens {
                        let n = if use_pread {
                            let saved = unsafe { _lseek(fd, 0, SEEK_CUR) };
                            unsafe { _lseek(fd, position + acc as i64, SEEK_SET) };
                            let n =
                                win_fd::_read(fd, buf.as_mut_ptr().add(acc) as *mut _, len as u32);
                            unsafe { _lseek(fd, saved, SEEK_SET) };
                            n
                        } else {
                            win_fd::_read(fd, buf.as_mut_ptr().add(acc) as *mut _, len as u32)
                        };
                        if n < 0 {
                            return ::std::result::Result::Err(::std::io::Error::last_os_error());
                        }
                        acc += n as usize;
                        if (n as usize) < len {
                            break;
                        }
                    }
                }
                bytes_read = acc;
            }
            buf.truncate(bytes_read);
            ::std::result::Result::Ok(FsAsyncResult::OkReadv {
                bytes_read: bytes_read as i32,
                buffer: buf,
            })
        },
    );
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_realpath(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let _encoding = get_encoding_opt(cx, &args, 1);

    if let Some((callback, cb_encoding)) = extract_callback_and_encoding(cx, &args, 1) {
        spawn_fs_async(
            cx,
            "realpath",
            path.clone(),
            callback,
            cb_encoding,
            move || {
                fs::canonicalize(&path)
                    .map(|p| FsAsyncResult::OkString(p.to_string_lossy().into_owned()))
            },
        );
        args.rval().set(UndefinedValue());
        return true;
    }

    match fs::canonicalize(&path) {
        ::std::result::Result::Ok(resolved) => {
            let s = resolved.to_string_lossy();
            let c_str = ZBox::from_bytes(s.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() {
                args.rval().set(UndefinedValue());
            } else {
                args.rval().set(mozjs::jsval::StringValue(&*js_str));
            }
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "realpath", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_rename(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let from = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let to = match get_path_arg(cx, &args, 1) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 2) {
        spawn_fs_async(cx, "rename", from.clone(), callback, None, move || {
            fs::rename(&from, &to).map(|_| FsAsyncResult::OkVoid)
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    match fs::rename(&from, &to) {
        ::std::result::Result::Ok(()) => {
            args.rval().set(UndefinedValue());
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "rename", &from, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_rm(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&path) {
        let c_msg = ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    let recursive = get_bool_option(cx, &args, 1, "recursive");
    let force = get_bool_option(cx, &args, 1, "force");

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 1) {
        spawn_fs_async(cx, "rm", path.clone(), callback, None, move || {
            rm_with_options(&path, recursive, force).map(|_| FsAsyncResult::OkVoid)
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    match rm_with_options(&path, recursive, force) {
        ::std::result::Result::Ok(()) => {
            args.rval().set(UndefinedValue());
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "rm", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_rmdir(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&path) {
        let c_msg = ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }

    // Upstream fff3b29d6: truthy `options.recursive` routes through rm
    // semantics (same parse/selection shape as fs_rm's callback branch —
    // get_bool_option reads arg 1 whether it is the options object or the
    // callback function; a function has no `recursive` boolean property).
    let recursive = get_bool_option(cx, &args, 1, "recursive");

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 1) {
        spawn_fs_async(cx, "rmdir", path.clone(), callback, None, move || {
            if recursive {
                fs::remove_dir_all(&path)
            } else {
                fs::remove_dir(&path)
            }
            .map(|_| FsAsyncResult::OkVoid)
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    let result = if recursive {
        fs::remove_dir_all(&path)
    } else {
        fs::remove_dir(&path)
    };
    match result {
        ::std::result::Result::Ok(()) => {
            args.rval().set(UndefinedValue());
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "rmdir", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_stat(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 1) {
        spawn_fs_async(cx, "stat", path.clone(), callback, None, move || {
            bun_fs::metadata(&path).map(FsAsyncResult::OkStat)
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    match bun_fs::metadata(&path) {
        ::std::result::Result::Ok(meta) => {
            let stats = create_stats_object(cx, &meta);
            args.rval().set(mozjs::jsval::ObjectValue(stats));
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "stat", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_symlink(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let target = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let path = match get_path_arg(cx, &args, 1) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 2) {
        spawn_fs_async(cx, "symlink", target.clone(), callback, None, move || {
            #[cfg(unix)]
            {
                ::std::os::unix::fs::symlink(&target, &path).map(|_| FsAsyncResult::OkVoid)
            }
            #[cfg(not(unix))]
            {
                fs::hard_link(&target, &path).map(|_| FsAsyncResult::OkVoid)
            }
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    #[cfg(unix)]
    let result = ::std::os::unix::fs::symlink(&target, &path);
    #[cfg(not(unix))]
    let result = fs::hard_link(&target, &path);
    match result {
        ::std::result::Result::Ok(()) => {
            args.rval().set(UndefinedValue());
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "symlink", &target, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_truncate(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let len_val = if argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };
    let len = truncate_len_from_val(len_val);

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 2) {
        spawn_fs_async(cx, "truncate", path.clone(), callback, None, move || {
            // write-open: ftruncate(2) rejects read-only fds (EINVAL)
            fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .and_then(|f| f.set_len(len.max(0) as u64))
                .map(|_| FsAsyncResult::OkVoid)
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    match fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .and_then(|f| f.set_len(len.max(0) as u64))
    {
        ::std::result::Result::Ok(()) => {
            args.rval().set(UndefinedValue());
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "truncate", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_unlink(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_write(&path) {
        let c_msg = ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 1) {
        spawn_fs_async(cx, "unlink", path.clone(), callback, None, move || {
            fs::remove_file(&path).map(|_| FsAsyncResult::OkVoid)
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    match fs::remove_file(&path) {
        ::std::result::Result::Ok(()) => {
            args.rval().set(UndefinedValue());
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "unlink", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_utimes(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let atime_val = if argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };
    let mtime_val = if argc > 2 {
        *args.get(2).ptr
    } else {
        UndefinedValue()
    };
    let atime = if atime_val.is_double() {
        atime_val.to_double()
    } else if atime_val.is_int32() {
        atime_val.to_int32() as f64
    } else {
        0.0
    };
    let mtime = if mtime_val.is_double() {
        mtime_val.to_double()
    } else if mtime_val.is_int32() {
        mtime_val.to_int32() as f64
    } else {
        0.0
    };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 3) {
        spawn_fs_async(cx, "utimes", path.clone(), callback, None, move || {
            #[cfg(unix)]
            {
                let c_path = ::std::ffi::CString::new(path.as_str()).unwrap_or_default();
                let tv = [
                    libc::timeval {
                        tv_sec: atime as i64,
                        tv_usec: ((atime % 1.0) * 1_000_000.0) as i64,
                    },
                    libc::timeval {
                        tv_sec: mtime as i64,
                        tv_usec: ((mtime % 1.0) * 1_000_000.0) as i64,
                    },
                ];
                let rv = unsafe { libc::utimes(c_path.as_ptr(), tv.as_ptr()) };
                if rv == 0 {
                    Ok(FsAsyncResult::OkVoid)
                } else {
                    Err(::std::io::Error::last_os_error())
                }
            }
            #[cfg(not(unix))]
            {
                Ok(FsAsyncResult::OkVoid)
            }
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    #[cfg(unix)]
    {
        let c_path = ::std::ffi::CString::new(path.as_str()).unwrap_or_default();
        let tv = [
            libc::timeval {
                tv_sec: atime as i64,
                tv_usec: ((atime % 1.0) * 1_000_000.0) as i64,
            },
            libc::timeval {
                tv_sec: mtime as i64,
                tv_usec: ((mtime % 1.0) * 1_000_000.0) as i64,
            },
        ];
        let rv = unsafe { libc::utimes(c_path.as_ptr(), tv.as_ptr()) };
        if rv == 0 {
            args.rval().set(UndefinedValue());
            true
        } else {
            throw_fs_error(cx, "utimes", &path, &::std::io::Error::last_os_error())
        }
    }
    #[cfg(not(unix))]
    {
        args.rval().set(UndefinedValue());
        true
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_write(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 {
        *args.get(0).ptr
    } else {
        UndefinedValue()
    };
    let fd = if fd_val.is_int32() {
        fd_val.to_int32()
    } else {
        -1
    };
    let (bytes, offset, length, use_pwrite, position) =
        match resolve_write_window(cx, &args, "write") {
            ::std::result::Result::Ok(w) => w,
            ::std::result::Result::Err(()) => return false,
        };

    // Node: write(fd, buffer[, offset[, length[, position]]], cb) and
    // write(fd, string[, position[, encoding]], cb). The callback is
    // REQUIRED (node v24.19.0 oracle: missing/non-function cb →
    // ERR_INVALID_ARG_TYPE); the former no-callback sync fallback is gone
    // (writeSync is the synchronous face).
    let callback = match extract_callback_and_encoding(cx, &args, 2) {
        Some((callback, _)) => callback,
        None => {
            return crate::bun_inspect_api::throw_invalid_arg_type(
                cx,
                &format!(
                    "The \"cb\" argument must be of type function. Received {}",
                    describe_received_fs_arg(cx, &args)
                ),
            );
        }
    };
    spawn_fs_async(
        cx,
        "write",
        format!("fd:{}", fd),
        callback,
        None,
        move || {
            let written = write_window(fd, &bytes, offset, length, use_pwrite, position);
            if written >= 0 {
                Ok(FsAsyncResult::OkWrite(written as i32))
            } else {
                Err(::std::io::Error::last_os_error())
            }
        },
    );
    args.rval().set(UndefinedValue());
    true
}

/// Node writev buffer-array flattening: each element is a Buffer/TypedArray
/// or string; the scatter-gather payload is the sequential concatenation.
/// Returns None when `buffers` is not an array-like object.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn collect_writev_bytes(
    cx: *mut JSContext,
    buffers_val: JSVal,
    op: &str,
) -> ::std::result::Result<Vec<u8>, ()> {
    if !buffers_val.is_object() {
        let c_msg = ZBox::from_bytes(format!("{}: buffers must be an array", op).as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return ::std::result::Result::Err(());
    }
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let buffers_obj = buffers_val.to_object());
    let mut all: Vec<u8> = Vec::new();
    let mut buf_idx: u32 = 0;
    loop {
        let mut elem = UndefinedValue();
        JS_GetElement(
            cx,
            buffers_obj.handle().into(),
            buf_idx,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut elem,
            },
        );
        if elem.is_undefined() {
            break;
        }
        if elem.is_string() {
            let s = elem.to_string();
            if !s.is_null() {
                all.extend_from_slice(&crate::jsstr_to_rust_string(cx, s).into_bytes());
            }
        } else if elem.is_object() {
            all.extend_from_slice(&crate::node_crypto::extract_buffer_bytes(cx, elem));
        }
        buf_idx += 1;
    }
    ::std::result::Result::Ok(all)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_writev(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 {
        *args.get(0).ptr
    } else {
        UndefinedValue()
    };
    let fd = if fd_val.is_int32() {
        fd_val.to_int32()
    } else {
        -1
    };
    let buffers_val = if argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };
    let bytes = match collect_writev_bytes(cx, buffers_val, "writev") {
        ::std::result::Result::Ok(b) => b,
        ::std::result::Result::Err(()) => return false,
    };
    let pos_arg = if argc > 2 {
        *args.get(2).ptr
    } else {
        UndefinedValue()
    };
    let pos = fs_number_arg(pos_arg);
    let (use_pwrite, position) = (pos.is_some(), pos.unwrap_or(-1.0) as i64);

    // Node: writev(fd, buffers[, position], callback).
    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 2) {
        spawn_fs_async(
            cx,
            "writev",
            format!("fd:{}", fd),
            callback,
            None,
            move || {
                let written =
                    write_window(fd, &bytes, 0, bytes.len(), use_pwrite, position);
                if written >= 0 {
                    Ok(FsAsyncResult::OkWrite(written as i32))
                } else {
                    Err(::std::io::Error::last_os_error())
                }
            },
        );
        args.rval().set(UndefinedValue());
        return true;
    }

    let written = write_window(fd, &bytes, 0, bytes.len(), use_pwrite, position);
    if written >= 0 {
        args.rval().set(mozjs::jsval::DoubleValue(written as f64));
        true
    } else {
        throw_fs_error(
            cx,
            "writev",
            &format!("fd:{}", fd),
            &::std::io::Error::last_os_error(),
        )
    }
}

// --- fs.promises ---

macro_rules! promise_simple_op {
    ($fn_name:ident, $op:expr, $op_name:expr) => {
        #[allow(unsafe_op_in_unsafe_fn)]
        unsafe extern "C" fn $fn_name(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
            let args = CallArgs::from_vp(vp, argc);
            let path = match get_path_arg(cx, &args, 0) { ::std::result::Result::Ok(p) => p, ::std::result::Result::Err(b) => return b };
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
            if promise.get().is_null() { args.rval().set(UndefinedValue()); return false; }
            match $op(&path) {
                ::std::result::Result::Ok(()) => {
                    resolve_undefined(cx, promise.get());
                }
                ::std::result::Result::Err(e) => {
                    reject_fs_error(cx, promise.get(), $op_name, &path, &e);
                }
            }
            args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
            true
        }
    };
}

promise_simple_op!(fs_promises_mkdir, |p: &str| fs::create_dir_all(p), "mkdir");
promise_simple_op!(fs_promises_unlink, |p: &str| fs::remove_file(p), "unlink");

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_rename(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let from = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let to = match get_path_arg(cx, &args, 1) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    match fs::rename(&from, &to) {
        ::std::result::Result::Ok(()) => resolve_undefined(cx, promise.get()),
        ::std::result::Result::Err(e) => {
            reject_fs_error(cx, promise.get(), "rename", &from, &e)
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_copy_file(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let from = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let to = match get_path_arg(cx, &args, 1) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    match fs::copy(&from, &to) {
        ::std::result::Result::Ok(_) => resolve_undefined(cx, promise.get()),
        ::std::result::Result::Err(e) => {
            reject_fs_error(cx, promise.get(), "copyFile", &from, &e)
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_read_file(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let encoding = get_encoding_opt(cx, &args, 1);
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }

    match bun_fs::read(&path) {
        ::std::result::Result::Ok(data) => {
            let val = string_or_buffer(cx, &data, encoding.as_deref());
            rooted!(&in(cx_ref) let val_rooted = val);
            unsafe {
                mozjs_sys::jsapi::JS::ResolvePromise(
                    cx,
                    promise.handle().into(),
                    val_rooted.handle().into(),
                );
            }
        }
        ::std::result::Result::Err(e) => {
            reject_fs_error(cx, promise.get(), "readFile", &path, &e)
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_write_file(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let data_val = if argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };
    // Node: promises.writeFile(file, data[, options]) — options.flag and
    // options.encoding honored (string data encodes per encoding, same
    // single-source helper as the sync/callback faces).
    let (encoding, flag) = write_file_options(cx, &args, 2);
    let flag = flag.unwrap_or_else(|| "w".to_string());
    let bytes = write_file_data_bytes(cx, data_val, encoding.as_deref());
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    match write_file_with_flag(&path, &bytes, &flag) {
        ::std::result::Result::Ok(()) => resolve_undefined(cx, promise.get()),
        ::std::result::Result::Err(e) => {
            reject_fs_error(cx, promise.get(), "writeFile", &path, &e)
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_stat(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    match bun_fs::metadata(&path) {
        ::std::result::Result::Ok(meta) => {
            let stats = create_stats_object(cx, &meta);
            rooted!(&in(cx_ref) let val = mozjs::jsval::ObjectValue(stats));
            unsafe {
                mozjs_sys::jsapi::JS::ResolvePromise(
                    cx,
                    promise.handle().into(),
                    val.handle().into(),
                );
            }
        }
        ::std::result::Result::Err(e) => {
            reject_fs_error(cx, promise.get(), "stat", &path, &e)
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_readdir(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };

    let mut wrapped_cx =
        unsafe { mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx)) };
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }

    match fs::read_dir(&path) {
        ::std::result::Result::Ok(entries) => {
            let names: Vec<::std::string::String> = entries
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            rooted!(&in(cx_ref) let arr = unsafe { w2::NewArrayObject1(cx_ref, names.len()) });
            if arr.get().is_null() {
                args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
                return true;
            }
            for (idx, name) in names.iter().enumerate() {
                let c_name = ZBox::from_bytes(name.as_bytes());
                let js_str = unsafe { JS_NewStringCopyZ(cx, c_name.as_ptr()) };
                if !js_str.is_null() {
                    rooted!(&in(cx_ref) let val = mozjs::jsval::StringValue(&*js_str));
                    unsafe {
                        JS_DefineElement(
                            cx,
                            arr.handle().into(),
                            idx as u32,
                            val.handle().into(),
                            JSPROP_ENUMERATE as u32,
                        );
                    }
                }
            }
            rooted!(&in(cx_ref) let arr_val = mozjs::jsval::ObjectValue(arr.get()));
            unsafe {
                mozjs_sys::jsapi::JS::ResolvePromise(
                    cx,
                    promise.handle().into(),
                    arr_val.handle().into(),
                );
            }
        }
        ::std::result::Result::Err(e) => {
            reject_fs_error(cx, promise.get(), "readdir", &path, &e)
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_lstat(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    match fs::symlink_metadata(&path) {
        ::std::result::Result::Ok(meta) => {
            let posix = metadata_to_posix_stat(&meta);
            let stats = create_stats_object(cx, &posix);
            rooted!(&in(cx_ref) let val = mozjs::jsval::ObjectValue(stats));
            unsafe {
                mozjs_sys::jsapi::JS::ResolvePromise(
                    cx,
                    promise.handle().into(),
                    val.handle().into(),
                );
            }
        }
        ::std::result::Result::Err(e) => {
            reject_fs_error(cx, promise.get(), "lstat", &path, &e)
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_append_file(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let data_val = if argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };
    // promises.appendFile(string, options.encoding) — same encode face.
    let encoding = get_encoding_opt(cx, &args, 2);
    let data = write_file_data_bytes(cx, data_val, encoding.as_deref());
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    match ::std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| ::std::io::Write::write_all(&mut f, &data))
    {
        ::std::result::Result::Ok(()) => resolve_undefined(cx, promise.get()),
        ::std::result::Result::Err(e) => {
            reject_fs_error(cx, promise.get(), "appendFile", &path, &e)
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_chmod(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let mode_val = if argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };
    let mode = if mode_val.is_int32() {
        mode_val.to_int32() as u32
    } else if mode_val.is_double() {
        mode_val.to_double() as u32
    } else {
        0o644
    };
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    #[cfg(unix)]
    let result = {
        use ::std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(mode))
    };
    #[cfg(not(unix))]
    let result = set_win_readonly(&path, mode as i32);
    match result {
        ::std::result::Result::Ok(()) => resolve_undefined(cx, promise.get()),
        ::std::result::Result::Err(e) => {
            reject_fs_error(cx, promise.get(), "chmod", &path, &e)
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_chown(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let uid = if argc > 1 {
        let v = *args.get(1).ptr;
        if v.is_int32() { v.to_int32() as u32 } else { 0 }
    } else {
        0
    };
    let gid = if argc > 2 {
        let v = *args.get(2).ptr;
        if v.is_int32() { v.to_int32() as u32 } else { 0 }
    } else {
        0
    };
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    #[cfg(unix)]
    let result = ::std::os::unix::fs::chown(&path, Some(uid), Some(gid));
    #[cfg(not(unix))]
    let result: ::std::io::Result<()> = Ok(());
    match result {
        ::std::result::Result::Ok(()) => resolve_undefined(cx, promise.get()),
        ::std::result::Result::Err(e) => {
            reject_fs_error(cx, promise.get(), "chown", &path, &e)
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_access(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    match fs::metadata(&path) {
        ::std::result::Result::Ok(_) => resolve_undefined(cx, promise.get()),
        ::std::result::Result::Err(e) => {
            reject_fs_error(cx, promise.get(), "access", &path, &e)
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_rm(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let recursive = get_bool_option(cx, &args, 1, "recursive");
    let force = get_bool_option(cx, &args, 1, "force");
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    match rm_with_options(&path, recursive, force) {
        ::std::result::Result::Ok(()) => resolve_undefined(cx, promise.get()),
        ::std::result::Result::Err(e) => {
            reject_fs_error(cx, promise.get(), "rm", &path, &e)
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_rmdir(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    // Upstream fff3b29d6: truthy `options.recursive` routes through rm
    // semantics; absent/false keeps plain rmdir (same shape as
    // fs_promises_rm).
    let recursive = get_bool_option(cx, &args, 1, "recursive");
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    let result = if recursive {
        fs::remove_dir_all(&path)
    } else {
        fs::remove_dir(&path)
    };
    match result {
        ::std::result::Result::Ok(()) => resolve_undefined(cx, promise.get()),
        ::std::result::Result::Err(e) => {
            reject_fs_error(cx, promise.get(), "rmdir", &path, &e)
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_realpath(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    match fs::canonicalize(&path) {
        ::std::result::Result::Ok(resolved) => {
            let s = resolved.to_string_lossy();
            let c_str = ZBox::from_bytes(s.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if !js_str.is_null() {
                rooted!(&in(cx_ref) let val = mozjs::jsval::StringValue(&*js_str));
                unsafe {
                    mozjs_sys::jsapi::JS::ResolvePromise(
                        cx,
                        promise.handle().into(),
                        val.handle().into(),
                    );
                }
            } else {
                resolve_undefined(cx, promise.get());
            }
        }
        ::std::result::Result::Err(e) => {
            reject_fs_error(cx, promise.get(), "realpath", &path, &e)
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_readlink(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    match fs::read_link(&path) {
        ::std::result::Result::Ok(target) => {
            let s = target.to_string_lossy();
            let c_str = ZBox::from_bytes(s.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if !js_str.is_null() {
                rooted!(&in(cx_ref) let val = mozjs::jsval::StringValue(&*js_str));
                unsafe {
                    mozjs_sys::jsapi::JS::ResolvePromise(
                        cx,
                        promise.handle().into(),
                        val.handle().into(),
                    );
                }
            } else {
                resolve_undefined(cx, promise.get());
            }
        }
        ::std::result::Result::Err(e) => {
            reject_fs_error(cx, promise.get(), "readlink", &path, &e)
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_symlink(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let target = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let path = match get_path_arg(cx, &args, 1) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    #[cfg(unix)]
    let result = ::std::os::unix::fs::symlink(&target, &path);
    #[cfg(not(unix))]
    let result = fs::hard_link(&target, &path);
    match result {
        ::std::result::Result::Ok(()) => resolve_undefined(cx, promise.get()),
        ::std::result::Result::Err(e) => {
            reject_fs_error(cx, promise.get(), "symlink", &target, &e)
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_link(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let from = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let to = match get_path_arg(cx, &args, 1) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    match fs::hard_link(&from, &to) {
        ::std::result::Result::Ok(()) => resolve_undefined(cx, promise.get()),
        ::std::result::Result::Err(e) => {
            reject_fs_error(cx, promise.get(), "link", &from, &e)
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_truncate(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let len_val = if argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };
    // Same undefined≡absent≡0 rule as every other truncate face; `.max(0)`
    // aligns the promise face with the sync/callback faces (which clamp
    // instead of wrapping negative lens into u64 overflow).
    let len = truncate_len_from_val(len_val).max(0) as u64;
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    // write-open: ftruncate(2) rejects read-only fds (EINVAL)
    match fs::OpenOptions::new().write(true).open(&path).and_then(|f| f.set_len(len)) {
        ::std::result::Result::Ok(()) => resolve_undefined(cx, promise.get()),
        ::std::result::Result::Err(e) => {
            reject_fs_error(cx, promise.get(), "truncate", &path, &e)
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_utimes(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let atime_val = if argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };
    let mtime_val = if argc > 2 {
        *args.get(2).ptr
    } else {
        UndefinedValue()
    };
    let atime = if atime_val.is_double() {
        atime_val.to_double()
    } else if atime_val.is_int32() {
        atime_val.to_int32() as f64
    } else {
        0.0
    };
    let mtime = if mtime_val.is_double() {
        mtime_val.to_double()
    } else if mtime_val.is_int32() {
        mtime_val.to_int32() as f64
    } else {
        0.0
    };
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    #[cfg(unix)]
    {
        let c_path = ::std::ffi::CString::new(path.as_str()).unwrap_or_default();
        let tv = [
            libc::timeval {
                tv_sec: atime as i64,
                tv_usec: ((atime % 1.0) * 1_000_000.0) as i64,
            },
            libc::timeval {
                tv_sec: mtime as i64,
                tv_usec: ((mtime % 1.0) * 1_000_000.0) as i64,
            },
        ];
        let rv = unsafe { libc::utimes(c_path.as_ptr(), tv.as_ptr()) };
        if rv == 0 {
            resolve_undefined(cx, promise.get());
        } else {
            reject_fs_error(
                cx,
                promise.get(),
                "utimes",
                &path,
                &::std::io::Error::last_os_error(),
            );
        }
    }
    #[cfg(not(unix))]
    {
        resolve_undefined(cx, promise.get());
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_mkdtemp(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let prefix = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    match mkdtemp_inner(&prefix) {
        ::std::result::Result::Ok(dir) => {
            let c_str = ZBox::from_bytes(dir.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if !js_str.is_null() {
                rooted!(&in(cx_ref) let val = mozjs::jsval::StringValue(&*js_str));
                unsafe {
                    mozjs_sys::jsapi::JS::ResolvePromise(
                        cx,
                        promise.handle().into(),
                        val.handle().into(),
                    );
                }
            } else {
                resolve_undefined(cx, promise.get());
            }
        }
        ::std::result::Result::Err(e) => {
            reject_fs_error(cx, promise.get(), "mkdtemp", &prefix, &e)
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_open(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let flags_val = if argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };
    let flags = if flags_val.is_int32() {
        flags_val.to_int32()
    } else if flags_val.is_string() {
        let s = flags_val.to_string();
        if !s.is_null() {
            parse_open_flags(&crate::jsstr_to_rust_string(cx, s))
        } else {
            0
        }
    } else {
        0
    };
    let mode_val = if argc > 2 {
        *args.get(2).ptr
    } else {
        UndefinedValue()
    };
    let mode = if mode_val.is_int32() {
        mode_val.to_int32() as u32
    } else {
        0o644
    };
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    #[cfg(unix)]
    {
        let c_path = ::std::ffi::CString::new(path.as_str()).unwrap_or_default();
        let fd = unsafe { libc::open(c_path.as_ptr(), flags, mode) };
        if fd >= 0 {
            // Wrap the fd in a FileHandle instance before resolving the promise.
            let fh = create_filehandle_object(cx, fd);
            if !fh.is_null() {
                rooted!(&in(cx_ref) let fh_val = mozjs::jsval::ObjectValue(fh));
                unsafe {
                    mozjs_sys::jsapi::JS::ResolvePromise(
                        cx,
                        promise.handle().into(),
                        fh_val.handle().into(),
                    );
                }
            } else {
                // Fallback: resolve with raw fd
                rooted!(&in(cx_ref) let val = mozjs::jsval::Int32Value(fd));
                unsafe {
                    mozjs_sys::jsapi::JS::ResolvePromise(
                        cx,
                        promise.handle().into(),
                        val.handle().into(),
                    );
                }
            }
        } else {
            reject_fs_error(
                cx,
                promise.get(),
                "open",
                &path,
                &::std::io::Error::last_os_error(),
            );
        }
    }
    #[cfg(not(unix))]
    {
        resolve_undefined(cx, promise.get());
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_read(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 {
        *args.get(0).ptr
    } else {
        UndefinedValue()
    };
    let fd = if fd_val.is_int32() {
        fd_val.to_int32()
    } else {
        -1
    };
    let length = if argc > 3 {
        let v = *args.get(3).ptr;
        if v.is_int32() {
            v.to_int32() as usize
        } else {
            65536
        }
    } else {
        65536
    };
    // node contract: a numeric position reads at that ABSOLUTE offset without
    // moving the cursor (pread); null/absent reads at the cursor.
    let pos = if argc > 4 {
        fs_number_arg(*args.get(4).ptr)
    } else {
        ::std::option::Option::None
    };
    let (use_pread, position) = (pos.is_some(), pos.unwrap_or(0.0) as i64);
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    let mut buf = vec![0u8; length];
    #[cfg(unix)]
    {
        let bytes_read = if use_pread {
            unsafe {
                libc::pread(
                    fd,
                    buf.as_mut_ptr() as *mut ::std::ffi::c_void,
                    length,
                    position,
                )
            }
        } else {
            unsafe { libc::read(fd, buf.as_mut_ptr() as *mut ::std::ffi::c_void, length) }
        };
        if bytes_read >= 0 {
            buf.truncate(bytes_read as usize);
            // node contract: read(buffer, offset, length, position) ALSO fills the
            // caller's buffer window [offset, offset + bytesRead) — the data previously
            // landed only in the fresh result buffer, leaving the caller's zero-filled
            // (C11 wave; readv C1 #3 sibling; node v24.19.0 oracle:
            // fh.read(b,0,2,0) fills b).
            {
                let buf_arg = *args.get(1).ptr;
                if buf_arg.is_object() {
                    let off_rb: u32 = if argc > 2 {
                        let v = *args.get(2).ptr;
                        if v.is_int32() { v.to_int32().max(0) as u32 } else { 0 }
                    } else {
                        0
                    };
                    let mut wrapped_rb =
                        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
                    let cx_rb = &mut wrapped_rb;
                    rooted!(&in(cx_rb) let caller_rb = buf_arg.to_object());
                    for (i_rb, &byte_rb) in buf.iter().enumerate() {
                        rooted!(&in(cx_rb) let bv_rb = mozjs::jsval::Int32Value(byte_rb as i32));
                        JS_SetElement(
                            cx,
                            caller_rb.handle().into(),
                            off_rb + i_rb as u32,
                            bv_rb.handle().into(),
                        );
                    }
                }
            }
            let buf_obj = crate::globals::create_buffer_object(cx, &buf);
            if !buf_obj.is_null() {
                rooted!(&in(cx_ref) let val = mozjs::jsval::ObjectValue(buf_obj));
                unsafe {
                    mozjs_sys::jsapi::JS::ResolvePromise(
                        cx,
                        promise.handle().into(),
                        val.handle().into(),
                    );
                }
            } else {
                resolve_undefined(cx, promise.get());
            }
        } else {
            reject_with_error(
                cx,
                promise.get(),
                &format!("read fd:{}: {}", fd, ::std::io::Error::last_os_error()),
            );
        }
    }
    #[cfg(not(unix))]
    {
        resolve_undefined(cx, promise.get());
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_write(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 {
        *args.get(0).ptr
    } else {
        UndefinedValue()
    };
    let fd = if fd_val.is_int32() {
        fd_val.to_int32()
    } else {
        -1
    };
    let data_val = if argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };
    let bytes = if data_val.is_string() {
        let s = data_val.to_string();
        if !s.is_null() {
            crate::jsstr_to_rust_string(cx, s).into_bytes()
        } else {
            Vec::new()
        }
    } else if data_val.is_object() {
        crate::node_crypto::extract_buffer_bytes(cx, data_val)
    } else {
        Vec::new()
    };
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    #[cfg(unix)]
    {
        let written =
            unsafe { libc::write(fd, bytes.as_ptr() as *const ::std::ffi::c_void, bytes.len()) };
        if written >= 0 {
            rooted!(&in(cx_ref) let val = mozjs::jsval::DoubleValue(written as f64));
            unsafe {
                mozjs_sys::jsapi::JS::ResolvePromise(
                    cx,
                    promise.handle().into(),
                    val.handle().into(),
                );
            }
        } else {
            reject_with_error(
                cx,
                promise.get(),
                &format!("write fd:{}: {}", fd, ::std::io::Error::last_os_error()),
            );
        }
    }
    #[cfg(not(unix))]
    {
        resolve_undefined(cx, promise.get());
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

// --- statfs ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_statfs_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    #[cfg(unix)]
    {
        let c_path = ::std::ffi::CString::new(path.as_str()).unwrap_or_default();
        let mut buf: libc::statfs = ::std::mem::zeroed();
        let rv = unsafe { libc::statfs(c_path.as_ptr(), &mut buf) };
        if rv == 0 {
            let sf = StatfsResult {
                type_: buf.f_type as u64,
                bsize: buf.f_bsize as u64,
                frsize: buf.f_frsize as u64,
                blocks: buf.f_blocks as u64,
                bfree: buf.f_bfree as u64,
                bavail: buf.f_bavail as u64,
                files: buf.f_files as u64,
                ffree: buf.f_ffree as u64,
            };
            let obj = create_statfs_object(cx, &sf);
            args.rval().set(mozjs::jsval::ObjectValue(obj));
            true
        } else {
            throw_fs_error(cx, "statfsSync", &path, &::std::io::Error::last_os_error())
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        args.rval().set(UndefinedValue());
        true
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_statfs(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 1) {
        spawn_fs_async(cx, "statfs", path.clone(), callback, None, move || {
            #[cfg(unix)]
            {
                let c_path = ::std::ffi::CString::new(path.as_str()).unwrap_or_default();
                let mut buf: libc::statfs = ::std::mem::zeroed();
                let rv = unsafe { libc::statfs(c_path.as_ptr(), &mut buf) };
                if rv == 0 {
                    Ok(FsAsyncResult::OkStatfs(StatfsResult {
                        type_: buf.f_type as u64,
                        bsize: buf.f_bsize as u64,
                        frsize: buf.f_frsize as u64,
                        blocks: buf.f_blocks as u64,
                        bfree: buf.f_bfree as u64,
                        bavail: buf.f_bavail as u64,
                        files: buf.f_files as u64,
                        ffree: buf.f_ffree as u64,
                    }))
                } else {
                    Err(::std::io::Error::last_os_error())
                }
            }
            #[cfg(not(unix))]
            {
                Ok(FsAsyncResult::OkStatfs(StatfsResult {
                    type_: 0,
                    bsize: 0,
                    frsize: 0,
                    blocks: 0,
                    bfree: 0,
                    bavail: 0,
                    files: 0,
                    ffree: 0,
                }))
            }
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    #[cfg(unix)]
    {
        let c_path = ::std::ffi::CString::new(path.as_str()).unwrap_or_default();
        let mut buf: libc::statfs = ::std::mem::zeroed();
        let rv = unsafe { libc::statfs(c_path.as_ptr(), &mut buf) };
        if rv == 0 {
            let sf = StatfsResult {
                type_: buf.f_type as u64,
                bsize: buf.f_bsize as u64,
                frsize: buf.f_frsize as u64,
                blocks: buf.f_blocks as u64,
                bfree: buf.f_bfree as u64,
                bavail: buf.f_bavail as u64,
                files: buf.f_files as u64,
                ffree: buf.f_ffree as u64,
            };
            let obj = create_statfs_object(cx, &sf);
            args.rval().set(mozjs::jsval::ObjectValue(obj));
            true
        } else {
            throw_fs_error(cx, "statfs", &path, &::std::io::Error::last_os_error())
        }
    }
    #[cfg(not(unix))]
    {
        args.rval().set(UndefinedValue());
        true
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_statfs(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }

    #[cfg(unix)]
    {
        let c_path = ::std::ffi::CString::new(path.as_str()).unwrap_or_default();
        let mut buf: libc::statfs = ::std::mem::zeroed();
        let rv = unsafe { libc::statfs(c_path.as_ptr(), &mut buf) };
        if rv == 0 {
            let sf = StatfsResult {
                type_: buf.f_type as u64,
                bsize: buf.f_bsize as u64,
                frsize: buf.f_bsize as u64,
                blocks: buf.f_blocks as u64,
                bfree: buf.f_bfree as u64,
                bavail: buf.f_bavail as u64,
                files: buf.f_files as u64,
                ffree: buf.f_ffree as u64,
            };
            let obj = create_statfs_object(cx, &sf);
            rooted!(&in(cx_ref) let val = mozjs::jsval::ObjectValue(obj));
            unsafe {
                mozjs_sys::jsapi::JS::ResolvePromise(
                    cx,
                    promise.handle().into(),
                    val.handle().into(),
                );
            }
        } else {
            reject_fs_error(
                cx,
                promise.get(),
                "statfs",
                &path,
                &::std::io::Error::last_os_error(),
            );
        }
    }
    #[cfg(not(unix))]
    {
        resolve_undefined(cx, promise.get());
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

// --- fd sync operations ---

#[allow(unsafe_op_in_unsafe_fn)]

// ── Windows sync fd face (UCRT) ─────────────────────────────────────────────
// The POSIX sync family uses libc fds directly; the windows arms were silent
// stubs (openSync returned a fake fd 0 — every later fd op silently "worked"
// on stdin). The UCRT exports below give the same fd contract on windows:
// _wopen takes a UTF-16 path; _chsize/_read/_write/_close operate on CRT fds.
#[cfg(windows)]
mod win_fd {
    unsafe extern "C" {
        pub fn _wopen(path: *const u16, oflag: i32, ...) -> i32;
        pub fn _close(fd: i32) -> i32;
        pub fn _chsize(fd: i32, size: i64) -> i32;
        pub fn _read(fd: i32, buf: *mut core::ffi::c_void, count: u32) -> i32;
        pub fn _write(fd: i32, buf: *const core::ffi::c_void, count: u32) -> i32;
    }
    pub fn open_w(path: &str, oflag: i32, mode: u32) -> i32 {
        let mut w: Vec<u16> = path.encode_utf16().collect();
        w.push(0);
        // SAFETY: `w` is NUL-terminated; oflag/mode are the CRT contract.
        unsafe { _wopen(w.as_ptr(), oflag, mode) }
    }
}

unsafe extern "C" fn fs_open_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let flags_val = if argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };
    let flags = if flags_val.is_int32() {
        flags_val.to_int32()
    } else if flags_val.is_string() {
        let s = flags_val.to_string();
        if !s.is_null() {
            parse_open_flags(&crate::jsstr_to_rust_string(cx, s))
        } else {
            0
        }
    } else {
        0
    };
    let mode_val = if argc > 2 {
        *args.get(2).ptr
    } else {
        UndefinedValue()
    };
    let mode = if mode_val.is_int32() {
        mode_val.to_int32() as u32
    } else {
        0o644
    };
    #[cfg(unix)]
    {
        let c_path = ::std::ffi::CString::new(path.as_str()).unwrap_or_default();
        let fd = unsafe { libc::open(c_path.as_ptr(), flags, mode) };
        if fd >= 0 {
            args.rval().set(mozjs::jsval::Int32Value(fd));
            true
        } else {
            throw_fs_error(cx, "openSync", &path, &::std::io::Error::last_os_error())
        }
    }
    #[cfg(windows)]
    {
        // CRT fd via _wopen (parse_open_flags yields the libc::O_* bits,
        // which are the CRT constants on windows).
        let fd = win_fd::open_w(&path, flags, mode);
        if fd >= 0 {
            args.rval().set(mozjs::jsval::Int32Value(fd));
            true
        } else {
            throw_fs_error(cx, "openSync", &path, &::std::io::Error::last_os_error())
        }
    }
}

/// Shared sync fd-void op core (`closeSync`/`fsyncSync`): identical arg
/// extraction + error delivery; the two faces differ only in the op label
/// and the syscall (REQ-ARCH-001 duplicate convergence).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn fs_fd_void_sync_op(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
    op: &'static str,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 {
        *args.get(0).ptr
    } else {
        UndefinedValue()
    };
    let fd = if fd_val.is_int32() {
        fd_val.to_int32()
    } else {
        -1
    };
    #[cfg(unix)]
    {
        let rv = fd_void_syscall(op, fd);
        if rv == 0 {
            args.rval().set(UndefinedValue());
            true
        } else {
            throw_fs_error(
                cx,
                &format!("{}Sync", op),
                &format!("fd:{}", fd),
                &::std::io::Error::last_os_error(),
            )
        }
    }
    #[cfg(not(unix))]
    {
        args.rval().set(UndefinedValue());
        true
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_close_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    fs_fd_void_sync_op(cx, argc, vp, "close")
}

// BCE-20260816-FS-READSYNC — the old implementation ignored the caller's
// buffer argument entirely: it read into a throwaway Vec (the created Buffer
// object was dropped unused) and returned only the byte count, so the
// canonical `fd = openSync(p, 'r'); readSync(fd, buf, 0, n, 0)` pattern left
// `buf` zeroed — the "openSync+readSync combo dead" audit item. Node
// semantics (fs.readSync(fd, buffer, offset, length, position)): bytes are
// written into the CALLER'S typed array at `offset`, `length` caps the read,
// and a numeric `position` reads via pread without moving the fd cursor
// (null/undefined position = current cursor via read).
// fs.truncateSync(path, len) — path-based truncate (the fd-based
// ftruncateSync already existed; the path form was missing entirely, so
// `typeof fs.truncateSync === 'undefined'`). Opened WRITE: ftruncate(2)
// rejects read-only fds with EINVAL — a read-only File::open + set_len
// always failed (probe: truncateSync EINVAL).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_truncate_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let len_val = if argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };
    let len = truncate_len_from_val(len_val);
    match fs::OpenOptions::new().write(true).open(&path).and_then(|f| f.set_len(len.max(0) as u64)) {
        ::std::result::Result::Ok(()) => {
            args.rval().set(UndefinedValue());
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "truncateSync", &path, &e),
    }
}

// fs.opendirSync(path) — same Dir object as the async opendir() (readSync /
// closeSync / async iteration), returned synchronously.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_opendir_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    match fs::metadata(&path) {
        ::std::result::Result::Ok(meta) if meta.is_dir() => {
            let dir_obj = create_dir_object(cx, &path);
            args.rval().set(mozjs::jsval::ObjectValue(dir_obj));
            true
        }
        ::std::result::Result::Ok(_) => {
            JS_ReportErrorUTF8(cx, c"opendirSync: path is not a directory".as_ptr());
            false
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "opendirSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_read_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 {
        *args.get(0).ptr
    } else {
        UndefinedValue()
    };
    let fd = if fd_val.is_int32() {
        fd_val.to_int32()
    } else {
        -1
    };

    // Caller-supplied buffer: must be a typed array (Buffer IS a Uint8Array).
    let buf_val = if argc > 1 { *args.get(1).ptr } else { UndefinedValue() };
    if !buf_val.is_object() {
        JS_ReportErrorUTF8(
            cx,
            c"readSync: buffer argument must be a Buffer or Uint8Array".as_ptr(),
        );
        return false;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let buf_obj = buf_val.to_object());
    if !mozjs_sys::jsapi::JS_IsArrayBufferViewObject(buf_obj.get()) {
        JS_ReportErrorUTF8(
            cx,
            c"readSync: buffer argument must be a Buffer or Uint8Array".as_ptr(),
        );
        return false;
    }

    let byte_len = mozjs_sys::jsapi::JS_GetTypedArrayByteLength(buf_obj.get()) as usize;

    let to_num = |v: JSVal| -> Option<f64> {
        if v.is_number() {
            Some(v.to_number())
        } else if v.is_int32() {
            Some(v.to_int32() as f64)
        } else {
            None
        }
    };
    let offset = if argc > 2 {
        to_num(*args.get(2).ptr).unwrap_or(0.0)
    } else {
        0.0
    };
    let length = if argc > 3 {
        to_num(*args.get(3).ptr)
            .filter(|n| *n >= 0.0)
            .map(|n| n as usize)
            .unwrap_or(byte_len)
    } else {
        byte_len
    };
    let position_arg = if argc > 4 { *args.get(4).ptr } else { UndefinedValue() };
    let use_pread = position_arg.is_number() || position_arg.is_int32();
    let position: i64 = if use_pread {
        to_num(position_arg).unwrap_or(0.0) as i64
    } else {
        -1
    };

    if offset < 0.0 || offset as usize > byte_len {
        JS_ReportErrorUTF8(
            cx,
            c"readSync: offset is out of bounds".as_ptr(),
        );
        return false;
    }
    let offset = offset as usize;
    if length > byte_len - offset {
        JS_ReportErrorUTF8(
            cx,
            c"readSync: length extends beyond buffer".as_ptr(),
        );
        return false;
    }
    if length == 0 {
        args.rval().set(mozjs::jsval::Int32Value(0));
        return true;
    }

    #[cfg(unix)]
    {
        // Rooted view — data pointer stays valid across the read (no JS runs).
        let mut is_shared = false;
        let data_ptr = mozjs_sys::jsapi::JS_GetUint8ArrayData(
            buf_obj.get(),
            &mut is_shared,
            ::std::ptr::null(),
        );
        if data_ptr.is_null() {
            JS_ReportErrorUTF8(
                cx,
                c"readSync: cannot access buffer storage".as_ptr(),
            );
            return false;
        }
        let dst = unsafe { data_ptr.add(offset) };
        let bytes_read = if use_pread {
            unsafe {
                libc::pread(
                    fd,
                    dst as *mut ::std::ffi::c_void,
                    length,
                    position,
                )
            }
        } else {
            unsafe { libc::read(fd, dst as *mut ::std::ffi::c_void, length) }
        };
        if bytes_read >= 0 {
            args.rval().set(mozjs::jsval::Int32Value(bytes_read as i32));
            true
        } else {
            throw_fs_error(
                cx,
                "readSync",
                &format!("fd:{}", fd),
                &::std::io::Error::last_os_error(),
            )
        }
    }
    #[cfg(windows)]
    {
        // SAFETY: rooted view — data pointer stays valid across the read
        // (no JS runs); _read operates on the CRT fd.
        let mut is_shared = false;
        let data_ptr = mozjs_sys::jsapi::JS_GetUint8ArrayData(
            buf_obj.get(),
            &mut is_shared,
            ::std::ptr::null(),
        );
        if data_ptr.is_null() {
            JS_ReportErrorUTF8(cx, c"readSync: cannot access buffer storage".as_ptr());
            return false;
        }
        let dst = unsafe { data_ptr.add(offset) };
        // Positional read on windows: seek-free via _lseek save/restore.
        let bytes_read = if use_pread {
            unsafe extern "C" {
                fn _lseek(fd: i32, off: i64, origin: i32) -> i64;
            }
            const SEEK_SET: i32 = 0;
            unsafe {
                let saved = _lseek(fd, 0, 1 /* SEEK_CUR */);
                _lseek(fd, position, SEEK_SET);
                let n = win_fd::_read(fd, dst as *mut _, length as u32);
                _lseek(fd, saved, SEEK_SET);
                n
            }
        } else {
            // SAFETY: plain CRT fd read.
            unsafe { win_fd::_read(fd, dst as *mut _, length as u32) }
        };
        if bytes_read >= 0 {
            args.rval().set(mozjs::jsval::Int32Value(bytes_read));
            true
        } else {
            throw_fs_error(
                cx,
                "readSync",
                &format!("fd:{}", fd),
                &::std::io::Error::last_os_error(),
            )
        }
    }
}

/// Resolve node's write positional contract for the sync and async faces:
///   buffer form: write(fd, buffer[, offset[, length[, position]]])
///   string form: write(fd, string[, position[, encoding]])
/// Returns (bytes, offset, length, use_pwrite, position). `position` numeric
/// (including 0) → pwrite at that absolute offset; null/undefined → current
/// fd cursor. Out-of-range offset/length reports a JS error and returns Err.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn resolve_write_window(
    cx: *mut JSContext,
    args: &CallArgs,
    op: &str,
) -> ::std::result::Result<(Vec<u8>, usize, usize, bool, i64), ()> {
    let data_val = if args.argc_ > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };
    if data_val.is_string() {
        let s = data_val.to_string();
        let bytes = if !s.is_null() {
            crate::jsstr_to_rust_string(cx, s).into_bytes()
        } else {
            Vec::new()
        };
        let len = bytes.len();
        let pos_arg = if args.argc_ > 2 {
            *args.get(2).ptr
        } else {
            UndefinedValue()
        };
        let pos = fs_number_arg(pos_arg);
        ::std::result::Result::Ok((bytes, 0, len, pos.is_some(), pos.unwrap_or(-1.0) as i64))
    } else if data_val.is_object() {
        let bytes = crate::node_crypto::extract_buffer_bytes(cx, data_val);
        let byte_len = bytes.len();
        let offset = if args.argc_ > 2 {
            fs_number_arg(*args.get(2).ptr).unwrap_or(0.0)
        } else {
            0.0
        };
        if !offset.is_finite() || offset < 0.0 || offset as usize > byte_len {
            let c_msg = ZBox::from_bytes(format!("{}: offset is out of bounds", op).as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return ::std::result::Result::Err(());
        }
        let offset = offset as usize;
        let length = if args.argc_ > 3 {
            match fs_number_arg(*args.get(3).ptr) {
                Some(n) => n,
                None => (byte_len - offset) as f64,
            }
        } else {
            (byte_len - offset) as f64
        };
        if !length.is_finite() || length < 0.0 || length as usize > byte_len - offset {
            let c_msg = ZBox::from_bytes(format!("{}: length extends beyond buffer", op).as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return ::std::result::Result::Err(());
        }
        let length = length as usize;
        let pos_arg = if args.argc_ > 4 {
            *args.get(4).ptr
        } else {
            UndefinedValue()
        };
        let pos = fs_number_arg(pos_arg);
        ::std::result::Result::Ok((bytes, offset, length, pos.is_some(), pos.unwrap_or(-1.0) as i64))
    } else {
        ::std::result::Result::Ok((Vec::new(), 0, 0, false, -1))
    }
}

/// One write syscall for a resolved window. `pwrite` leaves the fd cursor
/// untouched (node's positional form); the cursor form advances it.
#[cfg(unix)]
fn write_window(
    fd: i32,
    bytes: &[u8],
    offset: usize,
    length: usize,
    use_pwrite: bool,
    position: i64,
) -> i64 {
    let ptr = unsafe { bytes.as_ptr().add(offset) } as *const ::std::ffi::c_void;
    // SAFETY: `ptr` points inside `bytes` for `length` bytes; no JS runs
    // during the syscall, so the buffer cannot move.
    unsafe {
        if use_pwrite {
            libc::pwrite(fd, ptr, length, position) as i64
        } else {
            libc::write(fd, ptr, length) as i64
        }
    }
}

/// Windows: CRT fd write. The positional form is seek-free on the caller's
/// side via `_lseek` save/restore (same shape as readSync's pread shim).
#[cfg(windows)]
fn write_window(
    fd: i32,
    bytes: &[u8],
    offset: usize,
    length: usize,
    use_pwrite: bool,
    position: i64,
) -> i64 {
    unsafe extern "C" {
        fn _lseek(fd: i32, off: i64, origin: i32) -> i64;
    }
    const SEEK_SET: i32 = 0;
    const SEEK_CUR: i32 = 1;
    let ptr = unsafe { bytes.as_ptr().add(offset) };
    // SAFETY: `ptr` points inside `bytes` for `length` bytes.
    unsafe {
        if use_pwrite {
            let saved = _lseek(fd, 0, SEEK_CUR);
            _lseek(fd, position, SEEK_SET);
            let n = win_fd::_write(fd, ptr as *const _, length as u32) as i64;
            _lseek(fd, saved, SEEK_SET);
            n
        } else {
            win_fd::_write(fd, ptr as *const _, length as u32) as i64
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_write_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 {
        *args.get(0).ptr
    } else {
        UndefinedValue()
    };
    let fd = if fd_val.is_int32() {
        fd_val.to_int32()
    } else {
        -1
    };
    let (bytes, offset, length, use_pwrite, position) =
        match resolve_write_window(cx, &args, "writeSync") {
            ::std::result::Result::Ok(w) => w,
            ::std::result::Result::Err(()) => return false,
        };
    let written = write_window(fd, &bytes, offset, length, use_pwrite, position);
    if written >= 0 {
        args.rval().set(mozjs::jsval::DoubleValue(written as f64));
        true
    } else {
        throw_fs_error(
            cx,
            "writeSync",
            &format!("fd:{}", fd),
            &::std::io::Error::last_os_error(),
        )
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_mkdtemp_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let prefix = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    match mkdtemp_inner(&prefix) {
        ::std::result::Result::Ok(dir) => {
            let c_str = ZBox::from_bytes(dir.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() {
                args.rval().set(UndefinedValue());
            } else {
                args.rval().set(mozjs::jsval::StringValue(&*js_str));
            }
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "mkdtempSync", &prefix, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_fchmod_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 {
        *args.get(0).ptr
    } else {
        UndefinedValue()
    };
    let fd = if fd_val.is_int32() {
        fd_val.to_int32()
    } else {
        -1
    };
    let mode_val = if argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };
    let mode = if mode_val.is_int32() {
        mode_val.to_int32() as u32
    } else {
        0o644
    };
    #[cfg(unix)]
    {
        let rv = unsafe { libc::fchmod(fd, mode) };
        if rv == 0 {
            args.rval().set(UndefinedValue());
            true
        } else {
            throw_fs_error(
                cx,
                "fchmodSync",
                &format!("fd:{}", fd),
                &::std::io::Error::last_os_error(),
            )
        }
    }
    #[cfg(not(unix))]
    {
        args.rval().set(UndefinedValue());
        true
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_fchown_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 {
        *args.get(0).ptr
    } else {
        UndefinedValue()
    };
    let fd = if fd_val.is_int32() {
        fd_val.to_int32()
    } else {
        -1
    };
    let uid = if argc > 1 {
        let v = *args.get(1).ptr;
        if v.is_int32() { v.to_int32() as u32 } else { 0 }
    } else {
        0
    };
    let gid = if argc > 2 {
        let v = *args.get(2).ptr;
        if v.is_int32() { v.to_int32() as u32 } else { 0 }
    } else {
        0
    };
    #[cfg(unix)]
    {
        let rv = unsafe { libc::fchown(fd, uid as libc::uid_t, gid as libc::gid_t) };
        if rv == 0 {
            args.rval().set(UndefinedValue());
            true
        } else {
            throw_fs_error(
                cx,
                "fchownSync",
                &format!("fd:{}", fd),
                &::std::io::Error::last_os_error(),
            )
        }
    }
    #[cfg(not(unix))]
    {
        args.rval().set(UndefinedValue());
        true
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_fdatasync_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 {
        *args.get(0).ptr
    } else {
        UndefinedValue()
    };
    let fd = if fd_val.is_int32() {
        fd_val.to_int32()
    } else {
        -1
    };
    #[cfg(unix)]
    {
        let rv = unsafe { libc::fdatasync(fd) };
        if rv == 0 {
            args.rval().set(UndefinedValue());
            true
        } else {
            throw_fs_error(
                cx,
                "fdatasyncSync",
                &format!("fd:{}", fd),
                &::std::io::Error::last_os_error(),
            )
        }
    }
    #[cfg(not(unix))]
    {
        args.rval().set(UndefinedValue());
        true
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_fsync_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    fs_fd_void_sync_op(cx, argc, vp, "fsync")
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_ftruncate_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 {
        *args.get(0).ptr
    } else {
        UndefinedValue()
    };
    let fd = if fd_val.is_int32() {
        fd_val.to_int32()
    } else {
        -1
    };
    let len_val = if argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };
    let len = truncate_len_from_val(len_val);
    #[cfg(unix)]
    {
        let rv = unsafe { libc::ftruncate(fd, len) };
        if rv == 0 {
            args.rval().set(UndefinedValue());
            true
        } else {
            throw_fs_error(
                cx,
                "ftruncateSync",
                &format!("fd:{}", fd),
                &::std::io::Error::last_os_error(),
            )
        }
    }
    #[cfg(windows)]
    {
        // SAFETY: plain CRT fd resize (bad fd → -1 + errno, never UB).
        let rv = unsafe { win_fd::_chsize(fd, len as i64) };
        if rv == 0 {
            args.rval().set(UndefinedValue());
            true
        } else {
            throw_fs_error(
                cx,
                "ftruncateSync",
                &format!("fd:{}", fd),
                &::std::io::Error::last_os_error(),
            )
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_futimes_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 {
        *args.get(0).ptr
    } else {
        UndefinedValue()
    };
    let fd = if fd_val.is_int32() {
        fd_val.to_int32()
    } else {
        -1
    };
    let atime_val = if argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };
    let mtime_val = if argc > 2 {
        *args.get(2).ptr
    } else {
        UndefinedValue()
    };
    let atime = if atime_val.is_double() {
        atime_val.to_double()
    } else if atime_val.is_int32() {
        atime_val.to_int32() as f64
    } else {
        0.0
    };
    let mtime = if mtime_val.is_double() {
        mtime_val.to_double()
    } else if mtime_val.is_int32() {
        mtime_val.to_int32() as f64
    } else {
        0.0
    };
    #[cfg(unix)]
    {
        let tv = [
            libc::timeval {
                tv_sec: atime as i64,
                tv_usec: ((atime % 1.0) * 1_000_000.0) as i64,
            },
            libc::timeval {
                tv_sec: mtime as i64,
                tv_usec: ((mtime % 1.0) * 1_000_000.0) as i64,
            },
        ];
        let rv = unsafe { libc::futimes(fd, tv.as_ptr()) };
        if rv == 0 {
            args.rval().set(UndefinedValue());
            true
        } else {
            throw_fs_error(
                cx,
                "futimesSync",
                &format!("fd:{}", fd),
                &::std::io::Error::last_os_error(),
            )
        }
    }
    #[cfg(not(unix))]
    {
        args.rval().set(UndefinedValue());
        true
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_lchmod_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let mode_val = if argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };
    let mode = if mode_val.is_int32() {
        mode_val.to_int32() as u32
    } else {
        0o644
    };
    // lchmod is not available on Linux; fall back to chmod
    #[cfg(unix)]
    {
        use ::std::os::unix::fs::PermissionsExt;
        match fs::set_permissions(&path, fs::Permissions::from_mode(mode)) {
            ::std::result::Result::Ok(()) => {
                args.rval().set(UndefinedValue());
                true
            }
            ::std::result::Result::Err(e) => throw_fs_error(cx, "lchmodSync", &path, &e),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
        let _ = path;
        args.rval().set(UndefinedValue());
        true
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_lchown_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let uid = if argc > 1 {
        let v = *args.get(1).ptr;
        if v.is_int32() { v.to_int32() as u32 } else { 0 }
    } else {
        0
    };
    let gid = if argc > 2 {
        let v = *args.get(2).ptr;
        if v.is_int32() { v.to_int32() as u32 } else { 0 }
    } else {
        0
    };
    #[cfg(unix)]
    {
        let c_path = ::std::ffi::CString::new(path.as_str()).unwrap_or_default();
        let rv = unsafe { libc::lchown(c_path.as_ptr(), uid, gid) };
        if rv == 0 {
            args.rval().set(UndefinedValue());
            true
        } else {
            throw_fs_error(cx, "lchownSync", &path, &::std::io::Error::last_os_error())
        }
    }
    #[cfg(not(unix))]
    {
        args.rval().set(UndefinedValue());
        true
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_readv_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 {
        *args.get(0).ptr
    } else {
        UndefinedValue()
    };
    let fd = if fd_val.is_int32() {
        fd_val.to_int32()
    } else {
        -1
    };
    let buffers_val = if argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };
    if !buffers_val.is_object() {
        JS_ReportErrorUTF8(cx, c"readvSync: buffers must be an array".as_ptr());
        return false;
    }
    // Node: readvSync(fd, buffers[, position]) — fills each caller Buffer
    // sequentially from the fd (position numeric → pread at that absolute
    // offset; null/undefined → current cursor). C1 #3: bao read into an
    // internal temp copy and left the caller's Buffers zero-filled.
    let pos_arg = if argc > 2 {
        *args.get(2).ptr
    } else {
        UndefinedValue()
    };
    let pos = fs_number_arg(pos_arg);
    let (use_pread, position) = (pos.is_some(), pos.unwrap_or(-1.0) as i64);

    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let buffers_obj = buffers_val.to_object());

    #[cfg(unix)]
    {
        // iovec entries point straight at the caller's typed-array storage:
        // no JS runs between acquisition and the syscall, so the buffers
        // cannot be moved by the GC.
        let mut iov: Vec<libc::iovec> = Vec::new();
        let mut buf_idx: u32 = 0;
        loop {
            let mut elem = UndefinedValue();
            JS_GetElement(
                cx,
                buffers_obj.handle().into(),
                buf_idx,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut elem,
                },
            );
            if elem.is_undefined() {
                break;
            }
            if !elem.is_object() {
                JS_ReportErrorUTF8(
                    cx,
                    c"readvSync: buffers must be an array of Buffers".as_ptr(),
                );
                return false;
            }
            let view = elem.to_object();
            let len = mozjs_sys::jsapi::JS_GetTypedArrayByteLength(view) as usize;
            let mut is_shared = false;
            let data =
                mozjs_sys::jsapi::JS_GetUint8ArrayData(view, &mut is_shared, ::std::ptr::null());
            if data.is_null() {
                JS_ReportErrorUTF8(
                    cx,
                    c"readvSync: buffers must be an array of Buffers".as_ptr(),
                );
                return false;
            }
            if len > 0 {
                iov.push(libc::iovec {
                    iov_base: data as *mut ::std::ffi::c_void,
                    iov_len: len,
                });
            }
            buf_idx += 1;
        }

        let total_read: i64 = if iov.is_empty() {
            0
        } else if use_pread {
            // Sequential pread preserves readv's contract for regular files
            // (fill in order; a short read means EOF, stop).
            let mut total: i64 = 0;
            for v in &iov {
                let n =
                    unsafe { libc::pread(fd, v.iov_base, v.iov_len, position + total) } as i64;
                if n < 0 {
                    return throw_fs_error(
                        cx,
                        "readvSync",
                        &format!("fd:{}", fd),
                        &::std::io::Error::last_os_error(),
                    );
                }
                total += n;
                if (n as usize) < v.iov_len {
                    break;
                }
            }
            total
        } else {
            unsafe { libc::readv(fd, iov.as_ptr(), iov.len() as i32) as i64 }
        };
        if total_read < 0 {
            return throw_fs_error(
                cx,
                "readvSync",
                &format!("fd:{}", fd),
                &::std::io::Error::last_os_error(),
            );
        }
        args.rval().set(mozjs::jsval::Int32Value(total_read as i32));
        true
    }
    #[cfg(not(unix))]
    {
        // Windows: per-buffer CRT read; the positional form saves/restores
        // the cursor (same shape as readSync's pread shim).
        unsafe extern "C" {
            fn _lseek(fd: i32, off: i64, origin: i32) -> i64;
        }
        const SEEK_SET: i32 = 0;
        const SEEK_CUR: i32 = 1;
        let mut total_read: i32 = 0;
        let mut cursor: i64 = position;
        let mut buf_idx: u32 = 0;
        loop {
            let mut elem = UndefinedValue();
            JS_GetElement(
                cx,
                buffers_obj.handle().into(),
                buf_idx,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut elem,
                },
            );
            if elem.is_undefined() {
                break;
            }
            if elem.is_object() {
                let view = elem.to_object();
                let len = mozjs_sys::jsapi::JS_GetTypedArrayByteLength(view) as usize;
                let mut is_shared = false;
                let data = mozjs_sys::jsapi::JS_GetUint8ArrayData(
                    view,
                    &mut is_shared,
                    ::std::ptr::null(),
                );
                if len > 0 && !data.is_null() {
                    let n = if use_pread {
                        let saved = _lseek(fd, 0, SEEK_CUR);
                        _lseek(fd, cursor, SEEK_SET);
                        let n = win_fd::_read(fd, data as *mut _, len as u32);
                        _lseek(fd, saved, SEEK_SET);
                        n
                    } else {
                        win_fd::_read(fd, data as *mut _, len as u32)
                    };
                    if n < 0 {
                        return throw_fs_error(
                            cx,
                            "readvSync",
                            &format!("fd:{}", fd),
                            &::std::io::Error::last_os_error(),
                        );
                    }
                    total_read += n;
                    cursor += n as i64;
                    if (n as usize) < len {
                        break;
                    }
                }
            }
            buf_idx += 1;
        }
        args.rval().set(mozjs::jsval::Int32Value(total_read));
        true
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_writev_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 {
        *args.get(0).ptr
    } else {
        UndefinedValue()
    };
    let fd = if fd_val.is_int32() {
        fd_val.to_int32()
    } else {
        -1
    };
    let buffers_val = if argc > 1 {
        *args.get(1).ptr
    } else {
        UndefinedValue()
    };
    let bytes = match collect_writev_bytes(cx, buffers_val, "writevSync") {
        ::std::result::Result::Ok(b) => b,
        ::std::result::Result::Err(()) => return false,
    };
    // Node: writevSync(fd, buffers[, position]) — position numeric writes at
    // that absolute offset; null/undefined uses the fd cursor.
    let pos_arg = if argc > 2 {
        *args.get(2).ptr
    } else {
        UndefinedValue()
    };
    let pos = fs_number_arg(pos_arg);
    let (use_pwrite, position) = (pos.is_some(), pos.unwrap_or(-1.0) as i64);
    let written = write_window(fd, &bytes, 0, bytes.len(), use_pwrite, position);
    if written >= 0 {
        args.rval().set(mozjs::jsval::DoubleValue(written as f64));
        true
    } else {
        throw_fs_error(
            cx,
            "writevSync",
            &format!("fd:{}", fd),
            &::std::io::Error::last_os_error(),
        )
    }
}

// --- glob ---
//
// BCE-20260816-FS-GLOB — the hand-written glob_walk/glob_match walker had two
// fatal shape defects: (1) `options.cwd` was ignored entirely (patterns were
// always walked from process CWD — probe: globSync('**/*.ts', {cwd:'/tmp/x'})
// returned project-tree hits); (2) the matcher was a naive recursive glob
// with no brace/char-class/dotfile semantics and no `dot` switch. Replaced by
// the workspace's bun_glob engine (the Bun-faithful GlobWalker powering
// upstream fs.glob — see ~/code/rust/bun/src/js/internal/fs/glob.ts):
//   pattern: string | string[]
//   options: { cwd (default process cwd), root (fallback start dir),
//              dot (default false), exclude (fn | string[]) }
// Node/Bun fs.glob yield paths RELATIVE to the start dir; absolute patterns
// yield absolute paths. onlyFiles defaults to false (dirs match too), matching
// upstream mapOptions.

/// Options parsed from the JS `options` argument of glob/globSync.
struct GlobOptions {
    start_dir: String,
    dot: bool,
    /// JS exclude callbacks are applied post-walk (IgnoreFilterFn is a plain
    /// fn pointer and cannot close over a JS callable).
    exclude_fn: Option<*mut JSObject>,
    exclude_globs: Vec<String>,
}

/// Collect glob matches for one pattern via bun_glob::GlobWalker, applying
/// JS/glob excludes and the relative-path yield contract.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn glob_collect(
    cx: *mut JSContext,
    pattern: &str,
    opts: &GlobOptions,
    results: &mut Vec<String>,
) {
    type Walker = bun_glob::GlobWalker<bun_glob::walk::SyscallAccessor, false>;
    // Windows absolute patterns carry MAIN_SEPARATOR; the glob language
    // treats '\' as an escape — normalize to '/' (lossless: Windows
    // filenames cannot contain backslashes). Mirrors bun_glob_api.
    #[cfg(windows)]
    let pattern_owned;
    #[cfg(windows)]
    let pattern = if crate::bun_glob_api::glob_pattern_is_absolute(pattern) {
        pattern_owned = pattern.replace('\\', "/");
        &pattern_owned
    } else {
        pattern
    };
    let absolute = crate::bun_glob_api::glob_pattern_is_absolute(pattern);
    let mut walker = match Walker::init_with_cwd(
        pattern.as_bytes(),
        opts.start_dir.as_bytes(),
        opts.dot,
        absolute,
        // followSymlinks: true — upstream fs.glob pins this (mapOptions).
        true,
        false,
        // onlyFiles: false — upstream pins dirs+files (mapOptions).
        false,
        None,
    ) {
        Ok(Ok(w)) => w,
        // Malformed pattern (unbalanced brace/class) → no matches for it.
        _ => return,
    };
    let mut iter = bun_glob::walk::Iterator::new(&mut walker);
    if iter.init().is_err() {
        return;
    }
    let prefix = format!("{}/", opts.start_dir.trim_end_matches('/'));
    loop {
        match iter.next() {
            Ok(Ok(Some(path))) => {
                let full = String::from_utf8_lossy(&path).into_owned();
                let shown = if !absolute && full.starts_with(&prefix) {
                    full[prefix.len()..].to_string()
                } else {
                    full
                };
                if glob_excluded(cx, &shown, opts) {
                    continue;
                }
                results.push(shown);
            }
            _ => break,
        }
    }
}

/// Apply options.exclude: a JS predicate (path => boolean) or a list of glob
/// patterns (path is excluded when any pattern matches).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn glob_excluded(cx: *mut JSContext, path: &str, opts: &GlobOptions) -> bool {
    if let Some(cb) = opts.exclude_fn {
        if !cb.is_null() {
            let wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
            rooted!(&in(wrapped_cx) let cb_root = cb);
            let path_c = ZBox::from_bytes(path.as_bytes());
            let path_js = JS_NewStringCopyZ(cx, path_c.as_ptr());
            if path_js.is_null() {
                return false;
            }
            let arg = mozjs::jsval::StringValue(&*path_js);
            let args_arr = [arg];
            let call_args = HandleValueArray {
                length_: 1,
                elements_: args_arr.as_ptr(),
            };
            let global = CurrentGlobalOrNull(cx);
            if global.is_null() {
                return false;
            }
            rooted!(&in(wrapped_cx) let global_root = global);
            rooted!(&in(wrapped_cx) let cb_val = mozjs::jsval::ObjectValue(cb_root.get()));
            let mut rval = UndefinedValue();
            let ok = JS_CallFunctionValue(
                cx,
                global_root.handle().into(),
                cb_val.handle().into(),
                &call_args,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut rval,
                },
            );
            return ok && rval.is_boolean() && rval.to_boolean();
        }
    }
    for pat in &opts.exclude_globs {
        if let bun_glob::MatchResult::Match = bun_glob::r#match(pat.as_bytes(), path.as_bytes()) {
            return true;
        }
    }
    false
}

/// Read a string-valued property off a JS options object ("" when absent).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn glob_opt_string(cx: *mut JSContext, obj: *mut JSObject, name: &[u8]) -> Option<String> {
    let name_z = ZBox::from_bytes(name);
    let wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj_root = obj);
    let mut val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_root.handle().into(),
        name_z.as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut val,
        },
    );
    if val.is_string() {
        let s = crate::jsstr_to_rust_string(cx, val.to_string());
        if !s.is_empty() {
            return Some(s);
        }
    }
    None
}

/// Read a boolean-valued property off a JS options object.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn glob_opt_bool(cx: *mut JSContext, obj: *mut JSObject, name: &[u8]) -> bool {
    let name_z = ZBox::from_bytes(name);
    let wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj_root = obj);
    let mut val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_root.handle().into(),
        name_z.as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut val,
        },
    );
    val.is_boolean() && val.to_boolean()
}

/// Parse the `options` argument shared by glob/globSync into GlobOptions.
/// `arg_index` is where the options object may sit (1 for globSync, 1 or 2
/// for glob depending on callback arity).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn glob_parse_options(cx: *mut JSContext, args: &CallArgs, arg_index: u32) -> GlobOptions {
    let mut opts = GlobOptions {
        start_dir: ::std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
        dot: false,
        exclude_fn: None,
        exclude_globs: Vec::new(),
    };
    let val = *args.get(arg_index).ptr;
    if !val.is_object() {
        return opts;
    }
    let obj = val.to_object();
    // Node fs.glob: cwd takes precedence, root is the legacy fallback.
    if let Some(cwd) = glob_opt_string(cx, obj, b"cwd") {
        opts.start_dir = cwd;
    } else if let Some(root) = glob_opt_string(cx, obj, b"root") {
        opts.start_dir = root;
    }
    opts.dot = glob_opt_bool(cx, obj, b"dot");

    let excl_z = ZBox::from_bytes(b"exclude");
    let wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj_root = obj);
    let mut excl = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_root.handle().into(),
        excl_z.as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut excl,
        },
    );
    if excl.is_object() {
        let excl_obj = excl.to_object();
        if mozjs_sys::jsapi::JS::IsCallable(excl_obj) {
            opts.exclude_fn = Some(excl_obj);
        } else {
            // Array of glob patterns.
            let mut len_val = UndefinedValue();
            JS_GetProperty(
                cx,
                obj_root.handle().into(),
                c"length".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut len_val,
                },
            );
            if len_val.is_int32() {
                for i in 0..len_val.to_int32().max(0) as u32 {
                    let mut elem = UndefinedValue();
                    if JS_GetElement(
                        cx,
                        obj_root.handle().into(),
                        i,
                        MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &mut elem,
                        },
                    ) && elem.is_string()
                    {
                        opts.exclude_globs.push(crate::jsstr_to_rust_string(cx, elem.to_string()));
                    }
                }
            }
        }
    }
    opts
}

/// Expand the pattern argument: a single string or an array of strings.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn glob_patterns(cx: *mut JSContext, args: &CallArgs) -> Vec<String> {
    let val = *args.get(0).ptr;
    let mut patterns = Vec::new();
    if val.is_string() {
        patterns.push(crate::jsstr_to_rust_string(cx, val.to_string()));
    } else if val.is_object() {
        let wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        rooted!(&in(wrapped_cx) let arr = val.to_object());
        let mut len_val = UndefinedValue();
        JS_GetProperty(
            cx,
            arr.handle().into(),
            c"length".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut len_val,
            },
        );
        if len_val.is_int32() {
            for i in 0..len_val.to_int32().max(0) as u32 {
                let mut elem = UndefinedValue();
                if JS_GetElement(
                    cx,
                    arr.handle().into(),
                    i,
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut elem,
                    },
                ) && elem.is_string()
                {
                    patterns.push(crate::jsstr_to_rust_string(cx, elem.to_string()));
                }
            }
        }
    }
    patterns
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_glob_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let patterns = glob_patterns(cx, &args);
    let opts = glob_parse_options(cx, &args, 1);
    let mut results: Vec<String> = Vec::new();
    for pat in &patterns {
        glob_collect(cx, pat, &opts, &mut results);
    }
    normalize_glob_result_separators(&mut results);
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let arr = w2::NewArrayObject1(cx_ref, results.len()));
    if arr.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    for (idx, path) in results.iter().enumerate() {
        let c_path = ZBox::from_bytes(path.as_bytes());
        let js_str = JS_NewStringCopyZ(cx, c_path.as_ptr());
        if !js_str.is_null() {
            rooted!(&in(cx_ref) let val = mozjs::jsval::StringValue(&*js_str));
            JS_DefineElement(
                cx,
                arr.handle().into(),
                idx as u32,
                val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(arr.get()));
    true
}

/// Glob results are '/'-shaped on every platform (node fs.glob / Bun's
/// GlobWalker semantics — the pattern language is posix-shaped, so matches
/// report forward slashes even on win32). The engine's platform join emits
/// MAIN_SEPARATOR on Windows; normalize at the emission boundary. Safe as a
/// blanket replace: a Windows filename cannot contain a backslash, so every
/// '\' in a result IS a separator.
fn normalize_glob_result_separators(results: &mut [String]) {
    #[cfg(windows)]
    for r in results.iter_mut() {
        *r = r.replace('\\', "/");
    }
    #[cfg(not(windows))]
    let _ = results;
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_glob(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let patterns = glob_patterns(cx, &args);

    // Node fs.glob(pattern, options, callback): options may sit at index 1
    // (callback at 2) or be skipped (callback at 1).
    let opts_idx = if argc >= 3 { 1 } else { 1 };
    let opts = glob_parse_options(cx, &args, opts_idx);

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 2) {
        // The JS exclude predicate cannot cross the thread boundary — apply
        // it synchronously on the collected results instead of in the worker.
        let mut prefiltered: Vec<String> = Vec::new();
        for pat in &patterns {
            glob_collect(cx, pat, &opts, &mut prefiltered);
        }
        normalize_glob_result_separators(&mut prefiltered);
        spawn_fs_async(cx, "glob", patterns.join(","), callback, None, move || {
            Ok(FsAsyncResult::OkDirnames(prefiltered))
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    // No callback — behave like sync
    let mut results: Vec<String> = Vec::new();
    for pat in &patterns {
        glob_collect(cx, pat, &opts, &mut results);
    }
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let arr = w2::NewArrayObject1(cx_ref, results.len()));
    if arr.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    for (idx, path) in results.iter().enumerate() {
        let c_path = ZBox::from_bytes(path.as_bytes());
        let js_str = JS_NewStringCopyZ(cx, c_path.as_ptr());
        if !js_str.is_null() {
            rooted!(&in(cx_ref) let val = mozjs::jsval::StringValue(&*js_str));
            JS_DefineElement(
                cx,
                arr.handle().into(),
                idx as u32,
                val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(arr.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_open_as_blob(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }

    match bun_fs::read(&path) {
        ::std::result::Result::Ok(data) => {
            let buf_obj = crate::globals::create_buffer_object(cx, &data);
            if !buf_obj.is_null() {
                // Create a Blob-like object with arrayBuffer() and size
                rooted!(&in(cx_ref) let blob_obj = JS_NewPlainObject(cx));
                if !blob_obj.get().is_null() {
                    define_num_prop(cx, blob_obj.get(), "size", data.len() as f64);
                    rooted!(&in(cx_ref) let buf_val = mozjs::jsval::ObjectValue(buf_obj));
                    JS_DefineProperty(
                        cx,
                        blob_obj.handle().into(),
                        c"_buffer".as_ptr(),
                        buf_val.handle().into(),
                        0,
                    );
                    rooted!(&in(cx_ref) let val = mozjs::jsval::ObjectValue(blob_obj.get()));
                    unsafe {
                        mozjs_sys::jsapi::JS::ResolvePromise(
                            cx,
                            promise.handle().into(),
                            val.handle().into(),
                        );
                    }
                } else {
                    resolve_undefined(cx, promise.get());
                }
            } else {
                resolve_undefined(cx, promise.get());
            }
        }
        ::std::result::Result::Err(e) => {
            reject_fs_error(cx, promise.get(), "openAsBlob", &path, &e)
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

// --- Helper functions ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn get_bool_option(cx: *mut JSContext, args: &CallArgs, opt_index: u32, key: &str) -> bool {
    if args.argc_ <= opt_index {
        return false;
    }
    let opt_val = *args.get(opt_index).ptr;
    if !opt_val.is_object() {
        return false;
    }
    let mut wrapped_cx_opt =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref_opt = &mut wrapped_cx_opt;
    rooted!(&in(cx_ref_opt) let obj = opt_val.to_object());
    let c_key = ZBox::from_bytes(key.as_bytes());
    let mut val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj.handle().into(),
        c_key.as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut val,
        },
    );
    val.is_boolean() && val.to_boolean()
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn string_or_buffer(
    cx: *mut JSContext,
    data: &[u8],
    encoding: ::std::option::Option<&str>,
) -> JSVal {
    match encoding {
        Some("utf-8" | "utf8" | "text") | None => {
            // @trace REQ-ENG-005 — same mojibake class as return_string_content:
            // multibyte UTF-8 must go through JS_NewStringCopyUTF8N, not CopyZ.
            let s = ::std::string::String::from_utf8_lossy(data);
            let js_str = js_string_from_utf8(cx, &s);
            if js_str.is_null() {
                UndefinedValue()
            } else {
                mozjs::jsval::StringValue(&*js_str)
            }
        }
        Some("hex") => {
            let hex: ::std::string::String = bun_core::fmt::bytes_to_hex_lower_string(data);
            let c_str = ZBox::from_bytes(hex.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() {
                UndefinedValue()
            } else {
                mozjs::jsval::StringValue(&*js_str)
            }
        }
        Some("base64") => {
            // @trace REQ-ENG-005 [algorithm:base64]
            // SIMD-accelerated base64 encode via workspace bun_base64 (replaces crates.io base64).
            let encoded_bytes = bun_base64::encode_alloc(data);
            let encoded = ::std::str::from_utf8(&encoded_bytes).unwrap_or("");
            let c_str = ZBox::from_bytes(encoded.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c_str.as_ptr());
            if js_str.is_null() {
                UndefinedValue()
            } else {
                mozjs::jsval::StringValue(&*js_str)
            }
        }
        _ => UndefinedValue(),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
/// Convert `std::fs::Metadata` to `PosixStat` for use with `create_stats_object`.
/// Used as a bridge for `symlink_metadata` (lstat) which has no bun_sys equivalent yet.
#[cfg(unix)]
fn metadata_to_posix_stat(meta: &fs::Metadata) -> bun_sys::PosixStat {
    use ::std::os::unix::fs::MetadataExt;
    bun_sys::PosixStat {
        dev: meta.dev() as u64,
        ino: meta.ino() as u64,
        mode: meta.mode() as u64,
        nlink: meta.nlink() as u64,
        uid: meta.uid() as u64,
        gid: meta.gid() as u64,
        rdev: meta.rdev() as u64,
        size: meta.size(),
        blksize: meta.blksize() as u64,
        blocks: meta.blocks() as u64,
        atim: bun_sys::Timespec {
            sec: meta.atime(),
            nsec: meta.atime_nsec() as i64,
        },
        mtim: bun_sys::Timespec {
            sec: meta.mtime(),
            nsec: meta.mtime_nsec() as i64,
        },
        ctim: bun_sys::Timespec {
            sec: meta.ctime(),
            nsec: meta.ctime_nsec() as i64,
        },
        birthtim: bun_sys::Timespec { sec: 0, nsec: 0 },
    }
}

#[cfg(not(unix))]
fn metadata_to_posix_stat(meta: &fs::Metadata) -> bun_sys::PosixStat {
    bun_sys::PosixStat {
        dev: 0,
        ino: 0,
        mode: 0,
        nlink: 0,
        uid: 0,
        gid: 0,
        rdev: 0,
        size: meta.len(),
        blksize: 0,
        blocks: 0,
        atim: bun_sys::Timespec { sec: 0, nsec: 0 },
        mtim: bun_sys::Timespec { sec: 0, nsec: 0 },
        ctim: bun_sys::Timespec { sec: 0, nsec: 0 },
        birthtim: bun_sys::Timespec { sec: 0, nsec: 0 },
    }
}

/// Convert a raw `libc::stat` (from fstat/lstat syscalls) to `PosixStat`.
/// This is used by fd-based operations (fstat) that cannot go through
/// `std::fs::Metadata` since there is no `File` opened via Rust.
#[cfg(unix)]
fn posix_stat_from_libc(s: &libc::stat) -> bun_sys::PosixStat {
    bun_sys::PosixStat {
        dev: s.st_dev as u64,
        ino: s.st_ino as u64,
        mode: s.st_mode as u64,
        nlink: s.st_nlink as u64,
        uid: s.st_uid as u64,
        gid: s.st_gid as u64,
        rdev: s.st_rdev as u64,
        size: s.st_size as u64,
        blksize: s.st_blksize as u64,
        blocks: s.st_blocks as u64,
        atim: bun_sys::Timespec {
            sec: s.st_atime,
            nsec: s.st_atime_nsec as i64,
        },
        mtim: bun_sys::Timespec {
            sec: s.st_mtime,
            nsec: s.st_mtime_nsec as i64,
        },
        ctim: bun_sys::Timespec {
            sec: s.st_ctime,
            nsec: s.st_ctime_nsec as i64,
        },
        birthtim: bun_sys::Timespec { sec: 0, nsec: 0 },
    }
}

#[cfg(not(unix))]
fn posix_stat_from_libc(_s: &libc::stat) -> bun_sys::PosixStat {
    bun_sys::PosixStat {
        dev: 0,
        ino: 0,
        mode: 0,
        nlink: 0,
        uid: 0,
        gid: 0,
        rdev: 0,
        size: 0,
        blksize: 0,
        blocks: 0,
        atim: bun_sys::Timespec { sec: 0, nsec: 0 },
        mtim: bun_sys::Timespec { sec: 0, nsec: 0 },
        ctim: bun_sys::Timespec { sec: 0, nsec: 0 },
        birthtim: bun_sys::Timespec { sec: 0, nsec: 0 },
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn create_stats_object(cx: *mut JSContext, meta: &bun_fs::PosixStat) -> *mut JSObject {
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let stats = JS_NewPlainObject(cx));
    if stats.get().is_null() {
        return stats.get();
    }

    // Determine file type from mode (S_IFMT bits)
    #[cfg(windows)]
    let mode_type = (meta.mode as u32) & (S_IFMT as u32);
    #[cfg(not(windows))]
    let mode_type = (meta.mode as u32) & libc::S_IFMT;
    #[cfg(windows)]
    let (is_file, is_dir, is_symlink, is_block_device, is_character_device, is_fifo, is_socket) = (
        mode_type == S_IFREG as u32,
        mode_type == S_IFDIR as u32,
        mode_type == S_IFLNK as u32,
        mode_type == S_IFBLK as u32,
        mode_type == S_IFCHR as u32,
        mode_type == S_IFIFO as u32,
        mode_type == S_IFSOCK as u32,
    );
    #[cfg(not(windows))]
    let (is_file, is_dir, is_symlink, is_block_device, is_character_device, is_fifo, is_socket) = (
        mode_type == libc::S_IFREG,
        mode_type == libc::S_IFDIR,
        mode_type == libc::S_IFLNK,
        mode_type == libc::S_IFBLK,
        mode_type == libc::S_IFCHR,
        mode_type == libc::S_IFIFO,
        mode_type == libc::S_IFSOCK,
    );

    let atime_ms = meta.atim.sec as f64 * 1000.0 + meta.atim.nsec as f64 / 1_000_000.0;
    let mtime_ms = meta.mtim.sec as f64 * 1000.0 + meta.mtim.nsec as f64 / 1_000_000.0;
    let ctime_ms = meta.ctim.sec as f64 * 1000.0 + meta.ctim.nsec as f64 / 1_000_000.0;
    // On Linux, birthtime is often not available; fall back to ctime
    let birthtime_ms = if meta.birthtim.sec == 0 && meta.birthtim.nsec == 0 {
        ctime_ms
    } else {
        meta.birthtim.sec as f64 * 1000.0 + meta.birthtim.nsec as f64 / 1_000_000.0
    };

    define_num_prop(cx, stats.get(), "size", meta.size as f64);
    define_num_prop(cx, stats.get(), "dev", meta.dev as f64);
    define_num_prop(cx, stats.get(), "ino", meta.ino as f64);
    define_num_prop(cx, stats.get(), "mode", meta.mode as f64);
    define_num_prop(cx, stats.get(), "nlink", meta.nlink as f64);
    define_num_prop(cx, stats.get(), "uid", meta.uid as f64);
    define_num_prop(cx, stats.get(), "gid", meta.gid as f64);
    define_num_prop(cx, stats.get(), "rdev", meta.rdev as f64);
    define_num_prop(cx, stats.get(), "blksize", meta.blksize as f64);
    define_num_prop(cx, stats.get(), "blocks", meta.blocks as f64);
    define_num_prop(cx, stats.get(), "atimeMs", atime_ms);
    define_num_prop(cx, stats.get(), "mtimeMs", mtime_ms);
    define_num_prop(cx, stats.get(), "ctimeMs", ctime_ms);
    define_num_prop(cx, stats.get(), "birthtimeMs", birthtime_ms);

    // Date objects for atime, mtime, ctime, birthtime
    let date_props: [(&str, f64); 4] = [
        ("atime", atime_ms),
        ("mtime", mtime_ms),
        ("ctime", ctime_ms),
        ("birthtime", birthtime_ms),
    ];
    for (prop, ms) in &date_props {
        let date_obj = w2::NewDateObject(cx_ref, mozjs::jsapi::ClippedTime { t: *ms });
        if !date_obj.is_null() {
            rooted!(&in(cx_ref) let date_val = mozjs::jsval::ObjectValue(date_obj));
            let c_prop = ZBox::from_bytes(prop.as_bytes());
            JS_DefineProperty(
                cx,
                stats.handle().into(),
                c_prop.as_ptr(),
                date_val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }

    // Store boolean values as hidden properties for method callbacks
    define_bool_prop(cx, stats.get(), "_isFile", is_file);
    define_bool_prop(cx, stats.get(), "_isDirectory", is_dir);
    define_bool_prop(cx, stats.get(), "_isSymbolicLink", is_symlink);
    define_bool_prop(cx, stats.get(), "_isBlockDevice", is_block_device);
    define_bool_prop(cx, stats.get(), "_isCharacterDevice", is_character_device);
    define_bool_prop(cx, stats.get(), "_isFIFO", is_fifo);
    define_bool_prop(cx, stats.get(), "_isSocket", is_socket);

    // Node.js Stats methods
    w2::JS_DefineFunction(
        cx_ref,
        stats.handle().into(),
        c"isFile".as_ptr(),
        Some(stats_is_file),
        0,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        stats.handle().into(),
        c"isDirectory".as_ptr(),
        Some(stats_is_directory),
        0,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        stats.handle().into(),
        c"isSymbolicLink".as_ptr(),
        Some(stats_is_symlink),
        0,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        stats.handle().into(),
        c"isBlockDevice".as_ptr(),
        Some(stats_is_block_device),
        0,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        stats.handle().into(),
        c"isCharacterDevice".as_ptr(),
        Some(stats_is_character_device),
        0,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        stats.handle().into(),
        c"isFIFO".as_ptr(),
        Some(stats_is_fifo),
        0,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        stats.handle().into(),
        c"isSocket".as_ptr(),
        Some(stats_is_socket),
        0,
        JSPROP_ENUMERATE as u32,
    );

    stats.get()
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn create_dirent(cx: *mut JSContext, name: &str, is_dir: bool) -> *mut JSObject {
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let dirent = JS_NewPlainObject(cx));
    if dirent.get().is_null() {
        return dirent.get();
    }
    let c_name = ZBox::from_bytes(name.as_bytes());
    let js_str = JS_NewStringCopyZ(cx, c_name.as_ptr());
    if !js_str.is_null() {
        rooted!(&in(cx_ref) let name_val = mozjs::jsval::StringValue(&*js_str));
        JS_DefineProperty(
            cx,
            dirent.handle().into(),
            c"name".as_ptr(),
            name_val.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }
    // Type code: 0=file, 1=dir, 2=symlink, 3=block, 4=char, 5=fifo, 6=socket
    let type_code: i32 = if is_dir { 1 } else { 0 };
    set_hidden_int(cx, dirent.get(), "_typeCode", type_code);
    define_bool_prop(cx, dirent.get(), "isFile", !is_dir);
    define_bool_prop(cx, dirent.get(), "isDirectory", is_dir);

    w2::JS_DefineFunction(
        cx_ref,
        dirent.handle().into(),
        c"isFile".as_ptr(),
        Some(dirent_is_file),
        0,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        dirent.handle().into(),
        c"isDirectory".as_ptr(),
        Some(dirent_is_directory),
        0,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        dirent.handle().into(),
        c"isSymbolicLink".as_ptr(),
        Some(dirent_is_symbolic_link),
        0,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        dirent.handle().into(),
        c"isBlockDevice".as_ptr(),
        Some(dirent_is_block_device),
        0,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        dirent.handle().into(),
        c"isCharacterDevice".as_ptr(),
        Some(dirent_is_character_device),
        0,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        dirent.handle().into(),
        c"isFIFO".as_ptr(),
        Some(dirent_is_fifo),
        0,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        dirent.handle().into(),
        c"isSocket".as_ptr(),
        Some(dirent_is_socket),
        0,
        JSPROP_ENUMERATE as u32,
    );

    dirent.get()
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn resolve_undefined(cx: *mut JSContext, promise: *mut JSObject) {
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let val = UndefinedValue());
    rooted!(&in(cx_ref) let promise_rooted = promise);
    mozjs_sys::jsapi::JS::ResolvePromise(cx, promise_rooted.handle().into(), val.handle().into());
}

#[allow(unsafe_op_in_unsafe_fn)]
/// @trace REQ-ENG-007 — upstream 83350172b9 port: when a path is too long
/// for any syscall, the promise form REJECTS (never a synchronous throw),
/// and the rejected error is identifiable — `code` (e.g. ENAMETOOLONG) and
/// `path`, mirroring `throw_fs_error`'s sync-throw shape and Node's
/// `cb:ENAMETOOLONG` contract. The message format is byte-identical to the
/// old `reject_with_error` call sites (`{op} '{path}': {err}`), so
/// message-based consumers see no difference; `code`/`path` are additive.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn reject_fs_error(
    cx: *mut JSContext,
    promise: *mut JSObject,
    op: &str,
    path: &str,
    err: &::std::io::Error,
) {
    let msg = format!("{} '{}': {}", op, path, err);
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let err_obj = JS_NewPlainObject(cx));
    if !err_obj.get().is_null() {
        let c_msg = ZBox::from_bytes(msg.as_bytes());
        let js_str = JS_NewStringCopyZ(cx, c_msg.as_ptr());
        if !js_str.is_null() {
            rooted!(&in(cx_ref) let msg_val = mozjs::jsval::StringValue(&*js_str));
            JS_DefineProperty(
                cx,
                err_obj.handle().into(),
                c"message".as_ptr(),
                msg_val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
        let code = io_error_code(err);
        let c_code = ZBox::from_bytes(code.as_bytes());
        let code_str = JS_NewStringCopyZ(cx, c_code.as_ptr());
        if !code_str.is_null() {
            rooted!(&in(cx_ref) let code_val = mozjs::jsval::StringValue(&*code_str));
            JS_DefineProperty(
                cx,
                err_obj.handle().into(),
                c"code".as_ptr(),
                code_val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
        let c_path = ZBox::from_bytes(path.as_bytes());
        let path_str = JS_NewStringCopyZ(cx, c_path.as_ptr());
        if !path_str.is_null() {
            rooted!(&in(cx_ref) let path_val = mozjs::jsval::StringValue(&*path_str));
            JS_DefineProperty(
                cx,
                err_obj.handle().into(),
                c"path".as_ptr(),
                path_val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }
    rooted!(&in(cx_ref) let err_val = mozjs::jsval::ObjectValue(err_obj.get()));
    rooted!(&in(cx_ref) let promise_rooted = promise);
    mozjs_sys::jsapi::JS::RejectPromise(
        cx,
        promise_rooted.handle().into(),
        err_val.handle().into(),
    );
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn reject_with_error(cx: *mut JSContext, promise: *mut JSObject, msg: &str) {
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let err_obj = JS_NewPlainObject(cx));
    if !err_obj.get().is_null() {
        let c_msg = ZBox::from_bytes(msg.as_bytes());
        let js_str = JS_NewStringCopyZ(cx, c_msg.as_ptr());
        if !js_str.is_null() {
            rooted!(&in(cx_ref) let msg_val = mozjs::jsval::StringValue(&*js_str));
            JS_DefineProperty(
                cx,
                err_obj.handle().into(),
                c"message".as_ptr(),
                msg_val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }
    rooted!(&in(cx_ref) let err_val = mozjs::jsval::ObjectValue(err_obj.get()));
    rooted!(&in(cx_ref) let promise_rooted = promise);
    mozjs_sys::jsapi::JS::RejectPromise(
        cx,
        promise_rooted.handle().into(),
        err_val.handle().into(),
    );
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn define_num_prop(cx: *mut JSContext, obj_ptr: *mut JSObject, name: &str, val: f64) {
    let c_name = ZBox::from_bytes(name.as_bytes());
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = obj_ptr);
    let js_val = if val == (val as i32) as f64 && val.abs() < i32::MAX as f64 {
        mozjs::jsval::Int32Value(val as i32)
    } else {
        mozjs::jsval::DoubleValue(val)
    };
    rooted!(&in(cx_ref) let v = js_val);
    JS_DefineProperty(
        cx,
        obj.handle().into(),
        c_name.as_ptr(),
        v.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn define_bool_prop(cx: *mut JSContext, obj_ptr: *mut JSObject, name: &str, val: bool) {
    let c_name = ZBox::from_bytes(name.as_bytes());
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = obj_ptr);
    rooted!(&in(cx_ref) let v = mozjs::jsval::BooleanValue(val));
    JS_DefineProperty(
        cx,
        obj.handle().into(),
        c_name.as_ptr(),
        v.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn set_hidden_int(cx: *mut JSContext, obj: *mut JSObject, prop: &str, val: i32) {
    let c_name = ZBox::from_bytes(prop.as_bytes());
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_rooted = obj);
    rooted!(&in(cx_ref) let v = mozjs::jsval::Int32Value(val));
    JS_DefineProperty(
        cx,
        obj_rooted.handle().into(),
        c_name.as_ptr(),
        v.handle().into(),
        (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
    );
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn set_hidden_bool(cx: *mut JSContext, obj: *mut JSObject, prop: &str, val: bool) {
    let c_name = ZBox::from_bytes(prop.as_bytes());
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_rooted = obj);
    rooted!(&in(cx_ref) let v = mozjs::jsval::BooleanValue(val));
    JS_DefineProperty(
        cx,
        obj_rooted.handle().into(),
        c_name.as_ptr(),
        v.handle().into(),
        (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
    );
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn get_hidden_int(cx: *mut JSContext, obj: *mut JSObject, prop: &str) -> i32 {
    let c_name = ZBox::from_bytes(prop.as_bytes());
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_rooted = obj);
    let mut val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_rooted.handle().into(),
        c_name.as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut val,
        },
    );
    if val.is_int32() { val.to_int32() } else { -1 }
}

/// Build a Stats object from a `libc::stat` (used by FileHandle.stat).
unsafe fn build_stats_object(cx: *mut JSContext, st: &libc::stat) -> *mut JSObject {
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let stats = JS_NewPlainObject(cx));
    if stats.get().is_null() {
        return stats.get();
    }

    #[cfg(windows)]
    let mode_type = (st.st_mode as u32) & (S_IFMT as u32);
    #[cfg(not(windows))]
    let mode_type = (st.st_mode as u32) & libc::S_IFMT;
    #[cfg(windows)]
    let is_file = mode_type == S_IFREG as u32;
    #[cfg(not(windows))]
    let is_file = mode_type == libc::S_IFREG as u32;
    #[cfg(windows)]
    let is_dir = mode_type == S_IFDIR as u32;
    #[cfg(not(windows))]
    let is_dir = mode_type == libc::S_IFDIR as u32;
    #[cfg(windows)]
    let is_symlink = mode_type == S_IFLNK as u32;
    #[cfg(not(windows))]
    let is_symlink = mode_type == libc::S_IFLNK as u32;
    #[cfg(windows)]
    let is_block_device = mode_type == S_IFBLK as u32;
    #[cfg(not(windows))]
    let is_block_device = mode_type == libc::S_IFBLK as u32;
    #[cfg(windows)]
    let is_character_device = mode_type == S_IFCHR as u32;
    #[cfg(not(windows))]
    let is_character_device = mode_type == libc::S_IFCHR as u32;
    #[cfg(windows)]
    let is_fifo = mode_type == S_IFIFO as u32;
    #[cfg(not(windows))]
    let is_fifo = mode_type == libc::S_IFIFO as u32;
    #[cfg(windows)]
    let is_socket = mode_type == S_IFSOCK as u32;
    #[cfg(not(windows))]
    let is_socket = mode_type == libc::S_IFSOCK as u32;

    // windows CRT stat carries whole-second times (no nsec companions).
    #[cfg(windows)]
    let (atime_nsec, mtime_nsec, ctime_nsec): (i32, i32, i32) = (0, 0, 0);
    #[cfg(not(windows))]
    let (atime_nsec, mtime_nsec, ctime_nsec) =
        (st.st_atime_nsec, st.st_mtime_nsec, st.st_ctime_nsec);
    let atime_ms = st.st_atime as f64 * 1000.0 + atime_nsec as f64 / 1_000_000.0;
    let mtime_ms = st.st_mtime as f64 * 1000.0 + mtime_nsec as f64 / 1_000_000.0;
    let ctime_ms = st.st_ctime as f64 * 1000.0 + ctime_nsec as f64 / 1_000_000.0;
    let birthtime_ms = ctime_ms; // Linux fallback

    define_num_prop(cx, stats.get(), "size", st.st_size as f64);
    define_num_prop(cx, stats.get(), "dev", st.st_dev as f64);
    define_num_prop(cx, stats.get(), "ino", st.st_ino as f64);
    define_num_prop(cx, stats.get(), "mode", st.st_mode as f64);
    define_num_prop(cx, stats.get(), "nlink", st.st_nlink as f64);
    define_num_prop(cx, stats.get(), "uid", st.st_uid as f64);
    define_num_prop(cx, stats.get(), "gid", st.st_gid as f64);
    define_num_prop(cx, stats.get(), "rdev", st.st_rdev as f64);
    // windows CRT stat has no blksize/blocks — node reports 4096/0 there.
    #[cfg(windows)]
    let (blksize, blocks): (f64, f64) = (4096.0, 0.0);
    #[cfg(not(windows))]
    let (blksize, blocks) = (st.st_blksize as f64, st.st_blocks as f64);
    define_num_prop(cx, stats.get(), "blksize", blksize);
    define_num_prop(cx, stats.get(), "blocks", blocks);
    define_num_prop(cx, stats.get(), "atimeMs", atime_ms);
    define_num_prop(cx, stats.get(), "mtimeMs", mtime_ms);
    define_num_prop(cx, stats.get(), "ctimeMs", ctime_ms);
    define_num_prop(cx, stats.get(), "birthtimeMs", birthtime_ms);

    // Date objects for atime, mtime, ctime, birthtime
    let date_props: [(&str, f64); 4] = [
        ("atime", atime_ms),
        ("mtime", mtime_ms),
        ("ctime", ctime_ms),
        ("birthtime", birthtime_ms),
    ];
    for (prop, ms) in &date_props {
        let date_obj = w2::NewDateObject(cx_ref, mozjs::jsapi::ClippedTime { t: *ms });
        if !date_obj.is_null() {
            rooted!(&in(cx_ref) let date_val = mozjs::jsval::ObjectValue(date_obj));
            let c_prop = ZBox::from_bytes(prop.as_bytes());
            JS_DefineProperty(
                cx,
                stats.handle().into(),
                c_prop.as_ptr(),
                date_val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }

    define_bool_prop(cx, stats.get(), "_isFile", is_file);
    define_bool_prop(cx, stats.get(), "_isDirectory", is_dir);
    define_bool_prop(cx, stats.get(), "_isSymbolicLink", is_symlink);
    define_bool_prop(cx, stats.get(), "_isBlockDevice", is_block_device);
    define_bool_prop(cx, stats.get(), "_isCharacterDevice", is_character_device);
    define_bool_prop(cx, stats.get(), "_isFIFO", is_fifo);
    define_bool_prop(cx, stats.get(), "_isSocket", is_socket);

    w2::JS_DefineFunction(
        cx_ref,
        stats.handle().into(),
        c"isFile".as_ptr(),
        Some(stats_is_file),
        0,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        stats.handle().into(),
        c"isDirectory".as_ptr(),
        Some(stats_is_directory),
        0,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        stats.handle().into(),
        c"isSymbolicLink".as_ptr(),
        Some(stats_is_symlink),
        0,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        stats.handle().into(),
        c"isBlockDevice".as_ptr(),
        Some(stats_is_block_device),
        0,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        stats.handle().into(),
        c"isCharacterDevice".as_ptr(),
        Some(stats_is_character_device),
        0,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        stats.handle().into(),
        c"isFIFO".as_ptr(),
        Some(stats_is_fifo),
        0,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        stats.handle().into(),
        c"isSocket".as_ptr(),
        Some(stats_is_socket),
        0,
        JSPROP_ENUMERATE as u32,
    );

    stats.get()
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn get_hidden_bool(cx: *mut JSContext, obj: *mut JSObject, prop: &str) -> bool {
    let c_name = ZBox::from_bytes(prop.as_bytes());
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_rooted = obj);
    let mut val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_rooted.handle().into(),
        c_name.as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut val,
        },
    );
    val.to_boolean()
}

/// Shared `Stats.prototype.isXxx()` core: identical receiver plumbing; the
/// seven faces differ only in which hidden boolean slot they read
/// (REQ-ARCH-001 duplicate convergence).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn stats_hidden_bool_getter(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
    prop: &'static str,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let this = args.thisv().to_object());
    args.rval().set(mozjs::jsval::BooleanValue(get_hidden_bool(
        cx,
        this.get(),
        prop,
    )));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn stats_is_file(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    stats_hidden_bool_getter(cx, argc, vp, "_isFile")
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn stats_is_directory(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    stats_hidden_bool_getter(cx, argc, vp, "_isDirectory")
}
// --- FileHandle class for fs/promises ---
//
// FileHandle wraps a raw fd and provides async read/write/close/stat etc.
// Internal state is stored as hidden properties on the JS object:
//   _fd       — the raw file descriptor (i32)
//   _refs     — reference count (starts at 1, close only when 0)
//   _closed   — whether the fd has been closed (bool)

/// Create a FileHandle JS object wrapping the given fd.
unsafe fn create_filehandle_object(cx: *mut JSContext, fd: i32) -> *mut JSObject {
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let fh = mozjs_sys::jsapi::JS_NewPlainObject(cx));
    if fh.get().is_null() {
        return ::std::ptr::null_mut();
    }

    set_hidden_int(cx, fh.get(), "_fd", fd);
    set_hidden_int(cx, fh.get(), "_refs", 1);
    set_hidden_bool(cx, fh.get(), "_closed", false);

    rooted!(&in(cx_ref) let fd_val = mozjs::jsval::Int32Value(fd));
    JS_DefineProperty(
        cx,
        fh.handle().into(),
        c"fd".as_ptr(),
        fd_val.handle().into(),
        (JSPROP_ENUMERATE | JSPROP_READONLY) as u32,
    );
    JS_DefineFunction(
        cx,
        fh.handle().into(),
        c"read".as_ptr(),
        Some(fh_read),
        4,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx,
        fh.handle().into(),
        c"write".as_ptr(),
        Some(fh_write),
        4,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx,
        fh.handle().into(),
        c"close".as_ptr(),
        Some(fh_close),
        0,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx,
        fh.handle().into(),
        c"stat".as_ptr(),
        Some(fh_stat),
        0,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx,
        fh.handle().into(),
        c"chmod".as_ptr(),
        Some(fh_chmod),
        1,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx,
        fh.handle().into(),
        c"chown".as_ptr(),
        Some(fh_chown),
        2,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx,
        fh.handle().into(),
        c"datasync".as_ptr(),
        Some(fh_datasync),
        0,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx,
        fh.handle().into(),
        c"sync".as_ptr(),
        Some(fh_sync),
        0,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx,
        fh.handle().into(),
        c"truncate".as_ptr(),
        Some(fh_truncate),
        1,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx,
        fh.handle().into(),
        c"utimes".as_ptr(),
        Some(fh_utimes),
        2,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx,
        fh.handle().into(),
        c"appendFile".as_ptr(),
        Some(fh_append_file),
        1,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx,
        fh.handle().into(),
        c"readFile".as_ptr(),
        Some(fh_read_file),
        0,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx,
        fh.handle().into(),
        c"writeFile".as_ptr(),
        Some(fh_write_file),
        1,
        JSPROP_ENUMERATE as u32,
    );

    fh.get()
}

/// FileHandle constructor — callable as `new FileHandle(fd)`.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_filehandle_ctor(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd_val = if argc > 0 {
        *args.get(0).ptr
    } else {
        UndefinedValue()
    };
    let fd = if fd_val.is_int32() {
        fd_val.to_int32()
    } else {
        -1
    };
    if fd < 0 {
        JS_ReportErrorUTF8(cx, c"FileHandle requires a valid file descriptor".as_ptr());
        args.rval().set(UndefinedValue());
        return false;
    }
    let fh = create_filehandle_object(cx, fd);
    if fh.is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    args.rval().set(mozjs::jsval::ObjectValue(fh));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fh_read(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        JS_ReportErrorUTF8(
            cx,
            c"FileHandle.read() must be called on a FileHandle instance".as_ptr(),
        );
        args.rval().set(UndefinedValue());
        return false;
    }
    let fd = get_hidden_int(cx, this.to_object(), "_fd");
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()));
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }

    if fd < 0 {
        reject_with_error(
            cx,
            promise.get(),
            "FileHandle: fd is invalid (already closed)",
        );
        args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
        return true;
    }

    let length = if argc > 2 {
        let v = *args.get(2).ptr;
        if v.is_int32() {
            v.to_int32() as usize
        } else {
            65536
        }
    } else {
        65536
    };
    let position = if argc > 3 {
        let v = *args.get(3).ptr;
        if v.is_int32() {
            v.to_int32() as i64
        } else if v.is_double() {
            v.to_double() as i64
        } else {
            -1
        }
    } else {
        -1
    };

    let mut buf = vec![0u8; length];
    #[cfg(unix)]
    {
        // node contract: a numeric position reads at that ABSOLUTE offset
        // WITHOUT moving the cursor (pread); the former lseek(SET)+read pair
        // dragged the cursor along, so a following cursor-form fh.read /
        // fh.readFile started from position+len instead of where the caller
        // left it (node v24.19.0 oracle: fh.read(b,0,2,0) then fh.read(b,0,2)
        // reads file[0..2], and fh.readFile afterwards continues at 2).
        let bytes_read = if position >= 0 {
            unsafe {
                libc::pread(
                    fd,
                    buf.as_mut_ptr() as *mut ::std::ffi::c_void,
                    length,
                    position,
                )
            }
        } else {
            unsafe { libc::read(fd, buf.as_mut_ptr() as *mut ::std::ffi::c_void, length) }
        };
        if bytes_read >= 0 {
            buf.truncate(bytes_read as usize);
            // node contract: read(buffer, offset, length, position) ALSO fills the
            // caller's buffer window [offset, offset + bytesRead) — the data previously
            // landed only in the fresh result buffer, leaving the caller's zero-filled
            // (C11 wave; readv C1 #3 sibling; node v24.19.0 oracle:
            // fh.read(b,0,2,0) fills b).
            {
                let buf_arg = *args.get(0).ptr;
                if buf_arg.is_object() {
                    let off_rb: u32 = if argc > 1 {
                        let v = *args.get(1).ptr;
                        if v.is_int32() { v.to_int32().max(0) as u32 } else { 0 }
                    } else {
                        0
                    };
                    let mut wrapped_rb =
                        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
                    let cx_rb = &mut wrapped_rb;
                    rooted!(&in(cx_rb) let caller_rb = buf_arg.to_object());
                    for (i_rb, &byte_rb) in buf.iter().enumerate() {
                        rooted!(&in(cx_rb) let bv_rb = mozjs::jsval::Int32Value(byte_rb as i32));
                        JS_SetElement(
                            cx,
                            caller_rb.handle().into(),
                            off_rb + i_rb as u32,
                            bv_rb.handle().into(),
                        );
                    }
                }
            }
            let buf_obj = crate::globals::create_buffer_object(cx, &buf);
            if !buf_obj.is_null() {
                rooted!(&in(cx_ref) let result_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx));
                if !result_obj.get().is_null() {
                    rooted!(&in(cx_ref) let br_val = mozjs::jsval::DoubleValue(bytes_read as f64));
                    JS_DefineProperty(
                        cx,
                        result_obj.handle().into(),
                        c"bytesRead".as_ptr(),
                        br_val.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                    rooted!(&in(cx_ref) let buf_val = mozjs::jsval::ObjectValue(buf_obj));
                    JS_DefineProperty(
                        cx,
                        result_obj.handle().into(),
                        c"buffer".as_ptr(),
                        buf_val.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                    rooted!(&in(cx_ref) let result_val = mozjs::jsval::ObjectValue(result_obj.get()));
                    unsafe {
                        mozjs_sys::jsapi::JS::ResolvePromise(
                            cx,
                            promise.handle().into(),
                            result_val.handle().into(),
                        );
                    }
                } else {
                    resolve_undefined(cx, promise.get());
                }
            } else {
                resolve_undefined(cx, promise.get());
            }
        } else {
            reject_with_error(
                cx,
                promise.get(),
                &format!("FileHandle.read: {}", ::std::io::Error::last_os_error()),
            );
        }
    }
    #[cfg(not(unix))]
    {
        resolve_undefined(cx, promise.get());
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fh_write(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        JS_ReportErrorUTF8(
            cx,
            c"FileHandle.write() must be called on a FileHandle instance".as_ptr(),
        );
        args.rval().set(UndefinedValue());
        return false;
    }
    let fd = get_hidden_int(cx, this.to_object(), "_fd");
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()));
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }

    if fd < 0 {
        reject_with_error(
            cx,
            promise.get(),
            "FileHandle: fd is invalid (already closed)",
        );
        args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
        return true;
    }

    let data = if argc > 0 && (*args.get(0).ptr).is_object() {
        crate::node_crypto::extract_buffer_bytes(cx, *args.get(0).ptr)
    } else if argc > 0 && (*args.get(0).ptr).is_string() {
        let s = (*args.get(0).ptr).to_string();
        if !s.is_null() {
            crate::jsstr_to_rust_string(cx, s).into_bytes()
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };

    let position = if argc > 3 {
        let v = *args.get(3).ptr;
        if v.is_int32() {
            v.to_int32() as i64
        } else if v.is_double() {
            v.to_double() as i64
        } else {
            -1
        }
    } else {
        -1
    };

    #[cfg(unix)]
    {
        let bytes_written = if position >= 0 {
            unsafe {
                libc::lseek(fd, position, libc::SEEK_SET);
            }
            unsafe { libc::write(fd, data.as_ptr() as *const ::std::ffi::c_void, data.len()) }
        } else {
            unsafe { libc::write(fd, data.as_ptr() as *const ::std::ffi::c_void, data.len()) }
        };
        if bytes_written >= 0 {
            rooted!(&in(cx_ref) let result_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx));
            if !result_obj.get().is_null() {
                rooted!(&in(cx_ref) let bw_val = mozjs::jsval::DoubleValue(bytes_written as f64));
                JS_DefineProperty(
                    cx,
                    result_obj.handle().into(),
                    c"bytesWritten".as_ptr(),
                    bw_val.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
                let buf_obj = crate::globals::create_buffer_object(cx, &data);
                if !buf_obj.is_null() {
                    rooted!(&in(cx_ref) let buf_val = mozjs::jsval::ObjectValue(buf_obj));
                    JS_DefineProperty(
                        cx,
                        result_obj.handle().into(),
                        c"buffer".as_ptr(),
                        buf_val.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
                unsafe {
                    rooted!(&in(cx_ref) let result_val = mozjs::jsval::ObjectValue(result_obj.get()));
                    mozjs_sys::jsapi::JS::ResolvePromise(
                        cx,
                        promise.handle().into(),
                        result_val.handle().into(),
                    );
                }
            } else {
                resolve_undefined(cx, promise.get());
            }
        } else {
            reject_with_error(
                cx,
                promise.get(),
                &format!("FileHandle.write: {}", ::std::io::Error::last_os_error()),
            );
        }
    }
    #[cfg(not(unix))]
    {
        resolve_undefined(cx, promise.get());
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fh_close(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return false;
    }
    let fh_obj = this.to_object();
    let fd = get_hidden_int(cx, fh_obj, "_fd");
    let closed = get_hidden_bool(cx, fh_obj, "_closed");
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()));
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    if closed || fd < 0 {
        resolve_undefined(cx, promise.get());
    } else {
        #[cfg(unix)]
        {
            let ret = unsafe { libc::close(fd) };
            if ret == 0 {
                set_hidden_bool(cx, fh_obj, "_closed", true);
                set_hidden_int(cx, fh_obj, "_fd", -1);
                resolve_undefined(cx, promise.get());
            } else {
                reject_with_error(
                    cx,
                    promise.get(),
                    &format!("FileHandle.close: {}", ::std::io::Error::last_os_error()),
                );
            }
        }
        #[cfg(not(unix))]
        {
            set_hidden_bool(cx, fh_obj, "_closed", true);
            set_hidden_int(cx, fh_obj, "_fd", -1);
            resolve_undefined(cx, promise.get());
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fh_stat(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return false;
    }
    let fd = get_hidden_int(cx, this.to_object(), "_fd");
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()));
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    #[cfg(unix)]
    {
        let mut st: libc::stat = unsafe { ::std::mem::zeroed() };
        let ret = unsafe { libc::fstat(fd, &mut st) };
        if ret == 0 {
            let stats_obj = build_stats_object(cx, &st);
            if !stats_obj.is_null() {
                rooted!(&in(cx_ref) let stats_val = mozjs::jsval::ObjectValue(stats_obj));
                unsafe {
                    mozjs_sys::jsapi::JS::ResolvePromise(
                        cx,
                        promise.handle().into(),
                        stats_val.handle().into(),
                    );
                }
            } else {
                resolve_undefined(cx, promise.get());
            }
        } else {
            reject_with_error(
                cx,
                promise.get(),
                &format!("FileHandle.stat: {}", ::std::io::Error::last_os_error()),
            );
        }
    }
    #[cfg(not(unix))]
    {
        resolve_undefined(cx, promise.get());
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fh_chmod(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return false;
    }
    let fd = get_hidden_int(cx, this.to_object(), "_fd");
    let mode = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_int32() {
            v.to_int32() as u32
        } else if v.is_double() {
            v.to_double() as u32
        } else {
            0o644
        }
    } else {
        0o644
    };
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()));
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    #[cfg(unix)]
    {
        let ret = unsafe { libc::fchmod(fd, mode) };
        if ret == 0 {
            resolve_undefined(cx, promise.get());
        } else {
            reject_with_error(
                cx,
                promise.get(),
                &format!("FileHandle.chmod: {}", ::std::io::Error::last_os_error()),
            );
        }
    }
    #[cfg(not(unix))]
    {
        resolve_undefined(cx, promise.get());
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fh_chown(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return false;
    }
    let fd = get_hidden_int(cx, this.to_object(), "_fd");
    let uid = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_int32() { v.to_int32() } else { -1 }
    } else {
        -1
    };
    let gid = if argc > 1 {
        let v = *args.get(1).ptr;
        if v.is_int32() { v.to_int32() } else { -1 }
    } else {
        -1
    };
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()));
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    #[cfg(unix)]
    {
        let ret = unsafe { libc::fchown(fd, uid as u32, gid as u32) };
        if ret == 0 {
            resolve_undefined(cx, promise.get());
        } else {
            reject_with_error(
                cx,
                promise.get(),
                &format!("FileHandle.chown: {}", ::std::io::Error::last_os_error()),
            );
        }
    }
    #[cfg(not(unix))]
    {
        resolve_undefined(cx, promise.get());
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fh_datasync(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return false;
    }
    let fd = get_hidden_int(cx, this.to_object(), "_fd");
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()));
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    #[cfg(unix)]
    {
        let ret = unsafe { libc::fdatasync(fd) };
        if ret == 0 {
            resolve_undefined(cx, promise.get());
        } else {
            reject_with_error(
                cx,
                promise.get(),
                &format!("FileHandle.datasync: {}", ::std::io::Error::last_os_error()),
            );
        }
    }
    #[cfg(not(unix))]
    {
        resolve_undefined(cx, promise.get());
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fh_sync(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return false;
    }
    let fd = get_hidden_int(cx, this.to_object(), "_fd");
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()));
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    #[cfg(unix)]
    {
        let ret = unsafe { libc::fsync(fd) };
        if ret == 0 {
            resolve_undefined(cx, promise.get());
        } else {
            reject_with_error(
                cx,
                promise.get(),
                &format!("FileHandle.sync: {}", ::std::io::Error::last_os_error()),
            );
        }
    }
    #[cfg(not(unix))]
    {
        resolve_undefined(cx, promise.get());
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fh_truncate(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return false;
    }
    let fd = get_hidden_int(cx, this.to_object(), "_fd");
    let len = if argc > 0 {
        truncate_len_from_val(*args.get(0).ptr)
    } else {
        0
    };
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()));
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    #[cfg(unix)]
    {
        let ret = unsafe { libc::ftruncate(fd, len) };
        if ret == 0 {
            resolve_undefined(cx, promise.get());
        } else {
            reject_with_error(
                cx,
                promise.get(),
                &format!("FileHandle.truncate: {}", ::std::io::Error::last_os_error()),
            );
        }
    }
    #[cfg(not(unix))]
    {
        resolve_undefined(cx, promise.get());
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fh_utimes(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return false;
    }
    let fd = get_hidden_int(cx, this.to_object(), "_fd");
    let atime = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_double() {
            v.to_double()
        } else if v.is_int32() {
            v.to_int32() as f64
        } else {
            0.0
        }
    } else {
        0.0
    };
    let mtime = if argc > 1 {
        let v = *args.get(1).ptr;
        if v.is_double() {
            v.to_double()
        } else if v.is_int32() {
            v.to_int32() as f64
        } else {
            0.0
        }
    } else {
        0.0
    };
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()));
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    #[cfg(unix)]
    {
        let tv = [
            libc::timeval {
                tv_sec: atime as i64,
                tv_usec: 0,
            },
            libc::timeval {
                tv_sec: mtime as i64,
                tv_usec: 0,
            },
        ];
        let ret = unsafe { libc::futimes(fd, tv.as_ptr()) };
        if ret == 0 {
            resolve_undefined(cx, promise.get());
        } else {
            reject_with_error(
                cx,
                promise.get(),
                &format!("FileHandle.utimes: {}", ::std::io::Error::last_os_error()),
            );
        }
    }
    #[cfg(not(unix))]
    {
        resolve_undefined(cx, promise.get());
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fh_append_file(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return false;
    }
    let fd = get_hidden_int(cx, this.to_object(), "_fd");
    // FileHandle.appendFile(data[, options]) — string data honors
    // options.encoding (same single-source encoder as writeFile).
    let data_val = if argc > 0 {
        *args.get(0).ptr
    } else {
        UndefinedValue()
    };
    let encoding = get_encoding_opt(cx, &args, 1);
    let data = write_file_data_bytes(cx, data_val, encoding.as_deref());
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()));
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    #[cfg(unix)]
    {
        let ret =
            unsafe { libc::write(fd, data.as_ptr() as *const ::std::ffi::c_void, data.len()) };
        if ret >= 0 {
            resolve_undefined(cx, promise.get());
        } else {
            reject_with_error(
                cx,
                promise.get(),
                &format!(
                    "FileHandle.appendFile: {}",
                    ::std::io::Error::last_os_error()
                ),
            );
        }
    }
    #[cfg(not(unix))]
    {
        resolve_undefined(cx, promise.get());
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fh_read_file(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return false;
    }
    let fd = get_hidden_int(cx, this.to_object(), "_fd");
    // Node: fh.readFile([options]) — options is the encoding shorthand string
    // or {encoding}. C1 #13: bao always resolved a Buffer and ignored the
    // argument (node v24.19.0 oracle: fh.readFile('utf8') → string).
    let encoding = get_encoding_opt(cx, &args, 0);
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()));
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    #[cfg(unix)]
    {
        // node contract: FileHandle.readFile reads from the CURRENT file
        // position to EOF (advancing the cursor), not from offset 0. The old
        // lseek(END)→lseek(0) pair always re-read the whole file, so a second
        // readFile after a cursor read returned the full content instead of "".
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = vec![0u8; 64 * 1024];
        let mut read_err: ::std::option::Option<::std::io::Error> = ::std::option::Option::None;
        loop {
            let n = unsafe {
                libc::read(
                    fd,
                    chunk.as_mut_ptr() as *mut ::std::ffi::c_void,
                    chunk.len(),
                )
            };
            if n < 0 {
                read_err = ::std::option::Option::Some(::std::io::Error::last_os_error());
                break;
            }
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n as usize]);
        }
        match read_err {
            ::std::option::Option::Some(e) => reject_with_error(
                cx,
                promise.get(),
                &format!("FileHandle.readFile: {}", e),
            ),
            ::std::option::Option::None => {
                let val = content_jsval(cx, &buf, encoding.as_deref());
                rooted!(&in(cx_ref) let val_rooted = val);
                unsafe {
                    mozjs_sys::jsapi::JS::ResolvePromise(
                        cx,
                        promise.handle().into(),
                        val_rooted.handle().into(),
                    );
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        resolve_undefined(cx, promise.get());
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fh_write_file(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return false;
    }
    let fd = get_hidden_int(cx, this.to_object(), "_fd");
    // FileHandle.writeFile(data[, options]) — string data honors
    // options.encoding (same single-source encoder as writeFile).
    let data_val = if argc > 0 {
        *args.get(0).ptr
    } else {
        UndefinedValue()
    };
    let encoding = get_encoding_opt(cx, &args, 1);
    let data = write_file_data_bytes(cx, data_val, encoding.as_deref());
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()));
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    #[cfg(unix)]
    {
        let ret =
            unsafe { libc::write(fd, data.as_ptr() as *const ::std::ffi::c_void, data.len()) };
        if ret >= 0 {
            resolve_undefined(cx, promise.get());
        } else {
            reject_with_error(
                cx,
                promise.get(),
                &format!(
                    "FileHandle.writeFile: {}",
                    ::std::io::Error::last_os_error()
                ),
            );
        }
    }
    #[cfg(not(unix))]
    {
        resolve_undefined(cx, promise.get());
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

// --- Stats type methods ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn stats_is_block_device(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    stats_hidden_bool_getter(cx, argc, vp, "_isBlockDevice")
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn stats_is_character_device(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    stats_hidden_bool_getter(cx, argc, vp, "_isCharacterDevice")
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn stats_is_fifo(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    stats_hidden_bool_getter(cx, argc, vp, "_isFIFO")
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn stats_is_socket(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    stats_hidden_bool_getter(cx, argc, vp, "_isSocket")
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn stats_is_symlink(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    stats_hidden_bool_getter(cx, argc, vp, "_isSymbolicLink")
}

// --- Dirent type methods ---

/// Shared `Dirent.prototype.isXxx()` core: identical receiver plumbing; the
/// three faces differ only in the hidden `_typeCode` value they compare
/// against (0=file, 1=directory, 2=symbolic link) (REQ-ARCH-001 duplicate
/// convergence).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn dirent_type_code_getter(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
    expected: i32,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let this = args.thisv().to_object());
    let type_code = get_hidden_int(cx, this.get(), "_typeCode");
    args.rval().set(mozjs::jsval::BooleanValue(type_code == expected));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dirent_is_file(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    dirent_type_code_getter(cx, argc, vp, 0)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dirent_is_directory(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    dirent_type_code_getter(cx, argc, vp, 1)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dirent_is_symbolic_link(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    dirent_type_code_getter(cx, argc, vp, 2)
}

macro_rules! dirent_type_method {
    ($fn_name:ident) => {
        #[allow(unsafe_op_in_unsafe_fn)]
        unsafe extern "C" fn $fn_name(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
            let args = CallArgs::from_vp(vp, _argc);
            args.rval().set(mozjs::jsval::BooleanValue(false));
            true
        }
    };
}
dirent_type_method!(dirent_is_block_device);
dirent_type_method!(dirent_is_character_device);
dirent_type_method!(dirent_is_fifo);
dirent_type_method!(dirent_is_socket);

// --- statfs helper ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn create_statfs_object(cx: *mut JSContext, sf: &StatfsResult) -> *mut JSObject {
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = JS_NewPlainObject(cx));
    if obj.get().is_null() {
        return obj.get();
    }
    define_num_prop(cx, obj.get(), "type", sf.type_ as f64);
    define_num_prop(cx, obj.get(), "bsize", sf.bsize as f64);
    define_num_prop(cx, obj.get(), "blocks", sf.blocks as f64);
    define_num_prop(cx, obj.get(), "bfree", sf.bfree as f64);
    define_num_prop(cx, obj.get(), "bavail", sf.bavail as f64);
    define_num_prop(cx, obj.get(), "files", sf.files as f64);
    define_num_prop(cx, obj.get(), "ffree", sf.ffree as f64);
    obj.get()
}

// --- glob helpers: deleted (BCE-20260816-FS-GLOB) — see fs.glob section above ---

// --- Dir class ---
//
// fs.opendir() / fs.opendirSync() returns a Dir object with:
//   .path          — the directory path
//   .readSync()    — next Dirent or null
//   .read(cb)      — async next Dirent
//   .closeSync()   — close the dir
//   .close(cb)     — async close
//   [Symbol.asyncIterator]() — async iterable

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn create_dir_object(cx: *mut JSContext, dir_path: &str) -> *mut JSObject {
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let dir = JS_NewPlainObject(cx));
    if dir.get().is_null() {
        return dir.get();
    }

    // .path property
    let c_path = ZBox::from_bytes(dir_path.as_bytes());
    let path_str = JS_NewStringCopyZ(cx, c_path.as_ptr());
    if !path_str.is_null() {
        rooted!(&in(cx_ref) let path_val = mozjs::jsval::StringValue(&*path_str));
        JS_DefineProperty(
            cx,
            dir.handle().into(),
            c"path".as_ptr(),
            path_val.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }

    // Hidden _dirPath for method callbacks
    let c_dp = ZBox::from_bytes(dir_path.as_bytes());
    let dp_str = JS_NewStringCopyZ(cx, c_dp.as_ptr());
    if !dp_str.is_null() {
        rooted!(&in(cx_ref) let dp_val = mozjs::jsval::StringValue(&*dp_str));
        JS_DefineProperty(
            cx,
            dir.handle().into(),
            c"_dirPath".as_ptr(),
            dp_val.handle().into(),
            0,
        );
    }

    // _entriesIndex hidden prop (current position in entries cache)
    set_hidden_int(cx, dir.get(), "_entriesIndex", 0);
    // _closed hidden prop
    set_hidden_bool(cx, dir.get(), "_closed", false);

    // Methods
    JS_DefineFunction(
        cx,
        dir.handle().into(),
        c"readSync".as_ptr(),
        Some(dir_read_sync),
        0,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx,
        dir.handle().into(),
        c"read".as_ptr(),
        Some(dir_read),
        1,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx,
        dir.handle().into(),
        c"closeSync".as_ptr(),
        Some(dir_close_sync),
        0,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx,
        dir.handle().into(),
        c"close".as_ptr(),
        Some(dir_close),
        1,
        JSPROP_ENUMERATE as u32,
    );
    // Symbol.asyncIterator — for-await-of over the Dir itself (node
    // contract: `dir[Symbol.asyncIterator]` is a function; sync for..of is a
    // TypeError). C1 registry #15: the previous registration used the literal
    // string name "[]" so the iterator was never discoverable and
    // dir_symbol_iterator/dir_iterator_next were unreachable. Registered via
    // the well-known symbol PropertyKey (bao_GetWellKnownSymbolKeyRaw — the
    // PropertyKey sret-divergent under MSVC; same idiom as node_url.rs /
    // bun_sqlite.rs).
    let sym_ai_key = mozjs_sys::jsapi::JS::PropertyKey {
        asBits_: mozjs::glue::bao_GetWellKnownSymbolKeyRaw(
            cx,
            mozjs_sys::jsapi::JS::SymbolCode::asyncIterator as u32,
        ),
    };
    let ai_f = JS_NewFunction(cx, Some(dir_symbol_iterator), 0, 0, c"[Symbol.asyncIterator]".as_ptr());
    if !ai_f.is_null() {
        let ai_obj = JS_GetFunctionObject(ai_f);
        if !ai_obj.is_null() {
            rooted!(&in(cx_ref) let ai_val = mozjs::jsval::ObjectValue(ai_obj));
            JS_DefinePropertyById2(
                cx,
                dir.handle().into(),
                Handle::from_marked_location(&sym_ai_key),
                ai_val.handle().into(),
                0,
            );
        }
    }

    dir.get()
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn dir_ensure_entries(cx: *mut JSContext, dir_obj: *mut JSObject) {
    // Check if _entries already exists
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let dir_rooted = dir_obj);
    let mut entries_val = UndefinedValue();
    JS_GetProperty(
        cx,
        dir_rooted.handle().into(),
        c"_entries".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut entries_val,
        },
    );
    if !entries_val.is_undefined() {
        return;
    }

    // Read directory and cache entries
    let dir_path = {
        let mut dp_val = UndefinedValue();
        JS_GetProperty(
            cx,
            dir_rooted.handle().into(),
            c"_dirPath".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut dp_val,
            },
        );
        if dp_val.is_string() {
            crate::jsstr_to_rust_string(cx, dp_val.to_string())
        } else {
            return;
        }
    };

    match fs::read_dir(&dir_path) {
        Ok(raw_entries) => {
            let entries: Vec<(String, bool)> = raw_entries
                .flatten()
                .map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    let is_dir = e.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
                    (name, is_dir)
                })
                .collect();

            rooted!(&in(cx_ref) let arr = w2::NewArrayObject1(cx_ref, entries.len()));
            if !arr.get().is_null() {
                for (idx, (name, is_dir)) in entries.iter().enumerate() {
                    let dirent = create_dirent(cx, name, *is_dir);
                    rooted!(&in(cx_ref) let val = mozjs::jsval::ObjectValue(dirent));
                    JS_DefineElement(
                        cx,
                        arr.handle().into(),
                        idx as u32,
                        val.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
            }
            rooted!(&in(cx_ref) let arr_val = mozjs::jsval::ObjectValue(arr.get()));
            JS_DefineProperty(
                cx,
                dir_rooted.handle().into(),
                c"_entries".as_ptr(),
                arr_val.handle().into(),
                0,
            );
        }
        Err(_) => {
            // On error, set empty entries array
            rooted!(&in(cx_ref) let arr = w2::NewArrayObject1(cx_ref, 0));
            rooted!(&in(cx_ref) let arr_val = mozjs::jsval::ObjectValue(arr.get()));
            JS_DefineProperty(
                cx,
                dir_rooted.handle().into(),
                c"_entries".as_ptr(),
                arr_val.handle().into(),
                0,
            );
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dir_read_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let this = args.thisv().to_object());

    let closed = get_hidden_bool(cx, this.get(), "_closed");
    if closed {
        let c_msg = ZBox::from_bytes("Dir is already closed".as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }

    dir_ensure_entries(cx, this.get());

    let idx = get_hidden_int(cx, this.get(), "_entriesIndex") as u32;

    let mut entries_val = UndefinedValue();
    JS_GetProperty(
        cx,
        this.handle().into(),
        c"_entries".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut entries_val,
        },
    );
    if entries_val.is_object() {
        let entries_obj = entries_val.to_object();
        rooted!(&in(cx_ref) let eo = entries_obj);
        let mut len_val = UndefinedValue();
        JS_GetProperty(
            cx,
            eo.handle().into(),
            c"length".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut len_val,
            },
        );
        let len = if len_val.is_int32() {
            len_val.to_int32() as u32
        } else {
            0
        };

        if idx < len {
            let mut elem = UndefinedValue();
            JS_GetElement(
                cx,
                eo.handle().into(),
                idx,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut elem,
                },
            );
            args.rval().set(elem);
            set_hidden_int(cx, this.get(), "_entriesIndex", (idx + 1) as i32);
        } else {
            args.rval().set(mozjs::jsval::NullValue());
        }
    } else {
        args.rval().set(mozjs::jsval::NullValue());
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dir_read(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let this = args.thisv().to_object());

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 0) {
        // Synchronously read next entry, then invoke callback
        let closed = get_hidden_bool(cx, this.get(), "_closed");
        if closed {
            rooted!(&in(cx_ref) let err_obj = JS_NewPlainObject(cx));
            if !err_obj.get().is_null() {
                let c_msg = ZBox::from_bytes("Dir is already closed".as_bytes());
                let msg_str = JS_NewStringCopyZ(cx, c_msg.as_ptr());
                if !msg_str.is_null() {
                    rooted!(&in(cx_ref) let msg_val = mozjs::jsval::StringValue(&*msg_str));
                    JS_DefineProperty(
                        cx,
                        err_obj.handle().into(),
                        c"message".as_ptr(),
                        msg_val.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
            }
            rooted!(&in(cx_ref) let err_val = mozjs::jsval::ObjectValue(err_obj.get()));
            let err_args = HandleValueArray {
                length_: 1,
                elements_: &err_val.get() as *const JSVal,
            };
            let global = CurrentGlobalOrNull(cx);
            if !global.is_null() {
                rooted!(&in(cx_ref) let global_rooted = global);
                rooted!(&in(cx_ref) let cb = callback);
                rooted!(&in(cx_ref) let cb_val = mozjs::jsval::ObjectValue(cb.get()));
                let mut rval = UndefinedValue();
                JS_CallFunctionValue(
                    cx,
                    global_rooted.handle().into(),
                    cb_val.handle().into(),
                    &err_args,
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut rval,
                    },
                );
                JS_ClearPendingException(cx);
            }
            args.rval().set(UndefinedValue());
            return true;
        }

        dir_ensure_entries(cx, this.get());
        let idx = get_hidden_int(cx, this.get(), "_entriesIndex") as u32;
        let mut entries_val = UndefinedValue();
        JS_GetProperty(
            cx,
            this.handle().into(),
            c"_entries".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut entries_val,
            },
        );

        let next_val = if entries_val.is_object() {
            let entries_obj = entries_val.to_object();
            rooted!(&in(cx_ref) let eo = entries_obj);
            let mut len_val = UndefinedValue();
            JS_GetProperty(
                cx,
                eo.handle().into(),
                c"length".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut len_val,
                },
            );
            let len = if len_val.is_int32() {
                len_val.to_int32() as u32
            } else {
                0
            };
            if idx < len {
                let mut elem = UndefinedValue();
                JS_GetElement(
                    cx,
                    eo.handle().into(),
                    idx,
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut elem,
                    },
                );
                set_hidden_int(cx, this.get(), "_entriesIndex", (idx + 1) as i32);
                elem
            } else {
                mozjs::jsval::NullValue()
            }
        } else {
            mozjs::jsval::NullValue()
        };

        rooted!(&in(cx_ref) let next_rooted = next_val);
        rooted!(&in(cx_ref) let cb = callback);
        rooted!(&in(cx_ref) let cb_val = mozjs::jsval::ObjectValue(cb.get()));
        let args_arr = [UndefinedValue(), next_rooted.get()];
        let cb_args = HandleValueArray {
            length_: 2,
            elements_: args_arr.as_ptr(),
        };
        let global = CurrentGlobalOrNull(cx);
        if !global.is_null() {
            rooted!(&in(cx_ref) let global_rooted = global);
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(
                cx,
                global_rooted.handle().into(),
                cb_val.handle().into(),
                &cb_args,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut rval,
                },
            );
            JS_ClearPendingException(cx);
        }
        args.rval().set(UndefinedValue());
        return true;
    }

    // No callback: return a Promise
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }

    let closed = get_hidden_bool(cx, this.get(), "_closed");
    if closed {
        reject_with_error(cx, promise.get(), "Dir is already closed");
    } else {
        dir_ensure_entries(cx, this.get());
        let idx = get_hidden_int(cx, this.get(), "_entriesIndex") as u32;
        let mut entries_val = UndefinedValue();
        JS_GetProperty(
            cx,
            this.handle().into(),
            c"_entries".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut entries_val,
            },
        );

        let next_val = if entries_val.is_object() {
            let entries_obj = entries_val.to_object();
            rooted!(&in(cx_ref) let eo = entries_obj);
            let mut len_val = UndefinedValue();
            JS_GetProperty(
                cx,
                eo.handle().into(),
                c"length".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut len_val,
                },
            );
            let len = if len_val.is_int32() {
                len_val.to_int32() as u32
            } else {
                0
            };
            if idx < len {
                let mut elem = UndefinedValue();
                JS_GetElement(
                    cx,
                    eo.handle().into(),
                    idx,
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut elem,
                    },
                );
                set_hidden_int(cx, this.get(), "_entriesIndex", (idx + 1) as i32);
                elem
            } else {
                mozjs::jsval::NullValue()
            }
        } else {
            mozjs::jsval::NullValue()
        };

        rooted!(&in(cx_ref) let val = next_val);
        unsafe {
            mozjs_sys::jsapi::JS::ResolvePromise(cx, promise.handle().into(), val.handle().into());
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dir_close_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let this = args.thisv().to_object());
    set_hidden_bool(cx, this.get(), "_closed", true);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dir_close(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let this = args.thisv().to_object());
    set_hidden_bool(cx, this.get(), "_closed", true);

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 0) {
        rooted!(&in(cx_ref) let cb = callback);
        rooted!(&in(cx_ref) let cb_val = mozjs::jsval::ObjectValue(cb.get()));
        let null_args = HandleValueArray::empty();
        let global = CurrentGlobalOrNull(cx);
        if !global.is_null() {
            rooted!(&in(cx_ref) let global_rooted = global);
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(
                cx,
                global_rooted.handle().into(),
                cb_val.handle().into(),
                &null_args,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut rval,
                },
            );
            JS_ClearPendingException(cx);
        }
        args.rval().set(UndefinedValue());
        return true;
    }

    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dir_symbol_iterator(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    // Return an object with next() method for for-await-of
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let iter = JS_NewPlainObject(cx));
    if iter.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    // Store reference to the Dir object on the iterator
    let this_val = args.thisv();
    if this_val.is_object() {
        rooted!(&in(cx_ref) let dir_ref = this_val.to_object());
        rooted!(&in(cx_ref) let dir_ref_val = mozjs::jsval::ObjectValue(dir_ref.get()));
        JS_DefineProperty(
            cx,
            iter.handle().into(),
            c"_dirRef".as_ptr(),
            dir_ref_val.handle().into(),
            0,
        );
    }

    JS_DefineFunction(
        cx,
        iter.handle().into(),
        c"next".as_ptr(),
        Some(dir_iterator_next),
        0,
        JSPROP_ENUMERATE as u32,
    );
    args.rval().set(mozjs::jsval::ObjectValue(iter.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dir_iterator_next(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }

    // Get the Dir object reference
    let this_val = args.thisv();
    if !this_val.is_object() {
        reject_with_error(
            cx,
            promise.get(),
            "Dir iterator next() called on wrong object",
        );
        args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
        return true;
    }
    let iter_obj = this_val.to_object();
    rooted!(&in(cx_ref) let iter_rooted = iter_obj);
    let mut dir_ref_val = UndefinedValue();
    JS_GetProperty(
        cx,
        iter_rooted.handle().into(),
        c"_dirRef".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut dir_ref_val,
        },
    );

    if !dir_ref_val.is_object() {
        // No dir ref, return {done: true}
        rooted!(&in(cx_ref) let result_obj = JS_NewPlainObject(cx));
        if !result_obj.get().is_null() {
            define_bool_prop(cx, result_obj.get(), "done", true);
        }
        rooted!(&in(cx_ref) let val = mozjs::jsval::ObjectValue(result_obj.get()));
        unsafe {
            mozjs_sys::jsapi::JS::ResolvePromise(cx, promise.handle().into(), val.handle().into());
        }
        args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
        return true;
    }

    let dir_obj = dir_ref_val.to_object();
    let closed = get_hidden_bool(cx, dir_obj, "_closed");
    if closed {
        rooted!(&in(cx_ref) let result_obj = JS_NewPlainObject(cx));
        if !result_obj.get().is_null() {
            define_bool_prop(cx, result_obj.get(), "done", true);
        }
        rooted!(&in(cx_ref) let val = mozjs::jsval::ObjectValue(result_obj.get()));
        unsafe {
            mozjs_sys::jsapi::JS::ResolvePromise(cx, promise.handle().into(), val.handle().into());
        }
        args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
        return true;
    }

    dir_ensure_entries(cx, dir_obj);
    let idx = get_hidden_int(cx, dir_obj, "_entriesIndex") as u32;

    rooted!(&in(cx_ref) let dir_rooted = dir_obj);
    let mut entries_val = UndefinedValue();
    JS_GetProperty(
        cx,
        dir_rooted.handle().into(),
        c"_entries".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut entries_val,
        },
    );

    let next_dirent = if entries_val.is_object() {
        let entries_arr = entries_val.to_object();
        rooted!(&in(cx_ref) let ea = entries_arr);
        let mut len_val = UndefinedValue();
        JS_GetProperty(
            cx,
            ea.handle().into(),
            c"length".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut len_val,
            },
        );
        let len = if len_val.is_int32() {
            len_val.to_int32() as u32
        } else {
            0
        };
        if idx < len {
            let mut elem = UndefinedValue();
            JS_GetElement(
                cx,
                ea.handle().into(),
                idx,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut elem,
                },
            );
            set_hidden_int(cx, dir_obj, "_entriesIndex", (idx + 1) as i32);
            elem
        } else {
            mozjs::jsval::NullValue()
        }
    } else {
        mozjs::jsval::NullValue()
    };

    rooted!(&in(cx_ref) let result_obj = JS_NewPlainObject(cx));
    if !result_obj.get().is_null() {
        if next_dirent.is_null() {
            define_bool_prop(cx, result_obj.get(), "done", true);
        } else {
            define_bool_prop(cx, result_obj.get(), "done", false);
            rooted!(&in(cx_ref) let dv = next_dirent);
            JS_DefineProperty(
                cx,
                result_obj.handle().into(),
                c"value".as_ptr(),
                dv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }
    rooted!(&in(cx_ref) let val = mozjs::jsval::ObjectValue(result_obj.get()));
    unsafe {
        mozjs_sys::jsapi::JS::ResolvePromise(cx, promise.handle().into(), val.handle().into());
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

#[cfg(test)]
mod tests {
    use super::mkdtemp_inner;

    // Node parity: an empty prefix must fail with EINVAL before touching the
    // filesystem, never create a bare random directory in cwd (upstream b7a043103).
    #[test]
    fn mkdtemp_empty_prefix_returns_einval() {
        let err = mkdtemp_inner("").expect_err("empty prefix must be rejected");
        assert_eq!(err.raw_os_error(), Some(libc::EINVAL));
    }
}

#[cfg(test)]
mod fs_async_carrier_tests {
    //! Fail-closed completion (A' route): when the captured pump loop is NOT
    //! registered with ConcurrentWakeup (owning thread never materialized it
    //! through timers::with_event_loop — here: a bare MiniEventLoop that
    //! skipped the registration handshake), `complete_post` must RELEASE the
    //! carrier Box instead of leaking it. No JSContext is touched
    //! (`cb_root: None`), so the test runs engine-free.

    use super::*;

    #[test]
    fn complete_post_unregistered_loop_releases_carrier() {
        let before = fs_async_pending();

        // An UNREGISTERED loop: MiniEventLoop::init() without the
        // timers::with_event_loop registration. Leaked (never freed) — the
        // production lifetime contract for loop allocations.
        let orphan_loop: *mut bun_event_loop::MiniEventLoop::MiniEventLoop<'static> =
            bun_core::heap::into_raw(Box::new(
                bun_event_loop::MiniEventLoop::MiniEventLoop::init(),
            ));

        let ctx = Box::new(FsAsyncCtx {
            cx: ::std::ptr::null_mut(),
            callback: ::std::ptr::null_mut(),
            cb_root: None,
            result: Arc::new(Mutex::new(None)),
            encoding: None,
            op_name: "test".to_string(),
            path: String::new(),
            mini_loop_ptr: orphan_loop,
            concurrent_task:
                bun_event_loop::AnyTaskWithExtraContext::AnyTaskWithExtraContext::default(),
            has_scheduled: AtomicBool::new(false),
            writeback: None,
            writeback_offset: 0,
        });
        let ctx_ptr = Box::into_raw(ctx);
        // Mirror the spawn-side increment (retired by FsAsyncCtx::drop).
        FS_ASYNC_PENDING.fetch_add(1, AtomicOrdering::Release);

        // SAFETY: ctx_ptr is a live Box we just created; complete_post only
        // touches atomics + the registry-checked enqueue, which must observe
        // "unregistered" and return false without touching the loop.
        unsafe { complete_post(ctx_ptr) };

        assert_eq!(
            fs_async_pending(),
            before,
            "fail-closed release must retire the pending count (a leak would pin the pump forever)"
        );
    }
}
