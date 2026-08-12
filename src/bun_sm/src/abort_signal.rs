// @trace REQ-ENG-001
// AbortSignal stub for SpiderMonkey.
// Ported from Bun's AbortSignal pattern; the JS-side AbortSignal is a Web API
// that servo provides. This module handles the Rust-side bookkeeping.

/// Rust-side AbortSignal state. The actual JS AbortSignal object lives in
/// servo's DOM; this struct tracks the native bookkeeping.
#[derive(Debug)]
pub struct AbortSignal {
    aborted: bool,
    reason: Option<String>,
}

impl AbortSignal {
    pub fn new() -> Self {
        Self {
            aborted: false,
            reason: None,
        }
    }

    #[inline]
    pub fn aborted(&self) -> bool {
        self.aborted
    }

    #[inline]
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    pub fn abort(&mut self, reason: Option<String>) {
        self.aborted = true;
        self.reason = reason;
    }

    pub fn reset(&mut self) {
        self.aborted = false;
        self.reason = None;
    }
}

impl Default for AbortSignal {
    fn default() -> Self {
        Self::new()
    }
}

/// Controller that owns an AbortSignal.
#[derive(Debug, Default)]
pub struct AbortController {
    signal: AbortSignal,
}

impl AbortController {
    pub fn new() -> Self {
        Self::default()
    }

    #[inline]
    pub fn signal(&self) -> &AbortSignal {
        &self.signal
    }

    pub fn abort(&mut self, reason: Option<String>) {
        self.signal.abort(reason);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abort_signal_default() {
        let sig = AbortSignal::default();
        assert!(!sig.aborted());
        assert!(sig.reason().is_none());
    }

    #[test]
    fn abort_signal_abort() {
        let mut sig = AbortSignal::new();
        sig.abort(Some("timeout".into()));
        assert!(sig.aborted());
        assert_eq!(sig.reason(), Some("timeout"));
    }

    #[test]
    fn abort_signal_reset() {
        let mut sig = AbortSignal::new();
        sig.abort(None);
        assert!(sig.aborted());
        sig.reset();
        assert!(!sig.aborted());
    }

    #[test]
    fn abort_controller() {
        let mut ctrl = AbortController::new();
        assert!(!ctrl.signal().aborted());
        ctrl.abort(Some("cancelled".into()));
        assert!(ctrl.signal().aborted());
        assert_eq!(ctrl.signal().reason(), Some("cancelled"));
    }
}
