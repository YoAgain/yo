//! Seeded random runs: many clients, random sends, syncs, clock jumps and re-registrations. Invariant: every
//! message a client sent to a device that still exists arrives exactly once and nothing is dropped. A failure names
//! its seed; `YO_SEEDS=7,8` reruns those, `YO_CLIENTS` and `YO_STEPS` make them larger.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use yo_core::{Client, Content, Event};
use yo_tests::{new_phone, random_u64, register, user_of, username, Http, Rng, TestHub};

struct Device {
    /// The user id.
    user: String,
    client: Client,
    clock: Arc<AtomicU64>,
}

/// A new account for `name`, or with `old`, a new phone of its account.
fn device(hub: &TestHub, name: &str, old: Option<&Client>) -> Device {
    let mut client = match old {
        Some(old) => new_phone(Http::new(hub.url()), name, old),
        None => register(Http::new(hub.url()), name, Some("SIM")),
    };
    let clock = Arc::new(AtomicU64::new(yo_hub::now_ms()));
    let read = clock.clone();
    client.set_clock(Arc::new(move || read.load(Ordering::Relaxed)));
    // Kept by user id: what events name, and the same after a new phone.
    Device { user: user_of(name), client, clock }
}

fn collect(events: Vec<Event>, received: &mut BTreeMap<String, Vec<(String, String)>>, to: &str) {
    for event in events {
        if let Event::Message { user, content: Content::Text(text), .. } = event {
            received.entry(to.to_owned()).or_default().push((user, text));
        }
    }
}

fn run(seed: u64, clients: usize, steps: usize) {
    let hub = TestHub::start();
    let mut rng = Rng(seed.max(1));
    let base = random_u64() % 1_000_000_000;
    let names: Vec<String> = (0..clients as u64).map(|i| username(base + i)).collect();
    let mut devices: Vec<Device> = names.iter().map(|p| device(&hub, p, None)).collect();
    // What each user should receive, and what it did. Messages a device sent before it was replaced may be
    // lost (a first contact the recipient had not fetched yet) but never duplicated.
    let mut expected: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    let mut may_be_lost: std::collections::BTreeSet<(String, String)> = Default::default();
    let mut received: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for step in 0..steps {
        let roll = rng.below(100);
        let i = rng.below(clients as u64) as usize;
        if roll < 60 {
            let mut j = rng.below(clients as u64 - 1) as usize;
            if j >= i {
                j += 1;
            }
            let text = format!("{seed}-{step}");
            let to = devices[j].user.clone();
            let from = devices[i].user.clone();
            let events = devices[i]
                .client
                .send(&names[j], Content::Text(text.clone()))
                .unwrap_or_else(|e| panic!("seed {seed} step {step}: {from} -> {to}: {e}"));
            collect(events, &mut received, &from);
            // Not asserted taken: one to a contact on a new phone waits for the next inbox read, where its new
            // profile key is (spec 4.10); the quiet-down below checks that all went.
            expected.entry(to).or_default().push((from, text));
        } else if roll < 85 {
            let to = devices[i].user.clone();
            let events = devices[i].client.sync(0).unwrap_or_else(|e| panic!("seed {seed} step {step}: sync: {e}"));
            collect(events, &mut received, &to);
        } else if roll < 98 {
            let jump = rng.below(10) * 24 * 3600 * 1000;
            devices[i].clock.fetch_add(jump, Ordering::Relaxed);
            let to = devices[i].user.clone();
            let events =
                devices[i].client.maintenance().unwrap_or_else(|e| panic!("seed {seed} step {step}: maintenance: {e}"));
            collect(events, &mut received, &to);
        } else {
            // A new phone: what the old device had not fetched is gone with it.
            let to = devices[i].user.clone();
            let mut fresh = device(&hub, &names[i], Some(&devices[i].client));
            std::mem::swap(&mut devices[i], &mut fresh);
            // The new device starts with empty lists; what the old one received was checked as it came.
            for list in expected.values() {
                for (from, text) in list {
                    if *from == to {
                        may_be_lost.insert((from.clone(), text.clone()));
                    }
                }
            }
            // What the old one had not received yet may still wait in a sender's outbox (one kept for the next
            // inbox read, spec 4.10): it may come to the new one, or not.
            let got = received.insert(to.clone(), Vec::new()).unwrap_or_default();
            let unseen: Vec<_> =
                expected.remove(&to).unwrap_or_default().into_iter().filter(|m| !got.contains(m)).collect();
            may_be_lost.extend(unseen.iter().cloned());
            expected.insert(to, unseen);
        }
    }
    // Quiet down: everybody receives until nothing moves, a couple of minutes apart, as an app's syncs come: a roster
    // asked a moment ago is asked again then, and what waited for it (a device under another's number) is decided.
    for _ in 0..4 {
        for d in devices.iter_mut() {
            d.clock.fetch_add(2 * 60 * 1000, Ordering::Relaxed);
            let to = d.user.clone();
            let events = d.client.sync(0).unwrap();
            collect(events, &mut received, &to);
        }
    }
    // A roster that could not be asked these minutes (the day's lookups used up) is asked the next day: what waited
    // for it is decided then, not lost.
    for d in devices.iter_mut() {
        if d.client.deferred() > 0 {
            d.clock.fetch_add(25 * 3600 * 1000, Ordering::Relaxed);
            let to = d.user.clone();
            for _ in 0..2 {
                let events = d.client.sync(0).unwrap();
                collect(events, &mut received, &to);
            }
        }
    }
    for d in &devices {
        // Only what a replaced device sent may be dropped (spec 4.6), and is among `may_be_lost`.
        // A payload kept for a roster ("later") is no drop, once a later sync decided it; nor is one sent identified
        // for want of a sender certificate (spec 6.1: it went, unsealed).
        assert!(
            d.client
                .drop_reasons()
                .iter()
                .all(|r| r.contains("replaced device") || r.ends_with(": later") || r.starts_with("sent identified")),
            "seed {seed}: {} dropped: {:?}",
            d.user,
            d.client.drop_reasons()
        );
        assert_eq!(d.client.deferred(), 0, "seed {seed}: {} still waits for a roster", d.user);
        assert_eq!(d.client.pending(), 0, "seed {seed}: {} has an outbox", d.user);
    }
    for (to, want) in &expected {
        let got = received.get(to).cloned().unwrap_or_default();
        let mut sorted = got.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), got.len(), "seed {seed}: {to} received something twice");
        let must: Vec<_> = want.iter().filter(|m| !may_be_lost.contains(*m)).cloned().collect();
        for m in &must {
            assert!(got.contains(m), "seed {seed}: {to} never received {m:?}");
        }
        for m in &got {
            assert!(want.contains(m), "seed {seed}: {to} received {m:?}, which nobody sent to it");
        }
    }
}

fn seeds() -> Vec<u64> {
    match std::env::var("YO_SEEDS") {
        Ok(list) => list.split(',').filter_map(|s| s.trim().parse().ok()).collect(),
        Err(_) => (1..=6).collect(),
    }
}

/// A number from the environment (`YO_CLIENTS`, `YO_STEPS`: longer runs), else `default`.
fn env_or(name: &str, default: usize) -> usize {
    std::env::var(name).ok().and_then(|n| n.parse().ok()).unwrap_or(default)
}

#[test]
fn seeded_runs() {
    for seed in seeds() {
        run(seed, env_or("YO_CLIENTS", 6), env_or("YO_STEPS", 400));
    }
}

#[test]
fn many_clients_at_once() {
    let hub = Arc::new(TestHub::start());
    let clients = 200u64;
    let base = random_u64() % 1_000_000_000;
    let threads: Vec<_> = (0..8u64)
        .map(|t| {
            let hub = hub.clone();
            std::thread::spawn(move || {
                (0..clients)
                    .filter(|i| i % 8 == t)
                    .map(|i| (i, register(Http::new(hub.url()), &username(base + i), None)))
                    .collect::<Vec<_>>()
            })
        })
        .collect();
    let mut all: Vec<(u64, Client)> = threads.into_iter().flat_map(|t| t.join().unwrap()).collect();
    all.sort_by_key(|(i, _)| *i);
    // Every client sends a Yo to the next three, all at once.
    let senders: Vec<_> = all
        .into_iter()
        .map(|(i, mut client)| {
            std::thread::spawn(move || {
                // A send receives first: Yos that arrive meanwhile come back with it.
                let mut events = Vec::new();
                for k in 1..=3 {
                    events.extend(client.send_yo(&username(base + (i + k) % clients)).unwrap());
                }
                (i, client, events)
            })
        })
        .collect();
    let mut all: Vec<(u64, Client, Vec<Event>)> = senders.into_iter().map(|t| t.join().unwrap()).collect();
    let receivers: Vec<_> = all
        .drain(..)
        .map(|(i, mut client, events)| {
            std::thread::spawn(move || {
                let mut yos =
                    events.iter().filter(|e| matches!(e, Event::Message { content: Content::Yo, .. })).count();
                for _ in 0..3 {
                    yos += client
                        .sync(0)
                        .unwrap()
                        .iter()
                        .filter(|e| matches!(e, Event::Message { content: Content::Yo, .. }))
                        .count();
                }
                (i, yos, client.dropped())
            })
        })
        .collect();
    for receiver in receivers {
        let (i, yos, dropped) = receiver.join().unwrap();
        assert_eq!((yos, dropped), (3, 0), "client {i}");
    }
}
