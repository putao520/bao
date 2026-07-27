//! **Deprecated for product use.** CC/IDE workflow host inject belongs in the
//! product (Frog: `gsc-frog-workflow-js-host`), not in Bao's generic runtime.
//!
//! Kept as a thin re-export only so existing bao-side unit tests link until
//! they migrate. Do **not** auto-install from `install_all` / Bun globals.
//!
//! @deprecated product path: `gsc_frog_workflow_js_host`
pub use bao_workflow_host::*;
