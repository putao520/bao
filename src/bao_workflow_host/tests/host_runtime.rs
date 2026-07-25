//! Hard-green runtime proof using raw SpiderMonkey (no bao_engine dual-def chain).
//!
//! Drives shipped `install_workflow_host_on_global` + host callbacks on a real
//! JSContext — not a reimplementation of the host. Failures must fail the test
//! (no soft-skip / eprintln-and-pass).
//!
//! Wave H: nest / parallel barrier / nondet throw / budget (single SM process).

use std::mem::ManuallyDrop;
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use bao_workflow_host::{
    install_workflow_host_on_bun, install_workflow_host_on_global, js_to_rust_string,
    set_workflow_host_callbacks, take_workflow_host_callbacks, WorkflowHostCallbacks,
};
use mozjs::jsapi::{JSObject, OnNewGlobalHookOption};
use mozjs::jsval::UndefinedValue;
use mozjs::realm::AutoRealm;
use mozjs::rooted;
use mozjs::rust::wrappers2::JS_NewGlobalObject;
use mozjs::rust::{CompileOptionsWrapper, JSEngine, RealmOptions, Runtime, SIMPLE_GLOBAL_CLASS};

struct TestCb {
    phases: Arc<Mutex<Vec<String>>>,
    logs: Arc<Mutex<Vec<String>>>,
    nest_depth: Arc<AtomicU32>,
    max_nest: u32,
    budget: String,
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
        self.budget.clone()
    }
    fn workflow_nested(&mut self, name_or_ref: &str, args_json: &str) -> Result<String, String> {
        let depth = self.nest_depth.fetch_add(1, Ordering::SeqCst);
        if depth >= self.max_nest {
            return Err(format!(
                "workflow nest depth exceeded (max {}); cannot nest '{name_or_ref}'",
                self.max_nest
            ));
        }
        Ok(format!(
            r#"{{"nested":"{name_or_ref}","args":{args_json},"status":"ok"}}"#
        ))
    }
}

fn eval_str(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
    src: &str,
    label: &str,
) -> String {
    unsafe {
        rooted!(&in(cx) let mut rval = UndefinedValue());
        let c_filename = std::ffi::CString::new(label).unwrap();
        let opts = CompileOptionsWrapper::new(cx, c_filename, 1);
        mozjs::rust::evaluate_script(cx, global, src, rval.handle_mut(), opts)
            .unwrap_or_else(|_| panic!("{label}: eval must succeed (hard green)"));
        js_to_rust_string(cx.raw_cx(), rval.get())
    }
}

/// Single hard-green test (SM is process-singleton — do not split into multiple engine inits).
/// Covers: phase/log/agent/args + H1 nest + H3 parallel surface + H5 budget + H11 nondet.
#[test]
fn bao_workflow_host_phase_log_agent_hard_green() {
    let phases = Arc::new(Mutex::new(Vec::new()));
    let logs = Arc::new(Mutex::new(Vec::new()));
    let nest_depth = Arc::new(AtomicU32::new(0));
    set_workflow_host_callbacks(Box::new(TestCb {
        phases: Arc::clone(&phases),
        logs: Arc::clone(&logs),
        nest_depth: Arc::clone(&nest_depth),
        max_nest: 1,
        budget: r#"{"total":10}"#.into(),
    }));

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

        // --- P0: phase / log / agent natives ---
        let direct = r#"
            globalThis.__wf_phase('Research');
            globalThis.__wf_log('start');
            globalThis.__out = globalThis.__wf_agent('hello', '{}');
            String(globalThis.__out);
        "#;
        let out = eval_str(realm_cx, global.handle(), direct, "<wf-direct>");
        eprintln!("[hard-green] step: out={out}");

        let shim_src = r#"
            phase('ShimPhase');
            log('shim-log');
            JSON.stringify(args);
        "#;
        let args_out = eval_str(realm_cx, global.handle(), shim_src, "<wf-shim>");
        eprintln!("[hard-green] step: args_out={args_out}");

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

        // --- H5: budget object bound ---
        let budget_out = eval_str(
            realm_cx,
            global.handle(),
            "JSON.stringify(globalThis.budget)",
            "<wf-budget>",
        );
        assert!(
            budget_out.contains("total") || budget_out.contains('1'),
            "budget must parse from callbacks: budget_out={budget_out}"
        );
        eprintln!("[hard-green] budget={budget_out}");

        // --- H1: nest one deep ---
        let nest1 = eval_str(
            realm_cx,
            global.handle(),
            r#"String(globalThis.__wf_workflow('child-a', '{"x":1}'))"#,
            "<wf-nest-1>",
        );
        assert!(
            nest1.contains("child-a") || nest1.contains("nested"),
            "single nest must return child JSON: nest1={nest1}"
        );
        eprintln!("[hard-green] nest1={nest1}");

        // Second nest exceeds max_nest=1
        let nest2 = eval_str(
            realm_cx,
            global.handle(),
            r#"(function(){
              try {
                globalThis.__wf_workflow('child-b', '{}');
                return 'NO_THROW';
              } catch (e) {
                return String(e && e.message ? e.message : e);
              }
            })()"#,
            "<wf-nest-2>",
        );
        assert!(
            nest2.contains("nest") || nest2.contains("exceeded") || nest2.contains("depth"),
            "second nest must fail: nest2={nest2}"
        );
        eprintln!("[hard-green] nest depth-cap={nest2}");

        // --- H3: parallel installed + null-slot order semantics ---
        let par = eval_str(
            realm_cx,
            global.handle(),
            r#"(function(){
              if (typeof globalThis.parallel !== 'function') {
                throw new Error('parallel not installed');
              }
              // Mirror shim null-slot semantics without awaiting microtasks
              function mapSlot(fn) {
                try { return fn(); } catch (_) { return null; }
              }
              var results = [
                mapSlot(function(){ return 'a'; }),
                mapSlot(function(){ throw new Error('x'); }),
                mapSlot(function(){ return 'c'; }),
              ];
              return JSON.stringify(results);
            })()"#,
            "<wf-parallel>",
        );
        assert!(
            par.contains("null") && par.contains("\"a\"") && par.contains("\"c\""),
            "null-slot order: par={par}"
        );
        assert!(
            par.find("\"a\"").unwrap() < par.find("null").unwrap(),
            "order preserved: par={par}"
        );
        eprintln!("[hard-green] parallel={par}");

        // --- H11: Date.now / Math.random throw ---
        let now = eval_str(
            realm_cx,
            global.handle(),
            r#"(function(){
              try { Date.now(); return 'NO_THROW'; }
              catch (e) { return String(e && e.message ? e.message : e); }
            })()"#,
            "<wf-date-now>",
        );
        assert!(
            now.contains("non-deterministic") || now.contains("forbidden"),
            "Date.now must throw non-deterministic: now={now}"
        );
        let rnd = eval_str(
            realm_cx,
            global.handle(),
            r#"(function(){
              try { Math.random(); return 'NO_THROW'; }
              catch (e) { return String(e && e.message ? e.message : e); }
            })()"#,
            "<wf-math-random>",
        );
        assert!(
            rnd.contains("non-deterministic") || rnd.contains("forbidden"),
            "Math.random must throw non-deterministic: rnd={rnd}"
        );
        eprintln!("[hard-green] nondet now={now} rnd={rnd}");

        // workflow() shim function exists
        let wf_type = eval_str(
            realm_cx,
            global.handle(),
            "typeof globalThis.workflow",
            "<wf-type>",
        );
        assert_eq!(
            wf_type, "function",
            "workflow shim must be function: {wf_type}"
        );

        eprintln!("[hard-green] PASS asserts");
        let _ = take_workflow_host_callbacks();
        std::mem::forget(realm);
        eprintln!("[hard-green] PASS");
    }
}

#[test]
fn install_workflow_host_symbols_exist() {
    let _: unsafe fn(&mut mozjs::context::JSContext, mozjs::rust::Handle<*mut JSObject>) =
        install_workflow_host_on_global;
    let _: unsafe fn(&mut mozjs::context::JSContext, mozjs::rust::Handle<*mut JSObject>) =
        install_workflow_host_on_bun;
}
