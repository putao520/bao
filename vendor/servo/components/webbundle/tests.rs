/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Unit tests for the draft-2 parser.
//!
//! The fixtures live in the WPT checkout (`tests/web-bundle/resources/wbn`),
//! which is not part of this crate, so the builder below re-creates the same
//! serialization from scratch: an outer CBOR array of magic/version/
//! index-header/index-body/length-trailer where the index body is
//! `[map, (primary,)? responses]` and the map's `[offset, length]` ranges point
//! into the serialized responses array, whose entries are contiguous after its
//! CBOR array header.

use ciborium::Value;
use servo_url::ServoUrl;

use super::*;

struct TestResponse {
    /// Index key (absolute, relative, or `uuid-in-package:`).
    url: &'static str,
    body: &'static [u8],
    /// `None` omits the `:status` pseudo-header (invalid bundle).
    status: Option<u16>,
    headers: &'static [(&'static str, &'static str)],
}

impl TestResponse {
    fn new(url: &'static str, body: &'static [u8]) -> Self {
        Self {
            url,
            body,
            status: Some(200),
            headers: &[],
        }
    }
}

fn encode(value: &Value) -> Vec<u8> {
    let mut bytes = Vec::new();
    ciborium::ser::into_writer(value, &mut bytes).expect("CBOR serialization");
    bytes
}

fn encode_headers(status: Option<u16>, headers: &[(&str, &str)]) -> Vec<u8> {
    let mut pairs = Vec::new();
    if let Some(status) = status {
        pairs.push((
            Value::Bytes(b":status".to_vec()),
            Value::Bytes(status.to_string().into_bytes()),
        ));
    }
    for (name, value) in headers {
        pairs.push((
            Value::Bytes(name.as_bytes().to_vec()),
            Value::Bytes(value.as_bytes().to_vec()),
        ));
    }
    encode(&Value::Map(pairs))
}

fn array_header_len(count: usize) -> u64 {
    cbor_array_header_len(count)
}

/// Build a bundle. `primary` adds the optional `"primary"` index section.
fn build_bundle(responses: &[TestResponse], primary: Option<&'static str>) -> Vec<u8> {
    let entries: Vec<Value> = responses
        .iter()
        .map(|response| {
            Value::Array(vec![
                Value::Bytes(encode_headers(response.status, response.headers)),
                Value::Bytes(response.body.to_vec()),
            ])
        })
        .collect();

    // Compute the serialized offset/length of every response entry.
    let mut cursor = array_header_len(entries.len());
    let mut ranges = Vec::with_capacity(entries.len());
    for entry in &entries {
        let encoded = encode(entry);
        ranges.push((cursor, encoded.len() as u64));
        cursor += encoded.len() as u64;
    }
    let responses_value = Value::Array(entries);
    let responses_bytes = encode(&responses_value);
    assert_eq!(
        responses_bytes.len() as u64, cursor,
        "response entries must concatenate to the array body"
    );

    let map_pairs: Vec<(Value, Value)> = responses
        .iter()
        .zip(ranges.iter())
        .map(|(response, (offset, length))| {
            (
                Value::Text(response.url.to_string()),
                Value::Array(vec![
                    Value::Integer((*offset).into()),
                    Value::Integer((*length).into()),
                ]),
            )
        })
        .collect();
    let map_value = Value::Map(map_pairs);
    let map_bytes = encode(&map_value);

    let mut header_items = vec![
        Value::Text("index".to_string()),
        Value::Integer((map_bytes.len() as u64).into()),
    ];
    let mut body_items = vec![map_value];
    if let Some(primary) = primary {
        let primary_value = Value::Text(primary.to_string());
        let primary_bytes = encode(&primary_value);
        header_items.push(Value::Text("primary".to_string()));
        header_items.push(Value::Integer((primary_bytes.len() as u64).into()));
        body_items.push(primary_value);
    }
    header_items.push(Value::Text("responses".to_string()));
    header_items.push(Value::Integer((responses_bytes.len() as u64).into()));
    body_items.push(responses_value);

    let header_bytes = encode(&Value::Array(header_items));

    let mut outer = vec![
        Value::Bytes(MAGIC.to_vec()),
        Value::Bytes(VERSION_B2.to_vec()),
        Value::Bytes(header_bytes),
        Value::Array(body_items),
        // Placeholder trailer; the real total length is filled in below. Its
        // encoding is fixed-size so the total does not change.
        Value::Bytes(vec![0; 8]),
    ];
    let probe = encode(&Value::Array(outer.clone()));
    let total = probe.len() as u64;
    outer[4] = Value::Bytes(total.to_be_bytes().to_vec());
    let bundle = encode(&Value::Array(outer));
    assert_eq!(bundle.len() as u64, total);
    bundle
}

fn bundle_url() -> ServoUrl {
    ServoUrl::parse("https://example.test/dir/bundle.wbn").expect("valid bundle URL")
}

#[test]
fn parses_absolute_relative_and_uuid_urls() {
    let bundle_bytes = build_bundle(
        &[
            TestResponse {
                url: "relative.js",
                body: b"relative body",
                status: Some(200),
                headers: &[("content-type", "text/javascript")],
            },
            TestResponse {
                url: "https://example.test/dir/absolute.css",
                body: b"body{}",
                status: Some(200),
                headers: &[("content-type", "text/css")],
            },
            TestResponse {
                url: "uuid-in-package:020111b3-437a-4c5c-ae07-adb6bbffb720",
                body: b"window.report_result('OK');",
                status: Some(200),
                headers: &[],
            },
        ],
        None,
    );

    let bundle = parse(&bundle_bytes, &bundle_url()).expect("bundle parses");
    assert_eq!(bundle.responses.len(), 3);

    let relative = bundle
        .response_for("https://example.test/dir/relative.js")
        .expect("relative key resolves against the bundle URL");
    assert_eq!(relative.body, b"relative body".to_vec());
    assert_eq!(relative.status, 200);
    assert_eq!(
        relative
            .headers
            .get("content-type")
            .expect("content-type header")
            .to_str()
            .expect("ASCII header value"),
        "text/javascript"
    );

    assert!(
        bundle
            .response_for("https://example.test/dir/absolute.css")
            .is_some()
    );
    assert!(
        bundle
            .response_for("uuid-in-package:020111b3-437a-4c5c-ae07-adb6bbffb720")
            .is_some()
    );
    // Pseudo-headers are not exposed on the served response.
    assert!(!relative.headers.keys().any(|name| name.as_str() == ":status"));
}

#[test]
fn parses_primary_section() {
    let bundle_bytes = build_bundle(
        &[
            TestResponse::new("first.js", b"first"),
            TestResponse::new("second.js", b"second"),
        ],
        Some("https://example.test/dir/first.js"),
    );
    let bundle = parse(&bundle_bytes, &bundle_url()).expect("bundle with primary parses");
    assert_eq!(bundle.responses.len(), 2);
}

#[test]
fn rejects_bad_magic() {
    let mut bundle_bytes = build_bundle(&[TestResponse::new("a.js", b"a")], None);
    // The outer array header is one byte; the magic byte string follows it.
    bundle_bytes[2] ^= 0xFF;
    assert!(matches!(
        parse(&bundle_bytes, &bundle_url()),
        Err(BundleError::Malformed(_))
    ));
}

#[test]
fn rejects_unknown_version() {
    let mut bundle_bytes = build_bundle(&[TestResponse::new("a.js", b"a")], None);
    // Layout: 0x85 | 0x48 magic[8] | 0x44 version[4]. The version bytes are
    // b"b2\0\0" at offsets 11..15; turn it into b"b1\0\0".
    bundle_bytes[12] = b'1';
    assert_eq!(
        parse(&bundle_bytes, &bundle_url()).unwrap_err(),
        BundleError::UnsupportedVersion
    );
}

#[test]
fn rejects_truncated_bundle() {
    let bundle_bytes = build_bundle(&[TestResponse::new("a.js", b"a")], None);
    let truncated = &bundle_bytes[..bundle_bytes.len() - 10];
    assert!(matches!(
        parse(truncated, &bundle_url()),
        Err(BundleError::Malformed(_)) | Err(BundleError::LengthMismatch)
    ));
}

#[test]
fn rejects_length_mismatch() {
    let mut bundle_bytes = build_bundle(&[TestResponse::new("a.js", b"a")], None);
    let last = bundle_bytes.len() - 1;
    bundle_bytes[last] ^= 0x01;
    assert_eq!(
        parse(&bundle_bytes, &bundle_url()).unwrap_err(),
        BundleError::LengthMismatch
    );
}

#[test]
fn rejects_missing_status() {
    let bundle_bytes = build_bundle(
        &[TestResponse {
            url: "a.js",
            body: b"a",
            status: None,
            headers: &[],
        }],
        None,
    );
    assert_eq!(
        parse(&bundle_bytes, &bundle_url()).unwrap_err(),
        BundleError::InvalidResponse("response is missing the :status pseudo-header")
    );
}

#[test]
fn rejects_unsupported_index_scheme() {
    let bundle_bytes = build_bundle(&[TestResponse::new("data:text/plain,hello", b"x")], None);
    assert_eq!(
        parse(&bundle_bytes, &bundle_url()).unwrap_err(),
        BundleError::UnsupportedUrlScheme("data".to_string())
    );
}

#[test]
fn rejects_non_contiguous_offsets() {
    // Rebuild the outer value with a perturbed offset so the response range
    // check must reject it. This exercises the integrity check independently
    // of the fixture layout.
    let bundle_bytes = build_bundle(&[TestResponse::new("a.js", b"a"), TestResponse::new("b.js", b"b")], None);
    let mut outer = ciborium::de::from_reader::<Value, _>(bundle_bytes.as_slice()).expect("decode");
    let Value::Array(items) = &mut outer else {
        panic!("outer array")
    };
    let Value::Array(body) = &mut items[3] else {
        panic!("body array")
    };
    let Value::Map(pairs) = &mut body[0] else {
        panic!("map")
    };
    let (_, Value::Array(range)) = &mut pairs[0] else {
        panic!("range array")
    };
    // Keep the same CBOR length (single-byte integer) so the trailer check
    // passes and the contiguity check is what rejects the bundle.
    range[0] = Value::Integer(2u64.into());
    let broken = encode(&Value::Array(items.clone()));
    assert!(matches!(
        parse(&broken, &bundle_url()),
        Err(BundleError::Malformed(_)) | Err(BundleError::LengthMismatch)
    ));
}

#[test]
fn response_map_indexes_by_url() {
    let bundle_bytes = build_bundle(&[TestResponse::new("a.js", b"a")], None);
    let bundle = parse(&bundle_bytes, &bundle_url()).expect("parses");
    let map = response_map(bundle);
    assert!(map.contains_key("https://example.test/dir/a.js"));
}
