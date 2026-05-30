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
