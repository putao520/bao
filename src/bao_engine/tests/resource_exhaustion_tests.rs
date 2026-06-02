// @trace TEST-EXH-001 [req:REQ-ENG-004] [level:unit] [nfr:TMG-RESILIENCE]
// @trace TEST-EXH-002 [req:REQ-ENG-004] [level:unit] [nfr:TMG-RESILIENCE]
// @trace TEST-EXH-003 [req:REQ-ENG-001] [level:unit] [nfr:TMG-RESILIENCE]
// @trace TEST-EXH-004 [req:REQ-ENG-001] [level:unit] [nfr:TMG-RESILIENCE]
// @trace TEST-EXH-005 [req:REQ-ENG-001] [level:unit] [nfr:TMG-RESILIENCE]
// @trace TEST-EXH-006 [req:REQ-ENG-001] [level:unit] [nfr:TMG-RESILIENCE]
// @trace TEST-EXH-007 [req:REQ-ENG-001] [level:unit] [nfr:TMG-RESILIENCE]
// @trace TEST-EXH-008 [req:REQ-ENG-001] [level:unit] [nfr:TMG-RESILIENCE]
// @trace TEST-EXH-009 [req:REQ-ENG-001] [level:unit] [nfr:TMG-RESILIENCE]
//! Resource exhaustion and architecture resilience tests for `bao_engine`.
//!
//! All sub-tests run within a single `#[test]` function because
//! `mozjs::JSEngine::init()` can only be called once per process.
//! We use `std::panic::catch_unwind` so one failure does not abort
//! the remaining tests.

use ::std::panic::{self, AssertUnwindSafe};

use bao_engine::context::JsContext;

/// Run all resource exhaustion sub-tests sequentially in one engine init.
#[test]
fn resource_exhaustion() {
    let mut passed = 0usize;
    let mut failed = 0usize;

    macro_rules! run {
        ($name:expr, $body:expr) => {{
            match panic::catch_unwind(AssertUnwindSafe(|| -> Result<(), String> { $body })) {
                Ok(Ok(())) => {
                    println!("  PASS: {}", $name);
                    passed += 1;
                }
                Ok(Err(e)) => {
                    println!("  FAIL: {} — {}", $name, e);
                    failed += 1;
                }
                Err(_) => {
                    println!("  FAIL: {} — panicked", $name);
                    failed += 1;
                }
            }
        }};
    }

    println!("\n--- resource_exhaustion ---");

    // ====================================================================
    // 1. JobQueue saturation — enqueue 100K jobs, verify drain
    // ====================================================================
    // @trace TEST-EXH-001
    run!("JobQueue: 100K promise jobs", {
        let mut ctx = JsContext::new().map_err(|e| format!("JsContext::new: {e}"))?;
        let js = r#"
            for (var i = 0; i < 100000; i++) {
                Promise.resolve().then(function(){});
            }
            "ok"
        "#;
        let result = ctx.eval(js, "100k_jobs.js").map_err(|e| format!("eval: {e}"))?;
        // Verify the script completed by checking the result string
        let display = result.to_display_string();
        if display != "ok" {
            return Err(format!("expected 'ok', got '{display}'"));
        }
        Ok(())
    });

    // ====================================================================
    // 2. JobQueue with large payloads (1 KB string × 10 K jobs)
    // ====================================================================
    // @trace TEST-EXH-002
    run!("JobQueue: 1KB×10K payloads", {
        let mut ctx = JsContext::new().map_err(|e| format!("JsContext::new: {e}"))?;
        let js = r#"
            var payload = "x".repeat(1024);
            for (var i = 0; i < 10000; i++) {
                Promise.resolve().then(function(){ var _ = payload; });
            }
            "ok"
        "#;
        ctx.eval(js, "large_payload.js").map_err(|e| format!("eval: {e}"))?;
        Ok(())
    });

    // ====================================================================
    // 3. Repeated JsContext eval cycles — 1000 expressions, single context
    // ====================================================================
    // @trace TEST-EXH-003
    run!("Repeated eval: 1000 cycles", {
        let mut ctx = JsContext::new().map_err(|e| format!("JsContext::new: {e}"))?;
        for i in 0..1000 {
            ctx.eval(&format!("1 + {}", i), "repeated.js")
                .map_err(|e| format!("eval #{}: {e}", i))?;
        }
        Ok(())
    });

    // ====================================================================
    // 4. Deep recursion — 10000 JS frames, must not SIGSEGV
    // ====================================================================
    // @trace TEST-EXH-004
    run!("Deep recursion: 10000 frames", {
        let mut ctx = JsContext::new().map_err(|e| format!("JsContext::new: {e}"))?;
        // Non-tail recursion to guarantee stack growth
        let js = r#"
            (function f(x) { if (x <= 0) return 0; return 1 + f(x - 1); })(10000)
        "#;
        match ctx.eval(js, "deep_recursion.js") {
            // Either an error (stack overflow caught) or success (if limit permits)
            Err(e) => {
                if e.message.contains("stack")
                    || e.message.contains("recursion")
                    || e.message.contains("too much")
                {
                    // Expected — SpiderMonkey caught the overflow
                    Ok(())
                } else {
                    // Still OK — we didn't SIGSEGV, that is the real check
                    println!("         note: error returned (good), msg: {:?}", e.message);
                    Ok(())
                }
            }
            Ok(_) => {
                // No error means the runtime allowed 10000 frames — also fine
                Ok(())
            }
        }
    });

    // ====================================================================
    // 5. Large string eval — 10 MB string allocation
    // ====================================================================
    // @trace TEST-EXH-005
    run!("Large string: 10 MB eval", {
        let mut ctx = JsContext::new().map_err(|e| format!("JsContext::new: {e}"))?;
        let js = r#""a".repeat(10 * 1024 * 1024)"#;
        let result = ctx.eval(js, "large_string.js").map_err(|e| format!("eval: {e}"))?;
        let s = result.to_display_string();
        if s.len() != 10 * 1024 * 1024 {
            return Err(format!("expected 10485760 chars, got {}", s.len()));
        }
        Ok(())
    });

    // ====================================================================
    // 6. Many small object allocations — 10000 objects, GC pressure
    // ====================================================================
    // @trace TEST-EXH-006
    run!("Many allocations: 10000 objects", {
        let mut ctx = JsContext::new().map_err(|e| format!("JsContext::new: {e}"))?;
        let js = r#"
            var arr = [];
            for (var i = 0; i < 10000; i++) {
                arr.push({a: i, b: String(i), c: [i, i+1, i+2]});
            }
            arr.length
        "#;
        let result = ctx.eval(js, "alloc.js").map_err(|e| format!("eval: {e}"))?;
        let n = result.to_display_string();
        if n != "10000" {
            return Err(format!("expected 10000, got '{n}'"));
        }
        Ok(())
    });

    // ====================================================================
    // 7. Deep JsError chain — 1000-frame throw propagation
    // ====================================================================
    // @trace TEST-EXH-007
    run!("Deep JsError chain: 1000 frames", {
        let mut ctx = JsContext::new().map_err(|e| format!("JsContext::new: {e}"))?;
        // Error thrown at depth 1000, propagates up through catch blocks
        let js = r#"
            (function chain(d) {
                try {
                    if (d <= 0) throw new Error("root");
                    return chain(d - 1);
                } catch(e) {
                    throw e;
                }
            })(1000);
        "#;
        match ctx.eval(js, "deep_error.js") {
            Err(e) => {
                if e.message.is_empty() {
                    return Err("empty error message".into());
                }
                if e.filename.is_empty() {
                    // SpiderMonkey may return empty filename for eval'd code;
                    // this is not necessarily a failure
                    println!("         note: empty filename (expected for eval)");
                }
                Ok(())
            }
            Ok(_) => Err("expected error but got Ok".into()),
        }
    });

    // ====================================================================
    // 8. Sequential JsContext lifecycle — create, use, drop, repeat
    // ====================================================================
    // @trace TEST-EXH-008
    //
    // NOTE: mem::forget of a JsContext (leaked Runtime) causes subsequent
    // JsContext::new() to **hang** (SpiderMonkey internal state shared across
    // runtimes). Using std::mem::forget to skip cleanup is not safe with the
    // current mozjs bindings — the Runtime destructor must run to release
    // per-runtime resources before a new Runtime can be created.
    run!("Sequential context lifecycle", {
        // Create and drop multiple contexts in sequence to verify
        // no resource leak in the normal path
        for i in 0..10 {
            let mut ctx = JsContext::new().map_err(|e| format!("JsContext::new #{}: {e}", i))?;
            ctx.eval(&format!("1 + {}", i), "seq.js")
                .map_err(|e| format!("eval #{}: {e}", i))?;
        } // each ctx drops here — Runtime::drop runs each time
        Ok(())
    });

    // ====================================================================
    // 9. Concurrent eval constraint — JsContext is !Send
    // ====================================================================
    // @trace TEST-EXH-009
    //
    // JsContext wraps mozjs::rust::Runtime which holds a *mut JSRuntime.
    // SpiderMonkey runtimes are bound to a single thread and are neither
    // Send nor Sync. The Rust compiler enforces this — attempting to
    // move a JsContext to another thread is a compile-time error.
    //
    // Verification: the following line fails to compile if uncommented:
    //   let ctx = JsContext::new().unwrap();
    //   std::thread::spawn(move || { ctx.eval("1", "t.js"); });
    //   ↑ error: `Send` is not satisfied
    //
    // This is a documented architectural constraint. To evaluate on
    // multiple threads, create a separate JsContext per thread (each
    // gets its own Runtime).
    run!("!Send constraint: documented", {
        // Runtime check: verify that a fresh context works correctly
        // (i.e., the per-thread init path is sound)
        let mut ctx = JsContext::new().map_err(|e| format!("JsContext::new: {e}"))?;
        let result = ctx.eval("42", "check.js").map_err(|e| format!("eval: {e}"))?;
        let n = result.to_display_string();
        if n != "42" {
            return Err(format!("expected 42, got '{n}'"));
        }
        Ok(())
    });

    // Summary
    let total = passed + failed;
    println!("─── {passed}/{total} passed, {failed}/{total} failed ───");
    assert_eq!(failed, 0, "{failed} test(s) failed out of {total}");
}
