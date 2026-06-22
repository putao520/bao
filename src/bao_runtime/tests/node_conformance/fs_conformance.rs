// @trace REQ-ENG-007 [level:integration]
// Conformance tests for node:fs against Node.js / Bun reference behavior.
// Reference: ~/code/rust/bun/test/js/node/fs/fs.test.ts (MIT, Bun project)
//
// All checks live inside a single #[test] — SpiderMonkey is single-init.

#[path = "../conformance_common.rs"]
mod common;

use common::{js_path, make_ctx, run_checks, CHECK_SCAFFOLD};
use ::std::path::PathBuf;

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
            scaffold = CHECK_SCAFFOLD, p = p
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
            scaffold = CHECK_SCAFFOLD, p = p
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
            scaffold = CHECK_SCAFFOLD, fp = fp, sp = sp
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
            scaffold = CHECK_SCAFFOLD, fp = fp, mp = mp, dp = dp
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
            scaffold = CHECK_SCAFFOLD, dp = dp
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
            scaffold = CHECK_SCAFFOLD, sp = sp, dp = dp, cps = cps, cpd = cpd
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
            scaffold = CHECK_SCAFFOLD, fp = fp
        );
        run_checks(&mut ctx, &src);
        // Fire the promise write and verify from Rust side
        let _ = ctx.eval(
            &format!(r#"require('fs').promises.writeFile("{fp}", "hello promise");"#, fp = fp),
            "<conformance>",
        );
        let content = ::std::fs::read_to_string(&f).unwrap_or_default();
        assert_eq!(content, "hello promise", "promises.writeFile should persist");
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
            scaffold = CHECK_SCAFFOLD, tp = tp
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
        &format!(r#"Buffer.isBuffer(require('fs').readFileSync("{p}")) ? "PASS" : "FAIL""#, p = p),
    );
    assert_eq!(r, "PASS");
    let _ = ::std::fs::remove_dir_all(&dir);
    bun_runtime::shutdown_thread_sm();
}
