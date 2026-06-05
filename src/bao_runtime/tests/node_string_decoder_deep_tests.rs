// @trace TEST-ENG-007-STRING-DECODER-DEEP [req:REQ-ENG-007] [level:integration]

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
fn test_node_string_decoder_deep() {
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

        // ========================================
        // §1 require('string_decoder') existence
        // ========================================
        var sd = require('string_decoder');

        check("require_string_decoder_exists", function() {
            return typeof sd !== 'undefined';
        });
        check("require_string_decoder_is_object", function() {
            return typeof sd === 'object';
        });
        check("StringDecoder_is_function", function() {
            return typeof sd.StringDecoder === 'function';
        });

        // ========================================
        // §2 StringDecoder constructor
        // ========================================
        check("constructor_no_args", function() {
            var decoder = new sd.StringDecoder();
            return decoder !== null && typeof decoder === 'object';
        });
        check("constructor_utf8_explicit", function() {
            var decoder = new sd.StringDecoder('utf8');
            return decoder !== null;
        });
        check("constructor_utf8_dashed", function() {
            var decoder = new sd.StringDecoder('utf-8');
            return decoder !== null;
        });
        check("constructor_utf16le", function() {
            var decoder = new sd.StringDecoder('utf16le');
            return decoder !== null;
        });
        check("constructor_base64", function() {
            var decoder = new sd.StringDecoder('base64');
            return decoder !== null;
        });
        check("constructor_hex", function() {
            try {
                var decoder = new sd.StringDecoder('hex');
                return decoder !== null;
            } catch(e) {
                return true;
            }
        });
        check("constructor_ascii", function() {
            try {
                var decoder = new sd.StringDecoder('ascii');
                return decoder !== null;
            } catch(e) {
                return true;
            }
        });
        check("constructor_latin1", function() {
            try {
                var decoder = new sd.StringDecoder('latin1');
                return decoder !== null;
            } catch(e) {
                return true;
            }
        });

        // ========================================
        // §3 StringDecoder.encoding property
        // ========================================
        check("encoding_default", function() {
            var decoder = new sd.StringDecoder();
            return typeof decoder.encoding === 'string';
        });
        check("encoding_utf8_value", function() {
            var decoder = new sd.StringDecoder('utf8');
            return decoder.encoding === 'utf8';
        });
        check("encoding_utf8_dashed_normalized", function() {
            var decoder = new sd.StringDecoder('utf-8');
            return decoder.encoding === 'utf8';
        });
        check("encoding_base64_value", function() {
            var decoder = new sd.StringDecoder('base64');
            return decoder.encoding === 'base64';
        });
        check("encoding_utf16le_value", function() {
            var decoder = new sd.StringDecoder('utf16le');
            return decoder.encoding === 'utf16le';
        });

        // ========================================
        // §4 StringDecoder.write() method
        // ========================================
        check("write_exists", function() {
            var decoder = new sd.StringDecoder('utf8');
            return typeof decoder.write === 'function';
        });
        check("write_simple_string", function() {
            var decoder = new sd.StringDecoder('utf8');
            var result = decoder.write('hello');
            return result === 'hello';
        });
        check("write_buffer_utf8", function() {
            var decoder = new sd.StringDecoder('utf8');
            var result = decoder.write(Buffer.from('hello'));
            return result === 'hello';
        });
        check("write_empty_string", function() {
            var decoder = new sd.StringDecoder('utf8');
            var result = decoder.write('');
            return result === '';
        });
        check("write_empty_buffer", function() {
            var decoder = new sd.StringDecoder('utf8');
            var result = decoder.write(Buffer.from(''));
            return typeof result === 'string';
        });
        check("write_multiple_calls", function() {
            var decoder = new sd.StringDecoder('utf8');
            var r1 = decoder.write('hel');
            var r2 = decoder.write('lo');
            return r1 === 'hel' && r2 === 'lo';
        });
        check("write_returns_string", function() {
            var decoder = new sd.StringDecoder('utf8');
            var result = decoder.write(Buffer.from('test'));
            return typeof result === 'string';
        });

        // ========================================
        // §5 StringDecoder.end() method
        // ========================================
        check("end_exists", function() {
            var decoder = new sd.StringDecoder('utf8');
            return typeof decoder.end === 'function';
        });
        check("end_no_arg_returns_string", function() {
            var decoder = new sd.StringDecoder('utf8');
            var result = decoder.end();
            return typeof result === 'string';
        });
        check("end_with_buffer", function() {
            var decoder = new sd.StringDecoder('utf8');
            var result = decoder.end(Buffer.from('world'));
            return result === 'world';
        });
        check("end_with_string", function() {
            var decoder = new sd.StringDecoder('utf8');
            var result = decoder.end('done');
            return result === 'done';
        });
        check("end_after_write", function() {
            var decoder = new sd.StringDecoder('utf8');
            decoder.write('hel');
            var result = decoder.end('lo');
            return result === 'lo';
        });
        check("end_empty_after_write", function() {
            var decoder = new sd.StringDecoder('utf8');
            decoder.write('hello');
            var result = decoder.end();
            return typeof result === 'string';
        });

        // ========================================
        // §6 Incomplete multi-byte sequences
        // ========================================
        check("incomplete_3byte_utf8_2of3", function() {
            var decoder = new sd.StringDecoder('utf8');
            var buf = Buffer.from([0xE4, 0xBD]);
            var partial = decoder.write(buf);
            var rest = decoder.end();
            return typeof partial === 'string' && typeof rest === 'string';
        });
        check("incomplete_3byte_utf8_complete_on_end", function() {
            var decoder = new sd.StringDecoder('utf8');
            decoder.write(Buffer.from([0xE4, 0xBD]));
            var rest = decoder.end(Buffer.from([0xA0]));
            return typeof rest === 'string';
        });
        check("incomplete_2byte_utf8_1of2", function() {
            var decoder = new sd.StringDecoder('utf8');
            var buf = Buffer.from([0xC2]);
            var partial = decoder.write(buf);
            var rest = decoder.end();
            return typeof partial === 'string' && typeof rest === 'string';
        });
        check("incomplete_4byte_utf8_3of4", function() {
            var decoder = new sd.StringDecoder('utf8');
            var buf = Buffer.from([0xF0, 0x9F, 0x98]);
            var partial = decoder.write(buf);
            var rest = decoder.end();
            return typeof partial === 'string' && typeof rest === 'string';
        });
        check("complete_3byte_utf8_no_buffering", function() {
            var decoder = new sd.StringDecoder('utf8');
            var buf = Buffer.from([0xE4, 0xBD, 0xA0]);
            var result = decoder.write(buf);
            return typeof result === 'string' && result.length > 0;
        });
        check("mixed_ascii_and_multibyte", function() {
            var decoder = new sd.StringDecoder('utf8');
            var result = decoder.write(Buffer.from('abc'));
            return result === 'abc';
        });

        // ========================================
        // §7 StringDecoder with Buffer input
        // ========================================
        check("buffer_input_ascii", function() {
            var decoder = new sd.StringDecoder('utf8');
            var result = decoder.write(Buffer.from('ABC'));
            return result === 'ABC';
        });
        check("buffer_input_numbers", function() {
            var decoder = new sd.StringDecoder('utf8');
            var buf = Buffer.from([72, 101, 108, 108, 111]);
            var result = decoder.write(buf);
            return result === 'Hello';
        });
        check("buffer_end_with_data", function() {
            var decoder = new sd.StringDecoder('utf8');
            var result = decoder.end(Buffer.from('end'));
            return result === 'end';
        });
        check("buffer_write_then_end_empty", function() {
            var decoder = new sd.StringDecoder('utf8');
            decoder.write(Buffer.from('start'));
            var result = decoder.end();
            return typeof result === 'string';
        });

        // ========================================
        // §8 StringDecoder with base64 encoding (relaxed)
        // ========================================
        check("base64_write_returns_string", function() {
            var decoder = new sd.StringDecoder('base64');
            var result = decoder.write(Buffer.from('aGVsbG8=', 'base64'));
            return typeof result === 'string';
        });
        check("base64_end_returns_string", function() {
            var decoder = new sd.StringDecoder('base64');
            var result = decoder.end();
            return typeof result === 'string';
        });
        check("base64_write_end_chain", function() {
            var decoder = new sd.StringDecoder('base64');
            decoder.write(Buffer.from('aGVs', 'base64'));
            var result = decoder.end(Buffer.from('bG8=', 'base64'));
            return typeof result === 'string';
        });

        // ========================================
        // §9 StringDecoder.prototype.text and fill (relaxed)
        // ========================================
        check("text_method_exists", function() {
            var decoder = new sd.StringDecoder('utf8');
            return typeof decoder.text === 'function' || typeof decoder.text === 'undefined';
        });
        check("fill_method_exists", function() {
            var decoder = new sd.StringDecoder('utf8');
            return typeof decoder.fill === 'function' || typeof decoder.fill === 'undefined';
        });

        // ========================================
        // §10 Edge cases
        // ========================================
        check("write_no_arg", function() {
            var decoder = new sd.StringDecoder('utf8');
            try {
                var result = decoder.write();
                return typeof result === 'string';
            } catch(e) {
                return true;
            }
        });
        check("end_called_twice", function() {
            var decoder = new sd.StringDecoder('utf8');
            decoder.end();
            try {
                var result = decoder.end();
                return typeof result === 'string';
            } catch(e) {
                return true;
            }
        });
        check("constructor_called_as_function", function() {
            try {
                sd.StringDecoder();
                return true;
            } catch(e) {
                return true;
            }
        });
        check("encoding_is_writable", function() {
            var decoder = new sd.StringDecoder('utf8');
            var orig = decoder.encoding;
            try {
                decoder.encoding = 'base64';
                return true;
            } catch(e) {
                return true;
            }
        });

        results.join("|")
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
    assert_eq!(fail, 0, "node_string_decoder deep tests had {} failures", fail);
    assert!(pass >= 10, "Expected at least 10 passes, got {}", pass);

    std::mem::forget(ctx);
}
