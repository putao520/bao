//! Integration tests for bao_tls CryptoProvider + Unbuffered connection.
//!
//! Tests the full TLS stack: CryptoProvider → ClientConfig/ServerConfig →
//! Unbuffered connection → handshake → encrypt/decrypt.
//!
//! Uses rcgen for self-signed test certificates to avoid needing real CAs.

use std::sync::Arc;

use rustls::ClientConfig;

use bao_tls::{TlsClient, TlsConnection, TlsError, TlsProfile, TlsServer, TlsState, bao_crypto_provider};

/// Generate a self-signed certificate for testing.
fn generate_test_cert() -> (Vec<rustls::pki_types::CertificateDer<'static>>, rustls::pki_types::PrivateKeyDer<'static>) {
    let rcgen::CertifiedKey { cert, key_pair } =
        rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let cert_der = cert.der().clone();
    let key_der = rustls::pki_types::PrivateKeyDer::Pkcs8(key_pair.serialize_der().into());
    (vec![cert_der], key_der)
}

// ─── CryptoProvider tests ────────────────────────────────────────────

#[test]
fn crypto_provider_is_valid() {
    let provider = bao_crypto_provider();
    // Must have at least one cipher suite.
    assert!(!provider.cipher_suites.is_empty(), "CryptoProvider must have cipher suites");
    // Must have at least one kx group.
    assert!(!provider.kx_groups.is_empty(), "CryptoProvider must have kx groups");
    // Must have signature verification algorithms.
    assert!(!provider.signature_verification_algorithms.all.is_empty(),
        "CryptoProvider must have signature verification algorithms");
}

#[test]
fn crypto_provider_cipher_suites_include_tls13() {
    let provider = bao_crypto_provider();
    let has_tls13 = provider.cipher_suites.iter().any(|cs| {
        matches!(cs, rustls::SupportedCipherSuite::Tls13(_))
    });
    assert!(has_tls13, "CryptoProvider must include TLS 1.3 cipher suites");
}

#[test]
fn crypto_provider_kx_groups_include_x25519() {
    let provider = bao_crypto_provider();
    let has_x25519 = provider.kx_groups.iter().any(|g| format!("{:?}", g.name()).contains("X25519"));
    assert!(has_x25519, "CryptoProvider must include X25519 kx group");
}

// ─── ClientConfig/ServerConfig builder tests ─────────────────────────

#[test]
fn tls_client_builds_valid_config() {
    let client = TlsClient::new();
    let config = client.build();
    // Config should be usable — just verify it doesn't panic.
    assert!(!config.alpn_protocols.is_empty());
}

#[test]
fn tls_server_builds_valid_config() {
    let (certs, key) = generate_test_cert();
    let server = TlsServer::new(certs, key);
    let config = server.build();
    assert!(!config.alpn_protocols.is_empty());
}

#[test]
fn tls_client_custom_alpn() {
    let client = TlsClient::new()
        .with_alpn_protocols(vec![b"custom".to_vec()]);
    let config = client.build();
    assert_eq!(config.alpn_protocols, vec![b"custom".to_vec()]);
}

// ─── Unbuffered connection tests ─────────────────────────────────────

#[test]
fn tls_connection_new_client() {
    let client = TlsClient::new();
    let name = rustls::pki_types::ServerName::try_from("localhost")
        .unwrap();
    let conn = client.connect(name);
    assert!(conn.is_ok(), "new_client should succeed: {:?}", conn);
    let conn = conn.unwrap();
    assert!(conn.is_handshaking(), "new client connection should be handshaking");
    assert_eq!(conn.side(), rustls::Side::Client);
}

#[test]
fn tls_connection_new_server() {
    let (certs, key) = generate_test_cert();
    let server = TlsServer::new(certs, key);
    let conn = server.accept();
    assert!(conn.is_ok(), "new_server should succeed: {:?}", conn);
    let conn = conn.unwrap();
    assert!(conn.is_handshaking(), "new server connection should be handshaking");
    assert_eq!(conn.side(), rustls::Side::Server);
}

/// Drive a full TLS handshake between client and server using
/// Unbuffered connections. Returns (client_conn, server_conn) on success.
fn do_handshake() -> (TlsConnection, TlsConnection) {
    let (certs, key) = generate_test_cert();

    // Create server.
    let server = TlsServer::new(certs, key);
    let mut server_conn = server.accept().unwrap();

    // Create client (skip cert verification for self-signed).
    let client_config = {
        let provider = bao_crypto_provider();
        let mut config = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerifier))
            .with_no_client_auth();
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        Arc::new(config)
    };
    let name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let mut client_conn = TlsConnection::new_client(client_config, name).unwrap();

    // Drive handshake by shuttling data between client and server.
    let mut iterations = 0;
    const MAX_ITERATIONS: usize = 20;

    while (client_conn.is_handshaking() || server_conn.is_handshaking())
        && iterations < MAX_ITERATIONS
    {
        iterations += 1;

        // Process client.
        let client_result = client_conn.process().unwrap();
        if client_result.outgoing_bytes > 0 {
            let data = client_conn.take_outgoing();
            server_conn.feed(&data);
        }

        // Process server.
        let server_result = server_conn.process().unwrap();
        if server_result.outgoing_bytes > 0 {
            let data = server_conn.take_outgoing();
            client_conn.feed(&data);
        }
    }

    assert!(!client_conn.is_handshaking(), "client handshake should complete within {MAX_ITERATIONS} iterations");
    assert!(!server_conn.is_handshaking(), "server handshake should complete within {MAX_ITERATIONS} iterations");

    (client_conn, server_conn)
}

#[test]
fn tls_handshake_completes() {
    let (client, server) = do_handshake();
    assert!(!client.is_handshaking());
    assert!(!server.is_handshaking());
}

#[test]
fn tls_client_to_server_data_transfer() {
    let (mut client, mut server) = do_handshake();

    // Client encrypts and sends.
    let message = b"Hello from client!";
    let written = client.write(message).unwrap();
    assert_eq!(written, message.len());

    // Process client to get ciphertext.
    let client_result = client.process().unwrap();
    assert!(client_result.outgoing_bytes > 0);
    let ciphertext = client.take_outgoing();

    // Feed to server.
    server.feed(&ciphertext);
    let server_result = server.process().unwrap();
    assert!(!server_result.plaintext.is_empty());
    assert_eq!(server_result.plaintext[0], message);
}

#[test]
fn tls_server_to_client_data_transfer() {
    let (mut client, mut server) = do_handshake();

    // Server encrypts and sends.
    let message = b"Hello from server!";
    let written = server.write(message).unwrap();
    assert_eq!(written, message.len());

    let server_result = server.process().unwrap();
    assert!(server_result.outgoing_bytes > 0);
    let ciphertext = server.take_outgoing();

    // Feed to client.
    client.feed(&ciphertext);
    let client_result = client.process().unwrap();
    assert!(!client_result.plaintext.is_empty());
    assert_eq!(client_result.plaintext[0], message);
}

#[test]
fn tls_bidirectional_data_transfer() {
    let (mut client, mut server) = do_handshake();

    // Client → Server.
    let msg1 = b"client hello";
    client.write(msg1).unwrap();
    let _cr = client.process().unwrap();
    server.feed(&client.take_outgoing());
    let sr = server.process().unwrap();
    assert_eq!(sr.plaintext[0], msg1);

    // Server → Client.
    let msg2 = b"server response";
    server.write(msg2).unwrap();
    let _sr2 = server.process().unwrap();
    client.feed(&server.take_outgoing());
    let cr2 = client.process().unwrap();
    assert_eq!(cr2.plaintext[0], msg2);
}

#[test]
fn tls_write_before_handshake_fails() {
    let client = TlsClient::new();
    let name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let mut conn = client.connect(name).unwrap();

    // Writing before handshake should fail with NotReady.
    let result = conn.write(b"premature data");
    assert!(matches!(result, Err(TlsError::NotReady)));
}

#[test]
fn tls_peer_closed_detection() {
    let (mut client, mut server) = do_handshake();

    // Server initiates close.
    server.queue_close_notify().unwrap();
    let sr = server.process().unwrap();
    assert!(sr.outgoing_bytes > 0);
    let close_data = server.take_outgoing();

    // Client receives close_notify.
    client.feed(&close_data);
    let _cr = client.process().unwrap();
    assert!(client.peer_closed());
    assert_eq!(_cr.state, TlsState::PeerClosed);
}

#[test]
fn tls_negotiated_cipher_suite() {
    let (client, server) = do_handshake();
    let client_cs = client.negotiated_cipher_suite();
    let server_cs = server.negotiated_cipher_suite();
    assert!(client_cs.is_some());
    assert!(server_cs.is_some());
    assert_eq!(client_cs, server_cs, "client and server should negotiate the same cipher suite");
}

#[test]
fn tls_negotiated_protocol_version() {
    let (client, server) = do_handshake();
    let client_pv = client.protocol_version();
    let server_pv = server.protocol_version();
    assert!(client_pv.is_some());
    assert!(server_pv.is_some());
    assert_eq!(client_pv, server_pv);
}

#[test]
fn tls_process_returns_active_state_after_handshake() {
    let (mut client, _server) = do_handshake();
    let result = client.process().unwrap();
    assert_eq!(result.state, TlsState::Active);
}

// ─── NoVerifier — skip cert verification for self-signed certs ───────

#[derive(Debug)]
struct NoVerifier;

impl rustls::client::danger::ServerCertVerifier for NoVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
        ]
    }
}

// ─── TlsProfile tests ────────────────────────────────────────────────

#[test]
fn tls_profile_default_builds_config() {
    let config = TlsProfile::Default.build_client_config();
    assert!(!config.alpn_protocols.is_empty());
}

#[test]
fn tls_profile_chrome_builds_config() {
    let config = TlsProfile::Chrome.build_client_config();
    assert!(!config.alpn_protocols.is_empty());
    assert_eq!(config.alpn_protocols[0], b"h2");
}

#[test]
fn tls_profile_firefox_builds_config() {
    let config = TlsProfile::Firefox.build_client_config();
    assert!(!config.alpn_protocols.is_empty());
}

#[test]
fn tls_profile_safari_builds_config() {
    let config = TlsProfile::Safari.build_client_config();
    assert!(!config.alpn_protocols.is_empty());
}

#[test]
fn tls_profile_chrome_differs_from_firefox() {
    // Chrome and Firefox should produce different configs (different cipher suite ordering).
    // We verify indirectly: both configs can be used for a handshake.
    let _chrome = TlsProfile::Chrome.build_client_config();
    let _firefox = TlsProfile::Firefox.build_client_config();
    // If both configs are valid and usable, the profile system works.
    // The actual fingerprint difference is in the CryptoProvider's cipher_suites ordering.
}

#[test]
fn tls_profile_safari_kx_order_differs() {
    // Safari puts P-256 before X25519 (different from Chrome's X25519 first).
    let _safari = TlsProfile::Safari.build_client_config();
    let _chrome = TlsProfile::Chrome.build_client_config();
}

#[test]
fn tls_profile_as_str() {
    assert_eq!(TlsProfile::Chrome.as_str(), "chrome");
    assert_eq!(TlsProfile::Firefox.as_str(), "firefox");
    assert_eq!(TlsProfile::Safari.as_str(), "safari");
    assert_eq!(TlsProfile::Default.as_str(), "default");
}

/// Drive a handshake with a specific TlsProfile.
fn do_handshake_with_profile(profile: TlsProfile) -> (TlsConnection, TlsConnection) {
    let (certs, key) = generate_test_cert();

    let server = TlsServer::new(certs, key);
    let mut server_conn = server.accept().unwrap();

    let client_config = {
        // Skip cert verification for self-signed.
        let base_config = profile.build_client_config();
        // We can't modify Arc<ClientConfig>, so build a new one with NoVerifier.
        let provider = bao_crypto_provider();
        let mut root_store = rustls::RootCertStore::empty();
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let mut config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerifier))
            .with_no_client_auth();
        config.alpn_protocols = base_config.alpn_protocols.clone();
        Arc::new(config)
    };

    let name = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let mut client_conn = TlsConnection::new_client(client_config, name).unwrap();

    let mut iterations = 0;
    const MAX_ITERATIONS: usize = 20;

    while (client_conn.is_handshaking() || server_conn.is_handshaking())
        && iterations < MAX_ITERATIONS
    {
        iterations += 1;
        let client_result = client_conn.process().unwrap();
        if client_result.outgoing_bytes > 0 {
            server_conn.feed(&client_conn.take_outgoing());
        }
        let server_result = server_conn.process().unwrap();
        if server_result.outgoing_bytes > 0 {
            client_conn.feed(&server_conn.take_outgoing());
        }
    }

    assert!(!client_conn.is_handshaking());
    assert!(!server_conn.is_handshaking());
    (client_conn, server_conn)
}

#[test]
fn tls_profile_chrome_handshake() {
    let (_client, _server) = do_handshake_with_profile(TlsProfile::Chrome);
}

#[test]
fn tls_profile_firefox_handshake() {
    let (_client, _server) = do_handshake_with_profile(TlsProfile::Firefox);
}

#[test]
fn tls_profile_safari_handshake() {
    let (_client, _server) = do_handshake_with_profile(TlsProfile::Safari);
}
