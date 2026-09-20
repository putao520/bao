/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

// @trace REQ-ENG-012 [binding:EncodeStencil] — binding-level rehearsal for the
// XDR persistent cache (SM-EVOLUTION #26 stage 1): compile → JS::EncodeStencil
// (newly un-blacklisted) → byte buffer → JS::DecodeStencil → instantiate →
// execute, asserting execution equivalence between the freshly compiled and
// the decoded stencil. This is exactly the encode/decode surface the
// bao_engine persistent cache will sit on; the cache itself (REQ-ENG-012
// C1-C4) is a separate layer.

use std::ffi::CString;
use std::marker::PhantomData;
use std::os::raw::c_char;
use std::ptr;
use std::{mem, slice};

use mozjs::jsapi::{self, OnNewGlobalHookOption, TranscodeResult};
use mozjs::jsval::UndefinedValue;
use mozjs::realm::AutoRealm;
use mozjs::rooted;
use mozjs::rust::wrappers2;
use mozjs::rust::{
    CompileOptionsWrapper, HandleObject, JSEngine, RealmOptions, Runtime, SIMPLE_GLOBAL_CLASS,
    transform_str_to_source_text,
};

/// Closures + loops + a global binding + a guarded completion value, so a
/// mis-decoded stencil shows up as a wrong number, not as a crash.
const PAYLOAD_A: &str = r#"
var acc = 0;
function step(n) { acc = acc + n * 2; return acc; }
for (var i = 1; i <= 50; i++) { step(i); }
(acc === 2550) ? 2550 : -1
"#;

/// Second, structurally different source: guards against a vacuous decode
/// (returning the input stencil or any single cached script) passing.
const PAYLOAD_B: &str = r#"
(function() { var t = 0; for (var k = 1; k <= 8; k++) t += k; return t; })()
"#;

/// Embedder-side process build id. `JS::EncodeStencil`'s XDR version check
/// (StencilXdr.cpp VersionCheck → GetScriptTranscodingBuildId) invokes the
/// process BuildIdOp unconditionally; with no op installed SM calls a NULL
/// function pointer (SIGSEGV — upstream robustness gap, fail-open instead of
/// fail-closed). The persistent cache (REQ-ENG-012 C1-C4) installs the real
/// production tag; the binding rehearsal installs a deterministic fixture.
unsafe extern "C" fn bao_test_build_id(build_id: *mut jsapi::BuildIdCharVector) -> bool {
    const TAG: &[u8] = b"bao-xdr-smoke-1";
    mozjs::glue::SetBuildId(build_id, TAG.as_ptr() as *const c_char, TAG.len())
}

unsafe fn compile_to_stencil(
    cx: &mut mozjs::context::JSContext,
    filename: &str,
    src: &str,
) -> *mut jsapi::Stencil {
    let options = CompileOptionsWrapper::new(cx, CString::new(filename).unwrap(), 1);
    let mut source = transform_str_to_source_text(src);
    let addrefed = wrappers2::CompileGlobalScriptToStencil(cx, options.ptr, &mut source);
    let raw = addrefed.mRawPtr;
    assert!(!raw.is_null(), "CompileGlobalScriptToStencil failed");
    raw
}

/// REQ-ENG-012 encode half: stencil → opaque C++-owned TranscodeBuffer → bytes.
/// The buffer object itself never crosses into Rust beyond its opaque pointer.
unsafe fn encode_stencil(cx: &mozjs::context::JSContext, stencil: *mut jsapi::Stencil) -> Vec<u8> {
    let buffer = wrappers2::CreateTranscodeBuffer();
    assert!(!buffer.is_null(), "CreateTranscodeBuffer failed");
    assert!(
        wrappers2::TranscodeBufferLength(buffer) == 0,
        "fresh TranscodeBuffer must be empty"
    );

    let result = wrappers2::EncodeStencil(cx, stencil, buffer);
    assert_eq!(result, TranscodeResult::Ok, "EncodeStencil failed");

    let len = wrappers2::TranscodeBufferLength(buffer);
    assert!(len > 0, "encoded buffer must not be empty");
    let begin = wrappers2::TranscodeBufferBegin(buffer);
    assert!(!begin.is_null());
    let bytes = slice::from_raw_parts(begin, len).to_vec();
    wrappers2::DestroyTranscodeBuffer(buffer);
    bytes
}

/// REQ-ENG-012 decode half (already-bound surface, exercised here for the
/// round trip): bytes → TranscodeRange → stencil. borrowBuffer=false means the
/// decoded stencil owns its data — the byte buffer can be dropped afterwards,
/// which is the persistent-cache lifetime contract.
unsafe fn decode_stencil(cx: &mozjs::context::JSContext, bytes: &[u8]) -> *mut jsapi::Stencil {
    let options: jsapi::ReadOnlyDecodeOptions = mem::zeroed();
    let range = jsapi::TranscodeRange {
        _phantom_0: PhantomData,
        mStart: jsapi::mozilla::RangedPtr {
            _phantom_0: PhantomData,
            mPtr: bytes.as_ptr() as *mut u8,
        },
        mEnd: jsapi::mozilla::RangedPtr {
            _phantom_0: PhantomData,
            mPtr: (bytes.as_ptr() as *mut u8).add(bytes.len()),
        },
    };
    let mut decoded: *mut jsapi::Stencil = ptr::null_mut();
    let result = wrappers2::DecodeStencil(cx, &options, &range, &mut decoded);
    assert_eq!(result, TranscodeResult::Ok, "DecodeStencil failed");
    assert!(!decoded.is_null(), "DecodeStencil produced a null stencil");
    decoded
}

/// Instantiate `stencil` into a fresh realm of `glob` and run it to completion;
/// returns the completion value as an i32.
unsafe fn instantiate_and_execute(
    cx: &mut mozjs::context::JSContext,
    glob: HandleObject,
    stencil: *mut jsapi::Stencil,
) -> i32 {
    // C++ defaults (js/public/CompileOptions.h), same shape as every plain
    // JS::Evaluate compile.
    let inst_opts = jsapi::InstantiateOptions {
        skipFilenameValidation: false,
        hideScriptFromDebugger: false,
        deferDebugMetadata: false,
        eagerDelazificationStrategy_: jsapi::DelazificationOption::OnDemandOnly,
        // SM153.3: new TransitiveCompileOptions field (bindgen-regenerated);
        // mirrors the C++ default (CompileOptions.h:303 eagerBaselineStrategy_
        // = EagerBaselineOption::None).
        eagerBaselineStrategy_: jsapi::EagerBaselineOption::None,
    };

    let mut realm = AutoRealm::new_from_handle(cx, glob);
    let realm_cx: &mut mozjs::context::JSContext = &mut realm;

    rooted!(&in(realm_cx) let script =
        wrappers2::InstantiateGlobalStencil(realm_cx, &inst_opts, stencil, ptr::null_mut())
    );
    assert!(!script.get().is_null(), "InstantiateGlobalStencil failed");

    rooted!(&in(realm_cx) let mut rval = UndefinedValue());
    assert!(
        wrappers2::JS_ExecuteScript(realm_cx, script.handle(), rval.handle_mut()),
        "decoded-stencil script failed to execute"
    );
    rval.get().to_int32()
}

/// One JSEngine per process (mozjs per-process singleton): both payloads share
/// a single engine/runtime — a second `JSEngine::init` after the first
/// runtime's teardown fails with `AlreadyShutDown`.
fn roundtrip_case(
    cx: &mut mozjs::context::JSContext,
    global_a: HandleObject,
    global_b: HandleObject,
    src: &str,
    expected: i32,
    case: &str,
) {
    unsafe {
        // 1) Compile once — the persistent cache's "producer" step.
        let stencil = compile_to_stencil(cx, "<xdr-roundtrip>", src);

        // 2) Encode twice: the cache's stored payload, plus a determinism
        //    probe (same stencil, same bytes).
        let bytes1 = encode_stencil(cx, stencil);
        let bytes2 = encode_stencil(cx, stencil);
        assert_eq!(bytes1, bytes2, "{case}: XDR encoding must be deterministic");
        // Byte-buffer independence: decode from a moved copy (heap buffer the
        // cache would own), not from the C++ TranscodeBuffer.
        let stored = bytes1.clone();

        // 3) Decode — the cache's "consumer" step.
        let decoded = decode_stencil(cx, &stored);

        // 4) Execution equivalence: freshly compiled vs decoded, each
        //    instantiated into its own realm.
        let direct = instantiate_and_execute(cx, global_a, stencil);
        let from_xdr = instantiate_and_execute(cx, global_b, decoded);
        assert_eq!(direct, expected, "{case}: direct stencil completion value");
        assert_eq!(
            from_xdr, expected,
            "{case}: decoded (XDR) stencil completion value must be identical"
        );

        jsapi::StencilRelease(decoded);
        jsapi::StencilRelease(stencil);
    }
}

#[test]
fn stencil_xdr_encode_decode_roundtrip() {
    let engine = JSEngine::init().unwrap();
    let mut runtime = Runtime::new(engine.handle());
    let cx = runtime.cx();

    unsafe {
        // Encode's version check reads the process build id — without an op
        // installed SM dereferences a null fn pointer (see bao_test_build_id).
        jsapi::SetProcessBuildIdOp(Some(bao_test_build_id));

        let h_option = OnNewGlobalHookOption::FireOnNewGlobalHook;
        let c_option = RealmOptions::default();
        rooted!(&in(cx) let global_a = wrappers2::JS_NewGlobalObject(
            cx,
            &SIMPLE_GLOBAL_CLASS,
            ptr::null_mut(),
            h_option,
            &*c_option,
        ));
        rooted!(&in(cx) let global_b = wrappers2::JS_NewGlobalObject(
            cx,
            &SIMPLE_GLOBAL_CLASS,
            ptr::null_mut(),
            h_option,
            &*c_option,
        ));

        roundtrip_case(
            cx,
            global_a.handle(),
            global_b.handle(),
            PAYLOAD_A,
            2550,
            "payload_a",
        );
        roundtrip_case(
            cx,
            global_a.handle(),
            global_b.handle(),
            PAYLOAD_B,
            36,
            "payload_b",
        );
    }
}
