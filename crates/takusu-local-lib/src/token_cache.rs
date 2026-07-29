//! TTL-based cache for token verification results.
//!
//! Avoids a storage round-trip on every authenticated request. Cache key is
//! the SHA-256 hash of the token (raw tokens are never stored). Cache values
//! expire after `TAKUSU_TOKEN_CACHE_TTL_SECS` seconds (default 60).
//!
//! Invariant: a cached `Invalid` answer is also valid until TTL. To make
//! revocation effective sooner, call `invalidate` after revoking.

use std::time::Duration as StdDuration;
use web_time::{Duration, SystemTime, UNIX_EPOCH};

use moka::Expiry;
use moka::sync::Cache;
use sha2::{Digest, Sha256};
use takusu_types::{TokenClaims, jwt};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TokenState {
    Valid(TokenClaims),
    Invalid,
}

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn is_claims_expired(claims: &TokenClaims) -> bool {
    claims.exp.is_some_and(|exp| exp <= now_seconds())
}

fn entry_ttl(base: Duration, state: &TokenState) -> Duration {
    let TokenState::Valid(claims) = state else {
        return base;
    };
    let Some(exp) = claims.exp else {
        return base;
    };
    let now = now_seconds();
    if exp <= now {
        // Already expired; expire immediately.
        return Duration::from_secs(0);
    }
    let remaining = (exp - now) as u64;
    std::cmp::min(base, Duration::from_secs(remaining))
}

/// Computes a `std::time::Duration` from `web_time::Duration`.
///
/// Moka's `Expiry` trait uses `std::time::Duration`, while the rest of the
/// crate uses `web_time` for WASM portability. The two types convert 1:1.
fn to_std_duration(d: Duration) -> StdDuration {
    StdDuration::from_secs(d.as_secs()) + StdDuration::from_nanos(d.subsec_nanos() as u64)
}

/// Per-entry expiration for the token cache.
///
/// For `Valid` entries, the TTL is capped at the JWT `exp` so the cache does
/// not keep a token as valid beyond its real expiration. `Invalid` entries use
/// the configured base TTL.
struct TokenExpiry {
    base: Duration,
}

impl Expiry<String, TokenState> for TokenExpiry {
    fn expire_after_create(
        &self,
        _key: &String,
        value: &TokenState,
        _created_at: std::time::Instant,
    ) -> Option<StdDuration> {
        Some(self.ttl_for(value))
    }

    fn expire_after_update(
        &self,
        _key: &String,
        value: &TokenState,
        _updated_at: std::time::Instant,
        _current_duration: Option<StdDuration>,
    ) -> Option<StdDuration> {
        Some(self.ttl_for(value))
    }
}

impl TokenExpiry {
    fn ttl_for(&self, value: &TokenState) -> StdDuration {
        let ttl = entry_ttl(self.base, value);
        if ttl.is_zero() {
            // Moka does not treat a zero duration as "already expired", so use
            // the smallest positive duration to ensure immediate eviction.
            return StdDuration::from_nanos(1);
        }
        to_std_duration(ttl)
    }
}

#[derive(Clone)]
pub struct TokenCache {
    cache: Cache<String, TokenState>,
    ttl: Duration,
}

impl TokenCache {
    pub fn new(ttl: Duration) -> Self {
        let cache = Cache::builder()
            .expire_after(TokenExpiry { base: ttl })
            .build();
        Self { cache, ttl }
    }

    pub fn with_default_ttl() -> Self {
        let secs: u64 = std::env::var("TAKUSU_TOKEN_CACHE_TTL_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(60);
        Self::new(Duration::from_secs(secs))
    }

    pub fn get(&self, token: &str) -> Option<TokenState> {
        let hash = Self::hash(token);
        let state = self.cache.get(&hash)?;
        if let TokenState::Valid(ref claims) = state
            && is_claims_expired(claims)
        {
            let invalid = TokenState::Invalid;
            // Re-insert as Invalid so subsequent gets within the base TTL do
            // not need a storage round-trip.
            self.cache.insert(hash, invalid);
            return Some(TokenState::Invalid);
        }
        Some(state)
    }

    pub fn put(&self, token: &str, state: TokenState) {
        let hash = Self::hash(token);
        let ttl = entry_ttl(self.ttl, &state);
        if ttl.is_zero() {
            // An immediately expired entry should not be cached. Remove any
            // stale entry for this token and avoid inserting a no-op value.
            self.cache.invalidate(&hash);
            return;
        }
        self.cache.insert(hash, state);
    }

    pub fn invalidate(&self) {
        // `invalidate_all` sets an invalidation timestamp. `get` and other
        // retrieval methods are guaranteed by Moka not to return entries
        // inserted before or at that timestamp, so logically the cache is
        // cleared immediately even though physical removal happens as a
        // maintenance task.
        self.cache.invalidate_all();
    }

    fn hash(token: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        let result = hasher.finalize();
        jwt::hex(&result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_claims(scope: &str) -> TokenClaims {
        TokenClaims {
            sub: "sub".into(),
            jti: "jti".into(),
            scope: scope.into(),
            label: None,
            aud: takusu_types::DEFAULT_AUD.into(),
            iss: takusu_types::DEFAULT_ISS.into(),
            iat: 0,
            exp: None,
        }
    }

    #[test]
    fn hit_returns_state() {
        let cache = TokenCache::new(Duration::from_secs(10));
        cache.put("abc", TokenState::Valid(dummy_claims("read-write")));
        assert!(matches!(cache.get("abc"), Some(TokenState::Valid(_))));
    }

    #[test]
    fn miss_returns_none() {
        let cache = TokenCache::new(Duration::from_secs(10));
        assert_eq!(cache.get("xyz"), None);
    }

    #[test]
    fn expired_entry_returns_none() {
        let cache = TokenCache::new(Duration::from_millis(0));
        cache.put("abc", TokenState::Valid(dummy_claims("read-write")));
        std::thread::sleep(Duration::from_millis(1));
        assert_eq!(cache.get("abc"), None);
    }

    #[test]
    fn invalidate_clears_all() {
        let cache = TokenCache::new(Duration::from_secs(10));
        cache.put("a", TokenState::Valid(dummy_claims("read-write")));
        cache.put("b", TokenState::Invalid);
        cache.invalidate();
        assert_eq!(cache.get("a"), None);
        assert_eq!(cache.get("b"), None);
    }

    #[test]
    fn invalid_state_is_also_cached() {
        let cache = TokenCache::new(Duration::from_secs(10));
        cache.put("a", TokenState::Invalid);
        assert_eq!(cache.get("a"), Some(TokenState::Invalid));
    }

    #[test]
    fn ttl_zero_entry_is_immediately_expired() {
        // With TTL=0, the entry should not be cached at all.
        let cache = TokenCache::new(Duration::from_secs(0));
        cache.put("a", TokenState::Valid(dummy_claims("read-write")));
        assert_eq!(cache.get("a"), None, "TTL=0 entry should be expired on get");
    }

    #[test]
    fn overwrite_replaces_state_and_resets_ttl() {
        let cache = TokenCache::new(Duration::from_secs(10));
        cache.put("a", TokenState::Invalid);
        assert_eq!(cache.get("a"), Some(TokenState::Invalid));
        cache.put("a", TokenState::Valid(dummy_claims("read-write")));
        assert!(matches!(cache.get("a"), Some(TokenState::Valid(_))));
    }

    #[test]
    fn distinct_tokens_have_distinct_entries() {
        let cache = TokenCache::new(Duration::from_secs(10));
        cache.put("token-one", TokenState::Valid(dummy_claims("read-write")));
        cache.put("token-two", TokenState::Invalid);
        assert!(matches!(cache.get("token-one"), Some(TokenState::Valid(_))));
        assert_eq!(cache.get("token-two"), Some(TokenState::Invalid));
        // Invalidating must clear both.
        cache.invalidate();
        assert_eq!(cache.get("token-one"), None);
        assert_eq!(cache.get("token-two"), None);
    }

    #[test]
    fn expired_entries_are_not_returned_after_ttl() {
        // Expired entries should no longer be returned, regardless of whether
        // they have been physically removed from Moka's internal storage yet.
        let cache = TokenCache::new(Duration::from_millis(10));
        cache.put("old", TokenState::Valid(dummy_claims("read-write")));
        std::thread::sleep(Duration::from_millis(20));
        cache.put("new", TokenState::Valid(dummy_claims("read-write")));

        assert_eq!(cache.get("old"), None);
        assert!(matches!(cache.get("new"), Some(TokenState::Valid(_))));
    }

    #[test]
    fn invalidate_all_is_immediately_visible_to_get() {
        let cache = TokenCache::new(Duration::from_secs(10));
        cache.put("a", TokenState::Valid(dummy_claims("read-write")));
        cache.put("b", TokenState::Invalid);
        cache.invalidate();

        // Moka guarantees get() will not return entries inserted before or at
        // the invalidation timestamp, even if physical removal is still pending.
        assert_eq!(cache.get("a"), None);
        assert_eq!(cache.get("b"), None);
    }
}
