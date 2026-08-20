// @trace TEST-ENG-006 [req:REQ-ENG-006] [level:integration]
// BCE (atob binary truncation, 2026-08-18) regression pins:
//   - web_api::atob_fn returned only the bytes up to the first NUL (a
//     117k-char base64 WAV decoded to 8-9 bytes) — String::from_utf8_lossy
//     corrupted binary + JS_NewStringCopyZ cut at 0x00. Fixed to the HTML
//     spec contract (each octet = one code unit, explicit-length copy).
//   - Same-class sweep fixes pinned here too: bun:sqlite TEXT values with
//     embedded NULs, and Shell error-object stdout/stderr.
// The page realm shares this exact implementation via
// runtime_bridge::install_atob_btoa (media_e2e finding #3 was this bug,
// not vendor servo — servo's own base64_atob was always correct).

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Err(e) => format!("ERROR:{}", e.message),
        _ => String::new(),
    }
}

#[test]
fn test_atob_large_binary_payload_full_roundtrip() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    // 88,000-byte full-value-domain payload (NULs guaranteed: the pattern
    // hits every byte value, including the 0x00s of a RIFF size field),
    // encoded via btoa — the same size class as the media_e2e finding
    // (117k base64 chars → previously 8-9 bytes returned).
    let out = eval_string(
        &mut ctx,
        r#"
        function byteAt(i) { return (i * 7 + 13) & 0xFF; }
        // RIFF-style head: magic + LE size (contains 0x00 bytes at [6..8])
        var head = [0x52,0x49,0x46,0x46, 0xE0,0x2E,0x00,0x00, 0x57,0x41,0x56,0x45];
        var n = 88000;
        var chunks = [];
        var s = '';
        for (var i = 0; i < 12; i++) s += String.fromCharCode(head[i]);
        for (var i = 12; i < n; i++) {
            s += String.fromCharCode(byteAt(i));
            if ((i & 0xFFF) === 0) { chunks.push(s); s = ''; }
        }
        chunks.push(s);
        var src = chunks.join('');
        var enc = btoa(src);
        var dec = atob(enc);
        var headOk = true;
        for (var i = 0; i < 12; i++) if (dec.charCodeAt(i) !== head[i]) headOk = false;
        var deepOk = true;
        [100, 1000, 10000, 43999, 87999].forEach(function (off) {
            if (dec.charCodeAt(off) !== byteAt(off)) deepOk = false;
        });
        (enc.length >= 117000) + '|' + (dec.length === n) + '|' + headOk + '|' + deepOk
    "#,
    );
    assert_eq!(
        out, "true|true|true|true",
        "atob(btoa(x)) must round-trip an 88k full-domain payload byte-exactly \
         (input {} chars); previously truncated at the first NUL",
        out
    );
}

#[test]
fn test_atob_embedded_nul_and_edges() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    let out = eval_string(
        &mut ctx,
        r#"
        var nulCase = atob('YWIA').charCodeAt(2) === 0x00;       // "b\0" tail
        var empty = atob('') === '';                              // empty input
        var pad1 = atob('QQ==') === 'A';                          // single pad
        var pad2 = atob('QUI=') === 'AB';                         // double pad
        var err = 'NO-THROW';
        try { atob('A'); } catch (e) { err = 'THREW'; }           // len%4==1 must throw
        [nulCase, empty, pad1, pad2, err === 'THREW'].join('|')
    "#,
    );
    assert_eq!(
        out, "true|true|true|true|true",
        "atob embedded-NUL + padding + len%4==1 error edges (got {})", out
    );
}

#[test]
fn test_sqlite_text_embedded_nul_survives() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    // Same-class sweep fix: TEXT values with embedded NULs used to truncate
    // at the first 0x00 on the rusqlite Value -> JSVal path.
    let out = eval_string(
        &mut ctx,
        r#"
        var { Database } = require('bun:sqlite');
        var db = new Database(':memory:');
        db.exec('CREATE TABLE t(v TEXT)');
        db.prepare('INSERT INTO t VALUES (?)').run('a' + String.fromCharCode(0) + 'b');
        var v = db.prepare('SELECT v FROM t').get().v;
        (v.length) + '|' + (v.charCodeAt(1) === 0)
    "#,
    );
    assert_eq!(
        out, "3|true",
        "sqlite TEXT with embedded NUL must keep length 3 (got {})", out
    );
}
