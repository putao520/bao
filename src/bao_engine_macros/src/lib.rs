use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{parse_macro_input, FnArg, ItemFn, LitStr, Receiver};

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
}

mod kw {
    syn::custom_keyword!(export);
    syn::custom_keyword!(method);
    syn::custom_keyword!(getter);
    syn::custom_keyword!(setter);
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
/// ```ignore
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
    let shim_ident = args.export.as_deref().map(|s| format_ident!("{}", s))
        .unwrap_or_else(|| format_ident!("__bao_host_{}", fn_name));

    let has_receiver = func.sig.inputs.first().is_some_and(|a| {
        matches!(a, FnArg::Receiver(_))
    });
    let receiver_is_shared = func.sig.inputs.first().is_some_and(|a| {
        matches!(a, FnArg::Receiver(Receiver { mutability: None, .. }))
    });

    match args.kind {
        HostFnKind::Free if !has_receiver => expand_free_fn(&shim_ident, func),
        HostFnKind::Method | HostFnKind::Free => {
            expand_method_fn(&shim_ident, func, receiver_is_shared)
        }
        HostFnKind::Getter => expand_getter_fn(&shim_ident, func, receiver_is_shared),
        HostFnKind::Setter => expand_setter_fn(&shim_ident, func, receiver_is_shared),
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

fn expand_method_fn(
    shim: &syn::Ident,
    func: &ItemFn,
    shared: bool,
) -> syn::Result<TokenStream2> {
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

fn expand_getter_fn(
    shim: &syn::Ident,
    func: &ItemFn,
    shared: bool,
) -> syn::Result<TokenStream2> {
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

fn expand_setter_fn(
    shim: &syn::Ident,
    func: &ItemFn,
    _shared: bool,
) -> syn::Result<TokenStream2> {
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
