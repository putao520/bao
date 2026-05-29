use ::std::cell::RefCell;
use ::std::collections::HashMap;
use ::std::ffi::CString;
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

const SLOT_STATE: u32 = 0;

static EMITTER_CLASS: JSClass = JSClass {
    name: b"EventEmitter\0".as_ptr() as *const ::std::os::raw::c_char,
    flags: (1 << JSCLASS_RESERVED_SLOTS_SHIFT) as u32,
    cOps: ::std::ptr::null(),
    spec: ::std::ptr::null(),
    ext: ::std::ptr::null(),
    oOps: ::std::ptr::null(),
};

thread_local! {
    static EMITTER_PROTO: RefCell<*mut JSObject> = RefCell::new(::std::ptr::null_mut());
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
    }

    cache_builtin("events", events_obj.get());
}

const METHODS: &[JSFunctionSpec] = &[
    JSFunctionSpec {
        name: JSPropertySpec_Name { string_: b"on\0".as_ptr() as *const ::std::os::raw::c_char },
        call: JSNativeWrapper { op: Some(ee_on), info: ::std::ptr::null_mut() },
        nargs: 2,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name { string_: b"addEventListener\0".as_ptr() as *const ::std::os::raw::c_char },
        call: JSNativeWrapper { op: Some(ee_on), info: ::std::ptr::null_mut() },
        nargs: 2,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name { string_: b"off\0".as_ptr() as *const ::std::os::raw::c_char },
        call: JSNativeWrapper { op: Some(ee_off), info: ::std::ptr::null_mut() },
        nargs: 2,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name { string_: b"removeListener\0".as_ptr() as *const ::std::os::raw::c_char },
        call: JSNativeWrapper { op: Some(ee_off), info: ::std::ptr::null_mut() },
        nargs: 2,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name { string_: b"removeEventListener\0".as_ptr() as *const ::std::os::raw::c_char },
        call: JSNativeWrapper { op: Some(ee_off), info: ::std::ptr::null_mut() },
        nargs: 2,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name { string_: b"emit\0".as_ptr() as *const ::std::os::raw::c_char },
        call: JSNativeWrapper { op: Some(ee_emit), info: ::std::ptr::null_mut() },
        nargs: 1,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name { string_: b"once\0".as_ptr() as *const ::std::os::raw::c_char },
        call: JSNativeWrapper { op: Some(ee_once), info: ::std::ptr::null_mut() },
        nargs: 2,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name { string_: b"prependListener\0".as_ptr() as *const ::std::os::raw::c_char },
        call: JSNativeWrapper { op: Some(ee_prepend), info: ::std::ptr::null_mut() },
        nargs: 2,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name { string_: b"prependOnceListener\0".as_ptr() as *const ::std::os::raw::c_char },
        call: JSNativeWrapper { op: Some(ee_prepend_once), info: ::std::ptr::null_mut() },
        nargs: 2,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name { string_: b"removeAllListeners\0".as_ptr() as *const ::std::os::raw::c_char },
        call: JSNativeWrapper { op: Some(ee_remove_all), info: ::std::ptr::null_mut() },
        nargs: 0,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name { string_: b"listeners\0".as_ptr() as *const ::std::os::raw::c_char },
        call: JSNativeWrapper { op: Some(ee_listeners), info: ::std::ptr::null_mut() },
        nargs: 1,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name { string_: b"listenerCount\0".as_ptr() as *const ::std::os::raw::c_char },
        call: JSNativeWrapper { op: Some(ee_listener_count), info: ::std::ptr::null_mut() },
        nargs: 1,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name { string_: b"setMaxListeners\0".as_ptr() as *const ::std::os::raw::c_char },
        call: JSNativeWrapper { op: Some(ee_set_max), info: ::std::ptr::null_mut() },
        nargs: 1,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name { string_: b"getMaxListeners\0".as_ptr() as *const ::std::os::raw::c_char },
        call: JSNativeWrapper { op: Some(ee_get_max), info: ::std::ptr::null_mut() },
        nargs: 0,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name { string_: b"rawListeners\0".as_ptr() as *const ::std::os::raw::c_char },
        call: JSNativeWrapper { op: Some(ee_listeners), info: ::std::ptr::null_mut() },
        nargs: 1,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name { string_: b"eventNames\0".as_ptr() as *const ::std::os::raw::c_char },
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

fn get_state(obj: *mut JSObject) -> Option<Box<EmitterState>> {
    unsafe {
        let mut slot = UndefinedValue();
        JS_GetReservedSlot(obj, SLOT_STATE, &mut slot);
        if slot.is_double() {
            let ptr = slot.to_private() as *mut EmitterState;
            if !ptr.is_null() {
                return Some(Box::from_raw(ptr));
            }
        }
    }
    None
}

fn set_state(obj: *mut JSObject, state: Box<EmitterState>) {
    unsafe {
        let val = PrivateValue(Box::into_raw(state) as *const ::std::os::raw::c_void);
        JS_SetReservedSlot(obj, SLOT_STATE, &val);
    }
}

fn ensure_state(obj: *mut JSObject) -> Box<EmitterState> {
    get_state(obj).unwrap_or_else(|| {
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
            return Some(jsstr_to_string(cx, NonNull::new(s).unwrap()));
        }
    }
    if val.is_int32() {
        return Some(val.to_int32().to_string());
    }
    None
}}

unsafe fn get_callback(args: &CallArgs) -> Option<*mut JSObject> { unsafe {
    if args.argc_ < 2 { return None; }
    let val = *args.get(1).ptr;
    if val.is_object() { Some(val.to_object()) } else { None }
}}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn event_emitter_constructor(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let this = JS_NewObjectForConstructor(cx, &EMITTER_CLASS, &args);
    if this.is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }

    let state = Box::new(EmitterState {
        listeners: HashMap::new(),
        once_flags: HashMap::new(),
        max_listeners: 10,
    });
    set_state(this, state);

    args.rval().set(ObjectValue(this));
    true
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
    let callback = match get_callback(&args) {
        Some(cb) => cb,
        None => { args.rval().set(ObjectValue(this_obj)); return true; }
    };

    let mut state = ensure_state(this_obj);
    state.listeners.entry(event_name).or_default().push(callback);
    set_state(this_obj, state);
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

    let mut state = ensure_state(this_obj);
    state.listeners.entry(event_name.clone()).or_default().push(callback);
    state.once_flags.entry(event_name).or_default().push(true);
    set_state(this_obj, state);
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

    let mut state = ensure_state(this_obj);

    if argc < 2 {
        state.listeners.remove(&event_name);
        state.once_flags.remove(&event_name);
        set_state(this_obj, state);
        args.rval().set(ObjectValue(this_obj));
        return true;
    }

    let val = *args.get(1).ptr;
    if !val.is_object() { args.rval().set(ObjectValue(this_obj)); return true; }
    let callback = val.to_object();

    if let Some(listeners) = state.listeners.get_mut(&event_name) {
        if let Some(once_flags) = state.once_flags.get_mut(&event_name) {
            let mut i = 0;
            while i < listeners.len() {
                if listeners[i] == callback {
                    listeners.remove(i);
                    if i < once_flags.len() { once_flags.remove(i); }
                } else {
                    i += 1;
                }
            }
        }
    }

    set_state(this_obj, state);
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

    let mut state = ensure_state(this_obj);
    let listeners = match state.listeners.get(&event_name) {
        Some(l) => l.clone(),
        None => { set_state(this_obj, state); args.rval().set(BooleanValue(false)); return true; }
    };
    let once_flags = state.once_flags.get(&event_name).cloned().unwrap_or_default();
    let had_listeners = !listeners.is_empty();

    let emit_args: Vec<JSVal> = if argc > 1 {
        (1..argc).map(|i| *args.get(i).ptr).collect()
    } else {
        Vec::new()
    };

    let global = CurrentGlobalOrNull(cx);
    if global.is_null() { set_state(this_obj, state); args.rval().set(BooleanValue(had_listeners)); return true; }
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
    set_state(this_obj, state);

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

    let mut state = ensure_state(this_obj);
    state.listeners.entry(event_name.clone()).or_default().insert(0, callback);
    state.once_flags.entry(event_name).or_default().insert(0, false);
    set_state(this_obj, state);
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

    let mut state = ensure_state(this_obj);
    state.listeners.entry(event_name.clone()).or_default().insert(0, callback);
    state.once_flags.entry(event_name).or_default().insert(0, true);
    set_state(this_obj, state);
    args.rval().set(ObjectValue(this_obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ee_remove_all(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() { args.rval().set(UndefinedValue()); return true; }
    let this_obj = this.to_object();

    let mut state = ensure_state(this_obj);
    if argc > 0 {
        if let Some(event_name) = get_event_name(cx, &args) {
            state.listeners.remove(&event_name);
            state.once_flags.remove(&event_name);
        }
    } else {
        state.listeners.clear();
        state.once_flags.clear();
    }
    set_state(this_obj, state);
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

    let state = ensure_state(this_obj);
    let listeners = state.listeners.get(&event_name).cloned().unwrap_or_default();
    set_state(this_obj, state);

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

    let state = ensure_state(this_obj);
    let count = state.listeners.get(&event_name).map(|l| l.len()).unwrap_or(0) as i32;
    set_state(this_obj, state);
    args.rval().set(Int32Value(count));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ee_set_max(_cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
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

    let mut state = ensure_state(this_obj);
    state.max_listeners = n;
    set_state(this_obj, state);
    args.rval().set(ObjectValue(this_obj));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ee_get_max(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() { args.rval().set(Int32Value(10)); return true; }
    let this_obj = this.to_object();

    let state = ensure_state(this_obj);
    let max = state.max_listeners as i32;
    set_state(this_obj, state);
    args.rval().set(Int32Value(max));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ee_event_names(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() { args.rval().set(UndefinedValue()); return true; }
    let this_obj = this.to_object();

    let state = ensure_state(this_obj);
    let names: Vec<String> = state.listeners.keys().cloned().collect();
    set_state(this_obj, state);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let arr = mozjs::rust::wrappers2::NewArrayObject1(cx_ref, names.len()));
    for (i, name) in names.iter().enumerate() {
        let Ok(c_name) = CString::new(name.as_str()) else { continue };
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
    let on_name = CString::new("on").unwrap_or_default();
    let mut on_fn = UndefinedValue();
    JS_GetProperty(cx, emitter_h, on_name.as_ptr(), MutableHandle::<Value> {
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
    let once_name = CString::new("once").unwrap_or_default();
    let mut once_fn = UndefinedValue();
    JS_GetProperty(cx, emitter_h, once_name.as_ptr(), MutableHandle::<Value> {
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
    let emitter_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &emitter };
    let count_name = CString::new("listenerCount").unwrap_or_default();
    let mut count_fn = UndefinedValue();
    JS_GetProperty(cx, emitter_h, count_name.as_ptr(), MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData, ptr: &mut count_fn,
    });
    if count_fn.is_object() {
        let evt_val = *args.get(1).ptr;
        let call_args = HandleValueArray { length_: 1, elements_: &evt_val as *const JSVal };
        let global = CurrentGlobalOrNull(cx);
        if !global.is_null() {
            let global_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };
            let count_h = Handle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &count_fn };
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(cx, global_h, count_h, &call_args, MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData, ptr: &mut rval,
            });
            args.rval().set(rval);
            return true;
        }
    }
    args.rval().set(Int32Value(0));
    true
}
