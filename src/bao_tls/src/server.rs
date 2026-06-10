//! TLS server configuration and connection builder.

use std::sync::Arc;

use rustls::ServerConfig;

use crate::connection::{TlsConnection, TlsError};
use crate::provider::bao_crypto_provider;

/// TLS server builder using the Bao CryptoProvider.
pub struct TlsServer {
    config: Arc<ServerConfig>,
    alpn_protocols: Vec<Vec<u8>>,
}

impl TlsServer {
    /// Create a new TLS server with the given certificate and private key.
    pub fn new(
        certs: Vec<rustls::pki_types::CertificateDer<'static>>,
        key: rustls::pki_types::PrivateKeyDer<'static>,
    ) -> Self {
        let provider = bao_crypto_provider();
        let config = ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .expect("invalid certificate/key");

        Self {
            config: Arc::new(config),
            alpn_protocols: vec![b"h2".to_vec(), b"http/1.1".to_vec()],
        }
    }

    /// Set ALPN protocols.
    pub fn with_alpn_protocols(mut self, protocols: Vec<Vec<u8>>) -> Self {
        self.alpn_protocols = protocols;
        self.rebuild_config();
        self
    }

    /// Build the final server configuration.
    pub fn build(mut self) -> Arc<ServerConfig> {
        self.rebuild_config();
        self.config
    }

    /// Accept a TLS connection.
    ///
    /// Creates a new `TlsConnection` in server mode. The caller is responsible
    /// for feeding ciphertext from the transport layer via `TlsConnection::feed()`
    /// and driving the state machine via `TlsConnection::process()`.
    pub fn accept(&self) -> Result<TlsConnection, TlsError> {
        TlsConnection::new_server(self.config.clone())
    }

    fn rebuild_config(&mut self) {
        let mut config = (*self.config).clone();
        config.alpn_protocols = self.alpn_protocols.clone();
        self.config = Arc::new(config);
    }
}
