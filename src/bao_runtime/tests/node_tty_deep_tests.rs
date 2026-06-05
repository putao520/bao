// @trace TEST-ENG-007-NODE-TTY-DEEP [req:REQ-ENG-007] [level:integration]

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        _ => String::new(),
    }
}

#[test]
fn test_node_tty_deep() {
    bao_runtime::install_exit_handler();
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e).substring(0, 60)); }
        }

        // ========================================
        // §1 require('tty') module basics
        // ========================================
        var tty = require('tty');

        check("tty_exists", function() { return typeof tty !== 'undefined'; });
        check("tty_is_object", function() { return typeof tty === 'object'; });

        // ---- tty.isatty() ----
        check("tty_isatty_exists", function() { return typeof tty.isatty === 'function'; });
        check("tty_isatty_returns_bool_for_0", function() {
            var result = tty.isatty(0);
            return result === true || result === false;
        });
        check("tty_isatty_returns_bool_for_1", function() {
            var result = tty.isatty(1);
            return result === true || result === false;
        });
        check("tty_isatty_returns_bool_for_2", function() {
            var result = tty.isatty(2);
            return result === true || result === false;
        });
        check("tty_isatty_invalid_fd_returns_false", function() {
            var result = tty.isatty(-1);
            return result === false;
        });
        check("tty_isatty_high_fd_returns_false", function() {
            var result = tty.isatty(999);
            return result === false;
        });

        // ========================================
        // §2 tty.ReadStream
        // ========================================
        check("tty_ReadStream_exists", function() { return typeof tty.ReadStream === 'function'; });
        check("tty_ReadStream_is_constructor", function() {
            return typeof tty.ReadStream === 'function';
        });
        check("tty_ReadStream_instance_fd", function() {
            var rs = new tty.ReadStream(0);
            return typeof rs.fd === 'number';
        });
        check("tty_ReadStream_instance_isTTY", function() {
            var rs = new tty.ReadStream(0);
            return typeof rs.isTTY === 'boolean';
        });
        check("tty_ReadStream_instance_isRaw", function() {
            var rs = new tty.ReadStream(0);
            return typeof rs.isRaw === 'boolean';
        });
        check("tty_ReadStream_instance_readable", function() {
            var rs = new tty.ReadStream(0);
            return rs.readable === true;
        });
        check("tty_ReadStream_setRawMode_exists", function() {
            var rs = new tty.ReadStream(0);
            return typeof rs.setRawMode === 'function';
        });
        check("tty_ReadStream_ref_exists", function() {
            var rs = new tty.ReadStream(0);
            return typeof rs.ref === 'function';
        });
        check("tty_ReadStream_unref_exists", function() {
            var rs = new tty.ReadStream(0);
            return typeof rs.unref === 'function';
        });
        check("tty_ReadStream_on_exists", function() {
            var rs = new tty.ReadStream(0);
            return typeof rs.on === 'function';
        });

        // ========================================
        // §3 tty.WriteStream
        // ========================================
        check("tty_WriteStream_exists", function() { return typeof tty.WriteStream === 'function'; });
        check("tty_WriteStream_is_constructor", function() {
            return typeof tty.WriteStream === 'function';
        });
        check("tty_WriteStream_instance_fd", function() {
            var ws = new tty.WriteStream(1);
            return typeof ws.fd === 'number';
        });
        check("tty_WriteStream_instance_isTTY", function() {
            var ws = new tty.WriteStream(1);
            return typeof ws.isTTY === 'boolean';
        });
        check("tty_WriteStream_columns_type", function() {
            var ws = new tty.WriteStream(1);
            // columns may be undefined if not a TTY, or number if TTY
            return typeof ws.columns === 'number' || typeof ws.columns === 'undefined';
        });
        check("tty_WriteStream_rows_type", function() {
            var ws = new tty.WriteStream(1);
            return typeof ws.rows === 'number' || typeof ws.rows === 'undefined';
        });
        check("tty_WriteStream_getWindowSize_exists", function() {
            var ws = new tty.WriteStream(1);
            // getWindowSize only exists if TTY
            return typeof ws.getWindowSize === 'function' || typeof ws.getWindowSize === 'undefined';
        });
        check("tty_WriteStream_clearLine_exists", function() {
            var ws = new tty.WriteStream(1);
            return typeof ws.clearLine === 'function';
        });
        check("tty_WriteStream_clearScreenDown_exists", function() {
            var ws = new tty.WriteStream(1);
            return typeof ws.clearScreenDown === 'function';
        });
        check("tty_WriteStream_cursorTo_exists", function() {
            var ws = new tty.WriteStream(1);
            return typeof ws.cursorTo === 'function';
        });
        check("tty_WriteStream_moveCursor_exists", function() {
            var ws = new tty.WriteStream(1);
            return typeof ws.moveCursor === 'function';
        });
        check("tty_WriteStream_write_exists", function() {
            var ws = new tty.WriteStream(1);
            return typeof ws.write === 'function';
        });

        // ========================================
        // §4 process.stdin/stdout/stderr TTY properties
        // ========================================
        check("process_stdin_exists", function() { return typeof process.stdin !== 'undefined'; });
        check("process_stdin_has_fd", function() { return typeof process.stdin.fd === 'number'; });
        check("process_stdin_has_isTTY", function() { return typeof process.stdin.isTTY === 'boolean'; });
        check("process_stdin_fd_is_0", function() { return process.stdin.fd === 0; });

        check("process_stdout_exists", function() { return typeof process.stdout !== 'undefined'; });
        check("process_stdout_has_fd", function() { return typeof process.stdout.fd === 'number'; });
        check("process_stdout_has_isTTY", function() { return typeof process.stdout.isTTY === 'boolean'; });
        check("process_stdout_fd_is_1", function() { return process.stdout.fd === 1; });

        check("process_stderr_exists", function() { return typeof process.stderr !== 'undefined'; });
        check("process_stderr_has_fd", function() { return typeof process.stderr.fd === 'number'; });
        check("process_stderr_has_isTTY", function() { return typeof process.stderr.isTTY === 'boolean'; });
        check("process_stderr_fd_is_2", function() { return process.stderr.fd === 2; });

        // ========================================
        // §5 isTTY consistency check
        // ========================================
        check("isatty_0_matches_stdin_isTTY", function() {
            return tty.isatty(0) === process.stdin.isTTY;
        });
        check("isatty_1_matches_stdout_isTTY", function() {
            return tty.isatty(1) === process.stdout.isTTY;
        });
        check("isatty_2_matches_stderr_isTTY", function() {
            return tty.isatty(2) === process.stderr.isTTY;
        });

        // ========================================
        // §6 tty.WriteStream columns/rows when TTY
        // ========================================
        check("WriteStream_columns_rows_defined_when_tty", function() {
            var ws = new tty.WriteStream(1);
            if (ws.isTTY) {
                return typeof ws.columns === 'number' && typeof ws.rows === 'number';
            }
            return true; // skip if not a TTY
        });
        check("WriteStream_getWindowSize_when_tty", function() {
            var ws = new tty.WriteStream(1);
            if (ws.isTTY && typeof ws.getWindowSize === 'function') {
                var size = ws.getWindowSize();
                return Array.isArray(size) && size.length === 2;
            }
            return true; // skip if not a TTY or no getWindowSize
        });

        // ========================================
        // §7 Module keys enumeration
        // ========================================
        check("tty_module_keys_count", function() {
            var keys = Object.getOwnPropertyNames(tty);
            return keys.length >= 3;
        });
        check("tty_module_has_isatty", function() {
            return Object.hasOwnProperty.call(tty, 'isatty');
        });
        check("tty_module_has_ReadStream", function() {
            return Object.hasOwnProperty.call(tty, 'ReadStream');
        });
        check("tty_module_has_WriteStream", function() {
            return Object.hasOwnProperty.call(tty, 'WriteStream');
        });

        results.join("|")
    "#);

    let mut pass = 0;
    let mut fail = 0;
    for item in results.split('|') {
        if item.contains(" PASS") {
            pass += 1;
        } else if item.contains(" FAIL") || item.contains(" ERR") {
            fail += 1;
            eprintln!("FAILED: {}", item);
        }
    }
    assert_eq!(fail, 0, "node_tty deep tests had {} failures", fail);
    assert!(pass >= 15, "Expected at least 15 passes, got {}", pass);

    std::mem::forget(ctx);
}
