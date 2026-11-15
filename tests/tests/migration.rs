//! An account of before the keys were libsignal's (state 1, hub schema 2; `fixtures/state-1`, made by the code of
//! then): the phone's state opens as it is, keeps its device and its friends, and the storage (spec 6.9), sealed under
//! the Ed25519 seed's key, is taken in and written again under the new one. An iCloud Keychain item of then (the seed)
//! brings the friends back too.

use yo_core::{identity, storage, Client, Push, Store};
use yo_hub::config::Config;
use yo_tests::{random_u64, Http, TestHub};
use yo_wire::api::{self, status};

const STATE: &[u8] = include_bytes!("../fixtures/state-1/jo.state");
const SEED: &[u8] = include_bytes!("../fixtures/state-1/jo.seed");
const IDS: &str = include_str!("../fixtures/state-1/ids");
const HUB: &str = include_str!("../fixtures/state-1/hub.sql");
const KEY: [u8; 32] = [7; 32];

/// A phone's disk.
struct Nowhere;

impl Store for Nowhere {
    fn save(&self, _: Vec<u8>) -> Result<(), String> {
        Ok(())
    }
}

/// A hub on the database of then (schema 2, brought up to 3 as it opens).
fn hub() -> TestHub {
    let path = std::env::temp_dir().join(format!("yo-state-1-{}-{}.sqlite", std::process::id(), random_u64()));
    rusqlite::Connection::open(&path).unwrap().execute_batch(HUB).unwrap();
    let mut config = Config::for_tests();
    config.db_path = path.to_string_lossy().into_owned();
    TestHub::start_with(config, "127.0.0.1:0")
}

fn ids() -> (String, String) {
    let (jo, pal) = IDS.trim().split_once(' ').unwrap();
    (jo.to_owned(), pal.to_owned())
}

/// The storage at the hub opened under `secret`'s key alone.
fn stored_friends(client: &mut Client, secret: &[u8; 32]) -> Option<Vec<String>> {
    let response = client.signed_for_tests("GET", api::STORAGE, &[]).unwrap();
    assert_eq!(response.status, status::OK);
    let stored: api::Storage = yo_wire::decode(&response.body).unwrap();
    let user = yo_core::state::unhex(client.user_id()).unwrap();
    let opened = storage::open(secret, &user, stored.data.as_slice()).ok()?;
    Some(opened.friends.keys().cloned().collect())
}

#[test]
fn a_state_of_before_opens_with_its_device_and_friends() {
    let hub = hub();
    let (jo_id, pal_id) = ids();
    let mut jo = Client::restore(Http::new(hub.url()), STATE, &KEY).expect("the state of before opens");
    let device = jo.device_id();
    assert_eq!(jo.user_id(), jo_id);
    assert_eq!(
        jo.friends().iter().map(|f| (f.user.as_str(), f.name.as_deref())).collect::<Vec<_>>(),
        [(pal_id.as_str(), Some("Best Pal"))]
    );
    // Its device signs as before (the hub converted its auth key): not replaced, nothing registered anew.
    jo.sync(0).expect("signed requests are taken");
    jo.maintenance().unwrap();
    assert_eq!(jo.device_id(), device);
    // The storage is under the new secret's key now: a new phone with the account's key alone reads it.
    let secret = jo.account_secret();
    assert_eq!(stored_friends(&mut jo, &secret), Some(vec![pal_id.clone()]));
    let new = Client::register_username(Http::new(hub.url()), "Jo", Some(secret), None, Push::none()).unwrap();
    assert_eq!(new.user_id(), jo_id);
    assert_eq!(new.friends().len(), 1);
}

#[test]
fn an_icloud_item_of_before_brings_the_friends_back() {
    let (jo_id, pal_id) = ids();
    let seed: [u8; 32] = SEED.try_into().unwrap();
    let secret = identity::legacy_account_secret(&seed);
    let register = |hub: &TestHub, legacy: Option<[u8; 32]>| {
        let store = std::sync::Arc::new(Nowhere);
        Client::register_username_saved(
            Http::new(hub.url()),
            store,
            KEY,
            "Jo",
            Some(secret),
            legacy,
            None,
            Push::none(),
        )
        .unwrap()
    };
    // Without the seed the storage of then does not open: the friends were lost so.
    let without = hub();
    let lost = register(&without, None);
    assert_eq!(lost.user_id(), jo_id);
    assert!(lost.friends().is_empty());
    // With it (the item keeps it, `legacySeed`): the friends are back, and the storage is written under the new key.
    let with = hub();
    let mut back = register(&with, Some(seed));
    assert_eq!(back.user_id(), jo_id);
    assert_eq!(back.friends().iter().map(|f| f.user.as_str()).collect::<Vec<_>>(), [pal_id.as_str()]);
    assert_eq!(stored_friends(&mut back, &secret), Some(vec![pal_id]));
}
