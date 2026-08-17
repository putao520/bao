// @trace REQ-ENG-007 [level:integration]
// Conformance tests for node:fs against Node.js / Bun reference behavior.
// Reference: ~/code/rust/bun/test/js/node/fs/fs.test.ts (MIT, Bun project)
//
// All checks live inside a single #[test] — SpiderMonkey is single-init.

#[path = "../conformance_common.rs"]
mod common;

use ::std::path::PathBuf;
use common::{CHECK_SCAFFOLD, js_path, make_ctx, run_checks};

fn tmp_dir(label: &str) -> PathBuf {
    let d = ::std::env::temp_dir().join(format!("bao_fs_conf_{}", label));
    let _ = ::std::fs::remove_dir_all(&d);
    ::std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn test_fs_conformance_suite() {
    let mut ctx = make_ctx();

    // ===== writeFile / readFile sync =====
    // NOTE: bao_runtime's readFileSync without an encoding returns a String,
    // not a Buffer (Node.js returns Buffer). Documented in GAP_REPORT.
    {
        let dir = tmp_dir("writefile_readfile_sync");
        let f = dir.join("hello.txt");
        let p = js_path(&f);
        let src = format!(
            r##"
            {scaffold}
            var fs = require('fs');
            var p = "{p}";
            fs.writeFileSync(p, "hello world", "utf8");
            check("writeFile_then_readFile", function() {{
                return fs.readFileSync(p, "utf8") === "hello world";
            }});
            check("readFile_encoding_obj", function() {{
                return fs.readFileSync(p, {{encoding: "utf8"}}) === "hello world";
            }});
            check("writeFile_overwrite", function() {{
                fs.writeFileSync(p, "second content");
                return fs.readFileSync(p, "utf8") === "second content";
            }});
            results.join("|")
            "##,
            scaffold = CHECK_SCAFFOLD,
            p = p
        );
        run_checks(&mut ctx, &src);
        let _ = ::std::fs::remove_dir_all(&dir);
    }

    // ===== appendFileSync =====
    {
        let dir = tmp_dir("appendfile_sync");
        let f = dir.join("append.txt");
        let p = js_path(&f);
        let src = format!(
            r##"
            {scaffold}
            var fs = require('fs');
            var p = "{p}";
            fs.writeFileSync(p, "line1\n");
            fs.appendFileSync(p, "line2\n");
            fs.appendFileSync(p, "line3\n");
            check("appendFile_accumulates", function() {{
                return fs.readFileSync(p, "utf8") === "line1\nline2\nline3\n";
            }});
            check("appendFile_creates_if_missing", function() {{
                fs.appendFileSync(p + ".new", "fresh");
                return fs.readFileSync(p + ".new", "utf8") === "fresh";
            }});
            results.join("|")
            "##,
            scaffold = CHECK_SCAFFOLD,
            p = p
        );
        run_checks(&mut ctx, &src);
        let _ = ::std::fs::remove_dir_all(&dir);
    }

    // ===== multibyte utf8 / latin1 roundtrips (mojibake regression) =====
    // readFileSync utf8 previously went through JS_NewStringCopyZ, which
    // re-reads the multibyte UTF-8 bytes as Latin-1 (包子 → "å\x8c\x85").
    // The JSString must come from the UTF-8 decoder (JS_NewStringCopyUTF8N).
    {
        let dir = tmp_dir("multibyte_utf8");
        let f = dir.join("cjk.txt");
        let p = js_path(&f);
        let src = format!(
            r##"
            {scaffold}
            var fs = require('fs');
            var p = "{p}";
            // 2-byte (é) + 3-byte (包子) + 4-byte (🍞) UTF-8 sequences.
            var text = "café 包子🍞 你好";
            fs.writeFileSync(p, text, "utf8");
            check("readFileSync_utf8_multibyte_roundtrip", function() {{
                return fs.readFileSync(p, "utf8") === text;
            }});
            check("readFileSync_utf8_charCodes", function() {{
                var s = fs.readFileSync(p, "utf8");
                return s.charCodeAt(0) === 99 && s.indexOf("包子") === 5 && s.indexOf("🍞") === 7;
            }});
            check("readFileSync_encoding_obj_multibyte", function() {{
                return fs.readFileSync(p, {{encoding: "utf8"}}) === text;
            }});
            check("readFileSync_fallback_encoding_multibyte", function() {{
                return fs.readFileSync(p, "utf-8") === text;
            }});
            // latin1: each byte maps to U+0000..U+00FF (byte 0xE5 → 'å' 229).
            check("readFileSync_latin1_high_bytes", function() {{
                fs.writeFileSync(p, Buffer.from([0x41, 0xE5, 0x80, 0xFF]));
                var s = fs.readFileSync(p, "latin1");
                return s.length === 4 && s.charCodeAt(0) === 0x41 && s.charCodeAt(1) === 0xE5 &&
                       s.charCodeAt(2) === 0x80 && s.charCodeAt(3) === 0xFF;
            }});
            results.join("|")
            "##,
            scaffold = CHECK_SCAFFOLD,
            p = p
        );
        run_checks(&mut ctx, &src);
        let _ = ::std::fs::remove_dir_all(&dir);
    }

    // ===== createWriteStream binary-safe flush =====
    // Buffer chunks previously went through String(chunk) (utf8-lossy
    // re-encode) — invalid-UTF-8 bytes were corrupted (0xFF → U+FFFD).
    // Chunks must be flushed raw via the byte-view path.
    {
        let dir = tmp_dir("writestream_binary");
        let f = dir.join("bin.dat");
        let p = js_path(&f);
        let src = format!(
            r##"
            {scaffold}
            var fs = require('fs');
            var p = "{p}";
            // Invalid-as-UTF-8 bytes: any String() coercion mangles them.
            var payload = [0x00, 0x7F, 0x80, 0xC3, 0xFF, 0xFE, 0x01];
            var ws = fs.createWriteStream(p);
            ws.write(Buffer.from(payload.slice(0, 4)));
            ws.end(Buffer.from(payload.slice(4)));
            check("createWriteStream_binary_roundtrip", function() {{
                var back = fs.readFileSync(p);
                if (back.length !== payload.length) return false;
                for (var i = 0; i < payload.length; i++) {{
                    if (back[i] !== payload[i]) return false;
                }}
                return true;
            }});
            check("createWriteStream_bytesWritten", function() {{
                return ws.bytesWritten === payload.length;
            }});
            check("createWriteStream_finish_emitted", function() {{
                var finished = false;
                var ws2 = fs.createWriteStream(p + ".2");
                ws2.on('finish', function() {{ finished = true; }});
                ws2.end(Buffer.from([0x61]));
                return finished === true;
            }});
            // Mixed string + Buffer chunks: string contributes its UTF-8
            // bytes, Buffer its raw bytes.
            check("createWriteStream_mixed_chunks", function() {{
                var ws3 = fs.createWriteStream(p + ".3");
                ws3.write("AB");
                ws3.end(Buffer.from([0xC3, 0xA9, 0xFF]));
                var back = fs.readFileSync(p + ".3");
                return back.length === 5 && back[0] === 0x41 && back[1] === 0x42 &&
                       back[2] === 0xC3 && back[3] === 0xA9 && back[4] === 0xFF;
            }});
            // String-only chunks keep the legacy byte path (UTF-8 of join).
            check("createWriteStream_string_only", function() {{
                var ws4 = fs.createWriteStream(p + ".4");
                ws4.write("hello ");
                ws4.end("包子");
                return fs.readFileSync(p + ".4", "utf8") === "hello 包子";
            }});
            // Uint8Array (non-Buffer) chunks are byte views too.
            check("createWriteStream_uint8array_chunk", function() {{
                var ws5 = fs.createWriteStream(p + ".5");
                ws5.end(new Uint8Array([0x00, 0x99, 0xFE]));
                var back = fs.readFileSync(p + ".5");
                return back.length === 3 && back[1] === 0x99 && back[2] === 0xFE;
            }});
            results.join("|")
            "##,
            scaffold = CHECK_SCAFFOLD,
            p = p
        );
        run_checks(&mut ctx, &src);
        let _ = ::std::fs::remove_dir_all(&dir);
    }

    // ===== statSync =====
    {
        let dir = tmp_dir("stat_sync");
        let f = dir.join("file.txt");
        ::std::fs::write(&f, "stat me").unwrap();
        let subdir = dir.join("sub");
        ::std::fs::create_dir(&subdir).unwrap();
        let fp = js_path(&f);
        let sp = js_path(&subdir);
        let src = format!(
            r##"
            {scaffold}
            var fs = require('fs');
            check("stat_file_size", function() {{
                var s = fs.statSync("{fp}");
                return typeof s.size === "number" && s.size === 7;
            }});
            check("stat_isFile", function() {{
                var s = fs.statSync("{fp}");
                return s.isFile() === true && s.isDirectory() === false;
            }});
            check("stat_dir_isDirectory", function() {{
                var s = fs.statSync("{sp}");
                return s.isDirectory() === true && s.isFile() === false;
            }});
            results.join("|")
            "##,
            scaffold = CHECK_SCAFFOLD,
            fp = fp,
            sp = sp
        );
        run_checks(&mut ctx, &src);
        let _ = ::std::fs::remove_dir_all(&dir);
    }

    // ===== existsSync =====
    {
        let dir = tmp_dir("exists_sync");
        let f = dir.join("exists.txt");
        ::std::fs::write(&f, "yes").unwrap();
        let missing = dir.join("missing.txt");
        let fp = js_path(&f);
        let mp = js_path(&missing);
        let dp = js_path(&dir);
        let src = format!(
            r##"
            {scaffold}
            var fs = require('fs');
            check("existsSync_true", function() {{ return fs.existsSync("{fp}") === true; }});
            check("existsSync_false", function() {{ return fs.existsSync("{mp}") === false; }});
            check("existsSync_dir", function() {{ return fs.existsSync("{dp}") === true; }});
            results.join("|")
            "##,
            scaffold = CHECK_SCAFFOLD,
            fp = fp,
            mp = mp,
            dp = dp
        );
        run_checks(&mut ctx, &src);
        let _ = ::std::fs::remove_dir_all(&dir);
    }

    // ===== mkdir / readdir / rm sync =====
    {
        let dir = tmp_dir("mkdir_readdir_rm");
        let new_dir = dir.join("new_subdir");
        let dp = js_path(&new_dir);
        let src = format!(
            r##"
            {scaffold}
            var fs = require('fs');
            var d = "{dp}";
            check("mkdirSync_creates_dir", function() {{
                fs.mkdirSync(d);
                return fs.existsSync(d) === true;
            }});
            check("readdirSync_lists_files", function() {{
                fs.writeFileSync(d + "/a.txt", "a");
                fs.writeFileSync(d + "/b.txt", "b");
                var entries = fs.readdirSync(d);
                return Array.isArray(entries) && entries.length === 2;
            }});
            check("rmdirSync_removes_empty", function() {{
                fs.unlinkSync(d + "/a.txt");
                fs.unlinkSync(d + "/b.txt");
                fs.rmdirSync(d);
                return fs.existsSync(d) === false;
            }});
            check("rmSync_recursive", function() {{
                fs.mkdirSync(d);
                fs.writeFileSync(d + "/x.txt", "x");
                fs.rmSync(d, {{recursive: true}});
                return fs.existsSync(d) === false;
            }});
            results.join("|")
            "##,
            scaffold = CHECK_SCAFFOLD,
            dp = dp
        );
        run_checks(&mut ctx, &src);
        let _ = ::std::fs::remove_dir_all(&dir);
    }

    // ===== rename / copy / unlink sync =====
    {
        let dir = tmp_dir("rename_copy_unlink");
        let src_f = dir.join("src.txt");
        let dst_f = dir.join("dst.txt");
        let cp_src = dir.join("cp_src.txt");
        let cp_dst = dir.join("cp_dst.txt");
        ::std::fs::write(&src_f, "to rename").unwrap();
        ::std::fs::write(&cp_src, "to copy").unwrap();
        let sp = js_path(&src_f);
        let dp = js_path(&dst_f);
        let cps = js_path(&cp_src);
        let cpd = js_path(&cp_dst);
        let src = format!(
            r##"
            {scaffold}
            var fs = require('fs');
            check("renameSync_moves", function() {{
                fs.renameSync("{sp}", "{dp}");
                return fs.existsSync("{sp}") === false && fs.existsSync("{dp}") === true;
            }});
            check("copyFileSync_copies", function() {{
                fs.copyFileSync("{cps}", "{cpd}");
                return fs.existsSync("{cps}") === true && fs.existsSync("{cpd}") === true;
            }});
            check("unlinkSync_removes", function() {{
                fs.unlinkSync("{dp}");
                return fs.existsSync("{dp}") === false;
            }});
            results.join("|")
            "##,
            scaffold = CHECK_SCAFFOLD,
            sp = sp,
            dp = dp,
            cps = cps,
            cpd = cpd
        );
        run_checks(&mut ctx, &src);
        let _ = ::std::fs::remove_dir_all(&dir);
    }

    // ===== promises =====
    {
        let dir = tmp_dir("promises");
        let f = dir.join("p.txt");
        let fp = js_path(&f);
        let src = format!(
            r##"
            {scaffold}
            var fsp = require('fs').promises;
            var p = "{fp}";
            check("promises_exists", function() {{
                return typeof fsp === "object" && typeof fsp.readFile === "function";
            }});
            results.join("|")
            "##,
            scaffold = CHECK_SCAFFOLD,
            fp = fp
        );
        run_checks(&mut ctx, &src);
        // Fire the promise write and verify from Rust side
        let _ = ctx.eval(
            &format!(
                r#"require('fs').promises.writeFile("{fp}", "hello promise");"#,
                fp = fp
            ),
            "<conformance>",
        );
        let content = ::std::fs::read_to_string(&f).unwrap_or_default();
        assert_eq!(
            content, "hello promise",
            "promises.writeFile should persist"
        );
        let _ = ::std::fs::remove_dir_all(&dir);
    }

    // ===== realpathSync =====
    {
        let dir = tmp_dir("realpath_symlink");
        let target = dir.join("target.txt");
        ::std::fs::write(&target, "real").unwrap();
        let tp = js_path(&target);
        let src = format!(
            r##"
            {scaffold}
            var fs = require('fs');
            check("realpathSync_returns_string", function() {{
                var r = fs.realpathSync("{tp}");
                return typeof r === "string" && r.length > 0;
            }});
            results.join("|")
            "##,
            scaffold = CHECK_SCAFFOLD,
            tp = tp
        );
        run_checks(&mut ctx, &src);
        let _ = ::std::fs::remove_dir_all(&dir);
    }

    bun_runtime::shutdown_thread_sm();
}

#[test]
fn test_fs_conformance_create_read_stream() {
    let mut ctx = make_ctx();
    let src = format!(
        r##"
        {scaffold}
        var fs = require('fs');
        check("createReadStream_exists", function() {{
            return typeof fs.createReadStream === "function";
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);
    bun_runtime::shutdown_thread_sm();
}

#[test]
fn test_fs_conformance_watch() {
    let mut ctx = make_ctx();
    let src = format!(
        r##"
        {scaffold}
        var fs = require('fs');
        check("watch_is_function", function() {{
            return typeof fs.watch === "function";
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);
    bun_runtime::shutdown_thread_sm();
}

#[test]
fn test_fs_conformance_cp_recursive() {
    let mut ctx = make_ctx();
    let src = format!(
        r##"
        {scaffold}
        var fs = require('fs');
        check("cpSync_is_function", function() {{
            return typeof fs.cpSync === "function";
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);
    bun_runtime::shutdown_thread_sm();
}

#[test]
fn test_fs_conformance_readfile_returns_buffer() {
    // Node.js: fs.readFileSync(path) → Buffer
    // bao_runtime: returns String (utf8-decoded)
    let mut ctx = make_ctx();
    use common::eval_string;
    let dir = tmp_dir("readfile_dev");
    let f = dir.join("x.txt");
    ::std::fs::write(&f, "data").unwrap();
    let p = js_path(&f);
    let r = eval_string(
        &mut ctx,
        &format!(
            r#"Buffer.isBuffer(require('fs').readFileSync("{p}")) ? "PASS" : "FAIL""#,
            p = p
        ),
    );
    assert_eq!(r, "PASS");
    let _ = ::std::fs::remove_dir_all(&dir);
    bun_runtime::shutdown_thread_sm();
}

#[test]
fn test_fs_conformance_readfile_async_multibyte() {
    // fs.promises.readFile twin of the utf8 mojibake fix: string_or_buffer
    // previously built the resolved string via JS_NewStringCopyZ (Latin-1
    // re-read of multibyte UTF-8). The promise must resolve with the decoded
    // text; drive the SM job queue so the .then observation lands.
    use ::std::time::Duration;
    let mut ctx = make_ctx();
    use common::eval_string;
    let dir = tmp_dir("readfile_async_mb");
    let f = dir.join("cjk.txt");
    ::std::fs::write(&f, "包子🍞 café".as_bytes()).unwrap();
    let p = js_path(&f);
    eval_string(
        &mut ctx,
        &format!(
            r#"
            globalThis.__r = {{}};
            var fs = require('fs');
            var p = fs.promises.readFile("{p}", "utf8");
            p.then(
              function(data) {{
                globalThis.__r.ok = (typeof data === "string") + ":" + (data === "包子🍞 café");
              }},
              function(e) {{ globalThis.__r.ok = "REJ:" + (e && e.message); }}
            );
            "#
        ),
    );
    // RunJobs flushes the already-settled promise's .then (same discipline
    // as bun_sqlite_backup_tests' drive_event_loop).
    let mut got = String::new();
    for _ in 0..20 {
        let mut cxm = ctx.cx();
        bun_runtime::timers::drain_and_check(&mut cxm);
        let v = eval_string(&mut ctx, r#"globalThis.__r.ok || """#);
        if !v.is_empty() {
            got = v;
            break;
        }
        ::std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        got, "true:true",
        "fs.promises.readFile utf8 must resolve multibyte text un-mangled"
    );
    let _ = ::std::fs::remove_dir_all(&dir);
    bun_runtime::shutdown_thread_sm();
}
