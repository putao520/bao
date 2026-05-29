// REQ-STL-001: TLS fingerprint simulation (JA3/JA4)
pub struct TlsFingerprint {
    pub cipher_suites: Vec<u16>,
    pub extensions: Vec<u16>,
    pub signature_algorithms: Vec<u16>,
    pub supported_groups: Vec<u16>,
    pub alpn_protocols: Vec<Vec<u8>>,
    pub ja3_hash: &'static str,
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
        }
    }

    pub fn chrome() -> Self {
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

    pub fn alpn_strings(&self) -> Vec<&str> {
        self.alpn_protocols
            .iter()
            .filter_map(|p| std::str::from_utf8(p).ok())
            .collect()
    }
}
