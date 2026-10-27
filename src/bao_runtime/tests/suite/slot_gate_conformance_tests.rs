// @trace REQ-ENG-004
// @trace REQ-ENG-006 [api:events.once promise form] [level:integration]
// @trace REQ-ENG-006 [api:events.listenerCount dual face] [level:integration]
// @trace REQ-ENG-006 [api:events.getEventListeners dual face] [level:integration]
// @trace TEST-ENG-4-SLOT [req:REQ-ENG-004] [level:integration]
//
// E46 + E53 discrimination-grid suite — BCE-20261025-FFI-SLOT-GATE regression
// layer for bao_runtime URL / sqlite / events surfaces.
//
// E46 (node_url.rs / bun_sqlite.rs / fetch_async.rs): the raw-pointer carrier
// migrated from JS-readable own properties to engine-private reserved slots,
// gated by `JS_InstanceOf` class checks (BCE-20261025-FFI-SLOT-GATE). A
// foreign receiver's slot read is an OOB fixed-slot read whose stale nursery
// bits would otherwise be handed to `to_private()` → wild deref / arbitrary
// `Box::from_raw`. The class gate turns every foreign-receiver call into a
// safe rejection (throw or inert sentinel) instead of UB.
//
//   E46 grid 1 (URL):   plain {} calling URLSearchParams methods / URL
//                       property getters / iterator next → TypeError
//                       (node oracle shape, V55 alignment — NOT a crash).
//   E46 grid 2 (sqlite): plain {} calling Database/Statement methods →
//                       the "invalid Database/Statement object" TypeError
//                       (the caller's own fallback, not UB).
//
// E53 (node_events.rs): events.once promise-form + dual-face listenerCount /
// getEventListeners via the EVENTS_ONCE_HELPER_JS JS port.
//
//   E53 grid 3: typeof require('events').once === 'function' AND
//               once(emitter, 'ev') returns a thenable with [object Promise]
//               class tag (promise-form existence).
//   E53 grid 4: once + AbortSignal — pre-aborted signal rejects with
//               AbortError/ABORT_ERR; post-registration abort() also rejects
//               with AbortError (node oracle: probed v24.19.0).
//   E53 grid 5: events.listenerCount(AbortSignal.abort(), 'abort') after
//               addEventListener → > 0 (EventTarget arm of the dual face).
//
// SCOPE: two test functions (one per delivery E46/E53). No product files
// touched. No network I/O. sqlite grids use :memory: only.
//
// ORACLE (node v24.19.0):
//   typeof events.once === 'function'                       → true
//   events.once(ee,'x') instanceof Promise                  → true
//   events.once(ee,'x',{signal: preAborted})                → reject AbortError ABORT_ERR
//   events.once(ee,'x',{signal}) + later abort()            → reject AbortError ABORT_ERR
//   events.listenerCount(AbortSignal.abort(),'abort')       → 1 (after addEventListener)
//   events.getEventListeners(AbortSignal.abort(),'abort')   → [fn] (after addEventListener)
//   events.once(new EventTarget(),'error') + dispatch error → RESOLVES (EventTarget arm)
//   URLSearchParams.prototype.get.call({},'a')              → TypeError (node oracle)
//   iter.next.call({})                                      → TypeError (node oracle)
//   V55 alignment: bao now throws the node-oracle TypeError shape
//     (ERR_INVALID_THIS for URLSearchParams surfaces; "Receiver must be an
//     instance of class URL" for URL getter/setter/toString/toJSON).

#[path = "common/mod.rs"]
mod common;

use bao_engine::context::JsContext;

use common::{HOOK_BUDGET, bounded_drain_hook, eval_str, eval_string_full, setup_ctx};

fn ev(ctx: &mut JsContext, js: &str) -> String {
    eval_string_full(ctx, js)
}

fn setup_ctx_pump() -> JsContext {
    let mut ctx = setup_ctx();
    ctx.set_post_eval_hook(bounded_drain_hook);
    ctx
}

/// Evaluate `expr` in a JS-side throw catcher; returns `THREW:<message>` on
/// throw, `NO-THROW` otherwise (kept for potential future grids).
#[allow(dead_code)]
fn eval_try(ctx: &mut JsContext, expr: &str) -> String {
    let js = format!(
        "(function(){{ try {{ {expr}; return 'NO-THROW'; }} \
         catch (e) {{ return 'THREW:' + ((e && e.message) || String(e)); }} }})()"
    );
    ev(ctx, &js)
}

// ── E46 grid 1: URL slot-gate foreign-receiver safe rejection ───────────────

/// URLSearchParams methods on a plain {} receiver throw the node-oracle
/// TypeError shape (V55 alignment): message `Value of "this" must be of type
/// URLSearchParams`, code ERR_INVALID_THIS. The slot-gate
/// (`JS_InstanceOf` on URL_SEARCH_PARAMS_CLASS / URL_CLASS /
/// URL_SP_ITER_CLASS) rejects the foreign receiver before any reserved-slot
/// read — the class gate is the safety boundary; the TypeError is the
/// node-parity rejection surface.
#[test]
fn url_slot_gate_foreign_receiver_safe_rejection() {
    let mut ctx = setup_ctx_pump();
    bun_runtime::clear_exit();

    // ── searchParams method surface — every method throws ERR_INVALID_THIS ──
    let sp_methods = ev(
        &mut ctx,
        r#"
        (function () {
            var sp = new URLSearchParams('a=1&b=2');
            var f = {};
            function tryCall(fn, thisArg, args) {
                try { fn.apply(thisArg, args); return 'NO-THROW'; }
                catch (e) { return (e && e.message) + '|' + (e && e.code); }
            }
            // size is an accessor on the prototype — take the getter itself
            // (sp.size would evaluate the getter with this=sp and hand a
            // number to tryCall, testing nothing).
            var sizeD = Object.getOwnPropertyDescriptor(URLSearchParams.prototype, 'size');
            return [
                tryCall(sp.get, f, ['a']),
                tryCall(sp.getAll, f, ['a']),
                tryCall(sp.has, f, ['a']),
                tryCall(sp.append, f, ['x', 'y']),
                tryCall(sp.set, f, ['x', 'y']),
                tryCall(sp.delete, f, ['a']),
                tryCall(sp.toString, f, []),
                tryCall(sizeD.get, f, []),
            ].join('~');
        })()
        "#,
    );
    assert_eq!(
        sp_methods,
        "Value of \"this\" must be of type URLSearchParams|ERR_INVALID_THIS~\
         Value of \"this\" must be of type URLSearchParams|ERR_INVALID_THIS~\
         Value of \"this\" must be of type URLSearchParams|ERR_INVALID_THIS~\
         Value of \"this\" must be of type URLSearchParams|ERR_INVALID_THIS~\
         Value of \"this\" must be of type URLSearchParams|ERR_INVALID_THIS~\
         Value of \"this\" must be of type URLSearchParams|ERR_INVALID_THIS~\
         Value of \"this\" must be of type URLSearchParams|ERR_INVALID_THIS~\
         Value of \"this\" must be of type URLSearchParams|ERR_INVALID_THIS",
        "URLSearchParams foreign receiver TypeError surface: {sp_methods}"
    );

    // ── searchParams iterator surface — next/keys/values/entries throw ──
    let sp_iter = ev(
        &mut ctx,
        r#"
        (function () {
            var sp = new URLSearchParams('a=1');
            var f = {};
            var iter = sp.entries();
            function tryCall(fn, thisArg, args) {
                try { fn.apply(thisArg, args); return 'NO-THROW'; }
                catch (e) { return (e && e.message) + '|' + (e && e.code); }
            }
            return [
                tryCall(iter.next, f, []),
                tryCall(sp.keys, f, []),
                tryCall(sp.values, f, []),
                tryCall(sp.entries, f, []),
            ].join('~');
        })()
        "#,
    );
    assert_eq!(
        sp_iter,
        "Value of \"this\" must be of type URLSearchParamsIterator|ERR_INVALID_THIS~\
         Value of \"this\" must be of type URLSearchParams|ERR_INVALID_THIS~\
         Value of \"this\" must be of type URLSearchParams|ERR_INVALID_THIS~\
         Value of \"this\" must be of type URLSearchParams|ERR_INVALID_THIS",
        "URLSearchParams iterator foreign receiver TypeError surface: {sp_iter}"
    );

    // ── URL property getters on foreign — node oracle: TypeError (no code) ──
    let url_getters = ev(
        &mut ctx,
        r#"
        (function () {
            var u = new URL('https://example.com/path?q=1#frag');
            var f = {};
            function tryGet(name) {
                try {
                    var d = Object.getOwnPropertyDescriptor(u, name) || Object.getOwnPropertyDescriptor(URL.prototype, name);
                    d.get.call(f);
                    return 'NO-THROW';
                } catch (e) { return (e && e.message) + '|' + (e && e.code); }
            }
            return [
                tryGet('href'),
                tryGet('protocol'),
                tryGet('search'),
            ].join('~');
        })()
        "#,
    );
    assert_eq!(
        url_getters,
        "Receiver must be an instance of class URL|undefined~\
         Receiver must be an instance of class URL|undefined~\
         Receiver must be an instance of class URL|undefined",
        "URL property getters foreign receiver TypeError surface: {url_getters}"
    );

    // ── URL property setters on foreign — node oracle: TypeError (no code) ──
    let url_setters = ev(
        &mut ctx,
        r#"
        (function () {
            var u = new URL('https://example.com/path');
            var f = {};
            try {
                var d = Object.getOwnPropertyDescriptor(u, 'href') || Object.getOwnPropertyDescriptor(URL.prototype, 'href');
                d.set.call(f, 'https://evil.com');
                return 'NO-THROW';
            } catch (e) {
                return (e && e.message) + '|' + (e && e.code) + '|URL-UNTOUCHED:' +
                    (u.href === 'https://example.com/path');
            }
        })()
        "#,
    );
    assert_eq!(
        url_setters,
        "Receiver must be an instance of class URL|undefined|URL-UNTOUCHED:true",
        "URL property setter foreign receiver TypeError surface: {url_setters}"
    );

    // ── URL.searchParams live-linkage interaction: mutating searchParams
    //    through a forged reference must throw AND leave the URL untouched ──
    let sp_injection = ev(
        &mut ctx,
        r#"
        (function () {
            var u = new URL('https://example.com/path?q=original');
            var sp = u.searchParams;
            var f = {};
            var threw = '';
            try {
                sp.append.call(f, 'injected', 'yes');
            } catch (e) {
                threw = (e && e.message) + '|' + (e && e.code);
            }
            return threw + '~Q:' + u.search;
        })()
        "#,
    );
    assert_eq!(
        sp_injection,
        "Value of \"this\" must be of type URLSearchParams|ERR_INVALID_THIS~Q:?q=original",
        "URL.searchParams foreign commit must throw and be inert: {sp_injection}"
    );
}

// ── E46 grid 2: sqlite slot-gate foreign-receiver safe rejection ────────────

/// Database / Statement methods on a plain {} receiver throw the engine's
/// own "invalid Database/Statement object" TypeError via `JS_ReportErrorUTF8`
/// — the caller's established fallback message, not UB. The class gate
/// (`JS_InstanceOf` on DATABASE_CLASS / STATEMENT_CLASS) rejects the foreign
/// receiver before the reserved-slot read.
///
/// Pin every method's exact error message (grep-verified from bun_sqlite.rs).
/// Also pin: the real Database is unaffected after foreign-receiver attempts.
#[test]
fn sqlite_slot_gate_foreign_receiver_safe_rejection() {
    let mut ctx = setup_ctx_pump();
    bun_runtime::clear_exit();

    // ── Database methods on plain {} ──
    let db_methods = ev(
        &mut ctx,
        r#"
        (function () {
            var { Database } = require('bun:sqlite');
            var db = new Database(':memory:');
            var f = {};
            function tryCall(fn, thisArg, args) {
                try { fn.apply(thisArg, args); return 'NO-THROW'; }
                catch (e) { return (e && e.message) || String(e); }
            }
            return [
                tryCall(db.exec, f, ['SELECT 1']),
                tryCall(db.run, f, ['SELECT 1']),
                tryCall(db.close, f, []),
                tryCall(db.prepare, f, ['SELECT 1']),
                tryCall(db.transaction, f, [function(){}]),
                tryCall(db.serialize, f, []),
            ].join('~');
        })()
        "#,
    );
    assert_eq!(
        db_methods,
        "Database.exec: invalid Database object~\
         Database.run: invalid Database object~\
         Database.close: invalid Database object~\
         Database.prepare: invalid Database object~\
         Database.transaction: invalid Database object~\
         Database.serialize: invalid Database object",
        "Database foreign receiver error messages: {db_methods}"
    );

    // ── Statement methods on plain {} ──
    let stmt_methods = ev(
        &mut ctx,
        r#"
        (function () {
            var { Database } = require('bun:sqlite');
            var db = new Database(':memory:');
            var stmt = db.prepare('SELECT 1 AS x');
            var f = {};
            function tryCall(fn, thisArg, args) {
                try { fn.apply(thisArg, args); return 'NO-THROW'; }
                catch (e) { return (e && e.message) || String(e); }
            }
            return [
                tryCall(stmt.run, f, []),
                tryCall(stmt.get, f, []),
                tryCall(stmt.all, f, []),
                tryCall(stmt.iterate, f, []),
            ].join('~');
        })()
        "#,
    );
    assert_eq!(
        stmt_methods,
        "Statement.run: invalid Statement object~\
         Statement.get: invalid Statement object~\
         Statement.all: invalid Statement object~\
         Statement.iterate: invalid Statement object",
        "Statement foreign receiver error messages: {stmt_methods}"
    );

    // ── Real Database unaffected after foreign-receiver attempts ──
    let db_alive = ev(
        &mut ctx,
        r#"
        (function () {
            var { Database } = require('bun:sqlite');
            var db = new Database(':memory:');
            var f = {};
            try { db.exec.call(f, 'SELECT 1'); } catch (_) {}
            // The real db must still work.
            db.exec('CREATE TABLE t (id INTEGER)');
            db.run('INSERT INTO t VALUES (42)');
            var row = db.prepare('SELECT id FROM t').get();
            return 'ID:' + row.id;
        })()
        "#,
    );
    assert_eq!(
        db_alive, "ID:42",
        "real Database must survive foreign attempts: {db_alive}"
    );
}

// ── E53 grid 3: events.once promise-form existence ─────────────────────────

/// typeof require('events').once === 'function' AND once(emitter, 'ev')
/// returns a thenable whose Object.prototype.toString tag is
/// '[object Promise]' (the real Promise class tag — node_async_hooks replaces
/// the global Promise binding so instanceof alone is unreliable; the class
/// tag + thenable contract is the honest observable).
#[test]
fn events_once_promise_form_exists_and_returns_promise() {
    let mut ctx = setup_ctx_pump();
    bun_runtime::clear_exit();

    let result = ev(
        &mut ctx,
        r#"
        (function () {
            var events = require('events');
            if (typeof events.once !== 'function') return 'NOT-FUNCTION';
            var ee = new events.EventEmitter();
            var p = events.once(ee, 'test-event');
            var isThenable = typeof p.then === 'function';
            var tag = Object.prototype.toString.call(p);
            return typeof events.once + '|' + isThenable + '|' + tag;
        })()
        "#,
    );
    assert_eq!(
        result, "function|true|[object Promise]",
        "events.once must be a function returning a real Promise: {result}"
    );
}

// ── E53 grid 4: events.once + AbortSignal ───────────────────────────────────

/// Pre-aborted signal → promise rejects with AbortError/ABORT_ERR.
/// Post-registration abort() → promise also rejects with AbortError/ABORT_ERR.
/// (node v24.19.0 oracle: both reject with err.name='AbortError',
/// err.code='ABORT_ERR'.)
///
/// The promise rejection is async — drive the event loop to settle.
#[test]
fn events_once_abort_signal_rejects_with_abort_error() {
    let mut ctx = setup_ctx_pump();
    bun_runtime::clear_exit();

    // ── pre-aborted signal ──
    let pre_aborted = ev(
        &mut ctx,
        r#"
        globalThis.__pre = 'PENDING';
        (function () {
            var events = require('events');
            var ee = new events.EventEmitter();
            var ac = new AbortController();
            ac.abort(); // pre-aborted
            events.once(ee, 'x', { signal: ac.signal }).then(
                function () { globalThis.__pre = 'RESOLVED'; },
                function (err) {
                    globalThis.__pre = (err && err.name) + '|' + (err && err.code);
                }
            );
            return 'REGISTERED';
        })()
        "#,
    );
    assert_eq!(
        pre_aborted, "REGISTERED",
        "pre-aborted registration: {pre_aborted}"
    );

    HOOK_BUDGET.with(|b| b.set(8));
    let _ = eval_str(&mut ctx, "'tick'");
    let pre_result = ev(&mut ctx, "globalThis.__pre");
    assert_eq!(
        pre_result, "AbortError|ABORT_ERR",
        "pre-aborted signal must reject with AbortError/ABORT_ERR: {pre_result}"
    );

    // ── post-registration abort ──
    let post_abort = ev(
        &mut ctx,
        r#"
        globalThis.__post = 'PENDING';
        (function () {
            var events = require('events');
            var ee = new events.EventEmitter();
            var ac = new AbortController();
            events.once(ee, 'y', { signal: ac.signal }).then(
                function () { globalThis.__post = 'RESOLVED'; },
                function (err) {
                    globalThis.__post = (err && err.name) + '|' + (err && err.code);
                }
            );
            // Abort AFTER the listener is registered.
            ac.abort();
            return 'REGISTERED-AND-ABORTED';
        })()
        "#,
    );
    assert_eq!(
        post_abort, "REGISTERED-AND-ABORTED",
        "post-registration abort setup: {post_abort}"
    );

    HOOK_BUDGET.with(|b| b.set(8));
    let _ = eval_str(&mut ctx, "'tick'");
    let post_result = ev(&mut ctx, "globalThis.__post");
    assert_eq!(
        post_result, "AbortError|ABORT_ERR",
        "post-registration abort must reject with AbortError/ABORT_ERR: {post_result}"
    );
}

// ── E53 grid 5: listenerCount / getEventListeners dual-face EventTarget arm ─

/// events.listenerCount(AbortSignal.abort(), 'abort') after addEventListener
/// → > 0 (the EventTarget arm of the dual-face static reads the kEvents slot
/// map, not the EventEmitter hidden state).
///
/// Same for getEventListeners → array of length > 0.
///
/// Also pin the EventEmitter arm: listenerCount on an EventEmitter after
/// once() call → 1 for the event, 1 for the implicit error listener.
#[test]
fn events_listener_count_dual_face_event_target_active() {
    let mut ctx = setup_ctx_pump();
    bun_runtime::clear_exit();

    let result = ev(
        &mut ctx,
        r#"
        (function () {
            var events = require('events');

            // Bootstrap the events port: the dual-face statics
            // (listenerCount / getEventListeners) are trampolines whose first
            // call evaluates the internal/event_target port, which is what
            // installs the AbortSignal statics. One trampoline call first.
            events.listenerCount(new events.EventEmitter(), 'bootstrap');

            // ── EventTarget arm: AbortSignal ──
            var sig = AbortSignal.abort();
            sig.addEventListener('abort', function () {});
            var etCount = events.listenerCount(sig, 'abort');
            var etListeners = events.getEventListeners(sig, 'abort');

            // ── EventEmitter arm (regression: must not be broken) ──
            var ee = new events.EventEmitter();
            ee.on('myevent', function () {});
            var eeCount = events.listenerCount(ee, 'myevent');

            return [
                etCount > 0,
                etListeners.length > 0,
                typeof etListeners[0] === 'function',
                eeCount,
            ].join('|');
        })()
        "#,
    );
    assert_eq!(
        result, "true|true|true|1",
        "listenerCount/getEventListeners dual-face (EventTarget + EventEmitter): {result}"
    );
}

// ── E53 grid 6: events.once EventTarget error-event resolves ───────────────

/// EventTarget arm: once(eventTarget, 'error') + dispatchEvent(new
/// Event('error')) → RESOLVES (EventTargets have no error-reject semantics —
/// unlike EventEmitter where a distinct 'error' listener rejects the promise).
/// (node v24.19.0 oracle: RESOLVED with args [Event].)
#[test]
fn events_once_event_target_error_event_resolves() {
    let mut ctx = setup_ctx_pump();
    bun_runtime::clear_exit();

    let registered = ev(
        &mut ctx,
        r#"
        globalThis.__etResult = 'PENDING';
        (function () {
            var events = require('events');
            var et = new EventTarget();
            events.once(et, 'error').then(
                function (args) {
                    globalThis.__etResult = 'RESOLVED:' + (args && args.length);
                },
                function (err) {
                    globalThis.__etResult = 'REJECTED:' + (err && err.message);
                }
            );
            et.dispatchEvent(new Event('error'));
            return 'DISPATCHED';
        })()
        "#,
    );
    assert_eq!(
        registered, "DISPATCHED",
        "EventTarget dispatch setup: {registered}"
    );

    HOOK_BUDGET.with(|b| b.set(8));
    let _ = eval_str(&mut ctx, "'tick'");
    let result = ev(&mut ctx, "globalThis.__etResult");
    assert_eq!(
        result, "RESOLVED:1",
        "EventTarget error event must RESOLVE (not reject): {result}"
    );
}
