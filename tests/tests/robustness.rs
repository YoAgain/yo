//! What goes wrong on the way: lost answers, crashes between steps, passing refusals, hostile senders.

use std::sync::{Arc, Mutex};
use yo_core::{Client, Content, Event, Store};
use yo_hub::config::Config;
use yo_tests::{deliver_raw, new_phone, random_u64, register, user_of, username, Fault, Http, TestHub};
use yo_wire::api;

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
    fn save(&self, blob: Vec<u8>) -> Result<(), String> {
        *self.0.lock().unwrap() = blob;
        Ok(())
    }
}

#[test]
fn a_crash_after_processing_loses_nothing_and_repeats_nothing() {
    let hub = TestHub::start();
    let (mut a, mut b, _, hb, pa, pb) = chat(&hub);
    let saved = Arc::new(Saved::default());
    let key = [5u8; 32];
    b.persist_to(saved.clone(), key).unwrap();
    for _ in 0..3 {
        a.send_yo(&pb).unwrap();
    }
    // B processes the three Yos and saves; the ack is lost; B is killed before anything else.
    hb.fault("/v1/inbox/ack", Fault::LoseAnswer);
    let first = b.sync(0).unwrap();
    assert_eq!(yos(&first), 3);
    // Killed right after the call: the app may not have the events yet, so they come once more after the restart.
    let blob = saved.0.lock().unwrap().clone();
    let mut again = Client::restore(Http::new(hub.url()), &blob, &key).unwrap();
    assert_eq!(yos(&again.sync(0).unwrap()), 3, "not acknowledged: at least once");
    // The app acknowledged them before it was killed: the hub delivers the envelopes again (the ack never came),
    // they are recognised, and nothing is shown twice.
    b.acknowledge_events();
    drop(again);
    drop(b);
    let blob = saved.0.lock().unwrap().clone();
    let mut b = Client::restore(Http::new(hub.url()), &blob, &key).unwrap();
    assert_eq!(yos(&b.sync(0).unwrap()), 0);
    a.send_yo(&pb).unwrap();
    assert_eq!(yos(&b.sync(0).unwrap()), 1);
    b.send_yo(&pa).unwrap();
    assert_eq!(yos(&a.sync(0).unwrap()), 1);
    assert_eq!(b.dropped(), 0, "{:?}", b.drop_reasons());
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
    // B's check of A's identity hits a rate limit: the first message stays at the hub.
    hb.fault("/v1/lookup", Fault::Answer(429));
    assert!(b.sync(0).is_err());
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
    let junk = yo_tests::junk(7);
    // A stranger fills B's inbox with well-formed junk...
    let statuses: Vec<u16> = (0..6).map(|_| deliver_raw(&hub.url(), b.user_id(), junk.clone())).collect();
    assert_eq!(statuses, vec![202, 202, 202, 202, 202, 507]);
    // ... which holds A's Yo back only until B reads its mailbox.
    a.send_yo(&pb).unwrap();
    assert_eq!(a.pending(), 1);
    assert_eq!(yos(&b.sync(0).unwrap()), 0);
    b.after_sync().unwrap();
    a.sync(0).unwrap();
    assert_eq!(a.pending(), 0);
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

const TARGET: u64 = yo_wire::limits::PREKEY_TARGET as u64;

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
        .map(|(_, _, body)| yo_wire::decode(body).unwrap())
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
    assert_eq!(a.pending(), 1, "waits for the new device's session");
    a.sync(0).unwrap();
    assert_eq!(a.pending(), 0);
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
