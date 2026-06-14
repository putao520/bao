// @trace REQ-ENG-002 [entity:CodegenBackend]
// Hand-written Rust surface for the codegen output — adapted for SpiderMonkey.
//
// Two generators feed this module:
//   1. bindgen option-structs — GenOpt<T>, GenVal<T>, GenList<T> accessors
//   2. per-class cached accessor modules — codegen_cached_accessors! macro
//
// Until both generators grow a .rs backend, this file provides the
// common accessor shapes downstream crates reference directly.

/// Optional-value accessor: `field.get() -> Option<T>`.
#[derive(Debug, Default, Clone)]
pub struct GenOpt<T>(Option<T>);

impl<T: Clone> GenOpt<T> {
    #[inline]
    pub fn get(&self) -> Option<T> {
        self.0.clone()
    }

    #[inline]
    pub fn set(&mut self, val: Option<T>) {
        self.0 = val;
    }
}

/// Required-value accessor: `field.get() -> T`.
#[derive(Debug, Clone)]
pub struct GenVal<T>(T);

impl<T: Clone> GenVal<T> {
    #[inline]
    pub fn get(&self) -> T {
        self.0.clone()
    }

    #[inline]
    pub fn set(&mut self, val: T) {
        self.0 = val;
    }
}

/// Array accessor: `field.items() -> &[T]`.
#[derive(Debug, Default, Clone)]
pub struct GenList<T>(Vec<T>);

impl<T> GenList<T> {
    #[inline]
    pub fn items(&self) -> &[T] {
        &self.0
    }

    #[inline]
    pub fn push(&mut self, val: T) {
        self.0.push(val);
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Bindgen option-struct for `BunObject` — field order matches
/// `GeneratedBindings.zig` (parse, tokenize).
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct BracesOptions {
    pub open: u32,
    pub close: u32,
    pub comma: u32,
}

/// Bindgen option-struct for `ProcessConfig`.
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct ProcessConfigOptions {
    pub cwd_ptr: *const u8,
    pub cwd_len: usize,
    pub envc: u32,
}

// SAFETY: ProcessConfigOptions contains raw pointers that are never
// dereferenced by generated code — they are opaque values passed through
// the C ABI shim to the underlying dispatch.
unsafe impl Send for ProcessConfigOptions {}
unsafe impl Sync for ProcessConfigOptions {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gen_opt_some() {
        let mut opt = GenOpt(Some(42u32));
        assert_eq!(opt.get(), Some(42));
        opt.set(None);
        assert_eq!(opt.get(), None);
    }

    #[test]
    fn gen_opt_none() {
        let opt: GenOpt<u32> = GenOpt::default();
        assert_eq!(opt.get(), None);
    }

    #[test]
    fn gen_val_get() {
        let val = GenVal(3.14f64);
        assert_eq!(val.get(), 3.14);
    }

    #[test]
    fn gen_val_set() {
        let mut val = GenVal("hello".to_string());
        val.set("world".to_string());
        assert_eq!(val.get(), "world");
    }

    #[test]
    fn gen_list_items() {
        let mut list = GenList::default();
        assert!(list.is_empty());
        list.push(1u32);
        list.push(2);
        list.push(3);
        assert_eq!(list.items(), &[1, 2, 3]);
        assert_eq!(list.len(), 3);
    }

    #[test]
    fn braces_options_default() {
        let opts = BracesOptions::default();
        assert_eq!(opts.open, 0);
        assert_eq!(opts.close, 0);
        assert_eq!(opts.comma, 0);
    }

    #[test]
    fn process_config_default() {
        let opts = ProcessConfigOptions::default();
        assert!(opts.cwd_ptr.is_null());
        assert_eq!(opts.cwd_len, 0);
        assert_eq!(opts.envc, 0);
    }
}
