// @trace TEST-ENG-007-ZLIB-SILENT [req:REQ-ENG-007] [level:integration]
//
// The two SILENT classes in node:zlib, driven at the JS surface:
//   1. bad input swallowed — every *Sync decompressor returned undefined
//      (node throws ZlibError with zlib's own message) and the Transform
//      classes ended with len=0 and no 'error' event;
//   2. gunzipSync truncated multi-member streams to the first member
//      (node decodes ALL RFC 1952 §2.2 concatenated members, verifying
//      every member's CRC32/ISIZE).

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
fn test_zlib_silent_failures_fixed() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let results = eval_string(
        &mut ctx,
        r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e).substring(0, 60)); }
        }
        function throwsMsg(fn, needle) {
            try { fn(); return null; }
            catch (e) {
                if (!e || !e.message) return null;
                if (needle === undefined) return e;
                return String(e.message).indexOf(needle) !== -1 ? e : null;
            }
        }

        var zlib = require('zlib');

        // ---- fixtures: valid streams of all 3 algorithms + garbage ----
        var zl = zlib.deflateSync(Buffer.from('zlib-bad-input-payload-payload'));
        var gz = zlib.gzipSync(Buffer.from('gzip-bad-input-payload-payload'));
        var rw = zlib.deflateRawSync(Buffer.from('raw-bad-input-payload-payload'));
        var garbage = Buffer.from([0xff,0xfe,0xfd,0xfc,0xfb,0xfa,0xf9,0xf8,0xf7,0xf6,0xf5,0xf4]);
        // For the zlib header check specifically, 0xfffe is a multiple of 31
        // with CM=15, so zlib diagnoses "unknown compression method"; use a
        // leading pair that fails the FCHECK modulus instead.
        var garbageZ = Buffer.from([0x33,0x32,0x31,0x30,0x2f,0x2e,0x2d,0x2c]);
        function trunc(buf, n) { return buf.slice(0, buf.length - n); }
        function flip(buf, off) { var c = Buffer.from(buf); c[off] ^= 0xff; return c; }
        function member(payload) { return zlib.gzipSync(Buffer.from(payload)); }

        // ---- Item 1a: sync decompressors THROW on bad input (was: undefined) ----
        check("sync_inflateSync_truncated_throws", function() {
            return throwsMsg(function() { zlib.inflateSync(trunc(zl, 5)); }, 'unexpected end of file') !== null;
        });
        check("sync_inflateSync_bad_header_throws", function() {
            return throwsMsg(function() { zlib.inflateSync(flip(zl, 1)); }, 'incorrect header check') !== null;
        });
        check("sync_inflateSync_garbage_throws", function() {
            return throwsMsg(function() { zlib.inflateSync(garbageZ); }, 'incorrect header check') !== null;
        });
        check("sync_gunzipSync_truncated_throws", function() {
            return throwsMsg(function() { zlib.gunzipSync(trunc(gz, 5)); }, 'unexpected end of file') !== null;
        });
        check("sync_gunzipSync_bad_magic_throws", function() {
            return throwsMsg(function() { zlib.gunzipSync(flip(gz, 0)); }, 'incorrect header check') !== null;
        });
        check("sync_gunzipSync_garbage_throws", function() {
            return throwsMsg(function() { zlib.gunzipSync(garbage); }, 'incorrect header check') !== null;
        });
        check("sync_inflateRawSync_truncated_throws", function() {
            return throwsMsg(function() { zlib.inflateRawSync(trunc(rw, 3)); }, 'unexpected end of file') !== null;
        });
        check("sync_inflateRawSync_garbage_throws", function() {
            return throwsMsg(function() { zlib.inflateRawSync(garbage); }, undefined) !== null;
        });
        check("sync_unzipSync_garbage_throws", function() {
            return throwsMsg(function() { zlib.unzipSync(garbage); }, 'incorrect header check') !== null;
        });
        check("sync_brotliDecompressSync_garbage_throws", function() {
            return throwsMsg(function() { zlib.brotliDecompressSync(garbage); }, undefined) !== null;
        });
        check("sync_gunzipSync_empty_throws", function() {
            return throwsMsg(function() { zlib.gunzipSync(Buffer.alloc(0)); }, 'unexpected end of file') !== null;
        });
        check("sync_inflateSync_one_byte_throws", function() {
            return throwsMsg(function() { zlib.inflateSync(Buffer.from([0x78])); }, 'unexpected end of file') !== null;
        });

        // ---- Item 1b: error shape is a ZlibError (message + code + errno) ----
        check("sync_error_code_errno", function() {
            var e = throwsMsg(function() { zlib.gunzipSync(garbage); }, 'incorrect header check');
            return e && e.code === 'Z_DATA_ERROR' && e.errno === -3;
        });
        check("sync_error_is_error_instance", function() {
            try { zlib.inflateSync(garbage); return false; }
            catch (e) { return e instanceof Error; }
        });

        // ---- Item 1c: streaming Transform surfaces 'error' (was: silent END len=0) ----
        check("stream_Gunzip_bad_magic_error_event", function() {
            var errs = [], finished = 0, dataLen = -1;
            var g = new zlib.Gunzip();
            g.on('error', function(e) { errs.push(e); });
            g.on('finish', function() { finished++; });
            var chunks = [];
            g.on('data', function(c) { chunks.push(c); });
            g.end(flip(gz, 0));
            dataLen = chunks.reduce(function(a, c) { return a + c.length; }, 0);
            return errs.length === 1
                && String(errs[0].message).indexOf('incorrect header check') !== -1
                && dataLen === 0 && finished === 0;
        });
        check("stream_Gunzip_truncated_error_event", function() {
            var errs = [];
            var g = new zlib.Gunzip();
            g.on('error', function(e) { errs.push(e); });
            g.on('data', function() {});
            g.end(trunc(gz, 5));
            return errs.length === 1 && String(errs[0].message).indexOf('unexpected end of file') !== -1;
        });
        check("stream_Inflate_garbage_error_event", function() {
            var errs = [];
            var i = new zlib.Inflate();
            i.on('error', function(e) { errs.push(e); });
            i.on('data', function() {});
            i.end(garbage);
            return errs.length === 1;
        });
        check("stream_BrotliDecompress_garbage_error_event", function() {
            var errs = [];
            var b = new zlib.BrotliDecompress();
            b.on('error', function(e) { errs.push(e); });
            b.on('data', function() {});
            b.end(garbage);
            return errs.length === 1;
        });
        check("stream_Gunzip_ok_still_finishes", function() {
            var finished = 0, got = '';
            var g = new zlib.Gunzip();
            g.on('data', function(c) { got += c.toString(); });
            g.on('finish', function() { finished++; });
            g.end(gz);
            return got === 'gzip-bad-input-payload-payload' && finished === 1;
        });

        // ---- Item 2: gunzipSync decodes ALL members (was: first only) ----
        check("gunzipSync_two_members_full", function() {
            var both = Buffer.concat([member('first-|'), member('second')]);
            return zlib.gunzipSync(both).toString() === 'first-|second';
        });
        check("gunzipSync_three_members_full", function() {
            var all = Buffer.concat([member('A-'), member('B-'), member('C')]);
            return zlib.gunzipSync(all).toString() === 'A-B-C';
        });
        check("gunzipSync_member2_crc_verified", function() {
            var m1 = member('member-one-');
            var m2 = member('member-two');
            m2[m2.length - 8] ^= 0xff; // corrupt member 2's CRC32
            try { zlib.gunzipSync(Buffer.concat([m1, m2])); return false; }
            catch (e) { return String(e.message).indexOf('incorrect') !== -1; }
        });
        check("unzipSync_two_members_full", function() {
            var both = Buffer.concat([member('auto-1|'), member('auto-2')]);
            return zlib.unzipSync(both).toString() === 'auto-1|auto-2';
        });
        check("gunzipSync_empty_members_ok", function() {
            var empties = Buffer.concat([member(''), member('x'), member('')]);
            var out = zlib.gunzipSync(empties);
            return out.length === 1 && out.toString() === 'x';
        });
        check("gunzipSync_unicode_members", function() {
            var both = Buffer.concat([member('你好'), member('世界')]);
            return zlib.gunzipSync(both).toString() === '你好世界';
        });
        check("stream_Gunzip_two_members_full", function() {
            var got = '';
            var g = new zlib.Gunzip();
            g.on('data', function(c) { got += c.toString(); });
            g.end(Buffer.concat([member('s-first-|'), member('s-second')]));
            return got === 's-first-|s-second';
        });

        // ---- async callbacks: Error-valued err + multi-member ----
        check("async_gunzip_error_is_error_object", function() {
            var caught = null;
            zlib.gunzip(garbage, function(err, out) { caught = err; });
            return caught && caught instanceof Error
                && String(caught.message).indexOf('incorrect header check') !== -1
                && caught.code === 'Z_DATA_ERROR';
        });
        check("async_gunzip_multi_member_full", function() {
            var got = null;
            zlib.gunzip(Buffer.concat([member('a1-'), member('a2')]), function(err, out) {
                got = err ? null : out.toString();
            });
            return got === 'a1-a2';
        });
        check("async_inflate_error_object", function() {
            var caught = null;
            zlib.inflate(garbage, function(err, out) { caught = err; });
            return caught instanceof Error;
        });

        // ---- regressions: happy paths untouched ----
        check("reg_roundtrips_ok", function() {
            return zlib.inflateSync(zl).toString() === 'zlib-bad-input-payload-payload'
                && zlib.gunzipSync(gz).toString() === 'gzip-bad-input-payload-payload'
                && zlib.inflateRawSync(rw).toString() === 'raw-bad-input-payload-payload';
        });
        check("reg_unzipSync_each_wrapper", function() {
            return zlib.unzipSync(zl).length > 0 && zlib.unzipSync(gz).length > 0 && zlib.unzipSync(rw).length > 0;
        });
        check("reg_empty_roundtrips_ok", function() {
            return zlib.inflateSync(zlib.deflateSync(Buffer.alloc(0))).length === 0
                && zlib.gunzipSync(zlib.gzipSync(Buffer.alloc(0))).length === 0
                && zlib.inflateRawSync(zlib.deflateRawSync(Buffer.alloc(0))).length === 0
                && zlib.unzipSync(zlib.gzipSync(Buffer.alloc(0))).length === 0;
        });
        check("reg_brotli_roundtrip_ok", function() {
            var c = zlib.brotliCompressSync(Buffer.from('brotli-still-works'));
            return zlib.brotliDecompressSync(c).toString() === 'brotli-still-works';
        });
        check("reg_crc32_ok", function() {
            return zlib.crc32(Buffer.from('123456789')) === 0xCBF43926;
        });

        results.join("|");
    "#,
    );

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
    assert_eq!(fail, 0, "zlib silent-failure tests had {} failures", fail);
    assert!(pass >= 30, "Expected at least 30 passes, got {}", pass);
    bun_runtime::shutdown_thread_sm();
}
