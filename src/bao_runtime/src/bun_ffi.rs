// @trace REQ-ENG-009 [entity:FfiLibrary] [api:GET /api/ffi-bridge]
// bun:ffi SpiderMonkey bridge — dlopen/dlsym/dlclose via libc + libffi.
//
// Architecture: FfiLibrary pointer stored in JS object reserved slot 0
// via PrivateValue. Uses JS_InitClass for proper constructor/prototype chain.
// FfiCallback wraps a libffi Closure that bridges C → JS via JS_CallFunctionValue.

use ::std::os::raw::c_void;
use ::std::ptr::NonNull;
use ::std::result::Result;
use bun_core::ZBox;

use libffi::middle::{Cif, Closure, Type};

use mozjs::glue::JS_GetReservedSlot;
use mozjs::jsapi::*;
use mozjs::jsval::{
    BooleanValue, DoubleValue, JSVal, NullValue, ObjectValue, PrivateValue, UndefinedValue,
};
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

static FFI_CSTRING_CLASS: JSClass = JSClass {
    name: c"CString".as_ptr(),
    flags: (1u32 << JSCLASS_RESERVED_SLOTS_SHIFT),
    cOps: ::std::ptr::null(),
    spec: ::std::ptr::null(),
    ext: ::std::ptr::null(),
    oOps: ::std::ptr::null(),
};

/// Userdata held by the libffi closure. The C entry point uses `cx` and
/// `js_fn` to invoke the JavaScript callback via JS_CallFunctionValue, and
/// `arg_types`/`ret` to marshal C args → JS numbers and the JS return → C.
/// @trace REQ-ENG-009 [entity:FfiCallback]
struct FfiCallbackData {
    cx: *mut JSContext,
    js_fn: *mut JSObject,
    arg_types: Vec<TypeSpec>,
    ret: TypeSpec,
}

/// FfiCallback owns a libffi Closure that exposes a C-callable
/// `extern "C" fn(...)` which dispatches to a JavaScript function. The
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
    /// Build a callback with the given argument types and return type that
    /// invokes `js_fn` on `cx`. Returns the C code pointer the closure
    /// exposes (suitable for handing to `dlsym`-resolved call sites that
    /// accept `extern "C" fn(...)` — e.g. qsort comparators, atexit hooks).
    pub fn new(
        cx: *mut JSContext,
        js_fn: *mut JSObject,
        arg_types: Vec<TypeSpec>,
        ret: TypeSpec,
    ) -> Self {
        let cif = Cif::new(
            arg_types.iter().map(|t| t.libffi_type()),
            ret.libffi_type(),
        );
        let data = Box::new(FfiCallbackData {
            cx,
            js_fn,
            arg_types,
            ret,
        });
        // Safety: we extend the data's borrow to `'static` for the closure.
        // This is sound because `_data` outlives `_closure` in this struct
        // (drop order is declaration order; `_closure` is declared first).
        let data_ref: &'static FfiCallbackData =
            unsafe { &*(data.as_ref() as *const FfiCallbackData) };
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

/// Read one C argument slot as a JS number per its TypeSpec. Every supported
/// callback arg type marshals to a primitive JSVal (int32/double) — immediate
/// values the GC never moves, so the args Vec needs no rooting.
unsafe fn c_arg_to_jsval(spec: TypeSpec, slot: *const c_void) -> Value {
    unsafe {
        match spec {
            TypeSpec::Bool => mozjs::jsval::BooleanValue(*(slot as *const u8) != 0),
            TypeSpec::U8 => mozjs::jsval::Int32Value(*(slot as *const u8) as i32),
            TypeSpec::I8 => mozjs::jsval::Int32Value(*(slot as *const i8) as i32),
            TypeSpec::U16 => mozjs::jsval::Int32Value(*(slot as *const u16) as i32),
            TypeSpec::I16 => mozjs::jsval::Int32Value(*(slot as *const i16) as i32),
            TypeSpec::U32 => mozjs::jsval::DoubleValue(*(slot as *const u32) as f64),
            TypeSpec::I32 => mozjs::jsval::Int32Value(*(slot as *const i32)),
            TypeSpec::U64 | TypeSpec::Usize | TypeSpec::Ptr | TypeSpec::CString | TypeSpec::JsFunction => {
                mozjs::jsval::DoubleValue(*(slot as *const u64) as f64)
            }
            TypeSpec::I64 | TypeSpec::Isize => {
                mozjs::jsval::DoubleValue(*(slot as *const i64) as f64)
            }
            TypeSpec::F32 => mozjs::jsval::DoubleValue(*(slot as *const f32) as f64),
            TypeSpec::F64 => mozjs::jsval::DoubleValue(*(slot as *const f64)),
            TypeSpec::Void => UndefinedValue(),
        }
    }
}

/// libffi trampoline: invoked from C, marshals the C args to JS numbers,
/// dispatches to the JS function stored in `userdata`, and writes the JS
/// return value back to the C result slot per the callback's return spec.
/// Any JS exception is silently cleared — a C caller has no way to observe a
/// JS exception value (a non-void result slot keeps its zero-initialized
/// value, matching "callback failed ⇒ 0").
/// @trace REQ-ENG-009 [entity:FfiCallback]
unsafe extern "C" fn ffi_callback_dispatch(
    _cif: &libffi::low::ffi_cif,
    result: &mut c_void,
    args: *const *const c_void,
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

    // C arg slots → primitive JSVals (no allocation, no GC exposure).
    let js_args: Vec<Value> = userdata
        .arg_types
        .iter()
        .enumerate()
        .map(|(i, t)| c_arg_to_jsval(*t, *args.add(i)))
        .collect();
    let call_args = HandleValueArray {
        length_: js_args.len(),
        elements_: js_args.as_ptr(),
    };

    let mut rval = UndefinedValue();
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
    // Return conversion — write the JS number into the C result slot.
    unsafe {
        let num = if rval.is_int32() {
            rval.to_int32() as f64
        } else if rval.is_double() {
            rval.to_double()
        } else {
            ::std::f64::NAN
        };
        let slot = result as *mut c_void;
        match userdata.ret {
            TypeSpec::Void => {}
            TypeSpec::Bool => {
                *(slot as *mut u8) = (rval.is_boolean() && rval.to_boolean()) as u8
            }
            TypeSpec::U8 => *(slot as *mut u8) = num as u8,
            TypeSpec::I8 => *(slot as *mut i8) = num as i8,
            TypeSpec::U16 => *(slot as *mut u16) = num as u16,
            TypeSpec::I16 => *(slot as *mut i16) = num as i16,
            TypeSpec::U32 => *(slot as *mut u32) = num as u32,
            TypeSpec::I32 => *(slot as *mut i32) = num as i32,
            TypeSpec::U64 | TypeSpec::Usize => *(slot as *mut u64) = num as u64,
            TypeSpec::I64 | TypeSpec::Isize => *(slot as *mut i64) = num as i64,
            TypeSpec::F32 => *(slot as *mut f32) = num as f32,
            TypeSpec::F64 => *(slot as *mut f64) = num,
            TypeSpec::Ptr | TypeSpec::CString | TypeSpec::JsFunction => {
                if num.is_finite() && num >= 0.0 {
                    *(slot as *mut usize) = num as usize;
                }
            }
        }
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
                unsafe {
                    ::std::ffi::CStr::from_ptr(err)
                        .to_string_lossy()
                        .into_owned()
                }
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
            let msg = unsafe {
                ::std::ffi::CStr::from_ptr(err)
                    .to_string_lossy()
                    .into_owned()
            };
            return Err(msg);
        }
        Ok(sym)
    }
}

impl Drop for FfiLibrary {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                libc::dlclose(self.handle);
            }
        }
    }
}

// ── bun:ffi JS surface: types / dlopen(path, symbols) / CString / toBuffer ──

/// FFI value type descriptors accepted by `dlopen(path, symbols)` — both the
/// `types` export members and their string names ("i32", "cstring", ...).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TypeSpec {
    Void,
    Bool,
    U8,
    I8,
    U16,
    I16,
    U32,
    I32,
    U64,
    I64,
    Usize,
    Isize,
    F32,
    F64,
    Ptr,
    /// A C callback slot: the JS argument must be a `callback(...)`-built
    /// FfiCallback object; its closure code pointer is passed to the callee.
    JsFunction,
    CString,
}

impl TypeSpec {
    /// Resolve the canonical name for a token — accepts the bare string form
    /// ("i32") and the `types` export's frozen token objects.
    unsafe fn name_of(cx: *mut JSContext, v: JSVal) -> Result<&'static str, String> {
        if v.is_string() {
            let s = crate::js_to_rust_string(cx, v);
            return Ok(match s.as_str() {
                "void" => "void",
                "bool" | "bool8" => "bool",
                "u8" => "u8",
                "i8" => "i8",
                "u16" => "u16",
                "i16" => "i16",
                "u32" => "u32",
                "i32" => "i32",
                "u64" => "u64",
                "i64" => "i64",
                "usize" => "usize",
                "isize" => "isize",
                "f32" | "float" => "f32",
                "f64" | "double" => "f64",
                "ptr" | "pointer" => "ptr",
                "cstring" | "char*" => "cstring",
                "js_function" | "callback" => "js_function",
                other => return Err(format!("unknown FFI type: {}", other)),
            });
        }
        if v.is_object() {
            // `types` token objects carry { __ffiType: "<name>" }.
            let obj = v.to_object();
            let inner = unsafe { js_prop_string(cx, obj, "__ffiType") };
            if let Some(name) = inner {
                // Reuse the string-arm matcher.
                let c_name = ZBox::from_bytes(name.as_bytes());
                let v2 = {
                    let js = unsafe { JS_NewStringCopyZ(cx, c_name.as_ptr()) };
                    if js.is_null() {
                        return Err("type token read failed".to_string());
                    }
                    mozjs::jsval::StringValue(unsafe { &*js })
                };
                return TypeSpec::name_of(cx, v2);
            }
        }
        Err("FFI type must be a name string or a bun:ffi types token".to_string())
    }

    unsafe fn parse(cx: *mut JSContext, v: JSVal) -> Result<Self, String> {
        let name = TypeSpec::name_of(cx, v)?;
        Ok(match name {
            "void" => TypeSpec::Void,
            "bool" => TypeSpec::Bool,
            "u8" => TypeSpec::U8,
            "i8" => TypeSpec::I8,
            "u16" => TypeSpec::U16,
            "i16" => TypeSpec::I16,
            "u32" => TypeSpec::U32,
            "i32" => TypeSpec::I32,
            "u64" => TypeSpec::U64,
            "i64" => TypeSpec::I64,
            "usize" => TypeSpec::Usize,
            "isize" => TypeSpec::Isize,
            "f32" => TypeSpec::F32,
            "f64" => TypeSpec::F64,
            "ptr" => TypeSpec::Ptr,
            "cstring" => TypeSpec::CString,
            "js_function" => TypeSpec::JsFunction,
            other => return Err(format!("unknown FFI type: {}", other)),
        })
    }

    fn libffi_type(&self) -> Type {
        match self {
            TypeSpec::Void => Type::void(),
            TypeSpec::Bool | TypeSpec::U8 => Type::u8(),
            TypeSpec::I8 => Type::i8(),
            TypeSpec::U16 => Type::u16(),
            TypeSpec::I16 => Type::i16(),
            TypeSpec::U32 => Type::u32(),
            TypeSpec::I32 => Type::i32(),
            TypeSpec::U64 => Type::u64(),
            TypeSpec::I64 => Type::i64(),
            TypeSpec::Usize => Type::usize(),
            TypeSpec::Isize => Type::isize(),
            TypeSpec::F32 => Type::f32(),
            TypeSpec::F64 => Type::f64(),
            TypeSpec::Ptr | TypeSpec::CString | TypeSpec::JsFunction => Type::pointer(),
        }
    }
}

/// Read a string-valued property off a JS object.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn js_prop_string(cx: *mut JSContext, obj: *mut JSObject, name: &str) -> Option<String> {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let obj_root = obj);
    let mut v = UndefinedValue();
    JS_GetProperty(
        cx,
        obj_root.handle().into(),
        ZBox::from_bytes(name.as_bytes()).as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut v,
        },
    );
    if v.is_string() {
        Some(crate::js_to_rust_string(cx, v))
    } else {
        None
    }
}

/// Minimal x86-64 SysV dynamic-call trampoline (upstream Bun precedent: FFI
/// CALLS go through hand-written asm there too — libffi is only used for
/// closures). The vendored libffi's `ffi_call` path corrupts its unix64
/// trampoline frame inside this workspace's binaries (works in isolation,
/// crashes with valid cif/args verified via gdb), so primitive-signature
/// calls take this shuffle instead — and every TypeSpec IS a primitive, so no
/// libffi classifier is needed.
///
/// `fn_ptr` in %rdi, `ints` (6×u64) in %rsi, `sses` (8×f64) in %rdx. Unused
/// slots are zero-initialized (harmless: callees ignore unclaimed registers).
/// Returns (rax, xmm0) so callers pick the integer/SSE result per spec.
#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
    ".globl bao_sysv_call",
    "bao_sysv_call:",
    "    mov  r10, rdi",          // fn
    "    movsd xmm0, [rdx+0]",    // SSE args (read first: rdx is temp below)
    "    movsd xmm1, [rdx+8]",
    "    movsd xmm2, [rdx+16]",
    "    movsd xmm3, [rdx+24]",
    "    movsd xmm4, [rdx+32]",
    "    movsd xmm5, [rdx+40]",
    "    movsd xmm6, [rdx+48]",
    "    movsd xmm7, [rdx+56]",
    "    mov  rdi, [rsi+0]",      // int0 → rdi (rsi still needed below)
    "    mov  rax, [rsi+8]",      // int1
    "    mov  rcx, [rsi+16]",     // int2
    "    mov  r8,  [rsi+24]",     // int3
    "    mov  r9,  [rsi+32]",     // int4
    "    mov  r11, [rsi+40]",     // int5
    "    mov  rsi, rax",
    "    mov  rdx, rcx",
    "    mov  rcx, r8",
    "    mov  r8,  r9",
    "    mov  r9,  r11",
    // SysV: rsp must be 16-byte aligned AT the call instruction. We enter as
    // an extern "C" fn (rsp ≡ 8 mod 16) and have pushed nothing, so align
    // down 8 before the call and restore after. Without this, every callee
    // is entered misaligned and any aligned-SSE stack access inside it
    // (e.g. glibc qsort → libffi closure's `movdqa` prologue) SIGSEGVs —
    // leaf-ish targets like getpid never noticed.
    "    sub  rsp, 8",
    "    call r10",
    "    add  rsp, 8",
    "    ret",
);

/// {INTEGER, SSE} two-word return — the SysV ABI passes this class pair in
/// rax + xmm0, which is exactly what the trampoline leaves behind.
#[cfg(target_arch = "x86_64")]
#[repr(C)]
struct SysvRet {
    rax: u64,
    xmm0: f64,
}

#[cfg(target_arch = "x86_64")]
unsafe fn sysv_call(
    fn_ptr: *mut c_void,
    ints: &mut [u64; 6],
    sses: &mut [f64; 8],
) -> SysvRet {
    unsafe extern "C" {
        fn bao_sysv_call(
            fn_ptr: *mut c_void,
            ints: *mut u64,
            sses: *mut f64,
        ) -> SysvRet;
    }
    bao_sysv_call(fn_ptr, ints.as_mut_ptr(), sses.as_mut_ptr())
}

/// A bound foreign function: arg/ret specs + the raw fn pointer. Boxed; the
/// JS function object carries the box pointer as a hidden `_fnSpec` property
/// and the raw fn pointer as `_fnPtr`.
struct FfiFnSpec {
    args: Vec<TypeSpec>,
    ret: TypeSpec,
    fn_ptr: *mut c_void,
}

unsafe impl Send for FfiFnSpec {}

/// Read a hidden numeric property (pointer stored as double).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn fn_hidden_ptr(cx: *mut JSContext, fn_obj: *mut JSObject, name: &str) -> Option<usize> {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let fn_root = fn_obj);
    let mut v = UndefinedValue();
    JS_GetProperty(
        cx,
        fn_root.handle().into(),
        ZBox::from_bytes(name.as_bytes()).as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut v,
        },
    );
    // Private values look like doubles (payload in the low bits, zero high
    // bits) — to_double() would reinterpret the bits as a denormal f64 and
    // truncate to 0 on `as usize`. Private tag + to_private() is the only
    // correct decode.
    if v.is_double() && (v.asBits_ & 0xFFFF000000000000) == 0 {
        Some(v.to_private() as usize)
    } else {
        None
    }
}

/// Read a JSVal as an integral address (Number or BigInt).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn jsval_to_usize(cx: *mut JSContext, v: JSVal) -> Result<usize, String> {
    if v.is_double() {
        let d = v.to_double();
        if d.fract() != 0.0 || d < 0.0 {
            return Err("pointer must be a non-negative integer".to_string());
        }
        Ok(d as usize)
    } else if v.is_int32() {
        // Small integer literals (e.g. qsort's scratch base 0x100000) are
        // int32-tagged in SM — a Number is a Number regardless of tag.
        let n = v.to_int32();
        if n < 0 {
            return Err("pointer must be a non-negative integer".to_string());
        }
        Ok(n as usize)
    } else if v.is_bigint() {
        let s = {
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let v_root = v);
            let jsstr = mozjs::rust::ToString(cx_ref, v_root.handle());
            if jsstr.is_null() {
                return Err("BigInt pointer conversion failed".to_string());
            }
            mozjs::jsval::StringValue(&*jsstr)
        };
        crate::js_to_rust_string(cx, s)
            .parse::<usize>()
            .map_err(|_| "BigInt pointer out of range".to_string())
    } else {
        Err("pointer must be a Number or BigInt".to_string())
    }
}

/// One converted argument, stored OWNED in a Vec whose memory outlives the
/// ffi_call (Arg slots point INTO this Vec — loop-local stack slots would
/// dangle by call time, the BCE crash class this restructure fixes).
enum FfiArgSlot {
    U8(u8),
    I8(i8),
    U16(u16),
    I16(i16),
    U32(u32),
    I32(i32),
    U64(u64),
    I64(i64),
    F32(f32),
    F64(f64),
    Ptr(usize),
    NullPtr,
}

impl FfiArgSlot {
    fn arg(&self) -> libffi::middle::Arg {
        use libffi::middle::arg;
        match self {
            FfiArgSlot::U8(v) => arg(v),
            FfiArgSlot::I8(v) => arg(v),
            FfiArgSlot::U16(v) => arg(v),
            FfiArgSlot::I16(v) => arg(v),
            FfiArgSlot::U32(v) => arg(v),
            FfiArgSlot::I32(v) => arg(v),
            FfiArgSlot::U64(v) => arg(v),
            FfiArgSlot::I64(v) => arg(v),
            FfiArgSlot::F32(v) => arg(v),
            FfiArgSlot::F64(v) => arg(v),
            FfiArgSlot::Ptr(v) => arg(v),
            FfiArgSlot::NullPtr => {
                let n: *const ::std::os::raw::c_char = ::std::ptr::null();
                arg(&n)
            }
        }
    }
}

/// Convert one call argument into an owned slot per its TypeSpec. The CString
/// bytes live in `keepalive` (alive until after the call); the slot holds the
/// char pointer.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn spec_arg(
    cx: *mut JSContext,
    spec: TypeSpec,
    v: JSVal,
    keepalive: &mut Vec<ZBox>,
) -> Result<FfiArgSlot, String> {
    let bad = |want: &str| format!("argument must be {}", want);
    match spec {
        TypeSpec::Bool => {
            if !v.is_boolean() {
                return Err(bad("a boolean"));
            }
            let b: ::std::os::raw::c_uchar = if v.to_boolean() { 1 } else { 0 };
            Ok(FfiArgSlot::U8(b))
        }
        TypeSpec::U8 => {
            if !(v.is_int32() || v.is_double()) {
                return Err(bad("a number"));
            }
            let b = v.to_number() as u8;
            Ok(FfiArgSlot::U8(b))
        }
        TypeSpec::I8 => {
            if !(v.is_int32() || v.is_double()) {
                return Err(bad("a number"));
            }
            let b = v.to_number() as i8;
            Ok(FfiArgSlot::I8(b))
        }
        TypeSpec::U16 => {
            if !(v.is_int32() || v.is_double()) {
                return Err(bad("a number"));
            }
            let b = v.to_number() as u16;
            Ok(FfiArgSlot::U16(b))
        }
        TypeSpec::I16 => {
            if !(v.is_int32() || v.is_double()) {
                return Err(bad("a number"));
            }
            let b = v.to_number() as i16;
            Ok(FfiArgSlot::I16(b))
        }
        TypeSpec::U32 => {
            if !(v.is_int32() || v.is_double()) {
                return Err(bad("a number"));
            }
            let b = v.to_number() as u32;
            Ok(FfiArgSlot::U32(b))
        }
        TypeSpec::I32 => {
            if !(v.is_int32() || v.is_double()) {
                return Err(bad("a number"));
            }
            let b = v.to_number() as i32;
            Ok(FfiArgSlot::I32(b))
        }
        TypeSpec::U64 | TypeSpec::Usize => {
            let n = val_to_u64_bigint_ok(cx, v)?;
            Ok(FfiArgSlot::U64(n))
        }
        TypeSpec::I64 | TypeSpec::Isize => {
            let n = val_to_i64(cx, v)?;
            Ok(FfiArgSlot::I64(n))
        }
        TypeSpec::F32 => {
            if !(v.is_int32() || v.is_double()) {
                return Err(bad("a number"));
            }
            let b = v.to_number() as f32;
            Ok(FfiArgSlot::F32(b))
        }
        TypeSpec::F64 => {
            if !(v.is_int32() || v.is_double()) {
                return Err(bad("a number"));
            }
            let b = v.to_number();
            Ok(FfiArgSlot::F64(b))
        }
        TypeSpec::Ptr => {
            let p = jsval_to_usize(cx, v)?;
            Ok(FfiArgSlot::Ptr(p))
        }
        // Callback slot: accept ONLY a callback()-built FfiCallback wrapper —
        // its closure code pointer is what crosses the boundary. (Raw
        // pointers still go through `ptr`; functions would be silently
        // truncated to an address here, so anything callable is rejected.)
        TypeSpec::JsFunction => {
            if !v.is_object() {
                return Err(bad("a bun:ffi callback object"));
            }
            let obj = v.to_object();
            let mut slot = UndefinedValue();
            JS_GetReservedSlot(obj, SLOT_CB, &mut slot);
            if !(slot.is_double() && (slot.asBits_ & 0xFFFF000000000000) == 0) {
                return Err(bad("a bun:ffi callback object"));
            }
            let cb_ptr = slot.to_private() as *mut FfiCallback;
            if cb_ptr.is_null() {
                return Err(bad("a bun:ffi callback object"));
            }
            let code = (*cb_ptr).code_ptr() as usize;
            Ok(FfiArgSlot::Ptr(code))
        }
        TypeSpec::CString => {
            if !v.is_string() {
                // Null pointer passthrough for optional cstring args.
                if v.is_null() {
                    return Ok(FfiArgSlot::NullPtr);
                }
                return Err(bad("a string"));
            }
            let s = crate::js_to_rust_string(cx, v);
            let z = ZBox::from_bytes(s.as_bytes());
            keepalive.push(z);
            let p: usize = keepalive.last().unwrap().as_ptr() as usize;
            Ok(FfiArgSlot::Ptr(p))
        }
        TypeSpec::Void => Err("void is not a valid argument type".to_string()),
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn val_to_u64_bigint_ok(cx: *mut JSContext, v: JSVal) -> Result<u64, String> {
    if v.is_bigint() {
        let s = {
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let v_root = v);
            let jsstr = mozjs::rust::ToString(cx_ref, v_root.handle());
            if jsstr.is_null() {
                return Err("BigInt conversion failed".to_string());
            }
            mozjs::jsval::StringValue(&*jsstr)
        };
        crate::js_to_rust_string(cx, s)
            .parse::<u64>()
            .map_err(|_| "u64 argument out of range".to_string())
    } else if v.is_number() {
        // to_number() covers BOTH int32- and double-tagged numbers — calling
        // to_double() directly asserts on int32-tagged literals (e.g. 2, 4).
        let d = v.to_number();
        if d.fract() != 0.0 || d < 0.0 {
            return Err("u64 argument must be a non-negative integer".to_string());
        }
        Ok(d as u64)
    } else {
        Err("u64 argument must be a Number or BigInt".to_string())
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn val_to_i64(cx: *mut JSContext, v: JSVal) -> Result<i64, String> {
    if v.is_bigint() {
        let s = {
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let v_root = v);
            let jsstr = mozjs::rust::ToString(cx_ref, v_root.handle());
            if jsstr.is_null() {
                return Err("BigInt conversion failed".to_string());
            }
            mozjs::jsval::StringValue(&*jsstr)
        };
        crate::js_to_rust_string(cx, s)
            .parse::<i64>()
            .map_err(|_| "i64 argument out of range".to_string())
    } else if v.is_number() {
        // to_number(): int32- and double-tagged safe (see val_to_u64_bigint_ok).
        let d = v.to_number();
        if d.fract() != 0.0 {
            return Err("i64 argument must be an integer".to_string());
        }
        Ok(d as i64)
    } else {
        Err("i64 argument must be a Number or BigInt".to_string())
    }
}

/// The bound-function call body: converts JS args per the spec, invokes the
/// foreign function through the prepped CIF, converts the result.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ffi_fn_call(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let callee = args.calleev();
    if !callee.is_object() {
        let msg = ZBox::from_bytes("ffi: invalid callee".as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let callee_obj = callee.to_object());

    let Some(spec_ptr) = fn_hidden_ptr(cx, callee_obj.get(), "_fnSpec") else {
        let msg = ZBox::from_bytes("ffi: invalid bound function".as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    };
    let spec = &*(spec_ptr as *const FfiFnSpec);

    if (argc as usize) < spec.args.len() {
        let msg = ZBox::from_vec(format!(
            "ffi: expected {} arguments, got {}",
            spec.args.len(),
            argc
        )
        .into_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    }

    // keepalive owns the CString allocations for the duration of the call; the
    // slots Vec owns every argument VALUE (Arg views point into it).
    let mut keepalive: Vec<ZBox> = Vec::new();
    let mut slots: Vec<FfiArgSlot> = Vec::with_capacity(spec.args.len());
    for (i, a_spec) in spec.args.iter().enumerate() {
        match spec_arg(cx, *a_spec, *args.get(i as u32).ptr, &mut keepalive) {
            Ok(slot) => slots.push(slot),
            Err(e) => {
                let msg =
                    ZBox::from_vec(format!("ffi argument {}: {}", i + 1, e).into_bytes());
                JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
                return false;
            }
        }
    }
    // Partition the slots into the two SysV register banks. Arguments pass in
    // the order of their class (integer args fill rdi..r9 in declaration
    // order, SSE args fill xmm0..7 independently) — exactly how the banks are
    // built here. Register overflow (>6 int / >8 sse) is rejected up front:
    // the trampoline covers register-passed primitives only.
    let mut ints = [0u64; 6];
    let mut sses = [0f64; 8];
    let mut nint = 0usize;
    let mut nsse = 0usize;
    for slot in &slots {
        match slot {
            FfiArgSlot::F32(_) | FfiArgSlot::F64(_) => {
                if nsse == 8 {
                    report_ffi_error(cx, "ffi: more than 8 SSE arguments is not supported");
                    return false;
                }
                let v = match slot {
                    FfiArgSlot::F32(v) => *v as f64,
                    FfiArgSlot::F64(v) => *v,
                    _ => unreachable!(),
                };
                sses[nsse] = v;
                nsse += 1;
            }
            _ => {
                if nint == 6 {
                    report_ffi_error(cx, "ffi: more than 6 integer arguments is not supported");
                    return false;
                }
                ints[nint] = match slot {
                    FfiArgSlot::U8(v) => *v as u64,
                    FfiArgSlot::I8(v) => *v as i64 as u64,
                    FfiArgSlot::U16(v) => *v as u64,
                    FfiArgSlot::I16(v) => *v as i64 as u64,
                    FfiArgSlot::U32(v) => *v as u64,
                    FfiArgSlot::I32(v) => *v as i64 as u64,
                    FfiArgSlot::U64(v) => *v,
                    FfiArgSlot::I64(v) => *v as u64,
                    FfiArgSlot::Ptr(v) => *v as u64,
                    FfiArgSlot::NullPtr => 0,
                    _ => unreachable!(),
                };
                nint += 1;
            }
        }
    }

    #[cfg(target_arch = "x86_64")]
    let ret = sysv_call(spec.fn_ptr, &mut ints, &mut sses);
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = (&mut ints, &mut sses);
        report_ffi_error(cx, "ffi: dlopen-bound calls are implemented for x86-64 only");
        return false;
    }

    match spec.ret {
        TypeSpec::Void => {
            args.rval().set(UndefinedValue());
        }
        TypeSpec::Bool => {
            args.rval().set(BooleanValue(ret.rax & 0xFF != 0));
        }
        TypeSpec::U8 => {
            args.rval().set(DoubleValue((ret.rax & 0xFF) as f64));
        }
        TypeSpec::I8 => {
            args.rval().set(DoubleValue((ret.rax as u8 as i8) as f64));
        }
        TypeSpec::U16 => {
            args.rval().set(DoubleValue((ret.rax & 0xFFFF) as f64));
        }
        TypeSpec::I16 => {
            args.rval().set(DoubleValue((ret.rax as u16 as i16) as f64));
        }
        TypeSpec::U32 => {
            args.rval().set(DoubleValue((ret.rax as u32) as f64));
        }
        TypeSpec::I32 => {
            args.rval().set(mozjs::jsval::Int32Value(ret.rax as u32 as i32));
        }
        TypeSpec::U64 | TypeSpec::Usize => {
            args.rval().set(bigint_from_decimal(cx, &ret.rax.to_string()));
        }
        TypeSpec::I64 | TypeSpec::Isize => {
            args.rval().set(bigint_from_decimal(cx, &(ret.rax as i64).to_string()));
        }
        TypeSpec::F32 => {
            args.rval().set(DoubleValue(ret.xmm0 as f32 as f64));
        }
        TypeSpec::F64 => {
            args.rval().set(DoubleValue(ret.xmm0));
        }
        TypeSpec::Ptr => {
            args.rval().set(DoubleValue(ret.rax as f64));
        }
        // A C function returning a function pointer surfaces it as a raw
        // address (same numeric shape as Ptr) — building a JS wrapper for an
        // untyped foreign fn is out of scope for the descriptor contract.
        TypeSpec::JsFunction => {
            args.rval().set(DoubleValue(ret.rax as f64));
        }
        TypeSpec::CString => {
            let r = ret.rax as *const ::std::os::raw::c_char;
            if r.is_null() {
                args.rval().set(NullValue());
            } else {
                let bytes = ::std::ffi::CStr::from_ptr(r).to_bytes();
                let js = JS_NewStringCopyN(cx, bytes.as_ptr() as *const _, bytes.len());
                if js.is_null() {
                    args.rval().set(NullValue());
                } else {
                    args.rval().set(mozjs::jsval::StringValue(&*js));
                }
            }
        }
    }
    true
}

/// Report an ffi bridge error and return the native-call failure sentinel.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn report_ffi_error(cx: *mut JSContext, msg: &str) {
    let m = ZBox::from_bytes(msg.as_bytes());
    JS_ReportErrorUTF8(cx, c"%s".as_ptr(), m.as_ptr());
}

/// Create a JS BigInt from a u64/i64 via the realm's `BigInt` callable with a
/// decimal string (exact — no f64 rounding for values beyond 2^53).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn bigint_from_decimal(cx: *mut JSContext, decimal: &str) -> JSVal {    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    let global = CurrentGlobalOrNull(cx);
    if global.is_null() {
        return DoubleValue(decimal.parse::<f64>().unwrap_or(0.0));
    }
    rooted!(&in(cx_ref) let global_root = global);
    let mut bi_fn = UndefinedValue();
    JS_GetProperty(
        cx,
        global_root.handle().into(),
        c"BigInt".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut bi_fn,
        },
    );
    if !bi_fn.is_object() {
        return DoubleValue(decimal.parse::<f64>().unwrap_or(0.0));
    }
    rooted!(&in(cx_ref) let fn_val = bi_fn);
    let c_s = ZBox::from_bytes(decimal.as_bytes());
    let s_js = JS_NewStringCopyZ(cx, c_s.as_ptr());
    if s_js.is_null() {
        return DoubleValue(decimal.parse::<f64>().unwrap_or(0.0));
    }
    rooted!(&in(cx_ref) let sv = mozjs::jsval::StringValue(&*s_js));
    let elems = [*sv.handle()];
    let call_args = HandleValueArray {
        length_: 1,
        elements_: elems.as_ptr(),
    };
    rooted!(&in(cx_ref) let undef_this = ::std::ptr::null_mut::<JSObject>());
    let mut rval = UndefinedValue();
    let called = JS_CallFunctionValue(
        cx,
        undef_this.handle().into(),
        fn_val.handle().into(),
        &call_args,
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        },
    );
    if !called {
        JS_ClearPendingException(cx);
        return DoubleValue(decimal.parse::<f64>().unwrap_or(0.0));
    }
    rval
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

        // suffix — platform library suffixes (Bun API parity).
        {
            rooted!(&in(cx) let suffix_obj = w2::JS_NewPlainObject(cx));
            if !suffix_obj.get().is_null() {
                let sh = suffix_obj.handle().into();
                for (k, v) in [
                    ("dll", ".dll"),
                    ("so", ".so"),
                    ("so.6", ".so.6"),
                    ("dylib", ".dylib"),
                    ("node", ".node"),
                ] {
                    let c_v = ZBox::from_bytes(v.as_bytes());
                    let v_js = JS_NewStringCopyZ(cx.raw_cx(), c_v.as_ptr());
                    if !v_js.is_null() {
                        rooted!(&in(cx) let vv = mozjs::jsval::StringValue(&*v_js));
                        JS_DefineProperty(
                            cx.raw_cx(),
                            sh,
                            ZBox::from_bytes(k.as_bytes()).as_ptr(),
                            vv.handle().into(),
                            (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
                        );
                    }
                }
                rooted!(&in(cx) let sv = ObjectValue(suffix_obj.get()));
                JS_DefineProperty(
                    cx.raw_cx(),
                    obj.handle().into(),
                    c"suffix".as_ptr(),
                    sv.handle().into(),
                    (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
                );
            }
        }

        // types — FFI type tokens for dlopen(path, symbols) descriptors.
        {
            rooted!(&in(cx) let types_obj = w2::JS_NewPlainObject(cx));
            if !types_obj.get().is_null() {
                let th = types_obj.handle().into();
                for name in [
                    "void",
                    "bool",
                    "u8",
                    "i8",
                    "u16",
                    "i16",
                    "u32",
                    "i32",
                    "u64",
                    "i64",
                    "usize",
                    "isize",
                    "f32",
                    "f64",
                    "ptr",
                    "cstring",
                    "js_function",
                ] {
                    rooted!(&in(cx) let token = w2::JS_NewPlainObject(cx));
                    if token.get().is_null() {
                        continue;
                    }
                    let c_n = ZBox::from_bytes(name.as_bytes());
                    let n_js = JS_NewStringCopyZ(cx.raw_cx(), c_n.as_ptr());
                    if !n_js.is_null() {
                        rooted!(&in(cx) let nv = mozjs::jsval::StringValue(&*n_js));
                        JS_DefineProperty(
                            cx.raw_cx(),
                            token.handle().into(),
                            c"__ffiType".as_ptr(),
                            nv.handle().into(),
                            0,
                        );
                    }
                    rooted!(&in(cx) let tv = ObjectValue(token.get()));
                    JS_DefineProperty(
                        cx.raw_cx(),
                        th,
                        c_n.as_ptr(),
                        tv.handle().into(),
                        (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
                    );
                }
                rooted!(&in(cx) let tvv = ObjectValue(types_obj.get()));
                JS_DefineProperty(
                    cx.raw_cx(),
                    obj.handle().into(),
                    c"types".as_ptr(),
                    tvv.handle().into(),
                    (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
                );
            }
        }

        // toBuffer(pointer, byteLength, { free?: fnPtrNumber }) — zero-copy
        // Buffer view over foreign memory (external ArrayBuffer).
        w2::JS_DefineFunction(
            cx,
            obj.handle(),
            c"toBuffer".as_ptr(),
            Some(ffi_to_buffer),
            2,
            JSPROP_ENUMERATE as u32,
        );

        // CString class — read null-terminated strings from pointers.
        {
            rooted!(&in(cx) let null_proto_cs = ::std::ptr::null_mut::<JSObject>());
            let cs_proto = w2::JS_InitClass(
                cx,
                global.handle(),
                &FFI_CSTRING_CLASS,
                null_proto_cs.handle(),
                c"CString".as_ptr(),
                Some(cstring_constructor),
                1,
                ::std::ptr::null(),
                FFI_CSTRING_METHODS.as_ptr(),
                ::std::ptr::null(),
                ::std::ptr::null(),
            );
            if !cs_proto.is_null() {
                rooted!(&in(cx) let cs_proto_r = cs_proto);
                rooted!(&in(cx) let cs_ctor = JS_GetConstructor(cx.raw_cx(), cs_proto_r.handle().into()));
                if !cs_ctor.get().is_null() {
                    rooted!(&in(cx) let cv = ObjectValue(cs_ctor.get()));
                    JS_DefineProperty(
                        cx.raw_cx(),
                        obj.handle().into(),
                        c"CString".as_ptr(),
                        cv.handle().into(),
                        (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
                    );
                }
            }
        }

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
    if ptr.is_null() { None } else { Some(ptr) }
}

/// Module-level dlopen function: `dlopen(path)` → FfiLibrary JS object, or
/// the Bun contract `dlopen(path, symbols)` → `{ name: callable, ... }` where
/// each symbol descriptor `{ args: [type...], returns: type }` preps a libffi
/// CIF and the returned JS function converts arguments/results per spec.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ffi_dlopen(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
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

    let lib = match FfiLibrary::dlopen(&path) {
        Ok(l) => l,
        Err(e) => {
            let msg = ZBox::from_bytes(e.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
    };

    // One-arg form: the FfiLibrary introspection surface (close/symbol).
    if argc < 2 || !(*args.get(1).ptr).is_object() {
        let lib_ptr = Box::into_raw(Box::new(lib)) as *const ::std::os::raw::c_void;
        let obj = JS_NewObject(cx, &FFI_LIBRARY_CLASS);
        if obj.is_null() {
            let _ = Box::from_raw(lib_ptr as *mut FfiLibrary);
            args.rval().set(NullValue());
            return true;
        }
        let val = PrivateValue(lib_ptr);
        JS_SetReservedSlot(obj, SLOT_LIB, &val);

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
        return true;
    }

    // Two-arg Bun contract: dlopen(path, { sym: { args, returns } }).
    let symbols_obj = (*args.get(1).ptr).to_object();
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let lib_obj = symbols_obj);
    rooted!(&in(cx_ref) let exports_obj = w2::JS_NewPlainObject(cx_ref));
    if exports_obj.get().is_null() {
        args.rval().set(NullValue());
        return true;
    }

    // Collect own string-keyed symbol names (IdVector enumeration — same
    // pattern as node_vm::collect_sandbox_properties; symbol/int keys cannot
    // name C symbols and are skipped).
    let mut names: Vec<String> = Vec::new();
    {
        let mut ids = mozjs::rust::IdVector::new(cx);
        if !GetPropertyKeys(cx, lib_obj.handle().into(), JSITER_OWNONLY, ids.handle_mut()) {
            let msg = ZBox::from_bytes("dlopen: failed to enumerate symbols".as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
        for jsid in &*ids {
            if !jsid.is_string() {
                continue;
            }
            let key_str_ptr = jsid.to_string();
            if key_str_ptr.is_null() {
                continue;
            }
            names.push(crate::js_to_rust_string(
                cx,
                mozjs::jsval::StringValue(unsafe { &*key_str_ptr }),
            ));
        }
    }

    for name in names {
        let c_name = ZBox::from_bytes(name.as_bytes());
        let mut name_val = UndefinedValue();
        JS_GetProperty(
            cx,
            lib_obj.handle().into(),
            c_name.as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut name_val,
            },
        );
        if !name_val.is_object() {
            let msg = ZBox::from_bytes("dlopen: symbol descriptor must be an object".as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
        let desc_obj = name_val.to_object();
        // BCE-root discipline: rooted for the whole descriptor parse (inline
        // block-scoped roots dangle past their closing brace).
        rooted!(&in(cx_ref) let desc_root = desc_obj);

        // Resolve the foreign symbol.
        let fn_ptr = match lib.symbol(&name) {
            Ok(p) if !p.is_null() => p,
            Ok(_) => {
                let msg = ZBox::from_vec(
                    format!("dlopen: symbol {} resolved to null", name).into_bytes(),
                );
                JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
                return false;
            }
            Err(e) => {
                let msg = ZBox::from_vec(format!("dlopen: {}: {}", name, e).into_bytes());
                JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
                return false;
            }
        };

        // Parse args / returns.
        let mut arg_specs: Vec<TypeSpec> = Vec::new();
        {
            let mut args_v = UndefinedValue();
            JS_GetProperty(
                cx,
                desc_root.handle().into(),
                c"args".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut args_v,
                },
            );
            if args_v.is_object() {
                rooted!(&in(cx_ref) let arr = args_v.to_object());
                let mut len: u32 = 0;
                if w2::GetArrayLength(cx_ref, arr.handle().into(), &mut len) {
                    for i in 0..len {
                        let mut tv = UndefinedValue();
                        JS_GetElement(
                            cx,
                            arr.handle().into(),
                            i,
                            MutableHandle::<Value> {
                                _phantom_0: ::std::marker::PhantomData,
                                ptr: &mut tv,
                            },
                        );
                        match TypeSpec::parse(cx, tv) {
                            Ok(t) => arg_specs.push(t),
                            Err(e) => {
                                let msg = ZBox::from_vec(
                                    format!("dlopen: {} args: {}", name, e).into_bytes(),
                                );
                                JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
                                return false;
                            }
                        }
                    }
                }
            }
        }
        let mut ret_v = UndefinedValue();
        JS_GetProperty(
            cx,
            desc_root.handle().into(),
            c"returns".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut ret_v,
            },
        );
        let ret_spec = if ret_v.is_undefined() {
            TypeSpec::Void
        } else {
            match TypeSpec::parse(cx, ret_v) {
                Ok(t) => t,
                Err(e) => {
                    let msg =
                        ZBox::from_vec(format!("dlopen: {} returns: {}", name, e).into_bytes());
                    JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
                    return false;
                }
            }
        };

        let spec_box = Box::new(FfiFnSpec {
            args: arg_specs,
            ret: ret_spec,
            fn_ptr,
        });
        let spec_ptr = Box::into_raw(spec_box) as *const ::std::os::raw::c_void;

        // Build the JS callable.
        let f = JS_NewFunction(cx, Some(ffi_fn_call), arg_count_max(), 0, name_placeholder(&name));
        if f.is_null() {
            drop(Box::from_raw(spec_ptr as *mut FfiFnSpec));
            let msg = ZBox::from_bytes("dlopen: failed to create bound function".as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
        let fobj = JS_GetFunctionObject(f);
        if fobj.is_null() {
            drop(Box::from_raw(spec_ptr as *mut FfiFnSpec));
            let msg = ZBox::from_bytes("dlopen: failed to materialize bound function".as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
        rooted!(&in(cx_ref) let fo = fobj);
        let fo_h = fo.handle().into();
        rooted!(&in(cx_ref) let spec_v = PrivateValue(spec_ptr));
        JS_DefineProperty(cx, fo_h, c"_fnSpec".as_ptr(), spec_v.handle().into(), 0);
        rooted!(&in(cx_ref) let ptr_v = DoubleValue(fn_ptr as usize as f64));
        JS_DefineProperty(cx, fo_h, c"_fnPtr".as_ptr(), ptr_v.handle().into(), 0);

        rooted!(&in(cx_ref) let fv = ObjectValue(fo.get()));
        let c_name = ZBox::from_bytes(name.as_bytes());
        JS_DefineProperty(
            cx,
            exports_obj.handle().into(),
            c_name.as_ptr(),
            fv.handle().into(),
            (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
        );
    }

    // Dual entry: the exports object carries its symbols directly (Bun
    // contract `lib.getpid()`) AND as a `lib.symbols` namespace (Deno/Koffi
    // convention `lib.symbols.getpid()`). The self-reference exposes the
    // SAME callable objects — one registration, two faces, no divergence.
    rooted!(&in(cx_ref) let self_v = ObjectValue(exports_obj.get()));
    JS_DefineProperty(
        cx,
        exports_obj.handle().into(),
        c"symbols".as_ptr(),
        self_v.handle().into(),
        (JSPROP_ENUMERATE | JSPROP_PERMANENT) as u32,
    );

    args.rval().set(ObjectValue(exports_obj.get()));
    true
}

/// JS_NewFunction's name parameter must be a static C string — the per-symbol
/// name is carried by the property key; a stable placeholder keeps the call
/// legal while stack traces show the bound export via the property name.
fn name_placeholder(_name: &str) -> *const ::std::os::raw::c_char {
    c"ffi".as_ptr()
}

fn arg_count_max() -> u32 {
    16
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ffi_library_close(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let thisv = args.thisv();

    let lib_ptr = match get_lib(cx, thisv) {
        Some(p) => p,
        None => {
            let msg = ZBox::from_bytes("FfiLibrary.close: invalid FfiLibrary object".as_bytes());
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
unsafe extern "C" fn ffi_library_symbol(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let thisv = args.thisv();

    let lib_ptr = match get_lib(cx, thisv) {
        Some(p) => p,
        None => {
            let msg = ZBox::from_bytes("FfiLibrary.symbol: invalid FfiLibrary object".as_bytes());
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
/// `callback(...)` — wrap a JS function as a C-callable closure.
///
/// Accepted shapes (Bun contract `callback(argCount, returns, fn)` plus
/// explicit per-arg typing):
///   - `callback(fn)`                       → 0 args, void return
///   - `callback(argCount, returns, fn)`    → argCount × f64 args
///   - `callback([types], returns, fn)`     → per-type args
/// Every non-function argument is classified by shape (array → arg type
/// list, number → arg count, anything TypeSpec::parse accepts → return
/// type) so any argument order works.
unsafe extern "C" fn ffi_callback(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let mut js_fn: *mut JSObject = ::std::ptr::null_mut();
    let mut arg_types: Vec<TypeSpec> = Vec::new();
    let mut ret_spec = TypeSpec::Void;
    let mut have_arg_shape = false;

    for i in 0..argc {
        let v = *args.get(i).ptr;
        if v.is_object() && IsCallable(v.to_object()) {
            js_fn = v.to_object();
            continue;
        }
        if js_fn.is_null() && !have_arg_shape && v.is_object() {
            // Could be an arg-type array or a `types` token — tokens carry
            // __ffiType and parse via TypeSpec::parse, arrays parse
            // element-wise.
            let obj = v.to_object();
            let mut len: u32 = 0;
            let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let arr_root = obj);
            if w2::GetArrayLength(cx_ref, arr_root.handle().into(), &mut len) {
                for j in 0..len {
                    let mut tv = UndefinedValue();
                    JS_GetElement(
                        cx,
                        arr_root.handle().into(),
                        j,
                        MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &mut tv,
                        },
                    );
                    match TypeSpec::parse(cx, tv) {
                        Ok(t) => arg_types.push(t),
                        Err(e) => {
                            let msg = ZBox::from_vec(
                                format!("bun:ffi callback: args[{}]: {}", j, e).into_bytes(),
                            );
                            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
                            return false;
                        }
                    }
                }
                have_arg_shape = true;
                continue;
            }
        }
        if v.is_int32() || v.is_double() {
            // argCount form: N × f64 (JS numbers — the Bun-documented shape).
            let n = if v.is_int32() {
                v.to_int32()
            } else {
                v.to_double() as i32
            };
            if n < 0 || n > 16 {
                let msg = ZBox::from_bytes(
                    "bun:ffi callback: arg count must be 0..=16".as_bytes(),
                );
                JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
                return false;
            }
            if !have_arg_shape {
                arg_types = ::std::iter::repeat(TypeSpec::F64).take(n as usize).collect();
                have_arg_shape = true;
            }
            continue;
        }
        // Anything else must parse as the return type (string name or token).
        match TypeSpec::parse(cx, v) {
            Ok(t) => ret_spec = t,
            Err(e) => {
                let msg =
                    ZBox::from_vec(format!("bun:ffi callback: returns: {}", e).into_bytes());
                JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
                return false;
            }
        }
    }

    if js_fn.is_null() {
        let msg = ZBox::from_bytes("bun:ffi callback: function argument required".as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    }

    // Build the FfiCallback and stash it in a heap Box whose pointer is stored
    // in the JS wrapper's reserved slot.
    let cb = match ::std::panic::catch_unwind(::std::panic::AssertUnwindSafe(|| {
        FfiCallback::new(cx, js_fn, arg_types, ret_spec)
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
    if ptr.is_null() { None } else { Some(ptr) }
}

/// FfiCallback.prototype.close() — releases the libffi closure.
/// @trace REQ-ENG-009 [entity:FfiCallback]
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ffi_callback_close(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let thisv = args.thisv();

    let cb_ptr = match get_callback(cx, thisv) {
        Some(p) => p,
        None => {
            let msg = ZBox::from_bytes("FfiCallback.close: invalid FfiCallback object".as_bytes());
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
unsafe extern "C" fn ffi_callback_ptr(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
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

// ── CString — read null-terminated strings from foreign pointers ───────────

const FFI_CSTRING_METHODS: &[JSFunctionSpec] = &[
    JSFunctionSpec {
        name: JSPropertySpec_Name {
            string_: c"toString".as_ptr(),
        },
        call: JSNativeWrapper {
            op: Some(cstring_to_string),
            info: ::std::ptr::null_mut(),
        },
        nargs: 0,
        flags: JSPROP_ENUMERATE as u16,
        selfHostedName: ::std::ptr::null_mut(),
    },
    JSFunctionSpec::ZERO,
];

/// `new CString(pointer)` — reads the null-terminated string AT the pointer
/// (the read happens at construction; instances carry `.length` and the
/// string, exposed via `toString()` / value coercion).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cstring_constructor(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let this = JS_NewObjectForConstructor(cx, &FFI_CSTRING_CLASS, &args);
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

    if argc < 1 {
        let msg = ZBox::from_bytes("CString: pointer argument required".as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    }
    let ptr = match jsval_to_usize(cx, *args.get(0).ptr) {
        Ok(p) => p,
        Err(e) => {
            let msg = ZBox::from_bytes(e.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
    };

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let this_r = this);
    let this_h = this_r.handle().into();

    let text = if ptr == 0 {
        String::new()
    } else {
        ::std::ffi::CStr::from_ptr(ptr as *const ::std::os::raw::c_char)
            .to_string_lossy()
            .into_owned()
    };
    let c_t = ZBox::from_bytes(text.as_bytes());
    let t_js = JS_NewStringCopyZ(cx, c_t.as_ptr());
    if t_js.is_null() {
        args.rval().set(ObjectValue(this_r.get()));
        return true;
    }
    rooted!(&in(cx_ref) let tv = mozjs::jsval::StringValue(&*t_js));
    JS_DefineProperty(cx, this_h, c"_text".as_ptr(), tv.handle().into(), 0);
    rooted!(&in(cx_ref) let len_v = DoubleValue(text.len() as f64));
    JS_DefineProperty(
        cx,
        this_h,
        c"length".as_ptr(),
        len_v.handle().into(),
        (JSPROP_ENUMERATE | JSPROP_READONLY) as u32,
    );
    // String coercion (template literals, `${}`).
    let sym_key = mozjs_sys::jsapi::JS::GetWellKnownSymbolKey(
        cx,
        mozjs_sys::jsapi::JS::SymbolCode::toPrimitive,
    );
    let prim = JS_NewFunction(cx, Some(cstring_toprimitive), 1, 0, c"[toPrimitive]".as_ptr());
    if !prim.is_null() {
        let prim_obj = JS_GetFunctionObject(prim);
        if !prim_obj.is_null() {
            rooted!(&in(cx_ref) let pv = ObjectValue(prim_obj));
            JS_DefinePropertyById2(
                cx,
                this_h,
                Handle::from_marked_location(&sym_key),
                pv.handle().into(),
                0,
            );
        }
    }

    args.rval().set(ObjectValue(this_r.get()));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cstring_to_string(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    let thisv = args.thisv();
    if !thisv.is_object() {
        args.rval().set(mozjs::jsval::StringValue(
            &*(JS_NewStringCopyZ(cx, b"\0".as_ptr() as *const _)),
        ));
        return true;
    }
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let this_obj = thisv.to_object());
    let mut v = UndefinedValue();
    JS_GetProperty(
        cx,
        this_obj.handle().into(),
        c"_text".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut v,
        },
    );
    if v.is_string() {
        args.rval().set(v);
    } else {
        args.rval().set(UndefinedValue());
    }
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cstring_toprimitive(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    cstring_to_string(cx, _argc, vp)
}

// ── toBuffer(pointer, byteLength, opts?) — zero-copy foreign-memory view ────

/// External free callback for toBuffer's optional `opts.free` (a raw C
/// `free(void*)`-shaped function pointer). Invoked by the GC when the
/// ArrayBuffer is collected — must not touch JSAPI.
unsafe extern "C" fn external_free_fn(
    contents: *mut c_void,
    user: *mut c_void,
) {
    // user carries the free() function pointer itself. NULL user = BORROWED
    // memory (no opts.free): the view does not own it, so release NOTHING —
    // the previous libc::free here freed foreign pointers at GC time
    // (invalid frees / heap corruption for any borrowed view).
    if user.is_null() {
        return;
    }
    let free_fn: extern "C" fn(*mut c_void) = ::std::mem::transmute(user as *const ());
    free_fn(contents);
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn ffi_to_buffer(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    if argc < 2 {
        let msg = ZBox::from_bytes("toBuffer(pointer, byteLength) requires 2 arguments".as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    }
    let ptr = match jsval_to_usize(cx, *args.get(0).ptr) {
        Ok(p) => p,
        Err(e) => {
            let msg = ZBox::from_bytes(e.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
            return false;
        }
    };
    let len_v = *args.get(1).ptr;
    let len = if len_v.is_int32() {
        len_v.to_int32() as usize
    } else if len_v.is_double() && len_v.to_double() >= 0.0 {
        len_v.to_double() as usize
    } else {
        let msg = ZBox::from_bytes("toBuffer: byteLength must be a non-negative number".as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    };
    if ptr == 0 || len == 0 {
        let msg = ZBox::from_bytes("toBuffer: pointer and byteLength must be non-zero".as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    }

    // Optional { free: <fn ptr number> } — the free function runs when the
    // buffer is GC'd. Without it the memory must stay valid at the caller's
    // discretion (the view does NOT own it).
    let mut free_user: *mut c_void = ::std::ptr::null_mut();
    if argc >= 3 && (*args.get(2).ptr).is_object() {
        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;
        rooted!(&in(cx_ref) let opts = (*args.get(2).ptr).to_object());
        let mut free_v = UndefinedValue();
        JS_GetProperty(
            cx,
            opts.handle().into(),
            c"free".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut free_v,
            },
        );
        if !(free_v.is_undefined() || free_v.is_null()) {
            match jsval_to_usize(cx, free_v) {
                Ok(fp) if fp != 0 => free_user = fp as *mut c_void,
                _ => {
                    let msg = ZBox::from_bytes(
                        "toBuffer: opts.free must be a non-null function pointer number".as_bytes(),
                    );
                    JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
                    return false;
                }
            }
        }
    }

    // Ownership contract (doc above: "the view does NOT own it" without
    // opts.free): the deleter is ALWAYS registered (this SM build's
    // BufferContentsDeleter unconditionally calls freeFunc — a null fn
    // pointer crashes at GC), but external_free_fn releases NOTHING when no
    // free pointer was supplied. Previously it libc::free'd every borrowed
    // view at GC time (invalid frees / heap corruption).
    let ab = mozjs_sys::jsapi::glue::NewExternalArrayBuffer(
        cx,
        len,
        ptr as *mut c_void,
        Some(external_free_fn),
        free_user,
    );
    if ab.is_null() {
        let msg = ZBox::from_bytes("toBuffer: failed to create external buffer".as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), msg.as_ptr());
        return false;
    }
    // Buffer is a Uint8Array view over the external ArrayBuffer (no copy):
    // view it, rebind the prototype to Buffer.prototype and stamp _isBuffer —
    // same instance shape as globals::create_buffer_object.
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let ab_root = ab);
    let view = mozjs_sys::jsapi::JS_NewUint8ArrayWithBuffer(cx, ab_root.handle().into(), 0, len as i64);
    if view.is_null() {
        args.rval().set(ObjectValue(ab));
        return true;
    }
    rooted!(&in(cx_ref) let view_r = view);
        let global = CurrentGlobalOrNull(cx);
        if !global.is_null() {
            rooted!(&in(cx_ref) let global_root = global);
            let mut buffer_ctor = UndefinedValue();
            JS_GetProperty(
                cx,
                global_root.handle().into(),
                c"Buffer".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut buffer_ctor,
                },
            );
            if buffer_ctor.is_object() {
                rooted!(&in(cx_ref) let ctor_obj = buffer_ctor.to_object());
                let mut proto = UndefinedValue();
                JS_GetProperty(
                    cx,
                    ctor_obj.handle().into(),
                    c"prototype".as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut proto,
                    },
                );
                if proto.is_object() {
                    rooted!(&in(cx_ref) let proto_obj = proto.to_object());
                    mozjs_sys::jsapi::JS_SetPrototype(
                        cx,
                        view_r.handle().into(),
                        proto_obj.handle().into(),
                    );
                }
            }
        }
    rooted!(&in(cx_ref) let is_buf = BooleanValue(true));
    JS_DefineProperty(
        cx,
        view_r.handle().into(),
        c"_isBuffer".as_ptr(),
        is_buf.handle().into(),
        0,
    );
    args.rval().set(ObjectValue(view_r.get()));
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Isolation probe: raw libffi CIF call into libc atoi — no SpiderMonkey,
    /// no JS plumbing. If this crashes, the bug is in the Cif/Arg marshaling;
    /// if it passes, the bug is in the JS-side wiring.
    /// SysV trampoline isolation: getpid (zero-arg, int ret) and atoi
    /// (one pointer arg, int ret) through bao_sysv_call.
    #[test]
    fn test_sysv_trampoline_libc() {
        let lib = FfiLibrary::dlopen("libc.so.6").unwrap();
        {
            let getpid_ptr = lib.symbol("getpid").unwrap();
            let mut ints = [0u64; 6];
            let mut sses = [0f64; 8];
            let ret = unsafe { sysv_call(getpid_ptr, &mut ints, &mut sses) };
            assert_eq!(ret.rax as u32, ::std::process::id());
        }
        {
            let atoi_ptr = lib.symbol("atoi").unwrap();
            let c_str = b"42\0";
            let mut ints = [c_str.as_ptr() as u64, 0, 0, 0, 0, 0];
            let mut sses = [0f64; 8];
            let ret = unsafe { sysv_call(atoi_ptr, &mut ints, &mut sses) };
            assert_eq!(ret.rax as u32, 42);
        }
        {
            // SSE round: sqrt(2.25) = 1.5 (libm, not libc, exports it).
            let libm = FfiLibrary::dlopen("libm.so.6").unwrap();
            let sqrt_ptr = libm.symbol("sqrt").unwrap();
            let mut ints = [0u64; 6];
            let mut sses = [2.25f64, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
            let ret = unsafe { sysv_call(sqrt_ptr, &mut ints, &mut sses) };
            assert_eq!(ret.xmm0, 1.5);
        }
    }

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
        let cb = FfiCallback::new(::std::ptr::null_mut(), ::std::ptr::null_mut(), Vec::new(), TypeSpec::Void);
        assert!(
            !cb.code_ptr().is_null(),
            "FfiCallback must expose a non-null C code pointer"
        );
    }

    /// REQ-ENG-009 acceptance #2: code_ptr() is stable across calls while the
    /// FfiCallback is alive.
    /// @trace REQ-ENG-009 [test:TEST-ENG-009]
    #[test]
    fn test_ffi_callback_code_ptr_is_stable() {
        let cb = FfiCallback::new(::std::ptr::null_mut(), ::std::ptr::null_mut(), Vec::new(), TypeSpec::Void);
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
        let cb = FfiCallback::new(::std::ptr::null_mut(), ::std::ptr::null_mut(), Vec::new(), TypeSpec::Void);
        let _ptr = cb.code_ptr();
        drop(cb); // must not panic — exercises drop order (closure before data)
    }

    /// REQ-ENG-009 acceptance #2: multiple callbacks coexist, each with a
    /// distinct code pointer.
    /// @trace REQ-ENG-009 [test:TEST-ENG-009]
    #[test]
    fn test_ffi_callback_multiple_distinct_pointers() {
        let cb1 = FfiCallback::new(::std::ptr::null_mut(), ::std::ptr::null_mut(), Vec::new(), TypeSpec::Void);
        let cb2 = FfiCallback::new(::std::ptr::null_mut(), ::std::ptr::null_mut(), Vec::new(), TypeSpec::Void);
        assert_ne!(
            cb1.code_ptr(),
            cb2.code_ptr(),
            "distinct closures get distinct code pointers"
        );
    }

    /// REQ-ENG-009 acceptance #2: invoking the trampoline with null userdata is
    /// a safe no-op (the null-guard short-circuits before touching JS).
    /// @trace REQ-ENG-009 [test:TEST-ENG-009]
    #[test]
    fn test_ffi_callback_dispatch_null_userdata_is_noop() {
        let cb = FfiCallback::new(::std::ptr::null_mut(), ::std::ptr::null_mut(), Vec::new(), TypeSpec::Void);
        let fun: extern "C" fn() = unsafe { *cb._closure.instantiate_code_ptr() };
        // Must not crash — null cx/js_fn triggers the early return.
        fun();
    }
}
