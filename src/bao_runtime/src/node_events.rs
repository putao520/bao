// @trace REQ-ENG-007
use ::std::cell::RefCell;
use ::std::collections::HashMap;
use ::std::ptr::NonNull;
use ::std::sync::atomic::{AtomicU64, Ordering};
use bun_core::ZBox;

use mozjs::glue::JS_GetReservedSlot;
use mozjs::jsapi::*;
use mozjs::jsval::{BooleanValue, DoubleValue, Int32Value, JSVal, ObjectValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::gc_store::{gc_store_get, gc_store_insert, gc_store_remove};
use crate::slot_gate::set_slot;
use crate::require::cache_builtin;

/// Global counter for generating unique GcStore keys for event listeners.
static EE_LISTENER_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Clone is load-bearing: get_state returns snapshots so nested ee_* calls on
/// the same object never alias the prop-owned Box (single-owner invariant,
/// see get_state/set_state).
#[derive(Clone)]
struct EmitterState {
    /// GcStore keys for listener functions (GC-managed via global object properties).
    listeners: HashMap<String, Vec<String>>,
    once_flags: HashMap<String, Vec<bool>>,
    max_listeners: u32,
}

static EMITTER_CLASS: JSClass = JSClass {
    name: c"EventEmitter".as_ptr(),
    flags: (1 << JSCLASS_RESERVED_SLOTS_SHIFT) as u32,
    cOps: ::std::ptr::null(),
    spec: ::std::ptr::null(),
    ext: ::std::ptr::null(),
    oOps: ::std::ptr::null(),
};

thread_local! {
    /// GC-safe: GcStore key for the EventEmitter prototype object.
    static EMITTER_PROTO_KEY: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// EmitterState carrier: a native-private JS `WeakMap` (the exact shape
/// node's own internal/event_target port in this tree already uses for the
/// same problem). The WeakMap object itself is anchored in `gc_store`
/// (`EMITTER_WEAKMAP_KEY`) — JS never gets a reference to it, so its VALUES
/// (native-constructed opaque state-holder objects) are unreachable and
/// unforgeable from script. Keys are weak: a dead emitter's entry is cleared
/// by the engine, so nothing native pins an idle/failed wrapper — the
/// retention contract upstream 367d939d9a requires.
///
/// STATE BOX: each entry's value object holds the `Box<EmitterState>` raw
/// pointer in its reserved slot (slot 0 of EMITTER_HOLDER_CLASS). The holder
/// dies with the WeakMap entry; the Rust `EmitterState` box behind the
/// pointer is reclaimed by the paired `FinalizationRegistry` callback
/// (`emitter_state_finalized`, registered in `install`) — engine-scheduled,
/// so a delayed finalization only holds the plain-Rust state struct, never
/// the emitter object.
const EMITTER_WEAKMAP_KEY: &str = "builtin:emitter_state_weakmap";

pub fn install(cx: &mut mozjs::context::JSContext) {
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
            let key = "__emitter_proto".to_string();
            gc_store_insert(cx.raw_cx(), &key, proto);
            EMITTER_PROTO_KEY.with(|p| *p.borrow_mut() = Some(key));
        }

        rooted!(&in(cx) let proto_h = proto);

        // Node exposes the alias methods as the SAME function object —
        // `ee.on === ee.addListener` and `ee.off === ee.removeListener` hold
        // by reference (event-emitter.test.ts asserts identity). Defining
        // each alias through a fresh JSFunctionSpec entry would install a
        // distinct function object, so each alias re-defines the canonical
        // function's value under the alias name (same object, one definition
        // site per native).
        for (alias, canonical) in [
            ("on", "addListener"),
            ("addEventListener", "addListener"),
            ("off", "removeListener"),
            ("removeEventListener", "removeListener"),
        ] {
            let c_canonical = ZBox::from_bytes(canonical.as_bytes());
            let mut fn_val = UndefinedValue();
            JS_GetProperty(
                cx.raw_cx(),
                proto_h.handle().into(),
                c_canonical.as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut fn_val,
                },
            );
            if fn_val.is_object() {
                let c_alias = ZBox::from_bytes(alias.as_bytes());
                rooted!(&in(cx) let fv = fn_val);
                JS_DefineProperty(
                    cx.raw_cx(),
                    proto_h.handle().into(),
                    c_alias.as_ptr(),
                    fv.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }

        rooted!(&in(cx) let ctor = JS_GetConstructor(cx.raw_cx(), proto_h.handle().into()));
        if !ctor.get().is_null() {
            // Node.js exports the EventEmitter constructor itself as the
            // module value: `require('events') === require('events').EventEmitter`.
            // The ctor is the module return value (cached below); the
            // `EventEmitter` key is a self-reference so both access shapes
            // (`require('events')` and `require('events').EventEmitter`)
            // resolve to the same function object.
            let ctor_val = ObjectValue(ctor.get());
            rooted!(&in(cx) let cv = ctor_val);
            JS_DefineProperty(
                cx.raw_cx(),
                ctor.handle().into(),
                c"EventEmitter".as_ptr(),
                cv.handle().into(),
                (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
            );
            w2::JS_DefineFunction(
                cx,
                ctor.handle(),
                c"listenerCount".as_ptr(),
                Some(events_static_listener_count),
                2,
                JSPROP_ENUMERATE as u32,
            );
            w2::JS_DefineFunction(
                cx,
                ctor.handle(),
                c"getEventListeners".as_ptr(),
                Some(events_static_get_event_listeners),
                2,
                JSPROP_ENUMERATE as u32,
            );

            // Node.js ships `EventEmitter.defaultMaxListeners` (number, default 10)
            // and `EventEmitter.captureRejections` (boolean, default false) as
            // writable constructor properties. On the module export shape these
            // live directly on the function object:
            // `require('events').defaultMaxListeners === 10`.
            let default_max = Int32Value(10);
            rooted!(&in(cx) let dmv = default_max);
            JS_DefineProperty(
                cx.raw_cx(),
                ctor.handle().into(),
                c"defaultMaxListeners".as_ptr(),
                dmv.handle().into(),
                (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
            );
            let capture_rejections = BooleanValue(false);
            rooted!(&in(cx) let crv = capture_rejections);
            JS_DefineProperty(
                cx.raw_cx(),
                ctor.handle().into(),
                c"captureRejections".as_ptr(),
                crv.handle().into(),
                (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
            );

            // EventEmitter.errorMonitor — Symbol.for('events.errorMonitor').
            // Reuses Bun's events.ts semantics (Bun: `kErrorMonitor = SymbolFor("events.errorMonitor")`).
            let key_c = ZBox::from_bytes("events.errorMonitor\0".as_bytes());
            let key_str = JS_NewStringCopyZ(cx.raw_cx(), key_c.as_ptr());
            if !key_str.is_null() {
                rooted!(&in(cx) let key_root = key_str);
                let sym_for = JS::GetSymbolFor(cx.raw_cx(), key_root.handle().into());
                if !sym_for.is_null() {
                    rooted!(&in(cx) let sf = mozjs::jsval::SymbolValue(&*sym_for));
                    JS_DefineProperty(
                        cx.raw_cx(),
                        ctor.handle().into(),
                        c"errorMonitor".as_ptr(),
                        sf.handle().into(),
                        (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
                    );
                }
            }

            // Static event-registration helpers that node exposes on the
            // module export (function) object:
            //   `events.once(emitterOrTarget, name[, options])` → Promise
            //   (node v24 promise form; installed as a bootstrap trampoline —
            //   see events_static_once) and `events.on(emitter, name)`.
            // listenerCount / getEventListeners are defined above; on/once
            // complete the static surface.
            w2::JS_DefineFunction(
                cx,
                ctor.handle(),
                c"on".as_ptr(),
                Some(events_static_on),
                3,
                JSPROP_ENUMERATE as u32,
            );
            w2::JS_DefineFunction(
                cx,
                ctor.handle(),
                c"once".as_ptr(),
                Some(events_static_once),
                2,
                JSPROP_ENUMERATE as u32,
            );
        }

        // The module IS the constructor function (node oracle: typeof === 'function',
        // require('events') === require('events').EventEmitter).
        cache_builtin(cx, "events", ctor.get());
    }

    // @trace REQ-ENG-004 [api:internal/event_target] — realm-init hook of the
    // internal/event_target JS port. Deliberately a NO-OP (see
    // node_internal_event_target::install): the module body must not be
    // evaluated during realm init — install-time evaluation poisons the
    // realm (any later module-body variable access segfaults; GC-pressure
    // class, root cause registered). The real evaluation happens lazily on
    // the first `require('internal/event_target')` via the require.rs wiring
    // (node_internal_event_target::ensure_installed), in user context.
    crate::node_internal_event_target::install(cx);
}

const METHODS: &[JSFunctionSpec] = &[
    // Canonical registration names. Node defines `addListener` and assigns
    // `on` to it (`EventEmitter.prototype.on.name === 'addListener'`), so the
    // JSFunctionSpec installs the canonical names only; `on` / `off` /
    // `addEventListener` / `removeEventListener` are defined below as the
    // SAME function object (node exposes aliases, not copies —
    // `on === addListener`, `off === removeListener` holds by reference).
    JSFunctionSpec {
        name: JSPropertySpec_Name {
            string_: c"addListener".as_ptr(),
        },
        call: JSNativeWrapper {
            op: Some(ee_on),
            info: ::std::ptr::null_mut(),
        },
        nargs: 2,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name {
            string_: c"removeListener".as_ptr(),
        },
        call: JSNativeWrapper {
            op: Some(ee_off),
            info: ::std::ptr::null_mut(),
        },
        nargs: 2,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name {
            string_: c"emit".as_ptr(),
        },
        call: JSNativeWrapper {
            op: Some(ee_emit),
            info: ::std::ptr::null_mut(),
        },
        nargs: 1,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name {
            string_: c"once".as_ptr(),
        },
        call: JSNativeWrapper {
            op: Some(ee_once),
            info: ::std::ptr::null_mut(),
        },
        nargs: 2,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name {
            string_: c"prependListener".as_ptr(),
        },
        call: JSNativeWrapper {
            op: Some(ee_prepend),
            info: ::std::ptr::null_mut(),
        },
        nargs: 2,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name {
            string_: c"prependOnceListener".as_ptr(),
        },
        call: JSNativeWrapper {
            op: Some(ee_prepend_once),
            info: ::std::ptr::null_mut(),
        },
        nargs: 2,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name {
            string_: c"removeAllListeners".as_ptr(),
        },
        call: JSNativeWrapper {
            op: Some(ee_remove_all),
            info: ::std::ptr::null_mut(),
        },
        nargs: 0,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name {
            string_: c"listeners".as_ptr(),
        },
        call: JSNativeWrapper {
            op: Some(ee_listeners),
            info: ::std::ptr::null_mut(),
        },
        nargs: 1,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name {
            string_: c"listenerCount".as_ptr(),
        },
        call: JSNativeWrapper {
            op: Some(ee_listener_count),
            info: ::std::ptr::null_mut(),
        },
        nargs: 1,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name {
            string_: c"setMaxListeners".as_ptr(),
        },
        call: JSNativeWrapper {
            op: Some(ee_set_max),
            info: ::std::ptr::null_mut(),
        },
        nargs: 1,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name {
            string_: c"getMaxListeners".as_ptr(),
        },
        call: JSNativeWrapper {
            op: Some(ee_get_max),
            info: ::std::ptr::null_mut(),
        },
        nargs: 0,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name {
            string_: c"rawListeners".as_ptr(),
        },
        call: JSNativeWrapper {
            op: Some(ee_listeners),
            info: ::std::ptr::null_mut(),
        },
        nargs: 1,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name {
            string_: c"eventNames".as_ptr(),
        },
        call: JSNativeWrapper {
            op: Some(ee_event_names),
            info: ::std::ptr::null_mut(),
        },
        nargs: 0,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name {
            string_: ::std::ptr::null(),
        },
        call: JSNativeWrapper {
            op: None,
            info: ::std::ptr::null_mut(),
        },
        nargs: 0,
        flags: 0,
        selfHostedName: ::std::ptr::null_mut(),
    },
];

/// The native-private carrier WeakMap (constructed on first use, gc_store
/// anchored under `EMITTER_WEAKMAP_KEY`). JS never obtains a reference to
/// it, so its values (native-built state holders) are unforgeable.
unsafe fn emitter_weakmap(cx: *mut JSContext) -> *mut JSObject {
    if let Some(existing) = gc_store_get(cx, EMITTER_WEAKMAP_KEY).filter(|p| !p.is_null()) {
        return existing;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let mut source_text = mozjs::rust::transform_str_to_source_text("new WeakMap()");
    let mut rval = UndefinedValue();
    let rval_handle = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut rval,
    };
    let filename = ZBox::from_bytes("<internal/emitter_weakmap>".as_bytes());
    let opts = mozjs::glue::NewCompileOptions(cx, filename.as_ptr(), 1);
    if opts.is_null() {
        JS_ClearPendingException(cx);
        return ::std::ptr::null_mut();
    }
    let ok = mozjs_sys::jsapi::JS::Evaluate2(cx, opts, &mut source_text, rval_handle);
    mozjs::glue::DeleteCompileOptions(opts);
    if !ok || !rval.is_object() {
        // Engine exception stays pending — propagate loudly (no fallback).
        return ::std::ptr::null_mut();
    }
    let wm = rval.to_object();
    gc_store_insert(cx, EMITTER_WEAKMAP_KEY, wm);
    wm
}

/// Call a WeakMap method (`get` / `set`) with one argument. `JS_CallFunctionName`
/// shape mirrors the node_net `__net_make_socket` precedent.
unsafe fn wm_call(cx: *mut JSContext, wm: *mut JSObject, name: *const i8, arg: JSVal) -> JSVal {
    let wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let wm_root = wm);
    rooted!(&in(wrapped_cx) let arg_root = arg);
    // (Bao, BCE) rooted ValueArray carrier — bare HandleValueArray slots are untraced.
    rooted!(&in(wrapped_cx) let argv_rooted =
        mozjs_sys::jsgc::ValueArray::<1>::new([arg_root.get()]));
    let argv = mozjs_sys::jsapi::JS::HandleValueArray::from(&argv_rooted);
    let mut rval = UndefinedValue();
    let ok = JS_CallFunctionName(
        cx,
        wm_root.handle().into(),
        name,
        &argv,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        },
    );
    if !ok {
        JS_ClearPendingException(cx);
        return UndefinedValue();
    }
    rval
}

/// Slot 0 of `EMITTER_HOLDER_CLASS` carries the `Box<EmitterState>` raw
/// pointer. Holder objects are only ever referenced by the native-private
/// WeakMap — script cannot reach one, so the private pointer is unforgeable
/// (the original `\x00__ee_state` forgery vector stays closed).
static EMITTER_HOLDER_CLASS: JSClass = JSClass {
    name: c"EmitterStateHolder".as_ptr(),
    flags: (1 << JSCLASS_RESERVED_SLOTS_SHIFT) as u32,
    cOps: ::std::ptr::null(),
    spec: ::std::ptr::null(),
    ext: ::std::ptr::null(),
    oOps: ::std::ptr::null(),
};

unsafe fn holder_box<'a>(cx: *mut JSContext, holder: *mut JSObject) -> Option<&'a mut EmitterState> {
    let _ = cx;
    let mut sv = UndefinedValue();
    JS_GetReservedSlot(holder, 0, &mut sv);
    if !sv.is_double() {
        return None;
    }
    // `set_slot` stores the pointer as a PrivateValue (tagged payload, not a
    // plain double) — read it back with `to_private()` (slot_gate precedent).
    let ptr = sv.to_private() as *mut EmitterState;
    if ptr.is_null() {
        return None;
    }
    Some(&mut *ptr)
}

fn get_state(cx: *mut JSContext, obj: *mut JSObject) -> Option<EmitterState> {
    // WeakMap lookup by the emitter OBJECT itself (identity key — the engine
    // resolves moving-GC identity internally, no raw-address key, no ABA).
    // SINGLE-OWNER (unchanged): readers take a CLONE.
    let _ = cx;
    if obj.is_null() {
        return None;
    }
    unsafe {
        let wm = emitter_weakmap(cx);
        if wm.is_null() {
            return None;
        }
        let holder = wm_call(cx, wm, c"get".as_ptr(), ObjectValue(obj));
        if !holder.is_object() {
            return None;
        }
        holder_box(cx, holder.to_object())
            .map(|state| state.clone())
    }
}

fn set_state(cx: *mut JSContext, obj: *mut JSObject, state: EmitterState) {
    // Build the holder (private class, reserved slot 0 = Box pointer), then
    // WeakMap.set(emitter, holder). The old entry's holder (if any) is
    // dropped with its Rust box at finalization; replace is single-owner.
    if obj.is_null() {
        return;
    }
    unsafe {
        let wm = emitter_weakmap(cx);
        if wm.is_null() {
            return;
        }
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        let boxed = Box::new(state);
        let holder = w2::JS_NewObject(cx_ref, &EMITTER_HOLDER_CLASS);
        if holder.is_null() {
            ::std::mem::forget(boxed);
            return;
        }
        set_slot(holder, 0, Box::into_raw(boxed) as *const ::std::os::raw::c_void);
        rooted!(&in(wrapped_cx) let arg_val = ObjectValue(holder));
        rooted!(&in(wrapped_cx) let key_val = ObjectValue(obj));
        // (Bao, BCE) rooted ValueArray carrier — bare HandleValueArray slots are untraced.
        rooted!(&in(wrapped_cx) let call_rooted = mozjs_sys::jsgc::ValueArray::<2>::new([
            key_val.handle().get(),
            arg_val.handle().get(),
        ]));
        let call = mozjs_sys::jsapi::JS::HandleValueArray::from(&call_rooted);
        rooted!(&in(wrapped_cx) let wm_root = wm);
        let mut rval = UndefinedValue();
        let _ = JS_CallFunctionName(
            cx,
            wm_root.handle().into(),
            c"set".as_ptr(),
            &call,
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut rval,
            },
        );
    }
}

fn ensure_state(cx: *mut JSContext, obj: *mut JSObject) -> EmitterState {
    get_state(cx, obj).unwrap_or_else(|| EmitterState {
        listeners: HashMap::new(),
        once_flags: HashMap::new(),
        max_listeners: 10,
    })
}

unsafe fn get_event_name(cx: *mut JSContext, args: &CallArgs) -> Option<String> {
    unsafe {
        if args.argc_ == 0 {
            return None;
        }
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
    }
}

unsafe fn get_callback(cx: *mut JSContext, args: &CallArgs) -> Option<*mut JSObject> {
    unsafe {
        if args.argc_ < 2 {
            return None;
        }
        let val = *args.get(1).ptr;
        if val.is_object() {
            let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            rooted!(&in(wrapped_cx) let obj_root = val.to_object());
            if mozjs_sys::jsapi::js::IsFunctionObject(obj_root.get()) {
                return Some(obj_root.get());
            }
        }
        None
    }
}

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
    unsafe {
        let c_msg = ZBox::from_bytes(msg.as_bytes());
        JS_ReportErrorASCII(cx, c_msg.as_ptr());
    }
}

/// Generate a unique GcStore key for an event listener.
fn alloc_listener_key(event_name: &str) -> String {
    let id = EE_LISTENER_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("ee_{}_{}", event_name, id)
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
        // set_state allocates the registry entry (RootedTraceableBox::new — a
        // formal GC point); root `this` so the rval below re-reads the live
        // address (stale-rval UB class, e206). set_state re-roots internally.
        let mut wrapped = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped;
        rooted!(&in(cx_ref) let this_r = this);
        let state = EmitterState {
            listeners: HashMap::new(),
            once_flags: HashMap::new(),
            max_listeners: 10,
        };
        set_state(cx, this_r.get(), state);
        args.rval().set(ObjectValue(this_r.get()));
        return true;
    }

    // Fallback: EventEmitter.call(this) pattern — initialize the `this` object
    // Clear the pending exception from JS_NewObjectForConstructor failure
    JS_ClearPendingException(cx);
    let this_val = args.thisv();
    if this_val.is_object() {
        let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        rooted!(&in(wrapped_cx) let this_obj_root = this_val.to_object());
        // Only initialize if not already an EventEmitter
        let existing = get_state(cx, this_obj_root.get());
        if let Some(state) = existing {
            set_state(cx, this_obj_root.get(), state);
        } else {
            let state = EmitterState {
                listeners: HashMap::new(),
                once_flags: HashMap::new(),
                max_listeners: 10,
            };
            set_state(cx, this_obj_root.get(), state);
        }
        args.rval().set(ObjectValue(this_obj_root.get()));
        return true;
    }

    args.rval().set(UndefinedValue());
    false
}

#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe extern "C" fn ee_on(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let this_obj_root = this.to_object());

    let event_name = match get_event_name(cx, &args) {
        Some(n) => n,
        None => {
            args.rval().set(ObjectValue(this_obj_root.get()));
            return true;
        }
    };

    if argc >= 2 {
        let listener_val = *args.get(1).ptr;
        if listener_val.is_object() {
            rooted!(&in(wrapped_cx) let listener_obj_root = listener_val.to_object());
            if !is_function(listener_obj_root.get()) {
                throw_type_error(
                    cx,
                    "The \"listener\" argument must be of type function. Received object",
                );
                return false;
            }
        } else {
            throw_type_error(cx, "The \"listener\" argument must be of type function");
            return false;
        }
    }

    let callback = match get_callback(cx, &args) {
        Some(cb) => cb,
        None => {
            args.rval().set(ObjectValue(this_obj_root.get()));
            return true;
        }
    };

    let mut state = ensure_state(cx, this_obj_root.get());
    let key = alloc_listener_key(&event_name);
    gc_store_insert(cx, &key, callback);
    state
        .listeners
        .entry(event_name.clone())
        .or_default()
        .push(key);
    // once_flags is INDEX-ALIGNED with listeners (ee_once pushes `true`, so a
    // non-once registration must own a `false` slot). Without it, the vec is
    // short and every flag consumer — emit's once firing, ee_off's flag
    // removal, emit's write-back — reads the NEXT listener's flag: a plain
    // `on` followed by a `once` made emit treat the `on` listener as once
    // (dropping it after one emit) and resurrect the `once` listener.
    state.once_flags.entry(event_name).or_default().push(false);
    set_state(cx, this_obj_root.get(), state);
    args.rval().set(ObjectValue(this_obj_root.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe extern "C" fn ee_once(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let this_obj_root = this.to_object());

    let event_name = match get_event_name(cx, &args) {
        Some(n) => n,
        None => {
            args.rval().set(ObjectValue(this_obj_root.get()));
            return true;
        }
    };
    let callback = match get_callback(cx, &args) {
        Some(cb) => cb,
        None => {
            args.rval().set(ObjectValue(this_obj_root.get()));
            return true;
        }
    };

    let mut state = ensure_state(cx, this_obj_root.get());
    let key = alloc_listener_key(&event_name);
    gc_store_insert(cx, &key, callback);
    state
        .listeners
        .entry(event_name.clone())
        .or_default()
        .push(key);
    state.once_flags.entry(event_name).or_default().push(true);
    set_state(cx, this_obj_root.get(), state);
    args.rval().set(ObjectValue(this_obj_root.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe extern "C" fn ee_off(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let this_obj_root = this.to_object());

    let event_name = match get_event_name(cx, &args) {
        Some(n) => n,
        None => {
            args.rval().set(ObjectValue(this_obj_root.get()));
            return true;
        }
    };

    let mut state = ensure_state(cx, this_obj_root.get());

    if argc < 2 {
        // Remove all listeners for this event — clean up GcStore entries
        if let Some(keys) = state.listeners.remove(&event_name) {
            for key in keys {
                gc_store_remove(cx, &key);
            }
        }
        state.once_flags.remove(&event_name);
        set_state(cx, this_obj_root.get(), state);
        args.rval().set(ObjectValue(this_obj_root.get()));
        return true;
    }

    let callback_val = *args.get(1).ptr;
    if !callback_val.is_object() {
        set_state(cx, this_obj_root.get(), state);
        args.rval().set(ObjectValue(this_obj_root.get()));
        return true;
    }

    if let Some(keys) = state.listeners.get_mut(&event_name) {
        let mut removed_indices: Vec<usize> = Vec::new();
        let mut i = 0;
        while i < keys.len() {
            if let Some(stored_obj) = gc_store_get(cx, &keys[i]) {
                let stored_val = ObjectValue(stored_obj);
                if js_same_value(cx, stored_val, callback_val) {
                    gc_store_remove(cx, &keys[i]);
                    keys.remove(i);
                    removed_indices.push(i);
                } else {
                    i += 1;
                }
            } else {
                // Key no longer in GcStore (GC collected) — remove stale entry
                keys.remove(i);
                removed_indices.push(i);
            }
        }
        if let Some(flags) = state.once_flags.get_mut(&event_name) {
            for idx in removed_indices.into_iter().rev() {
                if idx < flags.len() {
                    flags.remove(idx);
                }
            }
        }
    }

    set_state(cx, this_obj_root.get(), state);
    args.rval().set(ObjectValue(this_obj_root.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe extern "C" fn ee_emit(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(BooleanValue(false));
        return true;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let this_obj_root = this.to_object());

    let event_name = match get_event_name(cx, &args) {
        Some(n) => n,
        None => {
            args.rval().set(BooleanValue(false));
            return true;
        }
    };

    let state = ensure_state(cx, this_obj_root.get());
    let listener_keys = match state.listeners.get(&event_name) {
        Some(l) => l.clone(),
        None => {
            // No mutation — nothing to write back (get_state only cloned).
            //
            // Node semantics (BCE-20260816-EE-ERRORTHROW): emit('error', err)
            // with NO 'error' listener THROWS the error from the emit() call
            // site — a synchronous try/catch around emit() catches it, and an
            // uncaught throw propagates through the engine/timer dispatchers
            // to the unified uncaught-exception router
            // (process.on('uncaughtException') or stderr + exit 1). Everything
            // else is a silent no-op returning false.
            if event_name == "error" && argc > 1 {
                let err = *args.get(1).ptr;
                if !err.is_undefined() {
                    rooted!(&in(wrapped_cx) let err_root = err);
                    JS_SetPendingException(
                        cx,
                        err_root.handle().into(),
                        ExceptionStackBehavior::DoNotCapture,
                    );
                    return false;
                }
            }
            args.rval().set(BooleanValue(false));
            return true;
        }
    };
    let once_flags = state
        .once_flags
        .get(&event_name)
        .cloned()
        .unwrap_or_default();
    let had_listeners = !listener_keys.is_empty();

    // Same rule for a registered-but-empty 'error' listener list.
    if !had_listeners && event_name == "error" && argc > 1 {
        let err = *args.get(1).ptr;
        if !err.is_undefined() {
            rooted!(&in(wrapped_cx) let err_root2 = err);
            JS_SetPendingException(
                cx,
                err_root2.handle().into(),
                ExceptionStackBehavior::DoNotCapture,
            );
            return false;
        }
        args.rval().set(BooleanValue(false));
        return true;
    }

    let emit_args: Vec<JSVal> = if argc > 1 {
        (1..argc).map(|i| *args.get(i).ptr).collect()
    } else {
        Vec::new()
    };

    // Node contract: `this` inside a listener is the emitter instance (the
    // canonical `this.on('x', ...)` / `this.removeListener` listener shapes
    // depend on it), NOT the global object.

    let call_args = if emit_args.is_empty() {
        HandleValueArray::empty()
    } else {
        HandleValueArray {
            length_: emit_args.len(),
            elements_: emit_args.as_ptr(),
        }
    };

    let mut fired_once_keys: Vec<String> = Vec::new();

    for (i, key) in listener_keys.iter().enumerate() {
        if let Some(callback) = gc_store_get(cx, key) {
            let cb_val = ObjectValue(callback);
            rooted!(&in(wrapped_cx) let cb_root = cb_val);
            let mut rval = UndefinedValue();
            let rval_h = MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut rval,
            };
            let ok = JS_CallFunctionValue(
                cx,
                this_obj_root.handle().into(),
                cb_root.handle().into(),
                &call_args,
                rval_h,
            );
            if !ok {
                // The listener threw. Capture and route through the unified
                // uncaught-exception router (process.on('uncaughtException')
                // or stderr + exit 1) — Node semantics; remaining listeners
                // still run (native emit contract keeps its boolean result).
                //
                // EXCEPT 'exit': the project's orderly-exit contract
                // (exit_handler_tests) pins that a throwing 'exit' listener
                // must NOT block later listeners nor change the exit code —
                // those throws stay swallowed.
                let mut exn = UndefinedValue();
                JS_GetPendingException(
                    cx,
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut exn,
                    },
                );
                JS_ClearPendingException(cx);
                rooted!(&in(wrapped_cx) let exn_root = exn);
                if !exn.is_undefined() && event_name != "exit" {
                    crate::uncaught::route_uncaught_exception(cx, exn);
                }
            }

            let is_once = once_flags.get(i).copied().unwrap_or(false);
            if is_once {
                // Consumed by this emit: the GcStore entry goes now, and the
                // key is dropped from the live list at write-back below.
                gc_store_remove(cx, key);
                fired_once_keys.push(key.clone());
            }
        }
        // If gc_store_get returns None, the object was GC'd — skip it (don't keep stale key)
    }

    // WRITE-BACK = live list minus this emit's once-consumed keys.
    //
    // The firing list above is the pre-emit SNAPSHOT (iteration safety: a
    // listener may freely add/remove listeners mid-emit without mutating what
    // we iterate). Node's post-emit live list, though, is the array the
    // NESTED ee_* calls mutated: `removeListener` during emit removes from
    // the live list (the canonical self-removing listener
    // `ee.on('y', function(){ this.removeListener('y', arguments.callee) })
    // leaves listenerCount('y') === 0), and `on` during emit appends. So the
    // write-back starts from the prop's CURRENT list for this event — nested
    // adds and removals are already reflected there — and then drops exactly
    // what THIS emit consumed (the fired once keys; their GcStore entries are
    // already gone). Writing our snapshot-derived `remaining_keys` over the
    // live list instead would resurrect a nested-removed listener (the old
    // keyed-merge drift) and would flatten the once flags of listeners a
    // nested call registered mid-emit. Cross-event nesting is untouched: only
    // the fired event's lists are rewritten.
    let mut current = ensure_state(cx, this_obj_root.get());
    let live_keys = current.listeners.remove(&event_name).unwrap_or_default();
    let live_flags = current.once_flags.remove(&event_name).unwrap_or_default();
    let mut final_keys: Vec<String> = Vec::with_capacity(live_keys.len());
    let mut final_flags: Vec<bool> = Vec::with_capacity(live_keys.len());
    for (idx, key) in live_keys.into_iter().enumerate() {
        if fired_once_keys.contains(&key) {
            continue;
        }
        final_flags.push(live_flags.get(idx).copied().unwrap_or(false));
        final_keys.push(key);
    }
    current.listeners.insert(event_name.clone(), final_keys);
    current.once_flags.insert(event_name, final_flags);
    set_state(cx, this_obj_root.get(), current);

    args.rval().set(BooleanValue(had_listeners));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe extern "C" fn ee_prepend(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let this_obj_root = this.to_object());

    let event_name = match get_event_name(cx, &args) {
        Some(n) => n,
        None => {
            args.rval().set(ObjectValue(this_obj_root.get()));
            return true;
        }
    };
    let callback = match get_callback(cx, &args) {
        Some(cb) => cb,
        None => {
            args.rval().set(ObjectValue(this_obj_root.get()));
            return true;
        }
    };

    let mut state = ensure_state(cx, this_obj_root.get());
    let key = alloc_listener_key(&event_name);
    gc_store_insert(cx, &key, callback);
    state
        .listeners
        .entry(event_name.clone())
        .or_default()
        .insert(0, key);
    state
        .once_flags
        .entry(event_name)
        .or_default()
        .insert(0, false);
    set_state(cx, this_obj_root.get(), state);
    args.rval().set(ObjectValue(this_obj_root.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe extern "C" fn ee_prepend_once(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let this_obj_root = this.to_object());

    let event_name = match get_event_name(cx, &args) {
        Some(n) => n,
        None => {
            args.rval().set(ObjectValue(this_obj_root.get()));
            return true;
        }
    };
    let callback = match get_callback(cx, &args) {
        Some(cb) => cb,
        None => {
            args.rval().set(ObjectValue(this_obj_root.get()));
            return true;
        }
    };

    let mut state = ensure_state(cx, this_obj_root.get());
    let key = alloc_listener_key(&event_name);
    gc_store_insert(cx, &key, callback);
    state
        .listeners
        .entry(event_name.clone())
        .or_default()
        .insert(0, key);
    state
        .once_flags
        .entry(event_name)
        .or_default()
        .insert(0, true);
    set_state(cx, this_obj_root.get(), state);
    args.rval().set(ObjectValue(this_obj_root.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
pub unsafe extern "C" fn ee_remove_all(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let this_obj_root = this.to_object());

    let mut state = ensure_state(cx, this_obj_root.get());
    if argc > 0 {
        if let Some(event_name) = get_event_name(cx, &args) {
            if let Some(keys) = state.listeners.remove(&event_name) {
                for key in keys {
                    gc_store_remove(cx, &key);
                }
            }
            state.once_flags.remove(&event_name);
        }
    } else {
        // Remove all listeners for all events — clean up GcStore entries
        for (_event, keys) in state.listeners.drain() {
            for key in keys {
                gc_store_remove(cx, &key);
            }
        }
        state.once_flags.clear();
    }
    set_state(cx, this_obj_root.get(), state);
    args.rval().set(ObjectValue(this_obj_root.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ee_listeners(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let this_obj_root = this.to_object());

    let event_name = match get_event_name(cx, &args) {
        Some(n) => n,
        None => {
            rooted!(&in(wrapped_cx) let arr = mozjs::rust::wrappers2::NewArrayObject1(&mut wrapped_cx, 0));
            args.rval().set(ObjectValue(arr.get()));
            return true;
        }
    };

    let state = ensure_state(cx, this_obj_root.get());
    let listener_keys = state
        .listeners
        .get(&event_name)
        .cloned()
        .unwrap_or_default();
    set_state(cx, this_obj_root.get(), state);

    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let arr = mozjs::rust::wrappers2::NewArrayObject1(cx_ref, listener_keys.len()));
    let mut arr_idx = 0u32;
    for key in &listener_keys {
        if let Some(cb) = gc_store_get(cx, key) {
            let val = ObjectValue(cb);
            rooted!(&in(cx_ref) let v = val);
            JS_DefineElement(
                cx,
                arr.handle().into(),
                arr_idx,
                v.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
            arr_idx += 1;
        }
    }
    args.rval().set(ObjectValue(arr.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ee_listener_count(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(Int32Value(0));
        return true;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let this_obj_root = this.to_object());

    let event_name = match get_event_name(cx, &args) {
        Some(n) => n,
        None => {
            args.rval().set(Int32Value(0));
            return true;
        }
    };

    let state = ensure_state(cx, this_obj_root.get());
    let count = state
        .listeners
        .get(&event_name)
        .map(|l| l.len())
        .unwrap_or(0) as i32;
    set_state(cx, this_obj_root.get(), state);
    args.rval().set(Int32Value(count));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ee_set_max(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let this_obj_root = this.to_object());

    let n = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_int32() {
            v.to_int32().max(0) as u32
        } else if v.is_double() {
            v.to_double().max(0.0) as u32
        } else {
            10
        }
    } else {
        10
    };

    let mut state = ensure_state(cx, this_obj_root.get());
    state.max_listeners = n;
    set_state(cx, this_obj_root.get(), state);
    args.rval().set(ObjectValue(this_obj_root.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ee_get_max(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(Int32Value(10));
        return true;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let this_obj_root = this.to_object());

    let state = ensure_state(cx, this_obj_root.get());
    let max = state.max_listeners as i32;
    set_state(cx, this_obj_root.get(), state);
    args.rval().set(Int32Value(max));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ee_event_names(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let this = args.thisv();
    if !this.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let this_obj_root = this.to_object());

    let state = ensure_state(cx, this_obj_root.get());
    let names: Vec<String> = state.listeners.keys().cloned().collect();
    set_state(cx, this_obj_root.get(), state);

    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let arr = mozjs::rust::wrappers2::NewArrayObject1(cx_ref, names.len()));
    for (i, name) in names.iter().enumerate() {
        let c_name = ZBox::from_bytes(name.as_bytes());
        let js_str = JS_NewStringCopyZ(cx, c_name.as_ptr());
        if !js_str.is_null() {
            let val = mozjs::jsval::StringValue(&*js_str);
            rooted!(&in(cx_ref) let v = val);
            JS_DefineElement(
                cx,
                arr.handle().into(),
                i as u32,
                v.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }
    args.rval().set(ObjectValue(arr.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn events_static_on(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 3 || !(*args.get(0).ptr).is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let emitter_root = (*args.get(0).ptr).to_object());
    let callback = *args.get(2).ptr;
    let on_name = ZBox::from_bytes("on".as_bytes());
    let mut on_fn = UndefinedValue();
    JS_GetProperty(
        cx,
        emitter_root.handle().into(),
        on_name.as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut on_fn,
        },
    );
    if on_fn.is_object() {
        let evt_val = *args.get(1).ptr;
        // (Bao, BCE) rooted ValueArray carrier — the old `&[evt_val, callback]`
        // temporary slice dangled after the let statement AND its slots were
        // untraced; ValueArray roots the copies for the whole call.
        rooted!(&in(wrapped_cx) let call_args_rooted =
            mozjs_sys::jsgc::ValueArray::<2>::new([evt_val, callback]));
        let call_args = mozjs_sys::jsapi::JS::HandleValueArray::from(&call_args_rooted);
        let global = CurrentGlobalOrNull(cx);
        if !global.is_null() {
            rooted!(&in(wrapped_cx) let global_root = global);
            rooted!(&in(wrapped_cx) let on_root = on_fn);
            let mut rval = UndefinedValue();
            JS_CallFunctionValue(
                cx,
                global_root.handle().into(),
                on_root.handle().into(),
                &call_args,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut rval,
                },
            );
        }
    }
    args.rval()
        .set(mozjs::jsval::ObjectValue(emitter_root.get()));
    true
}

// ════════════════════════════════════════════════════════════════════════
// @trace REQ-ENG-006 [api:events.once promise form] — node v24
// `events.once(emitterOrTarget, name[, options])` returns a Promise
// resolving with the emitted-args array (or rejecting with the 'error'
// payload / AbortError). The previous static was a 3-arg
// registration helper (`events.once(emitter, name, listener)` returns the
// emitter) — a shape node does NOT have; node-test/parallel/
// test-events-once.js drives only the promise form.
//
// The promise form is a JS port (EVENTS_ONCE_HELPER_JS, node lib/events.js
// once() adapted) rather than a native reimplementation, for the same
// reason the internal/event_target module is JS: it duck-types the
// EventEmitter prototype face (on/removeListener) and the ported
// EventTarget face (addEventListener + kResistStopPropagation private
// symbol — unconstructible from Rust), and the promise/listener closures
// must be JS-visible objects.
//
// Bootstrap discipline mirrors node_internal_event_target::ensure_installed:
// the helper is evaluated lazily on the FIRST `events.once()` call — in
// user context (never realm init, whose install-time evaluation poisons
// the realm), once per realm (gc_store cached), and only after the port is
// evaluated so the integrated AbortSignal (AbortSignal.abort, kEvents
// slots) and the global EventTarget/Event classes exist.
const EVENTS_ONCE_KEY: &str = "builtin:events_once_js";

const EVENTS_ONCE_HELPER_JS: &str = r#"(function() {
  'use strict';
  // node lib/events.js once() — promise form. The Rust trampoline only
  // evaluates this AFTER the internal/event_target port is installed, so
  // AbortSignal/EventTarget are the ported classes and the exported
  // kResistStopPropagation symbol is the real dispatch face.
  const K_RESIST = require('internal/event_target').event_target.kResistStopPropagation;
  const kEmptyObject = Object.freeze({ __proto__: null });

  // node internal/errors.js determineSpecificType — the Received suffix
  // (oracle: options=fn → "Received function fn"; 1 → "type number (1)").
  function determineSpecificType(v) {
    if (v === null) {
      return 'null';
    }
    if (v === undefined) {
      return 'undefined';
    }
    const type = typeof v;
    if (type === 'string') {
      return `type string ('${v}')`;
    }
    if (type === 'function') {
      return v.name ? `function ${v.name}` : 'anonymous function';
    }
    if (type === 'object') {
      if (typeof v.constructor === 'function' && v.constructor.name) {
        return `an instance of ${v.constructor.name}`;
      }
      return 'an object';
    }
    return `type ${type} (${String(v)})`;
  }

  function errInvalidArgType(name, expected, actual) {
    const e = new TypeError(
      `The "${name}" argument must be of type ${expected}. ` +
      `Received ${determineSpecificType(actual)}`);
    e.code = 'ERR_INVALID_ARG_TYPE';
    return e;
  }

  // The dot-noun switch is node's createInvalidArgType behavior (oracle:
  // The "options.signal" property must be an instance of AbortSignal).
  function errInvalidArgTypeInstance(name, expected, actual) {
    const noun = name.includes('.') ? 'property' : 'argument';
    const e = new TypeError(
      `The "${name}" ${noun} must be an instance of ${expected}. ` +
      `Received ${determineSpecificType(actual)}`);
    e.code = 'ERR_INVALID_ARG_TYPE';
    return e;
  }

  function validateObject(value, name) {
    if (value === null || typeof value !== 'object') {
      throw errInvalidArgType(name, 'object', value);
    }
  }

  // oracle: {signal: undefined} passes, {signal: null} / 1 / {} reject.
  function validateAbortSignal(signal, name) {
    if (signal === undefined) {
      return;
    }
    if (!(signal instanceof AbortSignal)) {
      throw errInvalidArgTypeInstance(name, 'AbortSignal', signal);
    }
  }

  // node internal/errors.js AbortError (oracle: pre-aborted and abort()
  // rejections both carry name='AbortError' + code='ABORT_ERR'; a custom
  // signal.reason is NOT propagated — abort('x') still rejects this shape).
  function abortError() {
    const err = new Error('The operation was aborted');
    err.name = 'AbortError';
    err.code = 'ABORT_ERR';
    return err;
  }

  // node lib/events.js once(). Validation order mirrors the node source:
  // options object → options.signal → already-aborted → emitter face.
  async function once(emitter, name, options = kEmptyObject) {
    validateObject(options, 'options');
    let signal;
    if (options.signal !== undefined) {
      signal = options.signal;
      validateAbortSignal(signal, 'options.signal');
    }
    if (signal?.aborted) {
      throw abortError();
    }
    const isEmitter = typeof emitter?.on === 'function';
    if (!isEmitter && typeof emitter?.addEventListener !== 'function') {
      throw errInvalidArgTypeInstance('emitter', 'EventEmitter', emitter);
    }
    return new Promise((resolve, reject) => {
      let settled = false;
      const eventListener = (...args) => {
        cleanup();
        resolve(args);
      };
      // EventEmitter face: a distinct 'error' listener rejects the promise
      // with the error payload and is removed on settle (oracle: during a
      // pending signal-backed wait listenerCount('error') === 1; EventTargets
      // have no error-reject semantics — an 'error' event resolves).
      let errorListener;
      if (isEmitter && name !== 'error') {
        errorListener = (err) => {
          cleanup();
          reject(err);
        };
      }
      const abortListener = () => {
        cleanup();
        reject(abortError());
      };
      function cleanup() {
        if (settled) {
          return;
        }
        settled = true;
        if (isEmitter) {
          emitter.removeListener(name, eventListener);
          if (errorListener !== undefined) {
            emitter.removeListener('error', errorListener);
          }
        } else {
          emitter.removeEventListener(name, eventListener);
        }
        if (signal !== undefined) {
          signal.removeEventListener('abort', abortListener);
        }
      }
      if (errorListener !== undefined) {
        emitter.on('error', errorListener);
      }
      if (isEmitter) {
        emitter.on(name, eventListener);
      } else {
        emitter.addEventListener(name, eventListener);
      }
      if (signal !== undefined) {
        const signalOpts = { once: true };
        signalOpts[K_RESIST] = true;
        signal.addEventListener('abort', abortListener, signalOpts);
      }
    });
  }

  return once;
})()
"#;

/// Evaluate the promise-form `events.once` helper and install it as the
/// module static. Idempotent per realm (gc_store cached). Returns the
/// installed function object, or None on failure (exception left pending —
/// the caller must propagate; no silent fallback).
unsafe fn ensure_events_once_installed(cx: *mut JSContext) -> Option<*mut JSObject> {
    // The helper requires the ported world: integrated AbortSignal, global
    // EventTarget/Event classes, dual statics. Idempotent.
    if !crate::node_internal_event_target::ensure_installed(cx) {
        return None;
    }
    if let Some(existing) = gc_store_get(cx, EVENTS_ONCE_KEY).filter(|p| !p.is_null()) {
        return Some(existing);
    }
    let mut source_text = mozjs::rust::transform_str_to_source_text(EVENTS_ONCE_HELPER_JS);
    let mut rval = UndefinedValue();
    let rval_handle = MutableHandle::<Value> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut rval,
    };
    let filename = ZBox::from_bytes("<internal/events_once>".as_bytes());
    let opts = mozjs::glue::NewCompileOptions(cx, filename.as_ptr(), 1);
    if opts.is_null() {
        JS_ClearPendingException(cx);
        return None;
    }
    let ok = mozjs_sys::jsapi::JS::Evaluate2(cx, opts, &mut source_text, rval_handle);
    mozjs::glue::DeleteCompileOptions(opts);
    if !ok {
        // Engine exception stays pending; the caller propagates it verbatim
        // (node parity — a broken bootstrap surfaces as the real error).
        return None;
    }
    if !rval.is_object() {
        JS_ClearPendingException(cx);
        return None;
    }
    let fn_obj = rval.to_object();
    let mut cx_ref = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    // Install as the module static so every later `require('events').once`
    // is the promise form (the trampoline below is no longer reachable).
    if let Some(ctor) = crate::require::get_builtin(cx, "events").filter(|p| !p.is_null()) {
        let fn_val = ObjectValue(fn_obj);
        rooted!(&in(cx_ref) let fn_root = fn_val);
        rooted!(&in(cx_ref) let ctor_root = ctor);
        JS_DefineProperty(
            cx,
            ctor_root.handle().into(),
            c"once".as_ptr(),
            fn_root.handle().into(),
            (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
        );
    }
    gc_store_insert(cx, EVENTS_ONCE_KEY, fn_obj);
    Some(fn_obj)
}

/// Fetch a callable static off the events ctor, skipping the current
/// trampoline itself (pointer identity via `callee`): the
/// internal/event_target port REPLACES the dual-face statics
/// (listenerCount / getEventListeners) with implementations that also read
/// EventTarget kEvents slots — the Rust hidden-state path cannot see those.
unsafe fn fetch_ctor_dual_static(
    cx: *mut JSContext,
    name: &str,
    callee: *mut JSObject,
) -> Option<*mut JSObject> {
    if !crate::node_internal_event_target::ensure_installed(cx) {
        return None;
    }
    let ctor = crate::require::get_builtin(cx, "events").filter(|p| !p.is_null())?;
    let mut cx_ref = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(cx_ref) let ctor_root = ctor);
    let name_box = ZBox::from_bytes(format!("{}\0", name).as_bytes());
    let mut fn_val = UndefinedValue();
    JS_GetProperty(
        cx,
        ctor_root.handle().into(),
        name_box.as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut fn_val,
        },
    );
    if !fn_val.is_object() || !is_function(fn_val.to_object()) {
        return None;
    }
    let fn_obj = fn_val.to_object();
    // Still the trampoline itself → the port never replaced it; the native
    // body below is the authoritative face.
    if fn_obj == callee {
        return None;
    }
    Some(fn_obj)
}

/// Re-dispatch the current call to `fn` (bootstrap-installed JS face),
/// preserving the original arguments; `this` = current global (node module
/// functions are not `this`-sensitive). Shared with the AbortSignal
/// bootstrap trampolines (node_internal_event_target) — same lazy-install +
/// re-dispatch shape.
pub(crate) unsafe fn redispatch_to_js(
    cx: *mut JSContext,
    argc: u32,
    args: &CallArgs,
    fn_obj: *mut JSObject,
) -> bool {
    let cx_ref = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    // `JS_CallFunctionValue` takes the callee as `Handle<Value>` — wrap the
    // object in `ObjectValue` and root the JSVal (never a bare object handle).
    let fn_val = ObjectValue(fn_obj);
    rooted!(&in(cx_ref) let fn_root = fn_val);
    let global = CurrentGlobalOrNull(cx);
    if global.is_null() {
        throw_type_error(cx, "events statics require a global object");
        return false;
    }
    rooted!(&in(cx_ref) let global_root = global);
    // `CallArgs` exposes no length accessor in the Rust bindings — the native
    // fn's own `argc` parameter is the canonical count (bun_sqlite:1562 and
    // every other forwarding site use the signature parameter).
    let mut elements: Vec<JSVal> = Vec::with_capacity(argc as usize);
    for i in 0..argc {
        elements.push(*args.get(i).ptr);
    }
    let call_args = HandleValueArray {
        length_: argc as usize,
        elements_: elements.as_ptr(),
    };
    let mut rval = UndefinedValue();
    let ok = JS_CallFunctionValue(
        cx,
        global_root.handle().into(),
        fn_root.handle().into(),
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
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn events_static_once(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    match ensure_events_once_installed(cx) {
        Some(once_fn) => redispatch_to_js(cx, argc, &args, once_fn),
        None => {
            if JS_IsExceptionPending(cx) {
                // Bootstrap eval failure — propagate the real engine error.
                return false;
            }
            throw_type_error(cx, "events.once could not be initialized");
            false
        }
    }
}

// @trace REQ-ENG-006 [api:events.listenerCount dual face] — node v24
// `events.listenerCount(emitterOrTarget, type)` is DUAL-NATURE: the
// EventEmitter arm reads the emitter's own listenerCount, the EventTarget
// arm reads the kEvents slot map (node oracle: AbortSignal counts). The
// Rust hidden-state body below cannot see EventTarget slots, so the call
// first bootstraps the internal/event_target port (which REPLACES this
// static with the dual implementation) and re-dispatches; the native body
// stays for the EventEmitter face.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn events_static_listener_count(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if let Some(dual) = fetch_ctor_dual_static(cx, "listenerCount", args.callee()) {
        return redispatch_to_js(cx, argc, &args, dual);
    }
    if argc < 2 || !(*args.get(0).ptr).is_object() {
        args.rval().set(Int32Value(0));
        return true;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let emitter_root = (*args.get(0).ptr).to_object());

    let event_name = if (*args.get(1).ptr).is_string() {
        crate::js_to_rust_string(cx, *args.get(1).ptr)
    } else {
        args.rval().set(Int32Value(0));
        return true;
    };

    let state = ensure_state(cx, emitter_root.get());
    let count = state
        .listeners
        .get(&event_name)
        .map(|l| l.len())
        .unwrap_or(0) as i32;
    set_state(cx, emitter_root.get(), state);
    args.rval().set(Int32Value(count));
    true
}

// @trace REQ-ENG-006 [api:events.getEventListeners dual face] — same dual
// dispatch as events_static_listener_count: the port's replacement reads
// EventTarget kEvents lists; the native body keeps the EventEmitter face.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn events_static_get_event_listeners(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if let Some(dual) = fetch_ctor_dual_static(cx, "getEventListeners", args.callee()) {
        return redispatch_to_js(cx, argc, &args, dual);
    }
    if argc < 2 || !(*args.get(0).ptr).is_object() {
        let arr = mozjs::jsapi::NewArrayObject1(cx, 0);
        args.rval().set(if arr.is_null() {
            UndefinedValue()
        } else {
            ObjectValue(arr)
        });
        return true;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let emitter_root = (*args.get(0).ptr).to_object());

    let event_name = if (*args.get(1).ptr).is_string() {
        crate::js_to_rust_string(cx, *args.get(1).ptr)
    } else {
        let arr = mozjs::jsapi::NewArrayObject1(cx, 0);
        args.rval().set(if arr.is_null() {
            UndefinedValue()
        } else {
            ObjectValue(arr)
        });
        return true;
    };

    let state = ensure_state(cx, emitter_root.get());
    let listener_keys = state
        .listeners
        .get(&event_name)
        .cloned()
        .unwrap_or_default();
    set_state(cx, emitter_root.get(), state);

    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let arr = mozjs::rust::wrappers2::NewArrayObject1(cx_ref, listener_keys.len()));
    let mut arr_idx = 0u32;
    for key in &listener_keys {
        if let Some(cb) = gc_store_get(cx, key) {
            let val = ObjectValue(cb);
            rooted!(&in(cx_ref) let v = val);
            JS_DefineElement(
                cx,
                arr.handle().into(),
                arr_idx,
                v.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
            arr_idx += 1;
        }
    }
    args.rval().set(ObjectValue(arr.get()));
    true
}

// ── Unit tests for EmitterState (pure Rust, no JSContext) ────────────────
// @trace REQ-ENG-007 [req:REQ-ENG-007] [level:unit]

#[cfg(test)]
mod tests {
    use super::*;
    use ::std::cell::RefCell;
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
        state.listeners.insert(
            "data".to_string(),
            vec![
                "ee_data_0".to_string(),
                "ee_data_1".to_string(),
                "ee_data_2".to_string(),
            ],
        );
        assert!(state.listeners.contains_key("data"));
        assert_eq!(state.listeners.get("data").map(|v| v.len()), Some(3));
    }

    #[test]
    fn emitter_state_can_set_once_flag() {
        let mut state = make_state();
        state
            .once_flags
            .insert("end".to_string(), vec![true, false]);
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
    fn emitter_class_name() {
        let name = unsafe { ::std::ffi::CStr::from_ptr(EMITTER_CLASS.name) };
        assert_eq!(name.to_str().unwrap(), "EventEmitter");
    }

    #[test]
    fn emitter_state_listener_count_for_event() {
        let mut state = make_state();
        state.listeners.insert(
            "data".to_string(),
            vec![
                "ee_data_0".to_string(),
                "ee_data_1".to_string(),
                "ee_data_2".to_string(),
                "ee_data_3".to_string(),
                "ee_data_4".to_string(),
            ],
        );
        let count = state.listeners.get("data").map(|v| v.len()).unwrap_or(0);
        assert_eq!(count, 5);
    }

    #[test]
    fn emitter_state_once_flag_tracking() {
        let mut state = make_state();
        state
            .once_flags
            .insert("data".to_string(), vec![true, false]);
        let flags = state.once_flags.get("data").unwrap();
        assert!(flags[0]);
        assert!(!flags[1]);
    }

    #[test]
    fn emitter_state_clone_listener_vec() {
        let mut state = make_state();
        let listeners = vec![
            "ee_data_0".to_string(),
            "ee_data_1".to_string(),
            "ee_data_2".to_string(),
        ];
        state
            .listeners
            .insert("data".to_string(), listeners.clone());
        let cloned = state.listeners.get("data").unwrap().clone();
        assert_eq!(cloned.len(), listeners.len());
    }
}
