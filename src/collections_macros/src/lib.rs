//! `derive(SoaRow)` — compile-time field-table registration for
//! `bun_collections::MultiArrayList`'s SoA column layout.
//!
//! The pre-derive layout walked SpiderMonkey-… er, rustc's experimental
//! `core::mem::type_info` reflection (nightly-only, and its `TypeId` size
//! lookup had known holes that forced offset-delta fallbacks). The derive
//! replaces all of that with a declaration table stamped straight from the
//! struct definition: per field, its name, `size_of::<FieldType>()`, and
//! `offset_of!(Struct, field)` — every value a stable-compiler const, every
//! value exact (no padding-span approximations). One code path on both
//! channels; the reflection path is retired wholesale.

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Fields};

#[proc_macro_derive(SoaRow)]
pub fn derive_soa_row(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let fields = match &input.data {
        Data::Struct(data) => &data.fields,
        _ => {
            return syn::Error::new_spanned(
                &input,
                "SoaRow: only structs are supported (MultiArrayList rows are named-field structs)",
            )
            .to_compile_error()
            .into();
        }
    };
    let named = match fields {
        Fields::Named(named) => named,
        Fields::Unit | Fields::Unnamed(_) => {
            return syn::Error::new_spanned(
                name,
                "SoaRow: MultiArrayList rows need named fields (column names come from them)",
            )
            .to_compile_error()
            .into();
        }
    };

    let entries = named.named.iter().map(|f| {
        let fname = f.ident.as_ref().expect("named field");
        let fname_str = fname.to_string();
        let fty = &f.ty;
        quote! {
            ::bun_collections::SoaFieldInfo {
                name: #fname_str,
                size: ::core::mem::size_of::<#fty>(),
                offset: ::core::mem::offset_of!(#name, #fname),
            }
        }
    });

    let expanded = quote! {
        #[automatically_derived]
        impl #impl_generics ::bun_collections::SoaRow for #name #ty_generics #where_clause {
            const SOA_FIELDS: &'static [::bun_collections::SoaFieldInfo] = &[#( #entries ),*];
        }
    };
    expanded.into()
}
