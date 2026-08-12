//! Keyboard — Page 持有的键盘输入实例。
//!
//! Keyboard 在 Page 构造时自动创建,持有本地状态(已按下的 modifier 状态等)。
//! D 类 method 纯本地状态:
//! - `is_shift_pressed() -> bool`
//! - `is_control_pressed() -> bool`
//! - `is_alt_pressed() -> bool`
//! - `is_meta_pressed() -> bool`
//! - `modifier_count() -> u8`
//!
//! 非 D 类 method(down/up/type)走 transport(Input.dispatchKeyEvent),不在本 TASK。
//!
//! @trace REQ-BAO-API-006 [class:Keyboard]

use std::cell::RefCell;

/// Keyboard modifier 标志位。
const MOD_SHIFT: u8 = 1;
const MOD_CONTROL: u8 = 2;
const MOD_ALT: u8 = 4;
const MOD_META: u8 = 8;

/// Keyboard 本地状态(Page 持有的实例)。
///
/// @trace REQ-BAO-API-006 [class:Keyboard]
pub struct Keyboard {
    modifiers: RefCell<u8>,
    /// 已按下的按键集合(本地缓存,用于断言)。
    pressed_keys: RefCell<Vec<String>>,
}

impl std::fmt::Debug for Keyboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Keyboard")
            .field("modifiers", &self.modifiers.borrow())
            .field("pressed_count", &self.pressed_keys.borrow().len())
            .finish()
    }
}

impl Keyboard {
    /// 构造 Keyboard(初始无 modifier)。
    ///
    /// @trace REQ-BAO-API-006 [class:Keyboard]
    pub fn new() -> Self {
        Self {
            modifiers: RefCell::new(0),
            pressed_keys: RefCell::new(Vec::new()),
        }
    }

    /// Shift 是否按下。
    ///
    /// @trace REQ-BAO-API-006 [class:Keyboard]
    pub fn is_shift_pressed(&self) -> bool {
        *self.modifiers.borrow() & MOD_SHIFT != 0
    }

    /// Control 是否按下。
    ///
    /// @trace REQ-BAO-API-006 [class:Keyboard]
    pub fn is_control_pressed(&self) -> bool {
        *self.modifiers.borrow() & MOD_CONTROL != 0
    }

    /// Alt 是否按下。
    ///
    /// @trace REQ-BAO-API-006 [class:Keyboard]
    pub fn is_alt_pressed(&self) -> bool {
        *self.modifiers.borrow() & MOD_ALT != 0
    }

    /// Meta(Cmd/Win)是否按下。
    ///
    /// @trace REQ-BAO-API-006 [class:Keyboard]
    pub fn is_meta_pressed(&self) -> bool {
        *self.modifiers.borrow() & MOD_META != 0
    }

    /// 已按下 modifier 数量(0-4)。
    ///
    /// @trace REQ-BAO-API-006 [class:Keyboard]
    pub fn modifier_count(&self) -> u8 {
        let m = *self.modifiers.borrow();
        let mut n = 0;
        if m & MOD_SHIFT != 0 {
            n += 1;
        }
        if m & MOD_CONTROL != 0 {
            n += 1;
        }
        if m & MOD_ALT != 0 {
            n += 1;
        }
        if m & MOD_META != 0 {
            n += 1;
        }
        n
    }

    /// 已按下按键集合(克隆)。
    ///
    /// @trace REQ-BAO-API-006 [class:Keyboard]
    pub fn pressed_keys(&self) -> Vec<String> {
        self.pressed_keys.borrow().clone()
    }

    /// 设置 modifier(本地状态)。down 事件触发。
    ///
    /// @trace REQ-BAO-API-006 [class:Keyboard]
    pub fn set_modifier(&self, mod_flag: u8, on: bool) {
        let mut m = self.modifiers.borrow_mut();
        if on {
            *m |= mod_flag;
        } else {
            *m &= !mod_flag;
        }
    }

    /// 添加已按下 key(本地状态)。
    ///
    /// @trace REQ-BAO-API-006 [class:Keyboard]
    pub fn add_pressed_key(&self, key: impl Into<String>) {
        let k = key.into();
        let mut pressed = self.pressed_keys.borrow_mut();
        if !pressed.contains(&k) {
            pressed.push(k);
        }
    }

    /// 移除已按下 key(本地状态)。
    ///
    /// @trace REQ-BAO-API-006 [class:Keyboard]
    pub fn remove_pressed_key(&self, key: &str) {
        self.pressed_keys.borrow_mut().retain(|k| k != key);
    }

    /// 重置所有 modifier 和 pressed keys。
    ///
    /// @trace REQ-BAO-API-006 [class:Keyboard]
    pub fn reset(&self) {
        *self.modifiers.borrow_mut() = 0;
        self.pressed_keys.borrow_mut().clear();
    }
}

impl Default for Keyboard {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_no_modifiers() {
        let k = Keyboard::new();
        assert!(!k.is_shift_pressed());
        assert!(!k.is_control_pressed());
        assert!(!k.is_alt_pressed());
        assert!(!k.is_meta_pressed());
        assert_eq!(k.modifier_count(), 0);
    }

    #[test]
    fn set_shift() {
        let k = Keyboard::new();
        k.set_modifier(MOD_SHIFT, true);
        assert!(k.is_shift_pressed());
        assert_eq!(k.modifier_count(), 1);
        k.set_modifier(MOD_SHIFT, false);
        assert!(!k.is_shift_pressed());
    }

    #[test]
    fn set_multiple_modifiers() {
        let k = Keyboard::new();
        k.set_modifier(MOD_SHIFT, true);
        k.set_modifier(MOD_CONTROL, true);
        k.set_modifier(MOD_ALT, true);
        k.set_modifier(MOD_META, true);
        assert_eq!(k.modifier_count(), 4);
    }

    #[test]
    fn pressed_keys_track() {
        let k = Keyboard::new();
        assert_eq!(k.pressed_keys().len(), 0);
        k.add_pressed_key("a");
        k.add_pressed_key("b");
        assert_eq!(k.pressed_keys().len(), 2);
        k.remove_pressed_key("a");
        assert_eq!(k.pressed_keys().len(), 1);
    }

    #[test]
    fn duplicate_pressed_key_no_dup() {
        let k = Keyboard::new();
        k.add_pressed_key("a");
        k.add_pressed_key("a");
        assert_eq!(k.pressed_keys().len(), 1);
    }

    #[test]
    fn reset_clears_all() {
        let k = Keyboard::new();
        k.set_modifier(MOD_SHIFT, true);
        k.add_pressed_key("a");
        k.reset();
        assert_eq!(k.modifier_count(), 0);
        assert_eq!(k.pressed_keys().len(), 0);
    }
}
