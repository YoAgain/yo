//! Contact requests (spec 4.10): what comes from someone not yet a contact waits as a request; a contact shows the
//! account's contact key to the hub, which takes requests only as the account allows and only so many a day.

use std::sync::Arc;
use std::time::Duration;
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

/// Everything waiting either way taken in: each has the other's contact key.
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

#[test]
fn requests_past_the_daily_cap_wait_while_contacts_go_on() {
    // Deliveries are counted, not senders (the hub cannot tell them apart): each stranger's first is its name and a Yo.
    let hub = TestHub::start_with(config(|c| c.limits.requests_per_user_day = 4), "127.0.0.1:0");
    let (mut a, _, _) = account(&hub, "ALICE");
    let (mut b, _, pb) = account(&hub, "BOB");
    friends(&mut a, &mut b, &pb);
    let mut strangers: Vec<Account> = (0..3).map(|_| account(&hub, "STRANGER")).collect();
    for (s, _, _) in strangers.iter_mut() {
        s.send_yo(&pb).unwrap();
    }
    let waiting: Vec<usize> = strangers.iter().map(|(s, _, _)| s.waiting()).collect();
    assert_eq!(waiting, [0, 0, 2], "the third's wait: no room for requests today");
    a.send_yo(&pb).unwrap();
    assert_eq!(a.waiting(), 0, "a contact is no request");
    let events = b.sync(0).unwrap();
    assert_eq!(yos(&events, &a), 1);
    assert_eq!(strangers.iter().map(|(s, _, _)| yos(&events, s)).sum::<usize>(), 2);
}

#[test]
fn a_block_rotates_the_key_the_others_get_it_and_the_old_one_lapses() {
    let grace = 1500;
    let hub = TestHub::start_with(config(|c| c.limits.contact_key_grace_ms = grace), "127.0.0.1:0");
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
    b.set_blocked(a.user_id(), true).unwrap();
    b.maintenance().unwrap();
    // Within the grace the old key still counts: the hub takes the blocked one's Yo (B's device drops it, 4.8).
    a.send_yo(&pb).unwrap();
    assert_eq!((a.waiting(), a.dropped()), (0, 0), "{:?}", a.drop_reasons());
    assert_eq!(yos(&b.sync(0).unwrap(), &a), 0);
    std::thread::sleep(Duration::from_millis(grace + 200));
    // Past it, C back: the waiting Yo goes first with the old key, refused, kept; the inbox brings the new key, and
    // it goes again.
    let before = deliveries(&hc, &b);
    c.maintenance().unwrap();
    assert_eq!((c.waiting(), c.dropped()), (0, 0), "{:?}", c.drop_reasons());
    assert!(deliveries(&hc, &b) >= before + 2, "refused once, then taken");
    assert_eq!(yos(&b.sync(0).unwrap(), &c), 1);
    c.send_yo(&pb).unwrap();
    assert_eq!(yos(&b.sync(0).unwrap(), &c), 1);
    // The blocked one is a stranger now: refused by the hub, once more after its inbox, then dropped.
    a.send_yo(&pb).unwrap();
    a.sync(0).unwrap();
    a.after_sync().unwrap();
    assert_eq!(a.waiting(), 0);
    assert!(a.drop_reasons().iter().any(|r| r.contains("no requests")), "{:?}", a.drop_reasons());
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
