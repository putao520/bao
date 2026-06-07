// @trace TEST-ENG-STEALTH-HTTP [req:REQ-STL-001] [level:integration]

use bao_engine::context::JsContext;
use bao_engine::value::JsValue;

fn eval_bool(ctx: &mut JsContext, source: &str) -> bool {
    match ctx.eval(source, "<test>") {
        Ok(JsValue::Bool(b)) => b,
        _ => false,
    }
}

#[test]
fn test_stealth_http_deep() {
    bao_runtime::install_exit_handler();
    bao_runtime::bun_api::init_process_start();
    let mut ctx = JsContext::for_test().expect("JsContext");
    ctx.set_global_setup(bao_runtime::globals::install_all);

    // === Bao global object ===
    assert!(eval_bool(&mut ctx, "typeof Bao === 'object'"), "Bao should be object");

    // === Bao === Bun alias ===
    assert!(eval_bool(&mut ctx, "Bao === Bun"), "Bao should be Bun alias");

    // === Rust-level stealth_http API tests ===
    // JA3 hash with Chrome profile
    let chrome_profile = bao_stealth::StealthProfile::chrome_default();
    let chrome_ja3 = bao_runtime::stealth_http::ja3_hash(&Some(chrome_profile.clone()));
    assert!(chrome_ja3.is_some(), "ja3_hash with Chrome profile should return Some");
    assert!(chrome_ja3.as_ref().unwrap().starts_with("771,"),
        "Chrome JA3 should start with 771, got: {}", chrome_ja3.as_ref().unwrap_or(&String::new()));

    // JA3 hash with Firefox profile
    let ff_profile = bao_stealth::StealthProfile::firefox_default();
    let ff_ja3 = bao_runtime::stealth_http::ja3_hash(&Some(ff_profile.clone()));
    assert!(ff_ja3.is_some(), "ja3_hash with Firefox profile should return Some");
    assert!(ff_ja3.as_ref().unwrap().starts_with("771,"),
        "Firefox JA3 should start with 771, got: {}", ff_ja3.as_ref().unwrap_or(&String::new()));

    // Chrome and Firefox JA3 hashes must differ
    assert_ne!(chrome_ja3, ff_ja3, "Chrome and Firefox JA3 hashes must differ");

    // JA3 hash with None profile
    let none_ja3 = bao_runtime::stealth_http::ja3_hash(&None);
    assert!(none_ja3.is_none(), "ja3_hash with None should return None");

    // Akamai fingerprint with Chrome
    let chrome_akamai = bao_runtime::stealth_http::akamai_fingerprint(&Some(chrome_profile.clone()));
    assert!(chrome_akamai.is_some(), "akamai_fingerprint with Chrome should return Some");
    let ak_parts: Vec<&str> = chrome_akamai.as_ref().unwrap().split(':').collect();
    assert_eq!(ak_parts.len(), 6, "Akamai fingerprint should have 6 colon-separated fields");

    // Akamai fingerprint with Firefox
    let ff_akamai = bao_runtime::stealth_http::akamai_fingerprint(&Some(ff_profile.clone()));
    assert!(ff_akamai.is_some(), "akamai_fingerprint with Firefox should return Some");

    // Chrome and Firefox Akamai fingerprints must differ
    assert_ne!(chrome_akamai, ff_akamai, "Chrome and Firefox Akamai fingerprints must differ");

    // Akamai fingerprint with None
    let none_akamai = bao_runtime::stealth_http::akamai_fingerprint(&None);
    assert!(none_akamai.is_none(), "akamai_fingerprint with None should return None");

    // === ordered_headers behavior ===
    let headers = vec![
        ("accept".to_string(), "*/*".to_string()),
        ("host".to_string(), "example.com".to_string()),
        ("content-type".to_string(), "text/html".to_string()),
    ];

    // No profile: preserve original order
    let ordered_none = bao_runtime::stealth_http::ordered_headers(&None, &headers);
    assert_eq!(ordered_none.len(), 3, "ordered_headers without profile should preserve count");
    assert_eq!(ordered_none[0].0, "accept");
    assert_eq!(ordered_none[1].0, "host");
    assert_eq!(ordered_none[2].0, "content-type");

    // Chrome profile: pseudo-headers first
    let pseudo_headers = vec![
        ("content-length".to_string(), "100".to_string()),
        (":method".to_string(), "GET".to_string()),
        (":authority".to_string(), "example.com".to_string()),
        (":scheme".to_string(), "https".to_string()),
        (":path".to_string(), "/".to_string()),
        ("accept".to_string(), "*/*".to_string()),
    ];
    let ordered_chrome = bao_runtime::stealth_http::ordered_headers(&Some(chrome_profile.clone()), &pseudo_headers);
    assert!(ordered_chrome[0].0.starts_with(':'),
        "Chrome: first header should be pseudo-header, got: {}", ordered_chrome[0].0);
    assert!(ordered_chrome[1].0.starts_with(':'),
        "Chrome: second header should be pseudo-header, got: {}", ordered_chrome[1].0);

    // Firefox profile: pseudo-headers first (but different order from Chrome)
    let ordered_ff = bao_runtime::stealth_http::ordered_headers(&Some(ff_profile.clone()), &pseudo_headers);
    assert!(ordered_ff[0].0.starts_with(':'),
        "Firefox: first header should be pseudo-header, got: {}", ordered_ff[0].0);

    // Empty headers
    let empty: Vec<(String, String)> = Vec::new();
    let ordered_empty = bao_runtime::stealth_http::ordered_headers(&None, &empty);
    assert!(ordered_empty.is_empty(), "ordered_headers with empty input should return empty");

    // Single header
    let single = vec![("host".to_string(), "example.com".to_string())];
    let ordered_single = bao_runtime::stealth_http::ordered_headers(&None, &single);
    assert_eq!(ordered_single.len(), 1);
    assert_eq!(ordered_single[0].0, "host");

    // === Profile difference: Chrome vs Firefox produce different fingerprints ===
    // (Only chrome_default and firefox_default are available)
    let chrome_ja3_val = bao_runtime::stealth_http::ja3_hash(&Some(chrome_profile.clone()));
    let ff_ja3_val = bao_runtime::stealth_http::ja3_hash(&Some(ff_profile.clone()));
    assert_ne!(chrome_ja3_val, ff_ja3_val, "Chrome and Firefox must produce different JA3");

    bao_runtime::shutdown_thread_sm();
}
