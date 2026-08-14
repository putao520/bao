// @trace REQ-ENG-003
use ::std::ptr::NonNull;

use mozjs::conversions::unsafe_jsstr_to_string;
use mozjs::glue::JS_GetReservedSlot;
use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::error::JsError;
use crate::value::{self, JsValue};

const HOST_OBJECT_SLOT: u32 = 0;

/// Result type for JS operations that may throw exceptions.
pub type JsResult<T> = ::std::result::Result<T, JsError>;

// ---------------------------------------------------------------------------
// JSHostFn — SpiderMonkey host function ABI type
// ---------------------------------------------------------------------------

/// SpiderMonkey host function type (equivalent to Bun's JSHostFn).
///
/// In Bun/JSC: `fn (*JSGlobalObject, *CallFrame) callconv(.c) JSValue`
/// In Bao/SM:  `unsafe extern "C" fn(*mut JSContext, u32, *mut JSVal) -> bool`
///
/// The function returns `true` on success (return value in vp) or `false`
/// if an exception was thrown on cx.
pub type JSHostFn = unsafe extern "C" fn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool;

/// Host function classification (mirrors Bun's HostFunctionType).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostFunctionType {
    /// Free function (no `this` receiver).
    Free,
    /// Instance method (receives `this` via HostObject).
    Method,
    /// Property getter.
    Getter,
    /// Property setter.
    Setter,
    /// Constructor (creates a new JSObject).
    Constructor,
    /// Static method on a class (no `this`, but class context).
    StaticMethod,
}

/// Convert a `JSHostFn` directly — identity function for type-compatible fn pointers.
///
/// This is the SpiderMonkey equivalent of Bun's `toJSHostFn`. The actual
/// trampoline generation is handled by the `#[host_fn]` proc-macro or
/// `define_host_fn!` macro.
pub const fn to_js_host_call(f: JSHostFn) -> JSHostFn {
    f
}

/// Create a new JSFunction from a host function pointer.
///
/// # Safety
/// `cx` must be a valid JSContext. `parent` must be a valid JSObject.
/// `host_fn` must be a valid JSHostFn that remains valid for the function's lifetime.
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn new_runtime_function(
    cx: &mut mozjs::context::JSContext,
    parent: mozjs::rust::Handle<*mut JSObject>,
    name: &str,
    nargs: u32,
    host_fn: JSHostFn,
    flags: u32,
) -> *mut JSObject {
    let c_name = ::std::ffi::CString::new(name).unwrap_or_default();
    let func = w2::JS_DefineFunction(cx, parent, c_name.as_ptr(), Some(host_fn), nargs, flags);
    JS_GetFunctionObject(func)
}
// ---------------------------------------------------------------------------

/// Trait for JS class construction operations.
///
/// Implemented by types annotated with `#[bao_engine_macros::JsClass]`.
/// The macro generates `HostObject` impl automatically; users must
/// implement `JsClassOps` to provide the constructor logic.
pub trait JsClassOps: HostObject {
    /// Construct a new instance of this class from JS call arguments.
    ///
    /// The implementor should:
    /// 1. Create the Rust instance
    /// 2. Create a JS object with the class prototype
    /// 3. Store the Rust pointer in the JS object's reserved slot via `HostObject::to_private`
    /// 4. Return the JS object as a `JsValue`
    unsafe fn construct(cx: *mut JSContext, args: &CallArgs) -> JsResult<JsValue>;
}

// ---------------------------------------------------------------------------
// Console installation
// ---------------------------------------------------------------------------

pub fn install_console(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    rooted!(&in(cx) let console_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if console_obj.get().is_null() {
        return;
    }

    unsafe {
        w2::JS_DefineFunction(
            cx,
            console_obj.handle(),
            c"log".as_ptr(),
            Some(console_log),
            0,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            console_obj.handle(),
            c"error".as_ptr(),
            Some(console_error),
            0,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            console_obj.handle(),
            c"warn".as_ptr(),
            Some(console_warn),
            0,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            console_obj.handle(),
            c"info".as_ptr(),
            Some(console_info),
            0,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            console_obj.handle(),
            c"debug".as_ptr(),
            Some(console_debug),
            0,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            console_obj.handle(),
            c"dir".as_ptr(),
            Some(console_dir),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            console_obj.handle(),
            c"time".as_ptr(),
            Some(console_time),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            console_obj.handle(),
            c"timeEnd".as_ptr(),
            Some(console_time_end),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            console_obj.handle(),
            c"trace".as_ptr(),
            Some(console_trace),
            0,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            console_obj.handle(),
            c"assert".as_ptr(),
            Some(console_assert),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            console_obj.handle(),
            c"clear".as_ptr(),
            Some(console_clear),
            0,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            console_obj.handle(),
            c"count".as_ptr(),
            Some(console_count),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            console_obj.handle(),
            c"countReset".as_ptr(),
            Some(console_count_reset),
            1,
            JSPROP_ENUMERATE as u32,
        );
        w2::JS_DefineFunction(
            cx,
            console_obj.handle(),
            c"table".as_ptr(),
            Some(console_table),
            1,
            JSPROP_ENUMERATE as u32,
        );

        w2::JS_DefineProperty3(
            cx,
            global,
            c"console".as_ptr(),
            console_obj.handle(),
            JSPROP_ENUMERATE as u32,
        );
    }
}

// ---------------------------------------------------------------------------
// HostObject trait — Reserved Slot based native pointer storage
// ---------------------------------------------------------------------------

pub trait HostObject: Sized {
    /// Extract a native pointer from a JS object's reserved slot 0.
    ///
    /// # Safety
    /// `thisv` must be a JS value containing a JSObject with a valid host pointer
    /// stored in reserved slot 0.
    /// @trace BCE-20260618-002 [level:regression]
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe fn from_private(_cx: *mut JSContext, thisv: JSVal) -> *mut Self {
        if !thisv.is_object() {
            return ::std::ptr::null_mut();
        }
        let obj = thisv.to_object();
        let mut slot = UndefinedValue();
        JS_GetReservedSlot(obj, HOST_OBJECT_SLOT, &mut slot);
        // Guard non-private doubles before to_private(). SpiderMonkey encodes
        // private values as doubles with zero high bits; calling to_private()
        // on an undefined/ordinary-double slot asserts is_double() → panic
        // across the extern "C" boundary. Bail out to null when not private.
        if !(slot.is_double() && (slot.asBits_ & 0xFFFF000000000000) == 0) {
            return ::std::ptr::null_mut();
        }
        let ptr = slot.to_private() as *mut Self;
        if ptr.is_null() {
            ::std::ptr::null_mut()
        } else {
            ptr
        }
    }

    /// Store a native pointer into a JS object's reserved slot 0.
    ///
    /// # Safety
    /// `obj` must be a valid JSObject pointer with at least 1 reserved slot.
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe fn to_private(&self, obj: *mut JSObject) {
        let val = mozjs::jsval::PrivateValue(self as *const Self as *const ::std::os::raw::c_void);
        JS_SetReservedSlot(obj, HOST_OBJECT_SLOT, &val);
    }
}

// ---------------------------------------------------------------------------
// Safe JS function call
// ---------------------------------------------------------------------------

/// Safely call a JS function value with the given arguments.
///
/// Returns `Ok(JsValue)` on success, `Err(JsError)` if the call throws.
///
/// # Safety
/// `cx` must be a valid JSContext. `func` must be a callable JS value.
/// All `args` items must be valid JSVal.
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn call_function(
    cx: *mut JSContext,
    func: JSVal,
    this_obj: *mut JSObject,
    args: &[JSVal],
) -> JsResult<JsValue> {
    if !func.is_object() {
        return Err(JsError {
            message: "value is not a function".into(),
            filename: String::new(),
            line: 0,
            column: 0,
            stack: None,
        });
    }

    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));

    rooted!(&in(wrapped_cx) let rooted_func = func);
    rooted!(&in(wrapped_cx) let rooted_this = this_obj);

    let mut rooted_args: Vec<JSVal> = args.to_vec();
    let handle_array = HandleValueArray {
        length_: rooted_args.len(),
        elements_: rooted_args.as_mut_ptr(),
    };

    let mut rval = UndefinedValue();
    let ok = JS_CallFunctionValue(
        cx,
        rooted_this.handle().into(),
        rooted_func.handle().into(),
        &handle_array,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        },
    );

    if !ok {
        Err(take_exception(cx))
    } else {
        Ok(value::jsval_to_jsvalue(cx, rval))
    }
}

/// Safely call a method on a JS object by name.
///
/// # Safety
/// `cx` must be a valid JSContext. `obj` must be a valid JSObject pointer.
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn call_method(
    cx: *mut JSContext,
    obj: *mut JSObject,
    name: &str,
    args: &[JSVal],
) -> JsResult<JsValue> {
    let c_name = ::std::ffi::CString::new(name).unwrap_or_default();
    // BCE-20260619-012: root obj before passing as Handle to JS API.
    let cx_ref = &mut mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(cx_ref) let obj_root = obj);
    let mut func_val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_root.handle().into(),
        c_name.as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut func_val,
        },
    );

    call_function(cx, func_val, obj, args)
}

// ---------------------------------------------------------------------------
// Exception handling
// ---------------------------------------------------------------------------

/// Extract the current pending exception from a JSContext and convert to JsError.
///
/// # Safety
/// `cx` must be a valid JSContext with a pending exception.
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn take_exception(cx: *mut JSContext) -> JsError {
    if !JS_IsExceptionPending(cx) {
        return JsError {
            message: "unknown error".into(),
            filename: String::new(),
            line: 0,
            column: 0,
            stack: None,
        };
    }

    let mut exc = UndefinedValue();
    JS_GetPendingException(
        cx,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut exc,
        },
    );
    JS_ClearPendingException(cx);

    if !exc.is_object() {
        return JsError {
            message: "non-object exception".into(),
            filename: String::new(),
            line: 0,
            column: 0,
            stack: None,
        };
    }

    let obj = exc.to_object();
    // BCE-20260619-012: root obj before passing as Handle to JS API.
    let cx_ref = &mut mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(cx_ref) let obj_root = obj);
    let obj_h = obj_root.handle().into();

    let message = get_string_property(cx, obj_h, "message").unwrap_or_else(|| "error".into());
    let filename = get_string_property(cx, obj_h, "fileName").unwrap_or_else(|| "<unknown>".into());
    let line = get_int_property(cx, obj_h, "lineNumber").unwrap_or(0);
    let column = get_int_property(cx, obj_h, "columnNumber").unwrap_or(0);
    let stack = get_string_property(cx, obj_h, "stack");

    JsError {
        message,
        filename,
        line,
        column,
        stack,
    }
}

/// Check if there is a pending exception and return it as `Err(JsError)`.
/// Returns `Ok(())` if no exception is pending.
///
/// # Safety
/// `cx` must be a valid JSContext.
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn check_exception(cx: *mut JSContext) -> JsResult<()> {
    if JS_IsExceptionPending(cx) {
        Err(take_exception(cx))
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Property helpers
// ---------------------------------------------------------------------------

/// Extract a setter value from CallArgs (first argument or Undefined).
///
/// # Safety
/// `cx` must be a valid JSContext. `args` must be valid CallArgs.
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn extract_setter_value(cx: *mut JSContext, args: &CallArgs) -> JsValue {
    if args.argc_ > 0 {
        crate::value::jsval_to_jsvalue(cx, *args.get(0).ptr)
    } else {
        JsValue::Undefined
    }
}

/// Get a string property from a JS object. Returns None if the property
/// doesn't exist or isn't a string.
///
/// # Safety
/// `cx` must be a valid JSContext. `obj_h` must be a valid Handle to a JSObject.
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn get_string_property(
    cx: *mut JSContext,
    obj_h: Handle<*mut JSObject>,
    name: &str,
) -> ::std::option::Option<String> {
    let c_name = ::std::ffi::CString::new(name).unwrap_or_default();
    let mut val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_h,
        c_name.as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut val,
        },
    );
    if val.is_string() {
        let s = val.to_string();
        if !s.is_null() {
            Some(unsafe_jsstr_to_string(cx, NonNull::new(s)?))
        } else {
            None
        }
    } else {
        None
    }
}

/// Get an integer property from a JS object.
///
/// # Safety
/// `cx` must be a valid JSContext. `obj_h` must be a valid Handle to a JSObject.
#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe fn get_int_property(
    cx: *mut JSContext,
    obj_h: Handle<*mut JSObject>,
    name: &str,
) -> ::std::option::Option<u32> {
    let c_name = ::std::ffi::CString::new(name).unwrap_or_default();
    let mut val = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_h,
        c_name.as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut val,
        },
    );
    if val.is_int32() {
        Some(val.to_int32() as u32)
    } else if val.is_double() {
        Some(val.to_double() as u32)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// JsError / JsValue extensions
// ---------------------------------------------------------------------------

impl JsError {
    /// Throw this error on the given JSContext.
    ///
    /// # Safety
    /// `cx` must be a valid JSContext pointer.
    pub unsafe fn throw_on(&self, cx: *mut JSContext) {
        let msg = ::std::ffi::CString::new(self.message.as_str())
            .unwrap_or_else(|_| ::std::ffi::CString::new("error").unwrap());
        unsafe {
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        }
    }
}

impl JsValue {
    /// Set this value as the return value of a JSNative callback.
    ///
    /// # Safety
    /// `cx` must be a valid JSContext. `args` must be valid CallArgs.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn set_as_rval(self, cx: *mut JSContext, args: &mut CallArgs) {
        args.rval().set(self.to_jsval(cx));
    }
}

// ---------------------------------------------------------------------------
// Console implementation
// ---------------------------------------------------------------------------
//
// Stream routing (Node semantics) through the unified `bun_core::output`
// layer — same single path as `bao_runtime`'s full console and
// `process.stdout.write`: log/info/debug/dir/table/timeEnd/count → stdout;
// warn/error/trace/assert (+ timer/counter warnings) → stderr. The previous
// implementation printed error/warn via `print!` (stdout!) and routed
// timer/count/assert output into the `log` crate macros, where it was
// invisible to script users.

use ::std::cell::RefCell;
use ::std::collections::HashMap;
use ::std::time::Instant;

thread_local! {
    static CONSOLE_TIMERS: RefCell<HashMap<String, Instant>> = RefCell::new(HashMap::new());
    static CONSOLE_COUNTERS: RefCell<HashMap<String, u32>> = RefCell::new(HashMap::new());
}

/// Bring up this thread's `bun_core::output::Source` if not yet configured.
/// Idempotent; publishes the global stream slots from the real stdio fds on
/// first use, so bare embedders that never ran a startup init still get
/// correct routing (no debug_assert trap, no colour clobbering, no JS
/// StackCheck FFI — a console write never executes JavaScript).
fn ensure_output_source() {
    bun_core::output::Source::ensure_thread_source();
}

/// Console stdout line (trailing `\n` appended here).
fn console_out(line: &str) {
    ensure_output_source();
    let mut bytes = Vec::with_capacity(line.len() + 1);
    bytes.extend_from_slice(line.as_bytes());
    bytes.push(b'\n');
    bun_core::output::write_bytes(bun_core::output::Destination::Stdout, &bytes);
}

/// Console stderr line (trailing `\n` appended here).
fn console_err(line: &str) {
    ensure_output_source();
    let mut bytes = Vec::with_capacity(line.len() + 1);
    bytes.extend_from_slice(line.as_bytes());
    bytes.push(b'\n');
    bun_core::output::write_bytes(bun_core::output::Destination::Stderr, &bytes);
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_log(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let args_slice = ::std::slice::from_raw_parts(args.argv_, argc as usize);
    let line = format_args_line(cx, args_slice);
    console_out(&line);
    args.rval().set(UndefinedValue());
    true
}

/// Render call arguments space-separated into one line (Node util.format-ish
/// for the primitives this bootstrap console supports).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn format_args_line(cx: *mut JSContext, args_slice: &[JSVal]) -> String {
    let mut parts = Vec::with_capacity(args_slice.len());
    for val in args_slice {
        parts.push(format_value(cx, *val));
    }
    parts.join(" ")
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn format_value(cx: *mut JSContext, val: JSVal) -> String {
    if val.is_undefined() {
        "undefined".into()
    } else if val.is_null() {
        "null".into()
    } else if val.is_boolean() {
        format!("{}", val.to_boolean())
    } else if val.is_int32() {
        format!("{}", val.to_int32())
    } else if val.is_double() {
        let d = val.to_double();
        if d.is_nan() {
            "NaN".into()
        } else if d.is_infinite() {
            format!("{}", if d > 0.0 { "Infinity" } else { "-Infinity" })
        } else {
            format!("{}", d)
        }
    } else if val.is_string() {
        let s = val.to_string();
        if !s.is_null() {
            unsafe_jsstr_to_string(cx, NonNull::new(s).expect("null-checked JSString"))
        } else {
            String::new()
        }
    } else if val.is_object() {
        "[object Object]".into()
    } else {
        String::new()
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_error(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let args_slice = ::std::slice::from_raw_parts(args.argv_, argc as usize);
    let line = format_args_line(cx, args_slice);
    console_err(&line);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_warn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let args_slice = ::std::slice::from_raw_parts(args.argv_, argc as usize);
    let line = format_args_line(cx, args_slice);
    console_err(&line);
    args.rval().set(UndefinedValue());
    true
}

unsafe extern "C" fn console_info(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    unsafe { console_log(cx, argc, vp) }
}

unsafe extern "C" fn console_debug(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    unsafe { console_log(cx, argc, vp) }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_dir(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc > 0 {
        let s = format_value(cx, *args.get(0).ptr);
        console_out(&s);
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_time(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let label = extract_label(cx, argc, &args);
    CONSOLE_TIMERS.with(|t| t.borrow_mut().insert(label, Instant::now()));
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_time_end(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let label = extract_label(cx, argc, &args);
    let elapsed = CONSOLE_TIMERS
        .with(|t| t.borrow_mut().remove(&label))
        .map(|start| start.elapsed());
    if let Some(d) = elapsed {
        console_out(&format!("{}: {:.3}ms", label, d.as_secs_f64() * 1000.0));
    } else {
        // Node routes this diagnostic through console.warn → stderr.
        console_err(&format!(
            "Warning: No such label '{}' for console.timeEnd()",
            label
        ));
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_trace(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let args_slice = ::std::slice::from_raw_parts(args.argv_, argc as usize);
    let label = if args_slice.is_empty() {
        String::new()
    } else {
        format!(" {}", format_args_line(cx, args_slice))
    };
    // Node: console.trace prints "Trace:" + stack to stderr.
    console_err(&format!("Trace{}:", label));
    console_err("    at <anonymous>");
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_assert(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc > 0 {
        let cond = *args.get(0).ptr;
        if !cond.is_boolean() || !cond.to_boolean() {
            let args_slice = ::std::slice::from_raw_parts(args.argv_, argc as usize);
            let mut msg = String::from("Assertion failed");
            let extra: Vec<String> = args_slice
                .iter()
                .skip(1)
                .map(|val| format_value(cx, *val))
                .collect();
            if !extra.is_empty() {
                msg.push_str(": ");
                msg.push_str(&extra.join(" "));
            }
            console_err(&msg);
        }
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_clear(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    // ANSI clear screen + move cursor home on stdout, no trailing newline
    // (Node writes the escape sequence verbatim).
    ensure_output_source();
    bun_core::output::write_bytes(
        bun_core::output::Destination::Stdout,
        b"\x1b[2J\x1b[H",
    );
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_count(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let label = extract_label(cx, argc, &args);
    let count = CONSOLE_COUNTERS.with(|c| {
        let mut map = c.borrow_mut();
        let entry = map.entry(label.clone()).or_insert(0);
        *entry += 1;
        *entry
    });
    console_out(&format!("{}: {}", label, count));
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_count_reset(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let label = extract_label(cx, argc, &args);
    let existed = CONSOLE_COUNTERS.with(|c| {
        let mut map = c.borrow_mut();
        if map.contains_key(&label) {
            map.insert(label.clone(), 0);
            true
        } else {
            false
        }
    });
    if !existed {
        // Node routes this diagnostic through console.warn → stderr.
        console_err(&format!("Warning: Count for '{}' does not exist", label));
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_table(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    console_log(cx, argc, vp)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn extract_label(cx: *mut JSContext, argc: u32, args: &CallArgs) -> String {
    if argc > 0 && (*args.get(0).ptr).is_string() {
        let s = (*args.get(0).ptr).to_string();
        if !s.is_null() {
            unsafe_jsstr_to_string(cx, NonNull::new(s).expect("null-checked JSString"))
        } else {
            "default".into()
        }
    } else {
        "default".into()
    }
}

// ---------------------------------------------------------------------------
// ArgReader — typed argument extraction from CallArgs
// ---------------------------------------------------------------------------

/// Typed argument reader wrapping SpiderMonkey CallArgs.
///
/// Provides safe extraction of typed arguments from JS function calls.
pub struct ArgReader<'a> {
    cx: *mut JSContext,
    args: &'a CallArgs,
}

impl<'a> ArgReader<'a> {
    /// Create an ArgReader from a JSContext and CallArgs.
    ///
    /// # Safety
    /// `cx` must be a valid JSContext. `args` must be valid CallArgs for the current call.
    pub unsafe fn new(cx: *mut JSContext, args: &'a CallArgs) -> Self {
        ArgReader { cx, args }
    }

    /// Number of arguments passed.
    pub fn argc(&self) -> u32 {
        self.args.argc_
    }

    /// Check if argument at `index` exists.
    pub fn has(&self, index: u32) -> bool {
        index < self.args.argc_
    }

    /// Get raw JSVal at index, or UndefinedValue if out of bounds.
    ///
    /// # Safety
    /// `cx` must be a valid JSContext. `index` must be in bounds or handled gracefully.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn get_raw(&self, index: u32) -> JSVal {
        if index < self.args.argc_ {
            *self.args.get(index).ptr
        } else {
            UndefinedValue()
        }
    }

    /// Extract a string argument. Returns default if missing or not a string.
    ///
    /// # Safety
    /// Must be called within a valid JSContext scope.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn get_string(&self, index: u32) -> ::std::string::String {
        let val = self.get_raw(index);
        if val.is_string() {
            let s = val.to_string();
            if !s.is_null() {
                unsafe_jsstr_to_string(self.cx, NonNull::new(s).expect("null-checked"))
            } else {
                ::std::string::String::new()
            }
        } else {
            ::std::string::String::new()
        }
    }

    /// Extract an optional string argument.
    ///
    /// # Safety
    /// Must be called within a valid JSContext scope.
    pub unsafe fn get_optional_string(
        &self,
        index: u32,
    ) -> ::std::option::Option<::std::string::String> {
        if !self.has(index) {
            return None;
        }
        let s = unsafe { self.get_string(index) };
        if s.is_empty() { None } else { Some(s) }
    }

    /// Extract an i32 argument. Returns default if missing or not a number.
    ///
    /// # Safety
    /// Must be called within a valid JSContext scope.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn get_int(&self, index: u32) -> i32 {
        let val = self.get_raw(index);
        if val.is_int32() {
            val.to_int32()
        } else if val.is_double() {
            val.to_double() as i32
        } else {
            0
        }
    }

    /// Extract an f64 argument. Returns default if missing or not a number.
    ///
    /// # Safety
    /// Must be called within a valid JSContext scope.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn get_f64(&self, index: u32) -> f64 {
        let val = self.get_raw(index);
        if val.is_int32() {
            val.to_int32() as f64
        } else if val.is_double() {
            val.to_double()
        } else {
            0.0
        }
    }

    /// Extract a bool argument. Returns false if missing or not a boolean.
    ///
    /// # Safety
    /// Must be called within a valid JSContext scope.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn get_bool(&self, index: u32) -> bool {
        let val = self.get_raw(index);
        if val.is_boolean() {
            val.to_boolean()
        } else {
            false
        }
    }

    /// Extract a JSObject pointer argument. Returns null if missing or not an object.
    ///
    /// # Safety
    /// Must be called within a valid JSContext scope.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn get_object(&self, index: u32) -> *mut JSObject {
        let val = self.get_raw(index);
        if val.is_object() {
            val.to_object()
        } else {
            ::std::ptr::null_mut()
        }
    }

    /// Extract a JsValue at index.
    ///
    /// # Safety
    /// Must be called within a valid JSContext scope.
    pub unsafe fn get_value(&self, index: u32) -> JsValue {
        unsafe { value::jsval_to_jsvalue(self.cx, self.get_raw(index)) }
    }

    /// Set the return value to undefined.
    ///
    /// # Safety
    /// Must be called within a valid JSContext scope.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn return_undefined(&self) {
        self.args.rval().set(UndefinedValue());
    }

    /// Set the return value to a JsValue.
    ///
    /// # Safety
    /// Must be called within a valid JSContext scope.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn return_value(&self, val: JsValue) {
        self.args.rval().set(val.to_jsval(self.cx));
    }

    /// Set the return value to a bool.
    ///
    /// # Safety
    /// Must be called within a valid JSContext scope.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn return_bool(&self, v: bool) {
        self.args.rval().set(mozjs::jsval::BooleanValue(v));
    }

    /// Set the return value to an i32.
    ///
    /// # Safety
    /// Must be called within a valid JSContext scope.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn return_int(&self, v: i32) {
        self.args.rval().set(mozjs::jsval::Int32Value(v));
    }

    /// Set the return value to an f64.
    ///
    /// # Safety
    /// Must be called within a valid JSContext scope.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn return_f64(&self, v: f64) {
        self.args.rval().set(mozjs::jsval::DoubleValue(v));
    }

    /// Set the return value to a Rust string (creates a JSString).
    ///
    /// # Safety
    /// Must be called within a valid JSContext scope.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn return_string(&self, s: &str) {
        let c_str = ::std::ffi::CString::new(s).unwrap_or_default();
        let js_str = JS_NewStringCopyZ(self.cx, c_str.as_ptr());
        if !js_str.is_null() {
            self.args.rval().set(mozjs::jsval::StringValue(&*js_str));
        } else {
            self.return_undefined();
        }
    }

    /// Throw an error message on the JSContext and return false.
    ///
    /// # Safety
    /// Must be called within a valid JSContext scope.
    pub unsafe fn throw(&self, msg: &str) -> bool {
        let c_msg = ::std::ffi::CString::new(msg)
            .unwrap_or_else(|_| ::std::ffi::CString::new("error").unwrap());
        unsafe {
            JS_ReportErrorUTF8(self.cx, c"%s".as_ptr(), c_msg.as_ptr());
        }
        false
    }
}

// ---------------------------------------------------------------------------
// define_host_fn! — macro for registering typed host functions
// ---------------------------------------------------------------------------

/// Register a host function on a JS object.
///
/// Generates a `JSNative` trampoline that wraps a safe Rust handler receiving
/// `(&mut JSContext, &ArgReader) -> bool`.
///
/// # Example
/// ```ignore
/// define_host_fn!(cx, obj, "myFunc", 2, |cx, args| {
///     let name = args.get_string(0);
///     let count = args.get_int(1);
///     args.return_string(&format!("{}: {}", name, count));
///     true
/// });
/// ```
#[macro_export]
macro_rules! define_host_fn {
    ($cx:expr, $obj:expr, $name:expr, $nargs:expr, $handler:expr) => {
        unsafe {
            static mut __HANDLER: ::std::option::Option<
                for<'a> fn(
                    *mut mozjs::jsapi::JSContext,
                    &'a $crate::host_fn::ArgReader<'a>,
                ) -> bool,
            > = None;
            ::std::ptr::write_volatile(&mut __HANDLER, Some($handler));
            unsafe extern "C" fn __trampoline(
                cx: *mut mozjs::jsapi::JSContext,
                argc: u32,
                vp: *mut mozjs::jsval::JSVal,
            ) -> bool {
                let args = mozjs::jsapi::CallArgs::from_vp(vp, argc);
                let reader = $crate::host_fn::ArgReader::new(cx, &args);
                match ::std::ptr::read_volatile(&__HANDLER) {
                    Some(h) => h(cx, &reader),
                    None => {
                        args.rval().set(mozjs::jsval::UndefinedValue());
                        true
                    }
                }
            }
            mozjs::rust::wrappers2::JS_DefineFunction(
                $cx,
                $obj,
                $name.as_ptr(),
                Some(__trampoline),
                $nargs,
                mozjs::jsapi::JSPROP_ENUMERATE as u32,
            );
        }
    };
}
