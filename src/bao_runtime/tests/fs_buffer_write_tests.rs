// @trace TEST-ENG-007-FS-BUFFER [req:REQ-ENG-007] [level:integration]
// Regression test for BUG-351: fs.writeFileSync/appendFileSync silently drop Buffer data.
// Before fix: Buffer/TypedArray data wrote 0 bytes (silent data loss).
// After fix: Buffer bytes are extracted and written correctly.

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
fn test_fs_write_buffer_bytes_preserved() {
    bao_runtime::install_exit_handler();
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    let tmp = ::std::env::temp_dir();
    let dir = tmp.join("bao_fs_buffer_write_test");
    let _ = ::std::fs::remove_dir_all(&dir);
    ::std::fs::create_dir_all(&dir).unwrap();

    let f1 = dir.join("buffer_write.bin");
    let p1 = f1.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
    let f2 = dir.join("buffer_append.bin");
    let p2 = f2.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");

    let results = eval_string(&mut ctx, &format!(r#"
        var results = [];
        function check(label, fn) {{
            try {{ var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }}
            catch(e) {{ results.push(label + " ERR:" + (e.message || e).substring(0, 80)); }}
        }}

        var fs = require('fs');

        // Scenario 1: writeFileSync with Buffer.from(string)
        check("writeFileSync_buffer_from_string", function() {{
            var buf = Buffer.from("hello");
            fs.writeFileSync("{p1}", buf);
            var read = fs.readFileSync("{p1}", "utf8");
            return read === "hello";
        }});

        // Scenario 2: writeFileSync with Buffer.from(hex)
        check("writeFileSync_buffer_from_hex", function() {{
            var buf = Buffer.from("deadbeef", "hex");
            fs.writeFileSync("{p1}", buf);
            // Should write 4 bytes: 0xde, 0xad, 0xbe, 0xef
            var read = fs.readFileSync("{p1}", "hex");
            return read === "deadbeef";
        }});

        // Scenario 3: writeFileSync with Buffer of binary data (non-utf8)
        check("writeFileSync_buffer_binary", function() {{
            var buf = Buffer.from([0, 1, 2, 3, 255, 254]);
            fs.writeFileSync("{p1}", buf);
            var read = fs.readFileSync("{p1}", "hex");
            return read === "00010203fffe";
        }});

        // Scenario 4: appendFileSync with Buffer
        check("appendFileSync_buffer", function() {{
            fs.writeFileSync("{p2}", Buffer.from("hello"));
            fs.appendFileSync("{p2}", Buffer.from(" world"));
            var read = fs.readFileSync("{p2}", "utf8");
            return read === "hello world";
        }});

        // Scenario 5: appendFileSync with mixed string + Buffer
        check("appendFileSync_mixed", function() {{
            fs.writeFileSync("{p2}", "header:");
            fs.appendFileSync("{p2}", Buffer.from("body"));
            var read = fs.readFileSync("{p2}", "utf8");
            return read === "header:body";
        }});

        // Scenario 6: writeFileSync with empty Buffer
        check("writeFileSync_empty_buffer", function() {{
            var buf = Buffer.from([]);
            fs.writeFileSync("{p1}", buf);
            var stats = fs.statSync("{p1}");
            return stats.size === 0;
        }});

        // Scenario 7: writeFileSync Buffer byte length matches content
        check("writeFileSync_byte_length_preserved", function() {{
            var buf = Buffer.from("deadbeef", "hex");
            fs.writeFileSync("{p1}", buf);
            var stats = fs.statSync("{p1}");
            return stats.size === 4;
        }});

        results.join("|")
    "#, p1=p1, p2=p2));

    let mut pass = 0;
    let mut fail = 0;
    for item in results.split('|') {
        if item.contains(" PASS") {
            pass += 1;
        } else if item.contains(" FAIL") || item.contains(" ERR") {
            fail += 1;
            eprintln!("FAILED: {}", item);
        }
    }
    assert_eq!(fail, 0, "fs buffer write tests had {} failures", fail);
    assert!(pass >= 7, "Expected 7 passes, got {}", pass);

    let _ = ::std::fs::remove_dir_all(&dir);
    std::mem::forget(ctx);
}
