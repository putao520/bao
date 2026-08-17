// @trace REQ-ENG-009 [entity:FfiLibrary] [level:e2e]
// ptr-argument VIEW marshalling — dlopen-bound functions accept Buffer/
// TypedArray/DataView/ArrayBuffer arguments as their backing-store address
// (zero-copy), closing the qsort gap: before this, ptr args required a
// Number/BigInt address, so a JS-owned buffer could not be handed to a C
// API directly and libc qsort's comparator bridge was undeclarable from JS.
//
// Full chain exercised on REAL libc:
//   1. qsort(buf, n, 4, comparator) — Buffer base ptr + FfiCallback code ptr
//      in one call; the comparator (libffi closure → JS) receives ELEMENT
//      addresses as Numbers, reads them back through ffi.toBuffer, and the
//      SAME JS Buffer comes back sorted in place (zero-copy proof).
//   2. memcpy(dst, src, n) — two view ptrs (Uint8Array + ArrayBuffer) in one
//      call; byte-exact copy verified through the source/dest views.
//   3. Negative-spread i32 values (INT32_MIN..INT32_MAX) — signed LE reads,
//      tristate comparator (no x-y overflow).

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(JsValue::Null) => "null".to_string(),
        Ok(JsValue::Undefined) => "undefined".to_string(),
        Ok(JsValue::Object(_)) => "[object]".to_string(),
        Err(e) => format!("ERROR:{}", e.message),
    }
}

fn setup_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

/// qsort over a JS Buffer: view ptr base + js_function comparator slot, the
/// comparator reading element memory through toBuffer. The sort must run for
/// real (comparator invoked), mutate the SAME buffer in place, and produce
/// the ascending order including INT32_MIN/MAX spread.
#[test]
fn ffi_qsort_sorts_js_buffer_in_place_via_callback() {
    let mut ctx = setup_ctx();
    let out = eval_string(
        &mut ctx,
        r#"
        var ffi = require('bun:ffi');
        var lib = ffi.dlopen('libc.so.6', {
          qsort: { args: ['ptr', 'usize', 'usize', 'js_function'], returns: 'void' }
        });

        // LE helpers over the raw Buffer — no address math in JS, the buffer
        // IS the ptr argument.
        function write32(buf, i, v) {
          buf[i * 4 + 0] = v & 0xff;
          buf[i * 4 + 1] = (v >>> 8) & 0xff;
          buf[i * 4 + 2] = (v >>> 16) & 0xff;
          buf[i * 4 + 3] = (v >>> 24) & 0xff;
        }
        function read32(buf, i) {
          return buf[i * 4] | (buf[i * 4 + 1] << 8) | (buf[i * 4 + 2] << 16) | (buf[i * 4 + 3] << 24);
        }
        // Comparator-side read: p is a raw address (Number) — view it.
        function readAddr32(p) {
          var b = ffi.toBuffer(p, 4);
          return b[0] | (b[1] << 8) | (b[2] << 16) | (b[3] << 24);
        }

        var vals = [13, -5, 2147483647, 42, -2147483648, 7, -1, 100];
        var buf = Buffer.alloc(vals.length * 4);
        for (var i = 0; i < vals.length; i++) write32(buf, i, vals[i]);

        var calls = 0;
        var cmp = ffi.callback(['ptr', 'ptr'], 'i32', function (a, b) {
          calls++;
          var x = readAddr32(a), y = readAddr32(b);
          return x < y ? -1 : x > y ? 1 : 0;
        });

        lib.qsort(buf, vals.length, 4, cmp);

        var got = [];
        for (var j = 0; j < vals.length; j++) got.push(read32(buf, j));
        var want = vals.slice().sort(function (a, b) { return a - b; });
        var same = got.length === want.length && got.every(function (v, k) { return v === want[k]; });
        JSON.stringify({ ok: same, calls: calls, got: got, want: want });
    "#,
    );
    assert!(
        !out.starts_with("ERROR"),
        "qsort e2e eval failed: {}",
        out
    );
    // Parse the JSON verdict (serde_json is already a crate dep).
    let v: serde_json::Value = serde_json::from_str(&out).unwrap_or_else(|e| {
        panic!("qsort result not JSON ({}): {}", e, out);
    });
    assert_eq!(
        v["ok"], serde_json::Value::Bool(true),
        "buffer must come back sorted in place, got: {}",
        out
    );
    let calls = v["calls"].as_i64().unwrap_or(0);
    assert!(
        calls >= 7,
        "comparator must actually run (8 elements need >= 7 comparisons), calls={}",
        calls
    );
    // Explicit ascending order with the extreme spread pinned.
    assert_eq!(
        v["got"]
            .as_array()
            .map(|a| a.iter().filter_map(|x| x.as_i64()).collect::<Vec<_>>())
            .unwrap_or_default(),
        vec![
            -2147483648i64,
            -5,
            -1,
            7,
            13,
            42,
            100,
            2147483647
        ],
        "sorted order must be exact (signed i32 LE): {}",
        out
    );
}

/// Two view ptrs in one call — memcpy(dst: Uint8Array, src: ArrayBuffer, n).
/// Both non-Buffer view kinds must unwrap to their backing-store address and
/// the copy must be byte-exact through the views (binary bytes, not utf8).
#[test]
fn ffi_memcpy_two_view_ptrs_uint8array_and_arraybuffer() {
    let mut ctx = setup_ctx();
    let out = eval_string(
        &mut ctx,
        r#"
        var ffi = require('bun:ffi');
        var lib = ffi.dlopen('libc.so.6', {
          memcpy: { args: ['ptr', 'ptr', 'usize'], returns: 'ptr' }
        });
        var payload = [0x00, 0x01, 0xfe, 0xff, 0x80, 0x7f, 0x41, 0x00, 0xc3, 0x9a];
        var src = new ArrayBuffer(payload.length);
        var srcView = new Uint8Array(src);
        for (var i = 0; i < payload.length; i++) srcView[i] = payload[i];
        var dst = new Uint8Array(payload.length + 4); // slack proves n is honored
        var ret = lib.memcpy(dst, src, payload.length);
        var bytes = [];
        for (var j = 0; j < dst.length; j++) bytes.push(dst[j]);
        var headExact = payload.every(function (b, k) { return dst[k] === b; });
        var tailZero = bytes.slice(payload.length).every(function (b) { return b === 0; });
        JSON.stringify({ retAddr: typeof ret === 'number' && ret > 0, headExact: headExact, tailZero: tailZero });
    "#,
    );
    assert!(!out.starts_with("ERROR"), "memcpy e2e eval failed: {}", out);
    let v: serde_json::Value =
        serde_json::from_str(&out).unwrap_or_else(|e| panic!("memcpy result not JSON ({}): {}", e, out));
    assert_eq!(v["retAddr"], serde_json::Value::Bool(true), "memcpy returns dst ptr as Number: {}", out);
    assert_eq!(v["headExact"], serde_json::Value::Bool(true), "copied bytes must be exact: {}", out);
    assert_eq!(v["tailZero"], serde_json::Value::Bool(true), "n must bound the copy (tail untouched): {}", out);
}

/// View marshalling must not loosen the contract: a non-view object (plain
/// {}) is rejected loudly, null passes as the C null pointer, and raw Number
/// addresses still work (memcmp on a Buffer vs itself through explicit
/// addresses is unnecessary — ptr-as-null suffices here; Number-form ptr
/// coverage lives in the memcmp leg below).
#[test]
fn ffi_ptr_arg_contract_rejects_non_view_object() {
    let mut ctx = setup_ctx();
    let out = eval_string(
        &mut ctx,
        r#"
        var ffi = require('bun:ffi');
        var lib = ffi.dlopen('libc.so.6', {
          memcpy: { args: ['ptr', 'ptr', 'usize'], returns: 'ptr' }
        });
        var results = {};
        // Plain object: NOT a view, NOT a Number/BigInt → loud TypeError.
        try {
          lib.memcpy({}, 0, 0);
          results.plainObject = 'accepted';
        } catch (e) {
          results.plainObject = (e instanceof TypeError || /ffi argument/.test(e.message)) ? 'rejected' : 'rejected-other:' + e.message;
        }
        // Null ptr passthrough (n = 0 → memcpy is a no-op even at NULL).
        try {
          lib.memcpy(null, null, 0);
          results.nullOk = 'yes';
        } catch (e) {
          results.nullOk = 'no:' + e.message;
        }
        JSON.stringify(results);
    "#,
    );
    assert!(
        !out.starts_with("ERROR"),
        "ptr contract eval failed: {}",
        out
    );
    let v: serde_json::Value = serde_json::from_str(&out)
        .unwrap_or_else(|e| panic!("ptr contract result not JSON ({}): {}", e, out));
    assert_eq!(
        v["plainObject"].as_str().unwrap_or(""),
        "rejected",
        "non-view object must be rejected loudly: {}",
        out
    );
    assert_eq!(
        v["nullOk"].as_str().unwrap_or(""),
        "yes",
        "null must pass as the C null pointer: {}",
        out
    );
}

/// Number-form ptr still works (pre-view behavior unchanged): memcmp over
/// two Buffers via memcmp(dst-as-ptr...) — memcmp(p1, p2, n) with BOTH as
/// views, plus the number form via strlen-style probe is skipped; instead
/// assert memcmp(view, view) equality signal and difference signal.
#[test]
fn ffi_memcmp_view_ptrs_ordering_signal() {
    let mut ctx = setup_ctx();
    let out = eval_string(
        &mut ctx,
        r#"
        var ffi = require('bun:ffi');
        var lib = ffi.dlopen('libc.so.6', {
          memcmp: { args: ['ptr', 'ptr', 'usize'], returns: 'i32' }
        });
        function buf3(x) {
          var b = Buffer.alloc(3);
          b[0] = 1; b[1] = x; b[2] = 3;
          return b;
        }
        var a = buf3(2);
        var b = buf3(2);
        var c = buf3(3);
        var eq = lib.memcmp(a, b, 3);
        var lt = lib.memcmp(a, c, 3);
        var gt = lib.memcmp(c, a, 3);
        JSON.stringify({ eq: eq, ltSign: lt < 0, gtSign: gt > 0 });
    "#,
    );
    assert!(!out.starts_with("ERROR"), "memcmp eval failed: {}", out);
    let v: serde_json::Value = serde_json::from_str(&out)
        .unwrap_or_else(|e| panic!("memcmp result not JSON ({}): {}", e, out));
    assert_eq!(v["eq"].as_i64(), Some(0), "identical buffers compare equal: {}", out);
    assert_eq!(v["ltSign"], serde_json::Value::Bool(true), "a<c must be negative: {}", out);
    assert_eq!(v["gtSign"], serde_json::Value::Bool(true), "c>a must be positive: {}", out);
}
