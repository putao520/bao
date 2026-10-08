// @trace REQ-BRW-050 [criterion:editcontext-api-p0] live
// EditContext JS API (W3C EditContext API-1, Bao fork self-build) — four
// locks over the real servo engine:
//   C1 constructor surface (EditContext/TextFormat/TextUpdateEvent/
//      TextFormatUpdateEvent dictionary init + defaults + enum validation)
//   C2 HTMLElement.editContext association (roundtrip, single-element
//      invariant, detach, TypeError/NotSupportedError classes)
//   C3 event dispatch (constructed TextUpdateEvent/TextFormatUpdateEvent
//      through dispatchEvent, handler attributes, characterBounds state)
//   C4 driver interop: real trusted keyboard delivery into a focused
//      EditContext element fires beforeinput + textupdate and performs NO
//      DOM mutation (the defining EditContext contract).
//
// Gated like the rest of the live servo suite:
//   BAO_TEST_NETWORK=1 xvfb-run cargo nt -p bao-browser \
//     -E 'test(editcontext_api)'

#![allow(dead_code)]

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use bao_browser::{BaoConfig, BrowserRuntime, PageConfig, PageHandle};
use servo::{Code, Key, KeyState, Location, Modifiers};

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

/// Percent-encode an HTML document into a `data:text/html` URL (no
/// subresources — the EditContext face needs no network).
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

fn type_char(page: &PageHandle, ch: char) {
    let key = Key::Character(ch.to_string());
    page.dispatch_key_event_full(
        KeyState::Down,
        key.clone(),
        Code::Unidentified,
        Location::Standard,
        Modifiers::empty(),
        false,
    );
    page.dispatch_key_event_full(
        KeyState::Up,
        key,
        Code::Unidentified,
        Location::Standard,
        Modifiers::empty(),
        false,
    );
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

fn poll_until_contains(page: &PageHandle, js: &str, target: &str, max_ms: u64) -> String {
    let start = Instant::now();
    let mut last = String::new();
    while start.elapsed().as_millis() < max_ms as u128 {
        if let Ok(s) = page.evaluate_js_web(js) {
            if s.contains(target) {
                return s;
            }
            last = s;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    last
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

/// @trace REQ-BRW-050 [criterion:editcontext-c1-constructor] live
///
/// C1: the constructor surface — EditContext dictionary init and defaults,
/// TextFormat defaults and enum validation, TextUpdateEvent /
/// TextFormatUpdateEvent dictionary init, and the updateText /
/// updateSelection / characterBounds state machines including reversed
/// ranges and the cached-bounds snapshot contract.
#[test]
fn editcontext_api_c1_constructor_surface() {
    if should_skip() {
        return;
    }
    let _guard = serializer().lock().unwrap_or_else(|e| e.into_inner());

    let (_runtime, page) =
        make_page(r#"<!DOCTYPE html><html><body><div id="t">seed</div></body></html>"#);
    let result = page.evaluate_js_web(
        r#"
        var out = [];
        function check(name, cond) { out.push((cond ? 'ok:' : 'FAIL:') + name); }

        // EditContext dictionary init.
        var ec = new EditContext({text: 'Hello world', selectionStart: 11, selectionEnd: 11});
        check('init.text', ec.text === 'Hello world');
        check('init.selectionStart', ec.selectionStart === 11);
        check('init.selectionEnd', ec.selectionEnd === 11);

        // EditContext defaults.
        var d = new EditContext();
        check('default.text', d.text === '');
        check('default.selectionStart', d.selectionStart === 0);
        check('default.selectionEnd', d.selectionEnd === 0);
        check('default.characterBoundsRangeStart', d.characterBoundsRangeStart === 0);
        check('default.characterBounds', d.characterBounds().length === 0);
        check('default.attachedElements', d.attachedElements().length === 0);
        check('exposed.onHTMLElement', 'editContext' in HTMLElement.prototype);

        // updateText semantics (spec: replace the substring; reversed ranges
        // swap; out-of-bounds clamp).
        var u = new EditContext();
        u.updateText(0, 3, 'foo'); check('updateText.empty', u.text === 'foo');
        u.updateText(6, 0, 'abcdef'); check('updateText.grow', u.text === 'abcdef');
        u.updateText(2, 5, 'ghi'); check('updateText.replace', u.text === 'abghif');
        u.updateText(5, 2, 'jkl'); check('updateText.reversed', u.text === 'abjklf');

        // updateSelection accepts backwards selections verbatim.
        u.updateSelection(1, 0);
        check('updateSelection.backwards', u.selectionStart === 1 && u.selectionEnd === 0);

        // TextFormat defaults and enum validation (TypeError on invalid).
        var tf = new TextFormat();
        check('tf.default.range', tf.rangeStart === 0 && tf.rangeEnd === 0);
        check('tf.default.style', tf.underlineStyle === 'none');
        check('tf.default.thickness', tf.underlineThickness === 'none');
        var tf2 = new TextFormat({rangeStart: 2, rangeEnd: 4, underlineStyle: 'wavy', underlineThickness: 'thick'});
        check('tf.init', tf2.rangeStart === 2 && tf2.rangeEnd === 4 &&
                         tf2.underlineStyle === 'wavy' && tf2.underlineThickness === 'thick');
        try { new TextFormat({underlineStyle: 'Solid'}); check('tf.invalidStyle', false); }
        catch (e) { check('tf.invalidStyle', e instanceof TypeError); }
        try { new TextFormat({underlineThickness: 'Thick'}); check('tf.invalidThickness', false); }
        catch (e) { check('tf.invalidThickness', e instanceof TypeError); }

        // TextUpdateEvent dictionary init.
        var tue = new TextUpdateEvent('x', {updateRangeStart: 1, updateRangeEnd: 3,
                                             text: 'ab', selectionStart: 3, selectionEnd: 5});
        check('tue.fields', tue.updateRangeStart === 1 && tue.updateRangeEnd === 3 &&
                            tue.text === 'ab' && tue.selectionStart === 3 && tue.selectionEnd === 5);

        // TextFormatUpdateEvent dictionary init.
        var tfue = new TextFormatUpdateEvent('y', {textFormats: [tf2]});
        var got = tfue.getTextFormats();
        check('tfue.getTextFormats', got.length === 1 && got[0].underlineStyle === 'wavy');

        // updateCharacterBounds snapshot contract: mutating the source rect
        // after the call must not change the cached values.
        var rect1 = DOMRect.fromRect({x: 0, y: 1, width: 100, height: 200});
        var rect2 = DOMRect.fromRect({x: 2, y: 3, width: 300, height: 400});
        var b = new EditContext();
        b.updateCharacterBounds(2, [rect1, rect2]);
        check('bounds.rangeStart', b.characterBoundsRangeStart === 2);
        rect2.x = 100;
        var arr = b.characterBounds();
        check('bounds.length', arr.length === 2);
        check('bounds.snapshot', arr[1].x === 2 && arr[1].y === 3 &&
                                 arr[1].width === 300 && arr[1].height === 400);
        // TypeError classes for non-DOMRect arguments.
        try { b.updateControlBounds(42); check('bounds.controlThrows', false); }
        catch (e) { check('bounds.controlThrows', e instanceof TypeError); }
        b.updateControlBounds(rect1); b.updateSelectionBounds(rect1);

        out.join('|')
    "#,
    )
    .unwrap_or_default();

    let result = result.trim().trim_matches('"').to_string();
    eprintln!("[editcontext-c1] {result}");
    let fails: Vec<&str> = result
        .split('|')
        .filter(|entry| entry.starts_with("FAIL:"))
        .collect();
    assert!(
        result.contains("ok:init.text") && fails.is_empty(),
        "C1 constructor surface must pass (fails={fails:?}, raw={result})"
    );
}

/// @trace REQ-BRW-050 [criterion:editcontext-c2-association] live
///
/// C2: the HTMLElement.editContext association algorithm — roundtrip on and
/// off the tree, the single-element invariant (NotSupportedError), the
/// TypeError class for non-EditContext values, and the NotSupportedError
/// class for disallowed elements.
#[test]
fn editcontext_api_c2_association() {
    if should_skip() {
        return;
    }
    let _guard = serializer().lock().unwrap_or_else(|e| e.into_inner());

    let (_runtime, page) =
        make_page(r#"<!DOCTYPE html><html><body><div id="t"></div></body></html>"#);
    let result = page.evaluate_js_web(
        r#"
        var out = [];
        function check(name, cond) { out.push((cond ? 'ok:' : 'FAIL:') + name); }

        // Association roundtrip on a disconnected element.
        var ec = new EditContext();
        var div = document.createElement('div');
        check('assoc.initiallyNull', div.editContext === null);
        div.editContext = ec;
        check('assoc.roundtrip', div.editContext === ec);
        check('assoc.attached', ec.attachedElements().length === 1 &&
                                ec.attachedElements()[0] === div);

        // Association survives tree removal (never auto-detached).
        document.body.appendChild(div);
        document.body.removeChild(div);
        check('assoc.survivesRemoval', div.editContext === null ? false : ec.attachedElements()[0] === div);

        // Detach via null.
        div.editContext = null;
        check('assoc.detach', ec.attachedElements().length === 0 && div.editContext === null);

        // Single-element invariant.
        var ec1 = new EditContext();
        var a = document.createElement('div');
        var b = document.createElement('div');
        a.editContext = ec1;
        var threw = null;
        try { b.editContext = ec1; } catch (e) { threw = e; }
        check('assoc.doubleAttach', threw instanceof DOMException &&
                                    thrown_name(threw) === 'NotSupportedError');
        check('assoc.firstUnchanged', a.editContext === ec1 && b.editContext === null);

        // Direct switch + same-value no-op.
        var ec2 = new EditContext();
        a.editContext = ec2;
        check('assoc.switch', a.editContext === ec2 && ec1.attachedElements().length === 0);
        a.editContext = ec2;
        check('assoc.noop', a.editContext === ec2);

        // TypeError for non-EditContext values.
        try { document.createElement('div').editContext = 'hello'; check('assoc.typeError', false); }
        catch (e) { check('assoc.typeError', e instanceof TypeError); }
        try { document.createElement('div').editContext = 42; check('assoc.typeError2', false); }
        catch (e) { check('assoc.typeError2', e instanceof TypeError); }

        // NotSupportedError for disallowed elements (input is not a valid
        // shadow host name and not canvas); the getter stays null.
        var input = document.createElement('input');
        var threwNS = null;
        try { input.editContext = new EditContext(); } catch (e) { threwNS = e; }
        check('assoc.disallowed', threwNS instanceof DOMException &&
                                  thrown_name(threwNS) === 'NotSupportedError');
        check('assoc.disallowedNull', input.editContext === null);

        // Allowed list spot checks (canvas and body allowed).
        var canvas = document.createElement('canvas');
        var ecc = new EditContext();
        canvas.editContext = ecc;
        check('assoc.canvasAllowed', canvas.editContext === ecc);

        function thrown_name(e) { return e.name; }
        out.join('|')
    "#,
    )
    .unwrap_or_default();

    let result = result.trim().trim_matches('"').to_string();
    eprintln!("[editcontext-c2] {result}");
    let fails: Vec<&str> = result
        .split('|')
        .filter(|entry| entry.starts_with("FAIL:"))
        .collect();
    assert!(
        result.contains("ok:assoc.roundtrip") && fails.is_empty(),
        "C2 association must pass (fails={fails:?}, raw={result})"
    );
}

/// @trace REQ-BRW-050 [criterion:editcontext-c3-event-dispatch] live
///
/// C3: event dispatch — constructed TextUpdateEvent/TextFormatUpdateEvent
/// flow through `dispatchEvent` and the `ontextupdate` handler attribute,
/// and the on* handler attributes exist on the prototype surface.
#[test]
fn editcontext_api_c3_event_dispatch() {
    if should_skip() {
        return;
    }
    let _guard = serializer().lock().unwrap_or_else(|e| e.into_inner());

    let (_runtime, page) =
        make_page(r#"<!DOCTYPE html><html><body><div id="t"></div></body></html>"#);
    let result = page.evaluate_js_web(
        r#"
        var out = [];
        function check(name, cond) { out.push((cond ? 'ok:' : 'FAIL:') + name); }
        var ec = new EditContext();

        // Handler attributes exist on the prototype (Chromium parity: five
        // on* attributes on EditContext.prototype).
        ['ontextupdate', 'ontextformatupdate', 'oncharacterboundsupdate',
         'oncompositionstart', 'oncompositionend'].forEach(function(name) {
            check('proto.' + name, name in EditContext.prototype);
        });

        // TextUpdateEvent through addEventListener.
        var seen = null;
        ec.addEventListener('textupdate', function(e) {
            seen = {t: e.type, rs: e.updateRangeStart, re: e.updateRangeEnd,
                    text: e.text, ss: e.selectionStart, se: e.selectionEnd};
        });
        ec.dispatchEvent(new TextUpdateEvent('textupdate', {updateRangeStart: 1, updateRangeEnd: 2,
                                                            text: 'q', selectionStart: 2, selectionEnd: 2}));
        check('dispatch.textupdate', seen && seen.t === 'textupdate' && seen.rs === 1 &&
                                     seen.re === 2 && seen.text === 'q' &&
                                     seen.ss === 2 && seen.se === 2);

        // ontextupdate handler attribute roundtrip.
        var viaAttr = null;
        ec.ontextupdate = function(e) { viaAttr = e.text; };
        ec.dispatchEvent(new TextUpdateEvent('textupdate', {text: 'zz'}));
        check('dispatch.onAttribute', viaAttr === 'zz');

        // TextFormatUpdateEvent through addEventListener.
        var formats = null;
        ec.addEventListener('textformatupdate', function(e) { formats = e.getTextFormats(); });
        ec.dispatchEvent(new TextFormatUpdateEvent('textformatupdate',
            {textFormats: [new TextFormat({underlineStyle: 'dotted'})]}));
        check('dispatch.textformatupdate', formats && formats.length === 1 &&
                                           formats[0].underlineStyle === 'dotted');

        out.join('|')
    "#,
    )
    .unwrap_or_default();

    let result = result.trim().trim_matches('"').to_string();
    eprintln!("[editcontext-c3] {result}");
    let fails: Vec<&str> = result
        .split('|')
        .filter(|entry| entry.starts_with("FAIL:"))
        .collect();
    assert!(
        result.contains("ok:dispatch.textupdate") && fails.is_empty(),
        "C3 event dispatch must pass (fails={fails:?}, raw={result})"
    );
}

/// @trace REQ-BRW-050 [criterion:editcontext-c4-driver-interop] live
///
/// C4: driver interop — real trusted keyboard delivery into a focused
/// element with an attached EditContext fires `beforeinput` (cancelable,
/// insertText / deleteContentBackward / deleteContentForward) then
/// `textupdate` carrying the UTF-16 range and new selection, and the DOM is
/// never mutated. Backspace/Delete run the collapsed-deletion state machine,
/// and canceling `beforeinput` suppresses the `textupdate`.
#[test]
fn editcontext_api_c4_driver_interop() {
    if should_skip() {
        return;
    }
    let _guard = serializer().lock().unwrap_or_else(|e| e.into_inner());

    let (_runtime, page) = make_page(
        r#"<!DOCTYPE html><html><body>
<div id="t" style="width:100px;height:20px;"></div>
<script>
window.__log = [];
window.__ec = new EditContext();
var div = document.getElementById('t');
div.addEventListener('beforeinput', function(e) {
    window.__log.push('bi:' + e.inputType + ':trusted=' + e.isTrusted);
});
window.__ec.addEventListener('textupdate', function(e) {
    window.__log.push('tu:' + e.updateRangeStart + ':' + e.updateRangeEnd + ':' +
                      e.text + ':' + e.selectionStart + ':' + e.selectionEnd + ':trusted=' + e.isTrusted);
});
div.editContext = window.__ec;
div.focus();
window.__focused = (document.activeElement === div);
</script></body></html>"#,
    );

    let focused = page
        .evaluate_js_web("String(window.__focused)")
        .unwrap_or_default();
    assert!(
        focused.trim().trim_matches('"') == "true",
        "an element with an attached EditContext must be focusable (activeElement={focused})"
    );

    // Insert 'a' at the collapsed start selection.
    type_char(&page, 'a');
    let state = poll_until_contains(&page, "window.__ec.text", "a", 10_000);
    eprintln!("[editcontext-c4] after 'a': text={state}");

    // Insert 'b' (caret after 'a').
    type_char(&page, 'b');
    poll_until_contains(&page, "window.__ec.text", "ab", 10_000);

    // Backspace removes 'b' (deleteContentBackward, range [1,2), caret 1).
    let key = Key::Named(servo::NamedKey::Backspace);
    page.dispatch_key_event_full(
        KeyState::Down,
        key.clone(),
        Code::Backspace,
        Location::Standard,
        Modifiers::empty(),
        false,
    );
    page.dispatch_key_event_full(
        KeyState::Up,
        key,
        Code::Backspace,
        Location::Standard,
        Modifiers::empty(),
        false,
    );
    poll_until_contains(&page, "window.__log.join('\\n')", "bi:deleteContentBackward", 10_000);

    // Re-insert 'c', then Delete-forward is a no-op at the end of text.
    type_char(&page, 'c');
    poll_until_contains(&page, "window.__ec.text", "ac", 10_000);
    let key = Key::Named(servo::NamedKey::Delete);
    page.dispatch_key_event_full(
        KeyState::Down,
        key.clone(),
        Code::Delete,
        Location::Standard,
        Modifiers::empty(),
        false,
    );
    page.dispatch_key_event_full(
        KeyState::Up,
        key,
        Code::Delete,
        Location::Standard,
        Modifiers::empty(),
        false,
    );

    // Pump until the deleteContentForward beforeinput lands, then assert.
    poll_until_contains(
        &page,
        "window.__log.join('\n')",
        "bi:deleteContentForward",
        10_000,
    );

    // Final assertions: run every C4 contract check in the page realm and
    // surface the joined result string.
    let checks = page
        .evaluate_js_web(
            r#"
        (function() {
            var ec = window.__ec;
            var log = window.__log.join('\n');
            var checks = [];
            function check(name, cond) { checks.push((cond ? 'ok:' : 'FAIL:') + name); }
            check('text', ec.text === 'ac');
            check('selection', ec.selectionStart === 2 && ec.selectionEnd === 2);
            check('noDomMutation', document.getElementById('t').innerHTML === '' &&
                                   document.getElementById('t').textContent === '');
            check('bi.insertText', log.indexOf('bi:insertText:trusted=true') !== -1);
            check('tu.insert', log.indexOf('tu:0:0:a:1:1:trusted=true') !== -1);
            check('tu.append', log.indexOf('tu:1:1:b:2:2:trusted=true') !== -1);
            check('bi.backspace', log.indexOf('bi:deleteContentBackward:trusted=true') !== -1);
            check('tu.backspace', log.indexOf('tu:1:2::1:1:trusted=true') !== -1);
            check('bi.delete', log.indexOf('bi:deleteContentForward:trusted=true') !== -1);
            // Four textupdates total: insert 'a', append 'b',
            // delete-backward of 'b', re-insert 'c'. The delete-forward at
            // the end of text is consumed but fires none.
            check('tu.count', log.split('tu:').length - 1 === 4);
            return checks.join('|');
        })()
        "#,
        )
        .unwrap_or_default();
    let checks = checks.trim().trim_matches('"').to_string();
    eprintln!("[editcontext-c4] checks = {checks}");
    let fails: Vec<&str> = checks
        .split('|')
        .filter(|entry| entry.starts_with("FAIL:"))
        .collect();
    assert!(
        checks.contains("ok:noDomMutation") && fails.is_empty(),
        "C4 driver interop must pass (fails={fails:?}, raw={checks})"
    );

    // Canceling beforeinput must suppress the textupdate (spec contract).
    page.evaluate_js_web(
        "window.__log = []; document.getElementById('t').addEventListener('beforeinput', function(e) { if (e.inputType === 'insertText') e.preventDefault(); });",
    )
    .expect("cancel-beforeinput wiring must eval");
    type_char(&page, 'z');
    std::thread::sleep(Duration::from_millis(300));
    let cancel_state = page
        .evaluate_js_web(
            "JSON.stringify({text: window.__ec.text, log: window.__log.join(',')})",
        )
        .unwrap_or_default();
    eprintln!("[editcontext-c4] cancel state = {cancel_state}");
    assert!(
        !cancel_state.contains("tu:"),
        "canceling beforeinput must suppress textupdate (state={cancel_state})"
    );
    assert!(
        !cancel_state.contains("z"),
        "canceling beforeinput must leave text unchanged (state={cancel_state})"
    );
}
