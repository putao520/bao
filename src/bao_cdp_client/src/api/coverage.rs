//! Coverage — JS/CSS 覆盖率追踪。
//!
//! D 类 method 纯本地状态:
//! - `is_started() -> bool`
//! - `js_started() -> bool`
//! - `css_started() -> bool`
//!
//! 非 D 类 method(startJSCoverage/startCSSCoverage/stop)走 transport。
//!
//! @trace REQ-BAO-API-006 [class:Coverage]

use std::cell::RefCell;

/// Coverage 本地状态(Page 持有的实例)。
///
/// @trace REQ-BAO-API-006 [class:Coverage]
pub struct Coverage {
    js_started: RefCell<bool>,
    css_started: RefCell<bool>,
    /// 本地缓存:覆盖率结果(由 B 类 method 填入)。
    js_results: RefCell<Vec<serde_json::Value>>,
    css_results: RefCell<Vec<serde_json::Value>>,
}

impl std::fmt::Debug for Coverage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Coverage")
            .field("js_started", &self.js_started.borrow())
            .field("css_started", &self.css_started.borrow())
            .finish()
    }
}

impl Coverage {
    /// 构造 Coverage(初始未启动)。
    ///
    /// @trace REQ-BAO-API-006 [class:Coverage]
    pub fn new() -> Self {
        Self {
            js_started: RefCell::new(false),
            css_started: RefCell::new(false),
            js_results: RefCell::new(Vec::new()),
            css_results: RefCell::new(Vec::new()),
        }
    }

    /// JS coverage 是否启动。
    ///
    /// @trace REQ-BAO-API-006 [class:Coverage]
    pub fn js_started(&self) -> bool {
        *self.js_started.borrow()
    }

    /// CSS coverage 是否启动。
    ///
    /// @trace REQ-BAO-API-006 [class:Coverage]
    pub fn css_started(&self) -> bool {
        *self.css_started.borrow()
    }

    /// 是否启动(JS 或 CSS 任一)。
    ///
    /// @trace REQ-BAO-API-006 [class:Coverage]
    pub fn is_started(&self) -> bool {
        *self.js_started.borrow() || *self.css_started.borrow()
    }

    /// 设置 JS coverage 状态(start/stop 时调用,本地)。
    ///
    /// @trace REQ-BAO-API-006 [class:Coverage]
    pub fn set_js_started(&self, v: bool) {
        *self.js_started.borrow_mut() = v;
    }

    /// 设置 CSS coverage 状态。
    ///
    /// @trace REQ-BAO-API-006 [class:Coverage]
    pub fn set_css_started(&self, v: bool) {
        *self.css_started.borrow_mut() = v;
    }

    /// 添加 JS coverage 结果(本地缓存)。
    ///
    /// @trace REQ-BAO-API-006 [class:Coverage]
    pub fn add_js_result(&self, v: serde_json::Value) {
        self.js_results.borrow_mut().push(v);
    }

    /// 添加 CSS coverage 结果(本地缓存)。
    ///
    /// @trace REQ-BAO-API-006 [class:Coverage]
    pub fn add_css_result(&self, v: serde_json::Value) {
        self.css_results.borrow_mut().push(v);
    }

    /// JS coverage 结果(克隆)。
    ///
    /// @trace REQ-BAO-API-006 [class:Coverage]
    pub fn js_results(&self) -> Vec<serde_json::Value> {
        self.js_results.borrow().clone()
    }

    /// CSS coverage 结果(克隆)。
    ///
    /// @trace REQ-BAO-API-006 [class:Coverage]
    pub fn css_results(&self) -> Vec<serde_json::Value> {
        self.css_results.borrow().clone()
    }

    /// 重置。
    ///
    /// @trace REQ-BAO-API-006 [class:Coverage]
    pub fn reset(&self) {
        *self.js_started.borrow_mut() = false;
        *self.css_started.borrow_mut() = false;
        self.js_results.borrow_mut().clear();
        self.css_results.borrow_mut().clear();
    }
}

impl Default for Coverage {
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
        let c = Coverage::new();
        assert!(!c.is_started());
        assert!(!c.js_started());
        assert!(!c.css_started());
    }

    #[test]
    fn start_js_coverage() {
        let c = Coverage::new();
        c.set_js_started(true);
        assert!(c.js_started());
        assert!(c.is_started());
    }

    #[test]
    fn start_css_coverage() {
        let c = Coverage::new();
        c.set_css_started(true);
        assert!(c.css_started());
        assert!(c.is_started());
    }

    #[test]
    fn js_results_round_trip() {
        let c = Coverage::new();
        assert_eq!(c.js_results().len(), 0);
        c.add_js_result(json!({"url": "a.js"}));
        c.add_js_result(json!({"url": "b.js"}));
        assert_eq!(c.js_results().len(), 2);
    }

    #[test]
    fn css_results_round_trip() {
        let c = Coverage::new();
        c.add_css_result(json!({"url": "a.css"}));
        assert_eq!(c.css_results().len(), 1);
    }

    #[test]
    fn reset_clears_all() {
        let c = Coverage::new();
        c.set_js_started(true);
        c.add_js_result(json!({"x": 1}));
        c.reset();
        assert!(!c.is_started());
        assert_eq!(c.js_results().len(), 0);
    }
}
