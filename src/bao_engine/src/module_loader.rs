use ::std::cell::RefCell;
use ::std::collections::HashMap;
use ::std::ffi::CString;
use ::std::fs;
use ::std::path::{Path, PathBuf};
use ::std::ptr::NonNull;

use mozjs::glue::NewCompileOptions;
use mozjs::jsapi::*;
use mozjs::jsval::UndefinedValue;
use mozjs::realm::AutoRealm;
use mozjs::rooted;
use mozjs::rust::wrappers2::{CompileModule1, ModuleEvaluate, ModuleLink};
use mozjs::rust::{
    transform_str_to_source_text, CompileOptionsWrapper, RealmOptions, Runtime,
    SIMPLE_GLOBAL_CLASS,
};

use crate::context::{GlobalSetupFn, PostEvalHook};
use crate::error::JsError;
use crate::job_queue::JobQueue;
use crate::value::{JsValue, jsval_to_jsvalue};

thread_local! {
    static MODULE_CACHE: RefCell<HashMap<::std::string::String, *mut JSObject>> = RefCell::new(HashMap::new());
}

pub struct ModuleLoader;

impl ModuleLoader {
    pub fn init(runtime: &Runtime) {
        let rt = runtime.rt();
        unsafe {
            SetModuleResolveHook(rt, Some(host_resolve_imported_module));
            SetModuleMetadataHook(rt, Some(host_populate_import_meta));
        }
    }

    pub fn eval_module(
        cx: &mut mozjs::context::JSContext,
        source: &str,
        filename: &str,
        global_setup: Option<GlobalSetupFn>,
        post_eval_hook: Option<PostEvalHook>,
    ) -> ::std::result::Result<JsValue, JsError> {
        let options = RealmOptions::default();

        rooted!(&in(cx) let global = unsafe {
            mozjs::rust::wrappers2::JS_NewGlobalObject(
                cx,
                &SIMPLE_GLOBAL_CLASS,
                ::std::ptr::null_mut(),
                OnNewGlobalHookOption::FireOnNewGlobalHook,
                &*options,
            )
        });

        let mut realm = AutoRealm::new_from_handle(cx, global.handle());
        let realm_cx: &mut mozjs::context::JSContext = &mut realm;

        crate::host_fn::install_console(realm_cx, global.handle());
        if let Some(setup) = global_setup {
            unsafe { setup(realm_cx, global.handle()) };
        }

        let c_filename = CString::new(filename)
            .unwrap_or_else(|_| CString::new("<module>").unwrap());
        let compile_opts = CompileOptionsWrapper::new(realm_cx, c_filename, 1);

        let mut src = transform_str_to_source_text(source);

        rooted!(&in(realm_cx) let mut module_obj = unsafe {
            CompileModule1(realm_cx, compile_opts.ptr, &mut src)
        });

        if module_obj.get().is_null() {
            return ::std::result::Result::Err(JsError {
                message: "Failed to compile module".into(),
                filename: filename.into(),
                line: 0,
                column: 0,
                stack: None,
            });
        }

        rooted!(&in(realm_cx) let mut rval = UndefinedValue());

        if !unsafe { ModuleLink(realm_cx, module_obj.handle()) } {
            return ::std::result::Result::Err(extract_module_error(realm_cx));
        }

        if !unsafe { ModuleEvaluate(realm_cx, module_obj.handle(), rval.handle_mut()) } {
            return ::std::result::Result::Err(extract_module_error(realm_cx));
        }

        JobQueue::drain(realm_cx);

        if let Some(hook) = post_eval_hook {
            for _ in 0..1000 {
                if !hook(realm_cx) {
                    break;
                }
                ::std::thread::sleep(::std::time::Duration::from_millis(1));
                hook(realm_cx);
                JobQueue::drain(realm_cx);
            }
        }

        ::std::result::Result::Ok(unsafe {
            jsval_to_jsvalue(realm_cx.raw_cx_no_gc(), rval.get())
        })
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn host_resolve_imported_module(
    raw_cx: *mut JSContext,
    _referencing_private: Handle<Value>,
    module_request: Handle<*mut JSObject>,
) -> *mut JSObject {
    let specifier = unsafe { GetModuleRequestSpecifier(raw_cx, module_request) };
    if specifier.is_null() {
        return ::std::ptr::null_mut();
    }

    let specifier_str = mozjs::conversions::jsstr_to_string(
        raw_cx,
        NonNull::new(specifier).unwrap(),
    );

    let resolved = resolve_specifier(&specifier_str);
    let ::std::option::Option::Some(path) = resolved else {
        return ::std::ptr::null_mut();
    };

    let content = match fs::read_to_string(&path) {
        ::std::result::Result::Ok(c) => c,
        ::std::result::Result::Err(_) => return ::std::ptr::null_mut(),
    };

    unsafe {
        let c_filename = CString::new(path.to_string_lossy().into_owned())
            .unwrap_or_else(|_| CString::new("<module>").unwrap());
        let opts = NewCompileOptions(raw_cx, c_filename.as_ptr(), 1);
        if opts.is_null() {
            return ::std::ptr::null_mut();
        }
        let mut src = transform_str_to_source_text(&content);
        let module = mozjs_sys::jsapi::JS::CompileModule1(raw_cx, opts, &mut src);
        libc::free(opts as *mut _);
        module
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn host_populate_import_meta(
    raw_cx: *mut JSContext,
    _private_value: Handle<Value>,
    meta_object: Handle<*mut JSObject>,
) -> bool {
    unsafe {
        let url_str = JS_NewStringCopyZ(
            raw_cx,
            b"file://\0".as_ptr() as *const ::std::os::raw::c_char,
        );
        if url_str.is_null() {
            return false;
        }

        let val = mozjs::jsval::StringValue(&*url_str);
        let mut handle_val = Handle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &val,
        };

        JS_DefineProperty(raw_cx, meta_object, c"url".as_ptr(), handle_val, JSPROP_ENUMERATE as u32)
    }
}

fn resolve_specifier(specifier: &str) -> ::std::option::Option<PathBuf> {
    let path = Path::new(specifier);
    if path.is_absolute() && path.exists() {
        return ::std::option::Option::Some(path.to_path_buf());
    }

    for ext in ["", ".js", ".mjs"] {
        let candidate = PathBuf::from(format!("{}{}", specifier, ext));
        if candidate.exists() {
            return ::std::option::Option::Some(candidate);
        }
    }

    ::std::option::Option::None
}

fn extract_module_error(cx: &mut mozjs::context::JSContext) -> JsError {
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
            message: "Unknown module error".into(),
            filename: "<module>".into(),
            line: 0,
            column: 0,
            stack: None,
        }
    }
}
