// @trace REQ-PURE-006
// Smoke tests for the pure-Rust highway replacement.

use bun_highway::{
    contains_newline_or_non_ascii_or_quote, copy_ascii_prefix, copy_u16_to_u8, decode_hex,
    decode_hex_u16, encode_hex_lower, fill_with_skip_mask, fill_with_skip_mask_inplace,
    index_of_any_char, index_of_char, index_of_interesting_character_in_multiline_comment,
    index_of_interesting_character_in_string_literal, index_of_needs_escape_for_javascript_string,
    index_of_newline_or_non_ascii, index_of_newline_or_non_ascii_or_hash_or_at,
    index_of_space_or_newline_or_non_ascii, scan_char_frequency,
};

#[test]
fn index_of_char_basic() {
    assert_eq!(index_of_char(b"hello world", b'o'), Some(4));
    assert_eq!(index_of_char(b"hello", b'z'), None);
    assert_eq!(index_of_char(b"", b'x'), None);
    assert_eq!(index_of_char(b"aaaa", b'a'), Some(0));
}

#[test]
fn index_of_any_char_dispatches_by_len() {
    assert_eq!(index_of_any_char(b"abcde", b"x"), None); // 'x' absent
    assert_eq!(index_of_any_char(b"abcde", b"xy"), None);
    assert_eq!(index_of_any_char(b"hello\rworld", b"\r\n"), Some(5));
    assert_eq!(index_of_any_char(b"abc", b"xyz"), None);
    assert_eq!(index_of_any_char(b"foo bar", b" "), Some(3));
    assert_eq!(index_of_any_char(b"abcdefg", b"xya"), Some(0));
}

#[test]
fn newline_or_non_ascii_finds_control_chars() {
    assert_eq!(index_of_newline_or_non_ascii(b"hello\nworld"), Some(5));
    assert_eq!(index_of_newline_or_non_ascii(b"\x01abc"), Some(0));
    assert_eq!(index_of_newline_or_non_ascii(b"plain ascii text only"), None);
    assert_eq!(index_of_newline_or_non_ascii(b"foo\xC3\xA9"), Some(3)); // 0xC3 > 127
}

#[test]
fn contains_quote_or_newline() {
    assert!(contains_newline_or_non_ascii_or_quote(b"he said \"hi\""));
    assert!(contains_newline_or_non_ascii_or_quote(b"line\nbreak"));
    assert!(contains_newline_or_non_ascii_or_quote(b"accentu\xC3\xA9"));
    assert!(!contains_newline_or_non_ascii_or_quote(b"plain ascii"));
    assert!(!contains_newline_or_non_ascii_or_quote(b""));
}

#[test]
fn needs_escape_for_js_string() {
    assert_eq!(
        index_of_needs_escape_for_javascript_string(b"hello\\world", b'"'),
        Some(5),
    );
    assert_eq!(
        index_of_needs_escape_for_javascript_string(b"line\nbreak", b'"'),
        Some(4),
    );
    assert_eq!(
        index_of_needs_escape_for_javascript_string(b"plain text", b'"'),
        None,
    );
    // Quote character itself must be escaped.
    assert_eq!(
        index_of_needs_escape_for_javascript_string(b"say \"hi\"", b'"'),
        Some(4),
    );
}

#[test]
fn multiline_comment_interesting_chars() {
    assert_eq!(
        index_of_interesting_character_in_multiline_comment(b"this is a comment */"),
        Some(18),
    );
    assert_eq!(
        index_of_interesting_character_in_multiline_comment(b"abc\ndef"),
        Some(3),
    );
    assert_eq!(
        index_of_interesting_character_in_multiline_comment(b"plain comment text"),
        None,
    );
}

#[test]
fn string_literal_interesting_chars() {
    // backslash before quote.
    assert_eq!(
        index_of_interesting_character_in_string_literal(b"plain\\escape", b'"'),
        Some(5),
    );
    assert_eq!(
        index_of_interesting_character_in_string_literal(b"line\nbreak", b'"'),
        Some(4),
    );
    assert_eq!(
        index_of_interesting_character_in_string_literal(b"plain text", b'"'),
        None,
    );
}

#[test]
fn space_or_newline_or_non_ascii_scan() {
    assert_eq!(index_of_space_or_newline_or_non_ascii(b"hello world"), Some(5));
    assert_eq!(index_of_space_or_newline_or_non_ascii(b"hello\nworld"), Some(5));
    assert_eq!(index_of_space_or_newline_or_non_ascii(b"helloworld"), None);
    assert_eq!(index_of_space_or_newline_or_non_ascii(b"foo\xC3\xA9"), Some(3));
}

#[test]
fn hash_or_at_or_newline_scan() {
    assert_eq!(index_of_newline_or_non_ascii_or_hash_or_at(b"she # said"), Some(4));
    assert_eq!(index_of_newline_or_non_ascii_or_hash_or_at(b"user@host"), Some(4));
    assert_eq!(index_of_newline_or_non_ascii_or_hash_or_at(b"line\nbreak"), Some(4));
    assert_eq!(index_of_newline_or_non_ascii_or_hash_or_at(b"plain"), None);
}

#[test]
fn hex_encode_lowercase() {
    let input = [0x12u8, 0xab, 0xff, 0x00];
    let mut output = [0u8; 8];
    encode_hex_lower(&input, &mut output);
    assert_eq!(&output, b"12abff00");
}

#[test]
fn hex_decode_roundtrip() {
    let original = [0x01u8, 0x23, 0x45, 0x67, 0x89, 0xab];
    let mut encoded = [0u8; 12];
    encode_hex_lower(&original, &mut encoded);
    let mut decoded = [0u8; 6];
    let n = decode_hex(&encoded, &mut decoded);
    assert_eq!(n, 6);
    assert_eq!(&decoded, &original);
}

#[test]
fn hex_decode_stops_on_invalid() {
    let mut out = [0u8; 4];
    // 'g' is not hex → stops at pair 1.
    let n = decode_hex(b"ZZgg1234", &mut out);
    assert_eq!(n, 0);
}

#[test]
fn hex_decode_u16_basic() {
    let input: [u16; 4] = [b'1' as u16, b'2' as u16, b'a' as u16, b'b' as u16];
    let mut out = [0u8; 2];
    let n = decode_hex_u16(&input, &mut out);
    assert_eq!(n, 2);
    assert_eq!(out, [0x12, 0xab]);
}

#[test]
fn hex_decode_u16_rejects_high_units() {
    let input: [u16; 2] = [0x100, 0x30];
    let mut out = [0u8; 1];
    let n = decode_hex_u16(&input, &mut out);
    assert_eq!(n, 0); // 0x100 > 0xFF → invalid
}

#[test]
fn copy_ascii_prefix_stops_at_non_ascii() {
    let src = b"hello\xc3\xa9world";
    let mut dst = [0u8; 32];
    let n = copy_ascii_prefix(src, &mut dst);
    assert_eq!(n, 5);
    assert_eq!(&dst[..5], b"hello");
}

#[test]
fn copy_ascii_prefix_full_copy() {
    let src = b"plain ascii";
    let mut dst = [0u8; 32];
    let n = copy_ascii_prefix(src, &mut dst);
    assert_eq!(n, src.len());
    assert_eq!(&dst[..n], src);
}

#[test]
fn copy_u16_to_u8_truncates_high_bytes() {
    let input: [u16; 4] = [0x41, 0x42, 0x143, 0x100];
    let mut output = [0u8; 4];
    copy_u16_to_u8(&input, &mut output);
    assert_eq!(output, [0x41, 0x42, 0x43, 0x00]);
}

#[test]
fn websocket_mask_roundtrip() {
    let original = b"Hello, WebSocket masking test!";
    let mask = [0x12u8, 0x34, 0x56, 0x78];
    let mut masked = vec![0u8; original.len()];
    fill_with_skip_mask(mask, &mut masked, original, false);
    assert_ne!(&masked[..], &original[..]);

    let mut unmasked = vec![0u8; original.len()];
    fill_with_skip_mask(mask, &mut unmasked, &masked, false);
    assert_eq!(&unmasked[..], &original[..]);
}

#[test]
fn websocket_mask_skip_is_copy() {
    let original = b"payload";
    let mut dst = [0u8; 7];
    fill_with_skip_mask([0xff; 4], &mut dst, original, true);
    assert_eq!(&dst, original);
}

#[test]
fn websocket_mask_inplace_xor() {
    let mut buf = *b"ABCDEFGH";
    let expected_after_xor: [u8; 8] = [
        b'A' ^ 0xff, b'B' ^ 0xff, b'C' ^ 0xff, b'D' ^ 0xff,
        b'E' ^ 0xff, b'F' ^ 0xff, b'G' ^ 0xff, b'H' ^ 0xff,
    ];
    fill_with_skip_mask_inplace([0xff; 4], &mut buf, false);
    assert_eq!(&buf, &expected_after_xor);
    fill_with_skip_mask_inplace([0xff; 4], &mut buf, false);
    assert_eq!(&buf, b"ABCDEFGH");
}

#[test]
fn char_frequency_counts_identifiers_only() {
    let mut freqs = [0i32; 64];
    let text = b"abc_123$XYZ";
    scan_char_frequency(text, &mut freqs, 1);
    // Lowercase 'a'..'c' → index 36..38.
    assert_eq!(freqs[36], 1); // 'a'
    assert_eq!(freqs[37], 1); // 'b'
    assert_eq!(freqs[38], 1); // 'c'
    // Digits '1','2','3' → index 1,2,3.
    assert_eq!(freqs[1], 1); // '1'
    assert_eq!(freqs[2], 1); // '2'
    assert_eq!(freqs[3], 1); // '3'
    // '_' at index 62.
    assert_eq!(freqs[62], 1);
    // '$' at index 63.
    assert_eq!(freqs[63], 1);
    // Uppercase 'X','Y','Z' → index 10+23, 10+24, 10+25 = 33,34,35.
    assert_eq!(freqs[33], 1); // 'X'
    assert_eq!(freqs[34], 1); // 'Y'
    assert_eq!(freqs[35], 1); // 'Z'
}

#[test]
fn char_frequency_ignores_non_identifiers() {
    let mut freqs = [0i32; 64];
    scan_char_frequency(b"  .,;  ", &mut freqs, 1);
    assert!(freqs.iter().all(|&v| v == 0));
}

#[test]
fn char_frequency_delta_zero_noop() {
    let mut freqs = [5i32; 64];
    scan_char_frequency(b"abc", &mut freqs, 0);
    assert!(freqs.iter().all(|&v| v == 5));
}
