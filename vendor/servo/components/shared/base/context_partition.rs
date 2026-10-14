/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! BAO PATCH (REQ-CDS-009 C6): the webview → context-partition registry.
//!
//! One entry per live page created inside an explicit browser context; the
//! entry is authoritative for every resource-plane face that consults it:
//!
//! - the net router resolves a message's partition (cookie faces via the
//!   [`net_traits::CoreResourceMsg::Partition`] envelope stamp, fetch faces
//!   via `RequestBuilder::target_webview_id`) and serves it from that
//!   partition's [`HttpState`](crate::..) — its own cookie jar, HTTP cache,
//!   HSTS list and auth cache;
//! - the constellation resolves a webview's storage group at event-loop
//!   spawn, so its webstorage / IndexedDB / CacheStorage face lands in the
//!   context's own thread group.
//!
//! Same lifecycle discipline as the per-WebViewId stealth registries
//! (R53-A): the embedder writes the entry the moment the WebView exists
//! (before the initial navigation can be dispatched) and drops it on page
//! close. Webview ids are not reused within a process, so a missed entry
//! can only mean "page was never attributed to an explicit context" — the
//! pre-C6 behavior — never a stale alias of another context.

use std::collections::HashMap;
use std::sync::LazyLock;

use parking_lot::RwLock;

use crate::id::{ContextPartitionId, WebViewId};

static PARTITION_BY_WEBVIEW: LazyLock<RwLock<HashMap<WebViewId, ContextPartitionId>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// Attribute one webview to a context partition. Called at WebView creation,
/// before the initial navigation is dispatched (so the page's very first
/// fetch and cookie write already land in its partition).
pub fn set_context_partition_for_webview(webview_id: WebViewId, partition: ContextPartitionId) {
    PARTITION_BY_WEBVIEW
        .write()
        .insert(webview_id, partition);
}

/// Drop a webview's attribution (page close; idempotent).
pub fn clear_context_partition_for_webview(webview_id: WebViewId) {
    PARTITION_BY_WEBVIEW.write().remove(&webview_id);
}

/// The partition a webview belongs to. `None` = the page was never attributed
/// to an explicit browser context — every consumer falls back to the
/// channel-implied default (public / private), which is exactly the pre-C6
/// behavior for un-partitioned pages.
pub fn context_partition_for_webview(webview_id: WebViewId) -> Option<ContextPartitionId> {
    PARTITION_BY_WEBVIEW.read().get(&webview_id).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    // @trace TEST-CDS-009 [req:REQ-CDS-009] [criterion:6] [level:unit]
    #[test]
    fn set_resolve_clear_roundtrip() {
        let webview = WebViewId::mock_for_testing(crate::id::TEST_BROWSING_CONTEXT_ID);
        assert!(context_partition_for_webview(webview).is_none());

        let partition = ContextPartitionId::new();
        set_context_partition_for_webview(webview, partition);
        assert_eq!(context_partition_for_webview(webview), Some(partition));

        clear_context_partition_for_webview(webview);
        clear_context_partition_for_webview(webview); // idempotent
        assert!(context_partition_for_webview(webview).is_none());
    }

    // @trace TEST-CDS-009 [req:REQ-CDS-009] [criterion:6] [level:unit]
    #[test]
    fn partition_ids_are_unique_and_upsert_replaces() {
        let webview = WebViewId::mock_for_testing(crate::id::TEST_BROWSING_CONTEXT_ID);
        let a = ContextPartitionId::new();
        let b = ContextPartitionId::new();
        assert_ne!(a, b, "ids are never reused within a process");

        set_context_partition_for_webview(webview, a);
        set_context_partition_for_webview(webview, b); // re-attribution wins
        assert_eq!(context_partition_for_webview(webview), Some(b));
        clear_context_partition_for_webview(webview);
    }
}
