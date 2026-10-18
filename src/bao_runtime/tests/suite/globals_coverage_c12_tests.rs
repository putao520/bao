// @trace TEST-ENG-005-C12 [req:REQ-ENG-005] [req:REQ-ENG-006] [level:integration]
// C12 line-coverage wave for src/bao_runtime/src/globals.rs — residual gap
// after C3 (fresh-lcov gap ≈ 1253 lines; Buffer family arms + structuredClone
// transfer + the REQ-SEC-002 scope-values install face).
//
// Characterization truth sources: local node v24.19.0 oracle (run outputs are
// embedded as comments at each expectation) and the bao source as the truth
// for faces where bao deliberately diverges (each divergence is a REGISTRY
// item below, pinned by the test, not fixed here).
//
// Branch map (source-anchored):
//   - buffer_from          : BE-encoding table arms (2750-2763), callable
//                            reject (2838-2845), String-wrapper ToString arm
//                            (2891-2903), ArrayBuffer byteOffset coercion
//                            arms NaN/{}/[1]/Infinity/overflow (2937-2993),
//                            Float16Array f16_to_f64 arm (3119-3131 +
//                            516-537), array-like length coercion
//                            (fractional/string/undefined 3257-3273), element
//                            coercion matrix (3314-3337), legacy blob
//                            {type:"Buffer",data} arm (3344-3399), throw
//                            fallback (3406-3413)
//   - base64_low_byte_decode (2467-2545): two-byte JSString low-byte arm +
//                            2-char/3-char padding decode arms
//   - buffer_to_string     : unknown-encoding throw arm (3605-3618)
//   - buffer_alloc         : boolean/undefined size arms, string size throw,
//                            MAX_BUFFER_SIZE RangeError (3676-3715)
//   - buffer_concat        : list-length double arm, totalLength negative/
//                            Infinity RangeError + string TypeError +
//                            zero-fill/truncate (3810-4023)
//   - buffer_slice         : ToInteger string/NaN/Infinity arms (4094-4176)
//   - buffer_copy          : no-target throw, targetStart/sourceStart
//                            ERR_OUT_OF_RANGE guards, offset write position,
//                            srcStart>=srcEnd 0 arm (4306-4475)
//   - buffer_equals/compare: non-object throws + memcmp faces (4564-4604,
//                            5249-5296)
//   - buffer_index_of_legacy (4904-5021): array-like `this` path with int
//                            needle, string needle, byteOffset, miss → -1,
//                            needle-longer-than-buffer → byteOffset arm
//   - buffer_byte_length   : no-arg throw, hex odd-length, ucs2 surrogate
//                            pair, base64 padding, latin1-vs-utf8, ArrayBuffer
//                            /DataView, plain-object/number throws (5061-5230)
//   - structured_clone_fn transfer arms (5734-5840): array transfer detaches,
//                            iterable (Set) drain arm, non-iterable TypeError
//   - create_node_api_scope_values / install_file_globals_on_target
//     (6568-6681): Rust-driven REQ-SEC-002 scope face
//
// REGISTRY (bao↔node v24 divergences and shadowed faces — characterization
// pins bao's behavior, product-side decision tracked separately; do NOT
// "fix" from the test):
//   R1. Buffer.from(str, "utf-16be"/"utf16be"): bao ACCEPTS and encodes
//       big-endian; node v24.19 throws ERR_UNKNOWN_ENCODING (only *le/ucs2
//       names are valid there). "ucs2be"/"ucs-2be" are NOT in bao's table
//       either (they throw like node).
//   R2. Buffer.concat accepts array-like lists ({length, 0:…}); node v24
//       requires a real Array (ERR_INVALID_ARG_TYPE).
//   R3. Buffer.prototype.indexOf/lastIndexOf accept non-typed-array
//       array-like `this` (legacy per-element path); node v24 throws.
//       R3b: the legacy path IGNORES the encoding argument — string needles
//       always encode as utf8 ("43" with "hex" does not find 0x43).
//   R4. Buffer.byteLength("abc","hex") = 2 in bao (dangling digit rounds
//       up); node v24 = 1 (complete pairs only).
//   R5. SHADOWED FACE: globals.rs crypto_subtle_digest (5478-5605) is
//       overwritten by the live WebCrypto install — crypto.subtle.digest in
//       the test realm is the async WebCrypto face returning an ArrayBuffer
//       (this file pins the LIVE face; the shadowed plain-object face is
//       unreachable-by-JS).
//   R6. Buffer.from(() => {}) throws a plain TypeError without
//       ERR_INVALID_ARG_TYPE code in bao; node carries the code.
//   R7. buf.copy(...) with a DETACHED source throws TypeError in bao; node
//       v24 returns 0 (post-detach length 0 → no-op copy).
//   R8. Buffer.compare(1, 2) throws a plain Error in bao (JS_ReportErrorUTF8
//       face); node v24 throws TypeError with ERR_INVALID_ARG_TYPE.
//       Buffer.equals(1,2) throws TypeError in both.

use bao_engine::context::JsContext;
use mozjs::rooted;
#[path = "common/mod.rs"]
mod common;

use common::eval_string_full as eval_string;
use common::setup_ctx;

fn make_ctx() -> JsContext {
    setup_ctx()
}

// ──────────────────── Buffer.from arms ────────────────────

/// REGISTRY R1 + BE-encode arms (2750-2763): bao accepts the utf-16be/
/// utf16be names node v24 rejects, and encodes true BE bytes (BMP + astral
/// surrogate-pair arm). "ucs-2be" is in NEITHER table → both throw.
#[test]
fn c12_buffer_from_be_encoding_table_arms() {
    let mut ctx = make_ctx();

    // node v24 oracle: Buffer.from("hi","utf-16be") → ERR_UNKNOWN_ENCODING.
    // bao accepts; "hi" BE = [0x00,0x68,0x00,0x69].
    assert_eq!(
        eval_string(&mut ctx, "Array.from(Buffer.from('hi','utf-16be')).join()"),
        "0,104,0,105",
        "utf-16be arm encodes big-endian (REGISTRY R1: node rejects this name)"
    );
    assert_eq!(
        eval_string(&mut ctx, "Array.from(Buffer.from('hi','utf16be')).join()"),
        "0,104,0,105",
        "utf16be alias arm"
    );
    let report = eval_string(
        &mut ctx,
        r#"
        var out = [];
        try { Buffer.from("hi", "ucs-2be"); out.push("ucs-2be=ok"); }
        catch (e) { out.push("ucs-2be=THREW:" + e.message); }
        out.join("|")
        "#,
    );
    assert_eq!(
        report,
        "ucs-2be=THREW:Unknown encoding: ucs-2be",
        "ucs-2be is outside bao's table too (node parity on this name)"
    );
    // Astral char → surrogate pair, BE order (U+1F600 → D83D DE00).
    assert_eq!(
        eval_string(
            &mut ctx,
            "Array.from(Buffer.from('\\u{1F600}','utf-16be')).join()"
        ),
        "216,61,222,0",
        "BE surrogate-pair arm for astral chars"
    );
}

/// ArrayBuffer byteOffset coercion matrix (2937-2993) — node v24 oracle:
/// NaN → 0, {} → 0, [1] → 1, Infinity → RangeError, offset>len → RangeError.
#[test]
fn c12_buffer_from_ab_offset_coercion_matrix() {
    let mut ctx = make_ctx();

    assert_eq!(
        eval_string(
            &mut ctx,
            r#"
            var ab = new ArrayBuffer(4); new Uint8Array(ab).set([1,2,3,4]);
            var out = [];
            out.push(Array.from(Buffer.from(ab, NaN)).join());
            out.push(Array.from(Buffer.from(ab, {})).join());
            out.push(Array.from(Buffer.from(ab, [1])).join());
            out.push(Array.from(Buffer.from(ab, 2)).join());
            var threw = "";
            try { Buffer.from(ab, Infinity); } catch (e) { threw = e.constructor.name; }
            out.push(threw);
            try { Buffer.from(ab, 100); } catch (e) { threw = e.constructor.name; }
            out.push(threw);
            out.join("|")
            "#,
        ),
        "1,2,3,4|1,2,3,4|2,3,4|3,4|RangeError|RangeError",
        "byteOffset: NaN and plain-object coerce to 0, [1] to 1, 2 to 2, Infinity/overflow RangeError (node oracle)"
    );
}

/// Callable input → TypeError (2838-2845); String wrapper object → ToString
/// recursion arm (2891-2903); primitive non-object non-string → throw
/// fallback (3406-3413).
#[test]
fn c12_buffer_from_callable_wrapper_and_fallback_throws() {
    let mut ctx = make_ctx();

    let report = eval_string(
        &mut ctx,
        r#"
        var out = [];
        function probe(label, fn) {
            try { out.push(label + "=" + fn()); }
            catch (e) { out.push(label + "=THREW:" + e.constructor.name); }
        }
        probe("callable", function () { return Array.from(Buffer.from(function () {})).join(); });
        probe("strobj", function () { return Array.from(Buffer.from(new String("test"))).join(); });
        probe("symbol", function () { return Array.from(Buffer.from(Symbol("s"))).join(); });
        probe("bigint", function () { return Array.from(Buffer.from(10n)).join(); });
        out.join("|")
        "#,
    );
    assert_eq!(
        report,
        "callable=THREW:TypeError|strobj=116,101,115,116|symbol=THREW:TypeError|bigint=THREW:TypeError",
        "callable/symbol/bigint throw; String wrapper ToString-recurses (node oracle: strobj=[116,101,115,116]; \
         REGISTRY R6: bao's TypeError carries no ERR_INVALID_ARG_TYPE code)"
    );
}

/// Array-like length coercion (3257-3273) + element coercion matrix
/// (3314-3337): fractional length floors, string length parses ("BAM" → 0),
/// undefined length throws, elements coerce per node's ToNumber→uint8 rules.
#[test]
fn c12_buffer_from_arraylike_length_and_elements() {
    let mut ctx = make_ctx();

    let report = eval_string(
        &mut ctx,
        r#"
        var out = [];
        function probe(label, fn) {
            try { out.push(label + "=" + fn()); }
            catch (e) { out.push(label + "=THREW:" + e.constructor.name); }
        }
        probe("frac", function () { return Buffer.from({ length: 3.3 }).length; });
        probe("strlen", function () { return Buffer.from({ length: "BAM" }).length; });
        probe("numstrlen", function () { return Buffer.from({ length: "7" }).length; });
        probe("undefinedLen", function () { return Buffer.from({}).length; });
        probe("elems", function () {
            return Array.from(Buffer.from({ 0: "0", 1: "1", 2: "2", 3: "3", length: 4 })).join();
        });
        probe("doubleElems", function () {
            return Array.from(Buffer.from({ 0: -1, 1: 257, 2: NaN, 3: Infinity, length: 4 })).join();
        });
        probe("boolElem", function () {
            return Array.from(Buffer.from({ 0: true, 1: false, length: 2 })).join();
        });
        probe("strElem", function () {
            return Array.from(Buffer.from({ 0: "abc", 1: " 66 ", length: 2 })).join();
        });
        probe("undefElem", function () {
            return Array.from(Buffer.from({ 0: undefined, 1: null, length: 2 })).join();
        });
        out.join("|")
        "#,
    );
    assert_eq!(
        report,
        "frac=3|strlen=0|numstrlen=7|undefinedLen=THREW:TypeError|elems=0,1,2,3|\
         doubleElems=255,1,0,0|boolElem=1,0|strElem=0,66|undefElem=0,0",
        "array-like length + element coercion (node oracle: 3.3→3, 'BAM'→0, missing length throws, \
         -1→255, 257→1, NaN/Infinity→0, true→1, 'abc'→0, ' 66 '→66, undefined/null→0)"
    );
}

/// Legacy Buffer blob shape {type:"Buffer", data:[...]} (3344-3399): data
/// elements clamp to bytes, and the JSON round-trip
/// (stringify → parse → Buffer.from) reconstructs the original bytes.
#[test]
fn c12_buffer_from_legacy_blob_shape() {
    let mut ctx = make_ctx();

    assert_eq!(
        eval_string(
            &mut ctx,
            "Array.from(Buffer.from({ type: 'Buffer', data: [102, 111, 300, NaN] })).join()"
        ),
        "102,111,44,0",
        "legacy blob data elements clamp (300→44, NaN→0)"
    );
    assert_eq!(
        eval_string(
            &mut ctx,
            "Buffer.from(JSON.parse(JSON.stringify({type:'Buffer',data:[65,66]}))).toString()"
        ),
        "AB",
        "parsed legacy blob reconstructs the bytes (node toJSON/parse round-trip parity)"
    );
}

/// Float16Array source → f16_to_f64 bit-decode arm (3119-3131 + 516-537):
/// half-float elements truncate toward a byte (node v24 oracle:
/// [1.5,-2.5,0] → [1,254,0]).
#[test]
fn c12_buffer_from_float16array_f16_decode() {
    let mut ctx = make_ctx();

    assert_eq!(
        eval_string(&mut ctx, "typeof Float16Array"),
        "function",
        "SM 153 exposes Float16Array (node v24 parity)"
    );
    assert_eq!(
        eval_string(
            &mut ctx,
            "Array.from(Buffer.from(new Float16Array([1.5, -2.5, 0]))).join()"
        ),
        "1,254,0",
        "Float16Array elements decode via f16 bits then truncate to bytes (node oracle)"
    );
}

/// base64 low-byte decode (2467-2545): two-byte JSString contributes the LOW
/// byte of each code unit; 2-char and 3-char inputs hit the padding arms.
#[test]
fn c12_buffer_from_base64_low_byte_and_padding() {
    let mut ctx = make_ctx();

    // node v24 oracle: Buffer.from("ŁYQ=","base64") → [1,132] (Ł low byte 0x41='A').
    assert_eq!(
        eval_string(&mut ctx, "Array.from(Buffer.from('\\u0141YQ=','base64')).join()"),
        "1,132",
        "two-byte JSString low-byte rule (node oracle)"
    );
    assert_eq!(
        eval_string(&mut ctx, "Array.from(Buffer.from('YQ','base64')).join()"),
        "97",
        "2-char input → 1 byte (missing c2/c3 arms)"
    );
    assert_eq!(
        eval_string(&mut ctx, "Array.from(Buffer.from('YWI','base64')).join()"),
        "97,98",
        "3-char input → 2 bytes (missing c3 arm)"
    );
    assert_eq!(
        eval_string(&mut ctx, "Buffer.from('aGVsbG8=','base64').toString()"),
        "hello",
        "canonical round-trip"
    );
}

// ──────────────────── toString / alloc / concat ────────────────────

/// Unknown toString encoding → TypeError "Unknown encoding: <name>"
/// (3605-3618); empty string falls back to utf8.
#[test]
fn c12_buffer_to_string_unknown_encoding_throws() {
    let mut ctx = make_ctx();

    let report = eval_string(
        &mut ctx,
        r#"
        var out = [];
        try { Buffer.from([104,105]).toString("bogus"); }
        catch (e) { out.push("THREW:" + e.message); }
        out.push(String(Buffer.from([104,105]).toString("") === "hi"));
        out.join("|")
        "#,
    );
    assert_eq!(
        report,
        "THREW:Unknown encoding: bogus|true",
        "explicit unknown encoding throws; empty encoding defaults to utf8"
    );
}

/// Buffer.alloc size arms (3676-3715): boolean→0/1, undefined/null→0,
/// string/object → TypeError, oversized → RangeError with the typed-array
/// ceiling message.
#[test]
fn c12_buffer_alloc_size_arms() {
    let mut ctx = make_ctx();

    let report = eval_string(
        &mut ctx,
        r#"
        var out = [];
        function probe(label, fn) {
            try { out.push(label + "=" + fn()); }
            catch (e) { out.push(label + "=THREW:" + e.constructor.name + ":" + e.message.slice(0, 30)); }
        }
        probe("true", function () { return Buffer.alloc(true).length; });
        probe("false", function () { return Buffer.alloc(false).length; });
        probe("undef", function () { return Buffer.alloc().length; });
        probe("str", function () { return Buffer.alloc("4").length; });
        probe("huge", function () { return Buffer.alloc(Math.pow(2, 53)).length; });
        out.join("|")
        "#,
    );
    assert_eq!(
        report,
        "true=1|false=0|undef=0|str=THREW:TypeError:The \"size\" argument must be of|\
         huge=THREW:RangeError:Typed arrays are currently lim",
        "alloc size coercion arms + ceiling RangeError"
    );
}

/// Buffer.concat totalLength matrix (3810-4023) — node v24 oracle:
/// zero-fill when total > sum, truncate when total < sum, negative/Infinity
/// → RangeError, string → TypeError. REGISTRY R2: bao also accepts
/// array-like lists which node rejects.
#[test]
fn c12_buffer_concat_total_length_matrix() {
    let mut ctx = make_ctx();

    let report = eval_string(
        &mut ctx,
        r#"
        var b = Buffer.alloc(2, 1);
        var out = [];
        function probe(label, fn) {
            try { out.push(label + "=" + fn()); }
            catch (e) { out.push(label + "=THREW:" + e.constructor.name); }
        }
        probe("zfill", function () { return Array.from(Buffer.concat([b], 5)).join(); });
        probe("trunc", function () { return Array.from(Buffer.concat([Buffer.alloc(4, 7)], 2)).join(); });
        probe("neg", function () { return Array.from(Buffer.concat([b], -1)).join(); });
        probe("inf", function () { return Array.from(Buffer.concat([b], Infinity)).join(); });
        probe("str", function () { return Array.from(Buffer.concat([b], "x")).join(); });
        probe("arraylike", function () { return Array.from(Buffer.concat({ length: 1, 0: b })).join(); });
        out.join("|")
        "#,
    );
    assert_eq!(
        report,
        "zfill=1,1,0,0,0|trunc=7,7|neg=THREW:RangeError|inf=THREW:RangeError|str=THREW:TypeError|\
         arraylike=1,1",
        "concat totalLength arms (node oracle); array-like acceptance is REGISTRY R2"
    );
}

// ──────────────────── slice / copy / equals / compare ────────────────────

/// slice ToInteger arms (4094-4176): string offsets, "-5", "111" (past end →
/// empty), 0/-0 (end<start clamp → empty), NaN → 0, Infinity arms.
/// node v24 oracle: [2,3,4,5] | [1,2,3,4,5] | [] | [] | [2,3].
#[test]
fn c12_buffer_slice_to_integer_arms() {
    let mut ctx = make_ctx();

    assert_eq!(
        eval_string(
            &mut ctx,
            r#"
            var buf = Buffer.from([1,2,3,4,5]);
            [
                Array.from(buf.slice("1")).join(),
                Array.from(buf.slice("-5","10")).join(),
                Array.from(buf.slice("111")).join(),
                Array.from(buf.slice(0,-0)).join(),
                Array.from(buf.slice("1","3")).join(),
                Array.from(buf.slice(NaN, 2)).join(),
                Array.from(buf.slice(-Infinity)).join()
            ].join("|")
            "#,
        ),
        "2,3,4,5|1,2,3,4,5|||2,3|1,2|1,2,3,4,5",
        "slice coercion arms match node ToInteger semantics"
    );
}

/// copy guards (4306-4475): missing target throws, negative targetStart →
/// ERR_OUT_OF_RANGE, sourceStart past source length → ERR_OUT_OF_RANGE,
/// targetStart positions the write, srcStart>=srcEnd → 0.
/// REGISTRY R7: a DETACHED source throws TypeError (node returns 0).
#[test]
fn c12_buffer_copy_range_guards() {
    let mut ctx = make_ctx();

    let report = eval_string(
        &mut ctx,
        r#"
        var out = [];
        function probe(label, fn) {
            try { out.push(label + "=" + fn()); }
            catch (e) { out.push(label + "=THREW:" + e.constructor.name + ":" + (e.code || "-")); }
        }
        var src = Buffer.from([1,2,3,4,5]);
        probe("noarg", function () { return src.copy(); });
        probe("negStart", function () { return src.copy(Buffer.alloc(4), -1); });
        probe("srcBig", function () { return src.copy(Buffer.alloc(4), 0, 100); });
        probe("offset", function () {
            var t = Buffer.alloc(5, 0);
            var n = src.copy(t, 2, 0, 3);
            return n + ":" + Array.from(t).join();
        });
        probe("emptyRange", function () { return src.copy(Buffer.alloc(4), 0, 3, 3); });
        probe("detached", function () {
            var ab = new ArrayBuffer(4);
            var u8 = new Uint8Array(ab);
            structuredClone({}, { transfer: [ab] });   // detaches ab
            return u8.copy(Buffer.alloc(4));
        });
        out.join("|")
        "#,
    );
    assert_eq!(
        report,
        "noarg=THREW:TypeError:-|negStart=THREW:RangeError:ERR_OUT_OF_RANGE|\
         srcBig=THREW:RangeError:ERR_OUT_OF_RANGE|offset=3:0,0,1,2,3|emptyRange=0|\
         detached=THREW:TypeError:-",
        "copy range guards + offset positioning; detached source throws (REGISTRY R7 — node returns 0)"
    );
}

/// equals/compare non-object throws (4564-4604 + 5249-5296) and the memcmp
/// faces: length-mismatch fast false, content equality.
/// REGISTRY R8: compare's non-object throw is a plain Error (no TypeError).
#[test]
fn c12_buffer_equals_compare_arms() {
    let mut ctx = make_ctx();

    let report = eval_string(
        &mut ctx,
        r#"
        var out = [];
        function probe(label, fn) {
            try { out.push(label + "=" + fn()); }
            catch (e) { out.push(label + "=THREW:" + e.constructor.name); }
        }
        probe("equalsNonObj", function () { return Buffer.equals(1, 2); });
        probe("compareNonObj", function () { return Buffer.compare(1, 2); });
        probe("eqLen", function () { return Buffer.from([1,2]).equals(Buffer.from([1,2,3])); });
        probe("eqTrue", function () { return Buffer.from([1,2]).equals(Buffer.from([1,2])); });
        probe("cmpOrder", function () { return Buffer.compare(Buffer.from([1]), Buffer.from([2])); });
        probe("cmpEqual", function () { return Buffer.compare(Buffer.from([9]), Buffer.from([9])); });
        out.join("|")
        "#,
    );
    assert_eq!(
        report,
        "equalsNonObj=THREW:TypeError|compareNonObj=THREW:Error|eqLen=false|eqTrue=true|\
         cmpOrder=-1|cmpEqual=0",
        "equals throws TypeError; compare throws plain Error (REGISTRY R8); memcmp faces"
    );
}

// ──────────────────── indexOf legacy path ────────────────────

/// REGISTRY R3/R3b — the legacy array-like indexOf path (4904-5021): int
/// needle, string needle (utf8 — the encoding argument is IGNORED, R3b),
/// byteOffset, miss → -1, and the needle-longer-than-buffer → byte_offset
/// arm. node v24 throws on a non-typed-array `this`; bao's legacy path
/// accepts it.
#[test]
fn c12_buffer_index_of_legacy_arraylike_arms() {
    let mut ctx = make_ctx();

    let report = eval_string(
        &mut ctx,
        r#"
        var out = [];
        function probe(label, fn) {
            try { out.push(label + "=" + fn()); }
            catch (e) { out.push(label + "=THREW:" + e.constructor.name); }
        }
        var arr = { 0: 65, 1: 66, 2: 67, length: 3 };  // "ABC"
        var idx = Buffer.prototype.indexOf;
        probe("intNeedle", function () { return idx.call(arr, 66); });
        probe("miss", function () { return idx.call(arr, 90); });
        probe("byteOffset", function () { return idx.call(arr, 65, 1); });
        probe("strNeedle", function () { return idx.call(arr, "BC"); });
        probe("needleLonger", function () { return idx.call(arr, "ABCD"); });
        probe("hexEncIgnored", function () { return idx.call(arr, "43", "hex"); });
        probe("ucs2EncIgnored", function () { return idx.call(arr, "B", "ucs2"); });
        out.join("|")
        "#,
    );
    assert_eq!(
        report,
        "intNeedle=1|miss=-1|byteOffset=-1|strNeedle=1|needleLonger=0|hexEncIgnored=-1|ucs2EncIgnored=1",
        "legacy indexOf: int/string utf8 needles, byteOffset, longer-needle → offset arm; \
         encoding arg ignored (REGISTRY R3/R3b — node throws on array-like this)"
    );
}

// ──────────────────── byteLength ────────────────────

/// byteLength matrix (5061-5230) — node v24 oracle: no-args throws,
/// "aaaa==" → 3, latin1 "Il était tué" → 12 vs utf8 → 14, ucs2 astral → 4,
/// ArrayBuffer(8) → 8, DataView counts, plain object / number throw.
/// REGISTRY R4: bao counts a dangling hex digit (abc → 2); node counts
/// complete pairs only (abc → 1).
#[test]
fn c12_buffer_byte_length_matrix() {
    let mut ctx = make_ctx();

    let report = eval_string(
        &mut ctx,
        r#"
        var out = [];
        function probe(label, fn) {
            try { out.push(label + "=" + fn()); }
            catch (e) { out.push(label + "=THREW:" + e.constructor.name); }
        }
        probe("noargs", function () { return Buffer.byteLength(); });
        probe("hexOdd", function () { return Buffer.byteLength("abc", "hex"); });
        probe("b64pad", function () { return Buffer.byteLength("aaaa==", "base64"); });
        probe("latin", function () { return Buffer.byteLength("Il était tué", "latin1"); });
        probe("utf8", function () { return Buffer.byteLength("Il était tué"); });
        probe("ucs2astral", function () { return Buffer.byteLength("\u{1F600}", "ucs2"); });
        probe("ab8", function () { return Buffer.byteLength(new ArrayBuffer(8)); });
        probe("dv", function () { return Buffer.byteLength(new DataView(new ArrayBuffer(6))); });
        probe("plainObj", function () { return Buffer.byteLength({}); });
        probe("number", function () { return Buffer.byteLength(42); });
        out.join("|")
        "#,
    );
    assert_eq!(
        report,
        "noargs=THREW:TypeError|hexOdd=2|b64pad=3|latin=12|utf8=14|ucs2astral=4|ab8=8|dv=6|\
         plainObj=THREW:TypeError|number=THREW:TypeError",
        "byteLength arms (node oracle values; hexOdd=2 is REGISTRY R4 — node says 1)"
    );
}

// ──────────────────── crypto.subtle.digest (live WebCrypto face) ────────────────────

/// The LIVE crypto.subtle.digest (REGISTRY R5 — globals.rs's plain-object
/// digest face is shadowed by the WebCrypto install): algorithm matrix
/// pinned against the well-known FIPS 180-4 digests of "abc" (node oracle),
/// unknown algorithm rejects, and the result is a real ArrayBuffer.
#[test]
fn c12_subtle_digest_algorithm_matrix() {
    let mut ctx = make_ctx();

    eval_string(
        &mut ctx,
        r#"
        globalThis.__digests = [];
        globalThis.__digestDone = false;
        function hex(ab) {
            return Array.from(new Uint8Array(ab)).map(function (b) {
                return ("0" + b.toString(16)).slice(-2);
            }).join("");
        }
        var jobs = [
            crypto.subtle.digest("SHA-256", new TextEncoder().encode("abc")),
            crypto.subtle.digest("sha-1", new TextEncoder().encode("abc")),
            crypto.subtle.digest("SHA-384", new TextEncoder().encode("abc")),
            crypto.subtle.digest("sha512", new TextEncoder().encode("abc")),
        ];
        Promise.all(jobs).then(function (results) {
            globalThis.__digests = results.map(hex);
            globalThis.__isAB = results.every(function (r) { return r instanceof ArrayBuffer; });
            crypto.subtle.digest("md5", "abc").then(
                function () { globalThis.__md5 = "no-reject"; },
                function (e) { globalThis.__md5 = "rejected"; }
            ).then(function () { globalThis.__digestDone = true; });
        });
        "#,
    );

    let done = common::wait_until_with(
        &mut ctx,
        r#"globalThis.__digestDone ? "y" : "n""#,
        60,
        400,
        |c, src| eval_string(c, src),
    );
    assert!(done, "async digests must settle");
    let digests = eval_string(&mut ctx, "globalThis.__digests.join('|')");
    assert_eq!(
        digests,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad|\
         a9993e364706816aba3e25717850c26c9cd0d89d|\
         cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7|\
         ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
        "digest values are the FIPS 180-4 vectors for 'abc' (node oracle)"
    );
    assert_eq!(
        eval_string(&mut ctx, "String(globalThis.__isAB)"),
        "true",
        "live face resolves ArrayBuffers (node parity)"
    );
    assert_eq!(
        eval_string(&mut ctx, "globalThis.__md5"),
        "rejected",
        "unknown algorithm rejects (WebCrypto notSupportedError face)"
    );
}

// ──────────────────── structuredClone transfer ────────────────────

/// structuredClone transfer arms (5734-5840): array transfer detaches the
/// source ArrayBuffer (node oracle: byteLength → 0), an iterable Set drains
/// through Symbol.iterator, a non-iterable transfer value throws TypeError.
#[test]
fn c12_structured_clone_transfer_arms() {
    let mut ctx = make_ctx();

    let report = eval_string(
        &mut ctx,
        r#"
        var out = [];
        var ab = new ArrayBuffer(4);
        var clone = structuredClone({ keep: ab }, { transfer: [ab] });
        out.push(String(clone.keep.byteLength) + ":" + String(ab.byteLength));
        var ab2 = new ArrayBuffer(2);
        structuredClone({}, { transfer: new Set([ab2]) });
        out.push(String(ab2.byteLength));
        try { structuredClone({}, { transfer: 42 }); out.push("noniter=no-throw"); }
        catch (e) { out.push("noniter=THREW:" + e.constructor.name); }
        out.join("|")
        "#,
    );
    assert_eq!(
        report,
        "4:0|0|noniter=THREW:TypeError",
        "transfer detaches (node oracle), Set drains via the iterable arm, non-iterable throws"
    );
}

// ──────────────────── REQ-SEC-002 scope-values install face ────────────────────

/// Rust-driven create_node_api_scope_values (6624-6681) +
/// install_file_globals_on_target (6568-6602): the randomized-name scope
/// object carries Bun/Bao/process/Buffer/module/require/__filename/__dirname,
/// is attached non-enumerably, and the global Buffer is preserved.
#[test]
fn c12_node_api_scope_values_rust_face() {
    let mut ctx = make_ctx();

    // Make sure the install ran (set_global_setup is lazy on first eval).
    assert_eq!(eval_string(&mut ctx, "typeof Buffer"), "function", "global install ran");

    // Fresh file globals for this test's thread (nextest isolates processes).
    bun_runtime::globals::set_file_globals(
        Some("/c12/app.js".to_string()),
        Some("/c12".to_string()),
    );

    let global = bao_engine::context::thread_realm_global().expect("persistent realm global");
    {
        let mut cxm = ctx.cx();
        // SAFETY: `global` outlives this block and points at the persistent
        // realm's global object; from_marked_location only re-handles it.
        let g = unsafe { mozjs::rust::Handle::from_marked_location(&global) };
        // The caller must have the realm ENTERED (production calls this from
        // servo's script-thread callback where the realm is current): the
        // body allocates plain objects, and JS_NewPlainObject resolves the
        // plain-object shape off CurrentGlobalOrNull — a bare test thread
        // with no AutoRealm SIGSEGVs there (crash class verified by gdb,
        // GlobalObject::getPlainObjectShapeWithDefaultProto on the null
        // global).
        rooted!(&in(cxm) let g_root = global);
        let mut realm = mozjs::realm::AutoRealm::new_from_handle(&mut cxm, g_root.handle());
        unsafe {
            bun_runtime::globals::create_node_api_scope_values(&mut realm, g, "__bao_c12_scope");
        }
    }

    let report = eval_string(
        &mut ctx,
        r#"
        var s = globalThis.__bao_c12_scope;
        var out = [];
        out.push(String(typeof s === "object" && s !== null));
        // Observed: the scope gets its OWN Bun object (populate_bun_object
        // builds a fresh instance per target) — functional parity, not
        // identity-shared with the global's Bun.
        out.push(String(typeof s.Bun === "object" && typeof s.Bun.listen === "function"));
        out.push(String(s.Bao === s.Bun));
        out.push(String(typeof s.process === "object" && typeof s.process.cwd === "function"));
        out.push(String(s.Buffer === globalThis.Buffer));
        out.push(String(typeof s.require === "function"));
        out.push(String(typeof s.module === "object"));
        out.push(String(s.__filename === "/c12/app.js"));
        out.push(String(s.__dirname === "/c12"));
        out.push(String(Object.keys(globalThis).indexOf("__bao_c12_scope") === -1));
        out.join("|")
        "#,
    );
    assert_eq!(
        report,
        "true|true|true|true|true|true|true|true|true|true",
        "scope object: identity-shared Bun/Buffer, live process, file globals from the \
         set_file_globals cache, non-enumerable attachment"
    );
}
