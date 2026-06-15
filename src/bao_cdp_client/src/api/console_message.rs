//! ConsoleMessage — Runtime.consoleAPICalled 事件的消息。
//!
//! D 类 method 全部本地状态:
//! - `console_type() -> &str`(log/info/warning/error/debug/...)
//! - `text() -> &str`(拼接的 message 文本)
//! - `args() -> Vec<JSHandle>`
//! - `location() -> Option<ConsoleLocation>`
//!
//! @trace REQ-BAO-API-006 [class:ConsoleMessage]

use std::cell::RefCell;
use std::rc::Rc;

use serde_json::Value;

use super::js_handle::JSHandle;

/// Console 调用位置(source)。
#[derive(Debug, Clone, Default)]
pub struct ConsoleLocation {
    pub url: String,
    pub line_number: u32,
    pub column_number: u32,
}

/// ConsoleMessage 本地状态。
///
/// @trace REQ-BAO-API-006 [class:ConsoleMessage]
pub struct ConsoleMessage {
    /// Console API type(log/info/warning/error/debug/dir/table/...).
    console_type: String,
    /// 拼接后的文本(由 args.toString 拼成)。
    text: RefCell<String>,
    /// Console args(JSHandle 列表)。
    args: RefCell<Vec<Rc<JSHandle>>>,
    /// 调用位置(可选)。
    location: RefCell<Option<ConsoleLocation>>,
    /// ExecutionContext id。
    execution_context_id: RefCell<Option<String>>,
    /// 序列化参数(JSON 形式,便于日志)。
    serialized_args: RefCell<Vec<Value>>,
}

impl std::fmt::Debug for ConsoleMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConsoleMessage")
            .field("console_type", &self.console_type)
            .field("text", &self.text.borrow())
            .field("arg_count", &self.args.borrow().len())
            .finish()
    }
}

impl ConsoleMessage {
    /// 构造 ConsoleMessage。
    ///
    /// @trace REQ-BAO-API-006 [class:ConsoleMessage]
    pub fn new(console_type: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            console_type: console_type.into(),
            text: RefCell::new(text.into()),
            args: RefCell::new(Vec::new()),
            location: RefCell::new(None),
            execution_context_id: RefCell::new(None),
            serialized_args: RefCell::new(Vec::new()),
        }
    }

    /// Console type(log/info/warning/error/...)。
    ///
    /// @trace REQ-BAO-API-006 [class:ConsoleMessage]
    pub fn console_type(&self) -> String {
        self.console_type.clone()
    }

    /// Console type 字符串引用(无 clone)。
    ///
    /// @trace REQ-BAO-API-006 [class:ConsoleMessage]
    pub fn type_str(&self) -> &str {
        &self.console_type
    }

    /// 文本(本地缓存)。
    ///
    /// @trace REQ-BAO-API-006 [class:ConsoleMessage]
    pub fn text(&self) -> String {
        self.text.borrow().clone()
    }

    /// 设置 text。
    ///
    /// @trace REQ-BAO-API-006 [class:ConsoleMessage]
    pub fn set_text(&self, t: impl Into<String>) {
        *self.text.borrow_mut() = t.into();
    }

    /// Args(JSHandle 列表克隆)。
    ///
    /// @trace REQ-BAO-API-006 [class:ConsoleMessage]
    pub fn args(&self) -> Vec<Rc<JSHandle>> {
        self.args.borrow().clone()
    }

    /// 添加 arg。
    ///
    /// @trace REQ-BAO-API-006 [class:ConsoleMessage]
    pub fn add_arg(&self, h: Rc<JSHandle>) {
        self.args.borrow_mut().push(h);
    }

    /// Arg 数量。
    ///
    /// @trace REQ-BAO-API-006 [class:ConsoleMessage]
    pub fn arg_count(&self) -> usize {
        self.args.borrow().len()
    }

    /// 调用位置。
    ///
    /// @trace REQ-BAO-API-006 [class:ConsoleMessage]
    pub fn location(&self) -> Option<ConsoleLocation> {
        self.location.borrow().clone()
    }

    /// 设置 location。
    ///
    /// @trace REQ-BAO-API-006 [class:ConsoleMessage]
    pub fn set_location(&self, loc: ConsoleLocation) {
        *self.location.borrow_mut() = Some(loc);
    }

    /// ExecutionContext id。
    ///
    /// @trace REQ-BAO-API-006 [class:ConsoleMessage]
    pub fn execution_context_id(&self) -> Option<String> {
        self.execution_context_id.borrow().clone()
    }

    /// 设置 ExecutionContext id。
    ///
    /// @trace REQ-BAO-API-006 [class:ConsoleMessage]
    pub fn set_execution_context_id(&self, id: impl Into<String>) {
        *self.execution_context_id.borrow_mut() = Some(id.into());
    }

    /// 序列化 args(JSON 形式)。
    ///
    /// @trace REQ-BAO-API-006 [class:ConsoleMessage]
    pub fn serialized_args(&self) -> Vec<Value> {
        self.serialized_args.borrow().clone()
    }

    /// 添加序列化 arg。
    ///
    /// @trace REQ-BAO-API-006 [class:ConsoleMessage]
    pub fn add_serialized_arg(&self, v: Value) {
        self.serialized_args.borrow_mut().push(v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::frame::ExecutionContext;

    fn make_msg() -> ConsoleMessage {
        ConsoleMessage::new("log", "hello world")
    }

    #[test]
    fn type_and_text() {
        let m = make_msg();
        assert_eq!(m.console_type(), "log");
        assert_eq!(m.type_str(), "log");
        assert_eq!(m.text(), "hello world");
    }

    #[test]
    fn set_text() {
        let m = make_msg();
        m.set_text("updated");
        assert_eq!(m.text(), "updated");
    }

    #[test]
    fn args_start_empty() {
        let m = make_msg();
        assert_eq!(m.arg_count(), 0);
        assert!(m.args().is_empty());
    }

    #[test]
    fn add_arg() {
        let m = make_msg();
        let ctx = Rc::new(ExecutionContext::new("CTX-1".into()));
        let h = Rc::new(JSHandle::new(ctx, "OBJ-1"));
        m.add_arg(h);
        assert_eq!(m.arg_count(), 1);
        assert_eq!(m.args()[0].remote_object_id(), "OBJ-1");
    }

    #[test]
    fn location_round_trip() {
        let m = make_msg();
        assert!(m.location().is_none());
        m.set_location(ConsoleLocation {
            url: "https://example.com".into(),
            line_number: 42,
            column_number: 7,
        });
        let loc = m.location().unwrap();
        assert_eq!(loc.url, "https://example.com");
        assert_eq!(loc.line_number, 42);
        assert_eq!(loc.column_number, 7);
    }

    #[test]
    fn execution_context_id_round_trip() {
        let m = make_msg();
        assert!(m.execution_context_id().is_none());
        m.set_execution_context_id("CTX-9");
        assert_eq!(m.execution_context_id(), Some("CTX-9".into()));
    }

    #[test]
    fn serialized_args_round_trip() {
        let m = make_msg();
        assert!(m.serialized_args().is_empty());
        m.add_serialized_arg(Value::from(1));
        m.add_serialized_arg(Value::from("x"));
        assert_eq!(m.serialized_args().len(), 2);
    }
}
