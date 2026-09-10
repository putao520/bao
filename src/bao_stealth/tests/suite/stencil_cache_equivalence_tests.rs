// SM-EVOLUTION #26 — live equivalence of the stencil-cached stealth blob
// injection against the uncached path.
//
// The production wiring (engine_props::inject_js_hooks) evaluates the
// per-profile combined JS blob in EVERY page/worker/SW realm through
// bao_engine::stencil_cache::evaluate_script_cached (compile-once /
// instantiate-per-realm). This file pins the #26 completion criterion ②:
// the observable post-injection state of a realm must be byte-for-byte
// identical whether the blob was evaluated by the plain JS::Evaluate path
// (the pre-#26 production path) or by the cached path — on BOTH the miss
// (first realm: compile + store) and the hit (second realm: pure
// instantiate + execute) arms.
//
// The fingerprint is taken in the same realm right after the blob ran, via
// the tiny uncached `ctx.eval` probe (below the cache admission floor, so
// the probe itself never touches the cache). In a bare test realm the
// blob's unconditional observable is the patched `Date.now` (the timing
// hook); DOM-dependent segments are typeof-guarded off in bare realms by
// design — the page/worker-realm hook-state assertions live in the
// bao_browser e2e suites that now run through the cached path.

use bao_engine::context::JsContext;
use bao_engine::stencil_cache;
use bao_engine::value::JsValue;
use bao_stealth::{StealthHooks, StealthProfile};

fn stealth_payload() -> String {
    let profile = StealthProfile::firefox_default();
    let hooks = StealthHooks::from_profile(
        &profile.canvas,
        &profile.audio,
        &profile.navigator,
        &profile.screen,
        &profile.webgl,
        &profile.font,
        &profile.battery,
        profile.webrtc_mode,
        &profile.timing,
        &profile.clientrects,
        &profile.screen_display,
        &profile.plugin,
        &profile.speech,
        &profile.media_devices,
        &profile.permissions,
        &profile.webgl_context,
        &profile.connection,
        &profile.iframe,
    );
    hooks.combined_js()
}

/// Structural fingerprint of a realm's post-blob state. Everything the bare
/// realm can observe about what the blob installed.
const FINGERPRINT: &str = r#"JSON.stringify({
  dateNowType: typeof Date.now,
  dateNowSrc: Date.now.toString(),
  dateNowName: Date.now.name,
  dateNowArity: Date.now.length,
  funcToStringCallable: typeof Date.now.toString === 'function',
  probeGlobals: Object.getOwnPropertyNames(globalThis).filter(function(n) {
    return n.indexOf('__probe') === 0;
  }).sort()
})"#;

/// Evaluate `src` in a fresh realm of the shared TLS test runtime via the
/// plain (uncached) production path and return the completion value.
fn eval_plain_fresh_realm(src: &str, filename: &str) -> Result<JsValue, String> {
    let mut ctx = JsContext::for_test().map_err(|e| e.message)?;
    ctx.eval(src, filename).map_err(|e| e.message)
}

/// Evaluate `src` in a fresh realm via the stencil-cached path.
fn eval_cached_fresh_realm(src: &str, filename: &str) -> Result<JsValue, String> {
    use bao_engine::stencil_cache::evaluate_script_cached;
    use mozjs::rooted;
    let mut ctx = JsContext::for_test().map_err(|e| e.message)?;
    let mut cx = ctx.cx();
    let global_ptr = ctx
        .ensure_realm_global(&mut cx, None)
        .map_err(|e| e.message)?;
    rooted!(&in(cx) let global = global_ptr);
    let c_filename = std::ffi::CString::new(filename).unwrap();
    rooted!(&in(cx) let mut rval = mozjs::jsval::UndefinedValue());
    evaluate_script_cached(&mut cx, global.handle(), src, &c_filename, 1, rval.handle_mut())
        .map_err(|_| "cached eval failed".to_string())?;
    unsafe {
        Ok(bao_engine::value::jsval_to_jsvalue(
            cx.raw_cx_no_gc(),
            rval.get(),
        ))
    }
}

/// Blob + fingerprint in one source: evaluate the blob, then probe.
fn blob_then_fingerprint(blob: &str) -> String {
    format!("{blob}\n;{FINGERPRINT}")
}

// @trace REQ-STL-007 [criterion:REQ-STL-007-C1] [req:REQ-STL-007] [level:integration]
// Cached injection equivalence: plain vs cached-miss vs cached-hit realms
// produce byte-identical post-blob state.
#[test]
fn stealth_blob_cached_injection_is_byte_equal_to_plain() -> Result<(), String> {
    stencil_cache::clear_thread_cache();
    let blob = stealth_payload();
    assert!(
        blob.len() >= 1024,
        "production blob must be above the cache admission floor (got {} bytes)",
        blob.len()
    );

    let src = blob_then_fingerprint(&blob);
    let filename = "<bao-stealth-hooks>";

    let plain = match eval_plain_fresh_realm(&src, filename)? {
        JsValue::String(s) => s,
        other => panic!("plain path fingerprint: expected JSON string, got {other:?}"),
    };
    let miss = match eval_cached_fresh_realm(&src, filename)? {
        JsValue::String(s) => s,
        other => panic!("cached-miss fingerprint: expected JSON string, got {other:?}"),
    };
    let hit = match eval_cached_fresh_realm(&src, filename)? {
        JsValue::String(s) => s,
        other => panic!("cached-hit fingerprint: expected JSON string, got {other:?}"),
    };

    assert_eq!(plain, miss, "cached MISS arm diverged from plain path");
    assert_eq!(plain, hit, "cached HIT arm diverged from plain path");

    // The fingerprint must actually prove the blob ran (patched Date.now),
    // not that all three realms are empty.
    assert!(
        plain.contains("roundToPrecision"),
        "fingerprint does not observe the patched Date.now — the blob did not run: {plain}"
    );

    // Cache engagement: exactly one entry (the blob), the third eval hit it.
    assert_eq!(stencil_cache::thread_cache_len(), 1);
    let (hits, misses, _) = stencil_cache::thread_cache_counters();
    assert!(hits >= 1, "second cached eval must hit the cache");
    assert!(misses >= 1, "first cached eval must be a miss");
    Ok(())
}

// @trace REQ-STL-007 [criterion:REQ-STL-007-C1] [req:REQ-STL-007] [level:integration]
// Per-profile isolation: two different profile blobs are distinct cache
// keys — a realm injected with profile B after profile A must observe B's
// fingerprint, never A's (guards source-key correctness end to end through
// the production StealthHooks builder).
#[test]
fn stealth_blob_cache_separates_profiles() -> Result<(), String> {
    stencil_cache::clear_thread_cache();

    let build = |ua_patch: &str| -> String {
        let profile = StealthProfile::firefox_default();
        let hooks = StealthHooks::from_profile(
            &profile.canvas,
            &profile.audio,
            &profile.navigator,
            &profile.screen,
            &profile.webgl,
            &profile.font,
            &profile.battery,
            profile.webrtc_mode,
            &profile.timing,
            &profile.clientrects,
            &profile.screen_display,
            &profile.plugin,
            &profile.speech,
            &profile.media_devices,
            &profile.permissions,
            &profile.webgl_context,
            &profile.connection,
            &profile.iframe,
        );
        let blob = hooks.combined_js();
        // Distinguish blobs the way distinct profiles do: different source
        // bytes (a different profile produces a different builder output;
        // simulate the minimal case with a distinguishing marker statement
        // that the fingerprint can observe).
        format!("globalThis.__probe_profile = '{ua_patch}';\n{blob}")
    };

    let fingerprint = |src: &str| -> String {
        format!("{src}\n;JSON.stringify({{ profile: globalThis.__probe_profile, dateNowSrc: Date.now.toString().slice(0, 64) }})")
    };

    let a = fingerprint(&build("A"));
    let b = fingerprint(&build("B"));
    // Interleave: A(miss) B(miss) A(hit) B(hit) — every realm must see its
    // OWN profile marker; a cross-contaminated cache would surface the wrong
    // marker.
    for (src, expect) in [(&a, "A"), (&b, "B"), (&a, "A"), (&b, "B"), (&a, "A")] {
        let v = match eval_cached_fresh_realm(src, "<bao-stealth-hooks>")? {
            JsValue::String(s) => s,
            other => panic!("expected JSON string, got {other:?}"),
        };
        assert!(
            v.contains(&format!(r#""profile":"{expect}""#)),
            "expected profile {expect}, got {v}"
        );
    }
    assert_eq!(stencil_cache::thread_cache_len(), 2, "A and B are two entries");
    Ok(())
}
