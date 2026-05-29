use ::std::ptr;

use mozjs::jsapi::OnNewGlobalHookOption;
use mozjs::jsval::UndefinedValue;
use mozjs::realm::AutoRealm;
use mozjs::rooted;
use mozjs::rust::wrappers2::JS_NewGlobalObject;
use mozjs::rust::{JSEngine, RealmOptions, Runtime, SIMPLE_GLOBAL_CLASS};

use crate::error::JsError;
use crate::host_fn;
use crate::job_queue::JobQueue;
use crate::module_loader::ModuleLoader;
use crate::value::{JsValue, jsval_to_jsvalue};

pub struct JsContext {
    runtime: Runtime,
    _engine: JSEngine,
}

impl JsContext {
    pub fn new() -> ::std::result::Result<Self, JsError> {
        let engine = JSEngine::init().map_err(|_| JsError {
            message: "Failed to initialize SpiderMonkey engine".into(),
            filename: "<engine>".into(),
            line: 0,
            column: 0,
            stack: None,
        })?;

        let mut runtime = Runtime::new(engine.handle());

        {
            let cx = runtime.cx();
            if !JobQueue::init(cx) {
                return ::std::result::Result::Err(JsError {
                    message: "Failed to initialize internal job queue".into(),
                    filename: "<engine>".into(),
                    line: 0,
                    column: 0,
                    stack: None,
                });
            }
        }

        ModuleLoader::init(&runtime);

        ::std::result::Result::Ok(JsContext { runtime, _engine: engine })
    }

    pub fn cx_mut(&mut self) -> &mut mozjs::context::JSContext {
        self.runtime.cx()
    }

    pub fn eval(&mut self, source: &str, filename: &str) -> ::std::result::Result<JsValue, JsError> {
        let cx = self.runtime.cx();
        let options = RealmOptions::default();

        rooted!(&in(cx) let global = unsafe {
            JS_NewGlobalObject(cx, &SIMPLE_GLOBAL_CLASS, ptr::null_mut(),
                               OnNewGlobalHookOption::FireOnNewGlobalHook,
                               &*options)
        });

        {
            let mut realm = AutoRealm::new_from_handle(cx, global.handle());
            let realm_cx: &mut mozjs::context::JSContext = &mut realm;
            host_fn::install_console(realm_cx, global.handle());
        }

        let c_filename = ::std::ffi::CString::new(filename)
            .unwrap_or_else(|_| ::std::ffi::CString::new("<eval>").unwrap());
        let compile_opts = mozjs::rust::CompileOptionsWrapper::new(cx, c_filename, 1);

        rooted!(&in(cx) let mut rval = UndefinedValue());

        let result = mozjs::rust::evaluate_script(
            cx,
            global.handle(),
            source,
            rval.handle_mut(),
            compile_opts,
        );

        if result.is_err() {
            let mut realm = AutoRealm::new_from_handle(cx, global.handle());
            let realm_cx: &mut mozjs::context::JSContext = &mut realm;
            return ::std::result::Result::Err(extract_exception(realm_cx));
        }

        JobQueue::drain(cx);

        ::std::result::Result::Ok(unsafe { jsval_to_jsvalue(cx.raw_cx_no_gc(), rval.get()) })
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
fn extract_exception(cx: &mut mozjs::context::JSContext) -> JsError {
    rooted!(&in(cx) let mut exn = UndefinedValue());
    if let ::std::option::Option::Some(info) = unsafe {
        mozjs::rust::error_info_from_exception_stack(cx.raw_cx_no_gc(), exn.handle_mut().into())
    } {
        JsError {
            message: info.message,
            filename: info.filename,
            line: info.line,
            column: info.col,
            stack: None,
        }
    } else {
        JsError {
            message: "Unknown JS error".into(),
            filename: "<unknown>".into(),
            line: 0,
            column: 0,
            stack: None,
        }
    }
}
