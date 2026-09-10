// @trace REQ-ENG-006 [api:Bun.concatArrayBuffers] [level:integration]
//
// GC rooting regression (SM-EVOLUTION #29 S2-续; same BCE class as the
// gc_store.rs dangling-nursery contract and the e4675351 vm registry fix —
// frame-level unrooted collection across a JS-running window):
// `bun_concat_array_buffers` walks the input once via JS_GetElement to fire
// getters and snapshot element identities. Each JS_GetElement can run a
// user-defined getter, which can GC. The elements used to be collected as
// bare `*mut JSObject` in a plain Vec — malloc memory the SpiderMonkey
// tracer never sees. Getter-returned buffers with no other referent were
// swept (or nursery-moved) mid-loop while the second sweep + memcpy pass
// kept dereferencing them: dangling reads, freed backing stores.
//
// The collector is now a RootedVec<Box<Heap<*mut JSObject>>> (servo
// structuredclone reader idiom): tracer-visible + barrier-pinned for the
// whole collection/snapshot/copy window.
//
// Both cases drive the REAL production path. Case 1 forces a deterministic
// FULL GC (Bun.gc → JS_GC API reason) inside every getter — exactly in the
// hazard window between two JS_GetElement calls. Case 2 uses large arrays
// and heavy allocation churn inside the getters to force natural major GCs
// across the window.

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<concat-gc-test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" } .to_string(),
        Ok(other) => panic!("unexpected eval result: {:?}", other),
        Err(e) => panic!("eval failed: {} ({}:{}:{})", e.message, e.filename, e.line, e.column),
    }
}

/// Getter-returned fresh Uint8Arrays (no other referent anywhere in the JS
/// heap) + a forced FULL GC inside every getter. Under the old bare-pointer
/// collector every previously collected element was unreachable from the
/// tracer's point of view and got swept here; the second sweep then read
/// freed objects / freed backing stores.
#[test]
fn test_concat_getter_forced_full_gc() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    // 8 elements × 32 bytes; element i's getter first forces a full GC (the
    // mid-sweep hazard window), then returns a patterned Uint8Array that
    // nothing else references.
    let out = eval_string(
        &mut ctx,
        r#"
        var list = { length: 8 };
        for (var i = 0; i < 8; i++) {
            (function (i) {
                Object.defineProperty(list, String(i), {
                    get: function () {
                        Bun.gc(); // full mark+sweep between JS_GetElement calls
                        var b = new Uint8Array(32);
                        for (var j = 0; j < 32; j++) b[j] = (i + j) & 0xff;
                        return b; // fresh, referenced only by the native collector
                    },
                    enumerable: true,
                    configurable: true,
                });
            })(i);
        }
        var out = Bun.concatArrayBuffers(list, undefined, true);
        var ok = out instanceof Uint8Array && out.byteLength === 8 * 32;
        for (var k = 0; k < out.length; k++) {
            var expect = (Math.floor(k / 32) + (k % 32)) & 0xff;
            if (out[k] !== expect) { ok = false; break; }
        }
        ok ? "OK" : "BAD len=" + out.byteLength + " first=" + out[0] + " k=" + k
        "#,
    );
    assert_eq!(out, "OK", "every byte must survive full GCs fired mid-sweep");
}

/// Large arrays + allocation churn inside the getters (no explicit gc()):
/// each getter allocates 4 MiB across fresh buffers before returning its
/// 64 KiB patterned element — multiple natural major GCs occur across the
/// collection window. Exercises the default (ArrayBuffer) output branch.
#[test]
fn test_concat_getter_allocation_storm() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let out = eval_string(
        &mut ctx,
        r#"
        var N = 128, ELEM = 64 * 1024;
        var list = { length: N };
        for (var i = 0; i < N; i++) {
            (function (i) {
                Object.defineProperty(list, String(i), {
                    get: function () {
                        // 4 MiB churn per getter — 512 MiB across the sweep.
                        var storm = [];
                        for (var j = 0; j < 16; j++) storm.push(new ArrayBuffer(256 * 1024));
                        storm = null;
                        var b = new Uint8Array(ELEM);
                        b[0] = i & 0xff;
                        b[1] = (i >> 8) & 0xff;
                        b[ELEM - 1] = 0xA5 ^ (i & 0xff);
                        return b;
                    },
                    enumerable: true,
                    configurable: true,
                });
            })(i);
        }
        var out = Bun.concatArrayBuffers(list); // default: ArrayBuffer
        var view = new Uint8Array(out);
        var ok = out instanceof ArrayBuffer && out.byteLength === N * ELEM;
        for (var i = 0; i < N; i++) {
            var base = i * ELEM;
            if (view[base] !== (i & 0xff)
                || view[base + 1] !== ((i >> 8) & 0xff)
                || view[base + ELEM - 1] !== (0xA5 ^ (i & 0xff))) {
                ok = false; break;
            }
        }
        ok ? "OK" : "BAD len=" + out.byteLength + " i=" + i
        "#,
    );
    assert_eq!(out, "OK", "patterned bytes must survive allocation-storm GCs");
}
