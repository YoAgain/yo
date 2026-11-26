//! What goes wrong on the way: lost answers, crashes between steps, passing refusals, hostile senders.

use std::sync::{Arc, Mutex};
use yo_core::{Client, Content, Event, Store};
use yo_hub::config::Config;
use yo_tests::{deliver_raw, new_phone, random_u64, register, user_of, username, Fault, Http, TestHub};
use yoip::net::api;

fn yos(events: &[Event]) -> usize {
    events.iter().filter(|e| matches!(e, Event::Message { content: Content::Yo, .. })).count()
}

/// A writes to B and B answers: sessions both ways, the steady state.
fn chat(hub: &TestHub) -> (Client, Client, Arc<Http>, Arc<Http>, String, String) {
    let n = random_u64() % 1_000_000_000;
    let (pa, pb) = (username(n), username(n + 1));
    let (ha, hb) = (Http::new(hub.url()), Http::new(hub.url()));
    let mut a = register(ha.clone(), &pa, None);
    let mut b = register(hb.clone(), &pb, None);
    a.send_yo(&pb).unwrap();
    b.sync(0).unwrap();
    b.send_yo(&pa).unwrap();
    a.sync(0).unwrap();
    (a, b, ha, hb, pa, pb)
}

#[derive(Default)]
struct Saved(Mutex<Vec<u8>>);

impl Store for Saved {
    fn save_state(&self, state: &yo_core::state::State, records: yo_core::signal::Entries) -> Result<(), String> {
        let blob = yo_tests::saved_bytes(state, records);
        *self.0.lock().unwrap() = blob;
        Ok(())
    }
}

#[test]
fn a_passing_refusal_does_not_drop_a_first_message() {
    let hub = TestHub::start();
    let n = random_u64() % 1_000_000_000;
    let (pa, pb) = (username(n), username(n + 1));
    let hb = Http::new(hub.url());
    let mut a = register(Http::new(hub.url()), &pa, None);
    let mut b = register(hb.clone(), &pb, None);
    a.send_yo(&pb).unwrap();
    // B's check of A's identity hits a rate limit: the first message is kept on B's device for later (spec 3.2),
    // the inbox goes on.
    hb.fault("/v1/lookup", Fault::Answer(429));
    assert_eq!(yos(&b.sync(0).unwrap()), 0);
    let events = b.sync(0).unwrap();
    assert!(events.contains(&Event::NewChat { user: user_of(&pa) }));
    assert_eq!(yos(&events), 1);
    assert_eq!(b.dropped(), 0, "{:?}", b.drop_reasons());
}

#[test]
fn a_full_mailbox_refuses_until_it_is_read_and_strangers_cannot_rate_limit_it() {
    let mut config = Config::for_tests();
    config.limits.max_pending_per_device = 5;
    let hub = TestHub::start_with(config, "127.0.0.1:0");
    let (mut a, mut b, _, _, pa, pb) = chat(&hub);
    // Acknowledge the setup's already-processed mail without running account maintenance;
    // this fixture deliberately tests the legacy inbox with no access check yet.
    let response = b.signed_for_tests("GET", "/v1/inbox?wait=0", &[]).unwrap();
    let inbox: api::Inbox = yoip::decode(&response.body).unwrap();
    let ids = inbox.items.into_iter().map(|item| item.id).collect();
    b.signed_for_tests("POST", api::INBOX_ACK, &yoip::encode(&api::Ack { ids })).unwrap();
    let junk = yo_tests::junk(7);
    // A stranger fills B's inbox with well-formed junk...
    let statuses: Vec<u16> = (0..6).map(|_| deliver_raw(&hub.url(), b.user_id(), junk.clone())).collect();
    assert_eq!(statuses, vec![202, 202, 202, 202, 202, 507]);
    // ... which holds A's Yo back only until B reads its mailbox.
    a.send_yo(&pb).unwrap();
    assert_eq!(a.waiting(), 1);
    assert_eq!(yos(&b.sync(0).unwrap()), 0);
    b.after_sync().unwrap();
    // A's sync takes B's identified answer in; its key goes back to B with the work after it (spec 3.3).
    a.sync(0).unwrap();
    a.after_sync().unwrap();
    assert_eq!(a.waiting(), 0);
    assert_eq!(yos(&b.sync(0).unwrap()), 1);
    b.send_yo(&pa).unwrap();
    assert_eq!(yos(&a.sync(0).unwrap()), 1);
}

/// Waits up to five seconds for the hub to have nothing under way at its inbox.
fn nothing_under_way(hub: &TestHub) -> bool {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while hub.hub.under_way() != (0, 0) {
        if std::time::Instant::now() > until {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    true
}

#[test]
fn a_long_poll_cut_off_leaves_nothing_behind() {
    let hub = TestHub::start();
    let http = Http::new(hub.url());
    let mut a = register(http.clone(), &username(random_u64() % 1_000_000_000), None);
    // A long poll, signed by A, sent as a client that gives up after a moment.
    let path = api::inbox_path(60);
    http.fault(&path, Fault::Answer(200));
    a.signed_for_tests("GET", &path, &[]).unwrap();
    let (_, _, headers, _) = http.recorded.lock().unwrap().pop().unwrap();
    let mut request = reqwest::blocking::Client::new()
        .get(format!("{}{path}", hub.url()))
        .timeout(std::time::Duration::from_millis(300));
    for (name, value) in headers {
        request = request.header(name, value);
    }
    assert!(request.send().unwrap_err().is_timeout());
    assert!(nothing_under_way(&hub), "{:?}", hub.hub.under_way());
}

/// Senders who take a prekey each, until the target's device should refill.
fn drain(hub: &TestHub, target: &str, base: u64, senders: std::ops::RangeInclusive<u64>) {
    for i in senders {
        register(Http::new(hub.url()), &username(base + i), None).send_yo(target).unwrap();
    }
}

fn new_chats(events: &[Event]) -> usize {
    events.iter().filter(|e| matches!(e, Event::NewChat { .. })).count()
}

const TARGET: u64 = yoip::net::limits::PREKEY_TARGET as u64;

/// A refill the hub took whose answer was lost is sent again as it was (the hub keeps one of each id once): the
/// device holds the private keys of every prekey the hub hands out.
#[test]
fn prekeys_whose_upload_answer_was_lost_still_open_sessions() {
    let hub = TestHub::start();
    let base = random_u64() % 1_000_000_000;
    let target = username(base);
    let hc = Http::new(hub.url());
    let mut c = register(hc.clone(), &target, None);
    drain(&hub, &target, base, 1..=TARGET - 10);
    hc.fault(api::PREKEYS, Fault::LoseAnswer);
    assert_eq!(new_chats(&c.sync(0).unwrap()) as u64, TARGET - 10);
    c.after_sync().unwrap_or_default();
    let _ = c.sync(0);
    drain(&hub, &target, base, TARGET..=TARGET + 19);
    assert_eq!(new_chats(&c.sync(0).unwrap()), 20);
    assert_eq!(c.dropped(), 0, "{:?}", c.drop_reasons());
}

/// A refill the hub does not take for now is sent again, the same keys, not a new batch per try; one it refuses as
/// it is (`400`) is made anew.
#[test]
fn a_refill_not_taken_is_sent_again_one_refused_made_anew() {
    let hub = TestHub::start();
    let base = random_u64() % 1_000_000_000;
    let target = username(base);
    let hc = Http::new(hub.url());
    let mut c = register(hc.clone(), &target, None);
    drain(&hub, &target, base, 1..=TARGET - 10);
    c.sync(0).unwrap();
    let before = c.own_prekeys();
    // Twice a round: a refill that failed stays due, so the next sync's start tries it again too.
    for _ in 0..12 {
        hc.fault(api::PREKEYS, Fault::Answer(503));
    }
    for _ in 0..6 {
        let _ = c.after_sync();
        let _ = c.sync(0);
    }
    let held = c.own_prekeys();
    assert!(held > before && held <= TARGET as usize + 10, "one batch, sent again: {before} → {held}");
    hc.fault(api::PREKEYS, Fault::Answer(400));
    let _ = c.maintenance();
    c.maintenance().unwrap();
    assert!(c.own_prekeys() > held, "the refused batch kept, a new one made and taken");
    drain(&hub, &target, base, TARGET..=TARGET + 19);
    assert!(new_chats(&c.sync(0).unwrap()) >= 20);
    assert_eq!(c.dropped(), 0, "{:?}", c.drop_reasons());
}

/// A rotation whose upload the hub refuses as it is (`400`) is still due: the batch made anew carries a new signed
/// prekey and last resort too, not only one-time keys (the hub would hand out the old ones for another week).
#[test]
fn a_refused_rotation_is_made_anew_with_its_signed_prekey() {
    let hub = TestHub::start();
    let base = random_u64() % 1_000_000_000;
    let hc = Http::new(hub.url());
    let mut c = register(hc.clone(), &username(base), None);
    let at = yo_hub::now_ms() + 8 * 24 * 3600 * 1000;
    c.set_clock(Arc::new(move || at));
    hc.fault(api::PREKEYS, Fault::Answer(400));
    let _ = c.maintenance();
    hc.sent.lock().unwrap().clear();
    c.maintenance().unwrap();
    let uploads: Vec<api::Prekeys> = hc
        .sent
        .lock()
        .unwrap()
        .iter()
        .filter(|(method, path, _)| method == "PUT" && path == api::PREKEYS)
        .map(|(_, _, body)| yoip::decode(body).unwrap())
        .collect();
    let last = uploads.last().expect("an upload made anew");
    assert!(last.signed.is_some() && last.last_resort.is_some(), "the rotation still in it");
}

/// A week: the signed prekey and the last resort are replaced; the oldest of them go once enough newer ones are at
/// the hub, and a sender can start a session all along.
#[test]
fn signed_prekeys_rotate_weekly_and_sessions_still_start() {
    let hub = TestHub::start();
    let base = random_u64() % 1_000_000_000;
    let target = username(base);
    let mut c = register(Http::new(hub.url()), &target, None);
    let start = yo_hub::now_ms();
    for week in 1..=8u64 {
        let at = start + week * 8 * 24 * 3600 * 1000;
        c.set_clock(Arc::new(move || at));
        c.maintenance().unwrap();
        drain(&hub, &target, base, week..=week);
        assert_eq!(new_chats(&c.sync(0).unwrap()), 1, "week {week}");
    }
    assert_eq!(c.dropped(), 0, "{:?}", c.drop_reasons());
}

/// B moves to a new phone while A's Yo waits: the hub names B's devices (`409`), A starts a session with the new one,
/// and the Yo goes there; nothing is lost on either side.
#[test]
fn a_new_phone_gets_what_waits_once_its_device_is_named() {
    let hub = TestHub::start();
    let (mut a, b, ha, _, pa, pb) = chat(&hub);
    let mut b2 = new_phone(Http::new(hub.url()), &pb, &b);
    drop(b);
    // The lookup for the new device's bundle fails once: the Yo waits for the next delivery.
    ha.fault(api::LOOKUP, Fault::Answer(503));
    a.send_yo(&user_of(&pb)).unwrap();
    assert_eq!(a.waiting(), 1, "waits for the new device's session");
    a.sync(0).unwrap();
    assert_eq!(a.waiting(), 0);
    assert_eq!(yos(&b2.sync(0).unwrap()), 1);
    b2.send_yo(&pa).unwrap();
    assert_eq!(yos(&a.sync(0).unwrap()), 1);
    assert_eq!(a.dropped() + b2.dropped(), 0, "{:?} {:?}", a.drop_reasons(), b2.drop_reasons());
}

/// A push token or a backup put: the request's signing time is saved with the state (one clock for every process of
/// the device), not only kept in memory.
#[test]
fn a_signed_put_saves_its_signing_time() {
    let hub = TestHub::start();
    let mut a = register(Http::new(hub.url()), &username(random_u64() % 1_000_000_000), None);
    let saved = Arc::new(Saved::default());
    a.persist_to(saved.clone(), [5; 32]).unwrap();
    let before = saved.0.lock().unwrap().clone();
    a.set_push(yo_core::Push::none()).unwrap();
    assert_ne!(*saved.0.lock().unwrap(), before, "saved after the signed request");
}

/// The first Yo to a new friend while the hub cannot answer who they are (its lookups limited, 429): an error the app
/// keeps and tries again ("not now", one rule for all, spec 3.2), and the Yo goes once it can.
#[test]
fn a_first_yo_the_hub_cannot_look_up_now_goes_when_tried_again() {
    let hub = TestHub::start();
    let n = random_u64() % 100_000_000;
    let (pa, pb) = (username(n), username(n + 1));
    let ha = Http::new(hub.url());
    let mut a = register(ha.clone(), &pa, None);
    let mut b = register(Http::new(hub.url()), &pb, None);
    ha.fault(api::LOOKUP, Fault::Answer(429));
    let error = a.send_yo(&pb).unwrap_err();
    assert!(yo_core::retry_later(&error), "{error:?}: kept and tried again");
    a.send_yo(&pb).unwrap();
    let yos = b.sync(0).unwrap().iter().filter(|e| matches!(e, Event::Message { content: Content::Yo, .. })).count();
    assert_eq!(yos, 1);
}

/// Sending never fetches the inbox. A later receive hands out durable events before acknowledging them.
#[test]
fn sending_leaves_inbox_reception_and_acknowledgement_to_sync() {
    let hub = TestHub::start();
    let (mut a, mut b, _, hb, pa, pb) = chat(&hub);
    b.after_sync().unwrap();
    a.send_yo(&pb).unwrap();
    let acks = |hb: &Http| hb.recorded.lock().unwrap().iter().filter(|r| r.1 == api::INBOX_ACK).count();
    let before = acks(&hb);
    let events = b.send_yo(&pa).unwrap();
    assert_eq!(yos(&events), 0, "send does not receive first");
    assert_eq!(acks(&hb), before, "send does no inbox housekeeping");
    assert_eq!(yos(&b.sync(0).unwrap()), 1, "sync hands out the Yo");
    assert_eq!(acks(&hb), before, "not acknowledged before events are durable");
    b.sync(0).unwrap();
    assert_eq!(acks(&hb), before + 1, "the next call acknowledges it");
    assert_eq!(yos(&b.sync(0).unwrap()), 0, "and it does not come again");
}

/// A message queued and saved is sent, whatever the hub says to its delivery now (spec 3.2): the send succeeds and it
/// waits in the outbox. An app that took "not now" for "not sent" would send it again itself (a share's photo, twice).
#[test]
fn a_send_the_hub_cannot_take_now_succeeds_and_waits() {
    let hub = TestHub::start();
    let (mut a, mut b, ha, _, _, pb) = chat(&hub);
    let path = api::deliver_path(&yo_tests::user_id(b.user_id()));
    ha.fault(&path, Fault::Answer(429));
    a.send_yo(&pb).expect("queued and saved: sent");
    assert_eq!(a.waiting(), 1, "it waits in the outbox");
    // Held for the 429's time (none named: 5 s): the next send is queued, nothing posted again at once.
    let posts = |ha: &yo_tests::Http| ha.sent.lock().unwrap().iter().filter(|r| r.1 == path).count();
    let before = posts(&ha);
    a.send_yo(&pb).expect("queued behind the limit");
    a.sync(0).unwrap();
    assert_eq!((posts(&ha), a.waiting_unheld()), (before, 2), "never retried into the limit, the banner says so");
    a.pass_time_for_tests(5_000);
    // Two faults: the first Yo's next try (before the second goes) and the next round's.
    ha.fault(&path, Fault::Answer(503));
    ha.fault(&path, Fault::Answer(507));
    a.sync(0).unwrap();
    assert!(a.waiting() >= 2, "also on a passing 503 or a full inbox");
    a.sync(0).unwrap();
    a.sync(0).unwrap();
    assert_eq!(a.waiting(), 0);
    assert_eq!(yos(&b.sync(0).unwrap()), 2, "each once");
}

/// A contact's delivery past the recipient's inbound bytes (`429 recipient SECONDS`, spec 4.1) waits for the seconds
/// the hub named, for that contact alone: never posted again at once, and the next send to someone else goes.
#[test]
fn a_contacts_rate_limit_waits_its_time_and_holds_no_one_else() {
    let hub = TestHub::start();
    let (mut a, mut b, ha, _, _, pb) = chat(&hub);
    let n = random_u64() % 1_000_000_000;
    let pc = username(n + 7);
    let mut c = register(Http::new(hub.url()), &pc, None);
    a.send_yo(&pc).unwrap();
    c.sync(0).unwrap();
    let path = api::deliver_path(&yo_tests::user_id(b.user_id()));
    // Contacts with each other's key, so what goes is no request (3.3); nothing left to send.
    for _ in 0..3 {
        b.maintenance().unwrap();
        a.maintenance().unwrap();
    }
    ha.fault(&path, Fault::Body(429, b"recipient 120"));
    a.send_yo(&pb).unwrap();
    let posts = |ha: &yo_tests::Http| ha.sent.lock().unwrap().iter().filter(|r| r.1 == path).count();
    let before = posts(&ha);
    a.send_yo(&pb).unwrap();
    a.send_yo(&pc).unwrap();
    a.sync(0).unwrap();
    assert_eq!(posts(&ha), before, "B's wait its time");
    assert_eq!(yos(&c.sync(0).unwrap()), 1, "C's went");
    assert_eq!(a.waiting_unheld(), 0, "no banner: B's row says it");
    assert!(a.request_held(b.user_id()));
    a.pass_time_for_tests(120_000);
    a.sync(0).unwrap();
    assert_eq!(a.waiting(), 0);
    assert_eq!(yos(&b.sync(0).unwrap()), 2);
}

/// No delivery answer: the persisted outbox survives a client restart and retries; the recipient skips the copy.
#[test]
fn an_unacknowledged_send_is_retried_after_restart() {
    let hub = TestHub::start();
    let (mut a, mut b, ha, _, _, pb) = chat(&hub);
    let saved = Arc::new(Saved::default());
    let key = [6; 32];
    a.persist_to(saved.clone(), key).unwrap();
    let path = api::deliver_path(&yo_tests::user_id(b.user_id()));
    ha.fault(&path, Fault::LoseAnswer);
    a.send_yo(&pb).unwrap();
    assert_eq!(a.waiting(), 1, "no 202 received: retained for retry");
    drop(a);
    let blob = saved.0.lock().unwrap().clone();
    let mut a = yo_tests::restore_bytes(Http::new(hub.url()), &blob).unwrap();
    a.sync(0).unwrap();
    assert_eq!(a.waiting(), 0, "retry acknowledged");
    assert_eq!(yos(&b.sync(0).unwrap()), 1, "original and retry shown once");
    assert_eq!(b.dropped(), 0);
}

/// Saves that fail after the message was queued and saved (the disk filling up as it went): the send is not reported
/// as failed, so the app does not send it a second time (spec 3.2); it arrives once.
#[test]
fn a_save_failing_after_the_message_was_saved_does_not_make_it_unsent() {
    struct Filling(Mutex<Vec<u8>>, std::sync::atomic::AtomicU32);
    impl Store for Filling {
        fn save_state(&self, state: &yo_core::state::State, records: yo_core::signal::Entries) -> Result<(), String> {
            let blob = yo_tests::saved_bytes(state, records);
            if self.1.load(std::sync::atomic::Ordering::SeqCst) == 0 {
                return Err("disk full".into());
            }
            self.1.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            *self.0.lock().unwrap() = blob;
            Ok(())
        }
    }
    let hub = TestHub::start();
    let (mut a, mut b, _, _, _, pb) = chat(&hub);
    let store = Arc::new(Filling(Mutex::default(), std::sync::atomic::AtomicU32::new(u32::MAX)));
    a.persist_to(store.clone(), [6; 32]).unwrap();
    // The message's own save goes; every save after it fails.
    store.1.store(1, std::sync::atomic::Ordering::SeqCst);
    a.send_yo(&pb).expect("saved with the message in it: sent");
    store.1.store(u32::MAX, std::sync::atomic::Ordering::SeqCst);
    a.sync(0).unwrap();
    assert_eq!(yos(&b.sync(0).unwrap()), 1, "once");
}

/// The same for a send split around its network step (`send_begin` … `send_end`, as the app sends): the message was
/// saved when queued, so saves failing after it do not report it unsent, and the app sends no second one.
#[test]
fn a_split_send_whose_later_saves_fail_is_still_sent() {
    struct Filling(Mutex<Vec<u8>>, std::sync::atomic::AtomicU32);
    impl Store for Filling {
        fn save_state(&self, state: &yo_core::state::State, records: yo_core::signal::Entries) -> Result<(), String> {
            let blob = yo_tests::saved_bytes(state, records);
            if self.1.load(std::sync::atomic::Ordering::SeqCst) == 0 {
                return Err("disk full".into());
            }
            self.1.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            *self.0.lock().unwrap() = blob;
            Ok(())
        }
    }
    let hub = TestHub::start();
    let (mut a, mut b, _, _, _, _) = chat(&hub);
    let store = Arc::new(Filling(Mutex::default(), std::sync::atomic::AtomicU32::new(u32::MAX)));
    a.persist_to(store.clone(), [6; 32]).unwrap();
    a.sync(0).unwrap();
    let b_id = b.user_id().to_owned();
    let deliveries = a.send_begin(&b_id, Content::Yo).expect("just synced");
    store.1.store(0, std::sync::atomic::Ordering::SeqCst);
    let answers = deliveries.send();
    assert!(matches!(a.send_end(&deliveries, answers), yo_core::Sent::Done(Ok(_))), "queued and saved: sent");
    store.1.store(u32::MAX, std::sync::atomic::Ordering::SeqCst);
    a.sync(0).unwrap();
    assert_eq!(yos(&b.sync(0).unwrap()), 1, "once");
}
