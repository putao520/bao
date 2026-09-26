// @trace TEST-ENG-007-WEB-PORT [req:REQ-ENG-007] [level:integration]
// Assertions ported from Bun's own test suite (~/code/rust/bun/test/js/web/
// and test/js/bun/) into the bao integration harness, to raise process/branch
// coverage of Web APIs and Bun-specific APIs:
//   - web/url/url.test.ts            → URL origin/field branches, URLSearchParams
//   - web/encoding/text-*.test.*     → TextEncoder/TextDecoder multi-byte,
//                                      lone surrogates, BOM, labels, stream mode
//   - bun/crypto + node vectors      → CryptoHasher digest formats, chaining, HMAC
//   - bun/io/bun-write.test.js       → Bun.file text/json/arrayBuffer/exists, Bun.write
//   - bun/globals.test.js            → Bun.env, class names, File/Blob ctor faces
//   - bun/http/bun-serve-*.test.ts   → Bun.serve fetch-handler branches (localhost only)
//   - web/streams/streams.test.js    → ReadableStream/WritableStream/TransformStream
//   - web/websocket/websocket.test.js→ client-side API shape (readyState constants)
//   - zlib sync aliases edge cases, performance.now monotonicity, Bun.semver.satisfies
//
// Divergences found against upstream are recorded as `// SKIPPED(bao-divergence)`
// next to the ported assertion they would have carried — product code is
// deliberately NOT changed by this port.
//
// Harness pattern: same as bun_wave_a_surface_tests.rs — one JSContext per
// #[test] (nextest per-test process isolation), globals installed, bounded
// drain hook ticking the uWS loop + microtasks; serve tests drive raw TCP.

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
use std::cell::Cell;
use std::io::{Read, Write};
use std::net::TcpStream;

thread_local! {
    static HOOK_BUDGET: Cell<usize> = const { Cell::new(0) };
}

fn bounded_drain_hook(cx: &mut mozjs::context::JSContext) -> bool {
    let exhausted = HOOK_BUDGET.with(|b| {
        let n = b.get();
        if n == 0 {
            return true;
        }
        b.set(n - 1);
        false
    });
    if exhausted {
        return false;
    }
    bun_runtime::timers::drain_and_check(cx)
}

fn fresh_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx.set_post_eval_hook(bounded_drain_hook);
    ctx
}

fn eval_str(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<web-port>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => {
            if n.fract() == 0.0 && n.abs() < 1e15 {
                format!("{}", n as i64)
            } else {
                format!("{}", n)
            }
        }
        Ok(JsValue::Bool(b)) => {
            if b {
                "true".to_string()
            } else {
                "false".to_string()
            }
        }
        Ok(_) => String::new(),
        Err(_) => "<eval-error>".to_string(),
    }
}

fn eval_ok(ctx: &mut JsContext, source: &str) -> bool {
    ctx.eval(source, "<web-port>").is_ok()
}

/// Poll a JS condition until it evaluates to "y", ticking the event loop
/// between attempts (each ctx.eval fires the bounded drain hook).
fn wait_until(ctx: &mut JsContext, js_condition: &str, budget: usize) -> bool {
    for _ in 0..120 {
        HOOK_BUDGET.with(|b| b.set(budget));
        if eval_str(ctx, js_condition) == "y" {
            return true;
        }
    }
    false
}

fn tick(ctx: &mut JsContext) {
    HOOK_BUDGET.with(|b| b.set(50));
    let _ = eval_str(ctx, "'t'");
}

/// Pump the loop while polling the socket until `done(buf)` holds.
fn tick_read_until<F: Fn(&[u8]) -> bool>(
    ctx: &mut JsContext,
    s: &mut TcpStream,
    done: F,
    max_ticks: usize,
) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    for _ in 0..max_ticks {
        tick(ctx);
        match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if done(&buf) {
                    break;
                }
            }
            Err(_) => {}
        }
    }
    buf
}

/// Issue one raw HTTP request against the in-process serve and return the
/// full response text (status line + headers + body).
fn http_send(ctx: &mut JsContext, port: u16, raw_req: &str) -> String {
    let mut s = TcpStream::connect(("127.0.0.1", port)).expect("connect serve");
    s.set_read_timeout(Some(std::time::Duration::from_millis(50))).ok();
    s.set_nonblocking(false).ok();
    s.write_all(raw_req.as_bytes()).unwrap();
    let buf = tick_read_until(ctx, &mut s, |_| false, 200);
    String::from_utf8_lossy(&buf).into_owned()
}

fn http_get(ctx: &mut JsContext, port: u16, path: &str) -> String {
    let req = format!(
        "GET {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nConnection: close\r\n\r\n",
        path, port
    );
    http_send(ctx, port, &req)
}

// ═══════════════════════════════════════════════════════════════════════
// Area 1: URL parse branches (web/url/url.test.ts)
// ═══════════════════════════════════════════════════════════════════════
#[test]
fn test_url_parse_branches_ported() {
    let mut ctx = fresh_ctx();

    // origin/protocol table (upstream "should have correct origin and protocol")
    assert_eq!(
        eval_str(&mut ctx, "new URL('https://example.com').protocol + '|' + new URL('https://example.com').origin"),
        "https:|https://example.com"
    );
    assert_eq!(
        eval_str(&mut ctx, "new URL('http://example.com').origin + '|' + new URL('ftp://example.com').origin"),
        "http://example.com|ftp://example.com"
    );
    assert_eq!(
        eval_str(&mut ctx, "new URL('ws://example.com').origin + '|' + new URL('wss://example.com').origin"),
        "ws://example.com|wss://example.com"
    );
    // data:/opaque-scheme URLs carry no origin (upstream expects "null" for
    // data:, javascript:, mailto:, and unknown schemes alike)
    assert_eq!(
        eval_str(
            &mut ctx,
            "new URL('data:text/plain,x').origin + '|' + new URL('blob:kjka://example.com').origin"
        ),
        "null|null"
    );
    // SKIPPED(bao-divergence): upstream parses non-special / non-hierarchical
    // URL forms (about:blank, mailto:, javascript:alert(1)) with protocol +
    // FIXED (URL conformance wave): about:/mailto:/javascript: parse with
    // origin "null"; file://example.com origin is "null"; blob: origins
    // derive from the inner URL (WHATWG).
    assert_eq!(
        eval_str(&mut ctx, "new URL('about:blank').protocol + '|' + new URL('about:blank').origin"),
        "about:|null"
    );
    assert_eq!(
        eval_str(&mut ctx, "new URL('file://example.com').origin"),
        "null"
    );
    assert_eq!(
        eval_str(&mut ctx, "new URL('blob:https://example.com/1234-5678').protocol + '|' + new URL('blob:https://example.com/1234-5678').origin"),
        "blob:|https://example.com"
    );
    assert_eq!(
        eval_str(&mut ctx, "new URL('blob:ws://example.com').origin + '|' + new URL('blob:file:///x').origin"),
        "ws://example.com|file://"
    );

    // full-field parse (upstream "works" — username:password@api.foo.bar.com:9999)
    assert_eq!(
        eval_str(
            &mut ctx,
            r#"(function(){
                var u = new URL('https://username:password@api.foo.bar.com:9999/baz/okay/i/123?ran=out&of=things#to-use-as-a-placeholder');
                return [u.protocol, u.username, u.password, u.hostname, u.port, u.host, u.pathname, u.search, u.hash, u.origin].join('~');
            })()"#
        ),
        "https:~username~password~api.foo.bar.com~9999~api.foo.bar.com:9999~/baz/okay/i/123~?ran=out&of=things~#to-use-as-a-placeholder~https://api.foo.bar.com:9999"
    );
    // SKIPPED(bao-divergence): upstream normalizes the href of
    // "https://url.spec.whatwg.org#url-serializing" to insert the root
    // pathname ("...org/#url-serializing"); bao keeps "...org#url-serializing".
    assert_eq!(
        eval_str(
            &mut ctx,
            "(function(){ var u = new URL('https://url.spec.whatwg.org#url-serializing'); return u.pathname + '|' + u.port + '|' + u.host; })()"
        ),
        "/||url.spec.whatwg.org"
    );

    // IPv6 literal host (WHATWG serialization keeps brackets)
    assert_eq!(
        eval_str(&mut ctx, "(function(){ var u = new URL('http://[::1]:8080/x'); return u.hostname + '|' + u.host + '|' + u.port; })()"),
        "[::1]|[::1]:8080|8080"
    );
    // FIXED (URL conformance wave): WHATWG special-scheme default ports are
    // elided — .port '' and .host without :80 (parser-side elision in
    // bun_url; href re-serialization lands with the node_url face fix).
    assert_eq!(
        eval_str(&mut ctx, "(function(){ var u = new URL('http://example.com:80/x'); return u.port + '|' + u.host; })()"),
        "|example.com",
        "default port :80 must be stripped from port and host"
    );
    assert_eq!(
        eval_str(&mut ctx, "(function(){ var u = new URL('https://example.com:443/x'); return u.port + '|' + u.host; })()"),
        "|example.com",
        "default port :443 must be stripped from port and host"
    );

    // cannot-be-a-base URL: opaque path, protocol data:, searchParams face exists
    assert_eq!(
        eval_str(
            &mut ctx,
            "(function(){ var u = new URL('data:text/plain,hi'); return u.protocol + '|' + u.pathname + '|' + typeof u.searchParams.get; })()"
        ),
        "data:|text/plain,hi|function"
    );

    // URL throws on unparseable input (upstream: message + ERR_INVALID_URL)
    assert!(!eval_ok(&mut ctx, "new URL('')"), "new URL('') must throw");
    assert!(!eval_ok(&mut ctx, "new URL(' ')"), "new URL(' ') must throw");
    assert!(!eval_ok(&mut ctx, "new URL('boop', 'http!/example.com')"), "relative against invalid base must throw");
    // SKIPPED(bao-divergence): upstream throws TypeError with code
    // 'ERR_INVALID_URL'; bao throws a plain Error with no .code.
    // SKIPPED(bao-divergence): upstream redacts credentials in the throw
    // message ('<redacted>'); bao has no redaction.
}

// ═══════════════════════════════════════════════════════════════════════
// Area 2: URLSearchParams (web/url/url.test.ts + WHATWG behaviors)
// ═══════════════════════════════════════════════════════════════════════
#[test]
fn test_url_search_params_ported() {
    let mut ctx = fresh_ctx();

    // '+' decodes to space; %2B survives as literal plus
    assert_eq!(
        eval_str(&mut ctx, "new URLSearchParams('q=a+b%2Bc').get('q')"),
        "a b+c"
    );
    // get returns null for missing keys
    assert_eq!(eval_str(&mut ctx, "String(new URLSearchParams('a=1').get('nope'))"), "null");
    // getAll preserves duplicates
    assert_eq!(
        eval_str(&mut ctx, "JSON.stringify(new URLSearchParams('a=1&b=2&a=3').getAll('a'))"),
        r#"["1","3"]"#
    );
    // has() covers both presence forms
    assert_eq!(
        eval_str(
            &mut ctx,
            "String(new URLSearchParams('a=1&b').has('a')) + '|' + String(new URLSearchParams('a=1&b').has('b')) + '|' + String(new URLSearchParams('a=1').has('z'))"
        ),
        "true|true|false"
    );
    // delete removes all pairs of the name
    assert_eq!(
        eval_str(&mut ctx, "(function(){ var s = new URLSearchParams('a=1&b=2&a=3'); s.delete('a'); return s.toString(); })()"),
        "b=2"
    );
    // set() collapses every existing pair of the name into one value.
    // SKIPPED(bao-divergence): WHATWG moves the surviving pair to the end
    // ("b=2&a=9"); bao replaces the first pair in place ("a=9&b=2").
    assert_eq!(
        eval_str(&mut ctx, "(function(){ var s = new URLSearchParams('a=1&b=2&a=3'); s.set('a', '9'); return s.toString() + '|' + s.getAll('a').length; })()"),
        "a=9&b=2|1"
    );
    // append keeps duplicates
    assert_eq!(
        eval_str(&mut ctx, "(function(){ var s = new URLSearchParams(); s.append('k', 'v 1'); s.append('k', '2'); return s.toString(); })()"),
        "k=v+1&k=2"
    );
    // sort is by Unicode code points, stable for equal names (upstream sort vectors)
    assert_eq!(
        eval_str(&mut ctx, "(function(){ var s = new URLSearchParams('a=3&b=1&a=1&c=0'); s.sort(); return s.toString(); })()"),
        "a=3&a=1&b=1&c=0"
    );
    // forEach iterates (value, key) in insertion order
    assert_eq!(
        eval_str(
            &mut ctx,
            "(function(){ var s = new URLSearchParams('a=1&b=2'); var acc=''; s.forEach(function(v,k){ acc += k+'='+v+';'; }); return acc; })()"
        ),
        "a=1;b=2;"
    );
    // constructor from object form
    assert_eq!(
        eval_str(&mut ctx, "(function(){ try { return new URLSearchParams({a: '1', b: '2'}).toString(); } catch(e) { return 'throw'; } })()"),
        "a=1&b=2"
    );
    // round-trip through URL.search
    assert_eq!(
        eval_str(
            &mut ctx,
            "(function(){ var u = new URL('http://x/p'); u.searchParams.append('q', 'zhi yu'); return u.search + '|' + u.href; })()"
        ),
        "?q=zhi+yu|http://x/p?q=zhi+yu"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Area 3: TextEncoder / TextDecoder (web/encoding/text-{en,de}coder.test.*)
// ═══════════════════════════════════════════════════════════════════════
#[test]
fn test_text_encoding_ported() {
    let mut ctx = fresh_ctx();

    // encode(undefined/'') → empty; (upstream: encode(null) encodes "null" → 4)
    assert_eq!(
        eval_str(&mut ctx, "new TextEncoder().encode(undefined).length + '|' + new TextEncoder().encode('').length"),
        "0|0"
    );
    // SKIPPED(bao-divergence): upstream TextEncoder().encode(null).length === 4
    // (stringifies to "null"); bao returns 0 bytes for null.

    // ASCII byte-for-byte (upstream latin1 vector)
    assert_eq!(
        eval_str(&mut ctx, "JSON.stringify(Array.from(new TextEncoder().encode('Hello World!')))"),
        "[72,101,108,108,111,32,87,111,114,108,100,33]"
    );
    // CJK 3-byte and U+FFFD 3-byte forms (upstream t-table)
    assert_eq!(
        eval_str(&mut ctx, "JSON.stringify(Array.from(new TextEncoder().encode('世'))) + '|' + JSON.stringify(Array.from(new TextEncoder().encode('\\uFFFD')))"),
        "[228,184,150]|[239,191,189]"
    );
    // NUL code point encodes to one zero byte (upstream)
    assert_eq!(
        eval_str(&mut ctx, "JSON.stringify(Array.from(new TextEncoder().encode(String.fromCodePoint(0))))"),
        "[0]"
    );
    // emoji: 4-byte UTF-8 + decode round-trip (upstream utf-16 text test)
    assert_eq!(
        eval_str(&mut ctx, "(function(){ var t = '🔥'; var e = new TextEncoder().encode(t); return e.length + '|' + (new TextDecoder().decode(e) === t); })()"),
        "4|true"
    );
    // latin1 non-ASCII chars encode as 2-byte UTF-8 (upstream H©ell©o vector)
    assert_eq!(
        eval_str(&mut ctx, "(function(){ var e = new TextEncoder().encode('H\\u00A9x'); return e.length + '|' + e[1] + '|' + e[2] + '|' + e[3]; })()"),
        "4|194|169|120"
    );
    // encodeInto face exists.
    // SKIPPED(bao-divergence): upstream TextEncoder.prototype.encodeInto
    // returns {read, written} and writes UTF-8 bytes into the destination
    // (with partial-character backpressure); bao's encodeInto is a no-op —
    // it returns undefined and writes zero bytes (probed: 16/2/3-byte
    // destinations all stay zeroed).
    assert_eq!(
        eval_str(&mut ctx, "typeof new TextEncoder().encodeInto"),
        "function"
    );
    // lone surrogate → U+FFFD (3 bytes), round-trips as replacement (upstream
    // "comprehensive invalid UTF-16 edge cases")
    assert_eq!(
        eval_str(
            &mut ctx,
            "(function(){ var e = new TextEncoder().encode(String.fromCharCode(0xd800)); var d = new TextDecoder().decode(e); return e.length + '|' + d.charCodeAt(0).toString(16); })()"
        ),
        "3|fffd"
    );
    // valid surrogate pair at boundary stays intact (upstream boundary test)
    assert_eq!(
        eval_str(
            &mut ctx,
            "(function(){ var s = String.fromCharCode(0xdbff, 0xdfff); var e = new TextEncoder().encode(s); return e.length + '|' + (new TextDecoder().decode(e) === s); })()"
        ),
        "4|true"
    );
    // SKIPPED(bao-divergence): upstream encodeInto with a 2-byte buffer that
    // cannot fit U+FFFD reports read=0/written=0 ("not enough space for
    // replacement character"), and with exactly 3 bytes reports read=1 /
    // written=3 ([239,191,189]); bao's encodeInto performs no write and
    // returns no record (see the divergence note above).
    // long ASCII round-trip (upstream "should encode long latin1 text")
    assert_eq!(
        eval_str(
            &mut ctx,
            "(function(){ var t = new Array(1001).join('Hello World!'); var e = new TextEncoder().encode(t); return e.length + '|' + (new TextDecoder().decode(e) === t); })()"
        ),
        "12000|true"
    );

    // Decoder: lenient invalid byte → single U+FFFD
    assert_eq!(
        eval_str(&mut ctx, "(function(){ var d = new TextDecoder('utf-8').decode(new Uint8Array([0xff])); return d.length + '|' + d.charCodeAt(0).toString(16); })()"),
        "1|fffd"
    );
    // SKIPPED(bao-divergence): upstream TextDecoder('utf-8', {fatal:true})
    // throws TypeError on invalid input; bao decodes to U+FFFD (fatal ignored).
    // SKIPPED(bao-divergence): upstream default decoder strips a leading UTF-8
    // BOM (decode([BOM,'a']) === 'a'); bao keeps it (length 2).
    // ignoreBOM keeps the BOM as U+FEFF
    assert_eq!(
        eval_str(&mut ctx, "new TextDecoder('utf-8', {ignoreBOM: true}).decode(new Uint8Array([0xef,0xbb,0xbf,0x61])).length"),
        "2"
    );
    // encoding labels
    assert_eq!(
        eval_str(&mut ctx, "new TextDecoder('utf-8').encoding + '|' + new TextDecoder().encoding"),
        "utf-8|utf-8"
    );
    assert_eq!(eval_str(&mut ctx, "new TextDecoder('latin1').encoding"), "latin1");
    // latin1 label accepted; ASCII-range bytes decode identically
    // SKIPPED(bao-divergence): upstream latin1/windows-1252 decoders map
    // bytes 0x80-0xFF to U+0080-U+00FF (0xA0 → U+00A0); bao emits U+FFFD
    // for high bytes (single-byte tables not implemented).
    assert_eq!(
        eval_str(&mut ctx, "(function(){ try { var d = new TextDecoder('latin1').decode(new Uint8Array([0x41, 0x00])); return d.length + '|' + d.charCodeAt(0).toString(16); } catch(e) { return 'throw'; } })()"),
        "2|41"
    );
    // utf-16le label accepted.
    // SKIPPED(bao-divergence): upstream consumes little-endian byte PAIRS
    // ([0x42,0x00] → "B"; lone surrogate [0x00,0xd8] → exactly U+FFFD);
    // bao decodes byte-per-unit ("B\0" for the ASCII pair, "\\u0000\\uFFFD"
    // for the surrogate pair) — no paired decoding.
    assert_eq!(
        eval_str(&mut ctx, "(function(){ try { return JSON.stringify(new TextDecoder('utf-16le').decode(new Uint8Array([0x42,0x00]))); } catch(e) { return 'throw'; } })()"),
        "\"B\\u0000\""
    );
    // SKIPPED(bao-divergence): upstream decode(..., {stream:true}) holds a
    // partial multi-byte sequence across calls ( '' then complete char );
    // bao emits U+FFFD for the partial first chunk and drops the byte.
    // TextDecoderStream faces
    assert_eq!(
        eval_str(&mut ctx, "(function(){ var t = new TextDecoderStream(); return t.encoding + '|' + typeof t.readable + '|' + typeof t.writable; })()"),
        "utf-8|object|object"
    );
    assert_eq!(
        eval_str(&mut ctx, "(function(){ var t = new TextEncoderStream(); return t.encoding + '|' + typeof t.readable + '|' + typeof t.writable; })()"),
        "utf-8|object|object"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Area 4: CryptoHasher digest formats / chaining / HMAC (bun/crypto + vectors)
// ═══════════════════════════════════════════════════════════════════════
#[test]
fn test_crypto_hasher_ported() {
    let mut ctx = fresh_ctx();

    // empty-input digests (SHA-256('') / SHA-1('') / MD5('') known vectors)
    assert_eq!(
        eval_str(&mut ctx, "new Bun.CryptoHasher('sha256').digest('hex')"),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        eval_str(&mut ctx, "new Bun.CryptoHasher('sha1').digest('hex')"),
        "da39a3ee5e6b4b0d3255bfef95601890afd80709"
    );
    assert_eq!(
        eval_str(&mut ctx, "new Bun.CryptoHasher('md5').digest('hex')"),
        "d41d8cd98f00b204e9800998ecf8427e"
    );
    // SHA-512('abc') known vector (prefix; full digest is 128 hex chars)
    assert_eq!(
        eval_str(&mut ctx, "new Bun.CryptoHasher('sha512').update('abc').digest('hex').slice(0, 16) + '|' + new Bun.CryptoHasher('sha512').update('abc').digest('hex').length"),
        "ddaf35a193617aba|128"
    );
    // base64 digest format (sha256('abc'))
    assert_eq!(
        eval_str(&mut ctx, "new Bun.CryptoHasher('sha256').update('abc').digest('base64')"),
        "ungWv48Bz+pBQUDeXa4iI7ADYaOWF3qctBD/YfIAFa0="
    );
    // SKIPPED(bao-divergence): upstream digest() with no argument returns a
    // Buffer (Uint8Array, 32 bytes for sha256); bao returns the hex string.
    assert_eq!(
        eval_str(&mut ctx, "(function(){ var d = new Bun.CryptoHasher('sha256').update('abc').digest(); return typeof d + '|' + String(d).length; })()"),
        "string|64"
    );
    // update() chaining equals one-shot
    assert_eq!(
        eval_str(
            &mut ctx,
            "String(new Bun.CryptoHasher('sha256').update('a').update('b').update('c').digest('hex') === new Bun.CryptoHasher('sha256').update('abc').digest('hex'))"
        ),
        "true"
    );
    // HMAC face: CryptoHasher(algorithm, key) — HMAC-SHA256(key='key', data='data')
    assert_eq!(
        eval_str(&mut ctx, "new Bun.CryptoHasher('sha256', 'key').update('data').digest('hex')"),
        "3a6eb0790f39ac87c94f3856b2dd2c5d110e6811602261a9a923d3bb23adc8b7"
    );
    // Buffer input digests identically to the string form
    assert_eq!(
        eval_str(
            &mut ctx,
            "String(new Bun.CryptoHasher('sha256').update(Buffer.from('abc')).digest('hex') === 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad')"
        ),
        "true"
    );
    // unknown algorithm throws (upstream error branch)
    assert!(
        !eval_ok(&mut ctx, "new Bun.CryptoHasher('not-an-algorithm')"),
        "unknown digest algorithm must throw"
    );
    // SKIPPED(bao-divergence): upstream supports 'sha384'; bao throws
    // "Unsupported algorithm: sha384".
    // Bun.password faces exist (bun/crypto password tests gate on them)
    assert_eq!(
        eval_str(&mut ctx, "typeof Bun.password.hash + '|' + typeof Bun.password.verify"),
        "function|function"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Area 5: Bun.file / Bun.write (bun/io/bun-write.test.js, bun-object/write.spec.ts)
// ═══════════════════════════════════════════════════════════════════════
#[test]
fn test_bun_file_io_ported() {
    let mut ctx = fresh_ctx();
    let dir = std::env::temp_dir().join(format!("bao_web_port_io_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("rt.txt"), b"roundtrip-body").unwrap();
    std::fs::write(dir.join("cfg.json"), b"{\"a\":7,\"s\":\"zhi\"}").unwrap();
    std::fs::write(dir.join("uni-\u{4e16}\u{754c}.txt"), "unicode-body").unwrap();
    let ds = dir.to_string_lossy().replace('\\', "/");
    let written = dir.join("written.txt");

    // .text() round-trip (bun-serve-file.test.ts face)
    eval_ok(
        &mut ctx,
        &format!(
            "globalThis.__rt = 'pending'; Bun.file('{ds}/rt.txt').text().then(function(t){{ globalThis.__rt = 'ok:' + t; }}).catch(function(e){{ globalThis.__rt = 'err:' + e.message; }}); 'k'"
        ),
    );
    assert!(
        wait_until(&mut ctx, "globalThis.__rt !== 'pending' ? 'y' : 'n'", 100),
        "Bun.file().text() must resolve"
    );
    assert_eq!(eval_str(&mut ctx, "globalThis.__rt"), "ok:roundtrip-body");

    // .json() parse
    eval_ok(
        &mut ctx,
        &format!(
            "globalThis.__rj = 'pending'; Bun.file('{ds}/cfg.json').json().then(function(j){{ globalThis.__rj = 'ok:' + j.a + ':' + j.s; }}).catch(function(e){{ globalThis.__rj = 'err:' + e.message; }}); 'k'"
        ),
    );
    assert!(
        wait_until(&mut ctx, "globalThis.__rj !== 'pending' ? 'y' : 'n'", 100),
        "Bun.file().json() must resolve"
    );
    assert_eq!(eval_str(&mut ctx, "globalThis.__rj"), "ok:7:zhi");

    // .arrayBuffer() byte length
    eval_ok(
        &mut ctx,
        &format!(
            "globalThis.__ra = 'pending'; Bun.file('{ds}/rt.txt').arrayBuffer().then(function(b){{ globalThis.__ra = 'ok:' + b.byteLength; }}).catch(function(e){{ globalThis.__ra = 'err:' + e.message; }}); 'k'"
        ),
    );
    assert!(
        wait_until(&mut ctx, "globalThis.__ra !== 'pending' ? 'y' : 'n'", 100),
        "Bun.file().arrayBuffer() must resolve"
    );
    assert_eq!(eval_str(&mut ctx, "globalThis.__ra"), "ok:14");

    // exists(): dir true / file true / missing false
    eval_ok(
        &mut ctx,
        &format!(
            "globalThis.__ex = ''; Bun.file('{ds}').exists().then(function(v){{ globalThis.__ex += 'dir:' + v + '|'; }}); Bun.file('{ds}/rt.txt').exists().then(function(v){{ globalThis.__ex += 'file:' + v + '|'; }}); Bun.file('{ds}/missing-xyz').exists().then(function(v){{ globalThis.__ex += 'missing:' + v; }}); 'k'"
        ),
    );
    assert!(
        wait_until(&mut ctx, "String(globalThis.__ex).indexOf('missing:') >= 0 ? 'y' : 'n'", 100),
        "exists() trio must resolve, got {}",
        eval_str(&mut ctx, "globalThis.__ex")
    );
    assert_eq!(eval_str(&mut ctx, "globalThis.__ex"), "dir:true|file:true|missing:false");

    // Bun.write(string path, string data) → readable via Bun.file().text()
    eval_ok(
        &mut ctx,
        &format!(
            "globalThis.__wr = 'pending'; Bun.write('{w}', 'written-body').then(function(){{ return Bun.file('{w}').text(); }}).then(function(t){{ globalThis.__wr = 'ok:' + t; }}).catch(function(e){{ globalThis.__wr = 'err:' + e.message; }}); 'k'",
            w = written.to_string_lossy().replace('\\', "/")
        ),
    );
    // SKIPPED(bao-divergence): upstream Bun.write resolves its Promise (to
    // the byte count); bao performs the real filesystem write but the
    // Promise never settles (waited 100 loop budgets). The write effect
    // itself is asserted from the Rust side below.
    // the file really landed on disk through the real filesystem
    assert_eq!(std::fs::read_to_string(&written).unwrap(), "written-body");

    // SKIPPED(bao-divergence): upstream Bun.file resolves paths containing
    // non-ASCII characters. bao cannot: with the file created on disk
    // ("uni-世界.txt") and the JS path string verified code-point-exact
    // (charCodes 0x4e16,0x754c,...), Bun.file(path).exists() still resolves
    // false ("ok:missing") — the native path handoff mangles non-ASCII.

    // SKIPPED(bao-divergence): upstream Bun.write accepts Blob/Buffer/Response
    // bodies; bao rejects non-string bodies ("Bun.write requires string
    // arguments").
    // SKIPPED(bao-divergence): upstream BunFile.writer() (streamed writes)
    // is not implemented in bao ("writer is not a function").
    let _ = std::fs::remove_dir_all(&dir);
}

// ═══════════════════════════════════════════════════════════════════════
// Area 6: Bun.env (bun/globals.test.js + bun-object surface)
// ═══════════════════════════════════════════════════════════════════════
#[test]
fn test_bun_env_ported() {
    let mut ctx = fresh_ctx();

    assert_eq!(eval_str(&mut ctx, "typeof Bun.env.PATH"), "string");
    assert_eq!(eval_str(&mut ctx, "typeof Bun.env.HOME"), "string");
    // same underlying view as process.env: set → visible, delete → gone
    assert_eq!(
        eval_str(
            &mut ctx,
            "(function(){ process.env.__BAO_PORT_ENV = 'v1'; var a = Bun.env.__BAO_PORT_ENV; var k = ('__BAO_PORT_ENV' in Bun.env); delete process.env.__BAO_PORT_ENV; var b = Bun.env.__BAO_PORT_ENV; var k2 = ('__BAO_PORT_ENV' in Bun.env); return a + '|' + k + '|' + String(b) + '|' + k2; })()"
        ),
        "v1|true|undefined|false"
    );
    // Object.keys sees an injected key
    assert_eq!(
        eval_str(
            &mut ctx,
            "(function(){ process.env.__BAO_PORT_ENV2 = 'x'; var n = Object.keys(Bun.env).indexOf('__BAO_PORT_ENV2') >= 0; delete process.env.__BAO_PORT_ENV2; return String(n); })()"
        ),
        "true"
    );
    // writing through Bun.env is visible in process.env (same record)
    assert_eq!(
        eval_str(
            &mut ctx,
            "(function(){ try { Bun.env.__BAO_PORT_ENV3 = 'y3'; var v = process.env.__BAO_PORT_ENV3; delete process.env.__BAO_PORT_ENV3; return String(v); } catch(e) { return 'throw'; } })()"
        ),
        "y3"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Area 7: Bun.serve fetch-handler branches (bun/http/*, localhost only)
// ═══════════════════════════════════════════════════════════════════════
#[cfg(unix)]
#[test]
fn test_bun_serve_branches_ported() {
    // Deadline isolation for the in-process server (same crash-class guard
    // as bun_wave_a_surface_tests.rs).
    crate::exit_isolation::dispatch_timeout(
        "web_api_port_tests::test_bun_serve_branches_ported",
        test_bun_serve_branches_ported_body,
    );
}

#[cfg(unix)]
fn test_bun_serve_branches_ported_body() {
    let mut ctx = fresh_ctx();
    let port = 19420u16;

    let setup = eval_str(
        &mut ctx,
        r#"
        globalThis.__srvW = Bun.serve({
            port: 19420,
            fetch: function(req) {
                if (req.url.indexOf('/created') === 0) {
                    return new Response('made', { status: 201, statusText: 'Created' });
                }
                if (req.url.indexOf('/nocontent') === 0) {
                    return new Response(null, { status: 204 });
                }
                if (req.url.indexOf('/method') === 0) {
                    return new Response('method:' + req.method);
                }
                if (req.url.indexOf('/reqtype') === 0) {
                    return new Response('req:' + typeof req.url + ':' + typeof req.method + ':' + typeof req.headers);
                }
                if (req.url.indexOf('/boom') === 0) { throw new Error('handler-boom'); }
                if (req.url.indexOf('/undef') === 0) { return undefined; }
                return new Response('default-body');
            },
        });
        'up'
    "#,
    );
    assert_eq!(setup, "up", "serve must start");
    assert!(
        wait_until(&mut ctx, "globalThis.__srvW && globalThis.__srvW.port === 19420 ? 'y' : 'n'", 30),
        "serve must report its port"
    );

    // default 200 (upstream bun-serve fixture default face)
    let resp = http_get(&mut ctx, port, "/");
    assert!(
        resp.contains("HTTP/1.1 200") && resp.contains("default-body"),
        "default fetch must return 200 + body:\n{}",
        resp
    );

    // custom status + statusText survive serialization (upstream status branches)
    let resp = http_get(&mut ctx, port, "/created");
    assert!(
        resp.contains("HTTP/1.1 201") && resp.to_ascii_lowercase().contains("created") && resp.contains("made"),
        "custom status/statusText must serialize:\n{}",
        resp
    );

    // 204 no-content with null body
    let resp = http_get(&mut ctx, port, "/nocontent");
    assert!(
        resp.contains("HTTP/1.1 204"),
        "204 null-body response must serialize:\n{}",
        resp
    );

    // Request face delivered to the handler: url/method strings + a headers
    // object (upstream also satisfies `req instanceof Request`).
    // SKIPPED(bao-divergence): upstream serve-side requests are Request
    // instances; bao's `req instanceof Request` is false.
    let resp = http_get(&mut ctx, port, "/reqtype");
    assert!(
        resp.contains("req:string:string:object"),
        "handler must receive url/method strings and a headers object:\n{}",
        resp
    );

    // method echo — upstream Request.method is uppercase ("POST"); bao's
    // serve-side request lowercases it, so compare case-insensitively and
    // assert the bao-observed form.
    let resp = http_send(
        &mut ctx,
        port,
        "POST /method HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    );
    assert!(
        resp.contains("method:post") || resp.contains("method:POST"),
        "request method must reach the handler:\n{}",
        resp
    );
    assert!(
        resp.contains("method:post"),
        "SKIPPED(bao-divergence): upstream echoes 'POST'; bao serves lowercase 'post'"
    );

    // routing key is the URL path (req.url starts with the request path)
    let resp = http_get(&mut ctx, port, "/created?x=1");
    assert!(
        resp.contains("HTTP/1.1 201"),
        "query string must not break path routing:\n{}",
        resp
    );
    // SKIPPED(bao-divergence): upstream Request.url is the absolute URL
    // ("http://127.0.0.1:19420/created?x=1"); bao exposes only the path
    // ("/created?x=1") — `new URL(req.url)` inside a handler throws.

    // SKIPPED(bao-divergence): upstream serve-side req.headers is a Headers
    // instance (req.headers.get('x-a')); bao exposes a plain object whose
    // .get is not a function.

    // handler throw → 500 with body (upstream error branch)
    let resp = http_get(&mut ctx, port, "/boom");
    assert!(
        resp.contains("500"),
        "throwing handler must produce a 500:\n{}",
        resp
    );

    // non-Response return must not hang the connection (bao answers 404;
    // upstream Bun also synthesizes an error status for undefined returns)
    let resp = http_get(&mut ctx, port, "/undef");
    let status = resp
        .split(' ')
        .nth(1)
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);
    assert!(
        (400..600).contains(&status),
        "undefined return must yield an error status, got:\n{}",
        resp
    );

    // server object faces
    assert_eq!(
        eval_str(&mut ctx, "typeof globalThis.__srvW.stop + '|' + typeof globalThis.__srvW.port"),
        "function|number"
    );
    // SKIPPED(bao-divergence): upstream accepts and exposes an idleTimeout
    // serve option; bao's server object exposes no idleTimeout face.
    eval_ok(&mut ctx, "globalThis.__srvW.stop(true); 'stopped'");
}

// ═══════════════════════════════════════════════════════════════════════
// Area 8: Streams (web/streams/streams.test.js)
// ═══════════════════════════════════════════════════════════════════════
#[test]
fn test_streams_ported() {
    let mut ctx = fresh_ctx();

    // ReadableStream: getReader → queued values then done (upstream basic shape)
    eval_ok(
        &mut ctx,
        "globalThis.__rsOut = []; var __rs = new ReadableStream({ start: function(c) { c.enqueue('a'); c.enqueue('b'); c.close(); } }); var __r = __rs.getReader(); __r.read().then(function(x){ globalThis.__rsOut.push(String(x.value)); return __r.read(); }).then(function(x){ globalThis.__rsOut.push(String(x.value)); return __r.read(); }).then(function(x){ globalThis.__rsOut.push('done:' + x.done); }); 'k'",
    );
    assert!(
        wait_until(&mut ctx, "globalThis.__rsOut.length === 3 ? 'y' : 'n'", 50),
        "readable read chain must drain"
    );
    assert_eq!(
        eval_str(&mut ctx, "JSON.stringify(globalThis.__rsOut)"),
        r#"["a","b","done:true"]"#
    );
    // SKIPPED(bao-divergence): upstream (streams spec) throws TypeError on a
    // second getReader() while a reader holds the lock; bao returns a fresh
    // reader without error.

    // WritableStream sink collects writes (upstream writable sink shape)
    eval_ok(
        &mut ctx,
        "globalThis.__wsSeen = []; var __ws = new WritableStream({ write: function(c) { globalThis.__wsSeen.push(String(c)); } }); var __w = __ws.getWriter(); __w.write('x').then(function(){ return __w.write('y'); }).then(function(){ return __w.close(); }); 'k'",
    );
    assert!(
        wait_until(&mut ctx, "globalThis.__wsSeen.length === 2 ? 'y' : 'n'", 50),
        "writable sink must collect both writes"
    );
    assert_eq!(eval_str(&mut ctx, "JSON.stringify(globalThis.__wsSeen)"), r#"["x","y"]"#);

    // identity TransformStream pipe-through
    eval_ok(
        &mut ctx,
        "globalThis.__tsOut = []; var __ts = new TransformStream(); var __tw = __ts.writable.getWriter(); var __tr = __ts.readable.getReader(); __tw.write('m').then(function(){ return __tr.read(); }).then(function(x){ globalThis.__tsOut.push(String(x.value)); return __tw.close(); }).catch(function(e){ globalThis.__tsOut.push('err:' + e.message); }); 'k'",
    );
    assert!(
        wait_until(&mut ctx, "globalThis.__tsOut.length === 1 ? 'y' : 'n'", 50),
        "transform read must resolve"
    );
    assert_eq!(eval_str(&mut ctx, "JSON.stringify(globalThis.__tsOut)"), r#"["m"]"#);

    // errored stream → read() rejects (upstream error branch)
    eval_ok(
        &mut ctx,
        "globalThis.__esOut = 'pending'; var __es = new ReadableStream({ start: function(c) { c.error(new Error('kaboom')); } }); __es.getReader().read().then(function(){ globalThis.__esOut = 'unexpected-resolve'; }).catch(function(e){ globalThis.__esOut = 'rejected:' + e.message; }); 'k'",
    );
    assert!(
        wait_until(&mut ctx, "globalThis.__esOut !== 'pending' ? 'y' : 'n'", 50),
        "errored stream read must settle"
    );
    assert_eq!(eval_str(&mut ctx, "globalThis.__esOut"), "rejected:kaboom");
}

// ═══════════════════════════════════════════════════════════════════════
// Area 9: Blob / File ctor faces (bun/globals.test.js "File" describe)
// ═══════════════════════════════════════════════════════════════════════
#[test]
fn test_blob_file_faces_ported() {
    let mut ctx = fresh_ctx();

    // File constructor faces (upstream File describe)
    assert_eq!(
        eval_str(
            &mut ctx,
            "(function(){ var f = new File(['foo'], 'bar.txt', { type: 'text/plain;charset=utf-8' }); return f.name + '|' + f.size + '|' + f.type + '|' + (f.lastModified > 0) + '|' + (f instanceof Blob); })()"
        ),
        "bar.txt|3|text/plain;charset=utf-8|true|true"
    );
    // empty parts → size 0, name/type preserved
    assert_eq!(
        eval_str(&mut ctx, "(function(){ var f = new File([], 'empty.txt', { type: 'text/plain' }); return f.name + '|' + f.size + '|' + f.type; })()"),
        "empty.txt|0|text/plain"
    );
    // lastModified option is honored exactly (upstream)
    assert_eq!(
        eval_str(&mut ctx, "String(new File(['foo'], 'b.txt', { lastModified: 123 }).lastModified)"),
        "123"
    );
    // Blob multi-part size + type + text() face
    assert_eq!(
        eval_str(&mut ctx, "(function(){ var b = new Blob(['foo','bar'], { type: 'text/plain' }); return b.size + '|' + b.type + '|' + typeof b.text; })()"),
        "6|text/plain|function"
    );
    // Blob.text() resolves with the concatenated body
    eval_ok(
        &mut ctx,
        "globalThis.__bt = 'pending'; new Blob(['foo','bar']).text().then(function(t){ globalThis.__bt = 'ok:' + t; }); 'k'",
    );
    assert!(wait_until(&mut ctx, "globalThis.__bt !== 'pending' ? 'y' : 'n'", 50));
    assert_eq!(eval_str(&mut ctx, "globalThis.__bt"), "ok:foobar");
    // Blob.slice subsets the byte range
    assert_eq!(
        eval_str(&mut ctx, "(function(){ try { return String(new Blob(['abcdef']).slice(1, 3).size); } catch(e) { return 'noslice'; } })()"),
        "2"
    );
    // SKIPPED(bao-divergence): upstream File() without `new` throws
    // TypeError ("Class constructor File cannot be invoked without 'new'");
    // bao returns undefined without throwing.
    // class names (upstream "name" test)
    assert_eq!(
        eval_str(&mut ctx, "[Blob, TextDecoder, TextEncoder, Request, Response, Headers, Buffer, File].map(function(c){ return c.name; }).join(',')"),
        "Blob,TextDecoder,TextEncoder,Request,Response,Headers,Buffer,File"
    );
    // globals are writable (upstream "writable" test; restored immediately)
    assert_eq!(
        eval_str(
            &mut ctx,
            "(function(){ var C = TextDecoder; try { globalThis.TextDecoder = 123; var a = globalThis.TextDecoder; globalThis.TextDecoder = C; var b = globalThis.TextDecoder === C; return String(a) + '|' + b; } catch(e) { globalThis.TextDecoder = C; return 'throw'; } })()"
        ),
        "123|true"
    );
    // SKIPPED(bao-divergence): upstream defines `self` as a getter resolving
    // to globalThis; bao leaves `self` undefined.
    // SKIPPED(bao-divergence): upstream Request.prototype.formData.call(...)
    // rejects with TypeError { code: 'ERR_INVALID_THIS' }; bao does not
    // surface the coded error shape.
}

// ═══════════════════════════════════════════════════════════════════════
// Area 10: misc — performance.now, WebSocket client shape, zlib edges,
//          fetch data: URL, console, semver.satisfies
// ═══════════════════════════════════════════════════════════════════════
#[test]
fn test_misc_web_surface_ported() {
    let mut ctx = fresh_ctx();

    // performance.now(): number + non-decreasing (web-globals face)
    assert_eq!(
        eval_str(&mut ctx, "(function(){ var a = performance.now(); var b = performance.now(); return (typeof a) + '|' + String(b >= a); })()"),
        "number|true"
    );

    // WebSocket client-side API shape only (server side covered elsewhere)
    assert_eq!(
        eval_str(&mut ctx, "typeof WebSocket + '|' + WebSocket.CONNECTING + '|' + WebSocket.OPEN + '|' + WebSocket.CLOSING + '|' + WebSocket.CLOSED"),
        "function|0|1|2|3"
    );

    // zlib: empty-input gzip round-trip (edge — header-only stream)
    assert_eq!(
        eval_str(
            &mut ctx,
            "(function(){ var gz = Bun.gzipSync(new TextEncoder().encode('')); var back = new TextDecoder().decode(Bun.gunzipSync(gz)); return 'ok:' + back.length + '|' + (gz.length > 0); })()"
        ),
        "ok:0|true"
    );
    // zlib: raw deflate/inflate pair
    assert_eq!(
        eval_str(&mut ctx, "new TextDecoder().decode(Bun.inflateSync(Bun.deflateSync(new TextEncoder().encode('raw-roundtrip'))))"),
        "raw-roundtrip"
    );
    // zlib: large (1MB) input round-trip stays byte-faithful
    eval_ok(
        &mut ctx,
        "globalThis.__zbig = 'pending'; (function(){ var t = new Array(4097).join('abcdefgh') + '!'; var gz = Bun.gzipSync(new TextEncoder().encode(t)); var back = new TextDecoder().decode(Bun.gunzipSync(gz)); globalThis.__zbig = 'ok:' + (back === t) + ':' + t.length; })(); 'k'",
    );
    assert_eq!(eval_str(&mut ctx, "globalThis.__zbig"), "ok:true:32769");

    // fetch of a data: URL (no network): text body decodes
    eval_ok(
        &mut ctx,
        "globalThis.__df = 'pending'; fetch('data:text/plain,hi').then(function(r){ return r.text(); }).then(function(t){ globalThis.__df = 'ok:' + t; }).catch(function(e){ globalThis.__df = 'err:' + e.message; }); 'k'",
    );
    assert!(
        wait_until(&mut ctx, "globalThis.__df !== 'pending' ? 'y' : 'n'", 50),
        "data: URL fetch must settle"
    );
    assert_eq!(eval_str(&mut ctx, "globalThis.__df"), "ok:hi");

    // console faces eval side-effect-free
    assert!(eval_ok(&mut ctx, "console.log('web-port-log'); console.warn('w'); console.error('e'); 'k'"));
    assert!(eval_ok(&mut ctx, "console.info('i'); console.debug('d'); 'k'"));

    // Bun.semver.satisfies (not covered by the wave-a order() assertions)
    assert_eq!(
        eval_str(
            &mut ctx,
            "String(Bun.semver.satisfies('1.2.3', '^1.0.0')) + '|' + String(Bun.semver.satisfies('2.0.0', '^1.0.0'))"
        ),
        "true|false"
    );
    assert_eq!(
        eval_str(
            &mut ctx,
            "String(Bun.semver.satisfies('1.2.3', '~1.2.0')) + '|' + String(Bun.semver.satisfies('1.3.0', '~1.2.0'))"
        ),
        "true|false"
    );
    assert_eq!(
        eval_str(
            &mut ctx,
            "String(Bun.semver.satisfies('1.2.4', '>1.2.3')) + '|' + String(Bun.semver.satisfies('1.2.3', '1.2.3'))"
        ),
        "true|true"
    );
}
