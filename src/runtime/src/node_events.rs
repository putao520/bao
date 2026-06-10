// @trace REQ-ENG-007
use ::std::cell::RefCell;
use ::std::collections::HashMap;
use ::std::ptr::NonNull;

use mozjs::conversions::jsstr_to_string;
use mozjs::glue::JS_GetReservedSlot;
use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, UndefinedValue, Int32Value, BooleanValue, ObjectValue, PrivateValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

struct EmitterState {
    listeners: HashMap<String, Vec<*mut JSObject>>,
    once_flags: HashMap<String, Vec<bool>>,
    max_listeners: u32,
}

/// Internal property name for storing EmitterState on any JS object.
/// Uses \x00 prefix so it's invisible to JS property enumeration.
const STATE_PROP: &[u8] = b"\x00__ee_state\0";

#[allow(dead_code)]
const SLOT_STATE: u32 = 0;

static EMITTER_CLASS: JSClass = JSClass {
    name: c"EventEmitter".as_ptr(),
    flags: (1 << JSCLASS_RESERVED_SLOTS_SHIFT) as u32,
    cOps: ::std::ptr::null(),
    spec: ::std::ptr::null(),
    ext: ::std::ptr::null(),
    oOps: ::std::ptr::null(),
};

thread_local! {
    static EMITTER_PROTO: RefCell<*mut JSObject> = const { RefCell::new(::std::ptr::null_mut()) };
}

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let events_obj = unsafe { mozjs::rust::wrappers2::JS_NewPlainObject(cx) });
    if events_obj.get().is_null() {
        return;
    }

    unsafe {
        rooted!(&in(cx) let global = CurrentGlobalOrNull(cx.raw_cx()));
        if global.get().is_null() {
            return;
        }

        rooted!(&in(cx) let null_proto = ::std::ptr::null_mut::<JSObject>());
        let proto = w2::JS_InitClass(
            cx,
            global.handle(),
            &EMITTER_CLASS,
            null_proto.handle(),
            c"EventEmitter".as_ptr(),
            Some(event_emitter_constructor),
            0,
            ::std::ptr::null(),
            METHODS.as_ptr(),
            ::std::ptr::null(),
            ::std::ptr::null(),
        );

        if !proto.is_null() {
            EMITTER_PROTO.with(|p| *p.borrow_mut() = proto);
        }

        rooted!(&in(cx) let proto_h = proto);
        rooted!(&in(cx) let ctor = JS_GetConstructor(cx.raw_cx(), proto_h.handle().into()));
        if !ctor.get().is_null() {
            let ctor_val = ObjectValue(ctor.get());
            rooted!(&in(cx) let cv = ctor_val);
            JS_DefineProperty(
                cx.raw_cx(), events_obj.handle().into(), c"EventEmitter".as_ptr(),
                cv.handle().into(), (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
            );
            w2::JS_DefineFunction(cx, ctor.handle(), c"listenerCount".as_ptr(), Some(events_static_listener_count), 2, JSPROP_ENUMERATE as u32);
            w2::JS_DefineFunction(cx, ctor.handle(), c"getEventListeners".as_ptr(), Some(events_static_get_event_listeners), 2, JSPROP_ENUMERATE as u32);
        }

        let default_max = Int32Value(10);
        rooted!(&in(cx) let dmv = default_max);
        JS_DefineProperty(
            cx.raw_cx(), events_obj.handle().into(), c"defaultMaxListeners".as_ptr(),
            dmv.handle().into(), (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
        );

        w2::JS_DefineFunction(cx, events_obj.handle(), c"on".as_ptr(), Some(events_static_on), 3, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, events_obj.handle(), c"once".as_ptr(), Some(events_static_once), 3, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, events_obj.handle(), c"listenerCount".as_ptr(), Some(events_static_listener_count), 2, JSPROP_ENUMERATE as u32);
        w2::JS_DefineFunction(cx, events_obj.handle(), c"getEventListeners".as_ptr(), Some(events_static_get_event_listeners), 2, JSPROP_ENUMERATE as u32);
    }

    cache_builtin(cx, "events", events_obj.get());
}

const METHODS: &[JSFunctionSpec] = &[
    JSFunctionSpec {
        name: JSPropertySpec_Name { string_: c"on".as_ptr() },
        call: JSNativeWrapper { op: Some(ee_on), info: ::std::ptr::null_mut() },
        nargs: 2,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name { string_: c"addListener".as_ptr() },
        call: JSNativeWrapper { op: Some(ee_on), info: ::std::ptr::null_mut() },
        nargs: 2,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name { string_: c"addEventListener".as_ptr() },
        call: JSNativeWrapper { op: Some(ee_on), info: ::std::ptr::null_mut() },
        nargs: 2,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name { string_: c"off".as_ptr() },
        call: JSNativeWrapper { op: Some(ee_off), info: ::std::ptr::null_mut() },
        nargs: 2,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name { string_: c"removeListener".as_ptr() },
        call: JSNativeWrapper { op: Some(ee_off), info: ::std::ptr::null_mut() },
        nargs: 2,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name { string_: c"removeEventListener".as_ptr() },
        call: JSNativeWrapper { op: Some(ee_off), info: ::std::ptr::null_mut() },
        nargs: 2,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name { string_: c"emit".as_ptr() },
        call: JSNativeWrapper { op: Some(ee_emit), info: ::std::ptr::null_mut() },
        nargs: 1,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name { string_: c"once".as_ptr() },
        call: JSNativeWrapper { op: Some(ee_once), info: ::std::ptr::null_mut() },
        nargs: 2,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name { string_: c"prependListener".as_ptr() },
        call: JSNativeWrapper { op: Some(ee_prepend), info: ::std::ptr::null_mut() },
        nargs: 2,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name { string_: c"prependOnceListener".as_ptr() },
        call: JSNativeWrapper { op: Some(ee_prepend_once), info: ::std::ptr::null_mut() },
        nargs: 2,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name { string_: c"removeAllListeners".as_ptr() },
        call: JSNativeWrapper { op: Some(ee_remove_all), info: ::std::ptr::null_mut() },
        nargs: 0,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name { string_: c"listeners".as_ptr() },
        call: JSNativeWrapper { op: Some(ee_listeners), info: ::std::ptr::null_mut() },
        nargs: 1,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name { string_: c"listenerCount".as_ptr() },
        call: JSNativeWrapper { op: Some(ee_listener_count), info: ::std::ptr::null_mut() },
        nargs: 1,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name { string_: c"setMaxListeners".as_ptr() },
        call: JSNativeWrapper { op: Some(ee_set_max), info: ::std::ptr::null_mut() },
        nargs: 1,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name { string_: c"getMaxListeners".as_ptr() },
        call: JSNativeWrapper { op: Some(ee_get_max), info: ::std::ptr::null_mut() },
        nargs: 0,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name { string_: c"rawListeners".as_ptr() },
        call: JSNativeWrapper { op: Some(ee_listeners), info: ::std::ptr::null_mut() },
        nargs: 1,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name { string_: c"eventNames".as_ptr() },
        call: JSNativeWrapper { op: Some(ee_event_names), info: ::std::ptr::null_mut() },
        nargs: 0,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name { string_: ::std::ptr::null() },
        call: JSNativeWrapper { op: None, info: ::std::ptr::null_mut() },
        nargs: 0,
        flags: 0,
        selfHostedName: ::std::ptr::null_mut(),
    },
];

fn get_state(cx: *mut JSContext, obj: *mut JSObject) -> Option<Box<EmitterState>> {
    unsafe {
        // Hidden property only — reserved slots are unsafe for plain JS objects
        // (Socket/Server inherit from EE via prototype chain but their own class
        // differs from EMITTER_CLASS, so reading slot 0 returns arbitrary data
        // that may pass is_double() but crash to_private()).
        let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
        let mut hidden = UndefinedValue();
        JS_GetProperty(cx, obj_h, STATE_PROP.as_ptr() as *const ::std::os::raw::c_char,
            MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut hidden });
        if hidden.is_double() {
            // Guard against non-private doubles (defensive — to_private asserts
            // high 16 bits are zero, which only holds for PrivateValue-encoded ptrs).
            if (hidden.asBits_ & 0xFFFF000000000000) != 0 {
                return None;
            }
            let ptr = hidden.to_private() as *mut EmitterState;
            if !ptr.is_null() {
                return Some(Box::from_raw(ptr));
            }
        }
    }
    None
}

fn set_state(cx: *mut JSContext, obj: *mut JSObject, state: Box<EmitterState>) {
    unsafe {
        // Hidden property only — see get_state for why reserved slots are unsafe.
        let val = PrivateValue(Box::into_raw(state) as *const ::std::os::raw::c_void);
        let obj_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj };
        let val_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &val };
        JS_DefineProperty(cx, obj_h, STATE_PROP.as_ptr() as *const ::std::os::raw::c_char,
            val_h, (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32);
    }
}

fn ensure_state(cx: *mut JSContext, obj: *mut JSObject) -> Box<EmitterState> {
    get_state(cx, obj).unwrap_or_else(|| {
        Box::new(EmitterState {
            listeners: HashMap::new(),
            once_flags: HashMap::new(),
            max_listeners: 10,
        })
    })
}

unsafe fn get_event_name(cx: *mut JSContext, args: &CallArgs) -> Option<String> { unsafe {
    if args.argc_ == 0 { return None; }
    let val = *args.get(0).ptr;
    if val.is_string() {
        let s = val.to_string();
        if !s.is_null() {
            return Some(crate::jsstr_to_rust_string(cx, s));
        }
    }
    if val.is_int32() {
        return Some(val.to_int32().to_string());
    }
    // Symbol support: use \x00SYM: prefix to distinguish from string keys
    // Use the Symbol pointer address as a unique identifier
    if val.is_symbol() {
        let sym = val.to_symbol();
        if !sym.is_null() {
            return Some(format!("\x00SYM:{:p}", sym));
        }
    }
    None
}}

unsafe fn get_callback(args: &CallArgs) -> Option<*mut JSObject> { unsafe {
    if args.argc_ < 2 { return None; }
    let val = *args.get(1).ptr;
    if val.is_object() {
        let obj = val.to_object();
        if mozjs_sys::jsapi::js::IsFunctionObject(obj) {
            return Some(obj);
        }
    }
    None
}}

unsafe fn is_function(obj: *mut JSObject) -> bool {
    mozjs_sys::jsapi::js::IsFunctionObject(obj)
}

unsafe fn js_same_value(cx: *mut JSContext, a: JSVal, b: JSVal) -> bool {
    let mut same = false;
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let av = a);
    rooted!(&in(wrapped_cx) let bv = b);
    w2::SameValue(&wrapped_cx, av.handle(), bv.handle(), &mut same);
    same
}

fn throw_type_error(cx: *mut JSContext, msg: &str) {
    let c_msg = bun_core::ZBox::from_bytes(msg.as_bytes());
    unsafe { JS_ReportErrorASCII(cx, c_msg.as_ptr()); }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn event_emitter_constructor(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    // Try normal constructor path (new EventEmitter())
    let this = JS_NewObjectForConstructor(cx, &EMITTER_CLASS, &args);
    if !this.is_null() {
        let state = Box::new(EmitterState {
            listeners: HashMap::new(),
            once_flags: HashMap::new(),
            max_listeners: 10,
        });
        set_state(cx, this, state);
        args.rval().set(ObjectValue(this));
        return true;
    }

    // Fallback: EventEmitter.call(this) pattern — initialize the `this` object
    // Clear the pending exception from JS_NewObjectForConstructor failure
    JS_ClearPendingException(cx);
    let this_val = args.thisv();
    if this_val.is_object() {
        let this_obj = this_val.to_object();
        // Only initialize if not already an EventEmitter
        let existing = get_state(cx, this_obj);
        if let Some(state) = existing {
            set_state(cx, this_obj, state);
        } else {
            let state = Box::new(EmitterState {
                listeners: HashMap::new(),
                once_flags: HashMap::new(),
                max_listeners: 10,
            });
            set_state(cx, this_obj, state);
        }
        args.rval().set(ObjectValue(this_obj));
        return true;
    }

    args.rval().set(UndefinedValue());
    false
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ee_on(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() { args.rval().set(UndefinedValue()); return true; }
    let this_obj = this.to_object();

    let event_name = match get_event_name(cx, &args) {
        Some(n) => n,
        None => { args.rval().set(ObjectValue(this_obj)); return true; }
    };

    if argc >= 2 {
        let listener_val = *args.get(1).ptr;
        if listener_val.is_object() {
            if !is_function(listener_val.to_object()) {
                throw_type_error(cx, "The \"listener\" argument must be of type function. Received object");
                return false;
            }
        } else {
            throw_type_error(cx, "The \"listener\" argument must be of type function");
            return false;
        }
    }

    let callback = match get_callback(&args) {
        Some(cb) => cb,
        None => { args.rval().set(ObjectValue(this_obj)); return true; }
    };

    let mut state = ensure_state(cx, this_obj);
    state.listeners.entry(event_name).or_default().push(callback);
    set_state(cx, this_obj, state);
    args.rval().set(ObjectValue(this_obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ee_once(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() { args.rval().set(UndefinedValue()); return true; }
    let this_obj = this.to_object();

    let event_name = match get_event_name(cx, &args) {
        Some(n) => n,
        None => { args.rval().set(ObjectValue(this_obj)); return true; }
    };
    let callback = match get_callback(&args) {
        Some(cb) => cb,
        None => { args.rval().set(ObjectValue(this_obj)); return true; }
    };

    let mut state = ensure_state(cx, this_obj);
    state.listeners.entry(event_name.clone()).or_default().push(callback);
    state.once_flags.entry(event_name).or_default().push(true);
    set_state(cx, this_obj, state);
    args.rval().set(ObjectValue(this_obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ee_off(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() { args.rval().set(UndefinedValue()); return true; }
    let this_obj = this.to_object();

    let event_name = match get_event_name(cx, &args) {
        Some(n) => n,
        None => { args.rval().set(ObjectValue(this_obj)); return true; }
    };

    let mut state = ensure_state(cx, this_obj);

    if argc < 2 {
        state.listeners.remove(&event_name);
        state.once_flags.remove(&event_name);
        set_state(cx, this_obj, state);
        args.rval().set(ObjectValue(this_obj));
        return true;
    }

    let callback_val = *args.get(1).ptr;
    if !callback_val.is_object() { set_state(cx, this_obj, state); args.rval().set(ObjectValue(this_obj)); return true; }

    if let Some(listeners) = state.listeners.get_mut(&event_name) {
        let mut removed_indices: Vec<usize> = Vec::new();
        let mut i = 0;
        while i < listeners.len() {
            let stored_val = ObjectValue(listeners[i]);
            if js_same_value(cx, stored_val, callback_val) {
                listeners.remove(i);
                removed_indices.push(i);
            } else {
                i += 1;
            }
        }
        if let Some(flags) = state.once_flags.get_mut(&event_name) {
            for idx in removed_indices.into_iter().rev() {
                if idx < flags.len() { flags.remove(idx); }
            }
        }
    }

    set_state(cx, this_obj, state);
    args.rval().set(ObjectValue(this_obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ee_emit(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() { args.rval().set(BooleanValue(false)); return true; }
    let this_obj = this.to_object();

    let event_name = match get_event_name(cx, &args) {
        Some(n) => n,
        None => { args.rval().set(BooleanValue(false)); return true; }
    };

    let mut state = ensure_state(cx, this_obj);
    let listeners = match state.listeners.get(&event_name) {
        Some(l) => l.clone(),
        None => { set_state(cx, this_obj, state); args.rval().set(BooleanValue(false)); return true; }
    };
    let once_flags = state.once_flags.get(&event_name).cloned().unwrap_or_default();
    let had_listeners = !listeners.is_empty();

    let emit_args: Vec<JSVal> = if argc > 1 {
        (1..argc).map(|i| *args.get(i).ptr).collect()
    } else {
        Vec::new()
    };

    let global = CurrentGlobalOrNull(cx);
    if global.is_null() { set_state(cx, this_obj, state); args.rval().set(BooleanValue(had_listeners)); return true; }
    let global_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };

    let call_args = if emit_args.is_empty() {
        HandleValueArray::empty()
    } else {
        HandleValueArray { length_: emit_args.len(), elements_: emit_args.as_ptr() }
    };

    let mut remaining_listeners: Vec<*mut JSObject> = Vec::new();
    let mut remaining_once: Vec<bool> = Vec::new();

    for (i, &callback) in listeners.iter().enumerate() {
        let cb_val = ObjectValue(callback);
        let cb_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &cb_val };
        let mut rval = UndefinedValue();
        let rval_h = MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut rval };
        JS_CallFunctionValue(cx, global_h, cb_h, &call_args, rval_h);
        JS_ClearPendingException(cx);

        let is_once = once_flags.get(i).copied().unwrap_or(false);
        if !is_once {
            remaining_listeners.push(callback);
            remaining_once.push(false);
        }
    }

    state.listeners.insert(event_name.clone(), remaining_listeners);
    state.once_flags.insert(event_name, remaining_once);
    set_state(cx, this_obj, state);

    args.rval().set(BooleanValue(had_listeners));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ee_prepend(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() { args.rval().set(UndefinedValue()); return true; }
    let this_obj = this.to_object();

    let event_name = match get_event_name(cx, &args) {
        Some(n) => n, None => { args.rval().set(ObjectValue(this_obj)); return true; }
    };
    let callback = match get_callback(&args) {
        Some(cb) => cb, None => { args.rval().set(ObjectValue(this_obj)); return true; }
    };

    let mut state = ensure_state(cx, this_obj);
    state.listeners.entry(event_name.clone()).or_default().insert(0, callback);
    state.once_flags.entry(event_name).or_default().insert(0, false);
    set_state(cx, this_obj, state);
    args.rval().set(ObjectValue(this_obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ee_prepend_once(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() { args.rval().set(UndefinedValue()); return true; }
    let this_obj = this.to_object();

    let event_name = match get_event_name(cx, &args) {
        Some(n) => n, None => { args.rval().set(ObjectValue(this_obj)); return true; }
    };
    let callback = match get_callback(&args) {
        Some(cb) => cb, None => { args.rval().set(ObjectValue(this_obj)); return true; }
    };

    let mut state = ensure_state(cx, this_obj);
    state.listeners.entry(event_name.clone()).or_default().insert(0, callback);
    state.once_flags.entry(event_name).or_default().insert(0, true);
    set_state(cx, this_obj, state);
    args.rval().set(ObjectValue(this_obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ee_remove_all(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() { args.rval().set(UndefinedValue()); return true; }
    let this_obj = this.to_object();

    let mut state = ensure_state(cx, this_obj);
    if argc > 0 {
        if let Some(event_name) = get_event_name(cx, &args) {
            state.listeners.remove(&event_name);
            state.once_flags.remove(&event_name);
        }
    } else {
        state.listeners.clear();
        state.once_flags.clear();
    }
    set_state(cx, this_obj, state);
    args.rval().set(ObjectValue(this_obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ee_listeners(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() { args.rval().set(UndefinedValue()); return true; }
    let this_obj = this.to_object();

    let event_name = match get_event_name(cx, &args) {
        Some(n) => n,
        None => {
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            rooted!(&in(wrapped_cx) let arr = mozjs::rust::wrappers2::NewArrayObject1(&mut wrapped_cx, 0));
            args.rval().set(ObjectValue(arr.get()));
            return true;
        }
    };

    let state = ensure_state(cx, this_obj);
    let listeners = state.listeners.get(&event_name).cloned().unwrap_or_default();
    set_state(cx, this_obj, state);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let arr = mozjs::rust::wrappers2::NewArrayObject1(cx_ref, listeners.len()));
    for (i, &cb) in listeners.iter().enumerate() {
        let val = ObjectValue(cb);
        rooted!(&in(cx_ref) let v = val);
        JS_DefineElement(cx, arr.handle().into(), i as u32, v.handle().into(), JSPROP_ENUMERATE as u32);
    }
    args.rval().set(ObjectValue(arr.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ee_listener_count(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() { args.rval().set(Int32Value(0)); return true; }
    let this_obj = this.to_object();

    let event_name = match get_event_name(cx, &args) {
        Some(n) => n, None => { args.rval().set(Int32Value(0)); return true; }
    };

    let state = ensure_state(cx, this_obj);
    let count = state.listeners.get(&event_name).map(|l| l.len()).unwrap_or(0) as i32;
    set_state(cx, this_obj, state);
    args.rval().set(Int32Value(count));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ee_set_max(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() { args.rval().set(UndefinedValue()); return true; }
    let this_obj = this.to_object();

    let n = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_int32() { v.to_int32().max(0) as u32 }
        else if v.is_double() { v.to_double().max(0.0) as u32 }
        else { 10 }
    } else { 10 };

    let mut state = ensure_state(cx, this_obj);
    state.max_listeners = n;
    set_state(cx, this_obj, state);
    args.rval().set(ObjectValue(this_obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ee_get_max(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() { args.rval().set(Int32Value(10)); return true; }
    let this_obj = this.to_object();

    let state = ensure_state(cx, this_obj);
    let max = state.max_listeners as i32;
    set_state(cx, this_obj, state);
    args.rval().set(Int32Value(max));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ee_event_names(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() { args.rval().set(UndefinedValue()); return true; }
    let this_obj = this.to_object();

    let state = ensure_state(cx, this_obj);
    let names: Vec<String> = state.listeners.keys().cloned().collect();
    set_state(cx, this_obj, state);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let arr = mozjs::rust::wrappers2::NewArrayObject1(cx_ref, names.len()));
    for (i, name) in names.iter().enumerate() {
        let c_name = bun_core::ZBox::from_bytes(name.as_bytes());
        let js_str = JS_NewStringCopyZ(cx, c_name.as_ptr());
        if !js_str.is_null() {
            let val = mozjs::jsval::StringValue(&*js_str);
            rooted!(&in(cx_ref) let v = val);
            JS_DefineElement(cx, arr.handle().into(), i as u32, v.handle().into(), JSPROP_ENUMERATE as u32);
        }
    }
    args.rval().set(ObjectValue(arr.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn events_static_on(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 3 || !(*args.get(0).ptr).is_object() { args.rval().set(UndefinedValue()); return true; }
    let emitter = (*args.get(0).ptr).to_object();
    let emitter_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &emitter };
    let callback = *args.get(2).ptr;
    let on_name = c"on".as_ptr();
    let mut on_fn = UndefinedValue();
    JS_GetProperty(cx, emitter_h, on_name, MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData, ptr: &mut on_fn,
    });
    if on_fn.is_object() {
        let evt_val = *args.get(1).ptr;
        let call_args = HandleValueArray { length_: 2, elements_: &[evt_val, callback] as *const JSVal };
        let global = CurrentGlobalOrNull(cx);
        if !global.is_null() {
            let global_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };
            let on_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &on_fn };
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(cx, global_h, on_h, &call_args, MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData, ptr: &mut rval,
            });
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(emitter));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn events_static_once(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 3 || !(*args.get(0).ptr).is_object() { args.rval().set(UndefinedValue()); return true; }
    let emitter = (*args.get(0).ptr).to_object();
    let emitter_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &emitter };
    let callback = *args.get(2).ptr;
    let once_name = c"once".as_ptr();
    let mut once_fn = UndefinedValue();
    JS_GetProperty(cx, emitter_h, once_name, MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData, ptr: &mut once_fn,
    });
    if once_fn.is_object() {
        let evt_val = *args.get(1).ptr;
        let call_args = HandleValueArray { length_: 2, elements_: &[evt_val, callback] as *const JSVal };
        let global = CurrentGlobalOrNull(cx);
        if !global.is_null() {
            let global_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };
            let once_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &once_fn };
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(cx, global_h, once_h, &call_args, MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData, ptr: &mut rval,
            });
        }
    }
    args.rval().set(mozjs::jsval::ObjectValue(emitter));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn events_static_listener_count(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 || !(*args.get(0).ptr).is_object() { args.rval().set(Int32Value(0)); return true; }
    let emitter = (*args.get(0).ptr).to_object();

    let event_name = if (*args.get(1).ptr).is_string() {
        crate::js_to_rust_string(cx, *args.get(1).ptr)
    } else {
        args.rval().set(Int32Value(0));
        return true;
    };

    let state = ensure_state(cx, emitter);
    let count = state.listeners.get(&event_name).map(|l| l.len()).unwrap_or(0) as i32;
    set_state(cx, emitter, state);
    args.rval().set(Int32Value(count));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn events_static_get_event_listeners(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 || !(*args.get(0).ptr).is_object() {
        let arr = mozjs::jsapi::NewArrayObject1(cx, 0);
        args.rval().set(if arr.is_null() { UndefinedValue() } else { ObjectValue(arr) });
        return true;
    }
    let emitter = (*args.get(0).ptr).to_object();

    let event_name = if (*args.get(1).ptr).is_string() {
        crate::js_to_rust_string(cx, *args.get(1).ptr)
    } else {
        let arr = mozjs::jsapi::NewArrayObject1(cx, 0);
        args.rval().set(if arr.is_null() { UndefinedValue() } else { ObjectValue(arr) });
        return true;
    };

    let state = ensure_state(cx, emitter);
    let listeners = state.listeners.get(&event_name).cloned().unwrap_or_default();
    set_state(cx, emitter, state);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let arr = mozjs::rust::wrappers2::NewArrayObject1(cx_ref, listeners.len()));
    for (i, &cb) in listeners.iter().enumerate() {
        let val = ObjectValue(cb);
        rooted!(&in(cx_ref) let v = val);
        JS_DefineElement(cx, arr.handle().into(), i as u32, v.handle().into(), JSPROP_ENUMERATE as u32);
    }
    args.rval().set(ObjectValue(arr.get()));
    true
}

// ── Unit tests for EmitterState (pure Rust, no JSContext) ────────────────
// @trace REQ-ENG-007 [req:REQ-ENG-007] [level:unit]

#[cfg(test)]
mod tests {
    use super::*;
    use ::std::collections::HashMap;

    fn make_state() -> EmitterState {
        EmitterState {
            listeners: HashMap::new(),
            once_flags: HashMap::new(),
            max_listeners: 10,
        }
    }

    #[test]
    fn emitter_state_default_max_listeners() {
        let state = make_state();
        assert_eq!(state.max_listeners, 10);
    }

    #[test]
    fn emitter_state_listeners_empty_initially() {
        let state = make_state();
        assert!(state.listeners.is_empty());
    }

    #[test]
    fn emitter_state_once_flags_empty_initially() {
        let state = make_state();
        assert!(state.once_flags.is_empty());
    }

    #[test]
    fn emitter_state_can_add_listener() {
        let mut state = make_state();
        state.listeners.insert("data".to_string(), vec![::std::ptr::null_mut(); 3]);
        assert!(state.listeners.contains_key("data"));
        assert_eq!(state.listeners.get("data").map(|v| v.len()), Some(3));
    }

    #[test]
    fn emitter_state_can_set_once_flag() {
        let mut state = make_state();
        state.once_flags.insert("end".to_string(), vec![true, false]);
        assert!(state.once_flags.contains_key("end"));
        assert_eq!(state.once_flags.get("end").map(|v| v.len()), Some(2));
    }

    #[test]
    fn emitter_state_max_listeners_mutable() {
        let mut state = make_state();
        state.max_listeners = 100;
        assert_eq!(state.max_listeners, 100);
    }

    #[test]
    fn emitter_state_multiple_events() {
        let mut state = make_state();
        state.listeners.insert("data".to_string(), vec![]);
        state.listeners.insert("end".to_string(), vec![]);
        state.listeners.insert("error".to_string(), vec![]);
        assert_eq!(state.listeners.len(), 3);
    }

    #[test]
    fn emitter_state_remove_event() {
        let mut state = make_state();
        state.listeners.insert("data".to_string(), vec![]);
        state.listeners.remove("data");
        assert!(!state.listeners.contains_key("data"));
    }

    #[test]
    fn emitter_state_clear_all() {
        let mut state = make_state();
        state.listeners.insert("a".to_string(), vec![]);
        state.listeners.insert("b".to_string(), vec![]);
        state.listeners.clear();
        assert!(state.listeners.is_empty());
    }

    #[test]
    fn state_prop_is_null_prefixed() {
        assert!(STATE_PROP.starts_with(b"\x00"));
    }

    #[test]
    fn slot_state_constant() {
        assert_eq!(SLOT_STATE, 0);
    }

    #[test]
    fn emitter_class_name() {
        let name = unsafe { bun_core::ZStr::from_c_ptr(EMITTER_CLASS.name) };
        assert_eq!(name.to_str().unwrap(), "EventEmitter");
    }

    #[test]
    fn emitter_state_listener_count_for_event() {
        let mut state = make_state();
        state.listeners.insert("data".to_string(), vec![::std::ptr::null_mut(); 5]);
        let count = state.listeners.get("data").map(|v| v.len()).unwrap_or(0);
        assert_eq!(count, 5);
    }

    #[test]
    fn emitter_state_once_flag_tracking() {
        let mut state = make_state();
        state.once_flags.insert("data".to_string(), vec![true, false]);
        let flags = state.once_flags.get("data").unwrap();
        assert!(flags[0]);
        assert!(!flags[1]);
    }

    #[test]
    fn emitter_state_clone_listener_vec() {
        let mut state = make_state();
        let listeners = vec![::std::ptr::null_mut(); 3];
        state.listeners.insert("data".to_string(), listeners.clone());
        let cloned = state.listeners.get("data").unwrap().clone();
        assert_eq!(cloned.len(), listeners.len());
    }
}
