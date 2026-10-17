// @trace TEST-ENG-007-CP-PATH-TTY-C5 [req:REQ-ENG-007] [level:integration]
//
// C5 line-coverage wave for src/bao_runtime/src/{node_child_process.rs,
// node_path.rs, node_tty.rs}. 既有族(child_process_deep/carrier/spawn_events/
// node_path/path_deep/node_tty_deep/perf_readline_tty)已覆盖主路径;本文件只补
// 缺口分支:
//   * path — posix/win32 双塔全方法变体与边界(node v24.19.0 oracle 逐值 pin),
//     错误臂(argc=0 / 非字符串 / parse/format 专属消息), matchesGlob,
//     win32 resolve/relative/toNamespacedPath 的 cwd 无关绝对向量。
//   * child_process — spawn/exec/execFile 变体与错误臂, SyncOutputCarrier
//     encoding 四象限(latin1/binary/ascii/utf16le/ucs2/utf-8 别名 + 不支持
//     encoding fail-closed), execSync/execFileSync 失败抛出形状, exec 回调
//     成败两臂, kill 信号名/号/缺省/未知名, __cp_* 内部 native 错误臂,
//     fork/spawn 缺参臂, stdin write+end 真回路。
//   * tty — 非 tty 环境(canonica env)的 Refused 语义: isatty 非整型臂,
//     ReadStream/WriteStream 缺省 fd, resume/pause/destroy/ref/unref,
//     setRawMode 无效 fd / 非 tty fd 拒绝, WriteStream 真文件 fd 转义字节
//     逐字节断言, 真 pty(unix openpty)的 isTTY/columns/getWindowSize/
//     setRawMode 成功臂。
//
// 分支触达对照表见任务回报。失败=真缺陷具名登记不修(migration-stage: adapter)。

use bao_engine::context::JsContext;
#[path = "common/mod.rs"]
mod common;

use common::eval_str;
use common::eval_string;
use common::setup_ctx;

/// Evaluate a label-check body (`results.join("|")` contract) and assert
/// every label passed, echoing failures with their captured diagnostics.
fn assert_labels(ctx: &mut JsContext, body: &str, min_pass: usize) {
    let results = eval_string(ctx, body);
    let mut pass = 0usize;
    let mut failures: Vec<String> = Vec::new();
    for item in results.split('|') {
        if item.is_empty() {
            continue;
        }
        if item.contains(" PASS") {
            pass += 1;
        } else {
            failures.push(item.to_string());
        }
    }
    assert!(
        failures.is_empty(),
        "{} label check(s) failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert!(pass >= min_pass, "expected >= {min_pass} passes, got {pass}");
}

/// JS preamble shared by every label-check body: results array + check().
const CHECK_PREAMBLE: &str = r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch (e) { results.push(label + " ERR:" + (e && e.message ? String(e.message).substring(0, 90) : String(e))); }
        }
"#;

// ═══════════════════════════════════════════════════════════════════════════
// §1 path — posix face + host face (pure string algorithms, every platform)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn c5_path_posix_face() {
    crate::exit_isolation::dispatch_timeout(
        "cp_path_tty_coverage_c5_tests::c5_path_posix_face",
        c5_path_posix_face_body,
    );
}

fn c5_path_posix_face_body() {
    let mut ctx = setup_ctx();

    // Values pinned against the node v24.19.0 oracle (2026-10-17 probe).
    assert_labels(
        &mut ctx,
        &format!(
            r#"
        {CHECK_PREAMBLE}
        var p = require("path");
        var x = p.posix;

        // ── normalize: trailing-separator preservation / dot-dot clamping ──
        check("px_norm_trailing_survivor", function() {{ return x.normalize("bar/foo../../") === "bar/"; }});
        check("px_norm_dotdot_beyond_root", function() {{ return x.normalize("/a/../../b") === "/b"; }});
        check("px_norm_empty", function() {{ return x.normalize("") === "."; }});
        check("px_norm_rel_to_dot", function() {{ return x.normalize("a/b/../..") === "."; }});
        check("px_norm_collapse", function() {{ return x.normalize("./a//b/./c/") === "a/b/c/"; }});
        check("px_norm_leading_dotdot", function() {{ return x.normalize("../a") === "../a"; }});

        // ── join: dot resolution, absolute mid-arg is NOT a reset ──
        check("px_join_dots_above_root", function() {{ return x.join("a", "..", "..") === ".."; }});
        check("px_join_abs_mid_no_reset", function() {{ return x.join("a", "/b", "c") === "a/b/c"; }});
        check("px_join_empty_args", function() {{ return x.join() === "."; }});
        check("px_join_all_empty", function() {{ return x.join("", "") === "."; }});
        check("px_join_skips_empty", function() {{ return x.join("", "x") === "x"; }});
        check("px_join_mixed_seps", function() {{ return x.join("a/", "b//", "c/") === "a/b/c/"; }});

        // ── dirname: root forms ──
        check("px_dirname_single_seg_rooted", function() {{ return x.dirname("/a") === "/"; }});
        check("px_dirname_root", function() {{ return x.dirname("/") === "/"; }});
        check("px_dirname_double_slash", function() {{ return x.dirname("//a/b") === "//a"; }});
        check("px_dirname_trailing_stripped", function() {{ return x.dirname("/a/b/") === "/a"; }});
        check("px_dirname_bare", function() {{ return x.dirname("a") === "."; }});

        // ── basename: suffix scan faces ──
        check("px_basename_root_only", function() {{ return x.basename("/") === ""; }});
        check("px_basename_suffix_mismatch", function() {{ return x.basename("/a/b.txt", ".jsx") === "b.txt"; }});
        check("px_basename_suffix_equals_path", function() {{ return x.basename("b.txt", "b.txt") === ""; }});
        check("px_basename_suffix_spans_sep", function() {{ return x.basename("/a.b/c", "b/c") === "c"; }});
        check("px_basename_trailing_stripped", function() {{ return x.basename("/dir/") === "dir"; }});
        check("px_basename_empty", function() {{ return x.basename("") === ""; }});

        // ── extname: dot-only / dotfile / trailing separators ──
        check("px_extname_dotdot", function() {{ return x.extname("..") === ""; }});
        check("px_extname_dotfile", function() {{ return x.extname(".bashrc") === ""; }});
        check("px_extname_dotdir", function() {{ return x.extname("a/.bashrc") === ""; }});
        check("px_extname_trailing_sep", function() {{ return x.extname("file.ext/") === ".ext"; }});
        check("px_extname_dots_only", function() {{ return x.extname("...") === "."; }});
        check("px_extname_no_sep_no_dot", function() {{ return x.extname("file") === ""; }});

        // ── parse: dotfile / root-only / '..' base / trailing separator ──
        check("px_parse_dotfile", function() {{
            var r = x.parse(".bashrc");
            return r.root === "" && r.dir === "" && r.base === ".bashrc" && r.ext === "" && r.name === ".bashrc";
        }});
        check("px_parse_root_only", function() {{
            var r = x.parse("/");
            return r.root === "/" && r.dir === "/" && r.base === "" && r.ext === "" && r.name === "";
        }});
        check("px_parse_ddot_base", function() {{
            var r = x.parse("a/..");
            return r.dir === "a" && r.base === ".." && r.name === ".." && r.ext === "";
        }});
        check("px_parse_trailing_sep", function() {{
            var r = x.parse("/a/b/");
            return r.root === "/" && r.dir === "/a" && r.base === "b" && r.ext === "" && r.name === "b";
        }});
        check("px_parse_full", function() {{
            var r = x.parse("/a/b/c.txt");
            return r.root === "/" && r.dir === "/a/b" && r.base === "c.txt" && r.ext === ".txt" && r.name === "c";
        }});

        // ── format: root-join vs dir+sep, formatExt dot supply ──
        check("px_format_root_name_ext", function() {{ return x.format({{ root: "/", name: "b", ext: ".txt" }}) === "/b.txt"; }});
        check("px_format_dir_ext_no_dot", function() {{ return x.format({{ dir: "/a", name: "b", ext: "txt" }}) === "/a/b.txt"; }});
        check("px_format_base_wins", function() {{ return x.format({{ dir: "/a", base: "b.txt", name: "ignored", ext: ".ignored" }}) === "/a/b.txt"; }});
        check("px_format_empty", function() {{ return x.format({{}}) === ""; }});

        // ── relative / resolve (absolute args — cwd independent) ──
        check("px_relative_updown", function() {{ return x.relative("/a/b/c", "/a/d") === "../../d"; }});
        check("px_relative_into", function() {{ return x.relative("/a", "/a/b/c") === "b/c"; }});
        check("px_relative_same", function() {{ return x.relative("/a/b", "/a/b") === ""; }});
        check("px_resolve_dotdot_root_clamp", function() {{ return x.resolve("/a", "../../b") === "/b"; }});

        // ── host face === posix face on a posix host; toNamespacedPath identity ──
        check("px_host_norm_matches_posix", function() {{ return p.normalize("/a/../../b") === "/b"; }});
        check("px_host_basename_matches", function() {{ return p.basename("a/b.txt") === "b.txt"; }});
        check("px_host_tnp_identity", function() {{ return p.toNamespacedPath("/a/b.txt") === "/a/b.txt"; }});

        results.join("|")
    "#,
        ),
        40,
    );

    // posix host identity: path === path.posix (module.exports IS the posix
    // object). Windows builds carry a separate posix face — gate by host.
    if !cfg!(windows) {
        let got = eval_str(&mut ctx, "var p = require('path'); (p.posix === p) ? 'self' : 'separate'");
        assert_eq!(got, "self", "posix host: path.posix must be the host object itself");
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// §2 path — win32 face (the genuine Windows algorithm on every platform)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn c5_path_win32_face() {
    crate::exit_isolation::dispatch_timeout(
        "cp_path_tty_coverage_c5_tests::c5_path_win32_face",
        c5_path_win32_face_body,
    );
}

fn c5_path_win32_face_body() {
    let mut ctx = setup_ctx();

    // `B` is one literal backslash in JS; every vector below is byte-pinned
    // against the node v24.19.0 oracle (2026-10-17 probe, /tmp/oracle_path.js).
    assert_labels(
        &mut ctx,
        r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch (e) { results.push(label + " ERR:" + (e && e.message ? String(e.message).substring(0, 90) : String(e))); }
        }
        var p = require("path");
        var w = p.win32;
        var B = "\\";

        // ── normalize: device / UNC roots, mixed separators ──
        check("w_norm_mixed_seps", function() { return w.normalize("a/b" + B + "c") === "a" + B + "b" + B + "c"; });
        check("w_norm_trailing", function() { return w.normalize("a" + B + "b" + B) === "a" + B + "b" + B; });
        check("w_norm_dotdot_up", function() { return w.normalize("a" + B + ".." + B + ".." + B + "c") === ".." + B + "c"; });
        check("w_norm_unc_up", function() { return w.normalize(B + B + "server" + B + "share" + B + ".." + B + "x") === B + B + "server" + B + "share" + B + "x"; });
        check("w_norm_drive_up", function() { return w.normalize("C:" + B + "a" + B + ".." + B + "..") === "C:" + B; });
        check("w_norm_bare_device", function() { return w.normalize("C:") === "C:."; });
        check("w_norm_device_relative", function() { return w.normalize("C:a" + B + "b") === "C:a" + B + "b"; });
        check("w_norm_single_bslash", function() { return w.normalize(B) === B; });
        check("w_norm_empty", function() { return w.normalize("") === "."; });
        check("w_norm_dotdot_at_root", function() { return w.normalize(B + ".." + B + "..") === B; });
        check("w_norm_fwd_root", function() { return w.normalize("/a/../b") === B + "b"; });

        // ── join: UNC first part keeps the double-root; collapse shapes ──
        check("w_join_unc_first", function() { return w.join(B + B + "s" + B + "sh", "a", "b") === B + B + "s" + B + "sh" + B + "a" + B + "b"; });
        check("w_join_lead2_keeps_unc_probe", function() { return w.join(B + B + "a", "b") === B + B + "a" + B + "b" + B; });
        check("w_join_lead3_collapses", function() { return w.join(B + B + B + "a", "b") === B + "a" + B + "b"; });
        check("w_join_mixed", function() { return w.join("a/", "b" + B + B, "c") === "a" + B + "b" + B + "c"; });
        check("w_join_dots", function() { return w.join("C:" + B + "a", "..", "b") === "C:" + B + "b"; });
        check("w_join_fwd", function() { return w.join("a/b", "c") === "a" + B + "b" + B + "c"; });
        check("w_join_empty_parts", function() { return w.join("", "", "x") === "x"; });
        check("w_join_all_empty", function() { return w.join("", "") === "."; });

        // ── dirname: UNC / device roots ──
        check("w_dirname_unc", function() { return w.dirname(B + B + "s" + B + "sh" + B + "a" + B + "b") === B + B + "s" + B + "sh" + B + "a"; });
        check("w_dirname_unc_share_only", function() { return w.dirname(B + B + "s" + B + "sh" + B) === B + B + "s" + B + "sh" + B; });
        check("w_dirname_drive", function() { return w.dirname("C:" + B + "a" + B + "b") === "C:" + B + "a"; });
        check("w_dirname_device_rel", function() { return w.dirname("C:a" + B + "b") === "C:a"; });
        check("w_dirname_device_root", function() { return w.dirname("C:" + B) === "C:" + B; });
        check("w_dirname_fwd_tail", function() { return w.dirname("C:a/b") === "C:a"; });
        check("w_dirname_bare_char", function() { return w.dirname("x") === "."; });

        // ── basename: drive skip + both separators ──
        check("w_basename_drive", function() { return w.basename("C:" + B + "a" + B + "f.txt") === "f.txt"; });
        check("w_basename_slash_only", function() { return w.basename(B + B) === ""; });
        check("w_basename_suffix", function() { return w.basename("C:" + B + "a" + B + "f.txt", ".txt") === "f"; });
        check("w_basename_both_seps", function() { return w.basename("a/b" + B + "c") === "c"; });
        check("w_basename_device_only", function() { return w.basename("C:") === ""; });
        check("w_basename_no_ext_suffix", function() { return w.basename("C:\\a\\f", "f") === ""; });

        // ── extname ──
        check("w_extname_mixed", function() { return w.extname("a" + B + "b.c" + B + "d.e") === ".e"; });
        check("w_extname_trailing", function() { return w.extname("file.ext" + B + B) === ".ext"; });
        check("w_extname_dotfile", function() { return w.extname(".bashrc") === ""; });

        // ── isAbsolute: rooted-without-drive IS absolute; bare device is NOT ──
        check("w_isabs_rooted", function() { return w.isAbsolute(B + "foo") === true; });
        check("w_isabs_fwd_rooted", function() { return w.isAbsolute("/foo") === true; });
        check("w_isabs_drive", function() { return w.isAbsolute("C:" + B + "a") === true; });
        check("w_isabs_device_rel", function() { return w.isAbsolute("C:a") === false; });
        check("w_isabs_device_bare", function() { return w.isAbsolute("C:") === false; });
        check("w_isabs_empty", function() { return w.isAbsolute("") === false; });

        // ── parse: UNC / device-rooted / device-relative / bare-root shapes ──
        check("w_parse_unc", function() {
            var r = w.parse(B + B + "s" + B + "sh" + B + "a" + B + "f.txt");
            return r.root === B + B + "s" + B + "sh" + B && r.dir === B + B + "s" + B + "sh" + B + "a"
                && r.base === "f.txt" && r.ext === ".txt" && r.name === "f";
        });
        check("w_parse_unc_root_only", function() {
            var r = w.parse(B + B + "s" + B + "sh" + B);
            return r.root === B + B + "s" + B + "sh" + B && r.dir === B + B + "s" + B + "sh" + B && r.base === "";
        });
        check("w_parse_device_rel", function() {
            var r = w.parse("C:a" + B + "f.txt");
            return r.root === "C:" && r.dir === "C:a" && r.base === "f.txt" && r.ext === ".txt" && r.name === "f";
        });
        check("w_parse_device_root", function() {
            var r = w.parse("C:" + B);
            return r.root === "C:" + B && r.dir === "C:" + B && r.base === "" && r.name === "";
        });
        check("w_parse_device_bare", function() {
            var r = w.parse("C:");
            return r.root === "C:" && r.dir === "C:" && r.base === "";
        });
        check("w_parse_single_bslash", function() {
            var r = w.parse(B);
            return r.root === B && r.dir === B && r.base === "";
        });
        check("w_parse_fwd_tail", function() {
            var r = w.parse("C:a/f.txt");
            return r.root === "C:" && r.dir === "C:a" && r.base === "f.txt";
        });
        check("w_parse_relative", function() {
            var r = w.parse("a.txt");
            return r.root === "" && r.dir === "" && r.base === "a.txt" && r.ext === ".txt" && r.name === "a";
        });

        // ── format: dir-vs-root join on the win32 separator ──
        check("w_format_dir_base", function() { return w.format({ dir: "C:" + B + "a", base: "b.txt" }) === "C:" + B + "a" + B + "b.txt"; });
        check("w_format_root_base", function() { return w.format({ root: "C:" + B, base: "b" }) === "C:" + B + "b"; });
        check("w_format_name_ext_nodot", function() { return w.format({ name: "b", ext: "txt" }) === "b.txt"; });
        check("w_format_dir_only", function() { return w.format({ dir: "C:" + B + "a" }) === "C:" + B + "a" + B; });
        check("w_format_base_only", function() { return w.format({ base: "x" }) === "x"; });
        check("w_format_empty", function() { return w.format({}) === ""; });
        check("w_format_root_empty_base", function() { return w.format({ root: "C:" + B }) === "C:" + B; });

        // ── resolve: device-qualified, cwd-independent absolute vectors ──
        check("w_resolve_drive_first", function() { return w.resolve("C:" + B + "a" + B + "b", "c") === "C:" + B + "a" + B + "b" + B + "c"; });
        check("w_resolve_device_last_wins", function() { return w.resolve("a", "C:" + B + "b") === "C:" + B + "b"; });
        check("w_resolve_rooted_resets", function() { return w.resolve("C:" + B + "a", B + "b") === "C:" + B + "b"; });
        check("w_resolve_rel_tail", function() { return w.resolve("C:" + B + "a", "b", "..") === "C:" + B + "a"; });
        check("w_resolve_lower_drive", function() { return w.resolve("c:" + B + "a" + B + "b", "c") === "c:" + B + "a" + B + "b" + B + "c"; });
        check("w_resolve_unc", function() { return w.resolve(B + B + "s" + B + "sh" + B + "a", "b") === B + B + "s" + B + "sh" + B + "a" + B + "b"; });

        // ── relative: case-insensitive device compare, cross-device, UNC ──
        check("w_relative_siblings", function() { return w.relative("C:" + B + "a" + B + "b", "C:" + B + "a" + B + "c") === ".." + B + "c"; });
        check("w_relative_cross_device", function() { return w.relative("C:" + B + "a", "D:" + B + "b") === "D:" + B + "b"; });
        check("w_relative_case_insensitive", function() { return w.relative("C:" + B + "A" + B + "b", "c:" + B + "a" + B + "b" + B + "c") === "c"; });
        check("w_relative_unc", function() { return w.relative(B + B + "s" + B + "sh" + B + "a", B + B + "s" + B + "sh" + B + "b") === ".." + B + "b"; });
        check("w_relative_into", function() { return w.relative("C:" + B + "a", "C:" + B + "a" + B + "b" + B + "c") === "b" + B + "c"; });
        check("w_relative_from_device_root", function() { return w.relative("C:" + B, "C:" + B + "foo") === "foo"; });
        check("w_relative_to_device_root", function() { return w.relative("C:" + B + "foo" + B + "bar", "C:" + B) === ".." + B + ".."; });

        // ── toNamespacedPath: \\?\ and \\?\UNC\ prefixes; long forms verbatim ──
        check("w_tnp_drive", function() { return w.toNamespacedPath("C:" + B + "a" + B + "b") === B + B + "?" + B + "C:" + B + "a" + B + "b"; });
        check("w_tnp_unc", function() { return w.toNamespacedPath(B + B + "s" + B + "sh" + B + "a") === B + B + "?" + B + "UNC" + B + "s" + B + "sh" + B + "a"; });
        check("w_tnp_long_verbatim", function() { return w.toNamespacedPath(B + B + "?") === B + B + "?"; });
        check("w_tnp_device_verbatim", function() { return w.toNamespacedPath(B + B + "." + B + "a") === B + B + "." + B + "a"; });
        check("w_tnp_fwd_abs", function() { return w.toNamespacedPath("/a/b") === B + "a" + B + "b"; });

        // ── face wiring: win32 self-ref + posix alias; separators ──
        check("w_win32_self_ref", function() { return w.win32 === w; });
        check("w_posix_alias", function() { return w.posix === p.posix; });
        check("w_sep", function() { return w.sep === B; });
        check("w_delimiter", function() { return w.delimiter === ";"; });

        results.join("|")
    "#,
    72,
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// §3 path — error arms + matchesGlob + module shape
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn c5_path_errors_matchesglob_shape() {
    crate::exit_isolation::dispatch_timeout(
        "cp_path_tty_coverage_c5_tests::c5_path_errors_matchesglob_shape",
        c5_path_errors_matchesglob_shape_body,
    );
}

fn c5_path_errors_matchesglob_shape_body() {
    let mut ctx = setup_ctx();

    assert_labels(
        &mut ctx,
        &format!(
            r#"
        {CHECK_PREAMBLE}
        var p = require("path");

        function msgOf(fn) {{
            try {{ fn(); return "NO-THROW"; }}
            catch (e) {{ return String(e && e.message ? e.message : e); }}
        }}

        // ── host-face error arms ──
        check("err_join_null", function() {{ return msgOf(function() {{ p.join(null); }}).indexOf("must be of type string") >= 0; }});
        check("err_dirname_noargs", function() {{ return msgOf(function() {{ p.dirname(); }}).indexOf("must be of type string") >= 0; }});
        check("err_dirname_null", function() {{ return msgOf(function() {{ p.dirname(null); }}).indexOf("must be of type string") >= 0; }});
        check("err_basename_noargs", function() {{ return msgOf(function() {{ p.basename(); }}).indexOf("must be of type string") >= 0; }});
        check("err_basename_null_suffix", function() {{ return msgOf(function() {{ p.basename("a", null); }}).indexOf("must be of type string") >= 0; }});
        check("err_extname_noargs", function() {{ return msgOf(function() {{ p.extname(); }}).indexOf("must be of type string") >= 0; }});
        check("err_normalize_noargs", function() {{ return msgOf(function() {{ p.normalize(); }}).indexOf("must be of type string") >= 0; }});
        check("err_relative_onearg", function() {{ return msgOf(function() {{ p.relative("a"); }}).indexOf("must be of type string") >= 0; }});
        check("err_parse_noargs", function() {{ return msgOf(function() {{ p.parse(); }}).indexOf("path.parse requires a path") >= 0; }});
        check("err_format_noargs", function() {{ return msgOf(function() {{ p.format(); }}).indexOf("pathObject") >= 0; }});
        check("err_format_nonobject", function() {{ return msgOf(function() {{ p.format(42); }}).indexOf("pathObject") >= 0; }});

        // ── win32-face error arms (posix_str_fn macro message) ──
        check("err_w32_join_null", function() {{ return msgOf(function() {{ p.win32.join(null); }}).indexOf("invalid argument") >= 0; }});
        check("err_w32_normalize_null", function() {{ return msgOf(function() {{ p.win32.normalize(null); }}).indexOf("invalid argument") >= 0; }});

        // ── non-throwing absence arms: isAbsolute / toNamespacedPath ──
        check("err_isabs_noargs_false", function() {{ return p.isAbsolute() === false; }});
        check("err_isabs_null_false", function() {{ return p.isAbsolute(null) === false; }});
        check("err_isabs_num_false", function() {{ return p.isAbsolute(5) === false; }});
        check("err_w32_isabs_noargs_false", function() {{ return p.win32.isAbsolute() === false; }});
        check("err_tnp_noargs_undef", function() {{ return p.toNamespacedPath() === undefined; }});
        check("err_tnp_null_undef", function() {{ return p.toNamespacedPath(null) === undefined; }});
        check("err_w32_tnp_noargs_undef", function() {{ return p.win32.toNamespacedPath() === undefined; }});

        // ── child_process missing-command / missing-arg arms live in §5; here:
        // ── module shape: sep / platform / delimiter / posix+win32 identity ──
        check("shape_sep_host", function() {{
            var want = {sep};
            return p.sep === want;
        }});
        check("shape_platform_matches_process", function() {{ return p.platform === process.platform; }});
        check("shape_delimiter_host", function() {{
            var want = {delim};
            return p.delimiter === want;
        }});
        check("shape_posix_is_object", function() {{ return typeof p.posix === "object"; }});
        check("shape_win32_is_object", function() {{ return typeof p.win32 === "object"; }});

        // ── matchesGlob (node-parity cases; wildcard translator faces) ──
        check("glob_dblstar_crosses", function() {{ return p.matchesGlob("src/a/b.js", "**/*.js") === true; }});
        check("glob_no_match", function() {{ return p.matchesGlob("a.txt", "*.md") === false; }});
        check("glob_qmark", function() {{ return p.matchesGlob("a.txt", "?.txt") === true; }});
        check("glob_char_class", function() {{ return p.matchesGlob("a.txt", "[ab].txt") === true; }});
        check("glob_literal_dot", function() {{ return p.matchesGlob("a.b/c", "a.b/c") === true; }});
        check("glob_path_typeerror", function() {{ return msgOf(function() {{ p.matchesGlob(42, "*"); }}).indexOf("path") >= 0; }});
        check("glob_pattern_typeerror", function() {{ return msgOf(function() {{ p.matchesGlob("a", 42); }}).indexOf("pattern") >= 0; }});

        results.join("|")
    "#,
        sep = if cfg!(windows) { r#"\"\\""# } else { r#""/""# },
        delim = if cfg!(windows) { r#"";""# } else { r#"":"# },
    ),
    30,
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// §4 child_process — sync faces: encoding carrier quadrants + failure shapes
// ═══════════════════════════════════════════════════════════════════════════

#[test]
#[cfg(unix)] // POSIX program content (/bin/sh, printf) — see carrier_tests for the windows arms
fn c5_cp_sync_faces_deep() {
    crate::exit_isolation::dispatch_timeout(
        "cp_path_tty_coverage_c5_tests::c5_cp_sync_faces_deep",
        c5_cp_sync_faces_deep_body,
    );
}

#[cfg(unix)]
fn c5_cp_sync_faces_deep_body() {
    let mut ctx = setup_ctx();

    assert_labels(
        &mut ctx,
        r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch (e) { results.push(label + " ERR:" + (e && e.message ? String(e.message).substring(0, 90) : String(e))); }
        }
        var cp = require("child_process");

        // ── spawnSync: full result shape on success and on failure ──
        var r = cp.spawnSync("/bin/sh", ["-c", "printf sync-ok; echo err-line >&2; exit 6"]);
        check("sync_status_nonzero", function() { return r.status === 6; });
        check("sync_pid_positive", function() { return typeof r.pid === "number" && r.pid > 0; });
        check("sync_signal_null", function() { return r.signal === null; });
        check("sync_error_null", function() { return r.error === null; });
        check("sync_stdout_buffer", function() { return Buffer.isBuffer(r.stdout) && r.stdout.toString() === "sync-ok"; });
        check("sync_stderr_buffer", function() { return Buffer.isBuffer(r.stderr) && r.stderr.toString().indexOf("err-line") >= 0; });
        check("sync_output_array", function() {
            return Array.isArray(r.output) && r.output.length === 3 && r.output[0] === null
                && Buffer.isBuffer(r.output[1]) && Buffer.isBuffer(r.output[2]);
        });

        // ── spawnSync ENOENT: fail object, not a throw ──
        var re = cp.spawnSync("/definitely-not-c5-xyz");
        check("sync_enoent_error_truthy", function() { return typeof re === "object" && re !== null && !!re.error; });
        check("sync_enoent_status_neg1", function() { return re.status === -1; });

        // ── spawnSync encoding quadrants (SyncOutputCarrier) ──
        function sh(script, opts) { return cp.spawnSync("/bin/sh", ["-c", script], opts); }
        var r8 = sh("printf héllo", { encoding: "utf8" });
        check("enc_utf8_string", function() { return typeof r8.stdout === "string" && r8.stdout.indexOf("héllo") >= 0; });
        var r8alias = sh("printf héllo", { encoding: "utf-8" });
        check("enc_utf8_alias", function() { return typeof r8alias.stdout === "string" && r8alias.stdout.indexOf("héllo") >= 0; });
        var rb = sh("printf héllo", { encoding: "buffer" });
        check("enc_buffer_face", function() { return Buffer.isBuffer(rb.stdout); });
        var rl = sh("printf '\\303\\251'", { encoding: "latin1" });
        check("enc_latin1_bytes", function() {
            return rl.stdout.length === 2 && rl.stdout.charCodeAt(0) === 0xC3 && rl.stdout.charCodeAt(1) === 0xA9;
        });
        var rbin = sh("printf '\\303\\251'", { encoding: "binary" });
        check("enc_binary_alias_latin1", function() {
            return rbin.stdout.length === 2 && rbin.stdout.charCodeAt(0) === 0xC3 && rbin.stdout.charCodeAt(1) === 0xA9;
        });
        var ra = sh("printf '\\303\\251'", { encoding: "ascii" });
        check("enc_ascii_masks_high_bit", function() {
            return ra.stdout.length === 2 && ra.stdout.charCodeAt(0) === 0x43 && ra.stdout.charCodeAt(1) === 0x29;
        });
        var ru = sh("printf 'A\\000B\\000'", { encoding: "utf16le" });
        check("enc_utf16le_decode", function() { return ru.stdout === "AB"; });
        var rcs = sh("printf 'A\\000B\\000'", { encoding: "ucs2" });
        check("enc_ucs2_alias", function() { return rcs.stdout === "AB"; });
        var rcsd = sh("printf 'A\\000B\\000'", { encoding: "ucs-2" });
        check("enc_ucs2_dash_alias", function() { return rcsd.stdout === "AB"; });
        var ru16d = sh("printf 'A\\000B\\000'", { encoding: "utf-16le" });
        check("enc_utf16le_dash_alias", function() { return ru16d.stdout === "AB"; });
        check("enc_unsupported_throws", function() {
            try { sh("printf x", { encoding: "base64" }); return "NO-THROW"; }
            catch (e) { return String(e.message).indexOf("unsupported encoding") >= 0; }
        });

        // ── spawnSync 3-arg (args array + options) carrier path ──
        var r3 = cp.spawnSync("/bin/sh", ["-c", "printf three-arg"], { encoding: "utf8" });
        check("sync3_args_array_carrier", function() { return r3.stdout === "three-arg"; });
        // ── spawnSync 2-arg options-object (no length → carrier parsed) ──
        var r2 = cp.spawnSync("/bin/sh", { encoding: "utf8" });
        check("sync2_opts_object_carrier", function() { return r2.stdout === ""; });

        // ── execSync: success carrier + failure throw shape ──
        var e = cp.execSync("/bin/sh -c 'printf esync-ok'");
        check("execsync_default_buffer", function() { return Buffer.isBuffer(e) && e.toString() === "esync-ok"; });
        var e8 = cp.execSync("/bin/sh -c 'printf esync-ok'", { encoding: "utf8" });
        check("execsync_utf8_string", function() { return e8 === "esync-ok"; });
        check("execsync_nonzero_throw_shape", function() {
            try { cp.execSync("/bin/sh -c 'exit 7'"); return "NO-THROW"; }
            catch (err) {
                return String(err.message).indexOf("Command failed: /bin/sh -c 'exit 7'") === 0
                    && err.status === 7 && typeof err.pid === "number" && err.pid > 0
                    && err.signal === null && Array.isArray(err.output) && err.output.length === 3
                    && err.output[0] === null;
            }
        });
        check("execsync_nonstring_throws", function() {
            try { cp.execSync(42); return "NO-THROW"; }
            catch (err) { return String(err.message).indexOf("requires a string command") >= 0; }
        });
        check("execsync_noargs_throws", function() {
            try { cp.execSync(); return "NO-THROW"; }
            catch (err) { return String(err.message).indexOf("requires a command string") >= 0; }
        });
        check("execsync_unsupported_encoding", function() {
            try { cp.execSync("true", { encoding: "base64" }); return "NO-THROW"; }
            catch (err) { return String(err.message).indexOf("unsupported encoding") >= 0; }
        });

        // ── execFile / execFileSync ──
        var f = cp.execFile("/bin/sh", ["-c", "printf file-ok"]);
        check("execfile_object_stdout", function() { return typeof f.stdout === "string" && f.stdout === "file-ok"; });
        check("execfile_exitcode_zero", function() { return f.exitCode === 0; });
        check("execfile_pid_positive", function() { return f.pid > 0; });
        var fnz = cp.execFile("/bin/sh", ["-c", "exit 5"]);
        check("execfile_nonzero_exitcode", function() { return fnz.exitCode === 5; });
        var fs8 = cp.execFileSync("/bin/sh", ["-c", "printf fsync-ok"], { encoding: "utf8" });
        check("execfilesync_utf8", function() { return fs8 === "fsync-ok"; });
        check("execfilesync_nonzero_throw", function() {
            try { cp.execFileSync("/bin/sh", ["-c", "exit 3"]); return "NO-THROW"; }
            catch (err) {
                return String(err.message).indexOf("execFileSync failed with status 3") === 0 && err.status === 3;
            }
        });
        check("execfilesync_string_args_form", function() {
            // execFileSync(file, "arg") — a string middle argument rides
            // argv[1]; sh treats it as a missing script → exit 127.
            try { cp.execFileSync("/bin/sh", "not-an-array"); return "NO-THROW"; }
            catch (err) { return err.status === 127 && String(err.message).indexOf("execFileSync failed with status 127") === 0; }
        });
        check("execfile_noargs_throws", function() {
            try { cp.execFile(); return "NO-THROW"; }
            catch (err) { return String(err.message).indexOf("requires a file path") >= 0; }
        });
        check("execfile_nonstring_throws", function() {
            try { cp.execFile(42); return "NO-THROW"; }
            catch (err) { return String(err.message).indexOf("requires a string file path") >= 0; }
        });
        check("execfilesync_noargs_throws", function() {
            try { cp.execFileSync(); return "NO-THROW"; }
            catch (err) { return String(err.message).indexOf("requires a file path") >= 0; }
        });
        check("execfilesync_unsupported_encoding", function() {
            try { cp.execFileSync("/bin/sh", ["-c", "true"], { encoding: "base64" }); return "NO-THROW"; }
            catch (err) { return String(err.message).indexOf("unsupported encoding") >= 0; }
        });

        // ── exec: callback success + failure + arg error arms ──
        var cbOk = { done: false, err: "unset", out: "", errout: "unset" };
        cp.exec("printf exec-cb-ok", function(err, so, se) {
            cbOk = { done: true, err: err, out: so, errout: se };
        });
        check("exec_cb_success", function() {
            return cbOk.done && cbOk.err === null && cbOk.out === "exec-cb-ok" && cbOk.errout === "";
        });
        var cbFail = { done: false, msg: "" };
        cp.exec("/bin/sh -c 'exit 7'", function(err) {
            cbFail = { done: true, msg: err ? String(err.message) : "null-err" };
        });
        check("exec_cb_failure_msg", function() {
            return cbFail.done && cbFail.msg === "Command failed with exit code 7";
        });
        var noCb = cp.exec("printf exec-nocb");
        check("exec_nocallback_child_shape", function() {
            return typeof noCb.stdout === "string" && noCb.stdout === "exec-nocb" && noCb.exitCode === 0 && noCb.pid > 0;
        });
        check("exec_noargs_throws", function() {
            try { cp.exec(); return "NO-THROW"; }
            catch (err) { return String(err.message).indexOf("requires a command string") >= 0; }
        });
        check("exec_nonstring_throws", function() {
            try { cp.exec(42); return "NO-THROW"; }
            catch (err) { return String(err.message).indexOf("requires a string command") >= 0; }
        });
        check("exec_enoent_shell_still_runs", function() {
            // The SHELL exists; a missing command inside it exits 127.
            try { cp.exec("definitely-not-c5-xyz"); return "NO-THROW"; }
            catch (err) { return err.status === 127; }
        });

        // ── spawnSync missing-arg arms ──
        check("syncspawn_noargs_throws", function() {
            try { cp.spawnSync(); return "NO-THROW"; }
            catch (err) { return String(err.message).indexOf("requires a command") >= 0; }
        });
        check("syncspawn_nonstring_throws", function() {
            try { cp.spawnSync(42); return "NO-THROW"; }
            catch (err) { return String(err.message).indexOf("requires a string command") >= 0; }
        });

        results.join("|")
    "#,
    44,
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// §5 child_process — error arms (spawn/fork) + native __cp_* faces
// ═══════════════════════════════════════════════════════════════════════════

#[test]
#[cfg(unix)]
fn c5_cp_native_error_arms() {
    crate::exit_isolation::dispatch_timeout(
        "cp_path_tty_coverage_c5_tests::c5_cp_native_error_arms",
        c5_cp_native_error_arms_body,
    );
}

#[cfg(unix)]
fn c5_cp_native_error_arms_body() {
    let mut ctx = setup_ctx();

    assert_labels(
        &mut ctx,
        r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch (e) { results.push(label + " ERR:" + (e && e.message ? String(e.message).substring(0, 90) : String(e))); }
        }
        var cp = require("child_process");

        // ── spawn argument-contract error arms ──
        check("spawn_noargs_throws", function() {
            try { cp.spawn(); return "NO-THROW"; }
            catch (e) { return String(e.message).indexOf("requires arguments") >= 0; }
        });
        check("spawn_number_throws", function() {
            try { cp.spawn(42); return "NO-THROW"; }
            catch (e) { return String(e.message).indexOf("requires arguments") >= 0; }
        });
        check("spawn_opts_no_command_throws", function() {
            try { cp.spawn({ cwd: "/tmp" }); return "NO-THROW"; }
            catch (e) { return String(e.message).indexOf("missing command") >= 0; }
        });

        // ── fork argument-contract error arms (no real fork) ──
        check("fork_noargs_throws", function() {
            try { cp.fork(); return "NO-THROW"; }
            catch (e) { return String(e.message).indexOf("requires a module path") >= 0; }
        });
        check("fork_nonstring_throws", function() {
            try { cp.fork(42); return "NO-THROW"; }
            catch (e) { return String(e.message).indexOf("requires a string module path") >= 0; }
        });

        // ── __cp_drain / __cp_poll_exit: pid-0 and registry-miss null arms ──
        check("drain_pid0_null", function() { return cp.__cp_drain(0, 0) === null; });
        check("drain_ghost_null", function() { return cp.__cp_drain(424242, 0) === null; });
        check("drain_which_default", function() { return cp.__cp_drain(424242) === null; });
        check("poll_pid0_null", function() { return cp.__cp_poll_exit(0) === null; });
        check("poll_ghost_null", function() { return cp.__cp_poll_exit(424242) === null; });

        // ── __cp_stdin_write / __cp_stdin_close: registry-miss false arms ──
        check("stdin_write_pid0_false", function() { return cp.__cp_stdin_write(0, "x") === false; });
        check("stdin_write_ghost_false", function() { return cp.__cp_stdin_write(424242, "x") === false; });
        check("stdin_close_pid0_false", function() { return cp.__cp_stdin_close(0) === false; });
        check("stdin_close_ghost_false", function() { return cp.__cp_stdin_close(424242) === false; });
        check("stdin_close_noargs_false", function() { return cp.__cp_stdin_close() === false; });

        // ── __cp_kill_child: pid-0 false arm + live-signal arm ──
        check("killchild_pid0_false", function() { return cp.__cp_kill_child(0, 15) === false; });
        check("killchild_noargs_false", function() { return cp.__cp_kill_child() === false; });
        var victim = cp.spawn("/bin/sh", ["-c", "sleep 30"]);
        check("killchild_live_signal_true", function() { return cp.__cp_kill_child(victim.pid, 9) === true; });

        // ── IPC channel registry-miss / pid-0 arms ──
        check("ipc_send_pid0_false", function() { return cp.__cp_ipc_send(0, "{}") === false; });
        check("ipc_send_ghost_throws", function() {
            try { cp.__cp_ipc_send(424242, "{}"); return "NO-THROW"; }
            catch (e) { return String(e.message).indexOf("no ipc channel") >= 0; }
        });
        check("ipc_send_ghost_negfd_throws", function() {
            try { cp.__cp_ipc_send(424242, "{}", -1); return "NO-THROW"; }
            catch (e) { return String(e.message).indexOf("no ipc channel") >= 0; }
        });
        check("ipc_recv_pid0_null", function() { return cp.__cp_ipc_recv(0) === null; });
        check("ipc_recv_ghost_throws", function() {
            try { cp.__cp_ipc_recv(424242); return "NO-THROW"; }
            catch (e) { return String(e.message).indexOf("no ipc channel") >= 0; }
        });
        check("ipc_recv_noargs_null", function() { return cp.__cp_ipc_recv() === null; });

        results.join("|")
    "#,
    22,
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// §6 child_process — async lifecycle: kill signal faces + stdin roundtrip
// ═══════════════════════════════════════════════════════════════════════════

#[test]
#[cfg(unix)]
fn c5_cp_async_kill_and_stdin() {
    crate::exit_isolation::dispatch_timeout(
        "cp_path_tty_coverage_c5_tests::c5_cp_async_kill_and_stdin",
        c5_cp_async_kill_and_stdin_body,
    );
}

#[cfg(unix)]
fn c5_cp_async_kill_and_stdin_body() {
    let mut ctx = setup_ctx();
    ctx.set_post_eval_hook(common::bounded_drain_hook);

    let setup = eval_str(
        &mut ctx,
        r#"
        var cp = require("child_process");
        globalThis.__klog = [];
        globalThis.__kill9done = false;
        globalThis.__termdone = false;

        // kill('SIGKILL') → exit event with the signal recorded.
        var slow9 = cp.spawn("/bin/sh", ["-c", "sleep 30"]);
        globalThis.__klog.push("pid9=" + (slow9.pid > 0));
        slow9.on("exit", function(code, sig) {
            globalThis.__klog.push("kill9=" + code + "/" + sig);
            globalThis.__kill9done = true;
        });
        var ret9 = slow9.kill("SIGKILL");
        globalThis.__klog.push("ret9=" + ret9 + "/flag=" + slow9.killed);

        // kill() default → SIGTERM.
        var slowT = cp.spawn("/bin/sh", ["-c", "sleep 30"]);
        slowT.on("exit", function(code, sig) {
            globalThis.__klog.push("term=" + code + "/" + sig);
            globalThis.__termdone = true;
        });
        slowT.kill();

        // stdin write + end roundtrip: the child reads one line and echoes it.
        globalThis.__stdinGot = "";
        globalThis.__stdindone = false;
        var cat = cp.spawn("/bin/sh", ["-c", "read line; printf 'got:%s' \"$line\""]);
        cat.stdout.on("data", function(d) {
            globalThis.__stdinGot += String.fromCharCode.apply(null, new Uint8Array(d));
        });
        cat.on("close", function() { globalThis.__stdindone = true; });
        cat.stdin.write("ping\n");
        cat.stdin.end();
        globalThis.__catpid = cat.pid;

        // After close, the exit slot is consumed: a direct poll sees null.
        globalThis.__pollAfterClose = "pending";

        "setup-ok"
    "#,
    );
    assert_eq!(setup, "setup-ok");

    let done = common::wait_until_with(
        &mut ctx,
        "(globalThis.__kill9done === true && globalThis.__termdone === true && globalThis.__stdindone === true) ? 'y' : 'n'",
        60,
        80,
        eval_str,
    );
    let klog = eval_str(&mut ctx, "globalThis.__klog.join('|')");
    let stdin_result = eval_str(&mut ctx, "globalThis.__stdinGot");
    assert!(done, "async kill/stdin lifecycle must complete; klog: {klog}; stdin got: {stdin_result:?}");

    for part in ["pid9=true", "ret9=true/flag=true", "kill9=-9/9", "term=-15/15"] {
        assert!(
            klog.split('|').any(|entry| entry == part),
            "kill lifecycle log must contain '{part}', got: {klog}"
        );
    }
    assert_eq!(stdin_result, "got:ping", "stdin write+end roundtrip");

    // Consume-once exit slot: after close, __cp_poll_exit answers null.
    let poll_after = eval_str(
        &mut ctx,
        "String(cp.__cp_poll_exit(globalThis.__catpid))",
    );
    assert_eq!(poll_after, "null", "exit slot must be consumed by the close poll");
}

// ═══════════════════════════════════════════════════════════════════════════
// §7 tty — non-tty (canonical env) refused semantics + module faces
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn c5_tty_nontty_refused_faces() {
    crate::exit_isolation::dispatch_timeout(
        "cp_path_tty_coverage_c5_tests::c5_tty_nontty_refused_faces",
        c5_tty_nontty_refused_faces_body,
    );
}

fn c5_tty_nontty_refused_faces_body() {
    let mut ctx = setup_ctx();

    assert_labels(
        &mut ctx,
        r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch (e) { results.push(label + " ERR:" + (e && e.message ? String(e.message).substring(0, 90) : String(e))); }
        }
        var tty = require("tty");

        // ── isatty: non-int32 / negative / absent / high fd arms → false ──
        check("isatty_noargs_false", function() { return tty.isatty() === false; });
        check("isatty_string_false", function() { return tty.isatty("x") === false; });
        check("isatty_bool_false", function() { return tty.isatty(true) === false; });
        check("isatty_double_false", function() { return tty.isatty(1.5) === false; });
        check("isatty_neg_false", function() { return tty.isatty(-1) === false; });
        check("isatty_high_false", function() { return tty.isatty(999) === false; });
        check("isatty_std_bool", function() { return typeof tty.isatty(0) === "boolean" && typeof tty.isatty(1) === "boolean" && typeof tty.isatty(2) === "boolean"; });

        // ── ReadStream defaults (fd 0) + lifecycle faces ──
        var rs = new tty.ReadStream(0);
        check("rs_fd_default", function() { return rs.fd === 0; });
        check("rs_isRaw_false", function() { return rs.isRaw === false; });
        check("rs_readable_true", function() { return rs.readable === true; });
        check("rs_isTTY_bool", function() { return typeof rs.isTTY === "boolean"; });
        check("rs_resume_returns_this", function() { return rs.resume() === rs && rs.readable === true; });
        check("rs_pause_unreadable", function() { return rs.pause() === rs && rs.readable === false; });
        check("rs_destroy_flag", function() { return rs.destroy() === rs && rs.destroyed === true; });
        check("rs_ref_identity", function() { return rs.ref() === rs; });
        check("rs_unref_identity", function() { return rs.unref() === rs; });
        check("rs_on_emit_roundtrip", function() {
            var got = 0;
            rs.on("c5evt", function() { got += 1; });
            rs.emit("c5evt");
            return got === 1;
        });
        check("rs_nonint_fd_defaults_zero", function() { return new tty.ReadStream("x").fd === 0; });
        check("rs_noargs_fd_zero", function() { return new tty.ReadStream().fd === 0; });

        // ── setRawMode refused arm (invalid fd — platform-independent guard) ──
        check("rs_rawmode_invalid_fd_throws", function() {
            try { new tty.ReadStream(-1).setRawMode(true); return "NO-THROW"; }
            catch (e) { return String(e.message).indexOf("setRawMode: invalid fd") === 0; }
        });

        // ── WriteStream defaults (fd 1) + non-tty size absence ──
        var ws = new tty.WriteStream(1);
        check("ws_fd_default", function() { return ws.fd === 1; });
        check("ws_isTTY_bool", function() { return typeof ws.isTTY === "boolean"; });
        check("ws_nonint_fd_defaults_one", function() { return new tty.WriteStream("x").fd === 1; });
        check("ws_noargs_fd_one", function() { return new tty.WriteStream().fd === 1; });
        check("ws_size_type_or_absent", function() {
            return (typeof ws.columns === "number" || ws.columns === undefined)
                && (typeof ws.rows === "number" || ws.rows === undefined);
        });
        check("ws_getWindowSize_type_or_absent", function() {
            return typeof ws.getWindowSize === "function" || ws.getWindowSize === undefined;
        });

        // ── module shape ──
        check("mod_keys", function() {
            var keys = Object.getOwnPropertyNames(tty);
            return keys.indexOf("isatty") >= 0 && keys.indexOf("ReadStream") >= 0 && keys.indexOf("WriteStream") >= 0;
        });
        check("mod_ReadStream_ctor", function() { return typeof tty.ReadStream === "function"; });
        check("mod_WriteStream_ctor", function() { return typeof tty.WriteStream === "function"; });

        results.join("|")
    "#,
    28,
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// §8 tty — WriteStream on a REAL file fd: every ANSI escape, byte-exact
// ═══════════════════════════════════════════════════════════════════════════

#[test]
#[cfg(unix)] // raw fd + libc::write face — windows CRT-fd interop is a separate wave
fn c5_tty_write_stream_file_escape_bytes() {
    crate::exit_isolation::dispatch_timeout(
        "cp_path_tty_coverage_c5_tests::c5_tty_write_stream_file_escape_bytes",
        c5_tty_write_stream_file_escape_bytes_body,
    );
}

#[cfg(unix)]
fn c5_tty_write_stream_file_escape_bytes_body() {
    let mut ctx = setup_ctx();

    let path = std::env::temp_dir().join(format!("bao_c5_tty_escape_{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let c_path = std::ffi::CString::new(path.to_string_lossy().as_bytes()).expect("temp path");
    let fd = unsafe {
        libc::open(
            c_path.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC,
            0o644,
        )
    };
    assert!(fd >= 0, "open temp file for tty WriteStream: {}", path.display());

    let js = format!(
        r#"
        var tty = require("tty");
        var ws = new tty.WriteStream({fd});
        var out = [];
        out.push("istty=" + ws.isTTY);
        out.push("cols-undef=" + (ws.columns === undefined));
        out.push("rows-undef=" + (ws.rows === undefined));
        out.push("getsize-undef=" + (ws.getWindowSize === undefined));
        ws.write("AB");
        ws.clearLine();
        ws.clearLine(-1);
        ws.clearLine(1);
        ws.clearScreenDown();
        ws.cursorTo(3, 1);
        ws.cursorTo(-1, -2);
        ws.moveCursor(2, 0);
        ws.moveCursor(0, -1);
        ws.moveCursor(0, 1);
        ws.moveCursor(-3, 0);
        ws.moveCursor(0, 0);
        ws.moveCursor();
        out.push("write-nonstring=" + ws.write(42));
        out.push("write-nonstr-obj=" + ws.write({{}}));
        out.join("|");
    "#,
        fd = fd,
    );
    let got = eval_str(&mut ctx, &js);
    unsafe {
        libc::close(fd);
    }

    let expected = format!(
        "istty=false|cols-undef=true|rows-undef=true|getsize-undef=true|write-nonstring=true|write-nonstr-obj=true"
    );
    assert_eq!(got, expected, "WriteStream file-fd face report");

    let bytes = std::fs::read(&path).expect("read escape capture file");
    // clearLine() → 2K; (-1) → 1K; (1) → 0K; clearScreenDown → J;
    // cursorTo(3,1) → 2;4H; cursorTo(-1,-2) → 1;1H; moveCursor: 2C, 1B, 1A,
    // 3D; (0,0) and no-arg emit nothing (empty-escape arm).
    let want: &[u8] = b"AB\x1b[2K\x1b[1K\x1b[0K\x1b[J\x1b[2;4H\x1b[1;1H\x1b[2C\x1b[1B\x1b[1A\x1b[3D";
    assert_eq!(
        bytes, want,
        "byte-exact ANSI escapes (got {:?})",
        String::from_utf8_lossy(&bytes)
    );

    let _ = std::fs::remove_file(&path);
}

// ═══════════════════════════════════════════════════════════════════════════
// §9 tty — real pty (unix openpty): isTTY / columns / getWindowSize /
//          setRawMode success arms, plus the master→slave transport
// ═══════════════════════════════════════════════════════════════════════════

#[test]
#[cfg(unix)]
fn c5_tty_pty_real_face() {
    crate::exit_isolation::dispatch_timeout(
        "cp_path_tty_coverage_c5_tests::c5_tty_pty_real_face",
        c5_tty_pty_real_face_body,
    );
}

#[cfg(unix)]
fn c5_tty_pty_real_face_body() {
    use std::ptr;

    let mut master: libc::c_int = -1;
    let mut slave: libc::c_int = -1;
    let rc = unsafe { libc::openpty(&mut master, &mut slave, ptr::null_mut(), ptr::null(), ptr::null()) };
    assert_eq!(
        rc, 0,
        "openpty failed — canonical test env must provide /dev/ptmx (errno {})",
        std::io::Error::last_os_error()
    );
    let ws = libc::winsize {
        ws_row: 40,
        ws_col: 120,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    assert_eq!(
        unsafe { libc::ioctl(slave, libc::TIOCSWINSZ, &ws) },
        0,
        "TIOCSWINSZ on pty slave"
    );

    let mut ctx = setup_ctx();

    assert_labels(
        &mut ctx,
        &format!(
            r#"
        {CHECK_PREAMBLE}
        var tty = require("tty");
        var S = {slave};
        var M = {master};

        // ── isatty truth arms on both pty ends ──
        check("pty_isatty_slave", function() {{ return tty.isatty(S) === true; }});
        check("pty_isatty_master", function() {{ return tty.isatty(M) === true; }});
        check("pty_isatty_neg_still_false", function() {{ return tty.isatty(-5) === false; }});

        // ── ReadStream on the slave: real isTTY + raw-mode success arms ──
        var rs = new tty.ReadStream(S);
        check("pty_rs_isTTY_true", function() {{ return rs.isTTY === true; }});
        check("pty_rs_fd", function() {{ return rs.fd === S; }});
        check("pty_rs_raw_on", function() {{ return rs.setRawMode(true) === rs && rs.isRaw === true; }});
        check("pty_rs_raw_off", function() {{ return rs.setRawMode(false) === rs && rs.isRaw === false; }});

        // ── WriteStream on the slave: winsize honored ──
        var ws = new tty.WriteStream(S);
        check("pty_ws_isTTY_true", function() {{ return ws.isTTY === true; }});
        check("pty_ws_columns", function() {{ return ws.columns === 120; }});
        check("pty_ws_rows", function() {{ return ws.rows === 40; }});
        check("pty_ws_getWindowSize", function() {{
            var sz = ws.getWindowSize();
            return Array.isArray(sz) && sz.length === 2 && sz[0] === 120 && sz[1] === 40;
        }});

        // ── transport: WriteStream on the MASTER lands bytes on the slave ──
        check("pty_master_write", function() {{ return new tty.WriteStream(M).write("Z") === true; }});

        results.join("|")
    "#,
            slave = slave,
            master = master,
        ),
        11,
    );

    // Read the transported byte back from the slave end (already buffered —
    // a single read returns immediately).
    let mut buf = [0u8; 1];
    let n = unsafe { libc::read(slave, buf.as_mut_ptr() as *mut libc::c_void, 1) };
    assert_eq!(n, 1, "pty master→slave transport byte");
    assert_eq!(buf[0], b'Z', "transported byte content");

    unsafe {
        libc::close(slave);
        libc::close(master);
    }
}
