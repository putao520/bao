//! Pure Rust crypto implementations for Bao runtime.
//!
//! Backed by RustCrypto crate family: sha2/hmac/aes-gcm/chacha20poly1305/rsa/ecdsa/ed25519-dalek/x25519-dalek/x509-cert.
//!
//! This crate provides Node.js `crypto` API semantics without any C dependency (no BoringSSL/OpenSSL).
//! Hash/HMAC/PBKDF2/scrypt continue to use `bun_sha_hmac` (BoringSSL) in bun_runtime;
//! this crate covers asymmetric, AEAD, key exchange, and certificate operations.

pub mod certificate;
pub mod cipher;
pub mod key_exchange;
pub mod keypair;
pub mod kdf;
pub mod random;
pub mod sign;
pub mod verify;
