/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL is not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! The DBSC session store and key store, hung off `HttpState`.
//!
//! BAO PATCH (REQ-BRW-051): per the draft, the session store is keyed by
//! registrable domain, then session identifier; sessions share the cookie
//! jar's partition (same `HttpState`), so browser-context partitioning
//! (REQ-CDS-009 C6) partitions DBSC sessions identically — no separate
//! partition key is introduced. Keys are stored alongside their sessions so
//! a cleared store drops both (the draft's "clearing cookies/site data must
//! clear sessions and keys" privacy constraint is honored by construction:
//! `clear_for_url` is invoked wherever site data is cleared for the origin).

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use parking_lot::RwLock;
use rustc_hash::FxHashMap;
use servo_url::ServoUrl;

use crate::dbsc::keys::DbscKeyPair;
use crate::dbsc::model::{DbscSession, same_registrable_domain};

/// A session plus its proof key, as the store keeps them together.
pub(crate) struct StoredSession {
    pub(crate) session: DbscSession,
    pub(crate) key: DbscKeyPair,
    /// Monotonic registration order, used to give sessions a stable
    /// iteration order (store iteration drives multi-session refresh).
    pub(crate) registration_order: u64,
}

/// Per-refresh backoff state after a 5xx (the draft: keep the session, retry
/// with backoff). A session in backoff does not re-enter refresh until the
/// deadline passes.
#[derive(Clone, Copy, Debug)]
struct BackoffState {
    failures: u32,
    until: SystemTime,
    /// The `Secure-Session-Skipped` token of the failure that opened the
    /// backoff window (`unreachable` / `server_error`). Requests that are
    /// skipped while the window is open report the same token — the reason
    /// the refresh did not run is still that failure.
    last_token: &'static str,
}

/// The process store for one cookie partition's DBSC sessions.
#[derive(Default)]
pub(crate) struct DbscStore {
    /// registrable domain → session identifier → session.
    sessions: RwLock<FxHashMap<String, HashMap<String, StoredSession>>>,
    /// registrable domain → backoff state after refresh failures.
    backoff: RwLock<FxHashMap<String, BackoffState>>,
    registration_counter: RwLock<u64>,
}

/// The base backoff after a 5xx refresh failure; doubles per consecutive
/// failure, capped at 32 minutes.
const BASE_BACKOFF: Duration = Duration::from_secs(30);
const MAX_BACKOFF: Duration = Duration::from_secs(32 * 60);

/// Hard cap on stored sessions per registrable domain (the draft's quota
/// guidance; excess registrations are rejected fail-closed).
const MAX_SESSIONS_PER_DOMAIN: usize = 16;

impl DbscStore {
    pub(crate) fn new() -> DbscStore {
        DbscStore::default()
    }

    fn domain_key(url: &ServoUrl) -> String {
        net_traits::pub_domains::reg_suffix(url.host_str().unwrap_or("")).to_lowercase()
    }

    /// Register a new session (registration completed: instructions
    /// validated, key generated). Returns false when the quota is exhausted
    /// or an identical session identifier already exists (the refresh URL
    /// ownership check happens in the caller).
    pub(crate) fn insert_session(
        &self,
        session: DbscSession,
        key: DbscKeyPair,
    ) -> bool {
        let domain = Self::domain_key(&session.refresh_url);
        let mut sessions = self.sessions.write();
        let domain_sessions = sessions.entry(domain.clone()).or_default();
        if domain_sessions.len() >= MAX_SESSIONS_PER_DOMAIN ||
            domain_sessions.contains_key(&session.session_identifier)
        {
            return false;
        }
        let mut counter = self.registration_counter.write();
        *counter += 1;
        domain_sessions.insert(
            session.session_identifier.clone(),
            StoredSession {
                session,
                key,
                registration_order: *counter,
            },
        );
        // A successful registration clears any backoff for the domain.
        self.backoff.write().remove(&domain);
        true
    }

    /// Replace a session's configuration after a successful refresh. The
    /// session identifier is the store key and must not change (the draft:
    /// a refresh response cannot re-key a session — see WPT
    /// refresh-replaces-config "Refresh cannot replace session identifier");
    /// a mismatched identifier fails the update and the caller terminates.
    pub(crate) fn replace_session(&self, session: DbscSession) -> bool {
        let domain = Self::domain_key(&session.refresh_url);
        let mut sessions = self.sessions.write();
        let Some(domain_sessions) = sessions.get_mut(&domain) else {
            return false;
        };
        let Some(stored) = domain_sessions.get_mut(&session.session_identifier) else {
            return false;
        };
        stored.session = session;
        true
    }

    /// Update only the cached challenge of a session.
    pub(crate) fn set_cached_challenge(
        &self,
        refresh_url: &ServoUrl,
        session_identifier: &str,
        challenge: Option<String>,
    ) {
        let domain = Self::domain_key(refresh_url);
        let mut sessions = self.sessions.write();
        if let Some(stored) = sessions
            .get_mut(&domain)
            .and_then(|map| map.get_mut(session_identifier))
        {
            stored.session.cached_challenge = challenge;
        }
    }

    /// Whether the domain is in refresh backoff right now.
    pub(crate) fn in_backoff(&self, refresh_url: &ServoUrl) -> bool {
        let domain = Self::domain_key(refresh_url);
        self.backoff
            .read()
            .get(&domain)
            .is_some_and(|state| SystemTime::now() < state.until)
    }

    /// Record a refresh failure for the domain (backoff grows). `token` is
    /// the draft §9.5 reason token that later skips report.
    pub(crate) fn record_refresh_failure(&self, refresh_url: &ServoUrl, token: &'static str) {
        let domain = Self::domain_key(refresh_url);
        let mut backoff = self.backoff.write();
        let state = backoff.entry(domain).or_insert(BackoffState {
            failures: 0,
            until: SystemTime::now(),
            last_token: token,
        });
        state.failures = state.failures.saturating_add(1);
        state.last_token = token;
        let shift = (state.failures - 1).min(6);
        state.until = SystemTime::now() + BASE_BACKOFF * (1u32 << shift);
    }

    /// When the domain is in refresh backoff, the reason token of the
    /// failure that opened the window (used for the
    /// `Secure-Session-Skipped` debug header of skipped sessions).
    pub(crate) fn backoff_token(&self, url: &ServoUrl) -> Option<&'static str> {
        let domain = Self::domain_key(url);
        self.backoff
            .read()
            .get(&domain)
            .filter(|state| SystemTime::now() < state.until)
            .map(|state| state.last_token)
    }

    /// Record a successful refresh (clears backoff).
    pub(crate) fn record_refresh_success(&self, refresh_url: &ServoUrl) {
        let domain = Self::domain_key(refresh_url);
        self.backoff.write().remove(&domain);
    }

    /// All sessions whose refresh URL shares the given URL's registrable
    /// domain, in registration order. (Lookup is by the request URL's site,
    /// per the draft's store layout.)
    pub(crate) fn sessions_for_url(&self, url: &ServoUrl) -> Vec<String> {
        let domain = Self::domain_key(url);
        let sessions = self.sessions.read();
        let Some(domain_sessions) = sessions.get(&domain) else {
            return Vec::new();
        };
        let mut ordered: Vec<&StoredSession> = domain_sessions.values().collect();
        ordered.sort_by_key(|stored| stored.registration_order);
        ordered
            .into_iter()
            .map(|stored| stored.session.session_identifier.clone())
            .collect()
    }

    /// Run `visitor` over the session with the given identifier (same
    /// registrable domain as `url`), if present. Returns the visitor's
    /// result. This is the single serialized access point so callers never
    /// clone key material across locks.
    pub(crate) fn with_session<R>(
        &self,
        url: &ServoUrl,
        session_identifier: &str,
        visitor: impl FnOnce(&StoredSession) -> R,
    ) -> Option<R> {
        let domain = Self::domain_key(url);
        let sessions = self.sessions.read();
        sessions
            .get(&domain)
            .and_then(|map| map.get(session_identifier))
            .map(visitor)
    }

    /// Run `visitor` mutably over the session with the given identifier.
    pub(crate) fn with_session_mut<R>(
        &self,
        url: &ServoUrl,
        session_identifier: &str,
        visitor: impl FnOnce(&mut StoredSession) -> R,
    ) -> Option<R> {
        let domain = Self::domain_key(url);
        let mut sessions = self.sessions.write();
        sessions
            .get_mut(&domain)
            .and_then(|map| map.get_mut(session_identifier))
            .map(visitor)
    }

    /// Remove one session (termination: `continue: false`, refresh 4xx, or
    /// explicit Clear-Site-Data handling).
    pub(crate) fn remove_session(&self, refresh_url: &ServoUrl, session_identifier: &str) {
        let domain = Self::domain_key(refresh_url);
        let mut sessions = self.sessions.write();
        if let Some(domain_sessions) = sessions.get_mut(&domain) {
            domain_sessions.remove(session_identifier);
            if domain_sessions.is_empty() {
                sessions.remove(&domain);
            }
        }
    }

    /// Remove all sessions of the site (registrable domain) of `url` — the
    /// Clear-Site-Data site-wide face.
    pub(crate) fn clear_for_site(&self, url: &ServoUrl) {
        let domain = Self::domain_key(url);
        self.sessions.write().remove(&domain);
        self.backoff.write().remove(&domain);
    }

    /// Remove sessions of the exact origin of `url` — the Clear-Site-Data
    /// origin face. Sessions of the same site but a different origin stay.
    pub(crate) fn clear_for_origin(&self, url: &ServoUrl) {
        let domain = Self::domain_key(url);
        let origin = url.origin();
        let mut sessions = self.sessions.write();
        if let Some(domain_sessions) = sessions.get_mut(&domain) {
            domain_sessions.retain(|_, stored| stored.session.refresh_url.origin() != origin);
            if domain_sessions.is_empty() {
                sessions.remove(&domain);
            }
        }
    }

    /// True when any session of `url`'s site is registered (fast pre-check
    /// used by the request path to skip store work entirely).
    pub(crate) fn has_sessions_for_site(&self, url: &ServoUrl) -> bool {
        let domain = Self::domain_key(url);
        self.sessions
            .read()
            .get(&domain)
            .is_some_and(|map| !map.is_empty())
    }
}

/// Check whether two URLs share a registrable domain (store-internal helper
/// mirroring the model's scope check).
pub(crate) fn urls_share_site(a: &ServoUrl, b: &ServoUrl) -> bool {
    same_registrable_domain(a.host_str(), b.host_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dbsc::model::{
        ScopeSpecificationItem, SessionCredential, SessionScope,
    };

    fn test_session(identifier: &str, refresh_url: &str) -> DbscSession {
        let refresh_url = ServoUrl::parse(refresh_url).unwrap();
        DbscSession {
            session_identifier: identifier.to_owned(),
            algorithm: crate::dbsc::model::DbscAlgorithm::Rs256,
            scope_origin: refresh_url.origin(),
            refresh_url,
            scope: SessionScope::Origin,
            include_site: false,
            scope_specification: vec![ScopeSpecificationItem {
                include: true,
                domain: "*".to_owned(),
                path: "/".to_owned(),
            }],
            credentials: vec![SessionCredential {
                cookie_name: "auth_cookie".to_owned(),
                domain: String::new(),
                path: String::new(),
                secure: false,
                http_only: false,
                same_site: crate::dbsc::model::SameSiteSpec::Lax,
                partitioned: false,
            }],
            allowed_refresh_initiators: vec!["*".to_owned()],
            cached_challenge: None,
        }
    }

    fn test_key() -> DbscKeyPair {
        DbscKeyPair::generate(crate::dbsc::model::DbscAlgorithm::Rs256).unwrap()
    }

    #[test]
    fn insert_lookup_replace_remove() {
        let store = DbscStore::new();
        let url = ServoUrl::parse("https://web-platform.test/device-bound-session-credentials/refresh_session.py").unwrap();
        assert!(!store.has_sessions_for_site(&url));
        assert!(store.insert_session(test_session("0", url.as_str()), test_key()));
        assert!(store.has_sessions_for_site(&url));
        assert_eq!(store.sessions_for_url(&url), vec!["0".to_owned()]);

        // Duplicate identifier rejected.
        assert!(!store.insert_session(test_session("0", url.as_str()), test_key()));

        // Replace keeps config keyed by identifier; a changed identifier
        // fails the replace.
        let mut replaced = test_session("0", url.as_str());
        replaced.cached_challenge = Some("new".to_owned());
        assert!(store.replace_session(replaced));
        let mut mismatched = test_session("1", url.as_str());
        mismatched.session_identifier = "9".to_owned();
        assert!(!store.replace_session(mismatched));
        store.with_session(&url, "0", |stored| {
            assert_eq!(stored.session.cached_challenge.as_deref(), Some("new"));
        })
        .unwrap();

        store.remove_session(&url, "0");
        assert!(!store.has_sessions_for_site(&url));
    }

    #[test]
    fn clear_site_vs_origin() {
        let store = DbscStore::new();
        let a = ServoUrl::parse("https://a.web-platform.test/refresh.py").unwrap();
        let b = ServoUrl::parse("https://b.web-platform.test/refresh.py").unwrap();
        store.insert_session(test_session("a", a.as_str()), test_key());
        store.insert_session(test_session("b", b.as_str()), test_key());

        // Origin face removes only that origin's sessions.
        store.clear_for_origin(&a);
        assert!(store.with_session(&b, "b", |_| ()).is_some());
        assert!(store.with_session(&a, "a", |_| ()).is_none());

        store.insert_session(test_session("a", a.as_str()), test_key());
        // Site face removes both (same registrable domain).
        store.clear_for_site(&a);
        assert!(!store.has_sessions_for_site(&b));
    }

    #[test]
    fn backoff_grows_and_success_clears() {
        let store = DbscStore::new();
        let url = ServoUrl::parse("https://web-platform.test/refresh.py").unwrap();
        assert!(!store.in_backoff(&url));
        assert_eq!(store.backoff_token(&url), None);
        store.record_refresh_failure(&url, "server_error");
        // The backoff window reports the failure's reason token, so a
        // request skipped during the window can still emit the debug header.
        assert_eq!(store.backoff_token(&url), Some("server_error"));
        store.record_refresh_failure(&url, "unreachable");
        assert!(store.in_backoff(&url));
        assert_eq!(store.backoff_token(&url), Some("unreachable"));
        store.record_refresh_success(&url);
        assert!(!store.in_backoff(&url));
        assert_eq!(store.backoff_token(&url), None);
    }

    #[test]
    fn quota_is_enforced() {
        let store = DbscStore::new();
        let url = ServoUrl::parse("https://web-platform.test/refresh.py").unwrap();
        for index in 0..MAX_SESSIONS_PER_DOMAIN {
            assert!(store.insert_session(
                test_session(&index.to_string(), url.as_str()),
                test_key()
            ));
        }
        assert!(!store.insert_session(test_session("overflow", url.as_str()), test_key()));
    }
}
