//! In-memory cache for the served Codex catalog.
//!
//! Two reasons this exists:
//!
//! - **Resilience.** A catalog supplied through `model_catalog_url` replaces the
//!   client's bundled catalog, so an upstream failure would leave the client with
//!   no models at all. Serving the last known good body keeps a transient
//!   upstream problem invisible to the client.
//! - **Cost.** The body is a few hundred kilobytes and the client refreshes it on
//!   a five minute cycle from every running client. Serving a stored copy avoids
//!   repeating the upstream round trip.
//!
//! The body is held **in memory only**. It contains upstream instruction text,
//! and TT does not persist prompts to disk, so a restart starts cold. That means
//! a cold start against a failing upstream is the one remaining case where the
//! client can end up with no catalog.
//!
//! The cache stores what TT serves, not the raw upstream document, so a hit is a
//! byte copy rather than a re-decode and re-filter.

use bytes::Bytes;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a stored catalog is served without asking upstream again.
///
/// Shorter than the client's own five minute cycle so a client never carries a
/// stale view much longer than TT does.
pub const CATALOG_CACHE_TTL: Duration = Duration::from_secs(120);

/// Cache key for one account's catalog as served to one client version.
///
/// The upstream tailors the catalog to the requested `client_version`, so a body
/// fetched for one version must never be served to a client on another. Keying
/// on the account alone let a single request without a usable version downgrade
/// every client that followed.
pub fn cache_key(account_id: &str, client_version: &str) -> String {
    format!("{account_id}\u{1f}{client_version}")
}

/// A stored catalog body and how long ago it was fetched.
#[derive(Debug, Clone)]
pub struct CachedCatalog {
    pub body: Bytes,
    pub age: Duration,
}

#[derive(Debug)]
struct Entry {
    body: Bytes,
    etag: Option<String>,
    fetched_at: Instant,
}

/// Per-account catalog cache.
#[derive(Debug)]
pub struct CatalogCache {
    ttl: Duration,
    entries: Mutex<HashMap<String, Entry>>,
}

impl Default for CatalogCache {
    fn default() -> Self {
        Self::new(CATALOG_CACHE_TTL)
    }
}

impl CatalogCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// The stored entry for `key`, whatever its age.
    ///
    /// Used to serve a slightly stale catalog rather than no catalog.
    pub fn get(&self, key: &str) -> Option<CachedCatalog> {
        self.with_entries(|entries| entries.get(key).map(CachedCatalog::from))
    }

    /// The stored entry for `key` if it is still within the TTL.
    pub fn fresh(&self, key: &str) -> Option<CachedCatalog> {
        let ttl = self.ttl;
        self.with_entries(|entries| {
            entries
                .get(key)
                .filter(|entry| entry.fetched_at.elapsed() < ttl)
                .map(CachedCatalog::from)
        })
    }

    /// The stored ETag for `key`, used for a conditional upstream request.
    pub fn etag(&self, key: &str) -> Option<String> {
        self.with_entries(|entries| entries.get(key).and_then(|entry| entry.etag.clone()))
    }

    /// Store a freshly fetched body, replacing any previous entry.
    pub fn store(&self, key: &str, body: Bytes, etag: Option<String>) {
        self.with_entries(|entries| {
            let etag = etag.or_else(|| entries.get(key).and_then(|entry| entry.etag.clone()));
            entries.insert(
                key.to_string(),
                Entry {
                    body,
                    etag,
                    fetched_at: Instant::now(),
                },
            );
        });
    }

    /// Refresh a `304` revalidation so the entry counts as fresh again.
    pub fn touch(&self, key: &str) {
        self.with_entries(|entries| {
            if let Some(entry) = entries.get_mut(key) {
                entry.fetched_at = Instant::now();
            }
        });
    }

    /// A panicked holder must not take the relay down with it: the data is a
    /// disposable copy of upstream state.
    fn with_entries<T>(&self, read: impl FnOnce(&mut HashMap<String, Entry>) -> T) -> T {
        let mut guard = self
            .entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        read(&mut guard)
    }
}

impl From<&Entry> for CachedCatalog {
    fn from(entry: &Entry) -> Self {
        Self {
            body: entry.body.clone(),
            age: entry.fetched_at.elapsed(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn body(text: &str) -> Bytes {
        Bytes::copy_from_slice(text.as_bytes())
    }

    #[test]
    fn an_unknown_key_has_nothing_to_serve() {
        let cache = CatalogCache::default();
        assert!(cache.get("account-a").is_none());
        assert!(cache.fresh("account-a").is_none());
        assert!(cache.etag("account-a").is_none());
    }

    #[test]
    fn a_stored_entry_is_fresh_and_carries_its_etag() {
        let cache = CatalogCache::default();
        cache.store("account-a", body("first"), Some("W/\"1\"".to_string()));

        assert_eq!(
            cache.fresh("account-a").map(|cached| cached.body),
            Some(body("first"))
        );
        assert_eq!(cache.etag("account-a").as_deref(), Some("W/\"1\""));
    }

    #[test]
    fn an_expired_entry_is_still_servable_but_not_fresh() {
        let cache = CatalogCache::new(Duration::ZERO);
        cache.store("account-a", body("first"), None);

        assert!(cache.fresh("account-a").is_none());
        assert_eq!(
            cache.get("account-a").map(|cached| cached.body),
            Some(body("first")),
            "a stale body is better than no body"
        );
    }

    #[test]
    fn keys_separate_client_versions_and_accounts() {
        let a_v1 = cache_key("account-a", "0.162.0");
        let a_v2 = cache_key("account-a", "0.163.0");
        let b_v1 = cache_key("account-b", "0.162.0");
        assert_ne!(
            a_v1, a_v2,
            "one client's version must not see another's body"
        );
        assert_ne!(a_v1, b_v1);
        assert_eq!(a_v1, cache_key("account-a", "0.162.0"));
    }

    #[test]
    fn accounts_do_not_share_entries() {
        let cache = CatalogCache::default();
        cache.store("account-a", body("a"), None);
        cache.store("account-b", body("b"), None);

        assert_eq!(
            cache.get("account-a").map(|cached| cached.body),
            Some(body("a"))
        );
        assert_eq!(
            cache.get("account-b").map(|cached| cached.body),
            Some(body("b"))
        );
    }

    #[test]
    fn revalidating_makes_an_expired_entry_fresh_again() {
        let cache = CatalogCache::new(Duration::ZERO);
        cache.store("account-a", body("first"), Some("W/\"1\"".to_string()));
        assert!(cache.fresh("account-a").is_none());

        cache.touch("account-a");
        // The TTL is zero, so freshness is asserted through the body instead of
        // the clock: touching must not lose the stored payload.
        assert_eq!(
            cache.get("account-a").map(|cached| cached.body),
            Some(body("first"))
        );
    }

    #[test]
    fn storing_without_an_etag_keeps_the_previous_one() {
        let cache = CatalogCache::default();
        cache.store("account-a", body("first"), Some("W/\"1\"".to_string()));
        cache.store("account-a", body("second"), None);

        assert_eq!(cache.etag("account-a").as_deref(), Some("W/\"1\""));
        assert_eq!(
            cache.get("account-a").map(|cached| cached.body),
            Some(body("second"))
        );
    }

    #[test]
    fn a_poisoned_lock_still_serves() {
        let cache = Arc::new(CatalogCache::default());
        cache.store("account-a", body("first"), None);

        let poisoner = Arc::clone(&cache);
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.entries.lock().expect("lock entries");
            panic!("poison the cache lock");
        })
        .join();

        assert_eq!(
            cache.get("account-a").map(|cached| cached.body),
            Some(body("first"))
        );
    }
}
