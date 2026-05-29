use bao_engine::context::JsContext;
use bao_engine::error::JsError;
use bao_engine::module_loader::ModuleLoader;
use bao_engine::value::JsValue;

use crate::globals;

pub struct BaoRuntime {
    ctx: JsContext,
}

impl BaoRuntime {
    pub fn new() -> ::std::result::Result<Self, JsError> {
        let ctx = JsContext::new()?;
        ::std::result::Result::Ok(BaoRuntime { ctx })
    }

    pub fn eval(&mut self, source: &str, filename: &str) -> ::std::result::Result<JsValue, JsError> {
        self.ctx.eval(source, filename)
    }

    pub fn eval_module(&mut self, source: &str, filename: &str) -> ::std::result::Result<JsValue, JsError> {
        ModuleLoader::eval_module(self.ctx.cx_mut(), source, filename)
    }

    pub fn run_file(&mut self, path: &str) -> ::std::result::Result<JsValue, JsError> {
        let source = ::std::fs::read_to_string(path).map_err(|e| JsError {
            message: format!("Error reading {}: {}", path, e),
            filename: path.into(),
            line: 0,
            column: 0,
            stack: None,
        })?;

        if path.ends_with(".mjs") {
            self.eval_module(&source, path)
        } else {
            self.eval(&source, path)
        }
    }
}
