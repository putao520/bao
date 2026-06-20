//! BCE-012 pattern classification helpers.
//!
//! These helpers operate on `syn` AST nodes and classify the payload of a
//! `Handle::<T> { ..., ptr: <expr> }` struct literal against the BCE-012
//! signatures documented in `src/BUG-KNOWLEDGE.md`.
//!
//! All classification is structural / AST-based — format-immune.

use syn::{Expr, ExprReference, Ident};

// (Classification types were initially sketched but the detector now reports
// findings directly; no public classification enum is exported.)

/// Path of a struct-literal type, e.g. `mozjs::jsapi::Handle` → last segment `Handle`.
pub fn last_path_segment(path: &syn::Path) -> Option<&Ident> {
    path.segments.last().map(|s| &s.ident)
}

/// Returns true if the path's last segment is `Handle` (covers `Handle`,
/// `mozjs::jsapi::Handle`, `mozjs::sys::jsapi::Handle`, etc.).
pub fn is_handle_path(path: &syn::Path) -> bool {
    last_path_segment(path).map(|i| i == "Handle").unwrap_or(false)
}

/// Returns true if the path's last segment is `MutableHandle`.
pub fn is_mutable_handle_path(path: &syn::Path) -> bool {
    last_path_segment(path)
        .map(|i| i == "MutableHandle")
        .unwrap_or(false)
}

/// The single type argument of `Handle::<T>` / `MutableHandle::<T>`, or the
/// `T` in `Handle<T>` written without turbofish. Returns its source snippet
/// (e.g. `*mut JSObject`, `Value`, `JSVal`, `*mut JSString`).
pub fn type_argument_snippet<'a>(path: &'a syn::Path) -> Option<&'a syn::PathArguments> {
    path.segments.last().and_then(|s| match &s.arguments {
        syn::PathArguments::AngleBracketed(_) | syn::PathArguments::Parenthesized(_) => {
            Some(&s.arguments)
        }
        syn::PathArguments::None => None,
    })
}

/// Renders a `PathArguments` back to source-like text for classification.
pub fn type_argument_text(args: &syn::PathArguments) -> String {
    match args {
        syn::PathArguments::AngleBracketed(ab) => {
            let inner = ab
                .args
                .iter()
                .map(|a| match a {
                    syn::GenericArgument::Type(ty) => quote_type(ty),
                    other => quote_generic(other),
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("<{}>", inner)
        }
        syn::PathArguments::Parenthesized(pb) => {
            let inputs = pb
                .inputs
                .iter()
                .map(quote_type)
                .collect::<Vec<_>>()
                .join(", ");
            format!("({})", inputs)
        }
        syn::PathArguments::None => String::new(),
    }
}

fn quote_type(ty: &syn::Type) -> String {
    // Lightweight rendering: covers TypePtr, TypePath, and the common cases.
    match ty {
        syn::Type::Ptr(p) => {
            let mutty = match p.const_token {
                Some(_) => "*const ",
                None => "*mut ",
            };
            let elem = match p.elem.as_ref() {
                syn::Type::Path(tp) => tp
                    .path
                    .segments
                    .last()
                    .map(|s| s.ident.to_string())
                    .unwrap_or_default(),
                other => quote_type(other),
            };
            format!("{}{}", mutty, elem)
        }
        syn::Type::Path(tp) => tp
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default(),
        _ => "_".to_string(),
    }
}

fn quote_generic(g: &syn::GenericArgument) -> String {
    match g {
        syn::GenericArgument::Type(ty) => quote_type(ty),
        _ => "_".to_string(),
    }
}

/// Classifies the type-argument text into one of our Handle payload kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandlePayloadKind {
    /// `*mut JSObject` — GC-managed object pointer.
    JsObjectPtr,
    /// `*mut JSString` — GC-managed string pointer.
    JsStringPtr,
    /// `Value` / `JSVal` — tagged JS value (may carry a GC pointer).
    Value,
    /// Anything else (unknown / non-GC). We do not flag these — only the three
    /// BCE-012-documented payload kinds are subject to detection.
    Other,
}

pub fn classify_payload(text: &str) -> HandlePayloadKind {
    let t = text.trim();
    match t {
        "<*mut JSObject>" => HandlePayloadKind::JsObjectPtr,
        "<*mut JSString>" => HandlePayloadKind::JsStringPtr,
        "<Value>" | "<JSVal>" => HandlePayloadKind::Value,
        _ => HandlePayloadKind::Other,
    }
}

/// Extracts the referent of an `&EXPR` form, if the outer expression is a
/// reference. Returns the inner expression.
pub fn as_reference(expr: &Expr) -> Option<&ExprReference> {
    match expr {
        Expr::Reference(r) => Some(r),
        _ => None,
    }
}

/// Identifiers that are the canonical "null" sources — safe because GC does
/// not move null pointers.
const NULL_SOURCES: &[&str] = &["null_mut", "null"];

/// Returns true if the expression is `null_mut()` or `null()` (any path).
pub fn is_null_construction(expr: &Expr) -> bool {
    let path = match expr {
        Expr::Call(c) => &c.func,
        Expr::Path(p) => return NULL_SOURCES
            .iter()
            .any(|n| last_path_segment(&p.path).map(|i| i == n).unwrap_or(false)),
        _ => return false,
    };
    // `null_mut()` — call form
    if let Expr::Path(p) = path.as_ref() {
        return NULL_SOURCES
            .iter()
            .any(|n| last_path_segment(&p.path).map(|i| i == n).unwrap_or(false));
    }
    false
}

/// Constructor names that produce GC-managed `Value` payloads (Object/String
/// tags). Used for backtracking `ptr: &IDENT` → `let IDENT = <CTOR>(...)`.
pub const GC_VALUE_CTORS: &[&str] = &[
    "ObjectValue",
    "StringValue",
    "SymbolValue",
    "MagicValue",
    "to_object",
];

/// Constructor names that produce primitive (non-GC) `Value` payloads.
pub const PRIMITIVE_VALUE_CTORS: &[&str] = &[
    "Int32Value",
    "DoubleValue",
    "Float32Value",
    "Float64Value",
    "BooleanValue",
    "UndefinedValue",
    "NullValue",
    "PrivateValue",
    "Int32OrNullValue",
    "DoubleOrNullValue",
];

/// Returns the constructor name (last path segment) if `expr` is a call like
/// `ObjectValue(...)`, `mozjs::jsval::ObjectValue(...)`, etc.
pub fn call_name(expr: &Expr) -> Option<&Ident> {
    let func = match expr {
        Expr::Call(c) => &c.func,
        Expr::MethodCall(m) => &m.receiver,
        _ => return None,
    };
    if let Expr::Path(p) = func.as_ref() {
        return last_path_segment(&p.path);
    }
    // method call: receiver.<method>(...) — return the method name
    if let Expr::MethodCall(m) = expr {
        return Some(&m.method);
    }
    None
}
