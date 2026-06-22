// @trace REQ-ENG-006 [api:node:repl]
//
// Node.js repl module stub. Bao does not implement an interactive REPL
// environment. The module surface matches Bun's repl.ts stub so that
// packages which import the module do not crash.

use mozjs::jsapi::*;
use mozjs::jsval::{ObjectValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;
use ::std::ptr::NonNull;

use crate::require::cache_builtin;

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let obj = unsafe { w2::JS_NewPlainObject(cx) });
    if obj.get().is_null() { return; }

    unsafe {
        let raw_cx = cx.raw_cx();

        // start() — throws not implemented
        let start_fn = JS_NewFunction(raw_cx, Some(repl_start), 0, 0, c"start".as_ptr());
        if !start_fn.is_null() {
            let fn_obj = JS_GetFunctionObject(start_fn);
            if !fn_obj.is_null() {
                rooted!(&in(cx) let val = ObjectValue(fn_obj));
                let _ = JS_DefineProperty(raw_cx, obj.handle().into(), c"start".as_ptr(), val.handle().into(), JSPROP_ENUMERATE as u32);
            }
        }

        // REPLServer class stub (constructor throws not implemented)
        let server_fn = JS_NewFunction(raw_cx, Some(repl_server_constructor), 0, 0x400, c"REPLServer".as_ptr());
        if !server_fn.is_null() {
            let fn_obj = JS_GetFunctionObject(server_fn);
            if !fn_obj.is_null() {
                rooted!(&in(cx) let val = ObjectValue(fn_obj));
                let _ = JS_DefineProperty(raw_cx, obj.handle().into(), c"REPLServer".as_ptr(), val.handle().into(), JSPROP_ENUMERATE as u32);
            }
        }

        // builtinModules — array of Node.js built-in module names
        rooted!(&in(cx) let mods_arr = w2::NewArrayObject1(cx, 0));
        if !mods_arr.get().is_null() {
            let builtin_names: &[&str] = &[
                "assert", "async_hooks", "buffer", "child_process", "cluster",
                "console", "constants", "crypto", "dgram", "diagnostics_channel",
                "dns", "domain", "events", "fs", "http", "http2", "https",
                "inspector", "module", "net", "os", "path", "perf_hooks",
                "process", "punycode", "querystring", "readline", "repl",
                "stream", "string_decoder", "sys", "timers", "tls", "trace_events",
                "tty", "url", "util", "v8", "vm", "worker_threads", "zlib",
            ];
            for (i, name) in builtin_names.iter().enumerate() {
                let c_name = ::std::ffi::CString::new(*name).unwrap();
                let js_str = JS_NewStringCopyZ(raw_cx, c_name.as_ptr());
                if !js_str.is_null() {
                    rooted!(&in(cx) let sv = mozjs::jsval::StringValue(&*js_str));
                    JS_DefineElement(raw_cx, mods_arr.handle().into(), i as u32, sv.handle().into(), (JSPROP_ENUMERATE | JSPROP_READONLY | JSPROP_PERMANENT) as u32);
                }
            }
            rooted!(&in(cx) let arr_val = ObjectValue(mods_arr.get()));
            let _ = JS_DefineProperty(raw_cx, obj.handle().into(), c"builtinModules".as_ptr(), arr_val.handle().into(), JSPROP_ENUMERATE as u32);
        }
    }

    cache_builtin(cx, "repl", obj.get());
}

/// repl.start() — throws not implemented
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn repl_start(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut mozjs::jsval::JSVal,
) -> bool {
    let args = mozjs::jsapi::CallArgs::from_vp(vp, _argc);
    JS_ReportErrorUTF8(cx, c"REPL is not implemented in Bao".as_ptr());
    args.rval().set(UndefinedValue());
    false
}

/// REPLServer constructor — throws not implemented
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn repl_server_constructor(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut mozjs::jsval::JSVal,
) -> bool {
    let args = mozjs::jsapi::CallArgs::from_vp(vp, _argc);
    JS_ReportErrorUTF8(cx, c"REPLServer is not implemented in Bao".as_ptr());
    args.rval().set(UndefinedValue());
    false
}
