// @trace TEST-ENG-007-PATH-DEEP [req:REQ-ENG-007] [level:integration]

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
fn test_path_deep() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e).substring(0, 60)); }
        }

        var path = require('path');

        // === path module shape ===
        check("path_is_object", function() { return typeof path === 'object' && path !== null; });

        // === path.join ===
        check("join_exists", function() { return typeof path.join === 'function'; });
        check("join_basic", function() { return path.join('/foo', 'bar', 'baz') === '/foo/bar/baz'; });
        check("join_relative", function() { return path.join('foo', 'bar') === 'foo/bar'; });

        // === path.resolve ===
        check("resolve_exists", function() { return typeof path.resolve === 'function'; });
        check("resolve_absolute", function() { return path.resolve('/foo', '/bar') === '/bar'; });

        // === path.basename ===
        check("basename_exists", function() { return typeof path.basename === 'function'; });
        check("basename_basic", function() { return path.basename('/foo/bar/baz.txt') === 'baz.txt'; });
        check("basename_with_ext", function() { return path.basename('/foo/bar/baz.txt', '.txt') === 'baz'; });

        // === path.dirname ===
        check("dirname_exists", function() { return typeof path.dirname === 'function'; });
        check("dirname_basic", function() { return path.dirname('/foo/bar/baz') === '/foo/bar'; });

        // === path.extname ===
        check("extname_exists", function() { return typeof path.extname === 'function'; });
        check("extname_basic", function() { return path.extname('file.txt') === '.txt'; });
        check("extname_no_ext", function() { return path.extname('file') === ''; });
        check("extname_multi", function() { return path.extname('file.tar.gz') === '.gz'; });

        // === path.normalize ===
        check("normalize_exists", function() { return typeof path.normalize === 'function'; });
        check("normalize_dots", function() { return path.normalize('/foo/bar/../baz') === '/foo/baz'; });
        check("normalize_double_slash", function() { return path.normalize('/foo//bar') === '/foo/bar'; });

        // === path.isAbsolute ===
        check("isAbsolute_exists", function() { return typeof path.isAbsolute === 'function'; });
        check("isAbsolute_true", function() { return path.isAbsolute('/foo') === true; });
        check("isAbsolute_false", function() { return path.isAbsolute('foo/bar') === false; });

        // === path.relative ===
        check("relative_exists", function() { return typeof path.relative === 'function'; });
        check("relative_basic", function() { return typeof path.relative('/foo/bar', '/foo/baz') === 'string'; });

        // === path.parse ===
        check("parse_exists", function() { return typeof path.parse === 'function'; });
        check("parse_basic", function() {
            var p = path.parse('/foo/bar/baz.txt');
            return p.root === '/' && p.dir === '/foo/bar' && p.base === 'baz.txt' && p.ext === '.txt' && p.name === 'baz';
        });

        // === path.format ===
        check("format_exists", function() { return typeof path.format === 'function'; });
        check("format_basic", function() { return path.format({dir: '/foo', base: 'bar.txt'}) === '/foo/bar.txt'; });

        // === path properties ===
        check("sep_exists", function() { return typeof path.sep === 'string'; });
        check("delimiter_exists", function() { return typeof path.delimiter === 'string'; });
        check("posix_exists", function() { return typeof path.posix === 'object' && path.posix !== null; });
        check("win32_exists", function() { return typeof path.win32 === 'object' && path.win32 !== null; });

        results.join("|")
    "#);

    let mut all_passed = true;
    for item in results.split('|') {
        if !item.contains(" PASS") {
            eprintln!("  FAIL: {}", item);
            all_passed = false;
        }
    }
    assert!(all_passed, "All path deep tests should pass. Results: {}", results);

    bun_runtime::shutdown_thread_sm();
}
