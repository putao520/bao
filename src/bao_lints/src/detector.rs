//! AST-based BCE-012 detector.
//!
//! Walks a parsed `syn::File`, finds every `Handle::<T> { ..., ptr: <expr> }`
//! struct-literal, classifies the payload, and backtracks `ptr: &IDENT` to the
//! nearest preceding `let IDENT = <expr>;` in the same function body when the
//! payload is a GC-managed `Value`/`JSVal`.
//!
//! The detector uses a single recursive walk over each function body, with a
//! locals table that is extended for nested blocks (`{}`, `if`, `match`, …)
//! so `ptr: &IDENT` can be backtracked to its initializer.

use std::path::Path;

use syn::{Expr, ExprStruct, Ident, Item, Stmt};

use crate::pattern::{
    as_reference, call_name, classify_payload, is_handle_path, is_mutable_handle_path,
    is_null_construction, type_argument_snippet, type_argument_text,
};

/// One finding emitted by the detector.
#[derive(Debug, Clone)]
pub struct Finding {
    pub file: String,
    pub line: usize,
    pub col: usize,
    pub message: String,
}

impl Finding {
    pub fn render(&self) -> String {
        format!("{}:{}:{}: {}", self.file, self.line, self.col, self.message)
    }
}

/// Scans a single file source for BCE-012 violations.
///
/// `file_path` is used only for reporting; `src` is the UTF-8 source text.
pub fn scan_source(file_path: &Path, src: &str) -> Vec<Finding> {
    let file = match syn::parse_file(src) {
        Ok(f) => f,
        Err(e) => {
            // Unparseable file: report as a non-fatal warning, do not crash.
            return vec![Finding {
                file: file_path.display().to_string(),
                line: 0,
                col: 0,
                message: format!("bao_lints: parse error, skipped: {}", e),
            }];
        }
    };

    let mut findings = Vec::new();
    let file_str = file_path.display().to_string();
    for item in &file.items {
        walk_item(item, &file_str, &mut findings);
    }
    findings
}

fn walk_item(item: &Item, file: &str, out: &mut Vec<Finding>) {
    // Only free-standing functions (and their nested closures/fns) carry the
    // BCE-012 pattern. Other items (consts, types, impls) wrap functions, so
    // we recurse into impl blocks too.
    match item {
        Item::Fn(f) => {
            let empty: Locals<'_> = Vec::new();
            walk_block(&f.block.stmts, &empty, file, out);
        }
        Item::Impl(i) => {
            let empty: Locals<'_> = Vec::new();
            for inner in &i.items {
                if let syn::ImplItem::Fn(m) = inner {
                    walk_block(&m.block.stmts, &empty, file, out);
                }
            }
        }
        Item::Mod(m) => {
            if let Some((_, items)) = &m.content {
                for inner in items {
                    walk_item(inner, file, out);
                }
            }
        }
        _ => {}
    }
}

/// Local-init binding: `(name, initializer)` — last definition wins in scope.
type Locals<'a> = Vec<(&'a Ident, &'a Expr)>;

fn walk_block(stmts: &[Stmt], parent_locals: &Locals<'_>, file: &str, out: &mut Vec<Finding>) {
    // Snapshot the parent locals then extend with this block's let-bindings.
    let mut locals: Locals<'_> = parent_locals.iter().cloned().collect();
    for stmt in stmts {
        collect_locals_in_stmt(stmt, &mut locals);
    }
    for stmt in stmts {
        walk_stmt(stmt, &locals, file, out);
    }
}

fn collect_locals_in_stmt<'a>(stmt: &'a Stmt, out: &mut Locals<'a>) {
    if let Stmt::Local(local) = stmt {
        if let Some(init) = &local.init {
            collect_idents_from_pat(&local.pat, &init.expr, out);
        }
    }
}

fn collect_idents_from_pat<'a>(pat: &'a syn::Pat, init: &'a Expr, out: &mut Locals<'a>) {
    match pat {
        syn::Pat::Ident(pi) => out.push((&pi.ident, init)),
        syn::Pat::Tuple(t) => {
            // Tuple-destructuring cannot map a single init to each slot; skip.
            for elem in &t.elems {
                collect_idents_from_pat(elem, init, out);
            }
        }
        _ => {}
    }
}

fn walk_stmt(stmt: &Stmt, locals: &Locals<'_>, file: &str, out: &mut Vec<Finding>) {
    match stmt {
        Stmt::Local(local) => {
            // The let-binding's initializer may itself contain a
            // `Handle { ptr: &... }` literal (the canonical BCE-012 case).
            if let Some(init) = &local.init {
                walk_expr(&init.expr, locals, file, out);
            }
        }
        Stmt::Item(item) => walk_item(item, file, out),
        Stmt::Expr(e, _) => walk_expr(e, locals, file, out),
        Stmt::Macro(_) => {} // macro invocation — out of BCE-012 scope
    }
}

fn walk_expr(expr: &Expr, locals: &Locals<'_>, file: &str, out: &mut Vec<Finding>) {
    if let Expr::Struct(s) = expr {
        check_struct(s, locals, file, out);
    }
    match expr {
        Expr::Block(b) => walk_block(&b.block.stmts, locals, file, out),
        Expr::If(i) => {
            walk_expr(&i.cond, locals, file, out);
            walk_block(&i.then_branch.stmts, locals, file, out);
            if let Some((_, eb)) = &i.else_branch {
                walk_expr(eb, locals, file, out);
            }
        }
        Expr::Match(m) => {
            walk_expr(&m.expr, locals, file, out);
            for arm in &m.arms {
                walk_expr(&arm.body, locals, file, out);
            }
        }
        Expr::Loop(l) => walk_block(&l.body.stmts, locals, file, out),
        Expr::While(w) => {
            walk_expr(&w.cond, locals, file, out);
            walk_block(&w.body.stmts, locals, file, out);
        }
        Expr::ForLoop(f) => {
            walk_expr(&f.expr, locals, file, out);
            walk_block(&f.body.stmts, locals, file, out);
        }
        Expr::Unsafe(u) => walk_block(&u.block.stmts, locals, file, out),
        Expr::Closure(_) | Expr::Async(_) => {
            // Closures have their own scope; their bodies are scanned
            // separately when they appear as function bodies elsewhere.
        }
        _ => {}
    }
}

fn report(file: &str, span: proc_macro2::Span, message: String, out: &mut Vec<Finding>) {
    out.push(Finding {
        file: file.to_string(),
        line: span.start().line.max(1),
        col: span.start().column.saturating_add(1),
        message,
    });
}

fn check_struct(s: &ExprStruct, locals: &Locals<'_>, file: &str, out: &mut Vec<Finding>) {
    if is_mutable_handle_path(&s.path) {
        return;
    }
    if !is_handle_path(&s.path) {
        return;
    }

    let payload_text = type_argument_snippet(&s.path)
        .map(type_argument_text)
        .unwrap_or_default();
    let kind = classify_payload(&payload_text);

    let ptr_field = s.fields.iter().find(|f| match &f.member {
        syn::Member::Named(ident) => *ident == "ptr",
        syn::Member::Unnamed(_) => false,
    });
    let ptr_value = match ptr_field.map(|f| &f.expr) {
        Some(e) => e,
        None => return,
    };

    match kind {
        crate::pattern::HandlePayloadKind::JsObjectPtr
        | crate::pattern::HandlePayloadKind::JsStringPtr => {
            check_raw_pointer_payload(s, ptr_value, &payload_text, file, out);
        }
        crate::pattern::HandlePayloadKind::Value => {
            check_value_payload(s, ptr_value, locals, file, out);
        }
        crate::pattern::HandlePayloadKind::Other => {}
    }
}

fn check_raw_pointer_payload(
    s: &ExprStruct,
    ptr_value: &Expr,
    payload_text: &str,
    file: &str,
    out: &mut Vec<Finding>,
) {
    let inner = match as_reference(ptr_value) {
        Some(r) => &r.expr,
        None => {
            if is_null_construction(ptr_value) {
                return;
            }
            report(
                file,
                s.brace_token.span.join(),
                format!(
                    "BCE-012: Handle{} {{ ptr: <non-& expr> }} — raw GC pointer payload requires rooted! + .handle().into()",
                    payload_text
                ),
                out,
            );
            return;
        }
    };
    if is_null_construction(inner) {
        return;
    }
    let referent = if let Expr::Path(p) = inner.as_ref() {
        p.path.segments.last().map(|seg| seg.ident.clone())
    } else {
        None
    };
    let referent_note = referent
        .as_ref()
        .map(|i| format!(" (referent `{}`)", i))
        .unwrap_or_default();
    report(
        file,
        s.brace_token.span.join(),
        format!(
            "BCE-012: Handle{} {{ ptr: &... }} — GC-managed raw pointer must be obtained via rooted! + .handle().into(){}",
            payload_text, referent_note
        ),
        out,
    );
}

fn check_value_payload(
    s: &ExprStruct,
    ptr_value: &Expr,
    locals: &Locals<'_>,
    file: &str,
    out: &mut Vec<Finding>,
) {
    let inner = match as_reference(ptr_value) {
        Some(r) => &r.expr,
        None => return, // BCE-012 signature is `ptr: &...`
    };

    // Inline `&ObjectValue(...)` / `&StringValue(...)` form.
    if let Some(name) = call_name(inner) {
        if is_gc_value_ctor(name) {
            report(
                file,
                s.brace_token.span.join(),
                format!(
                    "BCE-012: Handle<Value> {{ ptr: &{}(...) }} — GC-managed value requires rooted! + .handle().into()",
                    name
                ),
                out,
            );
            return;
        }
        if is_primitive_value_ctor(name) {
            return;
        }
    }

    // `&IDENT` form — backtrack via the locals table.
    let ident = match inner.as_ref() {
        Expr::Path(p) => p.path.get_ident(),
        _ => None,
    };
    let ident = match ident {
        Some(i) => i,
        None => return,
    };

    let init = match lookup_local(locals, ident) {
        Some(e) => e,
        None => return, // Cannot backtrack — do not flag (zero-FP policy).
    };

    if is_null_construction(init) {
        return;
    }
    if let Some(name) = call_name(init) {
        if is_gc_value_ctor(name) {
            report(
                file,
                s.brace_token.span.join(),
                format!(
                    "BCE-012: Handle<Value> {{ ptr: &{} }} where `{} = {}(...)` — GC-managed value requires rooted! + .handle().into()",
                    ident, ident, name
                ),
                out,
            );
            return;
        }
        if is_primitive_value_ctor(name) {
            return;
        }
    }
    // Unknown initializer — do not flag.
}

fn lookup_local<'a>(locals: &'a Locals<'a>, ident: &Ident) -> Option<&'a Expr> {
    // Last-write-wins scoping (later bindings shadow earlier ones).
    locals
        .iter()
        .rev()
        .find(|(name, _)| *name == ident)
        .map(|(_, init)| *init)
}

fn is_gc_value_ctor(name: &Ident) -> bool {
    let s = name.to_string();
    crate::pattern::GC_VALUE_CTORS.iter().any(|c| *c == s)
}

fn is_primitive_value_ctor(name: &Ident) -> bool {
    let s = name.to_string();
    crate::pattern::PRIMITIVE_VALUE_CTORS.iter().any(|c| *c == s)
}
