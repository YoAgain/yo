//! Contact requests (spec 4.10): what comes from someone not yet a contact waits as a request, signed by its sender;
//! a contact shows the account's profile key to the hub instead (sealed sender). The hub takes requests only as the
//! account allows and only so many per sender, in buckets that refill evenly.

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
    // The hub takes the stranger's request (the switch is B's device's); B drops it unseen, and answers nothing.
    c.send_yo(&pb).unwrap();
    assert_eq!((c.waiting(), c.dropped()), (0, 0));
    a.send_yo(&pb).unwrap();
    let events = b.sync(0).unwrap();
    assert_eq!((yos(&events, &a), yos(&events, &c)), (1, 0));
    assert!(!events.iter().any(|e| matches!(e, Event::NewChat { user } if user == c.user_id())));
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
    let limits = |c: &mut Config| (c.limits.requests_per_pair, c.limits.request_recipients_per_sender) = (3, 2);
    let hub = TestHub::start_with(config(limits), "127.0.0.1:0");
    let (mut a, _, _) = account(&hub, "ALICE");
    let (mut b, _, pb) = account(&hub, "BOB");
    friends(&mut a, &mut b, &pb);
    // A spammer: 3 requests at once to B (its key, then a Yo each), the rest waits with it.
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
    a.sync(0).unwrap(); // foreground reception takes in the key rotated by blocking
    a.send_yo(&pb).unwrap();
    assert_eq!(a.waiting(), 0);
    let events = b.sync(0).unwrap();
    assert_eq!((yos(&events, &t), yos(&events, &a), yos(&events, &s)), (1, 1, 0));
    // The spammer may start 2 requests at once: to a third account its requests wait.
    let (mut c, _, pc) = account(&hub, "CAROL");
    let (mut d, _, pd) = account(&hub, "DAVE");
    settle(&mut [&mut c, &mut d]);
    s.send_yo(&pc).unwrap();
    assert_eq!(yos(&c.sync(0).unwrap(), &s), 1);
    let before = s.waiting();
    s.send_yo(&pd).unwrap();
    assert!(s.waiting() > before, "the third new recipient waits");
    assert_eq!(yos(&d.sync(0).unwrap(), &s), 0);
}

/// The request limits are buckets that refill evenly (spec 4.10): past them `429` with `Retry-After`, for that
/// recipient only; after the wait the rest goes, and a key that comes lets it all go at once, as a contact's.
#[test]
fn a_request_past_its_bucket_waits_its_retry_after_and_holds_no_one_else() {
    let limits = |c: &mut Config| c.limits.requests_per_pair = 3;
    let hub = TestHub::start_with(config(limits), "127.0.0.1:0");
    let (mut b, _, pb) = account(&hub, "BOB");
    let (mut c, _, pc) = account(&hub, "CAROL");
    let (mut s, hs, _) = account(&hub, "SAM");
    settle(&mut [&mut b, &mut c]);
    // Its key and 2 Yos: 3 requests; the next 2 Yos wait.
    for _ in 0..4 {
        s.send_yo(&pb).unwrap();
    }
    assert_eq!(s.waiting(), 2);
    let refused = hs.recorded.lock().unwrap().len();
    // Another recipient is not held; nor is B tried again before its time.
    s.send_yo(&pc).unwrap();
    assert_eq!(yos(&c.sync(0).unwrap(), &s), 1);
    assert_eq!(s.waiting(), 2);
    let to_b = |h: &Http| {
        let path = api::deliver_path(&user_id(b.user_id()));
        h.recorded.lock().unwrap().iter().skip(refused).filter(|(_, p, _, _)| *p == path).count()
    };
    assert_eq!(to_b(&hs), 0, "held till its Retry-After");
    // An hour later (both clocks): one more token, one more Yo.
    hub.hub.shift_clock(3_600_000);
    let later = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
    s.set_clock(Arc::new(move || later() + 3_600_000));
    s.maintenance().unwrap();
    assert!(to_b(&hs) >= 1, "tried again after its wait");
    assert_eq!(yos(&b.sync(0).unwrap(), &s), 3);
    // B says yes: its key comes, and the last one goes at once, sealed, without waiting for a token.
    b.accept(s.user_id()).unwrap();
    s.sync(0).unwrap();
    s.after_sync().unwrap();
    assert_eq!(s.waiting(), 0, "{:?}", s.drop_reasons());
    assert_eq!(yos(&b.sync(0).unwrap(), &s), 1, "the fourth");
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
    // The block takes the old key at once: the blocked one is refused, then goes as a request, taken by the hub and
    // dropped unseen by B's device.
    b.set_blocked(a.user_id(), true).unwrap();
    a.send_yo(&pb).unwrap();
    a.sync(0).unwrap();
    a.after_sync().unwrap();
    assert_eq!(a.waiting(), 0);
    assert_eq!(yos(&b.sync(0).unwrap(), &a), 0);
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
    // The hub takes C's Yo as a request; B, its switch off, drops it unseen.
    c.send_yo(b.user_id()).unwrap();
    c.sync(0).unwrap();
    c.after_sync().unwrap();
    assert_eq!(c.dropped(), 0, "{:?}", c.drop_reasons());
    assert_eq!(yos(&b.sync(0).unwrap(), &c), 0);
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

/// A contact that missed a new key heals with its first identified Yo (spec 4.10): A blocks C, its new key never
/// reaches B, B sends A 15 Yos with a request bucket of 3: the first goes identified (a request), A sends its key
/// back at once, the rest go sealed. All arrive.
#[test]
fn a_contact_that_missed_the_new_key_heals_with_its_first_identified_yo() {
    let limits = |c: &mut Config| c.limits.requests_per_pair = 3;
    let hub = TestHub::start_with(config(limits), "127.0.0.1:0");
    let (mut a, ha, pa) = account(&hub, "ALICE");
    let (mut b, hb, pb) = account(&hub, "BOB");
    let (mut c, _, pc) = account(&hub, "CAROL");
    friends(&mut a, &mut b, &pb);
    friends(&mut a, &mut c, &pc);
    // The key's envelope to B is refused for good: gone, as if lost.
    ha.fault(&api::deliver_path(&user_id(b.user_id())), Fault::Answer(400));
    a.set_blocked(c.user_id(), true).unwrap();
    a.sync(0).unwrap();
    a.after_sync().unwrap();
    let (signed, sealed) = kinds(&hb, &a);
    let mut shown = 0;
    for i in 0..15 {
        b.send_yo(&pa).unwrap();
        if i % 3 == 2 {
            shown += yos(&a.sync(0).unwrap(), &b);
            a.after_sync().unwrap();
            b.sync(0).unwrap();
            b.after_sync().unwrap();
        }
    }
    for _ in 0..2 {
        b.sync(0).unwrap();
        b.after_sync().unwrap();
        shown += yos(&a.sync(0).unwrap(), &b);
        a.after_sync().unwrap();
    }
    assert_eq!((b.waiting(), b.dropped()), (0, 0), "{:?}", b.drop_reasons());
    assert_eq!(shown, 15);
    let (signed_now, sealed_now) = kinds(&hb, &a);
    assert!(signed_now - signed <= 3, "{} identified", signed_now - signed);
    assert!(sealed_now - sealed >= 12, "the rest sealed: {}", sealed_now - sealed);
}

/// A key missed comes with the next Yo too (spec 4.10, as Signal's every message carries it).
#[test]
fn a_yo_carries_the_profile_key() {
    let hub = TestHub::start();
    let (mut a, ha, pa) = account(&hub, "ALICE");
    let (mut b, hb, pb) = account(&hub, "BOB");
    let (c, _, _) = account(&hub, "CAROL");
    friends(&mut a, &mut b, &pb);
    ha.fault(&api::deliver_path(&user_id(b.user_id())), Fault::Answer(400));
    a.set_blocked(c.user_id(), true).unwrap();
    a.send_yo(&pb).unwrap();
    assert_eq!(yos(&b.sync(0).unwrap(), &a), 1);
    let (signed, sealed) = kinds(&hb, &a);
    b.send_yo(&pa).unwrap();
    assert_eq!(kinds(&hb, &a), (signed, sealed + 1), "with the new key: sealed at once");
    assert_eq!(yos(&a.sync(0).unwrap(), &b), 1);
}

/// An unblock gives the key made at the block (spec 4.10): what the unblocked one sends is a contact's again.
#[test]
fn an_unblocked_contact_gets_the_current_key() {
    let hub = TestHub::start();
    let (mut a, _, pa) = account(&hub, "ALICE");
    let (mut b, hb, pb) = account(&hub, "BOB");
    friends(&mut a, &mut b, &pb);
    a.set_blocked(b.user_id(), true).unwrap();
    a.set_blocked(b.user_id(), false).unwrap();
    a.sync(0).unwrap();
    a.after_sync().unwrap();
    settle(&mut [&mut b]);
    let (signed, sealed) = kinds(&hb, &a);
    b.send_yo(&pa).unwrap();
    assert_eq!(kinds(&hb, &a), (signed, sealed + 1));
    assert_eq!(yos(&a.sync(0).unwrap(), &b), 1);
}

/// A `403` to a delivery that showed a key never drops it, also a second time (two new keys one after the other):
/// only a request's `403` does.
#[test]
fn a_second_stale_key_drops_nothing() {
    let hub = TestHub::start();
    let (mut a, ha, pa) = account(&hub, "ALICE");
    let (mut b, _, pb) = account(&hub, "BOB");
    let (c, _, _) = account(&hub, "CAROL");
    let (d, _, _) = account(&hub, "DAVE");
    friends(&mut a, &mut b, &pb);
    // Two new keys; neither reaches B.
    ha.fault(&api::deliver_path(&user_id(b.user_id())), Fault::Answer(400));
    a.set_blocked(c.user_id(), true).unwrap();
    b.send_yo(&pa).unwrap();
    ha.fault(&api::deliver_path(&user_id(b.user_id())), Fault::Answer(400));
    a.set_blocked(d.user_id(), true).unwrap();
    b.send_yo(&pa).unwrap();
    let mut shown = 0;
    for _ in 0..3 {
        b.sync(0).unwrap();
        b.after_sync().unwrap();
        shown += yos(&a.sync(0).unwrap(), &b);
        a.after_sync().unwrap();
    }
    assert_eq!((b.waiting(), b.dropped()), (0, 0), "{:?}", b.drop_reasons());
    assert_eq!(shown, 2);
}

/// A warm app send recovers immediately, even with no concurrent inbox poll.
#[test]
fn warm_send_recovers_a_rotated_access_key() {
    for (lost_update, refusal) in [(false, None), (true, None), (false, Some(401)), (false, Some(403))] {
        let hub = TestHub::start();
        let (mut a, ha, _) = account(&hub, "ALICE");
        let (mut b, hb, pb) = account(&hub, "BOB");
        let (c, _, _) = account(&hub, "CAROL");
        friends(&mut a, &mut b, &pb);
        let old_key = b.access_key_for_tests();
        if lost_update {
            hb.fault(&api::deliver_path(&user_id(a.user_id())), Fault::Answer(400));
        }
        b.set_blocked(c.user_id(), true).unwrap();
        assert_ne!(b.access_key_for_tests(), old_key);
        let to = b.user_id().to_owned();
        let path = api::deliver_path(&user_id(&to));
        if let Some(status) = refusal {
            // First refusal triggers refresh; the refreshed key is refused once more.
            ha.fault(&path, Fault::Answer(status));
            ha.fault(&path, Fault::Answer(status));
        }
        let before = kinds(&ha, &b);
        let pending = a.send_begin(&to, Content::Yo).expect("warm session");
        let mut sent = a.send_end(&pending, pending.send());
        loop {
            match sent {
                yo_core::Sent::More(next) => sent = a.send_end(&next, next.send()),
                yo_core::Sent::Done(result) => {
                    result.unwrap();
                    break;
                }
            }
        }
        assert_eq!(a.waiting(), 0, "lost update: {lost_update}");
        assert_eq!(yos(&b.sync(0).unwrap(), &a), 1);
        assert_eq!(a.dropped(), 0, "{:?}", a.drop_reasons());
        if lost_update || refusal.is_some() {
            assert!(kinds(&ha, &b).0 > before.0, "falls back to a signed request");
        }
    }
}

#[test]
fn a_recoverable_access_refusal_keeps_the_outbox_in_order() {
    let hub = TestHub::start();
    let (mut a, ha, _) = account(&hub, "ALICE");
    let (mut b, _, pb) = account(&hub, "BOB");
    friends(&mut a, &mut b, &pb);
    let to = b.user_id().to_owned();
    let path = api::deliver_path(&user_id(&to));
    for text in ["first", "second"] {
        ha.fault(&path, Fault::Answer(503));
        a.send(&to, yo_tests::tagged(text.into())).unwrap();
    }
    assert_eq!(a.waiting(), 2);

    // Only the first envelope is refused. Its signed fallback needs to go before
    // the second envelope, which the hub would otherwise accept in this round.
    ha.fault(&path, Fault::Answer(403));
    a.sync(0).unwrap();
    assert_eq!(a.waiting(), 0);
    let received: Vec<_> = b
        .sync(0)
        .unwrap()
        .into_iter()
        .filter_map(|event| match event {
            Event::Message { content, .. } => yo_tests::tag_of(&content),
            _ => None,
        })
        .collect();
    assert_eq!(received, ["first", "second"]);
}

/// Spec 4.9, 6.9: a group accepted before the hub has this account's commitment is joined in the account's storage
/// at once, and its own entry gets the profile key once the commitment is there: on this phone, and on the next one
/// (the group from the storage) when this one never got to it.
#[test]
fn a_group_accepted_before_the_commitment_is_stored_and_keyed_later() {
    let hub = TestHub::start();
    let (mut b, hb, handle) = account(&hub, "BOB");
    let (mut c, _, _) = account(&hub, "CAROL");
    for _ in 0..10 {
        hb.fault(api::ACCESS_KEY, Fault::Answer(503));
    }
    let (id, _) = c.create_group("Strangers", &[b.user_id().to_owned()]).unwrap();
    settle(&mut [&mut b]);
    b.accept_group(&id).unwrap();
    let group = |b: &Client| b.saved().state.groups[&id].clone();
    assert!(!b.saved().state.access_key_told);
    assert!(!group(&b).keyed);
    b.write_storage().unwrap();
    let response = b.signed_for_tests("GET", api::STORAGE, &[]).unwrap();
    let stored: api::Storage = yo_wire::decode(&response.body).unwrap();
    let user = yo_core::state::unhex(b.user_id()).unwrap();
    let storage = yo_core::storage::open(&b.account_secret(), &user, stored.data.as_slice()).unwrap();
    assert!(storage.groups[&id].accepted.value, "joined in the storage");
    hb.faults.lock().unwrap().clear();
    let mut next =
        Client::register_username(Http::new(hub.url()), &handle, Some(b.account_secret()), None, yo_core::Push::none())
            .unwrap();
    next.maintenance().unwrap();
    next.maintenance().unwrap();
    assert!(group(&next).keyed, "keyed by the next phone");
}
