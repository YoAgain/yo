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
    /// Takes one token from the bucket `key`, which holds `per` and refills `per` every `window`.
    pub fn allow(&self, key: String, per: u32, window: Duration) -> bool {
        if per == u32::MAX {
            return true;
        }
        let now = Instant::now();
        let capacity = per as f64;
        let rate = capacity / window.as_secs_f64();
        let mut buckets = self.buckets.lock().unwrap_or_else(PoisonError::into_inner);
        if buckets.len() > 200_000 {
            buckets.retain(|_, (tokens, at)| *tokens + now.duration_since(*at).as_secs_f64() * rate < capacity);
        }
        let (tokens, at) = buckets.entry(key).or_insert((capacity, now));
        *tokens = (*tokens + now.duration_since(*at).as_secs_f64() * rate).min(capacity);
        *at = now;
        if *tokens >= 1.0 {
            *tokens -= 1.0;
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
    /// Records a signature valid until `expires_ms`; false when it was seen before.
    pub fn first_time(&self, signature: [u8; 64], now_ms: u64, expires_ms: u64) -> bool {
        let mut seen = self.seen.lock().unwrap_or_else(PoisonError::into_inner);
        if seen.len() > 100_000 {
            seen.retain(|_, expiry| *expiry > now_ms);
        }
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
    fn replay_once() {
        let cache = ReplayCache::default();
        assert!(cache.first_time([1; 64], 0, 100));
        assert!(!cache.first_time([1; 64], 50, 150));
        assert!(cache.first_time([1; 64], 101, 200));
    }
}
