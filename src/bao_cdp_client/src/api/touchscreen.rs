//! Touchscreen — Page 持有的触摸输入实例。
//!
//! D 类 method 纯本地状态:
//! - `current_touches() -> Vec<TouchPoint>`
//! - `touch_count() -> usize`
//!
//! 非 D 类 method(tap/startTouch/endTouch)走 transport(Input.dispatchTouchEvent)。
//!
//! @trace REQ-BAO-API-006 [class:Touchscreen]

use std::cell::RefCell;

/// TouchPoint(触摸点)。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TouchPoint {
    pub x: f64,
    pub y: f64,
    pub radius_x: f64,
    pub radius_y: f64,
    pub force: f64,
}

impl Default for TouchPoint {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            radius_x: 1.0,
            radius_y: 1.0,
            force: 1.0,
        }
    }
}

/// Touchscreen 本地状态(Page 持有的实例)。
///
/// @trace REQ-BAO-API-006 [class:Touchscreen]
pub struct Touchscreen {
    touches: RefCell<Vec<TouchPoint>>,
}

impl std::fmt::Debug for Touchscreen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Touchscreen")
            .field("touch_count", &self.touches.borrow().len())
            .finish()
    }
}

impl Touchscreen {
    /// 构造 Touchscreen(初始无触摸点)。
    ///
    /// @trace REQ-BAO-API-006 [class:Touchscreen]
    pub fn new() -> Self {
        Self {
            touches: RefCell::new(Vec::new()),
        }
    }

    /// 当前触摸点列表(克隆)。
    ///
    /// @trace REQ-BAO-API-006 [class:Touchscreen]
    pub fn current_touches(&self) -> Vec<TouchPoint> {
        self.touches.borrow().clone()
    }

    /// 触摸点数量。
    ///
    /// @trace REQ-BAO-API-006 [class:Touchscreen]
    pub fn touch_count(&self) -> usize {
        self.touches.borrow().len()
    }

    /// 添加触摸点(本地状态)。
    ///
    /// @trace REQ-BAO-API-006 [class:Touchscreen]
    pub fn add_touch(&self, p: TouchPoint) {
        self.touches.borrow_mut().push(p);
    }

    /// 移除指定索引的触摸点。
    ///
    /// @trace REQ-BAO-API-006 [class:Touchscreen]
    pub fn remove_touch_at(&self, idx: usize) {
        let mut t = self.touches.borrow_mut();
        if idx < t.len() {
            t.remove(idx);
        }
    }

    /// 清空所有触摸点。
    ///
    /// @trace REQ-BAO-API-006 [class:Touchscreen]
    pub fn clear(&self) {
        self.touches.borrow_mut().clear();
    }
}

impl Default for Touchscreen {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_empty() {
        let ts = Touchscreen::new();
        assert_eq!(ts.touch_count(), 0);
        assert!(ts.current_touches().is_empty());
    }

    #[test]
    fn add_remove_touch() {
        let ts = Touchscreen::new();
        ts.add_touch(TouchPoint {
            x: 10.0,
            y: 20.0,
            ..Default::default()
        });
        assert_eq!(ts.touch_count(), 1);
        ts.remove_touch_at(0);
        assert_eq!(ts.touch_count(), 0);
    }

    #[test]
    fn multi_touch() {
        let ts = Touchscreen::new();
        ts.add_touch(TouchPoint {
            x: 1.0,
            y: 1.0,
            ..Default::default()
        });
        ts.add_touch(TouchPoint {
            x: 2.0,
            y: 2.0,
            ..Default::default()
        });
        ts.add_touch(TouchPoint {
            x: 3.0,
            y: 3.0,
            ..Default::default()
        });
        assert_eq!(ts.touch_count(), 3);
    }

    #[test]
    fn clear_all() {
        let ts = Touchscreen::new();
        ts.add_touch(TouchPoint::default());
        ts.clear();
        assert_eq!(ts.touch_count(), 0);
    }

    #[test]
    fn remove_invalid_index_noop() {
        let ts = Touchscreen::new();
        ts.add_touch(TouchPoint::default());
        ts.remove_touch_at(99);
        assert_eq!(ts.touch_count(), 1);
    }
}
