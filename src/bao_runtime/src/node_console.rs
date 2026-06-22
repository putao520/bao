// @trace REQ-ENG-006 [api:node:console]
use bun_core::ZBox;
use ::std::cell::RefCell;
use ::std::collections::HashMap;
use ::std::ptr::NonNull;
use ::std::sync::{Mutex, OnceLock};
use ::std::time::Instant;

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue, StringValue, ObjectValue, BooleanValue, Int32Value, DoubleValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

/// Console timers storage — Mutex for cross-call safety (console.time/timeEnd
/// can be called from different scopes). Key = label, Value = Instant.
static CONSOLE_TIMERS: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();

/// Console counters storage.
static CONSOLE_COUNTERS: OnceLock<Mutex<HashMap<String, u32>>> = OnceLock::new();

fn console_timers() -> &'static Mutex<HashMap<String, Instant>> {
    CONSOLE_TIMERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn console_counters() -> &'static Mutex<HashMap<String, u32>> {
    CONSOLE_COUNTERS.get_or_init(|| Mutex::new(HashMap::new()))
}

thread_local! {
    /// Indent level for console.group/groupEnd
    static CONSOLE_INDENT: RefCell<u32> = const { RefCell::new(0) };
}

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let console_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if console_obj.get().is_null() {
        return;
    }

    unsafe {
        // Logging methods
        w2::JS_DefineFunction(cx, console_obj.handle(), c"log".as_ptr(), Some(console_log), 0, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, console_obj.handle(), c"warn".as_ptr(), Some(console_warn), 0, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, console_obj.handle(), c"error".as_ptr(), Some(console_error), 0, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, console_obj.handle(), c"info".as_ptr(), Some(console_info), 0, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, console_obj.handle(), c"debug".as_ptr(), Some(console_debug), 0, JSPROP_ENUMERATE as u32);

        // Formatting methods
        w2::JS_DefineFunction(cx, console_obj.handle(), c"dir".as_ptr(), Some(console_dir), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, console_obj.handle(), c"dirxml".as_ptr(), Some(console_dir), 1, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, console_obj.handle(), c"table".as_ptr(), Some(console_table), 1, JSPROP_ENUMERATE as u32);

        // Timer methods
        w2::JS_DefineFunction(cx, console_obj.handle(), c"time".as_ptr(), Some(console_time), 0, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, console_obj.handle(), c"timeEnd".as_ptr(), Some(console_time_end), 0, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, console_obj.handle(), c"timeLog".as_ptr(), Some(console_time_log), 0, JSPROP_ENUMERATE as u32);

        // Trace / assert
        w2::JS_DefineFunction(cx, console_obj.handle(), c"trace".as_ptr(), Some(console_trace), 0, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, console_obj.handle(), c"assert".as_ptr(), Some(console_assert), 0, JSPROP_ENUMERATE as u32);

        // Counter methods
        w2::JS_DefineFunction(cx, console_obj.handle(), c"count".as_ptr(), Some(console_count), 0, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, console_obj.handle(), c"countReset".as_ptr(), Some(console_count_reset), 0, JSPROP_ENUMERATE as u32);

        // Grouping methods
        w2::JS_DefineFunction(cx, console_obj.handle(), c"group".as_ptr(), Some(console_group), 0, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, console_obj.handle(), c"groupCollapsed".as_ptr(), Some(console_group), 0, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, console_obj.handle(), c"groupEnd".as_ptr(), Some(console_group_end), 0, JSPROP_ENUMERATE as u32);

        // Clear
        w2::JS_DefineFunction(cx, console_obj.handle(), c"clear".as_ptr(), Some(console_clear), 0, JSPROP_ENUMERATE as u32);
    }

    cache_builtin(cx, "console", console_obj.get());
}

// --- Helper: format arguments to a string ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn format_args(cx: *mut JSContext, args: &CallArgs) -> String {
    let mut parts: Vec<String> = Vec::new();
    for i in 0..args.argc_ {
        let val = *args.get(i).ptr;
        let s = js_val_to_display_string(cx, val);
        parts.push(s);
    }
    parts.join(" ")
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn js_val_to_display_string(cx: *mut JSContext, val: JSVal) -> String {
    if val.is_undefined() {
        return "undefined".to_string();
    }
    if val.is_null() {
        return "null".to_string();
    }
    if val.is_boolean() {
        return if val.to_boolean() { "true".to_string() } else { "false".to_string() };
    }
    if val.is_int32() {
        return val.to_int32().to_string();
    }
    if val.is_double() {
        return format!("{}", val.to_double());
    }
    if val.is_string() {
        let s = val.to_string();
        if !s.is_null() {
            return crate::jsstr_to_rust_string(cx, s);
        }
        return String::new();
    }
    if val.is_object() {
        // Try JSON.stringify for objects, fallback to toString
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let obj = val.to_object());

        // Try JSON.stringify
        let global = CurrentGlobalOrNull(cx);
        if !global.is_null() {
            rooted!(&in(cx_ref) let global_rooted = global);
            let mut json_val = UndefinedValue();
            JS_GetProperty(cx, global_rooted.handle().into(), c"JSON".as_ptr(),
                MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut json_val });
            if json_val.is_object() {
                rooted!(&in(cx_ref) let json_obj = json_val.to_object());
                let mut stringify_val = UndefinedValue();
                JS_GetProperty(cx, json_obj.handle().into(), c"stringify".as_ptr(),
                    MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut stringify_val });
                if stringify_val.is_object() {
                    let elems = [ObjectValue(obj.get())];
                    let call_args = HandleValueArray { length_: elems.len(), elements_: elems.as_ptr() };
                    rooted!(&in(cx_ref) let stringify_fn = ObjectValue(stringify_val.to_object()));
                    let mut rval = UndefinedValue();
                    JS_CallFunctionValue(cx, json_obj.handle().into(), stringify_fn.handle().into(),
                        &call_args, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval });
                    if rval.is_string() {
                        let s = rval.to_string();
                        if !s.is_null() {
                            return crate::jsstr_to_rust_string(cx, s);
                        }
                    }
                }
            }
        }
        // Fallback: try toString()
        let mut to_string_rval = UndefinedValue();
        JS_GetProperty(cx, obj.handle().into(), c"toString".as_ptr(),
            MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut to_string_rval });
        if to_string_rval.is_object() {
            rooted!(&in(cx_ref) let to_string_fn = ObjectValue(to_string_rval.to_object()));
            let null_args = HandleValueArray::empty();
            let mut rval2 = UndefinedValue();
            JS_CallFunctionValue(cx, obj.handle().into(), to_string_fn.handle().into(),
                &null_args, MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval2 });
            if rval2.is_string() {
                let s = rval2.to_string();
                if !s.is_null() {
                    return crate::jsstr_to_rust_string(cx, s);
                }
            }
        }
        return "[object]".to_string();
    }
    String::new()
}

/// Get the indent prefix string based on current indent level.
fn get_indent_prefix() -> String {
    CONSOLE_INDENT.with(|indent| {
        let level = *indent.borrow();
        "  ".repeat(level as usize)
    })
}

// --- Logging natives ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_log(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let prefix = get_indent_prefix();
    let msg = format_args(cx, &args);
    eprintln!("{}{}", prefix, msg);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_warn(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let prefix = get_indent_prefix();
    let msg = format_args(cx, &args);
    eprintln!("\x1b[33m{}Warning: {}\x1b[0m", prefix, msg);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_error(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let prefix = get_indent_prefix();
    let msg = format_args(cx, &args);
    eprintln!("\x1b[31m{}Error: {}\x1b[0m", prefix, msg);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_info(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    // info is same as log in most implementations
    let args = CallArgs::from_vp(vp, argc);
    let prefix = get_indent_prefix();
    let msg = format_args(cx, &args);
    eprintln!("{}{}", prefix, msg);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_debug(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    // debug is same as log in most implementations
    let args = CallArgs::from_vp(vp, argc);
    let prefix = get_indent_prefix();
    let msg = format_args(cx, &args);
    eprintln!("{}{}", prefix, msg);
    args.rval().set(UndefinedValue());
    true
}

// --- Formatting ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_dir(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        eprintln!("undefined");
        args.rval().set(UndefinedValue());
        return true;
    }
    let val = *args.get(0).ptr;
    let s = js_val_to_display_string(cx, val);
    eprintln!("{}", s);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_table(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        eprintln!("undefined");
        args.rval().set(UndefinedValue());
        return true;
    }
    // Simplified: output as formatted string (full table rendering would need
    // column width calculation which is overkill for a native implementation)
    let val = *args.get(0).ptr;
    let s = js_val_to_display_string(cx, val);
    eprintln!("{}", s);
    args.rval().set(UndefinedValue());
    true
}

// --- Timers ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_time(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let label = if argc > 0 {
        let val = *args.get(0).ptr;
        if val.is_string() {
            let s = val.to_string();
            if !s.is_null() { crate::jsstr_to_rust_string(cx, s) } else { "default".to_string() }
        } else {
            "default".to_string()
        }
    } else {
        "default".to_string()
    };

    let mut timers = console_timers().lock().unwrap();
    timers.insert(label, Instant::now());
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_time_end(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let label = if argc > 0 {
        let val = *args.get(0).ptr;
        if val.is_string() {
            let s = val.to_string();
            if !s.is_null() { crate::jsstr_to_rust_string(cx, s) } else { "default".to_string() }
        } else {
            "default".to_string()
        }
    } else {
        "default".to_string()
    };

    let mut timers = console_timers().lock().unwrap();
    if let Some(start) = timers.remove(&label) {
        let elapsed = start.elapsed();
        drop(timers);
        let ms = elapsed.as_secs_f64() * 1000.0;
        eprintln!("{}: {}ms", label, ms);
    } else {
        drop(timers);
        eprintln!("Warning: No such label '{}' for console.timeEnd()", label);
    }
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_time_log(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let label = if argc > 0 {
        let val = *args.get(0).ptr;
        if val.is_string() {
            let s = val.to_string();
            if !s.is_null() { crate::jsstr_to_rust_string(cx, s) } else { "default".to_string() }
        } else {
            "default".to_string()
        }
    } else {
        "default".to_string()
    };

    let timers = console_timers().lock().unwrap();
    if let Some(start) = timers.get(&label) {
        let elapsed = start.elapsed();
        drop(timers);
        let ms = elapsed.as_secs_f64() * 1000.0;
        // Additional args after the label
        let mut extra = Vec::new();
        for i in 1..argc {
            let val = *args.get(i).ptr;
            extra.push(js_val_to_display_string(cx, val));
        }
        if extra.is_empty() {
            eprintln!("{}: {}ms", label, ms);
        } else {
            eprintln!("{}: {}ms {}", label, ms, extra.join(" "));
        }
    } else {
        drop(timers);
        eprintln!("Warning: No such label '{}' for console.timeLog()", label);
    }
    args.rval().set(UndefinedValue());
    true
}

// --- Trace / Assert ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_trace(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let label = if argc > 0 {
        format!(" {}", format_args(cx, &args))
    } else {
        String::new()
    };

    // Use DescribeScriptedCaller for stack trace
    let mut filename = "<anonymous>".to_string();
    let mut lineno = 1u32;
    let mut colno = 0u32;
    // Allocate a buffer for the filename
    let mut buf = [0u8; 1024];
    if mozjs::glue::DescribeScriptedCaller(
        cx,
        buf.as_mut_ptr() as *mut ::std::os::raw::c_char,
        buf.len(),
        &mut lineno,
        &mut colno,
    ) {
        // Find the null terminator
        let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        if len > 0 {
            filename = ::std::str::from_utf8(&buf[..len]).unwrap_or("<unknown>").to_string();
        }
    }

    eprintln!("Trace{}:", label);
    eprintln!("    at <anonymous> ({}:{}:{})", filename, lineno, colno);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_assert(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 {
        // console.assert() with no args — no assertion to check
        args.rval().set(UndefinedValue());
        return true;
    }

    let condition = *args.get(0).ptr;
    if condition.is_boolean() && condition.to_boolean() {
        // Assertion passed, do nothing
        args.rval().set(UndefinedValue());
        return true;
    }
    // Also treat truthy non-boolean values as passing
    if !condition.is_boolean() {
        let is_truthy = if condition.is_int32() { condition.to_int32() != 0 }
            else if condition.is_double() { condition.to_double() != 0.0 }
            else if condition.is_string() { true }
            else if condition.is_object() { true }
            else { false };
        if is_truthy {
            args.rval().set(UndefinedValue());
            return true;
        }
    }

    // Assertion failed
    let mut msg_parts: Vec<String> = vec!["Assertion failed".to_string()];
    if argc > 1 {
        // Extra args after the condition are the message
        let mut extra = Vec::new();
        for i in 1..argc {
            let val = *args.get(i).ptr;
            extra.push(js_val_to_display_string(cx, val));
        }
        if !extra.is_empty() {
            msg_parts.push(extra.join(" "));
        }
    }
    eprintln!("\x1b[31m{}\x1b[0m", msg_parts.join(": "));
    args.rval().set(UndefinedValue());
    true
}

// --- Counters ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_count(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let label = if argc > 0 {
        let val = *args.get(0).ptr;
        if val.is_string() {
            let s = val.to_string();
            if !s.is_null() { crate::jsstr_to_rust_string(cx, s) } else { "default".to_string() }
        } else {
            "default".to_string()
        }
    } else {
        "default".to_string()
    };

    let mut counters = console_counters().lock().unwrap();
    let count = counters.entry(label.clone()).or_insert(0);
    *count += 1;
    let current = *count;
    drop(counters);

    eprintln!("{}: {}", label, current);
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_count_reset(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let label = if argc > 0 {
        let val = *args.get(0).ptr;
        if val.is_string() {
            let s = val.to_string();
            if !s.is_null() { crate::jsstr_to_rust_string(cx, s) } else { "default".to_string() }
        } else {
            "default".to_string()
        }
    } else {
        "default".to_string()
    };

    let mut counters = console_counters().lock().unwrap();
    if counters.contains_key(&label) {
        counters.insert(label, 0);
    } else {
        drop(counters);
        eprintln!("Warning: Count for '{}' does not exist", label);
        args.rval().set(UndefinedValue());
        return true;
    }
    args.rval().set(UndefinedValue());
    true
}

// --- Grouping ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_group(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc > 0 {
        let label = format_args(cx, &args);
        eprintln!("{}", label);
    }
    CONSOLE_INDENT.with(|indent| {
        *indent.borrow_mut() += 1;
    });
    args.rval().set(UndefinedValue());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_group_end(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    CONSOLE_INDENT.with(|indent| {
        let mut level = indent.borrow_mut();
        if *level > 0 { *level -= 1; }
    });
    args.rval().set(UndefinedValue());
    true
}

// --- Clear ---

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn console_clear(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    // Print ANSI clear screen + move cursor home
    eprintln!("\x1b[2J\x1b[H");
    args.rval().set(UndefinedValue());
    true
}
