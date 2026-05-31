// @trace REQ-BRW-001
use std::fmt;

#[derive(Debug)]
pub enum BrowserError {
    Init(String),
    Navigation(String),
    Rendering(String),
    JavaScript(String),
    CDP(String),
}

impl fmt::Display for BrowserError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            BrowserError::Init(msg) => write!(f, "browser init error: {msg}"),
            BrowserError::Navigation(msg) => write!(f, "navigation error: {msg}"),
            BrowserError::Rendering(msg) => write!(f, "rendering error: {msg}"),
            BrowserError::JavaScript(msg) => write!(f, "javascript error: {msg}"),
            BrowserError::CDP(msg) => write!(f, "cdp error: {msg}"),
        }
    }
}

impl std::error::Error for BrowserError {}

#[cfg(test)]
mod tests {
    // @trace REQ-BRW-001 [req:REQ-BRW-001] [level:unit]
    use super::BrowserError;
    use std::error::Error;

    #[test]
    fn init_display_format() {
        let err = BrowserError::Init("servo crashed".into());
        assert_eq!(err.to_string(), "browser init error: servo crashed");
    }

    #[test]
    fn navigation_display_format() {
        let err = BrowserError::Navigation("url not found".into());
        assert_eq!(err.to_string(), "navigation error: url not found");
    }

    #[test]
    fn rendering_display_format() {
        let err = BrowserError::Rendering("gpu lost".into());
        assert_eq!(err.to_string(), "rendering error: gpu lost");
    }

    #[test]
    fn javascript_display_format() {
        let err = BrowserError::JavaScript("syntax error".into());
        assert_eq!(err.to_string(), "javascript error: syntax error");
    }

    #[test]
    fn cdp_display_format() {
        let err = BrowserError::CDP("connection refused".into());
        assert_eq!(err.to_string(), "cdp error: connection refused");
    }

    #[test]
    fn error_trait_dyn_compatible() {
        let err: Box<dyn Error> = Box::new(BrowserError::Init("fail".into()));
        assert_eq!(err.to_string(), "browser init error: fail");
    }

    #[test]
    fn debug_format_roundtrip() {
        let err = BrowserError::Navigation("page load".into());
        let debug = format!("{err:?}");
        assert!(debug.contains("Navigation"), "Debug should contain variant name: {debug}");
        assert!(debug.contains("page load"), "Debug should contain message: {debug}");
    }

    #[test]
    fn empty_string_message() {
        let err = BrowserError::Rendering(String::new());
        assert_eq!(err.to_string(), "rendering error: ");
    }

    #[test]
    fn unicode_message() {
        let err = BrowserError::CDP("连接失败 🌐".into());
        assert_eq!(err.to_string(), "cdp error: 连接失败 🌐");
    }
}
