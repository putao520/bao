//! JSC-compatible error code classification.

/// Error codes matching JSC's classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ErrorCode {
    NoError = 0,
    TypeError = 1,
    RangeError = 2,
    SyntaxError = 3,
    ReferenceError = 4,
    StackOverflow = 5,
    OutOfMemory = 6,
    URIError = 7,
    EvalError = 8,
    InternalError = 9,
    GenericError = 10,
}
