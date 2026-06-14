//! Console types.
//!
//! Re-exports `crate::host_fn::install_console` for full implementation,
//! plus JSC-compatible ConsoleObject/ConsoleFormatter types.

/// Console object (JS-side provided by crate::host_fn::install_console).
/// This struct represents the Rust-side console state tracker.
#[derive(Debug)]
pub struct ConsoleObject {
    /// Whether ANSI colors are enabled for output.
    pub enable_ansi_colors: bool,
}

impl ConsoleObject {
    pub fn new() -> Self {
        Self {
            enable_ansi_colors: false,
        }
    }
}

impl Default for ConsoleObject {
    fn default() -> Self {
        Self::new()
    }
}

/// Console output formatter.
#[derive(Debug)]
pub struct ConsoleFormatter {
    /// Current indentation level.
    pub indent_level: u32,
}

impl ConsoleFormatter {
    pub fn new() -> Self {
        Self { indent_level: 0 }
    }

    /// Increase indentation level.
    pub fn indent(&mut self) {
        self.indent_level += 1;
    }

    /// Decrease indentation level.
    pub fn dedent(&mut self) {
        if self.indent_level > 0 {
            self.indent_level -= 1;
        }
    }
}

impl Default for ConsoleFormatter {
    fn default() -> Self {
        Self::new()
    }
}

/// RAII guard for indentation scope.
pub struct IndentScope<'a> {
    formatter: &'a mut ConsoleFormatter,
}

impl<'a> IndentScope<'a> {
    pub fn new(formatter: &'a mut ConsoleFormatter) -> Self {
        formatter.indent();
        Self { formatter }
    }
}

impl<'a> Drop for IndentScope<'a> {
    fn drop(&mut self) {
        self.formatter.dedent();
    }
}
