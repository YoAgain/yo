//! What a review of the core found (Codex, after the chaos runs), each the smallest case that showed it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use yo_core::{Client, Content, Error, Event, Push, Store};
use yo_tests::{random_u64, Fault, Http, TestHub};

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

/// A message made offline to someone then blocked still left with the next delivery. Now what waits for a blocked
/// user stays (not counted as waiting) and goes only after an unblock.
#[test]
fn nothing_waiting_goes_to_someone_blocked_meanwhile() {
    let hub = TestHub::start();
    let n = random_u64() % 100_000;
    let http = Http::new(hub.url());
    let mut a = by_name(http.clone(), &format!("Shy{n}"));
    let mut b = by_name(Http::new(hub.url()), &format!("Yhs{n}"));
    a.send(b.user_id(), text("hello")).unwrap();
    assert_eq!(texts(&b.sync(0).unwrap()), ["hello"]);
    let to_b = yo_wire::api::deliver_path(&yo_tests::user_id(b.user_id()));
    http.fault(&to_b, Fault::Answer(503));
    a.send(b.user_id(), text("made offline")).unwrap();
    assert_eq!(a.pending(), 1);
    a.set_blocked(b.user_id(), true).unwrap();
    assert_eq!(a.pending(), 0, "held, not waiting");
    a.sync(0).unwrap();
    a.maintenance().unwrap();
    assert!(texts(&b.sync(0).unwrap()).is_empty(), "nothing went to b");
    a.set_blocked(b.user_id(), false).unwrap();
    a.sync(0).unwrap();
    assert_eq!(texts(&b.sync(0).unwrap()), ["made offline"], "after the unblock, in order");
    a.send(b.user_id(), text("and on")).unwrap();
    assert_eq!(texts(&b.sync(0).unwrap()), ["and on"]);
}

fn to(user: &str) -> String {
    yo_wire::api::deliver_path(&yo_tests::user_id(user))
}

/// A name a contact gave up and someone else took: a send to the name still went to the contact, as this device
/// had learnt it. Now a name is asked of the hub, and the old contact loses it.
#[test]
fn a_name_that_changed_hands_goes_to_its_new_owner() {
    let hub = TestHub::start();
    let n = random_u64() % 100_000;
    let name = format!("Taken{n}");
    let mut a = by_name(Http::new(hub.url()), &name);
    let mut b = by_name(Http::new(hub.url()), &format!("Sender{n}"));
    b.send(&name, text("hi a")).unwrap();
    assert_eq!(texts(&a.sync(0).unwrap()), ["hi a"]);
    a.set_username(Some(&format!("Moved{n}"))).unwrap();
    let mut c = by_name(Http::new(hub.url()), &name);
    b.send(&name, text("hi whoever")).unwrap();
    assert_eq!(texts(&c.sync(0).unwrap()), ["hi whoever"]);
    assert!(texts(&a.sync(0).unwrap()).is_empty());
    let known = b.contacts();
    assert!(known.iter().any(|k| k.user == c.user_id() && k.handles == [name.clone()]));
    assert!(known.iter().any(|k| k.user == a.user_id() && k.handles.is_empty()), "{known:?}");
}

/// A `409` whose devices could not be read dropped the message, though the hub had not taken it. Now it stays in
/// the outbox and goes with the next delivery.
#[test]
fn an_unreadable_409_keeps_the_message() {
    let hub = TestHub::start();
    let n = random_u64() % 100_000;
    let http = Http::new(hub.url());
    let mut a = by_name(http.clone(), &format!("Odd{n}"));
    let mut b = by_name(Http::new(hub.url()), &format!("Ddo{n}"));
    a.send(b.user_id(), text("hi")).unwrap();
    assert_eq!(texts(&b.sync(0).unwrap()), ["hi"]);
    http.fault(&to(b.user_id()), Fault::Answer(409));
    a.send(b.user_id(), text("kept")).unwrap();
    assert_eq!((a.pending(), a.dropped()), (1, 0));
    a.sync(0).unwrap();
    assert_eq!(texts(&b.sync(0).unwrap()), ["kept"]);
}

/// The account was registered before its secrets were saved: a crash after the hub took the name lost the key the
/// name was registered with. Now the device and its request are saved before the request leaves, and a restart
/// finishes the registration from the state, the same device, whether the hub had taken it or not.
#[test]
fn a_registration_is_saved_before_it_leaves_and_finished_after_a_restart() {
    let hub = TestHub::start();
    let n = random_u64() % 100_000;
    // Taken by the hub, its answer lost, and the process gone before it learnt so (the question fails too).
    let taken = format!("Crash{n}");
    let http = Http::new(hub.url());
    http.fault("/v1/register", Fault::LoseAnswer);
    http.fault("/v1/lookup", Fault::Answer(503));
    let disk = Arc::new(Disk::default());
    let gone = Client::register_username_saved(http, disk.clone(), KEY, &taken, None, None, Push::none());
    assert_eq!(gone.err(), Some(Error::Refused(503)));
    // Never at the hub: the same device registers when started again.
    let lost = format!("Never{n}");
    let http = Http::new(hub.url());
    http.fault("/v1/register", Fault::Answer(503));
    let other_disk = Arc::new(Disk::default());
    assert!(Client::register_username_saved(http, other_disk.clone(), KEY, &lost, None, None, Push::none()).is_err());
    let mut sender = by_name(Http::new(hub.url()), &format!("Hello{n}"));
    for (disk, name) in [(disk, &taken), (other_disk, &lost)] {
        let blob = disk.saved.lock().unwrap().clone();
        let mut me = Client::restore(Http::new(hub.url()), &blob, &KEY).unwrap();
        assert!(me.is_registering());
        me.persist_to(disk.clone(), KEY).unwrap();
        me.finish_registration().unwrap();
        assert!(!me.is_registering());
        assert_eq!(me.username(), Some(name.as_str()));
        sender.send(name, text("found you")).unwrap();
        assert_eq!(texts(&me.sync(0).unwrap()), ["found you"], "{name}");
        let blob = disk.saved.lock().unwrap().clone();
        assert!(!Client::restore(Http::new(hub.url()), &blob, &KEY).unwrap().is_registering(), "saved");
    }
    // A save that fails: nothing leaves, the name stays free.
    let full = Arc::new(Disk::default());
    full.full.store(true, Ordering::Relaxed);
    let free = format!("Free{n}");
    let http = Http::new(hub.url());
    let refused = Client::register_username_saved(http.clone(), full, KEY, &free, None, None, Push::none());
    assert!(matches!(refused, Err(Error::Save(_))));
    assert!(Client::username_free(http.as_ref(), &free).unwrap());
}

// The final review (Codex, after the app's version and the saves that fail).
