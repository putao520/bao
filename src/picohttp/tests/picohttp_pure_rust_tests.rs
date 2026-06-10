//! TDD tests for bun_picohttp pure-Rust implementation (httparse-backed).
//!
//! Covers: request parsing, response parsing, header handling, chunked decoding,
//! partial/invalid input, and NUL sentinel behavior.

use bun_picohttp::{
    ChunkedDecoder, ChunkedError, ChunkedState, Header, HeaderList, Headers, ParseHeadersError,
    ParseRequestError, ParseResponseError, Request, Response,
};

// ──────────────────────────────────────────────────────────────────────────
// Header
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn header_zero_is_all_nulls() {
    let h = Header::ZERO;
    assert!(h.name().is_empty());
    assert!(h.value().is_empty());
    assert!(h.is_multiline());
}

#[test]
fn header_new_points_to_slices() {
    let name = b"Content-Type";
    let value = b"text/html";
    let h = Header::new(name, value);
    assert_eq!(h.name(), b"Content-Type");
    assert_eq!(h.value(), b"text/html");
    assert!(!h.is_multiline());
}

#[test]
fn header_new_empty_value() {
    let h = Header::new(b"X-Empty", b"");
    assert_eq!(h.name(), b"X-Empty");
    assert!(h.value().is_empty());
}

// ──────────────────────────────────────────────────────────────────────────
// HeaderList
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn header_list_get_finds_case_insensitive() {
    let h1 = Header::new(b"Content-Type", b"text/plain");
    let h2 = Header::new(b"host", b"example.com");
    let headers = [h1, h2];
    let list = HeaderList { list: &headers };
    assert_eq!(list.get(b"content-type"), Some(&b"text/plain"[..]));
    assert_eq!(list.get(b"HOST"), Some(&b"example.com"[..]));
    assert_eq!(list.get(b"accept"), None);
}

#[test]
fn header_list_get_if_other_is_absent() {
    let h1 = Header::new(b"Content-Encoding", b"gzip");
    let h2 = Header::new(b"Content-Type", b"text/html");
    let headers = [h1, h2];
    let list = HeaderList { list: &headers };
    // "Content-Type" found, "Content-Encoding" is absent -> return Content-Type
    assert_eq!(
        list.get_if_other_is_absent(b"Content-Type", b"Content-Encoding"),
        None
    );
    // "Accept" not found, "Content-Encoding" present -> None (other is present)
    assert_eq!(list.get_if_other_is_absent(b"Accept", b"Content-Encoding"), None);
    // "Content-Type" found, "Accept" absent -> return Content-Type
    assert_eq!(
        list.get_if_other_is_absent(b"Content-Type", b"Accept"),
        Some(&b"text/html"[..])
    );
}

// ──────────────────────────────────────────────────────────────────────────
// Request parsing
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn parse_get_request() {
    let raw = b"GET /index.html HTTP/1.1\r\nHost: example.com\r\n\r\n";
    let mut headers = [Header::ZERO; 16];
    let req = Request::parse(raw, &mut headers).unwrap();
    assert_eq!(req.method, b"GET");
    assert_eq!(req.path, b"/index.html");
    assert_eq!(req.minor_version, 1);
    assert_eq!(req.headers.len(), 1);
    assert_eq!(req.headers[0].name(), b"Host");
    assert_eq!(req.headers[0].value(), b"example.com");
}

#[test]
fn parse_post_request_with_body() {
    let raw = b"POST /submit HTTP/1.1\r\nContent-Length: 13\r\nContent-Type: text/plain\r\n\r\nHello, World!";
    let mut headers = [Header::ZERO; 16];
    let req = Request::parse(raw, &mut headers).unwrap();
    assert_eq!(req.method, b"POST");
    assert_eq!(req.path, b"/submit");
    assert_eq!(req.minor_version, 1);
    assert_eq!(req.headers.len(), 2);

    let content_length = req
        .headers
        .iter()
        .find(|h| h.name() == b"Content-Length")
        .unwrap();
    assert_eq!(content_length.value(), b"13");
}

#[test]
fn parse_request_http10() {
    let raw = b"GET / HTTP/1.0\r\n\r\n";
    let mut headers = [Header::ZERO; 4];
    let req = Request::parse(raw, &mut headers).unwrap();
    assert_eq!(req.minor_version, 0);
}

#[test]
fn parse_request_multiple_headers() {
    let raw = b"GET / HTTP/1.1\r\nAccept: */*\r\nAccept-Encoding: gzip\r\nUser-Agent: test/1.0\r\n\r\n";
    let mut headers = [Header::ZERO; 16];
    let req = Request::parse(raw, &mut headers).unwrap();
    assert_eq!(req.headers.len(), 3);
    assert_eq!(req.headers[0].name(), b"Accept");
    assert_eq!(req.headers[1].name(), b"Accept-Encoding");
    assert_eq!(req.headers[2].name(), b"User-Agent");
}

#[test]
fn parse_request_partial_returns_short_read() {
    let raw = b"GET / HTTP/1.1\r\nHost: incomplete";
    let mut headers = [Header::ZERO; 4];
    match Request::parse(raw, &mut headers) {
        Err(ParseRequestError::ShortRead) => {}
        other => panic!("expected ShortRead, got {:?}", other),
    }
}

#[test]
fn parse_request_garbage_returns_bad_request() {
    let raw = b"THIS IS NOT HTTP\r\n\r\n";
    let mut headers = [Header::ZERO; 4];
    match Request::parse(raw, &mut headers) {
        Err(ParseRequestError::BadRequest) => {}
        other => panic!("expected BadRequest, got {:?}", other),
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Response parsing
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn parse_response_200() {
    let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
    let mut headers = [Header::ZERO; 16];
    let resp = Response::parse(raw, &mut headers).unwrap();
    assert_eq!(resp.minor_version, 1);
    assert_eq!(resp.status_code, 200);
    assert_eq!(resp.status, b"OK");
    assert_eq!(resp.headers.list.len(), 1);
    assert_eq!(resp.headers.list[0].name(), b"Content-Length");
    assert_eq!(resp.headers.list[0].value(), b"5");
}

#[test]
fn parse_response_404() {
    let raw = b"HTTP/1.1 404 Not Found\r\nContent-Type: text/html\r\n\r\n<html>Not Found</html>";
    let mut headers = [Header::ZERO; 16];
    let resp = Response::parse(raw, &mut headers).unwrap();
    assert_eq!(resp.status_code, 404);
    assert_eq!(resp.status, b"Not Found");
}

#[test]
fn parse_response_100_continue() {
    let raw = b"HTTP/1.1 100 Continue\r\n\r\n";
    let mut headers = [Header::ZERO; 4];
    let resp = Response::parse(raw, &mut headers).unwrap();
    assert_eq!(resp.status_code, 100);
}

#[test]
fn parse_response_http10() {
    let raw = b"HTTP/1.0 200 OK\r\n\r\nbody";
    let mut headers = [Header::ZERO; 4];
    let resp = Response::parse(raw, &mut headers).unwrap();
    assert_eq!(resp.minor_version, 0);
}

#[test]
fn parse_response_partial_returns_short_read() {
    let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 100";
    let mut headers = [Header::ZERO; 4];
    match Response::parse(raw, &mut headers) {
        Err(ParseResponseError::ShortRead) => {}
        other => panic!("expected ShortRead, got {:?}", other),
    }
}

#[test]
fn parse_response_garbage_returns_malformed() {
    let raw = b"GARBAGE DATA\r\n\r\n";
    let mut headers = [Header::ZERO; 4];
    match Response::parse(raw, &mut headers) {
        Err(ParseResponseError::MalformedHttpResponse) => {}
        other => panic!("expected MalformedHttpResponse, got {:?}", other),
    }
}

#[test]
fn parse_response_with_offset() {
    let raw = b"HTTP/1.1 200 OK\r\n\r\n";
    let mut headers = [Header::ZERO; 4];
    let mut offset = 0usize;
    let resp = Response::parse_parts(raw, &mut headers, Some(&mut offset)).unwrap();
    assert_eq!(resp.status_code, 200);
}

// ──────────────────────────────────────────────────────────────────────────
// Headers parsing
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn parse_headers_from_request() {
    let raw = b"GET / HTTP/1.1\r\nX-Custom: value\r\n\r\n";
    let mut headers = [Header::ZERO; 4];
    let h = Headers::parse(raw, &mut headers).unwrap();
    assert_eq!(h.headers.len(), 1);
    assert_eq!(h.headers[0].name(), b"X-Custom");
    assert_eq!(h.headers[0].value(), b"value");
}

#[test]
fn parse_headers_partial() {
    let raw = b"GET / HTTP/1.1\r\nIncomplete: ";
    let mut headers = [Header::ZERO; 4];
    match Headers::parse(raw, &mut headers) {
        Err(ParseHeadersError::ShortRead) => {}
        other => panic!("expected ShortRead, got {:?}", other),
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Chunked Decoder
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn chunked_decode_single_chunk() {
    let mut decoder = ChunkedDecoder::default();
    let mut buf = b"5\r\nhello\r\n0\r\n\r\n".to_vec();
    let decoded_len = decoder.decode(&mut buf).unwrap();
    assert_eq!(&buf[..decoded_len], b"hello");
}

#[test]
fn chunked_decode_multiple_chunks() {
    let mut decoder = ChunkedDecoder::default();
    let mut buf = b"5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n".to_vec();
    let decoded_len = decoder.decode(&mut buf).unwrap();
    assert_eq!(&buf[..decoded_len], b"hello world");
}

#[test]
fn chunked_decode_empty_chunk() {
    let mut decoder = ChunkedDecoder::default();
    let mut buf = b"0\r\n\r\n".to_vec();
    let decoded_len = decoder.decode(&mut buf).unwrap();
    assert_eq!(&buf[..decoded_len], b"");
}

#[test]
fn chunked_decode_with_trailer() {
    let mut decoder = ChunkedDecoder {
        consume_trailer: 1,
        ..Default::default()
    };
    let mut buf = b"4\r\ndata\r\n0\r\nTrailer: value\r\n\r\n".to_vec();
    let decoded_len = decoder.decode(&mut buf).unwrap();
    assert_eq!(&buf[..decoded_len], b"data");
}

#[test]
fn chunked_decode_partial_returns_need_more() {
    let mut decoder = ChunkedDecoder::default();
    let mut buf = b"5\r\nhel".to_vec();
    let result = decoder.decode(&mut buf);
    assert!(matches!(result, Err(ChunkedError::NeedMore)));
}

#[test]
fn chunked_decode_invalid_hex() {
    let mut decoder = ChunkedDecoder::default();
    let mut buf = b"ZZ\r\nhello\r\n0\r\n\r\n".to_vec();
    let result = decoder.decode(&mut buf);
    assert!(matches!(result, Err(ChunkedError::Invalid)));
}

#[test]
fn chunked_decode_hex_uppercase() {
    let mut decoder = ChunkedDecoder::default();
    let mut buf = b"A\r\n1234567890\r\n0\r\n\r\n".to_vec();
    let decoded_len = decoder.decode(&mut buf).unwrap();
    assert_eq!(&buf[..decoded_len], b"1234567890");
}

#[test]
fn chunked_decode_hex_lowercase() {
    let mut decoder = ChunkedDecoder::default();
    let mut buf = b"a\r\n1234567890\r\n0\r\n\r\n".to_vec();
    let decoded_len = decoder.decode(&mut buf).unwrap();
    assert_eq!(&buf[..decoded_len], b"1234567890");
}

#[test]
fn chunked_decode_with_extension() {
    let mut decoder = ChunkedDecoder::default();
    let mut buf = b"5;ext=value\r\nhello\r\n0\r\n\r\n".to_vec();
    let decoded_len = decoder.decode(&mut buf).unwrap();
    assert_eq!(&buf[..decoded_len], b"hello");
}

#[test]
fn chunked_decode_state_after_partial() {
    let mut decoder = ChunkedDecoder::default();
    // First: partial data
    let mut buf1 = b"5\r\nhel".to_vec();
    let _ = decoder.decode(&mut buf1);
    assert!(decoder.bytes_left_in_chunk > 0);

    // Second: feed remaining data + termination
    let mut buf2 = b"lo\r\n0\r\n\r\n".to_vec();
    let decoded_len = decoder.decode(&mut buf2).unwrap();
    assert_eq!(&buf2[..decoded_len], b"lo");
}

#[test]
fn chunked_decode_raw_ffi_compatible() {
    let mut decoder = ChunkedDecoder::default();
    let mut buf = b"5\r\nhello\r\n0\r\n\r\n".to_vec();
    let mut len = buf.len();
    let rc = unsafe { ChunkedDecoder::decode_raw(&mut decoder, buf.as_mut_ptr(), &mut len) };
    assert_eq!(rc, 0);
    assert_eq!(&buf[..len], b"hello");
}

#[test]
fn chunked_decode_raw_ffi_via_phr_decode_chunked() {
    use bun_picohttp::phr_chunked_decoder;
    let mut decoder: phr_chunked_decoder = phr_chunked_decoder::default();
    let mut buf = b"5\r\nhello\r\n0\r\n\r\n".to_vec();
    let mut len = buf.len();
    let rc = unsafe { bun_picohttp::phr_decode_chunked(&mut decoder, buf.as_mut_ptr(), &mut len) };
    assert_eq!(rc, 0);
    assert_eq!(&buf[..len], b"hello");
}

#[test]
fn chunked_state_matches_picohttpparser_values() {
    // Verify state values match the original C constants so that downstream
    // code checking `decoder._state == 4` / `decoder._state == 5` still works.
    assert_eq!(ChunkedState::ChunkSize as i8, 0);
    assert_eq!(ChunkedState::ChunkExtension as i8, 1);
    assert_eq!(ChunkedState::ChunkData as i8, 2);
    assert_eq!(ChunkedState::ChunkCrlf as i8, 3);
    assert_eq!(ChunkedState::TrailerLineHead as i8, 4);
    assert_eq!(ChunkedState::TrailerLineMiddle as i8, 5);
    assert_eq!(ChunkedState::TrailerFinalCrlf as i8, 6);
}

#[test]
fn phr_decode_chunked_is_in_data_returns_correctly() {
    let mut decoder = ChunkedDecoder {
        _state: ChunkedState::ChunkData,
        ..Default::default()
    };
    assert_eq!(bun_picohttp::phr_decode_chunked_is_in_data(&mut decoder), 1);

    decoder._state = ChunkedState::ChunkSize;
    assert_eq!(bun_picohttp::phr_decode_chunked_is_in_data(&mut decoder), 0);
}

// ──────────────────────────────────────────────────────────────────────────
// Bytes read
// ──────────────────────────────────────────────────────────────────────────

#[test]
fn request_bytes_read_excludes_body() {
    let raw = b"GET / HTTP/1.1\r\nHost: test\r\n\r\nbody-after";
    let mut headers = [Header::ZERO; 4];
    let req = Request::parse(raw, &mut headers).unwrap();
    // bytes_read should be the offset where the body starts (after \r\n\r\n)
    assert_eq!(req.bytes_read as usize, raw.len() - b"body-after".len());
}

#[test]
fn response_bytes_read_excludes_body() {
    let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nOK";
    let mut headers = [Header::ZERO; 4];
    let resp = Response::parse(raw, &mut headers).unwrap();
    assert_eq!(resp.bytes_read as usize, raw.len() - 2);
}
