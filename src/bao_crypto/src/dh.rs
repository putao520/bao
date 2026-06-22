// @trace REQ-ENG-007 [entity:bao_crypto] [api:node:crypto createDiffieHellman]
// MODP Diffie-Hellman via BoringSSL DH_*. Backs `crypto.createDiffieHellman`
// in node_crypto. Node API surface (subset sufficient for real use):
//   createDiffieHellman(prime | primeLength[, generator])
//     .generateKeys([encoding])              -> Buffer / encoded string
//     .computeSecret(peerPub[, iEnc[, oEnc]]) -> Buffer / encoded string
//     .getPrime([encoding])                  -> Buffer / encoded string
//     .getGenerator([encoding])              -> Buffer / encoded string
//     .getPublicKey([encoding])              -> Buffer / encoded string
//     .getPrivateKey([encoding])             -> Buffer / encoded string
//
// Real BoringSSL work — no stub. DH_generate_parameters_ex builds a safe prime
// group when a prime length is given; DH_generate_key picks a private exponent;
// DH_compute_key_padded derives the shared secret with leading-zero padding
// (matches PKCS#3 / Node.js semantics).
use crate::CryptoError;
use bun_boringssl_sys as bssl;
use core::ptr;

/// RAII guard for a BoringSSL `BIGNUM*`.
struct BnGuard(*mut bssl::BIGNUM);
impl Drop for BnGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { bssl::BN_free(self.0) };
        }
    }
}

fn bn_to_vec(bn: *const bssl::BIGNUM) -> Vec<u8> {
    if bn.is_null() {
        return Vec::new();
    }
    // BN_num_bytes is a BoringSSL header macro: (BN_num_bits(bn)+7)/8.
    let num_bits = unsafe { bssl::BN_num_bits(bn) } as usize;
    let num_bytes = (num_bits + 7) / 8;
    if num_bytes == 0 {
        return Vec::new();
    }
    let mut buf = vec![0u8; num_bytes];
    let written = unsafe { bssl::BN_bn2bin(bn, buf.as_mut_ptr()) };
    if written <= 0 {
        return Vec::new();
    }
    buf.truncate(written as usize);
    buf
}

fn bytes_to_bn(bytes: &[u8]) -> Result<*mut bssl::BIGNUM, CryptoError> {
    let bn = unsafe { bssl::BN_bin2bn(bytes.as_ptr(), bytes.len(), ptr::null_mut()) };
    if bn.is_null() {
        return Err(CryptoError::KeyExchangeError("BN_bin2bn failed".into()));
    }
    Ok(bn)
}

/// MODP Diffie-Hellman key exchange context.
///
/// Holds a BoringSSL `DH*` plus the original prime/generator bytes for the
/// Node.js `getPrime`/`getGenerator` accessors. Private/public keys are owned
/// by the underlying `DH*` and materialised on `generate_keys()`.
pub struct DiffieHellman {
    dh: *mut bssl::DH,
    prime: Vec<u8>,
    generator: Vec<u8>,
    has_keys: bool,
}

impl Drop for DiffieHellman {
    fn drop(&mut self) {
        // SAFETY: dh is a valid DH* obtained from DH_new / DH_generate_*; we
        // free exactly once on Drop. After this point self.dh is dangling but
        // never dereferenced again.
        unsafe { bssl::DH_free(self.dh) };
    }
}

impl DiffieHellman {
    /// Build from an explicit prime buffer. `generator` defaults to 2 when 0.
    /// Mirrors Node's `createDiffieHellman(prime, generator)` overload.
    pub fn from_prime(prime: &[u8], generator: i32) -> Result<Self, CryptoError> {
        if prime.is_empty() {
            return Err(CryptoError::KeyExchangeError("empty prime".into()));
        }
        let g = if generator <= 0 { bssl::DH_GENERATOR_2 } else { generator };
        let dh = unsafe { bssl::DH_new() };
        if dh.is_null() {
            return Err(CryptoError::KeyGenerationFailed("DH_new failed".into()));
        }
        // Ownership of p_bn/g_bn transfers to the DH on success (DH_set0_pqg).
        let p_bn = bytes_to_bn(prime)?;
        let g_bn_raw = [g as u8];
        let g_bn = unsafe { bssl::BN_bin2bn(g_bn_raw.as_ptr(), g_bn_raw.len(), ptr::null_mut()) };
        if g_bn.is_null() {
            unsafe { bssl::BN_free(p_bn) };
            unsafe { bssl::DH_free(dh) };
            return Err(CryptoError::KeyGenerationFailed("BN_bin2bn(generator) failed".into()));
        }
        let rc = unsafe { bssl::DH_set0_pqg(dh, p_bn, ptr::null_mut(), g_bn) };
        if rc != 1 {
            // DH_set0_pqg did not take ownership on failure; free both bignums.
            unsafe { bssl::BN_free(p_bn) };
            unsafe { bssl::BN_free(g_bn) };
            unsafe { bssl::DH_free(dh) };
            return Err(CryptoError::KeyGenerationFailed("DH_set0_pqg failed".into()));
        }
        Ok(DiffieHellman {
            dh,
            prime: prime.to_vec(),
            generator: g_bn_raw.to_vec(),
            has_keys: false,
        })
    }

    /// Build by generating a safe-prime group of `prime_bits` length.
    /// Mirrors Node's `createDiffieHellman(primeLength, generator)` overload.
    pub fn generate(prime_bits: u32, generator: i32) -> Result<Self, CryptoError> {
        if prime_bits == 0 || prime_bits > 8192 {
            return Err(CryptoError::KeyExchangeError(format!(
                "invalid prime length: {}",
                prime_bits
            )));
        }
        let g = if generator <= 0 { bssl::DH_GENERATOR_2 } else { generator };
        let dh = unsafe { bssl::DH_new() };
        if dh.is_null() {
            return Err(CryptoError::KeyGenerationFailed("DH_new failed".into()));
        }
        // DH_generate_parameters_ex fills p,q,g on the DH. No BN_GENCB needed.
        let rc = unsafe {
            bssl::DH_generate_parameters_ex(dh, prime_bits as core::ffi::c_int, g, ptr::null_mut())
        };
        if rc != 1 {
            unsafe { bssl::DH_free(dh) };
            return Err(CryptoError::KeyGenerationFailed(
                "DH_generate_parameters_ex failed".into(),
            ));
        }
        let prime = {
            let p = unsafe { bssl::DH_get0_p(dh) };
            bn_to_vec(p)
        };
        let gen_bytes = {
            let g_bn = unsafe { bssl::DH_get0_g(dh) };
            bn_to_vec(g_bn)
        };
        Ok(DiffieHellman {
            dh,
            prime,
            generator: gen_bytes,
            has_keys: false,
        })
    }

    /// Generate a fresh (private, public) keypair for this DH group.
    /// Returns the public key bytes (Node's `generateKeys()` contract).
    pub fn generate_keys(&mut self) -> Result<Vec<u8>, CryptoError> {
        let rc = unsafe { bssl::DH_generate_key(self.dh) };
        if rc != 1 {
            return Err(CryptoError::KeyGenerationFailed("DH_generate_key failed".into()));
        }
        self.has_keys = true;
        let pub_bn = unsafe { bssl::DH_get0_pub_key(self.dh) };
        Ok(bn_to_vec(pub_bn))
    }

    /// Compute the shared secret given the peer's public key bytes.
    /// Uses `DH_compute_key_padded` so the output length equals `DH_size`
    /// (matches Node.js / PKCS#3, avoids leading-zero stripping).
    pub fn compute_secret(&self, peer_public: &[u8]) -> Result<Vec<u8>, CryptoError> {
        if !self.has_keys {
            return Err(CryptoError::SharedSecretFailed(
                "generate_keys() not called".into(),
            ));
        }
        let peer_bn = BnGuard(bytes_to_bn(peer_public)?);
        let out_len = unsafe { bssl::DH_size(self.dh) };
        if out_len <= 0 {
            return Err(CryptoError::SharedSecretFailed("DH_size <= 0".into()));
        }
        let mut out = vec![0u8; out_len as usize];
        let written = unsafe { bssl::DH_compute_key_padded(out.as_mut_ptr(), peer_bn.0, self.dh) };
        if written <= 0 {
            return Err(CryptoError::SharedSecretFailed(
                "DH_compute_key_padded failed".into(),
            ));
        }
        out.truncate(written as usize);
        Ok(out)
    }

    /// The DH group prime (p), raw bytes.
    pub fn prime(&self) -> &[u8] {
        &self.prime
    }

    /// The DH group generator (g), raw bytes.
    pub fn generator(&self) -> &[u8] {
        &self.generator
    }

    /// The public key bytes (empty until `generate_keys()` is called).
    pub fn public_key(&self) -> Vec<u8> {
        if !self.has_keys {
            return Vec::new();
        }
        let pub_bn = unsafe { bssl::DH_get0_pub_key(self.dh) };
        bn_to_vec(pub_bn)
    }

    /// The private key bytes (empty until `generate_keys()` is called).
    pub fn private_key(&self) -> Vec<u8> {
        if !self.has_keys {
            return Vec::new();
        }
        let priv_bn = unsafe { bssl::DH_get0_priv_key(self.dh) };
        bn_to_vec(priv_bn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dh_roundtrip_real_shared_secret() {
        // Generate a small safe-prime group; both parties derive the same secret.
        let mut alice = DiffieHellman::generate(512, 2).expect("generate alice group");
        let bob_group_prime = alice.prime().to_vec();
        let bob_group_g = alice.generator().first().copied().unwrap_or(2) as i32;
        let mut bob = DiffieHellman::from_prime(&bob_group_prime, bob_group_g)
            .expect("build bob from alice prime");

        let alice_pub = alice.generate_keys().expect("alice generateKeys");
        let bob_pub = bob.generate_keys().expect("bob generateKeys");

        assert!(!alice_pub.is_empty());
        assert!(!bob_pub.is_empty());

        let s_a = alice.compute_secret(&bob_pub).expect("alice computeSecret");
        let s_b = bob.compute_secret(&alice_pub).expect("bob computeSecret");
        assert_eq!(s_a.len(), s_b.len());
        assert_eq!(s_a, s_b, "DH shared secrets must match");
    }

    #[test]
    fn dh_from_prime_explicit_generator() {
        // Use a tiny known prime (not a safe prime, but valid for API wiring).
        let prime = vec![0xFFu8; 32];
        let mut dh = DiffieHellman::from_prime(&prime, 5).expect("from_prime");
        assert_eq!(dh.generator(), &[5u8]);
        let pub_key = dh.generate_keys().expect("generateKeys");
        assert!(!pub_key.is_empty());
    }

    #[test]
    fn dh_get_prime_generator_reflect_input() {
        let prime = vec![0xABu8, 0xCD, 0xEF];
        let dh = DiffieHellman::from_prime(&prime, 2).expect("from_prime");
        assert_eq!(dh.prime(), &prime);
        assert_eq!(dh.generator(), &[2u8]);
    }
}
