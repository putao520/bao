//! Hash implementations for rustls CryptoProvider.
//!
//! Implements `rustls::crypto::hash::Hash` and `rustls::crypto::hash::Context`
//! for SHA-256 and SHA-384 using the RustCrypto `sha2` crate.
//!
//! Follows the ring provider pattern: concrete struct types wrapping the
//! algorithm info, with static references providing `&'static dyn Hash`.

use rustls::crypto::hash::{Context, Hash, HashAlgorithm, Output};
use sha2::{Digest, Sha256, Sha384};

// ─── SHA-256 ──────────────────────────────────────────────────────────

pub(crate) static SHA256: &dyn Hash = &Sha256Hash;

pub(super) struct Sha256Hash;

impl Hash for Sha256Hash {
    fn start(&self) -> Box<dyn Context> {
        Box::new(Sha256Context(Sha256::new()))
    }

    fn hash(&self, data: &[u8]) -> Output {
        let result = Sha256::digest(data);
        Output::new(&result)
    }

    fn output_len(&self) -> usize {
        32
    }

    fn algorithm(&self) -> HashAlgorithm {
        HashAlgorithm::SHA256
    }
}

struct Sha256Context(Sha256);

impl Context for Sha256Context {
    fn fork_finish(&self) -> Output {
        let hasher = self.0.clone();
        let result = hasher.finalize();
        Output::new(&result)
    }

    fn fork(&self) -> Box<dyn Context> {
        Box::new(Sha256Context(self.0.clone()))
    }

    fn finish(self: Box<Self>) -> Output {
        let result = self.0.finalize();
        Output::new(&result)
    }

    fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }
}

// ─── SHA-384 ──────────────────────────────────────────────────────────

pub(crate) static SHA384: &dyn Hash = &Sha384Hash;

pub(super) struct Sha384Hash;

impl Hash for Sha384Hash {
    fn start(&self) -> Box<dyn Context> {
        Box::new(Sha384Context(Sha384::new()))
    }

    fn hash(&self, data: &[u8]) -> Output {
        let result = Sha384::digest(data);
        Output::new(&result)
    }

    fn output_len(&self) -> usize {
        48
    }

    fn algorithm(&self) -> HashAlgorithm {
        HashAlgorithm::SHA384
    }
}

struct Sha384Context(Sha384);

impl Context for Sha384Context {
    fn fork_finish(&self) -> Output {
        let hasher = self.0.clone();
        let result = hasher.finalize();
        Output::new(&result)
    }

    fn fork(&self) -> Box<dyn Context> {
        Box::new(Sha384Context(self.0.clone()))
    }

    fn finish(self: Box<Self>) -> Output {
        let result = self.0.finalize();
        Output::new(&result)
    }

    fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }
}
