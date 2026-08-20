// @trace TEST-ENG-003-HOSTFN-CONSOLE [req:REQ-ENG-003] [level:integration]
// Bootstrap console (bun_sm::host_fn::install_console — the console every
// bare bao_engine realm gets before any global_setup) stream routing, Node
// semantics: log/info/debug/dir/table/timeEnd/count → stdout; warn/error/
// trace/assert (+ timer warnings) → stderr.
//
// fd-level truth: dup(2) fd 1/2 onto pipes, eval, flush, restore, assert.

use bao_engine::context::JsContext;
use std::sync::Mutex;

/// fd 1/2 are process-wide: at most one test may hold a redirect at a time.
static REDIRECT_LOCK: Mutex<()> = Mutex::new(());

/// Owns the redirect; `restore` puts stdio back and drains both pipes.
struct Redirect {
    save_out: i32,
    save_err: i32,
    out_r: i32,
    out_w: i32,
    err_r: i32,
    err_w: i32,
}

impl Redirect {
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe fn capture() -> Redirect {
        let mut out_fds = [0i32; 2];
        let mut err_fds = [0i32; 2];
        assert_eq!(libc::pipe(out_fds.as_mut_ptr()), 0, "pipe(stdout) failed");
        assert_eq!(libc::pipe(err_fds.as_mut_ptr()), 0, "pipe(stderr) failed");
        let save_out = libc::dup(1);
        let save_err = libc::dup(2);
        assert!(save_out >= 0 && save_err >= 0, "dup failed");
        assert_eq!(libc::dup2(out_fds[1], 1), 1, "dup2 stdout redirect failed");
        assert_eq!(libc::dup2(err_fds[1], 2), 2, "dup2 stderr redirect failed");
        Redirect {
            save_out,
            save_err,
            out_r: out_fds[0],
            out_w: out_fds[1],
            err_r: err_fds[0],
            err_w: err_fds[1],
        }
    }

    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe fn restore(self) -> (Vec<u8>, Vec<u8>) {
        libc::dup2(self.save_out, 1);
        libc::close(self.save_out);
        libc::dup2(self.save_err, 2);
        libc::close(self.save_err);
        libc::close(self.out_w);
        let out = drain_fd(self.out_r);
        libc::close(self.err_w);
        let err = drain_fd(self.err_r);
        (out, err)
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn drain_fd(fd: i32) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = libc::read(fd, chunk.as_mut_ptr() as *mut libc::c_void, chunk.len());
        if n <= 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n as usize]);
    }
    libc::close(fd);
    buf
}

/// Bare-context global_setup: installs ONLY the bootstrap console — this is
/// exactly the surface non-bao_runtime embedders see.
unsafe fn setup_bootstrap_console(
    cx: &mut mozjs::context::JSContext,
    global: mozjs::rust::Handle<*mut mozjs::jsapi::JSObject>,
) {
    bao_engine::host_fn::install_console(cx, global);
}

#[test]
fn bootstrap_console_routes_streams_per_node_semantics() {
    let _redirect_guard = REDIRECT_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    // Publish the output-layer stream slots before any console write adopts
    // them (configure_thread debug_asserts this).
    bun_core::output::init_test();

    let mut ctx = JsContext::for_test().expect("Failed to create JsContext");
    ctx.set_global_setup(setup_bootstrap_console);

    // SAFETY: stdio fd juggling with checked returns; restore() runs before
    // any assertion below.
    let redirect = unsafe { Redirect::capture() };

    let r = ctx.eval(
        r#"
        console.log('log-line');
        console.info('info-line');
        console.debug('debug-line');
        console.dir('dir-line');
        console.table('table-line');
        console.time('tl');
        console.timeEnd('tl');
        console.count('cnt');
        console.error('err-line');
        console.warn('warn-line');
        console.trace('traced');
        console.assert(false, 'assert-msg');
        'done'
        "#,
        "<console-routing-test>",
    );

    // Drain output-layer buffers into the pipes, then restore real stdio.
    bun_core::output::flush();

    let (stdout_bytes, stderr_bytes) = unsafe { redirect.restore() };

    assert!(r.is_ok(), "eval must succeed: {:?}", r.err());
    let stdout = String::from_utf8_lossy(&stdout_bytes);
    let stderr = String::from_utf8_lossy(&stderr_bytes);

    // --- stdout family ---
    assert!(stdout.contains("log-line\n"), "console.log must hit stdout, got: {stdout:?}");
    assert!(stdout.contains("info-line\n"), "console.info must hit stdout, got: {stdout:?}");
    assert!(stdout.contains("debug-line\n"), "console.debug must hit stdout, got: {stdout:?}");
    assert!(stdout.contains("dir-line\n"), "console.dir must hit stdout, got: {stdout:?}");
    assert!(stdout.contains("table-line\n"), "console.table must hit stdout, got: {stdout:?}");
    assert!(stdout.contains("tl: "), "console.timeEnd must hit stdout, got: {stdout:?}");
    assert!(stdout.contains("cnt: 1\n"), "console.count must hit stdout, got: {stdout:?}");

    // --- stderr family ---
    assert!(stderr.contains("err-line\n"), "console.error must hit stderr, got: {stderr:?}");
    assert!(stderr.contains("warn-line\n"), "console.warn must hit stderr, got: {stderr:?}");
    assert!(stderr.contains("Trace"), "console.trace must hit stderr, got: {stderr:?}");
    assert!(
        stderr.contains("Assertion failed: assert-msg"),
        "console.assert failure must hit stderr with Node message format, got: {stderr:?}"
    );

    // --- stream separation: no cross-leak in either direction ---
    assert!(
        !stdout.contains("err-line") && !stdout.contains("warn-line"),
        "stderr-family output leaked into stdout: {stdout:?}"
    );
    assert!(
        !stderr.contains("log-line") && !stderr.contains("info-line"),
        "stdout-family output leaked into stderr: {stderr:?}"
    );
}

#[test]
fn bootstrap_console_timeend_warns_on_missing_label_via_stderr() {
    let _redirect_guard = REDIRECT_LOCK.lock().unwrap_or_else(|e| e.into_inner());

    bun_core::output::init_test();

    let mut ctx = JsContext::for_test().expect("Failed to create JsContext");
    ctx.set_global_setup(setup_bootstrap_console);

    let redirect = unsafe { Redirect::capture() };

    // timeEnd without a matching time() — Node emits a console.warn-style
    // diagnostic; it must be visible on stderr, not swallowed into a log macro.
    let r = ctx.eval("console.timeEnd('never-started'); 'done'", "<console-routing-test>");

    bun_core::output::flush();

    let (stdout_bytes, stderr_bytes) = unsafe { redirect.restore() };

    assert!(r.is_ok(), "eval must succeed: {:?}", r.err());
    let stdout = String::from_utf8_lossy(&stdout_bytes);
    let stderr = String::from_utf8_lossy(&stderr_bytes);
    assert!(
        stderr.contains("Warning: No such label 'never-started'"),
        "missing-timer diagnostic must hit stderr, got: {stderr:?}"
    );
    assert!(
        !stdout.contains("Warning"),
        "diagnostic leaked into stdout: {stdout:?}"
    );
}
