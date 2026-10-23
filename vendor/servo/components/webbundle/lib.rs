/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! WICG WebBundle (`application/webbundle`) parsing for web bundle subresource
//! loading.
//!
//! BAO PATCH (REQ-BRW-051): upstream Servo has no web-bundle support. This
//! component is a fork-only addition implementing the draft-2 serialization
//! that the Web Platform Tests fixtures use (`resources/wbn/*.wbn`). The
//! contract below is derived from the fixtures and the WICG draft:
//!
//! ```text
//! bundle   = [ magic: bstr,          ; F0 9F 8C 90 F0 9F 93 A6 ("🌐📦")
//!              version: bstr,        ; "b2\0\0" for draft-2
//!              index-header: bstr,   ; CBOR ["index", len, ("primary", len,)? "responses", len]
//!              index-body: array,    ; CBOR [map<text => [offset, length]>, (primary: text,)? responses: array]
//!              ? trailer: bstr ]     ; 8-byte big-endian total bundle length
//! ```
//!
//! The response section is the CBOR array of `[headers: bstr, body: bstr]`
//! entries. `offset`/`length` pairs point into that array's serialized bytes:
//! the first entry starts right after the CBOR array header and entries are
//! contiguous, so a bundle whose offsets are not contiguous is malformed.
//! `headers` is itself a CBOR map of byte-string pairs whose `:status` member
//! carries the HTTP status code.
//!
//! Relative index keys (e.g. `root.js`, `../wbn/file.js`, `//host/path`,
//! `/path`) are resolved against the bundle URL; absolute `http(s)` and
//! `uuid-in-package:` keys are kept as-is. This parser performs no origin or
//! path-restriction filtering — the `<script type=webbundle>` rule layer owns
//! those checks (they depend on the element's `resources`/`scopes` lists).

mod response;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::fmt;

use ciborium::Value;
use http::HeaderMap;
use servo_url::ServoUrl;

/// The WICG WebBundle magic number: the UTF-8 encoding of "🌐📦".
pub const MAGIC: [u8; 8] = [0xF0, 0x9F, 0x8C, 0x90, 0xF0, 0x9F, 0x93, 0xA6];

/// The draft-2 version marker carried by every WPT fixture.
pub const VERSION_B2: [u8; 4] = [b'b', b'2', 0, 0];

/// The MIME type a bundle response must carry.
pub const MIME_TYPE: &str = "application/webbundle";

/// The exact `Accept` header value a bundle request must send. WPT's
/// `check-accept-header-and-return-bundle.py` splits the header on commas and
/// requires this token to appear verbatim (no spaces, so the fixture's
/// `in accept_values` check succeeds).
pub const ACCEPT_HEADER_VALUE: &str = "application/webbundle;v=b2";

/// The `uuid-in-package:` URL scheme for in-bundle resources that cannot be
/// fetched from the network.
pub const UUID_IN_PACKAGE_SCHEME: &str = "uuid-in-package";

/// Errors produced while parsing a bundle. Every variant makes the bundle
/// element fail (error event) and every registered subresource fail closed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BundleError {
    /// Structurally invalid CBOR or bundle layout.
    Malformed(&'static str),
    /// The version marker is not `b2\0\0`.
    UnsupportedVersion,
    /// A response entry is invalid (missing `:status`, bad header bytes, ...).
    InvalidResponse(&'static str),
    /// An index key could not be resolved to a URL.
    InvalidUrl(String),
    /// An index key uses a scheme that subresource loading does not support.
    UnsupportedUrlScheme(String),
    /// The declared total bundle length does not match the input length.
    LengthMismatch,
}

impl fmt::Display for BundleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BundleError::Malformed(reason) => write!(formatter, "malformed web bundle: {reason}"),
            BundleError::UnsupportedVersion => {
                write!(formatter, "unsupported web bundle version")
            },
            BundleError::InvalidResponse(reason) => {
                write!(formatter, "invalid web bundle response: {reason}")
            },
            BundleError::InvalidUrl(url) => write!(formatter, "invalid web bundle URL: {url}"),
            BundleError::UnsupportedUrlScheme(scheme) => {
                write!(formatter, "unsupported web bundle URL scheme: {scheme}")
            },
            BundleError::LengthMismatch => {
                write!(formatter, "web bundle length does not match its trailer")
            },
        }
    }
}

impl std::error::Error for BundleError {}

/// One response stored in a bundle.
#[derive(Clone, Debug)]
pub struct BundleResponse {
    /// The absolute URL the response is served for (index key resolved against
    /// the bundle URL).
    pub url: ServoUrl,
    /// The HTTP status code from the stored `:status` pseudo-header.
    pub status: u16,
    /// The stored response headers (`:status` removed).
    pub headers: HeaderMap,
    /// The stored response body.
    pub body: Vec<u8>,
}

/// A parsed bundle: every response with its resolved URL.
#[derive(Clone, Debug)]
pub struct Bundle {
    pub responses: Vec<BundleResponse>,
}

impl Bundle {
    /// Returns the response stored for `url`, if the bundle contains one.
    pub fn response_for(&self, url: &str) -> Option<&BundleResponse> {
        self.responses.iter().find(|response| response.url.as_str() == url)
    }
}

struct IndexEntry {
    url: ServoUrl,
    offset: u64,
    length: u64,
}

/// Parse a draft-2 bundle, resolving relative index keys against `bundle_url`.
pub fn parse(bytes: &[u8], bundle_url: &ServoUrl) -> Result<Bundle, BundleError> {
    let Value::Array(outer_items) = decode(bytes)? else {
        return Err(BundleError::Malformed("top-level value is not an array"));
    };
    if outer_items.len() < 4 || outer_items.len() > 5 {
        return Err(BundleError::Malformed("bundle array has an unexpected number of elements"));
    }

    let mut outer_iter = outer_items.into_iter();
    let magic_value = outer_iter.next().expect("checked length");
    let version_value = outer_iter.next().expect("checked length");
    let header_value = outer_iter.next().expect("checked length");
    let body_value = outer_iter.next().expect("checked length");
    let trailer_value = outer_iter.next();

    let magic = magic_value
        .as_bytes()
        .ok_or(BundleError::Malformed("magic is not a byte string"))?;
    if magic.as_slice() != MAGIC.as_slice() {
        return Err(BundleError::Malformed("magic does not match the web bundle magic"));
    }

    let version = version_value
        .as_bytes()
        .ok_or(BundleError::Malformed("version is not a byte string"))?;
    if version.as_slice() != VERSION_B2.as_slice() {
        return Err(BundleError::UnsupportedVersion);
    }

    // The draft-2 trailer is the total length of the bundle as an 8-byte
    // big-endian byte string (the fixtures all carry it). Reject a bundle whose
    // trailer disagrees with its actual size: the offsets below are interpreted
    // relative to a section layout that a truncated/extended file would break.
    if let Some(trailer_value) = trailer_value {
        if let Some(trailer_bytes) = trailer_value.as_bytes() {
            if trailer_bytes.len() == 8 {
                let declared_length = u64::from_be_bytes(
                    <[u8; 8]>::try_from(trailer_bytes.as_slice()).expect("length checked"),
                );
                if declared_length != bytes.len() as u64 {
                    return Err(BundleError::LengthMismatch);
                }
            }
        }
    }

    let header_bytes = header_value
        .as_bytes()
        .ok_or(BundleError::Malformed("index header is not a byte string"))?;
    let (section_names, declared_sizes) = parse_index_header(header_bytes)?;

    let Value::Array(body_items) = body_value else {
        return Err(BundleError::Malformed("index body is not an array"));
    };
    if body_items.len() != section_names.len() {
        return Err(BundleError::Malformed("index body does not match the index header"));
    }

    let mut body_iter = body_items.into_iter();
    let index_value = body_iter.next().expect("non-empty section list");
    let mut responses_value = None;
    for name in section_names.iter().skip(1) {
        let value = body_iter.next().expect("checked section count");
        if name == "responses" {
            responses_value = Some(value);
        }
    }
    let Some(Value::Array(response_values)) = responses_value else {
        return Err(BundleError::Malformed("responses section is not an array"));
    };
    let Value::Map(index_entries) = index_value else {
        return Err(BundleError::Malformed("index section is not a map"));
    };

    let declared_responses_size = section_names
        .iter()
        .position(|name| name == "responses")
        .map(|position| declared_sizes[position]);

    let mut entries = Vec::with_capacity(index_entries.len());
    for (key_value, range_value) in index_entries {
        let key = match key_value {
            Value::Text(key) => key,
            Value::Bytes(key) => String::from_utf8(key)
                .map_err(|_| BundleError::Malformed("index key is not valid UTF-8"))?,
            _ => return Err(BundleError::Malformed("index key is not a string")),
        };
        let Value::Array(range) = range_value else {
            return Err(BundleError::Malformed("index entry is not an array"));
        };
        if range.len() < 2 {
            return Err(BundleError::Malformed("index entry needs an offset and a length"));
        }
        let offset = range[0]
            .as_integer()
            .and_then(|value| u64::try_from(value).ok())
            .ok_or(BundleError::Malformed("index entry offset is not an unsigned integer"))?;
        let length = range[1]
            .as_integer()
            .and_then(|value| u64::try_from(value).ok())
            .ok_or(BundleError::Malformed("index entry length is not an unsigned integer"))?;
        entries.push(IndexEntry {
            url: resolve_url(bundle_url, &key)?,
            offset,
            length,
        });
    }

    if entries.len() != response_values.len() {
        return Err(BundleError::Malformed(
            "index entry count does not match the response count",
        ));
    }

    // Offsets address the serialized responses array: the first entry starts
    // after the array header and entries are laid out contiguously.
    entries.sort_by_key(|entry| entry.offset);
    let mut expected_offset = cbor_array_header_len(response_values.len());
    let mut responses = Vec::with_capacity(response_values.len());
    for (entry, response_value) in entries.into_iter().zip(response_values.into_iter()) {
        if entry.offset != expected_offset {
            return Err(BundleError::Malformed("response offsets are not contiguous"));
        }
        let parsed = response::parse_response_entry(response_value)?;
        responses.push(BundleResponse {
            url: entry.url,
            status: parsed.status,
            headers: parsed.headers,
            body: parsed.body,
        });
        expected_offset = expected_offset
            .checked_add(entry.length)
            .ok_or(BundleError::Malformed("response offset overflow"))?;
    }
    if let Some(declared) = declared_responses_size {
        if declared != expected_offset {
            return Err(BundleError::Malformed("responses section size mismatch"));
        }
    }

    Ok(Bundle { responses })
}

/// Decode the `index-header` byte string: an array of alternating section names
/// and declared serialized sizes, e.g. `["index", N, "primary", M,
/// "responses", K]` or `["index", N, "responses", K]`.
fn parse_index_header(header_bytes: &[u8]) -> Result<(Vec<String>, Vec<u64>), BundleError> {
    let Value::Array(header_items) = decode(header_bytes)? else {
        return Err(BundleError::Malformed("index header is not an array"));
    };
    if header_items.len() % 2 != 0 || header_items.is_empty() {
        return Err(BundleError::Malformed("index header is not name/size pairs"));
    }
    let mut names = Vec::with_capacity(header_items.len() / 2);
    let mut sizes = Vec::with_capacity(header_items.len() / 2);
    let mut items = header_items.into_iter();
    while let (Some(name_value), Some(size_value)) = (items.next(), items.next()) {
        let Some(name) = name_value.as_text() else {
            return Err(BundleError::Malformed("index section name is not a string"));
        };
        let size = size_value
            .as_integer()
            .and_then(|value| u64::try_from(value).ok())
            .ok_or(BundleError::Malformed(
                "index section size is not an unsigned integer",
            ))?;
        names.push(name.to_string());
        sizes.push(size);
    }
    if names.first().map(String::as_str) != Some("index") ||
        names.last().map(String::as_str) != Some("responses")
    {
        return Err(BundleError::Malformed("unexpected index section names"));
    }
    Ok((names, sizes))
}

/// The serialized size of a CBOR array header for `count` elements.
fn cbor_array_header_len(count: usize) -> u64 {
    if count < 24 {
        1
    } else if count <= u8::MAX as usize {
        2
    } else if count <= u16::MAX as usize {
        3
    } else if count <= u32::MAX as usize {
        5
    } else {
        9
    }
}

/// Resolve an index key against the bundle URL. Absolute URLs are kept;
/// everything else is URL-joined (relative paths, `./`, `../`, `/absolute-path`
/// and `//host/path` all work through the URL parser).
fn resolve_url(bundle_url: &ServoUrl, key: &str) -> Result<ServoUrl, BundleError> {
    let url = match ServoUrl::parse(key) {
        Ok(url) => url,
        Err(_) => bundle_url
            .join(key)
            .map_err(|_| BundleError::InvalidUrl(key.to_string()))?,
    };
    match url.scheme() {
        "http" | "https" | UUID_IN_PACKAGE_SCHEME => Ok(url),
        scheme => Err(BundleError::UnsupportedUrlScheme(scheme.to_string())),
    }
}

/// Decode exactly one CBOR value from `bytes`.
fn decode(bytes: &[u8]) -> Result<Value, BundleError> {
    ciborium::de::from_reader(bytes).map_err(|_| BundleError::Malformed("CBOR decoding failed"))
}

/// A map from URL string to response, used by callers that serve many
/// subresource requests from the same bundle.
pub fn response_map(bundle: Bundle) -> HashMap<String, BundleResponse> {
    bundle
        .responses
        .into_iter()
        .map(|response| (response.url.as_str().to_string(), response))
        .collect()
}
