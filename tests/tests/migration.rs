//! An account of before the keys were libsignal's (state 1, hub schema 2; `fixtures/state-1`, made by the code of
//! then): an iCloud Keychain item of then (the seed) signs in again, and the storage (spec 4.3), sealed under the
//! Ed25519 seed's key, is taken in and written again under the new one.

use yo_core::{identity, storage, Client, Push, Store};
use yo_hub::config::Config;
use yo_tests::{random_u64, Http, TestHub};
use yoip::net::api::{self, status};

const STATE: &[u8] = include_bytes!("../fixtures/state-1/jo.state");
const SEED: &[u8] = include_bytes!("../fixtures/state-1/jo.seed");
const IDS: &str = include_str!("../fixtures/state-1/ids");
const HUB: &str = include_str!("../fixtures/state-1/hub.sql");
const KEY: [u8; 32] = [7; 32];

/// A phone's disk.
struct Nowhere;

impl Store for Nowhere {
    fn save_state(&self, _: &yo_core::state::State, _: yo_core::signal::Entries) -> Result<(), String> {
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
    let stored: api::Storage = yoip::decode(&response.body).unwrap();
    let user = yo_core::state::unhex(client.user_id()).unwrap();
    let opened = storage::open(secret, &user, stored.data.as_slice()).ok()?;
    Some(opened.friends.keys().cloned().collect())
}

#[test]
fn an_icloud_item_of_before_brings_the_friends_back() {
    let (jo_id, pal_id) = ids();
    let seed: [u8; 32] = SEED.try_into().unwrap();
    let secret = identity::legacy_account_secret(&seed);
    let old_key = storage::key(&seed);
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
            None,
        )
        .unwrap()
    };
    // Without the old key the storage of then does not open: left standing at the hub, never written over.
    let without = hub();
    let mut lost = register(&without, None);
    assert_eq!(lost.user_id(), jo_id);
    assert!(lost.friends().is_empty());
    lost.maintenance().unwrap();
    assert_eq!(stored_friends(&mut lost, &secret), None);
    let user = yo_core::state::unhex(lost.user_id()).unwrap();
    let response = lost.signed_for_tests("GET", api::STORAGE, &[]).unwrap();
    let stored: api::Storage = yoip::decode(&response.body).unwrap();
    assert!(storage::open_under(&old_key, &user, stored.data.as_slice()).is_ok(), "the old storage stands");
    // With it, from the item's seed or from a converted local state with an item of now (no seed in it): the
    // friends are back, and the storage is written under the new key.
    let converted = std::env::temp_dir().join(format!("yo-converted-{}", random_u64()));
    std::fs::write(&converted, include_bytes!("../fixtures/keychain-v1/state-v2.sealed")).unwrap();
    let local = yo_core::migrations::keychain_v1::read_identity(&converted, &KEY, 0).unwrap();
    std::fs::remove_file(&converted).unwrap();
    assert_eq!((local.secret, local.ed25519, local.storage_key), (secret, false, Some(old_key)));
    for legacy in [Some(old_key), local.storage_key] {
        let with = hub();
        let mut back = register(&with, legacy);
        assert_eq!(back.user_id(), jo_id);
        assert_eq!(back.friends().iter().map(|f| f.user.as_str()).collect::<Vec<_>>(), [pal_id.as_str()]);
        assert_eq!(stored_friends(&mut back, &secret), Some(vec![pal_id.clone()]));
    }
}

#[test]
fn clean_break_signs_in_each_old_state_and_keychain_item_with_the_same_account_key() {
    use std::{fs, sync::Arc};
    use yo_core::migrations::keychain_v1::legacy_account_secret;
    use yo_core::statedb::{DbStore, Loaded, StateDb};

    let temporary = std::env::temp_dir().join(format!("yo-keychain-fixtures-{}", random_u64()));
    fs::create_dir(&temporary).unwrap();
    let v2 = include_bytes!("../fixtures/keychain-v1/state-v2.sealed");
    let flat_v1 = temporary.join("flat-v1");
    let flat_v2 = temporary.join("flat-v2");
    let flat_shared = temporary.join("flat-shared");
    let single_row = temporary.join("single-row.sqlite");
    let old_tables = temporary.join("tables.sqlite");
    fs::write(&flat_v1, STATE).unwrap();
    fs::write(&flat_v2, v2).unwrap();
    fs::write(&flat_shared, [b"00000000-0000-0000-0000-000000000000".as_slice(), STATE].concat()).unwrap();
    let db = rusqlite::Connection::open(&single_row).unwrap();
    db.execute_batch(include_str!("../fixtures/keychain-v1/state-single-row.sql")).unwrap();
    db.execute("INSERT INTO state VALUES (1, 'fixture', ?1)", [v2.as_slice()]).unwrap();
    drop(db);
    rusqlite::Connection::open(&old_tables)
        .unwrap()
        .execute_batch(include_str!("../fixtures/keychain-v1/state-tables-v1.sql"))
        .unwrap();

    let sources = [(flat_v1, 0), (flat_v2, 0), (flat_shared, 1), (single_row, 2), (old_tables, 3)];
    let local = sources
        .into_iter()
        .map(|(path, format)| {
            yo_core::migrations::keychain_v1::read_identity(&path, &KEY, format).expect("fixture identity")
        })
        .map(|found| {
            // A converted state keeps the key the storage of then is sealed under; a seed's gives it.
            assert_eq!(found.storage_key, Some(storage::key(SEED.try_into().unwrap())));
            (found.secret, found.ed25519)
        });
    let items =
        [include_str!("../fixtures/keychain-v1/item-v1.json"), include_str!("../fixtures/keychain-v1/item-v3.json")]
            .into_iter()
            .map(|json| {
                use base64::{engine::general_purpose::STANDARD, Engine};
                let item: serde_json::Value = serde_json::from_str(json).unwrap();
                let secret = STANDARD.decode(item["accountKey"].as_str().unwrap()).unwrap().try_into().unwrap();
                (secret, item.get("version").is_none())
            });

    let expected_key = identity::public_of(&legacy_account_secret(SEED.try_into().unwrap()));
    let hub = hub();
    let http = Http::new(hub.url());
    for (index, (key, ed25519)) in local.chain(items).enumerate() {
        let seed = ed25519.then_some(key);
        let secret = seed.map_or(key, |seed| legacy_account_secret(&seed));
        let legacy = seed.map(|seed| storage::key(&seed));
        assert_eq!(identity::public_of(&secret), expected_key);
        let report = Arc::new(|_| {});
        let path = temporary.join(format!("fresh-{index}.sqlite"));
        let db = Arc::new(StateDb::open_fresh(&path, KEY, report.clone()).unwrap());
        assert!(matches!(db.load(None, &KEY).unwrap(), Loaded::Gone));
        let store = Arc::new(DbStore::new(db, None, None));
        let client = Client::register_username_saved(
            http.clone(),
            store,
            KEY,
            "Jo",
            Some(secret),
            legacy,
            None,
            Push::none(),
            None,
        )
        .unwrap();
        assert_eq!(client.user_id(), ids().0);
        assert!(!client.is_registering());
        drop(client);
        let db = StateDb::open_fresh(&path, KEY, report).unwrap();
        let Loaded::Newer { state, records, .. } = db.load(None, &KEY).unwrap() else {
            panic!("automatic sign-in must persist the account")
        };
        let (client, _) = Client::from_snapshot(http.clone(), *state, records, false).unwrap();
        assert_eq!(client.user_id(), ids().0);
        assert_eq!(identity::public_of(&client.account_secret()), expected_key);
        assert!(!client.is_registering());
    }

    // Busy (another process holds it, or the file is locked until the phone is unlocked): not now, read again later.
    // Missing or damaged: never there.
    let read =
        |name: &str, format| yo_core::migrations::keychain_v1::read_identity(&temporary.join(name), &KEY, format);
    // Android's one-time sign-in (`legacy_identity`): an old `yo-state` or tables database gives its key material,
    // one without a recovery key too; none is none; one not readable now is asked again, never taken as none.
    let android = |name: &str, format, keys: Vec<Vec<u8>>| {
        yo::legacy_identity(vec![yo::LegacySource {
            path: temporary.join(name).to_string_lossy().into_owned(),
            format,
            keys,
            keys_unavailable: false,
        }])
    };
    for (name, format) in [("flat-v1", 0), ("tables.sqlite", 3)] {
        let found = android(name, format, vec![vec![1; 32], KEY.to_vec()]).unwrap().expect("old identity");
        assert_eq!(found.storage_key, Some(storage::key(SEED.try_into().unwrap()).to_vec()));
    }
    assert!(android("missing", 3, vec![KEY.to_vec()]).unwrap().is_none());
    assert!(android("tables.sqlite", 3, vec![vec![1; 32]]).unwrap().is_none(), "another key opens nothing");
    let holder = rusqlite::Connection::open(temporary.join("single-row.sqlite")).unwrap();
    holder.execute_batch("BEGIN EXCLUSIVE").unwrap();
    assert!(matches!(read("single-row.sqlite", 2), Err(yo_core::Error::Network(_))));
    assert!(matches!(android("single-row.sqlite", 2, vec![KEY.to_vec()]), Err(yo::YoError::Network { .. })));
    drop(holder);
    fs::write(temporary.join("flat-v1"), b"damaged").unwrap();
    assert!(matches!(read("flat-v1", 0), Err(yo_core::Error::State)));
    assert!(matches!(read("missing", 0), Err(yo_core::Error::State)));
    fs::remove_dir_all(temporary).unwrap();
}

/// YoGPT's state file of the build before the database (README.md, CLI): taken into the database offline, the
/// bot goes on as the same device, with its sessions and what waited in its outbox and at the hub.
#[test]
fn a_bot_state_file_goes_on_as_the_same_device() {
    use std::sync::Arc;
    use yo_core::statedb::{DbStore, Loaded, StateDb};
    use yo_core::{Content, Event};
    let hub = TestHub::start();
    let n = random_u64() % 1_000_000;
    let http = Http::new(hub.url());
    let mut bot = Client::register_username(http.clone(), &format!("Bot{n}"), None, None, Push::none()).unwrap();
    let mut pal =
        Client::register_username(Http::new(hub.url()), &format!("Pal{n}"), None, None, Push::none()).unwrap();
    let yos =
        |events: &[Event]| events.iter().filter(|e| matches!(e, Event::Message { content: Content::Yo, .. })).count();
    pal.send_yo(bot.user_id()).unwrap();
    assert_eq!(yos(&bot.sync(0).unwrap()), 1);
    // One Yo waits in the bot's outbox, one for it at the hub.
    http.fault(&api::deliver_path(&yo_tests::user_id(pal.user_id())), yo_tests::Fault::Answer(503));
    bot.acknowledge_events();
    let _ = bot.send_yo(pal.user_id());
    assert_eq!(bot.waiting(), 1);
    pal.send_yo(bot.user_id()).unwrap();
    // The file as that build wrote it: a list of devices for each contact and outbox item.
    let saved = bot.saved();
    let mut json = serde_json::to_value(&saved.state).unwrap();
    // A Yo to the bot not handed on yet, and the copy of one it sent (`own`, kept by that build): no Yo to answer.
    let message =
        |own| serde_json::json!({ "Message": { "user": pal.user_id(), "sent_at_ms": 1, "content": "Yo", "own": own } });
    json["events"] = serde_json::json!([message(false), message(true)]);
    let blob = yo_core::state::seal_legacy(&serde_json::to_vec(&json).unwrap(), &saved.records, &KEY);
    drop(bot);

    let (state, records) = yo_core::migrations::keychain_v1::read_state(&blob, &KEY).unwrap();
    let path = std::env::temp_dir().join(format!("yo-bot-{}.sqlite", random_u64()));
    let db = Arc::new(StateDb::open_fresh(&path, KEY, Arc::new(|_| {})).unwrap());
    DbStore::new(db.clone(), None, None).save_state(&state, records).unwrap();
    let Loaded::Newer { word, state, records } = db.load(None, &KEY).unwrap() else { panic!("saved") };
    let (mut again, _) = Client::from_snapshot(Http::new(hub.url()), *state, records, false).unwrap();
    again.persist_to(Arc::new(DbStore::new(db, Some(word), None)), KEY).unwrap();
    assert_eq!(again.saved().state.device_key, saved.state.device_key);
    assert_eq!(yos(&again.sync(0).unwrap()), 2, "the one waiting and the hub's: {:?}", again.drop_reasons());
    again.maintenance().unwrap();
    assert_eq!(again.waiting(), 0);
    assert_eq!(yos(&pal.sync(0).unwrap()), 1, "{:?}", pal.drop_reasons());
    let _ = std::fs::remove_file(path);
}
