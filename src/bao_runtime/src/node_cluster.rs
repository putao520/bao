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

/// Pure worker-id predicate (no env access; testable without global state).
///
/// Strict form (BCE hardening for the "isPrimary occasionally flips false"
/// class): fork() only ever issues ids 1, 2, 3… (see the `_nextId` counter in
/// cluster_fork), so a well-formed worker env is a parseable integer ≥ 1.
/// Anything else — var present but EMPTY (e.g. `BAO_CLUSTER_WORKER_ID= bao`),
/// "0", or garbage — was never issued by our fork and must classify as
/// primary. The previous `is_some()` predicate flipped primary→worker on any
/// stray/empty env entry.
fn is_worker_env(worker_id: Option<&str>) -> bool {
    match worker_id {
        Some(s) => s.parse::<u32>().map(|n| n >= 1).unwrap_or(false),
        None => false,
    }
}

/// Process-wide frozen worker classification.
///
/// `process.env.X = v` in JS bridges to `std::env::set_var` (bun_api env
/// setter), so a per-install env read would let a user env write in one realm
/// flip `isPrimary` for every realm created afterwards (browser PagePool /
/// multi-context processes create realms lazily). Freezing at first install
/// makes the classification a property of process birth (exec env), which is
/// the Node semantic (NODE_WORKER_ID is decided by how the process was
/// started, never by later env writes).
static IS_WORKER_FROZEN: ::std::sync::OnceLock<bool> = ::std::sync::OnceLock::new();

/// Check if this process is a cluster worker (started with --cluster-worker env).
fn is_cluster_worker() -> bool {
    *IS_WORKER_FROZEN.get_or_init(|| {
        is_worker_env(::std::env::var("BAO_CLUSTER_WORKER_ID").ok().as_deref())
    })
}

// ─── Module install ────────────────────────────────────────────────────────

pub fn install(cx: &mut mozjs::context::JSContext) {
    rooted!(&in(cx) let obj = unsafe { w2::JS_NewPlainObject(cx) });
    if obj.get().is_null() {
        return;
    }

    let is_worker = is_cluster_worker();
    let is_primary = !is_worker;

    unsafe {
        let raw_cx = cx.raw_cx();

        // isPrimary
        rooted!(&in(cx) let is_primary_val = BooleanValue(is_primary));
        let _ = JS_DefineProperty(
            raw_cx,
            obj.handle().into(),
            c"isPrimary".as_ptr(),
            is_primary_val.handle().into(),
            JSPROP_ENUMERATE as u32,
        );

        // isMaster (deprecated alias)
        rooted!(&in(cx) let is_master_val = BooleanValue(is_primary));
        let _ = JS_DefineProperty(
            raw_cx,
            obj.handle().into(),
            c"isMaster".as_ptr(),
            is_master_val.handle().into(),
            JSPROP_ENUMERATE as u32,
        );

        // isWorker
        rooted!(&in(cx) let is_worker_val = BooleanValue(is_worker));
        let _ = JS_DefineProperty(
            raw_cx,
            obj.handle().into(),
            c"isWorker".as_ptr(),
            is_worker_val.handle().into(),
            JSPROP_ENUMERATE as u32,
        );

        // workers = empty object
        rooted!(&in(cx) let workers_obj = w2::JS_NewPlainObject(cx));
        if !workers_obj.get().is_null() {
            rooted!(&in(cx) let workers_val = ObjectValue(workers_obj.get()));
            let _ = JS_DefineProperty(
                raw_cx,
                obj.handle().into(),
                c"workers".as_ptr(),
                workers_val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }

        // settings = empty object
        rooted!(&in(cx) let settings_obj = w2::JS_NewPlainObject(cx));
        if !settings_obj.get().is_null() {
            rooted!(&in(cx) let settings_val = ObjectValue(settings_obj.get()));
            let _ = JS_DefineProperty(
                raw_cx,
                obj.handle().into(),
                c"settings".as_ptr(),
                settings_val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }

        // worker — current worker object (if worker), or undefined (if primary)
        if is_worker {
            rooted!(&in(cx) let worker_obj = make_worker_object(cx, raw_cx));
            if !worker_obj.get().is_null() {
                rooted!(&in(cx) let worker_val = ObjectValue(worker_obj.get()));
                let _ = JS_DefineProperty(
                    raw_cx,
                    obj.handle().into(),
                    c"worker".as_ptr(),
                    worker_val.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        } else {
            rooted!(&in(cx) let worker_val = UndefinedValue());
            let _ = JS_DefineProperty(
                raw_cx,
                obj.handle().into(),
                c"worker".as_ptr(),
                worker_val.handle().into(),
                JSPROP_ENUMERATE as u32,
            );
        }

        // fork() — spawns a worker process
        let fork_fn = JS_NewFunction(raw_cx, Some(cluster_fork), 0, 0, c"fork".as_ptr());
        if !fork_fn.is_null() {
            let fn_obj = JS_GetFunctionObject(fork_fn);
            if !fn_obj.is_null() {
                rooted!(&in(cx) let val = ObjectValue(fn_obj));
                let _ = JS_DefineProperty(
                    raw_cx,
                    obj.handle().into(),
                    c"fork".as_ptr(),
                    val.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }

        // disconnect()
        let disconnect_fn = JS_NewFunction(
            raw_cx,
            Some(cluster_disconnect),
            0,
            0,
            c"disconnect".as_ptr(),
        );
        if !disconnect_fn.is_null() {
            let fn_obj = JS_GetFunctionObject(disconnect_fn);
            if !fn_obj.is_null() {
                rooted!(&in(cx) let val = ObjectValue(fn_obj));
                let _ = JS_DefineProperty(
                    raw_cx,
                    obj.handle().into(),
                    c"disconnect".as_ptr(),
                    val.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }

        // setupPrimary() / setupMaster()
        let setup_fn = JS_NewFunction(
            raw_cx,
            Some(cluster_setup_primary),
            1,
            0,
            c"setupPrimary".as_ptr(),
        );
        if !setup_fn.is_null() {
            let fn_obj = JS_GetFunctionObject(setup_fn);
            if !fn_obj.is_null() {
                rooted!(&in(cx) let val = ObjectValue(fn_obj));
                let _ = JS_DefineProperty(
                    raw_cx,
                    obj.handle().into(),
                    c"setupPrimary".as_ptr(),
                    val.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }
        let setup_master_fn = JS_NewFunction(
            raw_cx,
            Some(cluster_setup_primary),
            1,
            0,
            c"setupMaster".as_ptr(),
        );
        if !setup_master_fn.is_null() {
            let fn_obj = JS_GetFunctionObject(setup_master_fn);
            if !fn_obj.is_null() {
                rooted!(&in(cx) let val = ObjectValue(fn_obj));
                let _ = JS_DefineProperty(
                    raw_cx,
                    obj.handle().into(),
                    c"setupMaster".as_ptr(),
                    val.handle().into(),
                    JSPROP_ENUMERATE as u32,
                );
            }
        }

        // schedulingPolicy = SCHED_RR (2) for round-robin connection distribution
        rooted!(&in(cx) let sched = Int32Value(2));
        let _ = JS_DefineProperty(
            raw_cx,
            obj.handle().into(),
            c"schedulingPolicy".as_ptr(),
            sched.handle().into(),
            JSPROP_ENUMERATE as u32,
        );

        // SCHED_NONE = 1, SCHED_RR = 2
        rooted!(&in(cx) let sched_none = Int32Value(1));
        let _ = JS_DefineProperty(
            raw_cx,
            obj.handle().into(),
            c"SCHED_NONE".as_ptr(),
            sched_none.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
        rooted!(&in(cx) let sched_rr = Int32Value(2));
        let _ = JS_DefineProperty(
            raw_cx,
            obj.handle().into(),
            c"SCHED_RR".as_ptr(),
            sched_rr.handle().into(),
            JSPROP_ENUMERATE as u32,
        );

        // Worker-boot + kill natives (see cluster_worker_boot / _kill docs).
        let boot_fn = JS_NewFunction(
            raw_cx,
            Some(cluster_worker_boot),
            1,
            0,
            c"__cluster_worker_boot".as_ptr(),
        );
        if !boot_fn.is_null() {
            let fn_obj = JS_GetFunctionObject(boot_fn);
            if !fn_obj.is_null() {
                rooted!(&in(cx) let val = ObjectValue(fn_obj));
                let _ = JS_DefineProperty(
                    raw_cx,
                    obj.handle().into(),
                    c"__cluster_worker_boot".as_ptr(),
                    val.handle().into(),
                    0,
                );
            }
        }
        let kill_fn = JS_NewFunction(
            raw_cx,
            Some(cluster_worker_kill),
            2,
            0,
            c"__cluster_worker_kill".as_ptr(),
        );
        if !kill_fn.is_null() {
            let fn_obj = JS_GetFunctionObject(kill_fn);
            if !fn_obj.is_null() {
                rooted!(&in(cx) let val = ObjectValue(fn_obj));
                let _ = JS_DefineProperty(
                    raw_cx,
                    obj.handle().into(),
                    c"__cluster_worker_kill".as_ptr(),
                    val.handle().into(),
                    0,
                );
            }
        }
        let ipc_send_fn = JS_NewFunction(
            raw_cx,
            Some(cluster_ipc_send),
            2,
            0,
            c"__cluster_ipc_send".as_ptr(),
        );
        if !ipc_send_fn.is_null() {
            let fn_obj = JS_GetFunctionObject(ipc_send_fn);
            if !fn_obj.is_null() {
                rooted!(&in(cx) let val = ObjectValue(fn_obj));
                let _ = JS_DefineProperty(
                    raw_cx,
                    obj.handle().into(),
                    c"__cluster_ipc_send".as_ptr(),
                    val.handle().into(),
                    0,
                );
            }
        }
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
) -> *mut JSObject {
    unsafe {
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
        JS_DefineProperty(
            cx.raw_cx(),
            worker_h,
            c"id".as_ptr(),
            id_val.handle().into(),
            JSPROP_ENUMERATE as u32,
        );

        // process — null (would need to reference the actual ChildProcess, set from JS shim)
        rooted!(&in(cx) let null_v = NullValue());
        JS_DefineProperty(
            cx.raw_cx(),
            worker_h,
            c"process".as_ptr(),
            null_v.handle().into(),
            JSPROP_ENUMERATE as u32,
        );

        // isConnected = true
        rooted!(&in(cx) let connected_v = BooleanValue(true));
        JS_DefineProperty(
            cx.raw_cx(),
            worker_h,
            c"isConnected".as_ptr(),
            connected_v.handle().into(),
            JSPROP_ENUMERATE as u32,
        );

        // isDead = false
        rooted!(&in(cx) let dead_v = BooleanValue(false));
        JS_DefineProperty(
            cx.raw_cx(),
            worker_h,
            c"isDead".as_ptr(),
            dead_v.handle().into(),
            JSPROP_ENUMERATE as u32,
        );

        // exitedAfterDisconnect = false
        rooted!(&in(cx) let ead_v = BooleanValue(false));
        JS_DefineProperty(
            cx.raw_cx(),
            worker_h,
            c"exitedAfterDisconnect".as_ptr(),
            ead_v.handle().into(),
            JSPROP_ENUMERATE as u32,
        );

        // _events placeholder (for JS shim to enhance with EventEmitter)
        rooted!(&in(cx) let events_obj = w2::JS_NewPlainObject(cx));
        if !events_obj.get().is_null() {
            rooted!(&in(cx) let events_val = ObjectValue(events_obj.get()));
            JS_DefineProperty(
                cx.raw_cx(),
                worker_h,
                c"_events".as_ptr(),
                events_val.handle().into(),
                0,
            );
        }

        worker_r.get()
    }
}

/// cluster.fork(env?) — spawn a worker process asynchronously.
///
/// BCE (v-surface P0-4) root causes fixed here:
///   1. envp entries were built WITHOUT NUL terminators — execve requires
///      NUL-terminated C strings, so the child exec'd with garbage env and
///      never ran its worker branch. spawn_cluster_worker now appends the
///      NULs (CString).
///   2. bun_spawn::sync::spawn BLOCKS until the child exits — fork() could
///      never deliver online/exit/message events. Now the async
///      spawn_process path (same as child_process.spawn) with exit tracking
///      via CP_ASYNC_STATES + a poll thread.
///   3. fork(env) — the env object argument was parsed nowhere; now merged
///      into the child env (Node semantics: fork env overrides matching
///      keys, the rest is inherited).
///   4. The IPC contract exists for real now: the child gets the IPC socket
///      at fd 3 (PosixStdio::Ipc) + BAO_CLUSTER_IPC_FD=3, and the worker boot
///      path (__cluster_worker_boot) wraps it into CP_IPC_CHANNELS keyed by
///      the worker's own pid, powering process.send / process.on('message').
///
/// The JS shim (CLUSTER_JS) wraps the returned object in a Worker with
/// EventEmitter methods and pumps online/message/exit events.
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
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    let script_path = {
        rooted!(&in(cx_ref) let global = CurrentGlobalOrNull(cx));
        let mut process_val = UndefinedValue();
        JS_GetProperty(
            cx,
            global.handle().into(),
            c"process".as_ptr(),
            MutableHandle::<Value> {
                _phantom_0: ::std::marker::PhantomData,
                ptr: &mut process_val,
            },
        );
        if process_val.is_object() {
            let process_obj = process_val.to_object();
            rooted!(&in(cx_ref) let process_r = process_obj);
            let mut argv_val = UndefinedValue();
            JS_GetProperty(
                cx,
                process_r.handle().into(),
                c"argv".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut argv_val,
                },
            );
            if argv_val.is_object() {
                let argv_obj = argv_val.to_object();
                rooted!(&in(cx_ref) let argv_r = argv_obj);
                // bao's process.argv = [exec, "run", <script>] when invoked via
                // the `run` subcommand (Node puts the script at argv[1]; bao
                // keeps the subcommand). The worker must re-run the SCRIPT.
                let mut first = UndefinedValue();
                JS_GetElement(
                    cx,
                    argv_r.handle().into(),
                    1,
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut first,
                    },
                );
                let mut second = UndefinedValue();
                JS_GetElement(
                    cx,
                    argv_r.handle().into(),
                    2,
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut second,
                    },
                );
                if first.is_string()
                    && crate::js_to_rust_string(cx, first) == "run"
                    && second.is_string()
                {
                    crate::js_to_rust_string(cx, second)
                } else if first.is_string() {
                    crate::js_to_rust_string(cx, first)
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
        JS_ReportErrorUTF8(
            cx,
            c"cluster.fork(): cannot determine script path (process.argv[1] is empty)".as_ptr(),
        );
        args.rval().set(UndefinedValue());
        return false;
    }

    // Determine the next worker ID from cluster.settings._nextId.
    let worker_id: i32 = {
        if let Some(cluster_mod) = crate::require::get_builtin(cx_ref.raw_cx(), "cluster") {
            if !cluster_mod.is_null() {
                rooted!(&in(cx_ref) let cm_r = cluster_mod);
                let mut settings_val = UndefinedValue();
                JS_GetProperty(
                    cx,
                    cm_r.handle().into(),
                    c"settings".as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut settings_val,
                    },
                );
                if settings_val.is_object() {
                    let settings_obj = settings_val.to_object();
                    rooted!(&in(cx_ref) let settings_r = settings_obj);
                    let mut next_id_val = UndefinedValue();
                    JS_GetProperty(
                        cx,
                        settings_r.handle().into(),
                        c"_nextId".as_ptr(),
                        MutableHandle::<Value> {
                            _phantom_0: ::std::marker::PhantomData,
                            ptr: &mut next_id_val,
                        },
                    );
                    if next_id_val.is_int32() {
                        let id = next_id_val.to_int32();
                        let new_id = id + 1;
                        rooted!(&in(cx_ref) let new_id_v = Int32Value(new_id));
                        JS_SetProperty(
                            cx,
                            settings_r.handle().into(),
                            c"_nextId".as_ptr(),
                            new_id_v.handle().into(),
                        );
                        id
                    } else {
                        rooted!(&in(cx_ref) let init_v = Int32Value(2));
                        JS_SetProperty(
                            cx,
                            settings_r.handle().into(),
                            c"_nextId".as_ptr(),
                            init_v.handle().into(),
                        );
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

    // Resolve the bao binary: explicit override first (tests run under a
    // cargo harness whose current_exe is the test binary, not bao), then
    // current_exe().
    let exec_str = ::std::env::var("BAO_CLUSTER_EXEC").unwrap_or_else(|_| {
        ::std::env::current_exe()
            .unwrap_or_else(|_| ::std::path::PathBuf::from("bao"))
            .to_string_lossy()
            .into_owned()
    });

    // Child environment: inherit current env, then merge the fork(env) object
    // argument (if any) and the cluster control vars.
    let primary_pid = ::std::process::id();
    let mut env_map: ::std::collections::BTreeMap<String, String> =
        ::std::env::vars().collect();
    if argc > 0 && (*args.get(0).ptr).is_object() {
        let env_obj = (*args.get(0).ptr).to_object();
        rooted!(&in(cx_ref) let env_r = env_obj);
        let mut ids = mozjs::rust::IdVector::new(cx);
        if GetPropertyKeys(cx, env_r.handle().into(), JSITER_OWNONLY, ids.handle_mut()) {
            for jsid in &*ids {
                if !jsid.is_string() {
                    continue;
                }
                let key_ptr = jsid.to_string();
                let key = mozjs::conversions::unsafe_jsstr_to_string(
                    cx,
                    ::std::ptr::NonNull::new_unchecked(key_ptr),
                );
                let c_key = bun_core::ZBox::from_bytes(key.as_bytes());
                let mut v_val = UndefinedValue();
                JS_GetProperty(
                    cx,
                    env_r.handle().into(),
                    c_key.as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut v_val,
                    },
                );
                let val = if v_val.is_string() {
                    crate::js_to_rust_string(cx, v_val)
                } else if v_val.is_int32() {
                    v_val.to_int32().to_string()
                } else if v_val.is_boolean() {
                    if v_val.to_boolean() {
                        "true".to_string()
                    } else {
                        "false".to_string()
                    }
                } else {
                    continue;
                };
                env_map.insert(key, val);
            }
        }
    }
    env_map.insert("BAO_CLUSTER_WORKER_ID".to_string(), worker_id.to_string());
    env_map.insert(
        "BAO_CLUSTER_PRIMARY_PID".to_string(),
        primary_pid.to_string(),
    );
    env_map.insert("BAO_CLUSTER_IPC_FD".to_string(), "3".to_string());
    let env_entries: Vec<Box<[u8]>> = env_map
        .into_iter()
        .map(|(k, v)| format!("{}={}", k, v).into_bytes().into_boxed_slice())
        .collect();

    // Build argv for the child: bao run <script>
    let argv: Vec<Box<[u8]>> = vec![
        exec_str.as_bytes().to_vec().into_boxed_slice(),
        b"run".to_vec().into_boxed_slice(),
        script_path.as_bytes().to_vec().into_boxed_slice(),
    ];

    // Async spawn with fd-3 IPC + exit tracking (see spawn_cluster_worker).
    let pid = match super::node_child_process::spawn_cluster_worker(argv, env_entries) {
        Ok(p) => p,
        Err(msg) => {
            let c_msg = bun_core::ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            return false;
        }
    };

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
    JS_DefineProperty(
        cx,
        worker_h,
        c"id".as_ptr(),
        id_v.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    // process — minimal ChildProcess-shaped object; exitCode is filled in by
    // the JS shim's exit poll once the worker exits.
    let proc_obj = w2::JS_NewPlainObject(cx_ref);
    if !proc_obj.is_null() {
        rooted!(&in(cx_ref) let proc_r = proc_obj);
        let proc_h = proc_r.handle().into();

        rooted!(&in(cx_ref) let pid_v = Int32Value(pid));
        JS_DefineProperty(
            cx,
            proc_h,
            c"pid".as_ptr(),
            pid_v.handle().into(),
            JSPROP_ENUMERATE as u32,
        );

        rooted!(&in(cx_ref) let ec_v = NullValue());
        JS_DefineProperty(
            cx,
            proc_h,
            c"exitCode".as_ptr(),
            ec_v.handle().into(),
            JSPROP_ENUMERATE as u32,
        );

        let proc_val = ObjectValue(proc_r.get());
        rooted!(&in(cx_ref) let pv = proc_val);
        JS_DefineProperty(
            cx,
            worker_h,
            c"process".as_ptr(),
            pv.handle().into(),
            JSPROP_ENUMERATE as u32,
        );
    }

    // isConnected
    rooted!(&in(cx_ref) let conn_v = BooleanValue(true));
    JS_DefineProperty(
        cx,
        worker_h,
        c"isConnected".as_ptr(),
        conn_v.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    // isDead — false at spawn; the async child has not exited yet. The JS
    // shim flips it on the 'exit' event.
    rooted!(&in(cx_ref) let dead_v = BooleanValue(false));
    JS_DefineProperty(
        cx,
        worker_h,
        c"isDead".as_ptr(),
        dead_v.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    // exitedAfterDisconnect
    rooted!(&in(cx_ref) let ead_v = BooleanValue(false));
    JS_DefineProperty(
        cx,
        worker_h,
        c"exitedAfterDisconnect".as_ptr(),
        ead_v.handle().into(),
        JSPROP_ENUMERATE as u32,
    );

    // _pid (for native send/kill)
    rooted!(&in(cx_ref) let npid_v = Int32Value(pid));
    JS_DefineProperty(cx, worker_h, c"_pid".as_ptr(), npid_v.handle().into(), 0);

    // ─── Mount `send(msg[, sendHandle])` on the worker ─────────────────────
    //
    // Delegates to the parent IPC channel registered in CP_IPC_CHANNELS[pid]
    // by spawn_cluster_worker. Same wire format as child.send in
    // node_child_process (newline-delimited JSON, optional SCM_RIGHTS fd).
    w2::JS_DefineFunction(
        cx_ref,
        worker_r.handle(),
        c"send".as_ptr(),
        Some(cluster_worker_send),
        2,
        JSPROP_ENUMERATE as u32,
    );
    // `disconnect()` — close the IPC channel and remove from registry.
    w2::JS_DefineFunction(
        cx_ref,
        worker_r.handle(),
        c"disconnect".as_ptr(),
        Some(cluster_worker_disconnect),
        0,
        JSPROP_ENUMERATE as u32,
    );
    // `_ipcFd` — child-side fd number (Node fd-3 IPC convention).
    rooted!(&in(cx_ref) let ipcfd_v = Int32Value(3));
    JS_DefineProperty(cx, worker_h, c"_ipcFd".as_ptr(), ipcfd_v.handle().into(), 0);

    // Register worker in cluster.workers
    {
        if let Some(cluster_mod) = crate::require::get_builtin(cx_ref.raw_cx(), "cluster") {
            if !cluster_mod.is_null() {
                rooted!(&in(cx_ref) let cm_r = cluster_mod);
                let mut workers_val = UndefinedValue();
                JS_GetProperty(
                    cx,
                    cm_r.handle().into(),
                    c"workers".as_ptr(),
                    MutableHandle::<Value> {
                        _phantom_0: ::std::marker::PhantomData,
                        ptr: &mut workers_val,
                    },
                );
                if workers_val.is_object() {
                    let workers_obj = workers_val.to_object();
                    rooted!(&in(cx_ref) let workers_r = workers_obj);
                    let worker_val = ObjectValue(worker_r.get());
                    rooted!(&in(cx_ref) let wv = worker_val);
                    let id_c_str = bun_core::ZBox::from_bytes(format!("{}", worker_id).as_bytes());
                    JS_SetProperty(
                        cx,
                        workers_r.handle().into(),
                        id_c_str.as_ptr(),
                        wv.handle().into(),
                    );
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
    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    if let Some(cluster_mod) = crate::require::get_builtin(cx_ref.raw_cx(), "cluster") {
        if !cluster_mod.is_null() {
            rooted!(&in(cx_ref) let cm_r = cluster_mod);
            let mut workers_val = UndefinedValue();
            JS_GetProperty(
                cx,
                cm_r.handle().into(),
                c"workers".as_ptr(),
                MutableHandle::<Value> {
                    _phantom_0: ::std::marker::PhantomData,
                    ptr: &mut workers_val,
                },
            );
            if workers_val.is_object() {
                let workers_obj = workers_val.to_object();
                rooted!(&in(cx_ref) let workers_r = workers_obj);
                // Iterate over workers and kill each one.
                // Since we can't easily enumerate JS objects from Rust,
                // we use the JS shim to handle disconnect logic.
                // For now, just set a flag that the JS shim will pick up.
                let disconnected_v = BooleanValue(true);
                rooted!(&in(cx_ref) let dv = disconnected_v);
                JS_SetProperty(
                    cx,
                    cm_r.handle().into(),
                    c"_disconnecting".as_ptr(),
                    dv.handle().into(),
                );
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

    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    // Store settings on cluster.settings.
    if argc > 0 {
        let settings_val = *args.get(0).ptr;
        if settings_val.is_object() {
            if let Some(cluster_mod) = crate::require::get_builtin(cx_ref.raw_cx(), "cluster") {
                if !cluster_mod.is_null() {
                    rooted!(&in(cx_ref) let cm_r = cluster_mod);
                    rooted!(&in(cx_ref) let sv = settings_val);
                    JS_SetProperty(
                        cx,
                        cm_r.handle().into(),
                        c"settings".as_ptr(),
                        sv.handle().into(),
                    );
                }
            }
        }
    }

    args.rval().set(UndefinedValue());
    true
}

// ─── Native: worker.send(msg[, sendHandle]) ────────────────────────────────
//
// Send a JSON message on the cluster worker's IPC channel. If a numeric fd is
// passed as the second argument, the message is sent via SCM_RIGHTS ancillary
// data (fd handoff — used by master round-robin server handle passing).
//
// The worker's IPC channel is keyed by pid in CP_IPC_CHANNELS (populated by
// cluster_fork). Args from JS:
//   args[0] = msg   (string — caller already JSON.stringify'd)
//   args[1] = fd    (optional i32 — if present, use SCM_RIGHTS path)

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cluster_worker_send(
    cx: *mut JSContext,
    argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, argc);

    // The `this` value is the Worker JS object — read its `_pid`.
    let this_v = *args.thisv().ptr;
    let this_obj = if this_v.is_object() {
        this_v.to_object()
    } else {
        ::std::ptr::null_mut::<JSObject>()
    };

    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;

    rooted!(&in(cx_ref) let this_r = this_obj);
    let mut pid_v = UndefinedValue();
    JS_GetProperty(
        cx,
        this_r.handle().into(),
        c"_pid".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut pid_v,
        },
    );
    let pid = if pid_v.is_int32() {
        pid_v.to_int32()
    } else {
        0
    };
    if pid == 0 {
        args.rval().set(BooleanValue(false));
        return true;
    }

    let json_str = if argc > 0 {
        let v = *args.get(0).ptr;
        if v.is_string() {
            crate::js_to_rust_string(cx, v)
        } else {
            String::new()
        }
    } else {
        String::new()
    };
    let fd_opt: Option<i32> = if argc > 1 {
        let v = *args.get(1).ptr;
        if v.is_int32() {
            let n = v.to_int32();
            if n >= 0 {
                Some(n)
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    // Look up channel by pid, send under short-lived lock. No `?` operator
    // since we are in an `extern "C" fn` returning bool — chain with
    // `.map_err().and_then()` instead.
    let outcome: ::std::result::Result<(), String> =
        super::node_child_process::CP_IPC_CHANNELS
            .lock()
            .map_err(|e| format!("registry lock poisoned: {}", e))
            .and_then(|registry| {
                registry
                    .get(&pid)
                    .cloned()
                    .ok_or_else(|| format!("no ipc channel for worker pid {}", pid))
                    .and_then(|chan_mtx| {
                        chan_mtx
                            .lock()
                            .map_err(|e| format!("channel lock poisoned: {}", e))
                            .and_then(|mut chan| {
                                if let Some(fd) = fd_opt {
                                    chan.send_handle(&json_str, fd)
                                        .map_err(|e| format!("send_handle: {}", e))
                                } else {
                                    chan.send_json(&json_str)
                                        .map_err(|e| format!("send_json: {}", e))
                                }
                            })
                    })
            });

    match outcome {
        Ok(()) => {
            args.rval().set(BooleanValue(true));
            true
        }
        Err(msg) => {
            let c_msg = bun_core::ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            args.rval().set(BooleanValue(false));
            false
        }
    }
}

// ─── Native: worker.disconnect() ───────────────────────────────────────────
//
// Close the IPC channel from the primary side. Removes the channel from
// CP_IPC_CHANNELS so subsequent send/recv calls return errors cleanly.

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cluster_worker_disconnect(
    cx: *mut JSContext,
    _argc: u32,
    vp: *mut JSVal,
) -> bool {
    let args = CallArgs::from_vp(vp, _argc);

    // Read pid from `this`.
    let this_v = *args.thisv().ptr;
    let this_obj = if this_v.is_object() {
        this_v.to_object()
    } else {
        ::std::ptr::null_mut::<JSObject>()
    };

    let mut wrapped_cx =
        mozjs::context::JSContext::from_ptr(::std::ptr::NonNull::new_unchecked(cx));
    let cx_ref = &mut wrapped_cx;
    rooted!(&in(cx_ref) let this_r = this_obj);
    let mut pid_v = UndefinedValue();
    JS_GetProperty(
        cx,
        this_r.handle().into(),
        c"_pid".as_ptr(),
        MutableHandle::<Value> {
            _phantom_0: ::std::marker::PhantomData,
            ptr: &mut pid_v,
        },
    );
    let pid = if pid_v.is_int32() {
        pid_v.to_int32()
    } else {
        0
    };
    if pid != 0 {
        if let Ok(mut registry) = super::node_child_process::CP_IPC_CHANNELS.lock() {
            registry.remove(&pid);
        }
    }
    args.rval().set(UndefinedValue());
    true
}

// ─── Native: __cluster_worker_boot(fd) — worker-side IPC registration ──────
//
// Runs INSIDE the worker process (called from CLUSTER_JS on boot when
// BAO_CLUSTER_WORKER_ID is set). Wraps the inherited fd-3 IPC socket (the
// other end of the primary's CP_IPC_CHANNELS[worker_pid] channel) into an
// IpcChannel registered under the worker's OWN pid, so child_process's
// __cp_ipc_send / __cp_ipc_recv reach it — powering process.send() and
// process.on('message') on the worker side.

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cluster_worker_boot(_cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let fd = if argc > 0 && (*args.get(0).ptr).is_int32() {
        (*args.get(0).ptr).to_int32()
    } else {
        3
    };
    if fd < 0 {
        args.rval().set(BooleanValue(false));
        return true;
    }
    // SAFETY: fd comes from PosixStdio::Ipc — a live AF_UNIX socket inherited
    // from the primary; from_raw_fd takes sole ownership of it.
    let sock = unsafe {
        <::std::os::unix::net::UnixStream as ::std::os::unix::io::FromRawFd>::from_raw_fd(fd)
    };
    let channel = crate::ipc_channel::IpcChannel::new(sock);
    let self_pid = unsafe { libc::getpid() } as i32;
    if let Ok(mut registry) = super::node_child_process::CP_IPC_CHANNELS.lock() {
        registry.insert(self_pid, ::std::sync::Arc::new(::std::sync::Mutex::new(channel)));
    }
    args.rval().set(BooleanValue(true));
    true
}

// ─── Native: __cluster_ipc_send(pid, json) ─────────────────────────────────
//
// Send a JSON message on the IPC channel registered under `pid` in
// CP_IPC_CHANNELS (the primary side registers by worker pid at fork; the
// worker side registers under its own pid in __cluster_worker_boot). Used by
// the worker's process.send() — child_process's __cp_ipc_send is attached
// per-child-object, not exported on its module.

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cluster_ipc_send(cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let pid = if argc > 0 && (*args.get(0).ptr).is_int32() {
        (*args.get(0).ptr).to_int32()
    } else {
        0
    };
    if pid == 0 {
        args.rval().set(BooleanValue(false));
        return true;
    }
    let json_str = if argc > 1 && (*args.get(1).ptr).is_string() {
        crate::js_to_rust_string(cx, *args.get(1).ptr)
    } else {
        String::new()
    };

    let outcome: ::std::result::Result<(), String> =
        super::node_child_process::CP_IPC_CHANNELS
            .lock()
            .map_err(|e| format!("registry lock poisoned: {}", e))
            .and_then(|registry| {
                registry
                    .get(&pid)
                    .cloned()
                    .ok_or_else(|| format!("no ipc channel for pid {}", pid))
                    .and_then(|chan_mtx| {
                        chan_mtx
                            .lock()
                            .map_err(|e| format!("channel lock poisoned: {}", e))
                            .and_then(|mut chan| {
                                chan
                                    .send_json(&json_str)
                                    .map_err(|e| format!("send_json: {}", e))
                            })
                    })
            });

    match outcome {
        Ok(()) => {
            args.rval().set(BooleanValue(true));
            true
        }
        Err(msg) => {
            let c_msg = bun_core::ZBox::from_bytes(msg.as_bytes());
            JS_ReportErrorUTF8(cx, c"%s".as_ptr(), c_msg.as_ptr());
            args.rval().set(BooleanValue(false));
            false
        }
    }
}

// ─── Native: __cluster_worker_kill(pid, signal) ────────────────────────────

#[allow(unsafe_op_in_unsafe_fn)]
unsafe extern "C" fn cluster_worker_kill(_cx: *mut JSContext, argc: u32, vp: *mut JSVal) -> bool {
    let args = CallArgs::from_vp(vp, argc);
    let pid = if argc > 0 && (*args.get(0).ptr).is_int32() {
        (*args.get(0).ptr).to_int32()
    } else {
        0
    };
    let sig = if argc > 1 && (*args.get(1).ptr).is_int32() {
        (*args.get(1).ptr).to_int32()
    } else {
        15 // SIGTERM
    };
    if pid <= 0 {
        args.rval().set(BooleanValue(false));
        return true;
    }
    // SAFETY: libc::kill with a numeric pid/sig — kernel validates both.
    let rc = unsafe { libc::kill(pid, sig) };
    args.rval().set(BooleanValue(rc == 0));
    true
}

const CLUSTER_JS: &str = r#"
(function() {
  var cluster = require('cluster');
  var cp = (function () { try { return require('child_process'); } catch (e) { return null; } })();

  var SIG = { SIGHUP: 1, SIGINT: 2, SIGQUIT: 3, SIGABRT: 6, SIGKILL: 9, SIGUSR1: 10, SIGUSR2: 12, SIGTERM: 15 };

  // Worker class with EventEmitter mixin.
  function Worker(id, process) {
    this.id = id;
    this.process = process;
    this.isConnected = true;
    this.isDead = false;
    this.exitedAfterDisconnect = false;
    this._events = {};
    this._onceFlags = {};
    this._online = false;
    this._disconnecting = false;
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
    if (!cbs || cbs.length === 0) return false;
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

  cluster._Worker = Worker;

  // ─── Worker process boot: IPC wiring + process.send / 'message' ──────────
  if (cluster.isWorker && cp) {
    var fd = parseInt(process.env.BAO_CLUSTER_IPC_FD || '3', 10);
    var booted = typeof cluster.__cluster_worker_boot === 'function'
      && cluster.__cluster_worker_boot(fd);
    if (booted) {
      process.connected = true;
      process.send = function(message, sendHandle) {
        try { return cluster.__cluster_ipc_send(process.pid, JSON.stringify(message)); }
        catch (e) { return false; }
      };
      process.disconnect = function() {
        try { process.exit(0); } catch (e) {}
      };
      // Poll the IPC channel for primary → worker messages.
      setInterval(function() {
        try {
          var m = cp.__cp_ipc_recv(process.pid);
          while (m && m.json) {
            var obj = null;
            try { obj = JSON.parse(m.json); } catch (e) { obj = null; }
            if (obj && obj.__cluster === 'disconnect') {
              process.exit(0);
            } else if (obj) {
              try { process.emit('message', obj); } catch (e) {}
            }
            m = cp.__cp_ipc_recv(process.pid);
          }
          // Primary closed the channel (disconnect) — exit gracefully.
          if (m && m.closed) {
            process.exit(0);
          }
        } catch (e) {}
      }, 10);
      // Online handshake → primary emits worker 'online'.
      try {
        cluster.__cluster_ipc_send(process.pid, JSON.stringify({
          __cluster: 'online',
          workerId: process.env.BAO_CLUSTER_WORKER_ID
        }));
      } catch (e) {}
    }
    var workerId = parseInt(process.env.BAO_CLUSTER_WORKER_ID || '0', 10);
    cluster.worker = new Worker(workerId, process);
  }

  // ─── Primary: wrap fork() results in Worker objects + event pump ─────────
  if (cluster.isPrimary) {
    var _originalFork = cluster.fork;
    var pollTimer = null;

    function ensurePolling() {
      if (pollTimer === null && typeof setInterval === 'function') {
        pollTimer = setInterval(pollWorkers, 10);
      }
    }

    function dispatchMessage(w, json) {
      var obj = null;
      try { obj = JSON.parse(json); } catch (e) { return; }
      if (!obj || typeof obj !== 'object') return;
      if (obj.__cluster === 'online') {
        if (!w._online) {
          w._online = true;
          w.emit('online');
          cluster.emit('online', w);
        }
        return;
      }
      w.emit('message', obj);
    }

    function handleExit(w, code, signal) {
      if (w.isDead) return;
      w.isDead = true;
      w.isConnected = false;
      w.exitedAfterDisconnect = !!w._disconnecting;
      if (w.process) w.process.exitCode = (code === -1 && signal) ? null : code;
      delete cluster.workers[w.id];
      try { if (typeof w.__disconnectNative === 'function') w.__disconnectNative(); } catch (e) {}
      w.emit('exit', code, signal);
      cluster.emit('exit', w, code, signal);
    }

    function pollWorkers() {
      var ids = Object.keys(cluster.workers);
      for (var i = 0; i < ids.length; i++) {
        var w = cluster.workers[ids[i]];
        if (!w || !w._pid) continue;
        if (cp) {
          try {
            var m = cp.__cp_ipc_recv(w._pid);
            while (m && m.json) {
              dispatchMessage(w, m.json);
              m = cp.__cp_ipc_recv(w._pid);
            }
          } catch (e) {}
          try {
            var ex = cp.__cp_poll_exit(w._pid);
            if (ex) handleExit(w, ex[0], ex[1]);
          } catch (e) {}
        }
      }
      if (Object.keys(cluster.workers).length === 0 && pollTimer !== null) {
        clearInterval(pollTimer);
        pollTimer = null;
      }
    }

    cluster.fork = function(env) {
      var result = _originalFork ? _originalFork.call(cluster, env) : null;
      if (result && result.id) {
        var worker = new Worker(result.id, result.process || result);
        worker._pid = result._pid || (result.process && result.process.pid) || 0;
        if (result.process) worker.process = result.process;

        // Native bridges from the fork result object.
        if (typeof result.send === 'function') {
          var nativeSend = result.send;
          worker.send = function(message, sendHandle) {
            try { return nativeSend.call(result, JSON.stringify(message), sendHandle); }
            catch (e) { return false; }
          };
        }
        if (typeof result.disconnect === 'function') {
          var nativeDisconnect = result.disconnect;
          worker.__disconnectNative = function() { nativeDisconnect.call(result); };
          worker.disconnect = function() {
            worker._disconnecting = true;
            worker.isConnected = false;
            try { nativeDisconnect.call(result); } catch (e) {}
          };
        }
        worker.kill = function(signal) {
          var sig = typeof signal === 'number' ? signal : (SIG[String(signal).toUpperCase()] || 15);
          if (cluster.__cluster_worker_kill) {
            try { cluster.__cluster_worker_kill(worker._pid, sig); } catch (e) {}
          }
          worker.isDead = true;
          worker.isConnected = false;
          worker.exitedAfterDisconnect = worker._disconnecting;
        };
        worker.destroy = function(signal) { worker.kill(signal); };

        if (!cluster.workers) cluster.workers = {};
        cluster.workers[result.id] = worker;
        ensurePolling();
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
      var wrap = function() {
        cluster.removeListener(event, wrap);
        cb.apply(null, arguments);
      };
      cluster.on(event, wrap);
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

    // cluster.disconnect(): ask every worker to exit (the worker exits on the
    // disconnect IPC message — bao's orderly-exit path can swallow SIGTERM),
    // close its channel, then SIGTERM as a backstop.
    cluster.disconnect = function(callback) {
      cluster._disconnecting = true;
      var ids = Object.keys(cluster.workers || {});
      for (var i = 0; i < ids.length; i++) {
        var w = cluster.workers[ids[i]];
        try { if (w.send) w.send({ __cluster: 'disconnect' }); } catch (e) {}
        try { if (w.disconnect) w.disconnect(); } catch (e) {}
        try { if (cluster.__cluster_worker_kill) cluster.__cluster_worker_kill(w._pid, 15); } catch (e) {}
      }
      if (typeof callback === 'function') {
        setTimeout(callback, 50);
      }
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
        // Pure predicate: no env set → not a worker. No env mutation, no race
        // with parallel tests.
        assert!(!is_worker_env(None));
    }

    #[test]
    fn test_is_cluster_worker_with_env() {
        // fork() issues ids 1, 2, 3… — anything ≥ 1 is a worker.
        assert!(is_worker_env(Some("1")));
        assert!(is_worker_env(Some("3")));
        // "0" is never issued by fork (first id is 1): classify as primary.
        assert!(!is_worker_env(Some("0")));
    }

    #[test]
    fn test_is_cluster_worker_strict_predicate() {
        // Empty / malformed env entries (e.g. `BAO_CLUSTER_WORKER_ID= bao`)
        // must NOT flip a primary into a worker — fork never issues these.
        assert!(!is_worker_env(Some("")));
        assert!(!is_worker_env(Some("garbage")));
        assert!(!is_worker_env(Some("-1")));
        assert!(!is_worker_env(Some("1.5")));
        assert!(!is_worker_env(Some("1x")));
    }

    #[test]
    fn test_is_primary_default() {
        // is_primary = !is_worker
        assert!(!is_worker_env(None));
    }
}
