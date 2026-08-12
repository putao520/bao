//! Response — HTTP 响应句柄(Network.responseReceived 事件)。
//!
//! D 类 method 全部本地状态:
//! - `url() -> &str`
//! - `status() -> u16`
//! - `status_text() -> &str`
//! - `ok() -> bool`(status in [200, 300))
//! - `headers() -> HashMap<String,String>`
//! - `from_cache() -> bool`
//! - `from_service_worker() -> bool`
//! - `security_details() -> Option<&SecurityDetails>`
//! - `remote_address() -> Option<&RemoteAddress>`
//! - `request() -> Rc<Request>`
//! - `body() -> Option<&str>`(本地缓存)
//!
//! @trace REQ-BAO-API-006 [class:Response]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use serde_json::Value;

use super::request::Request;

/// Security details(TLS 信息)。
#[derive(Debug, Clone, Default)]
pub struct SecurityDetails {
    pub protocol: String,
    pub subject_name: String,
    pub issuer: String,
    pub valid_from: f64,
    pub valid_to: f64,
}

/// Remote address(IP:port)。
#[derive(Debug, Clone, Default)]
pub struct RemoteAddress {
    pub ip: String,
    pub port: u16,
}

/// Response 本地状态。
///
/// @trace REQ-BAO-API-006 [class:Response]
pub struct Response {
    url: RefCell<String>,
    status: RefCell<Option<u16>>,
    status_text: RefCell<String>,
    headers: RefCell<HashMap<String, String>>,
    from_cache: RefCell<bool>,
    from_service_worker: RefCell<bool>,
    security_details: RefCell<Option<SecurityDetails>>,
    remote_address: RefCell<Option<RemoteAddress>>,
    request: RefCell<Option<Rc<Request>>>,
    body: RefCell<Option<Vec<u8>>>,
    body_text: RefCell<Option<String>>,
    body_json: RefCell<Option<Value>>,
}

impl std::fmt::Debug for Response {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Response")
            .field("url", &self.url.borrow())
            .field("status", &self.status.borrow())
            .field("from_cache", &self.from_cache.borrow())
            .field("from_service_worker", &self.from_service_worker.borrow())
            .finish()
    }
}

impl Response {
    /// 构造空 Response(由 responseReceived 事件填入)。
    ///
    /// @trace REQ-BAO-API-006 [class:Response]
    pub fn new() -> Self {
        Self {
            url: RefCell::new(String::new()),
            status: RefCell::new(None),
            status_text: RefCell::new(String::new()),
            headers: RefCell::new(HashMap::new()),
            from_cache: RefCell::new(false),
            from_service_worker: RefCell::new(false),
            security_details: RefCell::new(None),
            remote_address: RefCell::new(None),
            request: RefCell::new(None),
            body: RefCell::new(None),
            body_text: RefCell::new(None),
            body_json: RefCell::new(None),
        }
    }

    /// URL。
    ///
    /// @trace REQ-BAO-API-006 [class:Response]
    pub fn url(&self) -> String {
        self.url.borrow().clone()
    }

    /// 设置 URL。
    ///
    /// @trace REQ-BAO-API-006 [class:Response]
    pub fn set_url(&self, url: impl Into<String>) {
        *self.url.borrow_mut() = url.into();
    }

    /// Status code。
    ///
    /// @trace REQ-BAO-API-006 [class:Response]
    pub fn status(&self) -> Option<u16> {
        *self.status.borrow()
    }

    /// 设置 status。
    ///
    /// @trace REQ-BAO-API-006 [class:Response]
    pub fn set_status(&self, s: u16) {
        *self.status.borrow_mut() = Some(s);
    }

    /// Status text。
    ///
    /// @trace REQ-BAO-API-006 [class:Response]
    pub fn status_text(&self) -> String {
        self.status_text.borrow().clone()
    }

    /// 设置 status text。
    ///
    /// @trace REQ-BAO-API-006 [class:Response]
    pub fn set_status_text(&self, t: impl Into<String>) {
        *self.status_text.borrow_mut() = t.into();
    }

    /// OK = status in [200, 300)。
    ///
    /// @trace REQ-BAO-API-006 [class:Response]
    pub fn ok(&self) -> bool {
        matches!(self.status.borrow().as_ref(), Some(s) if (200..300).contains(s))
    }

    /// Headers(克隆)。
    ///
    /// @trace REQ-BAO-API-006 [class:Response]
    pub fn headers(&self) -> HashMap<String, String> {
        self.headers.borrow().clone()
    }

    /// 设置 headers。
    ///
    /// @trace REQ-BAO-API-006 [class:Response]
    pub fn set_headers(&self, h: HashMap<String, String>) {
        *self.headers.borrow_mut() = h;
    }

    /// 添加单个 header。
    ///
    /// @trace REQ-BAO-API-006 [class:Response]
    pub fn add_header(&self, name: impl Into<String>, value: impl Into<String>) {
        self.headers.borrow_mut().insert(name.into(), value.into());
    }

    /// 是否来自 cache。
    ///
    /// @trace REQ-BAO-API-006 [class:Response]
    pub fn from_cache(&self) -> bool {
        *self.from_cache.borrow()
    }

    /// 设置 from_cache。
    ///
    /// @trace REQ-BAO-API-006 [class:Response]
    pub fn set_from_cache(&self, v: bool) {
        *self.from_cache.borrow_mut() = v;
    }

    /// 是否来自 service worker。
    ///
    /// @trace REQ-BAO-API-006 [class:Response]
    pub fn from_service_worker(&self) -> bool {
        *self.from_service_worker.borrow()
    }

    /// 设置 from_service_worker。
    ///
    /// @trace REQ-BAO-API-006 [class:Response]
    pub fn set_from_service_worker(&self, v: bool) {
        *self.from_service_worker.borrow_mut() = v;
    }

    /// TLS security details(本地缓存)。
    ///
    /// @trace REQ-BAO-API-006 [class:Response]
    pub fn security_details(&self) -> Option<SecurityDetails> {
        self.security_details.borrow().clone()
    }

    /// 设置 security details。
    ///
    /// @trace REQ-BAO-API-006 [class:Response]
    pub fn set_security_details(&self, s: SecurityDetails) {
        *self.security_details.borrow_mut() = Some(s);
    }

    /// Remote address。
    ///
    /// @trace REQ-BAO-API-006 [class:Response]
    pub fn remote_address(&self) -> Option<RemoteAddress> {
        self.remote_address.borrow().clone()
    }

    /// 设置 remote address。
    ///
    /// @trace REQ-BAO-API-006 [class:Response]
    pub fn set_remote_address(&self, a: RemoteAddress) {
        *self.remote_address.borrow_mut() = Some(a);
    }

    /// 关联 Request。
    ///
    /// @trace REQ-BAO-API-006 [class:Response]
    pub fn request(&self) -> Option<Rc<Request>> {
        self.request.borrow().clone()
    }

    /// 设置 request(双向链接)。
    ///
    /// @trace REQ-BAO-API-006 [class:Response]
    pub fn set_request(&self, r: Rc<Request>) {
        *self.request.borrow_mut() = Some(r);
    }

    /// body bytes(本地缓存)。
    ///
    /// @trace REQ-BAO-API-006 [class:Response]
    pub fn body(&self) -> Option<Vec<u8>> {
        self.body.borrow().clone()
    }

    /// 设置 body bytes。
    ///
    /// @trace REQ-BAO-API-006 [class:Response]
    pub fn set_body(&self, b: Vec<u8>) {
        *self.body.borrow_mut() = Some(b);
    }

    /// body text(本地缓存)。
    ///
    /// @trace REQ-BAO-API-006 [class:Response]
    pub fn body_text(&self) -> Option<String> {
        self.body_text.borrow().clone()
    }

    /// 设置 body text。
    ///
    /// @trace REQ-BAO-API-006 [class:Response]
    pub fn set_body_text(&self, t: impl Into<String>) {
        *self.body_text.borrow_mut() = Some(t.into());
    }

    /// body JSON(本地缓存)。
    ///
    /// @trace REQ-BAO-API-006 [class:Response]
    pub fn body_json(&self) -> Option<Value> {
        self.body_json.borrow().clone()
    }

    /// 设置 body JSON。
    ///
    /// @trace REQ-BAO-API-006 [class:Response]
    pub fn set_body_json(&self, v: Value) {
        *self.body_json.borrow_mut() = Some(v);
    }
}

impl Default for Response {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_round_trip() {
        let r = Response::new();
        assert_eq!(r.url(), "");
        r.set_url("https://example.com");
        assert_eq!(r.url(), "https://example.com");
    }

    #[test]
    fn status_and_ok() {
        let r = Response::new();
        assert!(r.status().is_none());
        assert!(!r.ok());
        r.set_status(200);
        assert_eq!(r.status(), Some(200));
        assert!(r.ok());
        r.set_status(404);
        assert!(!r.ok());
        r.set_status(299);
        assert!(r.ok());
        r.set_status(300);
        assert!(!r.ok());
    }

    #[test]
    fn status_text_round_trip() {
        let r = Response::new();
        assert_eq!(r.status_text(), "");
        r.set_status_text("Not Found");
        assert_eq!(r.status_text(), "Not Found");
    }

    #[test]
    fn headers_round_trip() {
        let r = Response::new();
        assert_eq!(r.headers().len(), 0);
        r.add_header("content-type", "text/html");
        assert_eq!(r.headers().len(), 1);
        assert_eq!(
            r.headers().get("content-type"),
            Some(&"text/html".to_string())
        );
    }

    #[test]
    fn from_cache_default_false() {
        let r = Response::new();
        assert!(!r.from_cache());
        r.set_from_cache(true);
        assert!(r.from_cache());
    }

    #[test]
    fn from_service_worker_default_false() {
        let r = Response::new();
        assert!(!r.from_service_worker());
        r.set_from_service_worker(true);
        assert!(r.from_service_worker());
    }

    #[test]
    fn security_details_round_trip() {
        let r = Response::new();
        assert!(r.security_details().is_none());
        r.set_security_details(SecurityDetails {
            protocol: "TLS 1.3".into(),
            subject_name: "example.com".into(),
            issuer: "Let's Encrypt".into(),
            valid_from: 0.0,
            valid_to: 0.0,
        });
        let s = r.security_details().unwrap();
        assert_eq!(s.protocol, "TLS 1.3");
        assert_eq!(s.subject_name, "example.com");
    }

    #[test]
    fn remote_address_round_trip() {
        let r = Response::new();
        assert!(r.remote_address().is_none());
        r.set_remote_address(RemoteAddress {
            ip: "127.0.0.1".into(),
            port: 8080,
        });
        let a = r.remote_address().unwrap();
        assert_eq!(a.ip, "127.0.0.1");
        assert_eq!(a.port, 8080);
    }

    #[test]
    fn body_round_trip() {
        let r = Response::new();
        assert!(r.body().is_none());
        r.set_body(vec![1, 2, 3]);
        assert_eq!(r.body(), Some(vec![1, 2, 3]));
    }

    #[test]
    fn body_text_round_trip() {
        let r = Response::new();
        assert!(r.body_text().is_none());
        r.set_body_text("hello");
        assert_eq!(r.body_text(), Some("hello".into()));
    }

    #[test]
    fn body_json_round_trip() {
        let r = Response::new();
        assert!(r.body_json().is_none());
        r.set_body_json(serde_json::json!({"k": "v"}));
        assert_eq!(r.body_json().unwrap()["k"], "v");
    }

    #[test]
    fn request_link() {
        let r = Response::new();
        let req = Rc::new(Request::new("REQ-1"));
        r.set_request(req);
        assert_eq!(r.request().unwrap().id(), "REQ-1");
    }
}
