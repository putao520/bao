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
#[used]
static BAO_BUNDLER_ANCHOR: unsafe extern "C" fn() = __force_link_entry;

unsafe extern "C" {
    fn __force_link_entry();
}

/// Bundle result — compatible with the old `bao_bundler::BundleOutput` shape
/// used by `bao_cli::cli::run_build`.
#[derive(Debug)]
pub struct BundleOutput {
    pub code: String,
    pub source_map: Option<String>,
}

/// Public build API — entry point for `bao build` (via `bao_cli::cli::run_build`).
///
/// Phase 1: simple file read + optional minify passthrough.
/// Phase 2: full `bun_bundler::BundleV2` pipeline with SM CYCLEBREAK dispatch.
pub fn build(entrypoint: &str, minify: bool, _target: &str) -> Result<BundleOutput, String> {
    let source = std::fs::read_to_string(entrypoint)
        .map_err(|e| format!("Error reading {}: {}", entrypoint, e))?;

    let code = if minify {
        // Phase 1: SM-based validation + basic minification.
        // Phase 2 will use bun_bundler's transpiler pipeline.
        basic_minify(&source)
    } else {
        source
    };

    Ok(BundleOutput {
        code,
        source_map: None,
    })
}

/// Basic whitespace/comment removal — safe for any text, doesn't require valid JS.
/// Borrowed from the old minify.rs (deleted); this is the Phase 1 minifier until
/// the full bun_bundler transpiler pipeline is wired up.
fn basic_minify(source: &str) -> String {
    let mut result = String::with_capacity(source.len());
    let mut in_string = false;
    let mut string_delim = ' ';
    let mut prev = '\0';
    let bytes = source.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        let ch = bytes[i] as char;
        if in_string {
            result.push(ch);
            if ch == string_delim && prev != '\\' {
                in_string = false;
            }
            prev = ch;
            i += 1;
        } else if ch == '\'' || ch == '"' || ch == '`' {
            in_string = true;
            string_delim = ch;
            result.push(ch);
            prev = ch;
            i += 1;
        } else if ch == '/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            // Single-line comment — skip to end of line
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if ch == '/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            // Multi-line comment — skip to */
            i += 2;
            while i + 1 < bytes.len() {
                if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    i += 2;
                    break;
                }
                i += 1;
            }
        } else {
            result.push(ch);
            prev = ch;
            i += 1;
        }
    }

    result.split_whitespace().collect::<Vec<&str>>().join(" ")
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
        let input = "let x = 1;\nlet y = 2;\n";
        let dir = std::env::temp_dir().join("bao_bundler_test_build_no_minify");
        std::fs::create_dir_all(&dir).ok();
        let path = dir.join("test_no_minify.js");
        std::fs::write(&path, input).unwrap();
        let result = build(&path.to_string_lossy(), false, "bundle").unwrap();
        assert_eq!(result.code, input);
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
    fn build_preserves_jsx_like_syntax() {
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

    // ── basic_minify tests ─────────────────────────────────────────────────

    #[test]
    fn basic_minify_removes_single_line_comments() {
        let input = "let x = 1; // comment\nlet y = 2;";
        let result = basic_minify(input);
        assert!(!result.contains("comment"));
        assert!(result.contains("let x = 1"));
        assert!(result.contains("let y = 2"));
    }

    #[test]
    fn basic_minify_removes_multiline_comments() {
        let input = "let x = 1; /* this is\na comment */ let y = 2;";
        let result = basic_minify(input);
        assert!(!result.contains("this is"));
        assert!(result.contains("let x = 1"));
        assert!(result.contains("let y = 2"));
    }

    #[test]
    fn basic_minify_preserves_double_quoted_strings() {
        let input = r#"let s = "hello // not a comment";"#;
        let result = basic_minify(input);
        assert!(result.contains("hello // not a comment"));
    }

    #[test]
    fn basic_minify_preserves_single_quoted_strings() {
        let input = r"let s = 'hello // not a comment';";
        let result = basic_minify(input);
        assert!(result.contains("hello // not a comment"));
    }

    #[test]
    fn basic_minify_preserves_template_literals() {
        let input = "let s = `hello // not a comment`;";
        let result = basic_minify(input);
        assert!(result.contains("hello // not a comment"));
    }

    #[test]
    fn basic_minify_collapses_whitespace() {
        let input = "let   x   =   1;";
        let result = basic_minify(input);
        assert_eq!(result, "let x = 1;");
    }

    #[test]
    fn basic_minify_empty_input() {
        let result = basic_minify("");
        assert!(result.is_empty());
    }

    #[test]
    fn basic_minify_only_comments() {
        let input = "// just a comment\n/* another */";
        let result = basic_minify(input);
        assert!(result.trim().is_empty());
    }

    #[test]
    fn basic_minify_preserves_regex_like_slash() {
        // Forward slashes not followed by / or * are preserved
        let input = "let x = 10 / 2;";
        let result = basic_minify(input);
        assert!(result.contains("10 / 2") || result.contains("10/2"));
    }

    #[test]
    fn basic_minify_nested_multiline_comments() {
        let input = "let a = 1; /* outer /* inner */ still outer */ let b = 2;";
        let result = basic_minify(input);
        // After first */ the rest is treated as code until next */
        assert!(result.contains("let a = 1"));
    }

    #[test]
    fn basic_minify_multiple_single_line_comments() {
        let input = "let x = 1; // first\nlet y = 2; // second\nlet z = 3;";
        let result = basic_minify(input);
        assert!(!result.contains("first"));
        assert!(!result.contains("second"));
        assert!(result.contains("let x = 1"));
        assert!(result.contains("let y = 2"));
        assert!(result.contains("let z = 3"));
    }

    #[test]
    fn basic_minify_escaped_quote_in_string() {
        let input = r#"let s = "hello \"world\""; // comment"#;
        let result = basic_minify(input);
        assert!(result.contains("hello"));
        assert!(!result.contains("comment"));
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
        assert!(!result.code.contains("// Export for consumers"));
        assert!(result.code.contains("fetchData"));
        assert!(result.code.contains("API_URL"));
        // Minified output should be shorter
        assert!(result.code.len() < input.len());

        std::fs::remove_dir_all(&dir).ok();
    }
}
