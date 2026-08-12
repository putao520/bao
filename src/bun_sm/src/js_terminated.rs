//! JsTerminated — sentinel for terminated execution.

use crate::error_code::ErrorCode;
use crate::js_error::JsError;

/// Represents a terminated JavaScript execution.
pub struct JsTerminated;

impl JsTerminated {
    /// Convert to a JsError.
    pub fn to_error(&self) -> JsError {
        JsError {
            message: "JavaScript execution terminated".into(),
            filename: String::new(),
            line: 0,
            column: 0,
            stack: None,
        }
    }

    /// Get the error code.
    pub fn to_error_code(&self) -> ErrorCode {
        ErrorCode::InternalError
    }
}

/// Result of a terminated execution check.
#[derive(Debug)]
pub enum JsTerminatedResult {
    NotTerminated,
    Terminated,
}
