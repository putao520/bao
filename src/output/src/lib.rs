pub use bun_core::output::*;

// Re-export macros from bun_core (#[macro_export] macros live at crate root,
// not in the module, so `pub use *` doesn't pick them up).
pub use bun_core::{declare_scope, scoped_log, define_scoped_log, println, debug};
