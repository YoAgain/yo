//! Groups (spec 4.7): Yos to everyone in a group over the members' sessions, changes as whole revisions, nothing
//! of a group at the hub.

use yo_core::{Client, Content, Event};
use yo_tests::{deliver_raw, random_u64, register, username, Fault, Http, TestHub};

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
    // At once: each change to the other is held on its way (the hub answers, takes nothing), so each is made before
    // the other's arrives; then both arrive.
    let (to_b, to_c) = (
        yo_wire::api::deliver_path(&yo_tests::user_id(b.user_id())),
        yo_wire::api::deliver_path(&yo_tests::user_id(c.user_id())),
    );
    transports[1].fault(&to_c, Fault::Answer(202));
    transports[2].fault(&to_b, Fault::Answer(202));
    b.rename_group(&id, "BEN").unwrap();
    c.rename_group(&id, "CLEO").unwrap();
    assert_eq!((view(b, &id).name.as_str(), view(c, &id).name.as_str()), ("BEN", "CLEO"));
    let held = |t: &Http, path: &str| {
        t.sent
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == path)
            .map(|(_, _, b)| b.clone())
            .next_back()
            .unwrap()
    };
    assert_eq!(deliver_raw(&hub.url(), c.user_id(), held(&transports[1], &to_c)), 202);
    assert_eq!(deliver_raw(&hub.url(), b.user_id(), held(&transports[2], &to_b)), 202);
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

/// Two members remove each other at once (each before the other's change arrives): both ends still hear the rival
/// change, made from the same revision, and take the same one (the lower author's), so the group does not split.
#[test]
fn two_removing_each_other_at_once_end_the_same() {
    let hub = TestHub::start();
    let base = random_u64() % 1_000_000_000;
    let transports: Vec<_> = (0..3).map(|_| Http::new(hub.url())).collect();
    let mut p: Vec<Client> =
        transports.iter().enumerate().map(|(i, t)| register(t.clone(), &username(base + i as u64 * 7), None)).collect();
    let [a, b, c] = p.as_mut_slice() else { unreachable!() };
    let (id, _) = a.create_group("ONE", &[b.user_id().to_owned(), c.user_id().to_owned()]).unwrap();
    take(b);
    take(c);
    b.send_group(&id, Content::Yo).unwrap();
    take(c);
    c.send_group(&id, Content::Yo).unwrap();
    take(b);
    take(a);
    let (to_b, to_c) = (
        yo_wire::api::deliver_path(&yo_tests::user_id(b.user_id())),
        yo_wire::api::deliver_path(&yo_tests::user_id(c.user_id())),
    );
    transports[1].fault(&to_c, Fault::Answer(202));
    transports[2].fault(&to_b, Fault::Answer(202));
    let (bid, cid) = (b.user_id().to_owned(), c.user_id().to_owned());
    b.remove_from_group(&id, std::slice::from_ref(&cid)).unwrap();
    c.remove_from_group(&id, std::slice::from_ref(&bid)).unwrap();
    let held = |t: &Http, path: &str| {
        t.sent
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, p, _)| m == "POST" && p == path)
            .map(|(_, _, b)| b.clone())
            .next_back()
            .unwrap()
    };
    assert_eq!(deliver_raw(&hub.url(), c.user_id(), held(&transports[1], &to_c)), 202);
    assert_eq!(deliver_raw(&hub.url(), b.user_id(), held(&transports[2], &to_b)), 202);
    for _ in 0..3 {
        for x in [&mut *b, &mut *c, &mut *a] {
            take(x);
        }
    }
    let winner = if bid < cid { bid.clone() } else { cid.clone() };
    let out = if bid < cid { cid } else { bid };
    for x in [&*a, &*b, &*c] {
        let g = view(x, &id);
        assert_eq!((g.revision, g.members.contains(&winner), g.members.contains(&out)), (2, true, false), "{g:?}");
    }
}

/// A member added since sends a Yo that arrives before the change that added them: held, the group asked of a member
/// as Ben has it (Ann), not of the newcomer, who cannot vouch for its own addition; shown once Ann's answer has Cleo.
#[test]
fn a_new_members_yo_before_the_change_is_shown_once_the_change_is_here() {
    let hub = TestHub::start();
    let base = random_u64() % 1_000_000_000;
    let transports: Vec<_> = (0..3).map(|_| Http::new(hub.url())).collect();
    let mut p: Vec<Client> =
        transports.iter().enumerate().map(|(i, t)| register(t.clone(), &username(base + i as u64 * 7), None)).collect();
    let [a, b, c] = p.as_mut_slice() else { unreachable!() };
    let (id, _) = a.create_group("ONE", &[b.user_id().to_owned()]).unwrap();
    take(b);
    // Cleo is added; the change to Ben is held, so Cleo's Yo reaches Ben first.
    let to_b = yo_wire::api::deliver_path(&yo_tests::user_id(b.user_id()));
    transports[0].fault(&to_b, Fault::Answer(202));
    a.add_to_group(&id, &[c.user_id().to_owned()]).unwrap();
    take(c);
    c.send_group(&id, Content::Yo).unwrap();
    let mut events = take(b);
    assert!(group_yos(&events).is_empty(), "held: Ben's group has no Cleo yet");
    for _ in 0..2 {
        take(a);
        events.extend(take(b));
    }
    assert!(view(b, &id).members.contains(&c.user_id().to_owned()), "the group came from Ann when asked");
    assert_eq!(group_yos(&events), [(id.clone(), c.user_id().to_owned())], "and then the Yo, once");
}

/// The same photo to two groups and 1:1 to a member of both: three conversations, three messages; once-only holds
/// within each, not across them.
#[test]
fn the_same_photo_to_two_groups_and_one_to_one_goes_to_each() {
    use yo_core::media;
    let hub = TestHub::start();
    let mut p = people(&hub, 2);
    let [a, b] = p.as_mut_slice() else { unreachable!() };
    let (one, _) = a.create_group("ONE", &[b.user_id().to_owned()]).unwrap();
    let (two, _) = a.create_group("TWO", &[b.user_id().to_owned()]).unwrap();
    take(b);
    let (photo, sealed) = media::seal(b"one photo", 4, 3).unwrap();
    a.upload(&photo, 3).unwrap().send(&sealed).unwrap();
    a.send_group(&one, Content::Photo(photo.clone())).unwrap();
    a.send_group(&two, Content::Photo(photo.clone())).unwrap();
    a.send(b.user_id(), Content::Photo(photo.clone())).unwrap();
    let events = take(b);
    let in_group = |id: &str| {
        events
            .iter()
            .filter(|e| matches!(e, Event::GroupMessage { group, content: Content::Photo(_), .. } if group == id))
            .count()
    };
    let direct = events.iter().filter(|e| matches!(e, Event::Message { content: Content::Photo(_), .. })).count();
    assert_eq!((in_group(&one), in_group(&two), direct), (1, 1, 1));
}

/// A retry sends again what was sent to that contact, but no Yo of a group the sender has left since.
#[test]
fn a_retry_sends_no_yo_of_a_group_left() {
    let hub = TestHub::start();
    let mut p = people(&hub, 3);
    let [a, b, c] = p.as_mut_slice() else { unreachable!() };
    let (id, _) = a.create_group("ONE", &[b.user_id().to_owned(), c.user_id().to_owned()]).unwrap();
    take(b);
    take(c);
    // Sessions both ways first, then B's state from that moment.
    b.send(a.user_id(), Content::Yo).unwrap();
    take(a);
    a.send(b.user_id(), Content::Yo).unwrap();
    take(b);
    let early = b.export(&[3; 32]);
    a.send_group(&id, Content::Yo).unwrap();
    assert_eq!(group_yos(&take(b)).len(), 1);
    a.leave_group(&id).unwrap();
    take(b);
    // B's state goes back: A's next message does not open, B asks for a retry, A sends again what it keeps.
    let mut b = Client::restore(Http::new(hub.url()), &early, &[3; 32]).unwrap();
    a.send(b.user_id(), Content::Yo).unwrap();
    take(&mut b);
    b.after_sync().unwrap();
    take(a);
    a.after_sync().unwrap();
    let again = take(&mut b);
    assert!(group_yos(&again).is_empty(), "no Yo of a group A has left: {again:?}");
}

/// As above, with Ben's app restarted between the new member's Yo and their answer: the ask is kept with the state,
/// so the answer is still taken.
#[test]
fn a_new_members_answer_is_taken_after_a_restart() {
    let hub = TestHub::start();
    let base = random_u64() % 1_000_000_000;
    let transports: Vec<_> = (0..3).map(|_| Http::new(hub.url())).collect();
    let mut p: Vec<Client> =
        transports.iter().enumerate().map(|(i, t)| register(t.clone(), &username(base + i as u64 * 7), None)).collect();
    let [a, b, c] = p.as_mut_slice() else { unreachable!() };
    let (id, _) = a.create_group("ONE", &[b.user_id().to_owned()]).unwrap();
    take(b);
    let to_b = yo_wire::api::deliver_path(&yo_tests::user_id(b.user_id()));
    transports[0].fault(&to_b, Fault::Answer(202));
    a.add_to_group(&id, &[c.user_id().to_owned()]).unwrap();
    take(c);
    c.send_group(&id, Content::Yo).unwrap();
    assert!(group_yos(&take(b)).is_empty(), "held");
    let saved = b.export(&[4; 32]);
    let mut b = Client::restore(Http::new(hub.url()), &saved, &[4; 32]).unwrap();
    let mut events = Vec::new();
    for _ in 0..2 {
        take(a);
        events.extend(take(&mut b));
    }
    assert!(view(&b, &id).members.contains(&c.user_id().to_owned()), "the ask outlived the restart");
    assert_eq!(group_yos(&events).len(), 1, "and so did the held Yo");
}

/// A member removed cannot bring themselves back: their own change naming a newer revision with them in it is not
/// taken, nor does their Yo naming one make Ben ask them (Ben asks a member).
#[test]
fn a_removed_member_cannot_add_themselves_back() {
    let hub = TestHub::start();
    let mut p = people(&hub, 3);
    let [a, b, c] = p.as_mut_slice() else { unreachable!() };
    let (id, _) = a.create_group("TRIO", &[b.user_id().to_owned(), c.user_id().to_owned()]).unwrap();
    take(b);
    take(c);
    a.remove_from_group(&id, &[c.user_id().to_owned()]).unwrap();
    take(b);
    take(c);
    assert!(!view(b, &id).members.contains(&c.user_id().to_owned()));
    let mut members: Vec<_> = [a.user_id(), b.user_id(), c.user_id()].into_iter().map(yo_tests::user_id).collect();
    members.sort();
    let back = Content::Group(yo_wire::content::GroupInfo {
        id: yo_tests::user_id(&id),
        revision: 9,
        by: yo_tests::user_id(c.user_id()),
        name: "MINE".into(),
        members,
    });
    let ub = b.user_id().to_owned();
    c.send(&ub, back).unwrap();
    take(b);
    let g = view(b, &id);
    assert!(!g.members.contains(&c.user_id().to_owned()) && g.name == "TRIO", "{g:?}");
}

/// A group Yo still waiting for Cleo when Ann removes her does not go to her afterwards.
#[test]
fn a_waiting_group_yo_does_not_go_to_a_member_removed_since() {
    let hub = TestHub::start();
    let base = random_u64() % 1_000_000_000;
    let transports: Vec<_> = (0..3).map(|_| Http::new(hub.url())).collect();
    let mut p: Vec<Client> =
        transports.iter().enumerate().map(|(i, t)| register(t.clone(), &username(base + i as u64 * 7), None)).collect();
    let [a, b, c] = p.as_mut_slice() else { unreachable!() };
    let (id, _) = a.create_group("TRIO", &[b.user_id().to_owned(), c.user_id().to_owned()]).unwrap();
    take(b);
    take(c);
    let to_c = yo_wire::api::deliver_path(&yo_tests::user_id(c.user_id()));
    // Cleo's inbox refuses Ann twice: at the Yo, and at the flush the removal makes first.
    transports[0].fault(&to_c, Fault::Answer(429));
    transports[0].fault(&to_c, Fault::Answer(429));
    a.send_group(&id, Content::Yo).unwrap();
    assert!(a.waiting() > 0, "Cleo's copy waits");
    a.remove_from_group(&id, &[c.user_id().to_owned()]).unwrap();
    let _ = a.sync(0);
    let got = take(c);
    assert!(group_yos(&got).is_empty(), "removed before it went: {got:?}");
    assert_eq!(group_yos(&take(b)).len(), 1);
}

/// One not in the group (removed, or never in it) cannot speak in it by naming a revision newer than Ben's: Ben does
/// not show it, and asks a member instead (spec 4.9). Dan, a friend of Ben's, is not in TRIO.
#[test]
fn one_not_in_the_group_naming_a_newer_revision_is_not_shown() {
    let hub = TestHub::start();
    let mut p = people(&hub, 4);
    let [a, b, c, d] = p.as_mut_slice() else { unreachable!() };
    let (id, _) = a.create_group("TRIO", &[b.user_id().to_owned(), c.user_id().to_owned()]).unwrap();
    take(b);
    take(c);
    let ub = b.user_id().to_owned();
    d.send_yo(&ub).unwrap();
    take(b);
    d.send_group_tagged_for_tests(&ub, &id, 7, Content::Yo).unwrap();
    assert_eq!(d.waiting(), 0, "it went");
    let got = take(b);
    assert!(group_yos(&got).is_empty(), "{got:?}");
}

/// Changes cannot fill a device with groups any more than Yos can (spec 4.9): past 256, a group not known is not
/// taken in, by a change either.
#[test]
fn changes_cannot_fill_a_device_with_groups() {
    let hub = TestHub::start();
    let mut p = people(&hub, 2);
    let [a, b] = p.as_mut_slice() else { unreachable!() };
    let ub = b.user_id().to_owned();
    for i in 0..260u32 {
        let mut members = vec![yo_tests::user_id(a.user_id()), yo_tests::user_id(&ub)];
        members.sort();
        let mut gid = [0u8; 16];
        gid[..4].copy_from_slice(&i.to_be_bytes());
        let change = Content::Group(yo_wire::content::GroupInfo {
            id: gid,
            revision: 1,
            by: yo_tests::user_id(a.user_id()),
            name: format!("G{i}"),
            members,
        });
        a.send(&ub, change).unwrap();
        if i % 50 == 49 {
            take(b);
        }
    }
    take(b);
    assert_eq!(b.groups().len(), 256);
}

/// A newcomer's Yo held until the change adding them comes, and Ben blocks the newcomer meanwhile: the change
/// still counts, the held Yo is not shown (as no Yo of a blocked member is).
#[test]
fn a_held_yo_of_one_blocked_meanwhile_is_not_shown() {
    let hub = TestHub::start();
    let base = random_u64() % 1_000_000_000;
    let transports: Vec<_> = (0..3).map(|_| Http::new(hub.url())).collect();
    let mut p: Vec<Client> =
        transports.iter().enumerate().map(|(i, t)| register(t.clone(), &username(base + i as u64 * 7), None)).collect();
    let [a, b, c] = p.as_mut_slice() else { unreachable!() };
    let (id, _) = a.create_group("ONE", &[b.user_id().to_owned()]).unwrap();
    take(b);
    let to_b = yo_wire::api::deliver_path(&yo_tests::user_id(b.user_id()));
    transports[0].fault(&to_b, Fault::Answer(202));
    a.add_to_group(&id, &[c.user_id().to_owned()]).unwrap();
    take(c);
    c.send_group(&id, Content::Yo).unwrap();
    let mut events = take(b);
    assert!(group_yos(&events).is_empty(), "held");
    b.set_blocked(c.user_id(), true).unwrap();
    for _ in 0..2 {
        take(a);
        events.extend(take(b));
    }
    assert!(view(b, &id).members.contains(&c.user_id().to_owned()), "the change counts");
    assert!(group_yos(&events).is_empty(), "the blocked newcomer's held Yo stays hidden");
}

/// A Yo naming a newer revision makes the group asked of a member as it is here; that member need not be anyone this
/// device has written to: its sessions are looked up, the ask goes out and waits in no outbox (spec 4.9).
#[test]
fn a_group_is_asked_of_a_member_never_written_to() {
    let hub = TestHub::start();
    let mut p = people(&hub, 3);
    p.sort_by(|x, y| x.user_id().cmp(y.user_id()));
    // The lowest member but the asker is one the asker has no session with: Cleo (lowest), not Ann (who wrote).
    let [c, b, a] = p.as_mut_slice() else { unreachable!() };
    let (id, _) = a.create_group("FAMILY", &[b.user_id().to_owned(), c.user_id().to_owned()]).unwrap();
    take(b);
    take(c);
    let mut d = people(&hub, 1).remove(0);
    let b_name = b.username().unwrap().to_owned();
    d.send_yo(&b_name).unwrap();
    take(b);
    d.send_group_tagged_for_tests(b.user_id(), &id, 2, Content::Yo).unwrap();
    take(b);
    b.after_sync().unwrap();
    assert_eq!(b.pending(), 0, "the ask went out");
}

/// Many Yos naming the same newer revision ask for the group once while the ask is unanswered, not once each: the
/// outbox does not grow with them when the asked member cannot be reached.
#[test]
fn a_newer_revision_is_asked_for_once() {
    let hub = TestHub::start();
    let base = random_u64() % 1_000_000_000;
    let hb = Http::new(hub.url());
    let mut b = register(hb.clone(), &username(base), None);
    let mut a = register(Http::new(hub.url()), &username(base + 1), None);
    let mut d = register(Http::new(hub.url()), &username(base + 2), None);
    let (id, _) = a.create_group("FAMILY", &[b.user_id().to_owned()]).unwrap();
    take(&mut b);
    d.send_yo(&username(base)).unwrap();
    take(&mut b);
    // Ann cannot be reached from here now: the ask waits.
    for _ in 0..20 {
        hb.fault(&yo_wire::api::deliver_path(&yo_tests::user_id(a.user_id())), Fault::Answer(503));
    }
    for _ in 0..5 {
        d.send_group_tagged_for_tests(b.user_id(), &id, 2, Content::Yo).unwrap();
    }
    take(&mut b);
    assert_eq!(b.pending(), 1, "one ask");
}
