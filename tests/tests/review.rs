//! What a review of the core found (Codex, after the chaos runs), each the smallest case that showed it.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

/// A clock that can be moved on, for updates that are due.
fn movable_clock(client: &mut Client) -> Arc<AtomicU64> {
    let offset = Arc::new(AtomicU64::new(0));
    let moved = offset.clone();
    client.set_clock(Arc::new(move || {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
        now + moved.load(Ordering::Relaxed)
    }));
    offset
}

const DAYS_8: u64 = 8 * 24 * 3600 * 1000;

fn to(user: &str) -> String {
    yo_wire::api::deliver_path(&yo_tests::user_id(user))
}

/// A commit the hub refused (`403`, `404`) turned the chat's updates off for good: no more post-compromise
/// security, silently. Now the chat is lost and restarted (spec 4.9), and the new one takes updates.
#[test]
fn a_refused_commit_restarts_the_chat_instead_of_giving_up_updates() {
    let hub = TestHub::start();
    let n = random_u64() % 100_000;
    let http = Http::new(hub.url());
    let mut a = by_name(Http::new(hub.url()), &format!("Ref{n}"));
    let mut b = by_name(http.clone(), &format!("Fer{n}"));
    a.send(b.user_id(), text("hi")).unwrap();
    assert_eq!(texts(&b.sync(0).unwrap()), ["hi"]);
    let joined = b.chats_with(a.user_id())[0].group.clone();
    http.fault("/commit", Fault::Answer(403));
    b.send(a.user_id(), text("after a refusal")).unwrap();
    assert_eq!(texts(&a.sync(0).unwrap()), ["after a refusal"]);
    let chats = b.chats_with(a.user_id());
    assert_eq!(chats.len(), 1);
    assert_ne!(chats[0].group, joined, "a new group");
    assert!(chats[0].updates && !chats[0].lost);
    let epoch = chats[0].epoch;
    let clock = movable_clock(&mut b);
    clock.store(DAYS_8, Ordering::Relaxed);
    b.maintenance().unwrap();
    assert_eq!(b.chats_with(a.user_id())[0].epoch, epoch + 1, "updated");
    a.send(b.user_id(), text("still fine")).unwrap();
    assert_eq!(texts(&b.sync(0).unwrap()), ["still fine"]);
}

/// More commits missing than one call fetches: the error to come back later rolled back everything fetched, so
/// every call fetched the same ones again, for ever. Now what was fetched stays and the next call goes on.
#[test]
fn a_long_way_behind_is_caught_up_over_several_calls() {
    let hub = TestHub::start();
    let n = random_u64() % 100_000;
    let http = Http::new(hub.url());
    let mut a = by_name(http.clone(), &format!("Far{n}"));
    let mut b = by_name(Http::new(hub.url()), &format!("Raf{n}"));
    a.send(b.user_id(), text("hi")).unwrap();
    assert_eq!(texts(&b.sync(0).unwrap()), ["hi"]);
    b.fetch_at_most_for_tests(2);
    let clock = movable_clock(&mut a);
    for k in 1..=5 {
        clock.store(k * DAYS_8, Ordering::Relaxed);
        // The commit never reaches b's inbox: b fetches it from the hub.
        http.fault(&to(b.user_id()), Fault::Answer(400));
        a.maintenance().unwrap();
    }
    a.send(b.user_id(), text("far ahead")).unwrap();
    let mut epochs = Vec::new();
    let got = (0..5).find_map(|_| {
        let synced = b.sync(0);
        epochs.push(b.chats()[0].2);
        synced.ok().map(|events| texts(&events))
    });
    assert_eq!(got.unwrap(), ["far ahead"], "epochs on the way: {epochs:?}");
    assert_eq!(epochs, [3, 5, 6], "two at a time, kept");
}

/// A lost race whose catch-up met a passing trouble counted as done: the message was encrypted in an epoch the
/// hub's group had left. Now the send fails for now, and nothing is made in the old epoch.
#[test]
fn nothing_is_encrypted_in_an_epoch_already_left() {
    let hub = TestHub::start();
    let n = random_u64() % 100_000;
    let (http_a, http_b) = (Http::new(hub.url()), Http::new(hub.url()));
    let mut a = by_name(http_a.clone(), &format!("Old{n}"));
    let mut b = by_name(http_b.clone(), &format!("Dlo{n}"));
    a.send(b.user_id(), text("hi")).unwrap();
    assert_eq!(texts(&b.sync(0).unwrap()), ["hi"]);
    // a's update wins the epoch b's first update (due since it joined) will want; its commit does not reach b.
    let clock = movable_clock(&mut a);
    clock.store(DAYS_8, Ordering::Relaxed);
    http_a.fault(&to(b.user_id()), Fault::Answer(400));
    a.maintenance().unwrap();
    // b's receive first, then its catch-up after the lost race: both "not now".
    http_b.fault("/v1/inbox?", Fault::Answer(503));
    http_b.fault("/v1/inbox?", Fault::Answer(503));
    let sent = b.send(a.user_id(), text("not now"));
    assert_eq!(sent, Err(Error::Refused(503)), "for now");
    assert_eq!(b.pending(), 0, "not made in the old epoch");
    b.send(a.user_id(), text("now")).unwrap();
    assert_eq!(texts(&a.sync(0).unwrap()), ["now"]);
    assert_eq!(a.chats()[0].2, b.chats()[0].2, "one epoch");
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

/// A restart's `last_seen` is the latest message the peer got, not all it got: one lost before a later one that came
/// was never sent again. Now every message kept is, and the peer skips what it has seen.
#[test]
fn a_restart_brings_back_a_message_lost_before_a_later_one() {
    let hub = TestHub::start();
    let n = random_u64() % 100_000;
    let (http_a, http_b) = (Http::new(hub.url()), Http::new(hub.url()));
    let mut a = by_name(http_a.clone(), &format!("Gap{n}"));
    let mut b = by_name(http_b.clone(), &format!("Pag{n}"));
    a.send(b.user_id(), text("first")).unwrap();
    assert_eq!(texts(&b.sync(0).unwrap()), ["first"]);
    http_a.fault(&to(b.user_id()), Fault::Answer(400));
    a.send(b.user_id(), text("lost")).unwrap();
    a.send(b.user_id(), text("later")).unwrap();
    assert_eq!(texts(&b.sync(0).unwrap()), ["later"]);
    // b's chat is lost (the hub refuses its first update) and restarted: "later" is the last it saw.
    http_b.fault("/commit", Fault::Answer(403));
    b.send(a.user_id(), text("restarted")).unwrap();
    assert_eq!(texts(&a.sync(0).unwrap()), ["restarted"]);
    assert_eq!(texts(&b.sync(0).unwrap()), ["lost"], "the gap, once");
    a.send(b.user_id(), text("on")).unwrap();
    assert_eq!(texts(&b.sync(0).unwrap()), ["on"]);
}

/// A new last resort retired the one before when it was made, not when the hub took it: an upload that failed
/// and went a month later deleted the old one's keys at once, though the hub had handed it out until then.
#[test]
fn a_last_resort_is_retired_only_once_its_replacement_is_at_the_hub() {
    const DAY: u64 = 24 * 3600 * 1000;
    let hub = TestHub::start();
    let http = Http::new(hub.url());
    let mut a = by_name(http.clone(), &format!("Last{}", random_u64() % 100_000));
    let clock = movable_clock(&mut a);
    clock.store(31 * DAY, Ordering::Relaxed);
    http.fault("/v1/keypackages", Fault::Answer(503));
    assert_eq!(a.maintenance(), Err(Error::Refused(503)));
    let kept = a.own_key_packages();
    clock.store(63 * DAY, Ordering::Relaxed);
    a.maintenance().unwrap();
    assert_eq!(a.own_key_packages(), kept + 1, "the one the hub had until now is kept");
}
