//! Tracing — Performance tracing(Chrome trace event)。
//!
//! D 类 method 纯本地状态:
//! - `is_started() -> bool`
//! - `current_categories() -> Vec<String>`
//!
//! 非 D 类 method(start/end)走 transport(Tracing.start / Tracing.end)。
//!
//! @trace REQ-BAO-API-006 [class:Tracing]

use std::cell::RefCell;

/// Tracing 本地状态(Page 持有的实例)。
///
/// @trace REQ-BAO-API-006 [class:Tracing]
pub struct Tracing {
    started: RefCell<bool>,
    categories: RefCell<Vec<String>>,
    /// 本地缓存:trace 数据(B 类 method 填入)。
    trace_data: RefCell<Option<serde_json::Value>>,
}

impl std::fmt::Debug for Tracing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tracing")
            .field("started", &self.started.borrow())
            .field("category_count", &self.categories.borrow().len())
            .finish()
    }
}

impl Tracing {
    /// 构造 Tracing(初始未启动)。
    ///
    /// @trace REQ-BAO-API-006 [class:Tracing]
    pub fn new() -> Self {
        Self {
            started: RefCell::new(false),
            categories: RefCell::new(Vec::new()),
            trace_data: RefCell::new(None),
        }
    }

    /// 是否启动。
    ///
    /// @trace REQ-BAO-API-006 [class:Tracing]
    pub fn is_started(&self) -> bool {
        *self.started.borrow()
    }

    /// 设置启动状态。
    ///
    /// @trace REQ-BAO-API-006 [class:Tracing]
    pub fn set_started(&self, v: bool) {
        *self.started.borrow_mut() = v;
    }

    /// 当前 categories(克隆)。
    ///
    /// @trace REQ-BAO-API-006 [class:Tracing]
    pub fn current_categories(&self) -> Vec<String> {
        self.categories.borrow().clone()
    }

    /// 设置 categories。
    ///
    /// @trace REQ-BAO-API-006 [class:Tracing]
    pub fn set_categories(&self, cats: Vec<String>) {
        *self.categories.borrow_mut() = cats;
    }

    /// 添加 category。
    ///
    /// @trace REQ-BAO-API-006 [class:Tracing]
    pub fn add_category(&self, cat: impl Into<String>) {
        let c = cat.into();
        let mut cats = self.categories.borrow_mut();
        if !cats.contains(&c) {
            cats.push(c);
        }
    }

    /// Trace 数据(本地缓存)。
    ///
    /// @trace REQ-BAO-API-006 [class:Tracing]
    pub fn trace_data(&self) -> Option<serde_json::Value> {
        self.trace_data.borrow().clone()
    }

    /// 设置 trace 数据。
    ///
    /// @trace REQ-BAO-API-006 [class:Tracing]
    pub fn set_trace_data(&self, v: serde_json::Value) {
        *self.trace_data.borrow_mut() = Some(v);
    }

    /// 重置。
    ///
    /// @trace REQ-BAO-API-006 [class:Tracing]
    pub fn reset(&self) {
        *self.started.borrow_mut() = false;
        self.categories.borrow_mut().clear();
        *self.trace_data.borrow_mut() = None;
    }
}

impl Default for Tracing {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn new_not_started() {
        let t = Tracing::new();
        assert!(!t.is_started());
        assert!(t.current_categories().is_empty());
        assert!(t.trace_data().is_none());
    }

    #[test]
    fn start_stop() {
        let t = Tracing::new();
        t.set_started(true);
        assert!(t.is_started());
        t.set_started(false);
        assert!(!t.is_started());
    }

    #[test]
    fn categories_round_trip() {
        let t = Tracing::new();
        t.set_categories(vec!["devtools.timeline".into(), "v8".into()]);
        assert_eq!(t.current_categories().len(), 2);
    }

    #[test]
    fn add_category_no_dup() {
        let t = Tracing::new();
        t.add_category("x");
        t.add_category("x");
        assert_eq!(t.current_categories().len(), 1);
    }

    #[test]
    fn trace_data_round_trip() {
        let t = Tracing::new();
        t.set_trace_data(json!({"traceEvents": []}));
        assert!(t.trace_data().is_some());
    }

    #[test]
    fn reset_clears() {
        let t = Tracing::new();
        t.set_started(true);
        t.add_category("x");
        t.reset();
        assert!(!t.is_started());
        assert!(t.current_categories().is_empty());
    }
}
