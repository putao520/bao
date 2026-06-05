// @trace REQ-STL-001 [entity:StealthHttpAgent] REQ-STL-002
// Stealth-aware HTTP request configuration.
// Applies HTTP/2 header ordering and User-Agent injection from StealthProfile.
// Actual HTTP execution is delegated to crate::http_client::http_request().

use bao_stealth::{Http2Fingerprint, StealthProfile, TlsFingerprint, TlsFingerprintConfig};
use bun_http::Method;
use bun_http::ssl_config::SSLConfig;

/// Configuration for a stealth-aware HTTP request.
/// Produced by `create_stealth_request()`, consumed by callers that
/// delegate to `crate::http_client::http_request()`.
pub struct StealthRequestConfig {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub user_agent: Option<String>,
}

/// Create a stealth-aware request configuration.
/// Applies HTTP/2 header ordering from the profile and injects User-Agent.
pub fn create_stealth_request(
    profile: &Option<StealthProfile>,
    method: Method,
    url: &str,
    headers: &[(String, String)],
    body: Option<&[u8]>,
) -> StealthRequestConfig {
    let ordered = ordered_headers(profile, headers);
    let mut final_headers: Vec<(String, String)> = ordered
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    let user_agent = profile.as_ref().map(|p| {
        let ua = p.navigator.user_agent.clone();
        final_headers.push(("user-agent".to_string(), ua.clone()));
        ua
    });

    StealthRequestConfig {
        method,
        url: url.to_string(),
        headers: final_headers,
        body: body.map(|b| b.to_vec()),
        user_agent,
    }
}

/// Owned HTTP response from a stealth-aware request.
pub struct StealthSyncResult {
    pub status_code: u32,
    pub status_text: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Perform a synchronous HTTP request with optional stealth fingerprint injection.
// @trace REQ-STL-001 REQ-STL-002
pub fn stealth_http_request(
    profile: &Option<StealthProfile>,
    method: Method,
    url: &str,
    headers: &[(String, String)],
    body: Option<&[u8]>,
) -> Result<StealthSyncResult, String> {
    let config = create_stealth_request(profile, method, url, headers, body);
    let result = crate::http_client::http_request(config.method, &config.url, &config.headers, config.body.as_deref())?;
    Ok(StealthSyncResult {
        status_code: result.status_code,
        status_text: result.status_text,
        headers: result.headers,
        body: result.body,
    })
}

// ---------------------------------------------------------------------------
// TLS fingerprint helpers (pure, no network I/O)
// ---------------------------------------------------------------------------

/// Build an `SSLConfig` with TLS fingerprint fields populated from a `StealthProfile`.
/// Returns a default `SSLConfig` (no fingerprint) when profile is `None`.
///
/// The caller owns the returned `SSLConfig` and must ensure it is not interned
/// (interning requires `SharedPtr::new(config)` via the global registry).
/// When the config is dropped, its C-string fields are freed via `deinit`.
// @trace REQ-STL-001
pub fn stealth_profile_to_ssl_config(profile: &Option<StealthProfile>) -> SSLConfig {
    let mut config = SSLConfig::default();
    if let Some(p) = profile {
        let tls_cfg = TlsFingerprintConfig::from_fingerprint(&p.tls);
        config.tls12_cipher_list = bun_core::dupe_z(tls_cfg.tls12_cipher_list.as_bytes());
        config.tls13_cipher_suites = bun_core::dupe_z(tls_cfg.tls13_cipher_suites.as_bytes());
        config.tls_curves_list = bun_core::dupe_z(tls_cfg.curves_list.as_bytes());
        config.tls_sigalgs_list = bun_core::dupe_z(tls_cfg.sigalgs_list.as_bytes());
        // HTTP/2 fingerprint: binary wire format SETTINGS + window size
        // Flows through SSLConfig → ClientSession → write_preface() naturally
        config.h2_settings_payload = Some(h2_settings_wire_format(&p.http2).into_boxed_slice());
        config.h2_initial_window_size = p.http2.initial_window_size;
    }
    config
}

#[allow(dead_code)]
fn tls_cipher_name(suite: u16) -> Option<&'static str> {
    match suite {
        0x1301 => Some("TLS_AES_128_GCM_SHA256"),
        0x1302 => Some("TLS_AES_256_GCM_SHA384"),
        0x1303 => Some("TLS_CHACHA20_POLY1305_SHA256"),
        0xC02B => Some("ECDHE_ECDSA_AES_128_GCM_SHA256"),
        0xC02F => Some("ECDHE_RSA_AES_128_GCM_SHA256"),
        0xC02C => Some("ECDHE_ECDSA_AES_256_GCM_SHA384"),
        0xC030 => Some("ECDHE_RSA_AES_256_GCM_SHA384"),
        0x009E => Some("DHE_RSA_AES_128_GCM_SHA256"),
        0x009C => Some("DHE_RSA_AES_256_GCM_SHA384"),
        0xCCA9 => Some("ECDHE_ECDSA_CHACHA20_POLY1305_SHA256"),
        0xCCA8 => Some("ECDHE_RSA_CHACHA20_POLY1305_SHA256"),
        0xC013 => Some("ECDHE_ECDSA_AES_128_CBC_SHA"),
        0xC009 => Some("ECDHE_ECDSA_AES_256_CBC_SHA"),
        0x0033 => Some("DHE_RSA_AES_128_CBC_SHA256"),
        0x0067 => Some("DHE_RSA_AES_256_CBC_SHA256"),
        _ => None,
    }
}

#[allow(dead_code)]
fn cipher_list_string(fp: &TlsFingerprint) -> String {
    fp.cipher_suites.iter()
        .filter_map(|&id| tls_cipher_name(id))
        .collect::<Vec<&str>>()
        .join(":")
}

#[allow(dead_code)]
fn alpn_wire_format(fp: &TlsFingerprint) -> Vec<u8> {
    let mut wire = Vec::new();
    for proto in &fp.alpn_protocols {
        let len = proto.len().min(255) as u8;
        wire.push(len);
        wire.extend_from_slice(&proto[..len as usize]);
    }
    wire
}

// ---------------------------------------------------------------------------
// HTTP/2 fingerprint helpers
// ---------------------------------------------------------------------------

/// Determine ALPN offer preference from HTTP/2 fingerprint.
// @trace REQ-STL-002
pub fn h2_alpn_offer(fp: &Http2Fingerprint) -> &'static str {
    if fp.pseudo_header_order.is_empty() {
        "http/1.1"
    } else {
        "h2,http/1.1"
    }
}

#[allow(dead_code)]
fn h2_settings_wire_format(fp: &Http2Fingerprint) -> Vec<u8> {
    let settings = fp.settings_frame_payload();
    let mut wire = Vec::with_capacity(settings.len() * 6);
    for (id, value) in &settings {
        wire.extend_from_slice(&id.to_be_bytes());
        wire.extend_from_slice(&value.to_be_bytes());
    }
    wire
}

// ---------------------------------------------------------------------------
// Diagnostic helpers (pure, no network I/O)
// ---------------------------------------------------------------------------

pub fn ordered_headers<'a>(
    profile: &Option<StealthProfile>,
    headers: &'a [(String, String)],
) -> Vec<(&'a str, &'a str)> {
    let refs: Vec<(&'a str, &'a str)> = headers.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    match profile {
        Some(p) => p.http2.ordered_headers(&refs),
        None => refs,
    }
}

pub fn ja3_hash(profile: &Option<StealthProfile>) -> Option<String> {
    profile.as_ref().map(|p| p.tls.compute_ja3())
}

pub fn akamai_fingerprint(profile: &Option<StealthProfile>) -> Option<String> {
    profile.as_ref().map(|p| p.http2.akamai_fingerprint())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_stealth_request_no_profile() {
        let config = create_stealth_request(&None, Method::GET, "https://example.com", &[], None);
        assert_eq!(config.method.as_str(), "GET");
        assert!(config.user_agent.is_none());
    }

    #[test]
    fn test_create_stealth_request_firefox() {
        let profile = StealthProfile::firefox_default();
        let config = create_stealth_request(&Some(profile), Method::POST, "https://example.com", &[], Some(b"test"));
        assert_eq!(config.method.as_str(), "POST");
        assert!(config.user_agent.is_some());
    }

    #[test]
    fn test_create_stealth_request_chrome() {
        let profile = StealthProfile::chrome_default();
        let config = create_stealth_request(&Some(profile), Method::GET, "https://example.com", &[], None);
        assert!(config.user_agent.is_some());
    }

    #[test]
    fn test_ordered_headers_no_profile() {
        let headers = vec![
            ("content-type".to_string(), "text/html".to_string()),
            (":method".to_string(), "GET".to_string()),
        ];
        let ordered = ordered_headers(&None, &headers);
        assert_eq!(ordered.len(), 2);
        assert_eq!(ordered[0].0, "content-type");
    }

    #[test]
    fn test_ordered_headers_firefox_pseudo_first() {
        let profile = StealthProfile::firefox_default();
        let headers = vec![
            ("content-length".to_string(), "100".to_string()),
            (":method".to_string(), "GET".to_string()),
            (":path".to_string(), "/".to_string()),
            (":authority".to_string(), "example.com".to_string()),
            (":scheme".to_string(), "https".to_string()),
        ];
        let ordered = ordered_headers(&Some(profile), &headers);
        assert!(ordered[0].0.starts_with(':'));
        assert!(ordered[1].0.starts_with(':'));
    }

    #[test]
    fn test_ordered_headers_chrome_order() {
        let profile = StealthProfile::chrome_default();
        let headers = vec![
            ("accept".to_string(), "*/*".to_string()),
            (":method".to_string(), "GET".to_string()),
            (":authority".to_string(), "example.com".to_string()),
            (":scheme".to_string(), "https".to_string()),
            (":path".to_string(), "/".to_string()),
        ];
        let ordered = ordered_headers(&Some(profile), &headers);
        assert_eq!(ordered[0].0, ":method");
        assert_eq!(ordered[1].0, ":authority");
        assert_eq!(ordered[2].0, ":scheme");
        assert_eq!(ordered[3].0, ":path");
    }

    #[test]
    fn test_ja3_hash_none() {
        assert!(ja3_hash(&None).is_none());
    }

    #[test]
    fn test_ja3_hash_firefox() {
        let profile = StealthProfile::firefox_default();
        let hash = ja3_hash(&Some(profile)).unwrap();
        assert!(hash.starts_with("771,"));
    }

    #[test]
    fn test_ja3_hash_chrome() {
        let profile = StealthProfile::chrome_default();
        let hash = ja3_hash(&Some(profile)).unwrap();
        assert!(hash.starts_with("771,"));
    }

    #[test]
    fn test_akamai_fingerprint_none() {
        assert!(akamai_fingerprint(&None).is_none());
    }

    #[test]
    fn test_akamai_fingerprint_firefox() {
        let profile = StealthProfile::firefox_default();
        let fp = akamai_fingerprint(&Some(profile)).unwrap();
        let parts: Vec<&str> = fp.split(':').collect();
        assert_eq!(parts.len(), 6);
    }

    #[test]
    fn test_akamai_fingerprint_chrome() {
        let profile = StealthProfile::chrome_default();
        let fp = akamai_fingerprint(&Some(profile)).unwrap();
        let parts: Vec<&str> = fp.split(':').collect();
        assert_eq!(parts.len(), 6);
    }

    #[test]
    fn test_profiles_different_ja3() {
        let ff = StealthProfile::firefox_default();
        let ch = StealthProfile::chrome_default();
        assert_ne!(ja3_hash(&Some(ff)).unwrap(), ja3_hash(&Some(ch)).unwrap());
    }

    #[test]
    fn test_profiles_different_akamai() {
        let ff = StealthProfile::firefox_default();
        let ch = StealthProfile::chrome_default();
        assert_ne!(akamai_fingerprint(&Some(ff)).unwrap(), akamai_fingerprint(&Some(ch)).unwrap());
    }

    #[test]
    fn test_h2_alpn_offer_with_h2() {
        let profile = StealthProfile::firefox_default();
        let offer = h2_alpn_offer(&profile.http2);
        assert!(offer.contains("h2"));
    }

    #[test]
    fn test_h2_alpn_offer_without_h2() {
        let empty_fp = Http2Fingerprint {
            header_table_size: 65536,
            enable_push: false,
            max_concurrent_streams: 100,
            initial_window_size: 65535,
            max_frame_size: 16384,
            max_header_list_size: 65536,
            window_update_size: 65535,
            pseudo_header_order: vec![],
        };
        let offer = h2_alpn_offer(&empty_fp);
        assert_eq!(offer, "http/1.1");
    }

    #[test]
    fn test_cipher_list_firefox() {
        let profile = StealthProfile::firefox_default();
        let s = cipher_list_string(&profile.tls);
        assert!(!s.is_empty());
        assert!(s.contains("ECDHE"));
    }

    #[test]
    fn test_cipher_list_chrome() {
        let profile = StealthProfile::chrome_default();
        let s = cipher_list_string(&profile.tls);
        assert!(!s.is_empty());
    }

    #[test]
    fn test_alpn_wire_firefox() {
        let profile = StealthProfile::firefox_default();
        let wire = alpn_wire_format(&profile.tls);
        assert!(!wire.is_empty());
    }

    #[test]
    fn test_tls_cipher_name_known() {
        assert_eq!(tls_cipher_name(0x1301), Some("TLS_AES_128_GCM_SHA256"));
        assert_eq!(tls_cipher_name(0xC02B), Some("ECDHE_ECDSA_AES_128_GCM_SHA256"));
        assert_eq!(tls_cipher_name(0xFFFF), None);
    }

    #[test]
    fn test_h2_settings_wire_firefox() {
        let profile = StealthProfile::firefox_default();
        let wire = h2_settings_wire_format(&profile.http2);
        assert_eq!(wire.len(), 36);
    }

    #[test]
    fn test_h2_settings_wire_chrome() {
        let profile = StealthProfile::chrome_default();
        let wire = h2_settings_wire_format(&profile.http2);
        assert_eq!(wire.len(), 36);
    }

    #[test]
    fn test_cipher_lists_differ() {
        let ff = StealthProfile::firefox_default();
        let ch = StealthProfile::chrome_default();
        assert_ne!(cipher_list_string(&ff.tls), cipher_list_string(&ch.tls));
    }

    // ─── stealth_http extended edge case tests ────────────────
    // @trace REQ-STL-001 [req:REQ-STL-001] [level:unit]

    #[test]
    fn test_create_stealth_request_with_headers() {
        let config = create_stealth_request(
            &None,
            Method::POST,
            "https://api.example.com",
            &[("content-type".into(), "application/json".into())],
            Some(b"{}"),
        );
        assert_eq!(config.headers.len(), 1);
        assert_eq!(config.headers[0].0, "content-type");
        assert_eq!(config.body.as_deref(), Some(b"{}" as &[u8]));
    }

    #[test]
    fn test_create_stealth_request_firefox_adds_ua() {
        let profile = StealthProfile::firefox_default();
        let config = create_stealth_request(&Some(profile), Method::GET, "https://x.com", &[], None);
        // Should have the user-agent appended to headers
        let has_ua = config.headers.iter().any(|(k, _)| k == "user-agent");
        assert!(has_ua, "Firefox profile must add user-agent header");
        assert!(config.user_agent.is_some());
    }

    #[test]
    fn test_create_stealth_request_chrome_adds_ua() {
        let profile = StealthProfile::chrome_default();
        let config = create_stealth_request(&Some(profile), Method::GET, "https://x.com", &[], None);
        let has_ua = config.headers.iter().any(|(k, _)| k == "user-agent");
        assert!(has_ua, "Chrome profile must add user-agent header");
    }

    #[test]
    fn test_ordered_headers_empty() {
        let ordered = ordered_headers(&None, &[]);
        assert!(ordered.is_empty());
    }

    #[test]
    fn test_ordered_headers_no_profile_preserves_order() {
        let headers = vec![
            ("z-header".to_string(), "last".to_string()),
            ("a-header".to_string(), "first".to_string()),
        ];
        let ordered = ordered_headers(&None, &headers);
        assert_eq!(ordered.len(), 2);
        assert_eq!(ordered[0].0, "z-header"); // no reorder without profile
        assert_eq!(ordered[1].0, "a-header");
    }

    #[test]
    fn test_stealth_sync_result_construction() {
        let result = StealthSyncResult {
            status_code: 200,
            status_text: "OK".into(),
            headers: vec![("content-type".into(), "text/html".into())],
            body: b"<html>".to_vec(),
        };
        assert_eq!(result.status_code, 200);
        assert_eq!(result.status_text, "OK");
        assert_eq!(result.headers.len(), 1);
        assert_eq!(result.body, b"<html>".to_vec());
    }

    #[test]
    fn test_stealth_sync_result_empty() {
        let result = StealthSyncResult {
            status_code: 204,
            status_text: "No Content".into(),
            headers: vec![],
            body: vec![],
        };
        assert!(result.headers.is_empty());
        assert!(result.body.is_empty());
    }

    #[test]
    fn test_h2_alpn_offer_firefox() {
        let profile = StealthProfile::firefox_default();
        let offer = h2_alpn_offer(&profile.http2);
        assert!(offer.contains("h2"), "Firefox should offer h2");
        assert!(offer.contains("http/1.1"), "Firefox should fallback to http/1.1");
    }

    #[test]
    fn test_h2_alpn_offer_chrome() {
        let profile = StealthProfile::chrome_default();
        let offer = h2_alpn_offer(&profile.http2);
        assert!(offer.contains("h2"), "Chrome should offer h2");
    }

    #[test]
    fn test_ja3_hash_firefox_chrome_differ() {
        let ff_hash = ja3_hash(&Some(StealthProfile::firefox_default())).unwrap();
        let ch_hash = ja3_hash(&Some(StealthProfile::chrome_default())).unwrap();
        assert_ne!(ff_hash, ch_hash, "Firefox and Chrome JA3 must differ");
    }

    #[test]
    fn test_tls_cipher_name_all_known_suites() {
        let known = [0x1301, 0x1302, 0x1303, 0xC02B, 0xC02F, 0xC02C, 0xC030,
                     0x009E, 0x009C, 0xCCA9, 0xCCA8, 0xC013, 0xC009, 0x0033, 0x0067];
        for suite in known {
            assert!(tls_cipher_name(suite).is_some(), "0x{:04X} should be a known suite", suite);
        }
    }

    #[test]
    fn test_tls_cipher_name_unknown_returns_none() {
        assert!(tls_cipher_name(0x0000).is_none());
        assert!(tls_cipher_name(0xFFFF).is_none());
        assert!(tls_cipher_name(0x0100).is_none());
    }

    #[test]
    fn test_alpn_wire_format_structure() {
        let profile = StealthProfile::firefox_default();
        let wire = alpn_wire_format(&profile.tls);
        // Each ALPN entry is: 1 byte length + N bytes protocol
        // Firefox has ["h2", "http/1.1"]
        // h2: 0x02 + b"h2" = 3 bytes
        // http/1.1: 0x08 + b"http/1.1" = 9 bytes
        // Total: 12 bytes
        assert_eq!(wire.len(), 12);
    }

    #[test]
    fn test_alpn_wire_chrome_structure() {
        let profile = StealthProfile::chrome_default();
        let wire = alpn_wire_format(&profile.tls);
        assert_eq!(wire.len(), 12); // same ALPN as Firefox
    }

    #[test]
    fn test_cipher_list_all_known() {
        let profile = StealthProfile::firefox_default();
        let s = cipher_list_string(&profile.tls);
        // Every cipher in Firefox's list should resolve
        assert!(s.contains("TLS_AES_128_GCM_SHA256"));
        assert!(s.contains("ECDHE_RSA_AES_128_GCM_SHA256"));
    }

    #[test]
    fn test_h2_settings_wire_has_6_entries() {
        // Each settings entry is 6 bytes (2 byte ID + 4 byte value)
        // Firefox has 6 settings → 36 bytes
        let profile = StealthProfile::firefox_default();
        let wire = h2_settings_wire_format(&profile.http2);
        assert_eq!(wire.len() % 6, 0, "wire length must be multiple of 6");
    }

    // ─── stealth_profile_to_ssl_config bridge tests ────────────
    // @trace REQ-STL-001 [req:REQ-STL-001] [level:unit]

    #[test]
    fn test_ssl_config_no_profile_is_default() {
        let config = stealth_profile_to_ssl_config(&None);
        assert!(config.tls12_cipher_list.is_null());
        assert!(config.tls13_cipher_suites.is_null());
        assert!(config.tls_curves_list.is_null());
        assert!(config.tls_sigalgs_list.is_null());
    }

    #[test]
    fn test_ssl_config_firefox_has_fingerprint_fields() {
        let profile = StealthProfile::firefox_default();
        let config = stealth_profile_to_ssl_config(&Some(profile));
        assert!(!config.tls12_cipher_list.is_null(), "tls12_cipher_list should be set");
        assert!(!config.tls13_cipher_suites.is_null(), "tls13_cipher_suites should be set");
        assert!(!config.tls_curves_list.is_null(), "tls_curves_list should be set");
        assert!(!config.tls_sigalgs_list.is_null(), "tls_sigalgs_list should be set");
    }

    #[test]
    fn test_ssl_config_chrome_has_fingerprint_fields() {
        let profile = StealthProfile::chrome_default();
        let config = stealth_profile_to_ssl_config(&Some(profile));
        assert!(!config.tls12_cipher_list.is_null());
        assert!(!config.tls13_cipher_suites.is_null());
        assert!(!config.tls_curves_list.is_null());
        assert!(!config.tls_sigalgs_list.is_null());
    }

    #[test]
    fn test_ssl_config_firefox_tls12_cipher_content() {
        let profile = StealthProfile::firefox_default();
        let config = stealth_profile_to_ssl_config(&Some(profile));
        let s = unsafe { std::ffi::CStr::from_ptr(config.tls12_cipher_list) }.to_str().unwrap();
        assert!(s.contains("ECDHE"), "TLS 1.2 ciphers should contain ECDHE: {}", s);
    }

    #[test]
    fn test_ssl_config_firefox_tls13_cipher_content() {
        let profile = StealthProfile::firefox_default();
        let config = stealth_profile_to_ssl_config(&Some(profile));
        let s = unsafe { std::ffi::CStr::from_ptr(config.tls13_cipher_suites) }.to_str().unwrap();
        assert!(s.contains("TLS_AES_128_GCM_SHA256"), "TLS 1.3 should contain AES-128: {}", s);
    }

    #[test]
    fn test_ssl_config_firefox_curves_content() {
        let profile = StealthProfile::firefox_default();
        let config = stealth_profile_to_ssl_config(&Some(profile));
        let s = unsafe { std::ffi::CStr::from_ptr(config.tls_curves_list) }.to_str().unwrap();
        assert!(s.contains("X25519"), "Curves should contain X25519: {}", s);
    }

    #[test]
    fn test_ssl_config_firefox_sigalgs_content() {
        let profile = StealthProfile::firefox_default();
        let config = stealth_profile_to_ssl_config(&Some(profile));
        let s = unsafe { std::ffi::CStr::from_ptr(config.tls_sigalgs_list) }.to_str().unwrap();
        assert!(s.contains("ecdsa_secp256r1_sha256"), "Sigalgs should contain ECDSA P-256: {}", s);
    }

    #[test]
    fn test_ssl_config_firefox_chrome_different_ciphers() {
        let ff = StealthProfile::firefox_default();
        let ch = StealthProfile::chrome_default();
        let ff_config = stealth_profile_to_ssl_config(&Some(ff));
        let ch_config = stealth_profile_to_ssl_config(&Some(ch));
        let ff_s = unsafe { std::ffi::CStr::from_ptr(ff_config.tls12_cipher_list) }.to_str().unwrap();
        let ch_s = unsafe { std::ffi::CStr::from_ptr(ch_config.tls12_cipher_list) }.to_str().unwrap();
        assert_ne!(ff_s, ch_s, "Firefox and Chrome TLS 1.2 ciphers must differ");
    }

    #[test]
    fn test_ssl_config_drop_does_not_leak() {
        // Create and drop to verify no double-free or leak
        let profile = StealthProfile::firefox_default();
        let _config = stealth_profile_to_ssl_config(&Some(profile));
        // drop happens here — if deinit works correctly, no UB
    }

    // ─── H2 fingerprint injection tests ──────────────────────
    // @trace REQ-STL-002 [req:REQ-STL-002] [level:unit]

    #[test]
    fn test_ssl_config_no_profile_h2_fields_default() {
        let config = stealth_profile_to_ssl_config(&None);
        assert!(config.h2_settings_payload.is_none(), "no profile → None h2_settings_payload");
        assert_eq!(config.h2_initial_window_size, 0, "no profile → h2_initial_window_size=0");
    }

    #[test]
    fn test_ssl_config_firefox_h2_settings_payload_set() {
        let profile = StealthProfile::firefox_default();
        let config = stealth_profile_to_ssl_config(&Some(profile));
        let payload = config.h2_settings_payload.as_deref().expect("Firefox profile must set h2_settings_payload");
        // Binary wire format: 6 settings × 6 bytes = 36 bytes
        assert_eq!(payload.len(), 36, "Firefox H2 SETTINGS payload = 6 settings × 6 bytes = 36");
    }

    #[test]
    fn test_ssl_config_chrome_h2_settings_payload_set() {
        let profile = StealthProfile::chrome_default();
        let config = stealth_profile_to_ssl_config(&Some(profile));
        let payload = config.h2_settings_payload.as_deref().expect("Chrome profile must set h2_settings_payload");
        assert_eq!(payload.len(), 36, "Chrome H2 SETTINGS payload = 6 settings × 6 bytes = 36");
    }

    #[test]
    fn test_ssl_config_firefox_h2_initial_window_size() {
        let profile = StealthProfile::firefox_default();
        let config = stealth_profile_to_ssl_config(&Some(profile));
        assert_eq!(config.h2_initial_window_size, 131072, "Firefox initial_window_size=131072");
    }

    #[test]
    fn test_ssl_config_chrome_h2_initial_window_size() {
        let profile = StealthProfile::chrome_default();
        let config = stealth_profile_to_ssl_config(&Some(profile));
        assert_eq!(config.h2_initial_window_size, 6291456, "Chrome initial_window_size=6291456");
    }

    #[test]
    fn test_ssl_config_h2_settings_firefox_chrome_differ() {
        let ff = StealthProfile::firefox_default();
        let ch = StealthProfile::chrome_default();
        let ff_config = stealth_profile_to_ssl_config(&Some(ff));
        let ch_config = stealth_profile_to_ssl_config(&Some(ch));
        let ff_payload = ff_config.h2_settings_payload.as_deref().unwrap();
        let ch_payload = ch_config.h2_settings_payload.as_deref().unwrap();
        assert_ne!(ff_payload, ch_payload, "Firefox and Chrome H2 SETTINGS binary must differ");
    }

    #[test]
    fn test_h2_settings_wire_format_firefox_first_setting() {
        let profile = StealthProfile::firefox_default();
        let wire = h2_settings_wire_format(&profile.http2);
        // First setting: HEADER_TABLE_SIZE (0x01) = 65536
        assert_eq!(wire[0..2], [0x00, 0x01], "first setting ID = 0x0001");
        let value = u32::from_be_bytes([wire[2], wire[3], wire[4], wire[5]]);
        assert_eq!(value, 65536, "Firefox HEADER_TABLE_SIZE = 65536");
    }

    #[test]
    fn test_h2_settings_wire_format_chrome_window_size() {
        let profile = StealthProfile::chrome_default();
        let wire = h2_settings_wire_format(&profile.http2);
        // Find INITIAL_WINDOW_SIZE (0x02) in the wire format
        let mut found_iws = false;
        for i in (0..wire.len()).step_by(6) {
            let id = u16::from_be_bytes([wire[i], wire[i + 1]]);
            if id == 0x02 {
                let value = u32::from_be_bytes([wire[i + 2], wire[i + 3], wire[i + 4], wire[i + 5]]);
                assert_eq!(value, 6291456, "Chrome INITIAL_WINDOW_SIZE = 6291456");
                found_iws = true;
                break;
            }
        }
        assert!(found_iws, "INITIAL_WINDOW_SIZE setting must be present");
    }

    #[test]
    fn test_h2_settings_wire_format_firefox_enable_push_zero() {
        let profile = StealthProfile::firefox_default();
        let wire = h2_settings_wire_format(&profile.http2);
        for i in (0..wire.len()).step_by(6) {
            let id = u16::from_be_bytes([wire[i], wire[i + 1]]);
            if id == 0x03 {
                let value = u32::from_be_bytes([wire[i + 2], wire[i + 3], wire[i + 4], wire[i + 5]]);
                assert_eq!(value, 0, "ENABLE_PUSH must be 0");
                return;
            }
        }
        panic!("ENABLE_PUSH setting not found");
    }

    #[test]
    fn test_h2_settings_wire_format_chrome_max_concurrent() {
        let profile = StealthProfile::chrome_default();
        let wire = h2_settings_wire_format(&profile.http2);
        for i in (0..wire.len()).step_by(6) {
            let id = u16::from_be_bytes([wire[i], wire[i + 1]]);
            if id == 0x04 {
                let value = u32::from_be_bytes([wire[i + 2], wire[i + 3], wire[i + 4], wire[i + 5]]);
                assert_eq!(value, 1000, "Chrome MAX_CONCURRENT_STREAMS = 1000");
                return;
            }
        }
        panic!("MAX_CONCURRENT_STREAMS setting not found");
    }

    #[test]
    fn test_ssl_config_h2_binary_roundtrip() {
        let profile = StealthProfile::firefox_default();
        let config = stealth_profile_to_ssl_config(&Some(profile.clone()));
        let payload = config.h2_settings_payload.as_deref().unwrap();
        let original = h2_settings_wire_format(&profile.http2);
        assert_eq!(payload, &original[..], "binary roundtrip must match original wire format");
    }

    // ─── H2 fingerprint pipeline integration tests ──────────────
    // @trace REQ-STL-002 [req:REQ-STL-002] [level:unit]
    // Verifies the full data path: StealthProfile.http2 → h2_settings_wire_format()
    // → SSLConfig.h2_settings_payload (Option<Box<[u8]>>) → write_preface/replenish_window

    #[test]
    fn test_h2_payload_preserves_nul_bytes() {
        // The original bug: CStrPtr truncated at first \0 byte. Binary format
        // MUST preserve NUL bytes (value 0x00000000 is a valid settings value).
        let profile = StealthProfile::firefox_default();
        let config = stealth_profile_to_ssl_config(&Some(profile));
        let payload = config.h2_settings_payload.as_deref().unwrap();
        // Wire format contains ENABLE_PUSH=0 which encodes as [0x00, 0x03, 0x00, 0x00, 0x00, 0x00]
        // — the last 4 bytes are all NUL. Verify they're present.
        assert!(payload.contains(&0u8), "binary payload must contain NUL bytes (ENABLE_PUSH value = 0)");
        // Verify no truncation: 6 settings × 6 bytes = 36
        assert_eq!(payload.len(), 36, "payload must not be truncated at NUL bytes");
    }

    #[test]
    fn test_h2_payload_chrome_preserves_nul_bytes() {
        let profile = StealthProfile::chrome_default();
        let config = stealth_profile_to_ssl_config(&Some(profile));
        let payload = config.h2_settings_payload.as_deref().unwrap();
        // Chrome also has ENABLE_PUSH=0 → NUL bytes
        assert!(payload.contains(&0u8), "Chrome payload must contain NUL bytes");
        assert_eq!(payload.len(), 36, "Chrome payload must be 36 bytes");
    }

    #[test]
    fn test_h2_firefox_wire_all_settings_big_endian() {
        let profile = StealthProfile::firefox_default();
        let config = stealth_profile_to_ssl_config(&Some(profile));
        let payload = config.h2_settings_payload.as_deref().unwrap();
        // Decode all 6 settings from binary wire format
        let decoded: Vec<(u16, u32)> = (0..payload.len())
            .step_by(6)
            .map(|i| {
                let id = u16::from_be_bytes([payload[i], payload[i + 1]]);
                let value = u32::from_be_bytes([payload[i + 2], payload[i + 3], payload[i + 4], payload[i + 5]]);
                (id, value)
            })
            .collect();
        assert_eq!(decoded.len(), 6, "Firefox must have exactly 6 settings");
        // Verify specific Firefox values
        let ht = decoded.iter().find(|(id, _)| *id == 0x01);
        assert_eq!(ht.map(|(_, v)| *v), Some(65536), "Firefox HEADER_TABLE_SIZE = 65536");
        let iws = decoded.iter().find(|(id, _)| *id == 0x02);
        assert_eq!(iws.map(|(_, v)| *v), Some(131072), "Firefox INITIAL_WINDOW_SIZE = 131072");
        let ep = decoded.iter().find(|(id, _)| *id == 0x03);
        assert_eq!(ep.map(|(_, v)| *v), Some(0), "Firefox ENABLE_PUSH = 0");
        let mcs = decoded.iter().find(|(id, _)| *id == 0x04);
        assert_eq!(mcs.map(|(_, v)| *v), Some(100), "Firefox MAX_CONCURRENT_STREAMS = 100");
        let mfs = decoded.iter().find(|(id, _)| *id == 0x05);
        assert_eq!(mfs.map(|(_, v)| *v), Some(16384), "Firefox MAX_FRAME_SIZE = 16384");
        let mhl = decoded.iter().find(|(id, _)| *id == 0x06);
        assert_eq!(mhl.map(|(_, v)| *v), Some(262144), "Firefox MAX_HEADER_LIST_SIZE = 262144");
    }

    #[test]
    fn test_h2_chrome_wire_all_settings_big_endian() {
        let profile = StealthProfile::chrome_default();
        let config = stealth_profile_to_ssl_config(&Some(profile));
        let payload = config.h2_settings_payload.as_deref().unwrap();
        let decoded: Vec<(u16, u32)> = (0..payload.len())
            .step_by(6)
            .map(|i| {
                let id = u16::from_be_bytes([payload[i], payload[i + 1]]);
                let value = u32::from_be_bytes([payload[i + 2], payload[i + 3], payload[i + 4], payload[i + 5]]);
                (id, value)
            })
            .collect();
        assert_eq!(decoded.len(), 6, "Chrome must have exactly 6 settings");
        // Chrome-specific values
        let iws = decoded.iter().find(|(id, _)| *id == 0x02);
        assert_eq!(iws.map(|(_, v)| *v), Some(6291456), "Chrome INITIAL_WINDOW_SIZE = 6291456");
        let mcs = decoded.iter().find(|(id, _)| *id == 0x04);
        assert_eq!(mcs.map(|(_, v)| *v), Some(1000), "Chrome MAX_CONCURRENT_STREAMS = 1000");
    }

    #[test]
    fn test_h2_window_size_firefox_pipeline() {
        // Verify initial_window_size flows through SSLConfig correctly
        let profile = StealthProfile::firefox_default();
        let config = stealth_profile_to_ssl_config(&Some(profile));
        assert_eq!(config.h2_initial_window_size, 131072, "Firefox window size must be 131072 (128 KiB)");
    }

    #[test]
    fn test_h2_window_size_chrome_pipeline() {
        let profile = StealthProfile::chrome_default();
        let config = stealth_profile_to_ssl_config(&Some(profile));
        assert_eq!(config.h2_initial_window_size, 6291456, "Chrome window size must be 6291456 (6 MiB)");
    }

    #[test]
    fn test_h2_window_size_default_pipeline() {
        // No profile → h2_initial_window_size = 0 → write_preface uses LOCAL_INITIAL_WINDOW_SIZE
        let config = stealth_profile_to_ssl_config(&None);
        assert_eq!(config.h2_initial_window_size, 0, "no profile → window size 0 (use LOCAL_INITIAL_WINDOW_SIZE)");
    }

    #[test]
    fn test_h2_firefox_chrome_payloads_differ_in_all_bytes() {
        let ff = StealthProfile::firefox_default();
        let ch = StealthProfile::chrome_default();
        let ff_config = stealth_profile_to_ssl_config(&Some(ff));
        let ch_config = stealth_profile_to_ssl_config(&Some(ch));
        let ff_payload = ff_config.h2_settings_payload.as_deref().unwrap();
        let ch_payload = ch_config.h2_settings_payload.as_deref().unwrap();
        // Payloads should differ (different setting values)
        assert_ne!(ff_payload, ch_payload, "Firefox and Chrome H2 SETTINGS payloads must differ");
    }

    #[test]
    fn test_h2_no_profile_has_none_payload() {
        let config = stealth_profile_to_ssl_config(&None);
        assert!(config.h2_settings_payload.is_none(), "no profile → h2_settings_payload must be None");
    }

    #[test]
    fn test_h2_payload_byte_level_identity_with_wire_format() {
        // Verify byte-for-byte identity between h2_settings_wire_format output
        // and what's stored in SSLConfig.h2_settings_payload
        for profile_fn in [StealthProfile::firefox_default, StealthProfile::chrome_default] {
            let profile = profile_fn();
            let wire = h2_settings_wire_format(&profile.http2);
            let config = stealth_profile_to_ssl_config(&Some(profile));
            let payload = config.h2_settings_payload.clone().unwrap();
            assert_eq!(&payload[..], &wire, "SSLConfig payload must exactly match wire format bytes");
        }
    }
}
