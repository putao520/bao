//! Pure Rust TLS 1.2/1.3 for Bao runtime.
//!
//! Uses rustls with a self-built CryptoProvider backed by RustCrypto primitives.
//! Integrates with bun_uws_sys via Unbuffered API for zero-copy TLS operations.

pub mod client;
pub mod connection;
pub mod profile;
pub mod provider;
pub mod server;

pub use client::TlsClient;
pub use connection::{TlsConnection, TlsError, ProcessResult, TlsState};
pub use profile::TlsProfile;
pub use provider::bao_crypto_provider;
pub use server::TlsServer;
