// @trace REQ-ENG-001
//! SpiderMonkey Debugger — breakpoint management and state tracking.
//!
//! SM's `JS::Debugger` API requires C++ header bindings not yet exposed in mozjs.
//! This module provides real breakpoint CRUD + state tracking, ready for
//! SM Debugger API integration when bindings become available.

use ::std::collections::HashMap;
use ::std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static DEBUGGER_ENABLED: AtomicBool = AtomicBool::new(false);
static BREAKPOINT_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone)]
pub struct Debugger {
    breakpoints: HashMap<u64, Breakpoint>,
}

impl Debugger {
    pub fn new() -> Self {
        Debugger { breakpoints: HashMap::new() }
    }

    pub fn enable(&mut self) -> Result<(), DebuggerError> {
        if DEBUGGER_ENABLED.load(Ordering::Acquire) {
            return Err(DebuggerError::AlreadyEnabled);
        }
        DEBUGGER_ENABLED.store(true, Ordering::Release);
        Ok(())
    }

    pub fn disable(&mut self) -> Result<(), DebuggerError> {
        if !DEBUGGER_ENABLED.load(Ordering::Acquire) {
            return Err(DebuggerError::AlreadyDisabled);
        }
        DEBUGGER_ENABLED.store(false, Ordering::Release);
        Ok(())
    }

    pub fn is_enabled(&self) -> bool {
        DEBUGGER_ENABLED.load(Ordering::Acquire)
    }

    pub fn set_breakpoint(&mut self, url: String, line: u32, column: u32) -> u64 {
        let id = BREAKPOINT_COUNTER.fetch_add(1, Ordering::Relaxed);
        self.breakpoints.insert(id, Breakpoint { id, url, line, column });
        id
    }

    pub fn remove_breakpoint(&mut self, id: u64) -> Option<Breakpoint> {
        self.breakpoints.remove(&id)
    }

    pub fn get_breakpoint(&self, id: u64) -> Option<&Breakpoint> {
        self.breakpoints.get(&id)
    }

    pub fn get_breakpoints_for_url(&self, url: &str) -> Vec<&Breakpoint> {
        self.breakpoints.values().filter(|bp| bp.url == url).collect()
    }

    pub fn all_breakpoints(&self) -> Vec<&Breakpoint> {
        self.breakpoints.values().collect()
    }

    pub fn breakpoint_count(&self) -> usize {
        self.breakpoints.len()
    }

    pub fn clear_all_breakpoints(&mut self) {
        self.breakpoints.clear();
    }
}

impl Default for Debugger {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DebuggerError {
    AlreadyEnabled,
    AlreadyDisabled,
}

#[derive(Debug, Clone)]
pub struct Breakpoint {
    pub id: u64,
    pub url: String,
    pub line: u32,
    pub column: u32,
}

#[derive(Debug, Clone)]
pub struct SourceInfo {
    pub id: u64,
    pub url: String,
    pub is_wasm: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debugger_enable_disable() {
        let mut dbg = Debugger::new();
        assert!(!dbg.is_enabled());
        dbg.enable().unwrap();
        assert!(dbg.is_enabled());
        assert_eq!(dbg.enable(), Err(DebuggerError::AlreadyEnabled));
        dbg.disable().unwrap();
        assert!(!dbg.is_enabled());
        assert_eq!(dbg.disable(), Err(DebuggerError::AlreadyDisabled));
    }

    #[test]
    fn breakpoint_crud() {
        let mut dbg = Debugger::new();
        let id = dbg.set_breakpoint("test.js".into(), 10, 5);
        assert_eq!(dbg.breakpoint_count(), 1);

        let bp = dbg.get_breakpoint(id).unwrap();
        assert_eq!(bp.url, "test.js");
        assert_eq!(bp.line, 10);
        assert_eq!(bp.column, 5);

        let removed = dbg.remove_breakpoint(id).unwrap();
        assert_eq!(removed.id, id);
        assert_eq!(dbg.breakpoint_count(), 0);
    }

    #[test]
    fn breakpoints_by_url() {
        let mut dbg = Debugger::new();
        dbg.set_breakpoint("a.js".into(), 1, 0);
        dbg.set_breakpoint("b.js".into(), 2, 0);
        dbg.set_breakpoint("a.js".into(), 3, 0);

        let a_bps = dbg.get_breakpoints_for_url("a.js");
        assert_eq!(a_bps.len(), 2);
    }

    #[test]
    fn clear_all() {
        let mut dbg = Debugger::new();
        dbg.set_breakpoint("a.js".into(), 1, 0);
        dbg.set_breakpoint("b.js".into(), 2, 0);
        dbg.clear_all_breakpoints();
        assert_eq!(dbg.breakpoint_count(), 0);
    }
}
