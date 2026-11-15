//! Groups (spec 4.7): Yos to everyone in a group over the members' sessions, changes as whole revisions, nothing
//! of a group at the hub.

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

/// Ann makes a group with Ben and Cleo, who do not know each other: each Yo reaches the other two, as a group Yo,
/// and nobody becomes a 1:1 chat of anyone through it.
#[test]
fn a_group_yo_reaches_every_other_member_and_makes_no_chats() {
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
    b.send_group(&id, Content::Yo).unwrap();
    let to_c = take(c);
    assert_eq!(group_yos(&to_c), [(id.clone(), b.user_id().to_owned())]);
    assert_eq!(group_yos(&take(a)), [(id.clone(), b.user_id().to_owned())]);
    // Ben is no chat of Cleo's: no NewChat, no 1:1 message.
    assert!(!to_c.iter().any(|e| matches!(e, Event::NewChat { .. } | Event::Message { content: Content::Yo, .. })));
}

/// Added, removed, left: everyone in it before and after learns the new revision; who is out gets nothing more and
/// sees the group as left.
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
    // Cleo cannot send to it any more; what she sends anyway is not shown.
    assert!(c.send_group(&id, Content::Yo).is_err());
    // Ben leaves: the others see him gone, he sees it left.
    b.leave_group(&id).unwrap();
    take(a);
    take(d);
    assert!(view(b, &id).left);
    assert_eq!(view(a, &id).members, ids(&[a, d]));
    assert_eq!(view(d, &id).members, ids(&[a, d]));
    assert_eq!(view(d, &id).revision, 4);
}

/// Two members rename the group at once, each from the same revision and neither seeing the other's first (both
/// offline): once everything arrived, all three have the same name, the one by the lower user id.
#[test]
fn two_changes_at_once_end_the_same_everywhere() {
    let hub = TestHub::start();
    let base = random_u64() % 1_000_000_000;
    let transports: Vec<_> = (0..3).map(|_| Http::new(hub.url())).collect();
    let mut p: Vec<Client> =
        transports.iter().enumerate().map(|(i, t)| register(t.clone(), &username(base + i as u64 * 7), None)).collect();
    let [a, b, c] = p.as_mut_slice() else { unreachable!() };
    let (id, _) = a.create_group("ONE", &[b.user_id().to_owned(), c.user_id().to_owned()]).unwrap();
    take(b);
    take(c);
    // Sessions between Ben and Cleo first (a Yo each), then both go offline and rename.
    b.send_group(&id, Content::Yo).unwrap();
    take(c);
    c.send_group(&id, Content::Yo).unwrap();
    take(b);
    take(a);
    for t in &transports[1..] {
        *t.base.write().unwrap() = "http://127.0.0.1:1".into();
    }
    let _ = b.rename_group(&id, "BEN");
    let _ = c.rename_group(&id, "CLEO");
    assert_eq!((view(b, &id).name.as_str(), view(c, &id).name.as_str()), ("BEN", "CLEO"));
    for t in &transports[1..] {
        *t.base.write().unwrap() = hub.url();
    }
    for _ in 0..3 {
        for x in [&mut *b, &mut *c, &mut *a] {
            if let Ok(d) = x.deliver_begin() {
                let answers = d.send();
                x.send_end(&d, answers);
            }
            take(x);
        }
    }
    let lower = if b.user_id() < c.user_id() { "BEN" } else { "CLEO" };
    for x in [&*a, &*b, &*c] {
        assert_eq!((view(x, &id).name.as_str(), view(x, &id).revision), (lower, 2));
    }
}

/// A blocked member's Yos to the group are not shown to who blocked them; the others still see them, and the group
/// stays the same for all.
#[test]
fn a_blocked_member_is_not_heard_but_the_group_stays_one() {
    let hub = TestHub::start();
    let mut p = people(&hub, 3);
    let [a, b, c] = p.as_mut_slice() else { unreachable!() };
    let (id, _) = a.create_group("BAND", &[b.user_id().to_owned(), c.user_id().to_owned()]).unwrap();
    take(b);
    take(c);
    a.set_blocked(b.user_id(), true).unwrap();
    b.send_group(&id, Content::Yo).unwrap();
    assert!(group_yos(&take(a)).is_empty());
    assert_eq!(group_yos(&take(c)).len(), 1);
    // A change by the blocked member still counts, so all three keep one group.
    b.rename_group(&id, "RENAMED").unwrap();
    take(a);
    take(c);
    assert_eq!(view(a, &id).name, "RENAMED");
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

/// Ann removes Cleo; Ben, who has not read that yet, renames the group: his change is built on the removal (read as
/// it is made), so Cleo stays out and the revision goes on, not back.
#[test]
fn a_change_is_built_on_what_came_meanwhile() {
    let hub = TestHub::start();
    let mut p = people(&hub, 3);
    let [a, b, c] = p.as_mut_slice() else { unreachable!() };
    let (id, _) = a.create_group("TRIO", &[b.user_id().to_owned(), c.user_id().to_owned()]).unwrap();
    take(b);
    take(c);
    a.remove_from_group(&id, &[c.user_id().to_owned()]).unwrap();
    b.rename_group(&id, "DUO").unwrap();
    let g = view(b, &id);
    assert_eq!((g.name.as_str(), g.revision), ("DUO", 3));
    assert!(!g.members.contains(&c.user_id().to_owned()), "the removal stands");
    take(a);
    assert_eq!(view(a, &id).name, "DUO");
}

/// The last member leaving: nobody to tell, the group simply ends for it, without an error.
#[test]
fn the_last_member_can_leave() {
    let hub = TestHub::start();
    let mut p = people(&hub, 2);
    let [a, b] = p.as_mut_slice() else { unreachable!() };
    let (id, _) = a.create_group("PAIR", &[b.user_id().to_owned()]).unwrap();
    take(b);
    b.leave_group(&id).unwrap();
    take(a);
    a.leave_group(&id).unwrap();
    assert!(view(a, &id).left);
}

/// The same photo sent to a group again (an app that lost its answer and sends it once more) reaches each member
/// once; a member added since gets it, who had it does not get it twice (spec 4.7, 5).
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
