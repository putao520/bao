//! SM-backed JsError — re-exports bao_engine's JsError with compatibility.

pub use crate::error::JsError;
use crate::error_code::ErrorCode;

/// Extension trait for JsError compatibility with JSC.
pub trait JsErrorExt {
    fn to_error_code(&self) -> ErrorCode;
}

impl JsErrorExt for JsError {
    fn to_error_code(&self) -> ErrorCode {
        ErrorCode::GenericError
    }
}

/// Result type alias for JS operations.
pub type JsResult<T> = std::result::Result<T, JsError>;
