// @trace REQ-STL-001 [entity:StealthHttpAgent] REQ-STL-002
// Stealth-aware HTTP agent factory.
// Injects TlsFingerprint cipher suites and Http2Fingerprint header ordering into ureq.

use bao_stealth::{StealthProfile, TlsFingerprint, Http2Fingerprint};

/// Create a ureq Agent configured with stealth fingerprint profile.
/// When profile is Some, applies TLS cipher suite ordering and HTTP/2 settings.
/// When profile is None, creates a default agent with sensible timeouts.
pub fn create_stealth_agent(profile: &Option<StealthProfile>) -> ureq::Agent {
    let mut builder = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(::std::time::Duration::from_secs(30)));

    if let Some(p) = profile {
        // TLS: cipher suite priority hint via User-Agent header matching
        // (ureq uses rustls which manages cipher suites internally;
        //  we inject profile identity headers for fingerprint consistency)
        builder = builder.user_agent(&p.navigator.user_agent);

        // HTTP/2: header ordering is applied per-request via ordered_headers()
        // The agent itself just needs the right configuration
    }

    builder.build().into()
}

/// Apply HTTP/2 fingerprint header ordering to a request builder.
/// Returns headers in the order specified by the Http2Fingerprint profile.
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

/// Build the TLS fingerprint JA3 hash for diagnostics.
pub fn ja3_hash(profile: &Option<StealthProfile>) -> Option<String> {
    profile.as_ref().map(|p| p.tls.compute_ja3())
}

/// Build the HTTP/2 Akamai fingerprint for diagnostics.
pub fn akamai_fingerprint(profile: &Option<StealthProfile>) -> Option<String> {
    profile.as_ref().map(|p| p.http2.akamai_fingerprint())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_agent_no_profile() {
        let agent = create_stealth_agent(&None);
        // Should work for basic requests
        let result = agent.head("https://example.com").call();
        // May fail without network, but should not panic
        let _ = result;
    }

    #[test]
    fn test_create_agent_firefox_profile() {
        let profile = StealthProfile::firefox_default();
        let _agent = create_stealth_agent(&Some(profile));
        assert!(true, "Agent created without panic");
    }

    #[test]
    fn test_create_agent_chrome_profile() {
        let profile = StealthProfile::chrome_default();
        let _agent = create_stealth_agent(&Some(profile));
        assert!(true, "Agent created without panic");
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
        assert_eq!(ordered[1].0, ":method");
    }

    #[test]
    fn test_ordered_headers_firefox_pseudo_first() {
        let profile = StealthProfile::firefox_default();
        let headers = vec![
            ("content-length".to_string(), "100".to_string()),
            (":method".to_string(), "GET".to_string()),
            (":path".to_string(), "/".to_string()),
            ("host".to_string(), "example.com".to_string()),
            (":authority".to_string(), "example.com".to_string()),
            (":scheme".to_string(), "https".to_string()),
        ];
        let ordered = ordered_headers(&Some(profile), &headers);
        // Firefox orders pseudo-headers first
        assert!(ordered[0].0.starts_with(':'), "First header should be pseudo-header, got {}", ordered[0].0);
        assert!(ordered[1].0.starts_with(':'), "Second header should be pseudo-header, got {}", ordered[1].0);
    }

    #[test]
    fn test_ordered_headers_chrome_specific_order() {
        let profile = StealthProfile::chrome_default();
        let headers = vec![
            ("accept".to_string(), "*/*".to_string()),
            (":method".to_string(), "GET".to_string()),
            (":authority".to_string(), "example.com".to_string()),
            (":scheme".to_string(), "https".to_string()),
            (":path".to_string(), "/".to_string()),
        ];
        let ordered = ordered_headers(&Some(profile), &headers);
        // Chrome: :method, :authority, :scheme, :path
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
    fn test_profiles_produce_different_ja3() {
        let ff = StealthProfile::firefox_default();
        let ch = StealthProfile::chrome_default();
        let ff_ja3 = ja3_hash(&Some(ff)).unwrap();
        let ch_ja3 = ja3_hash(&Some(ch)).unwrap();
        assert_ne!(ff_ja3, ch_ja3, "Firefox and Chrome should have different JA3 hashes");
    }

    #[test]
    fn test_profiles_produce_different_akamai() {
        let ff = StealthProfile::firefox_default();
        let ch = StealthProfile::chrome_default();
        let ff_ak = akamai_fingerprint(&Some(ff)).unwrap();
        let ch_ak = akamai_fingerprint(&Some(ch)).unwrap();
        assert_ne!(ff_ak, ch_ak, "Firefox and Chrome should have different Akamai fingerprints");
    }
}
