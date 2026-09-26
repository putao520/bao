// @trace TEST-ENG-006-FACEGAP [req:REQ-ENG-006] [level:integration]
// Zero-coverage Bun.* / crypto face gap tests: global crypto.randomUUID(),
// Bun.fileURLToPath (incl. roundtrip with Bun.pathToFileURL), Bun.escapeHTML,
// Bun.deepLink (explicit not-implemented contract), the plain Bun.gzip /
// deflate / inflate / gunzip aliases (distinct from the *Sync forms covered
// in bun_wave_a_surface_tests), and the Bun.openInNewTab argument validation.
//
// All checks run in ONE #[test] fn (JSContext per-thread singleton) with a
// bounded drain hook — the same pattern as bun_wave_a_surface_tests.rs.

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;
use std::cell::Cell;

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

fn eval_str(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<face-gap>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => {
            // integral numbers must not print as "1.0"
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
    ctx.eval(source, "<face-gap>").is_ok()
}

#[test]
fn test_bun_face_gap_all() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("Failed to create JSContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);
    ctx.set_post_eval_hook(bounded_drain_hook);

    // ═══ 1. global crypto.randomUUID() — UUID v4 via BaoCrypto CSPRNG ═══
    // (the Web Crypto face on the realm's `crypto` object — distinct from the
    // require('crypto') node module face covered by node_conformance)
    assert_eq!(
        eval_str(&mut ctx, "typeof crypto.randomUUID"),
        "function",
        "the global crypto object must expose randomUUID()"
    );
    assert_eq!(
        eval_str(&mut ctx, "crypto.randomUUID().length"),
        "36",
        "randomUUID() must be the canonical 8-4-4-4-12 hex form (36 chars)"
    );
    assert!(
        eval_str(
            &mut ctx,
            r#"/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(crypto.randomUUID()) ? 'y' : 'n'"#
        ) == "y",
        "randomUUID() must match the UUID v4 shape (version nibble 4, RFC 4122 variant), got: {}",
        eval_str(&mut ctx, "crypto.randomUUID()")
    );
    assert!(
        eval_str(&mut ctx, "crypto.randomUUID() !== crypto.randomUUID() ? 'y' : 'n'") == "y",
        "two randomUUID() calls must differ (CSPRNG, not a fixed value)"
    );
    assert!(
        eval_str(
            &mut ctx,
            "(function(){ var u = crypto.randomUUID(); return u === u.toLowerCase() ? 'y' : 'n'; })()"
        ) == "y",
        "randomUUID() must be lowercase hex"
    );

    // ═══ 2. Bun.fileURLToPath — file:// URL → path (+ percent-decode) ═══
    assert_eq!(
        eval_str(&mut ctx, "typeof Bun.fileURLToPath"),
        "function",
        "Bun.fileURLToPath must be exposed"
    );
    // Roundtrip through Bun.pathToFileURL (its own basics are covered
    // elsewhere — here it is only the input vehicle). Actual implementation
    // contract: pathToFileURL returns the URL as a plain string (not a URL
    // object), so the roundtrip passes the string directly.
    if cfg!(unix) {
        assert_eq!(
            eval_str(&mut ctx, r#"Bun.fileURLToPath(Bun.pathToFileURL('/tmp/x'))"#),
            "/tmp/x",
            "fileURLToPath(pathToFileURL(p)) must roundtrip the path"
        );
        assert_eq!(
            eval_str(&mut ctx, r#"Bun.pathToFileURL('/tmp/x').slice(0, 7)"#),
            "file://",
            "pathToFileURL must produce a file:// URL string (roundtrip vehicle)"
        );
        // Percent-encoded path component decodes back to the raw path.
        assert_eq!(
            eval_str(&mut ctx, r#"Bun.fileURLToPath('file:///tmp/a%20b.txt')"#),
            "/tmp/a b.txt",
            "percent-encoded path components must decode (%20 → space)"
        );
    }
    // Actual implementation behavior: a non-file:// string is returned as-is
    // (bun_api.rs: "Not a file:// URL — return as-is (Bun behavior)") — it
    // does NOT throw.
    assert_eq!(
        eval_str(&mut ctx, "Bun.fileURLToPath('https://example.com/x')"),
        "https://example.com/x",
        "non-file:// input must be returned unchanged (no throw, no fake path)"
    );
    // Missing argument / non-string argument are explicit errors.
    assert!(
        !eval_ok(&mut ctx, "Bun.fileURLToPath()"),
        "fileURLToPath() with no argument must throw"
    );
    assert!(
        !eval_ok(&mut ctx, "Bun.fileURLToPath(42)"),
        "fileURLToPath(non-string) must throw"
    );

    // ═══ 3. Bun.escapeHTML — & < > " ' entity escaping ═══
    assert_eq!(
        eval_str(&mut ctx, r#"Bun.escapeHTML('<a href="x">&\'')"#),
        "&lt;a href=&quot;x&quot;&gt;&amp;&#x27;",
        "escapeHTML must escape & < > \" ' with HTML entities (&#x27; for ')"
    );
    assert_eq!(
        eval_str(&mut ctx, "Bun.escapeHTML('plain-text-123')"),
        "plain-text-123",
        "characters outside the escape set must pass through unchanged"
    );
    assert_eq!(
        eval_str(&mut ctx, "Bun.escapeHTML()"),
        "",
        "escapeHTML() with no argument must return the empty string"
    );
    assert_eq!(
        eval_str(&mut ctx, "typeof Bun.escapeHTML(42)"),
        "undefined",
        "escapeHTML(non-string) must return undefined (no coercion)"
    );

    // ═══ 4. Bun.deepLink — explicit not-implemented contract ═══
    // The implementation always reports an error; it must never resolve.
    assert!(
        !eval_ok(&mut ctx, "Bun.deepLink('bao://face-gap')"),
        "deepLink(url) must throw (not implemented in this environment)"
    );
    assert!(
        !eval_ok(&mut ctx, "Bun.deepLink()"),
        "deepLink() with no argument must throw as well"
    );

    // ═══ 5. Bun.gzip/deflate/inflate/gunzip — plain sync aliases ═══
    // (the *Sync forms are covered in bun_wave_a_surface_tests; these are the
    // distinct non-Sync names from the API table)
    assert_eq!(
        eval_str(
            &mut ctx,
            r#"
            (function() {
                var enc = new TextEncoder(); var dec = new TextDecoder();
                var g = Bun.gzip('face-gap-gzip');
                if (!(g instanceof Uint8Array)) return 'bad-type';
                var back = dec.decode(Bun.gunzip(g));
                var d = Bun.deflate(enc.encode('face-gap-deflate'));
                var back2 = dec.decode(Bun.inflate(d));
                return back + '|' + back2;
            })()
        "#
        ),
        "face-gap-gzip|face-gap-deflate",
        "plain gzip/gunzip and deflate/inflate aliases must round-trip (Uint8Array in/out)"
    );
    assert_eq!(
        eval_str(
            &mut ctx,
            "new TextDecoder().decode(Bun.inflate(Bun.deflate(''))).length"
        ),
        "0",
        "empty input must round-trip to empty output"
    );
    assert!(
        !eval_ok(&mut ctx, "Bun.gzip()"),
        "gzip() with no argument must throw"
    );
    assert!(
        !eval_ok(&mut ctx, "Bun.gunzip()"),
        "gunzip() with no argument must throw"
    );
    assert!(
        !eval_ok(&mut ctx, "Bun.inflate()"),
        "inflate() with no argument must throw"
    );
    assert!(
        !eval_ok(&mut ctx, "Bun.deflate()"),
        "deflate() with no argument must throw"
    );

    // ═══ 6. Bun.openInNewTab — argument validation contract ═══
    // (the success path spawns a platform opener process, so only the
    // validation faces are asserted here — no process side effects)
    assert!(
        !eval_ok(&mut ctx, "Bun.openInNewTab()"),
        "openInNewTab() with no argument must throw"
    );
    assert!(
        !eval_ok(&mut ctx, "Bun.openInNewTab(42)"),
        "openInNewTab(non-string) must throw"
    );
}
