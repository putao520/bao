// @trace REQ-BRW-050 [criterion:editcontext-api-p0] live
// EditContext JS API — Chrome parity direct measurement (audit G2 closure).
//
// REQ-BRW-050 AC C4 (anti-fingerprint constitution A): the prototype face /
// getOwnPropertyNames must be indistinguishable from real Chrome, property
// by property. The existing c1/c3 locks assert individual members
// (`editContext` on HTMLElement.prototype, five on* attributes) but never
// compare the full enumeration against the real oracle — a drifted surface
// (a property silently dropped, or an extra Bao-only property injected)
// would pass them. This file pins the complete enumeration.
//
// The baseline below is a frozen fixture collected from real Chrome on this
// machine (no runtime dependency on Chrome — the arrays are constants):
//
//   Chrome 150.0.7871.128, 2026-10-14, headless=new:
//   /usr/bin/google-chrome --headless=new --disable-gpu --no-sandbox \
//     --dump-dom file:///tmp/e215-parity/probe.html
//   where probe.html runs Object.getOwnPropertyNames(<ctor>.prototype) for
//   the five EditContext API prototypes on about:blank and prints the JSON.
//
// Chrome declared all five prototypes (no face missing), in this order:
//   EditContext.prototype (17): text, selectionStart, selectionEnd,
//     characterBoundsRangeStart, ontextupdate, ontextformatupdate,
//     oncharacterboundsupdate, oncompositionstart, oncompositionend,
//     attachedElements, characterBounds, updateCharacterBounds,
//     updateControlBounds, updateSelection, updateSelectionBounds,
//     updateText, constructor
//   TextFormat.prototype (5): rangeStart, rangeEnd, underlineStyle,
//     underlineThickness, constructor
//   TextFormatUpdateEvent.prototype (2): getTextFormats, constructor
//   CharacterBoundsUpdateEvent.prototype (3): rangeStart, rangeEnd,
//     constructor
//   TextUpdateEvent.prototype (6): updateRangeStart, updateRangeEnd, text,
//     selectionStart, selectionEnd, constructor
//
// The comparison is a set equality per face (both sides sorted): an extra
// Bao-only property and a missing property are both red, so the test fails
// on drift in either direction. Enumeration ORDER is recorded here but not
// asserted (Chrome's declaration order comes from the WebIDL attribute
// order; bao's reflector may legitimately differ — only membership is a
// fingerprint surface).
//
// Gated like the rest of the live servo suite:
//   BAO_TEST_NETWORK=1 xvfb-run cargo nt -p bao-browser \
//     -E 'test(editcontext_parity)'

#![allow(dead_code)]

use std::sync::{Mutex, OnceLock};

use bao_browser::{BaoConfig, BrowserRuntime, PageConfig, PageHandle};

/// Chrome 150.0.7871.128 baseline — EditContext.prototype, declaration order.
const CHROME_EDITCONTEXT: &[&str] = &[
    "text",
    "selectionStart",
    "selectionEnd",
    "characterBoundsRangeStart",
    "ontextupdate",
    "ontextformatupdate",
    "oncharacterboundsupdate",
    "oncompositionstart",
    "oncompositionend",
    "attachedElements",
    "characterBounds",
    "updateCharacterBounds",
    "updateControlBounds",
    "updateSelection",
    "updateSelectionBounds",
    "updateText",
    "constructor",
];

/// Chrome 150.0.7871.128 baseline — TextFormat.prototype, declaration order.
const CHROME_TEXTFORMAT: &[&str] = &[
    "rangeStart",
    "rangeEnd",
    "underlineStyle",
    "underlineThickness",
    "constructor",
];

/// Chrome 150.0.7871.128 baseline — TextFormatUpdateEvent.prototype.
const CHROME_TEXTFORMATUPDATEEVENT: &[&str] = &["getTextFormats", "constructor"];

/// Chrome 150.0.7871.128 baseline — CharacterBoundsUpdateEvent.prototype.
const CHROME_CHARACTERBOUNDSUPDATEEVENT: &[&str] =
    &["rangeStart", "rangeEnd", "constructor"];

/// Chrome 150.0.7871.128 baseline — TextUpdateEvent.prototype.
const CHROME_TEXTUPDATEEVENT: &[&str] = &[
    "updateRangeStart",
    "updateRangeEnd",
    "text",
    "selectionStart",
    "selectionEnd",
    "constructor",
];

fn serializer() -> &'static Mutex<()> {
    static SERIALIZER: OnceLock<Mutex<()>> = OnceLock::new();
    SERIALIZER.get_or_init(|| Mutex::new(()))
}

fn should_skip() -> bool {
    if std::env::var("BAO_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!("[skip] BAO_TEST_NETWORK != 1");
        return true;
    }
    if std::env::var("DISPLAY").unwrap_or_default().is_empty() {
        eprintln!("[skip] no DISPLAY");
        return true;
    }
    false
}

/// Percent-encode an HTML document into a `data:text/html` URL (the parity
/// face needs no subresources — no network).
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
    use std::time::{Duration, Instant};
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

/// Returns the runtime alongside the page: `BrowserRuntime::drop` runs
/// `close_all()`, so the runtime must outlive every page interaction.
fn make_page(html: &str) -> (BrowserRuntime, PageHandle) {
    let runtime = BrowserRuntime::new(BaoConfig::default())
        .expect("gated live test: BrowserRuntime::new must succeed");
    let page = runtime
        .create_page(&PageConfig {
            url: Some(data_url(html)),
            ..Default::default()
        })
        .expect("gated live test: create_page must succeed");
    wait_for_load(&page, 10_000);
    (runtime, page)
}

/// @trace REQ-BRW-050 [criterion:editcontext-chrome-parity] live
///
/// C4 Chrome parity: the full getOwnPropertyNames enumeration of the five
/// EditContext API prototypes must equal the frozen Chrome 150 baseline
/// property-for-property (set equality, both directions asserted). A
/// missing property or an extra Bao-only property is red — either one is a
/// fingerprint surface a detection page can enumerate.
#[test]
fn editcontext_parity_chrome_prototype_enums() {
    if should_skip() {
        return;
    }
    let _guard = serializer().lock().unwrap_or_else(|e| e.into_inner());

    let (_runtime, page) =
        make_page(r#"<!DOCTYPE html><html><body><div id="t">seed</div></body></html>"#);
    let raw = page
        .evaluate_js_web(
            r#"
        (function() {
            var faces = ['EditContext', 'TextFormat', 'TextFormatUpdateEvent',
                         'CharacterBoundsUpdateEvent', 'TextUpdateEvent'];
            var out = [];
            for (var i = 0; i < faces.length; i++) {
                var ctor = window[faces[i]];
                if (typeof ctor !== 'function') {
                    out.push(faces[i] + '=MISSING');
                    continue;
                }
                out.push(faces[i] + '=' +
                    Object.getOwnPropertyNames(ctor.prototype).sort().join(','));
            }
            return out.join(';');
        })()
    "#,
        )
        .unwrap_or_default();
    let raw = raw.trim().trim_matches('"').to_string();
    eprintln!("[editcontext-parity] raw = {raw}");

    // Parse "Face=a,b,c;Face2=..." into face -> properties.
    let mut actual: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for entry in raw.split(';') {
        if entry.is_empty() {
            continue;
        }
        let (face, props) = entry
            .split_once('=')
            .unwrap_or_else(|| panic!("malformed parity entry: {entry:?}"));
        actual.insert(
            face.to_string(),
            props.split(',').map(str::to_string).collect(),
        );
    }

    let baselines: &[(&str, &[&str])] = &[
        ("EditContext", CHROME_EDITCONTEXT),
        ("TextFormat", CHROME_TEXTFORMAT),
        ("TextFormatUpdateEvent", CHROME_TEXTFORMATUPDATEEVENT),
        ("CharacterBoundsUpdateEvent", CHROME_CHARACTERBOUNDSUPDATEEVENT),
        ("TextUpdateEvent", CHROME_TEXTUPDATEEVENT),
    ];

    let mut failures: Vec<String> = Vec::new();
    for (face, expected) in baselines {
        let got = match actual.get(*face) {
            Some(v) => v,
            None => {
                failures.push(format!("{face}: prototype not evaluated (raw={raw})"));
                continue;
            }
        };
        let mut expected_sorted: Vec<&str> = expected.to_vec();
        expected_sorted.sort_unstable();
        let mut got_sorted: Vec<&str> = got.iter().map(String::as_str).collect();
        got_sorted.sort_unstable();
        let missing: Vec<&str> = expected_sorted
            .iter()
            .filter(|p| !got_sorted.contains(p))
            .copied()
            .collect();
        let extra: Vec<&str> = got_sorted
            .iter()
            .filter(|p| !expected_sorted.contains(p))
            .copied()
            .collect();
        if !missing.is_empty() || !extra.is_empty() {
            failures.push(format!(
                "{face}.prototype ({} Chrome properties): missing={missing:?} extra={extra:?}",
                expected.len()
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "EditContext prototype enumeration must match Chrome 150.0.7871.128 property-for-property \
         (anti-fingerprint constitution A). Drift found: {failures:?}; raw bao enumeration: {raw}"
    );
}
