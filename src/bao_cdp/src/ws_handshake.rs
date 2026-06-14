// @trace REQ-CDP-002 [entity:CDPSession]
// WebSocket handshake (RFC 6455 §1.3) — replaces tungstenite::accept
// Uses bun_sha_hmac::SHA1 and bun_base64 for WebSocket accept computation.

use std::io::{BufRead, BufReader, Read, Write};

// GUID from RFC 6455 §1.3
const WEBSOCKET_GUID: &[u8] = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// Perform WebSocket server handshake on a TcpStream.
/// Reads the HTTP upgrade request, validates it, and sends the response.
/// Returns Ok if handshake succeeded, Err on failure.
pub fn server_handshake<S: Read + Write>(stream: &mut S) -> Result<(), HandshakeError> {
    let mut reader = BufReader::new(&mut *stream);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).map_err(|_| HandshakeError::ReadError)?;

    // Check request line: GET /path HTTP/1.1
    if !request_line.starts_with("GET ") {
        return Err(HandshakeError::InvalidRequest);
    }

    // Read headers until empty line
    let mut sec_websocket_key = None;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).map_err(|_| HandshakeError::ReadError)?;
        let line = line.trim();
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
    use bun_sha_hmac::SHA1;

    let key_bytes = key.as_bytes();
    let mut combined = Vec::with_capacity(key_bytes.len() + WEBSOCKET_GUID.len());
    combined.extend_from_slice(key_bytes);
    combined.extend_from_slice(WEBSOCKET_GUID);

    let mut hasher = SHA1::init();
    hasher.update(&combined);
    let mut output = [0u8; 20];
    hasher.r#final(&mut output);

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
        // RFC 6455 example: dGhlIHNhbXBsZSBub25jZQ==
        // → s3pPLMBiTxaQ9kYGzzhZRbmc+Pw=
        let accept = compute_accept("dGhlIHNhbXBsZSBub25jZQ==");
        assert_eq!(accept, "s3pPLMBiTxaQ9kYGzzhZRbmc+Pw=");
    }

    #[test]
    fn server_handshake_valid_request() {
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

        // Verify response
        let mut output = Vec::new();
        cursor.read_to_end(&mut output).unwrap();
        let response = String::from_utf8(output).unwrap();
        assert!(response.starts_with("HTTP/1.1 101"));
        assert!(response.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbmc+Pw="));
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
