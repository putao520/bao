//! Mouse — Page 持有的鼠标输入实例。
//!
//! D 类 method 纯本地状态:
//! - `current_x() -> f64`
//! - `current_y() -> f64`
//! - `current_button() -> MouseButton`
//! - `is_button_pressed(button) -> bool`
//!
//! 非 D 类 method(click/move/up/down/wheel)走 transport(Input.dispatchMouseEvent)。
//!
//! @trace REQ-BAO-API-006 [class:Mouse]

use std::cell::RefCell;

/// 鼠标按键。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseButton {
    #[default]
    None,
    Left,
    Middle,
    Right,
    Back,
    Forward,
}

impl MouseButton {
    pub fn as_str(&self) -> &'static str {
        match self {
            MouseButton::None => "none",
            MouseButton::Left => "left",
            MouseButton::Middle => "middle",
            MouseButton::Right => "right",
            MouseButton::Back => "back",
            MouseButton::Forward => "forward",
        }
    }
}

/// Mouse 本地状态(Page 持有的实例)。
///
/// @trace REQ-BAO-API-006 [class:Mouse]
pub struct Mouse {
    /// 当前光标 X(viewport 坐标)。
    x: RefCell<f64>,
    /// 当前光标 Y。
    y: RefCell<f64>,
    /// 当前按下按钮。
    button: RefCell<MouseButton>,
    /// 按钮按下状态集合(支持多按钮)。
    pressed_buttons: RefCell<Vec<MouseButton>>,
}

impl std::fmt::Debug for Mouse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mouse")
            .field("x", &self.x.borrow())
            .field("y", &self.y.borrow())
            .field("button", &self.button.borrow())
            .field("pressed_count", &self.pressed_buttons.borrow().len())
            .finish()
    }
}

impl Mouse {
    /// 构造 Mouse(初始 0,0,无按钮)。
    ///
    /// @trace REQ-BAO-API-006 [class:Mouse]
    pub fn new() -> Self {
        Self {
            x: RefCell::new(0.0),
            y: RefCell::new(0.0),
            button: RefCell::new(MouseButton::None),
            pressed_buttons: RefCell::new(Vec::new()),
        }
    }

    /// 当前 X。
    ///
    /// @trace REQ-BAO-API-006 [class:Mouse]
    pub fn current_x(&self) -> f64 {
        *self.x.borrow()
    }

    /// 当前 Y。
    ///
    /// @trace REQ-BAO-API-006 [class:Mouse]
    pub fn current_y(&self) -> f64 {
        *self.y.borrow()
    }

    /// 当前按钮。
    ///
    /// @trace REQ-BAO-API-006 [class:Mouse]
    pub fn current_button(&self) -> MouseButton {
        *self.button.borrow()
    }

    /// 设置当前位置(move 事件触发,本地状态)。
    ///
    /// @trace REQ-BAO-API-006 [class:Mouse]
    pub fn set_position(&self, x: f64, y: f64) {
        *self.x.borrow_mut() = x;
        *self.y.borrow_mut() = y;
    }

    /// 设置当前按钮。
    ///
    /// @trace REQ-BAO-API-006 [class:Mouse]
    pub fn set_button(&self, b: MouseButton) {
        *self.button.borrow_mut() = b;
    }

    /// 标记按钮按下(本地状态)。
    ///
    /// @trace REQ-BAO-API-006 [class:Mouse]
    pub fn press_button(&self, b: MouseButton) {
        let mut pressed = self.pressed_buttons.borrow_mut();
        if !pressed.contains(&b) {
            pressed.push(b);
        }
    }

    /// 标记按钮释放(本地状态)。
    ///
    /// @trace REQ-BAO-API-006 [class:Mouse]
    pub fn release_button(&self, b: MouseButton) {
        self.pressed_buttons.borrow_mut().retain(|x| *x != b);
    }

    /// 指定按钮是否按下。
    ///
    /// @trace REQ-BAO-API-006 [class:Mouse]
    pub fn is_button_pressed(&self, b: MouseButton) -> bool {
        self.pressed_buttons.borrow().contains(&b)
    }

    /// 已按下按钮数量。
    ///
    /// @trace REQ-BAO-API-006 [class:Mouse]
    pub fn pressed_button_count(&self) -> usize {
        self.pressed_buttons.borrow().len()
    }

    /// 重置所有状态。
    ///
    /// @trace REQ-BAO-API-006 [class:Mouse]
    pub fn reset(&self) {
        *self.x.borrow_mut() = 0.0;
        *self.y.borrow_mut() = 0.0;
        *self.button.borrow_mut() = MouseButton::None;
        self.pressed_buttons.borrow_mut().clear();
    }
}

impl Default for Mouse {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_defaults() {
        let m = Mouse::new();
        assert_eq!(m.current_x(), 0.0);
        assert_eq!(m.current_y(), 0.0);
        assert_eq!(m.current_button(), MouseButton::None);
        assert_eq!(m.pressed_button_count(), 0);
    }

    #[test]
    fn set_position() {
        let m = Mouse::new();
        m.set_position(10.0, 20.0);
        assert_eq!(m.current_x(), 10.0);
        assert_eq!(m.current_y(), 20.0);
    }

    #[test]
    fn set_button() {
        let m = Mouse::new();
        m.set_button(MouseButton::Right);
        assert_eq!(m.current_button(), MouseButton::Right);
    }

    #[test]
    fn press_release_button() {
        let m = Mouse::new();
        m.press_button(MouseButton::Left);
        assert!(m.is_button_pressed(MouseButton::Left));
        assert!(!m.is_button_pressed(MouseButton::Right));
        m.release_button(MouseButton::Left);
        assert!(!m.is_button_pressed(MouseButton::Left));
    }

    #[test]
    fn multi_button_pressed() {
        let m = Mouse::new();
        m.press_button(MouseButton::Left);
        m.press_button(MouseButton::Right);
        assert_eq!(m.pressed_button_count(), 2);
    }

    #[test]
    fn duplicate_press_no_dup() {
        let m = Mouse::new();
        m.press_button(MouseButton::Left);
        m.press_button(MouseButton::Left);
        assert_eq!(m.pressed_button_count(), 1);
    }

    #[test]
    fn reset_clears_all() {
        let m = Mouse::new();
        m.set_position(10.0, 20.0);
        m.press_button(MouseButton::Left);
        m.reset();
        assert_eq!(m.current_x(), 0.0);
        assert_eq!(m.pressed_button_count(), 0);
    }
}
