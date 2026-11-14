//! What a review of the core found (Codex, after the chaos runs), each the smallest case that showed it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use yo_core::{Client, Content, Error, Event, Push, Store};
use yo_tests::{random_u64, Http, TestHub};

fn by_name(http: Arc<Http>, name: &str) -> Client {
    Client::register_username(http, name, None, None, Push::none()).unwrap()
}

fn pair(hub: &TestHub, tag: &str) -> (Client, Client) {
    let n = random_u64() % 100_000;
    (by_name(Http::new(hub.url()), &format!("A{tag}{n}")), by_name(Http::new(hub.url()), &format!("B{tag}{n}")))
}

fn texts(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Message { content: Content::Text(t), .. } => Some(t.clone()),
            _ => None,
        })
        .collect()
}

fn text(t: &str) -> Content {
    Content::Text(t.into())
}

/// A disk that can be full.
#[derive(Default)]
struct Disk {
    saved: Mutex<Vec<u8>>,
    full: AtomicBool,
}

impl Store for Disk {
    fn save(&self, blob: Vec<u8>) -> Result<(), String> {
        if self.full.load(Ordering::Relaxed) {
            return Err("disk full".into());
        }
        *self.saved.lock().unwrap() = blob;
        Ok(())
    }
}

const KEY: [u8; 32] = [9; 32];

/// A save that failed could not stop anything: the message left, the inbox was acknowledged, with nothing saved.
/// Now the call fails, and nothing that needed the save leaves.
#[test]
fn a_save_that_fails_stops_the_send_and_the_ack() {
    let hub = TestHub::start();
    let (mut a, mut b) = pair(&hub, "Disk");
    let disk = Arc::new(Disk::default());
    a.persist_to(disk.clone(), KEY).unwrap();
    a.send(b.user_id(), text("one")).unwrap();
    assert_eq!(texts(&b.sync(0).unwrap()), ["one"]);
    disk.full.store(true, Ordering::Relaxed);
    assert!(matches!(a.send(b.user_id(), text("not saved")), Err(Error::Save(_))));
    assert_eq!(a.pending(), 0, "not made");
    assert!(texts(&b.sync(0).unwrap()).is_empty(), "nothing left");
    disk.full.store(false, Ordering::Relaxed);
    a.send(b.user_id(), text("two")).unwrap();
    assert_eq!(texts(&b.sync(0).unwrap()), ["two"]);

    // Receiving: not saved, not acknowledged; the hub still has it for the state on disk.
    b.send(a.user_id(), text("for a")).unwrap();
    disk.full.store(true, Ordering::Relaxed);
    assert!(matches!(a.maintenance(), Err(Error::Save(_))));
    disk.full.store(false, Ordering::Relaxed);
    let blob = disk.saved.lock().unwrap().clone();
    let mut again = Client::restore(Http::new(hub.url()), &blob, &KEY).unwrap();
    assert_eq!(texts(&again.sync(0).unwrap()), ["for a"], "still at the hub");
}
