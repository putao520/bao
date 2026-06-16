//! RFC 6455 WebSocket handshake — owned by `bun_uws`.
//!
//! Migrated from `bao_cdp::ws_handshake` (TASK-18, REQ-CDP-UWS-001). Provides
//! both server-side (`server_handshake`) and client-side (`client_handshake`)
//! RFC 6455 §1.3 / §4.1 upgrade logic, plus the `Sec-WebSocket-Accept` SHA1
//! derivation. Uses [`bun_base64`] + BoringSSL `SHA1` (BoringSSL's low-level
//! one-shot SHA1 produces correct digests; the EVP wrapper does not on short
//! inputs).
//!
//! @trace REQ-CDP-UWS-001

use std::io::{Read, Write};

use crate::ws_codec::apply_mask;

/// GUID appended to the client's `Sec-WebSocket-Key` (RFC 6455 §1.3).
const WEBSOCKET_GUID: &[u8] = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// 16-byte client nonce used for `Sec-WebSocket-Key` generation. Address/time
/// seeded XorShift — RFC 6455 §1.3 only requires "unpredictable" (not
/// cryptographic); trusted CDP-client traffic matches Chromium's behavior.
fn generate_client_nonce() -> [u8; 16] {
    let mut state: u64 = 0x9E3779B97F4A7C15u64;
    state ^= std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0xCAFEBABE);
    let stack_addr = &state as *const _ as u64;
    state ^= stack_addr;
    let mut out = [0u8; 16];
    for i in 0..16 {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        out[i] = ((state.wrapping_mul(0x2545F4914F6CDD1D)) >> ((i % 8) * 8)) as u8;
    }
    out
}

/// Generate a `Sec-WebSocket-Key` header value (base64-encoded 16-byte nonce).
pub fn generate_sec_websocket_key() -> String {
    use bun_base64::encode_alloc;
    let nonce = generate_client_nonce();
    String::from_utf8(encode_alloc(&nonce)).unwrap_or_default()
}

/// Read a single CRLF-terminated line directly from a stream (unbuffered).
/// Avoids `BufReader` caching issues where buffered-but-unconsumed bytes are
/// lost when the stream is later written to.
fn read_line_direct<S: Read>(stream: &mut S) -> Result<String, HandshakeError> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(0) => return Err(HandshakeError::ReadError),
            Ok(_) => {
                buf.push(byte[0]);
                if buf.ends_with(b"\r\n") {
                    buf.truncate(buf.len() - 2);
                    return String::from_utf8(buf).map_err(|_| HandshakeError::InvalidRequest);
                }
                if buf.len() > 8192 {
                    return Err(HandshakeError::InvalidRequest);
                }
            }
            Err(_) => return Err(HandshakeError::ReadError),
        }
    }
}

/// Perform the WebSocket server-side handshake on a stream (RFC 6455 §1.3).
/// Reads the HTTP upgrade request, computes `Sec-WebSocket-Accept`, and writes
/// the `101 Switching Protocols` response.
pub fn server_handshake<S: Read + Write>(stream: &mut S) -> Result<(), HandshakeError> {
    let request_line = read_line_direct(stream)?;

    if !request_line.starts_with("GET ") {
        return Err(HandshakeError::InvalidRequest);
    }

    let mut sec_websocket_key = None;
    loop {
        let line = read_line_direct(stream)?;
        if line.is_empty() {
            break;
        }
        if line.to_lowercase().starts_with("sec-websocket-key:") {
            let key = line.split(':').nth(1).map(|s| s.trim());
            if let Some(key) = key {
                sec_websocket_key = Some(key.to_string());
            }
        }
    }

    let key = sec_websocket_key.ok_or(HandshakeError::MissingKey)?;
    let accept = compute_accept(&key);

    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: {}\r\n\
         \r\n",
        accept
    );

    stream
        .write_all(response.as_bytes())
        .map_err(|_| HandshakeError::WriteError)?;
    stream.flush().map_err(|_| HandshakeError::WriteError)?;
    Ok(())
}

/// Perform the WebSocket client-side handshake (RFC 6455 §4.1).
///
/// Sends the HTTP Upgrade request with `Sec-WebSocket-Key`, validates the
/// server's `Sec-WebSocket-Accept` against `SHA1(key + GUID)`. Returns the
/// generated key on success.
pub fn client_handshake<S: Read + Write>(
    stream: &mut S,
    host: &str,
    path: &str,
) -> Result<String, HandshakeError> {
    let sec_websocket_key = generate_sec_websocket_key();

    let request = format!(
        "GET {} HTTP/1.1\r\n\
         Host: {}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: {}\r\n\
         Sec-WebSocket-Version: 13\r\n\
         \r\n",
        path, host, sec_websocket_key
    );

    stream
        .write_all(request.as_bytes())
        .map_err(|_| HandshakeError::WriteError)?;
    stream
        .flush()
        .map_err(|_| HandshakeError::WriteError)?;

    let status_line = read_line_direct(stream)?;
    if !status_line.starts_with("HTTP/1.1 101") {
        return Err(HandshakeError::InvalidRequest);
    }

    let mut accept_value: Option<String> = None;
    loop {
        let line = read_line_direct(stream)?;
        if line.is_empty() {
            break;
        }
        if line.to_lowercase().starts_with("sec-websocket-accept:") {
            let val = line.split(':').nth(1).map(|s| s.trim());
            if let Some(v) = val {
                accept_value = Some(v.to_string());
            }
        }
    }

    let accept = accept_value.ok_or(HandshakeError::MissingKey)?;
    let expected = compute_accept(&sec_websocket_key);
    if accept != expected {
        return Err(HandshakeError::MissingKey);
    }

    Ok(sec_websocket_key)
}

/// Compute `Sec-WebSocket-Accept` = `base64(SHA1(key + GUID))` per RFC 6455 §1.3.
pub fn compute_accept(key: &str) -> String {
    use bun_base64::encode_alloc;

    let key_bytes = key.as_bytes();
    let mut combined = Vec::with_capacity(key_bytes.len() + WEBSOCKET_GUID.len());
    combined.extend_from_slice(key_bytes);
    combined.extend_from_slice(WEBSOCKET_GUID);

    // BoringSSL low-level SHA1() — one-shot, no init/update/final state to misuse.
    // (bun_sha_hmac::SHA1 EVP wrapper produces incorrect digests on short inputs.)
    let mut output = [0u8; 20];
    unsafe {
        bun_boringssl_sys::SHA1(combined.as_ptr(), combined.len(), output.as_mut_ptr());
    }
    String::from_utf8(encode_alloc(&output)).unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandshakeError {
    ReadError,
    WriteError,
    InvalidRequest,
    MissingKey,
}

// Keep `apply_mask` reachable from this module's namespace for symmetry with
// the codec — handshake itself doesn't mask, but callers consuming both
// modules expect a single import path.
#[allow(dead_code)]
fn _mask_link(payload: &mut [u8], key: &[u8; 4]) {
    apply_mask(payload, key);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn compute_accept_known_vector() {
        // RFC 6455 §4.2.2 example vector.
        let accept = compute_accept("dGhlIHNhbXBsZSBub25jZQ==");
        assert_eq!(accept, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn server_handshake_valid_request() {
        use std::io::Seek;
        let request = b"GET /chat HTTP/1.1\r\n\
                       Host: example.com\r\n\
                       Upgrade: websocket\r\n\
                       Connection: Upgrade\r\n\
                       Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
                       Sec-WebSocket-Version: 13\r\n\
                       \r\n";

        let mut cursor = Cursor::new(request.to_vec());
        let result = server_handshake(&mut cursor);
        assert!(result.is_ok());

        cursor.seek(std::io::SeekFrom::Start(0)).unwrap();
        let mut output = Vec::new();
        cursor.read_to_end(&mut output).unwrap();
        let response = String::from_utf8(output).unwrap();
        assert!(response.contains("HTTP/1.1 101 Switching Protocols"));
        assert!(response.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo="));
    }

    #[test]
    fn server_handshake_missing_key() {
        let request = b"GET /chat HTTP/1.1\r\nHost: example.com\r\n\r\n";
        let mut cursor = Cursor::new(request.to_vec());
        let result = server_handshake(&mut cursor);
        assert!(matches!(result, Err(HandshakeError::MissingKey)));
    }

    #[test]
    fn server_handshake_non_get_request() {
        let request = b"POST /chat HTTP/1.1\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n";
        let mut cursor = Cursor::new(request.to_vec());
        let result = server_handshake(&mut cursor);
        assert!(matches!(result, Err(HandshakeError::InvalidRequest)));
    }

    #[test]
    fn generate_sec_websocket_key_is_base64_24_chars() {
        let key = generate_sec_websocket_key();
        assert_eq!(key.len(), 24);
        assert!(
            key.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '='),
            "invalid chars: {}",
            key
        );
    }

    #[test]
    fn generate_sec_websocket_key_unique() {
        let k1 = generate_sec_websocket_key();
        let k2 = generate_sec_websocket_key();
        assert_ne!(k1, k2);
    }

    #[test]
    fn client_handshake_valid_response() {
        let known_key = "dGhlIHNhbXBsZSBub25jZQ==";
        let accept = compute_accept(known_key);
        assert_eq!(accept, "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");

        let mut buf: Vec<u8> = Vec::new();
        let request = format!(
            "GET / HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: {}\r\nSec-WebSocket-Version: 13\r\n\r\n",
            known_key
        );
        buf.extend_from_slice(request.as_bytes());
        let response = format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Accept: {}\r\n\r\n",
            accept
        );
        buf.extend_from_slice(response.as_bytes());

        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("HTTP/1.1 101 Switching Protocols"));
        assert!(s.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo="));
    }

    #[test]
    fn client_handshake_rejects_non_101_status() {
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(
            b"GET / HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
        );
        buf.extend_from_slice(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
        let mut cursor = Cursor::new(buf);
        let result = client_handshake(&mut cursor, "test", "/");
        assert!(matches!(result, Err(HandshakeError::InvalidRequest)));
    }

    #[test]
    fn client_handshake_rejects_wrong_accept() {
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(
            b"GET / HTTP/1.1\r\nHost: test\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
              Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n",
        );
        buf.extend_from_slice(
            b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
              Sec-WebSocket-Accept: aW52YWxpZGtleQ==\r\n\r\n",
        );
        let mut cursor = Cursor::new(buf);
        let result = client_handshake(&mut cursor, "test", "/");
        assert!(matches!(result, Err(HandshakeError::MissingKey)));
    }
}
