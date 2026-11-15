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
/// each Yo reaches the other two, and nobody becomes a 1:1 chat of anyone through it. Ben has no profile key of
/// Cleo's: his Yo to her goes sealed on the group's send endorsement.
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
