//! JsClass trait — SM-backed class binding.

/// Trait for JS class bindings. In JSC, this is provided by `#[bun_jsc::JsClass]`.
/// In SM, it maps to `JS_InitClass` + reserved slot pattern.
pub trait JsClass: Sized + 'static {
    /// The JS class name.
    const NAME: &'static [u8];

    /// Whether this class has a constructor.
    const HAS_CONSTRUCTOR: bool = true;

    /// Whether this class has a finalizer.
    const HAS_FINALIZE: bool = true;
}
