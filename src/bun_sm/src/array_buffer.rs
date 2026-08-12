//! SM-backed ArrayBuffer.
//!
//! Wraps `*mut JSObject` representing an ArrayBuffer or TypedArray.
//! Provides methods for creation, data access, and detach checks.

use ::std::marker::PhantomData;
use ::std::ptr::NonNull;

use mozjs::jsapi::*;
use mozjs::jsval::{ObjectValue, UndefinedValue};

use crate::js_value::JSValue;
use crate::strong::Strong;

// ─── ArrayBuffer ─────────────────────────────────────────────────────────────

/// ArrayBuffer wrapper for SpiderMonkey.
pub struct ArrayBuffer {
    obj: NonNull<JSObject>,
}

impl ArrayBuffer {
    /// Create a new ArrayBuffer with the given byte length.
    ///
    /// # Safety
    /// `cx` must be a valid JSContext.
    pub unsafe fn new(cx: *mut JSContext, len: usize) -> Option<Self> {
        let obj = unsafe { NewArrayBuffer(cx, len) };
        NonNull::new(obj).map(|o| ArrayBuffer { obj: o })
    }

    /// Create from an existing JSObject.
    ///
    /// # Safety
    /// `obj` must be a valid JSObject that is an ArrayBuffer.
    pub unsafe fn from_object(obj: *mut JSObject) -> Option<Self> {
        NonNull::new(obj).map(|o| ArrayBuffer { obj: o })
    }

    /// Get the underlying JSObject pointer.
    pub fn as_object(&self) -> *mut JSObject {
        self.obj.as_ptr()
    }

    /// Convert to a JSValue.
    pub fn to_js_value(&self) -> JSValue {
        JSValue::from_object(self.obj.as_ptr())
    }

    /// Get a slice of the ArrayBuffer's data.
    ///
    /// # Safety
    /// `cx` must be a valid JSContext. The returned slice is valid only
    /// as long as the ArrayBuffer is not detached or GC'd.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn as_slice(&self, _cx: *mut JSContext) -> Option<&[u8]> {
        let mut data_len: usize = 0;
        let mut is_shared: bool = false;
        let mut data_ptr: *mut u8 = ::std::ptr::null_mut();
        unsafe {
            GetArrayBufferLengthAndData(
                self.obj.as_ptr(),
                &mut data_len,
                &mut is_shared,
                &mut data_ptr,
            )
        };
        if data_ptr.is_null() || data_len == 0 {
            return None;
        }
        Some(unsafe { ::std::slice::from_raw_parts(data_ptr as *const u8, data_len) })
    }

    /// Get the byte length of the ArrayBuffer.
    ///
    /// # Safety
    /// `cx` must be a valid JSContext.
    #[allow(unsafe_op_in_unsafe_fn)]
    pub unsafe fn length(&self, _cx: *mut JSContext) -> usize {
        let mut data_len: usize = 0;
        let mut is_shared: bool = false;
        let mut data_ptr: *mut u8 = ::std::ptr::null_mut();
        unsafe {
            GetArrayBufferLengthAndData(
                self.obj.as_ptr(),
                &mut data_len,
                &mut is_shared,
                &mut data_ptr,
            )
        };
        data_len
    }

    /// Check if the ArrayBuffer has been detached.
    pub fn is_detached(&self) -> bool {
        unsafe { IsDetachedArrayBufferObject(self.obj.as_ptr()) }
    }
}

// ─── ArrayBufferStrong ──────────────────────────────────────────────────────

/// Strong reference to an ArrayBuffer.
pub type ArrayBufferStrong = Strong<ArrayBuffer>;

// ─── MarkedArrayBuffer ──────────────────────────────────────────────────────

/// Marked array buffer (GC-rooted).
pub struct MarkedArrayBuffer {
    obj: Option<NonNull<JSObject>>,
}

impl MarkedArrayBuffer {
    /// Create an empty marked buffer.
    pub fn new() -> Self {
        Self { obj: None }
    }

    /// Create from an existing ArrayBuffer.
    pub fn from_array_buffer(ab: &ArrayBuffer) -> Self {
        Self { obj: Some(ab.obj) }
    }

    /// Get the underlying JSObject, if set.
    pub fn get(&self) -> Option<*mut JSObject> {
        self.obj.map(|o| o.as_ptr())
    }

    /// Check if this buffer is set.
    pub fn is_set(&self) -> bool {
        self.obj.is_some()
    }

    /// Clear the marked buffer.
    pub fn clear(&mut self) {
        self.obj = None;
    }
}

impl Default for MarkedArrayBuffer {
    fn default() -> Self {
        Self::new()
    }
}

// ─── TypedArrayType ─────────────────────────────────────────────────────────

/// Typed array type discriminant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypedArrayType {
    Int8,
    Uint8,
    Uint8Clamped,
    Int16,
    Uint16,
    Int32,
    Uint32,
    Float32,
    Float64,
    BigInt64,
    BigUint64,
}

impl TypedArrayType {
    /// Get the element size in bytes for this typed array type.
    pub fn element_size(&self) -> usize {
        match self {
            TypedArrayType::Int8 | TypedArrayType::Uint8 | TypedArrayType::Uint8Clamped => 1,
            TypedArrayType::Int16 | TypedArrayType::Uint16 => 2,
            TypedArrayType::Int32 | TypedArrayType::Uint32 | TypedArrayType::Float32 => 4,
            TypedArrayType::Float64 | TypedArrayType::BigInt64 | TypedArrayType::BigUint64 => 8,
        }
    }

    /// Check if this is an integer type.
    pub fn is_integer(&self) -> bool {
        !matches!(self, TypedArrayType::Float32 | TypedArrayType::Float64)
    }

    /// Check if this is a float type.
    pub fn is_float(&self) -> bool {
        matches!(self, TypedArrayType::Float32 | TypedArrayType::Float64)
    }
}
