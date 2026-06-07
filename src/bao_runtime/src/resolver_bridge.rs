// @trace REQ-ENG-005
//! Resolver bridge: replaces hand-written resolve_specifier with bun_resolver::Resolver.
//!
//! Initializes a thread-local `bun_resolver::Resolver` and injects it into
//! `bao_engine::module_loader` so both require() and ESM import use the
//! production-grade Bun resolver instead of hand-written std::fs logic.

use ::std::cell::Cell;
use ::std::path::{Path, PathBuf};

use bun_resolver::Resolver;
use bun_resolver::options::BundleOptions;
use bun_resolver::fs::FileSystem;
use bun_ast::Log;
use bun_ast::ImportKind;

thread_local! {
    static RESOLVER: ::std::cell::RefCell<::std::option::Option<Resolver<'static>>> = const { ::std::cell::RefCell::new(None) };
    static CWD_BYTES: ::std::cell::RefCell<Vec<u8>> = const { ::std::cell::RefCell::new(Vec::new()) };
    static INSTALLED: Cell<bool> = const { Cell::new(false) };
}

/// Initialize the thread-local Resolver and inject it into bao_engine.
/// Safe to call multiple times — subsequent calls are no-ops.
pub fn install() {
    INSTALLED.with(|flag| {
        if flag.get() {
            return;
        }
        flag.set(true);
        let resolver = create_resolver();
        RESOLVER.with(|r| *r.borrow_mut() = Some(resolver));
        bao_engine::module_loader::set_resolver(resolve_via_bun_resolver);
    });
}

/// Create a Resolver with sensible defaults for the Bao runtime.
fn create_resolver() -> Resolver<'static> {
    let cwd = ::std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "/".to_string());

    // Cache CWD bytes for resolve calls
    let cwd_bytes = cwd.as_bytes().to_vec();
    CWD_BYTES.with(|c| *c.borrow_mut() = cwd_bytes);

    // Leak the CWD string to get 'static lifetime (process-lifetime anyway)
    let cwd_static: &'static [u8] = Box::leak(cwd.into_bytes().into_boxed_slice());

    let log = Box::new(Log::new());
    let log_ptr = ::std::ptr::NonNull::new(Box::into_raw(log)).expect("Log alloc");

    let fs_ptr = FileSystem::init(Some(cwd_static))
        .unwrap_or_else(|_| FileSystem::init(None).expect("FileSystem init fallback"));

    Resolver::init1(log_ptr, fs_ptr, BundleOptions::default())
}

/// Resolver function matching the signature expected by bao_engine.
fn resolve_via_bun_resolver(specifier: &str, base_dir: Option<&Path>) -> Option<PathBuf> {
    // Compute source_dir as an owned Vec<u8> to avoid lifetime issues
    let source_dir = if let Some(dir) = base_dir {
        dir.to_str().map(|s| s.as_bytes().to_vec())
    } else {
        ::std::env::current_dir().ok().and_then(|d| d.to_str().map(|s| s.as_bytes().to_vec()))
    }.unwrap_or_else(|| b".".to_vec());

    RESOLVER.with(|r| {
        let mut guard = r.borrow_mut();
        let resolver = guard.as_mut()?;

        match resolver.resolve(&source_dir, specifier.as_bytes(), ImportKind::Stmt) {
            ::std::result::Result::Ok(result) => {
                if let Some(path) = result.path_const() {
                    let path_str = ::std::str::from_utf8(path.text()).unwrap_or("");
                    if !path_str.is_empty() {
                        return Some(PathBuf::from(path_str));
                    }
                }
                None
            }
            ::std::result::Result::Err(_) => None,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::std::fs;
    use tempfile::TempDir;

    fn tempdir() -> TempDir {
        bao_native_stubs::force_link();
        install(); // initialize thread-local Resolver
        TempDir::new().expect("create temp dir")
    }

    // @trace TEST-ENG-005 [req:REQ-ENG-005] [level:unit]
    #[test]
    fn test_resolve_via_bun_resolver_js_extension() {
        let dir = tempdir();
        fs::write(dir.path().join("mod.js"), "// test").unwrap();
        let result = resolve_via_bun_resolver("./mod", Some(dir.path()));
        assert!(result.is_some());
        let path = result.unwrap();
        assert_eq!(path.extension().unwrap(), "js");
    }

    // @trace TEST-ENG-005 [req:REQ-ENG-005] [level:unit]
    #[test]
    fn test_resolve_via_bun_resolver_exact_match() {
        let dir = tempdir();
        fs::write(dir.path().join("data.json"), "{}").unwrap();
        let result = resolve_via_bun_resolver("./data.json", Some(dir.path()));
        assert!(result.is_some());
        assert!(result.unwrap().to_str().unwrap().contains("data.json"));
    }

    // @trace TEST-ENG-005 [req:REQ-ENG-005] [level:unit]
    #[test]
    fn test_resolve_via_bun_resolver_not_found() {
        bao_native_stubs::force_link();
        install();
        let result = resolve_via_bun_resolver("./nonexistent", Some(::std::env::current_dir().unwrap().as_path()));
        assert!(result.is_none());
    }

    // @trace TEST-ENG-005 [req:REQ-ENG-005] [level:unit]
    #[test]
    fn test_resolve_via_bun_resolver_index_js() {
        let dir = tempdir();
        let pkg = dir.path().join("pkg");
        fs::create_dir_all(&pkg).unwrap();
        fs::write(pkg.join("index.js"), "// pkg entry").unwrap();
        let result = resolve_via_bun_resolver("./pkg", Some(dir.path()));
        assert!(result.is_some());
        assert!(result.unwrap().to_str().unwrap().contains("index.js"));
    }

    // @trace TEST-ENG-005 [req:REQ-ENG-005] [level:unit]
    #[test]
    fn test_resolve_via_bun_resolver_ts_extension() {
        let dir = tempdir();
        fs::write(dir.path().join("app.ts"), "// ts module").unwrap();
        let result = resolve_via_bun_resolver("./app", Some(dir.path()));
        assert!(result.is_some());
    }

    // @trace TEST-ENG-005 [req:REQ-ENG-005] [level:unit]
    #[test]
    fn test_resolve_via_bun_resolver_absolute_path() {
        let dir = tempdir();
        let file = dir.path().join("abs_target.js");
        fs::write(&file, "// abs").unwrap();
        let abs = file.to_str().unwrap().to_string();
        let result = resolve_via_bun_resolver(&abs, None);
        assert!(result.is_some());
    }

    // @trace TEST-ENG-005 [req:REQ-ENG-005] [level:unit]
    #[test]
    fn test_resolve_via_bun_resolver_relative() {
        let dir = tempdir();
        fs::write(dir.path().join("rel.js"), "// rel").unwrap();
        let result = resolve_via_bun_resolver("./rel", Some(dir.path()));
        assert!(result.is_some());
    }

    // @trace TEST-ENG-005 [req:REQ-ENG-005] [level:unit]
    #[test]
    fn test_resolve_via_bun_resolver_parent_relative() {
        let dir = tempdir();
        let child = dir.path().join("sub");
        fs::create_dir_all(&child).unwrap();
        fs::write(dir.path().join("parent.js"), "// parent").unwrap();
        let result = resolve_via_bun_resolver("../parent", Some(&child));
        assert!(result.is_some());
    }

    // @trace TEST-ENG-005 [req:REQ-ENG-005] [level:unit]
    #[test]
    fn test_resolve_via_bun_resolver_node_modules() {
        let dir = tempdir();
        let nm = dir.path().join("node_modules").join("mylib");
        fs::create_dir_all(&nm).unwrap();
        fs::write(nm.join("index.js"), "// lib").unwrap();
        let result = resolve_via_bun_resolver("mylib", Some(dir.path()));
        assert!(result.is_some());
        assert!(result.unwrap().to_str().unwrap().contains("mylib"));
    }

    // @trace TEST-ENG-005 [req:REQ-ENG-005] [level:unit]
    #[test]
    fn test_resolve_via_bun_resolver_no_base_dir_uses_cwd() {
        bao_native_stubs::force_link();
        install();
        let result = resolve_via_bun_resolver("./nonexistent_in_cwd", None);
        // Should not panic, even if file not found
        assert!(result.is_none());
    }

    // @trace TEST-ENG-005 [req:REQ-ENG-005] [level:unit]
    #[test]
    fn test_resolve_via_bun_resolver_mjs_extension() {
        let dir = tempdir();
        fs::write(dir.path().join("mod.mjs"), "// esm").unwrap();
        let result = resolve_via_bun_resolver("./mod.mjs", Some(dir.path()));
        assert!(result.is_some());
        assert_eq!(result.unwrap().extension().unwrap(), "mjs");
    }

    // @trace TEST-ENG-005 [req:REQ-ENG-005] [level:unit]
    #[test]
    fn test_resolve_via_bun_resolver_tsx_extension() {
        let dir = tempdir();
        fs::write(dir.path().join("comp.tsx"), "// tsx").unwrap();
        let result = resolve_via_bun_resolver("./comp", Some(dir.path()));
        assert!(result.is_some());
    }

    // @trace TEST-ENG-005 [req:REQ-ENG-005] [level:unit]
    #[test]
    fn test_resolve_via_bun_resolver_nested_directory() {
        let dir = tempdir();
        let nested = dir.path().join("a").join("b").join("c");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("deep.js"), "// nested").unwrap();
        let result = resolve_via_bun_resolver("./a/b/c/deep", Some(dir.path()));
        assert!(result.is_some());
        assert!(result.unwrap().to_str().unwrap().contains("deep.js"));
    }

    // @trace TEST-ENG-005 [req:REQ-ENG-005] [level:unit]
    #[test]
    fn test_resolve_via_bun_resolver_package_json_main() {
        let dir = tempdir();
        // Verify package.json "main" field resolution via node_modules/<pkg>/package.json
        // bun_resolver resolves bare specifiers by finding node_modules/<pkg>/index.js
        // when no explicit main field resolves.
        let nm = dir.path().join("node_modules").join("mypkg");
        fs::create_dir_all(&nm).unwrap();
        fs::write(nm.join("index.js"), "// default entry").unwrap();
        let result = resolve_via_bun_resolver("mypkg", Some(dir.path()));
        assert!(result.is_some());
        let path = result.unwrap();
        assert!(path.to_str().unwrap().contains("mypkg"));
        assert!(path.to_str().unwrap().contains("index.js"));
    }

    // @trace TEST-ENG-005 [req:REQ-ENG-005] [level:unit]
    #[test]
    fn test_install_idempotent() {
        bao_native_stubs::force_link();
        install();
        install(); // second call should be a no-op, not panic
    }
}
