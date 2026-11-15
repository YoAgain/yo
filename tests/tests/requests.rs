//! Contact requests (spec 4.10): what comes from someone not yet a contact waits as a request, signed by its sender;
//! a contact shows the account's profile key to the hub instead (sealed sender). The hub takes requests only as the
//! account allows and only so many a day per sender.

use std::sync::Arc;
use yo_core::{Client, Content, Event};
use yo_hub::config::Config;
use yo_tests::{random_u64, register, user_id, username, Fault, Http, TestHub};
use yo_wire::api;

/// An account: its client, its transport, its username.
type Account = (Client, Arc<Http>, String);

fn account(hub: &TestHub, name: &str) -> Account {
    let handle = username(random_u64() % 1_000_000_000);
    let http = Http::new(hub.url());
    (register(http.clone(), &handle, Some(name)), http, handle)
}

fn yos(events: &[Event], from: &Client) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, Event::Message { user, content: Content::Yo, .. } if user == from.user_id()))
        .count()
}

fn named(events: &[Event], from: &Client) -> bool {
    events.iter().any(|e| matches!(e, Event::Profile { user, .. } if user == from.user_id()))
}

/// What `of` sent to `to`'s inbox.
fn deliveries(of: &Http, to: &Client) -> usize {
    let path = api::deliver_path(&user_id(to.user_id()));
    of.sent.lock().unwrap().iter().filter(|(_, p, _)| *p == path).count()
}

fn accepted(of: &Client, who: &Client) -> Option<bool> {
    of.contacts().iter().find(|c| c.user == who.user_id()).map(|c| c.accepted)
}

/// Everything waiting either way taken in: each has the other's profile key.
fn settle(clients: &mut [&mut Client]) {
    for _ in 0..2 {
        for c in clients.iter_mut() {
            c.sync(0).unwrap();
            c.after_sync().unwrap();
        }
    }
}

/// `a` writes to `b`, who says yes: contacts both ways.
fn friends(a: &mut Client, b: &mut Client, b_name: &str) {
    a.send_yo(b_name).unwrap();
    b.sync(0).unwrap();
    b.accept(a.user_id()).unwrap();
    settle(&mut [a, b]);
}

fn config(edit: impl FnOnce(&mut Config)) -> Config {
    let mut config = Config::for_tests();
    edit(&mut config);
    config
}

#[test]
fn a_strangers_yo_is_a_request_that_tells_nothing_until_accepted() {
    let hub = TestHub::start();
    let (mut a, _, _) = account(&hub, "ALICE");
    let (mut b, hb, pb) = account(&hub, "BOB");
    a.send_yo(&pb).unwrap();
    // B sees it (the app shows it as a request) and keeps the contact unaccepted.
    assert_eq!(yos(&b.sync(0).unwrap(), &a), 1);
    b.after_sync().unwrap();
    b.maintenance().unwrap();
    assert_eq!(accepted(&b, &a), Some(false));
    assert_eq!(deliveries(&hb, &a), 0, "no receipt, no name, no key to a stranger");
    assert!(!named(&a.sync(0).unwrap(), &b));
    // Accepted: a contact; the name, the receipt and the key go.
    b.accept(a.user_id()).unwrap();
    assert_eq!(accepted(&b, &a), Some(true));
    assert!(deliveries(&hb, &a) > 0);
    let events = a.sync(0).unwrap();
    assert!(named(&events, &b), "{events:?}");
    // The sender wrote first: B was accepted on A's side all along.
    assert_eq!(accepted(&a, &b), Some(true));
}

#[test]
fn a_declined_request_is_gone_and_a_later_yo_is_a_request_again() {
    let hub = TestHub::start();
    let (mut a, _, _) = account(&hub, "ALICE");
    let (mut b, hb, pb) = account(&hub, "BOB");
    a.send_yo(&pb).unwrap();
    assert_eq!(yos(&b.sync(0).unwrap(), &a), 1);
    b.decline(a.user_id()).unwrap();
    a.send_yo(&pb).unwrap();
    assert_eq!(yos(&b.sync(0).unwrap(), &a), 1);
    b.after_sync().unwrap();
    assert_eq!(accepted(&b, &a), Some(false));
    assert_eq!(deliveries(&hb, &a), 0);
}

#[test]
fn writing_first_accepts_and_a_reply_accepts_too() {
    let hub = TestHub::start();
    let (mut a, _, pa) = account(&hub, "ALICE");
    let (mut b, _, pb) = account(&hub, "BOB");
    a.send_yo(&pb).unwrap();
    b.sync(0).unwrap();
    // B answers instead of tapping yes: that is a yes.
    b.send_yo(&pa).unwrap();
    assert_eq!(accepted(&b, &a), Some(true));
    let events = a.sync(0).unwrap();
    assert_eq!(yos(&events, &b), 1);
    assert!(named(&events, &b), "{events:?}");
}

#[test]
fn with_requests_off_only_contacts_reach_the_account() {
    let hub = TestHub::start();
    let (mut a, _, _) = account(&hub, "ALICE");
    let (mut b, _, pb) = account(&hub, "BOB");
    let (mut c, _, _) = account(&hub, "CAROL");
    friends(&mut a, &mut b, &pb);
    b.set_requests_open(false).unwrap();
    assert!(!b.requests_open());
    c.send_yo(&pb).unwrap();
    assert_eq!(c.waiting(), 0);
    assert!(c.drop_reasons().iter().any(|r| r.contains("no requests")), "{:?}", c.drop_reasons());
    a.send_yo(&pb).unwrap();
    let events = b.sync(0).unwrap();
    assert_eq!((yos(&events, &a), yos(&events, &c)), (1, 0));
    // On again: the stranger gets through as a request.
    b.set_requests_open(true).unwrap();
    c.send_yo(&pb).unwrap();
    assert_eq!(yos(&b.sync(0).unwrap(), &c), 1);
    assert_eq!(accepted(&b, &c), Some(false));
}

/// What `of` sent to `to`'s inbox signed by its device (a request) and showing a profile key (sealed sender).
fn kinds(of: &Http, to: &Client) -> (usize, usize) {
    let path = api::deliver_path(&user_id(to.user_id()));
    let recorded = of.recorded.lock().unwrap();
    let to_them = || recorded.iter().filter(|(_, p, _, _)| *p == path);
    let has = |h: &[(String, String)], name: &str| h.iter().any(|(n, _)| n == name);
    (
        to_them().filter(|(_, _, h, _)| has(h, yo_wire::request::HEADER_DEVICE) && !has(h, api::HEADER_ACCESS)).count(),
        to_them().filter(|(_, _, h, _)| has(h, api::HEADER_ACCESS) && !has(h, yo_wire::request::HEADER_DEVICE)).count(),
    )
}

#[test]
fn a_request_is_signed_a_contacts_delivery_is_sealed() {
    let hub = TestHub::start();
    let (mut a, ha, _) = account(&hub, "ALICE");
    let (mut b, _, pb) = account(&hub, "BOB");
    a.send_yo(&pb).unwrap();
    let (signed, sealed) = kinds(&ha, &b);
    assert!(signed > 0 && sealed == 0, "{signed} {sealed}");
    b.sync(0).unwrap();
    b.accept(a.user_id()).unwrap();
    settle(&mut [&mut a, &mut b]);
    let (signed, sealed) = kinds(&ha, &b);
    a.send_yo(&pb).unwrap();
    assert_eq!(kinds(&ha, &b), (signed, sealed + 1), "with B's key: sealed, unsigned");
    assert_eq!(yos(&b.sync(0).unwrap(), &a), 1);
}

#[test]
fn requests_are_limited_per_sender_and_a_blocked_stranger_takes_nobody_elses() {
    let limits = |c: &mut Config| (c.limits.requests_per_pair_day, c.limits.request_recipients_per_sender_day) = (3, 2);
    let hub = TestHub::start_with(config(limits), "127.0.0.1:0");
    let (mut a, _, _) = account(&hub, "ALICE");
    let (mut b, _, pb) = account(&hub, "BOB");
    friends(&mut a, &mut b, &pb);
    // A spammer: 3 deliveries a day to B (its first is its name and a Yo), the rest waits with it.
    let (mut s, _, _) = account(&hub, "SPAM");
    for _ in 0..4 {
        s.send_yo(&pb).unwrap();
    }
    assert_eq!(s.waiting(), 2);
    assert_eq!(yos(&b.sync(0).unwrap(), &s), 2);
    b.set_blocked(s.user_id(), true).unwrap();
    s.maintenance().unwrap();
    // Another stranger and the contact still get through.
    let (mut t, _, _) = account(&hub, "TOM");
    t.send_yo(&pb).unwrap();
    assert_eq!(t.waiting(), 0, "{:?}", t.drop_reasons());
    a.send_yo(&pb).unwrap();
    assert_eq!(a.waiting(), 0);
    let events = b.sync(0).unwrap();
    assert_eq!((yos(&events, &t), yos(&events, &a), yos(&events, &s)), (1, 1, 0));
    // The spammer may start 2 requests a day: to a third account its requests wait.
    let (mut c, _, pc) = account(&hub, "CAROL");
    let (mut d, _, pd) = account(&hub, "DAVE");
    settle(&mut [&mut c, &mut d]);
    s.send_yo(&pc).unwrap();
    assert_eq!(yos(&c.sync(0).unwrap(), &s), 1);
    let before = s.waiting();
    s.send_yo(&pd).unwrap();
    assert!(s.waiting() > before, "the third recipient of the day waits");
    assert_eq!(yos(&d.sync(0).unwrap(), &s), 0);
}

#[test]
fn a_blocked_contact_loses_sealed_delivery_at_once_the_others_keep_it() {
    let hub = TestHub::start();
    let (mut a, _, _) = account(&hub, "ALICE");
    let (mut b, _, pb) = account(&hub, "BOB");
    let (mut c, hc, _) = account(&hub, "CAROL");
    friends(&mut a, &mut b, &pb);
    friends(&mut c, &mut b, &pb);
    b.set_requests_open(false).unwrap();
    // C's next Yo waits in its outbox (the hub busy): C away from here on.
    hc.fault(&api::deliver_path(&user_id(b.user_id())), Fault::Answer(503));
    c.send_yo(&pb).unwrap();
    assert_eq!(c.waiting(), 1);
    // The block takes the old key at once: the blocked one is refused, once more after its inbox, then dropped.
    b.set_blocked(a.user_id(), true).unwrap();
    a.send_yo(&pb).unwrap();
    a.sync(0).unwrap();
    a.after_sync().unwrap();
    assert_eq!(a.waiting(), 0);
    assert!(a.drop_reasons().iter().any(|r| r.contains("no requests")), "{:?}", a.drop_reasons());
    // C back: the waiting Yo goes first with the old key, refused, kept; the inbox brings the new key, and it goes
    // again, sealed.
    let (_, sealed) = kinds(&hc, &b);
    c.maintenance().unwrap();
    assert_eq!((c.waiting(), c.dropped()), (0, 0), "{:?}", c.drop_reasons());
    assert!(kinds(&hc, &b).1 >= sealed + 2, "refused once, then taken");
    assert_eq!(yos(&b.sync(0).unwrap(), &c), 1);
    c.send_yo(&pb).unwrap();
    assert_eq!(yos(&b.sync(0).unwrap(), &c), 1);
}

#[test]
fn a_contact_without_the_new_key_goes_as_a_request_and_is_no_request_to_the_recipient() {
    let hub = TestHub::start();
    let (mut a, _, _) = account(&hub, "ALICE");
    let (mut b, hb, pb) = account(&hub, "BOB");
    let (mut c, hc, _) = account(&hub, "CAROL");
    friends(&mut a, &mut b, &pb);
    friends(&mut c, &mut b, &pb);
    // The new key does not reach C (its inbox full for a while): C's Yo with the old one is refused, then it goes
    // signed, the requests' way.
    hb.fault(&api::deliver_path(&user_id(c.user_id())), Fault::Answer(507));
    b.set_blocked(a.user_id(), true).unwrap();
    let (signed, _) = kinds(&hc, &b);
    c.send_yo(&pb).unwrap();
    c.sync(0).unwrap();
    c.after_sync().unwrap();
    assert_eq!((c.waiting(), c.dropped()), (0, 0), "{:?}", c.drop_reasons());
    assert!(kinds(&hc, &b).0 > signed, "sent as a request");
    // B knows C as a contact: shown as a Yo of a contact, not a request.
    assert_eq!(yos(&b.sync(0).unwrap(), &c), 1);
    assert_eq!(accepted(&b, &c), Some(true));
    // The key comes later; from then on sealed again.
    b.after_sync().unwrap();
    settle(&mut [&mut c]);
    let (_, sealed) = kinds(&hc, &b);
    c.send_yo(&pb).unwrap();
    assert_eq!(kinds(&hc, &b).1, sealed + 1);
    assert_eq!(yos(&b.sync(0).unwrap(), &c), 1);
}

#[test]
fn declining_a_strangers_group_sends_its_members_no_key() {
    let hub = TestHub::start();
    let (mut b, hb, _) = account(&hub, "BOB");
    let (mut c, _, _) = account(&hub, "CAROL");
    let members = vec![b.user_id().to_owned()];
    let (id, _) = c.create_group("Strangers", &members).unwrap();
    settle(&mut [&mut b]);
    assert_eq!(b.groups().into_iter().find(|g| g.id == id).map(|g| g.accepted), Some(false));
    b.leave_group(&id).unwrap();
    let told = deliveries(&hb, &c);
    b.maintenance().unwrap();
    c.sync(0).unwrap();
    c.after_sync().unwrap();
    b.set_requests_open(false).unwrap();
    b.maintenance().unwrap();
    assert_eq!(deliveries(&hb, &c), told, "nothing after the leave: no key, no name");
    c.send_yo(b.user_id()).unwrap();
    c.sync(0).unwrap();
    c.after_sync().unwrap();
    assert!(c.drop_reasons().iter().any(|r| r.contains("no requests")), "{:?}", c.drop_reasons());
}

#[test]
fn an_invite_to_a_group_from_a_stranger_is_a_request() {
    let hub = TestHub::start();
    let (mut a, _, _) = account(&hub, "ALICE");
    let (mut b, _, pb) = account(&hub, "BOB");
    let (mut c, _, _) = account(&hub, "CAROL");
    friends(&mut a, &mut b, &pb);
    let members = vec![b.user_id().to_owned()];
    let (from_friend, _) = a.create_group("Friends", &members).unwrap();
    let (from_stranger, _) = c.create_group("Strangers", &members).unwrap();
    settle(&mut [&mut b]);
    let state = |b: &Client, id: &str| b.groups().into_iter().find(|g| g.id == id).map(|g| g.accepted);
    assert_eq!(state(&b, &from_friend), Some(true), "a contact's group is joined");
    assert_eq!(state(&b, &from_stranger), Some(false), "a stranger's is a request");
    b.accept_group(&from_stranger).unwrap();
    assert_eq!(state(&b, &from_stranger), Some(true));
    // Its members get B's key: C is no stranger to B's hub check now.
    b.set_requests_open(false).unwrap();
    settle(&mut [&mut c]);
    c.send_yo(b.user_id()).unwrap();
    assert_eq!((c.waiting(), c.dropped()), (0, 0), "{:?}", c.drop_reasons());
}

/// Spec 4.10: what a contact shows the hub is Signal's access key (16 bytes), derived from the recipient's profile key;
/// a block makes a new profile key, so a new access key, and the old one makes a delivery no contact's any more.
#[test]
fn the_access_key_comes_from_the_profile_key_and_a_block_replaces_it() {
    let hub = TestHub::start();
    let (mut a, ha, _) = account(&hub, "Ann");
    let (mut b, _, b_name) = account(&hub, "Ben");
    friends(&mut a, &mut b, &b_name);
    let shown = yo_tests::access_shown(&ha, b.user_id()).expect("a contact's delivery");
    assert_eq!(Some(shown.clone()), b.access_key_for_tests());
    assert_eq!(yo_wire::unb64(&shown).unwrap().len(), api::ACCESS_KEY_LEN);
    // B blocks someone else: a new profile key, a new access key; unsigned with the old one, nothing is taken.
    let (c, _, _) = account(&hub, "Cleo");
    b.set_blocked(c.user_id(), true).unwrap();
    let new = b.access_key_for_tests().unwrap();
    assert_ne!(new, shown);
    let envelope = yo_tests::junk(1);
    assert_ne!(yo_tests::deliver_raw_with(&hub.url(), b.user_id(), Some(&shown), envelope.clone()), 202);
    assert_eq!(yo_tests::deliver_raw_with(&hub.url(), b.user_id(), Some(&new), envelope), 202);
}
