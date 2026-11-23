//! Rate limits and the replay cache, in memory: a restart forgets them, which costs at most one window.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct Limiter {
    buckets: Mutex<HashMap<String, (f64, Instant)>>,
}

impl Limiter {
    /// Drops buckets idle for `idle` (a day: full again), run periodically.
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

    /// Takes `cost` tokens from the bucket `key` (`per` every `window`), none if it holds fewer; `u64::MAX`: no limit.
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

impl Limiter {
    /// `cost` tokens from the bucket `key`, which holds `size` and gets `per` back every `window`; else none, and
    /// the ms until there are enough. `u64::MAX`: no limit.
    pub fn take_shaped(&self, key: String, cost: u64, size: u64, per: u64, window: Duration) -> Result<(), u64> {
        if size == u64::MAX {
            return Ok(());
        }
        let now = Instant::now();
        let rate = per.max(1) as f64 / window.as_secs_f64();
        let cost = (cost as f64).min(size as f64);
        let mut buckets = self.buckets.lock().unwrap_or_else(PoisonError::into_inner);
        let (tokens, at) = buckets.entry(key).or_insert((size as f64, now));
        *tokens = (*tokens + now.duration_since(*at).as_secs_f64() * rate).min(size as f64);
        *at = now;
        if *tokens >= cost {
            *tokens -= cost;
            Ok(())
        } else {
            Err((((cost - *tokens) / rate) * 1000.0).ceil() as u64)
        }
    }
}

/// Places under way by key: a device's inbox requests, a user's uploads, a blob id's upload.
pub type Places = Mutex<HashMap<[u8; 16], u32>>;

/// A place taken (`take_place`), given back when dropped.
pub struct Place {
    places: Arc<Places>,
    key: [u8; 16],
}

impl Drop for Place {
    fn drop(&mut self) {
        let mut places = self.places.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(n) = places.get_mut(&self.key) {
            *n -= 1;
            if *n == 0 {
                places.remove(&self.key);
            }
        }
    }
}

/// One more place for `key`, if it holds fewer than `most` and all together fewer than `all`.
pub fn take_place(places: &Arc<Places>, key: [u8; 16], most: u32, all: Option<u32>) -> Option<Place> {
    let mut taken = places.lock().unwrap_or_else(PoisonError::into_inner);
    let free = taken.get(&key).is_none_or(|n| *n < most) && all.is_none_or(|all| taken.values().sum::<u32>() < all);
    if free {
        *taken.entry(key).or_default() += 1;
    }
    free.then(|| Place { places: places.clone(), key })
}

/// Requests' buckets (spec 4.10) by kind (0 pair, 1 new recipients, 2 recipient) and key: tokens, and when (ms).
type Buckets = HashMap<(u8, Vec<u8>), (f64, u64)>;

pub struct Requests {
    pub pair: u64,
    pub recipients: u64,
    pub per_recipient: u64,
    /// The hour of the buckets, ms (a load test runs days of use in minutes: shorter).
    pub hour_ms: u64,
    buckets: Mutex<Buckets>,
}

/// A request bucket, its tokens now and ms per token.
type Level = ((u8, Vec<u8>), f64, f64);

/// The buckets a request took from (`Requests::reserve`).
pub struct Reserved(Vec<(u8, Vec<u8>)>);

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

    /// Tokens in a bucket now (none there: full), and ms per token.
    fn level(&self, buckets: &Buckets, bucket: &(u8, Vec<u8>), now: u64) -> Option<(f64, f64)> {
        let (size, every) = self.shape(bucket.0)?;
        let tokens = buckets
            .get(bucket)
            .map_or(size, |(tokens, at)| (tokens + now.saturating_sub(*at) as f64 / every).min(size));
        Some((tokens, every))
    }

    /// The buckets a request counts in, with tokens and ms per token; new recipients' only for one new to the sender.
    fn levels(&self, buckets: &Buckets, sender: &[u8], recipient: &[u8], now: u64) -> Vec<Level> {
        let keys = [(0, [sender, recipient].concat()), (1, sender.to_vec()), (2, recipient.to_vec())];
        let new = self.level(buckets, &keys[0], now).is_none_or(|(tokens, _)| tokens >= self.pair as f64);
        let counted = keys.into_iter().filter(|(kind, _)| *kind != 1 || new);
        counted.filter_map(|key| self.level(buckets, &key, now).map(|(tokens, every)| (key, tokens, every))).collect()
    }

    /// The emptiest bucket of `levels` below one token, and the ms until it has one (`Retry-After`).
    fn empty(levels: &[Level]) -> Option<(&'static str, u64)> {
        let mut longest: Option<(&'static str, u64)> = None;
        for ((kind, _), tokens, every) in levels {
            let wait = ((1.0 - tokens) * every).ceil() as u64;
            if *tokens < 1.0 && longest.is_none_or(|(_, longest)| wait > longest) {
                longest = Some((["pair", "new", "recipient"][*kind as usize], wait));
            }
        }
        longest
    }

    /// `Err` with the empty bucket and the ms until a token (`Retry-After`); takes nothing.
    pub fn check(&self, sender: &[u8], recipient: &[u8], now: u64) -> Result<(), (&'static str, u64)> {
        let buckets = self.buckets.lock().unwrap_or_else(PoisonError::into_inner);
        Self::empty(&self.levels(&buckets, sender, recipient, now)).map_or(Ok(()), Err)
    }

    /// A request stored: one token from each of its buckets.
    pub fn take(&self, sender: &[u8], recipient: &[u8], now: u64) {
        let mut buckets = self.buckets.lock().unwrap_or_else(PoisonError::into_inner);
        for (key, tokens, _) in self.levels(&buckets, sender, recipient, now) {
            buckets.insert(key, ((tokens - 1.0).max(0.0), now));
        }
    }

    /// `check` and `take` at once, before the request is stored: requests sent together cannot all pass one
    /// token. The buckets taken from, for `give_back` when it is not stored.
    pub fn reserve(&self, sender: &[u8], recipient: &[u8], now: u64) -> Result<Reserved, (&'static str, u64)> {
        let mut buckets = self.buckets.lock().unwrap_or_else(PoisonError::into_inner);
        let levels = self.levels(&buckets, sender, recipient, now);
        if let Some(empty) = Self::empty(&levels) {
            return Err(empty);
        }
        let mut taken = Vec::with_capacity(levels.len());
        for (key, tokens, _) in levels {
            buckets.insert(key.clone(), ((tokens - 1.0).max(0.0), now));
            taken.push(key);
        }
        Ok(Reserved(taken))
    }

    /// A reserved request not stored after all: its tokens back.
    pub fn give_back(&self, reserved: Reserved, now: u64) {
        let mut buckets = self.buckets.lock().unwrap_or_else(PoisonError::into_inner);
        for key in reserved.0 {
            if let Some((tokens, _)) = self.level(&buckets, &key, now) {
                let size = self.shape(key.0).map_or(tokens, |(size, _)| size);
                buckets.insert(key, ((tokens + 1.0).min(size), now));
            }
        }
    }

    /// Drops the buckets full again: run periodically.
    pub fn forget_full(&self, now: u64) {
        let mut buckets = self.buckets.lock().unwrap_or_else(PoisonError::into_inner);
        buckets.retain(|(kind, _), (tokens, at)| {
            self.shape(*kind)
                .is_some_and(|(size, every)| (*tokens + now.saturating_sub(*at) as f64 / every).min(size) < size)
        });
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

    #[test]
    fn a_reservation_holds_its_token_until_given_back() {
        let r = Requests::new(1, 20, 1000, 3_600_000);
        let (a, b) = (&[1u8][..], &[2u8][..]);
        let first = r.reserve(a, b, 0).expect("a token");
        assert!(r.reserve(a, b, 0).is_err(), "requests sent together share the one token");
        r.give_back(first, 0);
        assert!(r.reserve(a, b, 0).is_ok(), "a request not stored gives its token back");
    }
}
