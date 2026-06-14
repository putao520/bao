// @trace REQ-ENG-002 [module:bun_sm]
//! `RegularExpression` — SpiderMonkey RegExp wrapper.
//!
//! In JSC, `RegularExpression` is a YARR-based compiled regex. In SpiderMonkey,
//! RegExp is a `JSObject*` created via `JS::NewRegExpObject`. This module wraps
//! the SM API into a JSC-compatible API surface.

use ::std::ffi::CString;
use ::std::marker::PhantomData;

use mozjs::jsapi::{JSContext as RawJSContext, JSObject, Handle, MutableHandle, Value};
use mozjs::jsval::UndefinedValue;

/// RegExp flags — mirrors JS RegExp flag bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flags {
    bits: u8,
}

impl Flags {
    pub const NONE: Flags = Flags { bits: 0 };
    pub const GLOBAL: Flags = Flags { bits: mozjs::jsapi::RegExpFlag_Global };
    pub const IGNORE_CASE: Flags = Flags { bits: mozjs::jsapi::RegExpFlag_IgnoreCase };
    pub const MULTILINE: Flags = Flags { bits: mozjs::jsapi::RegExpFlag_Multiline };
    pub const DOT_ALL: Flags = Flags { bits: mozjs::jsapi::RegExpFlag_DotAll };
    pub const STICKY: Flags = Flags { bits: mozjs::jsapi::RegExpFlag_Sticky };
    pub const UNICODE: Flags = Flags { bits: mozjs::jsapi::RegExpFlag_Unicode };
    pub const HAS_INDICES: Flags = Flags { bits: mozjs::jsapi::RegExpFlag_HasIndices };
    pub const UNICODE_SETS: Flags = Flags { bits: mozjs::jsapi::RegExpFlag_UnicodeSets };

    pub fn is_global(&self) -> bool { self.bits & mozjs::jsapi::RegExpFlag_Global != 0 }
    pub fn is_ignore_case(&self) -> bool { self.bits & mozjs::jsapi::RegExpFlag_IgnoreCase != 0 }
    pub fn is_multiline(&self) -> bool { self.bits & mozjs::jsapi::RegExpFlag_Multiline != 0 }
    pub fn is_dot_all(&self) -> bool { self.bits & mozjs::jsapi::RegExpFlag_DotAll != 0 }
    pub fn is_sticky(&self) -> bool { self.bits & mozjs::jsapi::RegExpFlag_Sticky != 0 }
    pub fn is_unicode(&self) -> bool { self.bits & mozjs::jsapi::RegExpFlag_Unicode != 0 }

    fn to_sm_flags(&self) -> mozjs::jsapi::RegExpFlags {
        mozjs::jsapi::RegExpFlags { flags_: self.bits }
    }
}

impl ::std::ops::BitOr for Flags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self { Flags { bits: self.bits | rhs.bits } }
}

/// A compiled regular expression backed by SpiderMonkey's RegExpObject.
///
/// The inner `*mut JSObject` is NOT rooted — this type is only valid within
/// a single no-GC window. For persistent storage, use `GcStore`.
pub struct RegularExpression {
    obj: *mut JSObject,
    flags: Flags,
}

impl RegularExpression {
    /// Compile a new RegExp from a pattern string and flags.
    ///
    /// # Safety
    /// `cx` must be a valid JSContext.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn compile(cx: *mut RawJSContext, pattern: &str, flags: Flags) -> Option<Self> {
        let mut cx_ref = mozjs::context::JSContext::from_ptr(
            ::std::ptr::NonNull::new_unchecked(cx)
        );
        let c_pattern = CString::new(pattern).unwrap_or_default();
        let sm_flags = flags.to_sm_flags();
        let obj = unsafe {
            mozjs::rust::wrappers2::NewRegExpObject(
                &mut cx_ref,
                c_pattern.as_ptr(),
                pattern.len(),
                sm_flags,
            )
        };
        if obj.is_null() {
            None
        } else {
            Some(RegularExpression { obj, flags })
        }
    }

    /// Wrap an existing RegExp JSObject.
    ///
    /// # Safety
    /// `obj` must be a valid RegExpObject.
    pub unsafe fn from_object(obj: *mut JSObject) -> Self {
        RegularExpression { obj, flags: Flags::NONE }
    }

    /// Get the underlying JSObject pointer.
    pub fn as_object(&self) -> *mut JSObject {
        self.obj
    }

    /// Check if the pointer is null.
    pub fn is_null(&self) -> bool {
        self.obj.is_null()
    }

    /// Get the flags.
    pub fn flags(&self) -> Flags {
        self.flags
    }

    /// Test if the pattern matches the given string.
    ///
    /// # Safety
    /// `cx` must be a valid JSContext.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn test(&self, cx: *mut RawJSContext, input: &str) -> bool {
        if self.obj.is_null() {
            return false;
        }
        let chars: Vec<u16> = input.encode_utf16().collect();
        let mut index = 0usize;
        let mut rval = UndefinedValue();
        let re_h = Handle::<*mut JSObject> {
            _phantom_0: PhantomData,
            ptr: &self.obj,
        };
        unsafe {
            mozjs::jsapi::ExecuteRegExpNoStatics(
                cx,
                re_h,
                chars.as_ptr(),
                chars.len(),
                &mut index,
                true,
                MutableHandle::<Value> {
                    _phantom_0: PhantomData,
                    ptr: &mut rval,
                },
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_combine() {
        let f = Flags::GLOBAL | Flags::IGNORE_CASE;
        assert!(f.is_global());
        assert!(f.is_ignore_case());
        assert!(!f.is_multiline());
    }

    #[test]
    fn flags_none() {
        assert!(!Flags::NONE.is_global());
        assert!(!Flags::NONE.is_ignore_case());
    }

    #[test]
    fn regex_null_check() {
        let re = RegularExpression { obj: ::std::ptr::null_mut(), flags: Flags::NONE };
        assert!(re.is_null());
    }
}
