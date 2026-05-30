// REQ-STL-002: HTTP/2 fingerprint matching (Akamai)  @trace REQ-STL-002
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
        }
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
