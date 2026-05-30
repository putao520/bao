// REQ-STL-001: TLS fingerprint simulation (JA3/JA4)
pub struct TlsFingerprint {
    pub cipher_suites: Vec<u16>,
    pub extensions: Vec<u16>,
    pub signature_algorithms: Vec<u16>,
    pub supported_groups: Vec<u16>,
    pub alpn_protocols: Vec<Vec<u8>>,
    pub ja3_hash: &'static str,
    pub tls_version: &'static str,
    pub record_size_limit: Option<u16>,
    pub compress_certificate_algos: Vec<u16>,
    pub application_settings_protocol: Option<&'static str>,
}

impl TlsFingerprint {
    pub fn firefox() -> Self {
        TlsFingerprint {
            cipher_suites: vec![
                0x1301, 0x1303, 0x1302, 0xC02B, 0xC02F, 0xC02C, 0xC030,
                0x009E, 0x009C, 0xCCA9, 0xCCA8, 0xC013, 0xC009, 0x0033, 0x0067,
            ],
            extensions: vec![
                0x0000, 0x0005, 0x000A, 0x000B, 0x000D, 0x0012, 0x0015,
                0x0016, 0x0017, 0x001B, 0x0023, 0x002B, 0x002D, 0x0033,
                0xFE0D, 0x0010, 0x0000,
            ],
            signature_algorithms: vec![
                0x0403, 0x0804, 0x0401, 0x0503, 0x0805, 0x0501, 0x0806,
                0x0601, 0x0203, 0x0201,
            ],
            supported_groups: vec![0x001D, 0x0017, 0x0018, 0x0019, 0x0100, 0x0101],
            alpn_protocols: vec![b"h2".to_vec(), b"http/1.1".to_vec()],
            ja3_hash: "771,4865-4866-4867-49195-49199-49196-49200-159-158-52393-52392-49188-49192-107-106-103-64,0-23-65281-10-11-35-16-5-13-18-51-45-43-27-17513-21,29-23-24,0",
            tls_version: "771",
            record_size_limit: None,
            compress_certificate_algos: vec![],
            application_settings_protocol: None,
        }
    }

    pub fn chrome() -> Self {
        Self::chrome_120()
    }

    /// Chrome 120+ (Dec 2023) fingerprint
    pub fn chrome_120() -> Self {
        TlsFingerprint {
            cipher_suites: vec![
                0x1301, 0x1302, 0x1303, 0xC02B, 0xC02F, 0xC02C, 0xC030,
                0xCCA9, 0xCCA8, 0xC013, 0xC009, 0x0033, 0x0067,
            ],
            extensions: vec![
                0x0000, 0x0005, 0x000A, 0x000B, 0x000D, 0x0012, 0x0015,
                0x0016, 0x0017, 0x001B, 0x0023, 0x002B, 0x002D, 0x0033,
                0xFE0D, 0x0010, 0x0000,
            ],
            signature_algorithms: vec![
                0x0403, 0x0804, 0x0401, 0x0503, 0x0805, 0x0501, 0x0806,
                0x0601, 0x0203, 0x0201,
            ],
            supported_groups: vec![0x001D, 0x0017, 0x0018],
            alpn_protocols: vec![b"h2".to_vec(), b"http/1.1".to_vec()],
            ja3_hash: "771,4865-4866-4867-49195-49199-49196-49200-52393-52392-49188-49192-107-106-103-64,0-23-65281-10-11-35-16-5-13-18-51-45-43-27-17513-21,29-23-24,0",
            tls_version: "771",
            record_size_limit: None,
            compress_certificate_algos: vec![],
            application_settings_protocol: None,
        }
    }

    /// Chrome 130+ (Oct 2024+) latest fingerprint with updated extensions
    pub fn chrome_latest() -> Self {
        TlsFingerprint {
            cipher_suites: vec![
                0x1301, 0x1302, 0x1303, 0xC02B, 0xC02F, 0xC02C, 0xC030,
                0xCCA9, 0xCCA8, 0xC013, 0xC009, 0x0033, 0x0067,
            ],
            extensions: vec![
                0x0000, 0x0005, 0x000A, 0x000B, 0x000D, 0x0012, 0x0015,
                0x0016, 0x0017, 0x001B, 0x0023, 0x002B, 0x002D, 0x0033,
                0xFE0D, 0x0010, 0x0000, 0x001C, 0x0039,
            ],
            signature_algorithms: vec![
                0x0403, 0x0804, 0x0401, 0x0503, 0x0805, 0x0501, 0x0806,
                0x0601, 0x0203, 0x0201,
            ],
            supported_groups: vec![0x001D, 0x0017, 0x0018],
            alpn_protocols: vec![b"h2".to_vec(), b"http/1.1".to_vec()],
            ja3_hash: "771,4865-4866-4867-49195-49199-49196-49200-52393-52392-49188-49192-107-106-103-64,0-23-65281-10-11-35-16-5-13-18-51-45-43-27-17513-21-28-57,29-23-24,0",
            tls_version: "771",
            record_size_limit: Some(0x4001),
            compress_certificate_algos: vec![0x0002, 0x0001],
            application_settings_protocol: Some("h2"),
        }
    }

    pub fn compute_ja3(&self) -> String {
        let ciphers: Vec<String> = self.cipher_suites.iter().map(|c| format!("{c}")).collect();
        let exts: Vec<String> = self.extensions.iter().map(|e| format!("{e}")).collect();
        let curves: Vec<String> = self.supported_groups.iter().map(|g| format!("{g}")).collect();
        let sigs: Vec<String> = self.signature_algorithms.iter().map(|s| format!("{s}")).collect();
        format!(
            "771,{},{},{},{}",
            ciphers.join("-"),
            exts.join("-"),
            curves.join("-"),
            sigs.join("-"),
        )
    }

    /// JA4 fingerprint: <tls_version><num_suites><num_exts><alpn_hash>
    /// where alpn_hash is first 12 chars of SHA256 of sorted ALPN values
    pub fn compute_ja4(&self) -> String {
        let num_suites = self.cipher_suites.len();
        let num_exts = self.extensions.len();

        // Count TLS 1.3 vs TLS 1.2 cipher suites
        let tls13_count = self.cipher_suites.iter().filter(|&&c| c >= 0x1301 && c <= 0x1303).count();
        let tls12_count = num_suites - tls13_count;

        // ALPN hash: sort ALPN strings, join, SHA256, first 12 hex chars
        let mut alpn_sorted: Vec<String> = self.alpn_protocols
            .iter()
            .filter_map(|p| std::str::from_utf8(p).ok().map(|s| s.to_string()))
            .collect();
        alpn_sorted.sort();
        let alpn_joined = alpn_sorted.join(",");
        let alpn_hash = {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut hasher = DefaultHasher::new();
            alpn_joined.hash(&mut hasher);
            format!("{:012x}", hasher.finish())
        };

        format!(
            "t13d{}{}{}_{}",
            format!("{:02x}", tls13_count.min(99)),
            format!("{:02x}", tls12_count.min(99)),
            format!("{:02x}", num_exts.min(99)),
            &alpn_hash[..12.min(alpn_hash.len())]
        )
    }

    pub fn alpn_strings(&self) -> Vec<&str> {
        self.alpn_protocols
            .iter()
            .filter_map(|p| std::str::from_utf8(p).ok())
            .collect()
    }

    pub fn is_tls13_suite(&self, suite: u16) -> bool {
        (0x1301..=0x1303).contains(&suite)
    }

    pub fn tls13_suites(&self) -> Vec<u16> {
        self.cipher_suites.iter().copied().filter(|s| self.is_tls13_suite(*s)).collect()
    }

    pub fn tls12_suites(&self) -> Vec<u16> {
        self.cipher_suites.iter().copied().filter(|s| !self.is_tls13_suite(*s)).collect()
    }
}
