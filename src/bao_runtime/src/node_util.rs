use ::std::ffi::CString;
use ::std::ptr::NonNull;

use mozjs::conversions::jsstr_to_string;
use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue, BooleanValue, ObjectValue, StringValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

pub fn install_util(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let util_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if util_obj.get().is_null() {
        return;
    }

    unsafe {
        w2::JS_DefineFunction(cx, util_obj.handle(), c"inspect".as_ptr(), Some(util_inspect), 1, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"isBoolean".as_ptr(), Some(util_is_boolean), 1, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"isNumber".as_ptr(), Some(util_is_number), 1, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"isString".as_ptr(), Some(util_is_string), 1, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"isSymbol".as_ptr(), Some(util_is_symbol), 1, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"isUndefined".as_ptr(), Some(util_is_undefined), 1, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"isNull".as_ptr(), Some(util_is_null), 1, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"isObject".as_ptr(), Some(util_is_object), 1, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"isFunction".as_ptr(), Some(util_is_function), 1, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"isArray".as_ptr(), Some(util_is_array), 1, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"isDate".as_ptr(), Some(util_is_date), 1, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"isRegExp".as_ptr(), Some(util_is_regexp), 1, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"isError".as_ptr(), Some(util_is_error), 1, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"format".as_ptr(), Some(util_format), 0, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"promisify".as_ptr(), Some(util_promisify), 1, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"callbackify".as_ptr(), Some(util_callbackify), 1, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"deprecate".as_ptr(), Some(util_deprecate), 2, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"getSystemErrorName".as_ptr(), Some(util_get_system_error_name), 1, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"parseArgs".as_ptr(), Some(util_parse_args), 1, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"types".as_ptr(), Some(util_types), 0, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"inherits".as_ptr(), Some(util_inherits), 2, 0);
        w2::JS_DefineFunction(cx, util_obj.handle(), c"isDeepStrictEqual".as_ptr(), Some(util_is_deep_strict_equal), 2, 0);

        let promisify_custom = ObjectValue(util_obj.get());
        rooted!(&in(cx) let pc = promisify_custom);
        JS_DefineProperty(
            cx.raw_cx(), util_obj.handle().into(), c"promisify".as_ptr(),
            pc.handle().into(), JSPROP_ENUMERATE as u32,
        );
    }

    cache_builtin("util", util_obj.get());
}

pub fn install_assert(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let assert_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if assert_obj.get().is_null() {
        return;
    }

    unsafe {
        w2::JS_DefineFunction(cx, assert_obj.handle(), c"ok".as_ptr(), Some(assert_ok), 1, 0);
        w2::JS_DefineFunction(cx, assert_obj.handle(), c"equal".as_ptr(), Some(assert_equal), 2, 0);
        w2::JS_DefineFunction(cx, assert_obj.handle(), c"notEqual".as_ptr(), Some(assert_not_equal), 2, 0);
        w2::JS_DefineFunction(cx, assert_obj.handle(), c"deepEqual".as_ptr(), Some(assert_deep_equal), 2, 0);
        w2::JS_DefineFunction(cx, assert_obj.handle(), c"notDeepEqual".as_ptr(), Some(assert_not_deep_equal), 2, 0);
        w2::JS_DefineFunction(cx, assert_obj.handle(), c"strictEqual".as_ptr(), Some(assert_strict_equal), 2, 0);
        w2::JS_DefineFunction(cx, assert_obj.handle(), c"notStrictEqual".as_ptr(), Some(assert_not_strict_equal), 2, 0);
        w2::JS_DefineFunction(cx, assert_obj.handle(), c"throws".as_ptr(), Some(assert_throws), 1, 0);
        w2::JS_DefineFunction(cx, assert_obj.handle(), c"rejects".as_ptr(), Some(assert_rejects), 1, 0);
        w2::JS_DefineFunction(cx, assert_obj.handle(), c"doesNotThrow".as_ptr(), Some(assert_does_not_throw), 1, 0);
        w2::JS_DefineFunction(cx, assert_obj.handle(), c"fail".as_ptr(), Some(assert_fail), 0, 0);
        w2::JS_DefineFunction(cx, assert_obj.handle(), c"ifError".as_ptr(), Some(assert_if_error), 1, 0);
        w2::JS_DefineFunction(cx, assert_obj.handle(), c"deepStrictEqual".as_ptr(), Some(assert_deep_equal), 2, 0);

        let err_src = ::std::ffi::CString::new(r#"
          function AssertionError(options) {
            this.message = (options && options.message) || "Assertion failed";
            this.actual = options && options.actual;
            this.expected = options && options.expected;
            this.operator = options && options.operator;
            this.stack = new Error().stack;
          }
          AssertionError.prototype = Object.create(Error.prototype);
          AssertionError.prototype.constructor = AssertionError;
          AssertionError.prototype.name = "AssertionError";
          AssertionError;
        "#).unwrap_or_default();
        let mut err_rval = UndefinedValue();
        let err_opts = mozjs::glue::NewCompileOptions(cx.raw_cx(), b"assert\0".as_ptr() as *const ::std::os::raw::c_char, 1);
        if !err_opts.is_null() {
            let mut err_src_text = mozjs::rust::transform_str_to_source_text("function AssertionError(options) { this.message = (options && options.message) || 'Assertion failed'; this.actual = options && options.actual; this.expected = options && options.expected; this.operator = options && options.operator; Error.captureStackTrace && Error.captureStackTrace(this, AssertionError); } AssertionError.prototype = Object.create(Error.prototype); AssertionError.prototype.constructor = AssertionError; AssertionError.prototype.name = 'AssertionError'; AssertionError");
            JS::Evaluate2(cx.raw_cx(), err_opts, &mut err_src_text, MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData, ptr: &mut err_rval,
            });
            libc::free(err_opts as *mut _);
        }
        if err_rval.is_object() {
            let err_ctor = err_rval.to_object();
            let err_val = ObjectValue(err_ctor);
            let err_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &err_val };
            JS_DefineProperty(cx.raw_cx(), assert_obj.handle().into(), c"AssertionError".as_ptr(), err_h, JSPROP_ENUMERATE as u32);
        }

        let assert_fn = JS_NewFunction(cx.raw_cx(), Some(assert_function), 1, 0, c"assert".as_ptr());
        if !assert_fn.is_null() {
            let fn_obj = JS_GetFunctionObject(assert_fn);
            if !fn_obj.is_null() {
                cache_builtin("assert", fn_obj);
            }
        }
    }

    cache_builtin("assert", assert_obj.get());
}

unsafe fn jsval_to_display(cx: *mut JSContext, val: JSVal) -> String { unsafe {
    if val.is_undefined() { return "undefined".to_string(); }
    if val.is_null() { return "null".to_string(); }
    if val.is_boolean() { return val.to_boolean().to_string(); }
    if val.is_int32() { return val.to_int32().to_string(); }
    if val.is_double() { return val.to_double().to_string(); }
    if val.is_string() {
        return jsstr_to_string(cx, NonNull::new(val.to_string()).unwrap());
    }
    if val.is_object() {
        let obj = val.to_object();
        let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        rooted!(&in(wrapped_cx) let obj_r = obj);

        let mut ctor_name = UndefinedValue();
        let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
        JS_GetProperty(cx, obj_h, c"constructor".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut ctor_name });
        if ctor_name.is_object() {
            let ctor = ctor_name.to_object();
            let ctor_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &ctor };
            let mut name_val = UndefinedValue();
            JS_GetProperty(cx, ctor_h, c"name".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut name_val });
            if name_val.is_string() {
                let name = jsstr_to_string(cx, NonNull::new(name_val.to_string()).unwrap());
                return format!("[{}]", name);
            }
        }
        return "[Object]".to_string();
    }
    String::new()
}}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_inspect(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        let s = JS_NewStringCopyZ(cx, b"undefined\0".as_ptr() as *const ::std::os::raw::c_char);
        args.rval().set(if s.is_null() { UndefinedValue() } else { StringValue(&*s) });
        return true;
    }
    let val = *args.get(0).ptr;
    let result = jsval_to_display(cx, val);
    let utf16: Vec<u16> = result.encode_utf16().collect();
    let js_str = JS_NewUCStringCopyN(cx, utf16.as_ptr(), utf16.len());
    args.rval().set(if js_str.is_null() { UndefinedValue() } else { StringValue(&*js_str) });
    true
}

macro_rules! type_check_fn {
    ($name:ident, $check:expr) => {
        #[allow(unsafe_op_in_unsafe_fn)]
        unsafe extern "C" fn $name(_cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
            let args = CallArgs::from_vp(vp, argc);
            if argc == 0 { args.rval().set(BooleanValue(false)); return true; }
            let val = *args.get(0).ptr;
            args.rval().set(BooleanValue($check(&val)));
            true
        }
    };
}

type_check_fn!(util_is_boolean, |v: &JSVal| v.is_boolean());
type_check_fn!(util_is_number, |v: &JSVal| v.is_number());
type_check_fn!(util_is_string, |v: &JSVal| v.is_string());
type_check_fn!(util_is_symbol, |v: &JSVal| v.is_symbol());
type_check_fn!(util_is_undefined, |v: &JSVal| v.is_undefined());
type_check_fn!(util_is_null, |v: &JSVal| v.is_null());
type_check_fn!(util_is_object, |v: &JSVal| v.is_object());

unsafe fn is_function(val: &JSVal) -> bool { unsafe {
    if !val.is_object() { return false; }
    let obj = val.to_object();
    JS_ObjectIsFunction(obj)
}}

type_check_fn!(util_is_function, |v: &JSVal| unsafe { is_function(v) });

unsafe fn is_array(cx: *mut JSContext, val: &JSVal) -> bool { unsafe {
    if !val.is_object() { return false; }
    let mut result = false;
    let v = *val;
    let val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &v };
    IsArrayObject(cx, val_h, &mut result);
    result
}}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_is_array(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 { args.rval().set(BooleanValue(false)); return true; }
    let val = *args.get(0).ptr;
    args.rval().set(BooleanValue(is_array(cx, &val)));
    true
}

unsafe fn has_class_name(cx: *mut JSContext, val: &JSVal, name: &str) -> bool { unsafe {
    if !val.is_object() { return false; }
    let obj = val.to_object();
    let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
    let mut ctor = UndefinedValue();
    JS_GetProperty(cx, obj_h, c"constructor".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut ctor });
    if ctor.is_object() {
        let ctor_obj = ctor.to_object();
        let ctor_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &ctor_obj };
        let mut name_val = UndefinedValue();
        JS_GetProperty(cx, ctor_h, c"name".as_ptr(), MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut name_val });
        if name_val.is_string() {
            let n = jsstr_to_string(cx, NonNull::new(name_val.to_string()).unwrap());
            return n == name;
        }
    }
    false
}}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_is_date(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 { args.rval().set(BooleanValue(false)); return true; }
    let val = *args.get(0).ptr;
    args.rval().set(BooleanValue(has_class_name(cx, &val, "Date")));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_is_regexp(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 { args.rval().set(BooleanValue(false)); return true; }
    let val = *args.get(0).ptr;
    args.rval().set(BooleanValue(has_class_name(cx, &val, "RegExp")));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_is_error(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 { args.rval().set(BooleanValue(false)); return true; }
    let val = *args.get(0).ptr;
    args.rval().set(BooleanValue(has_class_name(cx, &val, "Error")));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_format(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        let s = JS_NewStringCopyZ(cx, b"\0".as_ptr() as *const ::std::os::raw::c_char);
        args.rval().set(if s.is_null() { UndefinedValue() } else { StringValue(&*s) });
        return true;
    }

    let first = *args.get(0).ptr;
    if first.is_string() {
        let fmt = jsstr_to_string(cx, NonNull::new(first.to_string()).unwrap());
        if fmt.contains('%') && argc > 1 {
            let mut arg_idx = 1;
            let mut result = String::new();
            let mut chars = fmt.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '%' {
                    match chars.peek() {
                        Some(&'s') | Some(&'d') | Some(&'i') | Some(&'f') | Some(&'j') | Some(&'o') | Some(&'O') => {
                            chars.next();
                            if arg_idx < argc {
                                result.push_str(&jsval_to_display(cx, *args.get(arg_idx).ptr));
                                arg_idx += 1;
                            }
                        }
                        Some(&'%') => { chars.next(); result.push('%'); }
                        _ => result.push(c),
                    }
                } else {
                    result.push(c);
                }
            }
            let utf16: Vec<u16> = result.encode_utf16().collect();
            let js_str = JS_NewUCStringCopyN(cx, utf16.as_ptr(), utf16.len());
            args.rval().set(if js_str.is_null() { UndefinedValue() } else { StringValue(&*js_str) });
            return true;
        }
    }

    let mut parts: Vec<String> = Vec::new();
    for i in 0..argc {
        parts.push(jsval_to_display(cx, *args.get(i).ptr));
    }
    let result = parts.join(" ");
    let utf16: Vec<u16> = result.encode_utf16().collect();
    let js_str = JS_NewUCStringCopyN(cx, utf16.as_ptr(), utf16.len());
    args.rval().set(if js_str.is_null() { UndefinedValue() } else { StringValue(&*js_str) });
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_promisify(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    if _argc == 0 || !(*args.get(0).ptr).is_object() {
        JS_ReportErrorUTF8(cx, b"promisify requires a function\0".as_ptr() as *const ::std::os::raw::c_char);
        return false;
    }
    args.rval().set(*args.get(0).ptr);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_callbackify(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(*args.get(0).ptr);
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_deprecate(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    if _argc > 0 { args.rval().set(*args.get(0).ptr); } else { args.rval().set(UndefinedValue()); }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_get_system_error_name(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_parse_args(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj = mozjs_sys::jsapi::JS_NewPlainObject(cx));
    args.rval().set(ObjectValue(obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_types(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj = mozjs_sys::jsapi::JS_NewPlainObject(cx));
    if !obj.get().is_null() {
        for (name, fn_ptr) in &[
            ("isBoolean", util_is_boolean as unsafe extern "C" fn(*mut JSContext, u32, *mut JSVal) -> bool),
            ("isNumber", util_is_number),
            ("isString", util_is_string),
            ("isSymbol", util_is_symbol),
            ("isUndefined", util_is_undefined),
            ("isNull", util_is_null),
            ("isObject", util_is_object),
            ("isFunction", util_is_function),
            ("isArray", util_is_array),
        ] {
            let Ok(c_name) = CString::new(*name) else { continue };
            JS_DefineFunction(cx, obj.handle().into(), c_name.as_ptr(), Some(*fn_ptr), 1, JSPROP_ENUMERATE as u32);
        }
    }
    args.rval().set(ObjectValue(obj.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn assert_ok(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        let msg = CString::new("No value argument passed to assert.ok()").unwrap_or_default();
        JS_ReportErrorUTF8(cx, b"%s\0".as_ptr() as *const ::std::os::raw::c_char, msg.as_ptr());
        return false;
    }
    let val = *args.get(0).ptr;
    if !val.to_boolean() {
        let msg = if argc > 1 { jsval_to_display(cx, *args.get(1).ptr) } else { "The expression evaluated to a falsy value".to_string() };
        let c_msg = CString::new(msg).unwrap_or_default();
        JS_ReportErrorUTF8(cx, b"AssertionError: %s\0".as_ptr() as *const ::std::os::raw::c_char, c_msg.as_ptr());
        return false;
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn assert_equal(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc >= 2 {
        let a = jsval_to_display(cx, *args.get(0).ptr);
        let b = jsval_to_display(cx, *args.get(1).ptr);
        if a != b {
            let msg = format!("{} == {}", a, b);
            let c_msg = CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, b"AssertionError: %s\0".as_ptr() as *const ::std::os::raw::c_char, c_msg.as_ptr());
            return false;
        }
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn assert_not_equal(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc >= 2 {
        let a = jsval_to_display(cx, *args.get(0).ptr);
        let b = jsval_to_display(cx, *args.get(1).ptr);
        if a == b {
            let msg = format!("{} != {}", a, b);
            let c_msg = CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, b"AssertionError: %s\0".as_ptr() as *const ::std::os::raw::c_char, c_msg.as_ptr());
            return false;
        }
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn assert_deep_equal(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc >= 2 {
        let a = jsval_to_display(cx, *args.get(0).ptr);
        let b = jsval_to_display(cx, *args.get(1).ptr);
        if a != b {
            let c_msg = CString::new(format!("Expected values to be deeply equal")).unwrap_or_default();
            JS_ReportErrorUTF8(cx, b"AssertionError: %s\0".as_ptr() as *const ::std::os::raw::c_char, c_msg.as_ptr());
            return false;
        }
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn assert_not_deep_equal(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

unsafe fn values_equal_strict(cx: *mut JSContext, a: JSVal, b: JSVal) -> bool { unsafe {
    if a.is_undefined() && b.is_undefined() { return true; }
    if a.is_null() && b.is_null() { return true; }
    if a.is_boolean() && b.is_boolean() { return a.to_boolean() == b.to_boolean(); }
    if a.is_int32() && b.is_int32() { return a.to_int32() == b.to_int32(); }
    if a.is_string() && b.is_string() {
        return jsval_to_display(cx, a) == jsval_to_display(cx, b);
    }
    if a.is_double() || b.is_double() {
        let da = if a.is_double() { a.to_double() } else if a.is_int32() { a.to_int32() as f64 } else { return false };
        let db = if b.is_double() { b.to_double() } else if b.is_int32() { b.to_int32() as f64 } else { return false };
        return da == db;
    }
    false
}}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn assert_strict_equal(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc >= 2 {
        if !values_equal_strict(cx, *args.get(0).ptr, *args.get(1).ptr) {
            let a = jsval_to_display(cx, *args.get(0).ptr);
            let b = jsval_to_display(cx, *args.get(1).ptr);
            let c_msg = CString::new(format!("Expected {} to strictly equal {}", a, b)).unwrap_or_default();
            JS_ReportErrorUTF8(cx, b"AssertionError: %s\0".as_ptr() as *const ::std::os::raw::c_char, c_msg.as_ptr());
            return false;
        }
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn assert_not_strict_equal(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc >= 2 {
        if values_equal_strict(cx, *args.get(0).ptr, *args.get(1).ptr) {
            let c_msg = CString::new(format!("Expected values to be strictly unequal")).unwrap_or_default();
            JS_ReportErrorUTF8(cx, b"AssertionError: %s\0".as_ptr() as *const ::std::os::raw::c_char, c_msg.as_ptr());
            return false;
        }
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn assert_throws(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn assert_rejects(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn assert_does_not_throw(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn assert_fail(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    JS_ReportErrorUTF8(cx, b"AssertionError: fail\0".as_ptr() as *const ::std::os::raw::c_char);
    args.rval().set(UndefinedValue());
    false
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn assert_if_error(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc > 0 {
        let val = *args.get(0).ptr;
        if !val.is_null() && !val.is_undefined() {
            JS_ReportErrorUTF8(cx, b"ifError got unwanted exception\0".as_ptr() as *const ::std::os::raw::c_char);
            return false;
        }
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn assert_function(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    assert_ok(cx, argc, vp)
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_inherits(_cx: *mut JSContext, _argc: u32, _vp: *mut JSVal) -> bool {
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn util_is_deep_strict_equal(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 {
        args.rval().set(BooleanValue(false));
        return true;
    }
    let a = *args.get(0).ptr;
    let b = *args.get(1).ptr;
    let equal = a.is_undefined() && b.is_undefined()
        || a.is_null() && b.is_null()
        || a.is_boolean() && b.is_boolean() && a.to_boolean() == b.to_boolean()
        || a.is_int32() && b.is_int32() && a.to_int32() == b.to_int32()
        || a.is_double() && b.is_double() && a.to_double() == b.to_double()
        || a.is_string() && b.is_string() && {
            let sa = jsstr_to_string(cx, ::std::ptr::NonNull::new_unchecked(a.to_string()));
            let sb = jsstr_to_string(cx, ::std::ptr::NonNull::new_unchecked(b.to_string()));
            sa == sb
        };
    args.rval().set(BooleanValue(equal));
    true
}
