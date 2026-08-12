//! Dialog — JavaScript dialog(alert/confirm/prompt/beforeunload)。
//!
//! D 类 method 全部本地状态:
//! - `dialog_type() -> &str`
//! - `message() -> &str`
//! - `default_value() -> Option<&str>`
//! - `is_closed() -> bool`
//!
//! A 类 method(本 TASK 不实现,但保留 trait):`accept(promptText)` / `dismiss()` —
//! 这些会调用 Page.handleJavaScriptDialog(走 transport)。
//!
//! @trace REQ-BAO-API-006 [class:Dialog]

use std::cell::RefCell;

/// Dialog 类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogType {
    Alert,
    Confirm,
    Prompt,
    BeforeUnload,
}

impl DialogType {
    /// 字符串形式(对应 CDP `Page.javascriptDialogOpening` 的 message type)。
    pub fn as_str(&self) -> &'static str {
        match self {
            DialogType::Alert => "alert",
            DialogType::Confirm => "confirm",
            DialogType::Prompt => "prompt",
            DialogType::BeforeUnload => "beforeunload",
        }
    }

    /// 从字符串解析。
    pub fn from_str(s: &str) -> Self {
        match s {
            "confirm" => DialogType::Confirm,
            "prompt" => DialogType::Prompt,
            "beforeunload" => DialogType::BeforeUnload,
            _ => DialogType::Alert, // 默认 alert(unknown type 也归为 alert)
        }
    }
}

/// Dialog 本地状态。
///
/// @trace REQ-BAO-API-006 [class:Dialog]
pub struct Dialog {
    /// Dialog 类型。
    dialog_type: DialogType,
    /// 消息文本。
    message: RefCell<String>,
    /// 默认输入值(prompt only)。
    default_value: RefCell<Option<String>>,
    /// 是否已关闭(accept/dismiss 后置 true)。
    closed: RefCell<bool>,
}

impl std::fmt::Debug for Dialog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dialog")
            .field("dialog_type", &self.dialog_type)
            .field("message", &self.message.borrow())
            .field("default_value", &self.default_value.borrow())
            .field("closed", &self.closed.borrow())
            .finish()
    }
}

impl Dialog {
    /// 构造 Dialog。
    ///
    /// @trace REQ-BAO-API-006 [class:Dialog]
    pub fn new(
        dialog_type: DialogType,
        message: impl Into<String>,
        default_value: Option<String>,
    ) -> Self {
        Self {
            dialog_type,
            message: RefCell::new(message.into()),
            default_value: RefCell::new(default_value),
            closed: RefCell::new(false),
        }
    }

    /// Dialog 类型。
    ///
    /// @trace REQ-BAO-API-006 [class:Dialog]
    pub fn dialog_type(&self) -> DialogType {
        self.dialog_type
    }

    /// 类型字符串。
    ///
    /// @trace REQ-BAO-API-006 [class:Dialog]
    pub fn type_str(&self) -> &'static str {
        self.dialog_type.as_str()
    }

    /// 消息文本。
    ///
    /// @trace REQ-BAO-API-006 [class:Dialog]
    pub fn message(&self) -> String {
        self.message.borrow().clone()
    }

    /// 默认值(prompt only)。
    ///
    /// @trace REQ-BAO-API-006 [class:Dialog]
    pub fn default_value(&self) -> Option<String> {
        self.default_value.borrow().clone()
    }

    /// 设置默认值。
    ///
    /// @trace REQ-BAO-API-006 [class:Dialog]
    pub fn set_default_value(&self, v: impl Into<String>) {
        *self.default_value.borrow_mut() = Some(v.into());
    }

    /// 是否已关闭。
    ///
    /// @trace REQ-BAO-API-006 [class:Dialog]
    pub fn is_closed(&self) -> bool {
        *self.closed.borrow()
    }

    /// 标记关闭(accept/dismiss 后调用)。
    ///
    /// 注:本 TASK 只标记本地状态,真正调用 Page.handleJavaScriptDialog 走
    /// transport 由调用方在 Page 上完成。
    ///
    /// @trace REQ-BAO-API-006 [class:Dialog]
    pub fn set_closed(&self) {
        *self.closed.borrow_mut() = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alert_dialog_basic() {
        let d = Dialog::new(DialogType::Alert, "Hello", None);
        assert_eq!(d.dialog_type(), DialogType::Alert);
        assert_eq!(d.type_str(), "alert");
        assert_eq!(d.message(), "Hello");
        assert!(d.default_value().is_none());
        assert!(!d.is_closed());
    }

    #[test]
    fn prompt_dialog_with_default() {
        let d = Dialog::new(DialogType::Prompt, "Name?", Some("Alice".into()));
        assert_eq!(d.dialog_type(), DialogType::Prompt);
        assert_eq!(d.type_str(), "prompt");
        assert_eq!(d.default_value(), Some("Alice".into()));
    }

    #[test]
    fn confirm_dialog_no_default() {
        let d = Dialog::new(DialogType::Confirm, "Are you sure?", None);
        assert_eq!(d.dialog_type(), DialogType::Confirm);
        assert_eq!(d.type_str(), "confirm");
        assert!(d.default_value().is_none());
    }

    #[test]
    fn beforeunload_dialog() {
        let d = Dialog::new(DialogType::BeforeUnload, "", None);
        assert_eq!(d.type_str(), "beforeunload");
    }

    #[test]
    fn closed_flag() {
        let d = Dialog::new(DialogType::Alert, "x", None);
        assert!(!d.is_closed());
        d.set_closed();
        assert!(d.is_closed());
    }

    #[test]
    fn set_default_value_updates() {
        let d = Dialog::new(DialogType::Prompt, "?", None);
        assert!(d.default_value().is_none());
        d.set_default_value("new");
        assert_eq!(d.default_value(), Some("new".into()));
    }

    #[test]
    fn from_str_round_trip() {
        assert_eq!(DialogType::from_str("alert"), DialogType::Alert);
        assert_eq!(DialogType::from_str("confirm"), DialogType::Confirm);
        assert_eq!(DialogType::from_str("prompt"), DialogType::Prompt);
        assert_eq!(
            DialogType::from_str("beforeunload"),
            DialogType::BeforeUnload
        );
        // unknown defaults to alert
        assert_eq!(DialogType::from_str("garbage"), DialogType::Alert);
    }
}
