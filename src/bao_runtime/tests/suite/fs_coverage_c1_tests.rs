// @trace TEST-ENG-7-FS-C1 [req:REQ-ENG-007] [level:integration]
//
// C1 line-coverage wave for src/bao_runtime/src/node_fs.rs (Node.js fs 适配面).
// 缺口面 = 错误路径分支 / 边界校验 / promises 变体 / 少用 API / watch/stream 面。
// 既有 fs 族(fs_deep/fs_async_callback/fs_buffer_write/fs_upstream_semantics/
// fs_watch_events)已覆盖主路径;本文件只补缺口分支,每条断言观测既有行为。
//
// 分支触达对照表见任务回报(逐 Err 臂/边界 if vs 测试清单)。

use bao_engine::context::JsContext;
#[path = "common/mod.rs"]
mod common;

use common::eval_string;
use common::eval_string_full;
use common::setup_ctx;

use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Escape a path for interpolation into a JS double-quoted string literal.
fn js_escape(p: &std::path::Path) -> String {
    p.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

/// Unique per-test temp dir (nextest runs each test in its own process, but
/// keep names collision-free anyway).
fn tmp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bao_fs_c1_{tag}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Boolean-predicate pump delegate: the unified suite pump
/// (`common::pump_until`) with the historical `== "true"` settle plus the
/// `PumpStuck` no-progress tripwire (a frozen predicate now fails at the 2s
/// window instead of silently burning the deadline). Returns the final
/// predicate value so the caller can fail with a precise message.
fn pump_until(ctx: &mut JsContext, predicate_js: &str, deadline_ms: u64) -> bool {
    let final_value = common::pump_until(
        ctx,
        predicate_js,
        Duration::from_millis(deadline_ms),
        eval_string,
        |v| v == "true",
    );
    final_value == "true"
}

/// Evaluate a check-batch body that returns `label PASS|FAIL|ERR:...` joined
/// by `|`; assert every check passed and hand back the full transcript.
fn run_checks(ctx: &mut JsContext, body: &str) -> Vec<String> {
    let out = eval_string(ctx, body);
    let results: Vec<String> = out.split('|').map(|s| s.trim().to_string()).collect();
    let failures: Vec<String> = results
        .iter()
        .filter(|i| i.contains(" FAIL") || i.contains(" ERR:"))
        .cloned()
        .collect();
    assert!(
        failures.is_empty(),
        "fs_coverage_c1 checks had {} failure(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
    results
}

/// Shared JS scaffolding for check batches: result collector + error-code
/// probe helper.
const CHECK_PRELUDE: &str = r#"
    var fs = require('fs');
    var R = [];
    function ck(n, f) {
        try { var ok = f(); R.push(n + (ok ? " PASS" : " FAIL")); }
        catch (e) { R.push(n + " ERR:" + String((e && e.code) || (e && e.message) || e).substring(0, 90)); }
    }
    function codeof(f) { try { f(); return "NOTHROW"; } catch (e) { return (e && e.code) || "THREW"; } }
    function msgof(f) { try { f(); return "NOTHROW"; } catch (e) { return String((e && e.message) || e); } }
"#;

// ---------------------------------------------------------------------------
// 1. Sync error paths — the ENOENT / EEXIST / ENOTDIR / EINVAL / ENAMETOOLONG
//    family (every Err arm that funnels through io_error_code).
// ---------------------------------------------------------------------------

#[test]
fn c1_sync_error_paths_code_family() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("errcodes");
    let missing = dir.join("definitely_absent_c1.txt");
    let plain = dir.join("plain.txt");
    std::fs::write(&plain, "x").unwrap();
    let sub = dir.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    let link_path = dir.join("link_exists");
    let _ = std::fs::remove_file(&link_path);
    std::os::unix::fs::symlink(&plain, &link_path).unwrap();
    let long = dir.join("a".repeat(5000));

    let body = format!(
        r#"
        {prelude}
        var miss = "{miss}";
        var file = "{file}";
        var dirp = "{dirp}";
        var linkp = "{linkp}";
        var longp = "{longp}";

        ck("readFileSync_enoent", function() {{ return codeof(function() {{ fs.readFileSync(miss); }}) === "ENOENT"; }});
        ck("statSync_enoent", function() {{ return codeof(function() {{ fs.statSync(miss); }}) === "ENOENT"; }});
        ck("lstatSync_enoent", function() {{ return codeof(function() {{ fs.lstatSync(miss); }}) === "ENOENT"; }});
        ck("unlinkSync_enoent", function() {{ return codeof(function() {{ fs.unlinkSync(miss); }}) === "ENOENT"; }});
        ck("rmdirSync_enotdir", function() {{ return codeof(function() {{ fs.rmdirSync(file); }}) === "ENOTDIR"; }});
        ck("readdirSync_enoent", function() {{ return codeof(function() {{ fs.readdirSync(miss); }}) === "ENOENT"; }});
        ck("readdirSync_enotdir", function() {{ return codeof(function() {{ fs.readdirSync(file); }}) === "ENOTDIR"; }});
        ck("renameSync_enoent_src", function() {{ return codeof(function() {{ fs.renameSync(miss, file + ".r"); }}) === "ENOENT"; }});
        ck("copyFileSync_enoent_src", function() {{ return codeof(function() {{ fs.copyFileSync(miss, file + ".c"); }}) === "ENOENT"; }});
        ck("chmodSync_enoent", function() {{ return codeof(function() {{ fs.chmodSync(miss, 0o644); }}) === "ENOENT"; }});
        ck("realpathSync_enoent", function() {{ return codeof(function() {{ fs.realpathSync(miss); }}) === "ENOENT"; }});
        ck("linkSync_enoent_src", function() {{ return codeof(function() {{ fs.linkSync(miss, file + ".l"); }}) === "ENOENT"; }});
        ck("openSync_enoent_r", function() {{ return codeof(function() {{ fs.openSync(miss, 'r'); }}) === "ENOENT"; }});
        ck("truncateSync_enoent", function() {{ return codeof(function() {{ fs.truncateSync(miss); }}) === "ENOENT"; }});
        ck("statfsSync_enoent", function() {{ return codeof(function() {{ fs.statfsSync(miss); }}) === "ENOENT"; }});
        ck("mkdirSync_eexist_plain", function() {{ return codeof(function() {{ fs.mkdirSync(dirp); }}) === "EEXIST"; }});
        ck("symlinkSync_eexist", function() {{ return codeof(function() {{ fs.symlinkSync(file, linkp); }}) === "EEXIST"; }});
        ck("readlinkSync_einval_regular", function() {{ return codeof(function() {{ fs.readlinkSync(file); }}) === "EINVAL"; }});
        ck("readFileSync_enametoolong", function() {{ return codeof(function() {{ fs.readFileSync(longp); }}) === "ENAMETOOLONG"; }});
        ck("cpSync_missing_reports_message", function() {{
            var m = msgof(function() {{ fs.cpSync(miss, file + ".cp"); }});
            return m.indexOf("cpSync") !== -1 && m.indexOf("ENOENT") === -1;
        }});
        R.join("|")
    "#,
        prelude = CHECK_PRELUDE,
        miss = js_escape(&missing),
        file = js_escape(&plain),
        dirp = js_escape(&sub),
        linkp = js_escape(&link_path),
        longp = js_escape(&long),
    );
    let results = run_checks(&mut ctx, &body);
    assert!(
        results.len() >= 20,
        "expected >= 20 checks, got {}",
        results.len()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// 2. get_path_arg boundary arms — missing / non-string path arguments throw.
// ---------------------------------------------------------------------------

#[test]
fn c1_sync_missing_or_bad_path_arg_throws() {
    let mut ctx = setup_ctx();
    let body = format!(
        r#"
        {prelude}
        ck("readFileSync_noargs_throws", function() {{
            return msgof(function() {{ fs.readFileSync(); }}).indexOf("Missing path") !== -1;
        }});
        ck("statSync_number_path_throws", function() {{
            return msgof(function() {{ fs.statSync(42); }}).indexOf("must be of type string") !== -1;
        }});
        ck("unlinkSync_noargs_throws", function() {{
            return msgof(function() {{ fs.unlinkSync(); }}).indexOf("Missing path") !== -1;
        }});
        ck("mkdirSync_noargs_throws", function() {{
            return msgof(function() {{ fs.mkdirSync(); }}).indexOf("Missing path") !== -1;
        }});
        ck("openSync_noargs_throws", function() {{
            return msgof(function() {{ fs.openSync(); }}).indexOf("Missing path") !== -1;
        }});
        ck("rmdirSync_number_path_throws", function() {{
            return msgof(function() {{ fs.rmdirSync(null); }}).indexOf("must be of type string") !== -1;
        }});
        ck("copyFileSync_second_arg_missing", function() {{
            return msgof(function() {{ fs.copyFileSync("/tmp"); }}).indexOf("Missing path") !== -1;
        }});
        ck("renameSync_second_arg_missing", function() {{
            return msgof(function() {{ fs.renameSync("/tmp"); }}).indexOf("Missing path") !== -1;
        }});
        R.join("|")
    "#,
        prelude = CHECK_PRELUDE,
    );
    let results = run_checks(&mut ctx, &body);
    assert_eq!(results.len(), 8);
}

// ---------------------------------------------------------------------------
// 3. readFileSync encoding faces — hex / base64 / latin1 / binary / unknown /
//    {encoding} object form / multibyte UTF-8 (mojibake class).
// ---------------------------------------------------------------------------

#[test]
fn c1_sync_encoding_faces() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("enc");
    let f = dir.join("bytes.bin");
    // "AB你" in UTF-8.
    std::fs::write(&f, [0x41u8, 0x42, 0xE4, 0xBD, 0xA0]).unwrap();

    let body = format!(
        r#"
        {prelude}
        var p = "{p}";
        var raw = Buffer.from([0x41, 0x42, 0xE4, 0xBD, 0xA0]);

        ck("hex_face", function() {{ return fs.readFileSync(p, "hex") === "4142e4bda0"; }});
        ck("base64_face_roundtrip", function() {{
            var b64 = fs.readFileSync(p, "base64");
            return Buffer.from(b64, "base64").toString("hex") === "4142e4bda0";
        }});
        ck("latin1_face_byte_per_char", function() {{
            var s = fs.readFileSync(p, "latin1");
            return s.length === 5 && s.charCodeAt(2) === 0xE4 && s.charCodeAt(4) === 0xA0;
        }});
        ck("binary_alias_latin1", function() {{
            return fs.readFileSync(p, "binary") === fs.readFileSync(p, "latin1");
        }});
        ck("unknown_encoding_falls_back_utf8", function() {{
            return fs.readFileSync(p, "xyzzy-unknown") === "AB你";
        }});
        ck("object_encoding_form", function() {{
            return fs.readFileSync(p, {{ encoding: "hex" }}) === "4142e4bda0";
        }});
        ck("utf8_multibyte_no_mojibake", function() {{
            return fs.readFileSync(p, "utf8") === "AB你";
        }});
        ck("utf8_underscore_alias", function() {{
            return fs.readFileSync(p, "utf-8") === "AB你";
        }});
        ck("no_encoding_returns_buffer", function() {{
            var b = fs.readFileSync(p);
            return Buffer.isBuffer(b) && b.length === 5 && b[2] === 0xE4;
        }});
        R.join("|")
    "#,
        prelude = CHECK_PRELUDE,
        p = js_escape(&f),
    );
    let results = run_checks(&mut ctx, &body);
    assert_eq!(results.len(), 9);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// 4. openSync flag-string faces — parse_open_flags branches ('r'/'r+'/'w'/
//    'w+'/'a'/'a+' + numeric int32 flags on unix).
// ---------------------------------------------------------------------------

#[test]
fn c1_open_sync_flag_faces() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("flags");
    let existing = dir.join("exists.txt");
    std::fs::write(&existing, "0123456789").unwrap();

    // Numeric-flags face is unix-shaped (raw libc bits reach libc::open).
    let unix_numeric = if cfg!(unix) {
        r#"
        ck("numeric_o_wronly_creat", function() {
            var np = dir + "/num.txt";
            var fd = fs.openSync(np, 0x41); // O_WRONLY|O_CREAT on Linux
            fs.writeSync(fd, "n");
            fs.closeSync(fd);
            return fs.readFileSync(np, "utf8") === "n";
        });
        ck("numeric_zero_is_rdonly", function() {
            var fd = fs.openSync(file, 0);
            var b = Buffer.alloc(2);
            var n = fs.readSync(fd, b, 0, 2, 0);
            fs.closeSync(fd);
            return n === 2 && b.toString("utf8") === "zz";
        });
        "#
    } else {
        ""
    };

    let body = format!(
        r#"
        {prelude}
        var dir = "{dir}";
        var file = "{file}";

        ck("flag_r_reads_existing", function() {{
            var fd = fs.openSync(file, "r");
            var b = Buffer.alloc(4);
            var n = fs.readSync(fd, b, 0, 4, 0);
            fs.closeSync(fd);
            return n === 4 && b.toString("utf8") === "0123";
        }});
        ck("flag_rplus_read_write", function() {{
            var fd = fs.openSync(file, "r+");
            fs.writeSync(fd, "AB", 0);
            fs.closeSync(fd);
            return fs.readFileSync(file, "utf8") === "AB23456789";
        }});
        ck("flag_w_creates_and_truncates", function() {{
            var fd = fs.openSync(file, "w");
            fs.writeSync(fd, "zz");
            fs.closeSync(fd);
            return fs.readFileSync(file, "utf8") === "zz";
        }});
        ck("flag_wplus_creat_readwrite", function() {{
            var p2 = dir + "/wplus.txt";
            var fd = fs.openSync(p2, "w+");
            fs.writeSync(fd, "q");
            var b = Buffer.alloc(1);
            fs.readSync(fd, b, 0, 1, 0);
            fs.closeSync(fd);
            return b.toString("utf8") === "q";
        }});
        ck("flag_a_appends_preserving", function() {{
            var fd = fs.openSync(file, "a");
            fs.writeSync(fd, "+app");
            fs.closeSync(fd);
            return fs.readFileSync(file, "utf8") === "zz+app";
        }});
        ck("flag_aplus_append_and_read", function() {{
            var fd = fs.openSync(file, "a+");
            fs.writeSync(fd, "!");
            fs.closeSync(fd);
            var s = fs.readFileSync(file, "utf8");
            return s === "zz+app!" && fs.openSync(file, "a+") >= 0;
        }});
        ck("empty_flag_string_rdonly_enoent", function() {{
            return codeof(function() {{ fs.openSync(dir + "/nope.txt", ""); }}) === "ENOENT";
        }});
        ck("unknown_flag_chars_enoent", function() {{
            return codeof(function() {{ fs.openSync(dir + "/nope.txt", "qq"); }}) === "ENOENT";
        }});
        {unix_numeric}
        R.join("|")
    "#,
        prelude = CHECK_PRELUDE,
        dir = js_escape(&dir),
        file = js_escape(&existing),
        unix_numeric = unix_numeric,
    );
    let results = run_checks(&mut ctx, &body);
    let want = if cfg!(unix) { 10 } else { 8 };
    assert_eq!(results.len(), want);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// 5. readSync/writeSync boundary arms — buffer type checks, offset/length
//    bounds, zero-length early return, pread vs cursor, EBADF on dead fd.
// ---------------------------------------------------------------------------

#[test]
fn c1_read_write_sync_boundary_faces() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("rwbound");
    let f = dir.join("data.bin");
    std::fs::write(&f, b"0123456789").unwrap();

    let body = format!(
        r#"
        {prelude}
        var p = "{p}";
        var fd = fs.openSync(p, "r");

        ck("readsync_pread_keeps_cursor", function() {{
            var b = Buffer.alloc(4);
            var n1 = fs.readSync(fd, b, 0, 4, 0);
            var n2 = fs.readSync(fd, b, 0, 4, 0);
            return n1 === 4 && n2 === 4 && b.toString("utf8") === "0123";
        }});
        ck("readsync_null_position_advances_cursor", function() {{
            var b = Buffer.alloc(3);
            var n = fs.readSync(fd, b, 0, 3, null);
            var b2 = Buffer.alloc(2);
            var n2 = fs.readSync(fd, b2, 0, 2, null);
            return n === 3 && b.toString("utf8") === "012" && n2 === 2 && b2.toString("utf8") === "34";
        }});
        ck("readsync_offset_subregion", function() {{
            var b = Buffer.alloc(6);
            b.write("------");
            var n = fs.readSync(fd, b, 2, 4, 5);
            return n === 4 && b.toString("utf8", 2, 6) === "5678" && b.toString("utf8", 0, 2) === "--";
        }});
        ck("readsync_zero_length_returns_zero", function() {{
            var b = Buffer.alloc(4);
            return fs.readSync(fd, b, 0, 0, 0) === 0;
        }});
        ck("readsync_string_buffer_throws", function() {{
            return msgof(function() {{ fs.readSync(fd, "notabuffer", 0, 1, 0); }}).indexOf("Buffer or Uint8Array") !== -1;
        }});
        ck("readsync_arraybuffer_nonview_throws", function() {{
            return msgof(function() {{ fs.readSync(fd, new ArrayBuffer(8), 0, 1, 0); }}).indexOf("Buffer or Uint8Array") !== -1;
        }});
        ck("readsync_negative_offset_throws", function() {{
            return msgof(function() {{ fs.readSync(fd, Buffer.alloc(4), -1, 1, 0); }}).indexOf("out of bounds") !== -1;
        }});
        ck("readsync_offset_past_len_throws", function() {{
            return msgof(function() {{ fs.readSync(fd, Buffer.alloc(4), 5, 1, 0); }}).indexOf("out of bounds") !== -1;
        }});
        ck("readsync_length_beyond_buffer_throws", function() {{
            return msgof(function() {{ fs.readSync(fd, Buffer.alloc(4), 0, 999, 0); }}).indexOf("beyond buffer") !== -1;
        }});
        fs.closeSync(fd);

        // node v24.19.0 oracle: writeSync(fd, buf, offset=2, length=2,
        // position=0) writes only buf[2..4] ("CD") at absolute position 0 and
        // leaves the fd cursor untouched (pwrite) → n=2; the cursor form then
        // writes "XY" at cursor 0 → n2=2; file becomes "XY23456789".
        ck("writesync_offset_length_position", function() {{
            var wfd = fs.openSync(p, "r+");
            var n = fs.writeSync(wfd, Buffer.from("ABCDEF"), 2, 2, 0);
            var n2 = fs.writeSync(wfd, Buffer.from("XY"));
            fs.closeSync(wfd);
            var c = fs.readFileSync(p, "utf8");
            if (n === 2 && n2 === 2 && c === "XY23456789") return true;
            throw new Error("n=" + n + " n2=" + n2 + " content=" + c);
        }});
        ck("writesync_string_position_form", function() {{
            fs.writeFileSync(p, "ABCDEFGHIJ");
            var wfd = fs.openSync(p, "r+");
            var n = fs.writeSync(wfd, "XY", 3);
            fs.closeSync(wfd);
            var c = fs.readFileSync(p, "utf8");
            if (n === 2 && c === "ABCXYFGHIJ") return true;
            throw new Error("n=" + n + " content=" + c);
        }});
        ck("readsync_badfd_throws", function() {{
            var b = Buffer.alloc(2);
            // node: EBADF (C11 wave: io_error_code gained the EBADF arm).
            return codeof(function() {{ fs.readSync(987654, b, 0, 2, 0); }}) === "EBADF";
        }});
        ck("writesync_badfd_throws", function() {{
            // C11 wave: EBADF arm (was "ERR").
            return codeof(function() {{ fs.writeSync(987654, Buffer.from("x"), 0, 1, 0); }}) === "EBADF";
        }});
        R.join("|")
    "#,
        prelude = CHECK_PRELUDE,
        p = js_escape(&f),
    );
    let results = run_checks(&mut ctx, &body);
    assert_eq!(results.len(), 13);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// 6. fd-oriented sync ops — fchmod/fchown/fdatasync/fsync/ftruncate/futimes/
//    lchmod/lchown success + EBADF/ENOENT error arms.
// ---------------------------------------------------------------------------

#[test]
fn c1_fd_ops_sync_faces() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("fdops");
    let f = dir.join("target.txt");
    std::fs::write(&f, "truncate-me").unwrap();

    let body = format!(
        r#"
        {prelude}
        var p = "{p}";
        var fd = fs.openSync(p, "r+");

        ck("fchmodSync_sets_mode", function() {{
            fs.fchmodSync(fd, 0o600);
            return (fs.statSync(p).mode & 0o777) === 0o600;
        }});
        ck("fchownSync_minus1_noop", function() {{
            fs.fchownSync(fd, -1, -1);
            return true;
        }});
        ck("fdatasyncSync_ok", function() {{ fs.fdatasyncSync(fd); return true; }});
        ck("fsyncSync_ok", function() {{ fs.fsyncSync(fd); return true; }});
        ck("ftruncateSync_len", function() {{
            fs.ftruncateSync(fd, 4);
            return fs.statSync(p).size === 4;
        }});
        ck("futimesSync_sets_mtime", function() {{
            fs.futimesSync(fd, 1000, 2000);
            return fs.statSync(p).mtimeMs === 2000000;
        }});
        fs.closeSync(fd);

        ck("closeSync_badfd_throws", function() {{
            // node: EBADF (C11 wave: io_error_code gained the EBADF arm —
            // refreshed from the pre-fix "ERR" pin per the oracle).
            return codeof(function() {{ fs.closeSync(987654); }}) === "EBADF";
        }});
        ck("fsyncSync_badfd_throws", function() {{
            // C11 wave: EBADF arm (was "ERR").
            return codeof(function() {{ fs.fsyncSync(987654); }}) === "EBADF";
        }});
        ck("fchmodSync_badfd_throws", function() {{
            // C11 wave: EBADF arm (was "ERR").
            return codeof(function() {{ fs.fchmodSync(987654, 0o600); }}) === "EBADF";
        }});
        ck("futimesSync_badfd_throws", function() {{
            // C11 wave: EBADF arm (was "ERR").
            return codeof(function() {{ fs.futimesSync(987654, 0, 0); }}) === "EBADF";
        }});
        ck("lchmodSync_fallback_chmod", function() {{
            fs.lchmodSync(p, 0o640);
            return (fs.statSync(p).mode & 0o777) === 0o640;
        }});
        ck("lchmodSync_enoent", function() {{
            return codeof(function() {{ fs.lchmodSync(p + ".nope", 0o600); }}) === "ENOENT";
        }});
        ck("lchownSync_minus1_noop", function() {{
            fs.lchownSync(p, -1, -1);
            return true;
        }});
        ck("lchownSync_enoent", function() {{
            return codeof(function() {{ fs.lchownSync(p + ".nope", 0, 0); }}) === "ENOENT";
        }});
        R.join("|")
    "#,
        prelude = CHECK_PRELUDE,
        p = js_escape(&f),
    );
    let results = run_checks(&mut ctx, &body);
    assert_eq!(results.len(), 14);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// 7. readvSync / writevSync — scatter-gather roundtrip + EBADF arm.
// ---------------------------------------------------------------------------

#[test]
fn c1_readv_writev_sync_faces() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("iov");
    let f = dir.join("iov.bin");

    let body = format!(
        r#"
        {prelude}
        var p = "{p}";

        ck("writevSync_two_buffers", function() {{
            var fd = fs.openSync(p, "w");
            var n = fs.writevSync(fd, [Buffer.from("AA"), Buffer.from("BB")]);
            fs.closeSync(fd);
            return n === 4 && fs.readFileSync(p, "utf8") === "AABB";
        }});
        ck("readvSync_two_buffers_position", function() {{
            var fd = fs.openSync(p, "r");
            var b1 = Buffer.alloc(2);
            var b2 = Buffer.alloc(2);
            // node v24.19.0 oracle: readvSync fills the caller's Buffers in
            // order (file holds "AABB" from writevSync above) → b1="AA",
            // b2="BB", n=4.
            var n = fs.readvSync(fd, [b1, b2], 0);
            fs.closeSync(fd);
            if (n === 4 && b1.toString() === "AA" && b2.toString() === "BB") return true;
            throw new Error("n=" + n + " b1=" + b1.toString("hex") + " b2=" + b2.toString("hex"));
        }});
        ck("readvSync_position_short_read", function() {{
            var fd = fs.openSync(p, "r");
            var b1 = Buffer.alloc(2);
            var b2 = Buffer.alloc(2);
            // node oracle: position 1 over "AABB" reads 3 bytes → b1="AB",
            // b2="B\0", n=3 (short read stops at EOF).
            var n = fs.readvSync(fd, [b1, b2], 1);
            fs.closeSync(fd);
            if (n === 3 && b1.toString("hex") === "4142" && b2.toString("hex") === "4200") return true;
            throw new Error("n=" + n + " b1=" + b1.toString("hex") + " b2=" + b2.toString("hex"));
        }});
        ck("readvSync_badfd_throws", function() {{
            // node: EBADF; bao io_error_code has no EBADF arm → "ERR".
            // C11 wave: EBADF arm (was "ERR").
            return codeof(function() {{ fs.readvSync(987654, [Buffer.alloc(2)], 0); }}) === "EBADF";
        }});
        ck("writevSync_badfd_throws", function() {{
            // C11 wave: EBADF arm (was "ERR").
            return codeof(function() {{ fs.writevSync(987654, [Buffer.from("x")]); }}) === "EBADF";
        }});
        R.join("|")
    "#,
        prelude = CHECK_PRELUDE,
        p = js_escape(&f),
    );
    let results = run_checks(&mut ctx, &body);
    assert_eq!(results.len(), 5);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// 8. mkdtemp + opendir/Dir object faces (readSync / closeSync / iterator /
//    error arms / async callback form).
// ---------------------------------------------------------------------------

#[test]
fn c1_mkdtemp_opendir_dir_faces() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("mkdtemp_dir");
    let work = dir.join("work");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(work.join("a.txt"), "a").unwrap();
    std::fs::write(work.join("b.txt"), "b").unwrap();
    let plain = dir.join("plain.txt");
    std::fs::write(&plain, "x").unwrap();

    // Async cb faces go through a separate scheduled round.
    let sched = format!(
        r#"
        var fs = require('fs');
        globalThis.__c1dir = {{ n: 0, want: 1, log: {{}} }};
        fs.opendir("{work}", function(err, d) {{
            globalThis.__c1dir.log.opendir = {{
                err: err ? String(err.code || err.message) : null,
                isObj: !!d && typeof d === "object",
                path: d ? d.path : null,
            }};
            globalThis.__c1dir.n++;
        }});
        'scheduled'
    "#,
        work = js_escape(&work),
    );
    let sched_out = eval_string_full(&mut ctx, &sched);
    assert_eq!(sched_out, "scheduled", "scheduling eval: {}", sched_out);
    assert!(
        pump_until(
            &mut ctx,
            "globalThis.__c1dir.n >= globalThis.__c1dir.want",
            10_000
        ),
        "opendir cb never fired"
    );
    let opendir_ok = eval_string(
        &mut ctx,
        r#"(function(){ var L = globalThis.__c1dir.log.opendir;
             return L.err === null && L.isObj && typeof L.path === "string"; })()"#,
    ) == "true";

    let body = format!(
        r#"
        {prelude}
        var base = "{base}";
        var work = "{work}";
        var plain = "{plain}";

        ck("mkdtempSync_template_expanded", function() {{
            var d = fs.mkdtempSync(base + "/tpl-XXXXXX");
            var ok = d.indexOf(base + "/tpl-") === 0 && d.length > (base + "/tpl-").length;
            return ok && fs.statSync(d).isDirectory();
        }});
        ck("mkdtempSync_empty_prefix_einval", function() {{
            return codeof(function() {{ fs.mkdtempSync(""); }}) === "EINVAL";
        }});
        ck("mkdtempSync_no_template_still_makes_dir", function() {{
            var d = fs.mkdtempSync(base + "/plain");
            return fs.statSync(d).isDirectory();
        }});
        ck("opendirSync_path_and_readSync", function() {{
            var d = fs.opendirSync(work);
            var names = [];
            var e;
            while ((e = d.readSync()) !== null) names.push(e.name);
            d.closeSync();
            return d.path === work && names.indexOf("a.txt") !== -1 && names.indexOf("b.txt") !== -1;
        }});
        ck("dir_readSync_after_close_throws", function() {{
            var d = fs.opendirSync(work);
            d.closeSync();
            return msgof(function() {{ d.readSync(); }}).indexOf("already closed") !== -1;
        }});
        ck("opendirSync_on_file_throws", function() {{
            return msgof(function() {{ fs.opendirSync(plain); }}).indexOf("not a directory") !== -1;
        }});
        ck("opendirSync_enoent", function() {{
            return codeof(function() {{ fs.opendirSync(base + "/nope_dir"); }}) === "ENOENT";
        }});
        ck("mkdtempSync_enoent_parent", function() {{
            return codeof(function() {{ fs.mkdtempSync(base + "/no-parent-dir/xXXXXXX"); }}) === "ENOENT";
        }});
        R.join("|")
    "#,
        prelude = CHECK_PRELUDE,
        base = js_escape(&dir),
        work = js_escape(&work),
        plain = js_escape(&plain),
    );
    let results = run_checks(&mut ctx, &body);
    assert_eq!(results.len(), 8);
    assert!(opendir_ok, "fs.opendir callback face failed");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// 9. statfsSync / globSync / glob cb / openAsBlob faces.
// ---------------------------------------------------------------------------

#[test]
fn c1_statfs_glob_openasblob_faces() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("statfs_glob");
    std::fs::write(dir.join("g_one.txt"), "1").unwrap();
    std::fs::write(dir.join("g_two.md"), "2").unwrap();
    std::fs::create_dir_all(dir.join("g_sub")).unwrap();
    std::fs::write(dir.join("g_sub").join("g_three.txt"), "3").unwrap();
    let blob_f = dir.join("blob.bin");
    std::fs::write(&blob_f, b"blobbytes").unwrap();

    // openAsBlob promise faces (success size + ENOENT rejection).
    let sched = format!(
        r#"
        var fs = require('fs');
        globalThis.__c1blob = {{ n: 0, want: 2, log: {{}} }};
        fs.openAsBlob("{blob}").then(function(b) {{
            globalThis.__c1blob.log.ok = {{ size: b ? b.size : -1 }};
            globalThis.__c1blob.n++;
        }}, function(e) {{
            globalThis.__c1blob.log.ok = {{ err: String((e && e.code) || e) }};
            globalThis.__c1blob.n++;
        }});
        fs.openAsBlob("{miss}").then(function(b) {{
            globalThis.__c1blob.log.miss = {{ unexpected: true }};
            globalThis.__c1blob.n++;
        }}, function(e) {{
            globalThis.__c1blob.log.miss = {{ code: String((e && e.code) || e) }};
            globalThis.__c1blob.n++;
        }});
        'scheduled'
    "#,
        blob = js_escape(&blob_f),
        miss = js_escape(&dir.join("absent.bin")),
    );
    let sched_out = eval_string_full(&mut ctx, &sched);
    assert_eq!(sched_out, "scheduled", "scheduling eval: {}", sched_out);
    assert!(
        pump_until(
            &mut ctx,
            "globalThis.__c1blob.n >= globalThis.__c1blob.want",
            10_000
        ),
        "openAsBlob promises never settled"
    );
    let blob_ok = eval_string(
        &mut ctx,
        r#"(function(){ var L = globalThis.__c1blob.log;
             return L.ok && L.ok.size === 9 && L.miss && L.miss.code === "ENOENT"; })()"#,
    ) == "true";

    let body = format!(
        r#"
        {prelude}
        var dir = "{dir}";

        ck("statfsSync_fields_numeric", function() {{
            var sf = fs.statfsSync(dir);
            return typeof sf === "object" && sf.bsize > 0 && sf.blocks > 0
                && typeof sf.bfree === "number" && typeof sf.files === "number";
        }});
        ck("globSync_cwd_match", function() {{
            var m = fs.globSync("*.txt", {{ cwd: dir }});
            return m.indexOf("g_one.txt") !== -1;
        }});
        ck("globSync_no_match_empty", function() {{
            return fs.globSync("*.zzzz", {{ cwd: dir }}).length === 0;
        }});
        ck("globSync_star_md_only", function() {{
            var m = fs.globSync("*.md", {{ cwd: dir }});
            return m.length === 1 && m[0] === "g_two.md";
        }});
        R.join("|")
    "#,
        prelude = CHECK_PRELUDE,
        dir = js_escape(&dir),
    );

    // glob callback face: schedules a worker delivery.
    let glob_sched = format!(
        r#"
        var fs = require('fs');
        globalThis.__c1glob = {{ n: 0, want: 1, log: {{}} }};
        fs.glob("*.txt", {{ cwd: "{dir}" }}, function(err, names) {{
            globalThis.__c1glob.log.g = {{
                err: err ? String(err.code || err.message) : null,
                has: !!names && names.indexOf("g_one.txt") !== -1,
            }};
            globalThis.__c1glob.n++;
        }});
        'scheduled'
    "#,
        dir = js_escape(&dir),
    );
    assert_eq!(eval_string(&mut ctx, &glob_sched), "scheduled");
    assert!(
        pump_until(
            &mut ctx,
            "globalThis.__c1glob.n >= globalThis.__c1glob.want",
            10_000
        ),
        "glob cb never fired"
    );
    let glob_ok = eval_string(
        &mut ctx,
        r#"(function(){ var L = globalThis.__c1glob.log.g; return L.err === null && L.has; })()"#,
    ) == "true";

    let results = run_checks(&mut ctx, &body);
    assert_eq!(results.len(), 4);
    assert!(blob_ok, "openAsBlob faces failed");
    assert!(glob_ok, "fs.glob callback face failed");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// 10. fs.promises success batch — every promise method's resolve arm observed
//     through side effects (async/await, drained by RunJobs).
// ---------------------------------------------------------------------------

#[test]
fn c1_promises_success_batch() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("prom_ok");
    let f = dir.join("f.txt");

    let sched = format!(
        r#"
        var fsp = require('fs/promises');
        globalThis.__c1p = {{ started: false, done: false, R: [] }};
        (async function() {{
            globalThis.__c1p.started = true;
            var R = globalThis.__c1p.R;
            function ck(n, ok) {{ R.push(n + (ok ? " PASS" : " FAIL")); }}
            var base = "{base}";
            var p = "{p}";
            try {{
                await fsp.writeFile(p, "hello promises");
                ck("writeFile", true);
                ck("readFile_utf8", (await fsp.readFile(p, "utf8")) === "hello promises");
                await fsp.appendFile(p, "+more");
                ck("appendFile", (await fsp.readFile(p, "utf8")) === "hello promises+more");
                var st = await fsp.stat(p);
                ck("stat_size", st.size === "hello promises+more".length && typeof st.isFile === "function");
                var ls = await fsp.lstat(p);
                ck("lstat", ls.isFile());
                await fsp.mkdir(base + "/made");
                ck("mkdir", require("fs").existsSync(base + "/made"));
                var names = await fsp.readdir(base);
                ck("readdir_array", Array.isArray(names) && names.indexOf("made") !== -1);
                await fsp.rename(p, base + "/renamed.txt");
                ck("rename", require("fs").existsSync(base + "/renamed.txt"));
                await fsp.copyFile(base + "/renamed.txt", base + "/copied.txt");
                ck("copyFile", (await fsp.readFile(base + "/copied.txt", "utf8")) === "hello promises+more");
                await fsp.chmod(base + "/copied.txt", 0o600);
                ck("chmod_mode", (require("fs").statSync(base + "/copied.txt").mode & 0o777) === 0o600);
                await fsp.access(base + "/copied.txt");
                ck("access_resolves", true);
                var rp = await fsp.realpath(base + "/copied.txt");
                ck("realpath_abs", typeof rp === "string" && rp.indexOf("/") === 0);
                await fsp.symlink(base + "/copied.txt", base + "/lnk");
                ck("symlink", (await fsp.readlink(base + "/lnk")) === base + "/copied.txt");
                await fsp.link(base + "/copied.txt", base + "/hard");
                ck("link", (await fsp.readFile(base + "/hard", "utf8")) === "hello promises+more");
                await fsp.truncate(base + "/hard", 5);
                ck("truncate", (await fsp.stat(base + "/hard")).size === 5);
                await fsp.utimes(base + "/hard", 1000, 2000);
                ck("utimes_mtime", (await fsp.stat(base + "/hard")).mtimeMs === 2000000);
                var td = await fsp.mkdtemp(base + "/pt-XXXXXX");
                ck("mkdtemp", require("fs").statSync(td).isDirectory());
                var sf = await fsp.statfs(base);
                ck("statfs", typeof sf === "object" && sf.blocks > 0);
                await fsp.rm(base + "/copied.txt");
                ck("rm", !require("fs").existsSync(base + "/copied.txt"));
                await fsp.rmdir(base + "/made");
                ck("rmdir", !require("fs").existsSync(base + "/made"));
            }} catch (e) {{
                R.push("unexpected ERR:" + String((e && e.code) || (e && e.message) || e));
            }}
            globalThis.__c1p.done = true;
        }})();
        'scheduled'
    "#,
        base = js_escape(&dir),
        p = js_escape(&f),
    );
    let sched_out = eval_string_full(&mut ctx, &sched);
    assert_eq!(sched_out, "scheduled", "scheduling eval: {}", sched_out);
    assert!(
        pump_until(&mut ctx, "globalThis.__c1p.done === true", 15_000),
        "promises batch never completed"
    );
    let out = eval_string(&mut ctx, "globalThis.__c1p.R.join('|')");
    let failures: Vec<&str> = out
        .split('|')
        .filter(|i| i.contains(" FAIL") || i.contains(" ERR:"))
        .collect();
    assert!(
        failures.is_empty(),
        "promises success batch failures:\n{}",
        failures.join("\n")
    );
    assert!(out.contains("writeFile PASS") && out.contains("rmdir PASS"));
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// 11. fs.promises rejection batch — reject_fs_error code delivery.
// ---------------------------------------------------------------------------

#[test]
fn c1_promises_reject_batch() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("prom_rej");
    let plain = dir.join("plain.txt");
    std::fs::write(&plain, "x").unwrap();

    let sched = format!(
        r#"
        var fsp = require('fs/promises');
        globalThis.__c1r = {{ done: false, R: [] }};
        (async function() {{
            var R = globalThis.__c1r.R;
            async function code(promise) {{
                try {{ await promise; return "RESOLVED"; }}
                catch (e) {{ return (e && e.code) || "REJECTED"; }}
            }}
            var miss = "{miss}";
            try {{
                R.push("readFile " + ((await code(fsp.readFile(miss))) === "ENOENT" ? "PASS" : "FAIL"));
                R.push("stat " + ((await code(fsp.stat(miss))) === "ENOENT" ? "PASS" : "FAIL"));
                R.push("lstat " + ((await code(fsp.lstat(miss))) === "ENOENT" ? "PASS" : "FAIL"));
                R.push("unlink " + ((await code(fsp.unlink(miss))) === "ENOENT" ? "PASS" : "FAIL"));
                R.push("chmod " + ((await code(fsp.chmod(miss, 0o600))) === "ENOENT" ? "PASS" : "FAIL"));
                R.push("realpath " + ((await code(fsp.realpath(miss))) === "ENOENT" ? "PASS" : "FAIL"));
                R.push("access " + ((await code(fsp.access(miss))) === "ENOENT" ? "PASS" : "FAIL"));
                R.push("readlink " + ((await code(fsp.readlink("{plain}"))) === "EINVAL" ? "PASS" : "FAIL"));
                R.push("truncate " + ((await code(fsp.truncate(miss))) === "ENOENT" ? "PASS" : "FAIL"));
                R.push("utimes " + ((await code(fsp.utimes(miss, 0, 0))) === "ENOENT" ? "PASS" : "FAIL"));
                R.push("readdir " + ((await code(fsp.readdir(miss))) === "ENOENT" ? "PASS" : "FAIL"));
                R.push("rm " + ((await code(fsp.rm(miss))) === "ENOENT" ? "PASS" : "FAIL"));
                R.push("rmdir " + ((await code(fsp.rmdir(miss))) === "ENOENT" ? "PASS" : "FAIL"));
                R.push("copyFile " + ((await code(fsp.copyFile(miss, "{plain}.c"))) === "ENOENT" ? "PASS" : "FAIL"));
                R.push("open " + ((await code(fsp.open(miss, "r"))) === "ENOENT" ? "PASS" : "FAIL"));
            }} catch (e) {{
                R.push("unexpected ERR:" + String((e && e.code) || (e && e.message) || e));
            }}
            globalThis.__c1r.done = true;
        }})();
        'scheduled'
    "#,
        miss = js_escape(&dir.join("absent_c1.txt")),
        plain = js_escape(&plain),
    );
    let sched_out = eval_string_full(&mut ctx, &sched);
    assert_eq!(sched_out, "scheduled", "scheduling eval: {}", sched_out);
    assert!(
        pump_until(&mut ctx, "globalThis.__c1r.done === true", 15_000),
        "promises reject batch never completed"
    );
    let out = eval_string(&mut ctx, "globalThis.__c1r.R.join('|')");
    let failures: Vec<&str> = out
        .split('|')
        .filter(|i| i.contains(" FAIL") || i.contains(" ERR:"))
        .collect();
    assert!(
        failures.is_empty(),
        "promises reject batch failures:\n{}",
        failures.join("\n")
    );
    assert!(out.contains("readFile PASS") && out.contains("open PASS"));
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// 12. FileHandle (fs.promises.open) full method surface + closed-fd rejection.
// ---------------------------------------------------------------------------

#[test]
fn c1_promises_filehandle_surface() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("fh");
    let f = dir.join("fh.txt");

    let sched = format!(
        r#"
        var fsp = require('fs/promises');
        globalThis.__c1fh = {{ done: false, R: [] }};
        (async function() {{
            var R = globalThis.__c1fh.R;
            function ck(n, ok) {{ R.push(n + (ok ? " PASS" : " FAIL")); }}
            var p = "{p}";
            try {{
                var fh = await fsp.open(p, "w+");
                ck("open_returns_handle", typeof fh.fd === "number" && fh.fd >= 0);
                await fh.writeFile("fh body");
                ck("fh_writeFile", (await fsp.readFile(p, "utf8")) === "fh body");
                // K wave (node v24.19.0 oracle): fh.writeFile advances the
                // cursor to EOF, and fh.readFile reads from the CURRENT
                // cursor — so it resolves "" here (probe: readFile after
                // writeFile === "").
                var eofRead = await fh.readFile("utf8");
                ck("fh_readFile_cursor_at_eof", eofRead === "");
                // C1 #13 fixed (kept): a fresh handle reads from 0 and the
                // encoding argument decodes a string; the no-argument form
                // resolves a Buffer.
                var fhro = await fsp.open(p, "r");
                var fhBody = await fhro.readFile("utf8");
                ck("fh_readFile_encoding", typeof fhBody === "string" && fhBody === "fh body");
                var fhro2 = await fsp.open(p, "r");
                var fhBuf = await fhro2.readFile();
                ck("fh_readFile_buffer", Buffer.isBuffer(fhBuf) && fhBuf.toString("utf8") === "fh body");
                await fhro.close();
                await fhro2.close();
                var r = await fh.read(Buffer.alloc(2), 0, 2, 0);
                ck("fh_read_bytesRead", r.bytesRead === 2 && Buffer.isBuffer(r.buffer) && r.buffer.toString("utf8") === "fh");
                var w = await fh.write("XX", 0, 0, 0); // fh_write: data=arg0, position=arg3
                ck("fh_write_bytesWritten", w.bytesWritten === 2);
                ck("fh_stat_size", (await fh.stat()).size === "fh body".length);
                await fh.truncate(2);
                ck("fh_truncate", (await fh.stat()).size === 2);
                await fh.appendFile("+tail");
                ck("fh_appendFile", (await fsp.readFile(p, "utf8")) === "XX+tail");
                await fh.datasync();
                await fh.sync();
                ck("fh_sync_faces", true);
                await fh.chmod(0o600);
                ck("fh_chmod", (require("fs").statSync(p).mode & 0o777) === 0o600);
                await fh.utimes(1000, 2000);
                ck("fh_utimes", (await fh.stat()).mtimeMs === 2000000);
                var fd_saved = fh.fd;
                await fh.close();
                ck("fh_close", true);
                var closedErr = "NOREJECT";
                try {{ await fh.readFile(); }} catch (e) {{ closedErr = String((e && e.message) || e); }}
                // Observed: post-close read rejects via read(-1) → EBADF message.
                ck("fh_read_after_close_rejects", closedErr.indexOf("Bad file descriptor") !== -1);
                // Observed: second close resolves (idempotent, no rejection).
                var doubleClose = "RESOLVED";
                try {{ await fh.close(); }} catch (e) {{ doubleClose = "REJECTED"; }}
                ck("fh_double_close_resolves", doubleClose === "RESOLVED");
                // Saved fd is now dead: raw promise read/write must reject.
                var deadR = "RESOLVED";
                try {{ await fsp.read(fd_saved, Buffer.alloc(2), 0, 2, 0); }} catch (e) {{ deadR = "REJECTED"; }}
                ck("fsp_read_dead_fd_rejects", deadR === "REJECTED");
            }} catch (e) {{
                R.push("unexpected ERR:" + String((e && e.code) || (e && e.message) || e));
            }}
            globalThis.__c1fh.done = true;
        }})();
        'scheduled'
    "#,
        p = js_escape(&f),
    );
    let sched_out = eval_string_full(&mut ctx, &sched);
    assert_eq!(sched_out, "scheduled", "scheduling eval: {}", sched_out);
    assert!(
        pump_until(&mut ctx, "globalThis.__c1fh.done === true", 15_000),
        "filehandle batch never completed"
    );
    let out = eval_string(&mut ctx, "globalThis.__c1fh.R.join('|')");
    let failures: Vec<&str> = out
        .split('|')
        .filter(|i| i.contains(" FAIL") || i.contains(" ERR:"))
        .collect();
    assert!(
        failures.is_empty(),
        "filehandle batch failures:\n{}",
        failures.join("\n")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// 13. fs/promises subpath identity + FileHandle constructor form.
// ---------------------------------------------------------------------------

#[test]
fn c1_fsp_subpath_and_filehandle_ctor() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("fspath");
    let f = dir.join("ctor.txt");
    std::fs::write(&f, "ctor body").unwrap();

    let sched = format!(
        r#"
        var fsp = require('fs/promises');
        globalThis.__c1ctor = {{ done: false, R: [] }};
        (async function() {{
            var R = globalThis.__c1ctor.R;
            function ck(n, ok) {{ R.push(n + (ok ? " PASS" : " FAIL")); }}
            var p = "{p}";
            try {{
                ck("subpath_readFile", (await fsp.readFile(p, "utf8")) === "ctor body");
                ck("subpath_is_fresh_builtin", typeof fsp.readFile === "function" && typeof fsp.FileHandle === "function");
                ck("promises_namespace_lacks_ctor", require("fs").promises.FileHandle === undefined);
                var fd = require("fs").openSync(p, "r");
                // Observed shape: the native is a plain function — `new` throws
                // "not a constructor"; call form returns the handle object.
                var fh = fsp.FileHandle(fd);
                ck("ctor_fd_prop", fh.fd === fd);
                var ctorBody = await fh.readFile("utf8");
                ck("ctor_readFile_encoding", typeof ctorBody === "string" && ctorBody === "ctor body");
                await fh.close();
                var threw = "";
                try {{ fsp.FileHandle(-1); }} catch (e) {{ threw = String(e.message || e); }}
                ck("ctor_negative_fd_throws", threw.indexOf("valid file descriptor") !== -1);
            }} catch (e) {{
                R.push("unexpected ERR:" + String((e && e.code) || (e && e.message) || e));
            }}
            globalThis.__c1ctor.done = true;
        }})();
        'scheduled'
    "#,
        p = js_escape(&f),
    );
    let sched_out = eval_string_full(&mut ctx, &sched);
    assert_eq!(sched_out, "scheduled", "scheduling eval: {}", sched_out);
    assert!(
        pump_until(&mut ctx, "globalThis.__c1ctor.done === true", 15_000),
        "ctor batch never completed"
    );
    let out = eval_string(&mut ctx, "globalThis.__c1ctor.R.join('|')");
    let failures: Vec<&str> = out
        .split('|')
        .filter(|i| i.contains(" FAIL") || i.contains(" ERR:"))
        .collect();
    assert!(
        failures.is_empty(),
        "fsp subpath batch failures:\n{}",
        failures.join("\n")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// 14. Async callback write-side batch — worker completions delivered with
//     null err + observable side effects.
// ---------------------------------------------------------------------------

#[test]
fn c1_async_cb_write_side_batch() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("cbw");
    let src = dir.join("src.txt");
    std::fs::write(&src, "to-rename").unwrap();
    let chmod_target = dir.join("chmod.txt");
    std::fs::write(&chmod_target, "c").unwrap();
    let trunc_target = dir.join("tr.txt");
    std::fs::write(&trunc_target, "0123456789").unwrap();
    let ut_target = dir.join("ut.txt");
    std::fs::write(&ut_target, "ut").unwrap();
    let lt_target = dir.join("lt.txt");
    std::fs::write(&lt_target, "lt").unwrap();
    let rm_target = dir.join("gone.txt");
    std::fs::write(&rm_target, "g").unwrap();
    let sym_target = dir.join("symt.txt");
    std::fs::write(&sym_target, "s").unwrap();

    let sched = format!(
        r#"
        var fs = require('fs');
        globalThis.__c1w = {{ n: 0, want: 13, log: {{}} }};
        function got(k, err) {{
            globalThis.__c1w.log[k] = err ? String(err.code || err.message || err) : null;
            globalThis.__c1w.n++;
        }}
        var d = "{d}";
        // K wave: fs.writeFile requires a callback (node ERR_INVALID_ARG_TYPE
        // otherwise); the callback face is now the only async face.
        fs.writeFile(d + "/w.txt", "written", function(e) {{ got("writeFile", e); }});
        // K wave: writeFile is now genuinely async (cb face) — give appendFile
        // its own file so completion order cannot truncate the append away.
        fs.appendFile(d + "/w2.txt", "+ap", function(e) {{ got("appendFile", e); }});
        fs.mkdir(d + "/made", function(e) {{ got("mkdir", e); }});
        fs.rename("{src}", d + "/renamed.txt", function(e) {{ got("rename", e); }});
        fs.chmod("{chmod}", 0o600, function(e) {{ got("chmod", e); }});
        fs.chown("{chmod}", -1, -1, function(e) {{ got("chown", e); }});
        fs.truncate("{trunc}", 4, function(e) {{ got("truncate", e); }});
        fs.utimes("{ut}", 1000, 2000, function(e) {{ got("utimes", e); }});
        fs.lutimes("{lt}", 100, 200, function(e) {{ got("lutimes", e); }});
        fs.symlink("{symt}", d + "/lnk", function(e) {{ got("symlink", e); }});
        fs.link("{symt}", d + "/hard", function(e) {{ got("link", e); }});
        fs.copyFile("{symt}", d + "/cop.txt", function(e) {{ got("copyFile", e); }});
        fs.rm("{rm}", function(e) {{ got("rm", e); }});
        'scheduled'
    "#,
        d = js_escape(&dir),
        src = js_escape(&src),
        chmod = js_escape(&chmod_target),
        trunc = js_escape(&trunc_target),
        ut = js_escape(&ut_target),
        lt = js_escape(&lt_target),
        rm = js_escape(&rm_target),
        symt = js_escape(&sym_target),
    );
    let sched_out = eval_string_full(&mut ctx, &sched);
    assert_eq!(
        sched_out, "scheduled",
        "write-side scheduling eval: {}",
        sched_out
    );
    assert!(
        pump_until(
            &mut ctx,
            "globalThis.__c1w.n >= globalThis.__c1w.want",
            20_000
        ),
        "write-side callbacks incomplete (n={} log={})",
        eval_string(&mut ctx, "String(globalThis.__c1w.n)"),
        eval_string(&mut ctx, "Object.keys(globalThis.__c1w.log).join(',')")
    );

    let body = format!(
        r#"
        {prelude}
        var d = "{d}";
        ck("all_errs_null", function() {{
            var L = globalThis.__c1w.log;
            var keys = ["writeFile","appendFile","mkdir","rename","chmod","chown","truncate","utimes","lutimes","symlink","link","copyFile","rm"];
            for (var i = 0; i < keys.length; i++) if (L[keys[i]] !== null) return false;
            return true;
        }});
        ck("write_append_content", function() {{
            return fs.readFileSync(d + "/w.txt", "utf8") === "written" &&
                   fs.readFileSync(d + "/w2.txt", "utf8") === "+ap";
        }});
        ck("mkdir_side_effect", function() {{ return fs.statSync(d + "/made").isDirectory(); }});
        ck("rename_side_effect", function() {{ return fs.existsSync(d + "/renamed.txt"); }});
        ck("chmod_side_effect", function() {{ return (fs.statSync(d + "/chmod.txt").mode & 0o777) === 0o600; }});
        ck("truncate_side_effect", function() {{ return fs.statSync(d + "/tr.txt").size === 4; }});
        ck("utimes_side_effect", function() {{ return fs.statSync(d + "/ut.txt").mtimeMs === 2000000; }});
        ck("lutimes_side_effect", function() {{ return fs.statSync(d + "/lt.txt").mtimeMs === 200000; }});
        ck("symlink_side_effect", function() {{ return fs.readlinkSync(d + "/lnk") === d + "/symt.txt"; }});
        ck("link_side_effect", function() {{ return fs.readFileSync(d + "/hard", "utf8") === "s"; }});
        ck("copyfile_side_effect", function() {{ return fs.readFileSync(d + "/cop.txt", "utf8") === "s"; }});
        ck("rm_side_effect", function() {{ return !fs.existsSync(d + "/gone.txt"); }});
        R.join("|")
    "#,
        prelude = CHECK_PRELUDE,
        d = js_escape(&dir),
    );
    let results = run_checks(&mut ctx, &body);
    assert_eq!(results.len(), 12);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// 15. Async callback read-side batch — data faces (buffer/encodings/stat
//     objects/dirents/exists-boolean/mkdtemp path).
// ---------------------------------------------------------------------------

#[test]
fn c1_async_cb_read_side_batch() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("cbr");
    let f = dir.join("r.txt");
    std::fs::write(&f, b"readbytes").unwrap();
    let sub = dir.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    let link = dir.join("rlnk");
    std::os::unix::fs::symlink(&f, &link).unwrap();

    let sched = format!(
        r#"
        var fs = require('fs');
        globalThis.__c1r2 = {{ n: 0, want: 11, log: {{}} }};
        function got(k, err, val) {{
            globalThis.__c1r2.log[k] = {{
                err: err ? String(err.code || err.message || err) : null,
                val: (val === undefined || val === null) ? "∅" : val,
            }};
            globalThis.__c1r2.n++;
        }}
        var f = "{f}";
        var d = "{d}";
        var lnk = "{lnk}";
        fs.readFile(f, "utf8", function(e, v) {{ got("readFile_utf8", e, v); }});
        fs.readFile(f, function(e, v) {{
            var kind;
            try {{ kind = Buffer.isBuffer(v) ? "isbuffer" : ("notbuffer:" + typeof v + ":" + (v && v.length) + ":" + (v && v.constructor && v.constructor.name)); }}
            catch (ie) {{ kind = "probe-err:" + ie; }}
            got("readFile_buffer", e, kind);
        }});
        fs.readFile(f, "hex", function(e, v) {{ got("readFile_hex", e, v); }});
        fs.stat(f, function(e, st) {{ got("stat", e, st ? "size=" + st.size + ",isfile=" + st.isFile() : "∅"); }});
        fs.lstat(lnk, function(e, st) {{ got("lstat", e, st ? String(st.isSymbolicLink()) : "∅"); }});
        fs.readdir(d, {{ withFileTypes: true }}, function(e, ents) {{
            var verdict = "empty";
            if (ents && ents.length) {{
                var byName = {{}};
                for (var i = 0; i < ents.length; i++) byName[ents[i].name] = ents[i];
                var fOk = byName["r.txt"] && byName["r.txt"].isFile && byName["r.txt"].isFile() === true;
                var dOk = byName["sub"] && byName["sub"].isDirectory && byName["sub"].isDirectory() === true;
                verdict = (fOk && dOk) ? "ok" : "bad";
            }}
            got("readdir_dirents", e, verdict);
        }});
        fs.realpath(f, function(e, v) {{ got("realpath", e, typeof v === "string" ? "abs" : "∅"); }});
        fs.readlink(lnk, function(e, v) {{ got("readlink", e, v); }});
        fs.mkdtemp(d + "/at-XXXXXX", function(e, v) {{ got("mkdtemp", e, v ? "made" : "∅"); }});
        // fs.exists face: callback receives the existence boolean (node shape
        // cb(exists); bao delivers it on the generic (err, value) slot).
        fs.exists(f, function(a, b) {{ got("exists_true", null, (a === true || b === true) ? "yes" : "no"); }});
        fs.statfs(d, function(e, sf) {{ got("statfs", e, sf && sf.blocks > 0 ? "ok" : "∅"); }});
        'scheduled'
    "#,
        f = js_escape(&f),
        d = js_escape(&dir),
        lnk = js_escape(&link),
    );
    let sched_out = eval_string_full(&mut ctx, &sched);
    assert_eq!(sched_out, "scheduled", "scheduling eval: {}", sched_out);
    assert!(
        pump_until(
            &mut ctx,
            "globalThis.__c1r2.n >= globalThis.__c1r2.want",
            20_000
        ),
        "read-side callbacks incomplete (n={})",
        eval_string(&mut ctx, "String(globalThis.__c1r2.n)")
    );

    let body = r#"
        var R = [];
        function ck(n, f) {
            try { var ok = f(); R.push(n + (ok ? " PASS" : " FAIL")); }
            catch (e) { R.push(n + " ERR:" + String((e && e.message) || e).substring(0, 80)); }
        }
        var L = globalThis.__c1r2.log;
        ck("all_errs_null", function() {
            for (var k in L) if (L[k].err !== null) return false;
            return true;
        });
        ck("readFile_utf8_value", function() { return L.readFile_utf8.val === "readbytes"; });
        // Observed parity gap: the async callback face of readFile with NO
        // encoding delivers a utf8 STRING, while the sync face returns a
        // Buffer (node: both are Buffer). Registered in the wave report;
        // assertion pins the observed delivery (content intact, length 9).
        ck("readFile_no_encoding_string_face", function() {
            var v = L.readFile_buffer.val;
            if (typeof v === "string" && v.indexOf("notbuffer:string:9:String") === 0) return true;
            throw new Error("probe=" + v);
        });
        ck("readFile_hex_value", function() { return L.readFile_hex.val === "726561646279746573"; });
        ck("stat_payload", function() { return L.stat.val === "size=9,isfile=true"; });
        ck("lstat_symlink", function() { return L.lstat.val === "true"; });
        ck("readdir_dirents_shape", function() { return L.readdir_dirents.val === "ok"; });
        ck("realpath_abs", function() { return L.realpath.val === "abs"; });
        ck("readlink_target", function() { return typeof L.readlink.val === "string" && L.readlink.val.indexOf("r.txt") !== -1; });
        ck("mkdtemp_made", function() { return L.mkdtemp.val === "made"; });
        ck("exists_true_face", function() { return L.exists_true.val === "yes"; });
        ck("statfs_payload", function() { return L.statfs.val === "ok"; });
        R.join("|")
    "#;
    let results = run_checks(&mut ctx, body);
    assert_eq!(results.len(), 12);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// 16. Async fd-flow batch — open/read/write/fstat/ftruncate/fsync/fdatasync/
//     fchmod/futimes/readv/writev callbacks on real fds.
// ---------------------------------------------------------------------------

#[test]
fn c1_async_cb_fd_flow() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("cbfd");
    // Read-face and write-face ops run on worker threads concurrently — keep
    // them on separate files so no content assertion can race.
    let f_read = dir.join("fd_read.txt");
    let f_write = dir.join("fd_write.txt");
    std::fs::write(&f_read, b"0123456789").unwrap();
    std::fs::write(&f_write, b"0123456789").unwrap();

    // Round A: schedule fd-op callbacks on fds opened synchronously.
    // Observed-shape notes: fs.readv/fs.writev async faces delegate to
    // fs.read/fs.write (buffers-array face not spread) — read length falls
    // back to 65536 and array payloads extract no bytes; assertions pin
    // delivery + the actual returned counts, not the node iovec semantics.
    let sched = format!(
        r#"
        var fs = require('fs');
        globalThis.__c1fd = {{ n: 0, want: 8, log: {{}} }};
        function got(k, err, val) {{
            globalThis.__c1fd.log[k] = (err ? "ERR:" + String(err.code || err.message) : "ok") + "|" + (val === undefined ? "∅" : val);
            globalThis.__c1fd.n++;
        }}
        var f1 = "{f1}";
        var f2 = "{f2}";
        var rfd = fs.openSync(f1, "r");
        var wfd = fs.openSync(f2, "r+");
        globalThis.__c1fd.rfd = rfd;
        globalThis.__c1fd.wfd = wfd;
        globalThis.__c1fd.f1 = f1;
        globalThis.__c1fd.f2 = f2;
        fs.fstat(rfd, function(e, st) {{ got("fstat", e, st ? "size=" + st.size : "∅"); }});
        fs.read(rfd, Buffer.alloc(4), 0, 4, 0, function(e, n, buf) {{ got("read", e, n === 4 ? "n4" : "n" + n); }});
        fs.write(wfd, Buffer.from("AB"), 0, 2, 0, function(e, n) {{ got("write", e, "n" + n); }});
        fs.ftruncate(wfd, 8, function(e) {{ got("ftruncate", e); }});
        fs.fsync(rfd, function(e) {{ got("fsync", e); }});
        fs.fdatasync(rfd, function(e) {{ got("fdatasync", e); }});
        fs.fchmod(wfd, 0o600, function(e) {{ got("fchmod", e); }});
        fs.futimes(rfd, 1000, 2000, function(e) {{ got("futimes", e); }});
        'scheduled'
    "#,
        f1 = js_escape(&f_read),
        f2 = js_escape(&f_write),
    );
    let sched_out = eval_string_full(&mut ctx, &sched);
    assert_eq!(sched_out, "scheduled", "scheduling eval: {}", sched_out);
    assert!(
        pump_until(
            &mut ctx,
            "globalThis.__c1fd.n >= globalThis.__c1fd.want",
            20_000
        ),
        "fd-flow round A incomplete (n={} log={})",
        eval_string(&mut ctx, "String(globalThis.__c1fd.n)"),
        eval_string(&mut ctx, "Object.keys(globalThis.__c1fd.log).join(',')")
    );

    let body = r#"
        var fs = require('fs');
        var R = [];
        function ck(n, f) {
            try { var ok = f(); R.push(n + (ok ? " PASS" : " FAIL")); }
            catch (e) { R.push(n + " ERR:" + String((e && e.message) || e).substring(0, 80)); }
        }
        var L = globalThis.__c1fd.log;
        ck("all_ok", function() {
            for (var k in L) if (L[k].indexOf("ok|") !== 0) return false;
            return true;
        });
        ck("fstat_size", function() { return L.fstat.indexOf("size=10") !== -1; });
        ck("read_n4", function() { return L.read.indexOf("n4") !== -1; });
        ck("write_n2", function() { return L.write.indexOf("n2") !== -1; });
        ck("fd_log_complete", function() {
            var keys = ["fstat","read","write","ftruncate","fsync","fdatasync","fchmod","futimes"];
            for (var i = 0; i < keys.length; i++) if (!L[keys[i]]) return false;
            return true;
        });
        ck("fchmod_side_effect", function() { return (fs.statSync(globalThis.__c1fd.f2).mode & 0o777) === 0o600; });
        ck("futimes_side_effect", function() { return fs.statSync(globalThis.__c1fd.f1).mtimeMs === 2000000; });
        R.join("|")
    "#;
    let results = run_checks(&mut ctx, body);
    assert_eq!(results.len(), 7);

    // Round B: async close on both fds.
    let sched2 = r#"
        var fs = require('fs');
        globalThis.__c1fd2 = { n: 0, want: 2, log: {} };
        fs.close(globalThis.__c1fd.rfd, function(e) {
            globalThis.__c1fd2.log.close_r = e ? String(e.code) : "ok";
            globalThis.__c1fd2.n++;
        });
        fs.close(globalThis.__c1fd.wfd, function(e) {
            globalThis.__c1fd2.log.close_w = e ? String(e.code) : "ok";
            globalThis.__c1fd2.n++;
        });
        'scheduled'
    "#;
    let sched2_out = eval_string_full(&mut ctx, sched2);
    assert_eq!(sched2_out, "scheduled", "scheduling eval: {}", sched2_out);
    assert!(
        pump_until(
            &mut ctx,
            "globalThis.__c1fd2.n >= globalThis.__c1fd2.want",
            10_000
        ),
        "close callbacks incomplete"
    );
    let closes = eval_string(
        &mut ctx,
        "globalThis.__c1fd2.log.close_r + ',' + globalThis.__c1fd2.log.close_w",
    );
    assert_eq!(closes, "ok,ok", "async close faces: {}", closes);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// 17. Async callback error batch — ENOENT code delivery through the carrier.
// ---------------------------------------------------------------------------

#[test]
fn c1_async_cb_error_batch() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("cberr");
    let plain = dir.join("plain.txt");
    std::fs::write(&plain, "x").unwrap();

    let sched = format!(
        r#"
        var fs = require('fs');
        globalThis.__c1e = {{ n: 0, want: 8, log: {{}} }};
        function got(k, err) {{
            globalThis.__c1e.log[k] = err ? String(err.code || err.message || err) : "NULL";
            globalThis.__c1e.n++;
        }}
        var miss = "{miss}";
        fs.readFile(miss, function(e) {{ got("readFile", e); }});
        fs.stat(miss, function(e) {{ got("stat", e); }});
        fs.access(miss, function(e) {{ got("access", e); }});
        fs.open(miss, "r", 0o644, function(e) {{ got("open", e); }}); // cb extraction starts at idx 3
        fs.readdir(miss, function(e) {{ got("readdir", e); }});
        fs.copyFile(miss, "{plain}.c", function(e) {{ got("copyFile", e); }});
        fs.unlink(miss, function(e) {{ got("unlink", e); }});
        fs.readlink(miss, function(e) {{ got("readlink", e); }});
        'scheduled'
    "#,
        miss = js_escape(&dir.join("absent_c1.txt")),
        plain = js_escape(&plain),
    );
    let sched_out = eval_string_full(&mut ctx, &sched);
    assert_eq!(sched_out, "scheduled", "scheduling eval: {}", sched_out);
    assert!(
        pump_until(
            &mut ctx,
            "globalThis.__c1e.n >= globalThis.__c1e.want",
            20_000
        ),
        "error callbacks incomplete (n={})",
        eval_string(&mut ctx, "String(globalThis.__c1e.n)")
    );
    let out = eval_string(
        &mut ctx,
        r#"(function(){ var L = globalThis.__c1e.log, bad = [];
             for (var k in L) if (L[k] !== "ENOENT") bad.push(k + "=" + L[k]);
             return bad.length ? bad.join("|") : "all_enoent"; })()"#,
    );
    assert_eq!(out, "all_enoent", "async error batch codes: {}", out);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// 18. watch / watchFile / unwatchFile faces. Live delivery runs on the
//     proven fs_watch_events pump shape: bounded post_eval_hook pumping
//     inside ctx.eval + JS-side setTimeout writes (direct Rust-side
//     drain_and_check against the fsw worker hard-crashes the dispatch).
// ---------------------------------------------------------------------------

use common::HOOK_BUDGET;
use common::setup_ctx_bounded;

/// Bounded eval-hook wait: each eval pumps through the post_eval_hook.
fn wait_until_bounded(
    ctx: &mut JsContext,
    js_condition: &str,
    per_eval_budget: usize,
    deadline_ms: u64,
) -> bool {
    let deadline = Instant::now() + Duration::from_millis(deadline_ms);
    while Instant::now() < deadline {
        HOOK_BUDGET.with(|b| b.set(per_eval_budget));
        if eval_string(ctx, js_condition) == "y" {
            return true;
        }
        std::thread::sleep(Duration::from_millis(4));
    }
    false
}

#[test]
fn c1_watch_faces() {
    // Static faces: registration/option/error arms (no live pump needed).
    let mut ctx = setup_ctx();
    let dir = tmp_dir("watch");
    let f = dir.join("w.txt");
    std::fs::write(&f, "v1").unwrap();

    let static_body = format!(
        r#"
        {prelude}
        var dir = "{dir}";
        ck("watch_missing_enoent", function() {{
            return codeof(function() {{ fs.watch("{miss}", function() {{}}); }}) === "ENOENT";
        }});
        {recursive_check}
        ck("watch_non_persistent_returns_watcher", function() {{
            var w = fs.watch(dir, {{ persistent: false }}, function() {{}});
            var ok = w && typeof w.on === "function" && typeof w.close === "function";
            w.close();
            return ok;
        }});
        ck("watchFile_returns_and_unwatch_clean", function() {{
            var w = fs.watchFile("{f}", {{ interval: 50 }}, function() {{}});
            var isObj = !!w && typeof w.on === "function";
            fs.unwatchFile("{f}");
            return isObj;
        }});
        ck("unwatchFile_unwatched_path_noop", function() {{
            fs.unwatchFile("{never}");
            return true;
        }});
        R.join("|")
    "#,
        prelude = CHECK_PRELUDE,
        dir = js_escape(&dir),
        miss = js_escape(&dir.join("nope")),
        f = js_escape(&f),
        never = js_escape(&dir.join("never-watched")),
        recursive_check = if cfg!(unix) {
            r#"ck("watch_recursive_unix_throws", function() {
                return msgof(function() { fs.watch(dir, { recursive: true }, function() {}); }).indexOf("recursive") !== -1;
            });"#
        } else {
            ""
        },
    );
    let results = run_checks(&mut ctx, &static_body);
    let want = if cfg!(unix) { 5 } else { 4 };
    assert_eq!(results.len(), want);
    drop(ctx);

    // Live faces: one bounded context, one file watch + one watchFile, JS-side
    // writes through setTimeout (the fs_watch_events pump contract).
    let mut ctx = setup_ctx_bounded();
    let dir2 = tmp_dir("watch_live");
    let f2 = dir2.join("live.txt");
    std::fs::write(&f2, "v1").unwrap(); // 2 bytes

    let sched = format!(
        r#"
var fs = require('fs');
globalThis.__ev = {{ events: 0, deltas: [] }};
globalThis.__w = fs.watch("{f}", {{ persistent: false }}, function (etype, fname) {{
    globalThis.__ev.events++;
}});
fs.watchFile("{f}", {{ interval: 60 }}, function (curr, prev) {{
    globalThis.__ev.deltas.push(prev.size + '->' + curr.size);
}});
setTimeout(function () {{ fs.writeFileSync("{f}", "v2-changed"); }}, 120);
setTimeout(function () {{ fs.writeFileSync("{f}", "v3-much-longer-content"); }}, 320);
globalThis.__done = function () {{
    return globalThis.__ev.events >= 1 && globalThis.__ev.deltas.join(',').indexOf('->22') !== -1;
}};
'ok'
"#,
        f = js_escape(&f2),
    );
    let sched_out = eval_string(&mut ctx, &sched);
    assert_eq!(sched_out, "ok", "live watch scheduling: {}", sched_out);

    let got = wait_until_bounded(
        &mut ctx,
        "(globalThis.__done && globalThis.__done()) ? 'y' : 'n'",
        80,
        20_000,
    );
    let deltas = eval_string(&mut ctx, "JSON.stringify(globalThis.__ev.deltas)");
    let events = eval_string(&mut ctx, "String(globalThis.__ev.events)");
    let _ = eval_string(&mut ctx, "globalThis.__w.close(); 'closed'");
    let _ = eval_string(
        &mut ctx,
        &format!("fs.unwatchFile('{}'); 'ok'", js_escape(&f2)),
    );
    assert!(
        got && events != "0" && deltas.contains("->22"),
        "live watch faces incomplete: events={} deltas={}",
        events,
        deltas
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&dir2);
    bun_runtime::shutdown_thread_sm();
}

// ---------------------------------------------------------------------------
// 19. cpSync / cp faces — file copy, recursive dir copy, missing src,
//     errorOnExist, callback form (sync impl, no cb delivery).
// ---------------------------------------------------------------------------

#[test]
fn c1_cp_faces() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("cp");
    let src_file = dir.join("s.txt");
    std::fs::write(&src_file, "cp-me").unwrap();
    let src_dir = dir.join("tree");
    std::fs::create_dir_all(src_dir.join("nested")).unwrap();
    std::fs::write(src_dir.join("top.txt"), "top").unwrap();
    std::fs::write(src_dir.join("nested").join("deep.txt"), "deep").unwrap();

    let body = format!(
        r#"
        {prelude}
        var d = "{d}";
        ck("cpSync_file", function() {{
            fs.cpSync(d + "/s.txt", d + "/out.txt");
            return fs.readFileSync(d + "/out.txt", "utf8") === "cp-me";
        }});
        ck("cpSync_dir_recursive", function() {{
            fs.cpSync(d + "/tree", d + "/tree2");
            return fs.readFileSync(d + "/tree2/top.txt", "utf8") === "top"
                && fs.readFileSync(d + "/tree2/nested/deep.txt", "utf8") === "deep";
        }});
        ck("cpSync_missing_src_throws", function() {{
            return msgof(function() {{ fs.cpSync(d + "/absent", d + "/x"); }}).indexOf("cpSync") !== -1;
        }});
        ck("cpSync_errorOnExist_file_dst", function() {{
            return msgof(function() {{
                fs.cpSync(d + "/tree", d + "/out.txt", {{ errorOnExist: true }});
            }}).indexOf("already exists") !== -1;
        }});
        ck("cp_form_no_callback", function() {{
            fs.cp(d + "/s.txt", d + "/out2.txt");
            return fs.readFileSync(d + "/out2.txt", "utf8") === "cp-me";
        }});
        R.join("|")
    "#,
        prelude = CHECK_PRELUDE,
        d = js_escape(&dir),
    );
    let results = run_checks(&mut ctx, &body);
    assert_eq!(results.len(), 5);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// 20. rm/rmdir faces — force option, recursive dir removal, ENOTEMPTY,
//     async rmdir callback form.
// ---------------------------------------------------------------------------

#[test]
fn c1_rm_rmdir_faces() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("rmrm");
    let plain = dir.join("p.txt");
    std::fs::write(&plain, "x").unwrap();
    let nonempty = dir.join("ne");
    std::fs::create_dir_all(&nonempty).unwrap();
    std::fs::write(nonempty.join("kid.txt"), "k").unwrap();
    let rmdir_target = dir.join("rt");
    std::fs::create_dir_all(&rmdir_target).unwrap();

    let sched = format!(
        r#"
        var fs = require('fs');
        globalThis.__c1rd = {{ n: 0, want: 1, err: "PENDING" }};
        fs.rmdir("{rt}", function(e) {{
            globalThis.__c1rd.err = e ? String(e.code || e.message) : null;
            globalThis.__c1rd.n++;
        }});
        'scheduled'
    "#,
        rt = js_escape(&rmdir_target),
    );
    let sched_out = eval_string_full(&mut ctx, &sched);
    assert_eq!(sched_out, "scheduled", "scheduling eval: {}", sched_out);
    assert!(
        pump_until(&mut ctx, "globalThis.__c1rd.n >= 1", 10_000),
        "rmdir cb never fired"
    );

    let body = format!(
        r#"
        {prelude}
        var d = "{d}";
        ck("rmSync_file", function() {{
            fs.rmSync(d + "/p.txt");
            return !fs.existsSync(d + "/p.txt");
        }});
        ck("rmSync_force_silences_enoent", function() {{
            // node v24.19.0 oracle: {{force:true}} swallows ENOENT for a
            // missing path (force:false / absent flag still throws).
            return codeof(function() {{ fs.rmSync(d + "/absent.txt", {{ force: true }}); }}) === "NOTHROW";
        }});
        ck("rmSync_missing_no_force_enoent", function() {{
            return codeof(function() {{ fs.rmSync(d + "/absent.txt"); }}) === "ENOENT";
        }});
        ck("rmSync_recursive_dir", function() {{
            fs.rmSync(d + "/ne", {{ recursive: true }});
            return !fs.existsSync(d + "/ne");
        }});
        ck("rmdirSync_nonempty_enotempty", function() {{
            var t = d + "/ne2";
            fs.mkdirSync(t);
            fs.writeFileSync(t + "/kid.txt", "k");
            return codeof(function() {{ fs.rmdirSync(t); }}) === "ENOTEMPTY";
        }});
        ck("rmdirSync_missing_enoent", function() {{
            return codeof(function() {{ fs.rmdirSync(d + "/no-dir"); }}) === "ENOENT";
        }});
        R.join("|")
    "#,
        prelude = CHECK_PRELUDE,
        d = js_escape(&dir),
    );
    let results = run_checks(&mut ctx, &body);
    assert_eq!(results.len(), 6);
    assert!(
        eval_string(&mut ctx, "globalThis.__c1rd.err === null ? 'yes' : 'no'") == "yes",
        "async rmdir cb err: {}",
        eval_string(&mut ctx, "String(globalThis.__c1rd.err)")
    );
    assert!(!rmdir_target.exists(), "async rmdir did not remove the dir");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// 21. Stream polyfill faces — createReadStream events, createWriteStream
//     write/end (string + Buffer chunks), pipe, error. Pumped through the
//     bounded post_eval_hook (same contract as the watch faces).
// ---------------------------------------------------------------------------

#[test]
fn c1_stream_faces() {
    let mut ctx = setup_ctx_bounded();
    let dir = tmp_dir("stream");
    let src = dir.join("in.txt");
    std::fs::write(&src, "stream body 你").unwrap();

    let sched = format!(
        r#"
var fs = require('fs');
globalThis.__s = {{ n: 0, log: {{}} }};
function mark(k, v) {{ globalThis.__s.log[k] = v; globalThis.__s.n++; }}
var src = "{src}";
var out = "{out}";

var rs = fs.createReadStream(src, {{ encoding: "utf8" }});
var chunks = [];
rs.on("data", function(c) {{ chunks.push(String(c)); }});
rs.on("end", function() {{ mark("read_end", chunks.join("")); }});
rs.on("close", function() {{ mark("read_close", "yes"); }});

var rsBad = fs.createReadStream(src + ".absent");
rsBad.on("error", function(e) {{ mark("read_error", String((e && e.code) || e)); }});

var ws = fs.createWriteStream(out);
ws.on("finish", function() {{ mark("write_finish", require("fs").existsSync(out) ? "exists" : "missing"); }});
ws.write("hello ");
ws.write(Buffer.from([0xE4, 0xBD, 0xA0]));
ws.end("+tail");

var psrc = fs.createReadStream(src, {{ encoding: "utf8" }});
var pdst = fs.createWriteStream(out + ".pipe");
pdst.on("finish", function() {{ mark("pipe_finish", "yes"); }});
psrc.pipe(pdst);
'scheduled'
"#,
        src = js_escape(&src),
        out = js_escape(&dir.join("out.txt")),
    );
    let sched_out = eval_string(&mut ctx, &sched);
    assert_eq!(sched_out, "scheduled", "stream scheduling: {}", sched_out);

    let got = wait_until_bounded(&mut ctx, "(globalThis.__s.n >= 5) ? 'y' : 'n'", 80, 20_000);
    let n = eval_string(&mut ctx, "String(globalThis.__s.n)");

    let out_path = dir.join("out.txt");
    let pipe_path = dir.join("out.txt.pipe");
    let out_content =
        std::fs::read_to_string(&out_path).unwrap_or_else(|_| "<unreadable>".to_string());
    let pipe_content =
        std::fs::read_to_string(&pipe_path).unwrap_or_else(|_| "<unreadable>".to_string());
    assert_eq!(
        out_content, "hello 你+tail",
        "createWriteStream content mismatch"
    );
    assert_eq!(pipe_content, "stream body 你", "pipe content mismatch");

    let body = r#"
        var R = [];
        function ck(n, f) {
            try { var ok = f(); R.push(n + (ok ? " PASS" : " FAIL")); }
            catch (e) { R.push(n + " ERR:" + String((e && e.message) || e).substring(0, 80)); }
        }
        var L = globalThis.__s.log;
        ck("read_end_content", function() { return L.read_end === "stream body 你"; });
        ck("read_close", function() { return L.read_close === "yes"; });
        ck("read_error_enoent", function() { return L.read_error === "ENOENT"; });
        ck("write_finish", function() { return L.write_finish === "exists"; });
        ck("pipe_finish", function() { return L.pipe_finish === "yes"; });
        R.join("|")
    "#;
    let results = run_checks(&mut ctx, body);
    assert_eq!(results.len(), 5);
    assert!(got, "stream events incomplete (n={})", n);
    let _ = std::fs::remove_dir_all(&dir);
    bun_runtime::shutdown_thread_sm();
}

// ---------------------------------------------------------------------------
// 22. truncate/utimes time-value faces (number + double + non-number → 0).
// ---------------------------------------------------------------------------

#[test]
fn c1_truncate_utimes_time_faces() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("times");
    let f = dir.join("t.txt");
    std::fs::write(&f, "0123456789").unwrap();

    let body = format!(
        r#"
        {prelude}
        var p = "{p}";
        ck("truncateSync_len4", function() {{
            fs.truncateSync(p, 4);
            return fs.statSync(p).size === 4;
        }});
        ck("truncateSync_double_len", function() {{
            fs.truncateSync(p, 2.9);
            return fs.statSync(p).size === 2;
        }});
        ck("truncateSync_undefined_zero", function() {{
            fs.truncateSync(p, undefined);
            return fs.statSync(p).size === 0;
        }});
        R.join("|")
    "#,
        prelude = CHECK_PRELUDE,
        p = js_escape(&f),
    );
    let results = run_checks(&mut ctx, &body);
    assert_eq!(results.len(), 3);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// 23. K wave — node:fs second-wave residuals (node v24.19.0 oracle probes):
//     fs.readv async backfills the caller iovecs; fs.read async hands the
//     CALLER's buffer to the callback; fh.readFile reads from the cursor.
// ---------------------------------------------------------------------------

#[test]
fn c1_readv_async_backfills_caller_buffers() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("kwreadv");
    let f = dir.join("rv.txt");
    std::fs::write(&f, "0123456789").unwrap();

    let sched = format!(
        r#"
        var fs = require('fs');
        globalThis.__krv = {{ n: 0, want: 3, log: {{}} }};
        var f = "{f}";
        (function() {{
            var fd = fs.openSync(f, "r");
            globalThis.__krv.fd = fd;
            // positional form: pread at 4, cursor untouched
            var b1 = Buffer.alloc(2), b2 = Buffer.alloc(3);
            fs.readv(fd, [b1, b2], 4, function(e, n) {{
                globalThis.__krv.log.pos = {{
                    err: e ? String(e.code || e.message) : null,
                    n: n,
                    b1: b1.toString("utf8"),
                    b2: b2.toString("utf8"),
                }};
                globalThis.__krv.n++;
            }});
            // cursor form: no position → reads at the current cursor (0)
            var c1 = Buffer.alloc(2), c2 = Buffer.alloc(3);
            fs.readv(fd, [c1, c2], function(e, n) {{
                globalThis.__krv.log.cur = {{
                    err: e ? String(e.code || e.message) : null,
                    n: n,
                    b1: c1.toString("utf8"),
                    b2: c2.toString("utf8"),
                }};
                globalThis.__krv.n++;
            }});
            // fs.read async: callback receives the SAME caller buffer, filled
            var rb = Buffer.alloc(4);
            fs.read(fd, rb, 1, 3, 0, function(e, n, buf) {{
                globalThis.__krv.log.read = {{
                    err: e ? String(e.code || e.message) : null,
                    n: n,
                    ident: buf === rb,
                    hex: rb.toString("hex"),
                }};
                globalThis.__krv.n++;
            }});
        }})();
        'scheduled'
    "#,
        f = js_escape(&f),
    );
    assert_eq!(eval_string_full(&mut ctx, &sched), "scheduled");
    assert!(
        pump_until(
            &mut ctx,
            "globalThis.__krv.n >= globalThis.__krv.want",
            20_000
        ),
        "readv/read async callbacks incomplete (n={} log={})",
        eval_string(&mut ctx, "String(globalThis.__krv.n)"),
        eval_string(&mut ctx, "JSON.stringify(globalThis.__krv.log)")
    );

    assert_eq!(
        eval_string(&mut ctx, "JSON.stringify(globalThis.__krv.log.pos)"),
        r#"{"err":null,"n":5,"b1":"45","b2":"678"}"#,
        "readv(fd,[b1,b2],4,cb) must fill the caller buffers at position 4"
    );
    assert_eq!(
        eval_string(&mut ctx, "JSON.stringify(globalThis.__krv.log.cur)"),
        r#"{"err":null,"n":5,"b1":"01","b2":"234"}"#,
        "readv(fd,[b1,b2],cb) must read from the current cursor"
    );
    assert_eq!(
        eval_string(&mut ctx, "JSON.stringify(globalThis.__krv.log.read)"),
        r#"{"err":null,"n":3,"ident":true,"hex":"00303132"}"#,
        "read(fd,rb,1,3,0,cb) must fill the caller buffer at offset 1 and hand it back"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn c1_fh_readfile_reads_from_cursor() {
    let mut ctx = setup_ctx();
    let dir = tmp_dir("kwfhread");
    let f = dir.join("cur.txt");
    std::fs::write(&f, "0123456789").unwrap();

    let sched = format!(
        r#"
        globalThis.__kfh = {{ done: false, posRead: null, whole: null, cursorRead: null, rest: null, rest2: null }};
        (async function() {{
            var fsp = require('fs').promises;
            var fh = await fsp.open("{f}", "r");
            // explicit position: pread semantics — does NOT advance the cursor
            var rb = Buffer.alloc(2);
            var r1 = await fh.read(rb, 0, 2, 0);
            globalThis.__kfh.posRead = r1.bytesRead;
            // readFile: from the CURRENT cursor (still 0) to EOF; the read
            // itself advances the cursor to EOF (node oracle sequence).
            globalThis.__kfh.whole = String(await fh.readFile());
            // cursor is now at EOF → a cursor read and a readFile both yield 0
            var rb2 = Buffer.alloc(2);
            var r2 = await fh.read(rb2, 0, 2, null);
            globalThis.__kfh.cursorRead = rb2.toString("utf8") + ":" + r2.bytesRead;
            globalThis.__kfh.rest = String(await fh.readFile());
            await fh.close();
            // continuation from a MID cursor: a cursor-form write advances the
            // cursor to 2, then readFile continues from there (node oracle:
            // after fh.write("XX") → readFile === "23456789").
            var fh2 = await fsp.open("{f}", "r+");
            await fh2.write("XY");
            globalThis.__kfh.rest2 = String(await fh2.readFile());
            await fh2.close();
            globalThis.__kfh.done = true;
        }})();
        'scheduled'
    "#,
        f = js_escape(&f),
    );
    assert_eq!(eval_string_full(&mut ctx, &sched), "scheduled");
    assert!(
        pump_until(&mut ctx, "globalThis.__kfh.done === true", 20_000),
        "fh.readFile cursor flow did not settle"
    );

    assert_eq!(
        eval_string(&mut ctx, "String(globalThis.__kfh.posRead)"),
        "2",
        "fh.read(rb,0,2,0) must read 2 bytes"
    );
    assert_eq!(
        eval_string(&mut ctx, "String(globalThis.__kfh.whole)"),
        "0123456789",
        "fh.readFile after an explicit-position read must read the whole file (cursor still 0)"
    );
    assert_eq!(
        eval_string(&mut ctx, "String(globalThis.__kfh.cursorRead)"),
        ":0",
        "fh.read(rb,0,2,null) at EOF must read 0 bytes"
    );
    assert_eq!(
        eval_string(&mut ctx, "String(globalThis.__kfh.rest)"),
        "",
        "fh.readFile at EOF must resolve empty (cursor semantics, not from-0)"
    );
    assert_eq!(
        eval_string(&mut ctx, "String(globalThis.__kfh.rest2)"),
        "23456789",
        "fh.readFile must continue from the mid cursor left by the write (node v24.19.0 oracle)"
    );
    let _ = std::fs::remove_dir_all(&dir);
    bun_runtime::shutdown_thread_sm();
}
