//! Token buckets for rate limits, and the replay cache of signed requests. Both in memory: a restart forgets them,
//! which costs at most one window.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct Limiter {
    buckets: Mutex<HashMap<String, (f64, Instant)>>,
}

impl Limiter {
    /// Drops buckets untouched for `idle`: run periodically, never inside a request. The longest window is a day,
    /// so a bucket idle that long is full again and costs nothing to forget.
    pub fn forget_idle(&self, idle: Duration) {
        let now = Instant::now();
        self.buckets.lock().unwrap_or_else(PoisonError::into_inner).retain(|_, (_, at)| now.duration_since(*at) < idle);
    }

    /// Takes one token from the bucket `key`, which holds `per` and refills `per` every `window`.
    pub fn allow(&self, key: String, per: u32, window: Duration) -> bool {
        if per == u32::MAX {
            return true;
        }
        self.allow_n(key, 1, per as u64, window)
    }

    /// Whether the bucket `key` is used up now, taking nothing from it.
    pub fn spent(&self, key: &str, per: u32, window: Duration) -> bool {
        if per == u32::MAX {
            return false;
        }
        let buckets = self.buckets.lock().unwrap_or_else(PoisonError::into_inner);
        let Some((tokens, at)) = buckets.get(key) else { return false };
        let rate = per as f64 / window.as_secs_f64();
        (tokens + at.elapsed().as_secs_f64() * rate).min(per as f64) < 1.0
    }

    /// Takes `cost` tokens (bytes, say) from the bucket `key`, which holds `per` and refills `per` every `window`;
    /// none when it holds fewer. `u64::MAX` is no limit.
    pub fn allow_n(&self, key: String, cost: u64, per: u64, window: Duration) -> bool {
        if per == u64::MAX {
            return true;
        }
        let now = Instant::now();
        let capacity = per as f64;
        let rate = capacity / window.as_secs_f64();
        let cost = cost as f64;
        let mut buckets = self.buckets.lock().unwrap_or_else(PoisonError::into_inner);
        let (tokens, at) = buckets.entry(key).or_insert((capacity, now));
        *tokens = (*tokens + now.duration_since(*at).as_secs_f64() * rate).min(capacity);
        *at = now;
        if *tokens >= cost {
            *tokens -= cost;
            true
        } else {
            false
        }
    }
}

/// Signatures seen within the time window: each signed request is accepted once.
#[derive(Default)]
pub struct ReplayCache {
    seen: Mutex<HashMap<[u8; 64], u64>>,
}

impl ReplayCache {
    /// Drops expired signatures: run periodically.
    pub fn forget_expired(&self, now_ms: u64) {
        self.seen.lock().unwrap_or_else(PoisonError::into_inner).retain(|_, expiry| *expiry > now_ms);
    }

    /// Records a signature valid until `expires_ms`; false when it was seen before.
    pub fn first_time(&self, signature: [u8; 64], now_ms: u64, expires_ms: u64) -> bool {
        let mut seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
        match seen.get(&signature) {
            Some(expiry) if *expiry > now_ms => false,
            _ => {
                seen.insert(signature, expires_ms);
                true
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_runs_dry() {
        let limiter = Limiter::default();
        for _ in 0..3 {
            assert!(limiter.allow("k".into(), 3, Duration::from_secs(3600)));
        }
        assert!(!limiter.allow("k".into(), 3, Duration::from_secs(3600)));
        assert!(limiter.allow("other".into(), 3, Duration::from_secs(3600)));
    }

    #[test]
    fn spent_looks_without_taking() {
        let (limiter, hour) = (Limiter::default(), Duration::from_secs(3600));
        assert!(!limiter.spent("k", 2, hour), "untouched: full");
        assert!(limiter.allow("k".into(), 2, hour));
        assert!(!limiter.spent("k", 2, hour));
        assert!(!limiter.spent("k", 2, hour), "looking took nothing");
        assert!(limiter.allow("k".into(), 2, hour));
        assert!(limiter.spent("k", 2, hour));
    }

    #[test]
    fn a_byte_bucket_runs_dry_by_size() {
        let limiter = Limiter::default();
        assert!(limiter.allow_n("b".into(), 60, 100, Duration::from_secs(3600)));
        assert!(!limiter.allow_n("b".into(), 60, 100, Duration::from_secs(3600)), "40 left");
        assert!(limiter.allow_n("b".into(), 40, 100, Duration::from_secs(3600)));
        assert!(limiter.allow_n("b".into(), u64::MAX - 1, u64::MAX, Duration::from_secs(1)), "no limit");
    }

    #[test]
    fn replay_once() {
        let cache = ReplayCache::default();
        assert!(cache.first_time([1; 64], 0, 100));
        assert!(!cache.first_time([1; 64], 50, 150));
        assert!(cache.first_time([1; 64], 101, 200));
    }
}
