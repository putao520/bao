// @trace TEST-ENG-PLATFORM [req:REQ-ENG-001 REQ-ENG-006 REQ-ENG-008 REQ-ENG-009] [level:e2e]
// Platform-surface e2e: crypto.subtle (WebCrypto), bun:sqlite
// transaction/iterate/serialize/backup, bun:ffi dlopen(path, symbols) +
// CString + toBuffer, EventSource (SSE), localStorage (CLI), and the
// fetch()-built Response surface (WHATWG Headers/instanceof/json/arrayBuffer).
//
// Exit strategy mirrors fetch_headers_e2e_tests: shutdown_for_exit +
// process::exit(0) (parked HTTPThread is a non-daemon thread; force-exit also
// sidesteps the mimalloc atexit double-free documented in fetch_e2e_tests.rs).

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
use bun_runtime::timers;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        Ok(JsValue::Null) => "null".to_string(),
        Ok(JsValue::Undefined) => "undefined".to_string(),
        Ok(JsValue::Object(_)) => "[object]".to_string(),
        Err(e) => format!("ERROR:{}", e.message),
    }
}

/// Drive the event loop until `probe` (a JS expression) satisfies `check`, or
/// the deadline passes. Returns the final probe value.
fn drive_until(
    ctx: &mut JsContext,
    probe: &str,
    check: impl Fn(&str) -> bool,
    timeout: Duration,
) -> String {
    let cx_raw = ctx.raw_cx();
    let deadline = Instant::now() + timeout;
    let mut last = String::new();
    while Instant::now() < deadline {
        unsafe {
            mozjs_sys::jsapi::js::RunJobs(cx_raw);
        }
        timers::with_event_loop(|loop_| {
            loop_.tick_without_idle(std::ptr::null_mut());
        });
        std::thread::sleep(Duration::from_millis(2));
        last = eval_string(ctx, probe);
        if check(&last) {
            return last;
        }
    }
    last
}

/// One-shot HTTP responder: serves exactly one connection with a fixed raw
/// response, capturing the raw request bytes.
fn start_oneshot_server(raw_response: &'static str) -> (u16, Arc<Mutex<Option<Vec<u8>>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let captured: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
    let sink = Arc::clone(&captured);
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .ok();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                match stream.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&chunk[..n]);
                        if let Some(pos) = find_sub(&buf, b"\r\n\r\n") {
                            // headers complete; body per Content-Length if any
                            let head = String::from_utf8_lossy(&buf[..pos]).to_lowercase();
                            let clen = head.lines().find_map(|l| {
                                l.strip_prefix("content-length:")
                                    .and_then(|v| v.trim().parse::<usize>().ok())
                            });
                            let done = match clen {
                                Some(n) => buf.len() >= pos + 4 + n,
                                None => true,
                            };
                            if done {
                                break;
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            *sink.lock().unwrap() = Some(buf);
            let _ = stream.write_all(raw_response.as_bytes());
            let _ = stream.flush();
        }
    });
    (port, captured)
}

fn find_sub(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[test]
fn test_platform_surface_e2e() {
    bun_core::output::init_test();
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();

    // Isolate localStorage persistence away from the real ~/.bao.
    let tmp_home = std::env::temp_dir().join(format!("bao-ls-test-{}", std::process::id()));
    std::fs::create_dir_all(&tmp_home).unwrap();
    // SAFETY: single-threaded test setup phase, before any JS context exists.
    unsafe { std::env::set_var("HOME", tmp_home.to_string_lossy().to_string()) };

    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    // ── A. crypto.subtle (WebCrypto over the real bao_crypto primitives) ────
    let setup = eval_string(
        &mut ctx,
        r#"
        (function() {
            globalThis.__errs = [];
            globalThis.__done = 0;
            globalThis.__total = 10;
            function step(name, p) {
                return p.then(function() { globalThis.__done++; })
                        .catch(function(e) { globalThis.__errs.push(name + ': ' + (e && e.message || String(e))); globalThis.__done++; });
            }
            var enc = new TextEncoder();
            var S = crypto.subtle;

            // A1 AES-GCM roundtrip + tag length
            step('A1-gcm', S.generateKey({name:'AES-GCM', length:256}, false, ['encrypt','decrypt'])
            .then(function(k) {
                return S.encrypt({name:'AES-GCM', iv:new Uint8Array(12), additionalData:enc.encode('aad')}, k, enc.encode('hello world'));
            }).then(function(ct) {
                if (ct.byteLength !== 'hello world'.length + 16) throw new Error('GCM ct+tag length wrong: ' + ct.byteLength);
                return ct;
            }));

            // A2 AES-CBC roundtrip via raw import
            var rawKey = new Uint8Array(32); for (var i=0;i<32;i++) rawKey[i] = i;
            var iv = new Uint8Array(16);
            step('A2-cbc', S.importKey('raw', rawKey, {name:'AES-CBC'}, false, ['encrypt','decrypt'])
            .then(function(k) {
                return S.encrypt({name:'AES-CBC', iv:iv}, k, enc.encode('cbc-data-16-bytes!'));
            }).then(function(ct) {
                return S.importKey('raw', rawKey, {name:'AES-CBC'}, false, ['encrypt','decrypt'])
                    .then(function(kd) { return S.decrypt({name:'AES-CBC', iv:iv}, kd, ct); });
            }).then(function(pt) {
                var s = new TextDecoder().decode(new Uint8Array(pt));
                if (s !== 'cbc-data-16-bytes!') throw new Error('CBC roundtrip mismatch: ' + s);
            }));

            // A3 decrypt with the WRONG key rejects (auth failure surfaces)
            var wrongKey = new Uint8Array(32).fill(7);
            step('A3-gcm-wrongkey', S.importKey('raw', rawKey, {name:'AES-GCM'}, false, ['encrypt'])
            .then(function(k) { return S.encrypt({name:'AES-GCM', iv:new Uint8Array(12)}, k, enc.encode('x')); })
            .then(function(ct) {
                return S.importKey('raw', wrongKey, {name:'AES-GCM'}, false, ['decrypt'])
                    .then(function(bad) { return S.decrypt({name:'AES-GCM', iv:new Uint8Array(12)}, bad, ct); });
            })
            .then(function() { throw new Error('wrong-key decrypt must reject'); },
                  function() { /* expected rejection */ }));

            // A4 HMAC sign/verify + tamper detection via raw import
            step('A4-hmac', S.importKey('raw', enc.encode('secret-key'), {name:'HMAC', hash:'SHA-256'}, false, ['sign','verify'])
            .then(function(k) {
                return S.sign({name:'HMAC'}, k, enc.encode('payload')).then(function(sig) {
                    if (sig.byteLength !== 32) throw new Error('HMAC-SHA256 sig length: ' + sig.byteLength);
                    // COPY before tampering — a Uint8Array view would mutate the original.
                    var sig2 = new Uint8Array(sig).slice(); sig2[0] ^= 1;
                    return S.verify({name:'HMAC'}, k, sig, enc.encode('payload')).then(function(ok) {
                        if (!ok) throw new Error('HMAC verify failed');
                        return S.verify({name:'HMAC'}, k, sig2.buffer, enc.encode('payload'));
                    }).then(function(okT) {
                        if (okT) throw new Error('tampered HMAC verified (must be false)');
                    });
                });
            }));

            // A5 jwk oct import roundtrip; asymmetric jwk import rejects explicitly
            step('A5-jwk', (function() {
                var b64 = function(u8) {
                    var s = ''; for (var i=0;i<u8.length;i++) s += String.fromCharCode(u8[i]);
                    return btoa(s).replace(/\+/g,'-').replace(/\//g,'_').replace(/=+$/,'');
                };
                var jwk = { kty: 'oct', k: b64(rawKey) };
                return S.importKey('jwk', jwk, {name:'AES-GCM'}, false, ['encrypt'])
                .then(function(k) { return S.encrypt({name:'AES-GCM', iv:new Uint8Array(12)}, k, enc.encode('jwk-ok')); })
                .then(function(ct) {
                    if (ct.byteLength !== 'jwk-ok'.length + 16) throw new Error('jwk oct encrypt length wrong');
                    return S.importKey('jwk', {kty:'RSA'}, {name:'RSA-RSASSA-PKCS1-v1_5'}, false, ['sign']);
                })
                .then(function() { throw new Error('RSA jwk import must reject'); },
                      function(e) { if (String(e.message).indexOf('oct only') === -1) throw e; });
            })());

            // A6 ECDSA P-256 generateKey (CryptoKeyPair) + sign(raw r||s) with
            // privateKey + verify with publicKey + tamper rejection
            step('A6-ecdsa', S.generateKey({name:'ECDSA', namedCurve:'P-256'}, false, ['sign','verify'])
            .then(function(kp) {
                if (!kp.privateKey || !kp.publicKey) throw new Error('ECDSA generateKey must resolve with a CryptoKeyPair');
                if (kp.privateKey.type !== 'private' || kp.publicKey.type !== 'public') throw new Error('key pair types');
                if (kp.privateKey.algorithm.name !== 'ECDSA' || kp.privateKey.algorithm.namedCurve !== 'P-256') throw new Error('key pair algorithm');
                return S.sign({name:'ECDSA', hash:'SHA-256'}, kp.privateKey, enc.encode('ec-data')).then(function(sig) {
                    if (sig.byteLength !== 64) throw new Error('ECDSA raw sig length: ' + sig.byteLength);
                    return S.verify({name:'ECDSA', hash:'SHA-256'}, kp.publicKey, sig, enc.encode('ec-data')).then(function(ok) {
                        if (!ok) throw new Error('ECDSA verify failed');
                        // COPY before tampering — a Uint8Array view would mutate the original.
                        var bad = new Uint8Array(sig).slice(); bad[0] ^= 1;
                        return S.verify({name:'ECDSA', hash:'SHA-256'}, kp.publicKey, bad, enc.encode('ec-data'));
                    }).then(function(okT) {
                        if (okT) throw new Error('tampered ECDSA verified (must be false)');
                    });
                });
            }));

            // A7 RSA-RSASSA generateKey (CryptoKeyPair) + sign with privateKey + verify with publicKey
            step('A7-rsa', S.generateKey({name:'RSA-RSASSA-PKCS1-v1_5', modulusLength:2048, publicExponent:new Uint8Array([1,0,1]), hash:'SHA-256'}, false, ['sign','verify'])
            .then(function(kp) {
                if (!kp.privateKey || !kp.publicKey) throw new Error('RSA generateKey must resolve with a CryptoKeyPair');
                return S.sign({name:'RSA-RSASSA-PKCS1-v1_5'}, kp.privateKey, enc.encode('rsa-data')).then(function(sig) {
                    if (sig.byteLength !== 256) throw new Error('RSA-2048 sig length: ' + sig.byteLength);
                    return S.verify({name:'RSA-RSASSA-PKCS1-v1_5'}, kp.publicKey, sig, enc.encode('rsa-data'));
                }).then(function(ok) {
                    if (!ok) throw new Error('RSA verify failed');
                });
            }));

            // A8 digest still intact (pre-existing surface)
            step('A8-digest', S.digest('SHA-256', enc.encode('abc')).then(function(d) {
                if (d.byteLength !== 32) throw new Error('digest length');
                return crypto.subtle.digest('SHA-256', enc.encode('abc'));
            }));

            // A9 unsupported algorithm rejects with a message
            step('A9-unsupported', S.importKey('raw', new Uint8Array(8), {name:'DES-CBC'}, false, ['encrypt'])
            .then(function() { throw new Error('DES must reject'); },
                  function(e) { if (String(e.message).indexOf('unsupported') === -1) throw e; }));

            // A10 CryptoKey shape (type/extractable/usages/algorithm.name)
            step('A10-shape', S.generateKey({name:'AES-GCM', length:128}, true, ['encrypt'])
            .then(function(k) {
                if (k.type !== 'secret') throw new Error('key.type');
                if (k.extractable !== true) throw new Error('key.extractable');
                if (k.algorithm.name !== 'AES-GCM') throw new Error('key.algorithm.name');
                if (k.algorithm.length !== 128) throw new Error('key.algorithm.length');
                if (!k.usages || k.usages[0] !== 'encrypt') throw new Error('key.usages');
            }));

            return 'scheduled';
        })()
        "#,
    );
    assert!(setup.contains("scheduled"), "crypto.subtle setup failed: {}", setup);
    let status = drive_until(
        &mut ctx,
        r#"globalThis.__done + ':' + globalThis.__errs.length"#,
        |s| s.starts_with("10:"),
        Duration::from_secs(60),
    );
    let errs = eval_string(&mut ctx, r#"JSON.stringify(globalThis.__errs)"#);
    assert!(
        status.starts_with("10:0"),
        "crypto.subtle steps did not all pass: status={} errs={}",
        status,
        errs
    );

    // ── A-crash. non-object key args must REJECT, never abort the process ──
    // P0 regression: generateKey resolved with a single key where spec code
    // expects a CryptoKeyPair, so `kp.privateKey` was `undefined`, and
    // sign/verify called JSVal::to_object() on it — a debug-assert abort
    // (jsval.rs `assertion failed: self.is_object()`), i.e. a process crash
    // instead of a rejected promise. Same hazard class swept across
    // encrypt/decrypt/generateKey/importKey usages/key args.
    let crash_setup = eval_string(
        &mut ctx,
        r#"
        (function() {
            globalThis.__c = { done: 0, errs: [] };
            // Each step must REJECT with a message containing `needle`;
            // resolving or rejecting with anything else lands in errs.
            function cstep(name, needle, p) {
                return p.then(
                    function() { globalThis.__c.errs.push(name + ': resolved (must reject)'); },
                    function(e) {
                        var m = String(e && e.message || e);
                        if (m.indexOf(needle) === -1) globalThis.__c.errs.push(name + ': wrong message: ' + m);
                    }
                ).then(function() { globalThis.__c.done++; });
            }
            var enc = new TextEncoder();
            var S = crypto.subtle;
            var alg = {name:'ECDSA', hash:'SHA-256'};

            // c1 the exact abort trigger: sign with a non-object key rejects
            cstep('c1', 'CryptoKey', S.sign(alg, undefined, enc.encode('x')));

            // c2 verify with a non-object key rejects
            cstep('c2', 'CryptoKey', S.verify(alg, undefined, new Uint8Array(64), enc.encode('x')));

            // c3 sign with the PUBLIC half of the pair rejects (InvalidAccess path)
            cstep('c3', 'private', S.generateKey({name:'ECDSA', namedCurve:'P-256'}, false, ['sign','verify'])
            .then(function(kp) { return S.sign(alg, kp.publicKey, enc.encode('x')); }));

            return 'scheduled';
        })()
        "#,
    );
    assert!(crash_setup.contains("scheduled"), "subtle crash-path setup failed: {}", crash_setup);
    let c_status = drive_until(
        &mut ctx,
        r#"globalThis.__c.done + ':' + globalThis.__c.errs.length"#,
        |s| s.starts_with("3:"),
        Duration::from_secs(30),
    );
    let c_errs = eval_string(&mut ctx, r#"JSON.stringify(globalThis.__c.errs)"#);
    assert!(
        c_status.starts_with("3:0"),
        "subtle non-object-key rejections missing: status={} errs={}",
        c_status,
        c_errs
    );

    // ── B. bun:sqlite transaction / iterate / serialize / backup ────────────
    let sqlite_out = eval_string(
        &mut ctx,
        r#"
        (function() {
            var { Database } = require('bun:sqlite');
            var db = new Database(':memory:');
            db.exec('CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)');
            var out = [];

            // B1 transaction commit: return value forwarded, rows visible
            var insertTwo = db.transaction(function(a, b) {
                db.run('INSERT INTO t (v) VALUES (?)', a);
                db.run('INSERT INTO t (v) VALUES (?)', b);
                return a + '+' + b;
            });
            var rv = insertTwo('x', 'y');
            out.push('ret=' + rv);
            out.push('count=' + db.prepare('SELECT COUNT(*) AS n FROM t').get().n);
            out.push('inTx=' + db.inTransaction);

            // B2 transaction rollback on throw: nothing persisted
            var boom = db.transaction(function() {
                db.run("INSERT INTO t (v) VALUES ('z')");
                throw new Error('rollback-me');
            });
            try { boom(); } catch (e) { out.push('threw=' + (e.message === 'rollback-me')); }
            out.push('count-after-rollback=' + db.prepare('SELECT COUNT(*) AS n FROM t').get().n);

            // B3 nested transaction (savepoint): inner rollback only
            var outer = db.transaction(function() {
                db.run("INSERT INTO t (v) VALUES ('o')");
                var inner = db.transaction(function() {
                    db.run("INSERT INTO t (v) VALUES ('i1')");
                    throw new Error('inner-fail');
                });
                try { inner(); } catch (e) {}
                db.run("INSERT INTO t (v) VALUES ('o2')");
            });
            outer();
            out.push('nested=' + db.prepare("SELECT COUNT(*) AS n FROM t WHERE v LIKE 'o%'").get().n
                     + '/inner=' + db.prepare("SELECT COUNT(*) AS n FROM t WHERE v='i1'").get().n);

            // B4 deferred/immediate variants exist
            out.push('variants=' + (typeof insertTwo.deferred === 'function')
                     + ',' + (typeof insertTwo.immediate === 'function')
                     + ',' + (typeof insertTwo.exclusive === 'function'));

            // B5 iterate: for..of over params + next() undefined at end
            db.exec('CREATE TABLE u (id INTEGER, name TEXT)');
            var st = db.prepare('SELECT id, name FROM u WHERE id > ? ORDER BY id');
            db.exec("INSERT INTO u VALUES (1,'a'),(2,'b'),(3,'c'),(4,'d')");
            var names = [];
            for (var row of st.iterate(1)) { names.push(row.name); }
            out.push('iter=' + names.join(','));
            var it = db.prepare('SELECT id FROM u WHERE id > ?').iterate(3);
            var n1 = it.next(), n2 = it.next(), n3 = it.next();
            out.push('next1=' + (n1.done ? 'D' : n1.value.id)
                     + ',next2=' + (n2.done ? 'D' : n2.value.id)
                     + ',next3=' + (n3.done ? 'D' : n3.value.id));

            // B6 serialize → Buffer with SQLite magic
            var ser = db.serialize();
            var magic = String.fromCharCode(ser[0]) + String.fromCharCode(ser[1]) + String.fromCharCode(ser[2]);
            out.push('ser=' + (typeof ser) + ',magic=' + (magic === 'SQL') + ',len>0=' + (ser.length > 0));

            // B7 backup to file → reopen → data present
            var path = '/tmp/bao-sqlite-backup-e2e-' + process.pid + '.db';
            db.backup(path);
            var db2 = new Database(path);
            out.push('backup=' + db2.prepare("SELECT COUNT(*) AS n FROM u WHERE name='d'").get().n);

            return out.join(' | ');
        })()
        "#,
    );
    let expected_fragments = [
        "ret=x+y",
        "count=2",
        "inTx=false",
        "threw=true",
        "count-after-rollback=2",
        "nested=2/inner=0",
        "variants=true,true,true",
        "iter=b,c,d",
        "next1=4,next2=D,next3=D",
        "ser=object,magic=true,len>0=true",
        "backup=1",
    ];
    for frag in expected_fragments {
        assert!(
            sqlite_out.contains(frag),
            "bun:sqlite missing fragment {:?} in: {}",
            frag,
            sqlite_out
        );
    }

    // ── C. bun:ffi dlopen(path, symbols) + CString + toBuffer ───────────────
    let ffi_out = eval_string(
        &mut ctx,
        r#"
        (function() {
            var ffi = require('bun:ffi');
            var out = [];
            var libc = ffi.dlopen('libc.so.6', {
                atoi:   { args: [ffi.types.cstring], returns: ffi.types.i32 },
                strlen: { args: [ffi.types.cstring], returns: ffi.types.usize },
                strdup: { args: [ffi.types.cstring], returns: ffi.types.ptr },
            });
            out.push('atoi=' + libc.atoi('42'));
            out.push('strlen=' + libc.strlen('abc'));
            var p = libc.strdup('hello-ffi');
            out.push('dup-nonzero=' + (p > 0));
            var cs = new ffi.CString(p);
            out.push('cstring=' + cs.toString() + ',len=' + cs.length);
            var buf = ffi.toBuffer(p, 9);
            out.push('tobuffer=' + (buf[0] === 104 /* 'h' */) + ',' + (buf[8] === 105 /* 'i' */) + ',isbuf=' + (typeof buf.subarray === 'function'));
            // string-name type descriptors also accepted
            var l2 = ffi.dlopen('libc.so.6', { abs: { args: ['i32'], returns: 'i32' } });
            out.push('abs=' + l2.abs(-7));
            // suffix table present (Bun parity)
            out.push('suffix=' + (ffi.suffix.so === '.so'));
            return out.join(' | ');
        })()
        "#,
    );
    for frag in [
        "atoi=42",
        "strlen=3",
        "dup-nonzero=true",
        "cstring=hello-ffi,len=9",
        "tobuffer=true,true,isbuf=true",
        "abs=7",
        "suffix=true",
    ] {
        assert!(
            ffi_out.contains(frag),
            "bun:ffi missing fragment {:?} in: {}",
            frag,
            ffi_out
        );
    }

    // ── D. EventSource (SSE over fetch) ─────────────────────────────────────
    let sse_body = "event: greet\ndata: first\n\nretry: 50\ndata: second\n\nid: 7\ndata: third\n\n";
    let sse_resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        sse_body.len(),
        sse_body
    );
    // Static lifetime: leak the formatted response (test process exits anyway).
    let sse_static: &'static str = Box::leak(sse_resp.into_boxed_str());
    let (sse_port, _cap) = start_oneshot_server(sse_static);
    let es_setup = eval_string(
        &mut ctx,
        &format!(
            r#"
            (function() {{
                globalThis.__es = {{ msgs: [], opened: 0, errs: [], named: 0 }};
                var es = new EventSource('http://127.0.0.1:{port}/sse');
                es.onopen = function() {{ globalThis.__es.opened++; }};
                es.onerror = function(e) {{ globalThis.__es.errs.push(e && e.message || 'err'); }};
                es.onmessage = function(ev) {{ globalThis.__es.msgs.push(ev.data + (ev.lastEventId ? '@' + ev.lastEventId : '')); }};
                es.addEventListener('greet', function(ev) {{ globalThis.__es.named++; }});
                globalThis.__es_close = function() {{ es.close(); }};
                return 'ok';
            }})()
            "#,
            port = sse_port
        ),
    );
    assert_eq!(es_setup, "ok", "EventSource construction failed");
    let es_status = drive_until(
        &mut ctx,
        r#"JSON.stringify(globalThis.__es.msgs)"#,
        |s| s.contains("third"),
        Duration::from_secs(20),
    );
    eval_string(&mut ctx, r#"globalThis.__es_close()"#); // cancel reconnect timer
    let es_dump = eval_string(
        &mut ctx,
        r#"globalThis.__es.opened + '/' + globalThis.__es.named + '/' + JSON.stringify(globalThis.__es.msgs)"#,
    );
    assert!(
        es_status.contains("second") && es_status.contains("third@7"),
        "EventSource messages wrong: {} (status {})",
        es_dump,
        es_status
    );
    assert_eq!(
        es_dump, "1/1/[\"second\",\"third@7\"]",
        "EventSource open/named/message counts wrong: {}",
        es_dump
    );

    // ── E. localStorage (CLI, persisted) ────────────────────────────────────
    let ls_out = eval_string(
        &mut ctx,
        r#"
        (function() {
            var out = [];
            localStorage.setItem('k1', 'v1');
            localStorage.setItem('k2', 'v2');
            out.push('get=' + localStorage.getItem('k1'));
            out.push('len=' + localStorage.length);
            out.push('key0=' + localStorage.key(0) + ',key9=' + localStorage.key(9));
            localStorage.removeItem('k1');
            out.push('after-rm=' + localStorage.getItem('k1') + ',len=' + localStorage.length);
            localStorage.clear();
            out.push('after-clear=' + localStorage.length);
            localStorage.setItem('persist', 'me');
            return out.join(' | ');
        })()
        "#,
    );
    assert_eq!(
        ls_out,
        "get=v1 | len=2 | key0=k1,key9=null | after-rm=null,len=1 | after-clear=0",
        "localStorage surface wrong: {}",
        ls_out
    );
    let ls_file = tmp_home.join(".bao/localstorage.json");
    let ls_text = std::fs::read_to_string(&ls_file)
        .unwrap_or_else(|e| panic!("localStorage persistence file missing ({}): {}", ls_file.display(), e));
    assert!(
        ls_text.contains("\"persist\"") && ls_text.contains("\"me\""),
        "localStorage file content wrong: {}",
        ls_text
    );

    // ── F. fetch() Response surface (WHATWG Headers via the wire) ───────────
    let wire_body = r#"{"ok":true,"n":7}"#;
    let wire_resp = format!(
        "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nX-Wire-A: wa\r\nX-Wire-B: wb\r\nSet-Cookie: a=1\r\nSet-Cookie: b=2\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        wire_body.len(),
        wire_body
    );
    let wire_static: &'static str = Box::leak(wire_resp.into_boxed_str());
    let (wire_port, _wcap) = start_oneshot_server(wire_static);
    let f_setup = eval_string(
        &mut ctx,
        &format!(
            r#"
            (function() {{
                globalThis.__f = {{ r: null, errs: [] }};
                fetch('http://127.0.0.1:{port}/w')
                .then(function(r) {{
                    globalThis.__f.r = {{
                        instanceof: (r instanceof Response),
                        status: r.status,
                        ok: r.ok,
                        statusText: r.statusText,
                        ct: r.headers.get('content-type'),
                        xa: r.headers.get('X-WIRE-A'.toLowerCase()),
                        has: r.headers.has('x-wire-b'),
                        cookies: r.headers.get('set-cookie'),
                        json: null, abLen: -1, textLen: -1
                    }};
                    var j = r.json().then(function(o) {{ globalThis.__f.r.json = o.ok + '/' + o.n; }});
                    return j;
                }})
                .catch(function(e) {{ globalThis.__f.errs.push(e && e.message || String(e)); }});
                return 'scheduled';
            }})()
            "#,
            port = wire_port
        ),
    );
    assert!(f_setup.contains("scheduled"));
    let f_status = drive_until(
        &mut ctx,
        r#"(globalThis.__f.r && globalThis.__f.r.json) ? 'DONE' : 'PEND'"#,
        |s| s == "DONE",
        Duration::from_secs(20),
    );
    assert_eq!(f_status, "DONE");
    let f_dump = eval_string(&mut ctx, r#"JSON.stringify(globalThis.__f.r) + '|' + JSON.stringify(globalThis.__f.errs)"#);
    for frag in [
        "\"instanceof\":true",
        "\"status\":201",
        "\"ok\":true",
        "\"statusText\":\"Created\"",
        "\"ct\":\"application/json\"",
        "\"xa\":\"wa\"",
        "\"has\":true",
        "\"cookies\":\"a=1, b=2\"",
        "\"json\":\"true/7\"",
    ] {
        assert!(
            f_dump.contains(frag),
            "fetch Response surface missing {:?} in: {}",
            frag,
            f_dump
        );
    }

    eprintln!("[PASS] platform-surface e2e: crypto.subtle(10) sqlite(7) ffi EventSource localStorage fetch-Response");

    bun_http::http_thread::shutdown_for_exit();
    bun_runtime::shutdown_thread_sm();
    std::process::exit(0);
}
