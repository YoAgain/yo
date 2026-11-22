//! What the chaos runs found (tests/CHAOS.md), each the smallest case that showed it, kept so it stays fixed.

use yo_core::{Client, Content, Error, Event, Push};
use yo_tests::{random_u64, Fault, Http, TestHub};

fn by_name(http: std::sync::Arc<Http>, name: &str, secret: Option<[u8; 32]>) -> Result<Client, Error> {
    Client::register_username(http, name, secret, None, Push::none())
}

fn texts(events: &[Event]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Message { content, .. } => yo_tests::tag_of(content),
            _ => None,
        })
        .collect()
}

/// A registration whose answer was lost took the name for a key the app never learnt: its next try, with a new
/// key, found the name taken (by itself) until the 365-day release. The core now tries again with the same key.
#[test]
fn a_registration_whose_answer_was_lost_keeps_the_name() {
    let hub = TestHub::start();
    let name = format!("Lost{}", random_u64() % 100_000);
    let http = Http::new(hub.url());
    http.fault("/v1/register", Fault::LoseAnswer);
    let mut me = by_name(http, &name, None).unwrap();
    let mut other = by_name(Http::new(hub.url()), &format!("Other{}", random_u64() % 100_000), None).unwrap();
    other.send(&name, yo_tests::tagged("found you".into())).unwrap();
    assert_eq!(texts(&me.sync(0).unwrap()), ["found you"], "the name is this device's");
}

/// Every answer lost, also the core's own question whether it is registered: the app keeps the account secret it made
/// (iOS: `NewAccountKey`) and its next try, with the same secret, is the same user with the name.
#[test]
fn the_apps_own_try_again_with_its_secret_keeps_the_name() {
    let hub = TestHub::start();
    let name = format!("Kept{}", random_u64() % 100_000);
    let http = Http::new(hub.url());
    http.fault("/v1/register", Fault::LoseAnswer);
    http.fault("/v1/lookup", Fault::LoseAnswer);
    let secret: [u8; 32] = std::array::from_fn(|_| random_u64() as u8);
    assert!(matches!(by_name(http.clone(), &name, Some(secret)), Err(Error::Network(_))), "every try lost");
    let mut me = by_name(http, &name, Some(secret)).expect("the name is this secret's");
    let mut other = by_name(Http::new(hub.url()), &format!("Tpek{}", random_u64() % 100_000), None).unwrap();
    other.send(&name, yo_tests::tagged("still you".into())).unwrap();
    assert_eq!(texts(&me.sync(0).unwrap()), ["still you"]);
}

/// Two processes of one device (the app and an extension, or a process restarted after a crash) signing the same
/// request in the same millisecond made the same signature, which the hub takes once: the second got `401`. The
/// time signed is now kept in the state, and a `401` is tried once more, signed anew.
#[test]
fn a_signature_the_hub_saw_already_is_made_anew() {
    let hub = TestHub::start();
    let n = random_u64() % 100_000;
    let http = Http::new(hub.url());
    let mut a = by_name(http.clone(), &format!("Sig{n}"), None).unwrap();
    let mut b = by_name(Http::new(hub.url()), &format!("Gis{n}"), None).unwrap();
    b.send(a.user_id(), yo_tests::tagged("one".into())).unwrap();
    http.fault("/v1/inbox", Fault::Answer(401));
    assert_eq!(texts(&a.sync(0).unwrap()), ["one"]);
    http.fault("/v1/storage", Fault::Answer(401));
    assert_eq!(a.signed_for_tests("GET", "/v1/storage", &[]).unwrap().status, 404, "none stored, asked twice");
}

/// A send whose receive-first step met a passing refusal (`429`, `5xx`) failed as a whole, the message never
/// queued, though "not now" is never "never" (spec 4.7).
#[test]
fn a_send_goes_into_the_outbox_when_receiving_first_is_refused_for_now() {
    let hub = TestHub::start();
    let n = random_u64() % 100_000;
    let http = Http::new(hub.url());
    let mut a = by_name(http.clone(), &format!("Busy{n}"), None).unwrap();
    let mut b = by_name(Http::new(hub.url()), &format!("Ybus{n}"), None).unwrap();
    a.send(b.user_id(), yo_tests::tagged("first".into())).unwrap();
    http.fault("/v1/inbox?wait=", Fault::Answer(429));
    a.send(b.user_id(), yo_tests::tagged("second".into())).unwrap();
    http.fault("/v1/inbox?wait=", Fault::Answer(503));
    a.send(b.user_id(), yo_tests::tagged("third".into())).unwrap();
    assert_eq!(texts(&b.sync(0).unwrap()), ["first", "second", "third"]);
}

/// A friend who deleted the account: the Yo went nowhere and the app was told it was sent; the name, taken by a
/// stranger since, still meant the old friend. Now the send says `NotOnYo`, the contact is gone, and its name is
/// asked of the hub again.
#[test]
fn a_friend_who_deleted_the_account_is_gone_and_the_name_free() {
    let hub = TestHub::start();
    let n = random_u64() % 100_000;
    let name = format!("Leaving{n}");
    let mut a = by_name(Http::new(hub.url()), &format!("Stay{n}"), None).unwrap();
    let mut b = by_name(Http::new(hub.url()), &name, None).unwrap();
    a.send(&name, yo_tests::tagged("hi".into())).unwrap();
    b.sync(0).unwrap();
    let old = b.user_id().to_owned();
    b.delete_account().unwrap();
    assert_eq!(a.send(&old, yo_tests::tagged("are you there".into())), Err(Error::NotOnYo));
    assert!(a.contacts().iter().any(|c| c.user == old && c.gone && c.handles.is_empty()));
    assert_eq!(a.send(&old, Content::Yo), Err(Error::NotOnYo), "known gone: no try");
    assert!(a.sessions_with(&old).is_empty(), "its sessions went with it");
    // A stranger takes the name: by the name, it is the stranger now, in a chat of its own.
    let mut c = by_name(Http::new(hub.url()), &name, None).unwrap();
    assert_ne!(c.user_id(), old);
    a.send(&name, yo_tests::tagged("who are you".into())).unwrap();
    assert_eq!(texts(&c.sync(0).unwrap()), ["who are you"]);
    assert_eq!(a.waiting(), 0);
    assert_eq!(a.dropped(), 0, "nothing dropped as an error: a gone user is a state");
}

/// What arrived before a block and waited to be handed out (the app restarted before it had it) was shown after the
/// block. A block now drops it too.
#[test]
fn a_block_also_drops_what_waits_to_be_shown() {
    let hub = TestHub::start();
    let n = random_u64() % 100_000;
    let http = Http::new(hub.url());
    let mut a = by_name(http.clone(), &format!("Calm{n}"), None).unwrap();
    let mut b = by_name(Http::new(hub.url()), &format!("Loud{n}"), None).unwrap();
    b.send(a.user_id(), yo_tests::tagged("shout".into())).unwrap();
    assert_eq!(texts(&a.sync(0).unwrap()), ["shout"]);
    // The app dies before it showed it: after the restart it comes again, unless blocked meanwhile.
    let saved = a.saved();
    let mut a = Client::restore(http, &saved).unwrap();
    a.set_blocked(b.user_id(), true).unwrap();
    assert!(texts(&a.sync(0).unwrap()).is_empty());
}

/// A chat started by someone blocked was ignored, and stayed ignored after the unblock: every later message in it
/// was discarded unseen, for good.
#[test]
fn a_chat_started_while_blocked_works_after_the_unblock() {
    let hub = TestHub::start();
    let n = random_u64() % 100_000;
    let mut r = by_name(Http::new(hub.url()), &format!("Picky{n}"), None).unwrap();
    let mut s = by_name(Http::new(hub.url()), &format!("Eager{n}"), None).unwrap();
    r.set_blocked(s.user_id(), true).unwrap();
    s.send(r.user_id(), yo_tests::tagged("let me in".into())).unwrap();
    assert!(texts(&r.sync(0).unwrap()).is_empty(), "blocked: not shown");
    r.set_blocked(s.user_id(), false).unwrap();
    s.send(r.user_id(), yo_tests::tagged("now?".into())).unwrap();
    let mut got = texts(&r.sync(0).unwrap());
    got.extend(texts(&r.sync(0).unwrap()));
    assert_eq!(got, ["now?"], "dropped: {:?}", r.drop_reasons());
    r.send(s.user_id(), yo_tests::tagged("hi".into())).unwrap();
    assert_eq!(texts(&s.sync(0).unwrap()), ["hi"]);
}

/// A phone whose clock ran two hours ahead signed with that time and saved it as the floor for every later
/// signature (spec 6.2: strictly increasing). With the clock set right again every request stayed two hours ahead,
/// refused (`401`), until the real time caught up: the device was cut off for as long as its clock had been wrong.
/// And a phone whose clock is off by more than the hub's window got `401` on every request, silently (the Android
/// hunt): now the hub tells its time and the device signs and dates its messages by it.
/// (yo-scale: also a clock a few minutes too far ahead, set back to within the window.)
#[test]
fn a_clock_set_right_again_reaches_the_hub_at_once() {
    let hub = TestHub::start();
    let n = random_u64() % 100_000;
    let http = Http::new(hub.url());
    let mut a = by_name(http.clone(), &format!("Late{n}"), None).unwrap();
    let mut b = by_name(Http::new(hub.url()), &format!("Early{n}"), None).unwrap();
    const HOUR: i64 = 3_600_000;
    let near = |offset: i64, want: i64| (offset - want).abs() < 10_000;
    // Sent before the hub told its time: dated two hours ahead, the floor of every later time (spec 6.2's rule
    // for signing times holds for them too: a floor far ahead of the clock is dropped).
    b.sync(0).unwrap();
    a.set_clock_skew(2 * HOUR);
    a.send(b.user_id(), yo_tests::tagged("early".into())).unwrap();
    assert_eq!(texts(&b.sync(0).unwrap()), ["early"]);
    b.acknowledge_events();
    for skew in [2 * HOUR, -2 * HOUR] {
        a.set_clock_skew(skew);
        b.send(a.user_id(), yo_tests::tagged("off".into())).unwrap();
        assert_eq!(texts(&a.sync(0).unwrap()), ["off"], "{skew}: corrected by the hub's time");
        assert!(near(a.hub_offset_for_tests(), -skew), "{}", a.hub_offset_for_tests());
        a.acknowledge_events();
        a.send(b.user_id(), yo_tests::tagged("dated".into())).unwrap();
        let sent_at = b.sync(0).unwrap().into_iter().find_map(|e| match e {
            Event::Message { sent_at_ms, .. } => Some(sent_at_ms),
            _ => None,
        });
        let now = yo_hub::now_ms();
        assert!(sent_at.is_some_and(|at| at.abs_diff(now) < 10_000), "{skew}: dated by the hub's time: {sent_at:?}");
        b.acknowledge_events();
    }
    a.set_clock_skew(0);
    b.send(a.user_id(), yo_tests::tagged("on time".into())).unwrap();
    assert_eq!(texts(&a.sync(0).unwrap()), ["on time"]);
    assert!(near(a.hub_offset_for_tests(), 0), "set right: the offset follows");
    a.acknowledge_events();
    // Also after a restart from what it saved while its clock was wrong (the offset is learned anew).
    a.set_clock_skew(2 * HOUR);
    a.sync(0).unwrap();
    let saved = a.saved();
    let mut a = Client::restore(http, &saved).unwrap();
    b.send(a.user_id(), yo_tests::tagged("still?".into())).unwrap();
    assert_eq!(texts(&a.sync(0).unwrap()), ["still?"]);
    a.send(b.user_id(), yo_tests::tagged("yes".into())).unwrap();
    assert_eq!(texts(&b.sync(0).unwrap()), ["yes"]);
    a.acknowledge_events();
    // A clock 4 minutes fast is within the window; at 8 minutes the hub's time corrects it; back at 4 the floor it
    // left must not hold.
    a.set_clock_skew(4 * 60 * 1000);
    assert!(a.sync(0).is_ok(), "4 minutes off: within the window");
    a.set_clock_skew(8 * 60 * 1000);
    assert!(a.sync(0).is_ok(), "8 minutes off: corrected");
    a.set_clock_skew(4 * 60 * 1000);
    b.send(a.user_id(), yo_tests::tagged("back".into())).unwrap();
    assert_eq!(texts(&a.sync(0).unwrap()), ["back"]);
}

/// A disk that is full for a while: every save fails.
#[derive(Default)]
struct Disk(std::sync::atomic::AtomicBool);

impl yo_core::Store for Disk {
    fn save_state(&self, _: &yo_core::state::State, _: yo_core::signal::Entries) -> Result<(), String> {
        if self.0.load(std::sync::atomic::Ordering::Relaxed) {
            Err("no space left on device".into())
        } else {
            Ok(())
        }
    }
}

/// A block whose save failed (the disk full) stayed in memory: the contact was hidden until the app was restarted
/// from the state without the block, which then showed what they had sent meanwhile (yo-scale). A failed save now
/// leaves the block as it was, as every operation (also verifying and confirming a key).
#[test]
fn a_block_that_could_not_be_saved_is_not_made() {
    let hub = TestHub::start();
    let n = random_u64() % 100_000;
    let mut a = by_name(Http::new(hub.url()), &format!("Full{n}"), None).unwrap();
    let mut b = by_name(Http::new(hub.url()), &format!("Pest{n}"), None).unwrap();
    b.send(a.user_id(), yo_tests::tagged("hi".into())).unwrap();
    assert_eq!(texts(&a.sync(0).unwrap()), ["hi"]);
    let disk = std::sync::Arc::new(Disk::default());
    a.persist_to(disk.clone(), [1; 32]).unwrap();
    disk.0.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(matches!(a.set_blocked(b.user_id(), true), Err(Error::Save(_))));
    assert!(matches!(a.set_verified(b.user_id(), true), Err(Error::Save(_))));
    let contact = a.contacts().into_iter().find(|c| c.user == b.user_id()).unwrap();
    assert!(!contact.blocked && !contact.verified, "not saved, not made: {contact:?}");
    disk.0.store(false, std::sync::atomic::Ordering::Relaxed);
    b.send(a.user_id(), yo_tests::tagged("still here".into())).unwrap();
    assert_eq!(texts(&a.sync(0).unwrap()), ["still here"], "not blocked, as on the disk");
    a.set_blocked(b.user_id(), true).unwrap();
    b.send(a.user_id(), yo_tests::tagged("hidden".into())).unwrap();
    assert!(texts(&a.sync(0).unwrap()).is_empty());
}

fn group_yos_from(events: &[Event], from: &str) -> usize {
    events.iter().filter(|e| matches!(e, Event::GroupMessage { user, .. } if user == from)).count()
}

/// A group Yo to a member this device had no session with yet, whose lookup the hub refused for now (`401`: a
/// clock off for a while; also a full disk), went to the others and never to that member, and the send said it went
/// (yo-scale). A refusal for now fails the send now, as the hub's `429` or no network did already.
#[test]
fn a_group_yo_leaves_no_member_out_for_a_refusal_for_now() {
    let hub = TestHub::start();
    let n = random_u64() % 100_000;
    let http = Http::new(hub.url());
    let mut ann = by_name(http.clone(), &format!("Ann{n}"), None).unwrap();
    let mut ben = by_name(Http::new(hub.url()), &format!("Ben{n}"), None).unwrap();
    let mut dot = by_name(Http::new(hub.url()), &format!("Dot{n}"), None).unwrap();
    let (id, _) = ann.create_group("Trio", &[ben.user_id().to_owned()]).unwrap();
    ben.sync(0).unwrap();
    // Ben adds Dot: Ann learns of her, without a session with her.
    ben.add_to_group(&id, &[dot.user_id().to_owned()]).unwrap();
    ann.sync(0).unwrap();
    dot.sync(0).unwrap();
    assert!(ann.groups().iter().any(|g| g.id == id && g.members.iter().any(|m| m == dot.user_id())));
    // Its lookup refused twice (the signature, and the one signed anew).
    http.fault("/v1/lookup", Fault::Answer(401));
    http.fault("/v1/lookup", Fault::Answer(401));
    assert!(ann.send_group(&id, Content::Yo).is_err(), "not sent to everyone: not sent");
    ann.send_group(&id, Content::Yo).unwrap();
    let dot_got = group_yos_from(&dot.sync(0).unwrap(), ann.user_id());
    let ben_got = group_yos_from(&ben.sync(0).unwrap(), ann.user_id());
    assert_eq!((dot_got, ben_got), (1, 1), "each member once: {:?}", ann.drop_reasons());
}
