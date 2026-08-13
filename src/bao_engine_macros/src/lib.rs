use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{FnArg, Ident, ItemFn, ItemStruct, LitStr, Receiver, parse_macro_input};

#[derive(Default)]
struct HostFnArgs {
    export: Option<String>,
    kind: HostFnKind,
}

#[derive(Default, PartialEq)]
enum HostFnKind {
    #[default]
    Free,
    Getter,
    Setter,
    Method,
    Constructor,
}

mod kw {
    syn::custom_keyword!(export);
    syn::custom_keyword!(method);
    syn::custom_keyword!(getter);
    syn::custom_keyword!(setter);
    syn::custom_keyword!(constructor);
}

impl syn::parse::Parse for HostFnArgs {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut args = HostFnArgs::default();
        while !input.is_empty() {
            let lookahead = input.lookahead1();
            if lookahead.peek(kw::export) {
                input.parse::<kw::export>()?;
                input.parse::<syn::Token![=]>()?;
                let lit: LitStr = input.parse()?;
                args.export = Some(lit.value());
            } else if lookahead.peek(kw::method) {
                input.parse::<kw::method>()?;
                args.kind = HostFnKind::Method;
            } else if lookahead.peek(kw::getter) {
                input.parse::<kw::getter>()?;
                args.kind = HostFnKind::Getter;
            } else if lookahead.peek(kw::setter) {
                input.parse::<kw::setter>()?;
                args.kind = HostFnKind::Setter;
            } else if lookahead.peek(kw::constructor) {
                input.parse::<kw::constructor>()?;
                args.kind = HostFnKind::Constructor;
            } else {
                return Err(lookahead.error());
            }
            if !input.is_empty() {
                input.parse::<syn::Token![,]>()?;
            }
        }
        Ok(args)
    }
}

/// `#[host_fn]` proc-macro for SpiderMonkey host function shims.
///
/// Generates a `unsafe extern "C" fn(cx, argc, vp) -> bool` that extracts
/// arguments from SM CallArgs, invokes the wrapped Rust function, and handles
/// exceptions.
///
/// Usage:
/// ```text
/// #[host_fn]
/// fn my_function(global: &JsGlobal, argc: u32, args: &[JsValue]) -> Result<JsValue, JsError> { ... }
///
/// #[host_fn(method)]
/// fn my_method(this: &MyType, global: &JsGlobal, argc: u32, args: &[JsValue]) -> Result<JsValue, JsError> { ... }
///
/// #[host_fn(getter)]
/// fn my_getter(this: &MyType) -> Result<JsValue, JsError> { ... }
///
/// #[host_fn(setter)]
/// fn my_setter(this: &mut MyType, value: JsValue) -> Result<(), JsError> { ... }
///
/// #[host_fn(export = "customName")]
/// fn some_fn(global: &JsGlobal, argc: u32, args: &[JsValue]) -> Result<JsValue, JsError> { ... }
/// ```
#[proc_macro_attribute]
pub fn host_fn(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as HostFnArgs);
    let func = parse_macro_input!(item as ItemFn);
    expand_host_fn(&args, &func)
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

fn expand_host_fn(args: &HostFnArgs, func: &ItemFn) -> syn::Result<TokenStream2> {
    let fn_name = &func.sig.ident;
    let shim_ident = args
        .export
        .as_deref()
        .map(|s| format_ident!("{}", s))
        .unwrap_or_else(|| format_ident!("__bao_host_{}", fn_name));

    let has_receiver = func
        .sig
        .inputs
        .first()
        .is_some_and(|a| matches!(a, FnArg::Receiver(_)));
    let receiver_is_shared = func.sig.inputs.first().is_some_and(|a| {
        matches!(
            a,
            FnArg::Receiver(Receiver {
                mutability: None,
                ..
            })
        )
    });

    match args.kind {
        HostFnKind::Free if !has_receiver => expand_free_fn(&shim_ident, func),
        HostFnKind::Method | HostFnKind::Free => {
            expand_method_fn(&shim_ident, func, receiver_is_shared)
        }
        HostFnKind::Getter => expand_getter_fn(&shim_ident, func, receiver_is_shared),
        HostFnKind::Setter => expand_setter_fn(&shim_ident, func, receiver_is_shared),
        HostFnKind::Constructor => expand_constructor_fn(&shim_ident, func),
    }
}

fn expand_free_fn(shim: &syn::Ident, func: &ItemFn) -> syn::Result<TokenStream2> {
    let fn_name = &func.sig.ident;
    let body = &func.block;

    Ok(quote! {
        #[allow(unsafe_op_in_unsafe_fn)]
        pub unsafe extern "C" fn #shim(
            cx: *mut ::mozjs::jsapi::JSContext,
            argc: u32,
            vp: *mut ::mozjs::jsval::JSVal,
        ) -> bool {
            let __args = ::mozjs::jsapi::CallArgs::from_vp(vp, argc);
            let __cx = unsafe {
                ::mozjs::context::JSContext::from_ptr(
                    ::std::ptr::NonNull::new_unchecked(cx)
                )
            };
            match #fn_name(&__cx, argc, __args) {
                ::std::result::Result::Ok(val) => {
                    val.set_as_rval(&mut __args);
                    true
                }
                ::std::result::Result::Err(err) => {
                    err.throw_on(cx);
                    false
                }
            }
        }

        #[allow(dead_code)]
        fn #fn_name #body
    })
}

fn expand_method_fn(shim: &syn::Ident, func: &ItemFn, shared: bool) -> syn::Result<TokenStream2> {
    let fn_name = &func.sig.ident;
    let body = &func.block;
    let this_reborrow = if shared {
        quote! { let __this: &Self = unsafe { &*__this_ptr }; }
    } else {
        quote! { let __this: &mut Self = unsafe { &mut *__this_ptr }; }
    };

    Ok(quote! {
        #[allow(unsafe_op_in_unsafe_fn)]
        pub unsafe extern "C" fn #shim(
            cx: *mut ::mozjs::jsapi::JSContext,
            argc: u32,
            vp: *mut ::mozjs::jsval::JSVal,
        ) -> bool {
            let __args = ::mozjs::jsapi::CallArgs::from_vp(vp, argc);
            let __this_ptr: *mut Self = unsafe {
                use ::bao_engine::host_fn::HostObject;
                HostObject::from_private(cx, __args.thisv())
            };
            #this_reborrow
            let __cx = unsafe {
                ::mozjs::context::JSContext::from_ptr(
                    ::std::ptr::NonNull::new_unchecked(cx)
                )
            };
            match #fn_name(__this, &__cx, argc, __args) {
                ::std::result::Result::Ok(val) => {
                    val.set_as_rval(&mut __args);
                    true
                }
                ::std::result::Result::Err(err) => {
                    err.throw_on(cx);
                    false
                }
            }
        }

        #[allow(dead_code)]
        fn #fn_name #body
    })
}

fn expand_getter_fn(shim: &syn::Ident, func: &ItemFn, shared: bool) -> syn::Result<TokenStream2> {
    let fn_name = &func.sig.ident;
    let body = &func.block;
    let this_reborrow = if shared {
        quote! { let __this: &Self = unsafe { &*__this_ptr }; }
    } else {
        quote! { let __this: &mut Self = unsafe { &mut *__this_ptr }; }
    };

    Ok(quote! {
        #[allow(unsafe_op_in_unsafe_fn)]
        pub unsafe extern "C" fn #shim(
            cx: *mut ::mozjs::jsapi::JSContext,
            argc: u32,
            vp: *mut ::mozjs::jsval::JSVal,
        ) -> bool {
            let __args = ::mozjs::jsapi::CallArgs::from_vp(vp, argc);
            let __this_ptr: *mut Self = unsafe {
                use ::bao_engine::host_fn::HostObject;
                HostObject::from_private(cx, __args.thisv())
            };
            #this_reborrow
            match #fn_name(__this) {
                ::std::result::Result::Ok(val) => {
                    val.set_as_rval(&mut __args);
                    true
                }
                ::std::result::Result::Err(err) => {
                    err.throw_on(cx);
                    false
                }
            }
        }

        #[allow(dead_code)]
        fn #fn_name #body
    })
}

fn expand_constructor_fn(shim: &syn::Ident, func: &ItemFn) -> syn::Result<TokenStream2> {
    let fn_name = &func.sig.ident;
    let body = &func.block;

    Ok(quote! {
        #[allow(unsafe_op_in_unsafe_fn)]
        pub unsafe extern "C" fn #shim(
            cx: *mut ::mozjs::jsapi::JSContext,
            argc: u32,
            vp: *mut ::mozjs::jsval::JSVal,
        ) -> bool {
            let __args = ::mozjs::jsapi::CallArgs::from_vp(vp, argc);
            let __cx = unsafe {
                ::mozjs::context::JSContext::from_ptr(
                    ::std::ptr::NonNull::new_unchecked(cx)
                )
            };
            match #fn_name(&__cx, argc, __args) {
                ::std::result::Result::Ok(val) => {
                    val.set_as_rval(&mut __args);
                    true
                }
                ::std::result::Result::Err(err) => {
                    err.throw_on(cx);
                    false
                }
            }
        }

        #[allow(dead_code)]
        fn #fn_name #body
    })
}

fn expand_setter_fn(shim: &syn::Ident, func: &ItemFn, _shared: bool) -> syn::Result<TokenStream2> {
    let fn_name = &func.sig.ident;
    let body = &func.block;

    Ok(quote! {
        #[allow(unsafe_op_in_unsafe_fn)]
        pub unsafe extern "C" fn #shim(
            cx: *mut ::mozjs::jsapi::JSContext,
            argc: u32,
            vp: *mut ::mozjs::jsval::JSVal,
        ) -> bool {
            let __args = ::mozjs::jsapi::CallArgs::from_vp(vp, argc);
            let __this_ptr: *mut Self = unsafe {
                use ::bao_engine::host_fn::HostObject;
                HostObject::from_private(cx, __args.thisv())
            };
            let __this: &mut Self = unsafe { &mut *__this_ptr };
            let __value = unsafe {
                ::bao_engine::host_fn::extract_setter_value(cx, &__args)
            };
            match #fn_name(__this, __value) {
                ::std::result::Result::Ok(()) => {
                    true
                }
                ::std::result::Result::Err(err) => {
                    err.throw_on(cx);
                    false
                }
            }
        }

        #[allow(dead_code)]
        fn #fn_name #body
    })
}

// ──────────────────────────────────────────────────────────────────────────
// bao_engine::codegen_cached_accessors!("TypeName"; prop_a, prop_b, ...)
//
// Emits one `${snake}_get_cached` / `${snake}_set_cached` / `${snake}_take_cached`
// triple per listed property, using SpiderMonkey ReservedSlot read/write instead
// of JSC WriteBarrier. Each property maps to a consecutive slot index starting
// from `CACHED_SLOT_OFFSET` (default 1; slot 0 is reserved for native private data).
//
// Also emits a `Gc` enum mirroring Bun's codegen pattern so that
// `js_class_module!` callers get `Gc::$prop.get()/.set()/.clear()`.
// ──────────────────────────────────────────────────────────────────────────

struct CachedAccessorsInput {
    type_name: LitStr,
    props: Vec<Ident>,
}

impl syn::parse::Parse for CachedAccessorsInput {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let type_name: LitStr = input.parse()?;
        if input.peek(syn::Token![;]) {
            input.parse::<syn::Token![;]>()?;
        } else if input.peek(syn::Token![,]) {
            input.parse::<syn::Token![,]>()?;
        }
        let mut props = Vec::new();
        while !input.is_empty() {
            props.push(input.parse()?);
            if input.peek(syn::Token![,]) {
                input.parse::<syn::Token![,]>()?;
            }
        }
        Ok(Self { type_name, props })
    }
}

#[proc_macro]
pub fn codegen_cached_accessors(input: TokenStream) -> TokenStream {
    let CachedAccessorsInput { type_name, props } =
        parse_macro_input!(input as CachedAccessorsInput);

    let mut out = TokenStream2::new();

    // Slot offset constant — slot 0 is private data, cached props start at 1
    let ty_ident = format_ident!("{}", type_name.value().replace('-', "_"));
    let slot_const = format_ident!("{}_CACHED_SLOT_OFFSET", ty_ident);
    let prop_count = props.len() as u32;

    out.extend(quote! {
        /// First ReservedSlot index used for cached properties (slot 0 = private data).
        pub const #slot_const: u32 = 1;
    });

    for (i, prop) in props.iter().enumerate() {
        let prop_str = prop.to_string();
        let snake = camel_to_snake(&prop_str);
        let get_fn = format_ident!("{snake}_get_cached");
        let set_fn = format_ident!("{snake}_set_cached");
        let take_fn = format_ident!("{snake}_take_cached");
        let slot_idx = 1u32 + i as u32;

        out.extend(quote! {
            /// Read a cached value from the object's ReservedSlot.
            /// Returns `None` if the slot contains undefined (never assigned).
            #[inline]
            pub fn #get_fn(obj: *mut ::mozjs::jsapi::JSObject) -> ::core::option::Option<::mozjs::jsval::JSVal> {
                let mut val = ::mozjs::jsval::UndefinedValue();
                unsafe { ::mozjs::jsapi::JS_GetReservedSlot(obj, #slot_idx, &mut val); }
                if val.is_undefined() || val.is_null() {
                    ::core::option::Option::None
                } else {
                    ::core::option::Option::Some(val)
                }
            }

            /// Write a value to the object's ReservedSlot (cache it).
            #[inline]
            pub fn #set_fn(
                obj: *mut ::mozjs::jsapi::JSObject,
                value: ::mozjs::jsval::JSVal,
            ) {
                unsafe { ::mozjs::jsapi::JS_SetReservedSlot(obj, #slot_idx, &value); }
            }

            /// Read-and-clear the ReservedSlot in one step.
            /// Returns `Some(value)` and resets the slot to undefined iff a
            /// value was cached; `None` if the slot was already empty.
            #[inline]
            pub fn #take_fn(
                obj: *mut ::mozjs::jsapi::JSObject,
            ) -> ::core::option::Option<::mozjs::jsval::JSVal> {
                let v = #get_fn(obj)?;
                #set_fn(obj, ::mozjs::jsval::UndefinedValue());
                ::core::option::Option::Some(v)
            }
        });
    }

    // Emit the `Gc` enum mirroring Bun's codegen pattern
    if !props.is_empty() {
        let variants = props.iter();
        let get_arms = props.iter().map(|p| {
            let f = format_ident!("{}_get_cached", camel_to_snake(&p.to_string()));
            quote! { Gc::#p => #f(obj), }
        });
        let set_arms = props.iter().map(|p| {
            let f = format_ident!("{}_set_cached", camel_to_snake(&p.to_string()));
            quote! { Gc::#p => #f(obj, value), }
        });
        let clear_arms = props.iter().map(|p| {
            let f = format_ident!("{}_set_cached", camel_to_snake(&p.to_string()));
            quote! { Gc::#p => #f(obj, ::mozjs::jsval::UndefinedValue()), }
        });
        out.extend(quote! {
            /// GC-cached value slots on the JS wrapper (mirrors Bun's `js.gc.<field>.get/set/clear`).
            #[allow(non_camel_case_types, dead_code)]
            #[derive(Clone, Copy)]
            #[repr(u8)]
            pub(crate) enum Gc { #( #variants, )* }
            #[allow(dead_code)]
            impl Gc {
                #[inline] pub fn get(self, obj: *mut ::mozjs::jsapi::JSObject) -> ::core::option::Option<::mozjs::jsval::JSVal> {
                    match self { #( #get_arms )* }
                }
                #[inline] pub fn set(self, obj: *mut ::mozjs::jsapi::JSObject, value: ::mozjs::jsval::JSVal) {
                    match self { #( #set_arms )* }
                }
                #[inline] pub fn clear(self, obj: *mut ::mozjs::jsapi::JSObject) {
                    match self { #( #clear_arms )* }
                }
            }
        });
    }

    let _ = type_name; // Used for slot offset constant naming
    let _ = prop_count;
    out.into()
}

fn camel_to_snake(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for (i, ch) in s.char_indices() {
        if ch.is_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// #[JsClass] — proc-macro for JS class boilerplate generation
// ---------------------------------------------------------------------------

/// `#[JsClass]` proc-macro for SpiderMonkey JS class registration.
///
/// Generates:
/// - `{Struct}__create` constructor trampoline (JSNative)
/// - `{Struct}__proto` prototype setup function
/// - `HostObject` impl for reserved slot pointer storage
///
/// Usage:
/// ```text
/// #[bao_engine_macros::JsClass]
/// pub struct MyClass { field1: String }
///
/// impl MyClass {
///     #[host_fn(method)]
///     fn my_method(&self, ...) -> JsResult<JsValue> { ... }
/// }
/// ```
#[proc_macro_attribute]
pub fn JsClass(attr: TokenStream, item: TokenStream) -> TokenStream {
    let struct_item = parse_macro_input!(item as ItemStruct);
    let struct_name = &struct_item.ident;

    let js_name = if attr.is_empty() {
        struct_name.to_string()
    } else {
        let name_arg = parse_macro_input!(attr as LitStr);
        name_arg.value()
    };

    let create_ident = format_ident!("{}__create", struct_name);
    let proto_ident = format_ident!("{}__proto", struct_name);

    let expanded = quote! {
        #struct_item

        impl ::bao_engine::host_fn::HostObject for #struct_name {}

        impl #struct_name {
            #[allow(unsafe_op_in_unsafe_fn)]
            pub unsafe extern "C" fn #create_ident(
                cx: *mut ::mozjs::jsapi::JSContext,
                argc: u32,
                vp: *mut ::mozjs::jsval::JSVal,
            ) -> bool {
                let __args = ::mozjs::jsapi::CallArgs::from_vp(vp, argc);
                match <Self as ::bao_engine::host_fn::JsClassOps>::construct(cx, &__args) {
                    ::std::result::Result::Ok(val) => {
                        val.set_as_rval(&mut __args);
                        true
                    }
                    ::std::result::Result::Err(err) => {
                        err.throw_on(cx);
                        false
                    }
                }
            }

            #[allow(unsafe_op_in_unsafe_fn)]
            pub unsafe fn #proto_ident(
                cx: &mut ::mozjs::context::JSContext,
                global: ::mozjs::rust::Handle<*mut ::mozjs::jsapi::JSObject>,
            ) {
                let c_name = ::std::ffi::CString::new(#js_name).unwrap_or_default();
                ::mozjs::rust::wrappers2::JS_DefineFunction(
                    cx,
                    global,
                    c_name.as_ptr(),
                    Some(Self::#create_ident),
                    0,
                    ::mozjs::jsapi::JSPROP_ENUMERATE as u32,
                );
            }
        }
    };

    expanded.into()
}
