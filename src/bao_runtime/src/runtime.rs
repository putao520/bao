// @trace REQ-ENG-006 REQ-CLI-001 [entity:BaoRuntime]
// @trace REQ-CLI-001: bao CLI entry point and runtime initialization
use bao_engine::context::{JsContext, SmRuntimeGuard};
use bao_engine::error::JsError;
use bao_engine::module_loader::ModuleLoader;
use bao_engine::value::JsValue;
use mozjs::realm::AutoRealm;
use mozjs::rooted;

use crate::globals;
use crate::require;

pub struct BaoRuntime {
    ctx: JsContext,
    // Declared after ctx so it drops last: guard drop triggers
    // JS_DestroyContext + JS_ShutDown after all JS execution is done.
    _guard: Option<SmRuntimeGuard>,
}

impl BaoRuntime {
    pub fn new() -> ::std::result::Result<Self, JsError> {
        // BAO_* → BUN_* env aliasing is resolved at the env read layer
        // (`bun_core::getenv_z` / `getenv_z_any_case`): a `BUN_<SUFFIX>` lookup
        // that misses falls back to `BAO_<SUFFIX>` (explicit BUN_ wins). The
        // constructor no longer copies BAO_* into the host process env via
        // `std::env::set_var` — a library constructor must not irreversibly
        // mutate the host environment (issue #32 / B0 census row 16;
        // the retired `init_env_aliases` lived here).
        // @trace REQ-CLI-001 — alias contract preserved, moved to read time
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
        // Drain the event loop first; once it is done (natural end or
        // process.exit()), dispatch process 'exit' listeners inside the live
        // realm. Node semantics: registration order, exit code argument,
        // exitCode set by a listener is respected by the CLI main loop.
        ctx.set_post_eval_hook(crate::bun_api::post_eval_drain_then_exit);
        ::std::result::Result::Ok(BaoRuntime { ctx, _guard: guard })
    }

    pub fn eval(
        &mut self,
        source: &str,
        filename: &str,
    ) -> ::std::result::Result<JsValue, JsError> {
        self.ctx.eval(source, filename)
    }

    /// Ensure this context's persistent realm exists (realm-per-context,
    /// ECMA-262/Node semantics: one realm per agent for its whole lifetime).
    ///
    /// Delegates to `JsContext::ensure_realm_global` (idempotent — first call
    /// lazily creates the global, applies `global_setup` exactly once,
    /// publishes `thread_realm_global` for async dispatch; later calls return
    /// the stored global). No eval runs here, so no post-eval hook / exit
    /// dispatch — safe to call before any user code.
    fn ensure_realm(
        &mut self,
    ) -> ::std::result::Result<*mut mozjs::jsapi::JSObject, JsError> {
        let setup = self.ctx.global_setup();
        let mut cx = self.ctx.cx();
        self.ctx.ensure_realm_global(&mut cx, setup)
    }

    pub fn eval_module(
        &mut self,
        source: &str,
        filename: &str,
    ) -> ::std::result::Result<JsValue, JsError> {
        let hook = self.ctx.post_eval_hook();
        let mut cx = self.ctx.cx();
        // Realm-per-context: evaluate the module in this context's single
        // persistent realm — the same realm every script `eval` uses (Node
        // semantics: `globalThis` and `require` singletons are shared across
        // script and module evals). No new global, no global_setup re-run.
        let global_ptr = self.ensure_realm()?;
        rooted!(&in(cx) let global = global_ptr);
        ModuleLoader::eval_module_in_realm(&mut cx, source, filename, hook, global.handle())
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
    /// Files that don't look like modules fall back to plain `eval` in the
    /// same persistent realm; a plain script cannot register bun:test suites
    /// via the module loader, so the report is empty — test files should use
    /// ESM/TS syntax.
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
            let hook = self.ctx.post_eval_hook();
            let mut cx = self.ctx.cx();
            // Realm-per-context: run the test module in the same persistent
            // realm as every script eval on this context.
            let global_ptr = self.ensure_realm()?;
            rooted!(&in(cx) let global = global_ptr);
            ModuleLoader::eval_module_in_realm_then(
                &mut cx,
                &source,
                path,
                hook,
                global.handle(),
                |realm_cx| unsafe { crate::bun_test::run_bun_tests_report(realm_cx.raw_cx()) },
            )
        } else {
            // Non-module (CJS/plain-script) file: run as a script in the
            // persistent realm, then drive the registered suites exactly like
            // the module branch above — require('node:test') in a CJS file
            // registers into the same bun:test collector, and registered but
            // never-executed suites are the silent fake pass this runner
            // exists to prevent.
            self.eval(&source, path)?;
            ::std::result::Result::Ok(self.run_registered_tests())
        }
    }

    /// Drive the bun:test suites registered in this context's persistent
    /// realm and return the report. `bao test` calls this after evaluating
    /// each test file — module files via `eval_module_in_realm_then`'s
    /// callback (which runs inside `AutoRealm`), plain-script files and
    /// `bao test -e` evals directly here — so every entry path executes what
    /// it registered instead of reporting a vacuous 0/0.
    //
    // @trace REQ-ENG-006 [entity:BaoRuntime] — bao test runner execution
    pub fn run_registered_tests(&mut self) -> crate::bun_test::TestReport {
        let global_ptr = match self.ensure_realm() {
            Ok(g) => g,
            Err(_) => return crate::bun_test::TestReport::default(),
        };
        let mut cx = self.ctx.cx();
        rooted!(&in(cx) let global = global_ptr);
        // Enter the persistent realm: the runner's shims evaluate against the
        // current realm's global (same contract as the module branch's
        // after_eval callback).
        let mut realm = AutoRealm::new_from_handle(&mut cx, global.handle());
        let realm_cx: &mut mozjs::context::JSContext = &mut realm;
        unsafe { crate::bun_test::run_bun_tests_report(realm_cx.raw_cx()) }
    }
}
