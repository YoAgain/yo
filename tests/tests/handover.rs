//! Events handed over before the hub hears of them (spec 4.7): `sync` returns once what arrived is saved; the hub's
//! ack and the work after it (KeyPackages, the outbox, the storage, delivery tokens) come with `after_sync` or the
//! next `sync`. A crash in between loses nothing and shows nothing twice; a guest only reads and acknowledges.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use yo_core::{Client, Content, Error, Event, Store};
use yo_tests::{random_u64, register, user_id, username, Fault, Http, TestHub};
use yo_wire::api::deliver_path;

const KEY: [u8; 32] = [3; 32];

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

impl Disk {
    fn blob(&self) -> Vec<u8> {
        self.saved.lock().unwrap().clone()
    }
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

/// The requests sent since the last look, as "METHOD /path" without the query.
fn requests(http: &Http) -> Vec<String> {
    let sent = std::mem::take(&mut *http.sent.lock().unwrap());
    sent.into_iter().map(|(method, path, _)| format!("{method} {}", path.split('?').next().unwrap_or(""))).collect()
}

fn acked(requests: &[String]) -> bool {
    requests.iter().any(|r| r == "POST /v1/inbox/ack")
}

/// A and B with a chat in use; B saved to a disk, acknowledging each batch itself as the app does.
fn chat(hub: &TestHub) -> (Client, Client, Arc<Http>, Arc<Disk>) {
    let n = random_u64() % 1_000_000_000;
    let (pa, pb) = (username(n), username(n + 1));
    let hb = Http::new(hub.url());
    let mut a = register(Http::new(hub.url()), &pa, None);
    let mut b = register(hb.clone(), &pb, None);
    a.send_yo(&pb).unwrap();
    b.sync(0).unwrap();
    b.send_yo(&pa).unwrap();
    a.sync(0).unwrap();
    b.after_sync().unwrap();
    let disk = Arc::new(Disk::default());
    b.persist_to(disk.clone(), KEY).unwrap();
    b.acknowledge_explicitly();
    (a, b, hb, disk)
}

fn restore(hub: &TestHub, disk: &Arc<Disk>) -> (Client, Arc<Http>) {
    let http = Http::new(hub.url());
    let mut client = Client::restore(http.clone(), &disk.blob(), &KEY).unwrap();
    client.persist_to(disk.clone(), KEY).unwrap();
    client.acknowledge_explicitly();
    (client, http)
}

#[test]
fn a_crash_between_the_hand_over_and_the_ack_loses_nothing_and_shows_nothing_twice() {
    let hub = TestHub::start();
    let (mut a, mut b, hb, disk) = chat(&hub);
    for t in ["one", "two", "three"] {
        a.send(b.user_id(), text(t)).unwrap();
    }
    requests(&hb);
    // Handed over before the hub is told: no ack on the way.
    assert_eq!(texts(&b.sync(0).unwrap()), ["one", "two", "three"]);
    assert!(!acked(&requests(&hb)));
    // Killed before the app stored them: they come again, once, in order; the hub's copies are skipped as seen.
    drop(b);
    let (mut again, http) = restore(&hub, &disk);
    assert_eq!(texts(&again.sync(0).unwrap()), ["one", "two", "three"]);
    // The app stores them, and is killed before the hub heard of them: the hub repeats, nothing shows twice.
    again.acknowledge_only(|_| true);
    assert!(!acked(&requests(&http)));
    drop(again);
    let (mut again, http) = restore(&hub, &disk);
    assert!(texts(&again.sync(0).unwrap()).is_empty());
    again.after_sync().unwrap();
    assert!(acked(&requests(&http)));
    // Acknowledged now: the next start reads an empty inbox.
    drop(again);
    let (mut b, http) = restore(&hub, &disk);
    assert!(texts(&b.sync(0).unwrap()).is_empty());
    b.after_sync().unwrap();
    assert!(!acked(&requests(&http)), "nothing left to acknowledge");
    a.send(b.user_id(), text("four")).unwrap();
    assert_eq!(texts(&b.sync(0).unwrap()), ["four"]);
    assert_eq!(b.dropped(), 0, "{:?}", b.drop_reasons());
}

#[test]
fn the_next_sync_acknowledges_what_the_last_one_handed_over() {
    let hub = TestHub::start();
    let (mut a, mut b, hb, _) = chat(&hub);
    a.send(b.user_id(), text("one")).unwrap();
    let first = b.sync(0).unwrap();
    assert_eq!(texts(&first), ["one"]);
    requests(&hb);
    a.send(b.user_id(), text("two")).unwrap();
    // A batch the app has not acknowledged yet stays handed; the hub is told before the next wait.
    let second = b.sync(0).unwrap();
    assert_eq!(texts(&second), ["two"]);
    let sent = requests(&hb);
    let ack = sent.iter().position(|r| r == "POST /v1/inbox/ack").expect("acknowledged");
    assert!(sent[..ack].iter().all(|r| r != "GET /v1/inbox"), "before the inbox is read again: {sent:?}");
    // Both batches still handed: a crash brings both, in order.
    let blob = b.export(&KEY);
    let mut again = Client::restore(Http::new(hub.url()), &blob, &KEY).unwrap();
    assert_eq!(texts(&again.sync(0).unwrap()), ["one", "two"]);
    b.acknowledge_only(|e| first.contains(e));
    let blob = b.export(&KEY);
    let mut again = Client::restore(Http::new(hub.url()), &blob, &KEY).unwrap();
    assert_eq!(texts(&again.sync(0).unwrap()), ["two"]);
}

#[test]
fn what_was_not_saved_is_not_acknowledged() {
    let hub = TestHub::start();
    let (mut a, mut b, hb, disk) = chat(&hub);
    a.send(b.user_id(), text("not saved")).unwrap();
    requests(&hb);
    let wait = b.sync_begin(0).unwrap();
    disk.full.store(true, Ordering::Relaxed);
    let answer = wait.send();
    assert!(matches!(b.sync_end(&wait, answer), Err(Error::Save(_))));
    assert!(matches!(b.after_sync(), Err(Error::Save(_))));
    assert!(!acked(&requests(&hb)));
    // The state on disk is from before: the hub still has it.
    disk.full.store(false, Ordering::Relaxed);
    drop(b);
    let (mut b, _) = restore(&hub, &disk);
    assert_eq!(texts(&b.sync(0).unwrap()), ["not saved"]);
}

#[test]
fn the_work_after_a_sync_waits_until_its_events_are_out() {
    let hub = TestHub::start();
    let (mut a, mut b, hb, _) = chat(&hub);
    // A friend changed and a message waiting in the outbox (no network for it a moment ago).
    b.set_friend(a.user_id(), Some("Al".into()), None).unwrap();
    hb.fault(&deliver_path(&user_id(a.user_id())), Fault::Answer(503));
    b.send(a.user_id(), text("waited")).unwrap();
    assert_eq!(b.pending(), 1);
    a.send(b.user_id(), text("one")).unwrap();
    // The first sync's wait goes out after the outbox, as ever; its answer comes back at once with the event.
    b.sync_begin(0).unwrap();
    assert_eq!(b.pending(), 0);
    requests(&hb);
    a.send(b.user_id(), text("two")).unwrap();
    b.set_friend(a.user_id(), Some("Alex".into()), None).unwrap();
    assert_eq!(texts(&b.sync(0).unwrap()), ["one", "two"]);
    let sent = requests(&hb);
    assert!(sent.iter().all(|r| r == "GET /v1/inbox"), "{sent:?}");
    // Then the rest: the ack, the storage.
    b.after_sync().unwrap();
    let sent = requests(&hb);
    assert!(acked(&sent), "{sent:?}");
    assert!(sent.iter().any(|r| r == "PUT /v1/storage"), "{sent:?}");
    // A change written at once, beside the wait.
    b.set_friend(a.user_id(), Some("Alexa".into()), None).unwrap();
    b.write_storage().unwrap();
    assert_eq!(requests(&hb), ["PUT /v1/storage"]);
    assert!(b.storage_saved_at().is_some());
}

#[test]
fn a_guest_only_reads_and_acknowledges() {
    let hub = TestHub::start();
    let (mut a, mut b, hb, disk) = chat(&hub);
    // The app has work waiting: a friend changed, a message in the outbox.
    b.set_friend(a.user_id(), Some("Al".into()), None).unwrap();
    hb.fault(&deliver_path(&user_id(a.user_id())), Fault::Answer(503));
    b.send(a.user_id(), text("waited")).unwrap();
    assert_eq!(b.pending(), 1);
    a.send(b.user_id(), text("for the guest")).unwrap();
    let guest_http = Http::new(hub.url());
    let mut guest = Client::restore_guest(guest_http.clone(), &disk.blob(), &KEY).unwrap();
    guest.persist_to(disk.clone(), KEY).unwrap();
    assert_eq!(texts(&guest.sync(0).unwrap()), ["for the guest"]);
    assert_eq!(requests(&guest_http), ["GET /v1/inbox"]);
    guest.after_sync().unwrap();
    assert_eq!(requests(&guest_http), ["POST /v1/inbox/ack"]);
    assert_eq!(guest.pending(), 1, "the outbox is the app's");
    drop(guest);
    // The app takes the event in as ever, and does its work.
    b.reload(&disk.blob()).unwrap();
    assert_eq!(texts(&b.sync(0).unwrap()), ["for the guest"]);
    b.after_sync().unwrap();
    assert_eq!(b.pending(), 0);
    assert!(b.storage_saved_at().is_some());
    assert_eq!(texts(&a.sync(0).unwrap()), ["waited"]);
}
