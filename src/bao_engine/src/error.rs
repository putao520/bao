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
