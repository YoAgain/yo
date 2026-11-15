//! What a large state costs each Yo: a client with 200 contacts (sessions, sent caches) receives and sends bursts
//! through an in-process hub, its state saved as the app saves it. A measurement, not a check: run it with
//! `cargo test --release -p yo-tests --test state_cost -- --ignored --nocapture`.

use std::sync::{Arc, Mutex};
use std::time::Instant;
use yo_core::{Client, Content, Event, Push, Store};
use yo_tests::{random_u64, Http, TestHub};

/// Keeps the last save, as the app's file does, and counts them.
#[derive(Default)]
struct Memory {
    saves: Mutex<(usize, usize)>,
}

impl Store for Memory {
    fn save(&self, blob: Vec<u8>) -> Result<(), String> {
        let mut saves = self.saves.lock().unwrap();
        saves.0 += 1;
        saves.1 = blob.len();
        Ok(())
    }
}

fn yos(events: &[Event]) -> usize {
    events.iter().filter(|e| matches!(e, Event::Message { content: Content::Yo, .. })).count()
}

#[test]
#[ignore = "a measurement: run with --ignored --nocapture"]
fn a_burst_with_200_contacts() {
    const CONTACTS: usize = 200;
    const BURST: usize = 300;
    let hub = TestHub::start();
    let n = random_u64() % 1_000_000;
    let named = |tag: &str, i: usize| {
        Client::register_username(Http::new(hub.url()), &format!("S{tag}{n}x{i}"), None, None, Push::none()).unwrap()
    };
    let mut me = named("me", 0);
    let memory = Arc::new(Memory::default());
    me.persist_to(memory.clone(), [7; 32]).unwrap();
    let mut friends: Vec<Client> = (0..CONTACTS).map(|i| named("f", i)).collect();
    // Sessions both ways with every contact, and a few Yos each in its sent cache.
    for friend in &mut friends {
        friend.send_yo(me.user_id()).unwrap();
    }
    while me.sync(0).unwrap().iter().any(|e| matches!(e, Event::Message { .. })) {}
    for friend in &friends {
        for _ in 0..8 {
            me.send_yo(friend.user_id()).unwrap();
        }
    }
    let state = memory.saves.lock().unwrap().1;

    let first = &mut friends[0];
    for _ in 0..BURST {
        first.send_yo(me.user_id()).unwrap();
    }
    let started = Instant::now();
    let mut got = 0;
    while got < BURST {
        got += yos(&me.sync(0).unwrap());
    }
    let receive = started.elapsed();

    let started = Instant::now();
    for _ in 0..BURST {
        me.send_yo(friends[1].user_id()).unwrap();
    }
    let send = started.elapsed();

    let started = Instant::now();
    for _ in 0..50 {
        std::hint::black_box(me.export(&[7; 32]));
    }
    let save = started.elapsed().as_secs_f64() * 1e6 / 50.0;
    println!("one save (export and seal): {save:.0} µs");
    let per = |d: std::time::Duration| d.as_secs_f64() * 1e6 / BURST as f64;
    println!(
        "state {} KiB, {CONTACTS} contacts: receive {:.0} µs/Yo ({:.0}/s), send {:.0} µs/Yo ({:.0}/s), {} saves",
        state / 1024,
        per(receive),
        1e6 / per(receive),
        per(send),
        1e6 / per(send),
        memory.saves.lock().unwrap().0
    );
}
