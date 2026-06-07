// @trace TEST-STL-049 [req:REQ-STL-002,REQ-STL-004,REQ-STL-007] [level:unit]
// NavigatorProfile firefox/chrome field verification, ScreenProfile construction,
// Http2Fingerprint akamai_fingerprint, settings_frame_payload, ordered_headers,
// StealthProfile firefox_default/chrome_default field completeness.

use bao_stealth::{NavigatorProfile, ScreenProfile, Http2Fingerprint, StealthProfile};

// ---- NavigatorProfile firefox ----

#[test]
fn test_nav_firefox_user_agent() {
    let nav = NavigatorProfile::firefox();
    assert!(nav.user_agent.contains("Firefox"));
    assert!(nav.user_agent.contains("Mozilla/5.0"));
}

#[test]
fn test_nav_firefox_platform() {
    assert_eq!(NavigatorProfile::firefox().platform, "Linux x86_64");
}

#[test]
fn test_nav_firefox_language() {
    assert_eq!(NavigatorProfile::firefox().language, "en-US");
}

#[test]
fn test_nav_firefox_hardware_concurrency() {
    assert_eq!(NavigatorProfile::firefox().hardware_concurrency, 8);
}

#[test]
fn test_nav_firefox_max_touch_points() {
    assert_eq!(NavigatorProfile::firefox().max_touch_points, 0);
}

#[test]
fn test_nav_firefox_vendor_empty() {
    assert!(NavigatorProfile::firefox().vendor.is_empty());
}

#[test]
fn test_nav_firefox_oscpu_some() {
    let nav = NavigatorProfile::firefox();
    assert!(nav.oscpu.is_some());
    assert!(nav.oscpu.as_ref().unwrap().contains("Linux"));
}

#[test]
fn test_nav_firefox_build_id_some() {
    let nav = NavigatorProfile::firefox();
    assert!(nav.build_id.is_some());
    assert!(!nav.build_id.as_ref().unwrap().is_empty());
}

#[test]
fn test_nav_firefox_product_sub() {
    assert_eq!(NavigatorProfile::firefox().product_sub, "20100101");
}

// ---- NavigatorProfile chrome ----

#[test]
fn test_nav_chrome_user_agent() {
    let nav = NavigatorProfile::chrome();
    assert!(nav.user_agent.contains("Chrome"));
    assert!(nav.user_agent.contains("AppleWebKit"));
}

#[test]
fn test_nav_chrome_vendor_google() {
    assert_eq!(NavigatorProfile::chrome().vendor, "Google Inc.");
}

#[test]
fn test_nav_chrome_oscpu_none() {
    assert!(NavigatorProfile::chrome().oscpu.is_none());
}

#[test]
fn test_nav_chrome_build_id_none() {
    assert!(NavigatorProfile::chrome().build_id.is_none());
}

#[test]
fn test_nav_chrome_product_sub() {
    assert_eq!(NavigatorProfile::chrome().product_sub, "20030107");
}

#[test]
fn test_nav_chrome_platform() {
    assert_eq!(NavigatorProfile::chrome().platform, "Linux x86_64");
}

#[test]
fn test_nav_chrome_max_touch_points() {
    assert_eq!(NavigatorProfile::chrome().max_touch_points, 0);
}

// ---- NavigatorProfile cross-profile ----

#[test]
fn test_nav_firefox_chrome_ua_differ() {
    assert_ne!(NavigatorProfile::firefox().user_agent, NavigatorProfile::chrome().user_agent);
}

#[test]
fn test_nav_firefox_chrome_vendor_differ() {
    assert_ne!(NavigatorProfile::firefox().vendor, NavigatorProfile::chrome().vendor);
}

#[test]
fn test_nav_firefox_chrome_oscpu_differ() {
    assert_ne!(NavigatorProfile::firefox().oscpu.is_some(), NavigatorProfile::chrome().oscpu.is_some());
}

#[test]
fn test_nav_firefox_chrome_product_sub_differ() {
    assert_ne!(NavigatorProfile::firefox().product_sub, NavigatorProfile::chrome().product_sub);
}

#[test]
fn test_nav_firefox_chrome_same_language() {
    assert_eq!(NavigatorProfile::firefox().language, NavigatorProfile::chrome().language);
}

// ---- NavigatorProfile Debug/Clone ----

#[test]
fn test_nav_debug() {
    let nav = NavigatorProfile::firefox();
    let debug = format!("{:?}", nav);
    assert!(debug.contains("NavigatorProfile"));
    assert!(debug.contains("Firefox"));
}

#[test]
fn test_nav_clone() {
    let nav = NavigatorProfile::chrome();
    let cloned = nav.clone();
    assert_eq!(cloned.user_agent, nav.user_agent);
    assert_eq!(cloned.vendor, nav.vendor);
    assert_eq!(cloned.oscpu, nav.oscpu);
    assert_eq!(cloned.build_id, nav.build_id);
}

// ---- NavigatorProfile custom construction ----

#[test]
fn test_nav_custom() {
    let nav = NavigatorProfile {
        user_agent: "CustomBot/1.0".into(),
        platform: "Win32".into(),
        language: "ja-JP".into(),
        languages: vec!["ja-JP".into(), "ja".into(), "en".into()],
        hardware_concurrency: 16,
        max_touch_points: 10,
        vendor: "CustomVendor".into(),
        app_version: "1.0".into(),
        oscpu: None,
        build_id: None,
        product_sub: "custom".into(),
        device_memory: 16.0,
    };
    assert_eq!(nav.user_agent, "CustomBot/1.0");
    assert_eq!(nav.hardware_concurrency, 16);
    assert_eq!(nav.max_touch_points, 10);
    assert!(nav.oscpu.is_none());
}

// ---- ScreenProfile default ----

#[test]
fn test_screen_default_values() {
    let s = ScreenProfile::default();
    assert_eq!(s.width, 1920);
    assert_eq!(s.height, 1080);
    assert_eq!(s.avail_width, 1920);
    assert_eq!(s.avail_height, 1040);
    assert_eq!(s.color_depth, 24);
    assert_eq!(s.pixel_depth, 24);
    assert!((s.device_pixel_ratio - 1.0).abs() < f64::EPSILON);
}

#[test]
fn test_screen_new_custom() {
    let s = ScreenProfile::new(2560, 1440, 2.0);
    assert_eq!(s.width, 2560);
    assert_eq!(s.height, 1440);
    assert_eq!(s.avail_width, 2560);
    assert_eq!(s.avail_height, 1400); // height - 40
    assert!((s.device_pixel_ratio - 2.0).abs() < f64::EPSILON);
}

#[test]
fn test_screen_new_small() {
    let s = ScreenProfile::new(800, 600, 1.0);
    assert_eq!(s.avail_height, 560);
}

#[test]
fn test_screen_new_hd() {
    let s = ScreenProfile::new(3840, 2160, 1.5);
    assert_eq!(s.width, 3840);
    assert_eq!(s.avail_height, 2120);
    assert!((s.device_pixel_ratio - 1.5).abs() < f64::EPSILON);
}

#[test]
fn test_screen_debug() {
    let s = ScreenProfile::default();
    let debug = format!("{:?}", s);
    assert!(debug.contains("ScreenProfile"));
    assert!(debug.contains("1920"));
}

#[test]
fn test_screen_clone() {
    let s = ScreenProfile::new(1024, 768, 1.25);
    let cloned = s.clone();
    assert_eq!(cloned.width, s.width);
    assert_eq!(cloned.height, s.height);
    assert!((cloned.device_pixel_ratio - s.device_pixel_ratio).abs() < f64::EPSILON);
}

// ---- Http2Fingerprint firefox ----

#[test]
fn test_http2_firefox_akamai() {
    let h2 = Http2Fingerprint::firefox();
    let fp = h2.akamai_fingerprint();
    assert!(fp.starts_with("65536:0:100:131072:16384:262144"));
}

#[test]
fn test_http2_firefox_settings_payload() {
    let h2 = Http2Fingerprint::firefox();
    let payload = h2.settings_frame_payload();
    assert_eq!(payload.len(), 6);
    assert_eq!(payload[0], (0x01, 65536));
    assert_eq!(payload[1], (0x03, 0)); // enable_push = false
    assert_eq!(payload[2], (0x04, 100));
    assert_eq!(payload[3], (0x02, 131072));
    assert_eq!(payload[4], (0x05, 16384));
    assert_eq!(payload[5], (0x06, 262144));
}

#[test]
fn test_http2_firefox_pseudo_order() {
    let h2 = Http2Fingerprint::firefox();
    assert_eq!(h2.pseudo_header_order, vec![":method", ":path", ":authority", ":scheme"]);
}

// ---- Http2Fingerprint chrome ----

#[test]
fn test_http2_chrome_akamai() {
    let h2 = Http2Fingerprint::chrome();
    let fp = h2.akamai_fingerprint();
    assert!(fp.starts_with("65536:0:1000:6291456:16384:262144"));
}

#[test]
fn test_http2_chrome_window_update() {
    let h2 = Http2Fingerprint::chrome();
    assert_eq!(h2.window_update_size, 15663105);
}

#[test]
fn test_http2_chrome_pseudo_order() {
    let h2 = Http2Fingerprint::chrome();
    assert_eq!(h2.pseudo_header_order, vec![":method", ":authority", ":scheme", ":path"]);
}

// ---- Http2Fingerprint cross-profile ----

#[test]
fn test_http2_firefox_chrome_akamai_differ() {
    assert_ne!(
        Http2Fingerprint::firefox().akamai_fingerprint(),
        Http2Fingerprint::chrome().akamai_fingerprint()
    );
}

#[test]
fn test_http2_firefox_chrome_window_differ() {
    assert_ne!(
        Http2Fingerprint::firefox().window_update_size,
        Http2Fingerprint::chrome().window_update_size
    );
}

#[test]
fn test_http2_firefox_chrome_pseudo_order_differ() {
    let ff = &Http2Fingerprint::firefox().pseudo_header_order;
    let cr = &Http2Fingerprint::chrome().pseudo_header_order;
    assert_ne!(ff, cr);
}

// ---- Http2Fingerprint ordered_headers ----

#[test]
fn test_ordered_headers_firefox_order() {
    let h2 = Http2Fingerprint::firefox();
    let headers = vec![
        (":scheme", "https"),
        (":method", "GET"),
        (":authority", "example.com"),
        (":path", "/"),
        ("host", "example.com"),
    ];
    let ordered = h2.ordered_headers(&headers);
    // Firefox order: :method, :path, :authority, :scheme
    assert_eq!(ordered[0].0, ":method");
    assert_eq!(ordered[1].0, ":path");
    assert_eq!(ordered[2].0, ":authority");
    assert_eq!(ordered[3].0, ":scheme");
    // Non-pseudo header at end
    assert_eq!(ordered[4].0, "host");
}

#[test]
fn test_ordered_headers_chrome_order() {
    let h2 = Http2Fingerprint::chrome();
    let headers = vec![
        (":path", "/"),
        (":scheme", "https"),
        (":method", "GET"),
        (":authority", "example.com"),
    ];
    let ordered = h2.ordered_headers(&headers);
    // Chrome order: :method, :authority, :scheme, :path
    assert_eq!(ordered[0].0, ":method");
    assert_eq!(ordered[1].0, ":authority");
    assert_eq!(ordered[2].0, ":scheme");
    assert_eq!(ordered[3].0, ":path");
}

#[test]
fn test_ordered_headers_no_pseudo() {
    let h2 = Http2Fingerprint::firefox();
    let headers = vec![
        ("host", "example.com"),
        ("accept", "*/*"),
    ];
    let ordered = h2.ordered_headers(&headers);
    assert_eq!(ordered.len(), 2);
    // Non-pseudo headers remain in original order
    assert_eq!(ordered[0].0, "host");
    assert_eq!(ordered[1].0, "accept");
}

#[test]
fn test_ordered_headers_empty() {
    let h2 = Http2Fingerprint::firefox();
    let ordered = h2.ordered_headers(&[]);
    assert!(ordered.is_empty());
}

#[test]
fn test_ordered_headers_only_pseudo() {
    let h2 = Http2Fingerprint::firefox();
    let headers = vec![
        (":method", "POST"),
        (":path", "/api"),
    ];
    let ordered = h2.ordered_headers(&headers);
    assert_eq!(ordered.len(), 2);
    assert_eq!(ordered[0].0, ":method");
    assert_eq!(ordered[1].0, ":path");
}

#[test]
fn test_ordered_headers_preserves_values() {
    let h2 = Http2Fingerprint::chrome();
    let headers = vec![
        (":method", "PUT"),
        (":authority", "api.test.com"),
        ("content-type", "application/json"),
    ];
    let ordered = h2.ordered_headers(&headers);
    assert_eq!(ordered[0], (":method", "PUT"));
    assert_eq!(ordered[1], (":authority", "api.test.com"));
    assert_eq!(ordered[2], ("content-type", "application/json"));
}

// ---- Http2Fingerprint Debug/Clone ----

#[test]
fn test_http2_debug() {
    let h2 = Http2Fingerprint::firefox();
    let debug = format!("{:?}", h2);
    assert!(debug.contains("Http2Fingerprint"));
    assert!(debug.contains("65536"));
}

#[test]
fn test_http2_clone() {
    let h2 = Http2Fingerprint::chrome();
    let cloned = h2.clone();
    assert_eq!(cloned.header_table_size, h2.header_table_size);
    assert_eq!(cloned.window_update_size, h2.window_update_size);
    assert_eq!(cloned.pseudo_header_order.len(), h2.pseudo_header_order.len());
}

// ---- Http2Fingerprint custom construction ----

#[test]
fn test_http2_custom() {
    let h2 = Http2Fingerprint {
        header_table_size: 4096,
        enable_push: true,
        max_concurrent_streams: 200,
        initial_window_size: 65535,
        max_frame_size: 8192,
        max_header_list_size: 32768,
        window_update_size: 65535,
        pseudo_header_order: vec![":method"],
    };
    let fp = h2.akamai_fingerprint();
    assert!(fp.starts_with("4096:1:200:65535:8192:32768"));
    assert_eq!(h2.settings_frame_payload()[1], (0x03, 1)); // enable_push = true
}

// ---- Http2Fingerprint settings_frame_payload ----

#[test]
fn test_settings_payload_frame_ids() {
    let h2 = Http2Fingerprint::firefox();
    let payload = h2.settings_frame_payload();
    let frame_ids: Vec<u16> = payload.iter().map(|(id, _)| *id).collect();
    assert_eq!(frame_ids, vec![0x01, 0x03, 0x04, 0x02, 0x05, 0x06]);
}

// ---- StealthProfile firefox_default ----

#[test]
fn test_stealth_firefox_has_tls() {
    let p = StealthProfile::firefox_default();
    let ja3 = p.tls.compute_ja3();
    assert!(!ja3.is_empty());
}

#[test]
fn test_stealth_firefox_has_http2() {
    let p = StealthProfile::firefox_default();
    let fp = p.http2.akamai_fingerprint();
    assert!(!fp.is_empty());
}

#[test]
fn test_stealth_firefox_canvas_seed() {
    let p = StealthProfile::firefox_default();
    assert_eq!(p.canvas.seed(), 42);
}

#[test]
fn test_stealth_firefox_navigator_vendor() {
    let p = StealthProfile::firefox_default();
    assert!(p.navigator.vendor.is_empty());
}

#[test]
fn test_stealth_firefox_screen_default() {
    let p = StealthProfile::firefox_default();
    assert_eq!(p.screen.width, 1920);
    assert_eq!(p.screen.height, 1080);
}

#[test]
fn test_stealth_firefox_webgl_vendor() {
    let p = StealthProfile::firefox_default();
    assert_eq!(p.webgl.vendor, "Mozilla");
}

#[test]
fn test_stealth_firefox_audio_seed() {
    let p = StealthProfile::firefox_default();
    assert_eq!(p.audio.seed(), 42);
}

#[test]
fn test_stealth_firefox_behavior_seed() {
    let p = StealthProfile::firefox_default();
    assert_eq!(p.behavior.seed(), 42);
}

// ---- StealthProfile chrome_default ----

#[test]
fn test_stealth_chrome_canvas_seed() {
    let p = StealthProfile::chrome_default();
    assert_eq!(p.canvas.seed(), 137);
}

#[test]
fn test_stealth_chrome_navigator_vendor() {
    let p = StealthProfile::chrome_default();
    assert_eq!(p.navigator.vendor, "Google Inc.");
}

#[test]
fn test_stealth_chrome_webgl_vendor() {
    let p = StealthProfile::chrome_default();
    assert_eq!(p.webgl.vendor, "Google Inc. (NVIDIA)");
}

#[test]
fn test_stealth_chrome_audio_seed() {
    let p = StealthProfile::chrome_default();
    assert_eq!(p.audio.seed(), 137);
}

// ---- StealthProfile cross-profile ----

#[test]
fn test_stealth_firefox_chrome_canvas_seed_differ() {
    let ff = StealthProfile::firefox_default();
    let cr = StealthProfile::chrome_default();
    assert_ne!(ff.canvas.seed(), cr.canvas.seed());
}

#[test]
fn test_stealth_firefox_chrome_tls_differ() {
    let ff = StealthProfile::firefox_default();
    let cr = StealthProfile::chrome_default();
    assert_ne!(ff.tls.compute_ja3(), cr.tls.compute_ja3());
}

#[test]
fn test_stealth_firefox_chrome_http2_differ() {
    let ff = StealthProfile::firefox_default();
    let cr = StealthProfile::chrome_default();
    assert_ne!(ff.http2.akamai_fingerprint(), cr.http2.akamai_fingerprint());
}

#[test]
fn test_stealth_firefox_chrome_navigator_differ() {
    let ff = StealthProfile::firefox_default();
    let cr = StealthProfile::chrome_default();
    assert_ne!(ff.navigator.user_agent, cr.navigator.user_agent);
    assert_ne!(ff.navigator.vendor, cr.navigator.vendor);
}

#[test]
fn test_stealth_firefox_chrome_webgl_differ() {
    let ff = StealthProfile::firefox_default();
    let cr = StealthProfile::chrome_default();
    assert_ne!(ff.webgl.vendor, cr.webgl.vendor);
    assert_ne!(ff.webgl.renderer, cr.webgl.renderer);
}

// ---- StealthProfile Debug/Clone ----

#[test]
fn test_stealth_debug() {
    let p = StealthProfile::firefox_default();
    let debug = format!("{:?}", p);
    assert!(debug.contains("StealthProfile"));
}

#[test]
fn test_stealth_clone() {
    let p = StealthProfile::chrome_default();
    let cloned = p.clone();
    assert_eq!(cloned.canvas.seed(), p.canvas.seed());
    assert_eq!(cloned.navigator.vendor, p.navigator.vendor);
    assert_eq!(cloned.webgl.vendor, p.webgl.vendor);
}
