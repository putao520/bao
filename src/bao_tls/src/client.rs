//! TLS client configuration and connection builder.

use std::sync::Arc;

use rustls::ClientConfig;
use rustls::pki_types::ServerName;

use crate::connection::{TlsConnection, TlsError};
use crate::provider::bao_crypto_provider;

/// TLS client builder using the Bao CryptoProvider.
pub struct TlsClient {
    config: Arc<ClientConfig>,
    root_store: rustls::RootCertStore,
    no_default_root_store: bool,
    alpn_protocols: Vec<Vec<u8>>,
}

impl TlsClient {
    /// Create a new TLS client with default configuration.
    pub fn new() -> Self {
        let provider = bao_crypto_provider();
        let mut root_store = rustls::RootCertStore::empty();
        // Load native platform certificates
        let native_certs = rustls_native_certs::load_native_certs();
        for cert in native_certs.certs {
            let _ = root_store.add(cert);
        }
        // Also load webpki built-in roots (TrustAnchor -> RootCertStore via extend)
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(root_store.clone())
            .with_no_client_auth();

        Self {
            config: Arc::new(config),
            root_store,
            no_default_root_store: false,
            alpn_protocols: vec![b"h2".to_vec(), b"http/1.1".to_vec()],
        }
    }

    /// Disable default root certificate store.
    pub fn danger_accept_invalid_certs(mut self) -> Self {
        self.no_default_root_store = true;
        self
    }

    /// Set ALPN protocols.
    pub fn with_alpn_protocols(mut self, protocols: Vec<Vec<u8>>) -> Self {
        self.alpn_protocols = protocols;
        self
    }

    /// Add a custom trusted certificate.
    pub fn add_root_certificate(mut self, cert: rustls::pki_types::CertificateDer<'static>) -> Self {
        self.root_store.add(cert).ok();
        self.rebuild_config();
        self
    }

    /// Build the final client configuration.
    pub fn build(mut self) -> Arc<ClientConfig> {
        self.rebuild_config();
        self.config
    }

    /// Connect to a TLS server.
    ///
    /// Creates a new `TlsConnection` in client mode. The caller is responsible
    /// for feeding ciphertext from the transport layer via `TlsConnection::feed()`
    /// and driving the state machine via `TlsConnection::process()`.
    pub fn connect(&self, name: ServerName<'static>) -> Result<TlsConnection, TlsError> {
        TlsConnection::new_client(self.config.clone(), name)
    }

    fn rebuild_config(&mut self) {
        let provider = bao_crypto_provider();
        let builder = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(self.root_store.clone())
            .with_no_client_auth();
        let mut config = builder;
        config.alpn_protocols = self.alpn_protocols.clone();
        self.config = Arc::new(config);
    }
}

impl Default for TlsClient {
    fn default() -> Self {
        Self::new()
    }
}
