// @trace TEST-ENG-007-PATH [req:REQ-ENG-007] [level:integration]
// Integration tests for node:path API (REQ-ENG-007)
// All JS assertions in one eval() call.

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
fn test_node_path_all() {
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::new().expect("Failed to create JSContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var path = require('path');
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + ":" + (ok ? "PASS" : "FAIL")); }
            catch(e) { results.push(label + ":ERROR:" + (e.message || e)); }
        }

        check("require", function() { return typeof path === 'object'; });
        check("join", function() { return path.join("a", "b", "c") === "a/b/c"; });
        check("join_abs", function() { return path.join("/foo", "bar", "baz") === "/foo/bar/baz"; });
        check("resolve", function() { var r = path.resolve("/foo/bar", "./baz"); return typeof r === "string" && r.indexOf("baz") >= 0; });
        check("basename", function() { return path.basename("/foo/bar/baz.txt") === "baz.txt"; });
        check("basename_ext", function() { return path.basename("/foo/bar/baz.txt", ".txt") === "baz"; });
        check("dirname", function() { return path.dirname("/foo/bar/baz.txt") === "/foo/bar"; });
        check("extname_txt", function() { return path.extname("file.txt") === ".txt"; });
        check("extname_gz", function() { return path.extname("file.tar.gz") === ".gz"; });
        check("extname_empty", function() { return path.extname("noext") === ""; });
        check("sep", function() { return typeof path.sep === "string" && path.sep.length > 0; });
        check("delimiter", function() { return typeof path.delimiter === "string"; });
        check("isAbsolute_true", function() { return path.isAbsolute("/foo") === true; });
        check("isAbsolute_false", function() { return path.isAbsolute("foo/bar") === false; });
        check("normalize", function() { var n = path.normalize("/foo/bar/../baz"); return n.indexOf("baz") >= 0 && n.indexOf("..") < 0; });
        check("relative", function() { var r = path.relative("/foo/bar", "/foo/baz"); return typeof r === "string" && r.length > 0; });
        check("parse", function() {
            var p = path.parse("/foo/bar/baz.txt");
            return typeof p.root === "string" && p.base === "baz.txt" && p.ext === ".txt" && p.name === "baz";
        });
        check("format", function() {
            var s = path.format({dir: "/foo", base: "bar.txt"});
            return typeof s === "string" && s.length > 0;
        });

        results.join("|")
    "#);

    let mut all_passed = true;
    for item in results.split('|') {
        if !item.contains(":PASS") {
            eprintln!("  FAIL: {}", item);
            all_passed = false;
        }
    }
    assert!(all_passed, "All path tests should pass. Results: {}", results);
    std::mem::forget(ctx);
}
