// REQ-CLI-001: bao CLI entry point and runtime initialization
use bao_engine::context::JsContext;
use bao_engine::error::JsError;
use bao_engine::module_loader::ModuleLoader;
use bao_engine::value::JsValue;

use crate::globals;
use crate::require;
use crate::timers;

pub struct BaoRuntime {
    ctx: JsContext,
}

impl BaoRuntime {
    pub fn new() -> ::std::result::Result<Self, JsError> {
        Self::init_env_aliases();
        crate::bun_api::init_process_start();
        let mut ctx = JsContext::new()?;
        ctx.set_global_setup(globals::install_all);
        ctx.set_post_eval_hook(timers::drain_and_check);
        ::std::result::Result::Ok(BaoRuntime { ctx })
    }

    fn init_env_aliases() {
        for (key, value) in ::std::env::vars() {
            if let Some(suffix) = key.strip_prefix("BAO_") {
                let bun_key = format!("BUN_{}", suffix);
                if ::std::env::var(&bun_key).is_err() {
                    unsafe { ::std::env::set_var(&bun_key, &value); }
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
        ModuleLoader::eval_module(self.ctx.cx_mut(), source, filename, setup, hook)
    }

    pub fn run_file(&mut self, path: &str) -> ::std::result::Result<JsValue, JsError> {
        let source = ::std::fs::read_to_string(path).map_err(|e| JsError {
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

        if path.ends_with(".mjs") {
            self.eval_module(&source, path)
        } else {
            self.eval(&source, path)
        }
    }
}
