// @trace REQ-ENG-007 [entity:HttpClientBridge]
//! Synchronous HTTP client bridge using bun_http::AsyncHTTP::init_sync + send_sync().
//!
//! Provides a simple synchronous HTTP request function that can be called
//! from anywhere in bao_runtime without needing SpiderMonkey context.

use bun_core::MutableString;
use bun_http::header_builder::HeaderBuilder;
use bun_http::{AsyncHTTP, FetchRedirect, Method};
use bun_url::URL;

/// Simplified HTTP response type extracted from picohttp::Response.
/// Owns all data (no borrowed lifetime) so it can be stored and used freely.
pub struct HttpResponse {
    /// HTTP status code (e.g. 200, 404, 500).
    pub status_code: u32,
    /// Status text (e.g. "OK", "Not Found").
    pub status_text: String,
    /// Response headers as (name, value) pairs.
    pub headers: Vec<(String, String)>,
    /// Response body as bytes.
    pub body: Vec<u8>,
}

/// Perform a synchronous HTTP request via bun_http::AsyncHTTP::send_sync().
///
/// This function:
/// 1. Parses the URL via bun_url::URL::parse
/// 2. Builds request headers via HeaderBuilder
/// 3. Initializes AsyncHTTP via init_sync()
/// 4. Executes the request via send_sync() (blocking)
/// 5. Extracts the response into an owned HttpResponse
pub fn http_request(
    method: Method,
    url: &str,
    headers: &[(String, String)],
    body: Option<&[u8]>,
) -> ::std::result::Result<HttpResponse, String> {
    // Parse URL from bytes
    let url_bytes = url.as_bytes();
    let parsed_url = URL::parse(url_bytes);

    // Build header entries via HeaderBuilder: count -> allocate -> append
    let mut hb = HeaderBuilder::default();
    for (name, value) in headers {
        hb.count(name.as_bytes(), value.as_bytes());
    }
    if let ::std::result::Result::Err(e) = hb.allocate() {
        return ::std::result::Result::Err(format!("Header allocation failed: {:?}", e));
    }
    for (name, value) in headers {
        hb.append(name.as_bytes(), value.as_bytes());
    }

    let entry_list = hb.entries;
    let headers_buf: &[u8] = unsafe {
        if let Some(ptr) = hb.content.ptr {
            ::std::slice::from_raw_parts(ptr.as_ptr(), hb.content.len)
        } else {
            &[]
        }
    };

    // Allocate response buffer — send_sync writes the response body here
    let response_buffer = Box::into_raw(Box::new(MutableString::default()));

    let body_slice: &[u8] = body.unwrap_or_default();

    let mut async_http = AsyncHTTP::init_sync(
        method,
        parsed_url,
        entry_list,
        headers_buf,
        response_buffer,
        body_slice,
        None,  // http_proxy
        None,  // hostname
        FetchRedirect::Follow,
    );

    let result = async_http.send_sync()
        .map_err(|e| format!("{:?}", e))?;

    // Read body from response_buffer before reclaiming it
    let body: Vec<u8> = unsafe { (*response_buffer).list.clone() };

    // Reclaim response buffer
    unsafe {
        drop(Box::from_raw(response_buffer));
    }

    // Extract fields from picohttp::Response into owned HttpResponse
    let status_code = result.status_code;
    let status_text = ::std::str::from_utf8(result.status)
        .unwrap_or("")
        .to_string();

    let headers: Vec<(String, String)> = result.headers.list.iter().map(|h| {
        let name = ::std::str::from_utf8(h.name()).unwrap_or("").to_string();
        let value = ::std::str::from_utf8(h.value()).unwrap_or("").to_string();
        (name, value)
    }).collect();

    ::std::result::Result::Ok(HttpResponse {
        status_code,
        status_text,
        headers,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_method_roundtrip() {
        assert_eq!(Method::GET.as_str(), "GET");
        assert_eq!(Method::POST.as_str(), "POST");
        assert_eq!(Method::PUT.as_str(), "PUT");
        assert_eq!(Method::DELETE.as_str(), "DELETE");
        assert_eq!(Method::PATCH.as_str(), "PATCH");
        assert_eq!(Method::HEAD.as_str(), "HEAD");
    }

    #[test]
    fn test_http_response_construction() {
        let resp = HttpResponse {
            status_code: 200,
            status_text: "OK".to_string(),
            headers: vec![("Content-Type".to_string(), "text/html".to_string())],
            body: b"hello".to_vec(),
        };
        assert_eq!(resp.status_code, 200);
        assert_eq!(resp.status_text, "OK");
        assert_eq!(resp.headers.len(), 1);
        assert_eq!(resp.body, b"hello".to_vec());
    }
}