// @trace REQ-ENG-001
use ::std::fmt;

#[derive(Debug)]
pub struct JsError {
    pub message: ::std::string::String,
    pub filename: ::std::string::String,
    pub line: u32,
    pub column: u32,
    pub stack: ::std::option::Option<::std::string::String>,
}

impl fmt::Display for JsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}: {}", self.filename, self.line, self.column, self.message)?;
        if let ::std::option::Option::Some(ref stack) = self.stack {
            write!(f, "\n{}", stack)?;
        }
        ::std::result::Result::Ok(())
    }
}

impl ::std::error::Error for JsError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_without_stack() {
        let err = JsError {
            message: "something went wrong".into(),
            filename: "test.js".into(),
            line: 10,
            column: 5,
            stack: None,
        };
        assert_eq!(format!("{err}"), "test.js:10:5: something went wrong");
    }

    #[test]
    fn display_with_stack() {
        let err = JsError {
            message: "oops".into(),
            filename: "app.js".into(),
            line: 1,
            column: 1,
            stack: Some("  at foo (app.js:1:1)\n  at bar (app.js:2:2)".into()),
        };
        let displayed = format!("{err}");
        assert!(displayed.starts_with("app.js:1:1: oops\n"));
        assert!(displayed.contains("at foo"));
    }

    #[test]
    fn error_trait_is_implemented() {
        let err = JsError {
            message: "test".into(),
            filename: "f.js".into(),
            line: 0,
            column: 0,
            stack: None,
        };
        let _: &dyn std::error::Error = &err;
    }

    #[test]
    fn debug_format_includes_all_fields() {
        let err = JsError {
            message: "err".into(),
            filename: "a.js".into(),
            line: 3,
            column: 7,
            stack: Some("trace".into()),
        };
        let debug = format!("{err:?}");
        assert!(debug.contains("err"));
        assert!(debug.contains("a.js"));
    }

    #[test]
    fn zero_position_formats_cleanly() {
        let err = JsError {
            message: "x".into(),
            filename: "y".into(),
            line: 0,
            column: 0,
            stack: None,
        };
        assert_eq!(format!("{err}"), "y:0:0: x");
    }
}
