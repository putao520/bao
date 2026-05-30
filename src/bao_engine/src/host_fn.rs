// REQ-ENG-005: Host function registration and callback
use ::std::ptr::NonNull;

use mozjs::conversions::jsstr_to_string;
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
        w2::JS_DefineFunction(cx, console_obj.handle(), c"log".as_ptr(), Some(console_log), 0, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, console_obj.handle(), c"error".as_ptr(), Some(console_error), 0, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, console_obj.handle(), c"warn".as_ptr(), Some(console_warn), 0, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, console_obj.handle(), c"info".as_ptr(), Some(console_info), 0, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, console_obj.handle(), c"debug".as_ptr(), Some(console_debug), 0, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, console_obj.handle(), c"dir".as_ptr(), Some(console_dir), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, console_obj.handle(), c"time".as_ptr(), Some(console_time), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, console_obj.handle(), c"timeEnd".as_ptr(), Some(console_time_end), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, console_obj.handle(), c"trace".as_ptr(), Some(console_trace), 0, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, console_obj.handle(), c"assert".as_ptr(), Some(console_assert), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, console_obj.handle(), c"clear".as_ptr(), Some(console_clear), 0, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, console_obj.handle(), c"count".as_ptr(), Some(console_count), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, console_obj.handle(), c"countReset".as_ptr(), Some(console_count_reset), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, console_obj.handle(), c"table".as_ptr(), Some(console_table), 1, JSPROP_ENUMERATE as u32);

        w2::JS_DefineProperty3(cx, global, c"console".as_ptr(), console_obj.handle(), JSPROP_ENUMERATE as u32);
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
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe fn from_private(_cx: *mut JSContext, thisv: JSVal) -> *mut Self {
        if !thisv.is_object() {
            return ::std::ptr::null_mut();
        }
        let obj = thisv.to_object();
        let mut slot = UndefinedValue();
        JS_GetReservedSlot(obj, HOST_OBJECT_SLOT, &mut slot);
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
    let obj_h = Handle::<*mut JSObject> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &obj,
    };
    let mut func_val = UndefinedValue();
    JS_GetProperty(cx, obj_h, c_name.as_ptr(), MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut func_val,
    });

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
    JS_GetPendingException(cx, MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut exc,
    });
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
    let obj_h = Handle::<*mut JSObject> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &obj,
    };

    let message = get_string_property(cx, obj_h, "message").unwrap_or_else(|| "error".into());
    let filename = get_string_property(cx, obj_h, "fileName").unwrap_or_else(|| "<unknown>".into());
    let line = get_int_property(cx, obj_h, "lineNumber").unwrap_or(0);
    let column = get_int_property(cx, obj_h, "columnNumber").unwrap_or(0);
    let stack = get_string_property(cx, obj_h, "stack");

    JsError { message, filename, line, column, stack }
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
    JS_GetProperty(cx, obj_h, c_name.as_ptr(), MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut val,
    });
    if val.is_string() {
        let s = val.to_string();
        if !s.is_null() {
            Some(jsstr_to_string(cx, NonNull::new(s)?))
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
    JS_GetProperty(cx, obj_h, c_name.as_ptr(), MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut val,
    });
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
    pub fn throw_on(&self, cx: *mut JSContext) {
        let msg = ::std::ffi::CString::new(self.message.as_str())
            .unwrap_or_else(|_| ::std::ffi::CString::new("error").unwrap());
        unsafe {
            JS_ReportErrorUTF8(cx, b"%s\0".as_ptr() as *const ::std::os::raw::c_char, msg.as_ptr());
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

use ::std::cell::RefCell;
use ::std::collections::HashMap;
use ::std::time::Instant;

thread_local! {
    static CONSOLE_TIMERS: RefCell<HashMap<String, Instant>> = RefCell::new(HashMap::new());
    static CONSOLE_COUNTERS: RefCell<HashMap<String, u32>> = RefCell::new(HashMap::new());
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_log(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    for i in 0..argc {
        if i > 0 { print!(" "); }
        print_value(cx, *args.get(i).ptr);
    }
    println!();
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn print_value(cx: *mut JSContext, val: JSVal) {
    if val.is_undefined() {
        print!("undefined");
    } else if val.is_null() {
        print!("null");
    } else if val.is_boolean() {
        print!("{}", val.to_boolean());
    } else if val.is_int32() {
        print!("{}", val.to_int32());
    } else if val.is_double() {
        let d = val.to_double();
        if d.is_nan() {
            print!("NaN");
        } else if d.is_infinite() {
            print!("{}", if d > 0.0 { "Infinity" } else { "-Infinity" });
        } else {
            print!("{}", d);
        }
    } else if val.is_string() {
        let s = val.to_string();
        if !s.is_null() {
            let rust_str = jsstr_to_string(cx, NonNull::new(s).expect("null-checked JSString"));
            print!("{}", rust_str);
        }
    } else if val.is_object() {
        print!("[object Object]");
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_error(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    for i in 0..argc {
        if i > 0 { print!(" "); }
        print_value(cx, *args.get(i).ptr);
    }
    eprintln!();
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_warn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    for i in 0..argc {
        if i > 0 { print!(" "); }
        print_value(cx, *args.get(i).ptr);
    }
    eprintln!();
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
        print_value(cx, *args.get(0).ptr);
    }
    println!();
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
    let elapsed = CONSOLE_TIMERS.with(|t| t.borrow_mut().remove(&label))
        .map(|start| start.elapsed());
    if let Some(d) = elapsed {
        println!("{}: {:.3}ms", label, d.as_secs_f64() * 1000.0);
    } else {
        println!("Timer '{}' does not exist", label);
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_trace(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    for i in 0..argc {
        if i > 0 { print!(" "); }
        print_value(cx, *args.get(i).ptr);
    }
    println!("\n    at <anonymous>");
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_assert(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc > 0 {
        let cond = *args.get(0).ptr;
        if !cond.is_boolean() || !cond.to_boolean() {
            for i in 1..argc {
                if i > 1 { print!(" "); }
                print_value(cx, *args.get(i).ptr);
            }
            println!("\nAssertion failed");
        }
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_clear(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
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
    println!("{}: {}", label, count);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_count_reset(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let label = extract_label(cx, argc, &args);
    CONSOLE_COUNTERS.with(|c| {
        c.borrow_mut().insert(label.clone(), 0);
    });
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
            jsstr_to_string(cx, NonNull::new(s).expect("null-checked JSString"))
        } else {
            "default".into()
        }
    } else {
        "default".into()
    }
}
