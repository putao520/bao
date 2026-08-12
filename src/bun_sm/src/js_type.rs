// @trace REQ-ENG-002 [module:bun_sm]
//! `JSType` — JavaScript type enumeration for SpiderMonkey.
//!
//! In JSC, `JSType` is a `u8` enum embedded in the `JSCell` header for
//! O(1) type identification. In SpiderMonkey, type checks are methods on
//! `JSVal` — there's no single enum. This module provides a `JSType` enum
//! for API compatibility with `bun_jsc`.

/// JavaScript type enumeration, compatible with `bun_jsc::JSType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum JSType {
    // ── Primitive types ──
    Undefined = 0,
    Null = 1,
    Boolean = 2,
    Number = 3,
    String = 4,
    Symbol = 5,
    BigInt = 6,

    // ── Object types ──
    Object = 10,
    Function = 11,
    Array = 12,
    Date = 13,
    RegExp = 14,
    Map = 15,
    Set = 16,
    Promise = 17,
    Error = 18,
    Arguments = 19,

    // ── Typed arrays ──
    Int8Array = 20,
    Uint8Array = 21,
    Int16Array = 22,
    Uint16Array = 23,
    Int32Array = 24,
    Uint32Array = 25,
    Float32Array = 26,
    Float64Array = 27,
    ArrayBuffer = 28,
    DataView = 29,

    // ── Internal types ──
    Cell = 30,
    Structure = 31,
    CodeBlock = 32,
}

impl JSType {
    /// Determine the JSType from a `JSValue`.
    ///
    /// Note: This is a coarse classification. SM doesn't expose the same
    /// granular type information as JSC's `JSCell::type()`.
    pub fn from_js_value(val: &crate::JSValue) -> Self {
        if val.is_undefined() {
            JSType::Undefined
        } else if val.is_null() {
            JSType::Null
        } else if val.is_boolean() {
            JSType::Boolean
        } else if val.is_number() {
            JSType::Number
        } else if val.is_string() {
            JSType::String
        } else if val.is_object() {
            JSType::Object
        } else {
            JSType::Undefined
        }
    }

    /// Check if this type is a primitive (not an object).
    pub fn is_primitive(&self) -> bool {
        matches!(
            self,
            JSType::Undefined
                | JSType::Null
                | JSType::Boolean
                | JSType::Number
                | JSType::String
                | JSType::Symbol
                | JSType::BigInt
        )
    }

    /// Check if this type is a typed array.
    pub fn is_typed_array(&self) -> bool {
        matches!(
            self,
            JSType::Int8Array
                | JSType::Uint8Array
                | JSType::Int16Array
                | JSType::Uint16Array
                | JSType::Int32Array
                | JSType::Uint32Array
                | JSType::Float32Array
                | JSType::Float64Array
        )
    }

    /// Check if this type is a function.
    pub fn is_function(&self) -> bool {
        matches!(self, JSType::Function)
    }

    /// Check if this type is an object (any kind).
    pub fn is_object(&self) -> bool {
        !self.is_primitive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::JSValue;

    #[test]
    fn undefined_type() {
        assert_eq!(
            JSType::from_js_value(&JSValue::UNDEFINED),
            JSType::Undefined
        );
    }

    #[test]
    fn null_type() {
        assert_eq!(JSType::from_js_value(&JSValue::NULL), JSType::Null);
    }

    #[test]
    fn boolean_type() {
        assert_eq!(JSType::from_js_value(&JSValue::TRUE), JSType::Boolean);
    }

    #[test]
    fn number_type() {
        assert_eq!(JSType::from_js_value(&JSValue::ONE), JSType::Number);
    }

    #[test]
    fn string_type() {
        assert_eq!(
            JSType::from_js_value(&JSValue::from_string("hello".into())),
            JSType::String
        );
    }

    #[test]
    fn primitive_check() {
        assert!(JSType::Undefined.is_primitive());
        assert!(JSType::Number.is_primitive());
        assert!(!JSType::Object.is_primitive());
        assert!(!JSType::Function.is_primitive());
    }

    #[test]
    fn typed_array_check() {
        assert!(JSType::Int8Array.is_typed_array());
        assert!(JSType::Float64Array.is_typed_array());
        assert!(!JSType::ArrayBuffer.is_typed_array());
        assert!(!JSType::Object.is_typed_array());
    }
}
