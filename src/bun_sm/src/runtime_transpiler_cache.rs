// @trace REQ-ENG-001
//! Runtime transpiler cache — hash-based cache for transpiled TS/JSX output.
//!
//! Backed by an in-memory HashMap. Phase 2 will add file-system persistence
//! via `bun_cache`.

use ::std::collections::HashMap;
use ::std::sync::Mutex;

pub const IS_DISABLED: bool = false;

#[derive(Debug, Clone)]
pub struct Entry {
    pub key: String,
    pub source_hash: u64,
    pub output: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranspilerCacheImplKind {
    None,
    InMemory,
    FileSystem,
}

pub struct RuntimeTranspilerCache {
    store: Mutex<HashMap<String, Entry>>,
    impl_kind: TranspilerCacheImplKind,
}

impl RuntimeTranspilerCache {
    pub fn new() -> Self {
        RuntimeTranspilerCache {
            store: Mutex::new(HashMap::new()),
            impl_kind: TranspilerCacheImplKind::InMemory,
        }
    }

    pub fn get(&self, key: &str) -> Option<String> {
        self.store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .map(|e| e.output.clone())
    }

    pub fn set(&self, key: &str, value: &str) {
        let entry = Entry {
            key: key.to_string(),
            source_hash: 0,
            output: value.to_string(),
        };
        self.store
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.to_string(), entry);
    }

    pub fn clear(&self) {
        self.store.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }

    pub fn is_disabled(&self) -> bool {
        IS_DISABLED
    }

    pub fn len(&self) -> usize {
        self.store.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

impl Default for RuntimeTranspilerCache {
    fn default() -> Self {
        Self::new()
    }
}

pub struct RuntimeTranspilerStore {
    data: Mutex<HashMap<String, Vec<u8>>>,
}

impl RuntimeTranspilerStore {
    pub fn new() -> Self {
        RuntimeTranspilerStore {
            data: Mutex::new(HashMap::new()),
        }
    }

    pub fn get(&self, hash: &str) -> Option<Vec<u8>> {
        self.data
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(hash)
            .cloned()
    }

    pub fn set(&self, hash: &str, data: &[u8]) {
        self.data
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(hash.to_string(), data.to_vec());
    }

    pub fn len(&self) -> usize {
        self.data.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

impl Default for RuntimeTranspilerStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_get_set() {
        let cache = RuntimeTranspilerCache::new();
        assert!(cache.get("key1").is_none());
        cache.set("key1", "output1");
        assert_eq!(cache.get("key1"), Some("output1".to_string()));
    }

    #[test]
    fn cache_clear() {
        let cache = RuntimeTranspilerCache::new();
        cache.set("a", "1");
        cache.set("b", "2");
        assert_eq!(cache.len(), 2);
        cache.clear();
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn cache_overwrite() {
        let cache = RuntimeTranspilerCache::new();
        cache.set("key", "v1");
        cache.set("key", "v2");
        assert_eq!(cache.get("key"), Some("v2".to_string()));
    }

    #[test]
    fn store_get_set() {
        let store = RuntimeTranspilerStore::new();
        assert!(store.get("h1").is_none());
        store.set("h1", &[1, 2, 3]);
        assert_eq!(store.get("h1"), Some(vec![1, 2, 3]));
    }

    #[test]
    fn not_disabled() {
        assert!(!IS_DISABLED);
    }
}
