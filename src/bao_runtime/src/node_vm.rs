// @trace REQ-ENG-011 [api:vm.createContext / vm.runInNewContext / vm.Script]
//! node:vm module — SpiderMonkey Realm-isolated sandbox implementation.
//!
//! ## Architecture (DEC-ENG-003)
//!
//! Each `vm.createContext(sandbox)` creates an independent SpiderMonkey
//! Compartment via `JS_NewGlobalObject`. The sandbox object's properties are
//! copied onto the new global so that `vm.runInNewContext(code, {x: 42})`
//! makes `x` available as a global variable in the sandbox realm.
//!
//! Code execution uses `AutoRealm` to temporarily switch the JSContext into
//! the sandbox Compartment; dropping the `AutoRealm` restores the caller's
//! realm. Cross-Compartment object references are automatically wrapped by
//! SM's CCW (Cross-Compartment Wrapper) mechanism.
//!
//! ## Key APIs
//!
//! - `vm.createContext(sandbox?, opts?)` → creates new SM Realm, copies
//!   sandbox properties to the new global, returns the contextified sandbox
//! - `vm.runInNewContext(code, sandbox?, opts?)` → createContext + evaluate
//!   in the new Realm, returns the last expression value
//! - `vm.runInThisContext(code, opts?)` → evaluate in caller's Realm
//! - `vm.isContext(obj)` → checks if obj was contextified by createContext
//! - `vm.Script(code, opts?)` → pre-compiled script with runInContext /
//!   runInNewContext methods
//! - `vm.compileFunction(code, params?, opts?)` → compile a function body

use ::std::cell::RefCell;
use ::std::ptr::{self, NonNull};

use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, ObjectValue, UndefinedValue, BooleanValue, Int32Value, NullValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;
use mozjs::rust::wrappers2::JS_NewGlobalObject;
use mozjs::rust::{RealmOptions, SIMPLE_GLOBAL_CLASS, CompileOptionsWrapper, IdVector};
use mozjs::realm::AutoRealm;
use mozjs::conversions::jsstr_to_string;

use crate::require::cache_builtin;

// ──────────────────────────────────────────────────────────────────────────
// VM Context Registry (thread-local)
// ──────────────────────────────────────────────────────────────────────────

/// Tracks contextified objects on this thread so `vm.isContext()` can
/// recognise them. Each entry maps a JSObject* to its sandbox global.
thread_local! {
    static VM_CONTEXT_MAP: RefCell<Vec<(*mut JSObject, *mut JSObject)>> = RefCell::new(Vec::new());
}

/// Register a sandbox object as contextified, with its associated global.
fn register_context(sandbox: *mut JSObject, global: *mut JSObject) {
    VM_CONTEXT_MAP.with(|m| {
        m.borrow_mut().push((sandbox, global));
    });
}

/// Check whether an object has been contextified.
fn is_context_registered(obj: *mut JSObject) -> bool {
    VM_CONTEXT_MAP.with(|m| {
        m.borrow().iter().any(|&(s, _)| ptr::eq(s, obj))
    })
}

/// Look up the sandbox global for a contextified object.
fn get_context_global(obj: *mut JSObject) -> Option<*mut JSObject> {
    VM_CONTEXT_MAP.with(|m| {
        m.borrow().iter().find(|&&(s, _)| ptr::eq(s, obj)).map(|&(_, g)| g)
    })
}

// ──────────────────────────────────────────────────────────────────────────
// Module install
// ──────────────────────────────────────────────────────────────────────────

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let vm_obj = unsafe { w2::JS_NewPlainObject(cx) });
    if vm_obj.get().is_null() {
        return;
    }

    unsafe {
        w2::JS_DefineFunction(cx, vm_obj.handle(), c"runInThisContext".as_ptr(), Some(vm_run_in_this_context), 2, 0);
        w2::JS_DefineFunction(cx, vm_obj.handle(), c"runInNewContext".as_ptr(), Some(vm_run_in_new_context), 3, 0);
        w2::JS_DefineFunction(cx, vm_obj.handle(), c"createContext".as_ptr(), Some(vm_create_context), 2, 0);
        w2::JS_DefineFunction(cx, vm_obj.handle(), c"isContext".as_ptr(), Some(vm_is_context), 1, 0);
        w2::JS_DefineFunction(cx, vm_obj.handle(), c"compileFunction".as_ptr(), Some(vm_compile_function), 2, 0);

        // Script constructor
        let script_fn = JS_NewFunction(
            cx.raw_cx(),
            Some(vm_script_ctor),
            2,
            JSFUN_CONSTRUCTOR,
            c"Script".as_ptr(),
        );
        if !script_fn.is_null() {
            let script_obj = JS_GetFunctionObject(script_fn);

            // Create Script.prototype with runIn* methods
            rooted!(&in(cx) let proto = w2::JS_NewPlainObject(cx));
            if !proto.get().is_null() {
                w2::JS_DefineFunction(cx, proto.handle(), c"runInThisContext".as_ptr(), Some(vm_script_run_in_this_context), 1, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"runInContext".as_ptr(), Some(vm_script_run_in_context), 2, 0);
                w2::JS_DefineFunction(cx, proto.handle(), c"runInNewContext".as_ptr(), Some(vm_script_run_in_new_context), 2, 0);

                // Set Script.prototype = proto on the constructor function
                let proto_val = ObjectValue(proto.get());
                rooted!(&in(cx) let pv = proto_val);
                rooted!(&in(cx) let script_h = script_obj);
                JS_DefineProperty(
                    cx.raw_cx(),
                    script_h.handle().into(),
                    c"prototype".as_ptr(),
                    pv.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }

            // Define vm.Script = constructor function
            let script_val = ObjectValue(script_obj);
            rooted!(&in(cx) let sv = script_val);
            JS_DefineProperty(
                cx.raw_cx(),
                vm_obj.handle().into(),
                c"Script".as_ptr(),
                sv.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }
    }

    cache_builtin(cx, "vm", vm_obj.get());
}

// ──────────────────────────────────────────────────────────────────────────
// vm.createContext — create an independent SM Realm
// ──────────────────────────────────────────────────────────────────────────

/// Creates a new SpiderMonkey Compartment with `JS_NewGlobalObject`,
/// copies sandbox properties onto the new global, marks the sandbox
/// as contextified, and returns it.
///
/// If the sandbox is already contextified, returns it as-is.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn vm_create_context(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    // Get or create the sandbox object.
    let sandbox = if argc > 0 && (*args.get(0).ptr).is_object() {
        (*args.get(0).ptr).to_object()
    } else {
        // No sandbox provided — create an empty object.
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        rooted!(&in(wrapped_cx) let empty = w2::JS_NewPlainObject(&mut wrapped_cx));
        empty.get()
    };

    if sandbox.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    // If already contextified, return as-is.
    if is_context_registered(sandbox) {
        args.rval().set(ObjectValue(sandbox));
        return true;
    }

    // Create a new independent SM Realm/Compartment.
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    let options = RealmOptions::default();

    rooted!(&in(cx_ref) let sandbox_global = JS_NewGlobalObject(
        cx_ref,
        &SIMPLE_GLOBAL_CLASS,
        ptr::null_mut(),
        OnNewGlobalHookOption::FireOnNewGlobalHook,
        &*options,
    ));

    if sandbox_global.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    // Phase 1: Collect sandbox properties in the CALLER's Realm (before
    // entering AutoRealm). This is critical because the sandbox object lives
    // in the caller's compartment; iterating it from the sandbox realm would
    // see it through a CCW, which may not enumerate properties correctly.
    let sandbox_props = collect_sandbox_properties(cx_ref, sandbox);

    // Phase 2: Enter the new Realm and define collected properties on the
    // sandbox global. Values from the caller's realm are automatically wrapped
    // as CCWs by SpiderMonkey when defined in the sandbox realm.
    {
        let mut realm = AutoRealm::new_from_handle(cx_ref, sandbox_global.handle());
        let realm_cx: &mut mozjs::context::JSContext = &mut realm;

        // Init standard classes (Object, Array, Function, etc.)
        rooted!(&in(realm_cx) let g = sandbox_global.get());

        define_properties_on_global(realm_cx, sandbox_global.get(), &sandbox_props);
    }

    // Register as contextified.
    register_context(sandbox, sandbox_global.get());

    // Mark with __isVMContext for isContext() backwards compat.
    rooted!(&in(cx_ref) let marker = BooleanValue(true));
    JS_DefineProperty(cx, Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &sandbox }, c"__isVMContext".as_ptr(), marker.handle().into(), 0);

    args.rval().set(ObjectValue(sandbox));
    true
}

/// Phase 1: Collect sandbox properties in the CALLER's Realm.
///
/// Iterates the sandbox object's own enumerable string-keyed properties
/// using `GetPropertyKeys` + `JS_GetPropertyById`, and stores each key as
/// a Rust `String` and each value as a GC-traced `Heap<JS::Value>`.
///
/// This MUST run before entering the sandbox AutoRealm, because the sandbox
/// object lives in the caller's compartment. From the sandbox realm it would
/// be a CCW, and `Object.keys()` / `GetPropertyKeys` on a CCW may not
/// enumerate properties correctly.
fn collect_sandbox_properties(
    cx: &mut mozjs::context::JSContext,
    sandbox: *mut JSObject,
) -> Vec<(::std::string::String, Box<Heap<JS::Value>>)> {
    let mut props = Vec::new();
    if sandbox.is_null() {
        return props;
    }

    let raw_cx = unsafe { cx.raw_cx() };
    let sandbox_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &sandbox };
    let mut ids = unsafe { IdVector::new(raw_cx) };
    let ok = unsafe { GetPropertyKeys(raw_cx, sandbox_h, JSITER_OWNONLY, ids.handle_mut()) };
    if !ok {
        return props;
    }

    for jsid in &*ids {
        // Only process string-keyed properties (skip symbols and int keys).
        if !jsid.is_string() {
            continue;
        }
        let key_str_ptr = jsid.to_string();
        if key_str_ptr.is_null() {
            continue;
        }
        let key = unsafe { jsstr_to_string(raw_cx, NonNull::new_unchecked(key_str_ptr)) };

        // Get the property value by id using the raw JS_GetPropertyById
        // (takes *mut JSContext + raw Handle types from mozjs_sys).
        let id_h = Handle::<jsid> { _phantom_0: ::std::marker::PhantomData, ptr: jsid as *const jsid as *mut jsid };
        let mut val = UndefinedValue();
        let val_h = MutableHandle::<JS::Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut val };
        let got = unsafe { JS_GetPropertyById(raw_cx, sandbox_h, id_h, val_h) };
        if !got {
            continue;
        }

        // Store key as Rust String, value as Box<Heap<Value>> (GC-traced,
        // survives across realm switches).
        let heap_val = Heap::boxed(val);
        props.push((key, heap_val));
    }

    props
}

/// Phase 2: Define collected properties on the sandbox Realm's global.
///
/// Runs INSIDE the sandbox AutoRealm. For each (key, value) pair collected
/// in Phase 1, defines the property on the global object. Values from the
/// caller's realm are automatically wrapped as CCWs by SpiderMonkey.
fn define_properties_on_global(
    realm_cx: &mut mozjs::context::JSContext,
    global: *mut JSObject,
    props: &[(::std::string::String, Box<Heap<JS::Value>>)],
) {
    if global.is_null() {
        return;
    }

    let raw_cx = unsafe { realm_cx.raw_cx() };
    let global_h = Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &global };

    for (key, heap_val) in props {
        let c_key = unsafe { bun_core::ZBox::from_bytes(key.as_bytes()) };
        if c_key.as_ptr().is_null() {
            continue;
        }

        // SpiderMonkey automatically wraps cross-compartment values as CCWs
        // when defining the property. Heap::handle() returns a Handle<Value>
        // from mozjs_sys which is compatible with the raw JS_DefineProperty.
        let val_h = unsafe { heap_val.handle() };
        unsafe {
            JS_DefineProperty(raw_cx, global_h, c_key.as_ptr(), val_h, JSPROP_ENUMERATE as u32);
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// vm.runInNewContext — createContext + evaluate in sandbox Realm
// ──────────────────────────────────────────────────────────────────────────

/// Creates a new context (if needed), enters the sandbox Realm, evaluates
/// the code, and returns the last expression value.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn vm_run_in_new_context(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || !(*args.get(0).ptr).is_string() {
        JS_ReportErrorUTF8(cx, c"runInNewContext requires a code string".as_ptr());
        return false;
    }

    let code = crate::js_to_rust_string(cx, *args.get(0).ptr);

    // Get sandbox (arg 1) — if not provided, create a default empty one.
    let sandbox_val = if argc > 1 { *args.get(1).ptr } else { UndefinedValue() };
    let sandbox = if sandbox_val.is_object() {
        sandbox_val.to_object()
    } else {
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        rooted!(&in(wrapped_cx) let empty = w2::JS_NewPlainObject(&mut wrapped_cx));
        empty.get()
    };

    // Create or reuse context.
    let sandbox_global = if is_context_registered(sandbox) {
        get_context_global(sandbox).unwrap_or(ptr::null_mut())
    } else {
        // Call vm_create_context internally to create the Realm.
        // We pass the sandbox as argument 0 to createContext.
        // vp layout for CallArgs::from_vp: [rval_slot, magic_marker, arg0, ...]
        let mut ctx_vp = [UndefinedValue(), UndefinedValue(), ObjectValue(sandbox)];
        if !vm_create_context(cx, 1, ctx_vp.as_mut_ptr()) {
            args.rval().set(UndefinedValue());
            return false;
        }
        // After createContext, sandbox is registered.
        get_context_global(sandbox).unwrap_or(ptr::null_mut())
    };

    if sandbox_global.is_null() {
        JS_ReportErrorUTF8(cx, c"runInNewContext: failed to create sandbox context".as_ptr());
        return false;
    }

    // Get filename from options (arg 2).
    let filename = if argc > 2 && (*args.get(2).ptr).is_object() {
        let opts = (*args.get(2).ptr).to_object();
        let mut fn_val = UndefinedValue();
        JS_GetProperty(
            cx,
            Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &opts },
            c"filename".as_ptr(),
            MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut fn_val },
        );
        if fn_val.is_string() { crate::js_to_rust_string(cx, fn_val) } else { "vm.js".to_string() }
    } else if argc > 2 && (*args.get(2).ptr).is_string() {
        crate::js_to_rust_string(cx, *args.get(2).ptr)
    } else {
        "vm.js".to_string()
    };

    // Enter the sandbox Realm, evaluate code, return result.
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let global_h = sandbox_global);

    let mut realm = AutoRealm::new_from_handle(cx_ref, global_h.handle());
    let realm_cx: &mut mozjs::context::JSContext = &mut realm;

    // Evaluate in the sandbox Realm.
    let c_filename = ::std::ffi::CString::new(filename.clone())
        .unwrap_or_else(|_| ::std::ffi::CString::new("vm.js").unwrap());
    let compile_opts = CompileOptionsWrapper::new(realm_cx, c_filename, 1);

    rooted!(&in(realm_cx) let mut rval = UndefinedValue());

    let result = mozjs::rust::evaluate_script(
        realm_cx,
        global_h.handle(),
        &code,
        rval.handle_mut(),
        compile_opts,
    );

    if result.is_err() {
        return false;
    }

    // Run pending jobs (microtasks, etc.).
    unsafe {
        mozjs::jsapi::js::RunJobs(realm_cx.raw_cx());
    }

    args.rval().set(rval.get());
    true
}

// ──────────────────────────────────────────────────────────────────────────
// vm.runInThisContext — evaluate in caller's Realm
// ──────────────────────────────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn vm_run_in_this_context(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || !(*args.get(0).ptr).is_string() {
        JS_ReportErrorUTF8(cx, c"runInThisContext requires a code string".as_ptr());
        return false;
    }

    let code = crate::js_to_rust_string(cx, *args.get(0).ptr);
    let filename = if argc > 1 && (*args.get(1).ptr).is_object() {
        let opts = (*args.get(1).ptr).to_object();
        let mut fn_val = UndefinedValue();
        JS_GetProperty(
            cx,
            Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &opts },
            c"filename".as_ptr(),
            MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut fn_val },
        );
        if fn_val.is_string() { crate::js_to_rust_string(cx, fn_val) } else { "vm.js".to_string() }
    } else if argc > 1 && (*args.get(1).ptr).is_string() {
        crate::js_to_rust_string(cx, *args.get(1).ptr)
    } else {
        "vm.js".to_string()
    };

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    let global = JS::CurrentGlobalOrNull(cx_ref.raw_cx());

    let c_filename = ::std::ffi::CString::new(filename.clone())
        .unwrap_or_else(|_| ::std::ffi::CString::new("vm.js").unwrap());
    let compile_opts = CompileOptionsWrapper::new(cx_ref, c_filename, 1);

    rooted!(&in(cx_ref) let global_h = global);
    rooted!(&in(cx_ref) let mut rval = UndefinedValue());

    let result = mozjs::rust::evaluate_script(
        cx_ref,
        global_h.handle(),
        &code,
        rval.handle_mut(),
        compile_opts,
    );

    if result.is_err() {
        return false;
    }

    args.rval().set(rval.get());
    true
}

// ──────────────────────────────────────────────────────────────────────────
// vm.isContext
// ──────────────────────────────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn vm_is_context(
    _cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc > 0 && (*args.get(0).ptr).is_object() {
        let obj = (*args.get(0).ptr).to_object();
        // Check our VM_CONTEXT_MAP registry (primary) AND the legacy
        // __isVMContext marker (secondary, for objects contextified before
        // this process started or from a different isolate).
        let registered = is_context_registered(obj);
        let marker = if !registered {
            let mut val = UndefinedValue();
            JS_GetProperty(
                _cx,
                Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &obj },
                c"__isVMContext".as_ptr(),
                MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut val },
            );
            val.is_boolean() && val.to_boolean()
        } else {
            true
        };
        args.rval().set(BooleanValue(marker));
    } else {
        args.rval().set(BooleanValue(false));
    }
    true
}

// ──────────────────────────────────────────────────────────────────────────
// vm.compileFunction
// ──────────────────────────────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn vm_compile_function(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || !(*args.get(0).ptr).is_string() {
        JS_ReportErrorUTF8(cx, c"compileFunction requires a code string".as_ptr());
        return false;
    }

    let code = crate::js_to_rust_string(cx, *args.get(0).ptr);
    let fn_name = if argc > 1 && (*args.get(1).ptr).is_string() {
        crate::js_to_rust_string(cx, *args.get(1).ptr)
    } else {
        "anonymous".to_string()
    };

    let wrapped = format!("(function {}() {{ {} }})", fn_name, code);

    let c_filename = bun_core::ZBox::from_bytes("vm.js".as_bytes());
    let opts = mozjs::glue::NewCompileOptions(cx, c_filename.as_ptr() as *const _, 1);
    if opts.is_null() {
        args.rval().set(UndefinedValue());
        return false;
    }

    let mut src = mozjs::rust::transform_str_to_source_text(&wrapped);
    let mut rval = UndefinedValue();
    let rval_h = MutableHandle::<JSVal> {
        _phantom_0: ::std::marker::PhantomData,
        ptr: &mut rval,
    };
    let ok = mozjs_sys::jsapi::JS::Evaluate2(cx, opts, &mut src, rval_h);
    libc::free(opts as *mut _);

    if ok && rval.is_object() {
        args.rval().set(rval);
        true
    } else {
        false
    }
}

// ──────────────────────────────────────────────────────────────────────────
// vm.Script constructor and methods
// ──────────────────────────────────────────────────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn vm_script_ctor(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc == 0 || !(*args.get(0).ptr).is_string() {
        JS_ReportErrorUTF8(cx, c"Script requires a code string argument".as_ptr());
        return false;
    }

    let code = crate::js_to_rust_string(cx, *args.get(0).ptr);

    // Get filename from options
    let filename = if argc > 1 && (*args.get(1).ptr).is_object() {
        let opts = (*args.get(1).ptr).to_object();
        let mut fn_val = UndefinedValue();
        JS_GetProperty(
            cx,
            Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &opts },
            c"filename".as_ptr(),
            MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut fn_val },
        );
        if fn_val.is_string() {
            crate::js_to_rust_string(cx, fn_val)
        } else {
            "vm.js".to_string()
        }
    } else if argc > 1 && (*args.get(1).ptr).is_string() {
        crate::js_to_rust_string(cx, *args.get(1).ptr)
    } else {
        "vm.js".to_string()
    };

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    // Use the `this` object that SM auto-creates for `new Script()` — it
    // already has Script.prototype as its prototype. If called without `new`,
    // create a plain object and set its prototype manually.
    let this_val = args.thisv();
    let this_obj = if this_val.is_object() && !this_val.to_object().is_null() {
        this_val.to_object()
    } else {
        rooted!(&in(cx_ref) let fallback = w2::JS_NewPlainObject(cx_ref));
        // Set prototype to Script.prototype
        let script_ctor_val = args.callee();
        let mut proto_val = UndefinedValue();
        JS_GetProperty(
            cx,
            Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &script_ctor_val },
            c"prototype".as_ptr(),
            MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut proto_val },
        );
        if proto_val.is_object() {
            rooted!(&in(cx_ref) let pv = proto_val.to_object());
            JS_SetPrototype(cx_ref.raw_cx(), fallback.handle().into(), pv.handle().into());
        }
        fallback.get()
    };

    // Store code and filename as hidden properties
    let code_str = JS_NewStringCopyN(cx, code.as_ptr() as *const ::std::os::raw::c_char, code.len());
    if !code_str.is_null() {
        rooted!(&in(cx_ref) let cv = mozjs::jsval::StringValue(&*code_str));
        JS_DefineProperty(cx, Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &this_obj }, c"__code".as_ptr(), cv.handle().into(), 0);
    }
    let fn_str = JS_NewStringCopyN(cx, filename.as_ptr() as *const ::std::os::raw::c_char, filename.len());
    if !fn_str.is_null() {
        rooted!(&in(cx_ref) let fv = mozjs::jsval::StringValue(&*fn_str));
        JS_DefineProperty(cx, Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &this_obj }, c"__filename".as_ptr(), fv.handle().into(), 0);
    }

    args.rval().set(ObjectValue(this_obj));
    true
}

/// Script.runInThisContext — evaluate in caller's Realm
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn vm_script_run_in_this_context(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv().to_object();

    let mut code_val = UndefinedValue();
    JS_GetProperty(
        cx,
        Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &this },
        c"__code".as_ptr(),
        MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut code_val },
    );
    let code = crate::js_to_rust_string(cx, code_val);

    let mut fn_val = UndefinedValue();
    JS_GetProperty(
        cx,
        Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &this },
        c"__filename".as_ptr(),
        MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut fn_val },
    );
    let filename = crate::js_to_rust_string(cx, fn_val);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    let global = JS::CurrentGlobalOrNull(cx_ref.raw_cx());

    let c_filename = ::std::ffi::CString::new(filename.clone())
        .unwrap_or_else(|_| ::std::ffi::CString::new("vm.js").unwrap());
    let compile_opts = CompileOptionsWrapper::new(cx_ref, c_filename, 1);

    rooted!(&in(cx_ref) let global_h = global);
    rooted!(&in(cx_ref) let mut rval = UndefinedValue());

    let result = mozjs::rust::evaluate_script(
        cx_ref,
        global_h.handle(),
        &code,
        rval.handle_mut(),
        compile_opts,
    );

    if result.is_err() {
        return false;
    }

    args.rval().set(rval.get());
    true
}

/// Script.runInContext(contextifiedSandbox) — evaluate in the sandbox Realm
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn vm_script_run_in_context(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv().to_object();

    // Read code and filename from Script instance
    let mut code_val = UndefinedValue();
    JS_GetProperty(
        cx,
        Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &this },
        c"__code".as_ptr(),
        MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut code_val },
    );
    let code = crate::js_to_rust_string(cx, code_val);

    let mut fn_val = UndefinedValue();
    JS_GetProperty(
        cx,
        Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &this },
        c"__filename".as_ptr(),
        MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut fn_val },
    );
    let filename = crate::js_to_rust_string(cx, fn_val);

    // Get contextified sandbox (arg 0)
    if argc == 0 || !(*args.get(0).ptr).is_object() {
        JS_ReportErrorUTF8(cx, c"runInContext requires a contextified sandbox argument".as_ptr());
        return false;
    }
    let sandbox = (*args.get(0).ptr).to_object();

    let sandbox_global = get_context_global(sandbox);
    if sandbox_global.is_none() {
        JS_ReportErrorUTF8(cx, c"runInContext: sandbox is not a contextified object".as_ptr());
        return false;
    }
    let global_ptr = sandbox_global.unwrap();
    if global_ptr.is_null() {
        JS_ReportErrorUTF8(cx, c"runInContext: sandbox global is null".as_ptr());
        return false;
    }

    // Enter sandbox Realm, evaluate code.
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let global_h = global_ptr);

    let mut realm = AutoRealm::new_from_handle(cx_ref, global_h.handle());
    let realm_cx: &mut mozjs::context::JSContext = &mut realm;

    let c_filename = ::std::ffi::CString::new(filename.clone())
        .unwrap_or_else(|_| ::std::ffi::CString::new("vm.js").unwrap());
    let compile_opts = CompileOptionsWrapper::new(realm_cx, c_filename, 1);

    rooted!(&in(realm_cx) let mut rval = UndefinedValue());

    let result = mozjs::rust::evaluate_script(
        realm_cx,
        global_h.handle(),
        &code,
        rval.handle_mut(),
        compile_opts,
    );

    if result.is_err() {
        return false;
    }

    unsafe {
        mozjs::jsapi::js::RunJobs(realm_cx.raw_cx());
    }

    args.rval().set(rval.get());
    true
}

/// Script.runInNewContext(sandbox?) — createContext + evaluate
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn vm_script_run_in_new_context(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let this = args.thisv().to_object();

    let mut code_val = UndefinedValue();
    JS_GetProperty(
        cx,
        Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &this },
        c"__code".as_ptr(),
        MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut code_val },
    );
    let code = crate::js_to_rust_string(cx, code_val);

    let mut fn_val = UndefinedValue();
    JS_GetProperty(
        cx,
        Handle::<*mut JSObject> { _phantom_0: ::std::marker::PhantomData, ptr: &this },
        c"__filename".as_ptr(),
        MutableHandle::<JSVal> { _phantom_0: ::std::marker::PhantomData, ptr: &mut fn_val },
    );
    let filename = crate::js_to_rust_string(cx, fn_val);

    // Get sandbox (arg 0) — if not provided, create a default empty one.
    let sandbox_val = if argc > 0 && (*args.get(0).ptr).is_object() {
        *args.get(0).ptr
    } else {
        UndefinedValue()
    };
    let sandbox = if sandbox_val.is_object() {
        sandbox_val.to_object()
    } else {
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        rooted!(&in(wrapped_cx) let empty = w2::JS_NewPlainObject(&mut wrapped_cx));
        empty.get()
    };

    // Create or reuse context
    let sandbox_global = if is_context_registered(sandbox) {
        get_context_global(sandbox).unwrap_or(ptr::null_mut())
    } else {
        let mut ctx_vp = [UndefinedValue(), UndefinedValue(), ObjectValue(sandbox)];
        if !vm_create_context(cx, 1, ctx_vp.as_mut_ptr()) {
            args.rval().set(UndefinedValue());
            return false;
        }
        get_context_global(sandbox).unwrap_or(ptr::null_mut())
    };

    if sandbox_global.is_null() {
        JS_ReportErrorUTF8(cx, c"Script.runInNewContext: failed to create sandbox context".as_ptr());
        return false;
    }

    // Enter sandbox Realm, evaluate code.
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let global_h = sandbox_global);

    let mut realm = AutoRealm::new_from_handle(cx_ref, global_h.handle());
    let realm_cx: &mut mozjs::context::JSContext = &mut realm;

    let c_filename = ::std::ffi::CString::new(filename.clone())
        .unwrap_or_else(|_| ::std::ffi::CString::new("vm.js").unwrap());
    let compile_opts = CompileOptionsWrapper::new(realm_cx, c_filename, 1);

    rooted!(&in(realm_cx) let mut rval = UndefinedValue());

    let result = mozjs::rust::evaluate_script(
        realm_cx,
        global_h.handle(),
        &code,
        rval.handle_mut(),
        compile_opts,
    );

    if result.is_err() {
        return false;
    }

    unsafe {
        mozjs::jsapi::js::RunJobs(realm_cx.raw_cx());
    }

    args.rval().set(rval.get());
    true
}

