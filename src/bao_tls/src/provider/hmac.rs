//! HMAC implementations for rustls CryptoProvider.
//!
//! Implements `rustls::crypto::hmac::Hmac` and `rustls::crypto::hmac::Key`
//! for SHA-256 and SHA-384 using the RustCrypto `hmac` and `sha2` crates.
//!
//! Follows the ring provider pattern: concrete struct types, with
//! static references providing `&'static dyn Hmac`.

use hmac::{Hmac as HmacImpl, Mac};
use rustls::crypto::hmac::{Hmac, Key, Tag};
use sha2::{Sha256, Sha384};

// ─── HMAC-SHA256 ─────────────────────────────────────────────────────

pub(super) static HMAC_SHA256: HmacSha256 = HmacSha256;

pub(super) struct HmacSha256;

impl Hmac for HmacSha256 {
    fn with_key(&self, key: &[u8]) -> Box<dyn Key> {
        Box::new(HmacSha256Key {
            mac: HmacImpl::<Sha256>::new_from_slice(key)
                .expect("HMAC-SHA256 key initialization accepts any length"),
        })
    }

    fn hash_output_len(&self) -> usize {
        32
    }
}

struct HmacSha256Key {
    mac: HmacImpl<Sha256>,
}

impl Key for HmacSha256Key {
    fn sign_concat(&self, first: &[u8], middle: &[&[u8]], last: &[u8]) -> Tag {
        let mut mac = self.mac.clone();
        mac.update(first);
        for chunk in middle {
            mac.update(chunk);
        }
        mac.update(last);
        let result = mac.finalize().into_bytes();
        Tag::new(&result)
    }

    fn tag_len(&self) -> usize {
        32
    }
}

// ─── HMAC-SHA384 ─────────────────────────────────────────────────────

pub(super) static HMAC_SHA384: HmacSha384 = HmacSha384;

pub(super) struct HmacSha384;

impl Hmac for HmacSha384 {
    fn with_key(&self, key: &[u8]) -> Box<dyn Key> {
        Box::new(HmacSha384Key {
            mac: HmacImpl::<Sha384>::new_from_slice(key)
                .expect("HMAC-SHA384 key initialization accepts any length"),
        })
    }

    fn hash_output_len(&self) -> usize {
        48
    }
}

struct HmacSha384Key {
    mac: HmacImpl<Sha384>,
}

impl Key for HmacSha384Key {
    fn sign_concat(&self, first: &[u8], middle: &[&[u8]], last: &[u8]) -> Tag {
        let mut mac = self.mac.clone();
        mac.update(first);
        for chunk in middle {
            mac.update(chunk);
        }
        mac.update(last);
        let result = mac.finalize().into_bytes();
        Tag::new(&result)
    }

    fn tag_len(&self) -> usize {
        48
    }
}
