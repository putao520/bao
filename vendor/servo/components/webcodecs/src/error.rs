// @trace REQ-BRW-051
//! Error surface for the WebCodecs decode plane.
//!
//! [`WebCodecsError`] classifies the failure (the DOM layer maps
//! NotSupported + Syntax sub-kinds to a NotSupportedError DOMException, and
//! NotSupported + Configure sub-kind to a TypeError thrown by `configure()` /
//! `isConfigSupported()` / `decode()` entry, per WPT video-decoder /
//! audio-decoder expectations).

use serde::{Deserialize, Serialize};

/// Codec-parse failure classification (no backend capability is implied).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WebCodecsErrorKind {
    /// Codec string is empty, has bad casing/whitespace, or an out-of-range
    /// level — a malformed codec string.
    Syntax,
    /// Well-formed-but-unsupportable codec string, or the platform backend is
    /// absent (the DOM maps this to NotSupportedError / `supported=false`).
    NotSupported,
}

/// Classified WebCodecs error with a stable machine-readable code and a
/// human-readable reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebCodecsError {
    /// Broad class.
    pub kind: WebCodecsErrorKind,
    /// Stable code string for the DOM layer to translate into a DOMException
    /// name / message.
    pub code: &'static str,
    /// Human-readable reason (never consumed by tests; for log/diagnostics).
    pub reason: String,
}

impl WebCodecsError {
    pub fn unsupported(code: &'static str, reason: impl Into<String>) -> Self {
        Self {
            kind: WebCodecsErrorKind::NotSupported,
            code,
            reason: reason.into(),
        }
    }
    pub fn syntax(code: &'static str, reason: impl Into<String>) -> Self {
        Self {
            kind: WebCodecsErrorKind::Syntax,
            code,
            reason: reason.into(),
        }
    }
}

impl std::fmt::Display for WebCodecsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.code, self.reason)
    }
}
