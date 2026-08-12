//! CC Dynamic Workflow host globals on Bao Bun / SpiderMonkey.
//!
//! Installs `agent` / `parallel` / `pipeline` / `phase` / `log` / `args` / `budget`
//! / nested `workflow` for Claude Code workflow scripts (plan-25). Agent work is
//! bridged via a thread-local [`WorkflowHostCallbacks`] set by the Frog runner
//! before eval.
//!
//! Thin crate (no `bun_install` / `bao_native_stubs`) so host tests can link.
//!
//! @trace plan-25 L2 install_workflow_host_on_bun Wave H

#![allow(unsafe_op_in_unsafe_fn)]

use ::std::cell::RefCell;
use ::std::ffi::CString;

// Avoid `mozjs::jsapi::Result` shadowing `std::result::Result`.
type StdResult<T, E> = ::core::result::Result<T, E>;

use bun_core::ZBox;
use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, NullValue, StringValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2::{JS_DefineFunction, JS_DefineProperty3, JS_NewPlainObject};

/// Convert a JS value to a Rust string (null-safe).
///
/// # Safety
/// `cx` must be a valid JSContext for this thread.
pub unsafe fn js_to_rust_string(cx: *mut JSContext, val: JSVal) -> String {
    let ptr = val.to_string();
    match ::std::ptr::NonNull::new(ptr) {
        Some(nn) => mozjs::conversions::jsstr_to_string(cx, nn),
        None => String::new(),
    }
}

/// Callbacks implemented by Frog (or tests) for one workflow run.
pub trait WorkflowHostCallbacks: Send {
    fn phase(&mut self, title: &str);
    fn log(&mut self, message: &str);
    /// Run one agent; return Ok(JSON text or plain string). Err aborts the call.
    /// Uses `std::result::Result` (mozjs jsapi re-exports a different `Result`).
    fn agent(&mut self, prompt: &str, opts_json: &str) -> StdResult<String, String>;
    fn args_json(&self) -> String;
    fn budget_json(&self) -> String;
    /// Nested `workflow(name|ref, args)` — P1 (Wave H1).
    /// Default: not configured (scripts that nest without Frog bridge fail closed).
    fn workflow_nested(&mut self, name_or_ref: &str, args_json: &str) -> StdResult<String, String> {
        let _ = (name_or_ref, args_json);
        Err("workflow nest: not configured".into())
    }
}

thread_local! {
    static HOST: RefCell<Option<Box<dyn WorkflowHostCallbacks>>> = const { RefCell::new(None) };
}

/// Install callbacks for the current thread (call before eval; clear after).
pub fn set_workflow_host_callbacks(cb: Box<dyn WorkflowHostCallbacks>) {
    HOST.with(|h| *h.borrow_mut() = Some(cb));
}

pub fn take_workflow_host_callbacks() -> Option<Box<dyn WorkflowHostCallbacks>> {
    HOST.with(|h| h.borrow_mut().take())
}

/// Invoke host callbacks without holding the TLS `RefCell` borrow across `f`.
///
/// Nested `workflow()` re-enters SM eval and must `set`/`take` child callbacks while
/// the parent call is still on the stack. Holding `borrow_mut` across `f` panics on
/// that re-entry — take ownership for the duration of `f`, then restore parent.
pub fn with_workflow_host<R>(f: impl FnOnce(&mut dyn WorkflowHostCallbacks) -> R) -> Option<R> {
    let mut cb = take_workflow_host_callbacks()?;
    let result = f(cb.as_mut());
    // Child nested eval may leave HOST empty (or stale); always restore parent.
    set_workflow_host_callbacks(cb);
    Some(result)
}

/// Install CC workflow host globals on the realm global (and optional Bun object).
///
/// # Safety
/// `cx` and `global` must be valid for this thread's JSContext.
pub unsafe fn install_workflow_host_on_global(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    // Native primitives
    JS_DefineFunction(
        cx,
        global,
        c"__wf_phase".as_ptr(),
        Some(wf_phase_fn),
        1,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx,
        global,
        c"__wf_log".as_ptr(),
        Some(wf_log_fn),
        1,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx,
        global,
        c"__wf_agent".as_ptr(),
        Some(wf_agent_fn),
        2,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx,
        global,
        c"__wf_args_json".as_ptr(),
        Some(wf_args_json_fn),
        0,
        JSPROP_ENUMERATE as u32,
    );
    JS_DefineFunction(
        cx,
        global,
        c"__wf_budget_json".as_ptr(),
        Some(wf_budget_json_fn),
        0,
        JSPROP_ENUMERATE as u32,
    );
    // H1: nested workflow(name|ref, args)
    JS_DefineFunction(
        cx,
        global,
        c"__wf_workflow".as_ptr(),
        Some(wf_workflow_fn),
        2,
        JSPROP_ENUMERATE as u32,
    );

    // JS surface matching CC host API (async-friendly)
    // H3: parallel uses Promise.all (true barrier), not serial for-await.
    // H1: workflow shim for nested runs.
    // H11: Date.now / Math.random throw non-deterministic.
    let shim = r#"(function(){
  globalThis.phase = function(title){ globalThis.__wf_phase(String(title)); };
  globalThis.log = function(msg){ globalThis.__wf_log(String(msg)); };
  globalThis.agent = async function(prompt, opts){
    const o = opts == null ? '{}' : JSON.stringify(opts);
    const raw = globalThis.__wf_agent(String(prompt), o);
    try { return JSON.parse(raw); } catch (_) { return raw; }
  };
  globalThis.parallel = async function(thunks){
    const results = await Promise.all((thunks||[]).map(async (t) => {
      try { return await t(); } catch (_) { return null; }
    }));
    return results;
  };
  globalThis.pipeline = async function(items, ...stages){
    const list = Array.from(items || []);
    return Promise.all(list.map(async (item, index) => {
      try {
        let prev = item;
        for (const stage of stages) {
          prev = await stage(prev, item, index);
        }
        return prev;
      } catch (_) { return null; }
    }));
  };
  globalThis.workflow = async function(nameOrRef, args){
    const name = typeof nameOrRef === 'string'
      ? nameOrRef
      : (nameOrRef && nameOrRef.scriptPath) || String(nameOrRef);
    const a = args == null ? '{}' : JSON.stringify(args);
    const raw = globalThis.__wf_workflow(String(name), a);
    try { return JSON.parse(raw); } catch (_) { return raw; }
  };
  try {
    globalThis.args = JSON.parse(globalThis.__wf_args_json());
  } catch (_) { globalThis.args = {}; }
  try {
    const b = globalThis.__wf_budget_json();
    globalThis.budget = (b === 'null' || b === '') ? null : JSON.parse(b);
  } catch (_) { globalThis.budget = null; }
  // Deterministic host: throw on forbidden APIs (H11)
  const ban = (name) => { throw new Error("workflow host: non-deterministic API '" + name + "' is forbidden"); };
  Date.now = function(){ ban('Date.now'); };
  Math.random = function(){ ban('Math.random'); };
})();"#;

    rooted!(&in(cx) let mut rval = UndefinedValue());
    let c_filename = CString::new("<workflow-host-shim>").unwrap();
    let opts = mozjs::rust::CompileOptionsWrapper::new(cx, c_filename, 1);
    let _ = mozjs::rust::evaluate_script(cx, global, shim, rval.handle_mut(), opts);
}

/// Install on Bun object as well (alias path `Bun.workflow` marker).
///
/// # Safety
/// Same as [`install_workflow_host_on_global`].
pub unsafe fn install_workflow_host_on_bun(
    cx: &mut mozjs::context::JSContext,
    bun_obj: mozjs::rust::Handle<*mut JSObject>,
) {
    // Marker property so scripts/tests can detect host
    rooted!(&in(cx) let marker = JS_NewPlainObject(cx));
    if !marker.get().is_null() {
        JS_DefineProperty3(
            cx,
            bun_obj,
            c"workflowHost".as_ptr(),
            marker.handle(),
            JSPROP_ENUMERATE as u32,
        );
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn wf_phase_fn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let title = if argc > 0 {
        crate::js_to_rust_string(cx, *args.get(0).ptr)
    } else {
        String::new()
    };
    with_workflow_host(|h| h.phase(&title));
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn wf_log_fn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let msg = if argc > 0 {
        crate::js_to_rust_string(cx, *args.get(0).ptr)
    } else {
        String::new()
    };
    with_workflow_host(|h| h.log(&msg));
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn wf_agent_fn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let prompt = if argc > 0 {
        crate::js_to_rust_string(cx, *args.get(0).ptr)
    } else {
        String::new()
    };
    let opts = if argc > 1 {
        crate::js_to_rust_string(cx, *args.get(1).ptr)
    } else {
        "{}".into()
    };
    let res: Option<StdResult<String, String>> = with_workflow_host(|h| h.agent(&prompt, &opts));
    match res {
        Some(Ok(raw)) => {
            let c = ZBox::from_bytes(raw.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c.as_ptr());
            if !js_str.is_null() {
                args.rval().set(StringValue(&*js_str));
            } else {
                args.rval().set(UndefinedValue());
            }
            true
        }
        Some(Err(e)) => {
            let c = ZBox::from_bytes(e.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c.as_ptr());
            false
        }
        None => {
            JS_ReportErrorUTF8(cx, c"workflow host callbacks not installed".as_ptr());
            false
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn wf_workflow_fn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let name = if argc > 0 {
        crate::js_to_rust_string(cx, *args.get(0).ptr)
    } else {
        String::new()
    };
    let args_json = if argc > 1 {
        crate::js_to_rust_string(cx, *args.get(1).ptr)
    } else {
        "{}".into()
    };
    let res: Option<StdResult<String, String>> =
        with_workflow_host(|h| h.workflow_nested(&name, &args_json));
    match res {
        Some(Ok(raw)) => {
            let c = ZBox::from_bytes(raw.as_bytes());
            let js_str = JS_NewStringCopyZ(cx, c.as_ptr());
            if !js_str.is_null() {
                args.rval().set(StringValue(&*js_str));
            } else {
                args.rval().set(UndefinedValue());
            }
            true
        }
        Some(Err(e)) => {
            let c = ZBox::from_bytes(e.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c.as_ptr());
            false
        }
        None => {
            JS_ReportErrorUTF8(cx, c"workflow host callbacks not installed".as_ptr());
            false
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn wf_args_json_fn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let raw = with_workflow_host(|h| h.args_json()).unwrap_or_else(|| "{}".into());
    let c = ZBox::from_bytes(raw.as_bytes());
    let js_str = JS_NewStringCopyZ(cx, c.as_ptr());
    if !js_str.is_null() {
        args.rval().set(StringValue(&*js_str));
    } else {
        args.rval().set(UndefinedValue());
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn wf_budget_json_fn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let raw = with_workflow_host(|h| h.budget_json()).unwrap_or_else(|| "null".into());
    let c = ZBox::from_bytes(raw.as_bytes());
    let js_str = JS_NewStringCopyZ(cx, c.as_ptr());
    if !js_str.is_null() {
        args.rval().set(StringValue(&*js_str));
    } else {
        args.rval().set(NullValue());
    }
    true
}
