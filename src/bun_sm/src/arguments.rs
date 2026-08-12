//! SM-backed argument types.

use crate::js_value::JSValue;
use mozjs::jsapi::JSContext;

/// Slice-based argument access (JSC compatibility).
pub struct ArgumentsSlice<'a> {
    values: &'a [JSValue],
    cx: *mut JSContext,
}

impl<'a> ArgumentsSlice<'a> {
    pub fn new(cx: *mut JSContext, values: &'a [JSValue]) -> Self {
        Self { values, cx }
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn get(&self, index: usize) -> JSValue {
        self.values
            .get(index)
            .cloned()
            .unwrap_or(JSValue::UNDEFINED)
    }

    /// Get the JSContext pointer.
    pub fn cx(&self) -> *mut JSContext {
        self.cx
    }
}

/// GC-rooted argument buffer for JS function calls.
pub struct MarkedArgumentBuffer {
    _private: (),
}

impl MarkedArgumentBuffer {
    pub fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for MarkedArgumentBuffer {
    fn default() -> Self {
        Self::new()
    }
}
