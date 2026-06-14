//! JSC ABI compatibility macros.
//!
//! These macros generate SM-native function declarations from JSC-style
//! function signatures. In SM, the ABI is:
//! `unsafe extern "C" fn(*mut JSContext, argc: u32, vp: *mut JSVal) -> bool`

/// Declare a JSC host ABI function.
/// In SM, this generates a proper SM-native function declaration.
#[macro_export]
macro_rules! jsc_host_abi {
    (
        $(#[$meta:meta])*
        $vis:vis fn $name:ident(
            $cx:ident : *mut $cx_ty:ty,
            $argc:ident : u32,
            $vp:ident : *mut $vp_ty:ty
        ) -> $ret:ty $body:block
    ) => {
        $(#[$meta])*
        $vis unsafe extern "C" fn $name(
            $cx: *mut mozjs::jsapi::JSContext,
            $argc: u32,
            $vp: *mut mozjs::jsval::JSVal,
        ) -> bool {
            let _args = mozjs::jsapi::CallArgs::from_vp($vp, $argc);
            $body
        }
    };
    ($($arg:tt)*) => {
        // Fallback: no-op for non-standard signatures
    };
}

/// Declare a JSC ABI extern function.
/// In SM, this declares an extern "C" function with SM ABI.
#[macro_export]
macro_rules! jsc_abi_extern {
    (
        $(#[$meta:meta])*
        $vis:vis fn $name:ident(
            $cx:ident : *mut $cx_ty:ty,
            $argc:ident : u32,
            $vp:ident : *mut $vp_ty:ty
        ) -> bool
    ) => {
        $(#[$meta])*
        $vis unsafe extern "C" fn $name(
            $cx: *mut mozjs::jsapi::JSContext,
            $argc: u32,
            $vp: *mut mozjs::jsval::JSVal,
        ) -> bool;
    };
    ($($arg:tt)*) => {
        // Fallback: no-op for non-standard signatures
    };
}
