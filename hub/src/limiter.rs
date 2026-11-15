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

/// Requests (signed deliveries without the recipient's access key, spec 4.10) as buckets that refill evenly, by the
/// hub's clock (ms, which tests move): per sender and recipient `pair`, refilling one an hour; recipients new to a
/// sender (no bucket of the pair, or a full one) `recipients`, one an hour; per recipient from everyone
/// `per_recipient`, all of them in a day. In memory, as every rate: a restart fills them.
/// A bucket by its kind (0 pair, 1 new recipients, 2 recipient) and key: its tokens, and when (ms) they were counted.
type Buckets = HashMap<(u8, Vec<u8>), (f64, u64)>;

pub struct Requests {
    pub pair: u64,
    pub recipients: u64,
    pub per_recipient: u64,
    /// The hour of the buckets, ms (a load test runs days of use in minutes: shorter).
    pub hour_ms: u64,
    buckets: Mutex<Buckets>,
}

impl Requests {
    pub fn new(pair: u64, recipients: u64, per_recipient: u64, hour_ms: u64) -> Self {
        Requests { pair, recipients, per_recipient, hour_ms: hour_ms.max(1), buckets: Mutex::default() }
    }

    /// Size and ms per token of a kind of bucket; none: no limit.
    fn shape(&self, kind: u8) -> Option<(f64, f64)> {
        let hour = self.hour_ms as f64;
        let (size, every) = match kind {
            0 => (self.pair, hour),
            1 => (self.recipients, hour),
            _ => (self.per_recipient, 24.0 * hour / self.per_recipient.max(1) as f64),
        };
        (size != u64::MAX).then_some((size as f64, every))
    }

    /// Tokens in a bucket now (none there: full), and the shape.
    fn level(
        &self,
        buckets: &HashMap<(u8, Vec<u8>), (f64, u64)>,
        kind: u8,
        key: &[u8],
        now: u64,
    ) -> Option<(f64, f64)> {
        let (size, every) = self.shape(kind)?;
        let tokens = buckets
            .get(&(kind, key.to_vec()))
            .map_or(size, |(tokens, at)| (tokens + now.saturating_sub(*at) as f64 / every).min(size));
        Some((tokens, every))
    }

    fn keys(sender: &[u8], recipient: &[u8]) -> [(u8, Vec<u8>); 3] {
        [(0, [sender, recipient].concat()), (1, sender.to_vec()), (2, recipient.to_vec())]
    }

    /// Whether a request of `sender` to `recipient` may go now: else the bucket that is empty ("pair", "new",
    /// "recipient") and the ms until it may (`Retry-After`). Takes nothing (`take` does, once it is stored).
    pub fn check(&self, sender: &[u8], recipient: &[u8], now: u64) -> Result<(), (&'static str, u64)> {
        let buckets = self.buckets.lock().unwrap_or_else(PoisonError::into_inner);
        let keys = Self::keys(sender, recipient);
        let new = self.level(&buckets, 0, &keys[0].1, now).is_none_or(|(tokens, _)| tokens >= self.pair as f64);
        let mut wait = (0u64, "");
        for (kind, key) in &keys {
            if *kind == 1 && !new {
                continue;
            }
            if let Some((tokens, every)) = self.level(&buckets, *kind, key, now) {
                let ms = ((1.0 - tokens) * every).ceil() as u64;
                if tokens < 1.0 && ms > wait.0 {
                    wait = (ms, ["pair", "new", "recipient"][*kind as usize]);
                }
            }
        }
        match wait {
            (0, _) => Ok(()),
            (ms, scope) => Err((scope, ms)),
        }
    }

    /// A request stored: one token from each of its buckets (a recipient new to the sender: from that one too).
    pub fn take(&self, sender: &[u8], recipient: &[u8], now: u64) {
        let mut buckets = self.buckets.lock().unwrap_or_else(PoisonError::into_inner);
        let keys = Self::keys(sender, recipient);
        let new = self.level(&buckets, 0, &keys[0].1, now).is_none_or(|(tokens, _)| tokens >= self.pair as f64);
        for (kind, key) in keys {
            if kind == 1 && !new {
                continue;
            }
            if let Some((tokens, _)) = self.level(&buckets, kind, &key, now) {
                buckets.insert((kind, key), ((tokens - 1.0).max(0.0), now));
            }
        }
    }

    /// Drops the buckets full again: run periodically.
    pub fn forget_full(&self, now: u64) {
        let mut buckets = self.buckets.lock().unwrap_or_else(PoisonError::into_inner);
        let full: Vec<(u8, Vec<u8>)> = buckets
            .keys()
            .filter(|(kind, key)| {
                self.level(&buckets, *kind, key, now)
                    .is_none_or(|(tokens, _)| tokens >= self.shape(*kind).map_or(0.0, |(size, _)| size))
            })
            .cloned()
            .collect();
        for key in full {
            buckets.remove(&key);
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
    fn requests_refill_evenly_and_a_new_recipient_counts_once() {
        let r = Requests::new(3, 2, 1000, 3_600_000);
        let (a, b, c, d) = (&[1u8][..], &[2u8][..], &[3u8][..], &[4u8][..]);
        for _ in 0..3 {
            assert_eq!(r.check(a, b, 0), Ok(()));
            r.take(a, b, 0);
        }
        assert_eq!(r.check(a, b, 0), Err(("pair", 3_600_000)), "a token an hour");
        assert_eq!(r.check(a, b, 1_800_000), Err(("pair", 1_800_000)));
        assert_eq!(r.check(a, b, 3_600_000), Ok(()), "one back after an hour");
        // b was the first new recipient, c the second; d is past them until one refills.
        r.take(a, c, 0);
        assert_eq!(r.check(a, c, 0), Ok(()), "c is no new recipient any more");
        assert_eq!(r.check(a, d, 0), Err(("new", 3_600_000)));
        assert_eq!(r.check(b, d, 0), Ok(()), "another sender is not held");
        r.forget_full(100 * 3_600_000);
        assert!(r.buckets.lock().unwrap().is_empty(), "full again: forgotten");
    }

    #[test]
    fn requests_to_one_recipient_refill_over_a_day() {
        let r = Requests::new(u64::MAX, u64::MAX, 4, 3_600_000);
        for sender in 0..4u8 {
            r.take(&[sender], &[9], 0);
        }
        assert_eq!(r.check(&[7], &[9], 0), Err(("recipient", 6 * 3_600_000)), "a quarter day for a token");
        assert_eq!(r.check(&[7], &[8], 0), Ok(()));
    }

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
