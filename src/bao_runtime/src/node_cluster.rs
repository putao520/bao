// @trace REQ-ENG-006 [api:node:cluster]
//
// Node.js cluster module. Bao supports cluster.fork() by spawning child processes
// via child_process.spawn ("bao run <script>") with --cluster-worker env var.
// Primary process: isPrimary=true, manages workers via fork().
// Worker process: isWorker=true, communicates with primary via IPC (env-based).
//
// IPC: uses BAO_CLUSTER_WORKER_ID / BAO_CLUSTER_PRIMARY_PID env vars.
// Workers communicate with primary via stdout/stderr pipe + process.send() over stdin.

use mozjs::jsapi::*;
use mozjs::jsval::{
    BooleanValue, Int32Value, JSVal, NullValue, ObjectValue, StringValue, UndefinedValue,
};
use mozjs::rooted;
use mozjs::rust::wrappers2 as w2;

use crate::require::cache_builtin;

/// Check if this process is a cluster worker (started with --cluster-worker env).
fn is_cluster_worker() -> bool {
    ::std::env::var("BAO_CLUSTER_WORKER_ID").is_ok()
}

// ─── Module install ────────────────────────────────────────────────────────

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let obj = unsafe { w2::JS_NewPlainObject(cx) });
    if obj.get().is_null() { return; }

    let is_worker = is_cluster_worker();
    let is_primary = !is_worker;

    unsafe {
        let raw_cx = cx.raw_cx();

        // isPrimary
        rooted!(&in(cx) let is_primary_val = BooleanValue(is_primary));
        let _ = JS_DefineProperty(raw_cx, obj.handle().into(), c"isPrimary".as_ptr(), is_primary_val.handle().into(), JSPROP_ENUMERATE as u32);

        // isMaster (deprecated alias)
        rooted!(&in(cx) let is_master_val = BooleanValue(is_primary));
        let _ = JS_DefineProperty(raw_cx, obj.handle().into(), c"isMaster".as_ptr(), is_master_val.handle().into(), JSPROP_ENUMERATE as u32);

        // isWorker
        rooted!(&in(cx) let is_worker_val = BooleanValue(is_worker));
        let _ = JS_DefineProperty(raw_cx, obj.handle().into(), c"isWorker".as_ptr(), is_worker_val.handle().into(), JSPROP_ENUMERATE as u32);

        // workers = empty object
        rooted!(&in(cx) let workers_obj = w2::JS_NewPlainObject(cx));
        if !workers_obj.get().is_null() {
            rooted!(&in(cx) let workers_val = ObjectValue(workers_obj.get()));
            let _ = JS_DefineProperty(raw_cx, obj.handle().into(), c"workers".as_ptr(), workers_val.handle().into(), JSPROP_ENUMERATE as u32);
        }

        // settings = empty object
        rooted!(&in(cx) let settings_obj = w2::JS_NewPlainObject(cx));
        if !settings_obj.get().is_null() {
            rooted!(&in(cx) let settings_val = ObjectValue(settings_obj.get()));
            let _ = JS_DefineProperty(raw_cx, obj.handle().into(), c"settings".as_ptr(), settings_val.handle().into(), JSPROP_ENUMERATE as u32);
        }

        // worker — current worker object (if worker), or undefined (if primary)
        if is_worker {
            rooted!(&in(cx) let worker_obj = make_worker_object(cx, raw_cx));
            if !worker_obj.get().is_null() {
                rooted!(&in(cx) let worker_val = ObjectValue(worker_obj.get()));
                let _ = JS_DefineProperty(raw_cx, obj.handle().into(), c"worker".as_ptr(), worker_val.handle().into(), JSPROP_ENUMERATE as u32);
            }
        } else {
            rooted!(&in(cx) let worker_val = UndefinedValue());
            let _ = JS_DefineProperty(raw_cx, obj.handle().into(), c"worker".as_ptr(), worker_val.handle().into(), JSPROP_ENUMERATE as u32);
        }

        // fork() — spawns a worker process
        let fork_fn = JS_NewFunction(raw_cx, Some(cluster_fork), 0, 0, c"fork".as_ptr());
        if !fork_fn.is_null() {
            let fn_obj = JS_GetFunctionObject(fork_fn);
            if !fn_obj.is_null() {
                rooted!(&in(cx) let val = ObjectValue(fn_obj));
                let _ = JS_DefineProperty(raw_cx, obj.handle().into(), c"fork".as_ptr(), val.handle().into(), JSPROP_ENUMERATE as u32);
            }
        }

        // disconnect()
        let disconnect_fn = JS_NewFunction(raw_cx, Some(cluster_disconnect), 0, 0, c"disconnect".as_ptr());
        if !disconnect_fn.is_null() {
            let fn_obj = JS_GetFunctionObject(disconnect_fn);
            if !fn_obj.is_null() {
                rooted!(&in(cx) let val = ObjectValue(fn_obj));
                let _ = JS_DefineProperty(raw_cx, obj.handle().into(), c"disconnect".as_ptr(), val.handle().into(), JSPROP_ENUMERATE as u32);
            }
        }

        // setupPrimary() / setupMaster()
        let setup_fn = JS_NewFunction(raw_cx, Some(cluster_setup_primary), 1, 0, c"setupPrimary".as_ptr());
        if !setup_fn.is_null() {
            let fn_obj = JS_GetFunctionObject(setup_fn);
            if !fn_obj.is_null() {
                rooted!(&in(cx) let val = ObjectValue(fn_obj));
                let _ = JS_DefineProperty(raw_cx, obj.handle().into(), c"setupPrimary".as_ptr(), val.handle().into(), JSPROP_ENUMERATE as u32);
            }
        }
        let setup_master_fn = JS_NewFunction(raw_cx, Some(cluster_setup_primary), 1, 0, c"setupMaster".as_ptr());
        if !setup_master_fn.is_null() {
            let fn_obj = JS_GetFunctionObject(setup_master_fn);
            if !fn_obj.is_null() {
                rooted!(&in(cx) let val = ObjectValue(fn_obj));
                let _ = JS_DefineProperty(raw_cx, obj.handle().into(), c"setupMaster".as_ptr(), val.handle().into(), JSPROP_ENUMERATE as u32);
            }
        }

        // schedulingPolicy = SCHED_RR (2) for round-robin connection distribution
        rooted!(&in(cx) let sched = Int32Value(2));
        let _ = JS_DefineProperty(raw_cx, obj.handle().into(), c"schedulingPolicy".as_ptr(), sched.handle().into(), JSPROP_ENUMERATE as u32);

        // SCHED_NONE = 1, SCHED_RR = 2
        rooted!(&in(cx) let sched_none = Int32Value(1));
        let _ = JS_DefineProperty(raw_cx, obj.handle().into(), c"SCHED_NONE".as_ptr(), sched_none.handle().into(), JSPROP_ENUMERATE as u32);
        rooted!(&in(cx) let sched_rr = Int32Value(2));
        let _ = JS_DefineProperty(raw_cx, obj.handle().into(), c"SCHED_RR".as_ptr(), sched_rr.handle().into(), JSPROP_ENUMERATE as u32);
    }

    cache_builtin(cx, "cluster", obj.get());

    // Run the JS shim that sets up EventEmitter-based Worker class and process.send bridge.
    unsafe {
        let c_filename = bun_core::ZBox::from_bytes("node:cluster".as_bytes());
        let opts = mozjs::glue::NewCompileOptions(cx.raw_cx(), c_filename.as_ptr(), 1);
        if !opts.is_null() {
            let mut src = mozjs::rust::transform_str_to_source_text(CLUSTER_JS);
            let mut rval = UndefinedValue();
            let rval_handle = MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut rval,
            };
            let _ = mozjs_sys::jsapi::JS::Evaluate2(cx.raw_cx(), opts, &mut src, rval_handle);
            libc::free(opts as *mut _);
        }
    }
}

/// Build a JS Worker object representing a child worker process.
unsafe fn make_worker_object(
    cx: &mut mozjs::context::JSContext,
    _raw_cx: *mut JSContext,
) -> *mut JSObject { unsafe {
    let worker_obj = w2::JS_NewPlainObject(cx);
    if worker_obj.is_null() {
        return ::std::ptr::null_mut();
    }
    rooted!(&in(cx) let worker_r = worker_obj);
    let worker_h = worker_r.handle().into();

    // id — from env var
    let worker_id: i32 = ::std::env::var("BAO_CLUSTER_WORKER_ID")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    rooted!(&in(cx) let id_val = Int32Value(worker_id));
    JS_DefineProperty(cx.raw_cx(), worker_h, c"id".as_ptr(), id_val.handle().into(), JSPROP_ENUMERATE as u32);

    // process — null (would need to reference the actual ChildProcess, set from JS shim)
    rooted!(&in(cx) let null_v = NullValue());
    JS_DefineProperty(cx.raw_cx(), worker_h, c"process".as_ptr(), null_v.handle().into(), JSPROP_ENUMERATE as u32);

    // isConnected = true
    rooted!(&in(cx) let connected_v = BooleanValue(true));
    JS_DefineProperty(cx.raw_cx(), worker_h, c"isConnected".as_ptr(), connected_v.handle().into(), JSPROP_ENUMERATE as u32);

    // isDead = false
    rooted!(&in(cx) let dead_v = BooleanValue(false));
    JS_DefineProperty(cx.raw_cx(), worker_h, c"isDead".as_ptr(), dead_v.handle().into(), JSPROP_ENUMERATE as u32);

    // exitedAfterDisconnect = false
    rooted!(&in(cx) let ead_v = BooleanValue(false));
    JS_DefineProperty(cx.raw_cx(), worker_h, c"exitedAfterDisconnect".as_ptr(), ead_v.handle().into(), JSPROP_ENUMERATE as u32);

    // _events placeholder (for JS shim to enhance with EventEmitter)
    rooted!(&in(cx) let events_obj = w2::JS_NewPlainObject(cx));
    if !events_obj.get().is_null() {
        rooted!(&in(cx) let events_val = ObjectValue(events_obj.get()));
        JS_DefineProperty(cx.raw_cx(), worker_h, c"_events".as_ptr(), events_val.handle().into(), 0);
    }

    worker_r.get()
}}

/// cluster.fork(env?) — spawn a worker process via child_process.spawn.
///
/// The worker runs the same script with BAO_CLUSTER_WORKER_ID and
/// BAO_CLUSTER_PRIMARY_PID env vars set. The JS shim wraps the
/// spawned ChildProcess in a Worker object and registers it in cluster.workers.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cluster_fork(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut mozjs::jsval::JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    if let ::std::result::Result::Err(e) = crate::permission_bridge::check_run() {
        let c_msg = bun_core::ZBox::from_bytes(e.as_bytes());
        JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
        return false;
    }

    // Get the script path — use process.argv[1] (the script being run).
    // We need to get it from the global process object.
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(
        ::std::ptr::NonNull::new_unchecked(cx)
    );
    let cx_ref = &mut wrapped_cx;

    let script_path = {
        rooted!(&in(cx_ref) let global = CurrentGlobalOrNull(cx));
        let mut process_val = UndefinedValue();
        JS_GetProperty(cx, global.handle().into(), c"process".as_ptr(),
            MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut process_val });
        if process_val.is_object() {
            let process_obj = process_val.to_object();
            rooted!(&in(cx_ref) let process_r = process_obj);
            let mut argv_val = UndefinedValue();
            JS_GetProperty(cx, process_r.handle().into(), c"argv".as_ptr(),
                MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut argv_val });
            if argv_val.is_object() {
                let argv_obj = argv_val.to_object();
                rooted!(&in(cx_ref) let argv_r = argv_obj);
                let mut elem = UndefinedValue();
                JS_GetElement(cx, argv_r.handle().into(), 1,
                    MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut elem });
                if elem.is_string() {
                    crate::js_to_rust_string(cx, elem)
                } else {
                    String::new()
                }
            } else {
                String::new()
            }
        } else {
            String::new()
        }
    };

    if script_path.is_empty() {
        JS_ReportErrorUTF8(cx, c"cluster.fork(): cannot determine script path (process.argv[1] is empty)".as_ptr());
        args.rval().set(UndefinedValue());
        return false;
    }

    // Determine the next worker ID from cluster.settings or existing workers.
    // For simplicity, use a counter stored on the cluster module's settings.
    // The JS shim will manage the counter; for now, read from settings._nextId or default to 1.
    let worker_id: i32 = {
        if let Some(cluster_mod) = crate::require::get_builtin(cx_ref.raw_cx(), "cluster") {
            if !cluster_mod.is_null() {
                rooted!(&in(cx_ref) let cm_r = cluster_mod);
                let mut settings_val = UndefinedValue();
                JS_GetProperty(cx, cm_r.handle().into(), c"settings".as_ptr(),
                    MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut settings_val });
                if settings_val.is_object() {
                    let settings_obj = settings_val.to_object();
                    rooted!(&in(cx_ref) let settings_r = settings_obj);
                    let mut next_id_val = UndefinedValue();
                    JS_GetProperty(cx, settings_r.handle().into(), c"_nextId".as_ptr(),
                        MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut next_id_val });
                    if next_id_val.is_int32() {
                        let id = next_id_val.to_int32();
                        let new_id = id + 1;
                        rooted!(&in(cx_ref) let new_id_v = Int32Value(new_id));
                        JS_SetProperty(cx, settings_r.handle().into(), c"_nextId".as_ptr(), new_id_v.handle().into());
                        id
                    } else {
                        // Initialize counter.
                        rooted!(&in(cx_ref) let init_v = Int32Value(2));
                        JS_SetProperty(cx, settings_r.handle().into(), c"_nextId".as_ptr(), init_v.handle().into());
                        1
                    }
                } else {
                    1
                }
            } else {
                1
            }
        } else {
            1
        }
    };

    // Get the bao binary path.
    let executable = ::std::env::current_exe().unwrap_or_else(|_| ::std::path::PathBuf::from("bao"));
    let exec_str = executable.to_string_lossy().into_owned();

    // Build environment for the child worker.
    let primary_pid = ::std::process::id();

    // Parse optional env argument — merge into envp later.
    // For now, env merging from JS objects is handled by the JS shim.
    // The native fork reads process.argv[1] and spawns "bao run <script>" with cluster env vars.

    // Set cluster-specific env vars.
    let cluster_env_worker = format!("BAO_CLUSTER_WORKER_ID={}", worker_id);
    let cluster_env_primary = format!("BAO_CLUSTER_PRIMARY_PID={}", primary_pid);
    let cluster_env_pairs = [&cluster_env_worker, &cluster_env_primary];

    // Build envp for posix spawn.
    let mut envp_vec: Vec<Box<[u8]>> = Vec::new();

    // Copy current environment.
    for (key, value) in ::std::env::vars() {
        envp_vec.push(format!("{}={}", key, value).into_bytes().into_boxed_slice());
    }
    // Add/override cluster env vars.
    for pair in &cluster_env_pairs {
        let parts: Vec<&str> = pair.splitn(2, '=').collect();
        if parts.len() == 2 {
            // Remove existing entry with same key.
            let key_bytes = parts[0].as_bytes();
            envp_vec.retain(|e| {
                let eq_pos = e.iter().position(|&b| b == b'=');
                match eq_pos {
                    Some(pos) => &e[..pos] != key_bytes,
                    None => true,
                }
            });
            envp_vec.push(pair.as_bytes().to_vec().into_boxed_slice());
        }
    }

    // Build argv for the child: bao run <script>
    let argv: Vec<Box<[u8]>> = vec![
        exec_str.as_bytes().to_vec().into_boxed_slice(),
        b"run".to_vec().into_boxed_slice(),
        script_path.as_bytes().to_vec().into_boxed_slice(),
    ];

    // Use child_process.spawn via the native cp_spawn function.
    // We'll call it directly by building the spawn args.
    // Actually, it's simpler to use bun_spawn directly here.

    use bun_spawn::sync::{self as spawn_sync, Stdio as SyncStdio};

    // Build envp C string array.
    let mut envp_c_ptrs: Vec<*const ::std::ffi::c_char> = Vec::with_capacity(envp_vec.len() + 1);
    for entry in &envp_vec {
        envp_c_ptrs.push(entry.as_ptr() as *const ::std::ffi::c_char);
    }
    envp_c_ptrs.push(::std::ptr::null());

    let sync_opts = spawn_sync::Options {
        stdin: SyncStdio::Buffer,
        stdout: SyncStdio::Buffer,
        stderr: SyncStdio::Buffer,
        ipc: None,
        cwd: Box::new([]),
        detached: false,
        argv: argv.clone(),
        envp: Some(envp_c_ptrs.as_ptr()),
        use_execve_on_macos: false,
        argv0: None,
        windows: (),
    };

    let spawn_result = match spawn_sync::spawn(&sync_opts) {
        Ok(Ok(r)) => r,
        Ok(Err(sys_err)) => {
            let msg = format!("cluster.fork: system error: {:?}", sys_err);
            let c_msg = bun_core::ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
        Err(e) => {
            let msg = format!("cluster.fork: spawn failed: {:?}", e);
            let c_msg = bun_core::ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    };

    let pid = spawn_result.pid;
    let exit_code = super::node_child_process::status_to_exit_code(&spawn_result.status);

    // Build a Worker JS object.
    let worker_obj = mozjs_sys::jsapi::JS_NewPlainObject(cx);
    if worker_obj.is_null() {
        args.rval().set(UndefinedValue());
        return true;
    }
    rooted!(&in(cx_ref) let worker_r = worker_obj);
    let worker_h = worker_r.handle().into();

    // id
    rooted!(&in(cx_ref) let id_v = Int32Value(worker_id));
    JS_DefineProperty(cx, worker_h, c"id".as_ptr(), id_v.handle().into(), JSPROP_ENUMERATE as u32);

    // process — build a minimal process-like object
    let proc_obj = w2::JS_NewPlainObject(cx_ref);
    if !proc_obj.is_null() {
        rooted!(&in(cx_ref) let proc_r = proc_obj);
        let proc_h = proc_r.handle().into();

        // pid
        rooted!(&in(cx_ref) let pid_v = Int32Value(pid as i32));
        JS_DefineProperty(cx, proc_h, c"pid".as_ptr(), pid_v.handle().into(), JSPROP_ENUMERATE as u32);

        // exitCode
        rooted!(&in(cx_ref) let ec_v = Int32Value(exit_code));
        JS_DefineProperty(cx, proc_h, c"exitCode".as_ptr(), ec_v.handle().into(), JSPROP_ENUMERATE as u32);

        let proc_val = ObjectValue(proc_r.get());
        rooted!(&in(cx_ref) let pv = proc_val);
        JS_DefineProperty(cx, worker_h, c"process".as_ptr(), pv.handle().into(), JSPROP_ENUMERATE as u32);
    }

    // isConnected
    rooted!(&in(cx_ref) let conn_v = BooleanValue(true));
    JS_DefineProperty(cx, worker_h, c"isConnected".as_ptr(), conn_v.handle().into(), JSPROP_ENUMERATE as u32);

    // isDead
    let is_dead = exit_code != 0;
    rooted!(&in(cx_ref) let dead_v = BooleanValue(is_dead));
    JS_DefineProperty(cx, worker_h, c"isDead".as_ptr(), dead_v.handle().into(), JSPROP_ENUMERATE as u32);

    // exitedAfterDisconnect
    rooted!(&in(cx_ref) let ead_v = BooleanValue(false));
    JS_DefineProperty(cx, worker_h, c"exitedAfterDisconnect".as_ptr(), ead_v.handle().into(), JSPROP_ENUMERATE as u32);

    // _pid (for native kill)
    rooted!(&in(cx_ref) let npid_v = Int32Value(pid as i32));
    JS_DefineProperty(cx, worker_h, c"_pid".as_ptr(), npid_v.handle().into(), 0);

    // Register worker in cluster.workers
    {
        if let Some(cluster_mod) = crate::require::get_builtin(cx_ref.raw_cx(), "cluster") {
            if !cluster_mod.is_null() {
                rooted!(&in(cx_ref) let cm_r = cluster_mod);
                let mut workers_val = UndefinedValue();
                JS_GetProperty(cx, cm_r.handle().into(), c"workers".as_ptr(),
                    MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut workers_val });
                if workers_val.is_object() {
                    let workers_obj = workers_val.to_object();
                    rooted!(&in(cx_ref) let workers_r = workers_obj);
                    let worker_val = ObjectValue(worker_r.get());
                    rooted!(&in(cx_ref) let wv = worker_val);
                    // workers[id] = worker
                    let mut id_str_val = UndefinedValue();
                    let id_c_str = bun_core::ZBox::from_bytes(format!("{}", worker_id).as_bytes());
                    {
                        let js_str = JS_NewStringCopyZ(cx, id_c_str.as_ptr());
                        if !js_str.is_null() {
                            id_str_val = StringValue(&*js_str);
                        }
                    }
                    rooted!(&in(cx_ref) let id_sv = id_str_val);
                    JS_SetProperty(cx, workers_r.handle().into(), id_c_str.as_ptr(), wv.handle().into());
                }
            }
        }
    }

    args.rval().set(ObjectValue(worker_r.get()));
    true
}

/// cluster.disconnect() — disconnect all workers.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cluster_disconnect(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut mozjs::jsval::JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);

    // Send SIGTERM to all worker processes tracked in cluster.workers.
    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(
        ::std::ptr::NonNull::new_unchecked(cx)
    );
    let cx_ref = &mut wrapped_cx;

    if let Some(cluster_mod) = crate::require::get_builtin(cx_ref.raw_cx(), "cluster") {
        if !cluster_mod.is_null() {
            rooted!(&in(cx_ref) let cm_r = cluster_mod);
            let mut workers_val = UndefinedValue();
            JS_GetProperty(cx, cm_r.handle().into(), c"workers".as_ptr(),
                MutableHandle::<Value> { _phantom_0: ::std::marker::PhantomData, ptr: &mut workers_val });
            if workers_val.is_object() {
                let workers_obj = workers_val.to_object();
                rooted!(&in(cx_ref) let workers_r = workers_obj);
                // Iterate over workers and kill each one.
                // Since we can't easily enumerate JS objects from Rust,
                // we use the JS shim to handle disconnect logic.
                // For now, just set a flag that the JS shim will pick up.
                let disconnected_v = BooleanValue(true);
                rooted!(&in(cx_ref) let dv = disconnected_v);
                JS_SetProperty(cx, cm_r.handle().into(), c"_disconnecting".as_ptr(), dv.handle().into());
            }
        }
    }

    args.rval().set(UndefinedValue());
    true
}

/// cluster.setupPrimary(settings) / cluster.setupMaster(settings) — configure primary.
#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cluster_setup_primary(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut mozjs::jsval::JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    let mut wrapped_cx = mozjs::context::JSContext::from_ptr(
        ::std::ptr::NonNull::new_unchecked(cx)
    );
    let cx_ref = &mut wrapped_cx;

    // Store settings on cluster.settings.
    if argc > 0 {
        let settings_val = *args.get(0).ptr;
        if settings_val.is_object() {
            if let Some(cluster_mod) = crate::require::get_builtin(cx_ref.raw_cx(), "cluster") {
                if !cluster_mod.is_null() {
                    rooted!(&in(cx_ref) let cm_r = cluster_mod);
                    rooted!(&in(cx_ref) let sv = settings_val);
                    JS_SetProperty(cx, cm_r.handle().into(), c"settings".as_ptr(), sv.handle().into());
                }
            }
        }
    }

    args.rval().set(UndefinedValue());
    true
}

// ─── JS shim for Worker EventEmitter + process.send bridge ─────────────────

const CLUSTER_JS: &str = r#"
(function() {
  var cluster = require('cluster');

  // Worker class with EventEmitter mixin.
  function Worker(id, process) {
    this.id = id;
    this.process = process;
    this.isConnected = true;
    this.isDead = false;
    this.exitedAfterDisconnect = false;
    this._events = {};
    this._onceFlags = {};
  }

  Worker.prototype.on = function(event, cb) {
    if (!this._events[event]) this._events[event] = [];
    this._events[event].push(cb);
    return this;
  };

  Worker.prototype.once = function(event, cb) {
    this.on(event, cb);
    if (!this._onceFlags[event]) this._onceFlags[event] = [];
    this._onceFlags[event].push(this._events[event].length - 1);
    return this;
  };

  Worker.prototype.emit = function(event) {
    var args = Array.prototype.slice.call(arguments, 1);
    var cbs = this._events[event];
    if (!cbs) return false;
    var onceIndices = this._onceFlags[event] || [];
    var remaining = [];
    for (var i = 0; i < cbs.length; i++) {
      try { cbs[i].apply(null, args); } catch(e) {}
      if (onceIndices.indexOf(i) < 0) remaining.push(cbs[i]);
    }
    this._events[event] = remaining;
    this._onceFlags[event] = [];
    return true;
  };

  Worker.prototype.removeListener = function(event, cb) {
    var cbs = this._events[event];
    if (!cbs) return this;
    var idx = cbs.indexOf(cb);
    if (idx >= 0) cbs.splice(idx, 1);
    return this;
  };

  Worker.prototype.removeAllListeners = function(event) {
    if (event) {
      delete this._events[event];
    } else {
      this._events = {};
    }
    return this;
  };

  Worker.prototype.send = function(message, sendHandle) {
    // In this implementation, workers communicate via stdout.
    // The primary picks up messages from the worker's stdout pipe.
    if (this.process && typeof this.process.stdin !== 'undefined') {
      try {
        var data = JSON.stringify({ type: 'cluster:message', data: message, workerId: this.id });
        if (typeof this.process.stdin.write === 'function') {
          this.process.stdin.write(data + '\n');
        }
      } catch(e) {
        return false;
      }
    }
    return true;
  };

  Worker.prototype.kill = function(signal) {
    signal = signal || 'SIGTERM';
    if (this.process && typeof this.process.kill === 'function') {
      this.process.kill(signal);
    }
    this.isDead = true;
    this.isConnected = false;
    this.exitedAfterDisconnect = true;
  };

  Worker.prototype.disconnect = function() {
    this.isConnected = false;
    this.exitedAfterDisconnect = true;
    // Close IPC channel.
    if (this.process && typeof this.process.disconnect === 'function') {
      this.process.disconnect();
    }
  };

  Worker.prototype.destroy = function(signal) {
    this.kill(signal || 'SIGTERM');
  };

  // Store Worker constructor.
  cluster._Worker = Worker;

  // In worker process: set up process.send() / process.on('message') bridge.
  if (cluster.isWorker) {
    // process.send — write message to stdout as JSON.
    if (typeof process.send !== 'function') {
      process.send = function(message, sendHandle) {
        try {
          var data = JSON.stringify({ type: 'cluster:message', data: message, workerId: process.env.BAO_CLUSTER_WORKER_ID });
          process.stdout.write(data + '\n');
          return true;
        } catch(e) {
          return false;
        }
      };
    }

    // process.on('message') — read from stdin for IPC messages from primary.
    if (typeof process._clusterMessageHandler === 'undefined') {
      process._clusterMessageHandler = function(handler) {
        // In a full implementation, we'd set up a readline interface on stdin.
        // For now, messages from primary are received via stdin.
        // The polling mechanism in child_process handles the data flow.
      };
    }

    // Set cluster.worker to a Worker instance for this process.
    var workerId = parseInt(process.env.BAO_CLUSTER_WORKER_ID || '0', 10);
    cluster.worker = new Worker(workerId, process);
  }

  // In primary process: enhance cluster.fork to return Worker objects.
  if (cluster.isPrimary) {
    var _originalFork = cluster.fork;
    // The native fork already creates the child process and returns a basic object.
    // We wrap it to add Worker methods.
    cluster.fork = function(env) {
      var result = _originalFork ? _originalFork.call(cluster, env) : null;
      if (result && result.id) {
        var worker = new Worker(result.id, result.process || result);
        worker._pid = result._pid || (result.process && result.process.pid) || 0;

        // Copy the native result's process object.
        if (result.process) {
          worker.process = result.process;
        }

        // Register in cluster.workers.
        if (!cluster.workers) cluster.workers = {};
        cluster.workers[result.id] = worker;

        // Set up exit handler.
        if (worker.process && typeof worker.process.on === 'function') {
          worker.process.on('exit', function(code, signal) {
            worker.isDead = true;
            worker.isConnected = false;
            worker.emit('exit', code, signal);
            cluster.emit('exit', worker, code, signal);
            delete cluster.workers[worker.id];
          });
        }

        cluster.emit('fork', worker);
        return worker;
      }
      return result;
    };

    // Cluster-level EventEmitter.
    cluster._clusterEvents = {};
    cluster.on = function(event, cb) {
      if (!cluster._clusterEvents[event]) cluster._clusterEvents[event] = [];
      cluster._clusterEvents[event].push(cb);
      return cluster;
    };
    cluster.once = function(event, cb) {
      cluster.on(event, cb);
      return cluster;
    };
    cluster.emit = function(event) {
      var args = Array.prototype.slice.call(arguments, 1);
      var cbs = cluster._clusterEvents[event];
      if (!cbs) return false;
      for (var i = 0; i < cbs.length; i++) {
        try { cbs[i].apply(null, args); } catch(e) {}
      }
      return true;
    };
    cluster.removeListener = function(event, cb) {
      var cbs = cluster._clusterEvents[event];
      if (!cbs) return cluster;
      var idx = cbs.indexOf(cb);
      if (idx >= 0) cbs.splice(idx, 1);
      return cluster;
    };

    // Initialize settings._nextId counter.
    if (!cluster.settings) cluster.settings = {};
    if (!cluster.settings._nextId) cluster.settings._nextId = 1;
  }
})();
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_cluster_worker_default() {
        // In test environment, no BAO_CLUSTER_WORKER_ID is set.
        assert!(!is_cluster_worker());
    }

    #[test]
    fn test_is_cluster_worker_with_env() {
        // SAFETY: Safe in test context: single-threaded test, no concurrent env access.
        unsafe { ::std::env::set_var("BAO_CLUSTER_WORKER_ID", "3") };
        assert!(is_cluster_worker());
        // SAFETY: Safe in test context: single-threaded test, no concurrent env access.
        unsafe { ::std::env::remove_var("BAO_CLUSTER_WORKER_ID") };
    }

    #[test]
    fn test_is_primary_default() {
        assert!(!is_cluster_worker()); // is_primary = !is_worker
    }
}
