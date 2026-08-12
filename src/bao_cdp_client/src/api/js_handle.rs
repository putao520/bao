//! JSHandle — 远程 JS 对象句柄。
//!
//! JSHandle 是 ElementHandle 的基类。持有 `remote_object_id`(从 Runtime.evaluate
//! 或 DOM.describeNode 返回),`json_value()` 缓存调用结果,`dispose()` 释放。
//!
//! D 类 method(全部本地状态,无 CDP 往返):
//! - `as_element() -> Option<&ElementHandle>`(JSHandle 返回 None,ElementHandle override)
//! - `json_value() -> Value`(读取本地缓存)
//! - `execution_context() -> &ExecutionContext`(读取本地引用)
//! - `get_properties() -> Vec<(String, JSHandle)>`(本地缓存)
//! - `get_property(name) -> Option<&JSHandle>`(本地查找)
//! - `remote_object_id() -> &str`
//! - `is_disposed() -> bool`(本地标记)
//! - `dispose()`(本地标记 + 调用 transport 一次 — 此处仅标记)
//!
//! @trace REQ-BAO-API-006 [class:JSHandle]

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use serde_json::Value;

use super::element_handle::ElementHandle;
use super::frame::ExecutionContext;

/// JSHandle 本地状态。
///
/// @trace REQ-BAO-API-006 [class:JSHandle]
pub struct JSHandle {
    execution_context: Rc<ExecutionContext>,
    remote_object_id: String,
    /// 缓存的 JSON value。`None` 表示未请求过 / 未缓存。
    json_value_cache: RefCell<Option<Value>>,
    /// 缓存的 properties。key = property name,value = 子 JSHandle。
    properties: RefCell<HashMap<String, Rc<JSHandle>>>,
    /// 是否已 dispose。
    disposed: RefCell<bool>,
    /// 是否是 ElementHandle(JSHandle 本身永远 false,ElementHandle 永远 true)。
    is_element: bool,
}

impl std::fmt::Debug for JSHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JSHandle")
            .field("remote_object_id", &self.remote_object_id)
            .field("is_element", &self.is_element)
            .field("disposed", &self.disposed.borrow())
            .finish()
    }
}

impl JSHandle {
    /// 构造 JSHandle(初始化为空缓存)。
    ///
    /// @trace REQ-BAO-API-006 [class:JSHandle]
    pub fn new(
        execution_context: Rc<ExecutionContext>,
        remote_object_id: impl Into<String>,
    ) -> Self {
        Self {
            execution_context,
            remote_object_id: remote_object_id.into(),
            json_value_cache: RefCell::new(None),
            properties: RefCell::new(HashMap::new()),
            disposed: RefCell::new(false),
            is_element: false,
        }
    }

    /// 远程对象 ID(从 Runtime/DOM 返回的 objectId)。
    ///
    /// @trace REQ-BAO-API-006 [class:JSHandle]
    pub fn remote_object_id(&self) -> &str {
        &self.remote_object_id
    }

    /// 所属 ExecutionContext。
    ///
    /// @trace REQ-BAO-API-006 [class:JSHandle]
    pub fn execution_context(&self) -> &ExecutionContext {
        &self.execution_context
    }

    /// 是否已 dispose(本地标记)。
    ///
    /// @trace REQ-BAO-API-006 [class:JSHandle]
    pub fn is_disposed(&self) -> bool {
        *self.disposed.borrow()
    }

    /// 类型检查。JSHandle 永远返回 None;ElementHandle override 返回 `Some(&self)`。
    ///
    /// 默认实现返回 `None`。ElementHandle 调用方应通过此方法做"动态分发"。
    ///
    /// @trace REQ-BAO-API-006 [class:JSHandle]
    pub fn as_element(&self) -> Option<&ElementHandle> {
        // JSHandle 本身不是 element。ElementHandle 类型在构造时 is_element=true,
        // 但 trait method 默认实现无法访问 vtable。此方法在 ElementHandle 上调用
        // 才返回 Some;这里 JSHandle 直接返回 None。
        None
    }

    /// 读取缓存的 JSON value(不调用 CDP)。
    ///
    /// 调用方需先通过 B 类方法(`Runtime.callFunctionOn` + JSON.stringify)
    /// 把结果填入缓存。此处仅返回本地值。
    ///
    /// @trace REQ-BAO-API-006 [class:JSHandle]
    pub fn json_value(&self) -> Option<Value> {
        self.json_value_cache.borrow().clone()
    }

    /// 设置缓存的 JSON value(由 B 类 method 填入)。
    ///
    /// @trace REQ-BAO-API-006 [class:JSHandle]
    pub fn set_json_value(&self, value: Value) {
        *self.json_value_cache.borrow_mut() = Some(value);
    }

    /// 查询本地缓存 property。
    ///
    /// @trace REQ-BAO-API-006 [class:JSHandle]
    pub fn get_property(&self, name: &str) -> Option<Rc<JSHandle>> {
        self.properties.borrow().get(name).cloned()
    }

    /// 列出所有本地缓存 properties。
    ///
    /// @trace REQ-BAO-API-006 [class:JSHandle]
    pub fn get_properties(&self) -> Vec<(String, Rc<JSHandle>)> {
        self.properties
            .borrow()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    /// 写入本地缓存 property(B 类 method 填入)。
    ///
    /// @trace REQ-BAO-API-006 [class:JSHandle]
    pub fn set_property(&self, name: impl Into<String>, handle: Rc<JSHandle>) {
        self.properties.borrow_mut().insert(name.into(), handle);
    }

    /// dispose — 标记本地 + 释放 properties 缓存。
    ///
    /// 注意:本方法仅清理本地状态;真正调用 `Runtime.releaseObject` 的 B 类
    /// 命令由 Page/Frame 持有的 transport 触发(不在本 D 类方法范围内)。
    ///
    /// @trace REQ-BAO-API-006 [class:JSHandle]
    pub fn dispose(&self) {
        *self.disposed.borrow_mut() = true;
        self.properties.borrow_mut().clear();
        *self.json_value_cache.borrow_mut() = None;
    }

    /// 是否是 Element 类型。
    ///
    /// @trace REQ-BAO-API-006 [class:JSHandle]
    pub fn is_element_handle(&self) -> bool {
        self.is_element
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ctx() -> Rc<ExecutionContext> {
        Rc::new(ExecutionContext::new("CTX-1".to_string()))
    }

    #[test]
    fn new_initial_state() {
        let h = JSHandle::new(make_ctx(), "OBJ-1");
        assert_eq!(h.remote_object_id(), "OBJ-1");
        assert!(!h.is_disposed());
        assert!(!h.is_element_handle());
        assert!(h.json_value().is_none());
        assert!(h.get_properties().is_empty());
    }

    #[test]
    fn as_element_returns_none_for_jshandle() {
        let h = JSHandle::new(make_ctx(), "OBJ-1");
        assert!(h.as_element().is_none());
    }

    #[test]
    fn set_json_value_then_get() {
        let h = JSHandle::new(make_ctx(), "OBJ-1");
        assert!(h.json_value().is_none());
        h.set_json_value(Value::from(42));
        assert_eq!(h.json_value(), Some(Value::from(42)));
    }

    #[test]
    fn set_property_then_get() {
        let h = JSHandle::new(make_ctx(), "OBJ-1");
        let child = Rc::new(JSHandle::new(make_ctx(), "OBJ-2"));
        h.set_property("foo", child.clone());
        assert_eq!(h.get_properties().len(), 1);
        let got = h.get_property("foo").unwrap();
        assert_eq!(got.remote_object_id(), "OBJ-2");
    }

    #[test]
    fn dispose_clears_state() {
        let h = JSHandle::new(make_ctx(), "OBJ-1");
        let child = Rc::new(JSHandle::new(make_ctx(), "OBJ-2"));
        h.set_property("foo", child);
        h.set_json_value(Value::from(1));
        h.dispose();
        assert!(h.is_disposed());
        assert!(h.json_value().is_none());
        assert!(h.get_properties().is_empty());
        assert!(h.get_property("foo").is_none());
    }

    #[test]
    fn execution_context_returns_ref() {
        let ctx = make_ctx();
        let h = JSHandle::new(ctx.clone(), "OBJ-1");
        assert_eq!(h.execution_context().id(), "CTX-1");
    }
}
