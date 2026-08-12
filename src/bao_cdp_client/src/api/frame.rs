//! Frame — 浏览器 Frame(tree 节点)。
//!
//! D 类 method(全部本地状态,无 CDP 往返):
//! - `execution_context() -> Rc<ExecutionContext>`(本地引用)
//! - `is_detached() -> bool`(本地标记)
//! - `child_frames() -> Vec<Rc<Frame>>`(本地树)
//! - `parent_frame() -> Option<Rc<Frame>>`(本地引用)
//! - `name() -> &str`(本地缓存)
//! - `url() -> &str`(本地缓存)
//! - `id() -> &str`(frame ID)
//! - `is_main_frame() -> bool`(本地标记)
//! - `page() -> Rc<Page>`(本地引用)
//!
//! @trace REQ-BAO-API-006 [class:Frame]

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use serde_json::Value;

use super::event_emitter::{EventEmitter, EventEmitterInner};
use super::page::Page;

/// ExecutionContext 是 JSHandle 的所属上下文(对应 Page 的 isolated world)。
///
/// 本 TASK 内简化为 ID holder;真实 ExecutionContext 持有
/// `Runtime.executionContextCreated` 事件返回的 uniqueId/name/origin/auxData。
///
/// @trace REQ-BAO-API-006 [class:ExecutionContext]
#[derive(Debug, Clone)]
pub struct ExecutionContext {
    id: String,
    /// 上下文名称(如 "main"、"isolated")。
    name: String,
    /// Origin(如 "https://example.com")。
    origin: String,
    /// 缓存的辅助数据(auxData from Runtime.executionContextCreated)。
    aux_data: RefCell<Option<Value>>,
}

impl ExecutionContext {
    /// 构造 ExecutionContext。
    ///
    /// @trace REQ-BAO-API-006 [class:ExecutionContext]
    pub fn new(id: String) -> Self {
        Self {
            id,
            name: String::new(),
            origin: String::new(),
            aux_data: RefCell::new(None),
        }
    }

    /// 构造带 name/origin。
    ///
    /// @trace REQ-BAO-API-006 [class:ExecutionContext]
    pub fn with_name_origin(
        id: String,
        name: impl Into<String>,
        origin: impl Into<String>,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            origin: origin.into(),
            aux_data: RefCell::new(None),
        }
    }

    /// 上下文 ID。
    ///
    /// @trace REQ-BAO-API-006 [class:ExecutionContext]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// 名称。
    ///
    /// @trace REQ-BAO-API-006 [class:ExecutionContext]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Origin。
    ///
    /// @trace REQ-BAO-API-006 [class:ExecutionContext]
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// auxData。
    ///
    /// @trace REQ-BAO-API-006 [class:ExecutionContext]
    pub fn aux_data(&self) -> Option<Value> {
        self.aux_data.borrow().clone()
    }

    /// 设置 auxData。
    ///
    /// @trace REQ-BAO-API-006 [class:ExecutionContext]
    pub fn set_aux_data(&self, data: Value) {
        *self.aux_data.borrow_mut() = Some(data);
    }
}

/// Frame 树节点。
///
/// 持有 frame ID / name / URL / parent/children / ExecutionContext / page 弱引用。
///
/// @trace REQ-BAO-API-006 [class:Frame]
pub struct Frame {
    /// Frame ID(CDP Frame.frameNavigated 返回的 frame.id)。
    id: String,
    /// Frame name(iframe 的 name 属性)。
    name: RefCell<String>,
    /// 当前 URL。
    url: RefCell<String>,
    /// 父 Frame(根 frame 为 None)。
    parent: RefCell<Option<Weak<Frame>>>,
    /// 子 Frame 列表。
    children: RefCell<Vec<Rc<Frame>>>,
    /// 所属 ExecutionContext(由 Runtime.executionContextCreated 事件填入)。
    execution_context: RefCell<Option<Rc<ExecutionContext>>>,
    /// 是否已 detached(Page.frameDetached 事件标记)。
    detached: RefCell<bool>,
    /// 是否主 frame。
    is_main: bool,
    /// 所属 Page(Weak 防循环)。
    page: Weak<Page>,
    /// EventEmitter inner。
    events: Rc<EventEmitterInner>,
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Frame")
            .field("id", &self.id)
            .field("name", &self.name.borrow())
            .field("url", &self.url.borrow())
            .field("is_main", &self.is_main)
            .field("detached", &self.detached.borrow())
            .field("child_count", &self.children.borrow().len())
            .finish()
    }
}

impl Frame {
    /// 构造 Frame。`is_main=true` 表示主 frame。
    ///
    /// @trace REQ-BAO-API-006 [class:Frame]
    pub fn new(id: impl Into<String>, is_main: bool, page: Weak<Page>) -> Self {
        Self {
            id: id.into(),
            name: RefCell::new(String::new()),
            url: RefCell::new(String::new()),
            parent: RefCell::new(None),
            children: RefCell::new(Vec::new()),
            execution_context: RefCell::new(None),
            detached: RefCell::new(false),
            is_main,
            page,
            events: Rc::new(EventEmitterInner::new()),
        }
    }

    /// Frame ID。
    ///
    /// @trace REQ-BAO-API-006 [class:Frame]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// 是否主 frame。
    ///
    /// @trace REQ-BAO-API-006 [class:Frame]
    pub fn is_main_frame(&self) -> bool {
        self.is_main
    }

    /// Frame name(本地缓存)。
    ///
    /// @trace REQ-BAO-API-006 [class:Frame]
    pub fn name(&self) -> String {
        self.name.borrow().clone()
    }

    /// 设置 Frame name(Page.frameNavigated 等事件填入)。
    ///
    /// @trace REQ-BAO-API-006 [class:Frame]
    pub fn set_name(&self, name: impl Into<String>) {
        *self.name.borrow_mut() = name.into();
    }

    /// 当前 URL(本地缓存)。
    ///
    /// @trace REQ-BAO-API-006 [class:Frame]
    pub fn url(&self) -> String {
        self.url.borrow().clone()
    }

    /// 设置 URL(frameNavigated 事件填入)。
    ///
    /// @trace REQ-BAO-API-006 [class:Frame]
    pub fn set_url(&self, url: impl Into<String>) {
        *self.url.borrow_mut() = url.into();
    }

    /// 父 Frame(根 frame 返回 None)。
    ///
    /// @trace REQ-BAO-API-006 [class:Frame]
    pub fn parent_frame(&self) -> Option<Rc<Frame>> {
        self.parent.borrow().as_ref().and_then(|w| w.upgrade())
    }

    /// 添加子 Frame。同步设置 child 的 parent 为自身弱引用。
    ///
    /// @trace REQ-BAO-API-006 [class:Frame]
    pub fn add_child(&self, child: Rc<Frame>, self_ref: Weak<Frame>) {
        // 设置 child 的 parent
        *child.parent.borrow_mut() = Some(self_ref);
        self.children.borrow_mut().push(child);
    }

    /// 移除子 Frame。
    ///
    /// @trace REQ-BAO-API-006 [class:Frame]
    pub fn remove_child(&self, child_id: &str) {
        self.children.borrow_mut().retain(|c| c.id() != child_id);
    }

    /// 子 Frame 列表(克隆,本地树)。
    ///
    /// @trace REQ-BAO-API-006 [class:Frame]
    pub fn child_frames(&self) -> Vec<Rc<Frame>> {
        self.children.borrow().clone()
    }

    /// 所属 ExecutionContext(本地缓存)。
    ///
    /// @trace REQ-BAO-API-006 [class:Frame]
    pub fn execution_context(&self) -> Option<Rc<ExecutionContext>> {
        self.execution_context.borrow().clone()
    }

    /// 设置 ExecutionContext(executionContextCreated 事件填入)。
    ///
    /// @trace REQ-BAO-API-006 [class:Frame]
    pub fn set_execution_context(&self, ctx: Rc<ExecutionContext>) {
        *self.execution_context.borrow_mut() = Some(ctx);
    }

    /// 是否已 detached(本地标记)。
    ///
    /// @trace REQ-BAO-API-006 [class:Frame]
    pub fn is_detached(&self) -> bool {
        *self.detached.borrow()
    }

    /// 标记 detached(frameDetached 事件填入)。
    ///
    /// @trace REQ-BAO-API-006 [class:Frame]
    pub fn set_detached(&self, detached: bool) {
        *self.detached.borrow_mut() = detached;
    }

    /// 所属 Page(Weak::upgrade 失败时返回 None)。
    ///
    /// @trace REQ-BAO-API-006 [class:Frame]
    pub fn page(&self) -> Option<Rc<Page>> {
        self.page.upgrade()
    }

    /// EventEmitter inner 引用(供 Page 上层转发)。
    ///
    /// @trace REQ-BAO-API-006 [class:Frame]
    pub fn events(&self) -> &Rc<EventEmitterInner> {
        &self.events
    }
}

impl EventEmitter for Frame {
    delegate_event_emitter!(self, events);
}

#[cfg(test)]
mod tests {
    use super::super::event_emitter::EventHandler;
    use super::*;
    use crate::api::browser::Browser as HighLevelBrowser;
    use crate::api::browser_context::BrowserContext;
    use crate::api::page::Page;
    use std::rc::Rc;

    fn make_frame(is_main: bool, page: Weak<Page>) -> Rc<Frame> {
        Rc::new(Frame::new("FRAME-1", is_main, page))
    }

    fn make_page() -> Rc<Page> {
        let browser = Rc::new(HighLevelBrowser::new_for_test("ws://x"));
        let ctx = HighLevelBrowser::new_context_for_test(&browser);
        BrowserContext::new_page_for_test(&ctx)
    }

    #[test]
    fn frame_id_and_is_main() {
        let page = make_page();
        let f = make_frame(true, Rc::downgrade(&page));
        assert_eq!(f.id(), "FRAME-1");
        assert!(f.is_main_frame());
    }

    #[test]
    fn frame_name_local() {
        let page = make_page();
        let f = make_frame(true, Rc::downgrade(&page));
        assert_eq!(f.name(), "");
        f.set_name("myIframe");
        assert_eq!(f.name(), "myIframe");
    }

    #[test]
    fn frame_url_local() {
        let page = make_page();
        let f = make_frame(true, Rc::downgrade(&page));
        assert_eq!(f.url(), "");
        f.set_url("https://example.com");
        assert_eq!(f.url(), "https://example.com");
    }

    #[test]
    fn frame_detached_flag() {
        let page = make_page();
        let f = make_frame(true, Rc::downgrade(&page));
        assert!(!f.is_detached());
        f.set_detached(true);
        assert!(f.is_detached());
    }

    #[test]
    fn frame_execution_context_local() {
        let page = make_page();
        let f = make_frame(true, Rc::downgrade(&page));
        assert!(f.execution_context().is_none());
        let ctx = Rc::new(ExecutionContext::new("CTX-1".into()));
        f.set_execution_context(ctx.clone());
        assert_eq!(f.execution_context().unwrap().id(), "CTX-1");
    }

    #[test]
    fn frame_parent_and_children() {
        let page = make_page();
        let parent = Rc::new(Frame::new("P", true, Rc::downgrade(&page)));
        let child = Rc::new(Frame::new("C", false, Rc::downgrade(&page)));
        parent.add_child(child.clone(), Rc::downgrade(&parent));
        assert!(parent.parent_frame().is_none());
        assert_eq!(parent.child_frames().len(), 1);
        assert_eq!(
            child.parent_frame().map(|f| f.id().to_string()),
            Some("P".into())
        );
        parent.remove_child("C");
        assert_eq!(parent.child_frames().len(), 0);
        // child 的 parent weak 引用未清除,upgrade 仍可成功(parent 仍存活)
        assert!(child.parent_frame().is_some());
    }

    #[test]
    fn frame_page_upgrade() {
        let page = make_page();
        let f = make_frame(true, Rc::downgrade(&page));
        assert!(f.page().is_some());
        drop(page);
        // page 已 drop
        assert!(f.page().is_none());
    }

    #[test]
    fn execution_context_data_holder() {
        let ctx = ExecutionContext::with_name_origin("CTX-1".into(), "main", "https://x");
        assert_eq!(ctx.id(), "CTX-1");
        assert_eq!(ctx.name(), "main");
        assert_eq!(ctx.origin(), "https://x");
        assert!(ctx.aux_data().is_none());
        ctx.set_aux_data(Value::from(42));
        assert_eq!(ctx.aux_data(), Some(Value::from(42)));
    }

    #[test]
    fn frame_event_emitter_via_inner() {
        let page = make_page();
        let f = make_frame(true, Rc::downgrade(&page));
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let c = counter.clone();
        let h: EventHandler = std::sync::Arc::new(move |_| {
            c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        f.events().on("fnav", h);
        f.events().emit("fnav", &[]);
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
