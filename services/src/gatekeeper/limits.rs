//! How often, the same for every service: a token bucket per sender, a pause per group, an amount an hour in all.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

const BURST: f64 = 3.0;
const PER_SECOND: f64 = 1.0;
const GROUP_PAUSE: Duration = Duration::from_secs(30);
/// Senders, or groups, at once. No active one is forgotten for a new one: a flood of fresh ones resets no limit.
const MAX: usize = 1000;
const HOUR: Duration = Duration::from_secs(3600);

pub struct Limits {
    senders: HashMap<String, (Instant, f64)>,
    groups: HashMap<String, Instant>,
    hour: VecDeque<Instant>,
    per_hour: usize,
}

impl Limits {
    /// `sent`: how many went out in the last hour already (before a restart).
    pub fn new(per_hour: usize, sent: usize, now: Instant) -> Limits {
        let hour = std::iter::repeat_n(now, sent.min(per_hour)).collect();
        Limits { senders: HashMap::new(), groups: HashMap::new(), hour, per_hour }
    }

    /// Whether what came from `sender` (in `group`) is taken.
    pub fn take(&mut self, sender: &str, group: Option<&str>, now: Instant) -> bool {
        self.senders.retain(|_, (last, _)| now - *last < Duration::from_secs(3));
        self.groups.retain(|_, last| now - *last < GROUP_PAUSE);
        if let Some(group) = group {
            if self.groups.contains_key(group) || self.groups.len() >= MAX {
                return false;
            }
        }
        if !self.senders.contains_key(sender) && self.senders.len() >= MAX {
            return false;
        }
        let (last, tokens) = self.senders.get(sender).copied().unwrap_or((now, BURST));
        let tokens = (tokens + (now - last).as_secs_f64() * PER_SECOND).min(BURST);
        let taken = tokens >= 1.0;
        self.senders.insert(sender.to_owned(), (now, if taken { tokens - 1.0 } else { tokens }));
        if let (true, Some(group)) = (taken, group) {
            self.groups.insert(group.to_owned(), now);
        }
        taken
    }

    /// Whether there is room for one more this hour.
    pub fn room(&mut self, now: Instant) -> bool {
        while self.hour.front().is_some_and(|t| now - *t >= HOUR) {
            self.hour.pop_front();
        }
        self.hour.len() < self.per_hour
    }

    /// Whether one more may go out this hour: then it counts.
    pub fn send(&mut self, now: Instant) -> bool {
        let may = self.room(now);
        if may {
            self.hour.push_back(now);
        }
        may
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn senders_groups_and_the_hour() {
        let (mut limits, now) = (Limits::new(3, 1, Instant::now()), Instant::now());
        assert_eq!((0..9).filter(|_| limits.take("a", None, now)).count(), 3);
        assert!(limits.take("b", None, now));
        assert!(limits.take("a", None, now + Duration::from_secs(1)));
        assert!(limits.take("c", Some("g"), now));
        assert!(!limits.take("d", Some("g"), now + Duration::from_secs(29)));
        assert!(limits.take("d", Some("g"), now + Duration::from_secs(30)));
        assert_eq!((0..5).filter(|_| limits.send(now)).count(), 2);
        assert!(limits.send(now + HOUR));
    }

    #[test]
    fn a_flood_of_fresh_senders_resets_nobody() {
        let (mut limits, now) = (Limits::new(1, 0, Instant::now()), Instant::now());
        while limits.take("a", None, now) {}
        for i in 0..3 * MAX {
            limits.take(&format!("{i}"), None, now);
        }
        assert!(!limits.take("a", None, now));
        assert!(limits.senders.len() <= MAX);
    }
}
