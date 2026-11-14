//! The account's storage at the hub (spec 6.9): friends, blocks and the app's icon, sealed under a key from the
//! account key; back on a new phone before the first message, merged when two writers meet, unreadable to the hub.

use yo_core::storage::{self, Storage};
use yo_core::{Client, Content, Error, Event, Push};
use yo_tests::{random_u64, Http, TestHub};
use yo_wire::api::{self, status};

fn by_name(hub: &TestHub, name: &str, secret: Option<[u8; 32]>) -> Client {
    Client::register_username(Http::new(hub.url()), name, secret, None, Push::none()).unwrap()
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

fn stored(client: &mut Client) -> api::Storage {
    let response = client.signed_for_tests("GET", api::STORAGE, &[]).unwrap();
    assert_eq!(response.status, status::OK);
    yo_wire::decode(&response.body).unwrap()
}

#[test]
fn a_new_phone_gets_friends_blocks_and_icon_back_before_the_first_message() {
    let hub = TestHub::start();
    let n = random_u64() % 100_000;
    let mut me = by_name(&hub, &format!("Me{n}"), None);
    let mut pest = by_name(&hub, &format!("Pest{n}"), None);
    let mut pal = by_name(&hub, &format!("Pal{n}"), None);
    pest.send(me.user_id(), Content::Text("hi".into())).unwrap();
    pal.send(me.user_id(), Content::Text("hey".into())).unwrap();
    me.sync(0).unwrap();
    me.set_friend(pal.user_id(), Some("Best Pal".into()), Some(3)).unwrap();
    me.set_friend(pest.user_id(), None, Some(1)).unwrap();
    me.set_blocked(pest.user_id(), true).unwrap();
    me.set_app_icon(Some("Negative".into())).unwrap();
    me.maintenance().unwrap();
    // The hub has it, sealed: not a name in it.
    let sealed = stored(&mut me);
    assert_eq!(sealed.version, 1);
    assert!(!sealed.data.as_slice().windows(8).any(|w| w == b"Best Pal"));
    assert!(!sealed.data.as_slice().windows(8).any(|w| w == &pal.user_id().as_bytes()[..8]));

    // The pest writes while the phone is lost; a new phone restores the account from its key.
    pest.send(me.user_id(), Content::Text("let me in".into())).unwrap();
    pal.send(me.user_id(), Content::Text("still here".into())).unwrap();
    let mut new = by_name(&hub, &format!("Me{n}"), Some(me.account_secret()));
    assert_eq!(new.user_id(), me.user_id());
    let mut friends = new.friends();
    friends.sort_by_key(|f| f.name.is_none());
    assert_eq!(friends.len(), 2);
    assert_eq!((friends[0].name.as_deref(), friends[0].color, friends[0].blocked), (Some("Best Pal"), Some(3), false));
    assert_eq!((friends[1].user.as_str(), friends[1].color, friends[1].blocked), (pest.user_id(), Some(1), true));
    assert_eq!(new.app_icon().as_deref(), Some("Negative"));
    assert!(new.contacts().iter().any(|c| c.user == pest.user_id() && c.blocked));
    // From the first message on: the pest's (in a chat the new phone joins now) is not shown, the pal's is.
    pest.send(me.user_id(), Content::Text("again".into())).unwrap();
    pal.send(me.user_id(), Content::Text("hello new phone".into())).unwrap();
    let mut seen = Vec::new();
    for _ in 0..3 {
        seen.extend(texts(&new.sync(0).unwrap()));
    }
    assert_eq!(seen, ["hello new phone"], "{seen:?}");
    assert_eq!(new.send(pest.user_id(), Content::Yo), Err(Error::Blocked));
    // The old phone is out: its storage writes are refused.
    me.set_app_icon(Some("Original".into())).unwrap();
    assert_eq!(me.maintenance(), Err(Error::Replaced));
    assert_eq!(new.app_icon().as_deref(), Some("Negative"));
}

#[test]
fn two_writers_merge_and_converge() {
    let hub = TestHub::start();
    let n = random_u64() % 100_000;
    let name = format!("Two{n}");
    let mut a = by_name(&hub, &name, None);
    a.set_friend(&"11".repeat(16), Some("Leo".into()), Some(1)).unwrap();
    a.set_app_icon(Some("Original".into())).unwrap();
    a.maintenance().unwrap();
    // Another writer of the account (a second device, later; here by hand, with the same key) writes on top of the
    // hub's version: a friend of its own, Leo renamed, the icon back to the default.
    let user = yo_core::state::unhex(a.user_id()).unwrap();
    let mut theirs = storage::open(&a.account_secret(), &user, stored(&mut a).data.as_slice()).unwrap();
    let later = yo_hub::now_ms() + 1000;
    theirs.friends.entry("22".repeat(16)).or_default().name = storage::Field { value: Some("Mia".into()), at: later };
    theirs.friends.get_mut(&"11".repeat(16)).unwrap().name = storage::Field { value: Some("Leon".into()), at: later };
    theirs.icon = storage::Field { value: None, at: later };
    theirs.version = 2; // as the hub will store it
    let body = yo_wire::encode(&api::Storage {
        version: 1,
        data: storage::seal(&a.account_secret(), &user, &theirs).unwrap().into(),
    });
    assert_eq!(a.signed_for_tests("PUT", api::STORAGE, &body).unwrap().status, status::NO_CONTENT);
    // A writes on its old version: 409, merged, written again.
    a.set_friend(&"33".repeat(16), Some("Zoë".into()), None).unwrap();
    a.set_friend(&"11".repeat(16), Some("Leo".into()), Some(5)).unwrap();
    a.maintenance().unwrap();
    let at_hub = stored(&mut a);
    assert_eq!(at_hub.version, 3);
    let merged: Storage = storage::open(&a.account_secret(), &user, at_hub.data.as_slice()).unwrap();
    let name_of = |u: &str| merged.friends[&u.repeat(16)].name.value.clone();
    assert_eq!(
        (name_of("11"), name_of("22"), name_of("33")),
        (Some("Leon".into()), Some("Mia".into()), Some("Zoë".into()))
    );
    assert_eq!(merged.friends[&"11".repeat(16)].color.value, Some(5), "each field the later one");
    assert_eq!(merged.icon.value, None);
    assert_eq!(a.friends().len(), 3, "A has the merge too");
    // A stale write of the version before is refused with the current one.
    let stale = yo_wire::encode(&api::Storage { version: 2, data: vec![1; 100].into() });
    let response = a.signed_for_tests("PUT", api::STORAGE, &stale).unwrap();
    assert_eq!(response.status, status::CONFLICT);
    assert_eq!(yo_wire::decode::<api::Storage>(&response.body).unwrap().version, 3);
    // Too large, or empty: refused.
    let large =
        yo_wire::encode(&api::Storage { version: 3, data: vec![1; yo_wire::limits::MAX_STORAGE_LEN + 1].into() });
    assert_eq!(a.signed_for_tests("PUT", api::STORAGE, &large).unwrap().status, status::TOO_LARGE);
    let empty = yo_wire::encode(&api::Storage { version: 3, data: Vec::new().into() });
    assert_eq!(a.signed_for_tests("PUT", api::STORAGE, &empty).unwrap().status, status::BAD_REQUEST);
}

#[test]
fn the_storage_goes_with_the_account() {
    let hub = TestHub::start();
    let n = random_u64() % 100_000;
    let name = format!("Gone{n}");
    let mut a = by_name(&hub, &name, None);
    a.set_app_icon(Some("Negative".into())).unwrap();
    a.maintenance().unwrap();
    a.delete_account().unwrap();
    assert_eq!(hub.hub.audit().unwrap(), [], "nothing left behind");
    // Whoever takes the name next starts with nothing.
    let mut b = by_name(&hub, &name, None);
    assert_eq!(b.signed_for_tests("GET", api::STORAGE, &[]).unwrap().status, status::NOT_FOUND);
    assert_eq!(b.app_icon(), None);
}

#[test]
fn a_replayed_storage_is_not_taken() {
    let hub = TestHub::start();
    let name = format!("Replay{}", random_u64() % 100_000);
    let mut a = by_name(&hub, &name, None);
    a.set_app_icon(Some("Negative".into())).unwrap();
    a.maintenance().unwrap();
    // The hub (or someone at it) puts the old blob, sealed as version 1, under version 2.
    let old = stored(&mut a).data;
    let body = yo_wire::encode(&api::Storage { version: 1, data: old });
    assert_eq!(a.signed_for_tests("PUT", api::STORAGE, &body).unwrap().status, status::NO_CONTENT);
    let new = by_name(&hub, &name, Some(a.account_secret()));
    assert_eq!(new.app_icon(), None, "not taken: sealed as another version");
    assert!(new.drop_reasons().iter().any(|r| r.contains("storage version")));
    assert!(a.set_friend(&"11".repeat(16), Some("x".repeat(65)), None).is_err(), "a name too long");
}

#[test]
fn saved_says_whether_the_hub_holds_it_all() {
    let hub = TestHub::start();
    let mut a = by_name(&hub, &format!("Saved{}", random_u64() % 100_000), None);
    a.maintenance().unwrap();
    let first = a.storage_saved_at().expect("a new account's empty storage is saved");
    a.set_friend(&"11".repeat(16), Some("Leo".into()), None).unwrap();
    assert_eq!(a.storage_saved_at(), None, "a change waits to go up");
    a.maintenance().unwrap();
    assert!(a.storage_saved_at().unwrap() >= first, "written");
}
