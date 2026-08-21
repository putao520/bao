// @trace TEST-ENG-007 [req:REQ-ENG-007] [level:integration]
// Semantic ports of two upstream bun fixes (verified against
// ~/code/rust/bun git show):
//   - fff3b29d6 "node:fs: accept rmdir recursive again and route it
//     through rm" — rmdir/rmdirSync/promises.rmdir with a truthy
//     `options.recursive` route to rm semantics (recursive tree removal);
//     absent/false keeps plain rmdir behavior (ENOTEMPTY on non-empty,
//     ENOTDIR on a file path).
//   - 46a6c3927 "node:http: emit ServerResponse 'close' asynchronously
//     after destroy()" — destroy() must NOT emit 'close' synchronously;
//     'close' fires once, after destroy() returns (Node: socket teardown
//     or nextTick emitCloseNT; bao: microtask, the crate's nextTick shape
//     per node_stream.rs Writable.end), and ServerResponse.destroy must
//     emit 'close' at all (it previously never did).

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(JsValue::Null) => "null".to_string(),
        Ok(JsValue::Undefined) => "undefined".to_string(),
        Ok(JsValue::Object(_)) => "[object]".to_string(),
        Err(e) => format!("ERROR:{}", e.message),
    }
}

fn setup_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

fn js_escape(p: &std::path::Path) -> String {
    p.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"")
}

// ============================================================================
// fff3b29d6 — fs.rmdir recursive routes through rm
// ============================================================================

#[test]
fn fs_rmdir_recursive_routes_through_rm() {
    let mut ctx = setup_ctx();

    let base = ::std::env::temp_dir().join("bao_upstream_rmdir_recursive");
    let _ = ::std::fs::remove_dir_all(&base);
    ::std::fs::create_dir_all(&base).unwrap();

    // Non-empty nested trees for the recursive route.
    let mk_tree = |name: &str| {
        let root = base.join(name);
        ::std::fs::create_dir_all(root.join("sub").join("deeper")).unwrap();
        ::std::fs::write(root.join("top.txt"), "top").unwrap();
        ::std::fs::write(root.join("sub").join("mid.txt"), "mid").unwrap();
        ::std::fs::write(root.join("sub").join("deeper").join("leaf.txt"), "leaf").unwrap();
        root
    };
    let tree_sync = mk_tree("tree_sync"); // rmdirSync recursive:true
    let tree_promises = mk_tree("tree_promises"); // promises.rmdir recursive:true
    let tree_cb = mk_tree("tree_cb"); // fs.rmdir callback recursive:true
    let guard_false = mk_tree("guard_false"); // recursive:false keeps rmdir
    let guard_none = mk_tree("guard_none"); // no options keeps rmdir
    let empty_dir = base.join("empty_dir"); // recursive on an empty dir
    ::std::fs::create_dir_all(&empty_dir).unwrap();
    let afile = base.join("afile.txt"); // plain rmdir on a file → ENOTDIR class
    ::std::fs::write(&afile, "f").unwrap();
    let missing = base.join("missing_dir_xyz");

    let p_tree_sync = js_escape(&tree_sync);
    let p_tree_promises = js_escape(&tree_promises);
    let p_tree_cb = js_escape(&tree_cb);
    let p_guard_false = js_escape(&guard_false);
    let p_guard_none = js_escape(&guard_none);
    let p_empty = js_escape(&empty_dir);
    let p_file = js_escape(&afile);
    let p_missing = js_escape(&missing);

    // --- sync + promises (single eval, `check` label aggregation) ---
    let out = eval_string(
        &mut ctx,
        &format!(
            r#"
        var fs = require('fs');
        var results = [];
        function check(label, fn) {{
            try {{ results.push(label + ':' + (fn() ? 'PASS' : 'FAIL')); }}
            catch (e) {{ results.push(label + ':ERROR:' + ((e && e.message) || e)); }}
        }}
        function throwsCode(kind, fn) {{
            try {{ fn(); return 'NO_THROW'; }}
            catch (e) {{
                var m = ((e && e.message) || String(e));
                if (kind === 'ENOTEMPTY') return /not empty|ENOTEMPTY/i.test(m) ? 'THREW' : 'OTHER:' + m.substring(0, 60);
                if (kind === 'ENOTDIR') return /directory|ENOTDIR|not a dir/i.test(m) ? 'THREW' : 'OTHER:' + m.substring(0, 60);
                if (kind === 'ENOENT') return /no such|ENOENT/i.test(m) ? 'THREW' : 'OTHER:' + m.substring(0, 60);
                return 'THREW';
            }}
        }}

        // rmdirSync recursive:true removes the whole tree (fff3b29d6 core).
        check("rmdirSync_recursive_tree", function() {{
            fs.rmdirSync("{p_tree_sync}", {{ recursive: true }});
            return !fs.existsSync("{p_tree_sync}");
        }});
        // recursive:true on an empty dir also succeeds.
        check("rmdirSync_recursive_empty", function() {{
            fs.rmdirSync("{p_empty}", {{ recursive: true }});
            return !fs.existsSync("{p_empty}");
        }});
        // recursive:false keeps plain rmdir: non-empty → ENOTEMPTY.
        check("rmdirSync_recursive_false_nonempty", function() {{
            return throwsCode('ENOTEMPTY', function() {{ fs.rmdirSync("{p_guard_false}", {{ recursive: false }}); }}) === 'THREW';
        }});
        // no options keeps plain rmdir: non-empty → ENOTEMPTY.
        check("rmdirSync_no_options_nonempty", function() {{
            return throwsCode('ENOTEMPTY', function() {{ fs.rmdirSync("{p_guard_none}"); }}) === 'THREW';
        }});
        // plain rmdir on a file path → ENOTDIR class.
        check("rmdirSync_file_enotdir", function() {{
            return throwsCode('ENOTDIR', function() {{ fs.rmdirSync("{p_file}"); }}) === 'THREW';
        }});
        // recursive:true on a missing path → ENOENT (rm's force defaults false).
        check("rmdirSync_recursive_missing_enoent", function() {{
            return throwsCode('ENOENT', function() {{ fs.rmdirSync("{p_missing}", {{ recursive: true }}); }}) === 'THREW';
        }});

        // promises.rmdir recursive:true resolves and removes the tree. The
        // reaction runs during this eval's post-script RunJobs; the ground
        // truth is asserted from the NEXT eval (fs state after drain).
        globalThis.__promises_rmdir_rejected = false;
        fs.promises.rmdir("{p_tree_promises}", {{ recursive: true }}).then(
            function() {{}},
            function(e) {{ globalThis.__promises_rmdir_rejected = String((e && e.message) || e).substring(0, 80); }}
        );
        results.join(',')
    "#,
        ),
    );
    println!("sync: {}", out);
    let sync_eval = out;

    // Post-RunJobs observation of the promises case (eval drained the
    // microtask queue after capturing the completion value).
    let promises_out = eval_string(
        &mut ctx,
        &format!(
            r#"__promises_rmdir_rejected === false && !fs.existsSync("{p_tree_promises}") ? 'TREE_GONE' : 'BAD:' + __promises_rmdir_rejected + ':' + fs.existsSync("{p_tree_promises}")"#
        ),
    );

    assert!(
        sync_eval.contains("rmdirSync_recursive_tree:PASS"),
        "rmdirSync(tree, {{recursive:true}}) must recursively delete (got: {})",
        sync_eval
    );
    assert!(
        sync_eval.contains("rmdirSync_recursive_empty:PASS"),
        "rmdirSync(empty, {{recursive:true}}) must succeed (got: {})",
        sync_eval
    );
    assert!(
        sync_eval.contains("rmdirSync_recursive_false_nonempty:PASS"),
        "rmdirSync(nonempty, {{recursive:false}}) must still throw ENOTEMPTY (got: {})",
        sync_eval
    );
    assert!(
        sync_eval.contains("rmdirSync_no_options_nonempty:PASS"),
        "rmdirSync(nonempty) must still throw ENOTEMPTY (got: {})",
        sync_eval
    );
    assert!(
        sync_eval.contains("rmdirSync_file_enotdir:PASS"),
        "rmdirSync(file) must throw ENOTDIR-class (got: {})",
        sync_eval
    );
    assert!(
        sync_eval.contains("rmdirSync_recursive_missing_enoent:PASS"),
        "rmdirSync(missing, {{recursive:true}}) must throw ENOENT (got: {})",
        sync_eval
    );
    assert_eq!(
        promises_out, "TREE_GONE",
        "promises.rmdir(tree, {{recursive:true}}) must resolve and remove the tree"
    );

    // --- callback form (async entry point). The recursive ROUTING is the
    //     ported semantics: spawn_fs_async's worker runs the same
    //     remove_dir_all/remove_dir selection as the sync path, so the
    //     filesystem effect is observable from Rust once the worker
    //     finishes.
    //     NOTE on callback DELIVERY (A' route, 2026-08-21): completions now
    //     cross to the JS thread as ConcurrentTask carriers on the pump's
    //     MiniEventLoop (node_fs complete_post), delivered by
    //     timers::drain_and_check's tick — see fs_async_callback_tests for
    //     the callback-state assertions. This test keeps the effect-based
    //     assertion (worker-side filesystem effect, pump-independent) as
    //     the routing discriminator. ---
    let cb_out = eval_string(
        &mut ctx,
        &format!(
            r#"
        var fs = require('fs');
        fs.rmdir("{p_tree_cb}", {{ recursive: true }}, function(err) {{}});
        'scheduled'
    "#
        ),
    );
    assert_eq!(cb_out, "scheduled");

    // Worker thread performs the removal; poll the filesystem from Rust.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < deadline {
        if !tree_cb.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        !tree_cb.exists(),
        "fs.rmdir(tree, {{recursive:true}}, cb): worker must remove the tree (recursive routing in the async entry)"
    );

    let _ = ::std::fs::remove_dir_all(&base);
}

// ============================================================================
// 46a6c3927 — destroy() emits 'close' asynchronously, exactly once
// ============================================================================

#[test]
fn http_destroy_emits_close_asynchronously() {
    let mut ctx = setup_ctx();

    // --- OutgoingMessage.destroy: no synchronous 'close' ---
    let sync_count = eval_string(
        &mut ctx,
        r#"
        var om = new (require('_http_outgoing').OutgoingMessage)();
        globalThis.__om_log = [];
        om.on('close', function() { globalThis.__om_log.push('closed:' + om.closed); });
        om.destroy();
        // Completion value is captured BEFORE RunJobs drains the microtask
        // scheduled by destroy() — this must be 0 (close not yet fired).
        globalThis.__om_log.length
    "#,
    );
    assert_eq!(
        sync_count, "0",
        "OutgoingMessage.destroy() must not emit 'close' synchronously"
    );

    // After the eval's RunJobs: close fired exactly once, closed===true
    // inside the listener (upstream: "res.closed must already be true
    // inside the 'close' listeners").
    let after = eval_string(&mut ctx, r#"__om_log.join()"#);
    assert_eq!(
        after, "closed:true",
        "OutgoingMessage 'close' must fire once after destroy() returns, with closed===true"
    );

    // --- end() then destroy(): finish sync, close async (upstream order
    //     "destroy() -> destroy() returned -> ... -> res.finish -> res.close"
    //     — bao's end() emits finish synchronously, so finish precedes
    //     destroy and close lands after both). ---
    let order_sync = eval_string(
        &mut ctx,
        r#"
        var OM = require('_http_outgoing').OutgoingMessage;
        var om2 = new OM();
        globalThis.__om2_order = [];
        om2.on('finish', function() { globalThis.__om2_order.push('finish'); });
        om2.on('close', function() { globalThis.__om2_order.push('close'); });
        om2.end();
        om2.destroy();
        globalThis.__om2_order.join() // sync completion value
    "#,
    );
    assert_eq!(
        order_sync, "finish",
        "end();destroy(): 'close' must not be in the synchronous event order"
    );
    let order_after = eval_string(&mut ctx, r#"__om2_order.join()"#);
    assert_eq!(
        order_after, "finish,close",
        "end();destroy(): order must be finish then close"
    );

    // --- destroy(err): 'error' fires synchronously, 'close' still async ---
    let err_sync = eval_string(
        &mut ctx,
        r#"
        var OM = require('_http_outgoing').OutgoingMessage;
        var om3 = new OM();
        globalThis.__om3 = { errored: false, closed: false };
        om3.on('error', function() { globalThis.__om3.errored = true; });
        om3.on('close', function() { globalThis.__om3.closed = true; });
        om3.destroy(new Error('boom'));
        globalThis.__om3.errored + ',' + globalThis.__om3.closed
    "#,
    );
    assert_eq!(
        err_sync, "true,false",
        "destroy(err) emits 'error' synchronously but 'close' asynchronously"
    );
    let err_after = eval_string(&mut ctx, r#"__om3.errored + ',' + __om3.closed"#);
    assert_eq!(err_after, "true,true", "destroy(err) close must land after drain");

    // --- destroy() twice: 'close' fires exactly once ---
    let twice = eval_string(
        &mut ctx,
        r#"
        var OM = require('_http_outgoing').OutgoingMessage;
        var om4 = new OM();
        globalThis.__om4_count = 0;
        om4.on('close', function() { globalThis.__om4_count++; });
        om4.destroy();
        om4.destroy();
        Promise.resolve().then(function(){}).then(function(){}); // extra turns
        'spawned'
    "#,
    );
    assert_eq!(twice, "spawned");
    let count = eval_string(&mut ctx, r#"__om4_count"#);
    assert_eq!(count, "1", "double destroy() must emit 'close' exactly once");

    // --- ServerResponse.destroy must emit 'close' at all, asynchronously ---
    let sr_sync = eval_string(
        &mut ctx,
        r#"
        var ServerResponse = require('_http_server').ServerResponse;
        var sr = new ServerResponse(null);
        globalThis.__sr = { sync: 0, async: 0, closedInListener: null, destroyed: false };
        sr.on('close', function() {
            globalThis.__sr.async++;
            globalThis.__sr.closedInListener = sr.closed;
        });
        sr.destroy();
        globalThis.__sr.sync = 1; // reaching here proves destroy() returned without sync close
        globalThis.__sr.destroyed = sr.destroyed;
        'ok'
    "#,
    );
    assert_eq!(sr_sync, "ok", "ServerResponse must be an EventEmitter (on/emit usable)");
    let sr_after = eval_string(
        &mut ctx,
        r#"__sr.async + ',' + __sr.closedInListener + ',' + __sr.destroyed"#,
    );
    assert_eq!(
        sr_after, "1,true,true",
        "ServerResponse.destroy() must emit 'close' once, async, closed/destroyed true (got: {})",
        sr_after
    );

    // --- ServerResponse destroy(err): 'error' sync, 'close' async ---
    let sr_err = eval_string(
        &mut ctx,
        r#"
        var ServerResponse = require('_http_server').ServerResponse;
        var sr2 = new ServerResponse(null);
        globalThis.__sr2 = { errored: false, closed: false };
        sr2.on('error', function() { globalThis.__sr2.errored = true; });
        sr2.on('close', function() { globalThis.__sr2.closed = true; });
        sr2.destroy(new Error('x'));
        globalThis.__sr2.errored + ',' + globalThis.__sr2.closed
    "#,
    );
    assert_eq!(
        sr_err, "true,false",
        "ServerResponse.destroy(err): error sync, close async"
    );
    let sr_err_after = eval_string(&mut ctx, r#"__sr2.errored + ',' + __sr2.closed"#);
    assert_eq!(sr_err_after, "true,true", "ServerResponse destroy(err) close lands after drain");
}
