// @trace TEST-ENG-007 [req:REQ-ENG-007] [level:integration]
// Timer + async-crypto COEXISTENCE pump tests — the domain-check e661130391
// (own-idiom fix) starvation regression.
//
// Pre-fix wedge: with a timer registered, `drain_and_check`'s branch order
// selects the timer branch every pass; `wait_for_timer_deadline`'s raw
// `tick_with_timeout` drains the uWS loop (the ConcurrentTask wakeup eventfd
// included) but never POPS `MiniEventLoop.concurrent_tasks` — the pop lives
// in `tick_once`/`tick_without_idle` only. A completed `spawn_crypto_async`
// worker's tasklet therefore parks in the queue forever while
// `crypto_async_pending() > 0` keeps the loop alive:
// `await util.promisify(crypto.pbkdf2)(...)` hangs WITH the interval firing
// alongside. The fix supplements one non-blocking `tick_without_idle` in the
// timer branch when fs/crypto work is in flight (timers.rs).
//
// Both mandatory coexistence forms are covered: setInterval+pbkdf2 (via the
// generic promisify wrapper) and setInterval+argon2 (same spawn_crypto_async
// path, different op). The interval stays REGISTERED across the whole await
// window — exactly the state that selected the starving branch.

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

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

fn setup_ctx() -> JsContext {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx
}

/// Production-shaped pump (fs_async_callback_tests precedent): keep calling
/// `timers::drain_and_check` while the loop reports liveness. Pre-fix, the
/// verdict never turns false (interval registered + crypto pending forever)
/// and this spins to the deadline; post-fix the crypto tasklet is delivered,
/// the continuation clears the interval, and the verdict turns false.
fn pump_until_quiescent(ctx: &mut JsContext, deadline_ms: u64) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(deadline_ms);
    while std::time::Instant::now() < deadline {
        let mut cxm = ctx.cx();
        if !bun_runtime::timers::drain_and_check(&mut cxm) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// setInterval(fn, 50) registered + `await util.promisify(crypto.pbkdf2)`
/// (generic (err, value) wrapper): the key must arrive while the interval is
/// still registered, and the hex must equal the pbkdf2Sync oracle.
#[test]
fn timer_crypto_coexistence_interval_pbkdf2_promisified() {
    let mut ctx = setup_ctx();
    let out = eval_string(
        &mut ctx,
        r#"
        var crypto = require('crypto');
        var util = require('util');
        globalThis.__pk = { done: false, hex: null, err: null };
        var iv = setInterval(function() {}, 50);
        util.promisify(crypto.pbkdf2)('secret', 'salt', 1, 32, 'sha256').then(
            function(key) {
                globalThis.__pk.done = true;
                globalThis.__pk.hex = key.toString('hex');
                clearInterval(iv);
            },
            function(err) {
                globalThis.__pk.done = true;
                globalThis.__pk.err = String((err && err.message) || err);
                clearInterval(iv);
            }
        );
        'scheduled'
    "#,
    );
    assert_eq!(out, "scheduled");

    pump_until_quiescent(&mut ctx, 10_000);

    let verdict = eval_string(
        &mut ctx,
        r#"
        var want = require('crypto').pbkdf2Sync('secret', 'salt', 1, 32, 'sha256').toString('hex');
        var pk = globalThis.__pk;
        [
            'done:' + (pk.done === true),
            'err:' + (pk.err === null ? 'null' : pk.err),
            'hex:' + (pk.hex === want ? 'matches-sync' : 'got<' + pk.hex + '>want<' + want + '>'),
            'hexlen:' + (pk.hex === null ? 0 : pk.hex.length),
        ].join('|')
    "#,
    );
    assert_eq!(
        verdict, "done:true|err:null|hex:matches-sync|hexlen:64",
        "setInterval + promisified pbkdf2 starved or diverged from sync oracle"
    );
    bun_runtime::shutdown_thread_sm();
}

/// setInterval(fn, 50) registered + `await util.promisify(crypto.argon2)`:
/// same spawn_crypto_async completion path, asserted against the in-repo RFC
/// 9106 vector (argon2 conformance "argon2id" defaults: message 32x0x01,
/// nonce 16x0x02, parallelism 1, tagLength 64, memory 8, passes 3).
#[test]
fn timer_crypto_coexistence_interval_argon2_promisified() {
    let mut ctx = setup_ctx();
    let out = eval_string(
        &mut ctx,
        r#"
        var crypto = require('crypto');
        var util = require('util');
        globalThis.__ag = { done: false, hex: null, err: null };
        var params = {
            message: Buffer.alloc(32, 0x01),
            nonce: Buffer.alloc(16, 0x02),
            parallelism: 1, tagLength: 64, memory: 8, passes: 3,
        };
        var iv = setInterval(function() {}, 50);
        util.promisify(crypto.argon2)('argon2id', params).then(
            function(tag) {
                globalThis.__ag.done = true;
                globalThis.__ag.hex = tag.toString('hex');
                clearInterval(iv);
            },
            function(err) {
                globalThis.__ag.done = true;
                globalThis.__ag.err = String((err && err.message) || err);
                clearInterval(iv);
            }
        );
        'scheduled'
    "#,
    );
    assert_eq!(out, "scheduled");

    pump_until_quiescent(&mut ctx, 10_000);

    // RFC 9106 / OpenSSL 3.2 vector — same constant as the argon2
    // conformance suite's vector_10 (["argon2id", {}] over the defaults).
    const EXPECTED_HEX: &str = "509fa5d06cdeb30aa3ae36410116bdbd98da46bbe034d50810ba8518de40867849ffdc2d57c5562abe837602ac0035c612fab842582e00009bd7733f4e6fd49e";
    let verdict = eval_string(
        &mut ctx,
        &format!(
            r#"
        var ag = globalThis.__ag;
        [
            'done:' + (ag.done === true),
            'err:' + (ag.err === null ? 'null' : ag.err),
            'hex:' + (ag.hex === "{expected}" ? 'matches-vector' : 'got<' + ag.hex + '>'),
            'hexlen:' + (ag.hex === null ? 0 : ag.hex.length),
        ].join('|')
    "#,
            expected = EXPECTED_HEX
        ),
    );
    assert_eq!(
        verdict, "done:true|err:null|hex:matches-vector|hexlen:128",
        "setInterval + promisified argon2 starved or diverged from the RFC vector"
    );
    bun_runtime::shutdown_thread_sm();
}
