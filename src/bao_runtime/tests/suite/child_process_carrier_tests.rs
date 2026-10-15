// Sync child_process output-carrier parity (node oracle-pinned, node v24):
// spawnSync/execSync/execFileSync put stdout/stderr up as a raw-bytes Buffer
// when `encoding` is absent (or explicitly 'buffer'), and as a decoded String
// when an explicit encoding is set. The pre-fix always-String shape broke
// `Buffer.isBuffer(stdout)` and lossy-replaced non-utf8 bytes.
// Windows runs the cmd.exe arms; unix runs the /bin/sh arms.

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
#[path = "common/mod.rs"]
mod common;

use common::eval_string;

fn eval_str(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        other => format!("<non-string eval result: {:?}>", other),
    }
}

fn setup() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

#[test]
fn test_spawn_sync_stdout_buffer_carrier() {
    crate::exit_isolation::dispatch_timeout(
        "child_process_carrier_tests::test_spawn_sync_stdout_buffer_carrier",
        test_spawn_sync_stdout_buffer_carrier_body,
    );
}

fn test_spawn_sync_stdout_buffer_carrier_body() {
    let mut ctx = setup();

    // Platform fixture: an echo whose stdout is exactly one short line.
    let (spawn_js, shell_js) = if cfg!(windows) {
        (
            r#"cp.spawnSync("cmd.exe", ["/c", "echo", "carrier-ok"])"#,
            r#"cp.execSync("cmd.exe /c echo carrier-ok")"#,
        )
    } else {
        (
            r#"cp.spawnSync("/bin/sh", ["-c", "printf carrier-ok"])"#,
            r#"cp.execSync("/bin/sh -c 'printf carrier-ok'")"#,
        )
    };

    let js = format!(
        r#"
        var cp = require('child_process');
        var r = {spawn_js};
        var out = [];
        out.push("default-buffer=" + Buffer.isBuffer(r.stdout));
        out.push("default-stderr-buffer=" + Buffer.isBuffer(r.stderr));
        out.push("default-text=" + (r.stdout.toString().indexOf("carrier-ok") !== -1));
        var r8 = cp.spawnSync({cmd_json}, {args_json}, {{ encoding: "utf8" }});
        out.push("utf8-string=" + (typeof r8.stdout === "string"));
        out.push("utf8-text=" + (r8.stdout.indexOf("carrier-ok") !== -1));
        var rb = cp.spawnSync({cmd_json}, {args_json}, {{ encoding: "buffer" }});
        out.push("buffer-enc-buffer=" + Buffer.isBuffer(rb.stdout));
        var e = {shell_js};
        out.push("execsync-buffer=" + Buffer.isBuffer(e));
        out.push("execsync-text=" + (e.toString().indexOf("carrier-ok") !== -1));
        var e8 = cp.execSync({shell_cmd_json}, {{ encoding: "utf8" }});
        out.push("execsync-utf8-string=" + (typeof e8 === "string"));
        var f = cp.execFileSync({cmd_json}, {args_json});
        out.push("execfilesync-buffer=" + Buffer.isBuffer(f));
        out.join(";");
        "#,
        spawn_js = spawn_js,
        shell_js = shell_js,
        cmd_json = if cfg!(windows) { "\"cmd.exe\"" } else { "\"/bin/sh\"" },
        args_json = if cfg!(windows) {
            "[\"/c\", \"echo\", \"carrier-ok\"]"
        } else {
            "[\"-c\", \"printf carrier-ok\"]"
        },
        shell_cmd_json = if cfg!(windows) {
            "\"cmd.exe /c echo carrier-ok\""
        } else {
            "\"/bin/sh -c 'printf carrier-ok'\""
        },
    );

    let got = eval_str(&mut ctx, &js);
    let expect = "default-buffer=true;default-stderr-buffer=true;default-text=true;\
utf8-string=true;utf8-text=true;buffer-enc-buffer=true;\
execsync-buffer=true;execsync-text=true;execsync-utf8-string=true;\
execfilesync-buffer=true";
    assert_eq!(got, expect, "sync output-carrier parity mismatch");
}

#[test]
#[cfg(unix)]
fn test_spawn_sync_raw_bytes_preserved_in_buffer() {
    crate::exit_isolation::dispatch_timeout(
        "child_process_carrier_tests::test_spawn_sync_raw_bytes_preserved_in_buffer",
        test_spawn_sync_raw_bytes_preserved_in_buffer_body,
    );
}

#[cfg(unix)]
fn test_spawn_sync_raw_bytes_preserved_in_buffer_body() {
    let mut ctx = setup();

    // 0xFF must survive verbatim in the default Buffer (the old lossy String
    // path rewrote it to U+FFFD = 0xEF 0xBF 0xBD).
    let js = r#"
        var cp = require('child_process');
        var r = cp.spawnSync("/bin/sh", ["-c", "printf 'A\\377B'"]);
        (Buffer.isBuffer(r.stdout) && r.stdout.length === 3 && r.stdout[1] === 255) ? "OK" : ("BAD " + r.stdout.length + "/" + r.stdout[1]);
    "#;
    let got = eval_str(&mut ctx, js);
    assert_eq!(got, "OK", "raw byte 0xFF must be preserved in the Buffer");
}
