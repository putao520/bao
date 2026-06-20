//! `bao_lints` — format-immune AST-based BCE-012 detector (library).
//!
//! Re-exports the detector so it can be invoked from the binary entry point
//! and from integration tests.
//!
//! See `src/main.rs` for the CLI and `src/BUG-KNOWLEDGE.md` (BCE-20260619-012)
//! for the pattern specification.

pub mod detector;
pub mod pattern;

pub use detector::{scan_source, Finding};
