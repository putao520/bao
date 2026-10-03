use bun_core::err;
use bun_core::strings;

use crate::Encoding;

// PORT NOTE: Zig stored a `std.mem.TokenIterator(u8, .scalar)` field. The Rust
// equivalent (`slice::Split<'_, u8, _>` + `.filter(..)`) has an unnameable
// closure type, so we store the remaining input slice and inline the
// tokenize-by-',' logic in `next()`. Behavior is identical.
pub struct HeaderValueIterator<'a> {
    remaining: &'a [u8],
}

impl<'a> HeaderValueIterator<'a> {
    pub fn init(input: &'a [u8]) -> HeaderValueIterator<'a> {
        HeaderValueIterator {
            // std.mem.tokenizeScalar(u8, std.mem.trim(u8, input, " \t"), ',')
            remaining: strings::trim(input, b" \t"),
        }
    }

    pub fn next(&mut self) -> Option<&'a [u8]> {
        // tokenizeScalar semantics: skip leading delimiters, take until next delimiter.
        while let Some((&b',', rest)) = self.remaining.split_first() {
            self.remaining = rest;
        }
        if self.remaining.is_empty() {
            return None;
        }
        let end = self
            .remaining
            .iter()
            .position(|&b| b == b',')
            .unwrap_or(self.remaining.len());
        let token = &self.remaining[..end];
        self.remaining = &self.remaining[end..];

        let slice = strings::trim(token, b" \t");
        if slice.is_empty() {
            return self.next();
        }

        Some(slice)
    }
}

/// RFC 9112 §6.1: `chunked`, if present, must be the final coding. Called once per field line —
/// a coding after `chunked`, or an unknown token, is `UnsupportedTransferEncoding`.
/// (Upstream bun 83913e746a: shared by the response receiver and the stream-body
/// request producer; bao carries the receiver half — its fetch() relay is
/// fail-closed for stream bodies, so no producer half exists.)
pub fn fold_transfer_encoding(value: &[u8], coding: &mut Encoding) -> Result<(), bun_core::Error> {
    // PORT NOTE: bao's copy predates the upstream `Iterator` impl on
    // `HeaderValueIterator` — the explicit `next()` loop is the local idiom
    // (the `for` form upstream uses would not compile here).
    let mut tokens = HeaderValueIterator::init(value);
    while let Some(token) = tokens.next() {
        if *coding == Encoding::Chunked {
            return Err(err!(UnsupportedTransferEncoding));
        }
        match Encoding::from_token(token) {
            Some(Encoding::Chunked) => *coding = Encoding::Chunked,
            Some(_) => {}
            None => return Err(err!(UnsupportedTransferEncoding)),
        }
    }
    Ok(())
}

// ported from: src/http/HeaderValueIterator.zig

#[cfg(test)]
mod tests {
    // @trace TEST-ENG-007 [req:REQ-ENG-007] [level:unit] — upstream bun 83913e746a
    use super::{Encoding, HeaderValueIterator, fold_transfer_encoding};

    fn fold(value: &[u8]) -> Result<Encoding, bun_core::Error> {
        let mut coding = Encoding::Identity;
        fold_transfer_encoding(value, &mut coding)?;
        Ok(coding)
    }

    #[test]
    fn fold_accepts_rfc_9112_list_forms() {
        assert_eq!(fold(b"chunked"), Ok(Encoding::Chunked));
        assert_eq!(fold(b"Chunked"), Ok(Encoding::Chunked));
        assert_eq!(fold(b"gzip, chunked"), Ok(Encoding::Chunked));
        // a recipient skips empty list elements (RFC 9110 §5.6.1)
        assert_eq!(fold(b"gzip,, chunked"), Ok(Encoding::Chunked));
        assert_eq!(fold(b"\tchunked "), Ok(Encoding::Chunked));
        assert_eq!(fold(b"identity"), Ok(Encoding::Identity));
        // a known non-chunked coding selects nothing: the fold state stays
        // Identity (the receiver's downstream framing cares about chunked only)
        assert_eq!(fold(b"x-gzip"), Ok(Encoding::Identity));
    }

    #[test]
    fn fold_rejects_non_final_chunked_and_unknown_tokens() {
        // chunked must be the final coding
        assert!(fold(b"chunked, chunked").is_err());
        assert!(fold(b"chunked, gzip").is_err());
        // unknown coding token
        assert!(fold(b"br2, chunked").is_err());
        assert!(fold(b"gzip; q=1, chunked").is_err());
        assert!(fold(b"\"\"").is_err());
        // once chunked, a second fold on the same state (a repeated field
        // line) rejects
        let mut coding = Encoding::Chunked;
        assert!(fold_transfer_encoding(b"identity", &mut coding).is_err());
    }

    #[test]
    fn iterator_skips_empty_tokens_like_a_recipient() {
        let mut it = HeaderValueIterator::init(b"a,, b");
        assert_eq!(it.next(), Some(b"a" as &[u8]));
        assert_eq!(it.next(), Some(b"b" as &[u8]));
        assert_eq!(it.next(), None);
    }
}
