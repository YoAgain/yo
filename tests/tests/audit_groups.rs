//! Groups against a stranger or a hostile member (security audit 2026-11-23: G1, G2, G3, G5, I5): a group of a
//! stranger's making reaches no one further than the stranger writing to them would, and with requests off not at
//! all; a group left is no yes; what a member writes into a group cannot break it for the others.

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

/// A hub with its database in a file the test opens beside it, that file's connection and directory.
fn hub_on_disk() -> (TestHub, rusqlite::Connection, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("yo-audit-groups-{}", random_u64()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut config = yo_hub::config::Config::for_tests();
    config.db_path = dir.join("hub.sqlite").to_string_lossy().into_owned();
    let hub = TestHub::start_with(config.clone(), "127.0.0.1:0");
    let db = rusqlite::Connection::open(&config.db_path).unwrap();
    db.busy_timeout(std::time::Duration::from_secs(10)).unwrap();
    (hub, db, dir)
}

fn unhex(id: &str) -> Vec<u8> {
    (0..id.len()).step_by(2).map(|i| u8::from_str_radix(&id[i..i + 2], 16).unwrap()).collect()
}

fn stored(db: &rusqlite::Connection, id: &str) -> yo_wire::api::GroupState {
    let state: Vec<u8> = db.query_row("SELECT state FROM zk_groups WHERE id = ?1", [unhex(id)], |r| r.get(0)).unwrap();
    yo_wire::decode(&state).unwrap()
}

fn store(db: &rusqlite::Connection, id: &str, state: &yo_wire::api::GroupState) {
    db.execute("UPDATE zk_groups SET state = ?2 WHERE id = ?1", (unhex(id), yo_wire::encode(state))).unwrap();
}

fn view(c: &Client, id: &str) -> yo_core::groups::GroupView {
    c.groups().into_iter().find(|g| g.id == id).expect("the group")
}

/// Group requests `of` sent for the group `id` (fetches and changes).
fn group_requests(of: &Http, id: &str) -> usize {
    let path = yo_wire::api::group_path(&unhex(id).try_into().unwrap());
    of.sent.lock().unwrap().iter().filter(|(_, p, _)| *p == path).count()
}

/// Audit G2. Spec 4.10: "one a stranger added it to is a request until a yes (leaving says no)". A group left and
/// then added back to by a stranger (whose key of this account is old: invited) is a request again, not joined; added
/// back by a contact it is joined at once.
#[test]
fn a_left_group_added_back_by_a_stranger_is_a_request_again() {
    let hub = TestHub::start();
    let base = random_u64() % 1_000_000_000;
    let mut a = register(Http::new(hub.url()), &username(base), Some("ANN"));
    let mut b = register(Http::new(hub.url()), &username(base + 7), Some("BEN"));
    let mut m = register(Http::new(hub.url()), &username(base + 14), Some("MAL"));
    let mut x = register(Http::new(hub.url()), &username(base + 21), Some("XAV"));
    // Ann and Ben are contacts, so are Xav and Ben; Mallory is neither's.
    a.send_yo(&username(base + 7)).unwrap();
    b.sync(0).unwrap();
    b.accept(a.user_id()).unwrap();
    x.send_yo(&username(base + 7)).unwrap();
    b.sync(0).unwrap();
    b.accept(x.user_id()).unwrap();
    for c in [&mut a, &mut b, &mut x] {
        rounds(c);
    }
    let (id, _) = a.create_group("CREW", &[b.user_id().to_owned(), m.user_id().to_owned()]).unwrap();
    take(&mut b);
    take(&mut m);
    assert!(view(&b, &id).accepted, "added by a contact: joined");
    b.leave_group(&id).unwrap();
    take(&mut a);
    take(&mut m);
    let left = view(&b, &id);
    assert!(left.left && !left.accepted, "leaving says no");
    // Ben blocks someone else: a new profile key, which goes to accepted contacts and joined groups, not to Mallory.
    b.set_blocked(x.user_id(), true).unwrap();
    rounds(&mut b);
    take(&mut m);
    m.add_to_group(&id, &[b.user_id().to_owned()]).unwrap();
    take(&mut b);
    let g = view(&b, &id);
    assert!(!g.left, "back in");
    assert!(!g.accepted, "added back by a stranger: a request again");
    // His no: he leaves again; Ann (a contact) adds him back: joined at once.
    b.leave_group(&id).unwrap();
    take(&mut a);
    a.add_to_group(&id, &[b.user_id().to_owned()]).unwrap();
    take(&mut b);
    let g = view(&b, &id);
    assert!(!g.left && g.accepted, "added back by a contact: joined");
    a.send_group(&id, Content::Yo).unwrap();
    assert_eq!(group_yos(&take(&mut b), &a), 1);
}

/// Audit G3. A member adds an entry no member's client can open (here written into the stored state: a
/// `UuidCiphertext` of another group, as the hub's unit test shows the hub takes over the API). It is skipped: a
/// member who had not fetched yet still has the group, members can change it, Yos go, and the entry is removed by
/// the next member's change.
#[test]
fn one_unreadable_member_entry_is_skipped_and_removed() {
    let (hub, db, dir) = hub_on_disk();
    let base = random_u64() % 1_000_000_000;
    let mut a = register(Http::new(hub.url()), &username(base), Some("ANN"));
    let mut b = register(Http::new(hub.url()), &username(base + 7), Some("BEN"));
    let c = register(Http::new(hub.url()), &username(base + 14), Some("CLE"));
    let (other, _) = a.create_group("OTHER", &[c.user_id().to_owned()]).unwrap();
    let foreign = stored(&db, &other).members[0].clone();
    let (id, _) = a.create_group("CREW", &[b.user_id().to_owned()]).unwrap();
    let mut s = stored(&db, &id);
    s.members.push(foreign);
    store(&db, &id, &s);
    // Ben, told of the group, fetches it: two members, the entry skipped.
    take(&mut b);
    let g = view(&b, &id);
    assert!(!g.left, "{:?}", b.drop_reasons());
    assert_eq!(g.members.len(), 2);
    b.accept_group(&id).unwrap();
    // His yes was a change: the entry went with it.
    assert_eq!(stored(&db, &id).members.len(), 2, "removed by the next change");
    // Ann can change it and Yo into it.
    take(&mut a);
    a.rename_group(&id, "FIXED").unwrap();
    a.send_group(&id, Content::Yo).unwrap();
    let events = take(&mut b);
    assert_eq!(group_yos(&events, &a), 1);
    assert_eq!(view(&b, &id).name, "FIXED");
    // Written in again: a joined member removes it after its next read, without any change of its own.
    let mut s = stored(&db, &id);
    s.members.push(stored(&db, &other).members[0].clone());
    s.revision += 1;
    store(&db, &id, &s);
    db.execute("UPDATE zk_groups SET revision = ?2 WHERE id = ?1", (unhex(&id), s.revision)).unwrap();
    a.rename_group(&id, "AGAIN").unwrap(); // a 409 on the stale revision, made again on the hub's: and purged
    assert_eq!(stored(&db, &id).members.len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Audit G5. A member's tag naming revision `u32::MAX`, and a non-member's: each makes the group fetched once, not at
/// every inbox read after.
#[test]
fn a_tag_naming_a_revision_that_is_not_there_makes_one_fetch() {
    let hub = TestHub::start();
    let base = random_u64() % 1_000_000_000;
    let ha = Http::new(hub.url());
    let mut a = register(ha.clone(), &username(base), Some("ANN"));
    let mut b = register(Http::new(hub.url()), &username(base + 7), Some("BEN"));
    let mut x = register(Http::new(hub.url()), &username(base + 14), Some("XAV"));
    // Ann and Xav are contacts, Xav in no group.
    x.send_yo(a.user_id()).unwrap();
    take(&mut a);
    a.accept(x.user_id()).unwrap();
    for c in [&mut a, &mut x] {
        rounds(c);
    }
    let (id, _) = a.create_group("CREW", &[b.user_id().to_owned()]).unwrap();
    take(&mut b);
    let key: [u8; 32] = unhex(&b.saved().state.groups[&id].key).try_into().unwrap();
    for from in [&mut b, &mut x] {
        let lie = Content::Group(yo_wire::content::GroupTag { key, revision: u32::MAX });
        from.send(a.user_id(), lie).unwrap();
        let before = group_requests(&ha, &id);
        rounds(&mut a);
        assert_eq!(group_requests(&ha, &id), before + 1, "fetched once");
        for _ in 0..3 {
            rounds(&mut a);
        }
        assert_eq!(group_requests(&ha, &id), before + 1, "and not again");
        assert_eq!(view(&a, &id).revision, 1);
    }
}

/// Audit I5. A title that is no name (a member's client could write any bytes; here written into the stored state:
/// control characters, too long, not UTF-8) is taken as none; a good one as it is.
#[test]
fn a_title_that_is_no_name_is_none() {
    let (hub, db, dir) = hub_on_disk();
    let base = random_u64() % 1_000_000_000;
    let mut a = register(Http::new(hub.url()), &username(base), Some("ANN"));
    let mut b = register(Http::new(hub.url()), &username(base + 7), Some("BEN"));
    for (title, shown) in [(&b"A\nB"[..], ""), (&[b'X'; 40][..], ""), (&[0xff, 0xfe][..], ""), (&b"GOOD"[..], "GOOD")] {
        let (id, _) = a.create_group("CREW", &[b.user_id().to_owned()]).unwrap();
        let key: [u8; 32] = unhex(&a.saved().state.groups[&id].key).try_into().unwrap();
        let secret =
            zkgroup::groups::GroupSecretParams::derive_from_master_key(zkgroup::groups::GroupMasterKey::new(key));
        let mut s = stored(&db, &id);
        s.title = secret.encrypt_blob_with_padding([7; 32], title, 0).into();
        store(&db, &id, &s);
        take(&mut b);
        assert_eq!(view(&b, &id).name, shown, "{title:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
