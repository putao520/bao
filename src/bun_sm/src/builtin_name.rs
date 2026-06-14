// @trace REQ-ENG-001
// Builtin name registry for SpiderMonkey.
// Provides interned names for built-in JS objects and constructors,
// matching servo's WebIDL-generated builtins.

/// Registry of built-in JS constructor and prototype names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinName {
    // ECMAScript builtins
    Object,
    Array,
    Function,
    String,
    Number,
    Boolean,
    Symbol,
    BigInt,
    Promise,
    Map,
    Set,
    WeakMap,
    WeakSet,
    Date,
    RegExp,
    Error,
    TypeError,
    RangeError,
    SyntaxError,
    ReferenceError,
    UriError,
    EvalError,
    ArrayBuffer,
    SharedArrayBuffer,
    DataView,
    Int8Array,
    Uint8Array,
    Uint8ClampedArray,
    Int16Array,
    Uint16Array,
    Int32Array,
    Uint32Array,
    Float32Array,
    Float64Array,
    BigInt64Array,
    BigUint64Array,
    // Web API builtins
    Response,
    Request,
    Headers,
    URL,
    URLSearchParams,
    FormData,
    Blob,
    File,
    ReadableStream,
    WritableStream,
    TransformStream,
    AbortController,
    AbortSignal,
    Event,
    CustomEvent,
    MessageEvent,
    // Bun/Bao builtins
    Bun,
    Process,
    Buffer,
}

impl BuiltinName {
    /// Get the JS-visible name string for this builtin.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Object => "Object",
            Self::Array => "Array",
            Self::Function => "Function",
            Self::String => "String",
            Self::Number => "Number",
            Self::Boolean => "Boolean",
            Self::Symbol => "Symbol",
            Self::BigInt => "BigInt",
            Self::Promise => "Promise",
            Self::Map => "Map",
            Self::Set => "Set",
            Self::WeakMap => "WeakMap",
            Self::WeakSet => "WeakSet",
            Self::Date => "Date",
            Self::RegExp => "RegExp",
            Self::Error => "Error",
            Self::TypeError => "TypeError",
            Self::RangeError => "RangeError",
            Self::SyntaxError => "SyntaxError",
            Self::ReferenceError => "ReferenceError",
            Self::UriError => "URIError",
            Self::EvalError => "EvalError",
            Self::ArrayBuffer => "ArrayBuffer",
            Self::SharedArrayBuffer => "SharedArrayBuffer",
            Self::DataView => "DataView",
            Self::Int8Array => "Int8Array",
            Self::Uint8Array => "Uint8Array",
            Self::Uint8ClampedArray => "Uint8ClampedArray",
            Self::Int16Array => "Int16Array",
            Self::Uint16Array => "Uint16Array",
            Self::Int32Array => "Int32Array",
            Self::Uint32Array => "Uint32Array",
            Self::Float32Array => "Float32Array",
            Self::Float64Array => "Float64Array",
            Self::BigInt64Array => "BigInt64Array",
            Self::BigUint64Array => "BigUint64Array",
            Self::Response => "Response",
            Self::Request => "Request",
            Self::Headers => "Headers",
            Self::URL => "URL",
            Self::URLSearchParams => "URLSearchParams",
            Self::FormData => "FormData",
            Self::Blob => "Blob",
            Self::File => "File",
            Self::ReadableStream => "ReadableStream",
            Self::WritableStream => "WritableStream",
            Self::TransformStream => "TransformStream",
            Self::AbortController => "AbortController",
            Self::AbortSignal => "AbortSignal",
            Self::Event => "Event",
            Self::CustomEvent => "CustomEvent",
            Self::MessageEvent => "MessageEvent",
            Self::Bun => "Bun",
            Self::Process => "process",
            Self::Buffer => "Buffer",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_name_as_str() {
        assert_eq!(BuiltinName::Object.as_str(), "Object");
        assert_eq!(BuiltinName::Promise.as_str(), "Promise");
        assert_eq!(BuiltinName::Response.as_str(), "Response");
        assert_eq!(BuiltinName::Bun.as_str(), "Bun");
        assert_eq!(BuiltinName::Process.as_str(), "process");
    }

    #[test]
    fn builtin_name_equality() {
        assert_eq!(BuiltinName::Array, BuiltinName::Array);
        assert_ne!(BuiltinName::Array, BuiltinName::Object);
    }
}
