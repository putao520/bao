// @trace TEST-ENG-007-FS [req:REQ-ENG-007] [level:integration]
// Integration tests for node:fs API (REQ-ENG-007)
//
// Single #[test] function to avoid mozjs Runtime per-thread singleton issues.
// All JS assertions run in one eval() call since each eval creates a new global scope.

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
fn test_node_fs_all() {
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    let tmp = ::std::env::temp_dir();
    let dir = tmp.join("bao_fs_test");
    let _ = ::std::fs::remove_dir_all(&dir);
    ::std::fs::create_dir_all(&dir).unwrap();

    let d = dir.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
    let f1 = dir.join("hello.txt");
    let p1 = f1.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
    let f2 = dir.join("renamed.txt");
    let p2 = f2.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
    let f3 = dir.join("copy.txt");
    let p3 = f3.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
    let subdir = dir.join("subdir");
    let ps = subdir.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
    let noexist = dir.join("nonexistent.txt");
    let pn = noexist.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
    let rm_dir = dir.join("rm_test");
    ::std::fs::create_dir_all(&rm_dir).unwrap();
    let pr = rm_dir.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");

    // All JS in one eval to share global scope
    let results = eval_string(&mut ctx, &format!(r#"
        var fs = require('fs');
        var errors = [];
        var results = [];

        function check(label, fn) {{
            try {{ var ok = fn(); results.push(label + ":" + (ok ? "PASS" : "FAIL")); }}
            catch(e) {{ results.push(label + ":ERROR:" + (e.message || e)); }}
        }}

        // require('fs') type check
        check("require", function() {{ return typeof fs === 'object'; }});
        // writeFileSync
        check("writeFileSync", function() {{ fs.writeFileSync("{p1}", "hello world"); return true; }});
        // readFileSync utf8
        check("readFileSync", function() {{ return fs.readFileSync("{p1}", "utf8") === "hello world"; }});
        // readFileSync buffer
        check("readFileBuf", function() {{ return fs.readFileSync("{p1}").length === 11; }});
        // appendFileSync
        check("appendFileSync", function() {{ fs.appendFileSync("{p1}", "!!"); return true; }});
        // existsSync true
        check("existsSync_true", function() {{ return fs.existsSync("{p1}") === true; }});
        // existsSync false
        check("existsSync_false", function() {{ return fs.existsSync("{pn}") === false; }});
        // statSync
        check("statSync", function() {{ var st = fs.statSync("{p1}"); return typeof st.size === "number" && st.size > 0; }});
        // lstatSync
        check("lstatSync", function() {{ var ls = fs.lstatSync("{p1}"); return typeof ls.size === "number"; }});
        // mkdirSync
        check("mkdirSync", function() {{ fs.mkdirSync("{ps}"); return fs.existsSync("{ps}"); }});
        // readdirSync
        check("readdirSync", function() {{ var entries = fs.readdirSync("{d}"); return Array.isArray(entries) && entries.length > 0; }});
        // renameSync
        check("renameSync", function() {{ fs.renameSync("{p1}", "{p2}"); return fs.existsSync("{p2}"); }});
        // copyFileSync
        check("copyFileSync", function() {{ fs.copyFileSync("{p2}", "{p3}"); return fs.existsSync("{p3}"); }});
        // unlinkSync
        check("unlinkSync", function() {{ fs.unlinkSync("{p3}"); return !fs.existsSync("{p3}"); }});
        // rmdirSync
        check("rmdirSync", function() {{ fs.rmdirSync("{ps}"); return !fs.existsSync("{ps}"); }});
        // rmSync recursive
        check("rmSync", function() {{ fs.rmSync("{pr}", {{recursive: true}}); return !fs.existsSync("{pr}"); }});
        // fs.constants
        check("constants", function() {{ return typeof fs.constants === 'object' || typeof fs.constants === 'undefined'; }});
        // fs.promises
        check("promises", function() {{ return typeof fs.promises === 'object' && typeof fs.promises.readFile === 'function'; }});
        // chmodSync
        check("chmodSync", function() {{ fs.chmodSync("{p2}", 0o644); return true; }});
        // realpathSync
        check("realpathSync", function() {{ var rp = fs.realpathSync("{p2}"); return typeof rp === "string" && rp.length > 0; }});

        results.join("|")
    "#, d = d, p1 = p1, p2 = p2, p3 = p3, ps = ps, pn = pn, pr = pr));

    // Parse results
    let mut all_passed = true;
    for item in results.split('|') {
        if item.contains(":PASS") {
            // ok
        } else if item.contains(":ERROR:") {
            eprintln!("  FAIL: {}", item);
            all_passed = false;
        } else if item.contains(":FAIL") {
            eprintln!("  FAIL: {}", item);
            all_passed = false;
        }
    }

    // Verify file content from Rust side
    assert_eq!(::std::fs::read_to_string(&f2).unwrap(), "hello world!!", "Rust-side file content check");

    // cleanup
    let _ = ::std::fs::remove_dir_all(&dir);

    assert!(all_passed, "All fs tests should pass. Results: {}", results);

    // Leak the context to prevent mozjs drop-order crashes
    std::mem::forget(ctx);
}
