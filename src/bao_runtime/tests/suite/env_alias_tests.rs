// @trace REQ-CLI-001 [test:TEST-CLI-001] [level:integration]
// BAO_* → BUN_* env alias contract after the B0 census row-16 transposition
// (issue #32): the alias is resolved at the env read layer
// (`bun_core::getenv_z` / `getenv_z_any_case` — a `BUN_<SUFFIX>` lookup that
// misses falls back to `BAO_<SUFFIX>`, explicit `BUN_` wins), and
// `BaoRuntime::new()` must NOT mutate the host process environment (the
// retired `init_env_aliases` `std::env::set_var` path is gone).
//
// Coverage:
//   - positive: a real env_var consumer (BUN_CONFIG_HTTP_IDLE_TIMEOUT, read by
//     src/http/HTTPThread.rs socket idle-timeout init) resolves the
//     BAO_<suffix> spelling through BaoRuntime::new().
//   - precedence: explicit BUN_<suffix> wins over BAO_<suffix> (the retired
//     `init_env_aliases` `is_err()` guard semantics).
//   - direct `getenv_z` / `getenv_z_any_case` consumer classes
//     (threading/ast/md/sys call sites; output.rs `BUN_DEBUG_<tag>`).
//   - negative: host process env has no injected BUN_* after new()+eval+drop.
//   - lifecycle: two sequential runtimes — alias reads work per instance,
//     no cross-pollution, no residue.
//
// Env keys are process-global: every test cleans up the keys it touches
// (setup and teardown), matching the cli_dispatch.rs alias-test precedent.

/// eval a trivial script so the runtime completes its full init path
/// (globals install, job queue, post-eval hook) exactly like real usage.
fn eval_ok(rt: &mut bun_runtime::BaoRuntime) {
    rt.eval("0", "<env-alias-test>").expect("eval must succeed");
}

/// Positive: BAO_CONFIG_HTTP_IDLE_TIMEOUT=777 must be resolved by the real
/// consumer accessor (`bun_core::env_var::BUN_CONFIG_HTTP_IDLE_TIMEOUT`, the
/// same accessor src/http/HTTPThread.rs reads) after BaoRuntime::new(),
/// while the host process env gains no BUN_ variable.
#[test]
fn env_alias_positive_real_env_var_consumer_resolves_bao_suffix() {
    unsafe {
        std::env::remove_var("BUN_CONFIG_HTTP_IDLE_TIMEOUT");
        std::env::remove_var("BAO_CONFIG_HTTP_IDLE_TIMEOUT");
        std::env::set_var("BAO_CONFIG_HTTP_IDLE_TIMEOUT", "777");
    }

    let mut rt = bun_runtime::BaoRuntime::new().expect("BaoRuntime");
    eval_ok(&mut rt);

    assert_eq!(
        bun_core::env_var::BUN_CONFIG_HTTP_IDLE_TIMEOUT.get(),
        Some(777),
        "BAO_CONFIG_HTTP_IDLE_TIMEOUT must be resolved by the BUN_CONFIG_HTTP_IDLE_TIMEOUT \
         consumer via the read-layer alias"
    );
    assert!(
        std::env::var("BUN_CONFIG_HTTP_IDLE_TIMEOUT").is_err(),
        "BaoRuntime::new() must not inject BUN_CONFIG_HTTP_IDLE_TIMEOUT into the host env"
    );

    drop(rt);
    unsafe {
        std::env::remove_var("BAO_CONFIG_HTTP_IDLE_TIMEOUT");
    }
}

/// Precedence: an explicit BUN_<suffix> must win over BAO_<suffix> — the
/// semantics of the retired `init_env_aliases` `if env::var(&bun_key).is_err()`
/// guard (BUN_* 上游生态显式优先, BAO_* 仅 fallback).
#[test]
fn env_alias_explicit_bun_wins_over_bao_alias() {
    unsafe {
        std::env::remove_var("BUN_CONFIG_DNS_TIME_TO_LIVE_SECONDS");
        std::env::remove_var("BAO_CONFIG_DNS_TIME_TO_LIVE_SECONDS");
        std::env::set_var("BUN_CONFIG_DNS_TIME_TO_LIVE_SECONDS", "55");
        std::env::set_var("BAO_CONFIG_DNS_TIME_TO_LIVE_SECONDS", "66");
    }

    let mut rt = bun_runtime::BaoRuntime::new().expect("BaoRuntime");
    eval_ok(&mut rt);

    assert_eq!(
        bun_core::env_var::BUN_CONFIG_DNS_TIME_TO_LIVE_SECONDS.get(),
        Some(55),
        "explicit BUN_CONFIG_DNS_TIME_TO_LIVE_SECONDS must win over the BAO_ alias"
    );

    drop(rt);
    unsafe {
        std::env::remove_var("BUN_CONFIG_DNS_TIME_TO_LIVE_SECONDS");
        std::env::remove_var("BAO_CONFIG_DNS_TIME_TO_LIVE_SECONDS");
    }
}

/// Direct-primitive consumer classes: call sites that read
/// `bun_core::getenv_z(zstr!("BUN_..."))` directly (src/threading/ThreadPool.rs,
/// src/ast/lib.rs, src/md/ansi_renderer.rs, src/sys/lib.rs) and any-case
/// readers (src/bun_core/output.rs `BUN_DEBUG_<tag>`) must keep resolving the
/// BAO_ spelling — no host-env materialization involved.
#[test]
fn env_alias_direct_getenv_z_primitives_resolve_bao_suffix() {
    unsafe {
        std::env::remove_var("BUN_ENVALIAS_DIRECTZ");
        std::env::remove_var("BAO_ENVALIAS_DIRECTZ");
        std::env::remove_var("BUN_ENVALIAS_ANYCZ");
        std::env::remove_var("BAO_ENVALIAS_ANYCZ");
        std::env::set_var("BAO_ENVALIAS_DIRECTZ", "dz");
        std::env::set_var("BAO_ENVALIAS_ANYCZ", "ac");
    }

    // Case-sensitive primitive.
    assert_eq!(
        bun_core::getenv_z(bun_core::zstr!("BUN_ENVALIAS_DIRECTZ")),
        Some(b"dz".as_slice()),
        "getenv_z must fall back from BUN_<suffix> to BAO_<suffix> on miss"
    );
    // Case-insensitive primitive.
    assert_eq!(
        bun_core::getenv_z_any_case(bun_core::zstr!("BUN_ENVALIAS_ANYCZ")),
        Some(b"ac".as_slice()),
        "getenv_z_any_case must fall back from BUN_<suffix> to BAO_<suffix> on miss"
    );
    // Host env stays unmaterialized.
    assert!(std::env::var("BUN_ENVALIAS_DIRECTZ").is_err());
    assert!(std::env::var("BUN_ENVALIAS_ANYCZ").is_err());

    unsafe {
        std::env::remove_var("BAO_ENVALIAS_DIRECTZ");
        std::env::remove_var("BAO_ENVALIAS_ANYCZ");
        std::env::remove_var("BUN_ENVALIAS_DIRECTZ");
        std::env::remove_var("BUN_ENVALIAS_ANYCZ");
    }
}

/// Negative (issue #32 core): constructing BaoRuntime, evaluating, and
/// dropping it must leave the host process env free of any injected
/// BUN_<SUFFIX> derived from BAO_<SUFFIX>. Unique key so parallel/host env
/// noise cannot mask the assertion.
#[test]
fn env_alias_negative_host_env_not_mutated_by_runtime_constructor() {
    let unique = format!(
        "BAO_TEST_ALS_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let bun_key = unique.replacen("BAO_", "BUN_", 1);

    unsafe {
        std::env::remove_var(&unique);
        std::env::remove_var(&bun_key);
        std::env::set_var(&unique, "alias_negative_proof");
    }

    let mut rt = bun_runtime::BaoRuntime::new().expect("BaoRuntime");
    eval_ok(&mut rt);
    drop(rt);

    assert!(
        std::env::var(&bun_key).is_err(),
        "host process env must not contain '{}' after BaoRuntime::new()+eval+drop \
         (no std::env::set_var from the library constructor)",
        bun_key
    );

    unsafe {
        std::env::remove_var(&unique);
        std::env::remove_var(&bun_key);
    }
}

/// Lifecycle (multi-runtime): two sequential BaoRuntime construct→use→drop
/// cycles — each instance resolves its own BAO_* alias at read time, the
/// first instance's reads leave no materialized BUN_* for the second, and
/// neither leaves host-env residue.
#[test]
fn env_alias_sequential_runtimes_no_cross_pollution_no_residue() {
    unsafe {
        std::env::remove_var("BUN_ENVALIAS_LCY_A");
        std::env::remove_var("BUN_ENVALIAS_LCY_B");
        std::env::set_var("BAO_ENVALIAS_LCY_A", "rt1");
    }

    {
        let mut rt1 = bun_runtime::BaoRuntime::new().expect("rt1");
        eval_ok(&mut rt1);
        assert_eq!(
            bun_core::getenv_z(bun_core::zstr!("BUN_ENVALIAS_LCY_A")),
            Some(b"rt1".as_slice()),
            "rt1 must resolve BAO_ENVALIAS_LCY_A through the read-layer alias"
        );
        assert!(
            std::env::var("BUN_ENVALIAS_LCY_A").is_err(),
            "rt1 must not materialize BUN_ENVALIAS_LCY_A in the host env"
        );
    }

    unsafe {
        std::env::set_var("BAO_ENVALIAS_LCY_B", "rt2");
    }
    {
        let mut rt2 = bun_runtime::BaoRuntime::new().expect("rt2");
        eval_ok(&mut rt2);
        assert_eq!(
            bun_core::getenv_z(bun_core::zstr!("BUN_ENVALIAS_LCY_B")),
            Some(b"rt2".as_slice()),
            "rt2 must resolve BAO_ENVALIAS_LCY_B through the read-layer alias"
        );
        assert!(
            std::env::var("BUN_ENVALIAS_LCY_A").is_err(),
            "rt1's alias read left no BUN_ENVALIAS_LCY_A residue for rt2"
        );
        assert!(
            std::env::var("BUN_ENVALIAS_LCY_B").is_err(),
            "rt2 must not materialize BUN_ENVALIAS_LCY_B in the host env"
        );
    }

    unsafe {
        std::env::remove_var("BAO_ENVALIAS_LCY_A");
        std::env::remove_var("BAO_ENVALIAS_LCY_B");
        std::env::remove_var("BUN_ENVALIAS_LCY_A");
        std::env::remove_var("BUN_ENVALIAS_LCY_B");
    }
}

/// JS enumeration surface: the `process.env` snapshot (bun_api.rs
/// `populate_process_object`) must expose the `BUN_<SUFFIX>` spelling for a
/// `BAO_<SUFFIX>`-only key — the read-layer alias covers keyed lookups
/// (`getenv_z`), but JS `process.env.BUN_X` property access and
/// `Object.keys(process.env)` read the snapshot object, so without the alias
/// pass the property would be `undefined` under `BAO_X=...`. Explicit
/// `BUN_<SUFFIX>` wins on the JS surface (same precedence as `getenv_z`);
/// the host process env gains no `BUN_*` variable.
#[test]
fn env_alias_js_process_env_snapshot_exposes_bun_spelling() {
    unsafe {
        std::env::remove_var("BUN_ENVALIAS_JS_A");
        std::env::remove_var("BAO_ENVALIAS_JS_A");
        std::env::remove_var("BUN_ENVALIAS_JS_B");
        std::env::remove_var("BAO_ENVALIAS_JS_B");
        std::env::set_var("BAO_ENVALIAS_JS_A", "js_alias");
        std::env::set_var("BUN_ENVALIAS_JS_B", "js_explicit");
        std::env::set_var("BAO_ENVALIAS_JS_B", "js_shadow");
    }

    let mut rt = bun_runtime::BaoRuntime::new().expect("BaoRuntime");
    eval_ok(&mut rt);

    // Positive: property access through the Proxy get trap reads the BAO_
    // value under the BUN_ spelling.
    match rt.eval("process.env.BUN_ENVALIAS_JS_A", "<env-alias-js>") {
        Ok(bao_engine::value::JsValue::String(s)) => assert_eq!(
            s, "js_alias",
            "process.env.BUN_ENVALIAS_JS_A must expose the BAO_ value on the JS surface"
        ),
        other => panic!(
            "process.env.BUN_ENVALIAS_JS_A must be the BAO_ value string, got {:?}",
            other
        ),
    }
    // Enumeration: the BUN_ spelling is an enumerable key of the snapshot.
    let enumerable = rt
        .eval(
            "Object.keys(process.env).includes('BUN_ENVALIAS_JS_A')",
            "<env-alias-js>",
        )
        .ok()
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    assert!(
        enumerable,
        "BUN_ENVALIAS_JS_A must be enumerable on the process.env snapshot"
    );
    // Precedence on the JS surface: explicit BUN_ wins over the BAO_ alias.
    match rt.eval("process.env.BUN_ENVALIAS_JS_B", "<env-alias-js>") {
        Ok(bao_engine::value::JsValue::String(s)) => assert_eq!(
            s, "js_explicit",
            "explicit BUN_ENVALIAS_JS_B must win over BAO_ENVALIAS_JS_B on the JS surface"
        ),
        other => panic!(
            "process.env.BUN_ENVALIAS_JS_B must be the explicit BUN_ value string, got {:?}",
            other
        ),
    }
    // Host env: no BUN_* materialized from the BAO_ alias.
    assert!(
        std::env::var("BUN_ENVALIAS_JS_A").is_err(),
        "the JS enumeration surface must not materialize BUN_ENVALIAS_JS_A into the host env"
    );

    drop(rt);
    unsafe {
        std::env::remove_var("BUN_ENVALIAS_JS_A");
        std::env::remove_var("BAO_ENVALIAS_JS_A");
        std::env::remove_var("BUN_ENVALIAS_JS_B");
        std::env::remove_var("BAO_ENVALIAS_JS_B");
    }
}
