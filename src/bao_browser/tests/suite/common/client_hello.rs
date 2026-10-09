// TLS ClientHello wire-capture stack (e158b M3 consolidation of the e152
// audit §一 DUP-TEST-FIXTURES stealth-capture family): the per-file inline
// copies that lived in sw_stealth_profile_tests / stealth_per_page_wire_tests
// / page_net_bun_fingerprint_e2e_tests.
//
// Record layer → handshake → body decoding, the ClientHello struct with the
// JA3/canonicalization accessors, and the never-replying CaptureServer that
// records each handshake attempt. Per-file accessor supersets are unified
// here: `canonical_bytes` (sw + page_net), `curves_field` (per_page),
// `first_parsed`/`stop` (per_page), `wait_for` (page_net), `count`/`parsed`
// (sw + per_page/page_net). Bodies are verbatim lifts of the inline copies;
// the per-file diff audit (e158b batch 1) recorded only doc-comment /
// derive / accessor-set divergence, zero behavioral divergence.

#![allow(dead_code)]

use std::io::Read;
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct ClientHello {
    pub legacy_version: u16,
    pub random: [u8; 32],
    pub session_id: Vec<u8>,
    pub cipher_suites: Vec<u8>,
    pub compression: Vec<u8>,
    pub extensions: Vec<(u16, Vec<u8>)>,
}

pub fn be16(bytes: &[u8]) -> u16 {
    u16::from_be_bytes([bytes[0], bytes[1]])
}

/// Read one full ClientHello (handshake body bytes) off a TCP stream. The
/// handshake may span several TLS records; reads are deadline-bounded.
pub fn read_client_hello(stream: &mut TcpStream) -> Result<Vec<u8>, String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut raw: Vec<u8> = Vec::with_capacity(1024);
    let mut handshake: Vec<u8> = Vec::with_capacity(512);
    let mut handshake_need: Option<usize> = None;
    let mut tmp = [0u8; 4096];
    loop {
        while raw.len() >= 5 {
            let record_len = be16(&raw[3..5]) as usize;
            if raw.len() < 5 + record_len {
                break;
            }
            if raw[0] != 0x16 {
                return Err(format!("non-handshake record type 0x{:02x}", raw[0]));
            }
            let payload: Vec<u8> = raw[5..5 + record_len].to_vec();
            raw.drain(..5 + record_len);
            if handshake_need.is_none() {
                if payload.len() < 4 || payload[0] != 0x01 {
                    return Err("first handshake message is not a ClientHello".into());
                }
                let length = (payload[1] as usize) << 16 | (payload[2] as usize) << 8 |
                    payload[3] as usize;
                handshake_need = Some(4 + length);
            }
            handshake.extend_from_slice(&payload);
            if let Some(need) = handshake_need {
                if handshake.len() >= need {
                    return Ok(handshake[4..need].to_vec());
                }
            }
        }
        if Instant::now() > deadline {
            return Err("timeout waiting for a full ClientHello".into());
        }
        match stream.read(&mut tmp) {
            Ok(0) => return Err("connection closed before a full ClientHello".into()),
            Ok(n) => raw.extend_from_slice(&tmp[..n]),
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock ||
                    e.kind() == std::io::ErrorKind::TimedOut =>
            {
                std::thread::sleep(Duration::from_millis(2));
            },
            Err(e) => return Err(format!("socket read error: {e}")),
        }
    }
}

pub fn parse_client_hello(body: &[u8]) -> Result<ClientHello, String> {
    let mut pos = 0usize;
    fn take<'a>(
        bytes: &'a [u8],
        pos: &mut usize,
        len: usize,
        what: &str,
    ) -> Result<&'a [u8], String> {
        if bytes.len() < *pos + len {
            return Err(format!("truncated ClientHello at {what}"));
        }
        let slice = &bytes[*pos..*pos + len];
        *pos += len;
        Ok(slice)
    }

    let legacy_version = be16(take(body, &mut pos, 2, "legacy_version")?);
    let random: [u8; 32] = take(body, &mut pos, 32, "random")?
        .try_into()
        .expect("32 bytes");
    let session_id_len = take(body, &mut pos, 1, "session_id length")?[0] as usize;
    let session_id = take(body, &mut pos, session_id_len, "session_id")?.to_vec();
    let cipher_len = be16(take(body, &mut pos, 2, "cipher_suites length")?) as usize;
    if cipher_len % 2 != 0 {
        return Err("odd cipher_suites length".into());
    }
    let cipher_suites = take(body, &mut pos, cipher_len, "cipher_suites")?.to_vec();
    let compression_len = take(body, &mut pos, 1, "compression length")?[0] as usize;
    let compression = take(body, &mut pos, compression_len, "compression")?.to_vec();

    let mut extensions = Vec::new();
    if pos < body.len() {
        let extensions_total = be16(take(body, &mut pos, 2, "extensions length")?) as usize;
        let extensions_end = pos + extensions_total;
        if extensions_end > body.len() {
            return Err("truncated extensions block".into());
        }
        while pos < extensions_end {
            let extension_type = be16(take(body, &mut pos, 2, "extension type")?);
            let extension_len = be16(take(body, &mut pos, 2, "extension length")?) as usize;
            let payload = take(body, &mut pos, extension_len, "extension body")?.to_vec();
            extensions.push((extension_type, payload));
        }
        if pos != extensions_end {
            return Err("extension lengths do not add up".into());
        }
    }

    Ok(ClientHello {
        legacy_version,
        random,
        session_id,
        cipher_suites,
        compression,
        extensions,
    })
}

impl ClientHello {
    pub fn extension(&self, extension_type: u16) -> Option<&[u8]> {
        self.extensions
            .iter()
            .find(|(t, _)| *t == extension_type)
            .map(|(_, payload)| payload.as_slice())
    }

    /// ALPN protocol list (extension 16), decoded. The payload is a
    /// `ProtocolNameList`: 2-byte total length, then (length-prefixed)
    /// protocol names in offer order.
    pub fn alpn_protocols(&self) -> Vec<Vec<u8>> {
        let Some(wire) = self.extension(0x0010) else {
            return Vec::new();
        };
        if wire.len() < 2 {
            return Vec::new();
        }
        let list_length = be16(wire) as usize;
        let body = &wire[2..];
        let body = &body[..body.len().min(list_length)];
        let mut protocols = Vec::new();
        let mut offset = 0usize;
        while offset < body.len() {
            let len = body[offset] as usize;
            offset += 1;
            if offset + len > body.len() {
                break;
            }
            protocols.push(body[offset..offset + len].to_vec());
            offset += len;
        }
        protocols
    }

    /// JA3 string in the repo convention (`bao_stealth::TlsFingerprint::
    /// compute_ja3`): `771,ciphers,extensions,curves,sigalgs`, wire order.
    pub fn ja3_string(&self) -> String {
        let ciphers: Vec<String> = self
            .cipher_suites
            .chunks_exact(2)
            .map(be16)
            .map(|id| id.to_string())
            .collect();
        let extensions: Vec<String> = self
            .extensions
            .iter()
            .map(|(t, _)| t.to_string())
            .collect();
        let u16_list = |extension_type: u16| -> Vec<String> {
            self.extension(extension_type)
                .map(|payload| {
                    payload
                        .chunks_exact(2)
                        .map(be16)
                        .map(|id| id.to_string())
                        .collect()
                })
                .unwrap_or_default()
        };
        format!(
            "771,{},{},{},{}",
            ciphers.join("-"),
            extensions.join("-"),
            u16_list(0x000a).join("-"), // supported_groups
            u16_list(0x000d).join("-"), // signature_algorithms
        )
    }

    /// The JA3 supported-groups field (`8-29-23-24-25` vs `6-29-23-24`) — the
    /// per-profile identity anchor for these tests (Firefox keeps P-521).
    pub fn curves_field(&self) -> String {
        self.ja3_string().split(',').nth(3).unwrap_or("").to_string()
    }

    /// Canonical byte form: the full ClientHello re-serialized with the
    /// per-connection random fields zeroed but length-preserved:
    /// client_random, legacy_session_id, the key_share (ext 51) ephemeral
    /// public key, and the pre_shared_key identities (ext 41) when present.
    /// Two ClientHellos from the same client configuration compare equal
    /// iff every fingerprint-relevant byte (cipher order, curves, sigalgs,
    /// extension list/order/payloads — including ALPN contents) is
    /// identical.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(256);
        out.extend_from_slice(&self.legacy_version.to_be_bytes());
        out.extend_from_slice(&[0u8; 32]);
        out.push(self.session_id.len() as u8);
        out.resize(out.len() + self.session_id.len(), 0);
        out.extend_from_slice(&(self.cipher_suites.len() as u16).to_be_bytes());
        out.extend_from_slice(&self.cipher_suites);
        out.push(self.compression.len() as u8);
        out.extend_from_slice(&self.compression);
        if !self.extensions.is_empty() {
            let total: usize = self.extensions.iter().map(|(_, p)| 4 + p.len()).sum();
            out.extend_from_slice(&(total as u16).to_be_bytes());
            for (extension_type, payload) in &self.extensions {
                out.extend_from_slice(&extension_type.to_be_bytes());
                out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
                match *extension_type {
                    0x0029 | 0x0033 => out.resize(out.len() + payload.len(), 0),
                    _ => out.extend_from_slice(payload),
                }
            }
        }
        out
    }
}

/// Accepts TLS connections, records each ClientHello, never replies (the
/// ClientHello is already captured when the handshake fails).
pub struct CaptureServer {
    pub port: u16,
    shutdown: Arc<AtomicBool>,
    hellos: Arc<Mutex<Vec<Result<ClientHello, String>>>>,
    signal: Arc<(Mutex<usize>, Condvar)>,
}

impl CaptureServer {
    pub fn spawn() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind capture server");
        let port = listener.local_addr().unwrap().port();
        let _ = listener.set_nonblocking(true);
        let shutdown = Arc::new(AtomicBool::new(false));
        let hellos: Arc<Mutex<Vec<Result<ClientHello, String>>>> =
            Arc::new(Mutex::new(Vec::new()));
        let signal = Arc::new((Mutex::new(0usize), Condvar::new()));
        let shutdown_c = Arc::clone(&shutdown);
        let hellos_c = Arc::clone(&hellos);
        let signal_c = Arc::clone(&signal);
        std::thread::Builder::new()
            .name("tls-capture-fixture".into())
            .spawn(move || {
                while !shutdown_c.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((mut tcp, _)) => {
                            let _ = tcp.set_nonblocking(false);
                            let _ = tcp.set_read_timeout(Some(Duration::from_millis(200)));
                            let captured = read_client_hello(&mut tcp)
                                .and_then(|body| parse_client_hello(&body));
                            hellos_c.lock().unwrap().push(captured);
                            let (count, cond) = &*signal_c;
                            let mut guard = count.lock().unwrap();
                            *guard += 1;
                            cond.notify_all();
                            // No TLS reply: the client handshake fails, which
                            // is fine — the ClientHello is already captured.
                            let _ = tcp.shutdown(std::net::Shutdown::Both);
                        },
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        },
                        Err(_) => return,
                    }
                }
            })
            .expect("spawn tls-capture-fixture");
        CaptureServer {
            port,
            shutdown,
            hellos,
            signal,
        }
    }

    pub fn count(&self) -> usize {
        *self.signal.0.lock().unwrap()
    }

    pub fn first_parsed(&self) -> Option<ClientHello> {
        self.hellos
            .lock()
            .unwrap()
            .iter()
            .find_map(|r| r.clone().ok())
    }

    /// Block until at least `n` ClientHellos arrived (or timeout).
    pub fn wait_for(&self, n: usize, timeout: Duration) -> bool {
        let (count, cond) = &*self.signal;
        let guard = count.lock().unwrap();
        let (guard, timed_out) = cond
            .wait_timeout_while(guard, timeout, |c| *c < n)
            .expect("capture condvar poisoned");
        !timed_out.timed_out() && *guard >= n
    }

    pub fn parsed(&self) -> Vec<ClientHello> {
        self.hellos
            .lock()
            .unwrap()
            .iter()
            .filter_map(|r| r.clone().ok())
            .collect()
    }

    pub fn errors(&self) -> Vec<String> {
        self.hellos
            .lock()
            .unwrap()
            .iter()
            .filter_map(|r| r.clone().err())
            .collect()
    }

    pub fn stop(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

impl Drop for CaptureServer {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}
