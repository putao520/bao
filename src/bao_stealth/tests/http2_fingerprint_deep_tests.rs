// @trace TEST-STL-023 [req:REQ-STL-002] [level:unit]
// Http2Fingerprint deep tests: settings_frame_payload structure, ordered_headers
// ordering and completeness, akamai_fingerprint format, preset differentiation,
// clone/debug, edge cases.

use bao_stealth::Http2Fingerprint;

// ---- Construction ----

#[test]
fn test_http2_firefox_construction() {
    let fp = Http2Fingerprint::firefox();
    assert_eq!(fp.header_table_size, 65536);
    assert!(!fp.enable_push);
    assert_eq!(fp.max_concurrent_streams, 100);
    assert_eq!(fp.initial_window_size, 131072);
    assert_eq!(fp.max_frame_size, 16384);
    assert_eq!(fp.max_header_list_size, 262144);
}

#[test]
fn test_http2_chrome_construction() {
    let fp = Http2Fingerprint::chrome();
    assert_eq!(fp.header_table_size, 65536);
    assert!(!fp.enable_push);
    assert_eq!(fp.max_concurrent_streams, 1000);
    assert_eq!(fp.initial_window_size, 6291456);
    assert_eq!(fp.max_frame_size, 16384);
    assert_eq!(fp.max_header_list_size, 262144);
}

// ---- akamai_fingerprint format ----

#[test]
fn test_akamai_fingerprint_firefox_format() {
    let fp = Http2Fingerprint::firefox();
    let ak = fp.akamai_fingerprint();
    // Format: "header_table_size:enable_push:max_concurrent_streams:initial_window_size:max_frame_size:max_header_list_size"
    let parts: Vec<&str> = ak.split(':').collect();
    assert_eq!(parts.len(), 6);
    assert_eq!(parts[0], "65536");
    assert_eq!(parts[1], "0"); // enable_push = false
    assert_eq!(parts[2], "100");
    assert_eq!(parts[3], "131072");
    assert_eq!(parts[4], "16384");
    assert_eq!(parts[5], "262144");
}

#[test]
fn test_akamai_fingerprint_chrome_format() {
    let fp = Http2Fingerprint::chrome();
    let ak = fp.akamai_fingerprint();
    let parts: Vec<&str> = ak.split(':').collect();
    assert_eq!(parts.len(), 6);
    assert_eq!(parts[0], "65536");
    assert_eq!(parts[2], "1000");
    assert_eq!(parts[3], "6291456");
}

#[test]
fn test_akamai_fingerprint_differs_between_presets() {
    let ff = Http2Fingerprint::firefox().akamai_fingerprint();
    let ch = Http2Fingerprint::chrome().akamai_fingerprint();
    assert_ne!(ff, ch);
}

#[test]
fn test_akamai_fingerprint_enable_push_true() {
    let fp = Http2Fingerprint {
        header_table_size: 4096,
        enable_push: true,
        max_concurrent_streams: 200,
        initial_window_size: 65535,
        max_frame_size: 16384,
        max_header_list_size: 8192,
        window_update_size: 0,
        pseudo_header_order: vec![],
    };
    let ak = fp.akamai_fingerprint();
    let parts: Vec<&str> = ak.split(':').collect();
    assert_eq!(parts[1], "1");
}

#[test]
fn test_akamai_fingerprint_deterministic() {
    let fp = Http2Fingerprint::firefox();
    let a1 = fp.akamai_fingerprint();
    let a2 = fp.akamai_fingerprint();
    assert_eq!(a1, a2);
}

// ---- settings_frame_payload ----

#[test]
fn test_settings_frame_payload_firefox_count() {
    let fp = Http2Fingerprint::firefox();
    let payload = fp.settings_frame_payload();
    assert_eq!(payload.len(), 6);
}

#[test]
fn test_settings_frame_payload_firefox_values() {
    let fp = Http2Fingerprint::firefox();
    let payload = fp.settings_frame_payload();
    // Verify setting IDs: 0x01=HEADER_TABLE_SIZE, 0x03=ENABLE_PUSH, 0x04=MAX_CONCURRENT,
    // 0x02=INITIAL_WINDOW_SIZE, 0x05=MAX_FRAME_SIZE, 0x06=MAX_HEADER_LIST_SIZE
    assert_eq!(payload[0], (0x01, 65536));
    assert_eq!(payload[1], (0x03, 0)); // enable_push=false → 0
    assert_eq!(payload[2], (0x04, 100));
    assert_eq!(payload[3], (0x02, 131072));
    assert_eq!(payload[4], (0x05, 16384));
    assert_eq!(payload[5], (0x06, 262144));
}

#[test]
fn test_settings_frame_payload_chrome_values() {
    let fp = Http2Fingerprint::chrome();
    let payload = fp.settings_frame_payload();
    assert_eq!(payload[0], (0x01, 65536));
    assert_eq!(payload[2], (0x04, 1000));
    assert_eq!(payload[3], (0x02, 6291456));
}

#[test]
fn test_settings_frame_payload_enable_push_true() {
    let fp = Http2Fingerprint {
        header_table_size: 4096,
        enable_push: true,
        max_concurrent_streams: 1,
        initial_window_size: 1,
        max_frame_size: 1,
        max_header_list_size: 1,
        window_update_size: 0,
        pseudo_header_order: vec![],
    };
    let payload = fp.settings_frame_payload();
    assert_eq!(payload[1], (0x03, 1));
}

#[test]
fn test_settings_frame_payload_setting_ids_unique() {
    let fp = Http2Fingerprint::firefox();
    let payload = fp.settings_frame_payload();
    let ids: Vec<u16> = payload.iter().map(|(id, _)| *id).collect();
    let unique: std::collections::HashSet<u16> = ids.iter().copied().collect();
    assert_eq!(ids.len(), unique.len(), "Setting IDs should be unique");
}

#[test]
fn test_settings_frame_payload_all_ids_standard() {
    let fp = Http2Fingerprint::firefox();
    let payload = fp.settings_frame_payload();
    let valid_ids = [0x01u16, 0x02, 0x03, 0x04, 0x05, 0x06];
    for (id, _) in &payload {
        assert!(valid_ids.contains(id), "Non-standard setting ID: 0x{:04X}", id);
    }
}

// ---- ordered_headers ----

#[test]
fn test_ordered_headers_firefox_pseudo_order() {
    let fp = Http2Fingerprint::firefox();
    // Firefox order: :method, :path, :authority, :scheme
    assert_eq!(fp.pseudo_header_order, vec![":method", ":path", ":authority", ":scheme"]);
}

#[test]
fn test_ordered_headers_chrome_pseudo_order() {
    let fp = Http2Fingerprint::chrome();
    // Chrome order: :method, :authority, :scheme, :path
    assert_eq!(fp.pseudo_header_order, vec![":method", ":authority", ":scheme", ":path"]);
}

#[test]
fn test_ordered_headers_basic() {
    let fp = Http2Fingerprint::firefox();
    let headers = vec![
        ("content-length", "100"),
        (":method", "GET"),
        (":authority", "example.com"),
        ("accept", "*/*"),
        (":path", "/"),
        (":scheme", "https"),
    ];
    let ordered = fp.ordered_headers(&headers);
    // Pseudo headers first in firefox order
    assert_eq!(ordered[0].0, ":method");
    assert_eq!(ordered[1].0, ":path");
    assert_eq!(ordered[2].0, ":authority");
    assert_eq!(ordered[3].0, ":scheme");
    // Then remaining in original order
    assert_eq!(ordered[4].0, "content-length");
    assert_eq!(ordered[5].0, "accept");
}

#[test]
fn test_ordered_headers_chrome_different() {
    let fp = Http2Fingerprint::chrome();
    let headers = vec![
        (":path", "/"),
        (":method", "GET"),
        (":scheme", "https"),
        (":authority", "example.com"),
    ];
    let ordered = fp.ordered_headers(&headers);
    // Chrome order: :method, :authority, :scheme, :path
    assert_eq!(ordered[0].0, ":method");
    assert_eq!(ordered[1].0, ":authority");
    assert_eq!(ordered[2].0, ":scheme");
    assert_eq!(ordered[3].0, ":path");
}

#[test]
fn test_ordered_headers_no_pseudo() {
    let fp = Http2Fingerprint::firefox();
    let headers = vec![
        ("content-type", "text/html"),
        ("accept", "*/*"),
    ];
    let ordered = fp.ordered_headers(&headers);
    assert_eq!(ordered.len(), 2);
    assert_eq!(ordered[0].0, "content-type");
    assert_eq!(ordered[1].0, "accept");
}

#[test]
fn test_ordered_headers_empty() {
    let fp = Http2Fingerprint::firefox();
    let headers: Vec<(&str, &str)> = vec![];
    let ordered = fp.ordered_headers(&headers);
    assert!(ordered.is_empty());
}

#[test]
fn test_ordered_headers_only_pseudo() {
    let fp = Http2Fingerprint::firefox();
    let headers = vec![
        (":method", "POST"),
        (":path", "/api"),
        (":authority", "api.example.com"),
        (":scheme", "https"),
    ];
    let ordered = fp.ordered_headers(&headers);
    assert_eq!(ordered.len(), 4);
    assert_eq!(ordered[0].0, ":method");
    assert_eq!(ordered[1].0, ":path");
    assert_eq!(ordered[2].0, ":authority");
    assert_eq!(ordered[3].0, ":scheme");
}

#[test]
fn test_ordered_headers_preserves_values() {
    let fp = Http2Fingerprint::firefox();
    let headers = vec![
        (":method", "GET"),
        (":path", "/test?q=1"),
    ];
    let ordered = fp.ordered_headers(&headers);
    assert_eq!(ordered[0].1, "GET");
    assert_eq!(ordered[1].1, "/test?q=1");
}

#[test]
fn test_ordered_headers_missing_pseudo() {
    let fp = Http2Fingerprint::firefox();
    let headers = vec![
        (":method", "GET"),
        ("accept", "*/*"),
    ];
    let ordered = fp.ordered_headers(&headers);
    assert_eq!(ordered.len(), 2);
    assert_eq!(ordered[0].0, ":method");
    assert_eq!(ordered[1].0, "accept");
}

#[test]
fn test_ordered_headers_duplicate_non_pseudo() {
    let fp = Http2Fingerprint::firefox();
    let headers = vec![
        (":method", "GET"),
        ("accept", "text/html"),
        ("accept", "application/json"),
    ];
    let ordered = fp.ordered_headers(&headers);
    assert_eq!(ordered.len(), 3);
    assert_eq!(ordered[0].0, ":method");
    assert_eq!(ordered[1].0, "accept");
    assert_eq!(ordered[2].0, "accept");
}

// ---- Clone ----

#[test]
fn test_http2_fingerprint_clone() {
    let fp = Http2Fingerprint::firefox();
    let cloned = fp.clone();
    assert_eq!(fp.header_table_size, cloned.header_table_size);
    assert_eq!(fp.enable_push, cloned.enable_push);
    assert_eq!(fp.max_concurrent_streams, cloned.max_concurrent_streams);
    assert_eq!(fp.initial_window_size, cloned.initial_window_size);
    assert_eq!(fp.max_frame_size, cloned.max_frame_size);
    assert_eq!(fp.max_header_list_size, cloned.max_header_list_size);
    assert_eq!(fp.window_update_size, cloned.window_update_size);
    assert_eq!(fp.pseudo_header_order, cloned.pseudo_header_order);
}

#[test]
fn test_http2_fingerprint_clone_same_fingerprint() {
    let fp = Http2Fingerprint::firefox();
    let cloned = fp.clone();
    assert_eq!(fp.akamai_fingerprint(), cloned.akamai_fingerprint());
}

// ---- Debug ----

#[test]
fn test_http2_fingerprint_debug() {
    let fp = Http2Fingerprint::firefox();
    let debug = format!("{:?}", fp);
    assert!(debug.contains("Http2Fingerprint") || debug.contains("65536"));
}

#[test]
fn test_http2_fingerprint_debug_chrome() {
    let fp = Http2Fingerprint::chrome();
    let debug = format!("{:?}", fp);
    assert!(debug.contains("6291456") || debug.contains("Http2Fingerprint"));
}

// ---- Preset differentiation ----

#[test]
fn test_presets_differ_in_window_size() {
    let ff = Http2Fingerprint::firefox();
    let ch = Http2Fingerprint::chrome();
    assert_ne!(ff.initial_window_size, ch.initial_window_size);
}

#[test]
fn test_presets_differ_in_concurrent_streams() {
    let ff = Http2Fingerprint::firefox();
    let ch = Http2Fingerprint::chrome();
    assert_ne!(ff.max_concurrent_streams, ch.max_concurrent_streams);
}

#[test]
fn test_presets_differ_in_window_update() {
    let ff = Http2Fingerprint::firefox();
    let ch = Http2Fingerprint::chrome();
    assert_ne!(ff.window_update_size, ch.window_update_size);
}

#[test]
fn test_presets_differ_in_pseudo_order() {
    let ff = Http2Fingerprint::firefox();
    let ch = Http2Fingerprint::chrome();
    assert_ne!(ff.pseudo_header_order, ch.pseudo_header_order);
}

#[test]
fn test_presets_share_common_values() {
    let ff = Http2Fingerprint::firefox();
    let ch = Http2Fingerprint::chrome();
    // Both disable push
    assert!(!ff.enable_push);
    assert!(!ch.enable_push);
    // Both have same header_table_size
    assert_eq!(ff.header_table_size, ch.header_table_size);
    // Both have same max_frame_size
    assert_eq!(ff.max_frame_size, ch.max_frame_size);
}

// ---- Custom fingerprint ----

#[test]
fn test_custom_fingerprint() {
    let fp = Http2Fingerprint {
        header_table_size: 8192,
        enable_push: false,
        max_concurrent_streams: 50,
        initial_window_size: 32768,
        max_frame_size: 8192,
        max_header_list_size: 16384,
        window_update_size: 32768,
        pseudo_header_order: vec![":method", ":scheme"],
    };
    assert_eq!(fp.header_table_size, 8192);
    assert_eq!(fp.pseudo_header_order.len(), 2);
    let ak = fp.akamai_fingerprint();
    assert!(ak.starts_with("8192:"));
}

#[test]
fn test_custom_fingerprint_settings_payload() {
    let fp = Http2Fingerprint {
        header_table_size: 2048,
        enable_push: true,
        max_concurrent_streams: 10,
        initial_window_size: 4096,
        max_frame_size: 2048,
        max_header_list_size: 4096,
        window_update_size: 0,
        pseudo_header_order: vec![],
    };
    let payload = fp.settings_frame_payload();
    assert_eq!(payload[0], (0x01, 2048));
    assert_eq!(payload[1], (0x03, 1)); // enable_push=true → 1
}

// ---- Edge: zero values ----

#[test]
fn test_zero_concurrent_streams() {
    let fp = Http2Fingerprint {
        header_table_size: 0,
        enable_push: false,
        max_concurrent_streams: 0,
        initial_window_size: 0,
        max_frame_size: 0,
        max_header_list_size: 0,
        window_update_size: 0,
        pseudo_header_order: vec![],
    };
    let ak = fp.akamai_fingerprint();
    assert_eq!(ak, "0:0:0:0:0:0");
}

#[test]
fn test_zero_settings_payload() {
    let fp = Http2Fingerprint {
        header_table_size: 0,
        enable_push: false,
        max_concurrent_streams: 0,
        initial_window_size: 0,
        max_frame_size: 0,
        max_header_list_size: 0,
        window_update_size: 0,
        pseudo_header_order: vec![],
    };
    let payload = fp.settings_frame_payload();
    for (id, val) in &payload {
        if *id != 0x03 {
            assert_eq!(*val, 0);
        }
    }
}
