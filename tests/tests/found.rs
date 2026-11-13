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
            Event::Message { content: Content::Text(t), .. } => Some(t.clone()),
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
    other.send(&name, Content::Text("found you".into())).unwrap();
    assert_eq!(texts(&me.sync(0).unwrap()), ["found you"], "the name is this device's");
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
    b.send(a.user_id(), Content::Text("one".into())).unwrap();
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
    a.send(b.user_id(), Content::Text("first".into())).unwrap();
    http.fault("/v1/inbox", Fault::Answer(429));
    a.send(b.user_id(), Content::Text("second".into())).unwrap();
    http.fault("/v1/inbox", Fault::Answer(503));
    a.send(b.user_id(), Content::Text("third".into())).unwrap();
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
    a.send(&name, Content::Text("hi".into())).unwrap();
    b.sync(0).unwrap();
    let old = b.user_id().to_owned();
    b.delete_account().unwrap();
    assert_eq!(a.send(&old, Content::Text("are you there".into())), Err(Error::NotOnYo));
    assert!(a.contacts().iter().any(|c| c.user == old && c.gone && c.handles.is_empty()));
    assert_eq!(a.send(&old, Content::Yo), Err(Error::NotOnYo), "known gone: no try");
    assert!(a.chats().is_empty(), "the chat went with it");
    // A stranger takes the name: by the name, it is the stranger now, in a chat of its own.
    let mut c = by_name(Http::new(hub.url()), &name, None).unwrap();
    assert_ne!(c.user_id(), old);
    a.send(&name, Content::Text("who are you".into())).unwrap();
    assert_eq!(texts(&c.sync(0).unwrap()), ["who are you"]);
    assert_eq!(a.pending(), 0);
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
    b.send(a.user_id(), Content::Text("shout".into())).unwrap();
    assert_eq!(texts(&a.sync(0).unwrap()), ["shout"]);
    // The app dies before it showed it: after the restart it comes again, unless blocked meanwhile.
    let saved = a.export(&[1; 32]);
    let mut a = Client::restore(http, &saved, &[1; 32]).unwrap();
    a.set_blocked(b.user_id(), true).unwrap();
    assert!(texts(&a.sync(0).unwrap()).is_empty());
}
