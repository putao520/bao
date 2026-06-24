// @trace REQ-ENG-009 [entity:FfiLibrary] [api:GET /api/ffi-bridge]
// bun:ffi SpiderMonkey bridge — dlopen/dlsym/dlclose via libc + libffi.
//
// Architecture: FfiLibrary pointer stored in JS object reserved slot 0
// via PrivateValue. Uses JS_InitClass for proper constructor/prototype chain.
// FfiCallback wraps a libffi Closure that bridges C → JS via JS_CallFunctionValue.

use bun_core::ZBox;
use ::std::os::raw::c_void;
use ::std::ptr::NonNull;
use ::std::result::Result;

use libffi::middle::{Cif, Closure, Type};

use mozjs::glue::JS_GetReservedSlot;
use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, ObjectValue, UndefinedValue, PrivateValue, NullValue, BooleanValue, DoubleValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

const SLOT_LIB: u32 = 0;
const SLOT_CB: u32 = 0;

static FFI_LIBRARY_CLASS: JSClass = JSClass {
    name: c"FfiLibrary".as_ptr(),
    flags: (1u32 << JSCLASS_RESERVED_SLOTS_SHIFT),
    cOps: ::std::ptr::null(),
    spec: ::std::ptr::null(),
    ext: ::std::ptr::null(),
    oOps: ::std::ptr::null(),
};

static FFI_CALLBACK_CLASS: JSClass = JSClass {
    name: c"FfiCallback".as_ptr(),
    flags: (1u32 << JSCLASS_RESERVED_SLOTS_SHIFT),
    cOps: ::std::ptr::null(),
    spec: ::std::ptr::null(),
    ext: ::std::ptr::null(),
    oOps: ::std::ptr::null(),
};

/// Userdata held by the libffi closure. The C entry point uses `cx` and
/// `js_fn` to invoke the JavaScript callback via JS_CallFunctionValue.
/// @trace REQ-ENG-009 [entity:FfiCallback]
struct FfiCallbackData {
    cx: *mut JSContext,
    js_fn: *mut JSObject,
}

/// FfiCallback owns a libffi Closure that exposes a C-callable
/// `extern "C" fn(c_void)` which dispatches to a JavaScript function. The
/// closure's userdata is heap-allocated and kept alive for the lifetime of
/// this struct, so the closure (and its code pointer) stay valid until
/// `.close()` is called or the JS wrapper is GC'd.
/// @trace REQ-ENG-009 [entity:FfiCallback]
pub struct FfiCallback {
    // Order matters: `_data` must outlive `_closure` because the closure
    // borrows `&'_ data` (libffi stores the userdata pointer). Drop order is
    // declaration order, so `_closure` drops first, freeing the trampoline
    // before the borrowed `_data` is released.
    _closure: Closure<'static>,
    _data: Box<FfiCallbackData>,
    code_ptr: *const c_void,
}

impl FfiCallback {
    /// Build a no-argument, void-return callback that invokes `js_fn` on `cx`.
    /// Returns the C code pointer the closure exposes (suitable for handing to
    /// `dlsym`-resolved call sites that accept `extern "C" fn()`).
    pub fn new(cx: *mut JSContext, js_fn: *mut JSObject) -> Self {
        // CIF: no arguments, void return.
        let cif = Cif::new(::std::iter::empty(), Type::void());
        let data = Box::new(FfiCallbackData { cx, js_fn });
        // Safety: we extend the data's borrow to `'static` for the closure.
        // This is sound because `_data` outlives `_closure` in this struct
        // (drop order is declaration order; `_closure` is declared first).
        let data_ref: &'static FfiCallbackData = unsafe {
            &*(data.as_ref() as *const FfiCallbackData)
        };
        let closure = Closure::new(cif, ffi_callback_dispatch, data_ref);
        let code_ptr = unsafe {
            let fun: &extern "C" fn() = closure.instantiate_code_ptr();
            (*fun) as extern "C" fn() as *const () as *const c_void
        };
        Self {
            _closure: closure,
            _data: data,
            code_ptr,
        }
    }

    /// Raw C function pointer exposed by the closure.
    pub fn code_ptr(&self) -> *const c_void {
        self.code_ptr
    }
}

/// libffi trampoline: invoked from C with no args, dispatches to the JS
/// function stored in `userdata`. Any JS exception is silently cleared — a C
/// caller has no way to observe a JS exception value.
/// @trace REQ-ENG-009 [entity:FfiCallback]
unsafe extern "C" fn ffi_callback_dispatch(
    _cif: &libffi::low::ffi_cif,
    _result: &mut c_void,
    _args: *const *const c_void,
    userdata: &FfiCallbackData,
) {
    let cx = userdata.cx;
    let js_fn = userdata.js_fn;
    if cx.is_null() || js_fn.is_null() {
        return;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let global = CurrentGlobalOrNull(cx));
    if global.get().is_null() {
        return;
    }
    rooted!(&in(wrapped_cx) let fn_root = js_fn);
    let fn_val = ObjectValue(fn_root.get());
    rooted!(&in(wrapped_cx) let fn_val_root = fn_val);
    let mut rval = UndefinedValue();
    let call_args = HandleValueArray::empty();
    JS_CallFunctionValue(
        cx,
        global.handle().into(),
        fn_val_root.handle().into(),
        &call_args,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        },
    );
    if JS_IsExceptionPending(cx) {
        JS_ClearPendingException(cx);
    }
}

/// FfiLibrary wraps a dlopen handle.
pub struct FfiLibrary {
    handle: *mut ::std::ffi::c_void,
}

impl FfiLibrary {
    pub fn dlopen(path: &str) -> Result<Self, String> {
        let c_path = ZBox::from_bytes(path.as_bytes());
        let handle = unsafe { libc::dlopen(c_path.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if handle.is_null() {
            let err = unsafe { libc::dlerror() };
            let msg = if err.is_null() {
                "dlopen failed: unknown error".to_string()
            } else {
                unsafe { ::std::ffi::CStr::from_ptr(err).to_string_lossy().into_owned() }
            };
            return Err(msg);
        }
        Ok(Self { handle })
    }

    pub fn close(&mut self) -> Result<(), String> {
        if self.handle.is_null() {
            return Err("Library already closed".into());
        }
        unsafe { libc::dlclose(self.handle) };
        self.handle = ::std::ptr::null_mut();
        Ok(())
    }

    pub fn is_closed(&self) -> bool {
        self.handle.is_null()
    }

    pub fn symbol(&self, name: &str) -> Result<*mut ::std::ffi::c_void, String> {
        if self.handle.is_null() {
            return Err("Library is closed".into());
        }
        let c_name = ZBox::from_bytes(name.as_bytes());
        // Clear any previous error
        unsafe { libc::dlerror() };
        let sym = unsafe { libc::dlsym(self.handle, c_name.as_ptr()) };
        let err = unsafe { libc::dlerror() };
        if !err.is_null() {
            let msg = unsafe { ::std::ffi::CStr::from_ptr(err).to_string_lossy().into_owned() };
            return Err(msg);
        }
        Ok(sym)
    }
}

impl Drop for FfiLibrary {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { libc::dlclose(self.handle); }
        }
    }
}

/// Install bun:ffi module with dlopen function and FfiLibrary class.
pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let obj = unsafe { w2::JS_NewPlainObject(cx) });
    if obj.get().is_null() {
        return;
    }

    unsafe {
        // Register FfiLibrary class
        rooted!(&in(cx) let global = CurrentGlobalOrNull(cx.raw_cx()));
        if !global.get().is_null() {
            rooted!(&in(cx) let null_proto = ::std::ptr::null_mut::<JSObject>());
            let proto = w2::JS_InitClass(
                cx,
                global.handle(),
                &FFI_LIBRARY_CLASS,
                null_proto.handle(),
                c"FfiLibrary".as_ptr(),
                Some(ffi_library_constructor),
                1,
                ::std::ptr::null(),
                FFI_LIBRARY_METHODS.as_ptr(),
                ::std::ptr::null(),
                ::std::ptr::null(),
            );

            if !proto.is_null() {
                rooted!(&in(cx) let proto_h = proto);
                rooted!(&in(cx) let ctor = JS_GetConstructor(cx.raw_cx(), proto_h.handle().into()));
                if !ctor.get().is_null() {
                    let ctor_val = ObjectValue(ctor.get());
                    rooted!(&in(cx) let cv = ctor_val);
                    JS_DefineProperty(
                        cx.raw_cx(),
                        obj.handle().into(),
                        c"FfiLibrary".as_ptr(),
                        cv.handle().into(),
                        (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
                    );
                }
            }
        }

        // dlopen as a module-level function
        w2::JS_DefineFunction(
            cx,
            obj.handle(),
            c"dlopen".as_ptr(),
            Some(ffi_dlopen),
            1,
            JSPROP_ENUMERATE as u32,
        );

        // callback(fn) — wraps a JS function as a C-callable extern "C" fn()
        w2::JS_DefineFunction(
            cx,
            obj.handle(),
            c"callback".as_ptr(),
            Some(ffi_callback),
            1,
            JSPROP_ENUMERATE as u32,
        );

        // Register FfiCallback class on the global so JS_NewObject with the
        // class succeeds at callback() time. Only when a global exists.
        if !global.get().is_null() {
            rooted!(&in(cx) let null_proto_cb = ::std::ptr::null_mut::<JSObject>());
            let _ = w2::JS_InitClass(
                cx,
                global.handle(),
                &FFI_CALLBACK_CLASS,
                null_proto_cb.handle(),
                c"FfiCallback".as_ptr(),
                ::std::option::Option::None,
                0,
                ::std::ptr::null(),
                FFI_CALLBACK_METHODS.as_ptr(),
                ::std::ptr::null(),
                ::std::ptr::null(),
            );
        }
    }

    cache_builtin(cx, "bun:ffi", obj.get());
}

const FFI_LIBRARY_METHODS: &[JSFunctionSpec] = &[
    JSFunctionSpec {
        name: JSPropertySpec_Name {
            string_: c"close".as_ptr(),
        },
        call: JSNativeWrapper {
            op: Some(ffi_library_close),
            info: ::std::ptr::null_mut(),
        },
        nargs: 0,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name {
            string_: c"symbol".as_ptr(),
        },
        call: JSNativeWrapper {
            op: Some(ffi_library_symbol),
            info: ::std::ptr::null_mut(),
        },
        nargs: 1,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec::ZERO,
];

const FFI_CALLBACK_METHODS: &[JSFunctionSpec] = &[
    JSFunctionSpec {
        name: JSPropertySpec_Name {
            string_: c"close".as_ptr(),
        },
        call: JSNativeWrapper {
            op: Some(ffi_callback_close),
            info: ::std::ptr::null_mut(),
        },
        nargs: 0,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec {
        name: JSPropertySpec_Name {
            string_: c"ptr".as_ptr(),
        },
        call: JSNativeWrapper {
            op: Some(ffi_callback_ptr),
            info: ::std::ptr::null_mut(),
        },
        nargs: 0,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec::ZERO,
];

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ffi_library_constructor(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let this = JS_NewObjectForConstructor(cx, &FFI_LIBRARY_CLASS, &args);
    if this.is_null() {
        JS_ClearPendingException(cx);
        let this_val = args.thisv();
        if this_val.is_object() {
            let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            rooted!(&in(wrapped_cx) let this_root = this_val.to_object());
            args.rval().set(ObjectValue(this_root.get()));
        } else {
            args.rval().set(UndefinedValue());
        }
        return true;
    }

    let path = if argc >= 1 {
        let path_val = *args.get(0).ptr;
        if path_val.is_string() {
            crate::js_to_rust_string(cx, path_val)
        } else {
            let msg = ZBox::from_bytes("FfiLibrary: path argument must be a string".as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
    } else {
        let msg = ZBox::from_bytes("FfiLibrary: path argument required".as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    };

    match FfiLibrary::dlopen(&path) {
        Ok(lib) => {
            let lib_ptr = Box::into_raw(Box::new(lib)) as *const ::std::os::raw::c_void;
            let val = PrivateValue(lib_ptr);
            JS_SetReservedSlot(this, SLOT_LIB, &val);
            args.rval().set(ObjectValue(this));
            true
        }
        Err(e) => {
            let msg = ZBox::from_bytes(e.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            false
        }
    }
}

/// Extract FfiLibrary pointer from `this` object's reserved slot.
/// @trace BCE-20260618-002 [level:regression]
unsafe fn get_lib(cx: *mut JSContext, thisv: Handle<Value>) -> Option<*mut FfiLibrary> {
    if !thisv.is_object() {
        return None;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj_root = thisv.to_object());
    let mut slot = UndefinedValue();
    JS_GetReservedSlot(obj_root.get(), SLOT_LIB, &mut slot);
    // Guard non-private doubles before to_private() — a freshly-constructed
    // FfiLibrary whose dlopen() failed leaves SLOT_LIB undefined, and
    // to_private() on undefined triggers the is_double() assertion panic.
    if !(slot.is_double() && (slot.asBits_ & 0xFFFF000000000000) == 0) {
        return None;
    }
    let ptr = slot.to_private() as *mut FfiLibrary;
    if ptr.is_null() {
        None
    } else {
        Some(ptr)
    }
}

/// Module-level dlopen function: `dlopen(path)` → FfiLibrary JS object.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ffi_dlopen(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let path = if argc >= 1 {
        let path_val = *args.get(0).ptr;
        if path_val.is_string() {
            crate::js_to_rust_string(cx, path_val)
        } else {
            let msg = ZBox::from_bytes("dlopen: argument must be a string".as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
    } else {
        let msg = ZBox::from_bytes("dlopen: path argument required".as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    };

    match FfiLibrary::dlopen(&path) {
        Ok(lib) => {
            let lib_ptr = Box::into_raw(Box::new(lib)) as *const ::std::os::raw::c_void;
            // Create a JS wrapper object with the FfiLibrary class
            let obj = JS_NewObject(cx, &FFI_LIBRARY_CLASS);
            if obj.is_null() {
                args.rval().set(NullValue());
                return true;
            }
            let val = PrivateValue(lib_ptr);
            JS_SetReservedSlot(obj, SLOT_LIB, &val);

            // Root the object and define instance methods using rooted handle
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let obj_r = obj);
            JS_DefineFunction(
                cx,
                obj_r.handle().into(),
                c"close".as_ptr(),
                Some(ffi_library_close),
                0,
                JSPROP_ENUMERATE as u32,
            );
            JS_DefineFunction(
                cx,
                obj_r.handle().into(),
                c"symbol".as_ptr(),
                Some(ffi_library_symbol),
                1,
                JSPROP_ENUMERATE as u32,
            );

            args.rval().set(ObjectValue(obj_r.get()));
            true
        }
        Err(e) => {
            let msg = ZBox::from_bytes(e.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            false
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ffi_library_close(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let thisv = args.thisv();

    let lib_ptr = match get_lib(cx, thisv) {
        Some(p) => p,
        None => {
            let msg =
                ZBox::from_bytes("FfiLibrary.close: invalid FfiLibrary object".as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
    };

    let lib = &mut *lib_ptr;
    match lib.close() {
        Ok(()) => {
            args.rval().set(UndefinedValue());
            true
        }
        Err(e) => {
            let msg = ZBox::from_bytes(e.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            false
        }
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ffi_library_symbol(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let thisv = args.thisv();

    let lib_ptr = match get_lib(cx, thisv) {
        Some(p) => p,
        None => {
            let msg =
                ZBox::from_bytes("FfiLibrary.symbol: invalid FfiLibrary object".as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
    };

    let name = if argc >= 1 {
        crate::js_to_rust_string(cx, *args.get(0).ptr)
    } else {
        let msg = ZBox::from_bytes("symbol: name argument required".as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    };

    let lib = &*lib_ptr;
    match lib.symbol(&name) {
        Ok(_ptr) => {
            // Return true to indicate the symbol was found
            args.rval().set(BooleanValue(true));
            true
        }
        Err(e) => {
            let msg = ZBox::from_bytes(e.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            false
        }
    }
}

/// Module-level `callback(fn)` — wraps a JavaScript function as a C-callable
/// `extern "C" fn()`. Returns an FfiCallback JS object exposing `.ptr` (raw
/// function pointer as a Number) and `.close()` to release the closure.
/// @trace REQ-ENG-009 [entity:FfiCallback] [api:POST /ffi/load]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ffi_callback(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    // Validate the JS callback argument is callable.
    let js_fn = if argc >= 1 {
        let fn_val = *args.get(0).ptr;
        if !(fn_val.is_object() && IsCallable(fn_val.to_object())) {
            let msg = ZBox::from_bytes(
                "bun:ffi callback: argument must be a function".as_bytes(),
            );
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
        fn_val.to_object()
    } else {
        let msg = ZBox::from_bytes("bun:ffi callback: function argument required".as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    };

    // Build the FfiCallback and stash it in a heap Box whose pointer is stored
    // in the JS wrapper's reserved slot.
    let cb = match ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| {
        FfiCallback::new(cx, js_fn)
    })) {
        Ok(c) => c,
        Err(_) => {
            let msg = ZBox::from_bytes("bun:ffi callback: closure allocation failed".as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
    };
    let code_ptr = cb.code_ptr();
    let cb_box = Box::new(cb);
    let cb_ptr = Box::into_raw(cb_box) as *const c_void;

    let obj = JS_NewObject(cx, &FFI_CALLBACK_CLASS);
    if obj.is_null() {
        // Reclaim the Box to avoid a leak on allocation failure.
        let _ = Box::from_raw(cb_ptr as *mut FfiCallback);
        args.rval().set(NullValue());
        return true;
    }
    let slot_val = PrivateValue(cb_ptr);
    JS_SetReservedSlot(obj, SLOT_CB, &slot_val);

    args.rval().set(ObjectValue(obj));

    // Attach code_ptr as a non-enumerable `ptrValue` property (raw pointer
    // surfaced as a Number so JS consumers can read it). Kept off the proto
    // because it is per-instance.
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj_r = obj);
    let ptr_as_f64 = code_ptr as usize as f64;
    rooted!(&in(wrapped_cx) let ptr_val = DoubleValue(ptr_as_f64));
    JS_DefineProperty(
        cx,
        obj_r.handle().into(),
        c"ptrValue".as_ptr(),
        ptr_val.handle().into(),
        JSPROP_ENUMERATE as u32,
    );
    true
}

/// Extract FfiCallback pointer from `this` object's reserved slot.
/// Mirrors the BCE-20260618-002 guard in `get_lib`: undefined slot must not
/// reach `to_private()`.
/// @trace BCE-20260618-002 [level:regression]
unsafe fn get_callback(cx: *mut JSContext, thisv: Handle<Value>) -> Option<*mut FfiCallback> {
    if !thisv.is_object() {
        return None;
    }
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj_root = thisv.to_object());
    let mut slot = UndefinedValue();
    JS_GetReservedSlot(obj_root.get(), SLOT_CB, &mut slot);
    if !(slot.is_double() && (slot.asBits_ & 0xFFFF000000000000) == 0) {
        return None;
    }
    let ptr = slot.to_private() as *mut FfiCallback;
    if ptr.is_null() {
        None
    } else {
        Some(ptr)
    }
}

/// FfiCallback.prototype.close() — releases the libffi closure.
/// @trace REQ-ENG-009 [entity:FfiCallback]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ffi_callback_close(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let thisv = args.thisv();

    let cb_ptr = match get_callback(cx, thisv) {
        Some(p) => p,
        None => {
            let msg =
                ZBox::from_bytes("FfiCallback.close: invalid FfiCallback object".as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
    };

    // Reclaim the Box (drops the libffi Closure, freeing the trampoline).
    let _ = Box::from_raw(cb_ptr);
    // Clear the slot so a second close() reports invalid FfiCallback.
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let obj_root = thisv.to_object());
    let cleared = UndefinedValue();
    JS_SetReservedSlot(obj_root.get(), SLOT_CB, &cleared);
    args.rval().set(UndefinedValue());
    true
}

/// FfiCallback.prototype.ptr() — returns the raw C function pointer as Number.
/// @trace REQ-ENG-009 [entity:FfiCallback]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ffi_callback_ptr(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let thisv = args.thisv();

    let cb_ptr = match get_callback(cx, thisv) {
        Some(p) => p,
        None => {
            let msg = ZBox::from_bytes("FfiCallback.ptr: invalid FfiCallback object".as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
    };

    let cb = &*cb_ptr;
    let ptr_as_f64 = cb.code_ptr() as usize as f64;
    args.rval().set(DoubleValue(ptr_as_f64));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ffi_dlopen_libc() {
        let lib = FfiLibrary::dlopen("libc.so.6").unwrap();
        assert!(!lib.is_closed());
    }

    #[test]
    fn test_ffi_dlopen_nonexistent() {
        assert!(FfiLibrary::dlopen("/nonexistent.so").is_err());
    }

    #[test]
    fn test_ffi_close() {
        let mut lib = FfiLibrary::dlopen("libc.so.6").unwrap();
        lib.close().unwrap();
        assert!(lib.is_closed());
    }

    #[test]
    fn test_ffi_symbol() {
        let lib = FfiLibrary::dlopen("libc.so.6").unwrap();
        let sym = lib.symbol("printf").unwrap();
        assert!(!sym.is_null());
    }

    #[test]
    fn test_ffi_symbol_nonexistent() {
        let lib = FfiLibrary::dlopen("libc.so.6").unwrap();
        assert!(lib.symbol("__nonexistent_symbol_xyz__").is_err());
    }

    #[test]
    fn test_ffi_close_twice_errors() {
        let mut lib = FfiLibrary::dlopen("libc.so.6").unwrap();
        lib.close().unwrap();
        assert!(lib.close().is_err());
    }

    /// REQ-ENG-009 acceptance #2: FFI callback registration. Verifies the
    /// libffi closure is constructed and exposes a non-null C code pointer.
    /// Uses null JSContext/JSObject — the closure is not invoked, so the
    /// trampoline's null-guard short-circuits any dispatch.
    /// @trace REQ-ENG-009 [test:TEST-ENG-009]
    #[test]
    fn test_ffi_callback_constructs_nonnull_code_ptr() {
        let cb = FfiCallback::new(::std::ptr::null_mut(), ::std::ptr::null_mut());
        assert!(!cb.code_ptr().is_null(), "FfiCallback must expose a non-null C code pointer");
    }

    /// REQ-ENG-009 acceptance #2: code_ptr() is stable across calls while the
    /// FfiCallback is alive.
    /// @trace REQ-ENG-009 [test:TEST-ENG-009]
    #[test]
    fn test_ffi_callback_code_ptr_is_stable() {
        let cb = FfiCallback::new(::std::ptr::null_mut(), ::std::ptr::null_mut());
        let p1 = cb.code_ptr();
        let p2 = cb.code_ptr();
        assert_eq!(p1, p2, "code_ptr() must return the same value while alive");
    }

    /// REQ-ENG-009 acceptance #2: dropping an FfiCallback releases the libffi
    /// closure without panicking. Exercises the drop order invariant
    /// (closure drops before userdata).
    /// @trace REQ-ENG-009 [test:TEST-ENG-009]
    #[test]
    fn test_ffi_callback_drop_does_not_panic() {
        let cb = FfiCallback::new(::std::ptr::null_mut(), ::std::ptr::null_mut());
        let _ptr = cb.code_ptr();
        drop(cb); // must not panic — exercises drop order (closure before data)
    }

    /// REQ-ENG-009 acceptance #2: multiple callbacks coexist, each with a
    /// distinct code pointer.
    /// @trace REQ-ENG-009 [test:TEST-ENG-009]
    #[test]
    fn test_ffi_callback_multiple_distinct_pointers() {
        let cb1 = FfiCallback::new(::std::ptr::null_mut(), ::std::ptr::null_mut());
        let cb2 = FfiCallback::new(::std::ptr::null_mut(), ::std::ptr::null_mut());
        assert_ne!(cb1.code_ptr(), cb2.code_ptr(), "distinct closures get distinct code pointers");
    }

    /// REQ-ENG-009 acceptance #2: invoking the trampoline with null userdata is
    /// a safe no-op (the null-guard short-circuits before touching JS).
    /// @trace REQ-ENG-009 [test:TEST-ENG-009]
    #[test]
    fn test_ffi_callback_dispatch_null_userdata_is_noop() {
        let cb = FfiCallback::new(::std::ptr::null_mut(), ::std::ptr::null_mut());
        let fun: extern "C" fn() = unsafe { *cb._closure.instantiate_code_ptr() };
        // Must not crash — null cx/js_fn triggers the early return.
        fun();
    }
}
