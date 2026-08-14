// @trace TEST-ENG-006-CONSOLE [req:REQ-ENG-006] [level:integration]
// console.* / process.stdout.write stream routing — Node semantics:
//   log/info/debug/dir/table/timeEnd/timeLog/count/group → stdout
//   warn/error/trace/assert (+ timer/counter warnings)      → stderr
// process.stdout.write / process.stderr.write share the same output layer.
//
// Verification is fd-level truth: dup(2) fd 1/2 onto pipes, eval, flush,
// restore, then assert exactly which stream each marker landed on. No
// test-only capture hooks in the output layer — the real path is exercised.

use bao_engine::context::JsContext;
use std::sync::Mutex;

/// fd 1/2 are process-wide: at most one test may hold a redirect at a time,
/// otherwise two concurrent dup2(2)s race and one test's output lands in the
/// other's pipe. This serializes only within this binary — other test
/// binaries are separate processes.
static REDIRECT_LOCK: Mutex<()> = Mutex::new(());

fn make_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    // Initialize Output before any event-loop tick flushes stdout.
    bun_core::output::init_test();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx.set_post_eval_hook(bun_runtime::bun_api::post_eval_drain_then_exit);
    ctx
}

/// Owns the redirect; `restore` puts stdio back and drains both pipes.
/// Explicit read-end fds — pipe(2) makes no ordering guarantee between the
/// two fds, so nothing is inferred from fd numbers.
struct Redirect {
    save_out: i32,
    save_err: i32,
    out_r: i32,
    out_w: i32,
    err_r: i32,
    err_w: i32,
}

impl Redirect {
    /// Redirect fd 1 (and optionally fd 2) into pipes.
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe fn capture(both: bool) -> Redirect {
        let mut out_fds = [0i32; 2];
        let mut err_fds = [0i32; 2];
        assert_eq!(libc::pipe(out_fds.as_mut_ptr()), 0, "pipe(stdout) failed");
        let (err_r, err_w, save_err) = if both {
            assert_eq!(libc::pipe(err_fds.as_mut_ptr()), 0, "pipe(stderr) failed");
            let save = libc::dup(2);
            assert!(save >= 0, "dup(2) failed");
            assert_eq!(libc::dup2(err_fds[1], 2), 2, "dup2 stderr redirect failed");
            (err_fds[0], err_fds[1], save)
        } else {
            (-1, -1, -1)
        };
        let save_out = libc::dup(1);
        assert!(save_out >= 0, "dup(1) failed");
        assert_eq!(libc::dup2(out_fds[1], 1), 1, "dup2 stdout redirect failed");
        Redirect {
            save_out,
            save_err,
            out_r: out_fds[0],
            out_w: out_fds[1],
            err_r,
            err_w,
        }
    }

    /// Restore original stdio, close write ends (EOF for the readers), then
    /// drain both pipes. Returns (stdout_bytes, stderr_bytes).
    #[allow(unsafe_op_in_unsafe_fn)]
    unsafe fn restore(self) -> (Vec<u8>, Vec<u8>) {
        libc::dup2(self.save_out, 1);
        libc::close(self.save_out);
        libc::close(self.out_w);
        let out = drain_fd(self.out_r);
        let err = if self.err_w >= 0 {
            libc::dup2(self.save_err, 2);
            libc::close(self.save_err);
            libc::close(self.err_w);
            drain_fd(self.err_r)
        } else {
            Vec::new()
        };
        (out, err)
    }
}

/// Read `fd` to EOF into a buffer, then close it.
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

/// A single test fn owns the full fd 1+2 redirect — stdio is process-wide, so
/// parallel tests inside this binary must not race the redirect window.
/// Markers are unique strings; stray output from other tests cannot forge them.
#[test]
fn console_routes_stdout_and_stderr_per_node_semantics() {
    let _redirect_guard = REDIRECT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut ctx = make_ctx();
    bun_runtime::clear_exit();

    // SAFETY: stdio fd juggling with checked returns; restore() runs on the
    // error path too (called before any assertion below).
    let redirect = unsafe { Redirect::capture(true) };

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
        console.group('group-label');
        console.groupEnd();
        process.stdout.write('ps-stdout');
        console.error('err-line');
        console.warn('warn-line');
        console.trace('traced');
        console.assert(false, 'assert-msg');
        process.stderr.write('ps-stderr');
        'done'
        "#,
        "<console-output-test>",
    );

    // Drain the output layer buffers into the pipes while the redirect is
    // still active, then restore the real stdio before asserting.
    bun_core::output::flush();

    let (stdout_bytes, stderr_bytes) = unsafe { redirect.restore() };

    assert!(r.is_ok(), "eval must succeed: {:?}", r.err());
    let stdout = String::from_utf8_lossy(&stdout_bytes);
    let stderr = String::from_utf8_lossy(&stderr_bytes);

    // --- stdout family (Node: console.log & friends write to stdout) ---
    assert!(stdout.contains("log-line\n"), "console.log must hit stdout, got: {stdout:?}");
    assert!(stdout.contains("info-line\n"), "console.info must hit stdout, got: {stdout:?}");
    assert!(stdout.contains("debug-line\n"), "console.debug must hit stdout, got: {stdout:?}");
    assert!(stdout.contains("dir-line\n"), "console.dir must hit stdout, got: {stdout:?}");
    assert!(stdout.contains("table-line\n"), "console.table must hit stdout, got: {stdout:?}");
    assert!(stdout.contains("tl: "), "console.timeEnd must hit stdout, got: {stdout:?}");
    assert!(stdout.contains("cnt: 1\n"), "console.count must hit stdout, got: {stdout:?}");
    assert!(stdout.contains("group-label\n"), "console.group must hit stdout, got: {stdout:?}");
    assert!(
        stdout.contains("ps-stdout"),
        "process.stdout.write must hit stdout, got: {stdout:?}"
    );

    // --- stderr family (Node: error/warn/trace/assert write to stderr) ---
    assert!(stderr.contains("err-line\n"), "console.error must hit stderr, got: {stderr:?}");
    assert!(stderr.contains("warn-line\n"), "console.warn must hit stderr, got: {stderr:?}");
    assert!(stderr.contains("Trace"), "console.trace must hit stderr, got: {stderr:?}");
    assert!(
        stderr.contains("Assertion failed"),
        "console.assert failure must hit stderr, got: {stderr:?}"
    );
    assert!(
        stderr.contains("ps-stderr"),
        "process.stderr.write must hit stderr, got: {stderr:?}"
    );

    // --- stream separation: no cross-leak in either direction ---
    assert!(
        !stdout.contains("err-line")
            && !stdout.contains("warn-line")
            && !stdout.contains("ps-stderr"),
        "stderr-family output leaked into stdout: {stdout:?}"
    );
    assert!(
        !stderr.contains("log-line") && !stderr.contains("info-line") && !stderr.contains("ps-stdout"),
        "stdout-family output leaked into stderr: {stderr:?}"
    );
}

/// Non-string chunks must not be silently dropped: Buffer/Uint8Array write
/// their bytes, other values coerce via ToString (`write(v)` ≡ `write(String(v))`).
#[test]
fn process_stdout_write_accepts_non_string_chunks() {
    let _redirect_guard = REDIRECT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut ctx = make_ctx();
    bun_runtime::clear_exit();

    // SAFETY: stdout-only redirect, stderr untouched.
    let redirect = unsafe { Redirect::capture(false) };

    let r = ctx.eval(
        r#"
        process.stdout.write(new Uint8Array([104, 105])); // "hi" as bytes
        process.stdout.write(42);                          // ToString coercion
        process.stdout.write(true);                         // ToString coercion
        'done'
        "#,
        "<console-output-test>",
    );
    bun_core::output::flush();

    let (stdout_bytes, _stderr_untouched) = unsafe { redirect.restore() };
    assert!(r.is_ok(), "eval must succeed: {:?}", r.err());
    // stderr routing is covered fd-level by the routing test above; here only
    // stdout is captured, so no assertion is made about stderr.
    let stdout = String::from_utf8_lossy(&stdout_bytes);
    assert!(
        stdout.contains("hi") && stdout.contains("42") && stdout.contains("true"),
        "Buffer and coerced chunks must all land on stdout, got: {stdout:?}"
    );
}
