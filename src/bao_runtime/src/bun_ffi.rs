// @trace REQ-ENG-009 [entity:FfiLibrary] [api:GET /api/ffi-bridge]
// bun:ffi SpiderMonkey bridge — dlopen/dlsym/dlclose via libc + libffi.
//
// Architecture: FfiLibrary pointer stored in JS object reserved slot 0
// via PrivateValue. Uses JS_InitClass for proper constructor/prototype chain.

use bun_core::ZBox;
use ::std::ptr::NonNull;
use ::std::result::Result;

use mozjs::glue::JS_GetReservedSlot;
use mozjs::jsapi::*;
use mozjs::jsval::{JSVal, ObjectValue, UndefinedValue, PrivateValue, NullValue, BooleanValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

const SLOT_LIB: u32 = 0;

static FFI_LIBRARY_CLASS: JSClass = JSClass {
    name: c"FfiLibrary".as_ptr(),
    flags: (1u32 << JSCLASS_RESERVED_SLOTS_SHIFT),
    cOps: ::std::ptr::null(),
    spec: ::std::ptr::null(),
    ext: ::std::ptr::null(),
    oOps: ::std::ptr::null(),
};

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

        // callback placeholder
        w2::JS_DefineFunction(
            cx,
            obj.handle(),
            c"callback".as_ptr(),
            Some(ffi_callback_placeholder),
            1,
            JSPROP_ENUMERATE as u32,
        );
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
            args.rval().set(ObjectValue(this_val.to_object()));
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
unsafe fn get_lib(thisv: Handle<Value>) -> Option<*mut FfiLibrary> {
    if !thisv.is_object() {
        return None;
    }
    let obj = thisv.to_object();
    let mut slot = UndefinedValue();
    JS_GetReservedSlot(obj, SLOT_LIB, &mut slot);
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

    let lib_ptr = match get_lib(thisv) {
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

    let lib_ptr = match get_lib(thisv) {
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

unsafe extern "C" fn ffi_callback_placeholder(
    cx: *mut JSContext,
    _argc: u32,
    _vp: *mut JSVal,
) -> bool {
    let msg = ZBox::from_bytes(b"bun:ffi callback is not yet implemented (tracking: REQ-ENG-009).");
    JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
    false
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
}
