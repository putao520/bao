// @trace REQ-ENG-006 [api:node:worker_threads]
//
// Node.js `worker_threads` builtin module — real OS-thread Workers.
//
// Architecture:
//   - SpiderMonkey's JSEngine is process-global (OnceLock<JSEngineHandle>).
//   - Each Worker spawns a std::thread that calls Runtime::new(handle) to get
//     its own thread-local JSContext. No cross-thread JSObject sharing.
//   - Messages cross threads as SpiderMonkey structured-clone bytes via mpsc
//     channels (Node semantics: postMessage = structured clone, NOT JSON —
//     TypedArray/Map/Set/Date/BigInt/cyclic objects keep their types).
//   - Worker JS objects (postMessage/terminate/threadId) are native host fns.

use ::std::cell::RefCell;
use ::std::ffi::CString;
use ::std::ptr::NonNull;
use ::std::sync::atomic::{AtomicU32, Ordering};
use ::std::sync::mpsc::{self, Receiver, Sender};
use ::std::sync::OnceLock;

use dashmap::DashMap;
use mozjs::conversions::unsafe_jsstr_to_string;
use mozjs::glue::{
    CopyJSStructuredCloneData, GetLengthOfJSStructuredCloneData, WriteBytesToJSStructuredCloneData,
};
use mozjs::jsapi::*;
use mozjs::jsval::{BooleanValue, Int32Value, JSVal, ObjectValue, StringValue, UndefinedValue};
use mozjs::realm::AutoRealm;
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;
use mozjs::rust::JSAutoStructuredCloneBufferWrapper;

use crate::require::cache_builtin;

// ---------------------------------------------------------------------------
// Worker registry (process-global)
// ---------------------------------------------------------------------------

/// Next thread ID counter (monotonically increasing).
static NEXT_THREAD_ID: AtomicU32 = AtomicU32::new(1);

/// Process-wide registry of live Workers, keyed by threadId.
/// Stores the sender half so main-thread code can postMessage / terminate.
static WORKER_REGISTRY: OnceLock<DashMap<u32, WorkerHandle>> = OnceLock::new();

fn worker_registry() -> &'static DashMap<u32, WorkerHandle> {
    WORKER_REGISTRY.get_or_init(DashMap::new)
}

/// Handle held by the main thread for each Worker.
struct WorkerHandle {
    sender: Sender<WorkerMessage>,
    /// JoinHandle for the worker OS thread, taken on terminate/join.
    thread: Option<::std::thread::JoinHandle<()>>,
    /// Receiver for worker → main messages, drained non-blockingly by
    /// `worker_try_recv` (the main-side receive primitive). Mutex-wrapped:
    /// mpsc::Receiver is !Sync but the registry is a process-global static.
    main_rx: Option<::std::sync::Mutex<Receiver<WorkerToMainMessage>>>,
}

// ---------------------------------------------------------------------------
// Cross-thread messages
// ---------------------------------------------------------------------------

enum WorkerMessage {
    /// Structured-clone bytes from main → worker.
    Data(Vec<u8>),
    /// Signal the worker thread to exit.
    Terminate,
}

/// Messages from worker thread → main thread.
enum WorkerToMainMessage {
    /// Structured-clone bytes.
    Data(Vec<u8>),
    /// Error message.
    Error(String),
}

// ---------------------------------------------------------------------------
// Structured clone (SpiderMonkey engine, no host callbacks)
// ---------------------------------------------------------------------------
//
// Node semantics: postMessage uses the structured clone algorithm. We use
// SpiderMonkey's own JS_WriteStructuredClone / JS_ReadStructuredClone — the
// same engine servo's DOM postMessage builds on — with NO host callbacks:
// every plain JS value type is covered natively (Map/Set/Date/RegExp/
// TypedArray/ArrayBuffer/BigInt/cyclic object graphs preserve identity), and
// anything the engine cannot clone (functions, WeakMap, ...) fails the write,
// which the callers surface as a DataCloneError. `DifferentProcess` scope
// keeps the serialized form a flat byte buffer, safe to move across threads
// via mpsc. Both ends live in the same binary, so no protocol versioning is
// needed beyond the engine's own JS_STRUCTURED_CLONE_VERSION header.

/// Clone data policy: shared-memory objects are rejected (cross-thread SAB
/// semantics are not provided); everything else clones.
fn sc_clone_policy() -> CloneDataPolicy {
    CloneDataPolicy {
        allowIntraClusterClonableSharedObjects_: false,
        allowSharedMemoryObjects_: false,
    }
}

/// Serialize `value` into structured-clone bytes. `Err(())` when the value
/// contains anything the structured clone algorithm cannot clone — the caller
/// must report a DataCloneError (Node ERR_DATACLONE_ERROR semantics).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn sc_serialize(raw_cx: *mut JSContext, value: JSVal) -> ::std::result::Result<Vec<u8>, ()> {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(raw_cx));
    let cx = &mut wrapped_cx;

    rooted!(&in(cx) let val = value);
    rooted!(&in(cx) let mut no_transfer = UndefinedValue());

    // SAFETY: scbuf owns the clone buffer until the bytes are copied out
    // below; null callbacks = no host custom types (unsupported → write
    // fails, which is the DataCloneError path).
    let scbuf = unsafe {
        JSAutoStructuredCloneBufferWrapper::new(
            StructuredCloneScope::DifferentProcess,
            ::std::ptr::null(),
        )
    };
    let scdata = unsafe { &mut ((*scbuf.as_raw_ptr()).data_) };

    let ok = unsafe {
        w2::JS_WriteStructuredClone(
            cx,
            val.handle(),
            scdata,
            StructuredCloneScope::DifferentProcess,
            &sc_clone_policy(),
            ::std::ptr::null(),
            ::std::ptr::null_mut(),
            no_transfer.handle(),
        )
    };
    if !ok {
        return Err(());
    }

    let nbytes = unsafe { GetLengthOfJSStructuredCloneData(scdata) };
    let mut bytes = Vec::with_capacity(nbytes);
    unsafe {
        CopyJSStructuredCloneData(scdata, bytes.as_mut_ptr());
        bytes.set_len(nbytes);
    }
    Ok(bytes)
}

/// Deserialize structured-clone bytes into `rval`. The caller must have
/// entered the realm the resulting objects should live in (objects are
/// created in the current realm).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn sc_deserialize(
    raw_cx: *mut JSContext,
    bytes: &[u8],
    rval: mozjs::gc::MutableHandleValue<'_>,
) -> bool {
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(raw_cx));
    let cx = &mut wrapped_cx;

    let scbuf = unsafe {
        JSAutoStructuredCloneBufferWrapper::new(
            StructuredCloneScope::DifferentProcess,
            ::std::ptr::null(),
        )
    };
    let scdata = unsafe { &mut ((*scbuf.as_raw_ptr()).data_) };

    if !bytes.is_empty()
        && !unsafe { WriteBytesToJSStructuredCloneData(bytes.as_ptr(), bytes.len(), scdata) }
    {
        return false;
    }

    unsafe {
        w2::JS_ReadStructuredClone(
            cx,
            scdata,
            JS_STRUCTURED_CLONE_VERSION,
            StructuredCloneScope::DifferentProcess,
            rval,
            &sc_clone_policy(),
            ::std::ptr::null(),
            ::std::ptr::null_mut(),
        )
    }
}

/// Report a DataCloneError (Node message shape) and clear any pending
/// engine exception so the thrown error is deterministic.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn report_data_clone_error(raw_cx: *mut JSContext) {
    let wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(raw_cx));
    if w2::JS_IsExceptionPending(&wrapped_cx) {
        JS_ClearPendingException(raw_cx);
    }
    JS_ReportErrorUTF8(
        raw_cx,
        c"DataCloneError: The object could not be cloned.".as_ptr(),
    );
}

// ---------------------------------------------------------------------------
// Worker thread entry point
// ---------------------------------------------------------------------------

fn worker_entry(
    filename: String,
    thread_id: u32,
    receiver: Receiver<WorkerMessage>,
    main_sender: Sender<WorkerToMainMessage>,
    worker_data_bytes: Option<Vec<u8>>,
) {
    // 1. Obtain process-global JSEngine handle.
    let engine_handle = match bao_engine::context::ensure_engine_handle() {
        Ok(h) => h,
        Err(_) => return,
    };

    // 2. Create a new Runtime on this thread — gets its own JSContext.
    let _runtime = mozjs::rust::Runtime::new(engine_handle);

    // 3. Wrap the worker's Runtime in a JsContext (parasitic — Runtime::new
    //    above already set the TLS) with the worker global setup.
    //    Realm-per-context: the worker's single realm is created lazily by
    //    the realm-init eval below and persists for the worker's whole
    //    lifetime, published to thread_realm_global so the message loop and
    //    async dispatch can AutoRealm into it.
    let mut ctx = match unsafe { bao_engine::context::JsContext::from_servo_runtime() } {
        Ok(c) => c,
        Err(_) => return,
    };
    ctx.set_global_setup(worker_global_setup);

    let mut cx = ctx.cx();
    let raw_cx = ctx.raw_cx();

    // 4. Init JobQueue + ModuleLoader on this thread's JSContext.
    if !bao_engine::job_queue::JobQueue::init(&cx) {
        return;
    }
    bao_engine::module_loader::ModuleLoader::init_thread_local(&cx);
    bao_engine::module_loader::set_job_queue_drain(bao_engine::job_queue::JobQueue::drain);

    // 5. Read the worker script from disk.
    let source = match ::std::fs::read_to_string(&filename) {
        Ok(s) => s,
        Err(e) => {
            let msg = format!("Worker: failed to read '{}': {}", filename, e);
            let _ = main_sender.send(WorkerToMainMessage::Error(msg));
            return;
        }
    };

    // 6. Build a bootstrap script that sets up self.onmessage / self.postMessage
    //    then evaluates the worker source.
    //
    //    The worker script can use:
    //      - self.onmessage = function(e) { ... }  (e.data = structured-clone value)
    //      - self.postMessage(data)  (structured-clones data, sends to main thread)
    //      - workerData (from options, also structured-cloned)
    let bootstrap = format!(
        r#"(function() {{
  // workerData — deserialized from structured-clone bytes by the host before
  // this script runs; non-enumerable raw value is deleted after capture.
  var workerData = (typeof __baoWorkerDataRaw === 'undefined') ? null : __baoWorkerDataRaw;
  delete self.__baoWorkerDataRaw;

  var __pendingMessages = [];

  // self.postMessage — structured clone via native host fn; uncloneable
  // values (functions, ...) throw DataCloneError, matching Node.
  self.postMessage = __baoPostToMain;

  // Queue messages until onmessage handler is set. `data` arrives already
  // deserialized from structured-clone bytes by the host.
  self.__baoDeliverMessage = function(data) {{
    if (typeof self.onmessage === 'function') {{
      self.onmessage({{ data: data }});
    }} else {{
      __pendingMessages.push(data);
    }}
  }};

  // When onmessage is set, deliver any queued messages
  var __origOnMessage = null;
  Object.defineProperty(self, 'onmessage', {{
    configurable: true,
    enumerable: true,
    get: function() {{ return __origOnMessage; }},
    set: function(fn) {{
      __origOnMessage = fn;
      // Deliver queued messages
      while (__pendingMessages.length > 0 && typeof fn === 'function') {{
        var data = __pendingMessages.shift();
        fn({{ data: data }});
      }}
    }}
  }});

  // parentPort stub (worker_threads compat)
  var parentPort = {{
    postMessage: self.postMessage,
    on: function() {{}},
    once: function() {{}},
    removeListener: function() {{}},
  }};

  // isMainThread is false inside workers
  self.isMainThread = false;
  self.threadId = {thread_id};
  self.parentPort = parentPort;

  // Execute the worker script
  {source}
}})();"#,
        thread_id = thread_id,
        source = source,
    );

    // 7. Initialize the worker's persistent realm (lazily creates the
    //     global, applies worker_global_setup exactly once, publishes
    //     thread_realm_global). Idempotent + no eval runs, so no exit
    //     dispatch. Then evaluate the bootstrap module INSIDE that realm —
    //     the same realm every later dispatch on this worker (message
    //     delivery, timers, job queue) uses.
    let global_ptr = match ctx.ensure_realm_global(&mut cx, Some(worker_global_setup)) {
        Ok(g) if !g.is_null() => g,
        Ok(_) => {
            let _ = main_sender.send(WorkerToMainMessage::Error(
                "Worker realm global null after ensure_realm_global".into(),
            ));
            return;
        }
        Err(e) => {
            let _ = main_sender.send(WorkerToMainMessage::Error(format!(
                "Worker realm init failed: {}",
                e.message
            )));
            return;
        }
    };
    rooted!(&in(cx) let global = global_ptr);

    // 7a. Deserialize workerData (structured-clone bytes produced on the main
    //     thread) and publish it on the worker global as a non-enumerable
    //     raw value; the bootstrap captures it into `var workerData` and
    //     deletes the global property.
    if let Some(wd_bytes) = worker_data_bytes.as_ref() {
        let mut realm = AutoRealm::new_from_handle(&mut cx, global.handle());
        let realm_cx: &mut mozjs::context::JSContext = &mut realm;
        rooted!(&in(realm_cx) let mut wd_val = UndefinedValue());
        let wd_ok = unsafe { sc_deserialize(realm_cx.raw_cx(), wd_bytes, wd_val.handle_mut()) };
        if !wd_ok {
            let _ = main_sender.send(WorkerToMainMessage::Error(
                "Worker: workerData structured-clone deserialization failed".into(),
            ));
            return;
        }
        unsafe {
            JS_DefineProperty(
                realm_cx.raw_cx(),
                global.handle().into(),
                c"__baoWorkerDataRaw".as_ptr(),
                wd_val.handle().into(),
                0u32, // not enumerable
            );
        }
    }

    let eval_result = bao_engine::module_loader::ModuleLoader::eval_module_in_realm(
        &mut cx,
        &bootstrap,
        &filename,
        None,
        global.handle(),
    );

    if let Err(e) = eval_result {
        let msg = format!(
            "Worker script error: {} ({}:{})",
            e.message, e.filename, e.line
        );
        let _ = main_sender.send(WorkerToMainMessage::Error(msg));
        return;
    }

    // 8. Drain the job queue (process any microtasks from the script).
    bao_engine::job_queue::JobQueue::drain(&mut cx);

    // 9. Store the main_sender in TLS for __baoPostToMain to access.
    WORKER_MAIN_SENDER.with(|s| {
        *s.borrow_mut() = Some(main_sender);
    });

    // 10. Message receive loop: wait for messages from main thread.
    loop {
        match receiver.recv() {
            Ok(WorkerMessage::Data(sc_bytes)) => {
                // Deserialize structured-clone bytes and call
                // self.__baoDeliverMessage(data) on the worker's global.
                deliver_message_to_worker(raw_cx, &sc_bytes);
                bao_engine::job_queue::JobQueue::drain(&mut cx);
            }
            Ok(WorkerMessage::Terminate) | Err(_) => {
                break;
            }
        }
    }
}

/// Global setup for worker JSContext — installs __baoPostToMain native function.
unsafe fn worker_global_setup(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut JSObject>,
) {
    // Install __baoPostToMain(data) on the global object.
    // This is called by self.postMessage() to send data to the main thread.
    w2::JS_DefineFunction(
        cx,
        global,
        c"__baoPostToMain".as_ptr(),
        Some(worker_post_to_main),
        1,
        JSPROP_ENUMERATE as u32,
    );

    // Node / WorkerGlobalScope semantics: `self` is an alias of the worker
    // global. SpiderMonkey does not provide it on a bare embedding global,
    // and without it every worker bootstrap that touches `self` throws a
    // ReferenceError that module evaluation silently captures in its
    // evaluation promise — the worker then runs its message loop with none
    // of its globals installed.
    rooted!(&in(cx) let global_val = ObjectValue(global.get()));
    JS_DefineProperty(
        cx.raw_cx(),
        global.into(),
        c"self".as_ptr(),
        global_val.handle().into(),
        (JSPROP_ENUMERATE | JSPROP_READONLY | JSPROP_PERMANENT) as u32,
    );
}

/// Native function: __baoPostToMain(data) — called from worker JS to post a
/// message to the main thread. The argument is structured-cloned; uncloneable
/// values throw DataCloneError (Node semantics).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn worker_post_to_main(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"__baoPostToMain requires a value argument".as_ptr());
        return false;
    }

    let data_val = *args.get(0).ptr;
    let sc_bytes = match unsafe { sc_serialize(cx, data_val) } {
        Ok(bytes) => bytes,
        Err(()) => {
            unsafe { report_data_clone_error(cx) };
            return false;
        }
    };

    // Find this worker's main-sender from a thread-local.
    WORKER_MAIN_SENDER.with(|sender| {
        if let Some(tx) = sender.borrow().as_ref() {
            let _ = tx.send(WorkerToMainMessage::Data(sc_bytes));
        }
    });

    args.rval().set(UndefinedValue());
    true
}

// Thread-local for the main-thread sender, set by worker_entry.
thread_local! {
    static WORKER_MAIN_SENDER: RefCell<Option<Sender<WorkerToMainMessage>>> =
        RefCell::new(None);
}

/// Deserialize structured-clone bytes and call self.__baoDeliverMessage(data)
/// in the worker's JSContext.
fn deliver_message_to_worker(raw_cx: *mut JSContext, sc_bytes: &[u8]) {
    unsafe {
        // Realm-per-context: the message loop runs after the bootstrap
        // eval's AutoRealm popped — no realm is entered, so
        // CurrentGlobalOrNull is NULL here (under the old eval-per-global
        // model every message was silently dropped at this point). Enter the
        // worker's persistent realm, published by the realm-init eval.
        let global = match bao_engine::context::thread_realm_global() {
            Some(g) if !g.is_null() => g,
            _ => return,
        };

        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(raw_cx));
        let cx = &mut wrapped_cx;

        rooted!(&in(cx) let global_root = global);
        // All JS below (deserialization, property lookup, call) must run in
        // the realm that owns the global — objects created by the clone
        // reader land in the current realm.
        let mut realm = AutoRealm::new_from_handle(cx, global_root.handle());
        let cx: &mut mozjs::context::JSContext = &mut realm;

        rooted!(&in(cx) let mut data_val = UndefinedValue());
        if !sc_deserialize(raw_cx, sc_bytes, data_val.handle_mut()) {
            // Explicit error path: corrupt bytes must not be silently
            // dropped (same-binary serialization makes this unreachable in
            // practice; report it to the main thread instead).
            WORKER_MAIN_SENDER.with(|sender| {
                if let Some(tx) = sender.borrow().as_ref() {
                    let _ = tx.send(WorkerToMainMessage::Error(
                        "Worker: message structured-clone deserialization failed".into(),
                    ));
                }
            });
            return;
        }

        rooted!(&in(cx) let mut fn_val = UndefinedValue());
        JS_GetProperty(
            raw_cx,
            global_root.handle().into(),
            c"__baoDeliverMessage".as_ptr(),
            fn_val.handle_mut().into(),
        );

        if !fn_val.is_object() {
            return;
        }

        rooted!(&in(cx) let fn_obj = fn_val.to_object());

        let call_args_elements = [data_val.get()];
        let call_args = HandleValueArray {
            length_: 1,
            elements_: call_args_elements.as_ptr() as *const Value,
        };

        rooted!(&in(cx) let fn_obj_val = ObjectValue(fn_obj.get()));
        rooted!(&in(cx) let mut rval = UndefinedValue());
        JS_CallFunctionValue(
            raw_cx,
            global_root.handle().into(),
            fn_obj_val.handle().into(),
            &call_args,
            rval.handle_mut().into(),
        );
    }
}

// ---------------------------------------------------------------------------
// JS-exposed Worker constructor and methods
// ---------------------------------------------------------------------------

/// Worker constructor: `new Worker(filename, options?)`.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn worker_constructor(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"Worker requires a filename argument".as_ptr());
        return false;
    }

    let filename_val = *args.get(0).ptr;
    if !filename_val.is_string() {
        JS_ReportErrorUTF8(
            cx,
            c"Worker first argument must be a string filename".as_ptr(),
        );
        return false;
    }

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let filename = unsafe_jsstr_to_string(
        wrapped_cx.raw_cx(),
        NonNull::new_unchecked(filename_val.to_string()),
    );

    // Validate the entry path synchronously, before spawning the worker thread.
    // Node.js throws ERR_WORKER_PATH / ENOENT from the Worker constructor itself
    // when the file does not exist or the path is empty; the previous async
    // (channel-reported) error never surfaced as a JS exception, so
    // `new Worker('/nonexistent')` did not throw — which several conformance
    // tests rely on. Do NOT defer this to the worker thread.
    // Check the raw filename *before* resolve — an empty string would otherwise
    // resolve to cwd and silently pass.
    if filename.is_empty() {
        JS_ReportErrorUTF8(cx, c"Worker: entry file path must not be empty".as_ptr());
        return false;
    }

    // Resolve filename to absolute path.
    let abs_filename = if ::std::path::Path::new(&filename).is_absolute() {
        filename.clone()
    } else {
        match ::std::env::current_dir() {
            Ok(cwd) => cwd.join(&filename).to_string_lossy().to_string(),
            Err(_) => filename.clone(),
        }
    };

    if !::std::path::Path::new(&abs_filename).exists() {
        let msg = format!("Worker: entry file not found: {}", abs_filename);
        let c_msg = ::std::ffi::CString::new(msg).unwrap_or_default();
        JS_ReportErrorUTF8(cx, c_msg.as_ptr());
        return false;
    }

    // Parse options (second argument, optional object).
    let mut worker_data_bytes: Option<Vec<u8>> = None;
    if argc > 1 {
        let opts_val = *args.get(1).ptr;
        if opts_val.is_object() {
            let opts_obj = opts_val.to_object();
            let cx_ref = &mut wrapped_cx;
            rooted!(&in(cx_ref) let opts_root = opts_obj);
            let mut wd_val = UndefinedValue();
            JS_GetProperty(
                cx,
                opts_root.handle().into(),
                c"workerData".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut wd_val,
                },
            );
            if !wd_val.is_undefined() {
                // Serialize workerData with the structured clone algorithm
                // (Node semantics). Uncloneable workerData is a constructor
                // error, not a silent null.
                match unsafe { sc_serialize(cx, wd_val) } {
                    Ok(bytes) => worker_data_bytes = Some(bytes),
                    Err(()) => {
                        unsafe { report_data_clone_error(cx) };
                        return false;
                    }
                }
            }
        }
    }

    // Allocate thread ID.
    let thread_id = NEXT_THREAD_ID.fetch_add(1, Ordering::Relaxed);

    // Create channels.
    let (main_to_worker_tx, main_to_worker_rx): (Sender<WorkerMessage>, Receiver<WorkerMessage>) =
        mpsc::channel();
    let (worker_to_main_tx, worker_to_main_rx): (
        Sender<WorkerToMainMessage>,
        Receiver<WorkerToMainMessage>,
    ) = mpsc::channel();

    // Spawn the worker OS thread.
    let worker_filename = abs_filename.clone();
    let join_handle = ::std::thread::Builder::new()
        .name(format!("bao-worker-{}", thread_id))
        .spawn(move || {
            worker_entry(
                worker_filename,
                thread_id,
                main_to_worker_rx,
                worker_to_main_tx,
                worker_data_bytes,
            );
        });

    let join_handle = match join_handle {
        Ok(h) => h,
        Err(e) => {
            let msg = format!("Worker: failed to spawn thread: {}", e);
            let c_msg = CString::new(msg).unwrap_or_default();
            JS_ReportErrorUTF8(cx, c_msg.as_ptr());
            return false;
        }
    };

    // Register the worker handle. The worker → main receiver lives here and
    // is drained via `worker_try_recv` (main-side receive primitive).
    worker_registry().insert(
        thread_id,
        WorkerHandle {
            sender: main_to_worker_tx,
            thread: Some(join_handle),
            main_rx: Some(::std::sync::Mutex::new(worker_to_main_rx)),
        },
    );

    // Create the Worker JS object with postMessage, terminate, threadId.
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let worker_obj = w2::JS_NewPlainObject(cx_ref));
    if worker_obj.get().is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }

    // Store threadId as a private property so host fns can read it.
    rooted!(&in(cx_ref) let tid_val = Int32Value(thread_id as i32));
    JS_DefineProperty(
        cx,
        worker_obj.handle().into(),
        c"__threadId".as_ptr(),
        tid_val.handle().into(),
        0u32, // not enumerable
    );

    // Store the worker_to_main receiver on the registry handle — drained via
    // `worker_try_recv` instead of a boxed raw pointer on the JS object.

    // Install methods on the instance.
    w2::JS_DefineFunction(
        cx_ref,
        worker_obj.handle(),
        c"postMessage".as_ptr(),
        Some(worker_post_message),
        1,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        worker_obj.handle(),
        c"terminate".as_ptr(),
        Some(worker_terminate),
        0,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        worker_obj.handle(),
        c"ref".as_ptr(),
        Some(worker_noop),
        0,
        JSPROP_ENUMERATE as u32,
    );
    w2::JS_DefineFunction(
        cx_ref,
        worker_obj.handle(),
        c"unref".as_ptr(),
        Some(worker_noop),
        0,
        JSPROP_ENUMERATE as u32,
    );

    // threadId (read-only enumerable property).
    rooted!(&in(cx_ref) let tid_enum = Int32Value(thread_id as i32));
    JS_DefineProperty(
        cx,
        worker_obj.handle().into(),
        c"threadId".as_ptr(),
        tid_enum.handle().into(),
        (JSPROP_ENUMERATE | JSPROP_READONLY) as u32,
    );

    args.rval().set(ObjectValue(worker_obj.get()));
    true
}

/// Worker.prototype.postMessage(data) — serialize data to JSON, send to worker thread.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn worker_post_message(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    // Get threadId from the Worker object.
    let this_val = args.thisv();
    if !this_val.is_object() {
        JS_ReportErrorUTF8(
            cx,
            c"Worker.prototype.postMessage called on non-object".as_ptr(),
        );
        return false;
    }

    let this_obj = this_val.to_object();
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let this_root = this_obj);
    let mut tid_val = UndefinedValue();
    JS_GetProperty(
        cx,
        this_root.handle().into(),
        c"__threadId".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut tid_val,
        },
    );

    if !tid_val.is_int32() {
        JS_ReportErrorUTF8(cx, c"Worker: invalid threadId".as_ptr());
        return false;
    }
    let thread_id = tid_val.to_int32() as u32;

    // Transfer list (second argument): explicitly rejected until transfer
    // infrastructure exists (Node accepts an empty list, which is a no-op).
    if argc > 1 {
        let transfer_val = *args.get(1).ptr;
        if transfer_val.is_object() {
            let transfer_obj = transfer_val.to_object();
            rooted!(&in(cx_ref) let transfer_root = transfer_obj);
            let mut len_val = UndefinedValue();
            JS_GetProperty(
                cx,
                transfer_root.handle().into(),
                c"length".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut len_val,
                },
            );
            if len_val.is_int32() && len_val.to_int32() > 0 {
                JS_ReportErrorUTF8(
                    cx,
                    c"DataCloneError: postMessage transfer list is not supported in Bao".as_ptr(),
                );
                return false;
            }
        }
    }

    // Serialize the argument with the structured clone algorithm.
    // Uncloneable values (functions, WeakMap, ...) throw DataCloneError —
    // the old JSON path silently degraded them to null.
    let sc_bytes = if argc > 0 {
        let data_val = *args.get(0).ptr;
        match unsafe { sc_serialize(cx, data_val) } {
            Ok(bytes) => bytes,
            Err(()) => {
                unsafe { report_data_clone_error(cx) };
                return false;
            }
        }
    } else {
        // No payload: postMessage() — clone `undefined` (SC supports it).
        match unsafe { sc_serialize(cx, UndefinedValue()) } {
            Ok(bytes) => bytes,
            Err(()) => {
                unsafe { report_data_clone_error(cx) };
                return false;
            }
        }
    };

    // Send to the worker thread.
    if let Some(handle) = worker_registry().get_mut(&thread_id) {
        let _ = handle.sender.send(WorkerMessage::Data(sc_bytes));
    }

    args.rval().set(UndefinedValue());
    true
}

/// Worker.prototype.terminate() — signal the worker to exit and join its thread.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn worker_terminate(cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);

    let this_val = args.thisv();
    if !this_val.is_object() {
        args.rval().set(UndefinedValue());
        return true;
    }

    let this_obj = this_val.to_object();
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let this_root = this_obj);
    let mut tid_val = UndefinedValue();
    JS_GetProperty(
        cx,
        this_root.handle().into(),
        c"__threadId".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut tid_val,
        },
    );

    if !tid_val.is_int32() {
        args.rval().set(UndefinedValue());
        return true;
    }
    let thread_id = tid_val.to_int32() as u32;

    // Remove from registry and join the thread (dropping the handle also
    // drops the worker → main receiver).
    if let Some((_, mut handle)) = worker_registry().remove(&thread_id) {
        let _ = handle.sender.send(WorkerMessage::Terminate);
        if let Some(join) = handle.thread.take() {
            let _ = join.join();
        }
    }

    args.rval().set(UndefinedValue());
    true
}

/// Worker.prototype.ref() / unref() — no-op (single-process runtime).
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn worker_noop(_cx: *mut JSContext, _argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, _argc);
    args.rval().set(UndefinedValue());
    true
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Main-side receive primitive (worker → main)
// ---------------------------------------------------------------------------

/// Outcome of a non-blocking poll of a worker's main-thread inbox.
#[derive(Debug, PartialEq)]
pub enum WorkerIncoming {
    /// A data message was deserialized into the caller-provided `rval`
    /// (inside the current thread's realm).
    Data,
    /// The worker reported an error (message text included).
    Error(String),
    /// No message pending (or the worker is no longer registered).
    Empty,
}

/// Try to receive ONE pending worker → main message and, for data messages,
/// deserialize the structured-clone bytes into `rval` inside the current
/// thread's persistent realm. Non-blocking; call from the main JS thread
/// only. This is the primitive a `worker.onmessage` event-loop integration
/// builds on.
pub fn worker_try_recv(
    cx: &mut mozjs::context::JSContext,
    thread_id: u32,
    rval: mozjs::gc::MutableHandleValue<'_>,
) -> WorkerIncoming {
    // Take one message out of the channel while holding the registry guard
    // only for the try_recv (no JS runs under the DashMap borrow).
    let msg = {
        let handle = match worker_registry().get_mut(&thread_id) {
            Some(h) => h,
            None => return WorkerIncoming::Empty,
        };
        match handle.main_rx.as_ref() {
            Some(rx) => rx.lock().ok().and_then(|rx| rx.try_recv().ok()),
            None => return WorkerIncoming::Empty,
        }
    };

    match msg {
        Some(WorkerToMainMessage::Data(bytes)) => unsafe {
            // Objects created by the clone reader land in the current realm —
            // enter this thread's persistent realm, same as
            // deliver_message_to_worker does on the worker side.
            let global = match bao_engine::context::thread_realm_global() {
                Some(g) if !g.is_null() => g,
                _ => {
                    return WorkerIncoming::Error(
                        "worker_try_recv: main thread realm not initialized".into(),
                    )
                }
            };
            rooted!(&in(cx) let global_root = global);
            let mut realm = AutoRealm::new_from_handle(cx, global_root.handle());
            let realm_cx: &mut mozjs::context::JSContext = &mut realm;
            if sc_deserialize(realm_cx.raw_cx(), &bytes, rval) {
                WorkerIncoming::Data
            } else {
                WorkerIncoming::Error(
                    "worker_try_recv: structured-clone deserialization failed".into(),
                )
            }
        },
        Some(WorkerToMainMessage::Error(msg)) => WorkerIncoming::Error(msg),
        None => WorkerIncoming::Empty,
    }
}

// ---------------------------------------------------------------------------
// Module install
// ---------------------------------------------------------------------------

pub fn install(cx: &mut mozjs::context::JSContext) {
    let raw_cx = unsafe { cx.raw_cx() };

    // Build the module exports object natively.
    rooted!(&in(cx) let exports = unsafe { w2::JS_NewPlainObject(cx) });
    if exports.get().is_null() {
        return;
    }

    unsafe {
        // Worker constructor function.
        let worker_fn = JS_NewFunction(
            raw_cx,
            Some(worker_constructor),
            1,     // min args
            0x400, // JSFUN_CONSTRUCTOR
            c"Worker".as_ptr(),
        );
        if !worker_fn.is_null() {
            let fn_obj = JS_GetFunctionObject(worker_fn);
            rooted!(&in(cx) let fn_root = fn_obj);

            // Worker.prototype — plain object with methods.
            rooted!(&in(cx) let proto = w2::JS_NewPlainObject(cx));
            if !proto.get().is_null() {
                w2::JS_DefineFunction(
                    cx,
                    proto.handle(),
                    c"postMessage".as_ptr(),
                    Some(worker_post_message),
                    1,
                    JSPROP_ENUMERATE as u32,
                );
                w2::JS_DefineFunction(
                    cx,
                    proto.handle(),
                    c"terminate".as_ptr(),
                    Some(worker_terminate),
                    0,
                    JSPROP_ENUMERATE as u32,
                );
                w2::JS_DefineFunction(
                    cx,
                    proto.handle(),
                    c"ref".as_ptr(),
                    Some(worker_noop),
                    0,
                    JSPROP_ENUMERATE as u32,
                );
                w2::JS_DefineFunction(
                    cx,
                    proto.handle(),
                    c"unref".as_ptr(),
                    Some(worker_noop),
                    0,
                    JSPROP_ENUMERATE as u32,
                );

                // Wire prototype onto the constructor.
                rooted!(&in(cx) let proto_val = ObjectValue(proto.get()));
                JS_DefineProperty(
                    raw_cx,
                    fn_root.handle().into(),
                    c"prototype".as_ptr(),
                    proto_val.handle().into(),
                    0u32,
                );
            }

            // Export Worker on the module object.
            rooted!(&in(cx) let fn_val = ObjectValue(fn_root.get()));
            JS_DefineProperty(
                raw_cx,
                exports.handle().into(),
                c"Worker".as_ptr(),
                fn_val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }

        // MessageChannel — use globalThis.MessageChannel if available, otherwise in-process stub.
        // We evaluate a JS helper that returns the constructor.
        let source = r#"(function() {
  var MC = (typeof globalThis.MessageChannel === 'function')
    ? globalThis.MessageChannel
    : function MessageChannel() {
        var queue1 = [];
        var queue2 = [];
        var onmsg1 = null;
        var onmsg2 = null;
        this.port1 = {
          postMessage: function(data) {
            if (typeof onmsg2 === 'function') {
              onmsg2({ data: data });
            } else {
              queue2.push(data);
            }
          },
          get onmessage() { return onmsg1; },
          set onmessage(fn) {
            onmsg1 = fn;
            while (queue1.length > 0 && typeof fn === 'function') {
              fn({ data: queue1.shift() });
            }
          },
          close: function() {},
          start: function() {},
          addEventListener: function() {},
          removeEventListener: function() {},
        };
        this.port2 = {
          postMessage: function(data) {
            if (typeof onmsg1 === 'function') {
              onmsg1({ data: data });
            } else {
              queue1.push(data);
            }
          },
          get onmessage() { return onmsg2; },
          set onmessage(fn) {
            onmsg2 = fn;
            while (queue2.length > 0 && typeof fn === 'function') {
              fn({ data: queue2.shift() });
            }
          },
          close: function() {},
          start: function() {},
          addEventListener: function() {},
          removeEventListener: function() {},
        };
      };
  return MC;
})()"#;

        let mut source_text = mozjs::rust::transform_str_to_source_text(source);
        let mut rval = UndefinedValue();
        let rval_handle = MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rval,
        };
        let opts =
            mozjs::glue::NewCompileOptions(raw_cx, c"<worker_threads:MessageChannel>".as_ptr(), 1);
        if !opts.is_null() {
            let ok = mozjs_sys::jsapi::JS::Evaluate2(raw_cx, opts, &mut source_text, rval_handle);
            libc::free(opts as *mut _);
            if ok && rval.is_object() {
                let mc_ctor_fn = rval.to_object();
                rooted!(&in(cx) let mc_root = mc_ctor_fn);
                // Call the IIFE to get the actual constructor.
                rooted!(&in(cx) let mc_fn_val = ObjectValue(mc_ctor_fn));
                rooted!(&in(cx) let mut mc_result = UndefinedValue());
                JS_CallFunctionValue(
                    raw_cx,
                    mc_root.handle().into(),
                    mc_fn_val.handle().into(),
                    &HandleValueArray::empty(),
                    mc_result.handle_mut().into(),
                );
                // mc_result is the MessageChannel constructor.
                if mc_result.is_object() {
                    rooted!(&in(cx) let mc_val = ObjectValue(mc_result.to_object()));
                    JS_DefineProperty(
                        raw_cx,
                        exports.handle().into(),
                        c"MessageChannel".as_ptr(),
                        mc_val.handle().into(),
                        JSPROP_ENUMERATE as u32,
                    );
                }
            }
        }

        // MessagePort — delegate to globalThis.MessagePort or stub.
        let mp_source = r#"(typeof globalThis.MessagePort === 'function'
  ? globalThis.MessagePort
  : function MessagePort() {})"#;
        let mut mp_text = mozjs::rust::transform_str_to_source_text(mp_source);
        let mut mp_val = UndefinedValue();
        let mp_opts =
            mozjs::glue::NewCompileOptions(raw_cx, c"<worker_threads:MessagePort>".as_ptr(), 1);
        if !mp_opts.is_null() {
            let mp_ok = mozjs_sys::jsapi::JS::Evaluate2(
                raw_cx,
                mp_opts,
                &mut mp_text,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut mp_val,
                },
            );
            libc::free(mp_opts as *mut _);
            if mp_ok && mp_val.is_object() {
                rooted!(&in(cx) let mp_obj = ObjectValue(mp_val.to_object()));
                JS_DefineProperty(
                    raw_cx,
                    exports.handle().into(),
                    c"MessagePort".as_ptr(),
                    mp_obj.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }

        // BroadcastChannel — delegate to globalThis or stub.
        let bc_source = r#"(typeof globalThis.BroadcastChannel === 'function'
  ? globalThis.BroadcastChannel
  : function BroadcastChannel(name) { this.name = name; this.postMessage = function() {}; this.close = function() {}; })"#;
        let mut bc_text = mozjs::rust::transform_str_to_source_text(bc_source);
        let mut bc_val = UndefinedValue();
        let bc_opts = mozjs::glue::NewCompileOptions(
            raw_cx,
            c"<worker_threads:BroadcastChannel>".as_ptr(),
            1,
        );
        if !bc_opts.is_null() {
            let bc_ok = mozjs_sys::jsapi::JS::Evaluate2(
                raw_cx,
                bc_opts,
                &mut bc_text,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut bc_val,
                },
            );
            libc::free(bc_opts as *mut _);
            if bc_ok && bc_val.is_object() {
                rooted!(&in(cx) let bc_obj = ObjectValue(bc_val.to_object()));
                JS_DefineProperty(
                    raw_cx,
                    exports.handle().into(),
                    c"BroadcastChannel".as_ptr(),
                    bc_obj.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }

        // Static properties.
        rooted!(&in(cx) let true_val = BooleanValue(true));
        JS_DefineProperty(
            raw_cx,
            exports.handle().into(),
            c"isMainThread".as_ptr(),
            true_val.handle().into(),
            JSPROP_ENUMERATE as u32,
        );

        rooted!(&in(cx) let zero_val = Int32Value(0));
        JS_DefineProperty(
            raw_cx,
            exports.handle().into(),
            c"threadId".as_ptr(),
            zero_val.handle().into(),
            JSPROP_ENUMERATE as u32,
        );

        // workerData and parentPort are null/undefined on the main thread.
        rooted!(&in(cx) let undef_val = UndefinedValue());
        JS_DefineProperty(
            raw_cx,
            exports.handle().into(),
            c"workerData".as_ptr(),
            undef_val.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
        JS_DefineProperty(
            raw_cx,
            exports.handle().into(),
            c"parentPort".as_ptr(),
            undef_val.handle().into(),
            JSPROP_ENUMERATE as u32,
        );

        rooted!(&in(cx) let empty_obj = w2::JS_NewPlainObject(cx));
        rooted!(&in(cx) let empty_obj_val = ObjectValue(empty_obj.get()));
        JS_DefineProperty(
            raw_cx,
            exports.handle().into(),
            c"resourceLimits".as_ptr(),
            empty_obj_val.handle().into(),
            JSPROP_ENUMERATE as u32,
        );

        // SHARE_ENV symbol — create via JS eval.
        let share_env_source = r#"Symbol('nodejs.worker_threads.SHARE_ENV')"#;
        let mut se_text = mozjs::rust::transform_str_to_source_text(share_env_source);
        rooted!(&in(cx) let mut se_val = UndefinedValue());
        let se_opts =
            mozjs::glue::NewCompileOptions(raw_cx, c"<worker_threads:SHARE_ENV>".as_ptr(), 1);
        if !se_opts.is_null() {
            let se_ok = mozjs_sys::jsapi::JS::Evaluate2(
                raw_cx,
                se_opts,
                &mut se_text,
                se_val.handle_mut().into(),
            );
            libc::free(se_opts as *mut _);
            if se_ok {
                JS_DefineProperty(
                    raw_cx,
                    exports.handle().into(),
                    c"SHARE_ENV".as_ptr(),
                    se_val.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }

        // Utility functions (JS-evaluated for simplicity).
        let utils_source = r#"({
  getEnvironmentData: function() {},
  setEnvironmentData: function() {},
  getHeapSnapshot: function() { return {}; },
  markAsUntransferable: function() { throw new Error('markAsUntransferable is not implemented in Bao'); },
  moveMessagePortToContext: function() { throw new Error('moveMessagePortToContext is not implemented in Bao'); },
  receiveMessageOnPort: function() { return undefined; },
})"#;
        let mut ut_text = mozjs::rust::transform_str_to_source_text(utils_source);
        let mut ut_val = UndefinedValue();
        let ut_opts = mozjs::glue::NewCompileOptions(raw_cx, c"<worker_threads:utils>".as_ptr(), 1);
        if !ut_opts.is_null() {
            let ut_ok = mozjs_sys::jsapi::JS::Evaluate2(
                raw_cx,
                ut_opts,
                &mut ut_text,
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut ut_val,
                },
            );
            libc::free(ut_opts as *mut _);
            if ut_ok && ut_val.is_object() {
                let utils_obj = ut_val.to_object();
                rooted!(&in(cx) let utils_root = utils_obj);
                // Copy each property to exports.
                for name in &[
                    "getEnvironmentData",
                    "setEnvironmentData",
                    "getHeapSnapshot",
                    "markAsUntransferable",
                    "moveMessagePortToContext",
                    "receiveMessageOnPort",
                ] {
                    let c_name = CString::new(*name).unwrap_or_default();
                    rooted!(&in(cx) let mut prop_val = UndefinedValue());
                    JS_GetProperty(
                        raw_cx,
                        utils_root.handle().into(),
                        c_name.as_ptr(),
                        prop_val.handle_mut().into(),
                    );
                    if !prop_val.is_undefined() {
                        JS_DefineProperty(
                            raw_cx,
                            exports.handle().into(),
                            c_name.as_ptr(),
                            prop_val.handle().into(),
                            JSPROP_ENUMERATE as u32,
                        );
                    }
                }
            }
        }
    }

    cache_builtin(cx, "worker_threads", exports.get());
}
