//! Seeded random runs: many clients, random sends, syncs, clock jumps and re-registrations. Invariant: every
//! message a client sent to a device that still exists arrives exactly once, nothing is dropped, and both sides of
//! every chat are in the same epoch at the end. A failure names its seed; `YO_SEEDS=7,8` reruns those.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use yo_core::{Client, Content, Event};
use yo_tests::{phone, random_u64, register, Http, Rng, TestHub};

struct Device {
    phone: String,
    client: Client,
    clock: Arc<AtomicU64>,
}

fn device(hub: &TestHub, phone: &str) -> Device {
    let mut client = register(Http::new(hub.url()), phone, Some("SIM"));
    let clock = Arc::new(AtomicU64::new(yo_hub::now_ms()));
    let read = clock.clone();
    client.set_clock(Arc::new(move || read.load(Ordering::Relaxed)));
    Device { phone: phone.trim_start_matches('+').to_owned(), client, clock }
}

fn collect(events: Vec<Event>, received: &mut BTreeMap<String, Vec<(String, String)>>, to: &str) {
    for event in events {
        if let Event::Message { phone, content: Content::Text(text), .. } = event {
            received.entry(to.to_owned()).or_default().push((phone, text));
        }
    }
}

fn run(seed: u64, clients: usize, steps: usize) {
    let hub = TestHub::start();
    let mut rng = Rng(seed.max(1));
    let base = random_u64() % 1_000_000_000;
    let phones: Vec<String> = (0..clients as u64).map(|i| phone(base + i)).collect();
    let mut devices: Vec<Device> = phones.iter().map(|p| device(&hub, p)).collect();
    // What each number should receive, and what it did. Messages a device sent before it was replaced may be
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
            let to = devices[j].phone.clone();
            let from = devices[i].phone.clone();
            let events = devices[i]
                .client
                .send(&phones[j], Content::Text(text.clone()))
                .unwrap_or_else(|e| panic!("seed {seed} step {step}: {from} -> {to}: {e}"));
            collect(events, &mut received, &from);
            assert_eq!(devices[i].client.pending(), 0, "seed {seed} step {step}");
            expected.entry(to).or_default().push((from, text));
        } else if roll < 85 {
            let to = devices[i].phone.clone();
            let events = devices[i].client.sync(0).unwrap_or_else(|e| panic!("seed {seed} step {step}: sync: {e}"));
            collect(events, &mut received, &to);
        } else if roll < 98 {
            let jump = rng.below(10) * 24 * 3600 * 1000;
            devices[i].clock.fetch_add(jump, Ordering::Relaxed);
            let to = devices[i].phone.clone();
            let events =
                devices[i].client.maintenance().unwrap_or_else(|e| panic!("seed {seed} step {step}: maintenance: {e}"));
            collect(events, &mut received, &to);
        } else {
            // A new phone: what the old device had not fetched is gone with it.
            let to = devices[i].phone.clone();
            let mut fresh = device(&hub, &phones[i]);
            std::mem::swap(&mut devices[i], &mut fresh);
            // The new device starts with empty lists; what the old one received was checked as it came.
            for list in expected.values() {
                for (from, text) in list {
                    if *from == to {
                        may_be_lost.insert((from.clone(), text.clone()));
                    }
                }
            }
            received.insert(to.clone(), Vec::new());
            expected.insert(to, Vec::new());
        }
    }
    // Quiet down: everybody receives until nothing moves.
    for _ in 0..4 {
        for d in devices.iter_mut() {
            let to = d.phone.clone();
            let events = d.client.sync(0).unwrap();
            collect(events, &mut received, &to);
        }
    }
    for d in &devices {
        assert_eq!(d.client.dropped(), 0, "seed {seed}: {} dropped: {:?}", d.phone, d.client.drop_reasons());
        assert_eq!(d.client.pending(), 0, "seed {seed}: {} has an outbox", d.phone);
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
    // Both sides of every chat agree on the epoch.
    let mut epochs: BTreeMap<[u8; 32], Vec<u64>> = BTreeMap::new();
    for d in &devices {
        for (_, group, epoch) in d.client.chats() {
            epochs.entry(group).or_default().push(epoch);
        }
    }
    for (group, seen) in epochs {
        if seen.len() == 2 {
            assert_eq!(seen[0], seen[1], "seed {seed}: group {:02x?} split", &group[..4]);
        }
    }
}

fn seeds() -> Vec<u64> {
    match std::env::var("YO_SEEDS") {
        Ok(list) => list.split(',').filter_map(|s| s.trim().parse().ok()).collect(),
        Err(_) => (1..=6).collect(),
    }
}

#[test]
fn seeded_runs() {
    for seed in seeds() {
        run(seed, 6, 400);
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
                    .map(|i| (i, register(Http::new(hub.url()), &phone(base + i), None)))
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
                    events.extend(client.send_yo(&phone(base + (i + k) % clients)).unwrap());
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
