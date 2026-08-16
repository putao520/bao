// @trace REQ-ENG-007 [level:integration]
// BCE-20260816 node stdlib wave — one labelled-check bundle per audit item.
// Covers: fs.readSync fd roundtrip, fs.glob (bun_glob engine, cwd/dot),
// stream.pipeline iterable sources + error throw, events emit('error'),
// os.networkInterfaces, url.resolve (RFC 3986), querystring escape %20,
// string_decoder partial UTF-8, net data Buffer (e2e in net suites),
// fs.truncateSync/opendirSync, path.platform.
#[path = "conformance_common.rs"] mod common;

use bao_engine::context::JsContext;
use common::{make_ctx, CHECK_SCAFFOLD};

/// run_checks with a false-green guard: a top-level JS throw makes
/// eval_string return "" which plain run_checks accepts as "no failures"
/// (that is exactly how the EINVAL truncate bug first slipped through —
/// the suite went green while its first statement crashed). Strict variant
/// fails when no check output was produced at all.
fn run_checks_strict(ctx: &mut JsContext, source: &str) {
    let results = common::eval_string(ctx, source);
    assert!(
        results.contains(":PASS") || results.contains(":FAIL") || results.contains(":ERROR:"),
        "suite produced no check output — top-level JS error? raw: {:?}",
        results
    );
    let mut failures = Vec::new();
    for item in results.split('|') {
        if item.is_empty() {
            continue;
        }
        if !item.contains(":PASS") {
            failures.push(item.to_string());
        }
    }
    assert!(
        failures.is_empty(),
        "conformance failures:\n  {}\nFull results: {}",
        failures.join("\n  "),
        results
    );
}

#[test]
fn fs_readsync_fd_roundtrip() {
    let mut ctx = make_ctx();
    let tmp = ::std::env::temp_dir().join("bao_wave_readsync.txt");
    ::std::fs::write(&tmp, b"HELLO-PROTO").unwrap();
    let p = common::js_path(&tmp);
    run_checks_strict(&mut ctx, &format!(r#"{CHECK_SCAFFOLD}
        var fs = require('fs');
        var fd = fs.openSync("{p}", 'r');
        check('open-fd', function() {{ return fd > 0; }});
        var buf = Buffer.alloc(12);
        var n = fs.readSync(fd, buf, 0, 12, 0);
        check('bytes-read', function() {{ return n === 11; }});
        check('buf-filled', function() {{ return buf.toString('utf8', 0, n) === 'HELLO-PROTO'; }});
        // offset + short length window
        var b2 = Buffer.alloc(8);
        var n2 = fs.readSync(fd, b2, 2, 4, 0);
        check('offset-window', function() {{
            return n2 === 4 && b2.toString('utf8', 2, 6) === 'HELL' && b2[0] === 0 && b2[1] === 0;
        }});
        // read at EOF position returns 0 (position arg = 11, file is 11 bytes)
        var b3 = Buffer.alloc(4);
        var n3 = fs.readSync(fd, b3, 0, 4, 11);
        check('after-eof', function() {{ return n3 === 0; }});
        fs.closeSync(fd);
        results.join('|');
    "#));
}

#[test]
fn fs_glob_cwd_and_dot() {
    let mut ctx = make_ctx();
    let dir = ::std::env::temp_dir().join("bao_wave_glob");
    let _ = ::std::fs::remove_dir_all(&dir);
    ::std::fs::create_dir_all(dir.join("a/b")).unwrap();
    ::std::fs::write(dir.join("a/b/x.ts"), b"1").unwrap();
    ::std::fs::write(dir.join("a/b/y.txt"), b"2").unwrap();
    ::std::fs::write(dir.join(".hidden.ts"), b"3").unwrap();
    let p = common::js_path(&dir);
    run_checks_strict(&mut ctx, &format!(r#"{CHECK_SCAFFOLD}
        var fs = require('fs');
        var ts = fs.globSync('**/*.ts', {{ cwd: "{p}" }});
        check('cwd-honored', function() {{
            return ts.length === 1 && ts[0] === 'a/b/x.ts';
        }});
        var all = fs.globSync('**/*', {{ cwd: "{p}", dot: true }});
        check('dot-includes-hidden', function() {{
            return all.indexOf('.hidden.ts') !== -1 && all.indexOf('a/b/y.txt') !== -1;
        }});
        var nodot = fs.globSync('**/*.ts', {{ cwd: "{p}", dot: false }});
        check('nodot-excludes-hidden', function() {{ return nodot.length === 1; }});
        // pattern array form
        var multi = fs.globSync(['a/b/x.ts', 'a/b/y.txt'], {{ cwd: "{p}" }});
        check('pattern-array', function() {{ return multi.length === 2; }});
        // exclude predicate
        var exc = fs.globSync('**/*.ts', {{ cwd: "{p}", exclude: function(x) {{ return x.indexOf('x.ts') !== -1; }} }});
        check('exclude-fn', function() {{ return exc.length === 0; }});
        results.join('|');
    "#));
    let _ = ::std::fs::remove_dir_all(&dir);
}

#[test]
fn fs_truncate_and_opendir_sync() {
    let mut ctx = make_ctx();
    let tmp = ::std::env::temp_dir().join("bao_wave_trunc");
    ::std::fs::create_dir_all(&tmp).unwrap();
    let f = tmp.join("t.txt");
    ::std::fs::write(&f, b"0123456789").unwrap();
    let d = tmp.join("sub");
    ::std::fs::create_dir_all(&d).unwrap();
    ::std::fs::write(d.join("one.js"), b"1").unwrap();
    ::std::fs::write(d.join("two.js"), b"2").unwrap();
    let fp = common::js_path(&f);
    let dp = common::js_path(&d);
    run_checks_strict(&mut ctx, &format!(r#"{CHECK_SCAFFOLD}
        var fs = require('fs');
        check('truncateSync-fn', function() {{ return typeof fs.truncateSync === 'function'; }});
        fs.truncateSync("{fp}", 4);
        check('truncateSync-effect', function() {{ return fs.readFileSync("{fp}", 'utf8') === '0123'; }});
        fs.truncateSync("{fp}");
        check('truncateSync-default-0', function() {{ return fs.readFileSync("{fp}", 'utf8') === ''; }});
        check('opendirSync-fn', function() {{ return typeof fs.opendirSync === 'function'; }});
        var dir = fs.opendirSync("{dp}");
        var names = [];
        var ent;
        while ((ent = dir.readSync()) !== null) names.push(ent.name);
        names.sort();
        dir.closeSync();
        check('opendirSync-entries', function() {{ return names.join(',') === 'one.js,two.js'; }});
        check('dir-path', function() {{ return dir.path === "{dp}"; }});
        results.join('|');
    "#));
    let _ = ::std::fs::remove_dir_all(&tmp);
}

// pipeline sources test: the completion callbacks land in microtasks, so the
// suite runs under a post-eval drain hook and the results are polled until
// all four async chains report — asserting on the synchronous eval return
// alone would never see them (async-assertion escape, false green).
#[test]
fn stream_pipeline_iterable_sources() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx.set_post_eval_hook(bounded_drain_hook);

    let setup = r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + ':' + (ok ? 'PASS' : 'FAIL')); }
            catch(e) { results.push(label + ':ERROR:' + (e && e.message ? e.message : e)); }
        }
        var stream = require('stream');
        var pipeline = stream.pipeline, Readable = stream.Readable, Writable = stream.Writable;
        check('pipeline-fn', function() { return typeof pipeline === 'function'; });
        globalThis.__results = results;
        // async generator source (the crash case: "streams[i].on is not a function")
        var gout = [];
        pipeline(
            (async function*() { yield 'g1'; yield 'g2'; })(),
            new Writable({ write: function(c, e, cb) { gout.push(c.toString()); cb(); } }),
            function(err) {
                results.push('async-gen:' + (err ? 'ERROR:' + err.message : (gout.join(',') === 'g1,g2' ? 'PASS' : 'FAIL:' + gout.join(','))));
            }
        );
        // plain array iterable source
        var aout = [];
        pipeline(
            ['p1', 'p2'],
            new Writable({ write: function(c, e, cb) { aout.push(c.toString()); cb(); } }),
            function(err) {
                results.push('array-src:' + (err ? 'ERROR:' + err.message : (aout.join(',') === 'p1,p2' ? 'PASS' : 'FAIL:' + aout.join(','))));
            }
        );
        // sync iterator object source
        var iout = [];
        pipeline(
            (function() { var i = 0; return { [Symbol.iterator]: function() { return { next: function() { return i < 2 ? { value: 'i' + (i++), done: false } : { done: true }; } }; } }; })(),
            new Writable({ write: function(c, e, cb) { iout.push(c.toString()); cb(); } }),
            function(err) {
                results.push('iter-src:' + (err ? 'ERROR:' + err.message : (iout.join(',') === 'i0,i1' ? 'PASS' : 'FAIL:' + iout.join(','))));
            }
        );
        // non-stream garbage reaches the callback as an error, not a crash
        var badErr = null;
        pipeline(12345, new Writable({ write: function(c, e, cb) { cb(); } }), function(err) { badErr = err; });
        check('bad-source-cb-error', function() { return badErr instanceof Error; });
        // classic stream chain still works
        var tout = [];
        pipeline(
            Readable.from(['a', 'b']),
            new stream.Transform({ transform: function(c, e, cb) { cb(null, c.toUpperCase()); } }),
            new Writable({ write: function(c, e, cb) { tout.push(c.toString()); cb(); } }),
            function(err) {
                results.push('classic:' + (err ? 'ERROR:' + err.message : (tout.join(',') === 'A,B' ? 'PASS' : 'FAIL:' + tout.join(','))));
            }
        );
    "#;
    ctx.eval(setup, "<pipeline-setup>").expect("pipeline setup eval");

    // Drain until all four async completion labels arrived.
    let mut final_results = String::new();
    for _ in 0..100 {
        HOOK_BUDGET.with(|b| b.set(30));
        let out = ctx
            .eval(
                "globalThis.__results.join('|')",
                "<poll>",
            )
            .ok()
            .and_then(|v| match v {
                bao_engine::value::JsValue::String(s) => Some(s),
                _ => None,
            })
            .unwrap_or_default();
        let done = out.matches("async-gen:").count() >= 1
            && out.matches("array-src:").count() >= 1
            && out.matches("iter-src:").count() >= 1
            && out.matches("classic:").count() >= 1;
        if done {
            final_results = out;
            break;
        }
        ::std::thread::sleep(::std::time::Duration::from_millis(2));
    }
    assert!(
        !final_results.is_empty(),
            "pipeline async chains never completed — raw results: {:?}",
            final_results
    );
    let mut failures: Vec<&str> = final_results
        .split('|')
        .filter(|item| !item.is_empty() && !item.contains(":PASS"))
        .collect();
    if let Some(last) = final_results.split('|').last() {
        if last.is_empty() {
            failures.pop();
        }
    }
    assert!(
        failures.is_empty(),
        "pipeline failures:\n  {}\nFull results: {}",
        failures.join("\n  "),
        final_results
    );
}

#[test]
fn events_error_emit_semantics() {
    let mut ctx = make_ctx();
    run_checks_strict(&mut ctx, &format!(r#"{CHECK_SCAFFOLD}
        var EventEmitter = require('events').EventEmitter;
        // no listener → throws the error
        var threw = null;
        try {{ new EventEmitter().emit('error', new Error('boom')); }} catch (e) {{ threw = e; }}
        check('error-no-listener-throws', function() {{ return threw !== null && threw.message === 'boom'; }});
        // with listener → delivered, no throw
        var got = null;
        var ee2 = new EventEmitter();
        ee2.on('error', function(e) {{ got = e; }});
        ee2.emit('error', new Error('handled'));
        check('error-with-listener', function() {{ return got !== null && got.message === 'handled'; }});
        // non-error events without listeners stay silent
        check('other-event-silent', function() {{ return new EventEmitter().emit('data', 1) === false; }});
        // stream EE mirrors the same rule
        var sThrew = null;
        try {{
            var r = new (require('stream').Readable)({{ read: function() {{}} }});
            r.emit('error', new Error('stream-boom'));
        }} catch (e) {{ sThrew = e; }}
        check('stream-ee-error-throws', function() {{ return sThrew !== null && sThrew.message === 'stream-boom'; }});
        results.join('|');
    "#));
}

// emit('error') throw routing — verified in TIMER-DISPATCH context (the
// production pump): an uncaught throw from a setTimeout callback is routed
// through route_uncaught_exception by the timer dispatcher, so a registered
// process.on('uncaughtException') observes it. Same wire as the CLI.
thread_local! {
    static HOOK_BUDGET: ::std::cell::Cell<usize> = const { ::std::cell::Cell::new(0) };
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

#[test]
fn events_error_routes_to_uncaught_handler() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx.set_post_eval_hook(bounded_drain_hook);

    ctx.eval(r#"
        globalThis.__routed = null;
        process.on('uncaughtException', function(e) { globalThis.__routed = e.message; });
        var EventEmitter = require('events').EventEmitter;
        setTimeout(function() {
            new EventEmitter().emit('error', new Error('routed-boom'));
        }, 0);
    "#, "<setup>").expect("setup eval");

    for _ in 0..40 {
        HOOK_BUDGET.with(|b| b.set(20));
        let got = ctx.eval("globalThis.__routed === null ? 'n' : globalThis.__routed", "<poll>")
            .ok()
            .and_then(|v| match v {
                bao_engine::value::JsValue::String(s) => Some(s),
                _ => None,
            });
        if let Some(s) = got {
            if s != "n" {
                assert_eq!(s, "routed-boom", "uncaughtException must receive the emit('error') throw");
                return;
            }
        }
        ::std::thread::sleep(::std::time::Duration::from_millis(2));
    }
    panic!("emit('error') throw never reached the uncaughtException handler");
}

#[test]
fn os_network_interfaces_real() {
    let mut ctx = make_ctx();
    run_checks_strict(&mut ctx, &format!(r#"{CHECK_SCAFFOLD}
        var os = require('os');
        var nis = os.networkInterfaces();
        var names = Object.keys(nis);
        check('non-empty', function() {{ return names.length > 0; }});
        check('has-loopback', function() {{ return names.indexOf('lo') !== -1; }});
        var lo = nis['lo'];
        check('lo-entries', function() {{ return Array.isArray(lo) && lo.length > 0; }});
        check('lo-internal', function() {{ return lo.every(function(e) {{ return e.internal === true; }}); }});
        check('entry-shape', function() {{
            return lo.every(function(e) {{
                return typeof e.address === 'string' && typeof e.netmask === 'string' &&
                    (e.family === 'IPv4' || e.family === 'IPv6') &&
                    typeof e.mac === 'string' && typeof e.cidr === 'string';
            }});
        }});
        check('lo-v4-loopback', function() {{
            return lo.some(function(e) {{ return e.family === 'IPv4' && e.address === '127.0.0.1' && e.cidr === '127.0.0.1/8'; }});
        }});
        results.join('|');
    "#));
}

#[test]
fn url_resolve_rfc3986() {
    let mut ctx = make_ctx();
    run_checks_strict(&mut ctx, &format!(r#"{CHECK_SCAFFOLD}
        var resolve = require('url').resolve;
        check('path-base', function() {{ return resolve('/one/two/three', 'four') === '/one/two/four'; }});
        check('dotdot', function() {{ return resolve('http://a/b/c', '../d') === 'http://a/d'; }});
        check('absolute-ref', function() {{ return resolve('http://a/b', 'http://x/y') === 'http://x/y'; }});
        check('root-ref', function() {{ return resolve('http://example.com/one', '/two') === 'http://example.com/two'; }});
        check('empty-ref', function() {{ return resolve('http://a/b/c?q', '') === 'http://a/b/c?q'; }});
        check('query-ref', function() {{ return resolve('http://a/b/c', '?q=1') === 'http://a/b/c?q=1'; }});
        check('scheme-merge', function() {{ return resolve('http://a', '//b/c') === 'http://b/c'; }});
        check('over-dots', function() {{
            return resolve('http://example.com/one/two/three', '../../../four') === 'http://example.com/four';
        }});
        check('frag', function() {{ return resolve('http://a/b', '#f') === 'http://a/b#f'; }});
        results.join('|');
    "#));
}

#[test]
fn querystring_escape_pct20() {
    let mut ctx = make_ctx();
    run_checks_strict(&mut ctx, &format!(r#"{CHECK_SCAFFOLD}
        var qs = require('querystring');
        check('escape-space', function() {{ return qs.escape('a b') === 'a%20b'; }});
        check('stringify-space', function() {{ return qs.stringify({{ q: 'x y' }}) === 'q=x%20y'; }});
        check('stringify-multi', function() {{ return qs.stringify({{ a: '1', b: '2' }}) === 'a=1&b=2'; }});
        check('stringify-array', function() {{ return qs.stringify({{ k: ['1', '2'] }}) === 'k=1&k=2'; }});
        check('parse-plus-as-space', function() {{ return qs.parse('q=x+y')['q'] === 'x y'; }});
        check('parse-pct20', function() {{ return qs.parse('q=x%20y')['q'] === 'x y'; }});
        check('roundtrip', function() {{ return qs.parse(qs.stringify({{ m: 'hello world' }}))['m'] === 'hello world'; }});
        check('unescape', function() {{ return qs.unescape('a%20b') === 'a b'; }});
        results.join('|');
    "#));
}

#[test]
fn string_decoder_partial_multibyte() {
    let mut ctx = make_ctx();
    run_checks_strict(&mut ctx, &format!(r#"{CHECK_SCAFFOLD}
        var StringDecoder = require('string_decoder').StringDecoder;
        var d = new StringDecoder('utf8');
        var s1 = d.write(Buffer.from([0xe4, 0xb8]));
        check('partial-empty', function() {{ return s1 === ''; }});
        var s2 = d.write(Buffer.from([0xad]));
        check('completed-char', function() {{ return s2 === '中'; }});
        // 4-byte emoji split across three writes
        var d2 = new StringDecoder('utf8');
        var e1 = d2.write(Buffer.from([0xf0, 0x9f]));
        var e2 = d2.write(Buffer.from([0x98, 0x80]));
        check('emoji-split', function() {{ return e1 === '' && e2 === '😀'; }});
        // end flushes orphan bytes as U+FFFD (Node semantics)
        var d3 = new StringDecoder('utf8');
        d3.write(Buffer.from([0xe4, 0xb8]));
        check('end-flushes-fffd', function() {{ return d3.end() === '�'; }});
        // end with complete data
        var d4 = new StringDecoder('utf8');
        check('end-with-buffer', function() {{ return d4.end(Buffer.from('ok')) === 'ok'; }});
        // text() joins the pending bytes
        var d5 = new StringDecoder('utf8');
        d5.text(Buffer.from([0xe4, 0xb8]), 0);
        check('text-continues', function() {{ return d5.text(Buffer.from([0xad]), 0) === '中'; }});
        // non-utf8 encoding passes through without hanging
        var d6 = new StringDecoder('hex');
        check('hex-passthrough', function() {{ return d6.write(Buffer.from([0x41, 0x42])) === '4142'; }});
        results.join('|');
    "#));
}

#[test]
fn path_platform_property() {
    let mut ctx = make_ctx();
    run_checks_strict(&mut ctx, &format!(r#"{CHECK_SCAFFOLD}
        var path = require('path');
        var os = require('os');
        check('platform-string', function() {{ return typeof path.platform === 'string'; }});
        check('platform-matches-os', function() {{ return path.platform === os.platform(); }});
        check('platform-value', function() {{ return path.platform === 'linux' || path.platform === 'darwin' || path.platform === 'win32'; }});
        check('posix-selfref', function() {{ return path.posix.sep === '/'; }});
        results.join('|');
    "#));
}
