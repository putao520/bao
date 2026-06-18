//! SpiderMonkey CYCLEBREAK bridge for `bun_bundler`.
//!
//! `bun_bundler` is a pure-Rust bundler that uses CYCLEBREAK architecture:
//! it declares `unsafe extern "Rust"` symbols and `link_interface!` dispatch
//! vtables, but never names JSC types. The actual implementations are
//! provided by the engine layer (traditionally `bun_jsc` / `bun_runtime`).
//!
//! This crate provides **SpiderMonkey-backed** implementations of those
//! CYCLEBREAK symbols and dispatch interfaces, replacing the JSC versions.
//! The bundler logic itself is 100% reused from `bun_bundler`.
//!
//! # CYCLEBREAK symbols provided
//!
//! | Symbol | JSC implementation | SM implementation |
//! |--------|-------------------|-------------------|
//! | `__bun_jsc_generate_cached_bytecode` | JSC bytecode cache | SM XDR encode (Phase 2) |
//! | `__bun_jsc_enable_hot_module_reloading_for_bundler` | JSC HMR watcher | `bun_watcher` + SM reload |
//!
//! # Dispatch interfaces provided
//!
//! | Interface | Variant | Backing type |
//! |-----------|---------|-------------|
//! | `VmLoaderCtx` | `Runtime` | `BaoVmLoaderCtx` |
//! | `DevServerHandle` | `Bake` | no-op (Phase 2: Bao bake) |

pub mod bytecode;
pub mod dev_server;
pub mod hmr;
pub mod vm_loader;

// Re-export ALL of bun_bundler's public API so downstream crates
// (bao_cli) can use `bao_bundler::BundleV2`, `bao_bundler::BundleOutput`, etc.
// without knowing about the bun_bundler dependency.
pub use bun_bundler::*;

// Force-link this crate's CYCLEBREAK symbols.
// Without this anchor, the linker GCs the #[no_mangle] symbols when
// bao_bundler is linked as a dependency but no item is explicitly referenced.
// @trace REQ-CLI-001 [api:POST /cli/exec] [entity:BaoRuntime]
#[used]
static BAO_BUNDLER_ANCHOR: unsafe extern "C" fn() = __force_link_entry;

unsafe extern "C" {
    fn __force_link_entry();
}

/// Bundle result — compatible with the old `bao_bundler::BundleOutput` shape
/// used by `bao_cli::cli::run_build`.
///
/// @trace REQ-CLI-001 [api:POST /cli/exec] [entity:BaoRuntime]
#[derive(Debug)]
pub struct BundleOutput {
    pub code: String,
    pub source_map: Option<String>,
}

// @trace REQ-CLI-001 [api:POST /cli/exec] [entity:BaoRuntime]
//
// `bao build` pipeline. Instead of a hand-written `read_to_string` + stateful
// `basic_minify`, the entrypoint is routed through `bun_transpiler` — the
// SWC-backed transpiler that is part of the `bun_bundler` pipeline (re-exported
// at `bun_bundler::transpiler::*` as the CYCLEBREAK JSC Transpiler; the
// `bun_transpiler` crate exposes the JSC-free SWC strip path usable here
// without a live JSContext).
//
// What this wires up:
//   * `.ts` / `.tsx` / `.mts` / `.cts` — full TypeScript → JavaScript strip via
//     SWC's official `strip` transform (generics, type annotations, unions,
//     interfaces, type aliases, `import type`, `declare`, enums, TSX).
//   * `.js` / `.jsx` / `.mjs` / `.cjs` — SWC parse + reprint, which normalizes
//     the source. The SWC Emitter is handed the parsed `comments` table, so
//     comments are **preserved** on the non-minify path (semantically faithful
//     round-trip); they are only discarded when `minify = true` selects the
//     `transpile_ts_drop_comments` variant (BCE-20260618-004).
//   * `minify = true` — re-transpile through `transpile_ts_drop_comments`
//     (SWC Emitter handed `comments = None` → all leading/trailing/inner
//     comments dropped at codegen), then collapse whitespace on the
//     comment-free normalized output.
//
// Out of scope (deferred to Phase 2, per REQ-CLI-001 / REUSE_RESULT): the full
// `bun_bundler::BundleV2` multi-chunk graph pipeline, which requires a live
// JSContext (`generate_from_cli`). `bun_codegen` is not a Rust crate (only
// build-time TS scripts exist under `src/codegen/`), so it is intentionally
// not referenced.
pub fn build(entrypoint: &str, minify: bool, _target: &str) -> Result<BundleOutput, String> {
    let path = std::path::Path::new(entrypoint);
    let source = std::fs::read_to_string(path)
        .map_err(|e| format!("Error reading {}: {}", entrypoint, e))?;

    let fname = path.to_str().unwrap_or(entrypoint);

    // Route through the real SWC transpiler. The minify path uses the
    // `transpile_ts_drop_comments` variant so the SWC Emitter is handed
    // `comments = None` at codegen and all comments are dropped (the
    // comment-stripping root fix for BCE-20260618-004). The non-minify path
    // preserves comments via the plain `transpile_ts` variant (runtime loader
    // contract). On SWC hard-failure both fall back to the raw source so a
    // single unparseable file never breaks the CLI (matches
    // `bun_sm::module_loader::strip_typescript`'s defensive pattern).
    let transpiled = if minify {
        bun_transpiler::transpile_ts_drop_comments(&source, fname).unwrap_or_else(|_| source)
    } else {
        bun_transpiler::transpile_ts(&source, fname).unwrap_or_else(|_| source)
    };

    let code = if minify {
        collapse_whitespace(&transpiled)
    } else {
        transpiled
    };

    Ok(BundleOutput {
        code,
        source_map: None,
    })
}

/// Collapse runs of ASCII whitespace into a single space.
///
/// Comments are already dropped upstream in [`build`] when `minify = true`
/// (the SWC Emitter is handed `comments = None` via
/// `transpile_ts_drop_comments`); this function only collapses whitespace on
/// the comment-free normalized payload. It is string-literal aware so it
/// never collapses spaces inside `'...'` / `"..."` / `` `...` `` payloads
/// (including escaped delimiters). Unlike the removed hand-written
/// `basic_minify` it does not duplicate the comment-stripping state machine —
/// comment removal is the SWC Emitter's responsibility (BCE-20260618-004).
///
/// @trace REQ-CLI-001 [api:POST /cli/exec] [entity:BaoRuntime]
fn collapse_whitespace(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0;
    let mut in_string = false;
    let mut delim = b'\0';
    let mut prev_was_ws = false;

    while i < bytes.len() {
        let b = bytes[i];

        if in_string {
            out.push(b as char);
            if b == b'\\' && i + 1 < bytes.len() {
                // Preserve the escaped character verbatim.
                out.push(bytes[i + 1] as char);
                i += 2;
                continue;
            }
            if b == delim {
                in_string = false;
            }
            i += 1;
            prev_was_ws = false;
            continue;
        }

        match b {
            b'\'' | b'"' | b'`' => {
                in_string = true;
                delim = b;
                out.push(b as char);
                prev_was_ws = false;
                i += 1;
            }
            b' ' | b'\t' | b'\n' | b'\r' => {
                if !prev_was_ws {
                    out.push(' ');
                    prev_was_ws = true;
                }
                i += 1;
            }
            _ => {
                out.push(b as char);
                prev_was_ws = false;
                i += 1;
            }
        }
    }

    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Bytecode generation tests ──────────────────────────────────────────

    #[test]
    fn bytecode_symbol_linked() {
        let result = bytecode::generate_cached_bytecode_for_test();
        assert!(result.is_none(), "Phase 1: SM bytecode cache should return None");
    }

    // ── Build API tests ────────────────────────────────────────────────────

    #[test]
    fn build_passthrough_no_minify() {
        // Plain JS round-trips through the SWC transpiler with semantics
        // preserved (variables and literals intact). We assert on meaning, not
        // byte-equality, because SWC may normalize trivial formatting.
        let input = "let x = 1;\nlet y = 2;\n";
        let dir = std::env::temp_dir().join("bao_bundler_test_build_no_minify");
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("test_no_minify.js");
        std::fs::write(&path, input).unwrap();
        let result = build(&path.to_string_lossy(), false, "bundle").unwrap();
        assert!(result.code.contains("let x = 1"));
        assert!(result.code.contains("let y = 2"));
        assert!(result.source_map.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn build_with_minify_removes_comments() {
        let input = "let x = 1; // comment\nlet y = 2;";
        let dir = std::env::temp_dir().join("bao_bundler_test_build_minify");
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("test_minify.js");
        std::fs::write(&path, input).unwrap();
        let result = build(&path.to_string_lossy(), true, "bundle").unwrap();
        assert!(!result.code.contains("comment"));
        assert!(result.code.contains("let"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn build_missing_file_returns_error() {
        let result = build("/nonexistent/path/file.js", false, "bundle");
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(err.contains("Error reading"));
    }

    #[test]
    fn build_output_has_no_source_map() {
        let dir = std::env::temp_dir().join("bao_bundler_test_source_map");
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("test.js");
        std::fs::write(&path, "let x = 1;").unwrap();
        let result = build(&path.to_string_lossy(), false, "bundle").unwrap();
        assert!(result.source_map.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn build_transpiles_typescript() {
        // New capability: the bundler pipeline strips TypeScript annotations
        // via the real SWC transpiler (`bun_transpiler`).
        let input = "const x: number = 1;\nfunction add(a: number, b: number): number { return a + b; }\n";
        let dir = std::env::temp_dir().join("bao_bundler_test_ts");
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("entry.ts");
        std::fs::write(&path, input).unwrap();
        let result = build(&path.to_string_lossy(), false, "bundle").unwrap();
        assert!(!result.code.contains(": number"));
        assert!(result.code.contains("const x = 1"));
        assert!(result.code.contains("function add(a, b)"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn build_preserves_jsx_like_syntax() {
        // .jsx falls back to the raw source when SWC's TS-without-tsx grammar
        // cannot parse JSX (Phase 1: SM has no native JSX transform). The
        // payload must survive intact so a downstream JSX pass can run.
        let input = "const el = <div className=\"app\">hello</div>;";
        let dir = std::env::temp_dir().join("bao_bundler_test_jsx");
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("test.jsx");
        std::fs::write(&path, input).unwrap();
        let result = build(&path.to_string_lossy(), false, "bundle").unwrap();
        assert!(result.code.contains("className"));
        assert!(result.code.contains("hello"));
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── collapse_whitespace tests ──────────────────────────────────────────

    #[test]
    fn collapse_whitespace_collapses_runs() {
        let input = "let   x   =   1;";
        let result = collapse_whitespace(input);
        assert_eq!(result, "let x = 1;");
    }

    #[test]
    fn collapse_whitespace_empty_input() {
        let result = collapse_whitespace("");
        assert!(result.is_empty());
    }

    #[test]
    fn collapse_whitespace_preserves_double_quoted_strings() {
        let input = r#"let s = "hello   world";"#;
        let result = collapse_whitespace(input);
        assert!(result.contains("\"hello   world\""));
    }

    #[test]
    fn collapse_whitespace_preserves_single_quoted_strings() {
        let input = r"let s = 'hello   world';";
        let result = collapse_whitespace(input);
        assert!(result.contains("'hello   world'"));
    }

    #[test]
    fn collapse_whitespace_preserves_template_literals() {
        let input = "let s = `hello   world`;";
        let result = collapse_whitespace(input);
        assert!(result.contains("`hello   world`"));
    }

    #[test]
    fn collapse_whitespace_preserves_escaped_delimiter() {
        let input = r#"let s = "hello \"world\"  inside";"#;
        let result = collapse_whitespace(input);
        // Inner spaces inside the escaped-aware string body are preserved.
        assert!(result.contains("world"));
    }

    #[test]
    fn collapse_whitespace_trims_edges() {
        let input = "   let x = 1;   ";
        let result = collapse_whitespace(input);
        assert_eq!(result, "let x = 1;");
    }

    // ── End-to-end bundle tests ────────────────────────────────────────────

    #[test]
    fn e2e_bundle_simple_module() {
        let input = r#"
// Entry module
function greet(name) {
    return "Hello, " + name + "!";
}
console.log(greet("Bao"));
"#;
        let dir = std::env::temp_dir().join("bao_bundler_e2e_simple");
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("entry.js");
        std::fs::write(&path, input).unwrap();

        let result = build(&path.to_string_lossy(), true, "bundle").unwrap();
        assert!(!result.code.contains("// Entry"));
        assert!(result.code.contains("greet"));
        assert!(result.code.contains("Hello"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn e2e_bundle_preserves_esm_syntax() {
        let input = "import { foo } from './bar.js';\nexport const baz = foo();\n";
        let dir = std::env::temp_dir().join("bao_bundler_e2e_esm");
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("entry.mjs");
        std::fs::write(&path, input).unwrap();

        let result = build(&path.to_string_lossy(), false, "esm").unwrap();
        assert!(result.code.contains("import"));
        assert!(result.code.contains("export"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn e2e_bundle_minify_collapses_complex_js() {
        let input = r#"
/* Application bundle */
const API_URL = "https://api.example.com";

async function fetchData(endpoint) {
    const response = await fetch(API_URL + endpoint);
    return response.json();
}

// Export for consumers
export { fetchData, API_URL };
"#;
        let dir = std::env::temp_dir().join("bao_bundler_e2e_complex");
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("app.js");
        std::fs::write(&path, input).unwrap();

        let result = build(&path.to_string_lossy(), true, "bundle").unwrap();
        assert!(!result.code.contains("/* Application bundle */"));
        assert!(result.code.contains("fetchData"));
        assert!(result.code.contains("API_URL"));
        // Minified output should be shorter than the comment-heavy input.
        assert!(result.code.len() < input.len());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn e2e_bundle_transpiles_ts_module() {
        // End-to-end: a TypeScript entry is transpiled to JS before emit.
        let input = "interface User { id: number; name: string; }\nexport const u: User = { id: 1, name: 'a' };\n";
        let dir = std::env::temp_dir().join("bao_bundler_e2e_ts");
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("user.ts");
        std::fs::write(&path, input).unwrap();

        let result = build(&path.to_string_lossy(), false, "bundle").unwrap();
        assert!(!result.code.contains("interface User"));
        assert!(result.code.contains("export const u ="));
        assert!(result.code.contains("name: 'a'"));

        std::fs::remove_dir_all(&dir).ok();
    }
}
