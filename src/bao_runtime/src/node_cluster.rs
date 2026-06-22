// @trace REQ-ENG-006 [api:node:cluster]
//
// Node.js cluster module stub. Bao does not support multi-process clustering
// (single-process runtime). Expose the correct surface so require("cluster")
// succeeds and isPrimary/isWorker return correct values for the main thread.

use mozjs::jsapi::*;
use mozjs::jsval::{BooleanValue, ObjectValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let obj = unsafe { w2::JS_NewPlainObject(cx) });
    if obj.get().is_null() { return; }

    unsafe {
        let raw_cx = cx.raw_cx();

        // isPrimary = true (Bao is always the primary process — no cluster forking)
        rooted!(&in(cx) let is_primary = BooleanValue(true));
        let _ = JS_DefineProperty(raw_cx, obj.handle().into(), c"isPrimary".as_ptr(), is_primary.handle().into(), JSPROP_ENUMERATE as u32);

        // isMaster = true (deprecated alias of isPrimary, kept for compat)
        rooted!(&in(cx) let is_master = BooleanValue(true));
        let _ = JS_DefineProperty(raw_cx, obj.handle().into(), c"isMaster".as_ptr(), is_master.handle().into(), JSPROP_ENUMERATE as u32);

        // isWorker = false
        rooted!(&in(cx) let is_worker = BooleanValue(false));
        let _ = JS_DefineProperty(raw_cx, obj.handle().into(), c"isWorker".as_ptr(), is_worker.handle().into(), JSPROP_ENUMERATE as u32);

        // worker = undefined (no workers in single-process mode)
        rooted!(&in(cx) let worker_val = UndefinedValue());
        let _ = JS_DefineProperty(raw_cx, obj.handle().into(), c"worker".as_ptr(), worker_val.handle().into(), JSPROP_ENUMERATE as u32);

        // workers = empty object
        rooted!(&in(cx) let workers_obj = w2::JS_NewPlainObject(cx));
        if !workers_obj.get().is_null() {
            rooted!(&in(cx) let workers_val = ObjectValue(workers_obj.get()));
            let _ = JS_DefineProperty(raw_cx, obj.handle().into(), c"workers".as_ptr(), workers_val.handle().into(), JSPROP_ENUMERATE as u32);
        }

        // settings = empty object
        rooted!(&in(cx) let settings_obj = w2::JS_NewPlainObject(cx));
        if !settings_obj.get().is_null() {
            rooted!(&in(cx) let settings_val = ObjectValue(settings_obj.get()));
            let _ = JS_DefineProperty(raw_cx, obj.handle().into(), c"settings".as_ptr(), settings_val.handle().into(), JSPROP_ENUMERATE as u32);
        }

        // fork() — throws ERR_CLUSTER_CANNOT_BE_USED
        let fork_fn = JS_NewFunction(raw_cx, Some(cluster_fork), 0, 0, c"fork".as_ptr());
        if !fork_fn.is_null() {
            let fn_obj = JS_GetFunctionObject(fork_fn);
            if !fn_obj.is_null() {
                rooted!(&in(cx) let val = ObjectValue(fn_obj));
                let _ = JS_DefineProperty(raw_cx, obj.handle().into(), c"fork".as_ptr(), val.handle().into(), JSPROP_ENUMERATE as u32);
            }
        }

        // disconnect() — no-op for primary
        let disconnect_fn = JS_NewFunction(raw_cx, Some(cluster_noop_undefined), 0, 0, c"disconnect".as_ptr());
        if !disconnect_fn.is_null() {
            let fn_obj = JS_GetFunctionObject(disconnect_fn);
            if !fn_obj.is_null() {
                rooted!(&in(cx) let val = ObjectValue(fn_obj));
                let _ = JS_DefineProperty(raw_cx, obj.handle().into(), c"disconnect".as_ptr(), val.handle().into(), JSPROP_ENUMERATE as u32);
            }
        }

        // setupPrimary() / setupMaster() — no-op for primary
        let setup_fn = JS_NewFunction(raw_cx, Some(cluster_noop_undefined), 1, 0, c"setupPrimary".as_ptr());
        if !setup_fn.is_null() {
            let fn_obj = JS_GetFunctionObject(setup_fn);
            if !fn_obj.is_null() {
                rooted!(&in(cx) let val = ObjectValue(fn_obj));
                let _ = JS_DefineProperty(raw_cx, obj.handle().into(), c"setupPrimary".as_ptr(), val.handle().into(), JSPROP_ENUMERATE as u32);
            }
        }
        let setup_master_fn = JS_NewFunction(raw_cx, Some(cluster_noop_undefined), 1, 0, c"setupMaster".as_ptr());
        if !setup_master_fn.is_null() {
            let fn_obj = JS_GetFunctionObject(setup_master_fn);
            if !fn_obj.is_null() {
                rooted!(&in(cx) let val = ObjectValue(fn_obj));
                let _ = JS_DefineProperty(raw_cx, obj.handle().into(), c"setupMaster".as_ptr(), val.handle().into(), JSPROP_ENUMERATE as u32);
            }
        }

        // schedulingPolicy = SCHED_NONE (1)
        rooted!(&in(cx) let sched = mozjs::jsval::Int32Value(1));
        let _ = JS_DefineProperty(raw_cx, obj.handle().into(), c"schedulingPolicy".as_ptr(), sched.handle().into(), JSPROP_ENUMERATE as u32);

        // SCHED_NONE = 1, SCHED_RR = 2
        rooted!(&in(cx) let sched_none = mozjs::jsval::Int32Value(1));
        let _ = JS_DefineProperty(raw_cx, obj.handle().into(), c"SCHED_NONE".as_ptr(), sched_none.handle().into(), JSPROP_ENUMERATE as u32);
        rooted!(&in(cx) let sched_rr = mozjs::jsval::Int32Value(2));
        let _ = JS_DefineProperty(raw_cx, obj.handle().into(), c"SCHED_RR".as_ptr(), sched_rr.handle().into(), JSPROP_ENUMERATE as u32);
    }

    cache_builtin(cx, "cluster", obj.get());
}

/// cluster.fork() — throws because Bao does not support multi-process clustering.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cluster_fork(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut mozjs::jsval::JSVal,
) -> bool {
    let args = mozjs::jsapi::CallArgs::from_vp(vp, _argc);
    JS_ReportErrorUTF8(cx, c"Cluster.fork() is not supported in Bao (single-process runtime)".as_ptr());
    args.rval().set(UndefinedValue());
    false
}

/// No-op function returning undefined (for disconnect, setupPrimary, setupMaster).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cluster_noop_undefined(
    _cx: *mut JSContext,
    _argc: u32,
    vp: *mut mozjs::jsval::JSVal,
) -> bool {
    let args = mozjs::jsapi::CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}
