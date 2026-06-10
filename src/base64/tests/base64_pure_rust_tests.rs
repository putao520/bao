use bun_base64::{encode_alloc, decode_alloc, encode_len, decode_len,
    simdutf_encode_url_safe_alloc, url_safe_encode_len};

// ──────────────────────────────────────────────────────────────────────────
// Standard base64 encode/decode roundtrip
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn encode_decode_roundtrip() {
    let input = b"Hello, base64! This is a pure Rust roundtrip test.";
    let encoded = encode_alloc(input);
    assert!(!encoded.is_empty());

    let decoded = decode_alloc(&encoded).expect("decode");
    assert_eq!(decoded, input);
}

#[test]
fn encode_decode_empty() {
    let encoded = encode_alloc(b"");
    let decoded = decode_alloc(&encoded).expect("decode empty");
    assert_eq!(decoded, b"");
}

#[test]
fn encode_decode_binary() {
    let input: Vec<u8> = (0..=255).collect();
    let encoded = encode_alloc(&input);
    let decoded = decode_alloc(&encoded).expect("decode binary");
    assert_eq!(decoded, input);
}

#[test]
fn encode_decode_large() {
    let input: Vec<u8> = (0..100_000).map(|i| (i % 256) as u8).collect();
    let encoded = encode_alloc(&input);
    let decoded = decode_alloc(&encoded).expect("decode large");
    assert_eq!(decoded, input);
}

#[test]
fn encode_known_value() {
    // RFC 4648 test vectors
    assert_eq!(&encode_alloc(b""), b"");
    assert_eq!(&encode_alloc(b"f"), b"Zg==");
    assert_eq!(&encode_alloc(b"fo"), b"Zm8=");
    assert_eq!(&encode_alloc(b"foo"), b"Zm9v");
    assert_eq!(&encode_alloc(b"foobar"), b"Zm9vYmFy");
}

// ──────────────────────────────────────────────────────────────────────────
// URL-safe encoding
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn url_safe_encode_no_padding() {
    let input = b"Hello, URL-safe base64!";
    let encoded = simdutf_encode_url_safe_alloc(input);
    // URL-safe uses - and _ instead of + and /
    assert!(!encoded.iter().any(|&b| b == b'+' || b == b'/'),
        "URL-safe should not contain + or /");
    assert!(!encoded.iter().any(|&b| b == b'='),
        "URL-safe should not contain padding =");
}

#[test]
fn url_safe_encode_non_empty() {
    let input = b">>?hello<<?world";
    let encoded = simdutf_encode_url_safe_alloc(input);
    assert!(!encoded.is_empty());
}

// ──────────────────────────────────────────────────────────────────────────
// Length calculations
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn encode_len_sufficient() {
    let input = b"test data for length check";
    let len = encode_len(input);
    let encoded = encode_alloc(input);
    assert!(encoded.len() <= len, "encode_len should be >= actual encoded size");
}

#[test]
fn decode_len_upper_bound() {
    let input = b"SGVsbG8="; // "Hello" in base64
    let len = decode_len(input);
    let decoded = decode_alloc(input).expect("decode");
    assert!(decoded.len() <= len, "decode_len should be >= actual decoded size");
}

#[test]
fn url_safe_encode_len_sufficient() {
    let input = b"test data for URL-safe length check";
    let len = url_safe_encode_len(input);
    let encoded = simdutf_encode_url_safe_alloc(input);
    assert!(encoded.len() <= len, "url_safe_encode_len should be >= actual encoded size");
}

// ──────────────────────────────────────────────────────────────────────────
// Error handling
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn decode_invalid_input_no_panic() {
    let result = decode_alloc(b"!!!invalid!!!");
    // Should either fail or produce some output (lenient mode)
    // The important thing is it doesn't panic
    let _ = result;
}

// ──────────────────────────────────────────────────────────────────────────
// VLQ encoding — uses VLQ struct API
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn vlq_encode_decode_roundtrip() {
    use bun_base64::VLQ;

    for value in [0i32, 1, -1, 63, -64, 64, -65, 1000, -1000, 10000, -10000] {
        let encoded = VLQ::encode(value);
        let slice = encoded.slice();
        assert!(!slice.is_empty(), "VLQ encode should produce bytes for {value}");

        let result = bun_base64::vlq_mod::decode(slice, 0);
        assert_eq!(result.value, value, "VLQ roundtrip for {value}");
    }
}

#[test]
fn vlq_encode_zero() {
    use bun_base64::VLQ;

    let encoded = VLQ::encode(0);
    let slice = encoded.slice();
    assert_eq!(slice.len(), 1);
    assert_eq!(slice[0], b'A'); // 0 maps to 'A' in base64 VLQ
}

#[test]
fn vlq_single_byte_range() {
    use bun_base64::VLQ;

    // Values 0..=15 should encode as single base64 characters
    for v in 0..=15 {
        let encoded = VLQ::encode(v);
        assert_eq!(encoded.slice().len(), 1, "value {v} should encode as single byte");
    }
}

#[test]
fn vlq_negative_values() {
    use bun_base64::VLQ;

    for v in [-1, -2, -15, -100] {
        let encoded = VLQ::encode(v);
        let result = bun_base64::vlq_mod::decode(encoded.slice(), 0);
        assert_eq!(result.value, v, "VLQ roundtrip for negative {v}");
    }
}
