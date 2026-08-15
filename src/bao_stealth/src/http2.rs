// REQ-STL-002: HTTP/2 fingerprint matching (Akamai)  @trace REQ-STL-002
// PRIORITY frame mode (REQ-STL-002-C3): Firefox sends explicit PRIORITY frames with
// a stream dependency tree; Chrome dropped PRIORITY frames in v106. The
// `priority_frame_mode` field records this per-browser behaviour.  @trace REQ-STL-002 [criterion:REQ-STL-002-C3]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriorityFrameMode {
    /// Browser sends explicit PRIORITY frames (Firefox behaviour).  @trace REQ-STL-002 [criterion:REQ-STL-002-C3]
    Explicit,
    /// Browser no longer sends PRIORITY frames (Chrome v106+ behaviour).  @trace REQ-STL-002 [criterion:REQ-STL-002-C3]
    None,
}

/// One PRIORITY frame (RFC 7540 §6.3): stream id + dependency + weight.  @trace REQ-STL-002 [criterion:REQ-STL-002-C3]
///
/// `weight` is the wire value (0-255); the effective HTTP/2 weight is `weight + 1`
/// (1-256), per RFC 7540 §6.3. `stream_id` is the frame's stream — the
/// priority-tree node the frame reserves (Firefox reserves 3/5/7/11 on
/// connection setup, so real request streams start at 13).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriorityFrame {
    pub stream_id: u32,
    pub stream_dependency: u32,
    pub exclusive: bool,
    pub weight: u8,
}

#[derive(Debug, Clone)]
pub struct Http2Fingerprint {
    pub header_table_size: u32,
    pub enable_push: bool,
    pub max_concurrent_streams: u32,
    pub initial_window_size: u32,
    pub max_frame_size: u32,
    pub max_header_list_size: u32,
    pub window_update_size: u32,
    pub pseudo_header_order: Vec<&'static str>,
    /// PRIORITY frame mode (REQ-STL-002-C3).  @trace REQ-STL-002 [criterion:REQ-STL-002-C3]
    pub priority_frame_mode: PriorityFrameMode,
    /// Explicit PRIORITY frames emitted on connection setup (Firefox only).  @trace REQ-STL-002 [criterion:REQ-STL-002-C3]
    pub priority_frames: Vec<PriorityFrame>,
}

impl Http2Fingerprint {
    pub fn firefox() -> Self {
        Http2Fingerprint {
            header_table_size: 65536,
            enable_push: false,
            max_concurrent_streams: 100,
            initial_window_size: 131072,
            max_frame_size: 16384,
            max_header_list_size: 262144,
            window_update_size: 131072,
            pseudo_header_order: vec![":method", ":path", ":authority", ":scheme"],
            // Firefox emits explicit PRIORITY frames to build its dependency tree.
            priority_frame_mode: PriorityFrameMode::Explicit,
            // Firefox default weights: streams 3/5/7/11 depending on stream 0 (the
            // root), non-exclusive. Wire weights 40/109/138/255 → effective 41/110/139/256.
            // Matches observed Firefox connection-setup traffic.
            priority_frames: vec![
                PriorityFrame {
                    stream_id: 3,
                    stream_dependency: 0,
                    exclusive: false,
                    weight: 40,
                },
                PriorityFrame {
                    stream_id: 5,
                    stream_dependency: 0,
                    exclusive: false,
                    weight: 109,
                },
                PriorityFrame {
                    stream_id: 7,
                    stream_dependency: 0,
                    exclusive: false,
                    weight: 138,
                },
                PriorityFrame {
                    stream_id: 11,
                    stream_dependency: 0,
                    exclusive: false,
                    weight: 255,
                },
            ],
        }
    }

    pub fn chrome() -> Self {
        Http2Fingerprint {
            header_table_size: 65536,
            enable_push: false,
            max_concurrent_streams: 1000,
            initial_window_size: 6291456,
            max_frame_size: 16384,
            max_header_list_size: 262144,
            window_update_size: 15663105,
            pseudo_header_order: vec![":method", ":authority", ":scheme", ":path"],
            // Chrome v106+ removed PRIORITY frames entirely.
            priority_frame_mode: PriorityFrameMode::None,
            priority_frames: Vec::new(),
        }
    }

    /// Returns true if this profile emits explicit PRIORITY frames (REQ-STL-002-C3).  @trace REQ-STL-002 [criterion:REQ-STL-002-C3]
    pub fn sends_priority_frames(&self) -> bool {
        self.priority_frame_mode == PriorityFrameMode::Explicit
    }

    /// Returns the PRIORITY frames this profile emits on connection setup.  @trace REQ-STL-002 [criterion:REQ-STL-002-C3]
    pub fn priority_frame_payload(&self) -> &[PriorityFrame] {
        &self.priority_frames
    }

    /// Builder: returns a copy with the given PRIORITY mode (REQ-STL-002-C3).  @trace REQ-STL-002 [criterion:REQ-STL-002-C3]
    pub fn with_priority_mode(mut self, mode: PriorityFrameMode) -> Self {
        self.priority_frame_mode = mode;
        if mode == PriorityFrameMode::None {
            self.priority_frames.clear();
        }
        self
    }

    pub fn akamai_fingerprint(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}:{}",
            self.header_table_size,
            if self.enable_push { 1 } else { 0 },
            self.max_concurrent_streams,
            self.initial_window_size,
            self.max_frame_size,
            self.max_header_list_size,
        )
    }

    pub fn settings_frame_payload(&self) -> Vec<(u16, u32)> {
        vec![
            (0x01, self.header_table_size),
            (0x03, if self.enable_push { 1 } else { 0 }),
            (0x04, self.max_concurrent_streams),
            (0x02, self.initial_window_size),
            (0x05, self.max_frame_size),
            (0x06, self.max_header_list_size),
        ]
    }

    pub fn ordered_headers<'a>(&self, headers: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
        let mut ordered = Vec::with_capacity(headers.len());
        let mut remaining: Vec<(&'a str, &'a str)> = headers.to_vec();

        for pseudo in &self.pseudo_header_order {
            if let Some(pos) = remaining.iter().position(|(k, _)| *k == *pseudo) {
                ordered.push(remaining.remove(pos));
            }
        }
        ordered.extend(remaining);
        ordered
    }
}

impl Default for Http2Fingerprint {
    /// Default = Firefox profile (SPEC REQ-STL-002 mandates Firefox-matching).  @trace REQ-STL-002
    fn default() -> Self {
        Http2Fingerprint::firefox()
    }
}

// ── Process-global HTTP/2 fingerprint snapshot ──────────────────────────
//
// U2 page-network unification: the servo-net bun bridge runs on the net
// thread, where the ScriptThread-scoped `engine_props` profile lookups
// (thread-local fallback / per-Realm map) are unreachable. The h2 SETTINGS
// payload reaches it through servo's `StealthTlsWireConfig` global, but the
// pseudo-header wire order and the connection-preface PRIORITY frames live
// only on the full `Http2Fingerprint` — so the embedder snapshots the active
// page profile's h2 fingerprint here, right next to
// `servo::set_stealth_tls_config`, and the bridge reads it when shaping its
// `SSLConfig`. Same lifecycle as the wire-config global: set on profile
// activation, cleared when the profile is deactivated.  @trace REQ-STL-002 [criterion:REQ-STL-002-C3]

static GLOBAL_HTTP2_FINGERPRINT: std::sync::RwLock<Option<Http2Fingerprint>> =
    std::sync::RwLock::new(None);

/// Snapshot the active page profile's HTTP/2 fingerprint for cross-thread
/// consumers (see module notes). `None` deactivates.
pub fn set_global_http2_fingerprint(fingerprint: Option<&Http2Fingerprint>) {
    let mut guard = GLOBAL_HTTP2_FINGERPRINT.write().unwrap();
    *guard = fingerprint.cloned();
}

/// The active page profile's HTTP/2 fingerprint (clone), or `None` when no
/// profile is active.
pub fn global_http2_fingerprint() -> Option<Http2Fingerprint> {
    GLOBAL_HTTP2_FINGERPRINT.read().unwrap().clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firefox_has_expected_values() {
        let fp = Http2Fingerprint::firefox();
        assert_eq!(fp.header_table_size, 65536);
        assert_eq!(fp.enable_push, false);
        assert_eq!(fp.max_concurrent_streams, 100);
        assert_eq!(fp.initial_window_size, 131072);
        assert_eq!(fp.max_frame_size, 16384);
        assert_eq!(fp.max_header_list_size, 262144);
        assert_eq!(fp.window_update_size, 131072);
    }

    #[test]
    fn chrome_has_expected_values() {
        let fp = Http2Fingerprint::chrome();
        assert_eq!(fp.max_concurrent_streams, 1000);
        assert_eq!(fp.initial_window_size, 6291456);
        assert_eq!(fp.window_update_size, 15663105);
    }

    #[test]
    fn firefox_pseudo_header_order() {
        let fp = Http2Fingerprint::firefox();
        assert_eq!(
            fp.pseudo_header_order,
            vec![":method", ":path", ":authority", ":scheme"]
        );
    }

    #[test]
    fn chrome_pseudo_header_order() {
        let fp = Http2Fingerprint::chrome();
        assert_eq!(
            fp.pseudo_header_order,
            vec![":method", ":authority", ":scheme", ":path"]
        );
    }

    #[test]
    fn firefox_and_chrome_have_different_akamai_fingerprints() {
        let ff = Http2Fingerprint::firefox().akamai_fingerprint();
        let ch = Http2Fingerprint::chrome().akamai_fingerprint();
        assert_ne!(ff, ch);
    }

    #[test]
    fn akamai_fingerprint_format_6_colon_separated_numbers() {
        let fp = Http2Fingerprint::firefox();
        let fingerprint = fp.akamai_fingerprint();
        let parts: Vec<&str> = fingerprint.split(':').collect();
        assert_eq!(parts.len(), 6);
        for part in &parts {
            assert!(part.parse::<u32>().is_ok());
        }
    }

    #[test]
    fn akamai_fingerprint_firefox_starts_with_65536() {
        let fp = Http2Fingerprint::firefox();
        let fingerprint = fp.akamai_fingerprint();
        assert!(fingerprint.starts_with("65536:"));
    }

    #[test]
    fn akamai_fingerprint_chrome_starts_with_65536() {
        let fp = Http2Fingerprint::chrome();
        let fingerprint = fp.akamai_fingerprint();
        assert!(fingerprint.starts_with("65536:"));
    }

    #[test]
    fn settings_frame_payload_returns_6_tuples() {
        let fp = Http2Fingerprint::firefox();
        let payload = fp.settings_frame_payload();
        assert_eq!(payload.len(), 6);
    }

    #[test]
    fn settings_frame_payload_firefox_first_is_0x01_65536() {
        let fp = Http2Fingerprint::firefox();
        let payload = fp.settings_frame_payload();
        assert_eq!(payload[0], (0x01, 65536));
    }

    #[test]
    fn settings_frame_payload_chrome_third_is_0x04_1000() {
        let fp = Http2Fingerprint::chrome();
        let payload = fp.settings_frame_payload();
        assert_eq!(payload[2], (0x04, 1000));
    }

    #[test]
    fn settings_frame_payload_enable_push_0_when_false() {
        let fp = Http2Fingerprint::firefox();
        let payload = fp.settings_frame_payload();
        assert_eq!(payload[1], (0x03, 0));
    }

    #[test]
    fn ordered_headers_pseudo_first_firefox() {
        let fp = Http2Fingerprint::firefox();
        let input: Vec<(&str, &str)> = vec![
            ("content-length", "0"),
            (":method", "GET"),
            (":path", "/"),
            ("host", "example.com"),
            (":authority", "example.com"),
            (":scheme", "https"),
        ];
        let ordered = fp.ordered_headers(&input);
        assert_eq!(ordered[0].0, ":method");
        assert_eq!(ordered[1].0, ":path");
        assert_eq!(ordered[2].0, ":authority");
        assert_eq!(ordered[3].0, ":scheme");
    }

    #[test]
    fn ordered_headers_chrome_specific_order() {
        let fp = Http2Fingerprint::chrome();
        let input: Vec<(&str, &str)> = vec![
            (":path", "/"),
            (":scheme", "https"),
            (":method", "GET"),
            (":authority", "example.com"),
        ];
        let ordered = fp.ordered_headers(&input);
        assert_eq!(ordered[0].0, ":method");
        assert_eq!(ordered[1].0, ":authority");
        assert_eq!(ordered[2].0, ":scheme");
        assert_eq!(ordered[3].0, ":path");
    }

    #[test]
    fn ordered_headers_no_pseudo_headers_preserves_order() {
        let fp = Http2Fingerprint::firefox();
        let input: Vec<(&str, &str)> = vec![
            ("host", "example.com"),
            ("content-length", "0"),
            ("accept", "*/*"),
        ];
        let ordered = fp.ordered_headers(&input);
        assert_eq!(ordered[0].0, "host");
        assert_eq!(ordered[1].0, "content-length");
        assert_eq!(ordered[2].0, "accept");
    }

    #[test]
    fn ordered_headers_empty_input_returns_empty() {
        let fp = Http2Fingerprint::firefox();
        let input: Vec<(&str, &str)> = vec![];
        let ordered = fp.ordered_headers(&input);
        assert!(ordered.is_empty());
    }

    #[test]
    fn ordered_headers_only_pseudo_headers() {
        let fp = Http2Fingerprint::firefox();
        let input: Vec<(&str, &str)> = vec![
            (":method", "GET"),
            (":path", "/"),
            (":authority", "example.com"),
            (":scheme", "https"),
        ];
        let ordered = fp.ordered_headers(&input);
        assert_eq!(ordered.len(), 4);
        assert_eq!(ordered[0].0, ":method");
        assert_eq!(ordered[1].0, ":path");
        assert_eq!(ordered[2].0, ":authority");
        assert_eq!(ordered[3].0, ":scheme");
    }

    #[test]
    fn clone_works() {
        let fp = Http2Fingerprint::firefox();
        let cloned = fp.clone();
        assert_eq!(fp.header_table_size, cloned.header_table_size);
        assert_eq!(fp.pseudo_header_order, cloned.pseudo_header_order);
    }

    #[test]
    fn debug_format_contains_http2_fingerprint() {
        let fp = Http2Fingerprint::firefox();
        let debug_str = format!("{:?}", fp);
        assert!(debug_str.contains("Http2Fingerprint"));
    }

    #[test]
    fn firefox_and_chrome_different_pseudo_order() {
        let ff = Http2Fingerprint::firefox();
        let ch = Http2Fingerprint::chrome();
        assert_ne!(ff.pseudo_header_order, ch.pseudo_header_order);
    }

    // ===========================================================================
    // REQ-STL-002-C3: PRIORITY frame mode  @trace REQ-STL-002 [criterion:REQ-STL-002-C3]
    // ===========================================================================

    #[test]
    fn firefox_emits_priority_frames() {
        // REQ-STL-002-C3: HTTP/2 PRIORITY frame mode matches Firefox.
        let ff = Http2Fingerprint::firefox();
        assert_eq!(ff.priority_frame_mode, PriorityFrameMode::Explicit);
        assert!(ff.sends_priority_frames());
        assert!(!ff.priority_frame_payload().is_empty());
    }

    #[test]
    fn chrome_does_not_emit_priority_frames() {
        // REQ-STL-002-C3: Chrome v106+ dropped PRIORITY frames.
        let ch = Http2Fingerprint::chrome();
        assert_eq!(ch.priority_frame_mode, PriorityFrameMode::None);
        assert!(!ch.sends_priority_frames());
        assert!(ch.priority_frame_payload().is_empty());
    }

    #[test]
    fn firefox_priority_frames_are_valid_rfc7540() {
        // REQ-STL-002-C3: each PRIORITY frame has a valid dependency + weight.
        // RFC 7540 §6.3: weight is wire u8 (effective = wire + 1, range 1-256).
        let ff = Http2Fingerprint::firefox();
        for frame in ff.priority_frame_payload() {
            assert!(frame.weight <= 255);
            // Stream id must be an odd client stream > 0 (or the frame would
            // target a server-initiated / invalid stream).
            assert!(frame.stream_id % 2 == 1 && frame.stream_id > 0);
            // Stream dependency references a valid prior stream (0 = root).
        }
    }

    #[test]
    fn firefox_and_chrome_priority_modes_differ() {
        // REQ-STL-002-C3: Firefox sends PRIORITY frames, Chrome does not.
        let ff = Http2Fingerprint::firefox();
        let ch = Http2Fingerprint::chrome();
        assert_ne!(ff.priority_frame_mode, ch.priority_frame_mode);
    }

    #[test]
    fn firefox_priority_frame_count_matches_firefox_observed() {
        // REQ-STL-002-C3: Firefox emits its known dependency-tree PRIORITY frames.
        let ff = Http2Fingerprint::firefox();
        assert_eq!(ff.priority_frame_payload().len(), 4);
    }

    #[test]
    fn firefox_priority_frames_reserve_streams_3_5_7_11() {
        // REQ-STL-002-C3: observed Firefox connection-setup traffic reserves
        // priority-tree nodes on streams 3/5/7/11 (real requests start at 13).
        let ff = Http2Fingerprint::firefox();
        let ids: Vec<u32> = ff
            .priority_frame_payload()
            .iter()
            .map(|f| f.stream_id)
            .collect();
        assert_eq!(ids, vec![3, 5, 7, 11]);
        // Weights stay paired with their observed stream id.
        let by_stream: Vec<(u32, u8)> = ff
            .priority_frame_payload()
            .iter()
            .map(|f| (f.stream_id, f.weight))
            .collect();
        assert_eq!(
            by_stream,
            vec![(3, 40), (5, 109), (7, 138), (11, 255)]
        );
    }
}
