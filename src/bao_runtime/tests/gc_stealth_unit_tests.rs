// @trace TEST-ENG-007-GC [req:REQ-ENG-007] [level:unit]
// @trace TEST-STL-001-HTTP [req:REQ-STL-001] [level:unit]
// Unit tests for gc_store (via require caching) and stealth_http (Rust-level API)

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn eval_string(ctx: &mut JsContext, source: &str) -> String {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::String(s)) => s,
        Ok(JsValue::Number(n)) => format!("{}", n),
        Ok(JsValue::Bool(b)) => if b { "true" } else { "false" }.to_string(),
        _ => String::new(),
    }
}

#[test]
fn test_stealth_http_rust_api() {
    let profile = bao_stealth::StealthProfile::chrome_default();

    let hash = bao_runtime::stealth_http::ja3_hash(&Some(profile.clone()));
    assert!(hash.is_some(), "ja3_hash should return Some with profile");
    assert!(!hash.unwrap().is_empty(), "ja3_hash should be non-empty");

    let fp = bao_runtime::stealth_http::akamai_fingerprint(&Some(profile.clone()));
    assert!(fp.is_some(), "akamai_fingerprint should return Some with profile");

    let none_hash = bao_runtime::stealth_http::ja3_hash(&None);
    assert!(none_hash.is_none(), "ja3_hash should return None without profile");

    let headers = vec![
        ("accept".to_string(), "*/*".to_string()),
        ("host".to_string(), "example.com".to_string()),
    ];
    let ordered = bao_runtime::stealth_http::ordered_headers(&None, &headers);
    assert_eq!(ordered.len(), 2);
    assert_eq!(ordered[0].0, "accept");
    assert_eq!(ordered[1].0, "host");

    let ordered_stealth = bao_runtime::stealth_http::ordered_headers(&Some(profile), &headers);
    assert_eq!(ordered_stealth.len(), 2);

    let config_none = bao_runtime::stealth_http::create_stealth_request(&None, bun_http::Method::GET, "https://example.com", &headers, None);
    assert_eq!(config_none.method.as_str(), "GET");
    assert!(config_none.user_agent.is_none());

    let profile2 = bao_stealth::StealthProfile::firefox_default();
    let config_stealth = bao_runtime::stealth_http::create_stealth_request(&Some(profile2), bun_http::Method::POST, "https://example.com", &headers, Some(b"test"));
    assert_eq!(config_stealth.method.as_str(), "POST");
    assert!(config_stealth.user_agent.is_some());
}

#[test]
fn test_stealth_http_firefox_profile() {
    let profile = bao_stealth::StealthProfile::firefox_default();

    let hash = bao_runtime::stealth_http::ja3_hash(&Some(profile.clone()));
    assert!(hash.is_some());

    let fp = bao_runtime::stealth_http::akamai_fingerprint(&Some(profile));
    assert!(fp.is_some());
}

#[test]
fn test_gc_store_via_require() {
    bao_runtime::install_exit_handler();
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    let result = eval_string(&mut ctx, r#"
        var results = [];
        function check(label, fn) {
            try { var ok = fn(); results.push(label + (ok ? " PASS" : " FAIL")); }
            catch(e) { results.push(label + " ERR:" + (e.message || e)); }
        }

        var path1 = require('path');
        var path2 = require('path');
        check("require_cache_hit", function() { return path1 === path2; });
        check("require_path_api", function() {
            return typeof path1.join === 'function' && typeof path1.resolve === 'function';
        });

        var fs1 = require('fs');
        var fs2 = require('fs');
        check("require_fs_cache", function() { return fs1 === fs2; });

        var crypto1 = require('crypto');
        var crypto2 = require('crypto');
        check("require_crypto_cache", function() { return crypto1 === crypto2; });

        check("require_6_modules", function() {
            var mods = ['path', 'fs', 'crypto', 'events', 'url', 'util'];
            var ok = true;
            for (var i = 0; i < mods.length; i++) {
                var m = require(mods[i]);
                if (typeof m !== 'object' && typeof m !== 'function') { ok = false; break; }
            }
            return ok;
        });

        results.join("|")
    "#);

    let mut all_passed = true;
    for item in result.split('|') {
        if !item.contains(" PASS") {
            eprintln!("  FAIL: {}", item);
            all_passed = false;
        }
    }
    assert!(all_passed, "GC store / require caching tests should pass. Results: {}", result);
    bao_runtime::shutdown_thread_sm();
}
