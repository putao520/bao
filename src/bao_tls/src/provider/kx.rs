//! Key exchange groups for rustls CryptoProvider.
//!
//! Implements `SupportedKxGroup` and `ActiveKeyExchange` traits using
//! RustCrypto backends: x25519-dalek (X25519), p256 (secp256r1), p384 (secp384r1).

use std::fmt;

use rand_core::OsRng;
use rustls::crypto::{ActiveKeyExchange, SharedSecret, SupportedKxGroup};
use rustls::{Error, NamedGroup};

// ─── X25519 ──────────────────────────────────────────────────────────

/// X25519 key exchange group.
pub(crate) static X25519: &dyn SupportedKxGroup = &X25519Group;

#[derive(Debug)]
struct X25519Group;

impl SupportedKxGroup for X25519Group {
    fn start(&self) -> Result<Box<dyn ActiveKeyExchange>, Error> {
        let secret = x25519_dalek::StaticSecret::random_from_rng(OsRng);
        let public = x25519_dalek::PublicKey::from(&secret);
        Ok(Box::new(X25519Kx {
            secret,
            public_key: public.as_bytes().to_vec(),
        }))
    }

    fn name(&self) -> NamedGroup {
        NamedGroup::X25519
    }
}

struct X25519Kx {
    secret: x25519_dalek::StaticSecret,
    public_key: Vec<u8>,
}

impl ActiveKeyExchange for X25519Kx {
    fn complete(self: Box<Self>, peer_pub_key: &[u8]) -> Result<SharedSecret, Error> {
        let peer: [u8; 32] = peer_pub_key
            .try_into()
            .map_err(|_| Error::General("X25519 peer public key must be 32 bytes".into()))?;
        let peer_pk = x25519_dalek::PublicKey::from(peer);
        let shared = self.secret.diffie_hellman(&peer_pk);
        // x25519-dalek validates the shared secret is not all-zeros
        if shared.was_contributory() {
            Ok(SharedSecret::from(shared.as_bytes().as_slice()))
        } else {
            Err(Error::General("X25519: all-zero shared secret (peer public key is contributory-equivalent to zero)".into()))
        }
    }

    fn pub_key(&self) -> &[u8] {
        &self.public_key
    }

    fn group(&self) -> NamedGroup {
        NamedGroup::X25519
    }
}

impl fmt::Debug for X25519Kx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("X25519Kx")
            .field("public_key", &self.public_key)
            .finish_non_exhaustive()
    }
}

// ─── secp256r1 (P-256) ───────────────────────────────────────────────

/// P-256 (secp256r1) key exchange group.
pub(crate) static SECP256R1: &dyn SupportedKxGroup = &Secp256r1Group;

#[derive(Debug)]
struct Secp256r1Group;

impl SupportedKxGroup for Secp256r1Group {
    fn start(&self) -> Result<Box<dyn ActiveKeyExchange>, Error> {
        let secret = p256::SecretKey::random(&mut OsRng);
        let public = secret.public_key();
        Ok(Box::new(Secp256r1Kx {
            secret,
            public_key: public.to_sec1_bytes().to_vec(),
        }))
    }

    fn name(&self) -> NamedGroup {
        NamedGroup::secp256r1
    }
}

struct Secp256r1Kx {
    secret: p256::SecretKey,
    public_key: Vec<u8>,
}

impl ActiveKeyExchange for Secp256r1Kx {
    fn complete(self: Box<Self>, peer_pub_key: &[u8]) -> Result<SharedSecret, Error> {
        let peer = p256::PublicKey::from_sec1_bytes(peer_pub_key)
            .map_err(|e| Error::General(format!("P-256: invalid peer public key: {e}")))?;
        let shared =
            p256::ecdh::diffie_hellman(self.secret.to_nonzero_scalar(), peer.as_affine());
        Ok(SharedSecret::from(shared.raw_secret_bytes().as_slice()))
    }

    fn pub_key(&self) -> &[u8] {
        &self.public_key
    }

    fn group(&self) -> NamedGroup {
        NamedGroup::secp256r1
    }
}

impl fmt::Debug for Secp256r1Kx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Secp256r1Kx")
            .field("public_key", &self.public_key)
            .finish_non_exhaustive()
    }
}

// ─── secp384r1 (P-384) ───────────────────────────────────────────────

/// P-384 (secp384r1) key exchange group.
pub(crate) static SECP384R1: &dyn SupportedKxGroup = &Secp384r1Group;

#[derive(Debug)]
struct Secp384r1Group;

impl SupportedKxGroup for Secp384r1Group {
    fn start(&self) -> Result<Box<dyn ActiveKeyExchange>, Error> {
        let secret = p384::SecretKey::random(&mut OsRng);
        let public = secret.public_key();
        Ok(Box::new(Secp384r1Kx {
            secret,
            public_key: public.to_sec1_bytes().to_vec(),
        }))
    }

    fn name(&self) -> NamedGroup {
        NamedGroup::secp384r1
    }
}

struct Secp384r1Kx {
    secret: p384::SecretKey,
    public_key: Vec<u8>,
}

impl ActiveKeyExchange for Secp384r1Kx {
    fn complete(self: Box<Self>, peer_pub_key: &[u8]) -> Result<SharedSecret, Error> {
        let peer = p384::PublicKey::from_sec1_bytes(peer_pub_key)
            .map_err(|e| Error::General(format!("P-384: invalid peer public key: {e}")))?;
        let shared =
            p384::ecdh::diffie_hellman(self.secret.to_nonzero_scalar(), peer.as_affine());
        Ok(SharedSecret::from(shared.raw_secret_bytes().as_slice()))
    }

    fn pub_key(&self) -> &[u8] {
        &self.public_key
    }

    fn group(&self) -> NamedGroup {
        NamedGroup::secp384r1
    }
}

impl fmt::Debug for Secp384r1Kx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Secp384r1Kx")
            .field("public_key", &self.public_key)
            .finish_non_exhaustive()
    }
}
