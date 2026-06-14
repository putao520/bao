// @trace REQ-CDP-002 [entity:CDPSession]
// WebSocket handshake (RFC 6455 §1.3) — replaces tungstenite::accept
// Uses bun_sha_hmac::SHA1 and bun_base64 for WebSocket accept computation.

use std::io::{Read, Write};

// GUID from RFC 6455 §1.3
const WEBSOCKET_GUID: &[u8] = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// Read a single CRLF-terminated line directly from a stream (unbuffered).
/// Avoids BufReader caching issues where buffered-but-unconsumed bytes get lost
/// when the stream is later written to (e.g. in Cursor-backed tests).
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

/// Perform WebSocket server handshake on a TcpStream.
/// Reads the HTTP upgrade request, validates it, and sends the response.
/// Returns Ok if handshake succeeded, Err on failure.
pub fn server_handshake<S: Read + Write>(stream: &mut S) -> Result<(), HandshakeError> {
    let request_line = read_line_direct(stream)?;

    // Check request line: GET /path HTTP/1.1
    if !request_line.starts_with("GET ") {
        return Err(HandshakeError::InvalidRequest);
    }

    // Read headers until empty line
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

    // Compute Sec-WebSocket-Accept: base64(sha1(key + GUID))
    let accept = compute_accept(&key);

    // Send HTTP 101 response
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

/// Compute Sec-WebSocket-Accept header value from client Sec-WebSocket-Key.
fn compute_accept(key: &str) -> String {
    use bun_base64::encode_alloc;

    let key_bytes = key.as_bytes();
    let mut combined = Vec::with_capacity(key_bytes.len() + WEBSOCKET_GUID.len());
    combined.extend_from_slice(key_bytes);
    combined.extend_from_slice(WEBSOCKET_GUID);

    // BoringSSL low-level SHA1() — one-shot, no init/update/final state to misuse.
    // (bun_sha_hmac::SHA1 EVP wrapper produces incorrect digests on short inputs,
    //  tracked separately.)
    let mut output = [0u8; 20];
    unsafe {
        bun_boringssl_sys::SHA1(combined.as_ptr(), combined.len(), output.as_mut_ptr());
    }

    String::from_utf8(encode_alloc(&output)).unwrap_or_default()
}

#[derive(Debug)]
pub enum HandshakeError {
    ReadError,
    WriteError,
    InvalidRequest,
    MissingKey,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn compute_accept_known_vector() {
        // RFC 6455 §4.2.2 example: Sec-WebSocket-Key dGhlIHNhbXBsZSBub25jZQ==
        // concatenated with GUID 258EAFA5-E914-47DA-95CA-C5AB0DC85B11
        // → SHA1 = b37a4f2cc0624f1690f64606cf385945b2bec4ea
        // → base64 = s3pPLMBiTxaQ9kYGzzhZRbK+xOo=
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

        // Rewind — server_handshake consumed the request and appended the
        // response; we want to inspect the full buffer from the start.
        cursor.seek(std::io::SeekFrom::Start(0)).unwrap();
        let mut output = Vec::new();
        cursor.read_to_end(&mut output).unwrap();
        let response = String::from_utf8(output).unwrap();
        // Buffer contains the original request followed by the server's response,
        // so check the response portion via contains rather than starts_with.
        assert!(response.contains("HTTP/1.1 101 Switching Protocols"));
        assert!(response.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo="));
    }

    #[test]
    fn server_handshake_missing_key() {
        let request = b"GET /chat HTTP/1.1\r\n\
                       Host: example.com\r\n\
                       \r\n";

        let mut cursor = Cursor::new(request.to_vec());
        let result = server_handshake(&mut cursor);
        assert!(matches!(result, Err(HandshakeError::MissingKey)));
    }

    #[test]
    fn server_handshake_non_get_request() {
        let request = b"POST /chat HTTP/1.1\r\n\
                       Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
                       \r\n";

        let mut cursor = Cursor::new(request.to_vec());
        let result = server_handshake(&mut cursor);
        assert!(matches!(result, Err(HandshakeError::InvalidRequest)));
    }
}
