//! Hard-green runtime proof using raw SpiderMonkey (no bao_engine dual-def chain).
//!
//! Drives shipped `install_workflow_host_on_global` + host callbacks on a real
//! JSContext — not a reimplementation of the host. Failures must fail the test
//! (no soft-skip / eprintln-and-pass).
//!
//! Wave H: nest / **real** `globalThis.parallel` (Promise.all + job-queue drain)
//! / pipeline / nondet throw / budget (single SM process).
//!
//! Promise path: bare `Runtime::new` already runs `InitSelfHostedCode`, so
//! `js::UseInternalJobQueues` is too late. Install embedding job queue via
//! `CreateJobQueue` + `SetJobQueue`, then `RunJobs` until async IIFEs settle.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::CString;
use std::mem::ManuallyDrop;
use std::os::raw::c_void;
use std::ptr;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use bao_workflow_host::{
    WorkflowHostCallbacks, install_workflow_host_on_bun, install_workflow_host_on_global,
    js_to_rust_string, set_workflow_host_callbacks, take_workflow_host_callbacks,
};
use mozjs::glue::{CreateJobQueue, DeleteJobQueue, JobQueueTraps};
use mozjs::jsapi::{
    CurrentGlobalOrNull, HandleValueArray, JS_CallFunctionValue, JS_ClearPendingException,
    JS_DefineProperty, JS_DeleteProperty1, JS_GetProperty, JSObject, OnNewGlobalHookOption,
};
use mozjs::jsapi::{Handle as RawHandle, MutableHandle as RawMutableHandle};
use mozjs::jsval::{ObjectValue, UndefinedValue};
use mozjs::realm::AutoRealm;
use mozjs::rooted;
use mozjs::rust::wrappers2::{JS_NewGlobalObject, RunJobs, SetJobQueue};
use mozjs::rust::{CompileOptionsWrapper, JSEngine, RealmOptions, Runtime, SIMPLE_GLOBAL_CLASS};

// ── minimal SM job queue (embedding traps; mirrors bao_engine JobQueue shape) ──

static JOB_COUNTER: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static JOB_IDS: RefCell<VecDeque<usize>> = const { RefCell::new(VecDeque::new()) };
    static QUEUE_PTR: RefCell<*mut mozjs::jsapi::JobQueue> = const { RefCell::new(ptr::null_mut()) };
}

fn job_prop_name(id: usize) -> CString {
    CString::new(format!("__wf_test_job_{id}")).unwrap_or_default()
}

struct TestJobQueue;

impl TestJobQueue {
    /// Install internal promise job queue on this context (post-Runtime::new).
    fn init(cx: &mozjs::context::JSContext) -> Self {
        let traps = JobQueueTraps {
            getHostDefinedData: Some(get_host_defined_data),
            enqueuePromiseJob: Some(enqueue_job),
            runJobs: Some(run_jobs_trap),
            empty: Some(is_empty),
            pushNewInterruptQueue: Some(push_new_interrupt_queue),
            popInterruptQueue: Some(pop_interrupt_queue),
            dropInterruptQueues: Some(drop_interrupt_queues),
        };
        let queue = unsafe { CreateJobQueue(&traps, ptr::null(), ptr::null_mut()) };
        assert!(
            !queue.is_null(),
            "CreateJobQueue must succeed for Promise drain"
        );
        QUEUE_PTR.with(|p| *p.borrow_mut() = queue);
        unsafe {
            SetJobQueue(cx, queue);
        }
        Self
    }

    fn drain(cx: &mut mozjs::context::JSContext) {
        unsafe {
            RunJobs(cx);
        }
    }
}

impl Drop for TestJobQueue {
    fn drop(&mut self) {
        QUEUE_PTR.with(|p| {
            let q = *p.borrow();
            if !q.is_null() {
                unsafe {
                    DeleteJobQueue(q);
                }
                *p.borrow_mut() = ptr::null_mut();
            }
        });
        JOB_IDS.with(|q| q.borrow_mut().clear());
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn enqueue_job(
    _queue: *const c_void,
    cx: *mut mozjs::jsapi::JSContext,
    _promise: RawHandle<*mut JSObject>,
    job: RawHandle<*mut JSObject>,
    _allocation_site: RawHandle<*mut JSObject>,
    _host_defined_data: RawHandle<*mut JSObject>,
) -> bool {
    let job_obj = *job.ptr;
    if job_obj.is_null() {
        return true;
    }
    let id = JOB_COUNTER.fetch_add(1, Ordering::Relaxed);
    let global = CurrentGlobalOrNull(cx);
    if global.is_null() {
        return true;
    }
    let prop = job_prop_name(id);
    let wrapped_cx = mozjs::context::JSContext::from_ptr(ptr::NonNull::new_unchecked(cx));
    rooted!(&in(wrapped_cx) let job_root = ObjectValue(job_obj));
    rooted!(&in(wrapped_cx) let global_root = global);
    JS_DefineProperty(
        cx,
        global_root.handle().into(),
        prop.as_ptr(),
        job_root.handle().into(),
        0,
    );
    JOB_IDS.with(|q| q.borrow_mut().push_back(id));
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn run_jobs_trap(_queue: *const c_void, cx: *mut mozjs::jsapi::JSContext) {
    loop {
        let Some(id) = JOB_IDS.with(|q| q.borrow_mut().pop_front()) else {
            break;
        };
        let global = CurrentGlobalOrNull(cx);
        if global.is_null() {
            break;
        }
        let prop = job_prop_name(id);
        let wrapped_cx = mozjs::context::JSContext::from_ptr(ptr::NonNull::new_unchecked(cx));
        rooted!(&in(wrapped_cx) let global_root = global);
        let mut job_val = UndefinedValue();
        JS_GetProperty(
            cx,
            global_root.handle().into(),
            prop.as_ptr(),
            RawMutableHandle {
                _phantom_0: std::marker::PhantomData,
                ptr: &mut job_val,
            },
        );
        if !job_val.is_object() {
            continue;
        }
        let mut rval = UndefinedValue();
        rooted!(&in(wrapped_cx) let obj_root = global);
        rooted!(&in(wrapped_cx) let fval_root = job_val);
        let empty_args = HandleValueArray::empty();
        let ok = JS_CallFunctionValue(
            cx,
            obj_root.handle().into(),
            fval_root.handle().into(),
            &empty_args,
            RawMutableHandle {
                _phantom_0: std::marker::PhantomData,
                ptr: &mut rval,
            },
        );
        if !ok {
            JS_ClearPendingException(cx);
        }
        JS_DeleteProperty1(cx, global_root.handle().into(), prop.as_ptr());
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn get_host_defined_data(
    _queue: *const c_void,
    _cx: *mut mozjs::jsapi::JSContext,
    data: RawMutableHandle<*mut JSObject>,
) -> bool {
    data.set(ptr::null_mut());
    true
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn is_empty(_queue: *const c_void) -> bool {
    JOB_IDS.with(|q| q.borrow().is_empty())
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn push_new_interrupt_queue(_queues: *mut c_void) -> *const c_void {
    ptr::null()
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn pop_interrupt_queue(_queues: *mut c_void) -> *const c_void {
    ptr::null()
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn drop_interrupt_queues(_queues: *mut c_void) {}

// ── host callbacks ──

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
        // Fixture-shaped fail slot: agent reject → parallel catch → null.
        if prompt.contains("__fail_slot__") {
            return Err(format!("agent fail slot: {prompt}"));
        }
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

// ── eval helpers ──

fn eval_str(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
    src: &str,
    label: &str,
) -> String {
    unsafe {
        rooted!(&in(cx) let mut rval = UndefinedValue());
        let c_filename = CString::new(label).unwrap();
        let opts = CompileOptionsWrapper::new(cx, c_filename, 1);
        mozjs::rust::evaluate_script(cx, global, src, rval.handle_mut(), opts)
            .unwrap_or_else(|_| panic!("{label}: eval must succeed (hard green)"));
        js_to_rust_string(cx.raw_cx(), rval.get())
    }
}

/// Run `body` inside an async IIFE (may use `await`), drain SM job queue until
/// settled, return `String(globalThis.__async_result)`.
///
/// Body must assign `globalThis.__async_result` before completing.
fn eval_async(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
    body: &str,
    label: &str,
) -> String {
    let src = format!(
        r#"(function(){{
  globalThis.__async_done = false;
  globalThis.__async_err = null;
  globalThis.__async_result = undefined;
  (async function(){{
    try {{
      {body}
      globalThis.__async_done = true;
    }} catch (e) {{
      globalThis.__async_err = String(e && e.message ? e.message : e);
      globalThis.__async_done = true;
    }}
  }})();
  return 'scheduled';
}})()"#
    );
    let scheduled = eval_str(cx, global, &src, label);
    assert_eq!(
        scheduled, "scheduled",
        "{label}: async schedule marker: {scheduled}"
    );

    let mut settled = false;
    for _ in 0..10_000 {
        TestJobQueue::drain(cx);
        let done = eval_str(
            cx,
            global,
            "String(globalThis.__async_done === true)",
            &format!("{label}-poll"),
        );
        if done == "true" {
            settled = true;
            break;
        }
    }
    assert!(
        settled,
        "{label}: async did not settle after RunJobs drain (Promise job queue broken?)"
    );

    let err = eval_str(
        cx,
        global,
        "globalThis.__async_err == null ? '' : String(globalThis.__async_err)",
        &format!("{label}-err"),
    );
    assert!(err.is_empty(), "{label}: async body threw: {err}");

    eval_str(
        cx,
        global,
        "globalThis.__async_result === undefined ? 'undefined' : String(globalThis.__async_result)",
        &format!("{label}-result"),
    )
}

/// Like [`eval_async`] but expects the async body to throw; returns error message.
fn eval_async_expect_throw(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
    body: &str,
    label: &str,
) -> String {
    let src = format!(
        r#"(function(){{
  globalThis.__async_done = false;
  globalThis.__async_err = null;
  globalThis.__async_result = undefined;
  (async function(){{
    try {{
      {body}
      globalThis.__async_done = true;
    }} catch (e) {{
      globalThis.__async_err = String(e && e.message ? e.message : e);
      globalThis.__async_done = true;
    }}
  }})();
  return 'scheduled';
}})()"#
    );
    let _ = eval_str(cx, global, &src, label);
    let mut settled = false;
    for _ in 0..10_000 {
        TestJobQueue::drain(cx);
        let done = eval_str(
            cx,
            global,
            "String(globalThis.__async_done === true)",
            &format!("{label}-poll"),
        );
        if done == "true" {
            settled = true;
            break;
        }
    }
    assert!(settled, "{label}: async did not settle");
    let err = eval_str(
        cx,
        global,
        "globalThis.__async_err == null ? '' : String(globalThis.__async_err)",
        &format!("{label}-err"),
    );
    assert!(
        !err.is_empty(),
        "{label}: expected throw, got result={:?}",
        eval_str(
            cx,
            global,
            "String(globalThis.__async_result)",
            &format!("{label}-unexpected"),
        )
    );
    err
}

/// Single hard-green test (SM is process-singleton — do not split into multiple engine inits).
/// Covers: phase/log/agent/args + H1 nest + **real** H3 parallel + pipeline + H5 budget + H11 nondet.
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

    // Job queue lives for the whole test (DeleteJobQueue on drop at end).
    let _job_queue: ManuallyDrop<TestJobQueue>;

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

        // Promise microtasks: install after Runtime::new (self-hosted already on).
        _job_queue = ManuallyDrop::new(TestJobQueue::init(realm_cx));

        install_workflow_host_on_global(realm_cx, global.handle());
        eprintln!("[hard-green] step: install done");

        // ── P0: phase / log / agent natives (sync) ──
        let direct = r#"
            globalThis.__wf_phase('Research');
            globalThis.__wf_log('start');
            globalThis.__out = globalThis.__wf_agent('hello', '{}');
            String(globalThis.__out);
        "#;
        let out = eval_str(realm_cx, global.handle(), direct, "<wf-direct>");
        assert_eq!(out, "\"echo:hello\"", "agent native exact: out={out}");

        let args_out = eval_str(
            realm_cx,
            global.handle(),
            "phase('ShimPhase'); log('shim-log'); JSON.stringify(args);",
            "<wf-shim-args>",
        );
        assert_eq!(args_out, r#"{"k":1}"#, "args exact: {args_out}");

        let ph = phases.lock().unwrap().clone();
        let lg = logs.lock().unwrap().clone();
        assert_eq!(
            ph,
            vec!["Research".to_string(), "ShimPhase".to_string()],
            "phases exact order: {ph:?}"
        );
        assert_eq!(
            lg,
            vec!["start".to_string(), "shim-log".to_string()],
            "logs exact order: {lg:?}"
        );

        // ── H5: budget ──
        let budget_out = eval_str(
            realm_cx,
            global.handle(),
            "JSON.stringify(globalThis.budget)",
            "<wf-budget>",
        );
        assert_eq!(budget_out, r#"{"total":10}"#, "budget exact: {budget_out}");
        eprintln!("[hard-green] budget={budget_out}");

        // ── Fixture shape: phase + log + agent + return (async, real shims) ──
        // Homologous to frog smoke-agent-phase.mjs
        let agent_fix = eval_async(
            realm_cx,
            global.handle(),
            r#"
              phase('Research2');
              log('start2');
              const out = await agent('hello-world', {
                schema: {
                  type: 'object',
                  required: ['files'],
                  properties: { files: { type: 'array' } },
                },
                label: 't1',
              });
              globalThis.__async_result = JSON.stringify({ out: out, status: 'ok' });
            "#,
            "<fixture-agent-phase>",
        );
        assert_eq!(
            agent_fix, r#"{"out":"echo:hello-world","status":"ok"}"#,
            "fixture agent+return exact: {agent_fix}"
        );
        eprintln!("[hard-green] fixture agent-phase={agent_fix}");

        // ── H3: REAL globalThis.parallel — three slots, middle throws → null ──
        // Exact order ["a",null,"c"] — no mapSlot reimplementation.
        let par = eval_async(
            realm_cx,
            global.handle(),
            r#"
              if (typeof globalThis.parallel !== 'function') {
                throw new Error('parallel not installed');
              }
              const results = await globalThis.parallel([
                function(){ return 'a'; },
                function(){ throw new Error('x'); },
                function(){ return 'c'; },
              ]);
              globalThis.__async_result = JSON.stringify(results);
            "#,
            "<wf-parallel-real>",
        );
        assert_eq!(
            par, r#"["a",null,"c"]"#,
            "real parallel null-slot order exact: par={par}"
        );
        eprintln!("[hard-green] parallel REAL={par}");

        // Fixture-shaped parallel (smoke-parallel.mjs): agent slots + fail → null
        let par_fix = eval_async(
            realm_cx,
            global.handle(),
            r#"
              phase('Fanout');
              const results = await parallel([
                () => agent('ok-a'),
                () => agent('ok-b'),
                () => agent('__fail_slot__'),
              ]);
              globalThis.__async_result = JSON.stringify(results);
            "#,
            "<fixture-parallel>",
        );
        assert_eq!(
            par_fix, r#"["echo:ok-a","echo:ok-b",null]"#,
            "fixture parallel exact: {par_fix}"
        );
        eprintln!("[hard-green] fixture parallel={par_fix}");

        // ── pipeline ≥1 stage (smoke-pipeline.mjs shape) ──
        let pipe = eval_async(
            realm_cx,
            global.handle(),
            r#"
              phase('Audit');
              const items = ['alpha', 'beta'];
              const audits = await pipeline(items, (item) => agent('Audit ' + item));
              globalThis.__async_result = JSON.stringify(audits);
            "#,
            "<fixture-pipeline>",
        );
        assert_eq!(
            pipe, r#"["echo:Audit alpha","echo:Audit beta"]"#,
            "pipeline one-stage exact: {pipe}"
        );
        eprintln!("[hard-green] fixture pipeline={pipe}");

        // ── H1: workflow nest one deep via real `workflow` shim (async) ──
        nest_depth.store(0, Ordering::SeqCst);
        let nest1 = eval_async(
            realm_cx,
            global.handle(),
            r#"
              phase('Parent');
              log('parent-start');
              const nested = await workflow('child-a', { x: 1 });
              globalThis.__async_result = JSON.stringify({ nested: nested, status: 'ok' });
            "#,
            "<fixture-nest-1>",
        );
        assert_eq!(
            nest1, r#"{"nested":{"nested":"child-a","args":{"x":1},"status":"ok"},"status":"ok"}"#,
            "workflow nest one-deep exact: {nest1}"
        );
        eprintln!("[hard-green] nest1={nest1}");

        // Over-depth: max_nest=1, depth already 1 after nest1 → second must throw
        let nest2 = eval_async_expect_throw(
            realm_cx,
            global.handle(),
            r#"
              await workflow('child-b', {});
              globalThis.__async_result = 'NO_THROW';
            "#,
            "<fixture-nest-deep>",
        );
        assert!(
            nest2.contains("nest") && (nest2.contains("exceeded") || nest2.contains("depth")),
            "second nest must fail with depth message: nest2={nest2}"
        );
        eprintln!("[hard-green] nest depth-cap={nest2}");

        // ── H11: Date.now / Math.random throw ──
        let now = eval_str(
            realm_cx,
            global.handle(),
            r#"(function(){
              try { Date.now(); return 'NO_THROW'; }
              catch (e) { return String(e && e.message ? e.message : e); }
            })()"#,
            "<wf-date-now>",
        );
        assert_eq!(
            now, "workflow host: non-deterministic API 'Date.now' is forbidden",
            "Date.now exact: now={now}"
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
        assert_eq!(
            rnd, "workflow host: non-deterministic API 'Math.random' is forbidden",
            "Math.random exact: rnd={rnd}"
        );
        eprintln!("[hard-green] nondet now={now} rnd={rnd}");

        // workflow / parallel / pipeline types
        assert_eq!(
            eval_str(
                realm_cx,
                global.handle(),
                "typeof globalThis.workflow",
                "<wf-type>",
            ),
            "function"
        );
        assert_eq!(
            eval_str(
                realm_cx,
                global.handle(),
                "typeof globalThis.parallel",
                "<par-type>",
            ),
            "function"
        );
        assert_eq!(
            eval_str(
                realm_cx,
                global.handle(),
                "typeof globalThis.pipeline",
                "<pipe-type>",
            ),
            "function"
        );

        eprintln!("[hard-green] PASS asserts");
        let _ = take_workflow_host_callbacks();
        // Leak realm + job queue + engine: SM process-singleton teardown is unsafe mid-test harness.
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
