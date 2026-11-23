//! Groups against a stranger (security audit 2026-11-23: G1): a group of a stranger's making reaches no one further
//! than the stranger writing to them would, and with requests off not at all.

use yo_core::{Client, Content, Event};
use yo_tests::{random_u64, register, username, Http, TestHub};

fn take(c: &mut Client) -> Vec<Event> {
    (0..3).flat_map(|_| c.sync(0).unwrap()).collect()
}

fn rounds(c: &mut Client) {
    for _ in 0..2 {
        c.sync(0).unwrap();
        c.after_sync().unwrap();
    }
}

fn group_yos(events: &[Event], from: &Client) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, Event::GroupMessage { user, content: Content::Yo, .. } if user == from.user_id()))
        .count()
}

/// Signed deliveries `of` sent to `to`: requests, which the hub's buckets count.
fn signed_to(of: &Http, to: &Client) -> usize {
    let path = yo_wire::api::deliver_path(&yo_tests::user_id(to.user_id()));
    let signed = |hs: &[(String, String)]| hs.iter().any(|(n, _)| n == yo_wire::request::HEADER_DEVICE);
    of.recorded.lock().unwrap().iter().filter(|(_, p, hs, _)| *p == path && signed(hs)).count()
}

/// A hub with the production request buckets (spec 6.6); the rest unlimited as on every test hub.
fn hub_with_buckets() -> TestHub {
    let mut config = yo_hub::config::Config::for_tests();
    (config.limits.requests_per_pair, config.limits.request_recipients_per_sender) = (10, 20);
    config.limits.requests_per_recipient = 1000;
    TestHub::start_with(config, "127.0.0.1:0")
}

/// A stranger who only knows a victim's user id makes a group with the victim invited (no key, no consent). The hub
/// endorses no one invited: every Yo into it goes to the victim as a request, signed and counted, held back past
/// the pair bucket exactly as the same stranger's 1:1 Yos are.
#[test]
fn a_strangers_group_reaches_the_victim_only_as_requests() {
    let hub = hub_with_buckets();
    let base = random_u64() % 1_000_000_000;
    let hm = Http::new(hub.url());
    let mut m = register(hm.clone(), &username(base), Some("MAL"));
    let mut v = register(Http::new(hub.url()), &username(base + 14), Some("VIC"));
    rounds(&mut v); // the victim's access key is at the hub: strangers are requests
    const N: usize = 30;
    let (id, _) = m.create_group("HI", &[v.user_id().to_owned()]).unwrap();
    for _ in 0..N {
        m.send_group(&id, Content::Yo).unwrap();
    }
    assert_eq!(m.endorsed_deliveries(), 0, "no endorsement for an invited member");
    assert!(signed_to(&hm, &v) > 0, "requests, signed: the hub knows and limits the sender");
    assert!(m.waiting() > 0, "held back past the pair bucket");
    let shown = group_yos(&take(&mut v), &m);
    assert!(shown < 10, "at most the pair bucket (the group's tag took one): {shown}");
}

/// "Requests from Strangers" off (spec 4.10): a stranger's group, its tag and its Yos, are dropped unseen: no group
/// shows, no change is told to the app, no Yo.
#[test]
fn requests_off_hides_a_strangers_group_and_its_yos() {
    let hub = TestHub::start();
    let base = random_u64() % 1_000_000_000;
    let mut m = register(Http::new(hub.url()), &username(base), Some("MAL"));
    let mut v = register(Http::new(hub.url()), &username(base + 7), Some("VIC"));
    rounds(&mut v);
    v.set_requests_open(false).unwrap();
    m.send_yo(v.user_id()).unwrap();
    let one_to_one = take(&mut v);
    assert!(
        !one_to_one
            .iter()
            .any(|e| matches!(e, Event::Message { user, content: Content::Yo, .. } if user == m.user_id())),
        "1:1 from a stranger: dropped"
    );
    let (id, _) = m.create_group("HI", &[v.user_id().to_owned()]).unwrap();
    m.send_group(&id, Content::Yo).unwrap();
    m.rename_group(&id, "HELLO").unwrap();
    m.send_group(&id, Content::Yo).unwrap();
    let events = take(&mut v);
    assert_eq!(group_yos(&events, &m), 0, "no group Yo shows with requests off");
    assert!(!events.iter().any(|e| matches!(e, Event::GroupChanged { .. })), "{events:?}");
    assert!(v.groups().is_empty(), "no group request shows");
}

/// Requests off, a group a contact made: joined, so a stranger member's Yos show (the stranger is the group's, not a
/// request). And a stranger's group seen while requests were on stays a request, hidden while they are off.
#[test]
fn requests_off_keeps_a_contacts_group_and_hides_a_strangers_request() {
    let hub = TestHub::start();
    let base = random_u64() % 1_000_000_000;
    let mut a = register(Http::new(hub.url()), &username(base), Some("ANN"));
    let mut v = register(Http::new(hub.url()), &username(base + 7), Some("VIC"));
    let mut s = register(Http::new(hub.url()), &username(base + 14), Some("SAM"));
    // Ann and Vic are contacts.
    v.send_yo(a.user_id()).unwrap();
    take(&mut a);
    a.accept(v.user_id()).unwrap();
    for c in [&mut a, &mut v, &mut s] {
        rounds(c);
    }
    // A stranger's group while requests are on: a request, its Yo shown.
    let (theirs, _) = s.create_group("SPAM", &[v.user_id().to_owned()]).unwrap();
    s.send_group(&theirs, Content::Yo).unwrap();
    assert_eq!(group_yos(&take(&mut v), &s), 1);
    assert!(v.groups().iter().any(|g| g.id == theirs && !g.accepted));
    v.set_requests_open(false).unwrap();
    assert!(!v.groups().iter().any(|g| g.id == theirs), "hidden while requests are off");
    s.send_group(&theirs, Content::Yo).unwrap();
    assert_eq!(group_yos(&take(&mut v), &s), 0);
    // Ann's group with Vic and Sam: joined by Vic; Sam's Yo there shows.
    let (ours, _) = a.create_group("CREW", &[v.user_id().to_owned(), s.user_id().to_owned()]).unwrap();
    take(&mut v);
    take(&mut s);
    s.accept_group(&ours).unwrap();
    assert!(v.groups().iter().any(|g| g.id == ours && g.accepted));
    s.send_group(&ours, Content::Yo).unwrap();
    a.send_group(&ours, Content::Yo).unwrap();
    let events = take(&mut v);
    assert_eq!(group_yos(&events, &s), 1, "a member of a group joined is no stranger there");
    assert_eq!(group_yos(&events, &a), 1);
    // Requests on again: the stranger's request is back, as it was.
    v.set_requests_open(true).unwrap();
    assert!(v.groups().iter().any(|g| g.id == theirs && !g.accepted));
}
