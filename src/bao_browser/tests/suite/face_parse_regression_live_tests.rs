// @trace REQ-BRW-050 [criterion:face-parse-regression-live] live
// e194 face-parse 类根修持久回归 — live e2e(镜像 /tmp/e190-facecrash-probe.py 行为真源).
//
// The face-parse class (e190 归因闭合): a static `<font face="">` +
// `getComputedStyle().fontFamily` panicked in stylo's
// `values/computed/font.rs` ("Family name with leading, trailing, or
// consecutive white spaces should have been marked quoted by the parser") —
// the synthesized presentational-hint declaration carried an `Identifiers`
// family name whose segments violate the identifier invariant. Root-fixed by
// 64088911c (three hunks in html/textual/htmlfontelement.rs): ① `get_face`
// empty atom → None (no declaration), ② `parse_face_attribute` drops empty
// segments, ③ whitespace-violating names are marked `Quoted` (Chromium
// serializes `<font face="a  b">` computed font-family as `"a  b"`).
//
// These pins hold the *behavioral* face on the live servo path (real page
// parse → real presentational hint → real computed style serialization):
//   1. `<font face="">` synthesizes NO font-family declaration —
//      computed fontFamily falls back to the initial value `serif`, with
//      zero panic (pre-fix this eval killed the Script thread; the eval
//      error/timeout here is the regression signal).
//   2. `<font face="a  b">` serializes the quoted form `"a  b"` (Chromium
//      parity — the double space survives inside the quotes).
//   3. The command face: `execCommand("fontName", false, "")` on a
//      contenteditable selection (the e190 A-layer producer path — typed
//      Atom attribute storage) reports queryCommandValue "serif" and
//      queryCommandIndeterm false, mirroring the pre-A-layer baseline.
//
// Gated like the rest of the live servo suite:
//   BAO_TEST_NETWORK=1 xvfb-run cargo nt -p bao-browser \
//     -E 'test(face_parse_regression_live_tests)'

#![allow(dead_code)]

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use bao_browser::{BaoConfig, BrowserRuntime, PageConfig, PageHandle};

fn serializer() -> &'static Mutex<()> {
    static SERIALIZER: OnceLock<Mutex<()>> = OnceLock::new();
    SERIALIZER.get_or_init(|| Mutex::new(()))
}

/// Percent-encode an HTML document into a `data:text/html` URL (no
/// subresources — the face-parse face needs no network). The `%20` form of
/// the attribute value decodes back to the literal two spaces the probe
/// exercises.
fn data_url(html: &str) -> String {
    let mut encoded = String::with_capacity(html.len() * 3);
    for b in html.bytes() {
        match b {
            b'#' => encoded.push_str("%23"),
            b'%' => encoded.push_str("%25"),
            b'\n' => encoded.push_str("%0A"),
            b'\r' => encoded.push_str("%0D"),
            b' ' => encoded.push_str("%20"),
            b'"' => encoded.push_str("%22"),
            b'\'' => encoded.push_str("%27"),
            _ => encoded.push(b as char),
        }
    }
    format!("data:text/html;charset=utf-8,{}", encoded)
}

fn wait_for_load(page: &PageHandle, max_ms: u64) {
    let start = Instant::now();
    while start.elapsed().as_millis() < max_ms as u128 {
        let _ = page.evaluate_js("");
        if page
            .evaluate_js("document.readyState")
            .map(|s| s.contains("complete") || s.contains("interactive"))
            .unwrap_or(false)
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Create a runtime + page. The runtime must outlive every `PageHandle`
/// use (dropping `BrowserRuntime` tears the pages down — "page is closed").
fn make_page(url: &str) -> (BrowserRuntime, PageHandle) {
    let runtime = BrowserRuntime::new(BaoConfig::default())
        .expect("gated live test: BrowserRuntime::new must succeed");
    let page = runtime
        .create_page(&PageConfig {
            url: Some(url.to_string()),
            ..Default::default()
        })
        .expect("gated live test: create_page must succeed");
    wait_for_load(&page, 10_000);
    (runtime, page)
}

/// The probe page: both static arms (`face=""` empty, `face="a  b"` double
/// space) plus the contenteditable host for the command arm — byte-for-byte
/// the /tmp/e190-facecrash-probe.py document shape.
const PROBE_PAGE_HTML: &str = r#"<!DOCTYPE html><html><body>
<font id="emptyface" face="">x</font>
<font id="dblspace" face="a  b">y</font>
<div contenteditable>foo</div>
</body></html>"#;

/// The probe script (IIFE — the webdriver probe's `return
/// JSON.stringify(out)` tail transcribed to an expression completion value).
/// Each arm is individually try/caught so a JS-side failure is *reported*
/// (err_* fields, asserted absent below) rather than silently skipped; the
/// face-parse panic class is NOT a JS exception — it kills the Script
/// thread, which surfaces as this eval failing/timing out, which the
/// `expect` below turns into the regression signal.
const PROBE_SCRIPT: &str = r#"(function () {
  const out = {};
  try {
    out.static_empty = getComputedStyle(document.querySelector('font#emptyface')).fontFamily;
    out.static_dblspace = getComputedStyle(document.querySelector('font#dblspace')).fontFamily;
  } catch (e) { out.err_static = String(e).slice(0, 80); }
  try {
    var div = document.querySelector('div[contenteditable]');
    var sel = getSelection();
    var r = document.createRange();
    r.selectNodeContents(div);
    sel.removeAllRanges(); sel.addRange(r);
    document.execCommand("styleWithCSS", false, "false");
    document.execCommand("fontName", false, "");
    out.cmd_innerHTML = div.innerHTML;
    out.cmd_value = document.queryCommandValue("fontName");
    out.cmd_indeterm = document.queryCommandIndeterm("fontName");
  } catch (e) { out.err_cmd = String(e).slice(0, 80); }
  return JSON.stringify(out);
})()"#;

/// @trace REQ-BRW-050 [criterion:face-parse-regression-live] live
///
/// The three e190/e191 green-account assertions on the live path. Any one
/// of them failing (or the eval itself erroring — the panic class face)
/// re-opens the face-parse defect class.
#[ignore = "需要 DISPLAY: servo WebRender GL-based(headless 亦需),scripts/local-smoke.sh:97 Xvfb :99 配方; 真跑: pgrep -x Xvfb || Xvfb :99 -screen 0 1280x720x24 -ac -nolisten tcp & 后 DISPLAY=:99 cargo nextest run --run-ignored=all -p bao-browser"]
#[test]
fn face_parse_static_and_command_regression_live() {
    let _guard = serializer().lock().unwrap_or_else(|e| e.into_inner());

    let (_runtime, page) = make_page(&data_url(PROBE_PAGE_HTML));

    // A face-parse panic class regression kills the Script thread mid-eval:
    // the result callback never fires and evaluate_js_web errors (15s
    // bounded spin) — the expect IS the zero-panic assertion.
    let result = page
        .evaluate_js_web(PROBE_SCRIPT)
        .expect("face-parse probe eval must succeed (a panic here is the font.rs:573 class \
                 regression — the Script thread died mid-eval)");
    eprintln!("[face-parse-e2e] probe result = {result}");

    // Zero-panic face part 2: no arm may report a JS-side error either.
    assert!(
        !result.contains("\"err_"),
        "neither probe arm may throw (err_* capture fields must stay absent): {result}"
    );

    // 断言 ① 静态 face="": no declaration synthesized — computed
    // font-family falls back to the initial value `serif`.
    assert!(
        result.contains("\"static_empty\":\"serif\""),
        "static <font face=\"\"> must synthesize no font-family declaration (computed fallback \
         is the initial `serif`); got: {result}"
    );

    // 断言 ② 静态 face="a  b": quoted serialization with the double space
    // preserved inside the quotes (Chromium parity; JSON-escaped form).
    assert!(
        result.contains("\"static_dblspace\":\"\\\"a  b\\\"\""),
        "static <font face=\"a  b\"> must serialize the quoted family name form \"a  b\" \
         (Chromium parity — whitespace-violating names are marked Quoted); got: {result}"
    );

    // 断言 ③ 命令面: execCommand("fontName","","") over a contenteditable
    // selection stores the typed empty-Atom attribute (e190 A layer) and the
    // query face reads value=serif + indeterm=false (pre-A-layer baseline
    // semantics, e191 green account).
    assert!(
        result.contains("\"cmd_value\":\"serif\""),
        "queryCommandValue(\"fontName\") after execCommand fontName \"\" must read serif; \
         got: {result}"
    );
    assert!(
        result.contains("\"cmd_indeterm\":false"),
        "queryCommandIndeterm(\"fontName\") after execCommand fontName \"\" must be false; \
         got: {result}"
    );
}
