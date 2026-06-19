//! `bun_transpiler` — TypeScript → JavaScript transpilation.
//!
//! Filled in for GAP-1: replaces the hand-written `strip_typescript` with the
//! official `swc_ecma_transforms_typescript::strip` transform, supporting the
//! full TypeScript grammar (generics, type annotations, unions, interfaces,
//! type aliases, `import type`, `declare`, enums, etc.).
//!
//! Re-exports `bun_bundler::transpiler::*` for the legacy bundler pipeline.
//!
//! @trace REQ-ENG-005 [api:POST /module/resolve] [entity:ModuleSource]

pub use bun_bundler::transpiler::*;

use std::path::Path;

use swc_common::comments::SingleThreadedComments;
use swc_common::errors::{ColorConfig, Handler};
use swc_common::sync::Lrc;
use swc_common::{FileName, Globals, Mark, SourceMap, GLOBALS};
use swc_ecma_ast::EsVersion;
use swc_ecma_codegen::to_code_default;
use swc_ecma_parser::{lexer::Lexer, Parser, StringInput, Syntax, TsSyntax};
use swc_ecma_transforms_base::{fixer::fixer, hygiene::hygiene, resolver};
use swc_ecma_transforms_typescript::strip;

/// Error returned by [`transpile_ts`].
///
/// @trace REQ-ENG-005 [api:POST /module/resolve] [entity:ModuleSource]
#[derive(Debug)]
pub struct TranspileError {
    pub message: String,
}

impl std::fmt::Display for TranspileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "transpile_ts error: {}", self.message)
    }
}

impl std::error::Error for TranspileError {}

/// Transpile a TypeScript / TSX source string into JavaScript by stripping
/// type-only constructs via SWC's official TypeScript strip transform.
///
/// - `tsx = true` for `.tsx` files (JSX + TS), `false` otherwise.
/// - Target is ES2020 (matches SpiderMonkey's modern feature set).
/// - Zero runtime: only types are removed; no down-leveling of syntax beyond
///   what SWC's strip performs. This mirrors `tsc --isolatedModules` with
///   `verbatimModuleSyntax` and Node.js's type-stripping.
/// - Comments are **preserved** (SWC Emitter is handed the parsed `comments`
///   table). This is the runtime-module-loader contract: comment-preserved
///   output is semantically faithful to the source. Use
///   [`transpile_ts_drop_comments`] for the bundler minify path that needs
///   comments discarded.
///
/// On any failure returns `TranspileError`; callers fall back to the legacy
/// hand-written stripper (kept as defensive fallback in `module_loader`).
///
/// @trace REQ-ENG-005 [api:POST /module/resolve] [entity:ModuleSource]
pub fn transpile_ts(source: &str, filename: &str) -> Result<String, TranspileError> {
    transpile_ts_impl(source, filename, false)
}

/// Transpile a TypeScript / TSX source string into JavaScript, **discarding
/// all comments** from the emitted output.
///
/// Same grammar / strip pipeline as [`transpile_ts`], but the SWC Emitter is
/// handed `comments = None` at codegen, so leading / trailing / inner
/// comments parsed from the source are dropped from the emitted string. This
/// is the contract the `bao_bundler` minify path requires (a comment-free
/// normalized payload that downstream whitespace-collapse can shrink).
///
/// On any failure returns `TranspileError`; callers fall back to the raw
/// source.
///
/// @trace REQ-CLI-001 [api:POST /cli/exec] [entity:BaoRuntime]
pub fn transpile_ts_drop_comments(
    source: &str,
    filename: &str,
) -> Result<String, TranspileError> {
    transpile_ts_impl(source, filename, true)
}

// @trace REQ-ENG-005 [api:POST /module/resolve] [entity:ModuleSource]
fn transpile_ts_impl(
    source: &str,
    filename: &str,
    drop_comments: bool,
) -> Result<String, TranspileError> {
    let path = Path::new(filename);
    let tsx = matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("tsx")
    );

    transpile_ts_with(source, tsx, filename, drop_comments)
}

// @trace REQ-ENG-005 [api:POST /module/resolve] [entity:ModuleSource]
fn transpile_ts_with(
    source: &str,
    tsx: bool,
    filename: &str,
    drop_comments: bool,
) -> Result<String, TranspileError> {
    let cm: Lrc<SourceMap> = Default::default();
    let handler = Handler::with_tty_emitter(ColorConfig::Auto, true, false, Some(cm.clone()));

    let fm = cm.new_source_file(
        FileName::Custom(filename.to_string()).into(),
        source.to_string(),
    );

    let comments: SingleThreadedComments = SingleThreadedComments::default();

    // Parser: TypeScript syntax (with TSX flag). `decorators: true` keeps
    // stage-2 decorator syntax; `dts: false` so we emit runtime code.
    let syntax = Syntax::Typescript(TsSyntax {
        tsx,
        decorators: true,
        dts: false,
        no_early_errors: false,
        disallow_ambiguous_jsx_like: false,
    });

    let lexer = Lexer::new(syntax, EsVersion::Es2020, StringInput::from(&*fm), Some(&comments));

    let mut parser = Parser::new_from(lexer);

    // Surface parse diagnostics but do not hard-fail: SWC's error recovery
    // often still yields a usable program; real errors are caught downstream
    // by SpiderMonkey when the JS is evaluated.
    for e in parser.take_errors() {
        e.into_diagnostic(&handler).emit();
    }

    let program = parser
        .parse_program()
        .map_err(|e| {
            e.into_diagnostic(&handler).emit();
            TranspileError {
                message: format!("swc parse_program failed for {filename}"),
            }
        })?;

    // Apply the official TS strip pipeline (mirrors swc's `ts_to_js.rs`):
    //   resolver → strip → hygiene → fixer
    let globals = Globals::default();
    let out = GLOBALS.set(&globals, || {
        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();

        let p = program.apply(resolver(unresolved_mark, top_level_mark, true));
        let p = p.apply(strip(unresolved_mark, top_level_mark));
        let p = p.apply(hygiene());
        let p = p.apply(fixer(Some(&comments)));
        p
    });

    // BCE-20260618-004: the SWC Emitter only emits comments when handed a
    // `Some(comments)` table. `Some` → comments preserved (runtime loader
    // contract); `None` → comments dropped (bundler minify contract).
    let comments_for_emit: Option<&dyn swc_common::comments::Comments> =
        if drop_comments { None } else { Some(&comments) };
    Ok(to_code_default(cm, comments_for_emit, &out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_simple_type_annotations() {
        let src = "const x: number = 1;\nfunction add(a: number, b: number): number { return a + b; }\n";
        let out = transpile_ts(src, "x.ts").expect("transpile ok");
        assert!(out.contains("const x = 1;"));
        assert!(out.contains("function add(a, b)"));
        assert!(!out.contains("number"));
    }

    #[test]
    fn strips_interface_and_type_alias() {
        let src = "interface User { id: number; name: string; }\ntype ID = number;\nconst u: User = { id: 1, name: 'a' };\n";
        let out = transpile_ts(src, "x.ts").expect("transpile ok");
        assert!(!out.contains("interface User"));
        assert!(!out.contains("type ID"));
        assert!(out.contains("const u = {"));
    }

    #[test]
    fn strips_generic_constraints_and_union_types() {
        // The buffer-concat.test.ts pattern the hand-written stripper could
        // not handle: function generic with `extends` + union.
        let src = "function concat<T extends { buffer: ArrayBuffer } | ArrayBuffer>(first: T): T[] {\n  const arr: T[] = [];\n  return arr;\n}\n";
        let out = transpile_ts(src, "x.ts").expect("transpile ok");
        // Type annotations gone, function body preserved.
        assert!(out.contains("function concat"));
        assert!(out.contains("const arr = [];"));
        assert!(!out.contains("ArrayBuffer"));
        assert!(!out.contains(": T"));
    }

    #[test]
    fn strips_import_type_and_export_type() {
        let src = "import type { Foo } from './foo';\nexport type Bar = string;\nexport const x = 1;\n";
        let out = transpile_ts(src, "x.ts").expect("transpile ok");
        assert!(!out.contains("import type"));
        assert!(!out.contains("export type Bar"));
        assert!(out.contains("export const x = 1"));
    }

    #[test]
    fn preserves_plain_javascript() {
        let src = "const x = 1;\nconst y = x * 2;\nexport { y };\n";
        let out = transpile_ts(src, "x.ts").expect("transpile ok");
        assert!(out.contains("const x = 1;"));
        assert!(out.contains("const y = x * 2;"));
        assert!(out.contains("export { y };"));
    }

    #[test]
    fn handles_tsx() {
        let src = "const el: JSX.Element = <div>{42}</div>;\nexport { el };\n";
        let out = transpile_ts(src, "x.tsx").expect("transpile ok");
        // TSX preserved as JSX (no runtime transform), TS annotation stripped.
        assert!(out.contains("<div>"));
        assert!(out.contains("{42}"));
        assert!(!out.contains("JSX.Element"));
    }

    // ── BCE-20260618-004 regression: comment preservation / drop contract ──

    #[test]
    fn transpile_ts_preserves_comments_runtime_contract() {
        // Runtime loader contract: comments are preserved so emitted JS is
        // semantically faithful to the source.
        let src = "// leading\nconst x = 1; /* trailing */\n";
        let out = transpile_ts(src, "x.ts").expect("transpile ok");
        assert!(out.contains("leading"), "line comment preserved: {out:?}");
        assert!(out.contains("trailing"), "block comment preserved: {out:?}");
        assert!(out.contains("const x = 1;"));
    }

    #[test]
    fn transpile_ts_drop_comments_removes_all_comments() {
        // BCE-20260618-004 regression: the bundler minify path must emit a
        // comment-free payload. Constructing this pattern (any combination of
        // line + block comments) MUST yield zero comment text in the output.
        let src = "// leading line comment\nconst API = \"v1\"; /* block comment */\n// trailing\n";
        let out = transpile_ts_drop_comments(src, "x.ts").expect("transpile ok");
        assert!(!out.contains("leading line comment"), "line comment dropped: {out:?}");
        assert!(!out.contains("block comment"), "block comment dropped: {out:?}");
        assert!(!out.contains("trailing"), "trailing comment dropped: {out:?}");
        assert!(out.contains("const API = \"v1\";"), "code preserved: {out:?}");
    }

    #[test]
    fn transpile_ts_drop_comments_still_strips_types() {
        // The drop-comments variant must still perform the full TS strip.
        let src = "// header\nconst x: number = 1;\nfunction add(a: number, b: number): number { return a + b; }\n";
        let out = transpile_ts_drop_comments(src, "x.ts").expect("transpile ok");
        assert!(!out.contains("header"));
        assert!(!out.contains("number"));
        assert!(out.contains("const x = 1"));
        assert!(out.contains("function add(a, b)"));
    }
}
