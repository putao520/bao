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
        // D52: bun_sys::environ instead of std::env::vars
        for entry in bun_sys::environ() {
            // D58: ZStr::from_c_ptr instead of CStr::from_ptr
            let entry_bytes = unsafe { bun_core::ZStr::from_c_ptr(*entry) }.as_bytes();
            let Some(pos) = entry_bytes.iter().position(|b| *b == b'=') else { continue };
            let key = match ::std::str::from_utf8(&entry_bytes[..pos]) { Ok(k) => k, Err(_) => continue };
            let value = ::std::str::from_utf8(&entry_bytes[pos+1..]).unwrap_or("");
            if let Some(suffix) = key.strip_prefix("BAO_") {
                let bun_key = format!("BUN_{}", suffix);
                // D78: bun_core::getenv_z + setenv_z instead of std::env::var + set_var
                let key_z = bun_core::ZBox::from_bytes(bun_key.as_bytes());
                if bun_core::getenv_z(&key_z).is_none() {
                    let val_z = bun_core::ZBox::from_bytes(value.as_bytes());
                    let _ = bun_core::setenv_z(&key_z, &val_z, false);
                }
            }
        }
    }

    pub fn eval(&mut self, source: &str, filename: &str) -> ::std::result::Result<JsValue, JsError> {
        self.ctx.eval(source, filename)
    }

    pub fn eval_module(&mut self, source: &str, filename: &str) -> ::std::result::Result<JsValue, JsError> {
        let setup = self.ctx.global_setup();
        let hook = self.ctx.post_eval_hook();
        let mut cx = self.ctx.cx();
        ModuleLoader::eval_module(&mut cx, source, filename, setup, hook)
    }

    pub fn run_file(&mut self, path: &str) -> ::std::result::Result<JsValue, JsError> {
        let bytes = bun_sys::File::read_from(bun_sys::Fd::cwd(), path.as_bytes())
            .map_err(|e| JsError {
                message: format!("Error reading {}: {}", path, e),
                filename: path.into(),
                line: 0,
                column: 0,
                stack: None,
            })?;
        let source = String::from_utf8_lossy(&bytes).into_owned();

        let abs_path = if ::std::path::Path::new(path).is_absolute() {
            ::std::path::PathBuf::from(path)
        } else {
                    // D50: bun_sys::getcwd_alloc instead of std::env::current_dir
            let cwd = bun_sys::getcwd_alloc()
                .ok()
                .map(|zb| ::std::path::PathBuf::from(::std::str::from_utf8(zb.as_bytes()).unwrap_or(".")))
                .unwrap_or_default();
            cwd.join(path)
        };
        if let Some(dir) = abs_path.parent() {
            require::set_require_dir(dir.to_path_buf());
        }

        let filename_str = abs_path.to_string_lossy().into_owned();
        let dirname_str = abs_path.parent()
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
        } else if source.contains("import ") && (source.contains(" from ") || source.contains(" from\"") || source.contains("from '")) && !source.contains("require(") {
            // JS files with ESM imports (and no require): treat as ESM
            self.eval_module(&source, path)
        } else if source.trim_start().starts_with("import ") {
            self.eval_module(&source, path)
        } else {
            self.eval(&source, path)
        }
    }
}