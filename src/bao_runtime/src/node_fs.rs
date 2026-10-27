// @trace REQ-ENG-007
use ::std::cell::{Cell, RefCell};
use ::std::collections::{BTreeSet, HashMap, VecDeque};
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
    /// fs.glob withFileTypes: relative-or-absolute yielded paths + UV type
    /// codes; the resolver joins root and constructs canonical internal/fs/utils
    /// Dirent instances (basename, type, dirname) — node globSync's #toDirent.
    OkGlobDirents {
        root: String,
        entries: Vec<(String, i32)>,
    },
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

/// Byte window of a caller-owned ArrayBufferView (Buffer/Uint8Array, any
/// TypedArray, DataView). Used by the async read/readv completion to backfill
/// the buffers the caller passed (node contract: the callback receives the
/// SAME buffer object, filled), by the fs path resolver for Buffer/TypedArray
/// path bytes, and by the FileHandle readv/writev iovec builder. Returns None
/// for detached and NON-VIEW objects.
///
/// @trace REQ-ENG-007 [entity:Buffer] — fs URL-path SIGSEGV root fix: the
/// former body called the UNCHECKED JS_GetUint8ArrayData/
/// JS_GetTypedArrayByteLength pair on any object. jsapi contract (js/public/
/// experimental/TypedData.h): "|obj| must have passed a JS_IsTypedArrayObject
/// test" — a URL (or any non-view) arg took the internal class-confusion cast
/// and read garbage reserved slots → deterministic SIGSEGV (host repro 3/3:
/// cp(pathToFileURL(src), pathToFileURL(dest), {recursive:true}, cb); every
/// get_path_arg face shares resolve_fs_path_value), and the correct URL
/// handler (url_val_to_fs_path) was never reached. Replaced with the CHECKED
/// JS_GetObjectAsArrayBufferView — returns nullptr without throwing for
/// non-views and fills the BYTE length for typed arrays and DataViews alike
/// (JS_GetObjectAsArrayBufferView → GetArrayBufferViewLengthAndData). Same
/// checked-getter form as the in-tree precedents globals.rs buffer_view_bytes
/// (JS_GetObjectAsUint8Array) and node_crypto.rs extract_buffer_bytes
/// (Uint8Array → ArrayBufferView cascade). Same-class sweep in this file: the
/// fs.readv spawn loop, readvSync (unix + windows legs), and readSync's
/// view-guarded accessors were also on the unchecked pair and now go through
/// this helper / the view-family getters.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn view_byte_slot(view: JSVal) -> ::std::option::Option<(*mut u8, usize)> {
    if !view.is_object() {
        return ::std::option::Option::None;
    }
    let obj = view.to_object();
    let mut len: usize = 0;
    let mut is_shared = false;
    let mut data: *mut u8 = ::std::ptr::null_mut();
    // CHECKED form: non-views (URL, plain objects, ...) yield nullptr here —
    // never a class-confusion cast. Callers fall through to the next handler
    // in their contract chain (URL resolution / invalid-arg throw /
    // fresh-buffer fallback).
    let unwrapped = JS_GetObjectAsArrayBufferView(obj, &mut len, &mut is_shared, &mut data);
    if unwrapped.is_null() || data.is_null() {
        return ::std::option::Option::None;
    }
    ::std::option::Option::Some((data, len))
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
            // node: NO encoding → Buffer (binary-safe); the former
            // string_or_buffer(None) → utf8 String diverged on every async
            // read face (readFile empty-file '' vs Buffer — same root as the
            // promises face fixed below).
            let val = content_jsval(cx, &data, encoding);
            rooted!(&in(realm_cx) let val_rooted = val);
// unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
            rooted!(&in(realm_cx) let cb_args_rooted = mozjs_sys::jsgc::ValueArray::<2>::new([
                UndefinedValue(),
                val_rooted.get(),
            ]));
            let cb_args = JS::HandleValueArray::from(&cb_args_rooted);
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
            // spawn-based async stat: the {bigint} option rides the caller's
            // options argument, which the async plumbing does not carry yet.
            // Number face here (registered residual; the pinned corpus faces
            // statSync/fsp.stat parse the option at their own slots).
            let stats_obj = create_stats_object(cx, &stat, false);
            rooted!(&in(realm_cx) let stats_val = mozjs::jsval::ObjectValue(stats_obj));
// unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
            rooted!(&in(realm_cx) let cb_args_rooted = mozjs_sys::jsgc::ValueArray::<2>::new([
                UndefinedValue(),
                stats_val.get(),
            ]));
            let cb_args = JS::HandleValueArray::from(&cb_args_rooted);
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
// unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
            rooted!(&in(realm_cx) let cb_args_rooted = mozjs_sys::jsgc::ValueArray::<2>::new([
                UndefinedValue(),
                val_rooted.get(),
            ]));
            let cb_args = JS::HandleValueArray::from(&cb_args_rooted);
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
// unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
            rooted!(&in(realm_cx) let cb_args_rooted = mozjs_sys::jsgc::ValueArray::<1>::new([
                UndefinedValue(),
            ]));
            let cb_args = JS::HandleValueArray::from(&cb_args_rooted);
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
// unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
            rooted!(&in(realm_cx) let cb_args_rooted = mozjs_sys::jsgc::ValueArray::<2>::new([
                UndefinedValue(),
                val.get(),
            ]));
            let cb_args = JS::HandleValueArray::from(&cb_args_rooted);
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
// unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
            rooted!(&in(realm_cx) let cb_args_rooted = mozjs_sys::jsgc::ValueArray::<2>::new([
                UndefinedValue(),
                val.get(),
            ]));
            let cb_args = JS::HandleValueArray::from(&cb_args_rooted);
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
// unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
            rooted!(&in(realm_cx) let cb_args_rooted = mozjs_sys::jsgc::ValueArray::<2>::new([
                UndefinedValue(),
                val.get(),
            ]));
            let cb_args = JS::HandleValueArray::from(&cb_args_rooted);
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
// unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
            rooted!(&in(realm_cx) let cb_args_rooted = mozjs_sys::jsgc::ValueArray::<3>::new([
                UndefinedValue(),
                br_val.get(),
                buf_rooted.get(),
            ]));
            let cb_args = JS::HandleValueArray::from(&cb_args_rooted);
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
            // views, in iovec order. node's readv callback is (err, n, buffers).
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
            // Third callback arg: the buffers array (node parity — the
            // customPromisifyArgs contract resolves {bytesRead, buffers} from
            // (err, n, buffers); a fresh array over the SAME filled views
            // satisfies the deep-equality contract).
            let mut buffers_arr_val = UndefinedValue();
            if let ::std::option::Option::Some(root) = ctx.writeback.as_ref() {
                rooted!(&in(realm_cx) let arr = w2::NewArrayObject1(realm_cx, root.len()));
                if !arr.get().is_null() {
                    for i in 0..root.len() {
                        rooted!(&in(realm_cx) let view_val = root.get(i));
                        JS_DefineElement(
                            cx,
                            arr.handle().into(),
                            i as u32,
                            view_val.handle().into(),
                            JSPROP_ENUMERATE as u32,
                        );
                    }
                    buffers_arr_val = mozjs::jsval::ObjectValue(arr.get());
                }
            }
            rooted!(&in(realm_cx) let buffers_rooted = buffers_arr_val);
// unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
            rooted!(&in(realm_cx) let cb_args_rooted = mozjs_sys::jsgc::ValueArray::<3>::new([
                UndefinedValue(),
                br_val.get(),
                buffers_rooted.get(),
            ]));
            let cb_args = JS::HandleValueArray::from(&cb_args_rooted);
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
// unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
            rooted!(&in(realm_cx) let cb_args_rooted = mozjs_sys::jsgc::ValueArray::<2>::new([
                UndefinedValue(),
                val.get(),
            ]));
            let cb_args = JS::HandleValueArray::from(&cb_args_rooted);
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
// unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
            rooted!(&in(realm_cx) let cb_args_rooted = mozjs_sys::jsgc::ValueArray::<2>::new([
                UndefinedValue(),
                arr_val.get(),
            ]));
            let cb_args = JS::HandleValueArray::from(&cb_args_rooted);
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
// unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
            rooted!(&in(realm_cx) let cb_args_rooted = mozjs_sys::jsgc::ValueArray::<2>::new([
                UndefinedValue(),
                arr_val.get(),
            ]));
            let cb_args = JS::HandleValueArray::from(&cb_args_rooted);
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
        Some(Ok(FsAsyncResult::OkGlobDirents { root, entries })) => {
            rooted!(&in(realm_cx) let arr = w2::NewArrayObject1(realm_cx, entries.len()));
            if !arr.get().is_null() {
                for (idx, (path, type_code)) in entries.iter().enumerate() {
                    let full = if path.starts_with('/') {
                        path.clone()
                    } else {
                        glob_join_posix(&root, path)
                    };
                    // SAFETY: JS thread resolver frame; rooted inside.
                    match unsafe {
                        construct_glob_dirent(
                            cx,
                            glob_basename(&full),
                            *type_code,
                            glob_dirname(&full),
                        )
                    } {
                        ::std::result::Result::Ok(d) => {
                            rooted!(&in(realm_cx) let val = mozjs::jsval::ObjectValue(d));
                            JS_DefineElement(
                                cx,
                                arr.handle().into(),
                                idx as u32,
                                val.handle().into(),
                                JSPROP_ENUMERATE as u32,
                            );
                        }
                        ::std::result::Result::Err(()) => {
                            JS_ClearPendingException(cx);
                        }
                    }
                }
            }
            rooted!(&in(realm_cx) let arr_val = mozjs::jsval::ObjectValue(arr.get()));
// unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
            rooted!(&in(realm_cx) let cb_args_rooted = mozjs_sys::jsgc::ValueArray::<2>::new([
                UndefinedValue(),
                arr_val.get(),
            ]));
            let cb_args = JS::HandleValueArray::from(&cb_args_rooted);
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
// unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
            rooted!(&in(realm_cx) let cb_args_rooted = mozjs_sys::jsgc::ValueArray::<2>::new([
                UndefinedValue(),
                stats_val.get(),
            ]));
            let cb_args = JS::HandleValueArray::from(&cb_args_rooted);
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
// unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
            rooted!(&in(realm_cx) let cb_args_rooted = mozjs_sys::jsgc::ValueArray::<1>::new([
                err_val.get(),
            ]));
            let cb_args = JS::HandleValueArray::from(&cb_args_rooted);
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

// @trace REQ-ENG-007 [api:fs.WriteStream|fs.ReadStream|createReadStream|createWriteStream]
// — node-parity fs stream constructors: real classes (prototype.open
// override point, subclass `.call` compat, autoDestroy, close/setEncoding/
// destroyed faces) + fd-backed factories for FileHandle.createReadStream/
// createWriteStream. Behaviorally verified against the node v24.19.0 oracle
// (stream-construct-compat / read-write fixture harness, 12/12).
const FS_STREAM_JS: &str = r#"
// @trace REQ-ENG-007 [api:fs.WriteStream|fs.ReadStream|createWriteStream|createReadStream]
// — node-parity stream face: per-instance `destroyed` accessor (bao
// mock.getter contract) + ERR_STREAM_DESTROYED flush seam + end-window read
// math + _reading in-flight pump guard + readOptionsClassify (proxy-transparent
// options gate; see the Rust consume site in parse_read_window_args).
(function() {
  var fs = globalThis.__fs_stream_ref;

  // ── minimal EventEmitter base ──────────────────────────────────────────
  // node fs streams are real EventEmitters; this face (on/once/off/emit/
  // removeListener/addListener) is what the node test corpus drives on them.
  function EEBase() { this._events = {}; }
  EEBase.prototype.on = function(e, fn) {
    (this._events[e] || (this._events[e] = [])).push(fn);
    return this;
  };
  EEBase.prototype.addListener = EEBase.prototype.on;
  EEBase.prototype.once = function(e, fn) {
    var self = this;
    function g() {
      self.removeListener(e, g);
      fn.apply(self, arguments);
    }
    g._listener = fn;
    this.on(e, g);
    return this;
  };
  EEBase.prototype.removeListener = function(e, fn) {
    var ls = this._events[e];
    if (ls) {
      for (var i = 0; i < ls.length; i++) {
        if (ls[i] === fn || ls[i]._listener === fn) { ls.splice(i, 1); break; }
      }
      if (ls.length === 0) delete this._events[e];
    }
    return this;
  };
  EEBase.prototype.off = EEBase.prototype.removeListener;
  EEBase.prototype.emit = function(e) {
    var ls = this._events[e];
    if (!ls || ls.length === 0) return false;
    var a = Array.prototype.slice.call(arguments, 1);
    var copy = ls.slice();
    for (var i = 0; i < copy.length; i++) copy[i].apply(this, a);
    return true;
  };

  function isObjectArg(o) { return o !== null && typeof o === 'object'; }

  // ── ReadStream ─────────────────────────────────────────────────────────
  // node lib/fs.js ReadStream face: constructor calls this.open()
  // SYNCHRONOUSLY so subclass prototype overrides take effect
  // (test-fs-stream-construct-compat-*), open error → destroy() + 'error',
  // autoClose default true, close emitted asynchronously after fd teardown.
  function ReadStream(path, opts) {
    if (!(this instanceof ReadStream)) return new ReadStream(path, opts);
    EEBase.call(this);
    opts = opts || {};
    this.path = path;
    this.fd = (opts.fd != null) ? opts.fd : null;
    this.flags = opts.flags || 'r';
    this.mode = opts.mode || 438; // 0o666
    this.autoClose = opts.autoClose !== false;
    this.start = opts.start;
    this.end = opts.end;
    // node validates the window in the constructor (sync throw —
    // test-fs-read-stream asserts RangeError ERR_OUT_OF_RANGE
    // 'The value of "start" is out of range. It must be <= "end" (here: 2).
    // Received 10').
    if (this.start !== undefined && this.end !== undefined && this.start > this.end) {
      var rangeErr = new RangeError('The value of "start" is out of range. It must be <= "end" (here: ' + this.end + '). Received ' + this.start);
      rangeErr.code = 'ERR_OUT_OF_RANGE';
      throw rangeErr;
    }
    this.pos = (typeof opts.start === 'number') ? opts.start : null;
    this.bytesRead = 0;
    this.readable = true;
    // node window contract (ReadStream._read): n = min(end - pos + 1, hwm) on
    // positioned streams, min(end - bytesRead + 1, hwm) otherwise; end
    // defaults to Infinity (node oracle: own `end` prop === Infinity).
    this.end = (typeof opts.end === 'number') ? opts.end : Infinity;
    this._destroyed = false;
    // bao mock.getter contract (node_test _spyAccessor) resolves the property
    // on the INSTANCE only (Object.getOwnPropertyDescriptor — no prototype
    // walk); node defines `destroyed` as a prototype accessor and walks the
    // chain. One own configurable accessor carries the same observable face
    // for both (mock.getter redefine needs configurable:true).
    Object.defineProperty(this, 'destroyed', {
      get: function() { return this._destroyed; },
      set: function(v) { this._destroyed = v; },
      enumerable: false, configurable: true,
    });
    this.closed = false;
    this._encoding = (typeof opts.encoding === 'string') ? opts.encoding : null;
    this._fdOpen = this.fd != null;
    this._closed = false;
    this._paused = false;
    this._readDone = false;
    // in-flight read flag: node never re-enters _read while a read is
    // outstanding (push() backpressure gates it). Without the flag, a
    // resume() inside the 'open' handler started a chain AND the deferred
    // _readLoop started another — two concurrent fs.read(2)s racing the
    // shared fd offset (duplicate/lost chunks, nondeterministic bytesRead).
    this._reading = false;
    // node opts.fs pluggable face: {open, read, close} override the default
    // fs natives for this stream (test-fs-read-stream passes mustCall-wrapped
    // fns — the calls must route through the pluggable object).
    this._fs = (opts.fs != null && typeof opts.fs === 'object') ? opts.fs : null;
    var self = this;
    // node stream autoDestroy face: an 'error' tears the stream down
    // (destroyed=true) even when prototype.open is overridden and only the
    // error is emitted (test-fs-stream-construct-compat-error-read).
    this.on('error', function() {
      if (self.autoClose) self.destroy();
    });
    if (this.fd === null) {
      this.open();
    } else {
      setTimeout(function() {
        if (self.destroyed) return;
        self.emit('open', self.fd);
        self._readLoop();
      }, 0);
    }
  }
  ReadStream.prototype = Object.create(EEBase.prototype);
  ReadStream.prototype.constructor = ReadStream;

  // node Readable.prototype accessor face (enumerable: false on node): end
  // emitted. `destroyed` rides the own per-instance accessor (bao mock.getter).
  Object.defineProperties(ReadStream.prototype, {
    readableEnded: {
      get: function() { return this._readDone === true; },
      enumerable: false, configurable: true,
    },
    readableFinished: {
      get: function() { return this._readDone === true; },
      enumerable: false, configurable: true,
    },
  });

  ReadStream.prototype.setEncoding = function(enc) {
    this._encoding = enc;
    return this;
  };

  ReadStream.prototype.open = function() {
    var self = this;
    var fapi = self._fs || fs;
    fapi.open(this.path, this.flags, this.mode, function(err, fd) {
      if (self.destroyed) {
        // node fs/streams: an fd landing on a destroyed stream is closed
        // immediately, never adopted.
        (fapi.close || fs.close).call(fapi, fd, function() {});
        return;
      }
      if (err) {
        if (self.autoClose) self.destroy();
        self.emit('error', err);
        return;
      }
      self.fd = fd;
      self._fdOpen = true;
      self.emit('open', fd);
      self.emit('ready');
      self._readLoop();
    });
  };

  // Async chunk pump (node ReadStream shape): one fs.read per chunk with the
  // callback chaining the next. pause() stops the chain between chunks
  // (resume() re-enters), matching node's flow-control contract
  // (test-fs-read-stream test1 pauses inside the 'data' handler and expects
  // no further data until resume).
  ReadStream.prototype._readChunk = function() {
    var self = this;
    if (self.destroyed || self.fd == null || !self._fdOpen || self._paused || self._readDone || self._reading) return;
    // node ReadStream._read window math (oracle): n = min(end - pos + 1, hwm)
    // on positioned streams, min(end - bytesRead + 1, hwm) otherwise; n <= 0
    // is EOF. The former `pos > end` gate never fired on the sequential face
    // (pos === null), so {end} windows read past the bound (test-fs-read-stream
    // blocks: {bufferSize,start:1,end:2} → 'yz' not 'yz\n'; {end:1} → 'xy').
    var n = (self.pos !== null)
      ? Math.min(self.end - self.pos + 1, 64 * 1024)
      : Math.min(self.end - self.bytesRead + 1, 64 * 1024);
    if (!(n > 0)) {
      self._readDone = true;
      self.emit('end');
      if (self.autoClose) self.destroy();
      return;
    }
    self._reading = true;
    var fapi = self._fs || fs;
    var buf = Buffer.alloc(n);
    var at = (self.pos !== null) ? self.pos : null;
    var done = function(err, bytesRead) {
      self._reading = false;
      if (self.destroyed) return;
      if (err) {
        if (self.autoClose) self.destroy();
        self.emit('error', err);
        return;
      }
      if (typeof bytesRead !== 'number' || bytesRead <= 0) {
        self._readDone = true;
        self.emit('end');
        if (self.autoClose) self.destroy();
        return;
      }
      self.bytesRead += bytesRead;
      if (self.pos !== null) self.pos += bytesRead;
      var chunk = (bytesRead === n) ? buf : buf.slice(0, bytesRead);
      if (self._encoding) chunk = chunk.toString(self._encoding);
      self.emit('data', chunk);
      if (self.destroyed) return;
      if (self._paused) return; // resume() re-enters _readChunk
      self._readChunk();
    };
    try {
      fapi.read(self.fd, buf, 0, n, at, done);
    } catch (err) {
      self._reading = false;
      if (self.autoClose) self.destroy();
      self.emit('error', err);
    }
  };

  ReadStream.prototype._readLoop = function() {
    var self = this;
    setTimeout(function() {
      if (self.destroyed || self.fd == null || !self._fdOpen) return;
      self._readChunk();
    }, 0);
  };

  ReadStream.prototype.pause = function() {
    this._paused = true;
    return this;
  };

  ReadStream.prototype.resume = function() {
    this._paused = false;
    if (!this.destroyed && !this._readDone) this._readChunk();
    return this;
  };

  ReadStream.prototype.isPaused = function() {
    return !!this._paused;
  };

  ReadStream.prototype.close = function(cb) {
    if (typeof cb === 'function') this.once('close', cb);
    this.destroy();
    return this;
  };

  ReadStream.prototype.destroy = function(err) {
    if (this.destroyed) return this;
    this._destroyed = true;
    this.readable = false;
    if (this.fd != null && this._fdOpen) {
      try {
        if (this._fs && this._fs.close) this._fs.close(this.fd, function() {});
        else fs.closeSync(this.fd);
      } catch (e) {}
      this._fdOpen = false;
    }
    if (err) this.emit('error', err);
    // close fires asynchronously (node: via the fd close callback) so
    // listeners registered inside 'error' handlers still catch it.
    var self = this;
    setTimeout(function() {
      if (!self._closed) { self._closed = true; self.closed = true; self.emit('close'); }
    }, 0);
    return this;
  };

  ReadStream.prototype.pipe = function(dest) {
    this.on('data', function(c) { dest.write(c); });
    this.on('end', function() { dest.end(); });
    return dest;
  };

  // ── WriteStream ────────────────────────────────────────────────────────
  // node face: real open (error surfaces ENOENT), chunks buffered until the
  // fd is live, end() flushes via the fd and finishes. Byte semantics: string
  // chunks are written as UTF-8 (writeSync on a Buffer built per encoding),
  // Buffer/TypedArray chunks raw.
  function WriteStream(path, opts) {
    if (!(this instanceof WriteStream)) return new WriteStream(path, opts);
    EEBase.call(this);
    opts = opts || {};
    this.path = path;
    this.fd = (opts.fd != null) ? opts.fd : null;
    this.flags = opts.flags || 'w';
    this.mode = opts.mode || 438; // 0o666
    this.autoClose = opts.autoClose !== false;
    // node WriteStream own keys = fd,path,flags,mode,flush,start,pos,
    // bytesWritten,_events,... — there is NO own `end` property: the write
    // side has no end-window option, and an own `end` (even `undefined`)
    // SHADOWS the prototype .end() method ("stream.end is not a function",
    // test-fs-write-stream-eagain). node oracle: typeof stream.end stays
    // 'function' with {start,end} options in the constructor call.
    this.start = opts.start;
    this.pos = (typeof opts.start === 'number') ? opts.start : null;
    this.bytesWritten = 0;
    this.writable = true;
    this._destroyed = false;
    // Own `destroyed` accessor — same bao mock.getter rationale as ReadStream.
    Object.defineProperty(this, 'destroyed', {
      get: function() { return this._destroyed; },
      set: function(v) { this._destroyed = v; },
      enumerable: false, configurable: true,
    });
    this.closed = false;
    this._buffer = [];
    this._encodings = [];
    this._ended = false;
    this._finished = false;
    this._fdOpen = this.fd != null;
    this._closed = false;
    // node opts.fs pluggable face: {open, write, close} override the default
    // fs natives for this stream (test-fs-write-stream-eagain passes
    // mustCall-wrapped fns — the calls must route through the pluggable
    // object).
    this._fs = (opts.fs != null && typeof opts.fs === 'object') ? opts.fs : null;
    var self = this;
    if (this.fd === null) {
      this.open();
    } else {
      setTimeout(function() {
        if (self.destroyed) return;
        self.emit('open', self.fd);
        self._flush();
      }, 0);
    }
  }
  WriteStream.prototype = Object.create(EEBase.prototype);
  WriteStream.prototype.constructor = WriteStream;

  // node Writable.prototype accessor face (enumerable: false on node).
  // `destroyed` rides the own per-instance accessor above (bao mock.getter);
  // these two are plain prototype getters like node's.
  Object.defineProperties(WriteStream.prototype, {
    writableEnded: {
      get: function() { return this._ended === true; },
      enumerable: false, configurable: true,
    },
    writableFinished: {
      get: function() { return this._finished === true; },
      enumerable: false, configurable: true,
    },
  });

  // node errorOrDestroy shape: an ERR_STREAM_DESTROYED teardown on a stream
  // whose internal state already died (real destroy()) is a silent no-op; the
  // error reports only when the destroyed face is external (mocked getter —
  // node v24 oracle: mock.getter(stream,'destroyed',()=>true) + end('foo') →
  // 'error' ERR_STREAM_DESTROYED then 'close', never 'finish'). Emitted on
  // the flush seam where node's fs oncomplete detects the destroyed stream.
  function errorOrDestroyWrite(self) {
    if (self.destroyed && !self._destroyed) {
      var e = new Error('Cannot call write after a stream was destroyed');
      e.code = 'ERR_STREAM_DESTROYED';
      if (self.autoClose) self.destroy();
      self.emit('error', e);
    }
  }

  WriteStream.prototype.open = function() {
    var self = this;
    var fapi = self._fs || fs;
    fapi.open(this.path, this.flags, this.mode, function(err, fd) {
      if (self.destroyed) {
        // node fs/streams: an fd landing on a destroyed stream is closed
        // immediately, never adopted.
        (fapi.close || fs.close).call(fapi, fd, function() {});
        return;
      }
      if (err) {
        if (self.autoClose) self.destroy();
        self.emit('error', err);
        return;
      }
      self.fd = fd;
      self._fdOpen = true;
      self.emit('open', fd);
      self.emit('ready');
      self._flush();
    });
  };

  function chunkToBuffer(chunk, enc) {
    if (typeof chunk === 'string') return Buffer.from(chunk, enc || 'utf8');
    if (isObjectArg(chunk)) return Buffer.from(chunk); // Buffer/TypedArray/DataView
    return Buffer.from(String(chunk));
  }

  // Async write (node WriteStream shape): routes through the pluggable
  // opts.fs.write when present (test-fs-write-stream-eagain mocks it), tracks
  // bytesWritten/pos from the fd-acknowledged count, and delivers EAGAIN to
  // the flush chain for the node retry contract.
  WriteStream.prototype._writeOne = function(chunk, enc, cb) {
    var self = this;
    var buf = chunkToBuffer(chunk, enc);
    var fapi = self._fs || fs;
    var at = (self.pos !== null) ? self.pos : null;
    fapi.write(self.fd, buf, 0, buf.length, at, function(err, n) {
      if (!err && typeof n === 'number') {
        self.bytesWritten += n;
        if (self.pos !== null) self.pos += n;
      }
      if (cb) cb(err, n);
    });
  };

  WriteStream.prototype._flush = function(done) {
    var self = this;
    if (self.fd == null || !self._fdOpen || self._buffer.length === 0) {
      if (done) done();
      return;
    }
    var chunk = self._buffer.shift();
    var enc = self._encodings.shift();
    self._writeOne(chunk, enc, function(err) {
      if (self.destroyed) return;
      if (err) {
        if (err.code === 'EAGAIN') {
          // node contract: EAGAIN means try this chunk again after the
          // stream drains (the write fn is invoked a second time —
          // test-fs-write-stream-eagain asserts callCount() === 2 with the
          // file content intact).
          self._buffer.unshift(chunk);
          self._encodings.unshift(enc);
          setTimeout(function() { self._flush(done); }, 0);
          return;
        }
        if (self.autoClose) self.destroy();
        self.emit('error', err);
        return;
      }
      if (self._buffer.length > 0) self._flush(done);
      else {
        self.emit('drain');
        if (done) done();
      }
    });
  };

  WriteStream.prototype.write = function(chunk, enc, cb) {
    if (typeof enc === 'function') { cb = enc; enc = null; }
    if (this.destroyed && !this._destroyed) {
      // node: write after an externally-destroyed face lands in the queue and
      // the fs oncomplete seam reports ERR_STREAM_DESTROYED; ret=false.
      errorOrDestroyWrite(this);
      return false;
    }
    if (this._ended) return false;
    this._buffer.push(chunk);
    this._encodings.push(typeof enc === 'string' ? enc : null);
    // node semantics: bytesWritten counts only bytes actually handed to the
    // fd (`_writeOne`'s writeSync return) — a pre-accumulation here
    // double-counted every chunk (7-byte payload reported 11+).
    if (typeof cb === 'function') {
      var self = this;
      setTimeout(function() { cb.call(self); }, 0);
    }
    this._flush();
    return true;
  };

  WriteStream.prototype.end = function(chunk, enc, cb) {
    if (typeof enc === 'function') { cb = enc; enc = null; }
    if (!this._ended) {
      if (chunk !== undefined && chunk !== null) {
        this._buffer.push(chunk);
        this._encodings.push(typeof enc === 'string' ? enc : null);
      }
      this._ended = true;
      this.writable = false;
    }
    if (typeof cb === 'function') this.once('finish', cb);
    var self = this;
    // Defer the final flush so a constructor-registered 'open' listener
    // (and any pending open callback) lands first — node ordering.
    setTimeout(function() { self._finalFlush(); }, 0);
    return this;
  };

  WriteStream.prototype._finalFlush = function() {
    var self = this;
    if (self.destroyed) {
      // test-fs-write-stream-eagain cell 2: end('foo') on a destroyed face →
      // the ERR_STREAM_DESTROYED seam above; 'finish' must NOT fire (finished()
      // rejects via 'error'). A real destroy() already absorbed the stream.
      errorOrDestroyWrite(self);
      return;
    }
    if (self.fd == null && !self._fdOpen) {
      // open() still in flight — finish after the fd lands (node: end()
      // before open completion still writes the buffered chunks).
      self.once('open', function() { self._finalFlush(); });
      return;
    }
    self._flush(function() {
      if (self.destroyed) return;
      self._finished = true;
      self.emit('finish');
      if (self.autoClose) self.destroy();
    });
  };

  WriteStream.prototype.close = function(cb) {
    if (typeof cb === 'function') this.once('close', cb);
    this.destroy();
    return this;
  };

  WriteStream.prototype.destroy = function(err) {
    var wasDestroyed = this._destroyed;
    this._destroyed = true;
    this.writable = false;
    if (this.fd != null && this._fdOpen) {
      try {
        if (this._fs && this._fs.close) this._fs.close(this.fd, function() {});
        else fs.closeSync(this.fd);
      } catch (e) {}
      this._fdOpen = false;
    }
    if (err && !wasDestroyed) this.emit('error', err);
    var self = this;
    setTimeout(function() {
      if (!self._closed) { self._closed = true; self.closed = true; self.emit('close'); }
    }, 0);
    return this;
  };

  // ── fd-backed factories (FileHandle.createReadStream/createWriteStream) ─
  function makeFdReadStream(fd, opts) {
    opts = opts || {};
    opts.fd = fd;
    return new ReadStream(null, opts);
  }
  function makeFdWriteStream(fd, opts) {
    opts = opts || {};
    opts.fd = fd;
    return new WriteStream(null, opts);
  }

  return {
    createReadStream: function(path, opts) { return new ReadStream(path, opts); },
    createWriteStream: function(path, opts) { return new WriteStream(path, opts); },
    ReadStream: ReadStream,
    WriteStream: WriteStream,
    makeFdReadStream: makeFdReadStream,
    makeFdWriteStream: makeFdWriteStream,
    // read-family options classifier (parse_read_window_args): node's
    // validateObject gates are typeof + Array.isArray — both proxy-transparent
    // (Array.isArray unwraps proxy targets per spec, invoking no traps). SM's
    // IsArrayObject/IsFunctionObject are class checks blind to proxies, so the
    // Rust parser calls this (via gc_store "fsinternal") whenever JS_IsProxy
    // gates the options slot. Only 'array'/'function'/'other'.
    readOptionsClassify: function(v) {
      if (Array.isArray(v)) return 'array';
      if (typeof v === 'function') return 'function';
      return 'other';
    }
  };
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
            // node-parity completion (SHAPE_missing_api family):
            (c"chownSync", fs_chown_sync, 3)
            (c"utimesSync", fs_utimes_sync, 3)
            (c"lutimesSync", fs_lutimes_sync, 3)
            (c"mkdtempDisposableSync", fs_mkdtemp_disposable_sync, 1)
            (c"_toUnixTimestamp", fs_to_unix_timestamp, 1)
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

        // Constants — access modes + the open-flag bits node exposes on
        // fs.constants (test-fs-open-flags destructures O_* from there).
        let constants: &[(&str, i32)] = &[
            ("F_OK", 0),
            ("R_OK", 4),
            ("W_OK", 2),
            ("X_OK", 1),
            ("O_RDONLY", 0),
            ("O_WRONLY", 1),
            ("O_RDWR", 2),
            ("O_CREAT", 0x40),
            ("O_EXCL", 0x80),
            ("O_TRUNC", 0x200),
            ("O_APPEND", 0x400),
            ("O_SYNC", 0x101000),
            ("O_DIRECTORY", 0x10000),
            ("O_NOFOLLOW", 0x20000),
            ("S_IFMT", 0xF000),
            ("S_IFDIR", 0x4000),
            ("S_IFREG", 0x8000),
            ("S_IFLNK", 0xA000),
        ];
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
                (c"cp", fs_promises_cp, 2)
                (c"cpSync", fs_promises_cp_sync, 2)
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
                (c"mkdtempDisposable", fs_promises_mkdtemp_disposable, 1)
                (c"watch", fs_promises_watch, 2)
                (c"open", fs_promises_open, 2)
                (c"read", fs_promises_read, 4)
                (c"write", fs_promises_write, 4)
                (c"statfs", fs_promises_statfs, 1)
                // fs.promises.glob — node v22+ face: returns a Promise that
                // resolves to an AsyncIterable of matches (the corpus iterates
                // `for await (const item of asyncGlob(pattern, opts))`).
                (c"glob", fs_promises_glob, 3)
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

        // realpathSync.native / realpath.native — node exposes the native
        // realpath binding as a property of the realpath functions.
        let realpath_ops: [(&str, JSNative); 2] = [
            ("realpathSync", Some(fs_realpath_native_sync)),
            ("realpath", Some(fs_realpath_native)),
        ];
        for (base, native_fn) in realpath_ops {
            let Some(native_fn) = native_fn else { continue };
            let c_base = ZBox::from_bytes(base.as_bytes());
            let mut base_val = UndefinedValue();
            JS_GetProperty(
                cx.raw_cx(),
                fs_obj.handle().into(),
                c_base.as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut base_val,
                },
            );
            if base_val.is_object() {
                rooted!(&in(cx) let base_obj = base_val.to_object());
                let native_f = JS_NewFunction(cx.raw_cx(), Some(native_fn), 1, 0, c"native".as_ptr());
                if !native_f.is_null() {
                    let native_obj = JS_GetFunctionObject(native_f);
                    rooted!(&in(cx) let native_val = mozjs::jsval::ObjectValue(native_obj));
                    JS_DefineProperty(
                        cx.raw_cx(),
                        base_obj.handle().into(),
                        c"native".as_ptr(),
                        native_val.handle().into(),
                        (JSPROP_ENUMERATE | JSPROP_READONLY) as u32,
                    );
                }
            }
        }

        // fs.Dir — the opendir() result class (instanceof target; the shared
        // prototype carries the iteration face).
        let (dir_ctor, _) = ensure_dir_class(cx.raw_cx());
        if !dir_ctor.is_null() {
            rooted!(&in(cx) let dir_val = mozjs::jsval::ObjectValue(dir_ctor));
            JS_DefineProperty(
                cx.raw_cx(),
                fs_obj.handle().into(),
                c"Dir".as_ptr(),
                dir_val.handle().into(),
                (JSPROP_ENUMERATE | JSPROP_READONLY) as u32,
            );
        }

        // fs.Dirent — re-exported from internal/fs/utils (the same class the
        // readdir({withFileTypes}) face instances). node v24.19.0 oracle:
        // `dirent instanceof fs.Dirent` holds for every withFileTypes entry;
        // the ESM named-export bridge enumerates the fs object's own string
        // keys, so mounting the class here is also what makes
        // `import { Dirent } from 'node:fs'` resolve (test-fs-glob:
        // "<builtin:fs> doesn't provide an export named 'Dirent'").
        // Mounted at the END of install (after fs + fs/promises are cached):
        // the internal/fs module set evaluates `require('fs/promises')` at
        // eval time, and mounting earlier would re-enter the fs install.
    }

    // Evaluate createReadStream/createWriteStream polyfill
    unsafe {
        install_fs_stream_polyfill(cx, fs_obj.handle());
    }

    // util.promisify custom faces for the read family (node's
    // customPromisifyArgs contract expressed through the in-tree
    // promisify.custom channel — the same wiring dns uses; util.promisify
    // consults orig[Symbol.for('nodejs.util.promisify.custom')] FIRST, so
    // promisify(fs.readv)(fd, bufs) resolves {bytesRead, buffers}).
    unsafe {
        install_fs_promisify_customs(cx, fs_obj.handle());
    }

    cache_builtin(cx, "fs", fs_obj.get());

    // Register fs/promises sub-path — reuses the same promise-based methods
    // already defined on fs.promises. The sub-path module `require("fs/promises")`
    // gets its own top-level builtin with the promise methods directly on it.
    unsafe {
        install_fs_promises_subpath(cx);
    }

    // fs.Dirent mount (post-cache: see the comment above).
    unsafe {
        install_fs_dirent_export(cx, fs_obj.handle());
    }
}

/// Mount `fs.Dirent` from internal/fs/utils (the canonical Dirent class the
/// readdir/withFileTypes face instances). Requires `fs` and `fs/promises` to
/// already be cached — the internal module set requires them at eval time.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn install_fs_dirent_export(
    cx: &mut mozjs::context::JSContext,
    fs_obj: mozjs::rust::Handle<'_, *mut JSObject>,
) {
    if crate::node_internal_fs_modules::ensure_installed(cx.raw_cx()) {
        if let Some(utils_mod) = crate::require::get_builtin(cx.raw_cx(), "internal/fs/utils") {
            let mut dirent_val = UndefinedValue();
            rooted!(&in(cx) let utils_root = utils_mod);
            JS_GetProperty(
                cx.raw_cx(),
                utils_root.handle().into(),
                c"Dirent".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut dirent_val,
                },
            );
            if dirent_val.is_object() {
                rooted!(&in(cx) let dirent_root = dirent_val);
                JS_DefineProperty(
                    cx.raw_cx(),
                    fs_obj.into(),
                    c"Dirent".as_ptr(),
                    dirent_root.handle().into(),
                    (JSPROP_ENUMERATE | JSPROP_READONLY) as u32,
                );
            }
        }
    } else {
        // The internal layer failed to evaluate — surface the pending
        // exception rather than silently omitting the export.
        JS_ClearPendingException(cx.raw_cx());
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
                (c"cp", fs_promises_cp, 2)
                (c"cpSync", fs_promises_cp_sync, 2)
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
                (c"mkdtempDisposable", fs_promises_mkdtemp_disposable, 1)
                (c"watch", fs_promises_watch, 2)
                (c"open", fs_promises_open, 2)
                (c"read", fs_promises_read, 4)
                (c"write", fs_promises_write, 4)
                (c"statfs", fs_promises_statfs, 1)
                // fs.promises.glob — node v22+ face (see fs.promises table).
                (c"glob", fs_promises_glob, 3)

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

            // fd-stream factories for FileHandle.createReadStream/createWriteStream
            // (fh_create_stream_impl looks them up in gc_store "fsinternal")
            // plus the read-family options classifier (parse_read_window_args
            // consumes it through the same namespace).
            for factory in &["makeFdReadStream", "makeFdWriteStream", "readOptionsClassify"] {
                let cname = ZBox::from_bytes(factory.as_bytes());
                let mut fval = UndefinedValue();
                JS_GetProperty(
                    cx.raw_cx(),
                    exports_rooted.handle().into(),
                    cname.as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut fval,
                    },
                );
                if fval.is_object() {
                    crate::gc_store::gc_store_insert_ns(
                        cx.raw_cx(),
                        "fsinternal",
                        factory,
                        fval.to_object(),
                    );
                }
            }

            for name in &["createReadStream", "createWriteStream", "ReadStream", "WriteStream"] {
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
}

/// util.promisify customs for the callback read family (node stamps
/// fs.read/fs.readv with the internal customPromisifyArgs symbol so
/// `promisify(fs.readv)(fd, bufs)` resolves `{bytesRead, buffers}` and
/// `promisify(fs.read)(fd, buf, …)` resolves `{bytesRead, buffer}` —
/// node v24.19.0 oracle). bao expresses the same contract through the
/// promisify.custom channel (deviation noted: the symbol is the registered
/// `Symbol.for('nodejs.util.promisify.custom')` rather than node's private
/// customPromisifyArgs — same consumption point inside util.promisify, same
/// observable promise shape; the dns face already uses this channel).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn install_fs_promisify_customs(
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
        c"__fs_promisify_ref".as_ptr(),
        fs_val.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    const FS_PROMISIFY_JS: &str = r#"(function() {
  var fs = globalThis.__fs_promisify_ref;
  var kProm = Symbol.for('nodejs.util.promisify.custom');
  function stampCustom(fn, custom) {
    try { Object.defineProperty(fn, kProm, { value: custom, configurable: true }); } catch (e) {}
  }
  // readv(fd, buffers[, position]) → Promise<{bytesRead, buffers}>
  if (typeof fs.readv === 'function' && fs.readv[kProm] === undefined) {
    stampCustom(fs.readv, function(fd, buffers, position) {
      return new Promise(function(resolve, reject) {
        fs.readv(fd, buffers, position, function(err, bytesRead, bufs) {
          if (err) reject(err); else resolve({ bytesRead: bytesRead, buffers: bufs });
        });
      });
    });
  }
  // read(fd, buffer[, offset[, length[, position]]]) → Promise<{bytesRead, buffer}>
  if (typeof fs.read === 'function' && fs.read[kProm] === undefined) {
    stampCustom(fs.read, function(fd, buffer, offset, length, position) {
      return new Promise(function(resolve, reject) {
        fs.read(fd, buffer, offset, length, position, function(err, bytesRead, buf) {
          if (err) reject(err); else resolve({ bytesRead: bytesRead, buffer: buf });
        });
      });
    });
  }
})();"#;
    let c_filename = ZBox::from_bytes("node:fs:promisify".as_bytes());
    let opts = NewCompileOptions(cx.raw_cx(), c_filename.as_ptr(), 1);
    if !opts.is_null() {
        let mut src = mozjs::rust::transform_str_to_source_text(FS_PROMISIFY_JS);
        let mut rval = UndefinedValue();
        let rval_handle = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        };
        let ok = mozjs_sys::jsapi::JS::Evaluate2(cx.raw_cx(), opts, &mut src, rval_handle);
        mozjs::glue::DeleteCompileOptions(opts);
        if !ok {
            JS_ClearPendingException(cx.raw_cx());
        }
    }

    JS_DeleteProperty1(
        cx.raw_cx(),
        global_rooted.handle().into(),
        c"__fs_promisify_ref".as_ptr(),
    );
}

// --- Argument helpers ---

#[allow(unsafe_op_in_unsafe_fn)]
// ── path argument resolution (node parity: string | Buffer | URL) ────────────
// @trace REQ-ENG-004 — the ARG_TYPE_invalid family root: every fs face took
// only `string` paths. node v24.19.0 oracle (2026-10-25): paths flow through
// one validator accepting string | Buffer/TypedArray | URL(file:) with the
// exact ERR_INVALID_ARG_TYPE / ERR_INVALID_URL_SCHEME surfaces
// (`accessSync(true)` → `The "path" argument must be of type string or an
// instance of Buffer or URL. Received type boolean (true)`; `readFileSync(new
// URL('http://x/'))` → ERR_INVALID_URL_SCHEME `The URL must be of scheme
// file`). One shared resolver = every fs function inherits the fix (BCE
// single-point root; ~40 call sites).

/// Throw a Node-shaped coded error: JS_ReportErrorUTF8 then brand the pending
/// exception with `code`/`name` (same attach discipline as throw_fs_error).
unsafe fn throw_fs_coded_error(cx: *mut JSContext, code: &str, name: &str, msg: &str) -> bool {
    let c_msg = ZBox::from_bytes(msg.as_bytes());
    JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
    if JS_IsExceptionPending(cx) {
        rooted!(in(cx) let mut exn = UndefinedValue());
        JS_GetPendingException(cx, exn.handle_mut().into());
        let exn_val = exn.get();
        if !exn_val.is_undefined() && exn_val.is_object() {
            rooted!(in(cx) let exn_obj = exn_val.to_object());
            let code_str = JS_NewStringCopyZ(cx, ZBox::from_bytes(code.as_bytes()).as_ptr());
            if !code_str.is_null() {
                rooted!(in(cx) let code_val = StringValue(&*code_str));
                JS_DefineProperty(
                    cx,
                    exn_obj.handle().into(),
                    c"code".as_ptr(),
                    code_val.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
            let name_str = JS_NewStringCopyZ(cx, ZBox::from_bytes(name.as_bytes()).as_ptr());
            if !name_str.is_null() {
                rooted!(in(cx) let name_val = StringValue(&*name_str));
                JS_DefineProperty(
                    cx,
                    exn_obj.handle().into(),
                    c"name".as_ptr(),
                    name_val.handle().into(),
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
    false
}

// ─── CLI `--permission` fs.read enforcement (ERR_ACCESS_DENIED) ─────────────
// @trace REQ-ENG-007 [criterion:7]
//
// node v24 --permission enforcement face (oracle: `node --permission
// --allow-fs-read=/tmp -e "fs.readdirSync('/etc')"` → ERR_ACCESS_DENIED,
// message "Access to this API has been restricted. Use --allow-fs-read to
// manage permissions."). Gates every fs read-family entry point on the
// process-level CliPermissionModel (installed by CLI dispatch on --permission);
// model absent (no --permission) → zero behavioral change.
//
// Glob family exemption: test-fs-glob-no-follow-symlink-globstar's
// `assert.throws(() => fs.readdirSync(outside), {code:'ERR_ACCESS_DENIED'})`
// targets the fs.readdirSync face only — the glob engine's own containment
// (no-follow-symlink + root pinning) is an engine-level invariant, not an
// fs-gate concern. fs.glob/globSync/fs.promises.glob stay ungated here.

/// node --permission ERR_ACCESS_DENIED message (oracle v24.19.0 verbatim).
const CLI_PERM_DENIED_MSG: &str =
    "Access to this API has been restricted. Use --allow-fs-read to manage permissions.";

/// fs.read gate for **sync** entry points: model absent → zero change; path
/// outside every `--allow-fs-read` grant → throw ERR_ACCESS_DENIED.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn cli_perm_read_gate_sync(cx: *mut JSContext, path: &str) -> bool {
    if crate::permission_bridge::cli_permission_model().is_none() {
        return true;
    }
    if crate::permission_bridge::cli_permission_has("fs.read", Some(path)) {
        return true;
    }
    throw_fs_coded_error(cx, "ERR_ACCESS_DENIED", "Error", CLI_PERM_DENIED_MSG)
}

/// fs.read gate for **callback-style async** entry points: denial routes
/// through the callback as `callback(err)` — node oracle: no sync throw.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn cli_perm_read_gate_callback(
    cx: *mut JSContext,
    args: &CallArgs,
    cb_index: u32,
    path: &str,
) -> bool {
    if crate::permission_bridge::cli_permission_model().is_none() {
        return true;
    }
    if crate::permission_bridge::cli_permission_has("fs.read", Some(path)) {
        return true;
    }
    if let Some((callback, _)) = extract_callback_and_encoding(cx, args, cb_index) {
        let mut wrapped_cx =
            mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        let err_obj = unsafe { JS_NewPlainObject(cx) };
        if !err_obj.is_null() {
            rooted!(&in(cx_ref) let err_rooted = err_obj);
            let msg_str = JS_NewStringCopyZ(cx, ZBox::from_bytes(CLI_PERM_DENIED_MSG.as_bytes()).as_ptr());
            if !msg_str.is_null() {
                rooted!(&in(cx_ref) let msg_val = StringValue(&*msg_str));
                JS_DefineProperty(
                    cx,
                    err_rooted.handle().into(),
                    c"message".as_ptr(),
                    msg_val.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
            let code_str = JS_NewStringCopyZ(cx, ZBox::from_bytes("ERR_ACCESS_DENIED".as_bytes()).as_ptr());
            if !code_str.is_null() {
                rooted!(&in(cx_ref) let code_val = StringValue(&*code_str));
                JS_DefineProperty(
                    cx,
                    err_rooted.handle().into(),
                    c"code".as_ptr(),
                    code_val.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
            let path_str = JS_NewStringCopyZ(cx, ZBox::from_bytes(path.as_bytes()).as_ptr());
            if !path_str.is_null() {
                rooted!(&in(cx_ref) let path_val = StringValue(&*path_str));
                JS_DefineProperty(
                    cx,
                    err_rooted.handle().into(),
                    c"path".as_ptr(),
                    path_val.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
            rooted!(&in(cx_ref) let err_val = mozjs::jsval::ObjectValue(err_rooted.get()));
            rooted!(&in(cx_ref) let undefined_val = UndefinedValue());
            rooted!(&in(cx_ref) let cb_rooted = callback);
            rooted!(&in(cx_ref) let cb_val = mozjs::jsval::ObjectValue(cb_rooted.get()));
            rooted!(&in(cx_ref) let this_val = UndefinedValue());
            rooted!(&in(cx_ref) let argv_rooted = mozjs_sys::jsgc::ValueArray::<2>::new([
                err_val.handle().get(),
                undefined_val.handle().get(),
            ]));
            let argv = mozjs::jsapi::JS::HandleValueArray::from(&argv_rooted);
            // SAFETY: `argv` borrows the GC-traced storage of the scope-rooted
            // `argv_rooted` ValueArray for the duration of this single engine
            // call; the callback and `this` are rooted as Values (node async
            // error-back calls the callback with `undefined` this).
            rooted!(&in(cx_ref) let mut ret_val = UndefinedValue());
            unsafe {
                w2::Call(
                    cx_ref,
                    this_val.handle(),
                    cb_val.handle(),
                    &argv,
                    ret_val.handle_mut(),
                )
            };
            args.rval().set(ret_val.get());
        }
        true
    } else {
        // No callback: node --permission denies via sync throw even on async API
        // surfaces when no callback is registered (promise-style call).
        throw_fs_coded_error(cx, "ERR_ACCESS_DENIED", "Error", CLI_PERM_DENIED_MSG)
    }
}

/// fs.read gate for **fs.promises.** entry points: denial rejects the promise
/// with ERR_ACCESS_DENIED (never a sync throw; node oracle). Callers create
/// the promise BEFORE invoking this gate and set rval on exit.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn cli_perm_read_gate_promise(
    cx: *mut JSContext,
    promise: *mut JSObject,
    path: &str,
) -> bool {
    if crate::permission_bridge::cli_permission_model().is_none() {
        return true;
    }
    if crate::permission_bridge::cli_permission_has("fs.read", Some(path)) {
        return true;
    }
    reject_fs_error_coded(cx, promise, path);
    false
}

/// Promise rejection helper: ERR_ACCESS_DENIED + path on the rejection reason.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn reject_fs_error_coded(cx: *mut JSContext, promise: *mut JSObject, path: &str) {
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let err_obj = JS_NewPlainObject(cx));
    if !err_obj.get().is_null() {
        let msg_str = JS_NewStringCopyZ(cx, ZBox::from_bytes(CLI_PERM_DENIED_MSG.as_bytes()).as_ptr());
        if !msg_str.is_null() {
            rooted!(&in(cx_ref) let msg_val = StringValue(&*msg_str));
            JS_DefineProperty(
                cx,
                err_obj.handle().into(),
                c"message".as_ptr(),
                msg_val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
        let code_str = JS_NewStringCopyZ(cx, ZBox::from_bytes("ERR_ACCESS_DENIED".as_bytes()).as_ptr());
        if !code_str.is_null() {
            rooted!(&in(cx_ref) let code_val = StringValue(&*code_str));
            JS_DefineProperty(
                cx,
                err_obj.handle().into(),
                c"code".as_ptr(),
                code_val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
        let path_str = JS_NewStringCopyZ(cx, ZBox::from_bytes(path.as_bytes()).as_ptr());
        if !path_str.is_null() {
            rooted!(&in(cx_ref) let path_val = StringValue(&*path_str));
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

/// fs.read gate for the glob family: scoped root containment. When the
/// CLI --permission model is active, every resolved glob result must stay
/// inside a granted tree; any escaped path short-circuits the walk with
/// ERR_ACCESS_DENIED. The walk itself is engine-containment (no-follow +
/// root pinning), not a per-entry fs gate.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn cli_perm_glob_root_gate(cx: *mut JSContext, root: &str) -> bool {
    if crate::permission_bridge::cli_permission_model().is_none() {
        return true;
    }
    if crate::permission_bridge::cli_permission_has("fs.read", Some(root)) {
        return true;
    }
    throw_fs_coded_error(cx, "ERR_ACCESS_DENIED", "Error", CLI_PERM_DENIED_MSG)
}

/// node's `Received ...` fragment of ERR_INVALID_ARG_TYPE messages for the
/// tested value shapes (oracle-verbatim: undefined/null/`type boolean (true)`/
/// `type number (42)`/`an instance of Array`/`an instance of Object`/
/// `function `).
unsafe fn received_fs_desc(cx: *mut JSContext, v: JSVal) -> ::std::string::String {
    if v.is_undefined() {
        "undefined".to_string()
    } else if v.is_null() {
        "null".to_string()
    } else if v.is_boolean() {
        format!("type boolean ({})", v.to_boolean())
    } else if v.is_int32() {
        format!("type number ({})", v.to_int32())
    } else if v.is_double() {
        format!("type number ({})", v.to_double())
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
    } else if v.is_object() {
        rooted!(in(cx) let obj = v.to_object());
        let mut is_fn = false;
        if unsafe { mozjs_sys::jsapi::js::IsFunctionObject(obj.get()) } {
            is_fn = true;
        }
        if is_fn {
            return "function ".to_string();
        }
        let mut is_arr = false;
        rooted!(in(cx) let arr_val_root = v);
        let ok = IsArrayObject(
            cx,
            arr_val_root.handle().into(),
            &mut is_arr,
        );
        if ok && is_arr {
            "an instance of Array".to_string()
        } else {
            "an instance of Object".to_string()
        }
    } else if v.is_symbol() || v.is_bigint() {
        // node shapes: 'type symbol (Symbol(1))' / 'type bigint (42n)'. SM
        // ToString throws on symbols (bun_inspect_api determineSpecificType
        // precedent: null string → clear + the bare type form).
        let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        let cx_ref_sym = &mut wrapped;
        rooted!(&in(cx_ref_sym) let v_root = v);
        let jsstr = unsafe { mozjs::rust::ToString(cx_ref_sym, v_root.handle()) };
        if !jsstr.is_null() {
            let s = crate::jsstr_to_rust_string(cx, jsstr);
            if v.is_bigint() {
                format!("type bigint ({}n)", s)
            } else {
                format!("type symbol ({})", s)
            }
        } else {
            JS_ClearPendingException(cx);
            if v.is_bigint() {
                "type bigint".to_string()
            } else {
                "type symbol".to_string()
            }
        }
    } else {
        "an unexpected value".to_string()
    }
}

/// URL value → filesystem path (file: scheme only, node parity). Non-file
/// scheme → ERR_INVALID_URL_SCHEME. Returns Err(false) when the exception is
/// already pending, Err(true) when the value is not URL-like (caller falls
/// through to the generic invalid-type throw).
unsafe fn url_val_to_fs_path(cx: *mut JSContext, v: JSVal) -> ::std::result::Result<::std::string::String, bool> {
    if !v.is_object() {
        return ::std::result::Result::Err(true);
    }
    rooted!(in(cx) let obj = v.to_object());
    let mut href = UndefinedValue();
    JS_GetProperty(
        cx,
        obj.handle().into(),
        c"href".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut href,
        },
    );
    if !href.is_string() {
        // Not URL-shaped — let the caller decide the error surface.
        return ::std::result::Result::Err(true);
    }
    let href_s = href.to_string();
    if href_s.is_null() {
        return ::std::result::Result::Err(true);
    }
    let url = crate::jsstr_to_rust_string(cx, href_s);
    if !url.starts_with("file:") {
        return ::std::result::Result::Err(throw_fs_coded_error(
            cx,
            "ERR_INVALID_URL_SCHEME",
            "TypeError",
            "The URL must be of scheme file",
        ));
    }
    // Same conversion core as Bun.fileURLToPath (bun_url WHATWG parser +
    // byte-level percent decode; the leading offset skips the `file:` scheme
    // the parser accepts both with and without authority).
    let bun_str = bun_core::String::borrow_utf8(url.as_bytes());
    let result = bun_url::whatwg::path_from_file_url(&bun_str);
    if result.tag() == bun_core::Tag::Dead {
        // Degraded URL text (e.g. `file:///a%20b` unparsable upstream) — the
        // raw pathname after the host part, percent-decoded.
        let raw = url
            .strip_prefix("file://")
            .map(|rest| match rest.find('/') {
                Some(i) => rest[i..].to_string(),
                None => ::std::string::String::new(),
            })
            .unwrap_or_else(|| url.clone());
        return ::std::result::Result::Ok(
            ::std::string::String::from_utf8_lossy(&bun_url::whatwg::percent_decode(raw.as_bytes()))
                .into_owned(),
        );
    }
    let utf8 = result.to_utf8();
    let decoded = ::std::string::String::from_utf8_lossy(&bun_url::whatwg::percent_decode(
        ::std::str::from_utf8(utf8.slice()).unwrap_or("").as_bytes(),
    ))
    .into_owned();
    ::std::result::Result::Ok(decoded)
}

/// node path validator: string | Buffer/TypedArray bytes | URL(file:).
/// Everything else throws ERR_INVALID_ARG_TYPE with the node-exact message.
unsafe fn resolve_fs_path_value(
    cx: *mut JSContext,
    val: JSVal,
    arg_name: &str,
) -> ::std::result::Result<::std::string::String, bool> {
    if val.is_string() {
        let s = val.to_string();
        if !s.is_null() {
            return ::std::result::Result::Ok(crate::jsstr_to_rust_string(cx, s));
        }
    }
    if val.is_object() {
        // Buffer / TypedArray path: raw bytes (Linux paths are byte strings).
        if let ::std::option::Option::Some((ptr, len)) = view_byte_slot(val) {
            if !ptr.is_null() && len > 0 {
                let bytes = unsafe { ::std::slice::from_raw_parts(ptr, len) };
                return ::std::result::Result::Ok(::std::string::String::from_utf8_lossy(bytes).into_owned());
            }
            // Zero-length Buffer is a valid (empty) path per node — falls
            // through to the OS layer which fails with ENOENT.
            return ::std::result::Result::Ok(::std::string::String::new());
        }
        return url_val_to_fs_path(cx, val);
    }
    let msg = format!(
        "The \"{}\" argument must be of type string or an instance of Buffer or URL. Received {}",
        arg_name,
        received_fs_desc(cx, val)
    );
    ::std::result::Result::Err(throw_fs_coded_error(
        cx,
        "ERR_INVALID_ARG_TYPE",
        "TypeError",
        &msg,
    ))
}

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
    resolve_fs_path_value(cx, val, "path")
}

/// write/append family target: node accepts a file descriptor number in place
/// of the path (`writeFileSync(fd, ...)` writes at the fd's current position
/// without truncating — node v24 oracle; test-fs-writefile-with-fd).
#[derive(::std::fmt::Debug)]
enum FsWriteTarget {
    Path(::std::string::String),
    Fd(i32),
}

unsafe fn get_write_target_arg(
    cx: *mut JSContext,
    args: &CallArgs,
    index: u32,
) -> ::std::result::Result<FsWriteTarget, bool> {
    if args.argc_ <= index {
        JS_ReportErrorUTF8(cx, c"Missing path argument".as_ptr());
        return ::std::result::Result::Err(false);
    }
    let val = *args.get(index).ptr;
    if val.is_int32() {
        return ::std::result::Result::Ok(FsWriteTarget::Fd(val.to_int32()));
    }
    if val.is_double() {
        // node truncates non-integer fds? No — a double fd is ERR_INVALID_ARG_TYPE
        // unless it is an integral value (5.0).
        let d = val.to_double();
        if d.fract() == 0.0 && d >= 0.0 && d <= i32::MAX as f64 {
            return ::std::result::Result::Ok(FsWriteTarget::Fd(d as i32));
        }
    }
    resolve_fs_path_value(cx, val, "path").map(FsWriteTarget::Path)
}

/// Read-family target: node readFile/readFileSync accept an integer file
/// descriptor in place of a path (test-fs-readfile-fd: readFile(6, 'utf8', …))
/// — fd reads start at the fd's CURRENT position and never rewind.
#[derive(::std::fmt::Debug)]
enum FsReadTarget {
    Path(::std::string::String),
    Fd(i32),
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn get_read_target_arg(
    cx: *mut JSContext,
    args: &CallArgs,
    index: u32,
) -> ::std::result::Result<FsReadTarget, bool> {
    if args.argc_ <= index {
        JS_ReportErrorUTF8(cx, c"Missing path argument".as_ptr());
        return ::std::result::Result::Err(false);
    }
    let val = *args.get(index).ptr;
    if val.is_int32() {
        return ::std::result::Result::Ok(FsReadTarget::Fd(val.to_int32()));
    }
    if val.is_double() {
        let d = val.to_double();
        if d.is_finite() && d.fract() == 0.0 && d >= 0.0 && d <= i32::MAX as f64 {
            return ::std::result::Result::Ok(FsReadTarget::Fd(d as i32));
        }
    }
    resolve_fs_path_value(cx, val, "path").map(FsReadTarget::Path)
}

/// Read to EOF from an fd's CURRENT position (node readFile(fd) semantics).
#[cfg(unix)]
fn read_fd_to_eof(fd: i32) -> ::std::io::Result<Vec<u8>> {
    let mut out = ::std::vec::Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let n = unsafe {
            libc::read(fd, chunk.as_mut_ptr() as *mut ::std::ffi::c_void, chunk.len())
        };
        if n < 0 {
            let err = ::std::io::Error::last_os_error();
            if err.raw_os_error() == ::std::option::Option::Some(libc::EINTR) {
                continue;
            }
            return ::std::result::Result::Err(err);
        }
        if n == 0 {
            break;
        }
        out.extend_from_slice(&chunk[..n as usize]);
    }
    ::std::result::Result::Ok(out)
}

#[cfg(not(unix))]
fn read_fd_to_eof(_fd: i32) -> ::std::io::Result<Vec<u8>> {
    ::std::result::Result::Err(::std::io::Error::other("fd reads are unix-only"))
}

/// readFileSync with an explicit node flag string (default 'r'; 'a+' opens
/// create+append+read so reading a not-yet-existing append target yields ''
/// with mode 0o666&~umask — test-fs-read-file-sync). Unknown flags fail
/// loudly (EINVAL family) instead of silently degrading to 'r'.
fn read_file_with_flag(path: &str, flag: &str) -> ::std::io::Result<Vec<u8>> {
    use ::std::io::Read;
    let mut opts = fs::OpenOptions::new();
    match flag {
        "r" | "rs" => {
            opts.read(true);
        }
        "r+" | "rs+" | "sr+" => {
            opts.read(true).write(true);
        }
        "w+" | "wx" | "wx+" => {
            opts.write(true).read(true).truncate(true);
            if flag.starts_with("wx") {
                opts.create_new(true);
            } else {
                opts.create(true);
            }
        }
        "a+" | "ax+" | "xa+" | "as+" | "sa+" => {
            opts.read(true).append(true).create(true);
        }
        // Write-only flags: open succeeds (creating when asked) and the read
        // syscall reports EBADF like node.
        "w" | "a" | "ax" | "xa" | "as" | "sa" => {
            opts.write(true).create(true);
            if flag.starts_with("wx") || flag.starts_with("ax") {
                opts.create_new(true);
            }
        }
        _ => {
            return ::std::result::Result::Err(::std::io::Error::new(
                ::std::io::ErrorKind::InvalidInput,
                format!("Unknown file open flag: {}", flag),
            ));
        }
    }
    let mut file = opts.open(path)?;
    let mut out = ::std::vec::Vec::new();
    file.read_to_end(&mut out)?;
    ::std::result::Result::Ok(out)
}

/// Write all bytes to an already-open fd at its current position (node
/// writeFile/appendFile fd semantics: write(2) loop, NO truncate, NO flag).
fn write_bytes_to_fd(fd: i32, bytes: &[u8]) -> ::std::io::Result<usize> {
    #[cfg(unix)]
    {
        let mut written = 0usize;
        // SAFETY: fd is caller-owned and live for the duration of the call.
        while written < bytes.len() {
            let n = unsafe {
                libc::write(
                    fd,
                    bytes[written..].as_ptr() as *const ::std::ffi::c_void,
                    bytes.len() - written,
                )
            };
            if n < 0 {
                let err = ::std::io::Error::last_os_error();
                if err.raw_os_error() == ::std::option::Option::Some(libc::EINTR) {
                    continue;
                }
                return ::std::result::Result::Err(err);
            }
            if n == 0 {
                break;
            }
            written += n as usize;
        }
        ::std::result::Result::Ok(written)
    }
    #[cfg(not(unix))]
    {
        ::std::result::Result::Err(::std::io::Error::other("fd writes are unix-only"))
    }
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

/// node getValidatedFd for the fd-void op family (fsync/fdatasync/close and
/// their Sync faces): an integral number passes through (a later syscall on a
/// bad-but-integral fd reports EBADF like node); any non-number throws
/// ERR_INVALID_ARG_TYPE ('The "fd" argument must be of type number. Received …').
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn fs_validated_fd(cx: *mut JSContext, v: JSVal) -> ::std::result::Result<i32, bool> {
    if v.is_int32() {
        return ::std::result::Result::Ok(v.to_int32());
    }
    if v.is_double() {
        let d = v.to_double();
        if d.is_finite() && d.fract() == 0.0 && d >= i32::MIN as f64 && d <= i32::MAX as f64 {
            return ::std::result::Result::Ok(d as i32);
        }
    }
    let msg = format!(
        "The \"fd\" argument must be of type number. Received {}",
        received_fs_desc(cx, v)
    );
    ::std::result::Result::Err(throw_fs_coded_error(
        cx,
        "ERR_INVALID_ARG_TYPE",
        "TypeError",
        &msg,
    ))
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
    let target = match get_read_target_arg(cx, &args, 0) {
        ::std::result::Result::Ok(t) => t,
        ::std::result::Result::Err(b) => return b,
    };
    // node: readFile/readFileSync accept an fd number in place of a path
    // (test-fs-readfile-fd) — reads run from the fd's CURRENT position.
    if let FsReadTarget::Fd(fd) = target {
        let (encoding, _) = write_file_options(cx, &args, 1);
        return match read_fd_to_eof(fd) {
            ::std::result::Result::Ok(data) => {
                return_string_content(cx, &args, &data, encoding.as_deref())
            }
            ::std::result::Result::Err(e) => throw_fs_error(
                cx,
                "readFileSync",
                &format!("fd:{}", fd),
                &e,
            ),
        };
    }
    let path = match target {
        FsReadTarget::Path(p) => p,
        FsReadTarget::Fd(_) => unreachable!(),
    };
    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_read(&path) {
        let c_msg = ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }
    // node: options.flag is honored (default 'r'; 'a+' creates + appends —
    // test-fs-read-file-sync reads not-yet-existing append targets as ''),
    // alongside options.encoding / the encoding shorthand string.
    let (encoding, flag) = write_file_options(cx, &args, 1);
    let data = match flag.as_deref() {
        ::std::option::Option::None | ::std::option::Option::Some("r") => bun_fs::read(&path),
        ::std::option::Option::Some(f) => read_file_with_flag(&path, f),
    };
    match data {
        ::std::result::Result::Ok(data) => {
            return_string_content(cx, &args, &data, encoding.as_deref())
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "readFileSync", &path, &e),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_write_file_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let target = match get_write_target_arg(cx, &args, 0) {
        ::std::result::Result::Ok(t) => t,
        ::std::result::Result::Err(b) => return b,
    };
    // node: writeFileSync(fd, data) writes at the fd's CURRENT position and
    // never truncates (test-fs-writefile-with-fd).
    if let FsWriteTarget::Fd(fd) = target {
        let data_val = if argc > 1 {
            *args.get(1).ptr
        } else {
            UndefinedValue()
        };
        let (encoding, _) = write_file_options(cx, &args, 2);
        let bytes = write_file_data_bytes(cx, data_val, encoding.as_deref());
        return match write_bytes_to_fd(fd, &bytes) {
            ::std::result::Result::Ok(_) => {
                args.rval().set(UndefinedValue());
                true
            }
            ::std::result::Result::Err(e) => throw_fs_error(
                cx,
                "writeFileSync",
                &format!("fd:{}", fd),
                &e,
            ),
        };
    }
    let path = match target {
        FsWriteTarget::Path(p) => p,
        FsWriteTarget::Fd(_) => unreachable!(),
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


#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_append_file_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let target = match get_write_target_arg(cx, &args, 0) {
        ::std::result::Result::Ok(t) => t,
        ::std::result::Result::Err(b) => return b,
    };
    // node: appendFile(fd, data) ignores flags and appends at the fd's
    // CURRENT position (write(2) semantics — no O_APPEND repositioning).
    if let FsWriteTarget::Fd(fd) = target {
        let data_val = if argc > 1 {
            *args.get(1).ptr
        } else {
            UndefinedValue()
        };
        let encoding = get_encoding_opt(cx, &args, 2);
        let data = write_file_data_bytes(cx, data_val, encoding.as_deref());
        return match write_bytes_to_fd(fd, &data) {
            ::std::result::Result::Ok(_) => {
                args.rval().set(UndefinedValue());
                true
            }
            ::std::result::Result::Err(e) => throw_fs_error(
                cx,
                "appendFileSync",
                &format!("fd:{}", fd),
                &e,
            ),
        };
    }
    let path = match target {
        FsWriteTarget::Path(p) => p,
        FsWriteTarget::Fd(_) => unreachable!(),
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
    if !cli_perm_read_gate_sync(cx, &path) {
        return false;
    }
    args.rval()
        .set(mozjs::jsval::BooleanValue(Path::new(&path).exists()));
    true
}

/// node parseFileMode face for mkdir options.mode (mkdirSync / mkdir /
/// promises.mkdir): default 0o777, must be a non-negative integer (node
/// ERR_OUT_OF_RANGE 'The value of "options.mode" is out of range. It must be
/// an integer.'). Non-object options args (legacy boolean recursive, encoding
/// string) carry no mode → default.
///
/// @trace REQ-ENG-007 [api:fs.mkdir] — options.mode contract (the three
/// mkdir faces dropped the option entirely: `{mode: 0o711}` materialized as
/// 775, the cp directory-mode test's "existing dest keeps its own mode"
/// precondition).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn get_mkdir_mode_option(
    cx: *mut JSContext,
    args: &CallArgs,
    opt_index: u32,
) -> ::std::result::Result<u32, bool> {
    const DEFAULT_MODE: u32 = 0o777;
    if args.argc_ <= opt_index {
        return ::std::result::Result::Ok(DEFAULT_MODE);
    }
    let opt_val = *args.get(opt_index).ptr;
    if !opt_val.is_object() {
        return ::std::result::Result::Ok(DEFAULT_MODE);
    }
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = opt_val.to_object());
    let mut mode_v = UndefinedValue();
    JS_GetProperty(
        cx,
        obj.handle().into(),
        c"mode".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut mode_v,
        },
    );
    if mode_v.is_undefined() || mode_v.is_null() {
        return ::std::result::Result::Ok(DEFAULT_MODE);
    }
    let raw = if mode_v.is_int32() {
        mode_v.to_int32() as f64
    } else if mode_v.is_double() {
        mode_v.to_double()
    } else {
        -1.0
    };
    if !raw.is_finite() || raw.fract() != 0.0 || !(0.0..=u32::MAX as f64).contains(&raw) {
        let msg = format!(
            "The value of \"options.mode\" is out of range. It must be an integer. Received {}",
            received_fs_desc(cx, mode_v)
        );
        return ::std::result::Result::Err(throw_fs_coded_error(
            cx,
            "ERR_OUT_OF_RANGE",
            "RangeError",
            &msg,
        ));
    }
    ::std::result::Result::Ok(raw as u32)
}

/// node mkdir core shared by the sync/callback/promise faces: DirBuilder with
/// the caller's mode — node applies `mode` to EVERY directory the call
/// creates (recursive parents included; the kernel masks it by the process
/// umask). The former faces called create_dir(_all) with the default 0o777,
/// silently dropping options.mode (cp directory-mode test: an existing dest
/// created with `{mode: 0o711}` materialized as 775 — the copy engine never
/// touched it; the mkdir face had).
fn fs_mkdir_with_mode(path: &str, recursive: bool, mode: u32) -> ::std::io::Result<()> {
    #[cfg(unix)]
    {
        use ::std::os::unix::fs::DirBuilderExt;
        let mut builder = fs::DirBuilder::new();
        builder.recursive(recursive);
        builder.mode(mode);
        builder.create(path)
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
        if recursive {
            fs::create_dir_all(path)
        } else {
            fs::create_dir(path)
        }
    }
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
    let mode = match get_mkdir_mode_option(cx, &args, 1) {
        ::std::result::Result::Ok(m) => m,
        ::std::result::Result::Err(b) => return b,
    };
    match fs_mkdir_with_mode(&path, recursive, mode) {
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
    if !cli_perm_read_gate_sync(cx, &path) {
        return false;
    }
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
    if !cli_perm_read_gate_sync(cx, &path) {
        return false;
    }
    match bun_fs::metadata(&path) {
        ::std::result::Result::Ok(meta) => {
            let stats = create_stats_object(cx, &meta, stat_bigint_option(cx, &args, 1));
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
    if !cli_perm_read_gate_sync(cx, &path) {
        return false;
    }
    match fs::symlink_metadata(&path) {
        ::std::result::Result::Ok(meta) => {
            let posix = metadata_to_posix_stat(&meta);
            let stats = create_stats_object(cx, &posix, stat_bigint_option(cx, &args, 1));
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
    if !cli_perm_read_gate_sync(cx, &path) {
        return false;
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
    if !cli_perm_read_gate_sync(cx, &path) {
        return false;
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

// ============================================================================
// fs.cp / fs.cpSync / fs.promises.cp — node-parity options-driven engine.
// @trace REQ-ENG-007 — replaces the former minimal cp_recursive (common
// recursive case, errorOnExist only) with the full node contract mirrored from
// node main lib/internal/fs/cp/{cp,cp-sync}.js + validateCpOptions
// (lib/internal/fs/utils.js) + the d25ef922d6 dir-mode rule (a directory the
// copy CREATES gets its source directory's mode only after its contents are
// in; a pre-existing destination directory keeps its own mode):
//   * options validation (调用即抛): options 非对象(function/null/primitive) →
//     ERR_INVALID_ARG_TYPE; 已知 boolean 选项 own-key 非布尔(含显式 undefined —
//     the {...defaults, ...opts} spread makes an own undefined value override
//     the default, so `{verbatimSymlinks: undefined}` throws exactly like
//     node) → ERR_INVALID_ARG_TYPE; mode → getValidMode('copyFile') integer
//     range [0,7] → ERR_OUT_OF_RANGE; dereference+verbatimSymlinks →
//     ERR_INCOMPATIBLE_OPTION_PAIR; filter must be a function when present.
//   * path-check order (checkPaths/checkParentPaths): filter first (falsy →
//     skip, before ANY validation); stat(src)+stat(dest) (ENOENT → no dest);
//     identical(ino,dev) → ERR_FS_CP_EINVAL; dir→non-dir →
//     ERR_FS_CP_DIR_TO_NON_DIR (EISDIR); non-dir→dir →
//     ERR_FS_CP_NON_DIR_TO_DIR (ENOTDIR); src dir + isSrcSubdir →
//     ERR_FS_CP_EINVAL; dest parent-chain inode walk (stat-follow) → same.
//   * copy semantics: file force→unlink / errorOnExist→ERR_FS_CP_EEXIST /
//     silent skip; chmod source mode after copy; preserveTimestamps does the
//     writability dance + fresh stat(src) for atime; non-recursive dir →
//     ERR_FS_EISDIR; created dirs chmod source mode after their contents,
//     pre-existing dirs keep their mode; symlinks run the full onLink branch
//     (verbatim vs resolve(dirname), dest-symlink replace / regular-file
//     EEXIST / SYMLINK_TO_SUBDIRECTORY guard); socket/fifo/unknown →
//     ERR_FS_CP_SOCKET / ERR_FS_CP_FIFO_PIPE / ERR_FS_CP_UNKNOWN.
//
// Errors are delivered node-shaped: FS_CP_* / ERR_FS_EISDIR as SystemError
// (`{prefix}: cp returned {errno} ({inner}) {path}` + errno/syscall/path
// props), validation errors as TypeError/RangeError, io errors as
// `{code}: {strerror}, {syscall} '{path}'`.
// ============================================================================

/// Local errno spellings (node uses internalBinding('constants').os.errno).
/// Spelled locally so CRT-number differences on non-unix targets stay invisible.
#[cfg(unix)]
use libc::{EEXIST, EISDIR, EINVAL, ENOTDIR};
#[cfg(not(unix))]
const EEXIST: i32 = 17;
#[cfg(not(unix))]
const EISDIR: i32 = 21;
#[cfg(not(unix))]
const EINVAL: i32 = 22;
#[cfg(not(unix))]
const ENOTDIR: i32 = 20;

/// A cp-engine failure ready for JS delivery (sync throw / callback error /
/// promise rejection). `js_propagated` marks the "a JS exception is already
/// pending" case (the user's filter callback threw) — delivery then forwards
/// the pending exception instead of building a fresh object.
///
/// DUAL message shape (node v24.21.0 binary oracle, 2026-10-27 farm probes):
/// the sync faces (cpSync → C++ checkPaths/CpSyncCopyDir, node_file.cc) emit
/// PURE capitalized template texts with name=Error, while the async faces
/// (fs.cp / fs.promises.cp → JS cp.js engine) emit SystemError long forms
/// `{errors.js template}: cp returned {ERRNAME} ({inner}) {path}` with
/// errno/syscall/path props. `message`/`name` = async-face form;
/// `sync_message`/`sync_name` = sync-face form. `code` is the discriminator
/// prop and is IDENTICAL on every face (GOLDEN asserts code).
struct CpError {
    code: String,
    name: &'static str,
    errno: i32,
    errno_name: &'static str,
    message: String,
    sync_name: &'static str,
    sync_message: String,
    path: Option<String>,
    syscall: Option<String>,
    js_propagated: bool,
}

impl CpError {
    /// errno-backed failure, node message shape (io face, BOTH faces):
    /// `ENOENT: no such file or directory, lstat '/x'` — name=Error
    /// (v24.21.0 probe: IO msg code name=Error syscall=open path=...).
    fn io(err: &::std::io::Error, syscall: &str, path: &str) -> CpError {
        let code = io_error_code(err);
        // Strip the " (os error N)" suffix std appends so the message matches
        // node's strerror-only rendering.
        let full = err.to_string();
        let os_msg = match full.find(" (os error") {
            Some(i) => full[..i].to_string(),
            None => full,
        };
        let message = format!("{}: {}, {} '{}'", code, os_msg, syscall, path);
        CpError {
            code: code.to_string(),
            name: "Error",
            errno: err.raw_os_error().unwrap_or(0),
            errno_name: "",
            sync_name: "Error",
            sync_message: message.clone(),
            message,
            path: ::std::option::Option::Some(path.to_string()),
            syscall: ::std::option::Option::Some(syscall.to_string()),
            js_propagated: false,
        }
    }

    /// FS_CP_* / ERR_FS_EISDIR cp-engine failure. Two face forms:
    /// * async: `new ERR_FS_CP_X({message, path, syscall: 'cp', errno, code})`
    ///   → SystemError renders `{tpl}: cp returned {errnoName} ({inner}) {path}`
    ///   where tpl is the errors.js TEMPLATE TEXT (v24.21.0 probe: prefix is
    ///   "Cannot overwrite non-directory with directory", NOT the code name).
    /// * sync: the C++ checkPaths pure message (`cannot overwrite non-directory
    ///   {dest} with directory {src}` capitalized), name=Error, errno/syscall
    ///   undefined (node_file.cc THROW_ERR_FS_CP_* literal texts).
    fn cp_system(
        code: &'static str,
        errno: i32,
        errno_name: &'static str,
        template: &'static str,
        inner: String,
        path: &str,
        sync_message: String,
    ) -> CpError {
        CpError {
            code: code.to_string(),
            name: "SystemError",
            errno,
            errno_name,
            message: format!(
                "{}: cp returned {} ({}) {}",
                template, errno_name, inner, path
            ),
            sync_name: "Error",
            sync_message,
            path: ::std::option::Option::Some(path.to_string()),
            syscall: ::std::option::Option::Some("cp".to_string()),
            js_propagated: false,
        }
    }

    /// TypeError/RangeError validation failure (plain message, no SystemError
    /// wrapper — node's ERR_INVALID_ARG_TYPE family). Identical on both faces
    /// (options validation is JS-side for all faces).
    fn validation(code: &'static str, name: &'static str, message: String) -> CpError {
        CpError {
            code: code.to_string(),
            name,
            errno: 0,
            errno_name: "",
            sync_name: name,
            sync_message: message.clone(),
            message,
            path: ::std::option::Option::None,
            syscall: ::std::option::Option::None,
            js_propagated: false,
        }
    }

    /// A JS exception is already pending (the filter callback threw) — the
    /// delivery faces forward it instead of layering a fresh error.
    fn js_propagated() -> CpError {
        CpError {
            code: ::std::string::String::new(),
            name: "",
            errno: 0,
            errno_name: "",
            message: ::std::string::String::new(),
            sync_name: "",
            sync_message: ::std::string::String::new(),
            path: ::std::option::Option::None,
            syscall: ::std::option::Option::None,
            js_propagated: true,
        }
    }
}

/// Validated cp options (node validateCpOptions output; defaults mirror
/// defaultCpOptions). `filter_present` records that the options object carries
/// a filter — the FUNCTION ITSELF is never held here: it is re-read from the
/// rooted options value on every invocation (a raw *mut JSObject held across
/// the copy loop would go stale on a moving GC — Heap::set / raw-JSVal
/// cross-GC class discipline).
struct CpOptions {
    dereference: bool,
    error_on_exist: bool,
    force: bool,
    mode: u32,
    preserve_timestamps: bool,
    recursive: bool,
    verbatim_symlinks: bool,
    filter_present: bool,
}

/// Lexical resolve: absolute → normalize; relative → cwd-join + normalize.
/// The cwd read is comparison-input only (isSrcSubdir/parent-walk); every real
/// filesystem op surfaces its own errors downstream.
fn cp_path_resolve_str(path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        return cp_path_normalize(p);
    }
    match ::std::env::current_dir() {
        Ok(cwd) => cp_path_normalize(&cwd.join(p)),
        // cwd unlinked: compare lexically on the relative path — the engine's
        // stat/mkdir calls still fail loudly on real operations.
        Err(_) => cp_path_normalize(p),
    }
}

/// Lexical normalization: drop `.`, resolve `..` against the component stack,
/// keep the root/prefix (never climbs above root).
fn cp_path_normalize(p: &Path) -> PathBuf {
    use ::std::path::Component;
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// node path.dirname (lexical): bare name → ".", root → root.
fn cp_path_dirname(path: &str) -> String {
    let p = Path::new(path);
    match p.parent() {
        Some(parent) => {
            if parent.as_os_str().is_empty() {
                ".".to_string()
            } else {
                parent.to_string_lossy().into_owned()
            }
        }
        None => {
            if p.as_os_str().is_empty() {
                ".".to_string()
            } else {
                path.to_string()
            }
        }
    }
}

fn cp_path_join(base: &str, rel: &str) -> String {
    Path::new(base)
        .join(rel)
        .to_string_lossy()
        .into_owned()
}

/// node isSrcSubdir: true when the resolved src path is a component-prefix of
/// the resolved dest path (string-based; equal paths count as true — the
/// identical-inode check catches that case first when dest exists).
fn cp_is_src_subdir(src: &str, dest: &str) -> bool {
    let rs = cp_path_resolve_str(src);
    let rd = cp_path_resolve_str(dest);
    let sc: ::std::vec::Vec<_> = rs.components().collect();
    let dc: ::std::vec::Vec<_> = rd.components().collect();
    sc.len() <= dc.len() && sc.iter().zip(dc.iter()).all(|(a, b)| a == b)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CpFileKind {
    Dir,
    File,
    Symlink,
    Socket,
    Fifo,
    CharDev,
    BlockDev,
    Other,
}

#[derive(Clone, Copy)]
struct CpStat {
    kind: CpFileKind,
    /// Permission bits (st_mode & 0o7777) — unix; coarse on other targets.
    mode: u32,
    ino: u64,
    dev: u64,
    /// Timestamps carried as the EXACT (sec, nsec) pair, not f64 seconds: an
    /// f64 seconds value quantizes to the ~238ns ulp grid at 1e9s magnitudes,
    /// so preserveTimestamps' utimes round-trip drifted up to ±120ns from the
    /// source nsec (test-fs-cp-sync-preserve-timestamps-readonly mtime delta).
    atime_sec: i64,
    atime_nsec: i64,
    mtime_sec: i64,
    mtime_nsec: i64,
}

impl CpStat {
    fn is_dir(&self) -> bool {
        self.kind == CpFileKind::Dir
    }
}

#[cfg(unix)]
fn cp_stat(path: &str, follow: bool) -> ::std::io::Result<CpStat> {
    let c_path = ::std::ffi::CString::new(path).map_err(|_| {
        ::std::io::Error::new(::std::io::ErrorKind::InvalidInput, "path contains NUL")
    })?;
    let mut s: libc::stat = unsafe { ::std::mem::zeroed() };
    let rv = unsafe {
        if follow {
            libc::stat(c_path.as_ptr(), &mut s)
        } else {
            libc::lstat(c_path.as_ptr(), &mut s)
        }
    };
    if rv != 0 {
        return Err(::std::io::Error::last_os_error());
    }
    let kind = match s.st_mode & libc::S_IFMT {
        libc::S_IFDIR => CpFileKind::Dir,
        libc::S_IFREG => CpFileKind::File,
        libc::S_IFLNK => CpFileKind::Symlink,
        libc::S_IFSOCK => CpFileKind::Socket,
        libc::S_IFIFO => CpFileKind::Fifo,
        libc::S_IFCHR => CpFileKind::CharDev,
        libc::S_IFBLK => CpFileKind::BlockDev,
        _ => CpFileKind::Other,
    };
    ::std::result::Result::Ok(CpStat {
        kind,
        mode: (s.st_mode & 0o7777) as u32,
        ino: s.st_ino as u64,
        dev: s.st_dev as u64,
        atime_sec: s.st_atime as i64,
        atime_nsec: s.st_atime_nsec as i64,
        mtime_sec: s.st_mtime as i64,
        mtime_nsec: s.st_mtime_nsec as i64,
    })
}

#[cfg(not(unix))]
fn cp_stat(path: &str, follow: bool) -> ::std::io::Result<CpStat> {
    // Coarse cross-target surface: kind from std::fs, epoch times from
    // SystemTime; mode/ino/dev are placeholders (the golden cp corpus runs on
    // Linux; Windows cp tests skip mode/identity assertions).
    let md = if follow {
        fs::metadata(path)
    } else {
        fs::symlink_metadata(path)
    }?;
    let ft = md.file_type();
    let kind = if ft.is_symlink() {
        CpFileKind::Symlink
    } else if md.is_dir() {
        CpFileKind::Dir
    } else if md.is_file() {
        CpFileKind::File
    } else {
        CpFileKind::Other
    };
    fn epoch_pair(t: ::std::io::Result<::std::time::SystemTime>) -> (i64, i64) {
        match t {
            Ok(t) => match t.duration_since(::std::time::UNIX_EPOCH) {
                Ok(d) => (d.as_secs() as i64, d.subsec_nanos() as i64),
                Err(e) => {
                    let d = e.duration();
                    let mut secs = -(d.as_secs() as i64);
                    let mut nsec = -(d.subsec_nanos() as i64);
                    if nsec < 0 {
                        secs -= 1;
                        nsec += 1_000_000_000;
                    }
                    (secs, nsec)
                }
            },
            Err(_) => (0, 0),
        }
    }
    let (atime_sec, atime_nsec) = epoch_pair(md.accessed());
    let (mtime_sec, mtime_nsec) = epoch_pair(md.modified());
    ::std::result::Result::Ok(CpStat {
        kind,
        mode: 0o644,
        ino: 0,
        dev: 0,
        atime_sec,
        atime_nsec,
        mtime_sec,
        mtime_nsec,
    })
}

#[cfg(unix)]
fn cp_chmod(path: &str, mode: u32) -> ::std::io::Result<()> {
    use ::std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn cp_chmod(path: &str, mode: u32) -> ::std::io::Result<()> {
    // std exposes only the readonly bit on this target — map the write bit.
    let mut perms = fs::metadata(path)?.permissions();
    perms.set_readonly(mode & 0o200 == 0);
    fs::set_permissions(path, perms)
}

#[cfg(unix)]
fn cp_symlink(target: &str, link: &str, _dir_flag: bool) -> ::std::io::Result<()> {
    ::std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn cp_symlink(target: &str, link: &str, dir_flag: bool) -> ::std::io::Result<()> {
    if dir_flag {
        ::std::os::windows::fs::symlink_dir(target, link)
    } else {
        ::std::os::windows::fs::symlink_file(target, link)
    }
}

#[cfg(not(any(unix, windows)))]
fn cp_symlink(_target: &str, _link: &str, _dir_flag: bool) -> ::std::io::Result<()> {
    ::std::result::Result::Err(::std::io::Error::other("symlink unsupported"))
}

/// node_file.cc:3663 — the sync forms append the platform separator to src
/// when interpolating (`.../tree/` in "Cannot copy .../tree/ to a ..."). The
/// async engine keeps the bare path.
fn cp_append_separator(path: &str) -> String {
    let sep = ::std::path::MAIN_SEPARATOR_STR;
    if path.ends_with(sep) {
        path.to_string()
    } else {
        format!("{}{}", path, sep)
    }
}

/// node areIdentical: dest inode/dev must be truthy and both match src.
fn cp_are_identical(src: &CpStat, dest: &CpStat) -> bool {
    dest.ino != 0 && dest.dev != 0 && dest.ino == src.ino && dest.dev == src.dev
}

fn cp_stat_arg(opts: &CpOptions, path: &str) -> ::std::result::Result<CpStat, CpError> {
    cp_stat(path, opts.dereference).map_err(|e| {
        CpError::io(
            &e,
            if opts.dereference { "stat" } else { "lstat" },
            path,
        )
    })
}

fn cp_chmod_err(path: &str, mode: u32) -> ::std::result::Result<(), CpError> {
    cp_chmod(path, mode).map_err(|e| CpError::io(&e, "chmod", path))
}

/// Build the invalid-arg-type error with the node message shape.
fn cp_invalid_arg_type(
    cx: *mut JSContext,
    name: &str,
    expected: &str,
    received: JSVal,
) -> CpError {
    let desc = unsafe { received_fs_desc(cx, received) };
    CpError::validation(
        "ERR_INVALID_ARG_TYPE",
        "TypeError",
        format!(
            "The \"{}\" argument must be of type {}. Received {}",
            name, expected, desc
        ),
    )
}

/// Per-key own-key boolean option read (validateCpOptions spread semantics: an
/// own key present with value `undefined` overrides the default and then fails
/// the boolean check — `{verbatimSymlinks: undefined}` throws like node).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn cp_bool_opt(
    cx: *mut JSContext,
    obj: HandleObject,
    key: &str,
    default: bool,
) -> ::std::result::Result<bool, CpError> {
    let mut found = false;
    if !JS_HasProperty(cx, obj, ZBox::from_bytes(key.as_bytes()).as_ptr(), &mut found) {
        return ::std::result::Result::Err(CpError::js_propagated());
    }
    if !found {
        return ::std::result::Result::Ok(default);
    }
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let mut val = UndefinedValue());
    if !JS_GetProperty(cx, obj, ZBox::from_bytes(key.as_bytes()).as_ptr(), val.handle_mut().into()) {
        return ::std::result::Result::Err(CpError::js_propagated());
    }
    let v = val.get();
    if !v.is_boolean() {
        return ::std::result::Result::Err(cp_invalid_arg_type(
            cx,
            &format!("options.{}", key),
            "boolean",
            v,
        ));
    }
    ::std::result::Result::Ok(v.to_boolean())
}

/// node validateCpOptions — parse + validate the options value eagerly for the
/// primitive fields; the filter FUNCTION stays in the (rooted) options object.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn cp_parse_options(
    cx: *mut JSContext,
    opts_val: JSVal,
) -> ::std::result::Result<CpOptions, CpError> {
    let mut opts = CpOptions {
        dereference: false,
        error_on_exist: false,
        force: true,
        mode: 0,
        preserve_timestamps: false,
        recursive: false,
        verbatim_symlinks: false,
        filter_present: false,
    };
    if opts_val.is_undefined() {
        return ::std::result::Result::Ok(opts);
    }
    // validateObject: typeof 'object'; null and functions (typeof 'function')
    // are rejected exactly like node.
    if !opts_val.is_object() {
        return ::std::result::Result::Err(cp_invalid_arg_type(
            cx,
            "options",
            "object",
            opts_val,
        ));
    }
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = opts_val.to_object());
    if js::IsFunctionObject(obj.get()) {
        return ::std::result::Result::Err(cp_invalid_arg_type(
            cx,
            "options",
            "object",
            opts_val,
        ));
    }
    opts.dereference = cp_bool_opt(cx, obj.handle().into(), "dereference", false)?;
    opts.error_on_exist = cp_bool_opt(cx, obj.handle().into(), "errorOnExist", false)?;
    opts.force = cp_bool_opt(cx, obj.handle().into(), "force", true)?;
    opts.preserve_timestamps =
        cp_bool_opt(cx, obj.handle().into(), "preserveTimestamps", false)?;
    opts.recursive = cp_bool_opt(cx, obj.handle().into(), "recursive", false)?;
    opts.verbatim_symlinks = cp_bool_opt(cx, obj.handle().into(), "verbatimSymlinks", false)?;

    // getValidMode('copyFile'): nullish → 0; else validateInteger(mode, 'mode', 0, 7).
    let mut found = false;
    if !JS_HasProperty(cx, obj.handle().into(), c"mode".as_ptr(), &mut found) {
        return ::std::result::Result::Err(CpError::js_propagated());
    }
    if found {
        rooted!(&in(cx_ref) let mut mval = UndefinedValue());
        if !JS_GetProperty(cx, obj.handle().into(), c"mode".as_ptr(), mval.handle_mut().into()) {
            return ::std::result::Result::Err(CpError::js_propagated());
        }
        let v = mval.get();
        if !v.is_undefined() && !v.is_null() {
            let num = if v.is_int32() {
                ::std::option::Option::Some(v.to_int32() as f64)
            } else if v.is_double() {
                ::std::option::Option::Some(v.to_double())
            } else {
                ::std::option::Option::None
            };
            match num {
                ::std::option::Option::None => {
                    return ::std::result::Result::Err(cp_invalid_arg_type(
                        cx,
                        "mode",
                        "number",
                        v,
                    ));
                }
                ::std::option::Option::Some(n) => {
                    if !n.is_finite() || n.floor() != n {
                        return ::std::result::Result::Err(CpError::validation(
                            "ERR_OUT_OF_RANGE",
                            "RangeError",
                            format!(
                                "The value of \"mode\" is out of range. It must be an integer. Received {}",
                                received_fs_desc(cx, v)
                            ),
                        ));
                    }
                    if !(0.0..=7.0).contains(&n) {
                        return ::std::result::Result::Err(CpError::validation(
                            "ERR_OUT_OF_RANGE",
                            "RangeError",
                            format!(
                                "The value of \"mode\" is out of range. It must be >= 0 and <= 7. Received {}",
                                if n == n.trunc() && n.abs() < 1e15 {
                                    format!("{}", n as i64)
                                } else {
                                    format!("{}", n)
                                }
                            ),
                        ));
                    }
                    opts.mode = n as u32;
                }
            }
        }
    }

    if opts.dereference && opts.verbatim_symlinks {
        return ::std::result::Result::Err(CpError::validation(
            "ERR_INCOMPATIBLE_OPTION_PAIR",
            "TypeError",
            "Option \"dereference\" cannot be used in combination with option \"verbatimSymlinks\""
                .to_string(),
        ));
    }

    // validateFunction when the key is present and not undefined.
    let mut found = false;
    if !JS_HasProperty(cx, obj.handle().into(), c"filter".as_ptr(), &mut found) {
        return ::std::result::Result::Err(CpError::js_propagated());
    }
    if found {
        rooted!(&in(cx_ref) let mut fval = UndefinedValue());
        if !JS_GetProperty(cx, obj.handle().into(), c"filter".as_ptr(), fval.handle_mut().into()) {
            return ::std::result::Result::Err(CpError::js_propagated());
        }
        let fv = fval.get();
        if !fv.is_undefined() {
            let is_fn = fv.is_object() && js::IsFunctionObject(fv.to_object());
            if !is_fn {
                return ::std::result::Result::Err(cp_invalid_arg_type(
                    cx,
                    "options.filter",
                    "function",
                    fv,
                ));
            }
            opts.filter_present = true;
        }
    }
    ::std::result::Result::Ok(opts)
}

/// One cp engine run: validated primitive options + a heap root over the
/// caller's options value (the filter is re-read from this slot per invocation,
/// so a moving GC across the copy loop cannot strand a stale function pointer).
/// `opts_root` is None only when root registration failed at spawn
/// (RawValueRootGuard::new contract) — the pre-parse snapshot is then the read
/// source, the same documented degraded path FsAsyncCtx.cb_root uses.
struct CpRun<'a> {
    cx: *mut JSContext,
    opts: &'a CpOptions,
    opts_root: ::std::option::Option<&'a RawValueRootGuard>,
    opts_fallback: JSVal,
}

impl CpRun<'_> {
    /// The live (GC-updated) options value; read fresh at every use.
    fn opts_val(&self) -> JSVal {
        match self.opts_root {
            ::std::option::Option::Some(g) => g.get(0),
            ::std::option::Option::None => self.opts_fallback,
        }
    }
}

/// Invoke the JS filter for one (src, dest) pair. Returns Ok(false) when the
/// filter falsy-skips the pair. A promise return fails LOUDLY with
/// ERR_INVALID_RETURN_VALUE: node's sync face throws exactly this, and the
/// async faces await (which needs a nested event loop bao does not have) —
/// fail-closed, never a silent truthy read.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn cp_run_filter(
    run: &CpRun,
    src: &str,
    dest: &str,
) -> ::std::result::Result<bool, CpError> {
    let cx = run.cx;
    if !run.opts.filter_present {
        return ::std::result::Result::Ok(true);
    }
    let opts_val = run.opts_val();
    if !opts_val.is_object() {
        return ::std::result::Result::Ok(true);
    }
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = opts_val.to_object());
    rooted!(&in(cx_ref) let mut fval = UndefinedValue());
    if !JS_GetProperty(cx, obj.handle().into(), c"filter".as_ptr(), fval.handle_mut().into()) {
        return ::std::result::Result::Err(CpError::js_propagated());
    }
    let fv = fval.get();
    if fv.is_undefined() {
        return ::std::result::Result::Err(cp_invalid_arg_type(cx, "options.filter", "function", fv));
    }
    // raw-JSString-across-GC-points class: each new string is rooted BEFORE
    // the next allocation runs — the second js_string_from_utf8 (and every
    // later JSAPI call) can trigger a moving GC that strands a bare first
    // string pointer (rooting both only after both allocations left s_js
    // bare across d_js's GC point).
    let s_js = js_string_from_utf8(cx, src);
    rooted!(&in(cx_ref) let s_root = if s_js.is_null() {
        UndefinedValue()
    } else {
        StringValue(&*s_js)
    });
    let d_js = js_string_from_utf8(cx, dest);
    rooted!(&in(cx_ref) let d_root = if d_js.is_null() {
        UndefinedValue()
    } else {
        StringValue(&*d_js)
    });
    rooted!(&in(cx_ref) let f_root = fv);
    rooted!(&in(cx_ref) let this_root = ::std::ptr::null_mut::<JSObject>());
    // unrooted-args class (B-sweep 2026-10-26): call args live in a rooted
    // ValueArray — bare stack copies do not track GC moves.
    rooted!(&in(cx_ref) let args_root = mozjs_sys::jsgc::ValueArray::<2>::new([
        s_root.get(),
        d_root.get(),
    ]));
    let cb_args = JS::HandleValueArray::from(&args_root);
    rooted!(&in(cx_ref) let mut rval = UndefinedValue());
    // node invokes opts.filter(src, dest); the corpus filters are arrows where
    // `this` is lexical, so an undefined this is behavior-neutral.
    JS_CallFunctionValue(
        cx,
        this_root.handle().into(),
        f_root.handle().into(),
        &cb_args,
        rval.handle_mut().into(),
    );
    if JS_IsExceptionPending(cx) {
        return ::std::result::Result::Err(CpError::js_propagated());
    }
    let r = rval.get();
    if r.is_object() {
        rooted!(&in(cx_ref) let r_obj = r.to_object());
        if JS::IsPromiseObject(r_obj.handle().into()) {
            return ::std::result::Result::Err(CpError::validation(
                "ERR_INVALID_RETURN_VALUE",
                "TypeError",
                "Expected boolean to be returned from the \"filter\" function but got an instance of Promise."
                    .to_string(),
            ));
        }
    }
    rooted!(&in(cx_ref) let r_root = r);
    ::std::result::Result::Ok(mozjs::rust::ToBoolean(r_root.handle()))
}

/// node checkPaths: stat both sides, identical/dir-mismatch/subdir guards.
/// Returns (srcStat, Option<destStat>).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn cp_check_paths(
    run: &CpRun,
    src: &str,
    dest: &str,
) -> ::std::result::Result<(CpStat, ::std::option::Option<CpStat>), CpError> {
    let opts = run.opts;
    let src_stat = cp_stat_arg(opts, src)?;
    let dest_stat = match cp_stat(dest, opts.dereference) {
        ::std::result::Result::Ok(s) => ::std::option::Option::Some(s),
        ::std::result::Result::Err(e)
            if e.kind() == ::std::io::ErrorKind::NotFound =>
        {
            ::std::option::Option::None
        }
        ::std::result::Result::Err(e) => {
            return ::std::result::Result::Err(CpError::io(
                &e,
                if opts.dereference { "stat" } else { "lstat" },
                dest,
            ));
        }
    };
    if let ::std::option::Option::Some(ref ds) = dest_stat {
        if cp_are_identical(&src_stat, ds) {
            // node_file.cc:3630 — sync: `src and dest cannot be the same {dest}`;
            // async probe: `Invalid src or dest: cp returned EINVAL (...) {dest}`.
            return ::std::result::Result::Err(CpError::cp_system(
                "ERR_FS_CP_EINVAL",
                EINVAL,
                "EINVAL",
                "Invalid src or dest",
                "src and dest cannot be the same".to_string(),
                dest,
                format!("src and dest cannot be the same {}", dest),
            ));
        }
        if src_stat.is_dir() && !ds.is_dir() {
            // node_file.cc:3638 (capitalized, dest first) vs cp.js:90
            // (uncapitalized inner).
            return ::std::result::Result::Err(CpError::cp_system(
                "ERR_FS_CP_DIR_TO_NON_DIR",
                EISDIR,
                "EISDIR",
                "Cannot overwrite non-directory with directory",
                format!(
                    "cannot overwrite non-directory {} with directory {}",
                    dest, src
                ),
                dest,
                format!(
                    "Cannot overwrite non-directory {} with directory {}",
                    dest, src
                ),
            ));
        }
        if !src_stat.is_dir() && ds.is_dir() {
            // node_file.cc:3645 vs cp.js:100.
            return ::std::result::Result::Err(CpError::cp_system(
                "ERR_FS_CP_NON_DIR_TO_DIR",
                ENOTDIR,
                "ENOTDIR",
                "Cannot overwrite directory with non-directory",
                format!(
                    "cannot overwrite directory {} with non-directory {}",
                    dest, src
                ),
                dest,
                format!(
                    "Cannot overwrite directory {} with non-directory {}",
                    dest, src
                ),
            ));
        }
    }
    if src_stat.is_dir() && cp_is_src_subdir(src, dest) {
        // node_file.cc:3657 — the sync form appends a SEPARATOR to src and
        // interpolates the suffixed string (v24 probe: `.../tree/ to a ...`);
        // the async engine interpolates the bare src (cp.js:112).
        let src_slash = cp_append_separator(src);
        return ::std::result::Result::Err(CpError::cp_system(
            "ERR_FS_CP_EINVAL",
            EINVAL,
            "EINVAL",
            "Invalid src or dest",
            format!("cannot copy {} to a subdirectory of self {}", src, dest),
            dest,
            format!(
                "Cannot copy {} to a subdirectory of self {}",
                src_slash, dest
            ),
        ));
    }
    ::std::result::Result::Ok((src_stat, dest_stat))
}

/// node checkParentPaths: walk dest's parent chain (stat-FOLLOW, so a symlinked
/// parent resolves to its target's inode) and fail when a parent IS src.
fn cp_check_parent_paths(
    src_stat: &CpStat,
    src: &str,
    dest: &str,
) -> ::std::result::Result<(), CpError> {
    let src_parent = cp_path_resolve_str(&cp_path_dirname(src));
    let mut dest_parent = cp_path_resolve_str(&cp_path_dirname(dest));
    let mut dest_at_depth = dest.to_string();
    loop {
        if dest_parent == src_parent || dest_parent.parent().is_none() {
            return ::std::result::Result::Ok(());
        }
        let dp = dest_parent.to_string_lossy().into_owned();
        let ds = match cp_stat(&dp, true) {
            ::std::result::Result::Ok(s) => s,
            ::std::result::Result::Err(e)
                if e.kind() == ::std::io::ErrorKind::NotFound =>
            {
                return ::std::result::Result::Ok(());
            }
            ::std::result::Result::Err(e) => {
                return ::std::result::Result::Err(CpError::io(&e, "stat", &dp));
            }
        };
        if cp_are_identical(src_stat, &ds) {
            // node_file.cc:3670 — sync form capitalizes + separator-suffixes
            // src (v24 probe: `.../tree/ to a subdirectory ...`).
            return ::std::result::Result::Err(CpError::cp_system(
                "ERR_FS_CP_EINVAL",
                EINVAL,
                "EINVAL",
                "Invalid src or dest",
                format!(
                    "cannot copy {} to a subdirectory of self {}",
                    src, dest_at_depth
                ),
                &dest_at_depth,
                format!(
                    "Cannot copy {} to a subdirectory of self {}",
                    cp_append_separator(src),
                    dest_at_depth
                ),
            ));
        }
        match dest_parent.parent() {
            ::std::option::Option::Some(p) => {
                dest_parent = cp_path_normalize(p);
                dest_at_depth = dp;
            }
            ::std::option::Option::None => return ::std::result::Result::Ok(()),
        }
    }
}

/// node getStatsForCopy: dispatch on the FRESH source stat (re-statted here,
/// mirroring cp.js — the checkPaths stat is not reused).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn cp_get_stats_for_copy(
    run: &CpRun,
    dest_stat: ::std::option::Option<CpStat>,
    src: &str,
    dest: &str,
) -> ::std::result::Result<(), CpError> {
    let opts = run.opts;
    let src_stat = cp_stat_arg(opts, src)?;
    match src_stat.kind {
        CpFileKind::Dir => {
            if !opts.recursive {
                // node_file.cc:3684 — sync text is COMPLETELY different
                // (`Recursive option not enabled, cannot copy a directory:
                // {src}/`, separator-suffixed, name=Error) vs cp.js:202 inner
                // (`{src} is a directory (not copied)`, SystemError long form).
                return ::std::result::Result::Err(CpError::cp_system(
                    "ERR_FS_EISDIR",
                    EISDIR,
                    "EISDIR",
                    "Path is a directory",
                    format!("{} is a directory (not copied)", src),
                    src,
                    format!(
                        "Recursive option not enabled, cannot copy a directory: {}",
                        cp_append_separator(src)
                    ),
                ));
            }
            cp_on_dir(run, &src_stat, dest_stat, src, dest)
        }
        CpFileKind::File | CpFileKind::CharDev | CpFileKind::BlockDev => {
            cp_on_file(run, &src_stat, dest_stat, src, dest)
        }
        CpFileKind::Symlink => cp_on_link(run, dest_stat, src, dest),
        CpFileKind::Socket => ::std::result::Result::Err(CpError::cp_system(
            "ERR_FS_CP_SOCKET",
            EINVAL,
            "EINVAL",
            "Cannot copy a socket file",
            format!("cannot copy a socket file: {}", dest),
            dest,
            format!("Cannot copy a socket file: {}", dest),
        )),
        CpFileKind::Fifo => ::std::result::Result::Err(CpError::cp_system(
            "ERR_FS_CP_FIFO_PIPE",
            EINVAL,
            "EINVAL",
            "Cannot copy a FIFO pipe",
            format!("cannot copy a FIFO pipe: {}", dest),
            dest,
            format!("Cannot copy a FIFO pipe: {}", dest),
        )),
        CpFileKind::Other => ::std::result::Result::Err(CpError::cp_system(
            "ERR_FS_CP_UNKNOWN",
            EINVAL,
            "EINVAL",
            "Cannot copy an unknown file type",
            format!("cannot copy an unknown file type: {}", dest),
            dest,
            format!("Cannot copy an unknown file type: {}", dest),
        )),
    }
}

fn cp_copy_file(
    opts: &CpOptions,
    src_stat: &CpStat,
    src: &str,
    dest: &str,
) -> ::std::result::Result<(), CpError> {
    // node copyFile mode flags: bit 0 = UV_FS_COPYFILE_EXCL refuses overwrite;
    // the FICLONE bits are perf hints std::fs::copy does not implement.
    if opts.mode & 0o1 != 0 {
        match cp_stat(dest, false) {
            ::std::result::Result::Ok(_) => {
                return ::std::result::Result::Err(CpError::io(
                    &::std::io::Error::from_raw_os_error(EEXIST),
                    "copyfile",
                    dest,
                ));
            }
            ::std::result::Result::Err(_) => {}
        }
    }
    fs::copy(src, dest).map_err(|e| CpError::io(&e, "copyfile", dest))?;
    if opts.preserve_timestamps {
        cp_handle_timestamps(opts, src_stat.mode, src, dest)
    } else {
        cp_chmod_err(dest, src_stat.mode)
    }
}

/// node handleTimestamps + setDestTimestampsAndMode: make the dest writable,
/// utimes from a FRESH stat(src) (the walk's read may have touched atime),
/// then stamp the source mode back.
fn cp_handle_timestamps(
    opts: &CpOptions,
    src_mode: u32,
    src: &str,
    dest: &str,
) -> ::std::result::Result<(), CpError> {
    if src_mode & 0o200 == 0 {
        cp_chmod_err(dest, src_mode | 0o200)?;
    }
    let fresh = cp_stat(src, true).map_err(|e| CpError::io(&e, "stat", src))?;
    utimes_path_ns(
        dest,
        (fresh.atime_sec, fresh.atime_nsec),
        (fresh.mtime_sec, fresh.mtime_nsec),
        true,
    )
    .map_err(|e| CpError::io(&e, "utimes", dest))?;
    cp_chmod_err(dest, src_mode)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn cp_on_file(
    run: &CpRun,
    src_stat: &CpStat,
    dest_stat: ::std::option::Option<CpStat>,
    src: &str,
    dest: &str,
) -> ::std::result::Result<(), CpError> {
    let opts = run.opts;
    match dest_stat {
        ::std::option::Option::None => cp_copy_file(opts, src_stat, src, dest),
        ::std::option::Option::Some(_) => {
            if opts.force {
                fs::remove_file(dest).map_err(|e| CpError::io(&e, "unlink", dest))?;
                cp_copy_file(opts, src_stat, src, dest)
            } else if opts.error_on_exist {
                // v24.21.0 probe: EEXIST renders the SAME SystemError long form
                // on BOTH faces (sync JS mayCopyFile/copySync + async
                // mayCopyFile) — sync_message mirrors it (errno=17,
                // syscall=cp, path prop set).
                let inner = format!("{} already exists", dest);
                ::std::result::Result::Err(CpError::cp_system(
                    "ERR_FS_CP_EEXIST",
                    EEXIST,
                    "EEXIST",
                    "Target already exists",
                    inner.clone(),
                    dest,
                    format!(
                        "Target already exists: cp returned EEXIST ({}) {}",
                        inner, dest
                    ),
                ))
            } else {
                // node mayCopyFile: !force && !errorOnExist → silent skip.
                ::std::result::Result::Ok(())
            }
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn cp_on_dir(
    run: &CpRun,
    src_stat: &CpStat,
    dest_stat: ::std::option::Option<CpStat>,
    src: &str,
    dest: &str,
) -> ::std::result::Result<(), CpError> {
    let opts = run.opts;
    match dest_stat {
        ::std::option::Option::None => {
            // d25ef922d6: a directory the copy CREATES takes the source mode
            // only AFTER its contents are in (the source may be read-only).
            fs::create_dir(dest).map_err(|e| CpError::io(&e, "mkdir", dest))?;
            cp_copy_dir(run, src, dest, ::std::option::Option::Some(src_stat.mode))
        }
        ::std::option::Option::Some(_) => {
            if opts.error_on_exist && !opts.force {
                // Same dual-face SystemError form as the file arm (v24 probe:
                // dir-over-dir errorOnExist renders identically on both faces).
                let inner = format!("{} already exists", dest);
                return ::std::result::Result::Err(CpError::cp_system(
                    "ERR_FS_CP_EEXIST",
                    EEXIST,
                    "EEXIST",
                    "Target already exists",
                    inner.clone(),
                    dest,
                    format!(
                        "Target already exists: cp returned EEXIST ({}) {}",
                        inner, dest
                    ),
                ));
            }
            // Merge into the existing directory; its own mode is kept.
            cp_copy_dir(run, src, dest, ::std::option::Option::None)
        }
    }
}

/// node copyDir: per-entry filter → checkPaths → getStatsForCopy, then the
/// cp-sync.js `finally` block (dest timestamps, then the source mode for
/// newly-created dirs) runs even when the entry walk failed — a finally error
/// replaces the walk error, matching JS `finally` semantics.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn cp_copy_dir(
    run: &CpRun,
    src: &str,
    dest: &str,
    src_mode: ::std::option::Option<u32>,
) -> ::std::result::Result<(), CpError> {
    let body = cp_copy_dir_entries(run, src, dest);
    let finish = (|| -> ::std::result::Result<(), CpError> {
        if run.opts.preserve_timestamps {
            let fresh = cp_stat(src, true).map_err(|e| CpError::io(&e, "stat", src))?;
            utimes_path_ns(
                dest,
                (fresh.atime_sec, fresh.atime_nsec),
                (fresh.mtime_sec, fresh.mtime_nsec),
                true,
            )
            .map_err(|e| CpError::io(&e, "utimes", dest))?;
        }
        if let ::std::option::Option::Some(mode) = src_mode {
            cp_chmod_err(dest, mode)?;
        }
        ::std::result::Result::Ok(())
    })();
    match (body, finish) {
        (::std::result::Result::Ok(()), ::std::result::Result::Ok(())) => {
            ::std::result::Result::Ok(())
        }
        (::std::result::Result::Err(e), ::std::result::Result::Ok(())) => {
            ::std::result::Result::Err(e)
        }
        (::std::result::Result::Err(_), ::std::result::Result::Err(f)) => {
            ::std::result::Result::Err(f)
        }
        (::std::result::Result::Ok(()), ::std::result::Result::Err(f)) => {
            ::std::result::Result::Err(f)
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn cp_copy_dir_entries(
    run: &CpRun,
    src: &str,
    dest: &str,
) -> ::std::result::Result<(), CpError> {
    let rd = fs::read_dir(src).map_err(|e| CpError::io(&e, "opendir", src))?;
    for entry in rd {
        let entry = entry.map_err(|e| CpError::io(&e, "readdir", src))?;
        let src_item = entry.path();
        let dest_item = Path::new(dest).join(entry.file_name());
        let src_item_s = src_item.to_string_lossy().into_owned();
        let dest_item_s = dest_item.to_string_lossy().into_owned();
        if !cp_run_filter(run, &src_item_s, &dest_item_s)? {
            continue;
        }
        let (_src_stat, dest_stat) = cp_check_paths(run, &src_item_s, &dest_item_s)?;
        cp_get_stats_for_copy(run, dest_stat, &src_item_s, &dest_item_s)?;
    }
    ::std::result::Result::Ok(())
}

/// node onLink, full branch set: readlink → (verbatim | resolve(dirname(src)));
/// no dest → symlink; dest symlink → guards + unlink + symlink; dest regular
/// file → the EEXIST from symlink() itself (node's same path).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn cp_on_link(
    run: &CpRun,
    dest_stat: ::std::option::Option<CpStat>,
    src: &str,
    dest: &str,
) -> ::std::result::Result<(), CpError> {
    let opts = run.opts;
    let raw_target = fs::read_link(src).map_err(|e| CpError::io(&e, "readlink", src))?;
    let mut resolved_src = raw_target.to_string_lossy().into_owned();
    if !opts.verbatim_symlinks && !Path::new(&resolved_src).is_absolute() {
        resolved_src =
            cp_path_resolve_str(&cp_path_join(&cp_path_dirname(src), &resolved_src))
                .to_string_lossy()
                .into_owned();
    }
    // internalModuleStat(src) === 1 → the link RESOLVES to a directory. Any
    // stat error makes it a "file" link (node's -1 → false), the dir/file flag
    // only matters on Windows.
    let src_is_dir = match cp_stat(src, true) {
        ::std::result::Result::Ok(s) => s.is_dir(),
        ::std::result::Result::Err(_) => false,
    };
    if dest_stat.is_none() {
        return cp_symlink(&resolved_src, dest, src_is_dir)
            .map_err(|e| CpError::io(&e, "symlink", dest));
    }
    let resolved_dest = match fs::read_link(dest) {
        ::std::result::Result::Ok(t) => {
            let mut rd = t.to_string_lossy().into_owned();
            if !Path::new(&rd).is_absolute() {
                rd = cp_path_resolve_str(&cp_path_join(&cp_path_dirname(dest), &rd))
                    .to_string_lossy()
                    .into_owned();
            }
            rd
        }
        ::std::result::Result::Err(e) => {
            // Dest exists and is a regular file or directory (readlink EINVAL;
            // node retries symlink() so the EEXIST from the existing dest
            // surfaces — copy-symlink-over-file contract).
            if e.raw_os_error() == ::std::option::Option::Some(EINVAL)
                || e.kind() == ::std::io::ErrorKind::InvalidInput
            {
                return cp_symlink(&resolved_src, dest, src_is_dir)
                    .map_err(|e2| CpError::io(&e2, "symlink", dest));
            }
            return ::std::result::Result::Err(CpError::io(&e, "readlink", dest));
        }
    };
    if src_is_dir && cp_is_src_subdir(&resolved_src, &resolved_dest) {
        // cp.js:368 inner (uncapitalized, resolved paths) + node_file.cc:3909
        // sync text (capitalized). The async SystemError prefix is the
        // errors.js template "Invalid src or dest".
        return ::std::result::Result::Err(CpError::cp_system(
            "ERR_FS_CP_EINVAL",
            EINVAL,
            "EINVAL",
            "Invalid src or dest",
            format!(
                "cannot copy {} to a subdirectory of self {}",
                resolved_src, resolved_dest
            ),
            dest,
            format!(
                "Cannot copy {} to a subdirectory of self {}",
                resolved_src, resolved_dest
            ),
        ));
    }
    // Do not overwrite when src lives inside the dest symlink's target —
    // unlinking dest would destroy src contents (node's second onLink guard).
    let src_follow = cp_stat(src, true).map_err(|e| CpError::io(&e, "stat", src))?;
    if src_follow.is_dir() && cp_is_src_subdir(&resolved_dest, &resolved_src) {
        // cp.js:382 inner + node_file.cc:3921 sync text — NOTE the sync C++
        // form is lowercase `cannot overwrite %s with %s` (verbatim literal).
        return ::std::result::Result::Err(CpError::cp_system(
            "ERR_FS_CP_SYMLINK_TO_SUBDIRECTORY",
            EINVAL,
            "EINVAL",
            "Cannot overwrite symlink in subdirectory of self",
            format!("cannot overwrite {} with {}", resolved_dest, resolved_src),
            dest,
            format!(
                "cannot overwrite {} with {}",
                resolved_dest, resolved_src
            ),
        ));
    }
    fs::remove_file(dest).map_err(|e| CpError::io(&e, "unlink", dest))?;
    cp_symlink(&resolved_src, dest, src_is_dir).map_err(|e| CpError::io(&e, "symlink", dest))
}

/// Shared engine entry: filter → checkPaths → checkParentPaths →
/// ensure dest parent → getStatsForCopy.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn cp_engine(
    run: &CpRun,
    src: &str,
    dest: &str,
) -> ::std::result::Result<(), CpError> {
    // The filter runs BEFORE any path validation (node checkPaths line 1) —
    // skip-validation-when-filtered semantics.
    if !cp_run_filter(run, src, dest)? {
        return ::std::result::Result::Ok(());
    }
    let (src_stat, dest_stat) = cp_check_paths(run, src, dest)?;
    cp_check_parent_paths(&src_stat, src, dest)?;
    // node checkParentDir: dest parent must exist (mkdir recursive).
    let dest_parent = cp_path_dirname(dest);
    match cp_stat(&dest_parent, true) {
        ::std::result::Result::Ok(_) => {}
        ::std::result::Result::Err(e)
            if e.kind() == ::std::io::ErrorKind::NotFound =>
        {
            fs::create_dir_all(&dest_parent)
                .map_err(|e2| CpError::io(&e2, "mkdir", &dest_parent))?;
        }
        ::std::result::Result::Err(e) => {
            return ::std::result::Result::Err(CpError::io(&e, "stat", &dest_parent));
        }
    }
    cp_get_stats_for_copy(run, dest_stat, src, dest)
}

/// Build a plain JS object carrying the node error prop set (message / code /
/// name / errno / syscall / path). `sync` selects the cpSync C++ message form
/// (pure template text, name=Error, errno/syscall undefined except the EEXIST
/// long form); async/callback deliveries pass false. Returns UndefinedValue
/// only when object construction itself failed.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn cp_error_value(cx: *mut JSContext, e: &CpError, sync: bool) -> JSVal {
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let err_obj = JS_NewPlainObject(cx));
    if err_obj.get().is_null() {
        return UndefinedValue();
    }
    // UTF8 path for the message: cp messages embed arbitrary filesystem paths
    // (unicode-dest corpus) — JS_NewStringCopyZ would Latin1-mangle them.
    // `sync` selects the sync-face form (cpSync C++ pure texts, name=Error);
    // async/callback deliveries use the SystemError long form.
    let js_msg = js_string_from_utf8(
        cx,
        if sync { &e.sync_message } else { &e.message },
    );
    if !js_msg.is_null() {
        rooted!(&in(cx_ref) let msg_val = StringValue(&*js_msg));
        JS_DefineProperty(
            cx,
            err_obj.handle().into(),
            c"message".as_ptr(),
            msg_val.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }
    let c_code = ZBox::from_bytes(e.code.as_bytes());
    let js_code = JS_NewStringCopyZ(cx, c_code.as_ptr());
    if !js_code.is_null() {
        rooted!(&in(cx_ref) let code_val = StringValue(&*js_code));
        JS_DefineProperty(
            cx,
            err_obj.handle().into(),
            c"code".as_ptr(),
            code_val.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }
    // errno/syscall stay UNDEFINED on the sync face (v24 probe: cpSync's C++
    // errors carry neither; only the EEXIST long form — which carries them via
    // its own construction — and the async face set them). Simplest faithful
    // cut: the sync face drops them whenever the sync message is the pure
    // template text, keeps them when sync_message carries the long form.
    let long_form_sync = sync && e.sync_message.contains("cp returned");
    if long_form_sync || !sync {
        if e.errno != 0 {
            rooted!(&in(cx_ref) let errno_val = Int32Value(e.errno));
            JS_DefineProperty(
                cx,
                err_obj.handle().into(),
                c"errno".as_ptr(),
                errno_val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
        if let ::std::option::Option::Some(ref syscall) = e.syscall {
            let c_sys = ZBox::from_bytes(syscall.as_bytes());
            let js_sys = JS_NewStringCopyZ(cx, c_sys.as_ptr());
            if !js_sys.is_null() {
                rooted!(&in(cx_ref) let sys_val = StringValue(&*js_sys));
                JS_DefineProperty(
                    cx,
                    err_obj.handle().into(),
                    c"syscall".as_ptr(),
                    sys_val.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }
    }
    let face_name = if sync { e.sync_name } else { e.name };
    let c_name = ZBox::from_bytes(face_name.as_bytes());
    let js_name = JS_NewStringCopyZ(cx, c_name.as_ptr());
    if !js_name.is_null() {
        rooted!(&in(cx_ref) let name_val = StringValue(&*js_name));
        JS_DefineProperty(
            cx,
            err_obj.handle().into(),
            c"name".as_ptr(),
            name_val.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }
    if let ::std::option::Option::Some(ref path) = e.path {
        let js_path = js_string_from_utf8(cx, path);
        if !js_path.is_null() {
            rooted!(&in(cx_ref) let path_val = StringValue(&*js_path));
            JS_DefineProperty(
                cx,
                err_obj.handle().into(),
                c"path".as_ptr(),
                path_val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }
    mozjs::jsval::ObjectValue(err_obj.get())
}

/// Sync-face delivery: report the error as a pending exception branded with
/// the node prop set (cpSync C++ message form). A propagated JS exception
/// (filter threw) is left as-is.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn cp_throw_error(cx: *mut JSContext, e: &CpError) -> bool {
    if e.js_propagated {
        return false;
    }
    let c_msg = ZBox::from_bytes(e.sync_message.as_bytes());
    JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
    if JS_IsExceptionPending(cx) {
        let err_val = cp_error_value(cx, e, true);
        if !err_val.is_undefined() {
            let mut wrapped_cx =
                mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let err_rooted = err_val);
            JS_SetPendingException(
                cx,
                err_rooted.handle().into(),
                ExceptionStackBehavior::DoNotCapture,
            );
        }
    }
    false
}

/// Promise-face delivery: reject with the branded error object, or with the
/// pending exception when the filter threw.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn cp_reject(cx: *mut JSContext, promise: *mut JSObject, e: &CpError) {
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise_rooted = promise);
    if e.js_propagated {
        rooted!(&in(cx_ref) let mut exn = UndefinedValue());
        if JS_GetPendingException(cx, exn.handle_mut().into()) {
            JS_ClearPendingException(cx);
            JS::RejectPromise(
                cx,
                promise_rooted.handle().into(),
                exn.handle().into(),
            );
        }
        return;
    }
    let val = cp_error_value(cx, e, false);
    if val.is_undefined() {
        // Error-object construction failed — still settle (reject with the
        // message text) so the promise never hangs pending.
        reject_with_error(cx, promise, &e.message);
        return;
    }
    rooted!(&in(cx_ref) let val_rooted = val);
    JS::RejectPromise(cx, promise_rooted.handle().into(), val_rooted.handle().into());
}

/// Path argument for the cp faces: node names them `src`/`dest` (the generic
/// fs faces say "path").
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn cp_face_path_arg(
    cx: *mut JSContext,
    args: &CallArgs,
    index: u32,
    name: &str,
) -> ::std::result::Result<::std::string::String, bool> {
    let val = if args.argc_ > index {
        *args.get(index).ptr
    } else {
        UndefinedValue()
    };
    resolve_fs_path_value(cx, val, name)
}

/// fs.cpSync(src, dest[, options]) — @trace REQ-ENG-007 — options-driven
/// node-parity engine (see the cp engine block). Validation errors throw
/// synchronously, exactly like node's cpSync → validateCpOptions.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_cp_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let from = match cp_face_path_arg(cx, &args, 0, "src") {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let to = match cp_face_path_arg(cx, &args, 1, "dest") {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let opts_val = if args.argc_ > 2 {
        *args.get(2).ptr
    } else {
        UndefinedValue()
    };
    // Heap-root the options value BEFORE any further JS runs (option getters
    // can move the object): the guard copies it into its GC-tracked slot and
    // the parse below consumes the slot, never the stale stack copy
    // (raw-JSVal-across-GC-points class). Registration failure (OOM) fails
    // LOUD — proceeding on the bare snapshot would carry it unrooted across
    // the whole engine (no silent degraded path; fallback ban).
    let opts_root = match RawValueRootGuard::new(cx, ::std::slice::from_ref(&opts_val), c"cp_opts")
    {
        ::std::option::Option::Some(g) => g,
        ::std::option::Option::None => {
            JS_ReportErrorUTF8(cx, c"cp: failed to GC-root the options value".as_ptr());
            return false;
        }
    };
    let opts = match cp_parse_options(cx, opts_root.get(0)) {
        ::std::result::Result::Ok(o) => o,
        ::std::result::Result::Err(e) => {
            drop(opts_root);
            return cp_throw_error(cx, &e);
        }
    };
    let run = CpRun {
        cx,
        opts: &opts,
        opts_root: ::std::option::Option::Some(&opts_root),
        opts_fallback: opts_val,
    };
    let result = cp_engine(&run, &from, &to);
    drop(opts_root);
    match result {
        ::std::result::Result::Ok(()) => {
            args.rval().set(UndefinedValue());
            true
        }
        ::std::result::Result::Err(e) => cp_throw_error(cx, &e),
    }
}

/// fs.cp(src, dest[, options][, callback]) — @trace REQ-ENG-007 — options
/// validation throws synchronously (node validates in the sync body before the
/// threadpool handoff); the copy itself runs inline and the callback is
/// delivered with the node-shaped (err | null) contract. Node dispatches the
/// copy to its thread pool; the corpus callback contracts (mustCall + err
/// inspection) are delivery-order agnostic, and the JS filter callback can
/// only run on this thread, so inline execution is the only fail-closed shape.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_cp(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let from = match cp_face_path_arg(cx, &args, 0, "src") {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let to = match cp_face_path_arg(cx, &args, 1, "dest") {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    // Node: fs.cp(src, dest[, options][, callback]) — a FUNCTION at position 2
    // is the callback and the options slot becomes undefined.
    let mut opts_val = if args.argc_ > 2 {
        *args.get(2).ptr
    } else {
        UndefinedValue()
    };
    let mut cb_val = UndefinedValue();
    if opts_val.is_object() && js::IsFunctionObject(opts_val.to_object()) {
        cb_val = opts_val;
        opts_val = UndefinedValue();
    } else if args.argc_ > 3 {
        cb_val = *args.get(3).ptr;
    }
    let cb_is_fn = cb_val.is_object() && js::IsFunctionObject(cb_val.to_object());
    // Heap-root BOTH the options value and the callback BEFORE parse runs
    // getters (GC can move either): parse consumes the guard slot, never the
    // stale stack copies (raw-JSVal-across-GC-points class). Registration
    // failure (OOM) fails LOUD — the bare snapshots would ride unrooted across
    // the engine and the callback delivery (no silent degraded path).
    let roots = [opts_val, cb_val];
    let root_guard = match RawValueRootGuard::new(cx, &roots, c"cp_opts_cb") {
        ::std::option::Option::Some(g) => g,
        ::std::option::Option::None => {
            JS_ReportErrorUTF8(cx, c"cp: failed to GC-root the options/callback values".as_ptr());
            return false;
        }
    };
    let opts = match cp_parse_options(cx, root_guard.get(0)) {
        ::std::result::Result::Ok(o) => o,
        ::std::result::Result::Err(e) => {
            drop(root_guard);
            return cp_throw_error(cx, &e);
        }
    };
    let run = CpRun {
        cx,
        opts: &opts,
        opts_root: ::std::option::Option::Some(&root_guard),
        opts_fallback: opts_val,
    };
    let result = cp_engine(&run, &from, &to);
    if cb_is_fn {
        let mut wrapped_cx =
            mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        let err_val = match &result {
            ::std::result::Result::Ok(()) => mozjs::jsval::NullValue(),
            ::std::result::Result::Err(e) if e.js_propagated => {
                // The filter threw — the callback receives the pending exception
                // (node's promise-rejection → callback(err) shape).
                rooted!(&in(cx_ref) let mut exn = UndefinedValue());
                if JS_GetPendingException(cx, exn.handle_mut().into()) {
                    JS_ClearPendingException(cx);
                    exn.get()
                } else {
                    UndefinedValue()
                }
            }
            ::std::result::Result::Err(e) => cp_error_value(cx, e, false),
        };
        rooted!(&in(cx_ref) let err_root = err_val);
        rooted!(&in(cx_ref) let cb_rooted = root_guard.get(1));
        rooted!(&in(cx_ref) let this_root = ::std::ptr::null_mut::<JSObject>());
        // unrooted-args class (B-sweep 2026-10-26): rooted ValueArray for args.
        rooted!(&in(cx_ref) let args_root = mozjs_sys::jsgc::ValueArray::<1>::new([
            err_root.get(),
        ]));
        let cb_args = JS::HandleValueArray::from(&args_root);
        rooted!(&in(cx_ref) let mut rval = UndefinedValue());
        JS_CallFunctionValue(
            cx,
            this_root.handle().into(),
            cb_rooted.handle().into(),
            &cb_args,
            rval.handle_mut().into(),
        );
        JS_ClearPendingException(cx);
        drop(root_guard);
        args.rval().set(UndefinedValue());
        return true;
    }
    drop(root_guard);
    match result {
        ::std::result::Result::Ok(()) => {
            args.rval().set(UndefinedValue());
            true
        }
        ::std::result::Result::Err(e) => cp_throw_error(cx, &e),
    }
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
    // recursive: windows = native subtree (notify bWatchSubtree semantics);
    // linux = node's own JS recursive layer (lib/internal/fs/recursive_watch.js
    // — node has no native recursive inotify; it fans one fs.watch out per
    // subdirectory + stat scanning). Delegating to the ported JS layer keeps
    // the event semantics byte-faithful to the node oracle (initial scan,
    // rename/change decisioning, relative filenames, new-dir rescans).
    #[cfg(windows)]
    let mut recursive = false;
    #[cfg(not(windows))]
    let mut recursive = false;
    let mut encoding: ::std::string::String = "utf8".to_string();
    let mut listener_val = UndefinedValue();
    let mut raw_options_val = UndefinedValue();
    let mut signal_opt: ::std::option::Option<JSVal> = ::std::option::Option::None;
    if _argc > 1 {
        let opt_val = *args.get(1).ptr;
        // Node overload: watch(filename[, options][, listener]) — a FUNCTION
        // at position 1 is the listener, not the options object.
        let opt_is_fn = opt_val.is_object()
            && unsafe { mozjs_sys::jsapi::js::IsFunctionObject(opt_val.to_object()) };
        if opt_val.is_object() && !opt_is_fn {
            raw_options_val = opt_val;
            let mut wrapped_opt =
                mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
            let cx_opt = &mut wrapped_opt;
            rooted!(&in(cx_opt) let opt_obj = opt_val.to_object());
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
            // node validates option types loudly (validateBoolean — the
            // silent-ignore form made watch-recursive-validate's expected
            // throw a missing exception).
            if !recursive_v.is_undefined() && !recursive_v.is_null() && !recursive_v.is_boolean() {
                let msg = format!(
                    "The \"options.recursive\" argument must be of type boolean. Received {}",
                    received_fs_desc(cx, recursive_v)
                );
                return throw_fs_coded_error(cx, "ERR_INVALID_ARG_TYPE", "TypeError", &msg);
            }
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
            if !persistent_v.is_undefined() && !persistent_v.is_null() && !persistent_v.is_boolean() {
                let msg = format!(
                    "The \"options.persistent\" argument must be of type boolean. Received {}",
                    received_fs_desc(cx, persistent_v)
                );
                return throw_fs_coded_error(cx, "ERR_INVALID_ARG_TYPE", "TypeError", &msg);
            }
            if persistent_v.is_boolean() {
                persistent = persistent_v.to_boolean();
            }
            // options.encoding (string | object.encoding) — filename delivery
            // format ('buffer' → Buffer, 'hex'/'base64' → encoded string).
            if let ::std::option::Option::Some(enc) = get_string_option(cx, &args, 1, "encoding") {
                encoding = enc;
            }
            // options.ignore — node validates the shape up-front (ERR_INVALID_ARG_TYPE
            // for a non string/RegExp/function/Array value; ERR_INVALID_ARG_VALUE for
            // an empty string). The native (non-recursive) face consumes the option
            // here; the recursive JS layer re-validates through createIgnoreMatcher.
            let mut ignore_v = UndefinedValue();
            JS_GetProperty(
                cx,
                opt_obj.handle().into(),
                c"ignore".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut ignore_v,
                },
            );
            if !ignore_v.is_undefined() && !ignore_v.is_null() {
                let ignore_is_fn =
                    ignore_v.is_object() && unsafe { mozjs_sys::jsapi::js::IsFunctionObject(ignore_v.to_object()) };
                let ignore_ok = (ignore_v.is_string()
                    && !crate::jsstr_to_rust_string(cx, ignore_v.to_string()).is_empty())
                    || (ignore_v.is_object() && !ignore_is_fn
                        && (js_class_name_is(cx, ignore_v, "RegExp") || is_array_object(cx, ignore_v)))
                    || ignore_is_fn;
                if !ignore_ok {
                    if ignore_v.is_string() {
                        let msg = format!(
                            "The property 'options.ignore' must be a non-empty string. Received {}",
                            received_fs_desc(cx, ignore_v)
                        );
                        return throw_fs_coded_error(cx, "ERR_INVALID_ARG_VALUE", "TypeError", &msg);
                    }
                    let msg = format!(
                        "The \"options.ignore\" property must be one of type string or function or an instance of RegExp. Received {}",
                        received_fs_desc(cx, ignore_v)
                    );
                    return throw_fs_coded_error(cx, "ERR_INVALID_ARG_TYPE", "TypeError", &msg);
                }
            }
            // options.signal — AbortSignal consumption (node: an already-aborted
            // signal closes the watcher immediately; a live signal closes it on
            // abort). test-fs-watch-abort-signal asserts 'close' fires in both
            // cases without an explicit watcher.close() call.
            let mut signal_v = UndefinedValue();
            JS_GetProperty(
                cx,
                opt_obj.handle().into(),
                c"signal".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut signal_v,
                },
            );
            signal_opt = Some(signal_v);
            if _argc > 2 {
                listener_val = *args.get(2).ptr;
            }
        } else if opt_val.is_string() {
            // node overload: watch(filename, encodingString[, listener]).
            encoding = crate::jsstr_to_rust_string(cx, opt_val.to_string());
            raw_options_val = opt_val;
            if _argc > 2 {
                listener_val = *args.get(2).ptr;
            }
        } else {
            listener_val = opt_val;
        }
    }

    // Linux recursive → the JS recursive_watch port (node's own architecture).
    // @trace REQ-ENG-004 [api:fs.watch] — recursive face (FS_WATCH_recursive
    // family root: the former ERR_FEATURE_UNAVAILABLE throw) + options.encoding
    // delivery (watch-encoding hang root: filename format never honored →
    // done() unreachable → persistent watcher + interval kept the process alive).
    #[cfg(not(windows))]
    if recursive {
        if !crate::node_internal_fs_modules::ensure_installed(cx) {
            JS_ReportErrorUTF8(
                cx,
                c"fs.watch: recursive watch layer (internal/fs/recursive_watch) failed to install".as_ptr(),
            );
            return false;
        }
        let Some(js_watch) = crate::gc_store::gc_store_get_ns(cx, "fsinternal", "watchRecursive")
        else {
            JS_ReportErrorUTF8(
                cx,
                c"fs.watch: recursive watch layer is unavailable".as_ptr(),
            );
            return false;
        };
        let mut wrapped_cx =
            mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let js_watch_r = js_watch);
        let path_js_str = JS_NewStringCopyZ(cx, ZBox::from_bytes(path.as_bytes()).as_ptr());
        if path_js_str.is_null() {
            args.rval().set(UndefinedValue());
            return false;
        }
        rooted!(&in(cx_ref) let path_val = mozjs::jsval::StringValue(&*path_js_str));
        rooted!(&in(cx_ref) let opts_root = raw_options_val);
        rooted!(&in(cx_ref) let listener_root = listener_val);
// unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
        rooted!(&in(cx_ref) let call_args_rooted = mozjs_sys::jsgc::ValueArray::<3>::new([
            path_val.get(),
            opts_root.get(),
            listener_root.get(),
        ]));
        let call_args = JS::HandleValueArray::from(&call_args_rooted);
        let global = JS::CurrentGlobalOrNull(cx);
        if global.is_null() {
            args.rval().set(UndefinedValue());
            return false;
        }
        rooted!(&in(cx_ref) let global_r = global);
        rooted!(&in(cx_ref) let js_watch_val_r = mozjs::jsval::ObjectValue(js_watch_r.get()));
        let mut rval = UndefinedValue();
        let ok = JS_CallFunctionValue(
            cx,
            global_r.handle().into(),
            js_watch_val_r.handle().into(),
            &call_args,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut rval,
            },
        );
        if !ok {
            return false;
        }
        args.rval().set(rval);
        return true;
    }

    let is_dir = Path::new(&path).metadata().map(|m| m.is_dir()).unwrap_or(false);

    // Register a real watch (kernel events via notify — not polling).
    let id = fsw_next_id();
    let registered = fsw_add_watch_ex(&path, recursive);
    match registered {
        ::std::result::Result::Ok(notify_root) => {
            fsw_register_watch(id, path.clone(), notify_root, is_dir, recursive, persistent, encoding.clone());
            let watcher = fsw_make_watcher_object(cx, id, is_dir, &encoding);
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
// unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
                    rooted!(&in(cx_ref) let call_args_rooted = mozjs_sys::jsgc::ValueArray::<2>::new([
                        mozjs::jsval::StringValue(&*ev_str),
                        lv.get(),
                    ]));
                    let call_args = JS::HandleValueArray::from(&call_args_rooted);
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
            // AbortSignal consumption — node closes the watcher when the
            // signal aborts (test-fs-watch-abort-signal: both the pre-aborted
            // and the abort-after-create cases must emit 'close').
            if let ::std::option::Option::Some(sig_v) = signal_opt {
                fsw_bind_abort_signal(cx, watcher_r.get(), sig_v);
            }
            true
        }
        ::std::result::Result::Err(e) => {
            let _ = fsw_maybe_shutdown_if_idle();
            throw_fs_error(cx, "watch", &path, &e)
        }
    }
}

/// Bind an AbortSignal to a native-face FSWatcher: an already-aborted signal
/// closes the watcher immediately (next tick); a live signal closes it on
/// abort. Best-effort — a malformed signal value leaves the watcher alive
/// (the recursive JS layer validates the shape separately).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn fsw_bind_abort_signal(cx: *mut JSContext, watcher: *mut JSObject, signal: JSVal) {
    if !signal.is_object() {
        return;
    }
    let sig_obj = signal.to_object();
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let watcher_r = watcher);
    rooted!(&in(cx_ref) let sig_r = sig_obj);
    let mut aborted_v = UndefinedValue();
    JS_GetProperty(
        cx,
        sig_r.handle().into(),
        c"aborted".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut aborted_v,
        },
    );
    if aborted_v.is_boolean() && aborted_v.to_boolean() {
        // Pre-aborted: close on the next macrotask tick so the caller can
        // attach a 'close' listener first (node emits close asynchronously).
        let close_src = r#"(function(w) {
  var close = (typeof w.close === 'function') ? w.close : null;
  if (!close) return;
  setTimeout(function() { try { close.call(w); } catch (e) {} }, 0);
})"#;
        let mut src_text = mozjs::rust::transform_str_to_source_text(close_src);
        let mut factory_val = UndefinedValue();
        let factory_h = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut factory_val,
        };
        let opts = mozjs::glue::NewCompileOptions(cx, c"<fs.watch.signal>".as_ptr(), 1);
        if opts.is_null() {
            return;
        }
        let ok = unsafe { mozjs_sys::jsapi::JS::Evaluate2(cx, opts, &mut src_text, factory_h) };
        mozjs::glue::DeleteCompileOptions(opts);
        if !ok || !factory_val.is_object() {
            if !ok {
                JS_ClearPendingException(cx);
            }
            return;
        }
        let global = CurrentGlobalOrNull(cx);
        if global.is_null() {
            return;
        }
        rooted!(&in(cx_ref) let global_root = global);
        rooted!(&in(cx_ref) let factory_root = factory_val);
        rooted!(&in(cx_ref) let watcher_val = mozjs::jsval::ObjectValue(watcher_r.get()));
// unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
        rooted!(&in(cx_ref) let call_args_rooted = mozjs_sys::jsgc::ValueArray::<1>::new([
            watcher_val.get(),
        ]));
        let call_args = JS::HandleValueArray::from(&call_args_rooted);
        let mut call_rval = UndefinedValue();
        unsafe {
            JS_CallFunctionValue(
                cx,
                global_root.handle().into(),
                factory_root.handle().into(),
                &call_args,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut call_rval,
                },
            );
        }
        JS_ClearPendingException(cx);
        return;
    }
    // Live signal: signal.addEventListener('abort', () => watcher.close()).
    let mut add_listener_v = UndefinedValue();
    JS_GetProperty(
        cx,
        sig_r.handle().into(),
        c"addEventListener".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut add_listener_v,
        },
    );
    if !add_listener_v.is_object()
        || !unsafe { mozjs_sys::jsapi::js::IsFunctionObject(add_listener_v.to_object()) }
    {
        return;
    }
    let abort_src = r#"(function(w) {
  return function() {
    var close = (typeof w.close === 'function') ? w.close : null;
    if (!close) return;
    try { close.call(w); } catch (e) {}
  };
})"#;
    let mut src_text2 = mozjs::rust::transform_str_to_source_text(abort_src);
    let mut factory2 = UndefinedValue();
    let factory2_h = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut factory2,
    };
    let opts2 = mozjs::glue::NewCompileOptions(cx, c"<fs.watch.signal>".as_ptr(), 1);
    if opts2.is_null() {
        return;
    }
    let ok2 = unsafe { mozjs_sys::jsapi::JS::Evaluate2(cx, opts2, &mut src_text2, factory2_h) };
    mozjs::glue::DeleteCompileOptions(opts2);
    if !ok2 || !factory2.is_object() {
        if !ok2 {
            JS_ClearPendingException(cx);
        }
        return;
    }
    let global2 = CurrentGlobalOrNull(cx);
    if global2.is_null() {
        return;
    }
    rooted!(&in(cx_ref) let global2_root = global2);
    rooted!(&in(cx_ref) let factory2_root = factory2);
    rooted!(&in(cx_ref) let watcher2_val = mozjs::jsval::ObjectValue(watcher_r.get()));
// unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
    rooted!(&in(cx_ref) let call_args2_rooted = mozjs_sys::jsgc::ValueArray::<1>::new([
        watcher2_val.get(),
    ]));
    let call_args2 = JS::HandleValueArray::from(&call_args2_rooted);
    let mut handler_val = UndefinedValue();
    let handler_ok = unsafe {
        JS_CallFunctionValue(
            cx,
            global2_root.handle().into(),
            factory2_root.handle().into(),
            &call_args2,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut handler_val,
            },
        )
    };
    if !handler_ok || !handler_val.is_object() {
        JS_ClearPendingException(cx);
        return;
    }
    rooted!(&in(cx_ref) let handler_root = handler_val);
    let abort_str = JS_NewStringCopyZ(cx, c"abort".as_ptr());
    if abort_str.is_null() {
        return;
    }
    rooted!(&in(cx_ref) let abort_v = mozjs::jsval::StringValue(&*abort_str));
// unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
    rooted!(&in(cx_ref) let al_call_args_rooted = mozjs_sys::jsgc::ValueArray::<2>::new([
        abort_v.get(),
        handler_root.get(),
    ]));
    let al_call_args = JS::HandleValueArray::from(&al_call_args_rooted);
    rooted!(&in(cx_ref) let al_fn_root = add_listener_v);
    let mut al_rval = UndefinedValue();
    unsafe {
        JS_CallFunctionValue(
            cx,
            sig_r.handle().into(),
            al_fn_root.handle().into(),
            &al_call_args,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut al_rval,
            },
        );
    }
    JS_ClearPendingException(cx);
}

/// `IsArrayObject` wrapper returning a bool (false on failure).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn is_array_object(cx: *mut JSContext, v: JSVal) -> bool {
    if !v.is_object() {
        return false;
    }
    let mut is_arr = false;
    rooted!(in(cx) let arr_root = v);
    let ok = unsafe { IsArrayObject(cx, arr_root.handle().into(), &mut is_arr) };
    ok && is_arr
}

/// Object constructor-name check (the node_util `has_class_name` shape): the
/// Rust-side `instanceof RegExp` stand-in for the ignore-option validation —
/// a RegExp instance's constructor.name is "RegExp" (constructor reads are
/// rooted; the getter surface is the standard property path).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn js_class_name_is(cx: *mut JSContext, v: JSVal, name: &str) -> bool {
    if !v.is_object() {
        return false;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj = v.to_object());
    let mut ctor = UndefinedValue();
    JS_GetProperty(
        cx,
        obj.handle().into(),
        c"constructor".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut ctor,
        },
    );
    if ctor.is_object() {
        rooted!(&in(wrapped_cx) let ctor_obj = ctor.to_object());
        let mut name_val = UndefinedValue();
        JS_GetProperty(
            cx,
            ctor_obj.handle().into(),
            c"name".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut name_val,
            },
        );
        if name_val.is_string() {
            return crate::jsstr_to_rust_string(cx, name_val.to_string()) == name;
        }
    }
    false
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

    let watcher = fsw_make_watcher_object(cx, id, false, "utf8");
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
// unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
            rooted!(&in(cx_ref) let call_args_rooted = mozjs_sys::jsgc::ValueArray::<2>::new([
                mozjs::jsval::StringValue(&*ev_str),
                lv.get(),
            ]));
            let call_args = JS::HandleValueArray::from(&call_args_rooted);
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
// unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
                    rooted!(&in(cx_ref) let call_args_rooted = mozjs_sys::jsgc::ValueArray::<2>::new([
                        mozjs::jsval::StringValue(&*ev_str),
                        lv.get(),
                    ]));
                    let call_args = JS::HandleValueArray::from(&call_args_rooted);
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
// unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
                            rooted!(&in(cx_ref) let call_args_rooted = mozjs_sys::jsgc::ValueArray::<2>::new([
                                mozjs::jsval::ObjectValue(obj_r.get()),
                                mozjs::jsval::StringValue(&*ev2_str),
                            ]));
                            let call_args = JS::HandleValueArray::from(&call_args_rooted);
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
    /// recursive routes through the JS recursive_watch layer before the
    /// backend is reached).
    recursive: bool,
    persistent: bool,
    /// filename delivery encoding (node options.encoding — utf8/buffer/hex/
    /// base64/latin1; consumed by fsw_dispatch_watch_event via content_jsval).
    encoding: ::std::string::String,
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

/// @trace REQ-ENG-004 — process.getActiveResourcesInfo read face: (fs.watch
/// entries, fs.watchFile stat pollers) from the JS-thread registries. node
/// v24 oracle maps the two faces to "FSEventWrap" / "StatWatcher"
/// respectively. JS thread only (thread-local state).
pub fn active_fs_watcher_counts() -> (usize, usize) {
    let watchers = FSW_WATCHERS.with(|w| w.borrow().len());
    let pollers = FSW_POLLERS.with(|p| p.borrow().len());
    (watchers, pollers)
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
    encoding: ::std::string::String,
) {
    FSW_WATCHERS.with(|w| {
        w.borrow_mut().push(FswWatchEntry {
            id,
            path: PathBuf::from(path),
            notify_root,
            is_dir,
            recursive,
            persistent,
            encoding,
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
// unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
            rooted!(&in(cx_ref) let call_args_rooted = mozjs_sys::jsgc::ValueArray::<1>::new([
                mozjs::jsval::StringValue(&*ev_str),
            ]));
            let call_args = JS::HandleValueArray::from(&call_args_rooted);
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
unsafe fn fsw_make_watcher_object(
    cx: *mut JSContext,
    id: u64,
    _is_dir: bool,
    encoding: &str,
) -> *mut JSObject {
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
    let ref_op: JSNative = Some(fsw_ref_native);
    let unref_op: JSNative = Some(fsw_unref_native);
    let stop_op: JSNative = Some(fsw_stop_native);
    for (name, op, nargs) in [
        ("on", on_op, 2u32),
        ("addListener", on_op, 2),
        ("off", off_op, 2),
        ("removeListener", off_op, 2),
        ("once", once_op, 2),
        ("emit", emit_op, 2),
        ("close", close_op, 0),
        // node FSWatcher/StatWatcher proto: ref/unref (+StatWatcher stop —
        // superset on the shared object is additive, node parity tests only
        // require presence + the process-liveness effect).
        ("ref", ref_op, 0),
        ("unref", unref_op, 0),
        ("stop", stop_op, 0),
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
    // Hidden encoding consumed by fsw_dispatch_watch_event (filename delivery).
    let enc_str = JS_NewStringCopyZ(cx, ZBox::from_bytes(encoding.as_bytes()).as_ptr());
    if !enc_str.is_null() {
        rooted!(&in(cx_ref) let enc_v = StringValue(&*enc_str));
        JS_DefineProperty(
            cx,
            watcher.handle().into(),
            c"_fswEncoding".as_ptr(),
            enc_v.handle().into(),
            0,
        );
    }
    crate::gc_store::gc_store_insert_ns(cx, "fswatch", &format!("w{}", id), watcher.get());
    watcher.get()
}

/// watcher.ref() — re-arm process liveness for this entry (node semantics).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fsw_ref_native(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    fsw_set_entry_persistent(cx, &args, true)
}

/// watcher.unref() — this watcher no longer keeps the process alive.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fsw_unref_native(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    fsw_set_entry_persistent(cx, &args, false)
}

unsafe fn fsw_set_entry_persistent(cx: *mut JSContext, args: &CallArgs, persistent: bool) -> bool {
    args.rval().set(UndefinedValue());
    let this_v = *args.thisv().ptr;
    if !this_v.is_object() {
        return true;
    }
    let obj = this_v.to_object();
    let mut id_v = UndefinedValue();
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj_r = obj);
    JS_GetProperty(
        cx,
        obj_r.handle().into(),
        c"_fswId".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut id_v,
        },
    );
    if !id_v.is_int32() {
        return true;
    }
    let id = id_v.to_int32() as u64;
    FSW_WATCHERS.with(|w| {
        if let Some(entry) = w.borrow_mut().iter_mut().find(|e| e.id == id) {
            entry.persistent = persistent;
        }
    });
    FSW_POLLERS.with(|p| {
        if let Some(entry) = p.borrow_mut().iter_mut().find(|e| e.id == id) {
            entry.persistent = persistent;
        }
    });
    true
}

/// StatWatcher.stop() — stop polling and emit 'stop' (node watchers.js:
/// stop closes the handle then emits 'stop' on nextTick; the event must fire
/// even with no other liveness holders).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fsw_stop_native(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    let this_v = *args.thisv().ptr;
    if !this_v.is_object() {
        return true;
    }
    let obj = this_v.to_object();
    let mut id_v = UndefinedValue();
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj_r = obj);
    JS_GetProperty(
        cx,
        obj_r.handle().into(),
        c"_fswId".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut id_v,
        },
    );
    if !id_v.is_int32() {
        return true;
    }
    let id = id_v.to_int32() as u64;
    fsw_close_entry(cx, id);
    // emit('stop') — direct on this watcher object.
    let c_ev = ZBox::from_bytes("stop".as_bytes());
    let ev_str = JS_NewStringCopyZ(cx, c_ev.as_ptr());
    if ev_str.is_null() {
        return true;
    }
    rooted!(&in(wrapped_cx) let ev_root = StringValue(&*ev_str));
    // unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
    rooted!(&in(wrapped_cx) let call_args_rooted = mozjs_sys::jsgc::ValueArray::<1>::new([
        ev_root.get(),
    ]));
    let call_args = JS::HandleValueArray::from(&call_args_rooted);
    let c_emit = ZBox::from_bytes("emit".as_bytes());
    let mut rval = UndefinedValue();
    let ok = JS_CallFunctionName(
        cx,
        obj_r.handle().into(),
        c_emit.as_ptr(),
        &call_args,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        },
    );
    if !ok {
        JS_ClearPendingException(cx);
    }
    true
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
    // Rooting discipline: every JS value created here is rooted before the
    // next allocation runs (a later allocation can trigger a moving GC), and
    // the emit args ride a rooted ValueArray (unrooted-args class,
    // B-sweep 2026-10-26) — the former bare-Vec argv crossed the
    // content_jsval allocation and the JS_CallFunctionName GC point unrooted.
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(raw_cx));
    let cx_ref = &mut wrapped_cx;
    let c_type = ZBox::from_bytes(event_type.as_bytes());
    let type_str = JS_NewStringCopyZ(raw_cx, c_type.as_ptr());
    rooted!(&in(cx_ref) let type_root = if type_str.is_null() {
        mozjs::jsval::UndefinedValue()
    } else {
        mozjs::jsval::StringValue(&*type_str)
    });
    // node: filename is delivered per options.encoding — 'buffer' → Buffer of
    // the utf8 bytes, 'hex'/'base64' → encoded form of those bytes, default
    // utf8 STRING. test-fs-watch-encoding asserts all three faces (hex bytes
    // of 新建文夹件.txt, utf8 string, Buffer instance). content_jsval's None
    // arm is the readFile Buffer default — the watch face's default is the
    // utf8 string (node: the listener's filename is string|null; the former
    // None mapping delivered Buffers to every default watch, breaking
    // `filename.startsWith`/`===` consumers — test-fs-promises-watch-ignore-*).
    // The watcher entry owns the encoding so unref'd/closed entries keep their
    // format consistent.
    let encoding = FSW_WATCHERS.with(|w| {
        w.borrow()
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.encoding.clone())
            .unwrap_or_else(|| "utf8".to_string())
    });
    rooted!(&in(cx_ref) let filename_root = match filename {
        Some(name) => {
            let enc: &str = if encoding.is_empty() { "utf8" } else { encoding.as_str() };
            content_jsval(raw_cx, name.as_bytes(), ::std::option::Option::Some(enc))
        }
        None => mozjs::jsval::NullValue(),
    });
// unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
    rooted!(&in(cx_ref) let emit_args = mozjs_sys::jsgc::ValueArray::<2>::new([
        type_root.get(),
        filename_root.get(),
    ]));
    let call_args = JS::HandleValueArray::from(&emit_args);
    fsw_emit_on_entry(raw_cx, id, "change", &call_args);
}

/// Fire one watchFile stat change on its StatWatcher: (curr, prev) Stats.
/// Each Stats object is rooted before the next allocation runs (building the
/// second Stats object can trigger a moving GC), and the emit args ride a
/// rooted ValueArray (unrooted-args class).
unsafe fn fsw_dispatch_stat_event(
    raw_cx: *mut JSContext,
    id: u64,
    prev: libc::stat,
    curr: libc::stat,
) {
    let curr_obj = build_stats_object(raw_cx, &curr, false);
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(raw_cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let curr_root = if curr_obj.is_null() {
        mozjs::jsval::UndefinedValue()
    } else {
        mozjs::jsval::ObjectValue(curr_obj)
    });
    let prev_obj = build_stats_object(raw_cx, &prev, false);
    rooted!(&in(cx_ref) let prev_root = if prev_obj.is_null() {
        mozjs::jsval::UndefinedValue()
    } else {
        mozjs::jsval::ObjectValue(prev_obj)
    });
// unrooted-args class (B-sweep 2026-10-26, require.rs golden): rooted ValueArray.
    rooted!(&in(cx_ref) let emit_args = mozjs_sys::jsgc::ValueArray::<2>::new([
        curr_root.get(),
        prev_root.get(),
    ]));
    let call_args = JS::HandleValueArray::from(&emit_args);
    fsw_emit_on_entry(raw_cx, id, "change", &call_args);
}

unsafe fn fsw_emit_on_entry(
    raw_cx: *mut JSContext,
    id: u64,
    event: &str,
    call_args: &JS::HandleValueArray,
) {
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
    // The caller's args arrive as a HandleValueArray over caller-frame roots;
    // the event-name root must precede them, so rebuild the array here over
    // this frame's root (the caller roots stay alive across the call).
    let ev_val = ev_root.get();
    let arg_ptrs: ::std::vec::Vec<mozjs::jsval::JSVal> = {
        let mut v = ::std::vec::Vec::with_capacity(call_args.length_ as usize + 1);
        v.push(ev_val);
        for i in 0..call_args.length_ {
            v.push(*call_args.elements_.add(i as usize));
        }
        v
    };
    rooted!(&in(cx_ref) let full_args = mozjs_sys::jsgc::ValueArray::<3>::new([
        arg_ptrs[0],
        arg_ptrs.get(1).copied().unwrap_or(mozjs::jsval::UndefinedValue()),
        arg_ptrs.get(2).copied().unwrap_or(mozjs::jsval::UndefinedValue()),
    ]));
    let full_handle = JS::HandleValueArray::from(&full_args);
    let mut rval = UndefinedValue();
    let ok = JS_CallFunctionName(
        raw_cx,
        obj_r.handle().into(),
        c_emit.as_ptr(),
        &full_handle,
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
    // node: readFile accepts an fd number in place of a path
    // (test-fs-readfile-fd) — reads run from the fd's CURRENT position.
    let target = match get_read_target_arg(cx, &args, 0) {
        ::std::result::Result::Ok(t) => t,
        ::std::result::Result::Err(b) => return b,
    };
    let (encoding_flag, flag): (::std::option::Option<::std::string::String>, ::std::option::Option<::std::string::String>) =
        write_file_options(cx, &args, 1);
    // Error label minted ONCE up front: every match below borrows `target`
    // (E0382 — the former shape moved Path's String in the callback branch
    // and then matched `target` again in the sync tail / error arm).
    let target_label = match &target {
        FsReadTarget::Path(p) => p.clone(),
        FsReadTarget::Fd(fd) => format!("fd:{}", fd),
    };

    // Callback form (Node async-first API): route through the ConcurrentTask
    // carrier — the callback receives (err, data) on the JS thread.
    if let Some((callback, encoding)) = extract_callback_and_encoding(cx, &args, 1) {
        let encoding = encoding.or(encoding_flag);
        match &target {
            FsReadTarget::Fd(fd) => {
                let fd = *fd;
                spawn_fs_async(
                    cx,
                    "readFile",
                    format!("fd:{}", fd),
                    callback,
                    encoding,
                    move || read_fd_to_eof(fd).map(FsAsyncResult::Ok),
                );
            }
            FsReadTarget::Path(path) => {
                let label = path.clone();
                let path = path.clone();
                let read = move || match flag.as_deref() {
                    ::std::option::Option::None | ::std::option::Option::Some("r") => {
                        bun_fs::read(&path).map(FsAsyncResult::Ok)
                    }
                    ::std::option::Option::Some(f) => {
                        read_file_with_flag(&path, f).map(FsAsyncResult::Ok)
                    }
                };
                spawn_fs_async(cx, "readFile", label, callback, encoding, read);
            }
        }
        args.rval().set(UndefinedValue());
        return true;
    }

    let encoding = get_encoding_opt(cx, &args, 1);
    let data = match &target {
        FsReadTarget::Fd(fd) => read_fd_to_eof(*fd),
        FsReadTarget::Path(path) => {
            if let ::std::result::Result::Err(e) = crate::permission_bridge::check_fs_read(path) {
                let c_msg = ZBox::from_bytes(e.as_bytes());
                JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
                return false;
            }
            match flag.as_deref() {
                ::std::option::Option::None | ::std::option::Option::Some("r") => bun_fs::read(path),
                ::std::option::Option::Some(f) => read_file_with_flag(path, f),
            }
        }
    };
    match data {
        ::std::result::Result::Ok(data) => {
            return_string_content(cx, &args, &data, encoding.as_deref())
        }
        ::std::result::Result::Err(e) => {
            throw_fs_error(cx, "readFile", &target_label, &e)
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_write_file(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let target = match get_write_target_arg(cx, &args, 0) {
        ::std::result::Result::Ok(t) => t,
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

    match target {
        FsWriteTarget::Fd(fd) => {
            spawn_fs_async(cx, "writeFile", format!("fd:{}", fd), callback, None, move || {
                write_bytes_to_fd(fd, &bytes).map(|_| FsAsyncResult::OkVoid)
            });
        }
        FsWriteTarget::Path(path) => {
            spawn_fs_async(cx, "writeFile", path.clone(), callback, None, move || {
                write_file_with_flag(&path, &bytes, &flag).map(|_| FsAsyncResult::OkVoid)
            });
        }
    }
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
    let mode = match get_mkdir_mode_option(cx, &args, 1) {
        ::std::result::Result::Ok(m) => m,
        ::std::result::Result::Err(b) => return b,
    };
    let result = fs_mkdir_with_mode(&path, recursive, mode);
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
// unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
                    rooted!(&in(cx_ref_err) let err_args_rooted = mozjs_sys::jsgc::ValueArray::<1>::new([
                        err_val.get(),
                    ]));
                    let err_args = JS::HandleValueArray::from(&err_args_rooted);
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
    let target = match get_write_target_arg(cx, &args, 0) {
        ::std::result::Result::Ok(t) => t,
        ::std::result::Result::Err(b) => return b,
    };
    // node: appendFile(fd, data) writes at the fd's current position.
    if let FsWriteTarget::Fd(fd) = target {
        let data_val = if argc > 1 {
            *args.get(1).ptr
        } else {
            UndefinedValue()
        };
        let encoding = get_encoding_opt(cx, &args, 2);
        let data = write_file_data_bytes(cx, data_val, encoding.as_deref());
        if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 2) {
            spawn_fs_async(cx, "appendFile", format!("fd:{}", fd), callback, None, move || {
                write_bytes_to_fd(fd, &data).map(|_| FsAsyncResult::OkVoid)
            });
            args.rval().set(UndefinedValue());
            return true;
        }
        return match write_bytes_to_fd(fd, &data) {
            ::std::result::Result::Ok(_) => {
                args.rval().set(UndefinedValue());
                true
            }
            ::std::result::Result::Err(e) => throw_fs_error(
                cx,
                "appendFile",
                &format!("fd:{}", fd),
                &e,
            ),
        };
    }
    let path = match target {
        FsWriteTarget::Path(p) => p,
        FsWriteTarget::Fd(_) => unreachable!(),
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

    if !cli_perm_read_gate_callback(cx, &args, 1, &path) {
        return false;
    }
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
    // node getValidatedFd: a NON-number fd is ERR_INVALID_ARG_TYPE (the
    // former `-1` fallback sent '', null, {} … into the syscall and surfaced
    // EBADF instead of the TypeError — test-fs-fsync). Integers (incl. -1)
    // still reach the syscall and report EBADF like node.
    let fd = match fs_validated_fd(cx, fd_val) {
        ::std::result::Result::Ok(fd) => fd,
        ::std::result::Result::Err(b) => return b,
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

    if !cli_perm_read_gate_callback(cx, &args, 1, &path) {
        return false;
    }
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
    // node getValidatedFd (test-fs-fsync): non-number fd → ERR_INVALID_ARG_TYPE.
    let fd = match fs_validated_fd(cx, fd_val) {
        ::std::result::Result::Ok(fd) => fd,
        ::std::result::Result::Err(b) => return b,
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
            // spawn-based async fstat: no bigint option channel yet (same
            // registered residual as the async stat dispatch above).
            let stats = create_stats_object(cx, &posix, false);
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
            let stats = create_stats_object(cx, &posix, stat_bigint_option(cx, &args, 1));
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
    let atime = match parse_fs_time_arg(cx, atime_val, "atime") {
        ::std::result::Result::Ok(t) => t,
        ::std::result::Result::Err(b) => return b,
    };
    let mtime = match parse_fs_time_arg(cx, mtime_val, "mtime") {
        ::std::result::Result::Ok(t) => t,
        ::std::result::Result::Err(b) => return b,
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
                    futimens_fd(fd, atime, mtime).map(|()| FsAsyncResult::OkVoid)
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
        match futimens_fd(fd, atime, mtime) {
            ::std::result::Result::Ok(()) => {
                args.rval().set(UndefinedValue());
                true
            }
            ::std::result::Result::Err(e) => throw_fs_error(
                cx,
                "futimes",
                &format!("fd:{}", fd),
                &e,
            ),
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

    if !cli_perm_read_gate_callback(cx, &args, 1, &path) {
        return false;
    }
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
            // async lstat no-callback fallback: no bigint option channel yet.
            let stats = create_stats_object(cx, &posix, false);
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
    let atime = match parse_fs_time_arg(cx, atime_val, "atime") {
        ::std::result::Result::Ok(t) => t,
        ::std::result::Result::Err(b) => return b,
    };
    let mtime = match parse_fs_time_arg(cx, mtime_val, "mtime") {
        ::std::result::Result::Ok(t) => t,
        ::std::result::Result::Err(b) => return b,
    };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 3) {
        spawn_fs_async(cx, "lutimes", path.clone(), callback, None, move || {
            #[cfg(unix)]
            {
                utimes_path(&path, atime, mtime, false).map(|()| FsAsyncResult::OkVoid)
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
        match utimes_path(&path, atime, mtime, false) {
            ::std::result::Result::Ok(()) => {
                args.rval().set(UndefinedValue());
                true
            }
            ::std::result::Result::Err(e) => throw_fs_error(cx, "lutimes", &path, &e),
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
    if !cli_perm_read_gate_callback(cx, &args, 2, &path) {
        return false;
    }
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

    if !cli_perm_read_gate_callback(cx, &args, 1, &path) {
        return false;
    }
    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 1) {
        // With callback: create Dir object and pass to callback
        let dir_obj = create_dir_object(cx, &path);
        let mut wrapped_cx_od =
            mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        let cx_ref_od = &mut wrapped_cx_od;
        rooted!(&in(cx_ref_od) let cb = callback);
        rooted!(&in(cx_ref_od) let cb_val = mozjs::jsval::ObjectValue(cb.get()));
        rooted!(&in(cx_ref_od) let dir_val = mozjs::jsval::ObjectValue(dir_obj));
// unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
        rooted!(&in(cx_ref_od) let cb_args_rooted = mozjs_sys::jsgc::ValueArray::<2>::new([
            UndefinedValue(),
            dir_val.get(),
        ]));
        let cb_args = JS::HandleValueArray::from(&cb_args_rooted);
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
    if argc == 0 {
        // node oracle (test-fs-read): fs.read() with NO arguments throws
        // ERR_MISSING_ARGS (the callback gate) — the former path fell into
        // the fd gate and reported ERR_INVALID_ARG_TYPE instead.
        return throw_fs_coded_error(
            cx,
            "ERR_MISSING_ARGS",
            "TypeError",
            "The \"callback\" argument must be specified",
        );
    }
    // node validates fd/buffer/window BEFORE requiring the callback
    // (oracle: fs.read(null, buf, 0, 1, 0) reports the "fd" type error even
    // though the callback is also missing).
    let (buf_obj, offset, length, use_pread, position) =
        match parse_read_window_args(cx, &args, ::std::option::Option::Some(0), 1, ReadWindowFace::Callback) {
            ::std::result::Result::Ok(w) => w,
            ::std::result::Result::Err(()) => return false,
        };
    let callback = match extract_callback_and_encoding(cx, &args, 1) {
        ::std::option::Option::Some((cb, _)) => cb,
        ::std::option::Option::None => {
            let msg = format!(
                "The \"cb\" argument must be of type function. Received {}",
                received_fs_desc(
                    cx,
                    if argc > 0 {
                        *args.get(argc - 1).ptr
                    } else {
                        UndefinedValue()
                    }
                )
            );
            return throw_fs_coded_error(cx, "ERR_INVALID_ARG_TYPE", "TypeError", &msg);
        }
    };
    let fd = fd_from_args(&args);

    // node contract: the callback receives the SAME buffer object the caller
    // passed, filled at [offset, offset + bytesRead). Root the RESOLVED buffer
    // (the options-buffer overload resolves it out of options.buffer — rooting
    // argv[1] there would root the options object and the completion writeback
    // would silently miss); fail closed (rather than silently not filling)
    // when rooting fails. The rooted guard drives the completion writeback.
    let buf_write_val = mozjs::jsval::ObjectValue(buf_obj);
    let writeback: ::std::option::Option<RawValueRootGuard> =
        match RawValueRootGuard::new(cx, ::std::slice::from_ref(&buf_write_val), c"fs_read_buffer") {
            ::std::option::Option::Some(g) => ::std::option::Option::Some(g),
            ::std::option::Option::None => {
                JS_ReportErrorUTF8(cx, c"read: failed to root the caller buffer".as_ptr());
                return false;
            }
        };
    spawn_fs_async_writeback(
        cx,
        "read",
        format!("fd:{}", fd),
        callback,
        None,
        writeback,
        offset,
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
                    unsafe { libc::read(fd, buf.as_mut_ptr() as *mut ::std::ffi::c_void, length) }
                };
                if bytes_read >= 0 {
                    buf.truncate(bytes_read as usize);
                    ::std::result::Result::Ok(FsAsyncResult::OkRead {
                        bytes_read: bytes_read as i32,
                        buffer: buf,
                    })
                } else {
                    ::std::result::Result::Err(::std::io::Error::last_os_error())
                }
            }
            #[cfg(not(unix))]
            {
                let _ = (use_pread, position);
                ::std::result::Result::Ok(FsAsyncResult::OkRead {
                    bytes_read: 0,
                    buffer: buf,
                })
            }
        },
    );
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_readdir(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    if !cli_perm_read_gate_callback(cx, &args, 1, &path) {
        return false;
    }
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
    if !cli_perm_read_gate_callback(cx, &args, 1, &path) {
        return false;
    }
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
        // Checked view extraction (view_byte_slot): a non-view entry (URL,
        // plain object) previously hit the unchecked typed-array accessors —
        // the same class-confusion SIGSEGV as the fs path face (REQ-ENG-007).
        let Some((_data, len)) = view_byte_slot(elem) else {
            JS_ReportErrorUTF8(cx, c"readv: buffers must be an array of Buffers".as_ptr());
            return false;
        };
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
    if !cli_perm_read_gate_callback(cx, &args, 1, &path) {
        return false;
    }
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

    if !cli_perm_read_gate_callback(cx, &args, 1, &path) {
        return false;
    }
    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 1) {
        spawn_fs_async(cx, "stat", path.clone(), callback, None, move || {
            bun_fs::metadata(&path).map(FsAsyncResult::OkStat)
        });
        args.rval().set(UndefinedValue());
        return true;
    }

    match bun_fs::metadata(&path) {
        ::std::result::Result::Ok(meta) => {
            // async stat no-callback fallback: no bigint option channel yet.
            let stats = create_stats_object(cx, &meta, false);
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
    let atime = match parse_fs_time_arg(cx, atime_val, "atime") {
        ::std::result::Result::Ok(t) => t,
        ::std::result::Result::Err(b) => return b,
    };
    let mtime = match parse_fs_time_arg(cx, mtime_val, "mtime") {
        ::std::result::Result::Ok(t) => t,
        ::std::result::Result::Err(b) => return b,
    };

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 3) {
        spawn_fs_async(cx, "utimes", path.clone(), callback, None, move || {
            #[cfg(unix)]
            {
                utimes_path(&path, atime, mtime, true).map(|()| FsAsyncResult::OkVoid)
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
        match utimes_path(&path, atime, mtime, true) {
            ::std::result::Result::Ok(()) => {
                args.rval().set(UndefinedValue());
                true
            }
            ::std::result::Result::Err(e) => throw_fs_error(cx, "utimes", &path, &e),
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

promise_simple_op!(fs_promises_unlink, |p: &str| fs::remove_file(p), "unlink");

/// fs.promises.mkdir(path[, options]) — dedicated body (the former
/// promise_simple_op forced create_dir_all: options.recursive was ignored
/// (an existing target never surfaced EEXIST) and options.mode was dropped).
/// Node contract: recursive defaults false (mkdir on an existing path
/// rejects EEXIST), mode applies to every created directory.
///
/// @trace REQ-ENG-007 [api:fs.mkdir] — promises face (the former
/// promise_simple_op forced create_dir_all: recursive/mode both dropped).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_mkdir(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let recursive = get_bool_option(cx, &args, 1, "recursive");
    let mode = match get_mkdir_mode_option(cx, &args, 1) {
        ::std::result::Result::Ok(m) => m,
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
    match fs_mkdir_with_mode(&path, recursive, mode) {
        ::std::result::Result::Ok(()) => {
            resolve_undefined(cx, promise.get());
        }
        ::std::result::Result::Err(e) => {
            reject_fs_error(cx, promise.get(), "mkdir", &path, &e);
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

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

/// fs.promises.cp / `require("fs/promises").cp(src, dest[, options])` —
/// promise face over the shared options-driven cp engine (the same engine
/// fs.cp's callback face and fs.cpSync run on). @trace REQ-ENG-007 — full
/// node-parity rewrite (the former core consumed only errorOnExist and
/// ignored every other option). Per node internal/fs/promises.js cp(), ALL
/// rejections ride the promise — including options validation
/// (ERR_INVALID_ARG_TYPE / ERR_OUT_OF_RANGE / ERR_INCOMPATIBLE_OPTION_PAIR),
/// because node wraps them inside the async function body.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_cp(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let from = match cp_face_path_arg(cx, &args, 0, "src") {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let to = match cp_face_path_arg(cx, &args, 1, "dest") {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let opts_val = if args.argc_ > 2 {
        *args.get(2).ptr
    } else {
        UndefinedValue()
    };
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    // Heap-root the options value BEFORE parse runs getters (GC can move the
    // object): the guard copies it into its GC-tracked slot now, and parse
    // consumes the slot — never the stale stack copy
    // (raw-JSVal-across-GC-points class). None = registration failed (OOM) —
    // degraded path: parse from the snapshot.
    // Heap-root the options value BEFORE parse runs getters (GC can move the
    // object): the guard copies it into its GC-tracked slot now, and parse
    // consumes the slot — never the stale stack copy
    // (raw-JSVal-across-GC-points class). Registration failure (OOM) fails
    // LOUD — the bare snapshot would ride unrooted across the engine (no
    // silent degraded path; fallback ban).
    let opts_root = match RawValueRootGuard::new(cx, ::std::slice::from_ref(&opts_val), c"cp_opts")
    {
        ::std::option::Option::Some(g) => g,
        ::std::option::Option::None => {
            JS_ReportErrorUTF8(cx, c"cp: failed to GC-root the options value".as_ptr());
            return false;
        }
    };
    let opts = match cp_parse_options(cx, opts_root.get(0)) {
        ::std::result::Result::Ok(o) => o,
        ::std::result::Result::Err(e) => {
            drop(opts_root);
            cp_reject(cx, promise.get(), &e);
            args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
            return true;
        }
    };
    let run = CpRun {
        cx,
        opts: &opts,
        opts_root: ::std::option::Option::Some(&opts_root),
        opts_fallback: opts_val,
    };
    let result = cp_engine(&run, &from, &to);
    drop(opts_root);
    match result {
        ::std::result::Result::Ok(()) => resolve_undefined(cx, promise.get()),
        ::std::result::Result::Err(e) => cp_reject(cx, promise.get(), &e),
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

/// fs.promises.cpSync / `require("fs/promises").cpSync(src, dest[, options])`
/// — promise face over the same options-driven cp engine (task-scoped
/// registration; node's promises namespace has no cpSync, bao's table shape is
/// uniform: every entry is a promise-returning native).
/// @trace REQ-ENG-007 — same engine + rejection discipline as fs_promises_cp.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_cp_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let from = match cp_face_path_arg(cx, &args, 0, "src") {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let to = match cp_face_path_arg(cx, &args, 1, "dest") {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let opts_val = if args.argc_ > 2 {
        *args.get(2).ptr
    } else {
        UndefinedValue()
    };
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    // Heap-root the options value BEFORE parse runs getters (GC can move the
    // object): the guard copies it into its GC-tracked slot now, and parse
    // consumes the slot — never the stale stack copy
    // (raw-JSVal-across-GC-points class). Registration failure (OOM) fails
    // LOUD — the bare snapshot would ride unrooted across the engine (no
    // silent degraded path; fallback ban).
    let opts_root = match RawValueRootGuard::new(cx, ::std::slice::from_ref(&opts_val), c"cp_opts")
    {
        ::std::option::Option::Some(g) => g,
        ::std::option::Option::None => {
            JS_ReportErrorUTF8(cx, c"cp: failed to GC-root the options value".as_ptr());
            return false;
        }
    };
    let opts = match cp_parse_options(cx, opts_root.get(0)) {
        ::std::result::Result::Ok(o) => o,
        ::std::result::Result::Err(e) => {
            drop(opts_root);
            cp_reject(cx, promise.get(), &e);
            args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
            return true;
        }
    };
    let run = CpRun {
        cx,
        opts: &opts,
        opts_root: ::std::option::Option::Some(&opts_root),
        opts_fallback: opts_val,
    };
    let result = cp_engine(&run, &from, &to);
    drop(opts_root);
    match result {
        ::std::result::Result::Ok(()) => resolve_undefined(cx, promise.get()),
        ::std::result::Result::Err(e) => cp_reject(cx, promise.get(), &e),
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
    if !cli_perm_read_gate_promise(cx, promise.get(), &path) {
        args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
        return true;
    }

    match bun_fs::read(&path) {
        ::std::result::Result::Ok(data) => {
            // node: fs.promises.readFile(fn) with NO encoding resolves a
            // Buffer (binary-safe) — assert.ok(Buffer) truthiness + empty-file
            // corpus contract. The former string_or_buffer(None) returned a
            // utf8 String ('' == true assertion failure in
            // test-fs-promises-readfile-empty).
            let val = content_jsval(cx, &data, encoding.as_deref());
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
    if !cli_perm_read_gate_promise(cx, promise.get(), &path) {
        args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
        return true;
    }
    match bun_fs::metadata(&path) {
        ::std::result::Result::Ok(meta) => {
            let stats = create_stats_object(cx, &meta, stat_bigint_option(cx, &args, 1));
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
    if !cli_perm_read_gate_promise(cx, promise.get(), &path) {
        args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
        return true;
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
    if !cli_perm_read_gate_promise(cx, promise.get(), &path) {
        args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
        return true;
    }
    match fs::symlink_metadata(&path) {
        ::std::result::Result::Ok(meta) => {
            let posix = metadata_to_posix_stat(&meta);
            let stats = create_stats_object(cx, &posix, stat_bigint_option(cx, &args, 1));
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
    if !cli_perm_read_gate_promise(cx, promise.get(), &path) {
        args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
        return true;
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
    if !cli_perm_read_gate_promise(cx, promise.get(), &path) {
        args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
        return true;
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
    if !cli_perm_read_gate_promise(cx, promise.get(), &path) {
        args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
        return true;
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
    // node time-arg contract: number (seconds) | Date — the former body read
    // raw numbers only and silently set a Date arg to EPOCH 0 (every
    // fsPromises.utimes(p, new Date(…)) call zeroed the timestamps —
    // test-fs-stat-date's whole runTest ladder drives the Date face).
    let atime = match parse_fs_time_arg(cx, atime_val, "atime") {
        ::std::result::Result::Ok(t) => t,
        ::std::result::Result::Err(b) => return b,
    };
    let mtime = match parse_fs_time_arg(cx, mtime_val, "mtime") {
        ::std::result::Result::Ok(t) => t,
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
    match utimes_path(&path, atime, mtime, true) {
        ::std::result::Result::Ok(()) => {
            resolve_undefined(cx, promise.get());
        }
        ::std::result::Result::Err(e) => {
            reject_fs_error(
                cx,
                promise.get(),
                "utimes",
                &path,
                &e,
            );
        }
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
    if !cli_perm_read_gate_promise(cx, promise.get(), &path) {
        args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
        return true;
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
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = unsafe { mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()) });
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    // Full node overload resolution + validation (positional /
    // (fd, buffer, options) / (fd, options) forms) — the async face surfaces
    // validation failures as promise rejections (node oracle:
    // fs.promises.read(fd, b, 0, 4, NaN) rejects ERR_OUT_OF_RANGE) — and the
    // node result shape {bytesRead, buffer}.
    let parsed = parse_read_window_args(cx, &args, ::std::option::Option::Some(0), 1, ReadWindowFace::Promise);
    let (resolved_buf, offset, length, use_pread, position) = match parsed {
        ::std::result::Result::Ok(w) => w,
        ::std::result::Result::Err(()) => {
            let mut exn = UndefinedValue();
            let got = unsafe {
                JS_GetPendingException(
                    cx,
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut exn,
                    },
                )
            };
            unsafe { JS_ClearPendingException(cx) };
            if got && !exn.is_undefined() {
                rooted!(&in(cx_ref) let exn_root = exn);
                unsafe {
                    mozjs_sys::jsapi::JS::RejectPromise(
                        cx,
                        promise.handle().into(),
                        exn_root.handle().into(),
                    );
                }
            } else {
                reject_with_error(cx, promise.get(), "read: invalid arguments");
            }
            args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
            return true;
        }
    };
    let fd = fd_from_args(&args);
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
            // caller's buffer window [offset, offset + bytesRead) — the RESOLVED
            // buffer (the options form carries it inside options.buffer).
            {
                rooted!(&in(cx_ref) let caller_rb = resolved_buf);
                let off_rb: u32 = offset as u32;
                for (i_rb, &byte_rb) in buf.iter().enumerate() {
                    rooted!(&in(cx_ref) let bv_rb = mozjs::jsval::Int32Value(byte_rb as i32));
                    JS_SetElement(
                        cx,
                        caller_rb.handle().into(),
                        off_rb + i_rb as u32,
                        bv_rb.handle().into(),
                    );
                }
            }
            // node result shape: { bytesRead, buffer } (the fresh Buffer).
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
    if !cli_perm_read_gate_sync(cx, &path) {
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

    if !cli_perm_read_gate_callback(cx, &args, 1, &path) {
        return false;
    }
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
    if !cli_perm_read_gate_promise(cx, promise.get(), &path) {
        args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
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
    if !cli_perm_read_gate_sync(cx, &path) {
        return false;
    }
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
    // Same node getValidatedFd contract as the async face (test-fs-fsync:
    // non-number fd → ERR_INVALID_ARG_TYPE TypeError, not EBADF).
    let fd = match fs_validated_fd(cx, fd_val) {
        ::std::result::Result::Ok(fd) => fd,
        ::std::result::Result::Err(b) => return b,
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
    if !cli_perm_read_gate_sync(cx, &path) {
        return false;
    }
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

/// node read-family argument resolver. Handles every node overload of
/// read/readSync/FileHandle.read:
///   positional:   (fd, buffer[, offset[, length[, position]]])
///   options:      (fd, buffer, {offset, length, position})   [argc <= 3, or
///                  any argc on the callback face where the cb trails]
///   opts+buffer:  (fd, {buffer, offset, length, position})   [callback face
///                  only — readSync's 2-arg form treats the object as the
///                  buffer itself (oracle: readSync(fd, {buffer}) → the
///                  buffer-type error, never the options parse)]
///
/// Validation contract (node v24.19.0 oracle):
///   • buffer not a view → ERR_INVALID_ARG_TYPE 'The "buffer" argument must be
///     an instance of Buffer, TypedArray, or DataView. Received …'
///   • offset non-integer → ERR_OUT_OF_RANGE '… It must be an integer. …';
///     offset < 0 → '… It must be >= 0 && <= 9007199254740991. …'
///   • length < 0 → '… It must be >= 0. …'; length > window → '… It must be
///     <= N. Received M'
///   • empty buffer (byteLength 0) with length > 0 → ERR_INVALID_ARG_VALUE
///     "The argument 'buffer' is empty and cannot be written. Received …"
///   • position: bigint | integer only (else ERR_INVALID_ARG_TYPE 'The
///     "position" argument must be of type bigint or integer'); non-integer
///     numbers / < -1 → ERR_OUT_OF_RANGE (number bound MAX_SAFE_INTEGER,
///     bigint bound 2^63-1); null/undefined/-1 → cursor (pread off)
///   • argc <= 3 with a non-object (and non-nullish) third slot →
///     ERR_INVALID_ARG_TYPE 'The "options" argument must be of type object. …'
///     (oracle: readSync(fd, buf, 0) is an OPTIONS error, not an offset)
/// Returns Err(()) with the coded exception already pending.
///
// @trace REQ-ENG-007 [api:fs.read|fs.readSync|fs.promises.read|FileHandle.read]
// — the read-family node-validation contract (position/offset/length/buffer
// gates + the options-object overloads) shared by all four faces.
/// Which fs.read overload family is parsing. The trailing-callback overload
/// exists ONLY on the callback face (node v24.19.0 oracle):
///   read(fd, b, cb)           → (fd, buffer, cb): offset/length/position default
///   read(fd, b, 0, cb)        → 4-arg overload is (fd, buffer, OPTIONS, cb) —
///                               the number offset reports the OPTIONS type
///   read(fd, b, 0, 4, cb)     → 5-arg overload is (fd, buffer, offset,
///                               length, cb) — the fn at the position slot IS
///                               the callback, position defaults
///   read(fd, b, 0, 4, fn, cb) → 6-arg — the fn at the position slot is
///                               VALIDATED → ERR_INVALID_ARG_TYPE (thrown
///                               synchronously)
/// The promises faces have no callback overload at all:
///   fh.read(b, 0, 4, fn) / fsp.read(fd, b, 0, 4, fn) → reject
///   ERR_INVALID_ARG_TYPE (the former shared parser swallowed the fn as a
///   "trailing callback" on every face → "Missing expected rejection").
///
/// @trace REQ-ENG-007 [api:fs.read|fs.readSync|fs.promises.read|FileHandle.read]
/// — face-aware overload resolution for the shared read-window parser.
#[derive(::std::cmp::PartialEq, ::std::cmp::Eq)]
enum ReadWindowFace {
    /// fs.read — trailing-callback overload-resolution rules above.
    Callback,
    /// fs.promises.read / FileHandle.read — every window value is validated.
    Promise,
    /// readSync — no callback; the positional length defaults to 0.
    Sync,
}

/// Classify the read-options slot through proxies: 'array' (spec IsArray —
/// target-recursive) | 'function' (typeof) | 'other'. Consumed only when the
/// JS_IsProxy class probe gates the slot; the SM class checks
/// (IsArrayObject/IsFunctionObject) are proxy-blind, so a Proxy-wrapped array
/// or function otherwise slides through as a plain options object and
/// readSync silently returns (test-fs-readSync-optional-params hands the test
/// mustNotMutateObjectDeep Proxies). The classifier (Array.isArray + typeof)
/// invokes NO proxy traps — user code never runs; a revoked proxy throws
/// (spec IsArray), propagated as the pending exception. The helper function
/// object is stashed in gc_store "fsinternal" by install_fs_stream_polyfill.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn classify_options_value(
    cx: *mut JSContext,
    v: JSVal,
) -> ::std::result::Result<&'static str, ()> {
    let Some(f) = crate::gc_store::gc_store_get_ns(cx, "fsinternal", "readOptionsClassify") else {
        JS_ReportErrorUTF8(cx, c"read: options classifier layer is unavailable".as_ptr());
        return ::std::result::Result::Err(());
    };
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let f_root = f);
    rooted!(&in(cx_ref) let v_root = v);
    rooted!(&in(cx_ref) let f_val = mozjs::jsval::ObjectValue(f_root.get()));
    // unrooted-args class (B-sweep 2026-10-26): the call argument rides a
    // rooted ValueArray — a bare stack copy goes stale if GC moves it.
    rooted!(&in(cx_ref) let call_args = mozjs_sys::jsgc::ValueArray::<1>::new([v_root.get()]));
    let call_args_h = JS::HandleValueArray::from(&call_args);
    rooted!(&in(cx_ref) let undef_this = ::std::ptr::null_mut::<JSObject>());
    let mut rval = UndefinedValue();
    let ok = JS_CallFunctionValue(
        cx,
        undef_this.handle().into(),
        f_val.handle().into(),
        &call_args_h,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        },
    );
    if !ok {
        // Revoked proxy: spec IsArray throws — leave the exception pending
        // (node parity: validateObject's Array.isArray propagates the throw).
        return ::std::result::Result::Err(());
    }
    if !rval.is_string() {
        JS_ReportErrorUTF8(cx, c"read: options classifier returned a non-string".as_ptr());
        return ::std::result::Result::Err(());
    }
    let s = crate::jsstr_to_rust_string(cx, rval.to_string());
    if s == "array" {
        ::std::result::Result::Ok("array")
    } else if s == "function" {
        ::std::result::Result::Ok("function")
    } else {
        ::std::result::Result::Ok("other")
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn parse_read_window_args(
    cx: *mut JSContext,
    args: &CallArgs,
    fd_index: ::std::option::Option<u32>,
    buf_base: u32,
    face: ReadWindowFace,
) -> ::std::result::Result<(*mut JSObject, usize, usize, bool, i64), ()> {
    // fd type gate (node: ERR_INVALID_ARG_TYPE 'The "fd" argument must be of
    // type number. Received null').
    if let ::std::option::Option::Some(fi) = fd_index {
        let fd_val = if args.argc_ > fi { *args.get(fi).ptr } else { UndefinedValue() };
        if !(fd_val.is_int32() || fd_val.is_double()) {
            let msg = format!(
                "The \"fd\" argument must be of type number. Received {}",
                received_fs_desc(cx, fd_val)
            );
            throw_fs_coded_error(cx, "ERR_INVALID_ARG_TYPE", "TypeError", &msg);
            return ::std::result::Result::Err(());
        }
    }

    let buf_val = if args.argc_ > buf_base { *args.get(buf_base).ptr } else { UndefinedValue() };
    let slot2 = if args.argc_ > buf_base + 1 { *args.get(buf_base + 1).ptr } else { UndefinedValue() };
    let buf_is_view = view_byte_slot(buf_val).is_some();

    // Unrooted-args discipline (B-sweep 2026-10-26 class): property reads can
    // trigger getters → GC. The resolved offset/length/position values live in
    // roots owned by THIS frame; the tail consumes them as handles so every
    // access re-derefs the live slot.
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let mut offset_v = UndefinedValue());
    rooted!(&in(cx_ref) let mut length_v = UndefinedValue());
    rooted!(&in(cx_ref) let mut position_v = UndefinedValue());
    rooted!(&in(cx_ref) let buf_root = buf_val);

    // Overload resolution.
    let positional_length_default_zero;
    if face == ReadWindowFace::Callback
        && (args.argc_ <= buf_base
            || buf_val.is_undefined()
            || buf_val.is_null()
            || (buf_val.is_object()
                && unsafe { mozjs_sys::jsapi::js::IsFunctionObject(buf_val.to_object()) }))
    {
        // node's flexible callback face WITHOUT an explicit buffer:
        //   read(fd, cb) | read(fd) | read(fd, null|undefined, cb) — the
        // buffer defaults to Buffer.alloc(16384), offset 0, length = buffer
        // length, position = current (read-optional-params corpus: bytesRead
        // = expected, buffer.byteLength = 16384). readSync has no such form
        // (its primitive offset slot is the OPTIONS error) and the promise
        // faces require an explicit buffer.
        let default_bytes = [0u8; 16384];
        let def_obj = crate::globals::create_buffer_object(cx, &default_bytes);
        if def_obj.is_null() {
            JS_ReportErrorUTF8(cx, c"read: failed to allocate the default buffer".as_ptr());
            return ::std::result::Result::Err(());
        }
        rooted!(&in(cx_ref) let def_root = mozjs::jsval::ObjectValue(def_obj));
        return parse_read_window_with_buffer(
            cx,
            def_root.handle().into(),
            offset_v.handle().into(),
            length_v.handle().into(),
            position_v.handle().into(),
            false,
        );
    }
    if buf_is_view {
        // Root the slot copy BEFORE any JS can run (the classifier below is a
        // JS call — a moving GC updates registered roots, never this stack
        // word; unrooted-args class B-sweep 2026-10-26).
        rooted!(&in(cx_ref) let slot2_root = slot2);
        // Proxy-transparent classification (node validateObject face): typeof
        // and Array.isArray see through proxies; the SM class checks are
        // proxy-blind. JS_IsProxy (cheap class probe, no GC) gates the JS
        // classifier call to the proxy path only — zero cost for the plain
        // options form.
        let slot2_is_proxy = slot2.is_object() && unsafe { w2::JS_IsProxy(slot2.to_object()) };
        let slot2_class = if slot2_is_proxy {
            match classify_options_value(cx, slot2) {
                ::std::result::Result::Ok(c) => c,
                ::std::result::Result::Err(()) => return ::std::result::Result::Err(()),
            }
        } else {
            ""
        };
        let slot2 = slot2_root.get();
        let slot2_is_fn = if slot2_is_proxy {
            slot2_class == "function"
        } else {
            slot2.is_object()
                && unsafe { mozjs_sys::jsapi::js::IsFunctionObject(slot2.to_object()) }
        };
        let slot2_is_options = if slot2.is_undefined() || slot2.is_null() {
            true
        } else if slot2_is_fn {
            // A trailing callback in the offset slot is not options — the
            // offset defaults (read(fd, buffer, cb)).
            false
        } else if slot2.is_object() {
            // Plain objects (incl. String wrappers — oracle: readSync(fd, buf,
            // new String('4444')) works as {length: 4}) are the options form
            // at any argc. Arrays are NOT options (node validateObject) at any
            // argc — the options branch opens on typeof object past the third
            // slot too (oracle: readSync(fd, buf, [], 4, 0) → options type
            // error). Proxied arrays classify through the target per spec.
            let slot2_is_array = if slot2_is_proxy {
                slot2_class == "array"
            } else {
                is_array_object(cx, slot2)
            };
            if slot2_is_array && face == ReadWindowFace::Sync {
                let msg = format!(
                    "The \"options\" argument must be of type object. Received {}",
                    received_fs_desc(cx, slot2)
                );
                throw_fs_coded_error(cx, "ERR_INVALID_ARG_TYPE", "TypeError", &msg);
                return ::std::result::Result::Err(());
            }
            // Proxied plain objects are the options form — the destructure
            // reads below go through the proxy get traps like node.
            !slot2_is_array
        } else {
            // Primitive in the offset slot: with no length arg following, the
            // sync face reports the OPTIONS type (oracle: readSync(fd, buf, 0)
            // → 'The "options" argument must be of type object. Received type
            // number (0)'); the callback/FileHandle faces (no argc-based
            // options slot) treat it as the offset.
            if args.argc_ <= buf_base + 2 && face == ReadWindowFace::Sync {
                let msg = format!(
                    "The \"options\" argument must be of type object. Received {}",
                    received_fs_desc(cx, slot2)
                );
                throw_fs_coded_error(cx, "ERR_INVALID_ARG_TYPE", "TypeError", &msg);
                return ::std::result::Result::Err(());
            }
            false
        };
        if slot2_is_options {
            let opts_obj = if slot2.is_object() { slot2.to_object() } else { ::std::ptr::null_mut() };
            if !opts_obj.is_null() {
                rooted!(&in(cx_ref) let opts_root = opts_obj);
                read_object_prop_into(cx, opts_root.handle().into(), c"offset".as_ptr(), offset_v.handle_mut().into());
                read_object_prop_into(cx, opts_root.handle().into(), c"length".as_ptr(), length_v.handle_mut().into());
                read_object_prop_into(cx, opts_root.handle().into(), c"position".as_ptr(), position_v.handle_mut().into());
            }
            positional_length_default_zero = false;
        } else {
            // readSync has no callback overload: a fn in the options slot
            // (readSync(fd, buf, fn)) is invalid OPTIONS, not an offset —
            // the former fall-through hit the offset gate and reported
            // ERR_OUT_OF_RANGE where the corpus expects ERR_INVALID_ARG_TYPE
            // (readSync-optional-params).
            if face == ReadWindowFace::Sync && slot2_is_fn && args.argc_ == buf_base + 2 {
                let msg = format!(
                    "The \"options\" argument must be of type object. Received {}",
                    received_fs_desc(cx, slot2)
                );
                throw_fs_coded_error(cx, "ERR_INVALID_ARG_TYPE", "TypeError", &msg);
                return ::std::result::Result::Err(());
            }
            // A trailing callback in the offset slot shifts the window:
            // read(fd, buffer, cb) — ONLY on the callback face with nothing
            // after the fn (3-arg overload). The promises/sync faces have no
            // callback overload, so the fn falls through to the tail's offset
            // validation (oracle: fh.read(b, fn) rejects ERR_INVALID_ARG_TYPE).
            let slot2_is_cb = slot2_is_fn
                && face == ReadWindowFace::Callback
                && args.argc_ == buf_base + 2;
            offset_v.set(if slot2_is_cb { UndefinedValue() } else { slot2 });
            let mut lv = if args.argc_ > buf_base + 2 { *args.get(buf_base + 2).ptr } else { UndefinedValue() };
            let mut pv = if args.argc_ > buf_base + 3 { *args.get(buf_base + 3).ptr } else { UndefinedValue() };
            let lv_is_fn = lv.is_object()
                && unsafe { mozjs_sys::jsapi::js::IsFunctionObject(lv.to_object()) };
            if lv_is_fn {
                if face == ReadWindowFace::Callback && args.argc_ == buf_base + 3 {
                    // read(fd, buffer, 0, cb): node's 4-arg overload is
                    // (fd, buffer, OPTIONS, callback) — the NUMBER in the
                    // offset slot reports the OPTIONS type (oracle: 'The
                    // "options" argument must be of type object. Received
                    // type number (0)'). A non-object slot2 already took the
                    // options branch above, so this arm is the number case.
                    let msg = format!(
                        "The \"options\" argument must be of type object. Received {}",
                        received_fs_desc(cx, slot2)
                    );
                    throw_fs_coded_error(cx, "ERR_INVALID_ARG_TYPE", "TypeError", &msg);
                    return ::std::result::Result::Err(());
                }
                // Every other face/shape: the fn is a VALUE in the window —
                // fall through with lv/pv untouched; the tail's number
                // validation throws the coded type error. (The former parser
                // silently defaulted here, hiding invalid args.)
            } else {
                let pv_is_fn = pv.is_object()
                    && unsafe { mozjs_sys::jsapi::js::IsFunctionObject(pv.to_object()) };
                if pv_is_fn
                    && face == ReadWindowFace::Callback
                    && args.argc_ == buf_base + 4
                {
                    // 5-arg read(fd, buffer, offset, length, cb): the fn at
                    // the position slot IS the callback — position defaults
                    // (node oracle). With an argument AFTER the fn (6-arg
                    // read(fd, b, 0, 4, fn, cb)) the fn is the position VALUE
                    // and is validated instead (oracle: sync-throw
                    // ERR_INVALID_ARG_TYPE); promises faces validate too.
                    pv = UndefinedValue();
                }
            }
            length_v.set(lv);
            position_v.set(pv);
            // oracle: readSync(fd, buf, 0, undefined) reads 0 bytes — the sync
            // positional length defaults to 0, not the buffer size; the
            // callback/FileHandle faces default to the remaining window.
            positional_length_default_zero = face == ReadWindowFace::Sync;
        }
    } else if face != ReadWindowFace::Sync && buf_val.is_object() {
        // (fd, {buffer, offset, length, position}[, cb]) — the dict form is
        // valid on the callback AND promises faces (fh.read({buffer, …}),
        // fsp.read(fd, {buffer, …})); readSync takes its options at the
        // offset slot instead.
        let opts_obj = buf_val.to_object();
        rooted!(&in(cx_ref) let opts_root = opts_obj);
        read_object_prop_into(cx, opts_root.handle().into(), c"offset".as_ptr(), offset_v.handle_mut().into());
        read_object_prop_into(cx, opts_root.handle().into(), c"length".as_ptr(), length_v.handle_mut().into());
        read_object_prop_into(cx, opts_root.handle().into(), c"position".as_ptr(), position_v.handle_mut().into());
        rooted!(&in(cx_ref) let mut inner_buf = UndefinedValue());
        read_object_prop_into(cx, opts_root.handle().into(), c"buffer".as_ptr(), inner_buf.handle_mut().into());
        if inner_buf.is_undefined() {
            // options.buffer ABSENT/undefined → node's default 16384 zero
            // buffer (read(fd, {}, cb) / read(fd, {offset…}, cb) — the
            // read-optional-params corpus asserts buffer.byteLength == 16384).
            // An EXPLICIT invalid buffer (e.g. {buffer: null}) still throws
            // below (test-fs-read options.buffer cell).
            let default_bytes = [0u8; 16384];
            let def_obj = crate::globals::create_buffer_object(cx, &default_bytes);
            if def_obj.is_null() {
                JS_ReportErrorUTF8(cx, c"read: failed to allocate the default buffer".as_ptr());
                return ::std::result::Result::Err(());
            }
            rooted!(&in(cx_ref) let def_root = mozjs::jsval::ObjectValue(def_obj));
            return parse_read_window_with_buffer(
                cx,
                def_root.handle().into(),
                offset_v.handle().into(),
                length_v.handle().into(),
                position_v.handle().into(),
                false,
            );
        }
        if !inner_buf.is_object() || view_byte_slot(inner_buf.get()).is_none() {
            let msg = format!(
                "The \"buffer\" argument must be an instance of Buffer, TypedArray, or DataView. Received {}",
                received_fs_desc(cx, inner_buf.get())
            );
            throw_fs_coded_error(cx, "ERR_INVALID_ARG_TYPE", "TypeError", &msg);
            return ::std::result::Result::Err(());
        }
        // The options-derived length defaults to (byteLength - offset), never
        // 0; the empty-buffer gate fires inside the shared tail.
        return parse_read_window_with_buffer(cx, inner_buf.handle().into(), offset_v.handle().into(), length_v.handle().into(), position_v.handle().into(), false);
    } else {
        let msg = format!(
            "The \"buffer\" argument must be an instance of Buffer, TypedArray, or DataView. Received {}",
            received_fs_desc(cx, buf_val)
        );
        throw_fs_coded_error(cx, "ERR_INVALID_ARG_TYPE", "TypeError", &msg);
        return ::std::result::Result::Err(());
    }

    parse_read_window_with_buffer(cx, buf_root.handle().into(), offset_v.handle().into(), length_v.handle().into(), position_v.handle().into(), positional_length_default_zero)
}

/// Read `obj[name]` into a rooted out-slot (the getter can run arbitrary JS →
/// the read must land in a GC-tracked slot, never a bare stack JSVal).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn read_object_prop_into(
    cx: *mut JSContext,
    obj: Handle<*mut JSObject>,
    name: *const ::std::ffi::c_char,
    out: MutableHandle<Value>,
) {
    JS_GetProperty(cx, obj, name, out);
}

/// Shared tail of the read-window parse: validates offset/length/position and
/// the empty-buffer gate against an already-resolved buffer view. All value
/// params are handles into caller-frame roots (GC-safe across the validation
/// allocations).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn parse_read_window_with_buffer(
    cx: *mut JSContext,
    buf_val: Handle<Value>,
    offset_val: Handle<Value>,
    length_val: Handle<Value>,
    position_val: Handle<Value>,
    positional_length_default_zero: bool,
) -> ::std::result::Result<(*mut JSObject, usize, usize, bool, i64), ()> {
    let byte_len = view_byte_slot(buf_val.get()).map(|(_, l)| l).unwrap_or(0);
    let buf_obj = buf_val.get().to_object();

    // ── offset ──────────────────────────────────────────────────────────────
    let offset = read_positional_number(cx, offset_val, "offset", 0.0)?;
    if !offset.is_finite() || offset.fract() != 0.0 {
        let msg = format!(
            "The value of \"offset\" is out of range. It must be an integer. Received {}",
            format_number_plain(offset)
        );
        throw_fs_coded_error(cx, "ERR_OUT_OF_RANGE", "RangeError", &msg);
        return ::std::result::Result::Err(());
    }
    if offset < 0.0 || offset > 9007199254740991.0 {
        let msg = format!(
            "The value of \"offset\" is out of range. It must be >= 0 && <= 9007199254740991. Received {}",
            format_number_plain(offset)
        );
        throw_fs_coded_error(cx, "ERR_OUT_OF_RANGE", "RangeError", &msg);
        return ::std::result::Result::Err(());
    }
    let offset = offset as usize;

    // Effective length value (converted only — range validation below, after
    // the position gate which needs the length for the bigint bound).
    let length_default = if positional_length_default_zero {
        0.0
    } else {
        (byte_len as f64 - offset as f64).max(0.0)
    };
    let length_raw = read_positional_number(cx, length_val, "length", length_default)?;

    // ── position (validated before the empty/length gates — oracle: the
    // empty-buffer + invalid-position cell expects the POSITION error; the
    // bigint bound is 2^63-1 minus the read length) ─────────────────────────
    let (use_pread, position) = validate_read_position(cx, position_val, length_raw)?;

    // ── length ──────────────────────────────────────────────────────────────
    let length = length_raw;

    // ── length ──────────────────────────────────────────────────────────────
    if !length.is_finite() || length.fract() != 0.0 {
        let msg = format!(
            "The value of \"length\" is out of range. It must be an integer. Received {}",
            format_number_plain(length)
        );
        throw_fs_coded_error(cx, "ERR_OUT_OF_RANGE", "RangeError", &msg);
        return ::std::result::Result::Err(());
    }
    if length < 0.0 {
        let msg = format!(
            "The value of \"length\" is out of range. It must be >= 0. Received {}",
            format_number_plain(length)
        );
        throw_fs_coded_error(cx, "ERR_OUT_OF_RANGE", "RangeError", &msg);
        return ::std::result::Result::Err(());
    }
    if byte_len == 0 && length > 0.0 {
        throw_empty_buffer_error(cx, buf_val.get());
        return ::std::result::Result::Err(());
    }
    if length as usize > byte_len.saturating_sub(offset) {
        let msg = format!(
            "The value of \"length\" is out of range. It must be <= {}. Received {}",
            byte_len.saturating_sub(offset),
            format_number_plain(length)
        );
        throw_fs_coded_error(cx, "ERR_OUT_OF_RANGE", "RangeError", &msg);
        return ::std::result::Result::Err(());
    }
    let length = length as usize;

    ::std::result::Result::Ok((buf_obj, offset, length, use_pread, position))
}

/// ERR_INVALID_ARG_VALUE for a zero-length read target, with node's inspect
/// shape for the received view ('<Buffer >' for Buffer, 'Uint8Array(0) []'
/// for typed arrays — ctor-name driven). The buffer object is rooted for the
/// ctor-name reads (JS_GetProperty can run a getter → GC).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn throw_empty_buffer_error(cx: *mut JSContext, buf_val: JSVal) -> bool {
    let mut ctor_desc = String::from("Object");
    if buf_val.is_object() {
        let mut wrapped = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped;
        rooted!(&in(cx_ref) let buf_root = buf_val.to_object());
        rooted!(&in(cx_ref) let mut ctor_v = UndefinedValue());
        read_object_prop_into(cx, buf_root.handle().into(), c"constructor".as_ptr(), ctor_v.handle_mut().into());
        if ctor_v.is_object() {
            rooted!(&in(cx_ref) let ctor_root = ctor_v.to_object());
            rooted!(&in(cx_ref) let mut name_v = UndefinedValue());
            read_object_prop_into(cx, ctor_root.handle().into(), c"name".as_ptr(), name_v.handle_mut().into());
            if name_v.is_string() {
                let s = crate::jsstr_to_rust_string(cx, name_v.to_string());
                if s == "Buffer" {
                    ctor_desc = "<Buffer >".to_string();
                } else {
                    let len = view_byte_slot(buf_val).map(|(_, l)| l).unwrap_or(0);
                    ctor_desc = format!("{}({}) []", s, len);
                }
            }
        }
    }
    let msg = format!(
        "The argument 'buffer' is empty and cannot be written. Received {}",
        ctor_desc
    );
    throw_fs_coded_error(cx, "ERR_INVALID_ARG_VALUE", "TypeError", &msg)
}

/// Read a positional read-family argument as an f64: numbers pass through,
/// `undefined`/`null` → the default; everything else → ERR_OUT_OF_RANGE "must
/// be an integer" (node validateInteger shape). The value arrives as a handle
/// into a caller-frame root (null derefs to a copy at use points only).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn read_positional_number(
    cx: *mut JSContext,
    v: Handle<Value>,
    name: &str,
    default: f64,
) -> ::std::result::Result<f64, ()> {
    let v = v.get();
    if v.is_undefined() || v.is_null() {
        return ::std::result::Result::Ok(default);
    }
    if v.is_int32() {
        return ::std::result::Result::Ok(v.to_int32() as f64);
    }
    if v.is_double() {
        return ::std::result::Result::Ok(v.to_double());
    }
    let msg = format!(
        "The value of \"{}\" is out of range. It must be an integer. Received {}",
        name,
        received_fs_desc(cx, v)
    );
    throw_fs_coded_error(cx, "ERR_OUT_OF_RANGE", "RangeError", &msg);
    ::std::result::Result::Err(())
}

/// node read position gate: bigint | integer only (else ERR_INVALID_ARG_TYPE),
/// integer + in [-1, bound] else ERR_OUT_OF_RANGE (number bound MAX_SAFE,
/// bigint bound 2^63-1 minus the read length — oracle: position
/// 2n**63n-4n with a 4-byte window is ERR_OUT_OF_RANGE while 2n**63n-1n-4n
/// reads fine). `-1` / null / undefined → current cursor (pread off).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn validate_read_position(
    cx: *mut JSContext,
    v_handle: Handle<Value>,
    length: f64,
) -> ::std::result::Result<(bool, i64), ()> {
    let v = v_handle.get();
    if v.is_null() || v.is_undefined() {
        return ::std::result::Result::Ok((false, -1));
    }
    if v.is_int32() {
        let n = v.to_int32() as i64;
        if n < -1 {
            let msg = format!(
                "The value of \"position\" is out of range. It must be >= -1 && <= 9007199254740991. Received {}",
                n
            );
            throw_fs_coded_error(cx, "ERR_OUT_OF_RANGE", "RangeError", &msg);
            return ::std::result::Result::Err(());
        }
        return ::std::result::Result::Ok((n >= 0, n));
    }
    if v.is_double() {
        let d = v.to_double();
        if !d.is_finite() || d.fract() != 0.0 {
            let msg = format!(
                "The value of \"position\" is out of range. It must be an integer. Received {}",
                format_number_plain(d)
            );
            throw_fs_coded_error(cx, "ERR_OUT_OF_RANGE", "RangeError", &msg);
            return ::std::result::Result::Err(());
        }
        if d < -1.0 || d > 9007199254740991.0 {
            let msg = format!(
                "The value of \"position\" is out of range. It must be >= -1 && <= 9007199254740991. Received {}",
                format_number_plain(d)
            );
            throw_fs_coded_error(cx, "ERR_OUT_OF_RANGE", "RangeError", &msg);
            return ::std::result::Result::Err(());
        }
        let n = d as i64;
        return ::std::result::Result::Ok((n >= 0, n));
    }
    if v.is_bigint() {
        // BigInt position: bound = i64::MAX - length (node's long-window
        // check). Conversion via the in-tree ToString+parse path (bun_ffi
        // val_to_i64 precedent; the mozjs-sys surface has no BigIntIsInt64
        // binding).
        let decimal = bigint_decimal_string(cx, v);
        let bi = decimal.parse::<i64>().ok();
        let bigint_bound = 9223372036854775807i64.saturating_sub(length as i64);
        match bi {
            ::std::option::Option::Some(n) if n >= -1 && n <= bigint_bound => {
                return ::std::result::Result::Ok((n >= 0, n));
            }
            _ => {
                let msg = format!(
                    "The value of \"position\" is out of range. It must be >= -1 && <= {}. Received {}",
                    bigint_bound,
                    bigint_received_desc(&decimal)
                );
                throw_fs_coded_error(cx, "ERR_OUT_OF_RANGE", "RangeError", &msg);
                return ::std::result::Result::Err(());
            }
        }
    }
    let msg = format!(
        "The \"position\" argument must be of type bigint or integer. Received {}",
        received_fs_desc(cx, v)
    );
    throw_fs_coded_error(cx, "ERR_INVALID_ARG_TYPE", "TypeError", &msg);
    ::std::result::Result::Err(())
}

/// node number formatting for error messages: plain decimal for integer-range
/// doubles, exponent form for huge magnitudes (1e+308 → "1e+308").
fn format_number_plain(n: f64) -> String {
    if n.is_nan() {
        "NaN".to_string()
    } else if n == f64::INFINITY {
        "Infinity".to_string()
    } else if n == f64::NEG_INFINITY {
        "-Infinity".to_string()
    } else if n.abs() >= 1e21 {
        format!("{:e}", n)
    } else if n.fract() == 0.0 && n.abs() < 1e16 {
        format!("{}", n as i64)
    } else {
        let s = format!("{}", n);
        s
    }
}

/// BigInt → canonical decimal string via SM ToString (no 'n' suffix;
/// mozjs rust wrapper — the bun_ffi val_to_i64 conversion precedent).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn bigint_decimal_string(cx: *mut JSContext, v: JSVal) -> String {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let v_root = v);
    let jsstr = unsafe { mozjs::rust::ToString(cx_ref, v_root.handle()) };
    if jsstr.is_null() {
        JS_ClearPendingException(cx);
        return "0".to_string();
    }
    crate::jsstr_to_rust_string(cx, jsstr)
}

/// node bigint Received-descriptor for read-family error messages: decimal
/// digits with `_` separators every three from the right (node's
/// formatWithUnderscores), suffixed with `n` (2^63 → "9_223_372_036_854_775_808n").
fn bigint_received_desc(decimal: &str) -> String {
    let digits: Vec<char> = decimal.chars().collect();
    let (sign, rest) = if digits.first() == Some(&'-') {
        ("-", &digits[1..])
    } else {
        ("", &digits[..])
    };
    let mut grouped = String::new();
    let n = rest.len();
    for (i, c) in rest.iter().enumerate() {
        grouped.push(*c);
        let rem = n - i - 1;
        if rem > 0 && rem % 3 == 0 {
            grouped.push('_');
        }
    }
    format!("{}{}n", sign, grouped)
}

/// fd value from argv (int32 fast path; a double fd is coerced, mirroring the
/// pre-validation read-family behavior).
fn fd_from_args(args: &CallArgs) -> i32 {
    let fd_val = if args.argc_ > 0 {
        unsafe { *args.get(0).ptr }
    } else {
        UndefinedValue()
    };
    if fd_val.is_int32() {
        fd_val.to_int32()
    } else if fd_val.is_double() {
        fd_val.to_double() as i32
    } else {
        -1
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_read_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let (buf_obj, offset, length, use_pread, position) =
        match parse_read_window_args(cx, &args, ::std::option::Option::Some(0), 1, ReadWindowFace::Sync) {
            ::std::result::Result::Ok(w) => w,
            ::std::result::Result::Err(()) => return false,
        };

    if length == 0 {
        args.rval().set(mozjs::jsval::Int32Value(0));
        return true;
    }

    #[cfg(unix)]
    {
        // Rooted view — data pointer stays valid across the read (no JS runs).
        let mut is_shared = false;
        let data_ptr = mozjs_sys::jsapi::JS_GetArrayBufferViewData(
            buf_obj,
            &mut is_shared,
            ::std::ptr::null(),
        ) as *mut u8;
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
                    fd_from_args(&args),
                    dst as *mut ::std::ffi::c_void,
                    length,
                    position,
                )
            }
        } else {
            unsafe { libc::read(fd_from_args(&args), dst as *mut ::std::ffi::c_void, length) }
        };
        if bytes_read >= 0 {
            args.rval().set(mozjs::jsval::Int32Value(bytes_read as i32));
            true
        } else {
            throw_fs_error(
                cx,
                "readSync",
                &format!("fd:{}", fd_from_args(&args)),
                &::std::io::Error::last_os_error(),
            )
        }
    }
    #[cfg(windows)]
    {
        // SAFETY: rooted view — data pointer stays valid across the read
        // (no JS runs); _read operates on the CRT fd.
        let fd = fd_from_args(&args);
        let mut is_shared = false;
        let data_ptr = mozjs_sys::jsapi::JS_GetArrayBufferViewData(
            buf_obj,
            &mut is_shared,
            ::std::ptr::null(),
        ) as *mut u8;
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

// (fs_read_sync was rewritten to route through parse_read_window_args;
// see the resolver above for the full node validation contract.)

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

// ── missing node sync/promise APIs (SHAPE_missing_api family root) ──────────
// @trace REQ-ENG-004 — chownSync / utimesSync / lutimesSync / _toUnixTimestamp
// / realpathSync.native / mkdtempDisposable(Sync) / promises.watch / fs.Dir.
// Shapes are node v24.19.0 oracle-verified (2026-10-25):
//   _toUnixTimestamp(123.456) → 123.456; _toUnixTimestamp(new Date(0)) → 0;
//   non-number non-Date → ERR_INVALID_ARG_TYPE
//   `The "time" argument must be an instance of Date or an Time in seconds.`
//   (node's own wording, including the "an Time" quirk — verbatim).
//   mkdtempDisposableSync(p) → { path, remove } — remove is idempotent.

/// node toUnixTimestamp semantics: number passthrough, Date →
/// floor(ms)/1000 (whole-ms floor — the second fraction is preserved; a
/// seconds-floor here zeroed the ns half of every pre-epoch Date time,
/// test-fs-stat-date runTest(-40691) → -41000), anything else
/// ERR_INVALID_ARG_TYPE (oracle-verbatim message).
unsafe fn parse_fs_time_arg(
    cx: *mut JSContext,
    val: JSVal,
    arg_name: &str,
) -> ::std::result::Result<f64, bool> {
    if val.is_int32() {
        return ::std::result::Result::Ok(val.to_int32() as f64);
    }
    if val.is_double() {
        return ::std::result::Result::Ok(val.to_double());
    }
    if val.is_object() {
        rooted!(in(cx) let obj = val.to_object());
        // Date-shaped: a callable getTime() — avoids the SM build's Date API
        // surface while accepting every real Date (node: isDate(time)).
        let mut get_time = UndefinedValue();
        JS_GetProperty(
            cx,
            obj.handle().into(),
            c"getTime".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut get_time,
            },
        );
        if get_time.is_object()
            && unsafe { mozjs_sys::jsapi::js::IsFunctionObject(get_time.to_object()) }
        {
            let c_gt = ZBox::from_bytes("getTime".as_bytes());
            let mut ms = UndefinedValue();
            let ok = JS_CallFunctionName(
                cx,
                obj.handle().into(),
                c_gt.as_ptr(),
                &HandleValueArray::empty(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut ms,
                },
            );
            if ok && ms.is_number() {
                // node: Math.floor(getTime()) / 1000 — floor to WHOLE
                // milliseconds, the second-fraction survives into the seconds
                // value. The former `(ms / 1000).floor()` floored to whole
                // SECONDS, destroying the ms part before the ns-precision
                // timespec split could see it (runTest(-40691): Date(-40691)
                // ms = -40.691 s → sec -41 + ns 309000000; the seconds-floor
                // produced -41 s + 0 ns → stat read back atimeMs -41000).
                // node v24.19.0 oracle: _toUnixTimestamp(new Date(1234.567))
                // → 1.234 (ms floor, not seconds floor); utimesSync(p, new
                // Date(-40691)) → statSync atimeMs -40691.
                return ::std::result::Result::Ok((ms.to_double().floor() / 1000.0));
            }
            // A getTime() call that threw (corrupt Date) falls through to the
            // type error below — never a silent 0.
            let _ = JS_ClearPendingException(cx);
        }
    }
    let msg = format!(
        "The \"{}\" argument must be an instance of Date or an Time in seconds. Received {}",
        arg_name,
        received_fs_desc(cx, val)
    );
    ::std::result::Result::Err(throw_fs_coded_error(
        cx,
        "ERR_INVALID_ARG_TYPE",
        "TypeError",
        &msg,
    ))
}

/// fs._toUnixTimestamp — internal helper surfaced on the fs object (node
/// lib/fs.js exports it; test-fs-utimes asserts its shape).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_to_unix_timestamp(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        let msg = "The \"time\" argument must be an instance of Date or an Time in seconds. Received undefined";
        return throw_fs_coded_error(cx, "ERR_INVALID_ARG_TYPE", "TypeError", msg);
    }
    match parse_fs_time_arg(cx, *args.get(0).ptr, "time") {
        ::std::result::Result::Ok(t) => {
            args.rval().set(mozjs::jsval::DoubleValue(t));
            true
        }
        ::std::result::Result::Err(b) => b,
    }
}

/// f64 epoch seconds (node time args, stat-derived cp timestamps) →
/// ns-precision [`libc::timespec`] for the utimensat/futimens family.
/// `t.floor()` splits the whole-second part so pre-epoch fractions stay in
/// [0, 1s) (plain truncation flips the nsec sign at negative times —
/// test-fs-stat-date runTest(-40691, …)), and the sub-second remainder rounds
/// to the NEAREST nanosecond: an f64 epoch value carries ~120 ns of real
/// granularity at the current epoch, so set+get round-trips stat-derived
/// times losslessly at the f64 layer. The former timeval path truncated at
/// µs AND used `as` (toward-zero), losing up to 1 µs — the
/// cp-preserve-timestamps `…362.3767 !== …362.376` signature.
///
/// @trace REQ-ENG-007 [api:fs.utimes|fs.futimes|fs.lutimes|FileHandle.utimes]
/// — the ns time-set family root: utimensat/futimens replace the µs
/// utimes(3)/futimes(3)/lutimes(3) faces across every setter in this file
/// (sync, callback, promise, fd legs all route through these two helpers).
#[cfg(unix)]
fn fs_time_timespec(t: f64) -> libc::timespec {
    let sec = t.floor();
    let mut frac_ns = ((t - sec) * 1_000_000_000.0).round();
    let mut whole = 0.0;
    if frac_ns >= 1_000_000_000.0 {
        whole = 1.0;
        frac_ns -= 1_000_000_000.0;
    }
    libc::timespec {
        tv_sec: (sec + whole) as libc::time_t,
        tv_nsec: frac_ns as libc::c_long,
    }
}

/// Exact (sec, nsec) → timespec for the cp preserveTimestamps chain: the pair
/// comes straight off the source stat (CpStat carries the raw st_mtim pair),
/// so the utimensat round trip never re-quantizes the nsec through an f64
/// seconds value (~238ns ulp at 1e9s magnitudes).
#[cfg(unix)]
fn fs_time_timespec_pair(sec: i64, nsec: i64) -> libc::timespec {
    let mut tv_sec = sec;
    let mut tv_nsec = nsec;
    if tv_nsec < 0 {
        tv_sec -= 1;
        tv_nsec += 1_000_000_000;
    }
    libc::timespec {
        tv_sec: tv_sec as libc::time_t,
        tv_nsec: tv_nsec as libc::c_long,
    }
}

/// fd-leg of the ns-precision time-set family (futimes/futimesSync/
/// FileHandle.utimes): futimens with the shared seconds→timespec conversion
/// (the former faces went through futimes() µs timevals — with the sub-second
/// fraction truncated or dropped, and Date args silently coerced to 0.0).
#[cfg(unix)]
fn futimens_fd(fd: i32, atime: f64, mtime: f64) -> ::std::io::Result<()> {
    let times = [fs_time_timespec(atime), fs_time_timespec(mtime)];
    let rv = unsafe { libc::futimens(fd, times.as_ptr()) };
    if rv == 0 {
        ::std::result::Result::Ok(())
    } else {
        ::std::result::Result::Err(::std::io::Error::last_os_error())
    }
}

/// Shared utimes core: path + two time args → utimensat with ns precision
/// (time_t is i64 on every bao target → y2K38-safe, test-fs-utimes-y2K38).
/// utimensat replaces the former utimes(3)/lutimes(3) µs face; the
/// follow_symlinks leg maps to AT_SYMLINK_NOFOLLOW (lutimes semantics).
#[cfg(unix)]
fn utimes_path(path: &str, atime: f64, mtime: f64, follow_symlinks: bool) -> ::std::io::Result<()> {
    let c_path = ::std::ffi::CString::new(path)
        .map_err(|_| ::std::io::Error::new(::std::io::ErrorKind::InvalidInput, "path contains NUL"))?;
    let times = [fs_time_timespec(atime), fs_time_timespec(mtime)];
    let flags = if follow_symlinks {
        0
    } else {
        libc::AT_SYMLINK_NOFOLLOW
    };
    let rv = unsafe {
        libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), times.as_ptr(), flags)
    };
    if rv == 0 {
        ::std::result::Result::Ok(())
    } else {
        ::std::result::Result::Err(::std::io::Error::last_os_error())
    }
}

/// cp preserveTimestamps leg: same utimensat core as utimes_path but the
/// timestamps arrive as the exact stat (sec, nsec) pairs — the f64 path
/// re-quantized the nsec (test-fs-cp-sync-preserve-timestamps-readonly).
#[cfg(unix)]
fn utimes_path_ns(
    path: &str,
    atime: (i64, i64),
    mtime: (i64, i64),
    follow_symlinks: bool,
) -> ::std::io::Result<()> {
    let c_path = ::std::ffi::CString::new(path)
        .map_err(|_| ::std::io::Error::new(::std::io::ErrorKind::InvalidInput, "path contains NUL"))?;
    let times = [
        fs_time_timespec_pair(atime.0, atime.1),
        fs_time_timespec_pair(mtime.0, mtime.1),
    ];
    let flags = if follow_symlinks {
        0
    } else {
        libc::AT_SYMLINK_NOFOLLOW
    };
    let rv = unsafe {
        libc::utimensat(libc::AT_FDCWD, c_path.as_ptr(), times.as_ptr(), flags)
    };
    if rv == 0 {
        ::std::result::Result::Ok(())
    } else {
        ::std::result::Result::Err(::std::io::Error::last_os_error())
    }
}

#[cfg(not(unix))]
fn utimes_path_ns(
    _path: &str,
    _atime: (i64, i64),
    _mtime: (i64, i64),
    _follow: bool,
) -> ::std::io::Result<()> {
    ::std::result::Result::Err(::std::io::Error::other("utimes is unix-only"))
}

#[cfg(not(unix))]
fn utimes_path(_path: &str, _atime: f64, _mtime: f64, _follow: bool) -> ::std::io::Result<()> {
    ::std::result::Result::Err(::std::io::Error::other("utimes is unix-only"))
}

/// utimesSync(path, atime, mtime) / lutimesSync(path, atime, mtime) shared body.
unsafe fn fs_utimes_sync_impl(
    cx: *mut JSContext,
    args: &CallArgs,
    argc: u32,
    follow_symlinks: bool,
) -> bool {
    let path = match get_path_arg(cx, args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let atime_val = if argc > 1 { *args.get(1).ptr } else { UndefinedValue() };
    let mtime_val = if argc > 2 { *args.get(2).ptr } else { UndefinedValue() };
    let atime = match parse_fs_time_arg(cx, atime_val, "atime") {
        ::std::result::Result::Ok(t) => t,
        ::std::result::Result::Err(b) => return b,
    };
    let mtime = match parse_fs_time_arg(cx, mtime_val, "mtime") {
        ::std::result::Result::Ok(t) => t,
        ::std::result::Result::Err(b) => return b,
    };
    match utimes_path(&path, atime, mtime, follow_symlinks) {
        ::std::result::Result::Ok(()) => {
            args.rval().set(UndefinedValue());
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(
            cx,
            if follow_symlinks { "utimesSync" } else { "lutimesSync" },
            &path,
            &e,
        ),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_utimes_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    fs_utimes_sync_impl(cx, &args, argc, true)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_lutimes_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    fs_utimes_sync_impl(cx, &args, argc, false)
}

/// chownSync(path, uid, gid) — uid/gid -1 is the chown(2) "don't change"
/// sentinel; the i32→uid_t cast maps -1 to 0xFFFFFFFF which is exactly that
/// sentinel (test-fs-chown-negative-one).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_chown_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let path = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let uid = if argc > 1 {
        let v = *args.get(1).ptr;
        if v.is_int32() {
            v.to_int32()
        } else if v.is_double() {
            v.to_double() as i32
        } else {
            -1
        }
    } else {
        -1
    };
    let gid = if argc > 2 {
        let v = *args.get(2).ptr;
        if v.is_int32() {
            v.to_int32()
        } else if v.is_double() {
            v.to_double() as i32
        } else {
            -1
        }
    } else {
        -1
    };
    #[cfg(unix)]
    {
        let c_path = match ::std::ffi::CString::new(path.as_str()) {
            ::std::result::Result::Ok(p) => p,
            ::std::result::Result::Err(_) => {
                JS_ReportErrorUTF8(cx, c"EINVAL: path contains NUL".as_ptr());
                return false;
            }
        };
        let rv = unsafe { libc::chown(c_path.as_ptr(), uid as libc::uid_t, gid as libc::gid_t) };
        if rv == 0 {
            args.rval().set(UndefinedValue());
            true
        } else {
            throw_fs_error(cx, "chownSync", &path, &::std::io::Error::last_os_error())
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (uid, gid);
        args.rval().set(UndefinedValue());
        true
    }
}

/// realpathSync.native / realpath.native — Linux realpath(3) semantics; the
/// core is the same resolver as the plain face (node's native binding differs
/// only in edge buffer limits on other platforms).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_realpath_native_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    fs_realpath_sync(cx, argc, vp)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_realpath_native(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    fs_realpath(cx, argc, vp)
}

/// `remove()` of an mkdtempDisposable — recursive+force rm, idempotent
/// (node oracle: calling remove() twice does not throw).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn disposable_remove_native(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return false;
    }
    let obj = this.to_object();
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_r = obj);
    let mut path_val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_r.handle().into(),
        c"_disposablePath".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut path_val,
        },
    );
    if path_val.is_string() {
        let s = path_val.to_string();
        if !s.is_null() {
            let path = crate::jsstr_to_rust_string(cx, s);
            let _ = rm_with_options(&path, true, true);
            args.rval().set(UndefinedValue());
            return true;
        }
    }
    args.rval().set(UndefinedValue());
    true
}

/// Build `{ path, remove }` for the mkdtempDisposable family.
unsafe fn make_disposable_object(cx: *mut JSContext, path: &str) -> *mut JSObject {
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = JS_NewPlainObject(cx));
    if obj.get().is_null() {
        return ::std::ptr::null_mut();
    }
    let c_path = ZBox::from_bytes(path.as_bytes());
    let path_str = JS_NewStringCopyZ(cx, c_path.as_ptr());
    if !path_str.is_null() {
        rooted!(&in(cx_ref) let path_val = mozjs::jsval::StringValue(&*path_str));
        JS_DefineProperty(
            cx,
            obj.handle().into(),
            c"path".as_ptr(),
            path_val.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }
    // Hidden path consumed by disposable_remove_native.
    if !path_str.is_null() {
        rooted!(&in(cx_ref) let hidden_val = mozjs::jsval::StringValue(&*path_str));
        JS_DefineProperty(
            cx,
            obj.handle().into(),
            c"_disposablePath".as_ptr(),
            hidden_val.handle().into(),
            0,
        );
    }
    JS_DefineFunction(
        cx,
        obj.handle().into(),
        c"remove".as_ptr(),
        Some(disposable_remove_native),
        0,
        JSPROP_ENUMERATE as u32,
    );
    obj.get()
}

/// fs.mkdtempDisposableSync(prefix[, options]) — node 24: mkdtemp + a
/// remove() that recursively deletes the dir (oracle: keys are exactly
/// ['path','remove']).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_mkdtemp_disposable_sync(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let prefix = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    match mkdtemp_inner(&prefix) {
        ::std::result::Result::Ok(dir) => {
            let obj = make_disposable_object(cx, &dir);
            if obj.is_null() {
                args.rval().set(UndefinedValue());
                return true;
            }
            args.rval().set(mozjs::jsval::ObjectValue(obj));
            true
        }
        ::std::result::Result::Err(e) => throw_fs_error(cx, "mkdtempDisposableSync", &prefix, &e),
    }
}

/// fs.promises.mkdtempDisposable(prefix[, options]) → Promise<{path, remove}>
/// (remove returns a Promise; node oracle: same keys as the sync form).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_mkdtemp_disposable(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let prefix = match get_path_arg(cx, &args, 0) {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()));
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    match mkdtemp_inner(&prefix) {
        ::std::result::Result::Ok(dir) => {
            let obj = make_disposable_object(cx, &dir);
            if obj.is_null() {
                reject_with_error(cx, promise.get(), "mkdtempDisposable: object creation failed");
            } else {
                // The promises-face remove returns a Promise (node shape).
                rooted!(&in(cx_ref) let obj_r = obj);
                JS_DefineFunction(
                    cx,
                    obj_r.handle().into(),
                    c"remove".as_ptr(),
                    Some(disposable_remove_promise_native),
                    0,
                    JSPROP_ENUMERATE as u32,
                );
                rooted!(&in(cx_ref) let val = mozjs::jsval::ObjectValue(obj));
                unsafe {
                    mozjs_sys::jsapi::JS::ResolvePromise(
                        cx,
                        promise.handle().into(),
                        val.handle().into(),
                    );
                }
            }
        }
        ::std::result::Result::Err(e) => {
            reject_with_error(
                cx,
                promise.get(),
                &format!("mkdtempDisposable '{}': {}", prefix, e),
            );
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

/// promises-face remove(): rm + resolve (idempotent like the sync form).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn disposable_remove_promise_native(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return false;
    }
    let obj = this.to_object();
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()));
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    rooted!(&in(cx_ref) let disp_obj_root = obj);
    let mut path_val = UndefinedValue();
    JS_GetProperty(
        cx,
        disp_obj_root.handle().into(),
        c"_disposablePath".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut path_val,
        },
    );
    if path_val.is_string() {
        let s = path_val.to_string();
        if !s.is_null() {
            let path = crate::jsstr_to_rust_string(cx, s);
            match rm_with_options(&path, true, true) {
                ::std::result::Result::Ok(()) => resolve_undefined(cx, promise.get()),
                ::std::result::Result::Err(e) => reject_with_error(
                    cx,
                    promise.get(),
                    &format!("remove '{}': {}", path, e),
                ),
            }
            args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
            return true;
        }
    }
    resolve_undefined(cx, promise.get());
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

/// fs.promises.watch(filename[, options]) → the async-iterable event stream
/// ITSELF. node v24.19.0 oracle: `then: undefined` (NOT a Promise),
/// `[Symbol.asyncIterator]()` on the return value — `for await (const
/// {eventType, filename} of watch(dir, opts))` iterates the RETURN value
/// (the former Promise-wrapping shape made the watch-ignore corpus fail with
/// "watcher is not iterable").
///
/// options.ignore (string glob | RegExp | fn | mixed array — shapes already
/// validated by fs.watch above, so a bad option surfaces as the face's
/// synchronous throw, which is exactly the assert.rejects arm
/// watch-ignore-invalid drives) filters the yielded events: a matched
/// filename is never delivered (node: matched entries are not reported).
/// The recursive face's watcher already carries Symbol.asyncIterator (the JS
/// recursive layer filters ignore itself) and passes through untouched.
///
/// @trace REQ-ENG-004 [api:fs.promises.watch] — return-shape + options.ignore
/// contract (watch-ignore-function/glob/regexp/mixed family root).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_watch(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    // Re-enter the callback face with the SAME argv and capture its watcher.
    let ok = fs_watch(cx, argc, vp);
    if !ok {
        // fs.watch threw (bad path / bad ignore shape) — propagate.
        args.rval().set(UndefinedValue());
        return false;
    }
    let watcher_val = *vp; // vp now holds fs.watch's return value
    if !watcher_val.is_object() {
        args.rval().set(watcher_val);
        return true;
    }

    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let watcher_root = watcher_val);
    // The {eventType, filename} async-iterator face with ignore filtering,
    // self-contained (no internal-module dependency): next()/return()/
    // [Symbol.asyncIterator] over the watcher's 'change'/'close' events.
    let iter_src = r#"(function(watcher, options) {
  if (watcher == null || typeof watcher.on !== 'function') return watcher;
  if (watcher[Symbol.asyncIterator]) return watcher;
  const ignore = options && typeof options === 'object' ? options.ignore : undefined;
  const matchers = [];
  if (ignore != null) {
    const items = Array.isArray(ignore) ? ignore : [ignore];
    for (let i = 0; i < items.length; i++) {
      const item = items[i];
      if (typeof item === 'string') {
        // glob → RegExp (the node internal minimatch subset the watch-ignore
        // corpus drives: * / ** / ? / literal; slash-less patterns get
        // matchBase semantics — they match the basename).
        let src = '';
        for (let j = 0; j < item.length; j++) {
          const c = item[j];
          if (c === '*') {
            if (item[j + 1] === '*') {
              while (item[j + 1] === '*') j++;
              if (item[j + 1] === '/') { src += '(?:[^/]*/)*'; j++; }
              else src += '.*';
            } else src += '[^/]*';
          } else if (c === '?') {
            src += '[^/]';
          } else if ('\\^$.|+()[]{}'.indexOf(c) !== -1) {
            src += '\\' + c;
          } else {
            src += c;
          }
        }
        const re = item.indexOf('/') !== -1
          ? new RegExp('^' + src + '$')
          : new RegExp('(^|/)(' + src + ')$');
        matchers.push(function(f) { return re.test(f); });
      } else if (item instanceof RegExp) {
        matchers.push(function(f) { return item.test(f); });
      } else if (typeof item === 'function') {
        matchers.push(item);
      } else {
        const err = new TypeError('The "options.ignore" property must be one of type string or function or an instance of RegExp.');
        err.code = 'ERR_INVALID_ARG_TYPE';
        throw err;
      }
    }
  }
  const ignored = function(filename) {
    if (filename == null || matchers.length === 0) return false;
    for (let i = 0; i < matchers.length; i++) if (matchers[i](filename)) return true;
    return false;
  };
  let pending = null;
  let done = false;
  watcher.on('change', function(eventType, filename) {
    if (ignored(filename)) return;
    if (pending) { const p = pending; pending = null; p.resolve({ value: { eventType: eventType, filename: filename }, done: false }); }
  });
  watcher.on('close', function() {
    done = true;
    if (pending) { const p = pending; pending = null; p.resolve({ done: true }); }
  });
  return {
    next: function() {
      if (done) return Promise.resolve({ value: undefined, done: true });
      if (pending) return pending.promise;
      let resolve;
      const promise = new Promise(function(r) { resolve = r; });
      pending = { promise: promise, resolve: resolve };
      return promise;
    },
    return: function() {
      done = true;
      if (typeof watcher.close === 'function') { try { watcher.close(); } catch (e) {} }
      return Promise.resolve({ value: undefined, done: true });
    },
    [Symbol.asyncIterator]: function() { return this; },
  };
})"#;
    let mut src_text = mozjs::rust::transform_str_to_source_text(iter_src);
    let mut factory_val = UndefinedValue();
    let factory_h = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut factory_val,
    };
    let compile_opts = mozjs::glue::NewCompileOptions(cx, c"<fs.promises.watch>".as_ptr(), 1);
    if compile_opts.is_null() {
        args.rval().set(watcher_val);
        return true;
    }
    let ok = unsafe { mozjs_sys::jsapi::JS::Evaluate2(cx, compile_opts, &mut src_text, factory_h) };
    mozjs::glue::DeleteCompileOptions(compile_opts);
    if !ok || !factory_val.is_object() {
        if !ok {
            JS_ClearPendingException(cx);
        }
        // Iterator-face absence must never drop the watcher itself (events
        // still flow through the EventEmitter face).
        args.rval().set(watcher_val);
        return true;
    }
    let options_val = if argc > 1 { *args.get(1).ptr } else { UndefinedValue() };
    rooted!(&in(cx_ref) let global_root = JS::CurrentGlobalOrNull(cx));
    if global_root.get().is_null() {
        args.rval().set(watcher_val);
        return true;
    }
    rooted!(&in(cx_ref) let factory_root = factory_val);
    rooted!(&in(cx_ref) let options_root = options_val);
// unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
    rooted!(&in(cx_ref) let call_args_rooted = mozjs_sys::jsgc::ValueArray::<2>::new([
        watcher_root.get(),
        options_root.get(),
    ]));
    let call_args = JS::HandleValueArray::from(&call_args_rooted);
    let mut iter_val = UndefinedValue();
    let call_ok = unsafe {
        JS_CallFunctionValue(
            cx,
            global_root.handle().into(),
            factory_root.handle().into(),
            &call_args,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut iter_val,
            },
        )
    };
    if !call_ok || !iter_val.is_object() {
        if !call_ok {
            // A malformed ignore ARRAY ELEMENT throws here (fs.watch's
            // up-front check validates the container shape only) — surface it.
            return false;
        }
        args.rval().set(watcher_val);
        return true;
    }
    rooted!(&in(cx_ref) let iter_rooted = iter_val);
    args.rval().set(iter_rooted.get());
    true
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
    // node getValidatedFd (test-fs-fsync): non-number fd → ERR_INVALID_ARG_TYPE.
    let fd = match fs_validated_fd(cx, fd_val) {
        ::std::result::Result::Ok(fd) => fd,
        ::std::result::Result::Err(b) => return b,
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
    let atime = match parse_fs_time_arg(cx, atime_val, "atime") {
        ::std::result::Result::Ok(t) => t,
        ::std::result::Result::Err(b) => return b,
    };
    let mtime = match parse_fs_time_arg(cx, mtime_val, "mtime") {
        ::std::result::Result::Ok(t) => t,
        ::std::result::Result::Err(b) => return b,
    };
    #[cfg(unix)]
    {
        match futimens_fd(fd, atime, mtime) {
            ::std::result::Result::Ok(()) => {
                args.rval().set(UndefinedValue());
                true
            }
            ::std::result::Result::Err(e) => throw_fs_error(
                cx,
                "futimesSync",
                &format!("fd:{}", fd),
                &e,
            ),
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
            // Checked view extraction (view_byte_slot) — same class fix as
            // the fs path face (REQ-ENG-007): non-view entries must error,
            // not take the unchecked typed-array cast.
            let Some((data, len)) = view_byte_slot(elem) else {
                JS_ReportErrorUTF8(
                    cx,
                    c"readvSync: buffers must be an array of Buffers".as_ptr(),
                );
                return false;
            };
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
                // Checked view extraction (view_byte_slot) — same class fix
                // as the fs path face (REQ-ENG-007): non-view entries skip
                // instead of taking the unchecked typed-array cast.
                if let Some((data, len)) = view_byte_slot(elem) {
                    if len > 0 {
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
// with no brace/char-class/dotfile semantics and no `dot` switch. That fix
// routed through the workspace bun_glob GlobWalker, whose language diverges
// from node fs.glob on every long-tail shape (no `.`/`..` parts, no `**`
// zero-dir tail, no trailing-slash dir contract, no arm-scoped seen cache,
// no symlink-descent blocking) — 342/449 golden tests red. Replaced by the
// node v24.19.0 walk engine + minimatch 10.2.5 preprocess port below (see
// the engine header for the three corpus-mandated deviations):
//   pattern: string | string[]
//   options: { cwd (default process cwd), root (fallback start dir),
//              followSymlinks (default false), withFileTypes (default false),
//              exclude (fn | string[]; resolved against cwd, absolute-matched) }
// Node fs.glob yields paths RELATIVE to the start dir; absolute patterns
// yield absolute paths.

/// Options parsed from the JS `options` argument of glob/globSync.
struct GlobOptions {
    start_dir: String,
    /// node v24 fs.glob has no `dot` option (minimatch runs dot:false fixed);
    /// parsed for backwards tolerance and ignored by the engine.
    #[allow(dead_code)]
    dot: bool,
    /// node fs.glob `followSymlinks` (default false).
    follow_symlinks: bool,
    /// node fs.glob `withFileTypes` — results are Dirent instances.
    with_file_types: bool,
    /// JS exclude predicate. Payload shape follows with_file_types (node v24
    /// glob.js #excludeAdapter): Dirent instances when set, path strings when
    /// not.
    ///
    /// CAPTURE-TIME carrier only: `glob_collect` inserts it into gc_store
    /// before the first GC point and holds the KEY for the whole walk — this
    /// bare pointer must never be dereferenced past that insert (raw pointer
    /// across GC points = stale after the first callback-run GC).
    exclude_fn: Option<*mut JSObject>,
    /// Exclude glob patterns — RESOLVED against start_dir at parse time
    /// (node v24 glob.js constructor: matching happens on absolute paths).
    exclude_globs: Vec<String>,
}

// ═══════════════════════════════════════════════════════════════════════════
// @trace REQ-ENG-007 [algorithm:fs.glob-walk] — node v24.19.0
// lib/internal/fs/glob.js walk + minimatch 10.2.5 preprocessing +
// brace-expansion 5.0.5, ported verbatim from the oracle's own sources
// (fetched at the v24.19.0 tags) and validated 92/92 against the golden
// corpus (test-fs-glob.mjs: 29 patterns × sync face + patterns2 excludes +
// 29 exclude-empty + followSymlinks + ENOTDIR + seen-cache).
//
// Three corpus-mandated deviations from v24.19.0 (corpus is final truth;
// v24.19.0 fails its own golden on these — measured):
//  1. GLOBSTAR-matched symlink entries never produce descent-capable
//     subpatterns (v24's `|| at(0) === '.'` / `|| entry.isSymbolicLink()`
//     clause descends through symlinks; corpus `**/a/**`, `./**/a/**`,
//     `a/symlink/a/**/*`, `./**/a/**/a/**/`, `./**/a/**/` say no).
//  2. Seen-cache keys are arm-scoped: v24 keys on the raw pattern-suffix
//     string, which collides across brace arms and drops sibling-arm results
//     (corpus `a/!(symlink)/**/../*/*` needs both arms).
//  3. No child-level seen abort: v24 aborts the whole sibling scan when a
//     child path was already registered for the current index — its own
//     seen-cache corpus test (gh-62897) fails on v24.19.0 (measured). The
//     corpus requires all siblings; the top-level state cacheAdd already
//     dedupes re-processed states.
// ═══════════════════════════════════════════════════════════════════════════

/// UV_DIRENT_* type codes shared with internal/fs/utils (1=file 2=dir 3=link).
const GLOB_UV_FILE: i32 = 1;
const GLOB_UV_DIR: i32 = 2;
const GLOB_UV_LINK: i32 = 3;

// ── path helpers (node path.posix subset used by the walk) ─────────────────
fn glob_normalize_posix(p: &str) -> String {
    let absolute = p.starts_with('/');
    let mut res: Vec<&str> = Vec::new();
    for part in p.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            if let Some(last) = res.last() {
                if *last != ".." {
                    res.pop();
                    continue;
                }
            }
            if !absolute {
                res.push("..");
            }
        } else {
            res.push(part);
        }
    }
    let mut out = res.join("/");
    if absolute {
        out.insert(0, '/');
    }
    if out.is_empty() {
        out.push('.');
    }
    out
}

fn glob_join_posix(a: &str, b: &str) -> String {
    let a = if a == "." { "" } else { a };
    let b = if b == "." { "" } else { b };
    let joined = if a.is_empty() {
        b.to_string()
    } else if b.is_empty() {
        a.to_string()
    } else {
        format!("{a}/{b}")
    };
    if joined.is_empty() {
        return ".".to_string();
    }
    glob_normalize_posix(&joined)
}

fn glob_resolve_root(root: &str, p: &str) -> String {
    if p.starts_with('/') {
        return glob_normalize_posix(p);
    }
    glob_normalize_posix(&format!("{root}/{p}"))
}

fn glob_basename(p: &str) -> &str {
    match p.rfind('/') {
        Some(i) => &p[i + 1..],
        None => p,
    }
}

fn glob_dirname(p: &str) -> &str {
    match p.rfind('/') {
        Some(0) => "/",
        Some(i) => &p[..i],
        None => ".",
    }
}

// ── balanced-match + brace-expansion 5.0.5 port ────────────────────────────
/// balanced-match: first `{`, then its matching `}` (depth-counted, `\`-aware).
fn glob_balanced(s: &str) -> ::std::option::Option<(usize, usize)> {
    let bytes: Vec<char> = s.chars().collect();
    let open = match bytes.iter().position(|&c| c == '{') {
        Some(i) => i,
        None => return None,
    };
    let mut depth = 0usize;
    let mut i = open;
    while i < bytes.len() {
        if bytes[i] == '\\' {
            i += 2;
            continue;
        }
        if bytes[i] == '{' {
            depth += 1;
        } else if bytes[i] == '}' {
            depth -= 1;
            if depth == 0 {
                return Some((open, i));
            }
        }
        i += 1;
    }
    None
}

const GLOB_EXPANSION_MAX: usize = 100_000;

fn glob_parse_comma_parts(s: &str) -> Vec<String> {
    if s.is_empty() {
        return vec![String::new()];
    }
    let (open, close) = match glob_balanced(s) {
        Some((o, c)) => (o, c),
        None => return s.split(',').map(|p| p.to_string()).collect(),
    };
    let chars: Vec<char> = s.chars().collect();
    let pre: String = chars[..open].iter().collect();
    let body: String = chars[open + 1..close].iter().collect();
    let post: String = chars[close + 1..].iter().collect();
    let mut p: Vec<String> = pre.split(',').map(|x| x.to_string()).collect();
    let last = p.len() - 1;
    p[last] = format!("{}{{{}}}", p[last], body);
    let post_parts = glob_parse_comma_parts(&post);
    if !post.is_empty() {
        let mut it = post_parts.into_iter();
        if let Some(first) = it.next() {
            p[last] = format!("{}{}", p[last], first);
        }
        p.extend(it);
    }
    p
}

fn glob_expand_(s: &str, max: usize, is_top: bool, out: &mut Vec<String>) {
    let (open, close) = match glob_balanced(s) {
        Some(pair) => pair,
        None => {
            out.push(s.to_string());
            return;
        }
    };
    let chars: Vec<char> = s.chars().collect();
    let pre: String = chars[..open].iter().collect();
    let body: String = chars[open + 1..close].iter().collect();
    let post_str: String = chars[close + 1..].iter().collect();
    let mut post: Vec<String> = Vec::new();
    if !post_str.is_empty() {
        glob_expand_(&post_str, max, false, &mut post);
    } else {
        post.push(String::new());
    }
    if pre.ends_with('$') {
        for k in 0..post.len().min(max) {
            out.push(format!("{pre}{{{body}}}{}", post[k]));
        }
        return;
    }
    let is_numeric_sequence = {
        // ^-?\d+\.\.-?\d+(?:\.\.-?\d+)?$
        let seq_parts: Vec<&str> = body.split("..").collect();
        (seq_parts.len() == 2 || seq_parts.len() == 3)
            && seq_parts.iter().all(|t| t.parse::<i64>().is_ok())
            && !body.is_empty()
    };
    let is_alpha_sequence = {
        // ^[a-zA-Z]\.\.[a-zA-Z](?:\.\.-?\d+)?$
        let b: Vec<char> = body.chars().collect();
        b.len() >= 4
            && b[0].is_ascii_alphabetic()
            && b[1] == '.'
            && b[2] == '.'
            && b[3].is_ascii_alphabetic()
            && (b.len() == 4
                || (b.len() > 6
                    && b[4] == '.'
                    && b[5] == '.'
                    && b[6..].iter().all(|c| c.is_ascii_digit())
                    && !b[6..].is_empty()))
    };
    let is_sequence = is_numeric_sequence || is_alpha_sequence;
    let is_options = body.contains(',');
    if !is_sequence && !is_options {
        // `{a},b}` recovery (brace-expansion: post matches /,(?!,).*\}/):
        // hide the group's `}` behind the ESCCLOSE sentinel and restart from
        // the top; the sentinel is restored to '}' by glob_brace_expand.
        let has_comma_close = {
            let pc: Vec<char> = post_str.chars().collect();
            let mut found = false;
            for (i2, c) in pc.iter().enumerate() {
                if *c == ',' && pc.get(i2 + 1) != Some(&',') {
                    if pc[i2 + 1..].contains(&'}') {
                        found = true;
                    }
                    break;
                }
            }
            found
        };
        if has_comma_close {
            let rebuilt = format!("{}{{{}}}\0{}", pre, body, post_str);
            glob_expand_(&rebuilt, max, true, out);
            return;
        }
        out.push(s.to_string());
        return;
    }
    let mut n: Vec<String> = Vec::new();
    if is_sequence {
        n = body.split("..").map(|x| x.to_string()).collect();
    } else {
        let mut parts = glob_parse_comma_parts(&body);
        if parts.len() == 1 {
            let mut expanded = Vec::new();
            glob_expand_(&parts[0], max, false, &mut expanded);
            if expanded.len() == 1 {
                for p2 in &post {
                    out.push(format!("{}{{{}}}{}", pre, expanded[0], p2));
                }
                return;
            }
            parts = expanded.into_iter().map(|e| format!("{{{e}}}")).collect();
        }
        n = parts;
    }
    let mut big_n: Vec<String> = Vec::new();
    if is_sequence && n.len() >= 2 {
        let numeric = |t: &str| -> i64 { t.parse::<i64>().unwrap_or_else(|_| t.chars().next().map(|c| c as i64).unwrap_or(0)) };
        let x = numeric(&n[0]);
        let y = numeric(&n[1]);
        let width = n[0].chars().count().max(n[1].chars().count());
        let mut incr: i64 = if n.len() == 3 { numeric(&n[2]).abs().max(1) } else { 1 };
        let reverse = y < x;
        if reverse {
            incr = -incr;
        }
        let pad = n.iter().any(|t| {
            let t = t.strip_prefix('-').unwrap_or(t);
            t.len() > 1 && t.starts_with('0')
        });
        let mut i = x;
        loop {
            let done = if reverse { i < y } else { i > y };
            if done {
                break;
            }
            let mut c = if is_alpha_sequence {
                match char::from_u32(i as u32) {
                    Some(ch) if ch != '\\' => ch.to_string(),
                    _ => String::new(),
                }
            } else {
                let mut cs = i.to_string();
                if pad {
                    let need = width.saturating_sub(cs.chars().count());
                    if need > 0 {
                        let z = "0".repeat(need);
                        cs = if i < 0 {
                            format!("-{z}{}", cs.strip_prefix('-').unwrap_or(&cs))
                        } else {
                            format!("{z}{cs}")
                        };
                    }
                }
                cs
            };
            if is_alpha_sequence && c == "\\" {
                c = String::new();
            }
            big_n.push(c);
            i += incr;
        }
    } else {
        for item in &n {
            let mut sub = Vec::new();
            glob_expand_(item, max, false, &mut sub);
            big_n.extend(sub);
        }
    }
    for item in &big_n {
        for p2 in &post {
            if out.len() >= max {
                return;
            }
            let expansion = format!("{pre}{item}{p2}");
            if !is_top || is_sequence || !expansion.is_empty() {
                out.push(expansion);
            }
        }
    }
}

fn glob_brace_expand(pattern: &str) -> Vec<String> {
    if pattern.is_empty() {
        return Vec::new();
    }
    let mut p = pattern.to_string();
    if p.starts_with("{}") {
        p = format!("\\{{\\}}{}", &p[2..]);
    }
    // The char-based balancer below is `\`-pair-aware (escaped braces are
    // skipped during matching), so brace-expansion's escapeBraces sentinel
    // pass is unnecessary; the ESCCLOSE sentinel ('\0') from the `{a},b}`
    // recovery is restored here.
    let mut out = Vec::new();
    glob_expand_(&p, GLOB_EXPANSION_MAX, true, &mut out);
    out.into_iter().map(|s| s.replace('\u{0}', "}")).collect()
}

// ── minimatch preprocess (slashSplit + firstPhase + secondPhase) ───────────
fn glob_slash_split(p: &str) -> Vec<String> {
    // split(/\/+/): coalesce runs of slashes; '' parts preserved at the
    // string edges (leading '' = absolute, trailing '' = trailing slash).
    let mut parts: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut prev_slash = false;
    for c in p.chars() {
        if c == '/' {
            if !prev_slash {
                parts.push(::std::mem::take(&mut cur));
            }
            prev_slash = true;
        } else {
            cur.push(c);
            prev_slash = false;
        }
    }
    parts.push(cur);
    parts
}

fn glob_first_phase(parts_list: &mut Vec<Vec<String>>) {
    let mut did_something = false;
    loop {
        did_something = false;
        // The JS iterates globParts with for-of WHILE pushing new split arms
        // into it; Rust cannot push during a borrow, so arms are taken out
        // one index at a time and spawned arms are appended after processing.
        // The outer do/while rescans everything to fixpoint either way.
        let mut spawned: Vec<Vec<String>> = Vec::new();
        let mut idx = 0usize;
        while idx < parts_list.len() {
            let mut parts = ::std::mem::take(&mut parts_list[idx]);
            // collapse adjacent `**` and split `<pre>/**/../<p>/<p2>/<rest>`
            let mut gs = 0usize;
            while let Some(found) = parts[gs.min(parts.len())..].iter().position(|p| p == "**") {
                let gs_abs = gs.min(parts.len()) + found;
                let mut gss = gs_abs;
                while parts.get(gss + 1).map(|p| p == "**").unwrap_or(false) {
                    gss += 1;
                }
                if gss > gs_abs {
                    parts.drain(gs_abs + 1..gss + 1);
                }
                let next_ok = parts.get(gs_abs + 1).map(|p| p == "..").unwrap_or(false);
                let p1 = parts.get(gs_abs + 2).cloned().unwrap_or_default();
                let p2 = parts.get(gs_abs + 3).cloned().unwrap_or_default();
                if !next_ok
                    || p1.is_empty()
                    || p1 == "."
                    || p1 == ".."
                    || p2.is_empty()
                    || p2 == "."
                    || p2 == ".."
                {
                    gs = gs_abs + 1;
                    if gs >= parts.len() {
                        break;
                    }
                    continue;
                }
                did_something = true;
                parts.remove(gs_abs);
                let mut other = parts.clone();
                other[gs_abs] = "**".to_string();
                spawned.push(other);
                // JS: gs-- then indexOf('**', gs + 1) — rescan this position
                gs = gs_abs.saturating_sub(1);
                if gs >= parts.len() {
                    break;
                }
            }
            // <pre>/<e>/<rest> → <pre>/<rest>: drop mid '.' / ''
            let mut i = 1usize;
            while i + 1 < parts.len() {
                let p = parts[i].clone();
                if i == 1 && p.is_empty() && parts[0].is_empty() {
                    i += 1;
                    continue;
                }
                if p == "." || p.is_empty() {
                    did_something = true;
                    parts.remove(i);
                    if i > 1 {
                        i -= 1;
                    }
                    continue;
                }
                i += 1;
            }
            if parts.len() == 2 && parts[0] == "." && (parts[1] == "." || parts[1].is_empty()) {
                did_something = true;
                parts.pop();
            }
            // <pre>/<p>/../<rest> → <pre>/<rest>
            let mut search_from = 0usize;
            loop {
                if search_from >= parts.len() {
                    break;
                }
                let Some(dd_abs) = parts[search_from..].iter().position(|p| p == "..").map(|f| search_from + f)
                else {
                    break;
                };
                let prev_ok = if dd_abs == 0 {
                    false
                } else {
                    let p = &parts[dd_abs - 1];
                    !p.is_empty() && p != "." && p != ".." && p != "**"
                };
                if prev_ok {
                    did_something = true;
                    let need_dot = dd_abs == 1 && parts.get(dd_abs + 1).map(|p| p == "**").unwrap_or(false);
                    let splin: Vec<String> = if need_dot { vec![".".to_string()] } else { Vec::new() };
                    parts.splice(dd_abs - 1..dd_abs + 1, splin);
                    if parts.is_empty() {
                        parts.push(String::new());
                    }
                    // JS: dd -= 2 → next indexOf resumes at dd - 1
                    search_from = dd_abs.saturating_sub(2);
                } else {
                    search_from = dd_abs + 1;
                }
            }
            parts_list[idx] = parts;
            idx += 1;
        }
        parts_list.extend(spawned);
        if !did_something {
            break;
        }
    }
}

fn glob_parts_match(a: &[String], b: &[String], empty_gs_match: bool) -> ::std::option::Option<Vec<String>> {
    let mut ai = 0usize;
    let mut bi = 0usize;
    let mut result: Vec<String> = Vec::new();
    let mut which = ' ';
    while ai < a.len() && bi < b.len() {
        if a[ai] == b[bi] {
            result.push(if which == 'b' { b[bi].clone() } else { a[ai].clone() });
            ai += 1;
            bi += 1;
        } else if empty_gs_match && a[ai] == "**" && b.get(bi) == a.get(ai + 1) {
            result.push(a[ai].clone());
            ai += 1;
        } else if empty_gs_match && b[bi] == "**" && a.get(ai) == b.get(bi + 1) {
            result.push(b[bi].clone());
            bi += 1;
        } else if a[ai] == "*" && !b[bi].is_empty() && !b[bi].starts_with('.') && b[bi] != "**" {
            if which == 'b' {
                return None;
            }
            which = 'a';
            result.push(a[ai].clone());
            ai += 1;
            bi += 1;
        } else if b[bi] == "*" && !a[ai].is_empty() && !a[ai].starts_with('.') && a[ai] != "**" {
            if which == 'a' {
                return None;
            }
            which = 'b';
            result.push(b[bi].clone());
            ai += 1;
            bi += 1;
        } else {
            return None;
        }
    }
    if a.len() == b.len() {
        Some(result)
    } else {
        None
    }
}

fn glob_second_phase(parts_list: &mut Vec<Vec<String>>) {
    let mut i = 0usize;
    while i + 1 < parts_list.len() {
        let mut j = i + 1;
        while j < parts_list.len() {
            let matched = glob_parts_match(&parts_list[i], &parts_list[j], true);
            if let Some(m) = matched {
                parts_list[i] = Vec::new();
                parts_list[j] = m;
                break;
            }
            j += 1;
        }
        i += 1;
    }
    parts_list.retain(|gs| !gs.is_empty());
}

// ── single-segment matcher (minimatch part semantics, dot:false) ───────────
#[derive(Clone)]
enum GlobTok {
    Lit(char),
    AnyOne,
    AnySeq,
    Class { neg: bool, items: Vec<GlobClassItem>, dot_member: bool },
    Ext { kind: char, alts: Vec<Vec<GlobTok>> },
}

#[derive(Clone)]
struct GlobClassItem {
    lo: char,
    hi: char,
}

struct GlobMatcher {
    toks: Vec<GlobTok>,
    dot_locked: bool,
}

fn glob_has_magic(part: &str) -> bool {
    let ch: Vec<char> = part.chars().collect();
    let mut i = 0usize;
    while i < ch.len() {
        match ch[i] {
            '?' | '*' | '[' | ']' => return true,
            '+' | '@' | '!' => {
                if ch.get(i + 1) == Some(&'(') {
                    return true;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    false
}

fn glob_unescape_part(part: &str) -> String {
    let ch: Vec<char> = part.chars().collect();
    let mut out = String::new();
    let mut i = 0usize;
    while i < ch.len() {
        if ch[i] == '\\' && i + 1 < ch.len() {
            out.push(ch[i + 1]);
            i += 2;
        } else {
            out.push(ch[i]);
            i += 1;
        }
    }
    out
}

fn glob_parse_tokens(s: &str) -> Vec<GlobTok> {
    let ch: Vec<char> = s.chars().collect();
    let mut toks: Vec<GlobTok> = Vec::new();
    let mut i = 0usize;
    while i < ch.len() {
        let c = ch[i];
        if c == '\\' && i + 1 < ch.len() {
            toks.push(GlobTok::Lit(ch[i + 1]));
            i += 2;
            continue;
        }
        // extglob check BEFORE bare * / ? — a following '(' makes it an extglob
        if (c == '+' || c == '@' || c == '!' || c == '?' || c == '*') && ch.get(i + 1) == Some(&'(') {
            // find matching close paren (depth-counted, escape-aware)
            let mut depth = 0i32;
            let mut j = i + 1;
            while j < ch.len() {
                if ch[j] == '\\' {
                    j += 2;
                    continue;
                }
                if ch[j] == '(' {
                    depth += 1;
                } else if ch[j] == ')' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                j += 1;
            }
            let body: String = ch[i + 2..j.min(ch.len())].iter().collect();
            // split top-level '|'
            let mut alts: Vec<String> = Vec::new();
            let mut cur = String::new();
            let mut d = 0i32;
            let bc: Vec<char> = body.chars().collect();
            let mut x = 0usize;
            while x < bc.len() {
                if bc[x] == '\\' && x + 1 < bc.len() {
                    cur.push(bc[x]);
                    cur.push(bc[x + 1]);
                    x += 2;
                    continue;
                }
                if bc[x] == '(' {
                    d += 1;
                }
                if bc[x] == ')' {
                    d -= 1;
                }
                if bc[x] == '|' && d == 0 {
                    alts.push(::std::mem::take(&mut cur));
                    x += 1;
                    continue;
                }
                cur.push(bc[x]);
                x += 1;
            }
            alts.push(cur);
            toks.push(GlobTok::Ext {
                kind: c,
                alts: alts.iter().map(|a| glob_parse_tokens(a)).collect(),
            });
            i = j + 1;
            continue;
        }
        if c == '*' {
            toks.push(GlobTok::AnySeq);
            i += 1;
            continue;
        }
        if c == '?' {
            toks.push(GlobTok::AnyOne);
            i += 1;
            continue;
        }
        if c == '[' {
            let mut j = i + 1;
            let mut neg = false;
            if ch.get(j) == Some(&'!') || ch.get(j) == Some(&'^') {
                neg = true;
                j += 1;
            }
            let mut items: Vec<GlobClassItem> = Vec::new();
            let mut first = true;
            let mut dot_member = false;
            while j < ch.len() && (ch[j] != ']' || first) {
                if j + 2 < ch.len() && ch[j + 1] == '-' && ch[j + 2] != ']' {
                    items.push(GlobClassItem { lo: ch[j], hi: ch[j + 2] });
                    if first && ch[j] == '.' {
                        dot_member = true;
                    }
                    j += 3;
                } else {
                    items.push(GlobClassItem { lo: ch[j], hi: ch[j] });
                    if first && ch[j] == '.' {
                        dot_member = true;
                    }
                    j += 1;
                }
                first = false;
            }
            toks.push(GlobTok::Class { neg, items, dot_member });
            i = j + 1;
            continue;
        }
        toks.push(GlobTok::Lit(c));
        i += 1;
    }
    toks
}

fn glob_toks_dot_locked(toks: &[GlobTok]) -> bool {
    match toks.first() {
        None => false,
        Some(GlobTok::Lit(_)) => false,
        Some(GlobTok::Class { dot_member, .. }) => !dot_member,
        Some(_) => true,
    }
}

fn glob_cls_match(neg: bool, items: &[GlobClassItem], c: char) -> bool {
    let mut hit = false;
    for item in items {
        let cp = c as u32;
        if cp >= item.lo as u32 && cp <= item.hi as u32 {
            hit = true;
            break;
        }
    }
    if neg {
        !hit
    } else {
        hit
    }
}

/// End positions reachable in `s` starting at `si` after consuming all of
/// `toks`. Memo keyed on the token-slice identity (the compiled tree is
/// immutable during matching, so slice addresses are stable).
fn glob_tok_ends(
    toks: &[GlobTok],
    s: &[char],
    si: usize,
    memo: &mut HashMap<(usize, usize), BTreeSet<usize>>,
) -> BTreeSet<usize> {
    let key = (toks.as_ptr() as usize, si);
    if let Some(hit) = memo.get(&key) {
        return hit.clone();
    }
    let mut ends: BTreeSet<usize> = BTreeSet::new();
    memo.insert(key, ends.clone()); // cycle guard (empty-alt repetition)
    if toks.is_empty() {
        ends.insert(si);
        memo.insert(key, ends.clone());
        return ends;
    }
    let rest_toks = &toks[1..];
    match &toks[0] {
        GlobTok::Lit(c) => {
            if s.get(si) == Some(c) {
                for e in glob_tok_ends(rest_toks, s, si + 1, memo) {
                    ends.insert(e);
                }
            }
        }
        GlobTok::AnyOne => {
            if si < s.len() {
                for e in glob_tok_ends(rest_toks, s, si + 1, memo) {
                    ends.insert(e);
                }
            }
        }
        GlobTok::Class { neg, items, .. } => {
            if si < s.len() && glob_cls_match(*neg, items, s[si]) {
                for e in glob_tok_ends(rest_toks, s, si + 1, memo) {
                    ends.insert(e);
                }
            }
        }
        GlobTok::AnySeq => {
            let mut j = s.len() + 1;
            while j > si {
                j -= 1;
                for e in glob_tok_ends(rest_toks, s, j, memo) {
                    ends.insert(e);
                }
            }
        }
        GlobTok::Ext { kind, alts } => {
            let one_rep = |pos: usize, memo: &mut HashMap<(usize, usize), BTreeSet<usize>>| -> BTreeSet<usize> {
                let mut r = BTreeSet::new();
                for alt in alts {
                    for e in glob_tok_ends(alt, s, pos, memo) {
                        r.insert(e);
                    }
                }
                r
            };
            match kind {
                '@' => {
                    for e in one_rep(si, memo) {
                        for f in glob_tok_ends(rest_toks, s, e, memo) {
                            ends.insert(f);
                        }
                    }
                }
                '?' => {
                    for f in glob_tok_ends(rest_toks, s, si, memo) {
                        ends.insert(f);
                    }
                    for e in one_rep(si, memo) {
                        for f in glob_tok_ends(rest_toks, s, e, memo) {
                            ends.insert(f);
                        }
                    }
                }
                '+' | '*' => {
                    let mut acc: BTreeSet<usize> = BTreeSet::new();
                    if *kind == '*' {
                        acc.insert(si);
                    }
                    let mut frontier: BTreeSet<usize> = BTreeSet::new();
                    frontier.insert(si);
                    let mut global_seen: BTreeSet<usize> = BTreeSet::new();
                    global_seen.insert(si);
                    let mut reps = 0usize;
                    while !frontier.is_empty() && reps <= s.len() + 1 {
                        let mut next: BTreeSet<usize> = BTreeSet::new();
                        for pos in &frontier {
                            for e in one_rep(*pos, memo) {
                                if !global_seen.contains(&e) {
                                    global_seen.insert(e);
                                    next.insert(e);
                                }
                            }
                        }
                        reps += 1;
                        for e in &next {
                            acc.insert(*e);
                        }
                        frontier = next;
                    }
                    for e in acc {
                        for f in glob_tok_ends(rest_toks, s, e, memo) {
                            ends.insert(f);
                        }
                    }
                }
                '!' => {
                    // minimatch `!` semantics (part regex
                    // `(?!(?:alt+REST-anchored))[^/]*?REST`): forbidden when
                    // some alt matches s[..q] and REST matches s[q..len]
                    // fully; a lone-star REST is non-empty inside the
                    // lookahead (minimatch compiles it `[^/]+?`), forcing
                    // q < len there. Match = some split p with REST matching
                    // s[p..len] fully and not forbidden.
                    let rest_is_lone_star = rest_toks.len() == 1 && matches!(rest_toks[0], GlobTok::AnySeq);
                    let mut forbidden = false;
                    for alt in alts {
                        if forbidden {
                            break;
                        }
                        for q in 1..=s.len() {
                            if rest_is_lone_star && q >= s.len() {
                                break;
                            }
                            if glob_tok_ends(alt, s, 0, memo).contains(&q)
                                && glob_tok_ends(rest_toks, s, q, memo).contains(&s.len())
                            {
                                forbidden = true;
                                break;
                            }
                        }
                    }
                    if !forbidden {
                        for p in si..=s.len() {
                            if glob_tok_ends(rest_toks, s, p, memo).contains(&s.len()) {
                                ends.insert(p);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    memo.insert(key, ends.clone());
    ends
}

fn glob_match_seg(m: &GlobMatcher, name: &str) -> bool {
    if m.dot_locked && name.starts_with('.') {
        return false;
    }
    let s: Vec<char> = name.chars().collect();
    let mut memo: HashMap<(usize, usize), BTreeSet<usize>> = HashMap::new();
    glob_tok_ends(&m.toks, &s, 0, &mut memo).contains(&s.len())
}

// ── part classification ────────────────────────────────────────────────────
#[derive(Clone)]
enum GlobPart {
    Globstar { src: String },
    Literal { str_: String, src: String },
    Pattern { m: ::std::rc::Rc<GlobMatcher>, src: String },
}

impl GlobPart {
    fn src(&self) -> &str {
        match self {
            GlobPart::Globstar { src } | GlobPart::Literal { src, .. } | GlobPart::Pattern { src, .. } => src,
        }
    }
    fn is_globstar(&self) -> bool {
        matches!(self, GlobPart::Globstar { .. })
    }
    fn is_literal(&self) -> bool {
        matches!(self, GlobPart::Literal { .. })
    }
    fn literal_str(&self) -> &str {
        match self {
            GlobPart::Literal { str_, .. } => str_,
            _ => "",
        }
    }
    fn test(&self, name: &str) -> bool {
        match self {
            GlobPart::Globstar { .. } => true,
            GlobPart::Literal { str_, .. } => str_ == name,
            GlobPart::Pattern { m, .. } => glob_match_seg(m, name),
        }
    }
}

fn glob_classify_part(src: &str) -> GlobPart {
    if src == "**" {
        return GlobPart::Globstar { src: src.to_string() };
    }
    if !glob_has_magic(src) {
        return GlobPart::Literal { str_: glob_unescape_part(src), src: src.to_string() };
    }
    let toks = glob_parse_tokens(src);
    let dot_locked = glob_toks_dot_locked(&toks);
    GlobPart::Pattern { m: ::std::rc::Rc::new(GlobMatcher { toks, dot_locked }), src: src.to_string() }
}

/// Compile one pattern string → brace-expanded, preprocessed, classified arms
/// (the minimatch globParts pipeline verbatim).
fn glob_compile_pattern(pattern: &str) -> Vec<::std::rc::Rc<Vec<GlobPart>>> {
    let mut glob_parts: Vec<Vec<String>> = glob_brace_expand(pattern)
        .iter()
        .map(|s| glob_slash_split(s))
        .collect();
    glob_first_phase(&mut glob_parts);
    glob_second_phase(&mut glob_parts);
    glob_parts
        .into_iter()
        .map(|parts| ::std::rc::Rc::new(parts.iter().map(|p| glob_classify_part(p)).collect::<Vec<_>>()))
        .collect()
}

/// Full-path match (exclude patterns / matchGlobPattern): each part consumes
/// one '/'-segment, globstar spans segments.
fn glob_match_full_path(parts: &[GlobPart], full: &str) -> bool {
    fn m(parts: &[GlobPart], segs: &[&str], pi: usize, si: usize, memo: &mut HashMap<(usize, usize), bool>) -> bool {
        let key = (pi, si);
        if let Some(hit) = memo.get(&key) {
            return *hit;
        }
        memo.insert(key, false);
        let r = if pi == parts.len() {
            si == segs.len()
        } else if parts[pi].is_globstar() {
            let mut hit = false;
            let mut j = segs.len() + 1;
            while j > si {
                j -= 1;
                if m(parts, segs, pi + 1, j, memo) {
                    hit = true;
                    break;
                }
            }
            hit
        } else if si < segs.len() && parts[pi].test(segs[si]) {
            m(parts, segs, pi + 1, si + 1, memo)
        } else {
            false
        };
        memo.insert(key, r);
        r
    }
    let segs: Vec<&str> = full.split('/').collect();
    let mut memo: HashMap<(usize, usize), bool> = HashMap::new();
    m(parts, &segs, 0, 0, &mut memo)
}

// ── walk engine (node v24.19.0 glob.js Glob, sync face) ────────────────────
#[derive(Clone)]
struct GlobWalkState {
    parts: ::std::rc::Rc<Vec<GlobPart>>,
    indexes: BTreeSet<usize>,
    symlinks: BTreeSet<usize>,
    realpaths: ::std::rc::Rc<BTreeSet<String>>,
}

#[derive(Clone, Copy, PartialEq)]
enum GlobEntryKind {
    Dir,
    File,
    Symlink,
}

struct GlobEngine<'a> {
    root: String,
    /// JS exclude predicate (payload shape follows with_file_types) — held as
    /// a gc_store KEY, never a bare pointer: the object lives as a GC-managed
    /// property on the JS global and every invocation re-resolves a fresh live
    /// address via `gc_store_get`. A bare `*mut JSObject` held here goes stale
    /// on the first GC inside a callback run (nursery evacuation moves the
    /// function object), and the per-call `rooted!` would then root an already
    /// dangling address — the bun_serve SIGSEGV precedent (bun_api.rs, #GP on
    /// a non-canonical stale callee) in the gc_store.rs rooting contract.
    exclude_fn_key: ::std::option::Option<String>,
    cx: *mut JSContext,
    _marker: ::std::marker::PhantomData<&'a ()>,
    with_file_types: bool,
    follow_symlinks: bool,
    arms: Vec<::std::rc::Rc<Vec<GlobPart>>>,
    exclude_matchers: Vec<Vec<GlobPart>>,
    stat_cache: HashMap<String, ::std::option::Option<GlobEntryKind>>,
    follow_cache: HashMap<String, ::std::option::Option<GlobEntryKind>>,
    readdir_cache: HashMap<String, Vec<(String, GlobEntryKind)>>,
    realpath_cache: HashMap<String, ::std::option::Option<String>>,
    seen: ::std::collections::HashSet<String>,
    results: BTreeSet<String>,
    queue: Vec<(String, GlobWalkState)>,
    wave: HashMap<String, Vec<GlobWalkState>>,
}

impl<'a> GlobEngine<'a> {
    fn lstat_kind(&self, p: &str) -> ::std::option::Option<GlobEntryKind> {
        // SAFETY: caller frame holds a JSNative context; these are pure syscalls.
        match unsafe { fs::symlink_metadata(p) } {
            ::std::result::Result::Ok(md) => {
                let ft = md.file_type();
                Some(
                    if ft.is_symlink() {
                        GlobEntryKind::Symlink
                    } else if ft.is_dir() {
                        GlobEntryKind::Dir
                    } else {
                        GlobEntryKind::File
                    },
                )
            }
            ::std::result::Result::Err(_) => None,
        }
    }
    fn stat_kind(&self, p: &str) -> ::std::option::Option<GlobEntryKind> {
        match unsafe { fs::metadata(p) } {
            ::std::result::Result::Ok(md) => Some(
                if md.is_dir() {
                    GlobEntryKind::Dir
                } else if md.is_symlink() {
                    GlobEntryKind::Symlink
                } else {
                    GlobEntryKind::File
                },
            ),
            ::std::result::Result::Err(_) => None,
        }
    }
    fn stat(&mut self, p: &str) -> ::std::option::Option<GlobEntryKind> {
        if let Some(hit) = self.stat_cache.get(p) {
            return *hit;
        }
        let v = self.lstat_kind(p);
        self.stat_cache.insert(p.to_string(), v);
        v
    }
    fn follow_stat(&mut self, p: &str) -> ::std::option::Option<GlobEntryKind> {
        if let Some(hit) = self.follow_cache.get(p) {
            return *hit;
        }
        let v = self.stat_kind(p);
        self.follow_cache.insert(p.to_string(), v);
        v
    }
    fn readdir(&mut self, p: &str) -> Vec<(String, GlobEntryKind)> {
        if let Some(hit) = self.readdir_cache.get(p) {
            return hit.clone();
        }
        let mut val: Vec<(String, GlobEntryKind)> = Vec::new();
        // SAFETY: pure syscall; caller frame is a JSNative.
        if let ::std::result::Result::Ok(entries) = unsafe { fs::read_dir(p) } {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let kind = match entry.file_type() {
                    ::std::result::Result::Ok(ft) => {
                        if ft.is_symlink() {
                            GlobEntryKind::Symlink
                        } else if ft.is_dir() {
                            GlobEntryKind::Dir
                        } else {
                            GlobEntryKind::File
                        }
                    }
                    ::std::result::Result::Err(_) => match self.lstat_kind(p) {
                        Some(k) => k,
                        None => continue,
                    },
                };
                val.push((name, kind));
            }
        }
        // Deterministic order: the walk result is order-independent (BTreeSet
        // results, first-wins wave carries identical states), so sorting only
        // removes filesystem readdir-order nondeterminism.
        val.sort_by(|x, y| x.0.cmp(&y.0));
        self.readdir_cache.insert(p.to_string(), val.clone());
        val
    }
    fn realpath(&mut self, p: &str) -> ::std::option::Option<String> {
        if let Some(hit) = self.realpath_cache.get(p) {
            return hit.clone();
        }
        let v = fs::canonicalize(p).ok().map(|pb| pb.to_string_lossy().into_owned());
        self.realpath_cache.insert(p.to_string(), v.clone());
        v
    }
    fn pattern_cache_key(arm: &[GlobPart], index: usize) -> String {
        // CORPUS deviation 2: keys are arm-scoped (toks-id analog = the arm
        // slice address; the arm Rc target is stable for the engine lifetime).
        let mut key = format!("\u{1}{:p}\u{1}", arm.as_ptr());
        for (i, part) in arm.iter().enumerate().skip(index) {
            key.push_str(part.src());
            if i != arm.len() - 1 {
                key.push('/');
            }
        }
        key
    }
    fn cache_add(&mut self, path: &str, arm: &[GlobPart], indexes: &BTreeSet<usize>) -> bool {
        let mut grew = false;
        for index in indexes {
            let k = format!("{}\u{0}{}", path, Self::pattern_cache_key(arm, *index));
            if self.seen.insert(k) {
                grew = true;
            }
        }
        !grew
    }
    fn seen_p(&self, path: &str, arm: &[GlobPart], index: usize) -> bool {
        self.seen.contains(&format!("{}\u{0}{}", path, Self::pattern_cache_key(arm, index)))
    }
    /// JS exclude predicate check. `dirent` payload = (name, type, parent).
    /// SAFETY: cx must be the calling JSNative's context; the fn object is
    /// re-resolved LIVE from gc_store (fresh address, never a held bare
    /// pointer) and rooted immediately — gc_store_get→rooted! is a GC-free
    /// window, so the root always captures the live address.
    unsafe fn exclude_fn_check(&self, dirent: ::std::option::Option<(&str, i32, &str)>) -> bool {
        let cx = self.cx;
        let Some(cb) = self
            .exclude_fn_key
            .as_ref()
            .and_then(|k| crate::gc_store::gc_store_get(cx, k))
        else {
            return false;
        };
        let wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        rooted!(&in(wrapped_cx) let cb_root = cb);
        let arg: mozjs::jsval::JSVal = if let Some((name, type_code, parent)) = dirent {
            let d = match construct_glob_dirent(cx, name, type_code, parent) {
                ::std::result::Result::Ok(o) => o,
                ::std::result::Result::Err(()) => return false,
            };
            rooted!(&in(wrapped_cx) let d_root = d);
            mozjs::jsval::ObjectValue(d_root.get())
        } else {
            return false; // path payload handled by exclude_fn_check_path
        };
        rooted!(&in(wrapped_cx) let args_rooted = mozjs_sys::jsgc::ValueArray::<1>::new([arg]));
        let call_args = JS::HandleValueArray::from(&args_rooted);
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
        ok && rval.is_boolean() && rval.to_boolean()
    }
    /// Path-string payload variant of the JS exclude predicate.
    /// SAFETY: as `exclude_fn_check` (live gc_store re-resolve + immediate root).
    unsafe fn exclude_fn_check_path(&self, path: &str) -> bool {
        let cx = self.cx;
        let Some(cb) = self
            .exclude_fn_key
            .as_ref()
            .and_then(|k| crate::gc_store::gc_store_get(cx, k))
        else {
            return false;
        };
        let wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        rooted!(&in(wrapped_cx) let cb_root = cb);
        let path_c = ZBox::from_bytes(path.as_bytes());
        let path_js = JS_NewStringCopyZ(cx, path_c.as_ptr());
        if path_js.is_null() {
            return false;
        }
        let arg = mozjs::jsval::StringValue(&*path_js);
        rooted!(&in(wrapped_cx) let args_rooted = mozjs_sys::jsgc::ValueArray::<1>::new([arg]));
        let call_args = JS::HandleValueArray::from(&args_rooted);
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
        ok && rval.is_boolean() && rval.to_boolean()
    }
    fn is_excluded(&self, value: &str) -> bool {
        if self.exclude_matchers.is_empty() {
            return false;
        }
        let full = glob_resolve_root(&self.root, value);
        self.exclude_matchers.iter().any(|parts| glob_match_full_path(parts, &full))
    }
    fn has_seen_symlinks(state: &GlobWalkState) -> bool {
        state.indexes.iter().any(|i| !state.symlinks.contains(i))
    }
    fn is_directory_state(&mut self, fullpath: &str, stat: ::std::option::Option<GlobEntryKind>, state: &GlobWalkState) -> bool {
        if stat == Some(GlobEntryKind::Dir) {
            return true;
        }
        if stat != Some(GlobEntryKind::Symlink) {
            return false;
        }
        if self.follow_symlinks {
            return self.follow_stat(fullpath) == Some(GlobEntryKind::Dir);
        }
        Self::has_seen_symlinks(state)
    }
    fn is_last(state: &GlobWalkState, is_directory: bool) -> bool {
        let last = state.parts.len().saturating_sub(1);
        if state.indexes.contains(&last) {
            return true;
        }
        if last == 0 {
            return false;
        }
        state.parts[last].src().is_empty()
            && is_directory
            && state.indexes.contains(&(last - 1))
            && state.parts[last - 1].is_globstar()
    }
    fn is_cyclic(&mut self, fullpath: &str, is_directory: bool, state: &GlobWalkState) -> bool {
        if !self.follow_symlinks || !is_directory {
            return false;
        }
        match self.realpath(fullpath) {
            Some(real) => state.realpaths.contains(&real),
            None => false,
        }
    }
    fn next_realpaths(&mut self, fullpath: &str, is_directory: bool, state: &GlobWalkState) -> ::std::rc::Rc<BTreeSet<String>> {
        if !self.follow_symlinks || !is_directory {
            return state.realpaths.clone();
        }
        match self.realpath(fullpath) {
            Some(real) => {
                let mut next = (*state.realpaths).clone();
                next.insert(real);
                ::std::rc::Rc::new(next)
            }
            None => state.realpaths.clone(),
        }
    }
    fn add_result(&mut self, path: &str) {
        if self.is_excluded(path) {
            return;
        }
        self.results.insert(path.to_string());
    }
    /// SAFETY: cx is the calling JSNative's context (exclude_fn call).
    unsafe fn add_subpattern(&mut self, path: &str, state: GlobWalkState) {
        let fullpath = glob_resolve_root(&self.root, path);
        if self.is_excluded(&fullpath) {
            return;
        }
        let stat = self.stat(&fullpath);
        if self.is_excluded(&format!("{fullpath}/")) && stat == Some(GlobEntryKind::Dir) {
            return;
        }
        if self.exclude_fn_key.is_some() {
            if self.with_file_types {
                if let Some(kind) = stat {
                    let type_code = glob_entry_type_code(kind);
                    let name = glob_basename(&fullpath).to_string();
                    let parent = glob_dirname(&fullpath).to_string();
                    // SAFETY: JSNative frame; rooted inside.
                    if unsafe { self.exclude_fn_check(Some((&name, type_code, &parent))) } {
                        return;
                    }
                }
            } else {
                // SAFETY: JSNative frame; rooted inside.
                if unsafe { self.exclude_fn_check_path(path) } {
                    return;
                }
            }
        }
        self.wave.entry(path.to_string()).or_default().push(state);
    }
    /// SAFETY: cx is the calling JSNative's context (exclude_fn calls).
    unsafe fn run(&mut self) {
        for parts in &self.arms {
            self.queue.push((
                ".".to_string(),
                GlobWalkState {
                    parts: parts.clone(),
                    indexes: BTreeSet::from([0usize]),
                    symlinks: BTreeSet::new(),
                    realpaths: ::std::rc::Rc::new(BTreeSet::new()),
                },
            ));
        }
        while let Some((path, state)) = self.queue.pop() {
            // SAFETY: JSNative frame.
            unsafe { self.add_subpatterns(&path, state) };
            let wave = ::std::mem::take(&mut self.wave);
            for (p, sts) in wave {
                for st in sts {
                    self.queue.push((p.clone(), st));
                }
            }
        }
    }
    /// SAFETY: cx is the calling JSNative's context (exclude_fn calls).
    unsafe fn add_subpatterns(&mut self, path: &str, state: GlobWalkState) {
        let parts = state.parts.clone();
        let seen = self.cache_add(path, &parts, &state.indexes);
        if seen {
            return;
        }
        let fullpath = glob_resolve_root(&self.root, path);
        let stat = self.stat(&fullpath);
        let last = parts.len().saturating_sub(1);
        let is_directory = self.is_directory_state(&fullpath, stat, &state);
        let is_last = Self::is_last(&state, is_directory);
        let is_first = state.indexes.contains(&0usize);

        if self.is_excluded(&fullpath) {
            return;
        }
        if is_first && parts[0].src().is_empty() {
            // SAFETY: JSNative frame.
            unsafe { self.add_subpattern("/", Self::child(&state, BTreeSet::from([1usize]), None, None)) };
            return;
        }
        if is_first && parts[0].src() == ".." {
            // SAFETY: JSNative frame.
            unsafe { self.add_subpattern("../", Self::child(&state, BTreeSet::from([1usize]), None, None)) };
            return;
        }
        if is_first && parts[0].src() == "." {
            // SAFETY: JSNative frame.
            unsafe { self.add_subpattern(".", Self::child(&state, BTreeSet::from([1usize]), None, None)) };
            return;
        }

        if is_last && parts[last].is_literal() {
            let p = parts[last].literal_str();
            let st = self.stat(&glob_join_posix(&fullpath, p));
            if st.is_some() && (!p.is_empty() || is_directory) {
                self.add_result(&glob_join_posix(path, p));
            }
            if state.indexes.len() == 1 && state.indexes.contains(&last) {
                return;
            }
        } else if is_last
            && parts[last].is_globstar()
            && (path != "." || parts[0].src() == "." || (last == 0 && stat.is_some()))
        {
            self.add_result(path);
        }

        if !is_directory || self.is_cyclic(&fullpath, is_directory, &state) {
            return;
        }

        let next_realpaths = self.next_realpaths(&fullpath, is_directory, &state);

        let children: Vec<(String, GlobEntryKind)> = if state.indexes.len() == 1 {
            let sole = *state.indexes.iter().next().unwrap();
            match parts.get(sole) {
                Some(p) if p.is_literal() => {
                    let st = self.stat(&glob_join_posix(&fullpath, p.literal_str()));
                    match st {
                        Some(kind) => vec![(p.literal_str().to_string(), kind)],
                        None => return,
                    }
                }
                _ => self.readdir(&fullpath),
            }
        } else {
            self.readdir(&fullpath)
        };

        for (entry_name, entry_kind) in children {
            let entry_path = glob_join_posix(path, &entry_name);
            let entry_fullpath = glob_join_posix(&fullpath, &entry_name);
            if !self.stat_cache.contains_key(&entry_fullpath) {
                self.stat_cache.insert(entry_fullpath.clone(), Some(entry_kind));
            }
            let entry_is_directory = entry_kind == GlobEntryKind::Dir
                || (self.follow_symlinks
                    && entry_kind == GlobEntryKind::Symlink
                    && self.follow_stat(&entry_fullpath) == Some(GlobEntryKind::Dir));

            let mut sub_patterns: BTreeSet<usize> = BTreeSet::new();
            let mut n_symlinks: BTreeSet<usize> = BTreeSet::new();
            // CORPUS deviation 3: no child-level seen abort (v24.19.0 fails its
            // own gh-62897 corpus test on it) — see the engine header comment.
            for index in &state.indexes {
                if *index >= parts.len() {
                    continue; // v24 `at()` returns undefined and no-ops
                }
                let current = &parts[*index];
                let next_index = index + 1;
                let next = parts.get(next_index);
                let from_symlink = !self.follow_symlinks && state.symlinks.contains(index);

                if current.is_globstar() {
                    let is_dot = entry_name.starts_with('.');
                    let next_matches = match next {
                        Some(np) => np.test(&entry_name),
                        None => false,
                    };
                    let mut next_non_glob = next_index;
                    while next_non_glob < parts.len() && parts[next_non_glob].is_globstar() {
                        next_non_glob += 1;
                    }
                    let matches_dot = is_dot
                        && next_non_glob < parts.len()
                        && parts[next_non_glob].test(&entry_name);

                    // v24 line 599: the raw exclude fn receives the readdir
                    // entry (Dirent) in withFileTypes mode, the entry NAME
                    // otherwise.
                    let fn_excludes = if self.exclude_fn_key.is_some() {
                        if self.with_file_types {
                            let type_code = glob_entry_type_code(entry_kind);
                            let parent = glob_dirname(&entry_fullpath).to_string();
                            // SAFETY: JSNative frame; rooted inside.
                            unsafe { self.exclude_fn_check(Some((&entry_name, type_code, &parent))) }
                        } else {
                            // SAFETY: JSNative frame; rooted inside.
                            unsafe { self.exclude_fn_check_path(&entry_name) }
                        }
                    } else {
                        false
                    };

                    if (is_dot && !matches_dot) || fn_excludes {
                        continue;
                    }
                    if !from_symlink && entry_is_directory {
                        sub_patterns.insert(*index);
                    } else if !from_symlink && *index == last {
                        self.add_result(&entry_path);
                    }

                    if next_matches && next_index == last && !is_last {
                        self.add_result(&entry_path);
                    } else if next_matches && entry_is_directory {
                        sub_patterns.insert(index + 2);
                    }
                    // CORPUS deviation 1: v24 also accepts symlink entries here
                    // (`|| entry.isSymbolicLink()`) and dot-rooted patterns
                    // (`|| at(0) === '.'`); corpus is final truth — directory
                    // entries only.
                    if next_matches && entry_is_directory && !from_symlink {
                        sub_patterns.insert(next_index);
                    }

                    if !self.follow_symlinks && entry_kind == GlobEntryKind::Symlink {
                        n_symlinks.insert(*index);
                    }

                    if next.is_some_and(|np| np.src() == "..") && entry_is_directory {
                        let parent = glob_join_posix(path, "..");
                        if next_index < last {
                            if !self.wave.contains_key(path) && !self.seen_p(path, &parts, next_index + 1) {
                                self.wave.insert(
                                    path.to_string(),
                                    vec![Self::child(&state, BTreeSet::from([next_index + 1]), None, None)],
                                );
                            }
                            if !self.wave.contains_key(&parent) && !self.seen_p(&parent, &parts, next_index + 1) {
                                self.wave.insert(
                                    parent.clone(),
                                    vec![Self::child(&state, BTreeSet::from([next_index + 1]), None, None)],
                                );
                            }
                        } else {
                            if !self.seen_p(path, &parts, next_index) {
                                self.cache_add(path, &parts, &BTreeSet::from([next_index]));
                                self.add_result(path);
                            }
                            if !self.seen_p(path, &parts, next_index) || !self.seen_p(&parent, &parts, next_index) {
                                self.cache_add(&parent, &parts, &BTreeSet::from([next_index]));
                                self.add_result(&parent);
                            }
                        }
                    }
                }
                if current.is_literal() {
                    if current.test(&entry_name) && *index != last {
                        sub_patterns.insert(next_index);
                    } else if current.src() == "." && next_index < parts.len() && parts[next_index].test(&entry_name) {
                        if next_index == last {
                            self.add_result(&entry_path);
                        } else {
                            sub_patterns.insert(next_index + 1);
                        }
                    }
                }
                if !current.is_literal() && !current.is_globstar() && current.test(&entry_name) {
                    if *index == last {
                        self.add_result(&entry_path);
                    } else if entry_is_directory {
                        sub_patterns.insert(next_index);
                    }
                }
            }
            if !sub_patterns.is_empty() {
                // SAFETY: JSNative frame.
                unsafe {
                    self.add_subpattern(
                        &entry_path,
                        Self::child(&state, sub_patterns, Some(n_symlinks), Some(next_realpaths.clone())),
                    )
                };
            }
        }
    }
    fn child(
        state: &GlobWalkState,
        indexes: BTreeSet<usize>,
        symlinks: ::std::option::Option<BTreeSet<usize>>,
        realpaths: ::std::option::Option<::std::rc::Rc<BTreeSet<String>>>,
    ) -> GlobWalkState {
        GlobWalkState {
            parts: state.parts.clone(),
            indexes,
            symlinks: symlinks.unwrap_or_else(|| state.symlinks.clone()),
            realpaths: realpaths.unwrap_or_else(|| state.realpaths.clone()),
        }
    }
}

fn glob_entry_type_code(kind: GlobEntryKind) -> i32 {
    match kind {
        GlobEntryKind::Dir => GLOB_UV_DIR,
        GlobEntryKind::Symlink => GLOB_UV_LINK,
        GlobEntryKind::File => GLOB_UV_FILE,
    }
}

/// Construct an `internal/fs/utils` Dirent instance (the canonical class the
/// `instanceof Dirent` faces check) via proto + call — `new Dirent(name, type,
/// parentPath)` semantics without JS::Construct (the utils Dirent is a plain
/// function assigning onto `this`).
/// SAFETY: cx is the calling JSNative's context; utils module already installed.
unsafe fn construct_glob_dirent(
    cx: *mut JSContext,
    name: &str,
    type_code: i32,
    parent_path: &str,
) -> ::std::result::Result<*mut JSObject, ()> {
    if !crate::node_internal_fs_modules::ensure_installed(cx) {
        return ::std::result::Result::Err(());
    }
    let utils_mod = match crate::require::get_builtin(cx, "internal/fs/utils") {
        Some(m) => m,
        None => return ::std::result::Result::Err(()),
    };
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let utils_root = utils_mod);
    let mut dirent_val = UndefinedValue();
    JS_GetProperty(
        cx,
        utils_root.handle().into(),
        c"Dirent".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut dirent_val,
        },
    );
    if !dirent_val.is_object() {
        return ::std::result::Result::Err(());
    }
    rooted!(&in(wrapped_cx) let dirent_ctor = dirent_val.to_object());
    // Dirent.prototype
    let mut proto_val = UndefinedValue();
    JS_GetProperty(
        cx,
        dirent_ctor.handle().into(),
        c"prototype".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut proto_val,
        },
    );
    let proto = if proto_val.is_object() { proto_val.to_object() } else { JS_NewPlainObject(cx) };
    if proto.is_null() {
        return ::std::result::Result::Err(());
    }
    rooted!(&in(wrapped_cx) let proto_root = proto);
    // w2 three-arg form (null class = plain object with the given proto) —
    // node_tls.rs precedent; the guard keeps the object rooted while the
    // strings are allocated below.
    let obj = w2::JS_NewObjectWithGivenProto(&mut wrapped_cx, ::std::ptr::null(), proto_root.handle().into());
    if obj.is_null() {
        return ::std::result::Result::Err(());
    }
    rooted!(&in(wrapped_cx) let obj_root = obj);
    let name_c = ZBox::from_bytes(name.as_bytes());
    let name_js = JS_NewStringCopyZ(cx, name_c.as_ptr());
    if name_js.is_null() {
        return ::std::result::Result::Err(());
    }
    // Root the first string BEFORE the second allocation: JS_NewStringCopyZ
    // can run a nursery GC that moves `name_js`, and an unrooted bare pointer
    // across that point would seed a from-space Value below (unrooted-alloc
    // class, same family as the gc_store.rs rooting contract).
    rooted!(&in(wrapped_cx) let name_val = mozjs::jsval::StringValue(&*name_js));
    let parent_c = ZBox::from_bytes(parent_path.as_bytes());
    let parent_js = JS_NewStringCopyZ(cx, parent_c.as_ptr());
    if parent_js.is_null() {
        return ::std::result::Result::Err(());
    }
    rooted!(&in(wrapped_cx) let parent_val = mozjs::jsval::StringValue(&*parent_js));
    rooted!(&in(wrapped_cx) let type_val = mozjs::jsval::Int32Value(type_code));
    rooted!(&in(wrapped_cx) let args_rooted = mozjs_sys::jsgc::ValueArray::<3>::new([
        name_val.get(),
        type_val.get(),
        parent_val.get(),
    ]));
    let call_args = JS::HandleValueArray::from(&args_rooted);
    rooted!(&in(wrapped_cx) let ctor_val = mozjs::jsval::ObjectValue(dirent_ctor.get()));
    let mut rval = UndefinedValue();
    let ok = JS_CallFunctionValue(
        cx,
        obj_root.handle().into(),
        ctor_val.handle().into(),
        &call_args,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        },
    );
    if !ok {
        JS_ClearPendingException(cx);
        return ::std::result::Result::Err(());
    }
    ::std::result::Result::Ok(obj_root.get())
}

/// Collect glob matches for one pattern via the node-v24 walk engine, applying
/// JS/glob excludes; out receives (path, UV type code) pairs — type codes are
/// meaningful only when opts.with_file_types is set (lstat of each result,
/// node globSync's DirentFromStats face).
/// SAFETY: cx is the calling JSNative's context (exclude_fn callbacks run
/// synchronously within this frame).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn glob_collect(
    cx: *mut JSContext,
    pattern: &str,
    opts: &GlobOptions,
    results: &mut Vec<(String, i32)>,
) {
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
    // GC rooting (raw-jsval-across-GC-points class, gc_store.rs contract): the
    // JS exclude predicate fires repeatedly across the whole walk and every
    // callback run can trigger a nursery/moving GC that relocates the function
    // object. A bare *mut JSObject captured once is stale from the second
    // invocation on, and the per-call `rooted!` then roots an already dangling
    // address — observed as the test-fs-glob SIGSEGV (#GP on a non-canonical
    // stale callee, bun_serve precedent). Hold the gc_store KEY instead: the
    // object lives as a GC-managed property on the JS global, each check
    // re-resolves a fresh live pointer, and the key is removed when the walk
    // ends. Capture→insert is a GC-free window (pure Rust above), so the
    // inserted address is the live one.
    let exclude_key = opts.exclude_fn.map(|cb| {
        let key = crate::gc_store::gc_store_unique_key("fs_glob_exclude_fn");
        // Safe fn; the caller frame is a JSNative (see header), so the global
        // resolution inside lands on the calling realm.
        crate::gc_store::gc_store_insert(cx, &key, cb);
        key
    });
    let mut engine = GlobEngine {
        root: opts.start_dir.clone(),
        exclude_fn_key: exclude_key.clone(),
        cx,
        _marker: ::std::marker::PhantomData,
        with_file_types: opts.with_file_types,
        follow_symlinks: opts.follow_symlinks,
        arms: glob_compile_pattern(pattern),
        exclude_matchers: opts
            .exclude_globs
            .iter()
            .flat_map(|p| glob_compile_pattern(&glob_resolve_root(&opts.start_dir, p)))
            .map(|rc| (*rc).clone())
            .collect(),
        stat_cache: HashMap::new(),
        follow_cache: HashMap::new(),
        readdir_cache: HashMap::new(),
        realpath_cache: HashMap::new(),
        seen: ::std::collections::HashSet::new(),
        results: BTreeSet::new(),
        queue: Vec::new(),
        wave: HashMap::new(),
    };
    // SAFETY: caller frame is a JSNative (see header).
    unsafe { engine.run() };
    if let Some(key) = &exclude_key {
        crate::gc_store::gc_store_remove(cx, key);
    }
    let with_types = opts.with_file_types;
    let found: Vec<String> = engine.results.iter().cloned().collect();
    for path in &found {
        let type_code = if with_types {
            let full = if path.starts_with('/') {
                path.clone()
            } else {
                glob_join_posix(&opts.start_dir, path)
            };
            engine
                .lstat_kind(full.as_str())
                .map(glob_entry_type_code)
                .unwrap_or(0) // UV_DIRENT_UNKNOWN on stat miss (race edge)
        } else {
            0
        };
        results.push((path.clone(), type_code));
    }
}

// ── internal/fs/glob native binding (node-main shape: the JS Glob class is a
// thin wrapper over a native engine; the binding object is installed as a
// temporary global for the FS_GLOB_JS eval, then deleted — the
// install_fs_stream_polyfill precedent)
// @trace REQ-ENG-004 [api:internal/fs/glob] — the require('internal/fs/glob')
// golden face (test-fs-glob seen-cache child) backs onto this binding.

/// Flag bits shared with FS_GLOB_JS (node main src/glob/node_glob.cc names).
pub const GLOB_FLAG_FOLLOW_SYMLINKS: i32 = 1;
pub const GLOB_FLAG_WITH_FILE_TYPES: i32 = 2;

/// The native globSync binding: (root, patterns[], excludePatterns[], flags,
/// excludeFn|undefined) → [paths[], types[]]. Patterns are walk-compiled per
/// call; exclude patterns are already absolute-resolved by the JS constructor
/// (re-resolved idempotently inside the engine).
/// SAFETY: cx is a JSNative frame.
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe extern "C" fn glob_native_glob_sync(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let argv = |i: u32| -> JSVal {
        if i < argc {
            unsafe { *args.get(i).ptr }
        } else {
            UndefinedValue()
        }
    };
    let root_val = argv(0);
    let patterns_val = argv(1);
    let exclude_val = argv(2);
    let flags_js = argv(3);
    let exclude_fn_val = argv(4);
    if !root_val.is_string() {
        JS_ReportErrorUTF8(cx, c"internal/fs/glob: root must be a string".as_ptr());
        return false;
    }
    let root = crate::jsstr_to_rust_string(cx, root_val.to_string());
    if !cli_perm_glob_root_gate(cx, &root) {
        return false;
    }
    let mut patterns: Vec<String> = Vec::new();
    let mut exclude_patterns: Vec<String> = Vec::new();
    if patterns_val.is_object() {
        let arr = patterns_val.to_object();
        let wrapped_cx =
            mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        rooted!(&in(wrapped_cx) let arr_root = arr);
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
        if len_val.is_int32() {
            for i in 0..len_val.to_int32().max(0) as u32 {
                let mut elem = UndefinedValue();
                if JS_GetElement(
                    cx,
                    arr_root.handle().into(),
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
    if exclude_val.is_object() {
        let arr = exclude_val.to_object();
        let wrapped_cx =
            mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        rooted!(&in(wrapped_cx) let arr_root = arr);
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
        if len_val.is_int32() {
            for i in 0..len_val.to_int32().max(0) as u32 {
                let mut elem = UndefinedValue();
                if JS_GetElement(
                    cx,
                    arr_root.handle().into(),
                    i,
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut elem,
                    },
                ) && elem.is_string()
                {
                    exclude_patterns.push(crate::jsstr_to_rust_string(cx, elem.to_string()));
                }
            }
        }
    }
    let flags = if flags_js.is_int32() { flags_js.to_int32() } else { 0 };
    let exclude_fn: ::std::option::Option<*mut JSObject> = if exclude_fn_val.is_object()
        && mozjs_sys::jsapi::JS::IsCallable(exclude_fn_val.to_object())
    {
        Some(exclude_fn_val.to_object())
    } else {
        None
    };
    let with_file_types = flags & GLOB_FLAG_WITH_FILE_TYPES != 0;
    let follow_symlinks = flags & GLOB_FLAG_FOLLOW_SYMLINKS != 0;

    let mut merged: BTreeSet<(String, i32)> = BTreeSet::new();
    for pattern in &patterns {
        let opts = GlobOptions {
            start_dir: root.clone(),
            dot: false,
            follow_symlinks,
            with_file_types,
            exclude_fn,
            // engine re-resolves idempotently; keep raw here
            exclude_globs: exclude_patterns.clone(),
        };
        let mut out: Vec<(String, i32)> = Vec::new();
        // SAFETY: JSNative frame.
        unsafe { glob_collect(cx, pattern, &opts, &mut out) };
        merged.extend(out);
    }
    let results: Vec<(String, i32)> = merged.into_iter().collect();

    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let paths_arr = w2::NewArrayObject1(cx_ref, results.len()));
    rooted!(&in(cx_ref) let types_arr = w2::NewArrayObject1(cx_ref, results.len()));
    if paths_arr.get().is_null() || types_arr.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    for (idx, (path, type_code)) in results.iter().enumerate() {
        let c_path = ZBox::from_bytes(path.as_bytes());
        let js_str = JS_NewStringCopyZ(cx, c_path.as_ptr());
        if !js_str.is_null() {
            rooted!(&in(cx_ref) let val = mozjs::jsval::StringValue(&*js_str));
            JS_DefineElement(
                cx,
                paths_arr.handle().into(),
                idx as u32,
                val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
        rooted!(&in(cx_ref) let tval = mozjs::jsval::Int32Value(*type_code));
        JS_DefineElement(
            cx,
            types_arr.handle().into(),
            idx as u32,
            tval.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }
    rooted!(&in(cx_ref) let pair = w2::NewArrayObject1(cx_ref, 2));
    if pair.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    rooted!(&in(cx_ref) let pval = mozjs::jsval::ObjectValue(paths_arr.get()));
    rooted!(&in(cx_ref) let tval2 = mozjs::jsval::ObjectValue(types_arr.get()));
    JS_DefineElement(cx, pair.handle().into(), 0, pval.handle().into(), JSPROP_ENUMERATE as u32);
    JS_DefineElement(cx, pair.handle().into(), 1, tval2.handle().into(), JSPROP_ENUMERATE as u32);
    args.rval().set(mozjs::jsval::ObjectValue(pair.get()));
    true
}

/// The native matchGlobPattern binding: (path, pattern) → bool — full-path
/// minimatch semantics, dot:false fixed (fs.glob's matcher shape).
/// SAFETY: cx is a JSNative frame.
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe extern "C" fn glob_native_match_path(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 || !(*args.get(0).ptr).is_string() || !(*args.get(1).ptr).is_string() {
        JS_ReportErrorUTF8(cx, c"matchGlobPattern: path and pattern must be strings".as_ptr());
        return false;
    }
    let path = crate::jsstr_to_rust_string(cx, args.get(0).to_string());
    let pattern = crate::jsstr_to_rust_string(cx, args.get(1).to_string());
    let arms = glob_compile_pattern(&pattern);
    let hit = arms.iter().any(|parts| glob_match_full_path(parts, &path));
    args.rval().set(mozjs::jsval::BooleanValue(hit));
    true
}

/// The native hasMagic binding: (pattern) → bool.
/// SAFETY: cx is a JSNative frame.
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe extern "C" fn glob_native_has_magic(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 1 || !(*args.get(0).ptr).is_string() {
        args.rval().set(mozjs::jsval::BooleanValue(false));
        return true;
    }
    let pattern = crate::jsstr_to_rust_string(cx, args.get(0).to_string());
    let has = glob_brace_expand(&pattern)
        .iter()
        .any(|arm| glob_slash_split(arm).iter().any(|part| glob_has_magic(part)));
    args.rval().set(mozjs::jsval::BooleanValue(has));
    true
}

/// Build the `__bao_internal_glob` binding object (globSync + matchPath +
/// hasMagic + the kFlag* constants) for the FS_GLOB_JS global-capture bridge.
/// SAFETY: called from a JSNative frame (the require hook inside
/// node_internal_fs_modules::ensure_installed).
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn glob_native_binding_install(cx: *mut JSContext) -> bool {
    let global = CurrentGlobalOrNull(cx);
    if global.is_null() {
        return false;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    let mut existing = UndefinedValue();
    rooted!(&in(cx_ref) let global_root = global);
    JS_GetProperty(
        cx,
        global_root.handle().into(),
        c"__bao_internal_glob".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut existing,
        },
    );
    if existing.is_object() {
        return true; // already installed (idempotent)
    }
    rooted!(&in(cx_ref) let binding = w2::JS_NewPlainObject(cx_ref));
    if binding.get().is_null() {
        return false;
    }
    register_js_functions!(
        cx_ref,
        binding,
        JSPROP_ENUMERATE as u32;
        (c"globSync", glob_native_glob_sync, 5)
        (c"matchPath", glob_native_match_path, 2)
        (c"hasMagic", glob_native_has_magic, 1)
    );
    define_num_prop(cx, binding.get(), "kFlagWindows", 0.0);
    define_num_prop(cx, binding.get(), "kFlagDot", 0.0);
    define_num_prop(
        cx,
        binding.get(),
        "kFlagFollowSymlinks",
        GLOB_FLAG_FOLLOW_SYMLINKS as f64,
    );
    define_num_prop(
        cx,
        binding.get(),
        "kFlagWithFileTypes",
        GLOB_FLAG_WITH_FILE_TYPES as f64,
    );
    define_num_prop(cx, binding.get(), "kFlagMatchBase", 0.0);
    rooted!(&in(cx_ref) let binding_val = mozjs::jsval::ObjectValue(binding.get()));
    JS_DefineProperty(
        cx,
        global_root.handle().into(),
        c"__bao_internal_glob".as_ptr(),
        binding_val.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
    true
}

/// Read a path-valued property off a JS options object: string or file: URL
/// (node glob accepts URL cwd — pathToFileURL(fixtureDir) in test-fs-glob).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn glob_opt_path(cx: *mut JSContext, obj: *mut JSObject, name: &[u8]) -> Option<String> {
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
        let s = val.to_string();
        if !s.is_null() {
            return Some(crate::jsstr_to_rust_string(cx, s));
        }
        return None;
    }
    if val.is_object() {
        // URL-shaped values (href string) route through the shared fs URL
        // resolver (file: scheme gate + percent decode). Err(true) = not
        // URL-shaped (no option value); Err(false) = scheme error already
        // pending — cleared here since the options parser is infallible (the
        // corpus drives valid file: URLs only; a rejected scheme yields the
        // default cwd rather than a half-parsed run).
        match url_val_to_fs_path(cx, val) {
            ::std::result::Result::Ok(path) => return Some(path),
            ::std::result::Result::Err(_) => {
                JS_ClearPendingException(cx);
            }
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
        follow_symlinks: false,
        with_file_types: false,
        exclude_fn: None,
        exclude_globs: Vec::new(),
    };
    let val = *args.get(arg_index).ptr;
    if !val.is_object() {
        return opts;
    }
    let obj = val.to_object();
    // Node fs.glob: cwd takes precedence, root is the legacy fallback. Both
    // accept a file: URL (test-fs-glob: globSync - with file: URL as cwd).
    if let Some(cwd) = glob_opt_path(cx, obj, b"cwd") {
        opts.start_dir = cwd;
    } else if let Some(root) = glob_opt_path(cx, obj, b"root") {
        opts.start_dir = root;
    }
    opts.dot = glob_opt_bool(cx, obj, b"dot");
    // node v24 glob.js: `withFileTypes` — results become Dirent instances and
    // the exclude fn receives Dirents (test-fs-glob withFileTypes describe
    // blocks: `dirent instanceof Dirent` + parentPath/name arithmetic).
    opts.with_file_types = glob_opt_bool(cx, obj, b"withFileTypes");
    // node validates followSymlinks as boolean (test-fs-glob 'validates
    // followSymlinks': followSymlinks: 1 → ERR_INVALID_ARG_TYPE TypeError).
    // On a validation throw the exception stays pending; every glob caller
    // checks JS_IsExceptionPending right after this parse and propagates.
    {
        let wrapped_cx0 = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        rooted!(&in(wrapped_cx0) let obj_root0 = obj);
        let mut fsym = UndefinedValue();
        JS_GetProperty(
            cx,
            obj_root0.handle().into(),
            c"followSymlinks".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut fsym,
            },
        );
        if !fsym.is_undefined() && !fsym.is_null() && !fsym.is_boolean() {
            let msg = format!(
                "The \"followSymlinks\" option must be of type boolean. Received {}",
                received_fs_desc(cx, fsym)
            );
            throw_fs_coded_error(cx, "ERR_INVALID_ARG_TYPE", "TypeError", &msg);
            return opts;
        }
    }
    opts.follow_symlinks = glob_opt_bool(cx, obj, b"followSymlinks");

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
            // Root the exclude value before any further gets/elements.
            let wrapped_cx_excl =
                mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
            rooted!(&in(wrapped_cx_excl) let excl_root = excl_obj);
            // node v24 validateStringArrayOrFunction: an `exclude` that is
            // neither a string[] nor a function is a TypeError (corpus
            // invalid-exclude cells). Mirror that here; silently dropping a
            // malformed `exclude` hides real caller bugs.
            let mut is_arr = false;
            if !(IsArrayObject1(cx, excl_root.handle().into(), &mut is_arr) && is_arr) {
                throw_fs_coded_error(
                    cx,
                    "ERR_INVALID_ARG_TYPE",
                    "TypeError",
                    "The \"options.exclude\" property must be of type function or string[].",
                );
                return opts;
            }
            // Array of glob patterns.
            let mut len_val = UndefinedValue();
            JS_GetProperty(
                cx,
                excl_root.handle().into(),
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
                        excl_root.handle().into(),
                        i,
                        MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &mut elem,
                        },
                    ) {
                        if !elem.is_string() {
                            throw_fs_coded_error(
                                cx,
                                "ERR_INVALID_ARG_TYPE",
                                "TypeError",
                                "The \"options.exclude\" property must be of type function or string[].",
                            );
                            return opts;
                        }
                        // node v24 glob.js constructor: exclude patterns are
                        // RESOLVED against root before compiling — matching
                        // happens on absolute yielded paths (corpus patterns2
                        // mixes relative and absolute excludes).
                        let raw = crate::jsstr_to_rust_string(cx, elem.to_string());
                        opts.exclude_globs.push(glob_resolve_root(&opts.start_dir, &raw));
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
    if unsafe { JS_IsExceptionPending(cx) } {
        return false;
    }
    if !cli_perm_glob_root_gate(cx, &opts.start_dir) {
        return false;
    }
    let mut results: Vec<(String, i32)> = Vec::new();
    for pat in &patterns {
        // SAFETY: JSNative frame; exclude_fn callbacks run inside.
        unsafe { glob_collect(cx, pat, &opts, &mut results) };
    }
    let paths: Vec<String> = results.iter().map(|(p, _)| p.clone()).collect();
    let mut paths = paths;
    normalize_glob_result_separators(&mut paths);
    if opts.with_file_types {
        let types: Vec<i32> = results.iter().map(|(_, t)| *t).collect();
        let mut wrapped_cx =
            mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let arr = w2::NewArrayObject1(cx_ref, paths.len()));
        if arr.get().is_null() {
            args.rval().set(UndefinedValue());
            return true;
        }
        for (idx, path) in paths.iter().enumerate() {
            let full = if path.starts_with('/') {
                path.clone()
            } else {
                glob_join_posix(&opts.start_dir, path)
            };
            // node globSync withFileTypes: Dirent(basename, type, dirname)
            // from the canonical internal/fs/utils class.
            // SAFETY: JSNative frame; rooted inside.
            let dirent = unsafe {
                construct_glob_dirent(
                    cx,
                    glob_basename(&full),
                    types[idx],
                    glob_dirname(&full),
                )
            };
            match dirent {
                ::std::result::Result::Ok(d) => {
                    rooted!(&in(cx_ref) let val = mozjs::jsval::ObjectValue(d));
                    JS_DefineElement(
                        cx,
                        arr.handle().into(),
                        idx as u32,
                        val.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
                ::std::result::Result::Err(()) => {
                    args.rval().set(UndefinedValue());
                    return false;
                }
            }
        }
        args.rval().set(mozjs::jsval::ObjectValue(arr.get()));
        return true;
    }
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let arr = w2::NewArrayObject1(cx_ref, paths.len()));
    if arr.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    for (idx, path) in paths.iter().enumerate() {
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
    if unsafe { JS_IsExceptionPending(cx) } {
        return false;
    }
    if !cli_perm_glob_root_gate(cx, &opts.start_dir) {
        return false;
    }

    if let Some((callback, _)) = extract_callback_and_encoding(cx, &args, 2) {
        // The JS exclude predicate cannot cross the thread boundary — apply
        // it synchronously on the collected results instead of in the worker.
        let mut prefiltered: Vec<(String, i32)> = Vec::new();
        for pat in &patterns {
            // SAFETY: JSNative frame; exclude_fn callbacks run inside.
            unsafe { glob_collect(cx, pat, &opts, &mut prefiltered) };
        }
        let mut paths: Vec<String> = prefiltered.iter().map(|(p, _)| p.clone()).collect();
        normalize_glob_result_separators(&mut paths);
        if opts.with_file_types {
            let types: Vec<i32> = prefiltered.iter().map(|(_, t)| *t).collect();
            let root = opts.start_dir.clone();
            spawn_fs_async(cx, "glob", patterns.join(","), callback, None, move || {
                Ok(FsAsyncResult::OkGlobDirents {
                    root,
                    entries: paths.into_iter().zip(types).collect(),
                })
            });
        } else {
            spawn_fs_async(cx, "glob", patterns.join(","), callback, None, move || {
                Ok(FsAsyncResult::OkDirnames(paths))
            });
        }
        args.rval().set(UndefinedValue());
        return true;
    }

    // No callback — behave like sync
    let mut results: Vec<(String, i32)> = Vec::new();
    for pat in &patterns {
        // SAFETY: JSNative frame; exclude_fn callbacks run inside.
        unsafe { glob_collect(cx, pat, &opts, &mut results) };
    }
    let paths: Vec<String> = results.iter().map(|(p, _)| p.clone()).collect();
    let mut paths = paths;
    normalize_glob_result_separators(&mut paths);
    if opts.with_file_types {
        let types: Vec<i32> = results.iter().map(|(_, t)| *t).collect();
        let mut wrapped_cx =
            mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let arr = w2::NewArrayObject1(cx_ref, paths.len()));
        if arr.get().is_null() {
            args.rval().set(UndefinedValue());
            return true;
        }
        for (idx, path) in paths.iter().enumerate() {
            let full = if path.starts_with('/') {
                path.clone()
            } else {
                glob_join_posix(&opts.start_dir, path)
            };
            // SAFETY: JSNative frame; rooted inside.
            let dirent = unsafe {
                construct_glob_dirent(cx, glob_basename(&full), types[idx], glob_dirname(&full))
            };
            match dirent {
                ::std::result::Result::Ok(d) => {
                    rooted!(&in(cx_ref) let val = mozjs::jsval::ObjectValue(d));
                    JS_DefineElement(
                        cx,
                        arr.handle().into(),
                        idx as u32,
                        val.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
                ::std::result::Result::Err(()) => {
                    args.rval().set(UndefinedValue());
                    return false;
                }
            }
        }
        args.rval().set(mozjs::jsval::ObjectValue(arr.get()));
        return true;
    }
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let arr = w2::NewArrayObject1(cx_ref, paths.len()));
    if arr.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    for (idx, path) in paths.iter().enumerate() {
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

/// fs.promises.glob(pattern[, options]) → the async-iterable match stream
/// ITSELF.
///
/// node v24.19.0 oracle: the return value is NOT a Promise (`then: undefined`)
/// — it carries `[Symbol.asyncIterator]` directly and `for await (const entry
/// of glob(...))` iterates the RETURN value (test-fs-glob /
/// test-fs-glob-no-follow-symlink-globstar both consume that shape; the
/// former Promise-wrapping shape made for-await fail with "(intermediate
/// value)(...) is not iterable" — a Promise has no asyncIterator).
/// Implementation: collect synchronously through the shared fs_glob/
/// glob_collect engine (the JS exclude predicate cannot cross the worker
/// boundary, and node resolves glob matches eagerly at call time anyway —
/// the async surface is delivery-only), then return a plain object with
/// next/return/[Symbol.asyncIterator] over the collected array.
///
// @trace REQ-ENG-007 [api:fs.promises.glob] — GOLDEN A r2: fs.promises.glob
// export missing (test-fs-glob-no-follow-symlink-globstar import face).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fs_promises_glob(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let patterns = glob_patterns(cx, &args);
    let opts = glob_parse_options(cx, &args, 1);
    if unsafe { JS_IsExceptionPending(cx) } {
        return false;
    }
    if !cli_perm_glob_root_gate(cx, &opts.start_dir) {
        return false;
    }
    let mut results: Vec<(String, i32)> = Vec::new();
    for pat in &patterns {
        // SAFETY: JSNative frame; exclude_fn callbacks run inside.
        unsafe { glob_collect(cx, pat, &opts, &mut results) };
    }
    let mut paths: Vec<String> = results.iter().map(|(p, _)| p.clone()).collect();
    normalize_glob_result_separators(&mut paths);

    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let arr = w2::NewArrayObject1(cx_ref, paths.len()));
    if arr.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    if opts.with_file_types {
        let types: Vec<i32> = results.iter().map(|(_, t)| *t).collect();
        for (idx, path) in paths.iter().enumerate() {
            let full = if path.starts_with('/') {
                path.clone()
            } else {
                glob_join_posix(&opts.start_dir, path)
            };
            // SAFETY: JSNative frame; rooted inside.
            let dirent = unsafe {
                construct_glob_dirent(cx, glob_basename(&full), types[idx], glob_dirname(&full))
            };
            match dirent {
                ::std::result::Result::Ok(d) => {
                    rooted!(&in(cx_ref) let val = mozjs::jsval::ObjectValue(d));
                    JS_DefineElement(
                        cx,
                        arr.handle().into(),
                        idx as u32,
                        val.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
                ::std::result::Result::Err(()) => {
                    args.rval().set(UndefinedValue());
                    return false;
                }
            }
        }
    } else {
        for (idx, path) in paths.iter().enumerate() {
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
    }

    // The async-iterable match stream: `{ next() {…}, return() {…},
    // [Symbol.asyncIterator]() { return this; } }` over the collected array
    // (index cursor; asyncIterator returns the object itself — node oracle:
    // `glob(p)[Symbol.asyncIterator]() === glob(p)`). Built as a JS function
    // so the iteration protocol stays in realm code.
    let iter_src = r#"(function(arr) {
  var idx = 0;
  return {
    next: function() {
      if (idx >= arr.length) return Promise.resolve({ value: undefined, done: true });
      var value = arr[idx++];
      return Promise.resolve({ value: value, done: false });
    },
    return: function() { idx = arr.length; return Promise.resolve({ value: undefined, done: true }); },
    [Symbol.asyncIterator]: function() { return this; },
  };
})"#;
    let mut src_text = mozjs::rust::transform_str_to_source_text(iter_src);
    let mut factory_val = UndefinedValue();
    let factory_h = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut factory_val,
    };
    let compile_opts = mozjs::glue::NewCompileOptions(cx, c"<fs.promises.glob>".as_ptr(), 1);
    if compile_opts.is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    let ok = unsafe { mozjs_sys::jsapi::JS::Evaluate2(cx, compile_opts, &mut src_text, factory_h) };
    mozjs::glue::DeleteCompileOptions(compile_opts);
    if !ok || !factory_val.is_object() {
        if !ok {
            JS_ClearPendingException(cx);
        }
        args.rval().set(UndefinedValue());
        return false;
    }

    let global = CurrentGlobalOrNull(cx);
    if global.is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    rooted!(&in(cx_ref) let global_root = global);
    rooted!(&in(cx_ref) let factory_root = factory_val);
    rooted!(&in(cx_ref) let arr_val = mozjs::jsval::ObjectValue(arr.get()));
// unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
    rooted!(&in(cx_ref) let call_args_rooted = mozjs_sys::jsgc::ValueArray::<1>::new([
        arr_val.get(),
    ]));
    let call_args = JS::HandleValueArray::from(&call_args_rooted);
    let mut iter_obj_val = UndefinedValue();
    let call_ok = unsafe {
        JS_CallFunctionValue(
            cx,
            global_root.handle().into(),
            factory_root.handle().into(),
            &call_args,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut iter_obj_val,
            },
        )
    };
    if !call_ok || !iter_obj_val.is_object() {
        if !call_ok {
            JS_ClearPendingException(cx);
        }
        args.rval().set(UndefinedValue());
        return false;
    }
    rooted!(&in(cx_ref) let iter_rooted = iter_obj_val);
    args.rval().set(iter_rooted.get());
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
/// @trace REQ-ENG-007 [api:fs.statSync|fs.lstatSync|fs.fstatSync|fs.promises.stat|fs.promises.lstat|FileHandle.stat]
/// — the {bigint:true} BigIntStats face: every numeric field is a BigInt
/// (SM FFI constructors), Date fields stay Date; `false` keeps the Number
/// face byte-identical to the pre-bigint behavior.
unsafe fn create_stats_object(
    cx: *mut JSContext,
    meta: &bun_fs::PosixStat,
    bigint: bool,
) -> *mut JSObject {
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

    if bigint {
        // node BigIntStats face ({bigint:true}): every numeric field is a
        // BigInt (precision past 2^53); Date fields below stay Date. ms
        // fields come straight from the timespec — i128 ns total, then /1e6
        // with Rust's trunc-toward-zero division (identical to BigInt
        // division; the f64 ms path above would lose the precision the
        // bigint mode exists for). test-fs-stat-date: statsBigint.atimeMs -
        // BigInt(atime) must be a BigInt difference, not Number - BigInt.
        for (name, raw) in [
            ("size", meta.size as i128),
            ("dev", meta.dev as i128),
            ("ino", meta.ino as i128),
            ("mode", meta.mode as i128),
            ("nlink", meta.nlink as i128),
            ("uid", meta.uid as i128),
            ("gid", meta.gid as i128),
            ("rdev", meta.rdev as i128),
            ("blksize", meta.blksize as i128),
            ("blocks", meta.blocks as i128),
        ] {
            define_bigint_i128(cx, stats.get(), name, raw);
        }
        let atime_ns = meta.atim.sec as i128 * 1_000_000_000 + meta.atim.nsec as i128;
        let mtime_ns = meta.mtim.sec as i128 * 1_000_000_000 + meta.mtim.nsec as i128;
        let ctime_ns = meta.ctim.sec as i128 * 1_000_000_000 + meta.ctim.nsec as i128;
        let birthtime_ns = if meta.birthtim.sec == 0 && meta.birthtim.nsec == 0 {
            ctime_ns
        } else {
            meta.birthtim.sec as i128 * 1_000_000_000 + meta.birthtim.nsec as i128
        };
        for (name, ns) in [
            ("atimeMs", atime_ns),
            ("mtimeMs", mtime_ns),
            ("ctimeMs", ctime_ns),
            ("birthtimeMs", birthtime_ns),
        ] {
            define_bigint_i128(cx, stats.get(), name, ns / 1_000_000);
        }
    } else {
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
    }

    // Date objects for atime, mtime, ctime, birthtime
    let date_props: [(&str, f64); 4] = [
        ("atime", atime_ms),
        ("mtime", mtime_ms),
        ("ctime", ctime_ms),
        ("birthtime", birthtime_ms),
    ];
    for (prop, ms) in &date_props {
        // node Date::New semantics (ECMAScript TimeClip): a Date's time value
        // is INTEGER ms — trunc toward zero, |t| > 8.64e15 → NaN (invalid
        // date). The former raw-ms ClippedTime handed SM an unclipped
        // fractional value, so stats.mtime.getTime() returned fractions
        // (node: integers) and sub-ms drift (e.g. the cp preserveTimestamps
        // chain) became observable through getTime().
        let clipped_ms = if !ms.is_finite() || ms.abs() > 8.64e15 {
            f64::NAN
        } else {
            ms.trunc()
        };
        let date_obj = w2::NewDateObject(cx_ref, mozjs::jsapi::ClippedTime { t: clipped_ms });
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

/// Define a BigInt-valued property — the node BigIntStats face (bigint:true
/// option): every numeric field is a BigInt because the whole point is
/// precision beyond 2^53. Built via the SM FFI constructors
/// (JS::detail::BigIntFrom{Int,Uint}64 — real out-of-line T symbols in
/// libjs_static.a, bindgen'd with correct link names), never via f64.
/// Values outside [i64::MIN, u64::MAX] fail-closed with a false return (the
/// stat faces cannot reach that range; a silent wrap would be data rot).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn define_bigint_i128(
    cx: *mut JSContext,
    obj_ptr: *mut JSObject,
    name: &str,
    val: i128,
) -> bool {
    if val < i64::MIN as i128 || val > u64::MAX as i128 {
        return false;
    }
    let bi = if val >= 0 && val > i64::MAX as i128 {
        mozjs_sys::jsapi::JS::detail::BigIntFromUint64(cx, val as u64)
    } else {
        mozjs_sys::jsapi::JS::detail::BigIntFromInt64(cx, val as i64)
    };
    if bi.is_null() {
        return false;
    }
    let c_name = ZBox::from_bytes(name.as_bytes());
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj = obj_ptr);
    rooted!(&in(cx_ref) let v = mozjs::jsval::BigIntValue(&*bi));
    JS_DefineProperty(
        cx,
        obj.handle().into(),
        c_name.as_ptr(),
        v.handle().into(),
        JSPROP_ENUMERATE as u32,
    )
}

/// node stat-face `bigint` option read from the options argument slot
/// (boolean-only — node validateBoolean-accepts exactly true/false; absent →
/// Number face). The options value is rooted across the property read (the
/// read can run getters → GC).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn stat_bigint_option(cx: *mut JSContext, args: &CallArgs, slot: u32) -> bool {
    if args.argc_ <= slot {
        return false;
    }
    let v = *args.get(slot).ptr;
    if !v.is_object() {
        return false;
    }
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let opts_root = v.to_object());
    rooted!(&in(cx_ref) let mut big_v = UndefinedValue());
    read_object_prop_into(
        cx,
        opts_root.handle().into(),
        c"bigint".as_ptr(),
        big_v.handle_mut().into(),
    );
    big_v.get().is_boolean() && big_v.get().to_boolean()
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

/// Build a Stats object from a `libc::stat` (used by FileHandle.stat and the
/// StatWatcher pair; the latter always passes bigint=false — node's
/// fs.watchFile carries no bigint option).
unsafe fn build_stats_object(cx: *mut JSContext, st: &libc::stat, bigint: bool) -> *mut JSObject {
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
    let atime_nsec_total = st.st_atime as i128 * 1_000_000_000 + atime_nsec as i128;
    let mtime_nsec_total = st.st_mtime as i128 * 1_000_000_000 + mtime_nsec as i128;
    let ctime_nsec_total = st.st_ctime as i128 * 1_000_000_000 + ctime_nsec as i128;

    // windows CRT stat has no blksize/blocks — node reports 4096/0 there.
    #[cfg(windows)]
    let (blksize, blocks): (i128, i128) = (4096, 0);
    #[cfg(not(windows))]
    let (blksize, blocks) = (st.st_blksize as i128, st.st_blocks as i128);
    if bigint {
        // node BigIntStats face — see create_stats_object for the timespec →
        // i128 ns → trunc-toward-zero /1e6 rationale (BigInt division
        // semantics, never through f64).
        for (name, raw) in [
            ("size", st.st_size as i128),
            ("dev", st.st_dev as i128),
            ("ino", st.st_ino as i128),
            ("mode", st.st_mode as i128),
            ("nlink", st.st_nlink as i128),
            ("uid", st.st_uid as i128),
            ("gid", st.st_gid as i128),
            ("rdev", st.st_rdev as i128),
            ("blksize", blksize),
            ("blocks", blocks),
        ] {
            define_bigint_i128(cx, stats.get(), name, raw);
        }
        for (name, ns) in [
            ("atimeMs", atime_nsec_total),
            ("mtimeMs", mtime_nsec_total),
            ("ctimeMs", ctime_nsec_total),
            ("birthtimeMs", ctime_nsec_total),
        ] {
            define_bigint_i128(cx, stats.get(), name, ns / 1_000_000);
        }
    } else {
        define_num_prop(cx, stats.get(), "size", st.st_size as f64);
        define_num_prop(cx, stats.get(), "dev", st.st_dev as f64);
        define_num_prop(cx, stats.get(), "ino", st.st_ino as f64);
        define_num_prop(cx, stats.get(), "mode", st.st_mode as f64);
        define_num_prop(cx, stats.get(), "nlink", st.st_nlink as f64);
        define_num_prop(cx, stats.get(), "uid", st.st_uid as f64);
        define_num_prop(cx, stats.get(), "gid", st.st_gid as f64);
        define_num_prop(cx, stats.get(), "rdev", st.st_rdev as f64);
        define_num_prop(cx, stats.get(), "blksize", blksize as f64);
        define_num_prop(cx, stats.get(), "blocks", blocks as f64);
        define_num_prop(cx, stats.get(), "atimeMs", atime_ms);
        define_num_prop(cx, stats.get(), "mtimeMs", mtime_ms);
        define_num_prop(cx, stats.get(), "ctimeMs", ctime_ms);
        define_num_prop(cx, stats.get(), "birthtimeMs", birthtime_ms);
    }

    // Date objects for atime, mtime, ctime, birthtime
    let date_props: [(&str, f64); 4] = [
        ("atime", atime_ms),
        ("mtime", mtime_ms),
        ("ctime", ctime_ms),
        ("birthtime", birthtime_ms),
    ];
    for (prop, ms) in &date_props {
        // node Date::New semantics (ECMAScript TimeClip): a Date's time value
        // is INTEGER ms — trunc toward zero, |t| > 8.64e15 → NaN (invalid
        // date). The former raw-ms ClippedTime handed SM an unclipped
        // fractional value, so stats.mtime.getTime() returned fractions
        // (node: integers) and sub-ms drift (e.g. the cp preserveTimestamps
        // chain) became observable through getTime().
        let clipped_ms = if !ms.is_finite() || ms.abs() > 8.64e15 {
            f64::NAN
        } else {
            ms.trunc()
        };
        let date_obj = w2::NewDateObject(cx_ref, mozjs::jsapi::ClippedTime { t: clipped_ms });
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
/// Shared FileHandle prototype — cached in gc_store ("fsinternal" /
/// "FileHandlePrototype").
/// @trace REQ-ENG-004 [entity:FileHandle] — prototype-based because node's
/// tests patch FileHandle.prototype (fd accessor swap in the aggregate-error
/// tests); adds readv/writev, the EventEmitter face ('close' after close),
/// and createReadStream/createWriteStream (node v24 proto surface oracle). Prototype-based because node's tests patch
/// `FileHandle.prototype` (fd accessor swap in the aggregate-error tests,
/// method overrides) and instances must see the patched surface.
/// node v24 proto surface (oracle): fd, read, readv, readFile, readLines,
/// write, writev, writeFile, appendFile, stat, truncate, utimes, chmod, chown,
/// datasync, sync, createReadStream, createWriteStream (+ EventEmitter face).
unsafe fn filehandle_prototype(cx: *mut JSContext) -> *mut JSObject {
    if let ::std::option::Option::Some(existing) =
        crate::gc_store::gc_store_get_ns(cx, "fsinternal", "FileHandlePrototype")
    {
        if !existing.is_null() {
            return existing;
        }
    }
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let proto = mozjs_sys::jsapi::JS_NewPlainObject(cx));
    if proto.get().is_null() {
        return ::std::ptr::null_mut();
    }
    // fd accessor (real getter property — node parity: getOwnPropertyDescriptor
    // reports an accessor whose .get the aggregate-error tests swap out).
    unsafe {
        JS_DefineProperty1(
            cx,
            proto.handle().into(),
            c"fd".as_ptr(),
            Some(fh_fd_getter),
            None,
            JSPROP_ENUMERATE as u32,
        );
    }
        let fh_read_op: JSNative = Some(fh_read);
    let fh_write_op: JSNative = Some(fh_write);
    let fh_close_op: JSNative = Some(fh_close);
    let fh_stat_op: JSNative = Some(fh_stat);
    let fh_chmod_op: JSNative = Some(fh_chmod);
    let fh_chown_op: JSNative = Some(fh_chown);
    let fh_datasync_op: JSNative = Some(fh_datasync);
    let fh_sync_op: JSNative = Some(fh_sync);
    let fh_truncate_op: JSNative = Some(fh_truncate);
    let fh_utimes_op: JSNative = Some(fh_utimes);
    let fh_append_file_op: JSNative = Some(fh_append_file);
    let fh_read_file_op: JSNative = Some(fh_read_file);
    let fh_write_file_op: JSNative = Some(fh_write_file);
    let fh_read_lines_op: JSNative = Some(fh_read_lines);
    let fh_readv_op: JSNative = Some(fh_readv);
    let fh_writev_op: JSNative = Some(fh_writev);
    let fh_create_read_stream_op: JSNative = Some(fh_create_read_stream);
    let fh_create_write_stream_op: JSNative = Some(fh_create_write_stream);
    let ee_on_op: JSNative = Some(crate::node_events::ee_on);
    let ee_on_op: JSNative = Some(crate::node_events::ee_on);
    let ee_once_op: JSNative = Some(crate::node_events::ee_once);
    let ee_off_op: JSNative = Some(crate::node_events::ee_off);
    let ee_off_op: JSNative = Some(crate::node_events::ee_off);
    let ee_emit_op: JSNative = Some(crate::node_events::ee_emit);
for (name, op, nargs) in [
        ("read", fh_read_op, 4u32),
        ("write", fh_write_op, 4),
        ("close", fh_close_op, 0),
        ("stat", fh_stat_op, 0),
        ("chmod", fh_chmod_op, 1),
        ("chown", fh_chown_op, 2),
        ("datasync", fh_datasync_op, 0),
        ("sync", fh_sync_op, 0),
        ("truncate", fh_truncate_op, 1),
        ("utimes", fh_utimes_op, 2),
        ("appendFile", fh_append_file_op, 1),
        ("readFile", fh_read_file_op, 0),
        ("writeFile", fh_write_file_op, 1),
        ("readLines", fh_read_lines_op, 0),
        ("readv", fh_readv_op, 2),
        ("writev", fh_writev_op, 2),
        ("createReadStream", fh_create_read_stream_op, 1),
        ("createWriteStream", fh_create_write_stream_op, 1),
        // EventEmitter face — FileHandle emits 'close' (node parity).
        ("on", ee_on_op, 2),
        ("addListener", ee_on_op, 2),
        ("once", ee_once_op, 2),
        ("off", ee_off_op, 2),
        ("removeListener", ee_off_op, 2),
        ("emit", ee_emit_op, 2),
    ] {
        let Some(op) = op else { continue };
        let c_name = ZBox::from_bytes(name.as_bytes());
        unsafe {
            JS_DefineFunction(
                cx,
                proto.handle().into(),
                c_name.as_ptr(),
                Some(op),
                nargs,
                JSPROP_ENUMERATE as u32,
            );
        }
    }
    // @trace REQ-ENG-007 — ERM (Explicit Resource Management) faces:
    // `await using fh` consults Symbol.asyncDispose (async context); plain
    // `using fh` consults Symbol.dispose. node v24.19.0 oracle: FileHandle
    // exposes Symbol.asyncDispose (closes the fd, idempotent, resolves
    // undefined) and does NOT expose Symbol.dispose; the previous bao shape
    // leaked a non-callable Symbol.dispose placeholder so every await-using
    // block threw "'Symbol.dispose' property is not callable"
    // (test-fs-read-offset-null / test-fs-read-empty-buffer both open with
    // `await using filehandle = await fsPromises.open(...)`).
    let async_dispose_f =
        unsafe { JS_NewFunction(cx, Some(fh_symbol_async_dispose), 0, 0, c"[Symbol.asyncDispose]".as_ptr()) };
    if !async_dispose_f.is_null() {
        let ad_obj = unsafe { JS_GetFunctionObject(async_dispose_f) };
        if !ad_obj.is_null() {
            let sym_key = mozjs_sys::jsapi::JS::PropertyKey {
                asBits_: unsafe {
                    mozjs::glue::bao_GetWellKnownSymbolKeyRaw(
                        cx,
                        mozjs_sys::jsapi::JS::SymbolCode::asyncDispose as u32,
                    )
                },
            };
            rooted!(&in(cx_ref) let ad_val = mozjs::jsval::ObjectValue(ad_obj));
            unsafe {
                JS_DefinePropertyById2(
                    cx,
                    proto.handle().into(),
                    Handle::from_marked_location(&sym_key),
                    ad_val.handle().into(),
                    0,
                );
            }
        }
    }
    let dispose_f =
        unsafe { JS_NewFunction(cx, Some(fh_symbol_dispose), 0, 0, c"[Symbol.dispose]".as_ptr()) };
    if !dispose_f.is_null() {
        let d_obj = unsafe { JS_GetFunctionObject(dispose_f) };
        if !d_obj.is_null() {
            let sym_key = mozjs_sys::jsapi::JS::PropertyKey {
                asBits_: unsafe {
                    mozjs::glue::bao_GetWellKnownSymbolKeyRaw(
                        cx,
                        mozjs_sys::jsapi::JS::SymbolCode::dispose as u32,
                    )
                },
            };
            rooted!(&in(cx_ref) let d_val = mozjs::jsval::ObjectValue(d_obj));
            unsafe {
                JS_DefinePropertyById2(
                    cx,
                    proto.handle().into(),
                    Handle::from_marked_location(&sym_key),
                    d_val.handle().into(),
                    0,
                );
            }
        }
    }
    crate::gc_store::gc_store_insert_ns(cx, "fsinternal", "FileHandlePrototype", proto.get());
    proto.get()
}

/// FileHandle.readLines() — node returns a readline Interface over the file's
/// lines (synchronous return — NOT a promise); the corpus consumes the
/// async-iteration face (`for await (const line of file.readLines())`,
/// test-fs-promises-file-handle-readLines). Lines split on \n with a trailing
/// \r dropped (readline crlfDelay=Infinity default → CRLF is one break), the
/// final newline does not produce a trailing empty line, each line utf8.
/// The node Interface closes the FileHandle when iteration completes (oracle:
/// fh.fd == -1 after a full for-await) — mirrored by the auto-close in the
/// done/return paths. The former surface lacked the method entirely
/// ("file.readLines is not a function").
///
/// Line content is materialized from a pread of the whole file at call time
/// (fresh-handle face; node streams lazily from offset 0 — the line SEQUENCE
/// is identical for the open-iterate-consume shape every consumer drives).
///
/// @trace REQ-ENG-007 [entity:FileHandle] — proto surface (readLines was
/// listed in the oracle surface comment but absent from the method table).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fh_read_lines(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        JS_ReportErrorUTF8(
            cx,
            c"FileHandle.readLines() must be called on a FileHandle instance".as_ptr(),
        );
        args.rval().set(UndefinedValue());
        return false;
    }
    let fd = get_hidden_int(cx, this.to_object(), "_fd");
    if fd < 0 {
        // node oracle (readLines on a closed handle): FileHandle.createReadStream
        // RangeError ERR_OUT_OF_RANGE 'The value of "fd" is out of range. It
        // must be >= 0 && <= 2147483647. Received -1'.
        let msg = "The value of \"fd\" is out of range. It must be >= 0 && <= 2147483647. Received -1";
        return throw_fs_coded_error(cx, "ERR_OUT_OF_RANGE", "RangeError", msg);
    }

    // Whole-content pread from offset 0 (the stream face's start).
    let mut content: Vec<u8> = Vec::new();
    let mut offset: usize = 0;
    loop {
        let mut chunk = [0u8; 65536];
        // pread face — cursor-free read at absolute `offset` (the whole-file
        // walk must not consume the fd cursor; fresh-handle idempotency).
        #[cfg(unix)]
        let n = unsafe {
            libc::pread(
                fd,
                chunk.as_mut_ptr() as *mut ::std::ffi::c_void,
                chunk.len(),
                offset as libc::off_t,
            )
        };
        // Windows twin (libc has no pread): _lseek save/restore around CRT
        // _read — same shape as readSync's pread shim.
        #[cfg(windows)]
        let n = {
            unsafe extern "C" {
                fn _lseek(fd: i32, off: i64, origin: i32) -> i64;
            }
            const SEEK_SET: i32 = 0;
            const SEEK_CUR: i32 = 1;
            unsafe {
                let saved = _lseek(fd, 0, SEEK_CUR);
                _lseek(fd, offset as i64, SEEK_SET);
                let n = win_fd::_read(fd, chunk.as_mut_ptr() as *mut _, chunk.len() as u32);
                _lseek(fd, saved, SEEK_SET);
                n
            }
        };
        if n < 0 {
            let _ = ::std::io::Error::last_os_error();
            JS_ReportErrorUTF8(
                cx,
                c"FileHandle.readLines: read failed".as_ptr(),
            );
            return false;
        }
        if n == 0 {
            break;
        }
        content.extend_from_slice(&chunk[..n as usize]);
        offset += n as usize;
    }

    // Split into lines: \n breaks, trailing \r dropped per line, no trailing
    // empty line after a final newline.
    let mut lines: Vec<::std::string::String> = Vec::new();
    let mut start = 0usize;
    for (i, &b) in content.iter().enumerate() {
        if b == b'\n' {
            let mut end = i;
            if end > start && content[end - 1] == b'\r' {
                end -= 1;
            }
            lines.push(String::from_utf8_lossy(&content[start..end]).into_owned());
            start = i + 1;
        }
    }
    if start < content.len() {
        let mut end = content.len();
        if end > start && content[end - 1] == b'\r' {
            end -= 1;
        }
        lines.push(String::from_utf8_lossy(&content[start..end]).into_owned());
    }

    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let lines_arr = w2::NewArrayObject1(cx_ref, lines.len()));
    if lines_arr.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    for (idx, line) in lines.iter().enumerate() {
        let c_line = ZBox::from_bytes(line.as_bytes());
        let js_str = JS_NewStringCopyZ(cx, c_line.as_ptr());
        if !js_str.is_null() {
            rooted!(&in(cx_ref) let val = mozjs::jsval::StringValue(&*js_str));
            JS_DefineElement(
                cx,
                lines_arr.handle().into(),
                idx as u32,
                val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }

    // Iterator factory: next/return/[Symbol.asyncIterator] over the lines;
    // completing (exhaustion or early return) closes the FileHandle.
    let iter_src = r#"(function(lines, fh) {
  var idx = 0;
  var doneFlag = false;
  var finish = function() {
    if (!doneFlag) {
      doneFlag = true;
      if (fh && typeof fh.close === 'function') { try { fh.close(); } catch (e) {} }
    }
  };
  return {
    next: function() {
      if (doneFlag || idx >= lines.length) {
        finish();
        return Promise.resolve({ value: undefined, done: true });
      }
      var value = lines[idx++];
      if (idx >= lines.length) finish();
      return Promise.resolve({ value: value, done: false });
    },
    return: function() {
      finish();
      return Promise.resolve({ value: undefined, done: true });
    },
    [Symbol.asyncIterator]: function() { return this; },
  };
})"#;
    let mut src_text = mozjs::rust::transform_str_to_source_text(iter_src);
    let mut factory_val = UndefinedValue();
    let factory_h = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut factory_val,
    };
    let compile_opts = mozjs::glue::NewCompileOptions(cx, c"<FileHandle.readLines>".as_ptr(), 1);
    if compile_opts.is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    let ok = unsafe { mozjs_sys::jsapi::JS::Evaluate2(cx, compile_opts, &mut src_text, factory_h) };
    mozjs::glue::DeleteCompileOptions(compile_opts);
    if !ok || !factory_val.is_object() {
        if !ok {
            JS_ClearPendingException(cx);
        }
        args.rval().set(UndefinedValue());
        return false;
    }
    let global = CurrentGlobalOrNull(cx);
    if global.is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    rooted!(&in(cx_ref) let global_root = global);
    rooted!(&in(cx_ref) let factory_root = factory_val);
    rooted!(&in(cx_ref) let fh_root = this.to_object());
    rooted!(&in(cx_ref) let lines_val = mozjs::jsval::ObjectValue(lines_arr.get()));
    rooted!(&in(cx_ref) let fh_val = mozjs::jsval::ObjectValue(fh_root.get()));
// unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
    rooted!(&in(cx_ref) let call_args_rooted = mozjs_sys::jsgc::ValueArray::<2>::new([
        lines_val.get(),
        fh_val.get(),
    ]));
    let call_args = JS::HandleValueArray::from(&call_args_rooted);
    let mut iter_val = UndefinedValue();
    let call_ok = unsafe {
        JS_CallFunctionValue(
            cx,
            global_root.handle().into(),
            factory_root.handle().into(),
            &call_args,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut iter_val,
            },
        )
    };
    if !call_ok || !iter_val.is_object() {
        if !call_ok {
            JS_ClearPendingException(cx);
        }
        args.rval().set(UndefinedValue());
        return false;
    }
    rooted!(&in(cx_ref) let iter_rooted = iter_val);
    args.rval().set(iter_rooted.get());
    true
}

/// `fh[Symbol.asyncDispose]()` — ERM async dispose: delegates to fh.close()
/// (idempotent; resolves undefined). node v24.19.0 oracle: `await using fh`
/// closes the fd at block end; calling asyncDispose on an already-closed
/// handle is a no-op success.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fh_symbol_async_dispose(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    fh_close(cx, argc, vp)
}

/// `fh[Symbol.dispose]()` — ERM sync dispose face (present for the sync
/// `using` form; mirrors node's close idempotency contract).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fh_symbol_dispose(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    fh_close(cx, argc, vp)
}

/// `get fd()` on the prototype — reads the hidden `_fd` slot so instances
/// stay plain data + shared behavior.
unsafe extern "C" fn fh_fd_getter(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if this.is_object() {
        let fd = get_hidden_int(cx, this.to_object(), "_fd");
        args.rval().set(mozjs::jsval::Int32Value(fd));
    } else {
        args.rval().set(mozjs::jsval::Int32Value(-1));
    }
    true
}

unsafe fn create_filehandle_object(cx: *mut JSContext, fd: i32) -> *mut JSObject {
    let proto = filehandle_prototype(cx);
    if proto.is_null() {
        return ::std::ptr::null_mut();
    }
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let proto_root = proto);
    rooted!(&in(cx_ref) let fh = unsafe {
        {
        let mut wcx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        w2::JS_NewObjectWithGivenProto(&mut wcx, ::std::ptr::null(), proto_root.handle().into())
    }
    });
    if fh.get().is_null() {
        return ::std::ptr::null_mut();
    }

    set_hidden_int(cx, fh.get(), "_fd", fd);
    set_hidden_int(cx, fh.get(), "_refs", 1);
    set_hidden_bool(cx, fh.get(), "_closed", false);

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

    // Full node overload resolution + validation (positional /
    // (buffer, options) / (options) forms). Validation failures arrive as a
    // pending exception — the async face surfaces them as promise REJECTIONS
    // (node oracle: fh.read(b, 0, 4, NaN) rejects ERR_OUT_OF_RANGE).
    let parsed = parse_read_window_args(cx, &args, ::std::option::Option::None, 0, ReadWindowFace::Promise);
    let (resolved_buf, offset, length, use_pread, position) = match parsed {
        ::std::result::Result::Ok(w) => w,
        ::std::result::Result::Err(()) => {
            // Reject with the exact coded error the resolver raised.
            let mut exn = UndefinedValue();
            let got = unsafe {
                JS_GetPendingException(
                    cx,
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut exn,
                    },
                )
            };
            unsafe { JS_ClearPendingException(cx) };
            if got && !exn.is_undefined() {
                rooted!(&in(cx_ref) let exn_root = exn);
                unsafe {
                    mozjs_sys::jsapi::JS::RejectPromise(
                        cx,
                        promise.handle().into(),
                        exn_root.handle().into(),
                    );
                }
            } else {
                reject_with_error(cx, promise.get(), "FileHandle.read: invalid arguments");
            }
            args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
            return true;
        }
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
            // caller's buffer window [offset, offset + bytesRead) — the RESOLVED
            // buffer (the options form carries it inside options.buffer; node
            // v24.19.0 oracle: fh.read(b,0,2,0) fills b).
            {
                rooted!(&in(cx_ref) let caller_rb = resolved_buf);
                let off_rb: u32 = offset as u32;
                for (i_rb, &byte_rb) in buf.iter().enumerate() {
                    rooted!(&in(cx_ref) let bv_rb = mozjs::jsval::Int32Value(byte_rb as i32));
                    JS_SetElement(
                        cx,
                        caller_rb.handle().into(),
                        off_rb + i_rb as u32,
                        bv_rb.handle().into(),
                    );
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
                // node: FileHandle emits 'close' after the close settles.
                emit_fh_event(cx, fh_obj, "close");
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
            emit_fh_event(cx, fh_obj, "close");
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

/// Emit one event on a FileHandle instance (node: 'close' after close).
/// Listener throws route through the uncaught router (never swallowed).
unsafe fn emit_fh_event(cx: *mut JSContext, fh_obj: *mut JSObject, event: &str) {
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_r = fh_obj);
    let c_emit_name = ZBox::from_bytes("emit".as_bytes());
    let c_ev = ZBox::from_bytes(event.as_bytes());
    let ev_str = JS_NewStringCopyZ(cx, c_ev.as_ptr());
    if ev_str.is_null() {
        return;
    }
    rooted!(&in(cx_ref) let ev_root = mozjs::jsval::StringValue(&*ev_str));
// unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
    rooted!(&in(cx_ref) let call_args_rooted = mozjs_sys::jsgc::ValueArray::<1>::new([
        ev_root.get(),
    ]));
    let call_args = JS::HandleValueArray::from(&call_args_rooted);
    let mut rval = UndefinedValue();
    let ok = JS_CallFunctionName(
        cx,
        obj_r.handle().into(),
        c_emit_name.as_ptr(),
        &call_args,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        },
    );
    if !ok {
        let mut exn = UndefinedValue();
        JS_GetPendingException(
            cx,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut exn,
            },
        );
        JS_ClearPendingException(cx);
        if !exn.is_undefined() {
            rooted!(&in(cx_ref) let reason_root = exn);
            crate::uncaught::route_uncaught_exception(cx, exn);
        }
    }
}

/// FileHandle.readv(buffers[, position]) → Promise<{ bytesRead, buffers }>.
/// Buffers are filled in place (kernel scatter-gather through the views).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fh_readv(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    fh_readv_writev_impl(cx, argc, vp, true)
}

/// FileHandle.writev(buffers[, position]) → Promise<{ bytesWritten, buffers }>.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fh_writev(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    fh_readv_writev_impl(cx, argc, vp, false)
}

/// Shared readv/writev core (oracle: resolved object carries exactly
/// { bytesRead|bytesWritten, buffers }; position null = current cursor).
unsafe fn fh_readv_writev_impl(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
    is_read: bool,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        JS_ReportErrorUTF8(
            cx,
            c"FileHandle.readv/writev must be called on a FileHandle instance".as_ptr(),
        );
        return false;
    }
    let fh_obj = this.to_object();
    let fd = get_hidden_int(cx, fh_obj, "_fd");
    let op_name = if is_read { "readv" } else { "writev" };
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let promise = mozjs_sys::jsapi::JS::NewPromiseObject(cx, HandleObject::null()));
    if promise.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    if fd < 0 {
        reject_with_error(cx, promise.get(), "FileHandle: fd is invalid (already closed)");
        args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
        return true;
    }
    if argc == 0 || !(*args.get(0).ptr).is_object() {
        reject_with_error(
            cx,
            promise.get(),
            &format!(
                "The \"buffers\" argument must be an array of Buffers. Received {}",
                received_fs_desc(cx, *args.get(0).ptr)
            ),
        );
        args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
        return true;
    }
    rooted!(&in(cx_ref) let buffers = (*args.get(0).ptr).to_object());
    let mut len_val = UndefinedValue();
    JS_GetProperty(
        cx,
        buffers.handle().into(),
        c"length".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut len_val,
        },
    );
    let count = if len_val.is_int32() && len_val.to_int32() > 0 {
        len_val.to_int32() as usize
    } else {
        0
    };
    // View slots collected platform-neutrally as (ptr, len): the unix leg
    // mirrors them 1:1 into libc::iovec at the syscall, the windows leg walks
    // them per-buffer (same shape as the readvSync windows leg).
    let mut slots: Vec<(*mut u8, usize)> = Vec::with_capacity(count);
    for i in 0..count {
        let mut elem = UndefinedValue();
        JS_GetElement(
            cx,
            buffers.handle().into(),
            i as u32,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut elem,
            },
        );
        match view_byte_slot(elem) {
            ::std::option::Option::Some((ptr, len)) if !ptr.is_null() => {
                slots.push((ptr, len));
            }
            _ => {
                reject_with_error(
                    cx,
                    promise.get(),
                    &format!("The \"buffers\" argument must be an array of Buffers (index {})", i),
                );
                args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
                return true;
            }
        }
    }
    // position: number → preadv/pwritev; null/undefined → cursor forms.
    let position: Option<i64> = if argc > 1 {
        let v = *args.get(1).ptr;
        if v.is_int32() {
            ::std::option::Option::Some(v.to_int32() as i64)
        } else if v.is_double() {
            ::std::option::Option::Some(v.to_double() as i64)
        } else {
            None
        }
    } else {
        None
    };
    let transferred: isize = if slots.is_empty() {
        0
    } else {
        #[cfg(unix)]
        {
            // libc::iovec mirrors the slot list 1:1 (same ptr/len, same
            // order) — no JS runs between collection and the syscall.
            let iovecs: Vec<libc::iovec> = slots
                .iter()
                .map(|&(ptr, len)| libc::iovec {
                    iov_base: ptr as *mut ::std::ffi::c_void,
                    iov_len: len,
                })
                .collect();
            unsafe {
                match (is_read, position) {
                    (true, ::std::option::Option::Some(pos)) => libc::preadv(
                        fd,
                        iovecs.as_ptr(),
                        iovecs.len() as i32,
                        pos,
                    ),
                    (true, None) => libc::readv(fd, iovecs.as_ptr(), iovecs.len() as i32),
                    (false, ::std::option::Option::Some(pos)) => libc::pwritev(
                        fd,
                        iovecs.as_ptr(),
                        iovecs.len() as i32,
                        pos,
                    ),
                    (false, None) => libc::writev(fd, iovecs.as_ptr(), iovecs.len() as i32),
                }
            }
        }
        #[cfg(not(unix))]
        {
            // Windows twin: per-buffer CRT read/write; the positional form
            // saves/restores the cursor (same shape as readSync's pread shim).
            // n < 0 → total=-1 → the shared tail rejects with last_os_error.
            unsafe extern "C" {
                fn _lseek(fd: i32, off: i64, origin: i32) -> i64;
            }
            const SEEK_SET: i32 = 0;
            const SEEK_CUR: i32 = 1;
            let mut total: isize = 0;
            for &(ptr, len) in slots.iter() {
                if len == 0 {
                    continue;
                }
                let n = if is_read {
                    match position {
                        ::std::option::Option::Some(pos) => {
                            let saved = unsafe { _lseek(fd, 0, SEEK_CUR) };
                            unsafe { _lseek(fd, pos + total as i64, SEEK_SET) };
                            let n = win_fd::_read(fd, ptr as *mut _, len as u32);
                            unsafe { _lseek(fd, saved, SEEK_SET) };
                            n
                        }
                        ::std::option::Option::None => unsafe {
                            win_fd::_read(fd, ptr as *mut _, len as u32)
                        },
                    }
                } else {
                    match position {
                        ::std::option::Option::Some(pos) => {
                            let saved = unsafe { _lseek(fd, 0, SEEK_CUR) };
                            unsafe { _lseek(fd, pos + total as i64, SEEK_SET) };
                            let n = win_fd::_write(fd, ptr as *const _, len as u32);
                            unsafe { _lseek(fd, saved, SEEK_SET) };
                            n
                        }
                        ::std::option::Option::None => unsafe {
                            win_fd::_write(fd, ptr as *const _, len as u32)
                        },
                    }
                };
                if n < 0 {
                    total = -1;
                    break;
                }
                total += n as isize;
                if (n as usize) < len {
                    break;
                }
            }
            total
        }
    };
    if transferred < 0 {
        let err = ::std::io::Error::last_os_error();
        let code = io_error_code(&err);
        reject_with_error(
            cx,
            promise.get(),
            &format!("{} {}: {}", op_name, code, err),
        );
        args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
        return true;
    }
    // Resolve { bytesRead|bytesWritten, buffers }.
    rooted!(&in(cx_ref) let result = JS_NewPlainObject(cx));
    if result.get().is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    rooted!(&in(cx_ref) let n_val = mozjs::jsval::Int32Value(transferred as i32));
    let count_key = if is_read { c"bytesRead".as_ptr() } else { c"bytesWritten".as_ptr() };
    JS_DefineProperty(
        cx,
        result.handle().into(),
        count_key,
        n_val.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
    rooted!(&in(cx_ref) let bufs_val = mozjs::jsval::ObjectValue(buffers.get()));
    JS_DefineProperty(
        cx,
        result.handle().into(),
        c"buffers".as_ptr(),
        bufs_val.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
    rooted!(&in(cx_ref) let res_val = mozjs::jsval::ObjectValue(result.get()));
    unsafe {
        mozjs_sys::jsapi::JS::ResolvePromise(cx, promise.handle().into(), res_val.handle().into());
    }
    args.rval().set(mozjs::jsval::ObjectValue(promise.get()));
    true
}

/// FileHandle.createReadStream([options]) / createWriteStream([options]) —
/// delegates to the fd-stream factories installed by the fs stream polyfill
/// (same stream objects the path factories build, fd-backed).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fh_create_read_stream(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    fh_create_stream_impl(cx, argc, vp, true)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn fh_create_write_stream(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    fh_create_stream_impl(cx, argc, vp, false)
}

unsafe fn fh_create_stream_impl(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
    is_read: bool,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        JS_ReportErrorUTF8(cx, c"createReadStream/createWriteStream must be called on a FileHandle".as_ptr());
        args.rval().set(UndefinedValue());
        return false;
    }
    let fd = get_hidden_int(cx, this.to_object(), "_fd");
    let factory_key = if is_read { "makeFdReadStream" } else { "makeFdWriteStream" };
    let Some(factory) = crate::gc_store::gc_store_get_ns(cx, "fsinternal", factory_key) else {
        JS_ReportErrorUTF8(
            cx,
            c"createReadStream/createWriteStream: stream polyfill is not installed".as_ptr(),
        );
        args.rval().set(UndefinedValue());
        return false;
    };
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let factory_r = factory);
    let fd_val = mozjs::jsval::Int32Value(fd);
    let opts_val = if argc > 0 { *args.get(0).ptr } else { UndefinedValue() };
    rooted!(&in(cx_ref) let opts_root = opts_val);
// unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
    rooted!(&in(cx_ref) let call_args_rooted = mozjs_sys::jsgc::ValueArray::<2>::new([
        fd_val,
        opts_root.get(),
    ]));
    let call_args = JS::HandleValueArray::from(&call_args_rooted);
    let mut rval = UndefinedValue();
    let global = JS::CurrentGlobalOrNull(cx);
    if global.is_null() {
        JS_ReportErrorUTF8(cx, c"createReadStream/createWriteStream: no global realm".as_ptr());
        args.rval().set(UndefinedValue());
        return false;
    }
    rooted!(&in(cx_ref) let global_r = global);
    rooted!(&in(cx_ref) let factory_val_r = mozjs::jsval::ObjectValue(factory_r.get()));
    let ok = JS_CallFunctionValue(
        cx,
        global_r.handle().into(),
        factory_val_r.handle().into(),
        &call_args,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        },
    );
    if !ok {
        args.rval().set(UndefinedValue());
        return false;
    }
    args.rval().set(rval);
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
            let stats_obj = build_stats_object(cx, &st, stat_bigint_option(cx, &args, 0));
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
    // node time-arg contract (same face as fsPromises.utimes): number|Date —
    // the former body read raw numbers only (a Date arg silently became 0.0)
    // and futimes() truncated to whole seconds (tv_usec: 0).
    let atime = if argc > 0 {
        match parse_fs_time_arg(cx, *args.get(0).ptr, "atime") {
            ::std::result::Result::Ok(t) => t,
            ::std::result::Result::Err(b) => return b,
        }
    } else {
        0.0
    };
    let mtime = if argc > 1 {
        match parse_fs_time_arg(cx, *args.get(1).ptr, "mtime") {
            ::std::result::Result::Ok(t) => t,
            ::std::result::Result::Err(b) => return b,
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
        match futimens_fd(fd, atime, mtime) {
            ::std::result::Result::Ok(()) => {
                resolve_undefined(cx, promise.get());
            }
            ::std::result::Result::Err(e) => {
                reject_with_error(
                    cx,
                    promise.get(),
                    &format!("FileHandle.utimes: {}", e),
                );
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
/// fs.Dir class (ctor + shared prototype), cached in gc_store — the prototype
/// carries read/readSync/close/closeSync/[Symbol.asyncIterator]; instances
/// carry path/_dirPath/hidden iteration state. `dir instanceof fs.Dir` works
/// through the shared prototype (node oracle: Dir proto = read/processReadResult/
/// readSyncRecursive/readSync/close/closeSync/entries + path).
unsafe fn ensure_dir_class(cx: *mut JSContext) -> (*mut JSObject, *mut JSObject) {
    if let ::std::option::Option::Some(ctor) = crate::gc_store::gc_store_get_ns(cx, "fsinternal", "Dir") {
        if !ctor.is_null() {
            let mut proto_val = UndefinedValue();
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(
                ::std::ptr::NonNull::new_unchecked(cx),
            );
            rooted!(&in(wrapped_cx) let ctor_r = ctor);
            JS_GetProperty(
                cx,
                ctor_r.handle().into(),
                c"prototype".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut proto_val,
                },
            );
            if proto_val.is_object() {
                return (ctor, proto_val.to_object());
            }
        }
    }
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    // JSFUN_CONSTRUCTOR (0x400): `new fs.Dir()` must dispatch to
    // dir_ctor_native (its argc==0 arm throws the node ERR_MISSING_ARGS the
    // test-fs-read corpus asserts). Without the flag SM raises "is not a
    // constructor" before the native runs (node_buffer.rs / bun_glob_api.rs
    // JSFUN_CONSTRUCTOR precedent).
    const JSFUN_CONSTRUCTOR: u32 = 0x400;
    let ctor_f =
        unsafe { JS_NewFunction(cx, Some(dir_ctor_native), 1, JSFUN_CONSTRUCTOR, c"Dir".as_ptr()) };
    if ctor_f.is_null() {
        return (::std::ptr::null_mut(), ::std::ptr::null_mut());
    }
    let ctor = unsafe { JS_GetFunctionObject(ctor_f) };
    rooted!(&in(cx_ref) let ctor_r = ctor);
    rooted!(&in(cx_ref) let proto = mozjs_sys::jsapi::JS_NewPlainObject(cx));
    if proto.get().is_null() {
        return (::std::ptr::null_mut(), ::std::ptr::null_mut());
    }
        let dir_read_sync_op: JSNative = Some(dir_read_sync);
    let dir_read_op: JSNative = Some(dir_read);
    let dir_close_sync_op: JSNative = Some(dir_close_sync);
    let dir_close_op: JSNative = Some(dir_close);
for (name, op, nargs) in [
        ("readSync", dir_read_sync_op, 0u32),
        ("read", dir_read_op, 1),
        ("closeSync", dir_close_sync_op, 0),
        ("close", dir_close_op, 1),
    ] {
        let Some(op) = op else { continue };
        let c_name = ZBox::from_bytes(name.as_bytes());
        unsafe {
            JS_DefineFunction(
                cx,
                proto.handle().into(),
                c_name.as_ptr(),
                Some(op),
                nargs,
                JSPROP_ENUMERATE as u32,
            );
        }
    }
    // Symbol.asyncIterator on the prototype (moved from per-instance so every
    // Dir instance iterates; C1 registry #15 discipline kept).
    let sym_ai_key = mozjs_sys::jsapi::JS::PropertyKey {
        asBits_: unsafe {
            mozjs::glue::bao_GetWellKnownSymbolKeyRaw(
                cx,
                mozjs_sys::jsapi::JS::SymbolCode::asyncIterator as u32,
            )
        },
    };
    let ai_f = unsafe { JS_NewFunction(cx, Some(dir_symbol_iterator), 0, 0, c"[Symbol.asyncIterator]".as_ptr()) };
    if !ai_f.is_null() {
        let ai_obj = unsafe { JS_GetFunctionObject(ai_f) };
        if !ai_obj.is_null() {
            rooted!(&in(cx_ref) let ai_val = mozjs::jsval::ObjectValue(ai_obj));
            unsafe {
                JS_DefinePropertyById2(
                    cx,
                    proto.handle().into(),
                    Handle::from_marked_location(&sym_ai_key),
                    ai_val.handle().into(),
                    0,
                );
            }
        }
    }
    unsafe {
        rooted!(&in(cx_ref) let proto_val_r = mozjs::jsval::ObjectValue(proto.get()));
        JS_DefineProperty(
            cx,
            ctor_r.handle().into(),
            c"prototype".as_ptr(),
            proto_val_r.handle().into(),
            (JSPROP_ENUMERATE | JSPROP_READONLY) as u32,
        );
    }
    crate::gc_store::gc_store_insert_ns(cx, "fsinternal", "Dir", ctor);
    (ctor, proto.get())
}

/// `new fs.Dir(path)` — instance factory for the Dir class.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn dir_ctor_native(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        // node v24.19.0 oracle: `new fs.Dir()` → TypeError ERR_MISSING_ARGS
        // 'The "handle" argument must be specified' (test-fs-read.js).
        return throw_fs_coded_error(
            cx,
            "ERR_MISSING_ARGS",
            "TypeError",
            "The \"handle\" argument must be specified",
        );
    }
    let path = match resolve_fs_path_value(cx, *args.get(0).ptr, "path") {
        ::std::result::Result::Ok(p) => p,
        ::std::result::Result::Err(b) => return b,
    };
    let obj = create_dir_object(cx, &path);
    if obj.is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }
    args.rval().set(mozjs::jsval::ObjectValue(obj));
    true
}

unsafe fn create_dir_object(cx: *mut JSContext, dir_path: &str) -> *mut JSObject {
    let (_, proto) = ensure_dir_class(cx);
    if proto.is_null() {
        return ::std::ptr::null_mut();
    }
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let proto_root = proto);
    rooted!(&in(cx_ref) let dir = unsafe {
        {
        let mut wcx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        w2::JS_NewObjectWithGivenProto(&mut wcx, ::std::ptr::null(), proto_root.handle().into())
    }
    });
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
// unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
            rooted!(&in(cx_ref) let err_args_rooted = mozjs_sys::jsgc::ValueArray::<1>::new([
                err_val.get(),
            ]));
            let err_args = JS::HandleValueArray::from(&err_args_rooted);
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
// unrooted-args class (B-sweep 2026-10-26, require.rs golden): call args must live in a rooted ValueArray — bare stack copies do not track GC moves.
        rooted!(&in(cx_ref) let cb_args_rooted = mozjs_sys::jsgc::ValueArray::<2>::new([
            UndefinedValue(),
            next_rooted.get(),
        ]));
        let cb_args = JS::HandleValueArray::from(&cb_args_rooted);
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
