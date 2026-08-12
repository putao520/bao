// @trace REQ-ENG-006 REQ-CLI-001 [entity:BaoRuntime]
// @trace REQ-CLI-001: bao CLI entry point and runtime initialization
use bao_engine::context::{JsContext, SmRuntimeGuard};
use bao_engine::error::JsError;
use bao_engine::module_loader::ModuleLoader;
use bao_engine::value::JsValue;

use crate::globals;
use crate::require;
use crate::timers;

pub struct BaoRuntime {
    ctx: JsContext,
    // Declared after ctx so it drops last: guard drop triggers
    // JS_DestroyContext + JS_ShutDown after all JS execution is done.
    _guard: Option<SmRuntimeGuard>,
}

impl BaoRuntime {
    pub fn new() -> ::std::result::Result<Self, JsError> {
        Self::init_env_aliases();
        // @trace REQ-H3-001: 默认启用 h3/HTTP3 fetch 能力（BAO 是正常 BUN）。
        // 在 HTTP 线程启动前设置，确保 bun_http::h3_alt_svc_enabled() 返回 true，
        // fetch() 默认支持 Alt-Svc 协商 + force_http3 显式协议选项。
        crate::h3_fetch::enable_h3_by_default();
        // Initialize bun_core output subsystem before any background thread
        // (e.g. fetch() worker) calls configure_thread() and hits the
        // STDOUT_STREAM_SET debug_assert.
        bun_core::output::init_test();
        crate::resolver_bridge::install();
        crate::bun_api::init_process_start();
        let (mut ctx, guard) = JsContext::init_runtime()?;
        ctx.set_global_setup(globals::install_all);
        ctx.set_post_eval_hook(timers::drain_and_check);
        ::std::result::Result::Ok(BaoRuntime { ctx, _guard: guard })
    }

    fn init_env_aliases() {
        for (key, value) in ::std::env::vars() {
            if let Some(suffix) = key.strip_prefix("BAO_") {
                let bun_key = format!("BUN_{}", suffix);
                if ::std::env::var(&bun_key).is_err() {
                    unsafe {
                        ::std::env::set_var(&bun_key, &value);
                    }
                }
            }
        }
    }

    pub fn eval(
        &mut self,
        source: &str,
        filename: &str,
    ) -> ::std::result::Result<JsValue, JsError> {
        self.ctx.eval(source, filename)
    }

    pub fn eval_module(
        &mut self,
        source: &str,
        filename: &str,
    ) -> ::std::result::Result<JsValue, JsError> {
        let setup = self.ctx.global_setup();
        let hook = self.ctx.post_eval_hook();
        let mut cx = self.ctx.cx();
        ModuleLoader::eval_module(&mut cx, source, filename, setup, hook)
    }

    pub fn run_file(&mut self, path: &str) -> ::std::result::Result<JsValue, JsError> {
        let source = bun_sys::fs::read_to_string(path).map_err(|e| JsError {
            message: format!("Error reading {}: {}", path, e),
            filename: path.into(),
            line: 0,
            column: 0,
            stack: None,
        })?;

        let abs_path = if ::std::path::Path::new(path).is_absolute() {
            ::std::path::PathBuf::from(path)
        } else {
            ::std::env::current_dir().unwrap_or_default().join(path)
        };
        if let Some(dir) = abs_path.parent() {
            require::set_require_dir(dir.to_path_buf());
        }

        let filename_str = abs_path.to_string_lossy().into_owned();
        let dirname_str = abs_path
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        globals::install_file_globals(&mut self.ctx, &filename_str, &dirname_str);

        if path.ends_with(".mjs") || path.ends_with(".mts") {
            self.eval_module(&source, path)
        } else if path.ends_with(".ts") || path.ends_with(".tsx") || path.ends_with(".jsx") {
            // TypeScript/JSX files: treat as ESM if they contain import/export
            if source.contains("import ") || source.contains("export ") {
                self.eval_module(&source, path)
            } else {
                self.eval(&source, path)
            }
        } else if source.contains("import ")
            && (source.contains(" from ")
                || source.contains(" from\"")
                || source.contains("from '"))
            && !source.contains("require(")
        {
            // JS files with ESM imports (and no require): treat as ESM
            self.eval_module(&source, path)
        } else if source.trim_start().starts_with("import ") {
            self.eval_module(&source, path)
        } else {
            self.eval(&source, path)
        }
    }

    /// Load and execute a test file, then run the registered `bun:test`
    /// suites while the realm that registered them is still alive. Returns
    /// a full report (counters + named passes/failures).
    ///
    /// Files that don't look like modules fall back to plain `eval`, but in
    /// that case `bun:test` registration happens in a different realm and the
    /// report will be empty — test files should use ESM/TS syntax.
    //
    // @trace REQ-ENG-006 [entity:BaoRuntime] — bao test runner execution
    pub fn run_test_file(
        &mut self,
        path: &str,
    ) -> ::std::result::Result<crate::bun_test::TestReport, JsError> {
        let source = bun_sys::fs::read_to_string(path).map_err(|e| JsError {
            message: format!("Error reading {}: {}", path, e),
            filename: path.into(),
            line: 0,
            column: 0,
            stack: None,
        })?;

        let abs_path = if ::std::path::Path::new(path).is_absolute() {
            ::std::path::PathBuf::from(path)
        } else {
            ::std::env::current_dir().unwrap_or_default().join(path)
        };
        if let Some(dir) = abs_path.parent() {
            require::set_require_dir(dir.to_path_buf());
        }

        let filename_str = abs_path.to_string_lossy().into_owned();
        let dirname_str = abs_path
            .parent()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        globals::install_file_globals(&mut self.ctx, &filename_str, &dirname_str);

        let is_module = path.ends_with(".mjs")
            || path.ends_with(".mts")
            || path.ends_with(".ts")
            || path.ends_with(".tsx")
            || path.ends_with(".jsx")
            || (source.contains("import ")
                && (source.contains(" from ")
                    || source.contains(" from\"")
                    || source.contains("from '"))
                && !source.contains("require("))
            || source.trim_start().starts_with("import ");

        if is_module {
            let setup = self.ctx.global_setup();
            let hook = self.ctx.post_eval_hook();
            let mut cx = self.ctx.cx();
            ModuleLoader::eval_module_then(&mut cx, &source, path, setup, hook, |realm_cx| unsafe {
                crate::bun_test::run_bun_tests_report(realm_cx.raw_cx())
            })
        } else {
            // Non-module file: bun:test registration won't survive eval's separate realm.
            // Run as plain script and produce an empty report (no bun:test suites collected).
            self.eval(&source, path)?;
            ::std::result::Result::Ok(crate::bun_test::TestReport::default())
        }
    }
}
