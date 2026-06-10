//! Proc-macro crate for `uws_callback` — the C-ABI thunk generator.
//!
//! This crate was extracted from `bun_jsc_macros` (Bun upstream) to decouple
//! `bun_uws` from JSC. Only the `uws_callback` macro is retained; all JSC-
//! specific macros (callback, js_class, etc.) belong in `bao_engine_macros`.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{
    FnArg, Ident, ItemFn, LitStr, Token,
    parse::{Parse, ParseStream},
    parse_macro_input,
    spanned::Spanned,
};

// ──────────────────────────────────────────────────────────────────────────
// #[uws_callback] / #[uws_callback(export = "Name", no_catch, thunk = "name")]
//
// Wraps a `&self` / `&mut self` method in an `extern "C"` thunk suitable for
// registration with uWS / uSockets / any C-ABI callback that round-trips a
// type-erased `*mut c_void` user-data pointer. The thunk:
//
//   - takes `*mut c_void` (or `*const c_void` for `&self`) as the receiver
//     position and casts it back to `Self`;
//   - lowers each `&[T]` / `&mut [T]` parameter to a `(ptr, len)` pair and
//     reconstructs the slice via `slice::from_raw_parts{,_mut}`.
//   - passes every other parameter through verbatim.
//
// Generated thunk name defaults to `__<method>_c`; override with `thunk = "x"`.
// `export = "Sym"` adds `#[unsafe(export_name = "Sym")]` for link-time
// dispatch shims.
// ──────────────────────────────────────────────────────────────────────────

#[derive(Default)]
struct UwsCallbackArgs {
    export: Option<LitStr>,
    thunk: Option<LitStr>,
    no_catch: bool,
}

impl Parse for UwsCallbackArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut out = UwsCallbackArgs::default();
        while !input.is_empty() {
            let ident: Ident = input.parse()?;
            match ident.to_string().as_str() {
                "no_catch" => out.no_catch = true,
                "export" => {
                    input.parse::<Token![=]>()?;
                    out.export = Some(input.parse()?);
                }
                "thunk" => {
                    input.parse::<Token![=]>()?;
                    out.thunk = Some(input.parse()?);
                }
                other => {
                    return Err(syn::Error::new(
                        ident.span(),
                        format!("unknown #[uws_callback] argument `{other}`"),
                    ));
                }
            }
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(out)
    }
}

#[proc_macro_attribute]
pub fn uws_callback(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as UwsCallbackArgs);
    let func = parse_macro_input!(item as ItemFn);
    expand_uws_callback(&args, &func)
        .unwrap_or_else(|e| e.to_compile_error())
        .into()
}

fn expand_uws_callback(args: &UwsCallbackArgs, func: &ItemFn) -> syn::Result<TokenStream2> {
    let fn_name = &func.sig.ident;
    let vis = &func.vis;

    let recv = match func.sig.inputs.first() {
        Some(FnArg::Receiver(r)) => r,
        _ => {
            return Err(syn::Error::new(
                func.sig.ident.span(),
                "#[uws_callback] requires `&self` or `&mut self` as the first parameter",
            ));
        }
    };
    let recv_mut = recv.mutability.is_some();
    let (ctx_ty, recv_expr) = if recv_mut {
        (
            quote! { *mut ::core::ffi::c_void },
            quote! { &mut *__ctx.cast::<Self>() },
        )
    } else {
        (
            quote! { *const ::core::ffi::c_void },
            quote! { &*__ctx.cast::<Self>() },
        )
    };

    let mut thunk_params: Vec<TokenStream2> = vec![quote! { __ctx: #ctx_ty }];
    let mut prelude: Vec<TokenStream2> = Vec::new();
    let mut call_args: Vec<TokenStream2> = Vec::new();

    for (i, arg) in func.sig.inputs.iter().enumerate().skip(1) {
        let FnArg::Typed(pt) = arg else {
            return Err(syn::Error::new(arg.span(), "unexpected receiver"));
        };
        let name = match &*pt.pat {
            syn::Pat::Ident(id) => id.ident.clone(),
            _ => format_ident!("__arg{}", i),
        };
        match classify_uws_arg(&pt.ty) {
            UwsArg::Slice { elem, mutable } => {
                let p = format_ident!("{}_ptr", name);
                let l = format_ident!("{}_len", name);
                let ptr_ty = if mutable {
                    quote! { *mut #elem }
                } else {
                    quote! { *const #elem }
                };
                thunk_params.push(quote! { #p: #ptr_ty });
                thunk_params.push(quote! { #l: usize });
                prelude.push(if mutable {
                    quote! {
                        let #name: &mut [#elem] = unsafe {
                            ::core::slice::from_raw_parts_mut(
                                if #l == 0 {
                                    ::core::ptr::NonNull::<#elem>::dangling().as_ptr()
                                } else {
                                    #p
                                },
                                #l,
                            )
                        };
                    }
                } else {
                    quote! {
                        let #name: &[#elem] = if #l == 0 {
                            &[]
                        } else {
                            unsafe { ::core::slice::from_raw_parts(#p, #l) }
                        };
                    }
                });
                call_args.push(quote! { #name });
            }
            UwsArg::PassThrough(ty) => {
                thunk_params.push(quote! { #name: #ty });
                call_args.push(quote! { #name });
            }
        }
    }

    let ret = match &func.sig.output {
        syn::ReturnType::Default => quote! { () },
        syn::ReturnType::Type(_, t) => quote! { #t },
    };

    let thunk_ident = match &args.thunk {
        Some(l) => format_ident!("{}", l.value()),
        None => format_ident!("__{}_c", fn_name),
    };
    let export_attr = args.export.as_ref().map(|l| {
        quote! { #[unsafe(export_name = #l)] }
    });

    let inner_call = quote! {
        #(#prelude)*
        let __this = unsafe { #recv_expr };
        Self::#fn_name(__this, #(#call_args),*)
    };

    let _ = args.no_catch;
    let body = quote! { #inner_call };

    let thunk = quote! {
        #export_attr
        #[doc(hidden)]
        #[allow(improper_ctypes_definitions, clippy::not_unsafe_ptr_arg_deref)]
        #vis unsafe extern "C" fn #thunk_ident(#(#thunk_params),*) -> #ret {
            #body
        }
    };

    Ok(quote! {
        #func
        #thunk
    })
}

enum UwsArg {
    Slice { elem: syn::Type, mutable: bool },
    PassThrough(syn::Type),
}

fn classify_uws_arg(ty: &syn::Type) -> UwsArg {
    if let syn::Type::Reference(r) = ty {
        if let syn::Type::Slice(s) = &*r.elem {
            return UwsArg::Slice {
                elem: (*s.elem).clone(),
                mutable: r.mutability.is_some(),
            };
        }
    }
    UwsArg::PassThrough(ty.clone())
}