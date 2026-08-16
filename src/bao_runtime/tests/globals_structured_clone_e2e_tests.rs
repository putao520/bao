// @trace TEST-ENG-006-SC [req:REQ-ENG-006] [level:e2e]
// structuredClone fidelity e2e — the global now rides SpiderMonkey's
// JS_WriteStructuredClone / JS_ReadStructuredClone (the same engine facility
// worker postMessage uses). These tests pin the classes the old
// JSON.parse(JSON.stringify) fallback corrupted silently:
//   - Map/Set cloned into EMPTY objects (entries lost = data loss)
//   - nested Date → string, TypedArray → plain object
//   - cyclic graphs → the ORIGINAL object returned (aliasing pollution)
//   - transfer list → manual detach, clone ≠ engine transfer map
// Every check asserts observable values (instanceof / identity / bytes), not
// typeof-only shapes.
//
// Single #[test] body (mozjs thread-singleton rule, same as bun_api_tests).

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<sc-e2e>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(JsValue::Null) => "null".to_string(),
        Ok(JsValue::Undefined) => "undefined".to_string(),
        Ok(JsValue::Object(_)) => "[object]".to_string(),
        Ok(_) => "[other]".to_string(),
        Err(e) => format!("ERROR:{}", e.message),
    }
}

/// All checks run in one eval so a mid-script throw is caught per-check.
const CHECKS: &str = r#"
globalThis.__r = {};
var results = [];
function check(name, fn) {
  try { results.push(name + ':' + (fn() ? 'PASS' : 'FAIL')); }
  catch (e) { results.push(name + ':ERROR:' + (e.message || e)); }
}

// ── primitives roundtrip ──
check('prim_number', function() { return structuredClone(42) === 42; });
check('prim_string', function() { return structuredClone('bao') === 'bao'; });
check('prim_bool', function() { return structuredClone(true) === true; });
check('prim_null', function() { return structuredClone(null) === null; });
check('prim_undefined', function() { return structuredClone(undefined) === undefined; });
check('prim_nan', function() { var v = structuredClone(NaN); return typeof v === 'number' && v !== v; });
check('prim_bigint', function() { return structuredClone(10n) === 10n; });

// ── plain object: deep copy, independent ──
check('plain_deep', function() {
  var o = { a: 1, inner: { b: 'x' } };
  var c = structuredClone(o);
  return c.a === 1 && c.inner.b === 'x' && c.inner !== o.inner && c !== o;
});

// ── Map: instanceof + entries preserved (old fallback cloned it to {}) ──
check('map_roundtrip', function() {
  var m = new Map([['k1', 'v1'], ['k2', 42], [7, 'numeric key']]);
  var c = structuredClone(m);
  return c instanceof Map && c !== m && c.size === 3 &&
         c.get('k1') === 'v1' && c.get('k2') === 42 && c.get(7) === 'numeric key';
});
check('map_nested_and_cyclic', function() {
  var inner = new Set([1, 2]);
  var m = new Map([['s', inner]]);
  m.set('self', m);
  var c = structuredClone(m);
  return c.get('s') instanceof Set && c.get('s').size === 2 &&
         c.get('self') === c; // cyclic identity preserved through the clone
});

// ── Set: instanceof + members (old fallback cloned it to {}) ──
check('set_roundtrip', function() {
  var s = new Set(['a', 'b', 'b']);
  var c = structuredClone(s);
  return c instanceof Set && c !== s && c.size === 2 && c.has('a') && c.has('b');
});

// ── cyclic object graph: identity preserved, no aliasing to the source ──
check('cyclic_identity', function() {
  var shared = { v: 1 };
  var o = { a: shared, b: shared };
  o.self = o;
  var c = structuredClone(o);
  return c.a === c.b &&          // shared reference cloned once, shared
         c.self === c &&         // cycle points at the CLONE
         c.self !== o &&         // no aliasing back to the source
         c.a.v === 1;
});
check('cyclic_array', function() {
  var arr = [1, 2];
  arr.push(arr);
  var c = structuredClone(arr);
  return c.length === 3 && c[2] === c && c[0] === 1;
});

// ── Date: top-level AND nested stay Date (old fallback stringified nested) ──
check('date_top', function() {
  var d = new Date(1234567890000);
  var c = structuredClone(d);
  return c instanceof Date && c.getTime() === d.getTime() && c !== d;
});
check('date_nested', function() {
  var o = { when: new Date(1234567890000) };
  var c = structuredClone(o);
  return c.when instanceof Date && c.when.getTime() === 1234567890000;
});

// ── RegExp: prototype + source + flags ──
check('regexp_flags', function() {
  var r = /bao-(\d+)/giy;
  var c = structuredClone(r);
  return c instanceof RegExp && c !== r &&
         c.source === r.source && c.flags === 'giy';
});

// ── TypedArray: prototype + byte-exact (old fallback → plain object) ──
check('u8_bytes', function() {
  var u = new Uint8Array([0, 1, 254, 255, 128]);
  var c = structuredClone(u);
  return c instanceof Uint8Array && c !== u && c.length === 5 &&
         c[0] === 0 && c[3] === 255 && c[4] === 128;
});
check('f64_bytes', function() {
  var f = new Float64Array([3.14, -0.0]);
  var c = structuredClone(f);
  return c instanceof Float64Array && c.length === 2 && c[0] === 3.14;
});

// ── ArrayBuffer clone (no transfer): real ArrayBuffer, DataView-usable ──
check('arraybuffer_clone', function() {
  var ab = new ArrayBuffer(4);
  new Uint8Array(ab).set([9, 8, 7, 6]);
  var c = structuredClone(ab);
  return c instanceof ArrayBuffer && c !== ab && c.byteLength === 4 &&
         new DataView(c).getUint8(0) === 9 && new Uint8Array(c)[3] === 6;
});

// ── transfer: ArrayBuffers are moved (source detached, clone gets bytes) ──
check('transfer_top', function() {
  var ab = new ArrayBuffer(8);
  new Uint8Array(ab).set([1, 2, 3, 4, 5, 6, 7, 254]);
  var c = structuredClone(ab, { transfer: [ab] });
  return c instanceof ArrayBuffer && c.byteLength === 8 &&
         new Uint8Array(c)[7] === 254 &&
         ab.byteLength === 0; // the source was detached by the engine transfer map
});
check('transfer_inside_object', function() {
  var ab = new ArrayBuffer(4);
  new Uint8Array(ab).set([7, 7, 7, 9]);
  var c = structuredClone({ buf: ab }, { transfer: [ab] });
  return c.buf instanceof ArrayBuffer && c.buf.byteLength === 4 &&
         new Uint8Array(c.buf)[3] === 9 && ab.byteLength === 0;
});
check('transfer_view_detached', function() {
  var ab = new ArrayBuffer(4);
  var view = new Uint8Array(ab);
  var c = structuredClone(ab, { transfer: [ab] });
  return c.byteLength === 4 && view.length === 0;
});

// ── non-clonable → DataCloneError (Node ERR_DATACLONE_ERROR shape) ──
check('function_throws', function() {
  try { structuredClone(function(){}); return false; }
  catch (e) { return (e.message || '').indexOf('could not be cloned') >= 0; }
});
check('nested_function_throws', function() {
  try { structuredClone({ cb: () => 1 }); return false; }
  catch (e) { return (e.message || '').indexOf('could not be cloned') >= 0; }
});
check('symbol_throws', function() {
  try { structuredClone(Symbol('x')); return false; }
  catch (e) { return (e.message || '').indexOf('could not be cloned') >= 0; }
});

// ── options shape: scalar transfer → TypeError, not silent ignore ──
check('transfer_scalar_typeerror', function() {
  try { structuredClone({a:1}, { transfer: 3 }); return false; }
  catch (e) { return e instanceof TypeError; }
});
check('transfer_arraylike_accepted', function() {
  // WebIDL sequence input: array-likes materialize into the transfer list.
  var ab = new ArrayBuffer(2);
  var list = { length: 1, 0: ab };
  var c = structuredClone(ab, { transfer: list });
  return c.byteLength === 2 && ab.byteLength === 0;
});

globalThis.__r.all = results.join('|');
"#;

#[test]
fn test_structured_clone_engine_fidelity() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext::for_test");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let all = eval_string(&mut ctx, CHECKS);
    for item in all.split('|') {
        assert!(
            item.ends_with(":PASS"),
            "structuredClone fidelity check failed: {}",
            item
        );
    }
    // Sanity: the battery actually ran every check (no silent empty result).
    let count = all.split('|').count();
    assert_eq!(count, 27, "expected 27 checks, got {}: {}", count, all);
}
