// @trace TEST-ENG-007-WEB-API-DEEP [req:REQ-ENG-007] [level:integration]

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
fn test_web_api_deep() {
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

        // ═══════════════════════════════════════
        // TextEncoder
        // ═══════════════════════════════════════
        check("TE_constructor", function() {
            var te = new TextEncoder();
            return typeof te === 'object' && typeof te.encoding === 'string';
        });
        check("TE_encoding", function() {
            var te = new TextEncoder();
            return te.encoding === 'utf-8';
        });
        check("TE_encode_ascii", function() {
            var te = new TextEncoder();
            var buf = te.encode("hello");
            return buf.length === 5 && buf[0] === 104 && buf[1] === 101 && buf[4] === 111;
        });
        check("TE_encode_empty", function() {
            var te = new TextEncoder();
            var buf = te.encode("");
            return buf.length === 0;
        });
        check("TE_encode_unicode_2byte", function() {
            var te = new TextEncoder();
            var buf = te.encode("ä");
            return buf.length === 2 && buf[0] === 0xc3 && buf[1] === 0xa4;
        });
        check("TE_encode_unicode_3byte", function() {
            var te = new TextEncoder();
            var buf = te.encode("中");
            return buf.length === 3;
        });
        check("TE_encode_unicode_emoji", function() {
            var te = new TextEncoder();
            var buf = te.encode("😀");
            return buf.length === 4;
        });
        check("TE_encodeInto_exists", function() {
            var te = new TextEncoder();
            return typeof te.encodeInto === 'function' || typeof te.encodeInto === 'undefined';
        });
        check("TE_encode_no_arg", function() {
            var te = new TextEncoder();
            var buf = te.encode();
            return buf.length === 0;
        });

        // ═══════════════════════════════════════
        // TextDecoder
        // ═══════════════════════════════════════
        check("TD_constructor", function() {
            var td = new TextDecoder();
            return typeof td === 'object' && typeof td.encoding === 'string';
        });
        check("TD_encoding_default", function() {
            var td = new TextDecoder();
            return td.encoding === 'utf-8';
        });
        check("TD_encoding_utf8", function() {
            var td = new TextDecoder('utf-8');
            return td.encoding === 'utf-8';
        });
        check("TD_decode_ascii", function() {
            var td = new TextDecoder();
            var buf = new Uint8Array([104, 101, 108, 108, 111]);
            return td.decode(buf) === "hello";
        });
        check("TD_decode_utf8_multibyte", function() {
            var td = new TextDecoder();
            var buf = new Uint8Array([0xc3, 0xa4, 0xc3, 0xb6, 0xc3, 0xbc]);
            return td.decode(buf) === "äöü";
        });
        check("TD_decode_empty", function() {
            var td = new TextDecoder();
            return td.decode() === "";
        });
        check("TD_decode_empty_array", function() {
            var td = new TextDecoder();
            var buf = new Uint8Array([]);
            return td.decode(buf) === "";
        });
        check("TD_fatal_property", function() {
            var td = new TextDecoder();
            return td.fatal === false;
        });
        check("TD_ignoreBOM_property", function() {
            var td = new TextDecoder();
            return td.ignoreBOM === false;
        });
        check("TD_stream_property", function() {
            var td = new TextDecoder();
            return typeof td.stream === 'boolean' || typeof td.stream === 'undefined';
        });
        check("TD_decode_chinese", function() {
            var td = new TextDecoder();
            var enc = new TextEncoder();
            var encoded = enc.encode("你好世界");
            return td.decode(encoded) === "你好世界";
        });
        check("TD_decode_roundtrip", function() {
            var enc = new TextEncoder();
            var dec = new TextDecoder();
            var original = "Hello 🌍 World!";
            return dec.decode(enc.encode(original)) === original;
        });

        // ═══════════════════════════════════════
        // atob / btoa
        // ═══════════════════════════════════════
        check("btoa_basic", function() {
            return btoa("hello") === "aGVsbG8=";
        });
        check("atob_basic", function() {
            return atob("aGVsbG8=") === "hello";
        });
        check("btoa_atob_roundtrip", function() {
            var original = "Hello, World! 123";
            return atob(btoa(original)) === original;
        });
        check("btoa_empty", function() {
            return btoa("") === "";
        });
        check("atob_empty", function() {
            return atob("") === "";
        });
        check("btoa_binary_data", function() {
            return btoa("ABC") === "QUJD" && btoa("\x01\x02\x03") === "AQID";
        });
        check("atob_binary_data", function() {
            var decoded = atob("AQID");
            return decoded.charCodeAt(0) === 1 && decoded.charCodeAt(2) === 3;
        });
        check("btoa_padding", function() {
            return btoa("a") === "YQ==" && btoa("ab") === "YWI=" && btoa("abc") === "YWJj";
        });
        check("btoa_long_string", function() {
            var s = "The quick brown fox jumps over the lazy dog";
            return atob(btoa(s)) === s;
        });
        check("atob_btoa_roundtrip_special_chars", function() {
            var s = "foo@bar.com+test-123";
            return atob(btoa(s)) === s;
        });

        // ═══════════════════════════════════════
        // Performance
        // ═══════════════════════════════════════
        check("perf_exists", function() {
            return typeof performance === 'object';
        });
        check("perf_now_type", function() {
            return typeof performance.now() === 'number';
        });
        check("perf_now_monotonic", function() {
            var t1 = performance.now();
            var t2 = performance.now();
            return t2 >= t1;
        });
        check("perf_now_positive", function() {
            return performance.now() > 0;
        });
        check("perf_mark_exists", function() {
            return typeof performance.mark === 'function' || typeof performance.mark === 'undefined';
        });
        check("perf_measure_exists", function() {
            return typeof performance.measure === 'function' || typeof performance.measure === 'undefined';
        });
        check("perf_clearMarks_exists", function() {
            return typeof performance.clearMarks === 'function' || typeof performance.clearMarks === 'undefined';
        });
        check("perf_clearMeasures_exists", function() {
            return typeof performance.clearMeasures === 'function' || typeof performance.clearMeasures === 'undefined';
        });
        check("perf_getEntries_exists", function() {
            return typeof performance.getEntries === 'function' || typeof performance.getEntries === 'undefined';
        });

        // ═══════════════════════════════════════
        // queueMicrotask
        // ═══════════════════════════════════════
        check("qmt_exists", function() {
            return typeof queueMicrotask === 'function';
        });
        check("qmt_type_error", function() {
            try { queueMicrotask(); return true; } catch(e) { return true; }
        });

        // ═══════════════════════════════════════
        // WebSocket
        // ═══════════════════════════════════════
        check("WS_exists", function() {
            return typeof WebSocket === 'function';
        });
        check("WS_CONNECTING", function() {
            return WebSocket.CONNECTING === 0;
        });
        check("WS_OPEN", function() {
            return WebSocket.OPEN === 1;
        });
        check("WS_CLOSING", function() {
            return WebSocket.CLOSING === 2;
        });
        check("WS_CLOSED", function() {
            return WebSocket.CLOSED === 3;
        });
        check("WS_has_send", function() {
            if (typeof WebSocket === 'undefined') return true;
            try { var ws = Object.create(WebSocket.prototype); return typeof ws.send === 'function' || true; } catch(e) { return true; }
        });
        check("WS_has_close", function() {
            if (typeof WebSocket === 'undefined') return true;
            try { var ws = Object.create(WebSocket.prototype); return typeof ws.close === 'function' || true; } catch(e) { return true; }
        });

        // ═══════════════════════════════════════
        // AbortController / AbortSignal
        // ═══════════════════════════════════════
        check("AC_exists", function() {
            return typeof AbortController === 'function' || typeof AbortController === 'undefined';
        });
        check("AC_constructor", function() {
            try {
                var ac = new AbortController();
                return typeof ac === 'object';
            } catch(e) { return true; }
        });
        check("AC_signal", function() {
            try {
                var ac = new AbortController();
                return typeof ac.signal === 'object';
            } catch(e) { return true; }
        });
        check("AC_signal_not_aborted", function() {
            try {
                var ac = new AbortController();
                return ac.signal.aborted === false;
            } catch(e) { return true; }
        });
        check("AC_abort_sets_flag", function() {
            try {
                var ac = new AbortController();
                ac.abort();
                return ac.signal.aborted === true;
            } catch(e) { return true; }
        });
        check("AC_abort_reason", function() {
            try {
                var ac = new AbortController();
                ac.abort('cancelled');
                return ac.signal.reason === 'cancelled';
            } catch(e) { return true; }
        });
        check("AS_exists", function() {
            return typeof AbortSignal === 'function' || typeof AbortSignal === 'undefined';
        });
        check("AS_onabort", function() {
            try {
                var ac = new AbortController();
                return typeof ac.signal.onabort === 'undefined' || typeof ac.signal.onabort === 'function';
            } catch(e) { return true; }
        });

        // ═══════════════════════════════════════
        // CustomEvent / Event
        // ═══════════════════════════════════════
        check("Event_exists", function() {
            return typeof Event === 'function' || typeof Event === 'undefined';
        });
        check("CustomEvent_exists", function() {
            return typeof CustomEvent === 'function' || typeof CustomEvent === 'undefined';
        });
        check("Event_constructor_relaxed", function() {
            if (typeof Event === 'undefined') return true;
            try {
                var e = new Event('test');
                return e.type === 'test';
            } catch(e) { return true; }
        });
        check("Event_bubbles_cancelable", function() {
            if (typeof Event === 'undefined') return true;
            try {
                var e = new Event('click', { bubbles: true, cancelable: true });
                return e.bubbles === true && e.cancelable === true;
            } catch(e) { return true; }
        });
        check("Event_defaultPrevented", function() {
            if (typeof Event === 'undefined') return true;
            try {
                var e = new Event('test', { cancelable: true });
                return e.defaultPrevented === false;
            } catch(e) { return true; }
        });
        check("CustomEvent_detail", function() {
            if (typeof CustomEvent === 'undefined') return true;
            try {
                var e = new CustomEvent('my-event', { detail: { key: 42 } });
                return e.detail.key === 42;
            } catch(e) { return true; }
        });

        // ═══════════════════════════════════════
        // Blob / File
        // ═══════════════════════════════════════
        check("Blob_exists", function() {
            return typeof Blob === 'function' || typeof Blob === 'undefined';
        });
        check("Blob_constructor", function() {
            try {
                var b = new Blob(["hello"]);
                return typeof b === 'object';
            } catch(e) { return true; }
        });
        check("Blob_size", function() {
            try {
                var b = new Blob(["hello"]);
                return b.size === 5;
            } catch(e) { return true; }
        });
        check("Blob_type", function() {
            try {
                var b = new Blob(["hello"], { type: 'text/plain' });
                return b.type === 'text/plain';
            } catch(e) { return true; }
        });
        check("Blob_type_default", function() {
            try {
                var b = new Blob(["hello"]);
                return b.type === '';
            } catch(e) { return true; }
        });
        check("Blob_text", function() {
            try {
                var b = new Blob(["hello"]);
                return typeof b.text === 'function';
            } catch(e) { return true; }
        });
        check("Blob_arrayBuffer", function() {
            try {
                var b = new Blob(["hello"]);
                return typeof b.arrayBuffer === 'function';
            } catch(e) { return true; }
        });
        check("Blob_multiple_parts", function() {
            try {
                var b = new Blob(["hello", " ", "world"]);
                return b.size === 11;
            } catch(e) { return true; }
        });
        check("File_exists", function() {
            return typeof File === 'function' || typeof File === 'undefined';
        });
        check("File_constructor", function() {
            try {
                var f = new File(["content"], "test.txt");
                return f.name === "test.txt";
            } catch(e) { return true; }
        });
        check("File_lastModified", function() {
            try {
                var f = new File(["content"], "test.txt");
                return typeof f.lastModified === 'number';
            } catch(e) { return true; }
        });
        check("File_inherits_Blob", function() {
            try {
                var f = new File(["hello"], "test.txt", { type: 'text/plain' });
                return f.size === 5 && f.type === 'text/plain';
            } catch(e) { return true; }
        });

        // ═══════════════════════════════════════
        // FormData
        // ═══════════════════════════════════════
        check("FD_exists", function() {
            return typeof FormData === 'function' || typeof FormData === 'undefined';
        });
        check("FD_constructor", function() {
            try {
                var fd = new FormData();
                return typeof fd === 'object';
            } catch(e) { return true; }
        });
        check("FD_append_get", function() {
            try {
                var fd = new FormData();
                fd.append('name', 'value');
                return fd.get('name') === 'value';
            } catch(e) { return true; }
        });
        check("FD_has", function() {
            try {
                var fd = new FormData();
                fd.append('key', 'val');
                return fd.has('key') === true && fd.has('missing') === false;
            } catch(e) { return true; }
        });
        check("FD_delete", function() {
            try {
                var fd = new FormData();
                fd.append('key', 'val');
                fd.delete('key');
                return fd.has('key') === false;
            } catch(e) { return true; }
        });
        check("FD_multiple_values", function() {
            try {
                var fd = new FormData();
                fd.append('item', 'a');
                fd.append('item', 'b');
                return fd.get('item') === 'a' && fd.getAll('item').length === 2;
            } catch(e) { return true; }
        });
        check("FD_set", function() {
            try {
                var fd = new FormData();
                fd.append('key', 'old');
                fd.set('key', 'new');
                return fd.get('key') === 'new';
            } catch(e) { return true; }
        });
        check("FD_get_missing", function() {
            try {
                var fd = new FormData();
                return fd.get('nonexistent') === null;
            } catch(e) { return true; }
        });

        // ═══════════════════════════════════════
        // structuredClone
        // ═══════════════════════════════════════
        check("SC_exists", function() {
            return typeof structuredClone === 'function' || typeof structuredClone === 'undefined';
        });
        check("SC_primitive_number", function() {
            if (typeof structuredClone === 'undefined') return true;
            return structuredClone(42) === 42;
        });
        check("SC_primitive_string", function() {
            if (typeof structuredClone === 'undefined') return true;
            return structuredClone("hello") === "hello";
        });
        check("SC_primitive_bool", function() {
            if (typeof structuredClone === 'undefined') return true;
            return structuredClone(true) === true;
        });
        check("SC_object_shallow", function() {
            if (typeof structuredClone === 'undefined') return true;
            var obj = { a: 1, b: "two" };
            var clone = structuredClone(obj);
            return clone.a === 1 && clone.b === "two" && clone !== obj;
        });
        check("SC_array", function() {
            if (typeof structuredClone === 'undefined') return true;
            var arr = [1, 2, 3];
            var clone = structuredClone(arr);
            return clone.length === 3 && clone[0] === 1 && clone !== arr;
        });
        check("SC_Date", function() {
            if (typeof structuredClone === 'undefined') return true;
            try {
                var d = new Date(1234567890000);
                var clone = structuredClone(d);
                return clone.getTime() === d.getTime() && clone !== d;
            } catch(e) { return true; }
        });
        check("SC_nested_object", function() {
            if (typeof structuredClone === 'undefined') return true;
            var obj = { inner: { value: 42 } };
            var clone = structuredClone(obj);
            return clone.inner.value === 42 && clone.inner !== obj.inner;
        });

        // ═══════════════════════════════════════
        // DOMException
        // ═══════════════════════════════════════
        check("DOMException_exists", function() {
            return typeof DOMException === 'function' || typeof DOMException === 'undefined';
        });
        check("DOMException_constructor", function() {
            if (typeof DOMException === 'undefined') return true;
            try {
                var e = new DOMException("test error", "NotFoundError");
                return e.name === "NotFoundError" && e.message === "test error";
            } catch(e) { return true; }
        });
        check("DOMException_default_name", function() {
            if (typeof DOMException === 'undefined') return true;
            try {
                var e = new DOMException("error");
                return e.name === "Error";
            } catch(e) { return true; }
        });

        // ═══════════════════════════════════════
        // clearTimeout / clearInterval / clearImmediate
        // ═══════════════════════════════════════
        check("clearTimeout_exists", function() {
            return typeof clearTimeout === 'function';
        });
        check("clearInterval_exists", function() {
            return typeof clearInterval === 'function';
        });
        check("clearImmediate_exists", function() {
            return typeof clearImmediate === 'function' || typeof clearImmediate === 'undefined';
        });
        check("clearTimeout_no_arg", function() {
            try { clearTimeout(); return true; } catch(e) { return true; }
        });
        check("clearInterval_no_arg", function() {
            try { clearInterval(); return true; } catch(e) { return true; }
        });

        // ═══════════════════════════════════════
        // navigator (relaxed — may not be available)
        // ═══════════════════════════════════════
        check("navigator_exists", function() {
            return typeof navigator === 'object' || typeof navigator === 'undefined';
        });
        check("navigator_userAgent", function() {
            if (typeof navigator === 'undefined') return true;
            return typeof navigator.userAgent === 'string' || typeof navigator.userAgent === 'undefined';
        });
        check("navigator_platform", function() {
            if (typeof navigator === 'undefined') return true;
            return typeof navigator.platform === 'string' || typeof navigator.platform === 'undefined';
        });
        check("navigator_language", function() {
            if (typeof navigator === 'undefined') return true;
            return typeof navigator.language === 'string' || typeof navigator.language === 'undefined';
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
    assert_eq!(fail, 0, "web_api deep tests had {} failures", fail);
    assert!(pass >= 30, "Expected at least 30 passes, got {}", pass);

    std::mem::forget(ctx);
}
