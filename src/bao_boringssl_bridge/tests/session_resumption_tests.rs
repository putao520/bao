// @trace TEST-STL-001-RESUME [req:REQ-STL-001] [level:integration]
//
// In-process TLS loopback (memory-BIO, no network) proving the unified
// session-resumption layer: same-origin reconnects resume (1-RTT / PSK),
// cross-origin and cross-profile connections do not. Both client sockets go
// through `session_cache::offer_session` and the ctx-level new-session
// callback — the exact production wiring of the bun_http and servo stacks.

use bao_boringssl_bridge::session_cache;
use bao_boringssl_bridge::{TlsClient, TlsConnection, TlsServer, generate_self_signed_pem};
use bun_boringssl_sys::boringssl::{ERR_clear_error, ERR_error_string, ERR_get_error};

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

/// One mutual drive pass: client → server → client.
fn drive(c: &mut TlsConnection, s: &mut TlsConnection) {
    ERR_clear_error();
    c.process().unwrap_or_else(|e| panic!("client process: {e} | {}", err_queue()));
    s.feed(&c.take_outgoing());
    ERR_clear_error();
    s.process().unwrap_or_else(|e| panic!("server process: {e} | {}", err_queue()));
    c.feed(&s.take_outgoing());
}

/// Drive until both handshakes complete, then flush the deferred TLS 1.3
/// NewSessionTickets (the server sends them on its first write over TCP —
/// tls13_server.cc `do_send_new_session_ticket`) and drive them to the
/// client, where processing fires the new-session callback.
fn drive_to_completion(c: &mut TlsConnection, s: &mut TlsConnection) {
    for _ in 0..64 {
        drive(c, s);
        if !c.is_handshaking() && !s.is_handshaking() {
            break;
        }
    }
    assert!(!c.is_handshaking(), "client handshake must complete | {}", err_queue());
    assert!(!s.is_handshaking(), "server handshake must complete | {}", err_queue());
    // One-byte marker write flushes the ticket flight; the marker itself is
    // consumed (and discarded) by the extra drive passes below.
    s.write(b"\x00").expect("ticket-flush marker write");
    for _ in 0..8 {
        drive(c, s);
    }
}

fn setup() -> (TlsServer, TlsClient) {
    let (cert, key) = generate_self_signed_pem("resume.local", 365).expect("generate cert");
    let der = bao_boringssl_bridge::pem_parse_certs(&cert).into_iter().next().unwrap();
    let server = TlsServer::new(&cert, &key).expect("TlsServer::new");
    // TlsClient::new registers the session-cache callbacks (production path).
    let client = TlsClient::new().expect("TlsClient::new");
    assert!(client.add_trusted_der(&der), "trust anchor must install");
    (server, client)
}

/// What a driven connection reports about resumption.
struct Outcome {
    /// Whether `offer_session` found a cached session to offer.
    offered: bool,
    /// Whether the handshake actually resumed (asserted equal on the server
    /// side inside `connect_and_drive`).
    client_resumed: bool,
}

/// Connect through the production offer path and drive to completion.
fn connect_and_drive(
    server: &TlsServer,
    client: &TlsClient,
    host: &str,
    port: u16,
    salt: u64,
) -> Outcome {
    let mut s = server.accept().expect("server accept");
    let mut c = TlsConnection::new_client(client, host).expect("client conn");
    // Must offer BEFORE the handshake starts (SSL_set_session contract).
    let offered = session_cache::offer_session(c.ssl_ptr(), host, port, salt);
    drive_to_completion(&mut c, &mut s);
    let client_resumed = session_cache::session_reused(c.ssl_ptr());
    let server_resumed = session_cache::session_reused(s.ssl_ptr());
    assert_eq!(
        client_resumed, server_resumed,
        "client and server must agree on resumption (queue: {})",
        err_queue()
    );
    let _ = c;
    Outcome {
        offered,
        client_resumed,
    }
}

#[test]
fn same_origin_second_connection_resumes() {
    let (server, client) = setup();
    let host = "resume.local";
    let cache = session_cache::global();
    cache.clear();

    // First connection: full handshake; the new-session callback must
    // populate the store (TLS 1.3 ticket arrives post-handshake, TLS 1.2
    // at completion — either way drive_to_completion covers it).
    let o1 = connect_and_drive(&server, &client, host, 443, 0);
    assert!(!o1.client_resumed, "first connection is a full handshake");
    assert!(!o1.offered, "empty store must offer nothing");
    assert!(
        cache.contains_key(&session_cache::origin_key(host, 443, 0)),
        "new-session callback must populate the store (queue: {})",
        err_queue()
    );

    // Second connection to the same origin: offered and resumed.
    let o2 = connect_and_drive(&server, &client, host, 443, 0);
    assert!(o2.offered, "store hit must be offered");
    assert!(
        o2.client_resumed,
        "second connection to the same origin must resume (queue: {})",
        err_queue()
    );
    cache.clear();
}

#[test]
fn cross_origin_and_cross_profile_do_not_resume() {
    let (server, client) = setup();
    let cache = session_cache::global();
    cache.clear();

    let o1 = connect_and_drive(&server, &client, "resume.local", 443, 0);
    assert!(!o1.client_resumed);
    assert!(cache.contains_key(&session_cache::origin_key("resume.local", 443, 0)));

    // Different port → different origin → full handshake.
    let o2 = connect_and_drive(&server, &client, "resume.local", 8443, 0);
    assert!(!o2.offered, "different origin must not hit the store");
    assert!(!o2.client_resumed, "different origin must not resume");

    // Different profile salt → segregated cache → full handshake.
    let o3 = connect_and_drive(&server, &client, "resume.local", 443, 0xDEAD_BEEF);
    assert!(!o3.offered, "different profile must not hit the store");
    assert!(!o3.client_resumed, "different profile must not resume");

    // And the default-profile session is still resumable afterwards.
    let o4 = connect_and_drive(&server, &client, "resume.local", 443, 0);
    assert!(o4.offered, "default-profile session must survive cross-profile connects");
    assert!(
        o4.client_resumed,
        "default-profile resumption must still work (queue: {})",
        err_queue()
    );
    cache.clear();
}

#[test]
fn resumed_connection_carries_application_data() {
    let (server, client) = setup();
    let cache = session_cache::global();
    cache.clear();

    let _ = connect_and_drive(&server, &client, "resume.local", 443, 0);
    assert!(cache.contains_key(&session_cache::origin_key("resume.local", 443, 0)));

    // Resumed connection must still exchange application data intact.
    let mut s = server.accept().expect("server accept");
    let mut c = TlsConnection::new_client(&client, "resume.local").expect("client conn");
    assert!(session_cache::offer_session(c.ssl_ptr(), "resume.local", 443, 0));
    drive_to_completion(&mut c, &mut s);
    assert!(session_cache::session_reused(c.ssl_ptr()), "resumption expected");

    c.write(b"ping").expect("client write");
    s.feed(&c.take_outgoing());
    let r = s.process().expect("server process data");
    let mut got = Vec::new();
    for chunk in &r.plaintext {
        got.extend_from_slice(chunk);
    }
    assert_eq!(got, b"ping", "server must decrypt data on a resumed connection");

    s.write(b"pong").expect("server write");
    c.feed(&s.take_outgoing());
    let r = c.process().expect("client process data");
    let mut got = Vec::new();
    for chunk in &r.plaintext {
        got.extend_from_slice(chunk);
    }
    assert_eq!(got, b"pong", "client must decrypt data on a resumed connection");
    cache.clear();
}
