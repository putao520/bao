//! ElementHandle — DOM Element 句柄(继承 JSHandle)。
//!
//! ElementHandle 是特殊的 JSHandle,持有 DOM Node 的引用。D 类 method 全部本地状态:
//! - `as_element() -> Option<&ElementHandle>`(override → Some(&self))
//! - `is_visible() -> bool`(本地缓存)
//! - `is_hidden() -> bool`(本地缓存)
//! - `is_enabled() -> bool`(本地缓存)
//! - `is_disabled() -> bool`(本地缓存)
//! - `is_checked() -> bool`(本地缓存)
//! - `is_editable() -> bool`(本地缓存)
//! - `bounding_box() -> Option<BoundingBox>`(本地缓存)
//! - `content_frame() -> Option<Rc<Frame>>`(本地缓存)
//! - `owner_frame() -> Rc<Frame>`(本地引用)
//! - `scroll_into_view_needed() -> bool`(本地缓存)
//!
//! @trace REQ-BAO-API-006 [class:ElementHandle]

use std::cell::RefCell;
use std::rc::Rc;

use super::frame::Frame;
use super::js_handle::JSHandle;

/// Element 的 BoundingBox(布局矩形)。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoundingBox {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// ElementHandle 本地状态(包装 JSHandle)。
///
/// 注意:由于 Rust 没有"继承",我们用"组合 + Deref"模式。
/// ElementHandle 内部持有 `Rc<JSHandle>`,通过显式 getter 暴露 JSHandle API。
///
/// @trace REQ-BAO-API-006 [class:ElementHandle]
pub struct ElementHandle {
    /// 内部 JSHandle(作为基类)。
    js: Rc<JSHandle>,
    /// 所属 Frame(此 element 所在的 frame)。
    owner_frame: Rc<Frame>,
    /// 本地缓存:element 是否可见(由 B 类 method 填入)。
    visible: RefCell<Option<bool>>,
    /// 本地缓存:element 是否启用。
    enabled: RefCell<Option<bool>>,
    /// 本地缓存:element 是否选中(checkbox/radio)。
    checked: RefCell<Option<bool>>,
    /// 本地缓存:element 是否可编辑。
    editable: RefCell<Option<bool>>,
    /// 本地缓存:bounding box。
    bbox: RefCell<Option<BoundingBox>>,
    /// 本地缓存:内容 frame(对 iframe element 有效)。
    content_frame: RefCell<Option<Rc<Frame>>>,
    /// 本地缓存:是否需要 scroll into view。
    scroll_needed: RefCell<bool>,
}

impl std::fmt::Debug for ElementHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ElementHandle")
            .field("remote_object_id", &self.js.remote_object_id())
            .field("owner_frame", &self.owner_frame.id())
            .field("visible", &self.visible.borrow())
            .field("bbox", &self.bbox.borrow())
            .finish()
    }
}

impl ElementHandle {
    /// 构造 ElementHandle。
    ///
    /// @trace REQ-BAO-API-006 [class:ElementHandle]
    pub fn new(js: Rc<JSHandle>, owner_frame: Rc<Frame>) -> Self {
        Self {
            js,
            owner_frame,
            visible: RefCell::new(None),
            enabled: RefCell::new(None),
            checked: RefCell::new(None),
            editable: RefCell::new(None),
            bbox: RefCell::new(None),
            content_frame: RefCell::new(None),
            scroll_needed: RefCell::new(false),
        }
    }

    /// 引用内部 JSHandle(基类访问)。
    ///
    /// @trace REQ-BAO-API-006 [class:ElementHandle]
    pub fn as_js_handle(&self) -> &JSHandle {
        &self.js
    }

    /// 类型 override。ElementHandle 返回 `Some(&self)`。
    ///
    /// 注:由于 ElementHandle 持有 JSHandle(而非相反方向),JSHandle.as_element
    /// 不能直接返回。调用方拿到 JSHandle 后,可通过 `try_as_element` 方法
    /// 或在 Page/Frame 上使用 ElementHandle 列表。
    ///
    /// @trace REQ-BAO-API-006 [class:ElementHandle]
    pub fn as_element(&self) -> Option<&ElementHandle> {
        Some(self)
    }

    /// 所属 Frame。
    ///
    /// @trace REQ-BAO-API-006 [class:ElementHandle]
    pub fn owner_frame(&self) -> Rc<Frame> {
        self.owner_frame.clone()
    }

    /// 是否可见(本地缓存)。
    ///
    /// @trace REQ-BAO-API-006 [class:ElementHandle]
    pub fn is_visible(&self) -> Option<bool> {
        *self.visible.borrow()
    }

    /// 设置 visible 缓存。
    ///
    /// @trace REQ-BAO-API-006 [class:ElementHandle]
    pub fn set_visible(&self, v: bool) {
        *self.visible.borrow_mut() = Some(v);
    }

    /// 是否隐藏(本地缓存)。
    ///
    /// @trace REQ-BAO-API-006 [class:ElementHandle]
    pub fn is_hidden(&self) -> Option<bool> {
        self.visible.borrow().map(|v| !v)
    }

    /// 是否启用(本地缓存)。
    ///
    /// @trace REQ-BAO-API-006 [class:ElementHandle]
    pub fn is_enabled(&self) -> Option<bool> {
        *self.enabled.borrow()
    }

    /// 设置 enabled 缓存。
    ///
    /// @trace REQ-BAO-API-006 [class:ElementHandle]
    pub fn set_enabled(&self, e: bool) {
        *self.enabled.borrow_mut() = Some(e);
    }

    /// 是否禁用(本地缓存)。
    ///
    /// @trace REQ-BAO-API-006 [class:ElementHandle]
    pub fn is_disabled(&self) -> Option<bool> {
        self.enabled.borrow().map(|e| !e)
    }

    /// 是否选中(checkbox/radio,本地缓存)。
    ///
    /// @trace REQ-BAO-API-006 [class:ElementHandle]
    pub fn is_checked(&self) -> Option<bool> {
        *self.checked.borrow()
    }

    /// 设置 checked 缓存。
    ///
    /// @trace REQ-BAO-API-006 [class:ElementHandle]
    pub fn set_checked(&self, c: bool) {
        *self.checked.borrow_mut() = Some(c);
    }

    /// 是否可编辑(本地缓存)。
    ///
    /// @trace REQ-BAO-API-006 [class:ElementHandle]
    pub fn is_editable(&self) -> Option<bool> {
        *self.editable.borrow()
    }

    /// 设置 editable 缓存。
    ///
    /// @trace REQ-BAO-API-006 [class:ElementHandle]
    pub fn set_editable(&self, e: bool) {
        *self.editable.borrow_mut() = Some(e);
    }

    /// Bounding box(本地缓存)。
    ///
    /// @trace REQ-BAO-API-006 [class:ElementHandle]
    pub fn bounding_box(&self) -> Option<BoundingBox> {
        *self.bbox.borrow()
    }

    /// 设置 bbox 缓存。
    ///
    /// @trace REQ-BAO-API-006 [class:ElementHandle]
    pub fn set_bounding_box(&self, b: BoundingBox) {
        *self.bbox.borrow_mut() = Some(b);
    }

    /// 内容 frame(对 iframe element 有效,本地缓存)。
    ///
    /// @trace REQ-BAO-API-006 [class:ElementHandle]
    pub fn content_frame(&self) -> Option<Rc<Frame>> {
        self.content_frame.borrow().clone()
    }

    /// 设置 content_frame 缓存。
    ///
    /// @trace REQ-BAO-API-006 [class:ElementHandle]
    pub fn set_content_frame(&self, f: Rc<Frame>) {
        *self.content_frame.borrow_mut() = Some(f);
    }

    /// 是否需要 scroll into view(本地缓存)。
    ///
    /// @trace REQ-BAO-API-006 [class:ElementHandle]
    pub fn scroll_into_view_needed(&self) -> bool {
        *self.scroll_needed.borrow()
    }

    /// 设置 scroll_into_view_needed。
    ///
    /// @trace REQ-BAO-API-006 [class:ElementHandle]
    pub fn set_scroll_into_view_needed(&self, needed: bool) {
        *self.scroll_needed.borrow_mut() = needed;
    }

    /// Dispose —— 转发到内部 JSHandle。
    ///
    /// @trace REQ-BAO-API-006 [class:ElementHandle]
    pub fn dispose(&self) {
        self.js.dispose();
        *self.visible.borrow_mut() = None;
        *self.enabled.borrow_mut() = None;
        *self.checked.borrow_mut() = None;
        *self.editable.borrow_mut() = None;
        *self.bbox.borrow_mut() = None;
        *self.content_frame.borrow_mut() = None;
        *self.scroll_needed.borrow_mut() = false;
    }

    /// 是否已 dispose(委托 JSHandle)。
    ///
    /// @trace REQ-BAO-API-006 [class:ElementHandle]
    pub fn is_disposed(&self) -> bool {
        self.js.is_disposed()
    }

    /// remote object ID(委托 JSHandle)。
    ///
    /// @trace REQ-BAO-API-006 [class:ElementHandle]
    pub fn remote_object_id(&self) -> &str {
        self.js.remote_object_id()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::frame::ExecutionContext;
    use crate::api::page::Page;
    use std::rc::Rc;

    fn make_handle() -> (ElementHandle, Rc<Page>) {
        let browser = Rc::new(crate::api::browser::Browser::new_for_test("ws://x"));
        let ctx = crate::api::browser::Browser::new_context_for_test(&browser);
        let page = crate::api::browser_context::BrowserContext::new_page_for_test(&ctx);
        let frame = page.main_frame().clone();
        let js_handle = Rc::new(JSHandle::new(
            Rc::new(ExecutionContext::new("CTX-1".into())),
            "OBJ-1",
        ));
        (ElementHandle::new(js_handle, frame), page)
    }

    #[test]
    fn as_element_returns_some() {
        let (h, _p) = make_handle();
        assert!(h.as_element().is_some());
    }

    #[test]
    fn visibility_cache_round_trip() {
        let (h, _p) = make_handle();
        assert!(h.is_visible().is_none());
        assert!(h.is_hidden().is_none());
        h.set_visible(true);
        assert_eq!(h.is_visible(), Some(true));
        assert_eq!(h.is_hidden(), Some(false));
    }

    #[test]
    fn enabled_cache_round_trip() {
        let (h, _p) = make_handle();
        assert!(h.is_enabled().is_none());
        h.set_enabled(false);
        assert_eq!(h.is_enabled(), Some(false));
        assert_eq!(h.is_disabled(), Some(true));
    }

    #[test]
    fn checked_cache_round_trip() {
        let (h, _p) = make_handle();
        assert!(h.is_checked().is_none());
        h.set_checked(true);
        assert_eq!(h.is_checked(), Some(true));
    }

    #[test]
    fn editable_cache_round_trip() {
        let (h, _p) = make_handle();
        assert!(h.is_editable().is_none());
        h.set_editable(true);
        assert_eq!(h.is_editable(), Some(true));
    }

    #[test]
    fn bounding_box_cache() {
        let (h, _p) = make_handle();
        assert!(h.bounding_box().is_none());
        h.set_bounding_box(BoundingBox {
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 50.0,
        });
        let b = h.bounding_box().unwrap();
        assert_eq!(b.x, 10.0);
        assert_eq!(b.width, 100.0);
    }

    #[test]
    fn content_frame_cache() {
        let (h, p) = make_handle();
        assert!(h.content_frame().is_none());
        let sub_frame = Rc::new(crate::api::frame::Frame::new(
            "SUB",
            false,
            Rc::downgrade(&p),
        ));
        h.set_content_frame(sub_frame.clone());
        assert_eq!(h.content_frame().unwrap().id(), "SUB");
    }

    #[test]
    fn scroll_needed_flag() {
        let (h, _p) = make_handle();
        assert!(!h.scroll_into_view_needed());
        h.set_scroll_into_view_needed(true);
        assert!(h.scroll_into_view_needed());
    }

    #[test]
    fn dispose_clears_state() {
        let (h, _p) = make_handle();
        h.set_visible(true);
        h.set_enabled(true);
        h.set_bounding_box(BoundingBox {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        });
        h.dispose();
        assert!(h.is_disposed());
        assert!(h.is_visible().is_none());
        assert!(h.is_enabled().is_none());
        assert!(h.bounding_box().is_none());
    }

    #[test]
    fn owner_frame_returns_frame() {
        let (h, p) = make_handle();
        let f = h.owner_frame();
        // owner frame is the page's main frame
        assert!(Rc::ptr_eq(&f, &p.main_frame()));
    }

    #[test]
    fn as_js_handle_delegates() {
        let (h, _p) = make_handle();
        assert_eq!(h.as_js_handle().remote_object_id(), "OBJ-1");
    }
}
