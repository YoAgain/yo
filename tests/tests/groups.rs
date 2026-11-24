//! Groups (spec 4.9): Signal's Groups v2 at the hub, zkgroup. The hub keeps each group encrypted with its revision;
//! members change it one revision on (the hub's compare and swap settles two at once), Yos go over the members'
//! sessions, sealed with group send endorsements where the sender lacks a member's profile key.

use yo_core::{Client, Content, Event};
use yo_tests::{random_u64, register, username, Http, TestHub};

fn people(hub: &TestHub, n: usize) -> Vec<Client> {
    let base = random_u64() % 1_000_000_000;
    (0..n as u64).map(|i| register(Http::new(hub.url()), &username(base + i * 7), None)).collect()
}

/// What a client gets in a few rounds of its inbox.
fn take(c: &mut Client) -> Vec<Event> {
    (0..3).flat_map(|_| c.sync(0).unwrap()).collect()
}

/// The group Yos in events: (group, from).
fn group_yos(events: &[Event]) -> Vec<(String, String)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::GroupMessage { group, user, content: Content::Yo, .. } => Some((group.clone(), user.clone())),
            _ => None,
        })
        .collect()
}

fn view(c: &Client, id: &str) -> yo_core::groups::GroupView {
    c.groups().into_iter().find(|g| g.id == id).expect("the group")
}

fn ids(cs: &[&Client]) -> Vec<String> {
    let mut ids: Vec<String> = cs.iter().map(|c| c.user_id().to_owned()).collect();
    ids.sort();
    ids
}

/// Ann makes a group with Ben and Cleo, who do not know each other: everyone fetches the same group from the hub,
/// each Yo reaches the other two, and nobody becomes a 1:1 chat of anyone through it. Both say yes (their keys into
/// their entries, spec 4.9); Ben has no profile key of Cleo's yet: his Yo to her goes sealed on the group's send
/// endorsement, which the hub issues for members with their key only.
#[test]
fn a_group_yo_reaches_every_member_also_without_their_profile_key() {
    let hub = TestHub::start();
    let mut p = people(&hub, 3);
    let [a, b, c] = p.as_mut_slice() else { unreachable!() };
    let (id, _) = a.create_group("FAMILY", &[b.user_id().to_owned(), c.user_id().to_owned()]).unwrap();
    for x in [&mut *b, &mut *c] {
        let events = take(x);
        assert!(events.iter().any(|e| matches!(e, Event::GroupChanged { group } if *group == id)));
    }
    for x in [&*a, &*b, &*c] {
        let g = view(x, &id);
        assert_eq!((g.name.as_str(), g.revision, g.left), ("FAMILY", 1, false));
        assert_eq!(g.members, ids(&[a, b, c]));
    }
    a.send_group(&id, Content::Yo).unwrap();
    assert_eq!(group_yos(&take(b)), [(id.clone(), a.user_id().to_owned())]);
    assert_eq!(group_yos(&take(c)), [(id.clone(), a.user_id().to_owned())]);
    // Cleo says yes first, then Ben: his answer has her keyed, so endorsed.
    c.accept_group(&id).unwrap();
    b.accept_group(&id).unwrap();
    let endorsed = b.endorsed_deliveries();
    b.send_group(&id, Content::Yo).unwrap();
    let to_c = take(c);
    assert_eq!(group_yos(&to_c), [(id.clone(), b.user_id().to_owned())]);
    assert_eq!(group_yos(&take(a)), [(id.clone(), b.user_id().to_owned())]);
    assert!(b.endorsed_deliveries() > endorsed, "sealed on endorsements: {:?}", b.drop_reasons());
    // Ben is no chat of Cleo's: no NewChat, no 1:1 message.
    assert!(!to_c.iter().any(|e| matches!(e, Event::NewChat { .. } | Event::Message { content: Content::Yo, .. })));
}

/// Added, removed, renamed, left: everyone in it before and after fetches the new revision; who is out sees the group
/// as left, gets nothing more and cannot change it.
#[test]
fn members_come_and_go_and_who_is_out_gets_nothing() {
    let hub = TestHub::start();
    let mut p = people(&hub, 4);
    let [a, b, c, d] = p.as_mut_slice() else { unreachable!() };
    let (id, _) = a.create_group("CREW", &[b.user_id().to_owned(), c.user_id().to_owned()]).unwrap();
    take(b);
    take(c);
    a.add_to_group(&id, &[d.user_id().to_owned()]).unwrap();
    take(d);
    assert_eq!(view(d, &id).members, ids(&[a, b, c, d]));
    a.remove_from_group(&id, &[c.user_id().to_owned()]).unwrap();
    take(b);
    take(c);
    take(d);
    assert!(view(c, &id).left, "removed");
    assert_eq!(view(b, &id).members, ids(&[a, b, d]));
    // A Yo after it: to Ben and Dana, not to Cleo.
    a.send_group(&id, Content::Yo).unwrap();
    assert_eq!(group_yos(&take(b)).len(), 1);
    assert_eq!(group_yos(&take(d)).len(), 1);
    assert!(group_yos(&take(c)).is_empty());
    // Cleo cannot send to it nor add herself back.
    assert!(c.send_group(&id, Content::Yo).is_err());
    assert!(c.add_to_group(&id, &[c.user_id().to_owned()]).is_err());
    // Dana renames it, Ben leaves.
    d.rename_group(&id, "GANG").unwrap();
    take(b);
    b.leave_group(&id).unwrap();
    take(a);
    take(d);
    assert!(view(b, &id).left);
    for x in [&*a, &*d] {
        let g = view(x, &id);
        assert_eq!((g.name.as_str(), g.revision, g.members.clone()), ("GANG", 5, ids(&[a, d])));
    }
}

/// Two members change the group at once, each from the same revision and neither seeing the other's first: the hub
/// takes one, refuses the other (`409`, the group as it is), which is made again on top of it. Both changes stand.
#[test]
fn two_changes_at_once_are_settled_by_the_hub() {
    let hub = TestHub::start();
    let mut p = people(&hub, 4);
    let [a, b, c, d] = p.as_mut_slice() else { unreachable!() };
    let (id, _) = a.create_group("ONE", &[b.user_id().to_owned(), c.user_id().to_owned()]).unwrap();
    take(b);
    take(c);
    // Neither reads before changing.
    a.rename_group(&id, "TWO").unwrap();
    b.add_to_group(&id, &[d.user_id().to_owned()]).unwrap();
    let g = view(b, &id);
    assert_eq!((g.name.as_str(), g.revision), ("TWO", 3), "made again on Ann's");
    assert!(g.members.contains(&d.user_id().to_owned()));
    let all = ids(&[a, b, c, d]);
    for x in [&mut *a, &mut *c, &mut *d] {
        take(x);
        let g = view(x, &id);
        assert_eq!((g.name.as_str(), g.revision, g.members.clone()), ("TWO", 3, all.clone()));
    }
}

/// A group a stranger added this account to is a request until a yes; one a contact (who has its profile key) added
/// it to is joined at once.
#[test]
fn a_strangers_group_is_a_request_a_contacts_is_joined() {
    let hub = TestHub::start();
    let mut p = people(&hub, 3);
    let [a, b, c] = p.as_mut_slice() else { unreachable!() };
    // Ben writes to Ann: Ann is his contact and gets his profile key.
    b.send_yo(a.user_id()).unwrap();
    take(a);
    take(b);
    let (id, _) = a.create_group("MIXED", &[b.user_id().to_owned(), c.user_id().to_owned()]).unwrap();
    take(b);
    take(c);
    assert!(view(a, &id).accepted);
    assert!(view(b, &id).accepted, "added by a contact holding his key");
    assert!(!view(c, &id).accepted, "added by a stranger: a request");
    // A Yo into it shows, as part of the request.
    a.send_group(&id, Content::Yo).unwrap();
    assert_eq!(group_yos(&take(c)).len(), 1);
    c.accept_group(&id).unwrap();
    assert!(view(c, &id).accepted);
    // Her yes put her profile key into her entry: another device of hers would see it joined.
    c.rename_group(&id, "YES").unwrap();
    take(a);
    assert_eq!(view(a, &id).name, "YES");
}

/// At most 20 accounts in a group, its maker among them.
#[test]
fn a_group_has_twenty_members_at_most() {
    let hub = TestHub::start();
    let mut p = people(&hub, 1);
    let a = &mut p[0];
    let others: Vec<String> = (1..=20u32).map(|i| format!("{i:032x}")).collect();
    assert!(a.create_group("TOO MANY", &others).is_err());
    assert!(a.groups().is_empty());
}

/// The last member leaving ends the group at the hub, without an error.
#[test]
fn the_last_member_can_leave() {
    let hub = TestHub::start();
    let mut p = people(&hub, 2);
    let [a, b] = p.as_mut_slice() else { unreachable!() };
    let (id, _) = a.create_group("PAIR", &[b.user_id().to_owned()]).unwrap();
    take(b);
    b.leave_group(&id).unwrap();
    take(a);
    assert!(matches!(a.send_group(&id, Content::Yo), Err(yo_core::Error::GroupEmpty)));
    a.leave_group(&id).unwrap();
    assert!(view(a, &id).left);
}

/// The same photo sent to a group again reaches each member once; a member added since gets it (spec 4.7, 5).
#[test]
fn the_same_group_photo_goes_to_each_member_once() {
    use yo_core::media;
    let hub = TestHub::start();
    let mut p = people(&hub, 4);
    let [a, b, c, d] = p.as_mut_slice() else { unreachable!() };
    let (id, _) = a.create_group("FAMILY", &[b.user_id().to_owned(), c.user_id().to_owned()]).unwrap();
    take(b);
    take(c);
    let (photo, sealed) = media::seal(b"one photo", 4, 3).unwrap();
    a.upload(&photo, 3).unwrap().send(&sealed).unwrap();
    let photos = |events: &[Event]| {
        events
            .iter()
            .filter(|e| matches!(e, Event::GroupMessage { content: Content::Photo(m), .. } if m.id == photo.id))
            .count()
    };
    a.send_group(&id, Content::Photo(photo.clone())).unwrap();
    a.send_group(&id, Content::Photo(photo.clone())).unwrap();
    assert_eq!((photos(&take(b)), photos(&take(c))), (1, 1), "once each, though sent twice");
    a.add_to_group(&id, &[d.user_id().to_owned()]).unwrap();
    take(b);
    take(c);
    a.send_group(&id, Content::Photo(photo.clone())).unwrap();
    assert_eq!(photos(&take(d)), 1, "the member added since gets it");
    assert_eq!((photos(&take(b)), photos(&take(c))), (0, 0), "who had it does not get it again");
}

/// A blocked member's group Yos are not shown; the group stays one.
#[test]
fn a_blocked_member_is_not_heard_but_the_group_stays_one() {
    let hub = TestHub::start();
    let mut p = people(&hub, 3);
    let [a, b, c] = p.as_mut_slice() else { unreachable!() };
    let (id, _) = a.create_group("TRIO", &[b.user_id().to_owned(), c.user_id().to_owned()]).unwrap();
    take(b);
    take(c);
    c.set_blocked(b.user_id(), true).unwrap();
    b.send_group(&id, Content::Yo).unwrap();
    assert!(group_yos(&take(c)).is_empty());
    b.rename_group(&id, "STILL").unwrap();
    take(c);
    assert_eq!(view(c, &id).name, "STILL");
}

/// The groups' master keys are in the account's storage, as Signal keeps them (GroupV2Record): a new phone of the
/// account fetches its groups from the hub with its first sync, named and with their members, before anyone writes to
/// them. A request stays a request; a group left does not come back.
#[test]
fn a_new_phone_gets_its_groups_back_before_any_yo() {
    let hub = TestHub::start();
    let base = random_u64() % 1_000_000_000;
    let names: Vec<String> = (0..3).map(|i| username(base + i * 7)).collect();
    let mut p: Vec<Client> = names.iter().map(|n| register(Http::new(hub.url()), n, None)).collect();
    let [a, b, c] = p.as_mut_slice() else { unreachable!() };
    let (id, _) = a.create_group("FAMILY", &[b.user_id().to_owned(), c.user_id().to_owned()]).unwrap();
    let (gone, _) = a.create_group("GONE", &[b.user_id().to_owned()]).unwrap();
    take(b);
    take(c);
    b.leave_group(&gone).unwrap();
    for x in [&mut *a, &mut *b] {
        x.maintenance().unwrap();
        assert!(x.storage_saved_at().is_some(), "written: {:?}", x.drop_reasons());
    }

    // Ann's new phone: both groups, joined, before anyone writes to them.
    let mut ann = yo_tests::new_phone(Http::new(hub.url()), &names[0], a);
    assert_eq!(ann.user_id(), a.user_id());
    let events = ann.sync(0).unwrap();
    assert!(events.iter().any(|e| matches!(e, Event::GroupChanged { group } if *group == id)), "{events:?}");
    let g = view(&ann, &id);
    assert_eq!((g.name.as_str(), g.revision, g.left, g.accepted), ("FAMILY", 1, false, true));
    assert_eq!(g.members, ids(&[a, b, c]));
    assert_eq!(view(&ann, &gone).members, ids(&[a]));

    // Ben's: FAMILY as he had it (a request, if it was one there), GONE not at all.
    let mut ben = yo_tests::new_phone(Http::new(hub.url()), &names[1], b);
    ben.sync(0).unwrap();
    let g = view(&ben, &id);
    assert_eq!((g.name.as_str(), g.left, g.accepted), ("FAMILY", false, view(b, &id).accepted));
    assert_eq!(g.members, ids(&[a, b, c]));
    assert!(!ben.groups().iter().any(|g| g.id == gone), "{:?}", ben.groups());

    // And it works: Ann's Yo from her new phone reaches the others.
    ann.send_group(&id, Content::Yo).unwrap();
    assert_eq!(group_yos(&take(c)), [(id.clone(), a.user_id().to_owned())]);
}

/// Deliveries `of` sent to `to` signed by its device: requests (spec 4.10), counted in the hub's buckets.
fn signed_to(of: &Http, to: &Client) -> usize {
    let path = yo_wire::api::deliver_path(&yo_tests::user_id(to.user_id()));
    let signed = |hs: &[(String, String)]| hs.iter().any(|(n, _)| n == yo_wire::request::HEADER_DEVICE);
    of.recorded.lock().unwrap().iter().filter(|(_, p, hs, _)| *p == path && signed(hs)).count()
}

/// Two accepted contacts and a third member who is neither's contact (as YoGPT), in a group: (a, b, c), their
/// transports, the group.
fn contacts_and_a_bot(hub: &TestHub) -> ([Client; 3], [std::sync::Arc<Http>; 3], String) {
    let https = [(); 3].map(|_| Http::new(hub.url()));
    let base = random_u64() % 1_000_000_000;
    let mut a = register(https[0].clone(), &username(base), Some("ANN"));
    let mut b = register(https[1].clone(), &username(base + 7), Some("BEN"));
    let mut c = register(https[2].clone(), &username(base + 14), Some("BOT"));
    a.send_yo(&username(base + 7)).unwrap();
    b.sync(0).unwrap();
    b.accept(a.user_id()).unwrap();
    for _ in 0..2 {
        for x in [&mut a, &mut b] {
            x.sync(0).unwrap();
            x.after_sync().unwrap();
        }
    }
    let (id, _) = a.create_group("FAMILY", &[b.user_id().to_owned(), c.user_id().to_owned()]).unwrap();
    take(&mut b);
    take(&mut c);
    c.accept_group(&id).unwrap();
    // Her yes is told to every member (type 8): they fetch the group, now endorsing her (spec 4.9), and get her key.
    take(&mut a);
    take(&mut b);
    ([a, b, c], https, id)
}

/// A group's Yo as the app sends it (`YoClient::send_group_yo`): split around its network step where it can, else
/// whole.
fn send_group_as_the_app(c: &mut Client, id: &str) -> Vec<Event> {
    let Some(mut deliveries) = c.send_group_begin(id, Content::Yo) else {
        return c.send_group(id, Content::Yo).unwrap();
    };
    loop {
        let answers = deliveries.send();
        match c.send_end(&deliveries, answers) {
            yo_core::Sent::More(more) => deliveries = more,
            yo_core::Sent::Done(events) => return events.unwrap(),
        }
    }
}

/// Two accepted contacts in a group with a third member who is neither's contact (as YoGPT, who answers into the
/// group) fire many Yos into it, fast: each goes as a contact's (sealed, the access key or the group's endorsement),
/// none as a request, so the hub's request bucket (ten per pair) never holds one back, and all arrive.
#[test]
fn contacts_in_a_group_send_many_yos_fast_and_none_is_a_request() {
    let hub = TestHub::start();
    let ([mut a, mut b, mut c], [ha, hb, hc], id) = contacts_and_a_bot(&hub);
    let requests = |a: &Client, b: &Client, c: &Client| {
        [(&ha, b), (&hb, a), (&ha, c), (&hb, c), (&hc, a), (&hc, b)].map(|(of, to)| signed_to(of, to))
    };
    let before = requests(&a, &b, &c);
    const N: usize = 30;
    let (mut at_a, mut at_b) = (Vec::new(), Vec::new());
    for i in 0..N {
        // Both ways the app sends: whole, and split around the network.
        let send = |x: &mut Client| match i % 2 {
            0 => x.send_group(&id, Content::Yo).unwrap(),
            _ => send_group_as_the_app(x, &id),
        };
        at_a.extend(send(&mut a));
        at_b.extend(send(&mut b));
        // The third member answers each Yo into the group, as YoGPT does.
        for _ in group_yos(&take(&mut c)) {
            c.send_group(&id, Content::Yo).unwrap();
        }
    }
    for x in [&a, &b, &c] {
        assert_eq!((x.waiting(), x.dropped()), (0, 0), "none held back: {:?}", x.drop_reasons());
    }
    assert_eq!(requests(&a, &b, &c), before, "none sent as a request");
    at_a.extend(take(&mut a));
    at_b.extend(take(&mut b));
    let from = |events: &[Event], who: &Client| group_yos(events).iter().filter(|(_, u)| u == who.user_id()).count();
    assert_eq!((from(&at_b, &a), from(&at_a, &b)), (N, N));
    assert_eq!((from(&at_a, &c), from(&at_b, &c)), (2 * N, 2 * N));
}

/// A burst of Yos into a group costs its deliveries alone, as a burst to one friend does: while the inbox is up to
/// date no inbox read and no acknowledgement goes before each (a round trip each, with the client held meanwhile),
/// one delivery per member, the members side by side.
#[test]
fn a_burst_into_a_group_costs_its_deliveries_alone() {
    let hub = TestHub::start();
    let ([mut a, mut b, mut c], [ha, _, _], id) = contacts_and_a_bot(&hub);
    // The endorsements fetched, the inbox just read.
    a.send_group(&id, Content::Yo).unwrap();
    a.sync(0).unwrap();
    a.after_sync().unwrap();
    let sent_before = ha.sent.lock().unwrap().len();
    const N: usize = 20;
    for _ in 0..N {
        let mut deliveries = a.send_group_begin(&id, Content::Yo).expect("the inbox just read");
        loop {
            let answers = deliveries.send();
            match a.send_end(&deliveries, answers) {
                yo_core::Sent::More(more) => deliveries = more,
                yo_core::Sent::Done(events) => break assert!(events.is_ok()),
            }
        }
    }
    let paths: Vec<String> = ha.sent.lock().unwrap()[sent_before..].iter().map(|(_, p, _)| p.clone()).collect();
    let to = |x: &Client| yo_wire::api::deliver_path(&yo_tests::user_id(x.user_id()));
    assert!(paths.iter().all(|p| *p == to(&b) || *p == to(&c)), "deliveries only: {paths:?}");
    assert_eq!(paths.len(), 2 * N, "one per member and Yo");
    assert_eq!(a.waiting(), 0);
    let from_a = |x: &mut Client| group_yos(&take(x)).iter().filter(|(_, u)| u == a.user_id()).count();
    assert_eq!((from_a(&mut b), from_a(&mut c)), (N + 1, N + 1));
}

#[test]
fn a_non_contact_group_sender_name_is_learned() {
    let hub = TestHub::start();
    let ([_, mut a, mut c], _, id) = contacts_and_a_bot(&hub);
    c.send_group(&id, Content::Yo).unwrap();
    assert_eq!(group_yos(&take(&mut a)), [(id, c.user_id().to_owned())]);
    let contact = a.contacts().into_iter().find(|x| x.user == c.user_id()).unwrap();
    assert!(
        contact.handles.iter().any(|name| Some(name.as_str()) == c.username()),
        "a non-contact member has the sender's username"
    );
}

/// A hub with its database in a file the test opens beside it, that file.
fn hub_on_disk() -> (TestHub, rusqlite::Connection, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("yo-groups-commitment-{}", random_u64()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut config = yo_hub::config::Config::for_tests();
    config.db_path = dir.join("hub.sqlite").to_string_lossy().into_owned();
    let hub = TestHub::start_with(config.clone(), "127.0.0.1:0");
    let db = rusqlite::Connection::open(&config.db_path).unwrap();
    db.busy_timeout(std::time::Duration::from_secs(10)).unwrap();
    (hub, db, dir)
}

/// The requests to `path` a transport sent.
fn asked(of: &Http, path: &str) -> usize {
    of.sent.lock().unwrap().iter().filter(|(_, p, _)| p == path).count()
}

/// Reads and the work after them, `n` times.
fn rounds(c: &mut Client, n: usize) {
    for _ in 0..n {
        c.sync(0).unwrap();
        c.after_sync().unwrap();
    }
}

/// YoGPT's case: an account that told its access key before the hub kept commitments (spec 4.9) has none there,
/// while its device holds it told. Its key going into a group it joined tells the access key with the commitment
/// again, once, and then goes in; the device does not ask for its own credential after every read (it did, 404 each
/// time, about 15 a second).
#[test]
fn a_commitment_the_hub_lacks_is_told_again_and_not_asked_for_after_every_read() {
    let (hub, db, dir) = hub_on_disk();
    let https = [(); 2].map(|_| Http::new(hub.url()));
    let base = random_u64() % 1_000_000_000;
    let mut a = register(https[0].clone(), &username(base), Some("ANN"));
    let mut c = register(https[1].clone(), &username(base + 7), Some("BOT"));
    rounds(&mut c, 2);
    let user = yo_tests::user_id(c.user_id()).to_vec();
    let gone = db.execute("DELETE FROM profile_commitments WHERE user_id = ?1", [&user]).unwrap();
    assert_eq!(gone, 1, "the commitment was told at first");
    let (id, _) = a.create_group("BOTS", &[c.user_id().to_owned()]).unwrap();
    take(&mut c);
    c.accept_group(&id).unwrap();
    rounds(&mut c, 3);
    let told: usize =
        db.query_row("SELECT count(*) FROM profile_commitments WHERE user_id = ?1", [&user], |row| row.get(0)).unwrap();
    assert_eq!(told, 1, "the commitment is at the hub again");
    let before = asked(&https[1], yo_wire::api::GROUP_CREDENTIAL);
    assert!(before <= 2, "its own credential asked for {before} times");
    rounds(&mut c, 10);
    assert_eq!(asked(&https[1], yo_wire::api::GROUP_CREDENTIAL), before, "keyed: not asked for again");
    // And the group works for it: Ann's Yo reaches the bot.
    a.send_group(&id, Content::Yo).unwrap();
    assert_eq!(group_yos(&take(&mut c)), [(id.clone(), a.user_id().to_owned())]);
    hub.stop();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A hub that keeps refusing the own credential (`404`, `403`): after the access key told again it is asked for at
/// most once a pass after a growing pause (a minute first), not after every read.
#[test]
fn an_own_credential_refused_for_good_is_asked_for_after_a_pause_only() {
    let hub = TestHub::start();
    let https = [(); 2].map(|_| Http::new(hub.url()));
    let base = random_u64() % 1_000_000_000;
    let mut a = register(https[0].clone(), &username(base), Some("ANN"));
    let mut c = register(https[1].clone(), &username(base + 7), Some("BOT"));
    rounds(&mut c, 2);
    let (id, _) = a.create_group("BOTS", &[c.user_id().to_owned()]).unwrap();
    take(&mut c);
    for _ in 0..200 {
        https[1].fault(yo_wire::api::GROUP_CREDENTIAL, yo_tests::Fault::Answer(404));
    }
    let credential = |h: &Http| asked(h, yo_wire::api::GROUP_CREDENTIAL);
    let access = |h: &Http| asked(h, yo_wire::api::ACCESS_KEY);
    let (asked_before, told_before) = (credential(&https[1]), access(&https[1]));
    c.accept_group(&id).unwrap();
    rounds(&mut c, 20);
    // One pass: refused, the access key told again, asked once more, refused: a pause.
    assert_eq!(credential(&https[1]) - asked_before, 2);
    assert_eq!(access(&https[1]) - told_before, 1);
    // A minute on (the monotonic clock: a wall clock set back or ahead does not move it): one more pass, then a
    // longer pause.
    c.set_clock_skew(-3_600_000);
    rounds(&mut c, 20);
    assert_eq!(credential(&https[1]) - asked_before, 2, "a clock set back: the pause holds as it was");
    c.pass_time_for_tests(61_000);
    rounds(&mut c, 20);
    assert_eq!(credential(&https[1]) - asked_before, 4);
    c.pass_time_for_tests(61_000);
    rounds(&mut c, 20);
    assert_eq!(credential(&https[1]) - asked_before, 4, "the second pause is longer");
}

/// Groups made are limited a day (each is kept until its last member leaves): by the address they come from, as the
/// hub does not know whose they are (spec 4.9, 6.6); past the limit `429`, which the maker sees as a refusal.
#[test]
fn the_groups_made_from_one_address_are_limited() {
    let mut config = yo_hub::config::Config::for_tests();
    config.limits.groups_per_ip_day = 2;
    let hub = TestHub::start_with(config, "127.0.0.1:0");
    let mut p = people(&hub, 3);
    let [a, b, c] = p.as_mut_slice() else { unreachable!() };
    let others = [b.user_id().to_owned()];
    for name in ["ONE", "TWO"] {
        a.create_group(name, &others).unwrap();
    }
    assert_eq!(a.create_group("THREE", &others).unwrap_err(), yo_core::Error::Refused(429), "past the day's groups");
    assert!(c.create_group("OTHER", &others).is_err(), "the same address: not told apart by account");
    assert_eq!(a.groups().len(), 2);
}

/// Audit M1 (spec 1, 4.9): the hub never learns who is in a group. Every group request but the day's auth credential
/// goes unsigned, on the connection that names no account (`api::anonymous`): making, fetching, changing, the yes,
/// leaving, the profile key credentials; a Yo and a photo to the group go as ever.
#[test]
fn group_requests_name_no_account() {
    let hub = TestHub::start();
    let https = [(); 3].map(|_| Http::new(hub.url()));
    let base = random_u64() % 1_000_000_000;
    let mut a = register(https[0].clone(), &username(base), Some("ANN"));
    let mut b = register(https[1].clone(), &username(base + 7), Some("BEN"));
    let mut c = register(https[2].clone(), &username(base + 14), Some("CLE"));
    // Ann and Ben are contacts: Ben goes in with his key (a credential), Cleo invited.
    b.send_yo(a.user_id()).unwrap();
    take(&mut a);
    take(&mut b);
    let (id, _) = a.create_group("CREW", &[b.user_id().to_owned(), c.user_id().to_owned()]).unwrap();
    take(&mut b);
    take(&mut c);
    c.accept_group(&id).unwrap();
    a.rename_group(&id, "GANG").unwrap();
    take(&mut b);
    b.send_group(&id, Content::Yo).unwrap();
    assert_eq!(group_yos(&take(&mut c)), [(id.clone(), b.user_id().to_owned())]);
    c.leave_group(&id).unwrap();
    a.add_to_group(&id, &[c.user_id().to_owned()]).unwrap();
    let is_group =
        |path: &str| path.starts_with(&format!("{}/", yo_wire::api::GROUPS)) || path == yo_wire::api::GROUP_CREDENTIAL;
    let mut groups = 0;
    for http in &https {
        for (method, path, _) in http.sent.lock().unwrap().iter().filter(|(_, p, _)| is_group(p)) {
            groups += 1;
            assert!(yo_wire::api::anonymous(method, path), "{method} {path} on the anonymous connection");
        }
        // Signed requests (those with headers) are recorded: none is a group's.
        for (method, path, headers, _) in http.recorded.lock().unwrap().iter().filter(|(_, p, _, _)| is_group(p)) {
            let signed = headers.iter().any(|(n, _)| n.eq_ignore_ascii_case(yo_wire::request::HEADER_DEVICE));
            assert!(!signed, "{method} {path} signed");
        }
    }
    assert!(groups >= 8, "made, fetched, changed, credentials: {groups}");
    assert_eq!(view(&a, &id).name, "GANG");
}
