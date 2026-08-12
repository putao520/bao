// @trace REQ-ENG-001
//! Hot module reloader — file watcher for HMR.
//!
//! Uses `notify` crate for file system watching when available.
//! Phase 1: polling-based change detection with in-memory file timestamps.

use ::std::collections::HashMap;
use ::std::path::PathBuf;
use ::std::sync::Mutex;
use ::std::time::SystemTime;

pub struct HotReloader {
    watched: Mutex<HashMap<PathBuf, SystemTime>>,
    imports: Mutex<HashMap<String, Vec<String>>>,
}

impl HotReloader {
    pub fn new() -> Self {
        HotReloader {
            watched: Mutex::new(HashMap::new()),
            imports: Mutex::new(HashMap::new()),
        }
    }

    pub fn enable_hot_module_reloading(&self) {}

    pub fn watch(&self, path: &str) -> Result<(), ()> {
        let p = PathBuf::from(path);
        if let Ok(metadata) = ::std::fs::metadata(&p) {
            if let Ok(modified) = metadata.modified() {
                self.watched
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(p, modified);
            }
        }
        Ok(())
    }

    pub fn unwatch(&self, path: &str) {
        let p = PathBuf::from(path);
        self.watched
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&p);
    }

    pub fn has_changes(&self) -> bool {
        let watched = self.watched.lock().unwrap_or_else(|e| e.into_inner());
        for (path, prev_time) in watched.iter() {
            if let Ok(metadata) = ::std::fs::metadata(path) {
                if let Ok(modified) = metadata.modified() {
                    if modified > *prev_time {
                        return true;
                    }
                }
            }
        }
        false
    }

    pub fn changed_files(&self) -> Vec<String> {
        let mut watched = self.watched.lock().unwrap_or_else(|e| e.into_inner());
        let mut changed = Vec::new();
        for (path, prev_time) in watched.iter_mut() {
            if let Ok(metadata) = ::std::fs::metadata(path) {
                if let Ok(modified) = metadata.modified() {
                    if modified > *prev_time {
                        changed.push(path.to_string_lossy().to_string());
                        *prev_time = modified;
                    }
                }
            }
        }
        changed
    }
}

impl Default for HotReloader {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ImportWatcher {
    imports: Mutex<HashMap<String, Vec<String>>>,
}

impl ImportWatcher {
    pub fn new() -> Self {
        ImportWatcher {
            imports: Mutex::new(HashMap::new()),
        }
    }

    pub fn register_import(&self, from: &str, to: &str) {
        self.imports
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(from.to_string())
            .or_default()
            .push(to.to_string());
    }

    pub fn get_importers(&self, path: &str) -> Vec<String> {
        let imports = self.imports.lock().unwrap_or_else(|e| e.into_inner());
        let mut result = Vec::new();
        for (from, targets) in imports.iter() {
            if targets.iter().any(|t| t == path) {
                result.push(from.clone());
            }
        }
        result
    }
}

impl Default for ImportWatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_watcher() {
        let w = ImportWatcher::new();
        w.register_import("a.js", "b.js");
        w.register_import("c.js", "b.js");
        let importers = w.get_importers("b.js");
        assert_eq!(importers.len(), 2);
    }

    #[test]
    fn no_changes_initially() {
        let h = HotReloader::new();
        assert!(!h.has_changes());
    }
}
