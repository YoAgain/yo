//! Each hub publishes its own signatures and bucket arrivals through its local agent.

use crate::{blobs::hex, corrosion::Agent, db::Db, Hub};
use rusqlite::{params, Result};
use serde_json::json;
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

const MAX_PENDING: usize = 100_000;
type Seen = ([u8; 16], [u8; 64], u64);
type Slots = BTreeMap<u64, f64>;

#[derive(Default)]
pub(crate) struct Shared {
    seen: Mutex<VecDeque<Seen>>,
    buckets: Mutex<HashMap<String, Bucket>>,
    seen_error: AtomicBool,
    limit_error: AtomicBool,
}

struct Bucket {
    slots: Slots,
    window: u64,
    width: u64,
    dirty: bool,
}

fn live(start: u64, width: u64, window: u64, now: u64) -> bool {
    start.saturating_add(width).saturating_add(window) > now
}

/// Walk backwards from the latest arrival. Each suffix is a possible uninterrupted busy period.
fn check(
    remote: &Slots,
    local: &Slots,
    width: u64,
    now: u64,
    cost: f64,
    size: f64,
    rate: f64,
) -> std::result::Result<(), u64> {
    if remote.is_empty() {
        return Ok(());
    }
    let mut slots = remote.clone();
    for (start, n) in local {
        *slots.entry(*start).or_default() += n;
    }
    let mut sum = 0.0;
    let mut level = 0.0_f64;
    for (start, n) in slots.into_iter().rev() {
        sum += n;
        level = level.max(sum - rate * now.saturating_sub(start.saturating_add(width)) as f64);
    }
    let excess = level + cost - size;
    if excess > 0.0 {
        Err((excess / rate).ceil() as u64)
    } else {
        Ok(())
    }
}

fn read<T: Default>(result: Result<T>, failing: &AtomicBool) -> T {
    match result {
        Ok(value) => {
            failing.store(false, Ordering::Relaxed);
            value
        }
        Err(error) => {
            if !failing.swap(true, Ordering::Relaxed) {
                eprintln!("yo-hub: shared admission read: {error}");
            }
            T::default()
        }
    }
}

impl Db {
    pub(crate) fn unseen(
        &self,
        signer: [u8; 16],
        signature: [u8; 64],
        expires: u64,
        now: u64,
        forwarded: bool,
    ) -> bool {
        let Some(agent) = &self.agent else { return true };
        if !forwarded {
            let found = self
                .conn()
                .prepare_cached("SELECT 1 FROM corro.hub_seen WHERE signature=? AND expires>? LIMIT 1")
                .and_then(|mut q| q.exists(params![signature.as_slice(), now]));
            if read(found, &agent.shared.seen_error) {
                return false;
            }
        }
        let mut seen = agent.shared.seen.lock().unwrap();
        if seen.len() == MAX_PENDING {
            seen.pop_front();
        }
        seen.push_back((signer, signature, expires));
        true
    }

    fn remote_slots(&self, key: &str, hub: &str, width: u64, window: u64, now: u64) -> Result<Slots> {
        let conn = self.conn();
        let mut stmt = conn.prepare_cached("SELECT slots FROM corro.hub_limit WHERE key=? AND hub!=?")?;
        let mut rows = stmt.query(params![key, hub])?;
        let mut remote = Slots::new();
        while let Some(row) = rows.next()? {
            let slots: Vec<(u64, f64)> = serde_json::from_str(&row.get::<_, String>(0)?)
                .map_err(|e| rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e)))?;
            for (start, n) in slots {
                if n > 0.0 && live(start, width, window, now) {
                    *remote.entry(start).or_default() += n;
                }
            }
        }
        Ok(remote)
    }

    pub(crate) fn shared_take(
        &self,
        key: &str,
        cost: f64,
        size: f64,
        rate: f64,
        now: u64,
    ) -> std::result::Result<(), u64> {
        let Some(agent) = &self.agent else { return Ok(()) };
        let window = (size / rate).ceil() as u64;
        let width = (window / 32).max(1000);
        let mut buckets = agent.shared.buckets.lock().unwrap();
        let remote = read(self.remote_slots(key, &agent.config.hub, width, window, now), &agent.shared.limit_error);
        let bucket =
            buckets.entry(key.into()).or_insert_with(|| Bucket { slots: Slots::new(), window, width, dirty: false });
        bucket.slots.retain(|start, _| live(*start, width, window, now));
        check(&remote, &bucket.slots, width, now, cost, size, rate)?;
        *bucket.slots.entry(now / width * width).or_default() += cost;
        bucket.dirty = true;
        Ok(())
    }

    pub(crate) fn shared_give_back(&self, key: &str, cost: f64, taken: u64) {
        let Some(agent) = &self.agent else { return };
        let mut buckets = agent.shared.buckets.lock().unwrap();
        if let Some(bucket) = buckets.get_mut(key) {
            let start = taken / bucket.width * bucket.width;
            if let Some(n) = bucket.slots.get_mut(&start) {
                *n -= cost;
                if *n <= 0.0 {
                    bucket.slots.remove(&start);
                }
                bucket.dirty = true;
            }
        }
    }
}

impl Agent {
    fn publish(&self, now: u64, request_now: u64) -> Result<()> {
        let mut batch = Vec::new();
        let seen = {
            let mut queue = self.shared.seen.lock().unwrap();
            queue.retain(|s| s.2 > now);
            let n = queue.len().min(2000);
            queue.drain(..n).collect::<Vec<_>>()
        };
        for chunk in seen.chunks(100) {
            let mut args = Vec::new();
            for (signer, sig, expires) in chunk {
                args.extend([json!(hex(sig)), json!(self.config.hub), json!(hex(signer)), json!(expires)]);
            }
            let values = vec!["(unhex(?),?,unhex(?),?)"; chunk.len()].join(",");
            batch.push(json!([
                format!("INSERT OR IGNORE INTO hub_seen(signature,hub,signer,expires) VALUES {values}"),
                args
            ]));
        }

        let mut written = Vec::new();
        {
            let mut buckets = self.shared.buckets.lock().unwrap();
            for (key, b) in buckets.iter_mut() {
                let clock = if key.starts_with("req") { request_now } else { now };
                b.slots.retain(|s, _| live(*s, b.width, b.window, clock));
                if b.dirty || b.slots.is_empty() {
                    let slots = json!(b.slots.iter().collect::<Vec<_>>()).to_string();
                    batch.push(json!(["INSERT INTO hub_limit(key,hub,slots,at) VALUES(?,?,?,?) ON CONFLICT(key,hub) DO UPDATE SET slots=excluded.slots, at=excluded.at",
                        [key,self.config.hub,slots,now]]));
                    b.dirty = false;
                    written.push(key.clone());
                }
            }
        }
        // A rush of distinct keys must still fit through the agent's HTTP body limit.
        let mut statements = batch.into_iter();
        let result = (|| {
            while !statements.as_slice().is_empty() {
                self.write(statements.by_ref().take(128).collect())?;
            }
            Ok(())
        })();
        let mut buckets = self.shared.buckets.lock().unwrap();
        for key in written {
            if let Some(b) = buckets.get_mut(&key) {
                if result.is_err() {
                    b.dirty = true;
                } else if !b.dirty && b.slots.is_empty() {
                    buckets.remove(&key);
                }
            }
        }
        if result.is_err() {
            let mut queue = self.shared.seen.lock().unwrap();
            let room = MAX_PENDING - queue.len();
            let now = crate::now_ms();
            for entry in seen.into_iter().rev().filter(|s| s.2 > now).take(room) {
                queue.push_front(entry);
            }
        }
        result
    }

    pub(crate) fn prune_shared(&self, now: u64) -> Result<usize> {
        self.write(vec![
            json!(["DELETE FROM hub_seen WHERE hub=? AND expires<=?", [self.config.hub, now]]),
            json!([
                "DELETE FROM hub_limit WHERE hub=? AND at<?",
                [self.config.hub, now.saturating_sub(2 * crate::request::DAY_MS)]
            ]),
        ])
    }
}

pub(crate) async fn run(hub: Arc<Hub>) {
    let mut failing = false;
    loop {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let result = hub.blocking(|h| h.db.agent.as_ref().unwrap().publish(crate::now_ms(), h.clock_ms())).await;
        if !matches!(result, Ok(Ok(()))) {
            if !failing {
                eprintln!("yo-hub: shared admission publish: {result:?}");
            }
            failing = true;
        } else {
            failing = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_bucket_refills_from_each_arrival() {
        let local = Slots::from([(0, 4.0)]);
        assert_eq!(check(&Slots::new(), &local, 1000, 1000, 1., 4., 0.001), Ok(()));
        let remote = Slots::from([(2000, 2.0)]);
        assert_eq!(check(&remote, &local, 1000, 3000, 1., 4., 0.001), Err(1000));
        assert_eq!(check(&remote, &local, 1000, 4000, 1., 4., 0.001), Ok(()));
        let fractional = Slots::from([(0, 2.0)]);
        assert_eq!(check(&fractional, &Slots::new(), 1000, 1000, 1., 2., 0.3), Err(4));
        let remote = Slots::from([(10000, 4.0)]);
        assert_eq!(check(&remote, &local, 1000, 10500, 1., 4., 0.001), Err(1000));
    }
}
