//! Process-wide shared DNS cache (BAO fusion: three resolution paths → one
//! per-host cache).
//!
//! Before this module, the three DNS paths in bao each resolved independently:
//!
//! | path | resolver |
//! |------|----------|
//! | servo/hyper (`ServoHttpConnector`) | hyper-util `GaiResolver` (blocking `getaddrinfo`) |
//! | usockets (`us_socket_group_connect` → `Bun__addrinfo_get`) | per-request resolution |
//! | `node:dns` (`resolve_hostname_libc`) | libc `getaddrinfo` |
//!
//! Same-host lookups inside a TTL window therefore hit the system resolver
//! once per path instead of once per process — a performance and a
//! fingerprinting cost (a real browser resolves through one resolver).
//!
//! This module is the single shared per-host cache all paths consult. It is
//! deliberately dependency-light (std only + `bun_core` env knob) so the
//! low-tier `bun_dns` crate can host it and every consumer — `bao_uloop`
//! (usockets seam), servo's net crate, `bao_runtime` — can depend on it.
//!
//! # TTL policy (honesty notes)
//!
//! POSIX `getaddrinfo` returns **no TTL** (see `GetAddrInfoResult::ttl = 0`
//! with the "no TTL in POSIX getaddrinfo()" note in this crate). For entries
//! produced without a TTL source we use the engine's existing cap —
//! `BUN_CONFIG_DNS_TIME_TO_LIVE_SECONDS`, default 30 s — which is exactly the
//! lifetime upstream Bun applies to its `getaddrinfo` cache
//! (`Resolver.getMaxDNSTimeToLiveSeconds`, src/runtime/dns_jsc/dns.zig). A
//! caller that *did* receive a real TTL (c-ares record TTLs) may pass it; it
//! is clamped to the same cap so one knob bounds worst-case staleness.
//!
//! Addresses are stored as a hand-rolled [`IpAddr`] (byte arrays) instead of
//! `std::net::IpAddr` or `bun_sys::net::Address` so this module stays neutral
//! between consumers that speak `std::net` (servo connector) and consumers
//! that speak `sockaddr` (usockets C ABI).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use bun_core::env_var::BUN_CONFIG_DNS_TIME_TO_LIVE_SECONDS;

/// Maximum number of hosts kept in the cache. Upstream Bun's
/// `GlobalCache.MAX_ENTRIES` is 256; matched here.
const MAX_ENTRIES: usize = 256;

/// Family-neutral IP address (raw bytes). See module docs for why this is not
/// `std::net::IpAddr`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IpAddr {
    V4([u8; 4]),
    V6([u8; 16]),
}

struct Entry {
    addrs: Vec<IpAddr>,
    /// Instant after which the entry is stale and lookups miss.
    expires_at: Instant,
    /// LRU clock: updated on hit; eviction picks the minimum.
    last_used: Instant,
}

#[derive(Default)]
struct Inner {
    map: HashMap<Box<str>, Entry>,
}

static CACHE: OnceLock<Mutex<Inner>> = OnceLock::new();

fn cache() -> &'static Mutex<Inner> {
    CACHE.get_or_init(|| Mutex::new(Inner::default()))
}

/// Upper bound for any cached entry's lifetime, from
/// `BUN_CONFIG_DNS_TIME_TO_LIVE_SECONDS` (default 30 s, matching upstream
/// Bun's DNS cache cap). Setting the variable to 0 disables caching.
pub fn max_ttl() -> Duration {
    Duration::from_secs(
        BUN_CONFIG_DNS_TIME_TO_LIVE_SECONDS::get()
            .unwrap_or(30)
            .max(0) as u64,
    )
}

/// Normalize a host as a cache key: DNS names are case-insensitive
/// (RFC 1035 §2.3.3), and callers arrive with URL-derived hosts of mixed case.
fn key(host: &[u8]) -> Box<str> {
    // ASCII-only lowercase: IDN A-labels Punycode (xn--) are ASCII already.
    String::from_utf8_lossy(&host.to_ascii_lowercase()).into()
}

/// Look up a host. Returns `None` on miss or if the entry expired (expired
/// entries are removed eagerly).
pub fn lookup(host: &[u8]) -> Option<Vec<IpAddr>> {
    lookup_at(host, Instant::now())
}

fn lookup_at(host: &[u8], now: Instant) -> Option<Vec<IpAddr>> {
    if host.is_empty() {
        return None;
    }
    let k = key(host);
    let mut inner = cache().lock().unwrap();
    let entry = inner.map.get_mut(&k)?;
    if now >= entry.expires_at {
        inner.map.remove(&k);
        return None;
    }
    entry.last_used = now;
    Some(entry.addrs.clone())
}

/// Insert a resolved address list. `reported_ttl` is the resolver-reported
/// TTL in seconds when the source provides one (c-ares record TTLs); `None`
/// means the source had no TTL (POSIX `getaddrinfo`). Both are clamped to
/// [`max_ttl`]. Empty hosts/address lists are ignored.
pub fn insert(host: &[u8], addrs: Vec<IpAddr>, reported_ttl: Option<u64>) {
    insert_at(host, addrs, reported_ttl, Instant::now())
}

fn insert_at(host: &[u8], addrs: Vec<IpAddr>, reported_ttl: Option<u64>, now: Instant) {
    if host.is_empty() || addrs.is_empty() {
        return;
    }
    let ttl = match reported_ttl {
        Some(secs) => Duration::from_secs(secs).min(max_ttl()),
        None => max_ttl(),
    };
    if ttl.is_zero() {
        return; // caching disabled via env knob
    }
    let k = key(host);
    let mut inner = cache().lock().unwrap();
    if inner.map.len() >= MAX_ENTRIES && !inner.map.contains_key(&k) {
        // First drop everything already expired, then evict LRU until there
        // is room. 256 entries → the O(n) scans are trivial.
        inner.map.retain(|_, e| now < e.expires_at);
        while inner.map.len() >= MAX_ENTRIES {
            let victim = inner
                .map
                .iter()
                .min_by_key(|(_, e)| e.last_used)
                .map(|(k, _)| k.clone());
            match victim {
                Some(v) => {
                    inner.map.remove(&v);
                }
                None => break,
            }
        }
    }
    inner.map.insert(
        k,
        Entry {
            addrs,
            expires_at: now + ttl,
            last_used: now,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cache is process-global and cargo runs module tests in parallel;
    /// the LRU flood test would otherwise evict entries other tests are
    /// actively asserting on. Serialize every test through this lock, and
    /// recover from poisoning so one failed assertion doesn't cascade.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn lock_tests() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn v4(octets: [u8; 4]) -> IpAddr {
        IpAddr::V4(octets)
    }

    #[test]
    fn hit_and_miss() {
        let _guard = lock_tests();
        let now = Instant::now();
        assert!(lookup_at(b"hit.test", now).is_none());
        insert_at(b"hit.test", vec![v4([1, 2, 3, 4])], None, now);
        let hit = lookup_at(b"HIT.TEST", now).expect("case-insensitive hit");
        assert_eq!(hit, vec![v4([1, 2, 3, 4])]);
    }

    #[test]
    fn empty_host_or_addrs_not_inserted() {
        let _guard = lock_tests();
        let now = Instant::now();
        insert_at(b"", vec![v4([1, 1, 1, 1])], None, now);
        insert_at(b"empty.test", vec![], None, now);
        assert!(lookup_at(b"empty.test", now).is_none());
    }

    #[test]
    fn reported_ttl_expires_entry() {
        let _guard = lock_tests();
        let now = Instant::now();
        insert_at(b"ttl.test", vec![v4([1, 2, 3, 4])], Some(1), now);
        assert!(lookup_at(b"ttl.test", now).is_some());
        // 2 s later a 1 s TTL is stale.
        assert!(lookup_at(b"ttl.test", now + Duration::from_secs(2)).is_none());
        // Expired entry was removed, not just skipped.
        let inner = cache().lock().unwrap();
        assert!(inner.map.get(&key(b"ttl.test")).is_none());
    }

    #[test]
    fn ttl_is_clamped_to_cap() {
        let _guard = lock_tests();
        let now = Instant::now();
        insert_at(b"clamp.test", vec![v4([1, 2, 3, 4])], Some(3600), now);
        let cap = max_ttl();
        // Inside the engine cap the entry survives even though the reported
        // TTL claimed an hour…
        assert!(lookup_at(b"clamp.test", now + cap - Duration::from_secs(1)).is_some());
        // …and beyond the cap it is gone (this lookup also eagerly removes
        // it, hence the ordering).
        assert!(lookup_at(b"clamp.test", now + cap + Duration::from_secs(1)).is_none());
    }

    #[test]
    fn lru_eviction_under_capacity() {
        let _guard = lock_tests();
        let now = Instant::now();
        let first = b"first.lru-test.test";
        insert_at(first, vec![v4([9, 9, 9, 9])], None, now);
        for i in 0..MAX_ENTRIES {
            let host = format!("host{i}.lru-test.test");
            // Distinct, increasing last_used timestamps.
            insert_at(
                host.as_bytes(),
                vec![v4([10, 0, 0, i as u8])],
                None,
                now + Duration::from_millis(i as u64 + 1),
            );
        }
        let inner = cache().lock().unwrap();
        assert_eq!(inner.map.len(), MAX_ENTRIES);
        // `first` has the oldest last_used → evicted to make room.
        assert!(inner.map.get(&key(first)).is_none());
        assert!(inner.map.get(&key(b"host255.lru-test.test")).is_some());
    }
}
