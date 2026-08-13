// @trace REQ-ENG-006 [api:node:worker_threads]
//
// Node.js `worker_threads` builtin module — real OS-thread Workers.
//
// Architecture:
//   - SpiderMonkey's JSEngine is process-global (OnceLock<JSEngineHandle>).
//   - Each Worker spawns a std::thread that calls Runtime::new(handle) to get
//     its own thread-local JSContext. No cross-thread JSObject sharing.
//   - Messages are serialized as JSON strings via mpsc channels.
//   - Worker JS objects (postMessage/terminate/threadId) are native host fns.

use ::std::cell::RefCell;
use ::std::ffi::CString;
use ::std::ptr::NonNull;
use ::std::sync::OnceLock;
use ::std::sync::atomic::{AtomicU32, Ordering};
use ::std::sync::mpsc::{self, Receiver, Sender};

use dashmap::DashMap;
use mozjs::conversions::unsafe_jsstr_to_string;
use mozjs::jsapi::*;
use mozjs::jsval::{
    BooleanValue, DoubleValue, Int32Value, JSVal, ObjectValue, StringValue, UndefinedValue,
};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

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
}

// ---------------------------------------------------------------------------
// Cross-thread messages
// ---------------------------------------------------------------------------

enum WorkerMessage {
    /// JSON-serialized data from main → worker.
    Data(String),
    /// Signal the worker thread to exit.
    Terminate,
}

/// Messages from worker thread → main thread.
#[allow(dead_code)]
enum WorkerToMainMessage {
    /// JSON-serialized data.
    Data(String),
    /// Error message.
    Error(String),
}

// ---------------------------------------------------------------------------
// Worker thread entry point
// ---------------------------------------------------------------------------

fn worker_entry(
    filename: String,
    thread_id: u32,
    receiver: Receiver<WorkerMessage>,
    main_sender: Sender<WorkerToMainMessage>,
    worker_data_json: Option<String>,
) {
    // 1. Obtain process-global JSEngine handle.
    let engine_handle = match bao_engine::context::ensure_engine_handle() {
        Ok(h) => h,
        Err(_) => return,
    };

    // 2. Create a new Runtime on this thread — gets its own JSContext.
    let _runtime = mozjs::rust::Runtime::new(engine_handle);

    let cx_ptr = match mozjs::rust::Runtime::get() {
        Some(p) => p,
        None => return,
    };

    // Safety: Runtime::get() returned a valid JSContext pointer for this thread.
    let mut cx = unsafe { mozjs::context::JSContext::from_ptr(cx_ptr) };
    let raw_cx = unsafe { cx.raw_cx() };

    // 3. Init JobQueue + ModuleLoader on this thread's JSContext.
    if !bao_engine::job_queue::JobQueue::init(&cx) {
        return;
    }
    bao_engine::module_loader::ModuleLoader::init_thread_local(&cx);
    bao_engine::module_loader::set_job_queue_drain(bao_engine::job_queue::JobQueue::drain);

    // 4. Read the worker script from disk.
    let source = match ::std::fs::read_to_string(&filename) {
        Ok(s) => s,
        Err(e) => {
            let msg = format!("Worker: failed to read '{}': {}", filename, e);
            let _ = main_sender.send(WorkerToMainMessage::Error(msg));
            return;
        }
    };

    // 5. Build a bootstrap script that sets up self.onmessage / self.postMessage
    //    then evaluates the worker source.
    //
    //    The worker script can use:
    //      - self.onmessage = function(e) { ... }  (e.data = parsed JSON)
    //      - self.postMessage(data)  (serializes to JSON, sends to main thread)
    //      - workerData (from options)
    let worker_data_decl = match &worker_data_json {
        Some(json) => format!("var workerData = JSON.parse({});", escape_js_string(json)),
        None => "var workerData = null;".to_string(),
    };

    let bootstrap = format!(
        r#"(function() {{
  {worker_data_decl}
  var __pendingMessages = [];

  // self.postMessage — serialize data to JSON, send to main thread
  self.postMessage = function(data) {{
    try {{
      var json = JSON.stringify(data);
      __baoPostToMain(json);
    }} catch(e) {{
      // silently fail on non-serializable data
    }}
  }};

  // Queue messages until onmessage handler is set
  self.__baoDeliverMessage = function(jsonStr) {{
    var data = JSON.parse(jsonStr);
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
        worker_data_decl = worker_data_decl,
        thread_id = thread_id,
        source = source,
    );

    // 6. Evaluate the bootstrap script.
    let eval_result = bao_engine::module_loader::ModuleLoader::eval_module(
        &mut cx,
        &bootstrap,
        &filename,
        Some(worker_global_setup),
        None,
    );

    if let Err(e) = eval_result {
        let msg = format!(
            "Worker script error: {} ({}:{})",
            e.message, e.filename, e.line
        );
        let _ = main_sender.send(WorkerToMainMessage::Error(msg));
        return;
    }

    // 7. Drain the job queue (process any microtasks from the script).
    bao_engine::job_queue::JobQueue::drain(&mut cx);

    // 8. Store the main_sender in TLS for __baoPostToMain to access.
    WORKER_MAIN_SENDER.with(|s| {
        *s.borrow_mut() = Some(main_sender);
    });

    // 9. Message receive loop: wait for messages from main thread.
    loop {
        match receiver.recv() {
            Ok(WorkerMessage::Data(json_str)) => {
                // Call self.__baoDeliverMessage(jsonStr) on the worker's global.
                deliver_message_to_worker(raw_cx, &json_str);
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
    // Install __baoPostToMain(dataJsonStr) on the global object.
    // This is called by self.postMessage() to send data to the main thread.
    w2::JS_DefineFunction(
        cx,
        global,
        c"__baoPostToMain".as_ptr(),
        Some(worker_post_to_main),
        1,
        JSPROP_ENUMERATE as u32,
    );
}

/// Native function: __baoPostToMain(jsonStr) — called from worker JS to post
/// a message to the main thread. The argument is already a JSON string.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn worker_post_to_main(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    if argc == 0 {
        JS_ReportErrorUTF8(cx, c"__baoPostToMain requires a string argument".as_ptr());
        return false;
    }

    let json_val = *args.get(0).ptr;
    if !json_val.is_string() {
        JS_ReportErrorUTF8(cx, c"__baoPostToMain argument must be a string".as_ptr());
        return false;
    }

    let mut wrapped_cx = unsafe { mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx)) };
    let rust_str = unsafe_jsstr_to_string(
        wrapped_cx.raw_cx(),
        NonNull::new_unchecked(json_val.to_string()),
    );

    // Find this worker's main-sender from a thread-local.
    WORKER_MAIN_SENDER.with(|sender| {
        if let Some(tx) = sender.borrow().as_ref() {
            let _ = tx.send(WorkerToMainMessage::Data(rust_str));
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

/// Call self.__baoDeliverMessage(jsonStr) in the worker's JSContext.
fn deliver_message_to_worker(raw_cx: *mut JSContext, json_str: &str) {
    unsafe {
        let global = CurrentGlobalOrNull(raw_cx);
        if global.is_null() {
            return;
        }

        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(raw_cx));
        let cx = &mut wrapped_cx;

        rooted!(&in(cx) let global_root = global);
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

        // Create JS string from the JSON string.
        let c_str = CString::new(json_str).unwrap_or_default();
        rooted!(&in(cx) let js_str = JS_NewStringCopyZ(raw_cx, c_str.as_ptr()));
        if js_str.get().is_null() {
            return;
        }
        rooted!(&in(cx) let json_str_val = StringValue(&*js_str.get()));

        let call_args_elements = [json_str_val.get()];
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
    let mut worker_data_json: Option<String> = None;
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
                // Serialize workerData to JSON using JSON.stringify.
                rooted!(&in(cx_ref) let mut json_val = UndefinedValue());
                let json_ok = json_stringify(cx, wd_val, json_val.handle_mut());
                if json_ok && json_val.is_string() {
                    worker_data_json = Some(unsafe_jsstr_to_string(
                        cx_ref.raw_cx(),
                        NonNull::new_unchecked(json_val.get().to_string()),
                    ));
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
    let worker_wd_json = worker_data_json.clone();
    let join_handle = ::std::thread::Builder::new()
        .name(format!("bao-worker-{}", thread_id))
        .spawn(move || {
            worker_entry(
                worker_filename,
                thread_id,
                main_to_worker_rx,
                worker_to_main_tx,
                worker_wd_json,
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

    // Register the worker handle.
    worker_registry().insert(
        thread_id,
        WorkerHandle {
            sender: main_to_worker_tx,
            thread: Some(join_handle),
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

    // Store the worker_to_main receiver as a private property so the main thread
    // can poll it for incoming messages.
    // We box the receiver and store the raw pointer (leaked, cleaned up on terminate).
    let rx_box = Box::new(worker_to_main_rx);
    let rx_ptr = Box::into_raw(rx_box) as u64;
    rooted!(&in(cx_ref) let rx_val = DoubleValue(rx_ptr as f64));
    JS_DefineProperty(
        cx,
        worker_obj.handle().into(),
        c"__mainRx".as_ptr(),
        rx_val.handle().into(),
        0u32,
    );

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

    // Serialize the argument to JSON.
    let json_str = if argc > 0 {
        let data_val = *args.get(0).ptr;
        rooted!(&in(cx_ref) let mut json_val = UndefinedValue());
        let ok = json_stringify(cx, data_val, json_val.handle_mut());
        if ok && json_val.is_string() {
            unsafe_jsstr_to_string(
                cx_ref.raw_cx(),
                NonNull::new_unchecked(json_val.get().to_string()),
            )
        } else {
            "null".to_string()
        }
    } else {
        "null".to_string()
    };

    // Send to the worker thread.
    if let Some(handle) = worker_registry().get_mut(&thread_id) {
        let _ = handle.sender.send(WorkerMessage::Data(json_str));
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

    // Clean up the mainRx boxed receiver.
    let mut rx_val = UndefinedValue();
    JS_GetProperty(
        cx,
        this_root.handle().into(),
        c"__mainRx".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut rx_val,
        },
    );
    if rx_val.is_double() {
        let rx_ptr = rx_val.to_double() as u64 as *mut Receiver<WorkerToMainMessage>;
        if !rx_ptr.is_null() {
            drop(Box::from_raw(rx_ptr));
        }
    }

    // Remove from registry and join the thread.
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

/// JSON.stringify a JS value. Returns true on success, false on failure.
fn json_stringify(
    cx: *mut JSContext,
    value: JSVal,
    out: mozjs::rust::MutableHandle<Value>,
) -> bool {
    unsafe {
        let global = CurrentGlobalOrNull(cx);
        if global.is_null() {
            return false;
        }

        let mut wrapped_cx = mozjs::context::JSContext::from_ptr(NonNull::new_unchecked(cx));
        let cx_ref = &mut wrapped_cx;

        // Get JSON object from global.
        rooted!(&in(cx_ref) let global_root = global);
        rooted!(&in(cx_ref) let mut json_obj_val = UndefinedValue());
        JS_GetProperty(
            cx,
            global_root.handle().into(),
            c"JSON".as_ptr(),
            json_obj_val.handle_mut().into(),
        );
        if !json_obj_val.is_object() {
            return false;
        }

        rooted!(&in(cx_ref) let json_obj = json_obj_val.to_object());

        rooted!(&in(cx_ref) let mut stringify_val = UndefinedValue());
        JS_GetProperty(
            cx,
            json_obj.handle().into(),
            c"stringify".as_ptr(),
            stringify_val.handle_mut().into(),
        );
        if !stringify_val.is_object() {
            return false;
        }

        rooted!(&in(cx_ref) let stringify_fn = stringify_val.to_object());
        rooted!(&in(cx_ref) let arg_val = value);

        let call_args_elements = [arg_val.get()];
        let call_args = HandleValueArray {
            length_: 1,
            elements_: call_args_elements.as_ptr() as *const Value,
        };

        rooted!(&in(cx_ref) let fn_val = ObjectValue(stringify_fn.get()));
        let raw_out: mozjs::jsapi::MutableHandle<Value> = out.into();
        JS_CallFunctionValue(
            cx,
            json_obj.handle().into(),
            fn_val.handle().into(),
            &call_args,
            raw_out,
        );

        raw_out.get().is_string()
    }
}

/// Escape a string for embedding inside JS single-quoted string literal.
fn escape_js_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
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
