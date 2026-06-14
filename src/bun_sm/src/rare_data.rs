// @trace REQ-ENG-001
// RareData stub for SpiderMonkey.
// Ported from Bun's RareData pattern — a bag of lazy-init optional subsystems.
// Low-tier storage for erased pointers; high tier (bao_runtime) owns typed accessors.

use std::collections::HashMap;
use std::ffi::c_void;

/// Bag of lazily-initialized subsystem state. Stores erased pointers
/// that high-tier crates (bao_runtime) can downcast to concrete types.
pub struct RareData {
    slots: HashMap<&'static str, *mut c_void>,
    hot_map: HotMap,
}

/// Low-tier hot map storage: (tag, ptr) pairs per docs/PORTING.md Dispatch pattern.
#[derive(Default)]
pub struct HotMap {
    entries: Vec<HotMapEntry>,
}

#[derive(Clone)]
pub struct HotMapEntry {
    pub tag: u32,
    pub ptr: *mut c_void,
}

// SAFETY: HotMapEntry contains raw pointers treated as opaque values
// passed through the dispatch layer.
unsafe impl Send for HotMapEntry {}
unsafe impl Sync for HotMapEntry {}

impl RareData {
    pub fn new() -> Self {
        Self {
            slots: HashMap::new(),
            hot_map: HotMap::default(),
        }
    }

    /// Store an erased pointer under the given key.
    pub fn set(&mut self, key: &'static str, ptr: *mut c_void) {
        self.slots.insert(key, ptr);
    }

    /// Retrieve an erased pointer by key.
    pub fn get(&self, key: &str) -> Option<*mut c_void> {
        self.slots.get(key).copied()
    }

    /// Remove a stored pointer by key.
    pub fn remove(&mut self, key: &str) -> Option<*mut c_void> {
        self.slots.remove(key)
    }

    /// Access the hot map for fast tagged lookup.
    pub fn hot_map(&self) -> &HotMap {
        &self.hot_map
    }

    /// Access the hot map mutably.
    pub fn hot_map_mut(&mut self) -> &mut HotMap {
        &mut self.hot_map
    }
}

impl Default for RareData {
    fn default() -> Self {
        Self::new()
    }
}

impl HotMap {
    pub fn insert(&mut self, tag: u32, ptr: *mut c_void) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.tag == tag) {
            entry.ptr = ptr;
        } else {
            self.entries.push(HotMapEntry { tag, ptr });
        }
    }

    pub fn get(&self, tag: u32) -> Option<*mut c_void> {
        self.entries.iter().find(|e| e.tag == tag).map(|e| e.ptr)
    }

    pub fn remove(&mut self, tag: u32) -> Option<*mut c_void> {
        if let Some(pos) = self.entries.iter().position(|e| e.tag == tag) {
            Some(self.entries.remove(pos).ptr)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rare_data_set_get() {
        let mut rd = RareData::new();
        let val = 42usize as *mut c_void;
        rd.set("test", val);
        assert_eq!(rd.get("test"), Some(val));
    }

    #[test]
    fn rare_data_remove() {
        let mut rd = RareData::new();
        rd.set("test", std::ptr::null_mut());
        assert!(rd.remove("test").is_some());
        assert!(rd.get("test").is_none());
    }

    #[test]
    fn hot_map_insert_get() {
        let mut hm = HotMap::default();
        let ptr = 0x1000usize as *mut c_void;
        hm.insert(1, ptr);
        assert_eq!(hm.get(1), Some(ptr));
        assert_eq!(hm.get(2), None);
    }

    #[test]
    fn hot_map_overwrite() {
        let mut hm = HotMap::default();
        hm.insert(1, std::ptr::null_mut());
        let new_ptr = 0x2000usize as *mut c_void;
        hm.insert(1, new_ptr);
        assert_eq!(hm.get(1), Some(new_ptr));
    }
}
