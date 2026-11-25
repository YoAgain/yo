//! Where a connection is begun, or waited for, from: so one source takes no place from the others. In memory alone, never
//! logged, forgotten within minutes.

use axum::http::HeaderMap;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Instant;

/// Begun at once, and then one in so many seconds.
const BURST: f64 = 10.0;
const EVERY_S: f64 = 10.0;
/// Waits at once (one of them may be a page's that went away).
const WAITS: usize = 3;
/// As many sources as are kept; more are refused.
const MAX: usize = 20_000;

/// The caller as the tunnel names it: the entrance listens on loopback alone, so nobody else reaches it. An IPv6 address
/// by its /64. All without one are one.
pub fn source(headers: &HeaderMap) -> u128 {
    let ip = headers.get("cf-connecting-ip").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok());
    match ip.map(|ip: IpAddr| ip.to_canonical()) {
        Some(IpAddr::V4(ip)) => u128::from(ip.to_ipv6_mapped()),
        Some(IpAddr::V6(ip)) => u128::from(ip) >> 64 << 64,
        None => 0,
    }
}

struct Source {
    tokens: f64,
    last: Instant,
    waits: usize,
}

impl Source {
    fn tokens(&self, now: Instant) -> f64 {
        (self.tokens + (now - self.last).as_secs_f64() / EVERY_S).min(BURST)
    }
}

#[derive(Default)]
pub struct Sources(Mutex<HashMap<u128, Source>>);

/// A place to wait, until dropped.
pub struct Waits<'a>(&'a Sources, u128);

impl Drop for Waits<'_> {
    fn drop(&mut self) {
        if let Some(source) = self.0.lock().get_mut(&self.1) {
            source.waits -= 1;
        }
    }
}

impl Sources {
    fn lock(&self) -> MutexGuard<'_, HashMap<u128, Source>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether `source` may begin a connection.
    pub fn connect(&self, source: u128) -> bool {
        let now = Instant::now();
        self.with(source, now, |s| {
            s.tokens = s.tokens(now);
            s.last = now;
            let may = s.tokens >= 1.0;
            s.tokens -= if may { 1.0 } else { 0.0 };
            may
        })
    }

    /// A place to wait.
    pub fn wait(&self, source: u128) -> Option<Waits<'_>> {
        let may = self.with(source, Instant::now(), |s| {
            let may = s.waits < WAITS;
            s.waits += usize::from(may);
            may
        });
        may.then(|| Waits(self, source))
    }

    fn with(&self, source: u128, now: Instant, may: impl FnOnce(&mut Source) -> bool) -> bool {
        let mut sources = self.lock();
        if sources.len() >= MAX && !sources.contains_key(&source) {
            sources.retain(|_, s| s.waits > 0 || s.tokens(now) < BURST);
            if sources.len() >= MAX {
                return false;
            }
        }
        may(sources.entry(source).or_insert(Source { tokens: BURST, last: now, waits: 0 }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_source_takes_nothing_from_another() {
        let sources = Sources::default();
        assert_eq!((0..20).filter(|_| sources.connect(1)).count(), BURST as usize);
        assert!(sources.connect(2));
        let waits: Vec<_> = std::iter::from_fn(|| sources.wait(1)).collect();
        assert_eq!(waits.len(), WAITS);
        assert!(sources.wait(2).is_some());
        drop(waits);
        assert!(sources.wait(1).is_some());
        let of = |ip: &str| source(&HeaderMap::from_iter([("cf-connecting-ip".parse().unwrap(), ip.parse().unwrap())]));
        assert_eq!((of("2001:db8::1"), of("::ffff:192.0.2.1")), (of("2001:db8::ff:2"), of("192.0.2.1")));
        assert_ne!((of("2001:db8::1"), of("192.0.2.1")), (of("2001:db8:0:1::1"), of("192.0.2.2")));
        assert_eq!(of("x"), source(&HeaderMap::new()));
    }
}
