// @trace TEST-ENG-007-PATH-C13 [req:REQ-ENG-007] [level:integration]
//
// C13 line-coverage wave for src/bao_runtime/src/node_path.rs — the residual
// gap after node_path_tests / path_deep_tests / C5 (error arms + basic
// matchesGlob): the win32 face success-literal matrix (normalize / join /
// dirname / basename / extname / isAbsolute / parse / format / resolve /
// relative / toNamespacedPath — every literal cross-checked against the
// node v24.19.0 oracle on this host), the face-identity anchors
// (path === path.posix / win32.posix alias / win32.win32 self-ref), the
// node_alg B-class regression pins (surviving trailing separator, '..' above
// a relative root, dot-basename extname/parse, format name+ext), the
// matchesGlob negation / unterminated-class / regex-metachar arms, and the
// cwd-bound resolve/relative shapes.
// All offline — pure string surface, no sockets, cwd-shape assertions only.
//
// 语义真源 = 本机 node v24.19.0 oracle 实测。C13 登记的 2 项 matchesGlob
// parity 偏离已于 D 波根治(按 node internal/fs/glob → minimatch
// platform:'posix' / dot:false 的 path-parts 语义重写),以下断言即 oracle:
//
// | # | 面 | 现行为(本文件钉死) | node v24 oracle |
// |---|----|----------------------|-----------------|
// | 1 | matchesGlob single-`*` 段语义 | 段内匹配:matchesGlob('src/a/b.js','*/b.js')===false,matchesGlob('a/b.js','*/b.js')===true | 同 |
// | 2 | matchesGlob 空串 | matchesGlob('', '*') === false | 同 |

use bao_engine::context::JsContext;
#[path = "common/mod.rs"]
mod common;

use common::eval_string;

fn fresh_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext init");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

/// Run a `check(label, fn)` battery in JS and assert every item PASSes.
fn assert_battery(ctx: &mut JsContext, battery: &str) {
    let results = eval_string(ctx, battery);
    let mut failures = Vec::new();
    for item in results.split('|') {
        if item.is_empty() {
            continue;
        }
        if !item.contains(" PASS") {
            failures.push(item.to_string());
        }
    }
    assert!(
        failures.is_empty(),
        "path c13 battery failures: {:?}\nfull: {}",
        failures,
        results
    );
}

/// Face-identity anchors: on a posix host `path` IS `path.posix` (by
/// reference), `path.win32.posix` aliases that same face, `path.win32.win32`
/// is a self-reference, and the per-face sep/delimiter/platform properties
/// carry the platform literals.
#[test]
fn pathc13_identity_anchors_and_face_props() {
    let mut ctx = fresh_ctx();
    assert_battery(
        &mut ctx,
        r#"
        var p = require('path');
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e)); }
        }

        // posix host: the host face is the posix face (node shape).
        check("host_is_posix_identity", function() { return p === p.posix; });
        check("win32_posix_alias", function() { return p.win32.posix === p.posix; });
        check("win32_self_reference", function() { return p.win32.win32 === p.win32; });

        // posix face carries its own sep/delimiter regardless of host.
        check("posix_sep_slash", function() { return p.posix.sep === '/'; });
        check("posix_delimiter_colon", function() { return p.posix.delimiter === ':'; });

        // win32 face literals on every platform.
        check("win32_sep_backslash", function() { return p.win32.sep === '\\'; });
        check("win32_delimiter_semicolon", function() { return p.win32.delimiter === ';'; });

        // host platform tag mirrors process.platform; host sep/delimiter
        // follow the host.
        check("platform_tag", function() { return p.platform === process.platform; });
        check("host_sep_follows_platform", function() {
            return p.sep === (process.platform === 'win32' ? '\\' : '/');
        });
        check("host_delimiter_follows_platform", function() {
            return p.delimiter === (process.platform === 'win32' ? ';' : ':');
        });

        // The win32 face carries the full method surface (11 fns).
        check("win32_method_surface", function() {
            var fns = ['join', 'normalize', 'dirname', 'basename', 'extname', 'isAbsolute',
                       'parse', 'format', 'resolve', 'relative', 'toNamespacedPath'];
            for (var i = 0; i < fns.length; i++) {
                if (typeof p.win32[fns[i]] !== 'function') return false;
            }
            return true;
        });

        results.join('|')
        "#,
    );
}

/// The win32 face success-literal matrix — every vector below was executed
/// against the node v24.19.0 oracle on this host and pinned byte-for-byte
/// (drive-relative vectors assert shape only: their expansion is
/// cwd-dependent by node semantics).
#[test]
fn pathc13_win32_success_literal_matrix() {
    let mut ctx = fresh_ctx();
    assert_battery(
        &mut ctx,
        r#"
        var p = require('path');
        var w = p.win32;
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e)); }
        }

        // normalize: forward slashes convert; mixed separators collapse;
        // '..' pops survive.
        check("wn_forward", function() { return w.normalize('a/b') === 'a\\b'; });
        check("wn_mixed", function() { return w.normalize('C:\\\\a////b') === 'C:\\a\\b'; });
        check("wn_reldots", function() { return w.normalize('foo\\bar\\..\\..\\baz') === 'baz'; });

        // basename / dirname / extname (win32 separator awareness).
        check("wb_plain", function() { return w.basename('C:\\dir\\f.txt') === 'f.txt'; });
        check("wb_ext_stripped", function() { return w.basename('C:\\dir\\f.txt', '.txt') === 'f'; });
        check("wb_trailing_sep", function() { return w.basename('C:\\dir\\') === 'dir'; });
        check("wd_plain", function() { return w.dirname('C:\\dir\\f.txt') === 'C:\\dir'; });
        check("wd_root_only", function() { return w.dirname('C:\\f') === 'C:\\'; });
        check("wd_relative", function() { return w.dirname('foo\\bar') === 'foo'; });
        check("wex", function() { return w.extname('C:\\a\\b.txt') === '.txt'; });

        // isAbsolute: drive-qualified / UNC / root-slash absolute.
        check("wisabs_drive", function() { return w.isAbsolute('C:\\a') === true; });
        check("wisabs_unc", function() { return w.isAbsolute('\\\\server') === true; });
        check("wisabs_relative", function() { return w.isAbsolute('a/b') === false; });
        check("wisabs_root_slash", function() { return w.isAbsolute('/') === true; });

        // parse: root/dir/base/ext/name quintuple (oracle literal).
        var parsed = w.parse('C:\\a\\b.txt');
        check("wparse_shape", function() {
            return parsed.root === 'C:\\' && parsed.dir === 'C:\\a' &&
                   parsed.base === 'b.txt' && parsed.ext === '.txt' && parsed.name === 'b';
        });

        // format round-trips through root+base and dir+name+ext.
        check("wformat_root_base", function() {
            return w.format({ root: 'C:\\', base: 'b.txt' }) === 'C:\\b.txt';
        });
        check("wformat_dir_name_ext", function() {
            return w.format({ dir: 'C:\\a', name: 'b', ext: '.txt' }) === 'C:\\a\\b.txt';
        });

        // join: drive root joining and '..' popping across mixed separators.
        check("wjoin_root", function() { return w.join('C:\\', 'a', 'b') === 'C:\\a\\b'; });
        check("wjoin_dotdot", function() { return w.join('C:\\a', '../b') === 'C:\\b'; });

        // resolve: absolute segment resets; a rooted segment resets the
        // directory part but keeps the drive.
        check("wresolve_appends", function() { return w.resolve('C:\\a\\b', 'c') === 'C:\\a\\b\\c'; });
        check("wresolve_rooted_reset", function() { return w.resolve('C:\\a\\b', '\\c') === 'C:\\c'; });
        // drive-relative target ('C:y') resolves against the C: drive cwd —
        // cwd-dependent by node semantics, so pin the SHAPE only.
        var drv = w.resolve('D:\\x', 'C:y');
        check("wresolve_drive_relative_shape", function() {
            return drv.indexOf('C:') === 0 && drv.lastIndexOf('\\y') === drv.length - 2;
        });

        // relative: same-drive delta is relative; cross-drive answers the
        // absolute target (oracle literal).
        check("wrelative_delta", function() { return w.relative('C:\\a\\b', 'C:\\a\\c') === '..\\c'; });
        check("wrelative_cross_drive", function() { return w.relative('C:\\a', 'D:\\b') === 'D:\\b'; });

        // toNamespacedPath: \\?\ prefix; already-namespaced input stays;
        // drive-relative is cwd-dependent — shape only.
        check("wtnp_prefix", function() { return w.toNamespacedPath('C:\\a\\b') === '\\\\?\\C:\\a\\b'; });
        check("wtnp_already", function() { return w.toNamespacedPath('\\\\?\\C:\\a') === '\\\\?\\C:\\a'; });
        var tnpRel = w.toNamespacedPath('a\\b');
        check("wtnp_relative_shape", function() {
            // cwd-dependent expansion: backslash-rooted, NO \\?\ prefix
            // (the expanded cwd is not drive-qualified on this host), and
            // the input's segments as the tail.
            return tnpRel.charAt(0) === '\\' &&
                   tnpRel.indexOf('\\\\?\\') !== 0 &&
                   tnpRel.lastIndexOf('\\a\\b') === tnpRel.length - 4;
        });

        results.join('|')
        "#,
    );
}

/// The node_alg B-class regression pins (the divergence class the node_alg
/// port fixed): surviving trailing separator in normalize, '..' above a
/// relative root in normalize and join, dot-only basenames in
/// extname/basename/parse, the format name+ext dot, and toNamespacedPath
/// passthrough on a posix host — all oracle-verified literals.
#[test]
fn pathc13_posix_core_regression_pins() {
    let mut ctx = fresh_ctx();
    assert_battery(
        &mut ctx,
        r#"
        var p = require('path');
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e)); }
        }

        // normalize: surviving trailing separator ('bar/foo../../' → 'bar/',
        // not 'bar'); '..' above a relative root survives; double slashes
        // collapse keeping one trailing separator.
        check("normalize_surviving_trailing_sep", function() {
            return p.normalize('bar/foo../../') === 'bar/';
        });
        check("normalize_dot_segments", function() { return p.normalize('./a/b/../c') === 'a/c'; });
        check("normalize_above_root_survives", function() { return p.normalize('a/../..') === '..'; });
        check("normalize_double_slash", function() { return p.normalize('a//b//') === 'a/b/'; });

        // join: leading '..' above a relative root is preserved ('..' not
        // '../..'), and mixed relative segments resolve in order.
        check("join_above_root", function() { return p.join('a', '..', '..') === '..'; });
        check("join_dots_mixed", function() { return p.join('/a', 'b/../c') === '/a/c'; });

        // basename: dot-only segments are basenames, never extensions.
        check("basename_dotdot", function() { return p.basename('..') === '..'; });
        check("basename_dotdot_seg", function() { return p.basename('a/..') === '..'; });

        // extname: dot-leading names carry no extension; '..' none; multi-dot
        // takes the last; a trailing dot is the extension.
        check("extname_dotfile_empty", function() { return p.extname('.bashrc') === ''; });
        check("extname_dotdot_empty", function() { return p.extname('..') === ''; });
        check("extname_multi", function() { return p.extname('a.tar.gz') === '.gz'; });
        check("extname_trailing_dot", function() { return p.extname('a.') === '.'; });

        // dirname: trailing separator is dropped; root stays root.
        check("dirname_trailing_sep", function() { return p.dirname('a/b/') === 'a'; });
        check("dirname_root", function() { return p.dirname('/') === '/'; });

        // parse: dot-leading basename carries base/name without ext; root
        // parse yields base:'' / name:''; plain file splits all fields.
        var pd = p.parse('.bashrc');
        check("parse_dotfile", function() {
            return pd.root === '' && pd.dir === '' && pd.base === '.bashrc' &&
                   pd.ext === '' && pd.name === '.bashrc';
        });
        var pr = p.parse('/');
        check("parse_root", function() {
            return pr.root === '/' && pr.dir === '/' && pr.base === '' &&
                   pr.ext === '' && pr.name === '';
        });
        var pp = p.parse('a/b/c.txt');
        check("parse_plain", function() {
            return pp.root === '' && pp.dir === 'a/b' && pp.base === 'c.txt' &&
                   pp.ext === '.txt' && pp.name === 'c';
        });

        // format: name+ext concatenates the dot; root+base ignores the root
        // label; dir+base joins; name-only omits the dot.
        check("format_name_ext", function() { return p.format({ name: 'x', ext: '.txt' }) === 'x.txt'; });
        check("format_root_base", function() { return p.format({ root: '/ignored', base: 'file' }) === '/ignoredfile'; });
        check("format_dir_base", function() { return p.format({ dir: '/a/b', base: 'c.txt' }) === '/a/b/c.txt'; });
        check("format_name_only", function() { return p.format({ name: 'x' }) === 'x'; });

        // toNamespacedPath on a posix host: identity passthrough.
        check("tnp_posix_passthrough", function() { return p.toNamespacedPath('/a/b') === '/a/b'; });
        check("tnp_posix_relative", function() { return p.toNamespacedPath('a/b') === 'a/b'; });

        // relative: identical inputs answer ''; a simple delta is '..'-based.
        check("relative_same_empty", function() { return p.relative('/a/b', '/a/b') === ''; });
        check("relative_basic", function() { return p.relative('/a/b', '/a/c') === '../c'; });

        results.join('|')
        "#,
    );
}

/// matchesGlob negation / unterminated-class / regex-metachar / empty arms,
/// plus the D-wave oracle flip for single-`*` semantics: the translator now
/// mirrors node v24's minimatch path-parts matching, so `*` stays within one
/// segment and the empty string never matches `*`.
#[test]
fn pathc13_matchesglob_negation_metachar_and_star_semantics() {
    let mut ctx = fresh_ctx();
    assert_battery(
        &mut ctx,
        r#"
        var p = require('path');
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e)); }
        }

        // Character-class negation ([!a] → regex [^a]).
        check("glob_neg_matches_other", function() { return p.matchesGlob('b.txt', '[!a].txt') === true; });
        check("glob_neg_rejects_self", function() { return p.matchesGlob('a.txt', '[!a].txt') === false; });

        // Unterminated class: falls back to a literal bracket (both the
        // pattern and the subject reach the regex as literals).
        check("glob_unterminated_class_literal", function() {
            return p.matchesGlob('a[', 'a[') === true;
        });

        // Regex metacharacters in the pattern are escaped: they match
        // literally, never as regex operators.
        check("glob_dot_literal", function() { return p.matchesGlob('a.b', 'a.b') === true; });
        check("glob_dot_not_wildcard", function() { return p.matchesGlob('aXb', 'a.b') === false; });
        check("glob_plus_literal", function() { return p.matchesGlob('a+b', 'a+b') === true; });

        // ? matches exactly one character.
        check("glob_qmark_single", function() { return p.matchesGlob('axc', 'a?c') === true; });
        check("glob_qmark_not_two", function() { return p.matchesGlob('axxc', 'a?c') === false; });

        // ** crosses segments (C5 pinned the true arm; the complement here).
        check("glob_dblstar_crosses_true", function() { return p.matchesGlob('src/a/b.js', '**/*.js') === true; });

        // Oracle flip #1: a single `*` never crosses '/'. The same pattern
        // does match when the subject has exactly one leading segment.
        check("glob_single_star_within_segment", function() {
            return p.matchesGlob('src/a/b.js', '*/b.js') === false;
        });
        check("glob_single_star_same_depth_true", function() {
            return p.matchesGlob('a/b.js', '*/b.js') === true;
        });
        // Oracle flip #2: `*` requires a non-empty segment, so '' is false.
        check("glob_empty_does_not_match_star", function() {
            return p.matchesGlob('', '*') === false;
        });
        // `**` (globstar) still spans segments and also matches the empty
        // path (minimatch: a trailing `**` swallows zero parts).
        check("glob_empty_matches_globstar", function() {
            return p.matchesGlob('', '**') === true;
        });

        results.join('|')
        "#,
    );
}

/// The cwd-bound resolve/relative shapes on the host face: bare resolve()
/// answers the cwd, resolve('') is the cwd, resolve of relative segments
/// joins onto the cwd, the host toNamespacedPath passthrough on posix, and
/// the win32 resolve drive-absolute reset.
#[test]
fn pathc13_resolve_relative_cwd_shapes() {
    let mut ctx = fresh_ctx();
    assert_battery(
        &mut ctx,
        r#"
        var p = require('path');
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e)); }
        }

        // Host resolve() / resolve('') answer the cwd (string, absolute).
        check("resolve_bare_is_cwd", function() {
            return p.resolve() === process.cwd();
        });
        check("resolve_empty_is_cwd", function() {
            return p.resolve('') === process.cwd();
        });

        // resolve of relative segments joins onto the cwd; an absolute
        // segment resets it.
        check("resolve_joins_cwd", function() {
            return p.resolve('a', 'b') === p.join(process.cwd(), 'a', 'b');
        });
        check("resolve_absolute_resets", function() {
            return p.resolve('/anywhere', 'x') === p.join('/anywhere', 'x');
        });

        // Host toNamespacedPath on a posix host is the identity.
        check("host_tnp_identity_posix", function() {
            return p.toNamespacedPath(p.resolve('a')) === p.resolve('a');
        });

        // win32 resolve: a drive-absolute path resets the directory but
        // keeps the drive (cwd-independent oracle literal).
        check("win32_resolve_drive_absolute", function() {
            return p.win32.resolve('C:\\a\\b', 'C:\\c') === 'C:\\c';
        });

        // relative from cwd onto itself is '' (shape only — cwd agnostic).
        check("relative_cwd_self_empty", function() {
            return p.relative(process.cwd(), process.cwd()) === '';
        });

        results.join('|')
        "#,
    );
}
