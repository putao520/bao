// @trace REQ-ENG-005 [bug:BUG-ENG-365]
//! Static named-export extraction from CJS module sources via OXC AST.
//!
//! SM's ESM linker requires static `export` declarations at ModuleLink time;
//! runtime `Object.defineProperty(exports, ...)` is invisible to it. This
//! module parses a CJS source with `oxc_parser` and walks the top-level
//! statement list to find the named keys that `module.exports = {...}` (or
//! `exports.foo = ...`, or `Object.defineProperty(exports, 'foo', ...)`)
//! would produce at runtime. The caller then emits static `export let <key>;`
//! declarations binding each key to `__cjs_default[<lit>]`.

use oxc_allocator::Allocator;
use oxc_ast::ast::{
    Argument, AssignmentTarget, Expression, ObjectPropertyKind, PropertyKey, Statement,
};
use oxc_parser::Parser;
use oxc_span::SourceType;

/// Extract the named-export key set from a CJS module source.
///
/// Returns keys in first-seen order, de-duplicated, with `default` and
/// `__esModule` filtered out (they are either the default export itself or a
/// transpiler artefact that must not shadow real exports).
///
/// Only **top-level** direct assignments are collected — the same shapes that
/// Node's own cjs-module-lexer recognises. Nested IIFE / conditional bodies
/// are intentionally not followed (they are dynamic in nature and cannot be
/// statically known without executing the module).
///
/// # Coverage shapes
///
/// | Shape                              | Example                        |
/// |------------------------------------|--------------------------------|
/// | `module.exports = { a, b: 1 }`     | shorthand + key-value pairs    |
/// | `module.exports = { "str": fn }`   | string-literal keys            |
/// | `exports.foo = ...`                | direct property assignment     |
/// | `Object.defineProperty(exports, 'foo', ...)` | explicit defineProperty |
///
/// # Intentionally NOT covered (registered as known gaps)
///
/// - Computed keys (`{ [expr]: v }`) — value not statically known
/// - Spread elements (`{ ...obj }`) — source object unknown
/// - Nested IIFE / conditional exports
/// - `module.exports = factory()` — call return value unknown
pub fn extract_cjs_named_exports(source: &str) -> Vec<String> {
    let allocator = Allocator::default();
    // CJS is a plain Script (`SourceType::cjs()` = Script kind), NOT a Module:
    // SourceType::default() is mjs/Module which enables module-grammar checks
    // and top-level await context that CJS sources must not see.
    let source_type = SourceType::cjs();
    let ret = Parser::new(&allocator, source, source_type).parse();
    if ret.panicked {
        // Unparseable source: fall back to default-only exports (honest
        // degradation — no named exports can be proven statically).
        return Vec::new();
    }

    let mut keys: Vec<String> = Vec::new();
    let mut seen: ::std::collections::HashSet<String> = ::std::collections::HashSet::new();

    for stmt in &ret.program.body {
        if let Statement::ExpressionStatement(expr_stmt) = stmt {
            collect_from_expression(&expr_stmt.expression, &mut keys, &mut seen);
        }
    }

    keys
}

/// Inspect a single top-level expression statement for export-producing
/// assignment or defineProperty calls.
fn collect_from_expression(
    expr: &Expression,
    keys: &mut Vec<String>,
    seen: &mut ::std::collections::HashSet<String>,
) {
    // We look for:
    //   1. Simple assignment `A = B` where A is `module.exports` or
    //      `exports.<prop>` and B is an object literal (for module.exports)
    //      or any expression (for exports.<prop>).
    //   2. Call expression `Object.defineProperty(exports, 'key', ...)`.
    let Expression::AssignmentExpression(assign) = expr else {
        // Check for Object.defineProperty(exports, 'key', ...) form.
        if let Expression::CallExpression(call) = expr {
            collect_from_define_property_call(call, keys, seen);
        }
        return;
    };

    // Only handle plain `=` (Assign operator), not compound assignments.
    if assign.operator != oxc_syntax::operator::AssignmentOperator::Assign {
        return;
    }

    // -- Case 1: module.exports = { ... } -------------------------------
    // The left side must be the expression `module.exports`.
    if is_module_exports(&assign.left) {
        if let Expression::ObjectExpression(obj) = &assign.right {
            for prop in &obj.properties {
                if let ObjectPropertyKind::ObjectProperty(op) = prop {
                    // Skip computed keys and spread (already filtered by
                    // ObjectPropertyKind, but double-check computed).
                    if op.computed {
                        continue;
                    }
                    match &op.key {
                        PropertyKey::StaticIdentifier(ident) => {
                            push_key(&ident.name, keys, seen);
                        }
                        PropertyKey::StringLiteral(slit) => {
                            push_key(&slit.value, keys, seen);
                        }
                        PropertyKey::NumericLiteral(nlit) => {
                            // `{ 42: v }` — the runtime property key is
                            // ToString(42) = "42" (JS ToPropertyKey on
                            // numbers). The wrapper emits it through the
                            // alias form (`export { local as "42" }`)
                            // because "42" is not an identifier.
                            push_key(&numeric_key_string(nlit.value), keys, seen);
                        }
                        _ => {}
                    }
                }
                // ObjectPropertyKind::SpreadProperty — skipped intentionally.
            }
        }
        return;
    }

    // -- Case 2: exports.<prop> = <value> -------------------------------
    // The left side must be `exports.<prop>` where <prop> is a plain
    // identifier (not computed). Note: AssignmentTarget inherits the
    // MemberExpression variants via `inherit_variants!`, so the variant is
    // `AssignmentTarget::StaticMemberExpression` (NOT
    // `AssignmentTargetStaticMemberExpression`).
    if let AssignmentTarget::StaticMemberExpression(member) = &assign.left {
        if is_identifier_named(&member.object, "exports") {
            // property is an IdentifierName; we take its text directly.
            push_key(&member.property.name, keys, seen);
        }
    }
}

/// Check whether an AssignmentTarget is the expression `module.exports`.
fn is_module_exports(target: &AssignmentTarget) -> bool {
    if let AssignmentTarget::StaticMemberExpression(member) = target {
        // member.object must be the identifier `module`
        // member.property must be `exports`
        if is_identifier_named(&member.object, "module") && member.property.name == "exports" {
            return true;
        }
    }
    false
}

/// Check whether an Expression is a plain IdentifierReference with the given
/// name.
fn is_identifier_named(expr: &Expression, name: &str) -> bool {
    if let Expression::Identifier(ident) = expr {
        return ident.name == name;
    }
    false
}

/// Handle `Object.defineProperty(exports, 'key', ...)` call expressions.
fn collect_from_define_property_call(
    call: &oxc_ast::ast::CallExpression,
    keys: &mut Vec<String>,
    seen: &mut ::std::collections::HashSet<String>,
) {
    // callee must be `Object.defineProperty`
    let Expression::StaticMemberExpression(callee_member) = &call.callee else {
        return;
    };
    if !is_identifier_named(&callee_member.object, "Object") {
        return;
    }
    if callee_member.property.name != "defineProperty" {
        return;
    }

    // arguments[0] must be `exports`
    let Some(Argument::Identifier(exports_ident)) = call.arguments.first() else {
        return;
    };
    if exports_ident.name != "exports" {
        return;
    }

    // arguments[1] must be a string literal — that is the export key.
    let Some(Argument::StringLiteral(key_lit)) = call.arguments.get(1) else {
        return;
    };

    push_key(&key_lit.value, keys, seen);
}

/// Push a key into the ordered output list, filtering out default/__esModule
/// and de-duplicating.
fn push_key(key: &str, keys: &mut Vec<String>, seen: &mut ::std::collections::HashSet<String>) {
    if key == "default" || key == "__esModule" {
        return;
    }
    if seen.insert(key.to_string()) {
        keys.push(key.to_string());
    }
}

/// Render a numeric literal's JS ToPropertyKey string form.
///
/// JS: `ToString(42)` = `"42"` (integral f64 prints without decimal point).
/// Rust f64 Display already matches for the common range (`42.0` → `"42"`,
/// `1.5` → `"1.5"`). Two divergences handled/registered:
/// - `-0.0`: JS ToString gives `"0"`, Rust gives `"-0"` → normalized.
/// - `|v| >= 1e21`: JS switches to exponential notation (`"1e+21"`), Rust
///   prints the full decimal — extremely rare as an export key, registered
///   as a known non-conformance rather than hand-rolling ToString.
fn numeric_key_string(value: f64) -> String {
    if value == 0.0 {
        return "0".to_string();
    }
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shorthand_and_key_value() {
        let src = "module.exports = { collectEntries, mkdirSync, readFileSync };";
        let keys = extract_cjs_named_exports(src);
        assert_eq!(keys, vec!["collectEntries", "mkdirSync", "readFileSync"]);
    }

    #[test]
    fn test_string_literal_key() {
        let src = r#"module.exports = { "custom-key": function() {} };"#;
        let keys = extract_cjs_named_exports(src);
        assert_eq!(keys, vec!["custom-key"]);
    }

    #[test]
    fn test_exports_dot_assignment() {
        let src = r#"
exports.foo = function() {};
exports.bar = 42;
exports.default = function() {};
exports.__esModule = true;
"#;
        let keys = extract_cjs_named_exports(src);
        assert_eq!(keys, vec!["foo", "bar"]);
    }

    #[test]
    fn test_define_property() {
        let src = r#"Object.defineProperty(exports, 'baz', { get: function() { return 1; } });"#;
        let keys = extract_cjs_named_exports(src);
        assert_eq!(keys, vec!["baz"]);
    }

    #[test]
    fn test_computed_and_spread_skipped() {
        let src = r#"module.exports = { [computed]: 1, ...spread, valid: 2 };"#;
        let keys = extract_cjs_named_exports(src);
        assert_eq!(keys, vec!["valid"]);
    }

    #[test]
    fn test_default_and_esmodule_filtered() {
        let src = r#"module.exports = { default: fn, __esModule: true, real: 1 };"#;
        let keys = extract_cjs_named_exports(src);
        assert_eq!(keys, vec!["real"]);
    }

    #[test]
    fn test_empty_source() {
        let keys = extract_cjs_named_exports("");
        assert!(keys.is_empty());
    }

    #[test]
    fn test_no_cjs_exports() {
        let src = "var x = 1; console.log(x);";
        let keys = extract_cjs_named_exports(src);
        assert!(keys.is_empty());
    }

    #[test]
    fn test_dedup() {
        let src = r#"
exports.foo = 1;
module.exports = { foo: 2, bar: 3 };
exports.foo = 4;
"#;
        let keys = extract_cjs_named_exports(src);
        assert_eq!(keys, vec!["foo", "bar"]);
    }

    // ─── GOLDEN real-file regression (node common fixtures, farm-derived) ───

    /// node test/common/fs.js — the 69/96-fixture load-bearing carrier.
    /// Real shape: multi-line `module.exports = { shorthand, ... }`.
    #[test]
    fn golden_real_fs_js() {
        let src = include_str!("testdata/golden_common/fs.js");
        let keys = extract_cjs_named_exports(src);
        // The exact assertion the GOLDEN fixtures need at SM link time.
        assert!(keys.contains(&"nextdir".to_string()), "fs.js must export nextdir, got {:?}", keys);
        assert!(
            keys.contains(&"assertDirEquivalent".to_string()),
            "fs.js must export assertDirEquivalent, got {:?}",
            keys
        );
        assert!(
            keys.contains(&"collectEntries".to_string()),
            "fs.js must export collectEntries, got {:?}",
            keys
        );
        assert_eq!(keys, vec!["nextdir", "assertDirEquivalent", "collectEntries"]);
    }

    /// node test/common/watch.js — `import { skipIfNoWatch }` (5 fixtures).
    #[test]
    fn golden_real_watch_js() {
        let src = include_str!("testdata/golden_common/watch.js");
        let keys = extract_cjs_named_exports(src);
        assert!(
            keys.contains(&"skipIfNoWatch".to_string()),
            "watch.js must export skipIfNoWatch, got {:?}",
            keys
        );
        assert!(
            keys.contains(&"skipIfNoWatchModeSignals".to_string()),
            "watch.js must export skipIfNoWatchModeSignals, got {:?}",
            keys
        );
    }

    /// node test/common/tmpdir.js — `import tmpdir from` + named access
    /// (`tmpdir.resolve` etc. via the default export object).
    #[test]
    fn golden_real_tmpdir_js() {
        let src = include_str!("testdata/golden_common/tmpdir.js");
        let keys = extract_cjs_named_exports(src);
        for expected in ["fileURL", "hasEnoughSpace", "refresh", "resolve", "path"] {
            assert!(
                keys.contains(&expected.to_string()),
                "tmpdir.js must export {}, got {:?}",
                expected,
                keys
            );
        }
    }

    /// node test/common/fixtures.js — `import fixtures from` default carrier.
    #[test]
    fn golden_real_fixtures_js() {
        let src = include_str!("testdata/golden_common/fixtures.js");
        let keys = extract_cjs_named_exports(src);
        for expected in
            ["fixturesDir", "path", "fileURL", "readSync", "readKey", "readKeys", "utf8TestText", "utf8TestTextPath"]
        {
            assert!(
                keys.contains(&expected.to_string()),
                "fixtures.js must export {}, got {:?}",
                expected,
                keys
            );
        }
    }

    /// node test/common/index.js — `module.exports = new Proxy(common, ...)`.
    /// Statically UNKNOWABLE: honest extraction is `[]` (same as node's
    /// cjs-module-lexer; node fixtures consume index.js via `require('.')`
    /// precisely because ESM named import of a Proxy-wrapped CJS export
    /// fails in real Node too). Pins the honesty boundary — a future change
    /// that starts inventing keys for this file is a regression.
    #[test]
    fn golden_real_index_js_proxy_is_honestly_empty() {
        let src = include_str!("testdata/golden_common/index.js");
        let keys = extract_cjs_named_exports(src);
        assert!(
            keys.is_empty(),
            "index.js Proxy-wrapped exports are statically unknowable; got {:?}",
            keys
        );
    }

    // ─── Number-literal keys ──────────────────────────────────────────────

    #[test]
    fn test_numeric_literal_keys() {
        let src = r#"module.exports = { 0: fn, 42: fn2, 1.5: fn3 };"#;
        let keys = extract_cjs_named_exports(src);
        // JS ToPropertyKey: ToString(0)="0", ToString(42)="42", ToString(1.5)="1.5"
        assert_eq!(keys, vec!["0", "42", "1.5"]);
    }

    #[test]
    fn test_numeric_negative_zero_normalized() {
        let src = r#"module.exports = { [-0]: fn };"#;
        // [-0] is a COMPUTED key — statically skipped (honest boundary).
        let keys = extract_cjs_named_exports(src);
        assert!(keys.is_empty(), "computed [-0] must be skipped, got {:?}", keys);
    }

    // ─── Shape-coverage round 2 (coordinator-requested forms) ────────────

    /// The exact fs.js:45-49 real shape: multi-line shorthand-only literal.
    #[test]
    fn test_multiline_shorthand_object_literal() {
        let src = "module.exports = {\n  nextdir,\n  assertDirEquivalent,\n  collectEntries,\n};";
        let keys = extract_cjs_named_exports(src);
        assert_eq!(keys, vec!["nextdir", "assertDirEquivalent", "collectEntries"]);
    }

    /// Two `module.exports = {...}` assignments — union of keys (lexer-form:
    /// no control-flow chasing, last-assignment wins at runtime, both
    /// statically enumerable).
    #[test]
    fn test_multiple_module_exports_assignments_union() {
        let src = r#"
module.exports = { alpha: 1 };
module.exports = { beta: 2, alpha: 9 };
"#;
        let keys = extract_cjs_named_exports(src);
        assert_eq!(keys, vec!["alpha", "beta"]);
    }

    /// Method-shorthand keys (`{ foo() {} }`) are also static export keys.
    #[test]
    fn test_method_shorthand_keys() {
        let src = r#"module.exports = { foo() { return 1; }, bar: 2 };"#;
        let keys = extract_cjs_named_exports(src);
        assert_eq!(keys, vec!["foo", "bar"]);
    }
}
