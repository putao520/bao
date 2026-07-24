//! Hard-green runtime proof using raw SpiderMonkey (no bao_engine dual-def chain).
//!
//! Drives shipped `install_workflow_host_on_global` + host callbacks on a real
//! JSContext — not a reimplementation of the host. Failures must fail the test
//! (no soft-skip / eprintln-and-pass).

use std::mem::ManuallyDrop;
use std::ptr;
use std::sync::{Arc, Mutex};

use bao_workflow_host::{
    WorkflowHostCallbacks, install_workflow_host_on_bun, install_workflow_host_on_global,
    js_to_rust_string, set_workflow_host_callbacks, take_workflow_host_callbacks,
};
use mozjs::jsapi::{JSObject, OnNewGlobalHookOption};
use mozjs::jsval::UndefinedValue;
use mozjs::rust::wrappers2::JS_NewGlobalObject;
use mozjs::rust::{CompileOptionsWrapper, JSEngine, RealmOptions, Runtime, SIMPLE_GLOBAL_CLASS};
use mozjs::realm::AutoRealm;
use mozjs::rooted;

struct TestCb {
    phases: Arc<Mutex<Vec<String>>>,
    logs: Arc<Mutex<Vec<String>>>,
}

impl WorkflowHostCallbacks for TestCb {
    fn phase(&mut self, title: &str) {
        self.phases.lock().unwrap().push(title.to_owned());
    }
    fn log(&mut self, message: &str) {
        self.logs.lock().unwrap().push(message.to_owned());
    }
    fn agent(&mut self, prompt: &str, _opts_json: &str) -> Result<String, String> {
        Ok(format!("\"echo:{prompt}\""))
    }
    fn args_json(&self) -> String {
        r#"{"k":1}"#.into()
    }
    fn budget_json(&self) -> String {
        "null".into()
    }
}

#[test]
fn bao_workflow_host_phase_log_agent_hard_green() {
    let phases = Arc::new(Mutex::new(Vec::new()));
    let logs = Arc::new(Mutex::new(Vec::new()));
    set_workflow_host_callbacks(Box::new(TestCb {
        phases: Arc::clone(&phases),
        logs: Arc::clone(&logs),
    }));

    // JSEngine is process-singleton semantics in SpiderMonkey; do not drop it
    // (same discipline as bao_engine NeverDrop).
    let engine = ManuallyDrop::new(JSEngine::init().expect("JSEngine::init"));
    let mut rt = ManuallyDrop::new(Runtime::new(engine.handle()));
    let cx = rt.cx();
    let options = RealmOptions::default();

    unsafe {
        eprintln!("[hard-green] step: NewGlobalObject");
        rooted!(&in(cx) let global = JS_NewGlobalObject(
            cx,
            &SIMPLE_GLOBAL_CLASS,
            ptr::null_mut(),
            OnNewGlobalHookOption::FireOnNewGlobalHook,
            &*options,
        ));
        assert!(!global.get().is_null(), "JS_NewGlobalObject must succeed");

        eprintln!("[hard-green] step: AutoRealm + install");
        let mut realm = AutoRealm::new_from_handle(cx, global.handle());
        let realm_cx: &mut mozjs::context::JSContext = &mut realm;

        install_workflow_host_on_global(realm_cx, global.handle());
        eprintln!("[hard-green] step: install done");

        // Call natives directly first (no shim dependence for core bridge proof).
        let direct = r#"
            globalThis.__wf_phase('Research');
            globalThis.__wf_log('start');
            globalThis.__out = globalThis.__wf_agent('hello', '{}');
            String(globalThis.__out);
        "#;
        rooted!(&in(realm_cx) let mut rval = UndefinedValue());
        let c_filename = std::ffi::CString::new("<wf-direct>").unwrap();
        let opts = CompileOptionsWrapper::new(realm_cx, c_filename, 1);
        eprintln!("[hard-green] step: evaluate direct natives");
        mozjs::rust::evaluate_script(
            realm_cx,
            global.handle(),
            direct,
            rval.handle_mut(),
            opts,
        )
        .expect("direct native eval must succeed (hard green)");

        let out = js_to_rust_string(realm_cx.raw_cx(), rval.get());
        eprintln!("[hard-green] step: out={out}");

        // Shim surface: phase/log/args as CC host would see them
        let shim_src = r#"
            phase('ShimPhase');
            log('shim-log');
            JSON.stringify(args);
        "#;
        rooted!(&in(realm_cx) let mut rval2 = UndefinedValue());
        let c_filename2 = std::ffi::CString::new("<wf-shim>").unwrap();
        let opts2 = CompileOptionsWrapper::new(realm_cx, c_filename2, 1);
        eprintln!("[hard-green] step: evaluate shim surface");
        mozjs::rust::evaluate_script(
            realm_cx,
            global.handle(),
            shim_src,
            rval2.handle_mut(),
            opts2,
        )
        .expect("shim surface eval must succeed (hard green)");
        let args_out = js_to_rust_string(realm_cx.raw_cx(), rval2.get());
        eprintln!("[hard-green] step: args_out={args_out}");

        // Assert before teardown (RunJobs without job-queue init can SIGSEGV).
        let ph = phases.lock().unwrap().clone();
        let lg = logs.lock().unwrap().clone();
        assert!(
            ph.iter().any(|p| p == "Research"),
            "phase native must hit callbacks: phases={ph:?} out={out}"
        );
        assert!(
            ph.iter().any(|p| p == "ShimPhase"),
            "phase() shim must hit callbacks: phases={ph:?}"
        );
        assert!(
            lg.iter().any(|l| l == "start"),
            "log native must hit callbacks: logs={lg:?}"
        );
        assert!(
            lg.iter().any(|l| l == "shim-log"),
            "log() shim must hit callbacks: logs={lg:?}"
        );
        assert!(
            out.contains("hello") || out.contains("echo"),
            "agent() must return bridge result: out={out}"
        );
        assert!(
            args_out.contains("\"k\"") || args_out.contains('1'),
            "args must be bound from callbacks: args_out={args_out}"
        );
        eprintln!("[hard-green] PASS asserts");
        let _ = take_workflow_host_callbacks();
        eprintln!("[hard-green] PASS teardown callbacks");
        // Leave AutoRealm / roots / Runtime alive until process exit via ManuallyDrop.
        // Explicit drop order of SM realms has been observed to SIGSEGV without
        // full bao_engine job-queue init; hard-green proof is the asserts above.
        std::mem::forget(realm);
        eprintln!("[hard-green] PASS");
    }
}

#[test]
fn install_workflow_host_symbols_exist() {
    let _: unsafe fn(
        &mut mozjs::context::JSContext,
        mozjs::rust::Handle<*mut JSObject>,
    ) = install_workflow_host_on_global;
    let _: unsafe fn(
        &mut mozjs::context::JSContext,
        mozjs::rust::Handle<*mut JSObject>,
    ) = install_workflow_host_on_bun;
}
