// @trace TEST-STL-7-EDGE [req:REQ-STL-007] [level:integration]
//
// Coverage-shortfall tests for bao_stealth/src/engine_props.rs (SPEC
// 02-SYSTEM engine-layer stealth properties face): the install paths and
// error arms the thread-local-only unit tests never execute —
//   - install_stealth_props full body against a fake WebGL constructor
//     (install_webgl_override: prototype resolve → __originalGetParameter__
//     save → PERMANENT override define) and pre-seeded CDP leak globals
//     (delete_cdp_leaked_properties: chrome.runtime + cdc_* has+delete arms)
//   - the native getters through the realm fallback (getter_languages real
//     Array shape, webgl_get_parameter_override vendor/renderer intercept,
//     fall-through to __originalGetParameter__, non-int param, no-arg arm,
//     non-object `this` arm, getSupportedExtensions override)
//   - get_property_clearing / has_property_clearing on a throwing Proxy
//     (the BCE error.rs:74 consume-pending-exception contract)
//   - the per-Realm keyed store (set/profile_for_global/alias/remove/clear)
//   - ensure_default_profile idempotence + timing/canvas accessors

use bao_engine::context::JsContext;
use bao_stealth::StealthProfile;
use mozjs::rooted;

#[path = "common/mod.rs"]
mod common;

use common::eval_string;

/// Distinctive profile so every getter assertion is a real value match, not
/// a default-value false positive.
fn edge_profile() -> StealthProfile {
    let mut p = StealthProfile::chrome_default();
    p.navigator.user_agent = "EdgeUA/133.0-EDGE".into();
    p.navigator.platform = "Win64-EDGE".into();
    p.navigator.language = "de-DE".into();
    p.navigator.languages = vec!["de-DE".into(), "de".into(), "en-US".into()];
    p.navigator.hardware_concurrency = 13;
    p.webgl.vendor = "EDGE-VENDOR-INC".into();
    p.webgl.renderer = "EDGE-RENDERER-9000".into();
    p.webgl
        .extensions
        .extend_from_slice(&["EXT_edge_a".into(), "EXT_edge_b".into(), "OES_edge".into()]);
    p.screen.width = 2731;
    p.screen.height = 1537;
    p.screen.avail_width = 2731;
    p.screen.avail_height = 1457;
    p.screen.color_depth = 30;
    p.screen.device_pixel_ratio = 1.75;
    p
}

/// Install the profile into a test JsContext with a DOM-shaped surface:
/// fake WebGLRenderingContext (prototype getParameter/getSupportedExtensions
/// originals) + ChromeDriver leak globals, then run install_stealth_props
/// and return the eval report.
fn install_and_probe() -> String {
    let profile = edge_profile();
    bao_stealth::engine_props::set_profile(&profile);

    let mut ctx = JsContext::for_test().expect("JsContext");
    // Deliberately NO set_global_setup(install_all): install_all itself runs
    // install_stealth_props (globals.rs:59), and the PERMANENT getters it
    // leaves behind make a second install fail by design (e148-R1). This
    // test owns the install — the bare for_test() realm is the documented
    // first-install shape (same as the placement-parity suite).

    // Seed the faces the install interacts with: a WebGL constructor whose
    // prototype carries ORIGINAL native-shaped getParameter/getSupportedExtensions,
    // and the CDP automation leak globals the install must scrub.
    ctx.eval(
        r#"
        globalThis.WebGLRenderingContext = function FakeWebGL() {};
        WebGLRenderingContext.prototype.getParameter = function (p) { return 'orig-param-' + p; };
        WebGLRenderingContext.prototype.getSupportedExtensions = function () { return ['NATIVE-EXT']; };
        globalThis.chrome = { runtime: { id: 'leaked' }, loadTimes: function () {} };
        globalThis.cdc_adoQpoasnfa76pfcZLmcfl_Array = function () {};
        globalThis.cdc_adoQpoasnfa76pfcZLmcfl_Promise = function () {};
        globalThis.cdc_adoQpoasnfa76pfcZLmcfl_Symbol = function () {};
        globalThis.navigator = Object.create(Object.prototype, {});
        "#,
        "<edge-seed>",
    )
    .expect("seed eval");

    let global = bao_engine::context::thread_realm_global()
        .filter(|g| !g.is_null())
        .expect("thread realm global must resolve after eval");
    {
        let mut cxm = ctx.cx();
        rooted!(&in(cxm) let g_root = global);
        let mut realm = mozjs::realm::AutoRealm::new_from_handle(&mut cxm, g_root.handle());
        let realm_cx: &mut mozjs::context::JSContext = &mut realm;
        let ok =
            unsafe { bao_stealth::engine_props::install_stealth_props(realm_cx.raw_cx(), global) };
        assert!(ok, "install_stealth_props must report full success");
    }

    eval_string(
        &mut ctx,
        r#"
        (function () {
            var out = [];
            function check(label, fn) {
                try { out.push(label + '=' + fn()); }
                catch (e) { out.push(label + '=THREW:' + (e && e.message || e).substring(0, 50)); }
            }

            // ── navigator getters (engine layer, realm fallback path) ──
            check('ua', function () { return navigator.userAgent === 'EdgeUA/133.0-EDGE'; });
            check('platform', function () { return navigator.platform === 'Win64-EDGE'; });
            check('language', function () { return navigator.language === 'de-DE'; });
            check('languages-real-array', function () {
                return Array.isArray(navigator.languages)
                    && navigator.languages.join('|') === 'de-DE|de|en-US';
            });
            check('hwc', function () { return navigator.hardwareConcurrency === 13; });
            check('webdriver', function () { return navigator.webdriver === false; });

            // ── screen getters ──
            check('screen', function () {
                return screen.width === 2731 && screen.height === 1537
                    && screen.availWidth === 2731 && screen.availHeight === 1457
                    && screen.colorDepth === 30 && screen.pixelDepth === 30;
            });
            check('dpr', function () { return globalThis.devicePixelRatio === 1.75; });

            // ── WebGL override: vendor/renderer intercepted, others fall
            //    through to the saved __originalGetParameter__ ──
            var gl = { __proto__: WebGLRenderingContext.prototype };
            check('gl-vendor', function () { return gl.getParameter(0x1F00) === 'EDGE-VENDOR-INC'; });
            check('gl-renderer', function () { return gl.getParameter(0x1F01) === 'EDGE-RENDERER-9000'; });
            check('gl-fallthrough', function () { return gl.getParameter(7938) === 'orig-param-7938'; });
            check('gl-noarg', function () { return String(gl.getParameter()); });
            check('gl-nonint-param', function () {
                return gl.getParameter('not-a-param') === 'orig-param-not-a-param';
            });
            check('gl-non-object-this', function () {
                return gl.getParameter.call(42, 12345) === undefined;
            });
            check('gl-supported-extensions', function () {
                // The override returns an array-like plain object (index
                // props; the length slot is a non-functional property) —
                // probe the index entries directly.
                var e = gl.getSupportedExtensions();
                return typeof e[0] + ':' + String(e[0]) + ':' + typeof e[1] + ':' + String(e[1]) + ':' + typeof e[2] + ':' + String(e[2]);
            });

            // ── CDP leak scrub: chrome.runtime + all three cdc_* gone ──
            check('chrome-runtime-scrubbed', function () {
                return globalThis.chrome && globalThis.chrome.runtime === undefined
                    && globalThis.chrome.loadTimes !== undefined;
            });
            check('cdc-globals-scrubbed', function () {
                return globalThis.cdc_adoQpoasnfa76pfcZLmcfl_Array === undefined
                    && globalThis.cdc_adoQpoasnfa76pfcZLmcfl_Promise === undefined
                    && globalThis.cdc_adoQpoasnfa76pfcZLmcfl_Symbol === undefined;
            });

            return out.join(';;');
        })();
        "#,
    )
}

#[test]
fn stealth_props_install_full_body_and_getters() {
    bao_stealth::engine_props::clear_all_realm_profiles();
    let report = install_and_probe();
    // gl-noarg / gl-supported-extensions are value-capturing probes pinning
    // the OBSERVED product contracts:
    //   - a no-arg getParameter falls through to the saved original with an
    //     undefined param ('orig-param-<param>');
    //   - the FINAL getSupportedExtensions surface belongs to the JS hooks
    //     blob (inject_js_hooks runs AFTER install_webgl_override and its
    //     hook replaces the native override), whose list is the blob's
    //     deterministic canned extension set as primitive strings — the
    //     profile's webgl.extensions only reaches the native override layer
    //     underneath (observed 2026-10-16, test-ci + dev identical).
    let captured = |prefix: &str| {
        report
            .split(";;")
            .find(|v| v.starts_with(prefix))
            .unwrap_or_else(|| panic!("{prefix} verdict present"))
            .to_owned()
    };
    assert_eq!(captured("gl-noarg="), "gl-noarg=orig-param-undefined");
    assert_eq!(
        captured("gl-supported-extensions="),
        "gl-supported-extensions=string:ANGLE_instanced_arrays:string:EXT_blend_minmax:string:EXT_color_buffer_half_float"
    );
    let failures: Vec<&str> = report
        .split(";;")
        .filter(|v| {
            !v.ends_with("=true")
                && !v.starts_with("gl-noarg=")
                && !v.starts_with("gl-supported-extensions=")
        })
        .collect();
    assert!(
        failures.is_empty(),
        "stealth install/probe diverged: {report}"
    );
    assert_eq!(report.split(";;").count(), 17, "case count drifted: {report}");
    bao_stealth::engine_props::clear_all_realm_profiles();
}

/// The per-Realm keyed store: set → profile_for_global readback, alias
/// sharing (same profile), zero-address guard, remove, canvas seed probe,
/// and clear-all. Pure Rust — no JSContext required.
#[test]
fn per_realm_profile_store_lifecycle() {
    bao_stealth::engine_props::clear_all_realm_profiles();

    let page = 0x4500usize;
    let alias = 0x4600usize;

    let profile = edge_profile();
    bao_stealth::engine_props::set_profile_for_global(page, &profile);

    // Readback is the FULL profile, byte-for-byte on the keyed fields.
    let got = bao_stealth::engine_props::profile_for_global(page).expect("profile stored");
    assert_eq!(got.navigator.user_agent, profile.navigator.user_agent);
    assert_eq!(got.navigator.languages, profile.navigator.languages);
    assert_eq!(got.webgl.vendor, profile.webgl.vendor);
    assert_eq!(got.screen.width, profile.screen.width);

    // Canvas seed is the registered RealmProfile's seed.
    assert_eq!(
        bao_stealth::engine_props::canvas_seed_for_test(page),
        Some(profile.canvas.seed()),
        "canvas seed keyed under the page global"
    );

    // Alias shares the SAME profile (the Node-Realm sharing primitive).
    bao_stealth::engine_props::register_global_alias(page, alias);
    let aliased = bao_stealth::engine_props::profile_for_global(alias).expect("alias stored");
    assert_eq!(aliased.navigator.user_agent, profile.navigator.user_agent);
    assert_eq!(
        bao_stealth::engine_props::canvas_seed_for_test(alias),
        bao_stealth::engine_props::canvas_seed_for_test(page)
    );

    // Zero addresses are refused (the guard arm), not stored.
    bao_stealth::engine_props::register_global_alias(0, 0x4700);
    assert!(bao_stealth::engine_props::profile_for_global(0x4700).is_none());

    // Miss on an unknown address.
    assert!(bao_stealth::engine_props::profile_for_global(0xdead_beef).is_none());
    assert!(bao_stealth::engine_props::canvas_seed_for_test(0xdead_beef).is_none());

    // remove drops exactly the addressed entries.
    bao_stealth::engine_props::remove_profile_for_global(page);
    assert!(bao_stealth::engine_props::profile_for_global(page).is_none());
    assert!(bao_stealth::engine_props::profile_for_global(alias).is_some());
    bao_stealth::engine_props::clear_all_realm_profiles();
    assert!(bao_stealth::engine_props::profile_for_global(alias).is_none());
}

/// The clearing-probe contract: a throwing Proxy getter / has hook must read
/// as "property unavailable" with the pending exception CONSUMED — the BCE
/// error.rs:74 rule that keeps servo's assert!(!JS_IsExceptionPending) alive.
#[test]
fn clearing_probes_consume_throwing_proxy_hooks() {
    bun_runtime::install_exit_handler();
    bun_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bun_runtime::globals::install_all);

    ctx.eval(
        r#"
        globalThis.__throwingGet = new Proxy({}, {
            get: function () { throw new Error('get-boom'); }
        });
        globalThis.__throwingHas = new Proxy({}, {
            has: function () { throw new Error('has-boom'); }
        });
        "#,
        "<proxy-seed>",
    )
    .expect("proxy seed");

    let global = bao_engine::context::thread_realm_global()
        .filter(|g| !g.is_null())
        .expect("global");
    {
        let mut cxm = ctx.cx();
        rooted!(&in(cxm) let g_root = global);
        let mut realm = mozjs::realm::AutoRealm::new_from_handle(&mut cxm, g_root.handle());
        let realm_cx: &mut mozjs::context::JSContext = &mut realm;
        let raw = unsafe { realm_cx.raw_cx() };

        unsafe {
            use std::marker::PhantomData;
            type MutH = mozjs::jsapi::MutableHandle<mozjs::jsapi::Value>;

            // Fetch the proxy objects (plain property reads — no traps fire).
            let mut get_proxy = mozjs::jsval::UndefinedValue();
            assert!(mozjs::jsapi::JS_GetProperty(
                raw,
                g_root.handle().into(),
                c"__throwingGet".as_ptr(),
                MutH { _phantom_0: PhantomData, ptr: &mut get_proxy },
            ));
            let mut has_proxy = mozjs::jsval::UndefinedValue();
            assert!(mozjs::jsapi::JS_GetProperty(
                raw,
                g_root.handle().into(),
                c"__throwingHas".as_ptr(),
                MutH { _phantom_0: PhantomData, ptr: &mut has_proxy },
            ));
            assert!(get_proxy.is_object() && has_proxy.is_object(), "seed objects");
            rooted!(&in(realm_cx) let get_obj_root = get_proxy.to_object());
            rooted!(&in(realm_cx) let has_obj_root = has_proxy.to_object());

            // ── get hook throws → false + consumed + out=undefined ──
            let mut out = mozjs::jsval::UndefinedValue();
            let ok = bao_stealth::engine_props::get_property_clearing(
                raw,
                get_obj_root.handle().into(),
                c"anything",
                &mut out,
            );
            assert!(!ok, "throwing get must report unavailable");
            assert!(out.is_undefined(), "the out slot must be reset to undefined");
            assert!(
                !mozjs::jsapi::JS_IsExceptionPending(raw),
                "get_property_clearing must consume the pending exception"
            );

            // ── has hook throws → false + consumed + has=false ──
            let mut has = true;
            let ok = bao_stealth::engine_props::has_property_clearing(
                raw,
                has_obj_root.handle().into(),
                c"anything",
                &mut has,
            );
            assert!(!ok, "throwing has must report unavailable");
            assert!(!has, "the has slot must be reset to false");
            assert!(
                !mozjs::jsapi::JS_IsExceptionPending(raw),
                "has_property_clearing must consume the pending exception"
            );

            // ── control: a plain object probes fine through the same helpers ──
            let mut out = mozjs::jsval::UndefinedValue();
            let ok = bao_stealth::engine_props::get_property_clearing(
                raw,
                g_root.handle().into(),
                c"TextEncoder",
                &mut out,
            );
            assert!(ok && out.is_object(), "plain probe must succeed");
            let mut has = false;
            let ok = bao_stealth::engine_props::has_property_clearing(
                raw,
                g_root.handle().into(),
                c"TextEncoder",
                &mut has,
            );
            assert!(ok && has, "plain has-probe must succeed");
        }
    }
}

/// Thread-local accessors + default-profile idempotence: ensure_default_profile
/// installs the Firefox default exactly once on a fresh thread, and a later
/// explicit set_profile wins over the default (timing/canvas accessors are
/// the observable face).
#[test]
fn ensure_default_profile_idempotent_and_accessors() {
    let was_set = bao_stealth::engine_props::is_profile_set();
    bao_stealth::engine_props::ensure_default_profile();
    assert!(
        bao_stealth::engine_props::is_profile_set(),
        "ensure_default_profile must leave a profile installed"
    );

    if !was_set {
        // The installed default is the Firefox profile — the timing grid is
        // its fingerprint.
        assert_eq!(
            bao_stealth::engine_props::timing_precision_us(),
            StealthProfile::firefox_default().timing.precision_us,
            "fresh thread default must be the Firefox profile's timing grid"
        );
    }

    // Idempotence: a second call must not clobber a subsequently-set custom
    // profile... and an explicit set_profile always wins over the default.
    let mut custom = StealthProfile::chrome_default();
    custom.timing.precision_us = 555;
    bao_stealth::engine_props::set_profile(&custom);
    bao_stealth::engine_props::ensure_default_profile();
    assert_eq!(
        bao_stealth::engine_props::timing_precision_us(),
        555,
        "ensure_default_profile must be a no-op once a profile is set"
    );

    // Canvas accessors reflect the custom profile's seed/amplitude.
    assert_eq!(bao_stealth::engine_props::canvas_seed(), custom.canvas.seed());
    assert!(
        (bao_stealth::engine_props::canvas_amplitude() - custom.canvas.noise_amplitude()).abs()
            < f64::EPSILON
    );
}
