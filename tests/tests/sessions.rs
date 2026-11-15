//! Sessions (spec 4): what the Double Ratchet and Yo's retry and receipts make of lost, duplicated and reordered
//! messages, a session out of step, and a month away.

use std::sync::{Arc, Mutex};
use yo_core::{Client, Content, Event, Push, Store};
use yo_tests::{deliver_raw, random_u64, Fault, Http, TestHub};

fn by_name(http: Arc<Http>, name: &str) -> Client {
    Client::register_username(http, name, None, None, Push::none()).unwrap()
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

/// The envelopes A posted to B's inbox, in order, whether the hub got them or not.
fn posted(http: &Http, to: &str) -> Vec<Vec<u8>> {
    let path = yo_wire::api::deliver_path(&yo_tests::user_id(to));
    http.sent.lock().unwrap().iter().filter(|(m, p, _)| m == "POST" && *p == path).map(|(_, _, b)| b.clone()).collect()
}

fn pair(hub: &TestHub, tag: &str) -> (Client, Client, Arc<Http>) {
    let n = random_u64() % 100_000;
    let ha = Http::new(hub.url());
    let mut a = by_name(ha.clone(), &format!("A{tag}{n}"));
    let mut b = by_name(Http::new(hub.url()), &format!("B{tag}{n}"));
    // Sessions both ways: A writes first, B answers.
    a.send(b.user_id(), text("hi")).unwrap();
    assert_eq!(texts(&b.sync(0).unwrap()), ["hi"]);
    b.send(a.user_id(), text("hello")).unwrap();
    assert_eq!(texts(&a.sync(0).unwrap()), ["hello"]);
    (a, b, ha)
}

/// One message never reaches the hub, one arrives late (after the next), one twice: the ratchet takes the late one
/// with its skipped key, the copy is skipped, the lost one costs nothing but itself.
#[test]
fn lost_late_and_doubled_messages() {
    let hub = TestHub::start();
    let (mut a, mut b, ha) = pair(&hub, "Lld");
    let to_b = yo_wire::api::deliver_path(&yo_tests::user_id(b.user_id()));
    // "lost": A is told the hub took it, the hub never saw it.
    ha.fault(&to_b, Fault::Answer(202));
    a.send(b.user_id(), text("lost")).unwrap();
    // "late": the same, then posted by hand after the next one.
    ha.fault(&to_b, Fault::Answer(202));
    a.send(b.user_id(), text("late")).unwrap();
    a.send(b.user_id(), text("next")).unwrap();
    let sent = posted(&ha, b.user_id());
    let (late, next) = (sent[sent.len() - 2].clone(), sent[sent.len() - 1].clone());
    assert_eq!(texts(&b.sync(0).unwrap()), ["next"]);
    assert_eq!(deliver_raw(&hub.url(), b.user_id(), late), 202);
    assert_eq!(deliver_raw(&hub.url(), b.user_id(), next), 202);
    assert_eq!(texts(&b.sync(0).unwrap()), ["late"], "the late one once, the copy skipped");
    assert_eq!(b.dropped(), 0, "{:?}", b.drop_reasons());
    b.send(a.user_id(), text("all good")).unwrap();
    assert_eq!(texts(&a.sync(0).unwrap()), ["all good"]);
}

#[derive(Default)]
struct Saved(Mutex<Vec<u8>>);

impl Store for Saved {
    fn save(&self, blob: Vec<u8>) -> Result<(), String> {
        *self.0.lock().unwrap() = blob;
        Ok(())
    }
}

/// B's session with A is gone (its state from before A wrote, brought back): A's next message does not open. B
/// starts a new session with A's device and sends a retry in it; A sends again what it keeps, and it arrives.
#[test]
fn a_session_out_of_step_heals_by_a_retry() {
    let hub = TestHub::start();
    let n = random_u64() % 100_000;
    let mut a = by_name(Http::new(hub.url()), &format!("Ret{n}"));
    let mut b = by_name(Http::new(hub.url()), &format!("Ter{n}"));
    let early = b.export(&[3; 32]);
    a.send(b.user_id(), text("1")).unwrap();
    assert_eq!(texts(&b.sync(0).unwrap()), ["1"]);
    b.send(a.user_id(), text("r")).unwrap();
    assert_eq!(texts(&a.sync(0).unwrap()), ["r"]);
    // B's state from before A wrote: it has no session with A.
    let mut b = Client::restore(Http::new(hub.url()), &early, &[3; 32]).unwrap();
    b.persist_to(Arc::new(Saved::default()), [3; 32]).unwrap();
    a.send(b.user_id(), text("2")).unwrap();
    assert!(texts(&b.sync(0).unwrap()).is_empty(), "it does not open");
    assert_eq!(b.dropped(), 1);
    b.after_sync().unwrap();
    a.sync(0).unwrap();
    a.after_sync().unwrap();
    let again = texts(&b.sync(0).unwrap());
    assert!(again.contains(&"2".to_owned()), "{again:?}");
    b.send(a.user_id(), text("back")).unwrap();
    assert_eq!(texts(&a.sync(0).unwrap()), ["back"]);
    assert_eq!(a.dropped(), 0, "{:?}", a.drop_reasons());
}

/// A writes first while B is away for four weeks, B's signed prekey and last resort replaced each week meanwhile:
/// the first message, held at the hub all along, still opens (a replaced prekey's private key is kept a month), and
/// the sessions work both ways.
#[test]
fn a_first_message_opens_after_four_weeks_away() {
    let hub = TestHub::start();
    let n = random_u64() % 100_000;
    let ha = Http::new(hub.url());
    let mut a = by_name(ha.clone(), &format!("Awa{n}"));
    let mut b = by_name(Http::new(hub.url()), &format!("Way{n}"));
    // Held back from the hub until B's weeks are over (posted by hand then), as if it waited in the inbox.
    ha.fault(&yo_wire::api::deliver_path(&yo_tests::user_id(b.user_id())), Fault::Answer(202));
    a.send(b.user_id(), text("waiting")).unwrap();
    let first = posted(&ha, b.user_id()).pop().unwrap();
    let start = yo_hub::now_ms();
    for week in 1..=4u64 {
        let at = start + week * 8 * 24 * 3600 * 1000;
        b.set_clock(Arc::new(move || at));
        b.maintenance().unwrap();
    }
    assert_eq!(deliver_raw(&hub.url(), b.user_id(), first), 202);
    assert_eq!(texts(&b.sync(0).unwrap()), ["waiting"]);
    b.send(a.user_id(), text("back")).unwrap();
    assert_eq!(texts(&a.sync(0).unwrap()), ["back"]);
    assert_eq!(a.dropped() + b.dropped(), 0, "{:?} {:?}", a.drop_reasons(), b.drop_reasons());
}

/// A payload anyone could seal, claiming a contact's keys under another device number with a message that does not
/// open: dropped, and nothing of it taken in: the real session goes on.
#[test]
fn a_forged_payload_changes_nothing() {
    let hub = TestHub::start();
    let (mut a, mut b, ha) = pair(&hub, "Fog");
    let real = posted(&ha, b.user_id()).pop().unwrap();
    let mut payload: yo_wire::envelope::Payload =
        yo_wire::decode(&yo_core::seal::open(&b.account_secret(), &real).unwrap()).unwrap();
    payload.device_id = 7;
    let mut message = payload.copies[0].message.as_slice().to_vec();
    let last = message.len() - 1;
    message[last] ^= 1;
    payload.copies[0].message = message.into();
    let account = ed25519_dalek::SigningKey::from_bytes(&b.account_secret()).verifying_key().to_bytes();
    let forged = yo_core::seal::to(&account, &yo_wire::encode(&payload)).unwrap();
    assert_eq!(deliver_raw(&hub.url(), b.user_id(), forged), 202);
    assert!(texts(&b.sync(0).unwrap()).is_empty());
    assert_eq!(b.dropped(), 1);
    assert!(
        b.sessions_with(a.user_id()).iter().all(|s| s.usable && !s.retrying),
        "no retry for a device under another number"
    );
    a.send(b.user_id(), text("still here")).unwrap();
    assert_eq!(texts(&b.sync(0).unwrap()), ["still here"]);
}

/// A's state goes back (its file from before its last Yo, the Yo gone out all the same): its next Yo is made with
/// message keys B used already, which libsignal calls a duplicate. It is none (its bytes were not seen): a session out
/// of step, healed by a retry like any; B shows the new Yo once, not the old one again.
#[test]
fn a_sender_whose_state_went_back_is_asked_again() {
    let hub = TestHub::start();
    let (mut a, mut b, _) = pair(&hub, "Back");
    let disk = Arc::new(Saved::default());
    a.persist_to(disk.clone(), [3; 32]).unwrap();
    let before = disk.0.lock().unwrap().clone();
    a.send(b.user_id(), text("1")).unwrap();
    assert_eq!(texts(&b.sync(0).unwrap()), ["1"]);
    let mut a = Client::restore(Http::new(hub.url()), &before, &[3; 32]).unwrap();
    a.persist_to(disk, [3; 32]).unwrap();
    a.send(b.user_id(), text("2")).unwrap();
    assert!(texts(&b.sync(0).unwrap()).is_empty(), "made with keys used before: it does not open");
    assert!(b.sessions_with(a.user_id()).iter().any(|s| s.retrying), "a retry is due");
    b.after_sync().unwrap();
    a.sync(0).unwrap();
    a.after_sync().unwrap();
    assert_eq!(texts(&b.sync(0).unwrap()), ["2"], "sent again, the one seen skipped");
    b.send(a.user_id(), text("back")).unwrap();
    assert_eq!(texts(&a.sync(0).unwrap()), ["back"]);
}

/// A contact's real message re-sealed with another device key under its number (by whoever has its bytes, the hub
/// say): the hub's roster has another device at that number, so it is dropped unopened and nothing of it is taken in;
/// the session goes on.
#[test]
fn a_device_key_the_hub_does_not_name_is_not_taken_in() {
    let hub = TestHub::start();
    let (mut a, mut b, ha) = pair(&hub, "Dkn");
    let to_b = yo_wire::api::deliver_path(&yo_tests::user_id(b.user_id()));
    ha.fault(&to_b, Fault::Answer(202));
    a.send(b.user_id(), text("held")).unwrap();
    let real = posted(&ha, b.user_id()).pop().unwrap();
    let mut payload: yo_wire::envelope::Payload =
        yo_wire::decode(&yo_core::seal::open(&b.account_secret(), &real).unwrap()).unwrap();
    payload.device_key = [9; 32];
    let account = ed25519_dalek::SigningKey::from_bytes(&b.account_secret()).verifying_key().to_bytes();
    let forged = yo_core::seal::to(&account, &yo_wire::encode(&payload)).unwrap();
    assert_eq!(deliver_raw(&hub.url(), b.user_id(), forged), 202);
    // Another key at A's number: the hub's roster says it is not A's device there, so it is not even opened.
    assert!(texts(&b.sync(0).unwrap()).is_empty());
    let sessions = b.sessions_with(a.user_id());
    assert!(sessions.len() == 1 && sessions[0].device != "09".repeat(32), "the made-up key is in no roster");
    a.send(b.user_id(), text("next")).unwrap();
    assert_eq!(texts(&b.sync(0).unwrap()), ["next"]);
}
