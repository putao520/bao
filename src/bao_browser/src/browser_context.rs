// REQ-CDS-009: in-process CDP browser context registry.
// @trace REQ-CDS-009 [entity:BrowserContextRegistry]
//
// Owns the explicit `Target.createBrowserContext` ids and the page→context
// attribution table for every live page. The registry lives in the PagePool's
// pump-thread domain (same `Rc`/`RefCell` discipline as the pool itself —
// REQ-PERF-002), because disposal must close the context's pages through the
// pool and every attribution query happens on the bridge drain path.
//
// Criterion 6 (resource partitioning): every explicit context owns one
// in-process `ContextPartitionId` (minted via `Servo::create_context_partition`)
// — its own cookie jar / HTTP cache on the servo net side and its own
// webstorage / IndexedDB / CacheStorage group on the storage side. Pages are
// attributed to their context's partition at WebView creation, so the implicit
// default context (no partition) keeps the pre-C6 channel-driven behavior.

use std::collections::HashMap;

use bao_cdp::servo_bridge::DEFAULT_BROWSER_CONTEXT_ID;
use servo::ContextPartitionId;

/// One explicit browser context. Chrome keeps a disposeOnDetach flag on the
/// wire; bao serves browser-level sessions only, so detach-driven disposal
/// has no trigger face and the entry stays a bare identity.
#[derive(Debug)]
struct BrowserContextEntry {
    id: String,
    /// REQ-CDS-009 C6: the in-process resource partition this context owns —
    /// isolated cookie jar / HTTP cache (servo net side) and webstorage /
    /// IndexedDB / CacheStorage group (storage side). Minted at context
    /// creation via `Servo::create_context_partition`.
    partition: ContextPartitionId,
}

/// In-process browser context registry: explicit-context identities plus the
/// page→context attribution for every live page. Default-context membership
/// is implicit (absence from the attribution table), so pages created outside
/// any explicit context need no bookkeeping.
#[derive(Default)]
pub struct BrowserContextRegistry {
    contexts: HashMap<String, BrowserContextEntry>,
    page_contexts: HashMap<usize, String>,
    next_id: u64,
}

impl BrowserContextRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mint a unique explicit context id (`bao-context-<n>`) bound to the
    /// partition created for it. The implicit default id can never collide
    /// (different prefix, never minted here).
    pub fn create_context(&mut self, partition: ContextPartitionId) -> String {
        self.next_id += 1;
        let id = format!("bao-context-{:016x}", self.next_id);
        self.contexts.insert(
            id.clone(),
            BrowserContextEntry {
                id: id.clone(),
                partition,
            },
        );
        id
    }

    /// The partition a context owns. `None` for the implicit default (pages
    /// of the default context need no partition) and unknown ids.
    pub fn partition_of_context(&self, id: &str) -> Option<ContextPartitionId> {
        self.contexts.get(id).map(|entry| entry.partition)
    }

    /// Destroy one explicit context and return the ids of every page that
    /// belonged to it (the caller closes them through the pool). Sorted for
    /// deterministic close/event ordering — HashMap iteration order must not
    /// leak into observable behavior. The implicit default is not a registry
    /// entry — disposing it is "not found", exactly like Chrome, which never
    /// lists it either.
    pub fn dispose_context(&mut self, id: &str) -> Result<Vec<usize>, String> {
        if self.contexts.remove(id).is_none() {
            return Err(format!("Failed to find browser context: {id}"));
        }
        let mut orphaned: Vec<usize> = self
            .page_contexts
            .iter()
            .filter(|(_, ctx)| ctx.as_str() == id)
            .map(|(page, _)| *page)
            .collect();
        orphaned.sort_unstable();
        for page in &orphaned {
            self.page_contexts.remove(page);
        }
        Ok(orphaned)
    }

    /// Chrome `Target.getBrowserContexts` semantics: the explicit contexts
    /// only — the implicit default is not listed.
    pub fn list_contexts(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.contexts.keys().cloned().collect();
        ids.sort();
        ids
    }

    pub fn context_exists(&self, id: &str) -> bool {
        self.contexts.contains_key(id)
    }

    /// Record a page's context. `None` / the default id stay unrecorded —
    /// the default is implicit.
    pub fn attribute_page(&mut self, page_id: usize, context_id: Option<&str>) {
        match context_id {
            Some(ctx) if ctx != DEFAULT_BROWSER_CONTEXT_ID && self.context_exists(ctx) => {
                self.page_contexts.insert(page_id, ctx.to_string());
            }
            _ => {
                self.page_contexts.remove(&page_id);
            }
        }
    }

    /// The context a live page belongs to. Absence = the implicit default.
    pub fn context_of_page(&self, page_id: usize) -> &str {
        self.page_contexts
            .get(&page_id)
            .map(|s| s.as_str())
            .unwrap_or(DEFAULT_BROWSER_CONTEXT_ID)
    }

    /// Drop a page's attribution (page closed — idempotent).
    pub fn remove_page(&mut self, page_id: usize) {
        self.page_contexts.remove(&page_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // @trace TEST-CDS-009 [req:REQ-CDS-009] [criterion:1] [level:unit]
    #[test]
    fn create_context_mints_unique_non_default_ids() {
        let mut reg = BrowserContextRegistry::new();
        let a = reg.create_context(ContextPartitionId::new());
        let b = reg.create_context(ContextPartitionId::new());
        assert_ne!(a, b, "each create mints a fresh id");
        assert_ne!(a, DEFAULT_BROWSER_CONTEXT_ID);
        assert_ne!(b, DEFAULT_BROWSER_CONTEXT_ID);
        assert!(a.starts_with("bao-context-"));
        assert!(reg.context_exists(&a));
        assert!(reg.context_exists(&b));
    }

    // @trace TEST-CDS-009 [req:REQ-CDS-009] [criterion:1] [level:unit]
    #[test]
    fn list_contexts_returns_explicit_only_sorted() {
        let mut reg = BrowserContextRegistry::new();
        assert!(
            reg.list_contexts().is_empty(),
            "default is implicit, never listed"
        );
        let a = reg.create_context(ContextPartitionId::new());
        let b = reg.create_context(ContextPartitionId::new());
        assert_eq!(reg.list_contexts(), vec![a, b]); // mint order == sorted order
    }

    // @trace TEST-CDS-009 [req:REQ-CDS-009] [criterion:2] [level:unit]
    #[test]
    fn dispose_context_returns_member_pages_and_forgets_them() {
        let mut reg = BrowserContextRegistry::new();
        let ctx = reg.create_context(ContextPartitionId::new());
        reg.attribute_page(3, Some(&ctx));
        reg.attribute_page(4, Some(&ctx));
        reg.attribute_page(5, None); // default-context page — untouched

        let orphaned = reg.dispose_context(&ctx).unwrap();
        assert_eq!(orphaned, vec![3, 4], "member pages handed to the closer");
        assert!(!reg.context_exists(&ctx));
        assert_eq!(reg.context_of_page(3), DEFAULT_BROWSER_CONTEXT_ID);
        assert_eq!(reg.context_of_page(4), DEFAULT_BROWSER_CONTEXT_ID);
        assert_eq!(reg.context_of_page(5), DEFAULT_BROWSER_CONTEXT_ID);
    }

    // @trace TEST-CDS-009 [req:REQ-CDS-009] [criterion:2] [level:unit]
    #[test]
    fn dispose_unknown_or_default_context_fails_closed() {
        let mut reg = BrowserContextRegistry::new();
        let err = reg.dispose_context("bao-context-99").unwrap_err();
        assert_eq!(err, "Failed to find browser context: bao-context-99");
        // The implicit default is not a registry entry — same verdict as
        // Chrome (it never lists the default either).
        let err = reg.dispose_context(DEFAULT_BROWSER_CONTEXT_ID).unwrap_err();
        assert!(err.contains("Failed to find browser context"));
    }

    // @trace TEST-CDS-009 [req:REQ-CDS-009] [criterion:5] [level:unit]
    #[test]
    fn context_of_page_defaults_when_unattributed() {
        let reg = BrowserContextRegistry::new();
        assert_eq!(reg.context_of_page(1), DEFAULT_BROWSER_CONTEXT_ID);
        assert_eq!(reg.context_of_page(42), DEFAULT_BROWSER_CONTEXT_ID);
    }

    // @trace TEST-CDS-009 [req:REQ-CDS-009] [criterion:5] [level:unit]
    #[test]
    fn attribute_page_records_only_known_explicit_contexts() {
        let mut reg = BrowserContextRegistry::new();
        let ctx = reg.create_context(ContextPartitionId::new());
        reg.attribute_page(7, Some(&ctx));
        assert_eq!(reg.context_of_page(7), ctx);
        // Unknown context: silently unrecorded (create_page validates before
        // building, so this shape only arrives from stale callers).
        reg.attribute_page(8, Some("bao-context-404"));
        assert_eq!(reg.context_of_page(8), DEFAULT_BROWSER_CONTEXT_ID);
        // The default id: implicit — never recorded.
        reg.attribute_page(9, Some(DEFAULT_BROWSER_CONTEXT_ID));
        assert_eq!(reg.context_of_page(9), DEFAULT_BROWSER_CONTEXT_ID);
        // None: explicit default attribution.
        reg.attribute_page(10, None);
        assert_eq!(reg.context_of_page(10), DEFAULT_BROWSER_CONTEXT_ID);
    }

    // @trace TEST-CDS-009 [req:REQ-CDS-009] [criterion:5] [level:unit]
    #[test]
    fn remove_page_drops_attribution_idempotently() {
        let mut reg = BrowserContextRegistry::new();
        let ctx = reg.create_context(ContextPartitionId::new());
        reg.attribute_page(2, Some(&ctx));
        reg.remove_page(2);
        reg.remove_page(2);
        assert_eq!(reg.context_of_page(2), DEFAULT_BROWSER_CONTEXT_ID);
    }
}
