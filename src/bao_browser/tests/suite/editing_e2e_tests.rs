// @trace REQ-BRW-002 [criterion:editing-behavior-surface] live
// e150 editing campaign e2e pins — real key delivery → DOM editing semantics.
//
// The editing engine (vendor EditingContext + execCommand 25 commands +
// selection) reached upstream-terminal state with the 2026-10-06 snapshot
// swap (e149 recon §1.3); these pins hold the *driving* face observable:
// real servo keyboard delivery into focused editables must run the real
// text-input engine (character insertion, Enter paragraph semantics,
// clipboard-action guards) — not a JS shortcut.
//
// Covered:
//   - Key::Character Down/Up pairs (the Input.insertText carrier,
//     cdp_handler::cmd_insert_text) insert text into contenteditable /
//     <input> / <textarea>; the DOM mutation is observable to page script
//     and the accompanying keydown/input events are trusted.
//   - Enter (NamedKey) drives the InsertParagraph editing action.
//   - Ctrl+C / Ctrl+X on a password field are blocked by the
//     copying_enabled/cutting_enabled guards (57a307695 semantics): no
//     clipboardchange, no deleteByCut beforeinput — while a text input on
//     the same page shows both signals (positive control).
//
// Gated like the rest of the live servo suite:
//   BAO_TEST_NETWORK=1 xvfb-run cargo nt -p bao-browser \
//     -E 'test(editing_e2e)'

#![allow(dead_code)]

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use bao_browser::{BaoConfig, BrowserRuntime, PageConfig, PageHandle};
use servo::{Code, Key, KeyState, Location, Modifiers, NamedKey};

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
/// subresources — the editing face needs no network).
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

/// The `Input.insertText` carrier: per-character trusted Down/Up pair with
/// `Key::Character` (mirrors `cdp_handler::cmd_insert_text` — the value
/// changes through the servo text-input engine, never a `el.value` shortcut).
fn type_text(page: &PageHandle, text: &str) {
    for ch in text.chars() {
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
}

fn press_named(page: &PageHandle, named: NamedKey, code: Code, modifiers: Modifiers) {
    let key = Key::Named(named);
    page.dispatch_key_event_full(
        KeyState::Down,
        key.clone(),
        code,
        Location::Standard,
        modifiers,
        false,
    );
    page.dispatch_key_event_full(
        KeyState::Up,
        key,
        code,
        Location::Standard,
        modifiers,
        false,
    );
}

/// Ctrl+C / Ctrl+X via the real keyboard path (ShortcutMatcher
/// CMD_OR_CONTROL → EditingAction::Clipboard).
fn press_ctrl(page: &PageHandle, ch: char) {
    let key = Key::Character(ch.to_string());
    page.dispatch_key_event_full(
        KeyState::Down,
        key.clone(),
        Code::Unidentified,
        Location::Standard,
        Modifiers::CONTROL,
        false,
    );
    page.dispatch_key_event_full(
        KeyState::Up,
        key,
        Code::Unidentified,
        Location::Standard,
        Modifiers::CONTROL,
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

/// The `Document::perform_editing_action` pref gate
/// (`dom_exec_command_enabled`, default false — the same pref gating the
/// execCommand webidl face) keeps every Document-context editing action dark
/// (contenteditable typing/Enter/Delete/arrows) until the pref contract
/// lands. Probes the page-observable face once; contenteditable pins skip
/// loudly when the engine is dark instead of pinning the defect green.
fn document_editing_enabled(page: &PageHandle) -> bool {
    page.evaluate_js_web("typeof document.execCommand")
        .map(|s| s.contains("function"))
        .unwrap_or(false)
}

/// Poll `js` until its result contains `target` (or deadline; returns the
/// last observed value either way so the caller's assertion reports it).
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

/// @trace REQ-BRW-002 [criterion:contenteditable-real-key-insertion] live
///
/// Real key delivery into a focused contenteditable must insert text through
/// the servo text-input engine: `innerText` changes, the `input` event fires,
/// and the key events are trusted (`isTrusted`).
#[test]
fn editing_e2e_contenteditable_real_key_insertion() {
    if should_skip() {
        return;
    }
    let _guard = serializer().lock().unwrap_or_else(|e| e.into_inner());

    let html = r#"<!DOCTYPE html><html><body>
<div id="ce" contenteditable="true"></div>
<script>
window.__events = [];
['keydown','input'].forEach(function(t){
  document.addEventListener(t, function(e){
    window.__events.push({type: t, trusted: e.isTrusted, data: e.data || null});
  }, true);
});
window.__state = function(){
  return JSON.stringify({text: document.getElementById('ce').innerText,
                         events: window.__events});
};
</script></body></html>"#;
    let (_runtime, page) = make_page(&data_url(html));

    if !document_editing_enabled(&page) {
        eprintln!(
            "[skip] document-context editing is pref-dark (dom_exec_command_enabled=false, \
             registered defect) — contenteditable insertion pin deferred until the pref \
             contract lands"
        );
        return;
    }

    page.evaluate_js_web("document.getElementById('ce').focus();")
        .expect("focus eval must run");
    type_text(&page, "Hi");

    let state = poll_until_contains(
        &page,
        "window.__state()",
        "\"text\":\"Hi\"",
        10_000,
    );
    eprintln!("[editing-e2e] contenteditable state = {state}");
    assert!(
        state.contains("\"text\":\"Hi\"") || state.contains("\"text\":\"Hi\\n\""),
        "innerText must contain the typed text: {state}"
    );
    // Trusted keydown + real input events (data payloads from the
    // text-input engine).
    assert!(
        state.contains("\"type\":\"keydown\",\"trusted\":true"),
        "keydown must be trusted: {state}"
    );
    assert!(
        state.contains("\"type\":\"input\""),
        "input event must fire: {state}"
    );
}

/// @trace REQ-BRW-002 [criterion:insert-text-carrier-values] live
///
/// The insertText carrier (per-character Character Down/Up) must update
/// `<input>`/`<textarea>` `.value` through the real engine, including
/// non-ASCII payloads, without touching unrelated fields.
#[test]
fn editing_e2e_insert_text_carrier_input_and_textarea() {
    if should_skip() {
        return;
    }
    let _guard = serializer().lock().unwrap_or_else(|e| e.into_inner());

    let html = r#"<!DOCTYPE html><html><body>
<input id="in"><textarea id="ta"></textarea>
<script>window.__state = function(){
  return JSON.stringify({in: document.getElementById('in').value,
                         ta: document.getElementById('ta').value});
};</script>
</body></html>"#;
    let (_runtime, page) = make_page(&data_url(html));

    page.evaluate_js_web("document.getElementById('in').focus();")
        .expect("focus input eval must run");
    type_text(&page, "bao");

    let state = poll_until_contains(
        &page,
        "window.__state()",
        "\"in\":\"bao\"",
        10_000,
    );
    eprintln!("[editing-e2e] input state = {state}");
    assert!(
        state.contains("\"in\":\"bao\""),
        "input.value must be updated by the carrier: {state}"
    );
    assert!(
        state.contains("\"ta\":\"\""),
        "textarea must stay untouched: {state}"
    );

    page.evaluate_js_web("document.getElementById('ta').focus();")
        .expect("focus textarea eval must run");
    type_text(&page, "编辑");

    let state = poll_until_contains(
        &page,
        "window.__state()",
        "\"ta\":\"编辑\"",
        10_000,
    );
    eprintln!("[editing-e2e] textarea state = {state}");
    assert!(
        state.contains("\"ta\":\"编辑\""),
        "textarea.value must carry the non-ASCII payload: {state}"
    );
    assert!(
        state.contains("\"in\":\"bao\""),
        "input.value must be stable: {state}"
    );
}

/// @trace REQ-BRW-002 [criterion:enter-insert-paragraph] live
///
/// Enter (NamedKey) must drive the InsertParagraph editing action: typing
/// across an Enter in a contenteditable yields a two-line innerText.
#[test]
fn editing_e2e_enter_key_paragraph_in_contenteditable() {
    if should_skip() {
        return;
    }
    let _guard = serializer().lock().unwrap_or_else(|e| e.into_inner());

    let html = r#"<!DOCTYPE html><html><body>
<div id="ce" contenteditable="true"></div>
<script>window.__lines = function(){
  var t = document.getElementById('ce').innerText;
  return t.split('\n').filter(function(l){return l.length > 0;}).length.toString();
};</script>
</body></html>"#;
    let (_runtime, page) = make_page(&data_url(html));

    if !document_editing_enabled(&page) {
        eprintln!(
            "[skip] document-context editing is pref-dark (dom_exec_command_enabled=false, \
             registered defect) — Enter paragraph pin deferred until the pref contract lands"
        );
        return;
    }

    page.evaluate_js_web("document.getElementById('ce').focus();")
        .expect("focus eval must run");
    type_text(&page, "a");
    press_named(&page, NamedKey::Enter, Code::Enter, Modifiers::empty());
    type_text(&page, "b");

    let lines = poll_until_contains(
        &page,
        "(function(){ return window.__lines() === '2' ? 'two-lines' : window.__lines(); })()",
        "two-lines",
        10_000,
    );
    eprintln!("[editing-e2e] enter lines = {lines}");
    assert!(
        lines.contains("two-lines"),
        "Enter must split the editable into two lines: {lines}"
    );
}

/// @trace REQ-BRW-002 [criterion:inputevent-init-datatransfer] live
///
/// The `InputEventInit.dataTransfer` dictionary member must be consumed by
/// the `InputEvent` constructor (input-events spec dictionary): a
/// script-constructed event carries the passed `DataTransfer` instance
/// (identity + payload round-trip), and the absent member defaults to
/// `null`. e159 邻接观察③清偿 pin — the UA-only setter path (trusted
/// clipboard events) is covered by the WPT insertFromPaste campaign; this
/// pin holds the script-construction face.
#[test]
fn editing_e2e_inputevent_init_datatransfer() {
    if should_skip() {
        return;
    }
    let _guard = serializer().lock().unwrap_or_else(|e| e.into_inner());

    let html = r#"<!DOCTYPE html><html><body><script>
window.__probe = function(){
  var dt = new DataTransfer();
  dt.setData('text/plain', 'payload');
  var withDt = new InputEvent('beforeinput',
                              {inputType: 'insertFromPaste', dataTransfer: dt});
  var withoutDt = new InputEvent('beforeinput',
                                 {inputType: 'insertText', data: 'x'});
  return JSON.stringify({
    identity: withDt.dataTransfer === dt,
    payload: withDt.dataTransfer ? withDt.dataTransfer.getData('text/plain') : null,
    defaultNull: withoutDt.dataTransfer === null
  });
};
</script></body></html>"#;
    let (_runtime, page) = make_page(&data_url(html));

    let state = page
        .evaluate_js_web("window.__probe()")
        .expect("probe eval must run");
    eprintln!("[editing-e2e] inputevent init dataTransfer = {state}");
    assert!(
        state.contains("\"identity\":true"),
        "constructor must carry the InputEventInit.dataTransfer member: {state}"
    );
    assert!(
        state.contains("\"payload\":\"payload\""),
        "the carried DataTransfer must be the passed instance (getData round-trip): {state}"
    );
    assert!(
        state.contains("\"defaultNull\":true"),
        "absent dictionary member must default to null: {state}"
    );
}

/// @trace REQ-BRW-002 [criterion:password-clipboard-guard] live
///
/// The copying_enabled/cutting_enabled guards (57a307695): Ctrl+C/Ctrl+X on a
/// password field must produce NO clipboardchange and NO deleteByCut
/// beforeinput, while a text input on the same page produces both (positive
/// control) — observed through the real keyboard clipboard-action path.
#[test]
fn editing_e2e_password_copy_cut_guard() {
    if should_skip() {
        return;
    }
    let _guard = serializer().lock().unwrap_or_else(|e| e.into_inner());

    let html = r#"<!DOCTYPE html><html><body>
<input id="pw" type="password"><input id="tx" type="text">
<script>
window.__signals = {clipboardchange: 0, deleteByCut: 0};
document.addEventListener('clipboardchange', function(){
  window.__signals.clipboardchange++;
});
document.addEventListener('beforeinput', function(e){
  if (e.inputType === 'deleteByCut') window.__signals.deleteByCut++;
});
window.__reset = function(){
  window.__signals = {clipboardchange: 0, deleteByCut: 0};
  return 'ok';
};
window.__count = function(){
  return JSON.stringify(window.__signals);
};
</script></body></html>"#;
    let (_runtime, page) = make_page(&data_url(html));

    // Positive control: text input copy/cut must be visible.
    page.evaluate_js_web(
        "var t = document.getElementById('tx'); t.value='secret'; t.focus(); t.select(); window.__reset();",
    )
    .expect("text field setup eval must run");
    press_ctrl(&page, 'c');
    press_ctrl(&page, 'x');
    // Copy fires clipboardchange; cut fires another clipboardchange plus the
    // deleteByCut beforeinput — poll for the cut signal (the later one).
    let text_signals = poll_until_contains(
        &page,
        "window.__count()",
        "\"deleteByCut\":1",
        10_000,
    );
    eprintln!("[editing-e2e] text-field signals = {text_signals}");
    assert!(
        !text_signals.contains("\"clipboardchange\":0"),
        "text input copy/cut must fire clipboardchange: {text_signals}"
    );
    assert!(
        text_signals.contains("\"deleteByCut\":1"),
        "text input cut must fire deleteByCut beforeinput: {text_signals}"
    );

    // Guard: password field copy/cut must be blocked.
    page.evaluate_js_web(
        "var p = document.getElementById('pw'); p.value='hunter2'; p.focus(); p.select(); window.__reset();",
    )
    .expect("password field setup eval must run");
    press_ctrl(&page, 'c');
    press_ctrl(&page, 'x');
    std::thread::sleep(Duration::from_millis(500));
    let pw_signals = page
        .evaluate_js_web("window.__count()")
        .expect("password signals eval must run");
    eprintln!("[editing-e2e] password signals = {pw_signals}");
    assert!(
        pw_signals.contains("\"clipboardchange\":0"),
        "password copy must NOT fire clipboardchange: {pw_signals}"
    );
    assert!(
        pw_signals.contains("\"deleteByCut\":0"),
        "password cut must NOT fire deleteByCut beforeinput: {pw_signals}"
    );
}
