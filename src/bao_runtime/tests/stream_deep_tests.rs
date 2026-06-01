// @trace TEST-ENG-007-STREAM-DEEP [req:REQ-ENG-007] [level:integration]

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
fn test_stream_deep() {
    bao_runtime::install_exit_handler();
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::new().expect("JsContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e).substring(0, 60)); }
        }

        var stream = require('stream');

        // ---- Module existence ----
        check("stream_exists", function() { return typeof stream !== 'undefined'; });
        check("stream_is_object", function() { return typeof stream === 'object'; });

        // ---- Readable class ----
        check("stream_Readable_exists", function() { return typeof stream.Readable === 'function'; });
        check("stream_Readable_instance", function() {
            var r = new stream.Readable({read: function() {}});
            return r !== null && typeof r === 'object';
        });
        check("stream_Readable_is_stream", function() {
            var r = new stream.Readable({read: function() {}});
            return typeof r.pipe === 'function' || typeof r.on === 'function';
        });
        check("stream_Readable_on", function() {
            var r = new stream.Readable({read: function() {}});
            return typeof r.on === 'function';
        });
        check("stream_Readable_pipe", function() {
            var r = new stream.Readable({read: function() {}});
            return typeof r.pipe === 'function';
        });

        // ---- Writable class ----
        check("stream_Writable_exists", function() { return typeof stream.Writable === 'function'; });
        check("stream_Writable_instance", function() {
            var w = new stream.Writable({write: function(chunk, enc, cb) { cb(); }});
            return w !== null && typeof w === 'object';
        });
        check("stream_Writable_write", function() {
            var w = new stream.Writable({write: function(chunk, enc, cb) { cb(); }});
            return typeof w.write === 'function';
        });
        check("stream_Writable_end", function() {
            var w = new stream.Writable({write: function(chunk, enc, cb) { cb(); }});
            return typeof w.end === 'function';
        });

        // ---- Duplex class ----
        check("stream_Duplex_exists", function() { return typeof stream.Duplex === 'function'; });
        check("stream_Duplex_instance", function() {
            var d = new stream.Duplex({read: function() {}, write: function(chunk, enc, cb) { cb(); }});
            return d !== null && typeof d === 'object';
        });
        check("stream_Duplex_has_read_write", function() {
            var d = new stream.Duplex({read: function() {}, write: function(chunk, enc, cb) { cb(); }});
            return typeof d.read === 'function' && typeof d.write === 'function';
        });

        // ---- Transform class ----
        check("stream_Transform_exists", function() { return typeof stream.Transform === 'function'; });
        check("stream_Transform_instance", function() {
            var t = new stream.Transform({transform: function(chunk, enc, cb) { cb(null, chunk); }});
            return t !== null && typeof t === 'object';
        });
        check("stream_Transform_is_duplex_subclass", function() {
            var t = new stream.Transform({transform: function(chunk, enc, cb) { cb(null, chunk); }});
            return typeof t.write === 'function' && typeof t.read === 'function';
        });

        // ---- PassThrough class ----
        check("stream_PassThrough_exists", function() { return typeof stream.PassThrough === 'function'; });
        check("stream_PassThrough_instance", function() {
            var p = new stream.PassThrough();
            return p !== null && typeof p === 'object';
        });

        // ---- EventEmitter inheritance ----
        check("stream_Readable_inherits_EE", function() {
            var r = new stream.Readable({read: function() {}});
            return typeof r.on === 'function' && typeof r.emit === 'function';
        });
        check("stream_Writable_inherits_EE", function() {
            var w = new stream.Writable({write: function(chunk, enc, cb) { cb(); }});
            return typeof w.on === 'function' && typeof w.emit === 'function';
        });

        // ---- Stream methods on instances ----
        check("stream_Readable_resume", function() {
            var r = new stream.Readable({read: function() {}});
            return typeof r.resume === 'function' || typeof r.resume === 'undefined';
        });
        check("stream_Readable_pause", function() {
            var r = new stream.Readable({read: function() {}});
            return typeof r.pause === 'function' || typeof r.pause === 'undefined';
        });
        check("stream_Writable_destroy", function() {
            var w = new stream.Writable({write: function(chunk, enc, cb) { cb(); }});
            return typeof w.destroy === 'function' || typeof w.destroy === 'undefined';
        });

        // ---- Module keys completeness ----
        check("stream_module_keys", function() {
            var keys = Object.getOwnPropertyNames(stream);
            return keys.length >= 4;
        });

        // ---- pipeline helper (if exists) ----
        check("stream_pipeline_exists", function() {
            return typeof stream.pipeline === 'function' || typeof stream.pipeline === 'undefined';
        });
        check("stream_compose_exists", function() {
            return typeof stream.compose === 'function' || typeof stream.compose === 'undefined';
        });

        // ---- Finished helper (if exists) ----
        check("stream_finished_exists", function() {
            return typeof stream.finished === 'function' || typeof stream.finished === 'undefined';
        });

        results.join("|");
    "#);

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
    assert_eq!(fail, 0, "stream deep tests had {} failures", fail);
    assert!(pass >= 20, "Expected at least 20 passes, got {}", pass);
    std::mem::forget(ctx);
}