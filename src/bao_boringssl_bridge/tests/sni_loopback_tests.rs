// @trace TEST-ENG-007-SNI [req:REQ-ENG-007] [level:integration]
//
// In-process TLS loopback (memory-BIO, no network): isolates the bridge
// handshake from the node:tls driver. Drives a TlsServer against a
// TlsClient until both sides complete, dumping the BoringSSL error queue
// on failure.

use bao_boringssl_bridge::{
    TlsClient, TlsConnection, TlsServer, generate_self_signed_pem, pem_parse_certs,
};
use bun_boringssl_sys::boringssl::{ERR_clear_error, ERR_error_string, ERR_get_error};

/// Drain the BoringSSL error queue into a printable string.
fn err_queue() -> String {
    let mut out = String::new();
    loop {
        let packed = ERR_get_error();
        if packed == 0 {
            break;
        }
        let mut buf = [0i8; 256];
        // SAFETY: buf is a 256-byte buffer for ERR_error_string.
        unsafe { ERR_error_string(packed, buf.as_mut_ptr()) };
        let bytes: Vec<u8> = buf.iter().map(|b| *b as u8).take_while(|b| *b != 0).collect();
        out.push_str(&String::from_utf8_lossy(&bytes));
        out.push_str("; ");
    }
    out
}

/// Drive one mutual process pass: client → server → client.
fn drive_pair(c: &mut TlsConnection, s: &mut TlsConnection) -> Result<(), String> {
    ERR_clear_error();
    let cr = c.process().map_err(|e| format!("client process: {} | {}", e, err_queue()))?;
    s.feed(&c.take_outgoing());
    ERR_clear_error();
    let sr = s.process().map_err(|e| format!("server process: {} | {}", e, err_queue()))?;
    c.feed(&s.take_outgoing());
    let _ = (cr.state, sr.state);
    Ok(())
}

#[test]
fn loopback_static_handshake() {
    let (cert, key) = generate_self_signed_pem("loop.local", 365).expect("generate cert");
    // Round-trip sanity: the generated PEM parses and loads.
    assert!(!pem_parse_certs(&cert).is_empty(), "cert PEM must parse");
    let expected_der = pem_parse_certs(&cert).into_iter().next().unwrap();

    let server = TlsServer::new(&cert, &key).expect("TlsServer::new");
    let client = TlsClient::new().expect("TlsClient::new");
    // The BoringSSL client verifies by default — anchor the self-signed
    // server certificate (the `ca` option equivalent).
    assert!(
        client.add_trusted_der(&expected_der),
        "trust anchor must install"
    );
    let mut s = server.accept().expect("server accept");
    let mut c = TlsConnection::new_client(&client, "loop.local").expect("client conn");

    for i in 0..32 {
        drive_pair(&mut c, &mut s).unwrap_or_else(|e| panic!("iteration {i}: {e}"));
        if !c.is_handshaking() && !s.is_handshaking() {
            break;
        }
    }
    assert!(!s.is_handshaking(), "server handshake must complete");
    assert!(!c.is_handshaking(), "client handshake must complete");

    // The client must see exactly the server's certificate.
    let peer = c.peer_certificate_der().expect("peer cert");
    assert_eq!(peer, expected_der, "peer cert must equal the server cert");

    // Application data both ways.
    c.write(b"ping").expect("client write");
    s.feed(&c.take_outgoing());
    let r = s.process().expect("server process data");
    let mut got = Vec::new();
    for chunk in &r.plaintext {
        got.extend_from_slice(chunk);
    }
    assert_eq!(got, b"ping", "server must decrypt client data");
    s.write(b"pong").expect("server write");
    c.feed(&s.take_outgoing());
    let r = c.process().expect("client process data");
    let mut got = Vec::new();
    for chunk in &r.plaintext {
        got.extend_from_slice(chunk);
    }
    assert_eq!(got, b"pong", "client must decrypt server data");
}
