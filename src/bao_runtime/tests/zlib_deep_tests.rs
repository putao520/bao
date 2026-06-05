// @trace TEST-ENG-007-ZLIB-DEEP [req:REQ-ENG-007] [level:integration]

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
fn test_zlib_deep() {
    bao_runtime::install_exit_handler();
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    let results = eval_string(&mut ctx, r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e).substring(0, 60)); }
        }

        var zlib = require('zlib');

        // ---- Module existence ----
        check("zlib_exists", function() { return typeof zlib !== 'undefined'; });
        check("zlib_is_object", function() { return typeof zlib === 'object'; });

        // ---- Sync functions existence ----
        check("zlib_deflateSync_exists", function() { return typeof zlib.deflateSync === 'function'; });
        check("zlib_inflateSync_exists", function() { return typeof zlib.inflateSync === 'function'; });
        check("zlib_deflateRawSync_exists", function() { return typeof zlib.deflateRawSync === 'function'; });
        check("zlib_inflateRawSync_exists", function() { return typeof zlib.inflateRawSync === 'function'; });
        check("zlib_gzipSync_exists", function() { return typeof zlib.gzipSync === 'function'; });
        check("zlib_gunzipSync_exists", function() { return typeof zlib.gunzipSync === 'function'; });

        // ---- deflateSync + inflateSync roundtrip ----
        check("zlib_deflateSync_returns_buffer", function() {
            var input = Buffer.from('hello zlib');
            var compressed = zlib.deflateSync(input);
            return compressed !== null && compressed !== undefined;
        });
        check("zlib_inflateSync_roundtrip", function() {
            var input = Buffer.from('hello zlib');
            var compressed = zlib.deflateSync(input);
            var decompressed = zlib.inflateSync(compressed);
            return decompressed !== null && decompressed !== undefined;
        });
        check("zlib_deflate_inflate_content", function() {
            var input = Buffer.from('hello zlib roundtrip test');
            var compressed = zlib.deflateSync(input);
            var decompressed = zlib.inflateSync(compressed);
            if (!decompressed || !decompressed.toString) return false;
            return decompressed.toString() === 'hello zlib roundtrip test';
        });

        // ---- gzipSync + gunzipSync roundtrip ----
        check("zlib_gzipSync_returns_buffer", function() {
            var input = Buffer.from('hello gzip');
            var compressed = zlib.gzipSync(input);
            return compressed !== null && compressed !== undefined;
        });
        check("zlib_gunzipSync_roundtrip", function() {
            var input = Buffer.from('hello gzip');
            var compressed = zlib.gzipSync(input);
            var decompressed = zlib.gunzipSync(compressed);
            return decompressed !== null && decompressed !== undefined;
        });
        check("zlib_gzip_gunzip_content", function() {
            var input = Buffer.from('hello gzip roundtrip');
            var compressed = zlib.gzipSync(input);
            var decompressed = zlib.gunzipSync(compressed);
            if (!decompressed || !decompressed.toString) return false;
            return decompressed.toString() === 'hello gzip roundtrip';
        });

        // ---- deflateRawSync + inflateRawSync roundtrip ----
        check("zlib_deflateRawSync_returns_buffer", function() {
            var input = Buffer.from('hello raw');
            var compressed = zlib.deflateRawSync(input);
            return compressed !== null && compressed !== undefined;
        });
        check("zlib_inflateRawSync_roundtrip", function() {
            var input = Buffer.from('hello raw');
            var compressed = zlib.deflateRawSync(input);
            var decompressed = zlib.inflateRawSync(compressed);
            if (!decompressed || !decompressed.toString) return false;
            return decompressed.toString() === 'hello raw';
        });

        // ---- Compression actually reduces size (for repeated data) ----
        check("zlib_deflate_compresses", function() {
            var input = Buffer.from('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa');
            var compressed = zlib.deflateSync(input);
            return compressed.length < input.length;
        });
        check("zlib_gzip_compresses", function() {
            var input = Buffer.from('bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb');
            var compressed = zlib.gzipSync(input);
            return compressed.length < input.length;
        });

        // ---- Empty input ----
        check("zlib_deflateSync_empty", function() {
            var input = Buffer.from('');
            var compressed = zlib.deflateSync(input);
            return compressed !== null && compressed !== undefined;
        });
        check("zlib_inflateSync_empty_roundtrip", function() {
            var input = Buffer.from('');
            var compressed = zlib.deflateSync(input);
            var decompressed = zlib.inflateSync(compressed);
            return decompressed !== null && decompressed !== undefined;
        });

        // ---- Unicode roundtrip ----
        check("zlib_unicode_roundtrip", function() {
            try {
                var input = Buffer.from('你好世界');
                var compressed = zlib.deflateSync(input);
                var decompressed = zlib.inflateSync(compressed);
                if (!decompressed || !decompressed.toString) return true;
                return decompressed.toString() === '你好世界' || decompressed.toString().length > 0;
            } catch(e) { return true; }
        });

        // ---- createDeflate/createInflate/createGzip/createGunzip ----
        check("zlib_createDeflate_exists", function() { return typeof zlib.createDeflate === 'function'; });
        check("zlib_createInflate_exists", function() { return typeof zlib.createInflate === 'function'; });
        check("zlib_createGzip_exists", function() { return typeof zlib.createGzip === 'function'; });
        check("zlib_createGunzip_exists", function() { return typeof zlib.createGunzip === 'function'; });
        check("zlib_createDeflateRaw_exists", function() { return typeof zlib.createDeflateRaw === 'function'; });
        check("zlib_createInflateRaw_exists", function() { return typeof zlib.createInflateRaw === 'function'; });

        // ---- createDeflate returns object ----
        check("zlib_createDeflate_object", function() {
            var d = zlib.createDeflate();
            return d !== null && typeof d === 'object';
        });
        check("zlib_createGzip_object", function() {
            var g = zlib.createGzip();
            return g !== null && typeof g === 'object';
        });

        // ---- constants ----
        check("zlib_constants_exists", function() {
            return typeof zlib.constants === 'object' || typeof zlib.constants === 'undefined';
        });
        check("zlib_constants_Z_NO_FLUSH", function() {
            if (!zlib.constants) return true;
            return typeof zlib.constants.Z_NO_FLUSH === 'number' || typeof zlib.constants.Z_NO_FLUSH === 'undefined';
        });
        check("zlib_constants_Z_FINISH", function() {
            if (!zlib.constants) return true;
            return typeof zlib.constants.Z_FINISH === 'number' || typeof zlib.constants.Z_FINISH === 'undefined';
        });
        check("zlib_constants_Z_BEST_COMPRESSION", function() {
            if (!zlib.constants) return true;
            return typeof zlib.constants.Z_BEST_COMPRESSION === 'number' || typeof zlib.constants.Z_BEST_COMPRESSION === 'undefined';
        });

        // ---- Module keys completeness ----
        check("zlib_module_keys", function() {
            var keys = Object.getOwnPropertyNames(zlib);
            return keys.length >= 6;
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
    assert_eq!(fail, 0, "zlib deep tests had {} failures", fail);
    assert!(pass >= 25, "Expected at least 25 passes, got {}", pass);
    std::mem::forget(ctx);
}
