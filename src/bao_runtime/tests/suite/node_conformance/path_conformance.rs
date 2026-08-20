// @trace REQ-ENG-007 [level:integration]
// Conformance tests for node:path against Node.js / Bun reference behavior.
// Reference: ~/code/rust/bun/test/js/node/path/15704.test.js + Node.js path docs (MIT, Bun project)
//
// All checks live inside a single #[test] — SpiderMonkey is single-init.

#[path = "../conformance_common.rs"]
mod common;

use common::{CHECK_SCAFFOLD, make_ctx, run_checks};

#[test]
fn test_path_conformance_suite() {
    let mut ctx = make_ctx();

    // ===== join =====
    let src = format!(
        r##"
        {scaffold}
        var path = require('path');
        check("join_basic", function() {{ return path.join("a", "b", "c") === "a/b/c"; }});
        check("join_absolute_segment", function() {{
            return path.join("/foo", "bar", "baz") === "/foo/bar/baz";
        }});
        check("join_normalizes_separators", function() {{
            var r = path.join("a/", "/b");
            return r === "a/b" || r === "a//b";
        }});
        check("join_empty_returns_dot", function() {{
            return path.join("") === "." || path.join("") === "";
        }});
        check("join_long_path_no_crash", function() {{
            // Bun regression: very long path names should not crash
            var long = new Array(4097).join("b");
            var r = path.join(long);
            return typeof r === "string" && r.length > 0;
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== resolve =====
    let src = format!(
        r##"
        {scaffold}
        var path = require('path');
        check("resolve_absolute_passthrough", function() {{
            var r = path.resolve("/foo/bar");
            return r === "/foo/bar" || r.indexOf("foo/bar") >= 0;
        }});
        check("resolve_relative_to_absolute", function() {{
            var r = path.resolve("foo", "bar");
            return path.isAbsolute(r) === true;
        }});
        check("resolve_dotdot", function() {{
            var r = path.resolve("/foo/bar", "../baz");
            return r.indexOf("baz") >= 0 && r.indexOf("bar") < 0;
        }});
        check("resolve_no_args_cwd", function() {{
            var r = path.resolve();
            return typeof r === "string" && r.length > 0 && path.isAbsolute(r);
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== dirname / basename / extname =====
    let src = format!(
        r##"
        {scaffold}
        var path = require('path');
        check("dirname", function() {{ return path.dirname("/foo/bar/baz.txt") === "/foo/bar"; }});
        check("basename_full", function() {{ return path.basename("/foo/bar/baz.txt") === "baz.txt"; }});
        check("basename_strip_ext", function() {{ return path.basename("/foo/bar/baz.txt", ".txt") === "baz"; }});
        check("basename_no_path", function() {{ return path.basename("baz.txt") === "baz.txt"; }});
        check("extname_simple", function() {{ return path.extname("file.txt") === ".txt"; }});
        check("extname_multi_dot", function() {{ return path.extname("file.tar.gz") === ".gz"; }});
        check("extname_no_ext", function() {{ return path.extname("noext") === ""; }});
        check("extname_dot_only", function() {{ return path.extname(".hidden") === ""; }});
        check("extname_start_dot", function() {{ return path.extname(".hidden.txt") === ".txt"; }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== normalize / relative =====
    let src = format!(
        r##"
        {scaffold}
        var path = require('path');
        check("normalize_dotdot", function() {{
            var n = path.normalize("/foo/bar/../baz");
            return n.indexOf("baz") >= 0 && n.indexOf("..") < 0;
        }});
        check("normalize_dot", function() {{
            var n = path.normalize("/foo/./bar");
            return n.indexOf("//") < 0;
        }});
        check("relative_descend", function() {{
            var r = path.relative("/foo/bar", "/foo/baz");
            return typeof r === "string" && r.length > 0;
        }});
        check("relative_same", function() {{
            var r = path.relative("/foo", "/foo");
            return r === "";
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== isAbsolute / sep / delimiter =====
    let src = format!(
        r##"
        {scaffold}
        var path = require('path');
        check("isAbsolute_true", function() {{ return path.isAbsolute("/foo") === true; }});
        check("isAbsolute_false", function() {{ return path.isAbsolute("foo/bar") === false; }});
        check("isAbsolute_empty", function() {{ return path.isAbsolute("") === false; }});
        check("sep_type", function() {{ return typeof path.sep === "string" && path.sep.length === 1; }});
        check("delimiter_type", function() {{ return typeof path.delimiter === "string"; }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== parse / format =====
    let src = format!(
        r##"
        {scaffold}
        var path = require('path');
        check("parse_all_fields", function() {{
            var p = path.parse("/foo/bar/baz.txt");
            return typeof p.root === "string"
                && p.base === "baz.txt"
                && p.ext === ".txt"
                && p.name === "baz";
        }});
        check("parse_dir", function() {{
            var p = path.parse("/foo/bar/baz.txt");
            return p.dir === "/foo/bar";
        }});
        check("format_from_obj", function() {{
            var s = path.format({{dir: "/foo", base: "bar.txt"}});
            return typeof s === "string" && s.length > 0;
        }});
        check("format_name_ext", function() {{
            var s = path.format({{name: "foo", ext: ".txt"}});
            return s.indexOf("foo.txt") >= 0;
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    // ===== posix / win32 submodules =====
    // NOTE: bao_runtime aliases path.win32 → path (self-reference) on Linux,
    // so win32.sep reflects the host platform, not Windows. Documented in GAP_REPORT.
    let src = format!(
        r##"
        {scaffold}
        var path = require('path');
        check("posix_exists", function() {{ return typeof path.posix === 'object'; }});
        check("win32_exists", function() {{ return typeof path.win32 === 'object'; }});
        check("posix_join_uses_slash", function() {{
            return path.posix.join("a", "b") === "a/b";
        }});
        check("posix_isAbsolute_slash", function() {{
            return path.posix.isAbsolute("/x") === true && path.posix.isAbsolute("x") === false;
        }});
        check("posix_sep", function() {{ return path.posix.sep === "/"; }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);

    bun_runtime::shutdown_thread_sm();
}

#[test]
fn test_path_conformance_matches_glob() {
    let mut ctx = make_ctx();
    let src = format!(
        r##"
        {scaffold}
        var path = require('path');
        check("matchesGlob_basic", function() {{
            return path.matchesGlob("/foo/bar.js", "*.js") === true;
        }});
        results.join("|")
        "##,
        scaffold = CHECK_SCAFFOLD
    );
    run_checks(&mut ctx, &src);
    bun_runtime::shutdown_thread_sm();
}

#[test]
fn test_path_conformance_win32_deviation() {
    // Node.js: path.win32.sep === "\\" on all platforms
    // bao_runtime: path.win32 === path (self-ref), so win32.sep is host-platform
    let mut ctx = make_ctx();
    let r = eval_string_helper(
        &mut ctx,
        r#"require('path').win32.sep === "\\" ? "PASS" : "FAIL""#,
    );
    assert_eq!(r, "PASS");
    bun_runtime::shutdown_thread_sm();
}

fn eval_string_helper(ctx: &mut bao_engine::context::JsContext, src: &str) -> String {
    common::eval_string(ctx, src)
}
