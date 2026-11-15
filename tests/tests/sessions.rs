//! Sessions (spec 4): what the Double Ratchet and Yo's retry and receipts make of lost, duplicated and reordered
//! messages, a session out of step, and a month away.

use std::sync::{Arc, Mutex};
use yo_core::{Client, Content, Event, Push, Store};
use yo_tests::{deliver_raw_with, random_u64, Fault, Http, TestHub};

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
    assert_eq!(deliver_raw_with(&hub.url(), b.user_id(), b.contact_key_for_tests().as_deref(), late), 202);
    assert_eq!(deliver_raw_with(&hub.url(), b.user_id(), b.contact_key_for_tests().as_deref(), next), 202);
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
    assert_eq!(deliver_raw_with(&hub.url(), b.user_id(), b.contact_key_for_tests().as_deref(), first), 202);
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
    assert_eq!(deliver_raw_with(&hub.url(), b.user_id(), b.contact_key_for_tests().as_deref(), forged), 202);
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
    assert_eq!(deliver_raw_with(&hub.url(), b.user_id(), b.contact_key_for_tests().as_deref(), forged), 202);
    // Another key at A's number: the hub's roster says it is not A's device there, so it is not even opened.
    assert!(texts(&b.sync(0).unwrap()).is_empty());
    let sessions = b.sessions_with(a.user_id());
    assert!(sessions.len() == 1 && sessions[0].device != "09".repeat(32), "the made-up key is in no roster");
    a.send(b.user_id(), text("next")).unwrap();
    assert_eq!(texts(&b.sync(0).unwrap()), ["next"]);
}

/// A payload naming a real account this device does not know (someone could write first) with a message that does not
/// open: dropped, and that account is not taken in as a contact; only a message that opens pins a first sender.
#[test]
fn a_stranger_named_by_a_payload_that_does_not_open_is_not_pinned() {
    let hub = TestHub::start();
    let (_a, mut b, ha) = pair(&hub, "Pin");
    let c = yo_tests::register(Http::new(hub.url()), &yo_tests::username(random_u64() % 1_000_000_000), None);
    let real = posted(&ha, b.user_id()).pop().unwrap();
    let mut payload: yo_wire::envelope::Payload =
        yo_wire::decode(&yo_core::seal::open(&b.account_secret(), &real).unwrap()).unwrap();
    payload.account_key = ed25519_dalek::SigningKey::from_bytes(&c.account_secret()).verifying_key().to_bytes();
    let mut message = payload.copies[0].message.as_slice().to_vec();
    let last = message.len() - 1;
    message[last] ^= 1;
    payload.copies[0].message = message.into();
    let account = ed25519_dalek::SigningKey::from_bytes(&b.account_secret()).verifying_key().to_bytes();
    let forged = yo_core::seal::to(&account, &yo_wire::encode(&payload)).unwrap();
    assert_eq!(deliver_raw_with(&hub.url(), b.user_id(), b.contact_key_for_tests().as_deref(), forged), 202);
    assert!(texts(&b.sync(0).unwrap()).is_empty());
    assert!(b.contacts().iter().all(|k| k.user != c.user_id()), "not pinned by a message that did not open");
}

/// Copies that do not open, claiming a contact's real device, once a minute all day (anyone can seal them): the
/// retries' lookups stay within the day's budget for recovery, the rest of the hub's allowance is left for real
/// sessions (6.6).
#[test]
fn forgeries_cannot_use_up_the_lookups() {
    use std::sync::atomic::{AtomicU64, Ordering};
    let hub = TestHub::start();
    let n = random_u64() % 100_000;
    let (ha, hb) = (Http::new(hub.url()), Http::new(hub.url()));
    let mut a = by_name(ha.clone(), &format!("Ab{n}"));
    let mut b = by_name(hb.clone(), &format!("Bb{n}"));
    a.send(b.user_id(), text("hi")).unwrap();
    assert_eq!(texts(&b.sync(0).unwrap()), ["hi"]);
    let real = posted(&ha, b.user_id()).pop().unwrap();
    let mut payload: yo_wire::envelope::Payload =
        yo_wire::decode(&yo_core::seal::open(&b.account_secret(), &real).unwrap()).unwrap();
    let account = ed25519_dalek::SigningKey::from_bytes(&b.account_secret()).verifying_key().to_bytes();
    let clock = Arc::new(AtomicU64::new(
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64,
    ));
    let read = clock.clone();
    b.set_clock(Arc::new(move || read.load(Ordering::Relaxed)));
    let lookups =
        || hb.sent.lock().unwrap().iter().filter(|(m, p, _)| m == "POST" && p == yo_wire::api::LOOKUP).count();
    let before = lookups();
    for i in 0..30u8 {
        clock.fetch_add(61_000, Ordering::Relaxed);
        let mut message = payload.copies[0].message.as_slice().to_vec();
        let last = message.len() - 1;
        message[last] ^= i.wrapping_add(1);
        payload.copies[0].message = message.into();
        let forged = yo_core::seal::to(&account, &yo_wire::encode(&payload)).unwrap();
        assert_eq!(deliver_raw_with(&hub.url(), b.user_id(), b.contact_key_for_tests().as_deref(), forged), 202);
        b.sync(0).unwrap();
        let _ = b.after_sync();
    }
    assert!(lookups() - before <= 10, "{} lookups for 30 forgeries", lookups() - before);
}

/// A forgery makes this device ask the hub for a contact's devices; within the minute the contact's new phone writes:
/// its message names another device at the old number, the roster cannot be asked again yet; it is kept and received
/// once it can, not lost.
#[test]
fn a_new_phones_message_right_after_a_forgery_is_kept_not_lost() {
    use std::sync::atomic::{AtomicU64, Ordering};
    let hub = TestHub::start();
    let n = random_u64() % 100_000;
    let name = format!("Np{n}");
    let mut a = by_name(Http::new(hub.url()), &name);
    let mut b = by_name(Http::new(hub.url()), &format!("Nq{n}"));
    a.send(b.user_id(), text("hi")).unwrap();
    assert_eq!(texts(&b.sync(0).unwrap()), ["hi"]);
    let clock = Arc::new(AtomicU64::new(
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64,
    ));
    let read = clock.clone();
    b.set_clock(Arc::new(move || read.load(Ordering::Relaxed)));
    // A forgery first, while A's old phone is still the device at A's number: a made-up key at that number makes B ask
    // the hub's roster (which names the old phone: the forgery is dropped).
    let real = {
        let ha = Http::new(hub.url());
        let mut a3 = Client::restore(ha.clone(), &a.export(&[1; 32]), &[1; 32]).unwrap();
        // The model of the forgery is not delivered itself (its bytes would be seen, the forgery skipped unasked).
        ha.fault(&yo_wire::api::deliver_path(&yo_tests::user_id(b.user_id())), Fault::Answer(202));
        let _ = a3.send(b.user_id(), text("forged"));
        posted(&ha, b.user_id()).pop()
    };
    let real = real.expect("the forgery's model was sent");
    {
        let mut payload: yo_wire::envelope::Payload =
            yo_wire::decode(&yo_core::seal::open(&b.account_secret(), &real).unwrap()).unwrap();
        payload.device_key = [8; 32];
        let account = ed25519_dalek::SigningKey::from_bytes(&b.account_secret()).verifying_key().to_bytes();
        let forged = yo_core::seal::to(&account, &yo_wire::encode(&payload)).unwrap();
        assert_eq!(deliver_raw_with(&hub.url(), b.user_id(), b.contact_key_for_tests().as_deref(), forged), 202);
        b.sync(0).unwrap();
    }
    // Within the minute, A's new phone: the same account, a new device at A's number. Its message names a key B's
    // roster does not have, and the roster may not be asked again yet: kept, not dropped.
    let mut a2 =
        Client::register_username(Http::new(hub.url()), &name, Some(a.account_secret()), None, Push::none()).unwrap();
    a2.send(b.user_id(), text("new phone")).unwrap();
    let first = texts(&b.sync(0).unwrap());
    assert!(!first.contains(&"new phone".to_owned()), "kept for the roster, not received yet: {first:?}");
    clock.fetch_add(61_000, Ordering::Relaxed);
    let later = texts(&b.sync(0).unwrap());
    let all: Vec<String> = first.into_iter().chain(later).collect();
    assert!(all.contains(&"new phone".to_owned()), "{all:?}");
    assert_eq!(b.deferred(), 0, "decided once the roster could be asked");
}

/// First Yos to a new friend, many at once and both ways: twenty strangers write to Bea by name while Bea writes to
/// each of them first too (sessions started from both ends at the same moment), some meeting a lost answer or a 429.
/// No first send fails for good; every one arrives once.
#[test]
fn first_yos_crossing_at_once_all_arrive() {
    use std::thread;
    let hub = TestHub::start();
    let n = random_u64() % 100_000;
    let bea_name = format!("Fb{n}");
    let bea = Arc::new(Mutex::new(by_name(Http::new(hub.url()), &bea_name)));
    let strangers: Vec<(Arc<Http>, Client)> = (0..20)
        .map(|i| {
            let http = Http::new(hub.url());
            let c = by_name(http.clone(), &format!("Fs{n}x{i}"));
            (http, c)
        })
        .collect();
    let users: Vec<String> = strangers.iter().map(|(_, c)| c.user_id().to_owned()).collect();
    let bea_id = bea.lock().unwrap().user_id().to_owned();
    let to_bea = yo_wire::api::deliver_path(&yo_tests::user_id(&bea_id));
    let handles: Vec<_> = strangers
        .into_iter()
        .enumerate()
        .map(|(i, (http, mut c))| {
            let (name, to_bea) = (bea_name.clone(), to_bea.clone());
            thread::spawn(move || {
                match i % 4 {
                    1 => http.fault(&to_bea, yo_tests::Fault::LoseAnswer),
                    2 => http.fault(&to_bea, yo_tests::Fault::Answer(429)),
                    _ => {}
                }
                // What any call hands back is kept: a send reads the inbox too (its events come with it).
                let mut seen = Vec::new();
                let first = c.send(&name, text(&format!("from {i}")));
                assert!(first.is_ok() || c.waiting() > 0, "first send {i} failed for good: {first:?}");
                seen.extend(texts(&first.unwrap_or_default()));
                for _ in 0..20 {
                    if c.waiting() == 0 {
                        break;
                    }
                    seen.extend(texts(&c.sync(0).unwrap_or_default()));
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                assert_eq!(c.waiting(), 0, "first send {i} still waiting");
                (c, seen)
            })
        })
        .collect();
    let mut got = Vec::new();
    {
        let mut b = bea.lock().unwrap();
        for user in &users {
            got.extend(texts(&b.send(user, text("hi from Bea")).unwrap()));
        }
    }
    let mut strangers: Vec<(Client, Vec<String>)> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let mut b = bea.lock().unwrap();
    for _ in 0..10 {
        got.extend(texts(&b.sync(0).unwrap()));
        if got.len() >= 20 {
            break;
        }
    }
    got.sort();
    let mut want: Vec<String> = (0..20).map(|i| format!("from {i}")).collect();
    want.sort();
    assert_eq!(got, want, "each first Yo once");
    for (i, (s, theirs)) in strangers.iter_mut().enumerate() {
        for _ in 0..5 {
            theirs.extend(texts(&s.sync(0).unwrap()));
            if !theirs.is_empty() {
                break;
            }
        }
        assert_eq!(theirs.as_slice(), ["hi from Bea"], "stranger {i}");
    }
}
