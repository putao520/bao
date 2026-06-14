//! JSC Options — API compatibility stub.

pub struct OptionsJsc {
    _private: (),
}

impl OptionsJsc {
    pub fn new() -> Self {
        OptionsJsc { _private: () }
    }
}

impl Default for OptionsJsc {
    fn default() -> Self {
        Self::new()
    }
}

pub struct VMOptions {
    _private: (),
}

impl VMOptions {
    pub fn new() -> Self {
        VMOptions { _private: () }
    }
}

impl Default for VMOptions {
    fn default() -> Self {
        Self::new()
    }
}
