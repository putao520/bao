// @trace REQ-ENG-001
// Fetch headers stub for SpiderMonkey.
// Provides the Rust-side header representation for HTTP fetch operations.
// The JS-side Headers object is provided by servo's DOM.

use std::collections::HashMap;

/// Case-insensitive HTTP headers stored as a HashMap.
#[derive(Debug, Clone, Default)]
pub struct FetchHeaders {
    headers: HashMap<String, String>,
}

impl FetchHeaders {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_iter<I: Iterator<Item = (String, String)>>(iter: I) -> Self {
        let mut headers = HashMap::new();
        for (key, value) in iter {
            headers.insert(key.to_ascii_lowercase(), value);
        }
        Self { headers }
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_ascii_lowercase()).map(|s| s.as_str())
    }

    pub fn set(&mut self, name: &str, value: String) {
        self.headers.insert(name.to_ascii_lowercase(), value);
    }

    pub fn has(&self, name: &str) -> bool {
        self.headers.contains_key(&name.to_ascii_lowercase())
    }

    pub fn delete(&mut self, name: &str) -> bool {
        self.headers.remove(&name.to_ascii_lowercase()).is_some()
    }

    pub fn len(&self) -> usize {
        self.headers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.headers.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.headers.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetch_headers_case_insensitive() {
        let mut h = FetchHeaders::new();
        h.set("Content-Type", "text/html".into());
        assert_eq!(h.get("content-type"), Some("text/html"));
        assert_eq!(h.get("CONTENT-TYPE"), Some("text/html"));
    }

    #[test]
    fn fetch_headers_delete() {
        let mut h = FetchHeaders::new();
        h.set("X-Custom", "value".into());
        assert!(h.delete("x-custom"));
        assert!(!h.has("X-Custom"));
    }

    #[test]
    fn fetch_headers_from_iter() {
        let h = FetchHeaders::from_iter(
            vec![("Name".into(), "val".into())].into_iter()
        );
        assert_eq!(h.get("name"), Some("val"));
    }
}
