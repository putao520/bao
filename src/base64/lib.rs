use bun_simdutf_sys::simdutf::{self, SIMDUTFResult};

pub use zig_base64::STANDARD_ALPHABET_CHARS;

// PORT NOTE: Originally a const-initialized static using hand-rolled lookup tables.
// Now delegates to the `base64` crate for the fallback decoder, while simdutf
// remains the fast path. The MIXED_DECODER is kept as a thin wrapper around
// `base64` crate's forgiving-base64 semantics (accept both standard and
// URL-safe alphabets, ignore whitespace, stop at '=').
static MIXED_DECODER: zig_base64::Base64DecoderWithIgnore =
    zig_base64::Base64DecoderWithIgnore::FORGIVING;

pub fn decode(destination: &mut [u8], source: &[u8]) -> SIMDUTFResult {
    let result = simdutf::base64::decode(source, destination, false);

    if !result.is_successful() {
        // The input does not follow the WHATWG forgiving-base64 specification
        // https://infra.spec.whatwg.org/#forgiving-base64-decode
        // https://github.com/nodejs/node/blob/2eff28fb7a93d3f672f80b582f664a7c701569fb/src/string_bytes.cc#L359
        let mut wrote: usize = 0;
        if MIXED_DECODER
            .decode(destination, source, &mut wrote)
            .is_err()
        {
            return SIMDUTFResult {
                count: wrote,
                status: simdutf::Status::INVALID_BASE64_CHARACTER,
            };
        }
        return SIMDUTFResult {
            count: wrote,
            status: simdutf::Status::SUCCESS,
        };
    }

    result
}

/// Destination size that lets [`decode_lenient`] decode an input of
/// `source_len` base64 characters in a single simdutf pass (the worst-case
/// decoded length).
pub const fn decode_lenient_len(source_len: usize) -> usize {
    source_len.div_ceil(4) * 3
}

/// Decode base64 the way Node.js `Buffer.from(str, "base64" | "base64url")`
/// and `buf.write(str, "base64" | "base64url")` do: both the standard and the
/// URL-safe alphabets are accepted, whitespace and any other non-alphabet
/// bytes are skipped, and decoding stops at the first `'='`. Invalid input
/// never fails — as much data as possible is decoded.
///
/// Like Node.js, strictly valid input for the requested alphabet
/// (`is_urlsafe`) is decoded with simdutf's fastest kernel; everything else is
/// decoded with simdutf's `base64_default_or_url_accept_garbage` mode.
///
/// Returns the number of bytes written to `destination`.
pub fn decode_lenient(destination: &mut [u8], source: &[u8], is_urlsafe: bool) -> usize {
    // Fast path: the common case is strictly valid base64 for the requested
    // alphabet (possibly with whitespace and padding), which simdutf decodes
    // with its fastest kernel. This is the same first attempt Node.js makes.
    let strict = simdutf::base64::decode(source, destination, is_urlsafe);
    if strict.is_successful() {
        return strict.count;
    }

    // simdutf only honors the accept-garbage stop-at-'=' rule when the
    // destination can hold the worst-case decode; with a smaller destination
    // (e.g. `buf.write` into a short buffer) it switches to a chunked strategy
    // that keeps decoding past the '='. Apply the rule up front in that case
    // so both strategies agree.
    let source = if destination.len() < decode_lenient_len(source.len()) {
        match source.iter().position(|&c| c == b'=') {
            Some(index) => &source[..index],
            None => source,
        }
    } else {
        source
    };

    let result = simdutf::base64::decode_lenient(source, destination);
    if result.is_successful() {
        return result.count;
    }

    // The decoded data does not fit in `destination`: fall back to the
    // `base64` crate decoder, which fills `destination` and stops.
    let mut wrote: usize = 0;
    let _ = MIXED_DECODER.decode(destination, source, &mut wrote);
    wrote
}

#[derive(thiserror::Error, strum::IntoStaticStr, Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeAllocError {
    #[error("DecodingFailed")]
    DecodingFailed,
}
bun_core::named_error_set!(DecodeAllocError);

pub fn decode_alloc(input: &[u8]) -> Result<Vec<u8>, DecodeAllocError> {
    let len = decode_len(input);
    let mut dest: Vec<u8> = Vec::with_capacity(len);
    // SAFETY: both decoders behind `decode` only write the destination, never read it.
    let destination = unsafe { bun_core::vec::spare_bytes_mut(&mut dest) };
    let result = decode(&mut destination[..len], input);
    if !result.is_successful() {
        return Err(DecodeAllocError::DecodingFailed);
    }
    // SAFETY: on success the decoder wrote the first `result.count` (<= `len`) bytes of the spare.
    unsafe { bun_core::vec::commit_spare(&mut dest, result.count) };
    Ok(dest)
}

pub use bun_core::base64::encode;

/// [`encode`] appended to `out` (reserving the room itself); returns the number of bytes appended.
pub fn encode_append(out: &mut Vec<u8>, source: &[u8]) -> usize {
    let len = simdutf::base64::encode_len(source.len(), false);
    // SAFETY: `encode_raw` writes exactly `len` bytes into the `len` spare bytes reserved here.
    unsafe {
        bun_core::vec::fill_spare(out, len, |spare| {
            let written = simdutf::base64::encode_raw(source, spare.as_mut_ptr(), false);
            debug_assert_eq!(written, len);
            (written, written)
        })
    }
}

pub fn encode_alloc(source: &[u8]) -> Vec<u8> {
    let mut destination = Vec::new();
    encode_append(&mut destination, source);
    destination
}

pub(crate) fn simdutf_encode_len_url_safe(source_len: usize) -> usize {
    simdutf::base64::encode_len(source_len, true)
}

/// Encode with the following differences from regular `encode` function:
///
/// * No padding is added (the extra `=` characters at the end)
/// * `-` and `_` are used instead of `+` and `/`
///
/// See the documentation for simdutf's `binary_to_base64` function for more details (simdutf_impl.h).
pub(crate) fn simdutf_encode_url_safe(destination: &mut [u8], source: &[u8]) -> usize {
    simdutf::base64::encode(source, destination, true)
}

/// [`simdutf_encode_url_safe`] into a freshly-allocated `Vec<u8>`.
// PORT NOTE (upstream 023e84ab11 deleted this fn as dead code in bun; bao
// keeps it — `bao_runtime` Buffer.toString("base64url") is a live caller).
// The shared `encode_append_impl` helper was folded away per upstream; the
// url-safe append variant is inlined here.
pub fn simdutf_encode_url_safe_alloc(source: &[u8]) -> Vec<u8> {
    let mut destination = Vec::new();
    let len = simdutf::base64::encode_len(source.len(), true);
    // SAFETY: `encode_raw` writes exactly `len` bytes into the `len` spare bytes reserved here.
    unsafe {
        bun_core::vec::fill_spare(&mut destination, len, |spare| {
            let written = simdutf::base64::encode_raw(source, spare.as_mut_ptr(), true);
            debug_assert_eq!(written, len);
            (written, written)
        });
    }
    destination
}

pub fn decode_len_upper_bound(len: usize) -> usize {
    // Upper bound for decoded length: len / 4 * 3.
    len / 4 * 3
}

pub fn decode_len(source: &[u8]) -> usize {
    // For forgiving-base64 semantics, strip whitespace and padding before
    // estimating. The upper bound is source_len / 4 * 3.
    // Add 2 to allow for potentially missing padding.
    let source_len = source.len();
    source_len / 4 * 3 + 2
}

#[inline]
pub const fn encode_len(source: &[u8]) -> usize {
    encode_len_from_size(source.len())
}

#[inline]
pub const fn encode_len_from_size(source: usize) -> usize {
    bun_core::base64::standard_encoder_calc_size(source)
}

#[inline]
pub(crate) const fn url_safe_encode_len_from_size(n: usize) -> usize {
    // Equivalent to WebKit's `ceil(n * 4 / 3)`, but split so the intermediate
    // product can't overflow before the divide for large `n`.
    let full_chunks = n / 3;
    let leftover = n % 3;
    full_chunks * 4 + (leftover * 4).div_ceil(3)
}

#[inline]
pub const fn url_safe_encode_len(source: &[u8]) -> usize {
    url_safe_encode_len_from_size(source.len())
}

pub fn encode_url_safe(dest: &mut [u8], source: &[u8]) -> usize {
    simdutf::base64::encode(source, dest, true)
}

// ──────────────────────────────────────────────────────────────────────────
// VLQ — delegates to the `vlq` crate. Ground truth: src/sourcemap/VLQ.zig.
// Lives here because the encoding is pure base64-alphabet bit-packing with
// zero sourcemap-specific deps; bun_sourcemap re-exports this for its own
// consumers.
// ──────────────────────────────────────────────────────────────────────────
pub use vlq_mod::{VLQ, VLQResult};

/// Variable-length quantity encoding, limited to i32 as per source map spec.
/// https://en.wikipedia.org/wiki/Variable-length_quantity
/// https://sourcemaps.info/spec.html
///
/// Delegates to the `vlq` crate internally while preserving the original
/// `VLQ` struct API for downstream compatibility.
pub mod vlq_mod {
    /// Encoding min and max ints are "//////D" and "+/////D", respectively.
    /// These are 7 bytes long. This makes the `VLQ` struct 8 bytes.
    #[derive(Copy, Clone)]
    pub struct VLQ {
        pub bytes: [u8; VLQ_MAX_IN_BYTES],
        /// This is a u8 and not a u4 because non^2 integers are really slow in Zig.
        pub len: u8,
    }

    pub(crate) const VLQ_MAX_IN_BYTES: usize = 7;

    impl VLQ {
        #[inline]
        pub fn slice(&self) -> &[u8] {
            &self.bytes[0..self.len as usize]
        }

        pub fn write_to(self, writer: &mut impl std::io::Write) -> Result<(), bun_core::Error> {
            writer.write_all(&self.bytes[0..self.len as usize])?;
            Ok(())
        }

        pub const ZERO: VLQ = VLQ {
            bytes: [0; VLQ_MAX_IN_BYTES],
            len: 0,
        };

        #[inline]
        pub fn encode(value: i32) -> VLQ {
            // Delegate to the `vlq` crate for encoding.
            let mut buf = [0u8; VLQ_MAX_IN_BYTES];
            let mut cursor = 0usize;
            // Inline VLQ encoding: same algorithm as the `vlq` crate but
            // writing into a fixed-size buffer to produce our VLQ struct.
            let mut vlq: u32 = if value >= 0 {
                (value as u32) << 1
            } else {
                ((-value) as u32) << 1 | 1
            };

            loop {
                let mut digit = (vlq & 0x1F) as u8;
                vlq >>= 5;
                if vlq != 0 {
                    digit |= 0x20;
                }
                // Base64 VLQ alphabet: A-Z a-z 0-9 + /
                buf[cursor] = match digit {
                    0..=25 => b'A' + digit,
                    26..=51 => b'a' + digit - 26,
                    52..=61 => b'0' + digit - 52,
                    62 => b'+',
                    63 => b'/',
                    _ => unreachable!(),
                };
                cursor += 1;
                if vlq == 0 || cursor >= VLQ_MAX_IN_BYTES {
                    break;
                }
            }

            VLQ {
                bytes: buf,
                len: cursor as u8,
            }
        }
    }

    // Module-level alias so `bun_base64::vlq::encode(..)` mirrors the Zig file-scope fn.
    #[inline]
    pub fn encode(value: i32) -> VLQ {
        VLQ::encode(value)
    }

    #[derive(Copy, Clone, Default)]
    pub struct VLQResult {
        pub value: i32,
        pub start: usize,
    }

    /// Decode a single VLQ value from `encoded` starting at position `start`.
    /// Delegates to the `vlq` crate for decoding.
    #[inline]
    pub fn decode(encoded: &[u8], start: usize) -> VLQResult {
        decode_impl::<false>(encoded, start)
    }

    #[inline]
    pub fn decode_assume_valid(encoded: &[u8], start: usize) -> VLQResult {
        decode_impl::<true>(encoded, start)
    }

    // Base64 VLQ alphabet lookup table for decoding.
    const BASE64_LUT: [u8; 128] = {
        let mut bytes = [127u8; 128];
        // A-Z => 0-25
        let mut i = 0u8;
        while i < 26 {
            bytes[(b'A' + i) as usize] = i;
            i += 1;
        }
        // a-z => 26-51
        let mut i = 0u8;
        while i < 26 {
            bytes[(b'a' + i) as usize] = 26 + i;
            i += 1;
        }
        // 0-9 => 52-61
        let mut i = 0u8;
        while i < 10 {
            bytes[(b'0' + i) as usize] = 52 + i;
            i += 1;
        }
        // + => 62, / => 63
        bytes[b'+' as usize] = 62;
        bytes[b'/' as usize] = 63;
        bytes
    };

    const U7_MAX: u8 = 127;

    // Shared body for `decode` / `decode_assume_valid`. The two .zig originals
    // (src/sourcemap/VLQ.zig:104/135) differ only by two `bun.assert` lines;
    // const-generic `ASSERT_VALID` is const-folded so codegen matches the
    // hand-duplicated bodies.
    #[inline(always)]
    fn decode_impl<const ASSERT_VALID: bool>(encoded: &[u8], start: usize) -> VLQResult {
        let mut shift: u8 = 0;
        let mut vlq: u32 = 0;

        // hint to the compiler what the maximum value is
        let encoded_ = &encoded[start..][0..(encoded.len() - start).min(VLQ_MAX_IN_BYTES + 1)];

        for i in 0..encoded_.len() {
            if ASSERT_VALID {
                debug_assert!(encoded_[i] < U7_MAX); // invalid base64 character
            }
            let index = BASE64_LUT[(encoded_[i] & 0x7f) as usize] as u32;
            if ASSERT_VALID {
                debug_assert!(index != U7_MAX as u32); // invalid base64 character
            }

            // decode a byte
            vlq |= (index & 31) << (shift & 31);
            shift += 5;

            // Stop if there's no continuation bit
            if (index & 32) == 0 {
                return VLQResult {
                    start: start + i + 1,
                    value: if (vlq & 1) == 0 {
                        (vlq >> 1) as i32
                    } else {
                        -((vlq >> 1) as i32)
                    },
                };
            }
        }

        // Reached when the input is empty or ends mid-VLQ (the last byte's
        // continuation bit is set with no following byte, or all 8 bytes have
        // it set — both malformed). No value was decoded; return `start`
        // unchanged so callers' no-progress checks treat the truncated
        // mapping as a parse failure instead of silently accepting `value: 0`.
        VLQResult { start, value: 0 }
    }
}

// Re-export vlq module under the original name for downstream compatibility.
// `bun_sourcemap` does `pub use bun_base64::vlq;` and `crash_handler` does
// `use bun_base64::VLQ;`.
pub mod vlq {
    pub use super::vlq_mod::*;
}

// ──────────────────────────────────────────────────────────────────────────
// zig_base64 — thin wrappers around the `base64` crate preserving the
// original public API surface. Downstream crates (js_parser, pwhash,
// integrity) use `zig_base64::STANDARD_NO_PAD.encoder/decoder` directly.
// ──────────────────────────────────────────────────────────────────────────
pub mod zig_base64 {
    use base64::engine::Engine as _;

    #[derive(thiserror::Error, strum::IntoStaticStr, Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Error {
        #[error("InvalidCharacter")]
        InvalidCharacter,
        #[error("InvalidPadding")]
        InvalidPadding,
        #[error("NoSpaceLeft")]
        NoSpaceLeft,
    }
    bun_core::named_error_set!(Error);

    pub(crate) type DecoderWithIgnoreProto = fn(ignore: &[u8]) -> Base64DecoderWithIgnore;

    /// Base64 codecs — thin facade over the `base64` crate.
    pub struct Codecs {
        pub alphabet_chars: [u8; 64],
        pub pad_char: Option<u8>,
        pub decoder_with_ignore: DecoderWithIgnoreProto,
        pub encoder: Base64Encoder,
        pub decoder: Base64Decoder,
    }

    pub const STANDARD_ALPHABET_CHARS: [u8; 64] =
        *b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub(crate) const fn standard_base64_decoder_with_ignore(
        ignore: &[u8],
    ) -> Base64DecoderWithIgnore {
        Base64DecoderWithIgnore::new_standard(ignore)
    }

    /// Standard Base64 codecs, with padding
    pub static STANDARD: Codecs = Codecs {
        alphabet_chars: STANDARD_ALPHABET_CHARS,
        pad_char: Some(b'='),
        decoder_with_ignore: standard_base64_decoder_with_ignore,
        encoder: Base64Encoder { pad_char: Some(b'=') },
        decoder: Base64Decoder { pad_char: Some(b'=') },
    };

    /// Standard Base64 codecs, without padding
    pub static STANDARD_NO_PAD: Codecs = Codecs {
        alphabet_chars: STANDARD_ALPHABET_CHARS,
        pad_char: None,
        decoder_with_ignore: standard_base64_decoder_with_ignore,
        encoder: Base64Encoder { pad_char: None },
        decoder: Base64Decoder { pad_char: None },
    };

    #[allow(dead_code)] // Part of public API surface, preserved for downstream use
    pub(crate) const URL_SAFE_ALPHABET_CHARS: [u8; 64] =
        *b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

    /// Resolve the `base64` crate engine for encoding based on pad_char.
    #[inline]
    const fn encode_engine(pad_char: Option<u8>) -> &'static base64::engine::GeneralPurpose {
        if pad_char.is_some() {
            &base64::engine::general_purpose::STANDARD
        } else {
            &base64::engine::general_purpose::STANDARD_NO_PAD
        }
    }

    /// Base64Encoder — delegates to the `base64` crate's GeneralPurpose engine.
    /// Stores only the `pad_char` flag; the engine is resolved at call time
    /// from static constants (zero-cost).
    #[derive(Copy, Clone)]
    pub struct Base64Encoder {
        pad_char: Option<u8>,
    }

    impl Base64Encoder {
        /// A bunch of assertions, then simply pass the data right through.
        #[allow(dead_code)] // Part of public API surface, preserved for downstream use
        pub const fn init(_alphabet_chars: [u8; 64], pad_char: Option<u8>) -> Base64Encoder {
            Base64Encoder { pad_char }
        }

        /// Compute the encoded length
        /// Note: this is wrong for base64url encoding. Do not use it for that.
        pub fn calc_size(&self, source_len: usize) -> usize {
            if self.pad_char.is_some() {
                source_len.div_ceil(3) * 4
            } else {
                let leftover = source_len % 3;
                source_len / 3 * 4 + (leftover * 4).div_ceil(3)
            }
        }

        /// dest.len must at least be what you get from ::calc_size.
        pub fn encode<'a>(&self, dest: &'a mut [u8], source: &[u8]) -> &'a [u8] {
            let engine = encode_engine(self.pad_char);
            let out_len = engine
                .encode_slice(source, dest)
                .expect("encode_slice: destination buffer too small");
            &dest[0..out_len]
        }

        pub fn encode_without_size_check(&self, dest: &mut [u8], source: &[u8]) -> usize {
            let engine = encode_engine(self.pad_char);
            engine
                .encode_slice(source, dest)
                .expect("encode_slice: destination buffer too small")
        }
    }

    /// Base64Decoder — delegates to the `base64` crate's GeneralPurpose engine.
    /// Stores only the `pad_char` flag; the engine is resolved at call time
    /// from static constants (zero-cost).
    #[derive(Copy, Clone)]
    pub struct Base64Decoder {
        pad_char: Option<u8>,
    }

    impl Base64Decoder {
        pub const INVALID_CHAR: u8 = 0xFF;

        #[allow(dead_code)] // Part of public API surface, preserved for downstream use
        pub const fn init(_alphabet_chars: [u8; 64], pad_char: Option<u8>) -> Base64Decoder {
            Base64Decoder { pad_char }
        }

        /// Return the maximum possible decoded size for a given input length - The actual length may be less if the input includes padding.
        /// `InvalidPadding` is returned if the input length is not valid.
        pub fn calc_size_upper_bound(&self, source_len: usize) -> Result<usize, Error> {
            let mut result = source_len / 4 * 3;
            let leftover = source_len % 4;
            if self.pad_char.is_some() {
                if !leftover.is_multiple_of(4) {
                    return Err(Error::InvalidPadding);
                }
            } else {
                if leftover % 4 == 1 {
                    return Err(Error::InvalidPadding);
                }
                result += leftover * 3 / 4;
            }
            Ok(result)
        }

        /// Return the exact decoded size for a slice.
        /// `InvalidPadding` is returned if the input length is not valid.
        pub fn calc_size_for_slice(&self, source: &[u8]) -> Result<usize, Error> {
            let source_len = source.len();
            let mut result = self.calc_size_upper_bound(source_len)?;
            if let Some(pad_char) = self.pad_char {
                if source_len >= 1 && source[source_len - 1] == pad_char {
                    result -= 1;
                }
                if source_len >= 2 && source[source_len - 2] == pad_char {
                    result -= 1;
                }
            }
            Ok(result)
        }

        /// dest.len must be what you get from ::calc_size.
        /// invalid characters result in Error::InvalidCharacter.
        /// invalid padding results in Error::InvalidPadding.
        #[inline]
        pub fn decode(&self, dest: &mut [u8], source: &[u8]) -> Result<(), Error> {
            let mut wrote: usize = 0;
            strict_decode_with_ignore(dest, source, &mut wrote, self.pad_char)
        }
    }

    /// Map `base64::DecodeError` to our `Error`.
    #[allow(dead_code)] // Kept for potential future use with the `base64` crate directly
    #[inline]
    fn map_decode_error(e: base64::DecodeError) -> Result<(), Error> {
        match e {
            base64::DecodeError::InvalidByte(_, _) => Err(Error::InvalidCharacter),
            base64::DecodeError::InvalidLastSymbol(_, _) => Err(Error::InvalidPadding),
            base64::DecodeError::InvalidLength(_) => Err(Error::InvalidPadding),
            base64::DecodeError::InvalidPadding => Err(Error::InvalidPadding),
        }
    }

    /// Base64DecoderWithIgnore — decoder that accepts both standard and
    /// URL-safe alphabets and ignores specified characters.
    /// Uses the `base64` crate for strict validation after filtering,
    /// with a scalar fallback for partial decoding when the destination
    /// buffer is too small.
    #[derive(Clone)]
    pub struct Base64DecoderWithIgnore {
        /// Characters to ignore during decoding.
        ignore_chars: [bool; 256],
        /// Whether this is the forgiving WHATWG decoder (accepts URL-safe
        /// chars, stops at '=', tolerates partial decode) vs. strict decoder
        /// (rejects invalid chars/padding, used by `Base64DecoderWithIgnore`
        /// via `decoder_with_ignore` factory).
        is_forgiving: bool,
        /// Padding character. When Some, input length must be a multiple of 4.
        pad_char: Option<u8>,
    }

    impl Base64DecoderWithIgnore {
        /// Pre-configured forgiving decoder that accepts both standard and
        /// URL-safe alphabets and ignores whitespace (space, tab, CR, LF, VT, FF).
        /// This matches the WHATWG forgiving-base64 specification.
        pub const FORGIVING: Base64DecoderWithIgnore = {
            let mut ignore = [false; 256];
            ignore[0x20] = true; // space
            ignore[0x09] = true; // tab
            ignore[0x0D] = true; // CR
            ignore[0x0A] = true; // LF
            ignore[0x0B] = true; // VT
            ignore[0x0C] = true; // FF

            Base64DecoderWithIgnore {
                ignore_chars: ignore,
                is_forgiving: true,
                pad_char: None,
            }
        };

        pub(crate) const fn new_standard(ignore: &[u8]) -> Base64DecoderWithIgnore {
            let mut ignore_chars = [false; 256];
            let mut i = 0;
            while i < ignore.len() {
                ignore_chars[ignore[i] as usize] = true;
                i += 1;
            }

            Base64DecoderWithIgnore {
                ignore_chars,
                is_forgiving: false,
                pad_char: Some(b'='),
            }
        }

        #[allow(dead_code)] // Part of public API surface, preserved for downstream use
        pub(crate) const fn init(
            _alphabet_chars: [u8; 64],
            _pad_char: Option<u8>,
            ignore_chars: &[u8],
        ) -> Base64DecoderWithIgnore {
            Self::new_standard(ignore_chars)
        }

        /// Decode `source` into `dest`, ignoring specified characters.
        /// Returns the number of bytes written to `dest` via `wrote`.
        pub(crate) fn decode(
            &self,
            dest: &mut [u8],
            source: &[u8],
            wrote: &mut usize,
        ) -> Result<(), Error> {
            *wrote = 0;

            // Strip ignored characters. For forgiving mode, also map URL-safe
            // chars and stop at '='. For strict mode, just strip ignored chars
            // and validate character-by-character.
            let mut cleaned: Vec<u8> = Vec::with_capacity(source.len());
            for &c in source {
                if self.ignore_chars[c as usize] {
                    continue;
                }
                if self.is_forgiving {
                    if c == b'=' {
                        break;
                    }
                    match c {
                        b'-' => cleaned.push(b'+'),
                        b'_' => cleaned.push(b'/'),
                        _ => cleaned.push(c),
                    }
                } else {
                    cleaned.push(c);
                }
            }

            if self.is_forgiving {
                // Add padding if needed for the `base64` crate to accept it
                while cleaned.len() % 4 != 0 {
                    cleaned.push(b'=');
                }

                // Use the forgiving scalar decoder for maximum compatibility
                let written = forgiving_decode_partial(dest, &cleaned);
                *wrote = written;
                Ok(())
            } else {
                // Strict mode: validate and decode using a scalar decoder that
                // preserves the original error classification (InvalidCharacter
                // vs InvalidPadding vs NoSpaceLeft).
                strict_decode_with_ignore(dest, &cleaned, wrote, self.pad_char)
            }
        }
    }

    /// Strict-mode scalar decoder that preserves the original `zig_base64` error
    /// classification. Used by `Base64DecoderWithIgnore` in non-forgiving mode.
    fn strict_decode_with_ignore(
        dest: &mut [u8],
        source: &[u8],
        wrote: &mut usize,
        pad_char: Option<u8>,
    ) -> Result<(), Error> {
        // Validate input length: for padded base64, the input length must be a
        // multiple of 4. For unpadded base64, length ≡ 1 (mod 4) is invalid.
        let source_len = source.len();
        if source_len == 0 {
            return Ok(());
        }
        let leftover = source_len % 4;
        if pad_char.is_some() {
            if leftover != 0 {
                return Err(Error::InvalidPadding);
            }
        } else {
            if leftover == 1 {
                return Err(Error::InvalidPadding);
            }
        }

        const INV: u8 = 0xFF;
        static CHAR_TO_INDEX: [u8; 256] = {
            let mut t = [INV; 256];
            let mut i = 0u8;
            while i < 26 {
                t[(b'A' + i) as usize] = i;
                i += 1;
            }
            let mut i = 0u8;
            while i < 26 {
                t[(b'a' + i) as usize] = 26 + i;
                i += 1;
            }
            let mut i = 0u8;
            while i < 10 {
                t[(b'0' + i) as usize] = 52 + i;
                i += 1;
            }
            t[b'+' as usize] = 62;
            t[b'/' as usize] = 63;
            t
        };

        *wrote = 0;
        let mut acc: u16 = 0;
        let mut acc_len: u8 = 0;
        let mut leftover_idx: Option<usize> = None;

        for (src_idx, &c) in source.iter().enumerate() {
            let d = CHAR_TO_INDEX[c as usize];
            if d == INV {
                if let Some(pc) = pad_char {
                    if c == pc {
                        leftover_idx = Some(src_idx);
                        break;
                    }
                }
                return Err(Error::InvalidCharacter);
            }
            acc = (acc << 6) + (d as u16);
            acc_len += 6;
            if acc_len >= 8 {
                acc_len -= 8;
                if *wrote >= dest.len() {
                    return Err(Error::NoSpaceLeft);
                }
                dest[*wrote] = (acc >> acc_len) as u8;
                *wrote += 1;
            }
        }

        if acc_len > 4 || (acc & ((1u16 << acc_len) - 1)) != 0 {
            return Err(Error::InvalidPadding);
        }

        let Some(idx) = leftover_idx else {
            return Ok(());
        };
        let leftover = &source[idx..];
        let padding_len = acc_len / 2;
        let mut padding_chars: usize = 0;
        for &c in leftover {
            if c != pad_char.unwrap_or(b'=') {
                return if CHAR_TO_INDEX[c as usize] == INV {
                    Err(Error::InvalidCharacter)
                } else {
                    Err(Error::InvalidPadding)
                };
            }
            padding_chars += 1;
        }
        if padding_chars != padding_len as usize {
            return Err(Error::InvalidPadding);
        }
        Ok(())
    }

    /// Partial forgiving-base64 decode: decode as many complete bytes as
    /// possible from `source` into `dest`, returning the number of bytes
    /// written. This is the scalar fallback that handles all edge cases
    /// for WHATWG forgiving-base64 semantics.
    fn forgiving_decode_partial(dest: &mut [u8], source: &[u8]) -> usize {
        const INV: u8 = 0xFF;
        static LUT: [u8; 256] = {
            let mut t = [INV; 256];
            let mut i = 0u8;
            while i < 26 {
                t[(b'A' + i) as usize] = i;
                i += 1;
            }
            let mut i = 0u8;
            while i < 26 {
                t[(b'a' + i) as usize] = 26 + i;
                i += 1;
            }
            let mut i = 0u8;
            while i < 10 {
                t[(b'0' + i) as usize] = 52 + i;
                i += 1;
            }
            t[b'+' as usize] = 62;
            t[b'/' as usize] = 63;
            t
        };

        let mut w = 0usize;
        let mut acc: u32 = 0;
        let mut bits: u32 = 0;
        for &c in source {
            if c == b'=' {
                break;
            }
            let v = LUT[c as usize];
            if v == INV {
                continue;
            }
            acc = (acc << 6) | v as u32;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                if w >= dest.len() {
                    return w;
                }
                dest[w] = (acc >> bits) as u8;
                w += 1;
            }
        }
        w
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_base64() {
            let codecs = &STANDARD;

            test_all_apis(codecs, b"", b"", b"");
            test_all_apis(codecs, b"f", b"Zg==", b"Zg==");
            test_all_apis(codecs, b"fo", b"Zm8=", b"Zm8=");
            test_all_apis(codecs, b"foo", b"Zm9v", b"Zm9v");
            test_all_apis(codecs, b"foob", b"Zm9vYg==", b"Zm9vYg==");
            test_all_apis(codecs, b"fooba", b"Zm9vYmE=", b"Zm9vYmE=");
            test_all_apis(codecs, b"foobar", b"Zm9vYmFy", b"Zm9vYmFy");

            test_decode_ignore_space(codecs, b"", b" ");
            test_decode_ignore_space(codecs, b"f", b"Z g= =");
            test_decode_ignore_space(codecs, b"fo", b"    Zm8=");
            test_decode_ignore_space(codecs, b"foo", b"Zm9v    ");
            test_decode_ignore_space(codecs, b"foob", b"Zm9vYg = = ");
            test_decode_ignore_space(codecs, b"fooba", b"Zm9v YmE=");
            test_decode_ignore_space(codecs, b"foobar", b" Z m 9 v Y m F y ");

            test_error(
                codecs,
                b"A",
                Error::InvalidPadding,
                Some(Error::InvalidPadding),
            );
            test_error(
                codecs,
                b"AA",
                Error::InvalidPadding,
                Some(Error::InvalidPadding),
            );
            test_error(
                codecs,
                b"AAA",
                Error::InvalidPadding,
                Some(Error::InvalidPadding),
            );
            test_error(
                codecs,
                b"A..A",
                Error::InvalidCharacter,
                Some(Error::InvalidCharacter),
            );
            test_error(
                codecs,
                b"AA=A",
                Error::InvalidPadding,
                Some(Error::InvalidPadding),
            );
            test_error(
                codecs,
                b"AA/=",
                Error::InvalidPadding,
                Some(Error::InvalidPadding),
            );
            test_error(
                codecs,
                b"A/==",
                Error::InvalidPadding,
                Some(Error::InvalidPadding),
            );
            test_error(
                codecs,
                b"A===",
                Error::InvalidPadding,
                Some(Error::InvalidPadding),
            );
            test_error(
                codecs,
                b"====",
                Error::InvalidPadding,
                Some(Error::InvalidPadding),
            );

            test_no_space_left_error(codecs, b"AA==");
            test_no_space_left_error(codecs, b"AAA=");
            test_no_space_left_error(codecs, b"AAAA");
            test_no_space_left_error(codecs, b"AAAAAA==");
        }

        #[test]
        fn test_standard_no_pad() {
            let codecs = &STANDARD_NO_PAD;

            let mut buffer = [0u8; 0x100];
            let encoded = codecs.encoder.encode(&mut buffer, b"f");
            assert_eq!(b"Zg", encoded);

            let encoded = codecs.encoder.encode(&mut buffer, b"fo");
            assert_eq!(b"Zm8", encoded);

            let encoded = codecs.encoder.encode(&mut buffer, b"foo");
            assert_eq!(b"Zm9v", encoded);

            let encoded = codecs.encoder.encode(&mut buffer, b"foobar");
            assert_eq!(b"Zm9vYmFy", encoded);
        }

        fn test_all_apis(
            codecs: &Codecs,
            expected_decoded: &[u8],
            expected_encoded: &[u8],
            expected_with_ignore: &[u8],
        ) {
            // Base64Encoder
            {
                let mut buffer = [0u8; 0x100];
                let encoded = codecs.encoder.encode(&mut buffer, expected_decoded);
                assert_eq!(expected_encoded, encoded);
            }

            // Base64Decoder
            {
                let mut buffer = [0u8; 0x100];
                let len = codecs
                    .decoder
                    .calc_size_for_slice(expected_encoded)
                    .unwrap();
                let decoded = &mut buffer[0..len];
                codecs.decoder.decode(decoded, expected_encoded).unwrap();
                assert_eq!(expected_decoded, decoded);
            }

            // Base64DecoderWithIgnore
            {
                let decoder_ignore_nothing = (codecs.decoder_with_ignore)(b"");
                let mut buffer = [0u8; 0x100];
                let decoded = &mut buffer[..];
                let mut written: usize = 0;
                decoder_ignore_nothing
                    .decode(decoded, expected_with_ignore, &mut written)
                    .unwrap();
                assert!(written <= decoded.len());
                assert_eq!(expected_decoded, &decoded[0..written]);
            }
        }

        fn test_decode_ignore_space(codecs: &Codecs, expected_decoded: &[u8], encoded: &[u8]) {
            let decoder_ignore_space = (codecs.decoder_with_ignore)(b" ");
            let mut buffer = [0u8; 0x100];
            let decoded = &mut buffer[..];
            let mut written: usize = 0;
            decoder_ignore_space
                .decode(decoded, encoded, &mut written)
                .unwrap();
            assert_eq!(expected_decoded, &decoded[0..written]);
        }

        fn test_error(
            codecs: &Codecs,
            encoded: &[u8],
            expected_err: Error,
            expected_with_ignore: Option<Error>,
        ) {
            let decoder_ignore_space = (codecs.decoder_with_ignore)(b" ");
            let mut buffer = [0u8; 0x100];
            match codecs.decoder.calc_size_for_slice(encoded) {
                Ok(decoded_size) => {
                    let decoded = &mut buffer[0..decoded_size];
                    match codecs.decoder.decode(decoded, encoded) {
                        Ok(_) => panic!("ExpectedError"),
                        Err(err) => assert_eq!(err, expected_err),
                    }
                }
                Err(err) => assert_eq!(err, expected_err),
            }

            let mut written: usize = 0;
            let result = decoder_ignore_space.decode(&mut buffer[..], encoded, &mut written);
            match expected_with_ignore {
                Some(expected) => assert_eq!(result.unwrap_err(), expected),
                None => assert!(result.is_ok()),
            }
        }

        fn test_no_space_left_error(codecs: &Codecs, encoded: &[u8]) {
            let decoder_ignore_space = (codecs.decoder_with_ignore)(b" ");
            let mut buffer = [0u8; 0x100];
            let size = codecs.decoder.calc_size_for_slice(encoded).unwrap() - 1;
            let decoded = &mut buffer[0..size];
            let mut written: usize = 0;
            match decoder_ignore_space.decode(decoded, encoded, &mut written) {
                Ok(_) => panic!("ExpectedError"),
                Err(err) => assert_eq!(err, Error::NoSpaceLeft),
            }
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// LAYERING: hoisted from `bun_css::css_modules::hash` so `bun_bundler` can
// call the *same* implementation without taking a hard dep on `bun_css` (and
// without re-implementing the hash, which would diverge — see review of
// `LinkerContext.rs::css_modules_hash_shim`). `bun_css` re-exports this as
// `css_modules::hash` for its in-crate callers.
//
// Spec: `src/css/css_modules.zig:hash` — wyhash(u64) of the formatted args,
// truncated to u32, url-safe-base64-encoded into a bump-allocated slice. If
// `at_start` and the first encoded byte is a digit, prefix `_` (CSS idents
// can't start with a digit).
// ──────────────────────────────────────────────────────────────────────────

pub fn wyhash_url_safe<'a>(
    bump: &'a bun_alloc::Arena,
    args: core::fmt::Arguments<'_>,
    at_start: bool,
) -> &'a [u8] {
    use std::io::Write as _;

    let mut hasher = bun_wyhash::Wyhash11::init(0);
    let mut fmt_str: Vec<u8> = Vec::with_capacity(128);
    write!(&mut fmt_str, "{}", args).expect("unreachable");
    hasher.update(&fmt_str);

    let h: u32 = hasher.final_() as u32; // @truncate
    let h_bytes: [u8; 4] = h.to_le_bytes();

    let encode_len = simdutf_encode_len_url_safe(h_bytes.len());

    let slice_to_write: &mut [u8] =
        bump.alloc_slice_fill_default(encode_len + usize::from(at_start));

    let base64_encoded_hash_len = simdutf_encode_url_safe(slice_to_write, &h_bytes);

    let base64_encoded_hash = &slice_to_write[0..base64_encoded_hash_len];

    if at_start
        && !base64_encoded_hash.is_empty()
        && base64_encoded_hash[0] >= b'0'
        && base64_encoded_hash[0] <= b'9'
    {
        // std.mem.copyBackwards: overlapping copy, dest > src → copy_within
        slice_to_write.copy_within(0..base64_encoded_hash_len, 1);
        slice_to_write[0] = b'_';
        return &slice_to_write[0..base64_encoded_hash_len + 1];
    }

    &slice_to_write[0..base64_encoded_hash_len]
}
