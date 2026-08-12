// @trace REQ-ENG-007 [api:node internal stream modules]
//
// Internal underscore-prefixed stream modules that re-export stream classes
// from the main `stream` builtin. In Bun these are 3-line modules:
//   _stream_duplex     → stream.Duplex
//   _stream_passthrough → stream.PassThrough
//   _stream_readable   → stream.Readable
//   _stream_transform  → stream.Transform
//   _stream_writable   → stream.Writable
//   _stream_wrap       → deprecation warning + re-export entire stream module
//
// Each module is resolved by looking up the `stream` builtin from gc_store
// and extracting the corresponding class.

use mozjs::jsapi::*;
use mozjs::jsval::{ObjectValue, UndefinedValue};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

/// Internal stream module definitions: (module_name, stream_property)
const STREAM_ALIASES: &[(&str, &str)] = &[
    ("_stream_duplex", "Duplex"),
    ("_stream_passthrough", "PassThrough"),
    ("_stream_readable", "Readable"),
    ("_stream_transform", "Transform"),
    ("_stream_writable", "Writable"),
];

/// Install all internal stream alias modules.
pub fn install(cx: &mut mozjs::context::JSContext) {
    for &(module_name, prop) in STREAM_ALIASES {
        install_stream_alias(cx, module_name, prop);
    }
    install_stream_wrap(cx);
}

/// Look up the `stream` builtin, extract the named class, and cache it
/// under the internal module name.
fn install_stream_alias(cx: &mut mozjs::context::JSContext, module_name: &str, prop: &str) {
    // Guard: never clobber a natively-implemented module.
    let cache_key = format!("builtin:{}", module_name);
    if let Some(existing) = crate::gc_store::gc_store_get(unsafe { cx.raw_cx() }, &cache_key) {
        if !existing.is_null() {
            return;
        }
    }

    let stream_key = "builtin:stream";
    let stream_obj = crate::gc_store::gc_store_get(unsafe { cx.raw_cx() }, stream_key);
    let Some(stream_obj) = stream_obj else { return };
    if stream_obj.is_null() {
        return;
    }

    unsafe {
        let raw_cx = cx.raw_cx();
        rooted!(&in(cx) let stream_root = stream_obj);
        let c_prop = bun_core::ZBox::from_bytes(prop.as_bytes());
        let mut prop_val = UndefinedValue();
        JS_GetProperty(
            raw_cx,
            stream_root.handle().into(),
            c_prop.as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut prop_val,
            },
        );

        if prop_val.is_object() {
            cache_builtin(cx, module_name, prop_val.to_object());
        } else {
            // Fallback: create an empty object
            rooted!(&in(cx) let alias_obj = w2::JS_NewPlainObject(cx));
            if !alias_obj.get().is_null() {
                cache_builtin(cx, module_name, alias_obj.get());
            }
        }
    }
}

/// `_stream_wrap` — deprecated module (DEP0125) that re-exports the entire
/// `stream` module. Emits a deprecation warning, then caches the stream
/// module object under `_stream_wrap`.
fn install_stream_wrap(cx: &mut mozjs::context::JSContext) {
    let module_name = "_stream_wrap";
    let cache_key = format!("builtin:{}", module_name);
    if let Some(existing) = crate::gc_store::gc_store_get(unsafe { cx.raw_cx() }, &cache_key) {
        if !existing.is_null() {
            return;
        }
    }

    let stream_key = "builtin:stream";
    let stream_obj = crate::gc_store::gc_store_get(unsafe { cx.raw_cx() }, stream_key);
    let Some(stream_obj) = stream_obj else { return };
    if stream_obj.is_null() {
        return;
    }

    // Emit deprecation warning via process.emitWarning if available
    unsafe {
        let raw_cx = cx.raw_cx();
        let global = CurrentGlobalOrNull(raw_cx);
        if !global.is_null() {
            rooted!(&in(cx) let global_root = global);
            let mut process_val = UndefinedValue();
            JS_GetProperty(
                raw_cx,
                global_root.handle().into(),
                c"process".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut process_val,
                },
            );
            if process_val.is_object() {
                rooted!(&in(cx) let process_obj = process_val.to_object());
                let mut emit_warning_val = UndefinedValue();
                JS_GetProperty(
                    raw_cx,
                    process_obj.handle().into(),
                    c"emitWarning".as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut emit_warning_val,
                    },
                );
                if emit_warning_val.is_object() {
                    rooted!(&in(cx) let ew_fn = emit_warning_val.to_object());
                    let msg_str = JS_NewStringCopyZ(
                        raw_cx,
                        c"The _stream_wrap module is deprecated.".as_ptr(),
                    );
                    if !msg_str.is_null() {
                        let msg_val = mozjs::jsval::StringValue(&*msg_str);
                        rooted!(&in(cx) let msg_root = msg_val);
                        let type_str = JS_NewStringCopyZ(raw_cx, c"DeprecationWarning".as_ptr());
                        if !type_str.is_null() {
                            let type_val = mozjs::jsval::StringValue(&*type_str);
                            rooted!(&in(cx) let type_root = type_val);
                            let code_str = JS_NewStringCopyZ(raw_cx, c"DEP0125".as_ptr());
                            if !code_str.is_null() {
                                let code_val = mozjs::jsval::StringValue(&*code_str);
                                rooted!(&in(cx) let code_root = code_val);
                                let elems = [msg_root.get(), type_root.get(), code_root.get()];
                                let call_args = HandleValueArray {
                                    length_: 3,
                                    elements_: elems.as_ptr(),
                                };
                                let mut call_rval = UndefinedValue();
                                let call_rval_h = MutableHandle::<Value> {
                                    _phantom_0: ::std::marker::PhantomData,
                                    ptr: &mut call_rval,
                                };
                                let ew_fn_val = ObjectValue(ew_fn.get());
                                rooted!(&in(cx) let ew_fn_val_root = ew_fn_val);
                                JS_CallFunctionValue(
                                    raw_cx,
                                    process_obj.handle().into(),
                                    ew_fn_val_root.handle().into(),
                                    &call_args,
                                    call_rval_h,
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    // Cache the entire stream module object as _stream_wrap
    cache_builtin(cx, module_name, stream_obj);
}
