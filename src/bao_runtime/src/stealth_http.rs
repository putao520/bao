// @trace REQ-STL-001 [entity:StealthHttpAgent] REQ-STL-002
// Stealth-aware HTTP request configuration.
// Applies HTTP/2 header ordering and User-Agent injection from StealthProfile.
// Actual HTTP execution is delegated to crate::http_client::http_request().

use bao_stealth::{Http2Fingerprint, StealthProfile, TlsFingerprint};
use bun_http::Method;

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

fn cipher_list_string(fp: &TlsFingerprint) -> String {
    fp.cipher_suites.iter()
        .filter_map(|&id| tls_cipher_name(id))
        .collect::<Vec<&str>>()
        .join(":")
}

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
}
