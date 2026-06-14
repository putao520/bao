// @trace REQ-ENG-002
// Common interned strings for SpiderMonkey JS atoms.
// Provides a lazily-initialized set of frequently used JS property names
// to avoid repeated string allocation when accessing JS object properties.

use std::ptr::NonNull;
use mozjs::jsapi::{JSContext, JSString};

/// Lazily-interned JS atom strings. Initialized once per JSContext lifetime.
pub struct CommonStrings {
    /// Interned JSString pointers for common property names.
    atoms: Box<[Option<NonNull<JSString>>]>,
}

const ATOM_NAMES: &[&str] = &[
    "length",
    "prototype",
    "constructor",
    "__proto__",
    "toString",
    "valueOf",
    "then",
    "catch",
    "finally",
    "resolve",
    "reject",
    "promise",
    "name",
    "message",
    "stack",
    "code",
    "errno",
    "path",
    "fd",
    "buffer",
    "encoding",
    "mode",
    "flag",
    "data",
    "error",
    "result",
    "value",
    "key",
    "type",
    "url",
    "method",
    "headers",
    "body",
    "status",
    "statusText",
    "ok",
    "redirected",
    "arrayBuffer",
    "json",
    "text",
    "blob",
];

impl CommonStrings {
    /// Create an empty CommonStrings (atoms not yet interned).
    pub fn new() -> Self {
        Self {
            atoms: vec![None; ATOM_NAMES.len()].into_boxed_slice(),
        }
    }

    /// Intern all common strings for the given JSContext.
    /// Must be called once after the JSContext is created.
    pub unsafe fn init(&mut self, cx: *mut JSContext) {
        use std::ffi::CString;
        use mozjs::jsapi::JS_NewStringCopyZ;

        for (i, &name) in ATOM_NAMES.iter().enumerate() {
            let c_str = CString::new(name).unwrap_or_default();
            // SAFETY: cx is a valid JSContext, c_str is a valid null-terminated C string.
            let js_str = unsafe { JS_NewStringCopyZ(cx, c_str.as_ptr()) };
            if !js_str.is_null() {
                self.atoms[i] = NonNull::new(js_str);
            }
        }
    }

    /// Get the interned JSString for a common atom by index.
    #[inline]
    pub fn get(&self, atom: CommonAtom) -> Option<NonNull<JSString>> {
        self.atoms[atom as usize]
    }

    /// Number of interned atoms.
    #[inline]
    pub fn len() -> usize {
        ATOM_NAMES.len()
    }
}

impl Default for CommonStrings {
    fn default() -> Self {
        Self::new()
    }
}

/// Indices into the CommonStrings atom table.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommonAtom {
    Length = 0,
    Prototype = 1,
    Constructor = 2,
    ProtoDunder = 3,
    ToString = 4,
    ValueOf = 5,
    Then = 6,
    Catch = 7,
    Finally = 8,
    Resolve = 9,
    Reject = 10,
    Promise = 11,
    Name = 12,
    Message = 13,
    Stack = 14,
    Code = 15,
    Errno = 16,
    Path = 17,
    Fd = 18,
    Buffer = 19,
    Encoding = 20,
    Mode = 21,
    Flag = 22,
    Data = 23,
    Error = 24,
    Result = 25,
    Value = 26,
    Key = 27,
    Type = 28,
    Url = 29,
    Method = 30,
    Headers = 31,
    Body = 32,
    Status = 33,
    StatusText = 34,
    Ok = 35,
    Redirected = 36,
    ArrayBuffer = 37,
    Json = 38,
    Text = 39,
    Blob = 40,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_strings_len() {
        assert_eq!(CommonStrings::len(), ATOM_NAMES.len());
        assert_eq!(ATOM_NAMES.len(), 41);
    }

    #[test]
    fn common_strings_default() {
        let cs = CommonStrings::default();
        assert!(cs.get(CommonAtom::Length).is_none());
        assert!(cs.get(CommonAtom::Prototype).is_none());
    }

    #[test]
    fn atom_names_match_enum() {
        assert_eq!(ATOM_NAMES[CommonAtom::Length as usize], "length");
        assert_eq!(ATOM_NAMES[CommonAtom::Prototype as usize], "prototype");
        assert_eq!(ATOM_NAMES[CommonAtom::Constructor as usize], "constructor");
        assert_eq!(ATOM_NAMES[CommonAtom::Then as usize], "then");
        assert_eq!(ATOM_NAMES[CommonAtom::Url as usize], "url");
        assert_eq!(ATOM_NAMES[CommonAtom::Status as usize], "status");
    }
}
