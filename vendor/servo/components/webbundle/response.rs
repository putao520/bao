/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Decoding of one stored response entry.
//!
//! A response entry is a CBOR array `[headers: bstr, body: bstr]`. The headers
//! byte string decodes to a CBOR map of byte-string pairs; the `:status`
//! pseudo-header carries the HTTP status code and the remaining members become
//! the response's header list. Additional trailing array elements (variants in
//! later drafts) are ignored: subresource loading only consumes the first two.

use ciborium::Value;
use http::{HeaderMap, HeaderName, HeaderValue};
use log::warn;

use super::{BundleError, decode};

pub(super) struct ParsedResponse {
    pub status: u16,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

pub(super) fn parse_response_entry(value: Value) -> Result<ParsedResponse, BundleError> {
    let Value::Array(mut items) = value else {
        return Err(BundleError::InvalidResponse("response entry is not an array"));
    };
    if items.len() < 2 {
        return Err(BundleError::InvalidResponse(
            "response entry needs headers and a body",
        ));
    }
    // Take the first two elements (headers, body); variants would follow.
    let headers_value = items.remove(0);
    let body_value = items.remove(0);

    let Value::Bytes(header_bytes) = headers_value else {
        return Err(BundleError::InvalidResponse(
            "response headers are not a byte string",
        ));
    };
    let Value::Bytes(body) = body_value else {
        return Err(BundleError::InvalidResponse(
            "response body is not a byte string",
        ));
    };

    let Value::Map(header_pairs) = decode(&header_bytes)? else {
        return Err(BundleError::InvalidResponse(
            "response header map is not a map",
        ));
    };

    let mut status = None;
    let mut headers = HeaderMap::new();
    for (name_value, value_value) in header_pairs {
        let Some(name_bytes) = name_value.as_bytes() else {
            return Err(BundleError::InvalidResponse(
                "response header name is not a byte string",
            ));
        };
        let Some(value_bytes) = value_value.as_bytes() else {
            return Err(BundleError::InvalidResponse(
                "response header value is not a byte string",
            ));
        };
        if name_bytes.as_slice() == b":status" {
            let text = std::str::from_utf8(value_bytes)
                .map_err(|_| BundleError::InvalidResponse("invalid :status value"))?;
            let code: u16 = text
                .parse()
                .map_err(|_| BundleError::InvalidResponse("invalid :status value"))?;
            if !(100..=599).contains(&code) {
                return Err(BundleError::InvalidResponse(":status is out of range"));
            }
            status = Some(code);
            continue;
        }
        // Other pseudo-headers (":authority", ...) are connection metadata and
        // are never exposed on the served response.
        if name_bytes.first() == Some(&b':') {
            continue;
        }
        let Ok(name) = HeaderName::from_bytes(name_bytes) else {
            warn!("Ignoring web bundle response header with an invalid name");
            continue;
        };
        let Ok(value) = HeaderValue::from_bytes(value_bytes) else {
            warn!("Ignoring web bundle response header with an invalid value");
            continue;
        };
        // The stored body is the decoded payload: a transfer/compound encoding
        // header would misdescribe it, so drop those (the draft stores bodies
        // post-decoding).
        if name == http::header::CONTENT_ENCODING || name == http::header::TRANSFER_ENCODING {
            continue;
        }
        headers.append(name, value);
    }

    let status = status.ok_or(BundleError::InvalidResponse(
        "response is missing the :status pseudo-header",
    ))?;

    Ok(ParsedResponse {
        status,
        headers,
        body,
    })
}
