#![allow(non_snake_case, non_camel_case_types, non_upper_case_globals)]
#![warn(unused_must_use)]
pub mod ETag;
pub mod Encoding;
pub mod FetchCacheMode;
pub mod FetchRedirect;
pub mod FetchRequestMode;
pub mod Method;
pub mod URLPath;
pub mod h2;
pub mod mime_type_list_enum;
pub use ETag::wtf;

// `mime_type_list_enum::MimeTypeList` is a hand-generated `&'static str`
// newtype (PERF(port) stand-in for the Zig packed-u14 table), so
// `Table`/`Compact`/`EXTENSIONS`/`sniff`/`from_table`/`create_hash_table`/`ALL`
// all compile. Only `by_loader` remains gated
// (same-tier `bun_ast::Loader`, intra-tier edge avoided).
pub mod MimeType;

/// `std.fmt.parseInt(usize, value, 10) catch 0` — RFC 9110 Content-Length is
/// `1*DIGIT`, so any parse failure (empty / non-digit / overflow) maps to 0.
/// Zig has no dedicated helper (http.zig:2800, server.zig:2442, etc. all inline
/// the `catch 0`); this wrapper gives the Rust port ONE call shape across
/// bun_http / bun_runtime::server / s3. Backed by the std.fmt.parseInt port.
#[inline]
pub fn parse_content_length(value: &[u8]) -> usize {
    bun_core::parse_int::<usize>(value, 10).unwrap_or(0)
}

/// RFC 9110 §8.6 `1*DIGIT`, strictly: `None` for anything else, or a
/// value that overflows `u64`. Distinct from [`parse_content_length`] (the
/// lenient `catch 0` fall-through non-framing call sites use): this is the
/// wire-grammar parser the framing decision must have — upstream bun
/// 83913e746a consolidates its inline copies onto this shape.
#[inline]
pub fn parse_content_length_strict(value: &[u8]) -> Option<u64> {
    if value.is_empty() {
        return None;
    }
    let mut n: u64 = 0;
    for &c in value {
        if !c.is_ascii_digit() {
            return None;
        }
        n = n.checked_mul(10)?.checked_add(u64::from(c - b'0'))?;
    }
    Some(n)
}

#[cfg(test)]
mod tests {
    // @trace TEST-ENG-007 [req:REQ-ENG-007] [level:unit] — upstream bun 83913e746a
    use super::parse_content_length_strict;

    #[test]
    fn strict_content_length_is_1digit_only() {
        assert_eq!(parse_content_length_strict(b"7"), Some(7));
        assert_eq!(parse_content_length_strict(b"0"), Some(0));
        assert_eq!(parse_content_length_strict(b"999999999999999999"), Some(999_999_999_999_999_999));
        // the grammar rejects everything that is not 1*DIGIT
        assert_eq!(parse_content_length_strict(b""), None);
        assert_eq!(parse_content_length_strict(b"+5"), None);
        assert_eq!(parse_content_length_strict(b"-1"), None);
        assert_eq!(parse_content_length_strict(b"0x10"), None);
        assert_eq!(parse_content_length_strict(b"5.0"), None);
        assert_eq!(parse_content_length_strict(b"abc"), None);
        assert_eq!(parse_content_length_strict(b"5, 7"), None);
        assert_eq!(parse_content_length_strict(b" 5"), None);
        assert_eq!(parse_content_length_strict(b"5 "), None);
        assert_eq!(parse_content_length_strict(b"7, 7"), None);
        assert_eq!(parse_content_length_strict(b"99999999999999999999"), None);
    }
}
