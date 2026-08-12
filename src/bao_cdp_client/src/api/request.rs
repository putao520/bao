//! Request — HTTP 请求句柄(Network.requestWillBeSent 事件)。
//!
//! D 类 method 全部本地状态:
//! - `url() -> &str`
//! - `method() -> &str`
//! - `headers() -> HashMap<String,String>`
//! - `post_data() -> Option<&str>`
//! - `resource_type() -> &str`
//! - `is_navigation_request() -> bool`
//! - `redirected_from() -> Option<Rc<Request>>`
//! - `redirected_to() -> Option<Rc<Request>>`
//! - `failure() -> Option<&str>`
//! - `frame() -> Option<Rc<Frame>>`
//!
//! @trace REQ-BAO-API-006 [class:Request]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use serde_json::Value;

use super::frame::Frame;

/// Request 本地状态(Network 事件链产生的请求记录)。
///
/// @trace REQ-BAO-API-006 [class:Request]
pub struct Request {
    /// 请求 ID(Network.requestWillBeSent 的 requestId)。
    id: String,
    /// URL。
    url: RefCell<String>,
    /// HTTP method。
    method: RefCell<String>,
    /// 请求头(本地缓存)。
    headers: RefCell<HashMap<String, String>>,
    /// POST body(本地缓存)。
    post_data: RefCell<Option<String>>,
    /// 资源类型(Document/Script/XHR/Fetch/...)。
    resource_type: RefCell<String>,
    /// 是否导航请求。
    is_navigation: RefCell<bool>,
    /// redirect 链(本地引用)。
    redirected_from: RefCell<Option<Weak<Request>>>,
    redirected_to: RefCell<Option<Rc<Request>>>,
    /// failure 文本(请求失败时填入)。
    failure: RefCell<Option<String>>,
    /// 关联 Response(本地弱引用)。
    response: RefCell<Option<Weak<super::response::Response>>>,
    /// 所属 Frame。
    frame: RefCell<Option<Weak<Frame>>>,
    /// 是否有 post data JSON(本地缓存)。
    post_data_json: RefCell<Option<Value>>,
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Request")
            .field("id", &self.id)
            .field("url", &self.url.borrow())
            .field("method", &self.method.borrow())
            .field("resource_type", &self.resource_type.borrow())
            .field("is_navigation", &self.is_navigation.borrow())
            .finish()
    }
}

impl Request {
    /// 构造新 Request(初始全空,由事件填入)。
    ///
    /// @trace REQ-BAO-API-006 [class:Request]
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            url: RefCell::new(String::new()),
            method: RefCell::new("GET".to_string()),
            headers: RefCell::new(HashMap::new()),
            post_data: RefCell::new(None),
            resource_type: RefCell::new("Other".to_string()),
            is_navigation: RefCell::new(false),
            redirected_from: RefCell::new(None),
            redirected_to: RefCell::new(None),
            failure: RefCell::new(None),
            response: RefCell::new(None),
            frame: RefCell::new(None),
            post_data_json: RefCell::new(None),
        }
    }

    /// 请求 ID。
    ///
    /// @trace REQ-BAO-API-006 [class:Request]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// URL(本地缓存)。
    ///
    /// @trace REQ-BAO-API-006 [class:Request]
    pub fn url(&self) -> String {
        self.url.borrow().clone()
    }

    /// 设置 URL(requestWillBeSent 事件填入)。
    ///
    /// @trace REQ-BAO-API-006 [class:Request]
    pub fn set_url(&self, url: impl Into<String>) {
        *self.url.borrow_mut() = url.into();
    }

    /// HTTP method。
    ///
    /// @trace REQ-BAO-API-006 [class:Request]
    pub fn method(&self) -> String {
        self.method.borrow().clone()
    }

    /// 设置 method。
    ///
    /// @trace REQ-BAO-API-006 [class:Request]
    pub fn set_method(&self, m: impl Into<String>) {
        *self.method.borrow_mut() = m.into();
    }

    /// 请求头(克隆)。
    ///
    /// @trace REQ-BAO-API-006 [class:Request]
    pub fn headers(&self) -> HashMap<String, String> {
        self.headers.borrow().clone()
    }

    /// 设置 / 替换 headers。
    ///
    /// @trace REQ-BAO-API-006 [class:Request]
    pub fn set_headers(&self, h: HashMap<String, String>) {
        *self.headers.borrow_mut() = h;
    }

    /// 添加单个 header。
    ///
    /// @trace REQ-BAO-API-006 [class:Request]
    pub fn add_header(&self, name: impl Into<String>, value: impl Into<String>) {
        self.headers.borrow_mut().insert(name.into(), value.into());
    }

    /// POST body。
    ///
    /// @trace REQ-BAO-API-006 [class:Request]
    pub fn post_data(&self) -> Option<String> {
        self.post_data.borrow().clone()
    }

    /// 设置 POST body。
    ///
    /// @trace REQ-BAO-API-006 [class:Request]
    pub fn set_post_data(&self, p: impl Into<String>) {
        *self.post_data.borrow_mut() = Some(p.into());
    }

    /// POST body 的 JSON 表示(本地缓存)。
    ///
    /// @trace REQ-BAO-API-006 [class:Request]
    pub fn post_data_json(&self) -> Option<Value> {
        self.post_data_json.borrow().clone()
    }

    /// 设置 POST JSON。
    ///
    /// @trace REQ-BAO-API-006 [class:Request]
    pub fn set_post_data_json(&self, v: Value) {
        *self.post_data_json.borrow_mut() = Some(v);
    }

    /// 资源类型。
    ///
    /// @trace REQ-BAO-API-006 [class:Request]
    pub fn resource_type(&self) -> String {
        self.resource_type.borrow().clone()
    }

    /// 设置 resource type。
    ///
    /// @trace REQ-BAO-API-006 [class:Request]
    pub fn set_resource_type(&self, t: impl Into<String>) {
        *self.resource_type.borrow_mut() = t.into();
    }

    /// 是否导航请求。
    ///
    /// @trace REQ-BAO-API-006 [class:Request]
    pub fn is_navigation_request(&self) -> bool {
        *self.is_navigation.borrow()
    }

    /// 设置 is_navigation。
    ///
    /// @trace REQ-BAO-API-006 [class:Request]
    pub fn set_navigation(&self, n: bool) {
        *self.is_navigation.borrow_mut() = n;
    }

    /// 重定向来源(本地 weak)。
    ///
    /// @trace REQ-BAO-API-006 [class:Request]
    pub fn redirected_from(&self) -> Option<Rc<Request>> {
        self.redirected_from
            .borrow()
            .as_ref()
            .and_then(|w| w.upgrade())
    }

    /// 设置 redirected_from。
    ///
    /// @trace REQ-BAO-API-006 [class:Request]
    pub fn set_redirected_from(&self, prev: Weak<Request>) {
        *self.redirected_from.borrow_mut() = Some(prev);
    }

    /// 重定向目标。
    ///
    /// @trace REQ-BAO-API-006 [class:Request]
    pub fn redirected_to(&self) -> Option<Rc<Request>> {
        self.redirected_to.borrow().clone()
    }

    /// 设置 redirected_to。
    ///
    /// @trace REQ-BAO-API-006 [class:Request]
    pub fn set_redirected_to(&self, next: Rc<Request>) {
        *self.redirected_to.borrow_mut() = Some(next);
    }

    /// 失败文本。
    ///
    /// @trace REQ-BAO-API-006 [class:Request]
    pub fn failure(&self) -> Option<String> {
        self.failure.borrow().clone()
    }

    /// 设置 failure。
    ///
    /// @trace REQ-BAO-API-006 [class:Request]
    pub fn set_failure(&self, msg: impl Into<String>) {
        *self.failure.borrow_mut() = Some(msg.into());
    }

    /// 关联 Response(weak 引用,响应可能已 drop)。
    ///
    /// @trace REQ-BAO-API-006 [class:Request]
    pub fn response(&self) -> Option<Rc<super::response::Response>> {
        self.response.borrow().as_ref().and_then(|w| w.upgrade())
    }

    /// 设置 Response(双向链接,responseCreated 事件填入)。
    ///
    /// @trace REQ-BAO-API-006 [class:Request]
    pub fn set_response(&self, r: Weak<super::response::Response>) {
        *self.response.borrow_mut() = Some(r);
    }

    /// 所属 Frame(weak)。
    ///
    /// @trace REQ-BAO-API-006 [class:Request]
    pub fn frame(&self) -> Option<Rc<Frame>> {
        self.frame.borrow().as_ref().and_then(|w| w.upgrade())
    }

    /// 设置 frame。
    ///
    /// @trace REQ-BAO-API-006 [class:Request]
    pub fn set_frame(&self, f: Weak<Frame>) {
        *self.frame.borrow_mut() = Some(f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_returns() {
        let r = Request::new("REQ-1");
        assert_eq!(r.id(), "REQ-1");
    }

    #[test]
    fn url_round_trip() {
        let r = Request::new("REQ-1");
        assert_eq!(r.url(), "");
        r.set_url("https://example.com");
        assert_eq!(r.url(), "https://example.com");
    }

    #[test]
    fn method_default_get() {
        let r = Request::new("REQ-1");
        assert_eq!(r.method(), "GET");
        r.set_method("POST");
        assert_eq!(r.method(), "POST");
    }

    #[test]
    fn headers_round_trip() {
        let r = Request::new("REQ-1");
        assert_eq!(r.headers().len(), 0);
        r.add_header("content-type", "application/json");
        r.add_header("x-foo", "bar");
        assert_eq!(r.headers().len(), 2);
        assert_eq!(
            r.headers().get("content-type"),
            Some(&"application/json".to_string())
        );
    }

    #[test]
    fn post_data_round_trip() {
        let r = Request::new("REQ-1");
        assert!(r.post_data().is_none());
        r.set_post_data("hello");
        assert_eq!(r.post_data(), Some("hello".into()));
    }

    #[test]
    fn resource_type_default_other() {
        let r = Request::new("REQ-1");
        assert_eq!(r.resource_type(), "Other");
        r.set_resource_type("XHR");
        assert_eq!(r.resource_type(), "XHR");
    }

    #[test]
    fn is_navigation_default_false() {
        let r = Request::new("REQ-1");
        assert!(!r.is_navigation_request());
        r.set_navigation(true);
        assert!(r.is_navigation_request());
    }

    #[test]
    fn failure_round_trip() {
        let r = Request::new("REQ-1");
        assert!(r.failure().is_none());
        r.set_failure("net::ERR_FAILED");
        assert_eq!(r.failure(), Some("net::ERR_FAILED".into()));
    }

    #[test]
    fn redirect_chain() {
        let prev = Rc::new(Request::new("REQ-1"));
        let next = Rc::new(Request::new("REQ-2"));
        next.set_redirected_from(Rc::downgrade(&prev));
        prev.set_redirected_to(next.clone());
        assert_eq!(next.redirected_from().unwrap().id(), "REQ-1");
        assert_eq!(prev.redirected_to().unwrap().id(), "REQ-2");
    }

    #[test]
    fn post_data_json() {
        let r = Request::new("REQ-1");
        assert!(r.post_data_json().is_none());
        r.set_post_data_json(serde_json::json!({"key": "val"}));
        assert_eq!(r.post_data_json().unwrap()["key"], "val");
    }
}
