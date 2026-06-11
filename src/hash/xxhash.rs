//! XxHash32 / XxHash64 / XxHash3 — pure Rust via `twox-hash` crate.
//!
//! Bit-identical output to the xxHash reference implementation and to
//! `std.hash.XxHash{32,64,3}` in Zig. Verified by SMHasher test vectors
//! and by `test/js/bun/util/hash.test.js` in CI.

use std::hash::Hasher;

pub struct XxHash32;

impl XxHash32 {
    #[inline]
    pub fn hash(seed: u32, input: &[u8]) -> u32 {
        twox_hash::XxHash32::oneshot(seed, input)
    }
}

pub struct XxHash64;

impl XxHash64 {
    #[inline]
    pub fn hash(seed: u64, input: &[u8]) -> u64 {
        twox_hash::XxHash64::oneshot(seed, input)
    }
}

/// Streaming XxHash64 — used by the bundler's `ContentHasher`,
/// the dev-server source-map hash, and the resolver stat hash.
/// Output is bit-identical to `XxHash64::hash` of the concatenated input.
pub struct XxHash64Streaming(twox_hash::XxHash64);

impl XxHash64Streaming {
    #[inline]
    pub fn new(seed: u64) -> Self {
        Self(twox_hash::XxHash64::with_seed(seed))
    }

    #[inline]
    pub fn update(&mut self, bytes: &[u8]) {
        self.0.write(bytes);
    }

    #[inline]
    pub fn digest(&self) -> u64 {
        // twox-hash doesn't offer a const digest; clone and finish.
        let clone = self.0.clone();
        clone.finish()
    }
}

impl Default for XxHash64Streaming {
    #[inline]
    fn default() -> Self {
        Self::new(0)
    }
}

pub struct XxHash3;

impl XxHash3 {
    #[inline]
    pub fn hash(seed: u64, input: &[u8]) -> u64 {
        let mut hasher = twox_hash::xxhash3_64::Hasher::with_seed(seed);
        hasher.write(input);
        hasher.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── SMHasher verification (mirrors vendor/zig/lib/std/hash/verify.zig) ──────

    fn smhasher_32(hash: impl Fn(&[u8], u32) -> u32) -> u32 {
        let mut buf = [0u8; 256];
        let mut buf_all = [0u8; 256 * 4];
        for i in 0..256u32 {
            buf[i as usize] = i as u8;
            let h = hash(&buf[..i as usize], 256 - i);
            buf_all[i as usize * 4..i as usize * 4 + 4].copy_from_slice(&h.to_le_bytes());
        }
        hash(&buf_all, 0)
    }

    fn smhasher_64(hash: impl Fn(&[u8], u64) -> u64) -> u32 {
        let mut buf = [0u8; 256];
        let mut buf_all = [0u8; 256 * 8];
        for i in 0..256u64 {
            buf[i as usize] = i as u8;
            let h = hash(&buf[..i as usize], 256 - i);
            buf_all[i as usize * 8..i as usize * 8 + 8].copy_from_slice(&h.to_le_bytes());
        }
        hash(&buf_all, 0) as u32
    }

    // ── XxHash32 ───────────────────────────────────────────────────────────────

    #[test]
    fn xxhash32_smhasher() {
        let result = smhasher_32(|input, seed| XxHash32::hash(seed, input));
        assert_eq!(result, 0xBA88B743, "SMHasher verification for XXH32");
    }

    #[test]
    fn xxhash32_known_values() {
        assert_eq!(XxHash32::hash(0, b""), 0x02CC5D05);
        assert_eq!(XxHash32::hash(0, b"a"), 0x550D7456);
        assert_eq!(XxHash32::hash(0, b"abc"), 0x32D153FF);
    }

    #[test]
    fn xxhash32_seeded() {
        let a = XxHash32::hash(0, b"hello");
        let b = XxHash32::hash(42, b"hello");
        assert_ne!(a, b, "different seeds must produce different outputs");
    }

    // ── XxHash64 ───────────────────────────────────────────────────────────────

    #[test]
    fn xxhash64_smhasher() {
        let result = smhasher_64(|input, seed| XxHash64::hash(seed, input));
        assert_eq!(result, 0x024B7CF4, "SMHasher verification for XXH64");
    }

    #[test]
    fn xxhash64_known_values() {
        assert_eq!(XxHash64::hash(0, b""), 0xEF46DB3751D8E999);
        assert_eq!(XxHash64::hash(0, b"a"), 0xD24EC4F1A98C6E5B);
        assert_eq!(XxHash64::hash(0, b"abc"), 0x44BC2CF5AD770999);
    }

    #[test]
    fn xxhash64_seeded() {
        let a = XxHash64::hash(0, b"hello");
        let b = XxHash64::hash(42, b"hello");
        assert_ne!(a, b, "different seeds must produce different outputs");
    }

    // ── XxHash64 streaming ─────────────────────────────────────────────────────

    #[test]
    fn xxhash64_streaming_matches_oneshot() {
        let oneshot = XxHash64::hash(0, b"Hello, streaming world!");

        let mut streaming = XxHash64Streaming::new(0);
        streaming.update(b"Hello, ");
        streaming.update(b"streaming ");
        streaming.update(b"world!");
        assert_eq!(streaming.digest(), oneshot);
    }

    #[test]
    fn xxhash64_streaming_seeded() {
        let oneshot = XxHash64::hash(42, b"test data");

        let mut streaming = XxHash64Streaming::new(42);
        streaming.update(b"test data");
        assert_eq!(streaming.digest(), oneshot);
    }

    #[test]
    fn xxhash64_streaming_empty() {
        let oneshot = XxHash64::hash(0, b"");
        let streaming = XxHash64Streaming::new(0);
        assert_eq!(streaming.digest(), oneshot);
    }

    #[test]
    fn xxhash64_streaming_many_small_chunks() {
        let data: Vec<u8> = (0..1000).map(|i| (i % 256) as u8).collect();
        let oneshot = XxHash64::hash(0, &data);

        let mut streaming = XxHash64Streaming::new(0);
        for chunk in data.chunks(7) {
            streaming.update(chunk);
        }
        assert_eq!(streaming.digest(), oneshot);
    }

    #[test]
    fn xxhash64_streaming_default_seed() {
        let default = XxHash64Streaming::default().digest();
        let zero_seed = XxHash64Streaming::new(0).digest();
        assert_eq!(default, zero_seed, "default seed should be 0");
    }

    // ── XxHash3 ────────────────────────────────────────────────────────────────

    #[test]
    fn xxhash3_known_values() {
        // XXH3_64bits_withSeed reference test vectors from xxHash
        assert_eq!(XxHash3::hash(0, b""), 0x2D06800538D394C2);
    }

    #[test]
    fn xxhash3_seeded() {
        let a = XxHash3::hash(0, b"hello");
        let b = XxHash3::hash(42, b"hello");
        assert_ne!(a, b, "different seeds must produce different outputs");
    }

    #[test]
    fn xxhash3_large_input() {
        let data: Vec<u8> = (0..10_000).map(|i| (i % 256) as u8).collect();
        let h1 = XxHash3::hash(0, &data);
        let h2 = XxHash3::hash(0, &data);
        assert_eq!(h1, h2, "deterministic");
    }

    // ── Cross-validation: streaming digest matches oneshot ─────────────────────

    #[test]
    fn xxhash64_streaming_digest_idempotent() {
        let mut s = XxHash64Streaming::new(123);
        s.update(b"test");
        let first = s.digest();
        let second = s.digest();
        assert_eq!(first, second, "digest() should be idempotent (does not consume state)");
    }
}
